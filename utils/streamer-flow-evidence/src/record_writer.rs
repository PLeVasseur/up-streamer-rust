// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0

//! Complete observational records on one explicit process-owned output channel.
//!
//! Structured capture stays separate from foreign/native human diagnostics.
//! Standalone stdout compatibility does not coordinate raw foreign writers.

use serde::Serialize;
use std::{
    fs::{File, OpenOptions},
    io::{self, Write},
    sync::{Mutex, OnceLock},
};

/// Explicit per-role file recorded in the process launch environment.
pub const OUTPUT_PATH_ENV: &str = "UPROTOCOL_STRUCTURED_EVIDENCE_PATH";

struct State<W> {
    sink: W,
    line: Vec<u8>,
}

struct RecordWriter<W> {
    state: Mutex<State<W>>,
}

fn encode_into<T: Serialize + ?Sized>(
    line: &mut Vec<u8>,
    marker: &str,
    value: &T,
) -> io::Result<()> {
    line.clear();
    line.extend_from_slice(marker.as_bytes());
    serde_json::to_writer(&mut *line, value).map_err(io::Error::other)?;
    line.push(b'\n');
    Ok(())
}

impl<W: Write> RecordWriter<W> {
    fn new(sink: W) -> Self {
        Self {
            state: Mutex::new(State {
                sink,
                // Initial reservation, not a maximum record length/cap. Growth
                // remains observable and charged; never truncate to this size.
                line: Vec::with_capacity(1024),
            }),
        }
    }

    fn emit<T: Serialize + ?Sized>(&self, marker: &str, value: &T) -> io::Result<()> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| io::Error::other("structured evidence writer poisoned"))?;
        let State { sink, line } = &mut *state;
        // Encoding finishes in owned memory before ANY file bytes are exposed.
        // The same lock covers every short write, so concurrent producers cannot
        // interleave records. Dedicated file excludes raw/native diagnostic FDs.
        encode_into(line, marker, value)?;
        sink.write_all(line)
    }

    fn emit_preencoded(&self, marker: &str, body: &str) -> io::Result<()> {
        if body.contains('\r') || body.contains('\n') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "preencoded record is not one physical line",
            ));
        }
        let mut state = self
            .state
            .lock()
            .map_err(|_| io::Error::other("structured evidence writer poisoned"))?;
        let State { sink, line } = &mut *state;
        line.clear();
        line.extend_from_slice(marker.as_bytes());
        line.extend_from_slice(body.as_bytes());
        line.push(b'\n');
        sink.write_all(line)
    }
}

enum Output {
    Dedicated(RecordWriter<File>),
    // Standalone compatibility only: stdout has foreign-writer limitations.
    Stdout(Mutex<Vec<u8>>),
}

static OUTPUT: OnceLock<Result<Output, String>> = OnceLock::new();

fn dedicated_mode(output: &OnceLock<Result<Output, String>>) -> bool {
    matches!(output.get(), Some(Ok(Output::Dedicated(_))))
}

/// Query setup's cached mode without environment access, allocation or initialization.
/// A declared capture must be prepared before traffic, rather than initialized here.
pub fn dedicated_output_is_prepared() -> bool {
    dedicated_mode(&OUTPUT)
}

fn output() -> io::Result<&'static Output> {
    match OUTPUT.get_or_init(|| {
        let Some(path) = std::env::var_os(OUTPUT_PATH_ENV) else {
            return Ok(Output::Stdout(Mutex::new(Vec::with_capacity(1024))));
        };
        // Parent owns and precreates this regular per-role file. A declared
        // dedicated destination must never silently fall back to mixed stdout.
        OpenOptions::new()
            .append(true)
            .open(path)
            .map(|file| Output::Dedicated(RecordWriter::new(file)))
            .map_err(|error| error.to_string())
    }) {
        Ok(output) => Ok(output),
        Err(error) => Err(io::Error::other(error.clone())),
    }
}

/// Initialize from the fixed child environment during setup, before READY/traffic.
/// Disabled observation paths must not call this or initialize an output sink.
pub fn prepare_output_from_env() -> io::Result<()> {
    output().map(|_| ())
}

/// Write existing marker+JSON+newline using process-owned reusable storage.
/// No worker, retry, size cap, parser change or per-record destination open.
pub fn emit_json_record<T: Serialize + ?Sized>(marker: &str, value: &T) -> io::Result<()> {
    match output()? {
        Output::Dedicated(writer) => writer.emit(marker, value),
        Output::Stdout(buffer) => {
            let mut line = buffer
                .lock()
                .map_err(|_| io::Error::other("structured stdout writer poisoned"))?;
            encode_into(&mut line, marker, value)?;
            io::stdout().lock().write_all(&line)
        }
    }
}

/// Preserve a trusted existing compact JSON/JSON5 body byte-for-byte.
/// This is used only where the original emitter already encoded that body;
/// receivers retain their existing grammar/schema checks. No string wrapping.
pub fn emit_preencoded_record(marker: &str, body: &str) -> io::Result<()> {
    if body.contains('\r') || body.contains('\n') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "preencoded record is not one physical line",
        ));
    }
    match output()? {
        Output::Dedicated(writer) => writer.emit_preencoded(marker, body),
        Output::Stdout(buffer) => {
            let mut line = buffer
                .lock()
                .map_err(|_| io::Error::other("structured stdout writer poisoned"))?;
            line.clear();
            line.extend_from_slice(marker.as_bytes());
            line.extend_from_slice(body.as_bytes());
            line.push(b'\n');
            io::stdout().lock().write_all(&line)
        }
    }
}

/// Fail evidence rather than double-encoding or silently losing an existing body.
pub fn emit_preencoded_or_fail(marker: &str, body: &str) {
    if let Err(error) = emit_preencoded_record(marker, body) {
        panic!("FLOW_EVIDENCE_ERROR {error}");
    }
}

/// Keep existing infallible emit call sites fail-closed on serialization/output
/// error. The error marker also remains detectable if a callback catches panic.
pub fn emit_json_or_fail<T: Serialize + ?Sized>(marker: &str, value: &T) {
    if let Err(error) = emit_json_record(marker, value) {
        panic!("FLOW_EVIDENCE_ERROR {error}");
    }
}

#[derive(Serialize)]
struct ForwardCompleted<'a> {
    schema: u8,
    run: &'a str,
    egress: &'a str,
    id: String,
    success: bool,
    error: Option<String>,
}

/// Emit the unchanged completion schema without an allocating serde Value/map.
/// The UUID formatting and an actual error's Debug string remain charged costs.
pub fn emit_native_forward_completed(
    run: &str,
    egress: &str,
    id: &up_rust::UUID,
    success: bool,
    error: Option<String>,
) {
    emit_json_or_fail(
        "FLOW_NATIVE_FORWARD_COMPLETED ",
        &ForwardCompleted {
            schema: 1,
            run,
            egress,
            id: id.to_hyphenated_string(),
            success,
            error,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[derive(Clone)]
    struct ShortSink(Arc<Mutex<Vec<u8>>>);
    impl Write for ShortSink {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            let n = bytes.len().min(7);
            self.0.lock().unwrap().extend_from_slice(&bytes[..n]);
            Ok(n)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[derive(Serialize)]
    struct Record {
        producer: usize,
        sequence: usize,
    }

    #[test]
    fn dedicated_records_stay_complete_across_concurrent_producers_and_short_writes() {
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let writer = Arc::new(RecordWriter::new(ShortSink(bytes.clone())));
        let mut workers = Vec::new();
        for producer in 0..8 {
            let writer = writer.clone();
            workers.push(std::thread::spawn(move || {
                for sequence in 0..64 {
                    writer
                        .emit("FLOW_MESSAGE_EVIDENCE ", &Record { producer, sequence })
                        .unwrap();
                }
            }));
        }
        for worker in workers {
            worker.join().unwrap();
        }
        let bytes = bytes.lock().unwrap();
        let text = std::str::from_utf8(&bytes).unwrap();
        let mut next = [0; 8];
        let mut count = 0;
        for line in text.lines() {
            let json = line.strip_prefix("FLOW_MESSAGE_EVIDENCE ").unwrap();
            let value: serde_json::Value = serde_json::from_str(json).unwrap();
            let p = value["producer"].as_u64().unwrap() as usize;
            let s = value["sequence"].as_u64().unwrap() as usize;
            assert_eq!(s, next[p]); // Preserve each producer's actual call order.
            next[p] += 1;
            count += 1;
        }
        assert_eq!(count, 512);
        assert_eq!(next, [64; 8]);
    }

    struct BrokenSerialize;
    impl Serialize for BrokenSerialize {
        fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom("deliberate encode failure"))
        }
    }

    #[test]
    fn failed_encoding_never_commits_a_partial_record() {
        // The same mode query used by projection must not initialize the sink
        // or inspect process-global environment on an observed message path.
        let cached = OnceLock::new();
        assert!(!dedicated_mode(&cached));
        assert!(cached.get().is_none());
        assert!(cached
            .set(Ok(Output::Stdout(Mutex::new(Vec::new()))))
            .is_ok());
        assert!(!dedicated_mode(&cached));
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let writer = RecordWriter::new(ShortSink(bytes.clone()));
        assert!(writer
            .emit("FLOW_MESSAGE_EVIDENCE ", &BrokenSerialize)
            .is_err());
        assert!(bytes.lock().unwrap().is_empty());
        writer
            .emit(
                "FLOW_MESSAGE_EVIDENCE ",
                &Record {
                    producer: 0,
                    sequence: 0,
                },
            )
            .unwrap();
        assert_eq!(
            std::str::from_utf8(&bytes.lock().unwrap())
                .unwrap()
                .lines()
                .count(),
            1
        );
    }
}
