//! What the audio device plays between packets. A lost packet, and a device that asks for more
//! than is queued, are both filled with Opus packet loss concealment, one packet's length at a
//! time, instead of silence. A packet whose time concealment already played is skipped when it
//! arrives, so riding out a late burst never adds latency. Silence is played only before the
//! first packet and after an outage longer than concealment covers.

use crate::media::OpusDecoder;

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

/// What one packet did.
#[derive(Debug, PartialEq, Eq)]
pub enum Fed {
    /// `pcm()` holds `lead` samples per channel of silence, then the packet. `underrun` says the
    /// device ran dry before it.
    Play { lead: usize, underrun: bool },
    /// Dropped to stay in sync: the queue was full, or concealment already played its time.
    Dropped,
    /// The decoder refused it.
    Failed,
}

pub struct Playout {
    decoder: OpusDecoder,
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
}

impl Playout {
    /// `device_frames` is how much the device takes at once (0 when unknown).
    pub fn new(decoder: OpusDecoder, packet_ms: u32, device_frames: usize) -> Self {
        let frame = 48 * packet_ms.max(1) as usize;
        let lead = (48 * LEAD_MS).max(device_frames).min(5760 - frame);
        Self {
            pcm: vec![0.0; 5760 * decoder.channels],
            decoder,
            len: 0,
            frame,
            max_queued: (48 * QUEUE_MS).max(lead + frame) + frame,
            lead,
            owed: 0,
            run: 0,
            primed: false,
            dry: false,
            asks: true,
            concealed: 0,
        }
    }

    pub fn channels(&self) -> usize {
        self.decoder.channels
    }

    /// What the last call put out, interleaved.
    pub fn pcm(&self) -> &[f32] {
        &self.pcm[..self.len * self.decoder.channels]
    }

    /// One packet from the stream, or `None` for a lost one, with `queued` samples per channel
    /// waiting in the device.
    pub fn packet(&mut self, packet: Option<&[u8]>, queued: usize) -> Fed {
        self.len = 0;
        if queued > self.max_queued {
            return Fed::Dropped;
        }
        if self.owed > 0 {
            self.owed -= 1;
            return Fed::Dropped;
        }
        self.dry |= !self.asks && self.primed && queued == 0;
        let lead = if queued == 0 && (!self.primed || self.dry) {
            self.lead
        } else {
            0
        };
        let channels = self.decoder.channels;
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
        let channels = self.decoder.channels;
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
    }

    /// The device never calls `starve`: only lost packets are concealed.
    pub fn without_requests(&mut self) {
        self.asks = false;
    }

    /// Packets concealed since the last call.
    pub fn take_concealed(&mut self) -> u64 {
        std::mem::take(&mut self.concealed)
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
            OpusDecoder::new(2, 1, 1, &[0, 1]).expect("the Opus decoder"),
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
