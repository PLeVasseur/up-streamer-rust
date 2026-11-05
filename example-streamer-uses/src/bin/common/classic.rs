// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Classic message flow over endpoints constructed by each role entrypoint.

use async_trait::async_trait;
use chrono::{Local, Timelike};
use std::{sync::Arc, time::Duration};
use tokio::{
    signal::unix::{signal, SignalKind},
    sync::RwLock,
};
use up_rust::{UCode, UListener, UMessage, UStatus, UTransport, UUri};

/// Produce one message per iteration after the configured delay. None is an
/// unbounded run; Some(0) sends nothing. Role frontends select their count policy.
pub async fn send(
    transport: &dyn UTransport,
    count: Option<u64>,
    interval: Duration,
    mut message: impl FnMut(u64) -> Result<UMessage, UStatus>,
) -> Result<u64, UStatus> {
    super::startup::wait_for_start().await?;
    let mut sent = 0_u64;
    while count.is_none_or(|limit| sent < limit) {
        tokio::time::sleep(interval).await;
        super::proof::send(transport, message(sent)?).await?;
        sent = sent.checked_add(1).ok_or_else(|| {
            UStatus::fail_with_code(UCode::OutOfRange, "classic send counter exhausted")
        })?;
    }
    Ok(sent)
}

/// Admit the listener before announcing readiness and serve until the
/// supervisor's SIGINT. The signal closes admission and waits for deliveries
/// already admitted, such as a reply whose send is still in flight, then ends
/// the process by SIGINT with its default disposition; under the default
/// dispositions supervised roles start with, the exit status is unchanged and
/// no destructors run. A delivery admitted before the signal thus completes
/// its operation and evidence unless the operation itself fails; deliveries
/// after the signal are never observed. A second SIGINT abandons a stalled
/// drain, and SIGTERM keeps its default disposition.
pub async fn listen(
    transport: &dyn UTransport,
    source: &UUri,
    sink: Option<&UUri>,
    listener: Arc<dyn UListener>,
) -> Result<(), UStatus> {
    let mut interrupt = signal(SignalKind::interrupt()).map_err(|error| {
        UStatus::fail_with_code(UCode::Internal, format!("SIGINT handler: {error}"))
    })?;
    let admission = Arc::new(Admission::new(listener));
    let serve = async {
        transport
            .register_listener(source, sink, admission.clone())
            .await?;
        println!("READY listener_registered");
        std::future::pending::<Result<(), UStatus>>().await
    };
    // Only a failed registration completes `serve`; a signal during
    // registration drains like one after readiness.
    tokio::select! {
        failed = serve => return failed,
        _ = interrupt.recv() => {}
    }
    tokio::select! {
        () = admission.close() => {}
        _ = interrupt.recv() => {}
    }
    end_by_interrupt();
    // The default action completes asynchronously after kill() returns.
    std::future::pending().await
}

/// Forwards deliveries to its listener until closed. Closing waits for the
/// deliveries it already admitted; later deliveries return unobserved.
struct Admission {
    listener: Arc<dyn UListener>,
    open: RwLock<bool>,
}

impl Admission {
    fn new(listener: Arc<dyn UListener>) -> Self {
        Self {
            listener,
            open: RwLock::new(true),
        }
    }

    async fn close(&self) {
        // The lock is fair: deliveries arriving after this request queue behind it.
        *self.open.write().await = false;
    }
}

#[async_trait]
impl UListener for Admission {
    async fn on_receive(&self, msg: UMessage) {
        let open = self.open.read().await;
        if *open {
            self.listener.on_receive(msg).await;
        }
    }
}

/// Ends the process as the unhandled SIGINT it received would have.
fn end_by_interrupt() {
    // SAFETY: plain libc calls from ordinary code, not from a signal handler:
    // restore the default disposition, then re-send the signal to this process.
    unsafe {
        libc::signal(libc::SIGINT, libc::SIG_DFL);
        libc::kill(libc::getpid(), libc::SIGINT);
    }
}

/// Existing classic timer example payload, sampled when its send iteration runs.
pub fn timer_payload() -> Vec<u8> {
    use hello_world_protos::{hello_world_topics::Timer, timeofday::TimeOfDay};
    let now = Local::now();
    super::protobuf_payload(&Timer {
        time: Some(TimeOfDay {
            hours: now.hour() as i32,
            minutes: now.minute() as i32,
            seconds: now.second() as i32,
            nanos: now.nanosecond() as i32,
            ..Default::default()
        })
        .into(),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Notify;
    use up_rust::{PayloadEncoding, UMessageBuilder};

    fn request() -> UMessage {
        UMessageBuilder::request(
            UUri::try_from("//authority-b/5BA0/1/1/421").unwrap(),
            UUri::try_from("//authority-a/5BA0/1/1/0").unwrap(),
            10_000,
        )
        .build_with_payload(b"interrupted".to_vec(), PayloadEncoding::RAW)
        .unwrap()
    }

    #[derive(Default)]
    struct Held {
        calls: AtomicUsize,
        entered: Notify,
        release: Notify,
    }

    #[async_trait]
    impl UListener for Held {
        async fn on_receive(&self, _: UMessage) {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.entered.notify_one();
            self.release.notified().await;
        }
    }

    #[tokio::test]
    async fn closing_admission_waits_for_admitted_deliveries_and_drops_later_ones() {
        let held = Arc::new(Held::default());
        let admission = Arc::new(Admission::new(held.clone()));
        let first = tokio::spawn({
            let admission = Arc::clone(&admission);
            async move { admission.on_receive(request()).await }
        });
        held.entered.notified().await;
        let closing = tokio::spawn({
            let admission = Arc::clone(&admission);
            async move { admission.close().await }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            !closing.is_finished(),
            "close must wait for an admitted delivery"
        );
        let late = tokio::spawn({
            let admission = Arc::clone(&admission);
            async move { admission.on_receive(request()).await }
        });
        held.release.notify_one();
        first.await.unwrap();
        closing.await.unwrap();
        late.await.unwrap();
        assert_eq!(
            held.calls.load(Ordering::SeqCst),
            1,
            "a delivery after close is never observed"
        );
    }
}

#[cfg(all(test, feature = "flow-evidence"))]
mod interrupt_tests {
    use super::*;
    use crate::common::ServiceRequestResponder;
    use async_trait::async_trait;
    use std::{
        io::{BufRead, BufReader},
        os::unix::process::ExitStatusExt,
        path::PathBuf,
        sync::{
            atomic::{AtomicBool, Ordering},
            Mutex,
        },
    };
    use tokio::sync::Notify;
    use up_rust::{PayloadEncoding, UMessageBuilder};

    const CHILD: &str = "EXAMPLE_STREAMER_USES_CLASSIC_INTERRUPT_CHILD";
    const IN_FLIGHT: &str = "CLASSIC_INTERRUPT_TEST reply_send_in_flight";

    /// Classic transport double: retains the registered listener, and each send
    /// announces itself and completes only once the parent test releases it.
    struct HeldSend {
        release: PathBuf,
        listener: Mutex<Option<Arc<dyn UListener>>>,
        registered: Notify,
    }

    #[async_trait]
    impl UTransport for HeldSend {
        async fn send(&self, _: UMessage) -> Result<(), UStatus> {
            println!("{IN_FLIGHT}");
            for _ in 0..3000 {
                if self.release.exists() {
                    return Ok(());
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Err(UStatus::fail_with_code(
                UCode::DeadlineExceeded,
                "send never released",
            ))
        }

        async fn register_listener(
            &self,
            _: &UUri,
            _: Option<&UUri>,
            listener: Arc<dyn UListener>,
        ) -> Result<(), UStatus> {
            *self.listener.lock().unwrap() = Some(listener);
            self.registered.notify_one();
            Ok(())
        }
    }

    /// Child half of the interrupt regression; a no-op unless launched by it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn interrupted_listener_child() {
        let Some(release) = std::env::var_os(CHILD) else {
            return;
        };
        // Start a fresh line after libtest's unterminated "test ... " prefix.
        println!();
        let transport = Arc::new(HeldSend {
            release: PathBuf::from(release),
            listener: Mutex::new(None),
            registered: Notify::new(),
        });
        let responder: Arc<dyn UListener> =
            Arc::new(ServiceRequestResponder::new(transport.clone()));
        let serving = tokio::spawn({
            let transport = Arc::clone(&transport);
            async move {
                let sink = UUri::try_from("//authority-b/5BA0/1/1/421").unwrap();
                listen(transport.as_ref(), &UUri::any(), Some(&sink), responder).await
            }
        });
        transport.registered.notified().await;
        let listener = transport.listener.lock().unwrap().clone().unwrap();
        let request = UMessageBuilder::request(
            UUri::try_from("//authority-b/5BA0/1/1/421").unwrap(),
            UUri::try_from("//authority-a/5BA0/1/1/0").unwrap(),
            10_000,
        )
        .build_with_payload(b"interrupted".to_vec(), PayloadEncoding::RAW)
        .unwrap();
        tokio::spawn(async move { listener.on_receive(request).await });
        let result = serving.await;
        panic!("the supervisor's SIGINT must end a classic listener; it returned {result:?}");
    }

    /// The supervisor's first teardown signal can arrive while a classic reply,
    /// for example to a repeated connectivity request, is still being sent.
    #[test]
    fn interrupted_listener_completes_admitted_operation() {
        let release =
            std::env::temp_dir().join(format!("classic-interrupt-release-{}", std::process::id()));
        let _ = std::fs::remove_file(&release);
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "common::classic::interrupt_tests::interrupted_listener_child",
                "--nocapture",
                "--test-threads",
                "1",
            ])
            .env(CHILD, &release)
            .env("UPROTOCOL_CLOUDEVENTS_PROOF", "protobuf-roundtrip-v1")
            .env("UPROTOCOL_FLOW_EVIDENCE_RUN", "classic-interrupt")
            .env("UPROTOCOL_FLOW_EVIDENCE_ACTOR", "passive")
            .env_remove(streamer_flow_evidence::OUTPUT_PATH_ENV)
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let pid = child.id().to_string();
        let finished = Arc::new(AtomicBool::new(false));
        let watchdog = std::thread::spawn({
            let finished = Arc::clone(&finished);
            let pid = pid.clone();
            move || {
                for _ in 0..600 {
                    if finished.load(Ordering::SeqCst) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                let _ = std::process::Command::new("kill")
                    .args(["-KILL", &pid])
                    .status();
            }
        });
        let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let mut output = Vec::new();
        for line in lines.by_ref() {
            let line = line.unwrap();
            let in_flight = line.ends_with(IN_FLIGHT);
            output.push(line);
            if in_flight {
                break;
            }
        }
        let interrupted = std::process::Command::new("kill")
            .args(["-INT", &pid])
            .status();
        // The admitted send is still held: a draining listener must outlive the signal.
        std::thread::sleep(Duration::from_millis(300));
        let draining = child.try_wait().unwrap().is_none();
        std::fs::write(&release, b"").unwrap();
        output.extend(lines.map(Result::unwrap));
        let status = child.wait().unwrap();
        finished.store(true, Ordering::SeqCst);
        watchdog.join().unwrap();
        let _ = std::fs::remove_file(&release);
        assert!(interrupted.unwrap().success(), "{output:#?}");
        assert_eq!(
            status.signal(),
            Some(2),
            "the listener still ends by SIGINT: {status:?} {output:#?}"
        );
        let records = |prefix: &str| -> Vec<streamer_flow_evidence::Observation> {
            output
                .iter()
                .filter_map(|line| line.strip_prefix(prefix))
                .map(|json| {
                    let value: serde_json::Value = serde_json::from_str(json).unwrap();
                    let value = if prefix == streamer_flow_evidence::MARKER {
                        value
                    } else {
                        value["before"].clone()
                    };
                    serde_json::from_value(value).unwrap()
                })
                .collect()
        };
        let operations = records(streamer_flow_evidence::MARKER);
        let stages: Vec<&str> = operations
            .iter()
            .map(|record| record.stage.as_str())
            .collect();
        assert_eq!(
            stages,
            ["rx", "tx"],
            "an operation admitted before SIGINT must complete before the listener ends: {output:#?}"
        );
        let canonical = |values: Vec<streamer_flow_evidence::Observation>| {
            let mut values: Vec<String> = values
                .iter()
                .map(|value| serde_json::to_string(value).unwrap())
                .collect();
            values.sort();
            values
        };
        assert_eq!(
            canonical(records("CLOUDEVENT_PROOF ")),
            canonical(operations),
            "every CloudEvents conversion needs its operation record: {output:#?}"
        );
        assert!(
            draining,
            "SIGINT must wait for the admitted send: {output:#?}"
        );
    }
}
