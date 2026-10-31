//! Dynamic HDR metadata in AMF's output: HDR10+ and HDR Vivid
//! (`butterpollo_core::hdr_dynamic`; HDR Vivid alone on HLG streams) before
//! each frame's first slice, and a Dolby Vision profile 8.1 or 8.4 RPU
//! (`butterpollo_core::dovi`) after its last; on AV1, a profile 10.1 RPU
//! metadata OBU goes before the frame too, after the HDR10+ one. The
//! conversion's statistics pass (`compute_hdr_stats.rs` behind the compute
//! conversion, `gpu_color/hdr_stats.rs` behind the D3D11 one) measures each
//! frame once it went to the encoder; the newest finished sample is smoothed
//! into what every output frame carries from then on. Nothing waits for a
//! sample: a frame that comes out before its own has finished carries the
//! previous one. Before the first sample, frames carry a stand-in from the
//! static HDR10 levels.
use crate::compute::HdrStatsReadout;
use crate::encoder::Unit;
use butterpollo_core::dovi;
use butterpollo_core::hdr_dynamic::{self, Codec, Writer};
use butterpollo_core::session::Warnings;
use std::time::{Duration, Instant};

/// What an HDR stream's frames carry beside the picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Wanted {
    pub(super) codec: Codec,
    /// HDR10+ and HDR Vivid (`hdr_dynamic_metadata`), HDR Vivid alone on an
    /// HLG stream.
    pub(super) t35: bool,
    /// A Dolby Vision RPU on every frame ([`dovi::negotiated`]): profile 10.1
    /// on AV1, 8.4 on an HLG HEVC stream, else 8.1.
    pub(super) dolby_vision: bool,
    /// An HLG stream (`Negotiated::hlg`): HLG or Dolby Vision 8.4.
    pub(super) hlg: bool,
}

impl Wanted {
    #[cfg(test)]
    pub(super) fn t35(codec: Codec) -> Self {
        Self {
            codec,
            t35: true,
            dolby_vision: false,
            hlg: false,
        }
    }

    /// The units the statistics become, and the RPU profile.
    fn writers(self, target_nits: u16) -> (Writer, Option<dovi::Writer>) {
        let writer = Writer::new(self.codec);
        // AV1 carries Dolby Vision over PQ only (`dovi::negotiated`).
        let (writer, profile) = if self.hlg {
            (writer.with_hlg(), dovi::Profile::P84)
        } else if self.codec == Codec::Av1 {
            (writer, dovi::Profile::P101)
        } else {
            (writer, dovi::Profile::P81)
        };
        let rpus = self
            .dolby_vision
            .then(|| dovi::Writer::new(target_nits).with_profile(profile));
        (writer, rpus)
    }
}

/// HEVC or AV1 HDR streams with `hdr_dynamic_metadata` on (HEVC only for HLG:
/// HDR Vivid has no AV1 carriage and HDR10+ describes PQ only), HEVC streams
/// whose client picked Dolby Vision 8.1 or 8.4, and AV1 streams whose client
/// picked 10.1. `None` leaves every output byte as AMF wrote it. Ten-bit SDR
/// never gets any, even if a path marked it SDR without clearing `hdr`.
pub(super) fn wanted(
    config: &butterpollo_core::rtsp::Negotiated,
    options: &butterpollo_core::config::Config,
) -> Option<Wanted> {
    if !config.hdr || config.sdr_10bit {
        return None;
    }
    let codec = Codec::from_video_format(config.codec)?;
    let hlg = config.hlg();
    let wanted = Wanted {
        codec,
        t35: options.hdr_dynamic_metadata() && !(hlg && codec == Codec::Av1),
        dolby_vision: dovi::negotiated(config).is_some(),
        hlg,
    };
    (wanted.t35 || wanted.dolby_vision).then_some(wanted)
}

/// The statistics pass behind the compute conversion
/// ([`crate::compute::Converter`]) or the D3D11 one
/// ([`crate::gpu_color::HdrStats`]), or a fake in tests.
pub(super) trait Stats {
    fn enable(&mut self) -> anyhow::Result<()>;
    fn submit(&mut self) -> anyhow::Result<()>;
    fn latest(&self, newer_than: u64) -> anyhow::Result<Option<HdrStatsReadout>>;
    /// No more passes: dynamic metadata is off for the rest of the stream.
    fn stop(&mut self);
}
impl Stats for crate::compute::Converter {
    fn enable(&mut self) -> anyhow::Result<()> {
        self.enable_hdr_stats()
    }
    fn submit(&mut self) -> anyhow::Result<()> {
        super::timed("HDR statistics submit", || self.submit_hdr_stats())
    }
    fn latest(&self, newer_than: u64) -> anyhow::Result<Option<HdrStatsReadout>> {
        self.latest_hdr_stats(newer_than)
    }
    fn stop(&mut self) {
        self.stop_hdr_stats();
    }
}
impl Stats for crate::gpu_color::HdrStats {
    fn enable(&mut self) -> anyhow::Result<()> {
        crate::gpu_color::HdrStats::enable(self)
    }
    fn submit(&mut self) -> anyhow::Result<()> {
        super::timed("HDR statistics submit", || {
            crate::gpu_color::HdrStats::submit(self)
        })
    }
    fn latest(&self, newer_than: u64) -> anyhow::Result<Option<HdrStatsReadout>> {
        crate::gpu_color::HdrStats::latest(self, newer_than)
    }
    fn stop(&mut self) {
        crate::gpu_color::HdrStats::stop(self);
    }
}

/// Something a stream wanted is off: the log and the session's warnings
/// (under the setting's name) say why.
fn off(warnings: &Warnings, code: &'static str, message: String) {
    tracing::warn!("{message}");
    warnings.set(code, message);
}

/// The statistics are gone for this stream (`why`, from `stream` on): HDR10+
/// is off, and Dolby Vision RPUs carry the static light levels.
fn unmeasured(wanted: Wanted, stream: &str, why: &str, warnings: &Warnings) {
    if wanted.t35 {
        let message = if wanted.hlg {
            format!("HDR Vivid is off for {stream}: {why}")
        } else {
            format!(
                "HDR10+ and HDR Vivid are off for {stream}: {why}. It carries static HDR10 metadata only"
            )
        };
        off(warnings, "hdr_dynamic_metadata", message);
    }
    if wanted.dolby_vision {
        off(
            warnings,
            "hdr_dolby_vision",
            format!(
                "Dolby Vision carries the stream's static light levels for {stream}, not each frame's: {why}"
            ),
        );
    }
}

/// Dynamic metadata for an HDR stream that wants it ([`wanted`]), measured
/// by the statistics pass behind its conversion (`stats`; `None` for an
/// input that has none). Without the pass, HDR10+ is off and Dolby Vision
/// goes on with the static metadata; the log and the session's warnings say
/// why. `packed` 4:4:4 carries neither and runs no statistics pass: its
/// bitstream says SDR, and the device unpacks the picture in its own shader
/// and takes the static HDR10 metadata from the control channel, so no
/// decoder would read them (`stream_policy::packed_drops_dolby_vision`).
pub(super) fn start(
    wanted: Wanted,
    packed: bool,
    stats: Option<&mut (impl Stats + ?Sized)>,
    warnings: &Warnings,
) -> Option<Box<Dynamic>> {
    if packed {
        if wanted.dolby_vision {
            off(
                warnings,
                "hdr_dolby_vision",
                "Dolby Vision is off for this stream: packed 4:4:4 streams carry static HDR10 only. Report this with the host log, the negotiation should have dropped it".to_owned(),
            );
        }
        if wanted.t35 {
            tracing::info!(
                codec = ?wanted.codec,
                "HDR10+ and HDR Vivid are off for this packed 4:4:4 stream: the device unpacks the picture itself and takes the static HDR10 metadata from the control channel, so no decoder would read them"
            );
        }
        return None;
    }
    let why = match stats.map(|stats| stats.enable()) {
        Some(Ok(())) => {
            tracing::info!(
                codec = ?wanted.codec,
                hdr10_plus = wanted.t35 && !wanted.hlg,
                vivid = wanted.t35 && wanted.codec == Codec::Hevc,
                dolby_vision = wanted.dolby_vision,
                hlg = wanted.hlg,
                packed,
                "HDR dynamic metadata on"
            );
            return Some(Box::new(Dynamic::with(wanted, true)));
        }
        Some(Err(error)) => format!(
            "the GPU statistics pass did not start ({error:#}). Update the AMD driver, or turn off hdr_dynamic_metadata"
        ),
        None => {
            "its encoder input has no GPU statistics pass. Report this with the host log".to_owned()
        }
    };
    unmeasured(wanted, "this stream", &why, warnings);
    if !wanted.dolby_vision {
        return None;
    }
    tracing::info!(
        codec = ?wanted.codec,
        hlg = wanted.hlg,
        "Dolby Vision on, from the static metadata"
    );
    Some(Box::new(Dynamic::with(
        Wanted {
            t35: false,
            ..wanted
        },
        false,
    )))
}

/// Once a frame went to the encoder: start its statistics pass, and take
/// the newest one finished, while the encoder works on the frame. Never
/// waits for the GPU. A pass that fails ends the measurements for the rest
/// of the stream and says so: it costs no frame, HDR10+ goes off and Dolby
/// Vision goes back to the static light levels.
pub(super) fn sample(
    slot: &mut Option<Box<Dynamic>>,
    stats: Option<&mut (impl Stats + ?Sized)>,
    warnings: &Warnings,
) {
    let (Some(dynamic), Some(stats)) = (slot.as_deref_mut(), stats) else {
        return;
    };
    if !dynamic.measuring {
        return;
    }
    let started = Instant::now();
    if let Err(error) = stats.submit() {
        stats.stop();
        unmeasured(
            dynamic.wanted,
            "the rest of this stream",
            &format!("the GPU statistics pass failed ({error:#}). Report this with the host log"),
            warnings,
        );
        if !dynamic.wanted.dolby_vision {
            *slot = None;
            return;
        }
        dynamic.measuring = false;
        dynamic.wanted.t35 = false;
        if let Some(rpus) = dynamic.dovi.as_mut() {
            rpus.forget_measurements();
        }
        return;
    }
    dynamic.submitted(started);
    let started = Instant::now();
    dynamic.take(stats.latest(dynamic.sampled()), started);
}

/// An output `unit` as the sender gets it, from the `bytes` AMF wrote and
/// its frame `cap`: HDR10+ and HDR Vivid go before a frame's first slice or
/// tile, the Dolby Vision RPU after its last (a whole frame gets both) or,
/// on AV1, before its first tile after HDR10+, and every unit of that frame
/// gets a cap that counts what the frame gained. A `keyframe` refreshes the
/// RPU's scene. The bytes are `None` to send them as they are: without
/// dynamic metadata, a middle unit, or nothing added.
pub(super) fn output(
    dynamic: Option<&mut Dynamic>,
    unit: Unit,
    bytes: &[u8],
    cap: Option<u64>,
    keyframe: bool,
) -> (Option<Vec<u8>>, Option<u64>) {
    let Some(dynamic) = dynamic else {
        return (None, cap);
    };
    let first = matches!(
        unit,
        Unit::Whole | Unit::Slice { index: 0 } | Unit::Last { index: 0 }
    );
    let mut out = None;
    if first {
        dynamic.next_rpu(!bytes.is_empty(), keyframe);
        out = dynamic.first_unit(bytes);
    }
    if matches!(unit, Unit::Whole | Unit::Last { .. }) {
        out = dynamic.last_unit(out, bytes);
    }
    (out, dynamic.cap(cap))
}

/// One encoder's dynamic metadata.
pub(super) struct Dynamic {
    wanted: Wanted,
    /// HDR10+ and HDR Vivid, and the smoothing every sample goes through.
    writer: Writer,
    display_peak: u16,
    dovi: Option<dovi::Writer>,
    /// The statistics pass runs for this stream.
    measuring: bool,
    /// The conversion the last sample measured: each is smoothed once.
    sampled: u64,
    /// Bytes the current frame gained. Each of its units' caps grows by as
    /// much, so a streamed frame's payload stays within it.
    added: u64,
    /// The current frame's RPU: an HEVC NAL unit until the frame's last unit
    /// takes it, an AV1 metadata OBU until its first does.
    rpu: Vec<u8>,
    report: Report,
}

/// What a stream's dynamic metadata did, logged when its encoder closes.
#[derive(Default)]
struct Report {
    frames: u64,
    spliced: u64,
    /// Spliced with the static stand-in, before the first sample.
    stand_in: u64,
    samples: u64,
    scene_cuts: u64,
    invalid: u64,
    readout_errors: u64,
    no_insertion: u64,
    /// Frames that got an RPU, and how many of them before any measurement.
    rpus: u64,
    unmeasured_rpus: u64,
    /// CPU time of submitting the statistics pass, taking a sample,
    /// splicing the units into a frame, and writing an RPU.
    submit: Cost,
    sample: Cost,
    splice: Cost,
    rpu: Cost,
}

#[derive(Default)]
struct Cost {
    total: Duration,
    worst: Duration,
    count: u64,
}
impl Cost {
    fn add(&mut self, started: Instant) {
        let elapsed = started.elapsed();
        self.total += elapsed;
        self.worst = self.worst.max(elapsed);
        self.count += 1;
    }
    fn mean_us(&self) -> f64 {
        self.total.as_secs_f64() * 1e6 / self.count.max(1) as f64
    }
    fn worst_us(&self) -> u64 {
        self.worst.as_micros() as u64
    }
}

impl Dynamic {
    /// HDR10+ and HDR Vivid from the statistics pass.
    #[cfg(test)]
    pub(super) fn new(codec: Codec) -> Self {
        Self::with(Wanted::t35(codec), true)
    }
    /// Until the stream names its target display, the 1000-nit default of
    /// [`hdr_dynamic::target_display_nits`]; until it names its static
    /// metadata, `hdr::Metadata::default`.
    pub(super) fn with(wanted: Wanted, measuring: bool) -> Self {
        let target = hdr_dynamic::target_display_nits(None, None);
        let (writer, dovi) = wanted.writers(target);
        Self {
            wanted,
            writer,
            display_peak: target,
            dovi,
            measuring,
            sampled: 0,
            added: 0,
            rpu: Vec::new(),
            report: Report::default(),
        }
    }
    /// The conversion the last sample measured, for
    /// [`crate::compute::Converter::latest_hdr_stats`].
    pub(super) fn sampled(&self) -> u64 {
        self.sampled
    }
    /// The receiver's display peak. HDR10+ profile A and HDR Vivid name no
    /// target display, so the units stay as they are; Dolby Vision's L2
    /// trims are written for it.
    pub(super) fn set_target(&mut self, nits: u16) {
        if self.display_peak != nits {
            tracing::info!(
                nits,
                previous = self.display_peak,
                "HDR dynamic metadata receiver display peak"
            );
            self.display_peak = nits;
            if let Some(rpus) = self.dovi.as_mut() {
                rpus.set_target_nits(nits);
            }
        }
    }
    /// The stream's static HDR10 metadata, which frames before the first
    /// sample carry as a stand-in, and which every RPU carries.
    pub(super) fn set_metadata(&mut self, metadata: butterpollo_core::hdr::Metadata) {
        self.writer.set_metadata(&metadata);
        if let Some(rpus) = self.dovi.as_mut() {
            rpus.set_metadata(metadata);
        }
    }
    pub(super) fn submitted(&mut self, started: Instant) {
        self.report.submit.add(started);
    }
    /// Take what [`crate::compute::Converter::latest_hdr_stats`] found.
    /// Nothing new keeps the current units; a readout that disagrees with
    /// itself is logged and skipped.
    pub(super) fn take(
        &mut self,
        readout: anyhow::Result<Option<HdrStatsReadout>>,
        started: Instant,
    ) {
        match readout {
            Ok(Some(readout)) => {
                self.sampled = readout.conversion;
                match readout
                    .frame_stats()
                    .and_then(|stats| self.writer.sample(&stats))
                {
                    Some(smoothed) => {
                        self.report.samples += 1;
                        self.report.scene_cuts += u64::from(smoothed.scene_cut);
                        if let Some(rpus) = self.dovi.as_mut() {
                            rpus.sample(&smoothed);
                        }
                    }
                    None => {
                        if self.report.invalid == 0 {
                            tracing::error!(
                                conversion = readout.conversion,
                                min_pq = readout.min_pq,
                                max_pq = readout.max_pq,
                                "HDR statistics gave invalid values; frames keep the previous dynamic metadata. Report this with the host log"
                            );
                        }
                        self.report.invalid += 1;
                    }
                }
                self.report.sample.add(started);
            }
            Ok(None) => {}
            Err(error) => {
                if self.report.readout_errors == 0 {
                    tracing::error!(
                        error = %format!("{error:#}"),
                        "HDR statistics readout failed; frames keep the previous dynamic metadata. Report this with the host log"
                    );
                }
                self.report.readout_errors += 1;
            }
        }
    }
    /// The first unit of an output frame with the current HDR10+ and HDR
    /// Vivid units in it, and on AV1 the frame's RPU OBU after them, or
    /// `None` to send it as it is: without them, when it has no insertion
    /// point, or when AMF produced nothing. Counts what the frame gains for
    /// its cap, an HEVC frame's RPU ([`Dynamic::next_rpu`]) included.
    pub(super) fn first_unit(&mut self, bytes: &[u8]) -> Option<Vec<u8>> {
        let started = Instant::now();
        self.added = 0;
        if bytes.is_empty() {
            return None;
        }
        self.report.frames += 1;
        let codec = self.writer.codec();
        let t35 = if self.wanted.t35 {
            self.writer.units()
        } else {
            &[]
        };
        // AV1 carries the RPU in a metadata OBU before the frame; HEVC's NAL
        // unit ends the access unit (`last_unit`).
        let rpu = if codec == Codec::Av1 {
            std::mem::take(&mut self.rpu)
        } else {
            self.added = self.rpu.len() as u64;
            Vec::new()
        };
        if t35.is_empty() && rpu.is_empty() {
            return None;
        }
        let Some(at) = hdr_dynamic::insert_offset(codec, bytes) else {
            if self.report.no_insertion == 0 {
                tracing::warn!(
                    codec = ?codec,
                    bytes = bytes.len(),
                    dolby_vision = !rpu.is_empty(),
                    "AMF output frame has no place for HDR dynamic metadata; sending it without. Report this with the host log"
                );
            }
            self.report.no_insertion += 1;
            return None;
        };
        let mut spliced = Vec::with_capacity(bytes.len() + t35.len() + rpu.len());
        spliced.extend_from_slice(&bytes[..at]);
        spliced.extend_from_slice(t35);
        spliced.extend_from_slice(&rpu);
        spliced.extend_from_slice(&bytes[at..]);
        self.added += (t35.len() + rpu.len()) as u64;
        self.report.spliced += 1;
        self.report.stand_in += u64::from(!t35.is_empty() && !self.writer.measured());
        self.report.splice.add(started);
        Some(spliced)
    }
    /// The RPU of a frame that starts, for [`Dynamic::first_unit`] to place
    /// and count: none for a frame AMF produced nothing for, or without Dolby
    /// Vision.
    fn next_rpu(&mut self, frame: bool, keyframe: bool) {
        self.rpu.clear();
        let Some(rpus) = self.dovi.as_mut().filter(|_| frame) else {
            return;
        };
        let started = Instant::now();
        let measured = rpus.measured();
        self.rpu = rpus.frame(keyframe).unit();
        self.report.rpus += 1;
        self.report.unmeasured_rpus += u64::from(!measured);
        self.report.rpu.add(started);
    }
    /// A frame's last unit (`out` if it was spliced, else `bytes`) with the
    /// frame's RPU after it, the last NAL unit of the access unit.
    fn last_unit(&mut self, out: Option<Vec<u8>>, bytes: &[u8]) -> Option<Vec<u8>> {
        if self.rpu.is_empty() {
            return out;
        }
        let mut out = out.unwrap_or_else(|| {
            let mut copy = Vec::with_capacity(bytes.len() + self.rpu.len());
            copy.extend_from_slice(bytes);
            copy
        });
        out.append(&mut self.rpu);
        Some(out)
    }
    /// A frame cap that counts the units the current frame gained. Zero (the
    /// driver's unrestricted default) and unknown stay as they are.
    pub(super) fn cap(&self, cap: Option<u64>) -> Option<u64> {
        cap.map(|cap| if cap > 0 { cap + self.added } else { cap })
    }
}

impl Drop for Dynamic {
    fn drop(&mut self) {
        let r = &self.report;
        if r.frames == 0 {
            return;
        }
        tracing::info!(
            codec = ?self.writer.codec(),
            hdr10_plus = self.wanted.t35 && !self.wanted.hlg,
            dolby_vision = self.dovi.is_some(),
            hlg = self.wanted.hlg,
            frames = r.frames,
            spliced = r.spliced,
            stand_in = r.stand_in,
            rpus = r.rpus,
            unmeasured_rpus = r.unmeasured_rpus,
            samples = r.samples,
            scene_cuts = r.scene_cuts,
            invalid = r.invalid,
            readout_errors = r.readout_errors,
            no_insertion = r.no_insertion,
            submit_mean_us = r.submit.mean_us(),
            submit_max_us = r.submit.worst_us(),
            sample_mean_us = r.sample.mean_us(),
            sample_max_us = r.sample.worst_us(),
            splice_mean_us = r.splice.mean_us(),
            splice_max_us = r.splice.worst_us(),
            rpu_mean_us = r.rpu.mean_us(),
            rpu_max_us = r.rpu.worst_us(),
            "HDR dynamic metadata"
        );
    }
}

#[cfg(test)]
mod tests;
