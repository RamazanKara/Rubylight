//! How decoded input is applied: gamepad backend and profile selection,
//! per-session input permissions and the back-button mapping.
use crate::{config::Config, input::Input};
use anyhow::Result;
use std::{collections::BTreeMap, time::Duration};

// Host profile IDs; 3..=7 and 9 are the VHF driver's profile numbers.
pub const VHF_AUTO: u16 = 10;
/// Composite HID + USB audio through usbip-win2.
pub const USBIP_DUALSENSE: u16 = 11;
/// A Switch 2 Pro Controller (057e:2069) through usbip-win2: what automatic
/// selection gives a client's own Switch 2 Pro Controller, and every
/// controller when `gamepad = usbip_switch2` chooses it.
pub const USBIP_SWITCH2: u16 = 12;
pub const VHF_XBOX_ONE: u16 = 3;
pub const VHF_XBOX_SERIES: u16 = 4;
pub const VHF_DS4: u16 = 5;
/// The VHF driver's DualSense, the only virtual pad with adaptive triggers.
pub const VHF_DUALSENSE: u16 = 6;
pub const VHF_SWITCH: u16 = 7;
/// The VHF driver's Steam Controller (2026), only when `gamepad = vhf_steam`
/// chooses it: libvirtualgamepad 0.1.0-beta.10 and later offer it.
pub const VHF_STEAM_CONTROLLER: u16 = 9;
/// An Xbox 360 pad on ViGEmBus, separately installed (`gamepad = vigem_x360`):
/// an emulated USB device for tools that don't see a VHF pad (#74).
pub const VIGEM_X360: u16 = 8;
/// A DualShock 4 on ViGEmBus (`gamepad = vigem_ds4`): the same USB device for
/// those tools, with touchpad, motion, battery and lightbar, as Vibepollo's
/// (#108).
pub const VIGEM_DS4: u16 = 13;
/// What ViGEmBus offers, as a bit per profile.
pub const VIGEM_PROFILES: u32 = (1 << (VIGEM_X360 - 1)) | (1 << (VIGEM_DS4 - 1));
/// Moonlight's `LI_CTYPE_STEAM`: a Valve controller such as the Steam Deck,
/// with an Xbox layout, two trackpads, motion sensors and back grips.
pub const CTYPE_STEAM: u8 = 4;

/// Moonlight's paddle buttons in the order of [`BACK_GRIP_KEYS`]: upper
/// right, upper left, lower right and lower left. They are a Steam Deck's
/// R4, L4, R5 and L5, and an Xbox Elite's P1, P3, P2 and P4.
pub const PADDLES: [u32; 4] = [0x01_0000, 0x02_0000, 0x04_0000, 0x08_0000];
/// The settings that choose what each back grip presses.
pub const BACK_GRIP_KEYS: [&str; 4] = [
    "back_grip_r4",
    "back_grip_l4",
    "back_grip_r5",
    "back_grip_l5",
];

/// The stream-card hint for a back grip pressed while it is unmapped.
pub const BACK_GRIP_HINT: &str = "A controller's back grips (a Steam Deck's L4, R4, L5 or R5) were pressed, but no virtual controller has them, so they do nothing. Choose what each one presses under Settings, Input, Steam Deck and back grips.";

/// The `steam_deck_controller` setting: what a Steam Deck client's
/// controller becomes on the host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeckPad {
    /// A real Steam Deck controller when usbip-win2 is installed and Steam
    /// is running on the host; otherwise the virtual pad.
    Auto,
    /// A real Steam Deck controller whenever usbip-win2 is installed.
    SteamDeck,
    /// Always the VHF pad, with back grips mapped by the `back_grip_*` settings.
    VirtualPad,
}
pub fn deck_pad(value: &str) -> DeckPad {
    match value {
        "auto" => DeckPad::Auto,
        "steam_deck" => DeckPad::SteamDeck,
        "virtual_pad" => DeckPad::VirtualPad,
        other => {
            crate::config::fallback("steam_deck_controller", other, "auto");
            DeckPad::Auto
        }
    }
}

/// What a back grip presses on the virtual pad, none of which has back grips.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GripTarget {
    /// Moonlight button flags.
    Buttons(u32),
    LeftTrigger,
    RightTrigger,
}
/// The `back_grip_*` setting `key`; `none` and unknown values leave the grip
/// unmapped.
pub fn grip_target(key: &str, value: &str) -> Option<GripTarget> {
    use GripTarget::*;
    Some(match value {
        "a" => Buttons(0x1000),
        "b" => Buttons(0x2000),
        "x" => Buttons(0x4000),
        "y" => Buttons(0x8000),
        "lb" => Buttons(0x0100),
        "rb" => Buttons(0x0200),
        "lt" => LeftTrigger,
        "rt" => RightTrigger,
        "l3" => Buttons(0x0040),
        "r3" => Buttons(0x0080),
        "back" => Buttons(0x0020),
        "start" => Buttons(0x0010),
        "guide" => Buttons(0x0400),
        "dpad_up" => Buttons(0x0001),
        "dpad_down" => Buttons(0x0002),
        "dpad_left" => Buttons(0x0004),
        "dpad_right" => Buttons(0x0008),
        "touchpad" => Buttons(0x10_0000),
        "misc" => Buttons(0x20_0000),
        other => {
            crate::config::fallback(key, other, "none");
            return None;
        }
    })
}

/// The stream-card hint for a Steam Deck whose controller reaches Moonlight
/// through Steam Input: Moonlight then sees Steam's virtual pad, without the
/// gyro, trackpads and back grips, and so does the host. SDL may still name
/// it a Steam Deck (Steam tells it the real controller), so only the
/// missing sensors and the device's name give such a Deck away.
pub fn steam_input_hint(device: &str, capabilities: u16) -> Option<String> {
    let name: String = device
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect();
    (name.contains("steamdeck") && capabilities & 0x30 == 0).then(|| {
        format!(
            "{device} looks like a Steam Deck whose controls reach Moonlight through Steam Input, so the host gets its buttons and sticks but no gyro, trackpads or back grips. To pass them on, set Moonlight's controller settings in Steam on the Deck to disable Steam Input, then reconnect."
        )
    })
}

/// The `gamepad` setting's profile. The retired ViGEmBus choices `x360` and
/// `ds4`, which Sunshine, Apollo and Vibepollo profiles can carry, select the
/// VHF pad of the same family (see [`crate::config::RETIRED_VALUES`]).
pub fn gamepad_profile(value: &str) -> u16 {
    match value {
        "vhf_xbox_one" | "x360" => VHF_XBOX_ONE,
        "vhf_xbox" => VHF_XBOX_SERIES,
        "vhf_ds4" | "ds4" => VHF_DS4,
        "vhf_ds5" | "ds5" => VHF_DUALSENSE,
        "usbip_ds5" => USBIP_DUALSENSE,
        "usbip_switch2" => USBIP_SWITCH2,
        "vhf_switch" => VHF_SWITCH,
        "vhf_steam" => VHF_STEAM_CONTROLLER,
        "vhf" => VHF_AUTO,
        "vigem_x360" => VIGEM_X360,
        "vigem_ds4" => VIGEM_DS4,
        other => {
            crate::config::fallback("gamepad", other, "auto");
            0
        }
    }
}

/// `mouse_mode`: how the host moves the PC's mouse and presses its buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseMode {
    /// The virtual USB mouse while it is plugged in, Windows' injected input otherwise:
    /// Foundation Sunshine's default ("use the driver if available").
    Auto,
    /// The virtual USB mouse; until it is plugged in, and where it cannot be, injected input,
    /// with a warning.
    VirtualMouse,
    /// Windows' injected input (SendInput) only.
    Injection,
}
impl MouseMode {
    /// The setting's value; Foundation Sunshine's `vmouse` and `sendinput` names are read too.
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value.trim().to_ascii_lowercase().as_str() {
            "auto" | "" => Self::Auto,
            "virtual_mouse" | "vmouse" | "virtual" => Self::VirtualMouse,
            "injection" | "sendinput" | "send_input" => Self::Injection,
            _ => return None,
        })
    }
    pub fn resolve(config: &Config) -> Self {
        let value = config.get("mouse_mode", "auto");
        Self::parse(value).unwrap_or_else(|| {
            crate::config::invalid("mouse_mode", value);
            Self::Auto
        })
    }
    /// Whether the virtual mouse is wanted at all.
    pub fn wants_virtual(self) -> bool {
        self != Self::Injection
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct Policy {
    pub keyboard: bool,
    pub mouse: bool,
    pub controller: bool,
    pub native_pen_touch: bool,
    /// `mouse_mode`: the virtual USB mouse or injected input.
    pub mouse_mode: MouseMode,
    /// `native_touchpad`: a tablet's physical touchpad replays on the virtual Precision
    /// Touchpad.
    pub native_touchpad: bool,
    /// The device's touch profile: native touches as sent, or as a mouse or trackpad.
    pub touch: crate::touch_profile::Settings,
    pub always_send_scancodes: bool,
    pub high_resolution_scrolling: bool,
    pub forward_rumble: bool,
    pub motion_as_ds4: bool,
    pub touchpad_as_ds4: bool,
    /// `ds4_back_as_touchpad_click`: Back also clicks the touchpad of a
    /// DualShock 4 chosen by name on ViGEmBus, for controllers without a
    /// touchpad button, as Vibepollo's manual DS4 does.
    pub ds4_back_as_touchpad_click: bool,
    /// `gamepad_feedback_fast_poll`: poll pad feedback every 2 ms for a
    /// moment after feedback came, not every 8 ms. Off until measured.
    pub feedback_fast_poll: bool,
    /// `gamepad_driver_host_priority`: the gamepad driver's WUDFHost at high
    /// priority while a VHF pad is plugged. Off until measured: it changes
    /// another process.
    pub driver_host_priority: bool,
    /// None: the host does not repeat held keys.
    pub repeat_delay: Option<Duration>,
    pub repeat_period: Duration,
    /// How long a left release waits after absolute input, so a long press can
    /// right-click first (`absolute_left_release_delay`); zero sends it at once.
    pub left_release_delay: Duration,
    pub back_button_timeout: Option<Duration>,
    /// What each back grip presses, in [`PADDLES`] order.
    pub back_grips: [Option<GripTarget>; 4],
    pub steam_deck: DeckPad,
    /// The profile of a Nucleus Co-op session's pads: Xbox pads whatever the
    /// clients' controllers are, since only those reach XInput, and
    /// ViGEmBus's Xbox 360 pad when `gamepad` chose a ViGEmBus pad, so a tool
    /// that needs the USB pad still sees one.
    pub session_pad: u16,
    pub keybindings: BTreeMap<u16, u16>,
}
impl Policy {
    pub fn resolve(config: &Config) -> Result<Self> {
        let mut keybindings = BTreeMap::from([(0x10, 0xa0), (0x11, 0xa2), (0x12, 0xa4)]);
        // Pairs of key codes, decimal or hexadecimal as Vibepollo writes them.
        // Unusable mappings keep the defaults instead of disabling input.
        let codes: Option<Vec<u16>> = config
            .list("keybindings")
            .iter()
            .map(|code| crate::config::parse_integer(code).and_then(|n| u16::try_from(n).ok()))
            .collect();
        match codes {
            Some(codes) if codes.len().is_multiple_of(2) => {
                for pair in codes.as_chunks::<2>().0 {
                    keybindings.insert(pair[0], pair[1]);
                }
            }
            _ => crate::config::invalid("keybindings", config.get("keybindings", "")),
        }
        if config.boolean("key_rightalt_to_key_win", false) {
            keybindings.entry(0xa5).or_insert(0x5b);
        }
        let repeat_period = crate::config::number_text(config.get("key_repeat_frequency", "24.9"))
            .parse::<f64>()
            .ok()
            .filter(|f| f.is_finite() && (0.0..=1000.0).contains(f))
            .and_then(|frequency| {
                Duration::try_from_secs_f64(1.0 / if frequency == 0.0 { 24.9 } else { frequency })
                    .ok()
            })
            .filter(|period| std::time::Instant::now().checked_add(*period).is_some())
            .unwrap_or_else(|| {
                crate::config::invalid(
                    "key_repeat_frequency",
                    config.get("key_repeat_frequency", ""),
                );
                Duration::from_secs_f64(1.0 / 24.9)
            });
        Ok(Self {
            keyboard: config.boolean("keyboard", true),
            mouse: config.boolean("mouse", true),
            controller: config.boolean("controller", true),
            native_pen_touch: config.boolean("native_pen_touch", true),
            mouse_mode: MouseMode::resolve(config),
            native_touchpad: config.boolean("native_touchpad", true),
            touch: crate::touch_profile::Settings::resolve(config),
            always_send_scancodes: config.boolean("always_send_scancodes", true),
            high_resolution_scrolling: config.boolean("high_resolution_scrolling", true),
            forward_rumble: config.boolean("forward_rumble", true),
            motion_as_ds4: config.boolean("motion_as_ds4", true),
            touchpad_as_ds4: config.boolean("touchpad_as_ds4", true),
            ds4_back_as_touchpad_click: config.boolean("ds4_back_as_touchpad_click", true),
            feedback_fast_poll: config.boolean("gamepad_feedback_fast_poll", false),
            driver_host_priority: config.boolean("gamepad_driver_host_priority", false),
            // As in Vibepollo: 0 turns host key repeat off and a negative
            // value keeps the default. Clamping 0 to an immediate repeat
            // typed two or three characters per tap for imported profiles
            // that had turned repeat off.
            repeat_delay: match config.integer("key_repeat_delay", 500) {
                0 => None,
                ms if ms < 0 => Some(Duration::from_millis(500)),
                ms => Some(Duration::from_millis(ms.min(60000) as u64)),
            },
            repeat_period,
            // Sunshine's 10 ms; a negative value keeps it, as for key repeat.
            left_release_delay: Duration::from_millis(
                match config.integer("absolute_left_release_delay", 10) {
                    ms if ms < 0 => 10,
                    ms => ms.min(1000) as u64,
                },
            ),
            back_button_timeout: back_button_timeout(config),
            back_grips: BACK_GRIP_KEYS.map(|key| grip_target(key, config.get(key, "none"))),
            steam_deck: deck_pad(config.get("steam_deck_controller", "auto")),
            session_pad: match config.get("gamepad", "auto") {
                "vigem_x360" | "vigem_ds4" => VIGEM_X360,
                _ => VHF_XBOX_SERIES,
            },
            keybindings,
        })
    }
    /// Presses what each mapped back grip is set to press, in place of the
    /// grip. An unmapped grip stays as it came: the driver ignores it.
    pub fn map_back_grips(&self, buttons: u32, left: u8, right: u8) -> (u32, u8, u8) {
        let (mut buttons, mut left, mut right) = (buttons, left, right);
        let held = buttons;
        for (flag, target) in PADDLES.into_iter().zip(self.back_grips) {
            let Some(target) = target else { continue };
            buttons &= !flag;
            if held & flag != 0 {
                match target {
                    GripTarget::Buttons(pressed) => buttons |= pressed,
                    GripTarget::LeftTrigger => left = u8::MAX,
                    GripTarget::RightTrigger => right = u8::MAX,
                }
            }
        }
        (buttons, left, right)
    }
    /// The back grips no setting maps, as Moonlight button flags.
    pub fn unmapped_back_grips(&self) -> u32 {
        PADDLES
            .into_iter()
            .zip(self.back_grips)
            .filter(|(_, target)| target.is_none())
            .fold(0, |mask, (flag, _)| mask | flag)
    }
    pub fn allows(&self, event: &Input) -> bool {
        match event {
            Input::Keyboard { .. } | Input::Text(_) => self.keyboard,
            Input::Relative { .. }
            | Input::Absolute { .. }
            | Input::MouseButton { .. }
            | Input::Scroll { .. } => self.mouse,
            Input::Pen { .. } | Input::Touch { .. } => self.mouse && self.native_pen_touch,
            Input::Touchpad { .. } => self.mouse && self.native_touchpad,
            _ => self.controller,
        }
    }
    pub fn key(&self, key: u16) -> u16 {
        self.keybindings.get(&key).copied().unwrap_or(key)
    }
    pub fn controller_profile(
        &self,
        configured: u16,
        kind: u8,
        capabilities: u16,
        available: u32,
    ) -> Option<u16> {
        let desired = if configured != 0 {
            configured
        } else if kind == 2 {
            VHF_DUALSENSE
        } else if kind == 3 {
            VHF_SWITCH
        } else if (self.motion_as_ds4 && capabilities & 0x30 != 0)
            || (self.touchpad_as_ds4 && capabilities & 8 != 0)
        {
            // As in Vibepollo, a controller of any other type with motion
            // sensors or a touchpad becomes a DualSense, an Xbox-type one too
            // (a Steam Deck), so its gyro and touchpad keep working.
            VHF_DUALSENSE
        } else {
            0
        };
        // A controller with motion sensors whose profile the driver lacks
        // (an older driver without the Switch Pro) keeps its gyro on a
        // DualSense or DS4 rather than becoming an Xbox pad.
        let fallbacks = if configured == 0 && desired != 0 && capabilities & 0x30 != 0 {
            [desired, VHF_DUALSENSE, VHF_DS4, VHF_XBOX_SERIES]
        } else {
            [desired, VHF_XBOX_SERIES, VHF_DUALSENSE, VHF_DS4]
        };
        fallbacks
            .into_iter()
            .find(|p| *p != 0 && available & (1 << (p - 1)) != 0)
    }
}

/// Whether a controller becomes a Switch 2 Pro Controller over usbip-win2:
/// `configured` (the host's or the device's `gamepad`) chose it by name, or
/// it is automatic and the client said the controller is a Switch 2 Pro
/// Controller (`identity`, USB vendor and product). Every other controller
/// keeps the pad it had.
pub fn wants_switch2(configured: u16, identity: Option<(u16, u16)>) -> bool {
    configured == USBIP_SWITCH2
        || (matches!(configured, 0 | VHF_AUTO)
            && identity.is_some_and(|(vendor, product)| {
                rubylight_protocol::controller_identity::is_switch2_pro(vendor, product)
            }))
}

/// The back grips (Moonlight's paddle flags) a controller's virtual pad has itself, so pressing
/// one unmapped reaches the game: all four on the Steam Controller, the upper two (GR and GL,
/// `switch2.rs`) on a Switch 2 Pro Controller (`identity` as for [`wants_switch2`]), none on
/// the other pads.
pub fn pad_back_grips(profile: u16, identity: Option<(u16, u16)>) -> u32 {
    if profile == VHF_STEAM_CONTROLLER {
        PADDLES.iter().fold(0, |mask, flag| mask | flag)
    } else if wants_switch2(profile, identity) {
        PADDLES[0] | PADDLES[1]
    } else {
        0
    }
}

/// `back_button_timeout`: how long Select is held before the host presses
/// Guide (see [`BackButton`]), `None` when off (any negative value).
pub fn back_button_timeout(config: &Config) -> Option<Duration> {
    u64::try_from(config.integer("back_button_timeout", -1))
        .ok()
        .map(|ms| Duration::from_millis(ms.min(60000)))
}

/// Back-to-Home emulation, advanced by the input worker without blocking it.
#[derive(Default)]
pub struct BackButton {
    pressed: Option<Duration>,
    home_until: Option<Duration>,
    suppressed: bool,
    physical: u32,
    output: u32,
}
impl BackButton {
    pub fn update(&mut self, buttons: u32, now: Duration, timeout: Option<Duration>) -> u32 {
        self.physical = buttons;
        if buttons & 0x20 == 0 {
            self.pressed = None;
            self.suppressed = false;
        } else if !self.suppressed && self.pressed.is_none() {
            self.pressed = Some(now);
        }
        self.advance(now, timeout);
        self.output
    }
    fn advance(&mut self, now: Duration, timeout: Option<Duration>) {
        if !self.suppressed
            && self
                .pressed
                .zip(timeout)
                .is_some_and(|(start, delay)| now.saturating_sub(start) >= delay)
        {
            self.suppressed = true;
            self.pressed = None;
            self.home_until = Some(now + Duration::from_millis(100));
        }
        if self.home_until.is_some_and(|until| now >= until) {
            self.home_until = None;
        }
        self.output = self.physical & if self.suppressed { !0x20 } else { u32::MAX };
        if self.home_until.is_some() {
            self.output |= 0x400;
        }
    }
    pub fn poll(&mut self, now: Duration, timeout: Option<Duration>) -> Option<u32> {
        let before = self.output;
        self.advance(now, timeout);
        (before != self.output).then_some(self.output)
    }
}
#[cfg(test)]
mod tests;
