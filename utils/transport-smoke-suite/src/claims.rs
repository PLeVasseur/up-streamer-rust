use crate::env;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimKind {
    MustMatch,
    MustNotMatch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimCategory {
    EndpointCommunication,
    StreamerEgress,
    ForbiddenSignature,
    Readiness,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThresholdSelector {
    EndpointCommunication,
    EgressSendAttempt,
    EgressSendOk,
    EgressWorkerCreateOrReuse,
    Fixed(usize),
}

#[derive(Clone, Copy, Debug)]
pub struct ClaimTemplate {
    pub claim_id: &'static str,
    pub category: ClaimCategory,
    pub kind: ClaimKind,
    pub file: &'static str,
    pub pattern: &'static str,
    pub threshold: ThresholdSelector,
}

impl ClaimTemplate {
    pub const fn must_match(
        claim_id: &'static str,
        category: ClaimCategory,
        file: &'static str,
        pattern: &'static str,
        threshold: ThresholdSelector,
    ) -> Self {
        Self {
            claim_id,
            category,
            kind: ClaimKind::MustMatch,
            file,
            pattern,
            threshold,
        }
    }

    pub const fn must_not_match(
        claim_id: &'static str,
        category: ClaimCategory,
        file: &'static str,
        pattern: &'static str,
    ) -> Self {
        Self {
            claim_id,
            category,
            kind: ClaimKind::MustNotMatch,
            file,
            pattern,
            threshold: ThresholdSelector::Fixed(0),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Thresholds {
    pub endpoint_communication_min_count: usize,
    pub egress_send_attempt_min_count: usize,
    pub egress_send_ok_min_count: usize,
    pub egress_worker_create_or_reuse_min_count: usize,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            endpoint_communication_min_count: env::DEFAULT_ENDPOINT_CLAIM_MIN_COUNT,
            egress_send_attempt_min_count: env::DEFAULT_EGRESS_SEND_ATTEMPT_MIN_COUNT,
            egress_send_ok_min_count: env::DEFAULT_EGRESS_SEND_OK_MIN_COUNT,
            egress_worker_create_or_reuse_min_count: env::DEFAULT_EGRESS_WORKER_MIN_COUNT,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ClaimSpec {
    pub claim_id: String,
    pub category: ClaimCategory,
    pub kind: ClaimKind,
    pub file: String,
    pub pattern: String,
    pub min_count: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClaimOutcome {
    pub claim_id: String,
    pub category: ClaimCategory,
    pub kind: ClaimKind,
    pub file: String,
    pub pattern: String,
    pub min_count: usize,
    pub observed_count: usize,
    pub pass: bool,
    pub first_match: Option<String>,
    pub error: Option<String>,
}

pub fn materialize_claims(
    claim_templates: &[ClaimTemplate],
    thresholds: Thresholds,
) -> Vec<ClaimSpec> {
    claim_templates
        .iter()
        .map(|claim_template| ClaimSpec {
            claim_id: claim_template.claim_id.to_string(),
            category: claim_template.category,
            kind: claim_template.kind,
            file: claim_template.file.to_string(),
            pattern: claim_template.pattern.to_string(),
            min_count: resolve_threshold(claim_template.threshold, thresholds),
        })
        .collect()
}

fn resolve_threshold(selector: ThresholdSelector, thresholds: Thresholds) -> usize {
    match selector {
        ThresholdSelector::EndpointCommunication => thresholds.endpoint_communication_min_count,
        ThresholdSelector::EgressSendAttempt => thresholds.egress_send_attempt_min_count,
        ThresholdSelector::EgressSendOk => thresholds.egress_send_ok_min_count,
        ThresholdSelector::EgressWorkerCreateOrReuse => {
            thresholds.egress_worker_create_or_reuse_min_count
        }
        ThresholdSelector::Fixed(value) => value,
    }
}

pub fn evaluate_claims(artifacts_dir: &Path, claims: &[ClaimSpec]) -> Vec<ClaimOutcome> {
    claims
        .iter()
        .map(|claim| evaluate_claim(artifacts_dir, claim))
        .collect()
}

fn evaluate_claim(artifacts_dir: &Path, claim: &ClaimSpec) -> ClaimOutcome {
    let file_path = artifacts_dir.join(&claim.file);
    let file_content = match fs::read_to_string(&file_path) {
        Ok(file_content) => file_content,
        Err(error) => {
            return ClaimOutcome {
                claim_id: claim.claim_id.clone(),
                category: claim.category,
                kind: claim.kind,
                file: claim.file.clone(),
                pattern: claim.pattern.clone(),
                min_count: claim.min_count,
                observed_count: 0,
                pass: false,
                first_match: None,
                error: Some(format!("unable to read {}: {error}", file_path.display())),
            }
        }
    };

    let regex = match Regex::new(&claim.pattern) {
        Ok(regex) => regex,
        Err(error) => {
            return ClaimOutcome {
                claim_id: claim.claim_id.clone(),
                category: claim.category,
                kind: claim.kind,
                file: claim.file.clone(),
                pattern: claim.pattern.clone(),
                min_count: claim.min_count,
                observed_count: 0,
                pass: false,
                first_match: None,
                error: Some(format!("invalid regex '{}': {error}", claim.pattern)),
            }
        }
    };

    let mut match_iter = regex.find_iter(&file_content);
    let first_match = match_iter
        .next()
        .map(|first_match| first_match.as_str().to_string());
    let observed_count = first_match
        .as_ref()
        .map(|_| 1 + match_iter.count())
        .unwrap_or(0);

    let pass = match claim.kind {
        ClaimKind::MustMatch => observed_count >= claim.min_count,
        ClaimKind::MustNotMatch => observed_count == 0,
    };

    ClaimOutcome {
        claim_id: claim.claim_id.clone(),
        category: claim.category,
        kind: claim.kind,
        file: claim.file.clone(),
        pattern: claim.pattern.clone(),
        min_count: claim.min_count,
        observed_count,
        pass,
        first_match,
        error: None,
    }
}

pub fn split_claim_outcomes(
    outcomes: Vec<ClaimOutcome>,
) -> (Vec<ClaimOutcome>, Vec<ClaimOutcome>, Option<String>) {
    let mut must_outcomes = Vec::new();
    let mut forbidden_outcomes = Vec::new();
    let mut first_failure_reason = None;

    for outcome in outcomes {
        if !outcome.pass && first_failure_reason.is_none() {
            first_failure_reason = Some(format!(
                "claim '{}' failed (file='{}', observed={}, min={}, pattern='{}')",
                outcome.claim_id,
                outcome.file,
                outcome.observed_count,
                outcome.min_count,
                outcome.pattern
            ));
        }

        if outcome.kind == ClaimKind::MustNotMatch {
            forbidden_outcomes.push(outcome);
        } else {
            must_outcomes.push(outcome);
        }
    }

    (must_outcomes, forbidden_outcomes, first_failure_reason)
}

#[cfg(test)]
mod tests {
    use super::{
        evaluate_claims, materialize_claims, split_claim_outcomes, ClaimCategory, ClaimKind,
        ClaimTemplate, ThresholdSelector, Thresholds,
    };
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn must_match_counts_non_overlapping_regex_matches() {
        let temp_dir = TempDir::new().expect("create temp dir");
        fs::write(
            temp_dir.path().join("streamer.log"),
            "event=egress_send_attempt event=egress_send_attempt\n",
        )
        .expect("write fixture");

        let claims = materialize_claims(
            &[ClaimTemplate::must_match(
                "attempt",
                ClaimCategory::StreamerEgress,
                "streamer.log",
                "event=egress_send_attempt",
                ThresholdSelector::Fixed(2),
            )],
            Thresholds::default(),
        );

        let outcomes = evaluate_claims(temp_dir.path(), &claims);
        assert_eq!(outcomes[0].observed_count, 2);
        assert!(outcomes[0].pass);
    }

    #[test]
    fn must_not_match_fails_when_forbidden_pattern_exists() {
        let temp_dir = TempDir::new().expect("create temp dir");
        fs::write(
            temp_dir.path().join("streamer.log"),
            "thread panicked at boom\n",
        )
        .expect("write fixture");

        let claims = materialize_claims(
            &[ClaimTemplate::must_not_match(
                "no_panic",
                ClaimCategory::ForbiddenSignature,
                "streamer.log",
                "panicked at",
            )],
            Thresholds::default(),
        );

        let outcomes = evaluate_claims(temp_dir.path(), &claims);
        assert_eq!(outcomes[0].kind, ClaimKind::MustNotMatch);
        assert_eq!(outcomes[0].observed_count, 1);
        assert!(!outcomes[0].pass);
    }

    #[test]
    fn missing_file_is_hard_claim_failure() {
        let temp_dir = TempDir::new().expect("create temp dir");
        let claims = materialize_claims(
            &[ClaimTemplate::must_match(
                "missing",
                ClaimCategory::EndpointCommunication,
                "service.log",
                "Sending Response message",
                ThresholdSelector::EndpointCommunication,
            )],
            Thresholds::default(),
        );

        let outcomes = evaluate_claims(temp_dir.path(), &claims);
        assert!(!outcomes[0].pass);
        assert!(outcomes[0].error.is_some());
    }

    #[test]
    fn malformed_regex_is_hard_claim_failure() {
        let temp_dir = TempDir::new().expect("create temp dir");
        fs::write(temp_dir.path().join("client.log"), "any content\n").expect("write fixture");

        let claims = materialize_claims(
            &[ClaimTemplate::must_match(
                "bad_regex",
                ClaimCategory::EndpointCommunication,
                "client.log",
                "(unclosed",
                ThresholdSelector::Fixed(1),
            )],
            Thresholds::default(),
        );

        let outcomes = evaluate_claims(temp_dir.path(), &claims);
        assert!(!outcomes[0].pass);
        assert!(outcomes[0].error.is_some());
    }

    #[test]
    fn split_claim_outcomes_separates_forbidden_claims() {
        let temp_dir = TempDir::new().expect("create temp dir");
        fs::write(
            temp_dir.path().join("streamer.log"),
            "event=egress_send_ok\n",
        )
        .expect("write fixture");

        let claims = materialize_claims(
            &[
                ClaimTemplate::must_match(
                    "send_ok",
                    ClaimCategory::StreamerEgress,
                    "streamer.log",
                    "event=egress_send_ok",
                    ThresholdSelector::Fixed(1),
                ),
                ClaimTemplate::must_not_match(
                    "no_panic",
                    ClaimCategory::ForbiddenSignature,
                    "streamer.log",
                    "panicked at",
                ),
            ],
            Thresholds::default(),
        );

        let outcomes = evaluate_claims(temp_dir.path(), &claims);
        let (must_outcomes, forbidden_outcomes, first_failure) = split_claim_outcomes(outcomes);

        assert_eq!(must_outcomes.len(), 1);
        assert_eq!(forbidden_outcomes.len(), 1);
        assert!(first_failure.is_none());
    }
}
