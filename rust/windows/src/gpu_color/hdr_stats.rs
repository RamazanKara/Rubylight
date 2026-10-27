//! HDR dynamic metadata statistics behind the D3D11 conversion
//! (`butterpollo_core::hdr_dynamic`): the compute conversion's pass
//! (`compute_hdr_stats.rs`), the same shaders on the graphics context. A
//! conversion leaves its source and constants with [`HdrStats::queue`]; once
//! the frame went to the encoder, [`HdrStats::submit`] histograms PQ(maxRGB)
//! of that source and copies the words into one of a ring of staging
//! buffers. Nothing waits for it on the CPU: [`HdrStats::latest`] maps the
//! newest copy the GPU has finished, so a late pass only keeps the previous
//! one current.
use super::*;
use crate::compute::hdr_stats::{HdrStatsReadout, WORDS, grid};

/// Staging copies in flight. Submitting reuses the oldest; one the GPU has
/// not finished by then is dropped for the newer pass.
const RING: usize = 4;
const BYTES: u32 = 4 * WORDS as u32;

/// A conversion's pass, left for [`HdrStats::submit`].
struct Queued {
    /// The capture, held until the pass is on the context behind the
    /// conversion that read it.
    image: GpuImage,
    /// The conversion's `cbuffer Config`, unchanged until it converts again.
    constants: ID3D11Buffer,
    grid: (u32, u32),
    conversion: u64,
}
struct Readback {
    staging: ID3D11Buffer,
    /// The conversion it measured and its sample count.
    measured: Option<(u64, u64)>,
}
/// The pass's GPU objects, made by [`HdrStats::enable`].
struct Passes {
    clear: ID3D11ComputeShader,
    pass: ID3D11ComputeShader,
    /// The words the pass accumulates into (u7).
    words: ID3D11Buffer,
    view: ID3D11UnorderedAccessView,
    /// `cbuffer Stats` (b3) and the grid it holds.
    grid: (ID3D11Buffer, (u32, u32)),
    ring: Vec<Readback>,
}
/// The statistics pass on the D3D11 conversion's device.
pub(crate) struct HdrStats {
    gpu: Device,
    passes: Option<Passes>,
    queued: Option<Queued>,
    /// The conversions numbered so far: [`HdrStatsReadout::conversion`] counts from 1.
    conversions: u64,
    /// [`HdrStats::stop`]: no more passes are queued.
    stopped: bool,
}
impl HdrStats {
    pub(crate) fn new(gpu: &Device) -> Self {
        Self {
            gpu: gpu.clone(),
            passes: None,
            queued: None,
            conversions: 0,
            stopped: false,
        }
    }
    /// Measure every HDR conversion from now on, for [`HdrStats::latest`].
    pub(crate) fn enable(&mut self) -> Result<()> {
        anyhow::ensure!(
            !self.stopped,
            "HDR statistics were stopped after a failed pass"
        );
        if self.passes.is_some() {
            return Ok(());
        }
        let device = &self.gpu.device;
        // SAFETY: Calls on the live device; `compile` gets NUL-terminated names, and every
        // descriptor and initial value outlives the call that reads it.
        unsafe {
            let shader = |entry: &'static [u8]| -> Result<ID3D11ComputeShader> {
                let mut shader = None;
                device.CreateComputeShader(
                    &compile(entry, b"cs_5_0\0")?,
                    None,
                    Some(&mut shader),
                )?;
                shader.context("no HDR statistics shader")
            };
            let clear = shader(b"hdr_stats_clear_cs\0")?;
            let pass = shader(b"hdr_stats_cs\0")?;
            let structured = D3D11_BUFFER_DESC {
                ByteWidth: BYTES,
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_UNORDERED_ACCESS.0 as u32,
                MiscFlags: D3D11_RESOURCE_MISC_BUFFER_STRUCTURED.0 as u32,
                StructureByteStride: 4,
                ..Default::default()
            };
            let mut words = None;
            device.CreateBuffer(&structured, None, Some(&mut words))?;
            let words = words.context("no HDR statistics buffer")?;
            let mut view = None;
            device.CreateUnorderedAccessView(
                &words,
                Some(&D3D11_UNORDERED_ACCESS_VIEW_DESC {
                    Format: DXGI_FORMAT_UNKNOWN,
                    ViewDimension: D3D11_UAV_DIMENSION_BUFFER,
                    Anonymous: D3D11_UNORDERED_ACCESS_VIEW_DESC_0 {
                        Buffer: D3D11_BUFFER_UAV {
                            FirstElement: 0,
                            NumElements: WORDS as u32,
                            Flags: 0,
                        },
                    },
                }),
                Some(&mut view),
            )?;
            let initial = [0u32; 4];
            let mut constants = None;
            device.CreateBuffer(
                &D3D11_BUFFER_DESC {
                    ByteWidth: 16,
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                    ..Default::default()
                },
                Some(&D3D11_SUBRESOURCE_DATA {
                    pSysMem: initial.as_ptr().cast(),
                    ..Default::default()
                }),
                Some(&mut constants),
            )?;
            let mut ring = Vec::with_capacity(RING);
            for _ in 0..RING {
                let mut staging = None;
                device.CreateBuffer(
                    &D3D11_BUFFER_DESC {
                        Usage: D3D11_USAGE_STAGING,
                        BindFlags: 0,
                        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                        ..structured
                    },
                    None,
                    Some(&mut staging),
                )?;
                ring.push(Readback {
                    staging: staging.context("no HDR statistics staging buffer")?,
                    measured: None,
                });
            }
            self.passes = Some(Passes {
                clear,
                pass,
                words,
                view: view.context("no HDR statistics view")?,
                grid: (constants.context("no HDR statistics constants")?, (0, 0)),
                ring,
            });
        }
        Ok(())
    }
    /// No more statistics passes: dynamic metadata is off for the rest of
    /// the stream. A pass left queued is dropped with its capture.
    pub(crate) fn stop(&mut self) {
        self.stopped = true;
        self.queued = None;
    }
    /// After `converter` converted `image`: leave the pass over its source for
    /// [`HdrStats::submit`], when enabled and the stream is HDR. A pass still
    /// queued is dropped.
    pub(crate) fn queue(&mut self, image: &GpuImage, converter: &Converter) {
        // cbuffer Config: sourceSize=0..2, hdr=5.
        if self.passes.is_none() || self.stopped || converter.values[5] == 0 {
            self.queued = None;
            return;
        }
        self.conversions += 1;
        self.queued = Some(Queued {
            image: image.clone(),
            constants: converter.constants.clone(),
            grid: grid(converter.values[0], converter.values[1]),
            conversion: self.conversions,
        });
    }
    /// Run the pass the last conversion left. Call it once the frame went to
    /// the encoder: nothing in the conversion's path records, submits or
    /// waits for a pass. The context runs it after the conversion, which set
    /// up the capture's ordering (`GpuImage::order_d3d11`).
    pub(crate) fn submit(&mut self) -> Result<()> {
        let (Some(passes), Some(queued)) = (self.passes.as_mut(), self.queued.take()) else {
            return Ok(());
        };
        let copy = (0..RING)
            .min_by_key(|&copy| {
                passes.ring[copy]
                    .measured
                    .map_or(0, |(conversion, _)| conversion)
            })
            .unwrap_or(0);
        let mut source = None;
        // SAFETY: `image.texture` is live and on this device, as the conversion that read it was.
        unsafe {
            self.gpu.device.CreateShaderResourceView(
                queued.image.texture.as_ref(),
                None,
                Some(&mut source),
            )?;
        }
        let views = [source, queued.image.cursor.as_ref().map(|c| c.view.clone())];
        let lock: ID3D11Multithread = windows::core::Interface::cast(&self.gpu.context)?;
        let grid = queued.grid;
        // SAFETY: The context is held from Enter to Leave with no early return between, every
        // bound object is on this device, and the grid's 16 bytes outlive the update.
        unsafe {
            let context = &self.gpu.context;
            lock.Enter();
            if passes.grid.1 != grid {
                let values = [grid.0, grid.1, 0, 0];
                context.UpdateSubresource(&passes.grid.0, 0, None, values.as_ptr().cast(), 0, 0);
                passes.grid.1 = grid;
            }
            context.CSSetShaderResources(0, Some(&views));
            context.CSSetConstantBuffers(0, Some(&[Some(queued.constants.clone())]));
            context.CSSetConstantBuffers(3, Some(&[Some(passes.grid.0.clone())]));
            context.CSSetUnorderedAccessViews(
                7,
                1,
                Some([Some(passes.view.clone())].as_ptr()),
                None,
            );
            context.CSSetShader(&passes.clear, None);
            context.Dispatch((WORDS as u32).div_ceil(64), 1, 1);
            context.CSSetShader(&passes.pass, None);
            context.Dispatch(grid.0.div_ceil(16), grid.1.div_ceil(16), 1);
            context.CSSetUnorderedAccessViews(7, 1, Some([None].as_ptr()), None);
            context.CSSetConstantBuffers(3, Some(&[None]));
            context.CSSetConstantBuffers(0, Some(&[None]));
            context.CSSetShaderResources(0, Some(&[None, None]));
            context.CSSetShader(None, None);
            context.CopyResource(&passes.ring[copy].staging, &passes.words);
            context.Flush();
            lock.Leave();
        }
        passes.ring[copy].measured =
            Some((queued.conversion, u64::from(grid.0) * u64::from(grid.1)));
        Ok(())
    }
    /// The newest finished statistics whose conversion is above `newer_than`.
    /// Maps without waiting: a copy the GPU has not finished is skipped.
    pub(crate) fn latest(&self, newer_than: u64) -> Result<Option<HdrStatsReadout>> {
        let Some(passes) = self.passes.as_ref() else {
            return Ok(None);
        };
        let mut copies: Vec<_> = passes
            .ring
            .iter()
            .filter_map(|copy| copy.measured.map(|measured| (copy, measured)))
            .filter(|&(_, (conversion, _))| conversion > newer_than)
            .collect();
        copies.sort_by_key(|&(_, (conversion, _))| std::cmp::Reverse(conversion));
        let lock: ID3D11Multithread = windows::core::Interface::cast(&self.gpu.context)?;
        for (copy, (conversion, samples)) in copies {
            let mut words = vec![0u32; WORDS];
            // SAFETY: The context is held from Enter to Leave with no early return between; a
            // mapped staging buffer holds `BYTES`, read before it is unmapped.
            let mapped = unsafe {
                lock.Enter();
                let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
                let result = self.gpu.context.Map(
                    &copy.staging,
                    0,
                    D3D11_MAP_READ,
                    D3D11_MAP_FLAG_DO_NOT_WAIT.0 as u32,
                    Some(&mut mapped),
                );
                if result.is_ok() {
                    std::ptr::copy_nonoverlapping(
                        mapped.pData.cast::<u32>(),
                        words.as_mut_ptr(),
                        WORDS,
                    );
                    self.gpu.context.Unmap(&copy.staging, 0);
                }
                lock.Leave();
                result
            };
            match mapped {
                Ok(()) => return HdrStatsReadout::parse(conversion, samples, &words).map(Some),
                Err(error)
                    if error.code()
                        == windows::Win32::Graphics::Dxgi::DXGI_ERROR_WAS_STILL_DRAWING => {}
                Err(error) => return Err(error).context("HDR statistics readback"),
            }
        }
        Ok(None)
    }
}
