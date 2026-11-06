// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Sustained application behavior over an already configured SDK endpoint.
//!
//! Endpoint construction, readiness and backend acknowledgements remain with the
//! existing role's binding setup. This module has no middleware configuration,
//! namespace management or OS-specific resource inspection.
use super::payloads::NativeContext;
use async_trait::async_trait;
use configurable_streamer_wire_support::prepared::{
    NativeSetupReady, PreparedEndpoint, PreparedSession,
};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use streamer_soak::control::{Phase, PhaseCommand, PhaseTracker};
use streamer_soak::{inbox, ReplayOptions, Role};
use up_rust::{PreparedTransportCore, UZeroCopyRxLease};
use up_rust::{
    ProtobufMappable, UCode, UFrameMetadata, UFrameView, UListener, UMessage, UMessageBuilder,
    UStatus, UTransport, UUri,
};

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerPlan {
    /// Finite correctness run; receivers remain admitted through controller drain.
    #[serde(default)]
    pub finite_messages: Option<u32>,
    /// Explicit sensor representation; available only in a large-fixture build.
    #[cfg(feature = "payload-contract-large-fixtures")]
    #[serde(default)]
    pub native_fixture: Option<configurable_streamer_wire_support::sensor::SensorFixture>,
    pub replay: ReplayOptions,
    pub control_file: PathBuf,
    pub offered_per_second: u32,
    pub consumer_delay_ms: u64,
    pub maximum_payload_bytes: usize,
    pub deadline_seconds: u64,
}

fn failure(error: impl std::fmt::Display) -> UStatus {
    UStatus::fail_with_code(UCode::Internal, error.to_string())
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, UStatus> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(failure)?
        .take(65_537)
        .read_to_end(&mut bytes)
        .map_err(failure)?;
    if bytes.len() > 65_536 {
        return Err(failure("workload input exceeds bound"));
    }
    serde_json::from_slice(&bytes).map_err(failure)
}

impl WorkerPlan {
    pub fn load(path: &Path) -> Result<Self, UStatus> {
        let mut plan: Self = read_json(path)?;
        streamer_soak::Reconciler::new(&plan.replay).map_err(failure)?;
        if plan.finite_messages.is_some()
            && (plan.finite_messages != Some(5)
                || plan.offered_per_second != 20
                || plan.consumer_delay_ms != 0
                || plan.replay.queue_capacity != 5)
        {
            return Err(failure(
                "finite conservation requires five messages at 50ms, no processing delay and inbox capacity five",
            ));
        }
        #[cfg(feature = "payload-contract-large-fixtures")]
        let maximum = if let Some(fixture) = plan.native_fixture {
            if plan.replay.role != Role::PublisherSubscriber
                || plan.maximum_payload_bytes != fixture.payload_len()
                || plan.offered_per_second != 2
                || plan.consumer_delay_ms != 1000
                || plan.replay.queue_capacity != 4
            {
                return Err(failure("sensor plan must select exact native pub/sub fixture and approved workload limits"));
            }
            fixture.payload_len()
        } else {
            65_536
        };
        #[cfg(not(feature = "payload-contract-large-fixtures"))]
        let maximum = 65_536;
        if !(1..=1000).contains(&plan.offered_per_second)
            || !(if plan.finite_messages.is_some() { 0 } else { 1 }..=1000)
                .contains(&plan.consumer_delay_ms)
            || !(1..=maximum).contains(&plan.maximum_payload_bytes)
            || !(1..=660).contains(&plan.deadline_seconds)
            || plan.replay.queue_capacity > 65_536
        {
            return Err(failure("workload limits outside bounded envelope"));
        }
        if plan.control_file.is_relative() {
            plan.control_file = path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join(&plan.control_file);
        }
        Ok(plan)
    }
}

struct Packet {
    metadata: UFrameMetadata,
    payload: Payload,
}

enum Payload {
    Owned(bytes::Bytes),
    Loan(Box<dyn RetainedPayload>),
}
impl Packet {
    fn bytes(&self) -> &[u8] {
        match &self.payload {
            Payload::Owned(bytes) => bytes,
            Payload::Loan(loan) => loan.bytes(),
        }
    }
}
trait RetainedPayload: Send {
    fn bytes(&self) -> &[u8];
    fn admit(&mut self);
}
struct Retained<R> {
    frame: R,
    _guard: LoanGuard,
}
impl<R: UZeroCopyRxLease + Send> RetainedPayload for Retained<R> {
    fn admit(&mut self) {
        self._guard.admit();
    }
    fn bytes(&self) -> &[u8] {
        self.frame
            .try_contiguous_payload()
            .expect("validated contiguous payload")
    }
}
#[derive(Default)]
struct Loans {
    rx: AtomicU64,
    tx: AtomicU64,
    peak_rx: AtomicU64,
    peak_tx: AtomicU64,
    transient_rx: AtomicU64,
    peak_transient_rx: AtomicU64,
}
struct LoanGuard {
    loans: Arc<Loans>,
    tx: bool,
    transient: bool,
}
impl LoanGuard {
    fn new(loans: Arc<Loans>, tx: bool) -> Self {
        let (live, peak) = if tx {
            (&loans.tx, &loans.peak_tx)
        } else {
            (&loans.transient_rx, &loans.peak_transient_rx)
        };
        peak.fetch_max(live.fetch_add(1, Ordering::SeqCst) + 1, Ordering::SeqCst);
        Self {
            loans,
            tx,
            transient: !tx,
        }
    }
    fn admit(&mut self) {
        if self.transient {
            self.loans.transient_rx.fetch_sub(1, Ordering::SeqCst);
            let live = self.loans.rx.fetch_add(1, Ordering::SeqCst) + 1;
            self.loans.peak_rx.fetch_max(live, Ordering::SeqCst);
            self.transient = false;
        }
    }
}
impl Drop for LoanGuard {
    fn drop(&mut self) {
        let live = if self.tx {
            &self.loans.tx
        } else if self.transient {
            &self.loans.transient_rx
        } else {
            &self.loans.rx
        };
        let previous = live.fetch_sub(1, Ordering::SeqCst);
        debug_assert!(previous > 0);
    }
}

struct Listener {
    admission: up_rust::ListenerAdmission,
    sender: inbox::Sender<Packet>,
    /// An RPC client completes its responses in this listener rather than queueing them.
    inline: bool,
    phase: Arc<Mutex<PhaseTracker>>,
    native: NativeContext,
    maximum: usize,
    errors: AtomicU64,
    loans: Arc<Loans>,
}

impl Listener {
    fn phase(&self) -> Result<Phase, UStatus> {
        Ok(self.phase.lock().map_err(failure)?.phase())
    }

    fn metadata(&self, message: &UMessage) -> Result<UFrameMetadata, UStatus> {
        if self.native.configured_agreement().is_some() {
            self.native.project_classic(message)
        } else {
            up_rust::frame::metadata::try_project_umessage_to_frame_metadata(message)
                .map_err(failure)
        }
    }

    fn emit(
        &self,
        event: &str,
        message: &UMessage,
        metadata: &UFrameMetadata,
        sequence: Option<u32>,
    ) -> Result<(), UStatus> {
        let bytes = message
            .payload()
            .ok_or_else(|| failure("workload payload missing"))?;
        self.emit_bytes(event, metadata, &bytes, sequence)
    }

    fn emit_bytes(
        &self,
        event: &str,
        metadata: &UFrameMetadata,
        bytes: &[u8],
        sequence: Option<u32>,
    ) -> Result<(), UStatus> {
        if bytes.len() > self.maximum {
            return Err(failure("workload payload exceeds declared bound"));
        }
        let attributes = if metadata.native_type_token().is_some() {
            metadata
                .try_project_to_attributes_with_native_agreement(self.native.agreement()?)
                .map_err(failure)?
        } else {
            metadata.try_project_to_attributes().map_err(failure)?
        };
        let attributes = attributes.write_to_protobuf_bytes().map_err(failure)?;
        let observation = streamer_flow_evidence::Observation::capture(
            event,
            None,
            metadata,
            bytes.len(),
            [bytes],
        )
        .map_err(failure)?
        .ok_or_else(|| failure("workload evidence is disabled"))?;
        streamer_flow_evidence::emit_json_or_fail(
            "OP_EVENT ",
            &json!({"event": event, "phase": self.phase()?, "sequence": sequence,
            "at_unix_ns": SystemTime::now().duration_since(UNIX_EPOCH).map_err(failure)?.as_nanos().to_string(),
            "attributes_sha256": format!("{:x}", Sha256::digest(attributes)), "observation": observation}),
        );
        Ok(())
    }

    fn metrics(&self, final_record: bool) -> Result<(), UStatus> {
        let snapshot = self.sender.snapshot();
        streamer_flow_evidence::emit_json_or_fail(
            "OP_METRICS ",
            &json!({"phase": self.phase()?, "final": final_record,
            "queue_occupancy": snapshot.queued, "queue_capacity": snapshot.capacity,
            "received": snapshot.received, "processed": snapshot.processed,
            "queue_drops": snapshot.overload_drops,
            "errors": self.errors.load(Ordering::SeqCst) + snapshot.closed_rejections + snapshot.abandoned,
            "live_application_rx_loans": self.loans.rx.load(Ordering::SeqCst), "max_application_rx_loans": self.loans.peak_rx.load(Ordering::SeqCst),
            "live_application_tx_loans": self.loans.tx.load(Ordering::SeqCst), "max_application_tx_loans": self.loans.peak_tx.load(Ordering::SeqCst),
            "live_transient_rx_loans": self.loans.transient_rx.load(Ordering::SeqCst), "max_transient_rx_loans": self.loans.peak_transient_rx.load(Ordering::SeqCst),
            "live_application_values": snapshot.live_held, "max_application_values": snapshot.peak_held}),
        );
        Ok(())
    }
}

#[async_trait]
impl UListener for Listener {
    async fn on_receive(&self, message: UMessage) {
        self.admission
            .dispatch(|| async { self.receive(message) })
            .await;
    }
}

impl Listener {
    fn receive(&self, message: UMessage) {
        let result = (|| {
            if self.native.configured_agreement().is_some() {
                self.native.verify_classic(&message)?;
            }
            let metadata = self.metadata(&message)?;
            self.enqueue(Packet {
                metadata,
                payload: Payload::Owned(
                    message
                        .payload()
                        .ok_or_else(|| failure("payload missing"))?,
                ),
            })
        })();
        self.report_error(result);
    }

    fn enqueue(&self, packet: Packet) -> Result<(), UStatus> {
        self.emit_bytes("receive", &packet.metadata, packet.bytes(), None)?;
        let admit = |packet: &mut Packet| {
            if let Payload::Loan(loan) = &mut packet.payload {
                loan.admit();
            }
        };
        // A binding may deliver a burst of responses in one pass while the drive task is not
        // scheduled, so an RPC client completes each one here instead of queueing it.
        if self.inline {
            return match self.sender.admit_inline(packet, admit) {
                Ok(delivery) => complete_response(self, delivery),
                Err(_) => Err(failure("workload receiver closed before listener quiesced")),
            };
        }
        match self.sender.try_send_with_admission(packet, admit) {
            Ok(()) => Ok(()),
            Err(tokio::sync::mpsc::error::TrySendError::Full(item)) => {
                let packet = item.value();
                self.emit_bytes("queue_drop", &packet.metadata, packet.bytes(), None)
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                Err(failure("workload receiver closed before listener quiesced"))
            }
        }
    }

    fn report_error(&self, result: Result<(), UStatus>) {
        if let Err(error) = result {
            self.errors.fetch_add(1, Ordering::SeqCst);
            streamer_flow_evidence::emit_json_or_fail(
                "OP_ERROR ",
                &json!({"error": error.to_string()}),
            );
        }
    }
}

// FrameSink::push is synchronous even when the middleware enters a Tokio
// runtime on its RX thread. This gate covers the complete bounded push, never
// crosses an await and drains before the application inbox is closed. This
// adapter does not expose a user callback that could unregister itself.
struct NativeReceiveGate(Mutex<bool>);
impl NativeReceiveGate {
    fn close(&self) {
        *self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = false;
    }
}

struct NativeListener<T: PreparedEndpoint> {
    gate: Arc<NativeReceiveGate>,
    listener: Arc<Listener>,
    session: Arc<PreparedSession<T>>,
    source: UUri,
    sink: Option<UUri>,
}
impl<T: PreparedEndpoint> up_rust::FrameSink<<T::Core as PreparedTransportCore>::Rx>
    for NativeListener<T>
{
    fn push(&self, raw: <T::Core as PreparedTransportCore>::Rx) {
        let open = self
            .gate
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !*open {
            return;
        }
        let guard = LoanGuard::new(self.listener.loans.clone(), false);
        let admitted = match self.session.admit(raw) {
            Ok(frame) => frame,
            Err(error) => {
                self.listener.report_error(Err(failure(format!(
                    "native workload admission rejected: {error}"
                ))));
                return;
            }
        };
        let result = (|| {
            let header = admitted.header().map_err(failure)?;
            if !self.source.matches(&header.source)
                || !match (self.sink.as_ref(), header.sink.as_ref()) {
                    (None, None) => true,
                    (Some(filter), Some(actual)) => filter.matches(actual),
                    _ => false,
                }
            {
                return Err(failure("native workload receive address differs"));
            }
            let frame = self.session.decode_compatibility_frame(admitted)?;
            if frame.try_contiguous_payload().is_none() {
                return Err(failure("noncontiguous native receive"));
            }
            if self.listener.native.configured_agreement().is_some() {
                self.listener.native.verify_view(&frame)?;
            } else if frame.metadata().native_type_token().is_some() {
                return Err(failure("unexpected native identity"));
            }
            let metadata = frame.metadata().clone();
            self.listener.enqueue(Packet {
                metadata,
                payload: Payload::Loan(Box::new(Retained {
                    frame,
                    _guard: guard,
                })),
            })
        })();
        self.listener.report_error(result);
    }
    fn close(&self) {
        self.gate.close();
        self.listener.admission.cancel();
    }
}

#[async_trait]
trait Registration: Send {
    async fn close(&self) -> Result<(), UStatus>;
}
#[async_trait]
trait Endpoint: Send + Sync {
    async fn ready(&self) -> Result<(), UStatus> {
        Ok(())
    }
    async fn send(&self, message: UMessage) -> Result<(), UStatus>;
    async fn register(
        &self,
        listener: Arc<Listener>,
        source: UUri,
        sink: Option<UUri>,
    ) -> Result<Box<dyn Registration>, UStatus>;
}
struct ClassicEndpoint(Arc<dyn UTransport>);
struct ClassicRegistration {
    transport: Arc<dyn UTransport>,
    source: UUri,
    sink: Option<UUri>,
    listener: Arc<dyn UListener>,
}
#[async_trait]
impl Registration for ClassicRegistration {
    async fn close(&self) -> Result<(), UStatus> {
        self.transport
            .unregister_listener(&self.source, self.sink.as_ref(), self.listener.clone())
            .await
    }
}
#[async_trait]
impl Endpoint for ClassicEndpoint {
    async fn send(&self, message: UMessage) -> Result<(), UStatus> {
        self.0.send(message).await
    }
    async fn register(
        &self,
        listener: Arc<Listener>,
        source: UUri,
        sink: Option<UUri>,
    ) -> Result<Box<dyn Registration>, UStatus> {
        let listener: Arc<dyn UListener> = listener;
        self.0
            .register_listener(&source, sink.as_ref(), listener.clone())
            .await?;
        Ok(Box::new(ClassicRegistration {
            transport: self.0.clone(),
            source,
            sink,
            listener,
        }))
    }
}
struct NativeEndpoint<T: PreparedEndpoint> {
    transport: Arc<PreparedSession<T>>,
    native: NativeContext,
    loans: Arc<Loans>,
    wait_matching: bool,
}
// A rendezvous for one existing transient native loan, not an additional
// admitted inbox. Each supported native registration serializes its pushes.
// The callback waits for application admission to finish before returning,
// so its successor cannot accumulate behind this worker. Every resource is
// created at registration; no task, channel node or payload copy is made per push.
struct NativeHandoffState<R> {
    closed: bool,
    pending: Option<R>,
    active: bool,
    failed: bool,
}
struct NativeHandoff<R> {
    state: Mutex<NativeHandoffState<R>>,
    ready: Condvar,
    completed: Condvar,
}
impl<R: Send + 'static> NativeHandoff<R> {
    fn start(
        mut consume: impl FnMut(R) + Send + 'static,
    ) -> Result<(Arc<Self>, std::thread::JoinHandle<()>), UStatus> {
        let shared = Arc::new(Self {
            state: Mutex::new(NativeHandoffState {
                closed: false,
                pending: None,
                active: false,
                failed: false,
            }),
            ready: Condvar::new(),
            completed: Condvar::new(),
        });
        let worker_shared = Arc::clone(&shared);
        let worker = std::thread::Builder::new()
            .name("native-workload-rx".into())
            .spawn(move || loop {
                let mut state = worker_shared
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                while state.pending.is_none() && !state.closed {
                    state = worker_shared
                        .ready
                        .wait(state)
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                }
                let Some(raw) = state.pending.take() else {
                    break;
                };
                state.active = true;
                drop(state);
                let outcome =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| consume(raw)));
                let mut state = worker_shared
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                state.active = false;
                if outcome.is_err() {
                    state.failed = true;
                    state.closed = true;
                }
                worker_shared.completed.notify_all();
                if state.failed {
                    break;
                }
            })
            .map_err(failure)?;
        Ok((shared, worker))
    }
    fn close(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.closed = true;
        self.ready.notify_all();
        self.completed.notify_all();
    }
    fn failed(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .failed
    }
}
impl<R: up_rust::Frame> up_rust::FrameSink<R> for NativeHandoff<R> {
    fn push(&self, raw: R) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while (state.pending.is_some() || state.active) && !state.closed {
            state = self
                .completed
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        if state.closed {
            return;
        }
        state.pending = Some(raw);
        self.ready.notify_one();
        while (state.pending.is_some() || state.active) && !state.failed {
            state = self
                .completed
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }
    fn close(&self) {
        NativeHandoff::close(self);
    }
}

struct NativeRegistration<T: PreparedEndpoint> {
    handoff: Arc<NativeHandoff<<T::Core as PreparedTransportCore>::Rx>>,
    worker: Mutex<Option<std::thread::JoinHandle<()>>>,
    gate: Arc<NativeReceiveGate>,
    transport: Arc<PreparedSession<T>>,
    subscription: Mutex<Option<<T::Core as PreparedTransportCore>::Subscription>>,
    listener: Arc<Listener>,
}
impl<T: PreparedEndpoint> Drop for NativeRegistration<T> {
    fn drop(&mut self) {
        self.handoff.close();
        if let Some(worker) = self
            .worker
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            let _ = worker.join();
        }
        self.gate.close();
        self.listener.admission.cancel();
        if let Some(handle) = self
            .subscription
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_mut()
        {
            let _ = self.transport.core().detach(handle);
        }
    }
}
#[async_trait]
impl<T> Registration for NativeRegistration<T>
where
    T: PreparedEndpoint,
{
    async fn close(&self) -> Result<(), UStatus> {
        self.handoff.close();
        let worker = self.worker.lock().map_err(failure)?.take();
        if let Some(worker) = worker {
            worker
                .join()
                .map_err(|_| failure("native callback worker panicked"))?;
        }
        self.gate.close();
        self.listener.admission.cancel();
        {
            let mut subscription = self.subscription.lock().map_err(failure)?;
            if let Some(handle) = subscription.as_mut() {
                self.transport.core().detach(handle).map_err(failure)?;
            }
            subscription.take();
        }
        if !self.listener.admission.is_current_callback() {
            self.listener
                .admission
                .quiesce(tokio::time::Instant::now() + Duration::from_secs(5))
                .await
                .map_err(failure)?;
        }
        if self.handoff.failed() {
            return Err(failure("native callback worker failed"));
        }
        Ok(())
    }
}
#[async_trait]
impl<T> Endpoint for NativeEndpoint<T>
where
    T: PreparedEndpoint,
    T::Core: NativeSetupReady,
{
    async fn ready(&self) -> Result<(), UStatus> {
        self.transport.wait_ready(self.wait_matching).await
    }
    async fn send(&self, message: UMessage) -> Result<(), UStatus> {
        let metadata = if self.native.configured_agreement().is_some() {
            self.native.project_classic(&message)?
        } else {
            up_rust::frame::metadata::try_project_umessage_to_frame_metadata(&message)
                .map_err(failure)?
        };
        let bytes = message
            .payload()
            .ok_or_else(|| failure("payload missing"))?;
        let guard = LoanGuard::new(self.loans.clone(), true);
        let result = self
            .transport
            .send(up_rust::TxHeaderParts::from(&metadata), Some(&bytes))
            .map_err(failure);
        drop(guard);
        result
    }
    async fn register(
        &self,
        listener: Arc<Listener>,
        source: UUri,
        sink: Option<UUri>,
    ) -> Result<Box<dyn Registration>, UStatus> {
        let gate = Arc::new(NativeReceiveGate(Mutex::new(true)));
        let native = Arc::new(NativeListener {
            gate: Arc::clone(&gate),
            listener: Arc::clone(&listener),
            session: Arc::clone(&self.transport),
            source: source.clone(),
            sink: sink.clone(),
        });
        let (handoff, worker) = NativeHandoff::start(move |raw| {
            up_rust::FrameSink::push(native.as_ref(), raw);
        })?;
        let subscription = self
            .transport
            .core()
            .attach(&source, sink.as_ref(), handoff.clone())
            .await;
        let subscription = match subscription {
            Ok(subscription) => subscription,
            Err(error) => {
                handoff.close();
                let _ = worker.join();
                return Err(failure(error));
            }
        };
        Ok(Box::new(NativeRegistration {
            handoff,
            worker: Mutex::new(Some(worker)),
            gate,
            transport: self.transport.clone(),
            subscription: Mutex::new(Some(subscription)),
            listener,
        }))
    }
}

pub async fn classic<F, G>(
    transport: Arc<dyn UTransport>,
    native: NativeContext,
    plan: WorkerPlan,
    source: bool,
    filters: Option<(UUri, Option<UUri>)>,
    message: F,
    finalize: G,
) -> Result<(), UStatus>
where
    F: FnMut(u32) -> Result<UMessage, UStatus>,
    G: FnOnce() -> Result<(), UStatus>,
{
    drive(
        Box::new(ClassicEndpoint(transport)),
        native,
        plan,
        source,
        filters,
        message,
        finalize,
        Arc::new(Loans::default()),
    )
    .await
}

// The approved sensor source has two real native readers: the bridge and a
// source-side fanout witness. This reader borrows and releases each actual frame;
// it never forwards a duplicate, copies a payload, or adds an application inbox.
#[cfg(feature = "payload-contract-large-fixtures")]
struct SensorFanout<T: PreparedEndpoint> {
    session: Arc<PreparedSession<T>>,
    source: UUri,
    received: AtomicU64,
    errors: AtomicU64,
}

#[cfg(feature = "payload-contract-large-fixtures")]
impl<T: PreparedEndpoint> up_rust::FrameSink<<T::Core as PreparedTransportCore>::Rx>
    for SensorFanout<T>
{
    fn push(&self, raw: <T::Core as PreparedTransportCore>::Rx) {
        let result = (|| {
            let frame = self.session.admit(raw).map_err(failure)?;
            let header = frame.header().map_err(failure)?;
            if header.source != self.source || header.sink.is_some() {
                return Err(failure("sensor fanout address differs"));
            }
            let record = streamer_flow_evidence::Observation::header(
                "fanout_receive",
                Some("sensor-source-native-fanout"),
                &header,
                frame.payload().map(|payload| payload.bytes()),
            )
            .map_err(failure)?
            .ok_or_else(|| failure("sensor fanout evidence disabled"))?;
            streamer_flow_evidence::emit_json_record("NATIVE_FANOUT_OBSERVATION ", &record)
                .map_err(failure)?;
            self.received.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })();
        if let Err(error) = result {
            self.errors.fetch_add(1, Ordering::SeqCst);
            streamer_flow_evidence::emit_json_or_fail(
                "NATIVE_FANOUT_ERROR ",
                &json!({"error": error.to_string()}),
            );
        }
    }
    fn close(&self) {}
}

/// Drive the existing native role, retaining both real sensor registrations
/// through drain and the end census. The source-side witness is detached after
/// the controller's Stop; its hashing/formatting costs stay in process totals.
pub async fn native<T, F>(
    transport: Arc<PreparedSession<T>>,
    native: NativeContext,
    plan: WorkerPlan,
    source: bool,
    filters: Option<(UUri, Option<UUri>)>,
    wait_matching: bool,
    mut message: F,
) -> Result<(), UStatus>
where
    T: PreparedEndpoint,
    T::Core: NativeSetupReady,
    F: FnMut(u32) -> Result<UMessage, UStatus>,
{
    #[cfg(feature = "payload-contract-large-fixtures")]
    let fanout = if source && native.fixture().is_some() {
        let metadata = native.project_classic(&message(0)?)?;
        let witness = Arc::new(SensorFanout {
            session: Arc::clone(&transport),
            source: metadata.source().clone(),
            received: AtomicU64::new(0),
            errors: AtomicU64::new(0),
        });
        let subscription = transport
            .core()
            .attach(metadata.source(), None, witness.clone())
            .await
            .map_err(failure)?;
        streamer_flow_evidence::emit_json_or_fail(
            "NATIVE_FANOUT_REGISTERED ",
            &json!({"source": metadata.source().to_uri(false),
            "registration": "source-native-fanout-witness", "application_inbox_capacity": 0,
            "payload_copies": 0}),
        );
        Some((Arc::clone(&transport), subscription, witness))
    } else {
        None
    };
    #[cfg(not(feature = "payload-contract-large-fixtures"))]
    let _ = &mut message;
    let loans = Arc::new(Loans::default());
    let final_transport = Arc::clone(&transport);
    let endpoint = NativeEndpoint {
        transport,
        native: native.clone(),
        loans: loans.clone(),
        wait_matching,
    };
    let outcome = drive(
        Box::new(endpoint),
        native,
        plan,
        source,
        filters,
        message,
        move || {
            let counters = final_transport.core().native_failure_counters();
            if !counters.is_empty() {
                let evidence: serde_json::Map<String, serde_json::Value> = counters
                    .iter()
                    .map(|(name, count)| ((*name).to_owned(), json!(count)))
                    .collect();
                streamer_flow_evidence::emit_json_or_fail("OP_TRANSPORT_HEALTH ", &evidence);
            }
            native_health_status(&counters)
        },
        loans,
    )
    .await;
    #[cfg(feature = "payload-contract-large-fixtures")]
    if let Some((session, mut subscription, witness)) = fanout {
        session.core().detach(&mut subscription).map_err(failure)?;
        let received = witness.received.load(Ordering::SeqCst);
        let errors = witness.errors.load(Ordering::SeqCst);
        streamer_flow_evidence::emit_json_or_fail(
            "NATIVE_FANOUT_FINISH ",
            &json!({"received": received, "errors": errors,
            "success": outcome.is_ok() && received > 0 && errors == 0}),
        );
        if outcome.is_ok() && (received == 0 || errors != 0) {
            return Err(failure("sensor native fanout witness failed"));
        }
    }
    outcome
}

fn native_health_status(counters: &[(&str, u64)]) -> Result<(), UStatus> {
    if let Some((name, count)) = counters.iter().find(|(_, count)| *count != 0) {
        return Err(failure(format!(
            "native transport health error: {name}={count}"
        )));
    }
    Ok(())
}

/// Records an RPC client's response as processed and releases it.
fn complete_response(
    listener: &Listener,
    delivery: inbox::Delivery<Packet>,
) -> Result<(), UStatus> {
    let packet = delivery.value();
    listener.emit_bytes("processed", &packet.metadata, packet.bytes(), None)?;
    delivery.complete().map_err(failure)
}

/// Runs application traffic over the role's existing classic SDK endpoint.
#[allow(clippy::too_many_arguments)]
async fn drive<F, G>(
    transport: Box<dyn Endpoint>,
    native: NativeContext,
    plan: WorkerPlan,
    source: bool,
    filters: Option<(UUri, Option<UUri>)>,
    mut message: F,
    finalize: G,
    loans: Arc<Loans>,
) -> Result<(), UStatus>
where
    F: FnMut(u32) -> Result<UMessage, UStatus>,
    G: FnOnce() -> Result<(), UStatus>,
{
    #[cfg(feature = "payload-contract-large-fixtures")]
    if native.fixture() != plan.native_fixture {
        return Err(failure(
            "workload native context differs from explicit sensor plan",
        ));
    }
    let actor = if source { "source" } else { "sink" };
    if std::env::var("UPROTOCOL_FLOW_EVIDENCE_RUN").as_deref() != Ok(plan.replay.run.as_str())
        || std::env::var("UPROTOCOL_FLOW_EVIDENCE_ACTOR").as_deref() != Ok(actor)
    {
        return Err(failure(
            "workload evidence context differs from configured role",
        ));
    }
    streamer_flow_evidence::emit_json_or_fail(
        "OP_START ",
        &json!({"actor": actor, "replay": plan.replay}),
    );
    let (sender, mut receiver) =
        inbox::bounded(plan.replay.queue_capacity as usize).map_err(failure)?;
    let listener = Arc::new(Listener {
        admission: up_rust::ListenerAdmission::new(),
        sender,
        inline: source,
        phase: Arc::new(Mutex::new(
            PhaseTracker::new(&plan.replay.plan_sha256).map_err(failure)?,
        )),
        native,
        maximum: plan.maximum_payload_bytes,
        errors: AtomicU64::new(0),
        loans,
    });
    let registration = if let Some((source, sink)) = &filters {
        Some(
            transport
                .register(listener.clone(), source.clone(), sink.clone())
                .await?,
        )
    } else {
        None
    };
    transport.ready().await?;
    println!("READY listener_registered");
    let drive = async {
        let mut control = tokio::time::interval(Duration::from_millis(25));
        control.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut offers = tokio::time::interval(Duration::from_nanos(
            1_000_000_000 / u64::from(plan.offered_per_second),
        ));
        offers.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut sequence = 0u32;
        let mut drained = false;
        let mut last_metrics = Instant::now();
        loop {
            tokio::select! {
                biased;
                _ = control.tick() => {
                    let command: PhaseCommand = read_json(&plan.control_file)?;
                    listener.phase.lock().map_err(failure)?.apply(&command).map_err(failure)?;
                    if listener.errors.load(Ordering::SeqCst) != 0 { return Err(failure("workload listener failed")); }
                    if matches!(listener.phase()?, Phase::Drain | Phase::Stop) && source && !drained {
                        streamer_flow_evidence::emit_json_or_fail("OP_SOURCE_DRAINED ", &json!({"offered": sequence}));
                        drained = true;
                    }
                    if listener.phase()? == Phase::Stop { break; }
                    if last_metrics.elapsed() >= Duration::from_millis(250) {
                        listener.metrics(false)?;
                        last_metrics = Instant::now();
                    }
                }
                _ = offers.tick(), if source => {
                    if !listener.phase()?.offers_messages() { continue; }
                    if let Some(limit)=plan.finite_messages {
                        if listener.phase()? != Phase::Normal || sequence>=limit { continue; }
                    }
                    sequence = sequence.checked_add(1).ok_or_else(|| failure("workload sequence exhausted"))?;
                    let value = message(sequence)?;
                    let metadata = listener.metadata(&value)?;
                    listener.emit("offer", &value, &metadata, Some(sequence))?;
                    match transport.send(value.clone()).await {
                        Ok(()) => listener.emit("accepted", &value, &metadata, Some(sequence))?,
                        Err(error) => {
                            listener.emit("rejected", &value, &metadata, Some(sequence))?;
                            streamer_flow_evidence::emit_json_or_fail("OP_REJECTION ", &json!({"sequence": sequence, "error": error.to_string()}));
                        }
                    }
                    if plan.finite_messages.is_some() {
                        offers.reset_after(Duration::from_millis(50));
                    }
                }
                // Only a sink consumes the inbox: an RPC client's listener completes its responses.
                delivery = receiver.recv(), if filters.is_some() && !source => {
                    let delivery = delivery.ok_or_else(|| failure("workload receiver disconnected"))?;
                    if listener.phase()? == Phase::Slow {
                        tokio::time::sleep(Duration::from_millis(plan.consumer_delay_ms)).await;
                    }
                    let packet = delivery.value();
                    listener.emit_bytes("processed", &packet.metadata, packet.bytes(), None)?;
                    if plan.replay.role == Role::ClientServerRpc {
                        let mut builder = UMessageBuilder::response(packet.metadata.source().clone(), packet.metadata.id().clone(),
                            packet.metadata.sink().ok_or_else(|| failure("request sink missing"))?.clone());
                        builder.with_comm_status(UCode::Ok);
                        let response = builder.build_with_payload(packet.bytes().to_vec(),
                            packet.metadata.payload_encoding().cloned().ok_or_else(|| failure("request encoding missing"))?).map_err(failure)?;
                        let metadata = listener.metadata(&response)?;
                        listener.emit("response_offer", &response, &metadata, None)?;
                        transport.send(response.clone()).await?;
                        listener.emit("response_accepted", &response, &metadata, None)?;
                    }
                    delivery.complete().map_err(failure)?;
                }
            }
        }
        Ok(())
    };
    let outcome = tokio::time::timeout(Duration::from_secs(plan.deadline_seconds), drive)
        .await
        .map_err(|_| failure("workload deadline expired"))
        .and_then(|result| result);
    listener.admission.cancel();
    let close_deadline =
        listener.admission.closed_at().expect("closed admission") + Duration::from_secs(5);
    let cleanup = if let Some(registration) = &registration {
        tokio::time::timeout_at(close_deadline, registration.close())
            .await
            .map_err(|_| failure("workload unregister deadline expired"))
            .and_then(|result| result)
    } else {
        Ok(())
    };
    let quiescence = listener
        .admission
        .quiesce(close_deadline)
        .await
        .map_err(failure);
    let cleanup = cleanup.and(quiescence);
    receiver.close();
    drop(receiver);
    drop(registration);
    drop(transport);
    let finalized = finalize();
    listener.metrics(true)?;
    let snapshot = listener.sender.snapshot();
    let complete = outcome.is_ok()
        && cleanup.is_ok()
        && finalized.is_ok()
        && snapshot.queued == 0
        && snapshot.live_held == 0
        && snapshot.abandoned == 0
        && snapshot.closed_rejections == 0
        && listener.errors.load(Ordering::SeqCst) == 0;
    streamer_flow_evidence::emit_json_or_fail(
        "OP_FINISH ",
        &json!({"actor": actor, "success": complete,
        "error": outcome.as_ref().err().or(cleanup.as_ref().err()).or(finalized.as_ref().err()).map(ToString::to_string)}),
    );
    outcome?;
    cleanup?;
    finalized?;
    if !complete {
        return Err(failure("workload did not drain completely"));
    }
    Ok(())
}

/// Run the explicit controller-only finite sink plan on this existing endpoint.
/// Ordinary role CLI behavior is unchanged when the environment is absent.
pub async fn finite_sink(
    transport: Arc<dyn UTransport>,
    native: NativeContext,
    source: UUri,
) -> Result<bool, UStatus> {
    let Some(path) = std::env::var_os("UPROTOCOL_FINITE_SINK_PLAN") else {
        return Ok(false);
    };
    let plan = WorkerPlan::load(Path::new(&path))?;
    if plan.finite_messages != Some(5)
        || plan.replay.role != Role::PublisherSubscriber
        || !plan.replay.require_fifo
        || plan.replay.pending_limit != 4096
    {
        return Err(failure(
            "finite sink requires the exact five-publication FIFO profile",
        ));
    }
    classic(
        transport,
        native,
        plan,
        false,
        Some((source, None)),
        |_| Err(failure("passive finite sink must never originate traffic")),
        || Ok(()),
    )
    .await?;
    Ok(true)
}

#[cfg(test)]
mod rpc_burst_tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use up_rust::{PayloadEncoding, UUID};

    #[test]
    fn native_health_rejects_each_binding_failure() {
        let names = [
            "zenoh_publish_failed",
            "zenoh_rejected_non_shm",
            "zenoh_rejected_layout",
        ];
        assert!(native_health_status(&[]).is_ok());
        assert!(native_health_status(&names.map(|name| (name, 0))).is_ok());
        for name in names {
            let error = native_health_status(&[(name, 1)]).unwrap_err();
            assert!(error.to_string().contains(name));
        }
    }

    /// Selects the child run; its value is the scratch directory.
    const BURST_CHILD: &str = "UPROTOCOL_TEST_RPC_BURST_CHILD";
    const RUN: &str = "5f1c2ab0e7d94e0f8d0a7b3c6e9f1a2b4c5d6e7f8091a2b3c4d5e6f708192a3b";
    const PLAN: &str = "0d6b1c3a5e7f9012a4c6e8f0b2d4f6a8c0e2a4c6e8f0b2d4f6a8c0e2a4c6e8f0";
    const CAPACITY: usize = 4;
    /// Two more responses than the inbox holds, like the bursts that follow a first-message stall.
    const BURST: usize = CAPACITY + 2;
    const CLIENT: &str = "//burst-client/5BA0/1/1/0";
    const METHOD: &str = "//burst-server/5BA0/1/1/1000";

    fn uri(value: &str) -> UUri {
        UUri::try_from(value).unwrap()
    }

    fn response(index: usize) -> UMessage {
        let mut builder = UMessageBuilder::response(uri(CLIENT), UUID::build(), uri(METHOD));
        builder.with_comm_status(UCode::Ok);
        builder
            .build_with_payload(
                format!("response {index}").into_bytes(),
                PayloadEncoding::RAW,
            )
            .unwrap()
    }

    struct Unregister;

    #[async_trait]
    impl Registration for Unregister {
        async fn close(&self) -> Result<(), UStatus> {
            Ok(())
        }
    }

    /// The first request's send delivers a burst of `BURST` responses back to back through the
    /// registered listener before it returns, as a binding delivers a burst in one pass: the
    /// drive task that awaits the send cannot consume anything in between.
    struct BurstEndpoint {
        listener: Mutex<Option<Arc<Listener>>>,
        sends: AtomicU64,
        burst_delivered: AtomicBool,
    }

    #[async_trait]
    impl Endpoint for BurstEndpoint {
        async fn send(&self, _: UMessage) -> Result<(), UStatus> {
            if self.sends.fetch_add(1, Ordering::SeqCst) == 0 {
                let listener = self
                    .listener
                    .lock()
                    .unwrap()
                    .clone()
                    .expect("registered listener");
                for index in 0..BURST {
                    listener.on_receive(response(index)).await;
                }
                self.burst_delivered.store(true, Ordering::SeqCst);
            }
            Ok(())
        }

        async fn register(
            &self,
            listener: Arc<Listener>,
            _: UUri,
            _: Option<UUri>,
        ) -> Result<Box<dyn Registration>, UStatus> {
            *self.listener.lock().unwrap() = Some(listener);
            Ok(Box::new(Unregister))
        }
    }

    fn write_phase(control: &Path, phase: &str) {
        let staged = control.with_extension("staged");
        std::fs::write(
            &staged,
            json!({"plan_sha256": PLAN, "phase": phase}).to_string(),
        )
        .unwrap();
        std::fs::rename(&staged, control).unwrap();
    }

    /// Child half of `rpc_client_completes_a_response_burst_its_drive_task_cannot_consume`:
    /// drives a real RPC client worker against `BurstEndpoint`, then stops it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn rpc_client_burst_child() {
        let Some(directory) = std::env::var_os(BURST_CHILD) else {
            return;
        };
        let control = PathBuf::from(directory).join("control.json");
        write_phase(&control, "warmup");
        let plan: WorkerPlan = serde_json::from_value(json!({
            "replay": {"run": RUN, "source_identity": PLAN, "plan_sha256": PLAN, "role": "client-server-rpc",
                       "queue_capacity": CAPACITY, "pending_limit": 4096, "require_fifo": false},
            "control_file": control, "offered_per_second": 10, "consumer_delay_ms": 250,
            "maximum_payload_bytes": 8192, "deadline_seconds": 30
        }))
        .unwrap();
        let endpoint = Arc::new(BurstEndpoint {
            listener: Mutex::new(None),
            sends: AtomicU64::new(0),
            burst_delivered: AtomicBool::new(false),
        });
        let stopper = {
            let endpoint = Arc::clone(&endpoint);
            let control = control.clone();
            tokio::spawn(async move {
                // Stop once every burst response is accounted for, completed or dropped, so the
                // parent sees final counts however slowly the worker runs (bounded at 10 s).
                let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
                loop {
                    let listener = endpoint.listener.lock().unwrap().clone();
                    let settled = listener.is_some_and(|listener| {
                        let counts = listener.sender.snapshot();
                        endpoint.burst_delivered.load(Ordering::SeqCst)
                            && counts.processed + counts.overload_drops == BURST as u64
                    });
                    if settled || tokio::time::Instant::now() >= deadline {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                write_phase(&control, "stop");
            })
        };
        struct Shared(Arc<BurstEndpoint>);
        #[async_trait]
        impl Endpoint for Shared {
            async fn send(&self, message: UMessage) -> Result<(), UStatus> {
                self.0.send(message).await
            }
            async fn register(
                &self,
                listener: Arc<Listener>,
                source: UUri,
                sink: Option<UUri>,
            ) -> Result<Box<dyn Registration>, UStatus> {
                self.0.register(listener, source, sink).await
            }
        }
        drive(
            Box::new(Shared(Arc::clone(&endpoint))),
            NativeContext::default(),
            plan,
            true,
            Some((uri(METHOD), Some(uri(CLIENT)))),
            |sequence| {
                UMessageBuilder::request(uri(METHOD), uri(CLIENT), 5000)
                    .build_with_payload(
                        format!("request {sequence}").into_bytes(),
                        PayloadEncoding::RAW,
                    )
                    .map_err(failure)
            },
            || Ok(()),
            Arc::new(Loans::default()),
        )
        .await
        .expect("RPC client worker completes");
        stopper.await.unwrap();
        assert!(endpoint.burst_delivered.load(Ordering::SeqCst));
    }

    /// The bounded inbox holds `CAPACITY` and the burst is `CAPACITY + 2`: every response must
    /// still be processed, although the drive task cannot run while the burst arrives.
    #[test]
    fn rpc_client_completes_a_response_burst_its_drive_task_cannot_consume() {
        let directory = std::env::temp_dir().join(format!("rpc-burst-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        let records = directory.join("records.jsonl");
        std::fs::write(&records, b"").unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "common::workload::rpc_burst_tests::rpc_client_burst_child",
                "--nocapture",
                "--test-threads",
                "1",
            ])
            .env(BURST_CHILD, &directory)
            .env(streamer_flow_evidence::OUTPUT_PATH_ENV, &records)
            .env("UPROTOCOL_FLOW_EVIDENCE_RUN", RUN)
            .env("UPROTOCOL_FLOW_EVIDENCE_ACTOR", "source")
            .output()
            .unwrap();
        let mut events = std::collections::BTreeMap::<String, usize>::new();
        for line in std::fs::read_to_string(&records).unwrap().lines() {
            if let Some(body) = line.strip_prefix("OP_EVENT ") {
                let value: serde_json::Value = serde_json::from_str(body).unwrap();
                *events
                    .entry(value["event"].as_str().unwrap().to_owned())
                    .or_default() += 1;
            }
        }
        let _ = std::fs::remove_dir_all(&directory);
        assert!(
            output.status.success(),
            "RPC client worker child failed: {:?}\n{}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            events.get("queue_drop"),
            None,
            "an RPC client must complete every response of a burst its drive task cannot consume: {events:?}"
        );
        assert_eq!(
            events.get("processed"),
            Some(&BURST),
            "every burst response is processed: {events:?}"
        );
    }
}
