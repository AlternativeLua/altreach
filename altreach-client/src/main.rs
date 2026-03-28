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

            if let Err(e) = conn.send(&ClientMessage::Handshake {
                version: PROTOCOL_VERSION,
                password,
            }).await {
                tracing::error!("Failed to send handshake: {e}");
                return;
            }

            info!("Handshake sent");

            loop {
                // Drain any pending input events and send them to the server.
                while let Ok(msg) = input_rx.try_recv() {
                    if let Err(e) = conn.send(&msg).await {
                        tracing::error!("Failed to send input: {e}");
                        return;
                    }
                }

                match conn.recv().await {
                    Ok(msg) => { let _ = frame_tx.send(msg); }
                    Err(e) => { tracing::error!("Disconnected: {e}"); break; }
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
