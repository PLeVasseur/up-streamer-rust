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

#[cfg(all(feature = "zenoh-transport", feature = "vsomeip-transport"))]
mod config;

#[cfg(all(feature = "zenoh-transport", feature = "vsomeip-transport"))]
mod real {
    use super::config::Config;
    use clap::Parser;
    use std::{fs::File, io::Read, path::Path, sync::Arc};
    use up_rust::usubscription::USubscription;
    use up_rust::{UCode, UStatus, UUri};
    use up_streamer::{OwnedFrameEndpoint, UStreamer};
    use up_transport_vsomeip::UPTransportVsomeip;
    use up_transport_zenoh::{zenoh_config::Config as ZenohConfig, UPTransportZenoh};
    use usubscription_static_file::USubscriptionStaticFile;

    #[derive(Parser, Debug)]
    #[command(version, about, long_about = None)]
    struct Args {
        #[arg(long, default_value = "DEFAULT_CONFIG.json5")]
        config: String,
    }

    fn status(code: UCode, message: impl Into<String>) -> UStatus {
        UStatus::fail_with_code(code, message.into())
    }

    fn load_config(path: &str) -> Result<Config, UStatus> {
        let mut file = File::open(path)
            .map_err(|error| status(UCode::NOT_FOUND, format!("config file not found: {error}")))?;
        let mut contents = String::new();
        file.read_to_string(&mut contents)
            .map_err(|error| status(UCode::INTERNAL, format!("unable to read config: {error}")))?;
        json5::from_str(&contents).map_err(|error| {
            status(
                UCode::INVALID_ARGUMENT,
                format!("unable to parse config: {error}"),
            )
        })
    }

    pub async fn run() -> Result<(), UStatus> {
        let _ = tracing_subscriber::fmt::try_init();
        let args = Args::parse();
        let config = load_config(&args.config)?;

        let usubscription: Arc<dyn USubscription> = Arc::new(USubscriptionStaticFile::new(
            config.usubscription_config.file_path.clone(),
        ));

        let zenoh_config = ZenohConfig::from_file(&config.zenoh_transport_config.config_file)
            .map_err(|error| {
                status(
                    UCode::INVALID_ARGUMENT,
                    format!("unable to load Zenoh config file: {error}"),
                )
            })?;
        let zenoh = Arc::new(
            UPTransportZenoh::builder(config.streamer_uuri.authority.clone())?
                .with_config(zenoh_config)
                .build()
                .await?,
        );

        let someip_uri = UUri::try_from_parts(
            &config.someip_config.authority,
            config.streamer_uuri.ue_id,
            config.streamer_uuri.ue_version_major,
            0,
        )
        .map_err(|error| {
            status(
                UCode::INVALID_ARGUMENT,
                format!("invalid SOME/IP URI: {error}"),
            )
        })?;
        let someip = Arc::new(UPTransportVsomeip::new_with_config(
            someip_uri,
            &config.streamer_uuri.authority,
            Path::new(&config.someip_config.config_file),
            None,
        )?);

        let zenoh_endpoint =
            OwnedFrameEndpoint::from_owned("zenoh", &config.streamer_uuri.authority, zenoh);
        let someip_endpoint =
            OwnedFrameEndpoint::from_owned("someip", &config.someip_config.authority, someip);

        let mut streamer = UStreamer::new(
            "zenoh-someip-streamer",
            config.up_streamer_config.message_queue_size,
            usubscription,
        )
        .await?;
        streamer
            .add_route_ref(&zenoh_endpoint, &someip_endpoint)
            .await?;
        streamer
            .add_route_ref(&someip_endpoint, &zenoh_endpoint)
            .await?;

        println!("READY streamer_initialized");
        tokio::signal::ctrl_c().await.map_err(|error| {
            status(
                UCode::INTERNAL,
                format!("unable to wait for shutdown signal: {error}"),
            )
        })
    }
}

#[cfg(all(feature = "zenoh-transport", feature = "vsomeip-transport"))]
#[tokio::main]
async fn main() -> Result<(), up_rust::UStatus> {
    real::run().await
}

#[cfg(not(all(feature = "zenoh-transport", feature = "vsomeip-transport")))]
fn main() -> Result<(), up_rust::UStatus> {
    Err(up_rust::UStatus::fail_with_code(
        up_rust::UCode::INVALID_ARGUMENT,
        "zenoh_someip requires features 'zenoh-transport' and 'vsomeip-transport'",
    ))
}
