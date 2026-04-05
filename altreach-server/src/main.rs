mod capture;
mod encoder;
mod input;
mod server;
mod clipboard;

use anyhow::Result;
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    rustls::crypto::ring::default_provider().install_default().expect("Failed to install crypto provider");
    tracing_subscriber::fmt::init();
    info!("altreach-server starting...");
    dotenvy::dotenv().ok();
    let addr = std::env::var("BIND_ADDR").expect("BIND_ADDR is not set");
    let password = std::env::var("PASSWORD").expect("PASSWORD is not set");
    server::run(&addr, password).await?;
    Ok(())
}
