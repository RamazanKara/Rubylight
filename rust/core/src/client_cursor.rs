//! A cursor the device draws itself (`rubylight_protocol::cursor_shape`, control messages
//! 0x5540 and 0x5541, Experimental).
//!
//! Between a device's enable and disable the stream leaves the pointer out of the video
//! and notes it here each picture; the control loop sends the pointer's shape when it
//! changed, at most one message per [`INTERVAL`] with the newest winning. A shape is sent
//! once a session: one the device still caches is shown again by id. The device keeps
//! drawing the pointer where it sends absolute mouse positions, so no position is sent.
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rubylight_protocol::cursor_shape::{CACHE_ENTRIES, MAX_SCALE, MIN_SCALE};
pub use rubylight_protocol::cursor_shape::{
    CURSOR_SHAPE_MESSAGE_TYPE, CURSOR_SHAPE_REQUEST_MESSAGE_TYPE, CursorImage, CursorShape,
    CursorShapeRequest,
};

/// Changes closer than this are coalesced: at most about 60 messages a second.
pub const INTERVAL: Duration = Duration::from_millis(16);

/// The pointer as the capture supplies it.
#[derive(Clone, Copy)]
pub struct Pointer<'a> {
    pub width: u32,
    pub height: u32,
    /// The point that clicks, from the image's top left.
    pub hotspot: [i32; 2],
    /// Desktop Duplication's logic alpha (0 transparent, 128 inverts the screen by the
    /// pixel's color, 255 opaque) for monochrome and masked shapes; straight alpha otherwise.
    pub logic: bool,
    /// `width * height` RGBA pixels, rows top to bottom.
    pub pixels: &'a Arc<[u8]>,
}

/// One stream's client-drawn cursor: the stream notes the pointer, the control loop takes
/// the messages.
#[derive(Default)]
pub struct ClientCursor {
    /// Between an enable and a disable: the stream leaves the pointer out of the video.
    active: AtomicBool,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    session: Option<Session>,
}

struct Session {
    /// The control peer that enabled it.
    owner: usize,
    max_edge: u16,
    /// The pointer as last noted (None: hidden), once one was.
    noted: Option<Option<Noted>>,
    /// Something was noted since the last message.
    dirty: bool,
    sent_at: Option<Instant>,
    /// The ids this session sent, least recently used first, with each image's hash.
    sent: Vec<(u32, u64)>,
    /// What the device shows now.
    shown: Option<Shown>,
    /// The display the scale was read for, and its scale in percent.
    scale: Option<(String, u16)>,
    /// Whether the pending shape waits for its display's scale, read elsewhere: no
    /// deadline then, as the reader wakes the caller once it is known.
    scale_pending: bool,
}

#[derive(Clone)]
struct Noted {
    width: u32,
    height: u32,
    hotspot: [i32; 2],
    logic: bool,
    pixels: Arc<[u8]>,
    output: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Shown {
    Hide,
    Default,
    Id(u32),
}

impl ClientCursor {
    /// A request from the device (0x5540) on control peer `owner`: Some(enabled) when it
    /// was valid, and offered unless it disables. Every enable starts a fresh session; a
    /// disable ends the peer's own, offered or not, so the pointer goes back into the video.
    pub fn on_request(&self, payload: &[u8], offered: bool, owner: usize) -> Option<bool> {
        let request =
            CursorShapeRequest::decode(payload).filter(|request| offered || !request.enabled)?;
        let mut state = self.state.lock().unwrap();
        if request.enabled {
            state.session = Some(Session {
                owner,
                max_edge: request.max_edge,
                noted: None,
                dirty: false,
                sent_at: None,
                sent: Vec::new(),
                shown: None,
                scale: None,
                scale_pending: false,
            });
        } else if state.session.as_ref().is_some_and(|s| s.owner == owner) {
            state.session = None;
        }
        self.active
            .store(state.session.is_some(), Ordering::Release);
        Some(request.enabled)
    }

    /// Control peer `owner` went: its session ends, and the pointer is drawn into the
    /// video again. Another peer's session stays.
    pub fn end(&self, owner: usize) {
        let mut state = self.state.lock().unwrap();
        if state.session.as_ref().is_some_and(|s| s.owner == owner) {
            state.session = None;
            self.active.store(false, Ordering::Release);
        }
    }

    /// Whether the stream leaves the pointer out of the video.
    pub fn active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    /// The stream's pointer for the picture it encodes on `output` (None: hidden). True when
    /// it changed, so the control loop should run.
    pub fn note(&self, pointer: Option<Pointer<'_>>, output: &str) -> bool {
        let mut state = self.state.lock().unwrap();
        let Some(session) = state.session.as_mut() else {
            return false;
        };
        let same = match (&session.noted, &pointer) {
            (Some(None), None) => true,
            (Some(Some(noted)), Some(pointer)) => {
                Arc::ptr_eq(&noted.pixels, pointer.pixels)
                    && (noted.width, noted.height, noted.hotspot, noted.logic)
                        == (
                            pointer.width,
                            pointer.height,
                            pointer.hotspot,
                            pointer.logic,
                        )
                    && noted.output == output
            }
            _ => false,
        };
        if same {
            return false;
        }
        session.noted = Some(pointer.map(|pointer| Noted {
            width: pointer.width,
            height: pointer.height,
            hotspot: pointer.hotspot,
            logic: pointer.logic,
            pixels: pointer.pixels.clone(),
            output: output.to_owned(),
        }));
        session.dirty = true;
        // Due again: the next take asks for the scale anew.
        session.scale_pending = false;
        true
    }

    /// When a coalesced change is due, for the control loop's wait.
    pub fn next_due(&self) -> Option<Instant> {
        let state = self.state.lock().unwrap();
        let session = state
            .session
            .as_ref()
            .filter(|s| s.dirty && !s.scale_pending)?;
        session.sent_at.map(|at| at + INTERVAL)
    }

    /// The message to send control peer `owner` now (0x5541), if any: only the peer that
    /// enabled the session gets its shapes. `scale` gives a display's scale in percent, once
    /// a display a session; None while it is still being read, and the shape waits for the
    /// caller's next pass.
    pub fn take(
        &self,
        owner: usize,
        now: Instant,
        scale: impl FnOnce(&str) -> Option<anyhow::Result<u32>>,
    ) -> Option<Vec<u8>> {
        let mut state = self.state.lock().unwrap();
        let session = state.session.as_mut().filter(|s| s.owner == owner)?;
        if !session.dirty
            || session
                .sent_at
                .is_some_and(|at| now.saturating_duration_since(at) < INTERVAL)
        {
            return None;
        }
        let noted = session.noted.clone().flatten();
        if let Some(noted) = &noted
            && session
                .scale
                .as_ref()
                .is_none_or(|(output, _)| *output != noted.output)
        {
            let Some(percent) = scale(&noted.output) else {
                session.scale_pending = true;
                return None;
            };
            let percent = match percent {
                Ok(percent) => percent,
                Err(error) => {
                    tracing::warn!(output = %noted.output, error = %format!("{error:#}"), "display scale unavailable for the client cursor; sent at 100%");
                    100
                }
            };
            session.scale = Some((noted.output.clone(), clamp_scale(percent)));
        }
        session.dirty = false;
        session.scale_pending = false;
        let shape = match noted {
            None => CursorShape::Hide,
            Some(noted) => {
                let display_scale = session.scale.as_ref().map_or(MIN_SCALE, |s| s.1);
                match image(&noted, session.max_edge, display_scale) {
                    Converted::Hidden => CursorShape::Hide,
                    Converted::Unusable(why) => {
                        tracing::warn!(
                            width = noted.width,
                            height = noted.height,
                            why,
                            "pointer shape not sent; the device draws its own"
                        );
                        CursorShape::Default
                    }
                    Converted::Image(image) => session.define(image),
                }
            }
        };
        let shown = match &shape {
            CursorShape::Hide => Shown::Hide,
            CursorShape::Default => Shown::Default,
            CursorShape::Show { id } | CursorShape::Image { id, .. } => Shown::Id(*id),
        };
        // A change back to what the device shows sends nothing.
        if session.shown == Some(shown) && !matches!(shape, CursorShape::Image { .. }) {
            return None;
        }
        session.shown = Some(shown);
        session.sent_at = Some(now);
        shape.encode()
    }
}

impl Session {
    /// SHOW for an id the device still caches with this image, IMAGE otherwise; the id
    /// becomes the most recently used.
    fn define(&mut self, image: CursorImage) -> CursorShape {
        let hash = hash(&image);
        let id = ((hash as u32) ^ ((hash >> 32) as u32)).max(1);
        let cached = self.sent.iter().position(|&(sent, _)| sent == id);
        let same = cached.is_some_and(|at| self.sent[at].1 == hash);
        if let Some(at) = cached {
            self.sent.remove(at);
        }
        self.sent.push((id, hash));
        if self.sent.len() > CACHE_ENTRIES {
            self.sent.remove(0);
        }
        if same {
            CursorShape::Show { id }
        } else {
            CursorShape::Image { id, image }
        }
    }
}

fn clamp_scale(percent: u32) -> u16 {
    percent.clamp(u32::from(MIN_SCALE), u32::from(MAX_SCALE)) as u16
}

/// FNV-1a over the image as sent.
fn hash(image: &CursorImage) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    let fields = [
        image.width,
        image.height,
        image.hotspot_x,
        image.hotspot_y,
        image.scale,
    ];
    for byte in fields
        .iter()
        .flat_map(|field| field.to_le_bytes())
        .chain(image.pixels.iter().copied())
    {
        hash = (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

enum Converted {
    Image(CursorImage),
    /// Nothing of the pointer is visible.
    Hidden,
    Unusable(&'static str),
}

/// The pointer as straight-alpha RGBA, at most `max_edge` on a side. Pixels that invert the
/// screen become black with a white outline; a shape larger than `max_edge` is shrunk, and
/// its hotspot and scale with it.
fn image(noted: &Noted, max_edge: u16, display_scale: u16) -> Converted {
    let (width, height) = (noted.width as usize, noted.height as usize);
    if width == 0 || height == 0 || width > 1024 || height > 1024 {
        return Converted::Unusable("size");
    }
    if noted.pixels.len() != width * height * 4 {
        return Converted::Unusable("pixel count");
    }
    let mut rgba = noted.pixels.to_vec();
    if noted.logic {
        let inverting: Vec<bool> = rgba
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| p[3] == 128 && p[..3] != [0, 0, 0])
            .collect();
        for (at, pixel) in rgba.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            *pixel = match pixel[3] {
                255 => [pixel[0], pixel[1], pixel[2], 255],
                128 if inverting[at] => [0, 0, 0, 255],
                _ => [0; 4],
            };
        }
        for y in 0..height {
            for x in 0..width {
                let at = y * width + x;
                if rgba[at * 4 + 3] != 0 {
                    continue;
                }
                let outline = (y.saturating_sub(1)..(y + 2).min(height)).any(|ny| {
                    (x.saturating_sub(1)..(x + 2).min(width)).any(|nx| inverting[ny * width + nx])
                });
                if outline {
                    rgba[at * 4..at * 4 + 4].copy_from_slice(&[255, 255, 255, 255]);
                }
            }
        }
    }
    if rgba.as_chunks::<4>().0.iter().all(|p| p[3] == 0) {
        return Converted::Hidden;
    }
    let hotspot = [
        noted.hotspot[0].clamp(0, width as i32 - 1) as usize,
        noted.hotspot[1].clamp(0, height as i32 - 1) as usize,
    ];
    let edge = usize::from(max_edge.max(1));
    let longer = width.max(height);
    let (image_width, image_height, pixels, scale) = if longer <= edge {
        (width, height, rgba, display_scale)
    } else {
        let to_width = (width * edge / longer).max(1);
        let to_height = (height * edge / longer).max(1);
        let scale = (u32::from(display_scale) * edge as u32 / longer as u32) as u16;
        (
            to_width,
            to_height,
            shrink(&rgba, (width, height), (to_width, to_height)),
            scale.max(MIN_SCALE),
        )
    };
    let at = |point: usize, from: usize, to: usize| (point * to / from).min(to - 1) as u16;
    Converted::Image(CursorImage {
        width: image_width as u16,
        height: image_height as u16,
        hotspot_x: at(hotspot[0], width, image_width),
        hotspot_y: at(hotspot[1], height, image_height),
        scale,
        pixels,
    })
}

/// Box-filters straight-alpha RGBA from `from` to the smaller `to`, weighting color by alpha.
fn shrink(rgba: &[u8], from: (usize, usize), to: (usize, usize)) -> Vec<u8> {
    let mut out = Vec::with_capacity(to.0 * to.1 * 4);
    for ty in 0..to.1 {
        let (y0, y1) = (
            ty * from.1 / to.1,
            ((ty + 1) * from.1 / to.1).max(ty * from.1 / to.1 + 1),
        );
        for tx in 0..to.0 {
            let (x0, x1) = (
                tx * from.0 / to.0,
                ((tx + 1) * from.0 / to.0).max(tx * from.0 / to.0 + 1),
            );
            let (mut color, mut alpha, mut count) = ([0u64; 3], 0u64, 0u64);
            for y in y0..y1 {
                for x in x0..x1 {
                    let p = &rgba[(y * from.0 + x) * 4..][..4];
                    let a = u64::from(p[3]);
                    for channel in 0..3 {
                        color[channel] += u64::from(p[channel]) * a;
                    }
                    alpha += a;
                    count += 1;
                }
            }
            // A fully transparent box has no color.
            let channel = |c: u64| (c + alpha / 2).checked_div(alpha).unwrap_or(0) as u8;
            out.extend([
                channel(color[0]),
                channel(color[1]),
                channel(color[2]),
                ((alpha + count / 2) / count) as u8,
            ]);
        }
    }
    out
}

#[cfg(test)]
mod tests;
