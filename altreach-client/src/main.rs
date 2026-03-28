mod client;
mod display;
mod input;

use anyhow::Result;
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    info!("altreach-client starting...");

    let mut conn = client::Connection::connect("127.0.0.1:7878").await?;
    conn.send(&altreach_proto::ClientMessage::Handshake {
        version: altreach_proto::PROTOCOL_VERSION,
        password: "test".to_string(),
    }).await?;
    info!("Handshake sent");

    Ok(())
}
