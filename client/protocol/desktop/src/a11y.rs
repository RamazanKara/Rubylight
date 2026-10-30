//! Screen readers: egui's AccessKit tree reaches UI Automation on Windows (Narrator, NVDA, JAWS)
//! and AT-SPI on Linux (Orca), on our SDL window instead of a winit one. It follows egui-winit's
//! AccessKit integration: the tree is built only once a screen reader asks for it (the activation
//! handler wakes the loop, which turns egui's AccessKit output on), every egui pass then sends a
//! full update, and the reader's actions come back as egui events. A stream's frames build no tree
//! unless its menu is open, and the output goes off again once no reader is there.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use egui::accesskit::{ActionHandler, ActionRequest, ActivationHandler, TreeUpdate};
use sdl3_sys::everything::*;

use crate::hosts::Waker;

/// What the platform adapter's handlers hand to the UI thread.
#[derive(Default)]
struct Shared {
    /// A screen reader asked for the tree; egui's AccessKit output goes on.
    activated: AtomicBool,
    /// The last screen reader went away (AT-SPI only); the output goes off again.
    deactivated: AtomicBool,
    actions: Mutex<Vec<ActionRequest>>,
}

struct Activation {
    shared: Arc<Shared>,
    wake: Waker,
}

impl ActivationHandler for Activation {
    fn request_initial_tree(&mut self) -> Option<TreeUpdate> {
        // The tree comes with the next egui pass, which the wake starts.
        self.shared.activated.store(true, Ordering::SeqCst);
        (self.wake)();
        None
    }
}

struct Actions {
    shared: Arc<Shared>,
    wake: Waker,
}

impl ActionHandler for Actions {
    fn do_action(&mut self, request: ActionRequest) {
        self.shared.actions.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(request);
        (self.wake)();
    }
}

#[cfg(not(windows))]
struct Deactivation {
    shared: Arc<Shared>,
}

#[cfg(not(windows))]
impl egui::accesskit::DeactivationHandler for Deactivation {
    fn deactivate_accessibility(&mut self) {
        self.shared.deactivated.store(true, Ordering::SeqCst);
    }
}

/// Whether egui builds its AccessKit tree: only while a reader is there and the pass has something
/// to read (not a stream's frames without the menu, which run at the stream's rate).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Switch {
    reader: bool,
    on: bool,
}

impl Switch {
    /// The output for the coming pass: Some(true) turns it on, Some(false) off, None keeps it.
    fn pass(&mut self, activated: bool, deactivated: bool, wanted: bool) -> Option<bool> {
        if activated {
            self.reader = true;
        }
        if deactivated {
            self.reader = false;
        }
        let on = self.reader && wanted;
        (on != self.on).then(|| {
            self.on = on;
            on
        })
    }

    /// The adapter had no reader to send the last tree to.
    fn no_reader(&mut self) {
        self.reader = false;
    }
}

pub struct Accessibility {
    shared: Arc<Shared>,
    switch: Switch,
    #[cfg(windows)]
    adapter: accesskit_windows::SubclassingAdapter,
    #[cfg(not(windows))]
    adapter: accesskit_unix::Adapter,
}

impl Accessibility {
    /// Attaches to `window`, which must not have been shown yet (UI Automation's subclassing
    /// adapter must be in place before the first WM_GETOBJECT).
    pub fn attach(window: *mut SDL_Window, wake: Waker) -> Result<Self, String> {
        let shared = Arc::new(Shared::default());
        let activation = Activation { shared: shared.clone(), wake: wake.clone() };
        let actions = Actions { shared: shared.clone(), wake };
        #[cfg(windows)]
        {
            // SAFETY: a live window; the property is SDL's HWND for it.
            let hwnd = unsafe {
                SDL_GetPointerProperty(SDL_GetWindowProperties(window), SDL_PROP_WINDOW_WIN32_HWND_POINTER, std::ptr::null_mut())
            };
            if hwnd.is_null() {
                return Err(format!("the window has no HWND: {}", crate::sdl::error()));
            }
            // SAFETY: plain flag query on a live window.
            if unsafe { SDL_GetWindowFlags(window) } & SDL_WINDOW_HIDDEN == 0 {
                return Err("the window is already shown".into());
            }
            let adapter = accesskit_windows::SubclassingAdapter::new(windows::Win32::Foundation::HWND(hwnd), activation, actions);
            Ok(Self { shared, switch: Switch::default(), adapter })
        }
        #[cfg(not(windows))]
        {
            let _ = window;
            let deactivation = Deactivation { shared: shared.clone() };
            let adapter = accesskit_unix::Adapter::new(activation, actions, deactivation);
            Ok(Self { shared, switch: Switch::default(), adapter })
        }
    }

    /// Before an egui pass: turns AccessKit output on or off as readers come and go and as the
    /// pass is `wanted` (false for a stream's frames without its menu), and passes the readers'
    /// actions (click, focus, scroll into view, set value) on as input.
    pub fn begin_pass(&mut self, ctx: &egui::Context, events: &mut Vec<egui::Event>, wanted: bool) {
        let activated = self.shared.activated.swap(false, Ordering::SeqCst);
        let deactivated = self.shared.deactivated.swap(false, Ordering::SeqCst);
        match self.switch.pass(activated, deactivated, wanted) {
            Some(true) => ctx.enable_accesskit(),
            Some(false) => ctx.disable_accesskit(),
            None => {}
        }
        let requests = std::mem::take(&mut *self.shared.actions.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        events.extend(requests.into_iter().map(egui::Event::AccessKitActionRequest));
    }

    /// After an egui pass: the tree it built goes to the screen reader.
    pub fn update(&mut self, window: *mut SDL_Window, update: &TreeUpdate) {
        #[cfg(windows)]
        {
            let _ = window;
            match self.adapter.update_if_active(|| update.clone()) {
                Some(events) => events.raise(),
                // No reader has the tree: stop building it until one asks again.
                None => self.switch.no_reader(),
            }
        }
        #[cfg(not(windows))]
        {
            // AT-SPI places the nodes on screen from the window's bounds (X11; Wayland keeps
            // window positions to itself).
            let (mut x, mut y, mut w, mut h) = (0, 0, 0, 0);
            let (mut top, mut left, mut bottom, mut right) = (0, 0, 0, 0);
            // SAFETY: queries on a live window.
            unsafe {
                SDL_GetWindowPosition(window, &mut x, &mut y);
                SDL_GetWindowSizeInPixels(window, &mut w, &mut h);
                SDL_GetWindowBordersSize(window, &mut top, &mut left, &mut bottom, &mut right);
            }
            let rect = |x0: i32, y0: i32, x1: i32, y1: i32| egui::accesskit::Rect::new(f64::from(x0), f64::from(y0), f64::from(x1), f64::from(y1));
            let inner = rect(x, y, x + w, y + h);
            let outer = rect(x - left, y - top, x + w + right, y + h + bottom);
            self.adapter.set_root_window_bounds(outer, inner);
            self.adapter.update_if_active(|| update.clone());
        }
    }

    /// The window gained or lost focus. On Windows the subclassing adapter sees that itself.
    pub fn focus(&mut self, focused: bool) {
        #[cfg(not(windows))]
        self.adapter.update_window_focus_state(focused);
        #[cfg(windows)]
        let _ = focused;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tree_is_built_only_for_a_reader_and_never_for_a_bare_stream() {
        let mut switch = Switch::default();
        assert_eq!(switch.pass(false, false, true), None, "no reader: no tree");
        assert_eq!(switch.pass(true, false, true), Some(true), "a reader asks: the launcher's tree");
        assert_eq!(switch.pass(false, false, true), None, "built every pass meanwhile");
        assert_eq!(switch.pass(false, false, false), Some(false), "a stream without its menu builds none");
        assert_eq!(switch.pass(false, false, false), None);
        assert_eq!(switch.pass(false, false, true), Some(true), "its menu opens: the tree again");
        switch.no_reader();
        assert_eq!(switch.pass(false, false, true), Some(false), "the adapter had no reader: off");
        assert_eq!(switch.pass(false, false, true), None, "until one asks again");
        assert_eq!(switch.pass(true, false, true), Some(true));
        assert_eq!(switch.pass(false, true, true), Some(false), "the last reader went away (AT-SPI)");
    }

    /// What building the tree costs one egui pass of a stream-menu-sized UI. Run with
    /// `--ignored --nocapture`.
    #[test]
    #[ignore = "a measurement, run by hand"]
    fn measure_the_trees_cost_per_pass() {
        let pass = |ctx: &egui::Context| {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                for i in 0..12 {
                    ui.label(format!("Line {i}: 12.3 ms"));
                }
                for i in 0..20 {
                    let _ = ui.button(format!("Button {i}"));
                }
            });
            output.textures_delta.clear();
        };
        for on in [false, true] {
            let ctx = egui::Context::default();
            if on {
                ctx.enable_accesskit();
            }
            for _ in 0..50 {
                pass(&ctx);
            }
            let start = std::time::Instant::now();
            for _ in 0..1000 {
                pass(&ctx);
            }
            println!("accesskit {}: {:.1} µs a pass", if on { "on" } else { "off" }, start.elapsed().as_secs_f64() * 1e3);
        }
    }
}
