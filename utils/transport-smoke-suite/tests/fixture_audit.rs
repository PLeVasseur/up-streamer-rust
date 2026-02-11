use std::path::PathBuf;
use transport_smoke_suite::claims::{
    evaluate_claims, materialize_claims, ClaimCategory, ClaimTemplate, ThresholdSelector,
    Thresholds,
};
use transport_smoke_suite::scenario;

#[test]
fn pass_fixtures_satisfy_scenario_claims() {
    for scenario_id in scenario::scenario_ids() {
        let claims = materialize_claims(
            &scenario_claim_templates(scenario_id),
            Thresholds::default(),
        );
        let fixture_dir = fixture_root().join(scenario_id).join("pass");
        let outcomes = evaluate_claims(&fixture_dir, &claims);

        let failed = outcomes
            .into_iter()
            .filter(|outcome| !outcome.pass)
            .collect::<Vec<_>>();
        assert!(
            failed.is_empty(),
            "pass fixture failed for {} with {:?}",
            scenario_id,
            failed
        );
    }
}

#[test]
fn fail_fixtures_trigger_claim_failures() {
    for scenario_id in scenario::scenario_ids() {
        let claims = materialize_claims(
            &scenario_claim_templates(scenario_id),
            Thresholds::default(),
        );
        let fixture_dir = fixture_root()
            .join(scenario_id)
            .join("fail-missing-egress-ok");
        let outcomes = evaluate_claims(&fixture_dir, &claims);

        assert!(
            outcomes.iter().any(|outcome| !outcome.pass),
            "expected at least one failing claim for {}",
            scenario_id
        );
    }
}

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

fn scenario_claim_templates(scenario_id: &str) -> Vec<ClaimTemplate> {
    let mut claims = Vec::new();

    if scenario_id.contains("-rr-") {
        claims.push(ClaimTemplate::must_match(
            "client_response_evidence",
            ClaimCategory::EndpointCommunication,
            "client.log",
            "ServiceResponseListener: Received a message|UMESSAGE_TYPE_RESPONSE|commstatus: Some\\(OK\\)",
            ThresholdSelector::EndpointCommunication,
        ));
        claims.push(ClaimTemplate::must_match(
            "service_request_response_evidence",
            ClaimCategory::EndpointCommunication,
            "service.log",
            "ServiceResponseListener: Received a message|Sending Response message",
            ThresholdSelector::EndpointCommunication,
        ));
    } else {
        claims.push(ClaimTemplate::must_match(
            "publisher_sent_messages",
            ClaimCategory::EndpointCommunication,
            "publisher.log",
            "Sending Publish message",
            ThresholdSelector::EndpointCommunication,
        ));
        claims.push(ClaimTemplate::must_match(
            "subscriber_received_messages",
            ClaimCategory::EndpointCommunication,
            "subscriber.log",
            "PublishReceiver: Received a message",
            ThresholdSelector::EndpointCommunication,
        ));
    }

    claims.push(ClaimTemplate::must_match(
        "streamer_egress_send_attempt",
        ClaimCategory::StreamerEgress,
        "streamer.log",
        "egress_send_attempt",
        ThresholdSelector::EgressSendAttempt,
    ));
    claims.push(ClaimTemplate::must_match(
        "streamer_egress_send_ok",
        ClaimCategory::StreamerEgress,
        "streamer.log",
        "egress_send_ok",
        ThresholdSelector::EgressSendOk,
    ));
    claims.push(ClaimTemplate::must_match(
        "streamer_egress_worker_create_or_reuse",
        ClaimCategory::StreamerEgress,
        "streamer.log",
        "egress_worker_create|egress_worker_reuse",
        ThresholdSelector::EgressWorkerCreateOrReuse,
    ));

    claims.push(ClaimTemplate::must_not_match(
        "streamer_no_panic",
        ClaimCategory::ForbiddenSignature,
        "streamer.log",
        "panicked at",
    ));
    claims.push(ClaimTemplate::must_not_match(
        "streamer_no_egress_send_failed",
        ClaimCategory::ForbiddenSignature,
        "streamer.log",
        "egress_send_failed",
    ));

    if scenario_id.contains("someip") {
        claims.push(ClaimTemplate::must_not_match(
            "streamer_no_missing_route_info",
            ClaimCategory::ForbiddenSignature,
            "streamer.log",
            "Routing info for remote service could not be found",
        ));
    }

    claims
}
