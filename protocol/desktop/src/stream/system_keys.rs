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
mod tests {
    use super::*;

    const ESC: u32 = 0x1B;
    const LWIN: u32 = 0x5B;
    const RWIN: u32 = 0x5C;
    const LALT: u32 = 0xA4;
    const LCTRL: u32 = 0xA2;
    const LSHIFT: u32 = 0xA0;
    const RSHIFT: u32 = 0xA1;
    const RCTRL: u32 = 0xA3;
    const D: u32 = 0x44;
    const E: u32 = 0x45;
    const DELETE: u32 = 0x2E;

    /// Plays key events through the hook's rules, as the hook keeps them: what Windows got.
    fn windows_sees(grab: bool, events: &[(u32, bool)], keys: &mut [Key; 256]) -> Vec<(u32, bool)> {
        events.iter().copied().filter(|&(vk, down)| {
            let (swallow, next) = step(grab, vk, down, keys[vk as usize]);
            keys[vk as usize] = next;
            !swallow
        }).collect()
    }

    #[test]
    fn the_shells_shortcuts_stay_with_the_stream_while_grabbed() {
        // nightjar#71: Alt+Tab, Ctrl+Shift+Alt+Tab, Win, Win+D, Alt+Esc and Ctrl+Esc left the stream.
        let shortcuts: [&[u32]; 6] = [&[LALT, TAB], &[LCTRL, LSHIFT, LALT, TAB], &[LWIN], &[RWIN, D], &[LALT, ESC], &[LCTRL, ESC]];
        for chord in shortcuts {
            let mut keys: Vec<(u32, bool)> = chord.iter().map(|&vk| (vk, true)).collect();
            keys.extend(chord.iter().rev().map(|&vk| (vk, false)));
            let seen = windows_sees(true, &keys, &mut [Key::Up; 256]);
            assert!(seen.is_empty(), "{chord:x?}: Windows got {seen:x?}");
            let seen = windows_sees(false, &keys, &mut [Key::Up; 256]);
            assert_eq!(seen, keys, "{chord:x?}: not grabbed, Windows gets every key");
        }
    }

    #[test]
    fn a_release_goes_the_way_its_press_went() {
        let mut keys = [Key::Up; 256];
        // Alt held from the Alt+Tab that brought the window back: Windows saw it pressed.
        keys[LALT as usize] = Key::Passed;
        assert_eq!(windows_sees(true, &[(LALT, false)], &mut keys), [(LALT, false)]);
        // Win pressed during the grab, released after it ended: Windows never saw it.
        assert!(windows_sees(true, &[(LWIN, true)], &mut keys).is_empty());
        assert!(windows_sees(false, &[(LWIN, false)], &mut keys).is_empty());
        assert_eq!(keys[LWIN as usize], Key::Up);
    }

    #[test]
    fn a_key_held_from_before_the_grab_repeats_and_is_released_in_windows() {
        // Win+number switched to the stream with Win still down: its repeats while grabbed and its
        // release reach Windows, or Windows kept Win down and "e" opened Explorer afterwards.
        let mut keys = [Key::Up; 256];
        keys[LWIN as usize] = Key::Passed;
        let repeats = [(LWIN, true), (LWIN, true), (LWIN, true), (LWIN, false)];
        assert_eq!(windows_sees(true, &repeats, &mut keys), repeats);
        assert_eq!(windows_sees(false, &[(E, true), (E, false)], &mut keys), [(E, true), (E, false)]);
        // A key Windows got before the grab turned on, repeating into it, stays Windows' as well.
        let mut keys = [Key::Up; 256];
        assert_eq!(windows_sees(false, &[(LALT, true)], &mut keys), [(LALT, true)]);
        assert_eq!(windows_sees(true, &[(LALT, true), (LALT, false)], &mut keys), [(LALT, true), (LALT, false)]);
        // A kept key's repeats stay kept, after the grab ends too.
        let mut keys = [Key::Up; 256];
        assert!(windows_sees(true, &[(LWIN, true), (LWIN, true)], &mut keys).is_empty());
        assert!(windows_sees(false, &[(LWIN, true), (LWIN, false)], &mut keys).is_empty());
    }

    #[test]
    fn a_press_the_table_missed_is_windows_until_its_release() {
        // Win went down between the seed and the hook going in: the table says Up, Windows holds it.
        let state = settle(true, Key::Up, true, 30);
        assert_eq!(state, Key::Passed);
        assert_eq!(step(true, LWIN, true, state), (false, Key::Passed), "its repeat reaches Windows");
        assert_eq!(step(true, LWIN, false, Key::Passed), (false, Key::Up), "and so does its release");
        assert_eq!(settle(true, Key::Up, false, 30), Key::Up, "a new press is still kept");
    }

    #[test]
    fn a_kept_key_whose_release_was_lost_is_pressed_afresh_later() {
        // Ctrl+Alt+Del took Ctrl's release to the secure desktop; Nightjar is behind another window.
        assert_eq!(settle(false, Key::Kept, false, REPEAT_MS + 1), Key::Up);
        assert_eq!(step(false, LCTRL, true, Key::Up), (false, Key::Passed));
        assert_eq!(settle(false, Key::Kept, false, 30), Key::Kept, "auto-repeat after the grab ends stays kept");
        assert_eq!(settle(true, Key::Kept, false, REPEAT_MS + 1), Key::Kept, "a held key while grabbed stays kept");
    }

    #[test]
    fn a_hung_main_loop_lets_the_shells_keys_through() {
        assert!(beating(10_000, 10_000 - ALIVE_MS + 1));
        assert!(!beating(10_000, 10_000 - ALIVE_MS), "a frozen stream: Alt+Tab and Ctrl+Shift+Esc reach Windows");
        assert!(beating(5, 10), "a beat after the hook's clock read");
    }

    // Above the loop's longest wait with no picture due.
    const _: () = assert!(ALIVE_MS > 500);

    #[test]
    fn every_key_but_the_locks_stays_with_the_stream_and_ctrl_alt_del_is_windows() {
        // Kept keys reach the PC only through the hook: nightjar#71's raw keyboard never saw them.
        for vk in [D, DELETE, LSHIFT, 0x5A, 0x73] {
            assert!(kept_key(vk), "{vk:#x}");
            assert_eq!(step(true, vk, true, Key::Up), (true, Key::Kept), "{vk:#x}");
        }
        // Caps, Num and Scroll Lock toggle in Windows (SDL reads them from the window), and so
        // do a touch keyboard's characters; Ctrl+Alt+Del is Windows' however a hook answers.
        for vk in [0x14, 0x90, 0x91, 0xE7] {
            assert!(!kept_key(vk), "{vk:#x}");
            assert_eq!(step(true, vk, true, Key::Up), (false, Key::Passed), "{vk:#x}");
        }
    }

    #[test]
    fn kept_keys_reach_sdl_with_the_scancodes_the_window_would_give() {
        use sdl3_sys::everything::*;
        let cases = [
            (0x0F, false, SDL_SCANCODE_TAB),
            (0x01, false, SDL_SCANCODE_ESCAPE),
            (0x38, false, SDL_SCANCODE_LALT),
            (0x38, true, SDL_SCANCODE_RALT),
            (0x1D, false, SDL_SCANCODE_LCTRL),
            (0x1D, true, SDL_SCANCODE_RCTRL),
            (0x2A, false, SDL_SCANCODE_LSHIFT),
            (0x5B, true, SDL_SCANCODE_LGUI),
            (0x5C, true, SDL_SCANCODE_RGUI),
            (0x1E, false, SDL_SCANCODE_A),
            (0x2C, false, SDL_SCANCODE_Z),
            (0x02, false, SDL_SCANCODE_1),
            (0x0B, false, SDL_SCANCODE_0),
            (0x3B, false, SDL_SCANCODE_F1),
            (0x44, false, SDL_SCANCODE_F10),
            (0x57, false, SDL_SCANCODE_F11),
            (0x64, false, SDL_SCANCODE_F13),
            (0x6E, false, SDL_SCANCODE_F23),
            (0x76, false, SDL_SCANCODE_F24),
            (0x45, false, SDL_SCANCODE_PAUSE),
            (0x45, true, SDL_SCANCODE_NUMLOCKCLEAR),
            (0x1C, true, SDL_SCANCODE_KP_ENTER),
            (0x48, true, SDL_SCANCODE_UP),
            (0x48, false, SDL_SCANCODE_KP_8),
            (0x53, true, SDL_SCANCODE_DELETE),
            (0x37, true, SDL_SCANCODE_PRINTSCREEN),
            (0x56, false, SDL_SCANCODE_NONUSBACKSLASH),
        ];
        for (scan, extended, code) in cases {
            assert_eq!(sdl_scancode(scan, extended).map(|c| c.0), Some(code.0), "{scan:#x} extended {extended}");
        }
        // Media keys stay Windows'; AltGr's added Ctrl and a laptop's added Shifts are no keys
        // of their own (added), nor is a key without a scan code.
        for (scan, extended) in [(0x30, true), (0x22, true), (0x21D, false), (0x2A, true), (0, false)] {
            assert_eq!(sdl_scancode(scan, extended).map(|c| c.0), None, "{scan:#x} extended {extended}");
        }
    }

    /// What Windows got (virtual key, down) and what the stream got (SDL scancode, down, repeat).
    type Seen = (Vec<(u32, bool)>, Vec<(i32, bool, bool)>);

    /// The hook's state: every key's, AltGr's added Ctrl's and the key last pressed.
    struct Board {
        keys: [Key; 256],
        altgr_ctrl: Key,
        last_pressed: u32,
    }

    impl Board {
        fn new() -> Self {
            Self { keys: [Key::Up; 256], altgr_ctrl: Key::Up, last_pressed: u32::MAX }
        }

        /// Plays key events (virtual key, scan, extended, down) through [`route`] as the hook
        /// keeps its slots, each `idle_ms` after its slot's last; Windows holds none of them.
        fn play_after(&mut self, grab: bool, idle_ms: u64, events: &[(u32, u32, bool, bool)]) -> Seen {
            let (mut windows, mut stream) = (Vec::new(), Vec::new());
            for &(vk, scan, extended, down) in events {
                let altgr = scan == ALTGR_CTRL;
                let state = if altgr { self.altgr_ctrl } else { self.keys[vk as usize] };
                let routed = super::route(grab, vk, scan, extended, down, state, false, idle_ms, self.last_pressed == vk);
                if let Some(next) = routed.next {
                    if altgr {
                        self.altgr_ctrl = next;
                    } else {
                        self.keys[vk as usize] = next;
                        if down {
                            self.last_pressed = vk;
                        }
                    }
                }
                if let Some((code, repeat)) = routed.hand_over {
                    stream.push((code.0, down, repeat));
                }
                if !routed.swallow {
                    windows.push((vk, down));
                }
            }
            (windows, stream)
        }

        fn play(&mut self, grab: bool, events: &[(u32, u32, bool, bool)]) -> Seen {
            self.play_after(grab, 30, events)
        }
    }

    const RALT: u32 = 0xA5;
    const ALTGR: [(u32, u32, bool, bool); 4] = [(LCTRL, ALTGR_CTRL, false, true), (RALT, 0x38, true, true), (LCTRL, ALTGR_CTRL, false, false), (RALT, 0x38, true, false)];

    #[test]
    fn altgrs_added_ctrl_reaches_neither_windows_nor_the_stream() {
        use sdl3_sys::everything::*;
        // Polish AltGr+Shift+Z (Ż) was Nightjar's Ctrl+Alt+Shift+Z, which releases the capture.
        let mut board = Board::new();
        let (windows, stream) = board.play(true, &ALTGR);
        assert!(windows.is_empty(), "Windows got {windows:x?}");
        assert_eq!(stream, [(SDL_SCANCODE_RALT.0, true, false), (SDL_SCANCODE_RALT.0, false, false)]);
        assert_eq!((board.keys[LCTRL as usize], board.altgr_ctrl), (Key::Up, Key::Up));
        // The real left Ctrl held across an AltGr tap stays down for the PC.
        let mut board = Board::new();
        let (windows, stream) = board.play(true, &[&[(LCTRL, 0x1D, false, true)][..], &ALTGR].concat());
        assert!(windows.is_empty(), "Windows got {windows:x?}");
        assert_eq!(board.keys[LCTRL as usize], Key::Kept, "Ctrl is still held");
        assert_eq!(stream.iter().filter(|&&(code, _, _)| code == SDL_SCANCODE_LCTRL.0).count(), 1, "{stream:?}");
        // Not grabbed: Windows gets AltGr whole.
        let (windows, _) = Board::new().play(false, &ALTGR);
        assert_eq!(windows, ALTGR.map(|(vk, _, _, down)| (vk, down)));
    }

    #[test]
    fn altgrs_added_ctrl_is_released_where_it_was_pressed() {
        // AltGr pressed before the grab (or while the main loop stalled), released during it:
        // Windows got the added Ctrl's press, so it gets its release, or it holds Ctrl for ever.
        let mut board = Board::new();
        assert_eq!(board.play(false, &ALTGR[..2]).0, [(LCTRL, true), (RALT, true)]);
        assert_eq!(board.play(true, &ALTGR[2..]).0, [(LCTRL, false), (RALT, false)]);
        assert_eq!(board.altgr_ctrl, Key::Up);
        // And the other way round: pressed while grabbed, its release stays kept.
        let mut board = Board::new();
        assert!(board.play(true, &ALTGR[..2]).0.is_empty());
        assert!(board.play(false, &ALTGR[2..]).0.is_empty());
    }

    #[test]
    fn a_laptops_added_shifts_leave_the_real_shift_down() {
        use sdl3_sys::everything::*;
        // Shift+Home on an i8042 keyboard with Num Lock on: the keyboard lifts Shift (extended
        // 0x2A) before Home and presses it again after.
        let (shift, added_shift, home) = ((LSHIFT, 0x2A, false), (LSHIFT, 0x2A, true), (0x24, 0x47, true));
        let press = |(vk, scan, extended): (u32, u32, bool), down| (vk, scan, extended, down);
        let mut board = Board::new();
        let events = [press(shift, true), press(added_shift, false), press(home, true), press(home, false), press(added_shift, true), press(shift, false)];
        let (windows, stream) = board.play(true, &events);
        assert!(windows.is_empty(), "Windows got {windows:x?}");
        assert_eq!(stream, [
            (SDL_SCANCODE_LSHIFT.0, true, false), (SDL_SCANCODE_HOME.0, true, false), (SDL_SCANCODE_HOME.0, false, false), (SDL_SCANCODE_LSHIFT.0, false, false),
        ]);
        assert_eq!(board.keys[LSHIFT as usize], Key::Up);
        // Home is handed over with Shift held.
        let mut board = Board::new();
        board.play(true, &events[..3]);
        assert_eq!(modifiers(|vk| board.keys[vk as usize] == Key::Kept, SDL_KMOD_NONE).0, SDL_KMOD_LSHIFT.0);
    }

    #[test]
    fn alt_print_screen_stays_with_the_stream() {
        use sdl3_sys::everything::*;
        // Alt+Print Screen comes as scan 0x54 (SysRq); it reached Windows' Snipping Tool.
        let (windows, stream) = Board::new().play(true, &[(LALT, 0x38, false, true), (0x2C, 0x54, false, true), (0x2C, 0x54, false, false), (LALT, 0x38, false, false)]);
        assert!(windows.is_empty(), "Windows got {windows:x?}");
        assert_eq!(stream[1], (SDL_SCANCODE_PRINTSCREEN.0, true, false));
    }

    #[test]
    fn a_kept_key_pressed_after_the_secure_desktop_is_a_new_press() {
        // Win+L took Win's and L's releases; the stream got the grab back with both still kept.
        use sdl3_sys::everything::*;
        const L: u32 = 0x4C;
        let mut board = Board::new();
        board.play(true, &[(LWIN, 0x5B, true, true), (L, 0x26, false, true)]);
        let (_, stream) = board.play_after(true, 5000, &[(LWIN, 0x5B, true, true)]);
        assert_eq!(stream, [(SDL_SCANCODE_LGUI.0, true, false)], "not the last key pressed");
        let (_, stream) = board.play_after(true, 300, &[(LCTRL, 0x1D, false, true), (L, 0x26, false, true)]);
        assert_eq!(stream[1], (SDL_SCANCODE_L.0, true, false), "the last key pressed, but another came between");
        let (_, stream) = board.play_after(true, REPEAT_MS + 1, &[(L, 0x26, false, true)]);
        assert_eq!(stream, [(SDL_SCANCODE_L.0, true, false)], "the last key pressed, quiet too long to repeat");
        assert!(!repeats(Key::Up, true, 33, true), "a new press");
        assert!(!repeats(Key::Kept, false, 33, true), "a release");
    }

    #[test]
    fn a_slow_first_auto_repeat_is_still_a_repeat() {
        // Windows' longest delay (a second), 20 % late on a PS/2 keyboard and a clock tick on top:
        // a second press would type the character twice.
        use sdl3_sys::everything::*;
        let mut board = Board::new();
        board.play(true, &[(D, 0x20, false, true)]);
        let (_, stream) = board.play_after(true, 1216, &[(D, 0x20, false, true), (D, 0x20, false, true)]);
        assert_eq!(stream, [(SDL_SCANCODE_D.0, true, true), (SDL_SCANCODE_D.0, true, true)]);
        // A kept key keeps repeating after the grab ends, too.
        assert_eq!(settle(false, Key::Kept, false, 1216), Key::Kept);
    }

    #[test]
    fn a_handed_over_key_waits_for_no_echo() {
        // The hook leaves the keycode unknown: a kept key types no text, so the next character
        // from a touch keyboard must not be taken for its echo.
        assert!(!super::super::text_field::types(sdl3_sys::everything::SDLK_UNKNOWN, sdl3_sys::everything::SDL_KMOD_NONE));
    }

    #[test]
    fn sdls_raw_keyboard_stays_off() {
        // nightjar#71: with SDL's raw keyboard on, the hook never saw a press and Alt+Tab left.
        fn scan(dir: &std::path::Path, hits: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    scan(&path, hits);
                } else if path.extension().is_some_and(|e| e == "rs") && std::fs::read_to_string(&path).unwrap().contains(concat!("WINDOWS_RAW", "_KEYBOARD")) {
                    hits.push(path);
                }
            }
        }
        let mut hits = Vec::new();
        scan(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut hits);
        assert!(hits.is_empty(), "SDL's raw keyboard hint in {hits:?}");
    }

    #[test]
    fn a_kept_key_carries_the_modifiers_held_in_the_hook_and_in_windows() {
        use sdl3_sys::everything::*;
        // Ctrl+Alt+Shift+Tab, Nightjar's stream controls: SDL never saw the three kept modifiers.
        let kept = |vk: u32| matches!(vk, LCTRL | LALT | LSHIFT);
        let mods = modifiers(kept, SDL_KMOD_NONE);
        assert_eq!(mods.0, SDL_KMOD_LCTRL.0 | SDL_KMOD_LALT.0 | SDL_KMOD_LSHIFT.0);
        assert_eq!(crate::shortcuts::key_action(crate::shortcuts::Scope::Stream, SDL_SCANCODE_TAB, mods), Some(crate::shortcuts::Action::StreamControls));
        // Caps Lock on, Win held from before the grab: SDL's own state.
        let mods = modifiers(|vk| vk == LALT, SDL_Keymod(SDL_KMOD_CAPS.0 | SDL_KMOD_LGUI.0));
        assert_eq!(mods.0, SDL_KMOD_LALT.0 | SDL_KMOD_CAPS.0 | SDL_KMOD_LGUI.0);
        assert_eq!(modifiers(|_| false, SDL_KMOD_NONE).0, 0);
    }

    #[test]
    fn the_chord_is_guarded_only_while_its_hotkey_can_act() {
        use crate::shortcuts::Action;
        let menu = Some(Action::StreamControls);
        // focused, grab, menu visible, mic only.
        assert!(tab_guard(menu, true, false, false, false), "Send system keys off or input released");
        assert!(tab_guard(menu, true, false, true, false), "the menu's second chord closes it");
        assert!(tab_guard(Some(Action::StreamShortcuts), true, false, true, false), "the sheet opens from the menu");
        assert!(!tab_guard(menu, false, false, false, false), "another window in front keeps its chord");
        assert!(!tab_guard(menu, true, true, false, false), "the grab's hook has it");
        assert!(!tab_guard(None, true, false, false, false), "no hotkey on the chord: no hook");
        // Rebound: kept from Windows only where it does something, never swallowed for nothing.
        for action in [Action::Disconnect, Action::Capture, Action::Overlay, Action::Fullscreen] {
            assert!(!tab_guard(Some(action), true, false, true, false), "{action:?}: the open menu passes it to its widgets");
            assert!(tab_guard(Some(action), true, false, false, false), "{action:?}: the menu closed, it acts");
        }
        assert!(!tab_guard(menu, true, false, false, true), "Microphone only has no menu");
        assert!(!tab_guard(Some(Action::Disconnect), true, false, false, true), "Microphone only leaves the chord to Windows");
    }

    #[test]
    fn the_guarded_chord_follows_the_binding() {
        use crate::shortcuts::Action;
        crate::shortcuts::set_hotkeys(Default::default());
        assert_eq!(tab_chord_action(), Some(Action::StreamControls));
        // Another action on it is guarded for that action; moved off, there is nothing to guard.
        let mut moved = crate::shortcuts::hotkey_bindings();
        moved.insert("stream_menu".to_string(), "CTRL+ALT+F7".to_string());
        crate::shortcuts::set_hotkeys(moved.clone());
        assert_eq!(tab_chord_action(), None);
        moved.insert("quit".to_string(), "CTRL+ALT+SHIFT+TAB".to_string());
        crate::shortcuts::set_hotkeys(moved);
        assert_eq!(tab_chord_action(), Some(Action::Disconnect));
        crate::shortcuts::set_hotkeys(Default::default());
    }

    #[test]
    fn only_tab_with_exactly_the_chords_modifiers_is_guarded() {
        assert!(tab_chord_held(|vk| matches!(vk, LCTRL | LALT | LSHIFT), false));
        assert!(tab_chord_held(|vk| matches!(vk, RCTRL | RALT | RSHIFT), false), "either side");
        // Windows' Alt+Tab, Ctrl+Tab and the rest are not the chord, nor is it with Windows held.
        for held in [&[][..], &[LALT], &[LALT, LSHIFT], &[LCTRL, LALT], &[LCTRL], &[LCTRL, LSHIFT], &[LSHIFT]] {
            assert!(!tab_chord_held(|vk| held.contains(&vk), false), "{held:x?}");
        }
        for win in [LWIN, RWIN] {
            assert!(!tab_chord_held(|vk| matches!(vk, LCTRL | LALT | LSHIFT) || vk == win, false), "{win:x}");
        }
    }

    #[test]
    fn altgrs_added_ctrl_is_no_ctrl_of_the_chord() {
        // AltGr+Shift+Tab: Windows holds the left Ctrl AltGr adds and the right Alt.
        let altgr_shift = |vk| matches!(vk, LCTRL | RALT | LSHIFT);
        assert!(!tab_chord_held(altgr_shift, true));
        assert!(tab_chord_held(altgr_shift, false), "a real left Ctrl with the right Alt");
        assert!(tab_chord_held(|vk| matches!(vk, RCTRL | RALT | LSHIFT), true), "a real right Ctrl with AltGr, as Windows reads it too");
    }

    #[test]
    fn the_modifiers_the_grab_kept_still_make_the_chord_that_closes_the_menu() {
        // Ctrl+Alt+Shift held, Tab opened the menu and ended the grab: Windows never saw them.
        let mut board = Board::new();
        board.play(true, &[(LCTRL, 0x1D, false, true), (LALT, 0x38, false, true), (LSHIFT, 0x2A, false, true)]);
        assert!(tab_chord_held(|vk| tab_chord_modifier(false, board.keys[vk as usize], 300), false));
        // Shift let go and pressed again in the open menu: that one is Windows'.
        board.play(false, &[(LSHIFT, 0x2A, false, false), (LSHIFT, 0x2A, false, true)]);
        assert_eq!(board.keys[LSHIFT as usize], Key::Passed);
        assert!(!tab_chord_held(|vk| tab_chord_modifier(false, board.keys[vk as usize], 300), false));
        assert!(tab_chord_held(|vk| tab_chord_modifier(vk == LSHIFT, board.keys[vk as usize], 300), false));
    }

    #[test]
    fn a_stale_kept_modifier_does_not_make_shift_tab_the_chord() {
        // Ctrl+Alt+Del while grabbed: the secure desktop took the releases of Ctrl and Alt, so the
        // table holds them kept, while Windows knows only the Shift pressed in the open menu.
        let mut board = Board::new();
        board.play(true, &[(LCTRL, 0x1D, false, true), (LALT, 0x38, false, true)]);
        assert_eq!((board.keys[LCTRL as usize], board.keys[LALT as usize]), (Key::Kept, Key::Kept));
        let quiet = REPEAT_MS + 1;
        assert!(!tab_chord_held(|vk| tab_chord_modifier(vk == LSHIFT, board.keys[vk as usize], quiet), false), "Shift+Tab in the menu stays the menu's Tab");
        assert!(!tab_chord_modifier(false, Key::Up, 0), "never pressed");
        assert!(!tab_chord_modifier(false, Key::Passed, 0), "Windows let it go");
    }

    #[test]
    fn hotkey_policy_leaves_this_one_tab_chord_for_windows_to_take() {
        // tab_chord_mods is the only chord to guard: Windows takes no other a hotkey may have.
        use nightjar_policy::shortcuts::{hotkey_keys, HotkeyError};
        let keys = |chord: &[&str]| hotkey_keys(&chord.iter().map(|key| key.to_string()).collect::<Vec<_>>());
        assert!(keys(&["CTRL", "ALT", "SHIFT", "TAB"]).is_ok());
        for reserved in [&["ALT", "TAB"][..], &["ALT", "SHIFT", "TAB"], &["CTRL", "ALT", "TAB"]] {
            assert_eq!(keys(reserved), Err(HotkeyError::Reserved), "{reserved:?}");
        }
        // These reach the window as they are, so nothing is kept for them: not the guarded chord.
        for fine in [&["CTRL", "TAB"][..], &["CTRL", "SHIFT", "TAB"]] {
            assert!(keys(fine).is_ok(), "{fine:?}");
        }
        assert!(!tab_chord_held(|vk| vk == LCTRL, false), "Ctrl+Tab");
        assert!(!tab_chord_held(|vk| matches!(vk, LSHIFT | LCTRL), false), "Ctrl+Shift+Tab");
    }

    #[test]
    fn without_the_grab_every_key_but_tab_goes_straight_on() {
        assert!(passes_by(false, D, false, Key::Up), "typing wakes nothing up");
        assert!(passes_by(false, LALT, false, Key::Up), "a modifier of the chord too: it stays Windows'");
        assert!(!passes_by(true, D, false, Key::Up), "the grab keeps it");
        assert!(!passes_by(false, TAB, false, Key::Up), "it may be the chord's");
        assert!(!passes_by(false, D, false, Key::Kept), "a kept key's release stays kept");
        assert!(!passes_by(false, D, false, Key::Passed), "a press from the grab's time is released in the table");
        assert!(!passes_by(false, LCTRL, true, Key::Up), "AltGr's added Ctrl remembers its way");
        // Passing by leaves the table Up; a press Windows holds is still Windows' once grabbed.
        assert_eq!(settle(true, Key::Up, true, 30), Key::Passed);
    }

    #[test]
    fn the_guard_reinstalls_the_hook_when_it_turns_on_and_removes_it_only_without_a_grab() {
        assert_eq!(guard_step(true, false), GuardStep::Reinstall, "focus back with Send system keys off: Windows may have removed it");
        assert_eq!(guard_step(true, true), GuardStep::Reinstall);
        assert_eq!(guard_step(false, false), GuardStep::Remove, "no cost per key in other apps");
        assert_eq!(guard_step(false, true), GuardStep::Keep, "the grab's hook stays, for the kept keys' releases");
    }

    #[test]
    fn the_hook_stays_until_a_kept_tab_is_released() {
        assert!(!unhooks_now(Key::Kept), "the chord still held: the next repeat would open the task switcher");
        assert!(unhooks_now(Key::Up));
        assert!(unhooks_now(Key::Passed), "Windows has that Tab already");
    }

    #[test]
    fn a_tab_kept_without_the_grab_goes_one_way_and_leaves_the_modifiers_windows() {
        use sdl3_sys::everything::*;
        const CTRL: (u32, u32, bool) = (LCTRL, 0x1D, false);
        const ALT: (u32, u32, bool) = (LALT, 0x38, false);
        const SHIFT: (u32, u32, bool) = (LSHIFT, 0x2A, false);
        let key = |(vk, scan, extended): (u32, u32, bool), down| (vk, scan, extended, down);
        let tab = |down| (TAB, 0x0F, false, down);
        let mut board = Board::new();
        // The menu is open: Windows gets the modifiers, the stream the Tab and its repeat, and the
        // Tab's release stays with the stream though the hook no longer holds the chord.
        let (windows, _) = board.play(false, &[key(CTRL, true), key(ALT, true), key(SHIFT, true)]);
        assert_eq!(windows, [(LCTRL, true), (LALT, true), (LSHIFT, true)]);
        let (windows, stream) = board.play(true, &[tab(true), tab(true)]);
        assert!(windows.is_empty(), "Windows got {windows:x?}");
        assert_eq!(stream, [(SDL_SCANCODE_TAB.0, true, false), (SDL_SCANCODE_TAB.0, true, true)]);
        let (windows, stream) = board.play(false, &[tab(false), key(SHIFT, false), key(ALT, false), key(CTRL, false)]);
        assert_eq!(windows, [(LSHIFT, false), (LALT, false), (LCTRL, false)], "Windows gets back what it held");
        assert_eq!(stream, [(SDL_SCANCODE_TAB.0, false, false)]);
    }

    /// The hook against Windows itself: an SDL window in front, Alt+Tab, Ctrl+Alt+Shift+Tab,
    /// Alt+Esc and a typed "d" injected with SendInput, by scan code as a keyboard sends them.
    /// Injected keys pass the low-level hook like typed ones (flagged LLKHF_INJECTED), so this
    /// covers the hook, Windows' shortcuts and SDL's queue, not the keyboard driver. It moves the
    /// foreground and types: run it alone, on an idle desktop.
    #[cfg(windows)]
    #[test]
    #[ignore = "takes the foreground and injects keys"]
    fn the_hook_keeps_alt_tab_and_hands_every_key_to_the_stream() {
        use sdl3_sys::everything::*;
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::Input::KeyboardAndMouse::*;
        use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, SetForegroundWindow};
        /// The key events SDL got: scancode, down, modifiers.
        fn pump(ms: u64, got: &mut Vec<(i32, bool, u16)>) {
            let end = std::time::Instant::now() + std::time::Duration::from_millis(ms);
            while std::time::Instant::now() < end {
                alive();
                // SAFETY: SDL's event queue on the thread that set up video.
                unsafe {
                    let mut event: SDL_Event = std::mem::zeroed();
                    while SDL_PollEvent(&mut event) {
                        if matches!(SDL_EventType(event.r#type), SDL_EVENT_KEY_DOWN | SDL_EVENT_KEY_UP) && !event.key.repeat {
                            // Without the lock keys' state, which is this PC's.
                            let locks = SDL_KMOD_NUM.0 | SDL_KMOD_CAPS.0 | SDL_KMOD_SCROLL.0;
                            got.push((event.key.scancode.0, event.key.down, event.key.r#mod.0 & !locks));
                        }
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
        fn key(vk: u32, down: bool, got: &mut Vec<(i32, bool, u16)>) {
            // SAFETY: any virtual key.
            let scan = unsafe { MapVirtualKeyW(vk, MAPVK_VK_TO_VSC_EX) };
            let mut flags = KEYEVENTF_SCANCODE;
            if !down {
                flags |= KEYEVENTF_KEYUP;
            }
            if scan >> 8 == 0xE0 {
                flags |= KEYEVENTF_EXTENDEDKEY;
            }
            let ki = KEYBDINPUT { wVk: VIRTUAL_KEY(vk as u16), wScan: (scan & 0xFF) as u16, dwFlags: flags, ..Default::default() };
            let input = INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki } };
            // SAFETY: one keyboard input.
            assert_eq!(unsafe { SendInput(&[input], size_of::<INPUT>() as i32) }, 1);
            pump(40, got);
        }
        let mut got = Vec::new();
        // SAFETY: SDL video on this thread, one window of this test's.
        let (window, hwnd, id) = unsafe {
            assert!(SDL_Init(SDL_INIT_VIDEO));
            let window = SDL_CreateWindow(c"nightjar system keys test".as_ptr(), 400, 300, SDL_WindowFlags(0));
            assert!(!window.is_null());
            let hwnd = SDL_GetPointerProperty(SDL_GetWindowProperties(window), SDL_PROP_WINDOW_WIN32_HWND_POINTER, std::ptr::null_mut());
            (window, HWND(hwnd), SDL_GetWindowID(window))
        };
        let in_front = || unsafe { GetForegroundWindow() } == hwnd;
        // An Alt tap lets this process take the foreground.
        key(LALT, true, &mut got);
        key(LALT, false, &mut got);
        // SAFETY: this test's window.
        let _ = unsafe { SetForegroundWindow(hwnd) };
        pump(500, &mut got);
        assert!(in_front(), "the test window did not come to the front");
        let mut hook = ShellKeyHook::start().unwrap();
        hook.set(true, hwnd.0 as isize, id);
        pump(300, &mut got);
        got.clear();
        let mut leaked = Vec::new();
        let mut stream = Vec::new();
        for chord in [&[LALT, TAB][..], &[LCTRL, LALT, LSHIFT, TAB], &[LALT, ESC], &[D]] {
            for &vk in chord {
                key(vk, true, &mut got);
            }
            for &vk in chord.iter().rev() {
                key(vk, false, &mut got);
            }
            pump(700, &mut got);
            stream.push(std::mem::take(&mut got));
            if !in_front() {
                leaked.push(chord);
                // SAFETY: this test's window, which had the last input.
                let _ = unsafe { SetForegroundWindow(hwnd) };
                pump(500, &mut got);
                got.clear();
            }
        }
        // Not grabbed (the stream menu open, input released): Windows' task switcher stays shut
        // and only the Tab of the bound chord comes through the hook.
        hook.set(false, hwnd.0 as isize, id);
        hook.guard(true, hwnd.0 as isize, id);
        pump(100, &mut got);
        got.clear();
        for &vk in &[LCTRL, LALT, LSHIFT, TAB] {
            key(vk, true, &mut got);
        }
        for &vk in &[TAB, LSHIFT, LALT, LCTRL] {
            key(vk, false, &mut got);
        }
        pump(700, &mut got);
        let chord_stayed = in_front();
        let tabs_of = |got: &[(i32, bool, u16)]| got.iter().copied().filter(|&(code, _, _)| code == SDL_SCANCODE_TAB.0).collect::<Vec<_>>();
        let tabs = tabs_of(&got);
        // The menu opened with the modifiers still down: they stay kept, and the next Tab is the
        // chord's again.
        hook.guard(false, hwnd.0 as isize, id);
        hook.set(true, hwnd.0 as isize, id);
        pump(100, &mut got);
        got.clear();
        for &vk in &[LCTRL, LALT, LSHIFT, TAB] {
            key(vk, true, &mut got);
        }
        key(TAB, false, &mut got);
        hook.set(false, hwnd.0 as isize, id);
        hook.guard(true, hwnd.0 as isize, id);
        pump(100, &mut got);
        key(TAB, true, &mut got);
        key(TAB, false, &mut got);
        for &vk in &[LSHIFT, LALT, LCTRL] {
            key(vk, false, &mut got);
        }
        pump(700, &mut got);
        let held_stayed = in_front();
        let held_tabs = tabs_of(&got);
        drop(hook);
        // The hotkey took the focus with the chord still held: the guard is off, the Tab repeats.
        let mut hook = ShellKeyHook::start().unwrap();
        hook.guard(true, hwnd.0 as isize, id);
        pump(100, &mut got);
        for &vk in &[LCTRL, LALT, LSHIFT, TAB] {
            key(vk, true, &mut got);
        }
        hook.guard(false, hwnd.0 as isize, id);
        pump(100, &mut got);
        key(TAB, true, &mut got);
        pump(700, &mut got);
        let repeat_stayed = in_front();
        key(TAB, false, &mut got);
        for &vk in &[LSHIFT, LALT, LCTRL] {
            key(vk, false, &mut got);
        }
        pump(100, &mut got);
        drop(hook);
        // SAFETY: this test's window and SDL.
        unsafe {
            SDL_DestroyWindow(window);
            SDL_Quit();
        }
        assert!(leaked.is_empty(), "{leaked:x?} took the foreground from the stream");
        let (tab, esc, d) = (SDL_SCANCODE_TAB.0, SDL_SCANCODE_ESCAPE.0, SDL_SCANCODE_D.0);
        let (lctrl, lalt, lshift) = (SDL_SCANCODE_LCTRL.0, SDL_SCANCODE_LALT.0, SDL_SCANCODE_LSHIFT.0);
        let (ctrl, alt, shift) = (SDL_KMOD_LCTRL.0, SDL_KMOD_LALT.0, SDL_KMOD_LSHIFT.0);
        // Each key once, in order, with the modifiers held: what the PC and Nightjar's shortcuts get.
        assert_eq!(stream[0], [(lalt, true, alt), (tab, true, alt), (tab, false, alt), (lalt, false, 0)]);
        let all = ctrl | alt | shift;
        assert_eq!(stream[1], [
            (lctrl, true, ctrl), (lalt, true, ctrl | alt), (lshift, true, all), (tab, true, all),
            (tab, false, all), (lshift, false, ctrl | alt), (lalt, false, ctrl), (lctrl, false, 0),
        ]);
        assert_eq!(stream[2], [(lalt, true, alt), (esc, true, alt), (esc, false, alt), (lalt, false, 0)]);
        assert_eq!(stream[3], [(d, true, 0), (d, false, 0)]);
        assert!(chord_stayed, "Ctrl+Alt+Shift+Tab left the window without the grab");
        assert_eq!(tabs, [(tab, true, all), (tab, false, all)]);
        assert!(held_stayed, "the second Tab with the chord held left the window");
        assert_eq!(held_tabs, [(tab, true, all), (tab, false, all), (tab, true, all), (tab, false, all)]);
        assert!(repeat_stayed, "a Tab's repeat reached Windows after the guard went off");
    }

    /// What the guard's hook costs a key that is not the chord's, against no hook, in rounds: the
    /// time SendInput takes (Windows runs the hook inside it) and from its call to SDL's queue.
    /// `cargo test --release -p nightjar-desktop --bin nightjar -- --ignored --nocapture the_hook_cost`
    #[cfg(windows)]
    #[test]
    #[ignore = "takes the foreground and injects keys"]
    fn the_hook_cost_of_a_key_without_the_grab() {
        use sdl3_sys::everything::*;
        use std::time::{Duration, Instant};
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::Input::KeyboardAndMouse::*;
        use windows::Win32::UI::WindowsAndMessaging::SetForegroundWindow;
        fn send(down: bool) {
            // F13: nothing acts on it.
            let flags = if down { KEYEVENTF_SCANCODE } else { KEYEVENTF_SCANCODE | KEYEVENTF_KEYUP };
            let ki = KEYBDINPUT { wScan: 0x64, dwFlags: flags, ..Default::default() };
            let input = INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki } };
            // SAFETY: one keyboard input.
            assert_eq!(unsafe { SendInput(&[input], size_of::<INPUT>() as i32) }, 1);
        }
        fn wait_for(kind: SDL_EventType) {
            let end = Instant::now() + Duration::from_secs(2);
            // SAFETY: SDL's event queue on the thread that set up video.
            unsafe {
                let mut event: SDL_Event = std::mem::zeroed();
                while Instant::now() < end {
                    while SDL_PollEvent(&mut event) {
                        if SDL_EventType(event.r#type) == kind && event.key.scancode == SDL_SCANCODE_F13 {
                            return;
                        }
                    }
                }
            }
            panic!("the key did not reach the window");
        }
        fn round(keys: usize, send_us: &mut Vec<f64>, queue_us: &mut Vec<f64>) {
            for _ in 0..keys {
                alive();
                let start = Instant::now();
                send(true);
                let sent = start.elapsed();
                wait_for(SDL_EVENT_KEY_DOWN);
                let queued = start.elapsed();
                send(false);
                wait_for(SDL_EVENT_KEY_UP);
                send_us.push(sent.as_secs_f64() * 1e6);
                queue_us.push(queued.as_secs_f64() * 1e6);
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        fn report(name: &str, label: &str, values: &mut [f64]) {
            values.sort_by(f64::total_cmp);
            let at = |q: f64| values[((values.len() - 1) as f64 * q) as usize];
            println!("{name} {label}: median {:.1} us, p95 {:.1} us, p99 {:.1} us, n={}", at(0.5), at(0.95), at(0.99), values.len());
        }
        // SAFETY: SDL video on this thread, one window of this test's.
        let (window, hwnd, id) = unsafe {
            assert!(SDL_Init(SDL_INIT_VIDEO));
            let window = SDL_CreateWindow(c"nightjar hook cost".as_ptr(), 400, 300, SDL_WindowFlags(0));
            assert!(!window.is_null());
            let hwnd = SDL_GetPointerProperty(SDL_GetWindowProperties(window), SDL_PROP_WINDOW_WIN32_HWND_POINTER, std::ptr::null_mut());
            (window, HWND(hwnd), SDL_GetWindowID(window))
        };
        // SAFETY: this test's window; the keys it sends give it the right to come forward.
        let _ = unsafe { SetForegroundWindow(hwnd) };
        std::thread::sleep(Duration::from_millis(300));
        let mut without = (Vec::new(), Vec::new());
        let mut with = (Vec::new(), Vec::new());
        for _ in 0..4 {
            round(1000, &mut without.0, &mut without.1);
            let mut hook = ShellKeyHook::start().unwrap();
            hook.guard(true, hwnd.0 as isize, id);
            std::thread::sleep(Duration::from_millis(100));
            round(1000, &mut with.0, &mut with.1);
            drop(hook);
        }
        // SAFETY: this test's window and SDL.
        unsafe {
            SDL_DestroyWindow(window);
            SDL_Quit();
        }
        report("no hook", "SendInput", &mut without.0);
        report("no hook", "to SDL's queue", &mut without.1);
        report("guard hook", "SendInput", &mut with.0);
        report("guard hook", "to SDL's queue", &mut with.1);
    }

    #[test]
    fn the_keyboard_is_grabbed_while_input_is_captured_with_system_keys_on() {
        assert!(keyboard_grab(true, true));
        assert!(!keyboard_grab(true, false), "Send system keys off");
        assert!(!keyboard_grab(false, true), "released, the stream menu open or the window out of focus");
    }

    // Nightjar #87.
    #[test]
    fn system_keys_can_wait_for_full_screen() {
        assert!(sent(true, false, false) && sent(true, false, true), "as before: in a window too");
        assert!(sent(true, true, true) && !sent(true, true, false), "only in full screen");
        assert!(!sent(false, true, true) && !sent(false, false, true), "Send system keys off");
    }
}
