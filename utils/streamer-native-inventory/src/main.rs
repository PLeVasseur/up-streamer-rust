// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Actual-target export of the native types used by Streamer and its role processes.
use clap::Parser;
use std::path::PathBuf;
use streamer_native_types::{NativeFlowPayload, SelectedWireNativePayload};
use up_native_profile::{
    read_artifact, write_artifact, AgreementArtifact, AllocationBand, IdentityDeclarations,
    InventoryArtifact, ProfileToolError, SeedAllocation, TypeKey,
};
use up_rust::StablePayload;

#[derive(Debug, Parser)]
#[command(about = "Export actual Streamer native types in the provisional target-qualified format")]
struct Args {
    /// New inventory artifact path.
    #[arg(long)]
    output: PathBuf,
    /// Require this compiler target; never relabel a host layout.
    #[arg(long)]
    expect_target: Option<String>,
    /// Also generate an agreement using the explicit deployment inputs.
    #[arg(long, requires_all = ["domain", "seeds", "identities"])]
    agreement_output: Option<PathBuf>,
    /// Deployment domain for agreement generation.
    #[arg(long, requires = "agreement_output")]
    domain: Option<String>,
    /// Explicit genesis seed reservations.
    #[arg(long, requires = "agreement_output")]
    seeds: Option<PathBuf>,
    /// Separate private payload and wire/family/layout declarations.
    #[arg(long, requires = "agreement_output")]
    identities: Option<PathBuf>,
    /// Generate SingleType for this exact linked type name and revision zero.
    #[arg(long, requires = "agreement_output")]
    single_type: Option<String>,
}

fn main() -> Result<(), ProfileToolError> {
    let args = Args::parse();
    let required = [
        TypeKey {
            name: SelectedWireNativePayload::TYPE_NAME.into(),
            revision: SelectedWireNativePayload::REVISION,
        },
        TypeKey {
            name: NativeFlowPayload::TYPE_NAME.into(),
            revision: NativeFlowPayload::REVISION,
        },
        #[cfg(feature = "payload-contract-large-fixtures")]
        TypeKey {
            name: streamer_native_types::large::LidarPointCloudHesaiAt128V1::TYPE_NAME.into(),
            revision: streamer_native_types::large::LidarPointCloudHesaiAt128V1::REVISION,
        },
        #[cfg(feature = "payload-contract-large-fixtures")]
        TypeKey {
            name: streamer_native_types::large::CameraBayerRggb12pFrame8mpV1::TYPE_NAME.into(),
            revision: streamer_native_types::large::CameraBayerRggb12pFrame8mpV1::REVISION,
        },
    ];
    let inventory = InventoryArtifact::from_linked(&required)?;
    if args
        .expect_target
        .is_some_and(|target| target != inventory.target.triple)
    {
        return Err(ProfileToolError::Invalid(
            "Streamer exporter was built for another target",
        ));
    }
    write_artifact(&args.output, &inventory)?;
    println!("{}", inventory.content_sha256);
    if let Some(output) = args.agreement_output {
        let seeds: Vec<SeedAllocation> = read_artifact(
            &args
                .seeds
                .ok_or(ProfileToolError::Invalid("missing generation seeds"))?,
        )?;
        let identities: IdentityDeclarations = read_artifact(
            &args
                .identities
                .ok_or(ProfileToolError::Invalid("missing identity declarations"))?,
        )?;
        let domain = args
            .domain
            .ok_or(ProfileToolError::Invalid("missing deployment domain"))?;
        let agreement = if let Some(name) = args.single_type {
            AgreementArtifact::generate_single_type(
                &inventory,
                domain,
                &TypeKey { name, revision: 0 },
                identities,
            )?
        } else {
            AgreementArtifact::generate_table(
                &inventory,
                domain,
                1,
                AllocationBand::default(),
                None,
                &seeds,
                identities,
            )?
        };
        write_artifact(&output, &agreement)?;
        println!("{}", agreement.content_sha256);
    }
    Ok(())
}
