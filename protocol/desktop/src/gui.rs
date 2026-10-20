//! egui on SDL3: SDL events become egui input, and egui's triangles are drawn with
//! `SDL_RenderGeometryRaw` on the same renderer that shows the video, so the launcher, the
//! stream and its overlay share one window and one swap chain.

use std::collections::HashMap;
use std::ffi::CString;
use std::time::Instant;

use egui::{epaint::Primitive, Event, Key, Modifiers, PointerButton, Pos2, Rect, TextureId, Vec2};
use sdl3_sys::everything::*;

pub struct Gui {
    pub ctx: egui::Context,
    textures: HashMap<TextureId, *mut SDL_Texture>,
    events: Vec<Event>,
    modifiers: Modifiers,
    start: Instant,
    xy: Vec<f32>,
    colors: Vec<SDL_FColor>,
    text_input: bool,
    cursor: Option<egui::CursorIcon>,
    cursors: HashMap<i32, *mut SDL_Cursor>,
    focused: bool,
}

/// Window scale: egui points to pixels, and window coordinates to pixels.
#[derive(Clone, Copy, Debug)]
pub struct Scale {
    pub pixels_per_point: f32,
    pub density: f32,
}

impl Scale {
    pub fn of(window: *mut SDL_Window) -> Self {
        // SAFETY: a live window.
        let (display, density) =
            unsafe { (SDL_GetWindowDisplayScale(window), SDL_GetWindowPixelDensity(window)) };
        let density = if density > 0.0 { density } else { 1.0 };
        let pixels_per_point = if display > 0.0 { display } else { density };
        Self {
            pixels_per_point,
            density,
        }
    }

    /// Window coordinates to egui points.
    pub fn point(&self, x: f32, y: f32) -> Pos2 {
        let f = self.density / self.pixels_per_point;
        Pos2::new(x * f, y * f)
    }
}

fn premultiplied_blend() -> SDL_BlendMode {
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
        SDLK_F11 => Key::F11,
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
        mac_cmd: false,
        command: ctrl,
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

impl Gui {
    pub fn new() -> Self {
        Self {
            ctx: egui::Context::default(),
            textures: HashMap::new(),
            events: Vec::new(),
            modifiers: Modifiers::NONE,
            start: Instant::now(),
            xy: Vec::new(),
            colors: Vec::new(),
            text_input: false,
            cursor: None,
            cursors: HashMap::new(),
            focused: true,
        }
    }

    pub fn push(&mut self, event: Event) {
        self.events.push(event);
    }

    /// Takes an SDL event. Returns whether egui used it.
    pub fn handle(&mut self, event: &SDL_Event, scale: Scale) -> bool {
        // SAFETY: the union member read matches the event type.
        unsafe {
            let kind = SDL_EventType(event.r#type);
            match kind {
                SDL_EVENT_MOUSE_MOTION => {
                    let m = event.motion;
                    self.events.push(Event::PointerMoved(scale.point(m.x, m.y)));
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
                        pos: scale.point(b.x, b.y),
                        button,
                        pressed: b.down,
                        modifiers: self.modifiers,
                    });
                }
                SDL_EVENT_MOUSE_WHEEL => {
                    let w = event.wheel;
                    let flip = if w.direction == SDL_MOUSEWHEEL_FLIPPED { -1.0 } else { 1.0 };
                    self.events.push(Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Line,
                        delta: Vec2::new(w.x * flip, w.y * flip),
                        phase: egui::TouchPhase::Move,
                        source: egui::MouseWheelSource::Unknown,
                        modifiers: self.modifiers,
                    });
                }
                SDL_EVENT_WINDOW_MOUSE_LEAVE => self.events.push(Event::PointerGone),
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
                    self.events.push(Event::WindowFocused(true));
                }
                SDL_EVENT_WINDOW_FOCUS_LOST => {
                    self.focused = false;
                    self.events.push(Event::WindowFocused(false));
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
        let (mut w, mut h) = (0, 0);
        // SAFETY: a live renderer.
        unsafe { SDL_GetCurrentRenderOutputSize(renderer, &mut w, &mut h) };
        let ppp = scale.pixels_per_point;
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
            viewport.native_pixels_per_point = Some(ppp);
        } else {
            input.viewports.insert(
                egui::ViewportId::ROOT,
                egui::ViewportInfo {
                    native_pixels_per_point: Some(ppp),
                    focused: Some(self.focused),
                    ..Default::default()
                },
            );
        }
        self.ctx.run_ui(input, ui)
    }

    /// Applies platform requests: text input, clipboard, cursor.
    pub fn platform(&mut self, window: *mut SDL_Window, output: &egui::PlatformOutput) {
        let wants_text = output.ime.is_some();
        if wants_text != self.text_input {
            self.text_input = wants_text;
            // SAFETY: a live window.
            unsafe {
                if wants_text {
                    SDL_StartTextInput(window);
                } else {
                    SDL_StopTextInput(window);
                }
            }
        }
        for command in &output.commands {
            if let egui::OutputCommand::CopyText(text) = command {
                if let Ok(text) = CString::new(text.as_str()) {
                    // SAFETY: a NUL-terminated string.
                    unsafe { SDL_SetClipboardText(text.as_ptr()) };
                }
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

    /// Uploads texture changes, draws the shapes and frees textures egui dropped.
    pub fn paint(&mut self, renderer: *mut SDL_Renderer, mut output: egui::FullOutput) {
        for (id, deltas) in output.textures_delta.set.drain() {
            for delta in deltas {
                self.set_texture(renderer, id, &delta);
            }
        }
        let ppp = output.pixels_per_point;
        let primitives = self.ctx.tessellate(std::mem::take(&mut output.shapes), ppp);
        for primitive in primitives {
            let Primitive::Mesh(mesh) = &primitive.primitive else {
                continue;
            };
            let Some(&texture) = self.textures.get(&mesh.texture_id) else {
                continue;
            };
            if mesh.indices.is_empty() {
                continue;
            }
            let clip = primitive.clip_rect;
            let rect = SDL_Rect {
                x: (clip.min.x * ppp).round() as i32,
                y: (clip.min.y * ppp).round() as i32,
                w: ((clip.max.x - clip.min.x) * ppp).round() as i32,
                h: ((clip.max.y - clip.min.y) * ppp).round() as i32,
            };
            if rect.w <= 0 || rect.h <= 0 {
                continue;
            }
            self.xy.clear();
            self.colors.clear();
            for v in &mesh.vertices {
                self.xy.push(v.pos.x * ppp);
                self.xy.push(v.pos.y * ppp);
                let [r, g, b, a] = v.color.to_array();
                self.colors.push(SDL_FColor {
                    r: f32::from(r) / 255.0,
                    g: f32::from(g) / 255.0,
                    b: f32::from(b) / 255.0,
                    a: f32::from(a) / 255.0,
                });
            }
            let stride = std::mem::size_of::<egui::epaint::Vertex>() as i32;
            let uv = &mesh.vertices[0].uv.x as *const f32;
            // SAFETY: the arrays hold `vertices.len()` entries; uv points into the vertex
            // array with the vertex stride; indices are u32.
            unsafe {
                SDL_SetRenderClipRect(renderer, &rect);
                SDL_RenderGeometryRaw(
                    renderer,
                    texture,
                    self.xy.as_ptr(),
                    8,
                    self.colors.as_ptr(),
                    std::mem::size_of::<SDL_FColor>() as i32,
                    uv,
                    stride,
                    mesh.vertices.len() as i32,
                    mesh.indices.as_ptr().cast(),
                    mesh.indices.len() as i32,
                    4,
                );
            }
        }
        // SAFETY: a live renderer.
        unsafe { SDL_SetRenderClipRect(renderer, std::ptr::null()) };
        for id in output.textures_delta.free.drain() {
            if let Some(texture) = self.textures.remove(&id) {
                // SAFETY: a texture we created.
                unsafe { SDL_DestroyTexture(texture) };
            }
        }
    }

    fn set_texture(&mut self, renderer: *mut SDL_Renderer, id: TextureId, delta: &egui::epaint::ImageDelta) {
        let egui::ImageData::Color(image) = &delta.image;
        let [w, h] = image.size;
        // SAFETY: SDL calls on a live renderer with a buffer of w*h RGBA pixels.
        unsafe {
            let texture = match (delta.pos, self.textures.get(&id)) {
                (Some(_), Some(&texture)) => texture,
                _ => {
                    if let Some(old) = self.textures.remove(&id) {
                        SDL_DestroyTexture(old);
                    }
                    let texture = SDL_CreateTexture(
                        renderer,
                        SDL_PIXELFORMAT_RGBA32,
                        SDL_TEXTUREACCESS_STATIC,
                        w as i32,
                        h as i32,
                    );
                    if texture.is_null() {
                        return;
                    }
                    SDL_SetTextureBlendMode(texture, premultiplied_blend());
                    let mode = if delta.options.magnification == egui::TextureFilter::Nearest {
                        SDL_SCALEMODE_NEAREST
                    } else {
                        SDL_SCALEMODE_LINEAR
                    };
                    SDL_SetTextureScaleMode(texture, mode);
                    self.textures.insert(id, texture);
                    texture
                }
            };
            let [x, y] = delta.pos.unwrap_or([0, 0]);
            let rect = SDL_Rect {
                x: x as i32,
                y: y as i32,
                w: w as i32,
                h: h as i32,
            };
            SDL_UpdateTexture(texture, &rect, image.pixels.as_ptr().cast(), (w * 4) as i32);
        }
    }

    /// Drops every texture, before the renderer goes away.
    pub fn destroy(&mut self) {
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
