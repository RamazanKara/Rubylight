//! The app's window while it is in front, the display while it is not
//! (`capture_window = window_focus`, issue #159): what follows the PC's focus
//! on top of [`super::Follow`].
//!
//! Each look tells [`Focus`] which of the app's windows is the foreground
//! window ([`in_front`]), or that another program, the desktop or nothing is.
//! A change of what is in front counts once it has lasted
//! [`BACKGROUND_DELAY`], so a launcher or store client that takes the focus
//! for a moment at launch (#131) never switches the stream; the window the
//! stream shows, or the one last in front, coming back to the front counts at
//! once. While another program has the focus, the stream shows the stream's
//! display, as it does for a minimised window, without a warning: it is what
//! this value asks for. A window that was never in front, such as a game that
//! opened behind its store client, is streamed as `window` streams it until
//! it first comes to the front, which launch focus brings about.
//!
//! The app's other windows follow the focus too: one of them that stays in
//! front for [`BACKGROUND_DELAY`] takes the stream over, where `window` keeps
//! the window it streams while that qualifies. Only the app's own windows are
//! ever streamed; anything else in front shows the display.
use super::{Background, Candidate, Decision, Follow, MINIMIZED, Settings, Target, pick};
use std::collections::BTreeSet;
use std::time::{Duration, Instant};

/// How long another window must stay in front before the stream shows the
/// display (or another of the app's windows) in place of the window: longer
/// than a launcher's focus steal at launch, short enough to follow Alt+Tab.
/// The worker looks every 250 ms, so the switch comes 1 to 1.25 s after it.
pub const BACKGROUND_DELAY: Duration = Duration::from_secs(1);

/// The app's window that is the foreground window `foreground`, as [`pick`]
/// would stream it (unowned, not a tool window, shown, of the app); None when
/// the foreground window is another program's, a dialog, minimised or none.
pub fn in_front(
    windows: &[Candidate],
    tree: &BTreeSet<u32>,
    settings: &Settings,
    foreground: Option<u64>,
) -> Option<u64> {
    let foreground = foreground?;
    pick(windows, tree, settings, Some(foreground))
        .filter(|window| window.id == foreground && window.visible)
        .map(|window| window.id)
}

/// What is in front, debounced, for `capture_window = window_focus`.
#[derive(Debug, Default)]
pub struct Focus {
    /// What is in front as last counted: None before the first look,
    /// `Some(None)` while no window of the app is.
    settled: Option<Option<u64>>,
    /// A change of what is in front, and since when, until it counts.
    pending: Option<(Option<u64>, Instant)>,
    /// The app's window last counted in front: only it goes to the
    /// background, and it comes back at once.
    fronted: Option<u64>,
}

impl Focus {
    /// One look at `now`: `front` is the app's window in front
    /// ([`in_front`]), `shown` the window the stream shows now.
    pub fn look(&mut self, front: Option<u64>, shown: Option<u64>, now: Instant) {
        if self.settled == Some(front) {
            self.pending = None;
            return;
        }
        let back = front.is_some() && (front == shown || front == self.fronted);
        let since = match self.pending {
            Some((pending, since)) if pending == front => since,
            _ => now,
        };
        if back || now.saturating_duration_since(since) >= BACKGROUND_DELAY {
            self.settled = Some(front);
            self.pending = None;
            if front.is_some() {
                self.fronted = front;
            }
        } else {
            self.pending = Some((front, since));
        }
    }

    /// The window [`pick`] keeps: the app's window counted in front, else
    /// `current`, the one streamed now.
    pub fn keep(&self, current: Option<u64>) -> Option<u64> {
        self.settled.flatten().or(current)
    }

    /// Whether `decision` streams a window that is in the background: the one
    /// last in front, while no window of the app has been for
    /// [`BACKGROUND_DELAY`].
    pub fn behind(&self, decision: &Decision) -> bool {
        matches!(decision.target, Target::Window { id, .. }
            if self.settled == Some(None) && self.fronted == Some(id))
    }

    /// [`super::Follow::step`]'s decision as this value shows it: the
    /// display in place of a window in the background, and no warning for a
    /// minimised window, which is in the background too.
    pub fn apply(&self, decision: Decision) -> Decision {
        if self.behind(&decision) {
            return Decision {
                target: Target::Display,
                warning: None,
                end: None,
            };
        }
        if decision.target == Target::Display && decision.warning == Some(MINIMIZED) {
            return Decision {
                warning: None,
                ..decision
            };
        }
        decision
    }
}

/// One look of [`Tracker`]: the app's window found, and what the stream shows.
#[derive(Debug)]
pub struct Look<'a> {
    pub picked: Option<&'a Candidate>,
    pub decision: Decision,
    /// The stream shows the display because the window is in the background.
    pub behind: bool,
}

/// What a stream of the app's window shows from look to look: [`Follow`]
/// alone for `window`, with [`Focus`] for `window_focus`.
#[derive(Debug)]
pub struct Tracker {
    follow: Follow,
    focus: Option<Focus>,
    /// The window the stream shows after the last look.
    shown: Option<u64>,
}

impl Tracker {
    pub fn new(settings: &Settings) -> Self {
        Self {
            follow: Follow::default(),
            focus: (settings.background == Background::Display).then(Focus::default),
            shown: None,
        }
    }

    /// Whether the stream follows the focus, so a look needs the foreground
    /// window.
    pub fn follows_focus(&self) -> bool {
        self.focus.is_some()
    }

    /// One look at `now`, at `windows` with `foreground` the foreground
    /// window (read only when [`Self::follows_focus`]) and `tree` the app's
    /// processes.
    pub fn look<'a>(
        &mut self,
        windows: &'a [Candidate],
        tree: &BTreeSet<u32>,
        settings: &Settings,
        foreground: Option<u64>,
        now: Instant,
    ) -> Look<'a> {
        let current = self.follow.current();
        let keep = match &mut self.focus {
            Some(focus) => {
                focus.look(
                    in_front(windows, tree, settings, foreground),
                    self.shown,
                    now,
                );
                focus.keep(current)
            }
            None => current,
        };
        let picked = pick(windows, tree, settings, keep);
        let decision = self.follow.step(picked, settings.closed, now);
        let (decision, behind) = match &self.focus {
            Some(focus) => (focus.apply(decision.clone()), focus.behind(&decision)),
            None => (decision, false),
        };
        self.shown = match decision.target {
            Target::Window { id, .. } => Some(id),
            Target::Display => None,
        };
        Look {
            picked,
            decision,
            behind,
        }
    }
}

#[cfg(test)]
mod tests;
