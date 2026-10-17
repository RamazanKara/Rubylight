//! Native AMD AMF encoder for H.264, HEVC and AV1.
//!
//! Calls AMF's C ABI directly (no FFmpeg wrapper), takes D3D11 or D3D12
//! textures from capture and Radeon compute without a copy, and applies the
//! per-codec settings from `butterpollo_core::encoder_policy`.
pub(crate) mod gpu;
mod metadata;
mod rate_control;

use crate::text::to_wide;
use crate::{
    amf_abi::*,
    capture::{Device, GpuImage, Image},
    encoder::{Convert, Encoded},
    ff,
};
use anyhow::{Context, Result, bail};
use butterpollo_core::encoder_backlog::{AMF_SURFACE_LIMIT, AmfBacklog, QUERY_TIMEOUT};
use std::{
    ptr,
    time::{Duration, Instant},
};
use windows::core::Interface;
/// A call this slow held the stream thread: a stalled VCN or compute queue
/// blocks inside the driver, where nothing can time it out.
const SLOW_CALL: Duration = Duration::from_millis(50);
fn timed<T>(call: &'static str, run: impl FnOnce() -> T) -> T {
    let started = Instant::now();
    let result = run();
    let elapsed = started.elapsed();
    if matches!(call, "AMF Init" | "AMF Terminate" | "AMF context Terminate") {
        tracing::debug!(
            call,
            elapsed_us = elapsed.as_micros() as u64,
            "encoder lifecycle call"
        );
    }
    if elapsed >= SLOW_CALL {
        tracing::warn!(
            call,
            elapsed_ms = elapsed.as_millis() as u64,
            "slow encoder call"
        );
    }
    result
}
pub(crate) fn check(n: AMF_RESULT) -> Result<()> {
    if n != AMF_RESULT_AMF_OK {
        bail!("AMF error {n}");
    }
    Ok(())
}
pub(crate) fn int(n: i64) -> AMFVariantStruct {
    AMFVariantStruct {
        type_: AMF_VARIANT_TYPE_AMF_VARIANT_INT64,
        __bindgen_anon_1: AMFVariantStruct__bindgen_ty_1 { int64Value: n },
    }
}
struct OutputRange {
    name: &'static str,
    full: bool,
    accepted: bool,
}
impl OutputRange {
    fn configure(codec: u8, full: bool, mut set: impl FnMut(&str, bool) -> Result<()>) -> Self {
        let (input, output) = match codec {
            0 => ("InputFullRangeColor", "FullRangeColor"),
            1 => ("HevcInputFullRangeColor", "HevcNominalRange"),
            _ => ("Av1InputFullRangeColor", "Av1NominalRange"),
        };
        // Input hints only inform AMF's own converter. In particular, rejecting
        // HevcInputFullRangeColor must not break the RX 6800 XT (issue #5).
        if let Err(error) = set(input, full) {
            tracing::debug!(error = %format!("{error:#}"), property = input, "AMF input range property unavailable");
        }
        let accepted = match set(output, full) {
            Ok(()) => true,
            Err(error) => {
                tracing::debug!(error = %format!("{error:#}"), property = output, "AMF output range setter unavailable; checking initialized driver state");
                false
            }
        };
        Self {
            name: output,
            full,
            accepted,
        }
    }

    fn validate(&self, actual: Option<i64>) -> Result<()> {
        // Read after Init: a preset may change properties during initialization.
        // Successful SetProperty is sufficient when GetProperty is unavailable,
        // but an observed mismatch overrides that success. All three codecs
        // document limited range as their default, preserving older drivers.
        match actual {
            Some(value) if value == i64::from(self.full) => Ok(()),
            Some(value) => bail!(
                "AMF output range mismatch: {} requested {}, driver reports {value}; use another codec/backend or update the AMD driver",
                self.name,
                i64::from(self.full),
            ),
            None if self.accepted || !self.full => Ok(()),
            None => bail!(
                "AMF full-range output cannot be established: {} was rejected and cannot be read back; select limited range, another codec/backend, or update the AMD driver",
                self.name,
            ),
        }
    }
}
fn guard_smart_access_video(properties: &mut [butterpollo_core::encoder_policy::Property]) {
    use butterpollo_core::encoder_policy::Value;
    // The C++ backend also excluded this combination after HEVC HDR runs
    // wedged AMF and reset the GPU (src/amf/amf_lifecycle.h at tag 2.0.0-rc.23).
    if properties.iter().any(|property| {
        property.name == "LowLatencyInternal" && property.value == Value::Boolean(true)
    }) && let Some(property) = properties.iter_mut().find(|property| {
        matches!(
            property.name.as_str(),
            "EnableEncoderSmartAccessVideo" | "HevcEnableEncoderSmartAccessVideo"
        ) && property.value == Value::Boolean(true)
    }) {
        property.value = Value::Boolean(false);
        // Do not proceed with both paths enabled if the driver refuses this.
        property.required = true;
        tracing::warn!(
            "AMF SmartAccess Video disabled because forced low latency can reset the GPU. Disable forced low latency to request SmartAccess Video safely"
        );
    }
}
struct Submission {
    pts: i64,
    started: Instant,
    accepted: Instant,
    presentation: Instant,
    after_invalidation: bool,
    _capture: Option<GpuImage>,
    _converted: Option<std::sync::Arc<windows::Win32::Graphics::Direct3D11::ID3D11Texture2D>>,
}
fn acknowledge_keyframe(
    idr: bool,
    result: AMF_RESULT,
    recover: &mut bool,
    metadata: &mut metadata::State,
) {
    // All reasons are satisfied by the same accepted keyframe. A non-IDR
    // must retain a recovery request raised by poll() during backpressure.
    if idr && result == AMF_RESULT_AMF_OK {
        *recover = false;
        metadata.submitted(result);
    }
}
pub struct Encoder {
    pub(crate) warnings: std::sync::Arc<butterpollo_core::session::Warnings>,
    component: *mut AMFComponent,
    context: *mut AMFContext,
    convert: Option<Convert>,
    config: butterpollo_core::rtsp::Negotiated,
    codec: u8,
    _device: Device,
    _library: libloading::Library,
    index: i64,
    gpu_convert: Option<gpu::Converter>,
    in_flight: std::collections::VecDeque<Submission>,
    backlog: AmfBacklog,
    /// A frame AMF never returned may have been a reference: the next frame
    /// is a keyframe, or the client decodes damaged pictures until one comes.
    recover: bool,
    rate_control: rate_control::State,
    references: butterpollo_core::ltr::References,
    ownership: Box<gpu::Ownership>,
    pub(crate) luminance: [f32; 2],
    /// Successfully applied metadata and any pending retry/keyframe.
    hdr_metadata: metadata::State,
    /// Conversion on a D3D12 compute queue, with AMF on D3D12.
    compute: Option<Box<ComputeInput>>,
    last_query: Option<AMF_RESULT>,
    last_submit: Option<AMF_RESULT>,
    /// When AMF last accepted an input, for the stream's stage timings.
    last_accepted: Option<Instant>,
    input_full: u64,
}
struct ComputeInput {
    converter: crate::compute::Converter,
    context: gpu::D3d12Context,
}
impl Encoder {
    pub fn new(config: &butterpollo_core::rtsp::Negotiated, display: &str) -> Result<Self> {
        Self::new_device(config, Device::new(display)?)
    }
    pub fn new_device(config: &butterpollo_core::rtsp::Negotiated, device: Device) -> Result<Self> {
        Self::new_device_options(config, device, &butterpollo_core::config::Config::default())
    }
    pub fn new_device_options(
        config: &butterpollo_core::rtsp::Negotiated,
        device: Device,
        options: &butterpollo_core::config::Config,
    ) -> Result<Self> {
        Self::new_device_alignment_options(
            config,
            device,
            options,
            AMF_VIDEO_ENCODER_AV1_ALIGNMENT_MODE_ENUM_AMF_VIDEO_ENCODER_AV1_ALIGNMENT_MODE_NO_RESTRICTIONS as i64,
        )
    }
    fn new_device_alignment_options(
        config: &butterpollo_core::rtsp::Negotiated,
        device: Device,
        options: &butterpollo_core::config::Config,
        av1_alignment: i64,
    ) -> Result<Self> {
        Self::create(config, device, options, av1_alignment, None)
    }
    /// An encoder for captured GPU images. With `compute`, the colours are
    /// converted on that D3D12 compute queue and AMF encodes from D3D12, so
    /// neither waits behind a game's work on the graphics engine.
    pub fn new_gpu(
        config: &butterpollo_core::rtsp::Negotiated,
        device: Device,
        options: &butterpollo_core::config::Config,
        compute: Option<std::sync::Arc<crate::compute::Compute>>,
    ) -> Result<Self> {
        Self::create(
            config,
            device,
            options,
            AMF_VIDEO_ENCODER_AV1_ALIGNMENT_MODE_ENUM_AMF_VIDEO_ENCODER_AV1_ALIGNMENT_MODE_NO_RESTRICTIONS as i64,
            compute,
        )
    }
    fn create(
        config: &butterpollo_core::rtsp::Negotiated,
        device: Device,
        options: &butterpollo_core::config::Config,
        av1_alignment: i64,
        compute: Option<std::sync::Arc<crate::compute::Compute>>,
    ) -> Result<Self> {
        let mut effective_config = config.clone();
        effective_config.intra_refresh =
            butterpollo_core::encoder_policy::amf_intra_refresh(config);
        if config.intra_refresh && !effective_config.intra_refresh {
            tracing::warn!(
                client_max_reference_frames = config.references,
                "AMF H.264 intra refresh requires two references; using larger IDR recovery frames for this decoder. Reduce bitrate if recovery stutters or use a client with two-reference support"
            );
        }
        let config = &effective_config;
        // VCN encodes 4:2:0 only: neither AMF nor D3D12 video encoding offers a
        // 4:4:4 profile or input format (rust/PERFORMANCE.md, October 7).
        if config.yuv444 {
            bail!("AMD's video encoder supports only 4:2:0; PyroWave or NVENC can stream 4:4:4");
        }
        if config.ten_bit() && config.codec == 0 {
            bail!("H.264 does not support HDR10");
        }
        // SAFETY: The AMD runtime exports AMFInit with the SDK's C ABI and performs
        // its own initialization. Successful factory calls return owned interfaces
        // with SDK vtables; the retained library/device outlive their use and release.
        // Property names are terminated temporaries and variant tags match initialized
        // members; AMF copies them synchronously and retains interface-valued properties.
        unsafe {
            let library =
                libloading::Library::new("amfrt64.dll").context("AMD AMF runtime unavailable")?;
            let init: libloading::Symbol<
                unsafe extern "C" fn(u64, *mut *mut AMFFactory) -> AMF_RESULT,
            > = library.get(b"AMFInit\0")?;
            let mut factory = ptr::null_mut();
            let version = ((AMF_VERSION_MAJOR as u64) << 48)
                | ((AMF_VERSION_MINOR as u64) << 32)
                | ((AMF_VERSION_RELEASE as u64) << 16)
                | AMF_VERSION_BUILD_NUM as u64;
            check(init(version, &mut factory))?;
            if factory.is_null() {
                bail!("AMF returned no factory");
            }
            let mut context = ptr::null_mut();
            check(((*(*factory).pVtbl).CreateContext.unwrap())(
                factory,
                &mut context,
            ))?;
            let mut component = ptr::null_mut();
            let mut d3d12 = None;
            let result = (|| -> Result<()> {
                if let Some(compute) = &compute {
                    let interface = gpu::D3d12Context::new(context)?;
                    interface.init(&compute.device)?;
                    d3d12 = Some(interface);
                } else {
                    check(((*(*context).pVtbl).InitDX11.unwrap())(
                        context,
                        device.device.as_raw(),
                        AMF_DX_VERSION_AMF_DX11_1,
                    ))
                    .context("AMF InitDX11")?;
                }
                let name = match config.codec {
                    0 => "AMFVideoEncoderVCE_AVC",
                    1 => "AMFVideoEncoderHW_HEVC",
                    2 => "AMFVideoEncoderHW_AV1",
                    _ => bail!("unsupported AMF codec"),
                };
                check(((*(*factory).pVtbl).CreateComponent.unwrap())(
                    factory,
                    context,
                    to_wide(name).as_ptr(),
                    &mut component,
                ))
                .with_context(|| format!("AMF CreateComponent {name}"))?;
                Ok(())
            })();
            if let Err(e) = result {
                drop(d3d12);
                if !component.is_null() {
                    ((*(*component).pVtbl).Release.unwrap())(component);
                }
                ((*(*context).pVtbl).Terminate.unwrap())(context);
                ((*(*context).pVtbl).Release.unwrap())(context);
                return Err(e);
            }
            let compute = match (d3d12, compute) {
                (Some(interface), Some(compute)) => match crate::compute::Converter::new(
                    compute,
                    config.width,
                    config.height,
                    config.ten_bit(),
                ) {
                    Ok(converter) => Some(Box::new(ComputeInput {
                        converter,
                        context: interface,
                    })),
                    // Released as above: recovery retries every 100 ms, and each
                    // leaked component would hold an encoder session.
                    Err(e) => {
                        drop(interface);
                        ((*(*component).pVtbl).Release.unwrap())(component);
                        ((*(*context).pVtbl).Terminate.unwrap())(context);
                        ((*(*context).pVtbl).Release.unwrap())(context);
                        return Err(e);
                    }
                },
                _ => None,
            };
            let mut e = Self {
                warnings: Default::default(),
                component,
                context,
                convert: None,
                config: config.clone(),
                codec: config.codec,
                _device: device,
                _library: library,
                index: 0,
                gpu_convert: None,
                in_flight: std::collections::VecDeque::new(),
                backlog: AmfBacklog::new(None),
                recover: false,
                rate_control: rate_control::State::new(config.bitrate_kbps),
                references: Default::default(),
                ownership: gpu::Ownership::new(),
                luminance: [100., 1.],
                hdr_metadata: metadata::State::default(),
                compute,
                last_query: None,
                last_submit: None,
                last_accepted: None,
                input_full: 0,
            };
            let mut properties = butterpollo_core::encoder_policy::amf(options, config)?;
            guard_smart_access_video(&mut properties);
            for property in properties {
                if let Err(error) = e.apply(&property) {
                    if property.required {
                        return Err(error)
                            .with_context(|| format!("AMF setting {}", property.name));
                    }
                    tracing::warn!(%error, setting=%property.name, "AMF setting unavailable; retaining driver default. Check the named setting and update the AMD driver if quality or cadence differs");
                }
            }
            e.property(
                "FrameSize",
                AMFVariantStruct {
                    type_: AMF_VARIANT_TYPE_AMF_VARIANT_SIZE,
                    __bindgen_anon_1: AMFVariantStruct__bindgen_ty_1 {
                        sizeValue: AMFSize {
                            width: config.width as i32,
                            height: config.height as i32,
                        },
                    },
                },
            )?;
            e.property(
                "FrameRate",
                AMFVariantStruct {
                    type_: AMF_VARIANT_TYPE_AMF_VARIANT_RATE,
                    __bindgen_anon_1: AMFVariantStruct__bindgen_ty_1 {
                        rateValue: AMFRate {
                            num: config.fps_millihz(),
                            den: 1000,
                        },
                    },
                },
            )?;
            e.property("TargetBitrate", int(i64::from(config.bitrate_kbps) * 1000))?;
            let limits = butterpollo_core::encoder_policy::amf_rate_control(options, config)?;
            let warnings = e.warnings.clone();
            rate_control::configure(&limits, &warnings, |property| e.apply(property))?;
            if config.codec == 0 {
                // Keyframes only when asked, as the original backend sets it:
                // the usage preset's periodic IDRs are large, can overflow
                // FEC at high bitrates and are not tracked for recovery.
                let _ = e.property("IDRPeriod", int(0));
            }
            if config.codec == 1 {
                // Request keyframes and headers per surface: finite GOPs stall
                // recent VCN drivers, as established by the original backend.
                e.property("GOPSize", int(0))?;
            }
            if config.codec == 2 {
                e.property("AlignmentMode", int(av1_alignment))?;
                e.property("GOPSize", int(0))?;
                // Despite the alignment mode, AMD drivers encode such sizes
                // padded (1968×2184 as 1984×2186) without a render size, so
                // clients that do not crop to the negotiated size show the
                // padding (AMF issue 423).
                if !config.width.is_multiple_of(64) || !config.height.is_multiple_of(16) {
                    tracing::warn!(
                        width = config.width,
                        height = config.height,
                        "AV1 at this size is encoded with padding on AMD; a client that does not crop shows extra rows or columns. HEVC, or a width divisible by 64 and a height by 16, avoids it"
                    );
                }
            }
            // QueryOutput waits in the driver and returns the moment a frame is
            // encoded; no host timer is involved. `poll` only queries while a
            // frame is in flight, so an expired wait never stalls an idle loop.
            let _ = e.property("QueryTimeout", int(QUERY_TIMEOUT.as_millis() as i64));
            if config.ten_bit() {
                e.property("ColorBitDepth", int(10))?;
                if config.codec == 1 {
                    e.property("Profile", int(2))?;
                }
            }
            let matrix = config.color_matrix();
            let profile = if config.full_range() {
                match matrix {
                    0 => 3,
                    1 => 7,
                    _ => 8,
                }
            } else {
                i64::from(matrix)
            };
            let primaries = match matrix {
                0 => 6,
                2 => 9,
                _ => 1,
            };
            let transfer = if config.hdr {
                16
            } else {
                match matrix {
                    0 => 6,
                    2 => 14,
                    _ => 1,
                }
            };
            let prefix = match config.codec {
                0 => "",
                1 => "Hevc",
                _ => "Av1",
            };
            let (input, output) = if config.codec == 2 {
                ("Input", "Output")
            } else {
                ("In", "Out")
            };
            for (suffix, value) in [
                ("ColorProfile", profile),
                ("ColorTransferChar", transfer),
                ("ColorPrimaries", primaries),
            ] {
                // The input description only informs AMF's own converter, and
                // the original backend never set it; a driver that rejects it,
                // as the RX 6800 XT rejected the input range (issue #5), must
                // not cost the encoder.
                let name = format!("{prefix}{input}{suffix}");
                if let Err(error) = e.property_raw(&name, int(value)) {
                    tracing::debug!(error = %format!("{error:#}"), property = name, "AMF input colour property unavailable");
                }
                // The output description reaches the bitstream, which an HDR
                // stream depends on.
                let name = format!("{prefix}{output}{suffix}");
                if let Err(error) = e.property_raw(&name, int(value)) {
                    if config.hdr {
                        return Err(error);
                    }
                    tracing::warn!(error = %format!("{error:#}"), property = name, "AMF output colour property unavailable; driver colour defaults remain. Update the AMD driver or use another codec if colours look wrong");
                }
            }
            let range = OutputRange::configure(config.codec, config.full_range(), |name, full| {
                e.property_raw(name, gpu::boolean(full))
            });
            // Only for a value the user set: the default asks for long-term
            // references in AV1 alone, and every H.264 and HEVC probe at
            // startup warned otherwise.
            if config.codec != 2
                && options.values.contains_key("amd_ltr_frames")
                && options.integer("amd_ltr_frames", 0) > 0
            {
                tracing::warn!(
                    "amd_ltr_frames applies to AV1 only: H.264 and HEVC reference invalidation froze the picture on a test decoder, so they recover with keyframes"
                );
            }
            let requested_ltr =
                butterpollo_core::encoder_policy::amf_ltr_requested(options, config.codec);
            let ltr_count = butterpollo_core::encoder_policy::amf_ltr_frames(options, config);
            if ltr_count < requested_ltr {
                tracing::info!(
                    requested_ltr,
                    client_max_reference_frames = config.references,
                    effective_ltr_frames = ltr_count,
                    intra_refresh = config.intra_refresh,
                    "AMF LTR anchors limited by decoder reference budget or intra refresh"
                );
            }
            e.configure_ltr(ltr_count);
            e.request_split_frame(options);
            check(timed("AMF Init", || {
                ((*(*e.component).pVtbl).Init.unwrap())(
                    e.component,
                    if config.ten_bit() {
                        AMF_SURFACE_FORMAT_AMF_SURFACE_P010
                    } else {
                        AMF_SURFACE_FORMAT_AMF_SURFACE_NV12
                    },
                    config.width as i32,
                    config.height as i32,
                )
            }))
            .context("AMF encoder Init")?;
            range.validate(e.read(range.name))?;
            // Learn only when the initialized driver reports its buffering
            // requirements. A preset can change these during Init.
            let preanalysis = match config.codec {
                0 => "EnablePreAnalysis",
                1 => "HevcEnablePreAnalysis",
                _ => "Av1EnablePreAnalysis",
            };
            let lookahead = match e.read(preanalysis) {
                Some(0) => Some(0),
                Some(_) => e
                    .read("PALookAheadBufferDepth")
                    .and_then(|n| usize::try_from(n).ok()),
                None => None,
            };
            e.backlog = AmfBacklog::new(lookahead);
            tracing::debug!(
                ?lookahead,
                limit = e.backlog.limit(),
                "AMF application backlog initialized"
            );
            e.log_effective();
            Ok(e)
        }
    }
    /// An integer or boolean property as the driver now has it.
    fn read(&self, name: &str) -> Option<i64> {
        // SAFETY: Self retains the component and runtime, the terminated name and
        // initialized output live through GetProperty, and the tag selects the union member.
        unsafe {
            let mut value = int(0);
            if ((*(*self.component).pVtbl).GetProperty.unwrap())(
                self.component,
                to_wide(name).as_ptr(),
                &mut value,
            ) != AMF_RESULT_AMF_OK
            {
                return None;
            }
            match value.type_ {
                AMF_VARIANT_TYPE_AMF_VARIANT_INT64 => Some(value.__bindgen_anon_1.int64Value),
                AMF_VARIANT_TYPE_AMF_VARIANT_BOOL => {
                    Some(i64::from(value.__bindgen_anon_1.boolValue))
                }
                _ => None,
            }
        }
    }
    /// A rate property, such as the frame rate the rate control budgets for.
    fn read_rate(&self, name: &str) -> Option<(u32, u32)> {
        // SAFETY: The component/runtime and terminated name remain live during the
        // query; success and the RATE tag are checked before reading the initialized union.
        unsafe {
            let mut value = int(0);
            if ((*(*self.component).pVtbl).GetProperty.unwrap())(
                self.component,
                to_wide(name).as_ptr(),
                &mut value,
            ) != AMF_RESULT_AMF_OK
                || value.type_ != AMF_VARIANT_TYPE_AMF_VARIANT_RATE
            {
                return None;
            }
            let rate = value.__bindgen_anon_1.rateValue;
            Some((rate.num, rate.den))
        }
    }
    /// An integer capability of this encoder, such as its engine count.
    fn cap(&self, name: &str) -> Option<i64> {
        // SAFETY: Self retains the component/runtime; caps is checked and its owned
        // reference released after the query. int(0) initializes the read storage and
        // AMF variant assignment zeroes it even for other tags, so the eager int64 read
        // has initialized bytes; only a successful INT64 result is returned.
        unsafe {
            let mut caps = ptr::null_mut();
            if ((*(*self.component).pVtbl).GetCaps.unwrap())(self.component, &mut caps)
                != AMF_RESULT_AMF_OK
                || caps.is_null()
            {
                return None;
            }
            let mut value = int(0);
            let result =
                ((*(*caps).pVtbl).GetProperty.unwrap())(caps, to_wide(name).as_ptr(), &mut value);
            ((*(*caps).pVtbl).Release.unwrap())(caps);
            (result == AMF_RESULT_AMF_OK && value.type_ == AMF_VARIANT_TYPE_AMF_VARIANT_INT64)
                .then_some(value.__bindgen_anon_1.int64Value)
        }
    }
    /// Set a property and read it back: a driver can accept a value and keep
    /// another.
    fn apply(&mut self, property: &butterpollo_core::encoder_policy::Property) -> Result<()> {
        use butterpollo_core::encoder_policy::Value;
        let value = match property.value {
            Value::Integer(n) => int(n),
            Value::Boolean(on) => gpu::boolean(on),
        };
        self.property_raw(&property.name, value)?;
        let mut applied = int(0);
        // SAFETY: The component/runtime, terminated name and initialized output live
        // through GetProperty; each union read is guarded by its matching variant tag.
        unsafe {
            check(((*(*self.component).pVtbl).GetProperty.unwrap())(
                self.component,
                to_wide(&property.name).as_ptr(),
                &mut applied,
            ))?;
            let matches = match property.value {
                Value::Integer(n) => {
                    applied.type_ == AMF_VARIANT_TYPE_AMF_VARIANT_INT64
                        && (applied.__bindgen_anon_1.int64Value == n
                            || (property.name == "Av1NumTilesPerFrame"
                                && applied.__bindgen_anon_1.int64Value > 0))
                }
                Value::Boolean(on) => match applied.type_ {
                    AMF_VARIANT_TYPE_AMF_VARIANT_BOOL => {
                        (applied.__bindgen_anon_1.boolValue != 0) == on
                    }
                    AMF_VARIANT_TYPE_AMF_VARIANT_INT64 => {
                        (applied.__bindgen_anon_1.int64Value != 0) == on
                    }
                    _ => false,
                },
            };
            if !matches {
                bail!(
                    "AMF did not apply {} = {:?} (reported variant {}, integer {})",
                    property.name,
                    property.value,
                    applied.type_,
                    if applied.type_ == AMF_VARIANT_TYPE_AMF_VARIANT_INT64 {
                        applied.__bindgen_anon_1.int64Value
                    } else {
                        -1
                    }
                );
            }
        }
        Ok(())
    }
    /// Split-frame encoding across the GPU's encoder engines, after usage,
    /// rate control and pre-analysis have set the driver's defaults. The
    /// value after Init is in the settings line; even that does not prove a
    /// frame is split, which the driver decides.
    fn request_split_frame(&mut self, options: &butterpollo_core::config::Config) {
        let Some((cap, name)) = butterpollo_core::encoder_policy::amf_multi_instance(self.codec)
        else {
            return;
        };
        let instances = self.cap(cap).unwrap_or(0);
        let driver = self.read(name).map(|value| value != 0);
        let setting = options.get("amd_split_frame", "auto");
        if instances <= 1
            && butterpollo_core::encoder_policy::tristate(options, "amd_split_frame", None)
                == Some(true)
        {
            tracing::warn!(
                instances,
                "AMF split-frame encoding was requested but multiple encode instances are unavailable. Encoding remains on one engine; select Automatic or check the AMD GPU and driver capabilities"
            );
        }
        match butterpollo_core::encoder_policy::amf_split_frame(
            options, self.codec, instances, driver,
        ) {
            None => tracing::info!(
                setting,
                instances,
                driver_value = ?driver,
                "AMF split-frame encoding left to the driver"
            ),
            Some(property) => match self.apply(&property) {
                Ok(()) => tracing::info!(
                    setting,
                    instances,
                    driver_value = ?driver,
                    requested = property.value
                        == butterpollo_core::encoder_policy::Value::Boolean(true),
                    "AMF split-frame encoding requested"
                ),
                Err(error) => tracing::warn!(
                    error = %format!("{error:#}"),
                    setting,
                    instances,
                    "AMF split-frame request not accepted; the driver decides and encoding may take longer. Update the AMD driver or leave split-frame encoding on Automatic"
                ),
            },
        }
    }
    fn setting_names(codec: u8) -> &'static [&'static str] {
        match codec {
            0 => &[
                "Usage",
                "QualityPreset",
                "RateControlMethod",
                "LowLatencyInternal",
                "EnableEncoderSmartAccessVideo",
                "BPicturesPattern",
                "EnablePreAnalysis",
                "RateControlPreanalysisEnable",
                "EnableVBAQ",
                "SlicesPerFrame",
                "InputQueueSize",
                "QueryTimeout",
                "FullRangeColor",
                "MaxNumRefFrames",
                "MaxOfLTRFrames",
                "FrameRate",
                "TargetBitrate",
                "PeakBitrate",
                "VBVBufferSize",
                "MaxAUSize",
                "IntraRefreshMBsNumberPerSlot",
                "EnforceHRD",
                "FillerDataEnable",
                "RateControlSkipFrameEnable",
            ],
            1 => &[
                "HevcUsage",
                "HevcQualityPreset",
                "HevcRateControlMethod",
                "LowLatencyInternal",
                "HevcEnableEncoderSmartAccessVideo",
                "HevcMultiHwInstanceEncode",
                "HevcEnablePreAnalysis",
                "HevcRateControlPreAnalysisEnable",
                "HevcEnableVBAQ",
                "HevcSlicesPerFrame",
                "HevcInputQueueSize",
                "HevcQueryTimeout",
                "HevcNominalRange",
                "HevcMaxNumRefFrames",
                "HevcMaxOfLTRFrames",
                "HevcFrameRate",
                "HevcTargetBitrate",
                "HevcPeakBitrate",
                "HevcVBVBufferSize",
                "HevcMaxAUSize",
                "HevcIntraRefreshCTBsNumberPerSlot",
                "HevcEnforceHRD",
                "HevcFillerDataEnable",
                "HevcRateControlSkipFrameEnable",
            ],
            _ => &[
                "Av1Usage",
                "Av1QualityPreset",
                "Av1RateControlMethod",
                "Av1EncodingLatencyMode",
                "Av1CdfUpdate",
                "Av1CdfFrameEndUpdateMode",
                "Av1EnableEncoderSmartAccessVideo",
                "Av1BPicturesPattern",
                "Av1MultiHwInstanceEncode",
                "Av1EnablePreAnalysis",
                "Av1RateControlPreEncode",
                "Av1AQMode",
                "Av1NumTilesPerFrame",
                "Av1InputQueueSize",
                "Av1QueryTimeout",
                "Av1NominalRange",
                "Av1MaxNumRefFrames",
                "Av1MaxNumLTRFrames",
                "Av1FrameRate",
                "Av1TargetBitrate",
                "Av1PeakBitrate",
                "Av1VBVBufferSize",
                "Av1MaxCompressedFrameSize",
                "Av1IntraRefreshMode",
                "Av1IntraRefreshNumOfStripes",
                "Av1EnforceHRD",
                "Av1FillerData",
                "Av1RateControlSkipFrameEnable",
            ],
        }
    }
    /// The settings that decide encode time and the bits each frame gets, as
    /// the driver applied them.
    fn log_effective(&self) {
        let names = Self::setting_names(self.codec);
        let mut settings: Vec<String> = names
            .iter()
            .map(|name| match self.read(name) {
                Some(value) => format!("{name}={value}"),
                None => match self.read_rate(name) {
                    Some((num, den)) => format!("{name}={num}/{den}"),
                    None => format!("{name}=?"),
                },
            })
            .collect();
        let caps = match self.codec {
            0 => ["NumOfHwInstances", "ColorConversion"],
            1 => ["HevcNumOfHwInstances", "HevcColorConversion"],
            _ => ["Av1CapNumOfHwInstances", "Av1CapColorConversion"],
        };
        for name in caps {
            if let Some(value) = self.cap(name) {
                settings.push(format!("{name}={value}"));
            }
        }
        let gpu_priority = self
            ._device
            .device
            .cast::<windows::Win32::Graphics::Dxgi::IDXGIDevice>()
            // SAFETY: `device` is a live IDXGIDevice obtained by `cast` from this
            // encoder's D3D11 device; the call only reads its priority.
            .and_then(|device| unsafe { device.GetGPUThreadPriority() });
        tracing::info!(
            settings = %settings.join(" "),
            client_max_reference_frames = self.config.references,
            input_memory = if self.compute.is_some() { "D3D12" } else { "D3D11/host" },
            compute_priority = ?self.compute.as_ref().map(|input| input.converter.compute().priority),
            d3d11_gpu_priority = ?gpu_priority,
            "AMF encoder settings"
        );
    }
    pub fn device_removed(&self) -> Option<crate::device_loss::DeviceLost> {
        crate::device_loss::DeviceLost::d3d11(&self._device.device).or_else(|| {
            self.compute.as_ref().and_then(|input| {
                crate::device_loss::DeviceLost::d3d12(&input.converter.compute().device)
            })
        })
    }
    /// Snapshot only at failure/recreation, before dropping the driver state.
    pub fn log_stall(&self) {
        let oldest = self.in_flight.front();
        // SAFETY: the D3D11 device is owned by this encoder and outlives the
        // call; GetDeviceRemovedReason only reads its state.
        let removed = unsafe { self._device.device.GetDeviceRemovedReason() };
        let result_name = |code| match code {
            Some(AMF_RESULT_AMF_OK) => "AMF_OK",
            Some(AMF_RESULT_AMF_REPEAT) => "AMF_REPEAT",
            Some(AMF_RESULT_AMF_EOF) => "AMF_EOF",
            Some(AMF_RESULT_AMF_NEED_MORE_INPUT) => "AMF_NEED_MORE_INPUT",
            Some(AMF_RESULT_AMF_INPUT_FULL) => "AMF_INPUT_FULL",
            Some(_) => "other",
            None => "not called",
        };
        tracing::warn!(
            last_submit = ?self.last_submit,
            last_query = ?self.last_query,
            submit_result = result_name(self.last_submit),
            query_result = result_name(self.last_query),
            input_full = self.input_full,
            in_flight = self.in_flight.len(),
            retained = self.ownership.retained(),
            oldest_pts = ?oldest.map(|s| s.pts),
            oldest_ms = ?oldest.map(|s| s.started.elapsed().as_millis()),
            next_pts = self.index,
            d3d11_removed = ?removed,
            "AMF stall snapshot"
        );
        if let Some(input) = &self.compute {
            input.converter.log_stall();
        }
    }
    fn property(&mut self, name: &str, value: AMFVariantStruct) -> Result<()> {
        let prefix = match self.codec {
            0 => "",
            1 => "Hevc",
            _ => "Av1",
        };
        let name = format!("{prefix}{name}");
        self.property_raw(&name, value)
    }
    /// Write HDR10 static metadata (the mastering display and light levels)
    /// into the bitstream, as FFmpeg's AMF encoder does from a frame's side
    /// data. Clients that size HDR from the stream rather than from the
    /// control channel's copy otherwise get no mastering metadata at all.
    pub fn set_hdr_metadata(&mut self, metadata: butterpollo_core::hdr::Metadata) {
        if !self.config.hdr || self.codec == 0 {
            return;
        }
        let (context, component, codec) = (self.context, self.component, self.codec);
        let update = self.hdr_metadata.update(
            metadata::EncodedMetadata::new(codec, metadata),
            Instant::now(),
            |value| Self::write_hdr_metadata(context, component, codec, value),
        );
        match update {
            metadata::Update::Applied => tracing::info!(
                maximum_nits = metadata.maximum_nits,
                minimum = metadata.minimum,
                max_cll = metadata.max_cll,
                max_fall = metadata.max_fall,
                "AMF HDR metadata accepted; requesting a keyframe to deliver it"
            ),
            metadata::Update::Failed {
                error,
                retry,
                report: true,
            } => {
                tracing::warn!(%error, ?retry, "AMF HDR metadata not written; HDR tone mapping may be incorrect. Transient errors are retried with backoff; unsupported properties require another codec or an AMD driver update")
            }
            _ => {}
        }
    }
    fn write_hdr_metadata(
        context: *mut AMFContext,
        component: *mut AMFComponent,
        codec: u8,
        metadata: &metadata::EncodedMetadata,
    ) -> Result<(), metadata::WriteError> {
        use metadata::{WriteError, WriteStage};
        // SAFETY: The live context allocates owned host memory with room/alignment for
        // AMFHDRMetadata. GetNative remains writable until Release; SetProperty retains
        // its own interface reference before our buffer reference is released once.
        unsafe {
            let mut buffer = ptr::null_mut();
            WriteError::check(
                WriteStage::Buffer,
                ((*(*context).pVtbl).AllocBuffer.unwrap())(
                    context,
                    AMF_MEMORY_TYPE_AMF_MEMORY_HOST,
                    std::mem::size_of::<AMFHDRMetadata>(),
                    &mut buffer,
                ),
            )?;
            if buffer.is_null() {
                return Err(WriteError {
                    stage: WriteStage::Buffer,
                    code: AMF_RESULT_AMF_INVALID_POINTER,
                });
            }
            let native = ((*(*buffer).pVtbl).GetNative.unwrap())(buffer).cast::<AMFHDRMetadata>();
            if native.is_null() {
                ((*(*buffer).pVtbl).Release.unwrap())(buffer);
                return Err(WriteError {
                    stage: WriteStage::Buffer,
                    code: AMF_RESULT_AMF_INVALID_POINTER,
                });
            }
            native.write(metadata.value());
            // SetProperty keeps its own reference to the buffer.
            let name = if codec == 2 {
                "Av1InHDRMetadata"
            } else {
                "HevcInHDRMetadata"
            };
            let result = ((*(*component).pVtbl).SetProperty.unwrap())(
                component,
                to_wide(name).as_ptr(),
                AMFVariantStruct {
                    type_: AMF_VARIANT_TYPE_AMF_VARIANT_INTERFACE,
                    __bindgen_anon_1: AMFVariantStruct__bindgen_ty_1 {
                        pInterface: buffer.cast(),
                    },
                },
            );
            ((*(*buffer).pVtbl).Release.unwrap())(buffer);
            WriteError::check(WriteStage::Property, result)
        }
    }
    fn property_raw(&mut self, name: &str, value: AMFVariantStruct) -> Result<()> {
        // SAFETY: Internal callers initialize the member named by value's tag and
        // retain any referenced interface during the call. Self keeps the component and
        // runtime live; AMF copies the terminated name/value and retains interface values.
        unsafe {
            check(((*(*self.component).pVtbl).SetProperty.unwrap())(
                self.component,
                to_wide(name).as_ptr(),
                value,
            ))
            .with_context(|| format!("AMF property {name}"))
        }
    }
    pub fn pending(&self) -> bool {
        !self.in_flight.is_empty()
    }
    /// When AMF accepted the last submitted input.
    pub fn last_submitted(&self) -> Option<Instant> {
        self.last_accepted
    }
    /// Frames submitted and not yet returned.
    pub fn backlog(&self) -> usize {
        self.in_flight.len()
    }
    /// Application depth, independent of AMF's driver queue and surface pool.
    pub fn backlog_limit(&self) -> usize {
        self.backlog.admission_limit(Instant::now())
    }
    pub fn set_next_frame(&mut self, frame: u64) {
        debug_assert!(self.in_flight.is_empty());
        self.index = frame.saturating_sub(1).min(i64::MAX as u64) as i64;
    }
    pub fn supports_invalidation(&self) -> bool {
        self.references.enabled()
    }
    pub fn invalidation_pending(&self) -> bool {
        self.references.invalidation_pending()
    }
    pub fn invalidate_ref_frames(&mut self, first: u64, last: u64) -> bool {
        // VCN AVC uses a four-bit frame_num with POC type 2. A decoder cannot
        // reliably infer a missing counter wrap. Use a recovery IDR for that
        // window, and keep LTR recovery for losses within the same epoch.
        if self.codec == 0
            && self.references.enabled()
            && first.saturating_sub(2) / 16 != self.index as u64 / 16
        {
            return false;
        }
        self.references.invalidate(first, last)
    }
    fn ltr_properties(&self) -> [&'static str; 4] {
        match self.codec {
            0 => [
                "MaxOfLTRFrames",
                "LTRMode",
                "MarkCurrentWithLTRIndex",
                "ForceLTRReferenceBitfield",
            ],
            1 => [
                "HevcMaxOfLTRFrames",
                "HevcLTRMode",
                "HevcMarkCurrentWithLTRIndex",
                "HevcForceLTRReferenceBitfield",
            ],
            _ => [
                "Av1MaxNumLTRFrames",
                "Av1LTRMode",
                "Av1MarkCurrentWithLTRIndex",
                "Av1ForceLTRReferenceBitfield",
            ],
        }
    }
    fn configure_ltr(&mut self, mut count: usize) {
        if count == 0 || self.config.intra_refresh {
            return;
        }
        let [maximum, mode, _, _] = self.ltr_properties();
        let result = (|| -> Result<usize> {
            // SAFETY: Self retains the component/runtime and its borrowed property info;
            // returned pointers and variant tags are checked before reads. Query outputs
            // and terminated names live through calls, and the owned caps reference is released.
            unsafe {
                let mut info = ptr::null();
                if ((*(*self.component).pVtbl).GetPropertyInfo.unwrap())(
                    self.component,
                    to_wide(maximum).as_ptr(),
                    &mut info,
                ) == AMF_RESULT_AMF_OK
                    && !info.is_null()
                {
                    let maximum = &(*info).maxValue;
                    if maximum.type_ == AMF_VARIANT_TYPE_AMF_VARIANT_INT64 {
                        count = count.min(maximum.__bindgen_anon_1.int64Value.max(0) as usize);
                    }
                }
                if self.codec == 2 {
                    let mut caps = ptr::null_mut();
                    if ((*(*self.component).pVtbl).GetCaps.unwrap())(self.component, &mut caps)
                        == AMF_RESULT_AMF_OK
                        && !caps.is_null()
                    {
                        let mut limit = int(0);
                        let result = ((*(*caps).pVtbl).GetProperty.unwrap())(
                            caps,
                            to_wide("Av1CapMaxNumLTRFrames").as_ptr(),
                            &mut limit,
                        );
                        if result == AMF_RESULT_AMF_OK
                            && limit.type_ == AMF_VARIANT_TYPE_AMF_VARIANT_INT64
                        {
                            count = count.min(limit.__bindgen_anon_1.int64Value.max(0) as usize);
                        }
                        ((*(*caps).pVtbl).Release.unwrap())(caps);
                    }
                }
                if count == 0 {
                    bail!("driver reports no LTR slots");
                }
                self.property_raw(maximum, int(count as i64))?;
                self.property_raw(mode, int(0))?;
                let mut applied = int(0);
                check(((*(*self.component).pVtbl).GetProperty.unwrap())(
                    self.component,
                    to_wide(maximum).as_ptr(),
                    &mut applied,
                ))?;
                if applied.type_ != AMF_VARIANT_TYPE_AMF_VARIANT_INT64
                    || applied.__bindgen_anon_1.int64Value <= 0
                {
                    bail!("LTR slot readback failed");
                }
                Ok(count.min(applied.__bindgen_anon_1.int64Value as usize))
            }
        })();
        match result {
            Ok(count) => {
                self.references = butterpollo_core::ltr::References::new(count);
                tracing::info!(count, "AMF long-term reference recovery enabled");
            }
            Err(error) => {
                let _ = self.property_raw(maximum, int(0));
                tracing::warn!(%error,"AMF LTR unavailable; using larger IDR recovery frames. Disable amd_ltr_frames or update the AMD driver; reduce bitrate if recovery stutters");
            }
        }
    }
    fn prepare_surface(
        &mut self,
        surface: *mut AMFSurface,
        mut idr: bool,
    ) -> Result<butterpollo_core::ltr::Plan> {
        let mut plan = self.references.plan(self.index as u64 + 1, idr);
        let [_, _, mark, reference] = self.ltr_properties();
        let apply = |name: &str, value: AMFVariantStruct| -> Result<()> {
            // SAFETY: Both callers own surface throughout preparation. These local
            // variants have matching scalar tags, and AMF copies them and the terminated
            // property name synchronously while the runtime remains loaded.
            unsafe {
                check(((*(*surface).pVtbl).SetProperty.unwrap())(
                    surface,
                    to_wide(name).as_ptr(),
                    value,
                ))
            }
        };
        let ltr = (|| -> Result<()> {
            if let Some(slot) = plan.mark {
                apply(mark, int(slot as i64))?;
            }
            if let Some(slot) = plan.reference {
                apply(reference, int(1 << slot))?;
            }
            Ok(())
        })();
        if let Err(error) = ltr {
            tracing::warn!(%error,"AMF LTR surface rejected; requesting a larger IDR recovery frame. Disable amd_ltr_frames if this repeats");
            self.references.disable();
            idr = true;
            plan = self.references.plan(self.index as u64 + 1, true);
        }
        if idr {
            apply(
                match self.codec {
                    0 => "ForcePictureType",
                    1 => "HevcForcePictureType",
                    _ => "Av1ForceFrameType",
                },
                int(if self.codec == 2 { 1 } else { 2 }),
            )?;
            // A requested keyframe must start a fresh decoder: a client that
            // reset its decoder otherwise stays frozen. AV1 repeats its
            // sequence header only when asked, as the original backend did.
            let headers: &[&str] = match self.codec {
                0 => &["InsertSPS", "InsertPPS"],
                1 => &["HevcInsertHeader"],
                _ => &["Av1ForceInsertSequenceHeader"],
            };
            for header in headers {
                apply(header, gpu::boolean(true))?;
            }
        }
        Ok(plan)
    }
    fn expire_unreturned(&mut self) {
        // A submission the driver never answers would otherwise keep every
        // later query waiting for its timeout.
        while self
            .in_flight
            .front()
            .is_some_and(|s| s.started.elapsed() > Duration::from_secs(2))
        {
            let stale = self.in_flight.pop_front();
            self.warnings.event("encoder_dropped", format!("AMF returned no output for frame {:?} within two seconds; dropping it and requesting a keyframe. You may see a pause; lower game GPU load or update the AMD driver if this repeats.", stale.map(|s| s.pts)), butterpollo_core::session::EVENT_PERIOD);
            self.recover = true;
        }
    }
    pub fn poll(&mut self) -> Result<Vec<Encoded>> {
        if let Some(input) = &mut self.compute {
            input.converter.release_completed();
        }
        // SAFETY: Self retains the component/runtime; encoder QueryOutput returns an
        // owned AMFBuffer through its AMFData base. Null and size checks bound its host
        // bytes, copied before Release; the picture union is read only with an INT64 tag.
        unsafe {
            let mut output = vec![];
            // With a query timeout, asking when nothing is in flight would only
            // wait for it. One finished frame is handed to the sender at once:
            // asking for the next one still in flight would hold it for the
            // timeout. Callers poll again while frames are pending.
            if self.in_flight.is_empty() {
                return Ok(output);
            }
            let mut data = ptr::null_mut();
            let code = timed("AMF QueryOutput", || {
                ((*(*self.component).pVtbl).QueryOutput.unwrap())(self.component, &mut data)
            });
            self.last_query = Some(code);
            if code == AMF_RESULT_AMF_REPEAT
                || code == AMF_RESULT_AMF_EOF
                || code == AMF_RESULT_AMF_NEED_MORE_INPUT
            {
                // A late caller does not mean the GPU stopped producing.
                // Query first, preserving ready pictures and their timestamps;
                // only an empty query can retire an unanswered submission.
                self.expire_unreturned();
                return Ok(output);
            }
            check(code)?;
            if data.is_null() {
                self.expire_unreturned();
                return Ok(output);
            }
            let buffer = data as *mut AMFBuffer;
            let pts = ((*(*data).pVtbl).GetPts.unwrap())(data);
            let submission = self
                .in_flight
                .iter()
                .position(|s| s.pts == pts)
                .and_then(|position| self.in_flight.remove(position));
            let latency = submission.as_ref().map(|s| s.started.elapsed());
            let presentation = submission.as_ref().map(|s| s.presentation);
            let timing = submission
                .as_ref()
                .map(|s| (s.accepted, s.accepted.duration_since(s.started)));
            let after_invalidation = submission.is_some_and(|s| s.after_invalidation);
            let v = &*(*buffer).pVtbl;
            let size = (v.GetSize.unwrap())(buffer);
            let raw = (v.GetNative.unwrap())(buffer) as *const u8;
            let mut picture: AMFVariantStruct = std::mem::zeroed();
            let prop = match self.codec {
                0 => "OutputDataType",
                1 => "HevcOutputDataType",
                _ => "Av1OutputFrameType",
            };
            let _ = (v.GetProperty.unwrap())(buffer, to_wide(prop).as_ptr(), &mut picture);
            if size > 64 * 1024 * 1024 || raw.is_null() {
                (v.Release.unwrap())(buffer);
                bail!("invalid AMF output buffer");
            }
            let idr = if picture.type_ == AMF_VARIANT_TYPE_AMF_VARIANT_INT64 {
                picture.__bindgen_anon_1.int64Value == 0
            } else {
                self.index == 1
            };
            let bytes = std::slice::from_raw_parts(raw, size as usize);
            let bytes = if self.codec == 0 && idr && self.references.enabled() {
                butterpollo_core::bitstream::h264_reference_recovery(bytes)
            } else {
                bytes.to_vec()
            };
            (v.Release.unwrap())(buffer);
            if let Some((accepted, preparation)) = timing.filter(|_| !bytes.is_empty()) {
                let previous = self.backlog.limit();
                let now = Instant::now();
                self.backlog
                    .completed(now, now.duration_since(accepted), preparation);
                if self.backlog.limit() != previous {
                    tracing::debug!(
                        previous,
                        limit = self.backlog.limit(),
                        "AMF application backlog learned"
                    );
                }
            }
            output.push(Encoded {
                bytes,
                idr,
                after_invalidation,
                latency,
                presentation,
            });
            Ok(output)
        }
    }
    pub fn encode(&mut self, image: &Image, idr: bool, bitrate: u32) -> Result<Vec<Encoded>> {
        let started = Instant::now();
        let idr = idr || self.hdr_metadata.needs_idr() || self.recover;
        self.set_bitrate(bitrate)?;
        if self.convert.is_none() {
            // Native GPU sessions never allocate a CPU YUV frame or swscale
            // context. Allocate compatibility buffers only when used.
            self.convert = Some(Convert::new_config(
                &self.config,
                if self.config.ten_bit() {
                    ff::AVPixelFormat_AV_PIX_FMT_P010LE
                } else {
                    ff::AVPixelFormat_AV_PIX_FMT_NV12
                },
            )?);
        }
        let convert = self.convert.as_mut().unwrap();
        convert.luminance = self.luminance;
        convert.convert(image)?;
        let frame = convert.frame;
        // SAFETY: The converter retains initialized NV12/P010 planes for these dimensions.
        // The live context allocates a separate owned host surface of the same format/size;
        // checked destination pointers/pitches and FFmpeg strides bound each disjoint row copy.
        // AMF retains submitted input before our surface reference is released once.
        unsafe {
            let mut surface = ptr::null_mut();
            check(((*(*self.context).pVtbl).AllocSurface.unwrap())(
                self.context,
                AMF_MEMORY_TYPE_AMF_MEMORY_HOST,
                if self.config.ten_bit() {
                    AMF_SURFACE_FORMAT_AMF_SURFACE_P010
                } else {
                    AMF_SURFACE_FORMAT_AMF_SURFACE_NV12
                },
                self.config.width as i32,
                self.config.height as i32,
                &mut surface,
            ))
            .context("AMF host surface allocation")?;
            let v = &*(*surface).pVtbl;
            let prepared = (|| -> Result<()> {
                check((v.SetCrop.unwrap())(
                    surface,
                    0,
                    0,
                    self.config.width as i32,
                    self.config.height as i32,
                ))
                .context("AMF host surface crop")?;
                (v.SetPts.unwrap())(surface, self.index);
                (v.SetDuration.unwrap())(
                    surface,
                    10_000_000_000 / i64::from(self.config.fps_millihz()),
                );
                for plane_index in 0..2 {
                    let plane = (v.GetPlaneAt.unwrap())(surface, plane_index);
                    if plane.is_null() {
                        bail!("AMF plane missing");
                    }
                    let pv = &*(*plane).pVtbl;
                    let dst = (pv.GetNative.unwrap())(plane) as *mut u8;
                    let pitch = (pv.GetHPitch.unwrap())(plane) as usize;
                    let width =
                        self.config.width as usize * if self.config.ten_bit() { 2 } else { 1 };
                    let height = self.config.height as usize / if plane_index == 0 { 1 } else { 2 };
                    let src = (*frame).data[plane_index];
                    let stride = (*frame).linesize[plane_index] as usize;
                    if dst.is_null() || pitch < width {
                        bail!("invalid AMF host surface pitch");
                    }
                    for row in 0..height {
                        ptr::copy_nonoverlapping(
                            src.add(row * stride),
                            dst.add(row * pitch),
                            width,
                        );
                    }
                }
                Ok(())
            })();
            if let Err(e) = prepared {
                (v.Release.unwrap())(surface);
                return Err(e);
            }
            let plan = match self.prepare_surface(surface, idr) {
                Ok(plan) => plan,
                Err(error) => {
                    (v.Release.unwrap())(surface);
                    return Err(error);
                }
            };
            let mut output = vec![];
            let deadline = Instant::now() + Duration::from_millis(100);
            loop {
                let result = ((*(*self.component).pVtbl).SubmitInput.unwrap())(
                    self.component,
                    surface as *mut AMFData,
                );
                self.last_submit = Some(result);
                acknowledge_keyframe(idr, result, &mut self.recover, &mut self.hdr_metadata);
                if result == AMF_RESULT_AMF_INPUT_FULL {
                    self.input_full += 1;
                    match self.poll() {
                        Ok(frames) => output.extend(frames),
                        Err(e) => {
                            (v.Release.unwrap())(surface);
                            return Err(e);
                        }
                    }
                    if Instant::now() > deadline {
                        (v.Release.unwrap())(surface);
                        bail!("AMF input queue failed to drain");
                    }
                    std::thread::yield_now();
                    continue;
                }
                let result = check(result);
                (v.Release.unwrap())(surface);
                result?;
                break;
            }
            let accepted = Instant::now();
            self.last_accepted = Some(accepted);
            self.in_flight.push_back(Submission {
                pts: self.index,
                started,
                accepted,
                presentation: image.captured,
                after_invalidation: plan.after_invalidation,
                _capture: None,
                _converted: None,
            });
            self.backlog.submitted();
            self.references.accepted(self.index as u64 + 1, &plan);
            self.index += 1;
            output.extend(self.poll()?);
            Ok(output)
        }
    }
}
fn scale_rate_control(value: i64, applied_kbps: u32, requested_kbps: u32) -> i64 {
    let scaled = i128::from(value) * i128::from(requested_kbps) / i128::from(applied_kbps.max(1));
    scaled.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
}
impl Encoder {
    fn set_bitrate(&mut self, bitrate: u32) -> Result<()> {
        let component = self.component;
        self.rate_control.update(
            self.codec,
            bitrate,
            &self.warnings,
            |name| {
                // SAFETY: Self retains the component/runtime and its borrowed
                // property info. Null checks and INT64 tags precede all reads;
                // only copied integers escape this call.
                unsafe {
                    let mut current = int(0);
                    let mut info = ptr::null();
                    let table = &*(*component).pVtbl;
                    if (table.GetProperty.unwrap())(component, to_wide(name).as_ptr(), &mut current)
                        != AMF_RESULT_AMF_OK
                        || current.type_ != AMF_VARIANT_TYPE_AMF_VARIANT_INT64
                        || (table.GetPropertyInfo.unwrap())(
                            component,
                            to_wide(name).as_ptr(),
                            &mut info,
                        ) != AMF_RESULT_AMF_OK
                        || info.is_null()
                    {
                        return None;
                    }
                    Some(rate_control::Limit {
                        value: current.__bindgen_anon_1.int64Value,
                        minimum: ((*info).minValue.type_ == AMF_VARIANT_TYPE_AMF_VARIANT_INT64)
                            .then(|| (*info).minValue.__bindgen_anon_1.int64Value),
                        maximum: ((*info).maxValue.type_ == AMF_VARIANT_TYPE_AMF_VARIANT_INT64)
                            .then(|| (*info).maxValue.__bindgen_anon_1.int64Value),
                    })
                }
            },
            |name, value| {
                // SAFETY: The live component copies the terminated name and
                // the correctly tagged integer during SetProperty.
                unsafe {
                    check(((*(*component).pVtbl).SetProperty.unwrap())(
                        component,
                        to_wide(name).as_ptr(),
                        int(value),
                    ))
                    .with_context(|| format!("AMF property {name}"))
                }
            },
        );
        Ok(())
    }
    pub fn accepts_gpu_device(&self, image: &GpuImage) -> bool {
        self._device.device.as_raw() == image.gpu.device.as_raw()
            && (self.compute.is_none() || crate::compute::shareable(&image.texture))
    }
    fn wait_capacity(&mut self) -> Result<Vec<Encoded>> {
        // Only a full queue waits: a poll blocks for the query timeout while a
        // frame is in flight, which delayed submitting the next frame.
        let mut output = Vec::new();
        let deadline = Instant::now() + Duration::from_millis(100);
        while self.in_flight.len() >= AMF_SURFACE_LIMIT
            || self.ownership.retained() >= AMF_SURFACE_LIMIT
        {
            output.extend(self.poll()?);
            if Instant::now() >= deadline {
                bail!("AMF GPU queue failed to drain");
            }
            std::thread::yield_now();
        }
        Ok(output)
    }
    pub fn encode_gpu(
        &mut self,
        image: &GpuImage,
        idr: bool,
        bitrate: u32,
    ) -> Result<Vec<Encoded>> {
        let started = Instant::now();
        let mut output = self.wait_capacity()?;
        let idr = idr || self.hdr_metadata.needs_idr() || self.recover;
        if self._device.device.as_raw() != image.gpu.device.as_raw() {
            bail!("GPU frame and encoder must use the same D3D11 device");
        }
        let source = (image.width, image.height, image.pixel);
        let (surface, converted) = if let Some(input) = self.compute.as_mut() {
            let compute = input.converter.compute().clone();
            let texture = timed("compute open", || compute.open(&image.texture))?;
            let pointer = image
                .cursor
                .as_ref()
                .map(|cursor| input.converter.pointer(cursor))
                .transpose()?;
            input.converter.values = crate::gpu_color::constants(
                &self.config,
                source,
                self.luminance,
                image.cursor.as_ref(),
            );
            let converted = timed("compute conversion", || {
                input.converter.convert(
                    &texture,
                    crate::compute::format(image.pixel),
                    pointer.as_ref(),
                    image,
                )
            })?;
            timed("conversion fence wait", || {
                gpu::synchronize(&converted.texture, &converted.fence, converted.value)
            })?;
            let surface = timed("AMF surface wrap", || {
                input.context.wrap(&converted.texture, &mut self.ownership)
            })?;
            (surface, None)
        } else {
            if self
                .gpu_convert
                .as_ref()
                .is_none_or(|converter| converter.source != source)
            {
                self.gpu_convert = Some(gpu::Converter::new(&self._device, &self.config, source)?);
            }
            self.gpu_convert
                .as_mut()
                .unwrap()
                .color
                .set_luminance(self.luminance);
            let (surface, texture) = timed("graphics conversion", || {
                self.gpu_convert
                    .as_mut()
                    .unwrap()
                    .convert(self.context, image, &mut self.ownership)
            })?;
            (surface, Some(texture))
        };
        self.set_bitrate(bitrate)?;
        // SAFETY: surface owns a live AMF interface backed by this encoder's device;
        // conversion has synchronized the GPU writes and the boxed ownership observer
        // retains textures until AMF releases them. SubmitInput retains its own surface
        // reference, while self keeps the component, context and runtime alive.
        unsafe {
            let v = &*(*surface.0).pVtbl;
            check((v.SetCrop.unwrap())(
                surface.0,
                0,
                0,
                self.config.width as i32,
                self.config.height as i32,
            ))
            .context("AMF native surface crop")?;
            (v.SetPts.unwrap())(surface.0, self.index);
            (v.SetDuration.unwrap())(
                surface.0,
                10_000_000_000 / i64::from(self.config.fps_millihz()),
            );
            let plan = self.prepare_surface(surface.0, idr)?;
            let deadline = Instant::now() + Duration::from_millis(100);
            loop {
                let result = timed("AMF SubmitInput", || {
                    ((*(*self.component).pVtbl).SubmitInput.unwrap())(
                        self.component,
                        surface.0.cast(),
                    )
                });
                self.last_submit = Some(result);
                acknowledge_keyframe(idr, result, &mut self.recover, &mut self.hdr_metadata);
                if result != AMF_RESULT_AMF_INPUT_FULL {
                    check(result)?;
                    break;
                }
                self.input_full += 1;
                output.extend(self.poll()?);
                if Instant::now() >= deadline {
                    bail!("AMF GPU input queue failed to drain");
                }
                std::thread::yield_now();
            }
            // Compute keeps the capture until conversion completes; graphics
            // still retains it until the encoder returns its output.
            let accepted = Instant::now();
            self.last_accepted = Some(accepted);
            self.in_flight.push_back(Submission {
                pts: self.index,
                started,
                accepted,
                presentation: image.captured,
                after_invalidation: plan.after_invalidation,
                _capture: self.compute.is_none().then(|| image.clone()),
                _converted: converted,
            });
            self.backlog.submitted();
            self.references.accepted(self.index as u64 + 1, &plan);
            self.index += 1;
            output.extend(self.poll()?);
            Ok(output)
        }
    }
}
impl Drop for Encoder {
    fn drop(&mut self) {
        // Destroy the converter before its context and dynamically loaded runtime.
        self.gpu_convert.take();
        // SAFETY: Self owns each non-null interface reference. Terminating the component
        // releases its inputs before compute/context cleanup; the device, runtime and
        // boxed surface observer remain alive through termination and each final Release.
        unsafe {
            if !self.component.is_null() {
                timed("AMF Terminate", || {
                    ((*(*self.component).pVtbl).Terminate.unwrap())(self.component)
                });
                ((*(*self.component).pVtbl).Release.unwrap())(self.component);
            }
            // After the encoder has let go of its D3D12 inputs.
            self.compute.take();
            if !self.context.is_null() {
                timed("AMF context Terminate", || {
                    ((*(*self.context).pVtbl).Terminate.unwrap())(self.context)
                });
                ((*(*self.context).pVtbl).Release.unwrap())(self.context);
            }
        }
    }
}

#[cfg(test)]
mod tests;
