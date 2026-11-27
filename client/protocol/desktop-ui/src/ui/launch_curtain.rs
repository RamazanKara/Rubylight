//! The launch curtain over a stream: while a game starts on the PC, the launch scene's calm picture
//! stands over the stream's video (the game's cover with the one thin ring turning around it, the
//! game, its PC and one stage word) instead of the PC's desktop, store windows and splash screens.
//! When a launcher or Windows wants the player, the stage word gives way to a card that says what
//! (`nightjar_policy::launch_curtain::Attention`). Show PC screen, once the picture runs, and
//! Cancel sit under the words. The app decides when the curtain is down and what a press does
//! (`nightjar_policy::launch_curtain::Curtain`); this draws it in the layer it is given, opaque
//! from edge to edge, under the Control Center and the toasts. Night-locked like the stream's HUD,
//! since it draws over video.

use std::sync::Arc;

use egui::epaint::{Mesh, Vertex, WHITE_UV};
use egui::Galley;
use nightjar_policy::launch_curtain::{Attention, View, HINT, SHOW_SCREEN};

use super::design_tokens as t;
use super::launch_scene::{cover_rect, curtain_words, turning_ring, words_column, COVER_AT, GLOW_BLUR, SHADOW_BLUR, SHADOW_Y};
use super::*;
use crate::strings::fill;

/// The attention card's round and the room inside it.
const CARD_RADIUS: f32 = 16.0;
const CARD_PAD: f32 = 16.0;
/// The least widths of Show PC screen and Cancel.
const SHOW_LEAST: f32 = 220.0;
const CANCEL_LEAST: f32 = 140.0;

/// What the curtain names and shows.
pub struct Scene<'a> {
    /// The game, or empty for a stream without one.
    pub game: &'a str,
    /// The PC.
    pub pc: &'a str,
    /// The PC and game its cover is kept under (`Host::art`).
    pub host_id: u64,
    pub app_id: i32,
    /// The cover, when the launcher has it; the game's tile otherwise.
    pub art: Option<&'a Art>,
    pub view: &'a View,
}

impl Scene<'_> {
    /// What the cover's tile and the first line name: the game, else the PC.
    fn title(&self) -> &str {
        if self.game.trim().is_empty() { self.pc } else { self.game }
    }
}

/// A button the player pressed on the curtain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pressed {
    ShowScreen,
    Cancel,
}

/// The card's line in the user's language, with the launcher's name in it.
pub fn attention_line(attention: &Attention) -> String {
    let line = curtain_words(attention.words);
    match attention.launcher {
        Some(name) => fill(line, &[&name]),
        None => line.to_owned(),
    }
}

/// The stage word under the game's name: the host's, or the connection's own before its first.
pub fn stage_word(view: &View) -> &'static str {
    view.stage.map_or_else(|| tr("Connecting"), curtain_words)
}

/// The curtain over the whole window, painted in `ui`'s layer; the button pressed this frame.
pub fn show(ui: &mut Ui, scene: &Scene<'_>) -> Option<Pressed> {
    let ctx = ui.ctx().clone();
    let screen = ctx.content_rect();
    let cover = cover_rect(screen, COVER_AT);
    let mut out = vec![backdrop(screen)];
    turning_ring(&mut out, &ctx, cover);
    glass::cover_glow(&mut out, &ctx, cover, t::RADIUS_COVER, shade::SKY_GLOW.get(), GLOW_BLUR, SHADOW_Y, SHADOW_BLUR);
    let cover_id = Id::unique(("curtain-cover", scene.host_id, scene.app_id));
    cover_shapes(ui, &mut out, cover, cover.width(), t::RADIUS_COVER, cover_id, scene.title(), scene.art, 1.0);
    ui.painter().extend(out);
    words(ui, screen, cover, scene)
}

/// The curtain's own sky, opaque from edge to edge so nothing of the picture shows through: the
/// launch scene's stage colours, darkening downwards.
fn backdrop(screen: Rect) -> egui::Shape {
    let rect = screen.expand(2.0);
    let opaque = |c: Color32| Color32::from_rgb(c.r(), c.g(), c.b());
    let (top, bottom) = (opaque(shade::SKY_TOP.get()), opaque(shade::SKY_BOTTOM.get()));
    let mut mesh = Mesh::default();
    for (pos, color) in [(rect.left_top(), top), (rect.right_top(), top), (rect.right_bottom(), bottom), (rect.left_bottom(), bottom)] {
        mesh.vertices.push(Vertex { pos, uv: WHITE_UV, color });
    }
    mesh.indices.extend([0, 1, 2, 0, 2, 3]);
    egui::Shape::mesh(mesh)
}

/// Beside the cover: the game, its PC, the stage word or the attention card, and the buttons.
fn words(ui: &mut Ui, screen: Rect, cover: Rect, scene: &Scene<'_>) -> Option<Pressed> {
    let ctx = ui.ctx().clone();
    let (middle, width) = words_column(screen);
    let lay = |text: &str, font: egui::FontId, ink: Color32, rows: usize, wrap: f32| {
        let mut job = egui::text::LayoutJob::simple(text.to_owned(), font, ink, wrap);
        job.wrap.max_rows = rows;
        job.halign = Align::Center;
        ui.painter().layout_job(job)
    };
    let title = lay(scene.title(), semibold(text::LARGE_TITLE), *ON_SURFACE, 2, width);
    let pc = (!scene.game.trim().is_empty() && !scene.pc.is_empty()).then(|| lay(scene.pc, regular(text::SECTION), *ON_SURFACE_VARIANT, 1, width));
    let card = scene.view.attention.as_ref();
    let inner = width - CARD_PAD * 2.0;
    let lines: Vec<Arc<Galley>> = match card {
        None => vec![lay(stage_word(scene.view), regular(text::BODY), *ON_SURFACE, 2, width)],
        Some(attention) => [
            Some(lay(&attention_line(attention), semibold(text::SECTION), *ON_SURFACE, 3, inner)),
            (!attention.title.is_empty()).then(|| lay(&attention.title, regular(text::LABEL), *ON_SURFACE_VARIANT, 2, inner)),
            Some(lay(curtain_words(HINT), regular(text::BODY), *ON_SURFACE_VARIANT, 4, inner)),
        ]
        .into_iter()
        .flatten()
        .collect(),
    };
    let lines_h = lines.iter().map(|g| g.size().y).sum::<f32>() + space::S * (lines.len().saturating_sub(1)) as f32;
    let block_h = lines_h + if card.is_some() { CARD_PAD * 2.0 } else { 0.0 };
    let buttons = 1 + usize::from(scene.view.dismissible);
    let buttons_h = Size::CONTROL.height * buttons as f32 + space::M * (buttons - 1) as f32;
    let pc_h = pc.as_ref().map_or(0.0, |g| space::S + g.size().y);
    let height = title.size().y + pc_h + space::XL + block_h + space::XL + buttons_h;
    let mut y = (cover.center().y - height / 2.0).max(screen.top() + space::L);
    let painter = ui.painter().clone();
    let title_h = title.size().y;
    painter.galley(pos2(middle, y), title, *ON_SURFACE);
    y += title_h;
    if let Some(pc) = pc {
        y += space::S;
        let h = pc.size().y;
        painter.galley(pos2(middle, y), pc, *ON_SURFACE_VARIANT);
        y += h;
    }
    y += space::XL;
    if card.is_some() {
        let mut shapes = Vec::new();
        let rect = Rect::from_x_y_ranges(middle - width / 2.0..=middle + width / 2.0, y..=y + block_h);
        glass::group(&mut shapes, &ctx, rect, CARD_RADIUS, group_fill());
        painter.extend(shapes);
        y += CARD_PAD;
    }
    for (n, line) in lines.into_iter().enumerate() {
        if n > 0 {
            y += space::S;
        }
        let h = line.size().y;
        painter.galley(pos2(middle, y), line, *ON_SURFACE);
        y += h;
    }
    if card.is_some() {
        y += CARD_PAD;
    }
    y += space::XL;
    let row = Rect::from_x_y_ranges(middle - width / 2.0..=middle + width / 2.0, y..=y + buttons_h);
    let mut pressed = None;
    ui.scope_builder(UiBuilder::new().max_rect(row).layout(Layout::top_down(Align::Center)), |ui| {
        ui.spacing_mut().item_spacing.y = space::M;
        if scene.view.dismissible
            && button_at_least(ui, Some(Id::unique("curtain-show-screen")), None, curtain_words(SHOW_SCREEN), None, Tone::Primary, Size::CONTROL, SHOW_LEAST).clicked()
        {
            pressed = Some(Pressed::ShowScreen);
        }
        if button_at_least(ui, Some(Id::unique("curtain-cancel")), None, tr("Cancel"), None, Tone::Quiet, Size::CONTROL, CANCEL_LEAST).clicked() {
            pressed = Some(Pressed::Cancel);
        }
    });
    pressed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strings::{with_lang, Lang};
    use crate::theme::{self, Look};
    use egui::{Event, PointerButton, Shape};
    use nightjar_policy::launch_curtain::{OPENING, SIGN_IN, UAC};

    struct Drawn {
        texts: Vec<String>,
        pressed: Option<Pressed>,
        canvas: crate::shot::Canvas,
    }

    /// The curtain over a white frame for a few frames at 1280×800, as the stream's overlay draws
    /// it; `click` names a button to press and release on the way.
    fn draw(view: &View, click: Option<&str>) -> Drawn {
        theme::own_look(Look::Night);
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        theme::set_over_video(&ctx, true);
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 800.0));
        let scene = Scene { game: "Open World", pc: "Living Room PC", host_id: 7, app_id: 3, art: None, view };
        let (mut target, mut pressed) = (None, None);
        let (mut shapes, mut textures) = (Vec::new(), egui::TexturesDelta::default());
        for frame in 0..6 {
            let button = |down| Event::PointerButton { pos: target.unwrap_or_default(), button: PointerButton::Primary, pressed: down, modifiers: Default::default() };
            let events = match (target, frame) {
                (Some(at), 3) => vec![Event::PointerMoved(at), button(true)],
                (Some(_), 4) => vec![button(false)],
                _ => Vec::new(),
            };
            let input = egui::RawInput { screen_rect: Some(screen), events, time: Some(f64::from(frame) / 60.0), ..Default::default() };
            let output = with_lang(Lang::En, || {
                ctx.run_ui(input, |ui| {
                    ui.painter().rect_filled(screen, 0, Color32::WHITE);
                    pressed = pressed.or(show(ui, &scene));
                })
            });
            if target.is_none() && let Some(id) = click {
                target = ctx.read_response(Id::unique(id)).map(|r| r.rect.center());
            }
            textures.append(output.textures_delta);
            shapes = output.shapes;
        }
        let mut texts = Vec::new();
        fn walk(shape: &Shape, out: &mut Vec<String>) {
            match shape {
                Shape::Vec(list) => list.iter().for_each(|shape| walk(shape, out)),
                Shape::Text(text) => out.push(text.galley.text().to_owned()),
                _ => {}
            }
        }
        shapes.iter().for_each(|clipped| walk(&clipped.shape, &mut texts));
        let primitives = ctx.tessellate(shapes, 1.0);
        let mut canvas = crate::shot::Canvas::new(1280, 800);
        canvas.update_textures(&textures);
        canvas.paint(&primitives, 1.0);
        textures.clear();
        theme::own_palette();
        Drawn { texts, pressed, canvas }
    }

    fn has(drawn: &Drawn, text: &str) -> bool {
        drawn.texts.iter().any(|t| t == text)
    }

    #[test]
    fn the_curtain_hides_the_picture_and_names_the_game_its_pc_and_the_stage() {
        let view = View { stage: Some(OPENING), attention: None, dismissible: true };
        let drawn = draw(&view, None);
        for text in ["Open World", "Living Room PC", "Opening window", "Show PC screen", "Cancel"] {
            assert!(has(&drawn, text), "{text} in {:?}", drawn.texts);
        }
        // The white frame under it shows nowhere: not at a corner, nor between cover and words.
        for (x, y) in [(2, 2), (1277, 797), (640, 40), (900, 790)] {
            let seen = drawn.canvas.pixel(x, y);
            assert!(seen[..3].iter().all(|c| *c < 0.3) && seen[3] > 0.99, "dark and opaque at {x},{y}: {seen:?}");
        }
    }

    #[test]
    fn show_pc_screen_waits_for_the_picture_and_cancel_is_always_there() {
        let view = View { stage: None, attention: None, dismissible: false };
        let drawn = draw(&view, Some("curtain-cancel"));
        assert!(has(&drawn, "Connecting") && !has(&drawn, "Show PC screen"), "{:?}", drawn.texts);
        assert_eq!(drawn.pressed, Some(Pressed::Cancel));

        let view = View { stage: Some(OPENING), attention: None, dismissible: true };
        assert_eq!(draw(&view, Some("curtain-show-screen")).pressed, Some(Pressed::ShowScreen));
        assert_eq!(draw(&view, None).pressed, None, "nothing is pressed without a click");
    }

    #[test]
    fn the_card_says_who_wants_the_player_with_the_window_and_the_hint() {
        let attention = Attention { words: SIGN_IN, launcher: Some("Steam"), title: "Steam Guard - Computer Authorization Required".into() };
        let view = View { stage: Some(nightjar_policy::launch_curtain::STARTING), attention: Some(attention), dismissible: true };
        let drawn = draw(&view, None);
        for text in ["Steam wants you to sign in", "Steam Guard - Computer Authorization Required", HINT.en, "Show PC screen", "Cancel"] {
            assert!(has(&drawn, text), "{text} in {:?}", drawn.texts);
        }
        assert!(!has(&drawn, "Starting game"), "the card stands in the stage word's place");
    }

    #[test]
    fn the_cards_line_fills_the_launchers_name_in_the_users_language() {
        let named = Attention { words: SIGN_IN, launcher: Some("Epic Games"), title: String::new() };
        assert_eq!(with_lang(Lang::En, || attention_line(&named)), "Epic Games wants you to sign in");
        let german = with_lang(Lang::De, || attention_line(&named));
        assert!(german.contains("Epic Games") && german != "Epic Games wants you to sign in", "{german}");
        let windows = Attention { words: UAC, launcher: None, title: String::new() };
        assert_eq!(with_lang(Lang::En, || attention_line(&windows)), UAC.en);
    }

    /// The curtain's stage and its card over a white frame, for the eye:
    /// `NIGHTJAR_SHOTS=<dir> cargo test -p nightjar-desktop-ui --lib launch_curtain_screenshots -- --ignored`.
    #[test]
    #[ignore = "writes PNGs; set NIGHTJAR_SHOTS"]
    fn launch_curtain_screenshots() {
        let Some(dir) = std::env::var_os("NIGHTJAR_SHOTS").map(std::path::PathBuf::from) else { return };
        std::fs::create_dir_all(&dir).expect("the folder for the screenshots");
        let sign_in = Attention { words: SIGN_IN, launcher: Some("Steam"), title: "Steam Guard - Computer Authorization Required".into() };
        let cases = [
            ("curtain-opening", View { stage: Some(OPENING), attention: None, dismissible: true }),
            ("curtain-waiting", View { stage: None, attention: None, dismissible: false }),
            ("curtain-sign-in", View { stage: Some(nightjar_policy::launch_curtain::STARTING), attention: Some(sign_in), dismissible: true }),
            ("curtain-uac", View { stage: None, attention: Some(Attention { words: UAC, launcher: None, title: String::new() }), dismissible: true }),
        ];
        for (name, view) in cases {
            draw(&view, None).canvas.save(&dir.join(format!("{name}.png")));
        }
    }
}
