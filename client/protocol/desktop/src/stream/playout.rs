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
mod tests;
