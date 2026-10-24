//! Tells a client that asks (control message 0x5538) when the host holds the video bitrate
//! below the requested one because of the network, so its overlay can say so.
//!
//! The link-rate controller ([`crate::link_rate`]) already tells the log and the console.
//! A client has no other way to see it: the picture just gets softer. The session
//! [`LinkNotices::note`]s each change; the control loop [`LinkNotices::take`]s the notice
//! to send, off the video send path, at most once a second. A change of state is sent when
//! that second is up; a bitrate that only drifts inside one state is held back (see
//! [`CALM_INTERVAL`]), so the overlay does not flicker.
pub use rubylight_protocol::control::{
    LINK_NOTICE_BYTES, LINK_NOTICE_MESSAGE_TYPE, LinkNotice, LinkNoticeRequest, LinkReason,
    LinkState,
};
use std::time::{Duration, Instant};

/// The least time between two notices. A burst of changes (a step down, a failed probe, the
/// next step down) sends the newest one when the second is up.
pub const INTERVAL: Duration = Duration::from_secs(1);

/// The least time between two notices of the same state (the bitrate moved inside "limited").
/// A move of more than [`CALM_FRACTION`] is sent after [`INTERVAL`] anyway.
pub const CALM_INTERVAL: Duration = Duration::from_secs(5);

/// A bitrate change inside one state is sent early only when it is more than this share (one
/// tenth) of the bitrate the client was last told.
const CALM_FRACTION: u32 = 10;

#[derive(Default)]
pub struct LinkNotices {
    enabled: bool,
    /// The newest state the controller reported; `None` until the first change.
    latest: Option<LinkNotice>,
    /// What the client was last sent, and when.
    sent: Option<(LinkNotice, Instant)>,
}

impl LinkNotices {
    /// Applies a client's request. Returns whether notices are on, or `None` for a malformed
    /// request, which changes nothing. A client that asks while the host is already below the
    /// requested bitrate hears it at once.
    pub fn on_request(&mut self, payload: &[u8]) -> Option<bool> {
        let request = LinkNoticeRequest::decode(payload)?;
        // Every valid request repeats a standing limit, also a second "on" in a row.
        self.sent = None;
        self.enabled = request.enabled;
        Some(self.enabled)
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Notes what the controller just did. A return to the requested rate is only kept for a
    /// client that asked; one that has not yet asked still hears a limit that holds.
    pub fn note(&mut self, notice: LinkNotice) {
        if !self.enabled && !notice.limited() {
            self.latest = None;
            return;
        }
        self.latest = Some(notice);
    }

    /// The host is at the requested bitrate again (a rebuilt controller, or the client set the
    /// bitrate itself) at `kbps`. A no-op unless a limit was reported.
    pub fn note_full(&mut self, kbps: u32) {
        if self.latest.is_some_and(|latest| latest.limited()) && kbps != 0 {
            self.note(LinkNotice {
                state: LinkState::Full,
                reason: LinkReason::None,
                ceiling_kbps: kbps,
                current_kbps: kbps,
            });
        }
    }

    /// The notice to send now, if the client asked, it differs from the last one sent and a
    /// second has passed since that. A change of state goes out then; a bitrate that moved
    /// inside the same state goes out when it moved by more than a tenth, or after
    /// [`CALM_INTERVAL`]. The newest wins. The first notice a client gets is never `Full`,
    /// which it assumes.
    pub fn take(&mut self, now: Instant) -> Option<[u8; LINK_NOTICE_BYTES]> {
        if !self.enabled {
            return None;
        }
        let latest = self.latest?;
        match self.sent {
            Some((sent, _)) if sent == latest => return None,
            Some((sent, at)) => {
                let waited = now.saturating_duration_since(at);
                if waited < INTERVAL || (calm(sent, latest) && waited < CALM_INTERVAL) {
                    return None;
                }
            }
            None if !latest.limited() => return None,
            _ => {}
        }
        self.sent = Some((latest, now));
        Some(latest.encode())
    }
}

/// Whether `latest` is only a small drift from what the client was told: the same state and
/// ceiling, and a bitrate within a tenth of the old one.
fn calm(sent: LinkNotice, latest: LinkNotice) -> bool {
    sent.state == latest.state
        && sent.ceiling_kbps == latest.ceiling_kbps
        && sent.current_kbps.abs_diff(latest.current_kbps) <= sent.current_kbps / CALM_FRACTION
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limited(current_kbps: u32) -> LinkNotice {
        LinkNotice {
            state: LinkState::Limited,
            reason: LinkReason::Network,
            ceiling_kbps: 150_000,
            current_kbps,
        }
    }

    fn full() -> LinkNotice {
        LinkNotice {
            state: LinkState::Full,
            reason: LinkReason::None,
            ceiling_kbps: 150_000,
            current_kbps: 150_000,
        }
    }

    fn asking() -> LinkNotices {
        let mut notices = LinkNotices::default();
        assert_eq!(
            notices.on_request(&LinkNoticeRequest { enabled: true }.encode()),
            Some(true)
        );
        notices
    }

    #[test]
    fn a_client_that_did_not_ask_never_hears() {
        let mut notices = LinkNotices::default();
        let t = Instant::now();
        notices.note(limited(75_000));
        assert_eq!(notices.take(t), None);
        assert_eq!(notices.take(t + Duration::from_secs(10)), None);
        assert!(!notices.enabled());
        // An explicit "off" is the same.
        assert_eq!(
            notices.on_request(&LinkNoticeRequest { enabled: false }.encode()),
            Some(false)
        );
        notices.note(limited(50_000));
        assert_eq!(notices.take(t + Duration::from_secs(20)), None);
    }

    #[test]
    fn a_malformed_request_changes_nothing() {
        let mut notices = LinkNotices::default();
        assert_eq!(notices.on_request(&[1, 1]), None);
        assert_eq!(notices.on_request(&[9, 1, 0, 0]), None);
        assert!(!notices.enabled());
        let mut notices = asking();
        assert_eq!(notices.on_request(&[1]), None);
        assert!(notices.enabled());
    }

    #[test]
    fn a_step_down_and_the_return_to_full_are_sent() {
        let mut notices = asking();
        let t = Instant::now();
        assert_eq!(notices.take(t), None, "nothing happened yet");
        notices.note(limited(75_000));
        let bytes = notices.take(t).expect("the step down is sent at once");
        assert_eq!(LinkNotice::decode(&bytes), Some(limited(75_000)));
        assert_eq!(notices.take(t + Duration::from_millis(8)), None, "once");
        notices.note(full());
        let bytes = notices.take(t + INTERVAL).expect("the return is sent");
        assert_eq!(LinkNotice::decode(&bytes), Some(full()));
        assert_eq!(notices.take(t + INTERVAL * 5), None);
    }

    #[test]
    fn at_most_one_notice_a_second_and_the_newest_wins() {
        let mut notices = asking();
        let t = Instant::now();
        notices.note(limited(100_000));
        assert!(notices.take(t).is_some());
        // Three changes inside the second: none goes out, and the last one is kept.
        for (ms, kbps) in [(100, 75_000), (400, 50_000), (800, 37_500)] {
            notices.note(limited(kbps));
            assert_eq!(notices.take(t + Duration::from_millis(ms)), None);
        }
        assert_eq!(notices.take(t + Duration::from_millis(999)), None);
        let bytes = notices.take(t + INTERVAL).expect("the second is up");
        assert_eq!(LinkNotice::decode(&bytes), Some(limited(37_500)));
        // Counting from that send, not from the first.
        notices.note(limited(30_000));
        assert_eq!(
            notices.take(t + INTERVAL + Duration::from_millis(500)),
            None
        );
        assert!(notices.take(t + INTERVAL * 2).is_some());
    }

    #[test]
    fn a_return_to_full_inside_the_second_is_not_lost() {
        let mut notices = asking();
        let t = Instant::now();
        notices.note(limited(75_000));
        assert!(notices.take(t).is_some());
        notices.note(full());
        assert_eq!(notices.take(t + Duration::from_millis(200)), None);
        // The control loop keeps asking; the notice goes out when it may.
        let bytes = notices
            .take(t + Duration::from_millis(1008))
            .expect("the clear arrives without a further change");
        assert_eq!(LinkNotice::decode(&bytes), Some(full()));
    }

    #[test]
    fn a_change_back_to_what_the_client_has_sends_nothing() {
        let mut notices = asking();
        let t = Instant::now();
        notices.note(limited(75_000));
        assert!(notices.take(t).is_some());
        notices.note(limited(50_000));
        notices.note(limited(75_000));
        assert_eq!(notices.take(t + INTERVAL), None);
    }

    #[test]
    fn a_full_the_client_never_saw_limited_is_not_sent() {
        let mut notices = asking();
        let t = Instant::now();
        notices.note(full());
        assert_eq!(notices.take(t), None);
        notices.note_full(150_000);
        assert_eq!(notices.take(t), None);
    }

    #[test]
    fn a_client_that_asks_late_hears_the_standing_limit() {
        let mut notices = LinkNotices::default();
        let t = Instant::now();
        // Reported before anyone asked: kept, because it still holds.
        notices.note(limited(60_000));
        assert_eq!(notices.take(t), None);
        notices.on_request(&LinkNoticeRequest { enabled: true }.encode());
        let bytes = notices.take(t).expect("heard at once");
        assert_eq!(LinkNotice::decode(&bytes), Some(limited(60_000)));
    }

    #[test]
    fn asking_again_repeats_the_standing_limit() {
        let mut notices = asking();
        let t = Instant::now();
        notices.note(limited(60_000));
        assert!(notices.take(t).is_some());
        notices.on_request(&LinkNoticeRequest { enabled: false }.encode());
        notices.on_request(&LinkNoticeRequest { enabled: true }.encode());
        assert!(notices.take(t + Duration::from_millis(1)).is_some());
    }

    #[test]
    fn asking_twice_in_a_row_repeats_the_standing_limit() {
        let mut notices = asking();
        let t = Instant::now();
        notices.note(limited(60_000));
        assert!(notices.take(t).is_some());
        assert_eq!(notices.take(t + Duration::from_millis(1)), None);
        notices.on_request(&LinkNoticeRequest { enabled: true }.encode());
        let bytes = notices
            .take(t + Duration::from_millis(2))
            .expect("the second request hears it again");
        assert_eq!(LinkNotice::decode(&bytes), Some(limited(60_000)));
    }

    #[test]
    fn a_small_drift_inside_one_state_waits_five_seconds_a_big_move_does_not() {
        let mut notices = asking();
        let t = Instant::now();
        notices.note(limited(100_000));
        assert!(notices.take(t).is_some());
        // Within a tenth: held back past the second, sent when the five are up, newest wins.
        notices.note(limited(95_000));
        assert_eq!(notices.take(t + INTERVAL), None);
        notices.note(limited(92_000));
        assert_eq!(notices.take(t + Duration::from_millis(4999)), None);
        let bytes = notices.take(t + CALM_INTERVAL).expect("the five are up");
        assert_eq!(LinkNotice::decode(&bytes), Some(limited(92_000)));
        // More than a tenth: sent when the second is up.
        notices.note(limited(80_000));
        assert_eq!(
            notices.take(t + CALM_INTERVAL + Duration::from_millis(500)),
            None
        );
        let bytes = notices
            .take(t + CALM_INTERVAL + INTERVAL)
            .expect("a big move is sent after the second");
        assert_eq!(LinkNotice::decode(&bytes), Some(limited(80_000)));
        // A change of state is never calmed: the floor and the return to full go out at once
        // (after the second), however small the number moved.
        let floor = LinkNotice {
            state: LinkState::Floor,
            ..limited(79_000)
        };
        notices.note(floor);
        let at = t + CALM_INTERVAL + INTERVAL * 2;
        assert_eq!(notices.take(at), Some(floor.encode()));
        notices.note(full());
        assert_eq!(notices.take(at + INTERVAL), Some(full().encode()));
    }

    #[test]
    fn a_rebuilt_controller_or_a_client_bitrate_clears_a_reported_limit() {
        let mut notices = asking();
        let t = Instant::now();
        notices.note(limited(60_000));
        assert!(notices.take(t).is_some());
        notices.note_full(120_000);
        let bytes = notices.take(t + INTERVAL).expect("cleared");
        let notice = LinkNotice::decode(&bytes).unwrap();
        assert_eq!(notice.state, LinkState::Full);
        assert_eq!(
            (notice.ceiling_kbps, notice.current_kbps),
            (120_000, 120_000)
        );
    }
}
