//! The Control Center over a stream: what the stream offers goes to `stream_menu::dock`, the
//! frame is `nightjar_desktop_ui::ui::control_center`, and this file runs the ways out and keeps
//! the page between frames. `pages.rs` draws the pages and runs what is chosen on them.
//!
//! `draw_overlay` calls it only while the stream's menu is up, so a closed Control Center costs
//! the stream nothing. Open, it stands on the baked panel (solid black in high contrast): no
//! presenter hands its frame to a blur pass, so the video is never sampled.

use nightjar_desktop_ui::ui::control_center as center;
use nightjar_policy::control_center::{Gates, Live, Surface};

use super::*;

mod pages;

/// LT and RT while the Control Center is open: one step along the rail for each pull.
#[derive(Default)]
pub(crate) struct Triggers {
    held: [bool; 2],
}

impl Triggers {
    /// Past this a trigger counts as pulled.
    const PULLED: i16 = 16000;
    /// Under this it was let go, and the next pull counts again.
    const RELEASED: i16 = 8000;

    /// The left or the `right` trigger now reads `value`: Some(forward) the moment it is pulled.
    pub(crate) fn pull(&mut self, right: bool, value: i16) -> Option<bool> {
        let held = &mut self.held[usize::from(right)];
        if !*held && value > Self::PULLED {
            *held = true;
            return Some(right);
        }
        if value < Self::RELEASED {
            *held = false;
        }
        None
    }

    /// On opening: a trigger already pulled on any controller counts as held, so it steps only once
    /// let go and pulled again; one let go while the menu was closed steps with its next pull.
    fn prime(&mut self) {
        let mut held = [false; 2];
        // SAFETY: SDL gamepad queries on the main thread; the list SDL hands out is freed here.
        unsafe {
            let mut count = 0;
            let ids = SDL_GetGamepads(&mut count);
            if !ids.is_null() {
                for &id in std::slice::from_raw_parts(ids, usize::try_from(count).unwrap_or(0)) {
                    let pad = SDL_GetGamepadFromID(id);
                    if !pad.is_null() {
                        held[0] |= SDL_GetGamepadAxis(pad, SDL_GAMEPAD_AXIS_LEFT_TRIGGER) > Self::RELEASED;
                        held[1] |= SDL_GetGamepadAxis(pad, SDL_GAMEPAD_AXIS_RIGHT_TRIGGER) > Self::RELEASED;
                    }
                }
                SDL_free(ids.cast());
            }
        }
        self.held = held;
    }
}

/// What the open Control Center is painted on.
fn surface(ctx: &egui::Context) -> Surface {
    let live = Live {
        tier: nightjar_desktop_ui::live_glass::tier(ctx),
        // No presenter hands its frame to a blur pass: nothing samples the video, open or closed.
        blur_available: false,
        power_save: false,
        thermal_severe: false,
        high_contrast: theme::high_contrast(),
    };
    nightjar_policy::control_center::surface(true, &live).unwrap_or(Surface::Baked)
}

/// The modules and cards the stream offers now, with `saved` shortcuts of the user's own.
fn dock_now(s: &Streaming, overlay: &StreamOverlay, pads: &Gamepads, saved: usize) -> nightjar_policy::control_center::Dock {
    let available = stream_menu::Available {
        mouse: s.controller_mouse_offered(pads).then(|| pads.mouse_active()),
        overlay: s.overlay_visible,
        shortcuts: s.facts.role != nightjar_core::http::AppRole::RemoteMonitor,
        microphone: s.microphone_available().then_some(!s.microphone_muted),
        quality: s.facts.runtime_bitrate || s.rate_change.is_some() || s.menu_controls().size_editor
            || s.facts.role != nightjar_core::http::AppRole::InputOnly,
        clipboard: s.facts.clipboard,
        image_clipboard: s.facts.image_clipboard,
        // Only commands with a name are listed: none leaves the page with Back alone.
        commands: s.facts.server_commands.iter().any(|name| !name.is_empty()),
        session: s.session_page_available(),
        host_audio: !s.host_audio.switches().is_empty() || s.host_audio.problem().is_some(),
        controller_audio: s.controller_audio_available(),
        usb: s.usb.is_some() || s.usb_refused.is_some(),
        controller_settings: !s.closes_app,
        guests: s.guests.shown(),
        suspend: s.quick_suspend.tile(),
        dvr: s.dvr_menu().map(|[_, record]| record.op == rubylight_protocol::stream_dvr::Op::StopRecord),
        exits: page_hosts::game_exits(s, overlay.hints),
    };
    // Instant Replay's cards on Clips: the host offers Stream DVR for this stream.
    let gates = Gates { stream_dvr: available.dvr.is_some(), ..Gates::default() };
    stream_menu::dock(&available, saved, pads.menu_buttons(), &gates)
}

/// A page of the Control Center that no module draws (Session and graphics, Shortcuts & help):
/// it stands in the dock's frame with the rail at rest, as a page a card opened does, and brings
/// its own ways back, so no footer is pinned under it.
pub(crate) fn show_page(ctx: &egui::Context, s: &Streaming, overlay: &mut StreamOverlay, pads: &Gamepads, saved: usize, title: &str, line: &str, page: impl FnOnce(&mut egui::Ui)) {
    let dock = dock_now(s, overlay, pads, saved);
    let frame = center::Frame { title, line, surface: surface(ctx), input: overlay.input, sub_page: true };
    let menu_focus = &mut overlay.menu_focus;
    center::show(ctx, &mut overlay.center, &dock, &frame, |ui, _| {
        gui::follow_focus(ui, menu_focus);
        page(ui);
    }, |_| {});
}

/// One frame of the open Control Center. True when a shortcut was saved or deleted in it.
pub(crate) fn show(ctx: &egui::Context, window: *mut SDL_Window, s: &mut Streaming, overlay: &mut StreamOverlay, pads: &mut Gamepads, shortcuts: &mut shortcuts::Shortcuts, title: &str) -> bool {
    if !overlay.center.is_open() {
        overlay.triggers.prime();
    }
    let dock = dock_now(s, overlay, pads, shortcuts.data.len());
    overlay.menu_hotkeys = dock.buttons.keys().copied().collect();
    let mut page = std::mem::take(&mut overlay.menu_page);
    // More, Quality and Guests were pages of the 6.x menu: their rows are the modules' own now.
    if matches!(page, stream_menu::Page::More | stream_menu::Page::Quality | stream_menu::Page::Guests) {
        page = stream_menu::Page::Grid;
    }
    let on_module = page == stream_menu::Page::Grid;
    let question = matches!(page, stream_menu::Page::Confirm(_) | stream_menu::Page::ConfirmDisconnect);
    let (heading, line) = if on_module { s.menu_header() } else { (page.title().to_owned(), title.to_owned()) };
    // Y, X and the shoulders run what the user gave them, on whatever module's page that is.
    let pressed = overlay.menu_hotkey.take().filter(|_| on_module);
    let button = pressed.and_then(|key| stream_menu::dock_button_leave(&dock, key));
    if let Some((module, index)) = pressed.and_then(|key| dock.hotkey(key)) {
        // The page turns to the card the button ran, so what the press did shows.
        overlay.center.select(ctx, module);
        if let Some(nightjar_policy::stream_menu::Cell::Tile { id, .. }) = dock.page(module).and_then(|page| page.cards.get(index))
            && *id != stream_menu::SLIDER
        {
            ctx.memory_mut(|m| m.request_focus(stream_menu::card_id(id)));
        }
    }
    // The key or button that also returns to the stream, beside Return to stream.
    let back = Some(glyphs::spoken(&overlay.input.back(pads.swap_face_buttons))).filter(|keys| on_module && !keys.is_empty());
    let frame = center::Frame { title: &heading, line: &line, surface: surface(ctx), input: overlay.input, sub_page: !on_module };
    let mut host = pages::Host {
        ctx,
        window,
        s: &mut *s,
        pads: &mut *pads,
        shortcuts: &mut *shortcuts,
        session_page_requested: &mut overlay.session_page_requested,
        pending_exit: &mut overlay.pending_exit,
        next: None,
        select: None,
        shortcuts_changed: false,
        slider_drawn: false,
    };
    let menu_focus = &mut overlay.menu_focus;
    let mut leave = None;
    let shown = center::show(
        ctx,
        &mut overlay.center,
        &dock,
        &frame,
        |ui, module| {
            gui::follow_focus(ui, menu_focus);
            // Nothing focused: the Control Center just opened or a page changed. The first card
            // takes the focus, so A right after opening never leaves the stream.
            let focus_first = ctx.memory(|m| m.focused().is_none());
            host.page(ui, &dock, module, &mut page, pressed, focus_first);
        },
        |ui| {
            leave = if question {
                // The question has the one red control: under it only the way back to the stream stays.
                menu_style::primary(ui, tr("Return to stream"), None).clicked().then_some(stream_menu::Leave::Close)
            } else {
                stream_menu::dock_footer(ui, &dock, back.as_deref())
            };
        },
    );
    let (mut next, select, shortcuts_changed, slider_drawn) = (host.next.take(), host.select, host.shortcuts_changed, host.slider_drawn);
    if !slider_drawn {
        // The slider left the screen with a change still waiting: it goes out now.
        s.settle_bitrate_slider(nightjar_desktop_ui::stream_overlay_widgets::take_pending_bitrate_slider(ctx));
    }
    // A click on the video beside the dock returns to the stream.
    let leave = leave.or(shown.scrim_clicked.then_some(stream_menu::Leave::Close));
    // From a button Disconnect asks first, as the ways out that end the game do.
    let leave = match (leave, button) {
        (None, Some(stream_menu::Leave::Disconnect)) => {
            next = Some(stream_menu::Page::ConfirmDisconnect);
            None
        }
        (None, button) => button,
        (clicked, _) => clicked,
    };
    match leave {
        Some(stream_menu::Leave::Close) => s.toggle_menu(window, pads),
        Some(stream_menu::Leave::Disconnect) => s.exit = Some(Exit::User),
        Some(stream_menu::Leave::Exit(exit)) => next = Some(stream_menu::Page::Confirm(exit)),
        None => {}
    }
    if let Some(next) = next {
        if next == stream_menu::Page::Grid && !on_module {
            // Back on the module's page, the focus returns to the card that opened the page left.
            overlay.center.restore_focus(ctx);
        } else {
            // The new page starts with nothing focused, so its first control takes the focus.
            ctx.memory_mut(|m| m.surrender_focus(m.focused().unwrap_or(egui::Id::NULL)));
        }
        page = next;
    }
    if let Some(module) = select {
        overlay.center.select(ctx, module);
    }
    // Closing from a page (Disconnect, a shortcut) already reset it to the module's page.
    if s.menu_visible {
        overlay.menu_page = page;
    }
    overlay.sync_to_stream(s);
    shortcuts_changed
}

#[cfg(test)]
mod tests {
    use super::Triggers;

    /// A pull steps once however long it is held, and a trigger let go half way does not count
    /// as a second pull; each trigger keeps its own count.
    #[test]
    fn a_trigger_steps_once_for_each_pull() {
        let mut triggers = Triggers::default();
        assert_eq!(triggers.pull(true, 4000), None, "resting");
        assert_eq!(triggers.pull(true, 20000), Some(true), "RT steps forward");
        assert_eq!(triggers.pull(true, 32767), None, "held");
        assert_eq!(triggers.pull(true, 12000), None, "not let go yet");
        assert_eq!(triggers.pull(true, 30000), None, "the same pull");
        assert_eq!(triggers.pull(false, 32767), Some(false), "LT steps back while RT is held");
        assert_eq!(triggers.pull(true, 0), None, "let go");
        assert_eq!(triggers.pull(true, 32767), Some(true), "a digital trigger's next pull");
        assert_eq!(triggers.pull(false, 0), None);
        assert_eq!(triggers.pull(false, 17000), Some(false));
    }
}
