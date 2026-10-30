//! Stream menu controls ported from Moonlight V+ PC (qiin2333/moonlight-qt,
//! `app/streaming/video/overlaymenupanel.cpp` and `app/streaming/input/gamepad.cpp`): the
//! controller mouse switch and the bitrate slider with its piecewise scale. Also the host's
//! night mode and headphone surround switches (Rubylight's native API `/audio`).

use std::sync::Arc;
use std::time::{Duration, Instant};

use nightjar_core::http::native_api::audio::{reason, Audio, AudioWrite, HEADPHONE_SURROUND, NIGHT_MODE};
use nightjar_core::http::native_api::capabilities::Capabilities;
use nightjar_core::http::native_api::radeon::{Change, ControlState, Value};
use nightjar_core::http::{AppRole, Feature, HostClient};

use crate::gamepad::Gamepads;
use crate::strings::{tr, trf};

use super::pipeline::{lock, Notice};
use super::Streaming;

impl Streaming {
    /// The controller mouse switch shows while controllers reach the PC and one is connected.
    pub fn controller_mouse_offered(&self, pads: &Gamepads) -> bool {
        self.sends_controllers() && !pads.pads.is_empty()
    }

    /// Turns the controller mouse on for the first controller, or off for all, from the menu. It
    /// works with holding Start switched off in the settings.
    pub fn toggle_controller_mouse(&mut self, pads: &mut Gamepads) {
        let mut active = None;
        self.session.input(|q| active = pads.toggle_mouse(q));
        let Some(active) = active else { return };
        self.controller_mouse_active = active;
        self.show_toast(mouse_toast(pads, active, self.settings.controller_mouse));
    }
}

/// What turning the controller mouse on or off says, in the connected controller's button names;
/// `hold_start` when holding Start turns it off again.
pub fn mouse_toast(pads: &Gamepads, active: bool, hold_start: bool) -> String {
    let style = crate::glyphs::Style::of(pads);
    let (left, right) = style.mouse_clicks(pads.swap_face_buttons);
    match (active, hold_start) {
        (true, true) => trf("Controller mouse enabled. {}/{} click; D-pad scrolls. Hold {} to return to gamepad.", &[&left, &right, &style.start()]),
        (true, false) => trf("Controller mouse enabled. {}/{} click; D-pad scrolls.", &[&left, &right]),
        (false, _) => tr("Controller mouse disabled.").to_owned(),
    }
}

/// The host's night mode and headphone surround (`GET /audio`): read each time the stream controls
/// open, so they show the PC's state, and changed live for this stream with `POST /audio/settings`.
#[derive(Debug, Default)]
pub struct HostAudio {
    /// The switches the PC offers this device, from its native capabilities; None until read.
    offered: Option<Vec<&'static str>>,
    audio: Option<Audio>,
    /// The PC offers switches but didn't say their state: the menu says so instead of showing none.
    unread: bool,
    /// A read or a change is on its way.
    busy: bool,
}

/// What came back from the PC for [`HostAudio`].
pub enum AudioAnswer {
    /// The offered switches (None when the capabilities weren't read) and, when there are any,
    /// their state.
    Read { offered: Option<Vec<&'static str>>, audio: Result<Option<Audio>, String> },
    Changed(Result<Audio, String>),
}

/// One switch as the stream controls show it.
#[derive(Debug, PartialEq)]
pub struct Switch {
    pub id: &'static str,
    pub on: bool,
    pub enabled: bool,
    /// Why this stream doesn't apply it now.
    pub note: Option<String>,
}

/// The switches the PC lists that this device may change: the host wants View or Launch.
pub fn offered_switches(capabilities: &Capabilities) -> Vec<&'static str> {
    let allowed = capabilities.permissions.view || capabilities.permissions.launch;
    [NIGHT_MODE, HEADPHONE_SURROUND]
        .into_iter()
        .filter(|id| allowed && capabilities.features.iter().any(|f| f == id))
        .collect()
}

pub fn audio_label(id: &str) -> &'static str {
    if id == NIGHT_MODE { tr("Night mode") } else { tr("Headphone surround") }
}

/// The request that turns one switch on or off.
pub fn audio_write(id: &str, on: bool) -> AudioWrite {
    AudioWrite { changes: vec![Change { id: id.into(), value: Value::Bool(on) }] }
}

impl HostAudio {
    /// The offered switches the PC's answer has, in the PC's state.
    pub fn switches(&self) -> Vec<Switch> {
        let (Some(offered), Some(audio)) = (&self.offered, &self.audio) else { return Vec::new() };
        offered
            .iter()
            .filter_map(|&id| {
                let setting = audio.settings.iter().find(|s| s.id == id)?;
                Some(Switch {
                    id,
                    on: setting.value == Some(Value::Bool(true)),
                    enabled: setting.state == ControlState::Configurable && !self.busy,
                    note: match setting.reason.as_deref() {
                        None => None,
                        Some(reason::NEXT_STREAM) => Some(tr("Applies next stream").to_owned()),
                        Some(_) => setting.message.clone(),
                    },
                })
            })
            .collect()
    }

    /// The line the menu shows when the PC offers switches but their state couldn't be read.
    pub fn problem(&self) -> Option<&'static str> {
        self.unread.then(|| tr("The PC's audio settings could not be read"))
    }

    /// Takes the PC's answer; a failed change is the text to show.
    pub fn answered(&mut self, answer: AudioAnswer) -> Result<(), String> {
        self.busy = false;
        match answer {
            AudioAnswer::Read { offered, audio } => {
                if offered.is_some() {
                    self.offered = offered;
                }
                match audio {
                    Ok(audio) => {
                        self.audio = audio;
                        self.unread = false;
                    }
                    // Nothing stale stays on screen: the switches go until the next read, and the
                    // menu says why when the PC offers them.
                    Err(error) => {
                        eprintln!("nightjar: the PC's audio settings were not read: {error}");
                        self.audio = None;
                        self.unread = self.offered.as_ref().is_some_and(|offered| !offered.is_empty());
                    }
                }
            }
            AudioAnswer::Changed(Ok(audio)) => self.audio = Some(audio),
            AudioAnswer::Changed(Err(error)) => return Err(trf("Could not change the PC's audio: {}", &[&error])),
        }
        Ok(())
    }
}

/// The switches the PC offers, read once per stream, and their state now.
fn read_host_audio(client: &HostClient, offered: Option<Vec<&'static str>>) -> AudioAnswer {
    let offered = match offered {
        Some(offered) => offered,
        None => match client.native_capabilities() {
            Ok(capabilities) => offered_switches(&capabilities),
            Err(e) => return AudioAnswer::Read { offered: None, audio: Err(e.to_string()) },
        },
    };
    let audio = if offered.is_empty() { Ok(None) } else { client.audio().map(Some).map_err(|e| e.to_string()) };
    AudioAnswer::Read { offered: Some(offered), audio }
}

impl Streaming {
    /// The PC's client for the native API, on a stream that may use it.
    fn audio_client(&self) -> Option<Arc<HostClient>> {
        if self.facts.role != AppRole::Stream || !self.plan.info.capabilities.has(Feature::NativeApi) {
            return None;
        }
        lock(&self.shared.host_client).clone()
    }

    /// Reads the switches as the stream controls open, on a thread. A PC that offers none is not
    /// asked again in this stream.
    pub fn refresh_host_audio(&mut self) {
        if self.host_audio.busy || self.host_audio.offered.as_ref().is_some_and(Vec::is_empty) { return; }
        let Some(client) = self.audio_client() else { return };
        let (offered, shared) = (self.host_audio.offered.clone(), self.shared.clone());
        self.host_audio.busy = true;
        std::thread::spawn(move || shared.notify(Notice::HostAudio(read_host_audio(&client, offered))));
    }

    /// Turns a switch on or off for this stream; the PC's answer sets what the menu shows.
    pub fn set_host_audio(&mut self, id: &'static str, on: bool) {
        if self.host_audio.busy { return; }
        let Some(client) = self.audio_client() else { return };
        let shared = self.shared.clone();
        self.host_audio.busy = true;
        std::thread::spawn(move || {
            let answer = client.write_audio(&audio_write(id, on)).map_err(|e| e.to_string());
            shared.notify(Notice::HostAudio(AudioAnswer::Changed(answer)));
        });
    }
}

const MIN_KBPS: u32 = 500;
const MAX_KBPS: u32 = 800_000;
/// Each segment's top and step, in kbps: fine steps where a few Mbps matter, coarse ones above.
const SEGMENTS: [(u32, u32); 7] = [(5_000, 500), (20_000, 1_000), (50_000, 2_000), (100_000, 5_000), (200_000, 10_000), (400_000, 50_000), (800_000, 100_000)];
/// The idle time after the last keyboard or gamepad step before the bitrate is sent.
const COMMIT_DELAY: Duration = Duration::from_millis(450);

fn positions() -> u32 {
    let mut from = MIN_KBPS;
    SEGMENTS.iter().map(|&(top, step)| { let steps = (top - from) / step; from = top; steps }).sum()
}

/// The slider position of a bitrate; values off the steps fall between them.
pub fn position(kbps: u32) -> f64 {
    let kbps = kbps.clamp(MIN_KBPS, MAX_KBPS);
    let (mut at, mut from) = (0.0, MIN_KBPS);
    for (top, step) in SEGMENTS {
        if kbps <= top { return at + f64::from(kbps - from) / f64::from(step); }
        at += f64::from((top - from) / step);
        from = top;
    }
    at
}

/// The bitrate at a slider position: a step at a whole position (the slider's own), in between
/// for a typed value off the steps.
pub fn kbps(position: f64) -> u32 {
    let position = position.clamp(0.0, f64::from(positions()));
    let (mut start, mut from) = (0.0, MIN_KBPS);
    for (top, step) in SEGMENTS {
        let steps = f64::from((top - from) / step);
        if position <= start + steps { return from + ((position - start) * f64::from(step)).round() as u32; }
        start += steps;
        from = top;
    }
    MAX_KBPS
}

fn label(kbps: u32) -> String {
    if kbps < 1000 { return format!("{kbps} kbps"); }
    let mbps = f64::from(kbps) / 1000.0;
    if kbps % 1000 == 0 { format!("{mbps:.0} Mbps") } else { format!("{mbps:.1} Mbps") }
}

/// Typed text: a number in Mbps, or with a kbps or Mbps suffix.
fn parse(text: &str) -> Option<u32> {
    let text = text.trim().to_lowercase().replace(',', ".");
    let (number, scale) = if let Some(n) = text.strip_suffix("kbps") { (n, 1.0) } else { (text.strip_suffix("mbps").unwrap_or(&text), 1000.0) };
    let value = number.trim().parse::<f64>().ok().filter(|v| v.is_finite() && *v > 0.0)? * scale;
    Some((value.round() as u32).clamp(MIN_KBPS, MAX_KBPS))
}

/// When the slider's bitrate goes out: on release after a drag at once, after keyboard or
/// gamepad steps once they stop for [`COMMIT_DELAY`].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Scrub {
    changed_at: Option<Instant>,
}

impl Scrub {
    /// Whether to send now.
    fn step(&mut self, changed: bool, dragging: bool, released: bool, now: Instant) -> bool {
        if changed { self.changed_at = Some(now); }
        let due = released || (!dragging && self.changed_at.is_some_and(|at| now.duration_since(at) >= COMMIT_DELAY));
        if due && self.changed_at.take().is_some() { return true; }
        false
    }
}

/// The bitrate slider. `value` (kbps) follows it; true when the new value should be sent.
pub fn bitrate_slider(ui: &mut egui::Ui, value: &mut u32, enabled: bool) -> bool {
    let mut at = position(*value);
    let before = at;
    let response = ui.horizontal(|ui| {
        ui.label(tr("Bitrate"));
        ui.add_enabled(enabled, egui::Slider::new(&mut at, 0.0..=f64::from(positions()))
            .step_by(1.0)
            .custom_formatter(|at, _| label(kbps(at)))
            .custom_parser(|text| parse(text).map(position)))
    }).inner;
    if at != before { *value = kbps(at); }
    let mut scrub = ui.data(|d| d.get_temp::<Scrub>(scrub_id())).unwrap_or_default();
    let send = scrub.step(at != before, response.dragged(), response.drag_stopped(), Instant::now());
    if scrub.changed_at.is_some() { ui.ctx().request_repaint_after(COMMIT_DELAY); }
    ui.data_mut(|d| d.insert_temp(scrub_id(), scrub));
    send && enabled
}

/// One slider is shown at a time (HTTP or 0x553C), so its wait is kept under one id that
/// [`Streaming::settle_bitrate_slider`] finds after the menu closed.
fn scrub_id() -> egui::Id {
    egui::Id::unique("bitrate-slider-scrub")
}

/// Whether a slider change was still waiting for [`COMMIT_DELAY`]; it no longer waits after this.
fn take_pending(ctx: &egui::Context) -> bool {
    ctx.data_mut(|d| d.remove_temp::<Scrub>(scrub_id())).is_some_and(|scrub| scrub.changed_at.is_some())
}

impl Streaming {
    /// For a frame without the menu: a slider change still waiting when the menu closed goes out
    /// now, instead of staying unsent while the menu shows it and leaving when the menu next opens.
    pub fn settle_bitrate_slider(&mut self, ctx: &egui::Context) {
        if !take_pending(ctx) { return; }
        let controls = self.menu_controls();
        if let Some(rates) = self.rate_change.as_ref().filter(|_| controls.rate_change) {
            if rates.menu_busy() {
                eprintln!("nightjar: bitrate slider: the PC is still answering, {} kbps not sent", rates.menu_kbps);
            } else {
                self.change_rates();
            }
        } else if self.facts.runtime_bitrate && controls.http_bitrate {
            if self.adaptive_bitrate || self.bitrate_busy {
                eprintln!("nightjar: bitrate slider: automatic bitrate or a change in flight, {} kbps not sent", self.bitrate_kbps);
            } else {
                self.set_bitrate();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capabilities(features: &[&str], view: bool, launch: bool) -> Capabilities {
        serde_json::from_value(serde_json::json!({
            "api_version": 1,
            "host": { "name": "PC", "version": "3.0.0", "unique_id": "1" },
            "features": features,
            "permissions": { "list": true, "view": view, "launch": launch, "host_control": false }
        }))
        .unwrap()
    }

    /// The host's `GET /audio` with night mode on and headphone surround waiting for the next stream.
    fn audio() -> Audio {
        serde_json::from_value(serde_json::json!({
            "revision": 3,
            "settings": [
                { "id": "audio.night_mode", "group": "sound", "kind": "toggle", "state": "configurable", "value": true },
                { "id": "audio.headphone_surround", "group": "sound", "kind": "toggle", "state": "configurable", "value": false,
                  "reason": "next_stream", "message": "Headphone surround applies from the next stream." }
            ]
        }))
        .unwrap()
    }

    #[test]
    fn the_audio_switches_show_only_what_the_pc_offers_and_allows() {
        let both = ["library", "audio.night_mode", "audio.headphone_surround"];
        assert_eq!(offered_switches(&capabilities(&both, true, false)), [NIGHT_MODE, HEADPHONE_SURROUND]);
        assert_eq!(offered_switches(&capabilities(&both, false, true)), [NIGHT_MODE, HEADPHONE_SURROUND], "Launch is enough");
        assert!(offered_switches(&capabilities(&both, false, false)).is_empty(), "the host refuses List alone");
        assert_eq!(offered_switches(&capabilities(&["audio.headphone_surround"], true, true)), [HEADPHONE_SURROUND]);
        assert!(offered_switches(&capabilities(&["library", "session.couch_menu"], true, true)).is_empty(), "an older Rubylight");

        let mut menu = HostAudio::default();
        assert!(menu.switches().is_empty(), "nothing before the PC answered");
        menu.answered(read(Some(vec![NIGHT_MODE, HEADPHONE_SURROUND]), Ok(Some(audio())))).unwrap();
        assert_eq!(menu.problem(), None);
        assert_eq!(menu.switches(), [
            Switch { id: NIGHT_MODE, on: true, enabled: true, note: None },
            Switch { id: HEADPHONE_SURROUND, on: false, enabled: true, note: Some("Applies next stream".into()) },
        ]);
        // Only what the capabilities offer, even when the answer has more.
        menu.answered(read(Some(vec![NIGHT_MODE]), Ok(Some(audio())))).unwrap();
        assert_eq!(menu.switches().iter().map(|s| s.id).collect::<Vec<_>>(), [NIGHT_MODE]);
        // A stream that has ended is unavailable: shown, not changeable.
        let mut ended = audio();
        ended.settings[0].state = ControlState::TemporarilyUnavailable;
        ended.settings[0].value = None;
        menu.answered(AudioAnswer::Changed(Ok(ended))).unwrap();
        assert_eq!(menu.switches()[0], Switch { id: NIGHT_MODE, on: false, enabled: false, note: None });
        // A failed change says so.
        assert!(menu.answered(AudioAnswer::Changed(Err("refused".into()))).unwrap_err().contains("refused"));
    }

    fn read(offered: Option<Vec<&'static str>>, audio: Result<Option<Audio>, String>) -> AudioAnswer {
        AudioAnswer::Read { offered, audio }
    }

    #[test]
    fn a_failed_read_says_so_in_the_menu_instead_of_showing_nothing() {
        let mut menu = HostAudio::default();
        menu.answered(read(Some(vec![NIGHT_MODE, HEADPHONE_SURROUND]), Ok(Some(audio())))).unwrap();
        // The switches go, nothing stale stays, and the menu says why.
        menu.answered(read(Some(vec![NIGHT_MODE, HEADPHONE_SURROUND]), Err("timed out".into()))).unwrap();
        assert!(menu.switches().is_empty());
        assert_eq!(menu.problem(), Some("The PC's audio settings could not be read"));
        // The next read that works takes the line away.
        menu.answered(read(Some(vec![NIGHT_MODE, HEADPHONE_SURROUND]), Ok(Some(audio())))).unwrap();
        assert_eq!(menu.problem(), None);
        assert_eq!(menu.switches().len(), 2);
        // The offered switches stay known when only the capabilities failed this time.
        menu.answered(read(None, Err("refused".into()))).unwrap();
        assert!(menu.problem().is_some());

        // A PC that offers no switches, or whose capabilities couldn't be read at all, gets no line.
        let mut none = HostAudio::default();
        none.answered(read(Some(Vec::new()), Ok(None))).unwrap();
        assert_eq!(none.problem(), None);
        let mut unknown = HostAudio::default();
        unknown.answered(read(None, Err("refused".into()))).unwrap();
        assert_eq!(unknown.problem(), None);
        assert!(unknown.switches().is_empty());
    }

    #[test]
    fn a_switch_writes_the_hosts_request() {
        let write = serde_json::to_value(audio_write(HEADPHONE_SURROUND, true)).unwrap();
        assert_eq!(write, serde_json::json!({ "changes": [{ "id": "audio.headphone_surround", "value": true }] }));
        let write = serde_json::to_value(audio_write(NIGHT_MODE, false)).unwrap();
        assert_eq!(write, serde_json::json!({ "changes": [{ "id": "audio.night_mode", "value": false }] }));
        assert_eq!((audio_label(NIGHT_MODE), audio_label(HEADPHONE_SURROUND)), ("Night mode", "Headphone surround"));
    }

    #[test]
    fn the_scale_matches_v_plus_and_round_trips() {
        assert_eq!(positions(), 9 + 15 + 15 + 10 + 10 + 4 + 4);
        assert_eq!(kbps(0.0), 500);
        assert_eq!(kbps(9.0), 5_000);
        assert_eq!(kbps(24.0), 20_000);
        assert_eq!(kbps(f64::from(positions())), 800_000);
        assert_eq!(kbps(-3.0), 500);
        assert_eq!(kbps(1e9), 800_000);
        let mut previous = 0;
        for at in 0..=positions() {
            let k = kbps(f64::from(at));
            assert!(k > previous, "{at}: {k} after {previous}");
            assert_eq!(position(k), f64::from(at), "{k} kbps");
            previous = k;
        }
        assert_eq!(position(2_000_000), f64::from(positions()), "a higher stream bitrate sits at the end");
        assert_eq!(position(5_500), 9.5, "between steps");
        assert_eq!(kbps(9.5), 5_500, "a typed value off the steps is kept");
    }

    #[test]
    fn typed_values_and_labels() {
        assert_eq!(parse("50"), Some(50_000));
        assert_eq!(parse(" 2,5 Mbps"), Some(2_500));
        assert_eq!(parse("750 kbps"), Some(750));
        assert_eq!(parse("10 kbps"), Some(500), "clamped to the range");
        assert_eq!(parse("fast"), None);
        assert_eq!(parse("-4"), None);
        assert_eq!(label(800), "800 kbps");
        assert_eq!(label(1_500), "1.5 Mbps");
        assert_eq!(label(150_000), "150 Mbps");
    }

    #[test]
    fn a_drag_sends_on_release_and_steps_after_a_pause() {
        let now = Instant::now();
        let mut scrub = Scrub::default();
        assert!(!scrub.step(true, true, false, now), "not while dragging");
        assert!(!scrub.step(false, true, false, now + Duration::from_secs(2)), "nor while held still");
        assert!(scrub.step(false, false, true, now + Duration::from_secs(2)), "on release");
        assert!(!scrub.step(false, false, true, now), "a release without a change sends nothing");

        assert!(!scrub.step(true, false, false, now), "a key step waits");
        assert!(!scrub.step(true, false, false, now + Duration::from_millis(300)));
        assert!(!scrub.step(false, false, false, now + Duration::from_millis(700)), "450 ms after the last step only");
        assert!(scrub.step(false, false, false, now + Duration::from_millis(750)));
        assert!(!scrub.step(false, false, false, now + Duration::from_secs(5)), "once");
    }

    #[test]
    fn a_step_still_waiting_when_the_menu_closes_is_found_once() {
        let ctx = egui::Context::default();
        let key = |key| egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() };
        let (mut value, mut sent) = (20_000, false);
        for events in [vec![key(egui::Key::Tab)], vec![key(egui::Key::ArrowRight)]] {
            ctx.run_ui(egui::RawInput { events, ..Default::default() }, |ui| sent |= bitrate_slider(ui, &mut value, true))
                .textures_delta.clear();
        }
        assert_eq!(value, 22_000, "one step up");
        assert!(!sent, "a key step waits");
        // The menu closed before the wait was over: the change is found, and only once.
        assert!(take_pending(&ctx));
        assert!(!take_pending(&ctx));
    }
}
