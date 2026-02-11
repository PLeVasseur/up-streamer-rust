use clap::Parser;
use std::process::ExitCode;
use transport_smoke_suite::claims::{ClaimCategory, ClaimTemplate, ThresholdSelector};
use transport_smoke_suite::scenario::{run_scenario, ScenarioCliArgs};

const SCENARIO_ID: &str = "smoke-zenoh-mqtt-rr-zenoh-client-mqtt-service";

const CLAIMS: &[ClaimTemplate] = &[
    ClaimTemplate::must_match(
        "client_response_evidence",
        ClaimCategory::EndpointCommunication,
        "client.log",
        "ServiceResponseListener: Received a message|UMESSAGE_TYPE_RESPONSE",
        ThresholdSelector::EndpointCommunication,
    ),
    ClaimTemplate::must_match(
        "service_request_response_evidence",
        ClaimCategory::EndpointCommunication,
        "service.log",
        "ServiceResponseListener: Received a message|Sending Response message",
        ThresholdSelector::EndpointCommunication,
    ),
    ClaimTemplate::must_match(
        "streamer_egress_send_attempt",
        ClaimCategory::StreamerEgress,
        "streamer.log",
        "event=egress_send_attempt",
        ThresholdSelector::EgressSendAttempt,
    ),
    ClaimTemplate::must_match(
        "streamer_egress_send_ok",
        ClaimCategory::StreamerEgress,
        "streamer.log",
        "event=egress_send_ok",
        ThresholdSelector::EgressSendOk,
    ),
    ClaimTemplate::must_match(
        "streamer_egress_worker_create_or_reuse",
        ClaimCategory::StreamerEgress,
        "streamer.log",
        "event=(egress_worker_create|egress_worker_reuse).*route_label=",
        ThresholdSelector::EgressWorkerCreateOrReuse,
    ),
    ClaimTemplate::must_not_match(
        "streamer_no_panic",
        ClaimCategory::ForbiddenSignature,
        "streamer.log",
        "panicked at",
    ),
    ClaimTemplate::must_not_match(
        "streamer_no_egress_send_failed",
        ClaimCategory::ForbiddenSignature,
        "streamer.log",
        "event=egress_send_failed",
    ),
    ClaimTemplate::must_not_match(
        "client_no_panic",
        ClaimCategory::ForbiddenSignature,
        "client.log",
        "panicked at",
    ),
    ClaimTemplate::must_not_match(
        "service_no_panic",
        ClaimCategory::ForbiddenSignature,
        "service.log",
        "panicked at",
    ),
];

#[derive(Debug, Parser)]
#[command(name = SCENARIO_ID)]
#[command(about = "Deterministic smoke scenario: zenoh client to mqtt service")]
struct Cli {
    #[command(flatten)]
    common: ScenarioCliArgs,
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();

    match run_scenario(SCENARIO_ID, CLAIMS, cli.common).await {
        Ok(result) => {
            if result.pass {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            }
        }
        Err(error) => {
            eprintln!("{SCENARIO_ID} failed: {error:#}");
            ExitCode::from(2)
        }
    }
}
