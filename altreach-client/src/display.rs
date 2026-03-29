use std::sync::mpsc::{Receiver, Sender};
use egui::{ColorImage, TextureHandle};
use altreach_proto::{ClientMessage, ServerMessage};
use openh264::decoder::Decoder;
use openh264::formats::YUVSource;
use crate::input::egui_key_to_vk;

pub struct Display {
    texture: Option<TextureHandle>,
    receiver: Receiver<ServerMessage>,
    sender: Sender<ClientMessage>,
    remote_size: Option<(u32, u32)>,
    clipboard: arboard::Clipboard,
    last_clipboard: String,
    decoder: Decoder,
}

impl Display {
    pub fn new(receiver: Receiver<ServerMessage>, sender: Sender<ClientMessage>) -> Self {
        Self {
            texture: None,
            receiver,
            sender,
            remote_size: None,
            clipboard: arboard::Clipboard::new().expect("Failed to init clipboard"),
            last_clipboard: String::new(),
            decoder: Decoder::new().expect("Failed to init H264 decoder"),
        }
    }
}

impl eframe::App for Display {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(msg) = self.receiver.try_recv() {
            match msg {
                ServerMessage::ClipboardSync { text } => {
                    self.clipboard.set_text(&text).ok();
                    self.last_clipboard = text;
                }
                ServerMessage::VideoFrame { width, height, data } => {
                    self.update_frame(ctx, width, height, &data);
                }
                _ => {}
            }
        }

        if let Ok(text) = self.clipboard.get_text() {
            if text != self.last_clipboard {
                self.last_clipboard = text.clone();
                let _ = self.sender.send(ClientMessage::ClipboardSync { text });
            }
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
        let screen_rect = ctx.screen_rect();

        if let Some((rw, rh)) = self.remote_size {
            let target_h = screen_rect.width() * rh as f32 / rw as f32;
            if (target_h - screen_rect.height()).abs() > 1.0 {
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(
                    egui::vec2(screen_rect.width(), target_h),
                ));
            }
        }

        ctx.input(|i| {
            if let Some(pos) = i.pointer.latest_pos() {
                let nx = (pos.x / screen_rect.width()).clamp(0.0, 1.0) * 65535.0;
                let ny = (pos.y / screen_rect.height()).clamp(0.0, 1.0) * 65535.0;
                msgs.push(ClientMessage::MouseMove { x: nx as i32, y: ny as i32 });
            }

            for event in &i.events {
                match event {
                    egui::Event::PointerButton { button, pressed, pos, .. } => {
                        let nx = (pos.x / screen_rect.width()).clamp(0.0, 1.0) * 65535.0;
                        let ny = (pos.y / screen_rect.height()).clamp(0.0, 1.0) * 65535.0;
                        let proto_button = match button {
                            egui::PointerButton::Primary => altreach_proto::MouseButton::Left,
                            egui::PointerButton::Secondary => altreach_proto::MouseButton::Right,
                            egui::PointerButton::Middle => altreach_proto::MouseButton::Middle,
                            _ => continue,
                        };
                        msgs.push(ClientMessage::MouseButton {
                            button: proto_button,
                            pressed: *pressed,
                            x: nx as i32,
                            y: ny as i32,
                        });
                    }
                    egui::Event::Key { key, pressed, modifiers, .. } => {
                        if modifiers.mac_cmd {
                            msgs.push(ClientMessage::KeyEvent { vk_code: 0x11, pressed: *pressed });
                        }
                        msgs.push(ClientMessage::KeyEvent {
                            vk_code: egui_key_to_vk(*key),
                            pressed: *pressed,
                        });
                    }
                    egui::Event::MouseWheel { delta, .. } => {
                        msgs.push(ClientMessage::MouseScroll {
                            delta_x: delta.x as i32,
                            delta_y: delta.y as i32,
                        });
                    }
                    _ => {}
                }
            }
        });

        for msg in msgs {
            let _ = self.sender.send(msg);
        }
    }
}

impl Display {
    fn update_frame(&mut self, ctx: &egui::Context, width: u32, height: u32, data: &[u8]) {
        let yuv = match self.decoder.decode(data) {
            Ok(Some(yuv)) => yuv,
            _ => return,
        };

        let (w, h) = yuv.dimensions();
        let mut rgba = vec![0u8; w * h * 4];
        yuv.write_rgba8(&mut rgba);

        let image = ColorImage::from_rgba_unmultiplied([w, h], &rgba);
        self.remote_size = Some((width, height));
        self.texture = Some(ctx.load_texture("frame", image, Default::default()));
    }
}
