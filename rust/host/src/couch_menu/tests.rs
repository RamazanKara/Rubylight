use super::*;
use crate::state::test_support::Fixture;
use butterpollo_core::couch_menu::chord;
use butterpollo_core::native::{LAUNCH, VIEW};

#[test]
fn couch_menu_refuses_input_only_readback_and_tiny_streams() {
    assert_eq!(refusal(Role::Stream, true, [1920, 1080]), None);
    assert_eq!(refusal(Role::RemoteMonitor, true, [1280, 720]), None);
    assert!(
        refusal(Role::InputOnly, true, [1920, 1080])
            .unwrap()
            .contains("input-only")
    );
    assert!(
        refusal(Role::Stream, false, [1920, 1080])
            .unwrap()
            .contains("wgc_direct_encoder_input")
    );
    assert_eq!(
        refusal(Role::Stream, true, [64, 64]).unwrap(),
        "couch menu: stream too small (64x64)"
    );
}

#[derive(Default)]
struct FakeGame {
    log: Vec<String>,
}
impl Game for FakeGame {
    fn release_all(&mut self) {
        self.log.push("release".into());
    }
    fn apply_all(&mut self, events: &[Input]) -> Vec<anyhow::Error> {
        self.log.push(format!("apply {}", events.len()));
        vec![]
    }
    fn pointer(&mut self) -> Option<(f32, f32)> {
        None
    }
}

fn pad(buttons: u32) -> Input {
    Input::Controller {
        id: 0,
        active: 1,
        buttons,
        left_trigger: 0,
        right_trigger: 0,
        sticks: [0; 4],
    }
}

fn followed(f: &Fixture, role: Role) -> (Arc<Session>, PeerMenu) {
    let s = f.session(f.client(VIEW | LAUNCH), role);
    let mut peer = PeerMenu::default();
    let made = peer.follow(&s, || crate::stream::effective_config(&f.host, &s.launch));
    assert_eq!(made, role != Role::InputOnly);
    (s, peer)
}

#[test]
fn couch_menu_chord_releases_the_game_once_before_the_kept_events() {
    let f = Fixture::new();
    let (s, mut peer) = followed(&f, Role::Stream);
    let mut game = FakeGame::default();
    let start = Instant::now();
    let mut events = vec![pad(chord::LS_CLK), pad(chord::PAD_CHORD)];
    let batch = apply_batch(&mut peer, Some(&mut game), &mut events, start);
    assert!(batch.nav.is_empty());
    assert_eq!(game.log, ["apply 2"]);
    // The hold completes on an empty batch at the due time.
    let due = start + chord::PAD_HOLD;
    assert_eq!(peer.next_due(), Some(due));
    let mut events = vec![];
    let batch = apply_batch(&mut peer, Some(&mut game), &mut events, due);
    assert_eq!(batch.nav, [Nav::Opened]);
    assert_eq!(game.log, ["apply 2", "release", "apply 0"]);
    assert!(menu::Shared::is_open(s.couch_menu.load()));
    // Nothing reaches the game while the menu is open.
    let mut events = vec![pad(0x1000)];
    apply_batch(
        &mut peer,
        Some(&mut game),
        &mut events,
        due + Duration::from_millis(10),
    );
    assert_eq!(game.log.last().unwrap(), "apply 0");
    // Closed from elsewhere, the peer tells the worker.
    s.couch_menu.close();
    let mut events = vec![];
    let batch = apply_batch(
        &mut peer,
        Some(&mut game),
        &mut events,
        due + Duration::from_millis(20),
    );
    assert_eq!(batch.nav, [Nav::Closed]);
}

#[test]
fn couch_menu_off_or_input_only_makes_no_filter_and_forwards_everything() {
    let f = Fixture::new();
    f.host
        .config
        .write()
        .unwrap()
        .values
        .insert("couch_menu".into(), "false".into());
    let s = f.session(f.client(VIEW | LAUNCH), Role::Stream);
    let mut peer = PeerMenu::default();
    assert!(!peer.follow(&s, || crate::stream::effective_config(&f.host, &s.launch)));
    // The same session is followed once.
    assert!(!peer.follow(&s, || panic!("read again")));
    let mut game = FakeGame::default();
    let start = Instant::now();
    let mut events = vec![pad(chord::PAD_CHORD)];
    apply_batch(&mut peer, Some(&mut game), &mut events, start);
    assert_eq!(peer.next_due(), None);
    assert_eq!(game.log, ["apply 1"]);
    let (_, input_only) = followed(&f, Role::InputOnly);
    assert!(input_only.filter.is_none());
}

/// A panel laid out for a 1920x1080 stream.
fn frame(origin: [u32; 2], size: [u32; 2]) -> Frame {
    Frame {
        origin,
        size,
        target: [1920, 1080],
        scale: 1,
        rgba: vec![255; (size[0] * size[1] * 4) as usize].into(),
    }
}

#[test]
fn couch_menu_video_overlay_follows_the_shared_state() {
    let shared = menu::Shared::new();
    let mut video = VideoOverlay::default();
    video.poll(&shared, [1920, 1080]);
    assert!(!video.dirty && !video.has_overlay());
    shared.open().unwrap();
    video.poll(&shared, [1920, 1080]);
    // Open with nothing drawn yet: no panel.
    assert!(!video.dirty && !video.has_overlay());
    assert!(shared.publish(frame([64, 64], [256, 128])));
    video.poll(&shared, [1920, 1080]);
    assert!(video.dirty && video.has_overlay());
    video.dirty = false;
    video.poll(&shared, [1920, 1080]);
    assert!(!video.dirty);
    // After a rebuild to a size the panel misses, it waits for a redraw.
    assert!(video.for_encoder([1920, 1080]).is_some());
    assert!(video.for_encoder([128, 128]).is_none());
    assert!(shared.publish(frame([64, 64], [256, 128])));
    video.poll(&shared, [1920, 1080]);
    assert!(video.dirty && video.has_overlay());
    video.dirty = false;
    shared.close();
    video.poll(&shared, [1920, 1080]);
    assert!(video.dirty && !video.has_overlay());
}

#[test]
fn couch_menu_panel_outside_the_stream_closes_the_menu() {
    let shared = menu::Shared::new();
    let mut video = VideoOverlay::default();
    shared.open().unwrap();
    assert!(shared.publish(frame([1792, 1024], [256, 128])));
    video.poll(&shared, [1920, 1080]);
    assert!(!video.has_overlay());
    assert!(!menu::Shared::is_open(shared.load()));
}

#[test]
fn couch_menu_panel_for_another_mode_waits_instead_of_closing() {
    let shared = menu::Shared::new();
    let mut video = VideoOverlay::default();
    shared.open().unwrap();
    // The client asked for 4K and the worker drew for it, but the
    // encoder refused the mode and the stream stayed at 1080p.
    let uhd = Frame {
        target: [3840, 2160],
        ..frame([1792, 1024], [256, 128])
    };
    assert!(shared.publish(uhd));
    video.poll(&shared, [1920, 1080]);
    assert!(!video.dirty && !video.has_overlay());
    assert!(menu::Shared::is_open(shared.load()), "the menu stays open");
    // A mode the stream does take shows the same panel.
    video.poll(&shared, [3840, 2160]);
    assert!(video.dirty && video.has_overlay());
    video.dirty = false;
    // The worker redraws at the mode the stream kept.
    video.poll(&shared, [1920, 1080]);
    assert!(video.dirty && !video.has_overlay());
    video.dirty = false;
    assert!(shared.publish(frame([832, 464], [256, 128])));
    video.poll(&shared, [1920, 1080]);
    assert!(video.dirty && video.has_overlay());
    video.dirty = false;
    // A rebuild to a mode the encoder then refuses drops the panel; the
    // stream back at the old mode shows it again.
    assert!(video.for_encoder([960, 540]).is_none());
    video.poll(&shared, [1920, 1080]);
    assert!(video.dirty && video.has_overlay());
    assert!(menu::Shared::is_open(shared.load()));
}

#[test]
fn couch_menu_strings_are_complete_in_english_and_german() {
    for locale in ["en", "de-DE", "fr"] {
        strings(locale, items::ITEMS).unwrap();
        for key in menu::model::items::ALL {
            let path = menu::model::items::label_path(key);
            let label = i18n::catalogue(locale, &path).unwrap();
            assert_eq!(menu::font::uncovered(&label), None, "{path}");
        }
    }
    assert_eq!(strings("de", &[]).unwrap().title, "Host-Menü");
    assert_eq!(strings("fr", &[]).unwrap().title, "Host menu");
}

#[test]
fn a_second_screens_menu_disconnects_its_own_stream_not_the_first_screens() {
    let f = Fixture::new();
    let device = f.client(VIEW | LAUNCH);
    let first = f.session(device.clone(), Role::Stream);
    let second = f.session(device.clone().second_screen(), Role::Stream);
    assert_eq!(client(&f.host, &first).unwrap().uuid, device.uuid);
    assert_eq!(
        client(&f.host, &second).unwrap().uuid,
        butterpollo_core::state::second_screen_uuid(&device.uuid)
    );
    act(&f.host, &second, &Action::Disconnect).unwrap();
    assert!(second.stopping());
    assert!(!first.stopping());
}

#[test]
fn couch_menu_request_needs_an_own_session_that_can_show_it() {
    let f = Fixture::new();
    let client = f.client(VIEW | LAUNCH);
    let code = |result: Result<bool, Failure>| result.unwrap_err().code();
    assert_eq!(code(request(&f.host, &client, true)), ErrorCode::NoSession);
    let other = Client {
        uuid: "other-device".into(),
        ..client.clone()
    };
    let s = f.session(client.clone(), Role::Stream);
    PeerMenu::default().follow(&s, || crate::stream::effective_config(&f.host, &s.launch));
    assert_eq!(
        code(request(&f.host, &other, true)),
        ErrorCode::NotSessionOwner
    );
    assert!(request(&f.host, &client, true).unwrap());
    assert!(request(&f.host, &client, true).unwrap(), "idempotent");
    assert!(!request(&f.host, &client, false).unwrap());
    assert!(!request(&f.host, &client, false).unwrap(), "idempotent");
}

#[test]
fn couch_menu_off_at_stream_start_stays_off_for_the_stream() {
    for key in ["couch_menu", "wgc_direct_encoder_input"] {
        let f = Fixture::new();
        let client = f.client(VIEW | LAUNCH);
        let set = |value: &str| {
            f.host
                .config
                .write()
                .unwrap()
                .values
                .insert(key.into(), value.into());
        };
        set("false");
        let s = f.session(client.clone(), Role::Stream);
        let mut peer = PeerMenu::default();
        peer.follow(&s, || crate::stream::effective_config(&f.host, &s.launch));
        // Turned on mid-stream: no filter, chord or worker would follow
        // an open, or the encoder would not draw it.
        set("true");
        assert_eq!(
            request(&f.host, &client, true).unwrap_err().code(),
            ErrorCode::TemporarilyUnavailable,
            "{key}"
        );
        assert!(!menu::Shared::is_open(s.couch_menu.load()), "{key}");
        // Closing works with the menu off.
        assert!(!request(&f.host, &client, false).unwrap());
    }
}

#[test]
fn couch_menu_request_waits_for_the_control_peer() {
    let f = Fixture::new();
    let client = f.client(VIEW | LAUNCH);
    let s = f.session(client.clone(), Role::Stream);
    assert_eq!(
        request(&f.host, &client, true).unwrap_err().code(),
        ErrorCode::TemporarilyUnavailable
    );
    assert!(!menu::Shared::is_open(s.couch_menu.load()));
    let mut peer = PeerMenu::default();
    assert!(peer.follow(&s, || crate::stream::effective_config(&f.host, &s.launch)));
    assert!(request(&f.host, &client, true).unwrap());
    // On at stream start, a later change waits for the next stream.
    f.host
        .config
        .write()
        .unwrap()
        .values
        .insert("couch_menu".into(), "false".into());
    assert!(!request(&f.host, &client, false).unwrap());
    assert!(request(&f.host, &client, true).unwrap());
}

#[test]
fn couch_menu_request_refuses_an_input_only_session() {
    let f = Fixture::new();
    let client = f.client(VIEW | LAUNCH);
    let s = f.session(client.clone(), Role::InputOnly);
    assert!(!PeerMenu::default().follow(&s, || panic!("an input-only stream reads no config")));
    let failure = request(&f.host, &client, true).unwrap_err();
    assert_eq!(failure.code(), ErrorCode::TemporarilyUnavailable);
    assert!(
        failure.message().contains("input-only"),
        "{}",
        failure.message()
    );
}

#[test]
fn couch_menu_worker_draws_the_open_menu_and_closes_on_back() {
    let f = Fixture::new();
    let (s, _peer) = followed(&f, Role::Stream);
    let mut workers = Workers::default();
    s.couch_menu.open().unwrap();
    workers.send(&f.host, &s, vec![Nav::Opened]);
    let drawn = |want: bool| {
        let until = Instant::now() + Duration::from_secs(5);
        while s.couch_menu.frame().is_some() != want && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(5));
        }
        s.couch_menu.frame().is_some() == want
    };
    assert!(drawn(true), "the worker drew the main page");
    let mode = s.stream_mode();
    let panel = s.couch_menu.frame().unwrap();
    Overlay::new(1, &panel)
        .validate([mode.width, mode.height])
        .unwrap();
    // Back on the main page is Back to game.
    workers.send(&f.host, &s, vec![Nav::Back]);
    let until = Instant::now() + Duration::from_secs(5);
    while menu::Shared::is_open(s.couch_menu.load()) && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!menu::Shared::is_open(s.couch_menu.load()));
    assert!(drawn(false), "a closed menu has no panel");
    workers.retain(|_| false);
}
