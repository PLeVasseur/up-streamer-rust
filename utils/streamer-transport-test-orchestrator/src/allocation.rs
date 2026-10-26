// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Explicit test-only actor allocation observation; counters include observer cost.
use super::*;
use std::ffi::OsString;
use std::os::unix::fs::FileExt;

pub(super) struct Launch {
    loader: PathBuf,
    library: PathBuf,
    counter: PathBuf,
}
impl Launch {
    pub(super) fn configured(directory: &Path, actor: &str) -> Result<Option<Self>> {
        if !matches!(actor, "streamer" | "active" | "passive" | "mqtt-broker") {
            return Ok(None);
        }
        let Some(library) = std::env::var_os("UPROTOCOL_ALLOCATION_OBSERVER") else {
            return Ok(None);
        };
        let library = fs::canonicalize(library)?;
        let loader = fs::canonicalize(
            std::env::var_os("UPROTOCOL_ALLOCATION_LOADER")
                .context("allocation observer requires an explicit frozen loader")?,
        )?;
        anyhow::ensure!(
            library.is_file()
                && loader.is_file()
                && actor
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "invalid observer inputs or actor"
        );
        let counter = directory.join(format!("{actor}.allocation-counters"));
        anyhow::ensure!(!counter.exists(), "allocation counter file already exists");
        atomic_write_json(
            &directory.join(format!("{actor}.allocation-setup.json")),
            &json!({
                "schema":1,"actor":actor,"counter":counter,"library":library,"library_sha256":sha256_file(&library)?,
                "loader":loader,"loader_sha256":sha256_file(&loader)?,"window":"constructor-through-exit; readiness/phase snapshots define deltas; no resets or cost subtraction",
                "scope":"glibc allocation requests, Rust System and native C/C++; external/custom allocators outside this explicit mechanism are not counted"
            }),
        )?;
        Ok(Some(Self {
            loader,
            library,
            counter,
        }))
    }
    pub(super) fn program(&self) -> &Path {
        &self.loader
    }
    pub(super) fn prefix(&self, executable: &Path) -> [OsString; 3] {
        [
            "--preload".into(),
            self.library.as_os_str().into(),
            executable.as_os_str().into(),
        ]
    }
    pub(super) fn environment(&self, command: &mut Command) {
        command.env("UPROTOCOL_ALLOCATION_COUNTER_FILE", &self.counter);
    }
}

fn read(path: &Path) -> Result<serde_json::Value> {
    let file = File::open(path)?;
    anyhow::ensure!(
        file.metadata()?.len() == 4096,
        "invalid allocation mapping size"
    );
    for _ in 0..10_000 {
        let mut first = [0u8; 8];
        file.read_exact_at(&mut first, 32)?;
        let sequence = u64::from_ne_bytes(first);
        if sequence & 1 != 0 {
            thread::yield_now();
            continue;
        }
        let mut bytes = [0u8; 152];
        file.read_exact_at(&mut bytes, 0)?;
        let mut last = [0u8; 8];
        file.read_exact_at(&mut last, 32)?;
        if first != last {
            thread::yield_now();
            continue;
        }
        let words: Vec<u64> = bytes
            .chunks_exact(8)
            .map(|v| u64::from_ne_bytes(v.try_into().expect("eight bytes")))
            .collect();
        if words[4] != sequence {
            continue;
        }
        anyhow::ensure!(
            words[0] == 0x3130434f4c415055
                && words[1] == 1
                && words[2] == 152
                && words[3] > 0
                && words[5] == 1
                && words[16] == 0
                && words[17] == 64,
            "invalid/inactive/overflowed observer header"
        );
        anyhow::ensure!(
            words[6] == words[10..14].iter().sum::<u64>()
                && words[8] <= words[6]
                && words[9] <= words[7]
                && words[15] <= words[14],
            "incoherent allocation counter families/scopes"
        );
        return Ok(
            json!({"pid":words[3],"sequence":sequence,"process_calls":words[6],"process_requested_bytes":words[7],"binding_calls":words[8],"binding_requested_bytes":words[9],"malloc":words[10],"calloc":words[11],"realloc":words[12],"aligned":words[13],"scope_entries":words[14],"scope_threads":words[15],"process_allocation_threads":words[18]}),
        );
    }
    Err(anyhow!(
        "allocation counter snapshot did not become coherent"
    ))
}

pub(super) fn snapshot(directory: &Path, phase: &str, expected_actors: usize) -> Result<()> {
    if std::env::var_os("UPROTOCOL_ALLOCATION_OBSERVER").is_none() {
        return Ok(());
    }
    let mut counters = BTreeMap::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if entry
            .path()
            .extension()
            .is_some_and(|x| x == "allocation-counters")
        {
            counters.insert(
                entry.file_name().to_string_lossy().into_owned(),
                read(&entry.path())?,
            );
        }
    }
    let broker = usize::from(counters.contains_key("mqtt-broker.allocation-counters"));
    anyhow::ensure!(
        counters.len() == expected_actors + broker,
        "partial actor allocation observation"
    );
    let path = directory.join("allocation-snapshots.jsonl");
    let mut output = OpenOptions::new().create(true).append(true).open(path)?;
    serde_json::to_writer(
        &mut output,
        &json!({"phase":phase,"at":Utc::now(),"counters":counters}),
    )?;
    output.write_all(b"\n")?;
    output.sync_data()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn file(words: [u64; 19]) -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "up-allocation-reader-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut bytes = vec![0; 4096];
        for (chunk, value) in bytes.chunks_exact_mut(8).zip(words) {
            chunk.copy_from_slice(&value.to_ne_bytes());
        }
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap()
            .write_all(&bytes)
            .unwrap();
        path
    }
    fn valid() -> [u64; 19] {
        let mut words = [0; 19];
        words[0] = 0x3130434f4c415055;
        words[1] = 1;
        words[2] = 152;
        words[3] = 1;
        words[5] = 1;
        words[17] = 64;
        words
    }
    #[test]
    fn reader_rejects_partial_headers_overflow_and_incoherent_families() {
        for index in [0, 1, 2, 3, 5, 6, 8, 9, 15, 16, 17] {
            let mut words = valid();
            words[index] ^= 1;
            let path = file(words);
            assert!(read(&path).is_err(), "corrupt word {index}");
            fs::remove_file(path).unwrap();
        }
        let path = file(valid());
        assert!(read(&path).is_ok());
        OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(151)
            .unwrap();
        assert!(read(&path).is_err());
        fs::remove_file(path).unwrap();
    }
}
