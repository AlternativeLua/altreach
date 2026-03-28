use std::sync::mpsc::{Receiver, Sender};
use egui::{ColorImage, TextureHandle};
use altreach_proto::{ClientMessage, ServerMessage};
use crate::input::egui_key_to_vk;

pub struct Display {
    texture: Option<TextureHandle>,
    receiver: Receiver<ServerMessage>,
    sender: Sender<ClientMessage>,
}

impl Display {
    pub fn new(receiver: Receiver<ServerMessage>, sender: Sender<ClientMessage>) -> Self {
        Self { texture: None, receiver, sender }
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

        egui::CentralPanel::default().frame(egui::Frame::none()).show(ctx, |ui| {
            if let Some(texture) = &self.texture {
                ui.add(
                    egui::Image::new(texture)
                        .fit_to_exact_size(ui.available_size())
                );
            }
        });

        ctx.request_repaint();

        let mut msgs = Vec::new();
        let mut current_pos = (0i32, 0i32);
        let screen_rect = ctx.screen_rect();
        ctx.input(|i| {
            if let Some(pos) = i.pointer.latest_pos() {
                let nx = (pos.x / screen_rect.width()) * 65535.0;
                let ny = (pos.y / screen_rect.height()) * 65535.0;
                current_pos = (nx as i32, ny as i32);
                msgs.push(ClientMessage::MouseMove { x: current_pos.0, y: current_pos.1 });
            }

            if i.pointer.button_pressed(egui::PointerButton::Primary) {
                msgs.push(ClientMessage::MouseButton {
                    button: altreach_proto::MouseButton::Left,
                    pressed: true,
                    x: current_pos.0,
                    y: current_pos.1,
                });
            } else if i.pointer.button_released(egui::PointerButton::Primary) {
                msgs.push(ClientMessage::MouseButton {
                    button: altreach_proto::MouseButton::Left,
                    pressed: false,
                    x: current_pos.0,
                    y: current_pos.1,
                });
            }
            if i.pointer.button_pressed(egui::PointerButton::Secondary) {
                msgs.push(ClientMessage::MouseButton {
                    button: altreach_proto::MouseButton::Right,
                    pressed: true,
                    x: current_pos.0,
                    y: current_pos.1,
                });
            } else if i.pointer.button_released(egui::PointerButton::Secondary) {
                msgs.push(ClientMessage::MouseButton {
                    button: altreach_proto::MouseButton::Right,
                    pressed: false,
                    x: current_pos.0,
                    y: current_pos.1,
                });
            }
            if i.pointer.button_pressed(egui::PointerButton::Middle) {
                msgs.push(ClientMessage::MouseButton {
                    button: altreach_proto::MouseButton::Middle,
                    pressed: true,
                    x: current_pos.0,
                    y: current_pos.1,
                });
            } else if i.pointer.button_released(egui::PointerButton::Middle) {
                msgs.push(ClientMessage::MouseButton {
                    button: altreach_proto::MouseButton::Middle,
                    pressed: false,
                    x: current_pos.0,
                    y: current_pos.1,
                });
            }

            for event in &i.events {
                if let egui::Event::Key { key, pressed, .. } = event {
                    msgs.push(ClientMessage::KeyEvent {
                        vk_code: egui_key_to_vk(*key),
                        pressed: *pressed,
                    });
                }
            }

        });
        for msg in msgs {
            let _ = self.sender.send(msg);
        }
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
