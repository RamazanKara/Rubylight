// SPDX-License-Identifier: GPL-3.0-only
//! A composite USB DualSense: HID controls plus its matching USB audio device.
//! Adapted for Rubylight on 2026-10-13 from Nonary/Vibepollo 8b3ea1c4
//! (GPL-3.0-or-later) and Chase Payne's libvirtualgamepad 9edbce11 (MIT).
//! Descriptor and calibration notices are retained in descriptors.rs.
use crate::{
    controller_audio, haptics,
    input::Input,
    usbip::{Device, Setup, UsbDevice},
};
use std::{
    sync::atomic::{AtomicU32, Ordering},
    time::{Duration, Instant},
};
mod descriptors;
use descriptors::*;

pub const REPORT_PERIOD: Duration = Duration::from_millis(4);
static SEQUENCES: [AtomicU32; 16] = [const { AtomicU32::new(0) }; 16];
static SPEAKER_SEQUENCES: [AtomicU32; 16] = [const { AtomicU32::new(0) }; 16];

pub struct DualSense {
    slot: u8,
    client: u16,
    input: [u8; 64],
    contacts: [Option<(u8, u32)>; 2],
    tracking: u8,
    dual_touch: bool,
    epoch: Instant,
    configuration: u8,
    alternate: [u8; 4],
    explicit: [bool; 4],
    idle: u8,
    protocol: u8,
    mute: [u8; 2],
    volume: [[i16; 5]; 2],
    rumble: Option<[u8; 4]>,
    rgb: Option<[u8; 3]>,
    triggers: [Option<[u8; 11]>; 2],
    pending: u8,
    waveform: bool,
    packetizer: haptics::Packetizer,
    pcm: haptics::Queue,
    speaker_packetizer: haptics::Packetizer,
    speaker: haptics::Queue,
    capture: controller_audio::Capture,
    audio_flags: u8,
    audio_output: [u8; 8],
    audio_pending: bool,
    hid_seen: bool,
    audio_seen: bool,
}
impl DualSense {
    pub fn new(slot: u8, client: u16) -> Self {
        assert!(slot < 16 && client < 16);
        let mut input = [0; 64];
        input[0] = 1;
        input[1..5].fill(128);
        input[8] = 8;
        input[25] = 0x20; // stationary +1 g on Y
        input[33] = 0x80;
        input[37] = 0x80;
        input[53] = 0x2a; // wired, charged
        Self {
            slot,
            client,
            input,
            contacts: [None; 2],
            tracking: 0,
            dual_touch: false,
            epoch: Instant::now(),
            configuration: 1,
            alternate: [0; 4],
            explicit: [false; 4],
            idle: 0,
            protocol: 1,
            mute: [0; 2],
            volume: [[0; 5]; 2],
            rumble: None,
            rgb: None,
            triggers: [None; 2],
            pending: 0,
            waveform: false,
            packetizer: Default::default(),
            pcm: Default::default(),
            speaker_packetizer: Default::default(),
            speaker: Default::default(),
            capture: Default::default(),
            audio_flags: 0,
            audio_output: [0; 8],
            audio_pending: false,
            hid_seen: false,
            audio_seen: false,
        }
    }
    pub fn capabilities(&mut self, caps: u16) {
        self.waveform = caps & haptics::CAPABILITY != 0;
        self.dual_touch = caps & 0x100 != 0;
        self.clear_pcm();
        self.pending = u8::from(self.rumble.is_some())
            | (u8::from(self.rgb.is_some()) << 1)
            | (u8::from(self.triggers[0].is_some()) << 3)
            | (u8::from(self.triggers[1].is_some()) << 2);
        self.audio_pending = true;
    }
    pub fn enumerated(&self) -> bool {
        self.hid_seen && self.audio_seen
    }
    pub fn audio(&mut self, kind: u16, payload: &[u8]) {
        if kind == controller_audio::CONTROL {
            if let Some((id, flags)) = controller_audio::status(payload)
                && id == self.client
            {
                if flags & controller_audio::CAPTURE == 0 || flags & controller_audio::MUTED != 0 {
                    self.capture.clear();
                }
                if self.audio_flags & controller_audio::PLAYBACK == 0
                    && flags & controller_audio::PLAYBACK != 0
                {
                    self.audio_pending = true;
                }
                self.audio_flags = flags;
                self.input[54] = 8 | if flags & controller_audio::HEADPHONES != 0 {
                    1
                } else {
                    0
                };
            }
        } else if kind == controller_audio::MICROPHONE
            && self.audio_flags & (controller_audio::CAPTURE | controller_audio::MUTED)
                == controller_audio::CAPTURE
            && let Some((id, seq, samples)) = controller_audio::pcm(payload)
            && id == self.client
        {
            self.capture.push(seq, samples, Instant::now());
        }
    }
    pub fn take_speaker(&mut self) -> Vec<haptics::Frame> {
        self.speaker.drain(Instant::now()).collect()
    }
    pub fn suspend_audio(&mut self) {
        self.audio_flags = 0;
        self.clear_pcm();
    }
    pub fn clear_pcm(&mut self) {
        self.speaker_packetizer.clear();
        self.speaker.retain_controllers(0);
        self.capture.clear();
        self.packetizer.clear();
        self.pcm.retain_controllers(0);
    }
    pub fn take_pcm(&mut self) -> Vec<haptics::Frame> {
        self.take_pcm_at(Instant::now())
    }
    /// The PCM still fresh at `now` (frames older than `haptics::MAX_AGE` are dropped).
    pub fn take_pcm_at(&mut self, now: Instant) -> Vec<haptics::Frame> {
        self.pcm.drain(now).collect()
    }
    /// Apply Moonlight state without requiring a VHF device.
    pub fn apply(&mut self, event: &Input) {
        match *event {
            Input::Controller {
                buttons,
                left_trigger,
                right_trigger,
                sticks,
                ..
            } => {
                for (i, v) in sticks.into_iter().enumerate() {
                    self.input[i + 1] = ((32768
                        + if i % 2 == 0 {
                            i32::from(v)
                        } else {
                            -i32::from(v)
                        })
                        >> 8)
                        .clamp(0, 255) as u8;
                }
                self.input[5] = left_trigger;
                self.input[6] = right_trigger;
                self.input[8] = match buttons & 15 {
                    1 => 0,
                    9 => 1,
                    8 => 2,
                    10 => 3,
                    2 => 4,
                    6 => 5,
                    4 => 6,
                    5 => 7,
                    _ => 8,
                };
                for (mask, bit) in [
                    (0x4000, 0x10),
                    (0x1000, 0x20),
                    (0x2000, 0x40),
                    (0x8000, 0x80),
                ] {
                    if buttons & mask != 0 {
                        self.input[8] |= bit;
                    }
                }
                self.input[9] =
                    (u8::from(left_trigger > 0) * 4) | (u8::from(right_trigger > 0) * 8);
                for (mask, bit) in [
                    (0x100, 1),
                    (0x200, 2),
                    (0x20, 0x10),
                    (0x10, 0x20),
                    (0x40, 0x40),
                    (0x80, 0x80),
                ] {
                    if buttons & mask != 0 {
                        self.input[9] |= bit;
                    }
                }
                self.input[10] = u8::from(buttons & 0x400 != 0)
                    | (u8::from(buttons & 0x100000 != 0) * 2)
                    | (u8::from(buttons & 0x200000 != 0) * 4);
            }
            Input::Motion { kind, xyz, .. } if matches!(kind, 1 | 2) => {
                let offset = if kind == 1 { 22 } else { 16 };
                let scale = if kind == 1 { 8192. / 9.80665 } else { 16. };
                for (i, v) in xyz.into_iter().enumerate() {
                    let value = (v * scale).clamp(-32768., 32767.) as i16;
                    self.input[offset + i * 2..offset + i * 2 + 2]
                        .copy_from_slice(&value.to_le_bytes());
                }
            }
            Input::ControllerTouch {
                touchpad,
                event,
                pointer,
                x,
                y,
                ..
            } if touchpad <= u8::from(self.dual_touch) => {
                if event == 7 {
                    self.contacts = [None; 2];
                    self.input[33] |= 0x80;
                    self.input[37] |= 0x80;
                    return;
                }
                let key = (touchpad, pointer);
                let existing = self.contacts.iter().position(|p| *p == Some(key));
                if matches!(event, 2 | 4 | 6) {
                    if let Some(i) = existing {
                        self.contacts[i] = None;
                        self.input[33 + i * 4] |= 0x80;
                    }
                } else if matches!(event, 1 | 3)
                    && let Some(i) =
                        existing.or_else(|| self.contacts.iter().position(Option::is_none))
                {
                    let at = 33 + i * 4;
                    if existing.is_none() {
                        self.tracking = self.tracking.wrapping_add(1) & 0x7f;
                        self.input[at] = self.tracking;
                        self.contacts[i] = Some(key);
                    }
                    let x = if self.dual_touch {
                        (x.clamp(0., 1.) + f32::from(touchpad)) * 0.5
                    } else {
                        x.clamp(0., 1.)
                    };
                    let x = (x * 1919.) as u16;
                    let y = (y.clamp(0., 1.) * 1079.) as u16;
                    self.input[at + 1..at + 4].copy_from_slice(&[
                        x as u8,
                        ((x >> 8) | (y << 4)) as u8,
                        (y >> 4) as u8,
                    ]);
                }
            }
            Input::Battery { state, percent, .. } => {
                if percent <= 100 {
                    self.input[53] = ((u16::from(percent) * 10 + 50) / 100) as u8;
                }
                self.input[53] &= 15;
                self.input[53] |= match state {
                    3 | 4 => 0x10,
                    5 => 0x20,
                    _ => 0,
                };
            }
            _ => {}
        }
    }
    fn input_report(&mut self) -> Vec<u8> {
        self.input[7] = self.input[7].wrapping_add(1);
        let ticks = (self.epoch.elapsed().as_micros() / 3) as u32;
        self.input[28..32].copy_from_slice(&ticks.to_le_bytes());
        self.input.to_vec()
    }
    fn hid_output(&mut self, bytes: &[u8]) -> bool {
        if !(48..=64).contains(&bytes.len()) || bytes[0] != 2 {
            return false;
        }
        if bytes[1] & 1 != 0 || bytes[39] & 4 != 0 {
            let motors = [0, bytes[4], 0, bytes[3]];
            if self.rumble.replace(motors) != Some(motors) {
                self.pending |= 1;
            }
        }
        if bytes[2] & 4 != 0 {
            let rgb = [bytes[45], bytes[46], bytes[47]];
            if self.rgb.replace(rgb) != Some(rgb) {
                self.pending |= 2;
            }
        }
        for (i, flag, offset) in [(0, 8, 22), (1, 4, 11)] {
            if bytes[1] & flag != 0 {
                let effect: [u8; 11] = bytes[offset..offset + 11].try_into().unwrap();
                if self.triggers[i].replace(effect) != Some(effect) {
                    self.pending |= flag;
                }
            }
        }
        // Preserve partial audio controls independently of triggers and lights.
        for (flag, src, target) in [(0x10, 5, 2), (0x20, 6, 3), (0x40, 7, 4), (0x80, 8, 5)] {
            if bytes[1] & flag != 0 {
                self.audio_output[0] |= flag;
                self.audio_output[target] = bytes[src];
                self.audio_pending = true;
            }
        }
        for (flag, src, target) in [(1, 9, 6), (2, 10, 7)] {
            if bytes[2] & flag != 0 {
                self.audio_output[1] |= flag;
                self.audio_output[target] = bytes[src];
                self.audio_pending = true;
            }
        }
        true
    }
    /// Wire feedback, without the controller index. Partial output reports
    /// never reset an omitted trigger, light or motor.
    pub fn take_feedback(&mut self) -> Vec<(u16, Vec<u8>)> {
        let pending = std::mem::take(&mut self.pending);
        let mut messages = vec![];
        if self.audio_pending && self.audio_flags & controller_audio::PLAYBACK != 0 {
            self.audio_pending = false;
            let mut control = vec![1, 2];
            control.extend_from_slice(&self.client.to_le_bytes());
            control.extend_from_slice(&self.audio_output);
            messages.push((controller_audio::CONTROL, control));
        }
        if pending & 1 != 0 {
            messages.push((0x010b, self.rumble.unwrap().to_vec()));
        }
        if pending & 2 != 0 {
            messages.push((0x5502, self.rgb.unwrap().to_vec()));
        }
        if pending & 12 != 0 {
            let [left, right] = self.triggers.map(|t| t.unwrap_or([0; 11]));
            let mut data = vec![pending & 12, left[0], right[0]];
            data.extend_from_slice(&left[1..]);
            data.extend_from_slice(&right[1..]);
            messages.push((0x5503, data));
        }
        messages
    }
    fn feature(&self, id: u8) -> Option<Vec<u8>> {
        match id {
            5 => Some(CALIBRATION.to_vec()),
            0x20 => Some(FIRMWARE.to_vec()),
            0x85 => Some(vec![0x85, 0, 0]),
            9 => {
                let mut bytes = vec![0; 20];
                bytes[..10].copy_from_slice(&[9, self.slot, 0x52, 0x55, 0x42, 0x59, 2, 8, 0x25, 0]);
                Some(bytes)
            }
            _ => None,
        }
    }
}

impl Device for DualSense {
    fn describe(&self) -> UsbDevice {
        UsbDevice {
            speed: 3,
            vendor: 0x054c,
            product: 0x0ce6,
            release: 0x0100,
            interfaces: vec![(1, 1, 0), (1, 2, 0), (1, 2, 0), (3, 0, 0)],
        }
    }
    fn control(&mut self, s: &Setup, data: &[u8]) -> Option<Vec<u8>> {
        let Setup {
            request_type: ty,
            request: code,
            value,
            index,
            length,
        } = *s;
        if ty & 0x80 == 0 && data.len() < usize::from(length) {
            return None;
        }
        if (ty, code, value) == (0x23, 3, 4) {
            // usbip-win2's synthetic port reset
            self.configuration = 1;
            self.alternate = [0; 4];
            self.explicit = [false; 4];
            self.clear_pcm();
            return Some(vec![]);
        }
        if ty & 0x60 == 0 {
            return match (ty, code) {
                (0x80 | 0x81, 6) => match value >> 8 {
                    1 => Some(DEVICE.to_vec()),
                    2 => Some(CONFIGURATION.to_vec()),
                    3 => match value as u8 {
                        0 => Some(vec![4, 3, 9, 4]),
                        1 => Some(usb_string("Sony Interactive Entertainment")),
                        2 => Some(usb_string("DualSense Wireless Controller")),
                        3 => Some(usb_string(&format!("RUBYDS5{}", self.slot))),
                        _ => None,
                    },
                    6 => Some(vec![10, 6, 0, 2, 0, 0, 0, 64, 1, 0]),
                    7 => {
                        let mut other = CONFIGURATION.to_vec();
                        other[1] = 7;
                        let mut at = 0;
                        while at < other.len() {
                            if other[at + 1] == 5 {
                                other[at + 6] = if other[at + 3] & 3 == 1 { 1 } else { 4 };
                            }
                            at += usize::from(other[at]);
                        }
                        Some(other)
                    }
                    0x21 if index == 3 => Some(CONFIGURATION[204..213].to_vec()),
                    0x22 if index == 3 => {
                        self.hid_seen = true;
                        Some(REPORT.to_vec())
                    }
                    _ => None,
                },
                (0x80..=0x82, 0) => Some(vec![u8::from(ty == 0x80), 0]),
                (0..=2, 1 | 3 | 5) => Some(vec![]),
                (0, 9) if value <= 1 => {
                    self.configuration = value as u8;
                    self.alternate = [0; 4];
                    self.explicit = [false; 4];
                    self.clear_pcm();
                    Some(vec![])
                }
                (0x80, 8) => Some(vec![self.configuration]),
                (1, 11) if index < 4 && value <= u16::from(matches!(index, 1 | 2)) => {
                    self.alternate[index as usize] = value as u8;
                    self.explicit[index as usize] = true;
                    if index == 1 {
                        self.clear_pcm();
                    }
                    Some(vec![])
                }
                (0x81, 10) if index < 4 => Some(vec![self.alternate[index as usize]]),
                _ => None,
            };
        }
        if ty & 0x60 != 0x20 {
            return None;
        }
        if ty & 0x1f == 1 && index == 3 {
            return match (ty, code, value >> 8) {
                (0xa1, 1, 1) if value as u8 == 1 => Some(self.input_report()),
                (0xa1, 1, 3) => self.feature(value as u8),
                (0x21, 9, 2) if value as u8 == 2 => {
                    let accepted = if length == 47 {
                        let mut report = vec![2];
                        report.extend_from_slice(&data[..47]);
                        self.hid_output(&report)
                    } else {
                        self.hid_output(data)
                    };
                    accepted.then(Vec::new)
                }
                (0x21, 9, 3) if !data.is_empty() && data[0] == value as u8 && length <= 64 => {
                    Some(vec![])
                }
                (0x21, 10, _) => {
                    self.idle = (value >> 8) as u8;
                    Some(vec![])
                }
                (0xa1, 2, _) => Some(vec![self.idle]),
                (0x21, 11, _) if value <= 1 => {
                    self.protocol = value as u8;
                    Some(vec![])
                }
                (0xa1, 3, _) => Some(vec![self.protocol]),
                _ => None,
            };
        }
        if ty & 0x1f == 2 && matches!(index, 1 | 0x82) && value == 0x0100 {
            return match (ty & 0x80, code) {
                (0x80, 0x81..=0x84) => Some(vec![0x80, 0xbb, 0]),
                (0, 1) if length == 3 && data[..3] == [0x80, 0xbb, 0] => Some(vec![]),
                _ => None,
            };
        }
        let unit = index >> 8;
        let channel = (value & 255) as usize;
        let selector = value >> 8;
        if ty & 0x1f != 1
            || index & 255 != 0
            || !matches!(unit, 2 | 5)
            || channel > if unit == 2 { 4 } else { 2 }
        {
            return None;
        }
        self.audio_seen = true;
        let control = usize::from(unit == 5);
        match (selector, ty & 0x80, code) {
            (1, 0x80, 0x81) if channel == 0 => Some(vec![self.mute[control]]),
            (1, 0, 1) if channel == 0 && length == 1 => {
                self.mute[control] = u8::from(data[0] != 0);
                Some(vec![])
            }
            (2, 0x80, 0x81..=0x84) => {
                let value: i16 = match code {
                    0x81 => self.volume[control][channel],
                    0x82 => {
                        if unit == 2 {
                            -25600
                        } else {
                            0
                        }
                    }
                    0x83 => {
                        if unit == 2 {
                            0
                        } else {
                            12288
                        }
                    }
                    _ => {
                        if unit == 2 {
                            256
                        } else {
                            122
                        }
                    }
                };
                Some(value.to_le_bytes().to_vec())
            }
            (2, 0, 1) if length == 2 => {
                self.volume[control][channel] = i16::from_le_bytes([data[0], data[1]]);
                Some(vec![])
            }
            _ => None,
        }
    }
    fn interrupt(&mut self, endpoint: u8) -> Option<Vec<u8>> {
        (endpoint == 4 && self.configuration == 1).then(|| {
            self.hid_seen = true;
            self.input_report()
        })
    }
    fn output(&mut self, endpoint: u8, data: &[u8]) -> bool {
        endpoint == 3 && self.configuration == 1 && self.hid_output(data)
    }
    fn iso_layout(&self, endpoint: u8, input: bool) -> Option<(usize, usize)> {
        if self.configuration != 1
            || !(endpoint == 1 && !input || endpoint == 2 && input)
            || (self.explicit[endpoint as usize] && self.alternate[endpoint as usize] == 0)
        {
            return None;
        }
        Some(if input { (196, 4) } else { (392, 8) })
    }
    fn iso_discontinuity(&mut self) {
        self.clear_pcm();
    }
    fn iso(&mut self, endpoint: u8, input: bool, data: &[u8], length: usize) -> Option<Vec<u8>> {
        let (max, align) = self.iso_layout(endpoint, input)?;
        if length > max || !length.is_multiple_of(align) || (!input && data.len() != length) {
            return None;
        }
        self.alternate[endpoint as usize] = 1;
        if input {
            let n = length.min(192);
            if self.mute[1] != 0
                || self.audio_flags & (controller_audio::CAPTURE | controller_audio::MUTED)
                    != controller_audio::CAPTURE
            {
                self.capture.clear();
                return Some(vec![0; n]);
            }
            let mut samples = self.capture.read(n, Instant::now());
            audio_volume(&mut samples, &self.volume[1], false);
            return Some(samples);
        }
        if self.audio_flags & controller_audio::PLAYBACK != 0 {
            let mut stereo: Vec<_> = data
                .as_chunks::<8>()
                .0
                .iter()
                .flat_map(|f| f[..4].iter().copied())
                .collect();
            audio_volume(&mut stereo, &self.volume[0], self.mute[0] != 0);
            let now = Instant::now();
            for samples in self.speaker_packetizer.push(&stereo, now) {
                self.speaker.push(
                    haptics::Frame {
                        id: self.client,
                        sequence: SPEAKER_SEQUENCES[self.slot as usize]
                            .fetch_add(1, Ordering::Relaxed),
                        created: now,
                        samples,
                    },
                    now,
                );
            }
        }
        if self.waveform {
            // Windows/game audio already applies volume. Preserve only the two
            // actuator channels byte-for-byte; speaker sound travels separately.
            let stereo: Vec<_> = data
                .as_chunks::<8>()
                .0
                .iter()
                .flat_map(|frame| frame[4..8].iter().copied())
                .collect();
            let now = Instant::now();
            for samples in self.packetizer.push(&stereo, now) {
                self.pcm.push(
                    haptics::Frame {
                        id: self.client,
                        sequence: SEQUENCES[self.slot as usize].fetch_add(1, Ordering::Relaxed),
                        created: now,
                        samples,
                    },
                    now,
                );
            }
        }
        Some(vec![])
    }
}
fn audio_volume(samples: &mut [u8], volume: &[i16; 5], muted: bool) {
    if muted {
        samples.fill(0);
        return;
    }
    for channel in 0..2 {
        let db = (i32::from(volume[0]) + i32::from(volume[channel + 1])) as f32 / 256.;
        if db == 0. {
            continue;
        }
        let gain = 10_f32.powf(db.clamp(-100., 40.) / 20.);
        for frame in samples.as_chunks_mut::<4>().0 {
            let at = channel * 2;
            let sample = i16::from_le_bytes([frame[at], frame[at + 1]]);
            let scaled = (f32::from(sample) * gain).round().clamp(-32768., 32767.) as i16;
            frame[at..at + 2].copy_from_slice(&scaled.to_le_bytes());
        }
    }
}
fn usb_string(s: &str) -> Vec<u8> {
    let mut bytes = vec![(s.encode_utf16().count() * 2 + 2) as u8, 3];
    for code in s.encode_utf16() {
        bytes.extend_from_slice(&code.to_le_bytes());
    }
    bytes
}

#[cfg(test)]
mod tests;
