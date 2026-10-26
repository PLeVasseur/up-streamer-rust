// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Typed selection for maintained execution/replay lanes; existing CLI stays intact.
use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Selection {
    Matrix,
    DirectPeers,
    SingleTypeControls,
    Sustained,
    ReplaySustained,
    ReplayWorkload,
}

pub(super) fn select(cli: &Cli) -> Result<Selection> {
    let requested = [
        cli.direct_pairs,
        cli.single_type_controls,
        cli.soak.is_some(),
        cli.replay_soak.is_some(),
        cli.replay_workload.is_some(),
    ];
    anyhow::ensure!(
        requested.into_iter().filter(|value| *value).count() <= 1,
        "execution and replay lanes are mutually exclusive"
    );
    Ok(if cli.direct_pairs {
        Selection::DirectPeers
    } else if cli.single_type_controls {
        Selection::SingleTypeControls
    } else if cli.soak.is_some() {
        Selection::Sustained
    } else if cli.replay_soak.is_some() {
        Selection::ReplaySustained
    } else if cli.replay_workload.is_some() {
        Selection::ReplayWorkload
    } else {
        Selection::Matrix
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn incompatible_real_and_replay_lanes_fail_before_process_setup() {
        for options in [
            vec!["--direct-pairs", "--single-type-controls"],
            vec!["--direct-pairs", "--soak", "diagnostic"],
            vec!["--single-type-controls", "--replay-soak", "unused"],
        ] {
            let cli = Cli::try_parse_from(std::iter::once("orchestrator").chain(options)).unwrap();
            assert!(select(&cli).is_err());
        }
    }
}
