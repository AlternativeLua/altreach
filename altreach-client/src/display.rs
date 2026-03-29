use std::sync::mpsc::{Receiver, Sender};
use egui::{ColorImage, TextureHandle};
use altreach_proto::{ClientMessage, ServerMessage};
use crate::input::egui_key_to_vk;

pub struct Display {
    texture: Option<TextureHandle>,
    receiver: Receiver<ServerMessage>,
    sender: Sender<ClientMessage>,
    remote_size: Option<(u32, u32)>,
    clipboard: arboard::Clipboard,
    last_clipboard: String,
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
        }
    }
}

impl eframe::App for Display {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Handle incoming server messages.
        let mut latest_frame = None;
        while let Ok(msg) = self.receiver.try_recv() {
            match msg {
                ServerMessage::ClipboardSync { text } => {
                    self.clipboard.set_text(&text).ok();
                    self.last_clipboard = text;
                }
                _ => latest_frame = Some(msg),
            }
        }
        if let Some(ServerMessage::Frame { width, height, data }) = latest_frame {
            self.update_frame(ctx, width, height, data);
        }

        // Poll local clipboard and send to server if it changed.
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
        let mut current_pos = (0i32, 0i32);
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
                current_pos = (nx as i32, ny as i32);
                msgs.push(ClientMessage::MouseMove { x: current_pos.0, y: current_pos.1 });
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
                        // Intercept Cmd+C/V/X on Mac and handle as clipboard,
                        // don't forward them as raw keypresses to Windows.
                        if modifiers.mac_cmd {
                            return;
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
    fn update_frame(&mut self, ctx: &egui::Context, width: u32, height: u32, data: Vec<u8>) {
        let data = lz4_flex::decompress_size_prepended(&data).unwrap();
        let mut rgba = data;
        for pixel in rgba.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }

        let image = ColorImage::from_rgba_unmultiplied(
            [width as usize, height as usize],
            &rgba,
        );

        self.remote_size = Some((width, height));
        self.texture = Some(ctx.load_texture("frame", image, Default::default()));
    }
}
