use super::*;

const SIZE: (u32, u32) = (1000, 500);

fn settings(mode: TouchMode, sensitivity: u16, gestures: bool) -> Settings {
    Settings {
        mode,
        sensitivity,
        gestures,
    }
}

/// Touches fed at chosen times after the first.
struct Feed {
    profile: TouchProfile,
    settings: Settings,
    start: Instant,
}
impl Feed {
    fn new(settings: Settings) -> Self {
        Self {
            profile: TouchProfile::default(),
            settings,
            start: Instant::now(),
        }
    }
    fn at(&mut self, ms: u64, event: u8, id: u32, x: f32, y: f32) -> Vec<Input> {
        let now = self.start + Duration::from_millis(ms);
        self.profile
            .translate(&self.settings, event, id, [x, y], SIZE, now)
    }
}

fn abs(x: i16, y: i16) -> Input {
    Input::Absolute {
        x,
        y,
        width: i16::MAX,
        height: i16::MAX,
    }
}

#[test]
fn the_profile_comes_from_the_device_configuration() {
    let config =
        Config::parse("touch_mode = trackpad\ntouch_sensitivity = 150\ntouch_gestures = false\n")
            .unwrap();
    assert_eq!(
        Settings::resolve(&config),
        settings(TouchMode::Trackpad, 150, false)
    );
    let default = Settings::resolve(&Config::parse("").unwrap());
    assert_eq!(default, settings(TouchMode::Client, 100, true));
    assert!(!default.translates());
}

#[test]
fn as_the_device_sends_them_turns_nothing_into_mouse_input() {
    let mut feed = Feed::new(settings(TouchMode::Client, 100, true));
    assert!(feed.at(0, DOWN, 1, 0.5, 0.5).is_empty());
    assert!(feed.at(10, UP, 1, 0.5, 0.5).is_empty());
}

#[test]
fn as_a_mouse_the_pointer_follows_the_finger_and_the_left_button_holds() {
    let mut feed = Feed::new(settings(TouchMode::Mouse, 400, true));
    assert_eq!(
        feed.at(0, DOWN, 7, 0.5, 0.25),
        [abs(16384, 8192), button(LEFT, true)]
    );
    // The sensitivity doesn't scale a finger the pointer sits under.
    assert_eq!(feed.at(100, MOVE, 7, 1.0, 0.0), [abs(i16::MAX, 0)]);
    assert_eq!(feed.at(400, UP, 7, 1.0, 0.0), [button(LEFT, false)]);
    // A tap is the press and release already sent: no second click.
    assert_eq!(
        feed.at(500, DOWN, 8, 0.0, 0.0),
        [abs(0, 0), button(LEFT, true)]
    );
    assert_eq!(feed.at(550, UP, 8, 0.0, 0.0), [button(LEFT, false)]);
}

#[test]
fn as_a_trackpad_a_swipe_moves_the_pointer_by_the_sensitivity() {
    let mut feed = Feed::new(settings(TouchMode::Trackpad, 200, true));
    assert!(feed.at(0, DOWN, 1, 0.5, 0.5).is_empty());
    // 0.125 of 1000 pixels at 200 %.
    assert_eq!(feed.at(50, MOVE, 1, 0.625, 0.5), relative(250, 0));
    // Sub-pixel movement adds up instead of being lost: 0.4 px at 200 % twice.
    assert!(feed.at(60, MOVE, 1, 0.6254, 0.5).is_empty());
    assert_eq!(feed.at(70, MOVE, 1, 0.6258, 0.5), relative(1, 0));
    // Upward on a 500-pixel stream at 200 %.
    assert_eq!(feed.at(90, MOVE, 1, 0.6258, 0.375), relative(0, -125));
    // A swipe is no tap.
    assert!(feed.at(100, UP, 1, 0.6258, 0.375).is_empty());
}

fn relative(x: i16, y: i16) -> [Input; 1] {
    [Input::Relative { x, y }]
}

#[test]
fn as_a_trackpad_only_a_quick_still_tap_clicks() {
    let mut feed = Feed::new(settings(TouchMode::Trackpad, 100, true));
    feed.at(0, DOWN, 1, 0.5, 0.5);
    // A wobble within the slop (5 px) is still a tap.
    feed.at(50, MOVE, 1, 0.505, 0.5);
    assert_eq!(feed.at(200, UP, 1, 0.505, 0.5), click(LEFT));
    feed.at(1000, DOWN, 1, 0.5, 0.5);
    assert!(feed.at(1300, UP, 1, 0.5, 0.5).is_empty(), "a long touch");
    feed.at(2000, DOWN, 1, 0.5, 0.5);
    feed.at(2050, MOVE, 1, 0.52, 0.5);
    feed.at(2060, MOVE, 1, 0.5, 0.5);
    assert!(
        feed.at(2100, UP, 1, 0.5, 0.5).is_empty(),
        "a finger that moved and came back"
    );
    feed.at(3000, DOWN, 1, 0.5, 0.5);
    assert!(
        feed.at(3050, CANCEL, 1, 0.5, 0.5).is_empty(),
        "a cancelled touch"
    );
}

#[test]
fn two_fingers_scroll_with_the_content_following_them() {
    for mode in [TouchMode::Mouse, TouchMode::Trackpad] {
        let mut feed = Feed::new(settings(mode, 100, true));
        feed.at(0, DOWN, 1, 0.25, 0.5);
        let second = feed.at(10, DOWN, 2, 0.5, 0.5);
        if mode == TouchMode::Mouse {
            assert_eq!(second, [button(LEFT, false)], "the second finger lets go");
        } else {
            assert!(second.is_empty());
        }
        // Both fingers down 125 px: 2 units a pixel, each finger its half.
        let scroll = |amount, horizontal| Input::Scroll { amount, horizontal };
        assert_eq!(feed.at(50, MOVE, 1, 0.25, 0.75), [scroll(125, false)]);
        assert_eq!(feed.at(50, MOVE, 2, 0.5, 0.75), [scroll(125, false)]);
        // Both fingers right 125 px: the view moves left.
        assert_eq!(feed.at(60, MOVE, 1, 0.375, 0.75), [scroll(-125, true)]);
        assert_eq!(feed.at(60, MOVE, 2, 0.625, 0.75), [scroll(-125, true)]);
        // A scroll is no tap.
        assert!(feed.at(70, UP, 1, 0.375, 0.75).is_empty());
        assert!(feed.at(80, UP, 2, 0.625, 0.75).is_empty());
    }
}

#[test]
fn a_two_finger_tap_clicks_right() {
    for mode in [TouchMode::Mouse, TouchMode::Trackpad] {
        let mut feed = Feed::new(settings(mode, 100, true));
        feed.at(0, DOWN, 1, 0.4, 0.5);
        feed.at(20, DOWN, 2, 0.6, 0.5);
        assert!(feed.at(100, UP, 2, 0.6, 0.5).is_empty());
        assert_eq!(feed.at(120, UP, 1, 0.4, 0.5), click(RIGHT), "{mode:?}");
    }
}

#[test]
fn without_gestures_more_fingers_do_nothing() {
    let mut feed = Feed::new(settings(TouchMode::Trackpad, 100, false));
    feed.at(0, DOWN, 1, 0.25, 0.5);
    assert!(feed.at(10, DOWN, 2, 0.5, 0.5).is_empty());
    assert!(feed.at(20, MOVE, 2, 0.5, 0.75).is_empty(), "no scroll");
    // The first finger still moves the pointer.
    assert_eq!(feed.at(30, MOVE, 1, 0.375, 0.5), relative(125, 0));
    feed.at(40, UP, 2, 0.5, 0.75);
    assert!(feed.at(50, UP, 1, 0.375, 0.5).is_empty(), "no right click");

    let mut feed = Feed::new(settings(TouchMode::Mouse, 100, false));
    feed.at(0, DOWN, 1, 0.5, 0.5);
    assert!(
        feed.at(10, DOWN, 2, 0.6, 0.5).is_empty(),
        "the left button holds"
    );
    assert_eq!(feed.at(20, MOVE, 1, 0.0, 0.0), [abs(0, 0)]);
    assert!(feed.at(30, UP, 2, 0.6, 0.5).is_empty());
    assert_eq!(feed.at(40, UP, 1, 0.0, 0.0), [button(LEFT, false)]);
}

#[test]
fn a_cancel_lets_the_button_go_and_clicks_nothing() {
    let mut feed = Feed::new(settings(TouchMode::Mouse, 100, true));
    feed.at(0, DOWN, 1, 0.5, 0.5);
    feed.at(10, DOWN, 2, 0.6, 0.5);
    assert!(
        feed.at(20, CANCEL_ALL, 0, 0.0, 0.0).is_empty(),
        "already let go"
    );
    // The next touch starts a new gesture.
    assert_eq!(
        feed.at(30, DOWN, 3, 0.0, 0.0),
        [abs(0, 0), button(LEFT, true)]
    );
    assert_eq!(feed.at(40, CANCEL_ALL, 0, 0.0, 0.0), [button(LEFT, false)]);
    assert!(feed.at(50, UP, 3, 0.0, 0.0).is_empty(), "an unknown finger");
}

#[test]
fn hovers_unknown_fingers_and_bad_positions_give_nothing() {
    let mut feed = Feed::new(settings(TouchMode::Trackpad, 100, true));
    assert!(feed.at(0, 0, 1, 0.5, 0.5).is_empty(), "a hover");
    assert!(
        feed.at(0, MOVE, 1, 0.5, 0.5).is_empty(),
        "a finger never down"
    );
    assert!(feed.at(0, DOWN, 1, f32::NAN, 0.5).is_empty());
    assert!(
        feed.at(0, MOVE, 1, 0.6, 0.5).is_empty(),
        "the NaN finger isn't down"
    );
    // A repeated down is no second finger.
    feed.at(0, DOWN, 1, 0.5, 0.5);
    assert!(feed.at(0, DOWN, 1, 0.5, 0.5).is_empty());
    assert_eq!(feed.at(100, UP, 1, 0.5, 0.5), click(LEFT));
}
