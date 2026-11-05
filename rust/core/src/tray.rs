//! What the notification-area icon says about the running app, and the
//! notifications Vibepollo shows when an app starts, pauses, resumes and
//! stops.

use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

/// The icon's state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Icon {
    #[default]
    Idle,
    Streaming,
    Paused,
}
/// The running app and whether a client streams it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub app: Option<String>,
    pub streaming: bool,
}
/// A notification: title and text.
pub type Notice = (&'static str, String);
/// Follows the running app. An app counts as streaming from its launch
/// until its stream ends, so the client connecting after the launch is no
/// pause and no resume.
#[derive(Clone, Debug, Default)]
pub struct Tracker {
    status: Status,
    paused: bool,
}
impl Tracker {
    pub fn icon(&self) -> Icon {
        match (&self.status.app, self.paused) {
            (None, _) => Icon::Idle,
            (Some(_), false) => Icon::Streaming,
            (Some(_), true) => Icon::Paused,
        }
    }
    pub fn app(&self) -> Option<&str> {
        self.status.app.as_deref()
    }
    /// The tooltip line, while an app runs.
    pub fn summary(&self) -> Option<String> {
        let app = self.status.app.as_ref()?;
        Some(if self.paused {
            format!("{app} is paused")
        } else {
            format!("Streaming {app}")
        })
    }
    /// Take the current status. Returns None when nothing changed, else
    /// the notification for the change, if it has one.
    pub fn update(&mut self, current: Status) -> Option<Option<Notice>> {
        if current == self.status {
            return None;
        }
        let previous = std::mem::replace(&mut self.status, current);
        let current = &self.status;
        let notice = match (&previous.app, &current.app) {
            (_, Some(app)) if previous.app.as_ref() != Some(app) => {
                self.paused = false;
                Some(("App launched", format!("Streaming started for {app}")))
            }
            (Some(app), None) => {
                self.paused = false;
                Some(("App stopped", format!("Streaming stopped for {app}")))
            }
            (Some(_), Some(app)) if previous.streaming && !current.streaming => {
                self.paused = true;
                Some((
                    "Stream paused",
                    format!("{app} keeps running; reconnect from Moonlight to resume."),
                ))
            }
            (Some(_), Some(app)) if self.paused && current.streaming => {
                self.paused = false;
                Some(("Stream resumed", format!("Streaming {app} again")))
            }
            _ => None,
        };
        Some(notice)
    }
}

/// Draw the state's dot, ringed in white, in the bottom-right corner of a
/// `width`×`height` icon image: BGRA rows with straight alpha. False,
/// leaving the image as is, for Idle or an image without alpha.
pub fn badge(pixels: &mut [u8], width: usize, height: usize, icon: Icon) -> bool {
    let color: [u8; 3] = match icon {
        Icon::Idle => return false,
        Icon::Streaming => [0x43, 0xa0, 0x2e],
        Icon::Paused => [0x22, 0x99, 0xd2],
    };
    if width == 0
        || pixels.len() < width * height * 4
        || !pixels.as_chunks::<4>().0.iter().any(|p| p[3] != 0)
    {
        return false;
    }
    let size = width.min(height) as f32;
    let radius = size / 4.0;
    let ring = (size / 16.0).max(1.0);
    let (cx, cy) = (width as f32 - radius, height as f32 - radius);
    for y in 0..height {
        for x in 0..width {
            let distance = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
            let coverage = (radius - distance + 0.5).clamp(0.0, 1.0);
            if coverage == 0.0 {
                continue;
            }
            let fill = if distance > radius - ring {
                [0xff; 3]
            } else {
                color
            };
            let pixel = &mut pixels[(y * width + x) * 4..][..4];
            let below = f32::from(pixel[3]) / 255.0 * (1.0 - coverage);
            let alpha = coverage + below;
            for c in 0..3 {
                pixel[c] = ((f32::from(fill[c]) * coverage + f32::from(pixel[c]) * below) / alpha)
                    .round() as u8;
            }
            pixel[3] = (alpha * 255.0).round() as u8;
        }
    }
    true
}

/// What clicking the tray's notification does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeAction {
    EnterPin,
    QuitApp,
}
/// How the tray puts up a notice it just posted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Post {
    Show,
    /// Take down what is up first: a click on it would do this notice's action.
    ClearThenShow,
    /// Not now, see [`Notices`].
    Held,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedNotice {
    pub title: String,
    pub text: String,
    pub action: Option<NoticeAction>,
    /// The pairing request it asks about, which may end while it waits.
    pub request: Option<String>,
}
/// More than this many notices waiting means something is wrong: the oldest plain one goes.
const MOST_HELD: usize = 16;
/// How long a launch error keeps others waiting if Windows never says it went,
/// as it may not under Do Not Disturb. Well below a pairing request's 300 s.
const QUIT_HOLDS_FOR: Duration = Duration::from_secs(30);
/// A click this soon after a launch error's post was on the notification before it,
/// already on its way when the launch error came: nobody reads and clicks this fast.
const QUIT_CLICKABLE_AFTER: Duration = Duration::from_millis(500);
/// A timeout this soon after a post is for the notification that post replaced:
/// none of ours is up for less, and the message can't say which one it was.
const REPLACED_WITHIN: Duration = Duration::from_secs(2);
/// The tray has one notification slot and a click doesn't say which notification
/// it was on, so a click can only be matched to the last one posted. A notice that
/// quits the app therefore holds the slot: while it can be clicked, others wait,
/// and it never goes up over another one, whose click would then quit the app. A
/// request it takes down waits with them. Only a fresh click on a recent launch error
/// quits: an older notification left in the notification center can still be clicked,
/// and the tray's menu quits at any time.
#[derive(Debug, Default)]
pub struct Notices {
    action: Option<NoticeAction>,
    /// The requests put up, oldest first, until one is clicked or goes away.
    up: Vec<QueuedNotice>,
    held: VecDeque<QueuedNotice>,
    posted: Option<Instant>,
}
impl Notices {
    pub const fn new() -> Self {
        Self {
            action: None,
            up: Vec::new(),
            held: VecDeque::new(),
            posted: None,
        }
    }
    pub fn post(&mut self, notice: &QueuedNotice, now: Instant) -> Post {
        if notice.action == Some(NoticeAction::QuitApp) {
            for up in std::mem::take(&mut self.up) {
                self.hold(up);
            }
            self.action = notice.action;
            self.posted = Some(now);
            return Post::ClearThenShow;
        }
        if self.action == Some(NoticeAction::QuitApp) {
            self.hold(notice.clone());
            return Post::Held;
        }
        self.action = notice.action;
        if notice.action.is_some() {
            self.up.retain(|up| up.request != notice.request);
            self.up.push(notice.clone());
        }
        self.posted = Some(now);
        Post::Show
    }
    fn hold(&mut self, notice: QueuedNotice) {
        if notice.request.is_some() {
            self.held.retain(|held| held.request != notice.request);
        }
        if self.held.len() >= MOST_HELD {
            let oldest_plain = self.held.iter().position(|held| held.action.is_none());
            // A plain notice never costs a request its place.
            if oldest_plain.is_none() && notice.action.is_none() {
                tracing::warn!(dropped = ?notice, "too many tray notifications waiting");
                return;
            }
            let dropped = self.held.remove(oldest_plain.unwrap_or(0));
            tracing::warn!(?dropped, "too many tray notifications waiting");
        }
        self.held.push_back(notice);
    }
    /// What goes up now, requests last so that a click on what stays up opens the PIN.
    fn release(&mut self) -> Vec<QueuedNotice> {
        let mut released: Vec<_> = self.held.drain(..).collect();
        released.sort_by_key(|notice| notice.action.is_some());
        released
    }
    fn quit_older_than(&self, age: Duration, now: Instant) -> bool {
        self.action == Some(NoticeAction::QuitApp)
            && self.posted.is_some_and(|posted| now - posted >= age)
    }
    /// The notification was clicked: what to do, and what may go up now. A click
    /// too soon after a launch error leaves it and what waits for it as they are.
    pub fn clicked(&mut self, now: Instant) -> (Option<NoticeAction>, Vec<QueuedNotice>) {
        if self.action == Some(NoticeAction::QuitApp)
            && !self.quit_older_than(QUIT_CLICKABLE_AFTER, now)
        {
            tracing::info!("a click this soon after a launch error was on an older notification");
            return (None, Vec::new());
        }
        if self.quit_older_than(QUIT_HOLDS_FOR, now) {
            self.action = None;
        }
        self.up.clear();
        (self.action.take(), self.release())
    }
    /// It went away unclicked: what may go up now.
    pub fn timed_out(&mut self, now: Instant) -> Vec<QueuedNotice> {
        if self
            .posted
            .is_some_and(|posted| now - posted < REPLACED_WITHIN)
        {
            return Vec::new();
        }
        self.action = None;
        self.up.clear();
        self.release()
    }
    /// Another app runs or none does, so a click can't mean the one that quit.
    /// Only that goes: a device still waits for its PIN.
    pub fn app_changed(&mut self) -> Vec<QueuedNotice> {
        if self.action != Some(NoticeAction::QuitApp) {
            return Vec::new();
        }
        self.action = None;
        self.release()
    }
    /// Windows refused the notification, so no click will come for it.
    pub fn not_shown(&mut self) -> Vec<QueuedNotice> {
        self.action = None;
        self.up.clear();
        self.release()
    }
    /// A pairing request ended: nothing may ask about it later.
    pub fn request_ended(&mut self, request: &str) {
        let ended = |notice: &QueuedNotice| notice.request.as_deref() == Some(request);
        self.held.retain(|held| !ended(held));
        self.up.retain(|up| !ended(up));
    }
    /// A launch error that nobody answered gives way to what waits for it. With
    /// nothing waiting its click still stops quitting: under Do Not Disturb no timeout
    /// says it went, and a click on anything older would quit the app.
    pub fn tick(&mut self, now: Instant) -> Vec<QueuedNotice> {
        if !self.quit_older_than(QUIT_HOLDS_FOR, now) {
            return Vec::new();
        }
        self.action = None;
        self.release()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use NoticeAction::{EnterPin, QuitApp};

    fn plain(text: &str) -> QueuedNotice {
        QueuedNotice {
            title: "Notice".into(),
            text: text.into(),
            action: None,
            request: None,
        }
    }
    fn request(id: &str) -> QueuedNotice {
        QueuedNotice {
            title: "Pair".into(),
            text: id.into(),
            action: Some(EnterPin),
            request: Some(id.into()),
        }
    }
    fn launch_error() -> QueuedNotice {
        QueuedNotice {
            title: "Launch Error".into(),
            text: "Game".into(),
            action: Some(QuitApp),
            request: None,
        }
    }
    fn texts(notices: &[QueuedNotice]) -> Vec<&str> {
        notices.iter().map(|notice| notice.text.as_str()).collect()
    }

    #[test]
    fn a_click_on_a_pairing_notice_never_quits_the_app() {
        let now = Instant::now();
        let mut notices = Notices::new();
        // A launch error while the pairing toast is up takes it down first,
        // and the request comes back after it.
        assert_eq!(notices.post(&request("TV"), now), Post::Show);
        assert_eq!(notices.post(&launch_error(), now), Post::ClearThenShow);
        // One during the launch error waits as well, and keeps its action for later.
        assert_eq!(notices.post(&request("Tablet"), now), Post::Held);
        let read = now + Duration::from_secs(1);
        let (action, held) = notices.clicked(read);
        assert_eq!(action, Some(QuitApp));
        assert_eq!(texts(&held), ["TV", "Tablet"]);
        for notice in &held {
            assert_eq!(notices.post(notice, read), Post::Show);
        }
        // Its click opens the PIN and there is nothing left to quit.
        assert_eq!(notices.clicked(read), (Some(EnterPin), Vec::new()));
        assert_eq!(notices.clicked(read), (None, Vec::new()));
    }

    #[test]
    fn a_click_right_after_a_launch_error_was_on_the_toast_before_it() {
        let now = Instant::now();
        let mut notices = Notices::new();
        notices.post(&request("TV"), now);
        notices.post(&launch_error(), now);
        notices.post(&plain("Approved"), now);
        // Already on its way when the launch error came: nothing quits, nothing moves.
        let soon = now + QUIT_CLICKABLE_AFTER - Duration::from_millis(1);
        assert_eq!(notices.clicked(soon), (None, Vec::new()));
        // The launch error itself is still there to answer, with what waits for it.
        let (action, held) = notices.clicked(now + QUIT_CLICKABLE_AFTER);
        assert_eq!(action, Some(QuitApp));
        assert_eq!(texts(&held), ["Approved", "TV"]);
    }

    #[test]
    fn two_requests_both_come_back_after_a_launch_error() {
        let now = Instant::now();
        let later = now + Duration::from_secs(1);
        let mut notices = Notices::new();
        notices.post(&request("TV"), now);
        notices.post(&request("Tablet"), now);
        notices.post(&request("TV"), now);
        notices.post(&request("Phone"), now);
        notices.request_ended("Phone");
        notices.post(&launch_error(), now);
        assert_eq!(texts(&notices.clicked(later).1), ["Tablet", "TV"]);

        // Clicked or gone, none of them asks again.
        notices.post(&request("TV"), now);
        notices.post(&request("Tablet"), now);
        assert_eq!(notices.clicked(later), (Some(EnterPin), Vec::new()));
        notices.post(&launch_error(), later);
        assert_eq!(
            notices.timed_out(later + Duration::from_secs(10)),
            Vec::new()
        );
    }

    #[test]
    fn nothing_is_lost_while_a_launch_error_waits_and_requests_go_up_last() {
        let start = Instant::now();
        let later = start + Duration::from_secs(10);
        let mut notices = Notices::new();
        notices.post(&launch_error(), start);
        assert_eq!(notices.post(&plain("Approved"), start), Post::Held);
        assert_eq!(notices.post(&request("TV"), start), Post::Held);
        assert_eq!(notices.post(&plain("Three wrong PINs"), start), Post::Held);
        assert_eq!(
            texts(&notices.timed_out(later)),
            ["Approved", "Three wrong PINs", "TV"]
        );
        assert_eq!(notices.clicked(later), (None, Vec::new()));

        notices.post(&launch_error(), later);
        notices.post(&request("TV"), later);
        notices.post(&request("TV"), later);
        assert_eq!(texts(&notices.app_changed()), ["TV"]);
        assert_eq!(notices.clicked(later), (None, Vec::new()));
    }

    #[test]
    fn a_full_queue_gives_up_a_plain_notice_before_a_request() {
        let now = Instant::now();
        let later = now + Duration::from_secs(1);
        let mut notices = Notices::new();
        notices.post(&launch_error(), now);
        notices.post(&request("TV"), now);
        notices.post(&plain("old"), now);
        for n in 0..MOST_HELD - 1 {
            notices.post(&request(&format!("{n}")), now);
        }
        assert_eq!(notices.held.len(), MOST_HELD);
        let (_, held) = notices.clicked(later);
        assert!(!texts(&held).contains(&"old"));
        assert!(texts(&held).contains(&"TV"));
    }

    #[test]
    fn a_plain_notice_never_takes_a_full_queues_place_from_a_request() {
        let now = Instant::now();
        let later = now + Duration::from_secs(1);
        let mut notices = Notices::new();
        notices.post(&launch_error(), now);
        for n in 0..MOST_HELD {
            notices.post(&request(&format!("{n}")), now);
        }
        assert_eq!(notices.post(&plain("late"), now), Post::Held);
        assert_eq!(notices.held.len(), MOST_HELD);
        let (_, held) = notices.clicked(later);
        assert!(!texts(&held).contains(&"late"));
        assert!(texts(&held).contains(&"0"), "the oldest request stays");
    }

    #[test]
    fn a_request_that_ended_does_not_ask_again() {
        let now = Instant::now();
        let mut notices = Notices::new();
        notices.post(&request("TV"), now);
        notices.request_ended("TV");
        notices.post(&launch_error(), now);
        notices.post(&request("Tablet"), now);
        notices.post(&request("Phone"), now);
        notices.request_ended("Tablet");
        assert_eq!(
            texts(&notices.clicked(now + Duration::from_secs(1)).1),
            ["Phone"]
        );
    }

    #[test]
    fn a_request_that_timed_out_was_seen_and_does_not_come_back() {
        let start = Instant::now();
        let mut notices = Notices::new();
        notices.post(&request("TV"), start);
        notices.timed_out(start + Duration::from_secs(10));
        notices.post(&launch_error(), start + Duration::from_secs(20));
        assert_eq!(
            notices.clicked(start + Duration::from_secs(21)),
            (Some(QuitApp), Vec::new())
        );
    }

    #[test]
    fn a_timeout_for_the_notification_a_launch_error_replaced_leaves_it_alone() {
        let start = Instant::now();
        let mut notices = Notices::new();
        notices.post(&request("TV"), start);
        notices.post(&launch_error(), start + Duration::from_secs(5));
        assert_eq!(
            notices.timed_out(start + Duration::from_secs(6)),
            Vec::new()
        );
        // The launch error's own timeout is a real one.
        let held = notices.timed_out(start + Duration::from_secs(15));
        assert_eq!(texts(&held), ["TV"]);
        assert_eq!(
            notices.clicked(start + Duration::from_secs(16)),
            (None, Vec::new())
        );
    }

    #[test]
    fn a_changed_app_takes_only_the_quit_away() {
        let now = Instant::now();
        let later = now + Duration::from_secs(1);
        let mut notices = Notices::new();
        notices.post(&request("TV"), now);
        assert_eq!(notices.app_changed(), Vec::new());
        assert_eq!(notices.clicked(later), (Some(EnterPin), Vec::new()));

        notices.post(&launch_error(), now);
        notices.app_changed();
        assert_eq!(notices.clicked(later), (None, Vec::new()));
    }

    #[test]
    fn a_launch_error_windows_refused_lets_what_waits_for_it_go_up() {
        let now = Instant::now();
        let mut notices = Notices::new();
        notices.post(&launch_error(), now);
        notices.post(&request("TV"), now);
        // A second launch error, refused: the request isn't wiped by the next post.
        notices.post(&launch_error(), now);
        assert_eq!(texts(&notices.not_shown()), ["TV"]);
        assert_eq!(
            notices.clicked(now + Duration::from_secs(1)),
            (None, Vec::new())
        );
        assert_eq!(notices.post(&plain("Ready"), now), Post::Show);
    }

    #[test]
    fn a_launch_error_nobody_answers_gives_way_but_never_to_a_click_that_quits() {
        let start = Instant::now();
        let mut notices = Notices::new();
        notices.post(&launch_error(), start);
        notices.post(&request("TV"), start);
        assert_eq!(
            notices.tick(start + QUIT_HOLDS_FOR - Duration::from_secs(1)),
            Vec::new()
        );
        let held = notices.tick(start + QUIT_HOLDS_FOR);
        assert_eq!(texts(&held), ["TV"]);
        assert_eq!(notices.post(&held[0], start + QUIT_HOLDS_FOR), Post::Show);
        assert_eq!(
            notices.clicked(start + QUIT_HOLDS_FOR),
            (Some(EnterPin), Vec::new())
        );

        // With nothing waiting no timeout may ever come, as under Do Not Disturb: the
        // click stops quitting all the same, for the rest of a detached stream.
        notices.post(&launch_error(), start);
        assert_eq!(notices.tick(start + QUIT_HOLDS_FOR), Vec::new());
        assert_eq!(notices.action, None);
        assert_eq!(
            notices.clicked(start + QUIT_HOLDS_FOR * 100),
            (None, Vec::new())
        );

        // A host loop that hasn't ticked yet changes nothing.
        notices.post(&launch_error(), start);
        assert_eq!(notices.clicked(start + QUIT_HOLDS_FOR), (None, Vec::new()));
    }

    fn status(app: Option<&str>, streaming: bool) -> Status {
        Status {
            app: app.map(str::to_owned),
            streaming,
        }
    }
    fn title(change: Option<Option<Notice>>) -> Option<&'static str> {
        change.expect("a change").map(|(title, _)| title)
    }
    #[test]
    fn launch_connect_pause_resume_and_stop() {
        let mut tray = Tracker::default();
        assert_eq!(tray.icon(), Icon::Idle);
        assert_eq!(tray.summary(), None);
        assert_eq!(
            tray.update(status(Some("Desktop"), false)),
            Some(Some((
                "App launched",
                "Streaming started for Desktop".to_owned()
            )))
        );
        assert_eq!(tray.icon(), Icon::Streaming);
        // The client connects after the launch: no notification.
        assert_eq!(tray.update(status(Some("Desktop"), true)), Some(None));
        assert_eq!(tray.update(status(Some("Desktop"), true)), None);
        assert_eq!(
            title(tray.update(status(Some("Desktop"), false))),
            Some("Stream paused")
        );
        assert_eq!(tray.icon(), Icon::Paused);
        assert_eq!(tray.summary().as_deref(), Some("Desktop is paused"));
        assert_eq!(
            title(tray.update(status(Some("Desktop"), true))),
            Some("Stream resumed")
        );
        assert_eq!(tray.icon(), Icon::Streaming);
        assert_eq!(tray.app(), Some("Desktop"));
        assert_eq!(
            tray.update(status(None, false)),
            Some(Some((
                "App stopped",
                "Streaming stopped for Desktop".to_owned()
            )))
        );
        assert_eq!(tray.icon(), Icon::Idle);
        assert_eq!(tray.app(), None);
    }
    #[test]
    fn switching_apps_announces_the_new_one_and_clears_the_pause() {
        let mut tray = Tracker::default();
        tray.update(status(Some("Desktop"), true));
        tray.update(status(Some("Desktop"), false));
        assert_eq!(tray.icon(), Icon::Paused);
        assert_eq!(
            title(tray.update(status(Some("Steam Big Picture"), false))),
            Some("App launched")
        );
        assert_eq!(tray.icon(), Icon::Streaming);
        assert_eq!(
            tray.summary().as_deref(),
            Some("Streaming Steam Big Picture")
        );
    }
    #[test]
    fn the_badge_marks_the_corner_of_icons_with_alpha() {
        let (width, height) = (16, 16);
        let red = [0x10, 0x4b, 0xad, 0xff];
        let mut pixels = red.repeat(width * height);
        assert!(!badge(&mut pixels, width, height, Icon::Idle));
        assert!(badge(&mut pixels, width, height, Icon::Streaming));
        let at = |pixels: &[u8], x: usize, y: usize| pixels[(y * width + x) * 4..][..4].to_vec();
        assert_eq!(at(&pixels, 0, 0), red);
        assert_eq!(at(&pixels, 12, 12), [0x43, 0xa0, 0x2e, 0xff]);
        // The ring's edge, nearly all white.
        assert!(at(&pixels, 12, 8).iter().all(|c| *c >= 0xf0));
        let mut paused = red.repeat(width * height);
        badge(&mut paused, width, height, Icon::Paused);
        assert_eq!(at(&paused, 12, 12), [0x22, 0x99, 0xd2, 0xff]);
        // A transparent corner gets the dot alone, opaque.
        let mut clear = [0u8; 4].repeat(width * height);
        clear[3] = 0xff;
        assert!(badge(&mut clear, width, height, Icon::Paused));
        assert_eq!(at(&clear, 12, 12), [0x22, 0x99, 0xd2, 0xff]);
        let mut flat = [0u8; 4].repeat(width * height);
        assert!(!badge(&mut flat, width, height, Icon::Streaming));
        assert!(flat.iter().all(|b| *b == 0));
    }
}
