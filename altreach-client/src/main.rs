mod client;
mod display;
mod input;

use std::sync::mpsc;
use anyhow::Result;
use tracing::info;
use altreach_proto::{ClientMessage, ServerMessage, PROTOCOL_VERSION};
use display::Display;

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    dotenvy::dotenv().ok();

    let addr = std::env::var("SERVER_ADDR").expect("SERVER_ADDR not set");
    let password = std::env::var("PASSWORD").expect("PASSWORD not set");

    let (frame_tx, frame_rx) = mpsc::channel::<ServerMessage>();
    let (input_tx, input_rx) = mpsc::channel::<ClientMessage>();

    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async move {
            let mut conn = match client::Connection::connect(&addr).await {
                Ok(c) => c,
                Err(e) => { tracing::error!("Failed to connect: {e}"); return; }
            };

            if let Err(e) = conn.sender.send(&ClientMessage::Handshake {
                version: PROTOCOL_VERSION,
                password,
            }).await {
                tracing::error!("Failed to send handshake: {e}");
                return;
            }

            info!("Handshake sent");

            match conn.control_recv.recv_control().await {
                Ok(ServerMessage::AuthResult { success: true, .. }) => info!("Authenticated"),
                Ok(ServerMessage::AuthResult { success: false, reason }) => {
                    tracing::error!("Auth failed: {:?}", reason);
                    return;
                }
                Err(e) => { tracing::error!("Auth error: {e}"); return; }
                _ => {}
            }

            loop {
                tokio::select! {
                    msg = conn.frame_recv.recv_frame() => {
                        match msg {
                            Ok(m) => { let _ = frame_tx.send(m); }
                            Err(e) => { tracing::error!("Disconnected: {e}"); break; }
                        }
                    }
                    msg = conn.control_recv.recv_control() => {
                        match msg {
                            Ok(m) => { let _ = frame_tx.send(m); }
                            Err(e) => { tracing::error!("Control error: {e}"); break; }
                        }
                    }
                    Ok(msg) = async { input_rx.try_recv() } => {
                        if let Err(e) = conn.sender.send(&msg).await {
                            tracing::error!("Failed to send input: {e}"); break;
                        }
                    }
                }
            }
        });
    });

    eframe::run_native(
        "altreach",
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_inner_size([1280.0, 720.0])
                .with_title("altreach"),
            ..Default::default()
        },
        Box::new(|_cc| Ok(Box::new(Display::new(frame_rx, input_tx)))),
    ).map_err(|e| anyhow::anyhow!("{e}"))?;

    Ok(())
}
