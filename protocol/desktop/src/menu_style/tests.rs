use super::*;

/// Whether `seen` is `colour` under some opacity: premultiplied, its channels keep their ratio.
fn same_colour(seen: Color32, colour: Color32) -> bool {
    let scale = seen.a() as f32 / colour.a() as f32;
    [(seen.r(), colour.r()), (seen.g(), colour.g()), (seen.b(), colour.b())]
        .iter()
        .all(|(s, c)| (*s as f32 - *c as f32 * scale).abs() <= 2.0)
}

fn run(ctx: &egui::Context, events: Vec<egui::Event>, add: impl FnMut(&mut Ui)) -> egui::FullOutput {
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(1280.0, 800.0))),
        events,
        ..Default::default()
    };
    let mut add = add;
    let mut output = ctx.run_ui(input, |ui| {
        show(ui.ctx(), "Stream menu", "menu-test", |ui| add(ui));
    });
    output.textures_delta.clear();
    output
}

/// One frame of a sample menu: the main action's and the switch's places, and whether it changed.
fn sample(ctx: &egui::Context, events: Vec<egui::Event>, on: &mut bool) -> (egui::Rect, egui::Rect, bool) {
    let mut out = (egui::Rect::NOTHING, egui::Rect::NOTHING, false);
    run(ctx, events, |ui| {
        header(ui, "Stream menu", "Game · PC");
        out.0 = primary(ui, "Return to stream", None).rect;
        let r = switch(ui, Icon::Microphone, "Mute microphone", on);
        out.1 = r.rect;
        out.2 |= r.changed();
    });
    out
}

/// The menu's main action spans its width, a switch sits at its right edge inside the row's
/// padding and turns on a click.
#[test]
fn the_menu_lays_out_across_its_width_and_its_switch_turns() {
    crate::theme::own_palette();
    let ctx = egui::Context::default();
    crate::theme::apply(&ctx);
    let mut on = false;
    sample(&ctx, Vec::new(), &mut on);
    let (wide, knob, _) = sample(&ctx, Vec::new(), &mut on);
    assert!((wide.width() - (WIDTH - 2.0 * MARGIN)).abs() < 4.0, "the main action spans the menu inside its margin: {wide:?}");
    assert!((knob.right() - (wide.right() - PAD)).abs() < 2.0, "the switch sits at the right edge: {knob:?} in {wide:?}");
    let at = knob.center();
    let click = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
    sample(&ctx, vec![egui::Event::PointerMoved(at)], &mut on);
    sample(&ctx, vec![click(true)], &mut on);
    let (_, _, changed) = sample(&ctx, vec![click(false)], &mut on);
    assert!(on && changed, "a click turns the switch on");
}

/// Hover lightens a row one step and focus rings it in amber, inside: nothing moves.
#[test]
fn a_row_lightens_on_hover_and_focus_does_not_move_it() {
    crate::theme::own_palette();
    let ctx = egui::Context::default();
    crate::theme::apply(&ctx);
    let frame = |events, focus: bool| {
        let mut rect = Rect::NOTHING;
        let output = run(&ctx, events, |ui| {
            let r = action(ui, Icon::Reconnect, "Reconnect", None);
            if focus {
                r.request_focus();
            }
            rect = r.rect;
        });
        (output.shapes, rect)
    };
    // A new window sizes itself over its first frames.
    frame(Vec::new(), false);
    frame(Vec::new(), false);
    let (_, rest) = frame(Vec::new(), false);
    let fills = |shapes: &[egui::epaint::ClippedShape]| -> Vec<(Rect, Color32)> {
        shapes.iter().filter_map(|s| if let egui::Shape::Rect(r) = &s.shape { Some((r.rect, r.fill)) } else { None }).collect()
    };
    frame(vec![egui::Event::PointerMoved(rest.center())], false);
    let (shapes, _) = frame(Vec::new(), false);
    // The window is still fading in here, so its colours carry its opacity: compare without it.
    let row = fills(&shapes).into_iter().find(|(r, _)| *r == rest).map(|(_, c)| c).expect("the row's fill");
    assert!(row.a() > 0 && row.a() <= t::TINT_CONTROL_FILL.a(), "hover lays a tint on the row: {row:?}");
    assert!(same_colour(row, t::TINT_CONTROL_FILL), "the control tint under any opacity: {row:?}");
    frame(vec![egui::Event::PointerMoved(Pos2::new(5.0, 5.0))], true);
    let (shapes, rect) = frame(Vec::new(), true);
    assert_eq!(rect, rest, "focus does not move or resize the row");
    let around = |colour: Color32| {
        shapes.iter().any(|s| matches!(&s.shape, egui::Shape::Mesh(m) if m.calc_bounds().contains_rect(rest)
            && m.calc_bounds() != rest
            && m.vertices.iter().any(|v| v.color.a() > 0 && same_colour(v.color, colour))))
    };
    assert!(around(t::FOCUS_RING_COLOR), "focus is the amber ring around the row");
    assert!(around(t::FOCUS_HALO_COLOR), "with its glow");
}

/// A disabled main action (Resume while End or Close game runs) loses its amber and says so to
/// a screen reader; enabled, it is amber.
#[test]
fn a_disabled_main_action_dims_and_says_so() {
    crate::theme::own_palette();
    let ctx = egui::Context::default();
    crate::theme::apply(&ctx);
    ctx.enable_accesskit();
    for enabled in [true, false] {
        let (mut rect, mut fill, mut disabled) = (Rect::NOTHING, None, None);
        for _ in 0..4 {
            let output = run(&ctx, Vec::new(), |ui| {
                rect = ui.add_enabled_ui(enabled, |ui| primary(ui, "Resume", None)).inner.rect;
            });
            // Its amber comes as a mesh. The window may still be fading in: compare red with blue.
            let amber = output.shapes.iter().any(|s| match &s.shape {
                egui::Shape::Mesh(m) if rect.contains_rect(m.calc_bounds().shrink(1.0)) => {
                    m.vertices.iter().any(|v| v.color.a() > 0 && f32::from(v.color.r()) > 1.5 * f32::from(v.color.b()))
                }
                _ => false,
            });
            fill = Some(amber);
            let tree = output.platform_output.accesskit_update.expect("AccessKit is on");
            disabled = tree.nodes.iter().find(|(_, n)| n.label() == Some("Resume")).map(|(_, n)| n.is_disabled()).or(disabled);
        }
        let fill = fill.expect("the action's fill");
        assert_eq!(fill, enabled, "amber only while it works");
        assert_eq!(disabled, Some(!enabled), "a screen reader hears whether Resume works");
    }
}

/// A PC command's long name stays inside the menu: it ends in an ellipsis, and the button is
/// no wider than the room.
#[test]
fn a_long_button_label_is_cut_with_an_ellipsis() {
    crate::theme::own_palette();
    let ctx = egui::Context::default();
    crate::theme::apply(&ctx);
    let long = "Restart the streaming service and reload every display profile on the living room PC";
    let (mut rect, mut room, mut drawn) = (Rect::NOTHING, 0.0, None);
    for _ in 0..3 {
        let output = run(&ctx, Vec::new(), |ui| {
            room = ui.available_width();
            rect = button(ui, long).rect;
        });
        drawn = output.shapes.iter().find_map(|s| match &s.shape { egui::Shape::Text(t) if t.galley.text() == long => Some((t.galley.elided, t.visual_bounding_rect())), _ => None });
    }
    assert!(rect.width() <= room + 0.5 && room <= WIDTH, "{rect:?} in {room}");
    let (elided, bounds) = drawn.expect("the label is drawn");
    assert!(elided, "the long label ends in an ellipsis");
    assert!(rect.contains_rect(bounds), "the label stays inside its button: {bounds:?} in {rect:?}");
    let short = run(&ctx, Vec::new(), |ui| {
        button(ui, "Run command");
    });
    assert!(short.shapes.iter().any(|s| matches!(&s.shape, egui::Shape::Text(t) if t.galley.text() == "Run command" && !t.galley.elided)), "a short label stays whole");
}

/// A shortcut's keys wider than its tile close up and stay whole: the German shortcuts page
/// cut Task-Manager's to "Ctrl + Shift +…".
#[test]
fn keys_too_wide_for_a_tile_close_up_instead_of_being_cut() {
    crate::theme::own_palette();
    let ctx = egui::Context::default();
    crate::theme::apply(&ctx);
    let texts = |width: f32, detail: &str| {
        let mut out = Vec::new();
        for _ in 0..2 {
            let output = run(&ctx, Vec::new(), |ui| {
                tile(ui, width, Icon::Keyboard, "Task-Manager", Some(detail), None, None);
            });
            out = output.shapes.iter().filter_map(|s| match &s.shape { egui::Shape::Text(t) => Some((t.galley.text().to_owned(), t.galley.elided)), _ => None }).collect();
        }
        out
    };
    let narrow = texts(100.0, "Ctrl + Shift + Esc");
    assert!(narrow.iter().all(|(_, elided)| !elided), "{narrow:?}");
    assert!(narrow.iter().any(|(text, _)| text == "Ctrl+Shift+Esc"), "{narrow:?}");
    let wide = texts(220.0, "Ctrl + Shift + Esc");
    assert!(wide.iter().any(|(text, _)| text == "Ctrl + Shift + Esc"), "room enough keeps the spaces: {wide:?}");
}

/// With "Back opens the stream menu" on, the sheet lists Select for the menu as the Help
/// page does (both through `Style::stream_press`); with it off, it doesn't.
#[test]
fn the_sheet_lists_select_for_the_controls_when_back_opens_them() {
    crate::theme::own_palette();
    let ctx = egui::Context::default();
    crate::theme::apply(&ctx);
    ctx.enable_accesskit();
    let quit = GamepadQuitCombo::StartSelectLbRb;
    let binding = shortcuts::pad_in(Scope::Stream).find(|b| b.action == Action::StreamControls).expect("the controls' buttons");
    let select = glyphs::spoken(&[Style::Xbox.piece(shortcuts::Button::Back)]);
    for back_opens in [false, true] {
        // AccessKit sends only what changed after the first frame: gather every frame's names.
        let mut names = Vec::new();
        for _ in 0..3 {
            let output = run(&ctx, Vec::new(), |ui| {
                sheet(ui, Style::Xbox, quit, back_opens, false, true);
            });
            let tree = output.platform_output.accesskit_update.expect("AccessKit is on");
            names.extend(tree.nodes.iter().filter_map(|(_, n)| n.label().or(n.value()).map(str::to_owned)));
        }
        let pieces = Style::Xbox.stream_press(binding, quit, back_opens);
        let row = format!("{}: {}", tr(Action::StreamControls.label()), glyphs::spoken(&pieces));
        assert!(names.contains(&row), "{row} on the sheet: {names:#?}");
        assert_eq!(row.ends_with(&format!(" {select}")), back_opens, "{row}");
    }
}

/// Every action the stream menu had is still in it (app/overlay.rs draws it with these pieces), and
/// the menu lists the sheet of shortcuts.
#[test]
fn the_stream_menu_keeps_every_action() {
    let source = include_str!("../app/overlay.rs");
    let start = source.find("fn draw_overlay(").expect("draw_overlay");
    let menu = &source[start..start + source[start..].find("\nfn ").unwrap_or(source.len() - start)];
    for action in [
        "s.toggle_menu(window, pads)",
        "s.overlay_visible = false",
        "s.overlay_mode) = (true, mode)",
        "s.overlay_corner",
        "s.set_video_scaling(",
        "s.mute_microphone(",
        "s.toggle_controller_mouse(",
        "s.set_host_audio(",
        "s.export_statistics = true",
        "s.clipboard(true)",
        "s.clipboard(false)",
        "s.clipboard_image(true)",
        "s.clipboard_image(false)",
        "s.send_shortcut(",
        "s.enable_adaptive_bitrate(",
        "s.set_bitrate()",
        "s.change_rates()",
        "s.health_trial(",
        "s.copy_health()",
        "s.set_quality(",
        "s.set_mode(",
        "pads.set_audio(",
        "pads.set_audio_input(",
        "usb::show(",
        "usb::show_refusal(",
        "s.server_command(",
        "s.session_page_requested = true",
        "Exit::Reconnect",
        "s.exit = Some(Exit::User)",
        "Some(stream::Sheet::FromMenu)",
        "menu_style::sheet(",
    ] {
        assert!(menu.contains(action), "the stream menu lost {action}");
    }
    // Why the PC offers no USB devices shows without opening a fold.
    assert!(!menu.contains("|ui| usb::show_refusal("), "the USB refusal is folded away");
    // Align video to top lives in the settings only.
    assert!(!menu.contains("Align video to top"), "the stream menu has no Align video to top");
}

/// nightjar#52 and #63: the grid comes first with the focus on its first tile, so A right after
/// opening never leaves; Return to stream and then Disconnect sit right under it, a step or two
/// down for a controller or remote. Reconnect is one level down, behind More.
#[test]
fn a_closed_menu_adds_nothing_to_the_stream() {
    let source = include_str!("../app/overlay.rs");
    let start = source.find("fn draw_overlay(").expect("draw_overlay");
    let overlay = &source[start..start + source[start..].find("\nfn ").unwrap_or(source.len() - start)];
    let run = overlay.find("gui.run(").expect("the pass");
    assert!(overlay[..run].contains("!s.overlay_visible && toast.is_none() && !input_only && !s.menu_visible"), "no pass at all with nothing to show");
    // The menu's glass is menu_style's; the statistics and the toast stand on the HUD's, which reads nothing behind it.
    assert_eq!(overlay.matches("glass::").count(), overlay.matches("glass::hud_surface(").count(), "the glass is menu_style's");
    assert!(!overlay.contains("LIVE_BACKDROP"), "nothing reads the stream");
    assert_eq!(overlay.matches("menu_style::show(").count(), 3);
    for at in overlay.match_indices("menu_style::show(").map(|(at, _)| at) {
        let opened = overlay[..at].rfind("if s.menu_visible").expect("under the open menu's check");
        assert!(!overlay[opened..at].contains("\n        }\n"), "the panel stands inside that check");
    }
}

#[test]
fn the_panels_shadow_falls_past_the_window() {
    let ctx = egui::Context::default();
    crate::theme::own_palette();
    crate::theme::apply(&ctx);
    let mut shapes = Vec::new();
    for _ in 0..3 {
        shapes = run(&ctx, Vec::new(), |ui| drop(ui.label("A row"))).shapes;
    }
    let wide = shapes.iter().any(|s| s.clip_rect == ctx.content_rect() && matches!(s.shape, egui::Shape::Vec(_)));
    assert!(wide, "the surface clips to the screen, not the window");
}

#[test]
fn the_heads_close_is_named_and_closes() {
    let ctx = egui::Context::default();
    crate::theme::own_palette();
    crate::theme::apply(&ctx);
    ctx.enable_accesskit();
    let mut closed = false;
    let draw = |ctx: &egui::Context, events: Vec<egui::Event>, closed: &mut bool| {
        run(ctx, events, |ui| {
            *closed |= head(ui, "Racing Game", "1080p");
        })
    };
    for _ in 0..3 {
        draw(&ctx, Vec::new(), &mut closed);
    }
    let output = draw(&ctx, Vec::new(), &mut closed);
    let nodes = output.platform_output.accesskit_update.expect("readers on").nodes;
    assert!(nodes.iter().any(|(_, node)| node.label() == Some("Return to stream")));
    let at = nodes.iter().find(|(_, node)| node.label() == Some("Return to stream")).and_then(|(_, node)| node.bounds()).expect("a place");
    let pos = egui::pos2((at.x0 + at.x1) as f32 / 2.0, (at.y0 + at.y1) as f32 / 2.0);
    draw(&ctx, vec![egui::Event::PointerMoved(pos)], &mut closed);
    let press = |pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
    draw(&ctx, vec![press(true)], &mut closed);
    draw(&ctx, vec![press(false)], &mut closed);
    assert!(closed);
}

#[test]
fn leaving_sits_right_under_the_grid() {
    let source = include_str!("../app/overlay.rs");
    let start = source.find("fn draw_overlay(").expect("draw_overlay");
    let overlay = &source[start..start + source[start..].find("
fn ").unwrap_or(source.len() - start)];
    let at = |needle: &str| overlay.find(needle).unwrap_or_else(|| panic!("{needle} in the stream controls"));
    assert!(at("stream_menu::grid(") < at("stream_menu::footer("), "the grid, then the footer");
    assert_eq!(overlay.matches("Exit::Reconnect").count(), 1, "Reconnect is drawn once");
    let menu = include_str!("../stream_menu.rs");
    let footer = &menu[menu.find("pub fn footer(").expect("the footer")..];
    let at = |needle: &str| footer.find(needle).unwrap_or_else(|| panic!("{needle} in the footer"));
    assert!(!footer[..at("tr(\"Disconnect\")")].contains("Return to stream"), "closing is the head's");
    // Once in the footer; the button's question page names it too.
    assert_eq!(footer[..footer.find("
}
").unwrap()].matches("tr(\"Disconnect\")").count(), 1, "Disconnect is drawn once");
    assert!(!overlay.contains("tr(\"Disconnect\")"), "Disconnect is the footer's");
    let grid = &menu[menu.find("pub fn grid(").expect("the grid")..];
    assert!(grid[..grid.find("
}
").unwrap()].contains("focus_first && index == 0"), "the first tile takes the focus");
}

fn shoot(dir: &std::path::Path, name: &str, width: f32, height: f32, ppp: f32, high_contrast: bool, what: &str) {
    let sheet = what == "sheet";
    let mut saved = crate::shortcuts::Shortcuts::default();
    for (name, keys) in [("Task view", &["WIN", "TAB"][..]), ("Snip", &["WIN", "SHIFT", "S"][..]), ("Steam overlay", &["SHIFT", "TAB"][..])] {
        saved.save(None, name, &keys.iter().map(|k| k.to_string()).collect::<Vec<_>>(), "desktop").unwrap();
    }
    let mut draft = crate::stream_menu::Draft::default();
    for key in ["CTRL", "ALT", "TAB"] {
        draft.toggle(key);
    }
    use crate::strings::tr;
    crate::theme::own_palette();
    crate::theme::set_high_contrast(high_contrast);
    let art = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../app/src/debug/assets/demo/art/3.jpg");
    let art = image::load_from_memory(&std::fs::read(art).expect("the demo cover")).expect("a JPEG").to_rgba8();
    let ctx = egui::Context::default();
    crate::theme::apply(&ctx);
    let picture = ctx.load_texture(
        "picture",
        egui::ColorImage::from_rgba_unmultiplied([art.width() as usize, art.height() as usize], &art),
        egui::TextureOptions::LINEAR,
    );
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
            if what == "stats" {
                // The statistics as app/overlay.rs draws them over the game: the HUD glass under the text, no menu.
                egui::Area::new(egui::Id::unique("overlay")).anchor(egui::Align2::LEFT_TOP, vec2(16.0, 16.0)).show(ui.ctx(), |ui| {
                    let place = ui.painter().add(egui::Shape::Noop);
                    let frame = egui::Frame::new().inner_margin(egui::Margin::symmetric(14, 10)).show(ui, |ui| {
                        use crate::theme;
                        ui.label(egui::RichText::new("Open World").font(theme::semibold(theme::text::LABEL)).color(theme::MOON));
                        ui.add_space(2.0);
                        egui::Grid::new("stats").num_columns(2).spacing([14.0, 3.0]).show(ui, |ui| {
                            for (k, v) in [("Video", "2560x1440 · 144 FPS · HEVC HDR"), ("Network", "4 ms · 0.0% loss"), ("Decode", "1.2 ms"),
                                ("Render", "0.8 ms"), ("Host processing", "3.1 / 3.0 / 3.8 ms")] {
                                ui.label(egui::RichText::new(k).font(theme::regular(theme::text::CAPTION)).color(theme::OUTLINE));
                                ui.label(egui::RichText::new(v).font(theme::regular(theme::text::CAPTION)).color(theme::ON_SURFACE));
                                ui.end_row();
                            }
                        });
                    });
                    let mut glass = Vec::new();
                    crate::ui::glass::hud_surface(&mut glass, ui.ctx(), frame.response.rect, 12.0);
                    ui.painter().set(place, egui::Shape::Vec(glass));
                });
                return;
            }
            if sheet {
                show(ui.ctx(), tr("Shortcuts & help"), "stream-controls", |ui| {
                    body(ui, |ui| {
                        header(ui, tr("Shortcuts & help"), "Open World · Living Room PC");
                        sheet_(ui);
                    })
                });
                return;
            }
            let available = crate::stream_menu::Available {
                mouse: Some(false), overlay: true, shortcuts: true, microphone: Some(true), quality: true, clipboard: true,
                commands: true, session: true, exits: vec![crate::session_page::HostExit::End, crate::session_page::HostExit::QuitApp], ..Default::default()
            };
            let layout = crate::stream_menu::layout(&available, saved.data.len(), &Default::default());
            let page = match what {
                "shortcuts" => crate::stream_menu::Page::Shortcuts,
                "editor" => crate::stream_menu::Page::Editor(draft.clone()),
                _ => crate::stream_menu::Page::Grid,
            };
            show(ui.ctx(), tr("Stream menu"), "stream-controls", |ui| {
                if what == "menu" {
                    // As app/overlay.rs draws the grid page: the game over its mode, the grid scrolling, the ways out pinned.
                    header(ui, "Open World", "1440p · 144 FPS · HEVC HDR");
                    let reserve = pinned_room(ui.ctx(), "shot-footer");
                    body_above(ui, reserve, |ui| crate::stream_menu::grid(ui, &layout, &saved, None, n == 0));
                    pinned(ui, "shot-footer", |ui| crate::stream_menu::footer(ui, &layout));
                    return;
                }
                body(ui, |ui| {
                    header(ui, page.title(), "Open World · Living Room PC");
                    match what {
                        "shortcuts" => {
                            crate::stream_menu::shortcuts_page(ui, &saved, n == 0);
                        }
                        "editor" => {
                            crate::stream_menu::editor(ui, &mut draft, &mut saved.clone(), false);
                        }
                        _ => {
                            crate::stream_menu::grid(ui, &layout, &saved, None, n == 0);
                            crate::stream_menu::footer(ui, &layout);
                        }
                    }
                })
            });
        });
        textures.append(output.textures_delta);
        shapes = output.shapes;
    }
    let primitives = ctx.tessellate(shapes, ppp);
    let mut canvas = crate::shot::Canvas::new((width * ppp) as usize, (height * ppp) as usize);
    canvas.update_textures(&textures);
    canvas.paint(&primitives, ppp);
    canvas.save(&dir.join(format!("{name}.png")));
    textures.clear();
    crate::theme::set_high_contrast(false);
}

fn sheet_(ui: &mut Ui) {
    sheet(ui, Style::Xbox, GamepadQuitCombo::StartSelectLbRb, false, true, true);
}

/// The stream menu's grid, its Shortcuts page, the shortcut editor and the help sheet as they open over a game, normal and high
/// contrast, and the statistics: `NIGHTJAR_SHOTS=<dir> cargo test -p nightjar-desktop stream_menu_screenshot -- --ignored`.
#[test]
#[ignore = "writes PNGs; set NIGHTJAR_SHOTS"]
fn stream_menu_screenshot() {
    let Some(dir) = std::env::var_os("NIGHTJAR_SHOTS").map(std::path::PathBuf::from) else { return };
    std::fs::create_dir_all(&dir).expect("the folder for the screenshots");
    for (width, height, ppp) in [(1280.0, 800.0, 1.5), (1920.0, 1080.0, 1.0)] {
        let size = format!("{}x{}", width as u32, height as u32);
        shoot(&dir, &format!("stream-menu-{size}"), width, height, ppp, false, "menu");
        shoot(&dir, &format!("stream-help-{size}"), width, height, ppp, false, "sheet");
        shoot(&dir, &format!("stream-shortcuts-{size}"), width, height, ppp, false, "shortcuts");
        shoot(&dir, &format!("stream-shortcut-editor-{size}"), width, height, ppp, false, "editor");
        shoot(&dir, &format!("stream-menu-{size}-high-contrast"), width, height, ppp, true, "menu");
        shoot(&dir, &format!("stream-stats-{size}"), width, height, ppp, false, "stats");
    }
}
