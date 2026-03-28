use std::sync::mpsc::Receiver;
use egui::{ColorImage, TextureHandle};
use altreach_proto::ServerMessage;

pub struct Display {
    texture: Option<TextureHandle>,
    receiver: Receiver<ServerMessage>,
}

impl Display {
    pub fn new(receiver: Receiver<ServerMessage>) -> Self {
        Self { texture: None, receiver }
    }
}

impl eframe::App for Display {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Drain the channel but only render the latest frame.
        // The server sends faster than we can render, so we skip stale frames.
        let mut latest = None;
        while let Ok(msg) = self.receiver.try_recv() {
            latest = Some(msg);
        }
        if let Some(ServerMessage::Frame { width, height, data }) = latest {
            self.update_frame(ctx, width, height, data);
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            if let Some(texture) = &self.texture {
                ui.image(texture);
            }
        });

        ctx.request_repaint();
    }
}

impl Display {
    fn update_frame(&mut self, ctx: &egui::Context, width: u32, height: u32, data: Vec<u8>) {
        let data = zstd::decode_all(&data[..]).unwrap();
        let mut rgba = data;
        for pixel in rgba.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }

        let image = ColorImage::from_rgba_unmultiplied(
            [width as usize, height as usize],
            &rgba,
        );

        self.texture = Some(ctx.load_texture("frame", image, Default::default()));
    }
}
