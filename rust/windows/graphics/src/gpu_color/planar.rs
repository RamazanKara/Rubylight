//! D3D11 three-plane Y, Cb, Cr texture allocation and fallback conversion for
//! PyroWave. The D3D12 compute path allocates only the shared plane textures
//! through [`planar_textures`] and never compiles D3D11 conversion shaders;
//! [`PlanarConverter`] compiles `pyro_cs` upfront and builds its pixel-shader
//! fallback lazily only if `pyro_cs` is unavailable.

use super::*;

/// Dimensions and DXGI format of PyroWave's three shared plane textures.
fn planar_plane_layout(
    config: &butterpollo_media_policy::rtsp::Negotiated,
) -> ([(u32, u32); 3], DXGI_FORMAT) {
    let divisor = if config.yuv444 { 1 } else { 2 };
    let chroma = (config.width / divisor, config.height / divisor);
    let format = if config.ten_bit() {
        DXGI_FORMAT_R16_UNORM
    } else {
        DXGI_FORMAT_R8_UNORM
    };
    ([(config.width, config.height), chroma, chroma], format)
}

/// Allocates PyroWave's three shared Y, Cb and Cr plane textures without
/// compiling any D3D11 conversion shaders: the D3D12 compute path writes these
/// planes directly and only falls back to [`PlanarConverter`] if needed.
pub(crate) fn planar_textures(
    gpu: &Device,
    config: &butterpollo_media_policy::rtsp::Negotiated,
) -> Result<[Arc<ID3D11Texture2D>; 3]> {
    validate_dimensions(config)?;
    let (sizes, format) = planar_plane_layout(config);
    let mut textures = Vec::with_capacity(3);
    // SAFETY: `gpu.device` is a live D3D11 device and each descriptor outlives
    // the call that reads it.
    unsafe {
        for (width, height) in sizes {
            let mut texture = None;
            gpu.device.CreateTexture2D(
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
                    // Unordered access for the conversion on the compute queue.
                    BindFlags: (D3D11_BIND_SHADER_RESOURCE.0
                        | D3D11_BIND_RENDER_TARGET.0
                        | D3D11_BIND_UNORDERED_ACCESS.0) as u32,
                    MiscFlags: (D3D11_RESOURCE_MISC_SHARED.0
                        | D3D11_RESOURCE_MISC_SHARED_NTHANDLE.0)
                        as u32,
                    ..Default::default()
                },
                None,
                Some(&mut texture),
            )?;
            textures.push(Arc::new(texture.context("no planar D3D11 texture")?));
        }
    }
    Ok(textures.try_into().ok().unwrap())
}

/// Pixel-shader fallback state for [`PlanarConverter`], created lazily only if
/// the single-pass D3D11 `pyro_cs` compute shader is unavailable.
struct PlanarPixel {
    vertex: ID3D11VertexShader,
    views: [ID3D11RenderTargetView; 3],
    combined: Option<ID3D11PixelShader>,
    separate: Option<[ID3D11PixelShader; 3]>,
}

pub(crate) struct PlanarConverter {
    gpu: Device,
    /// The context's multithread lock, held across each conversion.
    pub(super) lock: ID3D11Multithread,
    constants: ID3D11Buffer,
    values: [u32; 20],
    dirty: bool,
    width: u32,
    height: u32,
    menu: menu::Menu,
    sources: SourceViews,
    pub(super) compute: Option<(ID3D11ComputeShader, [ID3D11UnorderedAccessView; 3])>,
    pixel: Option<PlanarPixel>,
    /// Whether a 4:4:4 pixel-shader fallback uses the single MRT pass (`pyro_444`).
    pub(super) use_combined: bool,
    pub textures: [Arc<ID3D11Texture2D>; 3],
    divisor: u32,
}

impl PlanarConverter {
    #[cfg(test)]
    pub fn new(
        gpu: &Device,
        config: &butterpollo_media_policy::rtsp::Negotiated,
        source: (u32, u32, Pixel),
    ) -> Result<Self> {
        let textures = planar_textures(gpu, config)?;
        Self::with_textures(gpu, config, source, textures)
    }

    /// Builds a D3D11 planar converter targeting existing shared `textures`,
    /// compiling only `pyro_cs` upfront (pixel shaders compile lazily only if
    /// `pyro_cs` is unavailable).
    pub(crate) fn with_textures(
        gpu: &Device,
        config: &butterpollo_media_policy::rtsp::Negotiated,
        source: (u32, u32, Pixel),
        textures: [Arc<ID3D11Texture2D>; 3],
    ) -> Result<Self> {
        validate_dimensions(config)?;
        validate_source(config, source)?;
        let mut values = constants(config, source, [100., 1.], None);
        values[11] = u32::from(config.yuv444);
        let divisor = if config.yuv444 { 1 } else { 2 };
        let (width, height) = config.coded_size();
        // SAFETY: `gpu.device` is a live D3D11 device, `compile` gets NUL-terminated names, and
        // every descriptor and the 80-byte `values` outlive the calls that read them.
        unsafe {
            let mut constants = None;
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
                Some(&mut constants),
            )?;
            // `pyro_cs` covers 4:4:4 too: with `padding` set it writes full-size
            // chroma (`color.hlsl`), so only a failure falls back to the pixel shaders.
            let compute = (|| -> Result<_> {
                let mut shader = None;
                gpu.device.CreateComputeShader(
                    &compile(b"pyro_cs\0", b"cs_5_0\0")?,
                    None,
                    Some(&mut shader),
                )?;
                let mut unordered = Vec::with_capacity(3);
                for texture in &textures {
                    let mut view = None;
                    gpu.device.CreateUnorderedAccessView(
                        texture.as_ref(),
                        None,
                        Some(&mut view),
                    )?;
                    unordered.push(view.context("no planar UAV")?);
                }
                Ok((
                    shader.context("no planar compute shader")?,
                    unordered.try_into().ok().unwrap(),
                ))
            })()
            .ok();
            Ok(Self {
                gpu: gpu.clone(),
                lock: windows::core::Interface::cast(&gpu.context)?,
                constants: constants.context("no planar constant buffer")?,
                values,
                dirty: false,
                width,
                height,
                menu: Default::default(),
                sources: Default::default(),
                compute,
                pixel: None,
                use_combined: config.yuv444,
                textures,
                divisor,
            })
        }
    }

    /// Lazily compiles the D3D11 pixel-shader fallback outside the context lock.
    fn ensure_pixel(&mut self) -> Result<()> {
        // SAFETY: `self.gpu.device` is a live D3D11 device, `compile` gets NUL-terminated
        // names, and `self.textures` are live render-target textures on this device.
        unsafe {
            let pixel = match &mut self.pixel {
                Some(pixel) => pixel,
                None => {
                    let mut vertex = None;
                    self.gpu.device.CreateVertexShader(
                        &compile(b"vertex\0", b"vs_5_0\0")?,
                        None,
                        Some(&mut vertex),
                    )?;
                    let mut views = Vec::with_capacity(3);
                    for texture in &self.textures {
                        let mut view = None;
                        self.gpu.device.CreateRenderTargetView(
                            texture.as_ref(),
                            None,
                            Some(&mut view),
                        )?;
                        views.push(view.context("no planar RTV")?);
                    }
                    self.pixel.insert(PlanarPixel {
                        vertex: vertex.context("no planar vertex shader")?,
                        views: views.try_into().ok().unwrap(),
                        combined: None,
                        separate: None,
                    })
                }
            };
            if self.use_combined {
                if pixel.combined.is_none() {
                    let mut combined = None;
                    self.gpu.device.CreatePixelShader(
                        &compile(b"pyro_444\0", b"ps_5_0\0")?,
                        None,
                        Some(&mut combined),
                    )?;
                    pixel.combined = Some(combined.context("no planar 4:4:4 pixel shader")?);
                }
            } else if pixel.separate.is_none() {
                let mut shaders = Vec::with_capacity(3);
                for entry in [b"pyro_y\0", b"pyro_u\0", b"pyro_v\0"] {
                    let mut shader = None;
                    self.gpu.device.CreatePixelShader(
                        &compile(entry, b"ps_5_0\0")?,
                        None,
                        Some(&mut shader),
                    )?;
                    shaders.push(shader.context("no planar pixel shader")?);
                }
                pixel.separate = Some(shaders.try_into().ok().unwrap());
            }
        }
        Ok(())
    }

    pub fn set_overlay(&mut self, overlay: Option<&crate::overlay::Overlay>) {
        self.menu.set(overlay);
    }

    #[cfg(test)]
    pub(crate) fn menu_counts(&self) -> (u64, u64) {
        (self.menu.uploads, self.menu.passes)
    }

    pub fn convert(&mut self, image: &GpuImage, luminance: [f32; 2]) -> Result<()> {
        set_luminance_values(&mut self.values, &mut self.dirty, luminance);
        let source = prepare_source_views(
            &self.gpu,
            &mut self.sources,
            &mut self.values,
            &mut self.dirty,
            image,
        )?;
        if self.compute.is_none() {
            self.ensure_pixel()?;
        }
        let menu = self.menu.prepare(
            &self.gpu,
            &self.values,
            [2, 2],
            match (self.compute.is_some(), self.use_combined) {
                (true, _) => &[],
                (false, true) => &[b"menu_pyro_444\0"],
                (false, false) => &[b"menu_pyro_y\0", b"menu_pyro_u\0", b"menu_pyro_v\0"],
            },
            self.compute.is_some().then_some(&b"pyro_menu_cs\0"[..]),
        )?;
        // SAFETY: The context is held from Enter to Leave with no early return between, all bound
        // objects are on this device, and `values` is the buffer's 80 bytes.
        unsafe {
            let context = &self.gpu.context;
            let lock = &self.lock;
            lock.Enter();
            if self.dirty {
                context.UpdateSubresource(
                    &self.constants,
                    0,
                    None,
                    self.values.as_ptr().cast(),
                    0,
                    0,
                );
                self.dirty = false;
            }
            if let Some(menu) = &menu {
                menu.update(context);
            }
            if let Some((shader, views)) = &self.compute {
                context.CSSetShader(shader, None);
                context.CSSetShaderResources(0, Some(&source));
                context.CSSetConstantBuffers(0, Some(&[Some(self.constants.clone())]));
                context.CSSetUnorderedAccessViews(
                    2,
                    3,
                    Some(views.each_ref().map(|v| Some(v.clone())).as_ptr()),
                    None,
                );
                context.Dispatch(
                    (self.width / 2).div_ceil(8),
                    (self.height / 2).div_ceil(8),
                    1,
                );
                if let Some(menu) = &menu {
                    menu.dispatch(context);
                }
                context.CSSetUnorderedAccessViews(2, 3, Some([None, None, None].as_ptr()), None);
                context.CSSetShaderResources(0, Some(&[None, None]));
                context.CSSetShader(None, None);
            } else {
                let pixel = self.pixel.as_ref().expect("pixel fallback ensured above");
                context.IASetInputLayout(None);
                context.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
                context.VSSetShader(&pixel.vertex, None);
                context.PSSetShaderResources(0, Some(&source));
                context.PSSetConstantBuffers(0, Some(&[Some(self.constants.clone())]));
                context.RSSetState(None);
                context.OMSetBlendState(None, None, u32::MAX);
                if let Some(menu) = &menu {
                    menu.bind(context);
                }
                if self.use_combined {
                    let shader = pixel
                        .combined
                        .as_ref()
                        .expect("combined 4:4:4 pixel shader ensured above");
                    context.RSSetViewports(Some(&[D3D11_VIEWPORT {
                        Width: self.width as f32,
                        Height: self.height as f32,
                        MinDepth: 0.,
                        MaxDepth: 1.,
                        ..Default::default()
                    }]));
                    context.OMSetRenderTargets(
                        Some(&pixel.views.each_ref().map(|view| Some(view.clone()))),
                        None,
                    );
                    context.PSSetShader(shader, None);
                    context.Draw(3, 0);
                    if let Some(menu) = &menu {
                        context.PSSetShader(&menu.pixel[0], None);
                        context.Draw(3, 0);
                    }
                } else {
                    let shaders = pixel
                        .separate
                        .as_ref()
                        .expect("separate planar pixel shaders ensured above");
                    for (plane, (view, shader)) in
                        pixel.views.iter().zip(shaders.iter()).enumerate()
                    {
                        let divisor = if plane == 0 { 1 } else { self.divisor };
                        context.RSSetViewports(Some(&[D3D11_VIEWPORT {
                            Width: (self.width / divisor) as f32,
                            Height: (self.height / divisor) as f32,
                            MinDepth: 0.,
                            MaxDepth: 1.,
                            ..Default::default()
                        }]));
                        context.OMSetRenderTargets(Some(&[Some(view.clone())]), None);
                        context.PSSetShader(shader, None);
                        context.Draw(3, 0);
                        if let Some(menu) = &menu {
                            context.PSSetShader(&menu.pixel[plane], None);
                            context.Draw(3, 0);
                        }
                    }
                }
                if menu.is_some() {
                    menu::Pass::unbind(context);
                }
                context.PSSetShaderResources(0, Some(&[None, None]));
                context.OMSetRenderTargets(None, None);
            }
            lock.Leave();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planar_layout_and_validation_match_420_444_and_bit_depth() {
        let hdr_420 = butterpollo_media_policy::rtsp::Negotiated {
            width: 1920,
            height: 1080,
            codec: 3,
            hdr: true,
            ..Default::default()
        };
        assert!(validate_dimensions(&hdr_420).is_ok());
        assert_eq!(
            planar_plane_layout(&hdr_420),
            (
                [(1920, 1080), (960, 540), (960, 540)],
                DXGI_FORMAT_R16_UNORM
            )
        );

        let sdr_444 = butterpollo_media_policy::rtsp::Negotiated {
            width: 1920,
            height: 1080,
            codec: 3,
            yuv444: true,
            ..Default::default()
        };
        assert!(validate_dimensions(&sdr_444).is_ok());
        assert_eq!(
            planar_plane_layout(&sdr_444),
            (
                [(1920, 1080), (1920, 1080), (1920, 1080)],
                DXGI_FORMAT_R8_UNORM
            )
        );

        let sdr_10bit = butterpollo_media_policy::rtsp::Negotiated {
            width: 1280,
            height: 720,
            codec: 3,
            sdr_10bit: true,
            ..Default::default()
        };
        assert_eq!(
            planar_plane_layout(&sdr_10bit),
            ([(1280, 720), (640, 360), (640, 360)], DXGI_FORMAT_R16_UNORM)
        );

        // Zero or odd 4:2:0 dimensions are rejected; odd 4:4:4 dimensions pass.
        assert!(
            validate_dimensions(&butterpollo_media_policy::rtsp::Negotiated {
                width: 0,
                height: 1080,
                ..Default::default()
            })
            .is_err()
        );
        assert!(
            validate_dimensions(&butterpollo_media_policy::rtsp::Negotiated {
                width: 1921,
                height: 1080,
                ..Default::default()
            })
            .is_err()
        );
        assert!(
            validate_dimensions(&butterpollo_media_policy::rtsp::Negotiated {
                width: 1921,
                height: 1080,
                yuv444: true,
                ..Default::default()
            })
            .is_ok()
        );

        // SDR accepts BGRA8 and scRGB FP16, and PQ10 only tone-mapped.
        assert!(validate_source(&sdr_444, (1920, 1080, Pixel::Bgra8)).is_ok());
        assert!(validate_source(&sdr_444, (1920, 1080, Pixel::RgbaF16)).is_ok());
        assert!(validate_source(&sdr_444, (1920, 1080, Pixel::Rgba10Pq)).is_err());
        let tone_mapped = butterpollo_media_policy::rtsp::Negotiated {
            sdr_tone_map: true,
            ..sdr_444.clone()
        };
        assert!(validate_source(&tone_mapped, (1920, 1080, Pixel::Rgba10Pq)).is_ok());
        assert!(validate_source(&hdr_420, (1920, 1080, Pixel::Rgba10Pq)).is_ok());

        // Luminance dirty flag tracks changes accurately.
        let mut values = constants(&hdr_420, (1920, 1080, Pixel::RgbaF16), [100., 1.], None);
        let mut dirty = false;
        set_luminance_values(&mut values, &mut dirty, [100., 1.]);
        assert!(!dirty);
        set_luminance_values(&mut values, &mut dirty, [240., 1.]);
        assert!(dirty);
        assert_eq!(values[9], 3.0f32.to_bits());
    }
}
