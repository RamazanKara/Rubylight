//! Opening an encoder: the device, the AMF runtime and component, the
//! properties set before Init, and the checks of what Init applied.
use super::*;

/// The `ColorTransferChar` AMF writes into the VUI or sequence header
/// (`AMF_COLOR_TRANSFER_CHARACTERISTIC_*`, the H.273 codes): ARIB STD-B67
/// (18) for HLG and Dolby Vision 8.4, SMPTE ST 2084 (16) for other HDR, and
/// each SDR matrix's own curve.
pub(super) fn transfer_characteristic(config: &butterpollo_core::rtsp::Negotiated) -> i64 {
    if config.hlg() {
        18
    } else if config.hdr {
        16
    } else {
        match config.color_matrix() {
            0 => 6,
            2 => 14,
            _ => 1,
        }
    }
}
pub(super) struct OutputRange {
    pub(super) name: &'static str,
    full: bool,
    accepted: bool,
}
impl OutputRange {
    pub(super) fn configure(
        codec: u8,
        full: bool,
        mut set: impl FnMut(&str, bool) -> Result<()>,
    ) -> Self {
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

    pub(super) fn validate(&self, actual: Option<i64>) -> Result<()> {
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
pub(super) fn guard_smart_access_video(
    properties: &mut [butterpollo_core::encoder_policy::Property],
) {
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
    pub(super) fn new_device_alignment_options(
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
        let (encoder, fallback) = intra_refresh::initialize(|allow_hevc_intra_refresh| {
            Self::create_attempt(
                config,
                device.clone(),
                options,
                av1_alignment,
                compute.clone(),
                allow_hevc_intra_refresh,
            )
        })?;
        if let Some(reason) = fallback {
            encoder.warnings.set("encoder_intra_refresh", format!(
                "AMF could not initialize the HEVC intra-refresh workaround ({reason}); a fresh encoder uses the previous refresh policy. Xbox colour artefacts may persist. Update the AMD driver or reconnect the client if needed."
            ));
        }
        Ok(encoder)
    }

    fn create_attempt(
        config: &butterpollo_core::rtsp::Negotiated,
        device: Device,
        options: &butterpollo_core::config::Config,
        av1_alignment: i64,
        compute: Option<std::sync::Arc<crate::compute::Compute>>,
        allow_hevc_intra_refresh: bool,
    ) -> Result<Self> {
        let mut effective_config = config.clone();
        effective_config.intra_refresh =
            butterpollo_core::encoder_policy::amf_intra_refresh(config)
                || intra_refresh::av1_forced(config);
        let slice_blocks = butterpollo_core::encoder_policy::slice_delivery(options, config.codec);
        if slice_blocks > 0 && effective_config.intra_refresh {
            effective_config.intra_refresh = false;
            tracing::warn!(
                "video_slice_delivery is on: intra refresh is off for this stream and a lost frame is recovered with a keyframe"
            );
        }
        if config.intra_refresh && !effective_config.intra_refresh && slice_blocks == 0 {
            if config.codec == 2 {
                tracing::info!(
                    "AMF AV1 does not use the intra refresh the client asked for: a lost frame is recovered by reference invalidation (rust/PERFORMANCE.md, October 27)"
                );
            } else {
                tracing::warn!(
                    client_max_reference_frames = config.references,
                    "AMF H.264 intra refresh requires two references; using larger IDR recovery frames for this decoder. Reduce bitrate if recovery stutters or use a client with two-reference support"
                );
            }
        }
        let config = &effective_config;
        // VCN encodes 4:2:0 only: neither AMF nor D3D12 video encoding offers a
        // 4:4:4 profile or input format (rust/PERFORMANCE.md, October 7).
        if config.yuv444 {
            bail!("AMD's video encoder supports only 4:2:0; PyroWave can stream 4:4:4");
        }
        if config.chroma_packing != 0 {
            config.validate()?;
        }
        if config.ten_bit() && config.codec == 0 {
            bail!("H.264 does not support HDR10");
        }
        // Probe the actual planar target before choosing AMF's memory type.
        // Unsupported NV12/P010 UAV stores otherwise fail every frame after
        // AMF has already committed to D3D12.
        let compute = compute.and_then(|compute| {
            let (width, height) = config.coded_size();
            let prepared = crate::compute::Converter::new(
                compute, width, height, config.ten_bit(),
            ).and_then(|mut converter| {
                if config.chroma_packing != 0 {
                    converter.pack_chroma()?;
                }
                converter.prepare_yuv()?;
                Ok(converter)
            });
            match prepared {
                Ok(converter) => Some(converter),
                Err(error) => {
                    tracing::warn!(error = %format!("{error:#}"), "D3D12 YUV conversion unavailable; using D3D11 conversion");
                    None
                }
            }
        });
        let display_config = config.clone();
        let mut coded_config = config.clone();
        if config.chroma_packing != 0 {
            (coded_config.width, coded_config.height) = config.coded_size();
            coded_config.sdr_10bit = config.ten_bit();
            coded_config.hdr = false;
            coded_config.csc_mode = 3;
            coded_config.chroma_packing = 0;
        }
        let config = &coded_config;
        let limits = butterpollo_core::encoder_policy::amf_rate_control(options, config)?;
        let automatic_frame_cap =
            butterpollo_core::encoder_policy::amf_frame_cap_automatic(options);
        let scale_frame_cap = !automatic_frame_cap
            && limits
                .iter()
                .any(|property| property.name == rate_control::frame_cap_name(config.codec));
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
            let first = init(version, &mut factory);
            if first != AMF_RESULT_AMF_OK {
                // The runtime refuses a header newer than itself. Name both
                // versions, and retry with the runtime's own when it is an
                // older release of the same major version, as on a Radeon
                // driver branch that predates this SDK.
                let mut runtime = 0u64;
                let queried = library
                    .get::<unsafe extern "C" fn(*mut u64) -> AMF_RESULT>(b"AMFQueryVersion\0")
                    .ok()
                    .filter(|query| query(&mut runtime) == AMF_RESULT_AMF_OK)
                    .map(|_| runtime);
                let described = queried
                    .map(butterpollo_core::codec_support::amf_version_text)
                    .unwrap_or_else(|| "unknown".into());
                let header = butterpollo_core::codec_support::amf_version_text(version);
                match butterpollo_core::codec_support::amf_retry_version(version, queried) {
                    Some(older) => {
                        // Every encoder open takes this path on such a driver;
                        // say it once.
                        static RETRY_LOGGED: std::sync::Once = std::sync::Once::new();
                        RETRY_LOGGED.call_once(|| {
                            tracing::warn!(
                                header,
                                runtime = described,
                                "AMFInit refused this host's AMF version; retrying with the driver's older runtime version. Update the Radeon driver for full support"
                            );
                        });
                        factory = ptr::null_mut();
                        check(init(older, &mut factory)).with_context(|| {
                            format!(
                                "AMFInit with the driver's AMF runtime {described} (host built for {header}) also failed after the first attempt returned {}",
                                amf_error_text(first)
                            )
                        })?;
                    }
                    None => {
                        return Err(anyhow::anyhow!(amf_error_text(first))).with_context(|| {
                            format!("AMFInit (host built for AMF {header}, driver runtime {described}); update the Radeon driver")
                        });
                    }
                }
            }
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
                    interface.init(&compute.compute().device)?;
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
                (Some(interface), Some(converter)) => Some(Box::new(ComputeInput {
                    converter,
                    context: interface,
                })),
                _ => None,
            };
            let mut e = Self {
                warnings: Default::default(),
                component,
                context,
                config: display_config,
                codec: config.codec,
                _device: device,
                _library: library,
                index: 0,
                gpu_convert: None,
                in_flight: std::collections::VecDeque::new(),
                backlog: AmfBacklog::new(None),
                recover: false,
                probe: false,
                rate_control: rate_control::State::new(config.bitrate_kbps, scale_frame_cap),
                frame_cap: rate_control::FrameCap::new(automatic_frame_cap),
                cut_limit: None,
                reencoded_cuts: 0,
                frame_cap_target: None,
                keyframes_over_cap: 0,
                keyframe_cap: KeyframeCapScale::default(),
                reencode_failures: 0,
                preparing: false,
                references: Default::default(),
                repair_headers: None,
                repair_dropping: false,
                #[cfg(test)]
                unrewritten: None,
                frame_bytes: 0,
                ownership: gpu::Ownership::new(),
                luminance: [100., 1.],
                overlay: None,
                hdr_metadata: metadata::State::default(),
                compute,
                dynamic: None,
                graphics_stats: None,
                last_query: None,
                last_submit: None,
                last_accepted: None,
                input_full: 0,
                unit_log: None,
                slice_blocks: butterpollo_core::encoder_policy::slice_delivery(
                    options,
                    config.codec,
                ),
                slices_joined: false,
                units: Default::default(),
                unit_held: false,
                unit_idr_reported: false,
                focus: Default::default(),
            };
            let mut properties = butterpollo_core::encoder_policy::amf(options, config)?;
            properties.extend(intra_refresh::av1_forced_properties(config));
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
            let warnings = e.warnings.clone();
            rate_control::configure(&limits, &warnings, |property| e.apply(property))?;
            if config.codec == 0 {
                // Keyframes only when asked, as the original backend sets it:
                // the usage preset's periodic IDRs are large, can overflow
                // FEC at high bitrates and are not tracked for recovery.
                let _ = e.property("IDRPeriod", int(0));
            }
            if config.codec == 1 {
                // Keep normal HEVC free of periodic keyframe bursts. The
                // negotiated intra-refresh workaround below is the scoped
                // exception; never set HevcGOPSPerIDR=0 (RDNA4 recovery stalls).
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
            let transfer = transfer_characteristic(config);
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
            let requested_ltr =
                butterpollo_core::encoder_policy::amf_ltr_requested(options, config);
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
            e.apply_probe_properties(options)?;
            let hevc_request = if allow_hevc_intra_refresh {
                butterpollo_core::encoder_policy::amf_hevc_intra_refresh(config)
            } else {
                None
            };
            let hevc_intra_refresh = intra_refresh::configure(
                hevc_request,
                hevc_request.and_then(|_| e.read(intra_refresh::GOP)),
                hevc_request.and_then(|_| e.read(intra_refresh::CTBS)),
                &warnings,
                |name, value| {
                    e.property_raw(name, int(value))?;
                    Ok(e.read(name))
                },
            )?;
            if e.config.chroma_packing != 0 {
                e.validate_packed_capacity()?;
            }
            let initialized = check(timed("AMF Init", || {
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
            .context("AMF encoder Init");
            if let Err(error) = initialized {
                return Err(if hevc_intra_refresh.is_some() {
                    error.context(intra_refresh::InitializationFailed)
                } else {
                    error
                });
            }
            range.validate(e.read(range.name))?;
            if hevc_intra_refresh.is_some() {
                intra_refresh::validate(
                    hevc_intra_refresh,
                    e.read(intra_refresh::GOP),
                    e.read(intra_refresh::CTBS),
                    &warnings,
                    |name, value| {
                        e.property_raw(name, int(value))?;
                        Ok(e.read(name))
                    },
                )?;
            }
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
            e.frame_cap
                .observe(e.read(rate_control::frame_cap_name(e.codec)));
            e.dynamic = e.enable_dynamic(options);
            e.log_effective();
            Ok(e)
        }
    }
}
