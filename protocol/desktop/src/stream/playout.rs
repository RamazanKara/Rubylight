//! What the audio device plays between packets. A lost packet, and a device that asks for more
//! than is queued, are both filled with Opus packet loss concealment, one packet's length at a
//! time, instead of silence. A packet whose time concealment already played is skipped when it
//! arrives, so riding out a late burst never adds latency. Bursts that keep coming (packets
//! bunched every 20 ms for a minute) would starve the device on each one, though: a burst within
//! a second of the last keeps its late packets instead, growing the cushion to what the bursts
//! need, and the drift trim takes it back once they stop. Silence is played only before the
//! first packet and after an outage longer than concealment covers. PCM has no concealment: what
//! it lacks is filled with silence on the same terms. A device clock slower than the host's
//! builds up a surplus; it is trimmed a packet at a time, faded, long before the queue's limit.

use super::pipeline::AudioDecoder;

/// Silence led with on an empty queue (the start, or after an outage), in ms, or the device's
/// own buffer when that is longer: enough that the next packet's jitter does not starve the
/// device again. Concealment keeps this cushion from then on.
const LEAD_MS: usize = 10;
/// The longest outage concealed, in ms. Opus fades its concealment out well before this; past it
/// the device plays silence and the next packet starts again with the lead.
const CONCEAL_MS: usize = 100;
/// Most audio queued before packets are dropped, in ms (plus one packet): enough to ride out
/// scheduling jitter, short enough to stay in sync.
const QUEUE_MS: usize = 40;
/// A queue more than a packet above the lead for this long, in ms of packets, is surplus a slow
/// device clock built up (35 ppm measured: about 2 ms a minute), and one packet goes. Steady
/// state never stays there: any packet within a packet of the lead starts the count again.
const TRIM_AFTER_MS: usize = 2000;
/// The fade from a trimmed packet into the next, in ms.
const FADE_MS: usize = 5;
/// A starved device this soon after the last time, in ms of packets, is in a run of late bursts:
/// the packets concealment stood in for are kept, not skipped, so the queue grows by them.
const BURSTS_MS: usize = 1000;

/// What one packet did.
#[derive(Debug, PartialEq, Eq)]
pub enum Fed {
    /// `pcm()` holds `lead` samples per channel of silence, then the packet. `underrun` says the
    /// device ran dry before it.
    Play { lead: usize, underrun: bool },
    /// Dropped to stay in sync: the queue was full, or concealment already played its time.
    Dropped,
    /// Dropped as clock drift surplus; the next packet fades in from it.
    Trimmed,
    /// The decoder refused it.
    Failed,
}

pub struct Playout {
    decoder: AudioDecoder,
    pcm: Vec<f32>,
    /// Samples (per channel) `pcm` holds.
    len: usize,
    /// Samples per channel in one packet.
    frame: usize,
    max_queued: usize,
    /// Samples per channel of silence led with on an empty queue.
    lead: usize,
    /// Packets whose time concealment played; they are skipped when they arrive.
    owed: usize,
    /// Samples concealed since a packet last played.
    run: usize,
    /// A packet played, so there is something to conceal from.
    primed: bool,
    /// The device ran dry with nothing concealed for it.
    dry: bool,
    /// The device asks for what it lacks (`starve`); without that, an empty queue means it ran
    /// dry.
    asks: bool,
    concealed: u64,
    /// Samples per channel of packets seen above the trim line in a row.
    above: usize,
    /// Samples per channel of packets seen since the device last started to starve on a late
    /// burst.
    since_starved: usize,
    /// The device starved twice within `BURSTS_MS`: late packets are kept.
    bursts: bool,
    /// A run of bursts started since the last `take_bursts_started`, for the log.
    bursts_started: bool,
    /// The start of the packet trimmed last (interleaved), which the next one fades in from: it
    /// continues what already played.
    trim_head: Vec<f32>,
}

impl Playout {
    /// `device_frames` is how much the device takes at once (0 when unknown).
    pub fn new(decoder: AudioDecoder, packet_ms: u32, device_frames: usize) -> Self {
        let frame = 48 * packet_ms.max(1) as usize;
        let lead = (48 * LEAD_MS).max(device_frames).min(5760 - frame);
        Self {
            pcm: vec![0.0; 5760 * decoder.channels()],
            decoder,
            len: 0,
            frame,
            // Above the trim line (`drift`) by the device's own take at least: a queue that sits
            // just over the line peaks a device period higher between its requests.
            max_queued: (48 * QUEUE_MS).max(2 * lead + frame) + frame,
            lead,
            owed: 0,
            run: 0,
            primed: false,
            dry: false,
            asks: true,
            concealed: 0,
            above: 0,
            since_starved: usize::MAX,
            bursts: false,
            bursts_started: false,
            trim_head: Vec::new(),
        }
    }

    pub fn channels(&self) -> usize {
        self.decoder.channels()
    }

    /// What the last call put out, interleaved.
    pub fn pcm(&self) -> &[f32] {
        &self.pcm[..self.len * self.decoder.channels()]
    }

    /// One packet from the stream, or `None` for a lost one, with `queued` samples per channel
    /// waiting in the device.
    pub fn packet(&mut self, packet: Option<&[u8]>, queued: usize) -> Fed {
        self.len = 0;
        if queued > self.max_queued {
            return Fed::Dropped;
        }
        self.since_starved = self.since_starved.saturating_add(self.frame);
        if self.owed > 0 && packet.is_none() {
            // The device starved on a loss the receiver waited for, not a late burst: concealed
            // as ever, and not counted towards a run of bursts.
            self.bursts = false;
            self.since_starved = usize::MAX;
        }
        if self.owed > 0 {
            self.owed -= 1;
            if !self.bursts {
                return Fed::Dropped;
            }
        }
        self.dry |= !self.asks && self.primed && queued == 0;
        let lead = if queued == 0 && (!self.primed || self.dry) {
            self.lead
        } else {
            0
        };
        let channels = self.decoder.channels();
        // A lost packet is concealed for exactly its own length: given the whole buffer, Opus
        // would conceal 120 ms.
        let frame = if packet.is_some() {
            5760 - lead
        } else {
            self.frame
        };
        let out = &mut self.pcm[lead * channels..(lead + frame) * channels];
        let samples = self.decoder.decode(packet, out);
        if samples <= 0 {
            return Fed::Failed;
        }
        // Decoded all the same: the decoder goes on from it without a gap.
        if packet.is_some() && self.drift(queued) {
            self.run = 0;
            let head = (samples as usize).min(48 * FADE_MS) * channels;
            self.trim_head.clear();
            self.trim_head.extend_from_slice(&self.pcm[..head]);
            return Fed::Trimmed;
        }
        if lead == 0 && !self.trim_head.is_empty() {
            crossfade(&self.trim_head, &mut self.pcm[..samples as usize * channels], channels);
        }
        self.trim_head.clear();
        self.pcm[..lead * channels].fill(0.0);
        self.len = lead + samples as usize;
        if packet.is_some() {
            self.run = 0;
            self.primed = true;
        } else {
            self.concealed += 1;
        }
        let underrun = std::mem::take(&mut self.dry) && lead > 0;
        Fed::Play { lead, underrun }
    }

    /// Whether this packet is clock drift surplus: the queue stayed more than a packet above the
    /// lead for `TRIM_AFTER_MS` of packets.
    fn drift(&mut self, queued: usize) -> bool {
        if queued <= self.lead + self.frame {
            self.above = 0;
            return false;
        }
        self.above += self.frame;
        if self.above < 48 * TRIM_AFTER_MS {
            return false;
        }
        self.above = 0;
        true
    }

    /// The device needs `needed` samples per channel beyond what is queued. Returns the
    /// concealment for them, in whole packets, or nothing before the first packet and past the
    /// longest outage concealed.
    pub fn starve(&mut self, needed: usize) -> &[f32] {
        self.len = 0;
        if needed == 0 || !self.primed {
            return &[];
        }
        let room = (48 * CONCEAL_MS).saturating_sub(self.run) / self.frame;
        let frames = needed.div_ceil(self.frame).min(room);
        if frames == 0 {
            // Past what concealment rides out: what arrives next is new, not late.
            self.owed = 0;
            self.dry = true;
            return &[];
        }
        if self.run == 0 {
            // A packet played since the last time: this is a new late burst.
            let bursts = self.since_starved < 48 * BURSTS_MS;
            self.bursts_started |= bursts && !self.bursts;
            self.bursts = bursts;
            self.since_starved = 0;
        }
        let channels = self.decoder.channels();
        let samples = self
            .decoder
            .decode(None, &mut self.pcm[..frames * self.frame * channels]);
        if samples <= 0 {
            self.dry = true;
            return &[];
        }
        let samples = samples as usize;
        self.len = samples;
        self.run += samples;
        let packets = samples / self.frame;
        self.owed += packets;
        self.concealed += packets as u64;
        self.pcm()
    }

    /// Playback moved elsewhere (or stopped): nothing is concealed until a packet plays again.
    pub fn stop(&mut self) {
        self.primed = false;
        self.dry = false;
        self.owed = 0;
        self.run = 0;
        self.above = 0;
        self.since_starved = usize::MAX;
        self.bursts = false;
        self.trim_head.clear();
    }

    /// The device never calls `starve`: only lost packets are concealed.
    pub fn without_requests(&mut self) {
        self.asks = false;
    }

    /// Whether a run of late bursts started since the last call.
    pub fn take_bursts_started(&mut self) -> bool {
        std::mem::take(&mut self.bursts_started)
    }

    /// Packets concealed since the last call.
    pub fn take_concealed(&mut self) -> u64 {
        std::mem::take(&mut self.concealed)
    }
}

/// Fades `pcm` in from `head` over the frames both have, as Android's trim and stall fade-in do.
fn crossfade(head: &[f32], pcm: &mut [f32], channels: usize) {
    let frames = head.len().min(pcm.len()) / channels;
    for (i, (sample, from)) in pcm.iter_mut().zip(head).take(frames * channels).enumerate() {
        let step = (i / channels) as f32 / frames as f32;
        *sample = *sample * step + from * (1.0 - step);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::c_void;

    extern "C" {
        fn opus_encoder_create(
            rate: i32,
            channels: i32,
            application: i32,
            error: *mut i32,
        ) -> *mut c_void;
        fn opus_encoder_destroy(encoder: *mut c_void);
        fn opus_encode_float(
            encoder: *mut c_void,
            pcm: *const f32,
            samples: i32,
            data: *mut u8,
            length: i32,
        ) -> i32;
    }

    /// 5 ms stereo packets of a 440 Hz tone, as the host sends them (CELT, restricted low delay).
    fn packets(count: usize) -> Vec<Vec<u8>> {
        let mut error = 0;
        let encoder = unsafe { opus_encoder_create(48000, 2, 2051, &mut error) };
        assert!(!encoder.is_null(), "the Opus encoder ({error})");
        let packets = (0..count)
            .map(|n| {
                let pcm: Vec<f32> = (0..240)
                    .flat_map(|i| {
                        let t = (n * 240 + i) as f32 / 48000.0;
                        let v = (t * 440.0 * std::f32::consts::TAU).sin() * 0.5;
                        [v, v]
                    })
                    .collect();
                let mut packet = vec![0u8; 1275];
                let length = unsafe {
                    opus_encode_float(encoder, pcm.as_ptr(), 240, packet.as_mut_ptr(), 1275)
                };
                assert!(length > 0);
                packet.truncate(length as usize);
                packet
            })
            .collect();
        unsafe { opus_encoder_destroy(encoder) };
        packets
    }

    fn playout(device_frames: usize) -> Playout {
        Playout::new(
            AudioDecoder::Opus(crate::media::OpusDecoder::new(2, 1, 1, &[0, 1]).expect("the Opus decoder")),
            5,
            device_frames,
        )
    }

    #[derive(Default)]
    struct Run {
        /// Samples the device had to play as silence after the first packet.
        silent: usize,
        underruns: usize,
        dropped: usize,
        concealed: u64,
        /// The most queued over the last 100 ms, in samples.
        late_max: usize,
    }

    /// Plays 2 s of packets sent every 5 ms to a device asking for `request` samples whenever it
    /// has played that many. `arrive(n)` is when packet n arrives (ms) and whether it is lost;
    /// the receiver hands packets over in order, so one held back holds the rest too.
    fn run(request: usize, arrive: impl Fn(usize) -> (u64, bool)) -> Run {
        let source = packets(400);
        let mut playout = playout(request);
        let mut result = Run::default();
        let mut queued = 0usize;
        let mut started = false;
        let mut released = 0u64;
        let deliveries: Vec<(u64, Option<&[u8]>)> = source
            .iter()
            .enumerate()
            .map(|(n, packet)| {
                let (at, lost) = arrive(n);
                released = released.max(at);
                (released, (!lost).then_some(packet.as_slice()))
            })
            .collect();
        let mut next = deliveries.iter().peekable();
        // Samples the device has asked for since it started, kept in step with real time.
        let mut pulled = 0usize;
        let mut start = 0u64;
        for ms in 0..2100u64 {
            while let Some(&&(at, packet)) = next.peek() {
                if at > ms {
                    break;
                }
                next.next();
                match playout.packet(packet, queued) {
                    Fed::Play { underrun, .. } => {
                        queued += playout.pcm().len() / 2;
                        if !started {
                            start = ms;
                        }
                        started = true;
                        result.underruns += usize::from(underrun);
                    }
                    Fed::Dropped => result.dropped += 1,
                    Fed::Trimmed => panic!("no clock drift here, nothing to trim"),
                    Fed::Failed => panic!("a packet failed to decode"),
                }
            }
            if started && ms < 2000 && pulled <= (ms - start) as usize * 48 {
                pulled += request;
                if queued < request {
                    queued += playout.starve(request - queued).len() / 2;
                }
                let played = queued.min(request);
                result.silent += request - played;
                queued -= played;
            }
            if (1900..2000).contains(&ms) {
                result.late_max = result.late_max.max(queued);
            }
        }
        result.concealed = playout.take_concealed();
        result
    }

    fn on_time(n: usize) -> (u64, bool) {
        (5 * n as u64 + 1, false)
    }

    #[test]
    fn a_lost_packet_is_concealed_for_its_own_length_only() {
        let source = packets(3);
        let mut playout = playout(480);
        assert!(matches!(
            playout.packet(Some(&source[0]), 0),
            Fed::Play {
                lead: 480,
                underrun: false
            }
        ));
        assert_eq!(playout.pcm().len(), (480 + 240) * 2);
        assert_eq!(
            playout.packet(None, 720),
            Fed::Play {
                lead: 0,
                underrun: false
            }
        );
        assert_eq!(
            playout.pcm().len(),
            240 * 2,
            "one packet of concealment, not 120 ms"
        );
        assert!(
            playout.pcm().iter().any(|&v| v != 0.0),
            "concealment continues the tone"
        );
        assert_eq!(playout.take_concealed(), 1);
    }

    #[test]
    fn pcm_plays_its_packets_and_fills_what_is_missing_with_silence() {
        let mut playout = Playout::new(AudioDecoder::Pcm { channels: 2 }, 5, 0);
        // 240 stereo frames at half scale.
        let packet: Vec<u8> = (0..480).flat_map(|_| 16384i16.to_le_bytes()).collect();
        assert!(matches!(
            playout.packet(Some(&packet), 0),
            Fed::Play { lead: 480, .. }
        ));
        assert_eq!(playout.pcm().len(), (480 + 240) * 2);
        assert!(playout.pcm()[480 * 2..].iter().all(|&v| v == 0.5));
        assert_eq!(playout.packet(None, 720), Fed::Play { lead: 0, underrun: false });
        assert_eq!(playout.pcm().len(), 240 * 2, "one packet of silence");
        assert!(playout.pcm().iter().all(|&v| v == 0.0));
        // The device asks for 500 samples: whole packets of silence cover it, as Opus
        // concealment does, and the packets they stand for are skipped when they arrive.
        let filled = playout.starve(500).len();
        assert_eq!(filled, 3 * 240 * 2);
        assert_eq!(playout.packet(Some(&packet), 0), Fed::Dropped);
    }

    #[test]
    fn loss_patterns_are_concealed_without_silence_or_added_latency() {
        for request in [480, 1024] {
            let clean = run(request, on_time);
            assert_eq!(
                (clean.silent, clean.underruns, clean.concealed),
                (0, 0, 0),
                "{request}"
            );
        }
        // The receiver reports a loss after waiting four packets and 10 ms for its block, and
        // holds everything after it until then.
        type Pattern = (&'static str, fn(usize) -> bool);
        let patterns: [Pattern; 3] = [
            ("single", |n| n == 100),
            ("burst", |n| (100..104).contains(&n)),
            ("periodic", |n| n >= 40 && n % 20 == 0),
        ];
        for (name, lost) in patterns {
            for request in [480, 1024] {
                let lossy = run(request, |n| {
                    if lost(n) {
                        (5 * n as u64 + 30, true)
                    } else {
                        on_time(n)
                    }
                });
                assert_eq!(
                    lossy.silent, 0,
                    "{name}, {request}: the device never plays silence"
                );
                assert_eq!(lossy.underruns, 0, "{name}, {request}");
                assert!(
                    lossy.concealed > 0,
                    "{name}, {request}: concealment stood in"
                );
                let baseline = run(request, on_time);
                assert!(
                    lossy.late_max <= baseline.late_max + 240,
                    "{name}, {request}: {} queued after the losses, {} without",
                    lossy.late_max,
                    baseline.late_max
                );
            }
        }
    }

    #[test]
    fn late_packets_are_concealed_and_the_queue_shrinks_back() {
        // Every packet from 100 on arrives 40 ms late at once, none lost.
        let late = run(480, |n| {
            if (100..108).contains(&n) {
                (540, false)
            } else {
                on_time(n)
            }
        });
        assert_eq!((late.silent, late.underruns), (0, 0));
        assert!(
            late.concealed >= 6 && late.dropped > 0,
            "{} concealed, {} skipped",
            late.concealed,
            late.dropped
        );
        assert!(late.late_max <= run(480, on_time).late_max + 240);
    }

    #[test]
    fn an_outage_longer_than_concealment_falls_back_to_silence_and_starts_again() {
        let source = packets(2);
        let mut playout = playout(480);
        playout.packet(Some(&source[0]), 0);
        let mut concealed = 0;
        for _ in 0..30 {
            concealed += playout.starve(480).len() / 2;
        }
        assert_eq!(concealed, 4800, "100 ms concealed, then silence");
        assert_eq!(
            playout.packet(Some(&source[1]), 0),
            Fed::Play {
                lead: 480,
                underrun: true
            }
        );
        assert_eq!(
            playout.pcm().len(),
            (480 + 240) * 2,
            "a new start is played, not skipped"
        );
    }

    #[test]
    fn nothing_is_concealed_before_the_first_packet_or_after_a_stop() {
        let source = packets(1);
        let mut playout = playout(480);
        assert!(playout.starve(480).is_empty());
        playout.packet(Some(&source[0]), 0);
        playout.stop();
        assert!(playout.starve(480).is_empty());
        assert_eq!(playout.take_concealed(), 0);
    }

    #[test]
    fn without_device_requests_an_empty_queue_is_an_underrun_led_with_silence() {
        let source = packets(2);
        let mut playout = playout(480);
        playout.without_requests();
        playout.packet(Some(&source[0]), 0);
        assert_eq!(
            playout.packet(Some(&source[1]), 0),
            Fed::Play {
                lead: 480,
                underrun: true
            }
        );
    }

    /// 5 ms stereo PCM packets of a 440 Hz tone: no Opus state, so long runs stay quick.
    fn pcm_tone(n: usize) -> Vec<u8> {
        (0..240)
            .flat_map(|i| {
                let t = (n * 240 + i) as f32 / 48000.0;
                let v = ((t * 440.0 * std::f32::consts::TAU).sin() * 16384.0) as i16;
                [v.to_le_bytes(), v.to_le_bytes()].concat()
            })
            .collect()
    }

    #[derive(Default)]
    struct Drift {
        trims: usize,
        /// Dropped at the queue's limit, unfaded: what the trim keeps from happening.
        full: usize,
        /// The least queued over the last quarter of the run, in samples: the latency that stays.
        late_min: usize,
        /// When the device ran short and concealment stood in, in ms.
        starved: Vec<u64>,
    }

    /// `seconds` of packets in pairs every 10 ms, as hosts send them, to a device taking `request`
    /// samples at a time on its own clock, slower by `ppm` (negative: faster). `held(pair)` holds
    /// a pair back in ms; the receiver hands packets over in order. `trim` off plays as before.
    fn drift(seconds: u64, ppm: f64, request: usize, held: impl Fn(usize) -> u64, trim: bool) -> Drift {
        let source: Vec<Vec<u8>> = (0..200).map(pcm_tone).collect();
        let mut playout = Playout::new(AudioDecoder::Pcm { channels: 2 }, 5, request);
        let mut result = Drift { late_min: usize::MAX, ..Drift::default() };
        let (end, period) = (seconds * 1_000_000, request as f64 * 1000.0 / 48.0 * (1.0 + ppm / 1e6));
        let mut network = std::collections::VecDeque::new();
        let (mut queued, mut started, mut pair, mut next_pair, mut released, mut sent) = (0, false, 0, 0, 0, 0);
        let mut pulls = 0f64;
        let mut at = 0;
        while at < end {
            while next_pair <= at {
                released = released.max(next_pair + held(pair) * 1000);
                network.extend([released, released]);
                pair += 1;
                next_pair += 10_000;
            }
            while network.front().is_some_and(|&t| t <= at) {
                network.pop_front();
                if !trim {
                    playout.above = 0;
                }
                match playout.packet(Some(&source[sent % source.len()]), queued) {
                    Fed::Play { .. } => {
                        queued += playout.pcm().len() / 2;
                        started = true;
                    }
                    Fed::Trimmed => result.trims += 1,
                    Fed::Dropped => result.full += usize::from(queued > playout.max_queued),
                    Fed::Failed => panic!("a packet failed to decode"),
                }
                sent += 1;
            }
            if started && at as f64 >= pulls * period {
                pulls += 1.0;
                if queued < request {
                    let filled = playout.starve(request - queued).len() / 2;
                    if filled > 0 {
                        result.starved.push(at / 1000);
                    }
                    queued += filled;
                }
                queued -= queued.min(request);
            }
            if at >= end / 4 * 3 {
                result.late_min = result.late_min.min(queued);
            }
            at += 100;
        }
        result
    }

    #[test]
    fn a_slow_device_clock_is_trimmed_a_packet_at_a_time_before_the_queue_limit() {
        for request in [480, 1024] {
            // 35 ppm measured: 42 ms of surplus in 20 minutes, which ran into the queue's limit.
            let untrimmed = drift(1200, 35.0, request, |_| 0, false);
            assert!(untrimmed.full > 0, "{request}: untrimmed, the limit drops");
            // The lead led with on an empty queue; trims start a packet above it.
            let lead = request.max(480);
            for (seconds, ppm) in [(1200, 35.0), (300, 300.0)] {
                let run = drift(seconds, ppm, request, |_| 0, true);
                assert_eq!(run.full, 0, "{request}, {ppm} ppm");
                // A trim a packet of surplus, less what the queue held below the trim line first
                // (up to the lead and a packet, depending on the device's phase).
                let surplus = (seconds as f64 * ppm / 1e6 * 1000.0 / 5.0) as usize;
                assert!(
                    (surplus.saturating_sub(lead / 240 + 2)..=surplus + 1).contains(&run.trims),
                    "{request}, {ppm} ppm: {} trims for {surplus} packets of drift",
                    run.trims
                );
                // What stays is at most two packets over the lead (`full` above: the peaks never
                // reach the limit).
                assert!(run.late_min <= lead + 2 * 240, "{request}, {ppm} ppm: at least {} queued", run.late_min);
            }
        }
    }

    #[test]
    fn steady_state_fast_devices_and_jitter_bursts_are_never_trimmed() {
        // A 30 ms hold every 3 s, and a 12 ms one every 0.5 s.
        let jitter = |pair: usize| match pair {
            p if p % 300 == 7 => 30,
            p if p % 50 == 3 => 12,
            _ => 0,
        };
        for request in [480, 1024] {
            for ppm in [0.0, -35.0, -300.0] {
                for bursts in [false, true] {
                    let run = drift(300, ppm, request, |p| if bursts { jitter(p) } else { 0 }, true);
                    assert_eq!(run.trims, 0, "{request}, {ppm} ppm, bursts {bursts}");
                    if ppm == 0.0 {
                        assert_eq!(run.full, 0, "{request}, bursts {bursts}");
                    }
                }
            }
            // Jitter on top of drift still trims only the drift. A queue that dips below the trim
            // line every 0.5 s is never trimmed, so the drift case has only the 3 s holds: with a
            // 1024-sample device period on top, the 12 ms ones leave the limit to drop as before.
            let run = drift(1200, 35.0, request, |p| if p % 300 == 7 { 30 } else { 0 }, true);
            assert_eq!(run.full, 0, "{request}");
            assert!((4..=10).contains(&run.trims), "{request}: {} trims", run.trims);
        }
    }

    #[test]
    fn a_minute_of_late_bursts_grows_the_cushion_once_and_trims_it_back_after() {
        // #68: for a minute the packets arrived late p50 10 p99 16 max 18 ms, as four at a time
        // every 20 ms do (here from 10 s to 70 s); each burst used to starve the device, its packets were skipped, and
        // the next burst starved it again for the whole minute.
        let held = |pair: usize| match pair {
            p if (1000..7000).contains(&p) && p % 2 == 0 => 10 + if p % 26 == 0 { 3 } else { 0 },
            _ => 0,
        };
        for request in [480, 1024] {
            let run = drift(100, 0.0, request, held, true);
            let during = run.starved.iter().filter(|&&ms| (11_000..70_000).contains(&ms)).count();
            assert!(during <= 2, "{request}: starved {during} times once the bursts settled");
            assert!(run.starved.iter().filter(|&&ms| ms >= 70_000).count() == 0, "{request}: clean after");
            assert_eq!(run.full, 0, "{request}");
            // The cushion the bursts needed is trimmed away once they stop.
            let lead = request.max(480);
            assert!(run.late_min <= lead + 2 * 240, "{request}: {} queued at the end", run.late_min);
        }
    }

    #[test]
    fn a_trimmed_packet_fades_into_the_next_without_a_click() {
        let mut playout = Playout::new(AudioDecoder::Pcm { channels: 2 }, 5, 480);
        let mut out: Vec<f32> = Vec::new();
        let mut trimmed = None;
        // The queue sits 15 ms above the lead: the 400th packet, 2 s on, is trimmed.
        for n in 0..402 {
            match playout.packet(Some(&pcm_tone(n)), 480 + 720) {
                Fed::Play { lead, .. } => out.extend(playout.pcm()[lead * 2..].iter().step_by(2)),
                Fed::Trimmed => trimmed = Some(n),
                other => panic!("{other:?}"),
            }
        }
        assert_eq!(trimmed, Some(399));
        // The tone moves at most this much a sample; the splice without the fade jumps ~0.3.
        let natural = 0.5 * std::f32::consts::TAU * 440.0 / 48000.0;
        let worst = out.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
        assert!(worst <= natural * 1.5, "a step of {worst}, the tone's own {natural}");
    }

    #[test]
    fn a_full_queue_drops_packets() {
        let source = packets(1);
        let mut playout = playout(480);
        assert_eq!(
            playout.packet(Some(&source[0]), 48 * QUEUE_MS + 241),
            Fed::Dropped
        );
    }
}
