//! The couch menu: a host menu drawn into the video stream, which any
//! Moonlight client opens with a controller, a keyboard or touch
//! (`docs/couch-menu-design.md`). Shipped as Experimental.
//!
//! Everything here is pure: [`chord`] and [`filter`] watch and capture the
//! input, [`model`] holds the pages and turns navigation into [`model::Action`]s
//! for the host, [`layout`] places the panel and [`render`] draws it with the
//! embedded [`font`]. The host wires them: the control loop runs the
//! [`Filter`], a menu worker owns the [`Menu`], and the video loop reads
//! [`Shared`] once per frame.
pub mod chord;
pub mod filter;
pub mod font;
pub mod layout;
pub mod model;
pub mod render;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

pub use filter::{Filter, Outcome};
pub use layout::Layout;
pub use model::{Action, Menu, Snapshot, Strings};
pub use render::render;

/// What the player asked the open menu to do, in order. The control loop
/// sends these to the menu worker.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Nav {
    Up,
    Down,
    Left,
    Right,
    Accept,
    Back,
    /// A click or tap at a stream position, `0..=1` on each axis.
    Point {
        x: f32,
        y: f32,
    },
    /// The menu opened (by a chord or the native API): show the main page.
    Opened,
    /// The menu closed: drop the page state.
    Closed,
}

/// The panel the worker drew, as the encoder composites it. `origin` and
/// `size` are in target pixels and multiples of 16; `rgba` is the panel at
/// 1×, `size / scale` pixels, RGBA8 sRGB, alpha 255.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub origin: [u32; 2],
    pub size: [u32; 2],
    pub scale: u32,
    pub rgba: Arc<[u8]>,
}

type Wake = Mutex<Option<Box<dyn Fn() + Send + Sync>>>;

/// One session's menu state, shared by the control loop, the menu worker,
/// the video loop and the native API.
///
/// `state` packs the open flag (bit 63) and a revision (the low 63 bits)
/// that rises on every change, so the video loop does one atomic load per
/// frame and touches the frame lock only when that value changes.
#[derive(Default)]
pub struct Shared {
    state: AtomicU64,
    /// Rises on every open, so a close and reopen between two filter passes
    /// is still seen as one.
    opens: AtomicU64,
    /// The published panel, and the lock every change of `state` holds.
    frame: Mutex<Option<Arc<Frame>>>,
    /// Why the menu cannot open in this session (read-back encode path,
    /// stream too small), set by the host.
    refusal: Mutex<Option<String>>,
    /// Wakes the stream loop (`capture_wake.set()`).
    wake: Wake,
    /// Wakes the control loop, so its filter sees an open or close from
    /// elsewhere.
    control_wake: Wake,
}

impl Shared {
    pub const OPEN: u64 = 1 << 63;

    pub fn new() -> Self {
        Self::default()
    }
    /// The packed state: one `Acquire` load, for the video loop.
    pub fn load(&self) -> u64 {
        self.state.load(Ordering::Acquire)
    }
    pub fn is_open(state: u64) -> bool {
        state & Self::OPEN != 0
    }
    pub fn revision(state: u64) -> u64 {
        state & !Self::OPEN
    }
    pub fn set_wake(&self, wake: impl Fn() + Send + Sync + 'static) {
        *self.wake.lock().unwrap() = Some(Box::new(wake));
    }
    pub fn set_control_wake(&self, wake: impl Fn() + Send + Sync + 'static) {
        *self.control_wake.lock().unwrap() = Some(Box::new(wake));
    }
    /// From now on [`Shared::open`] fails with `reason`; `None` allows it again.
    /// Does not close a menu that is open.
    pub fn refuse(&self, reason: Option<String>) {
        *self.refusal.lock().unwrap() = reason;
    }
    /// Opens the menu. `Ok(false)` if it was open already; `Err` with the
    /// reason if this session cannot show it.
    pub fn open(&self) -> Result<bool, String> {
        if let Some(reason) = self.refusal.lock().unwrap().clone() {
            return Err(reason);
        }
        let changed = self.change(|state, _| {
            if Self::is_open(state) {
                return None;
            }
            // Before the open bit is stored, so whoever sees it open also
            // sees this open counted.
            self.opens.fetch_add(1, Ordering::AcqRel);
            Some(Self::OPEN)
        });
        if changed {
            self.wake_control();
        }
        Ok(changed)
    }
    /// Closes the menu and drops its panel. `false` if it was closed already.
    pub fn close(&self) -> bool {
        let changed = self.change(|state, frame| {
            if !Self::is_open(state) {
                return None;
            }
            *frame = None;
            Some(0)
        });
        if changed {
            self.wake_control();
        }
        changed
    }
    /// Publishes a newly drawn panel. Refused (returns `false`, frame
    /// dropped) once the menu has closed, so a late frame never shows.
    pub fn publish(&self, new: Frame) -> bool {
        let mut new = Some(Arc::new(new));
        self.change(|state, frame| {
            if !Self::is_open(state) {
                return None;
            }
            *frame = new.take();
            Some(Self::OPEN)
        })
    }
    /// The published panel, if the menu is open and one was drawn.
    pub fn frame(&self) -> Option<Arc<Frame>> {
        self.frame.lock().unwrap().clone()
    }
    pub(crate) fn opens(&self) -> u64 {
        self.opens.load(Ordering::Acquire)
    }
    /// Applies `f` under the frame lock; `f` returns the new open bit or
    /// `None` for no change. Raises the revision and wakes the stream loop
    /// (outside the lock) on a change.
    fn change(&self, f: impl FnOnce(u64, &mut Option<Arc<Frame>>) -> Option<u64>) -> bool {
        {
            let mut frame = self.frame.lock().unwrap();
            let state = self.state.load(Ordering::Acquire);
            let Some(open) = f(state, &mut frame) else {
                return false;
            };
            let revision = Self::revision(state).wrapping_add(1) & !Self::OPEN;
            self.state.store(open | revision, Ordering::Release);
        }
        if let Some(wake) = &*self.wake.lock().unwrap() {
            wake();
        }
        true
    }
    fn wake_control(&self) {
        if let Some(wake) = &*self.control_wake.lock().unwrap() {
            wake();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    fn frame() -> Frame {
        Frame {
            origin: [16, 32],
            size: [288, 272],
            scale: 1,
            rgba: vec![255; 288 * 272 * 4].into(),
        }
    }

    #[test]
    fn couch_menu_shared_packs_open_and_revision_and_wakes_on_change() {
        let shared = Shared::new();
        let wakes = Arc::new(AtomicUsize::new(0));
        let controls = Arc::new(AtomicUsize::new(0));
        let w = wakes.clone();
        shared.set_wake(move || {
            w.fetch_add(1, Ordering::Relaxed);
        });
        let c = controls.clone();
        shared.set_control_wake(move || {
            c.fetch_add(1, Ordering::Relaxed);
        });
        assert_eq!(shared.load(), 0);
        assert!(
            !shared.publish(frame()),
            "nothing is published while closed"
        );
        assert_eq!(shared.open(), Ok(true));
        assert_eq!(shared.open(), Ok(false), "opening is idempotent");
        let st = shared.load();
        assert!(Shared::is_open(st) && Shared::revision(st) == 1);
        assert!(shared.frame().is_none());
        assert!(shared.publish(frame()));
        let st = shared.load();
        assert!(Shared::is_open(st) && Shared::revision(st) == 2);
        assert_eq!(shared.frame().unwrap().origin, [16, 32]);
        assert!(shared.close());
        assert!(!shared.close());
        let st = shared.load();
        assert!(!Shared::is_open(st) && Shared::revision(st) == 3);
        assert!(shared.frame().is_none(), "closing drops the panel");
        assert!(
            !shared.publish(frame()),
            "a late frame after close is dropped"
        );
        assert_eq!(wakes.load(Ordering::Relaxed), 3);
        assert_eq!(controls.load(Ordering::Relaxed), 2, "open and close only");
        assert_eq!(shared.opens(), 1);
    }

    #[test]
    fn couch_menu_shared_counts_an_open_before_anyone_sees_it_open() {
        let shared = Arc::new(Shared::new());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (weak, log) = (Arc::downgrade(&shared), seen.clone());
        shared.set_wake(move || {
            let shared = weak.upgrade().unwrap();
            let open = Shared::is_open(shared.load());
            log.lock().unwrap().push((open, shared.opens()));
        });
        shared.open().unwrap();
        shared.close();
        shared.open().unwrap();
        assert_eq!(*seen.lock().unwrap(), [(true, 1), (false, 1), (true, 2)]);
    }

    #[test]
    fn couch_menu_shared_refusal_is_loud_and_reversible() {
        let shared = Shared::new();
        shared.refuse(Some("couch menu: needs wgc_direct_encoder_input".into()));
        assert_eq!(
            shared.open(),
            Err("couch menu: needs wgc_direct_encoder_input".into())
        );
        assert_eq!(shared.load(), 0);
        shared.refuse(None);
        assert_eq!(shared.open(), Ok(true));
    }
}
