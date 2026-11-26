//! The launcher: PCs, pairing, apps, settings, and launch card. Collects [`Command`]s for `App::command`.

mod about;
mod backdrop;
mod cards;
mod chrome;
#[allow(dead_code, reason = "generated: every token is listed, the pages use them as they are redesigned")]
pub(crate) mod design_tokens;
#[cfg(any(feature = "demo-library", test))]
pub mod demo;
mod dialogs;
pub mod files;
pub mod glass;
mod first_run;
mod game;
mod help;
mod home;
mod host_details_limiter;
mod icons;
mod launch_scene;
mod latency_bench;
mod library;
mod navigation;
mod pages;
pub(crate) mod pcs;
mod settings;
mod spring;
mod types;
mod widgets;

use egui::{
    pos2, vec2, Align, Align2, Color32, CornerRadius, Frame, Id, Key, Layout, Margin, Pos2, Rect, Response,
    RichText, Sense, Stroke, StrokeKind, TextureHandle, Ui, UiBuilder, Vec2,
};

use crate::config::{
    step_zoom, AudioChannels, CardSize, CodecPreference, DecoderPreference, ErrorCorrection, HomeEnumLabel, HomeGroup, HomeLayout, HomeOrder, HostDisplay, Language,
    MouseMode, Overrides, Profiles, ReduceMotion, Settings, WindowMode, UI_ZOOM_STEPS,
};
use crate::console::{self, EndAction, GameState, Playing, Source};
use crate::hosts::{Apps, Art, Handed, Host, Hosts, PcPowerStep, Reach, SleepStep, UpdateStep};
use nightjar_policy::http::PcPower;
use crate::strings::{en, tr, trf};
use crate::stream_types::{pyrowave_bitrate_warning, stream_mode, Display};
use crate::theme::*;
use cards::*;
use dialogs::*;
pub use about::SOURCE as ABOUT_SOURCE;
pub use library::{asked_start, at_sentence_start, join_target, playing_elsewhere, policy_role, running_name};
use library::*;
pub(crate) use widgets::{column, focus_ring, focus_ring_around, glass_behind, group_row, lift_shapes, name_for_readers, page_scroll_from, pin_key, pin_slots, primary_button, quiet_button, round_button, row, section, toggle, ToTop, DIALOG_MARGIN};
pub use widgets::{Combo, Tone};
pub(crate) use chrome::{BACK_SIZE, PAGE_TITLE};
use widgets::*;
pub use launch_scene::settings_line;
pub use settings::{pick_display_choice, DISPLAY_CHOICES};
pub(crate) use icons::Icon;
use navigation::{destination_icon, page_area};
pub use types::{Command, ConnectingView, Dialog, Screen, View, DESTINATIONS, GUIDES};

/// A PC in `Dialog::KeptSettings`: its name, whether it has settings of its own, and for how
/// many games. Put into words as it is drawn, in the language chosen by then.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeptPc {
    pub name: String,
    pub own: bool,
    pub games: usize,
}

impl KeptPc {
    pub fn line(&self) -> String {
        let mut kept = Vec::new();
        if self.own { kept.push(tr("PC settings").to_string()); }
        match self.games {
            0 => {}
            1 => kept.push(tr("1 game").to_string()),
            n => kept.push(trf("{} games", &[&n])),
        }
        format!("{}: {}", self.name, kept.join(", "))
    }
}

pub struct Launcher {
    pub screen: Screen,
    pub dialog: Dialog,
    pub settings: Settings,
    pub profiles: Profiles,
    pub network: crate::network_types::Network,
    pub ssid_supported: bool,
    home_wifi_name: String,
    /// Every PC's library merged, kept while the PCs and the hidden games are what it was made from.
    pub(crate) library: console::LibraryMemo,
    pub edit_settings: Settings,
    pub inherited_settings: Settings,
    pub edit_overrides: Overrides,
    /// The layers under the one being edited that can give an unchecked row its value, from the
    /// top, with what the row then says: the PC and the profile in use for a game, the profile
    /// for a PC, none for a profile.
    pub inherited_from: Vec<(Overrides, String)>,
    pub editing_app: Option<nightjar_policy::http::App>,
    pub edit_name: String,
    pub settings_dirty: bool,
    /// The settings page's open section, search and pending Reset all.
    settings_view: settings::SettingsView,
    /// The settings page where Custom… was chosen for the resolution and for the frame rate: its
    /// row shows there even at a preset, until a preset is chosen.
    custom_size: Option<Screen>,
    custom_fps: Option<Screen>,
    pub controller_key: String,
    pub controller_mapping: crate::gamepad_types::Mapping,
    pub controller_test: bool,
    /// The device in the hand: what the button hints name.
    pub input_source: crate::input_source::InputSource,
    pub shortcuts: crate::shortcuts::Shortcuts,
    /// Games streamed before, latest first.
    pub recents: Vec<crate::config::Recent>,
    /// Starred games, newest star first.
    pub favourites: Vec<crate::config::Favourite>,
    /// The game chosen for "When Nightjar opens".
    pub open_game: Option<crate::config::OpenGame>,
    /// Whether Nightjar starts at sign-in; None hides the row (no way to set it here).
    pub sign_in: Option<bool>,
    /// The games hidden on this device (hidden-games.json).
    pub hidden_games: Vec<crate::config::Favourite>,
    /// A PC's library page lists its hidden games too.
    show_hidden_apps: bool,
    logo: Option<TextureHandle>,
    focus_screen: bool,
    focus_dialog: bool,
    /// This frame's Escape came from a controller's B, which alone asks to close Nightjar where Back ends.
    pad_back: bool,
    /// The last Sleep PC token request's number.
    sleep_asks: u64,
    /// Messages that came while the user was busy with another dialog, shown once it closes.
    queued: std::collections::VecDeque<Dialog>,
    settings_return: Screen,
    /// Where Back goes: the screens left by moving forward, with the item that had focus.
    trail: Vec<(Screen, Option<String>)>,
    /// The item with focus on the current screen, by its key.
    focused_key: Option<String>,
    /// Where the open menu (PC options, a game's menu) is anchored.
    menu_anchor: Anchor,
    /// The All games page's filter.
    library_filter: console::LibraryFilter,
    /// All games' genre and length filters, from IGDB's facts; None shows every game.
    genre_filter: Option<String>,
    length_filter: Option<nightjar_policy::game_info::library::Length>,
    /// The SteamGridDB and IGDB keys as Settings → Advanced edits them; taken in when a field is left.
    pub game_info_keys: nightjar_policy::game_info::job::Keys,
    /// The item to focus when the screen is drawn again after Back.
    restore_key: Option<String>,
    /// The cards a menu's Hide can take off this frame's screen, in the order drawn, with their game.
    cards: Vec<(String, String)>,
    /// Some PC has games to show, so Home has something on it: a native library, or the app list
    /// of a PC without one.
    home_ready: bool,
    /// No PC is known yet: Home is the PCs' first run (`Launcher::ui`).
    no_pcs: bool,
    /// Some PC has a native library: Home is where Back ends. App lists alone leave that to the
    /// list of PCs, as on Android.
    home_root: bool,
    /// The width the header's actions took last frame, to keep them at the right edge.
    header_width: f32,
    /// The game Home is about; the chrome's PC capsule speaks of its PC.
    hero: Option<home::Hero>,
    /// What lies behind the page.
    ambient: backdrop::Ambient,
    /// The top bar's search field is open on All games, with these words.
    search_open: bool,
    search_focus: bool,
    /// All games was opened for the search, so leaving the search leaves it too.
    search_pushed: bool,
    library_query: String,
    /// How far under the chrome this page's content scrolls: an item revealed rests below it.
    scroll_inset: f32,
    /// How far Home has scrolled, for the scrim under the chrome.
    page_scrolled: f32,
    /// The stream menu's demo page, over its still.
    #[cfg(any(test, feature = "demo-library"))]
    pub demo_menu: Option<demo::DemoMenu>,
    #[cfg(any(test, feature = "demo-library"))]
    demo_video: Option<egui::TextureHandle>,
    /// The focus went up into the chrome: Home goes back to its start beneath it.
    page_to_top: bool,
    /// Back was used on the page header this frame.
    back_pressed: bool,
    /// The PC whose empty list is being read again: its page holds still until the answer.
    refreshing_empty: Option<u64>,
    /// The first run shows a found PC: when it first does, its Pair takes the focus.
    found_shown: bool,
    /// Play pressed before the PC's app list arrived: it starts when the list lands.
    pending_play: Option<library::PendingPlay>,
    /// Where the page's Back has its slot this frame: None on a page without one.
    back_slot: Option<f32>,
    /// The display's tearing support, for the VRR setting's note.
    pub tearing_supported: bool,
    /// A display of this computer shows HDR, read once when SDL starts (`sdl::any_display_hdr`).
    /// The simple layer's HDR Auto turns HDR on only when this is true; a test's launcher has none.
    pub hdr_display: bool,
    /// Re-reads `hdr_display` when the settings page opens (SDL is live; a test leaves this None).
    pub hdr_display_probe: Option<fn() -> bool>,
    /// `hdr_display` was read again for Home's setup card, once a run.
    setup_card_probed: bool,
    /// Where focus was when Up or Down was last pressed with the setup card on Home: a move from
    /// there across the card stops on its close button.
    setup_card_from: Option<Rect>,
    /// The renderer's Direct3D 11 device, which the PyroWave GPU decoder's settings probe runs on.
    pub gpu_device: Option<usize>,
    pub display: Display,
    /// The display's fullscreen modes in SDL's order, for fractional V-Sync's note (Windows' Fullscreen takes one); empty when unknown.
    pub fullscreen_modes: Vec<settings::FullscreenMode>,
    pub identity_ready: bool,
    /// Why this device's pairing key could not be read, when it could not: the first-run page says
    /// it instead of waiting for a key that will not come.
    pub identity_error: Option<String>,
    /// What this PC's power allows (`power::probe`): the Close Nightjar question offers these.
    /// Empty until known.
    pub power: Vec<crate::power_types::Offer>,
    /// The system's preferred languages, for the Language setting's default.
    pub system_languages: Vec<String>,
    /// Non-Latin-1 scripts required by shown PC names and game titles, cached by host/library signature.
    title_scripts: (u64, crate::theme::ScriptSet),
    /// The look and motion egui's style was last built for.
    look: Option<(crate::theme::Look, bool)>,
    /// Whether the system asks for light apps (`sdl::system_light`), for the System theme: the main
    /// loop keeps it current.
    pub system_light: bool,
    /// SDL can show an on-screen keyboard here (`SDL_HasScreenKeyboardSupport`), for Keyboard follows
    /// text fields' note; the main loop sets it.
    pub screen_keyboard: bool,
    /// SDL runs on a Wayland desktop, for HDR brightness follows the desktop's note.
    pub wayland: bool,
    /// The details page's remote access check.
    pub remote: crate::host_page::RemoteAccess,
    /// The details page's shared folder browser.
    pub folders: crate::folders_page::SharedFolders,
    /// The details page's list of Nucleus Co-op sessions.
    pub coop: crate::coop_page::CoopSessions,
    /// The game page's Neural Rendering.
    pub neural: crate::neural_page::NeuralPanel,
    /// The Files page and its transfers.
    pub files: files::Files,
    /// The latency benchmark page's test and results (`crate::latency_bench`); app/launcher.rs feeds it.
    pub latency_bench: crate::latency_bench_state::Bench,
    /// A short notice over the launcher, such as the game quitting after leaving its stream.
    toast: Option<(String, std::time::Instant)>,
    /// The PIN lock; None in tests that do not look at it.
    pub pin: Option<crate::pin_ui::PinLock>,
}

const CONTENT_MAX: f32 = 1760.0;
/// Seconds a pairing link's question shows before Pair can be chosen.
const PAIR_LINK_DELAY: f64 = 1.0;
/// The column of settings rows takes what the page gives it up to this, so that on a very wide
/// window a row's label and its control stay in one glance. No window under about 1400 reaches it.
pub(crate) const SETTINGS_W: f32 = 1300.0;

/// The monitor's height in egui points, with the OS scale divided out (`gui::Gui` fills
/// `monitor_size` from the window's display). Where the platform reports none, the window's own
/// screen rect stands in; with neither, Auto stays at 100 %.
fn auto_zoom_now(ctx: &egui::Context) -> u32 {
    let (height, os_scale) = ctx.input(|i| {
        let viewport = i.viewport();
        let height = viewport.monitor_size.or_else(|| viewport.outer_rect.map(|rect| rect.size())).map(|size| size.y);
        (height, viewport.native_pixels_per_point.unwrap_or(1.0))
    });
    crate::config::auto_zoom(height, os_scale)
}

impl Launcher {
    pub fn new(settings: Settings, profiles: Profiles, shortcuts: crate::shortcuts::Shortcuts, display: Display) -> Self {
        let more_settings = settings.settings_more_open;
        Self {
            screen: Screen::Hosts,
            dialog: Dialog::None,
            settings,
            profiles,
            network: crate::network_types::Network::default(),
            ssid_supported: crate::network_types::SSID_SUPPORTED,
            home_wifi_name: String::new(),
            library: console::LibraryMemo::default(),
            edit_settings: Settings::default(),
            inherited_settings: Settings::default(),
            edit_overrides: Overrides::default(),
            inherited_from: Vec::new(),
            editing_app: None,
            edit_name: String::new(),
            settings_dirty: false,
            settings_view: settings::SettingsView::remembered(more_settings),
            custom_size: None,
            custom_fps: None,
            controller_key: String::new(),
            controller_mapping: crate::gamepad_types::Mapping::default(),
            controller_test: false,
            input_source: crate::input_source::InputSource::None,
            shortcuts,
            recents: Vec::new(),
            favourites: Vec::new(),
            open_game: None,
            sign_in: None,
            hidden_games: Vec::new(),
            show_hidden_apps: false,
            logo: None,
            focus_screen: true,
            focus_dialog: false,
            pad_back: false,
            sleep_asks: 0,
            queued: Default::default(),
            settings_return: Screen::Hosts,
            trail: Vec::new(),
            focused_key: None,
            menu_anchor: Anchor::Center,
            library_filter: console::LibraryFilter::All,
            genre_filter: None,
            length_filter: None,
            game_info_keys: Default::default(),
            restore_key: None,
            cards: Vec::new(),
            home_ready: false,
            no_pcs: false,
            home_root: false,
            header_width: 0.0,
            hero: None,
            ambient: Default::default(),
            search_open: false,
            search_focus: false,
            search_pushed: false,
            library_query: String::new(),
            scroll_inset: 0.0,
            page_scrolled: 0.0,
            #[cfg(any(test, feature = "demo-library"))]
            demo_menu: None,
            #[cfg(any(test, feature = "demo-library"))]
            demo_video: None,
            page_to_top: false,
            back_pressed: false,
            refreshing_empty: None,
            found_shown: false,
            pending_play: None,
            back_slot: None,
            tearing_supported: false,
            hdr_display: false,
            hdr_display_probe: None,
            setup_card_probed: false,
            setup_card_from: None,
            gpu_device: None,
            display,
            fullscreen_modes: Vec::new(),
            identity_ready: false,
            identity_error: None,
            power: Vec::new(),
            system_languages: Vec::new(),
            title_scripts: (0, crate::theme::ScriptSet::default()),
            look: None,
            system_light: false,
            screen_keyboard: false,
            wayland: false,
            remote: Default::default(),
            folders: Default::default(),
            coop: Default::default(),
            neural: Default::default(),
            files: Default::default(),
            latency_bench: Default::default(),
            toast: None,
            pin: None,
        }
    }

    pub fn ui(&mut self, ui: &mut Ui, view: View<'_>) -> Vec<Command> {
        let mut commands = Vec::new();
        let ctx = ui.ctx().clone();
        if self.zoom_keys(&ctx) {
            commands.push(Command::SaveSettings);
        }
        self.sync_look(&ctx, view.hosts);
        #[cfg(any(test, feature = "demo-library"))]
        if let Some(menu) = self.demo_menu {
            self.demo_stream_menu(ui, menu);
            return commands;
        }
        // A check whose page was left is cancelled here. Settings opened over the page does not
        // leave it: Back returns to the page and its check.
        // The newest page is the one Back returns to: the trail ends with the latest.
        let page = std::iter::once(&self.screen).chain(self.trail.iter().rev().map(|(screen, _)| screen)).find_map(|screen| match screen {
            Screen::HostDetails(id) => Some(*id),
            _ => None,
        });
        self.remote.poll(page);
        self.folders.poll(page);
        // A co-op join that went through streams its slot: the session's entry, launched as any.
        if let Some((host, app)) = self.coop.poll(page) {
            commands.push(Command::Launch { host, app, resume: false });
        }
        // Likewise a game page that was left closes its Neural Rendering; Settings opened over it
        // (Start or the Settings link) keeps the panel as it was. The newest game page is the one
        // Back returns to: the trail ends with the latest.
        let game = std::iter::once(&self.screen).chain(self.trail.iter().rev().map(|(screen, _)| screen)).find_map(|screen| match screen {
            Screen::Game { host, app } => Some((*host, *app)),
            _ => None,
        });
        self.neural.poll(game);
        // Likewise a Files page that was left goes, once its transfers are done and a failure among
        // them has been shown.
        let files = std::iter::once(&self.screen).chain(self.trail.iter().rev().map(|(screen, _)| screen)).find_map(|screen| match screen {
            Screen::Files(id) => Some(*id),
            _ => None,
        });
        self.files.poll(files);
        // The PIN screen and the PIN lock page cover the launcher while they are up. The polls
        // above run under them, so a check, transfer or join in flight still ends or is cancelled.
        if let Some(pin) = &mut self.pin {
            let pcs: Vec<(String, String)> = view.hosts.list.iter().map(|h| (h.record.unique_id.clone(), h.display_name().to_owned())).collect();
            // Its sky goes under it as the launcher's does, painted once it is drawn.
            let slot = pin.up().then(|| {
                self.ambient.set_page(&ctx, "pin");
                self.ambient.prepare(&ctx, backdrop::Want::Sky(backdrop::Moon::High));
                let under = ui.painter().clone();
                let slot = under.add(egui::Shape::Noop);
                (under, slot)
            });
            if pin.ui(ui, &pcs, &mut commands) {
                if let Some((under, slot)) = slot {
                    under.set(slot, egui::Shape::Vec(self.ambient.finish(&ctx)));
                }
                return commands;
            }
        }
        let sources: Vec<Source> = view.hosts.list.iter().map(Source::of).collect();
        self.home_ready = console::has_library(&sources);
        self.no_pcs = view.hosts.list.is_empty();
        self.home_root = console::has_native_library(&sources);
        // With a menu open, Escape (B) only closes it: the menu closes itself as it is drawn.
        let menu_open = egui::Popup::is_any_open(&ctx);
        let escape = ctx.input(|i| i.key_pressed(Key::Escape)) && !menu_open;
        let pad = std::mem::take(&mut self.pad_back);
        // From Play until the stream takes the window, or a failed start's card: the launch scene
        // covers the launcher whole.
        if self.scene_up(&view) {
            self.launch_scene(ui, &view, escape, &mut commands);
            self.button_hints(&ctx, &view);
            self.draw_toast(&ctx);
            return commands;
        }
        // On a settings page and on All games, Escape leaves the search before the page.
        if escape && view.connecting.is_none() && view.waking.is_none() && !(matches!(self.dialog, Dialog::None) && (self.escape_search() || self.escape_library_search())) {
            self.back(&mut commands, pad);
        } else if view.connecting.is_none() && view.waking.is_none() {
            self.help_keys(&ctx, &mut commands);
        }
        if self.screen == Screen::Hosts
            && matches!(self.dialog, Dialog::None)
            && view.connecting.is_none()
            && view.waking.is_none()
            && ctx.input(|i| i.modifiers.command && i.key_pressed(Key::N))
        {
            self.add_host_dialog();
        }
        let logo = self.logo(&ctx);
        let full = ui.max_rect();
        // Home's hero is known before anything is drawn: the backdrop and the chrome speak of it.
        let mut home_data = (self.screen == Screen::Home).then(|| home::HomeData::gather(self, &sources));
        if let Some(data) = &home_data {
            self.hero.clone_from(&data.hero);
        }
        // Home has nothing to say with no PCs: the first run is on PCs, where they are found.
        if self.screen == Screen::Home && view.hosts.list.is_empty() {
            self.go(Screen::Hosts);
        }
        let want = self.page_backdrop(&view, &sources);
        // The backdrop goes under the page but is painted once the page is drawn: its moon stands
        // clear of what this page holds, in the same frame.
        self.ambient.set_page(&ctx, self.screen);
        self.ambient.set_orbit(self.screen == Screen::Hosts && first_run::first_run_of(view.hosts).is_some() && view.hosts.discovering);
        self.ambient.prepare(&ctx, want);
        let under = ui.painter().clone();
        let backdrop_slot = under.add(egui::Shape::Noop);
        let margin = chrome::page_margin(full.width());
        let content = Rect::from_min_max(
            Pos2::new(full.left() + margin, full.top() + chrome::PAGE_TOP),
            Pos2::new(full.right() - margin, full.bottom() - 16.0),
        );
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(content).layout(Layout::top_down(Align::Min)));
        let ui = &mut child;
        self.cards.clear();
        // A menu by its card leaves the page as it is (it holds the input itself); a dialog dims it.
        let menu = matches!(self.dialog, Dialog::HostOptions { .. } | Dialog::PcSheet { .. } | Dialog::GameMenu { .. } | Dialog::HomeView) && self.menu_anchor != Anchor::Center;
        let blocked = (!matches!(self.dialog, Dialog::None) && !menu) || view.connecting.is_some() || view.waking.is_some();
        // Screens take focus only when nothing covers them.
        let held_focus = self.focus_screen;
        if blocked {
            self.focus_screen = false;
        }
        self.settle_play(&view, &sources, &mut commands);
        let brand = self.top_bar(&ctx, &view, blocked, &mut commands);
        self.back_slot = None;
        // Every page scrolls whole beneath the chrome, so whatever is revealed rests below it.
        self.scroll_inset = chrome::PAGE_TOP;
        ui.add_enabled_ui(!blocked, |ui| {
            match self.screen {
                Screen::Home => {
                    let data = home_data.take().unwrap_or_else(|| home::HomeData::gather(self, &sources));
                    self.home(ui, &view, &sources, data, &mut commands)
                }
                Screen::Settings | Screen::HostSettings(_) | Screen::AppSettings(_) | Screen::ProfileSettings(_) => self.settings_page(ui, &view, &mut commands),
                screen => {
                    let to_top = self.chrome_to_top();
                    self.page_scrolled = page_scroll(ui, page_area(screen), to_top, |ui| {
                        self.header(ui, &view, &mut commands);
                        match screen {
                            Screen::AllGames => self.all_games(ui, &view, &mut commands),
                            Screen::Game { host, app } => self.game(ui, &view, host, app, &mut commands),
                            Screen::Hosts => self.hosts(ui, &view, &mut commands),
                            Screen::Apps(id) => self.apps(ui, &view, id, &mut commands),
                            Screen::HostDetails(id) => self.host_details(ui, &view, id, &mut commands),
                            Screen::Files(id) => self.files_page(ui, &view, id, &mut commands),
                            Screen::Profiles => self.profiles_page(ui, &mut commands),
                            Screen::Controllers => self.controllers_page(ui, view.pads, &mut commands),
                            Screen::Shortcuts => self.shortcuts_page(ui, &mut commands),
                            Screen::StreamHotkeys => self.stream_hotkeys_page(ui, &mut commands),
                            Screen::Help => self.help_page(ui, &view, &mut commands),
                            Screen::About => self.about_page(ui, &mut commands),
                            Screen::LatencyBenchmark => self.latency_benchmark_page(ui, &view, &mut commands),
                            Screen::Home | Screen::Settings | Screen::HostSettings(_) | Screen::AppSettings(_) | Screen::ProfileSettings(_) => {}
                        }
                    });
                }
            }
            // Back, once the page it leaves is drawn: the next one starts in its own scroll area.
            if std::mem::take(&mut self.back_pressed) {
                self.back(&mut commands, false);
            }
            self.paint_brand(&ctx, brand, logo);
            // What rose over its neighbours, under the chrome and whatever covers the page.
            glass::flush_deferred(ui);
            under.set(backdrop_slot, egui::Shape::Vec(self.ambient.finish(&ctx)));
            let veil = self.ambient.veil(&ctx, full, chrome::brand_rect(full), self.page_scrolled);
            ui.painter().with_clip_rect(full).extend(veil);
        });
        if blocked {
            self.focus_screen = held_focus;
        } else if self.restore_key.take().is_some() {
            // The remembered item is gone (or not loaded yet): the first one takes focus.
            ctx.request_repaint();
        }
        // A or a screen reader's click in an open menu closes it as a mouse click does (egui only
        // closes on the pointer), so the next B leaves the page rather than closing a used menu.
        let activated = ctx.input(|i| i.key_pressed(Key::Enter)
            || i.events.iter().any(|e| matches!(e, egui::Event::AccessKitActionRequest(r) if r.action == egui::accesskit::Action::Click)));
        if menu_open && activated && egui::Popup::is_any_open(&ctx) {
            egui::Popup::close_all(&ctx);
        }
        self.dialogs(&ctx, &view, &mut commands);
        if let Some(host) = &view.waking {
            waking(&ctx, host, &mut commands);
        }
        self.button_hints(&ctx, &view);
        // A Files page left while it transfers: its transfers go on, and this opens it again. One
        // that failed meanwhile keeps it until the page shows the failure.
        let files_shown = match self.screen { Screen::Files(id) => Some(id), _ => None };
        if let Some((host, away)) = self.files.running_away(files_shown)
            && files::running_button(&ctx, away) {
                if let Some(save) = self.screen.save_command() {
                    commands.push(save);
                }
                commands.push(Command::OpenFiles(host));
            }
        self.draw_toast(&ctx);
        commands
    }

    /// A dialog or a connecting or waking card is over the page.
    fn panel_is_up(&self, view: &View<'_>) -> bool {
        !matches!(self.dialog, Dialog::None) || view.connecting.is_some() || view.waking.is_some()
    }

    /// Focus that went up into the chrome brings the page back to its top.
    fn chrome_to_top(&mut self) -> ToTop {
        if std::mem::take(&mut self.page_to_top) {
            ToTop::Glide
        } else {
            ToTop::No
        }
    }

    /// What the page wants behind it: its game's ambient, the sky with its moon on a page of cards or
    /// rows. A page that belongs to what came before it keeps what shows.
    fn page_backdrop(&self, view: &View<'_>, sources: &[Source<'_>]) -> backdrop::Want {
        use backdrop::{Moon, Want};
        match self.screen {
            Screen::Home => match &self.hero {
                Some(hero) => home::ambient_for(view, hero.host, hero.app, &hero.title),
                None => Want::Sky(Moon::High),
            },
            Screen::Game { host, app } => {
                let game = sources.iter().find(|s| s.id == host).and_then(|s| s.library).and_then(|l| l.games.iter().find(|g| g.app_id == app));
                match game {
                    Some(game) => home::ambient_for(view, host, app, &game.title),
                    None => Want::Keep,
                }
            }
            // The game the PC runs lights its page; with none, what shows stays.
            Screen::Apps(id) => match view.hosts.get(id).and_then(|host| Some((host, running_app_id(host)?))) {
                Some((host, app)) => home::ambient_for(view, id, app as u32, &running_name(host)),
                None => Want::Keep,
            },
            Screen::AllGames => Want::Keep,
            Screen::Hosts => Want::Sky(first_run::first_run_of(view.hosts).map_or(Moon::Low, first_run::FirstRun::moon)),
            _ => Want::Sky(Moon::High),
        }
    }

    /// Under the chrome, below the top level: a row that leads back and names the page, with that
    /// page's actions at its right.
    fn header(&mut self, ui: &mut Ui, view: &View<'_>, commands: &mut Vec<Command>) {
        let title = match self.screen {
            Screen::Home | Screen::Hosts | Screen::Settings | Screen::Help => return,
            // The page itself is headed by the game's title, beside its cover.
            Screen::Game { .. } => String::new(),
            Screen::AllGames => tr("All games").to_owned(),
            Screen::Apps(id) | Screen::HostDetails(id) | Screen::Files(id) => view.hosts.get(id).map_or("", |h| h.display_name()).to_owned(),
            Screen::HostSettings(_) | Screen::AppSettings(_) | Screen::ProfileSettings(_) => self.edit_name.clone(),
            Screen::Profiles => tr("Profiles").to_owned(),
            Screen::Controllers => tr("Controller buttons").to_owned(),
            Screen::Shortcuts => tr("Keyboard shortcuts").to_owned(),
            Screen::StreamHotkeys => tr("Stream hotkeys").to_owned(),
            Screen::About => tr("About").to_owned(),
            Screen::LatencyBenchmark => tr("Latency benchmark").to_owned(),
        };
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 14.0;
            ui.set_min_height(chrome::BACK_SIZE);
            // Back names where it leads when that is a destination's page, so a screen reader tells
            // it from the destination of the same name in the row above.
            let said = match self.screen {
                Screen::Apps(_) if self.trail.last().is_none_or(|(s, _)| *s == Screen::Hosts) => en("Back to PCs"),
                Screen::Profiles | Screen::Controllers | Screen::Shortcuts | Screen::StreamHotkeys | Screen::About | Screen::LatencyBenchmark => en("Back to Settings"),
                _ => en("Back"),
            };
            let (slot, _) = ui.allocate_exact_size(Vec2::splat(chrome::BACK_SIZE), Sense::hover());
            if self.sticky_back(ui, slot, tr(said)).clicked() {
                self.back_pressed = true;
            }
            if !title.is_empty() {
                // A long name ends in an ellipsis before the page's actions.
                let room = ui.available_width() - self.header_width - 2.0 * space::M;
                ui.allocate_ui(vec2(room.max(0.0), chrome::BACK_SIZE), |ui| {
                    ui.add(egui::Label::new(RichText::new(title).font(semibold(chrome::PAGE_TITLE)).color(PAPER)).truncate());
                });
            }
            self.actions(ui, view, commands);
        });
        ui.add_space(space::L);
    }
}

/// [`Launcher::inherited_from`] for a game (`pc` is its PC's own settings) or a PC (`pc` None).
pub fn inherited_from(pc: Option<&Overrides>, profile: Option<&crate::config::Profile>) -> Vec<(Overrides, String)> {
    let pc = pc.map(|o| (o.clone(), tr("Set for this PC").to_owned()));
    let profile = profile.map(|p| (p.settings.clone(), trf("From profile {}", &[&p.name])));
    pc.into_iter().chain(profile).collect()
}

#[cfg(test)]
mod glass_shots;
#[cfg(test)]
use settings::*;
#[cfg(test)]
mod tests;
pub use settings::FullscreenMode;
