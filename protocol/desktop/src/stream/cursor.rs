use std::collections::VecDeque;

use nightjar_core::control::events::CursorShape;
use sdl3_sys::everything::*;

#[derive(Default)]
pub struct Cursor {
    cache: VecDeque<(u32, *mut SDL_Cursor)>,
    pub enabled: bool,
}

impl Cursor {
    pub fn apply(&mut self, shape: CursorShape) -> Result<(), String> {
        if !self.enabled {
            return Ok(());
        }
        unsafe {
            match shape {
                CursorShape::Hide => {
                    SDL_HideCursor();
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
                    let source = image::RgbaImage::from_raw(width, height, rgba)
                        .expect("validated cursor image");
                    let mut pixels = image::imageops::resize(
                        &source,
                        w,
                        h,
                        image::imageops::FilterType::Triangle,
                    );
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

    pub fn clear(&mut self) {
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
}
