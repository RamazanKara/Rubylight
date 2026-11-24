//! DualSense USB controller audio routing, microphone selection, and haptics PCM forwarding.

use sdl3_sys::everything::SDL_AudioDeviceID;

use super::Gamepads;

impl Gamepads {
    pub fn prepare_audio(&mut self, enabled: bool) {
        for pad in &mut self.pads {
            if let Some(audio) = pad.audio.take() { audio.reset(pad.handle); }
        }
        if !enabled { return; }
        self.auto_audio();
    }

    pub fn auto_audio(&mut self) {
        let outputs = crate::controller_audio::outputs();
        let pads: Vec<_> = self.pads.iter().filter(|p| p.supports_audio()).map(|p| p.index).collect();
        if let ([index], [(device, _)]) = (pads.as_slice(), outputs.as_slice()) {
            if self.pads.iter().any(|p| p.index == *index && p.audio.is_none())
                && let Err(error) = self.set_audio(*index, Some(*device)) { eprintln!("nightjar: controller audio: {error}"); }
            let inputs = crate::controller_audio::inputs();
            if let [(input, _)] = inputs.as_slice()
                && let Some(audio) = self.pads.iter_mut().find(|p| p.index == *index).and_then(|p| p.audio.as_mut())
                    && audio.input.is_none() { audio.set_input(Some(*input)); }
        }
    }

    pub fn set_audio(&mut self, index: u8, device: Option<SDL_AudioDeviceID>) -> Result<(), String> {
        if device.is_some_and(|device| self.pads.iter().any(|p| p.index != index && p.audio.as_ref().is_some_and(|a| a.device == device))) {
            return Err("That output already belongs to another controller.".into());
        }
        let Some(pad) = self.pads.iter_mut().find(|p| p.index == index && p.supports_audio()) else { return Err("Connect a DualSense controller over USB.".into()); };
        if device.is_some_and(|id| !crate::controller_audio::outputs().iter().any(|(candidate, _)| *candidate == id)) {
            return Err("The controller's four-channel output is unavailable.".into());
        }
        if let Some(audio) = pad.audio.take() { audio.reset(pad.handle); }
        pad.announced = false;
        if let Some(device) = device {
            let audio = crate::controller_audio::Audio::open(device, pad.handle)?;
            audio.send(pad.handle);
            pad.audio = Some(audio);
        }
        Ok(())
    }

    pub fn set_audio_input(&mut self, index: u8, device: Option<SDL_AudioDeviceID>) -> Result<(), String> {
        if device.is_some_and(|device| self.pads.iter().any(|p| p.index != index && p.audio.as_ref().is_some_and(|a| a.input == Some(device)))) {
            return Err("That microphone already belongs to another controller.".into());
        }
        if device.is_some_and(|id| !crate::controller_audio::inputs().iter().any(|(candidate, _)| *candidate == id)) {
            return Err("The controller microphone is unavailable.".into());
        }
        if let Some(audio) = self.pads.iter_mut().find(|p| p.index == index).and_then(|p| p.audio.as_mut()) { audio.set_input(device); }
        Ok(())
    }

    pub fn removed_audio_device(&mut self, device: SDL_AudioDeviceID) {
        for pad in &mut self.pads {
            if pad.audio.as_mut().is_some_and(|audio| audio.removed(device)) {
                if let Some(audio) = pad.audio.take() { audio.reset(pad.handle); }
                pad.announced = false;
            }
        }
    }

    pub fn update_audio(&mut self) {
        for pad in &mut self.pads {
            if let Some(audio) = &mut pad.audio { audio.update(pad.handle); }
        }
    }

    /// Haptics or speaker audio for gamepad `pcm.controller`: the controller sent as it that plays it.
    pub fn pcm(&mut self, pcm: nightjar_core::control::haptics::Pcm, speaker: bool) {
        if !speaker && self.disable_rumble { return; }
        if let Some(pad) = self.pads.iter_mut().find(|p| u16::from(p.sent_as) == pcm.controller && p.audio.is_some())
            && let Some(audio) = &mut pad.audio { audio.offer(pcm, speaker); audio.update(pad.handle); }
    }

    pub fn audio_control(&mut self, payload: &[u8]) {
        if !crate::controller_audio::valid_control(payload) { return; }
        let index = u16::from_le_bytes([payload[2], payload[3]]);
        if let Some(pad) = self.pads.iter_mut().find(|p| u16::from(p.sent_as) == index && p.audio.is_some())
            && let Some(audio) = &mut pad.audio { audio.control(payload, pad.handle); }
    }
}
