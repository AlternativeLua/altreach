use std::sync::mpsc::{Receiver, Sender};
use egui::{ColorImage, TextureHandle};
use altreach_proto::{ClientMessage, ServerMessage};
use openh264::formats::YUVSource;
use openh264::decoder::Decoder;
use openh264::OpenH264API;
use crate::input::egui_key_to_vk;

pub struct Display {
    texture: Option<TextureHandle>,
    receiver: Receiver<ServerMessage>,
    sender: Sender<ClientMessage>,
    remote_size: Option<(u32, u32)>,
    clipboard: arboard::Clipboard,
    last_clipboard: String,
    decoder: Decoder,
    rgba_buf: Vec<u8>,
    frames_received: u32,
}

impl Display {
    pub fn new(receiver: Receiver<ServerMessage>, sender: Sender<ClientMessage>) -> Self {
        let decoder = Decoder::with_api_config(
            OpenH264API::from_source(),
            openh264::decoder::DecoderConfig::default(),
        ).expect("Failed to create H.264 decoder");

        Self {
            texture: None,
            receiver,
            sender,
            remote_size: None,
            clipboard: arboard::Clipboard::new().expect("Failed to init clipboard"),
            last_clipboard: String::new(),
            decoder,
            rgba_buf: Vec::new(),
            frames_received: 0,
        }
    }

    fn decode_and_upload(&mut self, ctx: &egui::Context, data: &[u8], width: u32, height: u32) {
        self.frames_received += 1;
        if self.frames_received <= 5 && data.len() >= 8 {
            tracing::info!(
                "Frame {} ({} bytes): {:02X} {:02X} {:02X} {:02X} {:02X} {:02X} {:02X} {:02X}",
                self.frames_received, data.len(),
                data[0], data[1], data[2], data[3], data[4], data[5], data[6], data[7]
            );
        }
        match self.decoder.decode(data) {
            Ok(Some(yuv)) => {
                let (w, h) = yuv.dimensions();
                let y = yuv.y();
                let u = yuv.u();
                let v = yuv.v();
                let (ys, us, vs) = yuv.strides();

                self.rgba_buf.resize(w * h * 4, 0);

                for row in 0..h {
                    for col in 0..w {
                        let yv = y[row * ys + col] as f32;
                        let uv = u[(row / 2) * us + col / 2] as f32 - 128.0;
                        let vv = v[(row / 2) * vs + col / 2] as f32 - 128.0;

                        let r = (yv + 1.402 * vv).clamp(0.0, 255.0) as u8;
                        let g = (yv - 0.344 * uv - 0.714 * vv).clamp(0.0, 255.0) as u8;
                        let b = (yv + 1.772 * uv).clamp(0.0, 255.0) as u8;

                        let idx = (row * w + col) * 4;
                        self.rgba_buf[idx] = r;
                        self.rgba_buf[idx + 1] = g;
                        self.rgba_buf[idx + 2] = b;
                        self.rgba_buf[idx + 3] = 255;
                    }
                }

                self.remote_size = Some((w as u32, h as u32));
                let image = ColorImage::from_rgba_unmultiplied([w, h], &self.rgba_buf);
                self.texture = Some(ctx.load_texture("frame", image, Default::default()));
            }
            Ok(None) => {}
            Err(e) => {
                tracing::warn!("Decode error: {e}, recreating decoder");
                // Recreate the decoder to clear any broken state.
                // The next IDR frame (with embedded SPS/PPS) will re-initialize it.
                if let Ok(dec) = Decoder::with_api_config(
                    OpenH264API::from_source(),
                    openh264::decoder::DecoderConfig::default(),
                ) {
                    self.decoder = dec;
                }
            }
        }
    }
}

impl eframe::App for Display {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Drain all pending messages but only decode the LATEST VideoFrame.
        // Processing every queued frame would block the UI thread and cause
        // a cascade of tracing::warn! calls that freeze the window.
        let mut latest_frame: Option<(u32, u32, Vec<u8>)> = None;
        while let Ok(msg) = self.receiver.try_recv() {
            match msg {
                ServerMessage::ClipboardSync { text } => {
                    self.clipboard.set_text(&text).ok();
                    self.last_clipboard = text;
                }
                ServerMessage::VideoFrame { width, height, data } => {
                    latest_frame = Some((width, height, data));
                }
                _ => {}
            }
        }
        if let Some((width, height, data)) = latest_frame {
            self.decode_and_upload(ctx, &data, width, height);
        }

        if let Ok(text) = self.clipboard.get_text() {
            if text != self.last_clipboard {
                self.last_clipboard = text.clone();
                let _ = self.sender.send(ClientMessage::ClipboardSync { text });
            }
        }

        egui::CentralPanel::default().frame(egui::Frame::none()).show(ctx, |ui| {
            if let Some(texture) = &self.texture {
                ui.add(egui::Image::new(texture).fit_to_exact_size(ui.available_size()));
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
