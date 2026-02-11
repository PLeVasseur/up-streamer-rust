use clap::Parser;
use std::process::ExitCode;
use transport_smoke_suite::claims::{ClaimCategory, ClaimTemplate, ThresholdSelector};
use transport_smoke_suite::scenario::{run_scenario, ScenarioCliArgs};

const SCENARIO_ID: &str = "smoke-zenoh-mqtt-ps-zenoh-publisher-mqtt-subscriber";

const CLAIMS: &[ClaimTemplate] = &[
    ClaimTemplate::must_match(
        "publisher_sent_messages",
        ClaimCategory::EndpointCommunication,
        "publisher.log",
        "Sending Publish message",
        ThresholdSelector::EndpointCommunication,
    ),
    ClaimTemplate::must_match(
        "subscriber_received_messages",
        ClaimCategory::EndpointCommunication,
        "subscriber.log",
        "PublishReceiver: Received a message",
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
        "publisher_no_panic",
        ClaimCategory::ForbiddenSignature,
        "publisher.log",
        "panicked at",
    ),
    ClaimTemplate::must_not_match(
        "subscriber_no_panic",
        ClaimCategory::ForbiddenSignature,
        "subscriber.log",
        "panicked at",
    ),
];

#[derive(Debug, Parser)]
#[command(name = SCENARIO_ID)]
#[command(about = "Deterministic smoke scenario: zenoh publisher to mqtt subscriber")]
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
