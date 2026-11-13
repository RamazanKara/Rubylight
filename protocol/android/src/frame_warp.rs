//! Frame Warp for Android's renderers (docs/frame-warp.md, `nightjar_core::frame_warp`).
//!
//! The stream's [`FrameWarp`] is kept here from the engine's start to its stop, and reached two
//! ways: by Java (the decoder, the OpenGL ES path and the overlay) through `RustBridge`'s
//! `frameWarp*` natives in `lib.rs`, and by the native Vulkan and PyroWave renderers through
//! [`API`], a table of C functions whose address Java hands them (`RustBridge.frameWarpApi`).
//! The table rather than a symbol: Android's linker namespaces keep one app library from
//! resolving another's symbols portably, while the address of a static in a loaded library is
//! good for the whole process. `app/src/main/jni/frame_warp_api.h` is its C side.
//!
//! Frame numbers: a decoded picture reaches a renderer with the presentation time the decoder
//! was given, not its frame number. The decoder notes each pair as it queues the frame
//! ([`note_frame`]), and [`FrameRing`] keeps the last [`FRAME_RING`] so a renderer finds its
//! picture's number by that time. Only an exact match counts: a time it does not know (a frame
//! queued before the stream's Frame Warp began, or too long ago) draws without a shift rather
//! than with another frame's.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use nightjar_core::control::ClientMessage;
use nightjar_core::frame_warp::{FrameWarp, Mask, Model, Status, Warp, FLAT, FRAME_WARP_REQUEST_MESSAGE_TYPE};

use crate::engine;

/// The controller whose right stick Frame Warp follows: the first one, as games' camera is.
pub const CONTROLLER: u8 = 0;
/// The widest HUD mask asked for, in cells; the renderers upload it as one byte per cell.
pub const MASK_MAX_WIDTH: u16 = 1024;
/// Presentation times remembered with their frame numbers: about a second at 120 fps, far more
/// than a decoder and renderer ever hold.
pub const FRAME_RING: usize = 128;

/// [`FrameWarpApi::version`]; `frame_warp_api.h` checks it.
pub const API_VERSION: u32 = 3;
/// The floats `warp` and `peek` write: the centre's shift as fractions of the picture (x, y) and
/// in picture pixels (x, y), then the lens terms `[k.x, k.y, n.x, n.y]`
/// (`nightjar_core::frame_warp::source`). `frame_warp_api.h` and `MoonBridge` say the same.
pub const WARP_FLOATS: usize = 8;
/// What they write for a picture drawn as it is: no shift, through the lens of a flat picture.
pub const WARP_STILL: [f32; WARP_FLOATS] = [0.0, 0.0, 0.0, 0.0, FLAT[0], FLAT[1], FLAT[2], FLAT[3]];
/// `warp` codes besides a mask generation (0 to 65535).
pub const WARP_NO_MASK: i32 = -1;
pub const WARP_INACTIVE: i32 = -2;
/// `mask` codes besides a mask generation.
pub const MASK_UNCHANGED: i32 = -1;
pub const MASK_CLEARED: i32 = -2;
pub const MASK_TOO_SMALL: i32 = -3;
/// A mask cell's byte where the HUD stays put, and elsewhere.
pub const CELL_SET: u8 = 255;
pub const CELL_CLEAR: u8 = 0;

/// `RustBridge.FRAME_WARP_*`: what the overlay line says ([`status_numbers`]).
pub const STATUS_OFF: i32 = 0;
pub const STATUS_NOT_OFFERED: i32 = 1;
pub const STATUS_LEARNING: i32 = 2;
pub const STATUS_MOUSE: i32 = 3;
pub const STATUS_STICK: i32 = 4;
/// Asked for, but this stream's renderer cannot draw it ([`decline`]).
pub const STATUS_DECLINED: i32 = 5;
/// Numbers [`status_numbers`] writes: gain, lag ms, shift px, peak px.
pub const STATUS_NUMBERS: usize = 4;

/// Presentation times (µs) with the frame numbers queued with them, newest last.
#[derive(Debug)]
pub struct FrameRing {
    pts: [i64; FRAME_RING],
    frames: [u32; FRAME_RING],
    next: usize,
    len: usize,
}

impl FrameRing {
    pub const fn new() -> Self {
        Self {
            pts: [0; FRAME_RING],
            frames: [0; FRAME_RING],
            next: 0,
            len: 0,
        }
    }

    pub fn note(&mut self, pts_us: i64, frame: u32) {
        self.pts[self.next] = pts_us;
        self.frames[self.next] = frame;
        self.next = (self.next + 1) % FRAME_RING;
        self.len = (self.len + 1).min(FRAME_RING);
    }

    /// The frame queued with `pts_us`, the newest if several were (a decoder restart can start
    /// its times over).
    pub fn find(&self, pts_us: i64) -> Option<u32> {
        (1..=self.len)
            .map(|back| (self.next + FRAME_RING - back) % FRAME_RING)
            .find(|&i| self.pts[i] == pts_us)
            .map(|i| self.frames[i])
    }

    pub fn clear(&mut self) {
        self.len = 0;
        self.next = 0;
    }
}

impl Default for FrameRing {
    fn default() -> Self {
        Self::new()
    }
}

static CURRENT: Mutex<Option<Arc<FrameWarp>>> = Mutex::new(None);
static FRAMES: Mutex<FrameRing> = Mutex::new(FrameRing::new());
static DECLINED: AtomicBool = AtomicBool::new(false);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|p| p.into_inner())
}

/// The new stream's Frame Warp, or none; from before its video starts.
pub fn begin(warp: Option<Arc<FrameWarp>>) {
    lock(&FRAMES).clear();
    DECLINED.store(false, Ordering::SeqCst);
    *lock(&CURRENT) = warp;
}

/// The stream stopped.
pub fn end() {
    begin(None);
}

pub fn current() -> Option<Arc<FrameWarp>> {
    lock(&CURRENT).clone()
}

/// Whether this stream draws with Frame Warp: it asked for it, and the host announced the
/// request message. Known from the end of the RTSP handshake, so while the renderers start.
pub fn offered() -> bool {
    current().is_some() && engine::host_supports(FRAME_WARP_REQUEST_MESSAGE_TYPE)
}

/// The renderer cannot draw Frame Warp for this stream (HDR without Vulkan, full chroma, a
/// renderer that did not start): the host is told to stop its work for it. A renderer decides as
/// the video starts, before the session runs, so then [`after_start`] tells it.
pub fn decline() {
    if DECLINED.swap(true, Ordering::SeqCst) {
        return;
    }
    if let Some(warp) = current() {
        if engine::session_id().is_some() {
            engine::send(ClientMessage::FrameWarpRequest(warp.request(false)));
        }
    }
}

pub fn declined() -> bool {
    DECLINED.load(Ordering::SeqCst)
}

/// The session runs: a renderer that declined while it started is taken at its word now. A
/// repeated stop is harmless.
pub fn after_start() {
    if let (true, Some(warp)) = (declined(), current()) {
        engine::send(ClientMessage::FrameWarpRequest(warp.request(false)));
    }
}

/// The decoder queued `frame` with presentation time `pts_us`.
pub fn note_frame(pts_us: i64, frame: u32) {
    lock(&FRAMES).note(pts_us, frame);
}

pub fn frame_for_pts(pts_us: i64) -> Option<u32> {
    lock(&FRAMES).find(pts_us)
}

/// The shift for `frame` drawn now, as `warp` of [`API`] gives it: the code, and the centre's
/// shift as fractions of the picture (x, y) and in picture pixels (x, y), then the lens terms the
/// rest of the picture is turned through ([`WARP_FLOATS`]). Inactive without a stream's Frame
/// Warp or a frame number.
pub fn warp_for(frame: Option<u32>) -> (i32, [f32; WARP_FLOATS]) {
    match (current(), frame) {
        (Some(warp), Some(frame)) => encode_warp(&warp.warp(frame)),
        _ => (WARP_INACTIVE, WARP_STILL),
    }
}

/// As [`warp_for`], without counting it in the statistics: for a renderer deciding whether to
/// draw the picture on screen again. A redraw that follows calls [`warp_for`] for what it draws.
pub fn peek_for(frame: Option<u32>) -> (i32, [f32; WARP_FLOATS]) {
    match (current(), frame) {
        (Some(warp), Some(frame)) => encode_warp(&warp.peek(frame)),
        _ => (WARP_INACTIVE, WARP_STILL),
    }
}

/// The count of mouse and controller inputs noted for the running stream (wrapping), 0 without
/// one: a renderer that kept it from its last draw knows, without asking for a shift, whether
/// input was sent since.
pub fn input_seq() -> u32 {
    lock(&CURRENT).as_ref().map_or(0, |warp| warp.input_seq())
}

/// Whether the tracked right stick is deflected, whose turn grows without new input; false
/// without a stream.
pub fn stick_deflected() -> bool {
    lock(&CURRENT).as_ref().is_some_and(|warp| warp.stick_deflected())
}

/// [`input_state`]'s bit set while the stick is deflected; the input count is the low 32 bits.
pub const INPUT_STICK_DEFLECTED: i64 = 1 << 32;

/// [`input_seq`] and [`stick_deflected`] in one value, for Java's redraws between pictures to
/// read with one call: the count in the low 32 bits, [`INPUT_STICK_DEFLECTED`] while deflected.
pub fn input_state() -> i64 {
    pack_input_state(input_seq(), stick_deflected())
}

pub fn pack_input_state(seq: u32, stick_deflected: bool) -> i64 {
    i64::from(seq) | (i64::from(stick_deflected) * INPUT_STICK_DEFLECTED)
}

pub fn encode_warp(warp: &Warp) -> (i32, [f32; WARP_FLOATS]) {
    if !warp.active {
        return (WARP_INACTIVE, WARP_STILL);
    }
    let code = warp.mask_generation.map_or(WARP_NO_MASK, i32::from);
    let [kx, ky, nx, ny] = warp.lens;
    (code, [warp.uv[0], warp.uv[1], warp.pixels[0], warp.pixels[1], kx, ky, nx, ny])
}

/// Writes `mask` as one byte per cell, [`CELL_SET`] or [`CELL_CLEAR`], rows from the top, into
/// `out`; false if `out` is shorter than width times height.
pub fn pack_r8(mask: &Mask, out: &mut [u8]) -> bool {
    let width = usize::from(mask.width);
    let height = usize::from(mask.height);
    let Some(cells) = width.checked_mul(height) else {
        return false;
    };
    if out.len() < cells {
        return false;
    }
    let stride = Mask::stride(mask.width);
    for (y, row) in out[..cells].chunks_exact_mut(width.max(1)).enumerate().take(height) {
        let words = mask.words.get(y * stride..(y + 1) * stride).unwrap_or(&[]);
        for (x, cell) in row.iter_mut().enumerate() {
            let set = words.get(x / 32).is_some_and(|&w| (w >> (x % 32)) & 1 != 0);
            *cell = if set { CELL_SET } else { CELL_CLEAR };
        }
    }
    true
}

/// What a renderer that holds mask generation `known` (-1 for none) should upload.
#[derive(Debug, PartialEq)]
pub enum MaskUpdate {
    Unchanged,
    Cleared,
    New(Arc<Mask>),
}

pub fn mask_update(warp: Option<&FrameWarp>, known: i32) -> MaskUpdate {
    let known = u16::try_from(known).ok();
    match warp.map(|w| w.mask_since(known)) {
        None if known.is_none() => MaskUpdate::Unchanged,
        None => MaskUpdate::Cleared,
        Some(None) => MaskUpdate::Unchanged,
        Some(Some(None)) => MaskUpdate::Cleared,
        Some(Some(Some(mask))) => MaskUpdate::New(mask),
    }
}

/// The overlay's line for the running stream ([`status_numbers`]).
pub fn status(numbers: &mut [f32; STATUS_NUMBERS]) -> i32 {
    let warp = current();
    let status = warp.as_ref().map(|w| w.status());
    status_numbers(warp.is_some(), offered(), declined(), status.as_ref(), numbers)
}

/// The overlay's state and numbers: `numbers` gets the gain (picture px per mouse count, or
/// per second of full stick), the game's lag in ms, the latest shift and the last second's
/// peak, picture px.
pub fn status_numbers(
    requested: bool,
    offered: bool,
    declined: bool,
    status: Option<&Status>,
    numbers: &mut [f32; STATUS_NUMBERS],
) -> i32 {
    *numbers = [0.0; STATUS_NUMBERS];
    if !requested {
        return STATUS_OFF;
    }
    if !offered {
        return STATUS_NOT_OFFERED;
    }
    if declined {
        return STATUS_DECLINED;
    }
    let Some(status) = status else {
        return STATUS_LEARNING;
    };
    let shift = status.pixels[0].hypot(status.pixels[1]);
    let (state, gain) = match status.model {
        Some(model) if model.mouse_locked => (STATUS_MOUSE, axis_gain(model.mouse_gain)),
        Some(model) if model.stick_locked => (STATUS_STICK, axis_gain(model.stick_gain)),
        _ => return STATUS_LEARNING,
    };
    let lag_ms = status.model.map_or(0.0, |m| m.lag_us as f32 / 1000.0);
    *numbers = [gain, lag_ms, shift, status.peak.max(shift)];
    state
}

/// One number for a gain per axis: the larger, as a turn mostly moves along one.
fn axis_gain(gain: [f32; 2]) -> f32 {
    gain[0].abs().max(gain[1].abs())
}

/// The log line for a model from the host.
pub fn model_line(model: &Model) -> String {
    let state = match (model.mouse_locked, model.stick_locked) {
        (true, true) => "mouse and stick locked",
        (true, false) => "mouse locked",
        (false, true) => "stick locked",
        (false, false) => "unlocked",
    };
    let lens = match nightjar_core::frame_warp::field_of_view(model) {
        Some(degrees) => format!("a {degrees:.0} degree lens"),
        None => "no lens".to_owned(),
    };
    format!(
        "Frame Warp: model {} {state} for {}x{}: mouse {:.3}/{:.3} px/count, stick {:.0}/{:.0} px/s, lag {:.1} ms, fit {:.2}, {lens}",
        model.generation,
        model.picture_width,
        model.picture_height,
        model.mouse_gain[0],
        model.mouse_gain[1],
        model.stick_gain[0],
        model.stick_gain[1],
        model.lag_us as f32 / 1000.0,
        model.confidence
    )
}

/// The C side of Frame Warp for the native renderers; `app/src/main/jni/frame_warp_api.h`
/// declares the same layout. Every function is safe to call from any thread, with or without
/// a stream.
#[repr(C)]
pub struct FrameWarpApi {
    pub version: u32,
    /// The frame queued with presentation time `pts_us` (µs), or -1.
    pub frame_for_pts: extern "C" fn(pts_us: i64) -> i64,
    /// The shift for `frame` (-1 for unknown) drawn now: writes uv x, uv y, px x, px y and the
    /// lens terms k.x, k.y, n.x, n.y to `out` ([`WARP_FLOATS`] floats) and returns the HUD mask
    /// generation to draw with, [`WARP_NO_MASK`], or [`WARP_INACTIVE`] to draw the picture as it
    /// is ([`WARP_STILL`] written).
    pub warp: unsafe extern "C" fn(frame: i64, out: *mut f32) -> i32,
    /// The HUD mask when its generation is not `known` (-1 for none): writes width and height
    /// and, given room, the cells (one byte each, rows from the top) and returns the generation;
    /// or [`MASK_UNCHANGED`], [`MASK_CLEARED`], or [`MASK_TOO_SMALL`] with the size written.
    pub mask: unsafe extern "C" fn(known: i32, width: *mut u32, height: *mut u32, cells: *mut u8, capacity: usize) -> i32,
    /// As `warp`, without counting it in the statistics: whether the picture on screen should be
    /// drawn again. What a redraw then draws goes through `warp`.
    pub peek: unsafe extern "C" fn(frame: i64, out: *mut f32) -> i32,
    /// [`input_seq`]: read before a draw's `warp`, so a redraw between pictures can tell, without
    /// a `peek`, that no input was sent since.
    pub input_seq: extern "C" fn() -> u32,
    /// [`stick_deflected`]: 1 while the tracked right stick is deflected, else 0.
    pub stick_deflected: extern "C" fn() -> i32,
}

pub static API: FrameWarpApi = FrameWarpApi {
    version: API_VERSION,
    frame_for_pts: api_frame_for_pts,
    warp: api_warp,
    mask: api_mask,
    peek: api_peek,
    input_seq: api_input_seq,
    stick_deflected: api_stick_deflected,
};

extern "C" fn api_frame_for_pts(pts_us: i64) -> i64 {
    frame_for_pts(pts_us).map_or(-1, i64::from)
}

extern "C" fn api_input_seq() -> u32 {
    input_seq()
}

extern "C" fn api_stick_deflected() -> i32 {
    i32::from(stick_deflected())
}

/// # Safety
/// `out` is null or points at [`WARP_FLOATS`] writable floats.
unsafe extern "C" fn api_warp(frame: i64, out: *mut f32) -> i32 {
    let (code, values) = warp_for(u32::try_from(frame).ok());
    // SAFETY: as this function's.
    unsafe { write_shift(&values, out) };
    code
}

/// # Safety
/// `out` is null or points at [`WARP_FLOATS`] writable floats.
unsafe extern "C" fn api_peek(frame: i64, out: *mut f32) -> i32 {
    let (code, values) = peek_for(u32::try_from(frame).ok());
    // SAFETY: as this function's.
    unsafe { write_shift(&values, out) };
    code
}

/// # Safety
/// `out` is null or points at [`WARP_FLOATS`] writable floats.
unsafe fn write_shift(values: &[f32; WARP_FLOATS], out: *mut f32) {
    if !out.is_null() {
        // SAFETY: the caller passes that many floats (frame_warp_api.h).
        unsafe { std::ptr::copy_nonoverlapping(values.as_ptr(), out, values.len()) };
    }
}

/// # Safety
/// `width` and `height` are null or writable; `cells` is null or has `capacity` writable bytes.
unsafe extern "C" fn api_mask(known: i32, width: *mut u32, height: *mut u32, cells: *mut u8, capacity: usize) -> i32 {
    let warp = current();
    let mask = match mask_update(warp.as_deref(), known) {
        MaskUpdate::Unchanged => return MASK_UNCHANGED,
        MaskUpdate::Cleared => return MASK_CLEARED,
        MaskUpdate::New(mask) => mask,
    };
    // SAFETY: the caller passes null or writable pointers (frame_warp_api.h).
    unsafe {
        if !width.is_null() {
            *width = u32::from(mask.width);
        }
        if !height.is_null() {
            *height = u32::from(mask.height);
        }
    }
    let size = usize::from(mask.width) * usize::from(mask.height);
    if cells.is_null() || capacity < size {
        return MASK_TOO_SMALL;
    }
    // SAFETY: `cells` has `capacity` >= `size` writable bytes.
    let out = unsafe { std::slice::from_raw_parts_mut(cells, size) };
    if !pack_r8(&mask, out) {
        return MASK_TOO_SMALL;
    }
    i32::from(mask.generation)
}

/// The address Java hands the native renderers.
pub fn api_address() -> i64 {
    &API as *const FrameWarpApi as i64
}

#[cfg(test)]
mod tests;
