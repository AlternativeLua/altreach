use anyhow::Result;
use tracing::{info, warn};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::*;
use std::mem::ManuallyDrop;

pub struct H264Encoder {
    transform: IMFTransform,
    width: u32,
    height: u32,
    timestamp: i64,
    provides_samples: bool,
    output_buffer_size: u32,
}

// IMFTransform is a COM object — safe to send across threads when using COINIT_MULTITHREADED
unsafe impl Send for H264Encoder {}

/// Pack two u32 values into a u64 attribute (equivalent to MFSetAttributeSize / MFSetAttributeRatio).
#[inline]
unsafe fn set_attr_u64(attr: &IMFMediaType, key: &windows::core::GUID, hi: u32, lo: u32) -> Result<()> {
    let val: u64 = ((hi as u64) << 32) | (lo as u64);
    attr.SetUINT64(key, val)?;
    Ok(())
}

impl H264Encoder {
    pub fn new(width: u32, height: u32) -> Result<Self> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            MFStartup(MF_VERSION, MFSTARTUP_FULL)?;

            let transform = Self::find_encoder()?;

            // Output type (H.264) must be set before input type
            let out_type: IMFMediaType = MFCreateMediaType()?;
            out_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            out_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
            out_type.SetUINT32(&MF_MT_AVG_BITRATE, 10_000_000)?;
            out_type.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            set_attr_u64(&out_type, &MF_MT_FRAME_SIZE, width, height)?;
            set_attr_u64(&out_type, &MF_MT_FRAME_RATE, 30, 1)?;
            set_attr_u64(&out_type, &MF_MT_PIXEL_ASPECT_RATIO, 1, 1)?;
            transform.SetOutputType(0, &out_type, 0)?;

            // Input type (NV12)
            let in_type: IMFMediaType = MFCreateMediaType()?;
            in_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            in_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
            in_type.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            set_attr_u64(&in_type, &MF_MT_FRAME_SIZE, width, height)?;
            set_attr_u64(&in_type, &MF_MT_FRAME_RATE, 30, 1)?;
            set_attr_u64(&in_type, &MF_MT_PIXEL_ASPECT_RATIO, 1, 1)?;
            transform.SetInputType(0, &in_type, 0)?;

            transform.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
            transform.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;

            let stream_info = transform.GetOutputStreamInfo(0)?;
            let provides_samples = (stream_info.dwFlags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32) != 0;
            let output_buffer_size = stream_info.cbSize.max(1024 * 1024);

            Ok(Self { transform, width, height, timestamp: 0, provides_samples, output_buffer_size })
        }
    }

    unsafe fn find_encoder() -> Result<IMFTransform> {
        let input_info = MFT_REGISTER_TYPE_INFO {
            guidMajorType: MFMediaType_Video,
            guidSubtype: MFVideoFormat_NV12,
        };
        let output_info = MFT_REGISTER_TYPE_INFO {
            guidMajorType: MFMediaType_Video,
            guidSubtype: MFVideoFormat_H264,
        };

        let mut count = 0u32;
        let mut activates: *mut Option<IMFActivate> = std::ptr::null_mut();

        let _ = MFTEnumEx(
            MFT_CATEGORY_VIDEO_ENCODER,
            MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER,
            Some(&input_info),
            Some(&output_info),
            &mut activates,
            &mut count,
        );

        if count > 0 && !activates.is_null() {
            let slice = std::slice::from_raw_parts(activates, count as usize);
            if let Some(Some(activate)) = slice.first() {
                match activate.ActivateObject::<IMFTransform>() {
                    Ok(transform) => {
                        CoTaskMemFree(Some(activates as *mut std::ffi::c_void));
                        info!("Using hardware H.264 encoder");
                        return Ok(transform);
                    }
                    Err(e) => warn!("Hardware encoder activation failed: {e}"),
                }
            }
            CoTaskMemFree(Some(activates as *mut std::ffi::c_void));
        }

        warn!("No hardware H.264 encoder found, using software fallback");

        // Microsoft software H.264 encoder CLSID
        const CLSID_CMSH264EncoderMFT: windows::core::GUID = windows::core::GUID::from_values(
            0x49570228, 0x7d10, 0x4b93, [0xae, 0x7f, 0xaa, 0xaa, 0xd0, 0x35, 0x3b, 0x08],
        );

        let transform: IMFTransform = CoCreateInstance(
            &CLSID_CMSH264EncoderMFT,
            None,
            CLSCTX_INPROC_SERVER,
        )?;

        Ok(transform)
    }

    pub fn encode(&mut self, bgra: &[u8]) -> Result<Option<Vec<u8>>> {
        unsafe {
            let nv12 = bgra_to_nv12(bgra, self.width, self.height);

            let buffer: IMFMediaBuffer = MFCreateMemoryBuffer(nv12.len() as u32)?;
            let mut ptr: *mut u8 = std::ptr::null_mut();
            buffer.Lock(&mut ptr, None, None)?;
            std::ptr::copy_nonoverlapping(nv12.as_ptr(), ptr, nv12.len());
            buffer.Unlock()?;
            buffer.SetCurrentLength(nv12.len() as u32)?;

            let sample: IMFSample = MFCreateSample()?;
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime(self.timestamp)?;
            sample.SetSampleDuration(333_333)?;
            self.timestamp += 333_333;

            self.transform.ProcessInput(0, &sample, 0)?;
            self.get_output()
        }
    }

    unsafe fn get_output(&self) -> Result<Option<Vec<u8>>> {
        let pre_alloc_sample: Option<IMFSample> = if !self.provides_samples {
            let buf: IMFMediaBuffer = MFCreateMemoryBuffer(self.output_buffer_size)?;
            let s: IMFSample = MFCreateSample()?;
            s.AddBuffer(&buf)?;
            Some(s)
        } else {
            None
        };

        let mut output = MFT_OUTPUT_DATA_BUFFER {
            dwStreamID: 0,
            pSample: ManuallyDrop::new(pre_alloc_sample),
            dwStatus: 0,
            pEvents: ManuallyDrop::new(None),
        };

        let mut status = 0u32;

        match self.transform.ProcessOutput(0, std::slice::from_mut(&mut output), &mut status) {
            Ok(()) => {
                let sample_opt = ManuallyDrop::take(&mut output.pSample);
                if let Some(sample) = sample_opt {
                    let buffer: IMFMediaBuffer = sample.ConvertToContiguousBuffer()?;
                    let mut ptr: *mut u8 = std::ptr::null_mut();
                    let mut len = 0u32;
                    buffer.Lock(&mut ptr, None, Some(&mut len))?;
                    let data = std::slice::from_raw_parts(ptr, len as usize).to_vec();
                    buffer.Unlock()?;
                    Ok(Some(data))
                } else {
                    Ok(None)
                }
            }
            Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}

fn bgra_to_nv12(bgra: &[u8], width: u32, height: u32) -> Vec<u8> {
    let w = width as usize;
    let h = height as usize;
    let mut nv12 = vec![0u8; w * h * 3 / 2];

    // Y plane
    for row in 0..h {
        for col in 0..w {
            let s = (row * w + col) * 4;
            let b = bgra[s] as f32;
            let g = bgra[s + 1] as f32;
            let r = bgra[s + 2] as f32;
            nv12[row * w + col] = (0.257 * r + 0.504 * g + 0.098 * b + 16.5) as u8;
        }
    }

    // UV plane (interleaved, 2×2 subsampled)
    let uv_off = w * h;
    for row in (0..h).step_by(2) {
        for col in (0..w).step_by(2) {
            let s = (row * w + col) * 4;
            let b = bgra[s] as f32;
            let g = bgra[s + 1] as f32;
            let r = bgra[s + 2] as f32;
            let u = (-0.148 * r - 0.291 * g + 0.439 * b + 128.5) as u8;
            let v = (0.439 * r - 0.368 * g - 0.071 * b + 128.5) as u8;
            let idx = uv_off + (row / 2) * w + col;
            nv12[idx] = u;
            nv12[idx + 1] = v;
        }
    }

    nv12
}
