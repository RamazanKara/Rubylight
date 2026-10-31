//! Encoder properties: reads, capabilities, the policy and probe writes,
//! the settings log, HDR metadata and long-term reference setup.
use super::*;

impl Encoder {
    /// An integer or boolean property as the driver now has it.
    pub fn read(&self, name: &str) -> Option<i64> {
        // SAFETY: Self retains the component and its loaded runtime.
        unsafe { Self::read_component(self.component, name) }
    }
    /// The component and its runtime must stay live through the property read.
    pub(super) unsafe fn read_component(component: *mut AMFComponent, name: &str) -> Option<i64> {
        // SAFETY: The caller retains the component and runtime. The terminated
        // name and initialized output live through GetProperty, and the tag
        // selects the initialized union member.
        unsafe {
            let mut value = int(0);
            if ((*(*component).pVtbl).GetProperty.unwrap())(
                component,
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
    pub(super) fn validate_packed_capacity(&self) -> Result<()> {
        let (width, height) = self.config.coded_size();
        let throughput = self
            .cap(if self.codec == 1 {
                "HevcMaxThroughput"
            } else {
                "Av1CapMaxThroughput"
            })
            .filter(|&value| value > 0)
            .context("AMD driver does not expose packed encoder throughput limits")?;
        let requested = u64::from(width.div_ceil(16))
            * u64::from(height.div_ceil(16))
            * u64::from(self.config.fps_millihz());
        anyhow::ensure!(
            requested <= throughput as u64 * 1000,
            "packed {width}x{height} at {} fps exceeds AMD encoder throughput",
            self.config.fps
        );
        // SAFETY: The component owns its caps; both acquired interfaces are released
        // before inspecting the plain range values returned into local storage.
        let (min_w, max_w, min_h, max_h) = unsafe {
            let mut caps = ptr::null_mut();
            check(((*(*self.component).pVtbl).GetCaps.unwrap())(
                self.component,
                &mut caps,
            ))?;
            anyhow::ensure!(!caps.is_null(), "AMF returned no encoder limits");
            let mut input = ptr::null_mut();
            let result = ((*(*caps).pVtbl).GetInputCaps.unwrap())(caps, &mut input);
            ((*(*caps).pVtbl).Release.unwrap())(caps);
            check(result)?;
            anyhow::ensure!(!input.is_null(), "AMF returned no input limits");
            let (mut min_w, mut max_w, mut min_h, mut max_h) = (0, 0, 0, 0);
            ((*(*input).pVtbl).GetWidthRange.unwrap())(input, &mut min_w, &mut max_w);
            ((*(*input).pVtbl).GetHeightRange.unwrap())(input, &mut min_h, &mut max_h);
            ((*(*input).pVtbl).Release.unwrap())(input);
            (min_w, max_w, min_h, max_h)
        };
        anyhow::ensure!(
            (min_w..=max_w).contains(&(width as i32)) && (min_h..=max_h).contains(&(height as i32)),
            "packed {width}x{height} exceeds AMD input limits {min_w}..{max_w} by {min_h}..{max_h}"
        );
        Ok(())
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
            if result != AMF_RESULT_AMF_OK {
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
    /// An integer or boolean capability, for the probes.
    pub fn capability(&self, name: &str) -> Option<i64> {
        self.cap(name)
    }
    /// `probe_amf_properties = Name=int,Name=int`: integer properties set
    /// before Init, by their exact AMF names, for probes. A refusal is an error.
    pub(super) fn apply_probe_properties(
        &mut self,
        options: &butterpollo_core::config::Config,
    ) -> Result<()> {
        let Some(list) = options.values.get("probe_amf_properties") else {
            return Ok(());
        };
        for item in list.split(',').filter(|item| !item.trim().is_empty()) {
            let (name, value) = item
                .split_once('=')
                .with_context(|| format!("probe_amf_properties item {item:?} is not Name=int"))?;
            let value: i64 = value.trim().parse().with_context(|| {
                format!(
                    "probe_amf_properties value of {} is not an integer",
                    name.trim()
                )
            })?;
            self.property_raw(name.trim(), int(value))?;
        }
        Ok(())
    }
    /// Set a property and read it back: a driver can accept a value and keep
    /// another.
    pub(super) fn apply(
        &mut self,
        property: &butterpollo_core::encoder_policy::Property,
    ) -> Result<()> {
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
    pub(super) fn request_split_frame(&mut self, options: &butterpollo_core::config::Config) {
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
    pub(super) fn setting_names(codec: u8) -> &'static [&'static str] {
        match codec {
            0 => &[
                "Usage",
                "Profile",
                "ColorBitDepth",
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
                "HevcProfile",
                "HevcColorBitDepth",
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
                "HevcGOPSize",
                "HevcGOPSPerIDR",
                "HevcIntraRefreshCTBsNumberPerSlot",
                "HevcEnforceHRD",
                "HevcFillerDataEnable",
                "HevcRateControlSkipFrameEnable",
            ],
            _ => &[
                "Av1Usage",
                "Av1Profile",
                "Av1ColorBitDepth",
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
    pub(super) fn log_effective(&self) {
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
    pub(super) fn property(&mut self, name: &str, value: AMFVariantStruct) -> Result<()> {
        let prefix = match self.codec {
            0 => "",
            1 => "Hevc",
            _ => "Av1",
        };
        let name = format!("{prefix}{name}");
        self.property_raw(&name, value)
    }
    /// HDR10+ and HDR Vivid for an HDR stream with `hdr_dynamic_metadata` on,
    /// from the statistics pass behind its conversion, and Dolby Vision 8.1,
    /// 8.4 or 10.1 RPUs for a stream that picked it ([`dynamic::start`]). A
    /// stream that gets less than it wanted says why. Asks with the stream's
    /// own config, not the coded one: a packed 4:4:4 stream codes SDR, but
    /// what it wanted decides what it says.
    pub(super) fn enable_dynamic(
        &mut self,
        options: &butterpollo_core::config::Config,
    ) -> Option<Box<dynamic::Dynamic>> {
        let wanted = dynamic::wanted(&self.config, options)?;
        let packed = self.config.chroma_packing != 0;
        if self.compute.is_none() && !packed {
            self.graphics_stats = Some(Box::new(crate::gpu_color::HdrStats::new(&self._device)));
        }
        dynamic::start(
            wanted,
            packed,
            stats(
                self.compute.as_deref_mut(),
                self.graphics_stats.as_deref_mut(),
            ),
            &self.warnings,
        )
    }
    /// The receiver's display peak
    /// (`butterpollo_core::display_caps::HdrSource::target_nits`). HDR10+
    /// profile A names no target display, so it does not change the units.
    pub fn set_hdr_target(&mut self, nits: u16) {
        if let Some(dynamic) = self.dynamic.as_deref_mut() {
            dynamic.set_target(nits);
        }
    }
    /// Write HDR10 static metadata (the mastering display and light levels)
    /// into the bitstream. Clients that size HDR from the stream rather than from the
    /// control channel's copy otherwise get no mastering metadata at all.
    /// Dynamic metadata takes its first frames' stand-in from it, and Dolby
    /// Vision RPUs carry it from the next frame on.
    pub fn set_hdr_metadata(&mut self, metadata: butterpollo_core::hdr::Metadata) {
        if let Some(dynamic) = self.dynamic.as_deref_mut() {
            dynamic.set_metadata(metadata);
        }
        // A packed 4:4:4 bitstream signals full-range BT.709 SDR: the device
        // unpacks it in its own shader and applies the control channel's
        // copy, so mastering SEI here would contradict the stream's VUI.
        if !self.config.hdr || self.codec == 0 || self.config.chroma_packing != 0 {
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
    pub(super) fn property_raw(&mut self, name: &str, value: AMFVariantStruct) -> Result<()> {
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
    pub(super) fn ltr_properties(&self) -> [&'static str; 4] {
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
    pub(super) fn configure_ltr(&mut self, mut count: usize) {
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
                self.references = butterpollo_core::ltr::References::new(
                    count,
                    butterpollo_core::ltr::Counter::for_codec(self.codec),
                );
                self.repair_headers =
                    butterpollo_core::bitstream::RepairHeaders::for_codec(self.codec);
                tracing::info!(count, "AMF long-term reference recovery enabled");
            }
            Err(error) => {
                let _ = self.property_raw(maximum, int(0));
                tracing::warn!(%error,"AMF LTR unavailable; using larger IDR recovery frames. Disable amd_ltr_frames or update the AMD driver; reduce bitrate if recovery stutters");
            }
        }
    }
}
