use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub enum ClientMessage {
    Handshake { version: u32, password: String },
    MouseMove { x: i32, y: i32 },
    MouseButton { button: MouseButton, pressed: bool, x: i32, y: i32 },
    MouseScroll { delta_x: i32, delta_y: i32 },
    KeyEvent { vk_code: u16, pressed: bool },
    Ping,
    Disconnect { reason: String },
}

#[derive(Debug, Serialize, Deserialize)]
pub enum ServerMessage {
    AuthResult { success: bool, reason: Option<String> },
    Frame { width: u32, height: u32, data: Vec<u8> },
    Pong,
    Disconnect { reason: String },
}

#[derive(Debug, Serialize, Deserialize)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

// Wire format: [u32 length (4 bytes, little-endian)][bincode payload]

pub fn encode<T: Serialize>(msg: &T) -> Result<Vec<u8>, bincode::Error> {
    let payload = bincode::serialize(msg)?;
    let len = payload.len() as u32;
    let mut buf = Vec::with_capacity(4 + payload.len());
    buf.extend_from_slice(&len.to_le_bytes());
    buf.extend_from_slice(&payload);
    Ok(buf)
}

pub fn decode<T: for<'de> Deserialize<'de>>(
    buf: &[u8],
) -> Result<Option<(T, usize)>, bincode::Error> {
    if buf.len() < 4 {
        return Ok(None);
    }
    let len = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
    if buf.len() < 4 + len {
        return Ok(None);
    }
    let msg = bincode::deserialize(&buf[4..4 + len])?;
    Ok(Some((msg, 4 + len)))
}

pub const PROTOCOL_VERSION: u32 = 1;
// Hard coded for now :3
pub const PASSWORD: &str = "your_password_here";
