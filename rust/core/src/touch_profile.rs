//! Touch profiles (`touch_mode`, `touch_sensitivity` and `touch_gestures`, per device):
//! the host turns a device's native touches (0x55000002) into a mouse or a trackpad,
//! for devices and games that handle a mouse better than touch. Each touch maps to its
//! mouse input as it arrives: nothing waits to see what a gesture becomes.
//!
//! - Mouse: the pointer follows the first finger and the left button is down while it
//!   touches. With gestures, a second finger lets the left button go and the fingers
//!   scroll; a two-finger tap clicks right.
//! - Trackpad: the first finger moves the pointer by its swipe, scaled by the
//!   sensitivity; a tap clicks left. With gestures, two fingers scroll and a
//!   two-finger tap clicks right.
//!
//! The sensitivity scales trackpad movement and scrolling in both modes. The profile
//! takes native touches, so it needs `native_pen_touch` on (the default); with it off
//! the device turns touches into a mouse itself.
use crate::config::{Config, TouchMode};
use crate::input::Input;
use std::time::{Duration, Instant};

/// Moonlight's touch events.
const DOWN: u8 = 1;
const UP: u8 = 2;
const MOVE: u8 = 3;
const CANCEL: u8 = 4;
const CANCEL_ALL: u8 = 7;
/// Moonlight's left and right mouse buttons.
const LEFT: u8 = 1;
const RIGHT: u8 = 3;
/// The reference size absolute positions are sent against.
const ABSOLUTE_SCALE: i16 = i16::MAX;
/// A gesture that lifts within this, with no finger moving more than [`TAP_SLOP`]
/// pixels of the stream, is a tap.
const TAP_TIME: Duration = Duration::from_millis(250);
const TAP_SLOP: f32 = 12.0;
/// Wheel units (120 a notch) per stream pixel two fingers move, at 100 %.
const SCROLL_PER_PIXEL: f32 = 2.0;

/// A device's touch profile, from its effective configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    pub mode: TouchMode,
    /// Percent, 25 to 400.
    pub sensitivity: u16,
    pub gestures: bool,
}
impl Settings {
    pub fn resolve(config: &Config) -> Self {
        Self {
            mode: config.touch_mode(),
            sensitivity: config.touch_sensitivity(),
            gestures: config.touch_gestures(),
        }
    }
    /// Whether the host turns touches into mouse input.
    pub fn translates(&self) -> bool {
        self.mode != TouchMode::Client
    }
}

struct Finger {
    id: u32,
    start: [f32; 2],
    last: [f32; 2],
}

/// One stream's touches on their way to becoming mouse input.
#[derive(Default)]
pub struct TouchProfile {
    /// The fingers down, the first one first.
    fingers: Vec<Finger>,
    /// When the gesture's first finger touched, the most fingers down since, and whether
    /// any finger moved beyond the tap slop.
    began: Option<Instant>,
    most: usize,
    moved: bool,
    /// Whether the left button is down for the first finger (mouse mode).
    pressed: bool,
    /// Sub-pixel movement and sub-unit scrolling carried to the next touch.
    remainder: [f32; 2],
    scroll_remainder: [f32; 2],
}
impl TouchProfile {
    /// The mouse input for one touch event at `x`, `y` (fractions of the stream) on a
    /// stream of `size` pixels. Touches the profile doesn't use (hovers, a third finger
    /// without gestures) give nothing.
    pub fn translate(
        &mut self,
        settings: &Settings,
        event: u8,
        id: u32,
        [x, y]: [f32; 2],
        size: (u32, u32),
        now: Instant,
    ) -> Vec<Input> {
        let mut out = Vec::new();
        if !settings.translates() || !x.is_finite() || !y.is_finite() {
            return out;
        }
        let at = [x.clamp(0.0, 1.0), y.clamp(0.0, 1.0)];
        let size = [size.0.max(1) as f32, size.1.max(1) as f32];
        let mouse = settings.mode == TouchMode::Mouse;
        match event {
            DOWN if self.fingers.iter().all(|f| f.id != id) => {
                if self.fingers.is_empty() {
                    self.began = Some(now);
                    self.most = 0;
                    self.moved = false;
                    self.remainder = [0.0; 2];
                    self.scroll_remainder = [0.0; 2];
                }
                self.fingers.push(Finger {
                    id,
                    start: at,
                    last: at,
                });
                self.most = self.most.max(self.fingers.len());
                match self.fingers.len() {
                    1 if mouse => {
                        out.push(absolute(at));
                        out.push(button(LEFT, true));
                        self.pressed = true;
                    }
                    2 if settings.gestures => self.release(&mut out),
                    _ => {}
                }
            }
            MOVE => {
                let Some(index) = self.fingers.iter().position(|f| f.id == id) else {
                    return out;
                };
                let finger = &mut self.fingers[index];
                let delta = [
                    (at[0] - finger.last[0]) * size[0],
                    (at[1] - finger.last[1]) * size[1],
                ];
                finger.last = at;
                let travel = [
                    (at[0] - finger.start[0]) * size[0],
                    (at[1] - finger.start[1]) * size[1],
                ];
                self.moved |= travel[0].hypot(travel[1]) > TAP_SLOP;
                let scale = f32::from(settings.sensitivity) / 100.0;
                let fingers = self.fingers.len();
                if fingers >= 2 && settings.gestures {
                    // Each finger moves the scroll by its share, so two fingers moving
                    // together scroll by their common movement. The content follows the
                    // fingers: down scrolls up, right scrolls left.
                    let share = scale * SCROLL_PER_PIXEL / fingers as f32;
                    let vertical = take(&mut self.scroll_remainder[1], delta[1] * share);
                    let horizontal = take(&mut self.scroll_remainder[0], -delta[0] * share);
                    if vertical != 0 {
                        out.push(Input::Scroll {
                            amount: vertical,
                            horizontal: false,
                        });
                    }
                    if horizontal != 0 {
                        out.push(Input::Scroll {
                            amount: horizontal,
                            horizontal: true,
                        });
                    }
                } else if index == 0 {
                    if mouse {
                        out.push(absolute(at));
                    } else {
                        let x = take(&mut self.remainder[0], delta[0] * scale);
                        let y = take(&mut self.remainder[1], delta[1] * scale);
                        if x != 0 || y != 0 {
                            out.push(Input::Relative { x, y });
                        }
                    }
                }
            }
            UP | CANCEL => {
                let Some(index) = self.fingers.iter().position(|f| f.id == id) else {
                    return out;
                };
                self.fingers.remove(index);
                if index == 0 {
                    self.release(&mut out);
                }
                if self.fingers.is_empty() {
                    let tap = event == UP
                        && !self.moved
                        && self
                            .began
                            .is_some_and(|began| now.duration_since(began) <= TAP_TIME);
                    match self.most {
                        1 if tap && !mouse => out.extend(click(LEFT)),
                        2.. if tap && settings.gestures => out.extend(click(RIGHT)),
                        _ => {}
                    }
                    self.began = None;
                }
            }
            CANCEL_ALL => {
                self.fingers.clear();
                self.began = None;
                self.release(&mut out);
            }
            _ => {}
        }
        out
    }

    /// Lets the left button go if the first finger holds it.
    fn release(&mut self, out: &mut Vec<Input>) {
        if std::mem::take(&mut self.pressed) {
            out.push(button(LEFT, false));
        }
    }
}

fn absolute([x, y]: [f32; 2]) -> Input {
    let scale = f32::from(ABSOLUTE_SCALE);
    Input::Absolute {
        x: (x * scale).round() as i16,
        y: (y * scale).round() as i16,
        width: ABSOLUTE_SCALE,
        height: ABSOLUTE_SCALE,
    }
}

fn button(button: u8, down: bool) -> Input {
    Input::MouseButton { button, down }
}

fn click(which: u8) -> [Input; 2] {
    [button(which, true), button(which, false)]
}

/// The whole units of `remainder + amount`, keeping the fraction in `remainder`.
fn take(remainder: &mut f32, amount: f32) -> i16 {
    let total = *remainder + amount;
    let whole = total
        .trunc()
        .clamp(f32::from(i16::MIN), f32::from(i16::MAX));
    *remainder = total - whole;
    whole as i16
}

#[cfg(test)]
mod tests;
