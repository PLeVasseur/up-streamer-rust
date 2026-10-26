// SPDX-License-Identifier: Apache-2.0
//! Bounded, hashed artifact I/O shared by execution and independent replay.
use anyhow::{ensure, Result};
use serde::{de::DeserializeOwned, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, Read, Write};
use std::path::Path;

pub(crate) const MAX_LINE: usize = 65_536;

pub(crate) fn is_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub(crate) fn hash_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

pub(crate) fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let file = File::open(path)?;
    ensure!(
        file.metadata()?.len() <= 16 * 1024 * 1024,
        "artifact exceeds input bound"
    );
    let mut bytes = Vec::new();
    file.take(16 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= 16 * 1024 * 1024,
        "artifact grew beyond input bound"
    );
    Ok(serde_json::from_slice(&bytes)?)
}

pub(crate) fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    serde_json::to_writer_pretty(&mut file, value)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    Ok(())
}

#[cfg(test)]
pub(crate) fn replace_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let temporary = path.with_extension("next");
    let mut file = File::create(&temporary)?;
    serde_json::to_writer(&mut file, value)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    std::fs::rename(temporary, path)?;
    Ok(())
}

pub(crate) fn bounded_line(reader: &mut impl BufRead, line: &mut Vec<u8>) -> Result<bool> {
    line.clear();
    reader.take((MAX_LINE + 1) as u64).read_until(b'\n', line)?;
    ensure!(line.len() <= MAX_LINE, "record exceeds bounded line size");
    ensure!(
        line.is_empty() || line.last() == Some(&b'\n'),
        "truncated final record"
    );
    Ok(!line.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_oversized_and_truncated_records() {
        let mut output = Vec::new();
        assert!(bounded_line(&mut &b"partial"[..], &mut output).is_err());
        let bytes = vec![b'x'; MAX_LINE + 1];
        assert!(bounded_line(&mut bytes.as_slice(), &mut output).is_err());
        assert!(bounded_line(&mut &b"complete\n"[..], &mut output).unwrap());
        assert!(!bounded_line(&mut &b""[..], &mut output).unwrap());
    }
}
