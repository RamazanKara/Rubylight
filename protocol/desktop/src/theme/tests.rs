use super::*;

/// The moon fill and thumb stand where egui's own hit area does: equal ratios for bitrate.
#[test]
fn a_rail_places_its_thumb_by_the_scale_it_runs_in() {
    assert_eq!(rail_position(150.0, 100.0, 200.0, false), 0.5);
    assert!((rail_position(1000.0, 100.0, 10_000.0, true) - 0.5).abs() < 1e-5);
    assert_eq!(rail_position(50.0, 100.0, 200.0, true), 0.0);
    assert_eq!(rail_position(900.0, 100.0, 200.0, false), 1.0);
    // A range from zero cannot run in ratios, and an empty one has nowhere to go.
    assert_eq!(rail_position(5.0, 0.0, 10.0, true), 0.5);
    assert_eq!(rail_position(5.0, 5.0, 5.0, false), 0.0);
}

/// nightjar#61: the value floats over the thumb while it moves, then goes.
#[test]
fn a_slider_shows_its_value_while_it_moves() {
    assert!(value_floats(true, false, None), "a drag shows it");
    assert!(value_floats(false, true, Some(0.0)), "a key step shows it");
    assert!(value_floats(false, true, Some(VALUE_AFTER_KEY - 0.1)));
    assert!(!value_floats(false, true, Some(VALUE_AFTER_KEY)), "and it goes after a while");
    assert!(!value_floats(false, false, Some(0.1)), "or once the focus leaves");
    assert!(!value_floats(false, true, None), "focus alone does not show it");
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0));
    let rect = value_rect(egui::pos2(200.0, 100.0), egui::vec2(60.0, 14.0), screen);
    assert!(rect.bottom() < 100.0 - t::SIZE_SLIDER_THUMB / 2.0, "above the thumb, clear of the finger on it");
    assert_eq!(rect.center().x, 200.0);
    let edge = value_rect(egui::pos2(395.0, 100.0), egui::vec2(60.0, 14.0), screen);
    assert!(screen.contains_rect(edge), "a thumb at the edge keeps its value on screen");
    assert_eq!(percent(55.0), "55%");
    assert_eq!(mbps_text(20.0), "20.0 Mbps");
    assert_eq!(mbps_text(2.5), "2.5 Mbps");
}

/// The text a headless frame with one slider at `value` draws, after `events`.
fn slider_frame(ctx: &egui::Context, value: &mut f64, time: f64, events: Vec<egui::Event>) -> (Vec<String>, egui::Rect) {
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 200.0))),
        time: Some(time),
        events,
        ..Default::default()
    };
    let mut rail = egui::Rect::NOTHING;
    let mut output = ctx.run_ui(input, |ui| {
        ui.add_space(60.0);
        rail = slider(ui, value, 0.0..=100.0, 1.0, |v| format!("{v:.0} units"), |rail| rail, |field| field).rect;
    });
    output.textures_delta.clear();
    let mut drawn = Vec::new();
    for clipped in &output.shapes {
        if let egui::Shape::Text(text) = &clipped.shape { drawn.push(text.galley.text().to_owned()); }
    }
    (drawn, rail)
}

#[test]
fn dragging_a_slider_floats_its_value_and_letting_go_hides_it() {
    let ctx = egui::Context::default();
    let mut value = 20.0;
    let _ = slider_frame(&ctx, &mut value, 1.0, Vec::new());
    let (drawn, rail) = slider_frame(&ctx, &mut value, 1.1, Vec::new());
    assert!(!drawn.iter().any(|t| t.ends_with("units")), "at rest only the field shows it: {drawn:?}");
    let start = egui::pos2(rail.left() + 30.0, rail.center().y);
    let held = egui::pos2(rail.left() + 90.0, rail.center().y);
    let button = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
    slider_frame(&ctx, &mut value, 1.2, vec![egui::Event::PointerMoved(start), button(start, true)]);
    let (drawn, _) = slider_frame(&ctx, &mut value, 1.3, vec![egui::Event::PointerMoved(held)]);
    let shown = format!("{value:.0} units");
    assert!(drawn.contains(&shown), "the value floats while dragged: {shown} in {drawn:?}");
    slider_frame(&ctx, &mut value, 1.4, vec![button(held, false)]);
    let (drawn, _) = slider_frame(&ctx, &mut value, 1.5, Vec::new());
    assert!(!drawn.contains(&shown), "and goes when let go: {drawn:?}");
}

#[test]
fn bird_parses_into_a_closed_shape_inside_the_moon() {
    let bird = bird_polygon();
    assert!(bird.len() > 100);
    assert!(bird.iter().all(|&(x, y)| (27.0..=81.0).contains(&x) && (27.0..=81.0).contains(&y)));
    // The body is solid, the sky above the wings is not.
    assert!(inside(&bird, 54.5, 60.0));
    assert!(!inside(&bird, 54.0, 32.0));
}

/// A button, a stepped number field and a button in one column, in a headless egui context.
struct Column {
    ctx: egui::Context,
    value: u32,
    time: f64,
    ids: [egui::Id; 3],
    /// Whether the field reported a change in the frame that carried the last key.
    changed: bool,
}

impl Column {
    fn new(value: u32) -> Self {
        let mut column = Self { ctx: egui::Context::default(), value, time: 1.0, ids: [egui::Id::NULL; 3], changed: false };
        column.frame(Vec::new());
        column.frame(Vec::new());
        column
    }

    fn frame(&mut self, events: Vec<egui::Event>) -> bool {
        self.time += 0.1;
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(300.0, 300.0))),
            time: Some(self.time),
            events,
            ..Default::default()
        };
        let (value, ids) = (&mut self.value, &mut self.ids);
        let mut changed = false;
        let mut output = self.ctx.run_ui(input, |ui| {
            ids[0] = ui.button("above").id;
            let field = step_value(ui, value, 1000..=5000, 500.0, |d| d.suffix(" kbps"));
            (ids[1], changed) = (field.id, field.changed());
            ids[2] = ui.button("below").id;
        });
        output.textures_delta.clear();
        changed
    }

    fn press(&mut self, key: egui::Key) {
        self.changed = self.frame(crate::gui::tap(key).to_vec());
        self.frame(Vec::new());
    }

    fn focus_field(&mut self) {
        let field = self.ids[1];
        self.ctx.memory_mut(|m| m.request_focus(field));
        self.frame(Vec::new());
        self.frame(Vec::new());
        assert_eq!(self.ctx.memory(|m| m.focused()), Some(field));
    }

    fn focused(&self) -> Option<egui::Id> {
        self.ctx.memory(|m| m.focused())
    }
}

#[test]
fn up_and_down_leave_a_number_field_without_changing_it() {
    let mut column = Column::new(3000);
    column.focus_field();
    column.press(egui::Key::ArrowDown);
    assert_eq!(column.focused(), Some(column.ids[2]), "Down moves to the row below");
    assert_eq!(column.value, 3000, "Down does not step the field");
    column.focus_field();
    column.press(egui::Key::ArrowUp);
    assert_eq!(column.focused(), Some(column.ids[0]), "Up moves to the row above");
    assert_eq!(column.value, 3000, "Up does not step the field");
}

#[test]
fn left_and_right_step_a_number_field_inside_its_range() {
    let mut column = Column::new(4500);
    column.focus_field();
    column.press(egui::Key::ArrowRight);
    assert_eq!(column.value, 5000);
    assert!(column.changed, "a step is reported as a change, so callers write it back");
    column.press(egui::Key::ArrowRight);
    assert_eq!(column.value, 5000, "the top of the range holds");
    assert!(!column.changed, "holding at the top of the range is no change");
    assert_eq!(column.focused(), Some(column.ids[1]), "Right keeps the field focused");
    column.press(egui::Key::ArrowLeft);
    column.press(egui::Key::ArrowLeft);
    assert_eq!(column.value, 4000);
    assert_eq!(column.focused(), Some(column.ids[1]), "Left keeps the field focused");
    for _ in 0..10 { column.press(egui::Key::ArrowLeft); }
    assert_eq!(column.value, 1000, "the bottom of the range holds");
}

/// Typing into the field gives Left and Right back to the caret, so they neither step the value
/// nor clobber the typed text; once focus leaves and comes back they step again.
#[test]
fn left_and_right_move_the_caret_once_the_user_types() {
    let mut column = Column::new(2000);
    column.focus_field();
    column.frame(vec![egui::Event::Text("5".into())]);
    column.frame(Vec::new());
    let typed = column.value;
    assert!(typed >= 2000, "room for Left to step: {typed}");
    column.press(egui::Key::ArrowLeft);
    column.press(egui::Key::ArrowLeft);
    assert_eq!(column.value, typed, "Left does not step while typing");
    assert_eq!(column.focused(), Some(column.ids[1]));
    column.press(egui::Key::ArrowDown);
    assert_eq!(column.focused(), Some(column.ids[2]), "Down still leaves the field");
    column.focus_field();
    let before = column.value;
    column.press(egui::Key::ArrowLeft);
    assert_eq!(column.value, (before - 500).max(1000), "back on the field, Left steps again");
}

fn luminance(c: Color32) -> f64 {
    let lin = |v: u8| {
        let s = f64::from(v) / 255.0;
        if s <= 0.04045 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
}

/// WCAG contrast ratio of two colours, 1 to 21.
fn contrast(a: Color32, b: Color32) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

fn color(look: Look, role: Role) -> Color32 {
    palette(look)[usize::from(role.0)]
}

const GROUNDS: [Role; 4] = [NIGHT, SURFACE, SURFACE_HIGH, SURFACE_HIGHEST];

/// Every look and the contrast its text needs: WCAG AA, 7:1 in high contrast.
const LOOKS: [(Look, f64); 4] = [(Look::Night, 4.5), (Look::Day, 4.5), (Look::Oled, 4.5), (Look::HighContrast, 7.0)];

#[test]
fn text_meets_wcag_aa_in_every_look_and_seven_to_one_in_high_contrast() {
    for (look, floor) in LOOKS {
        for text in [ON_SURFACE, ON_SURFACE_VARIANT, OUTLINE, MOON, TERTIARY, ERROR, OK, WARN] {
            for ground in GROUNDS {
                let ratio = contrast(color(look, text), color(look, ground));
                assert!(ratio >= floor, "{look:?}: text {text:?} on {ground:?} is {ratio:.2}:1, wants {floor}");
            }
        }
        let on_moon = contrast(color(look, ON_MOON), color(look, MOON));
        assert!(on_moon >= floor, "{look:?}: button text on the moon is {on_moon:.2}:1");
        let on_container = contrast(color(look, ON_MOON_CONTAINER), color(look, MOON_CONTAINER));
        assert!(on_container >= floor, "{look:?}: text on a selection is {on_container:.2}:1");
        let on_error = contrast(color(look, ON_ERROR_CONTAINER), color(look, ERROR_CONTAINER));
        assert!(on_error >= floor, "{look:?}: text on a failure is {on_error:.2}:1");
    }
}

#[test]
fn the_theme_picks_the_look_and_high_contrast_wins() {
    use crate::config::Theme;
    for system_light in [false, true] {
        assert_eq!(Look::of(Theme::Dark, system_light, false), Look::Night, "Dark whatever the system says");
        assert_eq!(Look::of(Theme::Oled, system_light, false), Look::Oled, "OLED black whatever the system says");
        for theme in Theme::ALL {
            assert_eq!(Look::of(theme, system_light, true), Look::HighContrast, "{theme:?}: high contrast wins");
        }
    }
    assert_eq!(Look::of(Theme::System, false, false), Look::Night, "System on a dark system");
    assert_eq!(Look::of(Theme::System, true, false), Look::Day, "System on a light system");
    assert_eq!(Look::of(Theme::default(), false, false), Look::Night, "Dark is the default");
}

#[test]
fn the_theme_look_sets_the_roles_and_egui_light_or_dark() {
    let ctx = egui::Context::default();
    own_palette();
    for look in [Look::Day, Look::Oled, Look::HighContrast, Look::Night] {
        set_theme_look(&ctx, look, false);
        assert_eq!(self::look(), look);
        assert_eq!(*ON_SURFACE, color(look, ON_SURFACE), "{look:?}");
        let v = ctx.global_style().visuals.clone();
        assert_eq!(v.dark_mode, look != Look::Day, "{look:?}: egui's own widgets match the page");
        assert_eq!(v.panel_fill, color(look, NIGHT), "{look:?}");
    }
    // High contrast keeps the theme's look under it, and turning it off brings that back.
    set_theme_look(&ctx, Look::Day, false);
    set_look(&ctx, true, false);
    assert_eq!(look(), Look::HighContrast);
    set_look(&ctx, false, false);
    assert_eq!(look(), Look::Day);
    assert!(!ctx.global_style().visuals.dark_mode);
}

#[test]
fn over_video_draws_the_night_look_and_keeps_high_contrast() {
    let ctx = egui::Context::default();
    own_palette();
    for base in [Look::Day, Look::Oled] {
        set_theme_look(&ctx, base, false);
        set_over_video(&ctx, true);
        assert_eq!(look(), Look::Night, "{base:?}: the stream menu and overlay keep the night look");
        assert_eq!(*SURFACE, color(Look::Night, SURFACE));
        assert!(ctx.global_style().visuals.dark_mode, "{base:?}: egui's style follows");
        set_over_video(&ctx, false);
        assert_eq!(look(), base, "the launcher's look comes back with its frames");
        assert_eq!(ctx.global_style().visuals.panel_fill, color(base, NIGHT));
    }
    set_theme_look(&ctx, Look::HighContrast, false);
    set_over_video(&ctx, true);
    assert_eq!(look(), Look::HighContrast, "high contrast over video too");
    set_over_video(&ctx, false);
}

#[test]
fn oled_is_the_night_palette_on_black() {
    let surfaces = [NIGHT, SURFACE_LOWEST, SURFACE_LOW, SURFACE, SURFACE_HIGH, SURFACE_HIGHEST, SURFACE_BRIGHT].map(|role| usize::from(role.0));
    for i in (0..COUNT).filter(|i| !surfaces.contains(i)) {
        assert_eq!(OLED[i], NORMAL[i], "role {i} is the night one on OLED black");
    }
    for role in [NIGHT, SURFACE_LOWEST, SURFACE_LOW, SURFACE] {
        assert_eq!(color(Look::Oled, role), Color32::BLACK, "{role:?}");
    }
    // The steps a hover and a press climb to stay apart from black and from each other.
    assert_ne!(color(Look::Oled, SURFACE_HIGH), Color32::BLACK);
    assert_ne!(color(Look::Oled, SURFACE_HIGH), color(Look::Oled, SURFACE_HIGHEST));
}

#[test]
fn by_day_every_role_has_its_own_colour_and_text_is_dark_on_light() {
    for i in 0..COUNT {
        assert_ne!(DAY[i], NORMAL[i], "role {i} has a day colour of its own");
    }
    for ground in [NIGHT, SURFACE_LOWEST, SURFACE, SURFACE_HIGHEST] {
        for text in [ON_SURFACE, ON_SURFACE_VARIANT, MOON] {
            assert!(luminance(color(Look::Day, text)) < luminance(color(Look::Day, ground)), "{text:?} on {ground:?}");
        }
    }
}

#[test]
fn a_shade_gives_the_value_of_the_look() {
    let (night, day, oled) = (Color32::RED, Color32::GREEN, Color32::BLUE);
    let three = Shade::with_oled(night, day, oled);
    assert_eq!([Look::Night, Look::Day, Look::Oled, Look::HighContrast].map(|look| three.of(look)), [night, day, oled, night]);
    assert_eq!(Shade::new(night, day).of(Look::Oled), night, "OLED takes the night value unless it has one");
    own_look(Look::Day);
    assert_eq!(three.get(), day);
    assert_eq!(knob(), Color32::WHITE, "a switch's thumb stays white by day");
    own_look(Look::Oled);
    assert_eq!(three.get(), oled);
    assert_eq!(shade::COLOR_CANVAS.get(), Color32::BLACK);
    own_look(Look::HighContrast);
    assert_eq!(three.get(), night, "high contrast draws the night value where it has no look of its own");
    own_palette();
    assert_eq!(knob(), color(Look::Night, PAPER));
}

#[test]
fn status_on_the_scrim_reads_over_a_white_frame() {
    // Text over game art sits on the scrim; the worst frame behind it is white.
    let ground = over(SCRIM, Color32::WHITE);
    let white = contrast(Color32::WHITE, ground);
    assert!(white >= 7.0, "white on the scrim is {white:.2}:1");
    // By day the dot over art keeps the night green (`cards::running_dot`); the day one is for
    // light pages.
    for look in [Look::Night, Look::Oled, Look::HighContrast] {
        let ok = contrast(color(look, OK), ground);
        assert!(ok >= 4.5, "{look:?}: the running dot on the scrim is {ok:.2}:1");
    }
}

#[test]
fn warn_stays_apart_from_the_amber_of_the_action() {
    // They are close (the rule keeps warn out of filled shapes), but never the same colour.
    // Yellower is more green for its red; by day both are dark inks.
    let yellow = |c: Color32| f32::from(c.g()) / f32::from(c.r()).max(1.0);
    for (look, _) in LOOKS {
        assert_ne!(color(look, WARN), color(look, MOON), "{look:?}");
        assert!(yellow(color(look, WARN)) > yellow(color(look, MOON)), "{look:?}: warn is the yellower one");
    }
}

#[test]
fn the_look_follows_the_console_type_space_and_shape() {
    let ctx = egui::Context::default();
    own_palette();
    set_look(&ctx, false, false);
    let style = ctx.global_style();
    let size = |s: TextStyle| style.text_styles[&s].size;
    assert_eq!(
        [size(TextStyle::Heading), size(TextStyle::Body), size(TextStyle::Button), size(TextStyle::Small), size(TextStyle::Monospace)],
        [26.0, 16.0, 14.0, 12.0, 12.0]
    );
    assert_eq!(style.spacing.item_spacing, egui::vec2(8.0, 8.0));
    assert_eq!(style.spacing.button_padding.x, 16.0);
    assert_eq!(style.visuals.widgets.inactive.corner_radius, egui::CornerRadius::same(16));
    assert_eq!(style.visuals.window_corner_radius, egui::CornerRadius::same(22));
    assert_eq!(style.visuals.menu_corner_radius, egui::CornerRadius::same(16));
    assert_eq!([space::XS, space::S, space::M, space::L, space::XL, space::XXL, space::XXXL], [4.0, 8.0, 12.0, 16.0, 24.0, 32.0, 48.0]);
    assert_eq!(CONTROL_HEIGHT, 44.0);
}

#[test]
fn an_unchecked_box_shows_on_a_window_and_on_a_page() {
    let ctx = egui::Context::default();
    own_palette();
    for (look, _) in LOOKS {
        set_theme_look(&ctx, look, false);
        let v = ctx.global_style().visuals.clone();
        let box_ = &v.widgets.inactive;
        if look == Look::HighContrast {
            assert!(box_.bg_stroke.width > 0.0, "high contrast outlines the box");
        } else {
            for ground in [v.window_fill, v.panel_fill] {
                let ratio = contrast(box_.bg_fill, ground);
                assert!(ratio >= 1.3, "{look:?}: box on {ground:?} is {ratio:.2}:1");
            }
            // Buttons keep their own fill.
            assert_eq!(box_.weak_bg_fill, color(look, SURFACE_HIGH), "{look:?}");
        }
    }
}

/// `top` (maybe translucent) painted over the opaque `ground`, as egui blends it.
fn over(top: Color32, ground: Color32) -> Color32 {
    let keep = u32::from(255 - top.a());
    let mix = |t: u8, g: u8| (u32::from(t) + u32::from(g) * keep / 255).min(255) as u8;
    Color32::from_rgb(mix(top.r(), ground.r()), mix(top.g(), ground.g()), mix(top.b(), ground.b()))
}

#[test]
fn weak_and_disabled_text_stay_readable_in_both_palettes() {
    // What `ui.weak` and a disabled control draw come from the style, not from a role of
    // ours, so read them from the style the app sets. Disabled text is exempt from WCAG; it
    // still has to be read, so it gets 3:1 (4.5:1 in high contrast).
    for (look, weak_floor) in LOOKS {
        let disabled_floor = if look == Look::HighContrast { 4.5 } else { 3.0 };
        let ctx = egui::Context::default();
        own_palette();
        set_theme_look(&ctx, look, false);
        let visuals = ctx.global_style().visuals.clone();
        for ground in GROUNDS {
            let ground = color(look, ground);
            let weak = over(visuals.weak_text_color(), ground);
            let ratio = contrast(weak, ground);
            assert!(ratio >= weak_floor, "{look:?}: weak text on {ground:?} is {ratio:.2}:1, wants {weak_floor}");
            let disabled = over(visuals.disable(visuals.text_color()), ground);
            let ratio = contrast(disabled, ground);
            assert!(ratio >= disabled_floor, "{look:?}: disabled text on {ground:?} is {ratio:.2}:1, wants {disabled_floor}");
        }
    }
}

#[test]
fn the_focus_ring_stands_out_from_every_surface_in_every_look() {
    for (look, _) in LOOKS {
        for ground in GROUNDS {
            let ratio = contrast(color(look, MOON), color(look, ground));
            assert!(ratio >= 3.0, "{look:?}: ring on {ground:?} is {ratio:.2}:1, wants 3");
        }
    }
    // The ring is also thicker in high contrast.
    TEST_HIGH.with(|h| h.set(Some(false)));
    let normal = focus_ring_width();
    TEST_HIGH.with(|h| h.set(Some(true)));
    let thick = focus_ring_width();
    TEST_HIGH.with(|h| h.set(None));
    assert!(thick > normal);
}

#[test]
fn a_test_with_its_own_palette_leaves_the_global_one_alone() {
    own_palette();
    set_high_contrast(true);
    assert!(high_contrast());
    // Another thread has no override of its own: it still sees the global setting.
    assert!(!std::thread::spawn(high_contrast).join().unwrap());
    set_high_contrast(false);
    assert!(!high_contrast());
}

#[test]
fn hover_lifts_a_surface_one_step_and_pressing_two_never_past_the_brightest() {
    assert_eq!(lighter(SURFACE).0, SURFACE_HIGH.0);
    assert_eq!(lighter(SURFACE_HIGH).0, SURFACE_HIGHEST.0);
    assert_eq!(lighter(SURFACE_BRIGHT).0, SURFACE_BRIGHT.0, "the top of the ladder holds");
    assert_eq!(lighter(NIGHT).0, SURFACE.0, "the page background steps to a card surface");
    assert_eq!(state_fill(SURFACE, false, false).0, SURFACE.0);
    assert_eq!(state_fill(SURFACE, true, false).0, SURFACE_HIGH.0);
    assert_eq!(state_fill(SURFACE, true, true).0, SURFACE_HIGHEST.0);
    // Every step is a visible change and text stays readable on the brightest.
    for (look, floor) in LOOKS {
        for pair in LADDER.windows(2) {
            let (low, high) = (color(look, pair[0]), color(look, pair[1]));
            // OLED black's lower steps are all black; from there up each step shows.
            if look == Look::Oled && high == Color32::BLACK {
                continue;
            }
            assert_ne!(low, high, "{look:?}: {:?} and {:?}", pair[0], pair[1]);
        }
        for text in [ON_SURFACE, ON_SURFACE_VARIANT] {
            let ratio = contrast(color(look, text), color(look, SURFACE_BRIGHT));
            assert!(ratio >= floor, "{look:?}: {text:?} on the brightest surface is {ratio:.2}:1");
        }
    }
}

#[test]
fn hover_draws_no_outline_and_focus_stays_the_amber_ring() {
    let ctx = egui::Context::default();
    own_palette();
    for look in [Look::Night, Look::Day, Look::Oled] {
        set_theme_look(&ctx, look, false);
        let v = ctx.global_style().visuals.clone();
        assert_eq!(v.widgets.hovered.bg_stroke, Stroke::NONE, "{look:?}: no outline on hover");
        assert_eq!(v.widgets.active.bg_stroke.color, color(look, MOON), "{look:?}");
    }
    set_look(&ctx, true, false);
    let v = ctx.global_style().visuals.clone();
    assert_eq!(v.widgets.hovered.bg_stroke, v.widgets.inactive.bg_stroke, "high contrast keeps the outline every control has, no brighter one");
}

#[test]
fn high_contrast_outlines_are_visible_against_the_surfaces() {
    for ground in [NIGHT, SURFACE, SURFACE_HIGH] {
        let ratio = contrast(color(Look::HighContrast, OUTLINE_VARIANT), color(Look::HighContrast, ground));
        assert!(ratio >= 3.0, "outline on {ground:?} is {ratio:.2}:1, wants 3");
    }
}

/// The bird's mask holds as much as its polygon, at the place the moon's box puts it.
#[test]
fn the_birds_mask_covers_what_its_outline_does() {
    let polygon = bird_polygon();
    let twice: f32 = polygon
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let b = polygon[(i + 1) % polygon.len()];
            a.0 * b.1 - b.0 * a.1
        })
        .sum();
    let area = (twice / 2.0).abs();
    let size = 256;
    let mask = bird_mask(size);
    let covered: f32 = mask.pixels.iter().map(|p| f32::from(p.a()) / 255.0).sum();
    let units = covered * (54.0 / size as f32).powi(2);
    assert!((units - area).abs() / area < 0.03, "{units} of {area}");
    assert!(mask.pixels.iter().any(|p| p.a() == 255) && mask.pixels.iter().any(|p| p.a() == 0));
}

#[test]
fn logo_is_a_moon_with_a_night_bird() {
    let size = 64;
    let px = logo_rgba(size);
    let at = |x: usize, y: usize| &px[(y * size + x) * 4..(y * size + x) * 4 + 4];
    assert_eq!(at(0, 0)[3], 0, "corners are transparent");
    let moon = NORMAL[Index::MOON as usize];
    assert_eq!(at(32, 10), &[moon.r(), moon.g(), moon.b(), 255]);
    // The bird's body, just below the centre.
    let body = at(33, 40);
    assert!(body[0] < 80, "bird pixels are dark: {body:?}");
}

/// build.rs embeds this file as nightjar.exe's icon; the shell picks the size it needs from
/// small taskbar and title bar icons up to Explorer's extra large view.
#[test]
fn windows_icon_has_every_shell_size() {
    let ico = include_bytes!("../../packaging/icons/nightjar.ico");
    let u16_at = |i: usize| u16::from_le_bytes([ico[i], ico[i + 1]]);
    let u32_at = |i: usize| u32::from_le_bytes([ico[i], ico[i + 1], ico[i + 2], ico[i + 3]]) as usize;
    assert_eq!((u16_at(0), u16_at(2)), (0, 1), "an icon directory");
    let mut sizes = Vec::new();
    for entry in 0..usize::from(u16_at(4)) {
        let at = 6 + entry * 16;
        // A side of 0 means 256.
        let side = |byte: u8| if byte == 0 { 256 } else { usize::from(byte) };
        let (width, height) = (side(ico[at]), side(ico[at + 1]));
        assert_eq!(width, height, "square images");
        assert_eq!(u16_at(at + 6), 32, "{width} px has an alpha channel");
        assert!(u32_at(at + 12) + u32_at(at + 8) <= ico.len(), "{width} px lies inside the file");
        sizes.push(width);
    }
    for size in [16, 20, 24, 32, 40, 48, 64, 128, 256] {
        assert!(sizes.contains(&size), "{size} px missing from {sizes:?}");
    }
}

/// Whether the face at `index` in `bytes` maps `c` to a glyph.
fn covers(bytes: &[u8], index: u32, c: char) -> bool {
    use skrifa::MetadataProvider;
    skrifa::FontRef::from_index(bytes, index).is_ok_and(|font| font.charmap().map(c).is_some())
}

fn figtree_covers(c: char) -> bool {
    covers(FIGTREE, 0, c) && covers(FIGTREE_SEMIBOLD, 0, c)
}

/// Scripts written with spaces between words, where one word may not mix two faces.
fn alphabetic_script(c: char) -> bool {
    matches!(u32::from(c), 0x0041..=0x024F | 0x0370..=0x052F | 0x1E00..=0x1EFF)
}

#[test]
fn a_language_drawn_with_figtree_first_has_no_letter_figtree_lacks() {
    // Otherwise a word mixes Figtree with a lighter face. CJK may add its own scripts behind it.
    for lang in Lang::all() {
        let ahead = fallback(lang).is_some_and(|f| f.ahead);
        let lacking: std::collections::BTreeSet<char> =
            lang.texts().flat_map(str::chars).filter(|c| alphabetic_script(*c) && !figtree_covers(*c)).collect();
        if ahead {
            assert!(!lacking.is_empty(), "{lang:?} puts a system face ahead of Figtree without needing one");
        } else {
            assert!(lacking.is_empty(), "{lang:?} needs a system face ahead of Figtree for {lacking:?}");
        }
    }
}

#[test]
fn every_text_of_every_language_has_its_glyphs() {
    // System fonts differ per machine: a language whose face is not installed here is skipped
    // with a note (Linux CI without Noto CJK), its selection is covered above.
    let mut checked = 0;
    for lang in Lang::all() {
        let system = system_fonts(lang);
        if fallback(lang).is_some() && system.regular.is_none() {
            eprintln!("skipped {lang:?}: its system font is not installed here");
            continue;
        }
        // Each letter of a language drawn ahead of Figtree comes from its system face.
        if let Some(face) = system.regular.as_ref().filter(|_| system.ahead) {
            let bytes = (*face.font).as_ref();
            let lacking: std::collections::BTreeSet<char> = lang
                .texts()
                .flat_map(str::chars)
                .filter(|c| c.is_alphabetic() && !covers(bytes, face.index, *c))
                .collect();
            assert!(lacking.is_empty(), "{lang:?}'s system face lacks {lacking:?}");
        }
        let ctx = egui::Context::default();
        ctx.set_fonts(definitions(system));
        ctx.run_ui(egui::RawInput::default(), |_| {}).textures_delta.clear();
        for font in [regular(text::BODY), semibold(text::BODY)] {
            let missing: std::collections::BTreeSet<char> = lang
                .texts()
                .chain([lang.name()])
                .flat_map(str::chars)
                .filter(|c| !c.is_control() && !ctx.fonts_mut(|f| f.has_glyph(&font, *c)))
                .collect();
            assert!(missing.is_empty(), "{lang:?} draws {missing:?} as boxes in {font:?}");
        }
        checked += 1;
    }
    // A machine without any of the fonts still checks the languages Figtree draws alone.
    assert!(checked >= Lang::all().filter(|l| fallback(*l).is_none()).count());
}

#[test]
fn a_face_egui_cannot_read_is_dropped_instead_of_panicking() {
    // A WOFF2 or broken file, and a collection index past its faces.
    let mut bad_index = FontData::from_static(FIGTREE);
    bad_index.index = 7;
    let system = SystemFonts { ahead: true, regular: Some(FontData::from_owned(b"not a font".to_vec())), semibold: Some(bad_index) };
    let ctx = egui::Context::default();
    ctx.set_fonts(definitions(system));
    ctx.run_ui(egui::RawInput::default(), |_| {}).textures_delta.clear();
    assert!(ctx.fonts_mut(|f| f.has_glyph(&regular(text::BODY), 'A') && f.has_glyph(&semibold(text::BODY), 'A')));
}

#[test]
fn fontconfig_s_named_instance_index_picks_the_face_and_its_axes() {
    assert_eq!(face_data(FIGTREE.to_vec(), 0).map(|d| d.index), Some(0));
    // Figtree is static: its "first named instance" index still names face 0, without coordinates.
    assert_eq!(face_data(FIGTREE.to_vec(), 1 << 16).map(|d| d.index), Some(0));
    assert!(face_data(FIGTREE.to_vec(), 3).is_none());
    assert!(face_data(b"wOF2 not really".to_vec(), 0).is_none());
}

#[test]
fn a_language_s_faces_load_off_the_ui_thread() {
    let ctx = egui::Context::default();
    apply(&ctx);
    let started = Instant::now();
    fonts_for(&ctx, Lang::Ja);
    // The first call only starts the loader, however long the files take.
    while ctx.data(|d| d.get_temp::<Lang>(fonts_key())) != Some(Lang::Ja) {
        assert!(started.elapsed().as_secs() < 30, "the Japanese fonts never loaded");
        std::thread::sleep(std::time::Duration::from_millis(10));
        fonts_for(&ctx, Lang::Ja);
    }
    // Another language lets the Japanese files go, and the loader is done.
    fonts_for(&ctx, Lang::En);
    assert!(!LOADED.lock().unwrap().iter().any(|e| e.0 == Lang::Ja));
    assert!(!LOADING.lock().unwrap().contains(&Lang::Ja));
}

#[test]
fn a_loader_that_panics_lets_its_language_go() {
    LOADING.lock().unwrap().push(Lang::Vi);
    let ended = std::thread::spawn(|| {
        let _loading = Loading(Lang::Vi);
        panic!("a font file that breaks the loader");
    })
    .join();
    assert!(ended.is_err());
    assert!(!LOADING.lock().unwrap_or_else(PoisonError::into_inner).contains(&Lang::Vi));
}

#[test]
fn the_fonts_follow_the_language_once_per_change() {
    let ctx = egui::Context::default();
    apply(&ctx);
    fonts_for(&ctx, Lang::De);
    assert_eq!(ctx.data(|d| d.get_temp::<Lang>(fonts_key())), Some(Lang::De));
    // A new palette or test rebuilds the fonts, and the next frame loads the language's again.
    apply(&ctx);
    assert_eq!(ctx.data(|d| d.get_temp::<Lang>(fonts_key())), None);
}
