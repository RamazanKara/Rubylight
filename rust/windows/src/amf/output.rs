//! Encoder output: QueryOutput, the units and frames it returns, what each
//! one tells the backlog, the references and recovery, and frames AMF never
//! returned.
use super::*;

pub(super) fn output_is_idr(picture_type: Option<i64>, requested_idr: bool) -> bool {
    // A driver may not expose the output-type property. Use the request attached
    // to this output's PTS; the global input counter can already be several
    // frames ahead, or continue an earlier encoder's frame numbering.
    picture_type.map_or(requested_idr, |kind| kind == 0)
}
impl Encoder {
    fn expire_unreturned(&mut self) {
        // A submission the driver never answers would otherwise keep every
        // later query waiting for its timeout.
        let now = Instant::now();
        while self.in_flight.front().is_some_and(|s| s.unreturned(now)) {
            let stale = self.in_flight.pop_front();
            if let Some(stale) = &stale {
                self.units.forget(stale.pts);
            }
            match unreturned_notice(self.probe, stale.map(|s| s.pts)) {
                Unreturned::Warning(message) => self.warnings.event(
                    "encoder_dropped",
                    message,
                    butterpollo_core::session::EVENT_PERIOD,
                ),
                Unreturned::Probe(message) => tracing::info!("{message}"),
            }
            self.recover = true;
        }
    }
    pub fn poll(&mut self) -> Result<Vec<Encoded>> {
        // A unit of a joined frame leaves nothing to hand on: ask again at
        // once for the frame's next unit instead of after the caller's wait.
        loop {
            self.unit_held = false;
            let output = self.poll_once()?;
            if !(self.unit_held && output.is_empty()) {
                return Ok(output);
            }
        }
    }
    fn poll_once(&mut self) -> Result<Vec<Encoded>> {
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
            if self.unit_log.is_some() && self.log_unit(buffer, pts) {
                return Ok(output);
            }
            // In slice output mode a frame stays in flight until its last unit.
            let kind = if self.slice_blocks > 0 {
                Self::buffer_kind(self.codec, buffer)
            } else {
                None
            };
            let last = kind != Some(1);
            let position = self.in_flight.iter().position(|s| s.pts == pts);
            let submission = position.map(|position| {
                let s = &self.in_flight[position];
                Submitted {
                    started: s.started,
                    accepted: s.accepted,
                    presentation: s.presentation,
                    idr: s.idr,
                    after_invalidation: s.after_invalidation,
                    frame_cap_bytes: s.frame_cap_bytes,
                    frame_cap_target: s.frame_cap_target,
                    reencoded: s.reencoded,
                    skipped: s.skipped,
                }
            });
            let finished = position
                .filter(|_| last)
                .and_then(|position| self.in_flight.remove(position));
            // Nothing submitted after it references the frame yet.
            let newest = finished.is_some() && position == Some(self.in_flight.len());
            let latency = submission.as_ref().map(|s| s.started.elapsed());
            let presentation = submission.as_ref().map(|s| s.presentation);
            let timing = submission
                .as_ref()
                .map(|s| (s.accepted, s.accepted.duration_since(s.started)));
            let requested_idr = submission.as_ref().is_some_and(|s| s.idr);
            let frame_cap_bytes = submission.as_ref().and_then(|s| s.frame_cap_bytes);
            let frame_cap_target = submission.as_ref().and_then(|s| s.frame_cap_target);
            let reencoded = submission.as_ref().is_some_and(|s| s.reencoded);
            let after_invalidation = submission.is_some_and(|s| s.after_invalidation);
            let skipped = submission.map_or(0, |s| s.skipped);
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
            self.focus_feedback(buffer, pts, last);
            if size > 64 * 1024 * 1024 || raw.is_null() {
                (v.Release.unwrap())(buffer);
                bail!("invalid AMF output buffer");
            }
            let picture_type = if picture.type_ == AMF_VARIANT_TYPE_AMF_VARIANT_INT64 {
                Some(picture.__bindgen_anon_1.int64Value)
            } else {
                None
            };
            let idr = output_is_idr(picture_type, requested_idr);
            if idr && !requested_idr {
                // A keyframe the encoder chose (a pre-analysis scene change
                // or the GOP) settles the rate control as a requested one.
                self.focus.keyframe_out();
            }
            if let Some(cut) = finished.as_ref().filter(|cut| {
                reencodes_cut(
                    idr || cut.idr || cut.reencoded,
                    size as u64,
                    cut.cut_limit,
                    newest && !self.preparing && self.slice_blocks == 0,
                )
            }) {
                match self.reencode_as_idr(cut) {
                    Ok(()) => {
                        (v.Release.unwrap())(buffer);
                        // poll() asks again at once, for the keyframe.
                        self.unit_held = true;
                        return Ok(output);
                    }
                    Err(error) => {
                        self.reencode_failures += 1;
                        tracing::error!(error = %format!("{error:#}"), pts, bytes = size, failures = self.reencode_failures, "AMF could not encode an oversize scene cut again as a keyframe; sending the inter frame, which loses its FEC. Report this with the host log.");
                        self.warnings.event("encoder_reencode", format!("AMF could not encode an oversize scene cut again as a keyframe ({error:#}); the frame goes without full FEC and may freeze the picture briefly. Report this with the host log."), butterpollo_core::session::EVENT_PERIOD);
                    }
                }
            }
            let Some(sent) = self.repaired(
                std::slice::from_raw_parts(raw, size as usize),
                skipped,
                idr && requested_idr,
            ) else {
                (v.Release.unwrap())(buffer);
                if self.slice_blocks > 0 {
                    self.units.forget(pts);
                }
                if last {
                    self.frame_completed(timing, false);
                }
                return Ok(output);
            };
            self.frame_bytes += sent.len();
            if last {
                let frame_bytes = std::mem::take(&mut self.frame_bytes);
                if idr && over_cap_target(frame_bytes as u64, frame_cap_target) {
                    self.keyframes_over_cap += 1;
                }
                // The written cap is what the driver held at submission.
                if idr
                    && self.frame_cap.automatic()
                    && self.slice_blocks == 0
                    && let (Some(target), Some(written)) = (frame_cap_target, frame_cap_bytes)
                    && let Some((lowered, now)) =
                        self.keyframe_cap
                            .observe(frame_bytes as u64, target, written)
                {
                    tracing::warn!(
                        codec = self.codec,
                        lowered_overshoot = (lowered * 100.).round() / 100.,
                        overshoot = (now * 100.).round() / 100.,
                        keyframe_bytes = frame_bytes,
                        cap_bytes = written,
                        target_bytes = target,
                        "AMF keyframes did not shrink under a lower frame cap; the driver ignores it for keyframes, so the cap returns to its target for this encoder"
                    );
                }
                self.references.encoded(idr, frame_bytes);
            }
            let bytes: &[u8] = &sent;
            if self.slice_blocks > 0 {
                let streamable = !self.slices_joined
                    && !idr
                    && !after_invalidation
                    && frame_cap_bytes.is_some_and(|cap| cap > 0);
                let pushed = self.units.push(pts, kind, bytes, || units::Meta {
                    streamed: streamable,
                    idr,
                });
                (v.Release.unwrap())(buffer);
                // Whether a frame is a keyframe is read from its first slice.
                // If a later one says otherwise, that assumption is wrong for
                // this driver: say so, once, rather than stream a keyframe.
                if idr != pushed.meta.idr && !self.unit_idr_reported {
                    self.unit_idr_reported = true;
                    tracing::error!(
                        pts,
                        first_slice_idr = pushed.meta.idr,
                        this_slice_idr = idr,
                        "AMF reported a different picture type on a later slice of one frame; slice delivery decides a frame's type from its first slice. Report this with the host log."
                    );
                }
                if let Some(lost) = pushed.abandoned {
                    self.lost_frame(lost);
                }
                let (unit, bytes) = match pushed.step {
                    units::Step::Hold => {
                        self.unit_held = true;
                        return Ok(output);
                    }
                    units::Step::Unit(unit, bytes) => (unit, bytes),
                    units::Step::Whole(bytes) => (Unit::Whole, bytes),
                };
                // Dynamic metadata goes before the first slice or tile, the
                // Dolby Vision RPU after the last (AV1: before the first).
                let (spliced, frame_cap_bytes) = dynamic::output(
                    self.dynamic.as_deref_mut(),
                    unit,
                    &bytes,
                    frame_cap_bytes,
                    pushed.meta.idr,
                );
                let bytes = spliced.unwrap_or(bytes);
                let whole = unit == Unit::Whole;
                // A joined or frame-mode keyframe is recovered as a whole.
                let bytes =
                    if whole && self.codec == 0 && pushed.meta.idr && self.references.enabled() {
                        butterpollo_core::bitstream::h264_reference_recovery(&bytes)
                    } else {
                        bytes
                    };
                if last {
                    self.frame_completed(timing, !bytes.is_empty());
                }
                output.push(Encoded {
                    bytes,
                    unit,
                    idr: whole && pushed.meta.idr,
                    after_invalidation: whole && after_invalidation,
                    frame_cap_bytes,
                    reencoded: whole && reencoded,
                    frame_cap_target,
                    latency,
                    presentation,
                });
                return Ok(output);
            }
            // The sender outlives this AMF buffer reference. Own the compressed
            // bytes before Release makes its native storage available for reuse.
            // H.264 carries no dynamic metadata (`dynamic::wanted`).
            let (bytes, frame_cap_bytes) = if self.codec == 0 && idr && self.references.enabled() {
                (
                    butterpollo_core::bitstream::h264_reference_recovery(bytes),
                    frame_cap_bytes,
                )
            } else {
                let (spliced, cap) = dynamic::output(
                    self.dynamic.as_deref_mut(),
                    Unit::Whole,
                    bytes,
                    frame_cap_bytes,
                    idr,
                );
                (spliced.unwrap_or_else(|| sent.into_owned()), cap)
            };
            (v.Release.unwrap())(buffer);
            self.frame_completed(timing, !bytes.is_empty());
            output.push(Encoded {
                bytes,
                unit: Unit::Whole,
                idr,
                after_invalidation,
                frame_cap_bytes,
                reencoded,
                frame_cap_target,
                latency,
                presentation,
            });
            Ok(output)
        }
    }
    /// [`dynamic::sample`]: a failed statistics pass costs the stream its
    /// dynamic metadata, never a frame.
    pub(super) fn sample_dynamic(&mut self) {
        dynamic::sample(
            &mut self.dynamic,
            stats(
                self.compute.as_deref_mut(),
                self.graphics_stats.as_deref_mut(),
            ),
            &self.warnings,
        );
    }
    /// The `OutputBufferType` of a buffer QueryOutput returned: 0 frame, 1 slice
    /// or tile, 2 last slice or tile. None when the driver does not say.
    ///
    /// # Safety
    /// `buffer` is a live AMF buffer.
    unsafe fn buffer_kind(codec: u8, buffer: *mut AMFBuffer) -> Option<i64> {
        // SAFETY: The caller passes the live buffer; the property storage is
        // initialized and only an INT64 tag is read.
        unsafe {
            let v = &*(*buffer).pVtbl;
            let name = match codec {
                0 => "OutputBufferType",
                1 => "HevcOutputBufferType",
                _ => "AV1OutputBufferType",
            };
            let mut value: AMFVariantStruct = std::mem::zeroed();
            ((v.GetProperty.unwrap())(buffer, to_wide(name).as_ptr(), &mut value)
                == AMF_RESULT_AMF_OK
                && value.type_ == AMF_VARIANT_TYPE_AMF_VARIANT_INT64)
                .then_some(value.__bindgen_anon_1.int64Value)
        }
    }
    /// The backlog learns from every frame AMF finishes.
    fn frame_completed(&mut self, timing: Option<(Instant, Duration)>, produced: bool) {
        if let Some((accepted, preparation)) = timing.filter(|_| produced) {
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
    }
    /// AMF's bytes as the client must get them ([`RepairHeaders`]), or
    /// `None` to drop them: after a rewrite fails, every output until the
    /// requested keyframe, which brings AMF's own headers back.
    ///
    /// [`RepairHeaders`]: butterpollo_core::bitstream::RepairHeaders
    fn repaired<'a>(
        &mut self,
        bytes: &'a [u8],
        skipped: u64,
        requested_keyframe: bool,
    ) -> Option<std::borrow::Cow<'a, [u8]>> {
        #[cfg(test)]
        if let Some(unrewritten) = self.unrewritten.as_mut() {
            unrewritten.extend_from_slice(bytes);
        }
        if self.repair_dropping {
            if !requested_keyframe {
                return None;
            }
            self.repair_dropping = false;
        }
        match butterpollo_core::bitstream::repaired(self.repair_headers.as_mut(), bytes, skipped) {
            Ok(bytes) => Some(bytes),
            Err(error) => {
                tracing::error!(%error, codec = self.codec, "AMF output has headers the long-term reference repair cannot rewrite; dropping frames until a keyframe, then recovering with keyframes for the rest of the session. Report this with the host log.");
                self.repair_headers = None;
                self.references.disable();
                self.recover = true;
                self.repair_dropping = true;
                None
            }
        }
    }
    /// Slice output of a frame that never got its last unit: the sender may
    /// have started it, so the next frame is a keyframe.
    fn lost_frame(&mut self, pts: i64) {
        self.warnings.event("encoder_dropped", format!("AMF returned frame {pts} in slices and never finished it; requesting a keyframe. You may see a pause; lower game GPU load or update the AMD driver if this repeats."), butterpollo_core::session::EVENT_PERIOD);
        self.recover = true;
    }
    /// Blocks a P-frame is sent in, as its slices leave the encoder; 0 when
    /// `video_slice_delivery` is off.
    pub fn slice_delivery(&self) -> u8 {
        if self.slices_joined {
            0
        } else {
            self.slice_blocks
        }
    }
    /// Return every later frame whole, even P-frames, from the next one on.
    /// The encoder stays in slice output mode. A frame whose first unit has
    /// left is finished as it began.
    pub fn join_slices(&mut self) {
        self.slices_joined = true;
    }
    /// Record one output unit for the probe. True for a slice or tile that is
    /// not the last of its frame: the frame stays in flight.
    ///
    /// # Safety
    /// `buffer` is the AMF buffer QueryOutput just returned; this releases it
    /// when it returns true.
    unsafe fn log_unit(&mut self, buffer: *mut AMFBuffer, pts: i64) -> bool {
        let at = Instant::now();
        // SAFETY: The caller passes the live buffer; the property storage is
        // initialized and only an INT64 tag is read.
        unsafe {
            let v = &*(*buffer).pVtbl;
            let kind = Self::buffer_kind(self.codec, buffer);
            let submission = self.in_flight.iter().find(|s| s.pts == pts);
            let unit = OutputUnit {
                pts,
                kind,
                bytes: (v.GetSize.unwrap())(buffer),
                at,
                started: submission.map(|s| s.started),
                accepted: submission.map(|s| s.accepted),
            };
            if let Some(log) = self.unit_log.as_mut() {
                log.push(unit);
            }
            let partial = kind == Some(1);
            if partial {
                (v.Release.unwrap())(buffer);
            }
            partial
        }
    }
}
/// How a frame AMF did not return within two seconds is reported.
#[derive(Debug, PartialEq)]
pub(super) enum Unreturned {
    /// A stream's: shown to the client and in the console.
    Warning(String),
    /// The capability probe's: only logged. The first frame of an encoder the
    /// driver has just started (the service probes at boot or right after an
    /// install, and an iGPU shares memory with the desktop) can be slow; the
    /// probe still judges the codec by what it returns afterwards, and nobody
    /// watches it.
    Probe(String),
}
pub(super) fn unreturned_notice(probe: bool, pts: Option<i64>) -> Unreturned {
    if probe {
        Unreturned::Probe(format!(
            "AMF returned no output for the probe's frame {pts:?} within two seconds; the driver was probably still starting up. The probe goes on with a keyframe and counts the codec by what it returns."
        ))
    } else {
        Unreturned::Warning(format!(
            "AMF returned no output for frame {pts:?} within two seconds; dropping it and requesting a keyframe. You may see a pause; lower game GPU load or update the AMD driver if this repeats."
        ))
    }
}
