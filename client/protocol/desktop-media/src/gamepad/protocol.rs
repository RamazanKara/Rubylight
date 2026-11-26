//! Host controller protocol reporting, arrival capabilities, battery, rumble, and DualSense triggers.
//!
//! Controller types, capability flags, and arrival/rumble reporting follow moonlight-qt's
//! `app/streaming/input/gamepad.cpp` (GPL-3.0, Cameron Gutman and contributors).

use nightjar_core::input::packets::{capability, motion_type};
use nightjar_core::input::InputQueue;
use nightjar_policy::controllers::Controllers;
use sdl3_sys::everything::*;

use crate::gamepad_types::*;

use super::{Gamepads, Pad};

impl Gamepads {
    /// Rubylight's prepared controller list for a launch with `settings`: empty with "Controllers"
    /// Off, with One player the first controller as player 1 alone.
    pub fn launch_controllers(&self, settings: &crate::config::Settings) -> String {
        let players = Controllers::from_saved(settings.controllers, settings.multi_controller);
        let mut listed = 0u16;
        self.pads.iter().filter_map(|pad| {
            let sent = players.sent_index(pad.index)?;
            if listed & (1 << sent) != 0 {
                return None;
            }
            listed |= 1 << sent;
            let (kind, buttons, mut caps) = describe(pad);
            if !settings.controller_motion {
                caps &= !(capability::ACCEL | capability::GYRO);
            }
            let kind = if kind == CTYPE_UNKNOWN { CTYPE_XBOX } else { kind };
            Some(format!("{sent}:{kind}:{caps}:{buttons}"))
        }).collect::<Vec<_>>().join(",")
    }

    /// The gamepad slots a launch sends as PlayStation controllers (`psmap`), as
    /// [`Self::launch_controllers`] numbers them; the launch masks it by the gamepad mask.
    pub fn launch_playstation_mask(&self, settings: &crate::config::Settings) -> u32 {
        let players = Controllers::from_saved(settings.controllers, settings.multi_controller);
        self.pads.iter().fold(0, |mask, pad| match players.sent_index(pad.index) {
            Some(sent) if describe(pad).0 == CTYPE_PS && sent < 16 => mask | (1 << sent),
            _ => mask,
        })
    }

    /// Tells the host about every controller (at stream start).
    pub fn announce_all(&mut self, queue: &mut InputQueue) {
        let mask = self.active_mask();
        for pad in self.pads.iter_mut().filter(|pad| !pad.refused) {
            announce(pad, mask, self.motion, queue);
        }
    }

    pub fn announce_new(&mut self, queue: &mut InputQueue) {
        let mask = self.active_mask();
        for pad in self.pads.iter_mut().filter(|pad| !pad.announced && !pad.refused) {
            announce(pad, mask, self.motion, queue);
        }
    }

    /// The host forgets gamepad `index` ([`Self::removed`]): a neutral state with it out of the
    /// mask. With One player another controller still sent as it sends its own state instead, and
    /// player 1 stays in the mask.
    pub fn send_removed(&self, index: u8, queue: &mut InputQueue) {
        // A controller the host had no slot for was never announced: there is nothing to forget.
        if u16::from(index) >= u16::from(self.controller_limit()) {
            return;
        }
        let mask = self.active_mask();
        if let Some(pad) = self.sent_as(u16::from(index)).find(|pad| pad.announced) {
            send_state(pad, mask, queue);
            return;
        }
        let _ = queue.send_controller(u16::from(index), mask, 0, 0, 0, (0, 0), (0, 0));
    }

    /// Rumble from the host for gamepad `index`, on every controller sent as it; motor speeds 0-65535.
    pub fn rumble(&self, index: u16, low: u16, high: u16) {
        use nightjar_policy::rumble::shape;
        if self.disable_rumble { return; }
        for pad in self.sent_as(index) {
            if pad.audio.as_ref().is_some_and(|audio| audio.active()) { continue; }
            // SAFETY: an open gamepad. A long duration; the host sends 0 to stop.
            unsafe { SDL_RumbleGamepad(pad.handle, shape(low, self.rumble_feel), shape(high, self.rumble_feel), 30_000) };
        }
    }

    pub fn rumble_triggers(&self, index: u16, left: u16, right: u16) {
        use nightjar_policy::rumble::shape;
        if self.disable_rumble { return; }
        for pad in self.sent_as(index) {
            // SAFETY: an open gamepad.
            unsafe { SDL_RumbleGamepadTriggers(pad.handle, shape(left, self.rumble_feel), shape(right, self.rumble_feel), 30_000) };
        }
    }

    pub fn set_led(&self, index: u16, r: u8, g: u8, b: u8) {
        for pad in self.sent_as(index) {
            // SAFETY: an open gamepad.
            unsafe { SDL_SetGamepadLED(pad.handle, r, g, b) };
            if let Some(audio) = &pad.audio { audio.send(pad.handle); }
        }
    }

    pub fn adaptive_triggers(&self, index: u16, flags: u8, type_left: u8, type_right: u8, left: &[u8; 10], right: &[u8; 10]) {
        for pad in self.sent_as(index) {
            send_trigger_effect(pad, flags, type_left, type_right, left, right);
        }
    }

    /// The host wants motion data from gamepad `index` (rate 0 stops it), from every controller
    /// sent as it.
    pub fn set_motion(&self, index: u16, kind: u8, rate_hz: u16) {
        if !self.motion {
            return;
        }
        let (sensor, slot, label) = match kind {
            motion_type::ACCEL => (SDL_SENSOR_ACCEL, 0usize, "accel"),
            motion_type::GYRO => (SDL_SENSOR_GYRO, 1usize, "gyro"),
            _ => return,
        };
        for pad in self.sent_as(index) {
            // A pad pinned as Xbox told the host it has no motion sensors.
            let on = rate_hz != 0 && !pad.mapping.drops_motion_and_touchpad();
            let wanted_rate = if on { rate_hz } else { 0 };
            // SAFETY: an open gamepad.
            unsafe {
                let already = SDL_GamepadSensorEnabled(pad.handle, sensor);
                if !should_apply_motion_rate(pad.motion_rates[slot].get(), wanted_rate, already, on) {
                    continue;
                }
                // Nightjar #177: if a Bluetooth controller missed its initial enhanced-mode report
                // right on arrival, SDL3 can still report the sensor enabled and treat another
                // `true` call as a no-op; cycling `false -> true` forces SDL to resend it once per
                // rate transition.
                if should_cycle_sensor(on, already) {
                    let _ = SDL_SetGamepadSensorEnabled(pad.handle, sensor, false);
                }
                if !SDL_SetGamepadSensorEnabled(pad.handle, sensor, on) {
                    eprintln!(
                        "nightjar: gamepad {} ({}) {label} sensor ({rate_hz} Hz, on={on}) failed: {}",
                        pad.sent_as,
                        pad.name,
                        crate::sdl::error()
                    );
                } else {
                    pad.motion_rates[slot].set(wanted_rate);
                }
            }
        }
    }

    /// Stops every motor and sensor (stream end).
    pub fn reset(&mut self) {
        for pad in &self.pads {
            send_trigger_effect(pad, 0x0C, 0x05, 0x05, &[0; 10], &[0; 10]);
        }
        for pad in &mut self.pads {
            if let Some(audio) = pad.audio.take() { audio.reset(pad.handle); }
            // SAFETY: open gamepads.
            unsafe {
                SDL_RumbleGamepad(pad.handle, 0, 0, 0);
                SDL_RumbleGamepadTriggers(pad.handle, 0, 0, 0);
                SDL_SetGamepadSensorEnabled(pad.handle, SDL_SENSOR_ACCEL, false);
                SDL_SetGamepadSensorEnabled(pad.handle, SDL_SENSOR_GYRO, false);
            }
            for rate in &pad.motion_rates {
                rate.set(0);
            }
            pad.announced = false;
            pad.buttons = 0;
            pad.lt = 0;
            pad.rt = 0;
            pad.ls = (0, 0);
            pad.rs = (0, 0);
            pad.mouse = crate::controller_mouse::Mouse::default();
            pad.touchpad = crate::controller_mouse::Touchpad::default();
        }
        // The next stream sets its host's ([`Self::set_limit`]).
        self.limit = None;
        self.refused_notice_shown = false;
        self.assign_slots();
    }

    pub fn close_all(&mut self) {
        for pad in self.pads.drain(..) {
            if let Some(audio) = &pad.audio { audio.reset(pad.handle); }
            drop(pad.audio);
            // SAFETY: gamepads we opened.
            unsafe { SDL_CloseGamepad(pad.handle) };
        }
    }
}

// SDL's DS5EffectsState_t excludes the USB/Bluetooth report framing and CRC.
pub(super) fn trigger_effect(flags: u8, type_left: u8, type_right: u8, left: &[u8; 10], right: &[u8; 10]) -> [u8; 47] {
    let mut effect = [0; 47];
    effect[0] = flags & 0x0C;
    effect[10] = type_right;
    effect[11..21].copy_from_slice(right);
    effect[21] = type_left;
    effect[22..32].copy_from_slice(left);
    effect
}

/// The host's adaptive trigger effect, on a DualSense.
fn send_trigger_effect(pad: &Pad, flags: u8, type_left: u8, type_right: u8, left: &[u8; 10], right: &[u8; 10]) {
    // SAFETY: an open gamepad.
    unsafe {
        if SDL_GetGamepadType(pad.handle) == SDL_GAMEPAD_TYPE_PS5 {
            let effect = trigger_effect(flags, type_left, type_right, left, right);
            SDL_SendGamepadEffect(pad.handle, effect.as_ptr().cast(), effect.len() as i32);
        }
    }
}

/// A pad's own button word for the host. The C button is a Switch 2 Pro
/// Controller's, and its flag (0x40_0000) is also a Steam Controller's left
/// grip touch, so a host that takes no controller identities gets neither
/// (nightjar#125).
fn pad_host_buttons(buttons: u32, controller_identity: bool) -> u32 {
    host_buttons(buttons, controller_identity)
}

pub(super) fn send_state(pad: &Pad, mask: u16, queue: &mut InputQueue) {
    if pad.refused {
        return;
    }
    let report = if pad.mouse.active { Report::default() } else { pad.report() };
    let _ = queue.send_controller(
        u16::from(pad.sent_as),
        mask,
        pad_host_buttons(report.buttons, queue.controller_identity()),
        report.lt,
        report.rt,
        report.ls,
        report.rs,
    );
}

pub(super) fn describe(pad: &Pad) -> (u8, u32, u16) {
    // SAFETY: an open gamepad; read-only queries.
    unsafe {
        let kind = match SDL_GetGamepadType(pad.handle) {
            SDL_GAMEPAD_TYPE_XBOX360 | SDL_GAMEPAD_TYPE_XBOXONE => CTYPE_XBOX,
            SDL_GAMEPAD_TYPE_PS3 | SDL_GAMEPAD_TYPE_PS4 | SDL_GAMEPAD_TYPE_PS5 => CTYPE_PS,
            SDL_GAMEPAD_TYPE_NINTENDO_SWITCH_PRO
            | SDL_GAMEPAD_TYPE_NINTENDO_SWITCH_JOYCON_LEFT
            | SDL_GAMEPAD_TYPE_NINTENDO_SWITCH_JOYCON_RIGHT
            | SDL_GAMEPAD_TYPE_NINTENDO_SWITCH_JOYCON_PAIR => CTYPE_NINTENDO,
            _ => CTYPE_UNKNOWN,
        };
        let kind = pad.mapping.arrival_type(kind);
        let mut supported = 0u32;
        for (i, flag) in BUTTON_MAP.iter().enumerate() {
            if SDL_GamepadHasButton(pad.handle, SDL_GamepadButton(i as i32)) {
                supported |= pad.mapping.buttons[i].unwrap_or(*flag);
            }
        }
        let props = SDL_GetGamepadProperties(pad.handle);
        let has = |name: *const std::ffi::c_char| SDL_GetBooleanProperty(props, name, false);
        let mut caps = 0u16;
        if pad.audio.is_some() { caps |= nightjar_core::control::haptics::CAPABILITY; }
        if SDL_GamepadHasAxis(pad.handle, SDL_GAMEPAD_AXIS_LEFT_TRIGGER) {
            caps |= capability::ANALOG_TRIGGERS;
        }
        if has(SDL_PROP_GAMEPAD_CAP_RUMBLE_BOOLEAN) {
            caps |= capability::RUMBLE;
        }
        if has(SDL_PROP_GAMEPAD_CAP_TRIGGER_RUMBLE_BOOLEAN) {
            caps |= capability::TRIGGER_RUMBLE;
        }
        if has(SDL_PROP_GAMEPAD_CAP_RGB_LED_BOOLEAN) {
            caps |= capability::RGB_LED;
        }
        let touchpads = SDL_GetNumGamepadTouchpads(pad.handle);
        if touchpads > 0 {
            caps |= capability::TOUCHPAD;
        }
        if touchpads > 1 {
            caps |= capability::DUAL_TOUCHPAD;
        }
        if SDL_GamepadHasSensor(pad.handle, SDL_SENSOR_ACCEL) {
            caps |= capability::ACCEL;
        }
        if SDL_GamepadHasSensor(pad.handle, SDL_SENSOR_GYRO) {
            caps |= capability::GYRO;
        }
        let mut percent = -1;
        let state = SDL_GetGamepadPowerInfo(pad.handle, &mut percent);
        if state != SDL_POWERSTATE_UNKNOWN && state != SDL_POWERSTATE_NO_BATTERY && state != SDL_POWERSTATE_ERROR {
            caps |= capability::BATTERY_STATE;
        }
        (kind, supported, pad.mapping.arrival_capabilities(caps))
    }
}

fn announce(pad: &mut Pad, mask: u16, motion: bool, queue: &mut InputQueue) {
    if pad.refused {
        return;
    }
    let (kind, supported, mut caps) = describe(pad);
    if !motion { caps &= !(capability::ACCEL | capability::GYRO); }
    // Which controller it is goes first, so the host can choose its virtual pad on arrival
    // (a Switch 2 Pro Controller its own); the queue sends it only to a host that takes it.
    if pad.usb_id != (0, 0) {
        let _ = queue.send_controller_identity(pad.sent_as, pad.usb_id.0, pad.usb_id.1);
    }
    let _ = queue.send_controller_arrival(pad.sent_as, mask, kind, supported, caps);
    pad.announced = true;
    if caps & capability::BATTERY_STATE != 0 {
        let mut percent = -1;
        let state = unsafe { SDL_GetGamepadPowerInfo(pad.handle, &mut percent) };
        send_battery(pad, state, percent, queue);
    }
}

pub(super) fn send_battery(pad: &Pad, state: SDL_PowerState, percent: i32, queue: &mut InputQueue) {
    if pad.refused {
        return;
    }
    let state_code = match state {
        SDL_POWERSTATE_ON_BATTERY => BATTERY_DISCHARGING,
        SDL_POWERSTATE_CHARGING => BATTERY_CHARGING,
        SDL_POWERSTATE_CHARGED => BATTERY_FULL,
        SDL_POWERSTATE_NO_BATTERY => BATTERY_NOT_PRESENT,
        _ => BATTERY_UNKNOWN,
    };
    let percent = if (0..=100).contains(&percent) {
        percent as u8
    } else {
        BATTERY_PERCENT_UNKNOWN
    };
    let _ = queue.send_controller_battery(pad.sent_as, state_code, percent);
}

/// Whether `set_motion` should disable before re-enabling a sensor so SDL3 re-sends its
/// Bluetooth enhanced-mode report on host watchdog retries (Nightjar #177).
fn should_cycle_sensor(want_on: bool, already_enabled: bool) -> bool {
    want_on && already_enabled
}

/// Whether `set_motion` should touch SDL's sensor state: skips repeated identical states once applied,
/// while applying changes in rate or mismatched SDL sensor state.
fn should_apply_motion_rate(prev_rate: u16, wanted_rate: u16, already_enabled: bool, want_on: bool) -> bool {
    prev_rate != wanted_rate || already_enabled != want_on
}

#[cfg(test)]
mod tests {
    use super::{should_apply_motion_rate, should_cycle_sensor};

    #[test]
    fn re_enabling_an_already_enabled_sensor_cycles_off_then_on() {
        assert!(should_cycle_sensor(true, true));
        assert!(!should_cycle_sensor(true, false));
        assert!(!should_cycle_sensor(false, true));
        assert!(!should_cycle_sensor(false, false));
    }

    #[test]
    fn repeated_identical_motion_rate_is_skipped() {
        assert!(should_apply_motion_rate(0, 250, true, true), "first enable even if SDL left enabled");
        assert!(!should_apply_motion_rate(250, 250, true, true), "repeated host state does not cycle");
        assert!(should_apply_motion_rate(250, 125, true, true), "changed rate reapplies");
        assert!(should_apply_motion_rate(250, 0, true, false), "disabling reapplies");
        assert!(!should_apply_motion_rate(0, 0, false, false), "repeated disable skipped");
    }
}
