use anyhow::Result;
use std::collections::VecDeque;
use std::mem::ManuallyDrop;
use tracing::{info, warn};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::*;
use windows::core::Interface;

// GetEvent flag: return immediately if no event is queued
const MF_EVENT_FLAG_NO_WAIT: MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS =
    MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS(1);

pub struct H264Encoder {
    transform: IMFTransform,
    event_gen: Option<IMFMediaEventGenerator>, // Some = async hardware encoder
    _d3d_mgr: Option<IMFDXGIDeviceManager>,    // kept alive for hardware encoder
    width: u32,
    height: u32,
    timestamp: i64,
    provides_samples: bool,
    output_buffer_size: u32,
    need_input: bool,
    output_queue: VecDeque<Vec<u8>>,
    /// SPS + PPS in Annex B format, prepended to IDR frames.
    /// AMD puts these in MF_MT_MPEG_SEQUENCE_HEADER (only populated after the
    /// first frame is encoded) rather than embedding them inline.
    sequence_header: Vec<u8>,
    /// False until we have successfully delivered param sets to the client.
    headers_sent: bool,
    /// Input frame counter used to request IDR frames every GOP_SIZE frames.
    frame_count: u32,
}

// COM objects are safe to send across threads when using COINIT_MULTITHREADED
unsafe impl Send for H264Encoder {}

#[inline]
unsafe fn set_attr_u64(attr: &IMFMediaType, key: &windows::core::GUID, hi: u32, lo: u32) -> Result<()> {
    attr.SetUINT64(key, ((hi as u64) << 32) | lo as u64)?;
    Ok(())
}

impl H264Encoder {
    pub fn new(width: u32, height: u32) -> Result<Self> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            MFStartup(MF_VERSION, MFSTARTUP_FULL)?;

            let (transform, is_async) = Self::find_encoder()?;

            // Async hardware encoders must be unlocked before use.
            // Use GetAttributes() rather than QI for IMFAttributes — AMD encoders
            // don't expose IMFAttributes via QueryInterface (returns E_NOINTERFACE).
            if is_async {
                if let Ok(attrs) = transform.GetAttributes() {
                    let _ = attrs.SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1);
                }
            }

            // Set up D3D device manager for hardware encoders (required by NVENC/AMF)
            let d3d_mgr = if is_async {
                match Self::create_d3d_manager(&transform) {
                    Ok(mgr) => {
                        info!("D3D device manager set on hardware encoder");
                        Some(mgr)
                    }
                    Err(e) => {
                        warn!("Failed to set D3D manager: {e}");
                        None
                    }
                }
            } else {
                None
            };

            Self::init_encoder_with_mgr(transform, is_async, d3d_mgr, width, height)
        }
    }

    unsafe fn create_d3d_manager(transform: &IMFTransform) -> Result<IMFDXGIDeviceManager> {
        let mut device: Option<ID3D11Device> = None;
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            None,
            D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            None,
        )?;
        let device = device.unwrap();

        // Enable multithread protection required for MF + D3D11
        let mt: ID3D11Multithread = device.cast()?;
        let _ = mt.SetMultithreadProtected(true);

        let mut reset_token: u32 = 0;
        let mut mgr: Option<IMFDXGIDeviceManager> = None;
        MFCreateDXGIDeviceManager(&mut reset_token, &mut mgr)?;
        let mgr = mgr.unwrap();
        mgr.ResetDevice(&device, reset_token)?;

        // Pass device manager to the encoder (it will AddRef it internally)
        transform.ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, mgr.as_raw() as usize)?;

        Ok(mgr)
    }

    pub fn new_software(width: u32, height: u32) -> Result<Self> {
        unsafe {
            #[allow(non_upper_case_globals)]
            const CLSID_CMSH264EncoderMFT: windows::core::GUID = windows::core::GUID::from_values(
                0x49570228, 0x7d10, 0x4b93, [0xae, 0x7f, 0xaa, 0xaa, 0xd0, 0x35, 0x3b, 0x08],
            );
            let transform: IMFTransform = CoCreateInstance(
                &CLSID_CMSH264EncoderMFT,
                None,
                CLSCTX_INPROC_SERVER,
            )?;
            info!("Using software H.264 encoder");
            Self::init_encoder_with_mgr(transform, false, None, width, height)
        }
    }

    unsafe fn init_encoder_with_mgr(
        transform: IMFTransform,
        is_async: bool,
        d3d_mgr: Option<IMFDXGIDeviceManager>,
        width: u32,
        height: u32,
    ) -> Result<Self> {
        let out_type: IMFMediaType = MFCreateMediaType()?;
        out_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        out_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
        out_type.SetUINT32(&MF_MT_AVG_BITRATE, 10_000_000)?;
        out_type.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        set_attr_u64(&out_type, &MF_MT_FRAME_SIZE, width, height)?;
        set_attr_u64(&out_type, &MF_MT_FRAME_RATE, 30, 1)?;
        set_attr_u64(&out_type, &MF_MT_PIXEL_ASPECT_RATIO, 1, 1)?;
        transform.SetOutputType(0, &out_type, 0)?;

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

        let event_gen = if is_async {
            Some(transform.cast::<IMFMediaEventGenerator>()?)
        } else {
            None
        };

        // Read SPS+PPS from the output media type.  AMD's hardware encoder stores
        // these in MF_MT_MPEG_SEQUENCE_HEADER rather than embedding them in the
        // first IDR frame — the decoder needs them before it can decode anything.
        let sequence_header = Self::read_sequence_header(&transform);
        if !sequence_header.is_empty() {
            info!("Got sequence header ({} bytes)", sequence_header.len());
        }

        Ok(Self {
            transform,
            event_gen,
            _d3d_mgr: d3d_mgr,
            width,
            height,
            timestamp: 0,
            provides_samples,
            output_buffer_size,
            need_input: !is_async,
            output_queue: VecDeque::new(),
            sequence_header,
            headers_sent: false,
            frame_count: 0,
        })
    }

    /// Try to read the SPS+PPS blob from the encoder's current output media type.
    /// Returns an empty Vec if not present (software encoder includes them inline).
    unsafe fn read_sequence_header(transform: &IMFTransform) -> Vec<u8> {
        let Ok(out_type) = transform.GetOutputCurrentType(0) else { return Vec::new() };
        let mut buf = [0u8; 512];
        let mut size = 0u32;
        match out_type.GetBlob(
            &MF_MT_MPEG_SEQUENCE_HEADER,
            &mut buf,
            Some(&mut size),
        ) {
            Ok(()) => buf[..size as usize].to_vec(),
            Err(_) => Vec::new(),
        }
    }

    unsafe fn find_encoder() -> Result<(IMFTransform, bool)> {
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
                let is_async = activate
                    .cast::<IMFAttributes>()
                    .ok()
                    .and_then(|a| a.GetUINT32(&MF_TRANSFORM_ASYNC).ok())
                    .map(|v| v != 0)
                    .unwrap_or(false);

                match activate.ActivateObject::<IMFTransform>() {
                    Ok(transform) => {
                        CoTaskMemFree(Some(activates as *mut std::ffi::c_void));
                        info!("Using hardware H.264 encoder ({})", if is_async { "async" } else { "sync" });
                        return Ok((transform, is_async));
                    }
                    Err(e) => warn!("Hardware encoder activation failed: {e}"),
                }
            }
            CoTaskMemFree(Some(activates as *mut std::ffi::c_void));
        }

        warn!("No hardware H.264 encoder found, using software fallback");

        #[allow(non_upper_case_globals)]
        const CLSID_CMSH264EncoderMFT: windows::core::GUID = windows::core::GUID::from_values(
            0x49570228, 0x7d10, 0x4b93, [0xae, 0x7f, 0xaa, 0xaa, 0xd0, 0x35, 0x3b, 0x08],
        );

        let transform: IMFTransform = CoCreateInstance(
            &CLSID_CMSH264EncoderMFT,
            None,
            CLSCTX_INPROC_SERVER,
        )?;

        Ok((transform, false))
    }

    pub fn encode(&mut self, bgra: &[u8]) -> Result<Option<Vec<u8>>> {
        unsafe {
            if self.event_gen.is_some() {
                self.encode_async(bgra)
            } else {
                self.encode_sync(bgra)
            }
        }
    }

    unsafe fn encode_sync(&mut self, bgra: &[u8]) -> Result<Option<Vec<u8>>> {
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

        // Request IDR every 60 frames so the client can recover within ~2 s.
        self.frame_count += 1;
        if self.frame_count % 60 == 1 {
            let _ = sample.SetUINT32(&MFSampleExtension_CleanPoint, 1);
        }

        self.transform.ProcessInput(0, &sample, 0)?;
        self.get_output()
    }

    unsafe fn encode_async(&mut self, bgra: &[u8]) -> Result<Option<Vec<u8>>> {
        self.drain_events()?;

        if self.need_input {
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

            // Request IDR every 60 frames so the client can recover within ~2 s.
            self.frame_count += 1;
            if self.frame_count % 60 == 1 {
                let _ = sample.SetUINT32(&MFSampleExtension_CleanPoint, 1);
            }

            self.transform.ProcessInput(0, &sample, 0)?;
            self.need_input = false;

            // Drain again — sending input often triggers HaveOutput immediately
            self.drain_events()?;
        }

        Ok(self.output_queue.pop_front())
    }

    unsafe fn drain_events(&mut self) -> Result<()> {
        let event_gen = match self.event_gen.clone() {
            Some(eg) => eg,
            None => return Ok(()),
        };

        loop {
            match event_gen.GetEvent(MF_EVENT_FLAG_NO_WAIT) {
                Ok(event) => match event.GetType()? {
                    t if t == METransformNeedInput.0 as u32 => {
                        self.need_input = true;
                    }
                    t if t == METransformHaveOutput.0 as u32 => {
                        match self.get_output() {
                            Ok(Some(data)) => self.output_queue.push_back(data),
                            Ok(None) => {}
                            Err(e) => warn!("get_output error: {e}"),
                        }
                    }
                    _ => {}
                },
                Err(_) => break,
            }
        }
        Ok(())
    }

    unsafe fn get_output(&mut self) -> Result<Option<Vec<u8>>> {
        let pre_alloc: Option<IMFSample> = if !self.provides_samples {
            let buf: IMFMediaBuffer = MFCreateMemoryBuffer(self.output_buffer_size)?;
            let s: IMFSample = MFCreateSample()?;
            s.AddBuffer(&buf)?;
            Some(s)
        } else {
            None
        };

        let mut output = MFT_OUTPUT_DATA_BUFFER {
            dwStreamID: 0,
            pSample: ManuallyDrop::new(pre_alloc),
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
                    let bitstream = std::slice::from_raw_parts(ptr, len as usize).to_vec();
                    buffer.Unlock()?;

                    // Log first frame to help diagnose format issues
                    if !self.headers_sent && bitstream.len() >= 4 {
                        info!(
                            "First encoder output: {} bytes, first bytes: {:02X} {:02X} {:02X} {:02X}",
                            bitstream.len(), bitstream[0], bitstream[1], bitstream[2], bitstream[3]
                        );
                    }

                    // AMD async encoder emits P-frames before the first IDR.
                    // Lazily re-try reading the sequence header each frame.
                    if self.sequence_header.is_empty() {
                        let hdr = Self::read_sequence_header(&self.transform);
                        if !hdr.is_empty() {
                            info!("Got sequence header lazily ({} bytes)", hdr.len());
                            self.sequence_header = hdr;
                        }
                    }

                    let already_has_sps = annex_b_has_sps(&bitstream);
                    // Detect IDR by inspecting NAL type 5 directly — AMD does not
                    // reliably set MFSampleExtension_CleanPoint on output samples.
                    let has_idr_nal = annex_b_has_idr(&bitstream);

                    // Drop pre-IDR P-frames: the client cannot decode them without a
                    // preceding IDR, and AMD async encoders often produce a run of
                    // P-frames at startup before the first keyframe.
                    if !has_idr_nal && !already_has_sps && !self.headers_sent {
                        return Ok(None);
                    }

                    // Prepend SPS+PPS to every IDR frame that doesn't already carry them.
                    // Do this unconditionally on IDR detection (not via CleanPoint) so that
                    // periodic keyframes also carry parameter sets — required for the client
                    // decoder to reset its reference state correctly.
                    let data = if has_idr_nal && !already_has_sps && !self.sequence_header.is_empty() {
                        self.headers_sent = true;
                        let mut v = self.sequence_header.clone();
                        v.extend_from_slice(&bitstream);
                        v
                    } else {
                        if already_has_sps || has_idr_nal {
                            self.headers_sent = true;
                        }
                        bitstream
                    };

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

/// Returns true if the Annex B bitstream contains an IDR slice NAL (type 5).
fn annex_b_has_idr(data: &[u8]) -> bool {
    annex_b_has_nal_type(data, 5)
}

/// Returns true if the Annex B bitstream already contains a SPS NAL unit
/// (NAL type 7), meaning the encoder embedded param sets inline.
fn annex_b_has_sps(data: &[u8]) -> bool {
    annex_b_has_nal_type(data, 7)
}

fn annex_b_has_nal_type(data: &[u8], nal_type: u8) -> bool {
    let mut i = 0;
    while i + 3 < data.len() {
        let nal_offset = if data[i..].starts_with(&[0, 0, 0, 1]) {
            i + 4
        } else if data[i..].starts_with(&[0, 0, 1]) {
            i + 3
        } else {
            i += 1;
            continue;
        };
        if nal_offset < data.len() && (data[nal_offset] & 0x1F) == nal_type {
            return true;
        }
        i += 1;
    }
    false
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
