// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Setup-only erased route ownership; per-frame forwarding remains generic.

use crate::RouteDiagnostic;
use std::sync::Arc;
use up_rust::{UCode, UStatus, UUri};

pub(crate) fn status(error: impl std::fmt::Display) -> UStatus {
    UStatus::fail_with_code(UCode::FailedPrecondition, error.to_string())
}

#[derive(Clone, Debug, Hash, Eq, PartialEq)]
pub(crate) struct FrameRouteKey {
    ingress: String,
    ingress_authority: String,
    egress: String,
    egress_authority: String,
    kind: &'static str,
}
impl FrameRouteKey {
    pub(crate) fn new(
        ingress: &str,
        ingress_authority: &str,
        egress: &str,
        egress_authority: &str,
        kind: &'static str,
    ) -> Result<Self, UStatus> {
        if ingress_authority == egress_authority {
            return Err(UStatus::fail_with_code(
                UCode::InvalidArgument,
                "ingress and egress authorities must differ",
            ));
        }
        Ok(Self {
            ingress: ingress.into(),
            ingress_authority: ingress_authority.into(),
            egress: egress.into(),
            egress_authority: egress_authority.into(),
            kind,
        })
    }
}

#[async_trait::async_trait]
pub(crate) trait FrameRouteControl: Send {
    fn diagnostic(&self) -> &RouteDiagnostic;
    async fn stop(&mut self) -> Result<(), UStatus>;
}

#[cfg(feature = "experimental-copy-minimized-routing")]
pub(crate) mod native {
    use super::*;
    use crate::{prepared_endpoint::EgressCatalog, PreparedEndpoint, ZeroCopyFrameEndpoint};
    use up_rust::{
        Frame, HeaderAdmission, HeaderHandler, HeaderLease, HeaderLimits, PreparedTransportCore,
        RegistrationConfig, WireRegistration,
    };

    pub(crate) struct NativeBinding<C: PreparedTransportCore> {
        registrations: Vec<WireRegistration<C>>,
        diagnostic: RouteDiagnostic,
    }
    #[async_trait::async_trait]
    impl<C: PreparedTransportCore> FrameRouteControl for NativeBinding<C> {
        fn diagnostic(&self) -> &RouteDiagnostic {
            &self.diagnostic
        }
        async fn stop(&mut self) -> Result<(), UStatus> {
            // A failed physical detach retains its exact handle and the whole
            // route stays in the map for retry. All admission closes first.
            let mut failed = None;
            for registration in &mut self.registrations {
                if let Err(error) = registration.detach() {
                    failed.get_or_insert(error);
                }
            }
            if let Some(error) = failed {
                return Err(status(error));
            }
            while let Some(registration) = self.registrations.pop() {
                registration.stop().await.map_err(status)?;
            }
            Ok(())
        }
    }
    pub(crate) struct NativeForward<C: PreparedTransportCore> {
        pub(crate) egress: Arc<EgressCatalog<C>>,
        #[cfg(feature = "flow-evidence")]
        pub(crate) label: Option<String>,
    }
    impl<C: PreparedTransportCore, F: Frame> HeaderHandler<F> for NativeForward<C> {
        async fn on_receive(&mut self, frame: HeaderLease<F>) {
            // EgressCatalog records both successful and failed copies. Unknown
            // payload identity stays opaque; no decoder or native borrow is formed.
            #[cfg(feature = "flow-evidence")]
            let observation = self.label.as_deref().and_then(|label| {
                crate::flow_observation::emit(crate::flow_observation::prepared(
                    "ingress", label, &frame,
                ));
                crate::flow_observation::prepared("egress", label, &frame)
            });
            let result = self.egress.forward(&frame);
            #[cfg(feature = "flow-evidence")]
            if result.is_ok() {
                crate::flow_observation::emit(observation);
            }
            #[cfg(not(feature = "flow-evidence"))]
            let _ = result;
        }
    }
    pub(crate) async fn attach<T, H, M>(
        ingress: &ZeroCopyFrameEndpoint<T>,
        filters: Vec<(UUri, Option<UUri>)>,
        config: RegistrationConfig,
        diagnostic: RouteDiagnostic,
        mut handler: M,
    ) -> Result<Box<dyn FrameRouteControl>, UStatus>
    where
        T: PreparedEndpoint,
        H: HeaderHandler<<T::Core as PreparedTransportCore>::Rx>,
        M: FnMut() -> H,
    {
        let admission =
            HeaderAdmission::for_wire::<T::Wire>(HeaderLimits::default()).map_err(status)?;
        let mut binding = NativeBinding {
            registrations: Vec::with_capacity(filters.len()),
            diagnostic,
        };
        for (source, sink) in filters {
            if ingress.receive_catalog.as_ref().is_some_and(|catalog| {
                !catalog.iter().any(|(actual_source, actual_sink)| {
                    source.matches(actual_source)
                        && match (sink.as_ref(), actual_sink.as_ref()) {
                            (None, None) => true,
                            (Some(filter), Some(actual)) => filter.matches(actual),
                            _ => false,
                        }
                })
            }) {
                continue;
            }
            let worker = admission
                .register_filtered(source.clone(), sink.clone(), config, handler(), || {})
                .map_err(status)?;
            match WireRegistration::attach(
                Arc::clone(ingress.transport.prepared_core()),
                &source,
                sink.as_ref(),
                worker,
            )
            .await
            {
                Ok(registration) => binding.registrations.push(registration),
                Err(error) => {
                    let _ = binding.stop().await;
                    return Err(status(error));
                }
            }
        }
        Ok(Box::new(binding))
    }
}

#[cfg(feature = "owned-frame-transport")]
pub(crate) mod owned {
    use super::*;
    use crate::{Endpoint, OwnedFrameEndpoint};
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::sync::{mpsc, Notify};
    #[cfg(feature = "experimental-copy-minimized-routing")]
    use up_rust::UFrameMetadata;
    use up_rust::{
        NativeProfileAgreement, RegistrationConfig, UListener, UMessage, UOwnedFrame,
        UOwnedListener,
    };

    // This observation follows checked SDK projection. It records the actual
    // token transition, never infers that opaque bytes are a typed value.
    fn trace_native_projection(
        route: &str,
        direction: &str,
        agreement: Option<&NativeProfileAgreement>,
        metadata: &up_rust::UFrameMetadata,
    ) {
        #[cfg(feature = "flow-evidence")]
        let dedicated = streamer_flow_evidence::dedicated_output_is_prepared();
        #[cfg(not(feature = "flow-evidence"))]
        let dedicated = false;
        if !dedicated && !tracing::enabled!(tracing::Level::INFO) {
            return;
        }
        let (Some(agreement), Some(encoding), Some(token)) = (
            agreement,
            metadata.payload_encoding(),
            metadata.native_type_token(),
        ) else {
            return;
        };
        let profile = agreement.profile();
        let evidence = serde_json::json!({
            "route": route,
            "direction": direction,
            "domain": profile.domain(),
            "version": profile.version(),
            "profile_digest": profile.content_digest(),
            "encoding_id": encoding.id(),
            "native_type_token": token.as_u32(),
            "token_before": (direction == "frame_to_classic").then_some(token.as_u32()),
            "token_after": (direction == "classic_to_frame").then_some(token.as_u32()),
        });
        #[cfg(feature = "flow-evidence")]
        if dedicated {
            streamer_flow_evidence::emit_json_or_fail("NATIVE_PROJECTION_VERIFIED ", &evidence);
            return;
        }
        // Explicit standalone/default tracing remains available without a
        // declared structured capture; current acceptance uses the branch above.
        tracing::info!("NATIVE_PROJECTION_VERIFIED {evidence}");
    }

    pub(crate) trait Destination: Send + 'static {
        fn send(
            &mut self,
            frame: UOwnedFrame,
        ) -> impl std::future::Future<Output = Result<(), UStatus>> + Send;
    }
    #[derive(Clone)]
    pub(crate) struct OwnedDestination {
        pub(crate) endpoint: OwnedFrameEndpoint,
        pub(crate) label: String,
    }
    impl Destination for OwnedDestination {
        async fn send(&mut self, frame: UOwnedFrame) -> Result<(), UStatus> {
            crate::flow_observation::emit(crate::flow_observation::frame(
                "ingress",
                &self.label,
                &frame,
            ));
            let observation = crate::flow_observation::frame("egress", &self.label, &frame);
            self.endpoint.transport.send_owned(frame).await?;
            crate::flow_observation::emit(observation);
            Ok(())
        }
    }
    #[derive(Clone)]
    pub(crate) struct ClassicDestination {
        pub(crate) endpoint: Endpoint,
        pub(crate) agreement: Option<NativeProfileAgreement>,
        pub(crate) label: String,
    }
    impl Destination for ClassicDestination {
        async fn send(&mut self, frame: UOwnedFrame) -> Result<(), UStatus> {
            crate::flow_observation::emit(crate::flow_observation::frame(
                "ingress",
                &self.label,
                &frame,
            ));
            let message = if let Some(agreement) = &self.agreement {
                up_rust::frame::metadata::try_project_frame_to_umessage_with_native_agreement(
                    frame.metadata().clone(),
                    frame.payload().cloned(),
                    agreement,
                )
            } else {
                up_rust::frame::metadata::try_project_frame_to_umessage(
                    frame.metadata().clone(),
                    frame.payload().cloned(),
                )
            }
            .map_err(status)?;
            trace_native_projection(
                &self.label,
                "frame_to_classic",
                self.agreement.as_ref(),
                frame.metadata(),
            );
            let observation = crate::flow_observation::message("egress", &self.label, &message);
            self.endpoint.transport.send(message).await?;
            crate::flow_observation::emit(observation);
            Ok(())
        }
    }
    struct OwnedInbox {
        sender: mpsc::Sender<UOwnedFrame>,
        closed: AtomicBool,
        stop: Notify,
    }
    impl OwnedInbox {
        fn close(&self) {
            self.closed.store(true, Ordering::Release);
            self.stop.notify_waiters();
        }
        async fn push(&self, frame: UOwnedFrame) {
            let stop = self.stop.notified();
            tokio::pin!(stop);
            stop.as_mut().enable();
            if self.closed.load(Ordering::Acquire) {
                return;
            }
            // The compatibility callback is already asynchronous. Backpressure
            // yields to its one generic worker instead of blocking its executor.
            tokio::select! {
                biased;
                _ = &mut stop => {},
                result = self.sender.send(frame) => {
                    if result.is_err() { tracing::debug!("owned route inbox closed"); }
                }
            }
        }
    }
    struct OwnedWorker {
        inbox: Arc<OwnedInbox>,
        task: Option<tokio::task::JoinHandle<()>>,
    }
    impl OwnedWorker {
        fn new<D: Destination>(
            config: RegistrationConfig,
            mut destination: D,
        ) -> Result<Self, UStatus> {
            if config.capacity == 0 {
                return Err(status("owned route inbox capacity must be positive"));
            }
            let (sender, mut receiver) = mpsc::channel(config.capacity);
            let inbox = Arc::new(OwnedInbox {
                sender,
                closed: AtomicBool::new(false),
                stop: Notify::new(),
            });
            let state = Arc::clone(&inbox);
            let task = tokio::runtime::Handle::try_current()
                .map_err(status)?
                .spawn(async move {
                    loop {
                        let stop = state.stop.notified();
                        tokio::pin!(stop);
                        stop.as_mut().enable();
                        if state.closed.load(Ordering::Acquire) {
                            receiver.close();
                        }
                        let frame = if state.closed.load(Ordering::Acquire) {
                            receiver.recv().await
                        } else {
                            tokio::select! {
                                biased;
                                _ = &mut stop => { receiver.close(); receiver.recv().await },
                                frame = receiver.recv() => frame,
                            }
                        };
                        let Some(frame) = frame else {
                            break;
                        };
                        if let Err(error) = destination.send(frame).await {
                            tracing::warn!(%error, "owned route forwarding failed");
                        }
                    }
                });
            Ok(Self {
                inbox,
                task: Some(task),
            })
        }
        async fn stop(mut self) -> Result<(), UStatus> {
            self.inbox.close();
            if let Some(task) = self.task.take() {
                if tokio::task::try_id() != Some(task.id()) {
                    task.await.map_err(status)?;
                }
            }
            Ok(())
        }
    }
    impl Drop for OwnedWorker {
        fn drop(&mut self) {
            self.inbox.close();
            if let Some(task) = &self.task {
                if tokio::task::try_id() != Some(task.id()) {
                    task.abort();
                }
            }
        }
    }
    struct OwnedIngress {
        inbox: Arc<OwnedInbox>,
    }
    #[async_trait::async_trait]
    impl UOwnedListener for OwnedIngress {
        async fn on_receive_owned(&self, frame: UOwnedFrame) {
            self.inbox.push(frame).await;
        }
    }
    struct ClassicIngress {
        label: String,
        inbox: Arc<OwnedInbox>,
        agreement: Option<NativeProfileAgreement>,
    }
    #[async_trait::async_trait]
    impl UListener for ClassicIngress {
        async fn on_receive(&self, message: UMessage) {
            let metadata = match &self.agreement {
                Some(agreement) => up_rust::frame::metadata::try_project_umessage_to_frame_metadata_with_native_agreement(&message, agreement),
                None => up_rust::frame::metadata::try_project_umessage_to_frame_metadata(&message),
            };
            match metadata.and_then(|metadata| {
                UOwnedFrame::new(
                    metadata,
                    message
                        .payload()
                        .map(|payload| bytes::Bytes::copy_from_slice(&payload)),
                )
            }) {
                Ok(frame) => {
                    trace_native_projection(
                        &self.label,
                        "classic_to_frame",
                        self.agreement.as_ref(),
                        frame.metadata(),
                    );
                    self.inbox.push(frame).await;
                }
                Err(error) => tracing::warn!(%error, "classic route projection rejected"),
            }
        }
    }
    enum Ingress {
        Owned {
            endpoint: OwnedFrameEndpoint,
            listener: Arc<dyn UOwnedListener>,
        },
        Classic {
            endpoint: Endpoint,
            listener: Arc<dyn UListener>,
        },
    }
    struct Binding {
        ingress: Ingress,
        filters: Vec<(UUri, Option<UUri>)>,
        worker: Option<OwnedWorker>,
        diagnostic: RouteDiagnostic,
    }
    impl Binding {
        async fn register(&self, source: &UUri, sink: Option<&UUri>) -> Result<(), UStatus> {
            match &self.ingress {
                Ingress::Owned { endpoint, listener } => {
                    endpoint
                        .transport
                        .register_owned_listener(source, sink, Arc::clone(listener))
                        .await
                }
                Ingress::Classic { endpoint, listener } => {
                    endpoint
                        .transport
                        .register_listener(source, sink, Arc::clone(listener))
                        .await
                }
            }
        }
        async fn attach(
            mut self,
            filters: Vec<(UUri, Option<UUri>)>,
        ) -> Result<Box<dyn FrameRouteControl>, UStatus> {
            for (source, sink) in filters {
                if let Err(error) = self.register(&source, sink.as_ref()).await {
                    let _ = self.stop().await;
                    return Err(error);
                }
                self.filters.push((source, sink));
            }
            Ok(Box::new(self))
        }
    }
    #[async_trait::async_trait]
    impl FrameRouteControl for Binding {
        fn diagnostic(&self) -> &RouteDiagnostic {
            &self.diagnostic
        }
        async fn stop(&mut self) -> Result<(), UStatus> {
            if let Some(worker) = &self.worker {
                worker.inbox.close();
            }
            let mut remaining = Vec::with_capacity(self.filters.len());
            let mut first_error = None;
            for (source, sink) in self.filters.drain(..) {
                let result = match &self.ingress {
                    Ingress::Owned { endpoint, listener } => {
                        endpoint
                            .transport
                            .unregister_owned_listener(&source, sink.as_ref(), Arc::clone(listener))
                            .await
                    }
                    Ingress::Classic { endpoint, listener } => {
                        endpoint
                            .transport
                            .unregister_listener(&source, sink.as_ref(), Arc::clone(listener))
                            .await
                    }
                };
                if let Err(error) = result {
                    remaining.push((source, sink));
                    first_error.get_or_insert(error);
                }
            }
            self.filters = remaining;
            if let Some(error) = first_error {
                return Err(error);
            }
            if let Some(worker) = self.worker.take() {
                worker.stop().await.map_err(status)?;
            }
            Ok(())
        }
    }
    pub(crate) async fn attach_owned<D: Destination>(
        ingress: &OwnedFrameEndpoint,
        filters: Vec<(UUri, Option<UUri>)>,
        config: RegistrationConfig,
        diagnostic: RouteDiagnostic,
        destination: D,
    ) -> Result<Box<dyn FrameRouteControl>, UStatus> {
        let worker = OwnedWorker::new(config, destination)?;
        let listener = Arc::new(OwnedIngress {
            inbox: Arc::clone(&worker.inbox),
        });
        Binding {
            ingress: Ingress::Owned {
                endpoint: ingress.clone(),
                listener,
            },
            filters: Vec::new(),
            worker: Some(worker),
            diagnostic,
        }
        .attach(filters)
        .await
    }
    pub(crate) async fn attach_classic<D: Destination>(
        ingress: &Endpoint,
        filters: Vec<(UUri, Option<UUri>)>,
        config: RegistrationConfig,
        diagnostic: RouteDiagnostic,
        agreement: Option<NativeProfileAgreement>,
        destination: D,
    ) -> Result<Box<dyn FrameRouteControl>, UStatus> {
        let worker = OwnedWorker::new(config, destination)?;
        let listener = Arc::new(ClassicIngress {
            label: format!(
                "classic:{}@{} -> frame:{}@{}",
                diagnostic.route.ingress_name,
                diagnostic.route.ingress_authority,
                diagnostic.route.egress_name,
                diagnostic.route.egress_authority,
            ),
            inbox: Arc::clone(&worker.inbox),
            agreement,
        });
        Binding {
            ingress: Ingress::Classic {
                endpoint: ingress.clone(),
                listener,
            },
            filters: Vec::new(),
            worker: Some(worker),
            diagnostic,
        }
        .attach(filters)
        .await
    }

    #[cfg(feature = "experimental-copy-minimized-routing")]
    pub(crate) struct NativeDestination<C: up_rust::PreparedTransportCore> {
        pub(crate) egress: Arc<crate::prepared_endpoint::EgressCatalog<C>>,
        #[cfg(feature = "flow-evidence")]
        pub(crate) label: Option<String>,
    }
    #[cfg(feature = "experimental-copy-minimized-routing")]
    impl<C: up_rust::PreparedTransportCore> Destination for NativeDestination<C> {
        async fn send(&mut self, frame: UOwnedFrame) -> Result<(), UStatus> {
            #[cfg(feature = "flow-evidence")]
            let observation = self.label.as_deref().and_then(|label| {
                crate::flow_observation::emit(crate::flow_observation::frame(
                    "ingress", label, &frame,
                ));
                crate::flow_observation::frame("egress", label, &frame)
            });
            self.egress.forward_owned(&frame).map_err(status)?;
            #[cfg(feature = "flow-evidence")]
            crate::flow_observation::emit(observation);
            Ok(())
        }
    }

    #[cfg(feature = "experimental-copy-minimized-routing")]
    pub(crate) struct HeaderProjection<W: up_rust::UWire, D: Destination> {
        pub(crate) destination: D,
        pub(crate) wire: std::marker::PhantomData<W>,
    }
    #[cfg(feature = "experimental-copy-minimized-routing")]
    impl<W: up_rust::UWire + Send + 'static, D: Destination, F: up_rust::Frame>
        up_rust::HeaderHandler<F> for HeaderProjection<W, D>
    {
        async fn on_receive(&mut self, frame: up_rust::HeaderLease<F>) {
            use up_rust::UWireMetadataCodec;
            let frame = frame.into_frame();
            let metadata: Result<UFrameMetadata, _> = up_rust::UFrameFieldsMetadataCodec.decode_frame_metadata(
                up_rust::UWireMetadataContext::from_wire::<W, up_rust::UFrameFieldsMetadataCodec>(), frame.encoded_metadata());
            let result = metadata.map_err(status).and_then(|metadata| {
                UOwnedFrame::new(
                    metadata,
                    frame
                        .payload()
                        .map(|payload| bytes::Bytes::copy_from_slice(payload.bytes())),
                )
                .map_err(status)
            });
            match result {
                Ok(frame) => {
                    if let Err(error) = self.destination.send(frame).await {
                        tracing::warn!(%error, "native route projection failed");
                    }
                }
                Err(error) => tracing::warn!(%error, "native route metadata rejected"),
            }
        }
    }
    #[cfg(test)]
    mod projection_tests {
        use super::*;
        use std::sync::Mutex;
        use tracing::{span, Event, Metadata, Subscriber};
        use up_rust::{
            NativeProfile, NativeProfileMode, NativeProfileTable, PayloadEncoding, StablePayload,
        };

        #[derive(Clone, Default)]
        struct Capture(Arc<Mutex<Vec<String>>>);
        impl Subscriber for Capture {
            fn enabled(&self, _: &Metadata<'_>) -> bool {
                true
            }
            fn new_span(&self, _: &span::Attributes<'_>) -> span::Id {
                span::Id::from_u64(1)
            }
            fn record(&self, _: &span::Id, _: &span::Record<'_>) {}
            fn record_follows_from(&self, _: &span::Id, _: &span::Id) {}
            fn event(&self, event: &Event<'_>) {
                struct Visitor<'a>(&'a mut Vec<String>);
                impl tracing::field::Visit for Visitor<'_> {
                    fn record_debug(
                        &mut self,
                        field: &tracing::field::Field,
                        value: &dyn std::fmt::Debug,
                    ) {
                        if field.name() == "message" {
                            self.0.push(format!("{value:?}"));
                        }
                    }
                }
                event.record(&mut Visitor(&mut self.0.lock().unwrap()));
            }
            fn enter(&self, _: &span::Id) {}
            fn exit(&self, _: &span::Id) {}
        }

        #[derive(up_rust::StablePayload)]
        #[repr(C)]
        #[stable_payload(type_name = "uprotocol.test.ProjectionObservation")]
        struct Payload {
            value: u64,
        }

        fn agreement() -> NativeProfileAgreement {
            let profile = Arc::new(
                NativeProfile::new(
                    "projection-regression",
                    1,
                    NativeProfileMode::Table(
                        NativeProfileTable::new([(
                            PayloadEncoding::private_use(0xF101),
                            Payload::native_representation(),
                        )])
                        .unwrap(),
                    ),
                )
                .unwrap(),
            );
            NativeProfileAgreement::new(profile.clone(), &profile).unwrap()
        }

        #[derive(Default)]
        struct ClassicSink(Mutex<Vec<UMessage>>);
        #[async_trait::async_trait]
        impl up_rust::UTransport for ClassicSink {
            async fn send(&self, message: UMessage) -> Result<(), UStatus> {
                self.0.lock().unwrap().push(message);
                Ok(())
            }
            async fn register_listener(
                &self,
                _: &UUri,
                _: Option<&UUri>,
                _: Arc<dyn UListener>,
            ) -> Result<(), UStatus> {
                unreachable!()
            }
            async fn unregister_listener(
                &self,
                _: &UUri,
                _: Option<&UUri>,
                _: Arc<dyn UListener>,
            ) -> Result<(), UStatus> {
                unreachable!()
            }
        }

        #[tokio::test(flavor = "current_thread")]
        async fn actual_classic_adapters_emit_checked_token_transitions() {
            let capture = Capture::default();
            let _subscriber = tracing::subscriber::set_default(capture.clone());
            let agreement = agreement();
            let topic = UUri::try_from_parts("projection-source", 0x5BA0, 1, 0x8001).unwrap();
            let message = up_rust::UMessageBuilder::publish(topic.clone())
                .build_with_payload(
                    bytes::Bytes::from_static(&[0; 8]),
                    PayloadEncoding::private_use(0xF101),
                )
                .unwrap();
            let (sender, mut receiver) = mpsc::channel(1);
            let ingress = ClassicIngress {
                inbox: Arc::new(OwnedInbox {
                    sender,
                    closed: AtomicBool::new(false),
                    stop: Notify::new(),
                }),
                agreement: Some(agreement.clone()),
                label: "projection-ingress".into(),
            };
            ingress.on_receive(message).await;
            let frame = receiver.try_recv().unwrap();
            let token = frame.metadata().native_type_token().unwrap();
            let sink = Arc::new(ClassicSink::default());
            let mut destination = ClassicDestination {
                endpoint: Endpoint::new("classic-sink", "projection-sink", sink.clone()),
                agreement: Some(agreement),
                label: "projection-egress".into(),
            };
            destination.send(frame).await.unwrap();
            assert_eq!(sink.0.lock().unwrap().len(), 1);
            let opaque = up_rust::UMessageBuilder::publish(topic)
                .build_with_payload(
                    bytes::Bytes::from_static(b"opaque"),
                    PayloadEncoding::private_use(0xF102),
                )
                .unwrap();
            ingress.on_receive(opaque).await;
            let opaque = receiver.try_recv().unwrap();
            assert!(opaque.metadata().native_type_token().is_none());
            assert_eq!(opaque.payload_bytes(), b"opaque");
            let records: Vec<_> = capture
                .0
                .lock()
                .unwrap()
                .iter()
                .filter_map(|s| {
                    s.strip_prefix("NATIVE_PROJECTION_VERIFIED ")
                        .map(|json| serde_json::from_str::<serde_json::Value>(json).unwrap())
                })
                .collect();
            assert_eq!(records.len(), 2);
            assert_eq!(records[0]["direction"], "classic_to_frame");
            assert!(records[0]["token_before"].is_null());
            assert_eq!(records[0]["token_after"], token.as_u32());
            assert_eq!(records[1]["direction"], "frame_to_classic");
            assert_eq!(records[1]["token_before"], token.as_u32());
            assert!(records[1]["token_after"].is_null());
            assert!(records
                .iter()
                .all(|v| v["domain"] == "projection-regression" && v["encoding_id"] == 0xF101));
        }
    }
}
