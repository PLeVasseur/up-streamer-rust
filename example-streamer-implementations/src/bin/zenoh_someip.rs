/********************************************************************************
 * Copyright (c) 2024 Contributors to the Eclipse Foundation
 *
 * See the NOTICE file(s) distributed with this work for additional
 * information regarding copyright ownership.
 *
 * This program and the accompanying materials are made available under the
 * terms of the Apache License Version 2.0 which is available at
 * https://www.apache.org/licenses/LICENSE-2.0
 *
 * SPDX-License-Identifier: Apache-2.0
 ********************************************************************************/

use std::sync::Arc;

use clap::Parser;
use up_rust::{UStatus, USubscription};
use up_streamer::{Endpoint, UStreamer};
use up_transport_iceoryx2_rust::{transport::UTransportIceoryx2, MessagingPattern};
use up_transport_zenoh::{zenoh_config::Config as ZenohConfig, UPTransportZenoh};
use usubscription_static_file::USubscriptionStaticFile;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(long, default_value = "native-streamer")]
    streamer_name: String,
    #[arg(long, default_value_t = 32)]
    queue_size: u16,
    #[arg(
        long,
        default_value = "example-streamer-implementations/static-subscriptions.json"
    )]
    subscriptions: String,
    #[arg(long, default_value = "zenoh-authority")]
    zenoh_authority: String,
    #[arg(long, default_value = "iceoryx2-authority")]
    iceoryx2_authority: String,
}

#[tokio::main]
async fn main() -> Result<(), UStatus> {
    let _ = tracing_subscriber::fmt::try_init();
    let args = Args::parse();
    let usubscription: Arc<dyn USubscription> =
        Arc::new(USubscriptionStaticFile::new(args.subscriptions.clone()));

    let zenoh = Arc::new(
        UPTransportZenoh::builder(args.zenoh_authority.clone())?
            .with_config(ZenohConfig::default())
            .build()
            .await?,
    );
    let iceoryx2 = UTransportIceoryx2::build_zero_copy(MessagingPattern::PublishSubscribe)?;

    let zenoh_endpoint = Endpoint::from_owned("zenoh", &args.zenoh_authority, zenoh);
    let iceoryx2_endpoint =
        Endpoint::from_zero_copy("iceoryx2", &args.iceoryx2_authority, iceoryx2);

    let mut streamer = UStreamer::new(&args.streamer_name, args.queue_size, usubscription).await?;
    streamer
        .add_route_ref(&zenoh_endpoint, &iceoryx2_endpoint)
        .await?;
    streamer
        .add_route_ref(&iceoryx2_endpoint, &zenoh_endpoint)
        .await?;

    println!("READY streamer_initialized");
    tokio::signal::ctrl_c().await.map_err(|error| {
        up_rust::UStatus::fail_with_code(
            up_rust::UCode::INTERNAL,
            format!("Unable to wait for shutdown signal: {error}"),
        )
    })
}
