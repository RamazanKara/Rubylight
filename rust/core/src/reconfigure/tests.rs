use super::*;

fn mode(width: u32, height: u32, fps: u32) -> Mode {
    Mode {
        width,
        height,
        fps_millihz: fps * 1000,
    }
}

fn payload(width: u16, height: u16, fps_millihz: u32) -> Vec<u8> {
    Request {
        width,
        height,
        fps_millihz,
    }
    .encode()
    .to_vec()
}

#[test]
fn decodes_and_validates_requests() {
    let now = Instant::now();
    let mut state = Reconfiguration::new(mode(2400, 1080, 120));
    assert_eq!(
        state.on_payload(now, &payload(2176, 1812, 120_000)),
        Outcome::Queued(mode(2176, 1812, 120))
    );
    // Odd sizes, tiny sizes, rates outside 10-500 fps and short payloads.
    let mut state = Reconfiguration::new(mode(2400, 1080, 120));
    for bad in [
        payload(2177, 1812, 120_000),
        payload(200, 1812, 120_000),
        payload(2176, 1812, 5_000),
        payload(2176, 1812, 600_000),
        payload(2176, 1812, 120_000)[..11].to_vec(),
    ] {
        assert_eq!(state.on_payload(now, &bad), Outcome::Invalid);
    }
    let mut wrong_version = payload(2176, 1812, 120_000);
    wrong_version[0] = 9;
    assert_eq!(state.on_payload(now, &wrong_version), Outcome::Invalid);
    assert_eq!(state.take_due(now + Duration::from_secs(5)), None);
}

#[test]
fn an_identical_request_changes_nothing() {
    let now = Instant::now();
    let mut state = Reconfiguration::new(mode(1920, 1080, 60));
    assert_eq!(
        state.on_request(now, mode(1920, 1080, 60)),
        Outcome::Unchanged
    );
    assert_eq!(state.take_due(now + Duration::from_secs(5)), None);
    // Going back to the current mode within the wait withdraws the request.
    assert_eq!(
        state.on_request(now, mode(2560, 1440, 60)),
        Outcome::Queued(mode(2560, 1440, 60))
    );
    assert_eq!(
        state.on_request(now + Duration::from_millis(50), mode(1920, 1080, 60)),
        Outcome::Unchanged
    );
    assert_eq!(state.take_due(now + Duration::from_secs(5)), None);
    assert_eq!(state.current(), mode(1920, 1080, 60));
}

#[test]
fn a_burst_applies_the_latest_request_once_the_client_settles() {
    let now = Instant::now();
    let ms = |n| now + Duration::from_millis(n);
    let mut state = Reconfiguration::new(mode(2400, 1080, 120));
    state.on_request(now, mode(2000, 1600, 120));
    state.on_request(ms(100), mode(2100, 1700, 120));
    state.on_request(ms(150), mode(2176, 1812, 120));
    // 200 ms after the last request, not the first.
    assert_eq!(state.take_due(ms(299)), None);
    assert_eq!(state.take_due(ms(349)), None);
    assert_eq!(state.take_due(ms(350)), Some(mode(2176, 1812, 120)));
    assert_eq!(state.current(), mode(2176, 1812, 120));
    assert_eq!(state.take_due(ms(1000)), None);
    // Now at that mode, the same request is identical.
    assert_eq!(
        state.on_request(ms(400), mode(2176, 1812, 120)),
        Outcome::Unchanged
    );
}

#[test]
fn a_repeated_request_does_not_restart_the_wait() {
    let now = Instant::now();
    let ms = |n| now + Duration::from_millis(n);
    let mut state = Reconfiguration::new(mode(2400, 1080, 120));
    for at in [0, 100, 190] {
        state.on_request(ms(at), mode(2176, 1812, 120));
    }
    assert_eq!(state.take_due(ms(200)), Some(mode(2176, 1812, 120)));
}

#[test]
fn switches_at_most_once_a_second() {
    let now = Instant::now();
    let ms = |n| now + Duration::from_millis(n);
    let mut state = Reconfiguration::new(mode(2400, 1080, 120));
    state.on_request(now, mode(2176, 1812, 120));
    assert_eq!(state.take_due(ms(200)), Some(mode(2176, 1812, 120)));
    state.on_request(ms(300), mode(2400, 1080, 120));
    assert_eq!(
        state.take_due(ms(600)),
        None,
        "settled, too soon after the switch"
    );
    assert_eq!(state.take_due(ms(1199)), None);
    assert_eq!(state.take_due(ms(1200)), Some(mode(2400, 1080, 120)));
}

#[test]
fn a_refused_switch_keeps_the_mode() {
    let now = Instant::now();
    let mut state = Reconfiguration::new(mode(1920, 1080, 60));
    state.on_request(now, mode(7680, 4320, 60));
    let due = state.take_due(now + Reconfiguration::SETTLE);
    assert_eq!(due, Some(mode(7680, 4320, 60)));
    state.refused(mode(1920, 1080, 60));
    assert_eq!(state.current(), mode(1920, 1080, 60));
    // Asking again is a new request, not an identical one.
    assert_eq!(
        state.on_request(now + Duration::from_secs(1), mode(7680, 4320, 60)),
        Outcome::Queued(mode(7680, 4320, 60))
    );
}

#[test]
fn a_refused_switch_withdraws_a_request_for_the_restored_mode() {
    let now = Instant::now();
    let original = mode(1920, 1080, 60);
    let mut state = Reconfiguration::new(original);
    state.on_request(now, mode(7680, 4320, 60));
    state.take_due(now + Reconfiguration::SETTLE).unwrap();
    state.on_request(now + Reconfiguration::SETTLE, original);
    state.refused(original);
    assert_eq!(state.take_due(now + Duration::from_secs(2)), None);

    let latest = mode(2560, 1440, 60);
    state.on_request(now + Duration::from_secs(3), latest);
    state.refused(original);
    assert_eq!(state.take_due(now + Duration::from_secs(4)), Some(latest));
}

#[test]
fn fractional_rates_below_one_do_not_panic_before_validation() {
    let stream = Negotiated {
        fps: 1,
        rate_millihz: 500,
        ..Default::default()
    };
    assert!(stream.validate().is_err());
    let rates = Rates::new(&stream, "20");
    assert_eq!(rates.period, Duration::from_secs(2));
    assert_eq!(rates.static_period, Duration::from_secs(1));
}

#[test]
fn refuses_disabled_streams_and_pyrowave() {
    let stream = Negotiated::default();
    assert_eq!(refusal(&stream, true), None);
    assert!(refusal(&stream, false).is_some());
    for codec in [0, 1, 2] {
        let stream = Negotiated {
            codec,
            ..Default::default()
        };
        assert_eq!(refusal(&stream, true), None);
    }
    let pyrowave = Negotiated {
        codec: 3,
        ..Default::default()
    };
    assert!(refusal(&pyrowave, true).is_some());
    let packed = Negotiated {
        codec: 1,
        chroma_packing: 1,
        ..stream
    };
    assert!(refusal(&packed, true).unwrap().contains("reconnect"));
}

#[test]
fn applying_a_mode_changes_only_size_and_rate() {
    let mut stream = Negotiated {
        width: 2400,
        height: 1080,
        fps: 60,
        rate_millihz: 59_940,
        bitrate_kbps: 50_000,
        codec: 1,
        hdr: true,
        ..Default::default()
    };
    let before = stream.clone();
    assert_eq!(Mode::of(&stream), mode(2400, 1080, 0).with_millihz(59_940));
    mode(2176, 1812, 120).apply_to(&mut stream);
    assert_eq!(Mode::of(&stream), mode(2176, 1812, 120));
    assert_eq!((stream.width, stream.height, stream.fps), (2176, 1812, 120));
    assert_eq!(
        (stream.bitrate_kbps, stream.codec, stream.hdr),
        (before.bitrate_kbps, before.codec, before.hdr)
    );
    // A fractional rate keeps its exact millihertz.
    mode(1920, 1080, 0)
        .with_millihz(119_880)
        .apply_to(&mut stream);
    assert_eq!((stream.fps, stream.fps_millihz()), (120, 119_880));
    assert_eq!(
        mode(1920, 1080, 0).with_millihz(119_880).to_string(),
        "1920x1080@119.880"
    );
    assert_eq!(mode(2176, 1812, 120).to_string(), "2176x1812@120");
}

impl Mode {
    fn with_millihz(self, fps_millihz: u32) -> Self {
        Self {
            fps_millihz,
            ..self
        }
    }
}

#[test]
fn rates_follow_the_stream_rate() {
    let at = |fps_millihz: u32, codec: u8| Negotiated {
        fps: fps_millihz.div_ceil(1000),
        rate_millihz: fps_millihz,
        codec,
        ..Default::default()
    };
    let rates = Rates::new(&at(60_000, 1), "20");
    assert_eq!(rates.period, Duration::from_nanos(16_666_666));
    assert_eq!(rates.static_period, Duration::from_millis(50));
    assert!(rates.limit_static_rate);
    let rates = Rates::new(&at(120_000, 1), "20");
    assert_eq!(rates.period, Duration::from_nanos(8_333_333));
    assert_eq!(rates.static_period, Duration::from_millis(50));
    // A minimum above the stream rate is the stream rate.
    let rates = Rates::new(&at(30_000, 1), "45");
    assert_eq!(rates.static_period, Duration::from_secs_f64(1. / 30.));
    assert!(!rates.limit_static_rate);
    // 0: a fifth of the stream rate, at least 10; PyroWave resends at the full rate.
    let rates = Rates::new(&at(120_000, 1), "0");
    assert_eq!(rates.static_period, Duration::from_secs_f64(1. / 24.));
    assert_eq!(
        Rates::new(&at(30_000, 1), "0").static_period,
        Duration::from_millis(100)
    );
    let rates = Rates::new(&at(120_000, 3), "0");
    assert_eq!(rates.static_period, Duration::from_secs_f64(1. / 120.));
    assert!(!rates.limit_static_rate);
    // An unreadable value is the default of 20.
    assert_eq!(
        Rates::new(&at(60_000, 1), "fast").static_period,
        Duration::from_millis(50)
    );
    // A quoted one is read as the console shows it.
    assert_eq!(
        Rates::new(&at(60_000, 1), "\"30\"").static_period,
        Duration::from_secs_f64(1. / 30.)
    );
    assert_eq!(
        Rates::new(&at(120_000, 1), " \"12.5\" ").static_period,
        Duration::from_millis(80)
    );
}
