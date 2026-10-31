//! Captured mouse motion goes to the PC from the thread that reads the mouse, as each report
//! arrives.
//!
//! The main loop takes SDL's events once per pass, and with V-Sync a pass waits in the present
//! for most of a refresh: on the 120 Hz laptop panel a 1000 Hz mouse reached the PC as one move
//! per refresh, 5 to 12 ms apart in turn, so a steady swipe moved the PC's pointer in uneven
//! steps. An SDL event filter runs on the thread that pushes the event (SDL's raw input thread
//! for a captured mouse), so motion leaves at the mouse's own pace. A click or wheel turn still
//! waiting for the main loop holds later motion back, so motion never overtakes it.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;

use nightjar_core::input::queue::FAST_MOUSE_FLOOR;
use nightjar_core::session::InputHandle;
use sdl3_sys::everything::*;

/// Marks a click or wheel event that motion waits behind until the main loop has sent it.
const ORDERED: u32 = 0x4E4A_4D4F;

/// The mouse speed setting as a factor.
pub fn speed_factor(mouse_speed: u32) -> f32 {
    mouse_speed.clamp(25, 400) as f32 / 100.0
}

/// Scales a relative move and carries the fraction over to the next one; None when nothing
/// whole is left to send.
pub fn scale(remainder: &mut (f32, f32), xrel: f32, yrel: f32, speed: f32) -> Option<(i16, i16)> {
    let (dx, dy) = (xrel * speed + remainder.0, yrel * speed + remainder.1);
    let (ix, iy) = (dx.trunc(), dy.trunc());
    *remainder = (dx - ix, dy - iy);
    (ix != 0.0 || iy != 0.0).then_some((ix as i16, iy as i16))
}

struct Shared {
    input: InputHandle,
    /// Captured, relative, focused: motion may leave from the filter.
    active: AtomicBool,
    speed: AtomicU32,
    /// Touch and pen arrive as mouse events too; with touch on they are not mouse motion.
    touch: bool,
    /// Touchpad contacts go to the PC: SDL's global mouse, where the touchpad's pointer comes
    /// from, stays on the main loop, which swallows it.
    touchpad: AtomicBool,
    remainder: Mutex<(f32, f32)>,
    /// Clicks and wheel turns the main loop has not sent yet.
    ordered: AtomicU32,
    sent_direct: AtomicU64,
    sent_queued: AtomicU64,
}

impl Shared {
    fn send(&self, xrel: f32, yrel: f32, speed: f32) {
        let mut remainder = self.remainder.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((x, y)) = scale(&mut remainder, xrel, yrel, speed) {
            self.input.input(|q| {
                let _ = q.send_mouse_move(x, y);
            });
        }
    }
}

/// What the filter does with an event; the decision without SDL, for the tests.
#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    /// Sent here; SDL drops it.
    Sent,
    /// Queued for the main loop, marked so motion waits behind it.
    Ordered,
    /// Queued for the main loop as it is.
    Pass,
}

/// Clicks and wheel turns are marked captured or not: one queued before the capture starts still
/// holds the first direct motion back.
fn verdict(kind: SDL_EventType, which: SDL_MouseID, active: bool, ordered: u32, touch: bool, touchpad: bool) -> Verdict {
    let own = !(touch && matches!(which, SDL_TOUCH_MOUSEID | SDL_PEN_MOUSEID)) && !(touchpad && which == SDL_MouseID(0));
    match kind {
        SDL_EVENT_MOUSE_MOTION if active && ordered == 0 && own => Verdict::Sent,
        SDL_EVENT_MOUSE_BUTTON_DOWN | SDL_EVENT_MOUSE_BUTTON_UP | SDL_EVENT_MOUSE_WHEEL => Verdict::Ordered,
        _ => Verdict::Pass,
    }
}

unsafe extern "C" fn filter(userdata: *mut c_void, event: *mut SDL_Event) -> bool {
    // SAFETY: `userdata` is the Shared that DirectMotion keeps alive until it removes the filter,
    // and SDL calls the filter under the lock SDL_SetEventFilter takes; `event` is SDL's event.
    unsafe {
        let shared = &*(userdata as *const Shared);
        let kind = SDL_EventType((*event).r#type);
        let which = match kind {
            SDL_EVENT_MOUSE_MOTION => (*event).motion.which,
            _ => SDL_MouseID(0),
        };
        let (active, ordered) = (shared.active.load(Ordering::Acquire), shared.ordered.load(Ordering::Acquire));
        match verdict(kind, which, active, ordered, shared.touch, shared.touchpad.load(Ordering::Acquire)) {
            Verdict::Sent => {
                let m = (*event).motion;
                shared.send(m.xrel, m.yrel, f32::from_bits(shared.speed.load(Ordering::Relaxed)));
                shared.sent_direct.fetch_add(1, Ordering::Relaxed);
                false
            }
            queued => {
                mark(&shared.ordered, queued == Verdict::Ordered, &mut *event);
                true
            }
        }
    }
}

/// Marks a queued event that motion waits behind and counts it, and clears the mark on any
/// other: SDL builds events on the stack without clearing them, so a mark left from an earlier
/// event must not count as one.
fn mark(ordered: &AtomicU32, waits: bool, event: &mut SDL_Event) {
    if waits {
        ordered.fetch_add(1, Ordering::AcqRel);
    }
    event.common.reserved = if waits { ORDERED } else { 0 };
}

/// The main loop is done with `event`: a marked one no longer holds motion back.
fn release(ordered: &AtomicU32, event: &SDL_Event) {
    // SAFETY: every event starts with the common header.
    if unsafe { event.common.reserved } != ORDERED {
        return;
    }
    let mut n = ordered.load(Ordering::Acquire);
    while n > 0 {
        match ordered.compare_exchange_weak(n, n - 1, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => break,
            Err(now) => n = now,
        }
    }
}

/// The filter for one stream; removed when dropped.
pub struct DirectMotion {
    shared: Box<Shared>,
    installed: bool,
}

impl DirectMotion {
    /// Installs the filter, unless `NIGHTJAR_DIRECT_MOTION=0` (measurement only) keeps all motion
    /// on the main loop.
    pub fn new(input: InputHandle, touch: bool) -> Self {
        let shared = Box::new(Shared {
            input,
            active: AtomicBool::new(false),
            speed: AtomicU32::new(1.0f32.to_bits()),
            touch,
            touchpad: AtomicBool::new(false),
            remainder: Mutex::new((0.0, 0.0)),
            ordered: AtomicU32::new(0),
            sent_direct: AtomicU64::new(0),
            sent_queued: AtomicU64::new(0),
        });
        let installed = std::env::var("NIGHTJAR_DIRECT_MOTION").map_or(true, |v| v.trim() != "0");
        if installed {
            // The main loop's once-per-refresh pace used to merge a 4 or 8 kHz mouse's reports;
            // now the queue does, at most one packet per 0.5 ms.
            shared.input.input(|q| q.set_relative_motion_floor(FAST_MOUSE_FLOOR));
            // SAFETY: the Box outlives the filter (Drop removes it first).
            unsafe { SDL_SetEventFilter(Some(filter), &*shared as *const Shared as *mut c_void) };
        } else {
            eprintln!("nightjar: mouse motion through the main loop (NIGHTJAR_DIRECT_MOTION=0)");
        }
        Self { shared, installed }
    }

    /// Whether motion may leave from the filter: captured, relative and focused.
    pub fn set_active(&self, active: bool, mouse_speed: u32) {
        self.shared.speed.store(speed_factor(mouse_speed).to_bits(), Ordering::Relaxed);
        self.shared.active.store(active && self.installed, Ordering::Release);
    }

    /// Touchpad contacts go to the PC from now on: the touchpad's pointer moves stay on the main
    /// loop, which tells them from the mouse's.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn touchpad_contacts(&self) {
        self.shared.touchpad.store(true, Ordering::Release);
    }

    /// Relative motion the main loop handles (the filter let it through).
    pub fn send(&self, xrel: f32, yrel: f32, mouse_speed: u32) {
        self.shared.send(xrel, yrel, speed_factor(mouse_speed));
        self.shared.sent_queued.fetch_add(1, Ordering::Relaxed);
    }

    /// Call once the main loop is done with `event` (every event it takes while streaming): a
    /// click or wheel turn it has sent, or the menu took, no longer holds motion back.
    pub fn dispatched(&self, event: &SDL_Event) {
        release(&self.shared.ordered, event);
    }

    /// How many moves left from the filter and from the main loop.
    pub fn counts(&self) -> (u64, u64) {
        (self.shared.sent_direct.load(Ordering::Relaxed), self.shared.sent_queued.load(Ordering::Relaxed))
    }
}

impl Drop for DirectMotion {
    fn drop(&mut self) {
        if self.installed {
            // SAFETY: SDL takes the filter lock, so no call is running once this returns.
            unsafe { SDL_SetEventFilter(None, std::ptr::null_mut()) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speed_scales_and_fractions_carry_over() {
        let mut rest = (0.0, 0.0);
        assert_eq!(scale(&mut rest, 1.0, -1.0, 1.5), Some((1, -1)));
        assert_eq!(scale(&mut rest, 1.0, -1.0, 1.5), Some((2, -2)), "the halves add up");
        assert_eq!(scale(&mut rest, 1.0, 0.0, 0.25), None);
        assert_eq!(rest, (0.25, 0.0));
        assert_eq!(speed_factor(10), 0.25);
        assert_eq!(speed_factor(1000), 4.0);
    }

    #[test]
    fn motion_leaves_at_once_unless_a_click_waits() {
        let mouse = SDL_MouseID(7);
        assert_eq!(verdict(SDL_EVENT_MOUSE_MOTION, mouse, true, 0, false, false), Verdict::Sent);
        assert_eq!(verdict(SDL_EVENT_MOUSE_MOTION, mouse, true, 1, false, false), Verdict::Pass, "behind a click");
        assert_eq!(verdict(SDL_EVENT_MOUSE_MOTION, mouse, false, 0, false, false), Verdict::Pass, "not captured");
        for kind in [SDL_EVENT_MOUSE_BUTTON_DOWN, SDL_EVENT_MOUSE_BUTTON_UP, SDL_EVENT_MOUSE_WHEEL] {
            assert_eq!(verdict(kind, mouse, true, 0, false, false), Verdict::Ordered);
            assert_eq!(verdict(kind, mouse, false, 0, false, false), Verdict::Ordered, "queued before the capture");
        }
        assert_eq!(verdict(SDL_EVENT_KEY_DOWN, mouse, true, 0, false, false), Verdict::Pass);
    }

    #[test]
    fn touch_and_pen_stay_on_the_main_loop_with_touch_on() {
        for which in [SDL_TOUCH_MOUSEID, SDL_PEN_MOUSEID] {
            assert_eq!(verdict(SDL_EVENT_MOUSE_MOTION, which, true, 0, true, false), Verdict::Pass);
            assert_eq!(verdict(SDL_EVENT_MOUSE_MOTION, which, true, 0, false, false), Verdict::Sent);
        }
    }

    /// Button down, then a motion SDL built where the marked down was, then button up: the stale
    /// mark is cleared, so the queued button up still holds motion back after the down went out.
    #[test]
    fn a_stale_mark_does_not_let_motion_pass_a_queued_release() {
        let ordered = AtomicU32::new(0);
        // SAFETY: SDL_Event is plain data; all zero is a valid value.
        let mut down: SDL_Event = unsafe { std::mem::zeroed() };
        down.r#type = SDL_EVENT_MOUSE_BUTTON_DOWN.0;
        mark(&ordered, true, &mut down);
        let mut motion = down;
        motion.r#type = SDL_EVENT_MOUSE_MOTION.0;
        mark(&ordered, false, &mut motion);
        let mut up = down;
        up.r#type = SDL_EVENT_MOUSE_BUTTON_UP.0;
        mark(&ordered, true, &mut up);
        assert_eq!(ordered.load(Ordering::Acquire), 2);
        release(&ordered, &down);
        release(&ordered, &motion);
        assert_eq!(ordered.load(Ordering::Acquire), 1, "the release still waits");
        release(&ordered, &up);
        release(&ordered, &up);
        assert_eq!(ordered.load(Ordering::Acquire), 0, "and never below zero");
    }

    /// The main loop swallows the touchpad's pointer moves (SDL's global mouse) while its
    /// contacts go to the PC; a mouse of its own still goes straight out.
    #[test]
    fn the_touchpads_pointer_stays_on_the_main_loop_while_contacts_go_out() {
        let global = SDL_MouseID(0);
        assert_eq!(verdict(SDL_EVENT_MOUSE_MOTION, global, true, 0, false, true), Verdict::Pass);
        assert_eq!(verdict(SDL_EVENT_MOUSE_MOTION, global, true, 0, false, false), Verdict::Sent);
        assert_eq!(verdict(SDL_EVENT_MOUSE_MOTION, SDL_MouseID(7), true, 0, false, true), Verdict::Sent);
    }
}
