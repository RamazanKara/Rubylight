//! Which WGC compositions of a display changed the foreground window, for
//! `Freshness::observe_focus`: a window animating beside a game made every
//! refresh a composition of its own.
use butterpollo_core::capture_policy::{focus_changed, focus_on_display, merge_focus};
use std::time::{Duration, Instant};
use windows::Graphics::Capture::Direct3D11CaptureFrame;
use windows::Win32::{
    Foundation::RECT,
    Graphics::Dwm::{DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute},
    UI::WindowsAndMessaging::{GetForegroundWindow, IsIconic},
};

pub(super) struct Focus {
    /// The captured display's desktop rectangle; `None` for a window
    /// capture, whose every update is the window's own.
    display: Option<[i32; 4]>,
    /// The foreground window's bounds in the display's pixels.
    window: Option<[i32; 4]>,
    checked: Option<Instant>,
    /// Cleared once a frame cannot report dirty regions (before Windows 11
    /// 24H2): every composition is then unlabelled, as before.
    regions: bool,
    dirty: Vec<[i32; 4]>,
    /// The label of the compositions since the last `take`.
    since: Option<bool>,
}
impl Focus {
    /// The foreground window moves or changes rarely; one asked this often
    /// costs a DWM query only between frames.
    const CHECK: Duration = Duration::from_millis(50);
    pub(super) fn new(display: Option<[i32; 4]>) -> Self {
        Self {
            display,
            window: None,
            checked: None,
            regions: true,
            dirty: Vec::new(),
            since: None,
        }
    }
    /// Label a composition taken from the frame pool, published or not: a
    /// dropped one's focus change belongs to the next published picture.
    pub(super) fn observe(&mut self, frame: &Direct3D11CaptureFrame) {
        if self.display.is_none() || !self.regions {
            return;
        }
        let regions = match frame.DirtyRegions() {
            Ok(regions) => regions,
            Err(error) => {
                self.regions = false;
                tracing::debug!(%error, "WGC reports no dirty regions; another window's updates count as pictures");
                return;
            }
        };
        self.dirty.clear();
        let read = (|| -> windows::core::Result<()> {
            for index in 0..regions.Size()? {
                let rect = regions.GetAt(index)?;
                self.dirty.push([
                    rect.X,
                    rect.Y,
                    rect.X.saturating_add(rect.Width),
                    rect.Y.saturating_add(rect.Height),
                ]);
            }
            Ok(())
        })();
        // A list read only in part could miss the focus window's rectangle:
        // that composition is a picture of its own, as without the labels.
        if let Err(error) = read {
            tracing::debug!(%error, "WGC dirty regions unreadable for one composition");
            self.since = Some(true);
            return;
        }
        self.since = merge_focus(self.since, focus_changed(&self.dirty, self.window));
    }
    /// The label of the compositions since the last call, for the picture
    /// published now.
    pub(super) fn take(&mut self) -> Option<bool> {
        self.since.take()
    }
    /// Ask for the foreground window's bounds once they are `CHECK` old;
    /// called while no frame waits.
    pub(super) fn refresh(&mut self, now: Instant) {
        let Some(display) = self.display else {
            return;
        };
        if !self.regions
            || self
                .checked
                .is_some_and(|checked| now.duration_since(checked) < Self::CHECK)
        {
            return;
        }
        self.checked = Some(now);
        self.window = foreground().and_then(|window| focus_on_display(window, display));
    }
}
/// The foreground window's visible bounds in desktop coordinates.
fn foreground() -> Option<[i32; 4]> {
    // SAFETY: GetForegroundWindow and IsIconic take no pointers; each
    // DwmGetWindowAttribute writes at most the size given into a live local.
    unsafe {
        let window = GetForegroundWindow();
        if window.is_invalid() || IsIconic(window).as_bool() {
            return None;
        }
        let mut cloaked = 0u32;
        let _ = DwmGetWindowAttribute(window, DWMWA_CLOAKED, (&mut cloaked as *mut u32).cast(), 4);
        if cloaked != 0 {
            return None;
        }
        let mut rect = RECT::default();
        DwmGetWindowAttribute(
            window,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            (&mut rect as *mut RECT).cast(),
            std::mem::size_of::<RECT>() as u32,
        )
        .ok()?;
        Some([rect.left, rect.top, rect.right, rect.bottom])
    }
}
