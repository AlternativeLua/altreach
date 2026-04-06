use std::sync::{Arc, Mutex};
use std::time::Duration;
use anyhow::Result;
use quinn::{Endpoint, ServerConfig, Connection};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tracing::{info, warn, error};
use altreach_proto::*;
use crate::capture::Capturer;
use crate::{clipboard, input};

pub async fn run(addr: &str, password: String) -> Result<()> {
    let cert = rcgen::generate_simple_self_signed(vec!["altreach".to_string()])?;
    let cert_der = CertificateDer::from(cert.cert.der().to_vec());
    let key_der = PrivateKeyDer::try_from(cert.key_pair.serialize_der())
        .map_err(|e| anyhow::anyhow!(e))?;

    let mut server_crypto = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert_der], key_der)?;
    server_crypto.alpn_protocols = vec![b"altreach".to_vec()];

    let server_config = ServerConfig::with_crypto(Arc::new(
        quinn::crypto::rustls::QuicServerConfig::try_from(server_crypto)?
    ));

    let endpoint = Endpoint::server(server_config, addr.parse()?)?;
    info!("Listening on {addr}");

    while let Some(incoming) = endpoint.accept().await {
        let password = password.clone();
        tokio::spawn(async move {
            match incoming.await {
                Ok(conn) => {
                    let peer = conn.remote_address();
                    info!("Client connected: {peer}");
                    if let Err(e) = handle_client(conn, password).await {
                        warn!("Client {peer} disconnected with error: {e}");
                    } else {
                        info!("Client {peer} disconnected cleanly");
                    }
                }
                Err(e) => error!("Incoming connection failed: {e}"),
            }
        });
    }
    Ok(())
}

async fn handle_client(conn: Connection, password: String) -> Result<()> {
    let peer = conn.remote_address();
    info!("Accepting control stream from {peer}");
    let (control_send, mut control_recv) = conn.accept_bi().await?;
    info!("Control stream accepted from {peer}");
    let mut frame_send = conn.open_uni().await?;
    info!("Frame stream opened to {peer}");
    let mut buf = Vec::new();

    let mut control_send = loop {
        if let Some((msg, consumed)) = decode::<ClientMessage>(&buf)? {
            buf.drain(..consumed);
            break match_message(&msg, control_send, &password).await?;
        }

        let mut tmp = [0u8; 4096];
        let n = control_recv.read(&mut tmp).await?;

        if n == Some(0) {
            anyhow::bail!("Connection closed before auth");
        }

        if let Some(n) = n {
            buf.extend_from_slice(&tmp[..n]);
        }
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
                frame_send.write_all(&encoded).await?;
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
                frame_send.write_all(&encoded).await?;
            }
            _ = clipboard_ticker.tick() => {
                if let Some(text) = clipboard::get_clipboard() {
                    if text != last_clipboard {
                        last_clipboard = text.clone();
                        let encoded = encode(&ServerMessage::ClipboardSync { text })?;
                        control_send.write_all(&encoded).await?;
                    }
                }
            }
            msg = read_message(&mut control_recv, &mut buf) => {
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

async fn match_message(msg: &ClientMessage, mut control_send: quinn::SendStream, password: &str) -> Result<quinn::SendStream> {
    match msg {
        ClientMessage::Handshake { version, password: client_password } => {
            if version != &PROTOCOL_VERSION {
                let bytes = encode(&ServerMessage::AuthResult {
                    success: false,
                    reason: Some(String::from("Wrong protocol version")),
                })?;
                control_send.write_all(&bytes).await?;
                Err(anyhow::anyhow!("Wrong version"))
            } else if client_password == password {
                let bytes = encode(&ServerMessage::AuthResult {
                    success: true,
                    reason: None,
                })?;
                control_send.write_all(&bytes).await?;
                Ok(control_send)
            } else {
                let bytes = encode(&ServerMessage::AuthResult {
                    success: false,
                    reason: Some(String::from("Wrong password")),
                })?;
                control_send.write_all(&bytes).await?;
                Err(anyhow::anyhow!("Wrong password"))
            }
        }
        _ => Ok(control_send)
    }
}

async fn read_message(recv: &mut quinn::RecvStream, buf: &mut Vec<u8>) -> Result<ClientMessage> {
    loop {
        if let Some((msg, consumed)) = decode::<ClientMessage>(buf)? {
            buf.drain(..consumed);
            return Ok(msg);
        }

        let mut tmp = [0u8; 4096];
        match recv.read(&mut tmp).await? {
            Some(n) => buf.extend_from_slice(&tmp[..n]),
            None => anyhow::bail!("Connection closed"),
        }
    }
}
