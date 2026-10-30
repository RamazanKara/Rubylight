//! Microphone audio a client sends to the host, as Apollo's microphone
//! passthrough sends it: ClassicOldSong/moonlight-common-c
//! (`MicrophoneStream.c`) with Apollo pull request #1428. A client that
//! speaks it to Apollo works here unchanged.
//!
//! The host offers the stream in its `DESCRIBE` answer (an `m=audio` line
//! with mono Opus at 48 kHz) and asks for it to be encrypted. A client that
//! wants it sets up `streamid=mic`, learns the port from `Transport`, and
//! sends one datagram per Opus packet: a 12-byte little-endian header
//! (flags, type 0x61, sequence, a millisecond timestamp, magic 0x12345678)
//! followed by the packet, AES-128-CBC under the launch's remote input key.
//! The IV is the big-endian sum of the launch's key ID and the sequence
//! number, then twelve zero bytes, the same IV the host's own audio packets
//! use. Nothing goes back to the client.
//!
//! The client pads twice: `PltEncryptMessage` first pads the packet with
//! PKCS#7 up to the next block (nothing when it is already whole blocks,
//! one packet in sixteen), then OpenSSL or mbed TLS adds a full PKCS#7
//! block of its own. Both layers come off here; the inner one only when it
//! is valid, so a client that pads once works too. An unpadded packet whose
//! last bytes happen to look like padding is cut short, about once in 4000
//! packets, which Opus treats as a damaged frame.
//!
//! The host decodes packets as they arrive and does not wait to reorder
//! them: on a LAN that wait would cost more latency than the rare
//! reordering it saves. A packet behind the last one decoded is dropped,
//! and a short gap is covered by Opus loss concealment and the in-band FEC
//! of the packet after it.
use crate::crypto;
use anyhow::{Result, bail};
use std::collections::VecDeque;

/// `x-ss-general.encryption*` bit for microphone packets (`SS_ENC_MICROPHONE`).
pub const ENCRYPTION: u32 = 0x08;
pub const PACKET_TYPE_OPUS: u8 = 0x61;
pub const MAGIC: u32 = 0x1234_5678;
pub const HEADER: usize = 12;
/// The port is the base port plus this, after video (9), control (10) and
/// audio (11), as in Apollo.
pub const PORT_OFFSET: u16 = 12;
/// Samples per second; the stream is mono.
pub const RATE: usize = 48_000;

/// The `DESCRIBE` lines that offer the stream. Clients look for the
/// `rtpmap` line to decide whether the host takes a microphone.
pub fn sdp(port: u16) -> String {
    format!(
        "m=audio {port} RTP/AVP 96\r\na=rtpmap:96 opus/48000/1\r\na=fmtp:96 minptime=10;useinbandfec=1\r\n"
    )
}

#[derive(Debug, PartialEq, Eq)]
pub struct Packet<'a> {
    pub sequence: u16,
    /// The client's clock in milliseconds; only for logs.
    pub timestamp: u32,
    pub payload: &'a [u8],
}
/// A microphone datagram's header and payload, or `None` for anything else.
pub fn parse(datagram: &[u8]) -> Option<Packet<'_>> {
    if datagram.len() <= HEADER
        || datagram[1] != PACKET_TYPE_OPUS
        || u32::from_le_bytes(datagram[8..12].try_into().unwrap()) != MAGIC
    {
        return None;
    }
    Some(Packet {
        sequence: u16::from_le_bytes([datagram[2], datagram[3]]),
        timestamp: u32::from_le_bytes(datagram[4..8].try_into().unwrap()),
        payload: &datagram[HEADER..],
    })
}
fn iv(key_id: u32, sequence: u16) -> [u8; 16] {
    let mut iv = [0; 16];
    iv[..4].copy_from_slice(&key_id.wrapping_add(u32::from(sequence)).to_be_bytes());
    iv
}
/// The Opus packet inside an encrypted payload.
pub fn open(key: &[u8; 16], key_id: u32, packet: &Packet) -> Result<Vec<u8>> {
    if packet.payload.is_empty() {
        bail!("empty microphone payload");
    }
    let mut opus = crypto::cbc_open(key, &iv(key_id, packet.sequence), packet.payload)?;
    if let Some(&last) = opus.last()
        && opus.len().is_multiple_of(16)
    {
        let pad = usize::from(last);
        if (1..16).contains(&pad) && opus[opus.len() - pad..].iter().all(|&b| b == last) {
            opus.truncate(opus.len() - pad);
        }
    }
    if opus.is_empty() {
        bail!("empty microphone packet");
    }
    Ok(opus)
}
/// An encrypted datagram as a client sends it, padding included, for tests
/// and probes.
pub fn seal(key: &[u8; 16], key_id: u32, sequence: u16, timestamp: u32, opus: &[u8]) -> Vec<u8> {
    let mut datagram = vec![0, PACKET_TYPE_OPUS];
    datagram.extend_from_slice(&sequence.to_le_bytes());
    datagram.extend_from_slice(&timestamp.to_le_bytes());
    datagram.extend_from_slice(&MAGIC.to_le_bytes());
    let mut padded = opus.to_vec();
    let pad = (16 - opus.len() % 16) % 16;
    padded.resize(opus.len() + pad, pad as u8);
    datagram.extend_from_slice(&crypto::cbc_seal(key, &iv(key_id, sequence), &padded));
    datagram
}

/// How to decode a packet that just arrived.
#[derive(Debug, PartialEq, Eq)]
pub enum Arrival {
    /// The packet that was due.
    Next,
    /// `missing` packets before it never came: conceal all but the last,
    /// recover the last from this packet's in-band FEC, then decode it.
    AfterLoss { missing: u16 },
    /// A gap too long to conceal, or a client that started counting again:
    /// decode it as the start of new speech.
    Restart,
    /// Already decoded, or behind the last one decoded: drop it.
    Late,
}
/// Longest gap covered by concealment, in packets (100 ms of 20 ms packets).
/// Longer gaps are a pause, and concealing them would only add a smear.
const MAX_CONCEALED: u16 = 5;
/// Late packets in a row that mean the client's count started again.
const LATE_RESTART: u32 = 16;
#[derive(Default)]
pub struct Sequencer {
    next: Option<u16>,
    late_run: u32,
}
impl Sequencer {
    pub fn arrive(&mut self, sequence: u16) -> Arrival {
        let Some(next) = self.next else {
            self.next = Some(sequence.wrapping_add(1));
            return Arrival::Restart;
        };
        let ahead = sequence.wrapping_sub(next);
        if ahead >= 0x8000 {
            self.late_run += 1;
            if self.late_run < LATE_RESTART {
                return Arrival::Late;
            }
        }
        self.late_run = 0;
        self.next = Some(sequence.wrapping_add(1));
        match ahead {
            0 => Arrival::Next,
            1..=MAX_CONCEALED => Arrival::AfterLoss { missing: ahead },
            _ => Arrival::Restart,
        }
    }
}

/// Samples in `ms` milliseconds.
const fn ms(ms: usize) -> usize {
    RATE * ms / 1000
}
/// Played audio counts its time in `WINDOW` slices; the last `WINDOWS` of
/// them (two seconds) are what the queue learns the client's arrival from.
const WINDOW: usize = ms(100);
const WINDOWS: usize = 20;
/// The most the queue holds for a bursty client.
const CAP: usize = ms(150);
/// Audio the device and queue never fell below for the last `SETTLED`
/// windows, past what the next wake-up needs, is standing delay (from a
/// client clock that runs fast, an underrun, or a burst client that turned
/// steady), unless packets came late enough lately to need it (the gap less
/// a packet) or it is within `STANDING`. `SLICE` of it goes, at most once a
/// `WINDOW`, crossfaded over `FADE`; a learnt gap that falls also loses
/// `SLICE` a window, so its excess bleeds off the same way. Half a second
/// and 5 ms react before a client 1% fast reaches the 60 ms bound.
const SETTLED: usize = 5;
const STANDING: usize = ms(5);
const SLICE: usize = RATE * 25 / 10_000;
const FADE: usize = RATE * 25 / 10_000;
/// A dry spell longer than this is the client pausing (it stops sending to
/// mute), not an underrun.
const PAUSE: usize = ms(250);

#[derive(Clone, Copy, Default)]
struct Window {
    /// The least audio buffered at a render pull beyond the device period it
    /// needs, `usize::MAX` for none yet.
    low: usize,
    /// The longest gap between packets that their audio did not cover, in
    /// samples played.
    gap: usize,
}
/// Decoded samples waiting for the render device. The client delivers whole
/// packets, sometimes several at once, and Windows takes a device period at
/// a time; the queue absorbs that and the network's jitter. It learns the
/// gap between packets over the last two seconds, from gaps that came back
/// at least twice, and holds up to twice that, at most 150 ms (and at least
/// 60 ms or three packets, as before), so a client that sends 60 ms at a
/// time is not cut. A steady client, or one stall or loss burst, keeps the
/// 60 ms bound: the oldest samples go, down to one packet (at least 20 ms),
/// or to the gap for a bursty client. Standing delay goes in 2.5 ms slices,
/// crossfaded. When the queue runs dry it counts the underrun and plays
/// again once it has enough for a wake-up, adding no wait of its own.
#[derive(Default)]
pub struct Playout {
    queue: VecDeque<f32>,
    /// Samples dropped to keep the delay bounded.
    pub trimmed: u64,
    /// Times the queue ran dry while audio kept coming, and the silence
    /// that filled them, in samples.
    pub underruns: u64,
    pub underrun_samples: u64,
    /// Whether the device is being fed from the queue: off until the first
    /// audio and after an underrun until the queue has refilled.
    playing: bool,
    /// Silence written since the queue ran dry, while it is refilling.
    dry: Option<usize>,
    /// Samples written to the device, the queue's clock.
    clock: usize,
    last_push: Option<usize>,
    packet: usize,
    /// The learnt gap: it follows a higher one at once and a lower one by
    /// `SLICE` a window.
    held: usize,
    windows: [Window; WINDOWS],
    window: usize,
    window_end: usize,
}
impl Playout {
    pub fn push(&mut self, samples: &[f32]) {
        self.push_batch(samples, samples.len());
    }
    /// Queue a packet together with any preceding concealment. The queue
    /// bound follows one packet's duration, not the loss burst's total PCM,
    /// and the concealment covers its part of the gap before the packet.
    pub fn push_batch(&mut self, samples: &[f32], packet_frames: usize) {
        if let Some(last) = self.last_push.replace(self.clock) {
            // A pause (a mute) is not a gap to buffer for.
            let concealed = samples.len().saturating_sub(packet_frames);
            let gap = (self.clock - last).saturating_sub(concealed);
            if gap <= PAUSE {
                let window = &mut self.windows[self.window];
                window.gap = window.gap.max(gap);
                self.held = self.held.max(self.recurring());
            }
        }
        self.packet = packet_frames;
        self.queue.extend(samples.iter().map(|s| s.clamp(-1., 1.)));
        let limit = (2 * self.gap()).min(CAP).max(3 * packet_frames).max(ms(60));
        if self.queue.len() > limit {
            self.cut(self.queue.len().saturating_sub(self.depth()));
        }
    }
    /// The longest gap between packets lately that came back at least twice,
    /// in windows apart: one stall is not a pattern to buffer for, even when
    /// the network releases it in two batches in neighbouring windows.
    fn recurring(&self) -> usize {
        let top = (0..WINDOWS)
            .max_by_key(|&i| self.windows[i].gap)
            .unwrap_or(0);
        (0..WINDOWS)
            .filter(|&i| (2..=WINDOWS - 2).contains(&((i + WINDOWS - top) % WINDOWS)))
            .map(|i| self.windows[i].gap)
            .max()
            .unwrap_or(0)
    }
    /// The gap the queue buffers for.
    fn gap(&self) -> usize {
        self.held
    }
    /// What the queue keeps after a cut: enough to last until the next
    /// packet, and at least one packet (20 ms or more). Never more than the
    /// limit `push_batch` cuts at.
    fn depth(&self) -> usize {
        self.gap().min(CAP).max(self.packet).max(ms(20))
    }
    /// Drops the oldest `drop` samples. While playing, the samples kept
    /// first fade in from the ones dropped, so the seam with what the
    /// device already has does not click.
    fn cut(&mut self, drop: usize) {
        let fade = if self.playing {
            FADE.min(self.queue.len() - drop)
        } else {
            0
        };
        let mut old = [0.; FADE];
        for (slot, &sample) in old.iter_mut().zip(self.queue.iter()).take(fade) {
            *slot = sample;
        }
        for (i, &old) in old.iter().enumerate().take(fade) {
            let weight = (i + 1) as f32 / (fade + 1) as f32;
            let new = self.queue[drop + i];
            self.queue[drop + i] = old + (new - old) * weight;
        }
        self.queue.drain(..drop);
        self.trimmed += drop as u64;
        for window in &mut self.windows {
            if window.low != usize::MAX {
                window.low = window.low.saturating_sub(drop);
            }
        }
    }
    /// One render pull: the device holds `padding` of its `buffer` frames
    /// and plays `period` per wake-up. Moves the samples [`render_plan`]
    /// gives it into `out` and returns the silence to write after them.
    pub fn pull(
        &mut self,
        buffer: usize,
        padding: usize,
        period: usize,
        out: &mut Vec<f32>,
    ) -> usize {
        // Refilled: enough that this wake-up needs no silence, so a queue of
        // small packets does not play a sliver every period.
        if !self.playing && !self.queue.is_empty() && self.queue.len() + padding >= period {
            self.playing = true;
            if let Some(dry) = self.dry.take().filter(|&dry| dry <= PAUSE) {
                self.underruns += 1;
                self.underrun_samples += dry as u64;
            }
        }
        let window = &mut self.windows[self.window];
        window.low = window
            .low
            .min((self.queue.len() + padding).saturating_sub(period));
        let ready = if self.playing { self.queue.len() } else { 0 };
        let (samples, silence) = render_plan(buffer, padding, period, ready);
        out.clear();
        out.extend(self.queue.drain(..samples));
        if self.playing && silence > 0 {
            // The device would run dry: an underrun, unless no audio follows.
            self.playing = false;
            self.dry = Some(silence);
        } else if let Some(dry) = &mut self.dry {
            *dry += silence;
        }
        self.clock += samples + silence;
        while self.clock >= self.window_end {
            self.window_end += WINDOW;
            self.window = (self.window + 1) % WINDOWS;
            self.windows[self.window] = Window {
                low: usize::MAX,
                gap: 0,
            };
            self.held = self.recurring().max(self.held.saturating_sub(SLICE));
            let low = (1..=SETTLED)
                .map(|back| self.windows[(self.window + WINDOWS - back) % WINDOWS].low)
                .min()
                .unwrap_or(0);
            let needed = self.gap().saturating_sub(self.packet).max(STANDING);
            if self.playing
                && low != usize::MAX
                && low >= needed + SLICE
                && self.queue.len() >= SLICE + FADE
            {
                self.cut(SLICE);
            }
        }
        silence
    }
    pub fn len(&self) -> usize {
        self.queue.len()
    }
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }
    /// Moves up to `out.len()` samples into `out`; returns how many. Unlike
    /// [`Self::pull`], this is not a device pull and counts nothing.
    pub fn take(&mut self, out: &mut [f32]) -> usize {
        let n = out.len().min(self.queue.len());
        for (slot, sample) in out.iter_mut().zip(self.queue.drain(..n)) {
            *slot = sample;
        }
        n
    }
    /// Starts over for the next speaker, counters included.
    pub fn clear(&mut self) {
        *self = Self::default();
    }
}
/// Frames to write to a render buffer of `buffer` frames that still holds
/// `padding` unplayed frames and plays `period` frames per wake-up, with
/// `queued` decoded frames waiting: (samples, silence). The device is kept
/// at most two periods ahead, so the waiting audio stays in the queue,
/// where `Playout` bounds it. Silence goes in only when the device would
/// otherwise run dry before the next wake-up.
pub fn render_plan(buffer: usize, padding: usize, period: usize, queued: usize) -> (usize, usize) {
    let room = buffer.saturating_sub(padding);
    let samples = queued.min((2 * period).saturating_sub(padding)).min(room);
    let silence = period.saturating_sub(padding + samples).min(room - samples);
    (samples, silence)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn datagrams_match_the_apollo_client_layout() {
        let key = [7; 16];
        let datagram = seal(&key, 0xffff_fffe, 3, 0x0102_0304, b"opus frame");
        // flags, type, LE sequence, LE timestamp, LE magic.
        assert_eq!(
            &datagram[..HEADER],
            &[0, 0x61, 3, 0, 4, 3, 2, 1, 0x78, 0x56, 0x34, 0x12]
        );
        // The packet padded to a block, and a block of padding after it.
        assert_eq!(datagram.len(), HEADER + 32);
        let packet = parse(&datagram).unwrap();
        assert_eq!((packet.sequence, packet.timestamp), (3, 0x0102_0304));
        // The key ID and sequence add with wrap-around, as in C.
        let mut iv = [0; 16];
        iv[..4].copy_from_slice(&1u32.to_be_bytes());
        let mut inner = b"opus frame".to_vec();
        inner.extend([6; 6]);
        assert_eq!(crypto::cbc_open(&key, &iv, packet.payload).unwrap(), inner);
        assert_eq!(open(&key, 0xffff_fffe, &packet).unwrap(), b"opus frame");
        // Ciphertext from moonlight-common-c's own PltEncryptMessage (OpenSSL),
        // key 07.., key ID 9, sequence 70, packet bytes 0x20, 0x21, ...
        for (length, hex) in [
            (
                4,
                "306a36d0512bb803ccc69a69ae7e2aed501d20abae2a575017bdb0ca2cf32fc3",
            ),
            (
                16,
                "1e10689ae2b276383d9f36756341ba398f6060d44c12333163d5a710f9c2c20e",
            ),
            (
                33,
                "1e10689ae2b276383d9f36756341ba3996287332b3430f8ba7119c451d482a13c19f4f2afd4574a761afc77c968ba6025e91b8ea56f889d013870cd329a41bd6",
            ),
        ] {
            let opus: Vec<u8> = (0x20..0x20 + length).collect();
            let datagram = seal(&key, 9, 70, 0, &opus);
            assert_eq!(hex::encode(&datagram[HEADER..]), hex, "{length}");
            assert_eq!(open(&key, 9, &parse(&datagram).unwrap()).unwrap(), opus);
        }
        // A client that pads once.
        let once = Packet {
            sequence: 70,
            timestamp: 0,
            payload: &crypto::cbc_seal(&key, &super::iv(9, 70), &[1, 2, 3]),
        };
        assert_eq!(open(&key, 9, &once).unwrap(), [1, 2, 3]);
        assert_ne!(
            open(&[8; 16], 0xffff_fffe, &packet).ok().as_deref(),
            Some(&b"opus frame"[..])
        );
    }

    #[test]
    fn other_datagrams_are_not_microphone_packets() {
        let datagram = seal(&[1; 16], 0, 0, 0, b"x");
        assert!(parse(&datagram[..HEADER]).is_none());
        let mut wrong_type = datagram.clone();
        wrong_type[1] = 0x60;
        assert!(parse(&wrong_type).is_none());
        let mut wrong_magic = datagram.clone();
        wrong_magic[8] ^= 1;
        assert!(parse(&wrong_magic).is_none());
        for payload in [&[0; 15][..], &[]] {
            let packet = Packet {
                sequence: 0,
                timestamp: 0,
                payload,
            };
            assert!(open(&[1; 16], 0, &packet).is_err());
        }
    }

    #[test]
    fn the_offer_is_what_clients_look_for() {
        let sdp = sdp(48001);
        assert!(sdp.starts_with("m=audio 48001 RTP/AVP 96\r\n"));
        assert!(sdp.contains("a=rtpmap:96 opus/48000/1\r\n"));
    }

    #[test]
    fn gaps_are_concealed_late_packets_dropped_and_restarts_followed() {
        let mut s = Sequencer::default();
        assert_eq!(s.arrive(65534), Arrival::Restart);
        assert_eq!(s.arrive(65535), Arrival::Next);
        // Wrap-around is the next packet.
        assert_eq!(s.arrive(0), Arrival::Next);
        assert_eq!(s.arrive(0), Arrival::Late);
        assert_eq!(s.arrive(65535), Arrival::Late);
        assert_eq!(s.arrive(3), Arrival::AfterLoss { missing: 2 });
        // A packet that was concealed and then arrives is late.
        assert_eq!(s.arrive(2), Arrival::Late);
        assert_eq!(s.arrive(4), Arrival::Next);
        assert_eq!(
            s.arrive(4 + 1 + MAX_CONCEALED),
            Arrival::AfterLoss {
                missing: MAX_CONCEALED
            }
        );
        assert_eq!(s.arrive(100), Arrival::Restart);
        assert_eq!(s.arrive(101), Arrival::Next);
        // A client that counts from 0 again is followed after a short run.
        for n in 0..LATE_RESTART - 1 {
            assert_eq!(s.arrive(n as u16), Arrival::Late);
        }
        assert_eq!(s.arrive(LATE_RESTART as u16 - 1), Arrival::Restart);
        assert_eq!(s.arrive(LATE_RESTART as u16), Arrival::Next);
    }

    #[test]
    fn the_queue_stays_within_three_packets_and_keeps_the_newest() {
        let mut p = Playout::default();
        let packet: Vec<f32> = (0..960).map(|n| n as f32 / 960.).collect();
        for _ in 0..3 {
            p.push(&packet);
        }
        assert_eq!((p.len(), p.trimmed), (2880, 0));
        p.push(&packet);
        // Cut back to one packet, the newest.
        assert_eq!((p.len(), p.trimmed), (960, 2880));
        let mut out = vec![0.; 1000];
        assert_eq!(p.take(&mut out), 960);
        assert_eq!(out[959], 959. / 960.);
        assert!(p.is_empty());
        // Small packets still get 60 ms before anything goes.
        for _ in 0..12 {
            p.push(&[2.; 240]);
        }
        assert_eq!((p.len(), p.trimmed), (2880, 2880));
        assert_eq!(p.take(&mut out[..1]), 1);
        assert_eq!(out[0], 1., "samples are clamped");
    }

    #[test]
    fn concealment_batches_keep_the_individual_packet_latency_bound() {
        for packet_frames in [240, 960, 5760] {
            let mut playout = Playout::default();
            let count = (6 * packet_frames).max(RATE * 80 / 1000);
            let batch: Vec<_> = (0..count).map(|i| i as f32 / count as f32).collect();
            playout.push_batch(&batch, packet_frames);
            let keep = packet_frames.max(RATE * 20 / 1000);
            assert_eq!(playout.len(), keep);
            assert_eq!(playout.trimmed, (count - keep) as u64);
            let mut newest = vec![0.; keep];
            assert_eq!(playout.take(&mut newest), keep);
            assert_eq!(newest, batch[count - keep..]);
        }
    }

    /// What a render pull drains: the playout, or the bound it replaced, so
    /// the two can be compared.
    trait Queue: Default {
        fn push_batch(&mut self, samples: &[f32], packet_frames: usize);
        fn pull(
            &mut self,
            buffer: usize,
            padding: usize,
            period: usize,
            out: &mut Vec<f32>,
        ) -> usize;
        fn len(&self) -> usize;
    }
    impl Queue for Playout {
        fn push_batch(&mut self, samples: &[f32], packet_frames: usize) {
            Playout::push_batch(self, samples, packet_frames);
        }
        fn pull(
            &mut self,
            buffer: usize,
            padding: usize,
            period: usize,
            out: &mut Vec<f32>,
        ) -> usize {
            Playout::pull(self, buffer, padding, period, out)
        }
        fn len(&self) -> usize {
            Playout::len(self)
        }
    }
    /// The queue before: above three packets or 60 ms, cut to one packet
    /// (at least 20 ms), and the device fed whatever is there.
    #[derive(Default)]
    struct Bounded(VecDeque<f32>);
    impl Queue for Bounded {
        fn push_batch(&mut self, samples: &[f32], packet_frames: usize) {
            self.0.extend(samples);
            if self.0.len() > (3 * packet_frames).max(ms(60)) {
                let drop = self.0.len() - packet_frames.max(ms(20));
                self.0.drain(..drop);
            }
        }
        fn pull(
            &mut self,
            buffer: usize,
            padding: usize,
            period: usize,
            out: &mut Vec<f32>,
        ) -> usize {
            let (samples, silence) = render_plan(buffer, padding, period, self.0.len());
            out.clear();
            out.extend(self.0.drain(..samples));
            silence
        }
        fn len(&self) -> usize {
            self.0.len()
        }
    }
    struct Run<Q> {
        queue: Q,
        /// Everything the device played.
        played: Vec<f32>,
        /// The mean of what the device and the queue held at the pulls
        /// after `from`, in ms: how far playback is behind arrival.
        delay: f64,
    }
    /// A device of `period` samples in a buffer of two, pulled every period
    /// until `until` (µs), fed a 1 kHz tone: `deliveries` are (µs, samples),
    /// ascending, in packets of `packet` (more is concealment before one).
    /// `each` sees the queue after every push and pull, with the time.
    fn simulate<Q: Queue>(
        deliveries: &[(u64, usize)],
        packet: usize,
        period: usize,
        until: u64,
        from: u64,
        mut each: impl FnMut(u64, &Q),
    ) -> Run<Q> {
        let mut queue = Q::default();
        let (mut played, mut out) = (Vec::new(), Vec::new());
        let (mut padding, mut sent) = (0, 0);
        let (mut delay, mut pulls) = (0, 0);
        let mut deliveries = deliveries.iter().copied().peekable();
        for now in (0..until).step_by(period * 1_000_000 / RATE) {
            while let Some((at, frames)) = deliveries.next_if(|&(at, _)| at < now) {
                let samples: Vec<f32> = (sent..sent + frames)
                    .map(|n| ((n % 48) as f32 * std::f32::consts::TAU / 48.).sin() * 0.5)
                    .collect();
                sent += frames;
                queue.push_batch(&samples, packet);
                each(at, &queue);
            }
            padding = usize::saturating_sub(padding, period);
            if now >= from {
                delay += queue.len() + padding;
                pulls += 1;
            }
            let silence = queue.pull(2 * period, padding, period, &mut out);
            played.extend(&out);
            played.extend(std::iter::repeat_n(0., silence));
            padding += out.len() + silence;
            each(now, &queue);
        }
        let delay = delay as f64 / pulls as f64 * 1000. / RATE as f64;
        Run {
            queue,
            played,
            delay,
        }
    }
    /// 20 ms packets at `times`.
    fn packets(times: impl IntoIterator<Item = u64>) -> Vec<(u64, usize)> {
        times.into_iter().map(|at| (at, 960)).collect()
    }
    /// After the first `primed` µs: the most the queue held, and whether
    /// anything was trimmed or ran dry.
    fn after_priming(
        deliveries: &[(u64, usize)],
        until: u64,
        primed: u64,
    ) -> (Run<Playout>, usize, u64, u64) {
        let mut most = 0;
        let mut at_priming = None;
        let run = simulate(deliveries, 960, 480, until, primed, |now, p: &Playout| {
            if now >= primed {
                at_priming.get_or_insert((p.trimmed, p.underruns));
                most = most.max(p.len());
            }
        });
        let (trimmed, underruns) = at_priming.unwrap();
        let trimmed = run.queue.trimmed - trimmed;
        let underruns = run.queue.underruns - underruns;
        (run, most, trimmed, underruns)
    }
    /// A small deterministic jitter in 0..=2 * `range` µs.
    fn jitter(seed: &mut u32, range: u64) -> u64 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 17;
        *seed ^= *seed << 5;
        u64::from(*seed) % (2 * range + 1)
    }

    #[test]
    fn a_client_that_sends_in_bursts_is_not_cut() {
        // Three packets every 60 ms, as some Android TVs send them.
        let regular = packets((0..500u64).flat_map(|k| [5_000 + k * 60_000; 3]));
        let (run, most, trimmed, underruns) = after_priming(&regular, 30_000_000, 0);
        assert_eq!((trimmed, underruns, run.queue.underrun_samples), (0, 0, 0));
        assert!(most <= ms(60), "{most}");
        // Every sample sent was played.
        assert!(run.played.iter().filter(|s| **s != 0.).count() > 499 * 2880 * 9 / 10);
        // Bursts that are irregular, which a 60 ms bound cut a chunk out of
        // every time: each up to 15 ms early or late, or of two to four
        // packets. Once the queue has learnt the spread, it neither cuts nor
        // runs dry.
        let mut seed = 0x2545_f491;
        let jittered =
            packets((0..500u64).flat_map(|k| [2_000 + k * 60_000 + jitter(&mut seed, 15_000); 3]));
        let sized = packets(
            (0..500u64).flat_map(|k| vec![5_000 + k * 60_000; [3, 2, 4, 3][k as usize % 4]]),
        );
        for deliveries in [jittered, sized] {
            let (_, most, trimmed, underruns) = after_priming(&deliveries, 30_000_000, 5_000_000);
            assert_eq!((trimmed, underruns), (0, 0));
            assert!(most <= CAP, "{most}");
        }
    }

    #[test]
    fn a_steady_client_is_played_no_later_than_before() {
        let mut seed = 0x1234_5678;
        let straddling =
            packets((0..1500u64).map(|k| 8_000 + k * 20_000 + jitter(&mut seed, 2_000)));
        let short: Vec<_> = (0..3000u64)
            .map(|k| (3_000 + k * 10_000 + jitter(&mut seed, 1_500), 480))
            .collect();
        let late =
            packets((0..1500u64).map(|k| 5_000 + k * 20_000 + if k == 150 { 8_000 } else { 0 }));
        let muted = packets(
            (0..1500u64)
                .filter(|k| !(150..180).contains(k))
                .map(|k| 5_000 + k * 20_000),
        );
        // 100 ms with nothing, then its packets at once: once, and every 4 s.
        let stall = |every: u64| {
            packets((0..1500u64).map(move |k| {
                let at = 5_000 + k * 20_000;
                let phase = at % every;
                if (3_000_000..3_100_000).contains(&phase) {
                    at - phase + 3_100_000
                } else {
                    at
                }
            }))
        };
        // Three or five packets lost and concealed in front of the next one.
        let concealed = |lost: u64| -> Vec<(u64, usize)> {
            (0..1500u64)
                .filter(|k| !(150..150 + lost).contains(k))
                .map(|k| {
                    (
                        5_000 + k * 20_000,
                        if k == 150 + lost {
                            960 * (lost as usize + 1)
                        } else {
                            960
                        },
                    )
                })
                .collect()
        };
        for (name, deliveries, packet, period) in [
            (
                "20 ms packets around a wake-up",
                straddling.clone(),
                960,
                480,
            ),
            ("10 ms packets", short, 480, 480),
            ("one packet 8 ms late", late, 960, 480),
            ("600 ms muted", muted.clone(), 960, 480),
            ("one stall", stall(30_000_000), 960, 480),
            ("a stall every 4 s", stall(4_000_000), 960, 480),
            ("3 concealed", concealed(3), 960, 480),
            ("5 concealed", concealed(5), 960, 480),
            ("5 ms device", straddling, 960, 240),
        ] {
            let new = simulate::<Playout>(
                &deliveries,
                packet,
                period,
                30_000_000,
                1_000_000,
                |_, _| {},
            );
            let old = simulate::<Bounded>(
                &deliveries,
                packet,
                period,
                30_000_000,
                1_000_000,
                |_, _| {},
            );
            assert!(
                new.delay <= old.delay + 0.01,
                "{name}: {:.1} ms, before {:.1} ms",
                new.delay,
                old.delay
            );
        }
        // A mute is not an underrun.
        let run = simulate::<Playout>(&muted, 960, 480, 30_000_000, 0, |_, _| {});
        assert_eq!(run.queue.underruns, 0);
    }

    #[test]
    fn a_stall_released_in_two_batches_is_not_learnt() {
        // 80 ms with nothing, two packets, 60 ms more, then the rest: two
        // long gaps, often in neighbouring windows, but one stall. Every
        // phase of the stall against the windows keeps the old delay.
        for phase in 0..20u64 {
            let stall = 3_000_000 + phase * 5_000;
            let deliveries = packets((0..500u64).map(|k| {
                let at = 5_000 + k * 20_000;
                match at {
                    _ if at > stall && at < stall + 40_000 => stall + 80_000,
                    _ if at > stall + 40_000 && at < stall + 140_000 => stall + 140_000,
                    _ => at,
                }
            }));
            let new = simulate::<Playout>(&deliveries, 960, 480, 6_000_000, 2_000_000, |_, _| {});
            let old = simulate::<Bounded>(&deliveries, 960, 480, 6_000_000, 2_000_000, |_, _| {});
            assert!(
                new.delay <= old.delay + 0.01,
                "phase {phase}: {:.1} ms, before {:.1} ms",
                new.delay,
                old.delay
            );
        }
    }

    #[test]
    fn a_fast_client_clock_loses_small_crossfaded_slices() {
        // The client's 20 ms take 19.8 ms of the host's time: 1% too much audio.
        let deliveries = packets((0..3000u64).map(|k| 5_000 + k * 19_800));
        let mut trimmed = 0;
        let run = simulate(
            &deliveries,
            960,
            480,
            59_000_000,
            1_000_000,
            |now, p: &Playout| {
                assert!(p.trimmed - trimmed <= SLICE as u64, "{now}");
                trimmed = p.trimmed;
                if now > 1_000_000 {
                    assert!(p.len() <= ms(60), "{now}: {}", p.len());
                }
            },
        );
        assert_eq!(run.queue.underruns, 0);
        assert_eq!(run.queue.trimmed % SLICE as u64, 0);
        let sent = deliveries
            .iter()
            .filter(|&&(at, _)| at < 58_990_000)
            .count()
            * 960;
        let share = run.queue.trimmed as f64 / sent as f64;
        assert!((0.008..0.0105).contains(&share), "{share}");
        // The tone moves at most 0.065 a sample; a cut without the fade
        // would jump by up to 1.
        let jump = run
            .played
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0., f32::max);
        assert!(jump < 0.08, "{jump}");
        // No later than the old bound, which let it grow to 60 ms and cut.
        let old = simulate::<Bounded>(&deliveries, 960, 480, 59_000_000, 1_000_000, |_, _| {});
        assert!(
            run.delay <= old.delay,
            "{:.1} ms, before {:.1} ms",
            run.delay,
            old.delay
        );
    }

    #[test]
    fn a_short_cut_fades_from_the_samples_it_drops() {
        let ramp: Vec<f32> = (0..400).map(|n| n as f32 / 400.).collect();
        let mut playout = Playout::default();
        playout.push(&ramp);
        playout.playing = true;
        playout.cut(10);
        let mut kept = vec![0.; 390];
        assert_eq!(playout.take(&mut kept), 390);
        for (i, &sample) in kept.iter().enumerate() {
            let expected = if i < FADE {
                let weight = (i + 1) as f32 / (FADE + 1) as f32;
                ramp[i] + (ramp[10 + i] - ramp[i]) * weight
            } else {
                ramp[10 + i]
            };
            assert!((sample - expected).abs() < 1e-6, "{i}: {sample} {expected}");
        }
    }

    #[test]
    fn underruns_are_counted_and_refilled_but_pauses_are_not() {
        // Steady for two seconds with a 60 ms stall (its packets then come
        // at once), 600 ms of mute, a second more, and the end of the stream.
        let deliveries = packets((0..100u64).chain(130..180).map(|k| {
            5_000
                + k * 20_000
                + if (50..53).contains(&k) {
                    (53 - k) * 20_000
                } else {
                    0
                }
        }));
        let mut dry_pulls = 0;
        let mut was_dry = false;
        let run = simulate(&deliveries, 960, 480, 4_000_000, 0, |_, p: &Playout| {
            // One underrun is one dry spell, not one per device period.
            if p.dry.is_some() && !was_dry {
                dry_pulls += 1;
            }
            was_dry = p.dry.is_some();
        });
        let mut playout = run.queue;
        assert_eq!(playout.underruns, 1);
        // 60 ms late, less what the device still held.
        assert!(
            (ms(40)..=ms(70)).contains(&(playout.underrun_samples as usize)),
            "{}",
            playout.underrun_samples
        );
        // The stall, the mute, and the end of the stream.
        assert_eq!(dry_pulls, 3);
        playout.clear();
        assert_eq!(
            (playout.underruns, playout.trimmed, playout.len()),
            (0, 0, 0)
        );
    }

    #[test]
    fn the_device_is_kept_two_periods_ahead_and_never_runs_dry() {
        // 10 ms periods in a 20 ms buffer.
        assert_eq!(render_plan(960, 0, 480, 0), (0, 480));
        assert_eq!(render_plan(960, 0, 480, 960), (960, 0));
        assert_eq!(render_plan(960, 480, 480, 960), (480, 0));
        assert_eq!(render_plan(960, 480, 480, 100), (100, 0));
        assert_eq!(render_plan(960, 100, 480, 100), (100, 280));
        assert_eq!(render_plan(960, 960, 480, 960), (0, 0));
        // A larger buffer is not filled past two periods.
        assert_eq!(render_plan(4800, 0, 480, 4000), (960, 0));
    }
}
