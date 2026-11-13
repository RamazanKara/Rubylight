//! The OS side of streaming the launched app's window
//! (`butterpollo_core::app_window`): the top-level windows to pick it from,
//! the capture target that names it, and the rectangle absolute input maps
//! onto.
use super::window::{self, WindowTarget};
use anyhow::Result;
use butterpollo_core::app_window::Candidate;
use windows::Win32::{
    Foundation::{HWND, LPARAM, RECT},
    Graphics::Dwm::{DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute},
    UI::WindowsAndMessaging::{
        EnumWindows, GW_OWNER, GWL_EXSTYLE, GetWindow, GetWindowLongPtrW, GetWindowRect,
        GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
        WS_EX_TOOLWINDOW,
    },
};

/// The most windows one scan reads; a desktop has a few hundred.
const WINDOW_LIMIT: usize = 4096;
/// The longest title read, in characters.
const TITLE_LIMIT: i32 = 512;

/// Every top-level window of the desktop the signed-in user sees, without
/// program names (`Candidate::exe` is empty: the host knows the processes).
/// Read-only: no window is sent a message.
pub fn app_windows() -> Result<Vec<Candidate>> {
    unsafe extern "system" fn visit(window: HWND, context: LPARAM) -> windows::core::BOOL {
        // SAFETY: EnumWindows passes the `Vec` from app_windows as `context`, which outlives
        // the enumeration; every out-pointer is a live local of the size passed, and a window
        // that closed meanwhile only fails the calls.
        unsafe {
            let found = &mut *(context.0 as *mut Vec<Candidate>);
            if found.len() >= WINDOW_LIMIT {
                return false.into();
            }
            let mut pid = 0;
            GetWindowThreadProcessId(window, Some(&mut pid));
            let mut cloaked = 0u32;
            let _ =
                DwmGetWindowAttribute(window, DWMWA_CLOAKED, (&mut cloaked as *mut u32).cast(), 4);
            let shown = IsWindowVisible(window).as_bool() && cloaked == 0;
            let iconic = IsIconic(window).as_bool();
            let (width, height) =
                bounds(window).map_or((0, 0), |r| (r.right - r.left, r.bottom - r.top));
            let length = GetWindowTextLengthW(window).clamp(0, TITLE_LIMIT);
            let title = if length > 0 {
                let mut text = vec![0u16; length as usize + 1];
                let copied = GetWindowTextW(window, &mut text).max(0) as usize;
                String::from_utf16_lossy(&text[..copied.min(length as usize)])
            } else {
                String::new()
            };
            let ex = GetWindowLongPtrW(window, GWL_EXSTYLE) as u32;
            found.push(Candidate {
                id: window.0 as usize as u64,
                pid,
                exe: String::new(),
                title,
                width,
                height,
                visible: shown && !iconic,
                minimized: shown && iconic,
                owned: !GetWindow(window, GW_OWNER).unwrap_or_default().is_invalid(),
                tool: ex & WS_EX_TOOLWINDOW.0 != 0,
            });
            true.into()
        }
    }
    crate::input::follow_input_desktop();
    let mut found: Vec<Candidate> = Vec::new();
    // SAFETY: `found` outlives the synchronous EnumWindows call that fills it through `visit`.
    unsafe {
        EnumWindows(
            Some(visit),
            LPARAM((&mut found as *mut Vec<Candidate>) as isize),
        )?;
    }
    Ok(found)
}

/// What Windows Graphics Capture captures of the window, in desktop pixels:
/// its visible frame, without the invisible resize borders around it.
fn bounds(window: HWND) -> Option<RECT> {
    let mut rect = RECT::default();
    // SAFETY: Each call writes at most the size given into the live local `rect`; a stale
    // handle only fails it.
    unsafe {
        let framed = DwmGetWindowAttribute(
            window,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            (&mut rect as *mut RECT).cast(),
            std::mem::size_of::<RECT>() as u32,
        );
        if framed.is_err() {
            GetWindowRect(window, &mut rect).ok()?;
        }
    }
    (rect.right > rect.left && rect.bottom > rect.top).then_some(rect)
}

/// The capture target of window `id` of process `pid`, on the display that
/// shows most of it, whose GPU and colour space its capture uses. A window
/// moved to another display gets another target, and its capture reopens.
pub fn app_window_target(id: u64, pid: u32) -> Result<String> {
    let handle = usize::try_from(id)?;
    let display = window::display_of(handle)?;
    Ok(WindowTarget::new(id, &display).owned_by(pid).to_string())
}

/// The desktop rectangle a window target's picture shows, for absolute
/// input: None for a display name, a malformed target or a closed window.
pub fn target_rect(name: &str) -> Option<RECT> {
    let target = WindowTarget::parse(name).ok()??;
    bounds(HWND(target.handle() as *mut _))
}
