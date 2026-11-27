//! What egui draws over the picture: the launch curtain while the game starts (`ui::launch_curtain`),
//! the stream's HUD (the statistics capsule, the toasts and the prompts of a stream without a
//! picture, `ui::stream_hud`), the Session and graphics page, the sheet of shortcuts, and the
//! Control Center (`control_center.rs` hosts it).

use super::*;

/// UI-owned in-stream menu/overlay state lifted off [`stream::Streaming`].
#[derive(Default)]
pub(crate) struct StreamOverlay {
    /// The Control Center's module and focus, kept while the stream runs.
    pub(crate) center: nightjar_desktop_ui::ui::control_center::State,
    /// What was in the hand last while the Control Center was open: its hints name that device.
    pub(crate) input: input_source::InputSource,
    pub(crate) triggers: control_center::Triggers,
    pub(crate) menu_focus: Option<egui::Id>,
    pub(crate) session_page: Option<session_page::Page>,
    pub(crate) session_link: String,
    pub(crate) session_page_requested: bool,
    pub(crate) pending_exit: Option<session_page::HostExit>,
    pub(crate) hints: session_page::Hints,
    pub(crate) menu_page: stream_menu::Page,
    pub(crate) menu_hotkey: Option<nightjar_policy::stream_menu::Hotkey>,
    pub(crate) menu_hotkeys: Vec<nightjar_policy::stream_menu::Hotkey>,
    /// The game's cover from the launcher's PC, while the launch curtain is down.
    pub(crate) curtain_art: Option<nightjar_desktop_ui::hosts::Art>,
}

impl StreamOverlay {
    pub(crate) fn close_session_page(&mut self, streaming: &mut Streaming) {
        if self.session_page.as_ref().is_some_and(session_page::Page::busy) {
            return;
        }
        self.session_page = None;
        streaming.session_page_open = false;
        streaming.session_page_busy = false;
        streaming.needs_redraw = true;
    }

    pub(crate) fn sync_to_stream(&mut self, streaming: &mut Streaming) {
        if !streaming.menu_visible {
            self.session_page = None;
            self.pending_exit = None;
            self.session_page_requested = false;
            self.menu_page = stream_menu::Page::Grid;
            self.menu_hotkey = None;
            // Closed: the next frame the Control Center draws is an opening, back where it was left.
            self.center.closed();
        }
        streaming.session_page_open = self.session_page.is_some();
        streaming.session_page_busy = self.session_page.as_ref().is_some_and(session_page::Page::busy);
    }
}

/// Whether a stream with `settings` goes on video planes, drawing straight to the display
/// (`kms_scanout::planned`).
#[cfg(target_os = "linux")]
pub(crate) fn kms_planned(settings: &config::Settings, renderer: *mut SDL_Renderer) -> bool {
    stream::kms_scanout::planned(settings, &sdl::renderer_name(renderer)).is_ok()
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn kms_planned(_settings: &config::Settings, _renderer: *mut SDL_Renderer) -> bool {
    false
}

/// Whether the display Nightjar draws straight to takes HDR10 (`kms_scanout::display_hdr`).
#[cfg(target_os = "linux")]
pub(crate) fn kms_display_hdr(window: *mut SDL_Window) -> bool {
    match stream::kms_scanout::display_hdr(window) {
        Ok(()) => true,
        Err(why) => {
            eprintln!("nightjar: kms: no HDR on video planes: {why}");
            false
        }
    }
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn kms_display_hdr(_window: *mut SDL_Window) -> bool {
    false
}

pub(crate) fn draw_overlay(gui: &mut Gui, window: *mut SDL_Window, renderer: *mut SDL_Renderer, s: &mut Streaming, overlay: &mut StreamOverlay, pads: &mut Gamepads, shortcuts: &mut shortcuts::Shortcuts) -> bool {
    // Glass holds nothing while a stream plays, and draws no live blur over its video.
    gui.set_over_video(true);
    // A stream started from the command line never drew the launcher, which loads these too.
    theme::fonts_for(&gui.ctx, strings::current());
    gui.follow_auto_zoom(window);
    let mut shortcuts_changed = false;
    // While the game is paused on the PC, its line shows where a toast does.
    let toast = s.toast.as_ref().map(|(t, _)| t.clone()).or_else(|| s.quick_suspend.banner().map(str::to_owned));
    let input_only = s.facts.role == nightjar_core::http::AppRole::InputOnly;
    let small_stream = s.small_stream_input_only();
    let mic_only = s.mic_only();
    if !s.menu_visible {
        s.settle_bitrate_slider(nightjar_desktop_ui::stream_overlay_widgets::take_pending_bitrate_slider(&gui.ctx));
    }
    let curtain = s.curtain_view();
    if !s.overlay_visible && toast.is_none() && !input_only && !s.menu_visible && curtain.is_none() {
        return false;
    }
    let mode = s.overlay_mode;
    let (anchor, offset) = overlay_anchor(s.overlay_corner);
    let visible = s.overlay_visible;
    let title = format!("{} · {}", s.facts.app_name, s.facts.host_name);
    // Screen readers get the menu and the Remote Input screen, not every frame of the overlay.
    gui.set_readable(s.menu_visible || input_only);
    // The menu and the screens that take input always run egui. The overlay repaints the shapes it
    // drew while what it shows is unchanged: its lines change once a second, not every present.
    let live = s.menu_visible || mic_only || input_only || curtain.is_some();
    let compact_device_line = (s.overlay_visible
        && mode == config::OverlayMode::Compact
        && s.compact_overlay_device_info()
        && !s.overlay.compact.text.is_empty())
    .then(|| compact_with_device_info(s));
    let digest = overlay_digest(s, mode, live, &title, toast.as_deref(), gui.surface(window, renderer), compact_device_line.as_ref());
    if !gui::overlay_must_run(gui.overlay_shown(), digest, live, gui.input_pending(), gui.animating()) {
        gui.overlay_repaint(renderer);
        return false;
    }
    let output = gui.run(window, renderer, |ui| {
        let ctx = ui.ctx().clone();
        // The curtain in the root layer, opaque over the picture; the menu and the toasts draw over it.
        if let Some(view) = &curtain {
            let scene = ui::launch_curtain::Scene { game: &s.facts.app_name, pc: &s.facts.host_name, host_id: s.host_id, app_id: s.app().id, art: overlay.curtain_art.as_ref(), view };
            match ui::launch_curtain::show(ui, &scene) {
                Some(ui::launch_curtain::Pressed::ShowScreen) => s.curtain_show_screen(),
                Some(ui::launch_curtain::Pressed::Cancel) => s.curtain_cancel(),
                None => {}
            }
        }
        // The prompts of a stream without a picture stand on the HUD's surface, as the statistics do.
        if mic_only {
            ui::stream_hud::prompt(&ctx, "mic-only", |ui| {
                ui::stream_hud::heading(ui, tr(mic_only_title(s.microphone_muted, s.microphone_unavailable, s.microphone_busy())));
                ui::stream_hud::line(ui, &s.facts.host_name);
                ui.horizontal(|ui| {
                    // Unmuting also tries an unavailable microphone again.
                    let mute = if s.microphone_muted { tr("Unmute") } else { tr("Mute") };
                    if ui.button(mute).clicked() {
                        let muted = !s.microphone_muted;
                        s.mute_microphone(muted, pads);
                        s.needs_redraw = true;
                    }
                    if ui.button(tr("Stop")).clicked() {
                        s.exit = Some(Exit::User);
                    }
                });
            });
        } else if input_only {
            ui::stream_hud::prompt(&ctx, "input-only", |ui| {
                ui::stream_hud::heading(ui, tr("Remote Input connected"));
                ui::stream_hud::line(ui, &title);
                ui::stream_hud::line(ui, tr("Keyboard, mouse and controllers control the PC."));
                if let Some(keys) = shortcuts::chord_for(Scope::Stream, Action::Disconnect) {
                    ui::stream_hud::line(ui, &trf("{} disconnects", &[&keys.text()]));
                }
                if small_stream {
                    let mbps = format!("{:.1}", f64::from(nightjar_policy::launch::estimated_kbps()) / 1000.0);
                    ui::stream_hud::line(ui, &trf("The PC still streams small video (about {} Mbit/s); it isn’t shown.", &[&mbps]));
                }
            });
        }
        if s.menu_visible && overlay.session_page.is_some() {
            // The page stands in the Control Center's frame, as a page a card opened does.
            let lines = page_hosts::connection_lines(s, &overlay.session_link);
            let hints = overlay.hints;
            let mut outcome = session_ui::Outcome::Stay;
            if let Some(mut page) = overlay.session_page.take() {
                control_center::show_page(&ctx, s, overlay, pads, shortcuts.data.len(), tr("Session and graphics"), &title, |ui| {
                    outcome = session_ui::show(ui, &mut page, &lines, hints);
                });
                overlay.session_page = Some(page);
            }
            match outcome {
                session_ui::Outcome::Stay => {}
                session_ui::Outcome::Resume => s.toggle_menu(window, pads),
                session_ui::Outcome::HostMenu => page_hosts::open_host_menu(s, window, pads),
                session_ui::Outcome::Back => {
                    overlay.close_session_page(s);
                    // Back on the module's page, the card that opened this one has the focus again.
                    overlay.center.restore_focus(&ctx);
                }
                session_ui::Outcome::Disconnect | session_ui::Outcome::Leave => s.exit = Some(Exit::User),
            }
        }
        if s.menu_visible && overlay.session_page.is_none() && s.shortcut_sheet.is_some() {
            let from_menu = s.shortcut_sheet == Some(stream::Sheet::FromMenu);
            let style = glyphs::Style::of_brands(pads.pads.iter().map(gamepad::Pad::brand));
            let offered = s.controller_mouse_offered(pads);
            let (quit, back_opens) = (pads.quit_combo(), pads.back_opens_menu());
            let mut back = false;
            control_center::show_page(&ctx, s, overlay, pads, shortcuts.data.len(), tr("Shortcuts & help"), &title, |ui| {
                back = menu_style::sheet(ui, style, quit, back_opens, offered, from_menu);
            });
            if back {
                s.close_shortcut_sheet(window, pads);
                if from_menu {
                    overlay.center.restore_focus(&ctx);
                }
            }
        } else if s.menu_visible && overlay.session_page.is_none() {
            shortcuts_changed |= control_center::show(&ctx, window, s, overlay, pads, shortcuts, &title);
        } else {
            s.settle_bitrate_slider(nightjar_desktop_ui::stream_overlay_widgets::take_pending_bitrate_slider(&ctx));
        }
        // The HUD: shapes on `SURFACE_HUD_*`, no blur and no shadow, under the Control Center.
        if visible {
            let stats = if mode == config::OverlayMode::Compact {
                let line = compact_device_line.as_ref().unwrap_or(&s.overlay.compact);
                ui::stream_hud::Stats::Compact { mark: line.health.glyph(), health: stream::overlay::health_rgb(line.health), text: &line.text, notice: s.overlay.notice.as_deref() }
            } else {
                ui::stream_hud::Stats::Lines { title: &title, lines: s.overlay.lines(mode) }
            };
            ui::stream_hud::stats(&ctx, anchor, offset, stats);
        }
        if let Some(text) = &toast {
            ui::stream_hud::toast(&ctx, text);
        }
    });
    if s.menu_visible {
        gui.platform(window, &output.platform_output);
    }
    gui.overlay_paint(renderer, output, digest);
    shortcuts_changed
}

fn compact_with_device_info(s: &Streaming) -> stream::overlay::Compact {
    let clock = crate::startup::local_clock_hh_mm();
    s.overlay.compact.with_device_info(
        stream::overlay::battery_percent(),
        (!clock.is_empty()).then_some(clock.as_str()),
    )
}

/// Digests what the overlay pass draws: the stream's overlay state, the title and toast, the
/// language, the size and scale it draws at (`surface`) and whether a live screen is up. Equal
/// digests draw the same shapes, so `gui::overlay_must_run` can repaint them.
fn overlay_digest(s: &Streaming, mode: config::OverlayMode, live: bool, title: &str, toast: Option<&str>, surface: ((i32, i32), f32), compact_line: Option<&stream::overlay::Compact>) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    live.hash(&mut hasher);
    std::mem::discriminant(&mode).hash(&mut hasher);
    std::mem::discriminant(&s.overlay_corner).hash(&mut hasher);
    s.overlay_visible.hash(&mut hasher);
    strings::current().hash(&mut hasher);
    surface.0.hash(&mut hasher);
    surface.1.to_bits().hash(&mut hasher);
    title.hash(&mut hasher);
    toast.hash(&mut hasher);
    for (key, value) in s.overlay.lines(mode) {
        key.hash(&mut hasher);
        value.hash(&mut hasher);
    }
    let compact = compact_line.unwrap_or(&s.overlay.compact);
    compact.text.hash(&mut hasher);
    std::mem::discriminant(&compact.health).hash(&mut hasher);
    s.overlay.notice.hash(&mut hasher);
    hasher.finish()
}

/// Where the overlay sits: its corner, 16 points in from both edges.
pub(crate) fn overlay_anchor(corner: config::OverlayCorner) -> (egui::Align2, egui::Vec2) {
    use config::OverlayCorner;
    match corner {
        OverlayCorner::TopLeft => (egui::Align2::LEFT_TOP, egui::vec2(16.0, 16.0)),
        OverlayCorner::TopRight => (egui::Align2::RIGHT_TOP, egui::vec2(-16.0, 16.0)),
        OverlayCorner::BottomLeft => (egui::Align2::LEFT_BOTTOM, egui::vec2(16.0, -16.0)),
        OverlayCorner::BottomRight => (egui::Align2::RIGHT_BOTTOM, egui::vec2(-16.0, -16.0)),
    }
}

/// What a key or button does while the stream menu is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MenuStep {
    /// Not the menu's: egui or the controller navigation gets it.
    Pass,
    /// A held key's repeat of a toggle: holding it doesn't flip the menu open and shut.
    Swallow,
    ToggleSheet,
    CloseSessionPage,
    CloseSheet,
    /// Back from a page a tile opened to the grid.
    ClosePage,
    CloseMenu,
}

/// The step for a stream chord (`action`), Escape or B (`back`), with the menu's pages. Back acts
/// on what is drawn: the Session page first (the sheet never draws over it), then the sheet, then
/// a page a tile opened (`page`).
pub(crate) fn menu_step(action: Option<Action>, back: bool, repeat: bool, sheet: bool, session_page: bool, page: bool) -> MenuStep {
    let controls = action == Some(Action::StreamControls);
    if action != Some(Action::StreamShortcuts) && !back && !controls {
        return MenuStep::Pass;
    }
    if repeat {
        return MenuStep::Swallow;
    }
    if action == Some(Action::StreamShortcuts) {
        MenuStep::ToggleSheet
    } else if session_page {
        MenuStep::CloseSessionPage
    } else if back && sheet {
        MenuStep::CloseSheet
    } else if back && page {
        MenuStep::ClosePage
    } else {
        MenuStep::CloseMenu
    }
}
