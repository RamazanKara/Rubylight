//! The stream menu both clients draw: a grid of tiles, four to a row, the user's own shortcuts as
//! tiles at its end with Add after them, and under it Close and the ways out of the session.
//! What a client cannot do here (a remote monitor has no keyboard, a desktop no on-screen
//! controls) it leaves out of its offers; this decides where everything offered sits, in what
//! order, which controller button reaches a tile while the menu is open, and the short name both
//! clients give each tile ([`label`]).
//!
//! Android's `StreamMenu` asks through `StreamMenuModel` (protocol/android, [`json::call`]);
//! desktop Nightjar's stream menu (main.rs) calls [`layout`] directly.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::shortcuts::{self, ShortcutError};

/// Tiles in a row of the grid.
pub const COLUMNS: usize = 4;

/// Where a tile sits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    /// On the menu's first page.
    Grid,
    /// One level down, behind the More tile.
    More,
    /// Under the grid, after Close.
    Footer,
}

/// A controller button that runs a tile while the menu shows its first page. B closes the menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Hotkey {
    Y,
    X,
    Lb,
    Rb,
}

impl Hotkey {
    pub const ALL: [Hotkey; 4] = [Hotkey::Y, Hotkey::X, Hotkey::Lb, Hotkey::Rb];

    /// The button as the tile's corner shows it.
    pub fn cap(self) -> &'static str {
        match self {
            Hotkey::Y => "Y",
            Hotkey::X => "X",
            Hotkey::Lb => "LB",
            Hotkey::Rb => "RB",
        }
    }
}

/// A tile's short name, the same on both clients: `key` names Android's `stream_tile_<key>` string
/// and `en` is the English text desktop Nightjar translates with `tr`. Short enough for a compact
/// tile: two lines at most, in English and German.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Label {
    pub key: &'static str,
    pub en: &'static str,
}

const fn l(key: &'static str, en: &'static str) -> Label {
    Label { key, en }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tile {
    /// Stable: each client maps it to its icon and action.
    pub id: &'static str,
    pub place: Place,
    pub hotkey: Option<Hotkey>,
    /// Every tile in the grid or under it has one; a More page's rows say more and keep the
    /// client's own words.
    pub label: Option<Label>,
}

const fn grid(id: &'static str, hotkey: Option<Hotkey>, label: Label) -> Tile {
    Tile { id, place: Place::Grid, hotkey, label: Some(label) }
}

const fn more(id: &'static str) -> Tile {
    Tile { id, place: Place::More, hotkey: None, label: None }
}

const fn footer(id: &'static str, label: Option<Label>) -> Tile {
    Tile { id, place: Place::Footer, hotkey: None, label }
}

/// The first page's own tile for what sits one level down.
pub const MORE: &str = "more";
/// Closes the menu; always first in the footer.
pub const CLOSE: &str = "close";

/// Every tile either client offers, in the order the menu shows them. The keyboards, the mouse,
/// the overlay and the shortcuts come first, each with a button; the ways out of the session sit
/// in the footer, leaving first and quitting the game after it. Ending the PC's session while its
/// game keeps running is rarer and sits last under More, so the footer never offers three exits.
pub const TILES: &[Tile] = &[
    grid("keyboard", Some(Hotkey::Y), l("keyboard", "Keyboard")),
    grid("pc_keyboard", None, l("pc_keyboard", "PC keyboard")),
    grid("controller_mouse", Some(Hotkey::X), l("mouse", "Mouse")),
    grid("overlay", Some(Hotkey::Rb), l("overlay", "Overlay")),
    grid("shortcuts", Some(Hotkey::Lb), l("shortcuts", "Shortcuts")),
    grid("microphone", None, l("microphone", "Microphone")),
    grid("touch_mode", None, l("touch_mode", "Touch mode")),
    grid("onscreen_controls", None, l("onscreen_controls", "On-screen controls")),
    grid("bitrate", None, l("quality", "Quality")),
    grid("rate_change", None, l("quality", "Quality")),
    grid("video_output", None, l("video_output", "Video output")),
    grid("zoom", None, l("zoom", "Zoom")),
    grid("clipboard_send", None, l("send_clipboard", "Send clipboard")),
    grid("clipboard_receive", None, l("get_clipboard", "Get clipboard")),
    grid("server_commands", None, l("pc_commands", "PC commands")),
    grid("overlay_page", None, l("diagnostics", "Diagnostics")),
    grid("native_page", None, l("session_graphics", "Session and graphics")),
    more("mouse_cursor"),
    more("keyboard_keys"),
    more("controls_layout"),
    more("mic_button"),
    more("menu_button"),
    more("game_controls"),
    more("picture"),
    more("video_scaling"),
    more("zoom_reset"),
    more("picture_in_picture"),
    more("automatic_quality"),
    more("automatic_bitrate"),
    more("stream_size"),
    more("stream_health"),
    more("night_mode"),
    more("headphone_surround"),
    more("image_clipboard_send"),
    more("image_clipboard_receive"),
    more("host_menu"),
    more("usb_devices"),
    more("guests"),
    more("controller_audio"),
    more("files"),
    more("host_web_ui"),
    more("host_status"),
    more("export_statistics"),
    more("help"),
    more("reconnect"),
    more("end_host_session"),
    footer("disconnect", Some(l("disconnect", "Disconnect"))),
    footer("close_game", Some(l("close_game", "Quit game"))),
    footer("quit_game", Some(l("quit_game", "Quit game"))),
    footer("end_session", None),
];

pub fn tile(id: &str) -> Option<&'static Tile> {
    TILES.iter().find(|t| t.id == id)
}

/// The grid's tile that saves a new shortcut, as [`label`] names it ([`Cell::Add`] in a layout).
pub const ADD: &str = "add";

/// The short name both clients give a tile in the grid or under it: every grid tile, More, Add,
/// Return to stream ([`CLOSE`]) and the ways out. None for a More page's rows and for a way out
/// only one client offers in words of its own.
pub fn label(id: &str) -> Option<Label> {
    match id {
        MORE => Some(l("more", "More")),
        CLOSE => Some(l("return", "Return to stream")),
        ADD => Some(l("add_shortcut", "Add shortcut")),
        _ => tile(id).and_then(|t| t.label),
    }
}

/// Every label [`label`] gives, each key once, for the clients' tests.
pub fn labels() -> Vec<Label> {
    let mut all: Vec<Label> = Vec::new();
    for id in [MORE, CLOSE, ADD].into_iter().chain(TILES.iter().map(|t| t.id)) {
        if let Some(label) = label(id) {
            if !all.iter().any(|known| known.key == label.key) {
                all.push(label);
            }
        }
    }
    all
}

/// A tile the client can show now, and its state when it is a switch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Offer {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<bool>,
}

impl Offer {
    pub fn action(id: &str) -> Self {
        Self { id: id.into(), on: None }
    }

    pub fn switch(id: &str, on: bool) -> Self {
        Self { id: id.into(), on: Some(on) }
    }
}

/// One place in the grid.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Cell {
    Tile {
        id: &'static str,
        #[serde(skip_serializing_if = "Option::is_none")]
        on: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        hotkey: Option<Hotkey>,
    },
    /// The user's saved shortcut at this index of their list.
    Shortcut(usize),
    /// Saves a new shortcut from the stream.
    Add,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Layout {
    pub grid: Vec<Cell>,
    /// One level down, in order.
    pub more: Vec<&'static str>,
    /// Close, then the ways out of the session.
    pub footer: Vec<&'static str>,
    /// What each button runs now, in the grid or under it: only what is offered.
    pub buttons: BTreeMap<Hotkey, &'static str>,
}

/// A button's choice in [`Buttons`] that leaves it doing nothing in the menu.
pub const NO_TILE: &str = "none";

/// What the user chose for the controller's buttons in the menu, by tile ID or [`NO_TILE`]. A
/// button without a choice keeps its tile from [`TILES`], and so does one whose choice this version
/// cannot assign, as a settings file from another version may name.
pub type Buttons = BTreeMap<Hotkey, String>;

/// Whether a button can be given this tile: what shows on the first page, in the grid or under it.
/// Quality is one choice, as `bitrate`, and Quit game one, as `quit_game` (both read Quit game);
/// [`layout_with_buttons`] finds whichever a stream offers.
pub fn assignable(tile: &Tile) -> bool {
    tile.label.is_some() && tile.place != Place::More && tile.id != "rate_change" && tile.id != "close_game"
}

/// A saved choice as this version reads it: a settings file from before Quit game was one choice
/// may name close_game, which is Quit game now.
pub fn saved_choice(saved: &str) -> &str {
    if saved == "close_game" { "quit_game" } else { saved }
}

fn button_tile(key: Hotkey, buttons: &Buttons) -> Option<&'static str> {
    let default = || TILES.iter().find(|t| t.hotkey == Some(key)).map(|t| t.id);
    match buttons.get(&key).map(|saved| saved_choice(saved)) {
        Some(NO_TILE) => None,
        Some(id) => tile(id).filter(|t| assignable(t)).map(|t| t.id).or_else(default),
        None => default(),
    }
}

/// An offer this model does not know: the client and the model disagree, which a test catches.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnknownTile(pub String);

impl std::fmt::Display for UnknownTile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "unknown stream menu tile {:?}", self.0)
    }
}

/// The menu for what the client offers and how many shortcuts the user saved. `shortcuts` is
/// `None` where shortcuts cannot be sent (no keyboard permission, a remote monitor): then
/// neither they nor Add show.
pub fn layout(offers: &[Offer], shortcuts: Option<usize>) -> Result<Layout, UnknownTile> {
    layout_with_buttons(offers, shortcuts, &Buttons::new())
}

/// [`layout`] with the user's choices for the controller's buttons.
pub fn layout_with_buttons(offers: &[Offer], shortcuts: Option<usize>, buttons: &Buttons) -> Result<Layout, UnknownTile> {
    for offer in offers {
        if offer.id == MORE || offer.id == CLOSE || tile(&offer.id).is_none() {
            return Err(UnknownTile(offer.id.clone()));
        }
    }
    let mut layout = Layout { footer: vec![CLOSE], ..Layout::default() };
    for key in Hotkey::ALL {
        let offered = |id: &str| offers.iter().any(|o| o.id == id);
        let target = button_tile(key, buttons).and_then(|id| match id {
            "bitrate" if !offered("bitrate") && offered("rate_change") => Some("rate_change"),
            "quit_game" if !offered("quit_game") && offered("close_game") => Some("close_game"),
            id => offered(id).then_some(id),
        });
        if let Some(id) = target {
            layout.buttons.insert(key, id);
        }
    }
    for tile in TILES {
        let Some(offer) = offers.iter().find(|o| o.id == tile.id) else { continue };
        match tile.place {
            Place::Grid => {
                // A tile with two buttons shows the first.
                let hotkey = Hotkey::ALL.into_iter().find(|key| layout.buttons.get(key) == Some(&tile.id));
                layout.grid.push(Cell::Tile { id: tile.id, on: offer.on, hotkey });
            }
            Place::More => layout.more.push(tile.id),
            Place::Footer => layout.footer.push(tile.id),
        }
    }
    if !layout.more.is_empty() {
        layout.grid.push(Cell::Tile { id: MORE, on: None, hotkey: None });
    }
    if let Some(count) = shortcuts {
        layout.grid.extend((0..count.min(shortcuts::MAX_SHORTCUTS)).map(Cell::Shortcut));
        if count < shortcuts::MAX_SHORTCUTS {
            layout.grid.push(Cell::Add);
        }
    }
    Ok(layout)
}

impl Layout {
    /// The grid cell a controller button runs, if a tile with that button is offered. A way out
    /// under the grid has no cell: see [`Self::buttons`].
    pub fn hotkey(&self, key: Hotkey) -> Option<usize> {
        let id = *self.buttons.get(&key)?;
        self.grid.iter().position(|cell| matches!(cell, Cell::Tile { id: tile, .. } if *tile == id))
    }

    /// The tile ID in a grid cell, or None for a shortcut or Add.
    pub fn tile_id(&self, index: usize) -> Option<&'static str> {
        match self.grid.get(index) {
            Some(Cell::Tile { id, .. }) => Some(id),
            _ => None,
        }
    }
}

// Moonlight's video format bits (moonlight-common-c's VIDEO_FORMAT_MASK_*), as both clients
// report the negotiated format.
const FORMAT_H264: u32 = 0x000F;
const FORMAT_HEVC: u32 = 0x0F00;
const FORMAT_AV1: u32 = 0xF000;
const FORMAT_PYROWAVE: u32 = 0x0F_0000;

/// The longest header line that still takes the codec or HDR after the mode.
pub const MODE_LINE_MAX: usize = 28;

/// The stream's size as players name it: `1440p` for 16:9 and narrower shapes (by the short
/// side, so a portrait stream reads the same), `4K` and `8K`, and the full `3440×1440` for
/// anything wider than 16:9.
pub fn resolution_label(width: u32, height: u32) -> String {
    let (long, short) = (width.max(height), width.min(height));
    // 1% slack, so 1366×768 still counts as 16:9.
    if u64::from(long) * 900 > u64::from(short) * 1616 {
        return format!("{width}×{height}");
    }
    match short {
        2160 => "4K".into(),
        4320 => "8K".into(),
        _ => format!("{short}p"),
    }
}

fn codec_label(format: u32) -> Option<&'static str> {
    if format & FORMAT_PYROWAVE != 0 {
        Some("PyroWave")
    } else if format & FORMAT_AV1 != 0 {
        Some("AV1")
    } else if format & FORMAT_HEVC != 0 {
        Some("HEVC")
    } else if format & FORMAT_H264 != 0 {
        Some("H.264")
    } else {
        None
    }
}

/// The stream menu's header line, as both clients show it under the game's name: `1440p · 144 FPS`,
/// then the codec and HDR (`· HEVC HDR`) while the line stays within [`MODE_LINE_MAX`]
/// characters; the codec goes first when it does not fit. `format` is the negotiated video format
/// (0 while unknown). None while the stream's size or rate is not known yet.
pub fn mode_line(width: u32, height: u32, fps: u32, format: u32, hdr: bool) -> Option<String> {
    if width == 0 || height == 0 || fps == 0 {
        return None;
    }
    let mode = format!("{} · {fps} FPS", resolution_label(width, height));
    let hdr = hdr.then_some("HDR");
    let tails = [[codec_label(format), hdr], [None, hdr]];
    for tail in tails {
        let tail: Vec<&str> = tail.into_iter().flatten().collect();
        if tail.is_empty() {
            continue;
        }
        let line = format!("{mode} · {}", tail.join(" "));
        if line.chars().count() <= MODE_LINE_MAX {
            return Some(line);
        }
    }
    Some(mode)
}

/// The JSON calls Android makes through JNI (`StreamMenuModel.call`): one request in, one answer
/// out, `{"ok": …}` or `{"error": code, "detail": …}`.
pub mod json {
    use super::*;
    use serde_json::{json, Value};

    #[derive(Deserialize)]
    #[serde(tag = "op", rename_all = "snake_case")]
    enum Request {
        /// The menu: `offers` and the saved shortcuts' count (absent when they cannot be sent).
        Layout { offers: Vec<Offer>, shortcuts: Option<usize>, #[serde(default)] buttons: Buttons },
        /// The built-ins not saved under a name of their own, given each saved shortcut's keys.
        BuiltIns { saved: Vec<Vec<String>> },
        /// The key picker's keys.
        Picker,
        /// A shortcut about to be saved: its keys in their saved names, or why it is refused.
        Validate { name: String, keys: Vec<String> },
        /// A chord's text, `Ctrl + Shift + Esc`.
        ChordLabel { keys: Vec<String> },
        /// The header's line for the stream's mode ([`mode_line`]); null while it is not known.
        ModeLine { width: u32, height: u32, fps: u32, format: u32, hdr: bool },
        /// Each tile ID's label key ([`label`]), for the grid, More, Add, Return and the ways out.
        Labels,
        /// Android's stream hotkeys with the user's `bindings` ([`shortcuts::bound`]).
        Hotkeys { bindings: shortcuts::HotkeyBindings },
        /// A hotkey's new chord: its saved keys, or why it is refused ([`shortcuts::check_hotkey`]).
        CheckHotkey { id: String, keys: Vec<String>, bindings: shortcuts::HotkeyBindings },
        /// Whether a hotkey may go back to its default: null, or why not ([`shortcuts::check_reset`]).
        CheckReset { id: String, bindings: shortcuts::HotkeyBindings },
    }

    /// Why a request was refused: a code and what it names.
    struct Refusal(&'static str, Option<String>);

    impl From<ShortcutError> for Refusal {
        fn from(error: ShortcutError) -> Self {
            Refusal(error.code(), error.detail().map(str::to_string))
        }
    }

    impl From<shortcuts::HotkeyError> for Refusal {
        fn from(error: shortcuts::HotkeyError) -> Self {
            Refusal(error.code(), error.detail().map(str::to_string))
        }
    }

    /// A chord as the app matches it: the keys, their text and each key's (the app translates the
    /// words desktop Nightjar does), the modifiers in the input packets' bits and the other key's
    /// Windows virtual-key code.
    fn chord(keys: &[String]) -> Value {
        let (modifiers, vk) = shortcuts::hotkey_code(keys);
        let caps: Vec<String> = keys.iter().map(|key| shortcuts::key_label(key)).collect();
        json!({"keys": keys, "label": shortcuts::chord_label(keys), "caps": caps, "modifiers": modifiers, "vk": vk})
    }

    fn keys(keys: &[&str]) -> Vec<String> {
        keys.iter().map(|k| k.to_string()).collect()
    }

    fn answer(request: Request) -> Result<Value, Refusal> {
        Ok(match request {
            Request::Layout { offers, shortcuts, buttons } => match layout_with_buttons(&offers, shortcuts, &buttons) {
                Ok(layout) => serde_json::to_value(layout).expect("layout serializes"),
                Err(UnknownTile(id)) => return Err(ShortcutError::Invalid(format!("unknown tile {id}")).into()),
            },
            Request::BuiltIns { saved } => {
                let saved: Vec<_> = saved.into_iter().map(|keys| shortcuts::Shortcut { id: String::new(), name: String::new(), keys }).collect();
                Value::Array(shortcuts::built_ins(&saved).iter().map(|b| {
                    let keys = keys(b.keys);
                    json!({"id": b.id, "label": shortcuts::chord_label(&keys), "keys": keys})
                }).collect())
            }
            Request::Picker => Value::Array(shortcuts::PICKER.iter().map(|key| {
                json!({"key": key, "label": shortcuts::key_label(key), "modifier": shortcuts::is_modifier(key)})
            }).collect()),
            Request::Validate { name, keys } => {
                let mut scratch = shortcuts::Shortcuts::default();
                scratch.save(None, &name, &keys, "check")?;
                json!(scratch.data[0].keys)
            }
            Request::ChordLabel { keys } => json!(shortcuts::chord_label(&keys)),
            Request::ModeLine { width, height, fps, format, hdr } => json!(mode_line(width, height, fps, format, hdr)),
            Request::Labels => {
                let ids = [MORE, CLOSE, ADD].into_iter().chain(TILES.iter().map(|t| t.id));
                Value::Object(ids.filter_map(|id| label(id).map(|l| (id.to_string(), json!(l.key)))).collect())
            }
            Request::Hotkeys { bindings } => {
                let client = shortcuts::Client::Android;
                Value::Array(shortcuts::bound(client, &bindings).iter().map(|b| {
                    let default = keys(shortcuts::find_hotkey(b.id).expect("listed").keys);
                    json!({
                        "id": b.id,
                        "chord": b.keys.as_deref().map(chord),
                        "default": chord(&default),
                        "changed": b.changed,
                        "old": b.old.iter().map(|k| chord(k)).collect::<Vec<_>>(),
                        "taken": b.taken,
                    })
                }).collect())
            }
            Request::CheckHotkey { id, keys, bindings } => {
                json!(shortcuts::check_hotkey(shortcuts::Client::Android, &id, &keys, &bindings)?)
            }
            Request::CheckReset { id, bindings } => {
                shortcuts::check_reset(shortcuts::Client::Android, &id, &bindings)?;
                Value::Null
            }
        })
    }

    /// Answers one request; a malformed one is an error answer, never a panic across JNI.
    pub fn call(request: &str) -> String {
        let result = serde_json::from_str::<Request>(request)
            .map_err(|e| ShortcutError::Invalid(e.to_string()).into())
            .and_then(answer);
        match result {
            Ok(value) => json!({"ok": value}).to_string(),
            Err(Refusal(code, detail)) => json!({"error": code, "detail": detail}).to_string(),
        }
    }
}

#[cfg(test)]
mod tests;
