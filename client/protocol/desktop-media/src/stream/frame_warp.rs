//! Frame Warp on the desktop's presenter (docs/frame-warp.md). `nightjar_core::frame_warp`
//! predicts how far the picture's content has to move for the mouse and right-stick input the
//! host had not seen when it rendered the picture; this module applies that shift on every
//! present path, keeps the HUD mask's cells in place, and presents the held picture again when
//! the shift moves between pictures.
//!
//! What every path does, in fractions of the visible picture: the output point `uv` samples the
//! picture at `uv - shift`, or, when the host knows the game's lens, where a camera turned by
//! that shift at the picture's centre shows it (`nightjar_core::frame_warp::source`: the edges
//! move farther than the centre), with the edge fill where that is outside the picture (mirrored
//! at the edge and darkened, `nightjar_core::frame_warp::edge`), unless the HUD mask cell
//! `floor(uv * (width, height))` is set, or the cell that sample lands on is, which samples at
//! `uv` as it is: the HUD stays in place and isn't drawn a second time beside itself, a shift
//! away (the scene behind it isn't in the picture, so it stays unshifted there). The shader
//! paths share `frame_warp.hlsl` (`hlsl`) and read the mask as bytes (byte `x / 8` of row `y`,
//! bit `x % 8`: `Mask::words` in little-endian order) through two rows of their constants, `warp`
//! `(shift x, shift y, mask width, mask height)` and `warpLens` (the lens terms); the SDL renderer
//! path draws the same mapping as triangles (`Mesh`, `render_texture`). An identity shift takes
//! each path's draw as it was.

#[cfg(any(windows, test))]
use std::borrow::Cow;
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use nightjar_core::frame_warp::{FrameWarp, Mask, Model, Status, Warp, FLAT};
#[cfg(test)]
use nightjar_core::frame_warp::{edge, source, EDGE_RAMP, MIN_DEPTH};
#[cfg(test)]
use sdl3_sys::everything::SDL_FRect;

use crate::config::{HostRecord, NOTICE_FRAME_WARP_UNAVAILABLE};
use crate::strings::tr;

/// The HUD masks the presenter draws, the most the protocol sends (`MAX_MASK_EDGE` cells a side).
pub const MASK_EDGE: u16 = rubylight_protocol::frame_warp::MAX_MASK_EDGE;
/// The least move of the shift, in picture pixels, that presents the held picture again.
pub const REPEAT_STEP: f32 = 0.25;
/// The most HUD rectangles the SDL renderer path draws; a finer mask is drawn coarser.
pub const MAX_HUD_RECTS: usize = 1024;
/// Frame Warp's HLSL, which the HLSL shaders include as `frame_warp.hlsl` (`hlsl`).
#[cfg(any(windows, test))]
pub const HLSL: &str = include_str!("../frame_warp.hlsl");
#[cfg(any(windows, test))]
const HLSL_INCLUDE: &[u8] = b"#include \"frame_warp.hlsl\"";

/// `source` with each line that includes `frame_warp.hlsl` replaced by the file, as glslang finds
/// it beside the shader: Direct3D 11 compiles from memory, with no file to include from.
#[cfg(any(windows, test))]
pub fn hlsl(source: &[u8]) -> Cow<'_, [u8]> {
    let find = |text: &[u8]| text.windows(HLSL_INCLUDE.len()).position(|window| window == HLSL_INCLUDE);
    let Some(mut at) = find(source) else { return Cow::Borrowed(source) };
    let mut out = Vec::with_capacity(source.len() + HLSL.len());
    let mut rest = source;
    loop {
        out.extend_from_slice(&rest[..at]);
        out.extend_from_slice(HLSL.as_bytes());
        rest = &rest[at + HLSL_INCLUDE.len()..];
        match find(rest) {
            Some(next) => at = next,
            None => break,
        }
    }
    out.extend_from_slice(rest);
    Cow::Owned(out)
}

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
#[derive(Clone, Debug)]
pub struct WarpDraw {
    /// How far the picture's centre moves, in fractions of the visible picture: a flat picture
    /// samples at `uv - shift`.
    pub shift: [f32; 2],
    /// How the rest of the picture follows (`Warp::lens`): the point `uv` samples at
    /// `source(uv, shift, lens)`. `FLAT` for a picture that moves alike everywhere.
    pub lens: [f32; 4],
    /// The cells that stay in place; None for none.
    pub hud: Option<Arc<Hud>>,
}

impl Default for WarpDraw {
    fn default() -> Self {
        Self { shift: [0.0; 2], lens: FLAT, hud: None }
    }
}

impl WarpDraw {
    /// The draw for `warp`, with `hud` while the shift is on and the predictor knows a mask.
    pub fn of(warp: &Warp, hud: Option<&Arc<Hud>>) -> Self {
        if !warp.active {
            return Self::default();
        }
        Self { shift: warp.uv, lens: warp.lens, hud: hud.filter(|_| warp.mask_generation.is_some()).cloned() }
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

    /// The shaders' `warpLens` row, which they read only beside a `warp` row that shifts.
    pub fn lens_row(&self) -> [f32; 4] {
        if self.is_identity() { FLAT } else { self.lens }
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


mod mesh;
pub use mesh::{render_texture, unshifted, Corner, Mesh};
#[cfg(test)]
use mesh::{lens_bend, lens_cells, LENS_ERROR, MAX_LENS_CELLS};

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
    raced: Option<(u64, Shift)>,
    /// The shift the next present of the held picture uses (a repeat's check, or the raced
    /// frame's slices), instead of a new one.
    pending: Option<Shift>,
    /// The shift of the last draw, for `presented`.
    drawn: Shift,
    /// The input count (`FrameWarp::input_seq`) the last present's shift saw.
    presented_seq: u32,
    hud: Option<Arc<Hud>>,
    mask_known: Option<u16>,
    repeat: Repeat,
    locked: bool,
}

/// A shift and the input count (`FrameWarp::input_seq`) read before it was computed.
#[derive(Clone, Copy, Debug, Default)]
struct Shift {
    warp: Warp,
    input_seq: u32,
}

impl Presenter {
    pub fn new(warp: Arc<FrameWarp>) -> Self {
        Self { warp, held: None, raced: None, pending: None, drawn: Shift::default(), presented_seq: 0, hud: None, mask_known: None, repeat: Repeat::default(),
            locked: false }
    }

    /// The shift of picture `frame` drawn at `now`, counted as drawn.
    fn shift(&self, frame: u32, now: Instant) -> Shift {
        let input_seq = self.warp.input_seq();
        Shift { warp: self.warp.warp_at(frame, now), input_seq }
    }

    /// The decoder's picture `number` goes on the screen; `race_seq` is its raced frame (0 for
    /// none) and `ready_us` when it was decoded. A raced frame keeps its slices' shift.
    pub fn picture(&mut self, number: u32, race_seq: u64, ready_us: u64) {
        self.held = Some(Held { number, race_seq, ready_us });
        self.pending = match self.raced {
            Some((seq, shift)) if race_seq != 0 && seq == race_seq => Some(shift),
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
        let shift = match slice {
            Some(seq) => match self.raced {
                Some((raced, shift)) if raced == seq => shift,
                _ => {
                    let shift = self.shift(self.slice_frame(seq), now);
                    self.raced = Some((seq, shift));
                    shift
                }
            },
            None => match (self.pending.take(), self.held) {
                (Some(shift), _) => shift,
                (None, Some(held)) => self.shift(held.number, now),
                (None, None) => Shift { warp: Warp::default(), input_seq: self.warp.input_seq() },
            },
        };
        self.drawn = shift;
        WarpDraw::of(&shift.warp, self.hud.as_ref())
    }

    /// The present of the last draw went out at `at_us`.
    pub fn presented(&mut self, at_us: u64) {
        self.repeat.presented(at_us, self.drawn.warp.pixels);
        self.presented_seq = self.drawn.input_seq;
    }

    /// When the held picture may go out again, for the loop's wait: None while nothing is held
    /// or slices race, and unless the last present was shifted, or the model is locked and input
    /// that can move the shift was sent since that present's shift (`FrameWarp::input_seq`) or
    /// the stick is held out of its dead zone. Otherwise the shift stays where it was drawn, so
    /// an idle stream does not wake every refresh to find that out.
    pub fn repeat_due_us(&self, refresh_us: u64, racing: bool) -> Option<u64> {
        if racing || self.held.is_none() {
            return None;
        }
        let moved = self.repeat.last_pixels() != [0.0, 0.0];
        let input = self.locked && (self.warp.input_seq() != self.presented_seq || self.warp.stick_deflected());
        if !moved && !input {
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
            self.pending = Some(self.shift(held.number, now));
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
    if let Some(degrees) = nightjar_core::frame_warp::field_of_view(model) {
        parts.push(format!("{degrees:.0} degree lens"));
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
        if let Some((made, view)) = &self.made
            && Arc::ptr_eq(made, hud)
        {
            return view.clone();
        }
        // SAFETY: caller guarantees `device` is live on this thread.
        let view = match unsafe { Self::make(device, &hud.mask) } {
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
        // SAFETY: caller guarantees `device` is live; `bytes` outlives `CreateTexture2D`.
        unsafe {
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
}

#[cfg(test)]
mod tests;
