// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Prepared SDK conformance core with explicit failure/recording controls.
//! This allocating observer is not native or zero-allocation acceptance evidence.
#![allow(dead_code)]

use std::{
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use up_rust::{
    FrameSink, InitializedTx, LoanError, OpaqueTemplate, PoolCore, PoolCoreConfig,
    PreparedTransportCore, PreparedTxStorage, RouteLimits, SendError, SetupError, TxFrameLayout,
    UFrameFieldsMetadataCodec, UOwnedFrame, UUri, UWire, UWireMetadataCodec, UWireMetadataContext,
};

pub struct Core {
    pool: PoolCore,
    context: UWireMetadataContext,
    pub sent: Mutex<Vec<UOwnedFrame>>,
    pub attach_calls: AtomicUsize,
    pub detach_calls: AtomicUsize,
    pub fail_attach: AtomicUsize,
    pub fail_detach: AtomicBool,
    pub fail_publish: AtomicBool,
}
impl Default for Core {
    fn default() -> Self {
        Self::for_wire::<up_rust::ProtobufWire>()
    }
}
impl Core {
    pub fn for_wire<W: UWire>() -> Self {
        Self {
            pool: PoolCore::new(PoolCoreConfig {
                slots: 32,
                slot_bytes: 8192,
                alignment: 8,
                max_routes: 64,
                max_registrations: 32,
            })
            .unwrap(),
            context: UWireMetadataContext::from_wire::<W, UFrameFieldsMetadataCodec>(),
            sent: Mutex::new(Vec::new()),
            attach_calls: AtomicUsize::new(0),
            detach_calls: AtomicUsize::new(0),
            fail_attach: AtomicUsize::new(0),
            fail_detach: AtomicBool::new(false),
            fail_publish: AtomicBool::new(false),
        }
    }
    pub fn sent_frames(&self) -> Vec<UOwnedFrame> {
        self.sent.lock().unwrap().clone()
    }
    pub fn inject<W: UWire>(self: &Arc<Self>, frame: UOwnedFrame) {
        OpaqueTemplate::for_wire::<W>()
            .unwrap()
            .route(
                Arc::clone(self),
                frame.metadata().source().clone(),
                frame.metadata().sink().cloned(),
                limits(),
                up_rust::RouteConfig {
                    max_token_len: 2048,
                    allow_traceparent: true,
                },
            )
            .unwrap()
            .send_bytes(
                up_rust::TxHeaderParts::from(frame.metadata()),
                frame.payload().map(|bytes| bytes.as_ref()),
            )
            .unwrap();
    }
}
pub fn limits() -> RouteLimits {
    RouteLimits {
        max_metadata_len: 3072,
        max_payload_len: 4096,
        payload_alignment: 8,
    }
}
pub fn reservation(source: UUri, sink: Option<UUri>) -> up_streamer::PreparedRouteReservation {
    up_streamer::PreparedRouteReservation {
        source,
        sink,
        limits: limits(),
        config: up_rust::RouteConfig {
            max_token_len: 2048,
            allow_traceparent: true,
        },
    }
}
impl PreparedTransportCore for Core {
    type Route = <PoolCore as PreparedTransportCore>::Route;
    type Tx<'a> = <PoolCore as PreparedTransportCore>::Tx<'a>;
    type Rx = <PoolCore as PreparedTransportCore>::Rx;
    type Subscription = <PoolCore as PreparedTransportCore>::Subscription;
    fn prepare_route(
        &self,
        source: &UUri,
        sink: Option<&UUri>,
        limits: RouteLimits,
    ) -> Result<Self::Route, SetupError> {
        self.pool.prepare_route(source, sink, limits)
    }
    fn loan<'a>(
        &'a self,
        route: &'a Self::Route,
        layout: TxFrameLayout,
    ) -> Result<Self::Tx<'a>, LoanError> {
        self.pool.loan(route, layout)
    }
    fn publish<'a>(
        &'a self,
        route: &'a Self::Route,
        frame: InitializedTx<Self::Tx<'a>>,
    ) -> Result<(), SendError> {
        if self.fail_publish.load(Ordering::Acquire) {
            return Err(SendError::Delivery);
        }
        let mut storage = frame.into_storage();
        let bytes = storage.metadata_uninit_mut();
        // SAFETY: the consumed SDK initialization proof covers every metadata byte;
        // storage remains alive and this borrow ends before the payload accessor.
        let bytes = unsafe { std::slice::from_raw_parts(bytes.as_ptr().cast::<u8>(), bytes.len()) };
        let metadata = UFrameFieldsMetadataCodec
            .decode_frame_metadata(self.context, bytes)
            .map_err(|_| SendError::PayloadStorage)?;
        let payload = storage.payload_uninit_mut().map(|bytes| {
            // SAFETY: InitializedTx also proves this entire payload extent valid.
            let bytes =
                unsafe { std::slice::from_raw_parts(bytes.as_ptr().cast::<u8>(), bytes.len()) };
            bytes::Bytes::copy_from_slice(bytes)
        });
        self.sent
            .lock()
            .unwrap()
            .push(UOwnedFrame::new(metadata, payload).unwrap());
        // SAFETY: only immutable observation/copy occurred; storage, bytes,
        // header record and initializer witness are unchanged and still owned.
        self.pool
            .publish(route, unsafe { InitializedTx::from_initialized(storage) })
    }
    async fn attach(
        &self,
        source: &UUri,
        sink: Option<&UUri>,
        inbox: Arc<dyn FrameSink<Self::Rx>>,
    ) -> Result<Self::Subscription, SetupError> {
        let call = self.attach_calls.fetch_add(1, Ordering::AcqRel) + 1;
        if self.fail_attach.load(Ordering::Acquire) == call {
            return Err(SetupError::Worker);
        }
        self.pool.attach(source, sink, inbox).await
    }
    fn detach(&self, subscription: &mut Self::Subscription) -> Result<(), SetupError> {
        self.detach_calls.fetch_add(1, Ordering::AcqRel);
        if self.fail_detach.swap(false, Ordering::AcqRel) {
            return Err(SetupError::Worker);
        }
        self.pool.detach(subscription)
    }
}
pub async fn wait_for(predicate: impl Fn() -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !predicate() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("forwarding did not complete");
}
