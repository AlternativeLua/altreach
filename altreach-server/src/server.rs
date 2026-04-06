use std::sync::{Arc, Mutex};
use std::time::Duration;
use anyhow::Result;
use quinn::{Endpoint, ServerConfig, Connection};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tracing::{info, warn, error};
use altreach_proto::*;
use crate::capture::Capturer;
use crate::encoder::H264Encoder;
use crate::{clipboard, input};

struct CaptureEncoder {
    capturer: Capturer,
    encoder: Option<H264Encoder>,
}

impl CaptureEncoder {
    fn new() -> Result<Self> {
        Ok(Self { capturer: Capturer::new()?, encoder: None })
    }

    fn capture_and_encode(&mut self) -> Result<Option<(u32, u32, Vec<u8>)>> {
        let bgra = match self.capturer.capture_frame()? {
            Some(b) => b,
            None => return Ok(None),
        };

        let (w, h) = self.capturer.staging_size;

        if self.encoder.is_none() {
            info!("Creating H.264 encoder for {w}x{h}");
            self.encoder = Some(H264Encoder::new(w, h)?);
        }

        match self.encoder.as_mut().unwrap().encode(&bgra)? {
            Some(data) => Ok(Some((w, h, data))),
            None => Ok(None),
        }
    }
}

pub async fn run(addr: &str, password: String) -> Result<()> {
    let cert = rcgen::generate_simple_self_signed(vec!["altreach".to_string()])?;
    let cert_der = CertificateDer::from(cert.cert.der().to_vec());
    let key_der = PrivateKeyDer::try_from(cert.key_pair.serialize_der())
        .map_err(|e| anyhow::anyhow!(e))?;

    let mut server_crypto = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert_der], key_der)?;
    server_crypto.alpn_protocols = vec![b"altreach".to_vec()];

    let mut transport = quinn::TransportConfig::default();
    transport.max_concurrent_bidi_streams(100_u32.into());
    transport.max_concurrent_uni_streams(100_u32.into());
    transport.keep_alive_interval(Some(Duration::from_secs(5)));

    let mut server_config = ServerConfig::with_crypto(Arc::new(
        quinn::crypto::rustls::QuicServerConfig::try_from(server_crypto)?
    ));
    server_config.transport_config(Arc::new(transport));

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
    let (mut control_send, mut control_recv) = conn.accept_bi().await?;
    info!("Control stream accepted from {peer}");
    let (mut frame_send, _) = conn.accept_bi().await?;
    info!("Frame stream accepted from {peer}");
    let mut buf = Vec::new();

    let mut control_send = loop {
        if let Some((msg, consumed)) = decode::<ClientMessage>(&buf)? {
            buf.drain(..consumed);
            break match_message(&msg, control_send, &password).await?;
        }
        let mut tmp = [0u8; 4096];
        if let Some(n) = control_recv.read(&mut tmp).await? {
            buf.extend_from_slice(&tmp[..n]);
        } else {
            anyhow::bail!("Connection closed before auth");
        }
    };

    info!("Client {peer} authenticated");

    let cap_enc = Arc::new(Mutex::new(CaptureEncoder::new()?));

    let (frame_tx, mut frame_rx) = tokio::sync::mpsc::channel::<(u32, u32, Vec<u8>)>(2);

    {
        let cap_enc = cap_enc.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_millis(33));
            loop {
                ticker.tick().await;
                let ce = cap_enc.clone();
                let result = tokio::task::spawn_blocking(move || {
                    ce.lock().unwrap().capture_and_encode()
                }).await;

                match result {
                    Ok(Ok(Some(frame))) => {
                        if frame_tx.send(frame).await.is_err() { break; }
                    }
                    Ok(Ok(None)) => {}
                    Ok(Err(e)) => { warn!("Capture/encode error: {e}"); break; }
                    Err(e) => { warn!("Task error: {e}"); break; }
                }
            }
        });
    }

    let mut clipboard_ticker = tokio::time::interval(Duration::from_secs(1));
    let mut last_clipboard = String::new();

    loop {
        tokio::select! {
            Some((width, height, data)) = frame_rx.recv() => {
                let encoded = encode(&ServerMessage::VideoFrame { width, height, data })?;
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
                let bytes = encode(&ServerMessage::AuthResult { success: false, reason: Some("Wrong protocol version".into()) })?;
                control_send.write_all(&bytes).await?;
                Err(anyhow::anyhow!("Wrong version"))
            } else if client_password == password {
                let bytes = encode(&ServerMessage::AuthResult { success: true, reason: None })?;
                control_send.write_all(&bytes).await?;
                Ok(control_send)
            } else {
                let bytes = encode(&ServerMessage::AuthResult { success: false, reason: Some("Wrong password".into()) })?;
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
