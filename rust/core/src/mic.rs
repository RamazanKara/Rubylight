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
mod tests;
