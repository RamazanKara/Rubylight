//! Pure `egui` stream overlay widgets (bitrate slider, guest management panel, USB device picker)
//! lifted out of `desktop-media::stream` and `desktop-media::usb` so `nightjar-desktop-media` has
//! zero `egui` dependency.

use std::time::{Duration, Instant};

use egui::Ui;
use nightjar_policy::http::{
    GuestAction, InviteCreated, SessionGuest, SessionGuests, GUEST_CONTROLLER,
    GUEST_KEYBOARD_MOUSE, GUEST_VIEW,
};

use crate::strings::{en, tr, trf};

const MIN_KBPS: u32 = 500;
const MAX_KBPS: u32 = 2_000_000;
/// Each segment's top and step, in kbps: fine steps where a few Mbps matter, coarse ones above.
const SEGMENTS: [(u32, u32); 8] = [
    (5_000, 500),
    (20_000, 1_000),
    (50_000, 2_000),
    (100_000, 5_000),
    (200_000, 10_000),
    (400_000, 50_000),
    (800_000, 100_000),
    (2_000_000, 100_000),
];
/// The idle time after the last keyboard or gamepad step before the bitrate is sent.
pub const COMMIT_DELAY: Duration = Duration::from_millis(450);

fn positions() -> u32 {
    let mut from = MIN_KBPS;
    SEGMENTS
        .iter()
        .map(|&(top, step)| {
            let steps = (top - from) / step;
            from = top;
            steps
        })
        .sum()
}

/// The slider position of a bitrate; values off the steps fall between them.
pub fn position(kbps: u32) -> f64 {
    let kbps = kbps.clamp(MIN_KBPS, MAX_KBPS);
    let (mut at, mut from) = (0.0, MIN_KBPS);
    for (top, step) in SEGMENTS {
        if kbps <= top {
            return at + f64::from(kbps - from) / f64::from(step);
        }
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
        if position <= start + steps {
            return from + ((position - start) * f64::from(step)).round() as u32;
        }
        start += steps;
        from = top;
    }
    MAX_KBPS
}

pub fn bitrate_label(kbps: u32) -> String {
    if kbps < 1000 {
        return format!("{kbps} kbps");
    }
    let mbps = f64::from(kbps) / 1000.0;
    if kbps.is_multiple_of(1000) {
        format!("{mbps:.0} Mbps")
    } else {
        format!("{mbps:.1} Mbps")
    }
}

/// Typed text: a number in Mbps, or with a kbps or Mbps suffix.
pub fn parse_bitrate(text: &str) -> Option<u32> {
    let text = text.trim().to_lowercase().replace(',', ".");
    let (number, scale) = if let Some(n) = text.strip_suffix("kbps") {
        (n, 1.0)
    } else {
        (text.strip_suffix("mbps").unwrap_or(&text), 1000.0)
    };
    let value = number.trim().parse::<f64>().ok().filter(|v| v.is_finite() && *v > 0.0)? * scale;
    Some((value.round() as u32).clamp(MIN_KBPS, MAX_KBPS))
}

/// When the slider's bitrate goes out: on release after a drag at once, after keyboard or
/// gamepad steps once they stop for [`COMMIT_DELAY`].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Scrub {
    pub changed_at: Option<Instant>,
}

impl Scrub {
    /// Whether to send now.
    pub fn step(&mut self, changed: bool, dragging: bool, released: bool, now: Instant) -> bool {
        if changed {
            self.changed_at = Some(now);
        }
        let due = released || (!dragging && self.changed_at.is_some_and(|at| now.duration_since(at) >= COMMIT_DELAY));
        due && self.changed_at.take().is_some()
    }
}

/// The bitrate slider. `value` (kbps) follows it; true when the new value should be sent.
pub fn bitrate_slider(ui: &mut egui::Ui, value: &mut u32, enabled: bool) -> bool {
    let mut at = position(*value);
    let before = at;
    let response = ui
        .horizontal(|ui| {
            ui.add_space(crate::menu_style::PAD);
            ui.label(tr("Bitrate"));
            ui.add_enabled_ui(enabled, |ui| {
                crate::theme::slider(
                    ui,
                    &mut at,
                    0.0..=f64::from(positions()),
                    1.0,
                    |at| bitrate_label(kbps(at)),
                    |rail| rail.step_by(1.0),
                    |field| {
                        field
                            .speed(0.25)
                            .custom_formatter(|at, _| bitrate_label(kbps(at)))
                            .custom_parser(|text| parse_bitrate(text).map(position))
                    },
                )
            })
            .inner
        })
        .inner;
    if at != before {
        *value = kbps(at);
    }
    let mut scrub = ui.data(|d| d.get_temp::<Scrub>(scrub_id())).unwrap_or_default();
    let send = scrub.step(at != before, response.dragged(), response.drag_stopped(), Instant::now());
    if scrub.changed_at.is_some() {
        ui.ctx().request_repaint_after(COMMIT_DELAY);
    }
    ui.data_mut(|d| d.insert_temp(scrub_id(), scrub));
    send && enabled
}

/// One slider is shown at a time (HTTP or 0x553C), so its wait is kept under one id that
/// `settle_bitrate_slider` finds after the menu closed.
fn scrub_id() -> egui::Id {
    egui::Id::unique("bitrate-slider-scrub")
}

/// Whether a slider change was still waiting for [`COMMIT_DELAY`]; it no longer waits after this.
pub fn take_pending_bitrate_slider(ctx: &egui::Context) -> bool {
    ctx.data_mut(|d| d.remove_temp::<Scrub>(scrub_id())).is_some_and(|scrub| scrub.changed_at.is_some())
}

/// What a permission string means for the user.
pub fn permission_label(permission: &str) -> &'static str {
    match permission {
        GUEST_VIEW => tr("View only"),
        GUEST_KEYBOARD_MOUSE => tr("Controller, keyboard and mouse"),
        _ => tr("Controller"),
    }
}

/// One guest's line: its name, what it may do, the keyboard if it holds it, and away.
pub fn guest_line(read: &SessionGuests, guest: &SessionGuest) -> String {
    let mut line = format!("{} · {}", guest.name, permission_label(&guest.permission));
    if read.keyboard_mouse == Some(guest.id) {
        line.push_str(" · ");
        line.push_str(tr("has the keyboard"));
    }
    if !guest.streaming {
        line.push_str(" · ");
        line.push_str(tr("away"));
    }
    line
}

/// Invite status shown inside the Guests section of the stream menu.
#[derive(Clone, Debug)]
pub enum GuestsInviteView<'a> {
    None,
    Making,
    Made(&'a InviteCreated),
}

/// Action clicked by the user inside the Guests stream-menu section.
#[derive(Clone, Debug)]
pub enum GuestUiAction {
    Guest(GuestAction),
    CreateInvite,
    ResetInvite,
    Toast(&'static str),
}

/// Draws the Guests section inside the stream menu without coupling to `Streaming`.
pub fn show_guests(
    ui: &mut Ui,
    read: &SessionGuests,
    may_invite: bool,
    invite: GuestsInviteView<'_>,
) -> Vec<GuestUiAction> {
    let mut actions = Vec::new();
    // Opened for a join request, nothing has the focus yet: Let in takes it, as on Android.
    let mut focus_first = ui.ctx().memory(|m| m.focused().is_none());
    for request in &read.waiting {
        ui.label(trf("{} wants to join your game", &[&request.name]));
        ui.horizontal(|ui| {
            let let_in = ui.button(tr("Let in"));
            if std::mem::take(&mut focus_first) {
                let_in.request_focus();
            }
            if let_in.clicked() {
                actions.push(GuestUiAction::Guest(GuestAction::Approve { id: request.id, permission: None }));
            }
            if ui.button(tr("Deny")).clicked() {
                actions.push(GuestUiAction::Guest(GuestAction::Deny(request.id)));
            }
        });
    }
    if read.guests.is_empty() && read.waiting.is_empty() {
        ui.weak(tr("Nobody has joined your game"));
    }
    for guest in &read.guests {
        ui.separator();
        ui.label(guest_line(read, guest));
        ui.horizontal_wrapped(|ui| {
            for permission in [GUEST_VIEW, GUEST_CONTROLLER, GUEST_KEYBOARD_MOUSE] {
                if permission != guest.permission && ui.button(permission_label(permission)).clicked() {
                    actions.push(GuestUiAction::Guest(GuestAction::Permission {
                        id: guest.id,
                        permission: permission.to_owned(),
                    }));
                }
            }
            if guest.permission == GUEST_KEYBOARD_MOUSE && guest.streaming {
                if read.keyboard_mouse == Some(guest.id) {
                    if ui.button(tr("Take back keyboard")).clicked() {
                        actions.push(GuestUiAction::Guest(GuestAction::KeyboardMouse(None)));
                    }
                } else if ui.button(tr("Give keyboard")).clicked() {
                    actions.push(GuestUiAction::Guest(GuestAction::KeyboardMouse(Some(guest.id))));
                }
            }
            if ui.button(tr("Kick")).clicked() {
                actions.push(GuestUiAction::Guest(GuestAction::Kick(guest.id)));
            }
            if ui.button(tr("Block")).clicked() {
                actions.push(GuestUiAction::Guest(GuestAction::Revoke(guest.id)));
            }
        });
    }
    if may_invite {
        ui.separator();
        match invite {
            GuestsInviteView::None => {
                if ui.button(tr("Invite a friend over the internet")).clicked() {
                    actions.push(GuestUiAction::CreateInvite);
                }
            }
            GuestsInviteView::Making => {
                ui.weak(tr("Making an invite…"));
            }
            GuestsInviteView::Made(made) => {
                ui.label(tr("Send this invite to one person. It pairs their Nightjar as a guest once, and works for a day."));
                let message = trf(
                    "Join my game with Nightjar: open this link in Nightjar, or copy it and paste it under Add PC.\n{}",
                    &[&made.link],
                );
                ui.horizontal_wrapped(|ui| {
                    if ui.button(tr("Copy invite")).clicked() {
                        ui.ctx().copy_text(message);
                        actions.push(GuestUiAction::Toast(en("Invite copied")));
                    }
                    if ui.button(tr("New invite")).clicked() {
                        actions.push(GuestUiAction::ResetInvite);
                    }
                });
            }
        }
    }
    actions
}

/// Pure UI view of one plugged-in USB device.
#[derive(Clone, Debug)]
pub struct UsbDeviceView {
    pub busid: String,
    pub vid: u16,
    pub pid: u16,
    pub name: String,
    pub blocked_message: Option<String>,
    /// Windows has another driver on it and usbipd-win lists it: it may go through usbipd-win.
    pub usbipd: bool,
    pub can_auto: bool,
    pub auto_on: bool,
    pub opening: bool,
    pub active: Option<(u32, String)>,
}

/// Pure UI view of a finished/failed USB attachment notice.
#[derive(Clone, Debug)]
pub struct UsbFailedAttachment {
    pub busid: String,
    pub name: String,
    pub phase_label: String,
}

/// Pure UI view of the stream menu's USB section.
#[derive(Clone, Debug, Default)]
pub struct UsbPanelView {
    pub devices: Vec<UsbDeviceView>,
    pub failed_attachments: Vec<UsbFailedAttachment>,
    pub error: Option<String>,
    /// The shares Nightjar left in usbipd-win when it stopped, and whether they are being given back.
    pub usbipd_left: usize,
    pub usbipd_giving_back: bool,
}

/// Action triggered by the stream menu's USB section.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UsbUiAction {
    Refresh,
    Attach(String),
    /// Forward through usbipd-win, which the user asked for on a device another driver has.
    AttachUsbipd(String),
    Detach(u32),
    SetAuto(String, bool),
    /// Give back what Nightjar left shared in usbipd-win (Windows asks for administrator rights).
    GiveBackUsbipd,
}

/// The stream controls' USB section when the setting is on but the PC offers nothing: why.
pub fn usb_show_refusal(ui: &mut egui::Ui, why: &str) {
    crate::menu_style::row(ui, crate::glyphs::Icon::Usb, tr("USB devices"), |_| {});
    crate::menu_style::problem(ui, tr(why));
}

/// The stream controls' USB section rendered from a pure `UsbPanelView`.
pub fn usb_show(ui: &mut egui::Ui, view: &UsbPanelView) -> Vec<UsbUiAction> {
    let mut actions = Vec::new();
    if ui.button(tr("Refresh")).clicked() {
        actions.push(UsbUiAction::Refresh);
    }
    if view.devices.is_empty() {
        ui.weak(tr("No USB devices are plugged in here."));
    }
    for device in &view.devices {
        ui.horizontal(|ui| {
            ui.label(format!("{} ({:04x}:{:04x}, {})", device.name, device.vid, device.pid, device.busid));
            if device.can_auto {
                let mut on = device.auto_on;
                if ui
                    .checkbox(&mut on, tr("Automatic"))
                    .on_hover_text(tr("Attach this device to the PC whenever it is plugged in during a stream."))
                    .changed()
                {
                    actions.push(UsbUiAction::SetAuto(device.busid.clone(), on));
                }
            }
            if device.opening {
                ui.weak(tr("Waiting for your password"));
                return;
            }
            match (&device.active, device.blocked_message.is_some()) {
                (Some((request_id, phase_label)), _) => {
                    ui.weak(phase_label);
                    if ui.button(tr("Detach")).clicked() {
                        actions.push(UsbUiAction::Detach(*request_id));
                    }
                }
                (None, true) if device.usbipd => {
                    if ui
                        .button(tr("Forward with usbipd-win"))
                        .on_hover_text(tr("Windows keeps its own driver on the device: usbipd-win lends it to the PC while it is attached. Windows asks for administrator rights to share it, and again to stop."))
                        .clicked()
                    {
                        actions.push(UsbUiAction::AttachUsbipd(device.busid.clone()));
                    }
                }
                (None, true) => {
                    ui.add_enabled(false, egui::Button::new(tr("Attach")));
                }
                (None, false) => {
                    if ui.button(tr("Attach")).clicked() {
                        actions.push(UsbUiAction::Attach(device.busid.clone()));
                    }
                }
            }
        });
        if let Some(blocked) = &device.blocked_message {
            ui.colored_label(crate::theme::ERROR, blocked);
        }
    }
    if view.usbipd_left > 0 {
        ui.horizontal(|ui| {
            ui.label(trf("Still shared through usbipd-win since Nightjar closed: {}", &[&view.usbipd_left]));
            if view.usbipd_giving_back {
                ui.weak(tr("Giving back…"));
            } else if ui.button(tr("Give back to Windows")).clicked() {
                actions.push(UsbUiAction::GiveBackUsbipd);
            }
        });
    }
    for failed in &view.failed_attachments {
        ui.colored_label(crate::theme::ERROR, format!("{}: {}", failed.name, failed.phase_label));
    }
    if let Some(error) = &view.error {
        ui.colored_label(crate::theme::ERROR, error);
    }
    actions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scale_matches_v_plus_and_round_trips() {
        assert_eq!(positions(), 9 + 15 + 15 + 10 + 10 + 4 + 4 + 12);
        assert_eq!(kbps(0.0), 500);
        assert_eq!(kbps(9.0), 5_000);
        assert_eq!(kbps(24.0), 20_000);
        assert_eq!(kbps(f64::from(positions())), 2_000_000);
        assert_eq!(kbps(-3.0), 500);
        assert_eq!(kbps(1e9), 2_000_000);
        let mut previous = 0;
        for at in 0..=positions() {
            let k = kbps(f64::from(at));
            assert!(k > previous, "{at}: {k} after {previous}");
            assert_eq!(position(k), f64::from(at), "{k} kbps");
            previous = k;
        }
        assert_eq!(position(1_500_000).fract(), 0.0, "1.5 Gbps sits on an exact slider step");
        assert_eq!(position(2_500_000), f64::from(positions()), "a bitrate above the ceiling sits at the end");
        assert_eq!(position(5_500), 9.5, "between steps");
        assert_eq!(kbps(9.5), 5_500, "a typed value off the steps is kept");
    }

    #[test]
    fn typed_values_and_labels() {
        assert_eq!(parse_bitrate("50"), Some(50_000));
        assert_eq!(parse_bitrate(" 2,5 Mbps"), Some(2_500));
        assert_eq!(parse_bitrate("750 kbps"), Some(750));
        assert_eq!(parse_bitrate("10 kbps"), Some(500), "clamped to the range");
        assert_eq!(parse_bitrate("fast"), None);
        assert_eq!(parse_bitrate("-4"), None);
        assert_eq!(bitrate_label(800), "800 kbps");
        assert_eq!(bitrate_label(1_500), "1.5 Mbps");
        assert_eq!(bitrate_label(150_000), "150 Mbps");
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
        assert!(take_pending_bitrate_slider(&ctx));
        assert!(!take_pending_bitrate_slider(&ctx));
    }

    /// The stream menu's trap (nightjar#61): from the row below, Up landed in the slider's number
    /// box and every further Up raised the bitrate. Up and Down now pass the row, Left and Right step.
    #[test]
    fn up_and_down_pass_the_bitrate_row_without_changing_it() {
        let ctx = egui::Context::default();
        let (mut value, mut ids, mut time) = (20_000, [egui::Id::NULL; 2], 0.0);
        let mut frame = |events: Vec<egui::Event>, value: &mut u32| {
            time += 0.1;
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(600.0, 400.0))),
                time: Some(time),
                events,
                ..Default::default()
            };
            // Full-width rows, as the menu's are, so the bitrate row lies on the way up.
            let row = |text| egui::Button::new(text).min_size(egui::vec2(560.0, 30.0));
            ctx.run_ui(input, |ui| {
                ids[0] = ui.add(row("above")).id;
                bitrate_slider(ui, value, true);
                ids[1] = ui.add(row("below")).id;
            })
            .textures_delta
            .clear();
            ids
        };
        let ids = frame(Vec::new(), &mut value);
        ctx.memory_mut(|m| m.request_focus(ids[1]));
        frame(Vec::new(), &mut value);
        frame(Vec::new(), &mut value);
        assert_eq!(ctx.memory(|m| m.focused()), Some(ids[1]));
        let tap = |key| {
            vec![
                egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() },
                egui::Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers: Default::default() },
            ]
        };
        let mut press = |key, value: &mut u32| {
            frame(tap(key), value);
            frame(Vec::new(), value);
            ctx.memory(|m| m.focused())
        };
        let on_row = press(egui::Key::ArrowUp, &mut value);
        assert!(on_row.is_some_and(|id| !ids.contains(&id)), "Up from below reaches the bitrate row: {on_row:?} {ids:?}");
        assert_eq!(value, 20_000, "reaching it changes nothing");
        assert_eq!(press(egui::Key::ArrowRight, &mut value), on_row, "Right steps in place");
        assert_ne!(value, 20_000, "Right steps the bitrate");
        let stepped = value;
        let up = press(egui::Key::ArrowUp, &mut value);
        assert!(up == Some(ids[0]) || up.is_some_and(|id| id != on_row.unwrap() && id != ids[1]), "Up leaves the box: {up:?}");
        assert_eq!(value, stepped, "Up does not step");
        // Up again ends above the row, wherever the row's two stops are.
        let top = if up == Some(ids[0]) { up } else { press(egui::Key::ArrowUp, &mut value) };
        assert_eq!(top, Some(ids[0]), "never stuck on the row");
        assert_eq!(value, stepped);
    }

    #[test]
    fn guest_line_names_the_keyboard_and_away() {
        let mut list = SessionGuests { manage: true, ..Default::default() };
        list.keyboard_mouse = Some(2);
        let guest = SessionGuest {
            id: 2,
            name: "Phone".into(),
            permission: GUEST_KEYBOARD_MOUSE.into(),
            streaming: false,
            since: 0,
            expires: None,
        };
        assert_eq!(guest_line(&list, &guest), "Phone · Controller, keyboard and mouse · has the keyboard · away");
    }
}
