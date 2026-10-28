//! Mid-stream bitrate and frame-rate changes over 0x553C, on a stream whose DESCRIBE lists
//! them: the stream menu's requests, the client-hint bitrate controller's
//! (`Settings::client_hint_bitrate`, ticked once a second), and what the host answered.
//! Time is session-relative milliseconds.

use nightjar_core::http::AppRole;
use nightjar_policy::client_hint;
use nightjar_policy::stats::Stats;
use rubylight_protocol::rate_change::{Reason, RateChangeRequest, RateChangeResult, Status, RATE_CHANGE_MESSAGE_TYPE};

use crate::strings::{tr, trf};

/// Time between two controller ticks.
pub const TICK_MS: u64 = 1000;
/// A request the host has not answered in this long is given up on, loudly.
pub const ANSWER_MS: u64 = 5000;

/// Whether the stream takes rate changes: the host listed 0x553C, which it does only for a
/// launch with `rlRateChange=1`, and the stream is a full one.
pub fn offered(host_control_messages: &[u16], role: AppRole) -> bool {
    role == AppRole::Stream && host_control_messages.contains(&RATE_CHANGE_MESSAGE_TYPE)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Origin {
    Menu,
    Hint,
}

/// What a result changed, for the stream to follow.
#[derive(Debug, Default, PartialEq)]
pub struct Answer {
    /// Shown as a toast.
    pub toast: Option<String>,
    /// The stream's bitrate after the result.
    pub bitrate_kbps: Option<u32>,
    /// The stream's rate after the result, when it changed.
    pub fps_millihz: Option<u32>,
}

pub struct RateChange {
    next_serial: u32,
    /// Requests waiting for their result, with when they were sent.
    pending: Vec<(u32, Origin, u64)>,
    hint: Option<client_hint::Bitrate>,
    last_tick_ms: Option<u64>,
    bitrate_kbps: u32,
    fps_millihz: u32,
    /// The menu's fields.
    pub menu_kbps: u32,
    pub menu_fps: f64,
}

impl RateChange {
    /// `hint` runs the client-hint controller from the stream's bitrate, under this client's
    /// cap `limit_kbps` (0 for none).
    pub fn new(bitrate_kbps: u32, fps_millihz: u32, hint: bool, limit_kbps: u32, now_ms: u64) -> Self {
        Self {
            next_serial: 1,
            pending: Vec::new(),
            hint: hint.then(|| client_hint::Bitrate::new(bitrate_kbps, limit_kbps, now_ms)),
            last_tick_ms: None,
            bitrate_kbps,
            fps_millihz,
            menu_kbps: bitrate_kbps,
            menu_fps: f64::from(fps_millihz) / 1000.0,
        }
    }

    /// The client-hint controller steers this stream's bitrate.
    pub fn hint_active(&self) -> bool {
        self.hint.as_ref().is_some_and(|hint| !hint.stopped())
    }

    /// The controller can still raise the bitrate.
    pub fn hint_has_room(&self) -> bool {
        self.hint.as_ref().is_some_and(|hint| !hint.stopped() && hint.current_kbps() < hint.max_kbps())
    }

    /// A menu request waits for its result.
    pub fn menu_busy(&self) -> bool {
        self.pending.iter().any(|(_, origin, _)| *origin == Origin::Menu)
    }

    fn request(&mut self, origin: Origin, bitrate_kbps: u32, fps_millihz: u32, now_ms: u64) -> RateChangeRequest {
        let serial = self.next_serial;
        self.next_serial = self.next_serial.wrapping_add(1).max(1);
        self.pending.push((serial, origin, now_ms));
        RateChangeRequest { serial, bitrate_kbps, fps_millihz }
    }

    /// The menu's request: only the fields that differ from the stream's. `Err` is a toast.
    pub fn menu_request(&mut self, now_ms: u64) -> Result<RateChangeRequest, String> {
        let fps_millihz = (self.menu_fps * 1000.0).round() as u32;
        let bitrate = if self.menu_kbps != self.bitrate_kbps { self.menu_kbps } else { 0 };
        let fps = if fps_millihz != self.fps_millihz { fps_millihz } else { 0 };
        if bitrate == 0 && fps == 0 {
            return Err(tr("The stream already runs at this bitrate and frame rate.").to_owned());
        }
        let request = RateChangeRequest { serial: 0, bitrate_kbps: bitrate, fps_millihz: fps };
        if !request.valid() {
            return Err(tr("This stream cannot use the requested size or frame rate.").to_owned());
        }
        Ok(self.request(Origin::Menu, bitrate, fps, now_ms))
    }

    /// Once a second during the stream: the controller's request, if it wants a change.
    pub fn tick(&mut self, stats: &Stats) -> Option<RateChangeRequest> {
        // A change by hand goes first: the controller's would supersede it.
        if self.menu_busy() {
            return None;
        }
        let hint = self.hint.as_mut()?;
        if self.last_tick_ms.is_some_and(|at| stats.now_ms < at + TICK_MS) {
            return None;
        }
        self.last_tick_ms = Some(stats.now_ms);
        let kbps = hint.sample(stats)?;
        Some(self.request(Origin::Hint, kbps, 0, stats.now_ms))
    }

    /// Gives up on requests the host has not answered within [`ANSWER_MS`]: their toast. The
    /// controller stops, as for a refusal.
    pub fn expire(&mut self, now_ms: u64) -> Option<String> {
        let mut toast = None;
        while let Some(at) = self.pending.iter().position(|(_, _, sent)| now_ms >= sent + ANSWER_MS) {
            let (serial, origin, _) = self.pending.remove(at);
            eprintln!("nightjar: rate change {serial} got no answer in {ANSWER_MS} ms");
            toast = Some(match origin {
                Origin::Menu => tr("The PC did not answer the rate change.").to_owned(),
                Origin::Hint => {
                    if let Some(hint) = &mut self.hint { hint.refused(); }
                    trf("Client-hint bitrate stopped: {}", &[&tr("the PC did not answer")])
                }
            });
        }
        toast
    }

    /// `request` could not be sent: its toast. The controller stops, as for a refusal.
    pub fn send_failed(&mut self, request: &RateChangeRequest, error: &dyn std::fmt::Display) -> String {
        let origin = self.take(request.serial);
        if origin == Some(Origin::Hint) {
            if let Some(hint) = &mut self.hint { hint.refused(); }
            return trf("Client-hint bitrate stopped: {}", &[error]);
        }
        trf("Could not change the stream: {}", &[error])
    }

    fn take(&mut self, serial: u32) -> Option<Origin> {
        let at = self.pending.iter().position(|(s, _, _)| *s == serial)?;
        Some(self.pending.remove(at).1)
    }

    /// The host's answer to one request.
    pub fn result(&mut self, result: &RateChangeResult, now_ms: u64) -> Answer {
        let Some(origin) = self.take(result.serial) else {
            eprintln!("nightjar: rate change result for serial {} that is not pending, ignored", result.serial);
            return Answer::default();
        };
        let mut answer = Answer::default();
        if result.bitrate_kbps > 0 {
            self.bitrate_kbps = result.bitrate_kbps;
            self.menu_kbps = result.bitrate_kbps;
            answer.bitrate_kbps = Some(result.bitrate_kbps);
        }
        if result.fps_millihz > 0 && result.fps_millihz != self.fps_millihz {
            self.fps_millihz = result.fps_millihz;
            answer.fps_millihz = Some(result.fps_millihz);
        }
        self.menu_fps = f64::from(self.fps_millihz) / 1000.0;
        let changed = matches!(result.bitrate.status, Status::Applied | Status::Clamped);
        match origin {
            Origin::Menu => {
                if changed {
                    // A bitrate set by hand: the controller's range follows it.
                    if let Some(hint) = &mut self.hint { hint.overridden(self.bitrate_kbps, now_ms); }
                }
                answer.toast = Some(menu_text(result));
            }
            Origin::Hint => {
                let Some(hint) = &mut self.hint else { return answer };
                if changed {
                    hint.applied(self.bitrate_kbps, now_ms);
                } else if result.bitrate.reason == Reason::Superseded {
                    // A change by hand replaced it; that one's result sets the range.
                    hint.overridden(self.bitrate_kbps, now_ms);
                } else {
                    hint.refused();
                    answer.toast = Some(trf("Client-hint bitrate stopped: {}", &[&reason_text(result.bitrate.reason)]));
                }
            }
        }
        answer
    }

    /// The bitrate changed another way (by hand over HTTP): the controller's range follows.
    pub fn bitrate_set(&mut self, kbps: u32, now_ms: u64) {
        self.bitrate_kbps = kbps;
        self.menu_kbps = kbps;
        if let Some(hint) = &mut self.hint { hint.overridden(kbps, now_ms); }
    }

    /// The rate changed another way (a 0x5532 reconfigure).
    pub fn fps_set(&mut self, fps_millihz: u32) {
        self.fps_millihz = fps_millihz;
        self.menu_fps = f64::from(fps_millihz) / 1000.0;
    }
}

fn fps_text(fps_millihz: u32) -> String {
    let fps = f64::from(fps_millihz) / 1000.0;
    if fps_millihz % 1000 == 0 { format!("{fps:.0}") } else { format!("{fps:.3}") }
}

fn reason_text(reason: Reason) -> &'static str {
    match reason {
        Reason::Invalid => tr("the request was not valid"),
        Reason::Disabled => tr("the PC does not change this stream's rates"),
        Reason::Codec => tr("this stream's codec keeps its rates"),
        Reason::HostLimit => tr("the PC's limit"),
        Reason::EncoderLimit => tr("the encoder's limit"),
        Reason::EncoderFailed => tr("the encoder could not run at it"),
        Reason::Superseded => tr("a later change replaced it"),
        Reason::None | Reason::Other => tr("the PC gave no reason"),
    }
}

/// The toast for a menu request's result: what became of each value asked for.
fn menu_text(result: &RateChangeResult) -> String {
    let kbps = result.bitrate_kbps;
    let fps = fps_text(result.fps_millihz);
    let bitrate = match result.bitrate.status {
        Status::Unchanged => None,
        Status::Applied => Some(trf("Bitrate set to {} kbps.", &[&kbps])),
        Status::Clamped => Some(trf("Bitrate limited to {} kbps: {}.", &[&kbps, &reason_text(result.bitrate.reason)])),
        Status::Refused => Some(trf("Bitrate not changed: {}.", &[&reason_text(result.bitrate.reason)])),
    };
    let rate = match result.fps.status {
        Status::Unchanged => None,
        Status::Applied => Some(trf("Frame rate set to {} fps.", &[&fps])),
        Status::Clamped => Some(trf("Frame rate limited to {} fps: {}.", &[&fps, &reason_text(result.fps.reason)])),
        Status::Refused => Some(trf("Frame rate not changed: {}.", &[&reason_text(result.fps.reason)])),
    };
    match (bitrate, rate) {
        (Some(a), Some(b)) => format!("{a} {b}"),
        (Some(text), None) | (None, Some(text)) => text,
        (None, None) => tr("The PC changed nothing.").to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rubylight_protocol::rate_change::Outcome;

    fn stats(second: u64, loss: f32) -> Stats {
        Stats { now_ms: second * 1000, recent_video: true, loss_percent: Some(loss), ..Stats::default() }
    }

    fn result(request: &RateChangeRequest, bitrate_kbps: u32, fps_millihz: u32, bitrate: Outcome, fps: Outcome) -> RateChangeResult {
        RateChangeResult { serial: request.serial, bitrate_kbps, fps_millihz, bitrate, fps }
    }

    #[test]
    fn only_a_full_stream_whose_host_listed_0x553c_offers_the_menu() {
        assert!(offered(&[0x5530, 0x553C], AppRole::Stream));
        assert!(!offered(&[0x5530, 0x5532], AppRole::Stream), "a launch the host did not take");
        assert!(!offered(&[], AppRole::Stream));
        assert!(!offered(&[0x553C], AppRole::InputOnly));
        assert!(!offered(&[0x553C], AppRole::RemoteMonitor));
    }

    #[test]
    fn the_menu_asks_only_for_what_changed_and_shows_every_answer() {
        let mut rates = RateChange::new(20_000, 60_000, false, 0, 0);
        assert!(rates.menu_request(0).is_err(), "nothing to change");
        rates.menu_kbps = 40_000;
        let request = rates.menu_request(0).unwrap();
        assert_eq!((request.bitrate_kbps, request.fps_millihz), (40_000, 0));
        assert!(rates.menu_busy());
        let answer = rates.result(&result(&request, 30_000, 60_000, Outcome::clamped(Reason::HostLimit), Outcome::UNCHANGED), 1000);
        assert_eq!(answer.toast.as_deref(), Some("Bitrate limited to 30000 kbps: the PC's limit."));
        assert_eq!((answer.bitrate_kbps, answer.fps_millihz), (Some(30_000), None));
        assert!(!rates.menu_busy());
        assert_eq!(rates.menu_kbps, 30_000, "the field shows what the stream runs at");

        rates.menu_kbps = 50_000;
        rates.menu_fps = 119.88;
        let request = rates.menu_request(0).unwrap();
        assert_eq!((request.bitrate_kbps, request.fps_millihz), (50_000, 119_880));
        let answer = rates.result(&result(&request, 50_000, 119_880, Outcome::APPLIED, Outcome::APPLIED), 2000);
        assert_eq!(answer.toast.as_deref(), Some("Bitrate set to 50000 kbps. Frame rate set to 119.880 fps."));
        assert_eq!(answer.fps_millihz, Some(119_880));

        // Refusals are loud too.
        rates.menu_fps = 144.0;
        let request = rates.menu_request(0).unwrap();
        assert_eq!((request.bitrate_kbps, request.fps_millihz), (0, 144_000));
        let answer = rates.result(&result(&request, 50_000, 119_880, Outcome::UNCHANGED, Outcome::refused(Reason::Disabled)), 3000);
        assert_eq!(answer.toast.as_deref(), Some("Frame rate not changed: the PC does not change this stream's rates."));
        assert_eq!(answer.fps_millihz, None);
        assert_eq!(rates.menu_fps, 119.88, "the field goes back to the stream's rate");

        rates.menu_fps = 5.0;
        assert!(rates.menu_request(0).is_err(), "below the protocol's lowest rate");
        assert!(!rates.menu_busy());
        rates.menu_fps = 119.88;

        // A result nobody asked for changes nothing.
        let stray = RateChangeResult { serial: 99, ..result(&request, 1000, 30_000, Outcome::APPLIED, Outcome::APPLIED) };
        assert_eq!(rates.result(&stray, 4000), Answer::default());

        crate::strings::set(crate::strings::Lang::De);
        rates.menu_kbps = 10_000;
        let request = rates.menu_request(0).unwrap();
        let answer = rates.result(&result(&request, 10_000, 119_880, Outcome::APPLIED, Outcome::UNCHANGED), 5000);
        assert_eq!(answer.toast.as_deref(), Some("Bitrate auf 10000 kbit/s gesetzt."));
    }

    #[test]
    fn the_controller_ticks_once_a_second_and_one_request_at_a_time() {
        let mut off = RateChange::new(20_000, 60_000, false, 0, 0);
        assert!((0..30).all(|s| off.tick(&stats(s, 10.0)).is_none()), "off by default");

        let mut rates = RateChange::new(20_000, 60_000, true, 0, 0);
        assert!(rates.hint_active());
        let at = |ms: u64| Stats { now_ms: ms, ..stats(0, 10.0) };
        assert_eq!(rates.tick(&at(2500)), None, "start delay");
        assert_eq!(rates.tick(&at(3000)), None, "under a second since the last tick");
        let request = rates.tick(&at(3500)).unwrap();
        assert_eq!((request.bitrate_kbps, request.fps_millihz), (14_000, 0));
        assert!(!rates.menu_busy(), "the controller's request is not the menu's");
        assert_eq!(rates.tick(&stats(4, 10.0)), None, "the first is still pending");
        let answer = rates.result(&result(&request, 14_000, 60_000, Outcome::APPLIED, Outcome::UNCHANGED), 4000);
        assert_eq!(answer, Answer { bitrate_kbps: Some(14_000), ..Answer::default() }, "the controller's changes are quiet");
        assert_eq!(rates.tick(&stats(5, 10.0)), None, "cooldown");
        let request = rates.tick(&stats(6, 10.0)).unwrap();
        assert_eq!(request.bitrate_kbps, 9800);

        // A change by hand supersedes it: the controller follows instead of stopping.
        rates.menu_kbps = 30_000;
        let manual = rates.menu_request(0).unwrap();
        assert!(rates.result(&result(&request, 14_000, 60_000, Outcome::refused(Reason::Superseded), Outcome::UNCHANGED), 6100).toast.is_none());
        rates.result(&result(&manual, 30_000, 60_000, Outcome::APPLIED, Outcome::UNCHANGED), 6200);
        assert!(rates.hint_active());
        let request = rates.tick(&stats(9, 10.0)).unwrap();
        assert_eq!(request.bitrate_kbps, 21_000, "from the bitrate set by hand");

        // A refusal stops it, loudly.
        let answer = rates.result(&result(&request, 30_000, 60_000, Outcome::refused(Reason::Codec), Outcome::UNCHANGED), 10_000);
        assert_eq!(answer.toast.as_deref(), Some("Client-hint bitrate stopped: this stream's codec keeps its rates"));
        assert!(!rates.hint_active() && !rates.hint_has_room());
        assert!((11..60).all(|s| rates.tick(&stats(s, 10.0)).is_none()));
    }

    #[test]
    fn the_controller_waits_for_a_change_by_hand() {
        let mut rates = RateChange::new(20_000, 60_000, true, 0, 0);
        rates.menu_kbps = 30_000;
        let manual = rates.menu_request(0).unwrap();
        assert!((3..10).all(|s| rates.tick(&stats(s, 10.0)).is_none()), "the menu's request is pending");
        rates.result(&result(&manual, 30_000, 60_000, Outcome::APPLIED, Outcome::UNCHANGED), 10_000);
        assert_eq!(rates.tick(&stats(12, 10.0)).map(|r| r.bitrate_kbps), Some(21_000));
    }

    #[test]
    fn a_request_that_cannot_be_sent_is_loud_and_stops_the_controller() {
        let mut rates = RateChange::new(20_000, 60_000, true, 0, 0);
        let request = rates.tick(&stats(3, 10.0)).unwrap();
        assert_eq!(rates.send_failed(&request, &"closed"), "Client-hint bitrate stopped: closed");
        assert!(!rates.hint_active());
        rates.menu_kbps = 30_000;
        let request = rates.menu_request(0).unwrap();
        assert_eq!(rates.send_failed(&request, &"closed"), "Could not change the stream: closed");
        assert!(!rates.menu_busy());
    }

    #[test]
    fn a_request_the_host_never_answers_is_given_up_on_loudly() {
        let mut rates = RateChange::new(20_000, 60_000, true, 0, 0);
        rates.menu_kbps = 30_000;
        let manual = rates.menu_request(1000).unwrap();
        assert_eq!(rates.expire(5999), None);
        assert_eq!(rates.expire(6000).as_deref(), Some("The PC did not answer the rate change."));
        assert!(!rates.menu_busy(), "the menu takes a new request");
        assert_eq!(rates.result(&result(&manual, 30_000, 60_000, Outcome::APPLIED, Outcome::UNCHANGED), 7000), Answer::default(), "too late");
        let request = rates.tick(&stats(8, 10.0)).unwrap();
        assert_eq!(rates.expire(12_999), None);
        assert_eq!(rates.expire(13_000).as_deref(), Some("Client-hint bitrate stopped: the PC did not answer"));
        assert!(!rates.hint_active());
        assert_eq!(rates.result(&result(&request, 14_000, 60_000, Outcome::APPLIED, Outcome::UNCHANGED), 13_100), Answer::default());
    }
}
