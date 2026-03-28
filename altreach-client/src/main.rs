mod client;
mod display;
mod input;

use anyhow::Result;
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    info!("altreach-client starting...");

    dotenvy::dotenv().ok();
    let addr = std::env::var("SERVER_ADDR").expect("SERVER_ADDR not set");

    let mut conn = client::Connection::connect("addr").await?;
    conn.send(&altreach_proto::ClientMessage::Handshake {
        version: altreach_proto::PROTOCOL_VERSION,
        password: "test".to_string(),
    }).await?;
    info!("Handshake sent");

    Ok(())
}
