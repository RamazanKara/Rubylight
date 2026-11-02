//! The stream menu's first page and its shortcut pages: the tile grid and footer as
//! `nightjar_policy::stream_menu` lays them out (the menu Android draws too), the Shortcuts page
//! with the built-in chords and the user's own, and the editor that saves a shortcut from the
//! stream with nothing but a controller. main.rs draws the pages around them (More, Quality, PC
//! commands) with the rows it always had.

use egui::{vec2, RichText, Ui};
use nightjar_policy::shortcuts::{self as format, Shortcut, Shortcuts};
use nightjar_policy::stream_menu::{self as model, Cell, Hotkey, Layout, Offer};
use sdl3_sys::everything::*;

use crate::glyphs::Icon;
use crate::menu_style;
use crate::strings::tr;
use crate::theme::{regular, semibold, space, text, ERROR, ON_SURFACE};

/// The page the stream menu shows; Escape or B goes from any other back to the grid.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Page {
    #[default]
    Grid,
    Shortcuts,
    More,
    Quality,
    Commands,
    Editor(Draft),
}

impl Page {
    pub fn title(&self) -> &'static str {
        match self {
            Page::Grid => tr("Stream controls"),
            Page::Shortcuts => tr("Shortcuts"),
            Page::More => tr("More"),
            Page::Quality => tr("Quality"),
            Page::Commands => tr("PC commands"),
            Page::Editor(draft) if draft.id.is_some() => tr("Edit shortcut"),
            Page::Editor(_) => tr("New shortcut"),
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
}

/// The tiles for what is offered; main.rs draws More's rows for these same IDs.
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
    offers.push(Offer::action("export_statistics"));
    offers.push(Offer::action("help"));
    offers.push(Offer::action("reconnect"));
    offers.push(Offer::action("disconnect"));
    offers
}

/// The menu for what is offered and the user's `saved` shortcuts.
pub fn layout(a: &Available, saved: usize) -> Layout {
    model::layout(&offers(a), a.shortcuts.then_some(saved)).expect("the desktop offers only tiles the model knows")
}

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
        other => unreachable!("built-in {other} without a label"),
    }
}

/// The button that runs a tile while the grid shows, by its place on the controller: Y on top,
/// X on the left, and the shoulders.
pub fn hotkey(button: SDL_GamepadButton) -> Option<Hotkey> {
    match button {
        SDL_GAMEPAD_BUTTON_NORTH => Some(Hotkey::Y),
        SDL_GAMEPAD_BUTTON_WEST => Some(Hotkey::X),
        SDL_GAMEPAD_BUTTON_LEFT_SHOULDER => Some(Hotkey::Lb),
        SDL_GAMEPAD_BUTTON_RIGHT_SHOULDER => Some(Hotkey::Rb),
        _ => None,
    }
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
                menu_style::tile(ui, width, Icon::Keyboard, &shortcut.name, Some(&format::chord_label(&shortcut.keys)), None, None)
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

/// Under the grid: Return to stream across the width, then the ways out. Returns the footer ID
/// chosen.
pub fn footer(ui: &mut Ui, layout: &Layout) -> Option<&'static str> {
    ui.add_space(space::L);
    let mut picked = None;
    if menu_style::primary(ui, tr("Return to stream"), Some(crate::shortcuts::Key::Escape.cap())).clicked() {
        picked = Some(model::CLOSE);
    }
    ui.add_space(space::XS);
    for id in layout.footer.iter().filter(|id| **id != model::CLOSE) {
        let (label, shortcut) = match *id {
            "disconnect" => (tr("Disconnect"), Some(crate::shortcuts::Action::Disconnect)),
            other => unreachable!("footer action {other} on the desktop"),
        };
        if menu_style::danger(ui, Icon::Power, label, shortcut).clicked() {
            picked = Some(*id);
        }
    }
    picked
}

/// What was chosen on the Shortcuts page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShortcutChoice {
    Send(Shortcut),
    Edit(usize),
    Add,
}

/// The Shortcuts page: the built-ins not saved under a name of their own, the saved shortcuts
/// and Add as tiles that send at once, then each saved one to edit.
pub fn shortcuts_page(ui: &mut Ui, saved: &Shortcuts, focus_first: bool) -> Option<ShortcutChoice> {
    let built = format::built_ins(&saved.data);
    let add = saved.data.len() < format::MAX_SHORTCUTS;
    let count = built.len() + saved.data.len() + usize::from(add);
    let picked = tiles(ui, count, |ui, index, width| {
        let response = if let Some(b) = built.get(index) {
            let keys: Vec<String> = b.keys.iter().map(|k| k.to_string()).collect();
            menu_style::tile(ui, width, Icon::Keyboard, built_in_label(b.id), Some(&format::chord_label(&keys)), None, None)
        } else if let Some(s) = saved.data.get(index - built.len()) {
            menu_style::tile(ui, width, Icon::Keyboard, &s.name, Some(&format::chord_label(&s.keys)), None, None)
        } else {
            menu_style::tile(ui, width, Icon::Plus, tr("Add shortcut"), None, None, None)
        };
        if focus_first && index == 0 {
            response.request_focus();
        }
        response.clicked()
    });
    let mut choice = picked.map(|index| {
        if let Some(b) = built.get(index) {
            ShortcutChoice::Send(Shortcut { id: b.id.into(), name: built_in_label(b.id).into(), keys: b.keys.iter().map(|k| k.to_string()).collect() })
        } else if let Some(s) = saved.data.get(index - built.len()) {
            ShortcutChoice::Send(s.clone())
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
    for row in format::PICKER.chunks(PICKER_COLUMNS) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for key in row {
                let picked = draft.keys.iter().any(|k| k == key);
                let cap = egui::Button::new(RichText::new(format::key_label(key)).font(regular(text::LABEL))).selected(picked).min_size(vec2(width, 32.0));
                let response = ui.add(cap);
                if focus_first && *key == format::PICKER[0] {
                    response.request_focus();
                }
                if response.clicked() {
                    toggled = Some(*key);
                }
            }
        });
    }
    if let Some(key) = toggled {
        draft.toggle(key);
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
        if let Some(id) = draft.id.clone() {
            if menu_style::button(ui, tr("Delete")).clicked() {
                match saved.remove(&id) {
                    Ok(()) => edited = Edited::Changed,
                    Err(error) => draft.error = Some(crate::shortcuts::error_text(&error)),
                }
            }
        }
    });
    edited
}

#[cfg(test)]
mod tests {
    use super::*;

    fn everything() -> Available {
        Available {
            mouse: Some(true), overlay: true, shortcuts: true, microphone: Some(false), quality: true, clipboard: true, image_clipboard: true,
            commands: true, session: true, host_audio: true, controller_audio: true, usb: true,
        }
    }

    #[test]
    fn every_desktop_offer_is_a_tile_the_model_knows_with_a_label() {
        let layout = layout(&everything(), 1);
        let ids: Vec<_> = layout.grid.iter().filter_map(|cell| match cell { Cell::Tile { id, .. } => Some(*id), _ => None }).collect();
        assert_eq!(ids, ["controller_mouse", "overlay", "shortcuts", "microphone", "bitrate", "clipboard_send", "clipboard_receive",
            "server_commands", "native_page", "more"]);
        for id in ids {
            assert!(!label(id).is_empty());
        }
        assert_eq!(layout.grid[layout.grid.len() - 2..], [Cell::Shortcut(0), Cell::Add]);
        assert_eq!(layout.footer, ["close", "disconnect"]);
        assert_eq!(layout.more, ["picture", "night_mode", "image_clipboard_send", "image_clipboard_receive", "usb_devices",
            "controller_audio", "export_statistics", "help", "reconnect"]);
        for built_in in format::BUILT_INS {
            assert!(!built_in_label(built_in.id).is_empty());
        }
    }

    #[test]
    fn a_remote_monitor_gets_no_shortcuts_and_the_buttons_reach_their_tiles() {
        let layout = layout(&Available { shortcuts: false, ..everything() }, 3);
        assert!(!layout.grid.iter().any(|cell| matches!(cell, Cell::Shortcut(_) | Cell::Add)));
        assert_eq!(layout.hotkey(Hotkey::Lb), None);
        let layout = super::layout(&everything(), 0);
        assert_eq!(hotkey(SDL_GAMEPAD_BUTTON_WEST).and_then(|k| layout.hotkey(k)).and_then(|i| layout.tile_id(i)), Some("controller_mouse"));
        assert_eq!(hotkey(SDL_GAMEPAD_BUTTON_RIGHT_SHOULDER).and_then(|k| layout.hotkey(k)).and_then(|i| layout.tile_id(i)), Some("overlay"));
        assert_eq!(hotkey(SDL_GAMEPAD_BUTTON_LEFT_SHOULDER).and_then(|k| layout.hotkey(k)).and_then(|i| layout.tile_id(i)), Some("shortcuts"));
        assert_eq!(hotkey(SDL_GAMEPAD_BUTTON_NORTH).and_then(|k| layout.hotkey(k)), None, "no keyboard tile on the desktop");
        assert_eq!(hotkey(SDL_GAMEPAD_BUTTON_SOUTH), None);
    }

    #[test]
    fn a_shortcut_written_in_the_stream_is_saved_named_after_its_keys_and_can_be_deleted() {
        let mut saved = Shortcuts::default();
        let mut draft = Draft::default();
        for key in ["WIN", "SHIFT", "S", "S"] {
            draft.toggle(key);
        }
        assert_eq!(draft.keys, ["WIN", "SHIFT"]);
        draft.toggle("S");
        assert!(draft.save(&mut saved));
        assert_eq!((saved.data[0].name.as_str(), &saved.data[0].keys[..]), ("Win + Shift + S", &["WIN".to_owned(), "SHIFT".into(), "S".into()][..]));
        let mut edit = Draft::edit(&saved.data[0]);
        edit.name = "Snip".into();
        assert!(edit.save(&mut saved));
        assert_eq!(saved.data.len(), 1);
        assert_eq!(saved.data[0].name, "Snip");
        let mut empty = Draft::default();
        assert!(!empty.save(&mut saved));
        assert_eq!(empty.error.as_deref(), Some(tr("Use between 1 and 8 keys in each chord.")));
        for key in format::PICKER.iter().take(9) {
            empty.toggle(key);
        }
        assert_eq!(empty.keys.len(), format::MAX_KEYS, "a ninth key is not taken");
    }
}
