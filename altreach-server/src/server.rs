use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use anyhow::Result;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tracing::{info, warn, error};
use altreach_proto::*;
use crate::capture::Capturer;
use crate::encoder::compress;
use crate::{clipboard, input};

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
    let (mut reader, writer) = stream.into_split();
    let mut buf = Vec::new();

    // Auth loop — keep reading until we get a Handshake.
    let mut writer = loop {
        if let Some((msg, consumed)) = decode::<ClientMessage>(&buf)? {
            buf.drain(..consumed);
            break match_message(&msg, writer).await?;
        }

        let mut tmp = [0u8; 4096];
        let n = reader.read(&mut tmp).await?;

        if n == 0 {
            anyhow::bail!("Connection closed before auth");
        }

        buf.extend_from_slice(&tmp[..n]);
    };

    info!("Client {peer} authenticated");

    let capturer = std::sync::Arc::new(std::sync::Mutex::new(Capturer::new()?));
    let frame_interval = Duration::from_millis(33); // ~30fps
    let mut last_clipboard = String::new();

    loop {
        tokio::select! {
        _ = tokio::time::sleep(frame_interval) => {
            let cap = capturer.clone();
            let (width, height, bytes) = tokio::task::spawn_blocking(move || {
                cap.lock().unwrap().capture_frame()
            }).await??;
            let data = compress(&bytes)?;
            let encoded = encode(&ServerMessage::Frame { width, height, data })?;
            writer.write_all(&encoded).await?;
        }
        _ = tokio::time::sleep(tokio::time::Duration::from_secs(1)) => {
            if let Some(text) = clipboard::get_clipboard() {
                if text != last_clipboard {
                    last_clipboard = text.clone();
                    let encoded = encode(&ServerMessage::ClipboardSync { text })?;
                    writer.write_all(&encoded).await?;
                }
            }
        }
        msg = read_message(&mut reader, &mut buf) => {
            match msg? {
                ClientMessage::MouseMove { x, y } => input::inject_mouse_move(x, y)?,
                ClientMessage::MouseButton { button, pressed, .. } => input::inject_mouse_button(&button, pressed)?,
                ClientMessage::KeyEvent { vk_code, pressed } => input::inject_key(vk_code, pressed)?,
                ClientMessage::MouseScroll { delta_x, delta_y } => input::inject_mouse_scroll(delta_x, delta_y)?,
                ClientMessage::Disconnect { .. } => break Ok(()),
                ClientMessage::ClipboardSync { text } => clipboard::set_clipboard(&text)?,
                _ => {}
            }
        }
    }
    }

}

async fn match_message(msg: &ClientMessage, mut writer: OwnedWriteHalf) -> Result<OwnedWriteHalf> {
    match msg {
        ClientMessage::Handshake { version, password } => {
            if version != &PROTOCOL_VERSION {
                let error_message = ServerMessage::AuthResult {
                    success: false,
                    reason: Some(String::from("Wrong protocol version")),
                };
                let bytes = encode(&error_message)?;
                writer.write_all(&bytes).await?;

                Err(anyhow::anyhow!("Wrong version"))
            } else if password == PASSWORD
            {
                let response_message = ServerMessage::AuthResult {
                    success: true,
                    reason: None
                };
                let bytes = encode(&response_message)?;

                writer.write_all(&bytes).await?;

                Ok((writer))
            } else {
                let error_message = ServerMessage::AuthResult {
                    success: false,
                    reason: Some(String::from("Wrong password")),
                };
                let bytes = encode(&error_message)?;
                writer.write_all(&bytes).await?;

                Err(anyhow::anyhow!("Wrong password"))
            }
            
        }
        _ => Ok((writer))
    }
}

async fn read_message(reader: &mut OwnedReadHalf, buf: &mut Vec<u8>) -> Result<ClientMessage> {
    loop {
        if let Some((msg, consumed)) = decode::<ClientMessage>(&buf)? {
            buf.drain(..consumed);
            return Ok(msg);
        }

        let mut tmp = [0u8; 4096];
        let n = reader.read(&mut tmp).await?;

        if n == 0 {
            anyhow::bail!("Connection closed");
        }

        buf.extend_from_slice(&tmp[..n]);
    }
}
