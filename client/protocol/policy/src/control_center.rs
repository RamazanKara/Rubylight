//! The Control Center both clients open over a stream: a rail of eight modules, each a page of
//! cards and rows, with Return to stream and the ways out under every page. It is the stream
//! menu's content ([`crate::stream_menu`]) sorted by what it is about instead of one grid and a
//! More list: every tile the menu has sits in exactly one module, so nothing a client offers is
//! lost, and a module nothing is offered for does not show.
//!
//! The controller's four menu buttons (Y, X, LB, RB) stay the user's own choices; the triggers
//! step through the modules. What a later PC feature adds ([`Extra`]) shows only when the PC says
//! it has that feature ([`Gates`], filled from `HostCapabilities`, never from a PC's brand).
//!
//! [`surface`] decides what the open Control Center is drawn on. Closed, it is nothing at all: no
//! window, no blur pass, no surface.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::glass_probe::Tier;
use crate::stream_menu::{self, Buttons, Cell, Hotkey, Label, Offer, UnknownTile};
use crate::viewport_scale::FormFactor;

/// A page of the Control Center.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Module {
    /// The keyboards, the mouse, the overlay, the clipboard and the user's shortcuts.
    Quick,
    /// Quality, the picture and how it is shown.
    Picture,
    /// The microphone and where sound plays.
    Audio,
    /// Saved clips and recording.
    Clips,
    /// The PC: its session, its graphics, its commands and files.
    Pc,
    /// On-screen controls, their layout and attached devices.
    Controller,
    /// Who else plays.
    Guests,
    /// Reconnecting and ending the PC's session.
    Power,
}

impl Module {
    pub const ALL: [Module; 8] =
        [Module::Quick, Module::Picture, Module::Audio, Module::Clips, Module::Pc, Module::Controller, Module::Guests, Module::Power];

    /// The name under its icon on the rail: `key` names Android's `control_center_<key>` string
    /// and `en` is the English text the desktop translates. One short word, so eight fit a
    /// phone's width.
    pub fn label(self) -> Label {
        let (key, en) = match self {
            Module::Quick => ("quick", "Quick"),
            Module::Picture => ("picture", "Picture"),
            Module::Audio => ("audio", "Audio"),
            Module::Clips => ("clips", "Clips"),
            Module::Pc => ("pc", "PC"),
            Module::Controller => ("controller", "Controller"),
            Module::Guests => ("guests", "Guests"),
            Module::Power => ("power", "Power"),
        };
        Label { key, en }
    }

    /// A stable name for the JNI, icons and tests.
    pub fn key(self) -> &'static str {
        self.label().key
    }

    pub fn parse(key: &str) -> Option<Module> {
        Module::ALL.into_iter().find(|module| module.key() == key)
    }
}

/// The module a stream menu tile belongs to. The ways out that sit under every page
/// ([`FOOTER`]) and the menu's own More and Close have none.
pub fn module_of(tile: &str) -> Option<Module> {
    Some(match tile {
        "keyboard" | "pc_keyboard" | "controller_mouse" | "overlay" | "shortcuts" | "touch_mode" | "zoom" | "clipboard_send"
        | "clipboard_receive" | "mouse_cursor" | "keyboard_keys" | "zoom_reset" | "picture_in_picture" | "image_clipboard_send"
        | "image_clipboard_receive" | "help" => Module::Quick,
        "bitrate" | "rate_change" | "video_output" | "overlay_page" | "picture" | "video_scaling" | "automatic_quality"
        | "automatic_bitrate" | "stream_size" | "stream_health" | "export_statistics" | "picture_position"
        | "flip_screen" => Module::Picture,
        // The PC's night mode and headphone surround are sound, beside the microphone.
        "microphone" | "mic_button" | "night_mode" | "headphone_surround" | "controller_audio" => Module::Audio,
        "native_page" | "server_commands" | "host_menu" | "files" | "host_web_ui" | "host_status" => Module::Pc,
        "onscreen_controls" | "controls_layout" | "menu_button" | "game_controls" | "usb_devices" | "controller_settings" => {
            Module::Controller
        }
        "guests" => Module::Guests,
        // Quick Suspend: the host offers one of the two, as its game runs or waits paused.
        "suspend_game" | "resume_game" | "reconnect" | "end_host_session" | "end_session" => Module::Power,
        _ => return None,
    })
}

/// The ways out under every page, after Return to stream: leaving, then quitting the game. A
/// stream offers one of the two Quit game tiles.
pub const FOOTER: [&str; 3] = ["disconnect", "close_game", "quit_game"];

/// What the PC says it can do, as the Control Center needs it. Each is a `HostCapabilities`
/// feature of a later part of 7.0; until that part lands every client passes `false`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Gates {
    pub stream_dvr: bool,
    pub hdr_sdr_tone_map: bool,
    pub frame_warp: bool,
    pub party_audio: bool,
    pub input_deck: bool,
    pub quick_suspend: bool,
}

/// A card a later PC feature adds to a module. A client offers it like a tile; it shows only
/// when the PC has the feature.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Extra {
    pub id: &'static str,
    pub module: Module,
    gate: Gate,
}

/// The PC feature a gated card waits for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Gate {
    StreamDvr,
    HdrSdrToneMap,
    FrameWarp,
    PartyAudio,
    InputDeck,
    QuickSuspend,
}

impl Extra {
    pub fn allowed(&self, gates: &Gates) -> bool {
        match self.gate {
            Gate::StreamDvr => gates.stream_dvr,
            Gate::HdrSdrToneMap => gates.hdr_sdr_tone_map,
            Gate::FrameWarp => gates.frame_warp,
            Gate::PartyAudio => gates.party_audio,
            Gate::InputDeck => gates.input_deck,
            Gate::QuickSuspend => gates.quick_suspend,
        }
    }
}

/// Every gated card, in the order its module shows them, after the module's tiles.
pub const EXTRAS: &[Extra] = &[
    Extra { id: "tone_map", module: Module::Picture, gate: Gate::HdrSdrToneMap },
    Extra { id: "frame_warp", module: Module::Picture, gate: Gate::FrameWarp },
    Extra { id: "party_mix", module: Module::Audio, gate: Gate::PartyAudio },
    Extra { id: "save_clip", module: Module::Clips, gate: Gate::StreamDvr },
    Extra { id: "record", module: Module::Clips, gate: Gate::StreamDvr },
    Extra { id: "input_deck", module: Module::Controller, gate: Gate::InputDeck },
    Extra { id: "quick_suspend", module: Module::Power, gate: Gate::QuickSuspend },
];

pub fn extra(id: &str) -> Option<&'static Extra> {
    EXTRAS.iter().find(|extra| extra.id == id)
}

/// One module's page.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Page {
    pub module: Module,
    /// Its cards, in order: the menu's first-page tiles of this module, then gated extras, and on
    /// Quick the user's shortcuts and Add.
    pub cards: Vec<Cell>,
    /// Its rows under the cards: what the menu kept one level down.
    pub rows: Vec<&'static str>,
}

impl Page {
    /// How many things can take focus on it.
    pub fn len(&self) -> usize {
        self.cards.len() + self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// The Control Center for what a client offers.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Dock {
    /// The modules that have something, in rail order. Quick is always there.
    pub pages: Vec<Page>,
    /// Return to stream, then the ways out: under every page.
    pub footer: Vec<&'static str>,
    /// What each controller button runs now: only what is offered.
    pub buttons: BTreeMap<Hotkey, &'static str>,
}

impl Dock {
    pub fn page(&self, module: Module) -> Option<&Page> {
        self.pages.iter().find(|page| page.module == module)
    }

    /// The module a step along the rail lands on from `from`: the triggers and the rail's own
    /// arrows. It stops at both ends, so a held trigger does not spin.
    pub fn step(&self, from: Module, forward: bool) -> Module {
        let Some(at) = self.pages.iter().position(|page| page.module == from) else {
            return self.pages.first().map_or(Module::Quick, |page| page.module);
        };
        let to = if forward { (at + 1).min(self.pages.len() - 1) } else { at.saturating_sub(1) };
        self.pages[to].module
    }

    /// Where a controller button's tile is: its module, and its card there. A way out under the
    /// pages has no card.
    pub fn hotkey(&self, key: Hotkey) -> Option<(Module, usize)> {
        let id = *self.buttons.get(&key)?;
        self.pages.iter().find_map(|page| {
            let at = page.cards.iter().position(|cell| matches!(cell, Cell::Tile { id: tile, .. } if *tile == id))?;
            Some((page.module, at))
        })
    }
}

/// The Control Center for these offers: [`stream_menu::layout_with_buttons`] sorted into
/// modules. `shortcuts` is `None` where shortcuts cannot be sent. An offer may also name an
/// [`Extra`]; it shows only when `gates` allows it.
pub fn dock(offers: &[Offer], shortcuts: Option<usize>, buttons: &Buttons, gates: &Gates) -> Result<Dock, UnknownTile> {
    let (extras, tiles): (Vec<Offer>, Vec<Offer>) = offers.iter().cloned().partition(|offer| extra(&offer.id).is_some());
    let menu = stream_menu::layout_with_buttons(&tiles, shortcuts, buttons)?;
    let mut pages: Vec<Page> = Module::ALL.into_iter().map(|module| Page { module, cards: Vec::new(), rows: Vec::new() }).collect();
    let mut footer = Vec::with_capacity(menu.footer.len());
    let mut late: Vec<Cell> = Vec::new();
    for cell in menu.grid {
        match cell {
            Cell::Tile { id, .. } if id == stream_menu::MORE => {}
            Cell::Tile { id, .. } => {
                let Some(module) = module_of(id) else { return Err(UnknownTile(id.to_owned())) };
                push_card(&mut pages, module, cell);
            }
            // The user's shortcuts and Add close the Quick page, after its own cards.
            Cell::Shortcut(_) | Cell::Add => late.push(cell),
        }
    }
    for gated in EXTRAS {
        let Some(offer) = extras.iter().find(|offer| offer.id == gated.id) else { continue };
        if gated.allowed(gates) {
            push_card(&mut pages, gated.module, Cell::Tile { id: gated.id, on: offer.on, hotkey: None });
        }
    }
    for cell in late {
        push_card(&mut pages, Module::Quick, cell);
    }
    for id in menu.more {
        let Some(module) = module_of(id) else { return Err(UnknownTile(id.to_owned())) };
        if let Some(page) = pages.iter_mut().find(|page| page.module == module) {
            page.rows.push(id);
        }
    }
    for id in menu.footer {
        match module_of(id) {
            // Ending the PC's session is no everyday way out: it sits in Power.
            Some(module) => {
                let on = tiles.iter().find(|offer| offer.id == id).and_then(|offer| offer.on);
                push_card(&mut pages, module, Cell::Tile { id, on, hotkey: None });
            }
            None => footer.push(id),
        }
    }
    pages.retain(|page| page.module == Module::Quick || !page.is_empty());
    Ok(Dock { pages, footer, buttons: menu.buttons })
}

fn push_card(pages: &mut [Page], module: Module, cell: Cell) {
    if let Some(page) = pages.iter_mut().find(|page| page.module == module) {
        page.cards.push(cell);
    }
}

/// Where the Control Center sits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Placement {
    /// A sheet from the bottom edge with the rail across its top: a phone held upright.
    Sheet,
    /// A dock at the right edge with the rail down its left side: everything wide.
    Dock,
}

/// Where the Control Center sits on this screen. Only a phone held upright is too narrow for a
/// side dock; a phone on its side, a handheld, a TV and a desktop window all get the dock, so the
/// rail is where the thumb or the D-pad expects it on each.
pub fn placement(form: FormFactor, width_dp: f32, height_dp: f32) -> Placement {
    match form {
        FormFactor::Phone if width_dp < DOCK_MIN_WINDOW_DP && height_dp > width_dp => Placement::Sheet,
        // A desktop window dragged narrower than a dock and a margin of the game.
        FormFactor::Desktop if width_dp < DOCK_MIN_WINDOW_DP && height_dp > width_dp => Placement::Sheet,
        _ => Placement::Dock,
    }
}

/// The narrowest upright desktop window that still gets the side dock.
pub const DOCK_MIN_WINDOW_DP: f32 = 560.0;

/// What the open Control Center is drawn on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Surface {
    /// The game blurred behind it: one blur window on Android, one blur pass on the desktop.
    Blurred,
    /// An opaque tinted panel: no blur anywhere.
    Baked,
    /// Black with a rim: high contrast.
    Solid,
}

/// What the device allows this moment. All of it can change while the Control Center is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Live {
    /// The tier the interface landed on ([`crate::glass_probe`]).
    pub tier: Tier,
    /// The system composes blur between windows now (Android's `isCrossWindowBlurEnabled`; on
    /// the desktop, the renderer can sample the video).
    pub blur_available: bool,
    pub power_save: bool,
    pub thermal_severe: bool,
    pub high_contrast: bool,
}

/// What the Control Center is drawn on: `None` while it is closed, which costs nothing. Open, it
/// blurs only where the interface's own tier does and the system allows it right now; battery
/// saver, heat or blur switched off drop it to the baked panel at once. That drop is for this
/// moment only and never changes the stored tier.
pub fn surface(open: bool, live: &Live) -> Option<Surface> {
    if !open {
        return None;
    }
    Some(if live.high_contrast || live.tier == Tier::Solid {
        Surface::Solid
    } else if matches!(live.tier, Tier::Lens | Tier::Live) && live.blur_available && !live.power_save && !live.thermal_severe {
        Surface::Blurred
    } else {
        Surface::Baked
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stream_menu::{CLOSE, Place, TILES};

    fn everything() -> Vec<Offer> {
        TILES.iter().map(|tile| Offer::action(tile.id)).collect()
    }

    fn ids(page: &Page) -> Vec<&'static str> {
        page.cards.iter().filter_map(|cell| if let Cell::Tile { id, .. } = cell { Some(*id) } else { None }).collect()
    }

    #[test]
    fn every_tile_of_the_menu_has_one_place() {
        for tile in TILES {
            let module = module_of(tile.id);
            assert_eq!(module.is_none(), FOOTER.contains(&tile.id), "{}", tile.id);
        }
        let dock = dock(&everything(), Some(2), &Buttons::new(), &Gates::default()).unwrap();
        for tile in TILES {
            let cards = dock.pages.iter().filter(|page| ids(page).contains(&tile.id)).count();
            let rows = dock.pages.iter().filter(|page| page.rows.contains(&tile.id)).count();
            let footer = dock.footer.iter().filter(|id| **id == tile.id).count();
            assert_eq!(cards + rows + footer, 1, "{}", tile.id);
            // What the menu showed on its first page stays a card; its More rows stay rows.
            match tile.place {
                Place::Grid => assert_eq!(cards, 1, "{}", tile.id),
                Place::More => assert_eq!(rows, 1, "{}", tile.id),
                Place::Footer => assert_eq!(rows, 0, "{}", tile.id),
            }
        }
        assert_eq!(dock.footer.first(), Some(&CLOSE));
        assert_eq!(dock.footer, [CLOSE, "disconnect", "close_game", "quit_game"]);
    }

    #[test]
    fn modules_follow_the_rail_and_empty_ones_do_not_show() {
        let full = dock(&everything(), None, &Buttons::new(), &Gates::default()).unwrap();
        let modules: Vec<Module> = full.pages.iter().map(|page| page.module).collect();
        assert_eq!(modules, [Module::Quick, Module::Picture, Module::Audio, Module::Pc, Module::Controller, Module::Guests, Module::Power]);
        assert!(full.page(Module::Clips).is_none(), "nothing saves clips yet");
        let bare = dock(&[Offer::action("disconnect")], None, &Buttons::new(), &Gates::default()).unwrap();
        assert_eq!(bare.pages.len(), 1, "Quick is always there");
        assert!(bare.pages[0].is_empty());
        assert_eq!(bare.footer, [CLOSE, "disconnect"]);
    }

    #[test]
    fn the_quick_page_ends_with_the_shortcuts() {
        let dock = dock(&everything(), Some(2), &Buttons::new(), &Gates::default()).unwrap();
        let quick = dock.page(Module::Quick).unwrap();
        assert_eq!(ids(quick)[..5], ["keyboard", "pc_keyboard", "controller_mouse", "overlay", "shortcuts"]);
        assert_eq!(quick.cards[quick.cards.len() - 3..], [Cell::Shortcut(0), Cell::Shortcut(1), Cell::Add]);
        assert!(quick.rows.contains(&"help"));
        let none = super::dock(&everything(), None, &Buttons::new(), &Gates::default()).unwrap();
        assert!(!none.page(Module::Quick).unwrap().cards.iter().any(|cell| matches!(cell, Cell::Shortcut(_) | Cell::Add)));
    }

    #[test]
    fn the_pcs_sound_switches_are_on_audio() {
        for tile in ["night_mode", "headphone_surround", "microphone"] {
            assert_eq!(module_of(tile), Some(Module::Audio), "{tile}");
        }
    }

    #[test]
    fn the_buttons_stay_the_users() {
        let mut buttons = Buttons::new();
        buttons.insert(Hotkey::Y, "microphone".into());
        let dock = dock(&everything(), None, &buttons, &Gates::default()).unwrap();
        assert_eq!(dock.buttons.get(&Hotkey::Y), Some(&"microphone"));
        assert_eq!(dock.hotkey(Hotkey::Y), Some((Module::Audio, 0)));
        assert_eq!(dock.hotkey(Hotkey::X).map(|(module, _)| module), Some(Module::Quick));
        let audio = dock.page(Module::Audio).unwrap();
        assert_eq!(audio.cards[0], Cell::Tile { id: "microphone", on: None, hotkey: Some(Hotkey::Y) });
    }

    #[test]
    fn a_switch_keeps_its_state() {
        let offers = [Offer::switch("microphone", true), Offer::switch("overlay", false), Offer::switch("end_session", false)];
        let dock = dock(&offers, None, &Buttons::new(), &Gates::default()).unwrap();
        assert_eq!(dock.page(Module::Audio).unwrap().cards, [Cell::Tile { id: "microphone", on: Some(true), hotkey: None }]);
        assert_eq!(dock.page(Module::Power).unwrap().cards, [Cell::Tile { id: "end_session", on: Some(false), hotkey: None }]);
        assert_eq!(dock.footer, [CLOSE]);
    }

    #[test]
    fn a_gated_card_needs_the_pcs_feature() {
        let offers = [Offer::action("keyboard"), Offer::action("save_clip"), Offer::action("quick_suspend")];
        let closed = dock(&offers, None, &Buttons::new(), &Gates::default()).unwrap();
        assert!(closed.page(Module::Clips).is_none() && closed.page(Module::Power).is_none());
        let open = dock(&offers, None, &Buttons::new(), &Gates { stream_dvr: true, ..Gates::default() }).unwrap();
        assert_eq!(ids(open.page(Module::Clips).unwrap()), ["save_clip"]);
        assert!(open.page(Module::Power).is_none(), "one feature opens only its own cards");
        // A gate alone shows nothing: the client has to offer the card too.
        let unoffered = dock(&[Offer::action("keyboard")], None, &Buttons::new(), &Gates { stream_dvr: true, ..Gates::default() }).unwrap();
        assert!(unoffered.page(Module::Clips).is_none());
        for gated in EXTRAS {
            assert!(!gated.allowed(&Gates::default()), "{}", gated.id);
            assert!(stream_menu::tile(gated.id).is_none(), "{} is also a tile", gated.id);
        }
    }

    #[test]
    fn an_unknown_offer_is_an_error() {
        assert_eq!(dock(&[Offer::action("warp_drive")], None, &Buttons::new(), &Gates::default()), Err(UnknownTile("warp_drive".into())));
    }

    #[test]
    fn the_rail_stops_at_its_ends() {
        let dock = dock(&everything(), None, &Buttons::new(), &Gates::default()).unwrap();
        assert_eq!(dock.step(Module::Quick, true), Module::Picture);
        assert_eq!(dock.step(Module::Quick, false), Module::Quick);
        assert_eq!(dock.step(Module::Audio, true), Module::Pc, "Clips is skipped while it has nothing");
        assert_eq!(dock.step(Module::Power, true), Module::Power);
        assert_eq!(dock.step(Module::Clips, true), Module::Quick, "a module that is not shown starts over");
    }

    #[test]
    fn module_names_are_stable_and_short() {
        for module in Module::ALL {
            assert_eq!(Module::parse(module.key()), Some(module));
            assert!(module.label().en.chars().count() <= 10, "{module:?}");
        }
        assert_eq!(Module::parse("more"), None);
    }

    #[test]
    fn only_an_upright_phone_gets_the_sheet() {
        assert_eq!(placement(FormFactor::Phone, 393.0, 852.0), Placement::Sheet);
        assert_eq!(placement(FormFactor::Phone, 322.0, 792.0), Placement::Sheet);
        assert_eq!(placement(FormFactor::Phone, 673.0, 841.0), Placement::Dock);
        assert_eq!(placement(FormFactor::Phone, 841.0, 673.0), Placement::Dock);
        assert_eq!(placement(FormFactor::Phone, 852.0, 393.0), Placement::Dock);
        assert_eq!(placement(FormFactor::Handheld, 1280.0, 720.0), Placement::Dock);
        assert_eq!(placement(FormFactor::Tv, 960.0, 540.0), Placement::Dock);
        assert_eq!(placement(FormFactor::Desktop, 1280.0, 800.0), Placement::Dock);
        assert_eq!(placement(FormFactor::Desktop, 480.0, 900.0), Placement::Sheet);
    }

    #[test]
    fn closed_costs_nothing_and_open_blurs_only_when_allowed() {
        let live = Live { tier: Tier::Live, blur_available: true, power_save: false, thermal_severe: false, high_contrast: false };
        assert_eq!(surface(false, &live), None);
        assert_eq!(surface(true, &live), Some(Surface::Blurred));
        assert_eq!(surface(true, &Live { tier: Tier::Lens, ..live }), Some(Surface::Blurred));
        for dropped in [
            Live { blur_available: false, ..live },
            Live { power_save: true, ..live },
            Live { thermal_severe: true, ..live },
            Live { tier: Tier::Baked, ..live },
        ] {
            assert_eq!(surface(true, &dropped), Some(Surface::Baked), "{dropped:?}");
            assert_eq!(surface(false, &dropped), None);
        }
        assert_eq!(surface(true, &Live { high_contrast: true, ..live }), Some(Surface::Solid));
        assert_eq!(surface(true, &Live { tier: Tier::Solid, ..live }), Some(Surface::Solid));
    }
}
