//! DualSense's USB audio endpoint: speaker on channels 1/2, actuators on 3/4.

use std::collections::VecDeque;
use std::ffi::{c_void, CStr};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use nightjar_core::control::haptics::Pcm;
use sdl3_sys::everything::*;

use crate::stream::pipeline::lock;

struct Chunk {
    pcm: Vec<u8>,
    offset: usize,
    at: Instant,
}

#[derive(Default)]
struct Queue {
    chunks: VecDeque<Chunk>,
    sequence: Option<u32>,
    frames: usize,
    nonzero: Option<Instant>,
}

impl Queue {
    fn offer(&mut self, pcm: Pcm, now: Instant) {
        if self.sequence.is_some_and(|last| pcm.sequence.wrapping_sub(last) as i32 <= 0) { return; }
        if self.sequence.is_some_and(|last| pcm.sequence != last.wrapping_add(1)) || self.frames + pcm.frames() > 960 {
            self.chunks.clear(); self.frames = 0;
        }
        self.sequence = Some(pcm.sequence);
        if pcm.samples.iter().any(|s| *s != 0) { self.nonzero = Some(now); }
        if !self.active(now) { return; }
        self.frames += pcm.frames();
        self.chunks.push_back(Chunk { pcm: pcm.samples, offset: 0, at: now });
    }

    fn active(&self, now: Instant) -> bool { self.nonzero.is_some_and(|at| now.duration_since(at) < Duration::from_millis(100)) }

    fn render(&mut self, out: &mut [i16], offset: usize, now: Instant) {
        let mut frame = 0;
        while let Some(chunk) = self.chunks.front_mut() {
            if now.duration_since(chunk.at) >= Duration::from_millis(20) {
                self.frames -= (chunk.pcm.len() - chunk.offset) / 4;
                self.chunks.pop_front();
                continue;
            }
            let count = ((chunk.pcm.len() - chunk.offset) / 4).min(out.len() / 4 - frame);
            for _ in 0..count {
                for channel in 0..2 {
                    let at = chunk.offset + channel * 2;
                    out[frame * 4 + offset + channel] = i16::from_le_bytes([chunk.pcm[at], chunk.pcm[at + 1]]);
                }
                chunk.offset += 4;
                frame += 1;
            }
            self.frames -= count;
            if chunk.offset == chunk.pcm.len() { self.chunks.pop_front(); }
            if frame == out.len() / 4 { break; }
        }
    }
}

#[derive(Default)]
struct Playback {
    haptics: Queue,
    speaker: Queue,
}

unsafe extern "C" fn play(userdata: *mut c_void, stream: *mut SDL_AudioStream, additional: i32, _: i32) {
    let state = &*userdata.cast::<Mutex<Playback>>();
    // SDL calls only when it needs samples. Supply at most 20 ms, never an unbounded backlog.
    let frames = (additional.max(0) as usize / 8).min(960);
    if frames == 0 { return; }
    let mut samples = [0i16; 960 * 4];
    let output = &mut samples[..frames * 4];
    let mut state = lock(state);
    let now = Instant::now();
    state.speaker.render(output, 0, now);
    state.haptics.render(output, 2, now);
    SDL_PutAudioStreamData(stream, output.as_ptr().cast(), std::mem::size_of_val(output) as i32);
}

pub fn outputs() -> Vec<(SDL_AudioDeviceID, String)> {
    let mut result = Vec::new();
    unsafe {
        let mut count = 0;
        let devices = SDL_GetAudioPlaybackDevices(&mut count);
        if devices.is_null() { return result; }
        for &id in std::slice::from_raw_parts(devices, count.max(0) as usize) {
            let name = SDL_GetAudioDeviceName(id);
            if name.is_null() { continue; }
            let name = CStr::from_ptr(name).to_string_lossy().into_owned();
            let normalized = name.to_lowercase();
            if !normalized.contains("dualsense") && !normalized.contains("wireless controller") { continue; }
            let mut spec = SDL_AudioSpec::default();
            if SDL_GetAudioDeviceFormat(id, &mut spec, std::ptr::null_mut()) && spec.channels == 4 {
                result.push((id, name));
            }
        }
        SDL_free(devices.cast());
    }
    result
}

pub struct Audio {
    stream: *mut SDL_AudioStream,
    playback: Box<Mutex<Playback>>,
    pub device: SDL_AudioDeviceID,
    waveform: bool,
    effect: [u8; 47],
}

impl Audio {
    pub fn open(device: SDL_AudioDeviceID) -> Result<Self, String> {
        let mut playback = Box::new(Mutex::new(Playback::default()));
        let spec = SDL_AudioSpec { format: SDL_AUDIO_S16, channels: 4, freq: 48000 };
        let stream = unsafe { SDL_OpenAudioDeviceStream(device, &spec, Some(play), (&mut *playback as *mut Mutex<Playback>).cast()) };
        if stream.is_null() { return Err(crate::sdl::error()); }
        let mut audio = Self { stream, playback, device, waveform: false, effect: [0; 47] };
        audio.effect[4..8].copy_from_slice(&[80, 61, 64, 0x30]);
        if !unsafe { SDL_ResumeAudioStreamDevice(stream) } { return Err(crate::sdl::error()); }
        Ok(audio)
    }

    pub fn offer(&self, pcm: Pcm, speaker: bool) {
        let mut playback = lock(&self.playback);
        let queue = if speaker { &mut playback.speaker } else { &mut playback.haptics };
        queue.offer(pcm, Instant::now());
    }

    pub fn active(&self) -> bool { lock(&self.playback).haptics.active(Instant::now()) }

    pub fn update(&mut self, pad: *mut SDL_Gamepad) {
        let active = self.active();
        if active != self.waveform {
            if active { unsafe { SDL_RumbleGamepad(pad, 0, 0, 0) }; }
            self.waveform = active;
            self.send(pad);
        }
    }

    pub fn send(&self, pad: *mut SDL_Gamepad) {
        let mut effect = self.effect;
        // The PCM path never opens a microphone. Preserve mute regardless of host gain commands.
        effect[0] = 0xF0 | if self.waveform { 0 } else { 0x03 };
        effect[1] = 3;
        effect[8] = 1;
        effect[9] |= 0x10;
        unsafe { SDL_SendGamepadEffect(pad, effect.as_ptr().cast(), effect.len() as i32) };
    }

    pub fn control(&mut self, payload: &[u8], pad: *mut SDL_Gamepad) {
        if !valid_control(payload) || payload[1] != 2 { return; }
        for i in 0..4 {
            if payload[4] & (0x10 << i) != 0 { self.effect[4 + i] = payload[6 + i]; }
        }
        if payload[5] & 1 != 0 { self.effect[8] = payload[10]; }
        if payload[5] & 2 != 0 { self.effect[9] = payload[11]; }
        self.send(pad);
    }

    pub fn reset(&self, pad: *mut SDL_Gamepad) {
        let mut effect = [0u8; 47];
        effect[0] = 3;
        unsafe { SDL_SendGamepadEffect(pad, effect.as_ptr().cast(), effect.len() as i32) };
    }
}

impl Drop for Audio {
    fn drop(&mut self) {
        // Destroy waits for callbacks before freeing their userdata.
        unsafe { SDL_DestroyAudioStream(self.stream) };
    }
}

pub fn valid_control(p: &[u8]) -> bool {
    p.len() >= 8 && p[0] == 1 && u16::from_le_bytes([p[2], p[3]]) < 16
        && ((p[1] == 2 && p.len() == 12 && p[4] & 15 == 0 && p[5] & !3 == 0)
            || (p[1] == 3 && p.len() == 8 && p[4] & !15 == 0 && p[5..8] == [0, 0, 0]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pcm(sequence: u32) -> Pcm { Pcm { controller: 0, sequence, samples: vec![0xff, 0x7f, 0, 0x80] } }

    #[test]
    fn waveform_preserves_channels_rejects_old_packets_and_drops_stale_audio() {
        let now = Instant::now();
        let mut q = Queue::default();
        q.offer(pcm(u32::MAX), now);
        q.offer(pcm(0), now);
        q.offer(pcm(u32::MAX), now);
        let mut out = [0; 12];
        q.render(&mut out, 2, now);
        assert_eq!(out, [0, 0, 32767, -32768, 0, 0, 32767, -32768, 0, 0, 0, 0]);
        q.offer(pcm(1), now);
        let mut out = [0; 8];
        q.render(&mut out, 2, now + Duration::from_millis(20));
        assert_eq!(out, [0; 8]);
        assert!(!q.active(now + Duration::from_millis(100)));
    }

    #[test]
    fn discontinuity_discards_old_audio_and_control_is_bounded() {
        let mut q = Queue::default(); let now = Instant::now();
        q.offer(pcm(1), now); q.offer(pcm(3), now);
        assert_eq!(q.frames, 1);
        assert!(valid_control(&[1, 2, 0, 0, 0x10, 0, 42, 0, 0, 0, 0, 0]));
        assert!(!valid_control(&[1, 2, 0, 0, 1, 0, 42, 0, 0, 0, 0, 0]));
        assert!(!valid_control(&[1, 3, 16, 0, 0, 0, 0, 0]));
    }
}
