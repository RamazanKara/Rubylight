//! A DualSense's speaker and haptics over Bluetooth (Windows and Linux): Sony's 398-byte
//! output report 0x36, one per 10.67 ms, carrying 512 host frames of speaker audio as a
//! 480-frame stereo Opus packet and, while a haptics stream plays, 32 signed stereo frames of
//! actuator samples at 3 kHz. The microphone stays off.
//!
//! The report layout, the 512-to-480 resampling and the Opus settings are ported from Nonary's
//! moonlight-qt fork (https://github.com/Nonary/moonlight-qt, GPL-3.0-or-later, Chase Payne),
//! `app/streaming/input/dualsensebtaudio.h` at commit b72d0c2, whose protocol research is
//! mrFunkill's decky-dualsense-wired (`research/FINDINGS.md`). The checksum is the standard
//! CRC-32 over the Bluetooth output seed byte 0xA2 and the report, written here independently.
//!
//! [`BtAudio::open`] probes the controller with a silent report and then runs a worker thread
//! on its own HID handle, so SDL keeps the controller's input, rumble, lights and triggers.
//! Reports go only while speaker or haptics samples arrive; a silent controller gets none.

use std::collections::VecDeque;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use nightjar_core::control::haptics::Pcm;
use sdl3_sys::everything::*;

use crate::stream::pipeline::lock;

/// One report: the bytes the controller takes.
pub const REPORT_BYTES: usize = 398;
/// Host frames (48 kHz stereo) per report.
pub const REPORT_FRAMES: usize = 512;
/// How often a report goes: 512 frames at 48 kHz.
pub const PERIOD: Duration = Duration::from_nanos(10_666_667);
const OPUS_FRAMES: usize = 480;
const OPUS_BYTES: usize = 200;
const HAPTICS_FRAMES: usize = 32;
/// Frames a stream holds before it starts, against the host's 5 ms blocks arriving unevenly.
const START_FRAMES: usize = 768;
/// Frames a stream holds at most; past it, it drops back to [`START_FRAMES`].
const MAX_FRAMES: usize = 1536;
/// A stream with no sound for this long sends no more reports.
const IDLE: Duration = Duration::from_millis(100);

/// The standard CRC-32 (IEEE 802.3, reflected) of `bytes`, continuing from `crc`.
fn crc32(mut crc: u32, bytes: &[u8]) -> u32 {
    crc = !crc;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xEDB8_8320 & 0u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}

/// Writes the report's checksum into its last four bytes: CRC-32 of the Bluetooth output seed
/// 0xA2 and everything before them, little-endian.
pub fn seal(report: &mut [u8]) {
    let body = report.len() - 4;
    let crc = crc32(crc32(0, &[0xA2]), &report[..body]);
    report[body..].copy_from_slice(&crc.to_le_bytes());
}

/// Report 0x36 with `opus` as its speaker packet and `haptics`, if any, as its actuator frames.
/// `report` counts reports (its low four bits are sent), `audio` counts audio packets.
pub fn report(report: u8, audio: u8, opus: &[u8; OPUS_BYTES], haptics: Option<&[u8; 64]>) -> [u8; REPORT_BYTES] {
    let mut out = [0u8; REPORT_BYTES];
    out[0] = 0x36;
    out[1] = (report & 15) << 4;
    // Audio configuration: speaker on, microphone capture off.
    out[2..5].copy_from_slice(&[0x91, 7, 0xFE]);
    out[5..10].fill(0x30);
    out[10] = audio;
    // Output state: only the speaker volume and audio routing; SDL keeps the rest.
    out[11..15].copy_from_slice(&[0x90, 63, 0xA0, 0x80]);
    out[18] = 0x64;
    out[20] = 0x30;
    out[50] = 0x02;
    let mut at = 76;
    if let Some(haptics) = haptics {
        out[at..at + 2].copy_from_slice(&[0x92, 64]);
        out[at + 2..at + 66].copy_from_slice(haptics);
        at += 66;
    }
    out[at..at + 2].copy_from_slice(&[0x93, OPUS_BYTES as u8]);
    out[at + 2..at + 2 + OPUS_BYTES].copy_from_slice(opus);
    seal(&mut out);
    out
}

/// 512 stereo frames stretched linearly onto the 480 one Opus packet holds, first and last
/// frames kept.
pub fn resample(frames: &[[i16; 2]; REPORT_FRAMES]) -> [i16; OPUS_FRAMES * 2] {
    let mut out = [0i16; OPUS_FRAMES * 2];
    let (last_in, last_out) = (REPORT_FRAMES - 1, OPUS_FRAMES - 1);
    for i in 0..OPUS_FRAMES {
        let numerator = i * last_in;
        let (index, fraction) = (numerator / last_out, (numerator % last_out) as i32);
        let next = (index + 1).min(last_in);
        for channel in 0..2 {
            let (a, b) = (i32::from(frames[index][channel]), i32::from(frames[next][channel]));
            out[i * 2 + channel] = ((a * (last_out as i32 - fraction) + b * fraction) / last_out as i32) as i16;
        }
    }
    out
}

/// 512 actuator frames at 48 kHz as the 32 signed 8-bit stereo frames at 3 kHz a report
/// carries: each the mean of its 16.
pub fn decimate(frames: &[[i16; 2]; REPORT_FRAMES]) -> [u8; HAPTICS_FRAMES * 2] {
    let mut out = [0u8; HAPTICS_FRAMES * 2];
    let step = REPORT_FRAMES / HAPTICS_FRAMES;
    for (i, group) in frames.chunks_exact(step).enumerate() {
        for channel in 0..2 {
            let sum: i32 = group.iter().map(|frame| i32::from(frame[channel])).sum();
            out[i * 2 + channel] = ((sum / step as i32) >> 8) as i8 as u8;
        }
    }
    out
}

unsafe extern "C" {
    fn opus_encoder_create(rate: i32, channels: i32, application: i32, error: *mut i32) -> *mut c_void;
    fn opus_encoder_destroy(encoder: *mut c_void);
    fn opus_encoder_ctl(encoder: *mut c_void, request: i32, ...) -> i32;
    fn opus_encode(encoder: *mut c_void, pcm: *const i16, frames: i32, data: *mut u8, length: i32) -> i32;
}

const OPUS_APPLICATION_RESTRICTED_LOWDELAY: i32 = 2051;
const OPUS_SET_BITRATE: i32 = 4002;
const OPUS_SET_VBR: i32 = 4006;
const OPUS_SET_BANDWIDTH: i32 = 4008;
const OPUS_SET_COMPLEXITY: i32 = 4010;
const OPUS_SET_FORCE_CHANNELS: i32 = 4022;
const OPUS_BANDWIDTH_FULLBAND: i32 = 1105;

/// The controller's decoder's stream: 48 kHz stereo, 160 kb/s constant, so 480 frames are
/// always 200 bytes.
struct Opus(NonNull<c_void>);

// SAFETY: an Opus encoder is plain memory, used by one thread at a time.
unsafe impl Send for Opus {}

impl Opus {
    fn new() -> Result<Self, String> {
        let mut error = 0;
        // SAFETY: valid arguments; the result is checked.
        let raw = unsafe { opus_encoder_create(48_000, 2, OPUS_APPLICATION_RESTRICTED_LOWDELAY, &mut error) };
        let encoder = NonNull::new(raw).map(Self).ok_or_else(|| format!("speaker encoder failed ({error})"))?;
        for (request, value) in [
            (OPUS_SET_BITRATE, 160_000),
            (OPUS_SET_VBR, 0),
            (OPUS_SET_FORCE_CHANNELS, 2),
            (OPUS_SET_BANDWIDTH, OPUS_BANDWIDTH_FULLBAND),
            (OPUS_SET_COMPLEXITY, 0),
        ] {
            // SAFETY: a live encoder; each request takes one i32.
            let result = unsafe { opus_encoder_ctl(encoder.0.as_ptr(), request, value) };
            if result != 0 {
                return Err(format!("speaker encoder setting {request} failed ({result})"));
            }
        }
        Ok(encoder)
    }

    fn encode(&mut self, stereo: &[i16; OPUS_FRAMES * 2]) -> Option<[u8; OPUS_BYTES]> {
        let mut packet = [0u8; OPUS_BYTES];
        // SAFETY: a live encoder, 480 stereo frames in, 200 bytes of room out.
        let length = unsafe {
            opus_encode(self.0.as_ptr(), stereo.as_ptr(), OPUS_FRAMES as i32, packet.as_mut_ptr(), OPUS_BYTES as i32)
        };
        (length == OPUS_BYTES as i32).then_some(packet)
    }
}

impl Drop for Opus {
    fn drop(&mut self) {
        // SAFETY: created by `opus_encoder_create`, destroyed once.
        unsafe { opus_encoder_destroy(self.0.as_ptr()) };
    }
}

/// One stream's samples waiting for reports, in sequence order.
#[derive(Default)]
struct Fifo {
    frames: VecDeque<[i16; 2]>,
    sequence: Option<u32>,
    playing: bool,
    heard: Option<Instant>,
}

impl Fifo {
    fn offer(&mut self, pcm: &Pcm, now: Instant) {
        if self.sequence.is_some_and(|last| pcm.sequence.wrapping_sub(last) as i32 <= 0) {
            return;
        }
        if self.sequence.is_some_and(|last| pcm.sequence != last.wrapping_add(1)) {
            // A gap: start over rather than splice.
            self.frames.clear();
            self.playing = false;
        }
        self.sequence = Some(pcm.sequence);
        let mut loud = false;
        for frame in pcm.samples.as_chunks::<4>().0 {
            let frame = [i16::from_le_bytes([frame[0], frame[1]]), i16::from_le_bytes([frame[2], frame[3]])];
            loud |= frame != [0, 0];
            self.frames.push_back(frame);
        }
        if loud {
            self.heard = Some(now);
        }
        if self.frames.len() > MAX_FRAMES {
            let excess = self.frames.len() - START_FRAMES;
            self.frames.drain(..excess);
        }
    }

    fn active(&self, now: Instant) -> bool {
        self.heard.is_some_and(|at| now.duration_since(at) < IDLE)
    }

    /// The next report's frames: silence until enough are held, and after it runs dry.
    fn take(&mut self) -> [[i16; 2]; REPORT_FRAMES] {
        let mut out = [[0; 2]; REPORT_FRAMES];
        self.playing |= self.frames.len() >= START_FRAMES;
        if self.playing {
            let count = self.frames.len().min(REPORT_FRAMES);
            for (slot, frame) in out.iter_mut().zip(self.frames.drain(..count)) {
                *slot = frame;
            }
            self.playing = count == REPORT_FRAMES;
        }
        out
    }
}

/// The speaker and haptics streams of one controller.
#[derive(Default)]
struct Mix {
    speaker: Fifo,
    haptics: Fifo,
}

/// Makes the reports: the encoder and the two counters.
struct Player {
    opus: Opus,
    mix: Arc<Mutex<Mix>>,
    report: u8,
    audio: u8,
}

impl Player {
    fn new(mix: Arc<Mutex<Mix>>) -> Result<Self, String> {
        Ok(Self { opus: Opus::new()?, mix, report: 0, audio: 0 })
    }

    fn make(&mut self, speaker: &[[i16; 2]; REPORT_FRAMES], haptics: Option<[u8; 64]>) -> Option<[u8; REPORT_BYTES]> {
        let opus = self.opus.encode(&resample(speaker))?;
        let out = report(self.report, self.audio, &opus, haptics.as_ref());
        self.report = self.report.wrapping_add(1);
        self.audio = self.audio.wrapping_add(1);
        Some(out)
    }

    /// A silent report, without haptics: the probe that the controller takes them.
    fn silence(&mut self) -> Option<[u8; REPORT_BYTES]> {
        self.make(&[[0; 2]; REPORT_FRAMES], None)
    }

    /// The report due now, or `None` while both streams are silent.
    fn tick(&mut self, now: Instant) -> Option<[u8; REPORT_BYTES]> {
        let (speaker, haptics) = {
            let mut mix = lock(&self.mix);
            let (speaking, shaking) = (mix.speaker.active(now), mix.haptics.active(now));
            if !speaking && !shaking {
                return None;
            }
            // Haptics only while they play, so speaker sound alone leaves SDL's rumble be.
            let haptics = shaking.then(|| decimate(&mix.haptics.take()));
            (mix.speaker.take(), haptics)
        };
        self.make(&speaker, haptics)
    }
}

/// Where reports go: the controller's HID device, or a test's recorder.
pub trait Output: Send + 'static {
    /// Whether the controller took the whole report.
    fn write(&mut self, report: &[u8]) -> bool;
}

/// A HID handle of its own on the controller, closed with it.
struct Hid(*mut SDL_hid_device);

// SAFETY: the handle is used by one thread at a time: opened and probed on the UI thread, then
// only the worker writes and closes it.
unsafe impl Send for Hid {}

impl Output for Hid {
    fn write(&mut self, report: &[u8]) -> bool {
        // SAFETY: an open device; `report` is valid for its length. Windows pads the write to
        // the device's longest output report, so it may report more bytes than asked.
        let written = unsafe { SDL_hid_write(self.0, report.as_ptr(), report.len()) };
        usize::try_from(written).is_ok_and(|written| written >= report.len())
    }
}

impl Drop for Hid {
    fn drop(&mut self) {
        // SAFETY: opened by `SDL_hid_open_path`, closed once.
        unsafe { SDL_hid_close(self.0) };
    }
}

/// A DualSense on Bluetooth playing the stream's speaker and haptics PCM.
pub struct BtAudio {
    mix: Arc<Mutex<Mix>>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl BtAudio {
    /// Opens `pad`'s HID device and starts the reports, if a silent one goes through.
    pub fn open(pad: *mut SDL_Gamepad) -> Result<Self, String> {
        // SAFETY: an open gamepad on the UI thread; SDL owns the returned path.
        let hid = unsafe {
            let path = SDL_GetGamepadPath(pad);
            if path.is_null() { std::ptr::null_mut() } else { SDL_hid_open_path(path) }
        };
        if hid.is_null() {
            return Err(format!("no HID device: {}", crate::sdl::error()));
        }
        Self::start(Hid(hid))
    }

    /// Probes `output` with a silent report, then runs the worker on it.
    pub fn start(mut output: impl Output) -> Result<Self, String> {
        let mix = Arc::new(Mutex::new(Mix::default()));
        let mut player = Player::new(mix.clone())?;
        let probe = player.silence().ok_or("speaker encoder gave no packet")?;
        if !output.write(&probe) {
            return Err("the controller took no audio report".into());
        }
        let stop = Arc::new(AtomicBool::new(false));
        let worker = std::thread::Builder::new()
            .name("nightjar-ds-bt-audio".into())
            .spawn({
                let stop = stop.clone();
                move || run(output, player, &stop)
            })
            .map_err(|error| error.to_string())?;
        Ok(Self { mix, stop, worker: Some(worker) })
    }

    /// Queues `pcm`, a speaker block when `speaker`, else haptics.
    pub fn offer(&self, pcm: &Pcm, speaker: bool) {
        let mut mix = lock(&self.mix);
        let fifo = if speaker { &mut mix.speaker } else { &mut mix.haptics };
        fifo.offer(pcm, Instant::now());
    }

    /// Whether the worker still runs: a failed write stops it until the controller reconnects.
    pub fn alive(&self) -> bool {
        self.worker.as_ref().is_some_and(|worker| !worker.is_finished())
    }
}

impl Drop for BtAudio {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// One report per [`PERIOD`] while there is sound, until `stop` or a failed write.
fn run(mut output: impl Output, mut player: Player, stop: &AtomicBool) {
    let mut next = Instant::now();
    while !stop.load(Ordering::Acquire) {
        let now = Instant::now();
        if let Some(report) = player.tick(now)
            && !output.write(&report)
        {
            eprintln!("nightjar: controller speaker over Bluetooth stopped: the controller took no report");
            return;
        }
        next += PERIOD;
        if next + PERIOD * 4 < now {
            // Far behind (a stalled write): pick up from now rather than burst.
            next = now;
        }
        if let Some(wait) = next.checked_duration_since(Instant::now()) {
            std::thread::sleep(wait);
        }
    }
}

#[cfg(test)]
mod tests;
