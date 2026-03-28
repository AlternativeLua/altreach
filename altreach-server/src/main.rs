mod capture;
mod encoder;
mod input;
mod server;

use anyhow::Result;
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    info!("altreach-server starting...");
    server::run("0.0.0.0:7878").await?;
    Ok(())
}
