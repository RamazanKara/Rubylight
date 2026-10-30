//! The window's place between launches, as Moonlight V+ PC (qiin2333/moonlight-qt) remembers its
//! window: the display it was on, and its position and size there. The next launch opens it there
//! again, or on the primary display when that display is gone. Saved in `window.json`.

use serde::{Deserialize, Serialize};
use sdl3_sys::everything::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Placement {
    /// The display's name and its bounds then: two displays of one model share a name.
    pub display: String,
    pub display_bounds: Rect,
    /// The windowed position, relative to the display's top left corner, and size.
    pub window: Rect,
    pub maximized: bool,
}

/// Where the window opens: the saved display by name and bounds, else by name alone; None when
/// it is gone, so the window opens on the primary display. The window is moved and shrunk to fit
/// the display's usable area if that changed. `frame` is the window's border sizes (top, left,
/// bottom, right): positions are the client area's, and the title bar stays on the display too.
pub fn restore(saved: &Placement, displays: &[(String, Rect)], frame: (i32, i32, i32, i32)) -> Option<Rect> {
    let (_, usable) = displays.iter().find(|(name, bounds)| *name == saved.display && *bounds == saved.display_bounds)
        .or_else(|| displays.iter().find(|(name, _)| *name == saved.display))?;
    let (top, left, bottom, right) = frame;
    let area = Rect { x: usable.x + left, y: usable.y + top, w: (usable.w - left - right).max(1), h: (usable.h - top - bottom).max(1) };
    let w = saved.window.w.clamp(1, area.w);
    let h = saved.window.h.clamp(1, area.h);
    let x = (usable.x + saved.window.x).clamp(area.x, area.x + area.w - w);
    let y = (usable.y + saved.window.y).clamp(area.y, area.y + area.h - h);
    Some(Rect { x, y, w, h })
}

/// Each display's name and usable bounds (without the taskbar).
fn displays() -> Vec<(SDL_DisplayID, String, Rect)> {
    let mut count = 0;
    // SAFETY: SDL allocates the ID list, which is read here and then freed; names are owned by SDL.
    unsafe {
        let ids = SDL_GetDisplays(&mut count);
        if ids.is_null() { return Vec::new(); }
        let list = (0..count.max(0) as usize).map(|i| {
            let id = *ids.add(i);
            let mut r = SDL_Rect::default();
            SDL_GetDisplayUsableBounds(id, &mut r);
            let name = SDL_GetDisplayName(id);
            let name = if name.is_null() { String::new() } else { std::ffi::CStr::from_ptr(name).to_string_lossy().into_owned() };
            (id, name, Rect { x: r.x, y: r.y, w: r.w, h: r.h })
        }).collect();
        SDL_free(ids.cast());
        list
    }
}

/// Moves the hidden new window to its saved place, or leaves it on the primary display (logged).
pub fn apply(window: *mut SDL_Window, saved: Option<&Placement>) {
    let Some(saved) = saved else { return };
    let named: Vec<(String, Rect)> = displays().into_iter().map(|(_, name, rect)| (name, rect)).collect();
    let (mut top, mut left, mut bottom, mut right) = (0, 0, 0, 0);
    // SAFETY: a live window on the main thread; on failure the borders stay 0.
    unsafe { SDL_GetWindowBordersSize(window, &mut top, &mut left, &mut bottom, &mut right) };
    let Some(rect) = restore(saved, &named, (top, left, bottom, right)) else {
        eprintln!("nightjar: window: display \"{}\" is gone, opening on the primary display", saved.display);
        return;
    };
    // SAFETY: a live window on the main thread.
    unsafe {
        SDL_SetWindowSize(window, rect.w, rect.h);
        SDL_SetWindowPosition(window, rect.x, rect.y);
        if saved.maximized { SDL_MaximizeWindow(window); }
    }
}

/// The window's place now, to save at exit. A minimized or fullscreen window keeps the windowed
/// place saved before (`previous`) and only records the display it is on.
pub fn current(window: *mut SDL_Window, previous: Option<&Placement>) -> Option<Placement> {
    // SAFETY: a live window on the main thread.
    unsafe {
        let flags = SDL_GetWindowFlags(window);
        if flags & SDL_WINDOW_MINIMIZED != 0 { return None; }
        let id = SDL_GetDisplayForWindow(window);
        let (_, display, bounds) = displays().into_iter().find(|(d, _, _)| *d == id)?;
        let mut placement = previous.cloned().unwrap_or_default();
        if placement.display != display || placement.display_bounds != bounds {
            // Another display: the old offset meant the old one.
            placement.window = Rect { x: 0, y: 0, w: placement.window.w, h: placement.window.h };
        }
        placement.display = display;
        placement.display_bounds = bounds;
        placement.maximized = flags & SDL_WINDOW_MAXIMIZED != 0;
        if flags & (SDL_WINDOW_FULLSCREEN | SDL_WINDOW_MAXIMIZED) == 0 {
            let (mut x, mut y, mut w, mut h) = (0, 0, 0, 0);
            SDL_GetWindowPosition(window, &mut x, &mut y);
            SDL_GetWindowSize(window, &mut w, &mut h);
            placement.window = Rect { x: x - bounds.x, y: y - bounds.y, w, h };
        }
        (placement.window.w > 0 && placement.window.h > 0).then_some(placement)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn placement(display: &str, bounds: Rect, window: Rect) -> Placement {
        Placement { display: display.into(), display_bounds: bounds, window, maximized: false }
    }

    const LEFT: Rect = Rect { x: 0, y: 0, w: 1920, h: 1040 };
    const RIGHT: Rect = Rect { x: 1920, y: 0, w: 2560, h: 1400 };
    const NO_FRAME: (i32, i32, i32, i32) = (0, 0, 0, 0);

    #[test]
    fn the_window_returns_to_its_display() {
        let displays = vec![("DELL U2720Q".to_owned(), LEFT), ("LG ULTRAGEAR".to_owned(), RIGHT)];
        let saved = placement("LG ULTRAGEAR", RIGHT, Rect { x: 100, y: 50, w: 1280, h: 800 });
        assert_eq!(restore(&saved, &displays, NO_FRAME), Some(Rect { x: 2020, y: 50, w: 1280, h: 800 }));
    }

    #[test]
    fn a_gone_display_falls_back_to_the_primary() {
        let displays = vec![("DELL U2720Q".to_owned(), LEFT)];
        let saved = placement("LG ULTRAGEAR", RIGHT, Rect { x: 100, y: 50, w: 1280, h: 800 });
        assert_eq!(restore(&saved, &displays, NO_FRAME), None);
    }

    #[test]
    fn twins_are_told_apart_by_bounds_and_a_moved_display_is_still_found() {
        let twins = vec![("DELL".to_owned(), LEFT), ("DELL".to_owned(), RIGHT)];
        let saved = placement("DELL", RIGHT, Rect { x: 0, y: 0, w: 1280, h: 800 });
        assert_eq!(restore(&saved, &twins, NO_FRAME).unwrap().x, 1920);
        let moved = Rect { x: -2560, ..RIGHT };
        let displays = vec![("DELL U2720Q".to_owned(), LEFT), ("LG".to_owned(), moved)];
        let saved = placement("LG", RIGHT, Rect { x: 10, y: 10, w: 1280, h: 800 });
        assert_eq!(restore(&saved, &displays, NO_FRAME), Some(Rect { x: -2550, y: 10, w: 1280, h: 800 }));
    }

    #[test]
    fn a_window_that_no_longer_fits_is_shrunk_and_kept_on_screen() {
        let small = Rect { x: 0, y: 0, w: 1280, h: 680 };
        let displays = vec![("TV".to_owned(), small)];
        let saved = placement("TV", LEFT, Rect { x: 1500, y: 900, w: 1600, h: 900 });
        assert_eq!(restore(&saved, &displays, NO_FRAME), Some(Rect { x: 0, y: 0, w: 1280, h: 680 }));
        let saved = placement("TV", small, Rect { x: 1000, y: -40, w: 800, h: 600 });
        assert_eq!(restore(&saved, &displays, NO_FRAME), Some(Rect { x: 480, y: 0, w: 800, h: 600 }));
        // A 31 px title bar and 8 px borders stay on the display as well.
        assert_eq!(restore(&saved, &displays, (31, 8, 8, 8)), Some(Rect { x: 472, y: 31, w: 800, h: 600 }));
        let saved = placement("TV", small, Rect { x: 0, y: 0, w: 1280, h: 680 });
        assert_eq!(restore(&saved, &displays, (31, 8, 8, 8)), Some(Rect { x: 8, y: 31, w: 1264, h: 641 }));
    }

    #[test]
    fn the_file_round_trips() {
        let saved = Placement { maximized: true, ..placement("LG", RIGHT, Rect { x: 1, y: 2, w: 3, h: 4 }) };
        let json = serde_json::to_string(&saved).unwrap();
        assert_eq!(serde_json::from_str::<Placement>(&json).unwrap(), saved);
    }
}
