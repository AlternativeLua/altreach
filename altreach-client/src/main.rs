mod client;
mod display;
mod input;

use anyhow::Result;
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    info!("altreach-client starting...");
    Ok(())
}
