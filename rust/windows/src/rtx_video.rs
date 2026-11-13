//! RTX Video enhancement through the NVIDIA build's NGX adapter: NVIDIA's
//! video super resolution (VSR) at the capture's own size, which sharpens and
//! deblocks the picture before encoding. SDR pictures go to the model as
//! they are; an FP16 HDR picture stays HDR, its SDR proxy is enhanced and
//! only the bounded change is added back (`shaders/rtx_video.hlsl`, after
//! Foundation Sunshine's DLSS NR HDR filter). Creation, enhancement and
//! teardown stay on one thread.
use crate::capture::{Device, GpuImage, GpuPool, Pixel};
use crate::nvidia_enhance::describe;
use anyhow::{Context, Result, bail};
use std::{ffi::c_void, ptr};
use windows::{
    Win32::Graphics::{
        Direct3D::{D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST, Fxc::*},
        Direct3D11::*,
        Dxgi::Common::*,
    },
    core::{Interface, PCSTR},
};

const SHADER: &str = include_str!("shaders/rtx_video.hlsl");
type Create = unsafe extern "C" fn(*mut c_void, *mut u32) -> *mut c_void;
type Convert = unsafe extern "C" fn(*mut c_void, *mut c_void, u32, *mut u32) -> *mut c_void;
type Destroy = unsafe extern "C" fn(*mut c_void);

/// Waits for the capture's copy and orders D3D11 work after it: NGX and the
/// passes below read the texture on the D3D11 context.
pub(crate) fn ready_for_d3d11(image: &GpuImage) -> Result<()> {
    if let Some(ready) = &image.ready {
        ready
            .wait()
            .context("wait for the captured picture before enhancing it")?;
    }
    image.order_d3d11()
}

pub struct Filter {
    state: *mut c_void,
    convert: Convert,
    destroy: Destroy,
    device: Device,
    quality: u32,
    pool: GpuPool,
    hdr: Option<HdrPasses>,
    _dll: libloading::Library,
}
impl Filter {
    /// An enhancer on the capture's device at VSR quality level `quality`
    /// (`butterpollo_core::nvidia_enhance::rtx_video_quality`).
    pub fn new_gpu(image: &GpuImage, quality: u32) -> Result<Self> {
        let device = image.gpu.clone();
        if device.vendor != 0x10de {
            bail!("RTX Video enhancement needs an NVIDIA GPU");
        }
        let dll = crate::nvidia_enhance::load()?;
        // SAFETY: The adapter's ABI 2 (checked by `load`) exports these symbols with these types,
        // and Filter keeps `dll` and `device` while the pointers are used.
        unsafe {
            let create = *dll.get::<Create>(b"butterpollo_vsr_create\0")?;
            let convert = *dll.get::<Convert>(b"butterpollo_vsr_convert\0")?;
            let destroy = *dll.get::<Destroy>(b"butterpollo_vsr_destroy\0")?;
            let mut error = 0;
            let state = create(device.device.as_raw(), &mut error);
            if state.is_null() {
                bail!("RTX Video enhancement could not start: {}", describe(error));
            }
            Ok(Self {
                state,
                convert,
                destroy,
                device,
                quality: quality.clamp(1, 4),
                pool: GpuPool::default(),
                hdr: None,
                _dll: dll,
            })
        }
    }
    pub fn set_quality(&mut self, quality: u32) {
        self.quality = quality.clamp(1, 4);
    }
    /// The enhanced picture, in the capture's format. NGX's output is copied
    /// into a bounded pool before another enhancement can overwrite it.
    pub fn apply_gpu(&mut self, image: &GpuImage) -> Result<GpuImage> {
        if image.gpu.device.as_raw() != self.device.device.as_raw() {
            bail!("RTX Video enhancement needs a picture on the capture's GPU");
        }
        if !matches!(image.pixel, Pixel::Bgra8 | Pixel::RgbaF16) {
            bail!("RTX Video enhancement reads 8-bit SDR and FP16 HDR pictures only");
        }
        ready_for_d3d11(image)?;
        // SAFETY: `state` is the live NGX state on this device, whose multithread lock is held for
        // the whole enhancement; NGX's output is only borrowed while it is read.
        unsafe {
            let lock: ID3D11Multithread = self.device.context.cast()?;
            lock.Enter();
            let result = self.enhance(image);
            lock.Leave();
            result
        }
    }
    /// # Safety
    /// The caller holds the device's multithread lock.
    unsafe fn enhance(&mut self, image: &GpuImage) -> Result<GpuImage> {
        unsafe {
            let texture = if image.pixel == Pixel::Bgra8 {
                self.vsr(image.texture.as_raw())?
            } else {
                if self.hdr.is_none() {
                    self.hdr = Some(HdrPasses::new(&self.device)?);
                }
                let passes = self.hdr.as_mut().unwrap();
                passes.size(&self.device, image.width, image.height)?;
                let original = view(&self.device, image.texture.as_ref())?;
                passes.prepare(&self.device, &original, image.width, image.height);
                let proxy = passes.proxy.as_raw();
                let enhanced = self.vsr(proxy)?;
                let passes = self.hdr.as_mut().unwrap();
                passes.resolve(
                    &self.device,
                    &original,
                    &enhanced,
                    image.width,
                    image.height,
                )?;
                passes.output.clone()
            };
            let mut enhanced = self
                .pool
                .copy(&self.device, &texture)?
                .context("the enhanced pictures are all still being encoded")?;
            enhanced.inherit(image);
            Ok(enhanced)
        }
    }
    /// NGX's VSR over `input` at its own size.
    unsafe fn vsr(&mut self, input: *mut c_void) -> Result<ID3D11Texture2D> {
        unsafe {
            let mut error = 0;
            let output = (self.convert)(self.state, input, self.quality, &mut error);
            let texture = ID3D11Texture2D::from_raw_borrowed(&output)
                .with_context(|| format!("RTX Video enhancement failed: {}", describe(error)))?;
            Ok(texture.clone())
        }
    }
}
impl Drop for Filter {
    fn drop(&mut self) {
        // SAFETY: `state` came from create and is destroyed once, here; `_dll` is dropped after
        // this.
        unsafe {
            (self.destroy)(self.state);
        }
        self.state = ptr::null_mut();
    }
}

/// A shader resource view of `texture`.
unsafe fn view(device: &Device, texture: &ID3D11Texture2D) -> Result<ID3D11ShaderResourceView> {
    let mut view = None;
    // SAFETY: `texture` is a live texture on `device`.
    unsafe {
        device
            .device
            .CreateShaderResourceView(texture, None, Some(&mut view))
            .context("the picture cannot be read by a shader")?;
    }
    view.context("D3D11 returned no shader view")
}

/// The HDR picture's two passes around the model, and their textures.
struct HdrPasses {
    vertex: ID3D11VertexShader,
    prepare: ID3D11PixelShader,
    resolve: ID3D11PixelShader,
    proxy: ID3D11Texture2D,
    proxy_view: ID3D11ShaderResourceView,
    proxy_target: ID3D11RenderTargetView,
    output: ID3D11Texture2D,
    output_target: ID3D11RenderTargetView,
    shape: (u32, u32),
}
impl HdrPasses {
    fn new(device: &Device) -> Result<Self> {
        // SAFETY: `device` is a live D3D11 device; each bytecode blob outlives its call.
        unsafe {
            let mut vertex = None;
            let mut prepare = None;
            let mut resolve = None;
            device.device.CreateVertexShader(
                &compile(b"vertex\0", b"vs_5_0\0")?,
                None,
                Some(&mut vertex),
            )?;
            device.device.CreatePixelShader(
                &compile(b"prepare\0", b"ps_5_0\0")?,
                None,
                Some(&mut prepare),
            )?;
            device.device.CreatePixelShader(
                &compile(b"resolve\0", b"ps_5_0\0")?,
                None,
                Some(&mut resolve),
            )?;
            let (proxy, proxy_view, proxy_target) =
                target(device, 1, 1, DXGI_FORMAT_B8G8R8A8_UNORM)?;
            let (output, _, output_target) = target(device, 1, 1, DXGI_FORMAT_R16G16B16A16_FLOAT)?;
            Ok(Self {
                vertex: vertex.context("no vertex shader")?,
                prepare: prepare.context("no prepare shader")?,
                resolve: resolve.context("no resolve shader")?,
                proxy,
                proxy_view,
                proxy_target,
                output,
                output_target,
                shape: (1, 1),
            })
        }
    }
    /// Makes the proxy and output textures `width` by `height`.
    fn size(&mut self, device: &Device, width: u32, height: u32) -> Result<()> {
        if self.shape == (width, height) {
            return Ok(());
        }
        (self.proxy, self.proxy_view, self.proxy_target) =
            target(device, width, height, DXGI_FORMAT_B8G8R8A8_UNORM)?;
        let output = target(device, width, height, DXGI_FORMAT_R16G16B16A16_FLOAT)?;
        (self.output, self.output_target) = (output.0, output.2);
        self.shape = (width, height);
        Ok(())
    }
    /// # Safety
    /// The caller holds the device's multithread lock.
    unsafe fn draw(
        &self,
        device: &Device,
        shader: &ID3D11PixelShader,
        views: &[Option<ID3D11ShaderResourceView>],
        target: &ID3D11RenderTargetView,
        width: u32,
        height: u32,
    ) {
        let context = &device.context;
        // SAFETY: every bound object is live on this device; the bindings are cleared after the
        // draw, and the other passes on this context set their own state.
        unsafe {
            context.IASetInputLayout(None);
            context.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            context.VSSetShader(&self.vertex, None);
            context.RSSetState(None);
            context.OMSetBlendState(None, None, u32::MAX);
            context.RSSetViewports(Some(&[D3D11_VIEWPORT {
                Width: width as f32,
                Height: height as f32,
                MinDepth: 0.,
                MaxDepth: 1.,
                ..Default::default()
            }]));
            context.PSSetShaderResources(0, Some(views));
            context.OMSetRenderTargets(Some(&[Some(target.clone())]), None);
            context.PSSetShader(shader, None);
            context.Draw(3, 0);
            context.PSSetShaderResources(0, Some(&[None, None, None]));
            context.OMSetRenderTargets(None, None);
        }
    }
    unsafe fn prepare(
        &self,
        device: &Device,
        original: &ID3D11ShaderResourceView,
        width: u32,
        height: u32,
    ) {
        unsafe {
            self.draw(
                device,
                &self.prepare,
                &[Some(original.clone())],
                &self.proxy_target,
                width,
                height,
            );
        }
    }
    unsafe fn resolve(
        &self,
        device: &Device,
        original: &ID3D11ShaderResourceView,
        enhanced: &ID3D11Texture2D,
        width: u32,
        height: u32,
    ) -> Result<()> {
        unsafe {
            let enhanced = view(device, enhanced)?;
            self.draw(
                device,
                &self.resolve,
                &[
                    Some(original.clone()),
                    Some(self.proxy_view.clone()),
                    Some(enhanced),
                ],
                &self.output_target,
                width,
                height,
            );
            Ok(())
        }
    }
}

/// A render target texture with its shader and target views.
fn target(
    device: &Device,
    width: u32,
    height: u32,
    format: DXGI_FORMAT,
) -> Result<(
    ID3D11Texture2D,
    ID3D11ShaderResourceView,
    ID3D11RenderTargetView,
)> {
    // SAFETY: `device` is a live D3D11 device and the descriptor outlives the call.
    unsafe {
        let mut texture = None;
        device.device.CreateTexture2D(
            &D3D11_TEXTURE2D_DESC {
                Width: width,
                Height: height,
                MipLevels: 1,
                ArraySize: 1,
                Format: format,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0) as u32,
                ..Default::default()
            },
            None,
            Some(&mut texture),
        )?;
        let texture = texture.context("D3D11 returned no texture")?;
        let shader = view(device, &texture)?;
        let mut render = None;
        device
            .device
            .CreateRenderTargetView(&texture, None, Some(&mut render))?;
        Ok((
            texture,
            shader,
            render.context("D3D11 returned no render view")?,
        ))
    }
}

/// `entry` of `shaders/rtx_video.hlsl` for `target`, compiled when the
/// enhancement first meets an HDR picture.
///
/// # Safety
///
/// `entry` and `target` must be NUL-terminated.
unsafe fn compile(entry: &'static [u8], target: &'static [u8]) -> Result<Vec<u8>> {
    // SAFETY: `SHADER` is passed with its exact length, `entry` and `target` are NUL-terminated,
    // and each blob is read only within GetBufferSize bytes while it is alive.
    unsafe {
        let mut code = None;
        let mut errors = None;
        let result = D3DCompile(
            SHADER.as_ptr().cast(),
            SHADER.len(),
            PCSTR::null(),
            None,
            None,
            PCSTR(entry.as_ptr()),
            PCSTR(target.as_ptr()),
            D3DCOMPILE_OPTIMIZATION_LEVEL3,
            0,
            &mut code,
            Some(&mut errors),
        );
        if let Err(error) = result {
            let detail = errors
                .map(|blob| {
                    String::from_utf8_lossy(std::slice::from_raw_parts(
                        blob.GetBufferPointer().cast(),
                        blob.GetBufferSize(),
                    ))
                    .into_owned()
                })
                .unwrap_or_default();
            bail!("RTX Video shader compilation: {error}: {detail}");
        }
        let code = code.context("shader compiler returned no bytecode")?;
        Ok(
            std::slice::from_raw_parts(code.GetBufferPointer().cast::<u8>(), code.GetBufferSize())
                .to_vec(),
        )
    }
}

#[cfg(test)]
mod tests {
    /// The HLSL compiles for every entry the passes use, without a GPU.
    #[test]
    fn hdr_passes_compile() {
        let entries: [(&'static [u8], &'static [u8]); 3] = [
            (b"vertex\0", b"vs_5_0\0"),
            (b"prepare\0", b"ps_5_0\0"),
            (b"resolve\0", b"ps_5_0\0"),
        ];
        for (entry, target) in entries {
            // SAFETY: both names are NUL-terminated.
            let code = unsafe { super::compile(entry, target) };
            assert!(!code.unwrap().is_empty());
        }
    }
}
