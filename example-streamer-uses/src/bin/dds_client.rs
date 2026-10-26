// SPDX-License-Identifier: Apache-2.0
//! DDS client role binary.

#[path = "common/dds.rs"]
mod dds;

use clap::Parser;
use dds::{Encoding, NativeContext, ReliabilityArg, Role, RouteFamily};
use std::sync::Arc;
use up_rust::{UStatus, UWithNativePrefixWire};
use up_transport_dds::{
    encoded_owned::DdsEncodedOwnedCore, owned::UPTransportDdsOwned, DdsConfig, Reliability,
    UPTransportDds,
};

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
    let role = Role::Client;
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
    let mut config = DdsConfig::new(args.domain_id);
    if let Some(origin_id) = &args.origin_id {
        config.origin_id.clone_from(origin_id);
    }
    config.qos.reliability = match args.reliability {
        ReliabilityArg::Reliable => Reliability::Reliable,
        ReliabilityArg::BestEffort => Reliability::BestEffort,
    };
    config.qos.history_depth = args.history_depth;
    let runtime = tokio::runtime::Handle::current();
    match args.route_family {
        RouteFamily::Classic => {
            let transport = Arc::new(UPTransportDds::with_config(config, runtime)?);
            dds::run_classic(role, &args, transport).await
        }
        RouteFamily::OwnedFrame => {
            let transport = Arc::new(UPTransportDdsOwned::with_config(config, runtime)?);
            dds::run_owned(role, &args, transport).await
        }
        RouteFamily::EncodedOwned => {
            let core = DdsEncodedOwnedCore::with_config(config, runtime)?;
            match args.encoding {
                Encoding::Native => {
                    let profile = args.native.agreement()?.clone();
                    dds::run_encoded_owned(role, &args, core, |core| {
                        core.into_stable_container_transport(profile)
                    })
                    .await
                }
                Encoding::Protobuf => {
                    dds::run_encoded_owned(role, &args, core, |core| core.into_protobuf_transport())
                        .await
                }
                Encoding::Xcdrv2 => {
                    dds::run_encoded_owned(role, &args, core, |core| {
                        core.into_native_prefix_wire_transport(up_wire_xcdrv2::XcdrV2Wire)
                    })
                    .await
                }
                Encoding::Arrow => {
                    dds::run_encoded_owned(role, &args, core, |core| {
                        core.into_native_prefix_wire_transport(up_wire_arrow::ArrowWire)
                    })
                    .await
                }
                Encoding::OmgIdl => {
                    dds::run_encoded_owned(role, &args, core, |core| {
                        core.into_native_prefix_wire_transport(up_wire_omgidl::OmgIdlWire)
                    })
                    .await
                }
            }
        }
    }
}
