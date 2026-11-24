//! Mid-stream bitrate and frame-rate changes over 0x553C, on a stream whose DESCRIBE lists
//! them: the stream menu's requests, the client-hint bitrate controller's
//! (Lower bitrate's Lower it sooner, ticked once a second), and what the host answered.
//! Time is session-relative milliseconds.

use nightjar_core::http::AppRole;
use nightjar_policy::choices::LowerBitrate;
use nightjar_policy::client_hint;
use nightjar_policy::stats::Stats;
use rubylight_protocol::rate_change::{Reason, RateChangeRequest, RateChangeResult, Status, RATE_CHANGE_MESSAGE_TYPE};

use crate::strings::{tr, trf};

/// Time between two controller ticks.
pub const TICK_MS: u64 = 1000;
/// A request the host has not answered in this long is given up on, loudly.
pub const ANSWER_MS: u64 = 5000;

/// Whether the launch asks for rate changes (`rlRateChange=1`): full streams only.
pub fn asked_at_launch(role: AppRole) -> bool {
    role == AppRole::Stream
}

/// Whether the stream takes rate changes: the host listed 0x553C, which it does only for a
/// launch with `rlRateChange=1`, and the stream is a full one.
pub fn offered(host_control_messages: &[u16], role: AppRole) -> bool {
    asked_at_launch(role) && host_control_messages.contains(&RATE_CHANGE_MESSAGE_TYPE)
}

/// The stream menu's bitrate and rate controls. On a stream that takes 0x553C, "Change bitrate /
/// frame rate" replaces the HTTP bitrate row and the size editor's frame rate, so each shows once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MenuControls {
    /// "Change bitrate / frame rate".
    pub rate_change: bool,
    /// The bitrate field and "Apply bitrate" over HTTP.
    pub http_bitrate: bool,
    /// The 0x5532 size editor.
    pub size_editor: bool,
    /// The size editor's frame rate field.
    pub size_editor_fps: bool,
}

pub fn menu_controls(runtime_bitrate: bool, reconfigure: bool, rate_change: bool) -> MenuControls {
    MenuControls {
        rate_change,
        http_bitrate: runtime_bitrate && !rate_change,
        size_editor: reconfigure,
        size_editor_fps: reconfigure && !rate_change,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Origin {
    Menu,
    Hint,
    /// "Never: keep my bitrate": the stream's own bitrate, sent once so the PC stops lowering it.
    Keep,
    /// The query for the stream's bitrate as the PC counts it ([`RateChange::query_request`]).
    Query,
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
    /// The bitrate is one the player set in the menu, which a reconnect keeps.
    pub by_hand: bool,
}

pub struct RateChange {
    next_serial: u32,
    /// Requests waiting for their result, with when they were sent.
    pending: Vec<(u32, Origin, u64)>,
    /// The serial of the newest request whose result set the bitrate; 0 for none yet.
    bitrate_serial: u32,
    hint: Option<client_hint::Bitrate>,
    last_tick_ms: Option<u64>,
    /// "Never: keep my bitrate" is chosen and the takeover has not been sent yet.
    keep_pending: bool,
    /// The PC keeps this stream's bitrate: the takeover, the menu's Keep my bitrate or a bitrate
    /// the player chose in the menu went out (any bitrate change makes the PC stand back).
    kept: bool,
    /// The bitrate the player last chose in this stream's menu, which Keep my bitrate keeps.
    chosen_kbps: Option<u32>,
    /// The stream's bitrate at its start, which Keep my bitrate keeps when none was chosen: the
    /// PC's answer to the query once it came.
    configured_kbps: u32,
    /// The query is out and unanswered: the takeover and the client-hint controller wait for it.
    asking: bool,
    /// The query could not be sent at the start: when it goes once more.
    query_again_at: Option<u64>,
    query_retried: bool,
    /// The PC said it lowered the bitrate before the answer came ([`Self::pc_lowered`]).
    cut_before_answer: bool,
    /// The client-hint controller's cap and the cut a reconnect returns it to, for its start from
    /// the PC's answer.
    limit_kbps: u32,
    resume_kbps: Option<u32>,
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
            bitrate_serial: 0,
            hint: hint.then(|| client_hint::Bitrate::new(bitrate_kbps, limit_kbps, now_ms)),
            last_tick_ms: None,
            keep_pending: false,
            kept: false,
            chosen_kbps: None,
            configured_kbps: bitrate_kbps,
            asking: false,
            query_again_at: None,
            query_retried: false,
            cut_before_answer: false,
            limit_kbps,
            resume_kbps: None,
            bitrate_kbps,
            fps_millihz,
            menu_kbps: bitrate_kbps,
            menu_fps: f64::from(fps_millihz) / 1000.0,
        }
    }

    /// "Never: keep my bitrate": [`Self::takeover_request`] sends the stream's bitrate once.
    pub fn keep_bitrate(&mut self) {
        self.keep_pending = true;
    }

    /// A reconnect after the player chose a bitrate in the menu: Keep my bitrate keeps that one,
    /// which came from this PC's own answers, whatever the query answers.
    pub fn chosen_before(&mut self, kbps: u32) {
        self.chosen_kbps = Some(kbps);
    }

    /// The query (bitrate and rate 0), at the stream's start before anything that sets a
    /// bitrate: every PC that takes 0x553C answers it without changing anything, with the
    /// stream's bitrate as it counts it (Rubylight 6.0.1 and older in the encoder's unit, newer
    /// ones in the launch's). Keep my bitrate and the client-hint controller wait for the answer
    /// and start from it; without one in [`ANSWER_MS`] they go on as before.
    pub fn query_request(&mut self, now_ms: u64) -> RateChangeRequest {
        self.asking = true;
        eprintln!("nightjar: rate change: asking the PC what it counts this stream's bitrate as");
        self.request(Origin::Query, 0, 0, now_ms)
    }

    /// The query could not be sent: once more a second later ([`Self::query_again`]), then the
    /// stream goes on without it. Quietly, as the stream did before it asked.
    pub fn query_unsent(&mut self, request: &RateChangeRequest, error: &dyn std::fmt::Display, now_ms: u64) {
        self.take(request.serial);
        if !std::mem::replace(&mut self.query_retried, true) {
            eprintln!("nightjar: rate change: the bitrate query could not be sent yet ({error}); trying again in a second");
            self.query_again_at = Some(now_ms + TICK_MS);
        } else {
            eprintln!("nightjar: rate change: the bitrate query could not be sent ({error}); the stream goes on from {} kbps", self.configured_kbps);
            self.asking = false;
        }
    }

    /// The PC said it lowered the bitrate for the network (a 0x5538 notice, limited or floor):
    /// an answer to the query that comes after it may hold that cut, so it is not used.
    pub fn pc_lowered(&mut self) {
        if self.asking { self.cut_before_answer = true; }
    }

    /// While the query waits, the menu's bitrate cannot change the stream's: a number picked now
    /// would be in the setting's unit, which an older PC takes as its encoder's. The menu shows the
    /// control disabled until the answer, its timeout, or an answer that is not used.
    pub fn bitrate_locked(&self) -> bool {
        self.asking
    }

    /// The stream menu's change, as `Streaming::change_rates` sends it.
    pub fn menu_change(&mut self, now_ms: u64) -> Result<RateChangeRequest, String> {
        self.menu_request(now_ms)
    }
    /// The query once more, a second after it could not be sent.
    pub fn query_again(&mut self, now_ms: u64) -> Option<RateChangeRequest> {
        if !self.query_again_at.is_some_and(|at| now_ms >= at) { return None; }
        self.query_again_at = None;
        Some(self.request(Origin::Query, 0, 0, now_ms))
    }

    /// The one request that makes the PC's link-rate and congestion controllers stand back for
    /// this stream: the stream's own bitrate as a bitrate change (the PC stops lowering what a
    /// client has set). `None` once sent, while the query waits for its answer, or when "Never"
    /// is not chosen.
    pub fn takeover_request(&mut self, now_ms: u64) -> Option<RateChangeRequest> {
        if self.asking || !std::mem::take(&mut self.keep_pending) { return None; }
        self.kept = true;
        let kbps = self.keep_kbps();
        eprintln!("nightjar: keep my bitrate: sending the stream's {kbps} kbps as a bitrate change so the PC stops lowering it");
        Some(self.request(Origin::Keep, kbps, 0, now_ms))
    }

    /// The stream's tick: the takeover goes out with the first tick that saw video.
    pub fn takeover_on_tick(&mut self, recent_video: bool, now_ms: u64) -> Option<RateChangeRequest> {
        if !recent_video { return None; }
        self.takeover_request(now_ms)
    }

    /// What the stream menu's Keep my bitrate shows: the PC keeps this stream's bitrate (or will,
    /// once the takeover goes out) and no controller of this device steers it.
    pub fn kept(&self) -> bool {
        (self.kept || self.keep_pending) && !self.hint_active()
    }

    /// The stream menu's Keep my bitrate turned on: the bitrate the player last chose in this
    /// stream, else the stream's own, as one bitrate change, and this device's own lowering
    /// stops. `None` when the PC already keeps it, so nothing more goes out.
    pub fn keep_now(&mut self, now_ms: u64) -> Option<RateChangeRequest> {
        let steering = self.hint_active();
        self.hint = None;
        if !steering && (self.kept || self.keep_pending) { return None; }
        if self.asking {
            // From the stream's bitrate as the PC counts it: the takeover goes out after its answer.
            eprintln!("nightjar: keep my bitrate (stream menu): goes out once the PC answered the bitrate query");
            self.keep_pending = true;
            return None;
        }
        self.keep_pending = false;
        self.kept = true;
        let kbps = self.keep_kbps();
        eprintln!("nightjar: keep my bitrate (stream menu): sending {kbps} kbps as a bitrate change so the PC stops lowering it");
        Some(self.request(Origin::Keep, kbps, 0, now_ms))
    }

    /// The bitrate Keep my bitrate keeps, at the start and from the switch alike: the one the player
    /// last chose in this stream, else the one the stream started with (negotiated).
    fn keep_kbps(&self) -> u32 {
        self.chosen_kbps.unwrap_or(self.configured_kbps)
    }

    /// The stream menu's Keep my bitrate turned off: the switch shows off and nothing is sent.
    /// The PC has no message that gives it control back, so this stream keeps its bitrate.
    pub fn release(&mut self) {
        self.kept = false;
        self.keep_pending = false;
    }

    /// After an automatic reconnect: the client-hint controller returns to the rate the stream
    /// before had cut to.
    pub fn resume_hint(&mut self, kbps: u32) {
        self.resume_kbps = Some(kbps);
        if let Some(hint) = &mut self.hint { hint.resume_from(kbps); }
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
        if self.asking && self.menu_kbps != self.bitrate_kbps {
            eprintln!("nightjar: rate change: {} kbps not sent: the PC has not said yet what it counts this stream's bitrate as", self.menu_kbps);
            self.menu_kbps = self.bitrate_kbps;
        }
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
        // A change by hand goes first: the controller's would supersede it. Nor before the PC's
        // answer to the query: the controller starts from it.
        if self.menu_busy() || self.asking {
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
                    trf("Loss-based bitrate stopped: {}", &[&tr("the PC did not answer")])
                }
                Origin::Keep => {
                    eprintln!("nightjar: keep my bitrate: the PC did not answer; it may still lower the bitrate");
                    self.kept = false;
                    continue;
                }
                Origin::Query => {
                    // Quietly, as before the query: the takeover and the controller start from the stream's own.
                    eprintln!("nightjar: rate change: the PC did not answer the bitrate query; the stream goes on from {} kbps", self.configured_kbps);
                    self.asking = false;
                    continue;
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
            return trf("Loss-based bitrate stopped: {}", &[error]);
        }
        if origin == Some(Origin::Keep) {
            eprintln!("nightjar: keep my bitrate: the change could not be sent ({error}); the PC may still lower the bitrate");
            self.kept = false;
            return trf("Could not change the stream: {}", &[error]);
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
        if origin == Origin::Query && (self.query_retried || self.cut_before_answer) {
            // Video ran before the PC handled it (a retried query), or the PC said it lowered the
            // bitrate first: the answer may be a cut, which would be kept for the whole stream.
            eprintln!("nightjar: rate change: the PC's answer to the bitrate query ({} kbps) may hold a cut it made {}; the stream goes on from {} kbps",
                result.bitrate_kbps, if self.cut_before_answer { "before it" } else { "while the query waited to go out" }, self.configured_kbps);
            self.asking = false;
            self.query_again_at = None;
            return Answer::default();
        }
        let mut answer = Answer::default();
        let before_kbps = self.bitrate_kbps;
        // The host answers a request with a rate once its switch ran, with the bitrate it set
        // then, so a bitrate-only request sent after it is answered first: an older result keeps
        // the newer bitrate.
        let newest = self.bitrate_serial == 0 || (result.serial.wrapping_sub(self.bitrate_serial) as i32) > 0;
        if result.bitrate_kbps > 0 && newest {
            self.bitrate_serial = result.serial;
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
                if changed && answer.bitrate_kbps.is_some() {
                    // A bitrate set by hand: the controller's range follows it.
                    if let Some(hint) = &mut self.hint { hint.overridden(self.bitrate_kbps, now_ms); }
                    answer.by_hand = true;
                    // The PC stands back for a bitrate the player chose, as for the takeover.
                    self.chosen_kbps = Some(self.bitrate_kbps);
                    self.kept = true;
                }
                answer.toast = Some(menu_text(result));
            }
            Origin::Keep => {
                eprintln!("nightjar: keep my bitrate: the PC answered {:?} at {} kbps{}", result.bitrate.status, result.bitrate_kbps,
                    if changed { "; it no longer lowers the bitrate" } else { "; it may still lower the bitrate" });
                // Refused: the PC still lowers the bitrate, so the switch shows off and the notice may say so.
                if !changed { self.kept = false; }
            }
            Origin::Query => {
                self.asking = false;
                self.query_again_at = None;
                if result.bitrate_kbps > 0 {
                    eprintln!("nightjar: rate change: the PC counts this stream's bitrate as {} kbps; keep my bitrate and the client-hint controller start from it", result.bitrate_kbps);
                    self.configured_kbps = result.bitrate_kbps;
                    // Unless a change by hand was answered first: the controller follows that one.
                    if answer.bitrate_kbps.is_some()
                        && let Some(hint) = &mut self.hint {
                            *hint = client_hint::Bitrate::new(result.bitrate_kbps, self.limit_kbps, now_ms);
                            if let Some(kbps) = self.resume_kbps { hint.resume_from(kbps); }
                        }
                } else {
                    eprintln!("nightjar: rate change: the PC's answer to the bitrate query named no bitrate; the stream goes on from {} kbps", self.configured_kbps);
                }
                // It changed nothing: the stream follows only a count that differs from its own.
                if answer.bitrate_kbps == Some(before_kbps) { answer.bitrate_kbps = None; }
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
                    answer.toast = Some(trf("Loss-based bitrate stopped: {}", &[&reason_text(result.bitrate.reason)]));
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

/// Whether the stream starts a bitrate controller of this device (the busy-network one, also for
/// automatic quality): never under Never or after a bitrate the player chose.
pub fn own_lowering_at_start(manual_bitrate: bool, keep_bitrate: bool, adaptive_bitrate: bool, automatic_quality: bool) -> bool {
    !manual_bitrate && !keep_bitrate && (adaptive_bitrate || automatic_quality)
}

/// Whether a stream sends the takeover once its video runs: Never is chosen, or a reconnect starts
/// at a bitrate the player chose (the PC's new session would lower it again) with no controller of
/// this device steering.
pub fn takeover_at_start(keep_bitrate: bool, player_chose_bitrate: bool, hint: bool) -> bool {
    keep_bitrate || (player_chose_bitrate && !hint)
}

/// What the stream menu's Keep my bitrate shows; `None` hides it, on a PC that did not take this
/// stream's rate changes. Off while this device's busy-network controller steers the bitrate.
pub fn keep_switch(rates: Option<&RateChange>, adaptive_bitrate: bool) -> Option<bool> {
    rates.map(|rates| rates.kept() && !adaptive_bitrate)
}

/// Keep my bitrate as the main settings keep it: Lower bitrate's Never when on, The PC decides
/// when off, stored as choosing them in Settings stores them.
pub fn store_keep(settings: &mut crate::config::Settings, keep: bool) {
    settings.lower_bitrate = if keep { LowerBitrate::Keep } else { LowerBitrate::Off };
}

fn fps_text(fps_millihz: u32) -> String {
    let fps = f64::from(fps_millihz) / 1000.0;
    if fps_millihz.is_multiple_of(1000) { format!("{fps:.0}") } else { format!("{fps:.3}") }
}

fn reason_text(reason: Reason) -> &'static str {
    match reason {
        Reason::Invalid => tr("the request was not valid"),
        Reason::Disabled => tr("the PC does not change this stream’s rates"),
        Reason::Codec => tr("this stream’s codec keeps its rates"),
        Reason::HostLimit => tr("the PC’s limit"),
        Reason::EncoderLimit => tr("the encoder’s limit"),
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
mod tests;
