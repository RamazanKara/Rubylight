//! Stream menu controls ported from Moonlight V+ PC (qiin2333/moonlight-qt,
//! `app/streaming/video/overlaymenupanel.cpp` and `app/streaming/input/gamepad.cpp`): the
//! controller mouse switch and the bitrate slider with its piecewise scale.

use std::time::{Duration, Instant};

use crate::gamepad::Gamepads;
use crate::strings::{tr, trf};

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
    let id = response.id.with("bitrate-scrub");
    let mut scrub = ui.data(|d| d.get_temp::<Scrub>(id)).unwrap_or_default();
    let send = scrub.step(at != before, response.dragged(), response.drag_stopped(), Instant::now());
    ui.data_mut(|d| d.insert_temp(id, scrub));
    send && enabled
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
