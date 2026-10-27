//! The couch menu's pass on D3D11 (docs/couch-menu-design.md 6.3): while an
//! overlay is set, each plane's closed draw is followed by its `menu_*`
//! pixel shader, which discards outside the panel, or the closed dispatch by
//! its `*_menu_cs` entry over the panel's blocks. Closed, nothing here runs.
use super::*;
use crate::overlay::Overlay;

#[derive(Default)]
pub(super) struct Menu {
    overlay: Option<Overlay>,
    /// Made on the first open, by entry.
    pixel: Vec<(&'static [u8], ID3D11PixelShader)>,
    compute: Vec<(&'static [u8], ID3D11ComputeShader)>,
    /// `cbuffer Menu` and the values it holds.
    constants: Option<(ID3D11Buffer, [u32; 12])>,
    /// The panel image, its size and revision.
    image: Option<(ID3D11Texture2D, ID3D11ShaderResourceView, [u32; 2], u64)>,
    /// Images uploaded and passes run, for the tests.
    pub(super) uploads: u64,
    pub(super) passes: u64,
}
/// One conversion's menu pass: what to upload inside the context lock, and
/// what to bind.
pub(super) struct Pass {
    constants: ID3D11Buffer,
    view: ID3D11ShaderResourceView,
    texture: ID3D11Texture2D,
    write_constants: Option<[u32; 12]>,
    /// A new panel image and its row pitch.
    write_image: Option<(Arc<[u8]>, u32)>,
    pub(super) pixel: Vec<ID3D11PixelShader>,
    compute: Option<ID3D11ComputeShader>,
    groups: [u32; 2],
}
impl Menu {
    pub(super) fn set(&mut self, overlay: Option<&Overlay>) {
        if self.overlay.as_ref() != overlay {
            self.overlay = overlay.cloned();
        }
    }
    /// The next conversion's pass with the `pixel` shaders, or the
    /// `compute` entry whose threads cover `block`-sized target blocks;
    /// `None` while closed. Everything fallible happens here, outside the
    /// context lock.
    pub(super) fn prepare(
        &mut self,
        gpu: &Device,
        values: &[u32; 20],
        block: [u32; 2],
        pixel: &[&'static [u8]],
        compute: Option<&'static [u8]>,
    ) -> Result<Option<Pass>> {
        let Some(overlay) = &self.overlay else {
            return Ok(None);
        };
        overlay.validate([values[2], values[3]])?;
        let started = std::time::Instant::now();
        let mut made = false;
        // SAFETY: `gpu.device` is a live D3D11 device, `compile` gets NUL-terminated names, and
        // every descriptor outlives the call that reads it.
        unsafe {
            let mut shaders = Vec::with_capacity(pixel.len());
            for &entry in pixel {
                if let Some((_, shader)) = self.pixel.iter().find(|(e, _)| *e == entry) {
                    shaders.push(shader.clone());
                    continue;
                }
                let mut shader = None;
                gpu.device.CreatePixelShader(
                    &compile(entry, b"ps_5_0\0")?,
                    None,
                    Some(&mut shader),
                )?;
                let shader = shader.context("no couch menu pixel shader")?;
                self.pixel.push((entry, shader.clone()));
                shaders.push(shader);
                made = true;
            }
            let compute = match compute {
                Some(entry) => Some(match self.compute.iter().find(|(e, _)| *e == entry) {
                    Some((_, shader)) => shader.clone(),
                    None => {
                        let mut shader = None;
                        gpu.device.CreateComputeShader(
                            &compile(entry, b"cs_5_0\0")?,
                            None,
                            Some(&mut shader),
                        )?;
                        let shader = shader.context("no couch menu compute shader")?;
                        self.compute.push((entry, shader.clone()));
                        made = true;
                        shader
                    }
                }),
                None => None,
            };
            let values = overlay.constants(block);
            let write_constants =
                (self.constants.as_ref().map(|(_, v)| *v) != Some(values)).then_some(values);
            if self.constants.is_none() {
                let mut buffer = None;
                gpu.device.CreateBuffer(
                    &D3D11_BUFFER_DESC {
                        ByteWidth: 48,
                        Usage: D3D11_USAGE_DEFAULT,
                        BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                        ..Default::default()
                    },
                    None,
                    Some(&mut buffer),
                )?;
                self.constants = Some((buffer.context("no couch menu constants")?, values));
            }
            let size = overlay.image_size();
            if self.image.as_ref().is_none_or(|image| image.2 != size) {
                let mut texture = None;
                gpu.device.CreateTexture2D(
                    &D3D11_TEXTURE2D_DESC {
                        Width: size[0],
                        Height: size[1],
                        MipLevels: 1,
                        ArraySize: 1,
                        Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                        SampleDesc: DXGI_SAMPLE_DESC {
                            Count: 1,
                            Quality: 0,
                        },
                        Usage: D3D11_USAGE_DEFAULT,
                        BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                        ..Default::default()
                    },
                    None,
                    Some(&mut texture),
                )?;
                let texture = texture.context("no couch menu texture")?;
                let mut view = None;
                gpu.device
                    .CreateShaderResourceView(&texture, None, Some(&mut view))?;
                let view = view.context("no couch menu view")?;
                // No revision yet: the image is uploaded below.
                self.image = Some((texture, view, size, u64::MAX));
            }
            let (buffer, written) = self.constants.as_mut().expect("made above");
            *written = values;
            let (texture, view, _, revision) = self.image.as_mut().expect("made above");
            let write_image = (*revision != overlay.revision).then(|| {
                *revision = overlay.revision;
                self.uploads += 1;
                (overlay.rgba.clone(), size[0] * 4)
            });
            if made {
                tracing::info!(
                    elapsed_ms = started.elapsed().as_secs_f64() * 1e3,
                    "couch menu: D3D11 pass made"
                );
            }
            self.passes += 1;
            Ok(Some(Pass {
                constants: buffer.clone(),
                view: view.clone(),
                texture: texture.clone(),
                write_constants,
                write_image,
                pixel: shaders,
                compute,
                groups: [values[6].div_ceil(8), values[7].div_ceil(8)],
            }))
        }
    }
}
impl Pass {
    /// Inside the context lock: upload what changed.
    ///
    /// # Safety
    ///
    /// The context is this device's and locked.
    pub(super) unsafe fn update(&self, context: &ID3D11DeviceContext) {
        // SAFETY: `values` is the buffer's 48 bytes and `rgba` the texture's rows at `pitch`.
        unsafe {
            if let Some(values) = &self.write_constants {
                context.UpdateSubresource(&self.constants, 0, None, values.as_ptr().cast(), 0, 0);
            }
            if let Some((rgba, pitch)) = &self.write_image {
                context.UpdateSubresource(&self.texture, 0, None, rgba.as_ptr().cast(), *pitch, 0);
            }
        }
    }
    /// Bind `cbuffer Menu` (b2) and the panel image (t2) for the pixel
    /// shaders.
    ///
    /// # Safety
    ///
    /// The context is this device's and locked.
    pub(super) unsafe fn bind(&self, context: &ID3D11DeviceContext) {
        // SAFETY: As the caller guarantees.
        unsafe {
            context.PSSetConstantBuffers(2, Some(&[Some(self.constants.clone())]));
            context.PSSetShaderResources(2, Some(&[Some(self.view.clone())]));
        }
    }
    /// # Safety
    ///
    /// The context is this device's and locked.
    pub(super) unsafe fn unbind(context: &ID3D11DeviceContext) {
        // SAFETY: As the caller guarantees.
        unsafe { context.PSSetShaderResources(2, Some(&[None])) };
    }
    /// After the closed dispatch, with its `Config` and unordered views
    /// still bound: the panel's blocks again.
    ///
    /// # Safety
    ///
    /// The context is this device's and locked.
    pub(super) unsafe fn dispatch(&self, context: &ID3D11DeviceContext) {
        let Some(shader) = &self.compute else {
            return;
        };
        // SAFETY: As the caller guarantees.
        unsafe {
            context.CSSetShader(shader, None);
            context.CSSetConstantBuffers(2, Some(&[Some(self.constants.clone())]));
            context.CSSetShaderResources(2, Some(&[Some(self.view.clone())]));
            context.Dispatch(self.groups[0], self.groups[1], 1);
            context.CSSetShaderResources(2, Some(&[None]));
        }
    }
}
