//! The stream menu both clients draw: a grid of tiles, four to a row, the user's own shortcuts as
//! tiles at its end with Add after them, and under it Close and the ways out of the session.
//! What a client cannot do here (a remote monitor has no keyboard, a desktop no on-screen
//! controls) it leaves out of its offers; this decides where everything offered sits, in what
//! order, and which controller button reaches a tile while the menu is open.
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tile {
    /// Stable: each client maps it to its label, icon and action.
    pub id: &'static str,
    pub place: Place,
    pub hotkey: Option<Hotkey>,
}

const fn grid(id: &'static str, hotkey: Option<Hotkey>) -> Tile {
    Tile { id, place: Place::Grid, hotkey }
}

const fn more(id: &'static str) -> Tile {
    Tile { id, place: Place::More, hotkey: None }
}

const fn footer(id: &'static str) -> Tile {
    Tile { id, place: Place::Footer, hotkey: None }
}

/// The first page's own tile for what sits one level down.
pub const MORE: &str = "more";
/// Closes the menu; always first in the footer.
pub const CLOSE: &str = "close";

/// Every tile either client offers, in the order the menu shows them. The keyboards, the mouse,
/// the overlay and the shortcuts come first, each with a button; the ways out of the session sit
/// in the footer, leaving first, ending the PC's session or game after it.
pub const TILES: &[Tile] = &[
    grid("keyboard", Some(Hotkey::Y)),
    grid("pc_keyboard", None),
    grid("controller_mouse", Some(Hotkey::X)),
    grid("overlay", Some(Hotkey::Rb)),
    grid("shortcuts", Some(Hotkey::Lb)),
    grid("microphone", None),
    grid("touch_mode", None),
    grid("onscreen_controls", None),
    grid("bitrate", None),
    grid("rate_change", None),
    grid("video_output", None),
    grid("zoom", None),
    grid("clipboard_send", None),
    grid("clipboard_receive", None),
    grid("server_commands", None),
    grid("overlay_page", None),
    grid("native_page", None),
    more("mouse_cursor"),
    more("keyboard_keys"),
    more("controls_layout"),
    more("mic_button"),
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
    footer("disconnect"),
    footer("end_host_session"),
    footer("close_game"),
    footer("quit_game"),
    footer("end_session"),
];

pub fn tile(id: &str) -> Option<&'static Tile> {
    TILES.iter().find(|t| t.id == id)
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
        assert_eq!(built, ["task_manager", "close_window", "start_menu", "show_desktop"]);
        assert_eq!(answer["ok"][0]["label"], "Ctrl + Shift + Esc");

        let answer: serde_json::Value = serde_json::from_str(&json::call(r#"{"op":"picker"}"#)).unwrap();
        assert_eq!(answer["ok"].as_array().unwrap().len(), shortcuts::PICKER.len());
        assert_eq!(answer["ok"][0], serde_json::json!({"key": "CTRL", "label": "Ctrl", "modifier": true}));
        let answer: serde_json::Value = serde_json::from_str(&json::call(r#"{"op":"chord_label","keys":["ALT","F4"]}"#)).unwrap();
        assert_eq!(answer["ok"], "Alt + F4");
    }
}
