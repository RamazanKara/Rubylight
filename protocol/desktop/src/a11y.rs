//! Screen readers: egui's AccessKit tree reaches UI Automation on Windows (Narrator, NVDA, JAWS)
//! and AT-SPI on Linux (Orca), on our SDL window instead of a winit one. It follows egui-winit's
//! AccessKit integration: the tree is built only once a screen reader asks for it (the activation
//! handler wakes the loop, which turns egui's AccessKit output on), every egui pass then sends a
//! full update, and the reader's actions come back as egui events.

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

pub struct Accessibility {
    shared: Arc<Shared>,
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
            Ok(Self { shared, adapter })
        }
        #[cfg(not(windows))]
        {
            let _ = window;
            let deactivation = Deactivation { shared: shared.clone() };
            let adapter = accesskit_unix::Adapter::new(activation, actions, deactivation);
            Ok(Self { shared, adapter })
        }
    }

    /// Before an egui pass: turns AccessKit output on or off as readers come and go, and passes
    /// their actions (click, focus, scroll into view, set value) on as input.
    pub fn begin_pass(&self, ctx: &egui::Context, events: &mut Vec<egui::Event>) {
        if self.shared.activated.swap(false, Ordering::SeqCst) {
            ctx.enable_accesskit();
        }
        if self.shared.deactivated.swap(false, Ordering::SeqCst) {
            ctx.disable_accesskit();
        }
        let requests = std::mem::take(&mut *self.shared.actions.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        events.extend(requests.into_iter().map(egui::Event::AccessKitActionRequest));
    }

    /// After an egui pass: the tree it built goes to the screen reader.
    pub fn update(&mut self, window: *mut SDL_Window, update: &TreeUpdate) {
        #[cfg(windows)]
        {
            let _ = window;
            if let Some(events) = self.adapter.update_if_active(|| update.clone()) {
                events.raise();
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
