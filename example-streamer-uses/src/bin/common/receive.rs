// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Passive role readiness means registration has completed, not that polling will start.

use super::payloads::NativeContext;
use async_trait::async_trait;
use configurable_streamer_wire_support::prepared::{
    PreparedEndpoint, PreparedReceived, PreparedSession,
};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{oneshot, Notify};
use up_rust::{
    PreparedTransportCore, UCode, UMessageType, UOwnedFrame, UOwnedListener, UOwnedTransport,
    UStatus, UUri, UUID,
};

enum Expected {
    Any,
    Response(UUID),
}

struct OwnedState {
    expected: Option<Expected>,
    frame: Option<UOwnedFrame>,
    closed: bool,
}

struct OwnedInbox {
    state: Mutex<OwnedState>,
    ready: Notify,
}

impl OwnedInbox {
    fn clear(&self, close: bool) {
        let frame = {
            let mut state = self.state.lock().unwrap();
            state.closed |= close;
            state.expected = None;
            state.frame.take()
        };
        drop(frame);
        self.ready.notify_waiters();
    }
}

struct OwnedWait<'a>(&'a OwnedInbox);
impl Drop for OwnedWait<'_> {
    fn drop(&mut self) {
        self.0.clear(false);
    }
}

struct OwnedChannel(Arc<OwnedInbox>);

/// Deadline of a bounded passive receive, shared by the owned and native
/// receptions: the window opens at the controller's source release when one is
/// configured ([`super::startup::receive_window`]), otherwise at once.
async fn passive_deadline(
    timeout_ms: u64,
    overflow: &'static str,
) -> Result<tokio::time::Instant, UStatus> {
    super::startup::receive_window().await?;
    tokio::time::Instant::now()
        .checked_add(Duration::from_millis(timeout_ms))
        .ok_or_else(|| UStatus::fail_with_code(UCode::InvalidArgument, overflow))
}

pub fn listener_ready() {
    println!("READY listener_registered");
}

pub fn zenoh_listener_ready() {
    println!("READY zenoh_listener_registered");
    listener_ready();
}

#[async_trait]
impl UOwnedListener for OwnedChannel {
    async fn on_receive_owned(&self, frame: UOwnedFrame) {
        super::proof::received_frame(&frame);
        let mut state = self.0.state.lock().unwrap();
        let matches = match &state.expected {
            Some(Expected::Any) => true,
            Some(Expected::Response(id)) => {
                frame.metadata().kind() == UMessageType::Response
                    && frame.metadata().reqid() == Some(id)
            }
            None => false,
        };
        if state.closed || state.frame.is_some() || !matches {
            return;
        }
        state.frame = Some(frame);
        drop(state);
        self.0.ready.notify_one();
    }
}

/// One registered owned listener, one result slot and one setup-owned lifecycle task.
/// Dropping the reception closes admission immediately; the task finishes native
/// unregister even when the caller cancels registration, receive or explicit stop.
pub struct OwnedReception {
    inbox: Arc<OwnedInbox>,
    close: Option<oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<Result<(), UStatus>>>,
}

impl OwnedReception {
    pub fn expect_response(&mut self, id: &UUID) {
        let previous = {
            let mut state = self.inbox.state.lock().unwrap();
            state.expected = Some(Expected::Response(id.clone()));
            state.frame.take()
        };
        drop(previous);
    }

    /// Bounded receive; `passive_deadline` decides when its window opens.
    pub async fn receive(&mut self, timeout_ms: u64) -> Result<UOwnedFrame, UStatus> {
        let deadline = passive_deadline(timeout_ms, "owned receive deadline overflow").await?;
        self.receive_until(deadline).await
    }

    pub async fn receive_until(
        &mut self,
        deadline: tokio::time::Instant,
    ) -> Result<UOwnedFrame, UStatus> {
        let _waiting = OwnedWait(&self.inbox);
        loop {
            if tokio::time::Instant::now() >= deadline {
                return Err(UStatus::fail_with_code(
                    UCode::DeadlineExceeded,
                    "registered owned receive timed out",
                ));
            }
            let ready = self.inbox.ready.notified();
            tokio::pin!(ready);
            ready.as_mut().enable();
            {
                let mut state = self.inbox.state.lock().unwrap();
                if state.closed {
                    return Err(UStatus::fail_with_code(
                        UCode::Unavailable,
                        "registered owned receiver closed",
                    ));
                }
                if let Some(frame) = state.frame.take() {
                    state.expected = None;
                    return Ok(frame);
                }
            }
            tokio::time::timeout_at(deadline, ready)
                .await
                .map_err(|_| {
                    UStatus::fail_with_code(
                        UCode::DeadlineExceeded,
                        "registered owned receive timed out",
                    )
                })?;
        }
    }

    fn close(&mut self) {
        self.inbox.clear(true);
        if let Some(close) = self.close.take() {
            let _ = close.send(());
        }
    }

    pub async fn stop(mut self) -> Result<(), UStatus> {
        self.close();
        if let Some(task) = self.task.take() {
            task.await.map_err(|error| {
                UStatus::fail_with_code(
                    UCode::Internal,
                    format!("owned registration lifecycle task failed: {error}"),
                )
            })??;
        }
        Ok(())
    }
}

impl Drop for OwnedReception {
    fn drop(&mut self) {
        self.close();
        // Detach the setup task rather than aborting native unregister. This is
        // one lifecycle task per registration, never a task per received frame.
        self.task.take();
    }
}

pub async fn register_owned(
    transport: &Arc<dyn UOwnedTransport>,
    source: &UUri,
    sink: Option<&UUri>,
) -> Result<OwnedReception, UStatus> {
    let inbox = Arc::new(OwnedInbox {
        state: Mutex::new(OwnedState {
            expected: Some(Expected::Any),
            frame: None,
            closed: false,
        }),
        ready: Notify::new(),
    });
    let listener: Arc<dyn UOwnedListener> = Arc::new(OwnedChannel(Arc::clone(&inbox)));
    let transport = Arc::clone(transport);
    let source = source.clone();
    let sink = sink.cloned();
    let (ready, registered) = oneshot::channel();
    let (close, closed) = oneshot::channel();
    let task = tokio::spawn(async move {
        let result = transport
            .register_owned_listener(&source, sink.as_ref(), Arc::clone(&listener))
            .await;
        let active = result.is_ok();
        let _ = ready.send(result);
        if active {
            let _ = closed.await;
            transport
                .unregister_owned_listener(&source, sink.as_ref(), listener)
                .await
        } else {
            Ok(())
        }
    });
    let reception = OwnedReception {
        inbox,
        close: Some(close),
        task: Some(task),
    };
    registered.await.map_err(|error| {
        UStatus::fail_with_code(
            UCode::Internal,
            format!("owned registration did not complete: {error}"),
        )
    })??;
    Ok(reception)
}

pub type NativeFrame<T> =
    PreparedReceived<<<T as PreparedEndpoint>::Core as PreparedTransportCore>::Rx>;
pub struct NativeReception<T: PreparedEndpoint> {
    session: Arc<PreparedSession<T>>,
    inbox: Arc<up_rust::RegistrationInbox<<T::Core as PreparedTransportCore>::Rx>>,
    subscription: Option<<T::Core as PreparedTransportCore>::Subscription>,
    source: UUri,
    sink: Option<UUri>,
}
impl<T: PreparedEndpoint> NativeReception<T> {
    /// Bounded receive; `passive_deadline` decides when its window opens.
    pub async fn receive(&mut self, timeout_ms: u64) -> Result<NativeFrame<T>, UStatus> {
        let deadline = passive_deadline(timeout_ms, "native receive deadline overflow").await?;
        self.receive_until(deadline).await
    }
    pub async fn receive_until(
        &mut self,
        deadline: tokio::time::Instant,
    ) -> Result<NativeFrame<T>, UStatus> {
        loop {
            if tokio::time::Instant::now() >= deadline {
                return Err(UStatus::fail_with_code(
                    UCode::DeadlineExceeded,
                    "registered native receive timed out",
                ));
            }
            let raw = tokio::time::timeout_at(deadline, self.inbox.recv())
                .await
                .map_err(|_| {
                    UStatus::fail_with_code(
                        UCode::DeadlineExceeded,
                        "registered native receive timed out",
                    )
                })?
                .ok_or_else(|| {
                    UStatus::fail_with_code(UCode::Unavailable, "registered native receiver closed")
                })?;
            let frame = match self.session.admit(raw) {
                Ok(frame) => frame,
                Err(_) => continue,
            };
            let header = frame.header().map_err(|error| {
                UStatus::fail_with_code(UCode::InvalidArgument, error.to_string())
            })?;
            if !self.source.matches(&header.source)
                || !match (self.sink.as_ref(), header.sink.as_ref()) {
                    (None, None) => true,
                    (Some(filter), Some(actual)) => filter.matches(actual),
                    _ => false,
                }
            {
                continue;
            }
            let frame = self.session.decode_compatibility_frame(frame)?;
            super::proof::received_frame(&frame);
            super::payloads::verify_received_frame(self.session.as_ref(), &frame)?;
            return Ok(frame);
        }
    }
    pub async fn stop(mut self) -> Result<(), UStatus> {
        self.inbox.close();
        if let Some(subscription) = &mut self.subscription {
            self.session
                .core()
                .detach(subscription)
                .map_err(|error| UStatus::fail_with_code(UCode::Internal, error.to_string()))?;
        }
        self.subscription.take();
        Ok(())
    }
}
impl<T: PreparedEndpoint> Drop for NativeReception<T> {
    fn drop(&mut self) {
        self.inbox.close();
        if let Some(subscription) = &mut self.subscription {
            let _ = self.session.core().detach(subscription);
        }
    }
}
pub async fn register_native<T: PreparedEndpoint>(
    session: &Arc<PreparedSession<T>>,
    source: &UUri,
    sink: Option<&UUri>,
) -> Result<NativeReception<T>, UStatus> {
    let inbox = Arc::new(
        up_rust::RegistrationInbox::new(up_rust::RegistrationConfig {
            capacity: 4,
            overflow: up_rust::OverflowPolicy::Block {
                deadline: Duration::from_secs(5),
            },
        })
        .map_err(|error| UStatus::fail_with_code(UCode::FailedPrecondition, error.to_string()))?,
    );
    let subscription = session
        .core()
        .attach(source, sink, inbox.clone())
        .await
        .map_err(|error| UStatus::fail_with_code(UCode::FailedPrecondition, error.to_string()))?;
    Ok(NativeReception {
        session: Arc::clone(session),
        inbox,
        subscription: Some(subscription),
        source: source.clone(),
        sink: sink.cloned(),
    })
}

pub async fn owned(
    transport: &Arc<dyn UOwnedTransport>,
    source: &UUri,
    sink: Option<&UUri>,
    timeout_ms: u64,
    native: Option<&NativeContext>,
) -> Result<UOwnedFrame, UStatus> {
    owned_with_ready(transport, source, sink, timeout_ms, native, || {
        println!("READY listener_registered");
    })
    .await
}

/// Receive through a registered listener, announcing endpoint-specific readiness
/// only after admission succeeds and removing the listener on receipt or timeout.
pub async fn owned_with_ready(
    transport: &Arc<dyn UOwnedTransport>,
    source: &UUri,
    sink: Option<&UUri>,
    timeout_ms: u64,
    native: Option<&NativeContext>,
    ready: impl FnOnce(),
) -> Result<UOwnedFrame, UStatus> {
    let mut receiver = register_owned(transport, source, sink).await?;
    ready();
    let result = receiver.receive(timeout_ms).await;
    let cleanup = receiver.stop().await;
    let frame = result?;
    cleanup?;
    if let Some(native) = native {
        native.verify_owned(frame.metadata(), frame.payload_bytes())?;
    }
    Ok(frame)
}

pub async fn owned_payload(
    transport: &Arc<dyn UOwnedTransport>,
    source: &UUri,
    sink: Option<&UUri>,
    timeout_ms: u64,
    native: Option<&NativeContext>,
) -> Result<Vec<u8>, UStatus> {
    owned(transport, source, sink, timeout_ms, native)
        .await
        .map(|frame| frame.payload_bytes().to_vec())
}

pub async fn zero_copy<T>(
    transport: &Arc<PreparedSession<T>>,
    source: &UUri,
    sink: Option<&UUri>,
    timeout_ms: u64,
) -> Result<NativeFrame<T>, UStatus>
where
    T: PreparedEndpoint,
{
    zero_copy_with_ready(transport, source, sink, timeout_ms, || {
        println!("READY listener_registered");
    })
    .await
}

/// Native counterpart to [`owned_with_ready`], retaining the received lease
/// through quiescent unregister and validating its representation before return.
pub async fn zero_copy_with_ready<T>(
    transport: &Arc<PreparedSession<T>>,
    source: &UUri,
    sink: Option<&UUri>,
    timeout_ms: u64,
    ready: impl FnOnce(),
) -> Result<NativeFrame<T>, UStatus>
where
    T: PreparedEndpoint,
{
    let mut receiver = register_native(transport, source, sink).await?;
    ready();
    let result = receiver.receive(timeout_ms).await;
    let cleanup = receiver.stop().await;
    let frame = result?;
    cleanup?;
    super::payloads::verify_received_frame(transport.as_ref(), &frame)?;
    Ok(frame)
}

pub async fn zero_copy_payload<T>(
    transport: &Arc<PreparedSession<T>>,
    source: &UUri,
    sink: Option<&UUri>,
    timeout_ms: u64,
) -> Result<Vec<u8>, UStatus>
where
    T: PreparedEndpoint,
{
    zero_copy(transport, source, sink, timeout_ms)
        .await
        .and_then(|frame| super::payloads::copy_payload_bytes(&frame))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use up_rust::{PayloadEncoding, UFrameMetadata, UOwnedTransportImpl};

    struct ImmediateDelivery {
        removed: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl UOwnedTransportImpl for ImmediateDelivery {
        async fn send_validated_owned(&self, _: UOwnedFrame) -> Result<(), UStatus> {
            unreachable!("passive test never sends")
        }

        async fn register_validated_owned_listener(
            &self,
            source: &UUri,
            _: Option<&UUri>,
            listener: Arc<dyn UOwnedListener>,
        ) -> Result<(), UStatus> {
            let metadata = UFrameMetadata::publish(source.clone())
                .with_payload_encoding(PayloadEncoding::RAW)
                .build()
                .unwrap();
            listener
                .on_receive_owned(
                    UOwnedFrame::with_payload(metadata, b"first frame".to_vec()).unwrap(),
                )
                .await;
            Ok(())
        }

        async fn unregister_validated_owned_listener(
            &self,
            _: &UUri,
            _: Option<&UUri>,
            _: Arc<dyn UOwnedListener>,
        ) -> Result<(), UStatus> {
            self.removed.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[tokio::test]
    async fn frame_arriving_during_registration_is_retained_and_listener_removed() {
        let removed = Arc::new(AtomicUsize::new(0));
        let transport: Arc<dyn UOwnedTransport> = Arc::new(ImmediateDelivery {
            removed: removed.clone(),
        });
        let source = UUri::try_from_parts("ready-test", 0x0001_5BA0, 1, 0x8001).unwrap();
        let frame = owned(&transport, &source, None, 1000, None).await.unwrap();
        assert_eq!(frame.payload_bytes(), b"first frame");
        assert_eq!(removed.load(Ordering::SeqCst), 1);
    }

    struct RejectedRegistration;

    #[async_trait]
    impl UOwnedTransportImpl for RejectedRegistration {
        async fn send_validated_owned(&self, _: UOwnedFrame) -> Result<(), UStatus> {
            unreachable!("passive test never sends")
        }

        async fn register_validated_owned_listener(
            &self,
            _: &UUri,
            _: Option<&UUri>,
            _: Arc<dyn UOwnedListener>,
        ) -> Result<(), UStatus> {
            Err(UStatus::fail_with_code(
                UCode::Unavailable,
                "admission rejected",
            ))
        }

        async fn unregister_validated_owned_listener(
            &self,
            _: &UUri,
            _: Option<&UUri>,
            _: Arc<dyn UOwnedListener>,
        ) -> Result<(), UStatus> {
            panic!("failed registration must not be treated as admitted")
        }
    }

    #[tokio::test]
    async fn rejected_registration_never_announces_endpoint_readiness() {
        let transport: Arc<dyn UOwnedTransport> = Arc::new(RejectedRegistration);
        let source = UUri::try_from_parts("ready-test", 0x0001_5BA0, 1, 0x8001).unwrap();
        let error = owned_with_ready(&transport, &source, None, 1000, None, || {
            panic!("readiness must follow successful registration");
        })
        .await
        .unwrap_err();
        assert_eq!(error.code(), UCode::Unavailable);
    }

    struct Controlled {
        listener: Mutex<Option<Arc<dyn UOwnedListener>>>,
        registration_entered: Notify,
        registration_release: Notify,
        removal_entered: Notify,
        removal_release: Notify,
        removed: Notify,
        removals: AtomicUsize,
        pause_registration: bool,
        pause_removal: bool,
    }
    impl Controlled {
        fn new(pause_registration: bool, pause_removal: bool) -> Arc<Self> {
            Arc::new(Self {
                listener: Mutex::new(None),
                registration_entered: Notify::new(),
                registration_release: Notify::new(),
                removal_entered: Notify::new(),
                removal_release: Notify::new(),
                removed: Notify::new(),
                removals: AtomicUsize::new(0),
                pause_registration,
                pause_removal,
            })
        }
    }
    #[async_trait]
    impl UOwnedTransportImpl for Controlled {
        async fn send_validated_owned(&self, _: UOwnedFrame) -> Result<(), UStatus> {
            unreachable!()
        }
        async fn register_validated_owned_listener(
            &self,
            _: &UUri,
            _: Option<&UUri>,
            listener: Arc<dyn UOwnedListener>,
        ) -> Result<(), UStatus> {
            *self.listener.lock().unwrap() = Some(listener);
            self.registration_entered.notify_one();
            if self.pause_registration {
                self.registration_release.notified().await;
            }
            Ok(())
        }
        async fn unregister_validated_owned_listener(
            &self,
            _: &UUri,
            _: Option<&UUri>,
            _: Arc<dyn UOwnedListener>,
        ) -> Result<(), UStatus> {
            self.removal_entered.notify_one();
            if self.pause_removal {
                self.removal_release.notified().await;
            }
            self.listener.lock().unwrap().take();
            self.removals.fetch_add(1, Ordering::SeqCst);
            self.removed.notify_one();
            Ok(())
        }
    }

    async fn notified(notify: &Notify) {
        tokio::time::timeout(Duration::from_secs(2), notify.notified())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn cancellation_during_registration_retires_the_late_native_registration() {
        let control = Controlled::new(true, false);
        let transport: Arc<dyn UOwnedTransport> = control.clone();
        let caller = tokio::spawn(async move {
            owned(
                &transport,
                &UUri::try_from("//ready-test/5BA0/1/1/8001").unwrap(),
                None,
                1000,
                None,
            )
            .await
        });
        notified(&control.registration_entered).await;
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
        control.registration_release.notify_one();
        notified(&control.removed).await;
        assert_eq!(control.removals.load(Ordering::SeqCst), 1);
        assert!(control.listener.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn cancellation_while_receiving_retires_the_registration() {
        let control = Controlled::new(false, false);
        let transport: Arc<dyn UOwnedTransport> = control.clone();
        let ready = Arc::new(Notify::new());
        let callback = Arc::clone(&ready);
        let caller = tokio::spawn(async move {
            owned_with_ready(
                &transport,
                &UUri::try_from("//ready-test/5BA0/1/1/8001").unwrap(),
                None,
                1000,
                None,
                move || callback.notify_one(),
            )
            .await
        });
        notified(&ready).await;
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
        notified(&control.removed).await;
        assert_eq!(control.removals.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn cancelling_stop_keeps_the_setup_owned_retirement_task_alive() {
        let control = Controlled::new(false, true);
        let transport: Arc<dyn UOwnedTransport> = control.clone();
        let receiver = register_owned(
            &transport,
            &UUri::try_from("//ready-test/5BA0/1/1/8001").unwrap(),
            None,
        )
        .await
        .unwrap();
        let stopping = tokio::spawn(receiver.stop());
        notified(&control.removal_entered).await;
        stopping.abort();
        assert!(stopping.await.unwrap_err().is_cancelled());
        control.removal_release.notify_one();
        notified(&control.removed).await;
        assert_eq!(control.removals.load(Ordering::SeqCst), 1);
    }

    const WINDOW_CHILD: &str = "EXAMPLE_STREAMER_USES_RECEIVE_WINDOW_CHILD";
    const WINDOW_MS: u64 = 1000;

    /// Child half of the receive-window regression; a no-op unless launched by
    /// it. A passive registers, reports readiness and receives with a bounded
    /// window; the source sends only after the controller's release.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn receive_window_child() {
        let Some(release) = std::env::var_os(WINDOW_CHILD) else {
            return;
        };
        // Start a fresh line after libtest's unterminated "test ... " prefix.
        println!();
        let release = std::path::PathBuf::from(release);
        let release_path = release.clone();
        let control = Controlled::new(false, false);
        let transport: Arc<dyn UOwnedTransport> = control.clone();
        let source = UUri::try_from("//window-test/5BA0/1/1/8001").unwrap();
        let sender = {
            let control = Arc::clone(&control);
            let source = source.clone();
            tokio::spawn(async move {
                while !release.exists() {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                let listener = control.listener.lock().unwrap().clone();
                let metadata = UFrameMetadata::publish(source)
                    .with_payload_encoding(PayloadEncoding::RAW)
                    .build()
                    .unwrap();
                listener
                    .expect("registered listener")
                    .on_receive_owned(
                        UOwnedFrame::with_payload(metadata, b"released".to_vec()).unwrap(),
                    )
                    .await;
            })
        };
        let frame = owned_with_ready(&transport, &source, None, WINDOW_MS, None, || {
            println!("READY listener_registered");
        })
        .await
        .expect("passive receive window must cover the controller's release");
        assert_eq!(frame.payload_bytes(), b"released");
        sender.await.unwrap();
        // The window opens once per process: a later receive starts at once even
        // after the controller's file is gone.
        std::fs::remove_file(&release_path).unwrap();
        let later = {
            let control = Arc::clone(&control);
            let source = source.clone();
            tokio::spawn(async move {
                let listener = loop {
                    if let Some(listener) = control.listener.lock().unwrap().clone() {
                        break listener;
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                };
                let metadata = UFrameMetadata::publish(source)
                    .with_payload_encoding(PayloadEncoding::RAW)
                    .build()
                    .unwrap();
                listener
                    .on_receive_owned(
                        UOwnedFrame::with_payload(metadata, b"later".to_vec()).unwrap(),
                    )
                    .await;
            })
        };
        let frame = owned_with_ready(&transport, &source, None, WINDOW_MS, None, || {})
            .await
            .expect("a later passive receive must not wait for the release again");
        assert_eq!(frame.payload_bytes(), b"later");
        later.await.unwrap();
    }

    /// A contended controller can need longer than a passive's whole receive
    /// window to start the other peers after the passive reports readiness.
    #[test]
    fn passive_receive_window_opens_at_controller_release() {
        use std::io::{BufRead, BufReader};
        let release =
            std::env::temp_dir().join(format!("receive-window-release-{}", std::process::id()));
        let _ = std::fs::remove_file(&release);
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "common::receive::tests::receive_window_child",
                "--nocapture",
                "--test-threads",
                "1",
            ])
            .env(WINDOW_CHILD, &release)
            .env("UPROTOCOL_RECEIVE_START_FILE", &release)
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let mut output = Vec::new();
        for line in lines.by_ref() {
            let line = line.unwrap();
            let ready = line.ends_with("READY listener_registered");
            output.push(line);
            if ready {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(2 * WINDOW_MS));
        let staged = release.with_extension("staged");
        std::fs::write(&staged, b"START\n").unwrap();
        std::fs::rename(&staged, &release).unwrap();
        output.extend(lines.map(Result::unwrap));
        let status = child.wait().unwrap();
        let _ = std::fs::remove_file(&release);
        assert!(
            status.success(),
            "the passive receive window must open at the controller's release, not at readiness: {status:?} {output:#?}"
        );
    }
}
