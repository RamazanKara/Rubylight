//! Frame Warp on the desktop's presenter (docs/frame-warp.md). `nightjar_core::frame_warp`
//! predicts how far the picture's content has to move for the mouse and right-stick input the
//! host had not seen when it rendered the picture; this module applies that shift on every
//! present path, keeps the HUD mask's cells in place, and presents the held picture again when
//! the shift moves between pictures.
//!
//! What every path does, in fractions of the visible picture: the output point `uv` samples the
//! picture at `uv - shift`, clamped to the picture, unless the HUD mask cell `floor(uv * (width,
//! height))` is set, which samples at `uv`. The shader paths read the mask as bytes (byte `x / 8`
//! of row `y`, bit `x % 8`: `Mask::words` in little-endian order) through one `warp` row of their
//! constants, `(shift x, shift y, mask width, mask height)`; the SDL renderer path draws the same
//! mapping as triangles (`render_texture`). An identity shift takes each path's draw as it was.

use std::sync::{Arc, OnceLock};
use std::time::Instant;

use nightjar_core::frame_warp::{FrameWarp, Mask, Model, Status, Warp};
use sdl3_sys::everything::*;

use crate::config::{HostRecord, NOTICE_FRAME_WARP_UNAVAILABLE};
use crate::strings::tr;

/// The HUD masks the presenter draws, the most the protocol sends (`MAX_MASK_EDGE` cells a side).
pub const MASK_EDGE: u16 = rubylight_protocol::frame_warp::MAX_MASK_EDGE;
/// The least move of the shift, in picture pixels, that presents the held picture again.
pub const REPEAT_STEP: f32 = 0.25;
/// The most HUD rectangles the SDL renderer path draws; a finer mask is drawn coarser.
pub const MAX_HUD_RECTS: usize = 1024;

/// A HUD mask and, made on first use, its rectangles for the SDL renderer path.
#[derive(Debug)]
pub struct Hud {
    pub mask: Arc<Mask>,
    areas: OnceLock<Vec<[f32; 4]>>,
}

impl Hud {
    pub fn new(mask: Arc<Mask>) -> Self {
        Self { mask, areas: OnceLock::new() }
    }

    /// The set cells as at most `MAX_HUD_RECTS` rectangles, `[x0, y0, x1, y1]` in fractions of the
    /// picture (`hud_areas`).
    pub fn areas(&self) -> &[[f32; 4]] {
        self.areas.get_or_init(|| hud_areas(&self.mask, MAX_HUD_RECTS))
    }
}

/// What one present draws the picture with.
#[derive(Clone, Debug, Default)]
pub struct WarpDraw {
    /// Sample the picture at `uv - shift`, in fractions of the visible picture.
    pub shift: [f32; 2],
    /// The cells that stay in place; None for none.
    pub hud: Option<Arc<Hud>>,
}

impl WarpDraw {
    /// The draw for `warp`, with `hud` while the shift is on and the predictor knows a mask.
    pub fn of(warp: &Warp, hud: Option<&Arc<Hud>>) -> Self {
        if !warp.active {
            return Self::default();
        }
        Self { shift: warp.uv, hud: hud.filter(|_| warp.mask_generation.is_some()).cloned() }
    }

    /// Nothing moves: every path draws as it does without Frame Warp.
    pub fn is_identity(&self) -> bool {
        self.shift == [0.0, 0.0]
    }

    /// The shaders' `warp` row: the shift, then the mask's size in cells when `mask` (its texture
    /// is bound), else 0 for no mask. All zero for an identity draw, which the shaders skip.
    pub fn row(&self, mask: bool) -> [f32; 4] {
        if self.is_identity() {
            return [0.0; 4];
        }
        let cells = self.hud.as_ref().filter(|_| mask).map_or([0.0; 2], |hud| [f32::from(hud.mask.width), f32::from(hud.mask.height)]);
        [self.shift[0], self.shift[1], cells[0], cells[1]]
    }

    /// The HUD mask whose texture a shader path binds, when the draw shifts at all.
    pub fn mask(&self) -> Option<&Arc<Hud>> {
        self.hud.as_ref().filter(|hud| !self.is_identity() && hud.mask.width > 0 && hud.mask.height > 0)
    }
}

/// The mask as the shaders read it: rows of `Mask::stride(width) * 4` bytes, byte `x / 8` of row
/// `y` holding cell `x` in bit `x % 8`. Returns the row's bytes and the rows.
pub fn mask_bytes(mask: &Mask) -> (usize, Vec<u8>) {
    let row = Mask::stride(mask.width) * 4;
    let mut bytes: Vec<u8> = mask.words.iter().flat_map(|word| word.to_le_bytes()).collect();
    bytes.resize(row * usize::from(mask.height), 0);
    (row, bytes)
}

/// A rectangle of mask cells: its top left cell and its size in cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellRect {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
}

/// The runs of set cells in one row of `mask` (`stride` words), as (first cell, length).
fn row_runs(words: &[u32], width: u16, out: &mut Vec<(u16, u16)>) {
    out.clear();
    let width = u32::from(width);
    let mut start: Option<u32> = None;
    for (index, &word) in words.iter().enumerate() {
        let base = index as u32 * 32;
        if base >= width {
            break;
        }
        // Cells past the width are clear, so a run ends at the width at the latest.
        let valid = if width - base >= 32 { u64::from(u32::MAX) } else { (1u64 << (width - base)) - 1 };
        let bits = u64::from(word) & valid;
        let mut at = 0u32;
        while at < 32 {
            let looking = if start.is_none() { bits } else { !bits & u64::from(u32::MAX) };
            let rest = looking >> at;
            if rest == 0 {
                break;
            }
            at += rest.trailing_zeros();
            if at >= 32 {
                break;
            }
            match start.take() {
                None => start = Some(base + at),
                Some(first) => out.push((first as u16, (base + at - first) as u16)),
            }
        }
    }
    if let Some(first) = start {
        out.push((first as u16, (width - first) as u16));
    }
}

/// The set cells of `mask` as rectangles: each row's runs, a run below one of the same columns
/// growing that rectangle down. Every set cell is in exactly one rectangle.
pub fn cell_rects(mask: &Mask) -> Vec<CellRect> {
    let stride = Mask::stride(mask.width);
    let mut done = Vec::new();
    // Rectangles that reach the row above, by first column.
    let mut open: Vec<CellRect> = Vec::new();
    let mut next = Vec::new();
    let mut runs = Vec::new();
    for y in 0..mask.height {
        let start = usize::from(y) * stride;
        let Some(words) = mask.words.get(start..start + stride) else { break };
        row_runs(words, mask.width, &mut runs);
        next.clear();
        let mut above = open.drain(..).peekable();
        for &(x, w) in &runs {
            // The open rectangles left of this run end above it.
            while above.peek().is_some_and(|r| r.x < x) {
                done.push(above.next().expect("peeked"));
            }
            match above.peek() {
                Some(r) if r.x == x && r.w == w => {
                    let mut grown = above.next().expect("peeked");
                    grown.h += 1;
                    next.push(grown);
                }
                _ => next.push(CellRect { x, y, w, h: 1 }),
            }
        }
        done.extend(above);
        std::mem::swap(&mut open, &mut next);
    }
    done.append(&mut open);
    done
}

/// The HUD of `mask` as at most `limit` rectangles, `[x0, y0, x1, y1]` in fractions of the
/// picture: exact when that many are enough, else from the mask halved (`Mask::halved`) until they
/// are, which keeps a little more around the HUD in place.
pub fn hud_areas(mask: &Mask, limit: usize) -> Vec<[f32; 4]> {
    if mask.width == 0 || mask.height == 0 {
        return Vec::new();
    }
    let (width, height) = (f32::from(mask.width), f32::from(mask.height));
    let mut level = std::borrow::Cow::Borrowed(mask);
    let mut scale = 1u32;
    loop {
        let rects = cell_rects(&level);
        if rects.len() <= limit || (level.width <= 1 && level.height <= 1) {
            let edge = |cells: u16, scale: u32, size: f32| (cells as f32 * scale as f32 / size).min(1.0);
            return rects
                .iter()
                .map(|r| {
                    [edge(r.x, scale, width), edge(r.y, scale, height), edge(r.x + r.w, scale, width), edge(r.y + r.h, scale, height)]
                })
                .collect();
        }
        level = std::borrow::Cow::Owned(level.halved());
        scale *= 2;
    }
}

/// A rectangle of the destination and where it samples the picture: `at` and `uv` are
/// `[x0, y0, x1, y1]` in fractions of the destination and of the picture.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quad {
    pub at: [f32; 4],
    pub uv: [f32; 4],
}

/// The quads that draw the picture shifted by `shift` (fractions of the picture) and held at its
/// edges, where the shift uncovers the outside, then the HUD `areas` in place over it. Each
/// quad's mapping is linear: a column or row past an edge samples that edge throughout.
pub fn quads(shift: [f32; 2], areas: &[[f32; 4]]) -> Vec<Quad> {
    let cuts = |s: f32| [0.0, s.clamp(0.0, 1.0), (1.0 + s).clamp(0.0, 1.0), 1.0];
    let (xs, ys) = (cuts(shift[0]), cuts(shift[1]));
    let mut out = Vec::with_capacity(9 + areas.len());
    for row in 0..3 {
        for column in 0..3 {
            let at = [xs[column], ys[row], xs[column + 1], ys[row + 1]];
            if at[2] <= at[0] || at[3] <= at[1] {
                continue;
            }
            let uv = [
                (at[0] - shift[0]).clamp(0.0, 1.0),
                (at[1] - shift[1]).clamp(0.0, 1.0),
                (at[2] - shift[0]).clamp(0.0, 1.0),
                (at[3] - shift[1]).clamp(0.0, 1.0),
            ];
            out.push(Quad { at, uv });
        }
    }
    out.extend(areas.iter().map(|area| Quad { at: *area, uv: *area }));
    out
}

/// The vertices and indices of `quads` drawn into `destination` from `source` of a texture
/// `texture` texels large, as `SDL_RenderTexture` maps a source rectangle.
pub fn geometry(quads: &[Quad], source: SDL_FRect, texture: (f32, f32), destination: SDL_FRect) -> (Vec<SDL_Vertex>, Vec<i32>) {
    let mut vertices = Vec::with_capacity(quads.len() * 4);
    let mut indices = Vec::with_capacity(quads.len() * 6);
    let white = SDL_FColor { r: 1.0, g: 1.0, b: 1.0, a: 1.0 };
    for quad in quads {
        let first = vertices.len() as i32;
        for (x, y) in [(0, 1), (2, 1), (2, 3), (0, 3)] {
            vertices.push(SDL_Vertex {
                position: SDL_FPoint { x: destination.x + quad.at[x] * destination.w, y: destination.y + quad.at[y] * destination.h },
                color: white,
                tex_coord: SDL_FPoint {
                    x: (source.x + quad.uv[x] * source.w) / texture.0,
                    y: (source.y + quad.uv[y] * source.h) / texture.1,
                },
            });
        }
        indices.extend([first, first + 1, first + 2, first, first + 2, first + 3]);
    }
    (vertices, indices)
}

/// Says once, for the stream's life of the process, that a present path drew the picture without
/// its shift.
pub fn unshifted(path: &str, error: &str) {
    static SAID: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if !SAID.swap(true, std::sync::atomic::Ordering::Relaxed) {
        eprintln!("nightjar: Frame Warp: {path} could not shift the picture ({error}); it draws the picture unshifted");
    }
}

/// Draws `source` of `texture` (None: all of it) into `destination` as `SDL_RenderTexture` does,
/// shifted by `warp` with its HUD in place. An identity warp is `SDL_RenderTexture` itself; a
/// shifted draw SDL refuses is logged once (`unshifted`) and drawn unshifted.
///
/// # Safety
/// `renderer` and `texture` are live, on the calling thread.
pub unsafe fn render_texture(renderer: *mut SDL_Renderer, texture: *mut SDL_Texture, source: Option<&SDL_FRect>, destination: &SDL_FRect, warp: &WarpDraw) -> bool {
    let plain = || SDL_RenderTexture(renderer, texture, source.map_or(std::ptr::null(), |rect| rect as *const _), destination);
    if warp.is_identity() {
        return plain();
    }
    let (mut width, mut height) = (0.0, 0.0);
    if SDL_GetTextureSize(texture, &mut width, &mut height) && width > 0.0 && height > 0.0 {
        let source = source.copied().unwrap_or(SDL_FRect { x: 0.0, y: 0.0, w: width, h: height });
        let areas = warp.mask().map_or(&[][..], |hud| hud.areas());
        let (vertices, indices) = geometry(&quads(warp.shift, areas), source, (width, height), *destination);
        if SDL_RenderGeometry(renderer, texture, vertices.as_ptr(), vertices.len() as i32, indices.as_ptr(), indices.len() as i32) {
            return true;
        }
    }
    unshifted("SDL's renderer", &crate::sdl::error());
    plain()
}

/// When the display waits and when the next picture is expected, for `Repeat::decide`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Timing {
    pub refresh_us: u64,
    /// The present waits for the refresh (V-Sync): a repeat must leave the next picture's refresh to it.
    pub vsync: bool,
    /// When the next picture is expected: a waiting one's due time, else the held one's arrival plus a frame.
    pub next_picture_us: Option<u64>,
}

/// Presents the held picture again when the shift moved by `REPEAT_STEP` since the last present:
/// at most once a refresh, and never into the refresh the next picture is expected for.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Repeat {
    last_present_us: Option<u64>,
    last_pixels: [f32; 2],
    /// A check that found nothing to present waits until then.
    next_check_us: u64,
}

impl Repeat {
    /// A present went out at `at_us` with the shift `pixels`.
    pub fn presented(&mut self, at_us: u64, pixels: [f32; 2]) {
        self.last_present_us = Some(at_us);
        self.last_pixels = pixels;
    }

    pub fn last_pixels(&self) -> [f32; 2] {
        self.last_pixels
    }

    /// When to look again: a refresh after the last present, or the later check.
    pub fn due_us(&self, refresh_us: u64) -> Option<u64> {
        self.last_present_us.map(|last| (last + refresh_us).max(self.next_check_us))
    }

    /// Whether to present the held picture again now, its shift `pixels` now.
    pub fn decide(&mut self, now_us: u64, pixels: [f32; 2], timing: Timing) -> bool {
        let refresh = timing.refresh_us.max(1);
        let Some(due) = self.due_us(refresh) else { return false };
        if now_us < due {
            return false;
        }
        let moved = (pixels[0] - self.last_pixels[0]).hypot(pixels[1] - self.last_pixels[1]);
        if moved < REPEAT_STEP {
            self.next_check_us = now_us + refresh;
            return false;
        }
        if let Some(next) = timing.next_picture_us {
            let guard = if timing.vsync { refresh } else { refresh / 2 };
            let late = now_us > next + refresh / 2;
            if !late && next < now_us + guard {
                // The next picture takes this refresh; look again once it is late.
                self.next_check_us = next + refresh / 2 + 1;
                return false;
            }
        }
        true
    }
}

/// The picture on the screen.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Held {
    number: u32,
    race_seq: u64,
    ready_us: u64,
}

/// Frame Warp on the presenting thread for one stream.
pub struct Presenter {
    warp: Arc<FrameWarp>,
    held: Option<Held>,
    /// The raced frame whose slices are on the screen and the shift they were drawn with,
    /// computed at its first slice.
    raced: Option<(u64, Warp)>,
    /// The shift the next present of the held picture uses (a repeat's check, or the raced
    /// frame's slices), instead of a new one.
    pending: Option<Warp>,
    /// The shift of the last draw, for `presented`.
    drawn: Warp,
    hud: Option<Arc<Hud>>,
    mask_known: Option<u16>,
    repeat: Repeat,
    locked: bool,
}

impl Presenter {
    pub fn new(warp: Arc<FrameWarp>) -> Self {
        Self { warp, held: None, raced: None, pending: None, drawn: Warp::default(), hud: None, mask_known: None, repeat: Repeat::default(), locked: false }
    }

    /// The decoder's picture `number` goes on the screen; `race_seq` is its raced frame (0 for
    /// none) and `ready_us` when it was decoded. A raced frame keeps its slices' shift.
    pub fn picture(&mut self, number: u32, race_seq: u64, ready_us: u64) {
        self.held = Some(Held { number, race_seq, ready_us });
        self.pending = match self.raced {
            Some((seq, warp)) if race_seq != 0 && seq == race_seq => Some(warp),
            _ => None,
        };
    }

    /// The frame a slice of raced frame `seq` belongs to: as many frames after the held one as
    /// its raced frame is after the held one's.
    fn slice_frame(&self, seq: u64) -> u32 {
        match self.held {
            Some(held) if held.race_seq != 0 && seq > held.race_seq => held.number.wrapping_add((seq - held.race_seq).min(u64::from(u32::MAX)) as u32),
            Some(held) => held.number.wrapping_add(1),
            None => 0,
        }
    }

    /// Takes a new HUD mask, if one came.
    fn poll_mask(&mut self) {
        if let Some(mask) = self.warp.mask_since(self.mask_known) {
            self.mask_known = mask.as_ref().map(|mask| mask.generation);
            self.hud = mask.map(|mask| Arc::new(Hud::new(mask)));
        }
    }

    /// The draw of this present: of a slice of raced frame `slice`, or of the held picture.
    pub fn draw(&mut self, slice: Option<u64>) -> WarpDraw {
        self.draw_at(slice, Instant::now())
    }

    pub fn draw_at(&mut self, slice: Option<u64>, now: Instant) -> WarpDraw {
        self.poll_mask();
        let warp = match slice {
            Some(seq) => match self.raced {
                Some((raced, warp)) if raced == seq => warp,
                _ => {
                    let warp = self.warp.warp_at(self.slice_frame(seq), now);
                    self.raced = Some((seq, warp));
                    warp
                }
            },
            None => match (self.pending.take(), self.held) {
                (Some(warp), _) => warp,
                (None, Some(held)) => self.warp.warp_at(held.number, now),
                (None, None) => Warp::default(),
            },
        };
        self.drawn = warp;
        WarpDraw::of(&warp, self.hud.as_ref())
    }

    /// The present of the last draw went out at `at_us`.
    pub fn presented(&mut self, at_us: u64) {
        self.repeat.presented(at_us, self.drawn.pixels);
    }

    /// When the held picture may go out again, for the loop's wait: None while nothing is held,
    /// slices race, or nothing has locked or moved.
    pub fn repeat_due_us(&self, refresh_us: u64, racing: bool) -> Option<u64> {
        if racing || self.held.is_none() || (!self.locked && self.repeat.last_pixels() == [0.0, 0.0]) {
            return None;
        }
        self.repeat.due_us(refresh_us)
    }

    /// Whether the held picture goes out again now with a new shift (`Repeat`). `timing`'s next
    /// picture is one already waiting, if any; the held picture's arrival plus `frame_us` (one
    /// frame of the stream) is expected too.
    pub fn repeat_now(&mut self, now_us: u64, timing: Timing, frame_us: u64) -> bool {
        self.repeat_at(now_us, Instant::now(), timing, frame_us)
    }

    fn repeat_at(&mut self, now_us: u64, now: Instant, timing: Timing, frame_us: u64) -> bool {
        let Some(held) = self.held else { return false };
        if self.repeat_due_us(timing.refresh_us, false).is_none_or(|due| now_us < due) {
            return false;
        }
        let expected = held.ready_us + frame_us;
        let next_picture_us = Some(timing.next_picture_us.map_or(expected, |waiting| waiting.min(expected)));
        // Deciding counts nothing; only a repeat that goes out is counted as drawn.
        let peeked = self.warp.peek_at(held.number, now);
        let again = self.repeat.decide(now_us, peeked.pixels, Timing { next_picture_us, ..timing });
        if again {
            self.pending = Some(self.warp.warp_at(held.number, now));
        }
        again
    }

    /// A model from the host: the log line when it locked or unlocked.
    pub fn model(&mut self, model: &Model) -> Option<String> {
        let locked = model.locked();
        if std::mem::replace(&mut self.locked, locked) == locked {
            return None;
        }
        Some(if locked {
            format!("nightjar: Frame Warp: locked ({}), the picture follows the input", locked_text(model))
        } else {
            "nightjar: Frame Warp: unlocked, learning again".to_owned()
        })
    }

    pub fn status(&self) -> Status {
        self.warp.status()
    }
}

/// The input a locked model follows and its gain.
fn locked_text(model: &Model) -> String {
    let gain = |g: [f32; 2]| g[0].abs().max(g[1].abs());
    let mut parts = Vec::new();
    if model.mouse_locked {
        parts.push(format!("{:.2} px/count", gain(model.mouse_gain)));
    }
    if model.stick_locked {
        parts.push(format!("{:.0} px/s", gain(model.stick_gain)));
    }
    parts.join(", ")
}

/// The statistics overlay's Frame Warp line: what the host's model locked, its lag and the shift,
/// "learning" before it locks, or "not offered by the PC".
pub fn status_line(status: Option<&Status>) -> String {
    let Some(status) = status else { return "not offered by the PC".to_owned() };
    let Some(model) = status.model.filter(Model::locked) else {
        return match status.model {
            Some(model) if model.confidence > 0.0 => format!("learning ({:.0}% fit)", model.confidence * 100.0),
            _ => "learning".to_owned(),
        };
    };
    let shift = status.pixels[0].hypot(status.pixels[1]);
    let mut line = format!(
        "locked {}, lag {:.0} ms, shift {shift:.0} px (peak {:.0})",
        locked_text(&model),
        f64::from(model.lag_us) / 1000.0,
        status.peak
    );
    if status.mask_cells > 0 {
        line.push_str(&format!(", HUD {} cells", status.mask_cells));
    }
    line
}

/// The notice that this PC doesn't offer Frame Warp, once per PC like the PyroWave fallback's
/// (`pyrowave_notice`): the first stream from it with the setting on and Frame Warp refused says
/// it, then nothing until Frame Warp runs there again. Returns the text and whether `record` changed.
pub fn frame_warp_notice(record: &mut HostRecord, running: bool, refused: bool) -> (Option<String>, bool) {
    if refused {
        let first = record.claim_notice(NOTICE_FRAME_WARP_UNAVAILABLE);
        return (first.then(|| tr("This PC doesn’t offer Frame Warp, so Nightjar streams without it.").to_owned()), first);
    }
    (None, running && record.reset_notice(NOTICE_FRAME_WARP_UNAVAILABLE))
}

/// A HUD mask as a Direct3D 11 texture (R8, `mask_bytes`), made again when the mask changes.
#[cfg(windows)]
#[derive(Default)]
pub struct D3d11Mask {
    /// The HUD last asked for and its view, None when it could not be made.
    made: Option<(Arc<Hud>, Option<windows::Win32::Graphics::Direct3D11::ID3D11ShaderResourceView>)>,
    failed: bool,
}

#[cfg(windows)]
impl D3d11Mask {
    /// The view of `warp`'s HUD mask on `device`, None without one. A mask that cannot be made is
    /// logged once and drawn as none: the HUD then moves with the picture.
    ///
    /// # Safety
    /// `device` is live, used on this thread.
    pub unsafe fn view(&mut self, device: &windows::Win32::Graphics::Direct3D11::ID3D11Device, warp: &WarpDraw)
        -> Option<windows::Win32::Graphics::Direct3D11::ID3D11ShaderResourceView> {
        let hud = warp.mask()?;
        if let Some((made, view)) = &self.made {
            if Arc::ptr_eq(made, hud) {
                return view.clone();
            }
        }
        let view = match Self::make(device, &hud.mask) {
            Ok(view) => Some(view),
            Err(error) => {
                if !std::mem::replace(&mut self.failed, true) {
                    eprintln!("nightjar: Frame Warp: the HUD mask cannot be drawn ({error}); the HUD moves with the picture");
                }
                None
            }
        };
        self.made = Some((hud.clone(), view.clone()));
        view
    }

    unsafe fn make(device: &windows::Win32::Graphics::Direct3D11::ID3D11Device, mask: &Mask)
        -> Result<windows::Win32::Graphics::Direct3D11::ID3D11ShaderResourceView, String> {
        use windows::Win32::Graphics::Direct3D11::*;
        use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_R8_UNORM, DXGI_SAMPLE_DESC};
        let (row, bytes) = mask_bytes(mask);
        let data = D3D11_SUBRESOURCE_DATA { pSysMem: bytes.as_ptr().cast(), SysMemPitch: row as u32, SysMemSlicePitch: 0 };
        let mut texture = None;
        device.CreateTexture2D(&D3D11_TEXTURE2D_DESC { Width: row as u32, Height: u32::from(mask.height), MipLevels: 1, ArraySize: 1,
            Format: DXGI_FORMAT_R8_UNORM, SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 }, Usage: D3D11_USAGE_IMMUTABLE,
            BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32, ..Default::default() }, Some(std::ptr::from_ref(&data)), Some(&mut texture))
            .map_err(|e| format!("its texture: {e}"))?;
        let texture = texture.ok_or("no texture")?;
        let mut view = None;
        device.CreateShaderResourceView(&texture, None, Some(&mut view)).map_err(|e| format!("its view: {e}"))?;
        view.ok_or_else(|| "no view".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nightjar_core::frame_warp::{Tag, Tags};

    fn mask(width: u16, height: u16, set: impl Fn(u16, u16) -> bool) -> Mask {
        let mut mask = Mask::new(1, width, height);
        for y in 0..height {
            for x in 0..width {
                mask.set(x, y, set(x, y));
            }
        }
        mask
    }

    /// What every shader path samples for the output point `uv` (the module documentation).
    fn sample_at(draw: &WarpDraw, uv: [f32; 2]) -> [f32; 2] {
        if let Some(hud) = draw.mask() {
            let cell = |v: f32, n: u16| (v * f32::from(n)).floor().clamp(0.0, f32::from(n - 1)) as u16;
            if hud.mask.get(cell(uv[0], hud.mask.width), cell(uv[1], hud.mask.height)) {
                return uv;
            }
        }
        [(uv[0] - draw.shift[0]).clamp(0.0, 1.0), (uv[1] - draw.shift[1]).clamp(0.0, 1.0)]
    }

    /// Where the last quad that covers `at` samples it, as the GPU draws them in order.
    fn quad_sample(quads: &[Quad], at: [f32; 2]) -> Option<[f32; 2]> {
        quads.iter().rev().find(|q| at[0] >= q.at[0] && at[0] < q.at[2] && at[1] >= q.at[1] && at[1] < q.at[3]).map(|q| {
            let t = [(at[0] - q.at[0]) / (q.at[2] - q.at[0]), (at[1] - q.at[1]) / (q.at[3] - q.at[1])];
            [q.uv[0] + t[0] * (q.uv[2] - q.uv[0]), q.uv[1] + t[1] * (q.uv[3] - q.uv[1])]
        })
    }

    fn close(a: [f32; 2], b: [f32; 2]) -> bool {
        (a[0] - b[0]).abs() < 1e-4 && (a[1] - b[1]).abs() < 1e-4
    }

    #[test]
    fn the_shader_bytes_hold_each_cell_where_the_shaders_look() {
        for (width, height) in [(1, 1), (7, 3), (32, 2), (33, 5), (100, 9), (2048, 2)] {
            let mask = mask(width, height, |x, y| (x * 7 + y * 3) % 5 == 0 || x == width - 1);
            let (row, bytes) = mask_bytes(&mask);
            assert_eq!(row, Mask::stride(width) * 4);
            assert_eq!(bytes.len(), row * usize::from(height));
            for y in 0..height {
                for x in 0..width {
                    let byte = bytes[usize::from(y) * row + usize::from(x >> 3)];
                    assert_eq!(byte >> (x & 7) & 1 != 0, mask.get(x, y), "cell {x},{y} of {width}x{height}");
                }
            }
        }
    }

    #[test]
    fn row_runs_find_every_run_and_stop_at_the_width() {
        let mut runs = Vec::new();
        // Bits past the width are set in the word: they are not cells.
        row_runs(&[0b1110_0110 | 0xFFFF_0000], 12, &mut runs);
        assert_eq!(runs, [(1, 2), (5, 3)]);
        row_runs(&[u32::MAX, 1], 33, &mut runs);
        assert_eq!(runs, [(0, 33)], "a run across words");
        row_runs(&[1 << 31, 0b10], 64, &mut runs);
        assert_eq!(runs, [(31, 1), (33, 1)]);
        row_runs(&[0, 0], 64, &mut runs);
        assert!(runs.is_empty());
    }

    #[test]
    fn cell_rects_cover_exactly_the_set_cells_and_merge_down_the_rows() {
        type Shape = (u16, u16, fn(u16, u16) -> bool);
        let shapes: [Shape; 4] = [
            (8, 6, |x, y| (2..5).contains(&x) && (1..4).contains(&y)),
            (40, 10, |x, y| x < 3 || (x >= 30 && y >= 8)),
            (33, 7, |x, y| (x + y) % 2 == 0),
            (64, 64, |x, y| (x / 5 + y / 3) % 3 == 0),
        ];
        for (width, height, set) in shapes {
            let mask = mask(width, height, set);
            let rects = cell_rects(&mask);
            let mut drawn = Mask::new(1, width, height);
            for r in &rects {
                for y in r.y..r.y + r.h {
                    for x in r.x..r.x + r.w {
                        assert!(!drawn.get(x, y), "{r:?} overlaps another rectangle");
                        drawn.set(x, y, true);
                    }
                }
            }
            assert_eq!(drawn.words, mask.words, "{width}x{height}");
        }
        let block = mask(8, 6, |x, y| (2..5).contains(&x) && (1..4).contains(&y));
        assert_eq!(cell_rects(&block), [CellRect { x: 2, y: 1, w: 3, h: 3 }], "a block is one rectangle");
    }

    #[test]
    fn hud_areas_are_fractions_and_a_fine_mask_is_drawn_coarser_but_whole() {
        let corner = mask(10, 5, |x, y| x >= 8 && y == 0);
        assert_eq!(hud_areas(&corner, MAX_HUD_RECTS), [[0.8, 0.0, 1.0, 0.2]]);
        let checker = mask(64, 64, |x, y| (x + y) % 2 == 0);
        let areas = hud_areas(&checker, 100);
        assert!(areas.len() <= 100, "{}", areas.len());
        // Every set cell's centre stays inside some area.
        for y in 0..64u16 {
            for x in 0..64u16 {
                if checker.get(x, y) {
                    let c = [(f32::from(x) + 0.5) / 64.0, (f32::from(y) + 0.5) / 64.0];
                    assert!(areas.iter().any(|a| c[0] >= a[0] && c[0] <= a[2] && c[1] >= a[1] && c[1] <= a[3]), "cell {x},{y}");
                }
            }
        }
        assert!(hud_areas(&Mask::default(), MAX_HUD_RECTS).is_empty());
    }

    #[test]
    fn the_quads_sample_where_the_shaders_do() {
        let hud = Arc::new(Hud::new(Arc::new(mask(10, 10, |x, y| x >= 7 && y >= 8))));
        for shift in [[0.1, 0.0], [-0.05, 0.2], [0.3, -0.3], [-1.5, 0.0]] {
            for with_hud in [false, true] {
                let draw = WarpDraw { shift, hud: with_hud.then(|| hud.clone()) };
                let areas = draw.mask().map_or(&[][..], |hud| hud.areas());
                let quads = quads(draw.shift, areas);
                for j in 0..20 {
                    for i in 0..20 {
                        let at = [(i as f32 + 0.5) / 20.0, (j as f32 + 0.5) / 20.0];
                        let sampled = quad_sample(&quads, at).expect("the quads cover the destination");
                        assert!(close(sampled, sample_at(&draw, at)), "{shift:?} hud {with_hud} at {at:?}: {sampled:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn the_geometry_maps_the_source_rectangle_as_sdl_does() {
        let quads = quads([0.0, 0.0], &[]);
        assert_eq!(quads, [Quad { at: [0.0, 0.0, 1.0, 1.0], uv: [0.0, 0.0, 1.0, 1.0] }], "no shift is one quad");
        // A 1920x1080 picture in a 1920x1088 texture, drawn half size at (10, 20).
        let source = SDL_FRect { x: 0.0, y: 0.0, w: 1919.5, h: 1079.5 };
        let destination = SDL_FRect { x: 10.0, y: 20.0, w: 960.0, h: 540.0 };
        let (vertices, indices) = geometry(&quads, source, (1920.0, 1088.0), destination);
        assert_eq!(indices, [0, 1, 2, 0, 2, 3]);
        let corners: Vec<_> = vertices.iter().map(|v| (v.position.x, v.position.y, v.tex_coord.x, v.tex_coord.y)).collect();
        assert_eq!(corners, [
            (10.0, 20.0, 0.0, 0.0),
            (970.0, 20.0, 1919.5 / 1920.0, 0.0),
            (970.0, 560.0, 1919.5 / 1920.0, 1079.5 / 1088.0),
            (10.0, 560.0, 0.0, 1079.5 / 1088.0),
        ]);
    }

    #[test]
    fn the_warp_row_is_zero_without_a_shift_and_names_the_mask_only_when_bound() {
        let hud = Arc::new(Hud::new(Arc::new(mask(64, 36, |x, _| x == 0))));
        assert_eq!(WarpDraw { shift: [0.0, 0.0], hud: Some(hud.clone()) }.row(true), [0.0; 4]);
        assert!(WarpDraw { shift: [0.0, 0.0], hud: Some(hud.clone()) }.mask().is_none(), "no shift binds no mask");
        let draw = WarpDraw { shift: [0.01, -0.02], hud: Some(hud) };
        assert_eq!(draw.row(true), [0.01, -0.02, 64.0, 36.0]);
        assert_eq!(draw.row(false), [0.01, -0.02, 0.0, 0.0], "a mask that could not be bound is none");
        let inactive = Warp { pixels: [0.001, 0.0], uv: [0.000_001, 0.0], active: false, mask_generation: None };
        assert!(WarpDraw::of(&inactive, None).is_identity());
        let without_mask = Warp { pixels: [5.0, 0.0], uv: [0.005, 0.0], active: true, mask_generation: None };
        let held = Arc::new(Hud::new(Arc::new(Mask::new(1, 1, 1))));
        assert!(WarpDraw::of(&without_mask, Some(&held)).hud.is_none(), "the predictor knows no mask");
    }

    const TIMING: Timing = Timing { refresh_us: 10_000, vsync: false, next_picture_us: None };

    #[test]
    fn a_repeat_needs_a_refresh_since_the_last_present_and_a_quarter_pixel() {
        let mut repeat = Repeat::default();
        assert!(!repeat.decide(0, [5.0, 0.0], TIMING), "nothing presented yet");
        repeat.presented(1_000, [1.0, 1.0]);
        assert_eq!(repeat.due_us(10_000), Some(11_000));
        assert!(!repeat.decide(10_999, [9.0, 9.0], TIMING), "within a refresh of the last present");
        assert!(!repeat.decide(11_000, [1.1, 1.1], TIMING), "0.14 px is too little");
        assert_eq!(repeat.due_us(10_000), Some(21_000), "the next look is a refresh later");
        assert!(!repeat.decide(15_000, [3.0, 1.0], TIMING));
        assert!(repeat.decide(21_000, [1.0, 1.25], TIMING), "a quarter pixel goes out");
    }

    #[test]
    fn a_repeat_never_takes_the_next_pictures_refresh() {
        let mut repeat = Repeat::default();
        repeat.presented(0, [0.0, 0.0]);
        let vsync = |next: u64| Timing { refresh_us: 10_000, vsync: true, next_picture_us: Some(next) };
        assert!(!repeat.decide(10_000, [2.0, 0.0], vsync(19_000)), "the picture comes within a refresh");
        assert_eq!(repeat.due_us(10_000), Some(24_001), "looks again when it is half a refresh late");
        assert!(repeat.decide(24_001, [2.0, 0.0], vsync(19_000)), "a late picture leaves the refresh to the repeat");
        let mut repeat = Repeat::default();
        repeat.presented(0, [0.0, 0.0]);
        assert!(repeat.decide(10_000, [2.0, 0.0], vsync(20_001)), "the picture comes after the next refresh");
        // Without V-Sync half a refresh is enough room.
        let mut repeat = Repeat::default();
        repeat.presented(0, [0.0, 0.0]);
        let immediate = Timing { vsync: false, next_picture_us: Some(16_000), ..TIMING };
        assert!(repeat.decide(10_000, [2.0, 0.0], immediate));
        let mut repeat = Repeat::default();
        repeat.presented(0, [0.0, 0.0]);
        assert!(!repeat.decide(10_000, [2.0, 0.0], Timing { next_picture_us: Some(14_000), ..immediate }));
    }

    fn locked_mouse() -> Model {
        Model {
            generation: 1,
            mouse_locked: true,
            picture_width: 1000,
            picture_height: 500,
            mouse_gain: [-0.5, -0.25],
            max_offset: [200.0, 100.0],
            lag_us: 38_000,
            confidence: 0.94,
            ..Model::default()
        }
    }

    /// A Frame Warp whose frame 5 lacks 40 mouse counts in x: -20 px once its fade is in.
    fn moving() -> Arc<FrameWarp> {
        let warp = FrameWarp::new(Some(0), MASK_EDGE);
        warp.on_model(locked_mouse());
        warp.note_mouse(100, 40);
        warp.on_tags(&Tags { model_generation: 1, tags: vec![Tag { frame: 5, mouse_x: 60, mouse_y: 40, ..Tag::default() }] });
        warp
    }

    #[test]
    fn a_raced_frame_keeps_the_shift_of_its_first_slice_to_its_own_present() {
        let mut presenter = Presenter::new(moving());
        presenter.picture(4, 10, 0);
        let start = Instant::now();
        // The fade starts at the first slice: no shift yet.
        assert!(presenter.draw_at(Some(11), start).is_identity());
        let later = start + std::time::Duration::from_secs(1);
        assert!(presenter.draw_at(Some(11), later).is_identity(), "the frame's later slices draw as its first did");
        presenter.picture(5, 11, 0);
        assert!(presenter.draw_at(None, later).is_identity(), "its own present too");
        let redraw = presenter.draw_at(None, later);
        assert!((redraw.shift[0] + 0.02).abs() < 1e-6 && redraw.shift[1] == 0.0, "a redraw takes the shift now: {redraw:?}");
        let next = presenter.draw_at(Some(12), later);
        assert!(!next.is_identity(), "the next raced frame takes its own");
    }

    #[test]
    fn a_repeat_draws_the_shift_it_checked_and_records_it() {
        let warp = moving();
        let mut presenter = Presenter::new(warp.clone());
        presenter.model(&locked_mouse());
        presenter.picture(5, 0, 1_000);
        let start = Instant::now();
        presenter.draw_at(None, start);
        presenter.presented(1_000);
        assert_eq!(presenter.repeat_due_us(10_000, false), Some(11_000), "locked: it looks a refresh later");
        assert_eq!(presenter.repeat_due_us(10_000, true), None, "never while slices race");
        // A second on, the fade is in.
        let later = start + std::time::Duration::from_secs(1);
        assert!(presenter.repeat_at(11_000, later, TIMING, 100_000), "the shift moved 20 px");
        let drawn = presenter.draw_at(None, later + std::time::Duration::from_secs(1));
        assert!((drawn.shift[0] + 0.02).abs() < 1e-6, "the checked shift: {drawn:?}");
        presenter.presented(11_500);
        assert!(!presenter.repeat_at(21_500, later, TIMING, 100_000), "nothing moved since");
        warp.note_mouse(10, 0);
        assert!(!presenter.repeat_at(31_500, later, Timing { next_picture_us: Some(33_000), ..TIMING }, 100_000),
            "a picture waiting within half a refresh takes it");
        assert!(presenter.repeat_at(38_501, later, TIMING, 100_000), "the 5 px it moved go out once no picture is that close");
    }

    #[test]
    fn the_model_logs_lock_and_unlock_once_each() {
        let mut presenter = Presenter::new(FrameWarp::new(Some(0), MASK_EDGE));
        assert_eq!(presenter.model(&Model::default()), None, "learning from the start");
        assert_eq!(presenter.model(&locked_mouse()).as_deref(), Some("nightjar: Frame Warp: locked (0.50 px/count), the picture follows the input"));
        assert_eq!(presenter.model(&locked_mouse()), None);
        assert_eq!(presenter.model(&Model::default()).as_deref(), Some("nightjar: Frame Warp: unlocked, learning again"));
    }

    #[test]
    fn the_overlay_line_says_what_locked_and_how_far_it_shifts() {
        assert_eq!(status_line(None), "not offered by the PC");
        assert_eq!(status_line(Some(&Status::default())), "learning");
        let fitting = Status { model: Some(Model { confidence: 0.42, ..Model::default() }), ..Status::default() };
        assert_eq!(status_line(Some(&fitting)), "learning (42% fit)");
        let mouse = Status { model: Some(Model { mouse_gain: [0.94, -0.5], ..locked_mouse() }), pixels: [12.0, 0.0], peak: 41.0, ..Status::default() };
        assert_eq!(status_line(Some(&mouse)), "locked 0.94 px/count, lag 38 ms, shift 12 px (peak 41)");
        let both = Status {
            model: Some(Model { stick_locked: true, stick_gain: [-900.0, 400.0], ..locked_mouse() }),
            pixels: [3.0, 4.0],
            peak: 5.0,
            mask_cells: 120,
            ..Status::default()
        };
        assert_eq!(status_line(Some(&both)), "locked 0.50 px/count, 900 px/s, lag 38 ms, shift 5 px (peak 5), HUD 120 cells");
    }

    #[test]
    fn the_notice_is_said_once_per_pc_until_frame_warp_runs_there() {
        let mut pc = HostRecord::default();
        let text = "This PC doesn’t offer Frame Warp, so Nightjar streams without it.";
        assert_eq!(frame_warp_notice(&mut pc, false, true), (Some(text.to_owned()), true), "the first time");
        assert_eq!(frame_warp_notice(&mut pc, false, true), (None, false), "then not again");
        assert_eq!(frame_warp_notice(&mut pc, false, false), (None, false), "the setting off changes nothing");
        assert_eq!(frame_warp_notice(&mut pc, true, false), (None, true), "it runs there now");
        assert_eq!(frame_warp_notice(&mut pc, true, false), (None, false));
        assert_eq!(frame_warp_notice(&mut pc, false, true), (Some(text.to_owned()), true), "and is said again once it is gone");
    }
}
