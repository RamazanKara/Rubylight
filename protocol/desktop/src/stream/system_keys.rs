//! "Send system keys" on Windows: a low-level keyboard hook on a thread of its own keeps the
//! shell's shortcuts (Alt+Tab, Alt+Shift+Tab, the Windows key and Win+key, Alt+Esc, Ctrl+Esc)
//! from Windows while the stream has the keyboard; raw input still hands every key to the PC.
//!
//! SDL's keyboard grab installs the same kind of hook, but on the main thread, which answers it
//! only between its waits in the present. Windows gives a hook that answers late a short time,
//! passes the key on when it runs out and, from Windows 7 on, silently removes the hook: Alt+Tab
//! then left the stream (nightjar#71) while Moonlight-qt kept it. This thread does nothing but
//! answer the hook, so no key waits for a refresh either. Ctrl+Alt+Del is never a hook's.

#[cfg(windows)]
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};

/// The keys SDL's grab takes from Windows, by virtual key: the Windows keys, Alt, Ctrl, Print
/// Screen, and Tab and Esc (the second key of Alt+Tab, Alt+Esc, Ctrl+Esc and Ctrl+Shift+Esc).
/// Windows sees no shortcut without its first key; Nightjar's own (Ctrl+Alt+Shift+letter) come
/// through raw input like every other key.
pub fn shell_key(vk: u32) -> bool {
    matches!(vk, 0x5B | 0x5C | 0xA4 | 0xA5 | 0xA2 | 0xA3 | 0x2C | 0x09 | 0x1B)
}

/// Whether the hook keeps a key event from Windows. `grab`: the keyboard is the stream's and its
/// window is in front; `swallowed_press`: this key's last press was kept. A release goes the way
/// its press went, so a key held from before the grab (Alt of the Alt+Tab that came back) is
/// released in Windows too, and one pressed during the grab stays the PC's after it ends.
pub fn swallows(grab: bool, vk: u32, down: bool, swallowed_press: bool) -> bool {
    if down { grab && shell_key(vk) } else { swallowed_press }
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
#[cfg(windows)]
static SWALLOWED: [AtomicBool; 256] = [const { AtomicBool::new(false) }; 256];

#[cfg(windows)]
unsafe extern "system" fn hook(code: i32, wparam: windows::Win32::Foundation::WPARAM, lparam: windows::Win32::Foundation::LPARAM) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::Foundation::LRESULT;
    use windows::Win32::UI::WindowsAndMessaging::*;
    if code == HC_ACTION as i32 {
        // SAFETY: Windows passes a KBDLLHOOKSTRUCT with HC_ACTION.
        let info = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        if let Some(swallowed) = SWALLOWED.get(info.vkCode as usize) {
            let down = matches!(wparam.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
            // SAFETY: no arguments.
            let grab = GRAB.load(Ordering::Acquire) && unsafe { GetForegroundWindow() }.0 as isize == WINDOW.load(Ordering::Acquire);
            let swallow = swallows(grab, info.vkCode, down, swallowed.load(Ordering::Relaxed));
            swallowed.store(down && swallow, Ordering::Relaxed);
            if swallow {
                return LRESULT(1);
            }
        }
    }
    // SAFETY: the arguments Windows passed.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
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
        use windows::Win32::System::LibraryLoader::GetModuleHandleW;
        use windows::Win32::System::Threading::GetCurrentThreadId;
        use windows::Win32::UI::WindowsAndMessaging::*;
        let (tx, rx) = std::sync::mpsc::channel();
        let thread = std::thread::Builder::new().name("nightjar-system-keys".into()).spawn(move || {
            // SAFETY: the hook procedure lives as long as the program; this thread pumps its messages.
            let installed = unsafe {
                GetModuleHandleW(None).and_then(|module| SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook), Some(module.into()), 0))
            };
            let hook = match installed {
                Ok(hook) => hook,
                Err(error) => {
                    let _ = tx.send(Err(error.to_string()));
                    return;
                }
            };
            // SAFETY: no arguments.
            let _ = tx.send(Ok(unsafe { GetCurrentThreadId() }));
            let mut message = MSG::default();
            // SAFETY: this thread's queue; WM_QUIT (Drop) ends it.
            while unsafe { GetMessageW(&mut message, None, 0, 0) }.0 > 0 {}
            // SAFETY: the hook this thread installed.
            let _ = unsafe { UnhookWindowsHookEx(hook) };
        }).map_err(|error| error.to_string())?;
        match rx.recv() {
            Ok(Ok(thread_id)) => Ok(Self { thread_id, thread: Some(thread) }),
            Ok(Err(error)) => Err(error),
            Err(_) => Err("the hook thread ended".into()),
        }
    }

    /// Whether the hook takes the shell's keys, and from which window (`hwnd`).
    pub fn set(&self, grab: bool, hwnd: isize) {
        WINDOW.store(hwnd, Ordering::Release);
        GRAB.store(grab, Ordering::Release);
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
    const DELETE: u32 = 0x2E;

    /// Plays key events through the hook's rules, as the hook keeps them: what Windows got.
    fn windows_sees(grab: bool, keys: &[(u32, bool)], swallowed: &mut [bool; 256]) -> Vec<(u32, bool)> {
        keys.iter().copied().filter(|&(vk, down)| {
            let swallow = swallows(grab, vk, down, swallowed[vk as usize]);
            swallowed[vk as usize] = down && swallow;
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
            let seen = windows_sees(true, &keys, &mut [false; 256]);
            assert!(seen.iter().all(|&(vk, _)| !shell_key(vk)), "{chord:x?}: Windows got {seen:x?}");
            let seen = windows_sees(false, &keys, &mut [false; 256]);
            assert_eq!(seen, keys, "{chord:x?}: not grabbed, Windows gets every key");
        }
    }

    #[test]
    fn a_release_goes_the_way_its_press_went() {
        let mut swallowed = [false; 256];
        // Alt held from the Alt+Tab that brought the window back: Windows saw it pressed.
        assert_eq!(windows_sees(true, &[(LALT, false)], &mut swallowed), [(LALT, false)]);
        // Win pressed during the grab, released after it ended: Windows never saw it.
        assert!(windows_sees(true, &[(LWIN, true)], &mut swallowed).is_empty());
        assert!(windows_sees(false, &[(LWIN, false)], &mut swallowed).is_empty());
        assert!(!swallowed[LWIN as usize]);
    }

    #[test]
    fn other_keys_and_ctrl_alt_del_reach_windows() {
        // Letters still reach the window (Nightjar's own Ctrl+Alt+Shift+letter shortcuts come
        // through raw input anyway); Ctrl+Alt+Del is Windows' however a hook answers.
        for vk in [D, DELETE, LSHIFT, 0x5A] {
            assert!(!shell_key(vk), "{vk:#x}");
            assert!(!swallows(true, vk, true, false), "{vk:#x}");
        }
    }

    #[test]
    fn the_keyboard_is_grabbed_while_input_is_captured_with_system_keys_on() {
        assert!(keyboard_grab(true, true));
        assert!(!keyboard_grab(true, false), "Send system keys off");
        assert!(!keyboard_grab(false, true), "released, the stream menu open or the window out of focus");
    }
}
