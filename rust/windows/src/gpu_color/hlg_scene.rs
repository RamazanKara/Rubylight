//! The scene-linear HLG prepass for AMF's video converter (`amf::vpe`):
//! `hlg_scene_cs` in color.hlsl turns a capture (scRGB or PQ) into
//! scene-linear BT.2020 light in an RGBA16F texture AMF reads. After
//! Foundation Sunshine's HLG preparation for its `amd_vpe` path
//! (AlkaidLab/foundation-sunshine PR #1216, commit 1add1ed), GPL-3.0.
use super::{SourceViews, compile, constants, source_view};
use crate::capture::{Device, GpuImage, Pixel};
use anyhow::{Context, Result, bail};
use std::sync::Arc;
use windows::Win32::Graphics::{Direct3D11::*, Dxgi::Common::*};

/// Scene textures in flight at most: AMF holds an input until its output.
const RING: usize = 4;

pub(crate) struct HlgScene {
    gpu: Device,
    lock: ID3D11Multithread,
    shader: ID3D11ComputeShader,
    constants: ID3D11Buffer,
    size: (u32, u32),
    targets: Vec<(Arc<ID3D11Texture2D>, ID3D11UnorderedAccessView)>,
    sources: SourceViews,
}
impl HlgScene {
    pub fn new(
        gpu: &Device,
        config: &butterpollo_core::rtsp::Negotiated,
        source: (u32, u32, Pixel),
    ) -> Result<Self> {
        if source.2 == Pixel::Bgra8 {
            bail!("the HLG prepass reads HDR captures only");
        }
        let values = constants(config, source, [100., 1.], None);
        // SAFETY: `gpu.device` is live, `compile` gets NUL-terminated names, and the descriptor
        // and the 80-byte `values` outlive the calls that read them.
        unsafe {
            let mut shader = None;
            gpu.device.CreateComputeShader(
                &compile(b"hlg_scene_cs\0", b"cs_5_0\0")?,
                None,
                Some(&mut shader),
            )?;
            let mut buffer = None;
            gpu.device.CreateBuffer(
                &D3D11_BUFFER_DESC {
                    ByteWidth: 80,
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                    ..Default::default()
                },
                Some(&D3D11_SUBRESOURCE_DATA {
                    pSysMem: values.as_ptr().cast(),
                    ..Default::default()
                }),
                Some(&mut buffer),
            )?;
            Ok(Self {
                gpu: gpu.clone(),
                lock: windows::core::Interface::cast(&gpu.context)?,
                shader: shader.context("no HLG prepass shader")?,
                constants: buffer.context("no HLG prepass constants")?,
                size: (source.0, source.1),
                targets: vec![],
                sources: Default::default(),
            })
        }
    }
    fn target(&mut self) -> Result<usize> {
        if let Some(index) = self
            .targets
            .iter()
            .position(|(texture, _)| Arc::strong_count(texture) == 1)
        {
            return Ok(index);
        }
        if self.targets.len() >= RING {
            bail!("HLG prepass textures are all still held by AMF");
        }
        // SAFETY: `gpu.device` is live and the descriptor outlives the calls.
        unsafe {
            let mut texture = None;
            self.gpu.device.CreateTexture2D(
                &D3D11_TEXTURE2D_DESC {
                    Width: self.size.0,
                    Height: self.size.1,
                    MipLevels: 1,
                    ArraySize: 1,
                    Format: DXGI_FORMAT_R16G16B16A16_FLOAT,
                    SampleDesc: DXGI_SAMPLE_DESC {
                        Count: 1,
                        Quality: 0,
                    },
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: (D3D11_BIND_UNORDERED_ACCESS.0 | D3D11_BIND_SHADER_RESOURCE.0)
                        as u32,
                    ..Default::default()
                },
                None,
                Some(&mut texture),
            )?;
            let texture = texture.context("no HLG prepass texture")?;
            let mut view = None;
            self.gpu
                .device
                .CreateUnorderedAccessView(&texture, None, Some(&mut view))?;
            self.targets
                .push((Arc::new(texture), view.context("no HLG prepass view")?));
        }
        Ok(self.targets.len() - 1)
    }
    /// Run the prepass on `image`; AMF reads the returned texture.
    pub fn run(&mut self, image: &GpuImage) -> Result<Arc<ID3D11Texture2D>> {
        if (image.width, image.height) != self.size {
            bail!("HLG prepass made for another capture size");
        }
        image.order_d3d11()?;
        let index = self.target()?;
        let source = source_view(&mut self.sources, &self.gpu, image)?;
        let (texture, view) = &self.targets[index];
        // SAFETY: The context is held from Enter to Leave with no early return between, and every
        // bound object is on this device.
        unsafe {
            let context = &self.gpu.context;
            self.lock.Enter();
            context.CSSetShader(&self.shader, None);
            context.CSSetShaderResources(0, Some(&[Some(source)]));
            context.CSSetConstantBuffers(0, Some(&[Some(self.constants.clone())]));
            context.CSSetUnorderedAccessViews(9, 1, Some([Some(view.clone())].as_ptr()), None);
            context.Dispatch(self.size.0.div_ceil(16), self.size.1.div_ceil(16), 1);
            context.CSSetUnorderedAccessViews(9, 1, Some([None].as_ptr()), None);
            context.CSSetShaderResources(0, Some(&[None]));
            context.CSSetShader(None, None);
            context.Flush();
            self.lock.Leave();
        }
        Ok(texture.clone())
    }
}
