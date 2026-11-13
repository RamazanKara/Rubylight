//! Streaming the launched app's window instead of a display
//! (`capture_window = window`): which window that is, and what the stream
//! shows while it has none.
//!
//! The window is found among the windows of the app's process tree (the
//! processes the launch started and everything they started), optionally
//! narrowed by the program's file name (`capture_window_exe`) or a part of
//! the window's title (`capture_window_title`). A named program is also
//! looked for outside the tree, for a launcher that hands the game to a
//! process it did not start. Windows Graphics Capture captures it with the
//! GPU and colour space of the display it is on.
//!
//! What the stream shows follows [`Follow`]: the window while it is shown;
//! the stream's display, with a warning on the device, while the window is
//! minimised or before it appears; and once it closed, after a short wait
//! for a replacement (a splash screen giving way to the game), the display
//! again or the end of the stream (`capture_window_closed`).
//!
//! The picture keeps the window's shape: the GPU conversion scales it to the
//! stream and letterboxes it ([`crate::display_policy::letterbox`]).
//! Absolute mouse, touch and pen input map onto the window: a point on the
//! stream, black bars excluded, is the same point of the window's captured
//! area (its visible frame on the desktop), wherever the window is: the
//! input injector maps onto the window's rectangle as it does onto a
//! display's.
use crate::config::Config;
use std::collections::BTreeSet;
use std::time::{Duration, Instant};

/// `display` (the default) or `window`.
pub const SETTING: &str = "capture_window";
/// A program file name, such as `game.exe`; empty for any of the app's.
pub const EXE: &str = "capture_window_exe";
/// A part of the window's title, in any case; empty for any.
pub const TITLE: &str = "capture_window_title";
/// `display` (the default) or `end`: what follows the window's closing.
pub const CLOSED: &str = "capture_window_closed";
/// Every key of the feature, for a stream that never captures a window of
/// its own (a Nucleus Co-op player's).
pub const KEYS: [&str; 4] = [SETTING, EXE, TITLE, CLOSED];

/// How long a closed window's stream waits for the app's next window before
/// it shows the display for good or ends.
pub const REPLACEMENT_WAIT: Duration = Duration::from_secs(5);
/// The smallest window taken for the app's: smaller ones are tooltips,
/// splash corners and hidden message windows.
pub const MIN_SIZE: i32 = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Closed {
    /// Show the stream's display, with a warning on the device.
    Display,
    /// End the stream with the reason.
    End,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub exe: Option<String>,
    pub title: Option<String>,
    pub closed: Closed,
}

/// Whether `value` asks for the window. Unknown values are the display, so a
/// typo never hides the desktop.
pub fn wants_window(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "window" | "app" | "true" | "on" | "yes" | "1" | "enabled"
    )
}

fn nonempty(value: &str) -> Option<String> {
    let value = value.trim().trim_matches('"').trim();
    (!value.is_empty()).then(|| value.to_owned())
}

/// The stream's window settings, or None when it streams a display.
pub fn settings(config: &Config) -> Option<Settings> {
    if !wants_window(config.get(SETTING, "display")) {
        return None;
    }
    Some(Settings {
        exe: nonempty(config.get(EXE, "")),
        title: nonempty(config.get(TITLE, "")),
        closed: match config
            .get(CLOSED, "display")
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "end" | "stop" | "quit" => Closed::End,
            _ => Closed::Display,
        },
    })
}

/// A top-level window as the OS reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub id: u64,
    pub pid: u32,
    /// The program's file name, such as `game.exe`; empty when unknown.
    pub exe: String,
    pub title: String,
    pub width: i32,
    pub height: i32,
    /// Shown and not minimised (nor cloaked: on another virtual desktop, or
    /// a suspended app's).
    pub visible: bool,
    /// Shown but minimised: nothing to capture until it is restored.
    pub minimized: bool,
    /// A dialog or popup that another window owns.
    pub owned: bool,
    /// A tool window: overlays and launchers' helpers.
    pub tool: bool,
}
impl Candidate {
    fn area(&self) -> i64 {
        i64::from(self.width.max(0)) * i64::from(self.height.max(0))
    }
}

/// `exe` names `wanted`: the same file name in any case, with or without
/// `.exe`, and with or without a folder on either side.
pub fn exe_matches(exe: &str, wanted: &str) -> bool {
    let name = |path: &str| {
        let file = path.rsplit(['\\', '/']).next().unwrap_or(path).trim();
        let file = file.to_ascii_lowercase();
        file.strip_suffix(".exe").map(str::to_owned).unwrap_or(file)
    };
    let (exe, wanted) = (name(exe), name(wanted));
    !wanted.is_empty() && exe == wanted
}

/// `title` contains `wanted`, in any case.
pub fn title_matches(title: &str, wanted: &str) -> bool {
    title.to_lowercase().contains(&wanted.to_lowercase())
}

/// The app's window among `windows`: an unowned, non-tool window that is
/// shown or minimised, of the app's processes `tree` (or, when a program is
/// named, of any process of that program), matching the named program and
/// title. `current`, the window streamed now, stays while it qualifies, so a
/// second window never takes the stream over; otherwise a shown window wins
/// over a minimised one, then the largest, then the oldest handle.
pub fn pick<'a>(
    windows: &'a [Candidate],
    tree: &BTreeSet<u32>,
    settings: &Settings,
    current: Option<u64>,
) -> Option<&'a Candidate> {
    let qualifies = |w: &&Candidate| {
        !w.owned
            && !w.tool
            && (w.minimized || (w.visible && w.width >= MIN_SIZE && w.height >= MIN_SIZE))
            && match &settings.exe {
                Some(exe) => exe_matches(&w.exe, exe),
                None => tree.contains(&w.pid),
            }
            && settings
                .title
                .as_ref()
                .is_none_or(|title| title_matches(&w.title, title))
    };
    let mut found: Vec<&Candidate> = windows.iter().filter(qualifies).collect();
    // A named program's windows in the app's tree first: another copy of it
    // the user started at the PC is not the launched one.
    if settings.exe.is_some() && found.iter().any(|w| tree.contains(&w.pid)) {
        found.retain(|w| tree.contains(&w.pid));
    }
    if let Some(current) = current
        && let Some(kept) = found.iter().find(|w| w.id == current)
    {
        return Some(kept);
    }
    found
        .into_iter()
        .max_by_key(|w| (w.visible, w.area(), std::cmp::Reverse(w.id)))
}

/// What the stream captures.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// The window, owned by the process.
    Window { id: u64, pid: u32 },
    /// The stream's display.
    Display,
}

/// One step of [`Follow`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decision {
    pub target: Target,
    /// What the device is told; None clears the warning.
    pub warning: Option<&'static str>,
    /// The stream ends with this reason.
    pub end: Option<&'static str>,
}

pub const WAITING: &str = "Waiting for the app's window; the stream shows the display until it appears. If it does not, check the window settings of this app (program name or title).";
pub const MINIMIZED: &str =
    "The app's window is minimised; the stream shows the display until it is restored on the PC.";
pub const REPLACING: &str =
    "The app's window closed; waiting for its next window, the stream shows the display meanwhile.";
pub const CLOSED_DISPLAY: &str = "The app's window closed; the stream shows the display. Start the app again to stream its window.";
pub const CLOSED_END: &str = "The streamed app window closed";

/// What the stream shows, from the app's window found on each check.
#[derive(Debug, Default)]
pub struct Follow {
    /// The window streamed last, kept by [`pick`] while it qualifies.
    current: Option<u64>,
    /// Whether the app ever had a window: before it, the stream waits.
    seen: bool,
    /// Since when the app has had no window, after it had one.
    lost_since: Option<Instant>,
}
impl Follow {
    /// The window [`pick`] keeps.
    pub fn current(&self) -> Option<u64> {
        self.current
    }
    pub fn step(&mut self, found: Option<&Candidate>, closed: Closed, now: Instant) -> Decision {
        let display = |warning| Decision {
            target: Target::Display,
            warning: Some(warning),
            end: None,
        };
        match found {
            Some(window) => {
                self.current = Some(window.id);
                self.seen = true;
                self.lost_since = None;
                if window.visible {
                    Decision {
                        target: Target::Window {
                            id: window.id,
                            pid: window.pid,
                        },
                        warning: None,
                        end: None,
                    }
                } else {
                    display(MINIMIZED)
                }
            }
            None if !self.seen => display(WAITING),
            None => {
                self.current = None;
                let since = *self.lost_since.get_or_insert(now);
                if now.duration_since(since) < REPLACEMENT_WAIT {
                    display(REPLACING)
                } else if closed == Closed::End {
                    Decision {
                        target: Target::Display,
                        warning: None,
                        end: Some(CLOSED_END),
                    }
                } else {
                    display(CLOSED_DISPLAY)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
