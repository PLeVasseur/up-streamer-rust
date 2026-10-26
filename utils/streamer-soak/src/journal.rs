// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Full bounded journal chunks, generation binding and independent replay.
use crate::artifact::{self, bounded_line, hash_file, read_json, write_json};
use anyhow::{ensure, Result};
use flate2::{read::MultiGzDecoder, write::GzEncoder, Compression};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs::{File, OpenOptions};
use std::io::{BufReader, Write};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Context {
    pub run: String,
    pub source_identity: String,
    pub plan_sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Record {
    pub ordinal: u64,
    pub context: Context,
    pub actor: String,
    pub kind: String,
    pub value: Value,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Chunk {
    pub path: String,
    pub sha256: String,
    pub previous_sha256: Option<String>,
    pub first: u64,
    pub last: u64,
    pub uncompressed_bytes: usize,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Manifest {
    pub schema: String,
    pub version: u32,
    pub context: Context,
    pub complete: bool,
    pub chunk_bytes: usize,
    pub records: u64,
    pub chunks: Vec<Chunk>,
}

pub(crate) struct Journal {
    directory: PathBuf,
    manifest: Manifest,
    current: Option<GzEncoder<File>>,
    bytes: usize,
    first: u64,
}

impl Journal {
    pub fn new(directory: &Path, context: Context, chunk_bytes: usize) -> Result<Self> {
        ensure!(
            chunk_bytes >= artifact::MAX_LINE,
            "chunk smaller than bounded record"
        );
        Ok(Self {
            directory: directory.into(),
            manifest: Manifest {
                schema: "uprotocol.soak-journal".into(),
                version: 1,
                context,
                complete: false,
                chunk_bytes,
                records: 0,
                chunks: Vec::new(),
            },
            current: None,
            bytes: 0,
            first: 1,
        })
    }

    pub fn append(&mut self, actor: String, kind: String, value: Value) -> Result<Record> {
        let ordinal = self
            .manifest
            .records
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("journal ordinal exhausted"))?;
        let record = Record {
            ordinal,
            context: self.manifest.context.clone(),
            actor,
            kind,
            value,
        };
        let mut bytes = serde_json::to_vec(&record)?;
        bytes.push(b'\n');
        ensure!(
            bytes.len() <= artifact::MAX_LINE,
            "oversized journal record"
        );
        if self.current.is_some() && self.bytes + bytes.len() > self.manifest.chunk_bytes {
            self.finish_chunk()?;
        }
        if self.current.is_none() {
            let path = self.directory.join(format!(
                "journal-{:06}.jsonl.gz",
                self.manifest.chunks.len() + 1
            ));
            let file = OpenOptions::new().write(true).create_new(true).open(path)?;
            self.current = Some(GzEncoder::new(file, Compression::fast()));
            self.bytes = 0;
            self.first = ordinal;
        }
        self.current
            .as_mut()
            .expect("open chunk")
            .write_all(&bytes)?;
        self.bytes += bytes.len();
        self.manifest.records = ordinal;
        Ok(record)
    }

    fn finish_chunk(&mut self) -> Result<()> {
        if let Some(encoder) = self.current.take() {
            encoder.finish()?.sync_all()?;
            let name = format!("journal-{:06}.jsonl.gz", self.manifest.chunks.len() + 1);
            self.manifest.chunks.push(Chunk {
                sha256: hash_file(&self.directory.join(&name))?,
                path: name,
                previous_sha256: self
                    .manifest
                    .chunks
                    .last()
                    .map(|value| value.sha256.clone()),
                first: self.first,
                last: self.manifest.records,
                uncompressed_bytes: self.bytes,
            });
        }
        Ok(())
    }

    pub fn close(mut self, complete: bool) -> Result<Manifest> {
        self.finish_chunk()?;
        self.manifest.complete = complete;
        write_json(
            &self.directory.join("journal-manifest.json"),
            &self.manifest,
        )?;
        Ok(self.manifest)
    }
}

pub(crate) fn replay(
    directory: &Path,
    expected: &Context,
    mut consume: impl FnMut(Record) -> Result<()>,
) -> Result<u64> {
    let manifest: Manifest = read_json(&directory.join("journal-manifest.json"))?;
    ensure!(
        manifest.schema == "uprotocol.soak-journal" && manifest.version == 1 && manifest.complete,
        "incomplete or unknown journal"
    );
    ensure!(
        &manifest.context == expected,
        "journal generation/plan mismatch"
    );
    ensure!(
        (artifact::MAX_LINE..=4 * 1024 * 1024).contains(&manifest.chunk_bytes)
            && manifest.records > 0
            && !manifest.chunks.is_empty(),
        "invalid journal bounds"
    );
    let mut ordinal = 0u64;
    let mut previous = None;
    let mut line = Vec::new();
    for (index, chunk) in manifest.chunks.iter().enumerate() {
        ensure!(
            chunk.path == format!("journal-{:06}.jsonl.gz", index + 1),
            "unexpected journal locator"
        );
        ensure!(
            chunk.previous_sha256 == previous && chunk.first == ordinal + 1,
            "broken chunk chain"
        );
        ensure!(
            chunk.first <= chunk.last && chunk.uncompressed_bytes <= manifest.chunk_bytes,
            "invalid chunk extent"
        );
        let path = directory.join(&chunk.path);
        ensure!(
            std::fs::metadata(&path)?.len() <= (manifest.chunk_bytes + artifact::MAX_LINE) as u64,
            "compressed chunk exceeds input bound"
        );
        ensure!(
            hash_file(&path)? == chunk.sha256,
            "journal content hash changed"
        );
        let mut reader = BufReader::new(MultiGzDecoder::new(File::open(path)?));
        let mut bytes = 0usize;
        while bounded_line(&mut reader, &mut line)? {
            bytes = bytes
                .checked_add(line.len())
                .ok_or_else(|| anyhow::anyhow!("journal extent overflow"))?;
            ensure!(
                bytes <= manifest.chunk_bytes,
                "chunk exceeds declared bound"
            );
            let record: Record = serde_json::from_slice(&line)?;
            ordinal = ordinal
                .checked_add(1)
                .ok_or_else(|| anyhow::anyhow!("ordinal overflow"))?;
            ensure!(
                record.ordinal == ordinal && &record.context == expected,
                "record order or generation changed"
            );
            consume(record)?;
        }
        ensure!(
            bytes == chunk.uncompressed_bytes && ordinal == chunk.last,
            "incomplete chunk"
        );
        previous = Some(chunk.sha256.clone());
    }
    ensure!(ordinal == manifest.records, "record count mismatch");
    Ok(ordinal)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use test_case::test_case;

    fn fixture() -> (tempfile::TempDir, Context) {
        let temp = tempfile::tempdir().unwrap();
        let context = Context {
            run: "fixture".into(),
            source_identity: "a".repeat(64),
            plan_sha256: "b".repeat(64),
        };
        let mut journal = Journal::new(temp.path(), context.clone(), 65536).unwrap();
        for n in 0..150 {
            journal
                .append(
                    "source".into(),
                    "fixture".into(),
                    json!({"n": n, "body": "x".repeat(1024)}),
                )
                .unwrap();
        }
        journal.close(true).unwrap();
        (temp, context)
    }

    #[test_case("previous_sha256", json!("0".repeat(64)); "broken_hash_chain")]
    #[test_case("first", json!(1); "overlapping_ordinals")]
    #[test_case("path", json!("../outside.gz"); "escaping_locator")]
    #[test_case("uncompressed_bytes", json!(0); "changed_extent")]
    fn rejects_changed_chunk_metadata(field: &str, value: Value) {
        let (temp, context) = fixture();
        let path = temp.path().join("journal-manifest.json");
        let mut manifest: Value = read_json(&path).unwrap();
        manifest["chunks"][1][field] = value;
        artifact::replace_json(&path, &manifest).unwrap();
        assert!(replay(temp.path(), &context, |_| Ok(())).is_err());
    }

    #[test]
    fn rejects_corrupt_compressed_content() {
        let (temp, context) = fixture();
        let path = temp.path().join("journal-000001.jsonl.gz");
        let mut bytes = std::fs::read(&path).unwrap();
        bytes[20] ^= 1;
        std::fs::write(path, bytes).unwrap();
        assert!(replay(temp.path(), &context, |_| Ok(())).is_err());
    }

    #[test]
    fn rotated_journal_replays_and_rejects_other_generation() {
        let temp = tempfile::tempdir().unwrap();
        let context = Context {
            run: "fixture".into(),
            source_identity: "source".into(),
            plan_sha256: "plan".into(),
        };
        let mut journal = Journal::new(temp.path(), context.clone(), 65536).unwrap();
        for n in 0..150 {
            journal
                .append(
                    "source".into(),
                    "fixture".into(),
                    json!({"n": n, "body": "x".repeat(1024)}),
                )
                .unwrap();
        }
        assert!(journal.close(true).unwrap().chunks.len() > 1);
        assert_eq!(replay(temp.path(), &context, |_| Ok(())).unwrap(), 150);
        let wrong = Context {
            plan_sha256: "other".into(),
            ..context
        };
        assert!(replay(temp.path(), &wrong, |_| Ok(())).is_err());
    }

    #[test]
    fn incomplete_journal_never_passes_replay() {
        let temp = tempfile::tempdir().unwrap();
        let context = Context {
            run: "fixture".into(),
            source_identity: "source".into(),
            plan_sha256: "plan".into(),
        };
        Journal::new(temp.path(), context.clone(), 65536)
            .unwrap()
            .close(false)
            .unwrap();
        assert!(replay(temp.path(), &context, |_| Ok(())).is_err());
    }
}
