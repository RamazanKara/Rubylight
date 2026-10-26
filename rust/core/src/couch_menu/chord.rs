//! The ways to open the menu (`docs/couch-menu-design.md` section 3.2): a
//! button hold on one pad (L3+R3 by default), a keyboard chord
//! (Ctrl+Alt+Backspace by default), and a hold in the stream's top-left
//! corner. Time is the host's receive time; a pending hold has a deadline
//! that the control loop wakes for, because Moonlight sends controller state
//! only when it changes.
use std::fmt;
use std::ops::RangeInclusive;
use std::time::{Duration, Instant};

use crate::config::Config;
use crate::input::Input;

/// Moonlight's stick-click button flags.
pub const LS_CLK: u32 = 0x0040;
pub const RS_CLK: u32 = 0x0080;
/// The default pad chord: a pad's buttons must be exactly these, with sticks
/// and triggers ignored.
pub const PAD_CHORD: u32 = LS_CLK | RS_CLK;
pub const PAD_HOLD: Duration = Duration::from_millis(1000);
pub const CORNER_HOLD: Duration = Duration::from_millis(1500);
/// The holds `couch_menu_hold_ms` and `couch_menu_corner_hold_ms` may set.
pub const PAD_HOLD_MS: RangeInclusive<u64> = 300..=3000;
pub const CORNER_HOLD_MS: RangeInclusive<u64> = 500..=3000;
/// The corner box's side, as a share of the stream's shorter side.
pub const CORNER_SHARE: f32 = 0.06;

pub const VK_BACK: u16 = 0x08;
/// Moonlight's keyboard modifier flags.
pub const MODIFIER_SHIFT: u8 = 0x01;
pub const MODIFIER_CTRL: u8 = 0x02;
pub const MODIFIER_ALT: u8 = 0x04;

/// Moonlight's touch and pen events.
pub(crate) const TOUCH_DOWN: u8 = 1;
pub(crate) const TOUCH_UP: u8 = 2;
pub(crate) const TOUCH_MOVE: u8 = 3;
pub(crate) const TOUCH_CANCEL: u8 = 4;
pub(crate) const TOUCH_CANCEL_ALL: u8 = 7;
pub(crate) const MOUSE_LEFT: u8 = 1;

/// The settings that say how the menu opens, in the order the console lists
/// them.
pub const SETTINGS: [&str; 5] = [
    "couch_menu_open_with",
    "couch_menu_controller_chord",
    "couch_menu_hold_ms",
    "couch_menu_corner_hold_ms",
    "couch_menu_keyboard_chord",
];

/// `couch_menu_open_with`, which ways open the menu, and the chords and holds
/// they use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OpenWith {
    pub controller: bool,
    pub keyboard: bool,
    /// Touch, pen and mouse holds in the corner.
    pub touch: bool,
    /// `couch_menu_controller_chord`.
    pub pad_chord: PadButtons,
    /// `couch_menu_hold_ms`.
    pub pad_hold: Duration,
    /// `couch_menu_corner_hold_ms`.
    pub corner_hold: Duration,
    /// `couch_menu_keyboard_chord`.
    pub key_chord: KeyChord,
}

impl Default for OpenWith {
    fn default() -> Self {
        Self {
            controller: true,
            keyboard: true,
            touch: true,
            pad_chord: PadButtons::default(),
            pad_hold: PAD_HOLD,
            corner_hold: CORNER_HOLD,
            key_chord: KeyChord::default(),
        }
    }
}

impl OpenWith {
    /// The settings as `config` has them. A value the console would refuse
    /// (a hand-edited file) is logged and its default used.
    pub fn from_config(config: &Config) -> Self {
        Self::read(config, &mut crate::config::invalid)
    }
    /// [`OpenWith::from_config`], reporting each refused key and value to
    /// `refused`.
    fn read(config: &Config, refused: &mut dyn FnMut(&str, &str)) -> Self {
        let mut out = Self::default();
        let value = |key: &str| {
            let value = config.values.get(key)?;
            // A blank value is unset, except for the list, where it turns
            // every way off.
            (key == "couch_menu_open_with" || !value.trim().is_empty()).then_some(value.as_str())
        };
        if let Some(v) = value("couch_menu_open_with") {
            match Self::parse(v) {
                Ok(ways) => {
                    out.controller = ways.controller;
                    out.keyboard = ways.keyboard;
                    out.touch = ways.touch;
                }
                Err(_) => refused("couch_menu_open_with", v),
            }
        }
        if let Some(v) = value("couch_menu_controller_chord") {
            match PadButtons::parse(v) {
                Ok(chord) => out.pad_chord = chord,
                Err(_) => refused("couch_menu_controller_chord", v),
            }
        }
        if let Some(v) = value("couch_menu_hold_ms") {
            match parse_hold("couch_menu_hold_ms", v, PAD_HOLD_MS) {
                Ok(hold) => out.pad_hold = hold,
                Err(_) => refused("couch_menu_hold_ms", v),
            }
        }
        if let Some(v) = value("couch_menu_corner_hold_ms") {
            match parse_hold("couch_menu_corner_hold_ms", v, CORNER_HOLD_MS) {
                Ok(hold) => out.corner_hold = hold,
                Err(_) => refused("couch_menu_corner_hold_ms", v),
            }
        }
        if let Some(v) = value("couch_menu_keyboard_chord") {
            match KeyChord::parse(v) {
                Ok(chord) => out.key_chord = chord,
                Err(_) => refused("couch_menu_keyboard_chord", v),
            }
        }
        out
    }
    /// Parses the comma-separated list; an empty value turns every way off.
    /// Unknown entries are an error, so a typo fails loudly. The chords and
    /// holds keep their defaults.
    pub fn parse(value: &str) -> Result<Self, String> {
        let mut out = Self {
            controller: false,
            keyboard: false,
            touch: false,
            ..Self::default()
        };
        for entry in value.split(',').map(str::trim).filter(|e| !e.is_empty()) {
            match entry.to_ascii_lowercase().as_str() {
                "controller" => out.controller = true,
                "keyboard" => out.keyboard = true,
                "touch" => out.touch = true,
                _ => {
                    return Err(format!(
                        "couch_menu_open_with: unknown entry {entry:?} \
                         (use controller, keyboard, touch)"
                    ));
                }
            }
        }
        Ok(out)
    }
}

/// Checks a value for one of [`SETTINGS`] as the console saves it. `Ok`
/// carries a warning to log when the value works but may not reach the host.
/// Other keys pass.
pub fn check(key: &str, value: &str) -> Result<Option<&'static str>, String> {
    match key {
        "couch_menu_open_with" => OpenWith::parse(value).map(|_| None),
        "couch_menu_controller_chord" => PadButtons::parse(value).map(PadButtons::warning),
        "couch_menu_hold_ms" => parse_hold(key, value, PAD_HOLD_MS).map(|_| None),
        "couch_menu_corner_hold_ms" => parse_hold(key, value, CORNER_HOLD_MS).map(|_| None),
        "couch_menu_keyboard_chord" => KeyChord::parse(value).map(|_| None),
        _ => Ok(None),
    }
}

/// A hold in milliseconds within `range`.
pub fn parse_hold(key: &str, value: &str, range: RangeInclusive<u64>) -> Result<Duration, String> {
    crate::config::parse_integer(value)
        .and_then(|ms| u64::try_from(ms).ok())
        .filter(|ms| range.contains(ms))
        .map(Duration::from_millis)
        .ok_or_else(|| {
            format!(
                "{key} must be between {} and {} milliseconds",
                range.start(),
                range.end()
            )
        })
}

/// Moonlight's button flags by the names `couch_menu_controller_chord` uses,
/// in the order a chord is written back.
const BUTTONS: [(&str, u32); 21] = [
    ("up", 0x0001),
    ("down", 0x0002),
    ("left", 0x0004),
    ("right", 0x0008),
    ("start", START),
    ("select", SELECT),
    ("l3", LS_CLK),
    ("r3", RS_CLK),
    ("l1", L1),
    ("r1", R1),
    ("guide", GUIDE),
    ("a", 0x1000),
    ("b", 0x2000),
    ("x", X),
    ("y", 0x8000),
    ("paddle1", 0x01_0000),
    ("paddle2", 0x02_0000),
    ("paddle3", 0x04_0000),
    ("paddle4", 0x08_0000),
    ("touchpad", 0x10_0000),
    ("misc", 0x20_0000),
];
const START: u32 = 0x0010;
const SELECT: u32 = 0x0020;
const L1: u32 = 0x0100;
const R1: u32 = 0x0200;
const GUIDE: u32 = 0x0400;
const X: u32 = 0x4000;
/// Buttons Moonlight clients take for themselves, most specific first. A
/// chord holding all of one set never reaches the host as a hold.
const RESERVED: [(u32, &str); 6] = [
    (
        START | SELECT | L1 | R1,
        "Start+Select+L1+R1 quits the stream in every Moonlight client",
    ),
    (
        START | SELECT,
        "Moonlight turns Start+Select into Guide on pads without a Home button",
    ),
    (
        START,
        "Moonlight toggles mouse mode when Start is held for 750 ms, so no hold can include Start",
    ),
    (
        SELECT | L1 | R1 | X,
        "Select+L1+R1+X shows Moonlight's statistics",
    ),
    (
        SELECT | GUIDE,
        "moonlight-qt and Moonlight Android turn Select+Guide into a clickpad press",
    ),
    (
        SELECT | L1,
        "moonlight-qt and Moonlight Android turn Select+L1 into a clickpad press",
    ),
];

/// `couch_menu_controller_chord`: the buttons a pad must hold, exactly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PadButtons(u32);

impl Default for PadButtons {
    fn default() -> Self {
        Self(PAD_CHORD)
    }
}

impl PadButtons {
    /// Parses a `+`-joined button list such as `l3+r3` or `select`. Unknown
    /// buttons and the combinations Moonlight clients keep are an error.
    pub fn parse(value: &str) -> Result<Self, String> {
        if value.trim().is_empty() {
            return Err("couch_menu_controller_chord needs at least one button".into());
        }
        let mut buttons = 0;
        for name in value.split('+').map(str::trim) {
            let name = name.to_ascii_lowercase();
            let Some(&(_, bit)) = BUTTONS.iter().find(|(known, _)| *known == name) else {
                return Err(format!(
                    "couch_menu_controller_chord: unknown button {name:?} in {value:?} \
                     (use up, down, left, right, start, select, l3, r3, l1, r1, guide, \
                     a, b, x, y, paddle1 to paddle4, touchpad, misc; triggers cannot hold)"
                ));
            };
            buttons |= bit;
        }
        let chord = Self(buttons);
        if let Some((_, reason)) = RESERVED.iter().find(|(set, _)| buttons & set == *set) {
            return Err(format!(
                "couch_menu_controller_chord: {chord} cannot open the menu: {reason}"
            ));
        }
        Ok(chord)
    }
    pub fn buttons(self) -> u32 {
        self.0
    }
    /// Why the chord may not reach the host although it is allowed.
    pub fn warning(self) -> Option<&'static str> {
        (self.0 & GUIDE != 0).then_some(
            "couch_menu_controller_chord: many clients hand Guide to the OS \
             (Android TV home, the Steam button, iOS), so a chord with Guide may never reach the host",
        )
    }
}

impl fmt::Display for PadButtons {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut names = BUTTONS
            .iter()
            .filter(|(_, bit)| self.0 & bit != 0)
            .map(|(name, _)| name);
        if let Some(first) = names.next() {
            f.write_str(first)?;
        }
        names.try_for_each(|name| write!(f, "+{name}"))
    }
}

/// The keyboard chords `couch_menu_keyboard_chord` offers, all Ctrl+Alt: no
/// Moonlight client takes them, and AltGr (Ctrl+Alt on Windows) types no
/// character with these keys, as it does with letters on many layouts.
pub const KEY_CHORDS: [(&str, u16); 4] = [
    ("ctrl+alt+backspace", VK_BACK),
    ("ctrl+alt+home", 0x24),
    ("ctrl+alt+end", 0x23),
    ("ctrl+alt+insert", 0x2d),
];

/// `couch_menu_keyboard_chord`: one of [`KEY_CHORDS`], the first by default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KeyChord(usize);

impl KeyChord {
    /// Parses one of [`KEY_CHORDS`], in any case and with spaces around `+`.
    pub fn parse(value: &str) -> Result<Self, String> {
        let parts: Vec<String> = value
            .split('+')
            .map(|part| part.trim().to_ascii_lowercase())
            .collect();
        if ["ctrl", "alt", "shift"]
            .iter()
            .all(|m| parts.iter().any(|part| part == m))
        {
            return Err(format!(
                "couch_menu_keyboard_chord: {value:?} cannot open the menu: \
                 Moonlight keeps Ctrl+Alt+Shift for its own shortcuts"
            ));
        }
        let written = parts.join("+");
        KEY_CHORDS
            .iter()
            .position(|(name, _)| *name == written)
            .map(Self)
            .ok_or_else(|| {
                let names: Vec<&str> = KEY_CHORDS.iter().map(|(name, _)| *name).collect();
                format!(
                    "couch_menu_keyboard_chord: unknown chord {value:?} (use {})",
                    names.join(", ")
                )
            })
    }
    pub fn key(self) -> u16 {
        KEY_CHORDS[self.0].1
    }
    /// The chord's key down with Ctrl and Alt, without Shift. Meta does not
    /// matter.
    pub fn matches(self, event: &Input) -> bool {
        matches!(*event, Input::Keyboard { key, modifiers, down: true, .. }
            if key == self.key()
                && modifiers & (MODIFIER_CTRL | MODIFIER_ALT) == MODIFIER_CTRL | MODIFIER_ALT
                && modifiers & MODIFIER_SHIFT == 0)
    }
}

impl fmt::Display for KeyChord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(KEY_CHORDS[self.0].0)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum PadState {
    #[default]
    Idle,
    Armed(Instant),
    /// The hold fired; it arms again only after the buttons change.
    Fired,
}

/// The chord's hold on each of the 16 pads.
#[derive(Debug)]
pub struct PadChord {
    pads: [PadState; 16],
    chord: u32,
    hold: Duration,
}

impl Default for PadChord {
    fn default() -> Self {
        Self::new(PadButtons::default(), PAD_HOLD)
    }
}

impl PadChord {
    pub fn new(chord: PadButtons, hold: Duration) -> Self {
        Self {
            pads: [PadState::Idle; 16],
            chord: chord.buttons(),
            hold,
        }
    }
    /// A pad's new button state: exactly the chord arms it, anything else
    /// (another button, a chord button released) cancels it.
    pub fn update(&mut self, id: u16, buttons: u32, now: Instant) {
        let Some(pad) = self.pads.get_mut(usize::from(id)) else {
            return;
        };
        if buttons != self.chord {
            *pad = PadState::Idle;
        } else if *pad == PadState::Idle {
            *pad = PadState::Armed(now + self.hold);
        }
    }
    /// Fires the first hold due at `now`; that pad then waits for a change.
    pub fn fire(&mut self, now: Instant) -> bool {
        for pad in &mut self.pads {
            if let PadState::Armed(deadline) = *pad
                && deadline <= now
            {
                *pad = PadState::Fired;
                return true;
            }
        }
        false
    }
    /// Cancels every pending hold. A pad that was armed then waits for a
    /// button change like one that fired: going back to idle would re-arm it
    /// on its next report and fire again a second later.
    pub fn cancel(&mut self) {
        for pad in &mut self.pads {
            if matches!(pad, PadState::Armed(_)) {
                *pad = PadState::Fired;
            }
        }
    }
    pub fn next_due(&self) -> Option<Instant> {
        self.pads
            .iter()
            .filter_map(|pad| match pad {
                PadState::Armed(deadline) => Some(*deadline),
                _ => None,
            })
            .min()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Source {
    Touch(u32),
    Pen,
    /// A mouse button at the last absolute position.
    Absolute,
    /// A mouse button with a relative mouse: the host cursor is read only
    /// when the hold fires.
    Relative,
}

/// The press-and-hold in the top-left corner box.
#[derive(Debug)]
pub struct CornerHold {
    /// The box's size in stream coordinates (`0..=1`).
    box_w: f32,
    box_h: f32,
    /// The last absolute mouse position, `None` once the mouse moves
    /// relatively.
    last_absolute: Option<(f32, f32)>,
    /// How long a hold takes.
    duration: Duration,
    hold: Option<(Source, Instant)>,
}

impl CornerHold {
    /// For a stream of `width`×`height` target pixels, with holds of `duration`.
    pub fn new(width: u32, height: u32, duration: Duration) -> Self {
        let side = CORNER_SHARE * width.min(height).max(1) as f32;
        Self {
            box_w: side / width.max(1) as f32,
            box_h: side / height.max(1) as f32,
            last_absolute: None,
            duration,
            hold: None,
        }
    }
    pub fn inside(&self, (x, y): (f32, f32)) -> bool {
        (0.0..self.box_w).contains(&x) && (0.0..self.box_h).contains(&y)
    }
    /// The last absolute mouse position, for clicks while the menu is open.
    pub fn last_absolute(&self) -> Option<(f32, f32)> {
        self.last_absolute
    }
    /// Watches one event. `arm` is false while the menu is open or the touch
    /// way is off: positions are still followed, no hold starts.
    pub fn observe(&mut self, event: &Input, now: Instant, arm: bool) {
        let deadline = now + self.duration;
        match *event {
            Input::Absolute {
                x,
                y,
                width,
                height,
            } => {
                let p = (
                    f32::from(x) / f32::from(width.max(1)),
                    f32::from(y) / f32::from(height.max(1)),
                );
                self.last_absolute = Some(p);
                match self.hold {
                    Some((Source::Absolute, _)) if !self.inside(p) => self.hold = None,
                    Some((Source::Relative, _)) => self.hold = None,
                    _ => {}
                }
            }
            Input::Relative { .. } => {
                self.last_absolute = None;
                if matches!(self.hold, Some((Source::Absolute, _))) {
                    self.hold = None;
                }
            }
            Input::MouseButton { button, down } => {
                let mouse_held =
                    matches!(self.hold, Some((Source::Absolute | Source::Relative, _)));
                if button != MOUSE_LEFT || !down {
                    // Releasing the button or pressing another ends a mouse hold.
                    if mouse_held {
                        self.hold = None;
                    }
                } else if arm && self.hold.is_none() {
                    self.hold = match self.last_absolute {
                        Some(p) if self.inside(p) => Some((Source::Absolute, deadline)),
                        Some(_) => None,
                        None => Some((Source::Relative, deadline)),
                    };
                }
            }
            Input::Touch {
                event: touch,
                id,
                x,
                y,
                ..
            } => match (touch, self.hold) {
                (TOUCH_DOWN, None) if arm && self.inside((x, y)) => {
                    self.hold = Some((Source::Touch(id), deadline));
                }
                // A second finger is a gesture, not a hold.
                (TOUCH_DOWN, Some(_)) => self.hold = None,
                (TOUCH_MOVE, Some((Source::Touch(held), _)))
                    if held == id && !self.inside((x, y)) =>
                {
                    self.hold = None;
                }
                (TOUCH_UP | TOUCH_CANCEL, Some((Source::Touch(held), _))) if held == id => {
                    self.hold = None;
                }
                (TOUCH_CANCEL_ALL, Some((Source::Touch(_), _))) => self.hold = None,
                _ => {}
            },
            Input::Pen {
                event: pen, x, y, ..
            } => match (pen, self.hold) {
                (TOUCH_DOWN, None) if arm && self.inside((x, y)) => {
                    self.hold = Some((Source::Pen, deadline));
                }
                (TOUCH_MOVE, Some((Source::Pen, _))) if !self.inside((x, y)) => {
                    self.hold = None;
                }
                (TOUCH_UP | TOUCH_CANCEL | TOUCH_CANCEL_ALL, Some((Source::Pen, _))) => {
                    self.hold = None;
                }
                _ => {}
            },
            _ => {}
        }
    }
    /// Fires a hold due at `now`. Only a relative-mouse hold asks `pointer`
    /// for the host cursor's stream position, and only here.
    pub fn fire(&mut self, now: Instant, pointer: &mut dyn FnMut() -> Option<(f32, f32)>) -> bool {
        match self.hold {
            Some((source, deadline)) if deadline <= now => {
                self.hold = None;
                source != Source::Relative || pointer().is_some_and(|p| self.inside(p))
            }
            _ => false,
        }
    }
    pub fn cancel(&mut self) {
        self.hold = None;
    }
    pub fn next_due(&self) -> Option<Instant> {
        self.hold.map(|(_, deadline)| deadline)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: Duration = Duration::from_millis(1);

    fn key(key: u16, modifiers: u8, down: bool) -> Input {
        Input::Keyboard {
            key,
            modifiers,
            flags: 0,
            down,
        }
    }
    fn touch(event: u8, id: u32, x: f32, y: f32) -> Input {
        Input::Touch {
            event,
            id,
            x,
            y,
            pressure: 1.,
            major: 0.,
            minor: 0.,
            rotation: 0,
        }
    }
    fn pen(event: u8, x: f32, y: f32) -> Input {
        Input::Pen {
            event,
            tool: 1,
            buttons: 0,
            x,
            y,
            pressure: 1.,
            rotation: 0,
            tilt: 0,
        }
    }
    fn absolute(x: i16, y: i16) -> Input {
        Input::Absolute {
            x,
            y,
            width: 1920,
            height: 1080,
        }
    }
    fn button(down: bool) -> Input {
        Input::MouseButton {
            button: MOUSE_LEFT,
            down,
        }
    }

    #[test]
    fn couch_menu_pad_chord_fires_at_1000_ms_not_999() {
        let t0 = Instant::now();
        let mut chord = PadChord::default();
        chord.update(2, LS_CLK, t0);
        chord.update(2, PAD_CHORD, t0);
        assert_eq!(chord.next_due(), Some(t0 + 1000 * MS));
        // Stick moves resend the same buttons: the deadline stays.
        chord.update(2, PAD_CHORD, t0 + 500 * MS);
        assert!(!chord.fire(t0 + 999 * MS));
        assert!(chord.fire(t0 + 1000 * MS));
        assert_eq!(chord.next_due(), None);
        assert!(!chord.fire(t0 + 5000 * MS), "fires once per hold");
        chord.update(2, PAD_CHORD, t0 + 5000 * MS);
        assert!(!chord.fire(t0 + 9000 * MS), "still the same hold");
        chord.update(2, 0, t0 + 9000 * MS);
        chord.update(2, PAD_CHORD, t0 + 9000 * MS);
        assert!(chord.fire(t0 + 10_000 * MS), "a new hold fires again");
    }

    #[test]
    fn couch_menu_pad_chord_extra_button_or_release_cancels_per_pad() {
        let t0 = Instant::now();
        let mut chord = PadChord::default();
        chord.update(0, PAD_CHORD, t0);
        chord.update(1, PAD_CHORD, t0 + 100 * MS);
        chord.update(0, PAD_CHORD | 0x1000, t0 + 200 * MS);
        assert_eq!(chord.next_due(), Some(t0 + 1100 * MS), "pad 1 still armed");
        assert!(!chord.fire(t0 + 1099 * MS));
        chord.update(1, RS_CLK, t0 + 1050 * MS);
        assert!(!chord.fire(t0 + 2000 * MS));
        assert_eq!(chord.next_due(), None);
        // Going back to exactly L3+R3 starts a new hold.
        chord.update(0, PAD_CHORD, t0 + 3000 * MS);
        assert!(!chord.fire(t0 + 3999 * MS));
        assert!(chord.fire(t0 + 4000 * MS));
        // Pad ids beyond Moonlight's 16 are ignored, not a panic.
        chord.update(16, PAD_CHORD, t0);
        assert_eq!(chord.next_due(), None);
    }

    #[test]
    fn couch_menu_key_chord_needs_ctrl_alt_without_shift() {
        let ctrl_alt = MODIFIER_CTRL | MODIFIER_ALT;
        assert!(KeyChord::default().matches(&key(VK_BACK, ctrl_alt, true)));
        assert!(
            KeyChord::default().matches(&key(VK_BACK, ctrl_alt | 0x08, true)),
            "Meta does not matter"
        );
        assert!(!KeyChord::default().matches(&key(VK_BACK, ctrl_alt | MODIFIER_SHIFT, true)));
        assert!(!KeyChord::default().matches(&key(VK_BACK, ctrl_alt, false)));
        assert!(!KeyChord::default().matches(&key(VK_BACK, MODIFIER_CTRL, true)));
        assert!(!KeyChord::default().matches(&key(VK_BACK, 0, true)));
        assert!(
            !KeyChord::default().matches(&key(0x2e, ctrl_alt, true)),
            "not Ctrl+Alt+Del"
        );
    }

    #[test]
    fn couch_menu_open_with_parses_the_list_and_rejects_typos() {
        assert_eq!(
            OpenWith::parse("controller,keyboard,touch"),
            Ok(OpenWith::default())
        );
        assert_eq!(
            OpenWith::parse(" Keyboard "),
            Ok(OpenWith {
                controller: false,
                keyboard: true,
                touch: false,
                ..OpenWith::default()
            })
        );
        assert_eq!(
            OpenWith::parse(""),
            Ok(OpenWith {
                controller: false,
                keyboard: false,
                touch: false,
                ..OpenWith::default()
            })
        );
        assert!(
            OpenWith::parse("controler")
                .unwrap_err()
                .contains("controler")
        );
    }

    #[test]
    fn couch_menu_controller_chord_refuses_what_moonlight_takes_and_its_supersets() {
        let refused = |value: &str| PadButtons::parse(value).unwrap_err();
        let quit = refused("start+select+l1+r1");
        let guide = refused("select+start");
        let mouse = refused("start");
        let stats = refused("select+l1+r1+x");
        let clickpad_guide = refused("select+guide");
        let clickpad_l1 = refused("select+l1");
        assert!(quit.contains("quits the stream"), "{quit}");
        assert!(guide.contains("into Guide"), "{guide}");
        assert!(mouse.contains("mouse mode"), "{mouse}");
        assert!(stats.contains("statistics"), "{stats}");
        assert!(clickpad_guide.contains("Select+Guide into a clickpad"));
        assert!(clickpad_l1.contains("Select+L1 into a clickpad"));
        for (superset, message) in [
            ("guide+select+a", &clickpad_guide),
            ("select+l1+r1", &clickpad_l1),
            ("l1+select+up", &clickpad_l1),
            ("start+select+l1+r1+a", &quit),
            ("r1+l1+select+start+l3", &quit),
            ("start+select+y", &guide),
            ("start+select+l1", &guide),
            ("start+y", &mouse),
            ("l3+r3+start", &mouse),
            ("select+l1+r1+x+up", &stats),
            ("x+r1+l1+select+paddle1", &stats),
        ] {
            // The same reason; only the chord's name differs.
            let reason = |m: &str| m.rsplit(": ").next().unwrap().to_owned();
            assert_eq!(reason(&refused(superset)), reason(message), "{superset}");
        }
        // Subsets of a reserved set are fine.
        for value in [
            "select",
            "select+r1",
            "l1+r1+x",
            "select+x",
            "l3+r3",
            "l1+r1+y",
        ] {
            let chord = PadButtons::parse(value).unwrap();
            assert_eq!(chord.warning(), None, "{value}");
        }
        assert!(refused("l2").contains("unknown button \"l2\""));
        assert!(refused("l3++r3").contains("unknown button \"\""));
        assert!(refused(" ").contains("at least one button"));
    }

    #[test]
    fn couch_menu_controller_chord_accepts_guide_with_a_warning() {
        let guide = PadButtons::parse("guide").unwrap();
        assert_eq!(guide.buttons(), GUIDE);
        assert!(guide.warning().unwrap().contains("hand Guide to the OS"));
        assert_eq!(
            check("couch_menu_controller_chord", "guide+a"),
            Ok(guide.warning())
        );
        assert_eq!(check("couch_menu_controller_chord", "l3+r3"), Ok(None));
    }

    #[test]
    fn couch_menu_chords_round_trip_through_their_text() {
        for value in ["l3+r3", "select", "guide", "l1+r1+y", "up+a+paddle4+misc"] {
            let chord = PadButtons::parse(value).unwrap();
            assert_eq!(chord.to_string(), value);
            assert_eq!(PadButtons::parse(&chord.to_string()), Ok(chord));
        }
        assert_eq!(PadButtons::parse(" R3 + L3 ").unwrap().to_string(), "l3+r3");
        assert_eq!(PadButtons::parse("l3+r3"), Ok(PadButtons::default()));
        assert_eq!(PadButtons::default().buttons(), PAD_CHORD);
        for (name, key) in KEY_CHORDS {
            let chord = KeyChord::parse(name).unwrap();
            assert_eq!(chord.to_string(), name);
            assert_eq!(chord.key(), key);
            assert_eq!(KeyChord::parse(&chord.to_string()), Ok(chord));
        }
        assert_eq!(
            KeyChord::parse(" Ctrl + ALT + Home ").unwrap().to_string(),
            "ctrl+alt+home"
        );
        assert_eq!(KeyChord::default().key(), VK_BACK);
        let shift = KeyChord::parse("ctrl+alt+shift+backspace").unwrap_err();
        assert!(shift.contains("Ctrl+Alt+Shift"), "{shift}");
        for value in [
            "ctrl+alt+m",
            "alt+ctrl+backspace",
            "ctrl+backspace",
            "ctrl+alt+delete",
        ] {
            let error = KeyChord::parse(value).unwrap_err();
            assert!(
                error.contains("ctrl+alt+backspace, ctrl+alt+home"),
                "{value}: {error}"
            );
        }
    }

    #[test]
    fn couch_menu_hold_bounds() {
        for (key, low, high) in [
            ("couch_menu_hold_ms", 300, 3000),
            ("couch_menu_corner_hold_ms", 500, 3000),
        ] {
            for ok in [low, high] {
                assert_eq!(check(key, &ok.to_string()), Ok(None), "{key}={ok}");
            }
            for bad in [low - 1, high + 1] {
                let error = check(key, &bad.to_string()).unwrap_err();
                assert_eq!(
                    error,
                    format!("{key} must be between {low} and {high} milliseconds")
                );
            }
            for bad in ["", "-1", "1.5s", "fast"] {
                assert!(check(key, bad).is_err(), "{key}={bad:?}");
            }
        }
        assert_eq!(
            parse_hold("couch_menu_hold_ms", "\"300\"", PAD_HOLD_MS),
            Ok(300 * MS)
        );
    }

    #[test]
    fn couch_menu_single_button_hold_fires_at_its_own_time() {
        let t0 = Instant::now();
        let select = PadButtons::parse("select").unwrap();
        let mut chord = PadChord::new(select, 300 * MS);
        chord.update(0, select.buttons(), t0);
        assert_eq!(chord.next_due(), Some(t0 + 300 * MS));
        assert!(!chord.fire(t0 + 299 * MS));
        assert!(chord.fire(t0 + 300 * MS));
        // It waits for a button change after firing.
        chord.update(0, select.buttons(), t0 + 400 * MS);
        assert!(!chord.fire(t0 + 2000 * MS));
        // An extra button cancels the hold.
        chord.update(0, 0, t0 + 2000 * MS);
        chord.update(0, select.buttons(), t0 + 2000 * MS);
        chord.update(0, select.buttons() | 0x1000, t0 + 2100 * MS);
        assert!(!chord.fire(t0 + 3000 * MS));
        // A cancelled armed pad waits for a change too.
        chord.update(0, select.buttons(), t0 + 3000 * MS);
        chord.cancel();
        chord.update(0, select.buttons(), t0 + 3100 * MS);
        assert_eq!(chord.next_due(), None);
        // L3+R3 is not the chord any more.
        chord.update(1, PAD_CHORD, t0);
        assert_eq!(chord.next_due(), None);
    }

    #[test]
    fn couch_menu_corner_hold_takes_its_own_time() {
        let t0 = Instant::now();
        let mut never = || -> Option<(f32, f32)> { None };
        let mut corner = CornerHold::new(1920, 1080, 500 * MS);
        corner.observe(&touch(TOUCH_DOWN, 1, 0.01, 0.01), t0, true);
        assert!(!corner.fire(t0 + 499 * MS, &mut never));
        assert!(corner.fire(t0 + 500 * MS, &mut never));
    }

    #[test]
    fn couch_menu_open_with_reads_the_settings_and_refuses_hand_edits_loudly() {
        let config = Config::parse(
            "couch_menu_open_with = keyboard\n\
             couch_menu_controller_chord = select+y\n\
             couch_menu_hold_ms = 300\n\
             couch_menu_corner_hold_ms = 3000\n\
             couch_menu_keyboard_chord = ctrl+alt+end\n",
        )
        .unwrap();
        let mut refused = Vec::new();
        let read = OpenWith::read(&config, &mut |k, v| refused.push(format!("{k}={v}")));
        assert_eq!(
            read,
            OpenWith {
                controller: false,
                keyboard: true,
                touch: false,
                pad_chord: PadButtons::parse("select+y").unwrap(),
                pad_hold: 300 * MS,
                corner_hold: 3000 * MS,
                key_chord: KeyChord::parse("ctrl+alt+end").unwrap(),
            }
        );
        assert!(refused.is_empty(), "{refused:?}");
        assert_eq!(
            OpenWith::from_config(&Config::default()),
            OpenWith::default()
        );

        let config = Config::parse(
            "couch_menu_open_with = controler\n\
             couch_menu_controller_chord = start+select\n\
             couch_menu_hold_ms = 299\n\
             couch_menu_corner_hold_ms = 3001\n\
             couch_menu_keyboard_chord = ctrl+alt+shift+m\n",
        )
        .unwrap();
        let mut refused = Vec::new();
        let read = OpenWith::read(&config, &mut |k, v| refused.push(format!("{k}={v}")));
        assert_eq!(read, OpenWith::default(), "every bad value falls back");
        assert_eq!(
            refused,
            [
                "couch_menu_open_with=controler",
                "couch_menu_controller_chord=start+select",
                "couch_menu_hold_ms=299",
                "couch_menu_corner_hold_ms=3001",
                "couch_menu_keyboard_chord=ctrl+alt+shift+m",
            ]
        );
    }

    #[test]
    fn couch_menu_corner_box_is_six_percent_of_the_shorter_side() {
        let corner = CornerHold::new(1920, 1080, CORNER_HOLD);
        // 64.8 px on each axis.
        assert!(corner.inside((64. / 1920., 64. / 1080.)));
        assert!(!corner.inside((65. / 1920., 10. / 1080.)));
        assert!(!corner.inside((10. / 1920., 65. / 1080.)));
        let portrait = CornerHold::new(1080, 2400, CORNER_HOLD);
        assert!(portrait.inside((64. / 1080., 64. / 2400.)));
        assert!(!portrait.inside((64. / 1080., 66. / 2400.)));
    }

    /// Each source: the hold fires at 1500 ms, not 1499, and leaving the box
    /// or letting go cancels it.
    #[test]
    fn couch_menu_corner_hold_for_each_source() {
        let t0 = Instant::now();
        let mut never = || -> Option<(f32, f32)> { panic!("only relative holds read the cursor") };
        let sources: [(&str, Vec<Input>, Input, Input); 3] = [
            (
                "touch",
                vec![touch(TOUCH_DOWN, 7, 0.01, 0.02)],
                touch(TOUCH_MOVE, 7, 0.5, 0.02),
                touch(TOUCH_UP, 7, 0.01, 0.02),
            ),
            (
                "pen",
                vec![pen(TOUCH_DOWN, 0.01, 0.02)],
                pen(TOUCH_MOVE, 0.01, 0.5),
                pen(TOUCH_UP, 0.01, 0.02),
            ),
            (
                "absolute mouse",
                vec![absolute(20, 20), button(true)],
                absolute(900, 20),
                button(false),
            ),
        ];
        for (name, down, leave, up) in sources {
            let mut corner = CornerHold::new(1920, 1080, CORNER_HOLD);
            for e in &down {
                corner.observe(e, t0, true);
            }
            assert_eq!(corner.next_due(), Some(t0 + 1500 * MS), "{name}");
            assert!(!corner.fire(t0 + 1499 * MS, &mut never), "{name}");
            assert!(corner.fire(t0 + 1500 * MS, &mut never), "{name}");
            assert_eq!(corner.next_due(), None, "{name}");
            for cancel in [leave, up] {
                let mut corner = CornerHold::new(1920, 1080, CORNER_HOLD);
                for e in &down {
                    corner.observe(e, t0, true);
                }
                corner.observe(&cancel, t0 + 700 * MS, true);
                assert_eq!(corner.next_due(), None, "{name}: {cancel:?}");
                assert!(!corner.fire(t0 + 2000 * MS, &mut never), "{name}");
            }
            let mut corner = CornerHold::new(1920, 1080, CORNER_HOLD);
            for e in &down {
                corner.observe(e, t0, false);
            }
            assert_eq!(corner.next_due(), None, "{name}: no hold while not armed");
        }
        // Outside the box nothing starts.
        let mut corner = CornerHold::new(1920, 1080, CORNER_HOLD);
        corner.observe(&touch(TOUCH_DOWN, 1, 0.2, 0.2), t0, true);
        corner.observe(&absolute(300, 300), t0, true);
        corner.observe(&button(true), t0, true);
        assert_eq!(corner.next_due(), None);
        // A second finger cancels a touch hold.
        let mut corner = CornerHold::new(1920, 1080, CORNER_HOLD);
        corner.observe(&touch(TOUCH_DOWN, 1, 0.01, 0.01), t0, true);
        corner.observe(&touch(TOUCH_DOWN, 2, 0.5, 0.5), t0 + 10 * MS, true);
        assert_eq!(corner.next_due(), None);
    }

    #[test]
    fn couch_menu_relative_mouse_hold_reads_the_cursor_only_when_it_fires() {
        let t0 = Instant::now();
        let mut corner = CornerHold::new(1920, 1080, CORNER_HOLD);
        let mut reads = 0;
        corner.observe(&Input::Relative { x: -5, y: -5 }, t0, true);
        corner.observe(&button(true), t0, true);
        corner.observe(&Input::Relative { x: -5, y: -5 }, t0 + 100 * MS, true);
        let mut in_corner = || {
            reads += 1;
            Some((0.01, 0.01))
        };
        assert!(!corner.fire(t0 + 1499 * MS, &mut in_corner));
        assert!(corner.fire(t0 + 1500 * MS, &mut in_corner));
        assert_eq!(reads, 1, "one read, when the hold was due");
        // Cursor elsewhere or unknown: no open.
        let mut reads = 0;
        for p in [Some((0.5, 0.5)), None] {
            corner.observe(&button(false), t0, true);
            corner.observe(&button(true), t0, true);
            let mut cursor = || {
                reads += 1;
                p
            };
            assert!(!corner.fire(t0 + 1500 * MS, &mut cursor));
        }
        assert_eq!(reads, 2);
    }
}
