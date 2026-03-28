use std::net::SocketAddr;
use tokio::net::{TcpListener, TcpStream};
use anyhow::Result;
use tracing::{info, warn, error};

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

async fn handle_client(stream: TcpStream, peer: SocketAddr) -> Result<()> {
    let (reader, _writer) = stream.into_split();
    let mut buf = vec![0u8; 1024];

    loop {
        reader.readable().await?;

        match reader.try_read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                info!("Received {n} bytes from {peer}");
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
            Err(e) => return Err(e.into()),
        }
    }

    Ok(())
}
