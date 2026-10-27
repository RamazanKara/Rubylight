//! Where the encoded audio goes. `Media::audio` captures and encodes the same way for both
//! transports; only the last step differs: GameStream wraps each Opus packet in RTP with FEC and
//! sends it to the client's audio port, WebRTC hands it to the media thread's Opus track.

use super::qos::Tagged;
use crate::webrtc::media::{AudioPacket, AudioSender, Submit};
use anyhow::Result;
use butterpollo_core::{audio::HostLoss, packet::AudioPacketizer};
use butterpollo_windows::{audio::Loopback, net::ReplySocket};
use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

pub(super) trait AudioSink {
    /// Whether the far end is known. Audio is encoded either way (the capture must be drained),
    /// but a sink that never becomes ready ends the audio when the ping window passes.
    fn ready(&mut self) -> bool;
    /// Send one encoded Opus packet. Returns how many packets did not go out, which the caller
    /// counts as loss; only a broken transport is an error. `position` is where the packet starts
    /// in the audio thread's stream, in 48 kHz samples, counting every packet and any audio the
    /// endpoint lost, whether or not it was sent.
    fn send(&mut self, opus: &[u8], captured: Instant, position: u64) -> Result<usize>;
}

/// Where the audio thread is in its stream, in 48 kHz samples. It counts what the thread encoded
/// and sent, or would have sent, and the audio that never became a packet (Windows dropped it,
/// this host trimmed it, or the thread was too late to make silence for it), so a receiver
/// placing packets by their position hears a gap exactly where audio was lost and nowhere else.
/// It never reads a clock: the sound card's drifts against wall time, and a stall that lost
/// nothing is no gap.
#[derive(Default)]
pub(super) struct AudioClock {
    position: u64,
    /// The idle endpoint's silence has started: its first packet already counted the wait.
    silent: bool,
}

impl AudioClock {
    const RATE: u128 = 48_000;

    /// The start of the next packet of `frames` samples; the clock moves past it, sent or not.
    pub(super) fn packet(&mut self, frames: usize) -> u64 {
        let at = self.position;
        self.position += frames as u64;
        at
    }

    /// `span` as 48 kHz samples. Whole nanoseconds, so 90 ms is 4320 samples and not 4319.99.
    fn samples(span: Duration) -> u64 {
        (span.as_nanos() * Self::RATE / 1_000_000_000) as u64
    }

    /// `lost` of audio that never became a packet, before the packets about to be read. Returns
    /// it in samples.
    pub(super) fn lost(&mut self, lost: Duration) -> u64 {
        let samples = Self::samples(lost);
        self.position += samples;
        samples
    }

    /// The same, counted in 48 kHz samples.
    pub(super) fn lost_samples(&mut self, samples: u64) {
        self.position += samples;
    }

    /// Audio arrived: the endpoint is no longer idle.
    pub(super) fn heard(&mut self) {
        self.silent = false;
    }

    /// The first silence packet of an idle endpoint is due `waited` after the last audio. The
    /// time before it carried no packets, and is part of the stream. Returns whether this was
    /// that first packet.
    /// `counted` samples of that wait were already added as lost, and are not added twice.
    pub(super) fn idle(&mut self, waited: Duration, counted: u64) -> bool {
        let first = !std::mem::replace(&mut self.silent, true);
        if first {
            self.position += Self::samples(waited).saturating_sub(counted);
        }
        first
    }
}

/// How long an endpoint must be quiet before silence stands in for it. An endpoint capture keeps
/// delivering through silence (its keep-alive render stream); this covers game-only capture and a
/// capture that is gone.
const IDLE: Duration = Duration::from_millis(50);

/// What the audio thread decides each pass, apart from the sound card and the socket: when the
/// next packet is due, when silence replaces a quiet endpoint, and where each packet sits in the
/// stream. Its numbers go to the sink before anything is sent, so a packet the sink drops or the
/// peer never receives still holds its place.
pub(super) struct AudioPace {
    clock: AudioClock,
    interval: Duration,
    frames: usize,
    /// When audio last arrived.
    heard: Instant,
    /// When an idle endpoint's next silence is due.
    next: Instant,
    /// Samples counted as lost since audio last arrived. An idle endpoint's first silence
    /// counts the wait since then, and must not count these twice.
    counted: u64,
    /// When the capture went away, until audio or silence comes after it. The time without a
    /// capture is lost even when it is shorter than `IDLE`.
    gap: Option<Instant>,
}

impl AudioPace {
    pub(super) fn new(now: Instant, interval: Duration, frames: usize) -> Self {
        Self {
            clock: AudioClock::default(),
            interval,
            frames,
            heard: now,
            next: now,
            counted: 0,
            gap: None,
        }
    }

    /// When the thread has something to do: the packet deadline, but not before the endpoint has
    /// been quiet for long enough to need silence.
    pub(super) fn due(&self) -> Instant {
        self.next.max(self.heard + IDLE)
    }

    /// Windows reported `skipped` of audio it had no room for. `lost` says whether the read
    /// that found it was late enough to explain it; a skip nothing explains is the endpoint
    /// falling quiet and starting again, and is no gap.
    pub(super) fn windows_dropped(&mut self, lost: bool, skipped: Duration) {
        if lost {
            self.counted += self.clock.lost(skipped);
        }
    }

    /// This host discarded `samples` (48 kHz) of audio Windows had delivered.
    pub(super) fn host_dropped(&mut self, samples: u64) {
        self.clock.lost_samples(samples);
        self.counted += samples;
    }

    /// The capture went away at `now` (a reopen, a failed read). Until audio or silence follows,
    /// the time passing is lost.
    pub(super) fn capture_closed(&mut self, now: Instant) {
        self.gap.get_or_insert(now);
    }

    /// A capture delivered audio at `now`: the time since the last one went away was lost.
    pub(super) fn capture_delivered(&mut self, now: Instant) {
        if let Some(at) = self.gap.take() {
            self.clock.lost(now.saturating_duration_since(at));
        }
    }

    /// Numbers the packets read at `now`, in order. With none, and the endpoint quiet for 50 ms,
    /// one packet of `silence` is made for it. Returns each packet's position in the stream.
    pub(super) fn stamp(
        &mut self,
        now: Instant,
        packets: &mut Vec<Vec<f32>>,
        silence: &[f32],
    ) -> Vec<u64> {
        if !packets.is_empty() {
            self.counted = 0;
            self.heard = now;
            self.next = now + self.interval;
            self.clock.heard();
        } else if now >= self.next && now.saturating_duration_since(self.heard) >= IDLE {
            // WASAPI delivers roughly 10 ms chunks; allow for scheduling jitter
            // before treating a quiet capture queue as an idle endpoint.
            // Silence counts the whole wait since the last audio, which covers a gap.
            self.gap = None;
            let first = self
                .clock
                .idle(now.saturating_duration_since(self.heard), self.counted);
            self.counted = 0;
            packets.push(silence.to_vec());
            self.next += self.interval;
            if self.next < now {
                // One packet covers one interval; the rest of the late time (a blocked device
                // open, a reopen) is gone from the stream. The first packet's wait is already
                // counted by `idle`.
                if !first {
                    self.clock.lost(now - self.next);
                }
                self.next = now + self.interval;
            }
        }
        packets
            .iter()
            .map(|_| self.clock.packet(self.frames))
            .collect()
    }
}

/// What the audio thread needs from the sound card's capture; `Loopback` is the real one.
pub(super) trait AudioCapture {
    fn since_drained(&mut self) -> Option<Duration>;
    fn read(&mut self, frames: usize) -> Result<Option<Vec<f32>>>;
    fn take_delivered(&mut self) -> Option<Duration>;
    fn buffer(&self) -> Duration;
    /// Audio this host discarded after Windows delivered it, in 48 kHz samples.
    fn take_trimmed(&mut self) -> u64;
    /// The same, plus whatever was still queued: what a capture that is being dropped loses.
    fn take_discarded(&mut self) -> u64;
}

impl AudioCapture for Loopback {
    fn since_drained(&mut self) -> Option<Duration> {
        Loopback::since_drained(self)
    }
    fn read(&mut self, frames: usize) -> Result<Option<Vec<f32>>> {
        Loopback::read(self, frames)
    }
    fn take_delivered(&mut self) -> Option<Duration> {
        Loopback::take_delivered(self)
    }
    fn buffer(&self) -> Duration {
        Loopback::buffer(self)
    }
    fn take_trimmed(&mut self) -> u64 {
        Loopback::take_trimmed(self)
    }
    fn take_discarded(&mut self) -> u64 {
        Loopback::take_discarded(self)
    }
}

/// A capture is going away (a failed read, a sink change, a game-capture swap): its Windows
/// skip, the audio this host trimmed and whatever it still queued are lost, and so is the time
/// until the next capture delivers. `waited` is its `since_drained`.
pub(super) fn retire<C: AudioCapture>(
    old: &mut C,
    waited: Option<Duration>,
    loss: &mut HostLoss,
    pace: &mut AudioPace,
    now: Instant,
) {
    let queued = retire_read(old, waited, loss, pace, now);
    pace.host_dropped(queued);
}

/// The part of [`retire`] that precedes whatever the capture handed over: its Windows skip and
/// the audio this host trimmed. Returns the audio it still queued, which follows what was read.
fn retire_read<C: AudioCapture>(
    old: &mut C,
    waited: Option<Duration>,
    loss: &mut HostLoss,
    pace: &mut AudioPace,
    now: Instant,
) -> u64 {
    let delivered = old.take_delivered();
    if let Some(waited) = waited {
        let lost = loss.read(waited, delivered, old.buffer());
        pace.windows_dropped(lost, delivered.unwrap_or_default());
    }
    pace.host_dropped(old.take_trimmed());
    pace.capture_closed(now);
    old.take_discarded()
}

/// One pass of the audio thread's read side: every whole packet the capture has, with where each
/// sits in the stream, or one packet of `silence` for a quiet endpoint. What Windows skipped,
/// what this host trimmed and a capture that fails are all counted before the positions are
/// made. `interrupted` hears each read error; the capture is then gone.
pub(super) fn collect<C: AudioCapture>(
    capture: &mut Option<C>,
    frames: usize,
    loss: &mut HostLoss,
    pace: &mut AudioPace,
    silence: &[f32],
    now: impl Fn() -> Instant,
    mut interrupted: impl FnMut(&anyhow::Error),
) -> (Vec<Vec<f32>>, Vec<u64>) {
    let mut packets = Vec::new();
    // What a failed capture still queued: it comes after the packets it handed over first.
    let mut queued = 0;
    let waited = capture.as_mut().and_then(AudioCapture::since_drained);
    while let Some(capturing) = capture.as_mut() {
        match capturing.read(frames) {
            Ok(Some(samples)) => packets.push(samples),
            Ok(None) => break,
            Err(error) => {
                interrupted(&error);
                queued += retire_read(capturing, waited, loss, pace, now());
                *capture = None;
            }
        }
    }
    if let Some(capturing) = capture.as_mut() {
        let delivered = capturing.take_delivered();
        if let Some(waited) = waited {
            let lost = loss.read(waited, delivered, capturing.buffer());
            // A skip no late read explains is the endpoint falling quiet and starting again.
            let skipped = delivered.unwrap_or_default();
            // Windows dropped this audio: the RTP clock keeps its length as a gap for the
            // receiver to conceal.
            pace.windows_dropped(lost, skipped);
            if waited > capturing.buffer() || !skipped.is_zero() {
                tracing::debug!(
                    waited_ms = waited.as_secs_f64() * 1000.,
                    skipped_ms = skipped.as_secs_f64() * 1000.,
                    delivered = delivered.is_some(),
                    lost,
                    "audio sender read late or Windows skipped audio"
                );
            }
        }
        // Audio the host itself trimmed after a late wake was delivered, so the skip count
        // above does not hold it; the stream still lost its time.
        pace.host_dropped(capturing.take_trimmed());
    }
    // Packets read before a failure belong to the capture that is gone: they do not end its gap.
    let at = now();
    if capture.is_some() && !packets.is_empty() {
        pace.capture_delivered(at);
    }
    let positions = pace.stamp(at, &mut packets, silence);
    pace.host_dropped(queued);
    (packets, positions)
}

/// RTP with FEC on the client's audio port.
pub(super) struct GameStreamAudio<'a> {
    socket: &'a Arc<ReplySocket>,
    peers: &'a Mutex<HashMap<(String, bool), SocketAddr>>,
    launch: &'a str,
    packetizer: AudioPacketizer,
    qos: Tagged,
    use_qos: bool,
    peer: Option<SocketAddr>,
}

impl<'a> GameStreamAudio<'a> {
    pub(super) fn new(
        socket: &'a Arc<ReplySocket>,
        peers: &'a Mutex<HashMap<(String, bool), SocketAddr>>,
        launch: &'a str,
        packetizer: AudioPacketizer,
        use_qos: bool,
    ) -> Self {
        Self {
            socket,
            peers,
            launch,
            packetizer,
            qos: Tagged::default(),
            use_qos,
            peer: None,
        }
    }
}

impl AudioSink for GameStreamAudio<'_> {
    fn ready(&mut self) -> bool {
        self.peer = self
            .peers
            .lock()
            .unwrap()
            .get(&(self.launch.to_owned(), true))
            .copied();
        if let (Some(peer), true) = (self.peer, self.use_qos) {
            self.qos.follow(self.socket, peer, true);
        }
        self.peer.is_some()
    }

    fn send(&mut self, opus: &[u8], _captured: Instant, _position: u64) -> Result<usize> {
        let Some(peer) = self.peer else {
            return Ok(1);
        };
        let mut unsent = 0;
        for packet in self.packetizer.encode(opus)? {
            // A lost audio packet is concealed by the client; only a broken socket stops the audio.
            if !butterpollo_windows::net::send_datagram(self.socket, &packet, peer)? {
                unsent += 1;
            }
        }
        Ok(unsent)
    }
}

/// The WebRTC Opus track. The media thread sends nothing until the peer connects, and a full
/// queue drops the packet, which Opus conceals; neither is an error here.
pub(super) struct WebRtcAudio(pub(super) AudioSender);

impl AudioSink for WebRtcAudio {
    fn ready(&mut self) -> bool {
        true
    }

    fn send(&mut self, opus: &[u8], captured: Instant, position: u64) -> Result<usize> {
        Ok(
            match self.0.submit(AudioPacket {
                data: opus.to_vec(),
                capture: captured,
                position,
            }) {
                Submit::Queued => 0,
                Submit::Dropped => 1,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::UdpSocket;

    #[test]
    fn the_audio_clock_counts_packets_sent_or_not_and_what_windows_dropped() {
        let mut clock = AudioClock::default();
        assert_eq!(clock.packet(240), 0);
        assert_eq!(clock.packet(240), 240);
        // 40 ms dropped before the next read is 1920 samples of gap, no more and no less.
        clock.lost(Duration::from_millis(40));
        assert_eq!(clock.packet(240), 2400);
        // A hole the size of one sample at 48 kHz rounds down, never up past the audio.
        clock.lost(Duration::from_nanos(20_000));
        assert_eq!(clock.packet(240), 2640);
    }

    fn audio(count: usize) -> Vec<Vec<f32>> {
        vec![vec![0.5; 480]; count]
    }

    #[test]
    fn a_late_wake_that_trims_its_backlog_leaves_a_gap_as_long_as_the_trim() {
        let base = Instant::now();
        let at = |ms| base + Duration::from_millis(ms);
        let mut pace = AudioPace::new(base, Duration::from_millis(5), 240);
        assert_eq!(pace.stamp(at(0), &mut audio(1), &[0.; 480]), [0]);
        // The thread wakes 100 ms late. 100 ms of audio is waiting, the capture keeps the newest
        // 10 ms and reports the 90 ms it dropped (4320 samples), then two packets are read.
        pace.host_dropped(90 * 48);
        let positions = pace.stamp(at(105), &mut audio(2), &[0.; 480]);
        assert_eq!(positions, [240 + 4320, 240 + 4320 + 240]);
        // The stream is back in step with the wall: its last packet ends at 105 ms, where the
        // thread woke, and the next pass carries on from there.
        assert_eq!(240 + 4320 + 2 * 240, 105 * 48);
        assert_eq!(pace.stamp(at(110), &mut audio(1), &[0.; 480]), [105 * 48]);
    }

    #[test]
    fn a_wake_that_trimmed_nothing_leaves_the_positions_contiguous() {
        let base = Instant::now();
        let at = |ms| base + Duration::from_millis(ms);
        let mut pace = AudioPace::new(base, Duration::from_millis(5), 240);
        assert_eq!(pace.stamp(at(0), &mut audio(1), &[0.; 480]), [0]);
        pace.host_dropped(0);
        // A 25 ms late wake under the trim limit: five packets at once, no gap.
        let positions = pace.stamp(at(30), &mut audio(5), &[0.; 480]);
        assert_eq!(positions, [240, 480, 720, 960, 1200]);
    }

    #[test]
    fn windows_drops_move_the_clock_only_when_a_late_read_explains_them() {
        let base = Instant::now();
        let mut pace = AudioPace::new(base, Duration::from_millis(5), 240);
        // The endpoint falling quiet and starting again: Windows skipped, nothing was lost.
        pace.windows_dropped(false, Duration::from_millis(40));
        assert_eq!(pace.stamp(base, &mut audio(1), &[0.; 480]), [0]);
        pace.windows_dropped(true, Duration::from_millis(40));
        assert_eq!(pace.stamp(base, &mut audio(1), &[0.; 480]), [240 + 1920]);
    }

    #[test]
    fn packets_hold_their_place_whether_or_not_they_are_sent() {
        let base = Instant::now();
        let mut pace = AudioPace::new(base, Duration::from_millis(5), 240);
        // Three packets are numbered together before any is sent; dropping them upstream, or
        // having no peer, changes nothing about the next pass.
        let mut packets = audio(3);
        assert_eq!(pace.stamp(base, &mut packets, &[0.; 480]), [0, 240, 480]);
        assert_eq!(packets.len(), 3);
        assert_eq!(pace.stamp(base, &mut audio(2), &[0.; 480]), [720, 960]);
    }

    #[test]
    fn an_idle_endpoints_silence_is_paced_and_counts_a_late_pass() {
        let base = Instant::now();
        let at = |ms| base + Duration::from_millis(ms);
        let silence = [0.; 480];
        let mut pace = AudioPace::new(base, Duration::from_millis(5), 240);
        assert_eq!(pace.stamp(at(0), &mut audio(1), &silence), [0]);
        // Quiet for less than 50 ms: no silence yet, and nothing is numbered.
        for ms in [10, 20, 45] {
            let mut packets = Vec::new();
            assert!(pace.stamp(at(ms), &mut packets, &silence).is_empty());
            assert!(packets.is_empty(), "{ms} ms");
        }
        assert_eq!(pace.due(), at(50));
        // The first silence is due after 52 ms. Its wait is in the stream once (52 ms after the
        // 5 ms packet), and the pacing reset is not counted again on top of it.
        let mut packets = Vec::new();
        assert_eq!(pace.stamp(at(52), &mut packets, &silence), [240 + 2496]);
        assert_eq!(packets, [silence.to_vec()]);
        // Then one packet every 5 ms, contiguous.
        assert_eq!(pace.stamp(at(57), &mut Vec::new(), &silence), [2976]);
        assert_eq!(pace.stamp(at(62), &mut Vec::new(), &silence), [3216]);
        // A pass 33 ms late (a blocked device open, say): its packet ends where the thread woke,
        // 100 ms in, and the 28 ms the pacing missed before it are a gap.
        assert_eq!(
            pace.stamp(at(100), &mut Vec::new(), &silence),
            [3456 + 1344]
        );
        assert_eq!(pace.stamp(at(105), &mut Vec::new(), &silence), [5040]);
        // Audio returns; the next quiet spell counts its own wait.
        assert_eq!(pace.stamp(at(110), &mut audio(1), &silence), [5280]);
        assert_eq!(
            pace.stamp(at(170), &mut Vec::new(), &silence),
            [5520 + 60 * 48]
        );
    }

    #[test]
    fn an_idle_endpoints_first_silence_counts_the_wait_once() {
        let mut clock = AudioClock::default();
        assert_eq!(clock.packet(240), 0);
        clock.heard();
        // 50 ms of nothing, then silence every 5 ms: the wait is in the stream once.
        clock.idle(Duration::from_millis(50), 0);
        assert_eq!(clock.packet(240), 2640);
        clock.idle(Duration::from_millis(55), 0);
        clock.idle(Duration::from_millis(60), 0);
        assert_eq!(clock.packet(240), 2880);
        // Audio returns, and the next idle spell counts its own wait.
        clock.heard();
        assert_eq!(clock.packet(240), 3120);
        clock.idle(Duration::from_millis(50), 0);
        assert_eq!(clock.packet(240), 5760);
    }

    /// A scripted sound card: what each read returns, and what it reports about the time since
    /// the last one.
    #[derive(Default)]
    struct Fake {
        reads: std::collections::VecDeque<std::result::Result<Option<Vec<f32>>, &'static str>>,
        waited: Option<Duration>,
        delivered: Option<Duration>,
        trimmed: u64,
        queued: u64,
    }

    impl Fake {
        fn playing(packets: usize) -> Self {
            Self {
                reads: (0..packets).map(|_| Ok(Some(vec![0.5; 480]))).collect(),
                ..Self::default()
            }
        }
    }

    impl AudioCapture for Fake {
        fn since_drained(&mut self) -> Option<Duration> {
            self.waited
        }
        fn read(&mut self, _: usize) -> Result<Option<Vec<f32>>> {
            match self.reads.pop_front() {
                Some(Ok(samples)) => Ok(samples),
                Some(Err(error)) => Err(anyhow::anyhow!(error)),
                None => Ok(None),
            }
        }
        fn take_delivered(&mut self) -> Option<Duration> {
            self.delivered.take()
        }
        fn buffer(&self) -> Duration {
            Duration::from_millis(20)
        }
        fn take_trimmed(&mut self) -> u64 {
            std::mem::take(&mut self.trimmed)
        }
        fn take_discarded(&mut self) -> u64 {
            self.take_trimmed() + std::mem::take(&mut self.queued)
        }
    }

    struct Thread {
        base: Instant,
        pace: AudioPace,
        loss: HostLoss,
        errors: Vec<String>,
    }

    impl Thread {
        fn new() -> Self {
            let base = Instant::now();
            Self {
                base,
                pace: AudioPace::new(base, Duration::from_millis(5), 240),
                loss: HostLoss::default(),
                errors: Vec::new(),
            }
        }

        fn at(&self, ms: u64) -> Instant {
            self.base + Duration::from_millis(ms)
        }

        /// One pass of the audio thread at `ms`: the positions of what it sends.
        fn pass(&mut self, capture: &mut Option<Fake>, ms: u64) -> Vec<u64> {
            let now = self.at(ms);
            let errors = &mut self.errors;
            let (_, positions) = collect(
                capture,
                240,
                &mut self.loss,
                &mut self.pace,
                &[0.; 480],
                || now,
                |error| errors.push(error.to_string()),
            );
            positions
        }
    }

    #[test]
    fn the_audio_loop_counts_a_windows_skip_and_the_hosts_trim_before_numbering() {
        let mut audio = Thread::new();
        let mut capture = Some(Fake::playing(1));
        assert_eq!(audio.pass(&mut capture, 0), [0]);
        // A read 105 ms later finds 80 ms Windows skipped (explained by the late read, the sound
        // card keeps 20 ms) and 10 ms this host trimmed: two packets, 3840 + 480 samples after
        // the first packet.
        let card = capture.as_mut().unwrap();
        card.reads.extend(Fake::playing(2).reads);
        card.waited = Some(Duration::from_millis(105));
        card.delivered = Some(Duration::from_millis(80));
        card.trimmed = 480;
        assert_eq!(
            audio.pass(&mut capture, 105),
            [240 + 3840 + 480, 240 + 3840 + 480 + 240]
        );
    }

    #[test]
    fn a_stall_that_delivers_less_than_a_packet_counts_its_wait_once() {
        let mut audio = Thread::new();
        let mut capture = Some(Fake::playing(1));
        assert_eq!(audio.pass(&mut capture, 0), [0]);
        // 100 ms stall: Windows dropped 80 ms of it, and what remained is under one packet. The
        // silence the quiet endpoint gets covers the rest, not the 80 ms again: the packet ends
        // where the thread woke.
        let card = capture.as_mut().unwrap();
        card.waited = Some(Duration::from_millis(100));
        card.delivered = Some(Duration::from_millis(80));
        assert_eq!(audio.pass(&mut capture, 100), [240 + 100 * 48]);
    }

    #[test]
    fn an_idle_endpoints_silence_still_counts_its_wait_when_nothing_else_did() {
        let mut audio = Thread::new();
        let mut capture = Some(Fake::playing(1));
        assert_eq!(audio.pass(&mut capture, 0), [0]);
        capture.as_mut().unwrap().waited = Some(Duration::from_millis(80));
        assert_eq!(audio.pass(&mut capture, 80), [240 + 80 * 48]);
    }

    #[test]
    fn a_failed_capture_counts_what_it_lost_and_the_time_until_the_next_one() {
        let mut audio = Thread::new();
        let mut capture = Some(Fake::playing(0));
        capture
            .as_mut()
            .unwrap()
            .reads
            .extend([Ok(Some(vec![0.5; 480])), Err("the endpoint went away")]);
        let card = capture.as_mut().unwrap();
        card.waited = Some(Duration::from_millis(100));
        // 80 ms skipped by Windows (a late read explains it), 10 ms trimmed, 20 ms still queued.
        card.delivered = Some(Duration::from_millis(80));
        card.trimmed = 480;
        card.queued = 960;
        let positions = audio.pass(&mut capture, 0);
        assert!(capture.is_none());
        assert_eq!(audio.errors, ["the endpoint went away"]);
        // The packet read before the failure sits after the skip and the trim, which happened
        // before it, and before the 20 ms the capture still queued, which was never read.
        assert_eq!(positions, [3840 + 480]);
        // 30 ms later a reopened capture delivers: the time between, under the idle limit and
        // sent no silence, is lost as well.
        let mut capture = Some(Fake::playing(1));
        assert_eq!(
            audio.pass(&mut capture, 30),
            [3840 + 480 + 960 + 240 + 30 * 48]
        );
        // Counted once: the next packet follows it.
        capture
            .as_mut()
            .unwrap()
            .reads
            .extend(Fake::playing(1).reads);
        assert_eq!(
            audio.pass(&mut capture, 35),
            [3840 + 480 + 960 + 240 + 30 * 48 + 240]
        );
    }

    #[test]
    fn a_capture_dropped_for_a_new_sink_counts_its_residue_and_the_reopen_gap() {
        let mut audio = Thread::new();
        let mut capture = Some(Fake::playing(1));
        assert_eq!(audio.pass(&mut capture, 0), [0]);
        // The sink changes at 10 ms: 40 ms of skip a late read explains, 10 ms trimmed and
        // 20 ms the resampler still held.
        let mut old = Fake {
            waited: Some(Duration::from_millis(60)),
            delivered: Some(Duration::from_millis(40)),
            trimmed: 480,
            queued: 960,
            ..Fake::default()
        };
        let waited = old.since_drained();
        let closed = audio.at(10);
        retire(&mut old, waited, &mut audio.loss, &mut audio.pace, closed);
        // The new capture delivers at 25 ms: the 15 ms without one is lost too.
        let mut capture = Some(Fake::playing(1));
        assert_eq!(
            audio.pass(&mut capture, 25),
            [240 + 1920 + 480 + 960 + 15 * 48]
        );
    }

    #[test]
    fn silence_covers_a_reopen_that_outlasts_the_idle_limit_without_counting_it_twice() {
        let mut audio = Thread::new();
        let mut capture = Some(Fake::playing(1));
        assert_eq!(audio.pass(&mut capture, 0), [0]);
        let mut old = Fake::default();
        let closed = audio.at(10);
        retire(&mut old, None, &mut audio.loss, &mut audio.pace, closed);
        // No capture for 60 ms: the first silence counts the wait since the last audio.
        let mut none: Option<Fake> = None;
        assert_eq!(audio.pass(&mut none, 60), [240 + 60 * 48]);
        // Audio returns: the silence already covered the gap.
        let mut capture = Some(Fake::playing(1));
        assert_eq!(audio.pass(&mut capture, 65), [240 + 60 * 48 + 240]);
    }

    const KEY: [u8; 16] = *b"0123456789abcdef";

    /// What GameStream clients receive for a run of Opus packets, byte for byte: the sink must add
    /// nothing to, and take nothing from, what the packetizer produces.
    #[test]
    fn gamestream_audio_is_the_packetizer_output_on_the_wire() {
        for encrypted in [false, true] {
            let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
            receiver
                .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                .unwrap();
            let socket = Arc::new(ReplySocket::bind("127.0.0.1:0").unwrap());
            let peers = Mutex::new(HashMap::new());
            let mut sink = GameStreamAudio::new(
                &socket,
                &peers,
                "launch",
                AudioPacketizer::new(KEY, 77, encrypted, 5),
                false,
            );
            assert!(!sink.ready(), "no endpoint yet");
            peers
                .lock()
                .unwrap()
                .insert(("launch".into(), true), receiver.local_addr().unwrap());
            assert!(sink.ready());

            let mut reference = AudioPacketizer::new(KEY, 77, encrypted, 5);
            let mut expected = Vec::new();
            for index in 0..9u8 {
                let opus: Vec<u8> = (0..40 + usize::from(index))
                    .map(|i| i as u8 ^ index)
                    .collect();
                expected.extend(reference.encode(&opus).unwrap());
                assert_eq!(sink.send(&opus, Instant::now(), 0).unwrap(), 0);
            }
            // Nine data packets and two FEC groups of two parity packets.
            assert_eq!(expected.len(), 9 + 4);
            let mut buffer = [0u8; 2048];
            for (index, want) in expected.iter().enumerate() {
                let n = receiver.recv(&mut buffer).unwrap();
                assert_eq!(
                    &buffer[..n],
                    want.as_slice(),
                    "packet {index}, encrypted={encrypted}"
                );
            }
        }
    }

    use butterpollo_core::audio::OpusLayout;

    /// 5.1 as Chromium's multiopus decodes it, and as Moonlight's own mapping orders it.
    fn five_one(mapping: [u8; 6]) -> OpusLayout {
        OpusLayout {
            channels: 6,
            streams: 4,
            coupled: 2,
            mapping: mapping.to_vec(),
            bitrate: 256_000,
        }
    }

    fn session() -> Arc<crate::state::Session> {
        let f = crate::state::test_support::Fixture::new();
        f.session(f.client(u32::MAX), butterpollo_core::session::Role::Stream)
    }

    #[test]
    fn the_audio_thread_encodes_the_negotiated_layout_and_else_the_launch_one() {
        let s = session();
        let negotiated = five_one([0, 4, 1, 2, 3, 5]);
        assert_eq!(
            super::super::audio_layout(&s, Some(&negotiated)).unwrap(),
            negotiated
        );
        // Not the launch's own 2-channel default, and not core's Moonlight mapping.
        let other = five_one([0, 1, 4, 5, 2, 3]);
        assert_eq!(super::super::audio_layout(&s, Some(&other)).unwrap(), other);
        assert_eq!(
            super::super::audio_layout(&s, None).unwrap(),
            OpusLayout::select(
                s.config.audio_channels as usize,
                s.config.audio_quality,
                s.launch.options.get("surroundParams").map(String::as_str),
            )
            .unwrap()
        );
    }

    /// A negotiated mapping reaches libopus: the same tone on the same channel encodes to
    /// different packets under two mappings, and to identical ones under the same.
    #[test]
    #[ignore = "requires the packaged libopus DLL (BUTTERPOLLO_TEST_OPUS_ROOT)"]
    fn a_negotiated_mapping_reaches_the_opus_encoder() {
        let root = std::path::PathBuf::from(
            std::env::var_os("BUTTERPOLLO_TEST_OPUS_ROOT").expect("set BUTTERPOLLO_TEST_OPUS_ROOT"),
        );
        let s = session();
        let encode = |layout: &OpusLayout| -> Vec<Vec<u8>> {
            let (mut opus, used) =
                super::super::open_audio_encoder(&root, &s, Some(layout)).unwrap();
            assert_eq!(&used, layout);
            let frames = 48 * usize::from(s.config.audio_packet_ms);
            (0..20)
                .map(|packet| {
                    let samples: Vec<f32> = (0..frames)
                        .flat_map(|frame| {
                            (0..6).map(move |channel| {
                                // Front right carries a tone; the rest stay silent.
                                if channel == 1 {
                                    ((packet * frames + frame) as f32 * 0.07).sin() * 0.4
                                } else {
                                    0.0
                                }
                            })
                        })
                        .collect();
                    opus.encode(&samples).unwrap()
                })
                .collect()
        };
        let chromium = five_one([0, 4, 1, 2, 3, 5]);
        let moonlight = five_one([0, 1, 4, 5, 2, 3]);
        assert_eq!(encode(&chromium), encode(&chromium));
        assert_ne!(encode(&chromium), encode(&moonlight));
    }

    fn bytes(hex: &str) -> Vec<u8> {
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect()
    }

    /// Four fixed Opus packets (`a0 01 02 03` .. `a3 01 02 03`) from key `0123456789abcdef`,
    /// key id 77 and 5 ms packets, as a Moonlight client must receive them. Fixed bytes, so a
    /// change to the constructor, the flags or the packetizer shows here and not only as a
    /// difference between two copies of the same code. RTP: 0x80, payload type 97, sequence,
    /// timestamp (+5 per packet), SSRC 0; the FEC packets (type 127) name the group they cover
    /// and carry the two Reed-Solomon rows over it (0x82 = 0x77*a0 ^ 0x40*a1 ^ 0x38*a2 ^ 0x0e*a3
    /// in GF(256), and 0xa9 for the second row).
    const PLAIN: [&str; 6] = [
        "806100000000000000000000a0010203",
        "806100010000000500000000a1010203",
        "806100020000000a00000000a2010203",
        "806100030000000f00000000a3010203",
        "807f0004000000140000000000610000000000000000000082010203",
        "807f00050000001400000000016100000000000000000000a9010203",
    ];
    /// The same with AES-CBC payloads (IV = key id + sequence, big endian, then zeros).
    const SEALED: [&str; 6] = [
        "80610000000000000000000032b4fab2a3e6aacde5d4ca2f5b308994",
        "8061000100000005000000006623c01143909d9d3bfdfd6bca44cb9e",
        "806100020000000a00000000f1dd481c950a76ce04029c943d175893",
        "806100030000000f000000004b69739cb70aaffcf81d4d3d42e5fb42",
        "807f00040000001400000000006100000000000000000000cdd824dbbdd2b00f460c633313a52ab6",
        "807f000500000014000000000161000000000000000000009c8c956051ffe7293901e7b176084948",
    ];

    #[test]
    fn gamestream_audio_wire_bytes_are_pinned() {
        for (encrypted, qos) in [(false, false), (true, false), (false, true), (true, true)] {
            let want = if encrypted { SEALED } else { PLAIN };
            let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
            receiver
                .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                .unwrap();
            let socket = Arc::new(ReplySocket::bind("127.0.0.1:0").unwrap());
            let peers = Mutex::new(HashMap::new());
            peers
                .lock()
                .unwrap()
                .insert(("launch".into(), true), receiver.local_addr().unwrap());
            let mut sink = GameStreamAudio::new(
                &socket,
                &peers,
                "launch",
                AudioPacketizer::new(KEY, 77, encrypted, 5),
                qos,
            );
            assert!(sink.ready(), "qos={qos}");
            for index in 0..4u8 {
                assert_eq!(
                    sink.send(
                        &[0xa0 + index, 1, 2, 3],
                        Instant::now(),
                        u64::from(index) * 240
                    )
                    .unwrap(),
                    0
                );
            }
            let mut buffer = [0u8; 2048];
            for (index, hex) in want.iter().enumerate() {
                let n = receiver.recv(&mut buffer).unwrap();
                assert_eq!(
                    &buffer[..n],
                    bytes(hex).as_slice(),
                    "packet {index}, encrypted={encrypted}, qos={qos}"
                );
            }
        }
    }
}
