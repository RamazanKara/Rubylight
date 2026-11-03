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
//! thread reads the controllers every millisecond, as SDL does while the loop waits for events.
//! A key or click from SDL's raw input thread (Windows reads the keyboard there while "Capture
//! system keys" grabs it) and a controller event from that thread then leave at once; the loop still gets the keys and
//! clicks, marked, and keeps its state without sending them again. Anything the loop sends or
//! decides on itself (a stream shortcut, an event from before the present, a new controller, a
//! controller chord's action) holds the keys, clicks and controller events after it back, so
//! none overtakes it. Linux reads keys and clicks on the main thread: there only controllers
//! gain.

use std::ffi::c_void;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

use nightjar_core::input::queue::FAST_MOUSE_FLOOR;
use nightjar_core::session::InputHandle;
use sdl3_sys::everything::*;

use super::input::{key_input, mouse_button_code, send_wheel, KeyInput};
use crate::gamepad::{Action, Gamepads};

/// Marks a click or wheel event that motion waits behind until the main loop has sent it.
const ORDERED: u32 = 0x4E4A_4D4F;
/// Marks a key or controller event the main loop handles itself: keys, clicks and controller
/// events after it wait for it too (motion does not).
const QUEUED: u32 = 0x4E4A_5155;
/// Marks a key, click or wheel turn the filter sent: the main loop keeps its state, sends nothing.
const SENT: u32 = 0x4E4A_534E;
/// Marks a controller event the filter applied that asks the main loop to act, plus the action's
/// place in [`ACTIONS`]; it holds later events back like [`QUEUED`].
const ACTED: u32 = 0x4E4A_4100;
const ACTIONS: [Action; 4] = [Action::Quit, Action::Menu, Action::Mouse(false), Action::Mouse(true)];
/// How often the controllers are read while the main loop waits in the present: as often as SDL
/// reads them while the loop waits for events.
const POLL: Duration = Duration::from_millis(1);

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
    sent_direct: AtomicU64,
    sent_queued: AtomicU64,
    /// Keys, clicks, wheel turns and controller events that left from the filter.
    sent_input: AtomicU64,
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
    fn direct<R>(&self, send: impl FnOnce(Rules, &mut Option<Lent>) -> Option<R>) -> Option<R> {
        let mut frozen = self.frozen.try_lock().ok()?;
        let frozen = &mut *frozen;
        send(frozen.rules?, &mut frozen.pads)
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

/// The key a key event sends from the filter; None leaves it to the main loop: nothing lent,
/// something queued ahead (`held`: clicks, then keys and controller events), a stream shortcut,
/// or a key without a Windows code.
fn direct_key(rules: Option<Rules>, held: (u32, u32), scancode: SDL_Scancode, mods: SDL_Keymod, down: bool) -> Option<KeyInput> {
    let keys = rules?.keys.filter(|_| held == (0, 0))?;
    if down && crate::shortcuts::key_action(crate::shortcuts::Scope::Stream, scancode, mods).is_some() {
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
                    shared.input.input(|q| match kind {
                        SDL_EVENT_MOUSE_WHEEL => send_wheel(&event.wheel, q),
                        // A button the PC doesn't get: the main loop drops it too.
                        _ => if let Some(button) = mouse_button_code(event.button.button, clicks.nav_buttons) {
                            let _ = q.send_mouse_button(button, event.button.down);
                        },
                    });
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
                    shared.input.input(|q| {
                        let _ = q.send_keyboard(0x8000 | key.code, k.down, key.modifiers, key.flags);
                    });
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
                let view: &SDL_Event = event;
                let applied = shared.direct(|rules, pads| {
                    let Lent(pads) = pads.as_mut().filter(|_| direct_pad(Some(rules), held))?;
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
            SDL_EVENT_GAMEPAD_ADDED | SDL_EVENT_GAMEPAD_REMOVED => mark(event, QUEUED, Some(&shared.queued)),
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

/// The main loop is done with `event`: a marked one no longer holds others back.
fn release(ordered: &AtomicU32, queued: &AtomicU32, event: &SDL_Event) {
    // SAFETY: every event starts with the common header.
    let count = match unsafe { event.common.reserved } {
        ORDERED => ordered,
        QUEUED => queued,
        mark if action_of(mark).is_some() => queued,
        _ => return,
    };
    let mut n = count.load(Ordering::Acquire);
    while n > 0 {
        match count.compare_exchange_weak(n, n - 1, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => break,
            Err(now) => n = now,
        }
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
    fn start() -> Option<Self> {
        let state = Arc::new((Mutex::new(PollState::default()), Condvar::new()));
        let shared = state.clone();
        match std::thread::Builder::new().name("nightjar-controllers".into()).spawn(move || poll_controllers(&shared)) {
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
        if let Some(thread) = self.thread.take() {
            if thread.join().is_err() {
                eprintln!("nightjar: the controller thread panicked");
            }
        }
    }
}

fn poll_controllers(state: &(Mutex<PollState>, Condvar)) {
    // SAFETY: sets this thread's priority.
    if !unsafe { SDL_SetCurrentThreadPriority(SDL_THREAD_PRIORITY_HIGH) } {
        eprintln!("nightjar: the controller thread keeps normal priority: {}", crate::sdl::error());
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
        std::thread::sleep(POLL);
    }
}

/// Whether Windows reads the keyboard on SDL's raw input thread, where keys can leave during the
/// present: only while the keyboard is grabbed. Without the grab Windows keeps the second key of
/// its own shortcuts from the window (Tab of Alt+Tab, D of Win+D, Esc of Ctrl+Shift+Esc), but raw
/// input still reports it, so the PC would get Alt+Tab or a typed "d" as well.
#[cfg_attr(not(windows), allow(dead_code))]
fn raw_keyboard(direct_input: bool, grab: bool) -> bool {
    direct_input && grab
}

/// Whether SDL's keyboard grab is on while this stream's hook runs: only when the grab is wanted
/// but the hook does not take it (no raw keyboard). SDL's hook beside it would be the first one
/// Windows asks, and it answers only between the main loop's waits in the present.
#[cfg_attr(not(windows), allow(dead_code))]
fn sdl_grab(grab: bool, hooked: bool) -> bool {
    grab && !hooked
}

/// Turns Windows' raw keyboard on or off. SDL reports a raw input thread it could not set up only
/// through its error, not the hint's result. Returns whether it worked.
#[cfg(windows)]
fn set_raw_keyboard(on: bool) -> bool {
    // SAFETY: SDL applies the hint at once on this thread; the error is this thread's.
    let failed = unsafe {
        SDL_ClearError();
        let set = if on { SDL_SetHint(SDL_HINT_WINDOWS_RAW_KEYBOARD, c"1".as_ptr()) } else { SDL_ResetHint(SDL_HINT_WINDOWS_RAW_KEYBOARD) };
        !set || !crate::sdl::error().is_empty()
    };
    if failed {
        let state = if on { "keys wait for the main loop" } else { "the keyboard stays on raw input" };
        eprintln!("nightjar: {state}: raw keyboard input: {}", crate::sdl::error());
    }
    !failed
}

/// The filter for one stream; removed when dropped.
pub struct DirectMotion {
    shared: Arc<Shared>,
    installed: bool,
    /// Keys, clicks and controllers may leave from the filter during the present.
    direct_input: bool,
    poller: Option<Poller>,
    /// This stream turned Windows' raw keyboard on.
    #[cfg_attr(not(windows), allow(dead_code))]
    raw_keyboard: bool,
    /// Windows reads the keyboard as raw input (SDL could set it up).
    #[cfg(windows)]
    raw_reads: bool,
    /// The shell's shortcuts stay the stream's while the raw keyboard reads the keys.
    #[cfg(windows)]
    shell_keys: Option<super::system_keys::ShellKeyHook>,
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
            sent_direct: AtomicU64::new(0),
            sent_queued: AtomicU64::new(0),
            sent_input: AtomicU64::new(0),
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
            poller = Poller::start();
        } else if installed {
            eprintln!("nightjar: keys, clicks and controllers through the main loop (NIGHTJAR_DIRECT_INPUT=0)");
        }
        Self { shared, installed, direct_input, poller, raw_keyboard: false, #[cfg(windows)] raw_reads: false, #[cfg(windows)] shell_keys: None }
    }

    /// The keyboard grab ("Send system keys" while captured) changed: Windows reads the
    /// keyboard on SDL's raw input thread only while it is on ([`raw_keyboard`]), and then this
    /// stream's own hook keeps the shell's shortcuts from Windows (`system_keys`). Returns whether
    /// SDL's grab should do that instead: on Linux, without the raw keyboard, or without the hook.
    #[cfg_attr(not(windows), allow(unused_variables))]
    pub fn keyboard_grab(&mut self, window: *mut SDL_Window, grab: bool) -> bool {
        #[cfg(windows)]
        {
            let on = raw_keyboard(self.direct_input, grab);
            if on != self.raw_keyboard {
                self.raw_keyboard = on;
                self.raw_reads = set_raw_keyboard(on) && on;
            }
            // Without raw input the keys the hook keeps would reach nobody: SDL's hook sends them.
            let on = on && self.raw_reads;
            if on && self.shell_keys.is_none() {
                match super::system_keys::ShellKeyHook::start() {
                    Ok(hook) => self.shell_keys = Some(hook),
                    Err(error) => eprintln!("nightjar: system keys through SDL's grab: keyboard hook: {error}"),
                }
            }
            if let Some(hook) = &self.shell_keys {
                // SAFETY: a live window.
                let hwnd = unsafe { SDL_GetPointerProperty(SDL_GetWindowProperties(window), SDL_PROP_WINDOW_WIN32_HWND_POINTER, std::ptr::null_mut()) };
                hook.set(on, hwnd as isize);
                return sdl_grab(grab, on);
            }
        }
        grab
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
}

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
        #[cfg(windows)]
        if self.raw_keyboard {
            set_raw_keyboard(false);
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

    fn event(kind: SDL_EventType) -> SDL_Event {
        // SAFETY: SDL_Event is plain data; all zero is a valid value.
        let mut event: SDL_Event = unsafe { std::mem::zeroed() };
        event.r#type = kind.0;
        event
    }

    /// Button down, then a motion SDL built where the marked down was, then button up: the stale
    /// mark is cleared, so the queued button up still holds motion back after the down went out.
    #[test]
    fn a_stale_mark_does_not_let_motion_pass_a_queued_release() {
        let (ordered, queued) = (AtomicU32::new(0), AtomicU32::new(0));
        let mut down = event(SDL_EVENT_MOUSE_BUTTON_DOWN);
        mark(&mut down, ORDERED, Some(&ordered));
        let mut motion = down;
        motion.r#type = SDL_EVENT_MOUSE_MOTION.0;
        mark(&mut motion, 0, None);
        let mut up = down;
        up.r#type = SDL_EVENT_MOUSE_BUTTON_UP.0;
        mark(&mut up, ORDERED, Some(&ordered));
        assert_eq!(ordered.load(Ordering::Acquire), 2);
        release(&ordered, &queued, &down);
        release(&ordered, &queued, &motion);
        assert_eq!(ordered.load(Ordering::Acquire), 1, "the release still waits");
        release(&ordered, &queued, &up);
        release(&ordered, &queued, &up);
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

    const KEYS: KeyRules = KeyRules { system_keys: false, right_alt_as_meta: false };
    const PRESENTING: Rules = Rules { keys: Some(KEYS), clicks: Some(ClickRules { nav_buttons: true }), pads: true };

    /// Outside the present the main loop reads its events itself and may change what they do, so
    /// nothing leaves from the filter then.
    #[test]
    fn keys_clicks_and_controllers_leave_from_the_filter_only_during_the_present() {
        let a = direct_key(Some(PRESENTING), (0, 0), SDL_SCANCODE_A, SDL_KMOD_NONE, true).expect("A while presenting");
        assert_eq!(Some(a), key_input(SDL_SCANCODE_A, SDL_KMOD_NONE, false, false), "mapped as the main loop maps it");
        assert!(direct_click(Some(PRESENTING), (0, 0), true).is_some());
        assert!(direct_pad(Some(PRESENTING), (0, 0)));
        assert_eq!(direct_key(None, (0, 0), SDL_SCANCODE_A, SDL_KMOD_NONE, true), None);
        assert_eq!(direct_click(None, (0, 0), true), None);
        assert!(!direct_pad(None, (0, 0)));
        let menu = Rules { keys: None, clicks: None, pads: false };
        assert_eq!(direct_key(Some(menu), (0, 0), SDL_SCANCODE_A, SDL_KMOD_NONE, true), None, "the menu's");
        assert_eq!(direct_click(Some(menu), (0, 0), true), None);
        assert!(!direct_pad(Some(menu), (0, 0)));
    }

    /// Shift held through the main loop, then a click during the present: the click waits, or the
    /// PC would get it before Shift. A queued key or controller event holds keys, clicks and
    /// controller events back; motion only waits for clicks.
    #[test]
    fn anything_queued_holds_the_direct_events_after_it() {
        for held in [(1, 0), (0, 1), (2, 3)] {
            assert_eq!(direct_key(Some(PRESENTING), held, SDL_SCANCODE_A, SDL_KMOD_NONE, false), None, "{held:?}");
            assert_eq!(direct_click(Some(PRESENTING), held, true), None, "{held:?}");
            assert!(!direct_pad(Some(PRESENTING), held), "{held:?}");
        }
        assert_eq!(verdict(SDL_EVENT_MOUSE_MOTION, SDL_MouseID(7), true, 0, false, false), Verdict::Sent, "behind a key");
    }

    /// Nightjar's own chords and F12 (a screenshot) stay with the main loop, and their release
    /// goes out as the main loop sends it; a key without a Windows code is the loop's to drop.
    #[test]
    fn shortcuts_and_keys_without_a_code_stay_on_the_main_loop() {
        let chord = SDL_Keymod(SDL_KMOD_CTRL.0 | SDL_KMOD_ALT.0 | SDL_KMOD_SHIFT.0);
        assert_eq!(direct_key(Some(PRESENTING), (0, 0), SDL_SCANCODE_TAB, chord, true), None, "stream controls");
        assert_eq!(direct_key(Some(PRESENTING), (0, 0), SDL_SCANCODE_F12, SDL_KMOD_NONE, true), None, "screenshot");
        assert!(direct_key(Some(PRESENTING), (0, 0), SDL_SCANCODE_TAB, chord, false).is_some(), "the chord's release");
        assert!(direct_key(Some(PRESENTING), (0, 0), SDL_SCANCODE_TAB, SDL_KMOD_NONE, true).is_some(), "Tab alone");
        assert_eq!(direct_key(Some(PRESENTING), (0, 0), SDL_SCANCODE_UNKNOWN, SDL_KMOD_NONE, true), None);
    }

    #[test]
    fn touch_pen_and_touchpad_clicks_stay_on_the_main_loop() {
        assert_eq!(direct_click(Some(PRESENTING), (0, 0), false), None);
    }

    /// A sent key or click is the main loop's to keep track of, never to send again, and holds
    /// nothing back; queued keys, controller events and applied actions hold until handled.
    #[test]
    fn marks_tell_the_main_loop_what_was_sent_and_what_still_holds() {
        let (ordered, queued) = (AtomicU32::new(0), AtomicU32::new(0));
        let mut key = event(SDL_EVENT_KEY_DOWN);
        mark(&mut key, SENT, None);
        assert!(sent(&key));
        assert_eq!(acted(&key), None);
        release(&ordered, &queued, &key);
        assert_eq!((ordered.load(Ordering::Acquire), queued.load(Ordering::Acquire)), (0, 0));
        mark(&mut key, QUEUED, Some(&queued));
        assert!(!sent(&key));
        let mut pad = event(SDL_EVENT_GAMEPAD_BUTTON_DOWN);
        for action in ACTIONS {
            mark(&mut pad, acted_mark(action), Some(&queued));
            assert_eq!(acted(&pad), Some(action));
            assert!(!sent(&pad));
            release(&ordered, &queued, &pad);
        }
        assert_eq!(queued.load(Ordering::Acquire), 1, "the queued key still holds");
        release(&ordered, &queued, &key);
        assert_eq!(queued.load(Ordering::Acquire), 0);
        for other in [0, ORDERED, QUEUED, SENT] {
            assert_eq!(action_of(other), None, "{other:#x}");
        }
    }

    #[test]
    fn the_raw_keyboard_is_on_only_while_the_keyboard_is_grabbed() {
        // "Capture system keys" off, or not captured: Windows keeps the keys of its shortcuts
        // (Tab of Alt+Tab, D of Win+D) from the window, and raw input must not hand them over.
        assert!(!raw_keyboard(true, false));
        assert!(raw_keyboard(true, true), "grabbed: SDL's hook keeps them from Windows");
        assert!(!raw_keyboard(false, true), "NIGHTJAR_DIRECT_INPUT=0 leaves the keyboard alone");
    }

    #[test]
    fn sdls_grab_steps_aside_while_the_hook_takes_the_shells_keys() {
        // nightjar#71: SDL's hook on the main thread answered late, and Windows took Alt+Tab.
        assert!(!sdl_grab(true, true), "the hook's thread answers Windows instead");
        assert!(sdl_grab(true, false), "no raw keyboard or no hook: SDL's grab as before");
        assert!(!sdl_grab(false, false));
    }

    #[test]
    fn controller_input_goes_to_the_lent_controllers_and_devices_stay_on_the_main_loop() {
        for kind in [SDL_EVENT_GAMEPAD_BUTTON_DOWN, SDL_EVENT_GAMEPAD_BUTTON_UP, SDL_EVENT_GAMEPAD_AXIS_MOTION, SDL_EVENT_GAMEPAD_SENSOR_UPDATE] {
            assert!(applies_to_pads(kind), "{:#x}", kind.0);
        }
        for kind in [SDL_EVENT_GAMEPAD_ADDED, SDL_EVENT_GAMEPAD_REMOVED, SDL_EVENT_JOYSTICK_BUTTON_DOWN, SDL_EVENT_KEY_DOWN] {
            assert!(!applies_to_pads(kind), "{:#x}", kind.0);
        }
    }

    /// The controllers go to the filter for the present and come back unchanged; the loop draws
    /// with them meanwhile, and nothing is read without a controller or once the present returns.
    #[test]
    fn lent_controllers_come_back_and_are_read_only_during_the_present() {
        let frozen = Arc::new(Mutex::new(Frozen::default()));
        let mut pads = Gamepads::default();
        pads.swap_face_buttons = true;
        lock(&frozen).pads = Some(Lent(pads));
        let loan = Loan(frozen.clone());
        assert!(loan.pads().swap_face_buttons, "the loop draws with its own controllers");
        assert!(!freeze(&frozen, Some(PRESENTING)), "no controller to read");
        assert_eq!(lock(&frozen).rules, Some(PRESENTING));
        assert!(!freeze(&frozen, None));
        assert_eq!(lock(&frozen).rules, None);
        assert!(loan.take_back().swap_face_buttons);
        assert!(lock(&frozen).pads.is_none());
    }
}
