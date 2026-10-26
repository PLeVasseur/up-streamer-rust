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
use std::sync::{Arc, Mutex};
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
        println!(
            "OP_EVENT {}",
            json!({"event": event, "phase": self.phase()?, "sequence": sequence,
            "at_unix_ns": SystemTime::now().duration_since(UNIX_EPOCH).map_err(failure)?.as_nanos().to_string(),
            "attributes_sha256": format!("{:x}", Sha256::digest(attributes)), "observation": observation})
        );
        Ok(())
    }

    fn metrics(&self, final_record: bool) -> Result<(), UStatus> {
        let snapshot = self.sender.snapshot();
        println!(
            "OP_METRICS {}",
            json!({"phase": self.phase()?, "final": final_record,
            "queue_occupancy": snapshot.queued, "queue_capacity": snapshot.capacity,
            "received": snapshot.received, "processed": snapshot.processed,
            "queue_drops": snapshot.overload_drops,
            "errors": self.errors.load(Ordering::SeqCst) + snapshot.closed_rejections + snapshot.abandoned,
            "live_application_rx_loans": self.loans.rx.load(Ordering::SeqCst), "max_application_rx_loans": self.loans.peak_rx.load(Ordering::SeqCst),
            "live_application_tx_loans": self.loans.tx.load(Ordering::SeqCst), "max_application_tx_loans": self.loans.peak_tx.load(Ordering::SeqCst),
            "live_transient_rx_loans": self.loans.transient_rx.load(Ordering::SeqCst), "max_transient_rx_loans": self.loans.peak_transient_rx.load(Ordering::SeqCst),
            "live_application_values": snapshot.live_held, "max_application_values": snapshot.peak_held})
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
        match self.sender.try_send_with_admission(packet, |packet| {
            if let Payload::Loan(loan) = &mut packet.payload {
                loan.admit();
            }
        }) {
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
            println!("OP_ERROR {}", json!({"error": error.to_string()}));
        }
    }
}

struct NativeListener<T: PreparedEndpoint> {
    listener: Arc<Listener>,
    session: Arc<PreparedSession<T>>,
    source: UUri,
    sink: Option<UUri>,
}
impl<T: PreparedEndpoint> up_rust::FrameSink<<T::Core as PreparedTransportCore>::Rx>
    for NativeListener<T>
{
    fn push(&self, raw: <T::Core as PreparedTransportCore>::Rx) {
        let Some(_callback) = self.listener.admission.enter_native_callback() else {
            return;
        };
        let guard = LoanGuard::new(self.listener.loans.clone(), false);
        let admitted = match self.session.admit(raw) {
            Ok(frame) => frame,
            Err(_) => return,
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
                return Ok(());
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
struct NativeRegistration<T: PreparedEndpoint> {
    transport: Arc<PreparedSession<T>>,
    subscription: Mutex<Option<<T::Core as PreparedTransportCore>::Subscription>>,
    listener: Arc<Listener>,
}
impl<T: PreparedEndpoint> Drop for NativeRegistration<T> {
    fn drop(&mut self) {
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
        let native = Arc::new(NativeListener {
            listener: Arc::clone(&listener),
            session: Arc::clone(&self.transport),
            source: source.clone(),
            sink: sink.clone(),
        });
        let subscription = self
            .transport
            .core()
            .attach(&source, sink.as_ref(), native)
            .await
            .map_err(failure)?;
        Ok(Box::new(NativeRegistration {
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

pub async fn native<T, F>(
    transport: Arc<PreparedSession<T>>,
    native: NativeContext,
    plan: WorkerPlan,
    source: bool,
    filters: Option<(UUri, Option<UUri>)>,
    wait_matching: bool,
    message: F,
) -> Result<(), UStatus>
where
    T: PreparedEndpoint,
    T::Core: NativeSetupReady,
    F: FnMut(u32) -> Result<UMessage, UStatus>,
{
    let loans = Arc::new(Loans::default());
    let endpoint = NativeEndpoint {
        transport,
        native: native.clone(),
        loans: loans.clone(),
        wait_matching,
    };
    drive(
        Box::new(endpoint),
        native,
        plan,
        source,
        filters,
        message,
        || Ok(()),
        loans,
    )
    .await
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
    println!(
        "OP_START {}",
        json!({"actor": actor, "replay": plan.replay})
    );
    let (sender, mut receiver) =
        inbox::bounded(plan.replay.queue_capacity as usize).map_err(failure)?;
    let listener = Arc::new(Listener {
        admission: up_rust::ListenerAdmission::new(),
        sender,
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
                        println!("OP_SOURCE_DRAINED {}", json!({"offered": sequence}));
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
                            println!("OP_REJECTION {}", json!({"sequence": sequence, "error": error.to_string()}));
                        }
                    }
                    if plan.finite_messages.is_some() {
                        offers.reset_after(Duration::from_millis(50));
                    }
                }
                delivery = receiver.recv(), if filters.is_some() => {
                    let delivery = delivery.ok_or_else(|| failure("workload receiver disconnected"))?;
                    if !source && listener.phase()? == Phase::Slow {
                        tokio::time::sleep(Duration::from_millis(plan.consumer_delay_ms)).await;
                    }
                    let packet = delivery.value();
                    listener.emit_bytes("processed", &packet.metadata, packet.bytes(), None)?;
                    if !source && plan.replay.role == Role::ClientServerRpc {
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
    println!(
        "OP_FINISH {}",
        json!({"actor": actor, "success": complete,
        "error": outcome.as_ref().err().or(cleanup.as_ref().err()).or(finalized.as_ref().err()).map(ToString::to_string)})
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
