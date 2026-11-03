//! "Send system keys" on Windows: a low-level keyboard hook on a thread of its own keeps the
//! shell's shortcuts (Alt+Tab, Alt+Shift+Tab, the Windows key and Win+key, Alt+Esc, Ctrl+Esc)
//! from Windows while the stream has the keyboard; raw input still hands every key to the PC.
//!
//! SDL's keyboard grab installs the same kind of hook, but on the main thread, which answers it
//! only between its waits in the present. Windows gives a hook that answers late a short time,
//! passes the key on when it runs out and, from Windows 7 on, silently removes the hook: Alt+Tab
//! then left the stream (nightjar#71) while Moonlight-qt kept it. This thread does nothing but
//! answer the hook, so no key waits for a refresh either. Ctrl+Alt+Del is never a hook's.
//!
//! A main loop that stopped beating ([`alive`]) for [`ALIVE_MS`] gets no keys kept: a frozen
//! fullscreen stream lets Alt+Tab, the Windows key and Ctrl+Shift+Esc reach Windows, as SDL's
//! hook on the hung thread did.

#[cfg(windows)]
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU64, AtomicU8, Ordering};

/// The keys SDL's grab takes from Windows, by virtual key: the Windows keys, Alt, Ctrl, Print
/// Screen, and Tab and Esc (the second key of Alt+Tab, Alt+Esc, Ctrl+Esc and Ctrl+Shift+Esc).
/// Windows sees no shortcut without its first key; Nightjar's own (Ctrl+Alt+Shift+letter) come
/// through raw input like every other key.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn shell_key(vk: u32) -> bool {
    matches!(vk, 0x5B | 0x5C | 0xA4 | 0xA5 | 0xA2 | 0xA3 | 0x2C | 0x09 | 0x1B)
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

/// One key event through the hook: whether it is kept from Windows, and the key's state after.
/// `grab`: the keyboard is the stream's, its window is in front and the main loop beats. A key
/// goes one way from its press to its release, auto-repeats included: one Windows already holds
/// (Win of the Win+number that brought the stream to the front) stays Windows' until released,
/// or Windows would keep it down; one kept stays kept after the grab ends, so Windows never sees
/// half of it.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn step(grab: bool, vk: u32, down: bool, key: Key) -> (bool, Key) {
    match (down, key) {
        (true, Key::Up) if grab && shell_key(vk) => (true, Key::Kept),
        (true, Key::Kept) => (true, Key::Kept),
        (true, _) => (false, Key::Passed),
        (false, Key::Kept) => (true, Key::Up),
        (false, _) => (false, Key::Up),
    }
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

/// Whether the keyboard is grabbed: input is captured (keys go to the PC, the stream menu is
/// closed, the window is the PC's input) and "Send system keys" is on. Windowed or fullscreen,
/// relative or desktop pointer alike; out of focus, the hook and SDL let the keys be.
pub fn keyboard_grab(captured: bool, system_keys: bool) -> bool {
    captured && system_keys
}

#[cfg(windows)]
static GRAB: AtomicBool = AtomicBool::new(false);
/// The stream window's HWND: the hook takes keys only while it is in front.
#[cfg(windows)]
static WINDOW: AtomicIsize = AtomicIsize::new(0);
/// The main loop's last beat, in GetTickCount64 milliseconds.
#[cfg(windows)]
static BEAT: AtomicU64 = AtomicU64::new(0);
/// [`Key`] by virtual key; only the hook's thread writes it.
#[cfg(windows)]
static KEYS: [AtomicU8; 256] = [const { AtomicU8::new(Key::Up as u8) }; 256];
/// The thread message that reinstalls the hook.
#[cfg(windows)]
const REHOOK: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP;

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
    for (vk, key) in KEYS.iter().enumerate() {
        if !fresh && Key::from_u8(key.load(Ordering::Relaxed)) == Key::Kept {
            continue;
        }
        // SAFETY: any virtual key.
        let down = unsafe { GetAsyncKeyState(vk as i32) } < 0;
        key.store(if down { Key::Passed } else { Key::Up } as u8, Ordering::Relaxed);
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
            // SAFETY: no arguments.
            let grab = GRAB.load(Ordering::Acquire) && unsafe { GetForegroundWindow() }.0 as isize == WINDOW.load(Ordering::Acquire)
                && beating(now_ms(), BEAT.load(Ordering::Acquire));
            let (swallow, next) = step(grab, info.vkCode, down, Key::from_u8(key.load(Ordering::Relaxed)));
            key.store(next as u8, Ordering::Relaxed);
            if swallow {
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

/// The hook's thread, from the first grab to the end of the stream.
#[cfg(windows)]
pub struct ShellKeyHook {
    thread_id: u32,
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
                if message.message != REHOOK {
                    continue;
                }
                // Windows may have removed the hook without a word: a new one each time the grab
                // turns on.
                if let Some(old) = hook.take() {
                    // SAFETY: the hook this thread installed.
                    let _ = unsafe { UnhookWindowsHookEx(old) };
                }
                seed(false);
                match install() {
                    Ok(new) => hook = Some(new),
                    Err(error) => eprintln!("nightjar: system keys reach Windows until the next grab: keyboard hook: {error}"),
                }
            }
            if let Some(hook) = hook {
                // SAFETY: the hook this thread installed.
                let _ = unsafe { UnhookWindowsHookEx(hook) };
            }
        }).map_err(|error| error.to_string())?;
        match rx.recv() {
            Ok(Ok(thread_id)) => Ok(Self { thread_id, thread: Some(thread) }),
            Ok(Err(error)) => Err(error),
            Err(_) => Err("the hook thread ended".into()),
        }
    }

    /// Whether the hook takes the shell's keys, and from which window (`hwnd`). Turning the grab
    /// on reinstalls the hook.
    pub fn set(&self, grab: bool, hwnd: isize) {
        use windows::Win32::Foundation::{LPARAM, WPARAM};
        use windows::Win32::UI::WindowsAndMessaging::PostThreadMessageW;
        alive();
        WINDOW.store(hwnd, Ordering::Release);
        if grab && !GRAB.swap(grab, Ordering::AcqRel) {
            // SAFETY: the hook thread's id.
            if let Err(error) = unsafe { PostThreadMessageW(self.thread_id, REHOOK, WPARAM(0), LPARAM(0)) } {
                eprintln!("nightjar: the keyboard hook stays as it was: {error}");
            }
        } else {
            GRAB.store(grab, Ordering::Release);
        }
    }
}

#[cfg(windows)]
impl Drop for ShellKeyHook {
    fn drop(&mut self) {
        use windows::Win32::Foundation::{LPARAM, WPARAM};
        use windows::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, WM_QUIT};
        GRAB.store(false, Ordering::Release);
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

    const TAB: u32 = 0x09;
    const ESC: u32 = 0x1B;
    const LWIN: u32 = 0x5B;
    const RWIN: u32 = 0x5C;
    const LALT: u32 = 0xA4;
    const LCTRL: u32 = 0xA2;
    const LSHIFT: u32 = 0xA0;
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
            assert!(seen.iter().all(|&(vk, _)| !shell_key(vk)), "{chord:x?}: Windows got {seen:x?}");
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
    fn a_hung_main_loop_lets_the_shells_keys_through() {
        assert!(beating(10_000, 10_000 - ALIVE_MS + 1));
        assert!(!beating(10_000, 10_000 - ALIVE_MS), "a frozen stream: Alt+Tab and Ctrl+Shift+Esc reach Windows");
        assert!(beating(5, 10), "a beat after the hook's clock read");
    }

    // Above the loop's longest wait with no picture due.
    const _: () = assert!(ALIVE_MS > 500);

    #[test]
    fn other_keys_and_ctrl_alt_del_reach_windows() {
        // Letters still reach the window (Nightjar's own Ctrl+Alt+Shift+letter shortcuts come
        // through raw input anyway); Ctrl+Alt+Del is Windows' however a hook answers.
        for vk in [D, DELETE, LSHIFT, 0x5A] {
            assert!(!shell_key(vk), "{vk:#x}");
            assert_eq!(step(true, vk, true, Key::Up), (false, Key::Passed), "{vk:#x}");
        }
    }

    #[test]
    fn the_keyboard_is_grabbed_while_input_is_captured_with_system_keys_on() {
        assert!(keyboard_grab(true, true));
        assert!(!keyboard_grab(true, false), "Send system keys off");
        assert!(!keyboard_grab(false, true), "released, the stream menu open or the window out of focus");
    }
}
