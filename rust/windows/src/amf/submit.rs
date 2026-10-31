//! Encoder input: preparing and submitting a surface, the per-frame bitrate
//! and frame cap, waiting for queue capacity, and re-encoding a cut as a
//! keyframe.
use super::*;

pub(super) fn acknowledge_keyframe(
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
/// The pace-based cap target keyframes are measured against: none for an
/// explicit `amd_max_frame_size` (including driver default), which is the
/// user's limit, not the host's.
pub(super) fn frame_cap_target(automatic: bool, bytes: u64) -> Option<u64> {
    (automatic && bytes > 0).then_some(bytes)
}
impl Encoder {
    pub(super) fn prepare_surface(
        &mut self,
        surface: *mut AMFSurface,
        idr: bool,
    ) -> Result<(butterpollo_core::ltr::Plan, bool)> {
        let (plan, idr, ltr_rejected) =
            self.prepare_surface_at(surface, self.index as u64 + 1, idr)?;
        if ltr_rejected {
            self.references.disable();
        }
        Ok((plan, idr))
    }
    /// [`Self::prepare_surface`] for wire frame `frame`: a re-encode prepares
    /// the frame already submitted again. It changes only the surface: when
    /// the surface rejects the LTR properties, the plan is the keyframe of
    /// disabled references and the caller disables them (the third value)
    /// once that is certain.
    fn prepare_surface_at(
        &mut self,
        surface: *mut AMFSurface,
        frame: u64,
        mut idr: bool,
    ) -> Result<(butterpollo_core::ltr::Plan, bool, bool)> {
        let mut plan = self.references.plan(frame, idr);
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
        let ltr_rejected = ltr.is_err();
        if let Err(error) = ltr {
            tracing::warn!(%error,"AMF LTR surface rejected; requesting a larger IDR recovery frame. Disable amd_ltr_frames if this repeats");
            idr = true;
            plan = butterpollo_core::ltr::References::disabled_keyframe_plan();
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
        // LTR rejection can force a keyframe here even when the caller did not
        // request one. Retain that effective type for acknowledgements and output.
        Ok((plan, idr, ltr_rejected))
    }
    /// A CPU image, such as the codec probe's, goes through the same GPU
    /// conversion as a capture.
    pub fn encode(&mut self, image: &Image, idr: bool, bitrate: u32) -> Result<Vec<Encoded>> {
        anyhow::ensure!(
            self.overlay.is_none(),
            "couch menu: needs wgc_direct_encoder_input"
        );
        let image = GpuImage::upload(&self._device, image).context("AMF image upload")?;
        self.encode_gpu(&image, idr, bitrate)
    }
    /// Apply a pace-based encoded-byte budget. An explicit user setting,
    /// including driver default (zero), is preserved. Optional property failures
    /// warn without interrupting the stream; return only the observed limit.
    /// An automatic cap in frame mode is written under `bytes` while AMF's
    /// keyframes overshoot it ([`KeyframeCapScale`]), never under `floor`;
    /// submissions keep `bytes` as their target.
    pub fn set_frame_cap(&mut self, bytes: u64, floor: u64) -> Option<u64> {
        let target = bytes;
        self.frame_cap_target = frame_cap_target(self.frame_cap.automatic(), target);
        let bytes = butterpollo_core::encoder_policy::keyframe_cap_bytes(
            self.frame_cap.automatic(),
            self.slice_blocks,
            &self.keyframe_cap,
            target,
            floor,
        );
        let component = self.component;
        self.frame_cap
            .update_toward(self.codec, bytes, target, &self.warnings, |name, bits| {
                // SAFETY: Self owns the live component/runtime for this entire
                // update. SetProperty copies the terminated name and integer.
                unsafe {
                    let result = check(((*(*component).pVtbl).SetProperty.unwrap())(
                        component,
                        to_wide(name).as_ptr(),
                        int(bits),
                    ));
                    (result, Self::read_component(component, name))
                }
            })
    }
    pub fn frame_cap(&self) -> Option<u64> {
        self.frame_cap.effective()
    }
    /// The largest inter frame sent as encoded ([`reencodes_cut`]), with an
    /// automatic frame cap only: an explicit `amd_max_frame_size` is kept.
    pub fn set_cut_limit(&mut self, bytes: u64) {
        self.cut_limit = (self.frame_cap.automatic() && bytes > 0).then_some(bytes);
    }
    pub fn reencoded_cuts(&self) -> u64 {
        self.reencoded_cuts
    }
    /// Keyframes AMF made more than 1.1 times the cap the host asked for.
    pub fn keyframes_over_cap(&self) -> u64 {
        self.keyframes_over_cap
    }
    pub fn keyframe_cap_scale(&self) -> f64 {
        self.keyframe_cap.scale()
    }
    fn set_bitrate(&mut self, bitrate: u32) -> Result<()> {
        let changed = bitrate != self.rate_control.requested();
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
        if changed {
            self.frame_cap
                .observe(self.read(rate_control::frame_cap_name(self.codec)));
        }
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
            let queried = Instant::now();
            output.extend(self.poll()?);
            // A completion at the deadline still frees capacity successfully.
            let retained = self.ownership.retained();
            if self.in_flight.len() < AMF_SURFACE_LIMIT && retained < AMF_SURFACE_LIMIT {
                break;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                bail!("AMF GPU queue failed to drain");
            }
            if retained >= AMF_SURFACE_LIMIT {
                // Output can be complete while AMF still owns every input.
                // QueryOutput then returns immediately; wait on the native
                // release notification instead of spinning on that empty poll.
                self.ownership
                    .wait_below(AMF_SURFACE_LIMIT, remaining.min(QUERY_TIMEOUT));
            } else {
                // Some drivers ignore QueryTimeout or return REPEAT early.
                // Bound those retries too, without extending a real driver wait.
                let pause = QUERY_TIMEOUT
                    .saturating_sub(queried.elapsed())
                    .min(remaining);
                if !pause.is_zero() {
                    std::thread::sleep(pause);
                }
            }
        }
        Ok(output)
    }
    /// Submit `cut`'s converted input again as a keyframe, in place of the
    /// inter frame AMF made of it ([`reencodes_cut`]). The keyframe takes the
    /// frame's index and resets the long-term references, so a later repair
    /// names only the keyframe. SubmitInput is the last step that can fail:
    /// before it, nothing has changed.
    pub(super) fn reencode_as_idr(&mut self, cut: &Submission) -> Result<()> {
        let surface = match (&cut._converted, &cut._converted12, self.compute.as_ref()) {
            (Some(texture), _, _) => gpu::wrap(self.context, texture, &mut self.ownership)?.0,
            (None, Some(texture), Some(input)) => {
                input.context.wrap(texture, &mut self.ownership)?
            }
            _ => bail!("the frame's converted input was not kept"),
        };
        let frame = cut.pts as u64 + 1;
        // SAFETY: As in `encode_gpu`: the fresh surface wraps the converted
        // texture the submission kept, which no later conversion has reused.
        unsafe {
            let v = &*(*surface.0).pVtbl;
            check((v.SetCrop.unwrap())(
                surface.0,
                0,
                0,
                self.config.coded_size().0 as i32,
                self.config.coded_size().1 as i32,
            ))
            .context("AMF native surface crop")?;
            (v.SetPts.unwrap())(surface.0, cut.pts);
            (v.SetDuration.unwrap())(
                surface.0,
                10_000_000_000 / i64::from(self.config.fps_millihz()),
            );
            let (plan, _, ltr_rejected) = self.prepare_surface_at(surface.0, frame, true)?;
            let result = timed("AMF SubmitInput", || {
                ((*(*self.component).pVtbl).SubmitInput.unwrap())(self.component, surface.0.cast())
            });
            self.last_submit = Some(result);
            check(result).context("AMF SubmitInput of the keyframe")?;
            // Only now: had SubmitInput failed, the references stay as they were.
            if ltr_rejected {
                self.references.disable();
            }
            acknowledge_keyframe(true, result, &mut self.recover, &mut self.hdr_metadata);
            let accepted = Instant::now();
            self.last_accepted = Some(accepted);
            self.in_flight.push_back(Submission {
                pts: cut.pts,
                // From the first submission: the frame's encode time counts both.
                started: cut.started,
                accepted,
                presentation: cut.presentation,
                idr: true,
                after_invalidation: false,
                frame_cap_bytes: cut.frame_cap_bytes,
                frame_cap_target: cut.frame_cap_target,
                skipped: plan.skipped,
                cut_limit: cut.cut_limit,
                reencoded: true,
                _capture: cut._capture.clone(),
                _converted: cut._converted.clone(),
                _converted12: None,
            });
            self.references.accepted(frame, &plan);
            self.focus.keyframe_out();
            self.reencoded_cuts += 1;
            Ok(())
        }
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
        let (surface, converted, conversion) = if let Some(input) = self.compute.as_mut() {
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
            input.converter.set_overlay(self.overlay.as_ref());
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
            // Kept while the frame may need encoding again (`reencodes_cut`).
            let kept = self.cut_limit.map(|_| converted.texture.clone());
            (surface, (None, kept), Some(converted.conversion))
        } else {
            if self
                .gpu_convert
                .as_ref()
                .is_none_or(|converter| converter.source != source)
            {
                self.gpu_convert = Some(gpu::Converter::new(&self._device, &self.config, source)?);
            }
            let color = &mut self.gpu_convert.as_mut().unwrap().color;
            color.set_luminance(self.luminance);
            color.set_overlay(self.overlay.as_ref());
            let (surface, texture) = timed("graphics conversion", || {
                self.gpu_convert
                    .as_mut()
                    .unwrap()
                    .convert(self.context, image, &mut self.ownership)
            })?;
            // Submitted with `sample_dynamic`, once the frame went to the encoder.
            if let Some(stats) = self.graphics_stats.as_deref_mut() {
                stats.queue(image, &self.gpu_convert.as_ref().unwrap().color);
            }
            (surface, (Some(texture), None), None)
        };
        let (converted, converted12) = converted;
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
                self.config.coded_size().0 as i32,
                self.config.coded_size().1 as i32,
            ))
            .context("AMF native surface crop")?;
            (v.SetPts.unwrap())(surface.0, self.index);
            (v.SetDuration.unwrap())(
                surface.0,
                10_000_000_000 / i64::from(self.config.fps_millihz()),
            );
            let (plan, idr) = self.prepare_surface(surface.0, idr)?;
            self.attach_focus(surface.0, idr, plan.after_invalidation, conversion)?;
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
                // This input's references are planned: a re-encode now would
                // drop the ones it may name.
                self.preparing = true;
                let polled = self.poll();
                self.preparing = false;
                output.extend(polled?);
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
                idr,
                after_invalidation: plan.after_invalidation,
                frame_cap_bytes: self.frame_cap.effective(),
                frame_cap_target: self.frame_cap_target,
                skipped: plan.skipped,
                cut_limit: self.cut_limit,
                reencoded: false,
                _capture: self.compute.is_none().then(|| image.clone()),
                _converted: converted,
                _converted12: converted12,
            });
            self.backlog.submitted();
            self.references.accepted(self.index as u64 + 1, &plan);
            self.index += 1;
            // Before the poll, so the frame it returns carries the newest
            // finished sample; the frames already collected stay either way.
            self.sample_dynamic();
            output.extend(self.poll()?);
            Ok(output)
        }
    }
}
