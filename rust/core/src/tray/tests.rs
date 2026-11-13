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
fn the_requests_up_stay_as_few_as_the_queue_holds() {
    let now = Instant::now();
    let mut notices = Notices::new();
    for n in 0..MOST_HELD * 2 {
        notices.post(&request(&format!("{n}")), now);
    }
    assert_eq!(notices.up.len(), MOST_HELD);
    assert_eq!(
        notices.up.last(),
        Some(&request(&format!("{}", MOST_HELD * 2 - 1)))
    );
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

#[test]
fn notices_are_due_a_tick_only_when_a_launch_errors_hold_ends() {
    let start = Instant::now();
    let mut notices = Notices::new();
    assert_eq!(notices.due(), None);
    notices.post(&request("TV"), start);
    assert_eq!(notices.due(), None, "a request waits for nothing");
    notices.post(&launch_error(), start);
    notices.post(&request("Phone"), start + Duration::from_secs(1));
    let due = notices.due().expect("the launch error holds the slot");
    assert_eq!(due, start + QUIT_HOLDS_FOR);
    // Before then a tick does nothing; at it the held request goes up.
    assert_eq!(notices.tick(due - Duration::from_millis(1)), Vec::new());
    assert_eq!(texts(&notices.tick(due)), ["TV", "Phone"]);
    assert_eq!(notices.due(), None);
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
