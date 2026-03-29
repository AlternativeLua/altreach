use anyhow::Result;
use openh264::encoder::{Encoder, EncoderConfig, UsageType};
use openh264::formats::YUVBuffer;
use openh264::OpenH264API;

pub struct H264Encoder {
    inner: Option<Encoder>,
    width: u32,
    height: u32,
}

impl H264Encoder {
    pub fn new() -> Self {
        Self { inner: None, width: 0, height: 0 }
    }

    pub fn encode(&mut self, bgra: &[u8], width: u32, height: u32) -> Result<Option<Vec<u8>>> {
        if self.inner.is_none() || self.width != width || self.height != height {
            let config = EncoderConfig::new()
                .set_bitrate_bps(5_000_000)
                .max_frame_rate(30.0)
                .usage_type(UsageType::ScreenContentRealTime);
            self.inner = Some(Encoder::with_api_config(OpenH264API::from_source(), config)?);
            self.width = width;
            self.height = height;
        }

        let encoder = self.inner.as_mut().unwrap();
        let yuv = bgra_to_yuv420(bgra, width as usize, height as usize);
        let bitstream = encoder.encode(&yuv)?;
        let data = bitstream.to_vec();

        if data.is_empty() {
            return Ok(None);
        }

        Ok(Some(data))
    }
}

fn bgra_to_yuv420(bgra: &[u8], width: usize, height: usize) -> YUVBuffer {
    let y_size = width * height;
    let uv_size = (width / 2) * (height / 2);
    let mut yuv_data = vec![0u8; y_size + uv_size * 2];

    let (y_plane, uv) = yuv_data.split_at_mut(y_size);
    let (u_plane, v_plane) = uv.split_at_mut(uv_size);

    for row in 0..height {
        for col in 0..width {
            let i = (row * width + col) * 4;
            let b = bgra[i] as i32;
            let g = bgra[i + 1] as i32;
            let r = bgra[i + 2] as i32;

            let y = ((66 * r + 129 * g + 25 * b + 128) >> 8) + 16;
            y_plane[row * width + col] = y.clamp(0, 255) as u8;

            if row % 2 == 0 && col % 2 == 0 {
                let u = ((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128;
                let v = ((112 * r - 94 * g - 18 * b + 128) >> 8) + 128;
                let uv_idx = (row / 2) * (width / 2) + (col / 2);
                u_plane[uv_idx] = u.clamp(0, 255) as u8;
                v_plane[uv_idx] = v.clamp(0, 255) as u8;
            }
        }
    }

    YUVBuffer::from_vec(yuv_data, width, height)
}
