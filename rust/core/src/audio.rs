//! Bounded audio buffering, speaker mapping and fractional resampling.
use anyhow::{Result, bail};
use rubylight_protocol::audio_ext;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// The layouts and bitrates advertised by the retained Moonlight protocol.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpusLayout {
    pub channels: usize,
    pub streams: i32,
    pub coupled: i32,
    pub mapping: Vec<u8>,
    pub bitrate: i32,
}
impl OpusLayout {
    pub fn select(channels: usize, quality: bool, custom: Option<&str>) -> Result<Self> {
        let (streams, coupled, mapping, bitrate) = match (channels, quality) {
            (2, false) => (1, 1, vec![0, 1], 96000),
            (2, true) => (1, 1, vec![0, 1], 512000),
            (6, false) => (4, 2, vec![0, 1, 4, 5, 2, 3], 256000),
            (6, true) => (6, 0, vec![0, 1, 4, 5, 2, 3], 1536000),
            (8, false) => (5, 3, vec![0, 1, 4, 5, 6, 7, 2, 3], 450000),
            (8, true) => (8, 0, vec![0, 1, 4, 5, 6, 7, 2, 3], 2048000),
            // 7.1.4, as the host's DESCRIBE offers it (`audio_ext::LAYOUT_714*`).
            (12, quality) => {
                let (layout, bitrate) = if quality {
                    (audio_ext::LAYOUT_714_HQ, audio_ext::BITRATE_714_HQ)
                } else {
                    (audio_ext::LAYOUT_714, audio_ext::BITRATE_714)
                };
                (
                    i32::from(layout.streams),
                    i32::from(layout.coupled),
                    layout.mapping.to_vec(),
                    bitrate as i32,
                )
            }
            _ => bail!("unsupported Opus layout"),
        };
        let mut layout = Self {
            channels,
            streams,
            coupled,
            mapping,
            bitrate,
        };
        if let Some((streams, coupled, mapping)) =
            custom.and_then(|s| Self::parse_custom(channels, s))
        {
            layout.streams = streams;
            layout.coupled = coupled;
            layout.mapping = mapping;
        }
        Ok(layout)
    }
    fn parse_custom(channels: usize, text: &str) -> Option<(i32, i32, Vec<u8>)> {
        let b = text.as_bytes();
        if !matches!(channels, 6 | 8)
            || b.len() != channels + 3
            || !b.iter().all(u8::is_ascii_digit)
        {
            return None;
        }
        let c = usize::from(b[0] - b'0');
        let streams = i32::from(b[1] - b'0');
        let coupled = i32::from(b[2] - b'0');
        let mapping: Vec<_> = b[3..].iter().map(|v| v - b'0').collect();
        if c != channels
            || streams + coupled != channels as i32
            || coupled > streams
            || streams == 0
            || mapping.iter().any(|v| usize::from(*v) >= channels)
        {
            return None;
        }
        Some((streams, coupled, mapping))
    }
    pub fn valid_custom(text: &str) -> bool {
        matches!(text.as_bytes().first(), Some(b'6' | b'8'))
            && Self::parse_custom(usize::from(text.as_bytes()[0] - b'0'), text).is_some()
    }
}

pub struct Resampler {
    queue: VecDeque<f32>,
    channels: usize,
    rate: u32,
    phase: f64,
}
impl Resampler {
    pub fn new(rate: u32, channels: usize) -> Result<Self> {
        if !(8000..=384000).contains(&rate) || !matches!(channels, 2 | 6 | 8 | 12) {
            bail!("unsupported audio rate or layout");
        }
        Ok(Self {
            queue: VecDeque::new(),
            channels,
            rate,
            phase: 0.,
        })
    }
    pub fn reset(&mut self) {
        self.queue.clear();
        self.phase = 0.;
    }
    /// Queues one frame, and returns how many frames of the oldest audio the 100 ms cap
    /// discarded to make room.
    pub fn push(&mut self, frame: &[f32]) -> usize {
        debug_assert_eq!(frame.len(), self.channels);
        self.queue.extend(
            frame
                .iter()
                .map(|v| if v.is_finite() { v.clamp(-1., 1.) } else { 0. }),
        );
        let limit = (self.rate as usize / 10).max(2) * self.channels;
        if self.queue.len() > limit {
            let excess = self.queue.len() - limit;
            self.queue.drain(..excess);
            self.phase = 0.;
            return excess / self.channels;
        }
        0
    }
    /// Drop the oldest queued audio back to `target_ms` once more than
    /// `limit_ms` is queued. A late tick leaves extra audio behind, and
    /// each tick sends one packet, so the delay it added stayed until the
    /// 100 ms bound; clock drift crept up to that bound as well. Returns the
    /// frames dropped, at this resampler's input rate.
    pub fn bound(&mut self, limit_ms: u32, target_ms: u32) -> usize {
        let frames = |ms: u32| (self.rate as usize * ms as usize / 1000).max(1);
        let queued = self.queue.len() / self.channels;
        if queued <= frames(limit_ms) {
            return 0;
        }
        let excess = queued - frames(target_ms).min(queued);
        self.queue.drain(..excess * self.channels);
        self.phase = 0.;
        excess
    }
    /// The frames queued and not yet read, at this resampler's input rate.
    pub fn queued(&self) -> usize {
        self.queue.len() / self.channels
    }
    pub fn read(&mut self, frames: usize) -> Option<Vec<f32>> {
        if frames == 0 {
            return Some(Vec::new());
        }
        let ratio = f64::from(self.rate) / 48000.;
        let last = self.phase + (frames - 1) as f64 * ratio;
        let consumed = self.phase + frames as f64 * ratio;
        let needed = (last.floor() as usize + if last.fract() > 1e-10 { 2 } else { 1 })
            .max(consumed.floor() as usize);
        if self.queue.len() / self.channels < needed {
            return None;
        }
        let mut out = Vec::with_capacity(frames * self.channels);
        for i in 0..frames {
            let pos = self.phase + i as f64 * ratio;
            let lo = pos.floor() as usize;
            let frac = pos.fract() as f32;
            for ch in 0..self.channels {
                let a = self.queue[lo * self.channels + ch];
                let b = if frac > 1e-10 {
                    self.queue[(lo + 1) * self.channels + ch]
                } else {
                    a
                };
                out.push(a + (b - a) * frac);
            }
        }
        let whole = consumed.floor() as usize;
        self.phase = consumed - whole as f64;
        self.queue.drain(..whole * self.channels);
        Some(out)
    }
}
/// Audio the host lost before it reached the network. Windows keeps about
/// 22 ms of loopback audio (1056 frames at 48 kHz, on five endpoints of a
/// test PC including Steam Streaming Speakers) and drops a packet that
/// arrives while that is full, without an error; the next packet then starts
/// further on in the device's stream, skipping what it dropped. A datagram
/// Winsock refuses is dropped too. Neither left a trace in the log, so a
/// stalled sender could not be told from network loss.
///
/// Waiting longer than the buffer alone lost nothing: Windows delivers whole
/// 10 ms packets, so a read 23 to 27 ms after the last still found both in a
/// 22 ms buffer, and an endpoint with nothing playing delivers nothing to
/// lose. Counting those waits filled an idle host's log with warnings. Only
/// a skip counts, and only one a late read explains: away for more than half
/// the buffer, and at least as long as the skip. The late read itself still
/// finds the packets that fit, in order; the skip shows on the next packet,
/// a read or two later (on the host, 40 ms stalls skipped 18-28 ms there), so
/// a late read explains skips for one buffer after it. A skip with no late
/// read before it is the endpoint falling quiet and starting again.
#[derive(Default)]
pub struct HostLoss {
    late_reads: u32,
    lost: Duration,
    longest: Duration,
    buffer: Duration,
    unsent: u64,
    reported: Option<Instant>,
    /// A late read whose skip Windows has not delivered yet: its wait, and
    /// the time since.
    pending: Option<(Duration, Duration)>,
}
#[derive(Debug, PartialEq, Eq)]
pub struct HostLossReport {
    /// Reads that came so late that Windows had dropped audio.
    pub late_reads: u32,
    /// The audio Windows dropped before those reads.
    pub lost: Duration,
    /// The longest of their waits.
    pub longest: Duration,
    pub buffer: Duration,
    /// Audio datagrams Winsock refused.
    pub unsent: u64,
}
impl HostLossReport {
    pub fn message(&self) -> String {
        let ms = |d: Duration| d.as_secs_f64() * 1000.;
        let plural = |n: u64| if n == 1 { "" } else { "s" };
        let mut lost = Vec::new();
        let mut advice = Vec::new();
        if self.late_reads > 0 {
            lost.push(format!(
                "Windows dropped {:.0} ms of captured sound because the audio sender read late {} time{} (longest wait {:.1} ms; Windows keeps {:.1} ms)",
                ms(self.lost),
                self.late_reads,
                plural(u64::from(self.late_reads)),
                ms(self.longest),
                ms(self.buffer)
            ));
            advice.push("A late read means the host's audio thread was held up, by a busy CPU or the host itself, not by video bitrate or GPU load; if it repeats on an idle PC, report it with this log.");
        }
        if self.unsent > 0 {
            lost.push(format!(
                "the network adapter refused {} audio packet{}",
                self.unsent,
                plural(self.unsent)
            ));
            advice.push("Refused packets mean the adapter's send queue was full: lower the video bitrate or check the network adapter and its driver.");
        }
        format!(
            "Audio lost on the host before sending: {}. You may hear short gaps. {}",
            lost.join("; "),
            advice.join(" ")
        )
    }
}
impl HostLoss {
    const EVERY: Duration = Duration::from_secs(5);
    /// A read `waited` after the previous one emptied a capture buffer
    /// holding `buffer`. `delivered` is None when Windows had no audio, else
    /// what it skipped before the audio it delivered. Returns whether a late
    /// read explains the skip and it counted.
    pub fn read(
        &mut self,
        waited: Duration,
        delivered: Option<Duration>,
        buffer: Duration,
    ) -> bool {
        let pending = self.pending.take().and_then(|(wait, since)| {
            let since = since + waited;
            (since <= buffer).then_some((wait, since))
        });
        // An empty buffer did not overflow while the sender waited.
        let Some(skipped) = delivered else {
            self.pending = pending;
            return false;
        };
        let late = waited > buffer / 2;
        let explains = pending
            .map(|(wait, _)| wait)
            .into_iter()
            .chain(late.then_some(waited))
            .max();
        let Some(wait) = explains.filter(|wait| !skipped.is_zero() && skipped <= *wait) else {
            if late {
                self.pending = Some((waited, Duration::ZERO));
            } else {
                self.pending = pending;
            }
            return false;
        };
        self.late_reads += 1;
        self.lost += skipped;
        self.longest = self.longest.max(wait);
        self.buffer = buffer;
        true
    }
    pub fn unsent(&mut self) {
        self.unsent += 1;
    }
    /// What was lost since the last report, at most every five seconds.
    pub fn report(&mut self, now: Instant) -> Option<HostLossReport> {
        if (self.late_reads == 0 && self.unsent == 0)
            || self
                .reported
                .is_some_and(|at| now.saturating_duration_since(at) < Self::EVERY)
        {
            return None;
        }
        let report = HostLossReport {
            late_reads: self.late_reads,
            lost: self.lost,
            longest: self.longest,
            buffer: self.buffer,
            unsent: self.unsent,
        };
        *self = Self {
            reported: Some(now),
            pending: self.pending,
            ..Self::default()
        };
        Some(report)
    }
}
/// The Windows speaker bits of Moonlight's channel order, FL FR FC LFE BL BR SL SR, then
/// 7.1.4's heights TFL TFR TBL TBR (`audio_ext::CHANNEL_MASK_714`).
const MOONLIGHT_SPEAKERS: [u32; 12] = [
    1, 2, 4, 8, 16, 32, 512, 1024, 0x1000, 0x4000, 0x8000, 0x20000,
];
/// The Windows speaker bit of each of an endpoint's channels, 0 for one
/// without a known position; a mask that doesn't match the channel count
/// means the standard order.
pub fn speakers(input_channels: usize, mask: u32) -> Vec<u32> {
    if mask.count_ones() as usize == input_channels {
        (0..32)
            .filter(|i| mask & (1 << i) != 0)
            .map(|i| 1 << i)
            .collect()
    } else {
        let standard = MOONLIGHT_SPEAKERS;
        if input_channels == 1 {
            vec![4]
        } else {
            (0..input_channels)
                .map(|i| standard.get(i).copied().unwrap_or(0))
                .collect()
        }
    }
}
/// Windows speaker mask -> Moonlight's FL, FR, FC, LFE, BL, BR, SL, SR order, and for 7.1.4
/// TFL, TFR, TBL, TBR after it. Each speaker the endpoint has goes to its own channel; a
/// 7.1.4 client of an endpoint without heights gets silent heights rather than an upmix
/// that would invent them. A client with fewer channels than a 7.1.4 endpoint gets each
/// height in the ear-level speaker below it at -3 dB.
pub fn mix_matrix(
    input_channels: usize,
    mask: u32,
    output_channels: usize,
) -> Result<Vec<Vec<f32>>> {
    if input_channels == 0 || input_channels > 32 || !matches!(output_channels, 2 | 6 | 8 | 12) {
        bail!("invalid speaker layout");
    }
    let speakers = speakers(input_channels, mask);
    let mut matrix = vec![vec![0.; input_channels]; output_channels];
    let index = |bit| speakers.iter().position(|v| *v == bit);
    if output_channels == 2 && input_channels > 2 {
        for (ch, side, rear) in [(0, 1, 16), (1, 2, 32)] {
            for (bit, gain) in [
                (side, 1.),
                (4, 0.70710677),
                (rear, 0.70710677),
                (if ch == 0 { 512 } else { 1024 }, 0.70710677),
                (if ch == 0 { 0x1000 } else { 0x4000 }, 0.70710677),
                (if ch == 0 { 0x8000 } else { 0x20000 }, 0.70710677),
            ] {
                if let Some(i) = index(bit) {
                    matrix[ch][i] += gain;
                }
            }
            let sum: f32 = matrix[ch].iter().sum();
            if sum > 1. {
                for v in &mut matrix[ch] {
                    *v /= sum;
                }
            }
        }
    } else if input_channels == 1 && output_channels == 2 {
        matrix[0][0] = 1.;
        matrix[1][0] = 1.;
    } else {
        for (ch, bit) in MOONLIGHT_SPEAKERS
            .into_iter()
            .take(output_channels)
            .enumerate()
        {
            if output_channels == 6 && matches!(bit, 16 | 32) {
                // A 5.1 client has one surround pair. A shared virtual
                // sink may be 7.1 for another client, so fold both its back
                // and side channels into that pair instead of losing sides.
                let side = if bit == 16 { 512 } else { 1024 };
                let sources: Vec<_> = [index(bit), index(side)].into_iter().flatten().collect();
                let gain = 1. / sources.len().max(1) as f32;
                for i in sources {
                    matrix[ch][i] = gain;
                }
            } else if let Some(i) = index(bit) {
                matrix[ch][i] = 1.;
            }
        }
        if output_channels < 12 {
            // A client without heights hears them from the speaker below
            // each at -3 dB, as stereo does, rather than not at all.
            for (height, below) in [(0x1000, 0), (0x4000, 1), (0x8000, 4), (0x20000, 5)] {
                if let Some(i) = index(height) {
                    matrix[below][i] += 0.70710677;
                }
            }
            // One factor for every row, so nothing clips and the bed keeps
            // its balance: the center stays as loud as the fronts.
            let peak = matrix
                .iter()
                .map(|row| row.iter().sum::<f32>())
                .fold(0., f32::max);
            if peak > 1. {
                for v in matrix.iter_mut().flatten() {
                    *v /= peak;
                }
            }
        }
    }
    Ok(matrix)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn opus_layouts_preserve_legacy_surround_quality_and_validate_custom_mapping() {
        for (channels, streams, coupled, bitrate) in
            [(2, 1, 1, 512000), (6, 6, 0, 1536000), (8, 8, 0, 2048000)]
        {
            let layout = OpusLayout::select(channels, true, None).unwrap();
            assert_eq!(
                (layout.streams, layout.coupled, layout.bitrate),
                (streams, coupled, bitrate)
            );
        }
        let layout = OpusLayout::select(6, false, Some("660012345")).unwrap();
        assert_eq!((layout.streams, layout.coupled), (6, 0));
        assert_eq!(layout.mapping, [0, 1, 2, 3, 4, 5]);
        for invalid in [
            "600012345",
            "606012345",
            "642012346",
            "88001234567",
            "64201💥",
        ] {
            assert_eq!(
                OpusLayout::select(6, false, Some(invalid)).unwrap(),
                OpusLayout::select(6, false, None).unwrap()
            );
        }
    }
    /// The decoded channel each input channel takes, by RFC 7845 section 5.1.1: coupled
    /// stream `j` decodes to channels `2j` and `2j + 1`, mono stream `k` to `M + k`.
    fn decoded_channels(layout: &OpusLayout) -> Vec<(usize, Option<usize>)> {
        let coupled = layout.coupled as usize;
        layout
            .mapping
            .iter()
            .map(|&index| {
                let index = usize::from(index);
                if index < 2 * coupled {
                    (index / 2, Some(index % 2))
                } else {
                    (index - coupled, None)
                }
            })
            .collect()
    }
    #[test]
    fn seven_one_four_layouts_are_rfc_7845_mapping_tables_with_paired_speakers() {
        for (quality, streams, coupled, bitrate) in [
            (false, 7, 5, audio_ext::BITRATE_714),
            (true, 12, 0, audio_ext::BITRATE_714_HQ),
        ] {
            let layout = OpusLayout::select(12, quality, None).unwrap();
            assert_eq!(
                (layout.channels, layout.streams, layout.coupled),
                (12, streams, coupled)
            );
            assert_eq!(layout.bitrate, bitrate as i32);
            // Family 255 (RFC 7845 5.1.1.4): N + M decoded channels, each entry below it,
            // and here every decoded channel used once, so nothing is duplicated or lost.
            let mut seen = layout.mapping.clone();
            seen.sort_unstable();
            assert_eq!(seen, (0..12).collect::<Vec<u8>>(), "{quality}");
            // It matches what DESCRIBE offers.
            let offered = if quality {
                audio_ext::LAYOUT_714_HQ
            } else {
                audio_ext::LAYOUT_714
            };
            assert_eq!(layout.mapping, offered.mapping);
            let decoded = decoded_channels(&layout);
            if !quality {
                // The left/right pairs (fronts, backs, sides, top fronts, top backs) share a
                // coupled stream as family 1 couples them; centre and LFE are mono.
                for (left, right) in [(0, 1), (4, 5), (6, 7), (8, 9), (10, 11)] {
                    assert_eq!(decoded[left].0, decoded[right].0, "{left}");
                    assert_eq!(
                        (decoded[left].1, decoded[right].1),
                        (Some(0), Some(1)),
                        "{left}"
                    );
                }
                assert_eq!(decoded[2].1, None);
                assert_eq!(decoded[3].1, None);
            }
        }
        // surroundParams is single digits, so a custom 12-channel layout cannot be named.
        assert!(!OpusLayout::valid_custom("12750110"));
    }
    #[test]
    fn five_one_is_rfc_7845_family_one() {
        // Family 1's 5.1 (RFC 7845 5.1.1.2): 4 streams, 2 coupled, mapping 0 4 1 2 3 5 in
        // Vorbis order L C R RL RR LFE. Moonlight orders FL FR FC LFE BL BR.
        let family_one = [0u8, 4, 1, 2, 3, 5];
        let vorbis_of_moonlight = [0, 2, 1, 5, 3, 4];
        let layout = OpusLayout::select(6, false, None).unwrap();
        assert_eq!((layout.streams, layout.coupled), (4, 2));
        let reordered: Vec<u8> = vorbis_of_moonlight
            .iter()
            .map(|&vorbis| family_one[vorbis])
            .collect();
        assert_eq!(layout.mapping, reordered);
    }
    #[test]
    fn seven_one_four_capture_keeps_every_speaker_and_invents_no_heights() {
        let mask = audio_ext::CHANNEL_MASK_714;
        let matrix = mix_matrix(12, mask, 12).unwrap();
        // The endpoint's channels are in bit order: FL FR FC LFE BL BR SL SR TFL TFR TBL
        // TBR is also Moonlight's 7.1.4 order, so this is the identity.
        for (output, row) in matrix.iter().enumerate() {
            for (input, gain) in row.iter().enumerate() {
                assert_eq!(*gain, if input == output { 1. } else { 0. });
            }
        }
        // A 7.1 endpoint feeds the bed and leaves the heights silent.
        let matrix = mix_matrix(8, 0x63f, 12).unwrap();
        for (output, row) in matrix.iter().enumerate() {
            for (input, gain) in row.iter().enumerate() {
                assert_eq!(*gain, if input == output { 1. } else { 0. }, "{output}");
            }
        }
        assert!(matrix[8..].iter().flatten().all(|v| *v == 0.));
        // Stereo goes to the fronts only.
        let matrix = mix_matrix(2, 3, 12).unwrap();
        assert_eq!((matrix[0][0], matrix[1][1]), (1., 1.));
        assert!(matrix[2..].iter().flatten().all(|v| *v == 0.));
        // 7.1 clients of a 7.1.4 endpoint keep their bed and hear each height
        // from the speaker below it at -3 dB, scaled so nothing clips. One
        // factor scales the whole bed, so the center and the side pair stay as
        // loud as the fronts.
        let matrix = mix_matrix(12, mask, 8).unwrap();
        let bed = 1. / (1. + 0.70710677);
        for (output, row) in matrix.iter().enumerate() {
            let below = [0, 1, 4, 5].iter().position(|&o| o == output);
            assert!((row[output] - bed).abs() < 1e-6, "{output}");
            if let Some(height) = below {
                assert!(
                    (row[8 + height] - 0.70710677 * bed).abs() < 1e-6,
                    "{output}"
                );
            }
            assert!(row.iter().sum::<f32>() <= 1. + 1e-6);
            assert_eq!(
                row.iter().filter(|v| **v != 0.).count(),
                1 + below.map_or(0, |_| 1)
            );
        }
        // 5.1 and stereo clients keep the heights too, front ones in front.
        let matrix = mix_matrix(12, mask, 6).unwrap();
        assert!(matrix[0][8] > 0. && matrix[1][9] > 0. && matrix[4][10] > 0. && matrix[5][11] > 0.);
        let level = |row: &[f32]| row.iter().sum::<f32>();
        for row in &matrix {
            assert!(level(row) <= 1. + 1e-6);
        }
        // The fronts and the center keep their relative level.
        assert!((matrix[0][0] - matrix[2][2]).abs() < 1e-6);
        assert!((level(&matrix[0]) - level(&matrix[4])).abs() < 1e-6);
        let matrix = mix_matrix(12, mask, 2).unwrap();
        assert!(matrix[0][8] > 0. && matrix[0][10] > 0. && matrix[0][9] == 0.);
        assert!(matrix[1][9] > 0. && matrix[1][11] > 0. && matrix[1][8] == 0.);
        for row in &matrix {
            assert!(row.iter().sum::<f32>() <= 1. + 1e-6);
        }
        assert!(Resampler::new(48000, 12).is_ok());
        assert!(Resampler::new(48000, 10).is_err());
    }
    #[test]
    fn fractional_rates_do_not_overrun_and_remain_bounded() {
        for rate in [8000, 44100, 48000, 96000, 192000] {
            let mut r = Resampler::new(rate, 2).unwrap();
            for i in 0..10000 {
                r.push(&[0.25, -0.25]);
                if i % 7 == 0
                    && let Some(b) = r.read(240)
                {
                    assert!(b.chunks(2).all(|f| f == [0.25, -0.25]));
                }
            }
            assert!(r.queue.len() <= rate as usize / 10 * 2);
        }
    }
    #[test]
    fn a_backlog_after_a_late_tick_is_dropped_back_to_two_packets() {
        let mut r = Resampler::new(48000, 2).unwrap();
        for i in 0..48 * 50 {
            r.push(&[i as f32 / 1e6, 0.]);
        }
        // 50 ms queued: within a 60 ms limit nothing is dropped.
        assert_eq!(r.bound(60, 10), 0);
        // 50 ms down to 10 ms drops 40 ms.
        assert_eq!(r.bound(30, 10), 48 * 40);
        assert_eq!(r.queue.len() / 2, 480);
        // The newest audio is kept.
        let last = r.read(480).unwrap();
        assert_eq!(last[last.len() - 2], (48 * 50 - 1) as f32 / 1e6);
    }
    #[test]
    fn the_hundred_millisecond_cap_reports_the_frames_it_drops() {
        for rate in [44100u32, 48000] {
            let mut r = Resampler::new(rate, 2).unwrap();
            let cap = rate as usize / 10;
            let dropped: usize = (0..cap + 300).map(|_| r.push(&[0.1, 0.1])).sum();
            assert_eq!(dropped, 300, "{rate} Hz");
            assert_eq!(r.queue.len() / 2, cap);
        }
    }
    #[test]
    fn host_audio_loss_counts_only_skips_a_late_read_explains() {
        let ms = Duration::from_millis;
        let buffer = ms(22);
        let start = Instant::now();
        let none = Duration::ZERO;
        let mut loss = HostLoss::default();
        // A skip with no late read before it is the endpoint going quiet and
        // starting again.
        assert!(!loss.read(ms(10), Some(ms(10)), buffer));
        assert!(!loss.read(ms(5), Some(ms(480)), buffer));
        // The idle host's case: reads up to 27 ms apart, but Windows skipped
        // nothing because both 10 ms packets still fit, or nothing played.
        for waited in [10, 23, 26, 27] {
            assert!(!loss.read(ms(waited), Some(none), buffer));
            for _ in 0..5 {
                assert!(!loss.read(ms(5), None, buffer));
            }
        }
        assert_eq!(loss.report(start), None);
        // The host's 40 ms stall: the late read finds the packets that fit,
        // and the skip arrives a read or two later.
        assert!(!loss.read(ms(43), Some(none), buffer));
        assert!(!loss.read(ms(5), None, buffer));
        assert!(loss.read(ms(1), Some(ms(18)), buffer));
        // Counted once.
        assert!(!loss.read(ms(5), Some(ms(10)), buffer));
        // A skip on the late read itself counts too, unless it is longer
        // than the wait.
        assert!(loss.read(ms(52), Some(ms(30)), buffer));
        assert!(!loss.read(ms(30), Some(ms(480)), buffer));
        loss.unsent();
        assert_eq!(
            loss.report(start),
            Some(HostLossReport {
                late_reads: 2,
                lost: ms(48),
                longest: ms(52),
                buffer,
                unsent: 1,
            })
        );
        // A late read explains skips for one buffer after it, not later.
        assert!(!loss.read(ms(40), Some(none), buffer));
        assert!(!loss.read(ms(10), Some(none), buffer));
        assert!(!loss.read(ms(10), None, buffer));
        assert!(!loss.read(ms(5), Some(ms(10)), buffer));
        // Further loss waits five seconds and then reports only what is new.
        loss.unsent();
        assert_eq!(loss.report(start + ms(4999)), None);
        loss.unsent();
        assert_eq!(
            loss.report(start + ms(5000)),
            Some(HostLossReport {
                late_reads: 0,
                lost: Duration::ZERO,
                longest: Duration::ZERO,
                buffer: Duration::ZERO,
                unsent: 2,
            })
        );
        assert_eq!(loss.report(start + ms(20000)), None);
    }
    #[test]
    fn host_audio_loss_blames_the_cause_it_saw() {
        let ms = Duration::from_millis;
        let late = HostLossReport {
            late_reads: 1,
            lost: ms(10),
            longest: ms(32),
            buffer: ms(22),
            unsent: 0,
        };
        let text = late.message();
        assert!(text.contains("Windows dropped 10 ms of captured sound because the audio sender read late 1 time (longest wait 32.0 ms; Windows keeps 22.0 ms)"), "{text}");
        assert!(text.contains("not by video bitrate"), "{text}");
        assert!(!text.contains("lower the video bitrate"), "{text}");
        let unsent = HostLossReport {
            late_reads: 0,
            lost: Duration::ZERO,
            longest: Duration::ZERO,
            buffer: Duration::ZERO,
            unsent: 3,
        };
        let text = unsent.message();
        assert!(
            text.contains("the network adapter refused 3 audio packets"),
            "{text}"
        );
        assert!(text.contains("lower the video bitrate"), "{text}");
        assert!(!text.contains("late"), "{text}");
    }
    #[test]
    fn surround_upmix_does_not_duplicate_right_channel() {
        let m = mix_matrix(2, 3, 8).unwrap();
        assert_eq!(m[0], [1., 0.]);
        assert_eq!(m[1], [0., 1.]);
        assert!(m[2..].iter().flatten().all(|v| *v == 0.));
        let m = mix_matrix(6, 0x60f, 6).unwrap();
        assert_eq!(m[4][4], 1.);
        assert_eq!(m[5][5], 1.);
        assert!(
            mix_matrix(6, 0x3f, 2)
                .unwrap()
                .iter()
                .all(|r| r.iter().sum::<f32>() <= 1.00001)
        );
    }
    #[test]
    fn seven_point_one_downmix_preserves_every_channel_for_a_five_point_one_client() {
        let matrix = mix_matrix(8, 0x63f, 6).unwrap();
        // Isolate each input speaker, as a speaker-test tone would. Fronts,
        // center and LFE stay discrete; each back/side pair shares one
        // surround output, without crossing left and right.
        for (source, destination) in [0, 1, 2, 3, 4, 5, 4, 5].into_iter().enumerate() {
            let gain = if source < 4 { 1. } else { 0.5 };
            for (output, row) in matrix.iter().enumerate() {
                assert_eq!(row[source], if output == destination { gain } else { 0. });
            }
        }
        // Correlated full-scale surround channels retain headroom.
        assert!(matrix.iter().all(|row| row.iter().sum::<f32>() == 1.));
        // Native 5.1 endpoints keep unity gain with either speaker mask.
        for mask in [0x3f, 0x60f] {
            let matrix = mix_matrix(6, mask, 6).unwrap();
            for (channel, row) in matrix.iter().enumerate() {
                assert_eq!(row[channel], 1.);
                assert_eq!(row.iter().sum::<f32>(), 1.);
            }
        }
    }
}
