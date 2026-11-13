use super::*;

/// A renderer made again (Linux's switch to the GPU renderer) gets the font atlas whole: no
/// copy of it is kept, egui sends it again.
#[test]
fn a_new_renderer_gets_the_font_atlas_whole() {
    let ctx = egui::Context::default();
    let frame = |ctx: &egui::Context| {
        ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.label("Nightjar");
        })
        .textures_delta
    };
    let font = |mut delta: egui::TexturesDelta| {
        let pos = delta.set.get(&TextureId::default()).and_then(|d| d.first()).map(|d| d.pos);
        delta.clear();
        pos
    };
    assert_eq!(font(frame(&ctx)), Some(None), "the first frame sends it whole");
    assert_eq!(font(frame(&ctx)), None, "then only what changes");
    resend_fonts(&ctx);
    assert_eq!(font(frame(&ctx)), Some(None), "and whole again after a renderer change");
}

#[test]
fn the_keys_the_launcher_listens_for_reach_egui() {
    assert_eq!(key_of(SDLK_F2), Some(Key::F2));
    assert_eq!(key_of(SDLK_F10), Some(Key::F10), "Shift+F10 opens a PC's options");
    assert_eq!(key_of(SDLK_F11), Some(Key::F11));
    assert_eq!(key_of(SDLK_N), Some(Key::N), "Ctrl+N adds a PC");
    assert_eq!(key_of(SDLK_F1), Some(Key::F1), "F1 opens Help");
    assert_eq!(key_of(SDLK_QUESTION), Some(Key::Questionmark), "? opens Help outside a text field");
}

fn zoom_after_ctrl(ctx: &egui::Context, key: Key) -> f32 {
    let ctrl = Modifiers { ctrl: true, command: true, ..Modifiers::NONE };
    let input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))),
        events: vec![Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: ctrl,
        }],
        ..Default::default()
    };
    ctx.run_ui(input, |_| {}).textures_delta.clear();
    // A new zoom takes effect at the start of the next pass.
    ctx.run_ui(egui::RawInput::default(), |_| {}).textures_delta.clear();
    ctx.zoom_factor()
}

/// How far a tall page scrolls after `gui`'s events (nightjar#94).
fn page_offset(gui: &mut Gui) -> f32 {
    let mut offset = 0.0;
    for n in 0..3 {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 300.0))),
            time: Some(f64::from(n)),
            events: std::mem::take(&mut gui.events),
            ..Default::default()
        };
        gui.ctx
            .run_ui(input, |ui| {
                offset = egui::ScrollArea::vertical()
                    .auto_shrink(false)
                    .show(ui, |ui| ui.allocate_space(Vec2::new(300.0, 3000.0)))
                    .state
                    .clamped_offset()
                    .y;
            })
            .textures_delta
            .clear();
    }
    offset
}

/// A wheel turn scrolls the page under it with no pointer motion before it: the window just
/// opened, or a touchpad's two fingers moved no pointer.
#[test]
fn a_wheel_turn_scrolls_without_a_motion_first() {
    let scale = Scale { pixels_per_point: 1.0, density: 1.0 };
    // SAFETY: an all-zero SDL_Event is a valid value of the C union.
    let mut event: SDL_Event = unsafe { std::mem::zeroed() };
    event.r#type = SDL_EVENT_MOUSE_WHEEL.0;
    event.wheel.y = -3.0;
    event.wheel.mouse_x = 200.0;
    event.wheel.mouse_y = 150.0;
    // egui alone ignores the turn: it knows no pointer position.
    let mut bare = Gui::new();
    bare.push(Event::MouseWheel {
        unit: egui::MouseWheelUnit::Line,
        delta: Vec2::new(0.0, -3.0),
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Unknown,
        modifiers: Modifiers::NONE,
    });
    assert_eq!(page_offset(&mut bare), 0.0);
    let mut gui = Gui::new();
    gui.handle(&event, scale);
    assert!(page_offset(&mut gui) > 0.0, "the turn scrolls the page under the pointer");
}

#[test]
fn egui_does_not_zoom_on_its_own_ctrl_keys() {
    // The default context would, which is why `Gui::new` turns it off.
    assert!(zoom_after_ctrl(&egui::Context::default(), Key::Equals) > 1.0);
    let gui = Gui::new();
    for key in [Key::Equals, Key::Plus, Key::Minus, Key::Num0] {
        assert_eq!(zoom_after_ctrl(&gui.ctx, key), 1.0, "{key:?}");
    }
}

#[test]
fn a_pass_asking_for_a_repaint_wakes_the_loop_once_per_refresh_and_a_settled_one_does_not() {
    let mut gui = Gui::new();
    let pass = |gui: &mut Gui, n: u32, repaint: bool| {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(200.0, 200.0))),
            time: Some(1.0 + f64::from(n) * 0.1),
            ..Default::default()
        };
        gui.ran = Instant::now();
        let mut output = gui.ctx.run_ui(input, |ui| {
            ui.label("Settings");
            if repaint { ui.ctx().request_repaint(); }
        });
        output.textures_delta.clear();
        gui.note_repaint(&output);
    };
    // The first passes ask for repaints of their own (fonts, layout); look at a settled one.
    for n in 0..6 {
        pass(&mut gui, n, false);
    }
    assert_eq!(gui.repaint_in(Duration::ZERO), None, "nothing animates, nothing wakes the loop");
    pass(&mut gui, 6, true);
    assert_eq!(gui.repaint_in(Duration::ZERO), Some(Duration::ZERO), "an animation's next frame is due now");
    let wait = gui.repaint_in(Duration::from_millis(500)).expect("still asked for");
    assert!(wait > Duration::from_millis(400), "but no sooner than a refresh after the last pass: {wait:?}");
}

/// A settled pass over chrome through the same steps as `run`, without a window.
fn glass_pass(gui: &mut Gui, n: u32) -> Vec<egui::epaint::ClippedPrimitive> {
    gui.prepare_glass();
    let input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 300.0))),
        time: Some(1.0 + f64::from(n) * 0.1),
        ..Default::default()
    };
    gui.ran = Instant::now();
    let mut output = gui.ctx.run_ui(input, |ui| {
        let bar = Rect::from_min_size(Pos2::new(20.0, 20.0), Vec2::new(300.0, 48.0));
        ui.painter().rect_filled(ui.max_rect(), 0.0, egui::Color32::RED);
        crate::ui::glass::boundary(ui.painter(), vec![bar]);
        let mut shapes = Vec::new();
        crate::ui::glass::surface(&mut shapes, ui.ctx(), bar, 24.0, crate::ui::glass::Material::Chrome);
        ui.painter().extend(shapes);
    });
    output.textures_delta.clear();
    gui.note_repaint(&output);
    gui.ctx.tessellate(std::mem::take(&mut output.shapes), output.pixels_per_point)
}

fn has_boundary(primitives: &[egui::epaint::ClippedPrimitive]) -> bool {
    live_glass::find_boundary(primitives).is_some()
}

/// Records whether a frame ran the chain.
#[derive(Default)]
struct Counted { chains: u32, meshes: u32 }

impl Stage for Counted {
    fn size(&self) -> (i32, i32) { (400, 300) }
    fn mesh(&mut self, _: Rect, _: &egui::epaint::Mesh) { self.meshes += 1; }
    fn targets_work(&mut self) -> Result<(), String> { Ok(()) }
    fn begin_scene(&mut self) -> Result<(), String> { self.chains += 1; Ok(()) }
    fn end_scene(&mut self, _: bool) -> Result<Option<f32>, String> { Ok(None) }
    fn release(&mut self) {}
}

#[test]
fn live_glass_leaves_an_idle_launcher_idle() {
    crate::theme::own_palette();
    let mut gui = Gui::new();
    gui.glass = Glass::new(None);
    let mut stage = Counted::default();
    gui.glass.settle(&mut stage);
    assert!(gui.glass.take_changed(), "the targets work: live from the next pass");
    for n in 0..6 {
        glass_pass(&mut gui, n);
    }
    assert_eq!(live_glass::tier(&gui.ctx), Tier::Live);
    assert_eq!(gui.repaint_in(Duration::ZERO), None, "the boundary and the glass ask for no frames");
    let frame = glass_pass(&mut gui, 6);
    assert!(has_boundary(&frame));
    gui.glass.paint(&mut stage, &frame, false);
    gui.glass.settle(&mut stage);
    assert_eq!(stage.chains, 1);
    assert!(!gui.glass.take_changed(), "a chain that ran asks for no frame either");
    let frame = glass_pass(&mut gui, 7);
    gui.glass.paint(&mut stage, &frame, false);
    assert_eq!(stage.chains, 1, "nothing changed, no chain");
    assert_eq!(gui.repaint_in(Duration::ZERO), None);
}

#[test]
fn a_streams_frames_lay_out_no_boundary_and_run_no_chain() {
    crate::theme::own_palette();
    let mut gui = Gui::new();
    gui.glass = Glass::new(None);
    let mut stage = Counted::default();
    gui.glass.settle(&mut stage);
    assert!(has_boundary(&glass_pass(&mut gui, 0)), "the launcher, live");
    gui.set_over_video(true);
    let frame = glass_pass(&mut gui, 1);
    assert_eq!(live_glass::tier(&gui.ctx), Tier::Baked);
    assert!(!has_boundary(&frame));
    let live_mesh = |p: &egui::epaint::ClippedPrimitive| matches!(&p.primitive, egui::epaint::Primitive::Mesh(m) if m.texture_id == LIVE_BACKDROP);
    assert!(!frame.iter().any(live_mesh), "nothing samples the chain");
    gui.glass.paint(&mut stage, &frame, true);
    assert_eq!(stage.chains, 0);
    assert_eq!(stage.meshes as usize, frame.len());
    gui.set_over_video(false);
    assert!(has_boundary(&glass_pass(&mut gui, 2)), "back on the launcher");
}

/// `draw_overlay` returns early while nothing shows, so this has to come before its first return.
#[test]
fn a_streams_every_frame_says_it_is_over_video_before_anything_else() {
    let main = include_str!("../main.rs");
    let overlay = &main[main.find("fn draw_overlay(").expect("draw_overlay")..];
    let said = overlay.find("gui.set_over_video(true)").expect("the stream's frames say so");
    assert!(said < overlay.find("return false").expect("the early return"));
    let launcher = &main[main.find("self.gui.set_readable(true)").expect("the launcher's frame")..];
    assert!(launcher.find("self.gui.set_over_video(false)").expect("the launcher's frames say so") < launcher.find("self.gui.run(").expect("its pass"));
}

#[test]
fn follow_focus_scrolls_to_the_row_that_arrow_keys_reach() {
    let ctx = egui::Context::default();
    let mut last: Option<egui::Id> = None;
    let mut frame = |events: Vec<Event>| -> f32 {
        let mut offset = 0.0;
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 300.0))),
            events,
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            let out = egui::ScrollArea::vertical().max_height(100.0).show(ui, |ui| {
                follow_focus(ui, &mut last);
                for i in 0..30 {
                    let row = ui.button(format!("Row {i}"));
                    if i == 0 && !ui.ctx().memory(|m| m.focused().is_some()) {
                        row.request_focus();
                    }
                }
            });
            offset = out.state.clamped_offset().y;
        });
        // Nothing paints here; epaint asserts (debug builds) that texture uploads are not dropped.
        output.textures_delta.clear();
        offset
    };
    frame(vec![]);
    let offset = frame(vec![]);
    assert_eq!(offset, 0.0, "the first row starts in view");
    let mut offset = offset;
    for _ in 0..25 {
        for pressed in [true, false] {
            frame(vec![Event::Key { key: Key::ArrowDown, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE }]);
        }
        frame(vec![]);
        offset = frame(vec![]);
    }
    assert!(offset > 100.0, "the scroll area followed focus down to row 25, offset {offset}");
}

/// A header above a scrolled area (Settings' search field) taking focus leaves the rows where
/// they are: it lies inside the content's geometry once scrolled, but shows outside the area.
#[test]
fn focus_on_a_header_above_a_scrolled_area_does_not_scroll_it() {
    let ctx = egui::Context::default();
    let mut last: Option<egui::Id> = None;
    let mut frame = |n: usize, focus_header: bool| -> f32 {
        let mut offset = 0.0;
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 300.0))), ..Default::default() };
        let mut output = ctx.run_ui(input, |ui| {
            let header = ui.button("Search");
            if focus_header { header.request_focus(); }
            let mut area = egui::ScrollArea::vertical().max_height(100.0);
            if n == 0 { area = area.vertical_scroll_offset(300.0); }
            let out = area.show(ui, |ui| {
                follow_focus(ui, &mut last);
                for i in 0..30 { let _ = ui.button(format!("Row {i}")); }
            });
            offset = out.state.clamped_offset().y;
        });
        output.textures_delta.clear();
        offset
    };
    for n in 0..3 { frame(n, false); }
    frame(3, true);
    let mut offset = 0.0;
    for n in 4..8 { offset = frame(n, false); }
    assert_eq!(offset, 300.0, "the rows stay scrolled");
}
