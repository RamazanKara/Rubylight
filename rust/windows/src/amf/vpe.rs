//! AMF's video converter (`AMFVideoConverter`) in front of the encoder: the
//! captured texture goes to AMF as it is, the driver converts and scales it
//! (on the Video Processing Engine where the GPU has one, or on compute), and
//! the converter's NV12 or P010 surface goes to the encoder without a copy or
//! a CPU wait. `butterpollo_core::amf_vpe` decides what it may convert; every
//! other frame keeps the shader conversion (`gpu_color`).
//!
//! Ported from Foundation Sunshine's `amd_vpe` (src/amf/amf_vpe_d3d11.cpp,
//! AlkaidLab/foundation-sunshine PR #1216, commit 1add1ed), Copyright (c)
//! Foundation Sunshine authors, GPL-3.0: the converter's properties, its HDR
//! input limits and the HLG prepass. Foundation's waits for the GPU and copies
//! the result into the encoder's texture; this hands AMF's surface on.
use super::{check, gpu, int, timed};
use crate::{amf_abi::*, capture::GpuImage, text::static_wide};
use anyhow::{Context, Result, bail};
use butterpollo_core::amf_vpe::{self as plan, Frame, Input, Output, Refusal, Source};
use std::{
    ptr,
    sync::Arc,
    time::{Duration, Instant},
};
use windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;

/// How long a conversion may take before the stream gives up on it.
const QUERY_DEADLINE: Duration = Duration::from_millis(100);
/// How often the conversion tally is logged.
const REPORT_EVERY: Duration = Duration::from_secs(10);

/// A frame AMF converted, for the encoder.
pub(crate) struct Converted {
    pub surface: gpu::Surface,
    /// The prepass texture AMF reads, held until the frame's output.
    pub held: Option<Arc<ID3D11Texture2D>>,
}

pub(crate) struct Vpe {
    component: *mut AMFComponent,
    context: *mut AMFContext,
    config: butterpollo_core::rtsp::Negotiated,
    output: Output,
    /// The input the converter was initialized for.
    input: Option<(Input, u32, u32)>,
    /// The HLG prepass, and the capture size and format it was made for.
    prepass: Option<(
        crate::gpu_color::HlgScene,
        (u32, u32, crate::capture::Pixel),
    )>,
    /// Whether the driver reports a Video Processing Engine
    /// (`VPEAvailability`); without one it converts on compute.
    pub(crate) engine: bool,
    pub(crate) tally: plan::Tally,
    /// A conversion failed: the shaders convert the rest of the stream. The
    /// converter stays until the encoder, which may hold its surfaces, ends.
    pub(crate) failed: bool,
    /// `AccelerationType`: VPE preferred; the A/B harness also asks for compute.
    pub(crate) acceleration: i64,
    reported: Instant,
    /// The last frame refusal logged, so each new one is said once.
    refused: Option<Refusal>,
    index: i64,
}
// SAFETY: The component and context are used only from the encoder's thread,
// through `&mut self`, as the encoder's own are.
unsafe impl Send for Vpe {}

/// Set one of the converter's properties.
fn wide_set(
    component: *mut AMFComponent,
    name: &'static str,
    value: AMFVariantStruct,
) -> Result<()> {
    // SAFETY: `component` is a live converter, `name` a NUL-terminated static wide string, and
    // AMF copies the variant (retaining an interface value) before returning.
    unsafe {
        check(((*(*component).pVtbl).SetProperty.unwrap())(
            component,
            static_wide(name).as_ptr(),
            value,
        ))
        .with_context(|| format!("AMF video converter {name}"))
    }
}
fn size(width: u32, height: u32) -> AMFVariantStruct {
    AMFVariantStruct {
        type_: AMF_VARIANT_TYPE_AMF_VARIANT_SIZE,
        __bindgen_anon_1: AMFVariantStruct__bindgen_ty_1 {
            sizeValue: AMFSize {
                width: width as i32,
                height: height as i32,
            },
        },
    }
}
fn rect(r: plan::Rect) -> AMFVariantStruct {
    AMFVariantStruct {
        type_: AMF_VARIANT_TYPE_AMF_VARIANT_RECT,
        __bindgen_anon_1: AMFVariantStruct__bindgen_ty_1 {
            rectValue: AMFRect {
                left: r.x as i32,
                top: r.y as i32,
                right: (r.x + r.width) as i32,
                bottom: (r.y + r.height) as i32,
            },
        },
    }
}
/// Opaque black, for the bars.
fn black() -> AMFVariantStruct {
    // SAFETY: AMFColor is four bytes (r, g, b, a), all-zero valid; the write fills them as one
    // little-endian u32 with alpha 255.
    let color = unsafe {
        let mut color: AMFColor = std::mem::zeroed();
        ptr::addr_of_mut!(color)
            .cast::<u32>()
            .write_unaligned(0xff00_0000);
        color
    };
    AMFVariantStruct {
        type_: AMF_VARIANT_TYPE_AMF_VARIANT_COLOR,
        __bindgen_anon_1: AMFVariantStruct__bindgen_ty_1 { colorValue: color },
    }
}
pub(crate) fn source_of(pixel: crate::capture::Pixel) -> Source {
    match pixel {
        crate::capture::Pixel::Bgra8 => Source::Bgra8,
        crate::capture::Pixel::RgbaF16 => Source::ScRgbF16,
        crate::capture::Pixel::Rgba10Pq => Source::Pq10,
    }
}

impl Vpe {
    /// A converter on the encoder's D3D11 `context`, or why there is none.
    ///
    /// # Safety
    ///
    /// `factory` and `context` are the encoder's live AMF factory and its
    /// D3D11-initialized context, which outlive the converter.
    pub(crate) unsafe fn new(
        factory: *mut AMFFactory,
        context: *mut AMFContext,
        config: &butterpollo_core::rtsp::Negotiated,
    ) -> Result<Self> {
        let output = plan::stream_plan(&plan::Stream::of(config))
            .map_err(|refusal| anyhow::anyhow!("{}", refusal.code()))?;
        let mut component = ptr::null_mut();
        // SAFETY: The caller passes a live factory and context; CreateComponent writes an owned
        // reference into `component`, released on error or in Drop.
        unsafe {
            check(((*(*factory).pVtbl).CreateComponent.unwrap())(
                factory,
                context,
                static_wide("AMFVideoConverter").as_ptr(),
                &mut component,
            ))
            .context("AMF CreateComponent AMFVideoConverter")?;
        }
        if component.is_null() {
            bail!("AMF returned no video converter");
        }
        let mut vpe = Self {
            component,
            context,
            config: config.clone(),
            output,
            input: None,
            prepass: None,
            engine: false,
            tally: Default::default(),
            failed: false,
            acceleration: plan::vui::ACCELERATION_VPE_PREFERRED,
            reported: Instant::now(),
            refused: None,
            index: 0,
        };
        vpe.engine = vpe.engine_available();
        Ok(vpe)
    }
    /// The converter's `VPEAvailability` capability.
    fn engine_available(&self) -> bool {
        // SAFETY: `component` is live; caps is checked and its reference released after the
        // query. `int(0)` initializes the out-variant, and only a BOOL result is read.
        unsafe {
            let mut caps = ptr::null_mut();
            if ((*(*self.component).pVtbl).GetCaps.unwrap())(self.component, &mut caps)
                != AMF_RESULT_AMF_OK
                || caps.is_null()
            {
                return false;
            }
            let mut value = int(0);
            let result = ((*(*caps).pVtbl).GetProperty.unwrap())(
                caps,
                static_wide("VPEAvailability").as_ptr(),
                &mut value,
            );
            ((*(*caps).pVtbl).Release.unwrap())(caps);
            result == AMF_RESULT_AMF_OK
                && value.type_ == AMF_VARIANT_TYPE_AMF_VARIANT_BOOL
                && value.__bindgen_anon_1.boolValue != 0
        }
    }
    /// The converter's input brightness limits: `peak` nits.
    fn hdr_limits(&self, name: &'static str, peak: u32) -> Result<()> {
        // SAFETY: The live context allocates host memory sized for AMFHDRMetadata, written while
        // owned; SetProperty retains its own reference before this one is released.
        unsafe {
            let mut buffer = ptr::null_mut();
            check(((*(*self.context).pVtbl).AllocBuffer.unwrap())(
                self.context,
                AMF_MEMORY_TYPE_AMF_MEMORY_HOST,
                std::mem::size_of::<AMFHDRMetadata>(),
                &mut buffer,
            ))
            .context("AMF video converter HDR limits buffer")?;
            if buffer.is_null() {
                bail!("AMF returned no HDR limits buffer");
            }
            let native = ((*(*buffer).pVtbl).GetNative.unwrap())(buffer).cast::<AMFHDRMetadata>();
            if native.is_null() {
                ((*(*buffer).pVtbl).Release.unwrap())(buffer);
                bail!("AMF HDR limits buffer has no memory");
            }
            let mut limits: AMFHDRMetadata = std::mem::zeroed();
            limits.maxMasteringLuminance = peak * 10_000;
            limits.maxContentLightLevel = peak.min(u32::from(u16::MAX)) as u16;
            limits.maxFrameAverageLightLevel = limits.maxContentLightLevel;
            native.write_unaligned(limits);
            let result = wide_set(
                self.component,
                name,
                AMFVariantStruct {
                    type_: AMF_VARIANT_TYPE_AMF_VARIANT_INTERFACE,
                    __bindgen_anon_1: AMFVariantStruct__bindgen_ty_1 {
                        pInterface: buffer.cast(),
                    },
                },
            );
            ((*(*buffer).pVtbl).Release.unwrap())(buffer);
            result
        }
    }
    /// Initialize the converter for `input` at `width` x `height`.
    fn configure(&mut self, input: Input, width: u32, height: u32) -> Result<()> {
        if self.input.is_some() {
            // A capture of another format or size: start the converter again.
            // SAFETY: `component` is live; Terminate leaves it ready for Init.
            unsafe {
                ((*(*self.component).pVtbl).Terminate.unwrap())(self.component);
            }
            self.input = None;
        }
        let output = self.output;
        let c = self.component;
        let target = plan::content_rect((width, height), (self.config.width, self.config.height));
        wide_set(c, "AccelerationType", int(self.acceleration))?;
        wide_set(c, "MemoryType", int(AMF_MEMORY_TYPE_AMF_MEMORY_DX11 as i64))?;
        wide_set(c, "OutputFormat", int(output.format.amf()))?;
        wide_set(c, "OutputSize", size(self.config.width, self.config.height))?;
        wide_set(c, "OutputRect", rect(target))?;
        wide_set(c, "Fill", gpu::boolean(true))?;
        wide_set(c, "FillColor", black())?;
        wide_set(c, "ScaleType", int(plan::vui::SCALE_BICUBIC))?;
        wide_set(c, "ColorProfile", int(output.profile))?;
        wide_set(c, "UseDecoderHDRMetadata", gpu::boolean(false))?;
        wide_set(c, "InputTransferChar", int(input.color.transfer))?;
        wide_set(c, "InputColorPrimaries", int(input.color.primaries))?;
        wide_set(c, "InputColorMatrixCoeff", int(input.color.matrix))?;
        wide_set(c, "InputColorRange", int(input.color.range))?;
        wide_set(c, "InputTonemapping", int(plan::vui::TONEMAPPING_COPY))?;
        wide_set(c, "OutputColorPrimaries", int(output.color.primaries))?;
        wide_set(c, "OutputTransferChar", int(output.color.transfer))?;
        wide_set(c, "OutputColorMatrixCoeff", int(output.color.matrix))?;
        wide_set(c, "OutputColorRange", int(output.color.range))?;
        if let Some(peak) = input.hdr_peak_nits {
            self.hdr_limits("InputHdrMetadata", peak)?;
            if input.hlg_prepass {
                self.hdr_limits("OutputHdrMetadata", peak)?;
            }
        }
        // SAFETY: `component` is live and every property above was accepted.
        unsafe {
            check(timed("AMF converter Init", || {
                ((*(*c).pVtbl).Init.unwrap())(
                    c,
                    input.format.amf() as AMF_SURFACE_FORMAT,
                    width as i32,
                    height as i32,
                )
            }))
            .context("AMF video converter Init")?;
        }
        tracing::info!(
            engine = self.engine,
            input = ?input.format,
            output = ?output.format,
            width,
            height,
            stream_width = self.config.width,
            stream_height = self.config.height,
            rect = ?target,
            hlg_prepass = input.hlg_prepass,
            "AMF video converter ready (VPE preferred; the driver converts on compute where it has no VPE)"
        );
        self.input = Some((input, width, height));
        Ok(())
    }
    /// The format the converter writes.
    #[cfg(test)]
    pub(crate) fn output(&self) -> Output {
        self.output
    }
    /// How AMF converts `image`, or why the shader does.
    pub(crate) fn plan(
        &mut self,
        image: &GpuImage,
        luminance: [f32; 2],
        menu: bool,
    ) -> Result<Input, Refusal> {
        let planned = plan::frame_plan(
            &self.output,
            &Frame {
                source: source_of(image.pixel),
                hdr_scale: luminance[1],
                pointer: image.cursor.is_some(),
                menu,
            },
        );
        match planned {
            Err(refusal) if self.refused != Some(refusal) => {
                self.refused = Some(refusal);
                self.tally.refused = Some(refusal.code());
                tracing::info!(
                    reason = refusal.code(),
                    "AMF video converter: this frame uses the shader conversion"
                );
            }
            Ok(_) if self.refused.is_some() => {
                self.refused = None;
                tracing::info!("AMF video converter: converting frames again");
            }
            _ => {}
        }
        planned
    }
    /// Convert `image` as `input`; the surface goes to the encoder.
    pub(crate) fn convert(
        &mut self,
        image: &GpuImage,
        input: Input,
        ownership: &mut gpu::Ownership,
    ) -> Result<Converted> {
        // A compute-queue capture copy may still be writing the texture:
        // the converter reads it on D3D11, which does not wait for D3D12.
        if let Some(ready) = &image.ready {
            ready.wait().context("capture copy before AMF conversion")?;
        }
        image.order_d3d11()?;
        if self
            .input
            .is_none_or(|(old, w, h)| old != input || (w, h) != (image.width, image.height))
        {
            self.configure(input, image.width, image.height)?;
        }
        let held = if input.hlg_prepass {
            let key = (image.width, image.height, image.pixel);
            if self.prepass.as_ref().is_none_or(|(_, made)| *made != key) {
                self.prepass = Some((
                    crate::gpu_color::HlgScene::new(&image.gpu, &self.config, key)?,
                    key,
                ));
            }
            Some(self.prepass.as_mut().unwrap().0.run(image)?)
        } else {
            None
        };
        let texture = held.as_ref().unwrap_or(&image.texture);
        let (source, _) = gpu::wrap(self.context, texture, ownership)?;
        // SAFETY: `source` and `component` are live; SubmitInput retains the surface, and the
        // data QueryOutput returns is an owned reference queried for its surface interface and
        // released once.
        unsafe {
            ((*(*source.0).pVtbl).SetPts.unwrap())(source.0, self.index);
            self.index += 1;
            check(timed("AMF converter SubmitInput", || {
                ((*(*self.component).pVtbl).SubmitInput.unwrap())(self.component, source.0.cast())
            }))
            .context("AMF video converter SubmitInput")?;
            let deadline = Instant::now() + QUERY_DEADLINE;
            let data = loop {
                let mut data = ptr::null_mut();
                let result =
                    ((*(*self.component).pVtbl).QueryOutput.unwrap())(self.component, &mut data);
                if !data.is_null() {
                    break data;
                }
                if !matches!(
                    result,
                    AMF_RESULT_AMF_OK | AMF_RESULT_AMF_REPEAT | AMF_RESULT_AMF_NEED_MORE_INPUT
                ) {
                    check(result).context("AMF video converter QueryOutput")?;
                }
                if Instant::now() >= deadline {
                    bail!("AMF video converter returned no picture within 100 ms");
                }
                std::thread::yield_now();
            };
            let mut queried = ptr::null_mut();
            let result = ((*(*data).pVtbl).QueryInterface.unwrap())(
                data,
                &super::focus::SURFACE_IID,
                &mut queried,
            );
            ((*(*data).pVtbl).Release.unwrap())(data);
            check(result).context("AMF video converter output is not a surface")?;
            if queried.is_null() {
                bail!("AMF video converter returned no surface");
            }
            let surface = gpu::Surface(queried.cast());
            let format = ((*(*surface.0).pVtbl).GetFormat.unwrap())(surface.0);
            if i64::from(format) != self.output.format.amf() {
                bail!(
                    "AMF video converter wrote surface format {format}, not {:?}",
                    self.output.format
                );
            }
            Ok(Converted { surface, held })
        }
    }
    /// Count a converted frame, and log the tally now and then.
    pub(crate) fn record(&mut self, vpe: bool, conversion: Duration, captured: Instant) {
        let now = Instant::now();
        self.tally.add(
            vpe,
            conversion.as_micros() as u64,
            now.saturating_duration_since(captured).as_micros() as u64,
        );
        if now.duration_since(self.reported) >= REPORT_EVERY {
            self.report();
            self.reported = now;
            self.tally = self.tally.next_window();
        }
    }
    fn report(&self) {
        let t = &self.tally;
        tracing::info!(
            engine = self.engine,
            vpe_frames = t.vpe_frames,
            shader_frames = t.shader_frames,
            vpe_conversion_mean_us = format!("{:.0}", t.mean_vpe_us()),
            vpe_conversion_worst_us = t.vpe_worst_us,
            shader_conversion_mean_us = format!("{:.0}", t.mean_shader_us()),
            capture_to_submit_mean_us = format!("{:.0}", t.mean_capture_to_submit_us()),
            capture_to_submit_worst_us = t.capture_to_submit_worst_us,
            last_refusal = t.refused.unwrap_or(""),
            "vpe_conversion"
        );
    }
}
impl Drop for Vpe {
    fn drop(&mut self) {
        if self.tally.frames() > 0 {
            self.report();
        }
        // SAFETY: Self owns the component's one reference; the encoder, which held its output
        // surfaces, terminates first (`Encoder::drop`), and the context outlives this.
        unsafe {
            ((*(*self.component).pVtbl).Terminate.unwrap())(self.component);
            ((*(*self.component).pVtbl).Release.unwrap())(self.component);
        }
    }
}
/// Whether an AMF stream converts with AMF's video converter: the setting is
/// on and the stream's layout is one it can write.
pub(crate) fn wanted_for(
    options: &butterpollo_core::config::Config,
    config: &butterpollo_core::rtsp::Negotiated,
) -> Result<bool, Refusal> {
    if !plan::wanted(options) {
        return Ok(false);
    }
    plan::stream_plan(&plan::Stream::of(config)).map(|_| true)
}

#[cfg(test)]
#[path = "vpe_tests.rs"]
mod tests;
