//! Report 0x36 against vectors from an independent CRC-32 (Python's zlib), the resampling and
//! decimation, the Opus packet size, and the worker on a recording HID device.

use super::*;
use std::sync::mpsc::{channel, Receiver, Sender};

#[test]
fn the_checksum_is_crc32_over_the_0xa2_seed() {
    assert_eq!(crc32(0, b"123456789"), 0xCBF4_3926, "the standard check value");
    // The register after the seed byte, as moonlight-qt starts from it.
    assert_eq!(crc32(0, &[0xA2]), 0xEADA_2D49);
}

fn golden(haptics: Option<&[u8; 64]>) -> [u8; REPORT_BYTES] {
    report(0x13, 7, &[0x11; OPUS_BYTES], haptics)
}

#[test]
fn a_report_with_haptics_matches_the_golden_vector() {
    let out = golden(Some(&[0x22; 64]));
    assert_eq!(out[..15], [0x36, 0x30, 0x91, 7, 0xFE, 0x30, 0x30, 0x30, 0x30, 0x30, 7, 0x90, 63, 0xA0, 0x80]);
    assert_eq!((out[18], out[20], out[50]), (0x64, 0x30, 0x02));
    assert_eq!(out[76..78], [0x92, 64]);
    assert!(out[78..142].iter().all(|b| *b == 0x22));
    assert_eq!(out[142..144], [0x93, 200]);
    assert!(out[144..344].iter().all(|b| *b == 0x11));
    assert!(out[344..394].iter().all(|b| *b == 0));
    assert_eq!(u32::from_le_bytes(out[394..].try_into().unwrap()), 0x8720_8C8D);
}

#[test]
fn a_speaker_only_report_moves_the_opus_packet_up() {
    let out = golden(None);
    assert_eq!(out[76..78], [0x93, 200]);
    assert!(out[78..278].iter().all(|b| *b == 0x11));
    assert!(out[278..394].iter().all(|b| *b == 0));
    assert_eq!(u32::from_le_bytes(out[394..].try_into().unwrap()), 0x77E4_4BFD);
}

#[test]
fn resampling_keeps_the_ends_and_interpolates_between() {
    let mut frames = [[0i16; 2]; REPORT_FRAMES];
    for (i, frame) in frames.iter_mut().enumerate() {
        *frame = [i as i16 * 8, -(i as i16) * 8];
    }
    let out = resample(&frames);
    assert_eq!(out[..2], [0, 0]);
    assert_eq!(out[958..], [511 * 8, -511 * 8]);
    // Output frame 240 sits at input 240 * 511 / 479 = 256.03.
    assert_eq!(out[480], 2048);
    assert!(out.as_chunks::<2>().0.iter().all(|frame| frame[0] == -frame[1]));
}

#[test]
fn haptics_go_as_32_signed_bytes_per_channel_at_3_khz() {
    let mut frames = [[0i16; 2]; REPORT_FRAMES];
    for (i, frame) in frames.iter_mut().enumerate() {
        *frame = if i < 16 { [0x4000, -0x4000] } else if i >= 496 { [i16::MAX, i16::MIN] } else { [0, 0] };
    }
    let out = decimate(&frames);
    assert_eq!(out[..2], [0x40, 0xC0]);
    assert_eq!(out[2..62], [0; 60]);
    assert_eq!(out[62..], [0x7F, 0x80]);
}

#[test]
fn every_packet_is_exactly_200_bytes() {
    let mut player = Player::new(Arc::default()).unwrap();
    assert!(player.silence().is_some());
    let mut loud = [[0i16; 2]; REPORT_FRAMES];
    for (i, frame) in loud.iter_mut().enumerate() {
        let v = ((i as f32 * 0.07).sin() * 20_000.0) as i16;
        *frame = [v, v / 2];
    }
    for _ in 0..4 {
        let out = player.make(&loud, None).unwrap();
        assert_eq!(out[76..78], [0x93, 200]);
    }
}

fn pcm(sequence: u32, value: i16) -> Pcm {
    Pcm { controller: 0, sequence, samples: [value, value].iter().flat_map(|v| v.to_le_bytes()).cycle().take(960).collect() }
}

#[test]
fn reports_go_only_while_there_is_sound_and_haptics_only_while_they_play() {
    let mix = Arc::new(Mutex::new(Mix::default()));
    let mut player = Player::new(mix.clone()).unwrap();
    let now = Instant::now();
    assert!(player.tick(now).is_none(), "silence sends nothing");
    for sequence in 0..4 {
        lock(&mix).speaker.offer(&pcm(sequence, 1000), now);
    }
    let first = player.tick(now).unwrap();
    assert_eq!((first[1], first[10], first[76]), (0, 0, 0x93), "speaker alone");
    lock(&mix).haptics.offer(&pcm(9, 300), now);
    let second = player.tick(now).unwrap();
    assert_eq!((second[1], second[10], second[76]), (0x10, 1, 0x92), "with haptics");
    assert!(player.tick(now + IDLE).is_none(), "quiet again");
}

#[test]
fn a_stream_starts_once_it_holds_enough_and_a_gap_starts_it_over() {
    let mut fifo = Fifo::default();
    let now = Instant::now();
    fifo.offer(&pcm(0, 5), now);
    fifo.offer(&pcm(1, 5), now);
    assert_eq!(fifo.take()[0], [0, 0], "480 frames: still filling");
    fifo.offer(&pcm(2, 5), now);
    fifo.offer(&pcm(2, 7), now);
    assert_eq!(fifo.frames.len(), 720, "a duplicate is dropped");
    fifo.offer(&pcm(3, 5), now);
    assert_eq!(fifo.take()[511], [5, 5]);
    fifo.offer(&pcm(9, 6), now);
    assert_eq!(fifo.frames.len(), 240, "a gap drops what was held");
    assert_eq!(fifo.take()[0], [0, 0]);
    for sequence in 10..20 {
        fifo.offer(&pcm(sequence, 6), now);
    }
    assert!(fifo.frames.len() <= MAX_FRAMES);
}

/// A HID device that records the reports it takes: `refuse(n)` says whether its `n`th write
/// (0 being the probe) fails, and each write takes `delay`.
struct Recorder {
    sent: Sender<Vec<u8>>,
    refuse: fn(usize) -> bool,
    writes: usize,
    delay: Duration,
}

impl Recorder {
    fn new(sent: Sender<Vec<u8>>, refuse: fn(usize) -> bool) -> Self {
        Self { sent, refuse, writes: 0, delay: Duration::ZERO }
    }
}

impl Output for Recorder {
    fn write(&mut self, report: &[u8]) -> bool {
        let n = self.writes;
        self.writes += 1;
        std::thread::sleep(self.delay);
        if (self.refuse)(n) {
            return false;
        }
        let _ = self.sent.send(report.to_vec());
        true
    }
}

/// Feeds `audio` a speaker block per 5 ms, as a host does however late the worker runs, until
/// it stops or `until` passes: the reports it took meanwhile.
fn play(audio: &BtAudio, reports: &Receiver<Vec<u8>>, until: Duration) -> Vec<Vec<u8>> {
    let (mut sequence, mut played) = (0, Vec::new());
    let deadline = Instant::now() + until;
    while audio.alive() && Instant::now() < deadline {
        audio.offer(&pcm(sequence, 2000), true);
        sequence += 1;
        if let Ok(out) = reports.recv_timeout(Duration::from_millis(5)) {
            played.push(out);
        }
    }
    played.extend(reports.try_iter());
    played
}

#[test]
fn the_worker_probes_plays_and_stops_after_a_second_of_refused_writes() {
    let (sent, reports) = channel();
    assert!(BtAudio::start(Recorder::new(sent.clone(), |_| true)).is_err(), "probe refused");
    let audio = BtAudio::start(Recorder::new(sent, |n| n >= 3)).unwrap();
    let probe = reports.recv().unwrap();
    assert_eq!((probe.len(), probe[0], probe[76]), (REPORT_BYTES, 0x36, 0x93));
    let started = Instant::now();
    let played = play(&audio, &reports, Duration::from_secs(10));
    assert_eq!(played.len(), 2, "two accepted reports");
    for out in played {
        let mut check = out.clone();
        seal(&mut check);
        assert_eq!(check, out, "a valid checksum");
    }
    assert!(!audio.alive(), "refused writes stop the worker");
    assert!(started.elapsed() >= GIVE_UP, "only after a second of them");
}

#[test]
fn a_moment_of_refused_writes_leaves_it_playing() {
    let (sent, reports) = channel();
    let audio = BtAudio::start(Recorder::new(sent, |n| (2..=6).contains(&n))).unwrap();
    reports.recv().unwrap();
    let played = play(&audio, &reports, Duration::from_millis(400));
    assert!(audio.alive(), "five refused writes in a row are a busy link");
    assert!(played.len() >= 3, "it plays on after them: {} reports", played.len());
}

#[test]
fn dropping_it_never_waits_for_a_write_in_progress() {
    let (sent, reports) = channel();
    let mut slow = Recorder::new(sent, |_| false);
    slow.delay = Duration::from_millis(300);
    let audio = BtAudio::start(slow).unwrap();
    reports.recv().unwrap();
    audio.offer(&pcm(0, 2000), true);
    // The worker's next tick starts a write that takes 300 ms.
    std::thread::sleep(Duration::from_millis(60));
    let at = Instant::now();
    drop(audio);
    assert!(at.elapsed() < Duration::from_millis(100), "dropped in {:?}", at.elapsed());
    settle(Duration::from_secs(2));
}
