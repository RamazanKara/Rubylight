//! The menu's pages and focus (`docs/couch-menu-design.md` section 4.1). The
//! host fills a [`Snapshot`] (readings, values, availability) and runs the
//! [`Action`]s; the model only turns [`Nav`] into focus changes and actions,
//! so it holds no feature logic.
use std::time::{Duration, Instant};

use super::Nav;
use super::font;
use super::layout::{COLUMNS, Hit, Layout, MAX_LINES};
use crate::native_api::radeon::{self, Change, ControlState, Kind, Value};

/// How long End session and Close game wait for the second press.
pub const CONFIRM: Duration = Duration::from_secs(5);
/// Text lines a page has for its rows and their notes; the rest scroll.
pub const ROW_LINES: usize = MAX_LINES as usize - 4;

/// Keys of the extension rows (section 4.3). The host's registry implements
/// each one with that feature's own getter and setter; an item's label is
/// the catalogue string at [`items::label_path`].
pub mod items {
    pub const AUDIO_NIGHT_MODE: &str = "audio_night_mode";
    pub const AUDIO_HEADPHONE_SURROUND: &str = "audio_headphone_surround";
    /// Built from `native_api::stream_settings::read` with
    /// [`super::Setting::from_radeon`]; its change goes to
    /// `stream_settings::write`.
    pub const ENCODER_FOCUS: &str = "encoder_focus";
    pub const SLEEP_AFTER_IDLE_MINUTES: &str = "sleep_after_idle_minutes";
    /// The choices of `sleep_after_idle_minutes`, as `Value::Choice` values.
    pub const SLEEP_CHOICES: [&str; 5] = ["off", "15", "30", "60", "120"];
    pub const ALL: [&str; 4] = [
        AUDIO_NIGHT_MODE,
        AUDIO_HEADPHONE_SURROUND,
        ENCODER_FOCUS,
        SLEEP_AFTER_IDLE_MINUTES,
    ];

    /// The catalogue path of an item's label.
    pub fn label_path(key: &str) -> String {
        format!("couch_menu/items/{key}")
    }
}

macro_rules! strings {
    ($($(#[$doc:meta])* $field:ident,)*) => {
        /// The menu's own text, from the host catalogue (`couch_menu/<key>`).
        /// Labels of settings and items come with the [`Snapshot`].
        #[derive(Clone, Debug, PartialEq, Eq)]
        pub struct Strings {
            $($(#[$doc])* pub $field: String,)*
        }

        impl Strings {
            pub const KEYS: &[&str] = &[$(stringify!($field)),*];

            /// Reads every key from `lookup(path)`. Fails on a missing string
            /// or one the atlas cannot draw: the catalogue must be Latin-1.
            pub fn from_lookup(
                mut lookup: impl FnMut(&str) -> Option<String>,
            ) -> Result<Self, String> {
                Ok(Self {
                    $($field: checked(&mut lookup, stringify!($field))?,)*
                })
            }
        }
    };
}

strings! {
    /// "Host menu"
    title,
    /// "Back to game"
    back_to_game,
    /// "Radeon settings", the main-page row and the page title.
    radeon,
    /// "Restore Radeon settings"
    restore_radeon,
    /// "Back", the first row of the Radeon page.
    back,
    /// "Profile: ", before an active `radeon_profile`.
    profile,
    /// "Disconnect (game keeps running)"
    disconnect,
    /// "End session"
    end_session,
    /// "Close game"
    close_game,
    /// "Applies from the next stream"
    next_stream,
    /// "Press again within 5 s to confirm"
    confirm,
    /// "Working..."
    working,
    /// "Failed: ", before the English failure message.
    failed,
    /// "Needs host control"
    needs_host_control,
    /// "On"
    on,
    /// "Off"
    off,
    /// "Enter/A: select  Esc/B: back"
    hint,
}

fn checked(lookup: &mut impl FnMut(&str) -> Option<String>, key: &str) -> Result<String, String> {
    let path = format!("couch_menu/{key}");
    let text = lookup(&path).ok_or_else(|| format!("couch menu: string {path} is missing"))?;
    if let Some(c) = font::uncovered(&text) {
        return Err(format!(
            "couch menu: string {path} has {c:?}, outside Latin-1"
        ));
    }
    Ok(text)
}

/// Whether a row shows, and why it cannot change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Availability {
    Ready,
    /// Shown in grey; the reason shows when the row has focus.
    Locked(String),
    /// Not shown.
    Hidden,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Applies {
    Live,
    /// The row carries an "Applies from the next stream" note.
    NextStream,
}

/// What a value row shows and how Left, Right and Accept change it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Control {
    Toggle(bool),
    Choice {
        index: usize,
        options: Vec<radeon::Choice>,
    },
    Integer {
        value: i64,
        range: radeon::Range,
        unit: Option<String>,
    },
    /// Shown, never changed: a value of a shape this version does not know.
    Text(String),
}

/// A value row: a Radeon setting, the stream preset or an extension item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Setting {
    /// The id its [`Change`] carries: the Radeon or stream setting id, or
    /// the item key.
    pub id: String,
    pub label: String,
    pub control: Control,
    pub availability: Availability,
    pub applies: Applies,
    /// Locked with [`Strings::needs_host_control`] unless the caller has it.
    pub needs_host_control: bool,
}

impl Setting {
    /// A row for a native-API setting (`/radeon/settings` or a
    /// `stream_settings` entry), rendered generically by `kind`. Any state
    /// but `configurable` locks it with the host's reason.
    pub fn from_radeon(setting: &radeon::Setting, label: String) -> Self {
        let control = match (setting.kind, &setting.value, setting.range) {
            (Kind::Toggle, Some(Value::Bool(on)), _) => Control::Toggle(*on),
            (Kind::Integer, Some(Value::Integer(value)), Some(range)) => Control::Integer {
                value: *value,
                range,
                unit: setting.unit.clone(),
            },
            (Kind::Choice, Some(Value::Choice(value)), _) => {
                match setting.options.iter().position(|o| &o.value == value) {
                    Some(index) => Control::Choice {
                        index,
                        options: setting.options.clone(),
                    },
                    None => Control::Text(value.clone()),
                }
            }
            (_, Some(Value::Bool(on)), _) => Control::Text(on.to_string()),
            (_, Some(Value::Integer(value)), _) => Control::Text(value.to_string()),
            (_, Some(Value::Choice(value)), _) => Control::Text(value.clone()),
            (_, None, _) => Control::Text(String::new()),
        };
        let availability = match setting.state {
            ControlState::Configurable => Availability::Ready,
            state => Availability::Locked(
                setting
                    .reason
                    .clone()
                    .or_else(|| setting.message.clone())
                    .unwrap_or_else(|| format!("{state:?}")),
            ),
        };
        Self {
            id: setting.id.clone(),
            label,
            control,
            availability,
            applies: Applies::Live,
            needs_host_control: true,
        }
    }
}

/// The Radeon page's content (`current_view`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RadeonPage {
    pub gpu: i32,
    pub name: String,
    /// The active `radeon_profile`, named on the page.
    pub profile: Option<String>,
    pub settings: Vec<Setting>,
}

/// What the host knows when it draws a page; rebuilt on each change and once
/// a second while open.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    /// The main page's read-only line: GPU readings and stream mode, or why
    /// readings are off.
    pub status: String,
    /// The caller has `host_control`.
    pub host_control: bool,
    /// The "Radeon settings" row and page, if `radeon.read` applies.
    pub radeon: Option<RadeonPage>,
    /// The stream preset row (always [`Applies::NextStream`]).
    pub preset: Option<Setting>,
    /// Extension rows, in registry order.
    pub items: Vec<Setting>,
}

/// Where a change goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Radeon {
        gpu: i32,
    },
    /// The device's `stream_preset` override.
    Preset,
    /// The registry item whose key is the change's id.
    Item,
}

/// What the host should do. Every action but [`Action::Close`] keeps the menu
/// busy until [`Menu::finish`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Back to game: `Shared::close`.
    Close,
    Disconnect,
    EndSession,
    CloseGame,
    Change {
        target: Target,
        change: Change,
    },
    RestoreRadeon {
        gpu: i32,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Page {
    #[default]
    Main,
    Radeon,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum RowId {
    BackToGame,
    Radeon,
    Preset,
    Item(String),
    Disconnect,
    End,
    CloseGame,
    BackToMain,
    RadeonSetting(String),
    Restore,
}

struct Row<'a> {
    id: RowId,
    label: &'a str,
    setting: Option<&'a Setting>,
    locked: Option<&'a str>,
}

/// How a line is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Style {
    Title,
    Info,
    Row { focus: bool, locked: bool },
    Note,
    Message,
    Error,
    Hint,
}

/// One text line of a page, at most [`COLUMNS`] characters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Line {
    pub text: String,
    pub style: Style,
    /// The row this line belongs to, for taps.
    row: Option<usize>,
}

/// The open menu: its strings and [`State`]. The menu worker owns it.
pub struct Menu {
    strings: Strings,
    state: State,
}

/// Page, focus, the pending confirm, the running action and the last failure.
#[derive(Debug, Default)]
struct State {
    page: Page,
    focus: Option<RowId>,
    confirm: Option<(RowId, Instant)>,
    busy: Option<RowId>,
    error: Option<String>,
}

impl Menu {
    pub fn new(strings: Strings) -> Self {
        Self {
            strings,
            state: State::default(),
        }
    }
    pub fn strings(&self) -> &Strings {
        &self.strings
    }
    /// Back to the main page with nothing pending, as on open and close.
    pub fn reset(&mut self) {
        self.state = State::default();
    }
    /// An action is running.
    pub fn busy(&self) -> bool {
        self.state.busy.is_some()
    }
    /// Applies one navigation. `layout` maps taps and clicks to lines.
    pub fn nav(
        &mut self,
        nav: Nav,
        now: Instant,
        snap: &Snapshot,
        layout: &Layout,
    ) -> Option<Action> {
        self.tick(now);
        let state = &mut self.state;
        if state.page == Page::Radeon && snap.radeon.is_none() {
            state.page = Page::Main;
            state.focus = Some(RowId::Radeon);
        }
        let rows = rows(&self.strings, state.page, snap);
        let focus = state.focus_index(&rows);
        let mut index = focus;
        let action = match nav {
            Nav::Opened | Nav::Closed => {
                *state = State::default();
                return None;
            }
            Nav::Up => {
                index = (focus + rows.len() - 1) % rows.len();
                None
            }
            Nav::Down => {
                index = (focus + 1) % rows.len();
                None
            }
            Nav::Left => state.step(&rows[focus], -1, snap),
            Nav::Right => state.step(&rows[focus], 1, snap),
            Nav::Accept => state.accept(&rows[focus], now, snap),
            Nav::Back => match state.page {
                Page::Main => Some(Action::Close),
                Page::Radeon => {
                    *state = State {
                        focus: Some(RowId::Radeon),
                        ..State::default()
                    };
                    return None;
                }
            },
            Nav::Point { x, y } => {
                let lines = lines(&self.strings, state, snap, &rows);
                match layout.hit(lines.len() as u32, x, y) {
                    Hit::Outside => Some(Action::Close),
                    Hit::Panel => None,
                    Hit::Line(line) => match lines[line as usize].row {
                        Some(row) => {
                            index = row;
                            if row != focus {
                                state.confirm = None;
                                state.focus = Some(rows[row].id.clone());
                            }
                            state.accept(&rows[row], now, snap)
                        }
                        None => None,
                    },
                }
            }
        };
        if index != focus && !matches!(nav, Nav::Point { .. }) {
            state.focus = Some(rows[index].id.clone());
            state.confirm = None;
        }
        if let Some(action) = &action
            && *action != Action::Close
        {
            state.busy = Some(rows[index].id.clone());
            state.error = None;
            state.confirm = None;
        }
        action
    }
    /// The running action finished; a failure shows until the next action.
    pub fn finish(&mut self, result: Result<(), String>) {
        self.state.busy = None;
        self.state.error = result.err();
    }
    /// Expires a confirm that waited [`CONFIRM`]. `true` if the page changed.
    pub fn tick(&mut self, now: Instant) -> bool {
        if let Some((_, armed)) = &self.state.confirm
            && now >= *armed + CONFIRM
        {
            self.state.confirm = None;
            return true;
        }
        false
    }
    /// When [`Menu::tick`] has something to do.
    pub fn next_due(&self) -> Option<Instant> {
        self.state
            .confirm
            .as_ref()
            .map(|(_, armed)| *armed + CONFIRM)
    }
    /// The page as text lines: title, info line, rows (scrolled to keep the
    /// focus in view), message line and hint.
    pub(crate) fn lines(&self, snap: &Snapshot) -> Vec<Line> {
        if self.state.page == Page::Radeon && snap.radeon.is_none() {
            let main = State {
                focus: Some(RowId::Radeon),
                ..State::default()
            };
            return lines(
                &self.strings,
                &main,
                snap,
                &rows(&self.strings, Page::Main, snap),
            );
        }
        lines(
            &self.strings,
            &self.state,
            snap,
            &rows(&self.strings, self.state.page, snap),
        )
    }
}

impl State {
    fn focus_index(&self, rows: &[Row]) -> usize {
        self.focus
            .as_ref()
            .and_then(|id| rows.iter().position(|row| row.id == *id))
            .unwrap_or(0)
    }
    /// Left (-1) or Right (+1) on a value row; the ends stop.
    fn step(&mut self, row: &Row, by: i64, snap: &Snapshot) -> Option<Action> {
        if self.busy.is_some() || row.locked.is_some() {
            return None;
        }
        let setting = row.setting?;
        let value = match &setting.control {
            Control::Toggle(on) => (*on != (by > 0)).then_some(Value::Bool(by > 0))?,
            Control::Choice { index, options } => {
                let next = index.checked_add_signed(by as isize)?;
                Value::Choice(options.get(next)?.value.clone())
            }
            Control::Integer { value, range, .. } => {
                let next = value
                    .saturating_add(by * range.step.max(1))
                    .max(range.min)
                    .min(range.max);
                (next != *value).then_some(Value::Integer(next))?
            }
            Control::Text(_) => return None,
        };
        change(row, setting, value, snap)
    }
    fn accept(&mut self, row: &Row, now: Instant, snap: &Snapshot) -> Option<Action> {
        if self.busy.is_some() || row.locked.is_some() {
            return None;
        }
        if let Some(setting) = row.setting {
            // Accept (and a tap) moves a value forward, wrapping, so touch
            // can change every row.
            let value = match &setting.control {
                Control::Toggle(on) => Value::Bool(!on),
                Control::Choice { index, options } => Value::Choice(
                    options
                        .get((index + 1) % options.len().max(1))?
                        .value
                        .clone(),
                ),
                Control::Integer { value, range, .. } => Value::Integer(if *value >= range.max {
                    range.min
                } else {
                    value.saturating_add(range.step.max(1)).min(range.max)
                }),
                Control::Text(_) => return None,
            };
            return change(row, setting, value, snap);
        }
        match row.id {
            RowId::BackToGame => Some(Action::Close),
            RowId::Radeon => {
                *self = State {
                    page: Page::Radeon,
                    ..State::default()
                };
                None
            }
            RowId::BackToMain => {
                *self = State {
                    focus: Some(RowId::Radeon),
                    ..State::default()
                };
                None
            }
            RowId::Disconnect => Some(Action::Disconnect),
            RowId::End | RowId::CloseGame => {
                if let Some((id, armed)) = &self.confirm
                    && *id == row.id
                    && now < *armed + CONFIRM
                {
                    return Some(match row.id {
                        RowId::End => Action::EndSession,
                        _ => Action::CloseGame,
                    });
                }
                self.confirm = Some((row.id.clone(), now));
                None
            }
            RowId::Restore => Some(Action::RestoreRadeon {
                gpu: snap.radeon.as_ref()?.gpu,
            }),
            RowId::Preset | RowId::Item(_) | RowId::RadeonSetting(_) => None,
        }
    }
}

fn change(row: &Row, setting: &Setting, value: Value, snap: &Snapshot) -> Option<Action> {
    let target = match row.id {
        RowId::Preset => Target::Preset,
        RowId::Item(_) => Target::Item,
        RowId::RadeonSetting(_) => Target::Radeon {
            gpu: snap.radeon.as_ref()?.gpu,
        },
        _ => return None,
    };
    Some(Action::Change {
        target,
        change: Change {
            id: setting.id.clone(),
            value,
        },
    })
}

fn rows<'a>(s: &'a Strings, page: Page, snap: &'a Snapshot) -> Vec<Row<'a>> {
    let plain = |id, label| Row {
        id,
        label,
        setting: None,
        locked: None,
    };
    let setting = |id, setting: &'a Setting| Row {
        id,
        label: &setting.label,
        setting: Some(setting),
        locked: match &setting.availability {
            Availability::Locked(reason) => Some(reason),
            _ if setting.needs_host_control && !snap.host_control => Some(&s.needs_host_control),
            _ => None,
        },
    };
    let shown = |row: &&Setting| row.availability != Availability::Hidden;
    let host_control = (!snap.host_control).then_some(s.needs_host_control.as_str());
    let mut rows = Vec::new();
    match (page, &snap.radeon) {
        (Page::Radeon, Some(page)) => {
            rows.push(plain(RowId::BackToMain, &s.back));
            for row in page.settings.iter().filter(shown) {
                rows.push(setting(RowId::RadeonSetting(row.id.clone()), row));
            }
            rows.push(Row {
                locked: host_control,
                ..plain(RowId::Restore, &s.restore_radeon)
            });
        }
        _ => {
            rows.push(plain(RowId::BackToGame, &s.back_to_game));
            if snap.radeon.is_some() {
                rows.push(plain(RowId::Radeon, &s.radeon));
            }
            if let Some(preset) = snap.preset.as_ref().filter(shown) {
                rows.push(setting(RowId::Preset, preset));
            }
            for item in snap.items.iter().filter(shown) {
                rows.push(setting(RowId::Item(item.id.clone()), item));
            }
            rows.push(plain(RowId::Disconnect, &s.disconnect));
            rows.push(plain(RowId::End, &s.end_session));
            rows.push(Row {
                locked: host_control,
                ..plain(RowId::CloseGame, &s.close_game)
            });
        }
    }
    rows
}

fn lines(s: &Strings, state: &State, snap: &Snapshot, rows: &[Row]) -> Vec<Line> {
    let line = |text: &str, style| Line {
        text: fit(text, COLUMNS as usize),
        style,
        row: None,
    };
    let focus = state.focus_index(rows);
    let (title, info) = match (state.page, &snap.radeon) {
        (Page::Radeon, Some(page)) => (
            &s.radeon,
            match &page.profile {
                Some(profile) => format!("{}{profile}", s.profile),
                None => page.name.clone(),
            },
        ),
        _ => (&s.title, snap.status.clone()),
    };
    let mut body = Vec::new();
    let mut focus_end = 0;
    for (index, row) in rows.iter().enumerate() {
        let value = if state.busy.as_ref() == Some(&row.id) {
            s.working.clone()
        } else {
            match row.setting {
                Some(setting) => {
                    let value = shown(s, &setting.control);
                    if index == focus && row.locked.is_none() && !value.is_empty() {
                        format!("< {value} >")
                    } else {
                        value
                    }
                }
                // The page link ("Radeon settings ▸" in the design, Latin-1 here).
                None if row.id == RowId::Radeon => ">".into(),
                None => String::new(),
            }
        };
        body.push(Line {
            text: columns(row.label, &value),
            style: Style::Row {
                focus: index == focus,
                locked: row.locked.is_some(),
            },
            row: Some(index),
        });
        if row
            .setting
            .is_some_and(|s| s.applies == Applies::NextStream)
        {
            body.push(Line {
                row: Some(index),
                ..line(&format!("  {}", s.next_stream), Style::Note)
            });
        }
        if index == focus {
            focus_end = body.len();
        }
    }
    let start = focus_end.saturating_sub(ROW_LINES);
    let message = if let Some(error) = &state.error {
        line(&format!("{}{error}", s.failed), Style::Error)
    } else if state
        .confirm
        .as_ref()
        .is_some_and(|(id, _)| *id == rows[focus].id)
    {
        line(&s.confirm, Style::Message)
    } else if let Some(reason) = rows[focus].locked {
        line(reason, Style::Message)
    } else {
        line("", Style::Message)
    };
    let mut lines = vec![line(title, Style::Title), line(&info, Style::Info)];
    lines.extend(body.into_iter().skip(start).take(ROW_LINES));
    lines.push(message);
    lines.push(line(&s.hint, Style::Hint));
    lines
}

fn shown(s: &Strings, control: &Control) -> String {
    match control {
        Control::Toggle(true) => s.on.clone(),
        Control::Toggle(false) => s.off.clone(),
        Control::Choice { index, options } => options
            .get(*index)
            .map_or_else(String::new, |o| o.label.clone()),
        Control::Integer { value, unit, .. } => match unit {
            Some(unit) => format!("{value} {unit}"),
            None => value.to_string(),
        },
        Control::Text(text) => text.clone(),
    }
}

/// `text` cut to `width` characters, ending in "..." when cut.
fn fit(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_owned();
    }
    let mut cut: String = text.chars().take(width.saturating_sub(3)).collect();
    cut.push_str("...");
    cut
}

/// A row line: the label on the left, the value right-aligned.
fn columns(label: &str, value: &str) -> String {
    let width = COLUMNS as usize;
    let value = fit(value, width);
    let value_len = value.chars().count();
    if value_len == 0 {
        return fit(label, width);
    }
    let label = fit(label, width.saturating_sub(value_len + 1));
    let gap = width - value_len - label.chars().count();
    format!("{label}{}{value}", " ".repeat(gap))
}

#[cfg(test)]
pub(crate) mod tests;
