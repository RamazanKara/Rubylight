//! The Help page's tests: how it opens, that it lists every shortcut the key handling knows, and
//! that the launcher keys it lists do what it says.

use super::*;
use crate::config::HostRecord;
use crate::gamepad::Gamepads;
use crate::shortcuts::{KEYS, PAD};
use std::sync::Arc;

/// A launcher drawn headless, frame by frame.
struct Page {
    ctx: egui::Context,
    l: Launcher,
    hosts: Hosts,
    pads: Gamepads,
    time: f64,
    size: Vec2,
    ppp: f32,
    drawn: Vec<String>,
    access: Option<egui::accesskit::TreeUpdate>,
    textures: egui::TexturesDelta,
    shapes: Vec<egui::epaint::ClippedShape>,
}

impl Drop for Page {
    fn drop(&mut self) {
        self.textures.clear();
    }
}

fn record(name: &str) -> HostRecord {
    HostRecord { unique_id: name.into(), name: name.into(), address: "192.0.2.7".into(), server_certificate: Some("pem".into()), manual: true, ..Default::default() }
}

impl Page {
    fn new(records: Vec<HostRecord>) -> Self {
        crate::theme::own_palette();
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let display = Display { width: 1920, height: 1080, refresh_hz: 60.0, refresh_reported: true };
        let mut l = Launcher::new(Settings::default(), Profiles::default(), crate::shortcuts::Shortcuts::default(), display);
        l.identity_ready = true;
        l.go(Screen::Hosts);
        let hosts = Hosts::without_discovery(records, Arc::new(|| {}));
        let mut page = Self {
            ctx,
            l,
            hosts,
            pads: Gamepads::default(),
            time: 0.0,
            size: vec2(1280.0, 720.0),
            ppp: 1.0,
            drawn: Vec::new(),
            access: None,
            textures: Default::default(),
            shapes: Vec::new(),
        };
        page.settle();
        page
    }

    fn frame(&mut self, events: Vec<egui::Event>) -> Vec<Command> {
        self.time += 0.05;
        let mut input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.size)),
            time: Some(self.time),
            events,
            focused: true,
            ..Default::default()
        };
        input.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(self.ppp);
        let mut commands = Vec::new();
        let output = self.ctx.run_ui(input, |ui| {
            commands = self.l.ui(ui, View { hosts: &self.hosts, pads: &self.pads, connecting: None, waking: None });
        });
        self.access = output.platform_output.accesskit_update.clone();
        self.textures.append(output.textures_delta);
        self.drawn.clear();
        for clipped in &output.shapes {
            texts(&clipped.shape, &mut self.drawn);
        }
        self.shapes = output.shapes;
        commands
    }

    fn settle(&mut self) -> Vec<Command> {
        (0..4).flat_map(|_| self.frame(Vec::new())).collect()
    }

    fn press_with(&mut self, key: egui::Key, modifiers: egui::Modifiers) -> Vec<Command> {
        let event = |pressed| egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers };
        let mut commands = self.frame(vec![egui::Event::ModifiersChanged(modifiers), event(true), event(false)]);
        commands.extend(self.frame(vec![egui::Event::ModifiersChanged(egui::Modifiers::NONE)]));
        commands.extend(self.settle());
        commands
    }

    fn press(&mut self, key: egui::Key) -> Vec<Command> {
        self.press_with(key, egui::Modifiers::NONE)
    }

    /// Presses a chord from the binding list as egui would see it.
    fn chord(&mut self, chord: Chord) -> Vec<Command> {
        let modifiers = egui::Modifiers { ctrl: chord.ctrl, command: chord.ctrl, alt: chord.alt, shift: chord.shift, ..Default::default() };
        self.press_with(chord.key.egui()[0], modifiers)
    }

    /// The names a screen reader has for the page's widgets.
    fn names(&self) -> Vec<String> {
        let tree = self.access.as_ref().expect("AccessKit is on");
        tree.nodes.iter().filter_map(|(_, n)| n.label().or(n.value()).map(str::to_owned)).collect()
    }

    fn shoot(&self, dir: &std::path::Path, name: &str) {
        let ppp = self.ctx.pixels_per_point();
        let primitives = self.ctx.tessellate(self.shapes.clone(), ppp);
        let mut canvas = crate::shot::Canvas::new((self.size.x * ppp) as usize, (self.size.y * ppp) as usize);
        canvas.update_textures(&self.textures);
        canvas.paint(&primitives, ppp);
        canvas.save(&dir.join(format!("{name}.png")));
    }
}

fn texts(shape: &egui::Shape, out: &mut Vec<String>) {
    match shape {
        egui::Shape::Text(text) => out.push(text.galley.text().to_owned()),
        egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| texts(s, out)),
        _ => {}
    }
}

#[test]
fn f1_opens_help_from_any_page_and_back_returns() {
    let mut page = Page::new(vec![record("Desk")]);
    page.press(egui::Key::F1);
    assert_eq!(page.l.screen, Screen::Help, "F1 on the PCs opens Help");
    assert!(page.drawn.iter().any(|t| t == tr("Keyboard shortcuts").to_uppercase().as_str()), "the page draws its sections: {:?}", page.drawn);
    page.press(egui::Key::Escape);
    assert_eq!(page.l.screen, Screen::Hosts, "Back returns to the PCs");

    // From Settings: the settings are saved, and Help replaces them as a destination does.
    page.l.switch(Screen::Settings);
    page.settle();
    let commands = page.press(egui::Key::F1);
    assert_eq!(page.l.screen, Screen::Help);
    assert!(commands.iter().any(|c| matches!(c, Command::SaveSettings)), "leaving Settings for Help saves them");

    // Over a PC's page, Back returns to the page.
    let id = page.hosts.list[0].id;
    page.l.switch(Screen::Hosts);
    page.l.open(Screen::HostDetails(id));
    page.settle();
    page.press(egui::Key::F1);
    assert_eq!(page.l.screen, Screen::Help);
    page.press(egui::Key::Escape);
    assert_eq!(page.l.screen, Screen::HostDetails(id), "Back returns to the page Help was opened over");

    // A dialog keeps its keys: F1 under it does nothing.
    page.l.switch(Screen::Hosts);
    page.l.add_host_dialog();
    page.settle();
    page.press(egui::Key::F1);
    assert_eq!(page.l.screen, Screen::Hosts, "F1 waits until the dialog is closed");
}

#[test]
fn the_question_mark_opens_help_but_types_in_a_text_field() {
    let mut page = Page::new(Vec::new());
    page.press(egui::Key::Questionmark);
    assert_eq!(page.l.screen, Screen::Help, "? opens Help where nothing takes text");

    // A focused text field keeps ?, and still lets F1 through (F1 types nothing).
    crate::theme::own_palette();
    let ctx = egui::Context::default();
    let mut l = Launcher::new(Settings::default(), Profiles::default(), crate::shortcuts::Shortcuts::default(), page.l.display);
    l.go(Screen::Hosts);
    let mut text = String::new();
    let run = |events: Vec<egui::Event>, l: &mut Launcher, text: &mut String| {
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(400.0, 300.0))), events, focused: true, ..Default::default() };
        let mut commands = Vec::new();
        let mut output = ctx.run_ui(input, |ui| {
            let field = ui.text_edit_singleline(text);
            if !field.has_focus() {
                field.request_focus();
            }
            l.help_keys(ui.ctx(), &mut commands);
        });
        output.textures_delta.clear();
    };
    run(Vec::new(), &mut l, &mut text);
    run(Vec::new(), &mut l, &mut text);
    let key = |key, pressed| egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: egui::Modifiers::NONE };
    run(vec![key(egui::Key::Questionmark, true), egui::Event::Text("?".into()), key(egui::Key::Questionmark, false)], &mut l, &mut text);
    assert_eq!(l.screen, Screen::Hosts, "? in a text field is a character");
    assert_eq!(text, "?", "and the field got it");
    run(vec![key(egui::Key::F1, true), key(egui::Key::F1, false)], &mut l, &mut text);
    assert_eq!(l.screen, Screen::Help, "F1 opens Help from a text field too");
}

/// Every binding in the list is on the Help page, with its keys or buttons, as a row a screen
/// reader reads whole: what the keys do is what the page says.
#[test]
fn every_bound_shortcut_is_listed() {
    let mut page = Page::new(Vec::new());
    page.ctx.enable_accesskit();
    page.size = vec2(1600.0, 9000.0);
    page.l.settings.controller_mouse = true;
    page.press(egui::Key::F1);
    page.settle();
    let names = page.names();
    let style = Style::of(&page.pads);
    let quit = page.l.settings.gamepad_quit_combo;
    let mut missing = Vec::new();
    for binding in KEYS.iter().cloned().chain(shortcuts::keys_in(Scope::Stream)) {
        let spoken = glyphs::spoken(&glyphs::chords(&binding.chords, false));
        let row = format!("{}: {spoken}", tr(binding.action.label()));
        if !names.contains(&row) {
            missing.push(row);
        }
    }
    for binding in PAD {
        let row = format!("{}: {}", tr(binding.action.label()), style.press_text(binding.press, quit));
        if !names.contains(&row) {
            missing.push(row);
        }
    }
    assert!(missing.is_empty(), "not on the Help page: {missing:#?}\nnames: {names:#?}");
    // The quit combo follows the settings.
    page.l.settings.gamepad_quit_combo = crate::config::GamepadQuitCombo::LbRbAB;
    page.settle();
    let combo = style.press_text(Press::QuitCombo, crate::config::GamepadQuitCombo::LbRbAB);
    assert!(page.names().contains(&format!("{}: {combo}", tr(Action::Disconnect.label()))), "the chosen quit combo is listed: {combo}");
}

/// Without a choice on the page, the controller labels follow the pads connected now: Xbox with
/// none, PlayStation once a DualSense arrives with the page open. A choice made there stays.
#[test]
fn the_controller_labels_follow_a_pad_connected_later() {
    use sdl3_sys::everything::*;
    let _sdl = crate::sdl::TEST_LOCK.lock().unwrap();
    let mut page = Page::new(Vec::new());
    page.ctx.enable_accesskit();
    page.size = vec2(1600.0, 9000.0);
    page.press(egui::Key::F1);
    page.settle();
    let quit = page.l.settings.gamepad_quit_combo;
    let row = |style: Style| format!("{}: {}", tr(Action::Disconnect.label()), style.press_text(Press::QuitCombo, quit));
    assert_ne!(row(Style::Xbox), row(Style::PlayStation));
    assert!(page.names().contains(&row(Style::Xbox)), "no pad: Xbox labels");
    // SAFETY: SDL calls on this test's thread; the descriptor outlives the attach.
    unsafe {
        assert!(SDL_Init(SDL_INIT_GAMEPAD), "{}", crate::sdl::error());
        let mut desc: SDL_VirtualJoystickDesc = std::mem::zeroed();
        desc.version = std::mem::size_of::<SDL_VirtualJoystickDesc>() as u32;
        desc.r#type = SDL_JOYSTICK_TYPE_GAMEPAD.0 as u16;
        desc.vendor_id = 0x054c;
        desc.product_id = 0x0ce6;
        desc.naxes = SDL_GAMEPAD_AXIS_COUNT.0 as u16;
        desc.nbuttons = SDL_GAMEPAD_BUTTON_COUNT.0 as u16;
        desc.button_mask = (1u32 << SDL_GAMEPAD_BUTTON_COUNT.0) - 1;
        desc.axis_mask = (1u32 << SDL_GAMEPAD_AXIS_COUNT.0) - 1;
        desc.name = c"DualSense Wireless Controller".as_ptr();
        let id = SDL_AttachVirtualJoystick(&desc);
        assert!(id.0 != 0, "{}", crate::sdl::error());
        page.pads.added(id).unwrap();
        assert_eq!(Style::of(&page.pads), Style::PlayStation, "SDL reads the pad as a PlayStation one");
        page.settle();
        assert!(page.names().contains(&row(Style::PlayStation)), "a DualSense: PlayStation labels");
        page.ctx.data_mut(|d| d.insert_temp(Id::unique("help-pad-style"), Style::Nintendo));
        page.settle();
        assert!(page.names().contains(&row(Style::Nintendo)), "a choice on the page wins");
        page.pads.close_all();
        SDL_DetachVirtualJoystick(id);
        SDL_QuitSubSystem(SDL_INIT_GAMEPAD);
    }
}

/// The launcher keys the list names, which egui's pages handle themselves, do what it says.
#[test]
fn the_launcher_keys_do_what_the_list_says() {
    let binding = |action| -> &'static [Chord] { &KEYS.iter().find(|b| b.action == action && b.scope == Scope::Launcher).expect("listed").chords };
    let mut page = Page::new(vec![record("Desk")]);
    page.chord(binding(Action::AddPc)[0]);
    assert!(matches!(page.l.dialog, Dialog::AddHost { .. }), "{:?} adds a PC", binding(Action::AddPc));
    page.press(egui::Key::Escape);
    assert!(matches!(page.l.dialog, Dialog::None));
    for chord in binding(Action::Options) {
        let mut page = Page::new(vec![record("Desk")]);
        page.chord(*chord);
        assert!(matches!(page.l.dialog, Dialog::HostOptions { .. }), "{chord:?} opens the focused PC's options");
    }
    let zoom = page.l.settings.ui_zoom;
    page.chord(binding(Action::ZoomIn)[0]);
    assert!(page.l.settings.ui_zoom > zoom, "zoom in");
    page.chord(binding(Action::ZoomOut)[0]);
    page.chord(binding(Action::ZoomOut)[0]);
    assert!(page.l.settings.ui_zoom < zoom, "zoom out");
    page.chord(binding(Action::ZoomReset)[0]);
    assert_eq!(page.l.settings.ui_zoom, 100, "reset zoom");
    page.chord(binding(Action::Help)[0]);
    assert_eq!(page.l.screen, Screen::Help);
    page.chord(binding(Action::Back)[0]);
    assert_eq!(page.l.screen, Screen::Hosts, "Esc goes back");
    // Delete and the controller's Y take a favourite's star off (ui/cards.rs).
    assert_eq!(binding(Action::Unstar), [Chord::key(shortcuts::Key::Delete)]);
    assert!(include_str!("../cards.rs").contains("egui::Key::Delete"));
}

/// Help is a top-level destination: it has its place in the header, and its links go out.
#[test]
fn help_is_in_the_header_and_its_links_open_the_guides_and_logs() {
    let mut page = Page::new(Vec::new());
    assert!(page.drawn.iter().any(|t| t == tr("Help")), "the header offers Help: {:?}", page.drawn);
    page.l.switch(Screen::Help);
    page.size = vec2(1280.0, 9000.0);
    page.settle();
    assert!(page.drawn.iter().any(|t| *t == trf("Nightjar {}", &[&crate::VERSION])), "the version is shown");
    // The first jump chip has focus, so a controller starts at the top.
    let focused = page.ctx.memory(|m| m.focused()).expect("something has focus");
    let first = page.ctx.read_response(focused).expect("the focused widget").rect;
    assert!(first.top() < 140.0, "the first chip has focus: {first:?}");
    page.ctx.enable_accesskit();
    page.settle();
    let names = page.names();
    for name in [tr("Open log folder"), tr("Online guide"), tr("Pairing guide"), tr("Release notes")] {
        assert!(names.iter().any(|n| n == name), "{name} is on the page");
    }
}

/// Only Windows asks whether Nightjar may use private networks, so only there is it the fix.
#[test]
fn the_private_network_fix_is_offered_only_on_windows() {
    let fix = PROBLEMS[0].1;
    assert_eq!(fix.contains("Windows asks"), cfg!(windows), "{fix}");
    assert!(fix.contains("Add PC"), "the address fix is offered everywhere: {fix}");
}

/// The page in German fits: every section heading and row label is translated.
#[test]
fn the_page_reads_in_german() {
    let mut page = Page::new(Vec::new());
    page.l.settings.language = crate::config::Language::German;
    page.l.switch(Screen::Help);
    page.size = vec2(1280.0, 9000.0);
    page.settle();
    for word in ["TASTENKÜRZEL", "Stream-Steuerung", "Strg"] {
        assert!(page.drawn.iter().any(|t| t.contains(word)), "{word} on the German page: {:?}", page.drawn);
    }
}

/// The Help page at 1280x800 (150 %) and 1920x1080, normal and high contrast, top and scrolled:
/// `NIGHTJAR_SHOTS=<dir> cargo test -p nightjar-desktop help_screenshots -- --ignored`.
#[test]
#[ignore = "writes PNGs; set NIGHTJAR_SHOTS"]
fn help_screenshots() {
    let Some(dir) = std::env::var_os("NIGHTJAR_SHOTS").map(std::path::PathBuf::from) else { return };
    std::fs::create_dir_all(&dir).expect("the folder for the screenshots");
    for (width, height, ppp) in [(1280.0, 800.0, 1.5), (1920.0, 1080.0, 1.0)] {
        for high in [false, true] {
            let look = if high { "-high-contrast" } else { "" };
            let size = format!("{}x{}", width as u32, height as u32);
            let mut page = Page::new(vec![record("Living Room PC")]);
            page.size = vec2(width, height);
            page.ppp = ppp;
            page.l.settings.high_contrast = high;
            page.settle();
            page.press(egui::Key::F1);
            page.settle();
            page.shoot(&dir, &format!("help-{size}{look}"));
            for (n, (_, section)) in SECTIONS.iter().enumerate().skip(1) {
                page.ctx.data_mut(|d| d.insert_temp(Id::unique("help-jump"), *section));
                page.settle();
                page.settle();
                page.shoot(&dir, &format!("help-{size}{look}-{n}-{}", section.to_lowercase().replace(' ', "-")));
            }
        }
    }
}
