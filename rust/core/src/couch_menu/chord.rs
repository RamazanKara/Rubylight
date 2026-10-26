//! The ways to open the menu (`docs/couch-menu-design.md` section 3.2): L3+R3
//! held on one pad, Ctrl+Alt+Backspace, and a hold in the stream's top-left
//! corner. Time is the host's receive time; a pending hold has a deadline
//! that the control loop wakes for, because Moonlight sends controller state
//! only when it changes.
use std::time::{Duration, Instant};

use crate::input::Input;

/// Moonlight's stick-click button flags.
pub const LS_CLK: u32 = 0x0040;
pub const RS_CLK: u32 = 0x0080;
/// A pad's buttons must be exactly these, with sticks and triggers ignored.
pub const PAD_CHORD: u32 = LS_CLK | RS_CLK;
pub const PAD_HOLD: Duration = Duration::from_millis(1000);
pub const CORNER_HOLD: Duration = Duration::from_millis(1500);
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

/// `couch_menu_open_with`: which ways open the menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OpenWith {
    pub controller: bool,
    pub keyboard: bool,
    /// Touch, pen and mouse holds in the corner.
    pub touch: bool,
}

impl Default for OpenWith {
    fn default() -> Self {
        Self {
            controller: true,
            keyboard: true,
            touch: true,
        }
    }
}

impl OpenWith {
    /// Parses the comma-separated list; an empty value turns every way off.
    /// Unknown entries are an error, so a typo fails loudly.
    pub fn parse(value: &str) -> Result<Self, String> {
        let mut out = Self {
            controller: false,
            keyboard: false,
            touch: false,
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

/// Ctrl+Alt+Backspace down, without Shift.
pub fn is_key_chord(event: &Input) -> bool {
    matches!(event, Input::Keyboard { key: VK_BACK, modifiers, down: true, .. }
        if modifiers & (MODIFIER_CTRL | MODIFIER_ALT) == MODIFIER_CTRL | MODIFIER_ALT
            && modifiers & MODIFIER_SHIFT == 0)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum PadState {
    #[default]
    Idle,
    Armed(Instant),
    /// The hold fired; it arms again only after the buttons change.
    Fired,
}

/// The L3+R3 hold on each of the 16 pads.
#[derive(Debug, Default)]
pub struct PadChord {
    pads: [PadState; 16],
}

impl PadChord {
    /// A pad's new button state: exactly L3+R3 arms it, anything else
    /// (another button, either click released) cancels it.
    pub fn update(&mut self, id: u16, buttons: u32, now: Instant) {
        let Some(pad) = self.pads.get_mut(usize::from(id)) else {
            return;
        };
        if buttons != PAD_CHORD {
            *pad = PadState::Idle;
        } else if *pad == PadState::Idle {
            *pad = PadState::Armed(now + PAD_HOLD);
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
    hold: Option<(Source, Instant)>,
}

impl CornerHold {
    /// For a stream of `width`×`height` target pixels.
    pub fn new(width: u32, height: u32) -> Self {
        let side = CORNER_SHARE * width.min(height).max(1) as f32;
        Self {
            box_w: side / width.max(1) as f32,
            box_h: side / height.max(1) as f32,
            last_absolute: None,
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
        let deadline = now + CORNER_HOLD;
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
        assert!(is_key_chord(&key(VK_BACK, ctrl_alt, true)));
        assert!(
            is_key_chord(&key(VK_BACK, ctrl_alt | 0x08, true)),
            "Meta does not matter"
        );
        assert!(!is_key_chord(&key(
            VK_BACK,
            ctrl_alt | MODIFIER_SHIFT,
            true
        )));
        assert!(!is_key_chord(&key(VK_BACK, ctrl_alt, false)));
        assert!(!is_key_chord(&key(VK_BACK, MODIFIER_CTRL, true)));
        assert!(!is_key_chord(&key(VK_BACK, 0, true)));
        assert!(
            !is_key_chord(&key(0x2e, ctrl_alt, true)),
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
                touch: false
            })
        );
        assert_eq!(
            OpenWith::parse(""),
            Ok(OpenWith {
                controller: false,
                keyboard: false,
                touch: false
            })
        );
        assert!(
            OpenWith::parse("controler")
                .unwrap_err()
                .contains("controler")
        );
    }

    #[test]
    fn couch_menu_corner_box_is_six_percent_of_the_shorter_side() {
        let corner = CornerHold::new(1920, 1080);
        // 64.8 px on each axis.
        assert!(corner.inside((64. / 1920., 64. / 1080.)));
        assert!(!corner.inside((65. / 1920., 10. / 1080.)));
        assert!(!corner.inside((10. / 1920., 65. / 1080.)));
        let portrait = CornerHold::new(1080, 2400);
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
            let mut corner = CornerHold::new(1920, 1080);
            for e in &down {
                corner.observe(e, t0, true);
            }
            assert_eq!(corner.next_due(), Some(t0 + 1500 * MS), "{name}");
            assert!(!corner.fire(t0 + 1499 * MS, &mut never), "{name}");
            assert!(corner.fire(t0 + 1500 * MS, &mut never), "{name}");
            assert_eq!(corner.next_due(), None, "{name}");
            for cancel in [leave, up] {
                let mut corner = CornerHold::new(1920, 1080);
                for e in &down {
                    corner.observe(e, t0, true);
                }
                corner.observe(&cancel, t0 + 700 * MS, true);
                assert_eq!(corner.next_due(), None, "{name}: {cancel:?}");
                assert!(!corner.fire(t0 + 2000 * MS, &mut never), "{name}");
            }
            let mut corner = CornerHold::new(1920, 1080);
            for e in &down {
                corner.observe(e, t0, false);
            }
            assert_eq!(corner.next_due(), None, "{name}: no hold while not armed");
        }
        // Outside the box nothing starts.
        let mut corner = CornerHold::new(1920, 1080);
        corner.observe(&touch(TOUCH_DOWN, 1, 0.2, 0.2), t0, true);
        corner.observe(&absolute(300, 300), t0, true);
        corner.observe(&button(true), t0, true);
        assert_eq!(corner.next_due(), None);
        // A second finger cancels a touch hold.
        let mut corner = CornerHold::new(1920, 1080);
        corner.observe(&touch(TOUCH_DOWN, 1, 0.01, 0.01), t0, true);
        corner.observe(&touch(TOUCH_DOWN, 2, 0.5, 0.5), t0 + 10 * MS, true);
        assert_eq!(corner.next_due(), None);
    }

    #[test]
    fn couch_menu_relative_mouse_hold_reads_the_cursor_only_when_it_fires() {
        let t0 = Instant::now();
        let mut corner = CornerHold::new(1920, 1080);
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
