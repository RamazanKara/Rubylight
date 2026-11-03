//! The stream menu both clients draw: a grid of tiles, four to a row, the user's own shortcuts as
//! tiles at its end with Add after them, and under it Close and the ways out of the session.
//! What a client cannot do here (a remote monitor has no keyboard, a desktop no on-screen
//! controls) it leaves out of its offers; this decides where everything offered sits, in what
//! order, which controller button reaches a tile while the menu is open, and the short name both
//! clients give each tile ([`label`]).
//!
//! Android's `StreamMenu` asks through `StreamMenuModel` (protocol/android, [`json::call`]);
//! desktop Nightjar's stream menu (main.rs) calls [`layout`] directly.

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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
/// in the footer, leaving first, ending the PC's session or game after it.
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
    more("controller_audio"),
    more("files"),
    more("host_web_ui"),
    more("host_status"),
    more("export_statistics"),
    more("help"),
    more("reconnect"),
    footer("disconnect", Some(l("disconnect", "Disconnect"))),
    footer("end_host_session", Some(l("end_session", "End session"))),
    footer("close_game", Some(l("close_game", "Close game"))),
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
    for offer in offers {
        if offer.id == MORE || offer.id == CLOSE || tile(&offer.id).is_none() {
            return Err(UnknownTile(offer.id.clone()));
        }
    }
    let mut layout = Layout { footer: vec![CLOSE], ..Layout::default() };
    for tile in TILES {
        let Some(offer) = offers.iter().find(|o| o.id == tile.id) else { continue };
        match tile.place {
            Place::Grid => layout.grid.push(Cell::Tile { id: tile.id, on: offer.on, hotkey: tile.hotkey }),
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
    /// The grid cell a controller button runs, if a tile with that button is offered.
    pub fn hotkey(&self, key: Hotkey) -> Option<usize> {
        self.grid.iter().position(|cell| matches!(cell, Cell::Tile { hotkey: Some(k), .. } if *k == key))
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
        Layout { offers: Vec<Offer>, shortcuts: Option<usize> },
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
    }

    fn keys(keys: &[&str]) -> Vec<String> {
        keys.iter().map(|k| k.to_string()).collect()
    }

    fn answer(request: Request) -> Result<Value, ShortcutError> {
        Ok(match request {
            Request::Layout { offers, shortcuts } => match layout(&offers, shortcuts) {
                Ok(layout) => serde_json::to_value(layout).expect("layout serializes"),
                Err(UnknownTile(id)) => return Err(ShortcutError::Invalid(format!("unknown tile {id}"))),
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
        })
    }

    /// Answers one request; a malformed one is an error answer, never a panic across JNI.
    pub fn call(request: &str) -> String {
        let result = serde_json::from_str::<Request>(request)
            .map_err(|e| ShortcutError::Invalid(e.to_string()))
            .and_then(answer);
        match result {
            Ok(value) => json!({"ok": value}).to_string(),
            Err(error) => json!({"error": error.code(), "detail": error.detail()}).to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(layout: &Layout) -> Vec<String> {
        layout.grid.iter().map(|cell| match cell {
            Cell::Tile { id, .. } => id.to_string(),
            Cell::Shortcut(i) => format!("#{i}"),
            Cell::Add => "+".into(),
        }).collect()
    }

    #[test]
    fn the_grid_keeps_the_model_order_whatever_order_the_client_offers_in() {
        let offers = [Offer::action("quit_game"), Offer::action("reconnect"), Offer::action("shortcuts"),
            Offer::switch("overlay", true), Offer::action("keyboard"), Offer::action("disconnect"), Offer::switch("controller_mouse", false)];
        let layout = layout(&offers, Some(2)).unwrap();
        assert_eq!(ids(&layout), ["keyboard", "controller_mouse", "overlay", "shortcuts", "more", "#0", "#1", "+"]);
        assert_eq!(layout.more, ["reconnect"]);
        assert_eq!(layout.footer, ["close", "disconnect", "quit_game"]);
        assert_eq!(layout.grid[2], Cell::Tile { id: "overlay", on: Some(true), hotkey: Some(Hotkey::Rb) });
    }

    #[test]
    fn hotkeys_reach_only_offered_tiles() {
        let layout = layout(&[Offer::switch("overlay", false), Offer::action("shortcuts")], Some(0)).unwrap();
        assert_eq!(layout.hotkey(Hotkey::Rb), Some(0));
        assert_eq!(layout.hotkey(Hotkey::Lb), Some(1));
        assert_eq!(layout.hotkey(Hotkey::Y), None);
        assert_eq!(layout.hotkey(Hotkey::X), None);
        // Each button belongs to one tile.
        for key in Hotkey::ALL {
            assert_eq!(TILES.iter().filter(|t| t.hotkey == Some(key)).count(), 1, "{key:?}");
        }
        assert!(TILES.iter().filter(|t| t.hotkey.is_some()).all(|t| t.place == Place::Grid));
    }

    #[test]
    fn no_more_tile_without_anything_behind_it_and_no_shortcuts_without_a_keyboard() {
        let layout = layout(&[Offer::action("overlay_page"), Offer::action("disconnect")], None).unwrap();
        assert_eq!(ids(&layout), ["overlay_page"]);
        assert!(layout.more.is_empty());
        let full = super::layout(&[], Some(shortcuts::MAX_SHORTCUTS)).unwrap();
        assert_eq!(full.grid.len(), shortcuts::MAX_SHORTCUTS);
        assert!(!full.grid.contains(&Cell::Add));
        assert_eq!(super::layout(&[], Some(0)).unwrap().grid, [Cell::Add]);
    }

    #[test]
    fn unknown_or_reserved_offers_are_refused() {
        assert_eq!(layout(&[Offer::action("teleport")], None), Err(UnknownTile("teleport".into())));
        assert_eq!(layout(&[Offer::action(MORE)], None), Err(UnknownTile(MORE.into())));
        assert_eq!(layout(&[Offer::action(CLOSE)], None), Err(UnknownTile(CLOSE.into())));
        let mut seen = std::collections::HashSet::new();
        assert!(TILES.iter().all(|t| seen.insert(t.id)), "a tile listed twice");
    }

    #[test]
    fn json_calls_answer_or_name_the_error() {
        let answer: serde_json::Value = serde_json::from_str(&json::call(
            r#"{"op":"layout","offers":[{"id":"overlay","on":true},{"id":"reconnect"}],"shortcuts":1}"#)).unwrap();
        assert_eq!(answer["ok"]["grid"][0], serde_json::json!({"tile": {"id": "overlay", "on": true, "hotkey": "rb"}}));
        assert_eq!(answer["ok"]["grid"][1], serde_json::json!({"tile": {"id": "more"}}));
        assert_eq!(answer["ok"]["grid"][2], serde_json::json!({"shortcut": 0}));
        assert_eq!(answer["ok"]["grid"][3], serde_json::json!("add"));
        assert_eq!(answer["ok"]["more"], serde_json::json!(["reconnect"]));
        assert_eq!(answer["ok"]["footer"], serde_json::json!(["close"]));

        let answer: serde_json::Value = serde_json::from_str(&json::call(r#"{"op":"layout","offers":[{"id":"warp"}]}"#)).unwrap();
        assert_eq!(answer["error"], "invalid");
        let answer: serde_json::Value = serde_json::from_str(&json::call("not json")).unwrap();
        assert_eq!(answer["error"], "invalid");

        let answer: serde_json::Value = serde_json::from_str(&json::call(r#"{"op":"validate","name":"Snip","keys":["win","Shift","s"]}"#)).unwrap();
        assert_eq!(answer["ok"], serde_json::json!(["WIN", "SHIFT", "S"]));
        let answer: serde_json::Value = serde_json::from_str(&json::call(r#"{"op":"validate","name":"Twice","keys":["CTRL","CTRL_LEFT"]}"#)).unwrap();
        assert_eq!((answer["error"].as_str(), answer["detail"].as_str()), (Some("repeated_key"), Some("CTRL_LEFT")));
        let answer: serde_json::Value = serde_json::from_str(&json::call(r#"{"op":"validate","name":" ","keys":["A"]}"#)).unwrap();
        assert_eq!(answer["error"], "bad_name");

        let answer: serde_json::Value = serde_json::from_str(&json::call(r#"{"op":"built_ins","saved":[["ALT","TAB"],["NOPE"]]}"#)).unwrap();
        let built: Vec<_> = answer["ok"].as_array().unwrap().iter().map(|b| b["id"].as_str().unwrap().to_string()).collect();
        assert_eq!(built, ["task_manager", "close_window", "start_menu", "show_desktop", "game_bar", "record_that"]);
        assert_eq!(answer["ok"][0]["label"], "Ctrl + Shift + Esc");

        let answer: serde_json::Value = serde_json::from_str(&json::call(r#"{"op":"picker"}"#)).unwrap();
        assert_eq!(answer["ok"].as_array().unwrap().len(), shortcuts::PICKER.len());
        assert_eq!(answer["ok"][0], serde_json::json!({"key": "CTRL", "label": "Ctrl", "modifier": true}));
        let answer: serde_json::Value = serde_json::from_str(&json::call(r#"{"op":"chord_label","keys":["ALT","F4"]}"#)).unwrap();
        assert_eq!(answer["ok"], "Alt + F4");

        let answer: serde_json::Value =
            serde_json::from_str(&json::call(r#"{"op":"mode_line","width":2560,"height":1440,"fps":144,"format":512,"hdr":true}"#)).unwrap();
        assert_eq!(answer["ok"], "1440p · 144 FPS · HEVC HDR");
        let answer: serde_json::Value = serde_json::from_str(&json::call(r#"{"op":"mode_line","width":0,"height":0,"fps":0,"format":0,"hdr":false}"#)).unwrap();
        assert_eq!(answer, serde_json::json!({"ok": null}));
    }

    /// nightjar#63: a tile has one short name on both clients, so every tile the grid or its footer
    /// can show has a label, one key always means the same words, and the words stay short.
    #[test]
    fn every_tile_in_the_grid_or_under_it_has_one_short_label() {
        for tile in TILES {
            match tile.place {
                Place::Grid => assert!(tile.label.is_some(), "{} has no label", tile.id),
                Place::More => assert_eq!(tile.label, None, "{}: a More row keeps the client's words", tile.id),
                Place::Footer => {}
            }
        }
        for id in ["disconnect", "end_host_session", "close_game", "quit_game", MORE, CLOSE, ADD] {
            assert!(label(id).is_some(), "{id}");
        }
        assert_eq!(label("bitrate"), label("rate_change"), "both change the quality");
        assert_eq!(label("controller_mouse").map(|l| l.en), Some("Mouse"));
        assert_eq!(label("server_commands").map(|l| l.en), Some("PC commands"));
        assert_eq!(label("clipboard_receive").map(|l| l.en), Some("Get clipboard"));
        assert_eq!(label("reconnect"), None);
        assert_eq!(label("teleport"), None);
        let all = labels();
        for label in &all {
            let same: Vec<_> = TILES.iter().filter_map(|t| t.label).filter(|l| l.key == label.key).collect();
            assert!(same.iter().all(|l| l == label), "{} names two texts", label.key);
            assert!(label.key.chars().all(|c| c.is_ascii_lowercase() || c == '_'), "{} is not a resource name", label.key);
            assert!(label.en.chars().count() <= 20, "{:?} is too long for a tile", label.en);
            assert!(label.en.split(' ').count() <= 3, "{:?} takes more than two lines", label.en);
        }
        assert_eq!(all.len(), all.iter().map(|l| l.key).collect::<std::collections::HashSet<_>>().len());
    }

    #[test]
    fn the_labels_call_names_each_tile_s_key() {
        let answer: serde_json::Value = serde_json::from_str(&json::call(r#"{"op":"labels"}"#)).unwrap();
        assert_eq!(answer["ok"]["bitrate"], "quality");
        assert_eq!(answer["ok"]["rate_change"], "quality");
        assert_eq!(answer["ok"]["close"], "return");
        assert_eq!(answer["ok"]["add"], "add_shortcut");
        assert_eq!(answer["ok"]["more"], "more");
        assert_eq!(answer["ok"]["quit_game"], "quit_game");
        assert!(answer["ok"].get("reconnect").is_none());
        assert!(answer["ok"].get("end_session").is_none());
    }

    /// nightjar#63: the header says the mode the way players do, "1440p · 144 FPS", with the codec
    /// and HDR after it only while the line stays short.
    #[test]
    fn the_header_line_names_the_mode_like_players_do() {
        assert_eq!(mode_line(2560, 1440, 144, 0, false).as_deref(), Some("1440p · 144 FPS"));
        assert_eq!(mode_line(2560, 1440, 144, 0x0100, false).as_deref(), Some("1440p · 144 FPS · HEVC"));
        assert_eq!(mode_line(1920, 1080, 60, 0x0001, false).as_deref(), Some("1080p · 60 FPS · H.264"));
        assert_eq!(mode_line(3840, 2160, 120, 0x2000, true).as_deref(), Some("4K · 120 FPS · AV1 HDR"));
        assert_eq!(mode_line(2560, 1600, 120, 0x1000, false).as_deref(), Some("1600p · 120 FPS · AV1"), "16:10 is named by its height");
        assert_eq!(mode_line(1080, 1920, 60, 0x0100, false).as_deref(), Some("1080p · 60 FPS · HEVC"), "portrait by its short side");
        assert_eq!(mode_line(1366, 768, 60, 0, false).as_deref(), Some("768p · 60 FPS"));
        assert_eq!(mode_line(1280, 720, 60, 0x01_0000, false).as_deref(), Some("720p · 60 FPS · PyroWave"));
        // Wider than 16:9: the full size, and the codec gives way before HDR when the line runs long.
        assert_eq!(mode_line(3440, 1440, 144, 0x1000, false).as_deref(), Some("3440×1440 · 144 FPS · AV1"));
        assert_eq!(mode_line(3440, 1440, 100, 0x0200, true).as_deref(), Some("3440×1440 · 100 FPS · HDR"));
        assert_eq!(mode_line(5120, 1440, 240, 0x01_0000, false).as_deref(), Some("5120×1440 · 240 FPS"));
        for (w, h, fps) in [(0, 1080, 60), (1920, 0, 60), (1920, 1080, 0)] {
            assert_eq!(mode_line(w, h, fps, 0x0100, true), None, "{w}x{h}@{fps} is not known yet");
        }
        for line in [mode_line(3440, 1440, 100, 0x0200, true), mode_line(3840, 2160, 120, 0x2000, true)] {
            assert!(line.unwrap().chars().count() <= MODE_LINE_MAX);
        }
    }
}
