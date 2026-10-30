//! The PIN screen and the PIN lock settings page, as Nightjar Android's PinLockActivity and
//! PinLockSettingsActivity show them. The checks and the hashing are in pin_lock.rs; the slow
//! ones (about 150 ms, measured per computer) run on a worker thread while the screen says so.

use std::path::Path;
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::{vec2, Align, Key, Layout, RichText, Sense, Ui};

use crate::pin_lock::{Mode, Outcome, PinGate, PinStore, GRACE_CHOICES, MAX_DIGITS, MIN_DIGITS};
use crate::strings::{en, tr, trf};
use crate::theme::*;
use crate::ui::{primary_button, quiet_button, Command};

/// What the PIN screen is up for.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Purpose {
    /// The app is locked; the screen covers it and only closing Nightjar gets past it.
    App,
    /// Starting a stream on the PC `uuid`, called `pc`.
    Stream { uuid: String, pc: String },
    /// The current PIN, before the settings page opens or the PIN changes or goes.
    Verify(After),
    Choose,
    /// The PIN chosen in the step before, to type again.
    Confirm(Vec<u8>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum After {
    Open,
    Change,
    Remove,
}

struct Pad {
    purpose: Purpose,
    digits: Vec<u8>,
    error: Option<String>,
    /// Input is refused until then (too many wrong tries).
    locked_until: Option<Instant>,
}

impl Pad {
    fn new(purpose: Purpose, store: &PinStore) -> Self {
        let mut pad = Self { purpose, digits: Vec::new(), error: None, locked_until: None };
        if pad.checks() {
            pad.lock_for(store.locked_for_ms());
        }
        pad
    }

    /// Whether this screen checks the stored PIN (rather than choosing a new one).
    fn checks(&self) -> bool {
        matches!(self.purpose, Purpose::App | Purpose::Stream { .. } | Purpose::Verify(_))
    }

    fn lock_for(&mut self, ms: i64) {
        self.locked_until = (ms > 0).then(|| Instant::now() + Duration::from_millis(ms as u64));
    }

    fn clear(&mut self) {
        self.digits.fill(0);
        self.digits.clear();
    }
}

impl Drop for Pad {
    fn drop(&mut self) {
        self.clear();
        if let Purpose::Confirm(first) = &mut self.purpose {
            first.fill(0);
        }
    }
}

enum Done {
    Tried(Outcome),
    Saved(Result<(), String>),
}

/// The lock as the launcher holds it: the store, the gate, and what is on screen.
pub struct PinLock {
    pub store: PinStore,
    gate: PinGate,
    pad: Option<Pad>,
    /// The settings page is open (after its PIN was given, while the window stayed in view).
    page: bool,
    job: Option<Receiver<Done>>,
    notice: Option<&'static str>,
}

impl PinLock {
    pub fn open(dir: &Path) -> Self {
        Self::with_store(PinStore::new(dir, Arc::new(crate::pin_lock::SystemClock)))
    }

    pub fn with_store(store: PinStore) -> Self {
        let gate = PinGate::new(&store);
        let mut lock = Self { store, gate, pad: None, page: false, job: None, notice: None };
        lock.lock_app_if_needed();
        lock
    }

    fn lock_app_if_needed(&mut self) {
        if self.gate.needs_pin_for_app(&self.store) && !matches!(self.pad.as_ref().map(|p| &p.purpose), Some(Purpose::App)) {
            self.page = false;
            self.pad = Some(Pad::new(Purpose::App, &self.store));
        }
    }

    /// The app lock's screen is up.
    pub fn app_locked(&self) -> bool {
        matches!(self.pad.as_ref().map(|p| &p.purpose), Some(Purpose::App))
    }

    /// A check or a save runs: the loop keeps polling for its answer.
    pub fn busy(&self) -> bool {
        self.job.is_some()
    }

    /// The window went out of sight, or Nightjar is closing.
    pub fn left(&mut self) {
        self.gate.left(&self.store);
        // As on Android, the settings page asks for the PIN again on the next visit.
        self.page = false;
    }

    pub fn returned(&mut self) {
        self.gate.returned(&self.store);
        self.lock_app_if_needed();
    }

    /// Whether starting a stream on `uuid` has to wait for the PIN; a grant is used up here.
    pub fn needs_pin_for_stream(&mut self, uuid: &str) -> bool {
        self.gate.needs_pin_for_stream(&self.store, uuid)
    }

    /// Asks for the PIN to start a stream on `uuid` (`pc` names it). Under the app lock its
    /// screen asks already, and unlocking the app lets the start go on.
    pub fn ask_for_stream(&mut self, uuid: &str, pc: &str) {
        if !self.app_locked() {
            self.pad = Some(Pad::new(Purpose::Stream { uuid: uuid.to_owned(), pc: pc.to_owned() }, &self.store));
        }
    }

    /// The settings row: the page, after the PIN when one is set.
    pub fn open_settings(&mut self) {
        self.notice = None;
        if self.store.has_pin() {
            self.pad = Some(Pad::new(Purpose::Verify(After::Open), &self.store));
        } else {
            self.page = true;
        }
    }

    /// Draws the PIN screen or the settings page when one is up, over everything else, and says
    /// whether it did. `pcs` are the saved PCs as (unique id, name).
    pub fn ui(&mut self, ui: &mut Ui, pcs: &[(String, String)], commands: &mut Vec<Command>) -> bool {
        self.poll(commands);
        if self.pad.is_none() && !self.page {
            return false;
        }
        egui::Frame::new().fill(*NIGHT).show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            if self.pad.is_some() {
                self.pad_ui(ui, commands);
            } else {
                self.page_ui(ui, pcs);
            }
        });
        true
    }

    fn poll(&mut self, commands: &mut Vec<Command>) {
        let Some(done) = self.job.as_ref().and_then(|job| job.try_recv().ok()) else { return };
        self.job = None;
        let Some(pad) = self.pad.as_mut() else { return };
        match done {
            Done::Tried(Outcome::Ok) => {
                let purpose = pad.purpose.clone();
                self.pad = None;
                match purpose {
                    Purpose::App => {
                        self.gate.unlock_app();
                        commands.push(Command::PinPassed);
                    }
                    Purpose::Stream { uuid, .. } => {
                        self.gate.grant(&self.store, &uuid);
                        commands.push(Command::PinPassed);
                    }
                    Purpose::Verify(After::Open) => self.page = true,
                    Purpose::Verify(After::Change) => self.pad = Some(Pad::new(Purpose::Choose, &self.store)),
                    Purpose::Verify(After::Remove) => {
                        self.store.remove_pin();
                        self.notice = Some(en("PIN removed"));
                    }
                    Purpose::Choose | Purpose::Confirm(_) => {}
                }
            }
            Done::Tried(Outcome::Wrong { locked_for_ms }) => {
                pad.clear();
                pad.lock_for(locked_for_ms);
                pad.error = Some(tr("Wrong PIN. Try again.").to_owned());
            }
            Done::Tried(Outcome::Locked { locked_for_ms }) => {
                pad.clear();
                pad.lock_for(locked_for_ms);
            }
            Done::Saved(Ok(())) => {
                self.pad = None;
                if self.store.mode() == Mode::Off {
                    // A PIN that locks nothing would surprise: start with the app itself.
                    self.store.set_mode(Mode::App);
                }
                self.gate.unlock_app();
                self.notice = Some(en("PIN saved"));
                self.page = true;
            }
            Done::Saved(Err(why)) => {
                eprintln!("nightjar: saving the PIN failed: {why}");
                pad.clear();
                pad.error = Some(why);
            }
        }
    }

    /// Takes the digits typed so far: checks them, or moves to the next step of choosing a PIN.
    fn submit(&mut self) {
        let Some(pad) = self.pad.as_mut() else { return };
        if self.job.is_some() || pad.locked_until.is_some_and(|t| t > Instant::now()) {
            return;
        }
        let (sender, receiver) = channel();
        match &pad.purpose {
            Purpose::App | Purpose::Stream { .. } | Purpose::Verify(_) => {
                if pad.digits.len() < MIN_DIGITS {
                    pad.error = Some(tr("Use at least 4 digits.").to_owned());
                    return;
                }
                let (store, pin) = (self.store.clone(), std::mem::take(&mut pad.digits));
                std::thread::spawn(move || {
                    let mut pin = pin;
                    let _ = sender.send(Done::Tried(store.try_pin(&pin)));
                    pin.fill(0);
                });
            }
            Purpose::Choose => {
                if pad.digits.len() < MIN_DIGITS {
                    pad.error = Some(tr("Use at least 4 digits.").to_owned());
                    return;
                }
                let first = std::mem::take(&mut pad.digits);
                self.pad = Some(Pad::new(Purpose::Confirm(first), &self.store));
                return;
            }
            Purpose::Confirm(first) => {
                if pad.digits != *first {
                    self.pad = Some(Pad::new(Purpose::Choose, &self.store));
                    if let Some(pad) = self.pad.as_mut() {
                        pad.error = Some(tr("The PINs did not match. Choose a PIN again.").to_owned());
                    }
                    return;
                }
                let (store, pin) = (self.store.clone(), std::mem::take(&mut pad.digits));
                std::thread::spawn(move || {
                    let mut pin = pin;
                    let _ = sender.send(Done::Saved(store.set_pin(&pin)));
                    pin.fill(0);
                });
            }
        }
        pad.error = None;
        self.job = Some(receiver);
    }

    fn cancel(&mut self, commands: &mut Vec<Command>) {
        let Some(pad) = self.pad.take() else { return };
        if let Purpose::Stream { .. } = pad.purpose {
            commands.push(Command::PinCancelled);
        }
    }

    fn key(&mut self, key: PadKey) {
        let Some(pad) = self.pad.as_mut() else { return };
        if self.job.is_some() || pad.locked_until.is_some_and(|t| t > Instant::now()) {
            return;
        }
        let full = match &pad.purpose {
            Purpose::Confirm(first) => first.len(),
            Purpose::Choose => MAX_DIGITS,
            _ => self.store.pin_length().max(MIN_DIGITS),
        };
        match key {
            PadKey::Digit(d) if pad.digits.len() < full => {
                pad.digits.push(b'0' + d);
                pad.error = None;
                // Android submits on its own once the PIN's length is reached.
                if pad.digits.len() == full && !matches!(pad.purpose, Purpose::Choose) {
                    self.submit();
                }
            }
            PadKey::Digit(_) => {}
            PadKey::Delete => {
                if let Some(last) = pad.digits.last_mut() {
                    *last = 0;
                }
                pad.digits.pop();
            }
            PadKey::Confirm => self.submit(),
            PadKey::Cancel | PadKey::FocusFirst => {}
        }
    }

    fn pad_ui(&mut self, ui: &mut Ui, commands: &mut Vec<Command>) {
        let Some(pad) = self.pad.as_mut() else { return };
        if pad.locked_until.is_some_and(|t| t <= Instant::now()) {
            pad.locked_until = None;
        }
        let (title, subtitle) = match &pad.purpose {
            Purpose::App => (tr("Nightjar is locked"), tr("Enter your PIN to continue").to_owned()),
            Purpose::Stream { pc, .. } if !pc.is_empty() => (tr("Enter your PIN"), trf("Needed to start a stream on {}", &[pc])),
            Purpose::Stream { .. } => (tr("Enter your PIN"), tr("Needed to start a stream").to_owned()),
            Purpose::Verify(_) => (tr("Enter your PIN"), tr("Needed to change the PIN lock").to_owned()),
            Purpose::Choose => (tr("Choose a PIN"), tr("Use 4 to 8 digits").to_owned()),
            Purpose::Confirm(_) => (tr("Confirm your PIN"), tr("Type the same PIN again").to_owned()),
        };
        let status = if self.job.is_some() {
            Some((if pad.checks() { tr("Checking…") } else { tr("Saving…") }.to_owned(), false))
        } else if let Some(until) = pad.locked_until {
            let left = until.saturating_duration_since(Instant::now()).as_secs() + 1;
            Some((trf("Too many wrong tries. Try again in {}.", &[&format!("{}:{:02}", left / 60, left % 60)]), true))
        } else {
            pad.error.clone().map(|e| (e, true))
        };
        let full = match &pad.purpose {
            Purpose::Confirm(first) => first.len(),
            Purpose::Choose => MAX_DIGITS.min(pad.digits.len().max(MIN_DIGITS)),
            _ => self.store.pin_length().max(MIN_DIGITS),
        };
        let entered = pad.digits.len();
        let can_cancel = pad.purpose != Purpose::App;

        // Typing: digits, Backspace, Enter (unless a key of the pad has focus, which Enter presses).
        let focused = ui.ctx().memory(|m| m.focused());
        let mut typed = Vec::new();
        ui.input(|i| {
            for event in &i.events {
                if let egui::Event::Key { key, pressed: true, modifiers, .. } = event {
                    if modifiers.any() {
                        continue;
                    }
                    let digit = key.name().parse::<u8>().ok().filter(|d| *d <= 9);
                    match (key, digit) {
                        (_, Some(d)) => typed.push(PadKey::Digit(d)),
                        (Key::Backspace, _) => typed.push(PadKey::Delete),
                        (Key::Enter, _) if focused.is_none() => typed.push(PadKey::Confirm),
                        (Key::Escape, _) if can_cancel => typed.push(PadKey::Cancel),
                        (Key::ArrowUp | Key::ArrowDown | Key::ArrowLeft | Key::ArrowRight, _) if focused.is_none() => typed.push(PadKey::FocusFirst),
                        _ => {}
                    }
                }
            }
        });

        let mut pressed = Vec::new();
        let mut first_key = None;
        ui.vertical_centered(|ui| {
            ui.add_space((ui.available_height() * 0.12).max(space::XL));
            ui.label(RichText::new(title).font(semibold(text::TITLE)).color(ON_SURFACE));
            ui.add_space(space::S);
            ui.label(RichText::new(subtitle).font(regular(text::BODY)).color(ON_SURFACE_VARIANT));
            ui.add_space(space::XL);
            dots(ui, entered, full);
            ui.add_space(space::M);
            let (text, error) = status.unwrap_or_default();
            ui.label(RichText::new(text).font(regular(text::LABEL)).color(if error { ERROR } else { ON_SURFACE_VARIANT }));
            ui.add_space(space::L);
            let key_size = vec2(84.0, 56.0);
            let rows: [[PadKey; 3]; 4] = [
                [PadKey::Digit(1), PadKey::Digit(2), PadKey::Digit(3)],
                [PadKey::Digit(4), PadKey::Digit(5), PadKey::Digit(6)],
                [PadKey::Digit(7), PadKey::Digit(8), PadKey::Digit(9)],
                [PadKey::Delete, PadKey::Digit(0), PadKey::Confirm],
            ];
            let width = key_size.x * 3.0 + space::M * 2.0;
            for row in rows {
                ui.allocate_ui_with_layout(vec2(width, key_size.y), Layout::left_to_right(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = space::M;
                    for key in row {
                        let label = match key {
                            PadKey::Digit(d) => d.to_string(),
                            PadKey::Delete => tr("Delete").to_owned(),
                            PadKey::Confirm => tr("Confirm").to_owned(),
                            PadKey::Cancel | PadKey::FocusFirst => unreachable!("not on the pad"),
                        };
                        let font = if matches!(key, PadKey::Digit(_)) { semibold(text::SECTION) } else { semibold(text::LABEL) };
                        let button = egui::Button::new(RichText::new(label).font(font).color(ON_SURFACE))
                            .fill(*SURFACE_HIGHEST)
                            .corner_radius(egui::CornerRadius::same(radius::CONTROL))
                            .min_size(key_size);
                        let response = ui.add(button);
                        crate::ui::focus_ring(ui, response.rect, &response, radius::CONTROL);
                        if key == PadKey::Digit(1) {
                            first_key = Some(response.id);
                        }
                        if response.clicked() {
                            pressed.push(key);
                        }
                    }
                });
                ui.add_space(space::M);
            }
            ui.add_space(space::M);
            if can_cancel {
                if quiet_button(ui, "Cancel").clicked() {
                    pressed.push(PadKey::Cancel);
                }
            } else if quiet_button(ui, "Close Nightjar").clicked() {
                commands.push(Command::Quit);
            }
        });

        for key in typed.into_iter().chain(pressed) {
            match key {
                PadKey::Cancel => self.cancel(commands),
                PadKey::FocusFirst => {
                    if let Some(id) = first_key {
                        ui.ctx().memory_mut(|m| m.request_focus(id));
                    }
                }
                key => self.key(key),
            }
        }
    }

    fn page_ui(&mut self, ui: &mut Ui, pcs: &[(String, String)]) {
        let view = self.store.view();
        let mut close = ui.input(|i| i.key_pressed(Key::Escape));
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.add_space(space::XL);
            ui.horizontal(|ui| {
                ui.add_space(space::XL);
                ui.vertical(|ui| {
                    ui.set_max_width(720.0);
                    ui.label(RichText::new(tr("PIN lock")).font(semibold(text::TITLE)).color(ON_SURFACE));
                    ui.add_space(space::S);
                    if let Some(notice) = self.notice {
                        ui.label(RichText::new(tr(notice)).font(regular(text::LABEL)).color(OK));
                        ui.add_space(space::S);
                    }
                    let summary = if view.has_pin { tr("A PIN is set. Changing or removing it asks for the current one.") } else { tr("Choose 4 to 8 digits.") };
                    ui.label(RichText::new(summary).font(regular(text::BODY)).color(ON_SURFACE_VARIANT));
                    ui.add_space(space::M);
                    ui.horizontal(|ui| {
                        if view.has_pin {
                            if primary_button(ui, "Change PIN").clicked() {
                                self.notice = None;
                                self.pad = Some(Pad::new(Purpose::Verify(After::Change), &self.store));
                            }
                            if quiet_button(ui, "Remove PIN").clicked() {
                                self.notice = None;
                                self.pad = Some(Pad::new(Purpose::Verify(After::Remove), &self.store));
                            }
                        } else if primary_button(ui, "Set PIN").clicked() {
                            self.notice = None;
                            self.pad = Some(Pad::new(Purpose::Choose, &self.store));
                        }
                    });
                    ui.add_space(space::XL);
                    ui.add_enabled_ui(view.has_pin, |ui| {
                        ui.label(RichText::new(tr("Lock")).font(semibold(text::CARD_TITLE)).color(ON_SURFACE));
                        ui.add_space(space::S);
                        let mut mode = view.mode;
                        egui::ComboBox::from_id_salt("pin-mode").width(320.0).selected_text(tr(mode_label(mode))).show_ui(ui, |ui| {
                            for choice in Mode::ALL {
                                ui.selectable_value(&mut mode, choice, tr(mode_label(choice)));
                            }
                        });
                        if mode != view.mode {
                            self.store.set_mode(mode);
                            // Turning the app lock on while using the app must not lock it on the spot.
                            self.gate.unlock_app();
                        }
                        if view.mode == Mode::Selected {
                            ui.add_space(space::M);
                            ui.label(RichText::new(tr("Selected PCs")).font(semibold(text::LABEL)).color(ON_SURFACE));
                            if pcs.is_empty() {
                                ui.label(RichText::new(tr("No PCs added yet.")).font(regular(text::LABEL)).color(ON_SURFACE_VARIANT));
                            }
                            for (uuid, name) in pcs.iter().filter(|(uuid, _)| !uuid.is_empty()) {
                                let mut on = crate::pin_lock::contains(&view.pcs, uuid);
                                if ui.checkbox(&mut on, name).changed() {
                                    let mut chosen: Vec<String> = view.pcs.iter().filter(|pc| !pc.eq_ignore_ascii_case(uuid)).cloned().collect();
                                    if on {
                                        chosen.push(uuid.clone());
                                    }
                                    self.store.set_pcs(chosen);
                                }
                            }
                        }
                        if view.mode == Mode::App {
                            ui.add_space(space::M);
                            ui.label(RichText::new(tr("Ask again after")).font(semibold(text::LABEL)).color(ON_SURFACE));
                            let mut grace = view.grace_minutes;
                            egui::ComboBox::from_id_salt("pin-grace").width(320.0).selected_text(grace_label(grace)).show_ui(ui, |ui| {
                                for choice in GRACE_CHOICES {
                                    ui.selectable_value(&mut grace, choice, grace_label(choice));
                                }
                            });
                            if grace != view.grace_minutes {
                                self.store.set_grace_minutes(grace);
                            }
                        }
                    });
                    ui.add_space(space::XL);
                    ui.label(RichText::new(tr("If you forget the PIN, the only way to reset it is to delete pin_lock.json in Nightjar's settings folder, next to settings.json.")).font(regular(text::LABEL)).color(ON_SURFACE_VARIANT));
                    ui.add_space(space::L);
                    if primary_button(ui, "Done").clicked() {
                        close = true;
                    }
                });
            });
        });
        if close {
            self.page = false;
            self.notice = None;
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PadKey {
    Digit(u8),
    Delete,
    Confirm,
    Cancel,
    FocusFirst,
}

/// The digits typed so far as filled and empty circles; a screen reader hears the count.
fn dots(ui: &mut Ui, entered: usize, full: usize) {
    let step = 28.0;
    let (rect, response) = ui.allocate_exact_size(vec2(step * full as f32, 20.0), Sense::hover());
    for i in 0..full {
        let centre = egui::pos2(rect.left() + step * (i as f32 + 0.5), rect.center().y);
        if i < entered {
            ui.painter().circle_filled(centre, 7.0, *MOON);
        } else {
            ui.painter().circle_stroke(centre, 7.0, egui::Stroke::new(1.5, *OUTLINE));
        }
    }
    let said = trf("{} of {} digits entered", &[&entered, &full]);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &said));
}

fn mode_label(mode: Mode) -> &'static str {
    match mode {
        Mode::Off => en("Off"),
        Mode::App => en("Opening Nightjar"),
        Mode::Streams => en("Starting any stream"),
        Mode::Selected => en("Starting a stream on selected PCs"),
    }
}

fn grace_label(minutes: u32) -> String {
    match minutes {
        0 => tr("Every time Nightjar is left").to_owned(),
        60 => tr("1 hour").to_owned(),
        m => trf("{} minutes", &[&m]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pin_lock::Clock;
    use std::sync::atomic::{AtomicI64, Ordering};

    struct Still(AtomicI64);

    impl Clock for Still {
        fn elapsed(&self) -> i64 {
            self.0.load(Ordering::SeqCst)
        }
        fn boot(&self) -> i64 {
            1
        }
    }

    fn lock(tag: &str) -> (PinLock, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("nightjar-pin-ui-{tag}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        (PinLock::with_store(PinStore::new(&dir, Arc::new(Still(AtomicI64::new(5_000_000))))), dir)
    }

    fn type_pin(lock: &mut PinLock, pin: &str) {
        for d in pin.bytes() {
            lock.key(PadKey::Digit(d - b'0'));
        }
    }

    /// Waits for the worker's answer and takes it, as the next frame would.
    fn finish(lock: &mut PinLock) -> Vec<Command> {
        let mut commands = Vec::new();
        for _ in 0..500 {
            if lock.job.is_none() {
                break;
            }
            lock.poll(&mut commands);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(lock.job.is_none(), "the worker answered");
        commands
    }

    #[test]
    fn setting_a_pin_twice_turns_the_app_lock_on_without_locking_now() {
        let (mut lock, dir) = lock("set");
        lock.open_settings();
        assert!(lock.page && lock.pad.is_none(), "no PIN yet: the page opens straight away");
        lock.pad = Some(Pad::new(Purpose::Choose, &lock.store));
        type_pin(&mut lock, "2468");
        lock.key(PadKey::Confirm);
        assert!(matches!(lock.pad.as_ref().unwrap().purpose, Purpose::Confirm(_)));
        type_pin(&mut lock, "2469");
        assert_eq!(lock.pad.as_ref().unwrap().purpose, Purpose::Choose, "a mismatch starts over");
        assert!(lock.pad.as_ref().unwrap().error.is_some());
        type_pin(&mut lock, "2468");
        lock.key(PadKey::Confirm);
        type_pin(&mut lock, "2468");
        finish(&mut lock);
        assert!(lock.store.has_pin() && lock.store.mode() == Mode::App);
        assert!(!lock.app_locked() && lock.page, "the app stays open after setting it");
        // Leaving the window past the grace time locks it, and the right PIN opens it.
        lock.store.set_grace_minutes(0);
        lock.left();
        assert!(!lock.page, "the page asks again next time");
        lock.returned();
        assert!(lock.app_locked());
        type_pin(&mut lock, "1111");
        finish(&mut lock);
        assert!(lock.app_locked() && lock.pad.as_ref().unwrap().error.is_some(), "wrong PIN");
        type_pin(&mut lock, "2468");
        let commands = finish(&mut lock);
        assert!(!lock.app_locked() && matches!(commands[..], [Command::PinPassed]));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_stream_prompt_grants_one_start_or_cancels_it() {
        let (mut lock, dir) = lock("stream");
        lock.store.set_pin(b"1357").unwrap();
        lock.store.set_mode(Mode::Selected);
        lock.store.set_pcs(vec!["PC-1".into()]);
        assert!(!lock.needs_pin_for_stream("pc-2"));
        assert!(lock.needs_pin_for_stream("pc-1"));
        lock.ask_for_stream("pc-1", "Desk");
        let mut commands = Vec::new();
        lock.cancel(&mut commands);
        assert!(matches!(commands[..], [Command::PinCancelled]) && lock.pad.is_none());
        lock.ask_for_stream("pc-1", "Desk");
        type_pin(&mut lock, "1357");
        assert!(matches!(finish(&mut lock)[..], [Command::PinPassed]));
        assert!(!lock.needs_pin_for_stream("pc-1"), "granted once");
        assert!(lock.needs_pin_for_stream("pc-1"), "and used up");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn removing_the_pin_asks_for_it_first() {
        let (mut lock, dir) = lock("remove");
        lock.store.set_pin(b"8642").unwrap();
        lock.store.set_mode(Mode::Streams);
        lock.open_settings();
        assert!(!lock.page && matches!(lock.pad.as_ref().unwrap().purpose, Purpose::Verify(After::Open)));
        type_pin(&mut lock, "8642");
        finish(&mut lock);
        assert!(lock.page);
        lock.pad = Some(Pad::new(Purpose::Verify(After::Remove), &lock.store));
        type_pin(&mut lock, "8642");
        finish(&mut lock);
        assert!(!lock.store.has_pin() && lock.store.mode() == Mode::Off);
        std::fs::remove_dir_all(dir).ok();
    }
}
