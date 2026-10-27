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
        true
    }

    /// When a coalesced change is due, for the control loop's wait.
    pub fn next_due(&self) -> Option<Instant> {
        let state = self.state.lock().unwrap();
        let session = state.session.as_ref().filter(|s| s.dirty)?;
        session.sent_at.map(|at| at + INTERVAL)
    }

    /// The message to send control peer `owner` now (0x5541), if any: only the peer that
    /// enabled the session gets its shapes. `scale` reads a display's scale in percent, once
    /// a display a session.
    pub fn take(
        &self,
        owner: usize,
        now: Instant,
        scale: impl FnOnce(&str) -> anyhow::Result<u32>,
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
        session.dirty = false;
        let noted = session.noted.clone().flatten();
        let shape = match noted {
            None => CursorShape::Hide,
            Some(noted) => {
                if session
                    .scale
                    .as_ref()
                    .is_none_or(|(output, _)| *output != noted.output)
                {
                    let percent = match scale(&noted.output) {
                        Ok(percent) => percent,
                        Err(error) => {
                            tracing::warn!(output = %noted.output, error = %format!("{error:#}"), "display scale unavailable for the client cursor; sent at 100%");
                            100
                        }
                    };
                    session.scale = Some((noted.output.clone(), clamp_scale(percent)));
                }
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
mod tests {
    use super::*;

    fn request(enabled: bool, max_edge: u16) -> [u8; 4] {
        CursorShapeRequest { enabled, max_edge }.encode()
    }

    fn solid(width: u32, height: u32, rgba: [u8; 4]) -> Arc<[u8]> {
        rgba.repeat((width * height) as usize).into()
    }

    fn pointer(pixels: &Arc<[u8]>, width: u32, height: u32) -> Pointer<'_> {
        Pointer {
            width,
            height,
            hotspot: [1, 2],
            logic: false,
            pixels,
        }
    }

    fn scale(percent: u32) -> impl FnOnce(&str) -> anyhow::Result<u32> {
        move |_| Ok(percent)
    }

    fn decoded(message: Option<Vec<u8>>) -> CursorShape {
        CursorShape::decode(&message.expect("a message")).expect("a valid message")
    }

    /// Without an offered enable nothing is noted, sent or left out of the video; a disable
    /// ends the session.
    #[test]
    fn only_an_offered_enable_starts_a_session() {
        let cursor = ClientCursor::default();
        let arrow = solid(4, 4, [10, 20, 30, 255]);
        let now = Instant::now();
        assert_eq!(cursor.on_request(&request(true, 96), false, 1), None);
        assert_eq!(cursor.on_request(&[1, 2, 96, 0], true, 1), None);
        assert_eq!(cursor.on_request(&request(true, 97)[..], true, 1), None);
        assert!(!cursor.active());
        assert!(!cursor.note(Some(pointer(&arrow, 4, 4)), "D1"));
        assert_eq!(cursor.take(1, now, scale(100)), None);
        assert_eq!(cursor.on_request(&request(true, 96), true, 1), Some(true));
        assert!(cursor.active());
        assert!(cursor.note(Some(pointer(&arrow, 4, 4)), "D1"));
        // Another control peer on the session gets nothing.
        assert_eq!(cursor.take(2, now, scale(100)), None);
        assert!(matches!(
            decoded(cursor.take(1, now, scale(100))),
            CursorShape::Image { .. }
        ));
        // A disable is taken after the offer went too.
        assert_eq!(
            cursor.on_request(&request(false, 96), false, 1),
            Some(false)
        );
        assert!(!cursor.active());
        assert!(!cursor.note(None, "D1"));
        assert_eq!(cursor.take(1, now + INTERVAL, scale(100)), None);
        // Only the peer that enabled it ends the session, by a disable or when it goes.
        cursor.on_request(&request(true, 96), true, 1);
        assert_eq!(cursor.on_request(&request(false, 96), true, 2), Some(false));
        cursor.end(2);
        assert!(cursor.active());
        cursor.end(1);
        assert!(!cursor.active());
    }

    /// A shape is sent once a session and shown again by id; it is sent again after it
    /// dropped out of the device's 32 most recent, and after a fresh enable.
    #[test]
    fn shapes_are_cached_by_id() {
        let cursor = ClientCursor::default();
        let mut now = Instant::now();
        cursor.on_request(&request(true, 96), true, 1);
        let shapes: Vec<Arc<[u8]>> = (0..=CACHE_ENTRIES as u8)
            .map(|n| solid(2, 3, [n, 0, 0, 255]))
            .collect();
        let show = |pixels: &Arc<[u8]>, now: Instant| {
            assert!(cursor.note(Some(pointer(pixels, 2, 3)), "D1"));
            decoded(cursor.take(1, now, scale(150)))
        };
        let CursorShape::Image { id: first, image } = show(&shapes[0], now) else {
            panic!("first shape not defined");
        };
        assert_eq!(
            (
                image.width,
                image.height,
                image.hotspot_x,
                image.hotspot_y,
                image.scale
            ),
            (2, 3, 1, 2, 150)
        );
        assert_eq!(image.pixels, shapes[0].to_vec());
        now += INTERVAL;
        assert!(matches!(show(&shapes[1], now), CursorShape::Image { .. }));
        now += INTERVAL;
        // A copy of the first shape in another allocation is the same image.
        let copy: Arc<[u8]> = shapes[0].to_vec().into();
        assert_eq!(show(&copy, now), CursorShape::Show { id: first });
        // 32 other shapes push the first out of the device's cache.
        for shape in &shapes[1..] {
            now += INTERVAL;
            show(shape, now);
        }
        now += INTERVAL;
        assert!(matches!(show(&shapes[0], now), CursorShape::Image { id, .. } if id == first));
        now += INTERVAL;
        assert!(matches!(
            show(&shapes[CACHE_ENTRIES], now),
            CursorShape::Show { .. }
        ));
        // A fresh enable forgets what the device cached.
        cursor.on_request(&request(true, 96), true, 1);
        now += INTERVAL;
        assert!(matches!(
            show(&shapes[CACHE_ENTRIES], now),
            CursorShape::Image { .. }
        ));
    }

    /// The first change is sent at once, then the newest at most every interval; the same
    /// pointer noted again or a change back to what the device shows sends nothing.
    #[test]
    fn changes_are_coalesced() {
        let cursor = ClientCursor::default();
        let start = Instant::now();
        cursor.on_request(&request(true, 96), true, 1);
        assert_eq!(cursor.next_due(), None);
        let arrow = solid(4, 4, [1, 1, 1, 255]);
        let beam = solid(4, 4, [2, 2, 2, 255]);
        let hand = solid(4, 4, [3, 3, 3, 255]);
        assert!(cursor.note(None, "D1"));
        assert_eq!(
            decoded(cursor.take(1, start, scale(100))),
            CursorShape::Hide
        );
        assert!(!cursor.note(None, "D1"));
        assert!(cursor.note(Some(pointer(&arrow, 4, 4)), "D1"));
        assert!(!cursor.note(Some(pointer(&arrow, 4, 4)), "D1"));
        assert_eq!(cursor.next_due(), Some(start + INTERVAL));
        assert_eq!(cursor.take(1, start + INTERVAL / 2, scale(100)), None);
        assert!(cursor.note(Some(pointer(&beam, 4, 4)), "D1"));
        assert!(cursor.note(Some(pointer(&hand, 4, 4)), "D1"));
        let CursorShape::Image { image, .. } =
            decoded(cursor.take(1, start + INTERVAL, scale(100)))
        else {
            panic!("newest shape not sent");
        };
        assert_eq!(image.pixels, hand.to_vec());
        assert_eq!(cursor.next_due(), None);
        assert_eq!(cursor.take(1, start + INTERVAL * 3, scale(100)), None);
        // Hidden and back within an interval: the device already shows the hand.
        assert!(cursor.note(None, "D1"));
        assert!(cursor.note(Some(pointer(&hand, 4, 4)), "D1"));
        assert_eq!(cursor.take(1, start + INTERVAL * 4, scale(100)), None);
    }

    /// The scale is read once a display, and a failed read sends 100%.
    #[test]
    fn the_scale_is_read_per_display() {
        let cursor = ClientCursor::default();
        let mut now = Instant::now();
        cursor.on_request(&request(true, 96), true, 1);
        let a = solid(4, 4, [1, 1, 1, 255]);
        let b = solid(4, 4, [2, 2, 2, 255]);
        let scale_of = |shape: CursorShape| match shape {
            CursorShape::Image { image, .. } => image.scale,
            other => panic!("{other:?}"),
        };
        cursor.note(Some(pointer(&a, 4, 4)), "D1");
        assert_eq!(scale_of(decoded(cursor.take(1, now, scale(250)))), 250);
        cursor.note(Some(pointer(&b, 4, 4)), "D1");
        now += INTERVAL;
        assert_eq!(
            scale_of(decoded(cursor.take(1, now, |_| panic!("read again")))),
            250
        );
        cursor.note(Some(pointer(&a, 4, 4)), "D2");
        now += INTERVAL;
        assert_eq!(
            scale_of(decoded(cursor.take(1, now, |_| anyhow::bail!("gone")))),
            100
        );
        cursor.note(Some(pointer(&b, 4, 4)), "D3");
        now += INTERVAL;
        assert_eq!(
            scale_of(decoded(cursor.take(1, now, scale(900)))),
            MAX_SCALE
        );
    }

    fn converted(noted: Noted, max_edge: u16, scale: u16) -> CursorImage {
        match image(&noted, max_edge, scale) {
            Converted::Image(image) => image,
            Converted::Hidden => panic!("hidden"),
            Converted::Unusable(why) => panic!("{why}"),
        }
    }

    fn noted(width: u32, height: u32, logic: bool, pixels: Vec<u8>) -> Noted {
        Noted {
            width,
            height,
            hotspot: [0, 0],
            logic,
            pixels: pixels.into(),
            output: "D1".into(),
        }
    }

    /// Inverting pixels become black with a white outline, a transparent XOR is left out, and
    /// opaque logic pixels keep their color.
    #[test]
    fn inverting_pixels_become_black_with_a_white_outline() {
        // A 3x3 monochrome shape: one inverting pixel in the middle, a no-op XOR at the top
        // left, an opaque red one at the bottom right.
        let mut pixels = vec![0u8; 9 * 4];
        pixels[4 * 4..4 * 4 + 4].copy_from_slice(&[255, 255, 255, 128]);
        pixels[..4].copy_from_slice(&[0, 0, 0, 128]);
        pixels[8 * 4..].copy_from_slice(&[255, 0, 0, 255]);
        let image = converted(noted(3, 3, true, pixels), 96, 100);
        let at = |n: usize| &image.pixels[n * 4..n * 4 + 4];
        assert_eq!(at(4), [0, 0, 0, 255]);
        for outline in [0, 1, 2, 3, 5, 6, 7] {
            assert_eq!(at(outline), [255, 255, 255, 255], "{outline}");
        }
        assert_eq!(at(8), [255, 0, 0, 255]);
        // A color shape keeps its straight alpha.
        let glass = converted(noted(1, 1, false, vec![9, 8, 7, 100]), 96, 100);
        assert_eq!(glass.pixels, [9, 8, 7, 100]);
    }

    /// A shape past the device's edge is shrunk with its hotspot and scale; an invisible one
    /// hides the pointer and a malformed one is not sent.
    #[test]
    fn large_and_unusable_shapes() {
        let mut big = noted(128, 64, false, [0, 0, 255, 255].repeat(128 * 64));
        big.hotspot = [127, 32];
        let image = converted(big.clone(), 96, 200);
        assert_eq!((image.width, image.height), (96, 48));
        assert_eq!((image.hotspot_x, image.hotspot_y), (95, 24));
        assert_eq!(image.scale, 150);
        assert!(
            image
                .pixels
                .as_chunks::<4>()
                .0
                .iter()
                .all(|p| *p == [0, 0, 255, 255])
        );
        // Shrunk below 100% the scale stays at the least the protocol takes.
        let image = converted(big, 32, 100);
        assert_eq!((image.width, image.height, image.scale), (32, 16, 100));
        // Half transparent: the color is weighted by alpha.
        let mut pixels = [0u8; 8];
        pixels[..4].copy_from_slice(&[200, 100, 50, 255]);
        let image = converted(noted(2, 1, false, pixels.to_vec()), 1, 100);
        assert_eq!(image.pixels, [200, 100, 50, 128]);
        assert!(matches!(
            image_of(noted(2, 2, false, vec![0; 16])),
            Converted::Hidden
        ));
        assert!(matches!(
            image_of(noted(2, 2, true, [0, 0, 0, 128].repeat(4))),
            Converted::Hidden
        ));
        assert!(matches!(
            image_of(noted(2, 2, false, vec![255; 12])),
            Converted::Unusable(_)
        ));
        assert!(matches!(
            image_of(noted(0, 2, false, Vec::new())),
            Converted::Unusable(_)
        ));
        // Hidden shapes are sent as HIDE, malformed ones as DEFAULT.
        let cursor = ClientCursor::default();
        cursor.on_request(&request(true, 96), true, 1);
        let clear: Arc<[u8]> = vec![0; 16].into();
        cursor.note(Some(pointer(&clear, 2, 2)), "D1");
        let now = Instant::now();
        assert_eq!(decoded(cursor.take(1, now, scale(100))), CursorShape::Hide);
        let short: Arc<[u8]> = vec![255; 12].into();
        cursor.note(Some(pointer(&short, 2, 2)), "D1");
        assert_eq!(
            decoded(cursor.take(1, now + INTERVAL, scale(100))),
            CursorShape::Default
        );
    }

    fn image_of(noted: Noted) -> Converted {
        image(&noted, 96, 100)
    }
}
