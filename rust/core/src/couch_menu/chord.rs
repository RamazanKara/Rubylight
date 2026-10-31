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
        let out = Self::read(config, &mut crate::config::refused);
        if let Some(warning) = out.select_conflict(crate::input_policy::back_button_timeout(config))
        {
            tracing::warn!("{warning}");
        }
        out
    }
    /// Why the pad chord may press Guide before it opens the menu: with
    /// `back_button_timeout` on, the host turns Select held that long into
    /// Guide, which opens Xbox Game Bar. A chord with Select and a longer hold
    /// gets there first.
    pub fn select_conflict(&self, back_button_timeout: Option<Duration>) -> Option<String> {
        let guide_after = back_button_timeout?;
        (self.controller && self.pad_chord.0 & SELECT != 0 && guide_after < self.pad_hold).then(
            || {
                format!(
                    "couch_menu_controller_chord: {} holds Select for {} ms, but \
                     back_button_timeout presses Guide (Xbox Game Bar) after {} ms, before the \
                     menu opens; set back_button_timeout to -1 or to at least couch_menu_hold_ms",
                    self.pad_chord,
                    self.pad_hold.as_millis(),
                    guide_after.as_millis()
                )
            },
        )
    }
    /// [`OpenWith::from_config`], reporting each refused key and value to
    /// `refused`, with the reason.
    pub(crate) fn read(config: &Config, refused: &mut dyn FnMut(&str, &str, &str)) -> Self {
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
                Err(error) => refused("couch_menu_open_with", v, &error),
            }
        }
        if let Some(v) = value("couch_menu_controller_chord") {
            match PadButtons::parse(v) {
                Ok(chord) => out.pad_chord = chord,
                Err(error) => refused("couch_menu_controller_chord", v, &error),
            }
        }
        if let Some(v) = value("couch_menu_hold_ms") {
            match parse_hold("couch_menu_hold_ms", v, PAD_HOLD_MS) {
                Ok(hold) => out.pad_hold = hold,
                Err(error) => refused("couch_menu_hold_ms", v, &error),
            }
        }
        if let Some(v) = value("couch_menu_corner_hold_ms") {
            match parse_hold("couch_menu_corner_hold_ms", v, CORNER_HOLD_MS) {
                Ok(hold) => out.corner_hold = hold,
                Err(error) => refused("couch_menu_corner_hold_ms", v, &error),
            }
        }
        if let Some(v) = value("couch_menu_keyboard_chord") {
            match KeyChord::parse(v) {
                Ok(chord) => out.key_chord = chord,
                Err(error) => refused("couch_menu_keyboard_chord", v, &error),
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
    ("up", UP),
    ("down", DOWN),
    ("left", LEFT),
    ("right", RIGHT),
    ("start", START),
    ("select", SELECT),
    ("l3", LS_CLK),
    ("r3", RS_CLK),
    ("l1", L1),
    ("r1", R1),
    ("guide", GUIDE),
    ("a", A),
    ("b", B),
    ("x", X),
    ("y", 0x8000),
    ("paddle1", 0x01_0000),
    ("paddle2", 0x02_0000),
    ("paddle3", 0x04_0000),
    ("paddle4", 0x08_0000),
    ("touchpad", 0x10_0000),
    ("misc", 0x20_0000),
];
const UP: u32 = 0x0001;
const DOWN: u32 = 0x0002;
const LEFT: u32 = 0x0004;
const RIGHT: u32 = 0x0008;
const START: u32 = 0x0010;
const SELECT: u32 = 0x0020;
const L1: u32 = 0x0100;
const R1: u32 = 0x0200;
const GUIDE: u32 = 0x0400;
const A: u32 = 0x1000;
const B: u32 = 0x2000;
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
/// Buttons the open menu reads (`filter::Filter::pad_nav`): holding the chord
/// to close it would press them on the focused row first.
const NAVIGATION: [(u32, &str); 6] = [
    (
        A,
        "A presses the focused row in the open menu, so holding the chord to close it \
         would press that row, such as Disconnect",
    ),
    (
        B,
        "B goes back in the open menu, so holding the chord to close it would go back a page first",
    ),
    (
        UP,
        "Up moves the focus in the open menu, so holding the chord to close it would move it first",
    ),
    (
        DOWN,
        "Down moves the focus in the open menu, so holding the chord to close it would move it first",
    ),
    (
        LEFT,
        "Left moves the focus in the open menu, so holding the chord to close it would move it first",
    ),
    (
        RIGHT,
        "Right moves the focus in the open menu, so holding the chord to close it would move it first",
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
    /// buttons, the combinations Moonlight clients keep and the buttons the
    /// open menu reads are an error.
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
                     (use select, l3, r3, l1, r1, guide, x, y, paddle1 to paddle4, \
                     touchpad, misc; triggers cannot hold)"
                ));
            };
            buttons |= bit;
        }
        let chord = Self(buttons);
        if let Some((_, reason)) = RESERVED
            .iter()
            .chain(&NAVIGATION)
            .find(|(set, _)| buttons & set == *set)
        {
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
             (Android TV home, the Steam button, iOS), so a chord with Guide may never reach the host; \
             where it does, the Guide press reaches Windows too and opens Xbox Game Bar",
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
    /// Parses one of [`KEY_CHORDS`], in any case, with the modifiers in any
    /// order and with spaces around `+`.
    pub fn parse(value: &str) -> Result<Self, String> {
        let mut parts: Vec<String> = value
            .split('+')
            .map(|part| part.trim().to_ascii_lowercase())
            .collect();
        // The modifiers first, in a fixed order; the sort is stable, so the
        // rest keeps its order.
        parts.sort_by_key(|part| {
            ["ctrl", "alt", "shift"]
                .iter()
                .position(|m| m == part)
                .unwrap_or(3)
        });
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
mod tests;
