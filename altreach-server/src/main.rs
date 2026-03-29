mod capture;
mod encoder;
mod input;
mod server;
mod clipboard;

use anyhow::Result;
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    info!("altreach-server starting...");
    dotenvy::dotenv().ok();
    let addr = std::env::var("SERVER_ADDR").expect("SERVER_ADDR is not set");
    server::run(&addr).await?;
    Ok(())
}
