// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Complete managed native dependencies of the actual staged executables.
use super::*;

const LOLA: &str = "libup_lola_bridge.so";
const VSOMEIP: [&str; 3] = [
    "libvsomeip3.so.3",
    "libvsomeip3-cfg.so.3",
    "libvsomeip3-sd.so.3",
];

fn parse_needed(text: &str) -> Result<BTreeSet<String>> {
    if text.trim() == "There is no dynamic section in this file." {
        return Ok(BTreeSet::new());
    }
    anyhow::ensure!(
        text.lines()
            .any(|line| line.trim_start().starts_with("Dynamic section at offset ")),
        "readelf did not report a dynamic section or an explicit static ELF"
    );
    let mut needed = BTreeSet::new();
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let Some((tag, rest)) = line.split_once(char::is_whitespace) else {
            anyhow::ensure!(
                !line.starts_with("0x") && line != "(NEEDED)",
                "malformed dynamic record"
            );
            continue;
        };
        let rest = rest.trim_start();
        let Some(tag) = tag.strip_prefix("0x") else {
            anyhow::ensure!(
                tag != "(NEEDED)" && !rest.starts_with("(NEEDED)"),
                "malformed DT_NEEDED tag"
            );
            continue;
        };
        let tag = u64::from_str_radix(tag, 16).context("malformed dynamic tag")?;
        let (kind, value) = rest.split_once(')').context("malformed dynamic tag type")?;
        let kind = kind
            .strip_prefix('(')
            .context("malformed dynamic tag type")?;
        if tag != 1 && kind != "NEEDED" {
            continue;
        }
        anyhow::ensure!(
            tag == 1 && kind == "NEEDED",
            "mismatched DT_NEEDED tag/type"
        );
        let name = value
            .trim()
            .strip_prefix("Shared library: [")
            .and_then(|value| value.strip_suffix(']'))
            .context("malformed DT_NEEDED library")?;
        anyhow::ensure!(
            !name.is_empty()
                && name.len() <= 255
                && name.bytes().all(|byte| byte.is_ascii_alphanumeric()
                    || matches!(byte, b'.' | b'_' | b'-' | b'+')),
            "invalid DT_NEEDED SONAME"
        );
        needed.insert(name.to_string());
    }
    Ok(needed)
}

fn checked_output(success: bool, stdout: &[u8], stderr: &[u8]) -> Result<BTreeSet<String>> {
    anyhow::ensure!(
        success && stderr.is_empty(),
        "readelf failed or reported diagnostics"
    );
    anyhow::ensure!(
        stdout.len() <= 1024 * 1024,
        "readelf dynamic metadata is too large"
    );
    parse_needed(std::str::from_utf8(stdout).context("readelf output is not UTF-8")?)
}

static TOOL_STARTED: AtomicU64 = AtomicU64::new(0);
static TOOL_CLEANUPS: AtomicU64 = AtomicU64::new(0);
static TOOL_CLEANUP_FAILURES: AtomicU64 = AtomicU64::new(0);
static SOURCE_EXECUTABLES: AtomicU64 = AtomicU64::new(0);
static SOURCE_LIBRARIES: AtomicU64 = AtomicU64::new(0);
static BUNDLE_FILES: AtomicU64 = AtomicU64::new(0);

#[derive(Serialize)]
pub(super) struct MetadataToolProcesses {
    pub(super) started: u64,
    pub(super) cleanup_attempts: u64,
    pub(super) cleanup_failures: u64,
    pub(super) source_executable_inspections: u64,
    pub(super) source_library_inspections: u64,
    pub(super) bundle_file_inspections: u64,
}

pub(super) fn metadata_tool_summary() -> MetadataToolProcesses {
    MetadataToolProcesses {
        started: TOOL_STARTED.load(Ordering::SeqCst),
        cleanup_attempts: TOOL_CLEANUPS.load(Ordering::SeqCst),
        cleanup_failures: TOOL_CLEANUP_FAILURES.load(Ordering::SeqCst),
        source_executable_inspections: SOURCE_EXECUTABLES.load(Ordering::SeqCst),
        source_library_inspections: SOURCE_LIBRARIES.load(Ordering::SeqCst),
        bundle_file_inspections: BUNDLE_FILES.load(Ordering::SeqCst),
    }
}

const TOOL_TIMEOUT: Duration = Duration::from_secs(10);
const STDOUT_CAP: usize = 1024 * 1024;
const STDERR_CAP: usize = 64 * 1024;

fn append_capped(output: &mut Vec<u8>, bytes: &[u8], cap: usize) -> Result<()> {
    anyhow::ensure!(
        bytes.len() <= cap.saturating_sub(output.len()),
        "readelf output exceeded its capture cap"
    );
    output.extend_from_slice(bytes);
    Ok(())
}

fn read_chunk(pipe: &mut impl Read, output: &mut Vec<u8>, cap: usize) -> Result<bool> {
    let mut bytes = [0; 8192];
    let count = pipe.read(&mut bytes)?;
    append_capped(output, &bytes[..count], cap)?;
    Ok(count == 0)
}

fn inspect(tool: &Path, path: &Path, counter: &AtomicU64) -> Result<BTreeSet<String>> {
    use std::os::fd::AsRawFd;
    // Reuse owned-group cleanup; never execute the inspected ELF. Pipes are
    // drained together with bounded buffers, so stderr cannot deadlock stdout.
    let mut command = Command::new(tool);
    command
        .args(["-d", "--wide", "--"])
        .arg(path)
        .env_clear()
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut process = RunningProcess::spawn(&mut command, "readelf", path.to_path_buf())?;
    TOOL_STARTED.fetch_add(1, Ordering::SeqCst);
    counter.fetch_add(1, Ordering::SeqCst);
    let result = (|| {
        let mut stdout = process
            .child
            .stdout
            .take()
            .context("readelf stdout pipe missing")?;
        let mut stderr = process
            .child
            .stderr
            .take()
            .context("readelf stderr pipe missing")?;
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut out_done = false;
        let mut err_done = false;
        let deadline = Instant::now() + TOOL_TIMEOUT;
        loop {
            let status = process.child.try_wait()?;
            if out_done && err_done {
                if let Some(status) = status {
                    return checked_output(status.success(), &out, &err);
                }
            }
            anyhow::ensure!(
                Instant::now() < deadline,
                "readelf metadata deadline expired"
            );
            let mut descriptors = [
                libc::pollfd {
                    fd: if out_done { -1 } else { stdout.as_raw_fd() },
                    events: libc::POLLIN | libc::POLLHUP,
                    revents: 0,
                },
                libc::pollfd {
                    fd: if err_done { -1 } else { stderr.as_raw_fd() },
                    events: libc::POLLIN | libc::POLLHUP,
                    revents: 0,
                },
            ];
            // SAFETY: both descriptors belong to the still-owned pipe handles;
            // poll only writes this fixed two-element array during this call.
            let ready = unsafe { libc::poll(descriptors.as_mut_ptr(), 2, 20) };
            if ready < 0 {
                let error = std::io::Error::last_os_error();
                if error.kind() == ErrorKind::Interrupted {
                    continue;
                }
                return Err(error.into());
            }
            for descriptor in &descriptors {
                anyhow::ensure!(
                    descriptor.revents & libc::POLLNVAL == 0,
                    "readelf pipe descriptor became invalid"
                );
            }
            if descriptors[0].revents != 0 {
                out_done = read_chunk(&mut stdout, &mut out, STDOUT_CAP)?;
            }
            if descriptors[1].revents != 0 {
                err_done = read_chunk(&mut stderr, &mut err, STDERR_CAP)?;
            }
        }
    })();
    // Includes timeout, read/parse/cap failure and successful leader exit. The
    // shared supervisor drains the entire group with its existing2s/1s/2s policy.
    TOOL_CLEANUPS.fetch_add(1, Ordering::SeqCst);
    let cleanup = process.terminate();
    if cleanup.is_err() {
        TOOL_CLEANUP_FAILURES.fetch_add(1, Ordering::SeqCst);
    }
    cleanup.context("readelf process-group cleanup failed")?;
    result.with_context(|| format!("native dependency inspection failed for {}", path.display()))
}

fn boost_platform_soname(name: &str) -> bool {
    // vSomeIP's established Boost components are host ABI dependencies. Match
    // their ELF SONAME contract across installed Boost versions, not one distro.
    let Some((component, version)) = name
        .strip_prefix("libboost_")
        .and_then(|name| name.split_once(".so."))
    else {
        return false;
    };
    if !matches!(component, "thread" | "filesystem" | "system") {
        return false;
    }
    let parts: Vec<_> = version.split('.').collect();
    parts.len() == 3
        && parts.iter().all(|part| {
            !part.is_empty() && part.len() <= 5 && part.bytes().all(|b| b.is_ascii_digit())
        })
}

fn managed_names(needed: &BTreeSet<String>) -> Result<BTreeSet<String>> {
    let mut managed = BTreeSet::new();
    for name in needed {
        if name == LOLA {
            managed.insert(name.clone());
        } else if VSOMEIP.contains(&name.as_str()) {
            // cfg and sd are runtime plugins, not necessarily DT_NEEDED entries.
            managed.extend(VSOMEIP.map(str::to_string));
        } else {
            // Explicit host ABI dependencies remain supplied by the host image.
            // Unknown SONAMEs fail closed rather than searching global LD paths.
            anyhow::ensure!(
                boost_platform_soname(name)
                    || matches!(
                        name.as_str(),
                        "ld-linux-x86-64.so.2"
                            | "ld-linux-aarch64.so.1"
                            | "libc.so.6"
                            | "libm.so.6"
                            | "libgcc_s.so.1"
                            | "libstdc++.so.6"
                            | "libpthread.so.0"
                            | "libdl.so.2"
                            | "librt.so.1"
                            | "libatomic.so.1"
                    ),
                "unresolved non-platform native dependency {name}"
            );
        }
    }
    Ok(managed)
}

fn require_complete(needed: &BTreeSet<String>, provided: &BTreeSet<String>) -> Result<()> {
    for name in managed_names(needed)? {
        anyhow::ensure!(
            provided.contains(&name),
            "run bundle is missing native dependency {name}"
        );
    }
    Ok(())
}

pub(super) fn inputs(
    executables: &[PathBuf],
    target_directory: &Path,
) -> Result<Vec<(BundleFileKind, String, PathBuf)>> {
    let tool = resolve_executable("readelf")?;
    let mut required = BTreeSet::new();
    for path in executables {
        required.extend(managed_names(&inspect(&tool, path, &SOURCE_EXECUTABLES)?)?);
    }
    let mut libraries = BTreeMap::new();
    // There are only four managed SONAMEs. Inspect their own dependencies too.
    while let Some(name) = required
        .iter()
        .find(|name| !libraries.contains_key(*name))
        .cloned()
    {
        let directory = if name == LOLA {
            detect_lola_bridge_lib_dir(target_directory)?
        } else {
            detect_vsomeip_lib_dir(target_directory)?
        };
        let path = fs::canonicalize(directory.join(&name))
            .with_context(|| format!("required native library {name} is missing"))?;
        required.extend(managed_names(&inspect(&tool, &path, &SOURCE_LIBRARIES)?)?);
        libraries.insert(name, path);
    }
    Ok(libraries
        .into_iter()
        .map(|(name, path)| (BundleFileKind::NativeLibrary, name, path))
        .collect())
}

pub(super) fn validate(bundle: &RunBundle) -> Result<()> {
    // Called after all existing manifest/path/mode/hash checks. This checks the
    // staged immutable bytes, also closing inspection-before-copy source races.
    let tool = resolve_executable("readelf")?;
    let mut needed = BTreeSet::new();
    let mut provided = BTreeSet::new();
    for file in &bundle.manifest.files {
        if matches!(
            file.kind,
            BundleFileKind::MatrixExecutable | BundleFileKind::NativeLibrary
        ) {
            needed.extend(inspect(
                &tool,
                &bundle.root.join(&file.bundle_path),
                &BUNDLE_FILES,
            )?);
        }
        if file.kind == BundleFileKind::NativeLibrary {
            provided.insert(file.name.clone());
        }
    }
    require_complete(&needed, &provided)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn dynamic(names: &[&str]) -> String {
        let mut text = "Dynamic section at offset 0x100 contains 4 entries:\n".to_string();
        for name in names {
            text.push_str(&format!(
                " 0x0000000000000001 (NEEDED) Shared library: [{name}]\n"
            ));
        }
        text
    }
    #[test]
    fn managed_dependencies_follow_staged_elf_not_selected_route() {
        let needs = parse_needed(&dynamic(&[LOLA, VSOMEIP[0], "libc.so.6"])).unwrap();
        assert_eq!(
            managed_names(&needs).unwrap(),
            BTreeSet::from([
                LOLA.to_string(),
                VSOMEIP[0].into(),
                VSOMEIP[1].into(),
                VSOMEIP[2].into()
            ])
        );
    }
    #[test]
    fn incomplete_import_rejects_both_managed_groups_and_plugins() {
        let needs = parse_needed(&dynamic(&[LOLA, VSOMEIP[0]])).unwrap();
        let complete = managed_names(&needs).unwrap();
        require_complete(&needs, &complete).unwrap();
        for missing in &complete {
            let mut provided = complete.clone();
            provided.remove(missing);
            assert!(
                require_complete(&needs, &provided).is_err(),
                "accepted missing {missing}"
            );
        }
    }
    #[test]
    fn malformed_needed_and_unknown_dependency_are_rejected() {
        let valid = dynamic(&[LOLA])
            + " 0x000000000000001d (RUNPATH) Library runpath: [/valid/NEEDED]\n"
            + " 0x000000000000000e (SONAME) Library soname: [libNEEDED.so]\n";
        assert_eq!(
            parse_needed(&valid).unwrap(),
            BTreeSet::from([LOLA.to_string()])
        );
        for invalid in [
            "Dynamic section at offset 0x100\n 0x1",
            "Dynamic section at offset 0x100\n (NEEDED)",
            "Dynamic section at offset 0x100\n 0x1 (RUNPATH) Library runpath: [/valid]",
            "Dynamic section at offset 0x100\n 0x1d (NEEDED) Shared library: [libc.so.6]",
            "Dynamic section at offset 0x100\n 0xinvalid (NEEDED) Shared library: [libc.so.6]",
        ] {
            assert!(parse_needed(invalid).is_err());
        }
        for text in [
            "",
            "Dynamic section at offset 0x100\n (NEEDED) missing brackets",
        ] {
            assert!(parse_needed(text).is_err());
        }
        for name in [
            "../libup_lola_bridge.so",
            "libvsomeip3.so.999",
            "libunqualified.so",
            "libboost_python.so.1.83.0",
            "libboost_thread.so.1.83",
            "libboost_filesystem.so.1.x.0",
            "libboost_thread.so.1.83.0/extra",
        ] {
            assert!(parse_needed(&dynamic(&[name]))
                .and_then(|n| managed_names(&n))
                .is_err());
        }
    }
    #[test]
    fn readelf_failure_never_becomes_an_empty_closure() {
        assert!(checked_output(false, b"", b"error").is_err());
        assert!(checked_output(true, dynamic(&[LOLA]).as_bytes(), b"warning").is_err());
        assert!(checked_output(true, &[0xff], b"").is_err());
        assert!(checked_output(true, b"", b"").is_err());
        let mut captured = Vec::new();
        append_capped(&mut captured, b"1234", 4).unwrap();
        assert!(append_capped(&mut captured, b"5", 4).is_err());
        assert_eq!(captured, b"1234");
    }
    #[test]
    fn static_and_platform_only_elf_need_no_backend_install() {
        for text in [
            "There is no dynamic section in this file.".to_string(),
            dynamic(&[
                "libc.so.6",
                "libboost_thread.so.1.83.0",
                "libboost_filesystem.so.1.74.0",
                "libboost_system.so.1.86.0",
            ]),
        ] {
            assert!(managed_names(&parse_needed(&text).unwrap())
                .unwrap()
                .is_empty());
        }
    }
}
