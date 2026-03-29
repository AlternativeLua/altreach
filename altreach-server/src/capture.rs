use windows::Win32::Graphics::{
    Direct3D11::*,
    Dxgi::*,
    Dxgi::Common::*,
};
use windows::Win32::Graphics::Direct3D::*;
use anyhow::Result;
use windows::core::Interface;
use windows::Win32::Foundation::RECT;
use altreach_proto::FramePatch;

pub struct Capturer {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    duplication: IDXGIOutputDuplication,
    staging: Option<ID3D11Texture2D>,
    staging_size: (u32, u32),
}

impl Capturer {
    pub fn new() -> Result<Self> {
        unsafe {
            let mut device: Option<ID3D11Device> = None;
            let mut context: Option<ID3D11DeviceContext> = None;

            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                None,
                D3D11_CREATE_DEVICE_FLAG(0),
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )?;

            let device = device.unwrap();
            let context = context.unwrap();

            let dxgi_device: IDXGIDevice = device.cast()?;
            let adapter: IDXGIAdapter = dxgi_device.GetAdapter()?;
            let output: IDXGIOutput = adapter.EnumOutputs(0)?;
            let output1: IDXGIOutput1 = output.cast()?;
            let duplication = output1.DuplicateOutput(&device)?;

            Ok(Self { device, context, duplication, staging: None, staging_size: (0, 0) })
        }
    }

    pub fn capture_full(&mut self) -> Result<Option<(u32, u32, Vec<FramePatch>)>> {
        unsafe {
            let mut frame_info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut resource: Option<IDXGIResource> = None;
            self.duplication.AcquireNextFrame(500, &mut frame_info, &mut resource)?;

            let gpu_texture: ID3D11Texture2D = resource.unwrap().cast()?;

            let mut desc = D3D11_TEXTURE2D_DESC::default();
            gpu_texture.GetDesc(&mut desc);
            let width = desc.Width;
            let height = desc.Height;

            if self.staging.is_none() || self.staging_size != (width, height) {
                desc.Usage = D3D11_USAGE_STAGING;
                desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
                desc.BindFlags = 0;
                desc.MiscFlags = 0;
                desc.MipLevels = 1;
                desc.ArraySize = 1;
                desc.SampleDesc = DXGI_SAMPLE_DESC { Count: 1, Quality: 0 };

                let mut staging: Option<ID3D11Texture2D> = None;
                self.device.CreateTexture2D(&desc, None, Some(&mut staging))?;
                self.staging = staging;
                self.staging_size = (width, height);
            }

            let staging = self.staging.as_ref().unwrap();
            self.context.CopyResource(staging, &gpu_texture);

            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.context.Map(&staging.cast::<ID3D11Resource>()?, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;

            let row_pitch = mapped.RowPitch as usize;
            let data = mapped.pData as *const u8;
            let mut pixels = Vec::with_capacity((width * height * 4) as usize);

            for row in 0..height as usize {
                let src = std::slice::from_raw_parts(data.add(row * row_pitch), width as usize * 4);
                pixels.extend_from_slice(src);
            }

            self.context.Unmap(&staging.cast::<ID3D11Resource>()?, 0);
            self.duplication.ReleaseFrame()?;

            let patch = FramePatch {
                x: 0, y: 0, width, height,
                data: lz4_flex::compress_prepend_size(&pixels),
            };

            Ok(Some((width, height, vec![patch])))
        }
    }

    pub fn capture_frame(&mut self) -> Result<Option<(u32, u32, Vec<FramePatch>)>> {
        unsafe {
            let mut frame_info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut resource: Option<IDXGIResource> = None;
            self.duplication.AcquireNextFrame(500, &mut frame_info, &mut resource)?;

            if frame_info.AccumulatedFrames == 0 {
                self.duplication.ReleaseFrame()?;
                return Ok(None);
            }

            let mut size_needed: u32 = 0;
            let mut rects: Vec<RECT> = Vec::new();
            loop {
                let result = self.duplication.GetFrameDirtyRects(
                    (rects.len() * std::mem::size_of::<RECT>()) as u32,
                    rects.as_mut_ptr(),
                    &mut size_needed,
                );
                match result {
                    Ok(_) => break,
                    Err(e) if e.code().0 as u32 == 0x887A0003 => {
                        let count = size_needed as usize / std::mem::size_of::<RECT>();
                        rects.resize(count, RECT::default());
                    }
                    Err(e) => {
                        self.duplication.ReleaseFrame()?;
                        return Err(e.into());
                    }
                }
            }

            if rects.is_empty() {
                self.duplication.ReleaseFrame()?;
                return Ok(None);
            }

            let gpu_texture: ID3D11Texture2D = resource.unwrap().cast()?;

            let mut desc = D3D11_TEXTURE2D_DESC::default();
            gpu_texture.GetDesc(&mut desc);
            let width = desc.Width;
            let height = desc.Height;

            if self.staging.is_none() || self.staging_size != (width, height) {
                desc.Usage = D3D11_USAGE_STAGING;
                desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
                desc.BindFlags = 0;
                desc.MiscFlags = 0;
                desc.MipLevels = 1;
                desc.ArraySize = 1;
                desc.SampleDesc = DXGI_SAMPLE_DESC { Count: 1, Quality: 0 };

                let mut staging: Option<ID3D11Texture2D> = None;
                self.device.CreateTexture2D(&desc, None, Some(&mut staging))?;
                self.staging = staging;
                self.staging_size = (width, height);
            }

            let staging = self.staging.as_ref().unwrap();
            self.context.CopyResource(staging, &gpu_texture);

            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.context.Map(&staging.cast::<ID3D11Resource>()?, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;

            let row_pitch = mapped.RowPitch as usize;
            let data = mapped.pData as *const u8;
            let mut patches: Vec<FramePatch> = Vec::new();

            for rect in &rects {
                let x = rect.left as u32;
                let y = rect.top as u32;
                let w = (rect.right - rect.left) as u32;
                let h = (rect.bottom - rect.top) as u32;

                let mut patch_pixels = Vec::with_capacity((w * h * 4) as usize);
                for row in 0..h as usize {
                    let offset = (y as usize + row) * row_pitch + x as usize * 4;
                    let src = std::slice::from_raw_parts(data.add(offset), w as usize * 4);
                    patch_pixels.extend_from_slice(src);
                }

                patches.push(FramePatch {
                    x, y, width: w, height: h,
                    data: lz4_flex::compress_prepend_size(&patch_pixels),
                });
            }

            self.context.Unmap(&staging.cast::<ID3D11Resource>()?, 0);
            self.duplication.ReleaseFrame()?;

            Ok(Some((width, height, patches)))
        }
    }
}
