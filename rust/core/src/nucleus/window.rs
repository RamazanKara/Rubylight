//! Which window of a Nucleus instance a stream captures, and where it waits
//! while it streams: beside the PC's desktop, where the local mouse cannot
//! reach it and no monitor shows it.
use std::collections::BTreeSet;

/// A rectangle in desktop coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}
impl Rect {
    pub fn right(&self) -> i64 {
        i64::from(self.x) + i64::from(self.width)
    }
    pub fn bottom(&self) -> i64 {
        i64::from(self.y) + i64::from(self.height)
    }
    fn area(&self) -> i64 {
        i64::from(self.width.max(0)) * i64::from(self.height.max(0))
    }
    fn intersects(&self, other: &Rect) -> bool {
        i64::from(self.x) < other.right()
            && i64::from(other.x) < self.right()
            && i64::from(self.y) < other.bottom()
            && i64::from(other.y) < self.bottom()
    }
}

/// A top-level window as the OS reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Window {
    pub id: u64,
    pub pid: u32,
    pub rect: Rect,
    pub visible: bool,
    /// Owned windows are dialogs and popups of another window.
    pub owned: bool,
    /// Tool windows (overlays, launchers' helpers) never show in the taskbar.
    pub tool: bool,
}

/// The instance's game window: the largest visible, unowned, non-tool
/// window of the instance's processes. None while it has none.
pub fn pick(windows: &[Window], instance: &BTreeSet<u32>) -> Option<u64> {
    windows
        .iter()
        .filter(|w| w.visible && !w.owned && !w.tool && instance.contains(&w.pid))
        .filter(|w| w.rect.width >= 64 && w.rect.height >= 64)
        // The largest; among equals the oldest handle, so the choice is stable.
        .max_by_key(|w| (w.rect.area(), std::cmp::Reverse(w.id)))
        .map(|w| w.id)
}

/// Gap between parked windows and the desktop.
const GAP: i32 = 64;
/// Window messages carry positions as signed 16-bit numbers; a window past
/// this would read its own position as negative.
const LIMIT: i64 = i16::MAX as i64;

/// Where slot `slot`'s window waits while it streams at `size`: in a row
/// right of `desktop` (the virtual screen, every monitor), never on it.
pub fn parking(desktop: Rect, slot: usize, size: (u32, u32)) -> Result<Rect, String> {
    let width = i32::try_from(size.0).map_err(|_| "stream width out of range".to_string())?;
    let height = i32::try_from(size.1).map_err(|_| "stream height out of range".to_string())?;
    if width <= 0 || height <= 0 {
        return Err(format!("no window size for a {}x{} stream", size.0, size.1));
    }
    let x = desktop.right() + i64::from(GAP) + slot as i64 * i64::from(width + GAP);
    let y = i64::from(desktop.y);
    if x + i64::from(width) > LIMIT || y + i64::from(height) > LIMIT {
        return Err(format!(
            "Player {}'s {width}x{height} window does not fit beside the PC's desktop: lower the stream resolution or use fewer players",
            slot + 1
        ));
    }
    Ok(Rect {
        x: x as i32,
        y: y as i32,
        width,
        height,
    })
}

/// Whether a window must be moved back to its parking place: it moved or
/// was resized (games re-centre themselves), or it reached a monitor.
pub fn strayed(window: Rect, parked: Rect, monitors: &[Rect]) -> bool {
    window != parked || monitors.iter().any(|m| m.intersects(&window))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(id: u64, pid: u32, width: i32, height: i32) -> Window {
        Window {
            id,
            pid,
            rect: Rect {
                x: 0,
                y: 0,
                width,
                height,
            },
            visible: true,
            owned: false,
            tool: false,
        }
    }

    #[test]
    fn the_game_window_is_the_largest_real_window_of_the_instance() {
        let instance = BTreeSet::from([10, 11]);
        let mut splash = window(1, 10, 640, 360);
        let game = window(2, 11, 1920, 1080);
        let other_instance = window(3, 20, 3840, 2160);
        let mut dialog = window(4, 11, 2560, 1440);
        dialog.owned = true;
        let mut overlay = window(5, 10, 2560, 1440);
        overlay.tool = true;
        let mut hidden = window(6, 10, 2560, 1440);
        hidden.visible = false;
        let tiny = window(7, 10, 1, 1);
        let all = [
            splash.clone(),
            game,
            other_instance,
            dialog,
            overlay,
            hidden,
            tiny,
        ];
        assert_eq!(pick(&all, &instance), Some(2));
        assert_eq!(pick(&all, &BTreeSet::from([30])), None);
        // Equal sizes: the older handle stays chosen.
        splash.rect = all[1].rect;
        splash.id = 9;
        assert_eq!(pick(&[splash, all[1].clone()], &instance), Some(2));
    }

    const DESKTOP: Rect = Rect {
        x: -1920,
        y: 0,
        width: 4480,
        height: 1440,
    };

    #[test]
    fn windows_wait_in_a_row_beside_the_desktop() {
        let first = parking(DESKTOP, 0, (1920, 1080)).unwrap();
        assert_eq!(
            first,
            Rect {
                x: 2624,
                y: 0,
                width: 1920,
                height: 1080
            }
        );
        let second = parking(DESKTOP, 1, (1280, 720)).unwrap();
        assert_eq!(second.x, 2624 + 1280 + 64);
        assert!(!strayed(first, first, &[DESKTOP]));
    }

    #[test]
    fn a_window_that_would_not_fit_is_refused() {
        let error = parking(DESKTOP, 3, (7680, 4320)).unwrap_err();
        assert!(
            error.starts_with("Player 4's 7680x4320 window does not fit"),
            "{error}"
        );
        assert!(parking(DESKTOP, 0, (0, 1080)).is_err());
        assert!(parking(DESKTOP, 0, (u32::MAX, 1080)).is_err());
    }

    #[test]
    fn a_window_that_moved_or_reached_a_monitor_goes_back() {
        let parked = parking(DESKTOP, 0, (1920, 1080)).unwrap();
        let resized = Rect {
            width: 2560,
            ..parked
        };
        assert!(strayed(resized, parked, &[DESKTOP]));
        // A "parked" place on a monitor (a monitor added beside the desktop).
        let added = Rect {
            x: 2560,
            y: 0,
            width: 1920,
            height: 1080,
        };
        assert!(strayed(parked, parked, &[DESKTOP, added]));
    }
}
