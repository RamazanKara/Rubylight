//! Demo capture driver: script step parser, page/dialog navigation, and stream-menu preview.

use std::path::PathBuf;

use super::*;
use super::super::files::Page as FilesPage;
use super::super::{Command, Combo, Dialog};
use crate::hosts::Waker;

/// A page of the stream menu the demo shows, and over what.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DemoMenu {
    Grid,
    /// The grid over a white picture, the worst case for its words.
    White,
    Quality,
    More,
    Confirm,
}

/// Every page the driver can open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
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
pub enum DemoDialog {
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

    pub const ALL: [(&'static str, Page); 40] = [
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
pub enum Step {
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
    /// Sets one main setting as the settings page would: its key and a JSON value.
    Setting(String, serde_json::Value),
    /// Starts the first game of the first PC with a game list, as its Play does, as far as a demo
    /// PC goes: the stream's checks run (and Linux's renderer switch), no PC is asked.
    Launch,
    Quit,
}

/// Reads a driver line. What it can't read is said loudly by the caller.
pub fn parse(line: &str) -> Result<Step, String> {
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
        "setting" => {
            let (key, value) = pair()?;
            Step::Setting(key.to_owned(), serde_json::from_str(value).map_err(|e| format!("{line:?}: {e}"))?)
        }
        "launch" => Step::Launch,
        "quit" => Step::Quit,
        _ => return Err(format!("{line:?}: no such step")),
    })
}

impl Launcher {
    /// Opens a page the way its own control does. A page app/command.rs opens comes back as its command.
    pub fn demo_page(&mut self, page: Page, hosts: &Hosts, waker: Waker) -> Option<Command> {
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
                    .map_or(nightjar_policy::http::HostCapabilities::UNKNOWN, |i| {
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
                self.controller_mapping = crate::gamepad_types::Mapping {
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
                    DemoDialog::Message => Dialog::Message { title: "Can’t reach this PC".to_owned(), body: "It didn’t answer on the network.".to_owned() },
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
    pub fn demo_stream_menu(&mut self, ui: &mut egui::Ui, menu: DemoMenu) {
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
            // Disconnect and Quit game in the footer; End session is More's last row.
            exits: vec![HostExit::End, HostExit::QuitApp],
            ..Default::default()
        };
        let layout = stream_menu::layout(&available, self.shortcuts.data.len(), &Default::default());
        let saved = &self.shortcuts;
        let focus_first = ctx.memory(|m| m.focused().is_none());
        let line = "Racing Game · Living Room PC";
        style::show(&ctx, tr("Control Center"), "stream-controls", |ui| match menu {
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
                    style::switch(ui, Glyph::Gauge, tr("Lower bitrate when the network is busy"), &mut on);
                    let mut kbps = 80_000;
                    crate::stream_overlay_widgets::bitrate_slider(ui, &mut kbps, true);
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
                        Combo::from_id_salt("demo-overlay").width(190.0).selected_text(tr("Off")).show_ui(ui, |_| {});
                    });
                    style::row(ui, Glyph::Screen, tr("Video size"), |ui| {
                        Combo::from_id_salt("demo-scaling").width(190.0).selected_text(tr(crate::config::VideoScaling::Fit.label())).show_ui(ui, |_| {});
                    });
                    let mut on = true;
                    style::switch(ui, Glyph::Speaker, tr("Night mode"), &mut on);
                    style::action(ui, Glyph::Image, tr("Send image to PC"), None);
                    style::action(ui, Glyph::Download, tr("Export statistics (last hour)"), None);
                    style::action(ui, Glyph::Help, tr("Shortcuts & help"), Some(crate::shortcuts::Action::StreamShortcuts));
                    style::action(ui, Glyph::Reconnect, tr("Reconnect"), None);
                    style::action(ui, Glyph::Power, tr(HostExit::End.label()), None);
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
