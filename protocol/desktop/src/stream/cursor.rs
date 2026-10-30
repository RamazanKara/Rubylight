use std::collections::VecDeque;
use std::time::{Duration, Instant};

use image::{Rgba, RgbaImage};
use nightjar_core::control::events::CursorShape;
use sdl3_sys::everything::*;

/// How long a hide from the PC waits for a show that cancels it. Windows hides the cursor for a
/// moment while it changes it, and hiding at once made it flicker. Moonlight V+'s
/// `REMOTE_CURSOR_HIDE_DEBOUNCE_MS` (https://github.com/qiin2333/moonlight-qt, GPL-3.0).
pub const HIDE_DELAY: Duration = Duration::from_millis(150);

#[derive(Default)]
pub struct Cursor {
    cache: VecDeque<(u32, *mut SDL_Cursor)>,
    pub enabled: bool,
    /// When a hide from the PC takes effect, unless a show comes first.
    hide_at: Option<Instant>,
}

/// What a hide from the PC does: when it takes effect, if it waits for one.
fn hide_due(pending: Option<Instant>, visible: bool, now: Instant) -> Option<Instant> {
    match pending {
        Some(at) => Some(at),
        None if visible => Some(now + HIDE_DELAY),
        None => None,
    }
}

impl Cursor {
    pub fn apply(&mut self, shape: CursorShape) -> Result<(), String> {
        if !self.enabled {
            return Ok(());
        }
        if !matches!(shape, CursorShape::Hide) {
            self.hide_at = None;
        }
        unsafe {
            match shape {
                CursorShape::Hide => {
                    self.hide_at = hide_due(self.hide_at, SDL_CursorVisible(), Instant::now());
                }
                CursorShape::Default => self.default_cursor(),
                CursorShape::Show { id } => {
                    let Some(index) = self.cache.iter().position(|(key, _)| *key == id) else {
                        self.default_cursor();
                        return Err("The PC referenced an unknown cursor.".into());
                    };
                    let entry = self.cache.remove(index).unwrap();
                    SDL_SetCursor(entry.1);
                    SDL_ShowCursor();
                    self.cache.push_back(entry);
                }
                CursorShape::Image {
                    id,
                    width,
                    height,
                    hotspot_x,
                    hotspot_y,
                    scale_percent,
                    rgba,
                } => {
                    let width = u32::from(width);
                    let height = u32::from(height);
                    let (w, h, x, y) = geometry(width, height, hotspot_x, hotspot_y, scale_percent);
                    let source = RgbaImage::from_raw(width, height, rgba)
                        .expect("validated cursor image");
                    let mut pixels = scale(source, w, h);
                    let surface = SDL_CreateSurfaceFrom(
                        w as i32,
                        h as i32,
                        SDL_PIXELFORMAT_RGBA32,
                        pixels.as_mut_ptr().cast(),
                        (w * 4) as i32,
                    );
                    if surface.is_null() {
                        self.default_cursor();
                        return Err(crate::sdl::error());
                    }
                    let cursor = SDL_CreateColorCursor(surface, x as i32, y as i32);
                    SDL_DestroySurface(surface);
                    if cursor.is_null() {
                        self.default_cursor();
                        return Err(crate::sdl::error());
                    }
                    SDL_SetCursor(cursor);
                    SDL_ShowCursor();
                    if let Some(index) = self.cache.iter().position(|(key, _)| *key == id) {
                        let (_, old) = self.cache.remove(index).unwrap();
                        SDL_DestroyCursor(old);
                    }
                    self.cache.push_back((id, cursor));
                    if self.cache.len() > 32 {
                        let (_, old) = self.cache.pop_front().unwrap();
                        SDL_DestroyCursor(old);
                    }
                }
            }
        }
        Ok(())
    }

    pub fn default_cursor(&self) {
        unsafe {
            SDL_SetCursor(SDL_GetDefaultCursor());
            SDL_ShowCursor();
        }
    }

    /// How long until a hide from the PC takes effect, for the loop's wait.
    pub fn hide_in(&self, now: Instant) -> Option<Duration> {
        self.hide_at.map(|at| at.saturating_duration_since(now))
    }

    /// Hides the cursor once a hide from the PC has waited out [`HIDE_DELAY`].
    pub fn tick(&mut self, now: Instant) {
        if self.hide_at.is_some_and(|at| at <= now) {
            self.hide_at = None;
            unsafe { SDL_HideCursor() };
        }
    }

    pub fn clear(&mut self) {
        self.hide_at = None;
        self.default_cursor();
        for (_, cursor) in self.cache.drain(..) {
            unsafe { SDL_DestroyCursor(cursor) };
        }
        self.enabled = false;
    }
}

impl Drop for Cursor {
    fn drop(&mut self) {
        self.clear();
    }
}

/// `source` at `w` by `h`. The colours are premultiplied by their alpha for the filter, so the
/// colour of fully transparent pixels (black, mostly) does not bleed into the edges and darken
/// them; SDL takes straight alpha again. Moonlight V+'s scaling of host cursors.
fn scale(source: RgbaImage, w: u32, h: u32) -> RgbaImage {
    if source.dimensions() == (w, h) {
        return source;
    }
    let premultiplied = image::Rgba32FImage::from_fn(source.width(), source.height(), |x, y| {
        let [r, g, b, a] = source.get_pixel(x, y).0.map(|c| f32::from(c) / 255.0);
        Rgba([r * a, g * a, b * a, a])
    });
    let scaled = image::imageops::resize(
        &premultiplied,
        w,
        h,
        image::imageops::FilterType::Triangle,
    );
    let byte = |c: f32| (c.clamp(0.0, 1.0) * 255.0).round() as u8;
    RgbaImage::from_fn(w, h, |x, y| {
        let [r, g, b, a] = scaled.get_pixel(x, y).0;
        if a <= 0.0 {
            return Rgba([0, 0, 0, 0]);
        }
        Rgba([byte(r / a), byte(g / a), byte(b / a), byte(a)])
    })
}

fn geometry(width: u32, height: u32, x: u16, y: u16, scale: u16) -> (u32, u32, u32, u32) {
    let scale = u32::from(scale);
    let w = ((width * 100 + scale / 2) / scale).max(1);
    let h = ((height * 100 + scale / 2) / scale).max(1);
    (
        w,
        h,
        (u32::from(x) * 100 / scale).min(w - 1),
        (u32::from(y) * 100 / scale).min(h - 1),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_size_uses_host_scale_and_keeps_hotspot_inside() {
        assert_eq!(geometry(64, 48, 63, 47, 200), (32, 24, 31, 23));
        assert_eq!(geometry(1, 1, 0, 0, 400), (1, 1, 0, 0));
        assert_eq!(geometry(32, 32, 5, 7, 100), (32, 32, 5, 7));
    }

    #[test]
    fn scaling_keeps_the_edges_of_a_cursor_their_colour() {
        // A white cursor on fully transparent black, halved: the edge pixels mix the two.
        let source = RgbaImage::from_fn(8, 8, |x, _| {
            if x < 4 { Rgba([255, 255, 255, 255]) } else { Rgba([0, 0, 0, 0]) }
        });
        let scaled = scale(source, 4, 4);
        for pixel in scaled.pixels() {
            if pixel[3] > 0 {
                assert_eq!(&pixel.0[..3], &[255, 255, 255], "{pixel:?}");
            }
        }
        let edge = scaled.get_pixel(2, 1);
        assert!(edge[3] > 0 && edge[3] < 255, "{edge:?}");
        // A cursor at its size stays as it came.
        let same = RgbaImage::from_fn(3, 2, |x, y| Rgba([x as u8, y as u8, 9, 100]));
        assert_eq!(scale(same.clone(), 3, 2), same);
    }

    #[test]
    fn a_hide_waits_and_a_second_hide_does_not_put_it_off() {
        let now = Instant::now();
        assert_eq!(hide_due(None, true, now), Some(now + HIDE_DELAY));
        let pending = Some(now + HIDE_DELAY);
        assert_eq!(hide_due(pending, true, now + HIDE_DELAY / 2), pending);
        // Already hidden: nothing to wait for.
        assert_eq!(hide_due(None, false, now), None);
    }

    #[test]
    fn a_show_before_the_hide_is_due_keeps_the_cursor() {
        let _sdl = crate::sdl::TEST_LOCK.lock().unwrap();
        let mut cursor = Cursor::default();
        cursor.enabled = true;
        let now = Instant::now();
        cursor.hide_at = Some(now + HIDE_DELAY);
        assert_eq!(cursor.hide_in(now), Some(HIDE_DELAY));
        assert_eq!(cursor.hide_in(now + HIDE_DELAY * 2), Some(Duration::ZERO));
        cursor.tick(now);
        assert!(cursor.hide_at.is_some(), "not yet due");
        // The PC's own arrow (or any shape) cancels the hide.
        cursor.apply(CursorShape::Default).unwrap();
        assert_eq!(cursor.hide_in(now), None);
        cursor.hide_at = Some(now);
        cursor.tick(now);
        assert_eq!(cursor.hide_at, None);
        cursor.hide_at = Some(now + HIDE_DELAY);
        cursor.clear();
        assert_eq!(cursor.hide_in(now), None);
    }
}
