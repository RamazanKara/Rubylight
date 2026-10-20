//! The launcher: PCs, pairing, the app library, settings, and the connecting card. It only
//! draws and collects [`Command`]s; `main.rs` carries them out.

use egui::{
    vec2, Align, Align2, Color32, CornerRadius, Frame, Id, Key, Layout, Margin, Pos2, Rect, Response,
    RichText, Sense, Stroke, StrokeKind, TextureHandle, Ui, Vec2,
};

use crate::config::{
    default_bitrate_kbps, AudioChannels, CodecPreference, DecoderPreference, MouseMode, Settings, WindowMode,
};
use crate::hosts::{Apps, Art, Host, Hosts, Reach};
use crate::stream::{stream_mode, Display};
use crate::theme::*;

pub enum Command {
    Launch { host: u64, app: i32, resume: bool },
    QuitApp(u64),
    QuitAndLaunch { host: u64, app: i32 },
    Pair(u64),
    CancelPair,
    AddHost(String),
    Unpair(u64),
    Remove(u64),
    Refresh(u64),
    OpenApps(u64),
    SaveSettings,
    CancelConnect,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Hosts,
    Apps(u64),
    Settings,
}

pub enum Dialog {
    None,
    AddHost { text: String, error: Option<String> },
    Pairing { host: u64, pin: String },
    Message { title: String, body: String },
    ConfirmQuit { host: u64, start: Option<(i32, String)>, running: String },
}

/// The connecting card's content.
pub struct ConnectingView {
    pub app: String,
    pub host: String,
    pub stage: String,
}

pub struct Launcher {
    pub screen: Screen,
    pub dialog: Dialog,
    pub settings: Settings,
    pub settings_dirty: bool,
    logo: Option<TextureHandle>,
    focus_screen: bool,
    focus_dialog: bool,
    /// The display's tearing support, for the VRR setting's note.
    pub tearing_supported: bool,
    pub display: Display,
    pub identity_ready: bool,
}

/// What the launcher shows, borrowed from the app each frame.
pub struct View<'a> {
    pub hosts: &'a Hosts,
    pub connecting: Option<ConnectingView>,
}

const CARD_W: f32 = 300.0;
const CARD_H: f32 = 112.0;
const POSTER_W: f32 = 168.0;
const POSTER_H: f32 = 224.0;
const CONTENT_MAX: f32 = 1180.0;

fn focus_ring(ui: &Ui, rect: Rect, response: &Response, radius: u8) {
    if response.has_focus() {
        ui.painter().rect_stroke(
            rect.expand(3.0),
            CornerRadius::same(radius + 3),
            Stroke::new(2.0, MOON),
            StrokeKind::Outside,
        );
    } else if response.hovered() {
        ui.painter().rect_stroke(
            rect,
            CornerRadius::same(radius),
            Stroke::new(1.0, OUTLINE),
            StrokeKind::Inside,
        );
    }
}

/// A switch in the brand colours; returns whether it changed.
fn toggle(ui: &mut Ui, on: &mut bool) -> Response {
    let size = vec2(44.0, 24.0);
    let (rect, mut response) = ui.allocate_exact_size(size, Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    let t = ui.ctx().animate_bool_responsive(response.id, *on);
    let enabled = ui.is_enabled();
    let track = if *on { MOON } else { SURFACE_HIGHEST };
    let track = if enabled { track } else { track.gamma_multiply(0.4) };
    ui.painter().rect_filled(rect, CornerRadius::same(12), track);
    let knob_x = egui::lerp(rect.left() + 12.0..=rect.right() - 12.0, t);
    let knob = if *on { ON_MOON } else { OUTLINE };
    ui.painter().circle_filled(Pos2::new(knob_x, rect.center().y), 8.0, knob);
    focus_ring(ui, rect, &response, 12);
    response
}

fn primary_button(ui: &mut Ui, text: &str) -> Response {
    let button = egui::Button::new(RichText::new(text).color(ON_MOON).font(semibold(15.0)))
        .fill(MOON)
        .corner_radius(CornerRadius::same(8))
        .min_size(vec2(0.0, 36.0));
    let response = ui.add(button);
    focus_ring(ui, response.rect, &response, 8);
    response
}

fn quiet_button(ui: &mut Ui, text: &str) -> Response {
    let button = egui::Button::new(RichText::new(text).color(ON_SURFACE).font(semibold(15.0)))
        .fill(SURFACE_HIGHEST)
        .corner_radius(CornerRadius::same(8))
        .min_size(vec2(0.0, 36.0));
    let response = ui.add(button);
    focus_ring(ui, response.rect, &response, 8);
    response
}

fn text_button(ui: &mut Ui, text: &str) -> Response {
    let button = egui::Button::new(RichText::new(text).color(ON_SURFACE_VARIANT).font(semibold(15.0)))
        .frame(false)
        .min_size(vec2(0.0, 36.0));
    let response = ui.add(button);
    focus_ring(ui, response.rect, &response, 8);
    response
}

fn spinner(ui: &mut Ui, radius: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(radius * 2.0), Sense::hover());
    let t = ui.input(|i| i.time) as f32;
    let center = rect.center();
    let start = t * 4.0;
    let points: Vec<Pos2> = (0..=24)
        .map(|i| {
            let a = start + i as f32 / 24.0 * std::f32::consts::PI * 1.4;
            center + radius * 0.8 * vec2(a.cos(), a.sin())
        })
        .collect();
    ui.painter()
        .add(egui::Shape::line(points, Stroke::new(2.5, MOON)));
    ui.ctx().request_repaint();
}

impl Launcher {
    pub fn new(settings: Settings, display: Display) -> Self {
        Self {
            screen: Screen::Hosts,
            dialog: Dialog::None,
            settings,
            settings_dirty: false,
            logo: None,
            focus_screen: true,
            focus_dialog: false,
            tearing_supported: false,
            display,
            identity_ready: false,
        }
    }

    pub fn go(&mut self, screen: Screen) {
        if self.screen == Screen::Settings && screen != Screen::Settings && self.settings_dirty {
            self.settings_dirty = false;
        }
        self.screen = screen;
        self.focus_screen = true;
    }

    /// Gives the screen's first item focus again (after a stream or a dialog).
    pub fn refocus(&mut self) {
        self.focus_screen = true;
    }

    pub fn message(&mut self, title: impl Into<String>, body: impl Into<String>) {
        self.dialog = Dialog::Message {
            title: title.into(),
            body: body.into(),
        };
        self.focus_dialog = true;
    }

    fn logo(&mut self, ctx: &egui::Context) -> egui::TextureId {
        self.logo
            .get_or_insert_with(|| ctx.load_texture("logo", logo_image(128), egui::TextureOptions::LINEAR))
            .id()
    }

    /// Back: Escape or the controller's B button.
    fn back(&mut self, commands: &mut Vec<Command>) {
        match &self.dialog {
            Dialog::None => match self.screen {
                Screen::Hosts => {}
                Screen::Apps(_) => self.go(Screen::Hosts),
                Screen::Settings => {
                    commands.push(Command::SaveSettings);
                    self.go(Screen::Hosts);
                }
            },
            Dialog::Pairing { .. } => {
                commands.push(Command::CancelPair);
                self.dialog = Dialog::None;
            }
            _ => self.dialog = Dialog::None,
        }
    }

    pub fn ui(&mut self, ui: &mut Ui, view: View<'_>) -> Vec<Command> {
        let mut commands = Vec::new();
        let ctx = ui.ctx().clone();
        let escape = ctx.input(|i| i.key_pressed(Key::Escape));
        if escape && view.connecting.is_none() {
            self.back(&mut commands);
        }
        let logo = self.logo(&ctx);
        let full = ui.max_rect();
        ui.painter().rect_filled(full, 0.0, NIGHT);
        // The banner's orbit rings, off the top right corner.
        let center = Pos2::new(full.right() + 40.0, full.top() + 40.0);
        for r in [140.0, 210.0, 280.0, 350.0] {
            ui.painter().circle_stroke(center, r, Stroke::new(1.5, ORBIT));
        }

        let margin = ((full.width() - CONTENT_MAX) / 2.0).max(32.0);
        let content = Rect::from_min_max(
            Pos2::new(full.left() + margin, full.top() + 24.0),
            Pos2::new(full.right() - margin, full.bottom() - 16.0),
        );
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(content).layout(Layout::top_down(Align::Min)));
        let ui = &mut child;
        let blocked = !matches!(self.dialog, Dialog::None) || view.connecting.is_some();
        // Screens take focus only when nothing covers them.
        let held_focus = self.focus_screen;
        if blocked {
            self.focus_screen = false;
        }
        ui.add_enabled_ui(!blocked, |ui| {
            self.header(ui, logo, &view, &mut commands);
            ui.add_space(22.0);
            match self.screen {
                Screen::Hosts => self.hosts(ui, &view, &mut commands, logo),
                Screen::Apps(id) => self.apps(ui, &view, id, &mut commands),
                Screen::Settings => self.settings_page(ui),
            }
        });
        if blocked {
            self.focus_screen = held_focus;
        }
        self.dialogs(&ctx, &view, &mut commands);
        if let Some(c) = &view.connecting {
            self.connecting(&ctx, c, &mut commands);
        }
        commands
    }

    fn header(&mut self, ui: &mut Ui, logo: egui::TextureId, view: &View<'_>, commands: &mut Vec<Command>) {
        ui.horizontal(|ui| {
            ui.set_min_height(40.0);
            match self.screen {
                Screen::Hosts => {
                    egui::Image::from_texture((logo, vec2(30.0, 30.0))).ui(ui);
                    ui.add_space(4.0);
                    ui.label(RichText::new("Nightjar").font(semibold(22.0)).color(PAPER));
                }
                Screen::Apps(id) => {
                    if text_button(ui, "‹  PCs").clicked() {
                        self.go(Screen::Hosts);
                    }
                    let name = view.hosts.get(id).map_or("", |h| h.display_name()).to_owned();
                    ui.label(RichText::new(name).font(semibold(22.0)).color(PAPER));
                }
                Screen::Settings => {
                    if text_button(ui, "‹  Back").clicked() {
                        commands.push(Command::SaveSettings);
                        self.go(Screen::Hosts);
                    }
                    ui.label(RichText::new("Settings").font(semibold(22.0)).color(PAPER));
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 22.0;
                match self.screen {
                Screen::Hosts => {
                    if text_button(ui, "Settings").clicked() {
                        self.go(Screen::Settings);
                    }
                    if text_button(ui, "Add PC").clicked() {
                        self.dialog = Dialog::AddHost {
                            text: String::new(),
                            error: None,
                        };
                        self.focus_dialog = true;
                    }
                }
                Screen::Apps(id) => {
                    if let Some(host) = view.hosts.get(id) {
                        if host.running_app().is_some() && text_button(ui, "Quit running app").clicked() {
                            self.dialog = Dialog::ConfirmQuit {
                                host: id,
                                start: None,
                                running: running_name(host),
                            };
                            self.focus_dialog = true;
                        }
                        if text_button(ui, "Refresh").clicked() {
                            commands.push(Command::Refresh(id));
                        }
                    }
                }
                Screen::Settings => {}
                }
            });
        });
    }

    fn hosts(&mut self, ui: &mut Ui, view: &View<'_>, commands: &mut Vec<Command>, logo: egui::TextureId) {
        ui.label(RichText::new("Your PCs").font(semibold(30.0)).color(ON_SURFACE));
        ui.label(
            RichText::new("PCs running a Moonlight-compatible host on this network appear here. Add one by address if it doesn't.")
                .color(ON_SURFACE_VARIANT),
        );
        ui.add_space(18.0);
        if view.hosts.list.is_empty() {
            ui.add_space(60.0);
            ui.vertical_centered(|ui| {
                egui::Image::from_texture((logo, vec2(72.0, 72.0))).ui(ui);
                ui.add_space(14.0);
                if self.identity_ready {
                    ui.label(RichText::new("Looking for PCs…").font(semibold(18.0)));
                } else {
                    ui.label(RichText::new("Creating this device's pairing key…").font(semibold(18.0)));
                }
                ui.label(
                    RichText::new("Start Sunshine, Apollo, Vibepollo, Rubylight or GeForce Experience on your PC.")
                        .color(ON_SURFACE_VARIANT),
                );
                ui.add_space(12.0);
                let r = primary_button(ui, "Add a PC by address");
                if self.focus_screen {
                    r.request_focus();
                    self.focus_screen = false;
                }
                if r.clicked() {
                    self.dialog = Dialog::AddHost {
                        text: String::new(),
                        error: None,
                    };
                    self.focus_dialog = true;
                }
            });
            return;
        }
        scroll(ui, "hosts", |ui| {
            let columns = ((ui.available_width() + 16.0) / (CARD_W + 16.0)).floor().max(1.0) as usize;
            for (row, chunk) in view.hosts.list.chunks(columns).enumerate() {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 16.0;
                    for (i, host) in chunk.iter().enumerate() {
                        let response = host_card(ui, host);
                        if self.focus_screen && row == 0 && i == 0 {
                            response.request_focus();
                            self.focus_screen = false;
                        }
                        if response.clicked() {
                            if host.paired() {
                                commands.push(Command::OpenApps(host.id));
                            } else if host.reach == Reach::Offline {
                                commands.push(Command::Refresh(host.id));
                            } else {
                                commands.push(Command::Pair(host.id));
                            }
                        }
                        response.context_menu(|ui| {
                            if ui.button("Check again").clicked() {
                                commands.push(Command::Refresh(host.id));
                                ui.close();
                            }
                            if host.paired() && ui.button("Unpair").clicked() {
                                commands.push(Command::Unpair(host.id));
                                ui.close();
                            }
                            if ui.button("Remove").clicked() {
                                commands.push(Command::Remove(host.id));
                                ui.close();
                            }
                        });
                    }
                });
                ui.add_space(16.0);
            }
            ui.add_space(8.0);
            ui.label(
                RichText::new("Right-click a PC to unpair or remove it.")
                    .color(OUTLINE)
                    .font(regular(13.0)),
            );
        });
    }

    fn apps(&mut self, ui: &mut Ui, view: &View<'_>, id: u64, commands: &mut Vec<Command>) {
        let Some(host) = view.hosts.get(id) else {
            self.go(Screen::Hosts);
            return;
        };
        let running = host.running_app();
        match &host.apps {
            Apps::NotLoaded | Apps::Loading => {
                ui.add_space(40.0);
                ui.horizontal(|ui| {
                    spinner(ui, 10.0);
                    ui.label(RichText::new("Loading the library…").color(ON_SURFACE_VARIANT));
                });
            }
            Apps::Failed(e) => {
                ui.label(RichText::new("The library could not be loaded.").font(semibold(18.0)));
                ui.label(RichText::new(e).color(ERROR));
                if quiet_button(ui, "Try again").clicked() {
                    commands.push(Command::Refresh(id));
                }
            }
            Apps::Loaded(apps) => {
                if apps.is_empty() {
                    ui.label(RichText::new("This PC lists no apps.").color(ON_SURFACE_VARIANT));
                    return;
                }
                scroll(ui, "apps", |ui| {
                    let gap = 20.0;
                    let columns = ((ui.available_width() + gap) / (POSTER_W + gap)).floor().max(1.0) as usize;
                    for (row, chunk) in apps.chunks(columns).enumerate() {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = gap;
                            for (i, app) in chunk.iter().enumerate() {
                                let is_running = running.is_some_and(|r| app.matches_running_app(r, None));
                                let art = host.art.get(&app.id);
                                let response = poster(ui, &app.name, art, is_running);
                                let focus = self.focus_screen
                                    && (if running.is_some() { is_running } else { row == 0 && i == 0 });
                                if focus {
                                    response.request_focus();
                                    self.focus_screen = false;
                                }
                                if response.clicked() {
                                    if is_running {
                                        commands.push(Command::Launch {
                                            host: id,
                                            app: app.id,
                                            resume: true,
                                        });
                                    } else if running.is_some() {
                                        self.dialog = Dialog::ConfirmQuit {
                                            host: id,
                                            start: Some((app.id, app.name.clone())),
                                            running: running_name(host),
                                        };
                                        self.focus_dialog = true;
                                    } else {
                                        commands.push(Command::Launch {
                                            host: id,
                                            app: app.id,
                                            resume: false,
                                        });
                                    }
                                }
                            }
                        });
                        ui.add_space(gap);
                    }
                    // A focused-but-unloaded first poster still gets focus next frame.
                    if self.focus_screen && running.is_some() {
                        self.focus_screen = false;
                    }
                });
            }
        }
    }

    fn settings_page(&mut self, ui: &mut Ui) {
        let display = self.display;
        let tearing = self.tearing_supported;
        let s = &mut self.settings;
        let before = s.clone();
        let focus_first = std::mem::take(&mut self.focus_screen);
        scroll(ui, "settings", |ui| {
            ui.set_max_width(760.0);
            section(ui, "Video");
            let native = format!("Native ({}x{})", display.width, display.height);
            let current = if s.width == 0 {
                native.clone()
            } else {
                format!("{}x{}", s.width, s.height)
            };
            let first = row(ui, "Resolution", "The stream's size. Native matches this display.", |ui| {
                egui::ComboBox::from_id_salt("resolution")
                    .selected_text(current)
                    .show_ui(ui, |ui| {
                        let mut v = (s.width, s.height);
                        if ui.selectable_value(&mut v, (0, 0), native).clicked() {
                            (s.width, s.height) = v;
                        }
                        for (w, h) in [(1280, 720), (1600, 900), (1920, 1080), (2560, 1440), (3440, 1440), (3840, 2160)] {
                            let mut v = (s.width, s.height);
                            if ui.selectable_value(&mut v, (w, h), format!("{w}x{h}")).clicked() {
                                (s.width, s.height) = v;
                            }
                        }
                    })
                    .response
            });
            if focus_first {
                first.request_focus();
            }
            let display_fps = display.refresh_hz.round() as u32;
            let fps_text = if s.fps == 0 {
                format!("Display ({display_fps} Hz)")
            } else {
                format!("{} fps", s.fps)
            };
            row(ui, "Frame rate", "Display matches this monitor's refresh rate.", |ui| {
                egui::ComboBox::from_id_salt("fps")
                    .selected_text(fps_text)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut s.fps, 0, format!("Display ({display_fps} Hz)"));
                        for fps in [30, 60, 90, 120, 144, 165, 240] {
                            ui.selectable_value(&mut s.fps, fps, format!("{fps} fps"));
                        }
                    })
                    .response
            });
            let (w, h, fps, _) = stream_mode(s, display);
            let recommended = default_bitrate_kbps(w, h, fps);
            let mut automatic = s.bitrate_kbps == 0;
            row(
                ui,
                "Automatic bitrate",
                &format!("{:.0} Mbps for {w}x{h} at {fps} fps.", f64::from(recommended) / 1000.0),
                |ui| {
                    let r = toggle(ui, &mut automatic);
                    if r.changed() {
                        s.bitrate_kbps = if automatic { 0 } else { recommended };
                    }
                    r
                },
            );
            if !automatic {
                let mut mbps = s.bitrate_kbps as f32 / 1000.0;
                row(
                    ui,
                    "Bitrate",
                    "Higher is sharper and needs a faster network.",
                    |ui| {
                        let maximum = if s.experimental_packed_444 {
                            200.0
                        } else {
                            150.0
                        };
                        let r = ui.add(
                            egui::Slider::new(&mut mbps, 2.0..=maximum)
                                .suffix(" Mbps")
                                .max_decimals(0),
                        );
                        if r.changed() {
                            s.bitrate_kbps = (mbps * 1000.0) as u32;
                        }
                        r
                    },
                );
            }
            row(ui, "Codec", "Automatic picks HEVC when the PC encodes it, else H.264.", |ui| {
                egui::ComboBox::from_id_salt("codec")
                    .selected_text(match s.codec {
                        CodecPreference::Auto => "Automatic",
                        CodecPreference::H264 => "H.264",
                        CodecPreference::Hevc => "HEVC",
                        CodecPreference::Av1 => "AV1",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut s.codec, CodecPreference::Auto, "Automatic");
                        ui.selectable_value(&mut s.codec, CodecPreference::H264, "H.264");
                        ui.selectable_value(&mut s.codec, CodecPreference::Hevc, "HEVC");
                        ui.selectable_value(&mut s.codec, CodecPreference::Av1, "AV1");
                    })
                    .response
            });
            row(
                ui,
                "HDR",
                "Streams 10-bit HDR to an HDR-enabled display. Enable HDR in the operating system first.",
                |ui| toggle(ui, &mut s.hdr),
            );
            row(ui, "Experimental full chroma", "Requires the host option and HEVC or AV1. Restart Nightjar after changing this setting on Linux.",
                |ui| toggle(ui, &mut s.experimental_packed_444));
            row(ui, "Decoder", "Automatic uses the GPU (D3D11VA, VAAPI) and falls back to the CPU.", |ui| {
                egui::ComboBox::from_id_salt("decoder")
                    .selected_text(match s.decoder {
                        DecoderPreference::Auto => "Automatic",
                        DecoderPreference::Software => "Software",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut s.decoder, DecoderPreference::Auto, "Automatic");
                        ui.selectable_value(&mut s.decoder, DecoderPreference::Software, "Software");
                    })
                    .response
            });

            section(ui, "Display");
            row(ui, "Window", "Borderless covers the display without changing its mode.", |ui| {
                egui::ComboBox::from_id_salt("window")
                    .selected_text(match s.window_mode {
                        WindowMode::Fullscreen => "Fullscreen",
                        WindowMode::Borderless => "Borderless",
                        WindowMode::Windowed => "Windowed",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut s.window_mode, WindowMode::Fullscreen, "Fullscreen");
                        ui.selectable_value(&mut s.window_mode, WindowMode::Borderless, "Borderless");
                        ui.selectable_value(&mut s.window_mode, WindowMode::Windowed, "Windowed");
                    })
                    .response
            });
            let vrr_note = if tearing || cfg!(not(windows)) {
                "G-Sync, FreeSync or HDMI VRR is on for this display."
            } else {
                "This system cannot present with tearing, which VRR needs here."
            };
            row(ui, "Variable refresh rate", vrr_note, |ui| toggle(ui, &mut s.vrr));
            ui.add_enabled_ui(s.vrr, |ui| {
                row(
                    ui,
                    "Show frames at the PC's frame times",
                    "Each frame appears at the moment the game made it, plus a steady delay, so motion stays even.",
                    |ui| toggle(ui, &mut s.present_at_host_times),
                );
            });
            ui.add_enabled_ui(!s.vrr, |ui| {
                row(ui, "V-Sync", "Without VRR: wait for the display's refresh, no tearing.", |ui| {
                    toggle(ui, &mut s.vsync)
                });
            });
            row(ui, "Performance overlay", "Frame rates, decode and network times. Ctrl+Alt+Shift+S toggles it.", |ui| {
                toggle(ui, &mut s.overlay)
            });

            section(ui, "Audio");
            row(ui, "Channels", "Surround needs a host and output device that support it.", |ui| {
                egui::ComboBox::from_id_salt("audio")
                    .selected_text(match s.audio {
                        AudioChannels::Stereo => "Stereo",
                        AudioChannels::Surround51 => "5.1 surround",
                        AudioChannels::Surround71 => "7.1 surround",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut s.audio, AudioChannels::Stereo, "Stereo");
                        ui.selectable_value(&mut s.audio, AudioChannels::Surround51, "5.1 surround");
                        ui.selectable_value(&mut s.audio, AudioChannels::Surround71, "7.1 surround");
                    })
                    .response
            });
            row(ui, "Play audio on the PC too", "Keep sound on the PC's speakers while streaming.", |ui| {
                toggle(ui, &mut s.play_audio_on_host)
            });

            section(ui, "Input");
            row(ui, "Mouse", "Captured suits games; desktop pointer suits remote work. Ctrl+Alt+Shift+M switches.", |ui| {
                egui::ComboBox::from_id_salt("mouse")
                    .selected_text(match s.mouse_mode {
                        MouseMode::Capture => "Captured",
                        MouseMode::Absolute => "Desktop pointer",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut s.mouse_mode, MouseMode::Capture, "Captured");
                        ui.selectable_value(&mut s.mouse_mode, MouseMode::Absolute, "Desktop pointer");
                    })
                    .response
            });
            row(
                ui,
                "Send system keys",
                "Alt+Tab, the Windows key and similar go to the PC while input is captured.",
                |ui| toggle(ui, &mut s.capture_system_keys),
            );

            section(ui, "Controllers");
            row(ui, "Controllers", "Every connected controller becomes a controller on the PC.", |ui| {
                toggle(ui, &mut s.controllers)
            });
            ui.add_enabled_ui(s.controllers, |ui| {
                row(ui, "Nintendo button layout", "Swap A/B and X/Y.", |ui| toggle(ui, &mut s.swap_face_buttons));
                row(ui, "Motion controls", "Send gyro and accelerometer data when a game asks for it.", |ui| {
                    toggle(ui, &mut s.controller_motion)
                });
                row(ui, "Input in the background", "Keep controllers working when Nightjar is not focused.", |ui| {
                    toggle(ui, &mut s.background_controllers)
                });
            });
            ui.add_space(18.0);
            ui.label(
                RichText::new(
                    "While streaming: Ctrl+Alt+Shift+Q leaves, +Z releases input, +X toggles fullscreen, +D minimizes. \
                     On a controller, Start+Select+LB+RB leaves.",
                )
                .color(OUTLINE)
                .font(regular(13.0)),
            );
            ui.add_space(24.0);
        });
        if *s != before {
            self.settings_dirty = true;
        }
    }

    fn dialogs(&mut self, ctx: &egui::Context, view: &View<'_>, commands: &mut Vec<Command>) {
        let frame = Frame::new()
            .fill(SURFACE_HIGH)
            .corner_radius(CornerRadius::same(16))
            .inner_margin(Margin::same(28))
            .stroke(Stroke::new(1.0, OUTLINE_VARIANT));
        let mut close = false;
        let focus_first = self.focus_dialog;
        let mut focused = false;
        match &mut self.dialog {
            Dialog::None => return,
            Dialog::AddHost { text, error } => {
                egui::Modal::new(Id::unique("add-host"))
                    .frame(frame)
                    .backdrop_color(Color32::from_black_alpha(150))
                    .show(ctx, |ui| {
                        ui.set_width(420.0);
                        ui.label(RichText::new("Add a PC").font(semibold(22.0)));
                        ui.label(
                            RichText::new("Its IP address or name, with :port if the host uses another HTTP port.")
                                .color(ON_SURFACE_VARIANT),
                        );
                        ui.add_space(8.0);
                        let edit = ui.add(
                            egui::TextEdit::singleline(text)
                                .hint_text("192.168.1.20")
                                .desired_width(f32::INFINITY)
                                .margin(Margin::symmetric(12, 10)),
                        );
                        if focus_first {
                            edit.request_focus();
                            focused = true;
                        }
                        if let Some(e) = error {
                            ui.label(RichText::new(e.as_str()).color(ERROR));
                        }
                        ui.add_space(8.0);
                        let submitted = edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                        ui.horizontal(|ui| {
                            if (primary_button(ui, "Add").clicked() || submitted) && !text.trim().is_empty() {
                                commands.push(Command::AddHost(text.trim().to_owned()));
                                close = true;
                            }
                            if quiet_button(ui, "Cancel").clicked() {
                                close = true;
                            }
                        });
                    });
            }
            Dialog::Pairing { host, pin } => {
                let name = view.hosts.get(*host).map_or("the PC", |h| h.display_name()).to_owned();
                let pin = pin.clone();
                egui::Modal::new(Id::unique("pairing"))
                    .frame(frame)
                    .backdrop_color(Color32::from_black_alpha(150))
                    .show(ctx, |ui| {
                        ui.set_width(440.0);
                        ui.label(RichText::new(format!("Pair with {name}")).font(semibold(22.0)));
                        ui.label(
                            RichText::new("Enter this PIN on the PC: in the host's web page (Sunshine, Apollo, Vibepollo and Rubylight have a PIN page), or in the prompt GeForce Experience shows.")
                                .color(ON_SURFACE_VARIANT),
                        );
                        ui.add_space(18.0);
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 12.0;
                            for digit in pin.chars() {
                                let (rect, _) = ui.allocate_exact_size(vec2(64.0, 80.0), Sense::hover());
                                ui.painter().rect_filled(rect, CornerRadius::same(12), SURFACE_HIGHEST);
                                ui.painter().text(
                                    rect.center(),
                                    Align2::CENTER_CENTER,
                                    digit,
                                    semibold(44.0),
                                    MOON,
                                );
                            }
                        });
                        ui.add_space(18.0);
                        ui.horizontal(|ui| {
                            spinner(ui, 9.0);
                            ui.label(RichText::new("Waiting for the PC…").color(ON_SURFACE_VARIANT));
                        });
                        ui.add_space(10.0);
                        let r = quiet_button(ui, "Cancel");
                        if focus_first {
                            r.request_focus();
                            focused = true;
                        }
                        if r.clicked() {
                            commands.push(Command::CancelPair);
                            close = true;
                        }
                    });
            }
            Dialog::Message { title, body } => {
                egui::Modal::new(Id::unique("message"))
                    .frame(frame)
                    .backdrop_color(Color32::from_black_alpha(150))
                    .show(ctx, |ui| {
                        ui.set_width(440.0);
                        ui.label(RichText::new(title.as_str()).font(semibold(22.0)));
                        ui.label(RichText::new(body.as_str()).color(ON_SURFACE_VARIANT));
                        ui.add_space(12.0);
                        let r = primary_button(ui, "OK");
                        if focus_first {
                            r.request_focus();
                            focused = true;
                        }
                        if r.clicked() {
                            close = true;
                        }
                    });
            }
            Dialog::ConfirmQuit { host, start, running } => {
                let (host, start, running) = (*host, start.clone(), running.clone());
                egui::Modal::new(Id::unique("confirm-quit"))
                    .frame(frame)
                    .backdrop_color(Color32::from_black_alpha(150))
                    .show(ctx, |ui| {
                        ui.set_width(440.0);
                        let title = match &start {
                            Some((_, name)) => format!("Quit {running} and start {name}?"),
                            None => format!("Quit {running}?"),
                        };
                        ui.label(RichText::new(title).font(semibold(22.0)));
                        ui.label(RichText::new("Unsaved progress in the running app is lost.").color(ON_SURFACE_VARIANT));
                        ui.add_space(12.0);
                        ui.horizontal(|ui| {
                            let r = primary_button(ui, if start.is_some() { "Quit and start" } else { "Quit app" });
                            if focus_first {
                                r.request_focus();
                                focused = true;
                            }
                            if r.clicked() {
                                commands.push(match start {
                                    Some((app, _)) => Command::QuitAndLaunch { host, app },
                                    None => Command::QuitApp(host),
                                });
                                close = true;
                            }
                            if quiet_button(ui, "Cancel").clicked() {
                                close = true;
                            }
                        });
                    });
            }
        }
        if focused {
            self.focus_dialog = false;
        }
        if close {
            self.dialog = Dialog::None;
            self.focus_screen = true;
        }
    }

    fn connecting(&mut self, ctx: &egui::Context, c: &ConnectingView, commands: &mut Vec<Command>) {
        let frame = Frame::new()
            .fill(SURFACE_HIGH)
            .corner_radius(CornerRadius::same(16))
            .inner_margin(Margin::same(28))
            .stroke(Stroke::new(1.0, OUTLINE_VARIANT));
        egui::Modal::new(Id::unique("connecting"))
            .frame(frame)
            .backdrop_color(Color32::from_black_alpha(170))
            .show(ctx, |ui| {
                ui.set_width(400.0);
                ui.label(RichText::new(&c.app).font(semibold(24.0)));
                ui.label(RichText::new(format!("on {}", c.host)).color(ON_SURFACE_VARIANT));
                ui.add_space(16.0);
                ui.horizontal(|ui| {
                    spinner(ui, 10.0);
                    ui.label(RichText::new(&c.stage).color(ON_SURFACE));
                });
                ui.add_space(14.0);
                let r = quiet_button(ui, "Cancel");
                r.request_focus();
                if r.clicked() {
                    commands.push(Command::CancelConnect);
                }
            });
    }
}

/// A vertical scroll area with room for focus rings at its edges.
fn scroll(ui: &mut Ui, id: &str, add: impl FnOnce(&mut Ui)) {
    let rect = ui.available_rect_before_wrap();
    let rect = Rect::from_min_max(rect.min - vec2(6.0, 6.0), rect.max + vec2(6.0, 0.0));
    ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
        egui::ScrollArea::vertical()
            .id_salt(id)
            .auto_shrink(false)
            .content_margin(Margin::same(6))
            .show(ui, add);
    });
}

fn running_name(host: &Host) -> String {
    let Some(running) = host.running_app() else {
        return "the running app".into();
    };
    if let Apps::Loaded(apps) = &host.apps {
        if let Some(app) = apps.iter().find(|a| a.matches_running_app(running, None)) {
            return app.name.clone();
        }
    }
    "the running app".into()
}

fn section(ui: &mut Ui, title: &str) {
    ui.add_space(14.0);
    ui.label(RichText::new(title.to_uppercase()).font(semibold(13.0)).color(MOON));
    ui.add_space(2.0);
}

/// A settings row: label and description on the left, the control on the right.
fn row(ui: &mut Ui, label: &str, description: &str, control: impl FnOnce(&mut Ui) -> Response) -> Response {
    let mut out = None;
    Frame::new()
        .fill(SURFACE)
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(18, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let control_width = 240.0;
                ui.vertical(|ui| {
                    ui.set_width((ui.available_width() - control_width - 16.0).max(200.0));
                    ui.label(RichText::new(label).font(semibold(15.0)).color(ON_SURFACE));
                    ui.label(RichText::new(description).font(regular(13.0)).color(ON_SURFACE_VARIANT));
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    out = Some(control(ui));
                });
            });
        });
    ui.add_space(6.0);
    out.expect("control drawn")
}

fn host_card(ui: &mut Ui, host: &Host) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(CARD_W, CARD_H), Sense::click());
    let painter = ui.painter_at(rect.expand(6.0));
    let fill = if response.hovered() || response.has_focus() {
        SURFACE_HIGH
    } else {
        SURFACE
    };
    painter.rect_filled(rect, CornerRadius::same(12), fill);
    let (status, color) = match (host.reach, host.paired()) {
        (Reach::Offline, _) => ("Offline".to_owned(), OUTLINE),
        (Reach::Unknown, _) => ("Checking…".to_owned(), OUTLINE),
        (Reach::Online, false) => ("Not paired · select to pair".to_owned(), TERTIARY),
        (Reach::Online, true) => match host.running_app() {
            Some(_) => ("Paired · an app is running".to_owned(), MOON),
            None => ("Paired · ready".to_owned(), MOON),
        },
    };
    // The moon dot: full when ready, a ring otherwise.
    let dot = Pos2::new(rect.left() + 26.0, rect.top() + 30.0);
    if host.reach == Reach::Online && host.paired() {
        painter.circle_filled(dot, 7.0, MOON);
    } else {
        painter.circle_stroke(dot, 6.0, Stroke::new(2.0, color));
    }
    painter.text(
        Pos2::new(rect.left() + 44.0, rect.top() + 30.0),
        Align2::LEFT_CENTER,
        host.display_name(),
        semibold(19.0),
        ON_SURFACE,
    );
    painter.text(
        Pos2::new(rect.left() + 20.0, rect.top() + 62.0),
        Align2::LEFT_CENTER,
        &status,
        regular(14.0),
        color,
    );
    let detail = match host.info.as_ref().and_then(|i| i.gpu_type.clone()) {
        Some(gpu) if !gpu.is_empty() => format!("{}  ·  {gpu}", host.record.address),
        _ => host.record.address.clone(),
    };
    painter.text(
        Pos2::new(rect.left() + 20.0, rect.top() + 88.0),
        Align2::LEFT_CENTER,
        detail,
        regular(13.0),
        OUTLINE,
    );
    focus_ring(ui, rect, &response, 12);
    response
}

fn poster(ui: &mut Ui, name: &str, art: Option<&Art>, running: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(POSTER_W, POSTER_H + 34.0), Sense::click());
    let image = Rect::from_min_size(rect.min, vec2(POSTER_W, POSTER_H));
    let painter = ui.painter_at(rect.expand(6.0));
    match art {
        Some(Art::Ready(texture)) => {
            egui::Image::from_texture((texture.id(), image.size()))
                .corner_radius(CornerRadius::same(10))
                .paint_at(ui, image);
        }
        _ => {
            // No art from the host: the night surface with the banner's orbits.
            painter.rect_filled(image, CornerRadius::same(10), SURFACE);
            let orbits = ui.painter_at(image.shrink(1.0));
            for r in [70.0, 105.0, 140.0] {
                orbits.circle_stroke(image.right_bottom(), r, Stroke::new(1.5, ORBIT));
            }
            let initials: String = name
                .split_whitespace()
                .filter_map(|w| w.chars().next())
                .take(2)
                .collect::<String>()
                .to_uppercase();
            painter.text(image.center(), Align2::CENTER_CENTER, initials, semibold(40.0), OUTLINE);
        }
    }
    if running {
        let badge = Rect::from_min_size(image.left_top() + vec2(10.0, 10.0), vec2(76.0, 24.0));
        painter.rect_filled(badge, CornerRadius::same(12), MOON);
        painter.text(badge.center(), Align2::CENTER_CENTER, "Running", semibold(12.5), ON_MOON);
    }
    let label_color = if response.has_focus() { MOON } else { ON_SURFACE };
    let galley = painter.layout(name.to_owned(), semibold(14.5), label_color, POSTER_W);
    painter.galley(Pos2::new(rect.left() + 2.0, image.bottom() + 8.0), galley, label_color);
    focus_ring(ui, image, &response, 10);
    response
}

trait UiExt {
    fn ui(self, ui: &mut Ui) -> Response;
}

impl UiExt for egui::Image<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        ui.add(self)
    }
}
