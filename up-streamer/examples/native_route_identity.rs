// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Export and check a provisioned DoorState/SeatState native deployment.
use clap::{Parser, Subcommand};
use std::{path::PathBuf, sync::Arc};
use up_native_profile::{
    read_artifact, write_artifact, DeploymentManifest, InventoryArtifact, LoadedDeployment, TypeKey,
};
use up_rust::{NativeProfileMode, StableContainerPayload, StablePayload};
use up_streamer::{NativeRouteApi, NativeRouteHop, NativeRoutePolicy};

#[path = "support/native_route_types.rs"]
mod types;
use types::{DoorState, SeatState};

#[derive(Parser)]
#[command(about = "Provisional native deployment example: equal size is not type agreement")]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Export these actual target-linked types for the owning generator.
    Export {
        #[arg(long)]
        output: PathBuf,
    },
    /// Check an independently provisioned complete route before activating it.
    Check {
        #[arg(long)]
        profile_root: PathBuf,
        #[arg(long)]
        manifest: PathBuf,
        #[arg(long)]
        route: String,
        #[arg(long)]
        local: String,
        #[arg(long)]
        peer: String,
        /// Declare a classic member; other members use the owned-frame API in this example.
        #[arg(long)]
        classic_hop: Vec<String>,
        /// Explicit per-route acknowledgement; no profile/type check is skipped.
        #[arg(long)]
        allow_single_type_over_classic_hops: bool,
    },
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    match Args::parse().command {
        Command::Export { output } => {
            let inventory = InventoryArtifact::from_linked(&[
                TypeKey {
                    name: DoorState::TYPE_NAME.into(),
                    revision: DoorState::REVISION,
                },
                TypeKey {
                    name: SeatState::TYPE_NAME.into(),
                    revision: SeatState::REVISION,
                },
            ])?;
            write_artifact(&output, &inventory)?;
            println!("{}", inventory.content_sha256);
        }
        Command::Check {
            profile_root,
            manifest,
            route,
            local,
            peer,
            classic_hop,
            allow_single_type_over_classic_hops,
        } => {
            let manifest: DeploymentManifest = read_artifact(&manifest)?;
            let loaded = LoadedDeployment::load(&profile_root, manifest)?;
            loaded.require_local_types(
                &local,
                &[TypeKey {
                    name: DoorState::TYPE_NAME.into(),
                    revision: DoorState::REVISION,
                }],
            )?;
            let agreement = loaded.agreement(&route, &local, &peer)?;
            let members = &loaded
                .manifest()
                .routes
                .iter()
                .find(|value| value.id == route)
                .ok_or("missing route")?
                .endpoints;
            if classic_hop.iter().any(|member| !members.contains(member)) {
                return Err("undeclared classic member".into());
            }
            let mut hops = Vec::new();
            for name in members {
                hops.push(NativeRouteHop {
                    name: name.clone(),
                    api: if classic_hop.contains(name) {
                        NativeRouteApi::Classic
                    } else {
                        NativeRouteApi::OwnedFrame
                    },
                    profile: Arc::new(loaded.artifact(name)?.profile()?),
                });
            }
            let policy = NativeRoutePolicy::new(route, hops)
                .map_err(|error| error.to_string())?
                .with_allow_single_type_over_classic_hops(allow_single_type_over_classic_hops);
            policy.validate().map_err(|error| error.to_string())?;
            assert_eq!(
                std::mem::size_of::<DoorState>(),
                std::mem::size_of::<SeatState>()
            );
            assert_ne!(
                DoorState::native_representation(),
                SeatState::native_representation()
            );
            let door = StableContainerPayload::<DoorState>::identity(&agreement)?;
            if matches!(agreement.profile().mode(), NativeProfileMode::Table(_)) {
                let seat = StableContainerPayload::<SeatState>::identity(&agreement)?;
                assert_ne!(door.encoding(), seat.encoding());
                assert!(up_rust::payload::codec::PayloadIdentity::Native(seat)
                    .verify(Some(&door.encoding()), Some(door.token()))
                    .is_err());
                println!(
                    "TABLE_DISTINGUISHES_EQUAL_SIZE_TYPES door={} seat={}",
                    door.encoding().id(),
                    seat.encoding().id()
                );
            } else {
                assert!(StableContainerPayload::<SeatState>::identity(&agreement).is_err());
                println!(
                    "EXPLICIT_DOOR_OPERATION_CONTRACT encoding={}",
                    door.encoding().id()
                );
            }
            println!("NATIVE_ROUTE_POLICY_CHECKED allow_single_type_over_classic_hops={allow_single_type_over_classic_hops}");
        }
    }
    Ok(())
}
