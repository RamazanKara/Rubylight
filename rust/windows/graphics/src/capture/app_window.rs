//! The OS side of streaming the launched app's window
//! (`butterpollo_media_policy::app_window`): the top-level windows to pick it from,
//! the window in front, the capture target that names it, the rectangle
//! absolute input maps onto, and the window's client area inside a Windows
//! Graphics Capture frame.
use super::window::{self, WindowTarget};
use anyhow::Result;
use butterpollo_media_policy::app_window::Candidate;
use windows::Win32::{
    Foundation::{HWND, LPARAM, POINT, RECT},
    Graphics::{
        Direct3D11::D3D11_BOX,
        Direct3D12::D3D12_BOX,
        Dwm::{DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute},
        Gdi::ClientToScreen,
    },
    UI::WindowsAndMessaging::{
        EnumWindows, GW_OWNER, GWL_EXSTYLE, GetClientRect, GetForegroundWindow, GetWindow,
        GetWindowLongPtrW, GetWindowRect, GetWindowTextLengthW, GetWindowTextW,
        GetWindowThreadProcessId, IsIconic, IsWindowVisible, WS_EX_TOOLWINDOW,
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
    crate::process_info::follow_input_desktop();
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

/// The foreground window of the desktop the signed-in user sees, as
/// [`app_windows`] names windows; None when no window is in front (focus
/// changing hands, or a desktop the host cannot read). Read-only.
pub fn foreground_window() -> Option<u64> {
    crate::process_info::follow_input_desktop();
    // SAFETY: GetForegroundWindow takes nothing; a null window is none.
    let window = unsafe { GetForegroundWindow() };
    (!window.is_invalid()).then_some(window.0 as usize as u64)
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
/// `client` names the window's client area instead of its whole visible
/// frame, so input follows the crop (`capture_window_frame = client`).
pub fn app_window_target(id: u64, pid: u32, client: bool) -> Result<String> {
    let handle = usize::try_from(id)?;
    let display = window::display_of(handle)?;
    let target = WindowTarget::new(id, &display).owned_by(pid);
    Ok(if client { target.only_client() } else { target }.to_string())
}

/// The desktop rectangle a window target's picture shows, for absolute
/// input: None for a display name, a malformed target, a closed or minimised
/// window, or a handle Windows gave to another process's window since (the
/// checks capture makes), so input never lands on a window the stream is not.
/// A client-area target maps onto the client rectangle, capped to the frame
/// as the capture's crop is.
pub fn target_rect(name: &str) -> Option<RECT> {
    let target = WindowTarget::parse(name).ok()??;
    let handle = target.handle();
    window::check(handle).ok()?;
    if let Some(named) = target.process {
        window::owned_by(handle, window::owner_of(handle), named).ok()?;
    }
    let frame = bounds(HWND(handle as *mut _))?;
    if !target.client {
        return Some(frame);
    }
    let (origin, size) = client_area(handle)?;
    let surface = (
        u32::try_from(frame.right - frame.left).ok()?,
        u32::try_from(frame.bottom - frame.top).ok()?,
    );
    let crop = client_box(frame, origin, size, surface)?;
    Some(RECT {
        left: frame.left + i32::try_from(crop.left).ok()?,
        top: frame.top + i32::try_from(crop.top).ok()?,
        right: frame.left + i32::try_from(crop.right).ok()?,
        bottom: frame.top + i32::try_from(crop.bottom).ok()?,
    })
}

/// A rectangle of a captured frame's texture, in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Crop {
    pub left: u32,
    pub top: u32,
    pub right: u32,
    pub bottom: u32,
}

impl Crop {
    pub fn width(self) -> u32 {
        self.right - self.left
    }
    pub fn height(self) -> u32 {
        self.bottom - self.top
    }
    pub(crate) fn d3d11(self) -> D3D11_BOX {
        D3D11_BOX {
            left: self.left,
            top: self.top,
            front: 0,
            right: self.right,
            bottom: self.bottom,
            back: 1,
        }
    }
    pub(crate) fn d3d12(self) -> D3D12_BOX {
        D3D12_BOX {
            left: self.left,
            top: self.top,
            front: 0,
            right: self.right,
            bottom: self.bottom,
            back: 1,
        }
    }
}

/// The window's client area inside a Windows Graphics Capture frame of
/// `surface` pixels: `frame` is what WGC captures (its DWM extended frame
/// bounds in desktop pixels), `client_origin` the client area's top-left on
/// the desktop and `client_size` its size. Negative offsets and a box larger
/// than the surface are capped to the surface; None for a window without a
/// frame or a client area, or one entirely outside the surface, so callers
/// use the whole frame.
pub fn client_box(
    frame: RECT,
    client_origin: POINT,
    client_size: (i32, i32),
    surface: (u32, u32),
) -> Option<Crop> {
    if frame.right <= frame.left || frame.bottom <= frame.top || surface.0 == 0 || surface.1 == 0 {
        return None;
    }
    if client_size.0 <= 0 || client_size.1 <= 0 {
        return None;
    }
    let left = i64::from(client_origin.x) - i64::from(frame.left);
    let top = i64::from(client_origin.y) - i64::from(frame.top);
    let right = left + i64::from(client_size.0);
    let bottom = top + i64::from(client_size.1);
    let left = left.max(0);
    let top = top.max(0);
    let right = right.min(i64::from(surface.0));
    let bottom = bottom.min(i64::from(surface.1));
    if right <= left || bottom <= top {
        return None;
    }
    Some(Crop {
        left: left as u32,
        top: top as u32,
        right: right as u32,
        bottom: bottom as u32,
    })
}

/// The client area of window `window` on the desktop: its top-left and size,
/// in physical pixels. None for a closed or minimised window, or one that
/// has none.
fn client_area(window: usize) -> Option<(POINT, (i32, i32))> {
    let hwnd = HWND(window as *mut _);
    let mut client = RECT::default();
    let mut origin = POINT { x: 0, y: 0 };
    // SAFETY: Each call writes at most into the live local passed; a stale
    // handle only fails it.
    unsafe {
        GetClientRect(hwnd, &mut client).ok()?;
        if !ClientToScreen(hwnd, &mut origin).as_bool() {
            return None;
        }
    }
    (client.right > client.left && client.bottom > client.top).then_some((
        origin,
        (client.right - client.left, client.bottom - client.top),
    ))
}

/// The client area's box in a Windows Graphics Capture frame of window
/// `window` and `surface` pixels. None when it cannot be read (a closed or
/// minimised window) or is empty, so the whole frame is used.
pub(crate) fn window_client_box(window: usize, surface: (u32, u32)) -> Option<Crop> {
    let frame = bounds(HWND(window as *mut _))?;
    let (origin, size) = client_area(window)?;
    client_box(frame, origin, size, surface)
}

#[cfg(test)]
mod tests;
