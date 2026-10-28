//! Dynamic HDR metadata in AMF's output: HDR10+ and HDR Vivid
//! (`butterpollo_core::hdr_dynamic`; HDR Vivid alone on HLG streams) before
//! each frame's first slice, and a Dolby Vision profile 8.1 or 8.4 RPU
//! (`butterpollo_core::dovi`) after its last. The
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
    /// A Dolby Vision RPU on every frame ([`dovi::negotiated`]), profile 8.4
    /// on an HLG stream, else 8.1.
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
        let (writer, profile) = if self.hlg {
            (writer.with_hlg(), dovi::Profile::P84)
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
/// HDR Vivid has no AV1 carriage and HDR10+ describes PQ only), and HEVC
/// streams whose client picked Dolby Vision 8.1 or 8.4. `None` leaves every
/// output byte as AMF wrote it. Ten-bit SDR never gets any, even if a path
/// marked it SDR without clearing `hdr`.
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
/// tile, the Dolby Vision RPU after its last (a whole frame gets both), and
/// every unit of that frame gets a cap that counts what the frame gained.
/// A `keyframe` refreshes the RPU's scene. The bytes are `None` to send them
/// as they are: without dynamic metadata, a middle unit, or nothing added.
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
        out = dynamic.first_unit(bytes);
        dynamic.next_rpu(!bytes.is_empty(), keyframe);
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
    /// The current frame's RPU NAL unit, until its last unit takes it.
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
    /// Vivid units in it, or `None` to send it as it is: without them, when
    /// it has no insertion point, or when AMF produced nothing.
    pub(super) fn first_unit(&mut self, bytes: &[u8]) -> Option<Vec<u8>> {
        let started = Instant::now();
        self.added = 0;
        if bytes.is_empty() {
            return None;
        }
        self.report.frames += 1;
        let units = self.writer.units();
        if !self.wanted.t35 {
            return None;
        }
        let Some(spliced) = hdr_dynamic::spliced(self.writer.codec(), bytes, units) else {
            if self.report.no_insertion == 0 {
                tracing::warn!(
                    codec = ?self.writer.codec(),
                    bytes = bytes.len(),
                    "AMF output frame has no place for HDR dynamic metadata; sending it without. Report this with the host log"
                );
            }
            self.report.no_insertion += 1;
            return None;
        };
        self.added = units.len() as u64;
        self.report.spliced += 1;
        self.report.stand_in += u64::from(!self.writer.measured());
        self.report.splice.add(started);
        Some(spliced)
    }
    /// The RPU of a frame that starts, counted in its cap: none for a frame
    /// AMF produced nothing for, or without Dolby Vision.
    fn next_rpu(&mut self, frame: bool, keyframe: bool) {
        self.rpu.clear();
        let Some(rpus) = self.dovi.as_mut().filter(|_| frame) else {
            return;
        };
        let started = Instant::now();
        let measured = rpus.measured();
        self.rpu = rpus.frame(keyframe).nal();
        self.added += self.rpu.len() as u64;
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
mod tests {
    use super::*;
    use butterpollo_core::hdr_dynamic::{FrameStats, frame_units, insert_offset};

    fn nal(kind: u8, body: &[u8]) -> Vec<u8> {
        let mut unit = vec![0, 0, 0, 1, kind << 1, 0x01];
        unit.extend(body);
        unit
    }
    fn keyframe() -> Vec<u8> {
        [
            nal(32, &[0x0C]),
            nal(33, &[0x01]),
            nal(34, &[0xC1]),
            nal(19, &[0xAF, 0x12]),
        ]
        .concat()
    }
    /// A readout as the GPU pass leaves it: ten cells in each bin from `low`
    /// to `high`.
    fn readout(conversion: u64, low: usize, high: usize) -> HdrStatsReadout {
        let mut histogram = vec![0; crate::compute::HDR_STATS_BINS];
        histogram[low..=high].fill(10);
        let edge = |bin: usize| bin as f32 / crate::compute::HDR_STATS_BINS as f32;
        HdrStatsReadout {
            conversion,
            histogram,
            min_pq: edge(low),
            max_pq: edge(high),
        }
    }

    #[test]
    fn only_hdr_hevc_and_av1_with_the_setting_on_get_dynamic_metadata() {
        let on = butterpollo_core::config::Config::default();
        let off =
            butterpollo_core::config::Config::parse("hdr_dynamic_metadata = false\n").unwrap();
        let config = |codec, hdr| butterpollo_core::rtsp::Negotiated {
            codec,
            hdr,
            ..Default::default()
        };
        assert_eq!(
            wanted(&config(1, true), &on),
            Some(Wanted::t35(Codec::Hevc))
        );
        assert_eq!(wanted(&config(2, true), &on), Some(Wanted::t35(Codec::Av1)));
        assert_eq!(wanted(&config(1, false), &on), None, "SDR");
        assert_eq!(wanted(&config(0, true), &on), None, "H.264");
        assert_eq!(wanted(&config(3, true), &on), None, "PyroWave");
        for codec in [1, 2] {
            assert_eq!(wanted(&config(codec, true), &off), None, "setting off");
        }
    }

    #[test]
    fn ten_bit_sdr_never_gets_dynamic_metadata() {
        // Hardware lane 4a: a 10-bit SDR launch streamed as HDR switched HDR10+ on.
        let on = butterpollo_core::config::Config::default();
        for codec in [1, 2] {
            // RTSP reads dynamicRangeMode=1 as HDR; hdrMode=0 at launch makes it SDR.
            let mut stream = butterpollo_core::rtsp::Negotiated {
                codec,
                hdr: true,
                ..Default::default()
            };
            assert!(wanted(&stream, &on).is_some(), "an HDR launch keeps it");
            assert!(butterpollo_core::stream_policy::infer_sdr_10bit(
                &mut stream,
                true
            ));
            assert_eq!(wanted(&stream, &on), None, "codec {codec}");
            // Marked SDR but with `hdr` left set: still none.
            stream.hdr = true;
            assert_eq!(wanted(&stream, &on), None, "codec {codec}, hdr left set");
        }
    }

    #[test]
    fn frames_before_the_first_sample_carry_the_static_stand_in() {
        let mut dynamic = Dynamic::new(Codec::Hevc);
        let frame = keyframe();
        let at = insert_offset(Codec::Hevc, &frame).unwrap();
        let with = |metadata: &butterpollo_core::hdr::Metadata| {
            let units = frame_units(Codec::Hevc, &FrameStats::from_static(metadata)).unwrap();
            (
                [&frame[..at], &units, &frame[at..]].concat(),
                units.len() as u64,
            )
        };
        // Frame 0 comes out before its statistics: it carries the stand-in.
        let (expected, added) = with(&Default::default());
        assert_eq!(dynamic.first_unit(&frame), Some(expected));
        assert_eq!(dynamic.cap(Some(5000)), Some(5000 + added));
        // The stream's static metadata rebuilds it.
        let mut metadata = butterpollo_core::hdr::Metadata::display(600., 0.01, 300.);
        metadata.max_cll = 450;
        dynamic.set_metadata(metadata);
        // Nothing finished yet: still the stand-in.
        dynamic.take(Ok(None), Instant::now());
        assert_eq!(dynamic.first_unit(&frame), Some(with(&metadata).0));
        assert_eq!(dynamic.sampled(), 0);
        assert_eq!((dynamic.report.spliced, dynamic.report.stand_in), (2, 2));
        // An invalid first sample keeps it too.
        let mut empty = readout(4, 0, 0);
        empty.histogram.fill(0);
        dynamic.take(Ok(Some(empty)), Instant::now());
        assert_eq!(dynamic.first_unit(&frame), Some(with(&metadata).0));
        // A sample replaces it.
        let sample = readout(5, 200, 600);
        let units = frame_units(Codec::Hevc, &sample.frame_stats().unwrap()).unwrap();
        dynamic.take(Ok(Some(sample)), Instant::now());
        assert_eq!(
            dynamic.first_unit(&frame),
            Some([&frame[..at], &units, &frame[at..]].concat())
        );
        assert_eq!((dynamic.report.spliced, dynamic.report.stand_in), (4, 3));
    }

    #[test]
    fn a_sample_is_spliced_into_every_frame_until_the_next() {
        let mut dynamic = Dynamic::new(Codec::Hevc);
        let first = readout(7, 200, 600);
        let stats = first.frame_stats().unwrap();
        dynamic.take(Ok(Some(first)), Instant::now());
        assert_eq!(dynamic.sampled(), 7, "the next readout must be newer");
        let units = frame_units(Codec::Hevc, &stats).unwrap();
        let frame = keyframe();
        let at = insert_offset(Codec::Hevc, &frame).unwrap();
        let expected = [&frame[..at], &units, &frame[at..]].concat();
        for _ in 0..3 {
            assert_eq!(dynamic.first_unit(&frame).as_ref(), Some(&expected));
            assert_eq!(dynamic.cap(Some(5000)), Some(5000 + units.len() as u64));
            assert_eq!(dynamic.cap(Some(0)), Some(0), "unrestricted stays so");
            assert_eq!(dynamic.cap(None), None);
        }
        // A frame without an insertion point goes out as it is, its cap too.
        assert_eq!(dynamic.first_unit(&nal(32, &[0x0C])), None);
        assert_eq!(dynamic.cap(Some(5000)), Some(5000));
        assert_eq!(dynamic.report.no_insertion, 1);
        // An empty output is no frame to report.
        assert_eq!(dynamic.first_unit(&[]), None);
        assert_eq!(dynamic.cap(Some(5000)), Some(5000));
        assert_eq!(dynamic.report.no_insertion, 1);
        // A cut to a dark scene is taken as it is.
        let dark = readout(9, 10, 80);
        let dark_stats = dark.frame_stats().unwrap();
        dynamic.take(Ok(Some(dark)), Instant::now());
        let units = frame_units(Codec::Hevc, &dark_stats).unwrap();
        assert_eq!(
            dynamic.first_unit(&frame),
            Some([&frame[..at], &units, &frame[at..]].concat())
        );
        assert_eq!(dynamic.report.scene_cuts, 2);
    }

    #[test]
    fn a_failed_readout_keeps_the_previous_units() {
        let mut dynamic = Dynamic::new(Codec::Av1);
        dynamic.take(Ok(Some(readout(3, 100, 900))), Instant::now());
        let frame = [
            vec![0x12, 0x00],
            vec![0x0A, 0x01, 0x00],
            vec![0x32, 0x01, 0x10],
        ]
        .concat();
        let before = dynamic.first_unit(&frame).unwrap();
        dynamic.take(Err(anyhow::anyhow!("inconsistent")), Instant::now());
        let mut empty = readout(4, 0, 0);
        empty.histogram.fill(0);
        dynamic.take(Ok(Some(empty)), Instant::now());
        assert_eq!(dynamic.first_unit(&frame), Some(before));
        assert_eq!(
            (dynamic.report.readout_errors, dynamic.report.invalid),
            (1, 1)
        );
        assert_eq!(dynamic.sampled(), 4, "an invalid sample is not read again");
    }

    /// A statistics pass that fails to submit on call `fail_at`.
    #[derive(Default)]
    struct FakeStats {
        enables: u32,
        enable_fails: bool,
        submits: u32,
        fail_at: Option<u32>,
        stopped: bool,
        readout: Option<HdrStatsReadout>,
    }
    impl Stats for FakeStats {
        fn enable(&mut self) -> anyhow::Result<()> {
            self.enables += 1;
            if self.enable_fails {
                anyhow::bail!("root signature rejected");
            }
            Ok(())
        }
        fn submit(&mut self) -> anyhow::Result<()> {
            self.submits += 1;
            if self.fail_at == Some(self.submits) {
                anyhow::bail!("command allocator reset failed");
            }
            Ok(())
        }
        fn latest(&self, newer_than: u64) -> anyhow::Result<Option<HdrStatsReadout>> {
            Ok(self
                .readout
                .clone()
                .filter(|readout| readout.conversion > newer_than))
        }
        fn stop(&mut self) {
            self.stopped = true;
        }
    }

    #[test]
    fn a_failed_statistics_pass_turns_dynamic_metadata_off_loudly() {
        let warnings = Warnings::default();
        let mut stats = FakeStats {
            fail_at: Some(2),
            readout: Some(readout(5, 200, 600)),
            ..Default::default()
        };
        let mut slot = Some(Box::new(Dynamic::new(Codec::Hevc)));
        sample(&mut slot, Some(&mut stats), &warnings);
        assert_eq!(slot.as_deref().map(Dynamic::sampled), Some(5));
        assert!(warnings.snapshot().is_empty());
        // The second pass fails: no error reaches the encode, which keeps
        // its frames; the stream says why it lost its dynamic metadata.
        sample(&mut slot, Some(&mut stats), &warnings);
        assert!(slot.is_none(), "dynamic metadata stayed on");
        assert!(stats.stopped, "the converter kept queueing passes");
        let shown = warnings.snapshot();
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].code, "hdr_dynamic_metadata");
        assert!(
            shown[0]
                .message
                .contains("GPU statistics pass failed (command allocator reset failed)"),
            "{}",
            shown[0].message
        );
        // Off for the rest of the stream: no more passes.
        sample(&mut slot, Some(&mut stats), &warnings);
        assert_eq!(stats.submits, 2);
    }

    #[test]
    fn a_stream_without_the_statistics_pass_says_why_it_carries_static_hdr10_only() {
        let shown = |stats: Option<&mut FakeStats>| {
            let warnings = Warnings::default();
            let dynamic = start(Wanted::t35(Codec::Hevc), false, stats, &warnings);
            assert!(dynamic.is_none());
            let shown = warnings.snapshot();
            assert_eq!(shown.len(), 1);
            assert_eq!(shown[0].code, "hdr_dynamic_metadata");
            assert!(
                shown[0]
                    .message
                    .starts_with("HDR10+ and HDR Vivid are off for this stream: ")
                    && shown[0]
                        .message
                        .ends_with(". It carries static HDR10 metadata only"),
                "{}",
                shown[0].message
            );
            shown[0].message.clone()
        };
        assert!(shown(None::<&mut FakeStats>).contains("no GPU statistics pass"));
        let mut failing = FakeStats {
            enable_fails: true,
            ..Default::default()
        };
        assert!(
            shown(Some(&mut failing))
                .contains("statistics pass did not start (root signature rejected)")
        );
        // With the pass, dynamic metadata is on and nothing is shown.
        let mut stats = FakeStats::default();
        for codec in [Codec::Av1, Codec::Hevc] {
            let warnings = Warnings::default();
            let dynamic = start(Wanted::t35(codec), false, Some(&mut stats), &warnings);
            assert!(dynamic.is_some_and(|dynamic| dynamic.measuring));
            assert!(warnings.snapshot().is_empty());
        }
        assert_eq!(stats.enables, 2);
    }

    #[test]
    fn packed_444_runs_no_statistics_pass_for_dynamic_metadata() {
        // Its bitstream says SDR and the device unpacks it in its own
        // shader, with the control channel's HDR10 metadata: no decoder
        // would read HDR10+ or HDR Vivid, so the GPU never measures for it.
        let warnings = Warnings::default();
        let mut stats = FakeStats::default();
        for codec in [Codec::Hevc, Codec::Av1] {
            assert!(start(Wanted::t35(codec), true, Some(&mut stats), &warnings).is_none());
        }
        assert_eq!(stats.enables, 0);
        assert!(warnings.snapshot().is_empty());
    }

    #[test]
    fn packed_444_streams_ask_with_their_own_config_not_the_coded_one() {
        // amf.rs codes a packed stream as SDR; `wanted` gets the stream's
        // config, so an HDR packed stream reaches `start` and says why it
        // carries no dynamic metadata.
        let mut config = butterpollo_core::rtsp::Negotiated {
            codec: 1,
            hdr: true,
            chroma_packing: 1,
            ..Default::default()
        };
        let on = butterpollo_core::config::Config::default();
        assert_eq!(wanted(&config, &on), Some(Wanted::t35(Codec::Hevc)));
        config.hdr = false;
        assert_eq!(wanted(&config, &on), None);
    }

    fn dolby_vision(t35: bool) -> Wanted {
        Wanted {
            codec: Codec::Hevc,
            t35,
            dolby_vision: true,
            hlg: false,
        }
    }

    /// The RPU a frame ends with: its NAL unit from the last start code on.
    fn ending_rpu(bytes: &[u8]) -> &[u8] {
        let at = bytes
            .windows(6)
            .rposition(|window| window[..4] == [0, 0, 0, 1] && window[4] == 62 << 1)
            .expect("no RPU");
        &bytes[at..]
    }

    #[test]
    fn dolby_vision_is_wanted_for_hevc_streams_that_picked_it() {
        let on = butterpollo_core::config::Config::default();
        let off =
            butterpollo_core::config::Config::parse("hdr_dynamic_metadata = false\n").unwrap();
        let config = |codec, hdr_format| butterpollo_core::rtsp::Negotiated {
            codec,
            hdr: true,
            hdr_format,
            ..Default::default()
        };
        assert_eq!(wanted(&config(1, 4), &on), Some(dolby_vision(true)));
        // Its RPUs do not need HDR10+.
        assert_eq!(wanted(&config(1, 4), &off), Some(dolby_vision(false)));
        // A stock pick (none), PQ, and AV1 carry none.
        assert_eq!(wanted(&config(1, 0), &off), None);
        assert_eq!(wanted(&config(1, 1), &off), None);
        assert_eq!(wanted(&config(2, 4), &off), None);
    }

    #[test]
    fn hlg_streams_want_hdr_vivid_on_hevc_and_dolby_vision_8_4() {
        let on = butterpollo_core::config::Config::default();
        let off =
            butterpollo_core::config::Config::parse("hdr_dynamic_metadata = false\n").unwrap();
        let config = |codec, hdr_format| butterpollo_core::rtsp::Negotiated {
            codec,
            hdr: true,
            hdr_format,
            ..Default::default()
        };
        let hlg = |t35, dolby_vision| Wanted {
            codec: Codec::Hevc,
            t35,
            dolby_vision,
            hlg: true,
        };
        assert_eq!(wanted(&config(1, 2), &on), Some(hlg(true, false)));
        assert_eq!(wanted(&config(1, 2), &off), None);
        assert_eq!(wanted(&config(1, 8), &on), Some(hlg(true, true)));
        assert_eq!(wanted(&config(1, 8), &off), Some(hlg(false, true)));
        // AV1 HLG: HDR Vivid has no AV1 carriage and HDR10+ is PQ only.
        assert_eq!(wanted(&config(2, 2), &on), None);
        // The writers follow: HLG units and 8.4 RPUs.
        let (writer, rpus) = hlg(true, true).writers(1000);
        assert!(writer.hlg());
        assert_eq!(rpus.unwrap().profile(), dovi::Profile::P84);
        let (writer, rpus) = dolby_vision(true).writers(1000);
        assert!(!writer.hlg());
        assert_eq!(rpus.unwrap().profile(), dovi::Profile::P81);
    }

    #[test]
    fn a_dolby_vision_8_4_frame_carries_hdr_vivid_for_hlg_and_an_8_4_rpu() {
        let warnings = Warnings::default();
        let mut stats = FakeStats {
            readout: Some(readout(6, 100, 700)),
            ..Default::default()
        };
        let wanted = Wanted {
            hlg: true,
            ..dolby_vision(true)
        };
        let mut slot = start(wanted, false, Some(&mut stats), &warnings);
        sample(&mut slot, Some(&mut stats), &warnings);
        let dynamic = slot.as_deref_mut().unwrap();
        let measured = readout(6, 100, 700).frame_stats().unwrap();
        let units = hdr_dynamic::hlg_frame_units(Codec::Hevc, &measured).unwrap();
        let mut rpus = dovi::Writer::new(1000).with_profile(dovi::Profile::P84);
        rpus.sample(
            &hdr_dynamic::StatsSmoother::default()
                .update(&measured)
                .unwrap(),
        );
        let rpu = rpus.frame(false).nal();
        let frame = keyframe();
        let at = insert_offset(Codec::Hevc, &frame).unwrap();
        let (bytes, cap) = output(Some(dynamic), Unit::Whole, &frame, Some(5000), false);
        assert_eq!(
            bytes,
            Some([&frame[..at], &units, &frame[at..], &rpu].concat())
        );
        assert_eq!(cap, Some(5000 + (units.len() + rpu.len()) as u64));
        assert!(warnings.snapshot().is_empty());
    }

    #[test]
    fn every_dolby_vision_frame_ends_with_an_rpu_counted_in_its_cap() {
        let mut dynamic = Dynamic::with(dolby_vision(false), false);
        let mut metadata = butterpollo_core::hdr::Metadata::display(1000., 0.005, 400.);
        metadata.max_fall = 150;
        dynamic.set_metadata(metadata);
        dynamic.set_target(600);
        let mut rpus = dovi::Writer::new(600);
        rpus.set_metadata(metadata);
        // A keyframe, whole: the RPU after its last slice.
        let frame = keyframe();
        let expected = rpus.frame(true).nal();
        let (bytes, cap) = output(Some(&mut dynamic), Unit::Whole, &frame, Some(5000), true);
        assert_eq!(bytes, Some([frame.clone(), expected.clone()].concat()));
        assert_eq!(cap, Some(5000 + expected.len() as u64));
        // A streamed frame in slices: the first and middle units go out as
        // they are, the last takes the RPU, and all count it.
        let expected = rpus.frame(false).nal();
        let grown = Some(5000 + expected.len() as u64);
        let mut sequencer = super::super::units::Sequencer::default();
        let [streamed, ..] = frames();
        let sent = deliver(
            &mut sequencer,
            Some(&mut dynamic),
            1,
            streamed.0,
            &streamed.1,
        );
        assert_eq!(
            sent,
            [
                (
                    Unit::Slice { index: 0 },
                    [aud(), nal(1, &[0xA1])].concat(),
                    grown
                ),
                (Unit::Slice { index: 1 }, nal(1, &[0xA2]), grown),
                (
                    Unit::Last { index: 2 },
                    [nal(1, &[0xA3]), expected].concat(),
                    grown
                ),
            ]
        );
        // Static metadata only: the stream's light levels as L1.
        let parsed = ending_rpu(&sent[2].1);
        assert_eq!(parsed, &rpus.frame(false).nal()[..]);
        assert_eq!(dynamic.report.unmeasured_rpus, 2);
        // An empty output gets no RPU.
        assert_eq!(
            output(Some(&mut dynamic), Unit::Whole, &[], Some(5000), false),
            (None, Some(5000))
        );
    }

    #[test]
    fn dolby_vision_takes_each_sample_and_hdr10_plus_goes_first() {
        let warnings = Warnings::default();
        let mut stats = FakeStats {
            readout: Some(readout(6, 100, 700)),
            ..Default::default()
        };
        let mut slot = start(dolby_vision(true), false, Some(&mut stats), &warnings);
        sample(&mut slot, Some(&mut stats), &warnings);
        let dynamic = slot.as_deref_mut().unwrap();
        let measured = readout(6, 100, 700).frame_stats().unwrap();
        let units = frame_units(Codec::Hevc, &measured).unwrap();
        let mut rpus = dovi::Writer::new(1000);
        rpus.sample(
            &hdr_dynamic::StatsSmoother::default()
                .update(&measured)
                .unwrap(),
        );
        let rpu = rpus.frame(false).nal();
        let frame = keyframe();
        let at = insert_offset(Codec::Hevc, &frame).unwrap();
        let (bytes, cap) = output(Some(dynamic), Unit::Whole, &frame, Some(5000), false);
        assert_eq!(
            bytes,
            Some([&frame[..at], &units, &frame[at..], &rpu].concat())
        );
        assert_eq!(cap, Some(5000 + (units.len() + rpu.len()) as u64));
        assert_eq!(dynamic.report.unmeasured_rpus, 0);
        assert!(warnings.snapshot().is_empty());
    }

    #[test]
    fn dolby_vision_without_statistics_goes_on_from_the_static_metadata_and_says_so() {
        // An input without the statistics pass: HDR10+ off, RPUs from the
        // static values.
        let warnings = Warnings::default();
        let mut slot = start(dolby_vision(true), false, None::<&mut FakeStats>, &warnings);
        let dynamic = slot.as_deref_mut().expect("Dolby Vision went off");
        assert!(!dynamic.measuring && !dynamic.wanted.t35);
        let mut codes: Vec<_> = warnings.snapshot().into_iter().map(|w| w.code).collect();
        codes.sort();
        assert_eq!(codes, ["hdr_dolby_vision", "hdr_dynamic_metadata"]);
        let frame = keyframe();
        let (bytes, _) = output(Some(dynamic), Unit::Whole, &frame, None, false);
        assert_eq!(&bytes.unwrap()[..frame.len()], &frame[..], "HDR10+ went in");

        // A pass that fails mid-stream: the RPUs go back to the static values.
        let warnings = Warnings::default();
        let mut stats = FakeStats {
            fail_at: Some(2),
            readout: Some(readout(5, 200, 600)),
            ..Default::default()
        };
        let mut slot = start(dolby_vision(true), false, Some(&mut stats), &warnings);
        sample(&mut slot, Some(&mut stats), &warnings);
        assert!(slot.as_ref().unwrap().dovi.as_ref().unwrap().measured());
        sample(&mut slot, Some(&mut stats), &warnings);
        let dynamic = slot.as_deref_mut().expect("Dolby Vision went off");
        assert!(stats.stopped && !dynamic.measuring && !dynamic.wanted.t35);
        assert!(!dynamic.dovi.as_ref().unwrap().measured());
        let shown = warnings.snapshot();
        assert_eq!(shown.len(), 2);
        assert!(
            shown
                .iter()
                .all(|w| w.message.contains("the rest of this stream")),
            "{shown:?}"
        );
        let (bytes, _) = output(Some(dynamic), Unit::Whole, &frame, None, false);
        let bytes = bytes.unwrap();
        assert_eq!(&bytes[..frame.len()], &frame[..]);
        let mut rpus = dovi::Writer::new(1000);
        assert_eq!(ending_rpu(&bytes), &rpus.frame(false).nal()[..]);
        sample(&mut slot, Some(&mut stats), &warnings);
        assert_eq!(stats.submits, 2, "a pass after the failure");
    }

    #[test]
    fn packed_444_never_carries_dolby_vision() {
        let mut stats = FakeStats::default();
        for t35 in [false, true] {
            let warnings = Warnings::default();
            let slot = start(dolby_vision(t35), true, Some(&mut stats), &warnings);
            assert!(slot.is_none());
            let shown = warnings.snapshot();
            assert_eq!(shown.len(), 1);
            assert_eq!(shown[0].code, "hdr_dolby_vision");
            assert!(shown[0].message.contains("packed 4:4:4"));
        }
        assert_eq!(stats.enables, 0);
    }

    /// What the sender gets for one frame AMF returned as `units`
    /// (`OutputBufferType`, bytes), as `Encoder::poll_once` hands them on.
    fn deliver(
        sequencer: &mut super::super::units::Sequencer,
        mut dynamic: Option<&mut Dynamic>,
        pts: i64,
        meta: super::super::units::Meta,
        units: &[(i64, Vec<u8>)],
    ) -> Vec<(Unit, Vec<u8>, Option<u64>)> {
        use super::super::units::Step;
        let mut sent = vec![];
        for (kind, bytes) in units {
            let (unit, bytes) = match sequencer.push(pts, Some(*kind), bytes, || meta).step {
                Step::Hold => continue,
                Step::Unit(unit, bytes) => (unit, bytes),
                Step::Whole(bytes) => (Unit::Whole, bytes),
            };
            let (spliced, cap) = output(dynamic.as_deref_mut(), unit, &bytes, Some(5000), meta.idr);
            sent.push((unit, spliced.unwrap_or(bytes), cap));
        }
        sent
    }
    const STREAMED: super::super::units::Meta = super::super::units::Meta {
        streamed: true,
        idr: false,
    };
    const KEYFRAME: super::super::units::Meta = super::super::units::Meta {
        streamed: false,
        idr: true,
    };
    fn aud() -> Vec<u8> {
        nal(35, &[0x50])
    }
    /// A frame's metadata and its `(pts, bytes)` slices.
    type Frame = (super::super::units::Meta, Vec<(i64, Vec<u8>)>);
    /// A streamed frame in three slices, the first behind an AUD; a
    /// one-slice frame; and a keyframe in three slices, joined.
    fn frames() -> [Frame; 3] {
        [
            (
                STREAMED,
                vec![
                    (1, [aud(), nal(1, &[0xA1])].concat()),
                    (1, nal(1, &[0xA2])),
                    (2, nal(1, &[0xA3])),
                ],
            ),
            (STREAMED, vec![(2, [aud(), nal(1, &[0xB1])].concat())]),
            (
                KEYFRAME,
                vec![
                    (
                        1,
                        [
                            aud(),
                            nal(32, &[0x0C]),
                            nal(33, &[0x01]),
                            nal(34, &[0xC1]),
                            nal(19, &[0xC2]),
                        ]
                        .concat(),
                    ),
                    (1, nal(19, &[0xC3])),
                    (2, nal(19, &[0xC4])),
                ],
            ),
        ]
    }

    #[test]
    fn slice_output_carries_dynamic_metadata_on_each_frames_first_unit() {
        let mut dynamic = Dynamic::new(Codec::Hevc);
        let sample = readout(2, 150, 650);
        let units = frame_units(Codec::Hevc, &sample.frame_stats().unwrap()).unwrap();
        dynamic.take(Ok(Some(sample)), Instant::now());
        let grown = Some(5000 + units.len() as u64);
        let mut sequencer = super::super::units::Sequencer::default();
        let [streamed, single, keyframe] = frames();
        // Slice 0 alone gets the units, after its AUD; the frame's every
        // unit gets the cap it gained.
        assert_eq!(
            deliver(
                &mut sequencer,
                Some(&mut dynamic),
                1,
                streamed.0,
                &streamed.1
            ),
            [
                (
                    Unit::Slice { index: 0 },
                    [aud(), units.clone(), nal(1, &[0xA1])].concat(),
                    grown
                ),
                (Unit::Slice { index: 1 }, nal(1, &[0xA2]), grown),
                (Unit::Last { index: 2 }, nal(1, &[0xA3]), grown),
            ]
        );
        assert_eq!(
            deliver(&mut sequencer, Some(&mut dynamic), 2, single.0, &single.1),
            [(
                Unit::Last { index: 0 },
                [aud(), units.clone(), nal(1, &[0xB1])].concat(),
                grown
            )]
        );
        // A keyframe is joined: the units go after its AUD and parameter
        // sets, before its first IDR slice.
        assert_eq!(
            deliver(
                &mut sequencer,
                Some(&mut dynamic),
                3,
                keyframe.0,
                &keyframe.1
            ),
            [(
                Unit::Whole,
                [
                    aud(),
                    nal(32, &[0x0C]),
                    nal(33, &[0x01]),
                    nal(34, &[0xC1]),
                    units.clone(),
                    nal(19, &[0xC2]),
                    nal(19, &[0xC3]),
                    nal(19, &[0xC4]),
                ]
                .concat(),
                grown
            )]
        );
        // A first slice with no place for the units: none of its frame's
        // units count any.
        let bare = [(1, aud()), (2, nal(1, &[0xD1]))];
        assert_eq!(
            deliver(&mut sequencer, Some(&mut dynamic), 4, STREAMED, &bare),
            [
                (Unit::Slice { index: 0 }, aud(), Some(5000)),
                (Unit::Last { index: 1 }, nal(1, &[0xD1]), Some(5000)),
            ]
        );
    }

    #[test]
    fn without_dynamic_metadata_every_output_byte_is_amfs() {
        let off =
            butterpollo_core::config::Config::parse("hdr_dynamic_metadata = false\n").unwrap();
        let config = butterpollo_core::rtsp::Negotiated {
            codec: 1,
            hdr: true,
            ..Default::default()
        };
        assert_eq!(wanted(&config, &off), None, "the setting is off");
        // Off: what AMF wrote, unspliced.
        let mut sequencer = super::super::units::Sequencer::default();
        for (pts, (meta, units)) in frames().into_iter().enumerate() {
            let sent = deliver(&mut sequencer, None, pts as i64, meta, &units);
            let input: Vec<u8> = units.iter().flat_map(|(_, bytes)| bytes.clone()).collect();
            let bytes: Vec<u8> = sent
                .iter()
                .flat_map(|(_, bytes, _)| bytes.clone())
                .collect();
            assert_eq!(bytes, input);
            assert!(sent.iter().all(|&(_, _, cap)| cap == Some(5000)));
        }
        assert_eq!(
            output(None, Unit::Whole, &keyframe(), None, true),
            (None, None)
        );
    }

    #[test]
    fn the_receivers_display_peak_leaves_the_units() {
        let mut dynamic = Dynamic::new(Codec::Hevc);
        let sample = readout(1, 300, 700);
        let stats: FrameStats = sample.frame_stats().unwrap();
        dynamic.take(Ok(Some(sample)), Instant::now());
        dynamic.set_target(2600);
        assert_eq!(dynamic.display_peak, 2600);
        let frame = keyframe();
        let at = insert_offset(Codec::Hevc, &frame).unwrap();
        let units = frame_units(Codec::Hevc, &stats).unwrap();
        assert_eq!(
            dynamic.first_unit(&frame),
            Some([&frame[..at], &units, &frame[at..]].concat())
        );
    }
}
