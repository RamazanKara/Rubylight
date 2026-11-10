//! The audio stream's threads: UDP receive with the 100 ms poll timeouts and the 30-packet
//! queue to the renderer's thread (or direct submission on the receive thread). The socket
//! and its pings start during the RTSP handshake, as GFE 3.22 answers PLAY only after an
//! audio ping.
//!
//! Ported from moonlight-common-c's `AudioStream.c` (GPL-3.0, Cameron Gutman and
//! contributors).

use std::collections::VecDeque;
use std::net::UdpSocket;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;

use super::net::{self, NextSocket};
use super::trace::{PacketTrace, Stream};
use super::{now_us, AudioRenderer, Shared};
use crate::audio::{AudioOutput, AudioReceiver, AudioStats};

/// `packetQueue`'s bound; a full queue is emptied before the next packet goes in.
pub const AUDIO_QUEUE_BOUND: usize = 30;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|p| p.into_inner())
}

/// A packet in the stream's codec, or `None` for one lost (packet loss concealment).
type Item = Option<Vec<u8>>;

/// Packet buffers the decode thread has played, kept for the next packets: every packet the queue
/// can hold, and the one being played.
const SPARE_BOUND: usize = AUDIO_QUEUE_BOUND + 1;

#[derive(Default)]
struct QueueState {
    items: VecDeque<Item>,
    /// Played packets' buffers, which `offer_copy` fills again instead of allocating.
    spare: Vec<Vec<u8>>,
    shutdown: bool,
}

impl QueueState {
    fn keep_spare(&mut self, buffer: Vec<u8>) {
        if self.spare.len() < SPARE_BOUND {
            self.spare.push(buffer);
        }
    }
}

#[derive(Default)]
pub struct PacketQueue {
    state: Mutex<QueueState>,
    ready: Condvar,
    pub overflows: AtomicU64,
}

impl PacketQueue {
    /// Queues an item; a full queue is flushed first, as the C does.
    #[cfg(test)]
    pub fn offer(&self, item: Item) -> bool {
        let mut state = lock(&self.state);
        if state.shutdown {
            return false;
        }
        if state.items.len() >= AUDIO_QUEUE_BOUND {
            state.items.clear();
            self.overflows.fetch_add(1, Ordering::Relaxed);
        }
        state.items.push_back(item);
        self.ready.notify_one();
        true
    }

    /// [`PacketQueue::offer`] of a copy of `packet` (`None`: a lost one), in a buffer an earlier
    /// packet was played from, so steady streaming allocates nothing per packet. A full queue's
    /// flushed packets become spares too.
    pub fn offer_copy(&self, packet: Option<&[u8]>) -> bool {
        let mut state = lock(&self.state);
        if state.shutdown {
            return false;
        }
        if state.items.len() >= AUDIO_QUEUE_BOUND {
            while let Some(flushed) = state.items.pop_front() {
                if let Some(buffer) = flushed {
                    state.keep_spare(buffer);
                }
            }
            self.overflows.fetch_add(1, Ordering::Relaxed);
        }
        let item = packet.map(|bytes| {
            let mut buffer = state.spare.pop().unwrap_or_default();
            buffer.clear();
            buffer.extend_from_slice(bytes);
            buffer
        });
        state.items.push_back(item);
        self.ready.notify_one();
        true
    }

    #[cfg(test)]
    fn wait(&self) -> Option<Item> {
        self.wait_returning(None)
    }

    /// The next item, after taking back `played`, the buffer of the last one played.
    fn wait_returning(&self, played: Option<Vec<u8>>) -> Option<Item> {
        let mut state = lock(&self.state);
        if let Some(buffer) = played {
            state.keep_spare(buffer);
        }
        loop {
            if state.shutdown {
                return None;
            }
            if let Some(item) = state.items.pop_front() {
                return Some(item);
            }
            state = self.ready.wait(state).unwrap_or_else(|p| p.into_inner());
        }
    }

    pub fn len(&self) -> usize {
        lock(&self.state).items.len()
    }

    fn shutdown(&self) {
        lock(&self.state).shutdown = true;
        self.ready.notify_all();
    }
}

/// State shared with the session for statistics.
#[derive(Default)]
pub struct AudioShared {
    pub queue: PacketQueue,
    pub stats: Mutex<AudioStats>,
    pub packets: AtomicU64,
    /// The milliseconds of audio in each queued item: the Opus packet duration, PCM's packet
    /// for the channel count or one 32 ms AC3 or E-AC3 frame (`passthrough::packet_ms`).
    pub packet_ms: AtomicU32,
}

impl AudioShared {
    /// `LiGetPendingAudioDuration`: the milliseconds of audio waiting for the renderer.
    pub fn pending_ms(&self) -> u32 {
        self.queue.len() as u32 * self.packet_ms.load(Ordering::Relaxed)
    }
}

/// How late audio packets arrive, for the log: each packet against its place in the stream
/// (packet n is due n packets after the first), relative to the low end of the arrivals of the
/// last few seconds, which follows the drift between the host's clock and this one. A host that
/// sends its 5 ms packets in pairs every 10 ms shows as 5 ms; a Wi-Fi stall as its length.
/// A window is logged only when a packet in it came 8 ms or more late.
///
/// The low end, not the earliest: Windows' audio engine hands a host two of its 10 ms periods a
/// slot early about twice a second, then none for 20 ms. Against those few, every other packet
/// read 9-14 ms late for the whole stream (nightjar#74; a 40 s trace had 2% of the packets early)
/// although the cushion follows the arrivals of the rest.
pub struct ArrivalJitter {
    packet_us: u64,
    start_us: u64,
    last_us: u64,
    packets: u64,
    /// The low end of the lateness in each of the last seconds, newest last: (second, lateness).
    lows: VecDeque<(u64, i64)>,
    /// The lateness of this second's packets so far, and which second.
    second: Vec<i64>,
    second_at: u64,
    /// Packets per millisecond of lateness in this window; the last bucket holds the rest.
    histogram: [u32; ArrivalJitter::BUCKETS],
    window_us: u64,
}

impl ArrivalJitter {
    const BUCKETS: usize = 64;
    const WINDOW_US: u64 = 10_000_000;
    const DRIFT_SECONDS: u64 = 4;
    /// A gap this long (a pause, a roam) starts the stream's schedule, and the window, again.
    const RESTART_US: u64 = 1_000_000;
    /// The lateness a window must reach to be logged: what two 4 ms bursts would not hold.
    const LOG_FROM_MS: usize = 8;
    /// A second's low end is the lateness this share (1 in N) of its packets beat: within the
    /// earlier packet of each pair, above a few early wakes in a second.
    const LOW_END: usize = 10;

    pub fn new(packet_ms: u32) -> Self {
        Self {
            packet_us: u64::from(packet_ms.max(1)) * 1000,
            start_us: 0,
            last_us: 0,
            packets: 0,
            lows: VecDeque::new(),
            second: Vec::new(),
            second_at: 0,
            histogram: [0; Self::BUCKETS],
            window_us: 0,
        }
    }

    fn count(&mut self, late_us: i64) {
        let bucket = (late_us.max(0) / 1000) as usize;
        self.histogram[bucket.min(Self::BUCKETS - 1)] += 1;
    }

    /// The second before `now` (a second of the stream) has ended: its low end joins the last
    /// few. The stream's first second had none to go by until now, so it is counted here.
    fn end_second(&mut self, now: u64) {
        if self.second.is_empty() {
            return;
        }
        let share = self.second.len() / Self::LOW_END;
        let low = *self.second.select_nth_unstable(share).1;
        if self.lows.is_empty() {
            for at in 0..self.second.len() {
                self.count(self.second[at] - low);
            }
        }
        self.second.clear();
        self.lows.push_back((self.second_at, low));
        while self
            .lows
            .front()
            .is_some_and(|(at, _)| at + Self::DRIFT_SECONDS < now)
        {
            self.lows.pop_front();
        }
    }

    /// One packet in stream order at `now_us`; `lost` ones keep their place but have no
    /// arrival. Returns the window's summary line when a window ends.
    pub fn packet(&mut self, now_us: u64, lost: bool) -> Option<String> {
        if self.packets == 0 || now_us.saturating_sub(self.last_us) >= Self::RESTART_US {
            self.start_us = now_us;
            self.packets = 0;
            self.lows.clear();
            self.second.clear();
            self.second_at = 0;
            self.window_us = now_us;
            self.histogram = [0; Self::BUCKETS];
        }
        let due = self.start_us + self.packets * self.packet_us;
        self.packets += 1;
        if lost {
            return None;
        }
        self.last_us = now_us;
        let late = now_us as i64 - due as i64;
        let second = (now_us - self.start_us) / 1_000_000;
        if second != self.second_at {
            self.end_second(second);
            self.second_at = second;
        }
        self.second.push(late);
        if let Some(low) = self.lows.iter().map(|(_, low)| *low).min() {
            self.count(late - low);
        }
        if now_us.saturating_sub(self.window_us) < Self::WINDOW_US {
            return None;
        }
        self.window_us = now_us;
        let late = self.histogram[Self::LOG_FROM_MS..]
            .iter()
            .any(|&count| count > 0);
        let line = late.then(|| self.summary());
        self.histogram = [0; Self::BUCKETS];
        line
    }

    /// Percentiles of the window's lateness, and how many packets came later than each
    /// reserve a renderer might hold.
    fn summary(&self) -> String {
        let total: u32 = self.histogram.iter().sum();
        let at = |share: f64| {
            let wanted = (f64::from(total) * share).ceil().max(1.0) as u32;
            let mut seen = 0;
            for (ms, count) in self.histogram.iter().enumerate() {
                seen += count;
                if seen >= wanted {
                    return ms;
                }
            }
            Self::BUCKETS - 1
        };
        let max = self
            .histogram
            .iter()
            .rposition(|&count| count > 0)
            .unwrap_or(0);
        let over: Vec<String> = [8, 12, 16, 20, 24, 32, 40]
            .iter()
            .map(|&ms| format!(">={ms}:{}", self.histogram[ms..].iter().sum::<u32>()))
            .collect();
        format!(
            "Audio arrival over 10 s: {total} packets, late p50 {} p95 {} p99 {} max {max} ms, {}",
            at(0.5),
            at(0.95),
            at(0.99),
            over.join(" ")
        )
    }
}

pub struct AudioStream {
    shared: Arc<AudioShared>,
    stop: Arc<AtomicBool>,
    receive: Option<JoinHandle<Option<Box<dyn AudioRenderer>>>>,
    decode: Option<JoinHandle<Box<dyn AudioRenderer>>>,
}

impl AudioStream {
    pub fn start(
        socket: &UdpSocket,
        next_socket: Arc<NextSocket>,
        receiver: AudioReceiver,
        renderer: Box<dyn AudioRenderer>,
        direct_submit: bool,
        shared: Arc<AudioShared>,
        session: Arc<Shared>,
    ) -> std::io::Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let (direct, queued) = if direct_submit {
            (Some(renderer), None)
        } else {
            (None, Some(renderer))
        };
        let socket = socket.try_clone()?;
        let mut stream = Self {
            shared: shared.clone(),
            stop: stop.clone(),
            receive: None,
            decode: None,
        };
        let receive_shared = shared.clone();
        let decode_session = session.clone();
        stream.receive = Some(std::thread::Builder::new().name("AudioRecv".into()).spawn(
            move || {
                session.prioritize_thread();
                receive_thread(
                    socket,
                    next_socket,
                    receiver,
                    direct,
                    receive_shared,
                    session,
                    stop,
                )
            },
        )?);
        if let Some(mut renderer) = queued {
            stream.decode = Some(std::thread::Builder::new().name("AudioDec".into()).spawn(
                move || {
                    decode_session.prioritize_thread();
                    let mut played = None;
                    while let Some(item) = shared.queue.wait_returning(played.take()) {
                        renderer.play(item.as_deref());
                        played = item;
                    }
                    renderer
                },
            )?);
        }
        Ok(stream)
    }

    /// Stops and joins the threads and returns the renderer for `stop` and `cleanup`.
    pub fn stop(mut self) -> Option<Box<dyn AudioRenderer>> {
        self.stop.store(true, Ordering::Release);
        self.shared.queue.shutdown();
        let mut renderer = self.receive.take().and_then(|t| t.join().ok()).flatten();
        if let Some(thread) = self.decode.take() {
            renderer = thread.join().ok().or(renderer);
        }
        renderer
    }
}

fn receive_thread(
    mut socket: UdpSocket,
    next_socket: Arc<NextSocket>,
    mut receiver: AudioReceiver,
    mut direct: Option<Box<dyn AudioRenderer>>,
    shared: Arc<AudioShared>,
    session: Arc<Shared>,
    stop: Arc<AtomicBool>,
) -> Option<Box<dyn AudioRenderer>> {
    let mut buffer = vec![0u8; 65536];
    let mut received = false;
    let mut jitter: Option<ArrivalJitter> = None;
    let mut trace = PacketTrace::from_env(Stream::Audio);
    while !stop.load(Ordering::Acquire) {
        if let Some(trace) = trace.as_mut() {
            trace.waiting();
        }
        let length = match net::receive(&socket, &mut buffer) {
            Ok(Some(length)) => length,
            Ok(None) => {
                if let Some(next) = next_socket.take() {
                    session.log("Audio moved to the roam's new socket");
                    socket = next;
                }
                receiver.receive_timeout();
                continue;
            }
            Err(e) => {
                // The old socket's address went away with the network, maybe ahead of the
                // rebind: wait a while for the roam's new socket, then fail.
                if let Some(next) = next_socket.wait(net::REBIND_WAIT, &stop) {
                    session.log(&format!(
                        "Audio receive failed on the old socket, moved to the roam's new one: {e}"
                    ));
                    socket = next;
                    continue;
                }
                if stop.load(Ordering::Acquire) {
                    break;
                }
                session.log(&format!("Audio receive failed: {e}"));
                session.terminate(e.raw_os_error().unwrap_or(-1));
                break;
            }
        };
        session.received(length);
        if !received && length >= 12 {
            received = true;
            session.log("Received first audio packet");
        }
        let now = now_us();
        if let Some(trace) = trace.as_mut() {
            trace.push(now, &buffer[..length]);
        }
        receiver.receive(&buffer[..length], now, &mut |output| {
            shared.packets.fetch_add(1, Ordering::Relaxed);
            let lost = matches!(output, AudioOutput::Lost);
            let jitter = jitter.get_or_insert_with(|| {
                ArrivalJitter::new(shared.packet_ms.load(Ordering::Relaxed))
            });
            if let Some(line) = jitter.packet(now, lost) {
                session.log(&line);
            }
            let item = match output {
                AudioOutput::Packet(packet) => Some(packet),
                AudioOutput::Lost => None,
            };
            match direct.as_mut() {
                Some(renderer) => renderer.play(item),
                None => {
                    shared.queue.offer_copy(item);
                }
            }
        });
        *lock(&shared.stats) = receiver.stats();
    }
    if let Some(trace) = trace {
        let session = session.clone();
        trace.finish(move |line| session.log(line));
    }
    direct
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_queue_is_flushed_before_the_next_packet() {
        let queue = PacketQueue::default();
        for i in 0..AUDIO_QUEUE_BOUND {
            assert!(queue.offer(Some(vec![i as u8])));
        }
        assert_eq!(queue.len(), AUDIO_QUEUE_BOUND);
        queue.offer(None);
        assert_eq!(queue.len(), 1);
        assert_eq!(queue.overflows.load(Ordering::Relaxed), 1);
        assert_eq!(queue.wait(), Some(None));
        queue.shutdown();
        assert_eq!(queue.wait(), None);
    }

    /// Copied packets reuse the buffers of played ones, flushed ones included, and a lost packet
    /// stays a lost one.
    #[test]
    fn copied_packets_reuse_the_buffers_of_played_ones() {
        let queue = PacketQueue::default();
        assert!(queue.offer_copy(Some(&[1u8, 2, 3][..])));
        let first = queue.wait_returning(None).expect("an item").expect("a packet");
        assert_eq!(first, [1, 2, 3]);
        let address = first.as_ptr();
        let capacity = first.capacity();
        // Played (given back with the next wait): the next copy fills the same buffer.
        assert!(queue.offer_copy(None));
        assert_eq!(queue.wait_returning(Some(first)), Some(None), "the lost packet");
        assert!(queue.offer_copy(Some(&[4u8, 5][..])));
        let second = queue.wait_returning(None).expect("an item").expect("a packet");
        assert_eq!(second, [4, 5]);
        assert_eq!(
            (second.as_ptr(), second.capacity()),
            (address, capacity),
            "reused, not allocated"
        );
        // A full queue's flushed buffers are spares too.
        for i in 0..AUDIO_QUEUE_BOUND {
            assert!(queue.offer_copy(Some(&[i as u8; 8][..])));
        }
        assert!(queue.offer_copy(Some(&[9u8; 8][..])));
        assert_eq!(queue.overflows.load(Ordering::Relaxed), 1);
        assert_eq!(queue.len(), 1);
        assert!(lock(&queue.state).spare.len() <= SPARE_BOUND);
        assert!(!lock(&queue.state).spare.is_empty(), "flushed buffers kept");
        assert_eq!(queue.wait_returning(None), Some(Some(vec![9; 8])));
        queue.shutdown();
        assert!(!queue.offer_copy(Some(&[1u8][..])));
        assert_eq!(queue.wait_returning(None), None);
    }

    /// Pairs of 5 ms packets every 10 ms read as 5 ms late, a 30 ms stall as up to 30 ms, and
    /// the window's line counts the packets beyond each reserve.
    #[test]
    fn arrival_jitter_reads_paired_packets_and_a_stall() {
        let mut jitter = ArrivalJitter::new(5);
        let mut lines = Vec::new();
        for pair in 0..1002u64 {
            // A 30 ms stall: pairs 500 to 502 come with pair 503.
            let sent = if (500..503).contains(&pair) {
                503
            } else {
                pair
            };
            for _ in 0..2 {
                lines.extend(jitter.packet(1_000_000 + sent * 10_000, false));
            }
        }
        // A lost packet keeps its place and is not an arrival.
        assert_eq!(jitter.packet(1_000_000 + 1002 * 10_000, true), None);
        // A clean window, pairs only, is not logged (after the lost packet's pair, lost too).
        assert_eq!(jitter.packet(1_000_000 + 1002 * 10_000, true), None);
        let mut lines2 = Vec::new();
        for pair in 1003..2004u64 {
            for _ in 0..2 {
                lines2.extend(jitter.packet(1_000_000 + pair * 10_000, false));
            }
        }
        assert!(lines2.is_empty());
        // A 2 s pause starts the window again: its line covers only the packets after it.
        let base = 1_000_000 + 2003 * 10_000 + 2_000_000;
        let mut lines3 = Vec::new();
        for pair in 0..1002u64 {
            let sent = if (10..13).contains(&pair) { 13 } else { pair };
            for _ in 0..2 {
                lines3.extend(jitter.packet(base + sent * 10_000, false));
            }
        }
        assert_eq!(lines3.len(), 1);
        assert!(lines3[0].starts_with("Audio arrival over 10 s: 2001 packets"));
        assert_eq!(
            lines,
            [
                "Audio arrival over 10 s: 2001 packets, late p50 5 p95 5 p99 5 max 35 ms, \
              >=8:6 >=12:5 >=16:4 >=20:4 >=24:3 >=32:1 >=40:0"
            ]
        );
    }

    /// nightjar#74, as a 40 s trace showed it with Windows' audio engine under the host: pairs
    /// every 10 ms, and about twice a second two pairs a slot early (one with the pair before it,
    /// a wake of four), then 20 ms of nothing. No window was clean against the earliest packets:
    /// late p50 9 p95 14 p99 14 max 15 ms, >=8:1960 >=12:980. Early packets say nothing about the
    /// others; a real stall on top still shows.
    #[test]
    fn arrival_jitter_is_not_thrown_by_packets_that_come_early() {
        let arrivals = |stall: bool| {
            let mut jitter = ArrivalJitter::new(5);
            let mut lines = Vec::new();
            for pair in 0..3000u64 {
                let slot = match pair {
                    p if p % 50 == 20 || p % 50 == 21 => p - 1,
                    // A 30 ms stall: pairs 1500 to 1502 come with pair 1503.
                    1500..=1502 if stall => 1503,
                    p => p,
                };
                for _ in 0..2 {
                    lines.extend(jitter.packet(1_000_000 + slot * 10_000, false));
                }
            }
            lines
        };
        assert_eq!(arrivals(false), Vec::<String>::new());
        let lines = arrivals(true);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(
            lines[0].contains("p95 5 p99 5 max 35 ms, >=8:6 >=12:5 "),
            "{lines:?}"
        );
    }

    /// The pending duration counts each item as the packet the renderer gets, not the SDP's
    /// Opus duration: 32 ms AC3 frames and 1 ms 7.1 PCM packets, 5 ms Opus as before.
    #[test]
    fn pending_audio_counts_the_codec_packet_duration() {
        use crate::audio::passthrough::packet_ms;
        use rubylight_protocol::audio_ext::{CODEC_AC3, CODEC_EAC3, CODEC_OPUS, CODEC_PCM};
        for (codec, channels, expected) in [
            (CODEC_AC3, 6, 4 * 32),
            (CODEC_EAC3, 2, 4 * 32),
            (CODEC_PCM, 8, 4),
            (CODEC_PCM, 6, 4 * 2),
            (CODEC_OPUS, 2, 4 * 5),
            (CODEC_OPUS, 8, 4 * 5),
        ] {
            let shared = AudioShared::default();
            assert_eq!(shared.pending_ms(), 0);
            shared
                .packet_ms
                .store(packet_ms(codec, channels, 5), Ordering::Relaxed);
            for _ in 0..4 {
                shared.queue.offer(Some(vec![0]));
            }
            assert_eq!(
                shared.pending_ms(),
                expected,
                "codec {codec}, {channels} channels"
            );
        }
    }
}
