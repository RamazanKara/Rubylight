//! The visible top-level windows with a title and the process each belongs to, for the host to
//! tell a launcher showing a prompt from a game (`butterpollo_core::launcher_prompt`). Read-only:
//! it never sends a message, a click or a key to any of them.

use windows::Win32::{
    Foundation::{HWND, LPARAM},
    UI::WindowsAndMessaging::{
        EnumWindows, GetForegroundWindow, GetWindowTextLengthW, GetWindowTextW,
        GetWindowThreadProcessId, IsWindowVisible,
    },
};

pub struct TopWindow {
    pub pid: u32,
    pub title: String,
    /// Whether it is the window in front.
    pub foreground: bool,
}

/// The visible windows that have a title, on the desktop the signed-in user sees.
pub fn visible() -> Vec<TopWindow> {
    struct Scan {
        front: HWND,
        found: Vec<TopWindow>,
    }
    unsafe extern "system" fn visit(window: HWND, data: LPARAM) -> windows::core::BOOL {
        // SAFETY: EnumWindows passes the `&mut Scan` from `visible` as `data`, which outlives the
        // enumeration and is not otherwise used during it; `text` has the capacity passed.
        unsafe {
            let scan = &mut *(data.0 as *mut Scan);
            if IsWindowVisible(window).as_bool() {
                let length = GetWindowTextLengthW(window);
                if length > 0 {
                    let mut text = vec![0u16; length as usize + 1];
                    let copied = GetWindowTextW(window, &mut text).max(0) as usize;
                    let title = String::from_utf16_lossy(&text[..copied]);
                    if !title.trim().is_empty() {
                        let mut pid = 0;
                        GetWindowThreadProcessId(window, Some(&mut pid));
                        scan.found.push(TopWindow {
                            pid,
                            title,
                            foreground: window == scan.front,
                        });
                    }
                }
            }
            windows::core::BOOL(1)
        }
    }
    crate::input::follow_input_desktop();
    // SAFETY: GetForegroundWindow has no preconditions.
    let front = unsafe { GetForegroundWindow() };
    let mut scan = Scan {
        front,
        found: vec![],
    };
    // SAFETY: `scan` outlives the synchronous EnumWindows call that reads it through `visit`.
    unsafe {
        let _ = EnumWindows(Some(visit), LPARAM((&mut scan as *mut Scan) as isize));
    }
    scan.found
}
