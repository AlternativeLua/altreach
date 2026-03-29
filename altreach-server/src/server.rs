use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use anyhow::Result;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tracing::{info, warn, error};
use altreach_proto::*;
use crate::capture::Capturer;
use crate::{clipboard, input};

pub async fn run(addr: &str, password: String) -> Result<()> {
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

        let password = password.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_client(stream, peer, password).await {
                warn!("Client {peer} disconnected with error: {e}");
            } else {
                info!("Client {peer} disconnected cleanly");
            }
        });
    }
}

async fn handle_client(stream: TcpStream, peer: SocketAddr, password: String) -> Result<()> {
    let (mut reader, writer) = stream.into_split();
    let mut buf = Vec::new();

    let mut writer = loop {
        if let Some((msg, consumed)) = decode::<ClientMessage>(&buf)? {
            buf.drain(..consumed);
            break match_message(&msg, writer, &password).await?;
        }

        let mut tmp = [0u8; 4096];
        let n = reader.read(&mut tmp).await?;

        if n == 0 {
            anyhow::bail!("Connection closed before auth");
        }

        buf.extend_from_slice(&tmp[..n]);
    };

    info!("Client {peer} authenticated");

    let capturer = Arc::new(Mutex::new(Capturer::new()?));

    // Send initial full frame
    {
        let cap = capturer.clone();
        match tokio::task::spawn_blocking(move || cap.lock().unwrap().capture_full()).await {
            Ok(Ok(Some((sw, sh, patches)))) => {
                info!("Sending initial frame {sw}x{sh} with {} patches", patches.len());
                let encoded = encode(&ServerMessage::DeltaFrame { screen_width: sw, screen_height: sh, patches })?;
                writer.write_all(&encoded).await?;
            }
            Ok(Ok(None)) => warn!("Initial capture returned nothing"),
            Ok(Err(e)) => warn!("Initial capture error: {e}"),
            Err(e) => warn!("Initial capture task error: {e}"),
        }
    }

    let (frame_tx, mut frame_rx) = tokio::sync::mpsc::channel::<(u32, u32, Vec<FramePatch>)>(2);

    // Dedicated capture task — runs independently so the input loop is never blocked.
    {
        let capturer = capturer.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_millis(33));
            loop {
                ticker.tick().await;
                let cap = capturer.clone();
                let result = tokio::task::spawn_blocking(move || {
                    cap.lock().unwrap().capture_frame()
                }).await;

                match result {
                    Ok(Ok(Some(frame))) => {
                        if frame_tx.send(frame).await.is_err() {
                            break; // receiver dropped, client disconnected
                        }
                    }
                    Ok(Ok(None)) => {} // no new frame
                    Ok(Err(e)) => { warn!("Capture error: {e}"); break; }
                    Err(e) => { warn!("Capture task error: {e}"); break; }
                }
            }
        });
    }

    let mut clipboard_ticker = tokio::time::interval(Duration::from_secs(1));
    let mut last_clipboard = String::new();

    loop {
        tokio::select! {
            Some((screen_width, screen_height, patches)) = frame_rx.recv() => {
                let encoded = encode(&ServerMessage::DeltaFrame { screen_width, screen_height, patches })?;
                writer.write_all(&encoded).await?;
            }
            _ = clipboard_ticker.tick() => {
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

async fn match_message(msg: &ClientMessage, mut writer: OwnedWriteHalf, password: &str) -> Result<OwnedWriteHalf> {
    match msg {
        ClientMessage::Handshake { version, password: client_password } => {
            if version != &PROTOCOL_VERSION {
                let error_message = ServerMessage::AuthResult {
                    success: false,
                    reason: Some(String::from("Wrong protocol version")),
                };
                let bytes = encode(&error_message)?;
                writer.write_all(&bytes).await?;
                Err(anyhow::anyhow!("Wrong version"))
            } else if client_password == password {
                let response_message = ServerMessage::AuthResult {
                    success: true,
                    reason: None,
                };
                let bytes = encode(&response_message)?;
                writer.write_all(&bytes).await?;
                Ok(writer)
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
        _ => Ok(writer)
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
