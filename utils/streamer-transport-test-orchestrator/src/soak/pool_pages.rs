// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Linux owning-pool residency census with raw snapshots and explicit alias identity.
use super::*;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Mapping {
    start: u64,
    end: u64,
    permissions: String,
    offset: u64,
    device: String,
    inode: u64,
    path: String,
    size: u64,
    rss: u64,
    locked: u64,
    flags: Vec<String>,
}

fn parse_smaps(raw: &str) -> Result<Vec<Mapping>> {
    let mut headers = Vec::new();
    let mut current: Option<(Vec<String>, BTreeMap<String, String>)> = None;
    for line in raw.lines() {
        let columns: Vec<_> = line.split_whitespace().collect();
        if columns.first().is_some_and(|first| first.contains('-')) {
            if let Some(value) = current.take() {
                headers.push(value);
            }
            anyhow::ensure!(columns.len() >= 5, "incomplete mapping header");
            current = Some((
                columns.iter().map(|s| (*s).to_string()).collect(),
                BTreeMap::new(),
            ));
        } else if let Some((_, fields)) = &mut current {
            if let Some((name, value)) = line.split_once(':') {
                anyhow::ensure!(
                    fields.insert(name.into(), value.trim().into()).is_none(),
                    "duplicate mapping metric"
                );
            }
        }
    }
    if let Some(value) = current {
        headers.push(value);
    }
    anyhow::ensure!(!headers.is_empty(), "empty mapping census");
    headers
        .into_iter()
        .map(|(header, fields)| {
            let (start, end) = header[0].split_once('-').context("mapping range")?;
            let start = u64::from_str_radix(start, 16)?;
            let end = u64::from_str_radix(end, 16)?;
            let bytes = |name: &str| -> Result<u64> {
                let values: Vec<_> = fields
                    .get(name)
                    .context("missing mapping metric")?
                    .split_whitespace()
                    .collect();
                anyhow::ensure!(
                    values.len() == 2 && values[1] == "kB",
                    "mapping metric unit"
                );
                values[0]
                    .parse::<u64>()?
                    .checked_mul(1024)
                    .context("mapping metric overflow")
            };
            let size = bytes("Size")?;
            let rss = bytes("Rss")?;
            let locked = bytes("Locked")?;
            anyhow::ensure!(
                size > 0 && end.checked_sub(start) == Some(size) && rss <= size && locked <= size,
                "invalid mapping extent"
            );
            Ok(Mapping {
                start,
                end,
                permissions: header[1].clone(),
                offset: u64::from_str_radix(&header[2], 16)?,
                device: header[3].clone(),
                inode: header[4].parse()?,
                path: header[5..].join(" "),
                size,
                rss,
                locked,
                flags: fields
                    .get("VmFlags")
                    .context("missing VMA flags")?
                    .split_whitespace()
                    .map(str::to_owned)
                    .collect(),
            })
        })
        .collect()
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Actor {
    role: String,
    process: ProcessSample,
    raw_file: String,
    raw_sha256: String,
    mappings: Vec<Mapping>,
    backings: Vec<BackingProbe>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BackingProbe {
    mapping: Mapping,
    descriptor_device: String,
    descriptor_inode: u64,
    file_bytes: u64,
    raw_file: String,
    raw_sha256: String,
    argv: Vec<String>,
    tool_sha256: String,
}

struct NativeTool<'a> {
    binary: &'a Path,
    sha256: &'a str,
    page_size: u64,
}

fn fincore_measurement(
    raw: &str,
    expected_file: &str,
    file_bytes: u64,
    mapping_bytes: u64,
    page_size: u64,
) -> Result<u64> {
    let value: serde_json::Value = serde_json::from_str(raw)?;
    let rows = value["fincore"]
        .as_array()
        .context("missing native page measurements")?;
    anyhow::ensure!(
        rows.len() == 1,
        "partial or ambiguous native page measurement"
    );
    let row = &rows[0];
    let pages = row["pages"]
        .as_u64()
        .context("missing native resident pages")?;
    let resident = row["res"]
        .as_u64()
        .context("missing native resident bytes")?;
    anyhow::ensure!(
        row["file"].as_str() == Some(expected_file)
            && row["size"].as_u64() == Some(file_bytes)
            && file_bytes > 0
            && file_bytes <= mapping_bytes
            && resident <= mapping_bytes
            && page_size >= 1024
            && page_size.is_power_of_two()
            && mapping_bytes.is_multiple_of(page_size)
            && pages.checked_mul(page_size) == Some(resident),
        "native page extent or descriptor mismatch"
    );
    Ok(resident)
}

#[cfg(target_os = "linux")]
fn probe_backing(
    directory: &Path,
    role: &str,
    process: &ProcessSample,
    index: usize,
    mapping: &Mapping,
    tool: &NativeTool<'_>,
    label: &str,
) -> Result<BackingProbe> {
    use std::os::{fd::AsRawFd, unix::fs::MetadataExt};
    anyhow::ensure!(
        mapping.path.starts_with("/dev/shm/") && mapping.offset == 0,
        "whole native backing required"
    );
    let original = format!("/proc/{}/root{}", process.pid, mapping.path);
    let file = File::open(&original)?;
    let metadata = file.metadata()?;
    let device = format!(
        "{:02x}:{:02x}",
        libc::major(metadata.dev()),
        libc::minor(metadata.dev())
    );
    anyhow::ensure!(
        device == mapping.device
            && metadata.ino() == mapping.inode
            && metadata.len() <= mapping.size,
        "native descriptor and mapped object differ"
    );
    let descriptor = format!("/proc/{}/fd/{}", std::process::id(), file.as_raw_fd());
    let argv = vec![
        tool.binary.to_string_lossy().into_owned(),
        "--json".into(),
        "--bytes".into(),
        "--output".into(),
        "PAGES,RES,SIZE,FILE".into(),
        descriptor,
    ];
    let output = Command::new(tool.binary).args(&argv[1..]).output()?;
    anyhow::ensure!(
        output.status.success() && output.stdout.len() <= 65536 && output.stderr.len() <= 65536,
        "native backing residency measurement failed"
    );
    anyhow::ensure!(
        process_identity(process.pid)? == (process.process_group, process.start_time_ticks),
        "pool process changed during native query"
    );
    let raw = String::from_utf8(output.stdout)?;
    let resident =
        fincore_measurement(&raw, &argv[5], metadata.len(), mapping.size, tool.page_size)?;
    anyhow::ensure!(
        resident == mapping.size,
        "native backing was not fully materialized before normal traffic"
    );
    let raw_file = format!(
        "pool-pages-{label}-{role}-{}-{index}.fincore.json",
        process.pid
    );
    let path = directory.join(&raw_file);
    anyhow::ensure!(!path.exists(), "native backing snapshot already exists");
    fs::write(&path, raw)?;
    Ok(BackingProbe {
        mapping: mapping.clone(),
        descriptor_device: device,
        descriptor_inode: metadata.ino(),
        file_bytes: metadata.len(),
        raw_file,
        raw_sha256: sha256_file(&path)?,
        argv,
        tool_sha256: tool.sha256.into(),
    })
}

#[cfg(not(target_os = "linux"))]
fn probe_backing(
    _: &Path,
    _: &str,
    _: &ProcessSample,
    _: usize,
    _: &Mapping,
    _: &NativeTool<'_>,
    _: &str,
) -> Result<BackingProbe> {
    Err(anyhow!("native backing-page measurements require Linux"))
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Row {
    row: String,
    actors: Vec<Actor>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Census {
    schema: String,
    plan_sha256: String,
    elapsed: f64,
    rows: Vec<Row>,
}

fn process_identity(pid: u32) -> Result<(i32, u64)> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat"))?;
    let fields: Vec<_> = stat
        .rsplit_once(')')
        .context("process comm")?
        .1
        .split_whitespace()
        .collect();
    Ok((
        fields.get(2).context("process group")?.parse()?,
        fields.get(19).context("process start time")?.parse()?,
    ))
}

fn large_shared(raw: &str, minimum: u64) -> Result<Vec<Mapping>> {
    Ok(parse_smaps(raw)?
        .into_iter()
        .filter(|mapping| {
            mapping.permissions.as_bytes().get(3) == Some(&b's')
                && mapping.inode != 0
                && mapping.size >= minimum
        })
        .collect())
}

pub(super) fn capture(
    root: &Path,
    streams: &[Stream],
    plan_sha256: &str,
    elapsed: f64,
    label: &str,
) -> Result<()> {
    let source: serde_json::Value = read(&root.join("source-manifest.json"))?;
    let tool = PathBuf::from(
        source["native_tool"]["binary"]
            .as_str()
            .context("declared native page tool missing")?,
    );
    let tool_sha256 = source["native_tool"]["binary_sha256"]
        .as_str()
        .context("native tool hash missing")?;
    let page_size = source["native_tool"]["page_size"]
        .as_u64()
        .context("native page size missing")?;
    anyhow::ensure!(
        sha256_file(&tool)? == tool_sha256,
        "native page tool changed"
    );
    let tool = NativeTool {
        binary: &tool,
        sha256: tool_sha256,
        page_size,
    };
    let processes = process_tree_samples(std::process::id())?;
    let mut rows = Vec::new();
    for stream in streams {
        let row = stream
            .directory
            .file_name()
            .and_then(|n| n.to_str())
            .context("row name")?
            .to_string();
        let groups: serde_json::Value = read(&stream.directory.join("owned-groups.json"))?;
        let geometry: serde_json::Value = read(&stream.directory.join("sensor-geometry.json"))?;
        let minimum = geometry["payload_bytes"].as_u64().context("payload size")?;
        let mut actors = Vec::new();
        for role in ["source", "bridge", "sink"] {
            let group = i32::try_from(groups[role].as_i64().context("owned group")?)?;
            let mut found = false;
            for process in processes.iter().filter(|p| p.process_group == group) {
                let identity = (process.process_group, process.start_time_ticks);
                anyhow::ensure!(
                    identity.1 > 0 && process_identity(process.pid)? == identity,
                    "process identity changed before census"
                );
                let raw = fs::read_to_string(format!("/proc/{}/smaps", process.pid))?;
                anyhow::ensure!(
                    raw.len() <= 16 * 1024 * 1024 && process_identity(process.pid)? == identity,
                    "mapping census size or process identity changed"
                );
                let raw_file = format!("pool-pages-{label}-{role}-{}.smaps", process.pid);
                let path = stream.directory.join(&raw_file);
                anyhow::ensure!(!path.exists(), "mapping snapshot already exists");
                fs::write(&path, &raw)?;
                let mappings = large_shared(&raw, minimum)?;
                let mut backings = Vec::new();
                for (index, mapping) in mappings.iter().enumerate() {
                    backings.push(probe_backing(
                        &stream.directory,
                        role,
                        process,
                        index,
                        mapping,
                        &tool,
                        label,
                    )?);
                }
                found |= !mappings.is_empty();
                actors.push(Actor {
                    role: role.into(),
                    process: process.clone(),
                    raw_file,
                    raw_sha256: sha256_file(&path)?,
                    mappings,
                    backings,
                });
            }
            anyhow::ensure!(found, "missing {row}/{role} native shared mapping");
        }
        rows.push(Row { row, actors });
    }
    atomic_write_json(
        &root.join(format!("pool-page-census-{label}.json")),
        &Census {
            schema: "uprotocol.pool-page-census.v2".into(),
            plan_sha256: plan_sha256.into(),
            elapsed,
            rows,
        },
    )
}

fn backing(mapping: &Mapping) -> (&str, u64, u64, u64) {
    (&mapping.device, mapping.inode, mapping.offset, mapping.size)
}

pub(super) fn audit(
    root: &Path,
    plan: &Plan,
    phases: &[serde_json::Value],
    label: &str,
    end_census_completed: Option<f64>,
) -> Result<serde_json::Value> {
    let path = root.join(format!("pool-page-census-{label}.json"));
    let census: Census = read(&path)?;
    anyhow::ensure!(
        census.schema == "uprotocol.pool-page-census.v2"
            && census.plan_sha256 == sha256_file(&root.join("soak-plan.json"))?,
        "pool census plan mismatch"
    );
    anyhow::ensure!(
        census.rows.iter().map(|r| &r.row).eq(plan.rows.iter()),
        "partial pool census"
    );
    let (warm, normal) = match label {
        "normal" => (
            phases[0]["completed_at_elapsed_seconds"]
                .as_f64()
                .context("warmup end")?,
            phases[1]["elapsed_seconds"]
                .as_f64()
                .context("normal start")?,
        ),
        "end" => (
            phases[3]["completed_at_elapsed_seconds"]
                .as_f64()
                .context("drain end")?,
            end_census_completed.context("missing pre-stop census completion")?,
        ),
        _ => return Err(anyhow!("invalid census boundary")),
    };
    anyhow::ensure!(
        census.elapsed >= warm && census.elapsed <= normal,
        "pool census is outside its declared active-endpoint boundary"
    );
    let mut reports = Vec::new();
    for row in &census.rows {
        let directory = root.join(&row.row);
        let geometry: serde_json::Value = read(&directory.join("sensor-geometry.json"))?;
        let groups: serde_json::Value = read(&directory.join("owned-groups.json"))?;
        let config: serde_json::Value = read(&directory.join("configurable-streamer.json"))?;
        anyhow::ensure!(
            config.pointer("/transports/zenoh/endpoints/0/zenoh_shm_prewarm") == Some(&json!(true)),
            "provider prewarming was not configured"
        );
        let minimum = geometry["payload_bytes"]
            .as_u64()
            .context("payload extent")?;
        let mut mappings: BTreeMap<String, Vec<&Mapping>> = BTreeMap::new();
        let mut resident_backings: BTreeMap<(String, u64, u64, u64), u64> = BTreeMap::new();
        let source_manifest: serde_json::Value = read(&root.join("source-manifest.json"))?;
        let page_size = source_manifest["native_tool"]["page_size"]
            .as_u64()
            .context("native page size missing")?;
        let mut ids = BTreeSet::new();
        for actor in &row.actors {
            anyhow::ensure!(
                ["source", "bridge", "sink"].contains(&actor.role.as_str())
                    && ids.insert(actor.process.pid),
                "invalid pool actor"
            );
            anyhow::ensure!(
                actor.process.start_time_ticks > 0
                    && Some(i64::from(actor.process.process_group)) == groups[&actor.role].as_i64(),
                "pool process ownership mismatch"
            );
            anyhow::ensure!(
                actor.raw_file
                    == format!(
                        "pool-pages-{label}-{}-{}.smaps",
                        actor.role, actor.process.pid
                    ),
                "invalid raw census locator"
            );
            let raw_path = directory.join(&actor.raw_file);
            anyhow::ensure!(
                sha256_file(&raw_path)? == actor.raw_sha256
                    && large_shared(&fs::read_to_string(raw_path)?, minimum)? == actor.mappings,
                "raw mapping census mismatch"
            );
            anyhow::ensure!(
                actor.backings.len() == actor.mappings.len(),
                "partial backing probes"
            );
            for (index, probe) in actor.backings.iter().enumerate() {
                anyhow::ensure!(
                    probe.mapping == actor.mappings[index]
                        && probe.descriptor_device == probe.mapping.device
                        && probe.descriptor_inode == probe.mapping.inode
                        && probe.mapping.offset == 0,
                    "native backing descriptor mismatch"
                );
                anyhow::ensure!(
                    probe.raw_file
                        == format!(
                            "pool-pages-{label}-{}-{}-{index}.fincore.json",
                            actor.role, actor.process.pid
                        )
                        && probe.argv.len() == 6
                        && probe.argv[1..5]
                            == ["--json", "--bytes", "--output", "PAGES,RES,SIZE,FILE"]
                        && Some(probe.argv[0].as_str())
                            == source_manifest["native_tool"]["binary"].as_str()
                        && Some(probe.tool_sha256.as_str())
                            == source_manifest["native_tool"]["binary_sha256"].as_str(),
                    "native tool/argv identity mismatch"
                );
                let raw_path = directory.join(&probe.raw_file);
                anyhow::ensure!(
                    sha256_file(&raw_path)? == probe.raw_sha256,
                    "native backing transcript changed"
                );
                let descriptor_prefix = format!(
                    "/proc/{}/fd/",
                    groups["controller"]
                        .as_u64()
                        .context("controller identity missing")?
                );
                anyhow::ensure!(
                    probe.argv[5]
                        .strip_prefix(&descriptor_prefix)
                        .and_then(|value| value.parse::<u32>().ok())
                        .is_some_and(|fd| fd >= 3),
                    "native tool did not query an owned descriptor"
                );
                let resident = fincore_measurement(
                    &fs::read_to_string(raw_path)?,
                    &probe.argv[5],
                    probe.file_bytes,
                    probe.mapping.size,
                    page_size,
                )?;
                let key = (
                    probe.mapping.device.clone(),
                    probe.mapping.inode,
                    probe.mapping.offset,
                    probe.mapping.size,
                );
                anyhow::ensure!(
                    resident_backings
                        .insert(key, resident)
                        .is_none_or(|old| old == resident),
                    "backing residency observations disagree"
                );
            }
            mappings
                .entry(actor.role.clone())
                .or_default()
                .extend(&actor.mappings);
        }
        let source = mappings.get("source").context("missing source pool")?;
        let bridge = mappings.get("bridge").context("missing bridge pools")?;
        let sink = mappings.get("sink").context("missing sink pool")?;
        anyhow::ensure!(source.len() == 1, "ambiguous source pool");
        let arena = source[0];
        let reservation = geometry["sample_slice_bytes"]
            .as_u64()
            .context("sample slice")?
            .checked_mul(
                geometry["publisher_slots"]
                    .as_u64()
                    .context("publisher slots")?,
            )
            .context("pool reservation overflow")?;
        anyhow::ensure!(
            arena.size >= reservation
                && resident_backings.get(&(
                    arena.device.clone(),
                    arena.inode,
                    arena.offset,
                    arena.size
                )) == Some(&arena.size)
                && bridge.iter().any(|m| backing(m) == backing(arena)),
            "source pool capacity/residency/backing proof failed for {}",
            row.row
        );
        let providers: Vec<_> = bridge
            .iter()
            .filter(|m| backing(m) != backing(arena))
            .collect();
        anyhow::ensure!(providers.len() == 1, "ambiguous egress provider");
        let provider = providers[0];
        let configured = geometry["zenoh_pool_bytes"]
            .as_u64()
            .context("provider size")?;
        anyhow::ensure!(
            provider.size == configured
                && provider.rss == provider.size
                && resident_backings.get(&(
                    provider.device.clone(),
                    provider.inode,
                    provider.offset,
                    provider.size
                )) == Some(&provider.size)
                && provider.flags.iter().any(|f| f == "lo")
                && sink.iter().any(|m| backing(m) == backing(provider)),
            "egress provider capacity/residency/backing proof failed for {}",
            row.row
        );
        reports.push(json!({"row":row.row,"source_pool_bytes":arena.size,"source_backing_resident_bytes":arena.size,"source_vma_rss":arena.rss,"provider_bytes":provider.size,"provider_backing_resident_bytes":provider.size,"provider_vma_rss":provider.rss,"provider_locked_vma":true,"raw_snapshots":row.actors.len()}));
    }
    Ok(
        json!({"sha256":sha256_file(&path)?,"elapsed":census.elapsed,"physical_backing_fully_resident":true,"rows":reports}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_case::test_case;

    const RAW: &str = "1000-2000 rw-s 00000000 00:31 42 /dev/shm/pool\nSize: 4 kB\nRss: 4 kB\nLocked: 2 kB\nVmFlags: rd wr sh lo\n";

    #[test]
    fn shared_residency_keeps_proportional_lock_bytes_separate_from_vma_lock() {
        let parsed = parse_smaps(RAW).unwrap();
        assert_eq!(parsed[0].size, 4096);
        assert_eq!(parsed[0].rss, 4096);
        assert_eq!(parsed[0].locked, 2048);
        assert!(parsed[0].flags.iter().any(|f| f == "lo"));
        assert_eq!(backing(&parsed[0]), ("00:31", 42, 0, 4096));
    }

    #[test_case("Rss: 4 kB\n", ""; "missing_residency")]
    #[test_case("Size: 4 kB", "Size: 5 kB"; "wrong_extent")]
    #[test_case("Rss: 4 kB", "Rss: 5 kB"; "impossible_residency")]
    #[test_case("Size: 4 kB", "Size: 18446744073709551615 kB"; "overflow")]
    #[test_case("VmFlags: rd wr sh lo\n", ""; "missing_flags")]
    #[test_case("Rss: 4 kB", "Rss: 4 MB"; "wrong_units")]
    fn malformed_or_partial_kernel_measurements_fail(old: &str, new: &str) {
        assert!(parse_smaps(&RAW.replace(old, new)).is_err());
    }

    #[test_case("file"; "descriptor_identity")]
    #[test_case("size"; "file_extent")]
    #[test_case("pages"; "resident_page_count")]
    #[test_case("res"; "resident_byte_count")]
    fn native_page_measurement_rejects_wrong_identity_or_inconsistent_counts(field: &str) {
        let mut value =
            json!({"fincore":[{"file":"/proc/100/fd/3","size":4090,"pages":1,"res":4096}]});
        assert_eq!(
            fincore_measurement(&value.to_string(), "/proc/100/fd/3", 4090, 4096, 4096).unwrap(),
            4096
        );
        value["fincore"][0][field] = if field == "file" {
            json!("/proc/101/fd/3")
        } else {
            json!(0)
        };
        assert!(
            fincore_measurement(&value.to_string(), "/proc/100/fd/3", 4090, 4096, 4096).is_err()
        );
    }
}
