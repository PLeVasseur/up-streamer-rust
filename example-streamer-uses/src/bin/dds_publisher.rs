// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! DDS publisher role binary.

use example_streamer_uses::common::dds;

use clap::Parser;
use dds::{Encoding, NativeContext, ReliabilityArg, Role, RouteFamily};
use up_rust::UStatus;

#[derive(Debug, Parser)]
#[command(version, about = "Dust DDS uProtocol Streamer role process")]
struct Args {
    #[arg(skip)]
    native: NativeContext,
    /// Controller-provisioned sustained workload; uses this role's normal binding configuration.
    #[cfg(feature = "sustained-workload")]
    #[arg(long)]
    workload_plan: Option<std::path::PathBuf>,
    /// DDS domain used for discovery and RTPS traffic.
    #[arg(long, default_value_t = 80)]
    domain_id: i32,
    /// Unique local transport identity used for self-origin suppression.
    #[arg(long)]
    origin_id: Option<String>,
    /// DDS reliability policy.
    #[arg(long, value_enum, default_value = "reliable")]
    reliability: ReliabilityArg,
    /// DDS KEEP_LAST history depth.
    #[arg(long, default_value_t = 32)]
    history_depth: u32,
    /// DDS carriage family.
    #[arg(long, value_enum, default_value = "classic")]
    route_family: RouteFamily,
    /// Selected payload/metadata encoding.
    #[arg(long, value_enum, default_value = "protobuf")]
    encoding: Encoding,
    /// Authority of this role.
    #[arg(long, default_value = "authority-a")]
    local_authority: String,
    /// Authority of the peer role.
    #[arg(long, default_value = "authority-b")]
    peer_authority: String,
    /// Publish or notification resource ID.
    #[arg(long, default_value_t = 0x8001)]
    topic_resource_id: u16,
    /// RPC method resource ID.
    #[arg(long, default_value_t = 0x1000)]
    method_resource_id: u16,
    /// UTF-8 payload sent by active roles and echoed by servers.
    #[arg(long, default_value = "dds-role-payload")]
    payload: String,
    /// Receive and discovery timeout.
    #[arg(long, default_value_t = 8_000)]
    timeout_ms: u64,
}

#[tokio::main]
async fn main() -> Result<(), UStatus> {
    let mut args = Args::parse();
    let role = Role::Publisher;
    #[cfg(feature = "sustained-workload")]
    if args.workload_plan.is_some()
        && (!matches!(args.route_family, RouteFamily::Classic)
            || matches!(role, Role::Notifier | Role::Notifyee))
    {
        return Err(dds::invalid(
            "sustained role support currently requires classic pub/sub or RPC",
        ));
    }
    args.native = NativeContext::from_env()?;
    if args.encoding == Encoding::Native {
        args.native.identity()?;
    }
    let inputs = dds::DdsArgs {
        native: args.native,
        domain_id: args.domain_id,
        origin_id: args.origin_id,
        reliability: args.reliability,
        history_depth: args.history_depth,
        route_family: args.route_family,
        encoding: args.encoding,
        local_authority: args.local_authority,
        peer_authority: args.peer_authority,
        topic_resource_id: args.topic_resource_id,
        method_resource_id: args.method_resource_id,
        payload: args.payload,
        timeout_ms: args.timeout_ms,
        #[cfg(feature = "sustained-workload")]
        workload_plan: args.workload_plan,
    };
    dds::run(role, &inputs).await
}
