use std::net::SocketAddr;
use tokio::net::{TcpListener, TcpStream};
use anyhow::Result;
use tokio::io::AsyncReadExt;
use tracing::{info, warn, error};

use altreach_proto::*;

pub async fn run(addr: &str) -> Result<()> {
    let listener = TcpListener::bind(addr).await?;
    info!("Listening on {addr}");

    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(pair) => pair,
            Err(e) => {
                error!("Failed to accept connection: {e}");
                continue;
            }
        };

        info!("Client connected: {peer}");

        tokio::spawn(async move {
            if let Err(e) = handle_client(stream, peer).await {
                warn!("Client {peer} disconnected with error: {e}");
            } else {
                info!("Client {peer} disconnected cleanly");
            }
        });
    }
}

async fn handle_client(mut stream: TcpStream, peer: SocketAddr) -> Result<ClientMessage> {
    let mut buf = Vec::new();

    loop {
        if let Some((msg, consumed)) = decode::<ClientMessage>(&buf)? {
            buf.drain(..consumed);
            return Ok(msg);
        }

        let mut tmp = [0u8; 4096];
        let n = stream.read(&mut tmp).await?;

        if n == 0 {
            anyhow::bail!("Connection closed with empty buffer");
        }

        buf.extend_from_slice(&tmp[..n]);
    }
}
