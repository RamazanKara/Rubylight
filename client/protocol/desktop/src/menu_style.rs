//! The in-stream menus in the launcher's look: its dialog frame, a title and the game's line,
//! rows with a label left and the control right, its switches and its buttons. Everything here is
//! drawn only while a menu is open; a closed menu costs the stream nothing.

use egui::{vec2, Align, Align2, Color32, CornerRadius, Frame, Layout, Margin, Response, RichText, Stroke, Ui};

use crate::theme::{radius, regular, semibold, space, text, MOON, ON_MOON, ON_SURFACE, ON_SURFACE_VARIANT, OUTLINE_VARIANT, SURFACE_HIGH, SURFACE_HIGHEST};
use crate::ui::{focus_ring, name_for_readers, toggle};

/// The menus' width with their frame: wide enough for a row's label and its list side by side.
pub const WIDTH: f32 = 440.0;

/// A menu's window: the launcher's dialog frame, centred, without egui's title bar (the menu draws
/// its own title with [`header`]).
pub fn window<'a>(title: &'a str, id: &str) -> egui::Window<'a> {
    egui::Window::new(title)
        .id(egui::Id::unique(id))
        .title_bar(false)
        .anchor(Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .collapsible(false)
        .resizable(false)
        .min_width(WIDTH)
        .max_width(WIDTH)
        .frame(
            Frame::new()
                .fill(*SURFACE_HIGH)
                .corner_radius(CornerRadius::same(16))
                .inner_margin(Margin::same(24))
                .stroke(Stroke::new(1.0, OUTLINE_VARIANT)),
        )
}

/// A menu's content: as tall as the window allows less a margin, scrolling beyond that. egui
/// lays a new window out in at most 400 points the first time, which cut the menu short.
pub fn body<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let room = ui.ctx().content_rect().height() - 2.0 * (space::XL + 24.0);
    ui.set_max_height(room);
    egui::ScrollArea::vertical().max_height(room).show(ui, add).inner
}

/// The menu's title and, under it, what streams from where.
pub fn header(ui: &mut Ui, title: &str, line: &str) {
    ui.label(RichText::new(title).font(semibold(text::SECTION)).color(ON_SURFACE));
    ui.label(RichText::new(line).font(regular(text::LABEL)).color(ON_SURFACE_VARIANT));
    ui.add_space(space::M);
}

/// A row: its label left, its control (a list, a slider) right.
pub fn row<R>(ui: &mut Ui, label: &str, control: impl FnOnce(&mut Ui) -> R) -> R {
    ui.horizontal(|ui| {
        ui.set_min_height(36.0);
        ui.label(RichText::new(label).font(regular(text::BODY)).color(ON_SURFACE));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            // A list's box on the menu's own fill would vanish: one step lighter, as on a settings row.
            ui.visuals_mut().widgets.inactive.weak_bg_fill = *SURFACE_HIGHEST;
            control(ui)
        })
        .inner
    })
    .inner
}

/// A switch row, as the launcher's settings have them; the response says when it changed.
pub fn switch(ui: &mut Ui, label: &str, on: &mut bool) -> Response {
    row(ui, label, |ui| {
        let response = toggle(ui, on);
        name_for_readers(ui, &response, label, "");
        response
    })
}

fn wide_button(ui: &mut Ui, label: &str, fill: Color32, ink: Color32) -> Response {
    let button = egui::Button::new(RichText::new(label).font(semibold(text::LABEL)).color(ink))
        .fill(fill)
        .corner_radius(CornerRadius::same(radius::CONTROL))
        .min_size(vec2(ui.available_width(), 40.0));
    let response = ui.add(button);
    focus_ring(ui, response.rect, &response, radius::CONTROL);
    response
}

/// An action across the menu's width.
pub fn button(ui: &mut Ui, label: &str) -> Response {
    wide_button(ui, label, *SURFACE_HIGHEST, *ON_SURFACE)
}

/// The menu's one main action, in amber (Return to stream).
pub fn primary(ui: &mut Ui, label: &str) -> Response {
    wide_button(ui, label, *MOON, *ON_MOON)
}

/// Space and a thin line between groups.
pub fn divider(ui: &mut Ui) {
    ui.add_space(space::S);
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(width, 1.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 0.0, OUTLINE_VARIANT);
    ui.add_space(space::S);
}

/// A quiet line under a group: a hint or why something is off.
pub fn note(ui: &mut Ui, text_: &str) {
    ui.label(RichText::new(text_).font(regular(text::CAPTION)).color(ON_SURFACE_VARIANT));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(ctx: &egui::Context, events: Vec<egui::Event>, add: impl FnMut(&mut Ui)) -> egui::FullOutput {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(1280.0, 800.0))),
            events,
            ..Default::default()
        };
        let mut add = add;
        let mut output = ctx.run_ui(input, |ui| {
            window("Stream controls", "menu-test").show(ui.ctx(), |ui| add(ui));
        });
        output.textures_delta.clear();
        output
    }

    /// One frame of a sample menu: the main action's and the switch's places, and whether it changed.
    fn sample(ctx: &egui::Context, events: Vec<egui::Event>, on: &mut bool) -> (egui::Rect, egui::Rect, bool) {
        let mut out = (egui::Rect::NOTHING, egui::Rect::NOTHING, false);
        run(ctx, events, |ui| {
            header(ui, "Stream controls", "Game · PC");
            out.0 = primary(ui, "Return to stream").rect;
            let r = switch(ui, "Mute microphone", on);
            out.1 = r.rect;
            out.2 |= r.changed();
        });
        out
    }

    /// The menu's main action spans its width, a switch sits at its right edge and turns on a click.
    #[test]
    fn the_menu_lays_out_across_its_width_and_its_switch_turns() {
        crate::theme::own_palette();
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let mut on = false;
        sample(&ctx, Vec::new(), &mut on);
        let (wide, knob, _) = sample(&ctx, Vec::new(), &mut on);
        assert!((wide.width() - (WIDTH - 2.0 * 24.0)).abs() < 4.0, "the main action spans the menu inside its margin: {wide:?}");
        assert!((knob.right() - wide.right()).abs() < 2.0, "the switch sits at the right edge: {knob:?} in {wide:?}");
        let at = knob.center();
        let click = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        sample(&ctx, vec![egui::Event::PointerMoved(at)], &mut on);
        sample(&ctx, vec![click(true)], &mut on);
        let (_, _, changed) = sample(&ctx, vec![click(false)], &mut on);
        assert!(on && changed, "a click turns the switch on");
    }

    /// The stream menu as it opens over a game, drawn with these pieces and the real menu's rows:
    /// `NIGHTJAR_SHOTS=<dir> cargo test -p nightjar-desktop stream_menu_screenshot -- --ignored`.
    #[test]
    #[ignore = "writes PNGs; set NIGHTJAR_SHOTS"]
    fn stream_menu_screenshot() {
        use crate::strings::tr;
        let Some(dir) = std::env::var_os("NIGHTJAR_SHOTS").map(std::path::PathBuf::from) else { return };
        std::fs::create_dir_all(&dir).expect("the folder for the screenshots");
        crate::theme::own_palette();
        let art = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../app/src/debug/assets/demo/art/3.png");
        let art = image::load_from_memory(&std::fs::read(art).expect("the demo cover")).expect("a PNG").to_rgba8();
        for (width, height, ppp) in [(1280.0, 800.0, 1.5), (1920.0, 1080.0, 1.0)] {
            let ctx = egui::Context::default();
            crate::theme::apply(&ctx);
            let picture = ctx.load_texture(
                "picture",
                egui::ColorImage::from_rgba_unmultiplied([art.width() as usize, art.height() as usize], &art),
                egui::TextureOptions::LINEAR,
            );
            let (mut overlay, mut mouse, mut top, mut muted) = (0usize, false, false, false);
            let mut textures = egui::TexturesDelta::default();
            let mut shapes = Vec::new();
            for n in 0..6 {
                let mut input = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(width, height))),
                    time: Some(f64::from(n) * 0.05),
                    ..Default::default()
                };
                input.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(ppp);
                let output = ctx.run_ui(input, |ui| {
                    let screen = ui.ctx().content_rect();
                    // The game under the menu, as the stream shows it.
                    egui::Image::from_texture((picture.id(), screen.size()))
                        .uv(egui::Rect::from_min_max(egui::pos2(0.0, 0.3), egui::pos2(1.0, 0.75)))
                        .paint_at(ui, screen);
                    window(tr("Stream controls"), "stream-controls").show(ui.ctx(), |ui| body(ui, |ui| {
                        header(ui, tr("Stream controls"), "Open World · Living Room PC");
                        let back = primary(ui, tr("Return to stream"));
                        if n == 0 {
                            back.request_focus();
                        }
                        divider(ui);
                        row(ui, tr("Performance overlay"), |ui| {
                            egui::ComboBox::from_id_salt("o").width(190.0).selected_text(tr("Off")).show_index(ui, &mut overlay, 1, |_| tr("Off"));
                        });
                        row(ui, tr("Video size"), |ui| {
                            egui::ComboBox::from_id_salt("s").width(190.0).selected_text(tr("Fit")).show_index(ui, &mut overlay, 1, |_| tr("Fit"));
                        });
                        switch(ui, tr("Align video to top"), &mut top);
                        switch(ui, tr("Mute microphone"), &mut muted);
                        switch(ui, tr("Controller mouse"), &mut mouse);
                        divider(ui);
                        button(ui, tr("Export statistics (last hour)"));
                        button(ui, tr("Send clipboard to PC"));
                        button(ui, tr("Copy PC clipboard here"));
                        divider(ui);
                        button(ui, tr("Session and graphics"));
                        button(ui, tr("Reconnect"));
                        button(ui, tr("Disconnect"));
                        ui.add_space(space::XS);
                        note(ui, "Ctrl+Alt+Shift+Tab or Start + Back opens this menu.");
                    }));
                });
                textures.append(output.textures_delta);
                shapes = output.shapes;
            }
            let primitives = ctx.tessellate(shapes, ppp);
            let mut canvas = crate::shot::Canvas::new((width * ppp) as usize, (height * ppp) as usize);
            canvas.update_textures(&textures);
            canvas.paint(&primitives, ppp);
            canvas.save(&dir.join(format!("stream-menu-{}x{}.png", width as u32, height as u32)));
            textures.clear();
        }
    }
}
