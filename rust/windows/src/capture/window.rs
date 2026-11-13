//! One game window as the capture target, for Nucleus Co-op: each player's
//! stream shows that player's game instance. The target names the window and
//! the display whose GPU and colour space the capture uses:
//! `window:<hwnd>@<display>`, or `window:<hwnd>/<pid>@<display>` to name the
//! process that owns the window too. Window targets are captured with Windows
//! Graphics Capture only; Desktop Duplication and the display driver's frame
//! push capture whole displays, so neither is ever used in their place.

use anyhow::{Context, Result, bail, ensure};
use windows::Win32::{
    Foundation::HWND,
    Graphics::Gdi::{
        GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MONITORINFOEXW, MonitorFromWindow,
    },
    UI::WindowsAndMessaging::{GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible},
};

pub(super) const PREFIX: &str = "window:";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowTarget {
    /// The window's handle, as `EnumWindows` gave it.
    pub window: u64,
    /// The display the capture opens its GPU on, such as `\\.\DISPLAY1`.
    pub display: String,
    /// The process that owns the window, when the target names it: a
    /// handle Windows gave to another window is then refused.
    pub process: Option<u32>,
}

impl WindowTarget {
    pub fn new(window: u64, display: &str) -> Self {
        Self {
            window,
            display: display.to_string(),
            process: None,
        }
    }
    pub fn owned_by(self, process: u32) -> Self {
        Self {
            process: Some(process),
            ..self
        }
    }
    /// `None` for a display name; an error for a malformed window target, so
    /// it is never opened as a display.
    pub fn parse(name: &str) -> Result<Option<Self>> {
        let Some(rest) = name.strip_prefix(PREFIX) else {
            return Ok(None);
        };
        let shape = || format!("capture target {name:?} is not window:<hwnd>@<display>");
        let (handle, display) = rest.split_once('@').with_context(shape)?;
        let (handle, process) = match handle.split_once('/') {
            Some((handle, process)) => (
                handle,
                Some(process.parse::<u32>().ok().with_context(shape)?),
            ),
            None => (handle, None),
        };
        let window = match handle.strip_prefix("0x") {
            Some(hex) => u64::from_str_radix(hex, 16),
            None => handle.parse(),
        }
        .ok()
        .with_context(shape)?;
        ensure!(window != 0, "capture target {name:?} names no window");
        ensure!(
            usize::try_from(window).is_ok(),
            "capture target {name:?} is not a window handle"
        );
        ensure!(
            !display.is_empty(),
            "capture target {name:?} names no display"
        );
        ensure!(
            process != Some(0),
            "capture target {name:?} names no process"
        );
        Ok(Some(Self {
            process,
            ..Self::new(window, display)
        }))
    }
    pub(super) fn handle(&self) -> usize {
        self.window as usize
    }
}

impl std::fmt::Display for WindowTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{PREFIX}{}", self.window)?;
        if let Some(process) = self.process {
            write!(f, "/{process}")?;
        }
        write!(f, "@{}", self.display)
    }
}

/// Window targets refuse a capture setting that cannot capture a window,
/// instead of capturing the display the window is not on.
pub(super) fn refuse_backend(kind: &str) -> Result<()> {
    match kind {
        "ddx" | "dxgi" => bail!(
            "a game window can only be captured with Windows Graphics Capture; Desktop Duplication captures whole displays. Select WGC or automatic capture"
        ),
        "amd" => bail!(
            "a game window can only be captured with Windows Graphics Capture; AMD Direct Capture captures whole displays. Select WGC or automatic capture"
        ),
        "idd-push" => bail!(
            "a game window can only be captured with Windows Graphics Capture; the Rubylight display driver's frame push captures whole displays. Select WGC or automatic capture"
        ),
        _ => Ok(()),
    }
}

/// Refuses a Windows build without window capture. Its floor is above WGC's,
/// whose own text would suggest Desktop Duplication.
pub(super) fn require_capture(build: Option<u32>) -> Result<()> {
    use butterpollo_core::os_gate::{Feature, missing};
    const { assert!(butterpollo_core::os_gate::WGC_WINDOW >= butterpollo_core::os_gate::WGC) };
    match missing(build, Feature::WgcWindow) {
        Some(reason) => bail!("{reason}"),
        None => Ok(()),
    }
}

/// Why a window cannot be captured now. A minimised window is not composed,
/// so WGC would deliver no frames and the stream would freeze silently.
pub(super) fn unavailable(exists: bool, minimised: bool) -> Option<&'static str> {
    if !exists {
        Some("the game window closed")
    } else if minimised {
        Some("the game window is minimised; restore it on the PC to stream it")
    } else {
        None
    }
}

pub(super) fn check(window: usize) -> Result<()> {
    let hwnd = HWND(window as *mut _);
    // SAFETY: Both calls only read the handle; a stale one makes them return false.
    let (exists, minimised) = unsafe { (IsWindow(Some(hwnd)).as_bool(), IsIconic(hwnd).as_bool()) };
    match unavailable(exists, minimised) {
        Some(reason) => bail!("{reason} (window {window:#x})"),
        None => Ok(()),
    }
}

/// [`check`] while a capture waits for its first frame, where a hidden window fails too:
/// a game replacing its splash window as the capture opens leaves one that sends no frame
/// and no word of closing, and the wait would use up the whole helper start (#59).
pub(super) fn check_awaited(window: usize) -> Result<()> {
    check(window)?;
    // SAFETY: The call only reads the handle; a stale one makes it return false.
    let shown = unsafe { IsWindowVisible(HWND(window as *mut _)).as_bool() };
    ensure!(shown, "the game window is hidden (window {window:#x})");
    Ok(())
}

/// The display that shows most of the window, such as `\\.\DISPLAY1`.
pub(super) fn display_of(window: usize) -> Result<String> {
    let mut info = MONITORINFOEXW {
        monitorInfo: MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFOEXW>() as u32,
            ..Default::default()
        },
        ..Default::default()
    };
    // SAFETY: `info` is a live local whose size is set to the extended struct's;
    // a stale handle only fails the call.
    let found = unsafe {
        let monitor = MonitorFromWindow(HWND(window as *mut _), MONITOR_DEFAULTTONEAREST);
        GetMonitorInfoW(monitor, (&mut info as *mut MONITORINFOEXW).cast()).as_bool()
    };
    ensure!(found, "no display shows the window {window:#x}");
    Ok(crate::text::from_wide(&info.szDevice))
}

/// The process that owns the window, 0 for a closed one.
pub(super) fn owner_of(window: usize) -> u32 {
    let mut process = 0;
    // SAFETY: `process` is a live local; a stale handle only returns 0.
    unsafe { GetWindowThreadProcessId(HWND(window as *mut _), Some(&mut process)) };
    process
}

/// Refuses a window handle owned by another process than the target names:
/// Windows reuses the handles of closed windows.
pub(super) fn owned_by(window: usize, owner: u32, named: u32) -> Result<()> {
    ensure!(
        owner == named,
        "the window {window:#x} belongs to process {owner}, not {named}; the window it named has closed"
    );
    Ok(())
}

/// Refuses a window shown on another display than the target names: the
/// capture would use that display's GPU and colour space for it, and a name
/// no display has would match none.
pub(super) fn on_display(window: usize, shown_on: &str, named: &str) -> Result<()> {
    ensure!(
        shown_on.eq_ignore_ascii_case(named),
        "the window {window:#x} is on {shown_on}, not on {named}; name the display it is on"
    );
    Ok(())
}

/// What to do with a captured frame of a window, whose size can change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Frame {
    Use,
    /// Holds no current picture: an empty window, or a frame from before the
    /// frame pool took the window's new size.
    Skip,
    /// The window has a new size: restart the capture at it and drop this
    /// frame, whose surface has the old size.
    Resize((i32, i32)),
}

/// `pool` is the frame pool's size, `content` the window's size when the
/// frame was composed, `surface` the size of the frame's texture.
pub(super) fn frame(pool: (i32, i32), content: (i32, i32), surface: (i32, i32)) -> Frame {
    if content.0 <= 0 || content.1 <= 0 {
        Frame::Skip
    } else if content != pool {
        Frame::Resize(content)
    } else if surface != pool {
        Frame::Skip
    } else {
        Frame::Use
    }
}

#[cfg(test)]
mod tests;
