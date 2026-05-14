/********************************************************************************
 * Copyright (c) 2026 Contributors to the Eclipse Foundation
 *
 * See the NOTICE file(s) distributed with this work for additional
 * information regarding copyright ownership.
 *
 * This program and the accompanying materials are made available under the
 * terms of the Apache License Version 2.0 which is available at
 * https://www.apache.org/licenses/LICENSE-2.0
 *
 * SPDX-License-Identifier: Apache-2.0
 ********************************************************************************/

use std::{marker::PhantomData, sync::Arc};

use async_trait::async_trait;
use tokio::{sync::mpsc, task::JoinHandle};
use up_rust::{
    UOwnedFrame, UOwnedListener, UOwnedTransport, UStatus, UTxBuffer, UUri, UZeroCopyListener,
    UZeroCopyRxFrame, UZeroCopyTransport,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransportMode {
    Owned,
    ZeroCopy,
}

#[async_trait]
pub trait NativeFrameEndpoint: Send + Sync {
    fn mode(&self) -> TransportMode;

    async fn send_frame(&self, frame: UOwnedFrame) -> Result<(), UStatus>;

    async fn register_ingress(
        &self,
        source_filter: &UUri,
        sink_filter: Option<&UUri>,
        tx: mpsc::UnboundedSender<UOwnedFrame>,
    ) -> Result<(), UStatus>;
}

pub struct OwnedEndpoint<T> {
    transport: Arc<T>,
}

impl<T> OwnedEndpoint<T> {
    pub fn new(transport: Arc<T>) -> Self {
        Self { transport }
    }
}

#[async_trait]
impl<T> NativeFrameEndpoint for OwnedEndpoint<T>
where
    T: UOwnedTransport + Send + Sync,
{
    fn mode(&self) -> TransportMode {
        TransportMode::Owned
    }

    async fn send_frame(&self, frame: UOwnedFrame) -> Result<(), UStatus> {
        self.transport.send_owned(frame).await
    }

    async fn register_ingress(
        &self,
        source_filter: &UUri,
        sink_filter: Option<&UUri>,
        tx: mpsc::UnboundedSender<UOwnedFrame>,
    ) -> Result<(), UStatus> {
        self.transport
            .register_owned_listener(source_filter, sink_filter, Arc::new(OwnedIngress { tx }))
            .await
    }
}

struct OwnedIngress {
    tx: mpsc::UnboundedSender<UOwnedFrame>,
}

#[async_trait]
impl UOwnedListener for OwnedIngress {
    async fn on_receive_owned(&self, frame: UOwnedFrame) {
        let _ = self.tx.send(frame);
    }
}

pub struct ZeroCopyEndpoint<T> {
    transport: Arc<T>,
}

impl<T> ZeroCopyEndpoint<T> {
    pub fn new(transport: Arc<T>) -> Self {
        Self { transport }
    }
}

#[async_trait]
impl<T> NativeFrameEndpoint for ZeroCopyEndpoint<T>
where
    T: UZeroCopyTransport + Send + Sync,
{
    fn mode(&self) -> TransportMode {
        TransportMode::ZeroCopy
    }

    async fn send_frame(&self, frame: UOwnedFrame) -> Result<(), UStatus> {
        let payload_len = frame.payload().len();
        let mut buffer = self
            .transport
            .reserve(frame.header().clone(), payload_len, 1)
            .await?;
        buffer.payload_mut().copy_from_slice(frame.payload());
        self.transport.send_zero_copy(buffer).await
    }

    async fn register_ingress(
        &self,
        source_filter: &UUri,
        sink_filter: Option<&UUri>,
        tx: mpsc::UnboundedSender<UOwnedFrame>,
    ) -> Result<(), UStatus> {
        self.transport
            .register_zero_copy_listener(
                source_filter,
                sink_filter,
                Arc::new(ZeroCopyIngress::<T::Rx> {
                    tx,
                    _rx: PhantomData,
                }),
            )
            .await
    }
}

struct ZeroCopyIngress<Rx> {
    tx: mpsc::UnboundedSender<UOwnedFrame>,
    _rx: PhantomData<fn() -> Rx>,
}

#[async_trait]
impl<Rx> UZeroCopyListener<Rx> for ZeroCopyIngress<Rx>
where
    Rx: UZeroCopyRxFrame + Send + 'static,
{
    async fn on_receive_zero_copy(&self, frame: Rx) {
        let _ = self.tx.send(UOwnedFrame::new(
            frame.header().clone(),
            frame.payload().to_vec(),
        ));
    }
}

pub struct NativeFrameStreamer {
    tasks: Vec<JoinHandle<()>>,
}

impl NativeFrameStreamer {
    pub fn new() -> Self {
        Self { tasks: Vec::new() }
    }

    pub async fn route(
        &mut self,
        ingress: Arc<dyn NativeFrameEndpoint>,
        egress: Arc<dyn NativeFrameEndpoint>,
        source_filter: &UUri,
        sink_filter: Option<&UUri>,
    ) -> Result<(), UStatus> {
        let (tx, mut rx) = mpsc::unbounded_channel();
        ingress
            .register_ingress(source_filter, sink_filter, tx)
            .await?;
        self.tasks.push(tokio::spawn(async move {
            while let Some(frame) = rx.recv().await {
                let _ = egress.send_frame(frame).await;
            }
        }));
        Ok(())
    }
}

impl Default for NativeFrameStreamer {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for NativeFrameStreamer {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use up_rust::{RawBytes, UFrameHeader, UVecTxBuffer, UZeroCopyTransportExt, WireFormat};

    use super::*;

    #[derive(Default)]
    struct MemoryOwnedTransport {
        listeners: Mutex<Vec<Arc<dyn UOwnedListener>>>,
        sent: Mutex<Vec<UOwnedFrame>>,
    }

    impl MemoryOwnedTransport {
        async fn inject(&self, frame: UOwnedFrame) {
            let listeners = self
                .listeners
                .lock()
                .expect("listeners lock poisoned")
                .clone();
            for listener in listeners {
                listener.on_receive_owned(frame.clone()).await;
            }
        }

        fn sent(&self) -> Vec<UOwnedFrame> {
            self.sent.lock().expect("sent lock poisoned").clone()
        }
    }

    #[async_trait]
    impl UOwnedTransport for MemoryOwnedTransport {
        async fn send_owned(&self, frame: UOwnedFrame) -> Result<(), UStatus> {
            self.sent.lock().expect("sent lock poisoned").push(frame);
            Ok(())
        }

        async fn register_owned_listener(
            &self,
            _source_filter: &UUri,
            _sink_filter: Option<&UUri>,
            listener: Arc<dyn UOwnedListener>,
        ) -> Result<(), UStatus> {
            self.listeners
                .lock()
                .expect("listeners lock poisoned")
                .push(listener);
            Ok(())
        }
    }

    #[derive(Default)]
    struct MemoryZeroCopyTransport {
        listeners: Mutex<Vec<Arc<dyn UZeroCopyListener<UOwnedFrame>>>>,
        sent: Mutex<Vec<UOwnedFrame>>,
    }

    impl MemoryZeroCopyTransport {
        async fn inject(&self, frame: UOwnedFrame) {
            let listeners = self
                .listeners
                .lock()
                .expect("listeners lock poisoned")
                .clone();
            for listener in listeners {
                listener.on_receive_zero_copy(frame.clone()).await;
            }
        }

        fn sent(&self) -> Vec<UOwnedFrame> {
            self.sent.lock().expect("sent lock poisoned").clone()
        }
    }

    #[async_trait]
    impl UZeroCopyTransport for MemoryZeroCopyTransport {
        type Tx = UVecTxBuffer;
        type Rx = UOwnedFrame;

        async fn reserve(
            &self,
            header: UFrameHeader,
            payload_len: usize,
            _alignment: usize,
        ) -> Result<Self::Tx, UStatus> {
            Ok(UVecTxBuffer::new(header, payload_len))
        }

        async fn send_zero_copy(&self, buffer: Self::Tx) -> Result<(), UStatus> {
            self.sent
                .lock()
                .expect("sent lock poisoned")
                .push(buffer.into_frame());
            Ok(())
        }

        async fn register_zero_copy_listener(
            &self,
            _source_filter: &UUri,
            _sink_filter: Option<&UUri>,
            listener: Arc<dyn UZeroCopyListener<Self::Rx>>,
        ) -> Result<(), UStatus> {
            self.listeners
                .lock()
                .expect("listeners lock poisoned")
                .push(listener);
            Ok(())
        }
    }

    fn topic() -> UUri {
        UUri::try_from_parts("streamer-test", 0x4210, 1, 0x9001).expect("invalid topic")
    }

    fn frame() -> UOwnedFrame {
        UOwnedFrame::new(
            UFrameHeader::publish(topic()).with_encoding(RawBytes::encoding()),
            b"streamed".as_slice(),
        )
    }

    async fn yield_to_forwarder() {
        tokio::task::yield_now().await;
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }

    #[tokio::test]
    async fn routes_owned_to_owned() {
        let ingress = Arc::new(MemoryOwnedTransport::default());
        let egress = Arc::new(MemoryOwnedTransport::default());
        let mut streamer = NativeFrameStreamer::new();
        streamer
            .route(
                Arc::new(OwnedEndpoint::new(ingress.clone())),
                Arc::new(OwnedEndpoint::new(egress.clone())),
                &topic(),
                None,
            )
            .await
            .expect("route failed");

        ingress.inject(frame()).await;
        yield_to_forwarder().await;

        assert_eq!(egress.sent()[0].payload_bytes(), b"streamed");
    }

    #[tokio::test]
    async fn routes_owned_to_zero_copy() {
        let ingress = Arc::new(MemoryOwnedTransport::default());
        let egress = Arc::new(MemoryZeroCopyTransport::default());
        let mut streamer = NativeFrameStreamer::new();
        streamer
            .route(
                Arc::new(OwnedEndpoint::new(ingress.clone())),
                Arc::new(ZeroCopyEndpoint::new(egress.clone())),
                &topic(),
                None,
            )
            .await
            .expect("route failed");

        ingress.inject(frame()).await;
        yield_to_forwarder().await;

        assert_eq!(egress.sent()[0].payload_bytes(), b"streamed");
    }

    #[tokio::test]
    async fn routes_zero_copy_to_owned() {
        let ingress = Arc::new(MemoryZeroCopyTransport::default());
        let egress = Arc::new(MemoryOwnedTransport::default());
        let mut streamer = NativeFrameStreamer::new();
        streamer
            .route(
                Arc::new(ZeroCopyEndpoint::new(ingress.clone())),
                Arc::new(OwnedEndpoint::new(egress.clone())),
                &topic(),
                None,
            )
            .await
            .expect("route failed");

        ingress.inject(frame()).await;
        yield_to_forwarder().await;

        assert_eq!(egress.sent()[0].payload_bytes(), b"streamed");
    }

    #[tokio::test]
    async fn routes_zero_copy_to_zero_copy() {
        let ingress = Arc::new(MemoryZeroCopyTransport::default());
        let egress = Arc::new(MemoryZeroCopyTransport::default());
        let mut streamer = NativeFrameStreamer::new();
        streamer
            .route(
                Arc::new(ZeroCopyEndpoint::new(ingress.clone())),
                Arc::new(ZeroCopyEndpoint::new(egress.clone())),
                &topic(),
                None,
            )
            .await
            .expect("route failed");

        ingress.inject(frame()).await;
        yield_to_forwarder().await;

        assert_eq!(egress.sent()[0].payload_bytes(), b"streamed");
    }

    #[tokio::test]
    async fn zero_copy_endpoint_can_send_with_transport_extension() {
        let transport = MemoryZeroCopyTransport::default();
        let payload = b"extension path".as_slice();
        transport
            .send_serialized_zero_copy::<RawBytes, _>(UFrameHeader::publish(topic()), &payload)
            .await
            .expect("send failed");

        assert_eq!(transport.sent()[0].payload_bytes(), b"extension path");
    }
}
