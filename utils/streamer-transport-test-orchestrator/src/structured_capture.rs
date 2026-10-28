// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0

//! Explicitly bound per-role structured proof capture, separate from diagnostics.

use super::*;

const DIRECTORY_ENV: &str = "UPROTOCOL_STRUCTURED_EVIDENCE_DIRECTORY";
const PATH_ENV: &str = "UPROTOCOL_STRUCTURED_EVIDENCE_PATH";

const MARKERS: &[&str] = &[
    "FLOW_MESSAGE_EVIDENCE",
    "FLOW_PAYLOAD_COPY",
    "FLOW_NATIVE_FORWARD_COMPLETED",
    "CLOUDEVENT_SETUP",
    "CLOUDEVENT_PROOF",
    "CLOUDEVENT_FRAME_WITNESS",
    "OP_EVENT",
    "OP_METRICS",
    "OP_ERROR",
    "OP_START",
    "OP_SOURCE_DRAINED",
    "OP_REJECTION",
    "OP_FINISH",
    "OP_TRANSPORT_HEALTH",
    "NATIVE_FANOUT_OBSERVATION",
    "NATIVE_FANOUT_ERROR",
    "NATIVE_FANOUT_REGISTERED",
    "NATIVE_FANOUT_FINISH",
    "NATIVE_DEPLOYMENT_LOADED",
    "NATIVE_IDENTITY_VERIFIED",
    "NATIVE_SHM_PROVIDER_INITIALIZED",
    "NATIVE_PROJECTION_VERIFIED",
];

fn destination(log_path: &Path) -> PathBuf {
    log_path.with_extension("records.jsonl")
}

/// Precreate owned output and bind the exact same environment passed to spawn.
pub(super) fn prepare(
    directory: &Path,
    name: &str,
    environment: &[(String, String)],
) -> Result<Vec<(String, String)>> {
    anyhow::ensure!(
        !environment
            .iter()
            .any(|(key, _)| key == PATH_ENV || key == DIRECTORY_ENV),
        "structured capture destination is owned by the launcher"
    );
    let mut environment = environment.to_vec();
    if !environment
        .iter()
        .any(|(key, _)| key == "UPROTOCOL_FLOW_EVIDENCE_RUN")
    {
        return Ok(environment);
    }
    anyhow::ensure!(
        directory.is_absolute(),
        "capture directory must be absolute"
    );
    let path = destination(&directory.join(format!("{name}.log")));
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .with_context(|| format!("unable to create structured capture {}", path.display()))?;
    environment.push((
        DIRECTORY_ENV.into(),
        directory.to_string_lossy().into_owned(),
    ));
    environment.push((PATH_ENV.into(), path.to_string_lossy().into_owned()));
    Ok(environment)
}

fn single<'a>(environment: &'a [(String, String)], name: &str) -> Result<&'a str> {
    let values: Vec<_> = environment.iter().filter(|(key, _)| key == name).collect();
    anyhow::ensure!(values.len() == 1, "expected one explicit {name}");
    Ok(values[0].1.as_str())
}

/// Select only the declared channel. Current captures never fall back to console.
/// Captured absolute paths are checked, then rebased inside the recovered row so
/// portable audits do not access an original live absolute filesystem location.
pub(super) fn path(log_path: &Path) -> Result<PathBuf> {
    let launch_path = log_path.with_extension("launch.json");
    let launch: serde_json::Value = serde_json::from_str(&fs::read_to_string(&launch_path)?)?;
    let environment: Vec<(String, String)> = serde_json::from_value(
        launch
            .get("environment")
            .cloned()
            .ok_or_else(|| anyhow!("launch environment missing"))?,
    )?;
    let directory = Path::new(single(&environment, DIRECTORY_ENV)?);
    let captured = Path::new(single(&environment, PATH_ENV)?);
    let recorded_log = Path::new(
        launch
            .get("log_path")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| anyhow!("actual launch log path missing"))?,
    );
    let selected = destination(log_path);
    let name = selected
        .file_name()
        .ok_or_else(|| anyhow!("structured filename missing"))?;
    let local_directory = log_path
        .parent()
        .ok_or_else(|| anyhow!("log directory missing"))?;
    anyhow::ensure!(
        directory.is_absolute()
            && captured.is_absolute()
            && !directory
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir))
            && recorded_log.is_absolute()
            && recorded_log.file_name() == log_path.file_name()
            && recorded_log.parent() == Some(directory)
            && directory.file_name() == local_directory.file_name()
            && captured == directory.join(name),
        "structured capture path is not bound to its owned role directory"
    );
    let metadata = fs::symlink_metadata(&selected).with_context(|| {
        format!(
            "declared structured capture missing: {}",
            selected.display()
        )
    })?;
    anyhow::ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "structured capture is not a regular owned file"
    );
    Ok(selected)
}

fn check_raw_errors(log_path: &Path) -> Result<String> {
    let raw = fs::read_to_string(log_path)?;
    anyhow::ensure!(
        !raw.contains("FLOW_EVIDENCE_ERROR") && !raw.contains("CLOUDEVENT_ERROR"),
        "evidence capture failed in {}",
        log_path.display()
    );
    anyhow::ensure!(
        !raw.lines().any(|line| line
            .split_once(' ')
            .is_some_and(|(marker, _)| MARKERS.contains(&marker))),
        "structured proof was emitted into the human console in {}",
        log_path.display()
    );
    Ok(raw)
}

fn check_line(line: &str) -> Result<()> {
    let (marker, _) = line
        .split_once(' ')
        .ok_or_else(|| anyhow!("unframed structured record"))?;
    anyhow::ensure!(
        MARKERS.contains(&marker),
        "foreign structured record prefix"
    );
    Ok(())
}

/// Stream closed capture records without turning sustained logs into one buffer.
pub(super) fn lines(log_path: &Path) -> Result<impl Iterator<Item = Result<String>>> {
    use std::io::{Read as _, Seek as _, SeekFrom};
    check_raw_errors(log_path)?;
    let selected = path(log_path)?;
    let mut file = File::open(&selected)?;
    if file.metadata()?.len() != 0 {
        file.seek(SeekFrom::End(-1))?;
        let mut last = [0];
        file.read_exact(&mut last)?;
        anyhow::ensure!(
            last == [b'\n'],
            "truncated structured record in {}",
            selected.display()
        );
        file.seek(SeekFrom::Start(0))?;
    }
    Ok(BufReader::new(file).lines().map(|line| {
        let line = line?;
        check_line(&line)?;
        Ok(line)
    }))
}

/// Preserve raw error diagnostics while reading final structured records strictly.
pub(super) fn read(log_path: &Path) -> Result<String> {
    let mut text = String::new();
    for line in lines(log_path)? {
        text.push_str(&line?);
        text.push('\n');
    }
    Ok(text)
}

/// Legacy readiness/native validators still see their original diagnostic text
/// and proof markers. During live observation only complete structured lines are
/// exposed; the final closed-record gate always checks the full tail strictly.
pub(super) fn diagnostic_text(log_path: &Path) -> Result<String> {
    let mut raw = check_raw_errors(log_path)?;
    let records = fs::read_to_string(path(log_path)?)?;
    let end = records.rfind('\n').map_or(0, |index| index + 1);
    for line in records[..end].lines() {
        check_line(line)?;
    }
    raw.push('\n');
    raw.push_str(&records[..end]);
    Ok(raw)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "structured-capture-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn fixture() -> (Scratch, PathBuf, Vec<(String, String)>) {
        let scratch = Scratch::new();
        let log = scratch.0.join("active.log");
        fs::write(&log, b"FOREIGN_PARTIAL human console\n").unwrap();
        let environment = prepare(
            &scratch.0,
            "active",
            &[("UPROTOCOL_FLOW_EVIDENCE_RUN".into(), "run".into())],
        )
        .unwrap();
        fs::write(
            log.with_extension("launch.json"),
            serde_json::to_vec(&json!({"environment":environment,"log_path":log})).unwrap(),
        )
        .unwrap();
        (scratch, log, environment)
    }

    #[test]
    fn declared_records_are_selected_without_resynchronizing_console() {
        let (_scratch, log, _) = fixture();
        fs::write(destination(&log), b"FLOW_MESSAGE_EVIDENCE {\"schema\":1}\n").unwrap();
        assert_eq!(
            read(&log).unwrap(),
            "FLOW_MESSAGE_EVIDENCE {\"schema\":1}\n"
        );
        fs::write(
            &log,
            b"READY listener_registered\nnative_sink_identity_rejected\n",
        )
        .unwrap();
        let diagnostics = diagnostic_text(&log).unwrap();
        assert!(diagnostics.contains("native_sink_identity_rejected"));
        assert!(diagnostics.contains("READY listener_registered"));
        assert!(diagnostics.contains("FLOW_MESSAGE_EVIDENCE {\"schema\":1}"));
    }

    #[test]
    fn declared_missing_foreign_truncated_and_injected_capture_never_falls_back() {
        let (_scratch, log, mut environment) = fixture();
        fs::write(&log, b"FLOW_MESSAGE_EVIDENCE {\"schema\":1}\n").unwrap();
        // Even a valid native/OP proof on console cannot satisfy the dedicated
        // source when its declared file is empty.
        assert!(diagnostic_text(&log).is_err());
        fs::write(
            &log,
            b"NATIVE_DEPLOYMENT_LOADED {\"schema\":1}\nOP_METRICS {}\n",
        )
        .unwrap();
        assert!(diagnostic_text(&log).is_err());
        fs::write(&log, b"READY listener_registered\n").unwrap();
        fs::remove_file(destination(&log)).unwrap();
        assert!(read(&log).is_err());
        fs::write(destination(&log), b"FLOW_MESSAGE_EVIDENCE {\"schema\":1}").unwrap();
        assert!(read(&log).is_err());
        fs::write(
            destination(&log),
            b"FOREIGN_PARTIAL FLOW_MESSAGE_EVIDENCE {}\n",
        )
        .unwrap();
        assert!(read(&log).is_err());
        environment
            .iter_mut()
            .find(|(key, _)| key == PATH_ENV)
            .unwrap()
            .1 = "/foreign/active.records.jsonl".into();
        fs::write(
            log.with_extension("launch.json"),
            serde_json::to_vec(&json!({"environment":environment,"log_path":log})).unwrap(),
        )
        .unwrap();
        fs::write(destination(&log), b"FLOW_MESSAGE_EVIDENCE {}\n").unwrap();
        assert!(read(&log).is_err());
        let foreign_directory =
            Path::new("/foreign").join(log.parent().unwrap().file_name().unwrap());
        environment
            .iter_mut()
            .find(|(key, _)| key == PATH_ENV)
            .unwrap()
            .1 = foreign_directory
            .join("active.records.jsonl")
            .to_string_lossy()
            .into_owned();
        environment
            .iter_mut()
            .find(|(key, _)| key == DIRECTORY_ENV)
            .unwrap()
            .1 = foreign_directory.to_string_lossy().into_owned();
        fs::write(
            log.with_extension("launch.json"),
            serde_json::to_vec(&json!({"environment":environment,"log_path":log})).unwrap(),
        )
        .unwrap();
        assert!(read(&log).is_err());
    }
}
