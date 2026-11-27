//! Tests of the Control Center's frame as the app hosts it: placement, the rail, navigation, the
//! focus it keeps, the night lock and the contrast of its labels; and its screenshots.

use super::*;
use crate::menu_style;
use crate::session_page::HostExit;
use crate::shortcuts::Shortcuts;
use crate::stream_menu::{self, Available, Chosen, Leave};
use crate::theme::{self, Look};
use crate::ui::control_center_cards as cards;
use nightjar_policy::control_center::Gates;

/// A stream that offers everything the desktop can offer.
fn everything() -> Available {
    Available {
        mouse: Some(false),
        overlay: true,
        shortcuts: true,
        microphone: Some(true),
        quality: true,
        clipboard: true,
        image_clipboard: true,
        commands: true,
        session: true,
        host_audio: true,
        controller_audio: true,
        usb: true,
        controller_settings: true,
        guests: true,
        suspend: Some(false),
        dvr: None,
        exits: vec![HostExit::End, HostExit::QuitApp],
    }
}

/// The Control Center as the app hosts it, with stand-ins for the rows the app draws.
struct Demo {
    state: State,
    available: Available,
    saved: Shortcuts,
    surface: Surface,
    input: InputSource,
    size: egui::Vec2,
    ppp: f32,
    /// A sub-page's title, when one is up.
    sub_page: Option<&'static str>,
    notice: Option<&'static str>,
    kbps: u32,
    night_mode: bool,
    chosen: Option<Chosen>,
    leave: Option<Leave>,
    shown: Shown,
    /// Drawn under the Control Center: the video.
    video: Option<(egui::TextureId, Color32)>,
    /// Every frame's texture changes, for the canvas.
    textures: egui::TexturesDelta,
}

impl Drop for Demo {
    fn drop(&mut self) {
        self.textures.clear();
    }
}

impl Demo {
    fn new() -> Self {
        let mut saved = Shortcuts::default();
        for (name, keys) in [("Task view", &["WIN", "TAB"][..]), ("Snip", &["WIN", "SHIFT", "S"][..])] {
            saved.save(None, name, &keys.iter().map(|k| k.to_string()).collect::<Vec<_>>(), "desktop").unwrap();
        }
        Self {
            state: State::default(),
            available: everything(),
            saved,
            surface: Surface::Baked,
            input: InputSource::Keyboard,
            size: vec2(1280.0, 800.0),
            ppp: 1.0,
            sub_page: None,
            notice: None,
            kbps: 40_000,
            night_mode: true,
            chosen: None,
            leave: None,
            shown: Shown::default(),
            video: None,
            textures: Default::default(),
        }
    }

    fn dock(&self) -> Dock {
        // Instant Replay's cards on Clips when the stream offers Stream DVR, as `app::control_center` gates them.
        let gates = Gates { stream_dvr: self.available.dvr.is_some(), ..Gates::default() };
        stream_menu::dock(&self.available, self.saved.data.len(), &Default::default(), &gates)
    }

    fn frame(&mut self, ctx: &Context, events: Vec<egui::Event>) -> Vec<egui::epaint::ClippedShape> {
        let mut input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, self.size)),
            events,
            ..Default::default()
        };
        input.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(self.ppp);
        let dock = self.dock();
        let mut output = ctx.run_ui(input, |ui| {
            let ctx = ui.ctx().clone();
            if let Some((texture, tint)) = self.video {
                let screen = ctx.content_rect();
                egui::Image::from_texture((texture, screen.size())).uv(Rect::from_min_max(pos2(0.0, 0.3), pos2(1.0, 0.75))).tint(tint).paint_at(ui, screen);
            }
            let frame = Frame {
                title: self.sub_page.unwrap_or("Open World"),
                line: if self.sub_page.is_some() { "Open World · Living Room PC" } else { "2560×1440 · 120 FPS · HEVC" },
                surface: self.surface,
                input: self.input,
                sub_page: self.sub_page.is_some(),
            };
            let (saved, kbps, night_mode, notice, sub_page) = (&self.saved, &mut self.kbps, &mut self.night_mode, self.notice, self.sub_page);
            let (mut chosen, mut leave) = (None, None);
            self.shown = show(
                &ctx,
                &mut self.state,
                &dock,
                &frame,
                |ui, module| {
                    let focus_first = ctx.memory(|m| m.focused().is_none());
                    if sub_page == Some(HELP) {
                        // The help sheet brings its own way back: no footer under it (`app::control_center::show_page`).
                        menu_style::sheet(ui, crate::glyphs::Style::Xbox, crate::config::GamepadQuitCombo::default(), true, true, true);
                        return;
                    }
                    if sub_page.is_some() {
                        stream_menu::shortcuts_page(ui, saved, focus_first);
                        return;
                    }
                    if module == Module::Quick && let Some(notice) = notice {
                        cards::notice_row(ui, notice);
                        ui.add_space(cards::GAP);
                    }
                    chosen = stream_menu::cards(ui, &dock, module, saved, None, focus_first);
                    let page = dock.page(module).expect("the module shown has a page");
                    if !page.cards.is_empty() {
                        ui.add_space(cards::GAP);
                    }
                    if module == Module::Picture {
                        cards::wide(ui, Icon::Gauge, "Quality", None, |ui| crate::stream_overlay_widgets::bitrate_slider(ui, kbps, true));
                        ui.add_space(cards::GAP);
                        cards::status_row(ui, Icon::Gauge, "Video encoder load", "62%");
                    }
                    // As the app's pages: no empty group under a page of cards only (Clips).
                    if page.rows.is_empty() {
                        return;
                    }
                    menu_style::group(ui, |ui| {
                        for id in &page.rows {
                            match *id {
                                "night_mode" => {
                                    menu_style::switch(ui, Icon::Speaker, "Night mode", night_mode);
                                }
                                id => {
                                    menu_style::action(ui, Icon::More, id, None);
                                }
                            }
                        }
                    });
                },
                |ui| leave = (sub_page != Some(HELP)).then(|| stream_menu::dock_footer(ui, &dock, Some("Esc"))).flatten(),
            );
            self.chosen = chosen;
            self.leave = leave;
        });
        self.textures.append(std::mem::take(&mut output.textures_delta));
        output.shapes
    }

    /// Frames until the layout rests (the footer's height is one frame late).
    fn settle(&mut self, ctx: &Context) -> Vec<egui::epaint::ClippedShape> {
        for _ in 0..3 {
            self.frame(ctx, Vec::new());
        }
        self.frame(ctx, Vec::new())
    }
}

fn context() -> Context {
    theme::own_palette();
    let ctx = Context::default();
    theme::apply(&ctx);
    // As `Gui::set_over_video` does before the stream's overlay draws.
    theme::set_over_video(&ctx, true);
    ctx
}

fn key(key: Key, modifiers: Modifiers) -> Vec<egui::Event> {
    vec![egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }]
}

fn click(at: egui::Pos2) -> [Vec<egui::Event>; 3] {
    let button = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
    [vec![egui::Event::PointerMoved(at)], vec![button(true)], vec![button(false)]]
}

fn focused(ctx: &Context) -> Option<Id> {
    ctx.memory(|m| m.focused())
}

/// Every text drawn, with its ink.
fn texts(shapes: &[egui::epaint::ClippedShape]) -> Vec<(String, Color32)> {
    fn walk(shape: &egui::Shape, out: &mut Vec<(String, Color32)>) {
        match shape {
            egui::Shape::Vec(list) => list.iter().for_each(|shape| walk(shape, out)),
            egui::Shape::Text(text) => {
                let ink = text.override_text_color.or_else(|| text.galley.job.sections.first().map(|section| section.format.color)).unwrap_or(text.fallback_color);
                out.push((text.galley.text().to_owned(), ink));
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    shapes.iter().for_each(|clipped| walk(&clipped.shape, &mut out));
    out
}

/// The page title that stands for Shortcuts & help in a demo.
const HELP: &str = "Shortcuts & help";

fn paint(ctx: &Context, demo: &mut Demo) -> crate::shot::Canvas {
    let mut shapes = Vec::new();
    for _ in 0..5 {
        shapes = demo.frame(ctx, Vec::new());
    }
    let primitives = ctx.tessellate(shapes, demo.ppp);
    let mut canvas = crate::shot::Canvas::new((demo.size.x * demo.ppp) as usize, (demo.size.y * demo.ppp) as usize);
    canvas.update_textures(&demo.textures);
    canvas.paint(&primitives, demo.ppp);
    canvas
}

#[test]
fn the_dock_sits_at_the_right_edge_and_narrows_in_a_small_window() {
    let window = |w: f32, h: f32| geometry(Rect::from_min_size(egui::Pos2::ZERO, vec2(w, h)));
    let wide = window(1280.0, 800.0);
    assert_eq!(wide.placement, Placement::Dock);
    assert_eq!((wide.panel.width(), wide.panel.right(), wide.panel.height()), (tokens::SIZE_CONTROL_CENTER_WIDTH, 1280.0, 800.0));
    assert_eq!(wide.rail.width(), tokens::SIZE_CONTROL_CENTER_RAIL);
    assert!(wide.rail.left() == wide.panel.left() && wide.content.left() > wide.rail.right(), "the rail is the dock's left edge");
    let small = window(720.0, 480.0);
    assert_eq!((small.placement, small.panel.width()), (Placement::Dock, tokens::SIZE_CONTROL_CENTER_WIDTH_COMPACT));
    // A tall narrow window is the one place the policy gives the sheet.
    let tall = window(480.0, 900.0);
    assert_eq!(tall.placement, Placement::Sheet);
    assert_eq!((tall.panel.width(), tall.panel.bottom()), (480.0, 900.0));
    assert!(tall.panel.top() > 200.0 && tall.rail.height() == tokens::SIZE_CONTROL_CENTER_RAIL);
    // A narrow landscape window keeps the dock, never wider than the window.
    assert_eq!(window(540.0, 400.0).placement, Placement::Dock);
    assert_eq!(window(300.0, 200.0).panel.width(), 300.0);
}

#[test]
fn the_rail_lists_only_modules_with_something() {
    let ctx = context();
    let mut demo = Demo::new();
    demo.settle(&ctx);
    let dock = demo.dock();
    for module in Module::ALL {
        assert_eq!(ctx.read_response(rail_id(module)).is_some(), dock.page(module).is_some(), "{module:?}");
    }
    assert!(dock.page(Module::Clips).is_none(), "no gate is open: Clips has nothing");
    // A plain stream: no microphone, PC page, controller extras or guests; the controller editor opens.
    let (ctx, mut plain) = (context(), Demo::new());
    plain.available = Available { overlay: true, quality: true, controller_settings: true, ..Default::default() };
    plain.settle(&ctx);
    let shown: Vec<Module> = Module::ALL.into_iter().filter(|module| ctx.read_response(rail_id(*module)).is_some()).collect();
    assert_eq!(shown, [Module::Quick, Module::Picture, Module::Controller, Module::Power]);
}

#[test]
fn opening_puts_the_focus_on_the_first_card_and_left_reaches_the_rail() {
    let ctx = context();
    let mut demo = Demo::new();
    demo.settle(&ctx);
    assert_eq!(focused(&ctx), Some(stream_menu::card_id("controller_mouse")), "A right after opening never leaves the stream");
    demo.frame(&ctx, key(Key::ArrowLeft, Modifiers::NONE));
    demo.settle(&ctx);
    assert_eq!(focused(&ctx), Some(rail_id(Module::Quick)), "Left from the first column lands on the module shown");
    assert_eq!(demo.state.module, Module::Quick);
    // Along the rail the focus turns the pages.
    demo.frame(&ctx, key(Key::ArrowDown, Modifiers::NONE));
    demo.settle(&ctx);
    assert_eq!((focused(&ctx), demo.state.module), (Some(rail_id(Module::Picture)), Module::Picture));
    demo.frame(&ctx, key(Key::ArrowRight, Modifiers::NONE));
    demo.settle(&ctx);
    let on_page = focused(&ctx).expect("Right goes into the page");
    assert!(Module::ALL.iter().all(|module| rail_id(*module) != on_page));
    assert_eq!(demo.state.module, Module::Picture, "and the page stays");
    // Back over to the rail from lower down the page: still the module shown.
    demo.frame(&ctx, key(Key::ArrowDown, Modifiers::NONE));
    demo.settle(&ctx);
    demo.frame(&ctx, key(Key::ArrowLeft, Modifiers::NONE));
    demo.settle(&ctx);
    assert_eq!((focused(&ctx), demo.state.module), (Some(rail_id(Module::Picture)), Module::Picture));
}

#[test]
fn ctrl_tab_and_the_triggers_step_through_the_modules_and_stop_at_the_ends() {
    let ctx = context();
    let mut demo = Demo::new();
    demo.settle(&ctx);
    demo.frame(&ctx, key(Key::Tab, Modifiers::CTRL));
    demo.settle(&ctx);
    assert_eq!(demo.state.module, Module::Picture);
    assert!(focused(&ctx).is_some(), "the focus is not lost with the page");
    demo.frame(&ctx, key(Key::Tab, Modifiers::CTRL | Modifiers::SHIFT));
    demo.settle(&ctx);
    assert_eq!(demo.state.module, Module::Quick);
    assert_eq!(focused(&ctx), Some(stream_menu::card_id("controller_mouse")));
    // The triggers ask through the state; before the first module there is nothing.
    demo.state.request_step(false);
    demo.settle(&ctx);
    assert_eq!(demo.state.module, Module::Quick);
    let last = demo.dock().pages.last().unwrap().module;
    for _ in 0..Module::ALL.len() + 2 {
        demo.state.request_step(true);
        demo.frame(&ctx, Vec::new());
    }
    assert_eq!((demo.state.module, last), (Module::Power, Module::Power), "the last module is where it stops");
    // A page a card opened keeps the module: the triggers rest.
    demo.sub_page = Some("Shortcuts");
    demo.state.request_step(false);
    demo.frame(&ctx, key(Key::Tab, Modifiers::CTRL | Modifiers::SHIFT));
    assert_eq!(demo.state.module, Module::Power);
}

#[test]
fn the_focus_returns_to_where_it_was_when_it_opens_again() {
    let ctx = context();
    let mut demo = Demo::new();
    demo.settle(&ctx);
    ctx.memory_mut(|m| m.request_focus(stream_menu::card_id("overlay")));
    demo.settle(&ctx);
    // Closed: the stream has the keys, and nothing of the dock is drawn.
    demo.state.closed();
    ctx.memory_mut(|m| m.surrender_focus(stream_menu::card_id("overlay")));
    for _ in 0..2 {
        ctx.run_ui(egui::RawInput::default(), |_| {}).textures_delta.clear();
    }
    assert_eq!(focused(&ctx), None);
    demo.settle(&ctx);
    assert_eq!(focused(&ctx), Some(stream_menu::card_id("overlay")));
    // On another module too, and a control that is gone gives way to the first card.
    demo.state.select(&ctx, Module::Audio);
    demo.settle(&ctx);
    assert_eq!(focused(&ctx), Some(stream_menu::card_id("microphone")));
    demo.state.closed();
    demo.available.microphone = None;
    demo.settle(&ctx);
    assert_eq!(demo.state.module, Module::Audio, "the module is kept");
    assert!(focused(&ctx).is_some(), "something has the focus again");
}

#[test]
fn a_way_out_that_had_the_focus_does_not_get_it_back_when_it_opens_again() {
    let ctx = context();
    let mut demo = Demo::new();
    demo.settle(&ctx);
    // Tab down to the footer: Return to stream, Disconnect and Quit game stand under the page.
    let bottom = |ctx: &Context| focused(ctx).and_then(|id| ctx.read_response(id)).map_or(0.0, |r| r.rect.top());
    for _ in 0..40 {
        if bottom(&ctx) > 800.0 - 140.0 {
            break;
        }
        demo.frame(&ctx, key(Key::Tab, Modifiers::NONE));
    }
    assert!(bottom(&ctx) > 800.0 - 140.0, "the focus reached the footer");
    demo.state.closed();
    for _ in 0..2 {
        ctx.run_ui(egui::RawInput::default(), |_| {}).textures_delta.clear();
    }
    demo.settle(&ctx);
    assert_eq!(focused(&ctx), Some(stream_menu::card_id("controller_mouse")), "A right after opening never leaves the stream");
}

#[test]
fn a_click_on_the_video_returns_to_the_stream_and_one_on_the_dock_does_not() {
    let ctx = context();
    let mut demo = Demo::new();
    demo.settle(&ctx);
    let mut clicked = false;
    for events in click(pos2(200.0, 400.0)) {
        demo.frame(&ctx, events);
        clicked |= demo.shown.scrim_clicked;
    }
    assert!(clicked);
    let g = geometry(Rect::from_min_size(egui::Pos2::ZERO, demo.size));
    let mut clicked = false;
    for events in click(pos2(g.rail.center().x, g.rail.bottom() - 60.0)) {
        demo.frame(&ctx, events);
        clicked |= demo.shown.scrim_clicked;
    }
    assert!(!clicked, "an empty spot of the dock is not the video");
}

#[test]
fn the_footer_stays_under_every_page_with_one_red_control() {
    let ctx = context();
    let mut demo = Demo::new();
    for module in [Module::Quick, Module::Picture, Module::Power] {
        demo.state.select(&ctx, module);
        let texts = texts(&demo.settle(&ctx));
        for label in ["Return to stream", "Disconnect", "Quit game"] {
            assert_eq!(texts.iter().filter(|(text, _)| text == label).count(), 1, "{label} on {module:?}");
        }
        let red = texts.iter().filter(|(_, ink)| *ink == *theme::ERROR).map(|(text, _)| text.as_str()).collect::<Vec<_>>();
        assert_eq!(red, ["Quit game"], "the one red control on {module:?}");
    }
    // Clicking it asks through the same way out as before.
    demo.settle(&ctx);
    assert_eq!(demo.leave, None);
}

/// The dock and everything in it are Obsidian Night whatever the theme: the same inks and the
/// same pixels by day and on OLED; high contrast is solid black with a bright rim.
#[test]
fn the_control_center_is_night_in_every_theme_and_black_in_high_contrast() {
    let frame_in = |look: Look| {
        theme::own_look(look);
        let ctx = Context::default();
        theme::apply(&ctx);
        theme::set_over_video(&ctx, true);
        let mut demo = Demo::new();
        demo.state.module = Module::Audio;
        let inks = texts(&demo.settle(&ctx));
        (inks, paint(&ctx, &mut demo))
    };
    let (night, night_canvas) = frame_in(Look::Night);
    assert!(night.iter().any(|(text, ink)| text == "Open World" && *ink == tokens::SURFACE_CONTROL_CENTER_LABEL));
    let g = geometry(Rect::from_min_size(egui::Pos2::ZERO, vec2(1280.0, 800.0)));
    let spots = [g.rail.center_bottom() - vec2(0.0, 40.0), g.content.center(), g.content.left_top() + vec2(40.0, 120.0), pos2(200.0, 400.0)];
    for look in [Look::Day, Look::Oled] {
        let (inks, canvas) = frame_in(look);
        assert_eq!(inks, night, "{look:?}: every label keeps its night ink");
        for spot in spots {
            let (a, b) = (canvas.pixel(spot.x as usize, spot.y as usize), night_canvas.pixel(spot.x as usize, spot.y as usize));
            assert!(a.iter().zip(b).all(|(a, b)| (a - b).abs() < 0.004), "{look:?} at {spot:?}: {a:?} is not night's {b:?}");
        }
    }
    let (high, canvas) = frame_in(Look::HighContrast);
    assert!(high.iter().any(|(text, ink)| text == "Open World" && *ink == Color32::WHITE));
    let rail = g.rail.center_bottom() - vec2(0.0, 40.0);
    let black = canvas.pixel(rail.x as usize, rail.y as usize);
    assert!(black[..3].iter().all(|c| *c < 0.004), "high contrast is solid black: {black:?}");
    let rim = canvas.pixel(g.panel.left() as usize, 400);
    assert!(rim[..3].iter().all(|c| *c > 0.5), "with a bright rim: {rim:?}");
    theme::own_palette();
}

/// High contrast over a white frame: nothing of the video shows through the dock.
#[test]
fn high_contrast_hides_the_video_under_the_dock() {
    theme::own_look(Look::HighContrast);
    let ctx = Context::default();
    theme::apply(&ctx);
    theme::set_over_video(&ctx, true);
    crate::live_glass::set_tier(&ctx, crate::live_glass::Tier::Live);
    let white = ctx.load_texture("white", egui::ColorImage::filled([4, 4], Color32::WHITE), egui::TextureOptions::LINEAR);
    let mut demo = Demo::new();
    demo.video = Some((white.id(), Color32::WHITE));
    demo.state.module = Module::Power;
    let g = geometry(Rect::from_min_size(egui::Pos2::ZERO, demo.size));
    for surface in [Surface::Solid, Surface::Blurred] {
        demo.surface = surface;
        let canvas = paint(&ctx, &mut demo);
        for spot in [g.content.center(), g.rail.center_bottom() - vec2(0.0, 80.0)] {
            let seen = canvas.pixel(spot.x as usize, spot.y as usize);
            assert!(seen[..3].iter().all(|c| *c < 0.004), "{surface:?} at {spot:?}: {seen:?}");
        }
    }
    theme::own_palette();
}

/// The surface the policy gives decides the dock's fill: baked is the opaque night fill, blurred
/// lets the (blurred) video through the lighter one.
#[test]
fn the_surface_decides_what_the_dock_is_painted_on() {
    let ctx = context();
    crate::live_glass::set_tier(&ctx, crate::live_glass::Tier::Live);
    let white = ctx.load_texture("white", egui::ColorImage::filled([4, 4], Color32::WHITE), egui::TextureOptions::LINEAR);
    let mut demo = Demo::new();
    demo.video = Some((white.id(), Color32::WHITE));
    let g = geometry(Rect::from_min_size(egui::Pos2::ZERO, demo.size));
    let spot = g.rail.center_bottom() - vec2(-120.0, 150.0);
    let mut at = |surface: Surface| {
        demo.surface = surface;
        paint(&ctx, &mut demo).pixel(spot.x as usize, spot.y as usize)[1]
    };
    let (blurred, baked) = (at(Surface::Blurred), at(Surface::Baked));
    assert!(baked < 0.2, "the baked fill hides a white frame: {baked}");
    assert!(blurred > baked && blurred < 0.2, "the blurred one lets it glow through, still dark: {blurred} over {baked}");
}

/// Over a white frame, the worst there is, both labels keep 4.5:1 on the dock: on the blurred
/// surface, on the baked one and inside a card.
#[test]
fn the_labels_keep_their_contrast_over_a_white_frame() {
    let ctx = context();
    crate::live_glass::set_tier(&ctx, crate::live_glass::Tier::Live);
    let white = ctx.load_texture("white", egui::ColorImage::filled([4, 4], Color32::WHITE), egui::TextureOptions::LINEAR);
    let mut demo = Demo::new();
    demo.video = Some((white.id(), Color32::WHITE));
    demo.state.module = Module::Power;
    let g = geometry(Rect::from_min_size(egui::Pos2::ZERO, demo.size));
    // An empty spot of the page, and one inside the first card of Quick.
    let spots = [(Module::Power, g.content.center()), (Module::Quick, g.content.left_top() + vec2(100.0, 70.0))];
    let linear = |c: f32| if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) };
    let luminance = |rgb: [f32; 3]| 0.2126 * linear(rgb[0]) + 0.7152 * linear(rgb[1]) + 0.0722 * linear(rgb[2]);
    let ink = |c: Color32| luminance([f32::from(c.r()) / 255.0, f32::from(c.g()) / 255.0, f32::from(c.b()) / 255.0]);
    for surface in [Surface::Blurred, Surface::Baked] {
        for (module, spot) in spots {
            demo.surface = surface;
            demo.state.module = module;
            let seen = paint(&ctx, &mut demo).pixel(spot.x as usize, spot.y as usize);
            let under = luminance([seen[0], seen[1], seen[2]]);
            for (name, label) in [("label", tokens::SURFACE_CONTROL_CENTER_LABEL), ("secondary label", tokens::SURFACE_CONTROL_CENTER_LABEL_SECONDARY)] {
                let ratio = (ink(label) + 0.05) / (under + 0.05);
                assert!(ratio >= 4.5, "the {name} on {surface:?} at {module:?}: {ratio:.2}:1 over {seen:?}");
            }
        }
    }
}

fn shoot(dir: &std::path::Path, name: &str, size: (f32, f32, f32), look: Look, bright: bool, set: impl FnOnce(&mut Demo)) {
    theme::own_look(look);
    let ctx = Context::default();
    theme::apply(&ctx);
    theme::set_over_video(&ctx, true);
    crate::live_glass::set_tier(&ctx, crate::live_glass::Tier::Live);
    let art = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../app/src/debug/assets/demo/art/3.jpg");
    let art = image::load_from_memory(&std::fs::read(art).expect("the demo cover")).expect("a JPEG").to_rgba8();
    let mut pixels = egui::ColorImage::from_rgba_unmultiplied([art.width() as usize, art.height() as usize], &art);
    // A bright frame (a snow field at noon) and a dark one (a cave) from the same picture.
    for pixel in &mut pixels.pixels {
        let lift = |c: u8| if bright { 150 + (u16::from(c) * 105 / 255) as u8 } else { (u16::from(c) * 70 / 255) as u8 };
        *pixel = Color32::from_rgb(lift(pixel.r()), lift(pixel.g()), lift(pixel.b()));
    }
    let video = ctx.load_texture("video", pixels, egui::TextureOptions::LINEAR);
    let mut demo = Demo::new();
    (demo.size, demo.ppp) = (vec2(size.0, size.1), size.2);
    demo.video = Some((video.id(), Color32::WHITE));
    demo.surface = Surface::Blurred;
    set(&mut demo);
    paint(&ctx, &mut demo).save(&dir.join(format!("{name}.png")));
    theme::own_palette();
}

/// The Control Center over a bright and a dark frame, each module, a card that is on, the slider
/// card, a sub-page, the footer focused, by night, by day (still night) and in high contrast, at
/// 1280x800, in a small window and at 4K's proportions:
/// `NIGHTJAR_SHOTS=<dir> cargo test -p nightjar-desktop-ui --lib control_center_screenshots -- --ignored`.
#[test]
#[ignore = "writes PNGs; set NIGHTJAR_SHOTS"]
fn control_center_screenshots() {
    let Some(dir) = std::env::var_os("NIGHTJAR_SHOTS").map(std::path::PathBuf::from) else { return };
    std::fs::create_dir_all(&dir).expect("the folder for the screenshots");
    let only = std::env::var("NIGHTJAR_SHOTS_ONLY").ok();
    let wanted = |name: &str| only.as_ref().is_none_or(|only| name.contains(only.as_str()));
    let deck = (1280.0, 800.0, 1.0);
    for module in [Module::Quick, Module::Picture, Module::Audio, Module::Pc, Module::Controller, Module::Guests, Module::Power] {
        for bright in [true, false] {
            let name = format!("cc-{}-{}", module.key(), if bright { "bright" } else { "dark" });
            if wanted(&name) {
                shoot(&dir, &name, deck, Look::Night, bright, |demo| {
                    demo.state.module = module;
                    demo.available.mouse = Some(true);
                    demo.notice = Some("The game started at 1920×1080: the PC could not switch its display to 2560×1440.");
                    demo.input = InputSource::Pad(glyphs::Style::Xbox);
                });
            }
        }
    }
    let cases = [
        ("cc-day-quick", deck, Look::Day, true, Surface::Blurred),
        ("cc-oled-quick", deck, Look::Oled, false, Surface::Blurred),
        ("cc-high-contrast-quick", deck, Look::HighContrast, true, Surface::Solid),
        ("cc-baked-quick", deck, Look::Night, true, Surface::Baked),
        ("cc-small-quick", (720.0, 480.0, 1.0), Look::Night, true, Surface::Blurred),
        ("cc-small-high-contrast", (720.0, 480.0, 1.0), Look::HighContrast, false, Surface::Solid),
        ("cc-4k-quick", (1920.0, 1080.0, 2.0), Look::Night, false, Surface::Blurred),
        ("cc-4k-day", (1920.0, 1080.0, 2.0), Look::Day, true, Surface::Blurred),
        ("cc-sheet-quick", (480.0, 900.0, 1.0), Look::Night, true, Surface::Baked),
    ];
    for (name, size, look, bright, surface) in cases {
        if wanted(name) {
            shoot(&dir, name, size, look, bright, |demo| {
                demo.surface = surface;
                demo.available.mouse = Some(true);
            });
        }
    }
    if wanted("cc-sub-page") {
        shoot(&dir, "cc-sub-page", deck, Look::Night, true, |demo| demo.sub_page = Some("Shortcuts"));
    }
    // Instant Replay while a recording runs, and Quick Suspend's paused game (Resume game on Power).
    for (name, look, module) in [("cc-clips-recording", Look::Night, Module::Clips), ("cc-clips-day", Look::Day, Module::Clips), ("cc-power-paused", Look::Night, Module::Power)] {
        if wanted(name) {
            shoot(&dir, name, deck, look, true, |demo| {
                demo.state.module = module;
                demo.available.dvr = Some(true);
                demo.available.suspend = Some(true);
            });
        }
    }
    for (name, look, size) in [("cc-help-sheet", Look::Night, deck), ("cc-help-sheet-day", Look::Day, deck), ("cc-help-sheet-4k", Look::Night, (1920.0, 1080.0, 2.0))] {
        if wanted(name) {
            shoot(&dir, name, size, look, true, |demo| demo.sub_page = Some(HELP));
        }
    }
    if wanted("cc-picture-4k") {
        shoot(&dir, "cc-picture-4k", (1920.0, 1080.0, 2.0), Look::Night, true, |demo| demo.state.module = Module::Picture);
    }
    if wanted("cc-picture-high-contrast") {
        shoot(&dir, "cc-picture-high-contrast", deck, Look::HighContrast, true, |demo| {
            demo.state.module = Module::Picture;
            demo.surface = Surface::Solid;
        });
    }
}
