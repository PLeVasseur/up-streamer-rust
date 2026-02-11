use std::fs;
use std::path::PathBuf;
use transport_smoke_suite::scenario;

#[test]
fn scenario_registry_contains_exactly_eight_ids() {
    let scenario_ids = scenario::scenario_ids();
    assert_eq!(scenario_ids.len(), 8);
}

#[test]
fn each_scenario_binary_has_endpoint_egress_and_forbidden_claims() {
    for scenario_id in scenario::scenario_ids() {
        let source_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("bin")
            .join(format!("{scenario_id}.rs"));

        let source = fs::read_to_string(&source_path)
            .unwrap_or_else(|_| panic!("unable to read {}", source_path.display()));

        assert!(
            source.contains("const CLAIMS"),
            "{} must define a top-level CLAIMS section",
            source_path.display()
        );
        assert!(
            source.contains("ClaimCategory::EndpointCommunication"),
            "{} must include at least one endpoint communication claim",
            source_path.display()
        );
        assert!(
            source.contains("ClaimCategory::StreamerEgress"),
            "{} must include at least one streamer egress claim",
            source_path.display()
        );
        assert!(
            source.contains("ClaimCategory::ForbiddenSignature"),
            "{} must include at least one forbidden signature claim",
            source_path.display()
        );
    }
}
