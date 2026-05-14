/********************************************************************************
 * Copyright (c) 2024 Contributors to the Eclipse Foundation
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
use tokio::sync::mpsc;
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
pub(crate) trait NativeFrameEndpoint: Send + Sync {
    fn mode(&self) -> TransportMode;
    async fn send_frame(&self, frame: UOwnedFrame) -> Result<(), UStatus>;
    async fn register_ingress(
        &self,
        source_filter: &UUri,
        sink_filter: Option<&UUri>,
        tx: mpsc::Sender<UOwnedFrame>,
    ) -> Result<Arc<dyn NativeIngressRegistration>, UStatus>;
}

#[async_trait]
pub(crate) trait NativeIngressRegistration: Send + Sync {
    async fn unregister(&self) -> Result<(), UStatus>;
}

#[derive(Clone)]
pub struct Endpoint {
    pub(crate) name: String,
    pub(crate) authority: String,
    pub(crate) transport: Arc<dyn NativeFrameEndpoint>,
}

impl Endpoint {
    pub fn new(name: &str, authority: &str, transport: Arc<dyn UOwnedTransport>) -> Self {
        Self::from_owned(name, authority, transport)
    }

    pub fn from_owned(name: &str, authority: &str, transport: Arc<dyn UOwnedTransport>) -> Self {
        Self {
            name: name.to_string(),
            authority: authority.to_string(),
            transport: Arc::new(OwnedEndpoint { transport }),
        }
    }

    pub fn from_zero_copy<T>(name: &str, authority: &str, transport: Arc<T>) -> Self
    where
        T: UZeroCopyTransport + Send + Sync + 'static,
    {
        Self {
            name: name.to_string(),
            authority: authority.to_string(),
            transport: Arc::new(ZeroCopyEndpoint { transport }),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn authority(&self) -> &str {
        &self.authority
    }

    pub fn mode(&self) -> TransportMode {
        self.transport.mode()
    }
}

struct OwnedEndpoint {
    transport: Arc<dyn UOwnedTransport>,
}

#[async_trait]
impl NativeFrameEndpoint for OwnedEndpoint {
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
        tx: mpsc::Sender<UOwnedFrame>,
    ) -> Result<Arc<dyn NativeIngressRegistration>, UStatus> {
        let listener: Arc<dyn UOwnedListener> = Arc::new(OwnedIngress { tx });
        self.transport
            .register_owned_listener(source_filter, sink_filter, listener.clone())
            .await?;
        Ok(Arc::new(OwnedIngressRegistration {
            transport: self.transport.clone(),
            source_filter: source_filter.clone(),
            sink_filter: sink_filter.cloned(),
            listener,
        }))
    }
}

struct OwnedIngress {
    tx: mpsc::Sender<UOwnedFrame>,
}

#[async_trait]
impl UOwnedListener for OwnedIngress {
    async fn on_receive_owned(&self, frame: UOwnedFrame) {
        let _ = self.tx.send(frame).await;
    }
}

struct OwnedIngressRegistration {
    transport: Arc<dyn UOwnedTransport>,
    source_filter: UUri,
    sink_filter: Option<UUri>,
    listener: Arc<dyn UOwnedListener>,
}

#[async_trait]
impl NativeIngressRegistration for OwnedIngressRegistration {
    async fn unregister(&self) -> Result<(), UStatus> {
        self.transport
            .unregister_owned_listener(
                &self.source_filter,
                self.sink_filter.as_ref(),
                self.listener.clone(),
            )
            .await
    }
}

struct ZeroCopyEndpoint<T> {
    transport: Arc<T>,
}

#[async_trait]
impl<T> NativeFrameEndpoint for ZeroCopyEndpoint<T>
where
    T: UZeroCopyTransport + Send + Sync + 'static,
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
        tx: mpsc::Sender<UOwnedFrame>,
    ) -> Result<Arc<dyn NativeIngressRegistration>, UStatus> {
        let listener: Arc<dyn UZeroCopyListener<T::Rx>> = Arc::new(ZeroCopyIngress::<T::Rx> {
            tx,
            _rx: PhantomData,
        });
        self.transport
            .register_zero_copy_listener(source_filter, sink_filter, listener.clone())
            .await?;
        Ok(Arc::new(ZeroCopyIngressRegistration::<T> {
            transport: self.transport.clone(),
            source_filter: source_filter.clone(),
            sink_filter: sink_filter.cloned(),
            listener,
        }))
    }
}

struct ZeroCopyIngress<Rx> {
    tx: mpsc::Sender<UOwnedFrame>,
    _rx: PhantomData<fn() -> Rx>,
}

#[async_trait]
impl<Rx> UZeroCopyListener<Rx> for ZeroCopyIngress<Rx>
where
    Rx: UZeroCopyRxFrame + Send + 'static,
{
    async fn on_receive_zero_copy(&self, frame: Rx) {
        let owned = UOwnedFrame::new(frame.header().clone(), frame.payload().to_vec());
        let _ = self.tx.send(owned).await;
    }
}

struct ZeroCopyIngressRegistration<T>
where
    T: UZeroCopyTransport + Send + Sync + 'static,
{
    transport: Arc<T>,
    source_filter: UUri,
    sink_filter: Option<UUri>,
    listener: Arc<dyn UZeroCopyListener<T::Rx>>,
}

#[async_trait]
impl<T> NativeIngressRegistration for ZeroCopyIngressRegistration<T>
where
    T: UZeroCopyTransport + Send + Sync + 'static,
{
    async fn unregister(&self) -> Result<(), UStatus> {
        self.transport
            .unregister_zero_copy_listener(
                &self.source_filter,
                self.sink_filter.as_ref(),
                self.listener.clone(),
            )
            .await
    }
}
