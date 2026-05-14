mod cli;

use std::{str::FromStr, sync::Arc, time::Duration};

use async_trait::async_trait;
use clap::Parser;
use tokio::sync::mpsc;
use up_rust::{
    RawBytes, UFrameHeader, UMessageType, UOwnedFrame, UOwnedListener, UOwnedTransport,
    UOwnedTransportExt, UStatus, UUri,
};
use up_transport_zenoh::{
    zenoh_config::{Config, EndPoint},
    UPTransportZenoh,
};

const DEFAULT_ENDPOINT: &str = "tcp/127.0.0.1:7447";
const DEFAULT_UAUTHORITY: &str = "authority-a";
const DEFAULT_TARGET_AUTHORITY: &str = "authority-b";
const DEFAULT_UENTITY: &str = "0x4210";
const DEFAULT_UVERSION: &str = "0x1";
const DEFAULT_RESOURCE: &str = "0x8001";

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
pub(crate) struct NativeExampleArgs {
    #[arg(short, long, default_value = DEFAULT_ENDPOINT)]
    endpoint: String,
    #[arg(long, default_value = DEFAULT_UAUTHORITY)]
    uauthority: String,
    #[arg(long, default_value = DEFAULT_TARGET_AUTHORITY)]
    target_authority: String,
    #[arg(long, default_value = DEFAULT_UENTITY)]
    uentity: String,
    #[arg(long, default_value = DEFAULT_UVERSION)]
    uversion: String,
    #[arg(long, default_value = DEFAULT_RESOURCE)]
    resource: String,
    #[arg(long, default_value_t = 1)]
    send_count: u64,
    #[arg(long, default_value_t = 1)]
    receive_count: u64,
    #[arg(long, default_value_t = 1000)]
    interval_ms: u64,
}

fn topic(args: &NativeExampleArgs, authority: &str) -> Result<UUri, UStatus> {
    cli::build_uuri(
        authority,
        cli::parse_u32_status("--uentity", &args.uentity)?,
        cli::parse_u8_status("--uversion", &args.uversion)?,
        cli::parse_u16_status("--resource", &args.resource)?,
    )
}

fn wildcard(authority: &str) -> UUri {
    UUri {
        authority_name: authority.to_string(),
        ue_id: 0xFFFF_FFFF,
        ue_version_major: 0xFF,
        resource_id: 0xFFFF,
    }
}

async fn transport(args: &NativeExampleArgs) -> Result<Arc<dyn UOwnedTransport>, UStatus> {
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
        UPTransportZenoh::builder(args.uauthority.clone())?
            .with_config(zenoh_config)
            .build()
            .await?,
    ))
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
            frame.header().attributes().message_type(),
            String::from_utf8_lossy(frame.payload_bytes())
        );

        if let Some(responder) = self.responder.as_ref() {
            if frame.header().attributes().message_type() == UMessageType::Request {
                let Some(reply_to) = frame.header().sink().cloned() else {
                    let _ = self.tx.send(frame);
                    return;
                };
                let response = UOwnedFrame::from_serializable::<RawBytes, _>(
                    UFrameHeader::response(
                        reply_to,
                        frame.header().attributes().id().clone(),
                        frame.header().source().clone(),
                    ),
                    &&b"native response"[..],
                )
                .expect("raw response serialization should not fail");
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
    let transport = transport(&args).await?;
    let source = topic(&args, &args.uauthority)?;

    println!("READY {role}");
    for count in 0..args.send_count {
        let payload = format!("{role} native publish #{count}");
        transport
            .send_serialized::<RawBytes, _>(
                UFrameHeader::publish(source.clone()),
                &payload.as_bytes(),
            )
            .await?;
        tokio::time::sleep(Duration::from_millis(args.interval_ms)).await;
    }
    Ok(())
}

pub async fn run_subscriber(role: &'static str) -> Result<(), UStatus> {
    let args = NativeExampleArgs::parse();
    let transport = transport(&args).await?;
    let (tx, rx) = mpsc::unbounded_channel();
    transport
        .register_owned_listener(
            &topic(&args, &args.target_authority)?,
            None,
            Arc::new(PrintListener {
                role,
                tx,
                responder: None,
            }),
        )
        .await?;
    println!("READY {role}");
    receive_loop(role, rx, args.receive_count).await
}

pub async fn run_client(role: &'static str) -> Result<(), UStatus> {
    let args = NativeExampleArgs::parse();
    let transport = transport(&args).await?;
    let reply_to = topic(&args, &args.uauthority)?;
    let method = topic(&args, &args.target_authority)?;
    let (tx, rx) = mpsc::unbounded_channel();
    transport
        .register_owned_listener(
            &wildcard(&args.target_authority),
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
                UFrameHeader::request(method.clone(), reply_to.clone(), 5_000),
                &payload.as_bytes(),
            )
            .await?;
        tokio::time::sleep(Duration::from_millis(args.interval_ms)).await;
    }
    receive_loop(role, rx, args.receive_count).await
}

pub async fn run_service(role: &'static str) -> Result<(), UStatus> {
    let args = NativeExampleArgs::parse();
    let transport = transport(&args).await?;
    let (tx, rx) = mpsc::unbounded_channel();
    let method = topic(&args, &args.uauthority)?;
    transport
        .register_owned_listener(
            &wildcard(&args.target_authority),
            Some(&method),
            Arc::new(PrintListener {
                role,
                tx,
                responder: Some(transport.clone()),
            }),
        )
        .await?;
    println!("READY {role}");
    receive_loop(role, rx, args.receive_count).await
}
