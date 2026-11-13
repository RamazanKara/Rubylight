//! "Send system keys" on Windows: while the stream has the keyboard, a low-level keyboard hook on
//! a thread of its own takes the keys from Windows, the shell's shortcuts (Alt+Tab, Alt+Shift+Tab,
//! the Windows key and Win+key, Alt+Esc, Ctrl+Esc) included, and puts them in SDL's queue itself,
//! where the input filter sends them to the PC at once.
//!
//! nightjar#71: 4.4.0 kept only the shell's keys from Windows and read the others through SDL's
//! raw keyboard input, but with a raw keyboard registered in the process the hook never saw a
//! press, so Alt+Tab still switched away (in a test window as well: Alt+Tab, Ctrl+Alt+Shift+Tab
//! and Alt+Esc all left it, and none did without the raw keyboard). Raw input does not report a
//! key a hook kept either, so the raw keyboard stays off and the hook hands over every key it
//! keeps. SDL's grab installs the same kind of hook, but on the main thread, which answers it only
//! between its waits in the present; Windows passes on a key whose hook answers late and may
//! remove the hook without a word. This thread does nothing but answer the hook, so no key waits
//! for a refresh either. Ctrl+Alt+Del is never a hook's.
//!
//! The lock keys and media keys stay Windows' and come through the window on the main thread,
//! so one pressed a moment before a kept key can reach the PC after it (Caps Lock, then a letter
//! within a frame). Keeping them would leave Windows' lock state and lights behind.
//!
//! A main loop that stopped beating ([`alive`]) for [`ALIVE_MS`] gets no keys kept: a frozen
//! fullscreen stream lets Alt+Tab, the Windows key and Ctrl+Shift+Esc reach Windows, as SDL's
//! hook on the hung thread did.
//!
//! Windows opens its task switcher at Ctrl+Alt+Shift+Tab before the window hears of it, and takes
//! that chord for itself as it does Alt+Tab: RegisterHotKey refuses it (ERROR_HOTKEY_ALREADY_REGISTERED).
//! So a stream hotkey on it that can act now needs a hook without the grab too, with the stream
//! menu open, input released or "Send system keys" off: the hook keeps that one Tab ([`tab_guard`],
//! [`ShellKeyHook::guard`]). A stream that never grabbed has the hook only while the guard is on,
//! so its window in front, and any key but Tab goes straight on ([`passes_by`]).

#[cfg(windows)]
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, AtomicU64, AtomicU8, Ordering};

use sdl3_sys::everything::{SDL_Keymod, SDL_Scancode};

/// The keys the hook takes while grabbed, by virtual key: all but the lock keys (Caps, Num and
/// Scroll Lock), whose state and lights stay Windows' and which SDL reads from the window as
/// before, and VK_PACKET, a touch keyboard's characters.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn kept_key(vk: u32) -> bool {
    !matches!(vk, 0x14 | 0x90 | 0x91 | 0xE7)
}

/// SDL's scancode for a key's scan code and extended flag, as SDL reads a window's key messages
/// (scancodes_windows.h), plus Alt+Print Screen (0x54); None for a key it has none for: a media
/// key, which Windows keeps and SDL hands over, or an added one ([`route`]).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn sdl_scancode(scan: u32, extended: bool) -> Option<SDL_Scancode> {
    use sdl3_sys::everything::*;
    if scan == 0 || scan > 0xFF {
        return None;
    }
    let s = scan & 0x7F;
    let code = if extended {
        match s {
            0x1C => SDL_SCANCODE_KP_ENTER,
            0x1D => SDL_SCANCODE_RCTRL,
            0x35 => SDL_SCANCODE_KP_DIVIDE,
            0x36 => SDL_SCANCODE_RSHIFT,
            0x37 => SDL_SCANCODE_PRINTSCREEN,
            0x38 => SDL_SCANCODE_RALT,
            0x45 => SDL_SCANCODE_NUMLOCKCLEAR,
            0x46 => SDL_SCANCODE_PAUSE,
            0x47 => SDL_SCANCODE_HOME,
            0x48 => SDL_SCANCODE_UP,
            0x49 => SDL_SCANCODE_PAGEUP,
            0x4B => SDL_SCANCODE_LEFT,
            0x4D => SDL_SCANCODE_RIGHT,
            0x4F => SDL_SCANCODE_END,
            0x50 => SDL_SCANCODE_DOWN,
            0x51 => SDL_SCANCODE_PAGEDOWN,
            0x52 => SDL_SCANCODE_INSERT,
            0x53 => SDL_SCANCODE_DELETE,
            0x5B => SDL_SCANCODE_LGUI,
            0x5C => SDL_SCANCODE_RGUI,
            0x5D => SDL_SCANCODE_APPLICATION,
            // Media and browser keys: Windows acts on them, SDL hands them over.
            _ => return None,
        }
    } else {
        match s {
            0x01 => SDL_SCANCODE_ESCAPE,
            0x02..=0x0B => SDL_Scancode(SDL_SCANCODE_1.0 + (s - 0x02) as i32),
            0x0C => SDL_SCANCODE_MINUS,
            0x0D => SDL_SCANCODE_EQUALS,
            0x0E => SDL_SCANCODE_BACKSPACE,
            0x0F => SDL_SCANCODE_TAB,
            0x10 => SDL_SCANCODE_Q,
            0x11 => SDL_SCANCODE_W,
            0x12 => SDL_SCANCODE_E,
            0x13 => SDL_SCANCODE_R,
            0x14 => SDL_SCANCODE_T,
            0x15 => SDL_SCANCODE_Y,
            0x16 => SDL_SCANCODE_U,
            0x17 => SDL_SCANCODE_I,
            0x18 => SDL_SCANCODE_O,
            0x19 => SDL_SCANCODE_P,
            0x1A => SDL_SCANCODE_LEFTBRACKET,
            0x1B => SDL_SCANCODE_RIGHTBRACKET,
            0x1C => SDL_SCANCODE_RETURN,
            0x1D => SDL_SCANCODE_LCTRL,
            0x1E => SDL_SCANCODE_A,
            0x1F => SDL_SCANCODE_S,
            0x20 => SDL_SCANCODE_D,
            0x21 => SDL_SCANCODE_F,
            0x22 => SDL_SCANCODE_G,
            0x23 => SDL_SCANCODE_H,
            0x24 => SDL_SCANCODE_J,
            0x25 => SDL_SCANCODE_K,
            0x26 => SDL_SCANCODE_L,
            0x27 => SDL_SCANCODE_SEMICOLON,
            0x28 => SDL_SCANCODE_APOSTROPHE,
            0x29 => SDL_SCANCODE_GRAVE,
            0x2A => SDL_SCANCODE_LSHIFT,
            0x2B => SDL_SCANCODE_BACKSLASH,
            0x2C => SDL_SCANCODE_Z,
            0x2D => SDL_SCANCODE_X,
            0x2E => SDL_SCANCODE_C,
            0x2F => SDL_SCANCODE_V,
            0x30 => SDL_SCANCODE_B,
            0x31 => SDL_SCANCODE_N,
            0x32 => SDL_SCANCODE_M,
            0x33 => SDL_SCANCODE_COMMA,
            0x34 => SDL_SCANCODE_PERIOD,
            0x35 => SDL_SCANCODE_SLASH,
            0x36 => SDL_SCANCODE_RSHIFT,
            0x37 => SDL_SCANCODE_KP_MULTIPLY,
            0x38 => SDL_SCANCODE_LALT,
            0x39 => SDL_SCANCODE_SPACE,
            0x3A => SDL_SCANCODE_CAPSLOCK,
            0x3B..=0x44 => SDL_Scancode(SDL_SCANCODE_F1.0 + (s - 0x3B) as i32),
            // Pause comes as 0x45 without the extended flag (Num Lock has it).
            0x45 => SDL_SCANCODE_PAUSE,
            0x46 => SDL_SCANCODE_SCROLLLOCK,
            0x47 => SDL_SCANCODE_KP_7,
            0x48 => SDL_SCANCODE_KP_8,
            0x49 => SDL_SCANCODE_KP_9,
            0x4A => SDL_SCANCODE_KP_MINUS,
            0x4B => SDL_SCANCODE_KP_4,
            0x4C => SDL_SCANCODE_KP_5,
            0x4D => SDL_SCANCODE_KP_6,
            0x4E => SDL_SCANCODE_KP_PLUS,
            0x4F => SDL_SCANCODE_KP_1,
            0x50 => SDL_SCANCODE_KP_2,
            0x51 => SDL_SCANCODE_KP_3,
            0x52 => SDL_SCANCODE_KP_0,
            0x53 => SDL_SCANCODE_KP_PERIOD,
            // Print Screen with Alt held (SysRq): Windows' screenshot shortcut.
            0x54 => SDL_SCANCODE_PRINTSCREEN,
            0x56 => SDL_SCANCODE_NONUSBACKSLASH,
            0x57 => SDL_SCANCODE_F11,
            0x58 => SDL_SCANCODE_F12,
            0x59 => SDL_SCANCODE_KP_EQUALS,
            0x5C => SDL_SCANCODE_INTERNATIONAL6,
            0x64..=0x6E => SDL_Scancode(SDL_SCANCODE_F13.0 + (s - 0x64) as i32),
            0x70 => SDL_SCANCODE_INTERNATIONAL2,
            0x71 => SDL_SCANCODE_LANG2,
            0x72 => SDL_SCANCODE_LANG1,
            0x73 => SDL_SCANCODE_INTERNATIONAL1,
            0x76 => SDL_SCANCODE_F24,
            0x77 => SDL_SCANCODE_LANG4,
            0x78 => SDL_SCANCODE_LANG3,
            0x79 => SDL_SCANCODE_INTERNATIONAL4,
            0x7B => SDL_SCANCODE_INTERNATIONAL5,
            0x7D => SDL_SCANCODE_INTERNATIONAL3,
            0x7E => SDL_SCANCODE_KP_COMMA,
            _ => return None,
        }
    };
    Some(code)
}

/// The modifiers held for a key the hook hands over: SDL never sees the ones it keeps, so they
/// come from `kept` (by virtual key), and `windows` adds the ones SDL got from the window (the
/// lock keys, a modifier held from before the grab).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn modifiers(kept: impl Fn(u32) -> bool, windows: SDL_Keymod) -> SDL_Keymod {
    use sdl3_sys::everything::*;
    let held = [
        (0xA0, SDL_KMOD_LSHIFT),
        (0xA1, SDL_KMOD_RSHIFT),
        (0xA2, SDL_KMOD_LCTRL),
        (0xA3, SDL_KMOD_RCTRL),
        (0xA4, SDL_KMOD_LALT),
        (0xA5, SDL_KMOD_RALT),
        (0x5B, SDL_KMOD_LGUI),
        (0x5C, SDL_KMOD_RGUI),
    ];
    SDL_Keymod(held.iter().filter(|(vk, _)| kept(*vk)).fold(windows.0, |mods, (_, flag)| mods | flag.0))
}

/// A key as the hook last left it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(windows), allow(dead_code))]
pub enum Key {
    Up = 0,
    /// Windows holds it: its press reached Windows, or it was down when the hook started.
    Passed = 1,
    /// Its press was kept from Windows.
    Kept = 2,
}

#[cfg_attr(not(windows), allow(dead_code))]
impl Key {
    fn from_u8(value: u8) -> Self {
        match value {
            1 => Key::Passed,
            2 => Key::Kept,
            _ => Key::Up,
        }
    }
}

/// One key event through the hook: whether it is kept from Windows (and handed to SDL), and the
/// key's state after, for a key SDL has a scancode for. `grab`: the keyboard is the stream's, its
/// window is in front and the main loop beats. A key goes one way from its press to its release,
/// auto-repeats included: one Windows already holds (Win of the Win+number that brought the
/// stream to the front) stays Windows' until released, or Windows would keep it down; one kept
/// stays kept after the grab ends, so Windows never sees half of it.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn step(grab: bool, vk: u32, down: bool, key: Key) -> (bool, Key) {
    match (down, key) {
        (true, Key::Up) if grab && kept_key(vk) => (true, Key::Kept),
        (true, Key::Kept) => (true, Key::Kept),
        (true, _) => (false, Key::Passed),
        (false, Key::Kept) => (true, Key::Up),
        (false, _) => (false, Key::Up),
    }
}

/// Corrects a key's state at a press before [`step`]. `held`: Windows already holds it (a low-
/// level hook runs before Windows applies the event), so a press the table missed, between the
/// seed and the hook going in, is a repeat of Windows' and not a new one to keep. `idle_ms`: since
/// the key's last event; a kept key whose release the hook never saw (Ctrl+Alt+Del took it to the
/// secure desktop) is pressed afresh once the grab is off and it has been quiet a while.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn settle(grab: bool, key: Key, held: bool, idle_ms: u64) -> Key {
    let key = if key == Key::Kept && !grab && idle_ms > REPEAT_MS { Key::Up } else { key };
    if key == Key::Up && held { Key::Passed } else { key }
}

/// Whether a kept key's event is an auto-repeat: a press while it is kept and the last key
/// pressed (Windows repeats only that one), within [`REPEAT_MS`] of its last event. A release the
/// hook never saw (Ctrl+Alt+Del, Win+L: the secure desktop took it) leaves the key kept, and the
/// next press is a new one, or the stream would drop it as a repeat (the first Ctrl+C after).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn repeats(key: Key, down: bool, idle_ms: u64, last_pressed: bool) -> bool {
    down && key == Key::Kept && last_pressed && idle_ms <= REPEAT_MS
}

/// Longer than any first auto-repeat takes: Windows' longest delay is a second, a PS/2
/// keyboard's own timing is 20 % off either way, and the hook's clock ticks in 15.6 ms steps.
#[cfg_attr(not(windows), allow(dead_code))]
pub const REPEAT_MS: u64 = 2000;

const _: () = assert!(REPEAT_MS > 1000 * 6 / 5 + 16);

/// The scan code of the left Ctrl Windows adds to AltGr.
#[cfg_attr(not(windows), allow(dead_code))]
pub const ALTGR_CTRL: u32 = 0x21D;

/// What the hook does with one key event.
#[cfg_attr(not(windows), allow(dead_code))]
pub struct Routed {
    /// Kept from Windows.
    pub swallow: bool,
    /// The state of the event's slot after it; None leaves the slot and its time as they were.
    pub next: Option<Key>,
    /// The key for SDL's queue: its scancode and whether it repeats.
    pub hand_over: Option<(SDL_Scancode, bool)>,
}

/// Routes one key event: `key` is the state of its slot (its virtual key's, or AltGr's added
/// Ctrl's own), `idle_ms` the time since that slot's last event, `held` whether Windows holds the
/// virtual key, `last_pressed` whether it was the last key pressed.
///
/// An event without an SDL scancode is no key of its own but one Windows or the keyboard adds,
/// and never changes its virtual key's state, which belongs to the real key, so a real Ctrl or
/// Shift held around it stays down for the PC. The Shifts a laptop keyboard wraps around the
/// arrows (extended 0x2A) are kept while the real Shift is. AltGr's Ctrl has a slot of its own and
/// goes as its press went, never to SDL: kept while grabbed, as SDL's grab drops it (Windows would
/// hold a Ctrl the stream had: AltGr+Shift+Z as Nightjar's Ctrl+Alt+Shift+Z), and its release
/// reaches Windows when its press did.
#[cfg_attr(not(windows), allow(dead_code))]
#[allow(clippy::too_many_arguments)]
pub fn route(grab: bool, vk: u32, scan: u32, extended: bool, down: bool, key: Key, held: bool, idle_ms: u64, last_pressed: bool) -> Routed {
    if scan == ALTGR_CTRL {
        let key = if down { settle(grab, key, false, idle_ms) } else { key };
        let (swallow, next) = step(grab, vk, down, key);
        return Routed { swallow, next: Some(next), hand_over: None };
    }
    let Some(scancode) = sdl_scancode(scan, extended) else {
        return Routed { swallow: key == Key::Kept, next: None, hand_over: None };
    };
    let key = if down { settle(grab, key, held, idle_ms) } else { key };
    let (swallow, next) = step(grab, vk, down, key);
    Routed { swallow, next: Some(next), hand_over: swallow.then(|| (scancode, repeats(key, down, idle_ms, last_pressed))) }
}

/// How long the main loop may go without a beat before the hook lets every key through: above
/// its longest wait (500 ms with no picture due) plus a slow frame.
#[cfg_attr(not(windows), allow(dead_code))]
pub const ALIVE_MS: u64 = 1000;

/// Whether the main loop's last beat at `beat_ms` is recent at `now_ms`.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn beating(now_ms: u64, beat_ms: u64) -> bool {
    now_ms.saturating_sub(beat_ms) < ALIVE_MS
}

/// The modifiers of the one Tab chord a hotkey can have that Windows takes ahead of the window:
/// nightjar_policy refuses Alt+Tab, Alt+Shift+Tab and Ctrl+Alt+Tab, and Windows leaves Ctrl+Tab and
/// Ctrl+Shift+Tab to the window.
#[cfg_attr(not(windows), allow(dead_code))]
fn tab_chord_mods() -> SDL_Keymod {
    use sdl3_sys::everything::*;
    SDL_Keymod(SDL_KMOD_LCTRL.0 | SDL_KMOD_LALT.0 | SDL_KMOD_LSHIFT.0)
}

/// The stream hotkey on that chord, if any.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn tab_chord_action() -> Option<crate::shortcuts::Action> {
    use crate::shortcuts::{key_action, Scope};
    key_action(Scope::Stream, sdl3_sys::everything::SDL_SCANCODE_TAB, tab_chord_mods())
}

/// Whether the hook keeps the chord's Tab without the grab: its window has the keyboard and `action`,
/// the hotkey on the chord, does something now. With the stream menu open only its own chords act
/// (`menu_step`, the rest go to its widgets), and Microphone only leaves the chord to Windows.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn tab_guard(action: Option<crate::shortcuts::Action>, focused: bool, grab: bool, menu_visible: bool, mic_only: bool) -> bool {
    use crate::shortcuts::Action;
    focused && !grab && !mic_only && action.is_some_and(|action| !menu_visible || matches!(action, Action::StreamControls | Action::StreamShortcuts))
}

/// Whether exactly the chord's modifiers are held (`held`, by left and right virtual key). `altgr`:
/// AltGr's added Ctrl is down, so a left Ctrl in Windows' state is no Ctrl of the user's.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn tab_chord_held(held: impl Fn(u32) -> bool, altgr: bool) -> bool {
    (held(0xA0) || held(0xA1)) && ((held(0xA2) && !altgr) || held(0xA3)) && (held(0xA4) || held(0xA5)) && !held(0x5B) && !held(0x5C)
}

/// Whether the chord counts a modifier as held: Windows holds it, or the grab kept it, as with the
/// modifiers still down from the chord that opened the menu. A kept one quiet for longer than
/// [`settle`] allows does not count: its release may be lost (Ctrl+Alt+Del leaves Ctrl and Alt kept,
/// and Shift+Tab is not the chord).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn tab_chord_modifier(windows: bool, key: Key, idle_ms: u64) -> bool {
    windows || (key == Key::Kept && idle_ms <= REPEAT_MS)
}

/// Whether an event goes straight on, with no look at the foreground or Windows' key state: nothing
/// is grabbed and nothing kept of this key. Tab may be the chord's, and AltGr's added Ctrl has to
/// remember how its press went, as a press Windows held then repeats into a grab.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn passes_by(grabbed: bool, vk: u32, altgr: bool, key: Key) -> bool {
    !grabbed && vk != TAB && !altgr && key == Key::Up
}

/// What a change of the guard does to the hook: on, a new one, as Windows may have removed the last
/// without a word; off, none in a stream that never grabbed (the grab's hook stays to the end, so a
/// kept key is released where it was pressed).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(windows), allow(dead_code))]
pub enum GuardStep {
    Reinstall,
    Remove,
    Keep,
}

#[cfg_attr(not(windows), allow(dead_code))]
pub fn guard_step(on: bool, ever_grabbed: bool) -> GuardStep {
    match (on, ever_grabbed) {
        (true, _) => GuardStep::Reinstall,
        (false, false) => GuardStep::Remove,
        (false, true) => GuardStep::Keep,
    }
}

/// Whether the hook can go now that nothing guards the chord. Not with its Tab kept: a hotkey that
/// took the focus leaves the chord held, and the Tab's next repeat would reach Windows.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn unhooks_now(tab: Key) -> bool {
    tab != Key::Kept
}

/// Whether the keyboard is grabbed: input is captured (keys go to the PC, the stream menu is
/// closed, the window is the PC's input) and "Send system keys" is on. Windowed or fullscreen,
/// relative or desktop pointer alike; out of focus, the hook and SDL let the keys be.
pub fn keyboard_grab(captured: bool, system_keys: bool) -> bool {
    captured && system_keys
}

/// "Send system keys" as it holds now: with "System keys only in full screen", only while the
/// stream's window is full screen. In a window those keys then stay with this computer, as
/// they do with Send system keys off.
pub fn sent(system_keys: bool, fullscreen_only: bool, fullscreen: bool) -> bool {
    system_keys && (!fullscreen_only || fullscreen)
}

#[cfg(windows)]
static GRAB: AtomicBool = AtomicBool::new(false);
/// The hook keeps the chord's Tab without the grab ([`tab_guard`]).
#[cfg(windows)]
static GUARD: AtomicBool = AtomicBool::new(false);
/// The hook goes at the Tab's release ([`unhooks_now`]).
#[cfg(windows)]
static UNHOOK_AT_RELEASE: AtomicBool = AtomicBool::new(false);
/// The stream window's HWND: the hook takes keys only while it is in front.
#[cfg(windows)]
static WINDOW: AtomicIsize = AtomicIsize::new(0);
/// The stream window's SDL id, for the key events the hook hands over.
#[cfg(windows)]
static WINDOW_ID: AtomicU32 = AtomicU32::new(0);
/// The main loop's last beat, in GetTickCount64 milliseconds.
#[cfg(windows)]
static BEAT: AtomicU64 = AtomicU64::new(0);
/// [`Key`] by virtual key; only the hook's thread writes it.
#[cfg(windows)]
static KEYS: [AtomicU8; 256] = [const { AtomicU8::new(Key::Up as u8) }; 256];
/// Each key's last event, in the hook's event time (milliseconds).
#[cfg(windows)]
static LAST: [AtomicU32; 256] = [const { AtomicU32::new(0) }; 256];
/// AltGr's added Ctrl ([`ALTGR_CTRL`]) as the hook last left it, and its last event's time: a
/// slot of its own beside the real left Ctrl's.
#[cfg(windows)]
static ALTGR_CTRL_KEY: AtomicU8 = AtomicU8::new(Key::Up as u8);
#[cfg(windows)]
static ALTGR_CTRL_LAST: AtomicU32 = AtomicU32::new(0);
/// The virtual key last pressed: Windows repeats only that one.
#[cfg(windows)]
static LAST_PRESSED: AtomicU32 = AtomicU32::new(u32::MAX);
#[cfg_attr(not(windows), allow(dead_code))]
const TAB: u32 = 0x09;
/// The thread message that reinstalls the hook.
#[cfg(windows)]
const REHOOK: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP;
/// The thread message that removes it.
#[cfg(windows)]
const UNHOOK: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 1;

#[cfg(windows)]
fn now_ms() -> u64 {
    // SAFETY: no arguments.
    unsafe { windows::Win32::System::SystemInformation::GetTickCount64() }
}

/// The main loop beats once per pass: the hook keeps keys only while it does.
pub fn alive() {
    #[cfg(windows)]
    BEAT.store(now_ms(), Ordering::Release);
}

/// Sets each key from Windows' state when the hook starts: a key already down is Windows' until
/// its release. `fresh`: a new stream, where no press is kept; a reinstall keeps the kept ones
/// (Windows' state never saw them).
#[cfg(windows)]
fn seed(fresh: bool) {
    use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
    if fresh {
        ALTGR_CTRL_KEY.store(Key::Up as u8, Ordering::Relaxed);
        LAST_PRESSED.store(u32::MAX, Ordering::Relaxed);
    }
    for (vk, key) in KEYS.iter().enumerate() {
        if !fresh && Key::from_u8(key.load(Ordering::Relaxed)) == Key::Kept {
            continue;
        }
        // SAFETY: any virtual key.
        let down = unsafe { GetAsyncKeyState(vk as i32) } < 0;
        key.store(if down { Key::Passed } else { Key::Up } as u8, Ordering::Relaxed);
    }
}

/// Whether a modifier is really down, by its slot: one kept is, Windows knows of one it holds, and
/// AltGr's added Ctrl, which has a slot of its own, is no real Ctrl.
#[cfg(windows)]
fn modifier_down(vk: u32) -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
    match Key::from_u8(KEYS[vk as usize].load(Ordering::Relaxed)) {
        Key::Up => false,
        Key::Kept => true,
        Key::Passed => {
            // SAFETY: any virtual key.
            let state = unsafe { GetAsyncKeyState(vk as i32) };
            state < 0
        }
    }
}

/// Hands a key the hook kept to SDL's queue as the window would have: SDL's filter runs on this
/// thread and sends it to the PC at once, and the main loop gets it marked, as it got the raw
/// keyboard's. `raw`: Windows' scan code, as SDL reports it. `also`: modifiers known held, whose
/// window messages SDL may not have read yet.
#[cfg(windows)]
fn hand_over(scancode: SDL_Scancode, raw: u16, down: bool, repeat: bool, also: SDL_Keymod) {
    use sdl3_sys::everything::*;
    // SAFETY: SDL takes events and reads its modifier state from any thread; a key event's fields.
    let pushed = unsafe {
        // The modifiers Windows holds too: the window's messages for them may still be on their way.
        let mods = modifiers(modifier_down, SDL_Keymod(SDL_GetModState().0 | also.0));
        let mut event: SDL_Event = std::mem::zeroed();
        event.key.r#type = if down { SDL_EVENT_KEY_DOWN } else { SDL_EVENT_KEY_UP };
        event.key.windowID = SDL_WindowID(WINDOW_ID.load(Ordering::Acquire));
        // SDL_GLOBAL_KEYBOARD_ID, as SDL sends a window's keys.
        event.key.which = SDL_KeyboardID(0);
        event.key.scancode = scancode;
        // The keycode stays SDLK_UNKNOWN: SDL's keymap is the main thread's (a layout change
        // frees it), and a kept key types no text, so the text field must not wait for its echo
        // (text_field::types is false for it). The PC and Nightjar's shortcuts go by scancode.
        event.key.key = SDLK_UNKNOWN;
        event.key.r#mod = mods;
        event.key.raw = raw;
        event.key.down = down;
        event.key.repeat = repeat;
        SDL_PushEvent(&mut event)
    };
    if !pushed {
        eprintln!("nightjar: a key kept from Windows was lost: {}", crate::sdl::error());
    }
}

#[cfg(windows)]
unsafe extern "system" fn hook(code: i32, wparam: windows::Win32::Foundation::WPARAM, lparam: windows::Win32::Foundation::LPARAM) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::Foundation::LRESULT;
    use windows::Win32::UI::WindowsAndMessaging::*;
    if code == HC_ACTION as i32 {
        // SAFETY: Windows passes a KBDLLHOOKSTRUCT with HC_ACTION.
        let info = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        if let Some(key) = KEYS.get(info.vkCode as usize) {
            let down = matches!(wparam.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
            let (scan, extended) = if info.scanCode != 0 {
                (info.scanCode, info.flags.0 & LLKHF_EXTENDED.0 != 0)
            } else {
                // A key injected by virtual key alone: its scan code, as SDL looks it up.
                use windows::Win32::UI::Input::KeyboardAndMouse::{MapVirtualKeyW, MAPVK_VK_TO_VSC_EX};
                // SAFETY: a virtual key from Windows.
                match unsafe { MapVirtualKeyW(info.vkCode, MAPVK_VK_TO_VSC_EX) } {
                    0xE11D => (0x45, false),
                    scan => (scan & 0xFF, scan >> 8 == 0xE0),
                }
            };
            let altgr = scan == ALTGR_CTRL;
            let (slot, last) = if altgr { (&ALTGR_CTRL_KEY, &ALTGR_CTRL_LAST) } else { (key, &LAST[info.vkCode as usize]) };
            let state = Key::from_u8(slot.load(Ordering::Relaxed));
            let grabbed = GRAB.load(Ordering::Acquire);
            if passes_by(grabbed, info.vkCode, altgr, state) {
                if down {
                    LAST_PRESSED.store(info.vkCode, Ordering::Relaxed);
                }
                // SAFETY: the arguments Windows passed.
                return unsafe { CallNextHookEx(None, code, wparam, lparam) };
            }
            let guarded = !grabbed && down && info.vkCode == TAB && GUARD.load(Ordering::Acquire) && tab_chord_held(|vk| {
                use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
                let vk = vk as usize;
                // SAFETY: any virtual key.
                let state = unsafe { GetAsyncKeyState(vk as i32) };
                tab_chord_modifier(state < 0, Key::from_u8(KEYS[vk].load(Ordering::Relaxed)), u64::from(info.time.wrapping_sub(LAST[vk].load(Ordering::Relaxed))))
            }, Key::from_u8(ALTGR_CTRL_KEY.load(Ordering::Relaxed)) != Key::Up);
            // SAFETY: no arguments.
            let grab = (grabbed || guarded)
                && unsafe { GetForegroundWindow() }.0 as isize == WINDOW.load(Ordering::Acquire)
                && beating(now_ms(), BEAT.load(Ordering::Acquire));
            let idle_ms = u64::from(info.time.wrapping_sub(last.load(Ordering::Relaxed)));
            let held = down && !altgr && {
                use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
                // SAFETY: a virtual key from Windows.
                let state = unsafe { GetAsyncKeyState(info.vkCode as i32) };
                state < 0
            };
            let last_pressed = LAST_PRESSED.load(Ordering::Relaxed) == info.vkCode;
            let routed = route(grab, info.vkCode, scan, extended, down, state, held, idle_ms, last_pressed);
            if let Some(next) = routed.next {
                slot.store(next as u8, Ordering::Relaxed);
                last.store(info.time, Ordering::Relaxed);
                if down && !altgr {
                    LAST_PRESSED.store(info.vkCode, Ordering::Relaxed);
                }
            }
            if let Some((scancode, repeat)) = routed.hand_over {
                let raw = scan as u16 | if extended { 0xE000 } else { 0 };
                hand_over(scancode, raw, down, repeat, if guarded { tab_chord_mods() } else { SDL_Keymod(0) });
            }
            if info.vkCode == TAB && routed.next == Some(Key::Up) && UNHOOK_AT_RELEASE.swap(false, Ordering::Relaxed) {
                use windows::Win32::Foundation::{LPARAM, WPARAM};
                // SAFETY: no arguments, then this thread's own queue, which it reads after this event.
                if let Err(error) = unsafe { PostThreadMessageW(windows::Win32::System::Threading::GetCurrentThreadId(), UNHOOK, WPARAM(0), LPARAM(0)) } {
                    eprintln!("nightjar: the keyboard hook stays as it was: {error}");
                }
            }
            if routed.swallow {
                return LRESULT(1);
            }
        }
    }
    // SAFETY: the arguments Windows passed.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// Installs the hook on the calling thread, which must pump its messages.
#[cfg(windows)]
fn install() -> windows::core::Result<windows::Win32::UI::WindowsAndMessaging::HHOOK> {
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{SetWindowsHookExW, WH_KEYBOARD_LL};
    // SAFETY: the hook procedure lives as long as the program.
    unsafe { GetModuleHandleW(None).and_then(|module| SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook), Some(module.into()), 0)) }
}

/// The hook's thread, from the first grab or guard to the end of the stream.
#[cfg(windows)]
pub struct ShellKeyHook {
    thread_id: u32,
    ever_grabbed: bool,
    thread: Option<std::thread::JoinHandle<()>>,
}

#[cfg(windows)]
impl ShellKeyHook {
    /// Starts the thread and installs the hook; the error when Windows refused it.
    pub fn start() -> Result<Self, String> {
        use windows::Win32::System::Threading::{GetCurrentThread, GetCurrentThreadId, SetThreadPriority, THREAD_PRIORITY_TIME_CRITICAL};
        use windows::Win32::UI::WindowsAndMessaging::*;
        let (tx, rx) = std::sync::mpsc::channel();
        let thread = std::thread::Builder::new().name("nightjar-system-keys".into()).spawn(move || {
            // Windows drops a hook that answers late; under load this thread must still run first.
            // SAFETY: this thread's pseudo handle.
            let _ = unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_TIME_CRITICAL) };
            GRAB.store(false, Ordering::Release);
            GUARD.store(false, Ordering::Release);
            UNHOOK_AT_RELEASE.store(false, Ordering::Relaxed);
            seed(true);
            let mut hook = match install() {
                Ok(hook) => Some(hook),
                Err(error) => {
                    let _ = tx.send(Err(error.to_string()));
                    return;
                }
            };
            // SAFETY: no arguments.
            let _ = tx.send(Ok(unsafe { GetCurrentThreadId() }));
            let mut message = MSG::default();
            // SAFETY: this thread's queue; WM_QUIT (Drop) ends it.
            while unsafe { GetMessageW(&mut message, None, 0, 0) }.0 > 0 {
                if !matches!(message.message, REHOOK | UNHOOK) {
                    continue;
                }
                UNHOOK_AT_RELEASE.store(false, Ordering::Relaxed);
                if message.message == UNHOOK && !unhooks_now(Key::from_u8(KEYS[TAB as usize].load(Ordering::Relaxed))) {
                    UNHOOK_AT_RELEASE.store(true, Ordering::Relaxed);
                    continue;
                }
                // Windows may have removed the hook without a word: a new one each time the grab
                // or the guard turns on.
                if let Some(old) = hook.take() {
                    // SAFETY: the hook this thread installed.
                    let _ = unsafe { UnhookWindowsHookEx(old) };
                }
                if message.message == UNHOOK {
                    continue;
                }
                seed(false);
                match install() {
                    Ok(new) => hook = Some(new),
                    Err(error) => eprintln!("nightjar: system keys and the Ctrl+Alt+Shift+Tab hotkey reach Windows until the next grab or focus: keyboard hook: {error}"),
                }
            }
            if let Some(hook) = hook {
                // SAFETY: the hook this thread installed.
                let _ = unsafe { UnhookWindowsHookEx(hook) };
            }
        }).map_err(|error| error.to_string())?;
        match rx.recv() {
            Ok(Ok(thread_id)) => Ok(Self { thread_id, ever_grabbed: false, thread: Some(thread) }),
            Ok(Err(error)) => Err(error),
            Err(_) => Err("the hook thread ended".into()),
        }
    }

    /// Whether the hook takes the keyboard, from which window (`hwnd`) and for which SDL window
    /// (`window_id`). Turning the grab on reinstalls the hook.
    pub fn set(&mut self, grab: bool, hwnd: isize, window_id: sdl3_sys::everything::SDL_WindowID) {
        Self::window(hwnd, window_id);
        self.ever_grabbed |= grab;
        if grab && !GRAB.swap(grab, Ordering::AcqRel) {
            self.post(REHOOK);
        } else {
            GRAB.store(grab, Ordering::Release);
        }
    }

    /// Whether the hook keeps the chord's Tab without the grab, from which window; call it when
    /// that changes ([`tab_guard`]).
    pub fn guard(&mut self, on: bool, hwnd: isize, window_id: sdl3_sys::everything::SDL_WindowID) {
        Self::window(hwnd, window_id);
        GUARD.store(on, Ordering::Release);
        match guard_step(on, self.ever_grabbed) {
            GuardStep::Reinstall => self.post(REHOOK),
            GuardStep::Remove => self.post(UNHOOK),
            GuardStep::Keep => {}
        }
    }

    fn window(hwnd: isize, window_id: sdl3_sys::everything::SDL_WindowID) {
        alive();
        WINDOW_ID.store(window_id.0, Ordering::Release);
        WINDOW.store(hwnd, Ordering::Release);
    }

    fn post(&self, message: u32) {
        use windows::Win32::Foundation::{LPARAM, WPARAM};
        use windows::Win32::UI::WindowsAndMessaging::PostThreadMessageW;
        // SAFETY: the hook thread's id.
        if let Err(error) = unsafe { PostThreadMessageW(self.thread_id, message, WPARAM(0), LPARAM(0)) } {
            eprintln!("nightjar: the keyboard hook stays as it was: {error}");
        }
    }
}

#[cfg(windows)]
impl Drop for ShellKeyHook {
    fn drop(&mut self) {
        use windows::Win32::Foundation::{LPARAM, WPARAM};
        use windows::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, WM_QUIT};
        GRAB.store(false, Ordering::Release);
        GUARD.store(false, Ordering::Release);
        // SAFETY: the hook thread's id; it ends at WM_QUIT.
        if unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) }.is_ok() {
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }
}

#[cfg(test)]
mod tests;
