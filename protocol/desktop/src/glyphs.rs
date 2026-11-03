//! Button names in hints follow the connected controller's layout: Xbox by default, PlayStation
//! symbols or Switch labels when such a controller is attached. Ported from Moonlight V+
//! (streaming/input/gamepadglyphs.h and SdlGamepadKeyNavigation::detectUiStyle).
//!
//! Also how shortcuts are drawn (keycaps and controller buttons, [`Piece`]) and the line icons of
//! the stream controls and the Help page ([`icon`]), all in theme roles.

use egui::{pos2, vec2, Color32, CornerRadius, FontId, Pos2, Rect, Stroke, Ui};
use sdl3_sys::everything::*;

use crate::config::GamepadQuitCombo;
use crate::shortcuts::{Action, Button, Chord, PadBinding, Press};
use crate::strings::tr;
use crate::theme::{radius, regular, semibold, text, ON_SURFACE, ON_SURFACE_VARIANT, OUTLINE, OUTLINE_VARIANT, SURFACE_HIGHEST};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    Xbox,
    PlayStation,
    Nintendo,
}

/// A face button by its position, as SDL names them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Face {
    South,
    East,
    West,
    North,
}

impl Style {
    /// The first PlayStation or Switch controller sets the style; anything else reads as Xbox.
    pub fn of_types(types: impl IntoIterator<Item = SDL_GamepadType>) -> Style {
        types
            .into_iter()
            .find_map(|kind| match kind {
                SDL_GAMEPAD_TYPE_PS3 | SDL_GAMEPAD_TYPE_PS4 | SDL_GAMEPAD_TYPE_PS5 => Some(Style::PlayStation),
                SDL_GAMEPAD_TYPE_NINTENDO_SWITCH_PRO
                | SDL_GAMEPAD_TYPE_NINTENDO_SWITCH_JOYCON_LEFT
                | SDL_GAMEPAD_TYPE_NINTENDO_SWITCH_JOYCON_RIGHT
                | SDL_GAMEPAD_TYPE_NINTENDO_SWITCH_JOYCON_PAIR => Some(Style::Nintendo),
                _ => None,
            })
            .unwrap_or(Style::Xbox)
    }

    /// The style of the controllers attached now.
    pub fn of(pads: &crate::gamepad::Gamepads) -> Style {
        // SAFETY: gamepads the list opened and still holds.
        Style::of_types(pads.pads.iter().map(|pad| unsafe { SDL_GetGamepadType(pad.handle) }))
    }

    /// The label on the face button at `face`: Switch labels follow its layout (south is B).
    pub fn face(self, face: Face) -> &'static str {
        match (self, face) {
            // V+ shows ✕ (U+2715); no font here has it, × (U+00D7) is the same mark.
            (Style::PlayStation, Face::South) => "×",
            (Style::PlayStation, Face::East) => "○",
            (Style::PlayStation, Face::West) => "□",
            (Style::PlayStation, Face::North) => "△",
            (Style::Nintendo, Face::South) => "B",
            (Style::Nintendo, Face::East) => "A",
            (Style::Nintendo, Face::West) => "Y",
            (Style::Nintendo, Face::North) => "X",
            (Style::Xbox, Face::South) => "A",
            (Style::Xbox, Face::East) => "B",
            (Style::Xbox, Face::West) => "X",
            (Style::Xbox, Face::North) => "Y",
        }
    }

    pub fn start(self) -> &'static str {
        match self {
            Style::PlayStation => "Options",
            Style::Nintendo => "+",
            Style::Xbox => "Start",
        }
    }

    pub fn select(self) -> &'static str {
        match self {
            Style::PlayStation => "Share",
            Style::Nintendo => "−",
            Style::Xbox => "Select",
        }
    }

    pub fn left_shoulder(self) -> &'static str {
        match self {
            Style::PlayStation => "L1",
            Style::Nintendo => "L",
            Style::Xbox => "LB",
        }
    }

    pub fn right_shoulder(self) -> &'static str {
        match self {
            Style::PlayStation => "R1",
            Style::Nintendo => "R",
            Style::Xbox => "RB",
        }
    }

    /// A quit combo in this controller's names. Its face buttons are positions (A is south, as
    /// `gamepad::quit_mask` reads them), so a Switch controller's south button shows as B.
    pub fn quit_combo(self, combo: GamepadQuitCombo) -> String {
        let (lb, rb) = (self.left_shoulder(), self.right_shoulder());
        let buttons = match combo {
            GamepadQuitCombo::StartSelectLbRb => [self.start(), self.select(), lb, rb],
            GamepadQuitCombo::SelectLbRbY => [self.select(), lb, rb, self.face(Face::North)],
            GamepadQuitCombo::StartLbRbA => [self.start(), lb, rb, self.face(Face::South)],
            GamepadQuitCombo::StartLbRbB => [self.start(), lb, rb, self.face(Face::East)],
            GamepadQuitCombo::LbRbXY => [lb, rb, self.face(Face::West), self.face(Face::North)],
            GamepadQuitCombo::LbRbAB => [lb, rb, self.face(Face::South), self.face(Face::East)],
        };
        self.chord(&buttons)
    }

    /// Buttons held together: "Start+Select", or "+ + −" where the names are + and −.
    pub fn chord(self, buttons: &[&str]) -> String {
        buttons.join(if self == Style::Nintendo { " + " } else { "+" })
    }

    /// The buttons the controller mouse clicks left and right with: the PC's A and B, which the
    /// Nintendo layout setting moves from south and east to east and south.
    pub fn mouse_clicks(self, swapped: bool) -> (&'static str, &'static str) {
        let (left, right) = if swapped { (Face::East, Face::South) } else { (Face::South, Face::East) };
        (self.face(left), self.face(right))
    }
}

/// One piece of a drawn shortcut: a keycap, a controller button, or the small words between.
#[derive(Clone, Debug, PartialEq)]
pub enum Piece {
    /// A key with its name on it.
    Key(String),
    /// A round face button.
    Face(&'static str),
    /// A pill: a shoulder, Start or Select.
    Pill(&'static str),
    DPad,
    Stick,
    /// "+" between keys held together.
    Plus,
    /// "or" between alternatives.
    Or,
    /// A quiet word before the rest ("Hold").
    Word(String),
}

/// The height of a drawn shortcut.
pub const HEIGHT: f32 = 26.0;

impl Style {
    /// The pieces that draw one controller button in this style.
    pub fn piece(self, button: Button) -> Piece {
        match button {
            Button::South => Piece::Face(self.face(Face::South)),
            Button::East => Piece::Face(self.face(Face::East)),
            Button::West => Piece::Face(self.face(Face::West)),
            Button::North => Piece::Face(self.face(Face::North)),
            Button::Start => Piece::Pill(self.start()),
            Button::Back => Piece::Pill(self.select()),
            Button::LeftShoulder => Piece::Pill(self.left_shoulder()),
            Button::RightShoulder => Piece::Pill(self.right_shoulder()),
            Button::DPad => Piece::DPad,
            Button::LeftStick => Piece::Stick,
        }
    }

    /// A press's pieces: the buttons with "+" between those held together and "or" between
    /// alternatives. `quit` is the quit combo the settings chose.
    pub fn press(self, press: Press, quit: GamepadQuitCombo) -> Vec<Piece> {
        let together = |buttons: &[Button]| -> Vec<Piece> {
            let mut pieces = Vec::new();
            for (n, button) in buttons.iter().enumerate() {
                if n > 0 { pieces.push(Piece::Plus); }
                pieces.push(self.piece(*button));
            }
            pieces
        };
        match press {
            Press::Any(buttons) => {
                let mut pieces = Vec::new();
                for (n, button) in buttons.iter().enumerate() {
                    if n > 0 { pieces.push(Piece::Or); }
                    pieces.push(self.piece(*button));
                }
                pieces
            }
            Press::Together(buttons) => together(buttons),
            Press::Hold(button) => vec![Piece::Word(tr("Hold").to_owned()), self.piece(button)],
            Press::QuitCombo => together(&crate::shortcuts::quit_buttons(quit)),
        }
    }

    /// A stream binding's pieces: its press and, with "Back opens stream controls" on
    /// (`back_opens`), Select besides for the stream controls. The shortcut sheet and the Help
    /// page both list it so.
    pub fn stream_press(self, binding: &PadBinding, quit: GamepadQuitCombo, back_opens: bool) -> Vec<Piece> {
        let mut pieces = self.press(binding.press, quit);
        if binding.action == Action::StreamControls && back_opens {
            pieces.push(Piece::Or);
            pieces.push(self.piece(Button::Back));
        }
        pieces
    }

    /// A press as a screen reader says it: "Start+Select", "Hold Start", "D-pad or Left stick".
    /// The Help page's tests read the page through it.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn press_text(self, press: Press, quit: GamepadQuitCombo) -> String {
        spoken(&self.press(press, quit))
    }
}

/// A chord's pieces; `without_nightjar_keys` leaves out Ctrl+Alt+Shift where a heading says them once.
pub fn chord(chord: Chord, without_nightjar_keys: bool) -> Vec<Piece> {
    let caps = chord.caps();
    let skip = if without_nightjar_keys && chord.is_nightjar() { caps.len() - 1 } else { 0 };
    let mut pieces = Vec::new();
    for cap in &caps[skip..] {
        if !pieces.is_empty() { pieces.push(Piece::Plus); }
        pieces.push(Piece::Key(tr(cap.as_ref()).to_owned()));
    }
    pieces
}

/// Alternative chords, "or" between them.
pub fn chords(list: &[Chord], without_nightjar_keys: bool) -> Vec<Piece> {
    let mut pieces = Vec::new();
    for c in list {
        if !pieces.is_empty() { pieces.push(Piece::Or); }
        pieces.extend(chord(*c, without_nightjar_keys));
    }
    pieces
}

/// Pieces as words, for screen readers and tests: "Ctrl+N or F2", "Hold Start".
pub fn spoken(pieces: &[Piece]) -> String {
    let mut text = String::new();
    for piece in pieces {
        let word = match piece {
            Piece::Plus => {
                text.push('+');
                continue;
            }
            Piece::Or => tr("or"),
            Piece::Key(name) | Piece::Word(name) => name.as_str(),
            Piece::Face(name) | Piece::Pill(name) => name,
            Piece::DPad => tr("D-pad"),
            Piece::Stick => tr("Left stick"),
        };
        if !text.is_empty() && !text.ends_with('+') {
            text.push(' ');
        }
        text.push_str(word);
    }
    text
}

fn galley(ui: &Ui, text: &str, font: FontId, color: Color32) -> std::sync::Arc<egui::Galley> {
    ui.painter().layout_no_wrap(text.to_owned(), font, color)
}

/// How wide a piece is drawn, and its galley.
fn measure(ui: &Ui, piece: &Piece) -> (f32, Option<std::sync::Arc<egui::Galley>>) {
    let cap = semibold(text::CAPTION);
    match piece {
        Piece::Key(name) => {
            let g = galley(ui, name, cap, *ON_SURFACE);
            ((g.size().x + 16.0).max(HEIGHT), Some(g))
        }
        Piece::Face(name) => (HEIGHT, Some(galley(ui, name, cap, *ON_SURFACE))),
        Piece::Pill(name) => {
            let g = galley(ui, name, semibold(12.0), *ON_SURFACE);
            ((g.size().x + 18.0).max(HEIGHT + 8.0), Some(g))
        }
        Piece::DPad | Piece::Stick => (HEIGHT, None),
        Piece::Plus => {
            let g = galley(ui, "+", regular(text::CAPTION), *ON_SURFACE_VARIANT);
            (g.size().x + 8.0, Some(g))
        }
        Piece::Or => {
            let g = galley(ui, tr("or"), regular(text::CAPTION), *ON_SURFACE_VARIANT);
            (g.size().x + 16.0, Some(g))
        }
        Piece::Word(word) => {
            let g = galley(ui, word, regular(text::CAPTION), *ON_SURFACE_VARIANT);
            (g.size().x + 8.0, Some(g))
        }
    }
}

/// How wide `pieces` are drawn.
pub fn width(ui: &Ui, pieces: &[Piece]) -> f32 {
    pieces.iter().map(|p| measure(ui, p).0).sum()
}

/// Draws `pieces` left to right from `left`, centred on its height.
pub fn paint(ui: &Ui, left: Pos2, pieces: &[Piece]) {
    let painter = ui.painter();
    let mut x = left.x;
    let mid = left.y;
    for piece in pieces {
        let (w, g) = measure(ui, piece);
        let rect = Rect::from_min_size(pos2(x, mid - HEIGHT / 2.0), vec2(w, HEIGHT));
        let centred = |g: &std::sync::Arc<egui::Galley>, at: Pos2| at - g.size() / 2.0;
        match piece {
            Piece::Key(_) => {
                // A keycap: a darker edge under a raised top.
                painter.rect_filled(rect, CornerRadius::same(radius::SMALL), *OUTLINE_VARIANT);
                let top = Rect::from_min_max(rect.min + vec2(1.0, 1.0), rect.max - vec2(1.0, 3.0));
                painter.rect_filled(top, CornerRadius::same(radius::SMALL - 1), *SURFACE_HIGHEST);
                if let Some(g) = g { painter.galley(centred(&g, top.center()), g, *ON_SURFACE); }
            }
            Piece::Face(_) => {
                painter.circle_filled(rect.center(), HEIGHT / 2.0, *SURFACE_HIGHEST);
                painter.circle_stroke(rect.center(), HEIGHT / 2.0 - 0.5, Stroke::new(1.0, *OUTLINE_VARIANT));
                if let Some(g) = g { painter.galley(centred(&g, rect.center()), g, *ON_SURFACE); }
            }
            Piece::Pill(_) => {
                let pill = rect.shrink2(vec2(0.0, 2.0));
                painter.rect_filled(pill, CornerRadius::same((pill.height() / 2.0) as u8), *SURFACE_HIGHEST);
                painter.rect_stroke(pill, CornerRadius::same((pill.height() / 2.0) as u8), Stroke::new(1.0, *OUTLINE_VARIANT), egui::StrokeKind::Inside);
                if let Some(g) = g { painter.galley(centred(&g, pill.center()), g, *ON_SURFACE); }
            }
            Piece::DPad => {
                // A plus of four arms, the way a D-pad looks from above.
                let c = rect.center();
                let (long, short) = (HEIGHT / 2.0 - 1.0, 4.5);
                for r in [Rect::from_center_size(c, vec2(2.0 * long, 2.0 * short)), Rect::from_center_size(c, vec2(2.0 * short, 2.0 * long))] {
                    painter.rect_filled(r, CornerRadius::same(2), *SURFACE_HIGHEST);
                    painter.rect_stroke(r, CornerRadius::same(2), Stroke::new(1.0, *OUTLINE), egui::StrokeKind::Inside);
                }
                painter.rect_filled(Rect::from_center_size(c, vec2(2.0 * short - 2.0, 2.0 * short - 2.0)), 0.0, *SURFACE_HIGHEST);
            }
            Piece::Stick => {
                painter.circle_filled(rect.center(), HEIGHT / 2.0, *SURFACE_HIGHEST);
                painter.circle_stroke(rect.center(), HEIGHT / 2.0 - 0.5, Stroke::new(1.0, *OUTLINE_VARIANT));
                painter.circle_stroke(rect.center(), HEIGHT / 4.0, Stroke::new(1.5, *OUTLINE));
            }
            Piece::Plus | Piece::Or | Piece::Word(_) => {
                if let Some(g) = g { painter.galley(centred(&g, rect.center()), g, *ON_SURFACE_VARIANT); }
            }
        }
        x += w;
    }
}

/// Allocates room for `pieces` where the layout puts the next widget, and draws them there.
pub fn show(ui: &mut Ui, pieces: &[Piece]) -> Rect {
    let (rect, _) = ui.allocate_exact_size(vec2(width(ui, pieces), HEIGHT), egui::Sense::hover());
    paint(ui, rect.left_center(), pieces);
    rect
}

/// Line icons for the stream controls and the Help page, drawn with the painter (Figtree has no
/// symbols) in one stroke weight on an 18-point grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Play,
    Overlay,
    Corner,
    Screen,
    Microphone,
    Mouse,
    Speaker,
    Download,
    ClipboardUp,
    ClipboardDown,
    Image,
    Keyboard,
    Gauge,
    Pulse,
    Sliders,
    Help,
    Reconnect,
    Power,
    Usb,
    Terminal,
    Controller,
    Folder,
    File,
    Plus,
    More,
    Back,
}

/// Draws `icon` centred in `rect` in `color`.
pub fn icon(painter: &egui::Painter, rect: Rect, icon: Icon, color: Color32) {
    let c = rect.center();
    let s = rect.width().min(rect.height()) / 18.0;
    let p = |x: f32, y: f32| c + vec2(x, y) * s;
    let stroke = Stroke::new(1.6 * s.max(1.0), color);
    let line = |points: &[Pos2]| painter.add(egui::Shape::line(points.to_vec(), stroke));
    let r = |x0: f32, y0: f32, x1: f32, y1: f32| Rect::from_min_max(p(x0, y0), p(x1, y1));
    let round = |rect: Rect, radius: f32| painter.rect_stroke(rect, CornerRadius::same((radius * s) as u8), stroke, egui::StrokeKind::Middle);
    let arc = |center: Pos2, radius: f32, from: f32, to: f32| {
        let n = 20;
        let points: Vec<Pos2> = (0..=n).map(|i| {
            let a = from + (to - from) * i as f32 / n as f32;
            center + vec2(a.cos(), a.sin()) * radius * s
        }).collect();
        painter.add(egui::Shape::line(points, stroke));
    };
    use std::f32::consts::{FRAC_PI_2, PI};
    match icon {
        Icon::Play => {
            painter.add(egui::Shape::convex_polygon(vec![p(-4.0, -6.5), p(6.5, 0.0), p(-4.0, 6.5)], color, Stroke::NONE));
        }
        Icon::Overlay => {
            round(r(-7.5, -6.5, 7.5, 6.5), 2.0);
            line(&[p(-4.0, 3.0), p(-4.0, 0.0)]);
            line(&[p(0.0, 3.0), p(0.0, -3.0)]);
            line(&[p(4.0, 3.0), p(4.0, 1.0)]);
        }
        Icon::Corner => {
            round(r(-7.5, -6.5, 7.5, 6.5), 2.0);
            painter.rect_filled(r(-5.0, -4.0, -1.0, -1.0), CornerRadius::same(1), color);
        }
        Icon::Screen => {
            round(r(-7.5, -6.0, 7.5, 4.0), 2.0);
            line(&[p(-3.0, 7.0), p(3.0, 7.0)]);
            line(&[p(0.0, 4.0), p(0.0, 7.0)]);
        }
        Icon::Microphone => {
            round(r(-2.5, -7.5, 2.5, 2.0), 2.5);
            arc(p(0.0, -1.0), 5.5, 0.0, PI);
            line(&[p(0.0, 4.5), p(0.0, 7.5)]);
        }
        Icon::Mouse => {
            round(r(-4.5, -7.5, 4.5, 7.5), 4.5);
            line(&[p(0.0, -7.0), p(0.0, -2.5)]);
        }
        Icon::Speaker => {
            painter.add(egui::Shape::convex_polygon(vec![p(-7.0, -2.5), p(-4.0, -2.5), p(0.5, -6.5), p(0.5, 6.5), p(-4.0, 2.5), p(-7.0, 2.5)], Color32::TRANSPARENT, stroke));
            arc(p(1.0, 0.0), 3.5, -0.9, 0.9);
            arc(p(1.0, 0.0), 6.5, -0.9, 0.9);
        }
        Icon::Download => {
            line(&[p(0.0, -7.0), p(0.0, 3.0)]);
            line(&[p(-4.0, -1.0), p(0.0, 3.0), p(4.0, -1.0)]);
            line(&[p(-7.0, 7.0), p(7.0, 7.0)]);
        }
        Icon::ClipboardUp | Icon::ClipboardDown => {
            round(r(-6.0, -6.0, 6.0, 7.5), 2.0);
            painter.rect_filled(r(-3.0, -8.0, 3.0, -4.5), CornerRadius::same(1), color);
            if icon == Icon::ClipboardUp {
                line(&[p(0.0, 4.5), p(0.0, -1.5)]);
                line(&[p(-2.5, 1.0), p(0.0, -1.5), p(2.5, 1.0)]);
            } else {
                line(&[p(0.0, -1.5), p(0.0, 4.5)]);
                line(&[p(-2.5, 2.0), p(0.0, 4.5), p(2.5, 2.0)]);
            }
        }
        Icon::Image => {
            round(r(-7.5, -6.0, 7.5, 6.0), 2.0);
            line(&[p(-6.5, 4.5), p(-2.0, 0.0), p(1.5, 3.0), p(3.5, 1.0), p(6.5, 4.0)]);
            painter.circle_filled(p(3.0, -2.5), 1.5 * s, color);
        }
        Icon::Keyboard => {
            round(r(-8.0, -5.5, 8.0, 5.5), 2.0);
            for x in [-4.5, -1.5, 1.5, 4.5] {
                painter.circle_filled(p(x, -2.0), 0.9 * s, color);
            }
            line(&[p(-4.0, 2.0), p(4.0, 2.0)]);
        }
        Icon::Gauge => {
            arc(p(0.0, 3.0), 7.5, PI, 2.0 * PI);
            line(&[p(0.0, 3.0), p(4.0, -2.5)]);
            line(&[p(-7.5, 3.0), p(-7.5, 5.0)]);
            line(&[p(7.5, 3.0), p(7.5, 5.0)]);
        }
        Icon::Pulse => {
            line(&[p(-8.0, 0.0), p(-4.0, 0.0), p(-2.0, -5.5), p(1.5, 5.5), p(3.5, 0.0), p(8.0, 0.0)]);
        }
        Icon::Sliders => {
            for (y, knob) in [(-5.0, 2.5), (0.0, -3.0), (5.0, 1.0)] {
                line(&[p(-7.5, y), p(7.5, y)]);
                painter.circle_filled(p(knob, y), 2.2 * s, color);
            }
        }
        Icon::Help => {
            painter.circle_stroke(c, 7.5 * s, stroke);
            arc(p(0.0, -2.0), 2.6, PI, 2.0 * PI + 0.6);
            line(&[p(2.1, -0.4), p(0.0, 1.5), p(0.0, 2.5)]);
            painter.circle_filled(p(0.0, 5.0), 1.1 * s, color);
        }
        Icon::Reconnect => {
            // A clockwise arrow round a circle, its head at the top: refresh, not power.
            arc(c, 6.5, -FRAC_PI_2 + 0.9, 1.5 * PI - 0.15);
            painter.add(egui::Shape::convex_polygon(vec![p(3.2, -6.5), p(-1.0, -9.6), p(-1.0, -3.4)], color, Stroke::NONE));
        }
        Icon::Power => {
            arc(c, 6.5, -FRAC_PI_2 + 0.75, 1.5 * PI - 0.75);
            line(&[p(0.0, -8.0), p(0.0, -1.0)]);
        }
        Icon::Usb => {
            line(&[p(0.0, 7.5), p(0.0, -6.0)]);
            line(&[p(-2.5, -4.0), p(0.0, -7.0), p(2.5, -4.0)]);
            line(&[p(0.0, 3.0), p(-4.5, 0.0), p(-4.5, -2.5)]);
            line(&[p(0.0, 1.0), p(4.5, -1.5), p(4.5, -3.5)]);
            painter.circle_filled(p(0.0, 7.5), 1.8 * s, color);
        }
        Icon::Terminal => {
            round(r(-8.0, -6.5, 8.0, 6.5), 2.0);
            line(&[p(-4.5, -2.5), p(-1.5, 0.0), p(-4.5, 2.5)]);
            line(&[p(0.5, 3.0), p(4.5, 3.0)]);
        }
        Icon::Controller => {
            round(r(-8.0, -5.0, 8.0, 5.0), 5.0);
            line(&[p(-5.5, 0.0), p(-2.5, 0.0)]);
            line(&[p(-4.0, -1.5), p(-4.0, 1.5)]);
            painter.circle_filled(p(3.0, -1.2), 1.1 * s, color);
            painter.circle_filled(p(5.2, 1.2), 1.1 * s, color);
        }
        Icon::Folder => {
            painter.add(egui::Shape::line(vec![p(-8.0, 6.5), p(-8.0, -6.0), p(-3.0, -6.0), p(-1.0, -3.5), p(8.0, -3.5), p(8.0, 6.5), p(-8.0, 6.5)], stroke));
        }
        Icon::File => {
            painter.add(egui::Shape::line(vec![p(-5.5, -7.5), p(1.5, -7.5), p(5.5, -3.5), p(5.5, 7.5), p(-5.5, 7.5), p(-5.5, -7.5)], stroke));
            line(&[p(1.5, -7.5), p(1.5, -3.5), p(5.5, -3.5)]);
        }
        Icon::Plus => {
            line(&[p(-6.5, 0.0), p(6.5, 0.0)]);
            line(&[p(0.0, -6.5), p(0.0, 6.5)]);
        }
        Icon::Back => {
            line(&[p(6.5, 0.0), p(-6.0, 0.0)]);
            line(&[p(-1.0, -5.0), p(-6.0, 0.0), p(-1.0, 5.0)]);
        }
        Icon::More => {
            for x in [-5.5, 0.0, 5.5] {
                painter.circle_filled(p(x, 0.0), 1.6 * s, color);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_playstation_or_switch_controller_sets_the_style() {
        assert_eq!(Style::of_types([]), Style::Xbox);
        assert_eq!(Style::of_types([SDL_GAMEPAD_TYPE_XBOXONE, SDL_GAMEPAD_TYPE_STANDARD]), Style::Xbox);
        assert_eq!(Style::of_types([SDL_GAMEPAD_TYPE_XBOX360, SDL_GAMEPAD_TYPE_PS5]), Style::PlayStation);
        assert_eq!(Style::of_types([SDL_GAMEPAD_TYPE_PS4]), Style::PlayStation);
        assert_eq!(Style::of_types([SDL_GAMEPAD_TYPE_NINTENDO_SWITCH_JOYCON_PAIR, SDL_GAMEPAD_TYPE_PS3]), Style::Nintendo);
        assert_eq!(Style::of_types([SDL_GAMEPAD_TYPE_NINTENDO_SWITCH_PRO]), Style::Nintendo);
    }

    #[test]
    fn labels_follow_each_layout() {
        let west = [Style::Xbox, Style::PlayStation, Style::Nintendo].map(|s| s.face(Face::West));
        assert_eq!(west, ["X", "□", "Y"]);
        let south = [Style::Xbox, Style::PlayStation, Style::Nintendo].map(|s| s.face(Face::South));
        assert_eq!(south, ["A", "×", "B"]);
        assert_eq!(Style::PlayStation.face(Face::East), "○");
        assert_eq!(Style::PlayStation.face(Face::North), "△");
        assert_eq!((Style::Nintendo.start(), Style::Nintendo.select()), ("+", "−"));
    }

    #[test]
    fn quit_combos_and_mouse_clicks_name_the_buttons_where_they_are() {
        let xbox = GamepadQuitCombo::ALL.map(|combo| Style::Xbox.quit_combo(combo));
        assert_eq!(xbox, ["Start+Select+LB+RB", "Select+LB+RB+Y", "Start+LB+RB+A", "Start+LB+RB+B", "LB+RB+X+Y", "LB+RB+A+B"]);
        assert_eq!(Style::PlayStation.quit_combo(GamepadQuitCombo::StartSelectLbRb), "Options+Share+L1+R1");
        assert_eq!(Style::PlayStation.quit_combo(GamepadQuitCombo::LbRbXY), "L1+R1+□+△");
        // The south button of a Switch controller is its B.
        assert_eq!(Style::Nintendo.quit_combo(GamepadQuitCombo::StartLbRbA), "+ + L + R + B");
        assert_eq!(Style::Nintendo.chord(&[Style::Nintendo.start(), Style::Nintendo.select()]), "+ + −");
        assert_eq!(Style::Xbox.mouse_clicks(false), ("A", "B"));
        assert_eq!(Style::Xbox.mouse_clicks(true), ("B", "A"));
        assert_eq!(Style::PlayStation.mouse_clicks(false), ("×", "○"));
        // With the Nintendo layout on, a Switch controller clicks with A (east) and B (south).
        assert_eq!(Style::Nintendo.mouse_clicks(true), ("A", "B"));
    }

    #[test]
    fn the_fonts_draw_every_label() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        // The fonts load with the first frame; its textures are let go like the other tests do.
        ctx.run_ui(egui::RawInput::default(), |_| {}).textures_delta.clear();
        for style in [Style::Xbox, Style::PlayStation, Style::Nintendo] {
            let labels = [Face::South, Face::East, Face::West, Face::North].map(|f| style.face(f)).join(" ")
                + &[style.start(), style.select(), style.left_shoulder(), style.right_shoulder()].join(" ");
            for font in [crate::theme::regular(12.0), crate::theme::semibold(12.0)] {
                let missing: String = labels.chars().filter(|c| !ctx.fonts_mut(|f| f.has_glyph(&font, *c))).collect();
                assert!(missing.is_empty(), "{missing:?} of {labels:?} missing in {font:?}");
            }
        }
    }
}
