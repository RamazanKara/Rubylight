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
use crate::session_page::HostExit;
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
    /// Asks before a way out that ends the PC's session or game.
    Confirm(HostExit),
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
            Page::Confirm(exit) => tr(exit.label()),
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
    /// The ways out that end the PC's session or game, beside Disconnect: End session and Close
    /// or Quit game as the Session and graphics page offers them, Quit game on a PC without it.
    pub exits: Vec<HostExit>,
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
    offers.extend(a.exits.iter().map(|exit| Offer::action(exit_id(*exit))));
    offers
}

/// The model's footer ID for a way out.
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
        "game_bar" => tr("Game Bar"),
        "record_that" => tr("Record that"),
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

/// Under the grid: Return to stream across the width, then the ways out side by side, Disconnect
/// first.
pub fn footer(ui: &mut Ui, layout: &Layout) -> Option<Leave> {
    ui.add_space(space::L);
    let mut picked = None;
    if menu_style::primary(ui, tr("Return to stream"), Some(crate::shortcuts::Key::Escape.cap().as_ref())).clicked() {
        picked = Some(Leave::Close);
    }
    let exits: Vec<Leave> = layout.footer.iter().filter_map(|id| {
        let leave = leave(id);
        if leave.is_none() {
            eprintln!("nightjar: the stream menu's footer has {id}, which the desktop does not run");
        }
        leave
    }).filter(|leave| *leave != Leave::Close).collect();
    if exits.is_empty() {
        return picked;
    }
    ui.add_space(space::S);
    let count = exits.len() as f32;
    let width = ((ui.available_width() - menu_style::TILE_GAP * (count - 1.0)) / count).floor();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = menu_style::TILE_GAP;
        for leave in exits {
            let response = match leave {
                Leave::Disconnect => {
                    let keys = crate::shortcuts::chord_for(crate::shortcuts::Scope::Stream, crate::shortcuts::Action::Disconnect).map(crate::shortcuts::Chord::text);
                    menu_style::exit_button(ui, width, Icon::Power, tr("Disconnect"), keys.as_deref())
                }
                Leave::Exit(exit) => menu_style::exit_button(ui, width, Icon::Power, tr(exit.label()), None),
                Leave::Close => continue,
            };
            if response.clicked() {
                picked = Some(leave);
            }
        }
    });
    picked
}

/// Asks before `exit` runs, in the words the Session and graphics page uses: Some(true) to go on,
/// Some(false) to go back to the grid.
pub fn confirm(ui: &mut Ui, exit: HostExit, focus_first: bool) -> Option<bool> {
    menu_style::text_block(ui, tr(exit.confirm()));
    ui.add_space(space::M);
    let mut answer = None;
    ui.horizontal(|ui| {
        let cancel = menu_style::button(ui, tr("Cancel"));
        // Cancel takes the focus, so A pressed twice never ends the game.
        if focus_first {
            cancel.request_focus();
        }
        if cancel.clicked() {
            answer = Some(false);
        }
        if menu_style::button(ui, tr(exit.label())).clicked() {
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
            commands: true, session: true, host_audio: true, controller_audio: true, usb: true, exits: vec![HostExit::End, HostExit::CloseGame],
        }
    }

    /// A shortcut file from Android can hold keys the caps lack (F13, the right Ctrl): they get caps of their own,
    /// and taking one out leaves the rest to save.
    #[test]
    fn keys_the_picker_lacks_get_caps_of_their_own_and_can_be_taken_out() {
        let file = r#"{"version":1,"data":[{"id":"a","name":"Old","keys":["CTRL_RIGHT","F13","A"]}]}"#;
        let mut saved = Shortcuts::parse(file).unwrap();
        let mut draft = Draft::edit(&saved.data[0]);
        assert_eq!(extra_keys(&draft.keys), ["CTRL_RIGHT", "F13"]);
        draft.toggle("F13");
        assert_eq!(extra_keys(&draft.keys), ["CTRL_RIGHT"]);
        assert!(draft.save(&mut saved));
        assert_eq!(saved.data[0].keys, ["CTRL_RIGHT", "A"]);
    }

    /// A shortcut named after its keys says them once: the menu showed "Caps Lock / Caps Lock".
    #[test]
    fn a_shortcut_named_after_its_keys_shows_no_key_line() {
        assert_eq!(keys_under("Caps Lock", &["CAPS_LOCK".into()]), None);
        assert_eq!(keys_under("Snip", &["WIN".into(), "SHIFT".into(), "S".into()]).as_deref(), Some("Win + Shift + S"));
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
        assert_eq!(layout.footer, ["close", "disconnect", "end_host_session", "close_game"]);
        assert_eq!(layout.more, ["picture", "night_mode", "image_clipboard_send", "image_clipboard_receive", "usb_devices",
            "controller_audio", "export_statistics", "help", "reconnect"]);
        for built_in in format::BUILT_INS {
            assert!(!built_in_label(built_in.id).is_empty());
        }
    }

    /// Android's `stream_tile_<key>` strings in one language's folder (`values`, `values-de`).
    fn android_tile_labels(folder: &str) -> std::collections::HashMap<String, String> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../app/src/main/res").join(folder).join("strings_stream_menu.xml");
        let xml = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        xml.lines()
            .filter_map(|line| {
                let rest = line.trim().strip_prefix("<string name=\"stream_tile_")?;
                let (key, rest) = rest.split_once("\">")?;
                let text = rest.strip_suffix("</string>")?.replace("\\'", "'").replace("&amp;", "&");
                Some((key.to_owned(), text))
            })
            .collect()
    }

    /// nightjar#63: a tile has the same short name on the desktop and on Android, in English and in
    /// German: the model's label, and Android's string for its key.
    #[test]
    fn the_desktop_names_each_tile_as_the_model_and_android_do() {
        use crate::strings::{en, tests::with_lang, Lang};
        let layout = layout(&everything(), 0);
        let mut named: Vec<(&str, &str)> = layout.grid.iter().filter_map(|cell| match cell { Cell::Tile { id, .. } => Some(*id), _ => None })
            .map(|id| (id, with_lang(Lang::En, || label(id)))).collect();
        named.push((model::ADD, en("Add shortcut")));
        named.push((model::CLOSE, en("Return to stream")));
        named.push(("disconnect", en("Disconnect")));
        named.extend([HostExit::End, HostExit::CloseGame, HostExit::QuitApp].map(|exit| (exit_id(exit), exit.label())));
        let (english, german) = (android_tile_labels("values"), android_tile_labels("values-de"));
        for (id, text) in named {
            let model = model::label(id).unwrap_or_else(|| panic!("{id} has no label in the model"));
            assert_eq!(text, model.en, "{id}: the desktop's English differs from the model's");
            assert_eq!(english.get(model.key).map(String::as_str), Some(model.en), "{id}: Android's stream_tile_{}", model.key);
            let desktop = with_lang(Lang::De, || tr(text).to_owned());
            assert_eq!(german.get(model.key), Some(&desktop), "{id}: Android's German stream_tile_{}", model.key);
        }
        for label in model::labels() {
            assert_eq!(english.get(label.key).map(String::as_str), Some(label.en), "Android has no stream_tile_{}", label.key);
        }
    }

    /// nightjar#63: every way out the model may put in the footer runs on the desktop (no footer ID
    /// it cannot draw), Disconnect first and the ways that end the PC's game after it.
    #[test]
    fn every_way_out_the_desktop_offers_runs_from_the_footer() {
        for exits in [vec![], vec![HostExit::QuitApp], vec![HostExit::End, HostExit::QuitApp], vec![HostExit::End, HostExit::CloseGame]] {
            let layout = layout(&Available { exits: exits.clone(), ..everything() }, 0);
            let leaves: Vec<_> = layout.footer.iter().map(|id| leave(id).unwrap_or_else(|| panic!("{id} has no way to run"))).collect();
            let want: Vec<_> = [Leave::Close, Leave::Disconnect].into_iter().chain(exits.into_iter().map(Leave::Exit)).collect();
            assert_eq!(leaves, want);
        }
        assert_eq!(leave("end_session"), None, "a remote monitor's End session is Android's");
        for exit in [HostExit::End, HostExit::CloseGame, HostExit::QuitApp] {
            assert_eq!(model::tile(exit_id(exit)).map(|t| t.place), Some(model::Place::Footer));
        }
    }

    /// One frame in the menu's window: each named widget's place, and the focused one's name.
    fn frame(ctx: &egui::Context, add: impl FnOnce(&mut Ui)) -> (Vec<(String, egui::Rect)>, Option<String>) {
        let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(1280.0, 800.0))), ..Default::default() };
        let mut add = Some(add);
        let mut output = ctx.run_ui(input, |ui| {
            menu_style::window("Stream controls", "footer-test").show(ui.ctx(), |ui| (add.take().unwrap())(ui));
        });
        output.textures_delta.clear();
        let tree = output.platform_output.accesskit_update.expect("AccessKit is on");
        let focus = tree.nodes.iter().find(|(id, _)| *id == tree.focus).and_then(|(_, n)| n.label()).map(str::to_owned);
        let nodes = tree.nodes.iter().filter_map(|(_, n)| {
            let b = n.bounds()?;
            Some((n.label()?.to_owned(), egui::Rect::from_min_max(egui::pos2(b.x0 as f32, b.y0 as f32), egui::pos2(b.x1 as f32, b.y1 as f32))))
        }).collect();
        (nodes, focus)
    }

    /// Return to stream across the width, and under it Disconnect and the game's ways out side by
    /// side in equal widths, as Starwire's menu has them.
    #[test]
    fn the_ways_out_sit_side_by_side_under_return_to_stream() {
        crate::theme::own_palette();
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        ctx.enable_accesskit();
        let layout = layout(&everything(), 0);
        let mut nodes = Vec::new();
        for _ in 0..4 {
            nodes = frame(&ctx, |ui| {
                footer(ui, &layout);
            })
            .0;
        }
        let find = |name: &str| nodes.iter().find(|(label, _)| label.starts_with(name)).map(|(_, r)| *r).unwrap_or_else(|| panic!("{name} in {nodes:#?}"));
        let (back, disconnect, end, close) = (find("Return to stream"), find("Disconnect"), find("End session"), find("Close game"));
        assert!(disconnect.top() > back.bottom(), "the ways out sit under Return to stream");
        for other in [end, close] {
            assert!((other.top() - disconnect.top()).abs() < 1.0, "one row: {other:?} beside {disconnect:?}");
            assert!((other.width() - disconnect.width()).abs() < 1.5, "equal widths");
        }
        assert!(disconnect.right() < end.left() && end.right() < close.left(), "Disconnect, End session, Close game");
        assert!((close.right() - back.right()).abs() < 1.5 && (disconnect.left() - back.left()).abs() < 1.5, "as wide as Return to stream together");
        let spoken = nodes.iter().find(|(label, _)| label.starts_with("Disconnect")).unwrap().0.clone();
        assert!(spoken.starts_with("Disconnect, "), "Disconnect's keys are read out: {spoken}");
    }

    /// nightjar#61: Game Bar and Record that, new built-ins, landed ahead of a user's own list. The
    /// saved shortcuts keep their order at the start, whatever built-ins a version adds.
    #[test]
    fn the_saved_shortcuts_come_before_the_built_ins() {
        crate::theme::own_palette();
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        ctx.enable_accesskit();
        let mut saved = Shortcuts::default();
        saved.data.push(Shortcut { id: "s".into(), name: "Steam overlay".into(), keys: vec!["SHIFT".into(), "TAB".into()] });
        let mut nodes = Vec::new();
        for _ in 0..4 {
            nodes = frame(&ctx, |ui| {
                assert_eq!(shortcuts_page(ui, &saved, false), None);
            })
            .0;
        }
        // A saved shortcut also has a row to edit it, under the tiles: the topmost is the tile.
        let find = |name: &str| {
            nodes.iter().filter(|(label, _)| label.starts_with(name)).map(|(_, r)| *r).min_by(|a, b| a.top().total_cmp(&b.top())).unwrap_or_else(|| panic!("{name} in {nodes:#?}"))
        };
        let (own, task_manager, add) = (find("Steam overlay"), find(built_in_label("task_manager")), find(tr("Add shortcut")));
        let reading_order = |a: egui::Rect, b: egui::Rect| a.top() < b.top() - 1.0 || ((a.top() - b.top()).abs() <= 1.0 && a.left() < b.left());
        assert!(reading_order(own, task_manager), "{own:?} before {task_manager:?}");
        assert!(reading_order(task_manager, add), "{task_manager:?} before {add:?}");
    }

    /// Ending the PC's game asks first, with the focus on Cancel so A pressed twice never ends it.
    #[test]
    fn a_way_out_asks_first_with_cancel_focused() {
        crate::theme::own_palette();
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        ctx.enable_accesskit();
        let (mut nodes, mut focus) = (Vec::new(), None);
        for n in 0..3 {
            let (all, focused) = frame(&ctx, |ui| {
                assert_eq!(confirm(ui, HostExit::CloseGame, n == 0), None);
            });
            nodes.extend(all);
            focus = focused.or(focus);
        }
        assert!(nodes.iter().any(|(label, _)| label == tr("Close game")), "{nodes:#?}");
        assert_eq!(focus.as_deref(), Some(tr("Cancel")));
        assert_eq!(Page::Confirm(HostExit::CloseGame).title(), tr("Close game"));
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

    /// Named in the order it is saved and pressed, not the order picked: a Caps Lock then Shift
    /// draft was named "Caps Lock + Shift" though it sends Shift first.
    #[test]
    fn a_draft_is_named_modifiers_first_whatever_order_its_keys_were_picked_in() {
        let mut saved = Shortcuts::default();
        let mut draft = Draft::default();
        draft.toggle("CAPS_LOCK");
        draft.toggle("SHIFT");
        assert!(draft.save(&mut saved));
        assert_eq!((saved.data[0].name.as_str(), &saved.data[0].keys[..]), ("Shift + Caps Lock", &["SHIFT".to_owned(), "CAPS_LOCK".into()][..]));
    }
}
