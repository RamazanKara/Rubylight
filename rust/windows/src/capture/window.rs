//! One game window as the capture target, for Nucleus Co-op: each player's
//! stream shows that player's game instance. The target names the window and
//! the display whose GPU and colour space the capture uses:
//! `window:<hwnd>@<display>`. Window targets are captured with Windows
//! Graphics Capture only; Desktop Duplication and the display driver's frame
//! push capture whole displays, so neither is ever used in their place.

use anyhow::{Context, Result, bail, ensure};
use windows::Win32::{
    Foundation::HWND,
    UI::WindowsAndMessaging::{IsIconic, IsWindow},
};

pub(super) const PREFIX: &str = "window:";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowTarget {
    /// The window's handle, as `EnumWindows` gave it.
    pub window: u64,
    /// The display the capture opens its GPU on, such as `\\.\DISPLAY1`.
    pub display: String,
}

impl WindowTarget {
    pub fn new(window: u64, display: &str) -> Self {
        Self {
            window,
            display: display.to_string(),
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
        Ok(Some(Self::new(window, display)))
    }
    pub(super) fn handle(&self) -> usize {
        self.window as usize
    }
}

impl std::fmt::Display for WindowTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{PREFIX}{}@{}", self.window, self.display)
    }
}

/// Window targets refuse a capture setting that cannot capture a window,
/// instead of capturing the display the window is not on.
pub(super) fn refuse_backend(kind: &str) -> Result<()> {
    match kind {
        "ddx" | "dxgi" => bail!(
            "a game window can only be captured with Windows Graphics Capture; Desktop Duplication captures whole displays. Select WGC or automatic capture"
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

/// What to do with a captured frame of a window, whose size can change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Frame {
    Use,
    /// Holds no current picture: an empty window, or a frame from before the
    /// frame pool took the window's new size.
    Skip,
    /// The window has a new size: recreate the frame pool at it and drop this
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
mod tests {
    use super::*;

    #[test]
    fn window_targets_parse_and_print_back() {
        let target = WindowTarget::parse(r"window:132456@\\.\DISPLAY2")
            .unwrap()
            .unwrap();
        assert_eq!(target, WindowTarget::new(132456, r"\\.\DISPLAY2"));
        assert_eq!(target.to_string(), r"window:132456@\\.\DISPLAY2");
        assert_eq!(target.handle(), 132456);
        assert_eq!(
            WindowTarget::parse(r"window:0x2056c@\\.\DISPLAY1")
                .unwrap()
                .unwrap()
                .window,
            0x2056c
        );
        let round = WindowTarget::new(77, r"\\.\DISPLAY9");
        assert_eq!(
            WindowTarget::parse(&round.to_string()).unwrap(),
            Some(round)
        );
    }

    #[test]
    fn display_names_are_not_window_targets() {
        for name in [r"\\.\DISPLAY1", "", "windows", r"\\.\DISPLAY1@window:5"] {
            assert_eq!(WindowTarget::parse(name).unwrap(), None, "{name}");
        }
    }

    #[test]
    fn malformed_window_targets_are_refused_not_opened_as_displays() {
        for (name, says) in [
            ("window:", "is not window:<hwnd>@<display>"),
            ("window:123", "is not window:<hwnd>@<display>"),
            (r"window:@\\.\DISPLAY1", "is not window:<hwnd>@<display>"),
            (r"window:abc@\\.\DISPLAY1", "is not window:<hwnd>@<display>"),
            (r"window:-5@\\.\DISPLAY1", "is not window:<hwnd>@<display>"),
            (r"window:0x@\\.\DISPLAY1", "is not window:<hwnd>@<display>"),
            (r"window:0@\\.\DISPLAY1", "names no window"),
            ("window:123@", "names no display"),
        ] {
            let error = WindowTarget::parse(name).unwrap_err().to_string();
            assert!(error.contains(says), "{name}: {error}");
        }
    }

    #[test]
    fn window_targets_refuse_backends_that_capture_whole_displays() {
        for kind in ["ddx", "dxgi", "idd-push"] {
            let error = refuse_backend(kind).unwrap_err().to_string();
            assert!(
                error.contains("only be captured with Windows Graphics Capture"),
                "{error}"
            );
        }
        for kind in ["wgc", "auto", ""] {
            refuse_backend(kind).unwrap();
        }
    }

    #[test]
    fn builds_without_window_capture_are_refused_without_another_backend() {
        for build in [16299, 17134, 17763] {
            let error = require_capture(Some(build)).unwrap_err().to_string();
            assert!(
                error.contains("WGC window capture needs Windows build 18362"),
                "{error}"
            );
            assert!(!error.contains("Desktop Duplication"), "{error}");
        }
        for build in [Some(18362), Some(19045), Some(26200), None] {
            require_capture(build).unwrap();
        }
    }

    #[test]
    fn a_closed_or_minimised_window_is_refused_loudly() {
        assert_eq!(unavailable(true, false), None);
        assert_eq!(unavailable(false, false), Some("the game window closed"));
        // IsIconic of a destroyed handle is false anyway; closed wins.
        assert_eq!(unavailable(false, true), Some("the game window closed"));
        assert!(unavailable(true, true).unwrap().contains("minimised"));
    }

    #[test]
    fn windows_are_checked_until_they_close() -> Result<()> {
        use windows::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DestroyWindow, WINDOW_EX_STYLE, WS_MINIMIZE, WS_POPUP,
        };
        // Hidden windows of the predefined STATIC class: nothing appears.
        let open = |style| {
            // SAFETY: The class and title are static NUL-terminated literals; no other pointers.
            unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    windows::core::w!("STATIC"),
                    windows::core::w!("Rubylight capture test"),
                    style,
                    0,
                    0,
                    64,
                    64,
                    None,
                    None,
                    None,
                    None,
                )
            }
        };
        let normal = open(WS_POPUP)?;
        let minimised = open(WS_POPUP | WS_MINIMIZE)?;
        let result = (|| -> Result<()> {
            check(normal.0 as usize)?;
            let error = check(minimised.0 as usize).unwrap_err().to_string();
            assert!(error.starts_with("the game window is minimised"), "{error}");
            Ok(())
        })();
        // SAFETY: Both windows were created on this thread above.
        unsafe {
            DestroyWindow(normal)?;
            DestroyWindow(minimised)?;
        }
        result?;
        let error = check(normal.0 as usize).unwrap_err().to_string();
        assert_eq!(
            error,
            format!("the game window closed (window {:#x})", normal.0 as usize)
        );
        Ok(())
    }

    #[test]
    fn a_resized_window_recreates_the_pool_and_skips_stale_frames() {
        // Steady: the window, the pool and the frame agree.
        assert_eq!(frame((1280, 720), (1280, 720), (1280, 720)), Frame::Use);
        // The window grew: the frame still has the old pool's texture.
        assert_eq!(
            frame((1280, 720), (1920, 1080), (1280, 720)),
            Frame::Resize((1920, 1080))
        );
        // And shrank.
        assert_eq!(
            frame((1920, 1080), (800, 600), (1920, 1080)),
            Frame::Resize((800, 600))
        );
        // After the pool was recreated: a frame queued before it has the
        // window's new size but the old texture, and is skipped.
        assert_eq!(frame((1920, 1080), (1920, 1080), (1280, 720)), Frame::Skip);
        assert_eq!(frame((1920, 1080), (1920, 1080), (1920, 1080)), Frame::Use);
        // An empty window never reaches the pool.
        assert_eq!(frame((1280, 720), (0, 0), (1280, 720)), Frame::Skip);
        assert_eq!(frame((1280, 720), (1280, 0), (1280, 720)), Frame::Skip);
    }
}
