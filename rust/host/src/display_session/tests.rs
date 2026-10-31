use super::*;
use std::cell::Cell;
use std::sync::mpsc;

#[test]
fn the_configured_scale_reaches_every_stream_display_including_remote_monitors() {
    let mut seen = None;
    let display = r"\\.\DISPLAY7";
    scale_display(
        display,
        125,
        (2560, 1440),
        |output, scale, width, height| {
            seen = Some((output.to_owned(), scale, width, height));
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(seen, Some((display.to_owned(), 125, 2560, 1440)));
    // A display that cannot be scaled is a loud failure, not a silent skip.
    assert!(scale_display("x", 125, (1, 1), |_, _, _, _| anyhow::bail!("no")).is_err());
    // A physical display keeps the user's own scale; a retained remote monitor
    // and a virtual display take the configured one.
    assert_eq!(virtual_display_scale(false, 125), 0);
    assert_eq!(virtual_display_scale(true, 125), 125);
}

fn rotation_config(option: &str, manual: &str) -> Config {
    let mut config = Config::default();
    config
        .values
        .insert("dd_rotation_option".into(), option.into());
    config
        .values
        .insert("dd_manual_rotation".into(), manual.into());
    config
}

#[test]
fn the_clients_display_scale_comes_before_the_configured_one() {
    let mut config = Config::default();
    let options = |pairs: &[(&str, &str)]| -> std::collections::BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    };
    let unread = || -> Result<u32> { panic!("the scale range is read only for a request") };
    let setup = |config: &Config, pairs: &[(&str, &str)], virtual_layout, keeps| {
        scale_setup(config, &options(pairs), virtual_layout, keeps, || Ok(500)).unwrap()
    };
    let plain = |scale, learns| ScaleSetup {
        scale,
        requested: false,
        lowered: None,
        learns,
    };
    // Nothing configured or asked for: Windows' own scale, learned if the device keeps one.
    assert_eq!(setup(&config, &[], true, true), plain(0, true));
    assert_eq!(setup(&config, &[], true, false), plain(0, false));
    config
        .values
        .insert("dd_virtual_display_scale".into(), "125".into());
    assert_eq!(
        scale_setup(&config, &options(&[]), true, true, unread).unwrap(),
        plain(125, false),
        "a configured scale is applied and nothing is learned over it"
    );
    // The device's request wins on a virtual display and suppresses learning.
    assert_eq!(
        setup(&config, &[("rlDisplayScale", "200")], true, true),
        ScaleSetup {
            scale: 200,
            requested: true,
            lowered: None,
            learns: false,
        }
    );
    // The render scale (`scaleFactor`) never sets the Windows scale.
    assert_eq!(
        scale_setup(
            &config,
            &options(&[("scaleFactor", "200")]),
            true,
            true,
            unread
        )
        .unwrap(),
        plain(125, false)
    );
    assert!(
        scale_setup(
            &config,
            &options(&[("rlDisplayScale", "275")]),
            true,
            true,
            unread
        )
        .is_err()
    );
    // A physical display keeps the user's scale whatever the client asks, and the
    // request neither reads the range nor changes what is learned there.
    assert_eq!(
        scale_setup(
            &config,
            &options(&[("rlDisplayScale", "200")]),
            false,
            true,
            unread
        )
        .unwrap(),
        plain(0, false)
    );
    let unset = Config::default();
    assert_eq!(
        scale_setup(
            &unset,
            &options(&[("rlDisplayScale", "200")]),
            false,
            true,
            unread
        )
        .unwrap(),
        plain(0, true)
    );
    // Too large for the resolution: lowered, with the warning for the device.
    let lowered = scale_setup(
        &config,
        &options(&[("rlDisplayScale", "300")]),
        true,
        false,
        || Ok(175),
    )
    .unwrap();
    assert_eq!((lowered.scale, lowered.requested), (175, true));
    assert_eq!(lowered.warning(175), lowered.lowered);
    assert!(lowered.warning(175).unwrap().contains("at most 175%"));
}

#[test]
fn a_requested_scale_windows_refuses_is_a_warning_and_a_configured_one_fails() {
    let refuse = |_: &str, _: i64, _: u32, _: u32| -> Result<()> {
        anyhow::bail!("requested DPI scale is outside this display's range")
    };
    // The device's request: the stream goes on at the scale Windows shows, recovery puts
    // nothing back, and the device hears why.
    let (in_force, baseline) =
        set_up_scale("out", 200, true, None, (1920, 1080), false, refuse, |_| {
            panic!("nothing is learned under a request")
        })
        .unwrap();
    assert_eq!((in_force, baseline), (0, None));
    let setup = ScaleSetup {
        scale: 200,
        requested: true,
        lowered: None,
        learns: false,
    };
    assert!(
        setup
            .warning(in_force)
            .unwrap()
            .contains("did not take the 200%")
    );
    assert_eq!(setup.warning(200), None, "taken: no warning");
    // A request with an unreadable range is asked of Windows as chosen; a refusal is the
    // same warning, not a failed stream.
    assert_eq!(
        fit_client_scale(200, Err(anyhow::anyhow!("no"))),
        (200, None)
    );
    // The host's or the device's configured scale stays fatal.
    assert!(
        set_up_scale(
            "out",
            200,
            false,
            None,
            (1920, 1080),
            false,
            refuse,
            |_| Ok(100)
        )
        .is_err()
    );
    // A request that Windows takes is in force for recovery to put back.
    let (in_force, _) = set_up_scale(
        "out",
        175,
        true,
        Some(150),
        (1920, 1080),
        false,
        |_, percent, _, _| {
            assert_eq!(percent, 175, "the request wins over a kept scale");
            Ok(())
        },
        |_| panic!("not read"),
    )
    .unwrap();
    assert_eq!(in_force, 175);
}

#[test]
fn a_client_scale_too_large_for_the_resolution_is_lowered_with_a_warning() {
    let (scale, warning) = fit_client_scale(200, Ok(150));
    assert_eq!(scale, 150);
    assert!(warning.unwrap().contains("at most 150%"));
    assert_eq!(fit_client_scale(150, Ok(150)), (150, None));
    assert_eq!(fit_client_scale(125, Ok(300)), (125, None));
    // An unreadable range leaves the choice to Windows, which fails loudly if it must.
    assert_eq!(
        fit_client_scale(200, Err(anyhow::anyhow!("no"))),
        (200, None)
    );
}

#[test]
fn a_physical_display_takes_only_the_devices_own_scale() {
    let client = |overrides: serde_json::Value| -> butterpollo_core::state::Client {
        serde_json::from_value(serde_json::json!({
            "name": "phone", "cert": "", "uuid": "u", "config_overrides": overrides,
        }))
        .unwrap()
    };
    assert_eq!(physical_scale(&client(serde_json::json!({}))), 0);
    assert_eq!(
        physical_scale(&client(
            serde_json::json!({"dd_virtual_display_scale": 150})
        )),
        150
    );
    assert_eq!(
        physical_scale(&client(
            serde_json::json!({"dd_virtual_display_scale": "-1"})
        )),
        -1
    );
    assert_eq!(
        physical_scale(&client(
            serde_json::json!({"dd_virtual_display_scale": "big"})
        )),
        0
    );
    assert_eq!(
        physical_scale(&client(serde_json::json!({"dd_rotation_option": "auto"}))),
        0
    );
}

#[test]
fn a_virtual_display_is_made_in_the_portrait_streams_shape() {
    let phone = (1080, 2400);
    assert_eq!(
        virtual_display_size(&rotation_config("auto", "0"), (2560, 1440), phone),
        (1440, 2560)
    );
    assert_eq!(
        virtual_display_size(&rotation_config("manual", "90"), (1920, 1080), (1920, 1080)),
        (1080, 1920)
    );
    assert_eq!(
        virtual_display_size(&Config::default(), (2560, 1440), phone),
        (2560, 1440)
    );
}

#[test]
fn a_physical_display_turns_after_its_mode_is_set_in_its_shape_before_the_turn() -> Result<()> {
    let phone = (1080, 2400);
    let landscape = || Ok((0, (2560, 1440)));
    // Off: the display is not even read.
    let off = plan_physical_turn(&Config::default(), phone, Some(phone), || {
        panic!("not read")
    })?;
    assert_eq!(
        off,
        PhysicalTurn {
            degrees: None,
            before: Some(phone),
            after: Some(phone)
        }
    );
    // A portrait phone turns a landscape display: its mode goes on landscape.
    let auto = rotation_config("auto", "0");
    assert_eq!(
        plan_physical_turn(&auto, phone, Some(phone), landscape)?,
        PhysicalTurn {
            degrees: Some(90),
            before: Some((2400, 1080)),
            after: Some(phone)
        }
    );
    // Without a resolution of its own the display keeps its mode and turns.
    assert_eq!(
        plan_physical_turn(&auto, phone, None, landscape)?.degrees,
        Some(90)
    );
    // A display already in the stream's shape stays as it is.
    assert_eq!(
        plan_physical_turn(&auto, (1920, 1080), Some((1920, 1080)), landscape)?,
        PhysicalTurn {
            degrees: None,
            before: Some((1920, 1080)),
            after: Some((1920, 1080))
        }
    );
    // Upside down keeps the shape.
    assert_eq!(
        plan_physical_turn(
            &rotation_config("manual", "180"),
            (1920, 1080),
            Some((1920, 1080)),
            landscape
        )?,
        PhysicalTurn {
            degrees: Some(180),
            before: Some((1920, 1080)),
            after: Some((1920, 1080))
        }
    );
    // A display turned to 270° goes back upright for a landscape stream.
    assert_eq!(
        plan_physical_turn(&auto, (1920, 1080), Some((1920, 1080)), || Ok((
            270,
            (1440, 2560)
        )))?,
        PhysicalTurn {
            degrees: Some(0),
            before: Some((1080, 1920)),
            after: Some((1920, 1080))
        }
    );
    // A display that cannot be read fails the stream.
    assert!(plan_physical_turn(&auto, phone, None, || anyhow::bail!("gone")).is_err());
    Ok(())
}

#[derive(Default)]
struct FakeLease {
    calls: Vec<String>,
    refuse_turn: bool,
    /// The size its own virtual display shows; None for a shared one.
    size: Option<(u32, u32)>,
    reshaped: Cell<Option<(u32, u32)>>,
    refuse_shape: bool,
}
impl TurnLease for FakeLease {
    fn virtual_size(&self) -> Option<(u32, u32)> {
        self.size
    }
    fn reshape_virtual(&self, width: u32, height: u32) -> Result<()> {
        self.reshaped.set(Some((width, height)));
        if self.refuse_shape {
            anyhow::bail!("Windows did not show {width}x{height}");
        }
        Ok(())
    }
}
impl PhysicalAdjust for FakeLease {
    fn rotate(&mut self, degrees: u16) -> Result<()> {
        self.calls.push(format!("rotate {degrees}"));
        if self.refuse_turn {
            anyhow::bail!("Windows refused the rotation");
        }
        Ok(())
    }
    fn scale(&mut self, configured: i64) -> Result<()> {
        self.calls.push(format!("scale {configured}"));
        Ok(())
    }
}

#[test]
fn a_physical_display_turns_before_it_scales_and_a_refused_turn_fails_the_stream() {
    let mut lease = FakeLease::default();
    adjust_physical(&mut lease, Some(90), 150).unwrap();
    assert_eq!(lease.calls, ["rotate 90", "scale 150"]);
    // Nothing to turn and the device's scale left alone: the lease is untouched.
    let mut lease = FakeLease::default();
    adjust_physical(&mut lease, None, 0).unwrap();
    assert!(lease.calls.is_empty());
    let mut lease = FakeLease::default();
    adjust_physical(&mut lease, None, -1).unwrap();
    assert_eq!(lease.calls, ["scale -1"]);
    let mut lease = FakeLease {
        refuse_turn: true,
        ..Default::default()
    };
    assert!(adjust_physical(&mut lease, Some(90), 150).is_err());
    assert_eq!(lease.calls, ["rotate 90"]);
}

#[test]
fn a_virtual_display_changes_shape_when_its_stream_turns() -> Result<()> {
    let unread = || -> Result<(u16, (u32, u32))> { panic!("a virtual display is not read") };
    let mut lease = FakeLease {
        size: Some((2400, 1080)),
        ..Default::default()
    };
    // The phone turned to portrait: the display takes the portrait shape.
    assert_eq!(
        follow_turn(&mut lease, Follow::Virtual, (1080, 2400), unread)?,
        Some((1080, 2400))
    );
    assert_eq!(lease.reshaped.get(), Some((1080, 2400)));
    assert!(lease.calls.is_empty(), "a virtual display is never turned");
    // A rate change or a stream in the display's shape leaves it.
    let mut lease = FakeLease {
        size: Some((2400, 1080)),
        ..Default::default()
    };
    assert_eq!(
        follow_turn(&mut lease, Follow::Virtual, (1920, 1080), unread)?,
        None
    );
    assert_eq!(lease.reshaped.get(), None);
    // A shared display, or one Windows refused, keeps its shape and says so.
    let mut lease = FakeLease::default();
    assert!(follow_turn(&mut lease, Follow::Virtual, (1080, 2400), unread).is_err());
    let mut lease = FakeLease {
        size: Some((2400, 1080)),
        refuse_shape: true,
        ..Default::default()
    };
    assert!(follow_turn(&mut lease, Follow::Virtual, (1080, 2400), unread).is_err());
    Ok(())
}

#[test]
fn a_physical_display_turns_with_its_stream_and_back() -> Result<()> {
    // Upright landscape; the phone turned to portrait.
    let reads = std::cell::RefCell::new(vec![(90, (1440, 2560)), (0, (2560, 1440))]);
    let read = || Ok(reads.borrow_mut().pop().unwrap());
    let mut lease = FakeLease::default();
    assert_eq!(
        follow_turn(&mut lease, Follow::Physical, (1080, 2400), read)?,
        Some((1440, 2560))
    );
    assert_eq!(lease.calls, ["rotate 90"]);
    assert_eq!(
        lease.reshaped.get(),
        None,
        "a physical display is turned, not reshaped"
    );
    // Turned, and the phone went back to landscape: the display turns upright.
    let reads = std::cell::RefCell::new(vec![(0, (2560, 1440)), (90, (1440, 2560))]);
    let read = || Ok(reads.borrow_mut().pop().unwrap());
    let mut lease = FakeLease::default();
    assert_eq!(
        follow_turn(&mut lease, Follow::Physical, (2400, 1080), read)?,
        Some((2560, 1440))
    );
    assert_eq!(lease.calls, ["rotate 0"]);
    // Already in the stream's shape: read once and left.
    let mut lease = FakeLease::default();
    let reads = std::cell::RefCell::new(vec![(0, (2560, 1440))]);
    let read = || Ok(reads.borrow_mut().pop().unwrap());
    assert_eq!(
        follow_turn(&mut lease, Follow::Physical, (2400, 1080), read)?,
        None
    );
    assert!(lease.calls.is_empty());
    // Another stream keeps the display's rotation: nothing turned.
    let reads = std::cell::RefCell::new(vec![(0, (2560, 1440)), (0, (2560, 1440))]);
    let read = || Ok(reads.borrow_mut().pop().unwrap());
    let mut lease = FakeLease::default();
    assert_eq!(
        follow_turn(&mut lease, Follow::Physical, (1080, 2400), read)?,
        None
    );
    // A refused turn or an unreadable display fails.
    let mut lease = FakeLease {
        refuse_turn: true,
        ..Default::default()
    };
    assert!(
        follow_turn(&mut lease, Follow::Physical, (1080, 2400), || Ok((
            0,
            (2560, 1440)
        )))
        .is_err()
    );
    let mut lease = FakeLease::default();
    assert!(
        follow_turn(
            &mut lease,
            Follow::Physical,
            (1080, 2400),
            || anyhow::bail!("display unavailable")
        )
        .is_err()
    );
    assert!(lease.calls.is_empty());
    Ok(())
}

#[test]
fn only_an_auto_rotation_with_a_lease_follows_a_turned_stream() -> Result<()> {
    let fixture = crate::state::test_support::Fixture::new();
    let mut prepared = test_prepared(&fixture.host);
    let before = prepared.recovery_dimensions;
    // Not auto: a turned stream leaves the display alone.
    prepared.follow_shape((before.1, before.0))?;
    // Auto, but without a lease of its own (a remote monitor): nothing to turn.
    prepared.follow = Some(Follow::Physical);
    prepared.follow_shape((before.1, before.0))?;
    assert_eq!(prepared.recovery_dimensions, before);
    Ok(())
}

#[test]
fn a_virtual_hdr_display_gets_the_physical_calibration_only_when_nothing_else_is_chosen() {
    let looked = Cell::new(0);
    let calibration = || {
        looked.set(looked.get() + 1);
        Some("calibrated.icm".to_string())
    };
    assert_eq!(
        stream_profile(true, None, true, calibration),
        Some("calibrated.icm".into())
    );
    assert_eq!(
        stream_profile(true, Some(""), true, calibration),
        Some("calibrated.icm".into())
    );
    assert_eq!(looked.get(), 2);
    // The device's own profile wins; SDR, a physical display and the
    // setting off never read the physical display's.
    assert_eq!(
        stream_profile(true, Some("device.icm"), true, calibration),
        Some("device.icm".into())
    );
    assert_eq!(stream_profile(false, None, true, calibration), None);
    assert_eq!(
        stream_profile(false, Some("device.icm"), true, calibration),
        None
    );
    assert_eq!(stream_profile(true, None, false, calibration), None);
    assert_eq!(looked.get(), 2);
    assert_eq!(stream_profile(true, None, true, || None), None);
}

#[test]
fn a_window_output_streams_that_window_and_a_display_name_does_not() {
    let window = Config::parse(r"output_name = window:0x2056c/812@\\.\DISPLAY1 ").unwrap();
    assert_eq!(
        configured_window(&window).unwrap(),
        Some(
            butterpollo_windows::capture::WindowTarget::new(0x2056c, r"\\.\DISPLAY1").owned_by(812)
        )
    );
    for display in ["", r"output_name=\\.\DISPLAY1", "output_name=Dell U2723QE"] {
        assert_eq!(
            configured_window(&Config::parse(display).unwrap()).unwrap(),
            None,
            "{display}"
        );
    }
    // A malformed target is refused instead of being streamed as a display.
    for broken in [
        "output_name=window:garbage",
        r"output_name=window:0@\\.\DISPLAY1",
        "output_name=window:12@",
        r"output_name=window:@\\.\DISPLAY1",
        // Without its process a reused handle could show another window.
        r"output_name=window:0x2056c@\\.\DISPLAY1",
    ] {
        let error = configured_window(&Config::parse(broken).unwrap()).unwrap_err();
        assert!(
            format!("{error:#}").contains("output_name"),
            "{broken}: {error:#}"
        );
    }
}

#[test]
fn a_closed_configured_window_refuses_the_stream_with_the_reason() {
    // No window has this handle; a minimised, reused or misplaced one is
    // refused the same way (capture::window's tests).
    let gone =
        butterpollo_windows::capture::WindowTarget::new(0x7fff_fff0, r"\\.\DISPLAY1").owned_by(9);
    let error = format!("{:#}", configured_window_open(&gone).unwrap_err());
    assert!(
        error.contains(r"output_name names the window window:2147483632/9@\\.\DISPLAY1"),
        "{error}"
    );
    assert!(error.contains("closed"), "{error}");
}

#[test]
fn a_window_stream_is_not_kept_for_a_reconnect_and_a_display_stream_is() {
    // Resuming a kept window stream would skip the window's check and
    // send its warnings to the first session; the launch prepares again.
    let fixture = crate::state::test_support::Fixture::new();
    let config = Config::default();
    let stream = Negotiated {
        width: 1920,
        height: 1080,
        fps: 60,
        ..Default::default()
    };
    let window = Ready::window(
        &fixture.host,
        &stream,
        &config,
        r"window:5/9@\\.\DISPLAY1".into(),
        Box::new(|| Ok(r"window:5/9@\\.\DISPLAY1".into())),
        Default::default(),
    )
    .unwrap();
    assert!(!window.display.kept_for_reconnect());
    let prepared = test_prepared(&fixture.host);
    let limiter =
        limiter::Lease::acquire(&fixture.host.directory, &config, &prepared.framegen).unwrap();
    let display = Ready::prepare((prepared, limiter)).unwrap();
    assert!(display.display.kept_for_reconnect());
}

#[test]
fn only_an_opted_in_exclusive_layout_is_held_while_paused() {
    let on = Config::parse("dd_hold_layout_while_paused=true").unwrap();
    // Issue #56: off by default, so a paused stream leaves Win+P alone.
    assert!(!holds_layout_while_paused(&Config::default(), true));
    assert!(holds_layout_while_paused(&on, true));
    // Extended layouts never switched a monitor off; there is nothing to hold.
    assert!(!holds_layout_while_paused(&on, false));
    // Held or not, a paused stream with no layout to guard still renews its lease.
    let fixture = crate::state::test_support::Fixture::new();
    let mut paused = test_prepared(&fixture.host);
    paused.hold_layout_while_paused = true;
    paused.feed(false).unwrap();
}

/// A game display kept between streams, with no other holder, for tests
/// of the session state and the grace period.
pub(crate) fn retained_display(host: &Shared, config: &Config) -> Arc<Ready> {
    let prepared = test_prepared(host);
    let limiter = limiter::Lease::acquire(&host.directory, config, &prepared.framegen).unwrap();
    Ready::prepare((prepared, limiter)).unwrap().display.clone()
}

fn test_prepared(host: &Shared) -> Prepared {
    let framegen = Policy::resolve(
        &Config::default(),
        Rate(60_000),
        false,
        "none",
        false,
        false,
    )
    .unwrap();
    Prepared {
        display: None,
        output: String::new(),
        framegen,
        _arrangement: None,
        _activation: None,
        _profile: None,
        _retained: None,
        _vulkan: None,
        _golden: None,
        mode: stream_mode(&Negotiated::default()),
        client_display: None,
        virtual_layout: Some("extended".into()),
        layout_owner: None,
        inherited_layout: Some("extended".into()),
        resolution_owner: None,
        scale_baseline: None,
        follow: None,
        layout_snapshot: None,
        size_snapshot: None,
        scale_snapshot: None,
        scales_set: Mutex::new(Vec::new()),
        revision: 0,
        recovery: Recovery::new(),
        layout_watch: Default::default(),
        hold_layout_while_paused: false,
        mode_report: ModeReport::new(Arc::new(Warnings::default()), (None, None, None), 60_000),
        recovery_scale: 0,
        recovery_dimensions: (640, 480),
        recovery_profile: None,
        host: Arc::downgrade(host),
    }
}

#[test]
fn layout_preferences_are_saved_at_disconnect_and_not_relearned_while_paused() {
    let fixture = crate::state::test_support::Fixture::new();
    let client = fixture.client(u32::MAX);
    let config = Config::parse("dd_config_revert_delay=0").unwrap();
    *fixture.host.config.write().unwrap() = config.clone();
    let mut prepared = test_prepared(&fixture.host);
    prepared.output = r"\\.\DISPLAY9".into();
    prepared.layout_owner = Some(client.uuid.clone());
    prepared.layout_snapshot = Some(vec![(prepared.output.clone(), true)]);
    let limiter =
        limiter::Lease::acquire(&fixture.host.directory, &config, &prepared.framegen).unwrap();
    let first = Ready::prepare((prepared, limiter)).unwrap();
    let display = first.display.clone();
    let second = display
        .clone()
        .resume(
            &fixture.host.directory,
            &config,
            Arc::new(Warnings::default()),
        )
        .unwrap();
    let chosen = || {
        fixture.host.paired.read().unwrap().clients[0]
            .extra
            .get("virtual_display_layout")
            .cloned()
    };
    drop(first);
    assert_eq!(chosen(), None);
    drop(second);
    assert_eq!(chosen(), Some(serde_json::json!("exclusive")));
    assert_eq!(
        fixture.host.paired.read().unwrap().clients[0]
            .extra
            .get(LEARNED_OVER),
        Some(&serde_json::json!("extended")),
        "the learned layout records the layout it replaced"
    );
    // A later local layout differs, but the paused reaper must not save it.
    display._state.lock().unwrap().layout_snapshot = Some(vec![
        (r"\\.\DISPLAY9".into(), true),
        (r"\\.\DISPLAY1".into(), false),
    ]);
    fixture
        .host
        .app_display
        .lock()
        .unwrap()
        .insert(client.uuid, (display, None));
    fixture.host.reap_paused_display();
    assert!(fixture.host.app_display.lock().unwrap().is_empty());
    assert_eq!(chosen(), Some(serde_json::json!("exclusive")));
}

#[test]
fn a_resolution_chosen_during_the_stream_is_kept_for_the_device_at_disconnect() {
    let fixture = crate::state::test_support::Fixture::new();
    let client = fixture.client(u32::MAX);
    let config = Config::parse("dd_config_revert_delay=0").unwrap();
    *fixture.host.config.write().unwrap() = config.clone();
    let kept = || {
        let paired = fixture.host.paired.read().unwrap();
        let extra = &paired.clients[0].extra;
        assert_eq!(
            extra.get("display_mode"),
            None,
            "the refresh is never pinned"
        );
        extra.get(REMEMBERED_RESOLUTION).cloned()
    };
    let uhd = (3840, 2160);
    let qhd = (2560, 1440);
    for (owner, shown, expected) in [
        // Not opted in.
        (false, qhd, None),
        // Untouched: a refresh Windows lowered (VRR's 1000 Hz) is not a choice.
        (true, uhd, None),
        // Issue #36: 1440p picked in Windows under a 4K stream.
        (true, qhd, Some(serde_json::json!("2560x1440"))),
    ] {
        let mut prepared = test_prepared(&fixture.host);
        prepared.recovery_dimensions = uhd;
        prepared.resolution_owner = owner.then(|| client.uuid.clone());
        prepared.size_snapshot = Some((uhd, shown));
        let limiter =
            limiter::Lease::acquire(&fixture.host.directory, &config, &prepared.framegen).unwrap();
        let stream = Ready::prepare((prepared, limiter)).unwrap();
        let display = stream.display.clone();
        drop(stream);
        assert_eq!(kept(), expected);
        let dimensions = display._state.lock().unwrap().recovery_dimensions;
        assert_eq!(dimensions, if expected.is_some() { qhd } else { uhd });
    }
}

#[test]
fn a_scale_chosen_during_the_stream_is_kept_for_the_device_at_disconnect() {
    let fixture = crate::state::test_support::Fixture::new();
    let client = fixture.client(u32::MAX);
    let config = Config::parse("dd_config_revert_delay=0").unwrap();
    *fixture.host.config.write().unwrap() = config.clone();
    let kept = || {
        let paired = fixture.host.paired.read().unwrap();
        let extra = &paired.clients[0].extra;
        assert_eq!(
            extra.get("display_mode"),
            None,
            "the refresh is never pinned"
        );
        extra.get(REMEMBERED_SCALE).cloned()
    };
    for (owner, baseline, shown, expected) in [
        // Not opted in: nothing is stored.
        (false, Some(100), 200, None),
        // Untouched.
        (true, Some(100), 100, None),
        // No baseline: a configured scale, or one Windows did not let us read.
        (true, None, 200, None),
        // Issue #36: 200% picked in Windows.
        (true, Some(100), 200, Some(serde_json::json!(200))),
    ] {
        let mut prepared = test_prepared(&fixture.host);
        prepared.resolution_owner = owner.then(|| client.uuid.clone());
        prepared.scale_baseline = baseline;
        prepared.scale_snapshot = Some(shown);
        let limiter =
            limiter::Lease::acquire(&fixture.host.directory, &config, &prepared.framegen).unwrap();
        let stream = Ready::prepare((prepared, limiter)).unwrap();
        let display = stream.display.clone();
        drop(stream);
        assert_eq!(kept(), expected);
        let recovery = display._state.lock().unwrap().recovery_scale;
        assert_eq!(recovery, if expected.is_some() { 200 } else { 0 });
    }
}

#[test]
fn a_scale_windows_refused_is_a_warning_and_is_not_kept() {
    let fixture = crate::state::test_support::Fixture::new();
    let client = fixture.client(u32::MAX);
    let config = Config::parse("dd_config_revert_delay=0").unwrap();
    *fixture.host.config.write().unwrap() = config.clone();
    fixture.host.paired.write().unwrap().clients[0]
        .extra
        .insert(REMEMBERED_SCALE.into(), serde_json::json!(150));
    // Windows refuses the kept 150% and the display stays at 100%. The refusal does
    // not fail the stream, and the baseline is what Windows shows, not the refused 150.
    let (in_force, baseline) = set_up_scale(
        "out",
        0,
        false,
        Some(150),
        (2560, 1440),
        true,
        |_, _, _, _| anyhow::bail!("requested DPI scale is outside this display's range"),
        |_| Ok(100),
    )
    .unwrap();
    assert_eq!(in_force, 0, "recovery does not retry a refused scale");
    assert_eq!(baseline, Some(100));
    let mut prepared = test_prepared(&fixture.host);
    prepared.resolution_owner = Some(client.uuid.clone());
    prepared.scale_baseline = baseline;
    prepared.scale_snapshot = Some(100);
    let limiter =
        limiter::Lease::acquire(&fixture.host.directory, &config, &prepared.framegen).unwrap();
    drop(Ready::prepare((prepared, limiter)).unwrap());
    assert_eq!(
        fixture.host.paired.read().unwrap().clients[0]
            .extra
            .get(REMEMBERED_SCALE),
        Some(&serde_json::json!(150)),
        "the 100% Windows shows is not saved over the kept scale"
    );
}

#[test]
fn the_scale_baseline_is_read_after_the_kept_scale_is_applied() {
    let log = std::cell::RefCell::new(Vec::new());
    let (in_force, baseline) = set_up_scale(
        "out",
        0,
        false,
        Some(200),
        (2560, 1440),
        true,
        |_, percent, _, _| {
            log.borrow_mut().push(format!("set {percent}"));
            Ok(())
        },
        |_| {
            log.borrow_mut().push("read".to_owned());
            Ok(200)
        },
    )
    .unwrap();
    assert_eq!(*log.borrow(), ["set 200", "read"]);
    assert_eq!((in_force, baseline), (200, Some(200)));
    // Nothing is read when a scale is configured or the device keeps none.
    let (_, baseline) = set_up_scale(
        "out",
        150,
        false,
        Some(200),
        (2560, 1440),
        false,
        |_, _, _, _| Ok(()),
        |_| panic!("the scale is not read"),
    )
    .unwrap();
    assert_eq!(baseline, None);
}

#[test]
fn resuming_puts_the_scale_back_that_a_pause_may_have_changed() {
    let fixture = crate::state::test_support::Fixture::new();
    let config = Config::parse("dd_config_revert_delay=0").unwrap();
    *fixture.host.config.write().unwrap() = config.clone();
    for (baseline, recovery_scale, expected) in [
        // A device keeping the scale: the baseline, so a change at the PC while paused
        // is not learned at the next disconnect.
        (Some(200), 0, vec![200]),
        // A configured scale.
        (None, 150, vec![150]),
        // Nothing to put back.
        (None, 0, vec![]),
    ] {
        let mut prepared = test_prepared(&fixture.host);
        prepared.scale_baseline = baseline;
        prepared.recovery_scale = recovery_scale;
        let limiter =
            limiter::Lease::acquire(&fixture.host.directory, &config, &prepared.framegen).unwrap();
        let display = Ready::prepare((prepared, limiter)).unwrap().display.clone();
        let resumed = display
            .clone()
            .resume(
                &fixture.host.directory,
                &config,
                Arc::new(Warnings::default()),
            )
            .unwrap();
        assert_eq!(
            *display._state.lock().unwrap().scales_set.lock().unwrap(),
            expected
        );
        drop(resumed);
    }
}

#[test]
fn a_kept_scale_is_applied_unless_a_scale_is_configured() {
    let mut extra = std::collections::BTreeMap::new();
    extra.insert(REMEMBERED_SCALE.to_owned(), serde_json::json!(200));
    let applied = |configured: i64, remembered: Option<u32>| {
        let mut calls = Vec::new();
        let in_force = apply_virtual_scale(
            r"\\.\DISPLAY9",
            configured,
            false,
            remembered,
            (2560, 1440),
            |output, percent, width, height| {
                calls.push((output.to_owned(), percent, width, height));
                Ok(())
            },
        )
        .unwrap();
        (calls, in_force)
    };
    let call = |percent| vec![(r"\\.\DISPLAY9".to_owned(), percent, 2560, 1440)];
    // The kept scale is applied at the display's mode, which is the kept resolution.
    assert_eq!(applied(0, kept_scale(true, &extra)), (call(200), 200));
    // A configured scale, a percentage or recommended, wins and nothing is learned over it.
    assert_eq!(applied(150, kept_scale(true, &extra)), (call(150), 150));
    assert_eq!(applied(-1, kept_scale(true, &extra)), (call(-1), -1));
    // Nothing kept: the unconfigured scale 0 leaves Windows' own.
    assert_eq!(
        applied(0, kept_scale(true, &Default::default())),
        (call(0), 0)
    );
    // The option off: a scale left over is not applied.
    assert_eq!(kept_scale(false, &extra), None);
    assert_eq!(applied(0, kept_scale(false, &extra)), (call(0), 0));
    // Anything but a positive percentage is not a scale.
    for bad in [
        serde_json::json!("200"),
        serde_json::json!(0),
        serde_json::json!(-5),
    ] {
        extra.insert(REMEMBERED_SCALE.to_owned(), bad);
        assert_eq!(kept_scale(true, &extra), None);
    }
}

#[test]
fn disconnect_reaping_waits_for_active_owners_and_releases_exclusive_layouts() {
    for (settings, exclusive, released) in [
        ("dd_config_revert_delay=0", false, true),
        (
            "dd_config_revert_on_disconnect=false\ndd_paused_virtual_display_timeout_secs=0\ndd_config_revert_delay=0",
            false,
            false,
        ),
        (
            "dd_config_revert_on_disconnect=false\ndd_paused_virtual_display_timeout_secs=0\ndd_config_revert_delay=0",
            true,
            true,
        ),
    ] {
        let fixture = crate::state::test_support::Fixture::new();
        let config = Config::parse(settings).unwrap();
        *fixture.host.config.write().unwrap() = config.clone();
        let prepared = test_prepared(&fixture.host);
        let limiter =
            limiter::Lease::acquire(&fixture.host.directory, &config, &prepared.framegen).unwrap();
        let mut stream = Ready::prepare((prepared, limiter)).unwrap();
        // Supply the arrangement result without touching Windows displays.
        Arc::get_mut(&mut stream.display).unwrap().exclusive_layout = exclusive;
        fixture
            .host
            .app_display
            .lock()
            .unwrap()
            .insert("client".into(), (stream.display.clone(), None));
        fixture.host.reap_paused_display();
        assert_eq!(fixture.host.app_display.lock().unwrap().len(), 1);
        drop(stream);
        fixture.host.reap_paused_display();
        assert_eq!(
            fixture.host.app_display.lock().unwrap().is_empty(),
            released,
            "{settings}, exclusive={exclusive}"
        );
    }
}

#[test]
fn a_retained_display_stops_enforcing_layout_after_the_last_preparation_drops() {
    let fixture = crate::state::test_support::Fixture::new();
    let config = Config::default();
    let prepared = test_prepared(&fixture.host);
    let limiter =
        limiter::Lease::acquire(&fixture.host.directory, &config, &prepared.framegen).unwrap();
    let first = Ready::prepare((prepared, limiter)).unwrap();
    let display = first.display.clone();
    let mut changed = Config::parse("virtual_display_layout=extended").unwrap();
    let mut client = fixture.client(u32::MAX);
    let mut app: butterpollo_core::state::App =
        serde_json::from_value(serde_json::json!({"name":"Game"})).unwrap();
    assert!(display.serves(
        None,
        virtual_display_layout(&changed, &client, Some(&app), None)
    ));
    for layout in ["exclusive", "extended_primary"] {
        for source in ["host", "device", "app"] {
            match source {
                "host" => {
                    changed
                        .values
                        .insert("virtual_display_layout".into(), layout.into());
                }
                "device" => {
                    client
                        .extra
                        .insert("virtual_display_layout".into(), layout.into());
                }
                _ => {
                    app.extra
                        .insert("virtual-display-layout".into(), layout.into());
                }
            }
            assert!(
                !display.serves(
                    None,
                    virtual_display_layout(&changed, &client, Some(&app), None)
                ),
                "{source}: {layout}"
            );
            changed
                .values
                .insert("virtual_display_layout".into(), "extended".into());
            client.extra.remove("virtual_display_layout");
            app.extra.remove("virtual-display-layout");
        }
    }
    assert_eq!(display.active.load(Ordering::Acquire), 1);
    let second = display
        .clone()
        .resume(
            &fixture.host.directory,
            &config,
            Arc::new(Warnings::default()),
        )
        .unwrap();
    assert_eq!(display.active.load(Ordering::Acquire), 2);
    drop(first);
    assert_eq!(display.active.load(Ordering::Acquire), 1);
    drop(second);
    assert_eq!(display.active.load(Ordering::Acquire), 0);
    let mut paused = display._state.lock().unwrap();
    paused.recovery.start(std::time::Instant::now());
    paused.feed(false).unwrap();
    assert!(paused.recovery.pending);
    drop(paused);
    drop(display);
}

#[test]
#[ignore = "creates a virtual display, disables it while paused, and resumes it"]
fn native_paused_game_leaves_a_disabled_display_off_until_resume() -> Result<()> {
    use butterpollo_windows::display::{Snapshot, Topology, VirtualOptions, monitors};

    let before = Snapshot::capture()?;
    let result = (|| -> Result<()> {
        let fixture = crate::state::test_support::Fixture::new();
        let config = Config::default();
        let guard = Guard::new_virtual_options(
            "",
            true,
            &format!("paused-game-test-{}", std::process::id()),
            1280,
            720,
            Rate(60_000),
            None,
            None,
            None,
            &VirtualOptions::default(),
        )?;
        let target = monitors()?
            .into_iter()
            .find(|monitor| monitor.matches(&guard.output))
            .context("test display unavailable")?;
        let mut prepared = test_prepared(&fixture.host);
        prepared.output = guard.output.clone();
        prepared.display = Some(guard);
        let limiter =
            limiter::Lease::acquire(&fixture.host.directory, &config, &prepared.framegen)?;
        let stream = Ready::prepare((prepared, limiter))?;
        let display = stream.display.clone();
        drop(stream);
        anyhow::ensure!(display.active.load(Ordering::Acquire) == 0);

        // The same operation as a local layout that excludes the virtual display.
        let kept: Vec<_> = monitors()?
            .into_iter()
            .filter(|monitor| monitor.device_id != target.device_id)
            .map(|monitor| monitor.device_id)
            .collect();
        anyhow::ensure!(!kept.is_empty(), "test needs another active display");
        Topology::set_active(&kept)?;
        let physical = Snapshot::capture()?;
        // Cross the driver's 10 s watchdog and its 3 s polling ticks.
        let deadline = std::time::Instant::now() + Duration::from_secs(16);
        while std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(250));
            anyhow::ensure!(
                !monitors()?.iter().any(|m| m.device_id == target.device_id),
                "paused game reactivated the display the user switched off"
            );
        }
        anyhow::ensure!(
            Topology::query_all()?
                .monitors()
                .iter()
                .any(|m| m.device_id == target.device_id),
            "paused game's display lease expired"
        );
        anyhow::ensure!(
            serde_json::to_value(Snapshot::capture()?)? == serde_json::to_value(physical)?,
            "paused renewal changed the physical displays"
        );

        let resumed = display.clone().resume(
            &fixture.host.directory,
            &config,
            Arc::new(Warnings::default()),
        )?;
        let (output, generation) = display.capture_target();
        anyhow::ensure!(
            monitors()?
                .iter()
                .any(|m| m.device_id == target.device_id && m.matches(&output)),
            "resume returned before publishing the active capture target"
        );
        anyhow::ensure!(generation > 0, "resume did not publish the new generation");
        drop(resumed);
        drop(display);
        Ok(())
    })();
    before.restore()?;
    result
}

#[test]
fn explicit_virtual_layouts_take_precedence_over_the_legacy_primary_flag() {
    use butterpollo_core::display_policy::Arrangement;
    let config = Config::default();
    let mut client: butterpollo_core::state::Client =
        serde_json::from_value(serde_json::json!({"name":"Device","uuid":"device","cert":""}))
            .unwrap();
    let mut app: butterpollo_core::state::App =
        serde_json::from_value(serde_json::json!({"name":"Game","virtual-display-primary":true}))
            .unwrap();
    assert_eq!(
        virtual_display_layout(&config, &client, None, None),
        "exclusive"
    );
    assert_eq!(
        virtual_display_layout(&config, &client, Some(&app), None),
        "extended_primary"
    );
    app.extra
        .insert("virtual-display-layout".into(), "exclusive".into());
    assert_eq!(
        virtual_display_layout(&config, &client, Some(&app), None),
        "exclusive"
    );
    client
        .extra
        .insert("virtual_display_layout".into(), "extended".into());
    assert_eq!(
        virtual_display_layout(&config, &client, Some(&app), None),
        "extended"
    );
    client
        .extra
        .insert("virtual_display_layout".into(), "exclusive".into());
    assert_eq!(
        virtual_display_layout(&config, &client, Some(&app), None),
        "exclusive"
    );
    assert_eq!(
        virtual_display_layout(
            &config,
            &client,
            Some(&app),
            Some(HostDisplay::Virtual(Arrangement::Primary))
        ),
        "extended_primary"
    );
    for inherited in ["", "global"] {
        client
            .extra
            .insert("virtual_display_layout".into(), inherited.into());
        app.extra
            .insert("virtual-display-layout".into(), inherited.into());
        assert_eq!(
            virtual_display_layout(&config, &client, Some(&app), None),
            "extended_primary"
        );
        app.extra
            .insert("virtual-display-primary".into(), false.into());
        assert_eq!(
            virtual_display_layout(&config, &client, Some(&app), None),
            "exclusive"
        );
        app.extra
            .insert("virtual-display-primary".into(), true.into());
    }
}
/// One stream to the device (`planned` layout, `owner` as the stream sets it
/// when Windows took the layout) that ends with the stream's display in the
/// `ended` state; `during` runs while it is connected. Returns the device's
/// layout settings afterwards.
fn stream_then_disconnect(
    fixture: &crate::state::test_support::Fixture,
    client: &butterpollo_core::state::Client,
    owner: bool,
    ended: Vec<(String, bool)>,
    during: impl FnOnce(),
) -> (Option<serde_json::Value>, Option<serde_json::Value>) {
    let config = Config::parse("dd_config_revert_delay=0").unwrap();
    *fixture.host.config.write().unwrap() = config.clone();
    let mut prepared = test_prepared(&fixture.host);
    prepared.output = r"\\.\DISPLAY9".into();
    prepared.virtual_layout = Some("extended_primary".into());
    prepared.inherited_layout = Some("extended_primary".into());
    prepared.layout_owner = owner.then(|| client.uuid.clone());
    prepared.layout_snapshot = Some(ended);
    let limiter =
        limiter::Lease::acquire(&fixture.host.directory, &config, &prepared.framegen).unwrap();
    let stream = Ready::prepare((prepared, limiter)).unwrap();
    during();
    drop(stream);
    let paired = fixture.host.paired.read().unwrap();
    let extra = &paired.clients[0].extra;
    (
        extra.get("virtual_display_layout").cloned(),
        extra.get(LEARNED_OVER).cloned(),
    )
}
/// The stream's display is on beside the PC's, and no longer the primary.
fn windows_moved_the_primary() -> Vec<(String, bool)> {
    vec![
        (r"\\.\DISPLAY9".into(), false),
        (r"\\.\DISPLAY1".into(), true),
    ]
}
fn set_device_layout(fixture: &crate::state::test_support::Fixture, layout: &str) {
    let mut paired = fixture.host.paired.write().unwrap();
    paired.clients[0]
        .extra
        .insert("virtual_display_layout".into(), layout.into());
    paired.clients[0]
        .extra
        .insert(SET_BY_USER.into(), serde_json::Value::Bool(true));
    paired.clients[0].extra.remove(LEARNED_OVER);
}
#[test]
fn a_device_layout_the_user_set_survives_every_stream() {
    // Issue #40: the layout set under Devices was replaced by "extended" when
    // a later stream ended with Windows' primary elsewhere.
    for layout in ["exclusive", "extended_primary", "extended_isolated"] {
        let fixture = crate::state::test_support::Fixture::new();
        let client = fixture.client(u32::MAX);
        set_device_layout(&fixture, layout);
        for _stream in 0..3 {
            let (kept, over) =
                stream_then_disconnect(&fixture, &client, true, windows_moved_the_primary(), || {});
            assert_eq!(kept, Some(serde_json::json!(layout)));
            assert_eq!(over, None);
        }
    }
}
#[test]
fn a_device_layout_the_user_sets_during_a_stream_is_not_replaced_at_disconnect() {
    let fixture = crate::state::test_support::Fixture::new();
    let client = fixture.client(u32::MAX);
    let (kept, over) =
        stream_then_disconnect(&fixture, &client, true, windows_moved_the_primary(), || {
            set_device_layout(&fixture, "exclusive")
        });
    assert_eq!(kept, Some(serde_json::json!("exclusive")));
    assert_eq!(over, None);
}
#[test]
fn a_device_layout_the_user_set_survives_a_reconnect() {
    let fixture = crate::state::test_support::Fixture::new();
    let client = fixture.client(u32::MAX);
    set_device_layout(&fixture, "extended_primary");
    let config = Config::parse("dd_config_revert_delay=0").unwrap();
    *fixture.host.config.write().unwrap() = config.clone();
    let mut prepared = test_prepared(&fixture.host);
    prepared.output = r"\\.\DISPLAY9".into();
    prepared.virtual_layout = Some("extended_primary".into());
    prepared.layout_owner = Some(client.uuid.clone());
    prepared.layout_snapshot = Some(windows_moved_the_primary());
    let limiter =
        limiter::Lease::acquire(&fixture.host.directory, &config, &prepared.framegen).unwrap();
    let first = Ready::prepare((prepared, limiter)).unwrap();
    let second = first
        .display
        .clone()
        .resume(
            &fixture.host.directory,
            &config,
            Arc::new(Warnings::default()),
        )
        .unwrap();
    drop(first);
    drop(second);
    let paired = fixture.host.paired.read().unwrap();
    let extra = &paired.clients[0].extra;
    assert_eq!(
        extra.get("virtual_display_layout"),
        Some(&serde_json::json!("extended_primary"))
    );
    assert_eq!(extra.get(LEARNED_OVER), None);
}
#[test]
fn a_layout_windows_refused_is_not_learned() {
    // The stream sets no layout owner when Windows did not take the layout.
    let fixture = crate::state::test_support::Fixture::new();
    let client = fixture.client(u32::MAX);
    let (kept, over) =
        stream_then_disconnect(&fixture, &client, false, windows_moved_the_primary(), || {});
    assert_eq!((kept, over), (None, None));
}
#[test]
fn a_learned_device_layout_is_replaced_but_a_chosen_one_is_not_learned_over() {
    let fixture = crate::state::test_support::Fixture::new();
    let client = fixture.client(u32::MAX);
    // No device layout: the one the user left the stream in is learned.
    let (kept, over) =
        stream_then_disconnect(&fixture, &client, true, windows_moved_the_primary(), || {});
    assert_eq!(kept, Some(serde_json::json!("extended")));
    assert_eq!(over, Some(serde_json::json!("extended_primary")));
    // "global" is no layout of its own.
    set_device_layout(&fixture, "global");
    let (kept, _) =
        stream_then_disconnect(&fixture, &client, true, windows_moved_the_primary(), || {});
    assert_eq!(kept, Some(serde_json::json!("extended")));
    // A learned layout follows what the user leaves the next stream in.
    let only = vec![(r"\\.\DISPLAY9".to_string(), true)];
    let (kept, over) = stream_then_disconnect(&fixture, &client, true, only, || {});
    assert_eq!(kept, Some(serde_json::json!("exclusive")));
    assert_eq!(over, Some(serde_json::json!("extended_primary")));
}
#[test]
fn a_layout_saved_before_the_user_marker_existed_stays_learnable() {
    // Learning shipped before the marker: such a layout has neither marker
    // and must not be taken for the user's.
    let fixture = crate::state::test_support::Fixture::new();
    let client = fixture.client(u32::MAX);
    fixture.host.paired.write().unwrap().clients[0]
        .extra
        .insert("virtual_display_layout".into(), "extended".into());
    let (kept, over) = stream_then_disconnect(
        &fixture,
        &client,
        true,
        vec![(r"\\.\DISPLAY9".into(), true)],
        || {},
    );
    assert_eq!(kept, Some(serde_json::json!("exclusive")));
    assert_eq!(over, Some(serde_json::json!("extended_primary")));
}
#[test]
fn a_learned_device_layout_yields_when_the_host_layout_changes() {
    // Issue #40: "extended" was learned over the host's "extended_primary";
    // switching the host to "exclusive" must take effect.
    let mut client: butterpollo_core::state::Client =
        serde_json::from_value(serde_json::json!({"name":"Device","uuid":"device","cert":""}))
            .unwrap();
    client
        .extra
        .insert("virtual_display_layout".into(), "extended".into());
    client
        .extra
        .insert(LEARNED_OVER.into(), "extended_primary".into());
    let primary = Config::parse("virtual_display_layout=extended_primary").unwrap();
    let exclusive = Config::parse("virtual_display_layout=exclusive").unwrap();
    assert_eq!(
        virtual_display_layout(&primary, &client, None, None),
        "extended"
    );
    assert_eq!(
        virtual_display_layout(&exclusive, &client, None, None),
        "exclusive"
    );
    // A layout the user set for the device has no marker and always applies.
    client.extra.remove(LEARNED_OVER);
    assert_eq!(
        virtual_display_layout(&exclusive, &client, None, None),
        "extended"
    );
}
#[test]
fn a_retained_display_matches_only_the_exact_mode_it_was_made_for() {
    let made = Negotiated {
        width: 2560,
        height: 1440,
        fps: 120,
        hdr: true,
        ..Default::default()
    };
    assert_eq!(stream_mode(&made), stream_mode(&made.clone()));
    for other in [
        Negotiated {
            width: 1920,
            ..made.clone()
        },
        Negotiated {
            fps: 60,
            ..made.clone()
        },
        Negotiated {
            rate_millihz: 119_880,
            ..made.clone()
        },
        Negotiated {
            hdr: false,
            ..made.clone()
        },
        Negotiated {
            vrr_low_latency: true,
            ..made.clone()
        },
    ] {
        assert_ne!(stream_mode(&made), stream_mode(&other));
    }
}
#[test]
fn only_a_stream_takes_the_clients_display_choice() {
    use butterpollo_core::display_policy::Arrangement;
    let f = crate::state::test_support::Fixture::new();
    let client = f.client(u32::MAX);
    let launch = |role: Role, value: Option<&str>| {
        let mut launch = f.launch(client.clone(), role);
        if let Some(value) = value {
            launch
                .options
                .insert(HostDisplay::PARAMETER.into(), value.into());
        }
        client_display(&launch)
    };
    assert_eq!(launch(Role::Stream, None), None);
    assert_eq!(
        launch(Role::Stream, Some("physical")),
        Some(HostDisplay::Physical)
    );
    assert_eq!(
        launch(Role::Stream, Some("exclusive")),
        Some(HostDisplay::Virtual(Arrangement::Exclusive))
    );
    assert_eq!(
        launch(Role::Stream, Some("extended_primary")),
        Some(HostDisplay::Virtual(Arrangement::Primary))
    );
    for unknown in ["", "default", "mirror"] {
        assert_eq!(launch(Role::Stream, Some(unknown)), None, "{unknown}");
    }
    assert_eq!(launch(Role::RemoteMonitor, Some("physical")), None);
    assert_eq!(launch(Role::InputOnly, Some("exclusive")), None);
}
#[test]
fn legacy_virtual_display_zero_leaves_the_host_settings_in_control() {
    use butterpollo_core::display_policy::Arrangement;
    let f = crate::state::test_support::Fixture::new();
    let client = f.client(u32::MAX);
    let mut launch = f.launch(client, Role::Stream);
    assert_eq!(client_display(&launch), None);
    launch.options.insert("virtualDisplay".into(), "1".into());
    assert_eq!(client_display(&launch), None);
    // Apollo-compatible clients send 0 whenever their toggle is off; the
    // host's virtual display must still apply.
    launch.options.insert("virtualDisplay".into(), "0".into());
    assert_eq!(client_display(&launch), None);
    // An explicit hostDisplay=physical still selects the physical display.
    launch
        .options
        .insert(HostDisplay::PARAMETER.into(), "physical".into());
    let physical = client_display(&launch);
    assert_eq!(physical, Some(HostDisplay::Physical));
    assert!(!HostDisplay::reusable(
        Some(HostDisplay::Virtual(Arrangement::Exclusive)),
        physical
    ));
    launch
        .options
        .insert(HostDisplay::PARAMETER.into(), "extended".into());
    assert_eq!(
        client_display(&launch),
        Some(HostDisplay::Virtual(Arrangement::Extended))
    );
    for role in [Role::RemoteMonitor, Role::InputOnly] {
        launch.role = role;
        assert_eq!(client_display(&launch), None);
    }
}
#[test]
fn virtual_display_mode_keeps_device_app_host_precedence_and_legacy_global_inheritance() {
    let config = Config::parse("virtual_display_mode=shared").unwrap();
    let mut client: butterpollo_core::state::Client =
        serde_json::from_value(serde_json::json!({"name":"Device","uuid":"device","cert":""}))
            .unwrap();
    let app = serde_json::from_value(
        serde_json::json!({"name":"Game","virtual-display-mode":"per_client"}),
    )
    .unwrap();
    assert_eq!(
        virtual_display_mode(&Config::default(), &client, None, false),
        "disabled"
    );
    assert_eq!(
        virtual_display_mode(&Config::default(), &client, None, true),
        "per_client"
    );
    assert_eq!(virtual_display_mode(&config, &client, None, true), "shared");
    assert_eq!(
        virtual_display_mode(&config, &client, Some(&app), true),
        "per_client"
    );
    client
        .extra
        .insert("virtual_display_mode".into(), "disabled".into());
    assert_eq!(
        virtual_display_mode(&config, &client, Some(&app), true),
        "disabled"
    );
    for inherited in ["global", ""] {
        client
            .extra
            .insert("virtual_display_mode".into(), inherited.into());
        assert_eq!(
            virtual_display_mode(&config, &client, Some(&app), true),
            "per_client"
        );
        assert_eq!(virtual_display_mode(&config, &client, None, true), "shared");
    }
}

#[test]
fn background_display_ignores_shared_and_app_identity_overrides() {
    let fixture = crate::state::test_support::Fixture::new();
    let mut config =
        Config::parse("virtual_display_mode=shared\nvirtual_display_layout=exclusive").unwrap();
    let mut launch = fixture.launch(fixture.client(u32::MAX), Role::Stream);
    launch.options.insert(
        "uniqueid".into(),
        "00000000-0000-0000-0000-000000000001".into(),
    );
    let app: butterpollo_core::state::App = serde_json::from_value(serde_json::json!({
        "name":"Game", "uuid":"app-display", "use-app-identity":true,
        "virtual-display-mode":"shared", "virtual-display-layout":"exclusive"
    }))
    .unwrap();
    assert_eq!(
        virtual_display_identity(&config, &launch, Some(&app)),
        "app-display"
    );
    butterpollo_core::background::apply(&mut config);
    for mode in ["shared", "disabled", "per_client"] {
        launch
            .client
            .extra
            .insert("virtual_display_mode".into(), mode.into());
        assert_eq!(
            virtual_display_mode(&config, &launch.client, Some(&app), true),
            "per_client"
        );
        assert_eq!(
            virtual_display_identity(&config, &launch, Some(&app)),
            launch.client.uuid
        );
        assert_eq!(
            virtual_display_layout(
                &config,
                &launch.client,
                Some(&app),
                Some(HostDisplay::Physical)
            ),
            butterpollo_core::background::LAYOUT
        );
    }
}

#[test]
fn a_faster_stream_request_keeps_only_a_display_that_can_follow_it() {
    // Artemis's Warp launches at 60 and asks for 120. Without a virtual
    // display of its own to raise (a physical display here), the
    // launch's preparation goes back to be prepared again at 120; one
    // made for another size never serves the stream.
    let fixture = crate::state::test_support::Fixture::new();
    let config = Config::default();
    let launched = Negotiated {
        width: 2560,
        height: 1440,
        fps: 60,
        ..Default::default()
    };
    let policy = Policy::resolve(&config, Rate(60_000), false, "none", false, false).unwrap();
    let prepared = Prepared {
        display: None,
        output: String::new(),
        framegen: policy.clone(),
        _arrangement: None,
        _activation: None,
        _profile: None,
        _retained: None,
        _vulkan: None,
        _golden: None,
        mode: stream_mode(&launched),
        client_display: None,
        virtual_layout: None,
        layout_owner: None,
        inherited_layout: None,
        resolution_owner: None,
        scale_baseline: None,
        follow: None,
        layout_snapshot: None,
        size_snapshot: None,
        scale_snapshot: None,
        scales_set: Mutex::new(Vec::new()),
        revision: 0,
        recovery: Recovery::new(),
        layout_watch: Default::default(),
        hold_layout_while_paused: false,
        mode_report: ModeReport::new(Arc::new(Warnings::default()), (None, None, None), 60_000),
        recovery_scale: 0,
        recovery_dimensions: (2560, 1440),
        recovery_profile: None,
        host: Arc::downgrade(&fixture.host),
    };
    let limiter = limiter::Lease::acquire(&fixture.host.directory, &config, &policy).unwrap();
    let mut preparation = Ready::prepare((prepared, limiter)).unwrap();
    let display = preparation.display.clone();
    let launch = fixture.launch(fixture.client(u32::MAX), Role::Stream);
    for request in [
        Negotiated {
            fps: 120,
            ..launched.clone()
        },
        Negotiated {
            width: 1920,
            fps: 120,
            ..launched.clone()
        },
    ] {
        assert!(!display.matches(&request));
        preparation = retime(preparation, &fixture.host, &launch, &request, &config)
            .unwrap()
            .err()
            .expect("prepared again");
        assert_eq!(display.active.load(Ordering::Acquire), 1);
        assert_eq!(display.framegen.lock().unwrap().rate, Rate(60_000));
    }
    // The launch's own rate keeps the preparation as it is.
    assert!(display.matches(&launched));
    drop(preparation);
    assert_eq!(display.active.load(Ordering::Acquire), 0);
}

#[test]
fn vrr_negotiated_after_launch_requires_new_display_preparation() {
    let mut launch = Negotiated {
        fps: 116,
        ..Default::default()
    };
    let negotiated =
        Negotiated::from_sdp(b"a=x-nv-video[0].maxFPS:116\na=x-ss-video[0].vrrLowLatency:1\n")
            .unwrap();
    assert_eq!(negotiated.fps_millihz(), 116_000);
    assert_ne!(stream_mode(&launch), stream_mode(&negotiated));
    launch.vrr_low_latency = true;
    assert_eq!(stream_mode(&launch), stream_mode(&negotiated));
}

fn codes(warnings: &Warnings) -> Vec<String> {
    warnings.snapshot().into_iter().map(|w| w.code).collect()
}

#[test]
fn a_recovered_display_mode_is_reported_on_the_card_of_the_launch_using_it() {
    let first = Arc::new(Warnings::default());
    let report = ModeReport::new(
        first.clone(),
        (Some((3840, 2160)), Some(1_000_000), Some(true)),
        116_000,
    );
    report.publish(Ok((3840, 2160, 1_000_000, true)));
    assert!(codes(&first).is_empty());
    // Windows switched the display back on at its saved 60 Hz mode and
    // refused the stream's: the card says so, the stream continues.
    report.publish(Ok((3840, 2160, 60_000, true)));
    assert_eq!(codes(&first), ["display_refresh"]);
    assert!(first.snapshot()[0].message.contains("60.000 Hz"));
    // A resumed launch sees the current finding on its own card.
    let second = Arc::new(Warnings::default());
    report.attach(second.clone());
    assert_eq!(codes(&second), ["display_refresh"]);
    report.publish(Err(anyhow::anyhow!("display unavailable")));
    assert_eq!(codes(&second), ["display_refresh", "display_verify"]);
    report.publish(Ok((3840, 2160, 1_000_000, true)));
    assert!(codes(&second).is_empty());
    // Nothing found yet: attaching reports nothing.
    let fresh = ModeReport::new(first.clone(), (None, None, None), 60_000);
    let third = Arc::new(Warnings::default());
    fresh.attach(third.clone());
    assert!(codes(&third).is_empty());
}

#[test]
fn a_shared_virtual_display_raised_by_a_joining_stream_is_no_finding() {
    let warnings = Arc::new(Warnings::default());
    let requested = (Some((1920, 1080)), Some(120_000), Some(false));
    let report = ModeReport::new(warnings.clone(), requested, 60_000).shared();
    // A VRR stream that joined raised it to 1000 Hz: that serves this one.
    report.publish(Ok((1920, 1080, 1_000_000, false)));
    assert!(codes(&warnings).is_empty());
    // A slower refresh than requested is still reported.
    report.publish(Ok((1920, 1080, 60_000, false)));
    assert_eq!(codes(&warnings), ["display_refresh"]);
    // A display of its own still reports a refresh it did not ask for.
    let own = Arc::new(Warnings::default());
    ModeReport::new(own.clone(), requested, 60_000).publish(Ok((1920, 1080, 1_000_000, false)));
    assert_eq!(codes(&own), ["display_refresh"]);
}

#[test]
fn a_display_that_cannot_follow_a_turned_stream_says_so_on_the_card() {
    let first = Arc::new(Warnings::default());
    let report = ModeReport::new(first.clone(), (None, None, None), 60_000);
    report.followed(&Err(anyhow::anyhow!("rotation refused")));
    assert_eq!(codes(&first), ["display_follow"]);
    assert!(first.snapshot()[0].message.contains("rotation refused"));
    // A later mode finding keeps it; a resumed launch sees it on its own card.
    report.publish(Ok((1920, 1080, 60_000, false)));
    assert_eq!(codes(&first), ["display_follow"]);
    let second = Arc::new(Warnings::default());
    report.attach(second.clone());
    assert_eq!(codes(&second), ["display_follow"]);
    // The next turn follows: the warning goes.
    report.followed(&Ok(()));
    assert!(codes(&second).is_empty());
}

#[test]
fn a_recovery_sets_the_mode_once_while_its_layout_is_retried() {
    let now = std::time::Instant::now();
    let mut recovery = Recovery::new();
    assert!(!recovery.due(now));
    assert!(!recovery.take_mode());
    recovery.start(now);
    assert!(recovery.due(now));
    assert!(recovery.take_mode());
    // The layout or HDR profile failed: retried a second later, but a
    // refused mode is not requested from Windows again.
    assert!(!recovery.due(now + Duration::from_millis(100)));
    assert!(recovery.due(now + Duration::from_secs(1)));
    assert!(!recovery.take_mode());
    recovery.finish();
    assert!(!recovery.due(now + Duration::from_secs(5)));
    // The next recovery sets it again.
    recovery.start(now + Duration::from_secs(6));
    assert!(recovery.due(now + Duration::from_secs(6)));
    assert!(recovery.take_mode());
}

#[test]
fn the_saved_layout_is_restored_once_after_the_last_stream() {
    let mut baseline = Baseline::new();
    let journal = Cell::new(false);
    let restores = Cell::new(0);
    baseline
        .acquire(|| {
            journal.set(true);
            Ok("layout")
        })
        .unwrap();
    baseline
        .acquire(|| panic!("a second stream must join the recorded layout"))
        .unwrap();
    let restore = |layout: &str| {
        assert_eq!(layout, "layout");
        restores.set(restores.get() + 1);
        journal.set(false);
    };
    baseline.release(restore);
    assert_eq!(restores.get(), 0);
    assert!(journal.get(), "the crash journal must stay pending");
    baseline.release(restore);
    assert_eq!(restores.get(), 1);
    assert!(!journal.get());
    // A failed record leaves no user behind; the next stream records again.
    assert!(baseline.acquire(|| anyhow::bail!("journal busy")).is_err());
    baseline.acquire(|| Ok("next")).unwrap();
    baseline.release(|layout| assert_eq!(layout, "next"));
}

#[test]
fn capture_target_stays_readable_during_refresh_and_publishes_identity_together() {
    let target = CaptureTarget::new(("old-output".into(), 0));
    let publisher = target.clone();
    let (entered, waiting) = mpsc::channel();
    let (release, resume) = mpsc::channel();
    let refresh = std::thread::spawn(move || {
        // Stand in for a slow driver call: publication happens only after
        // maintenance returns, while current capture must remain usable.
        entered.send(()).unwrap();
        resume.recv().unwrap();
        publisher.publish(("new-output".into(), 1));
    });
    waiting.recv_timeout(Duration::from_secs(10)).unwrap();
    let reader = target.clone();
    let (read, result) = mpsc::channel();
    let read_thread = std::thread::spawn(move || read.send(reader.current()).unwrap());
    // The refresh is held until the read returns, so a read that waited
    // for it would time out however long this is. 100 ms also failed
    // when a busy PC was slow to start the reader thread.
    let current = result.recv_timeout(Duration::from_secs(10));
    release.send(()).unwrap();
    refresh.join().unwrap();
    read_thread.join().unwrap();
    assert_eq!(current.unwrap(), ("old-output".into(), 0));
    assert_eq!(target.current(), ("new-output".into(), 1));

    let publisher = target.clone();
    let refresh = std::thread::spawn(move || {
        for generation in 2..10000 {
            publisher.publish((format!("output-{generation}"), generation));
        }
    });
    while !refresh.is_finished() {
        let (output, generation) = target.current();
        if generation > 1 {
            assert_eq!(output, format!("output-{generation}"));
        }
    }
    refresh.join().unwrap();
    assert_eq!(target.current(), ("output-9999".into(), 9999));
}
