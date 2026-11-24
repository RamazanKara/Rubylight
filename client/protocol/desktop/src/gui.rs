//! egui on SDL3: SDL events become egui input, and egui's triangles are drawn with
//! `SDL_RenderGeometryRaw` on the same renderer that shows the video, so the launcher, the
//! stream and its overlay share one window and one swap chain.

mod stage;

use std::collections::HashMap;
use std::ffi::CString;
use std::time::{Duration, Instant};

use egui::{Event, Key, Modifiers, PointerButton, Pos2, Rect, TextureId, Vec2};
use sdl3_sys::everything::*;

#[cfg(test)]
use crate::live_glass::{Stage, LIVE_BACKDROP};
use crate::live_glass::{self, Chain, Glass, Tier};

pub struct Gui {
    pub ctx: egui::Context,
    textures: HashMap<TextureId, *mut SDL_Texture>,
    events: Vec<Event>,
    modifiers: Modifiers,
    start: Instant,
    xy: Vec<f32>,
    colors: Vec<SDL_FColor>,
    cursor: Option<egui::CursorIcon>,
    cursors: HashMap<i32, *mut SDL_Cursor>,
    focused: bool,
    /// Screen readers, once attached to the window (a11y.rs).
    a11y: Option<crate::a11y::Accessibility>,
    /// Whether the coming passes are worth a screen reader's tree (see [`Self::set_readable`]).
    readable: bool,
    /// When the last pass ran and when egui wants the next one (see [`Self::repaint_in`]).
    ran: Instant,
    repaint_delay: Duration,
    /// Which tier glass lands on, and the render targets of its live blur.
    glass: Glass,
    chain: Chain,
    /// The tier the context holds, so a stream's frames do not write it again.
    tier_set: Option<Tier>,
    no_output_said: bool,
    /// The render state the meshes drawn into the window's target take (`set_window_state`); null
    /// for SDL's own.
    window_state: *mut SDL_GPURenderState,
    /// The print of what the window shows; None after a frame that was not the launcher's.
    on_screen: Option<u64>,
    /// The stream overlay's last pass (see [`Self::overlay_paint`]).
    overlay: OverlayShown,
}

/// What a stream overlay pass drew: the digest of its inputs, and the shapes themselves.
#[derive(Default)]
struct OverlayShown {
    digest: Option<u64>,
    primitives: Vec<egui::epaint::ClippedPrimitive>,
    ppp: f32,
}

/// Whether an overlay pass runs egui, or repaints the shapes the last pass drew. Egui runs when a
/// screen that takes input is up (`live`), when input reached egui, when egui asked for another
/// pass, or when what the pass draws (`digest`, against `shown`) differs from the last one's.
pub fn overlay_must_run(shown: Option<u64>, digest: u64, live: bool, input: bool, animating: bool) -> bool {
    live || input || animating || shown != Some(digest)
}

/// The system clipboard's text, or None when it holds none.
pub fn clipboard_text() -> Option<String> {
    // SAFETY: SDL is initialised before the launcher runs; the string SDL returns is freed here.
    unsafe {
        let text = SDL_GetClipboardText();
        if text.is_null() {
            return None;
        }
        let pasted = std::ffi::CStr::from_ptr(text).to_string_lossy().into_owned();
        SDL_free(text.cast());
        Some(pasted).filter(|t| !t.is_empty())
    }
}

/// Window scale: egui points to pixels, and window coordinates to pixels.
#[derive(Clone, Copy, Debug)]
pub struct Scale {
    pub pixels_per_point: f32,
    pub density: f32,
    /// The client area in window coordinates, which SDL's mouse positions and its touches
    /// normalized to 0...1 are measured in.
    pub size: Vec2,
}

impl Scale {
    pub fn of(window: *mut SDL_Window) -> Self {
        let (mut w, mut h) = (0, 0);
        // SAFETY: a live window.
        let (display, density) = unsafe {
            SDL_GetWindowSize(window, &mut w, &mut h);
            (SDL_GetWindowDisplayScale(window), SDL_GetWindowPixelDensity(window))
        };
        let density = if density > 0.0 { density } else { 1.0 };
        let pixels_per_point = if display > 0.0 { display } else { density };
        Self {
            pixels_per_point,
            density,
            size: Vec2::new(w.max(0) as f32, h.max(0) as f32),
        }
    }

    /// Window coordinates to egui points.
    pub fn point(&self, x: f32, y: f32) -> Pos2 {
        let f = self.density / self.pixels_per_point;
        Pos2::new(x * f, y * f)
    }
}

/// The monitor's size in egui points (the OS scale divided out): Auto's zoom follows it, and it
/// is read every frame, so docking or undocking changes the zoom without a restart.
fn monitor_size(window: *mut SDL_Window, scale: &Scale) -> Option<Vec2> {
    // SAFETY: a live window; the mode pointer is owned by SDL.
    unsafe {
        let mode = SDL_GetCurrentDisplayMode(SDL_GetDisplayForWindow(window));
        if mode.is_null() {
            return None;
        }
        let mode = &*mode;
        let density = if mode.pixel_density > 0.0 { mode.pixel_density } else { 1.0 };
        let points = if scale.pixels_per_point > 0.0 { scale.pixels_per_point } else { 1.0 };
        Some(Vec2::new(mode.w.max(0) as f32 * density / points, mode.h.max(0) as f32 * density / points))
    }
}

pub(crate) fn premultiplied_blend() -> SDL_BlendMode {
    SDL_ComposeCustomBlendMode(
        SDL_BLENDFACTOR_ONE,
        SDL_BLENDFACTOR_ONE_MINUS_SRC_ALPHA,
        SDL_BLENDOPERATION_ADD,
        SDL_BLENDFACTOR_ONE_MINUS_DST_ALPHA,
        SDL_BLENDFACTOR_ONE,
        SDL_BLENDOPERATION_ADD,
    )
}

fn key_of(code: SDL_Keycode) -> Option<Key> {
    Some(match code {
        SDLK_LEFT => Key::ArrowLeft,
        SDLK_RIGHT => Key::ArrowRight,
        SDLK_UP => Key::ArrowUp,
        SDLK_DOWN => Key::ArrowDown,
        SDLK_RETURN | SDLK_KP_ENTER => Key::Enter,
        SDLK_ESCAPE => Key::Escape,
        SDLK_TAB => Key::Tab,
        SDLK_BACKSPACE => Key::Backspace,
        SDLK_DELETE => Key::Delete,
        SDLK_HOME => Key::Home,
        SDLK_END => Key::End,
        SDLK_PAGEUP => Key::PageUp,
        SDLK_PAGEDOWN => Key::PageDown,
        SDLK_SPACE => Key::Space,
        SDLK_INSERT => Key::Insert,
        SDLK_F1 => Key::F1,
        SDLK_F2 => Key::F2,
        SDLK_F10 => Key::F10,
        SDLK_F11 => Key::F11,
        SDLK_EQUALS => Key::Equals,
        SDLK_PLUS | SDLK_KP_PLUS => Key::Plus,
        SDLK_MINUS | SDLK_KP_MINUS => Key::Minus,
        // Shift and the key with ? on it (SDL's key codes follow the layout and Shift).
        SDLK_QUESTION => Key::Questionmark,
        _ => {
            let c = char::from_u32(code.0)?;
            if c.is_ascii_alphabetic() {
                return Key::from_name(&c.to_ascii_uppercase().to_string());
            }
            if c.is_ascii_digit() {
                return Key::from_name(&c.to_string());
            }
            return None;
        }
    })
}

fn modifiers_of(m: SDL_Keymod) -> Modifiers {
    let has = |flag: SDL_Keymod| m.0 & flag.0 != 0;
    let ctrl = has(SDL_KMOD_CTRL);
    Modifiers {
        alt: has(SDL_KMOD_ALT),
        ctrl,
        shift: has(SDL_KMOD_SHIFT),
        mac_cmd: cfg!(target_os = "macos") && has(SDL_KMOD_GUI),
        command: if cfg!(target_os = "macos") { has(SDL_KMOD_GUI) } else { ctrl },
    }
}

/// A synthetic key press and release, for gamepad navigation.
pub fn tap(key: Key) -> [Event; 2] {
    let event = |pressed| Event::Key {
        key,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: Modifiers::NONE,
    };
    [event(true), event(false)]
}

/// Scrolls the enclosing scroll area to the keyboard-focused widget when focus moves. egui
/// does not do this itself, so on a gamepad the stream menu's lower rows took focus out of sight.
/// Call it first in the scroll area's content, with the id it returned last time.
pub fn follow_focus(ui: &mut egui::Ui, last: &mut Option<egui::Id>) {
    follow_focus_inset(ui, last, 0.0);
}

/// [`follow_focus`] for content that scrolls beneath chrome: the widget comes to rest `top_inset`
/// below the area's top edge.
fn follow_focus_inset(ui: &mut egui::Ui, last: &mut Option<egui::Id>, top_inset: f32) {
    let focused = ui.ctx().memory(|m| m.focused());
    if focused == *last {
        return;
    }
    *last = focused;
    if let Some(response) = focused.and_then(|id| ui.ctx().read_response(id))
        && holds(ui, &response) {
            ui.scroll_to_rect(response.rect.with_min_y(response.rect.top() - top_inset), None);
        }
}

/// Whether the scroll area whose content `ui` is holds the widget: not a header or a side list
/// beside it. A widget still showing is clipped to its own area, so one showing outside this
/// area's view is another's. One out of sight (clipped away) is this area's when it lies in its
/// content: that starts at the area's top edge less the scroll offset, nothing of it above or beside.
fn holds(ui: &egui::Ui, response: &egui::Response) -> bool {
    // Chrome floats in its own layer, over the area and out of its hands.
    if response.layer_id != ui.layer_id() {
        return false;
    }
    if response.interact_rect.is_positive() {
        return ui.clip_rect().contains_rect(response.interact_rect);
    }
    let (rect, content) = (response.rect, ui.max_rect());
    content.x_range().contains(rect.center().x) && rect.center().y >= content.top()
}

/// Whether Text and button size is Auto, kept in egui's memory by the launcher for the passes that
/// run without it (keyed by type, as `CoversUsed` is).
#[derive(Clone, Copy, Default)]
pub(crate) struct AutoZoom(pub bool);

impl Gui {
    /// While the size is Auto, sizes the next pass for the screen the window is on now: an Exclusive
    /// fullscreen stream switches the display to the stream's mode, and the overlay drawn over it must
    /// follow that mode, not the desktop's the launcher last saw.
    pub(crate) fn follow_auto_zoom(&self, window: *mut SDL_Window) {
        let auto = self.ctx.data(|d| d.get_temp::<AutoZoom>(egui::Id::NULL)).unwrap_or_default();
        if !auto.0 {
            return;
        }
        let scale = Scale::of(window);
        let height = monitor_size(window, &scale).map(|size| size.y);
        let zoom = crate::config::auto_zoom(height, scale.pixels_per_point) as f32 / 100.0;
        if (self.ctx.zoom_factor() - zoom).abs() > f32::EPSILON {
            self.ctx.set_zoom_factor(zoom);
        }
    }

    pub fn new() -> Self {
        let ctx = egui::Context::default();
        // The UI zoom is ours (85-250 % steps, saved in the settings); egui's own Ctrl+=/-/0 would
        // zoom outside those steps and never be saved.
        ctx.options_mut(|o| o.zoom_with_keyboard = false);
        Self {
            ctx,
            textures: HashMap::new(),
            events: Vec::new(),
            modifiers: Modifiers::NONE,
            start: Instant::now(),
            xy: Vec::new(),
            colors: Vec::new(),
            cursor: None,
            cursors: HashMap::new(),
            focused: true,
            a11y: None,
            readable: true,
            ran: Instant::now(),
            repaint_delay: Duration::MAX,
            glass: Glass::new(live_glass::forced_by(std::env::var("NIGHTJAR_GLASS").ok().as_deref())),
            chain: Chain::default(),
            tier_set: None,
            no_output_said: false,
            window_state: std::ptr::null_mut(),
            on_screen: None,
            overlay: OverlayShown::default(),
        }
    }

    /// Frees everything made on the renderer, which is about to go. egui keeps no images, so the
    /// textures' owners make theirs again after [`Self::renderer_changed`].
    pub fn release_renderer(&mut self) {
        self.chain.release();
        self.overlay.digest = None;
        for (_, texture) in self.textures.drain() {
            // SAFETY: textures we created, on the renderer still alive.
            unsafe { SDL_DestroyTexture(texture) };
        }
    }

    /// A new renderer: egui sends the font atlas whole on the next frame ([`resend_fonts`]), and
    /// the next frame is drawn whole, glass tried and measured again on it, as at start.
    pub fn renderer_changed(&mut self) {
        self.overlay.digest = None;
        resend_fonts(&self.ctx);
        self.glass.renderer_changed();
        self.on_screen = None;
        self.repaint_delay = Duration::ZERO;
    }

    /// Whether the next frame may time live glass's chain, which reads back from the GPU renderer
    /// (`live_glass::Glass::timing`): that frame stays in the back buffer (`sdl::read_back_frames`).
    pub fn glass_timing(&self) -> bool {
        self.glass.timing()
    }

    /// The display's refresh rate: live glass has to fit its frame.
    pub fn set_refresh(&mut self, hz: f32) {
        self.glass.set_refresh(hz);
    }

    /// Whether the coming frames draw over a stream's video. Stream frames call this first thing,
    /// also those that draw nothing else: from then on there is no chain and no live glass.
    pub fn set_over_video(&mut self, over_video: bool) {
        if self.glass.set_over_video(over_video) {
            self.chain.release();
        }
        // The stream menu and overlay keep the night look in every theme.
        crate::theme::set_over_video(&self.ctx, over_video);
    }

    /// Tells this pass's glass its tier.
    fn prepare_glass(&mut self) {
        let tier = self.glass.pass_tier();
        if self.tier_set != Some(tier) {
            self.tier_set = Some(tier);
            live_glass::set_tier(&self.ctx, tier);
        }
    }

    /// How long until egui wants its next pass, at most one per `frame` after the last: hover
    /// fades, smooth scrolling, an opening list or a layout settling after a click ask for frames
    /// of their own, without any input arriving. None while nothing animates.
    pub fn repaint_in(&self, frame: Duration) -> Option<Duration> {
        if self.repaint_delay == Duration::MAX {
            return None;
        }
        let due = self.ran + self.repaint_delay.max(frame);
        Some(due.saturating_duration_since(Instant::now()))
    }

    /// Gives screen readers the launcher and the stream menu. Call before the window is shown.
    pub fn attach_accessibility(&mut self, window: *mut SDL_Window, wake: crate::hosts::Waker) -> Result<(), String> {
        self.a11y = Some(crate::a11y::Accessibility::attach(window, wake)?);
        Ok(())
    }

    /// The launcher and a stream's menu are read to screen readers; a stream's frames without its
    /// menu (the overlay, toasts) build no tree, which would cost every frame at the stream's rate.
    pub fn set_readable(&mut self, readable: bool) {
        self.readable = readable;
    }

    pub fn push(&mut self, event: Event) {
        self.events.push(event);
    }

    /// Tells egui a touch screen is here before the first finger. A scroll area offers drag-to-scroll
    /// only once egui has seen a touch (`InputState::has_touch_screen`), and the area a first finger
    /// presses has to exist a frame before that press, so one touch that starts and ends at once is
    /// announced. Call it before the first frame when SDL reports a touch screen.
    pub fn prime_touch(&mut self, present: bool) {
        if !present {
            return;
        }
        let touch = |phase| Event::Touch {
            device_id: egui::TouchDeviceId(0),
            id: egui::TouchId(0),
            phase,
            pos: Pos2::ZERO,
            force: None,
        };
        self.events.push(touch(egui::TouchPhase::Start));
        self.events.push(touch(egui::TouchPhase::End));
    }

    /// Takes an SDL event. Returns whether egui used it.
    pub fn handle(&mut self, event: &SDL_Event, scale: Scale) -> bool {
        // Points are the zoomed ones: egui's pixels per point is the display's times the zoom.
        let zoom = self.ctx.zoom_factor();
        let point = |x: f32, y: f32| {
            let p = scale.point(x, y);
            Pos2::new(p.x / zoom, p.y / zoom)
        };
        // SAFETY: the union member read matches the event type.
        unsafe {
            let kind = SDL_EventType(event.r#type);
            match kind {
                SDL_EVENT_MOUSE_MOTION => {
                    let m = event.motion;
                    self.events.push(Event::PointerMoved(point(m.x, m.y)));
                }
                SDL_EVENT_MOUSE_BUTTON_DOWN | SDL_EVENT_MOUSE_BUTTON_UP => {
                    let b = event.button;
                    let button = match i32::from(b.button) {
                        SDL_BUTTON_LEFT => PointerButton::Primary,
                        SDL_BUTTON_RIGHT => PointerButton::Secondary,
                        SDL_BUTTON_MIDDLE => PointerButton::Middle,
                        _ => return false,
                    };
                    self.events.push(Event::PointerButton {
                        pos: point(b.x, b.y),
                        button,
                        pressed: b.down,
                        modifiers: self.modifiers,
                    });
                }
                SDL_EVENT_MOUSE_WHEEL => {
                    let w = event.wheel;
                    let (x, y) = crate::sdl::wheel_turn(&w);
                    // egui scrolls what is under the pointer it last heard of: a turn with no
                    // motion before it (the window just opened, a stream just ended, a touchpad's
                    // two fingers that move no pointer) scrolled nothing until the pointer moved.
                    self.events.push(Event::PointerMoved(point(w.mouse_x, w.mouse_y)));
                    self.events.push(Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Line,
                        delta: Vec2::new(x, y),
                        phase: egui::TouchPhase::Move,
                        source: egui::MouseWheelSource::Unknown,
                        modifiers: self.modifiers,
                    });
                }
                SDL_EVENT_WINDOW_MOUSE_LEAVE => self.events.push(Event::PointerGone),
                // SDL also sends the virtual mouse a touch makes (`SDL_TOUCH_MOUSEID`), and that
                // is what egui's pointer sees, as a mouse's would: a tap clicks and a drag scrolls
                // a scroll area, whose drag-to-scroll egui turns on once a touch has been seen.
                // The finger itself is the touch egui tracks for that, never a second pointer.
                SDL_EVENT_FINGER_DOWN | SDL_EVENT_FINGER_MOTION | SDL_EVENT_FINGER_UP | SDL_EVENT_FINGER_CANCELED => {
                    let f = event.tfinger;
                    let phase = match kind {
                        SDL_EVENT_FINGER_DOWN => egui::TouchPhase::Start,
                        SDL_EVENT_FINGER_UP => egui::TouchPhase::End,
                        SDL_EVENT_FINGER_CANCELED => egui::TouchPhase::Cancel,
                        _ => egui::TouchPhase::Move,
                    };
                    self.events.push(Event::Touch {
                        device_id: egui::TouchDeviceId(f.touchID.0),
                        id: egui::TouchId(f.fingerID.0),
                        phase,
                        pos: point(f.x * scale.size.x, f.y * scale.size.y),
                        force: Some(f.pressure.clamp(0.0, 1.0)),
                    });
                }
                SDL_EVENT_KEY_DOWN | SDL_EVENT_KEY_UP => {
                    let k = event.key;
                    self.modifiers = modifiers_of(k.r#mod);
                    if let Some(key) = key_of(k.key) {
                        if k.down && key == Key::V && self.modifiers.command {
                            let text = SDL_GetClipboardText();
                            if !text.is_null() {
                                let pasted = std::ffi::CStr::from_ptr(text).to_string_lossy().into_owned();
                                SDL_free(text.cast());
                                self.events.push(Event::Paste(pasted));
                            }
                        } else if k.down && key == Key::C && self.modifiers.command {
                            self.events.push(Event::Copy);
                        }
                        self.events.push(Event::Key {
                            key,
                            physical_key: None,
                            pressed: k.down,
                            repeat: k.repeat,
                            modifiers: self.modifiers,
                        });
                    }
                }
                SDL_EVENT_TEXT_INPUT => {
                    let text = std::ffi::CStr::from_ptr(event.text.text).to_string_lossy().into_owned();
                    if !text.chars().any(char::is_control) {
                        self.events.push(Event::Text(text));
                    }
                }
                SDL_EVENT_WINDOW_FOCUS_GAINED => {
                    self.focused = true;
                    if let Some(a11y) = &mut self.a11y { a11y.focus(true); }
                    self.events.push(Event::WindowFocused(true));
                }
                SDL_EVENT_WINDOW_FOCUS_LOST => {
                    self.focused = false;
                    if let Some(a11y) = &mut self.a11y { a11y.focus(false); }
                    self.events.push(Event::WindowFocused(false));
                }
                SDL_EVENT_RENDER_TARGETS_RESET => {
                    self.glass.invalidate();
                    return false;
                }
                SDL_EVENT_RENDER_DEVICE_RESET | SDL_EVENT_RENDER_DEVICE_LOST => {
                    self.glass.invalidate();
                    self.chain.release();
                    return false;
                }
                _ => return false,
            }
        }
        true
    }

    /// Runs one egui pass over the window.
    pub fn run(
        &mut self,
        window: *mut SDL_Window,
        renderer: *mut SDL_Renderer,
        ui: impl FnMut(&mut egui::Ui),
    ) -> egui::FullOutput {
        let scale = Scale::of(window);
        let monitor = monitor_size(window, &scale);
        let (mut w, mut h) = (0, 0);
        // SAFETY: a live renderer.
        unsafe { SDL_GetCurrentRenderOutputSize(renderer, &mut w, &mut h) };
        // The UI zoom multiplies the display scale; egui lays out in points of the product.
        let ppp = scale.pixels_per_point * self.ctx.zoom_factor();
        if let Some(a11y) = &mut self.a11y {
            a11y.begin_pass(&self.ctx, &mut self.events, self.readable);
        }
        let mut input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(
                Pos2::ZERO,
                Vec2::new(w as f32 / ppp, h as f32 / ppp),
            )),
            time: Some(self.start.elapsed().as_secs_f64()),
            events: std::mem::take(&mut self.events),
            focused: self.focused,
            max_texture_side: Some(8192),
            ..Default::default()
        };
        if let Some(viewport) = input.viewports.get_mut(&egui::ViewportId::ROOT) {
            viewport.native_pixels_per_point = Some(scale.pixels_per_point);
            viewport.monitor_size = monitor;
        } else {
            input.viewports.insert(
                egui::ViewportId::ROOT,
                egui::ViewportInfo {
                    native_pixels_per_point: Some(scale.pixels_per_point),
                    monitor_size: monitor,
                    focused: Some(self.focused),
                    ..Default::default()
                },
            );
        }
        self.prepare_glass();
        self.ran = Instant::now();
        let output = self.ctx.run_ui(input, ui);
        self.note_repaint(&output);
        output
    }

    fn note_repaint(&mut self, output: &egui::FullOutput) {
        // egui says "never" with Duration::MAX; anything past a minute is the same here.
        self.repaint_delay = output.viewport_output.get(&egui::ViewportId::ROOT)
            .map(|v| v.repaint_delay)
            .filter(|d| *d < Duration::from_secs(60))
            .unwrap_or(Duration::MAX);
    }

    /// Applies platform requests: text input, clipboard, cursor, and the screen reader's tree.
    pub fn platform(&mut self, window: *mut SDL_Window, output: &egui::PlatformOutput) {
        if let (Some(a11y), Some(update)) = (&mut self.a11y, &output.accesskit_update) {
            a11y.update(window, update);
        }
        let wants_text = output.ime.is_some();
        // SDL's own state, not a copy: a stream's text field starts and stops text input too.
        // SAFETY: a live window.
        unsafe {
            if wants_text != SDL_TextInputActive(window) {
                if wants_text {
                    SDL_StartTextInput(window);
                } else {
                    SDL_StopTextInput(window);
                }
            }
        }
        for command in &output.commands {
            if let egui::OutputCommand::CopyText(text) = command
                && let Ok(text) = CString::new(text.as_str()) {
                    // SAFETY: a NUL-terminated string.
                    unsafe { SDL_SetClipboardText(text.as_ptr()) };
                }
        }
        if self.cursor != Some(output.cursor_icon) {
            self.cursor = Some(output.cursor_icon);
            let id = match output.cursor_icon {
                egui::CursorIcon::Text => SDL_SYSTEM_CURSOR_TEXT,
                egui::CursorIcon::PointingHand => SDL_SYSTEM_CURSOR_POINTER,
                egui::CursorIcon::ResizeHorizontal => SDL_SYSTEM_CURSOR_EW_RESIZE,
                _ => SDL_SYSTEM_CURSOR_DEFAULT,
            };
            let cursor = *self
                .cursors
                .entry(id.0)
                // SAFETY: creating a system cursor.
                .or_insert_with(|| unsafe { SDL_CreateSystemCursor(id) });
            if !cursor.is_null() {
                // SAFETY: a cursor we created.
                unsafe { SDL_SetCursor(cursor) };
            }
        }
    }

    /// The launcher's frame: drawn only when it differs from what the window shows, or `must_draw`
    /// (the window itself changed). `clear` runs just before drawing. False when nothing was
    /// drawn: the caller presents nothing, so a launcher at rest uses no GPU however often it wakes.
    /// The next frame runs the blur chain again, as a moving frame does.
    #[cfg(feature = "demo-library")]
    pub fn redo_glass(&mut self) {
        self.glass.invalidate();
    }

    /// The window's size in pixels and egui's points per pixel: what an overlay pass depends on
    /// besides its content.
    pub fn surface(&self, window: *mut SDL_Window, renderer: *mut SDL_Renderer) -> ((i32, i32), f32) {
        let (mut w, mut h) = (0, 0);
        // SAFETY: a live renderer and two live integers.
        unsafe { SDL_GetCurrentRenderOutputSize(renderer, &mut w, &mut h) };
        ((w, h), Scale::of(window).pixels_per_point * self.ctx.zoom_factor())
    }

    /// The digest of the overlay pass that drew last, if any.
    pub fn overlay_shown(&self) -> Option<u64> {
        self.overlay.digest
    }

    /// Whether input reached egui since its last pass.
    pub fn input_pending(&self) -> bool {
        !self.events.is_empty()
    }

    /// Whether egui asked for another pass after its last one (an animation, a settling layout).
    pub fn animating(&self) -> bool {
        self.repaint_delay != Duration::MAX
    }

    /// The render state the passes painted from now on draw their meshes with on the window's
    /// target, until it is set to null again: a stream's video-only frame on a PQ output, where
    /// SDL drops its own draws and `stream::ui_pq` writes them as PQ. The state must outlive
    /// the frame's flush.
    pub fn set_window_state(&mut self, state: *mut SDL_GPURenderState) {
        self.window_state = state;
    }

    /// Repaints the shapes the last overlay pass drew, without running egui. Only valid while
    /// [`Self::overlay_shown`] is the digest of the frame being drawn.
    pub fn overlay_repaint(&mut self, renderer: *mut SDL_Renderer) {
        self.on_screen = None;
        let output = egui::FullOutput { pixels_per_point: self.overlay.ppp, ..Default::default() };
        let primitives = std::mem::take(&mut self.overlay.primitives);
        self.paint_tessellated(renderer, output, &primitives);
        self.overlay.primitives = primitives;
    }

    /// Draws a stream overlay pass that ran egui for `digest`, and keeps its shapes for
    /// [`Self::overlay_repaint`].
    pub fn overlay_paint(&mut self, renderer: *mut SDL_Renderer, mut output: egui::FullOutput, digest: u64) {
        self.on_screen = None;
        let ppp = output.pixels_per_point;
        let primitives = self.ctx.tessellate(std::mem::take(&mut output.shapes), ppp);
        self.overlay = OverlayShown { digest: Some(digest), primitives: primitives.clone(), ppp };
        self.paint_tessellated(renderer, output, &primitives);
    }

    pub fn paint_if_changed(&mut self, renderer: *mut SDL_Renderer, mut output: egui::FullOutput, must_draw: bool, clear: impl FnOnce()) -> bool {
        let ppp = output.pixels_per_point;
        let primitives = self.ctx.tessellate(std::mem::take(&mut output.shapes), ppp);
        let (mut w, mut h) = (0, 0);
        // SAFETY: a live renderer and two live integers.
        unsafe { SDL_GetCurrentRenderOutputSize(renderer, &mut w, &mut h) };
        let print = live_glass::frame_print(&primitives, ppp, (w, h));
        let textures = !output.textures_delta.is_empty();
        if !live_glass::frame_differs(self.on_screen, print, must_draw, textures) {
            return false;
        }
        self.on_screen = Some(print);
        clear();
        self.paint_tessellated(renderer, output, &primitives);
        true
    }

    /// Drops every texture, before the renderer goes away.
    pub fn destroy(&mut self) {
        // The subclassing adapter comes off the window before the window goes.
        self.a11y = None;
        self.chain.release();
        self.overlay.digest = None;
        for (_, texture) in self.textures.drain() {
            // SAFETY: textures we created.
            unsafe { SDL_DestroyTexture(texture) };
        }
        for (_, cursor) in self.cursors.drain() {
            // SAFETY: cursors we created.
            unsafe { SDL_DestroyCursor(cursor) };
        }
    }
}

/// Makes egui build its fonts again, so the next frame sends the font atlas whole instead of only
/// the glyphs added since. Nightjar installs no glyph rasterizers: setting none changes nothing else.
fn resend_fonts(ctx: &egui::Context) {
    ctx.set_glyph_rasterizers(Vec::new());
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod overlay_rerun_tests;
