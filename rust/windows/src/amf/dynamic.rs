//! HDR10+ and HDR Vivid dynamic metadata in AMF's output
//! (`butterpollo_core::hdr_dynamic`). The compute conversion's statistics pass
//! (`compute_hdr_stats.rs`) measures each frame once it went to the encoder;
//! the newest finished sample is smoothed into the units every output frame
//! carries from then on. Nothing waits for a sample: a frame that comes out
//! before its own has finished carries the previous one, and frames before the
//! first sample carry none.
use crate::compute::HdrStatsReadout;
use butterpollo_core::hdr_dynamic::{self, Codec, Writer};
use std::time::{Duration, Instant};

/// The codec whose HDR stream gets dynamic metadata: HEVC or AV1, HDR, with
/// `hdr_dynamic_metadata` on. `None` leaves every output byte as AMF wrote it.
pub(super) fn wanted(
    config: &butterpollo_core::rtsp::Negotiated,
    options: &butterpollo_core::config::Config,
) -> Option<Codec> {
    if !config.hdr || !options.hdr_dynamic_metadata() {
        return None;
    }
    Codec::from_video_format(config.codec)
}

/// One encoder's dynamic metadata.
pub(super) struct Dynamic {
    writer: Writer,
    /// The conversion the last sample measured: each is smoothed once.
    sampled: u64,
    /// Bytes the current frame's first unit gained. Its frame cap grows by
    /// as much, so a streamed frame's payload stays within it.
    added: u64,
    report: Report,
}

/// What a stream's dynamic metadata did, logged when its encoder closes.
#[derive(Default)]
struct Report {
    frames: u64,
    spliced: u64,
    samples: u64,
    scene_cuts: u64,
    invalid: u64,
    readout_errors: u64,
    no_insertion: u64,
    /// CPU time of submitting the statistics pass, taking a sample, and
    /// splicing the units into a frame.
    submit: Cost,
    sample: Cost,
    splice: Cost,
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
    /// Until the stream names its target display, the 1000-nit default of
    /// [`hdr_dynamic::target_display_nits`].
    pub(super) fn new(codec: Codec) -> Self {
        Self {
            writer: Writer::new(codec, hdr_dynamic::target_display_nits(None, None)),
            sampled: 0,
            added: 0,
            report: Report::default(),
        }
    }
    /// The conversion the last sample measured, for
    /// [`crate::compute::Converter::latest_hdr_stats`].
    pub(super) fn sampled(&self) -> u64 {
        self.sampled
    }
    /// The target display peak HDR10+ is written for.
    pub(super) fn set_target(&mut self, nits: u16) {
        if self.writer.target_nits() != nits {
            tracing::info!(
                nits,
                previous = self.writer.target_nits(),
                "HDR dynamic metadata target display"
            );
            self.writer.set_target_nits(nits);
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
    /// The first unit of an output frame with the current units in it, or
    /// `None` to send it as it is: before the first sample, when it has no
    /// insertion point, or when AMF produced nothing.
    pub(super) fn first_unit(&mut self, bytes: &[u8]) -> Option<Vec<u8>> {
        let started = Instant::now();
        self.added = 0;
        if bytes.is_empty() {
            return None;
        }
        self.report.frames += 1;
        let units = self.writer.units();
        if units.is_empty() {
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
        self.report.splice.add(started);
        Some(spliced)
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
            target_nits = self.writer.target_nits(),
            frames = r.frames,
            spliced = r.spliced,
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
        assert_eq!(wanted(&config(1, true), &on), Some(Codec::Hevc));
        assert_eq!(wanted(&config(2, true), &on), Some(Codec::Av1));
        assert_eq!(wanted(&config(1, false), &on), None, "SDR");
        assert_eq!(wanted(&config(0, true), &on), None, "H.264");
        assert_eq!(wanted(&config(3, true), &on), None, "PyroWave");
        for codec in [1, 2] {
            assert_eq!(wanted(&config(codec, true), &off), None, "setting off");
        }
    }

    #[test]
    fn frames_before_the_first_sample_go_out_unchanged() {
        let mut dynamic = Dynamic::new(Codec::Hevc);
        assert_eq!(dynamic.first_unit(&keyframe()), None);
        assert_eq!(dynamic.cap(Some(5000)), Some(5000));
        // Nothing finished yet: still nothing to send.
        dynamic.take(Ok(None), Instant::now());
        assert_eq!(dynamic.first_unit(&keyframe()), None);
        assert_eq!(dynamic.sampled(), 0);
    }

    #[test]
    fn a_sample_is_spliced_into_every_frame_until_the_next() {
        let mut dynamic = Dynamic::new(Codec::Hevc);
        dynamic.set_target(600);
        let first = readout(7, 200, 600);
        let stats = first.frame_stats().unwrap();
        dynamic.take(Ok(Some(first)), Instant::now());
        assert_eq!(dynamic.sampled(), 7, "the next readout must be newer");
        let units = frame_units(Codec::Hevc, &stats, 600).unwrap();
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
        let units = frame_units(Codec::Hevc, &dark_stats, 600).unwrap();
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

    #[test]
    fn a_new_target_rewrites_the_current_units() {
        let mut dynamic = Dynamic::new(Codec::Hevc);
        let sample = readout(1, 300, 700);
        let stats: FrameStats = sample.frame_stats().unwrap();
        dynamic.take(Ok(Some(sample)), Instant::now());
        dynamic.set_target(2600);
        let frame = keyframe();
        let at = insert_offset(Codec::Hevc, &frame).unwrap();
        let units = frame_units(Codec::Hevc, &stats, 2600).unwrap();
        assert_eq!(
            dynamic.first_unit(&frame),
            Some([&frame[..at], &units, &frame[at..]].concat())
        );
    }
}
