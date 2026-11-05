// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Optional test observations at actual route handoff boundaries.

#[cfg(feature = "flow-evidence")]
pub(crate) type Record = Option<streamer_flow_evidence::Observation>;
#[cfg(not(feature = "flow-evidence"))]
pub(crate) struct Record;

pub(crate) fn message(stage: &str, route: &str, value: &up_rust::UMessage) -> Record {
    #[cfg(feature = "flow-evidence")]
    {
        checked(streamer_flow_evidence::Observation::message(
            stage,
            Some(route),
            value,
        ))
    }
    #[cfg(not(feature = "flow-evidence"))]
    {
        let _ = (stage, route, value);
        Record
    }
}

#[cfg(feature = "owned-frame-transport")]
pub(crate) fn frame(
    stage: &str,
    route: &str,
    value: &(impl up_rust::UFrameView + ?Sized),
) -> Record {
    #[cfg(feature = "flow-evidence")]
    {
        checked(streamer_flow_evidence::Observation::frame(
            stage,
            Some(route),
            value,
        ))
    }
    #[cfg(not(feature = "flow-evidence"))]
    {
        let _ = (stage, route, value);
        Record
    }
}

pub(crate) fn emit(record: Record) {
    #[cfg(feature = "flow-evidence")]
    streamer_flow_evidence::Observation::emit(record);
    #[cfg(not(feature = "flow-evidence"))]
    let _ = record;
}

#[cfg(feature = "flow-evidence")]
fn checked(result: Result<Record, up_rust::UStatus>) -> Record {
    result.unwrap_or_else(|error| {
        eprintln!("FLOW_EVIDENCE_ERROR {error}");
        None
    })
}

#[cfg(all(
    feature = "experimental-copy-minimized-routing",
    feature = "flow-evidence"
))]
pub(crate) fn prepared<F: up_rust::Frame>(
    stage: &str,
    route: &str,
    value: &up_rust::HeaderLease<F>,
) -> Record {
    match value.header() {
        Ok(header) => checked(streamer_flow_evidence::Observation::header(
            stage,
            Some(route),
            &header,
            value.payload().map(|payload| payload.bytes()),
        )),
        Err(error) => {
            eprintln!("FLOW_EVIDENCE_ERROR {error}");
            None
        }
    }
}

#[cfg(feature = "experimental-copy-minimized-routing")]
pub(crate) fn prepared_copy_observer(
    name: &str,
    authority: &str,
) -> Option<std::sync::Arc<dyn crate::PreparedForwardObserver>> {
    #[cfg(feature = "flow-evidence")]
    {
        std::env::var("UPROTOCOL_FLOW_EVIDENCE_RUN")
            .ok()
            .map(|run| {
                if std::env::var_os(streamer_flow_evidence::OUTPUT_PATH_ENV).is_some() {
                    streamer_flow_evidence::prepare_output_from_env()
                        .unwrap_or_else(|error| panic!("FLOW_EVIDENCE_ERROR {error}"));
                }
                std::sync::Arc::new(NativeCopyObserver {
                    run,
                    egress: format!("native:{name}@{authority}"),
                }) as std::sync::Arc<dyn crate::PreparedForwardObserver>
            })
    }
    #[cfg(not(feature = "flow-evidence"))]
    {
        let _ = (name, authority);
        None
    }
}
#[cfg(all(
    feature = "experimental-copy-minimized-routing",
    feature = "flow-evidence"
))]
struct NativeCopyObserver {
    run: String,
    egress: String,
}
#[cfg(all(
    feature = "experimental-copy-minimized-routing",
    feature = "flow-evidence"
))]
impl crate::PreparedForwardObserver for NativeCopyObserver {
    fn ingress(&self, _: &up_rust::RxHeader<'_>, _: Option<&[u8]>) {
        // Actual route ingress is already observed by its forwarding handler.
    }
    fn copied_metadata(&self, _: &up_rust::UUID, _: &[u8]) {
        // This observer claims payload-copy evidence only. The existing custom
        // observer API separately exposes actual initialized metadata bytes.
    }
    fn copied_payload(&self, id: &up_rust::UUID, payload: Option<&[u8]>) {
        streamer_flow_evidence::PayloadCopyObservation::capture(
            &self.run,
            &self.egress,
            id,
            payload,
        )
        .emit();
    }
    fn completed(&self, id: &up_rust::UUID, result: Result<(), up_rust::SendError>) {
        let success = result.is_ok();
        let error = result.err().map(|error| format!("{error:?}"));
        streamer_flow_evidence::emit_native_forward_completed(
            &self.run,
            &self.egress,
            id,
            success,
            error,
        );
    }
}

// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0

// DRAFT ONLY. Insert this private module in up-streamer/src/flow_observation.rs
// in the fresh admitted owner, BEFORE fixing the actual emitters for causal RED.
// It compiles against the old public utility APIs and actual private observer.
// No new writer API is called, so the same test source can run RED then GREEN.
// No native backend/network is started; the child is this already-built test exe.

#[cfg(all(test, target_os = "linux", feature = "flow-evidence", feature = "experimental-copy-minimized-routing"))]
#[rustfmt::skip]
mod record_writer_regression_tests {
    use super::*;
    use std::{
        fs::{self, File},
        io::{self, Write},
        os::fd::AsFd,
        process::{Command, Stdio},
        sync::mpsc,
        thread,
        time::{Duration, Instant, SystemTime, UNIX_EPOCH},
    };
    use streamer_flow_evidence::{Observation, PayloadCopyObservation};
    use up_rust::{PayloadEncoding, UMessageBuilder, UUri, UUID};

    const CHILD_ENV: &str = "UPROTOCOL_RECORD_WRITER_REGRESSION_CHILD";
    // Literal intentionally works before the new writer constant/API exists.
    const OUTPUT_ENV: &str = "UPROTOCOL_STRUCTURED_EVIDENCE_PATH";
    const RUN: &str = "actual-emitter-concurrent-records";

    #[test]
    fn actual_emitters_child() {
        if std::env::var_os(CHILD_ENV).is_none() { return }
        let message = UMessageBuilder::publish(
            UUri::try_from_parts("record-test", 0x0001_1234, 1, 0x8001).unwrap(),
        ).build_with_payload(&b"abc"[..], PayloadEncoding::RAW).unwrap();
        let record = Observation::message("ingress", Some("source -> sink"), &message)
            .unwrap().unwrap();
        let id: UUID = record.id.parse().unwrap();
        let observer = NativeCopyObserver { run: RUN.into(), egress: "native:test@record-test".into() };
        let mut message_record = Some(record);
        io::stdout().flush().unwrap();
        io::stderr().flush().unwrap();
        for which in 0..3 {
            // Duplicate the inherited stderr descriptor safely. File writes
            // bypass Rust stdout/stderr formatting locks, just like foreign
            // C++/raw-FD console output. The descriptor shares the capture OFD.
            let descriptor = if which % 2 == 0 {
                io::stdout().as_fd().try_clone_to_owned().unwrap()
            } else {
                io::stderr().as_fd().try_clone_to_owned().unwrap()
            };
            let mut console = File::from(descriptor);
            let (ready_tx, ready_rx) = mpsc::channel();
            let (resume_tx, resume_rx) = mpsc::channel();
            let foreign = thread::spawn(move || {
                write!(console, "FOREIGN_PARTIAL_CONSOLE_{which} ").unwrap();
                console.flush().unwrap();
                ready_tx.send(()).unwrap();
                resume_rx.recv().unwrap();
                writeln!(console, " FOREIGN_CONSOLE_FINISHED_{which}").unwrap();
            });
            ready_rx.recv().unwrap();
            match which {
                0 => Observation::emit(message_record.take()),
                1 => PayloadCopyObservation::capture(RUN, "native:test@record-test", &id, Some(b"abc")).emit(),
                2 => crate::PreparedForwardObserver::completed(&observer, &id, Ok(())),
                _ => unreachable!(),
            }
            resume_tx.send(()).unwrap();
            foreign.join().unwrap();
        }
        println!("ACTUAL_EMITTER_CHILD_RETURNED");
    }

    #[test]
    fn actual_structured_emitters_survive_foreign_partial_console() {
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let scratch = std::env::temp_dir().join(format!("actual-flow-record-{}-{stamp}", std::process::id()));
        fs::create_dir(&scratch).unwrap();
        let raw_path = scratch.join("console.log");
        let structured_path = scratch.join("streamer.records.jsonl");
        // Empty but declared file on RED: old actual emitters ignore OUTPUT_ENV.
        File::create(&structured_path).unwrap();
        let stdout = File::create(&raw_path).unwrap();
        let stderr = stdout.try_clone().unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "flow_observation::record_writer_regression_tests::actual_emitters_child", "--nocapture"])
            .env(CHILD_ENV, "1")
            .env(OUTPUT_ENV, &structured_path)
            .env("UPROTOCOL_FLOW_EVIDENCE_RUN", RUN)
            .env("UPROTOCOL_FLOW_EVIDENCE_ACTOR", "streamer")
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .spawn().unwrap();
        // Reuse the established native child regression policy, not a new row
        // retry or enlarged runtime operation/validation deadline.
        let deadline = Instant::now() + Duration::from_secs(10);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() { break status }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let status = child.wait().unwrap();
                panic!("owned record child timed out, reaped {status}; evidence {}", scratch.display());
            }
            thread::sleep(Duration::from_millis(10));
        };
        let raw = fs::read_to_string(&raw_path).unwrap();
        assert!(status.success(), "{status}; evidence {}\n{raw}", scratch.display());
        assert!(raw.contains("ACTUAL_EMITTER_CHILD_RETURNED"), "{raw}");
        let text = fs::read_to_string(&structured_path).unwrap();
        let lines: Vec<_> = text.lines().collect();
        for marker in ["FLOW_MESSAGE_EVIDENCE ", "FLOW_PAYLOAD_COPY ", "FLOW_NATIVE_FORWARD_COMPLETED "] {
            eprintln!("ACTUAL_EMITTER_CAPTURE_COUNTS marker={marker:?} raw_occurrences={} raw_column_zero={} structured_column_zero={}",
                raw.matches(marker).count(), raw.lines().filter(|line| line.starts_with(marker)).count(),
                text.lines().filter(|line| line.starts_with(marker)).count());
        }
        // This fails deterministically on the OLD actual three emitters. Raw
        // console prefixes embed their output, while the declared file is empty.
        // Keep the RED raw log; do not resynchronize or reconstruct its records.
        assert_eq!(lines.len(), 3, "strict structured record count; evidence {}\nraw={raw}\nstructured={text}", scratch.display());
        let message: Observation = serde_json::from_str(
            lines[0].strip_prefix("FLOW_MESSAGE_EVIDENCE ").expect("column-zero message marker"),
        ).unwrap();
        assert_eq!((&*message.run, &*message.actor, &*message.stage), (RUN, "streamer", "ingress"));
        assert_eq!(message.route.as_deref(), Some("source -> sink"));
        assert_eq!(message.sha256, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        let copied: serde_json::Value = serde_json::from_str(
            lines[1].strip_prefix("FLOW_PAYLOAD_COPY ").expect("column-zero copy marker"),
        ).unwrap();
        assert_eq!(copied["id"], message.id);
        assert_eq!(copied["copied_payload_bytes"], 3);
        assert_eq!(copied["sha256"], message.sha256);
        let completed: serde_json::Value = serde_json::from_str(
            lines[2].strip_prefix("FLOW_NATIVE_FORWARD_COMPLETED ").expect("column-zero completion marker"),
        ).unwrap();
        assert_eq!(completed["id"], message.id);
        assert_eq!(completed["success"], true);
        assert!(completed["error"].is_null());
        for which in 0..3 {
            assert!(raw.contains(&format!("FOREIGN_PARTIAL_CONSOLE_{which}  FOREIGN_CONSOLE_FINISHED_{which}")), "{raw}");
        }
        assert!(!raw.contains("FLOW_MESSAGE_EVIDENCE "));
        assert!(!raw.contains("FLOW_PAYLOAD_COPY "));
        assert!(!raw.contains("FLOW_NATIVE_FORWARD_COMPLETED "));
        assert!(!raw.contains("FLOW_EVIDENCE_ERROR"));
        println!("ACTUAL_EMITTER_FOREIGN_CONSOLE_REGRESSION_PASS evidence={}", scratch.display());
        // Keep owned raw/structured evidence for the command recorder. Any later
        // cleanup must be exact and occur after it has captured these files.
    }
}
