//! The Help page, the desktop counterpart of Android's Help topics: getting started, the
//! keyboard and controller shortcuts, sharing, the common problems with their fixes, and the
//! guides and the log folder. The shortcut lists are drawn from `shortcuts.rs`, the same list the
//! key handling looks its keys up in, so they cannot drift from what the keys do.

use super::*;
use crate::glyphs::{self, Icon, Piece, Style};
use crate::shortcuts::{self, Action, Button, Chord, Press, Scope};

/// Below this width the cards of a section stack instead of sitting side by side.
const TWO_UP: f32 = 760.0;
/// Room for each card's content inside its frame (left and right).
const CARD_MARGIN: f32 = 18.0;

/// The jump chips at the top and the section each one scrolls to.
const SECTIONS: [(&str, &str); 6] = [
    (en("Getting started"), en("Getting started")),
    (en("Keyboard"), en("Keyboard shortcuts")),
    (en("Controllers"), en("Controllers")),
    (en("Sharing"), en("Sharing")),
    (en("Troubleshooting"), en("Troubleshooting")),
    (en("More help"), en("More help")),
];

/// The common problems on Windows and Linux and their one-line fixes (`docs/TROUBLESHOOTING.md`).
/// Only Windows asks about private networks.
const PROBLEMS: [(&str, &str); 5] = [
    (
        en("PCs are not found"),
        if cfg!(windows) {
            en("Allow Nightjar on private networks when Windows asks, or add the PC by its address with Add PC.")
        } else {
            en("Connect both to the same network, or add the PC by its address with Add PC.")
        },
    ),
    (en("Pairing fails"), en("Cancel, clear the pending request in the host's web page, and pair again with a new PIN.")),
    (en("Black picture or no video"), en("Try H.264 at 720p with HDR and upscaling off, then raise one setting at a time.")),
    (en("Stutter or lag"), en("Lower the bitrate or connect the PC by cable. Why did my stream stutter? in the stream controls names the cause.")),
    (en("No sound"), en("Choose Stereo, check the PC's audio output, then reconnect.")),
];

/// Where Nightjar's releases and their notes are.
const RELEASES: &str = "https://github.com/RamazanKara/nightjar/releases";

impl Launcher {
    /// F1, or ? where no text field has focus, opens Help from anywhere in the launcher.
    pub(super) fn help_keys(&mut self, ctx: &egui::Context, commands: &mut Vec<Command>) {
        if self.screen == Screen::Help || !matches!(self.dialog, Dialog::None) || egui::Popup::is_any_open(ctx) {
            return;
        }
        let typing = ctx.text_edit_focused();
        if ctx.input(|i| shortcuts::pressed(i, Action::Help, typing)) {
            self.open_help(commands);
        }
    }

    /// Help from anywhere: from a top-level page (or a settings page, saved first) it replaces it
    /// as the other destinations do; over a PC's or game's page Back returns there.
    pub fn open_help(&mut self, commands: &mut Vec<Command>) {
        if self.screen == Screen::Help {
            return;
        }
        commands.extend(self.screen.save_command());
        let destination = DESTINATIONS.iter().any(|(_, screen)| *screen == self.screen);
        if destination || matches!(self.screen, Screen::HostSettings(_) | Screen::AppSettings(_) | Screen::ProfileSettings(_)) {
            self.switch(Screen::Help);
        } else {
            self.open(Screen::Help);
        }
    }

    pub(super) fn help_page(&mut self, ui: &mut Ui, view: &View<'_>, commands: &mut Vec<Command>) {
        let ctx = ui.ctx().clone();
        let jump_id = Id::unique("help-jump");
        let mut jump: Option<&'static str> = ctx.data(|d| d.get_temp(jump_id));
        let width = ui.available_width();
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(space::S, space::S);
            for (n, (name, section)) in SECTIONS.iter().enumerate() {
                let chip = chip(ui, name);
                self.focus_item(&chip, &format!("help-jump-{n}"), n == 0);
                if chip.clicked() {
                    jump = Some(section);
                }
            }
        });
        ui.add_space(space::S);

        section_at(ui, "Getting started", &mut jump);
        self.help_start(ui, width, view, commands);

        section_at(ui, "Keyboard shortcuts", &mut jump);
        self.help_keyboard(ui, width);

        section_at(ui, "Controllers", &mut jump);
        self.help_controller(ui, width, view);

        section_at(ui, "Sharing", &mut jump);
        self.help_sharing(ui, width);

        section_at(ui, "Troubleshooting", &mut jump);
        self.help_problems(ui, width, commands);

        section_at(ui, "More help", &mut jump);
        self.help_links(ui, commands);
        ui.add_space(space::XL);
        ctx.data_mut(|d| match jump {
            Some(section) => {
                d.insert_temp(jump_id, section);
            }
            None => {
                d.remove::<&'static str>(jump_id);
            }
        });
    }

    fn help_start(&mut self, ui: &mut Ui, width: f32, view: &View<'_>, commands: &mut Vec<Command>) {
        let style = Style::of(view.pads);
        let quit = self.settings.gamepad_quit_combo;
        let steps: [(&str, &str); 3] = [
            (en("Find your PC"), en("Start a Moonlight-compatible host on the same network. If it doesn't show under PCs, add it by its address.")),
            (en("Pair"), en("Select the PC and enter the PIN Nightjar shows in the host's web page.")),
            (en("Play"), en("Open the PC's library and choose a game.")),
        ];
        let columns = if width >= 960.0 { 3 } else { 1 };
        grid(ui, "help-steps", columns, steps.len(), |ui, n| {
            let (title, body) = steps[n];
            ui.horizontal(|ui| {
                step_badge(ui, n + 1);
                ui.add_space(space::S);
                ui.label(RichText::new(tr(title)).font(semibold(text::CARD_TITLE)).color(ON_SURFACE));
            });
            ui.add_space(space::S);
            ui.label(RichText::new(tr(body)).font(regular(text::LABEL)).color(ON_SURFACE_VARIANT));
            ui.add_space(space::M);
            match n {
                0 => {
                    ui.horizontal(|ui| {
                        let add = link_button(ui, "Add a PC");
                        self.focus_item(&add, "help-add-pc", false);
                        if add.clicked() {
                            if self.screen != Screen::Hosts {
                                self.switch(Screen::Hosts);
                            }
                            self.add_host_dialog();
                        }
                        if let Some(chord) = shortcuts::chord_for(Scope::Launcher, Action::AddPc) {
                            glyphs::show(ui, &glyphs::chord(chord, false));
                        }
                    });
                }
                1 => {
                    let guide = link_button(ui, "Pairing guide");
                    self.focus_item(&guide, "help-pairing", false);
                    if guide.clicked() {
                        commands.push(Command::OpenGuide("pairing.md"));
                    }
                }
                _ => {
                    ui.label(RichText::new(tr(Action::StreamControls.label())).font(semibold(text::CAPTION)).color(ON_SURFACE_VARIANT));
                    ui.add_space(space::XS);
                    let mut pieces = shortcuts::chord_for(Scope::Stream, Action::StreamControls).map(|c| glyphs::chord(c, false)).unwrap_or_default();
                    if let Some(pad) = shortcuts::pad_in(Scope::Stream).find(|b| b.action == Action::StreamControls) {
                        pieces.push(Piece::Or);
                        pieces.extend(style.stream_press(pad, quit, self.settings.back_opens_menu));
                    }
                    ui.horizontal_wrapped(|ui| {
                        glyphs::show(ui, &pieces);
                    });
                }
            }
        });
    }

    fn help_keyboard(&mut self, ui: &mut Ui, width: f32) {
        let launcher = shortcuts::keys_in(Scope::Launcher);
        let stream = shortcuts::keys_in(Scope::Stream);
        let columns = if width >= TWO_UP { 2 } else { 1 };
        grid(ui, "help-keys", columns, 2, |ui, n| {
            if n == 0 {
                card_title(ui, Icon::Keyboard, tr("In the launcher"));
                for (i, binding) in launcher.iter().enumerate() {
                    let pieces = glyphs::chords(&binding.chords, false);
                    self.keys_line(ui, &format!("help-key-{i}"), tr(binding.action.label()), &pieces, i + 1 == launcher.len());
                }
            } else {
                card_title(ui, Icon::Play, tr("In a stream"));
                // The chords that share Ctrl+Alt+Shift: said once above them, the rows show the last
                // key. Only while some do: the user may have changed every one.
                let (chords, alone): (Vec<&shortcuts::KeyBinding>, Vec<_>) = stream.iter().partition(|b| b.chords.iter().all(|c| c.is_nightjar()));
                if !chords.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(tr("Hold")).font(regular(text::LABEL)).color(ON_SURFACE_VARIANT));
                        let held = Chord::nightjar(shortcuts::Key::Tab);
                        let mut pieces = glyphs::chord(held, false);
                        pieces.truncate(pieces.len() - 2);
                        glyphs::show(ui, &pieces);
                        ui.label(RichText::new(tr("and press:")).font(regular(text::LABEL)).color(ON_SURFACE_VARIANT));
                    });
                    ui.add_space(space::XS);
                }
                for (i, binding) in chords.iter().enumerate() {
                    let pieces = glyphs::chords(&binding.chords, true);
                    let spoken = glyphs::spoken(&glyphs::chords(&binding.chords, false));
                    self.keys_line_spoken(ui, &format!("help-stream-key-{i}"), tr(binding.action.label()), &pieces, &spoken, i + 1 == chords.len() && alone.is_empty());
                }
                if !alone.is_empty() {
                    // Set apart from the held chords above, when there are any.
                    if !chords.is_empty() {
                        ui.add_space(space::S);
                        ui.label(RichText::new(tr("Without Ctrl+Alt+Shift")).font(semibold(text::CAPTION)).color(ON_SURFACE_VARIANT));
                    }
                    for (i, binding) in alone.iter().enumerate() {
                        let pieces = glyphs::chords(&binding.chords, false);
                        self.keys_line(ui, &format!("help-stream-alone-{i}"), tr(binding.action.label()), &pieces, i + 1 == alone.len());
                    }
                }
            }
        });
    }

    fn help_controller(&mut self, ui: &mut Ui, width: f32, view: &View<'_>) {
        let ctx = ui.ctx().clone();
        // Only a choice made here is kept; without one the labels follow the controllers connected
        // now, so a pad plugged in while the page is open changes them.
        let style_id = Id::unique("help-pad-style");
        let mut style = ctx.data(|d| d.get_temp::<Style>(style_id)).unwrap_or_else(|| Style::of(view.pads));
        ui.horizontal(|ui| {
            ui.label(RichText::new(tr("Buttons as on")).font(regular(text::LABEL)).color(ON_SURFACE_VARIANT));
            ui.add_space(space::XS);
            for (n, (choice, name)) in [(Style::Xbox, en("Xbox")), (Style::PlayStation, en("PlayStation")), (Style::Nintendo, en("Nintendo"))].into_iter().enumerate() {
                let response = segment(ui, tr(name), style == choice);
                self.focus_item(&response, &format!("help-style-{n}"), false);
                if response.clicked() {
                    style = choice;
                    ctx.data_mut(|d| d.insert_temp(style_id, choice));
                }
            }
        });
        ui.add_space(space::S);
        let s = &self.settings;
        let (swap, quit, back_opens, mouse) = (s.swap_face_buttons, s.gamepad_quit_combo, s.back_opens_menu, s.controller_mouse);
        // With the Nintendo layout on, the south button goes back and the east one selects.
        let swapped = |b: Button| match (swap, b) {
            (true, Button::South) => Button::East,
            (true, Button::East) => Button::South,
            _ => b,
        };
        let launcher: Vec<(Action, Vec<Piece>)> = shortcuts::pad_in(Scope::Launcher)
            .map(|b| {
                let press = match b.press {
                    Press::Any(buttons) if swap && buttons.iter().any(|b| matches!(b, Button::South | Button::East)) => {
                        let mut pieces = Vec::new();
                        for (n, button) in buttons.iter().enumerate() {
                            if n > 0 { pieces.push(Piece::Or); }
                            pieces.push(style.piece(swapped(*button)));
                        }
                        return (b.action, pieces);
                    }
                    press => press,
                };
                (b.action, style.press(press, quit))
            })
            .collect();
        let stream: Vec<(Action, Vec<Piece>)> = shortcuts::pad_in(Scope::Stream)
            .filter(|b| b.action != Action::ControllerMouse || mouse)
            .map(|b| (b.action, style.stream_press(b, quit, back_opens)))
            .collect();
        let columns = if width >= TWO_UP { 2 } else { 1 };
        grid(ui, "help-pad", columns, 2, |ui, n| {
            let (title, icon, rows, key) = if n == 0 { (tr("In the launcher"), Icon::Controller, &launcher, "help-pad") } else { (tr("In a stream"), Icon::Play, &stream, "help-stream-pad") };
            card_title(ui, icon, title);
            for (i, (action, pieces)) in rows.iter().enumerate() {
                self.keys_line(ui, &format!("{key}-{i}"), tr(action.label()), pieces, i + 1 == rows.len());
            }
        });
    }

    fn help_sharing(&mut self, ui: &mut Ui, width: f32) {
        let tiles: [(Icon, &str, &str); 4] = [
            (Icon::ClipboardUp, en("Clipboard"), en("Send text to the PC or copy the PC's text here from the stream controls, or with the keys below. Clipboard sync in Settings does it as you copy.")),
            (Icon::File, en("Files"), en("Send files to a PC's transfer folder and download from it: Files in the PC's options.")),
            (Icon::Folder, en("Shared folders"), en("Browse the folders a PC shares with this device and download from them, on the PC's details page.")),
            (Icon::Usb, en("USB devices"), en("Attach a USB device plugged in here to the PC from the stream controls, when the PC supports it.")),
        ];
        let columns = if width >= 1000.0 { 4 } else if width >= 560.0 { 2 } else { 1 };
        grid(ui, "help-sharing", columns, tiles.len(), |ui, n| {
            let (icon, title, body) = tiles[n];
            icon_tile(ui, icon);
            ui.add_space(space::S);
            ui.label(RichText::new(tr(title)).font(semibold(text::BODY)).color(ON_SURFACE));
            ui.add_space(space::XS);
            ui.label(RichText::new(tr(body)).font(regular(text::LABEL)).color(ON_SURFACE_VARIANT));
            let keys: &[Action] = match n {
                0 => &[Action::SendClipboard, Action::FetchClipboard],
                1 => &[Action::Options],
                _ => &[],
            };
            for action in keys {
                let scope = if *action == Action::Options { Scope::Launcher } else { Scope::Stream };
                if let Some(chord) = shortcuts::chord_for(scope, *action) {
                    ui.add_space(space::S);
                    ui.label(RichText::new(tr(action.label())).font(semibold(text::CAPTION)).color(ON_SURFACE_VARIANT));
                    ui.add_space(2.0);
                    glyphs::show(ui, &glyphs::chord(chord, false));
                }
            }
        });
    }

    fn help_problems(&mut self, ui: &mut Ui, width: f32, commands: &mut Vec<Command>) {
        let wide = width >= TWO_UP;
        card(ui, ui.available_width(), 0.0, |ui| {
            for (n, (symptom, fix)) in PROBLEMS.iter().enumerate() {
                let rect = ui
                    .scope(|ui| {
                        if wide {
                            ui.horizontal_top(|ui| {
                                let symptom_w = (ui.available_width() * 0.32).max(200.0);
                                ui.allocate_ui(vec2(symptom_w, 0.0), |ui| {
                                    ui.set_width(symptom_w);
                                    ui.label(RichText::new(tr(symptom)).font(semibold(text::BODY)).color(ON_SURFACE));
                                });
                                ui.label(RichText::new(tr(fix)).font(regular(text::LABEL)).color(ON_SURFACE_VARIANT));
                            });
                        } else {
                            ui.label(RichText::new(tr(symptom)).font(semibold(text::BODY)).color(ON_SURFACE));
                            ui.label(RichText::new(tr(fix)).font(regular(text::LABEL)).color(ON_SURFACE_VARIANT));
                        }
                    })
                    .response
                    .rect;
                let rect = rect.expand2(vec2(space::S, space::S / 2.0));
                let response = ui.interact(rect, Id::unique(("help-problem", n)), Sense::focusable_noninteractive());
                response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, format!("{}: {}", tr(symptom), tr(fix))));
                focus_ring(ui, rect, &response, radius::SMALL);
                self.focus_item(&response, &format!("help-problem-{n}"), false);
                ui.add_space(space::S);
                separator(ui);
                ui.add_space(space::S);
            }
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.set_width((ui.available_width() - 200.0).max(160.0));
                    ui.label(RichText::new(tr("Anything else")).font(semibold(text::BODY)).color(ON_SURFACE));
                    ui.label(RichText::new(tr("Attach nightjar.log from the log folder to a bug report.")).font(regular(text::LABEL)).color(ON_SURFACE_VARIANT));
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let logs = quiet_button(ui, "Open log folder");
                    self.focus_item(&logs, "help-logs", false);
                    if logs.clicked() {
                        commands.push(Command::OpenLogs);
                    }
                });
            });
        });
    }

    fn help_links(&mut self, ui: &mut Ui, commands: &mut Vec<Command>) {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(space::S, space::S);
            let links: [(&str, &'static str); 3] = [
                (en("Online guide"), "index.md"),
                (en("Controls guide"), "controls.md#nightjar-for-windows-and-linux"),
                (en("Troubleshooting guide"), "TROUBLESHOOTING.md#nightjar-for-windows-and-linux"),
            ];
            for (n, (name, page)) in links.into_iter().enumerate() {
                let button = quiet_button(ui, name);
                self.focus_item(&button, &format!("help-guide-{n}"), false);
                if button.clicked() {
                    commands.push(Command::OpenGuide(page));
                }
            }
            let notes = quiet_button(ui, "Release notes");
            self.focus_item(&notes, "help-releases", false);
            if notes.clicked() {
                commands.push(Command::OpenWebUi(RELEASES.into()));
            }
        });
        ui.add_space(space::M);
        ui.label(RichText::new(trf("Nightjar {}", &[&crate::VERSION])).font(regular(text::CAPTION)).color(ON_SURFACE_VARIANT));
    }

    /// One shortcut: what it does on the left, its keys or buttons on the right. The row takes
    /// focus (Tab, the arrow keys, the D-pad), so a controller can scroll the list and a screen
    /// reader reads each shortcut whole.
    fn keys_line(&mut self, ui: &mut Ui, key: &str, label: &str, pieces: &[Piece], last: bool) {
        self.keys_line_spoken(ui, key, label, pieces, &glyphs::spoken(pieces), last);
    }

    fn keys_line_spoken(&mut self, ui: &mut Ui, key: &str, label: &str, pieces: &[Piece], spoken: &str, last: bool) {
        let width = ui.available_width();
        let keys_w = glyphs::width(ui, pieces);
        let words = ui.painter().layout(label.to_owned(), regular(text::BODY), *ON_SURFACE, (width - keys_w - space::L - 2.0 * space::S).max(80.0));
        let height = (words.size().y + 2.0 * space::S + 2.0).max(40.0);
        let (rect, response) = ui.allocate_exact_size(vec2(width, height), Sense::focusable_noninteractive());
        ui.painter().galley(pos2(rect.left() + space::S, rect.center().y - words.size().y / 2.0), words, *ON_SURFACE);
        glyphs::paint(ui, pos2(rect.right() - space::S - keys_w, rect.center().y), pieces);
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, format!("{label}: {spoken}")));
        focus_ring(ui, rect, &response, radius::SMALL);
        self.focus_item(&response, key, false);
        if !last {
            separator(ui);
        }
    }
}

/// A hairline between rows, inset like the rows' text.
fn separator(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(rect.shrink2(vec2(space::S, 0.0)), 0.0, design_tokens::TINT_SEPARATOR);
}

/// A card's heading: a small icon and its title.
fn card_title(ui: &mut Ui, icon: Icon, title: &str) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::hover());
        glyphs::icon(ui.painter(), rect, icon, *MOON);
        ui.add_space(space::XS);
        ui.label(RichText::new(title).font(semibold(text::BODY)).color(ON_SURFACE));
    });
    ui.add_space(space::XS);
}

/// The number of a getting-started step in an amber-tinted disc.
fn step_badge(ui: &mut Ui, n: usize) {
    let (rect, _) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), 14.0, *MOON_CONTAINER);
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, n.to_string(), semibold(text::LABEL), *ON_MOON_CONTAINER);
}

/// An icon on a tinted disc.
fn icon_tile(ui: &mut Ui, icon: Icon) {
    let (rect, _) = ui.allocate_exact_size(vec2(36.0, 36.0), Sense::hover());
    let mut shapes = Vec::new();
    glass::control(&mut shapes, ui.ctx(), rect, rect.height() / 2.0);
    ui.painter().extend(shapes);
    glyphs::icon(ui.painter(), rect.shrink(8.0), icon, *ON_SURFACE);
}

/// One choice of a segmented control: the chosen one filled and in full ink.
fn segment(ui: &mut Ui, name: &str, chosen: bool) -> Response {
    let ink = if chosen { *ON_SURFACE } else { *ON_SURFACE_VARIANT };
    let button = egui::Button::new(RichText::new(name).font(semibold(text::CAPTION)).color(ink))
        .fill(if chosen { design_tokens::TINT_SEG_ON_FILL } else { Color32::TRANSPARENT })
        .corner_radius(CornerRadius::same(14))
        .min_size(vec2(0.0, 28.0));
    let response = ui.add(button);
    response.widget_info(|| egui::WidgetInfo::selected(egui::Role::RadioButton, true, chosen, name));
    focus_ring(ui, response.rect, &response, 14);
    response
}

/// A card: the settings rows' tint on the sky, `width` wide, at least `min_height` tall. Returns the
/// height its content took.
fn card(ui: &mut Ui, width: f32, min_height: f32, add: impl FnOnce(&mut Ui)) -> f32 {
    let mut natural = 0.0;
    let place = ui.painter().add(egui::Shape::Noop);
    let shown = Frame::new().inner_margin(Margin::symmetric(CARD_MARGIN as i8, 16)).show(ui, |ui| {
        ui.set_width(width - 2.0 * CARD_MARGIN);
        natural = ui.vertical(add).response.rect.height();
        ui.set_min_height(min_height);
    });
    let mut shapes = Vec::new();
    super::pcs::canvas_on_moon(&mut shapes, ui.ctx(), shown.response.rect, f32::from(radius::PANEL), super::backdrop::Moon::High);
    glass::group(&mut shapes, ui.ctx(), shown.response.rect, f32::from(radius::PANEL), rows_fill());
    ui.painter().set(place, egui::Shape::Vec(shapes));
    natural
}

/// Cards in `columns` columns of equal width; the cards of a row take the tallest one's height
/// (measured the frame before), so their edges line up.
fn grid(ui: &mut Ui, id: &str, columns: usize, count: usize, mut cell: impl FnMut(&mut Ui, usize)) {
    let gap = space::M;
    let width = ui.available_width();
    let cell_w = ((width - gap * (columns - 1) as f32) / columns as f32).floor();
    let key = Id::unique(("help-grid", id, columns));
    let heights: Vec<f32> = ui.ctx().data(|d| d.get_temp(key)).unwrap_or_default();
    let mut measured = Vec::new();
    for row in 0..count.div_ceil(columns) {
        let min_height = heights.get(row).copied().unwrap_or(0.0);
        let mut tallest: f32 = 0.0;
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for column in 0..columns {
                let n = row * columns + column;
                if n >= count {
                    break;
                }
                tallest = tallest.max(card(ui, cell_w, min_height, |ui| cell(ui, n)));
            }
        });
        ui.add_space(gap);
        measured.push(tallest);
    }
    if measured.iter().zip(&heights).any(|(a, b)| (a - b).abs() > 0.5) || measured.len() != heights.len() {
        ui.ctx().data_mut(|d| d.insert_temp(key, measured));
        ui.ctx().request_repaint();
    }
}

#[cfg(test)]
mod tests;
