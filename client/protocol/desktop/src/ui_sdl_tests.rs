//! SDL3- and launcher-coupled UI integration tests moved from `desktop-ui` so `nightjar-desktop-ui`
//! stays free of `sdl3-sys` and host/launcher orchestration.

use std::sync::Arc;

use egui::{vec2, Id, Pos2, Rect, Vec2};
use nightjar_core::http::{AppRole, ClientIdentity, Feature, LaunchRequest, ServerInfo};
use nightjar_desktop_config::config::{HostDisplay, HostRecord, Profiles, Settings};
use nightjar_desktop_config::shortcuts::{hotkey_bindings, isolate_thread_hotkeys_for_test, set_hotkeys, Action};
use nightjar_desktop_config::strings::tr;
use nightjar_desktop_media::gamepad::Gamepads;
use nightjar_desktop_media::shortcuts_runtime::use_layout;
use nightjar_desktop_ui::glyphs::{Press, Style};
use nightjar_desktop_ui::hosts_model;
use nightjar_desktop_ui::pads_view::{PadBrand, PadsView};
use nightjar_desktop_ui::theme;
use nightjar_desktop_ui::ui::{
    pick_display_choice, Command, Dialog, Launcher, Screen, View, DISPLAY_CHOICES,
};

use crate::hosts::Hosts;
use crate::{animations_seen, hdr_wait_goes_on, hotkey_sdl, launch_target, read_list_for_launch, unreadable_list, window_shown, LaunchApp};

struct Page {
    ctx: egui::Context,
    l: Launcher,
    hosts: hosts_model::Hosts,
    pads: Gamepads,
    time: f64,
    size: Vec2,
    ppp: f32,
    drawn: Vec<String>,
    access: Option<egui::accesskit::TreeUpdate>,
    textures: egui::TexturesDelta,
}

impl Drop for Page {
    fn drop(&mut self) {
        self.textures.clear();
    }
}

fn texts(shape: &egui::Shape, out: &mut Vec<String>) {
    match shape {
        egui::Shape::Text(text) => out.push(text.galley.text().to_owned()),
        egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| texts(s, out)),
        _ => {}
    }
}

impl Page {
    fn new(records: Vec<HostRecord>) -> Self {
        theme::own_palette();
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let display = nightjar_desktop_config::stream_types::Display {
            width: 1920,
            height: 1080,
            refresh_hz: 60.0,
            refresh_reported: true,
        };
        let mut l = Launcher::new(
            Settings::default(),
            Profiles::default(),
            nightjar_desktop_config::shortcuts::Shortcuts::default(),
            display,
        );
        l.identity_ready = true;
        l.go(Screen::Hosts);
        let hosts = hosts_model::Hosts::without_discovery(records, Arc::new(|| {}));
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
        input.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point =
            Some(self.ppp);
        let mut commands = Vec::new();
        let pads_view: PadsView = self.pads.view();
        let output = self.ctx.run_ui(input, |ui| {
            commands = self.l.ui(
                ui,
                View {
                    hosts: &self.hosts,
                    pads: &pads_view,
                    connecting: None,
                    waking: None,
                },
            );
        });
        self.access = output.platform_output.accesskit_update.clone();
        self.textures.append(output.textures_delta);
        self.drawn.clear();
        for clipped in &output.shapes {
            texts(&clipped.shape, &mut self.drawn);
        }
        commands
    }

    fn settle(&mut self) -> Vec<Command> {
        (0..4).flat_map(|_| self.frame(Vec::new())).collect()
    }

    fn press(&mut self, key: egui::Key) -> Vec<Command> {
        let event = |pressed| egui::Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        let mut commands = self.frame(vec![event(true), event(false)]);
        commands.extend(self.settle());
        commands
    }

    fn names(&self) -> Vec<String> {
        let tree = self.access.as_ref().expect("AccessKit is on");
        tree.nodes
            .iter()
            .filter_map(|(_, n)| n.label().or(n.value()).map(str::to_owned))
            .collect()
    }
}

fn launcher() -> Launcher {
    theme::own_palette();
    let display = nightjar_desktop_config::stream_types::Display {
        width: 1920,
        height: 1080,
        refresh_hz: 60.0,
        refresh_reported: true,
    };
    Launcher::new(
        Settings::default(),
        Profiles::default(),
        nightjar_desktop_config::shortcuts::Shortcuts::default(),
        display,
    )
}

fn record(name: &str, paired: bool, _native: bool) -> HostRecord {
    HostRecord {
        unique_id: name.into(),
        name: name.into(),
        address: "192.0.2.7".into(),
        server_certificate: paired.then(|| "pem".into()),
        manual: true,
        ..Default::default()
    }
}

#[test]
fn the_controller_labels_follow_a_pad_connected_later() {
    use sdl3_sys::everything::*;
    let _sdl = nightjar_desktop_media::sdl::TEST_LOCK.lock().unwrap();
    let mut page = Page::new(Vec::new());
    page.ctx.enable_accesskit();
    page.size = vec2(1600.0, 9000.0);
    page.press(egui::Key::F1);
    page.settle();
    let quit = page.l.settings.gamepad_quit_combo;
    let row = |style: Style| {
        format!(
            "{}: {}",
            tr(Action::Disconnect.label()),
            style.press_text(Press::QuitCombo, quit)
        )
    };
    assert_ne!(row(Style::Xbox), row(Style::PlayStation));
    assert!(page.names().contains(&row(Style::Xbox)), "no pad: Xbox labels");
    // SAFETY: SDL calls on this test's thread; the descriptor outlives the attach.
    unsafe {
        assert!(SDL_Init(SDL_INIT_GAMEPAD), "{}", nightjar_desktop_media::sdl::error());
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
        assert!(id.0 != 0, "{}", nightjar_desktop_media::sdl::error());
        page.pads.added(id).unwrap();
        let brands: Vec<PadBrand> = page.pads.pads.iter().map(|p| p.brand()).collect();
        assert_eq!(Style::of_brands(brands), Style::PlayStation, "SDL reads the pad as a PlayStation one");
        page.settle();
        assert!(
            page.names().contains(&row(Style::PlayStation)),
            "a DualSense: PlayStation labels"
        );
        page.ctx
            .data_mut(|d| d.insert_temp(Id::unique("help-pad-style"), Style::Nintendo));
        page.settle();
        assert!(
            page.names().contains(&row(Style::Nintendo)),
            "a choice on the page wins"
        );
        page.pads.close_all();
        SDL_DetachVirtualJoystick(id);
        SDL_QuitSubSystem(SDL_INIT_GAMEPAD);
    }
}

#[test]
fn the_page_names_the_keys_of_a_qwertz_keyboard() {
    let mut page = Page::new(Vec::new());
    page.ctx.enable_accesskit();
    page.l.switch(Screen::Help);
    page.size = vec2(1280.0, 9000.0);
    page.settle();
    assert!(
        page.names()
            .iter()
            .any(|n| n.contains(tr(Action::Capture.label())) && n.ends_with("+Z")),
        "{:?}",
        page.names()
    );
    use_layout(&[(sdl3_sys::everything::SDL_SCANCODE_Z, sdl3_sys::everything::SDLK_Y)]);
    page.settle();
    assert!(
        page.names()
            .iter()
            .any(|n| n.contains(tr(Action::Capture.label())) && n.ends_with("+Y")),
        "{:?}",
        page.names()
    );
    assert!(page.drawn.iter().any(|t| t == "Y"), "{:?}", page.drawn);
    use_layout(&[]);
}

#[test]
fn animations_wait_while_the_window_is_away() {
    use sdl3_sys::everything::*;
    assert!(animations_seen(SDL_WINDOW_INPUT_FOCUS));
    for away in [SDL_WINDOW_MINIMIZED, SDL_WINDOW_HIDDEN, SDL_WINDOW_OCCLUDED] {
        assert!(!animations_seen(SDL_WINDOW_INPUT_FOCUS | away));
    }
    assert!(!animations_seen(SDL_WindowFlags(0)), "in the background");
    assert!(window_shown(SDL_WindowFlags(0)) && window_shown(SDL_WINDOW_OCCLUDED));
    assert!(!window_shown(SDL_WINDOW_MINIMIZED) && !window_shown(SDL_WINDOW_HIDDEN));
}

#[test]
fn the_hotkey_dialog_records_the_keys_sdl_reports() {
    use sdl3_sys::everything::*;
    isolate_thread_hotkeys_for_test();
    nightjar_desktop_media::shortcuts_runtime::register_hooks();
    let bound = |id: &str| hotkey_bindings().get(id).cloned();
    let mut l = launcher();
    l.go(Screen::StreamHotkeys);
    l.dialog = Dialog::StreamHotkey { id: "quit", error: None };
    assert!(l.recording_hotkey());
    assert!(
        l.hotkey_input(hotkey_sdl::hotkey_input(SDL_SCANCODE_LCTRL, SDL_KMOD_LCTRL, false))
            .is_none()
            && l.recording_hotkey(),
        "Ctrl alone waits"
    );
    assert!(matches!(
        l.hotkey_input(hotkey_sdl::hotkey_input(SDL_SCANCODE_F9, SDL_KMOD_LCTRL, false)),
        Some(Command::SaveHotkeys)
    ));
    assert!(!l.recording_hotkey());
    assert_eq!(bound("quit").as_deref(), Some("CTRL+F9"));
    l.dialog = Dialog::StreamHotkey { id: "capture", error: None };
    assert!(l
        .hotkey_input(hotkey_sdl::hotkey_input(SDL_SCANCODE_1, SDL_KMOD_LSHIFT, false))
        .is_none());
    assert!(matches!(&l.dialog, Dialog::StreamHotkey { error: Some(e), .. } if e == tr("This combination is for typing.")));
    assert!(
        l.hotkey_input(hotkey_sdl::hotkey_input(
            SDL_SCANCODE_1,
            SDL_Keymod(SDL_KMOD_LCTRL.0 | SDL_KMOD_LSHIFT.0),
            true
        ))
        .is_none(),
        "a repeat is no new press"
    );
    assert!(l
        .hotkey_input(hotkey_sdl::hotkey_input(
            SDL_SCANCODE_1,
            SDL_Keymod(SDL_KMOD_LCTRL.0 | SDL_KMOD_LSHIFT.0),
            false
        ))
        .is_some());
    assert_eq!(bound("capture").as_deref(), Some("CTRL+SHIFT+1"));
    l.dialog = Dialog::StreamHotkey { id: "overlay", error: None };
    assert!(l
        .hotkey_input(hotkey_sdl::hotkey_input(SDL_SCANCODE_F11, SDL_KMOD_NONE, false))
        .is_some());
    assert_eq!(bound("overlay").as_deref(), Some("CTRL+ALT+SHIFT+F11"));
    let source = include_str!("app/launcher.rs");
    let at = |s: &str| source.find(s).unwrap_or_else(|| panic!("{s}"));
    assert!(
        at("SDL_EVENT_KEY_DOWN if self.launcher.recording_hotkey()") < at("== Some(Action::Fullscreen)"),
        "recorded before F11 and F12"
    );
    l.dialog = Dialog::StreamHotkey { id: "minimize", error: None };
    assert!(l
        .hotkey_input(hotkey_sdl::hotkey_input(SDL_SCANCODE_ESCAPE, SDL_KMOD_NONE, false))
        .is_none());
    assert!(!l.recording_hotkey() && bound("minimize").is_none());
    set_hotkeys(Default::default());
}

#[test]
fn arrows_and_enter_alone_move_in_the_hotkey_dialog() {
    use sdl3_sys::everything::*;
    isolate_thread_hotkeys_for_test();
    nightjar_desktop_media::shortcuts_runtime::register_hooks();
    let bound = |id: &str| hotkey_bindings().get(id).cloned();
    let mut l = launcher();
    l.go(Screen::StreamHotkeys);
    l.dialog = Dialog::StreamHotkey { id: "capture", error: None };
    for key in [
        SDL_SCANCODE_UP,
        SDL_SCANCODE_DOWN,
        SDL_SCANCODE_LEFT,
        SDL_SCANCODE_RIGHT,
        SDL_SCANCODE_RETURN,
        SDL_SCANCODE_KP_ENTER,
    ] {
        assert!(hotkey_sdl::moves_in_hotkey_dialog(key, SDL_KMOD_NONE), "{}", key.0);
        assert!(
            l.hotkey_input(hotkey_sdl::hotkey_input(key, SDL_KMOD_NONE, false))
                .is_none(),
            "{}",
            key.0
        );
        assert!(matches!(&l.dialog, Dialog::StreamHotkey { error: None, .. }), "{}", key.0);
        assert!(bound("capture").is_none(), "{}", key.0);
    }
    let source = include_str!("app/launcher.rs");
    assert!(
        source.contains("SDL_EVENT_KEY_DOWN if self.launcher.recording_hotkey() && !hotkey_sdl::moves_in_hotkey_dialog("),
        "egui gets them"
    );
    assert!(!hotkey_sdl::moves_in_hotkey_dialog(SDL_SCANCODE_TAB, SDL_KMOD_NONE));
    assert!(!hotkey_sdl::moves_in_hotkey_dialog(SDL_SCANCODE_RETURN, SDL_KMOD_LCTRL));
    assert!(l
        .hotkey_input(hotkey_sdl::hotkey_input(
            SDL_SCANCODE_RETURN,
            SDL_Keymod(SDL_KMOD_LCTRL.0 | SDL_KMOD_LALT.0),
            false
        ))
        .is_some());
    assert_eq!(bound("capture").as_deref(), Some("CTRL+ALT+ENTER"));
    l.dialog = Dialog::StreamHotkey { id: "overlay", error: None };
    assert!(l
        .hotkey_input(hotkey_sdl::hotkey_input(
            SDL_SCANCODE_LEFT,
            SDL_Keymod(SDL_KMOD_LCTRL.0 | SDL_KMOD_LALT.0),
            false
        ))
        .is_some());
    assert_eq!(bound("overlay").as_deref(), Some("CTRL+ALT+DPAD_LEFT"));
    set_hotkeys(Default::default());
}

#[test]
fn a_start_waits_for_the_hdr_report_up_to_its_deadline() {
    let now = std::time::Instant::now();
    let mut wait = None;
    assert!(hdr_wait_goes_on(&mut wait, now, true));
    assert!(
        hdr_wait_goes_on(&mut wait, now + std::time::Duration::from_millis(400), true),
        "the deadline is the first check's"
    );
    assert!(
        !hdr_wait_goes_on(&mut wait, now + std::time::Duration::from_millis(600), true),
        "past it the start goes on"
    );
    let mut wait = None;
    assert!(
        !hdr_wait_goes_on(&mut wait, now, false),
        "a report that cannot come is not waited for"
    );
}

#[test]
fn a_launch_with_no_list_read_loads_it_or_fails_out_loud() {
    let mut hosts = Hosts::without_discovery(vec![record("Den", true, true)], Arc::new(|| {}));
    let id = hosts.list[0].id;
    assert!(matches!(launch_target(&hosts.list[0], 5, false), LaunchApp::Gone));
    assert!(!read_list_for_launch(&mut hosts, id), "no key, no read");
    assert_eq!(
        unreadable_list(&hosts, id),
        "This device’s pairing key is not ready yet. Try again in a moment."
    );
    assert_eq!(unreadable_list(&hosts, id + 1), "This PC is no longer in your list.");
    hosts.set_identity(Arc::new(ClientIdentity::generate().expect("a key")));
    assert!(read_list_for_launch(&mut hosts, id), "the launch holds for this read");
    assert!(matches!(launch_target(&hosts.list[0], 5, false), LaunchApp::Wait));
}

#[test]
fn an_arrangement_pick_leaves_what_remote_monitor_asks_for() {
    let xml = "<?xml version=\"1.0\" encoding=\"utf-8\"?><root status_code=\"200\"><hostname>PC</hostname><uniqueid>uid</uniqueid><HttpsPort>47984</HttpsPort><ExternalPort>47989</ExternalPort><PairStatus>1</PairStatus><currentgame>0</currentgame><state>SUNSHINE_SERVER_FREE</state><Permission>1</Permission><RustHostVersion>3.0.0</RustHostVersion><VirtualDisplayCapable>true</VirtualDisplayCapable><VirtualDisplayDriverReady>true</VirtualDisplayDriverReady></root>";
    let caps = ServerInfo::parse(xml, 47989).expect("the answer parses").capabilities;
    assert!(caps.has(Feature::HostDisplayModes));
    let asks = |s: &Settings, role: AppRole| {
        LaunchRequest {
            role,
            virtual_display: s.virtual_display,
            host_display: s.host_display.into(),
            ..LaunchRequest::default()
        }
        .wants_virtual_display(&caps)
    };
    for mode in [
        HostDisplay::HostDefault,
        HostDisplay::Exclusive,
        HostDisplay::ExtendedPrimary,
        HostDisplay::Extended,
        HostDisplay::ExtendedIsolated,
        HostDisplay::ExtendedPrimaryIsolated,
        HostDisplay::Physical,
    ] {
        for virtual_display in [false, true] {
            for choice in DISPLAY_CHOICES.into_iter().filter(|c| c.0 != HostDisplay::HostDefault) {
                let mut s = Settings {
                    host_display: mode,
                    virtual_display,
                    ..Settings::default()
                };
                let before = asks(&s, AppRole::RemoteMonitor);
                assert_eq!(before, virtual_display, "Remote Monitor follows the switch alone");
                pick_display_choice(&mut s, choice);
                assert_eq!(
                    asks(&s, AppRole::RemoteMonitor),
                    before,
                    "{mode:?}/{virtual_display} -> {choice:?}"
                );
                assert_eq!(
                    asks(&s, AppRole::Stream),
                    choice.1,
                    "a stream gets the arrangement's display"
                );
            }
        }
    }
}
