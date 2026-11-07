//! The Android demo's 36-game library for the real launcher on the real SDL path, for captures of
//! the README, the stores and the trailer. No network and no host: the PCs are records with their
//! answers filled in, and no client identity exists, so nothing can ask anybody. Built with the
//! `demo-library` feature only (and for tests); a default build holds none of this.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use nightjar_core::http::native_api::library::{Artwork, Game, Library, Store};
use nightjar_core::http::{
    App, AppControl, Error, FileError, FileList, NativeCapabilities, RemoteFile, ServerInfo,
    TransferCancel, Uploaded,
};

use super::files::{FileHost, Page as FilesPage};
use super::{Command, Dialog, Launcher, Screen};
use crate::config::{HostRecord, Language, Profile, Recent};
use crate::hosts::{Apps, Art, Hosts, Reach, Waker};

/// Title, store, installed: `DemoConsole.LIBRARY` in the Android demo.
pub(crate) const GAMES: [(&str, Option<&str>, bool); 36] = [
    (crate::strings::en("Desktop"), None, true),
    ("Racing Game", Some("steam"), true),
    ("Open World", Some("steam"), true),
    ("Space Sim", Some("epic"), true),
    ("Puzzle Night", Some("gog"), true),
    ("Retro Arcade", Some("steam"), true),
    ("Strategy", Some("microsoft"), true),
    ("Music Studio", None, true),
    ("Ocean Explorer", Some("epic"), true),
    ("Skybound", Some("steam"), true),
    ("Ashfall", Some("steam"), true),
    ("Brightwater", Some("steam"), true),
    ("Cinder Road", Some("steam"), false),
    ("Deep Field", Some("steam"), true),
    ("Ember Keep", Some("steam"), true),
    ("Frostline", Some("steam"), true),
    ("Glass Harbor", Some("steam"), false),
    ("Hollow Peak", Some("steam"), true),
    ("Iron Tide", Some("steam"), true),
    ("Jade Circuit", Some("steam"), true),
    ("Kite Season", Some("steam"), true),
    ("Lantern Row", Some("steam"), false),
    ("Moss and Stone", Some("steam"), true),
    ("Night Courier", Some("steam"), true),
    ("Orbit Garden", Some("steam"), true),
    ("Paper Fleet", Some("steam"), true),
    ("Quiet Engine", Some("steam"), false),
    ("Red Mesa", Some("steam"), true),
    ("Salt Flats Rally", Some("steam"), true),
    ("Tidebreaker", Some("epic"), true),
    ("Under Ice", Some("epic"), false),
    ("Vector Bloom", Some("epic"), true),
    ("Wild Meridian", Some("gog"), true),
    ("Xeno Farm", Some("gog"), false),
    ("Yard Sale Hero", Some("microsoft"), true),
    ("Zero Gravity Golf", Some("microsoft"), true),
];

macro_rules! covers {
    ($($n:literal),* $(,)?) => {
        [$(include_bytes!(concat!("../../../../app/src/debug/assets/demo/art/", $n, ".jpg")) as &[u8]),*]
    };
}

/// Each game's cover, at its place in [`GAMES`].
pub(crate) const COVERS: [&[u8]; 36] = covers!(
    "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15", "16", "17",
    "18", "19", "20", "21", "22", "23", "24", "25", "26", "27", "28", "29", "30", "31", "32", "33",
    "34", "35", "36",
);

const RUNNING: usize = 1;
/// What Continue holds, as in the design's Home: a game's place and the days since it was played.
const RECENTS: [(usize, u64); 3] = [(2, 1), (8, 3), (3, 8)];
const PLAYED_GAME: u32 = 1002;
const FIRST_APP: u32 = 1000;
const PROFILE: u64 = 1;
pub(crate) const PAD: &str = "054c:0ce6";

/// The PCs: the library's, one up and idle, one asleep, one not paired yet.
pub(crate) fn records() -> Vec<HostRecord> {
    let pc = |name: &str, address: &str, paired: bool, mac: bool| HostRecord {
        unique_id: format!("demo-{name}"),
        name: name.into(),
        address: address.into(),
        server_certificate: paired.then(|| "demo".into()),
        mac: mac.then(|| "00:1a:2b:3c:4d:5e".into()),
        manual: true,
        ..Default::default()
    };
    vec![
        pc("Living Room PC", "192.0.2.10", true, true),
        pc("Studio PC", "192.0.2.13", true, true),
        pc("Office PC", "192.0.2.11", true, true),
        pc("Game Room PC", "192.0.2.12", false, false),
    ]
}

fn answer(paired: bool, running: i32) -> ServerInfo {
    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?><root status_code=\"200\"><hostname>PC</hostname><uniqueid>uid</uniqueid><HttpsPort>47984</HttpsPort>\
         <ExternalPort>47989</ExternalPort><PairStatus>{}</PairStatus><currentgame>{running}</currentgame><state>SUNSHINE_SERVER_{}</state>\
         <RustHostVersion>3.0.0</RustHostVersion><NativeApiVersion>1</NativeApiVersion></root>",
        u8::from(paired),
        if running == 0 { "FREE" } else { "BUSY" },
    );
    ServerInfo::parse(&xml, 47989).expect("the demo answer parses")
}

/// Puts the library, its covers, what was played and a profile where the real answers land.
pub(crate) fn fill(hosts: &mut Hosts, launcher: &mut Launcher, ctx: &egui::Context) {
    hosts.discovering = true;
    let games: Vec<Game> = GAMES
        .iter()
        .enumerate()
        .map(|(n, (title, store, installed))| Game {
            install_id: format!("install-{n}"),
            app_id: FIRST_APP + n as u32,
            title: (*title).into(),
            game_id: store.map(|store| format!("{store}:{n}")),
            store: store.map(|store| Store {
                source: store.into(),
                id: n.to_string(),
                name: String::new(),
            }),
            installed: Some(*installed),
            running: n == RUNNING,
            artwork: Artwork {
                cover: Some(format!("/cover/{n}")),
                version: "1".into(),
            },
        })
        .collect();
    let apps: Vec<App> = games
        .iter()
        .map(|g| App {
            name: g.title.clone(),
            id: g.app_id as i32,
            hdr_supported: false,
            uuid: g.install_id.clone(),
            host_index: None,
            art_version: String::new(),
            control: AppControl::None,
            monitor_resume: false,
        })
        .collect();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let library_host = hosts.list[0].record.unique_id.clone();
    launcher.recents = RECENTS
        .iter()
        .map(|&(n, days)| Recent {
            host: library_host.clone(),
            install_id: format!("install-{n}"),
            app_id: FIRST_APP + n as u32,
            title: GAMES[n].0.into(),
            at: now - days * 86_400,
        })
        .collect();
    for (name, chord) in [
        (crate::strings::en("Task Manager"), "Ctrl+Shift+Esc"),
        (crate::strings::en("Game Bar"), "Win+G"),
        ("Next window", "Alt+Tab"),
        (crate::strings::en("Close window"), "Alt+F4"),
        (crate::strings::en("Fullscreen"), "F11"),
        (crate::strings::en("Desktop"), "Win+D"),
        ("Screenshot", "Win+PrintScreen"),
    ] {
        if let Err(why) = crate::shortcuts::save(&mut launcher.shortcuts, None, name, chord) {
            eprintln!("nightjar: demo shortcut {name}: {why}");
        }
    }
    launcher.profiles.entries = vec![Profile {
        id: PROFILE,
        name: "Couch".into(),
        settings: Default::default(),
    }];
    let pc = &mut hosts.list[0];
    pc.reach = Reach::Online;
    pc.info = Some(answer(true, FIRST_APP as i32 + RUNNING as i32));
    pc.native.library = Some(Library {
        revision: "demo".into(),
        games,
    });
    pc.apps = Apps::Loaded(apps.clone());
    for (n, app) in apps.iter().enumerate() {
        let image = image::load_from_memory(COVERS[n])
            .expect("a demo cover")
            .to_rgba8();
        let image = egui::ColorImage::from_rgba_unmultiplied(
            [image.width() as usize, image.height() as usize],
            &image,
        );
        let ambient = crate::ambient::bake(&image);
        pc.ambient_glow
            .insert(app.id, crate::ambient::glow_of(&ambient));
        pc.ambient.insert(
            app.id,
            ctx.load_texture(
                format!("demo-ambient-{n}"),
                ambient,
                egui::TextureOptions::LINEAR,
            ),
        );
        pc.art.insert(
            app.id,
            Art::Ready(ctx.load_texture(
                format!("demo-cover-{n}"),
                image,
                egui::TextureOptions::LINEAR,
            )),
        );
    }
    let idle = &mut hosts.list[1];
    idle.reach = Reach::Online;
    idle.info = Some(answer(true, 0));
    idle.native.library = Some(Library {
        revision: "demo".into(),
        games: Vec::new(),
    });
    idle.apps = Apps::Loaded(Vec::new());
    hosts.list[2].reach = Reach::Offline;
    hosts.list[3].reach = Reach::Online;
    hosts.list[3].info = Some(answer(false, 0));
    launcher.identity_ready = true;
    launcher.settings.console_home = true;
    // What the captures show, whatever this PC's language.
    launcher.settings.language = Language::English;
    launcher.go(Screen::Home);
}

/// The Files page's answers, so that page opens with no PC behind it.
struct DemoFiles;

impl FileHost for DemoFiles {
    fn capabilities(&self) -> Result<NativeCapabilities, Error> {
        Ok(serde_json::from_str(
            "{\"api_version\":1,\"host\":{\"name\":\"PC\",\"version\":\"3.0.0\",\"unique_id\":\"uid\"},\"features\":[\"files.upload\",\"files.download\"],\
             \"permissions\":{\"list\":true,\"view\":true,\"launch\":true,\"host_control\":false,\"file_upload\":true,\"file_download\":true}}",
        )
        .expect("the demo capabilities parse"))
    }

    fn list(&self) -> Result<FileList, FileError> {
        let file = |name: &str, size: u64, modified_unix_ms: u64| RemoteFile {
            name: name.into(),
            size,
            modified_unix_ms,
        };
        Ok(FileList {
            folder: "C:\\Users\\Player\\Downloads\\Rubylight".into(),
            free_bytes: 412_000_000_000,
            max_file_bytes: 8_000_000_000,
            files: vec![
                file("Salt Flats lap.mp4", 482_000_000, 1_761_300_000_000),
                file("Night Courier save.zip", 12_400_000, 1_761_200_000_000),
                file("Controller layout.png", 860_000, 1_761_100_000_000),
            ],
        })
    }

    fn upload(
        &self,
        _: &str,
        _: &mut dyn Read,
        _: u64,
        _: &mut dyn FnMut(u64, u64),
        _: &TransferCancel,
    ) -> Result<Uploaded, FileError> {
        Err(FileError::Local(std::io::Error::other(
            "the demo library has no PC to send to",
        )))
    }

    fn download(
        &self,
        _: &str,
        _: u64,
        _: &Path,
        _: &mut dyn FnMut(u64, u64),
        _: &TransferCancel,
    ) -> Result<u64, FileError> {
        Err(FileError::Local(std::io::Error::other(
            "the demo library has no PC to fetch from",
        )))
    }
}

/// A page of the stream menu the demo shows, and over what.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DemoMenu {
    Grid,
    /// The grid over a white picture, the worst case for its words.
    White,
    Quality,
    More,
    Confirm,
}

/// Every page the driver can open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Page {
    Home,
    AllGames,
    Game,
    Pcs,
    Apps,
    AppsEmpty,
    Pc,
    Files,
    Settings,
    PcSettings,
    GameSettings,
    Profiles,
    ProfileSettings,
    Controllers,
    Shortcuts,
    Hotkeys,
    Help,
    Search,
    /// The first run: the driver keeps the PCs off the list for these two.
    FirstRun,
    FirstRunFound,
    /// A dialog over the PC list.
    Dialog(DemoDialog),
    /// The PIN screen: starting a stream, and choosing a PIN.
    PinStream,
    PinChoose,
    /// The PIN lock settings page, with a PIN and this mode or with none.
    PinLock(Option<crate::pin_lock::Mode>),
    /// The real stream menu's pages over a still standing in for the video.
    StreamMenu(DemoMenu),
    /// Home with a toast up.
    Toast,
}

/// The dialogs the demo can open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DemoDialog {
    Remove,
    Unpair,
    Rename,
    Add,
    Pairing,
    Approve,
    QuitGame,
    Message,
    PcMenu,
}

impl Page {
    /// A dialog or the PIN screen: what's under it doesn't scroll.
    #[cfg(test)]
    pub(crate) fn covers_the_page(self) -> bool {
        matches!(self, Page::Dialog(_) | Page::PinStream | Page::PinChoose | Page::PinLock(_) | Page::StreamMenu(_))
    }

    pub(crate) const ALL: [(&'static str, Page); 40] = [
        ("home", Page::Home),
        ("all-games", Page::AllGames),
        ("game", Page::Game),
        ("pcs", Page::Pcs),
        ("apps", Page::Apps),
        ("apps-empty", Page::AppsEmpty),
        ("pc", Page::Pc),
        ("files", Page::Files),
        ("settings", Page::Settings),
        ("pc-settings", Page::PcSettings),
        ("game-settings", Page::GameSettings),
        ("profiles", Page::Profiles),
        ("profile-settings", Page::ProfileSettings),
        ("controllers", Page::Controllers),
        ("shortcuts", Page::Shortcuts),
        ("hotkeys", Page::Hotkeys),
        ("help", Page::Help),
        ("search", Page::Search),
        ("first-run", Page::FirstRun),
        ("first-run-found", Page::FirstRunFound),
        ("dialog-remove", Page::Dialog(DemoDialog::Remove)),
        ("dialog-unpair", Page::Dialog(DemoDialog::Unpair)),
        ("dialog-rename", Page::Dialog(DemoDialog::Rename)),
        ("dialog-add", Page::Dialog(DemoDialog::Add)),
        ("dialog-pairing", Page::Dialog(DemoDialog::Pairing)),
        ("dialog-approve", Page::Dialog(DemoDialog::Approve)),
        ("dialog-quit", Page::Dialog(DemoDialog::QuitGame)),
        ("dialog-message", Page::Dialog(DemoDialog::Message)),
        ("dialog-pc-menu", Page::Dialog(DemoDialog::PcMenu)),
        ("pin-stream", Page::PinStream),
        ("pin-choose", Page::PinChoose),
        ("pin-page-empty", Page::PinLock(None)),
        ("pin-page", Page::PinLock(Some(crate::pin_lock::Mode::App))),
        ("pin-page-selected", Page::PinLock(Some(crate::pin_lock::Mode::Selected))),
        ("stream-menu", Page::StreamMenu(DemoMenu::Grid)),
        ("stream-menu-white", Page::StreamMenu(DemoMenu::White)),
        ("stream-menu-quality", Page::StreamMenu(DemoMenu::Quality)),
        ("stream-menu-more", Page::StreamMenu(DemoMenu::More)),
        ("stream-menu-confirm", Page::StreamMenu(DemoMenu::Confirm)),
        ("toast", Page::Toast),
    ];
}

/// One line from the driver on stdin.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Step {
    Go(Page),
    Zoom(u32),
    Size(i32, i32),
    Pointer(f32, f32),
    /// The left button pressed and let go there.
    Click(f32, f32),
    /// One wheel event, in points.
    Wheel(f32),
    /// A wheel event on each of the next `frames` frames.
    Scroll {
        frames: u32,
        delta: f32,
    },
    Key(egui::Key),
    /// Saves the next frame. `chain`: with the blur chain run again, as on a moving frame.
    Capture {
        chain: bool,
        path: PathBuf,
    },
    Contrast(bool),
    Input(crate::input_source::InputSource),
    /// Writes the frames presented so far.
    Stats(PathBuf),
    Quit,
}

/// Reads a driver line. What it can't read is said loudly by the caller.
pub(crate) fn parse(line: &str) -> Result<Step, String> {
    let line = line.trim();
    let (word, rest) = line.split_once(' ').unwrap_or((line, ""));
    let rest = rest.trim();
    let number = |text: &str| {
        text.parse::<f32>()
            .map_err(|_| format!("{line:?}: {text:?} is no number"))
    };
    let pair = || {
        rest.split_once(' ')
            .map(|(a, b)| (a, b.trim()))
            .ok_or_else(|| format!("{line:?}: two values"))
    };
    let path = || {
        if rest.is_empty() {
            Err(format!("{line:?}: a path"))
        } else {
            Ok(PathBuf::from(rest))
        }
    };
    Ok(match word {
        "go" => Step::Go(
            Page::ALL
                .iter()
                .find(|(name, _)| *name == rest)
                .map(|(_, page)| *page)
                .ok_or_else(|| format!("{line:?}: no such page"))?,
        ),
        "zoom" => Step::Zoom(number(rest)? as u32),
        "size" => {
            let (w, h) = pair()?;
            Step::Size(number(w)? as i32, number(h)? as i32)
        }
        "pointer" => {
            let (x, y) = pair()?;
            Step::Pointer(number(x)?, number(y)?)
        }
        "click" => {
            let (x, y) = pair()?;
            Step::Click(number(x)?, number(y)?)
        }
        "wheel" => Step::Wheel(number(rest)?),
        "scroll" => {
            let (frames, delta) = pair()?;
            Step::Scroll {
                frames: number(frames)? as u32,
                delta: number(delta)?,
            }
        }
        "key" => {
            Step::Key(egui::Key::from_name(rest).ok_or_else(|| format!("{line:?}: no such key"))?)
        }
        "capture" => match rest.strip_prefix("chain ") {
            Some(chained) => Step::Capture {
                chain: true,
                path: PathBuf::from(chained.trim()),
            },
            None => Step::Capture {
                chain: false,
                path: path()?,
            },
        },
        "contrast" => Step::Contrast(rest == "on"),
        "input" => Step::Input(match rest {
            "keyboard" => crate::input_source::InputSource::Keyboard,
            "pad" => crate::input_source::InputSource::Pad(crate::glyphs::Style::Xbox),
            "sony" => crate::input_source::InputSource::Pad(crate::glyphs::Style::PlayStation),
            "none" => crate::input_source::InputSource::None,
            _ => return Err(format!("{line:?}: no such input")),
        }),
        "stats" => Step::Stats(path()?),
        "quit" => Step::Quit,
        _ => return Err(format!("{line:?}: no such step")),
    })
}

impl Launcher {
    /// Opens a page the way its own control does. A page main.rs opens comes back as its command.
    pub(crate) fn demo_page(&mut self, page: Page, hosts: &Hosts, waker: Waker) -> Option<Command> {
        self.dialog = Dialog::None;
        self.pin = None;
        self.demo_menu = None;
        if matches!(page, Page::FirstRun | Page::FirstRunFound) {
            self.go(Screen::Hosts);
            return None;
        }
        let pc = &hosts.list[0];
        let id = pc.id;
        match page {
            Page::FirstRun | Page::FirstRunFound => {}
            Page::Home => self.go(Screen::Home),
            Page::AllGames => self.go(Screen::AllGames),
            Page::Game => self.go(Screen::Game {
                host: id,
                app: PLAYED_GAME,
            }),
            Page::Pcs => self.go(Screen::Hosts),
            Page::Apps => self.go(Screen::Apps(id)),
            Page::AppsEmpty => self.go(Screen::Apps(hosts.list[1].id)),
            Page::Pc => self.go(Screen::HostDetails(id)),
            Page::Files => {
                let base = pc
                    .info
                    .as_ref()
                    .map_or(nightjar_core::http::HostCapabilities::UNKNOWN, |i| {
                        i.capabilities.clone()
                    });
                self.files.page = Some(FilesPage::new(
                    id,
                    Arc::new(DemoFiles),
                    base,
                    Ok(std::env::temp_dir()),
                    waker,
                ));
                self.open(Screen::Files(id));
            }
            Page::Settings => self.go(Screen::Settings),
            Page::PcSettings => self.open_host_settings(id, pc),
            Page::GameSettings => {
                if let Apps::Loaded(apps) = &pc.apps {
                    self.open_app_settings(
                        id,
                        pc,
                        apps[(PLAYED_GAME - FIRST_APP) as usize].clone(),
                    );
                }
            }
            Page::Profiles => self.go(Screen::Profiles),
            Page::ProfileSettings => return Some(Command::ProfileSettings(PROFILE)),
            Page::Controllers => {
                self.controller_key = PAD.into();
                self.controller_mapping = crate::gamepad::Mapping {
                    name: "DualSense Wireless Controller".into(),
                    ..Default::default()
                };
                self.go(Screen::Controllers);
            }
            Page::Shortcuts => self.go(Screen::Shortcuts),
            Page::Hotkeys => self.go(Screen::StreamHotkeys),
            Page::Help => self.go(Screen::Help),
            Page::Dialog(dialog) => {
                self.go(Screen::Hosts);
                self.dialog = match dialog {
                    DemoDialog::Remove => Dialog::ConfirmRemove { host: id, name: pc.display_name().to_owned() },
                    DemoDialog::Unpair => Dialog::ConfirmUnpair { host: id, name: pc.display_name().to_owned() },
                    DemoDialog::Rename => Dialog::Rename { host: id, text: pc.display_name().to_owned() },
                    DemoDialog::Add => Dialog::AddHost { text: String::new(), error: None },
                    DemoDialog::Pairing => Dialog::Pairing { host: id, pin: "4821".to_owned() },
                    DemoDialog::Approve => Dialog::ApprovePairing {
                        host: id,
                        request: String::new(),
                        name: "Living room TV".to_owned(),
                        address: "192.168.1.40".to_owned(),
                        pc: pc.display_name().to_owned(),
                        pin: "48".to_owned(),
                        busy: false,
                    },
                    DemoDialog::QuitGame => Dialog::ConfirmClose { host: id, name: "Racing Game".to_owned() },
                    DemoDialog::Message => Dialog::Message { title: "Can't reach this PC".to_owned(), body: "It didn't answer on the network.".to_owned() },
                    DemoDialog::PcMenu => Dialog::HostOptions { host: id },
                };
            }
            Page::PinStream | Page::PinChoose => {
                self.go(Screen::Hosts);
                let dir = std::env::temp_dir().join("nightjar-demo-pin");
                self.pin = Some(crate::pin_ui::PinLock::demo(&dir, page == Page::PinChoose, pc.display_name()));
            }
            Page::StreamMenu(menu) => {
                self.go(Screen::Hosts);
                self.demo_menu = Some(menu);
            }
            Page::PinLock(mode) => {
                self.go(Screen::Hosts);
                let dir = std::env::temp_dir().join("nightjar-demo-pin-page");
                self.pin = Some(crate::pin_ui::PinLock::demo_page(&dir, mode));
            }
            Page::Toast => {
                self.go(Screen::Home);
                self.toast(crate::strings::trf("Quitting {} on the PC…", &[&"Racing Game"]));
            }
            Page::Search => {
                self.go(Screen::Home);
                self.open_search();
                self.library_query = "ocean".into();
            }
        }
        None
    }
}

impl Launcher {
    /// The stream menu over a still of the Racing Game, as `draw_overlay` draws it: the real panel,
    /// grid and footer, and the real rows on a stream's pages, with what a stream offers as a literal.
    pub(crate) fn demo_stream_menu(&mut self, ui: &mut egui::Ui, menu: DemoMenu) {
        use crate::glyphs::Icon as Glyph;
        use crate::menu_style as style;
        use crate::session_page::HostExit;
        use crate::stream_menu::{self, Page as MenuPage};
        use crate::strings::tr;
        let ctx = ui.ctx().clone();
        let full = ctx.content_rect();
        if menu == DemoMenu::White {
            ui.painter().rect_filled(full, 0.0, egui::Color32::WHITE);
        } else {
            let texture = self.demo_video.get_or_insert_with(|| {
                let at = GAMES.iter().position(|g| g.0 == "Racing Game").expect("the demo's racing game");
                let image = image::load_from_memory(COVERS[at]).expect("a JPEG").to_rgba8();
                let size = [image.width() as usize, image.height() as usize];
                ctx.load_texture("demo-video", egui::ColorImage::from_rgba_unmultiplied(size, &image), egui::TextureOptions::LINEAR)
            });
            // Cut to the window's shape, as the game fills it.
            let [w, h] = texture.size().map(|v| v as f32);
            let shown = (full.height() / full.width() * w / h).min(1.0);
            let uv = egui::Rect::from_min_max(egui::pos2(0.0, (1.0 - shown) * 0.62), egui::pos2(1.0, (1.0 - shown) * 0.62 + shown));
            ui.painter().image(texture.id(), full, uv, egui::Color32::WHITE);
        }
        let available = stream_menu::Available {
            mouse: Some(true),
            overlay: true,
            shortcuts: true,
            microphone: Some(false),
            quality: true,
            clipboard: true,
            image_clipboard: true,
            host_audio: true,
            exits: vec![HostExit::QuitApp],
            ..Default::default()
        };
        let layout = stream_menu::layout(&available, self.shortcuts.data.len(), &Default::default());
        let saved = &self.shortcuts;
        let focus_first = ctx.memory(|m| m.focused().is_none());
        let line = "Racing Game · Living Room PC";
        style::show(&ctx, tr("Stream controls"), "stream-controls", |ui| match menu {
            DemoMenu::Grid | DemoMenu::White => {
                style::head(ui, "Racing Game", "1080p · 120 FPS · HEVC HDR");
                let reserve = style::pinned_room(ui.ctx(), "stream-menu-footer");
                style::body_above(ui, reserve, |ui| stream_menu::grid(ui, &layout, saved, None, focus_first));
                style::pinned(ui, "stream-menu-footer", |ui| stream_menu::footer(ui, &layout));
            }
            DemoMenu::Quality => style::body(ui, |ui| {
                style::header(ui, MenuPage::Quality.title(), line);
                style::group(ui, |ui| {
                    let mut on = false;
                    style::switch(ui, Glyph::Gauge, tr("Device controls bitrate"), &mut on);
                    let mut kbps = 80_000;
                    crate::stream::menu_extras::bitrate_slider(ui, &mut kbps, true);
                    style::fold(ui, "demo-health", Glyph::Pulse, tr("Why did my stream stutter?"), |_| {});
                    style::fold(ui, "demo-size", Glyph::Screen, tr("Stream size"), |_| {});
                });
                ui.add_space(crate::theme::space::M);
                style::action_keys(ui, Glyph::Back, tr("Back"), Some(crate::shortcuts::Key::Escape.cap().as_ref()));
            }),
            DemoMenu::More => style::body(ui, |ui| {
                style::header(ui, MenuPage::More.title(), line);
                style::group(ui, |ui| {
                    style::row(ui, Glyph::Overlay, tr("Performance overlay"), |ui| {
                        super::Combo::from_id_salt("demo-overlay").width(190.0).selected_text(tr("Off")).show_ui(ui, |_| {});
                    });
                    style::row(ui, Glyph::Screen, tr("Video size"), |ui| {
                        super::Combo::from_id_salt("demo-scaling").width(190.0).selected_text(tr(crate::config::VideoScaling::Fit.label())).show_ui(ui, |_| {});
                    });
                    let mut on = true;
                    style::switch(ui, Glyph::Speaker, tr("Night mode"), &mut on);
                    style::action(ui, Glyph::Image, tr("Send image to PC"), None);
                    style::action(ui, Glyph::Download, tr("Export statistics (last hour)"), None);
                    style::action(ui, Glyph::Help, tr("Shortcuts & help"), Some(crate::shortcuts::Action::StreamShortcuts));
                    style::action(ui, Glyph::Reconnect, tr("Reconnect"), None);
                });
                ui.add_space(crate::theme::space::M);
                style::action_keys(ui, Glyph::Back, tr("Back"), Some(crate::shortcuts::Key::Escape.cap().as_ref()));
            }),
            DemoMenu::Confirm => style::body(ui, |ui| {
                style::header(ui, tr(HostExit::QuitApp.label()), line);
                stream_menu::confirm(ui, HostExit::QuitApp, focus_first);
            }),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real launcher's library is the one the Android demo, the README and the covers share:
    /// `art/N.jpg` carries the title of place N.
    #[test]
    fn the_demo_library_is_the_android_demos_library() {
        let java = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../app/src/debug/java/com/limelight/demo/DemoConsole.java"),
        )
        .expect("the demo library");
        let library = java
            .split("LIBRARY = {")
            .nth(1)
            .expect("the list")
            .split("};")
            .next()
            .expect("its end");
        let lines: Vec<&str> = library
            .lines()
            .map(str::trim)
            .filter(|line| line.starts_with("{\""))
            .collect();
        for (n, (title, store, installed)) in GAMES.iter().enumerate() {
            let fields: Vec<&str> = lines[n]
                .trim_matches(|c| "{},".contains(c))
                .split(", ")
                .collect();
            assert_eq!(fields[0].trim_matches('"'), *title, "place {n}");
            assert_eq!(
                fields[1],
                store.map_or("null".to_string(), |s| format!("\"{s}\"")),
                "{title}'s store"
            );
            assert_eq!(fields[2] == "\"1\"", *installed, "{title} installed");
        }
    }

    #[test]
    fn every_cover_is_a_picture_of_its_own() {
        let mut seen = std::collections::HashSet::new();
        for (n, cover) in COVERS.iter().enumerate() {
            image::load_from_memory(cover).expect("a JPEG");
            assert!(seen.insert(*cover), "cover {} repeats another", n + 1);
        }
    }

    /// The demo stays out of what ships: the feature is off unless asked for by name.
    #[test]
    fn the_demo_library_is_not_a_default_feature() {
        let manifest: &str = include_str!("../../Cargo.toml");
        assert!(
            manifest.lines().any(|line| line.trim() == "default = []"),
            "no default features"
        );
        assert!(
            manifest
                .lines()
                .any(|line| line.trim() == "demo-library = []"),
            "the feature adds no dependency"
        );
        let workflow = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../..")
            .join(".github/workflows/nightjar-android.yml");
        if let Ok(text) = std::fs::read_to_string(workflow) {
            assert!(
                !text.contains("--all-features") && !text.contains("demo-library"),
                "CI builds the demo"
            );
        }
    }

    #[test]
    fn the_driver_reads_each_step_and_refuses_the_rest() {
        assert_eq!(parse("go all-games"), Ok(Step::Go(Page::AllGames)));
        assert_eq!(parse("zoom 150"), Ok(Step::Zoom(150)));
        assert_eq!(parse("size 1280 800"), Ok(Step::Size(1280, 800)));
        assert_eq!(parse("pointer 640.5 400"), Ok(Step::Pointer(640.5, 400.0)));
        assert_eq!(parse("wheel -120"), Ok(Step::Wheel(-120.0)));
        assert_eq!(
            parse("scroll 80 -6"),
            Ok(Step::Scroll {
                frames: 80,
                delta: -6.0
            })
        );
        assert_eq!(parse("key ArrowDown"), Ok(Step::Key(egui::Key::ArrowDown)));
        assert_eq!(
            parse("capture C:\\a b\\c.png"),
            Ok(Step::Capture {
                chain: false,
                path: "C:\\a b\\c.png".into()
            })
        );
        assert_eq!(
            parse("capture chain C:\\a b\\c.png"),
            Ok(Step::Capture {
                chain: true,
                path: "C:\\a b\\c.png".into()
            })
        );
        assert_eq!(parse("contrast on"), Ok(Step::Contrast(true)));
        assert_eq!(
            parse("stats C:\\s.txt"),
            Ok(Step::Stats("C:\\s.txt".into()))
        );
        assert_eq!(parse("quit"), Ok(Step::Quit));
        for bad in [
            "go nowhere",
            "zoom big",
            "size 1280",
            "key Nope",
            "capture",
            "fly",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }
}
