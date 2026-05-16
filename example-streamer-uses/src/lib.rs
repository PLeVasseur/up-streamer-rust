mod cli;

#[cfg(feature = "zenoh-transport")]
use std::str::FromStr;
use std::{path::PathBuf, sync::Arc, time::Duration};

use async_trait::async_trait;
use clap::Parser;
use tokio::sync::mpsc;
use up_rust::{
    wire::RawBytes, UCode, UFrameMetadata, UMessageType, UOwnedFrame, UOwnedListener,
    UOwnedTransport, UOwnedTransportExt, UStatus, UUri,
};
#[cfg(feature = "mqtt-transport")]
use up_transport_mqtt5::{Mqtt5Transport, Mqtt5TransportOptions};
#[cfg(feature = "vsomeip-transport")]
use up_transport_vsomeip::UPTransportVsomeip;
#[cfg(feature = "zenoh-transport")]
use up_transport_zenoh::{
    zenoh_config::{Config, EndPoint},
    UPTransportZenoh,
};

const DEFAULT_ZENOH_ENDPOINT: &str = "tcp/127.0.0.1:7447";
const DEFAULT_ZENOH_AUTHORITY: &str = "authority-b";
const DEFAULT_EDGE_AUTHORITY: &str = "authority-a";
const DEFAULT_CLIENT_UENTITY: u32 = 0x5678;
const DEFAULT_CLIENT_RESOURCE: u16 = 0x0000;
const DEFAULT_SERVICE_UENTITY: u32 = 0x4321;
const DEFAULT_ZENOH_PUBLISHER_UENTITY: u32 = 0x3039;
const DEFAULT_EDGE_PUBLISHER_UENTITY: u32 = 0x5BA0;
const DEFAULT_UVERSION: u8 = 0x1;
const DEFAULT_RESOURCE: u16 = 0x8001;
const DEFAULT_BROKER_URI: &str = "localhost:1883";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExampleTransport {
    Zenoh,
    Mqtt,
    Vsomeip,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExampleRole {
    Publisher,
    Subscriber,
    Client,
    Service,
}

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
pub(crate) struct NativeExampleArgs {
    #[arg(short, long, default_value = DEFAULT_ZENOH_ENDPOINT)]
    endpoint: String,
    #[arg(long)]
    uauthority: Option<String>,
    #[arg(long)]
    target_authority: Option<String>,
    #[arg(long)]
    source_authority: Option<String>,
    #[arg(long)]
    uentity: Option<String>,
    #[arg(long)]
    uversion: Option<String>,
    #[arg(long)]
    resource: Option<String>,
    #[arg(long)]
    source_uentity: Option<String>,
    #[arg(long)]
    source_uversion: Option<String>,
    #[arg(long)]
    source_resource: Option<String>,
    #[arg(long, default_value = DEFAULT_BROKER_URI)]
    broker_uri: String,
    #[arg(long)]
    mqtt_client_id: Option<String>,
    #[arg(long)]
    vsomeip_config: Option<PathBuf>,
    #[arg(long)]
    remote_authority: Option<String>,
    #[arg(long, default_value_t = 1)]
    send_count: u64,
    #[arg(long)]
    receive_count: Option<u64>,
    #[arg(long, alias = "send-interval-ms", default_value_t = 1000)]
    interval_ms: u64,
}

fn transport_from_role(role: &str) -> Result<ExampleTransport, UStatus> {
    if role.starts_with("mqtt_") {
        Ok(ExampleTransport::Mqtt)
    } else if role.starts_with("someip_") {
        Ok(ExampleTransport::Vsomeip)
    } else if role.starts_with("zenoh_") {
        Ok(ExampleTransport::Zenoh)
    } else {
        Err(cli::invalid_argument_status(format!(
            "unable to infer transport family from role '{role}'"
        )))
    }
}

fn role_from_name(role: &str) -> Result<ExampleRole, UStatus> {
    if role.contains("publisher") {
        Ok(ExampleRole::Publisher)
    } else if role.contains("subscriber") {
        Ok(ExampleRole::Subscriber)
    } else if role.contains("client") {
        Ok(ExampleRole::Client)
    } else if role.contains("service") {
        Ok(ExampleRole::Service)
    } else {
        Err(cli::invalid_argument_status(format!(
            "unable to infer endpoint role from '{role}'"
        )))
    }
}

fn default_local_authority(transport: ExampleTransport) -> &'static str {
    match transport {
        ExampleTransport::Zenoh => DEFAULT_ZENOH_AUTHORITY,
        ExampleTransport::Mqtt | ExampleTransport::Vsomeip => DEFAULT_EDGE_AUTHORITY,
    }
}

fn default_remote_authority(transport: ExampleTransport) -> &'static str {
    match transport {
        ExampleTransport::Zenoh => DEFAULT_EDGE_AUTHORITY,
        ExampleTransport::Mqtt | ExampleTransport::Vsomeip => DEFAULT_ZENOH_AUTHORITY,
    }
}

fn default_publisher_uentity(authority: &str) -> u32 {
    if authority == DEFAULT_ZENOH_AUTHORITY {
        DEFAULT_ZENOH_PUBLISHER_UENTITY
    } else {
        DEFAULT_EDGE_PUBLISHER_UENTITY
    }
}

fn local_authority(args: &NativeExampleArgs, transport: ExampleTransport) -> String {
    args.uauthority
        .clone()
        .unwrap_or_else(|| default_local_authority(transport).to_string())
}

fn target_authority(args: &NativeExampleArgs, transport: ExampleTransport) -> String {
    args.target_authority
        .clone()
        .unwrap_or_else(|| default_remote_authority(transport).to_string())
}

fn source_authority(args: &NativeExampleArgs, transport: ExampleTransport) -> String {
    args.source_authority
        .clone()
        .unwrap_or_else(|| default_remote_authority(transport).to_string())
}

#[cfg(feature = "vsomeip-transport")]
fn remote_authority(args: &NativeExampleArgs, transport: ExampleTransport) -> String {
    args.remote_authority
        .clone()
        .or_else(|| args.target_authority.clone())
        .unwrap_or_else(|| default_remote_authority(transport).to_string())
}

fn parse_u32_or_default(flag: &str, value: Option<&String>, default: u32) -> Result<u32, UStatus> {
    value.map_or(Ok(default), |raw| cli::parse_u32_status(flag, raw))
}

fn parse_u16_or_default(flag: &str, value: Option<&String>, default: u16) -> Result<u16, UStatus> {
    value.map_or(Ok(default), |raw| cli::parse_u16_status(flag, raw))
}

fn parse_u8_or_default(flag: &str, value: Option<&String>, default: u8) -> Result<u8, UStatus> {
    value.map_or(Ok(default), |raw| cli::parse_u8_status(flag, raw))
}

fn build_uuri(authority: &str, uentity: u32, uversion: u8, resource: u16) -> Result<UUri, UStatus> {
    cli::build_uuri(authority, uentity, uversion, resource)
}

fn wildcard(authority: &str) -> UUri {
    UUri::try_from_parts(authority, 0xFFFF_FFFF, 0xFF, 0xFFFF).expect("valid wildcard filter")
}

fn publish_topic(args: &NativeExampleArgs, transport: ExampleTransport) -> Result<UUri, UStatus> {
    let authority = local_authority(args, transport);
    build_uuri(
        &authority,
        parse_u32_or_default(
            "--uentity",
            args.uentity.as_ref(),
            default_publisher_uentity(&authority),
        )?,
        parse_u8_or_default("--uversion", args.uversion.as_ref(), DEFAULT_UVERSION)?,
        parse_u16_or_default("--resource", args.resource.as_ref(), DEFAULT_RESOURCE)?,
    )
}

fn subscriber_source_filter(
    args: &NativeExampleArgs,
    transport: ExampleTransport,
) -> Result<UUri, UStatus> {
    let authority = source_authority(args, transport);
    build_uuri(
        &authority,
        parse_u32_or_default(
            "--source-uentity",
            args.source_uentity.as_ref(),
            default_publisher_uentity(&authority),
        )?,
        parse_u8_or_default(
            "--source-uversion",
            args.source_uversion.as_ref(),
            DEFAULT_UVERSION,
        )?,
        parse_u16_or_default(
            "--source-resource",
            args.source_resource.as_ref(),
            DEFAULT_RESOURCE,
        )?,
    )
}

fn client_reply_to(args: &NativeExampleArgs, transport: ExampleTransport) -> Result<UUri, UStatus> {
    let authority = local_authority(args, transport);
    build_uuri(
        &authority,
        parse_u32_or_default("--uentity", args.uentity.as_ref(), DEFAULT_CLIENT_UENTITY)?,
        parse_u8_or_default("--uversion", args.uversion.as_ref(), DEFAULT_UVERSION)?,
        parse_u16_or_default(
            "--resource",
            args.resource.as_ref(),
            DEFAULT_CLIENT_RESOURCE,
        )?,
    )
}

fn service_method_for_authority(
    args: &NativeExampleArgs,
    authority: &str,
) -> Result<UUri, UStatus> {
    build_uuri(
        authority,
        parse_u32_or_default("--uentity", args.uentity.as_ref(), DEFAULT_SERVICE_UENTITY)?,
        parse_u8_or_default("--uversion", args.uversion.as_ref(), DEFAULT_UVERSION)?,
        parse_u16_or_default("--resource", args.resource.as_ref(), DEFAULT_RESOURCE)?,
    )
}

fn client_method(args: &NativeExampleArgs, transport: ExampleTransport) -> Result<UUri, UStatus> {
    let authority = target_authority(args, transport);
    service_method_for_authority(args, &authority)
}

fn service_method(args: &NativeExampleArgs, transport: ExampleTransport) -> Result<UUri, UStatus> {
    let authority = local_authority(args, transport);
    service_method_for_authority(args, &authority)
}

#[cfg(feature = "vsomeip-transport")]
fn local_transport_uri(
    args: &NativeExampleArgs,
    transport: ExampleTransport,
    role: ExampleRole,
) -> Result<UUri, UStatus> {
    let authority = local_authority(args, transport);
    let default_uentity = match role {
        ExampleRole::Publisher => default_publisher_uentity(&authority),
        ExampleRole::Subscriber | ExampleRole::Client => DEFAULT_CLIENT_UENTITY,
        ExampleRole::Service => DEFAULT_SERVICE_UENTITY,
    };
    build_uuri(
        &authority,
        parse_u32_or_default("--uentity", args.uentity.as_ref(), default_uentity)?,
        parse_u8_or_default("--uversion", args.uversion.as_ref(), DEFAULT_UVERSION)?,
        0,
    )
}

#[cfg(feature = "mqtt-transport")]
fn normalize_mqtt_uri(uri: &str) -> String {
    if uri.contains("://") {
        uri.to_string()
    } else {
        format!("mqtt://{uri}")
    }
}

#[cfg(feature = "vsomeip-transport")]
fn default_vsomeip_config(role: ExampleRole) -> PathBuf {
    let file_name = match role {
        ExampleRole::Publisher => "someip_publisher.json",
        ExampleRole::Subscriber => "someip_subscriber.json",
        ExampleRole::Client => "someip_client.json",
        ExampleRole::Service => "someip_service.json",
    };
    PathBuf::from("example-streamer-uses")
        .join("vsomeip-configs")
        .join(file_name)
}

#[cfg(feature = "vsomeip-transport")]
fn resolve_vsomeip_config_path(path: PathBuf) -> PathBuf {
    if path.exists() {
        return path;
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let relative_to_manifest = path
        .strip_prefix("example-streamer-uses")
        .map_or_else(|_| path.clone(), PathBuf::from);
    let manifest_path = manifest_dir.join(relative_to_manifest);
    if manifest_path.exists() {
        manifest_path
    } else {
        path
    }
}

async fn transport(
    args: &NativeExampleArgs,
    family: ExampleTransport,
    role: ExampleRole,
    role_name: &str,
) -> Result<Arc<dyn UOwnedTransport>, UStatus> {
    let _ = args;
    let _ = role;
    match family {
        ExampleTransport::Zenoh => {
            #[cfg(feature = "zenoh-transport")]
            {
                let mut zenoh_config = Config::default();
                if !args.endpoint.is_empty() {
                    let endpoint = EndPoint::from_str(args.endpoint.as_str()).map_err(|error| {
                        cli::invalid_argument_status(format!("invalid Zenoh endpoint: {error}"))
                    })?;
                    zenoh_config
                        .connect
                        .endpoints
                        .set(vec![endpoint])
                        .map_err(|error| cli::invalid_argument_status(format!("{error:?}")))?;
                }

                Ok(Arc::new(
                    UPTransportZenoh::builder(local_authority(args, family))?
                        .with_config(zenoh_config)
                        .build()
                        .await?,
                ))
            }
            #[cfg(not(feature = "zenoh-transport"))]
            {
                Err(cli::invalid_argument_status(format!(
                    "{role_name} requires feature 'zenoh-transport'"
                )))
            }
        }
        ExampleTransport::Mqtt => {
            #[cfg(feature = "mqtt-transport")]
            {
                let mut options = Mqtt5TransportOptions::default();
                options.mqtt_client_options.broker_uri = normalize_mqtt_uri(&args.broker_uri);
                options.mqtt_client_options.client_id = args
                    .mqtt_client_id
                    .clone()
                    .or_else(|| Some(format!("{role_name}-{}", std::process::id())));
                let transport =
                    Arc::new(Mqtt5Transport::new(options, local_authority(args, family)).await?);
                transport.connect().await?;
                Ok(transport)
            }
            #[cfg(not(feature = "mqtt-transport"))]
            {
                Err(cli::invalid_argument_status(format!(
                    "{role_name} requires feature 'mqtt-transport'"
                )))
            }
        }
        ExampleTransport::Vsomeip => {
            #[cfg(feature = "vsomeip-transport")]
            {
                let local_uri = local_transport_uri(args, family, role)?;
                let config_path = resolve_vsomeip_config_path(
                    args.vsomeip_config
                        .clone()
                        .unwrap_or_else(|| default_vsomeip_config(role)),
                );
                Ok(Arc::new(UPTransportVsomeip::new_with_config(
                    local_uri,
                    &remote_authority(args, family),
                    &config_path,
                    None,
                )?))
            }
            #[cfg(not(feature = "vsomeip-transport"))]
            {
                Err(cli::invalid_argument_status(format!(
                    "{role_name} requires feature 'vsomeip-transport'"
                )))
            }
        }
    }
}

struct PrintListener {
    role: &'static str,
    tx: mpsc::UnboundedSender<UOwnedFrame>,
    responder: Option<Arc<dyn UOwnedTransport>>,
}

#[async_trait]
impl UOwnedListener for PrintListener {
    async fn on_receive_owned(&self, frame: UOwnedFrame) {
        println!(
            "{} received {:?}: {}",
            self.role,
            frame.metadata().attributes().message_type(),
            String::from_utf8_lossy(frame.payload_bytes())
        );

        match frame.metadata().attributes().message_type() {
            UMessageType::Publish => println!("PublishReceiver: Received a message"),
            UMessageType::Response => {
                println!("ServiceResponseListener: Received a message");
                println!(
                    "commstatus: {:?}",
                    frame.metadata().attributes().commstatus()
                );
            }
            _ => {}
        }

        if let Some(responder) = self.responder.as_ref() {
            if frame.metadata().attributes().message_type() == UMessageType::Request {
                let reply_to = frame.metadata().source().clone();
                let Some(invoked_method) = frame.metadata().sink().cloned() else {
                    let _ = self.tx.send(frame);
                    return;
                };
                let response_metadata = UFrameMetadata::response(
                    reply_to,
                    frame.metadata().attributes().id().clone(),
                    invoked_method,
                );
                let response_metadata = UFrameMetadata::new(
                    response_metadata
                        .attributes()
                        .clone()
                        .with_comm_status(UCode::OK),
                    response_metadata.encoding().cloned(),
                );
                let response = UOwnedFrame::from_serializable::<RawBytes, _>(
                    response_metadata,
                    &&b"native response"[..],
                )
                .expect("raw response serialization should not fail");
                println!("Sending Response message");
                let _ = responder.send_owned(response).await;
            }
        }

        let _ = self.tx.send(frame);
    }
}

async fn receive_loop(
    role: &'static str,
    mut rx: mpsc::UnboundedReceiver<UOwnedFrame>,
    receive_count: u64,
) -> Result<(), UStatus> {
    if receive_count == 0 {
        tokio::signal::ctrl_c()
            .await
            .map_err(|error| cli::invalid_argument_status(error.to_string()))?;
        return Ok(());
    }

    for received in 0..receive_count {
        let _ = rx.recv().await.ok_or_else(|| {
            cli::invalid_argument_status(format!("{role} listener channel closed unexpectedly"))
        })?;
        println!(
            "{role} completed receive {}/{}",
            received + 1,
            receive_count
        );
    }
    Ok(())
}

pub async fn run_publisher(role: &'static str) -> Result<(), UStatus> {
    let args = NativeExampleArgs::parse();
    let family = transport_from_role(role)?;
    let endpoint_role = role_from_name(role)?;
    let transport = transport(&args, family, endpoint_role, role).await?;
    let source = publish_topic(&args, family)?;

    println!("READY {role}");
    for count in 0..args.send_count {
        let payload = format!("{role} native publish #{count}");
        println!("Sending Publish message");
        transport
            .send_serialized::<RawBytes, _>(
                UFrameMetadata::publish(source.clone()),
                &payload.as_bytes(),
            )
            .await?;
        tokio::time::sleep(Duration::from_millis(args.interval_ms)).await;
    }
    Ok(())
}

pub async fn run_subscriber(role: &'static str) -> Result<(), UStatus> {
    let args = NativeExampleArgs::parse();
    let family = transport_from_role(role)?;
    let endpoint_role = role_from_name(role)?;
    let transport = transport(&args, family, endpoint_role, role).await?;
    let (tx, rx) = mpsc::unbounded_channel();
    transport
        .register_owned_listener(
            &subscriber_source_filter(&args, family)?,
            None,
            Arc::new(PrintListener {
                role,
                tx,
                responder: None,
            }),
        )
        .await?;
    println!("READY listener_registered");
    receive_loop(role, rx, args.receive_count.unwrap_or(0)).await
}

pub async fn run_client(role: &'static str) -> Result<(), UStatus> {
    let args = NativeExampleArgs::parse();
    let family = transport_from_role(role)?;
    let endpoint_role = role_from_name(role)?;
    let transport = transport(&args, family, endpoint_role, role).await?;
    let reply_to = client_reply_to(&args, family)?;
    let method = client_method(&args, family)?;
    let (tx, rx) = mpsc::unbounded_channel();
    transport
        .register_owned_listener(
            &method,
            Some(&reply_to),
            Arc::new(PrintListener {
                role,
                tx,
                responder: None,
            }),
        )
        .await?;

    println!("READY {role}");
    for count in 0..args.send_count {
        let payload = format!("{role} native request #{count}");
        transport
            .send_serialized::<RawBytes, _>(
                UFrameMetadata::request(method.clone(), reply_to.clone(), 5_000),
                &payload.as_bytes(),
            )
            .await?;
        tokio::time::sleep(Duration::from_millis(args.interval_ms)).await;
    }
    receive_loop(role, rx, args.receive_count.unwrap_or(args.send_count)).await
}

pub async fn run_service(role: &'static str) -> Result<(), UStatus> {
    let args = NativeExampleArgs::parse();
    let family = transport_from_role(role)?;
    let endpoint_role = role_from_name(role)?;
    let transport = transport(&args, family, endpoint_role, role).await?;
    let (tx, rx) = mpsc::unbounded_channel();
    let method = service_method(&args, family)?;
    let remote = target_authority(&args, family);
    transport
        .register_owned_listener(
            &wildcard(&remote),
            Some(&method),
            Arc::new(PrintListener {
                role,
                tx,
                responder: Some(transport.clone()),
            }),
        )
        .await?;
    println!("READY listener_registered");
    receive_loop(role, rx, args.receive_count.unwrap_or(0)).await
}
