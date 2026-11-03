//! A notice on the PC when a device starts a stream the person at the PC
//! can't see: a background stream or a seat in a Nucleus Co-op session. The
//! main stream takes over the screen, so it needs none.
use butterpollo_core::session::Seat;
use std::{
    collections::BTreeMap,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

/// The seat each device was last announced in, and whether a notice failed.
pub struct Notices {
    shown: Mutex<BTreeMap<String, Seat>>,
    failed: AtomicBool,
}
static NOTICES: Notices = Notices::new();

impl Notices {
    const fn new() -> Self {
        Self {
            shown: Mutex::new(BTreeMap::new()),
            failed: AtomicBool::new(false),
        }
    }
    /// Whether a launch of `device` in `seat` is announced: only a Nucleus
    /// seat, and only once per seat. A reconnect or resume of the same stream
    /// takes the same seat again; a new session of Nucleus has a new id.
    fn due(&self, device: &str, seat: Option<Seat>) -> bool {
        let Some(seat) = seat else {
            return false;
        };
        let mut shown = self.shown.lock().unwrap();
        if shown.get(device) == Some(&seat) {
            return false;
        }
        shown.insert(device.to_owned(), seat);
        true
    }
    /// Whether this failure is the first, the one that is logged as a warning.
    fn first_failure(&self) -> bool {
        !self.failed.swap(true, Ordering::Relaxed)
    }
}

/// The notice's title and text.
fn notice(background: bool, device: &str, app: &str) -> (&'static str, String) {
    (
        if background {
            "Background stream"
        } else {
            "Co-op stream"
        },
        format!("{device} started playing {app}."),
    )
}

/// Announce a launch that took a Nucleus Co-op seat (`seat`). A notice that
/// can't be shown is logged, once as a warning, and never fails the launch.
pub fn started(device_id: &str, device: &str, app: &str, background: bool, seat: Option<Seat>) {
    if !NOTICES.due(device_id, seat) {
        return;
    }
    let (title, text) = notice(background, device, app);
    if let Err(error) = butterpollo_windows::tray::try_notify(title, &text) {
        let error = format!("{error:#}");
        if NOTICES.first_failure() {
            tracing::warn!(%error, client = %device, "the stream notice could not be shown on the PC");
        } else {
            tracing::debug!(%error, client = %device, "the stream notice could not be shown on the PC");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_nucleus_seats_are_announced_once_per_seat() {
        let notices = Notices::new();
        let seat = |session, slot| Some(Seat { session, slot });
        // The main stream takes no seat: never announced.
        assert!(!notices.due("phone", None));
        assert!(!notices.due("phone", None));
        // A background or co-op stream: once, not again on a reconnect or resume.
        assert!(notices.due("phone", seat(1, 0)));
        assert!(!notices.due("phone", seat(1, 0)));
        // A main stream between does not make the reconnect new.
        assert!(!notices.due("phone", None));
        assert!(!notices.due("phone", seat(1, 0)));
        // Another device in the same co-op session is its own start.
        assert!(notices.due("tablet", seat(1, 1)));
        assert!(!notices.due("tablet", seat(1, 1)));
        // A new session of the same device is a new start.
        assert!(notices.due("phone", seat(2, 0)));
        assert!(!notices.due("phone", seat(2, 0)));
    }

    #[test]
    fn a_failed_notice_warns_once() {
        let notices = Notices::new();
        assert!(notices.first_failure());
        assert!(!notices.first_failure());
    }

    #[test]
    fn the_notice_names_the_device_and_the_game() {
        assert_eq!(
            notice(true, "Pixel 9", "Hades"),
            ("Background stream", "Pixel 9 started playing Hades.".into())
        );
        assert_eq!(notice(false, "Pixel 9", "Hades").0, "Co-op stream");
    }
}
