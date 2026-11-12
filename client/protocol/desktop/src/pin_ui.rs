//! The PIN screen and the PIN lock settings page, as Nightjar Android's PinLockActivity and
//! PinLockSettingsActivity show them. The checks and the hashing are in pin_lock.rs; the slow
//! ones (about 150 ms, measured per computer) run on a worker thread while the screen says so.

use std::path::Path;
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::{vec2, Align, Key, Layout, Rect, RichText, Ui, Vec2};

use crate::pin_lock::{Mode, Outcome, PinGate, PinStore, Problem, GRACE_CHOICES, MAX_DIGITS, MIN_DIGITS};
use crate::strings::{en, tr, trf};
use crate::ui::{design_tokens, glass, glass_behind, pin_key, pin_slots, round_button, Icon, Tone, DIALOG_MARGIN};
use crate::theme::*;
use crate::ui::{column, group_row, page_scroll_from, primary_button, quiet_button, row, section, toggle, Combo, Command, ToTop, BACK_SIZE, PAGE_TITLE, SETTINGS_W};

/// What the PIN screen is up for.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Purpose {
    /// The app is locked; the screen covers it and only closing Nightjar gets past it.
    App,
    /// Starting a stream on the PC with the key `key` (pin_lock::stream_key), called `pc`.
    Stream { key: String, pc: String },
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
    /// The page just opened: its first action takes the focus.
    focus_page: bool,
    job: Option<Receiver<Done>>,
    notice: Option<&'static str>,
}

impl PinLock {
    pub fn open(dir: &Path) -> Self {
        Self::with_store(PinStore::new(dir, Arc::new(crate::pin_lock::SystemClock)))
    }

    /// A PIN screen for the demo's shots, over an empty store in `dir`.
    #[cfg(any(test, feature = "demo-library"))]
    pub(crate) fn demo(dir: &Path, choose: bool, pc: &str) -> Self {
        let mut lock = Self::open(dir);
        let purpose = if choose { Purpose::Choose } else { Purpose::Stream { key: String::new(), pc: pc.to_owned() } };
        lock.pad = Some(Pad::new(purpose, &lock.store));
        lock
    }

    /// The settings page for the demo's shots, over a new store in `dir`: with a PIN and `mode`, or
    /// with no PIN.
    #[cfg(any(test, feature = "demo-library"))]
    pub(crate) fn demo_page(dir: &Path, mode: Option<Mode>) -> Self {
        std::fs::remove_dir_all(dir).ok();
        std::fs::create_dir_all(dir).ok();
        let store = PinStore::new(dir, Arc::new(crate::pin_lock::SystemClock));
        if let Some(mode) = mode {
            store.set_pin(b"1234").expect("the demo's PIN is valid");
            store.set_mode(mode);
        }
        let mut lock = Self::with_store(store);
        lock.pad = None;
        lock.gate.unlock_app();
        lock.show_page();
        lock
    }

    /// The second screen's lock: open when `hand_over` is the token the first screen's process
    /// wrote on this process's input ([`PinStore::hand_over`]), else as [`Self::open`].
    pub fn open_second_screen(dir: &Path, hand_over: Option<&str>) -> Self {
        let store = PinStore::new(dir, Arc::new(crate::pin_lock::SystemClock));
        let mut gate = PinGate::new(&store);
        match hand_over {
            Some(token) if store.handed_over(token) => gate.hand_over(),
            _ if !store.has_pin() => {}
            Some(token) if !token.is_empty() => eprintln!("nightjar: second screen: the first screen's hand-over of the PIN lock is not valid (late or rewritten); the lock asks for the PIN"),
            _ => eprintln!("nightjar: second screen: the first screen handed over no PIN lock; the lock asks for the PIN"),
        }
        Self::with_gate(store, gate)
    }

    pub fn with_store(store: PinStore) -> Self {
        let gate = PinGate::new(&store);
        Self::with_gate(store, gate)
    }

    fn with_gate(store: PinStore, gate: PinGate) -> Self {
        let mut lock = Self { store, gate, pad: None, page: false, focus_page: false, job: None, notice: None };
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

    /// Waits for the lock's file to be written: Nightjar is closing.
    pub fn flush(&self) {
        self.store.flush();
    }

    /// See [`PinStore::hand_over`].
    pub fn hand_over(&self) -> Result<Option<String>, String> {
        self.store.hand_over()
    }

    /// See [`PinStore::end_hand_over`].
    pub fn end_hand_over(&self) {
        self.store.end_hand_over();
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

    /// Whether starting a stream under `key` (pin_lock::stream_key) has to wait for the PIN; a
    /// grant is used up here.
    pub fn needs_pin_for_stream(&mut self, key: &str) -> bool {
        self.gate.needs_pin_for_stream(&self.store, key)
    }

    /// See [`crate::pin_lock::PinGate::locks_stream`].
    pub fn locks_stream(&self, key: &str) -> bool {
        self.gate.locks_stream(&self.store, key)
    }

    /// See [`crate::pin_lock::PinGate::forget_grant`].
    pub fn forget_grant(&mut self) {
        self.gate.forget_grant();
    }

    /// Asks for the PIN to start a stream under `key` (`pc` names the PC). Under the app lock its
    /// screen asks already, and unlocking the app lets the start go on.
    pub fn ask_for_stream(&mut self, key: &str, pc: &str) {
        if !self.app_locked() {
            self.pad = Some(Pad::new(Purpose::Stream { key: key.to_owned(), pc: pc.to_owned() }, &self.store));
        }
    }

    /// The settings row: the page, after the PIN when one is set.
    pub fn open_settings(&mut self) {
        self.notice = None;
        if self.store.has_pin() {
            self.pad = Some(Pad::new(Purpose::Verify(After::Open), &self.store));
        } else {
            self.show_page();
        }
    }

    fn show_page(&mut self) {
        self.page = true;
        self.focus_page = true;
    }

    /// Draws the PIN screen or the settings page when one is up, over everything else, and says
    /// whether it did. `pcs` are the saved PCs as (unique id, name).
    pub fn ui(&mut self, ui: &mut Ui, pcs: &[(String, String)], commands: &mut Vec<Command>) -> bool {
        self.poll(commands);
        // A lock file that could not be read, read again and found without the app lock, or
        // deleted as the lock screen said, opens the app.
        if self.app_locked() && self.job.is_none() && !self.gate.needs_pin_for_app(&self.store) {
            self.pad = None;
            commands.push(Command::PinPassed);
        }
        if self.pad.is_none() && !self.page {
            return false;
        }
        if self.pad.is_some() {
            self.pad_ui(ui, commands);
        } else {
            self.page_ui(ui, pcs);
        }
        true
    }

    /// Whether the PIN screen or the settings page is up: the sky goes behind them.
    pub fn up(&self) -> bool {
        self.pad.is_some() || self.page
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
                    Purpose::Stream { key, .. } => {
                        self.gate.grant(&self.store, &key);
                        commands.push(Command::PinPassed);
                    }
                    Purpose::Verify(After::Open) => self.show_page(),
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
            // The screen says why (problem_text).
            Done::Tried(Outcome::Unavailable) => pad.clear(),
            Done::Saved(Ok(())) => {
                self.pad = None;
                if self.store.mode() == Mode::Off {
                    // A PIN that locks nothing would surprise: start with the app itself.
                    self.store.set_mode(Mode::App);
                }
                self.gate.unlock_app();
                self.notice = Some(en("PIN saved"));
                self.show_page();
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
        // While the file cannot be used no PIN can match: the screen says why instead.
        if pad.checks() && self.store.problem().is_some() {
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
            Purpose::App => (tr("Nightjar is locked").to_owned(), None),
            Purpose::Stream { pc, .. } if !pc.is_empty() => (pc.clone(), Some(tr("PIN"))),
            Purpose::Stream { .. } | Purpose::Verify(_) => (tr("Enter your PIN").to_owned(), None),
            Purpose::Choose => (tr("Choose a PIN").to_owned(), Some(tr("Use 4 to 8 digits"))),
            Purpose::Confirm(_) => (tr("Confirm your PIN").to_owned(), None),
        };
        let problem = if pad.checks() { self.store.problem() } else { None };
        let status = if self.job.is_some() {
            Some((if pad.checks() { tr("Checking…") } else { tr("Saving…") }.to_owned(), false))
        } else if let Some(problem) = &problem {
            Some((problem_text(problem, self.store.path()), true))
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

        // The panel in the middle of the sky; in a short window its keys and slots give way.
        let screen = ui.max_rect();
        glass::boundary(ui.painter(), vec![ui.ctx().content_rect()]);
        let edge = DIALOG_MARGIN + space::M;
        let head = TITLE_ROW + if subtitle.is_some() { SUBTITLE_ROW } else { 0.0 } + space::M;
        let spare = screen.height() - 2.0 * edge - head - STATUS_ROW;
        let slot_height = (spare * 0.2).clamp(52.0, 76.0);
        let key_height = ((spare - slot_height - 3.0 * KEY_GAP) / 4.0).clamp(40.0, 64.0);
        let height = head + slot_height + STATUS_ROW + 4.0 * key_height + 3.0 * KEY_GAP;
        let width = PANEL_WIDTH.min(screen.width() - 2.0 * edge);
        let inner = Rect::from_center_size(screen.center(), vec2(width, height));
        let mut pressed = Vec::new();
        let mut first_key = None;
        let mut panel = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(Layout::top_down(Align::Center)));
        glass_behind(&mut panel, glass::Material::Panel, design_tokens::RADIUS_PANEL, DIALOG_MARGIN, |ui| {
            ui.set_min_width(inner.width());
            ui.spacing_mut().item_spacing.y = 0.0;
            // Clear of the round close at the panel's corner.
            let clear = inner.width() - 2.0 * CLOSE_SIZE.max(space::XL);
            ui.allocate_ui_with_layout(vec2(clear, TITLE_ROW), Layout::top_down(Align::Center), |ui| {
                ui.add(egui::Label::new(RichText::new(title).font(semibold(text::SECTION)).color(ON_SURFACE)).truncate());
            });
            if let Some(subtitle) = subtitle {
                ui.allocate_ui_with_layout(vec2(clear, SUBTITLE_ROW), Layout::top_down(Align::Center), |ui| {
                    ui.label(RichText::new(subtitle).font(regular(text::BODY)).color(ON_SURFACE_VARIANT));
                });
            }
            ui.add_space(space::M);
            let digits = vec!['*'; entered];
            pin_slots(ui, &digits, full, true, slot_height, &trf("{} of {} digits entered", &[&entered, &full]));
            ui.add_space(STATUS_GAP);
            let (words, error) = status.unwrap_or_default();
            ui.allocate_ui_with_layout(vec2(inner.width(), STATUS_ROW - 2.0 * STATUS_GAP), Layout::top_down(Align::Center), |ui| {
                ui.label(RichText::new(words).font(regular(text::LABEL)).color(if error { ERROR } else { ON_SURFACE_VARIANT }));
            });
            ui.add_space(STATUS_GAP);
            let key_width = (inner.width() - 2.0 * KEY_GAP) / 3.0;
            let key_size = vec2(key_width, key_height);
            let rows: [[PadKey; 3]; 4] = [
                [PadKey::Digit(1), PadKey::Digit(2), PadKey::Digit(3)],
                [PadKey::Digit(4), PadKey::Digit(5), PadKey::Digit(6)],
                [PadKey::Digit(7), PadKey::Digit(8), PadKey::Digit(9)],
                [PadKey::Confirm, PadKey::Digit(0), PadKey::Delete],
            ];
            for (n, row) in rows.into_iter().enumerate() {
                if n > 0 {
                    ui.add_space(KEY_GAP);
                }
                ui.allocate_ui_with_layout(vec2(inner.width(), key_height), Layout::left_to_right(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = KEY_GAP;
                    for key in row {
                        let response = match key {
                            PadKey::Digit(d) => {
                                let word = d.to_string();
                                pin_key(ui, None, &word, &word, Tone::Quiet, key_size)
                            }
                            PadKey::Delete => pin_key(ui, Some(Icon::Backspace), "", tr("Delete"), Tone::Quiet, key_size),
                            // The one primary, once there is a PIN to confirm.
                            PadKey::Confirm => {
                                let tone = if entered >= full { Tone::Primary } else { Tone::Quiet };
                                pin_key(ui, Some(Icon::Check), "", tr("Confirm"), tone, key_size)
                            }
                            PadKey::Cancel | PadKey::FocusFirst => unreachable!("not on the pad"),
                        };
                        if key == PadKey::Digit(1) {
                            first_key = Some(response.id);
                        }
                        if response.clicked() {
                            pressed.push(key);
                        }
                    }
                });
            }
        });
        let corner = panel.min_rect().expand(DIALOG_MARGIN).right_top() + vec2(-CLOSE_INSET - CLOSE_SIZE, CLOSE_INSET);
        let close = Rect::from_min_size(corner, Vec2::splat(CLOSE_SIZE));
        let said = if can_cancel { tr("Cancel") } else { tr("Close Nightjar") };
        if round_button(ui, close, Icon::Close, said).clicked() {
            if can_cancel {
                pressed.push(PadKey::Cancel);
            } else {
                commands.push(Command::Quit);
            }
        }

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

    /// The settings page, a settings sub-page in look: its head, then rows on the sky.
    fn page_ui(&mut self, ui: &mut Ui, pcs: &[(String, String)]) {
        let view = self.store.view();
        // With a list open, Escape (B) only closes the list.
        let mut close = ui.input(|i| i.key_pressed(Key::Escape)) && !egui::Popup::is_any_open(ui.ctx());
        let focus = std::mem::take(&mut self.focus_page);
        page_scroll_from(ui, "pin-lock-page", ToTop::No, PAGE_HEAD_TOP, |ui| {
            column(ui, SETTINGS_W, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 14.0;
                    ui.set_min_height(BACK_SIZE);
                    let (slot, _) = ui.allocate_exact_size(Vec2::splat(BACK_SIZE), egui::Sense::hover());
                    if round_button(ui, slot, Icon::Back, tr("Back to Settings")).clicked() {
                        close = true;
                    }
                    ui.label(RichText::new(tr("PIN lock")).font(semibold(PAGE_TITLE)).color(PAPER));
                });
                ui.add_space(space::L);
                if let Some(notice) = self.notice {
                    ui.label(RichText::new(tr(notice)).font(regular(text::LABEL)).color(OK));
                    ui.add_space(space::S);
                }
                self.pin_row(ui, view.has_pin, focus);
                if view.has_pin {
                    self.lock_rows(ui, &view, pcs);
                    ui.add_space(space::M);
                    ui.label(RichText::new(tr("If you forget the PIN, the only way to reset it is to delete pin_lock.json in Nightjar’s settings folder, next to settings.json.")).font(regular(text::CAPTION)).color(ON_SURFACE_VARIANT));
                }
            });
        });
        if close {
            self.page = false;
            self.notice = None;
        }
    }

    /// The PIN itself: setting one, or changing and removing it.
    fn pin_row(&mut self, ui: &mut Ui, has_pin: bool, focus: bool) {
        let about = if has_pin { NO_LINE } else { en("4 to 8 digits") };
        row(ui, "PIN", about, |ui| {
            ui.horizontal(|ui| {
                if !has_pin {
                    let set = primary_button(ui, "Set PIN");
                    if focus {
                        set.request_focus();
                    }
                    if set.clicked() {
                        self.notice = None;
                        self.pad = Some(Pad::new(Purpose::Choose, &self.store));
                    }
                    return set;
                }
                let change = quiet_button(ui, "Change PIN");
                if focus {
                    change.request_focus();
                }
                if change.clicked() {
                    self.notice = None;
                    self.pad = Some(Pad::new(Purpose::Verify(After::Change), &self.store));
                }
                let remove = quiet_button(ui, "Remove PIN");
                if remove.clicked() {
                    self.notice = None;
                    self.pad = Some(Pad::new(Purpose::Verify(After::Remove), &self.store));
                }
                remove
            })
            .inner
        });
    }

    /// What the PIN locks, and for how long a passed PIN holds.
    fn lock_rows(&mut self, ui: &mut Ui, view: &crate::pin_lock::View, pcs: &[(String, String)]) {
        row(ui, "Lock", NO_LINE, |ui| {
            let mut mode = view.mode;
            let shown = Combo::from_id_salt("pin-mode").selected_text(tr(mode_label(mode))).show_ui(ui, |ui| {
                for choice in Mode::ALL {
                    ui.selectable_value(&mut mode, choice, tr(mode_label(choice)));
                }
            });
            if mode != view.mode {
                self.store.set_mode(mode);
                // Turning the app lock on while using the app must not lock it on the spot.
                self.gate.unlock_app();
            }
            shown.response
        });
        if view.mode == Mode::App {
            row(ui, "Ask again after", NO_LINE, |ui| {
                let mut grace = view.grace_minutes;
                let shown = Combo::from_id_salt("pin-grace").selected_text(grace_label(grace)).show_ui(ui, |ui| {
                    for choice in GRACE_CHOICES {
                        ui.selectable_value(&mut grace, choice, grace_label(choice));
                    }
                });
                if grace != view.grace_minutes {
                    self.store.set_grace_minutes(grace);
                }
                shown.response
            });
        }
        if view.mode == Mode::Selected {
            section(ui, "Selected PCs");
            let named: Vec<_> = pcs.iter().filter(|(uuid, _)| !uuid.is_empty()).collect();
            if named.is_empty() {
                group_row(ui, |ui| {
                    ui.label(RichText::new(tr("No PCs added yet.")).font(regular(text::BODY)).color(ON_SURFACE_VARIANT));
                });
            }
            for (uuid, name) in named {
                row(ui, name, NO_LINE, |ui| {
                    let mut on = crate::pin_lock::contains(&view.pcs, uuid);
                    let switch = toggle(ui, &mut on);
                    if switch.changed() {
                        let mut chosen: Vec<String> = view.pcs.iter().filter(|pc| !pc.eq_ignore_ascii_case(uuid)).cloned().collect();
                        if on {
                            chosen.push(uuid.clone());
                        }
                        self.store.set_pcs(chosen);
                    }
                    switch
                });
            }
        }
    }
}

/// A row with its label alone.
const NO_LINE: &str = "";
/// How far down the page's head starts: this page has no top bar over it.
const PAGE_HEAD_TOP: f32 = 32.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PadKey {
    Digit(u8),
    Delete,
    Confirm,
    Cancel,
    FocusFirst,
}

/// The PIN screen's panel, inside its margin, and its rows.
const PANEL_WIDTH: f32 = 360.0;
const TITLE_ROW: f32 = 34.0;
const SUBTITLE_ROW: f32 = 24.0;
const STATUS_GAP: f32 = 8.0;
const STATUS_ROW: f32 = 20.0 + 2.0 * STATUS_GAP;
const KEY_GAP: f32 = 12.0;
const CLOSE_SIZE: f32 = 40.0;
const CLOSE_INSET: f32 = 14.0;

/// What the lock screen says while the lock's file cannot be used, with its full path so it can
/// be found and deleted.
fn problem_text(problem: &Problem, path: &Path) -> String {
    let path = path.display().to_string();
    match problem {
        Problem::Damaged(_) => trf("The PIN lock file is damaged: {}. Delete this file to remove the PIN lock.", &[&path]),
        Problem::Unreadable(why) => trf("The PIN lock file cannot be read right now ({}): {}. Nightjar tries again; delete this file to remove the PIN lock.", &[why, &path]),
    }
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
    fn a_damaged_lock_file_is_named_on_the_lock_screen_and_takes_no_tries() {
        let dir = std::env::temp_dir().join(format!("nightjar-pin-ui-damaged-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("pin_lock.json"), "{ not json").unwrap();
        let mut lock = PinLock::with_store(PinStore::new(&dir, Arc::new(Still(AtomicI64::new(5_000_000)))));
        assert!(lock.app_locked());
        let problem = lock.store.problem().expect("damaged");
        let said = problem_text(&problem, lock.store.path());
        assert!(said.contains(&dir.join("pin_lock.json").display().to_string()), "the full path: {said}");
        assert!(said.contains("Delete this file to remove the PIN lock"), "{said}");
        type_pin(&mut lock, "1234");
        lock.key(PadKey::Confirm);
        assert!(lock.job.is_none() && lock.pad.as_ref().unwrap().digits.is_empty(), "no try is made or counted");
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
