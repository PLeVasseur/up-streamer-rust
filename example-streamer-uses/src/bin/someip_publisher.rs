#[tokio::main]
async fn main() -> Result<(), up_rust::UStatus> {
    let _ = tracing_subscriber::fmt::try_init();
    example_streamer_uses::run_publisher("someip_publisher_native").await
}
