//! The stream menu's first page and its shortcut pages: the tile grid and footer as
//! `nightjar_policy::stream_menu` lays them out (the menu Android draws too), the Shortcuts page
//! with the built-in chords and the user's own, and the editor that saves a shortcut from the
//! stream with nothing but a controller. app/overlay.rs draws the pages around them (More, Quality, PC
//! commands) with the rows it always had.

use egui::{vec2, RichText, Ui};
use nightjar_policy::shortcuts::{self as format, Shortcut, Shortcuts};
use nightjar_policy::control_center::{Dock, Gates, Module};
use nightjar_policy::stream_menu::{self as model, Cell, Hotkey, Layout, Offer};
use crate::platform_hooks::MenuNavButton;
use crate::ui::control_center_cards as cards;

use crate::glyphs::Icon;
use crate::menu_style;
use crate::session_page::HostExit;
use crate::strings::{en, tr};
use crate::theme::{regular, semibold, space, text, ERROR, ON_SURFACE};
use crate::ui::Tone;

/// The page the stream menu shows; Escape or B goes from any other back to the grid.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Page {
    #[default]
    Grid,
    Shortcuts,
    More,
    Quality,
    Commands,
    /// Who waits to join and the guests: opened for a new join request.
    Guests,
    Editor(Draft),
    /// Asks before a way out that ends the PC's session or game.
    Confirm(HostExit),
    /// Asks before Disconnect run from a controller button, so a stray press never ends the stream.
    ConfirmDisconnect,
}

impl Page {
    pub fn title(&self) -> &'static str {
        match self {
            Page::Grid => tr("Control Center"),
            Page::Shortcuts => tr("Shortcuts"),
            Page::More => tr("More"),
            Page::Quality => tr("Quality"),
            Page::Commands => tr("PC commands"),
            Page::Guests => tr("Guests"),
            Page::Editor(draft) if draft.id.is_some() => tr("Edit shortcut"),
            Page::Editor(_) => tr("New shortcut"),
            Page::Confirm(exit) => tr(exit.label()),
            Page::ConfirmDisconnect => tr("Disconnect"),
        }
    }
}

/// What this stream offers now, as the menu's tiles need it.
#[derive(Clone, Debug, Default)]
pub struct Available {
    /// The controller mouse, and whether it is on.
    pub mouse: Option<bool>,
    pub overlay: bool,
    /// Key chords reach the PC (not a remote monitor).
    pub shortcuts: bool,
    /// The microphone, and whether it is live.
    pub microphone: Option<bool>,
    /// Bitrate, frame rate, stream size or the stutter report.
    pub quality: bool,
    pub clipboard: bool,
    pub image_clipboard: bool,
    pub commands: bool,
    pub session: bool,
    pub host_audio: bool,
    pub controller_audio: bool,
    pub usb: bool,
    /// Settings' Controller section can open once the stream is left: false where Nightjar closes
    /// when the stream ends (a command-line start that quits on exit, an autostream run).
    pub controller_settings: bool,
    /// This device manages the guests of its game (`session.guests`).
    pub guests: bool,
    /// Quick Suspend, as the host offers it: Some(false) offers Suspend game, Some(true) Resume
    /// game while the game is paused on the PC.
    pub suspend: Option<bool>,
    /// Instant Replay, on a stream whose host offers Stream DVR: whether a recording runs. Its
    /// cards (Save clip, Start / Stop recording) are Control Center extras on Clips.
    pub dvr: Option<bool>,
    /// The ways out that end the PC's session or game, beside Disconnect: End session (a More row)
    /// and Close or Quit game as the Session and graphics page offers them, Quit game on a PC
    /// without it.
    pub exits: Vec<HostExit>,
}

/// The tiles for what is offered; app/overlay.rs draws More's rows for these same IDs.
pub fn offers(a: &Available) -> Vec<Offer> {
    let mut offers = Vec::new();
    if let Some(on) = a.mouse {
        offers.push(Offer::switch("controller_mouse", on));
    }
    offers.push(Offer::switch("overlay", a.overlay));
    if a.shortcuts {
        offers.push(Offer::action("shortcuts"));
    }
    if let Some(live) = a.microphone {
        offers.push(Offer::switch("microphone", live));
    }
    if a.quality {
        offers.push(Offer::action("bitrate"));
    }
    if a.clipboard {
        offers.push(Offer::action("clipboard_send"));
        offers.push(Offer::action("clipboard_receive"));
    }
    if a.commands {
        offers.push(Offer::action("server_commands"));
    }
    if a.session {
        offers.push(Offer::action("native_page"));
    }
    if let Some(paused) = a.suspend {
        offers.push(Offer::action(if paused { "resume_game" } else { "suspend_game" }));
    }
    // One level down: the picture's settings, the rest of the clipboard and the session's extras.
    offers.push(Offer::action("picture"));
    if a.host_audio {
        // The PC's night mode and headphone surround draw as one group.
        offers.push(Offer::action("night_mode"));
    }
    if a.image_clipboard {
        offers.push(Offer::action("image_clipboard_send"));
        offers.push(Offer::action("image_clipboard_receive"));
    }
    if a.controller_audio {
        offers.push(Offer::action("controller_audio"));
    }
    if a.usb {
        offers.push(Offer::action("usb_devices"));
    }
    // Settings' Controller section, the controller editor; opening it leaves the stream.
    if a.controller_settings {
        offers.push(Offer::action("controller_settings"));
    }
    if a.guests {
        offers.push(Offer::action("guests"));
    }
    offers.push(Offer::action("export_statistics"));
    offers.push(Offer::action("help"));
    offers.push(Offer::action("reconnect"));
    offers.push(Offer::action("disconnect"));
    offers.extend(a.exits.iter().map(|exit| Offer::action(exit_id(*exit))));
    offers
}

/// The model's ID for a way out: Quit game's in the footer, End session's under More.
pub fn exit_id(exit: HostExit) -> &'static str {
    match exit {
        HostExit::End => "end_host_session",
        HostExit::CloseGame => "close_game",
        HostExit::QuitApp => "quit_game",
    }
}

/// What was chosen under the grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Leave {
    /// Return to stream.
    Close,
    Disconnect,
    /// Asked first ([`Page::Confirm`]).
    Exit(HostExit),
}

/// A footer ID as the desktop runs it; None for one it does not offer.
fn leave(id: &str) -> Option<Leave> {
    match id {
        model::CLOSE => Some(Leave::Close),
        "disconnect" => Some(Leave::Disconnect),
        _ => [HostExit::End, HostExit::CloseGame, HostExit::QuitApp].into_iter().find(|exit| exit_id(*exit) == id).map(Leave::Exit),
    }
}

/// The menu for what is offered, the user's `saved` shortcuts and what they chose for the
/// controller's buttons.
pub fn layout(a: &Available, saved: usize, buttons: &model::Buttons) -> Layout {
    model::layout_with_buttons(&offers(a), a.shortcuts.then_some(saved), buttons).expect("the desktop offers only tiles the model knows")
}

/// The way out under the grid a controller button runs, when the user gave it one.
pub fn button_leave(layout: &Layout, key: Hotkey) -> Option<Leave> {
    leave(layout.buttons.get(&key)?)
}

/// The controller button the user gave a way out under the grid, as a tile's corner shows it; the
/// first in Y, X, LB, RB order when several name it.
pub fn button_cap(layout: &Layout, way: Leave) -> Option<&'static str> {
    layout.buttons.iter().find(|(_, id)| leave(id) == Some(way)).map(|(key, _)| key.cap())
}

/// What the desktop's stream menu can give a controller button, for its setting: the ids and how
/// they read. A desktop has no on-screen keyboard or controls, so those are left out.
pub const ASSIGNABLE: [(&str, &str); 11] = [
    ("controller_mouse", en("Mouse")),
    ("overlay", en("Overlay")),
    ("shortcuts", en("Shortcuts")),
    ("microphone", en("Microphone")),
    ("bitrate", en("Quality")),
    ("clipboard_send", en("Send clipboard")),
    ("clipboard_receive", en("Get clipboard")),
    ("server_commands", en("PC commands")),
    ("native_page", en("Session and graphics")),
    ("disconnect", en("Disconnect")),
    ("quit_game", en("Quit game")),
];

/// A grid tile's label.
pub fn label(id: &str) -> &'static str {
    match id {
        "controller_mouse" => tr("Mouse"),
        "overlay" => tr("Overlay"),
        "shortcuts" => tr("Shortcuts"),
        "microphone" => tr("Microphone"),
        "bitrate" => tr("Quality"),
        "clipboard_send" => tr("Send clipboard"),
        "clipboard_receive" => tr("Get clipboard"),
        "server_commands" => tr("PC commands"),
        "native_page" => tr("Session and graphics"),
        "suspend_game" => tr("Suspend game"),
        "resume_game" => tr("Resume game"),
        "save_clip" => tr("Save clip"),
        "record" => tr("Start recording"),
        model::MORE => tr("More"),
        other => unreachable!("grid tile {other} without a label"),
    }
}

fn icon(id: &str) -> Icon {
    match id {
        "controller_mouse" => Icon::Mouse,
        "overlay" => Icon::Overlay,
        "shortcuts" => Icon::Keyboard,
        "microphone" => Icon::Microphone,
        "bitrate" => Icon::Gauge,
        "clipboard_send" => Icon::ClipboardUp,
        "clipboard_receive" => Icon::ClipboardDown,
        "server_commands" => Icon::Terminal,
        "native_page" => Icon::Sliders,
        "suspend_game" => Icon::Pause,
        "resume_game" => Icon::Play,
        "save_clip" => Icon::Download,
        "record" => Icon::Play,
        _ => Icon::More,
    }
}

/// A built-in chord's name.
pub fn built_in_label(id: &str) -> &'static str {
    match id {
        "task_manager" => tr("Task Manager"),
        "switch_window" => tr("Switch window"),
        "close_window" => tr("Close window"),
        "start_menu" => tr("Start menu"),
        "show_desktop" => tr("Show desktop"),
        "game_bar" => tr("Game Bar"),
        "record_that" => tr("Record that"),
        other => unreachable!("built-in {other} without a label"),
    }
}

/// The button that runs a tile while the grid shows, by its place on the controller: Y on top,
/// X on the left, and the shoulders.
pub fn hotkey(button: MenuNavButton) -> Option<Hotkey> {
    match button {
        MenuNavButton::North => Some(Hotkey::Y),
        MenuNavButton::West => Some(Hotkey::X),
        MenuNavButton::LeftShoulder => Some(Hotkey::Lb),
        MenuNavButton::RightShoulder => Some(Hotkey::Rb),
        _ => None,
    }
}

/// The grid's button for `button`, when the grid gives it a tile or a way out (`offered`, the
/// layout's buttons): a button whose tile is not offered is not taken.
pub fn offered_hotkey(button: MenuNavButton, offered: &[Hotkey]) -> Option<Hotkey> {
    hotkey(button).filter(|key| offered.contains(key))
}

/// What was chosen on the grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Chosen {
    Tile(&'static str),
    Shortcut(usize),
    Add,
}

fn chosen(cell: &Cell) -> Chosen {
    match cell {
        Cell::Tile { id, .. } => Chosen::Tile(id),
        Cell::Shortcut(index) => Chosen::Shortcut(*index),
        Cell::Add => Chosen::Add,
    }
}

fn tile_width(ui: &Ui) -> f32 {
    let columns = model::COLUMNS as f32;
    ((ui.available_width() - menu_style::TILE_GAP * (columns - 1.0)) / columns).floor()
}

/// Lays `count` tiles out four to a row; `draw` draws the one at an index and says if it was chosen.
fn tiles(ui: &mut Ui, count: usize, mut draw: impl FnMut(&mut Ui, usize, f32) -> bool) -> Option<usize> {
    let width = tile_width(ui);
    let mut picked = None;
    for row in 0..count.div_ceil(model::COLUMNS) {
        if row > 0 {
            ui.add_space(menu_style::TILE_GAP - ui.spacing().item_spacing.y);
        }
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = menu_style::TILE_GAP;
            for index in row * model::COLUMNS..count.min((row + 1) * model::COLUMNS) {
                if draw(ui, index, width) {
                    picked = Some(index);
                }
            }
        });
    }
    picked
}

/// The grid. `hotkey` is a controller button pressed since the last frame; `focus_first` puts
/// the focus on the first tile, as when the menu opens.
pub fn grid(ui: &mut Ui, layout: &Layout, saved: &Shortcuts, hotkey: Option<Hotkey>, focus_first: bool) -> Option<Chosen> {
    let pressed = hotkey.and_then(|key| layout.hotkey(key)).map(|index| chosen(&layout.grid[index]));
    let picked = tiles(ui, layout.grid.len(), |ui, index, width| {
        let cell = &layout.grid[index];
        let response = match cell {
            Cell::Tile { id, on, hotkey } => menu_style::tile(ui, width, icon(id), label(id), None, *on, hotkey.map(Hotkey::cap)),
            Cell::Shortcut(n) => {
                let shortcut = &saved.data[*n];
                menu_style::tile(ui, width, Icon::Keyboard, &shortcut.name, keys_under(&shortcut.name, &shortcut.keys).as_deref(), None, None)
            }
            Cell::Add => menu_style::tile(ui, width, Icon::Plus, tr("Add shortcut"), None, None, None),
        };
        if focus_first && index == 0 {
            response.request_focus();
        }
        response.clicked()
    });
    pressed.or(picked.map(|index| chosen(&layout.grid[index])))
}

/// Under the grid: the ways out side by side, Disconnect and Quit game. Closing the menu is the head's
/// round close, so it isn't one of them here, and End session is a row under More.
pub fn footer(ui: &mut Ui, layout: &Layout) -> Option<Leave> {
    ui.add_space(space::M);
    let mut picked = None;
    let exits: Vec<Leave> = layout.footer.iter().filter_map(|id| {
        let leave = leave(id);
        if leave.is_none() {
            eprintln!("nightjar: the stream menu's footer has {id}, which the desktop does not run");
        }
        leave
    }).filter(|leave| *leave != Leave::Close).collect();
    let count = exits.len() as f32;
    let width = ((ui.available_width() - menu_style::TILE_GAP * (count - 1.0)) / count).floor();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = menu_style::TILE_GAP;
        for leave in exits {
            let response = match leave {
                Leave::Disconnect => {
                    // The button the user gave it shows instead of the keyboard's chord, as on a tile.
                    let keys = button_cap(layout, leave).map(str::to_owned).or_else(|| {
                        crate::shortcuts::chord_for(crate::shortcuts::Scope::Stream, crate::shortcuts::Action::Disconnect).map(crate::shortcuts::Chord::text)
                    });
                    menu_style::exit_button(ui, width, Icon::Leave, tr("Disconnect"), keys.as_deref(), Tone::Quiet)
                }
                // Quiet with red words: Disconnect is the everyday way out, and the question that
                // follows keeps the solid red.
                Leave::Exit(exit) => menu_style::quiet_danger_exit_button(ui, width, Icon::Power, tr(exit.label()), button_cap(layout, leave)),
                Leave::Close => continue,
            };
            if response.clicked() {
                picked = Some(leave);
            }
        }
    });
    picked
}

/// The Control Center for what is offered: the same tiles as [`layout`], sorted into modules by
/// `nightjar_policy::control_center` (the dock Android draws too), with the extras `gates` allow.
pub fn dock(a: &Available, saved: usize, buttons: &model::Buttons, gates: &Gates) -> Dock {
    let mut offers = offers(a);
    if let Some(recording) = a.dvr {
        offers.push(Offer::action("save_clip"));
        offers.push(Offer::switch("record", recording));
    }
    nightjar_policy::control_center::dock(&offers, a.shortcuts.then_some(saved), buttons, gates).expect("the desktop offers only tiles the model knows")
}

/// The way out in the Control Center's footer a controller button runs, when the user gave it one.
pub fn dock_button_leave(dock: &Dock, key: Hotkey) -> Option<Leave> {
    leave(dock.buttons.get(&key)?)
}

/// The tile the Control Center draws as its slider card: wide, the bitrate's track inside. The app
/// draws it (it owns the stream), so [`cards`] leaves it out of the rows of two.
pub const SLIDER: &str = "bitrate";

/// A card's own id, so the focus finds it again the next time the Control Center opens.
pub fn card_id(id: &str) -> egui::Id {
    egui::Id::unique(("control-center-card", id))
}

/// The cards of `module`'s page, two to a row. `hotkey` is a controller button pressed since the
/// last frame: it runs its card on whatever page that is. `focus_first` puts the focus on the
/// first card, as when the Control Center opens.
pub fn cards(ui: &mut Ui, dock: &Dock, module: Module, saved: &Shortcuts, hotkey: Option<Hotkey>, focus_first: bool) -> Option<Chosen> {
    let pressed = hotkey
        .and_then(|key| dock.hotkey(key))
        .and_then(|(module, index)| dock.page(module).and_then(|page| page.cards.get(index)).map(chosen));
    let Some(page) = dock.page(module) else { return pressed };
    let shown: Vec<&Cell> = page.cards.iter().filter(|cell| !matches!(cell, Cell::Tile { id, .. } if *id == SLIDER)).collect();
    let mut picked = None;
    cards::grid(ui, shown.len(), |ui, index, width| {
        let cell = shown[index];
        let response = match cell {
            Cell::Tile { id, on, hotkey } => {
                let (label, state) = card_text(id, *on);
                cards::card(ui, card_id(id), width, &cards::Card { icon: icon(id), label, state, on: *on, cap: hotkey.map(Hotkey::cap) })
            }
            Cell::Shortcut(n) => {
                let shortcut = &saved.data[*n];
                let keys = keys_under(&shortcut.name, &shortcut.keys);
                let card = cards::Card { icon: Icon::Keyboard, label: &shortcut.name, state: keys.as_deref(), on: None, cap: None };
                cards::card(ui, egui::Id::unique(("control-center-shortcut", *n)), width, &card)
            }
            Cell::Add => {
                let card = cards::Card { icon: Icon::Plus, label: tr("Add shortcut"), state: None, on: None, cap: None };
                cards::card(ui, egui::Id::unique("control-center-add"), width, &card)
            }
        };
        if focus_first && index == 0 {
            response.request_focus();
        }
        if response.clicked() {
            picked = Some(chosen(cell));
        }
    });
    pressed.or(picked)
}

/// A card's label and the state under it. Recording says what pressing it does, lit while it runs.
pub fn card_text(id: &str, on: Option<bool>) -> (&'static str, Option<&'static str>) {
    match (id, on) {
        ("record", Some(true)) => (tr("Stop recording"), None),
        ("record", _) => (tr("Start recording"), None),
        _ => (label(id), on.map(|on| if on { tr("On") } else { tr("Off") })),
    }
}

/// The Control Center's footer, under every page: Return to stream, the main action (`back` is the
/// key or button that also returns), then the ways out side by side, Disconnect and Quit game,
/// the one red control.
pub fn dock_footer(ui: &mut Ui, dock: &Dock, back: Option<&str>) -> Option<Leave> {
    let mut picked = menu_style::primary(ui, tr("Return to stream"), back).clicked().then_some(Leave::Close);
    let exits: Vec<Leave> = dock.footer.iter().filter_map(|id| leave(id)).filter(|leave| *leave != Leave::Close).collect();
    if exits.is_empty() {
        return picked;
    }
    let cap = |way: Leave| dock.buttons.iter().find(|(_, id)| leave(id) == Some(way)).map(|(key, _)| key.cap());
    ui.add_space(menu_style::TILE_GAP - ui.spacing().item_spacing.y);
    let count = exits.len() as f32;
    let width = ((ui.available_width() - menu_style::TILE_GAP * (count - 1.0)) / count).floor();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = menu_style::TILE_GAP;
        for way in exits {
            let response = match way {
                Leave::Disconnect => {
                    // The button the user gave it shows instead of the keyboard's chord, as on a card.
                    let keys = cap(way).map(str::to_owned).or_else(|| {
                        crate::shortcuts::chord_for(crate::shortcuts::Scope::Stream, crate::shortcuts::Action::Disconnect).map(crate::shortcuts::Chord::text)
                    });
                    menu_style::exit_button(ui, width, Icon::Leave, tr("Disconnect"), keys.as_deref(), Tone::Quiet)
                }
                // Quiet with red words: the question that follows keeps the solid red.
                Leave::Exit(exit) => menu_style::quiet_danger_exit_button(ui, width, Icon::Power, tr(exit.label()), cap(way)),
                Leave::Close => continue,
            };
            if response.clicked() {
                picked = Some(way);
            }
        }
    });
    picked
}

/// Asks before `exit` runs, in the words the Session and graphics page uses: Some(true) to go on,
/// Some(false) to go back to the grid.
pub fn confirm(ui: &mut Ui, exit: HostExit, focus_first: bool) -> Option<bool> {
    ask(ui, tr(exit.confirm()), tr(exit.label()), focus_first)
}

/// Where Cancel on a question goes: End session is asked from More, the rest from the first page.
pub fn back_from_confirm(exit: HostExit) -> Page {
    if exit == HostExit::End { Page::More } else { Page::Grid }
}

/// Asks before Disconnect run from a controller button, as [`confirm`] does.
pub fn confirm_disconnect(ui: &mut Ui, focus_first: bool) -> Option<bool> {
    ask(ui, tr("Disconnect from the stream?"), tr("Disconnect"), focus_first)
}

fn ask(ui: &mut Ui, question: &str, action: &str, focus_first: bool) -> Option<bool> {
    menu_style::text_block(ui, question);
    ui.add_space(space::M);
    let mut answer = None;
    let width = ((ui.available_width() - menu_style::TILE_GAP) / 2.0).floor();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = menu_style::TILE_GAP;
        let cancel = menu_style::exit_button(ui, width, Icon::Back, tr("Cancel"), None, Tone::Quiet);
        // Cancel takes the focus, so A pressed twice never ends the game.
        if focus_first {
            cancel.request_focus();
        }
        if cancel.clicked() {
            answer = Some(false);
        }
        if menu_style::exit_button(ui, width, Icon::Power, action, None, Tone::Danger).clicked() {
            answer = Some(true);
        }
    });
    answer
}

/// What was chosen on the Shortcuts page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShortcutChoice {
    Send(Shortcut),
    Edit(usize),
    Add,
}

/// A shortcut's keys under its name, left out when the name already says them (Caps Lock, the
/// editor's default name), as on Android.
fn keys_under(name: &str, keys: &[String]) -> Option<String> {
    let chord = format::chord_label(keys);
    (chord != name).then_some(chord)
}

/// The Shortcuts page: the saved shortcuts in their order, the built-ins not saved under a name of
/// their own and Add as tiles that send at once, then each saved one to edit. The saved ones come
/// first so a new built-in never pushes them along (nightjar#61).
pub fn shortcuts_page(ui: &mut Ui, saved: &Shortcuts, focus_first: bool) -> Option<ShortcutChoice> {
    let built = format::built_ins(&saved.data);
    let add = saved.data.len() < format::MAX_SHORTCUTS;
    let count = built.len() + saved.data.len() + usize::from(add);
    let picked = tiles(ui, count, |ui, index, width| {
        let response = if let Some(s) = saved.data.get(index) {
            menu_style::tile(ui, width, Icon::Keyboard, &s.name, keys_under(&s.name, &s.keys).as_deref(), None, None)
        } else if let Some(b) = built.get(index - saved.data.len()) {
            let keys: Vec<String> = b.keys.iter().map(|k| k.to_string()).collect();
            menu_style::tile(ui, width, Icon::Keyboard, built_in_label(b.id), Some(&format::chord_label(&keys)), None, None)
        } else {
            menu_style::tile(ui, width, Icon::Plus, tr("Add shortcut"), None, None, None)
        };
        if focus_first && index == 0 {
            response.request_focus();
        }
        response.clicked()
    });
    let mut choice = picked.map(|index| {
        if let Some(s) = saved.data.get(index) {
            ShortcutChoice::Send(s.clone())
        } else if let Some(b) = built.get(index - saved.data.len()) {
            ShortcutChoice::Send(Shortcut { id: b.id.into(), name: built_in_label(b.id).into(), keys: b.keys.iter().map(|k| k.to_string()).collect() })
        } else {
            ShortcutChoice::Add
        }
    });
    if !saved.data.is_empty() {
        menu_style::caption(ui, tr("Edit shortcuts"));
        for (index, shortcut) in saved.data.iter().enumerate() {
            if menu_style::action_keys(ui, Icon::Sliders, &shortcut.name, Some(&format::chord_label(&shortcut.keys))).clicked() {
                choice = Some(ShortcutChoice::Edit(index));
            }
        }
    }
    choice
}

/// A shortcut being written in the stream: its name (the keys' text when left empty) and keys
/// picked from the grid of key caps.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Draft {
    /// The saved shortcut this edits; None for a new one.
    pub id: Option<String>,
    pub name: String,
    pub keys: Vec<String>,
    pub error: Option<String>,
}

impl Draft {
    pub fn edit(shortcut: &Shortcut) -> Self {
        Self { id: Some(shortcut.id.clone()), name: shortcut.name.clone(), keys: shortcut.keys.clone(), error: None }
    }

    /// Picks a key, or puts it back when already picked. A chord holds at most eight.
    pub fn toggle(&mut self, key: &str) {
        self.error = None;
        if let Some(at) = self.keys.iter().position(|k| k == key) {
            self.keys.remove(at);
        } else if self.keys.len() < format::MAX_KEYS {
            self.keys.push(key.to_owned());
        }
    }

    /// Saves into `saved`; false with the reason kept for the page when refused.
    pub fn save(&mut self, saved: &mut Shortcuts) -> bool {
        if self.keys.is_empty() {
            self.error = Some(crate::shortcuts::error_text(&format::ShortcutError::ChordLength));
            return false;
        }
        let name = if self.name.trim().is_empty() { format::chord_label(&self.keys) } else { self.name.clone() };
        match saved.save(self.id.as_deref(), &name, &self.keys, "desktop") {
            Ok(_) => true,
            Err(error) => {
                self.error = Some(crate::shortcuts::error_text(&error));
                false
            }
        }
    }
}

/// How the editor ended this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edited {
    Stay,
    /// Saved or deleted: `saved` changed and wants writing.
    Changed,
    Cancel,
}

/// Key caps in rows of this many.
const PICKER_COLUMNS: usize = 8;

/// The draft's keys the key picker has no cap for, in their order.
fn extra_keys(keys: &[String]) -> Vec<String> {
    keys.iter().filter(|key| !format::PICKER.contains(&key.as_str())).cloned().collect()
}

/// The editor: name, the chord so far, the key caps, then Save, Cancel and, for a saved
/// shortcut, Delete.
pub fn editor(ui: &mut Ui, draft: &mut Draft, saved: &mut Shortcuts, focus_first: bool) -> Edited {
    let chord = format::chord_label(&draft.keys);
    let hint = if draft.keys.is_empty() { tr("Shortcut name").to_owned() } else { chord.clone() };
    let name = ui.add(egui::TextEdit::singleline(&mut draft.name).hint_text(hint).desired_width(f32::INFINITY).char_limit(format::MAX_NAME));
    crate::ui::name_for_readers(ui, &name, tr("Shortcut name"), "");
    ui.add_space(space::M);
    let shown = if draft.keys.is_empty() { "—" } else { chord.as_str() };
    ui.label(RichText::new(shown).font(semibold(text::SECTION)).color(*ON_SURFACE));
    ui.add_space(space::S);
    let width = ((ui.available_width() - 4.0 * (PICKER_COLUMNS as f32 - 1.0)) / PICKER_COLUMNS as f32).floor();
    let mut toggled = None;
    // Keys of an imported or older shortcut that the caps lack (F13) come first, picked, so they show and can be taken out.
    let extra = extra_keys(&draft.keys);
    let caps: Vec<&str> = extra.iter().map(String::as_str).chain(format::PICKER.iter().copied()).collect();
    for row in caps.chunks(PICKER_COLUMNS) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for key in row {
                let picked = draft.keys.iter().any(|k| k == key);
                let cap = egui::Button::new(RichText::new(format::key_label(key)).font(regular(text::LABEL))).selected(picked).min_size(vec2(width, 32.0));
                let response = ui.add(cap);
                if focus_first && *key == caps[0] {
                    response.request_focus();
                }
                if response.clicked() {
                    toggled = Some(key.to_string());
                }
            }
        });
    }
    if let Some(key) = toggled {
        draft.toggle(&key);
    }
    if let Some(error) = &draft.error {
        ui.add_space(space::S);
        ui.label(RichText::new(error).font(regular(text::LABEL)).color(*ERROR));
    }
    ui.add_space(space::M);
    let mut edited = Edited::Stay;
    ui.horizontal(|ui| {
        if ui.add_enabled_ui(!draft.keys.is_empty(), |ui| menu_style::button(ui, tr("Save"))).inner.clicked() && draft.save(saved) {
            edited = Edited::Changed;
        }
        if menu_style::button(ui, tr("Cancel")).clicked() {
            edited = Edited::Cancel;
        }
        if let Some(id) = draft.id.clone()
            && menu_style::button(ui, tr("Delete")).clicked() {
                match saved.remove(&id) {
                    Ok(()) => edited = Edited::Changed,
                    Err(error) => draft.error = Some(crate::shortcuts::error_text(&error)),
                }
            }
    });
    edited
}

/// The Instant Replay group under the grid.
pub mod dvr;

#[cfg(test)]
mod tests;
