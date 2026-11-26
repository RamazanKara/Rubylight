//! Captured mouse motion goes to the PC from the thread that reads the mouse, as each report
//! arrives; keys, clicks and controllers do while the main loop waits in the present.
//!
//! The main loop takes SDL's events once per pass, and with V-Sync a pass waits in the present
//! for most of a refresh: on the 120 Hz laptop panel a 1000 Hz mouse reached the PC as one move
//! per refresh, 5 to 12 ms apart in turn, so a steady swipe moved the PC's pointer in uneven
//! steps. An SDL event filter runs on the thread that pushes the event (SDL's raw input thread
//! for a captured mouse), so motion leaves at the mouse's own pace. A click or wheel turn still
//! waiting for the main loop holds later motion back, so motion never overtakes it.
//!
//! Keys, clicks, wheel turns and controllers waited in the same way, up to a refresh (16.7 ms at
//! 60 Hz), and a stick's moves reached the PC bunched once per refresh. While the loop waits in
//! the present it changes nothing, so for that time it lends the filter what it decides with
//! (whether keys and clicks are the PC's, the settings that map them, its controllers) and a
//! thread reads the controllers, every millisecond while they are in use, as SDL does while the
//! loop waits for events.
//! A key from the system keys hook's thread (it takes the keyboard while "Send system keys"
//! grabs it, system_keys.rs), a click from SDL's raw input thread and a controller event from
//! that thread then leave at once; the loop still gets the keys and
//! clicks, marked, and keeps its state without sending them again. Anything the loop sends or
//! decides on itself (a stream shortcut, an event from before the present, a new controller, a
//! controller chord's action) holds the keys, clicks and controller events after it back, so
//! none overtakes it. Linux reads keys and clicks on the main thread: there only controllers
//! gain.

use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

use nightjar_core::input::queue::FAST_MOUSE_FLOOR;
use nightjar_core::session::InputHandle;
use sdl3_sys::everything::*;

use super::input::{key_input, mouse_button_code, send_wheel, KeyInput};
use crate::gamepad::{Action, Gamepads};

/// Marks a click or wheel event that motion waits behind until the main loop has sent it.
const ORDERED: u32 = 0x4E4A_4D4F;
/// Marks a key, click or controller event the main loop handles itself: keys, clicks and
/// controller events after it wait for it too (motion does not).
const QUEUED: u32 = 0x4E4A_5155;
/// Marks a window focus/visibility barrier (focus lost, minimized, hidden): holds both motion
/// (`ordered`) and keys/clicks/controllers (`queued`) until the main loop processes it.
const BARRIER: u32 = 0x4E4A_4252;
/// Marks a key, click or wheel turn the filter sent: the main loop keeps its state, sends nothing.
const SENT: u32 = 0x4E4A_534E;
/// Marks a controller event the filter applied that asks the main loop to act, plus the action's
/// place in [`ACTIONS`]; it holds later events back like [`QUEUED`].
const ACTED: u32 = 0x4E4A_4100;
const ACTIONS: [Action; 4] = [Action::Quit, Action::Menu, Action::Mouse(false), Action::Mouse(true)];
/// How often controllers are read while the main loop waits in the present: 1 ms (1000 Hz) while
/// the controller is actively producing input, relaxing to 4 ms (250 Hz) once idle for a second.
const POLL_ACTIVE_NS: u64 = 1_000_000;
const POLL_IDLE_NS: u64 = 4_000_000;
const POLL_ACTIVE_HOLD_US: u64 = 1_000_000;
/// Minimum analog stick/trigger change treated as intentional controller activity (~3%).
pub const PAD_AXIS_ACTIVITY_THRESHOLD: i32 = 1024;
/// A gyro sample whose largest axis exceeds this (rad/s, ~1.15 deg/s) is activity. A slow aiming
/// turn of 2 deg/s (0.035 rad/s) puts at least 0.035/sqrt(3) = 0.0202 on one axis however the pad
/// is held. A resting pad should stay below: noise of a few mrad/s, and SDL subtracts the pad's
/// calibrated bias (estimated, not measured on a pad).
pub const GYRO_ACTIVITY_THRESHOLD: f32 = 0.02;
const AXIS_JITTER_THRESHOLD: i32 = PAD_AXIS_ACTIVITY_THRESHOLD;

/// Returns whether a peak gyro sample (rad/s) exceeds [`GYRO_ACTIVITY_THRESHOLD`].
pub fn is_gyro_peak_active(gyro_peak: Option<f32>) -> bool {
    gyro_peak.is_some_and(|peak| peak.is_finite() && peak > GYRO_ACTIVITY_THRESHOLD)
}

/// Returns whether `event` is a gyroscope update above [`GYRO_ACTIVITY_THRESHOLD`]; non-gyro
/// sensors (such as accelerometers) and non-finite samples return `false`.
pub fn is_gyro_sensor_event(event: &SDL_Event) -> bool {
    // SAFETY: `gsensor` is read only when `event.r#type == SDL_EVENT_GAMEPAD_SENSOR_UPDATE.0`.
    unsafe {
        if SDL_EventType(event.r#type) != SDL_EVENT_GAMEPAD_SENSOR_UPDATE
            || SDL_SensorType(event.gsensor.sensor) != SDL_SENSOR_GYRO
            || !event.gsensor.data.iter().all(|v| v.is_finite())
        {
            return false;
        }
        let peak = event.gsensor.data.into_iter().map(f32::abs).fold(0.0_f32, f32::max);
        is_gyro_peak_active(Some(peak))
    }
}

/// Stateful per-controller activity filter rejecting resting stick drift and sub-threshold sensor noise.
#[derive(Default)]
pub struct PadActivity {
    last_axis: HashMap<(u32, u8), i16>,
}

impl PadActivity {
    fn note_axis(&mut self, pad: u32, axis: u8, value: i16) -> bool {
        let prev = self.last_axis.get(&(pad, axis)).copied().unwrap_or(0);
        if (i32::from(value) - i32::from(prev)).abs() < AXIS_JITTER_THRESHOLD { return false; }
        if self.last_axis.len() >= 256 && !self.last_axis.contains_key(&(pad, axis)) { self.last_axis.clear(); }
        self.last_axis.insert((pad, axis), value);
        true
    }

    /// Drops cached axis baselines when controller `pad` is removed.
    pub fn remove_pad(&mut self, pad: u32) { self.last_axis.retain(|&(id, _), _| id != pad); }

    /// Returns whether `event` represents intentional controller activity (buttons, touchpads,
    /// above-threshold stick/trigger motion, or above-noise-floor gyroscope motion).
    pub fn is_active(&mut self, event: &SDL_Event) -> bool {
        // SAFETY: union members read by event type.
        unsafe {
            match SDL_EventType(event.r#type) {
                SDL_EVENT_GAMEPAD_BUTTON_DOWN | SDL_EVENT_GAMEPAD_BUTTON_UP
                | SDL_EVENT_GAMEPAD_TOUCHPAD_DOWN | SDL_EVENT_GAMEPAD_TOUCHPAD_MOTION
                | SDL_EVENT_GAMEPAD_TOUCHPAD_UP => true,
                SDL_EVENT_GAMEPAD_AXIS_MOTION => self.note_axis(event.gaxis.which.0, event.gaxis.axis, event.gaxis.value),
                SDL_EVENT_GAMEPAD_SENSOR_UPDATE => is_gyro_sensor_event(event),
                _ => false,
            }
        }
    }
}

/// Sleep duration (ns) for the controller poller thread between `SDL_UpdateJoysticks` calls.
fn poller_sleep_ns(now_us: u64, last_pad_us: u64) -> u64 {
    if last_pad_us > 0 && now_us.saturating_sub(last_pad_us) < POLL_ACTIVE_HOLD_US {
        POLL_ACTIVE_NS
    } else {
        POLL_IDLE_NS
    }
}

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

/// What the main loop would do with keys, clicks and controllers, unchanging while it waits in
/// the present.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rules {
    /// Captured and focused with the menu hidden: keys are the PC's.
    pub keys: Option<KeyRules>,
    /// The same with a relative pointer: clicks and wheel turns are the PC's.
    pub clicks: Option<ClickRules>,
    /// Controllers send: menu hidden, controllers on, focused or allowed in the background.
    pub pads: bool,
}

/// The settings that map a key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyRules {
    pub system_keys: bool,
    pub right_alt_as_meta: bool,
}

/// The settings that map a click.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClickRules {
    pub nav_buttons: bool,
}

/// What the main loop lends the filter.
#[derive(Default)]
struct Frozen {
    /// Some while the loop waits in the present.
    rules: Option<Rules>,
    /// The loop's controllers, from before its present until after it.
    pads: Option<Lent>,
}

/// Filter-sent DOWN inputs, separate from `Frozen` so `take_sent_down` never blocks under `Loan::pads`.
#[derive(Default)]
struct SentDown { keys: HashSet<u16>, buttons: HashSet<u8> }

/// The main loop's controllers on loan.
struct Lent(Gamepads);

// SAFETY: another thread only calls `Gamepads::handle` on them (event fields, controller state and
// the input queue, no SDL call through the controllers' handles), under `Frozen`'s lock, while the
// main loop waits in the present without them.
unsafe impl Send for Lent {}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|p| p.into_inner())
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
    /// Keys and controller events the main loop has not handled yet.
    queued: AtomicU32,
    frozen: Arc<Mutex<Frozen>>,
    sent_down: Mutex<SentDown>,
    sent_direct: AtomicU64,
    sent_queued: AtomicU64,
    /// Keys, clicks, wheel turns and controller events that left from the filter.
    sent_input: AtomicU64,
    /// Monotonic session timestamp (µs) of the most recent active controller input seen by the filter.
    last_pad_activity_us: Arc<AtomicU64>,
    pad_activity: Mutex<PadActivity>,
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

    /// Runs `send` with what the main loop lent for its present; None while it lends nothing or
    /// holds the lock. Never waits: the filter may run under SDL's joystick lock, which the main
    /// loop may take while it holds this one.
    fn direct<R>(&self, send: impl FnOnce(Rules, &mut Frozen) -> Option<R>) -> Option<R> {
        let mut frozen = self.frozen.try_lock().ok()?;
        let rules = frozen.rules?;
        send(rules, &mut frozen)
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
    let own = !(touch && matches!(which, SDL_TOUCH_MOUSEID | SDL_PEN_MOUSEID))
        && !(touchpad && super::touchpad::pointer_from_touchpad(which));
    match kind {
        SDL_EVENT_MOUSE_MOTION if active && ordered == 0 && own => Verdict::Sent,
        SDL_EVENT_MOUSE_BUTTON_DOWN | SDL_EVENT_MOUSE_BUTTON_UP | SDL_EVENT_MOUSE_WHEEL => Verdict::Ordered,
        _ => Verdict::Pass,
    }
}

/// The key a key event sends from the filter; None leaves it to the main loop: nothing lent,
/// something queued ahead (`held`: clicks, then keys and controller events), a stream shortcut,
/// or a key without a Windows code.
fn direct_key(rules: Option<Rules>, held: (u32, u32), scancode: SDL_Scancode, mods: SDL_Keymod, down: bool) -> Option<KeyInput> {
    let keys = rules?.keys.filter(|_| held == (0, 0))?;
    if down && crate::shortcuts_runtime::key_action(crate::shortcuts::Scope::Stream, scancode, mods).is_some() {
        return None;
    }
    key_input(scancode, mods, keys.system_keys, keys.right_alt_as_meta)
}

/// Whether a click or wheel turn leaves from the filter (`own`: a mouse's, not touch, pen or the
/// touchpad's, which the main loop sorts out).
fn direct_click(rules: Option<Rules>, held: (u32, u32), own: bool) -> Option<ClickRules> {
    rules?.clicks.filter(|_| own && held == (0, 0))
}

/// Whether a controller event goes to the lent controllers.
fn direct_pad(rules: Option<Rules>, held: (u32, u32)) -> bool {
    rules.is_some_and(|r| r.pads) && held == (0, 0)
}

/// Window events that act as a barrier: no direct motion, key, click or controller event after
/// them may pass until the main loop has drained the event and updated focus/capture state.
fn is_barrier(kind: SDL_EventType) -> bool {
    matches!(kind, SDL_EVENT_WINDOW_FOCUS_LOST | SDL_EVENT_WINDOW_MINIMIZED | SDL_EVENT_WINDOW_HIDDEN)
}

/// Controller events `Gamepads::handle` turns into what the PC gets.
fn applies_to_pads(kind: SDL_EventType) -> bool {
    matches!(
        kind,
        SDL_EVENT_GAMEPAD_BUTTON_DOWN
            | SDL_EVENT_GAMEPAD_BUTTON_UP
            | SDL_EVENT_GAMEPAD_AXIS_MOTION
            | SDL_EVENT_GAMEPAD_TOUCHPAD_DOWN
            | SDL_EVENT_GAMEPAD_TOUCHPAD_MOTION
            | SDL_EVENT_GAMEPAD_TOUCHPAD_UP
            | SDL_EVENT_GAMEPAD_SENSOR_UPDATE
            | SDL_EVENT_JOYSTICK_BATTERY_UPDATED
    )
}

unsafe extern "C" fn filter(userdata: *mut c_void, event: *mut SDL_Event) -> bool {
    // SAFETY: `userdata` is the Shared that DirectMotion keeps alive until it removes the filter,
    // and SDL calls the filter under the lock SDL_SetEventFilter takes; `event` is SDL's event.
    unsafe { route(&*(userdata as *const Shared), &mut *event) }
}

/// The filter: true lets the event through to the main loop, marked; false drops it, sent.
///
/// # Safety
/// `event` is an event SDL built.
unsafe fn route(shared: &Shared, event: &mut SDL_Event) -> bool {
    // SAFETY: union members read by event type.
    unsafe {
        let kind = SDL_EventType(event.r#type);
        let held = (shared.ordered.load(Ordering::Acquire), shared.queued.load(Ordering::Acquire));
        let touchpad = shared.touchpad.load(Ordering::Acquire);
        let which = match kind {
            SDL_EVENT_MOUSE_MOTION => event.motion.which,
            _ => SDL_MouseID(0),
        };
        match verdict(kind, which, shared.active.load(Ordering::Acquire), held.0, shared.touch, touchpad) {
            Verdict::Sent => {
                let m = event.motion;
                shared.send(m.xrel, m.yrel, f32::from_bits(shared.speed.load(Ordering::Relaxed)));
                shared.sent_direct.fetch_add(1, Ordering::Relaxed);
                return false;
            }
            Verdict::Ordered => {
                let own = !touchpad
                    && !(kind != SDL_EVENT_MOUSE_WHEEL && shared.touch && matches!(event.button.which, SDL_TOUCH_MOUSEID | SDL_PEN_MOUSEID));
                let sent = shared.direct(|rules, _| {
                    let clicks = direct_click(Some(rules), held, own)?;
                    if kind == SDL_EVENT_MOUSE_WHEEL {
                        shared.input.input(|q| send_wheel(&event.wheel, q));
                    } else if let Some(button) = mouse_button_code(event.button.button, clicks.nav_buttons) {
                        set_sent(&mut lock(&shared.sent_down).buttons, button, event.button.down);
                        shared.input.input(|q| { let _ = q.send_mouse_button(button, event.button.down); });
                    }
                    Some(())
                });
                if sent.is_some() {
                    shared.sent_input.fetch_add(1, Ordering::Relaxed);
                    mark(event, SENT, None);
                } else {
                    mark(event, ORDERED, Some(&shared.ordered));
                }
                return true;
            }
            Verdict::Pass => {}
        }
        match kind {
            SDL_EVENT_KEY_DOWN | SDL_EVENT_KEY_UP if event.key.repeat => mark(event, 0, None),
            SDL_EVENT_KEY_DOWN | SDL_EVENT_KEY_UP => {
                let k = event.key;
                let sent = shared.direct(|rules, _| {
                    let key = direct_key(Some(rules), held, k.scancode, k.r#mod, k.down)?;
                    set_sent(&mut lock(&shared.sent_down).keys, key.code, k.down);
                    shared.input.input(|q| { let _ = q.send_keyboard(0x8000 | key.code, k.down, key.modifiers, key.flags); });
                    Some(())
                });
                if sent.is_some() {
                    shared.sent_input.fetch_add(1, Ordering::Relaxed);
                    mark(event, SENT, None);
                } else {
                    mark(event, QUEUED, Some(&shared.queued));
                }
            }
            kind if applies_to_pads(kind) => {
                if lock(&shared.pad_activity).is_active(event) {
                    shared.last_pad_activity_us.store(nightjar_core::session::now_us(), Ordering::Relaxed);
                }
                let view: &SDL_Event = event;
                let applied = shared.direct(|rules, frozen| {
                    let Lent(pads) = frozen.pads.as_mut().filter(|_| direct_pad(Some(rules), held))?;
                    Some(shared.input.input(|q| pads.handle(view, q)))
                });
                match applied {
                    Some(None) => {
                        shared.sent_input.fetch_add(1, Ordering::Relaxed);
                        return false;
                    }
                    Some(Some(action)) => mark(event, acted_mark(action), Some(&shared.queued)),
                    None => mark(event, QUEUED, Some(&shared.queued)),
                }
            }
            // A controller's later events wait until the main loop has opened or closed it.
            SDL_EVENT_GAMEPAD_ADDED => mark(event, QUEUED, Some(&shared.queued)),
            SDL_EVENT_GAMEPAD_REMOVED => {
                lock(&shared.pad_activity).remove_pad(event.gdevice.which.0);
                mark(event, QUEUED, Some(&shared.queued));
            }
            kind if is_barrier(kind) => mark_barrier(event, &shared.ordered, &shared.queued),
            _ => mark(event, 0, None),
        }
        true
    }
}

fn acted_mark(action: Action) -> u32 {
    ACTED + ACTIONS.iter().position(|a| *a == action).expect("every action has a mark") as u32
}

fn action_of(mark: u32) -> Option<Action> {
    ACTIONS.get(mark.checked_sub(ACTED)? as usize).copied()
}

/// Marks a queued event and counts it when events wait behind it. Every event the filter lets
/// through is marked (0 for none): SDL builds events on the stack without clearing them, so a
/// mark left from an earlier event must not count as one.
fn mark(event: &mut SDL_Event, mark: u32, waits: Option<&AtomicU32>) {
    if let Some(count) = waits {
        count.fetch_add(1, Ordering::AcqRel);
    }
    event.common.reserved = mark;
}

fn mark_barrier(event: &mut SDL_Event, ordered: &AtomicU32, queued: &AtomicU32) {
    ordered.fetch_add(1, Ordering::AcqRel);
    queued.fetch_add(1, Ordering::AcqRel);
    event.common.reserved = BARRIER;
}

fn release_one(count: &AtomicU32) {
    let mut n = count.load(Ordering::Acquire);
    while n > 0 {
        match count.compare_exchange_weak(n, n - 1, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => break,
            Err(now) => n = now,
        }
    }
}

/// The main loop is done with `event`: a marked one no longer holds others back.
fn release(ordered: &AtomicU32, queued: &AtomicU32, event: &SDL_Event) {
    // SAFETY: every event starts with the common header.
    match unsafe { event.common.reserved } {
        ORDERED => release_one(ordered),
        QUEUED => release_one(queued),
        BARRIER => {
            release_one(ordered);
            release_one(queued);
        }
        mark if action_of(mark).is_some() => release_one(queued),
        _ => {}
    }
}

/// Whether the filter sent this key, click or wheel turn: the main loop keeps its state and sends
/// nothing.
pub fn sent(event: &SDL_Event) -> bool {
    // SAFETY: every event starts with the common header.
    unsafe { event.common.reserved == SENT }
}

/// The action of a controller event the filter applied; the main loop only acts on it.
pub fn acted(event: &SDL_Event) -> Option<Action> {
    // SAFETY: every event starts with the common header.
    action_of(unsafe { event.common.reserved })
}

/// The main loop's controllers on loan for its present ([`DirectMotion::lend`]).
pub struct Loan(Arc<Mutex<Frozen>>);

impl Loan {
    /// The controllers for the loop's own drawing during the present; the filter leaves them
    /// alone meanwhile.
    pub fn pads(&self) -> Pads<'_> {
        Pads(lock(&self.0))
    }

    /// The controllers back, after the present.
    pub fn take_back(self) -> Gamepads {
        lock(&self.0).pads.take().expect("the controllers lent for the present").0
    }
}

/// The lent controllers, locked for the main loop.
pub struct Pads<'a>(MutexGuard<'a, Frozen>);

impl Deref for Pads<'_> {
    type Target = Gamepads;
    fn deref(&self) -> &Gamepads {
        &self.0.pads.as_ref().expect("the controllers lent for the present").0
    }
}

impl DerefMut for Pads<'_> {
    fn deref_mut(&mut self) -> &mut Gamepads {
        &mut self.0.pads.as_mut().expect("the controllers lent for the present").0
    }
}

/// Sets what the loop lends for its present (None once it returns); true when the controllers
/// are to be read meanwhile.
fn freeze(frozen: &Mutex<Frozen>, rules: Option<Rules>) -> bool {
    let mut frozen = lock(frozen);
    frozen.rules = rules;
    rules.is_some_and(|r| r.pads) && frozen.pads.as_ref().is_some_and(|Lent(pads)| !pads.pads.is_empty())
}

#[derive(Default)]
struct PollState {
    polling: bool,
    quit: bool,
}

/// The thread that reads the controllers while the main loop waits in the present: their events
/// go through the filter on it.
struct Poller {
    state: Arc<(Mutex<PollState>, Condvar)>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Poller {
    fn start(last_pad_us: Arc<AtomicU64>) -> Option<Self> {
        let state = Arc::new((Mutex::new(PollState::default()), Condvar::new()));
        let shared = state.clone();
        match std::thread::Builder::new().name("nightjar-controllers".into()).spawn(move || poll_controllers(&shared, &last_pad_us)) {
            Ok(thread) => Some(Self { state, thread: Some(thread) }),
            Err(e) => {
                eprintln!("nightjar: controllers wait for the main loop: no thread to read them ({e})");
                None
            }
        }
    }

    fn set(&self, polling: bool) {
        let (state, wake) = &*self.state;
        let mut state = lock(state);
        if state.polling != polling {
            state.polling = polling;
            wake.notify_one();
        }
    }
}

impl Drop for Poller {
    fn drop(&mut self) {
        let (state, wake) = &*self.state;
        lock(state).quit = true;
        wake.notify_one();
        if let Some(thread) = self.thread.take()
            && thread.join().is_err() {
                eprintln!("nightjar: the controller thread panicked");
            }
    }
}

fn poll_controllers(state: &(Mutex<PollState>, Condvar), last_pad_us: &AtomicU64) {
    // SAFETY: sets this thread's priority.
    if !unsafe { SDL_SetCurrentThreadPriority(SDL_THREAD_PRIORITY_HIGH) } {
        let sdl = crate::sdl::error();
        // A Flatpak or other unprivileged Linux process may not lower its nice value, and SDL's
        // own attempt failed: the shared fallback asks the desktop portal and RealtimeKit, at
        // the decode thread's level.
        #[cfg(target_os = "linux")]
        match nightjar_core::priority::raise_current_thread(nightjar_core::priority::ThreadLevel::Decode) {
            Ok(Some(raised)) => eprintln!("nightjar: raised the controller thread to {raised}"),
            Ok(None) => {}
            Err(e) => eprintln!("nightjar: the controller thread keeps normal priority: {sdl}; {e}"),
        }
        #[cfg(not(target_os = "linux"))]
        eprintln!("nightjar: the controller thread keeps normal priority: {sdl}");
    }
    let (state, wake) = state;
    loop {
        {
            let mut state = lock(state);
            while !state.polling && !state.quit {
                state = wake.wait(state).unwrap_or_else(|p| p.into_inner());
            }
            if state.quit {
                return;
            }
        }
        // SAFETY: SDL updates joysticks from any thread; their events go through the filter here.
        unsafe { SDL_UpdateJoysticks() };
        let sleep_ns = poller_sleep_ns(nightjar_core::session::now_us(), last_pad_us.load(Ordering::Relaxed));
        // SAFETY: plain high-resolution timer sleep (no CPU busy-spin).
        unsafe { SDL_DelayNS(sleep_ns) };
    }
}

/// Whether SDL's keyboard grab is on while this stream's hook runs: only when the grab is wanted
/// but the hook could not start. SDL's hook beside it would be the first one Windows asks, and it
/// answers only between the main loop's waits in the present.
#[cfg_attr(not(windows), allow(dead_code))]
fn sdl_grab(grab: bool, hooked: bool) -> bool {
    grab && !hooked
}

/// A window's HWND and SDL id, for the hook.
#[cfg(windows)]
fn window_ids(window: *mut SDL_Window) -> (isize, SDL_WindowID) {
    // SAFETY: a live window.
    unsafe {
        (SDL_GetPointerProperty(SDL_GetWindowProperties(window), SDL_PROP_WINDOW_WIN32_HWND_POINTER, std::ptr::null_mut()) as isize, SDL_GetWindowID(window))
    }
}

/// The filter for one stream; removed when dropped.
pub struct DirectMotion {
    shared: Arc<Shared>,
    installed: bool,
    /// Keys, clicks and controllers may leave from the filter during the present.
    direct_input: bool,
    poller: Option<Poller>,
    /// Takes the keyboard from Windows while it is grabbed, the shell's shortcuts included.
    #[cfg(windows)]
    shell_keys: Option<super::system_keys::ShellKeyHook>,
    #[cfg(windows)]
    tab_guard: bool,
}

impl DirectMotion {
    /// Installs the filter, unless `NIGHTJAR_DIRECT_MOTION=0` (measurement only) keeps all input
    /// on the main loop; `NIGHTJAR_DIRECT_INPUT=0` (measurement only) keeps keys, clicks and
    /// controllers there.
    pub fn new(input: InputHandle, touch: bool) -> Self {
        let shared = Arc::new(Shared {
            input,
            active: AtomicBool::new(false),
            speed: AtomicU32::new(1.0f32.to_bits()),
            touch,
            touchpad: AtomicBool::new(false),
            remainder: Mutex::new((0.0, 0.0)),
            ordered: AtomicU32::new(0),
            queued: AtomicU32::new(0),
            frozen: Arc::new(Mutex::new(Frozen::default())),
            sent_down: Mutex::new(SentDown::default()),
            sent_direct: AtomicU64::new(0),
            sent_queued: AtomicU64::new(0),
            sent_input: AtomicU64::new(0),
            last_pad_activity_us: Arc::new(AtomicU64::new(0)),
            pad_activity: Mutex::new(PadActivity::default()),
        });
        let installed = std::env::var("NIGHTJAR_DIRECT_MOTION").map_or(true, |v| v.trim() != "0");
        let direct_input = installed && std::env::var("NIGHTJAR_DIRECT_INPUT").map_or(true, |v| v.trim() != "0");
        let mut poller = None;
        if installed {
            // The main loop's once-per-refresh pace used to merge a 4 or 8 kHz mouse's reports;
            // now the queue does, at most one packet per 0.5 ms.
            shared.input.input(|q| q.set_relative_motion_floor(FAST_MOUSE_FLOOR));
            // SAFETY: the Arc outlives the filter (Drop removes it first).
            unsafe { SDL_SetEventFilter(Some(filter), Arc::as_ptr(&shared) as *mut c_void) };
        } else {
            eprintln!("nightjar: mouse motion through the main loop (NIGHTJAR_DIRECT_MOTION=0)");
        }
        if direct_input {
            poller = Poller::start(shared.last_pad_activity_us.clone());
        } else if installed {
            eprintln!("nightjar: keys, clicks and controllers through the main loop (NIGHTJAR_DIRECT_INPUT=0)");
        }
        Self { shared, installed, direct_input, poller, #[cfg(windows)] shell_keys: None, #[cfg(windows)] tab_guard: false }
    }

    /// The keyboard grab ("Send system keys" while captured) changed: this stream's own hook
    /// takes the keyboard from Windows while it is on and hands the keys to SDL (`system_keys`).
    /// Returns whether SDL's grab should do that instead: on Linux, or without the hook.
    #[cfg_attr(not(windows), allow(unused_variables))]
    pub fn keyboard_grab(&mut self, window: *mut SDL_Window, grab: bool) -> bool {
        #[cfg(windows)]
        {
            if grab && self.shell_keys.is_none() {
                match super::system_keys::ShellKeyHook::start() {
                    Ok(hook) => {
                        eprintln!("nightjar: system keys: the keyboard hook takes the keyboard while grabbed");
                        self.shell_keys = Some(hook);
                    }
                    Err(error) => eprintln!("nightjar: system keys through SDL's grab: keyboard hook: {error}"),
                }
            }
            if let Some(hook) = &mut self.shell_keys {
                let (hwnd, id) = window_ids(window);
                hook.set(grab, hwnd, id);
                return sdl_grab(grab, true);
            }
        }
        grab
    }

    /// Whether the hook keeps the Tab of a stream hotkey Windows would take without the grab
    /// (`system_keys::tab_guard`); checked once per pass, it costs nothing until it changes. The
    /// hook starts for it only when it turns on.
    #[cfg(windows)]
    pub fn tab_guard(&mut self, window: *mut SDL_Window, on: bool) {
        if on == self.tab_guard {
            return;
        }
        self.tab_guard = on;
        if on && self.shell_keys.is_none() {
            match super::system_keys::ShellKeyHook::start() {
                Ok(hook) => {
                    eprintln!("nightjar: system keys: the keyboard hook keeps the Tab of Ctrl+Alt+Shift+Tab");
                    self.shell_keys = Some(hook);
                }
                Err(error) => eprintln!("nightjar: Ctrl+Alt+Shift+Tab stays Windows': keyboard hook: {error}"),
            }
        }
        if let Some(hook) = &mut self.shell_keys {
            let (hwnd, id) = window_ids(window);
            hook.guard(on, hwnd, id);
        }
    }

    /// Whether motion may leave from the filter: captured, relative and focused.
    pub fn set_active(&self, active: bool, mouse_speed: u32) {
        self.shared.speed.store(speed_factor(mouse_speed).to_bits(), Ordering::Relaxed);
        self.shared.active.store(active && self.installed, Ordering::Release);
    }

    /// Touchpad contacts go to the PC from now on: the touchpad's pointer moves stay on the main
    /// loop, which tells them from the mouse's.
    pub fn touchpad_contacts(&self) {
        self.shared.touchpad.store(true, Ordering::Release);
    }

    /// Relative motion the main loop handles (the filter let it through).
    pub fn send(&self, xrel: f32, yrel: f32, mouse_speed: u32) {
        self.shared.send(xrel, yrel, speed_factor(mouse_speed));
        self.shared.sent_queued.fetch_add(1, Ordering::Relaxed);
    }

    /// Call once the main loop is done with `event` (every event it takes while streaming): a
    /// click, key or controller event it has sent, or the menu took, no longer holds others back.
    pub fn dispatched(&self, event: &SDL_Event) {
        release(&self.shared.ordered, &self.shared.queued, event);
    }

    /// Lends the controllers until [`Loan::take_back`], around the main loop's present.
    pub fn lend(&self, pads: Gamepads) -> Loan {
        lock(&self.shared.frozen).pads = Some(Lent(pads));
        Loan(self.shared.frozen.clone())
    }

    /// Around the present's wait: what the main loop would do with keys, clicks and controllers
    /// meanwhile, then None once it returns. Controllers are read while it waits.
    pub fn presenting(&self, rules: Option<Rules>) {
        if !self.direct_input {
            return;
        }
        let poll = freeze(&self.shared.frozen, rules);
        if let Some(poller) = &self.poller {
            poller.set(poll);
        }
    }

    /// How many moves left from the filter and from the main loop.
    pub fn counts(&self) -> (u64, u64) {
        (self.shared.sent_direct.load(Ordering::Relaxed), self.shared.sent_queued.load(Ordering::Relaxed))
    }

    /// How many keys, clicks, wheel turns and controller events left from the filter.
    pub fn input_count(&self) -> u64 {
        self.shared.sent_input.load(Ordering::Relaxed)
    }

    /// Monotonic session timestamp (µs) of the last active controller input seen by the filter (0 if none).
    pub fn last_pad_activity_us(&self) -> u64 {
        self.shared.last_pad_activity_us.load(Ordering::Relaxed)
    }

    /// Takes keys and mouse buttons sent DOWN directly by the filter so `raise_all_keys()` releases them.
    pub fn take_sent_down(&self) -> (HashSet<u16>, HashSet<u8>) { take_sent_down_from(&self.shared.sent_down) }
    pub fn is_sent_key_down(&self, code: u16) -> bool { lock(&self.shared.sent_down).keys.contains(&code) }
    pub fn is_sent_button_down(&self, button: u8) -> bool { lock(&self.shared.sent_down).buttons.contains(&button) }
    pub fn clear_sent_key(&self, code: u16) { lock(&self.shared.sent_down).keys.remove(&code); }
    pub fn clear_sent_button(&self, button: u8) { lock(&self.shared.sent_down).buttons.remove(&button); }
}

fn set_sent<T: Eq + std::hash::Hash>(set: &mut HashSet<T>, item: T, down: bool) { if down { set.insert(item); } else { set.remove(&item); } }

fn take_sent_down_from(sent_down: &Mutex<SentDown>) -> (HashSet<u16>, HashSet<u8>) { let mut d = lock(sent_down); (std::mem::take(&mut d.keys), std::mem::take(&mut d.buttons)) }

impl Drop for DirectMotion {
    fn drop(&mut self) {
        // The controller thread ends first: none of its events reach a filter that is gone.
        self.poller = None;
        if self.installed {
            // SAFETY: SDL takes the filter lock, so no call is running once this returns.
            unsafe { SDL_SetEventFilter(None, std::ptr::null_mut()) };
        }
        #[cfg(windows)]
        {
            self.shell_keys = None;
        }
    }
}

#[cfg(test)]
mod tests;
