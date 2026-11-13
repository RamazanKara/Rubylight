use super::{fake::*, *};
use crate::state::test_support::Fixture;
use butterpollo_core::{
    session::Role,
    wake::{AdapterFix, idle::ES_SYSTEM_REQUIRED},
};
use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicUsize, Ordering},
};

const MINUTE: Duration = Duration::from_secs(60);

fn configure(f: &Fixture, values: &[(&str, &str)]) {
    let mut config = f.host.config.write().unwrap();
    for (key, value) in values {
        config.values.insert((*key).into(), (*value).into());
    }
}
/// A host with sleep after idle at 10 minutes on the fake PC.
fn armed() -> (Fixture, Arc<Fake>, Instant) {
    let f = Fixture::new();
    let start = Instant::now();
    let fake = Fake::new().on(&f.host);
    configure(&f, &[("sleep_after_idle_minutes", "10")]);
    (f, fake, start)
}

#[test]
fn the_default_config_never_sleeps_and_asks_windows_nothing() {
    let f = Fixture::new();
    let fake = Fake::new().on(&f.host);
    let start = Instant::now();
    for hours in 1..5 {
        tick_at(&f.host, start + hours * 60 * MINUTE);
    }
    assert_eq!(fake.sleeps(), 0);
    assert_eq!(fake.inputs.load(Ordering::SeqCst), 0);
    assert_eq!(status(&f.host).state, api::IdleSleepState::Off);
}

#[test]
fn it_sleeps_exactly_once_when_due() {
    let (f, fake, start) = armed();
    tick_at(&f.host, start + MINUTE);
    assert_eq!(fake.sleeps(), 0);
    let armed = status(&f.host);
    assert_eq!(armed.state, api::IdleSleepState::Armed);
    assert!(armed.remaining.unwrap() <= 9 * MINUTE);
    tick_at(&f.host, start + 11 * MINUTE);
    assert_eq!(fake.sleeps(), 1);
    // The next attempt waits a full interval.
    tick_at(&f.host, start + 12 * MINUTE);
    tick_at(&f.host, start + 20 * MINUTE);
    assert_eq!(fake.sleeps(), 1);
    tick_at(&f.host, start + 22 * MINUTE);
    assert_eq!(fake.sleeps(), 2);
    assert_eq!(
        locked(&f.host.wake.idle).last_sleep.as_ref().unwrap().1,
        None
    );
}

#[test]
fn an_active_or_pending_stream_blocks_sleep() {
    let (f, fake, start) = armed();
    let session = f.session(f.client(0), Role::Stream);
    tick_at(&f.host, start + 11 * MINUTE);
    assert_eq!(fake.sleeps(), 0);
    assert_eq!(status(&f.host).state, api::IdleSleepState::Blocked);
    // A stream skips the helper.
    assert_eq!(fake.inputs.load(Ordering::SeqCst), 0);
    f.host.sessions.lock().unwrap().active.clear();
    drop(session);
    let launch = f.launch(f.client(0), Role::Stream);
    f.host
        .sessions
        .lock()
        .unwrap()
        .pending
        .insert(launch.id.clone(), launch);
    tick_at(&f.host, start + 30 * MINUTE);
    assert_eq!(fake.sleeps(), 0);
    f.host.sessions.lock().unwrap().pending.clear();
    // The quiet interval counts from the stream's end.
    tick_at(&f.host, start + 35 * MINUTE);
    assert_eq!(fake.sleeps(), 0);
    tick_at(&f.host, start + 41 * MINUTE);
    assert_eq!(fake.sleeps(), 1);
}

#[test]
fn a_launched_app_blocks_sleep_unless_sleep_while_suspended() {
    let (f, fake, start) = armed();
    let app = f.host.apps.read().unwrap()[0].clone();
    *f.host.current_app.lock().unwrap() =
        Some(crate::process::RunningApp::with_environment(&app, BTreeMap::new()).unwrap());
    tick_at(&f.host, start + 11 * MINUTE);
    assert_eq!(fake.sleeps(), 0);
    let blocked = status(&f.host);
    assert_eq!(blocked.state, api::IdleSleepState::Blocked);
    assert!(blocked.reason.unwrap().contains("launched app"));
    configure(&f, &[("sleep_while_suspended", "true")]);
    tick_at(&f.host, start + 22 * MINUTE);
    assert_eq!(fake.sleeps(), 1);
    *f.host.current_app.lock().unwrap() = None;
}

#[test]
fn every_other_blocking_condition_keeps_the_pc_awake() {
    type Setup = fn(&Fixture, &Fake);
    let cases: [(&str, Setup); 8] = [
        ("a power request", |_, fake| {
            *locked(&fake.execution) = ES_SYSTEM_REQUIRED
        }),
        ("a failed helper", |_, fake| {
            *locked(&fake.input) = Input::Failed("helper".into())
        }),
        ("recent local input", |_, fake| {
            *locked(&fake.input) = Input::Idle(MINUTE)
        }),
        ("an update", |f, _| {
            f.host.updates.lock().unwrap()["phase"] = json!("downloading")
        }),
        ("Modern Standby", |_, fake| {
            locked(&fake.facts).as_mut().unwrap().modern_standby = true
        }),
        ("unreadable power facts", |_, fake| {
            *locked(&fake.facts) = Err("denied".into())
        }),
        ("a bad setting", |f, _| {
            configure(f, &[("sleep_while_suspended", "maybe")])
        }),
        ("an unreadable sleep clock", |_, fake| {
            *locked(&fake.slept) = Err("QueryUnbiasedInterruptTime failed".into())
        }),
    ];
    for (name, setup) in cases {
        let (f, fake, start) = armed();
        setup(&f, &fake);
        tick_at(&f.host, start + 11 * MINUTE);
        assert_eq!(fake.sleeps(), 0, "{name}");
    }
    // A console request restarts the interval.
    let (f, fake, start) = armed();
    tick_at(&f.host, start + 9 * MINUTE);
    *locked(&f.host.wake.console) = Some(start + 9 * MINUTE);
    tick_at(&f.host, start + 11 * MINUTE);
    assert_eq!(fake.sleeps(), 0);
    tick_at(&f.host, start + 20 * MINUTE);
    assert_eq!(fake.sleeps(), 1);
    // A launch in flight: the next tick looks again.
    let (f, fake, start) = armed();
    let transition = f.host.launch_transition.lock().unwrap();
    tick_at(&f.host, start + 11 * MINUTE);
    assert_eq!(fake.sleeps(), 0);
    drop(transition);
    tick_at(&f.host, start + 11 * MINUTE + idle::TICK);
    assert_eq!(fake.sleeps(), 1);
}

#[test]
fn what_sleep_pc_counts_as_busy_blocks_it_too() {
    // A stream still tearing down.
    let (f, fake, start) = armed();
    f.host
        .sessions
        .lock()
        .unwrap()
        .teardown
        .insert("x".into(), Instant::now());
    tick_at(&f.host, start + 11 * MINUTE);
    assert_eq!(fake.sleeps(), 0, "a teardown");
    // A WebRTC transport nobody took yet.
    let (f, fake, start) = armed();
    let (handle, _) = crate::webrtc::media::tests::real_handle(Default::default());
    f.host.webrtc_links.put("x".into(), handle);
    tick_at(&f.host, start + 11 * MINUTE);
    assert_eq!(fake.sleeps(), 0, "a WebRTC transport");
    f.host.webrtc_links.discard("x");
    // A controller session a stream still holds.
    let (f, fake, start) = armed();
    let controller = f
        .host
        .controller_sessions
        .acquire(&f.launch(f.client(0), Role::Stream), &Default::default())
        .unwrap();
    tick_at(&f.host, start + 11 * MINUTE);
    assert_eq!(fake.sleeps(), 0, "a controller session");
    drop(controller);
    // The quiet interval counts from when it let go.
    tick_at(&f.host, start + 12 * MINUTE);
    tick_at(&f.host, start + 20 * MINUTE);
    assert_eq!(fake.sleeps(), 0);
    tick_at(&f.host, start + 23 * MINUTE);
    assert_eq!(fake.sleeps(), 1);
}

#[test]
fn sleep_pc_and_sleep_after_idle_never_suspend_together() {
    let (f, fake, start) = armed();
    // Sleep PC holds the lifecycle gate from its idle check through its
    // suspend, which returns once the PC woke.
    let gate = f.host.launch_transition.lock().unwrap();
    tick_at(&f.host, start + 11 * MINUTE);
    assert_eq!(fake.sleeps(), 0);
    *locked(&fake.slept) = Ok(60 * MINUTE);
    drop(gate);
    // Back from Sleep PC's sleep: a full interval from the resume.
    tick_at(&f.host, start + 12 * MINUTE);
    tick_at(&f.host, start + 21 * MINUTE);
    assert_eq!(fake.sleeps(), 0);
    tick_at(&f.host, start + 23 * MINUTE);
    assert_eq!(fake.sleeps(), 1);
}

#[test]
fn after_a_resume_it_waits_a_full_interval_again() {
    let (f, fake, start) = armed();
    *locked(&fake.nap) = 29 * MINUTE;
    tick_at(&f.host, start + 11 * MINUTE);
    assert_eq!(fake.sleeps(), 1);
    // Back after 29 minutes: the attempt at 11 minutes is long past, but
    // the interval counts from the resume.
    tick_at(&f.host, start + 40 * MINUTE);
    tick_at(&f.host, start + 49 * MINUTE);
    assert_eq!(fake.sleeps(), 1);
    tick_at(&f.host, start + 51 * MINUTE);
    assert_eq!(fake.sleeps(), 2);
}

#[test]
fn the_last_sleep_is_when_it_went_down() {
    let (f, fake, start) = armed();
    let down = Arc::new(Mutex::new(None));
    fake.hook({
        let down = down.clone();
        move |_, method| {
            if method == "sleep" {
                *down.lock().unwrap() = Some(SystemTime::now());
                // Asleep for a while.
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    });
    tick_at(&f.host, start + 11 * MINUTE);
    let (at, error) = locked(&f.host.wake.idle).last_sleep.clone().unwrap();
    assert_eq!(error, None);
    assert!(at <= down.lock().unwrap().unwrap());
}

#[test]
fn it_checks_everything_again_before_sleeping() {
    // An app's power request comes after the first snapshot.
    let (f, fake, start) = armed();
    let reads = Arc::new(AtomicUsize::new(0));
    fake.hook({
        let reads = reads.clone();
        move |fake, method| {
            if method == "execution_state" && reads.fetch_add(1, Ordering::SeqCst) == 1 {
                *locked(&fake.execution) = ES_SYSTEM_REQUIRED;
            }
        }
    });
    tick_at(&f.host, start + 11 * MINUTE);
    assert_eq!(fake.sleeps(), 0);
    assert_eq!(reads.load(Ordering::SeqCst), 2);
    assert!(status(&f.host).reason.unwrap().contains("keeps the system"));
    *locked(&fake.execution) = 0;
    tick_at(&f.host, start + 11 * MINUTE + idle::TICK);
    assert_eq!(fake.sleeps(), 1);
    // A console request comes after the first snapshot read the console.
    let (f, fake, start) = armed();
    let host = Arc::downgrade(&f.host);
    let reads = Arc::new(AtomicUsize::new(0));
    fake.hook({
        let reads = reads.clone();
        move |_, method| {
            if method == "execution_state"
                && reads.fetch_add(1, Ordering::SeqCst) == 0
                && let Some(h) = host.upgrade()
            {
                *locked(&h.wake.console) = Some(start + 11 * MINUTE);
            }
        }
    });
    tick_at(&f.host, start + 11 * MINUTE);
    assert_eq!(fake.sleeps(), 0);
    // Seen by the recheck, not the first snapshot.
    assert_eq!(reads.load(Ordering::SeqCst), 2);
    assert_eq!(
        status(&f.host).reason.as_deref(),
        Some("waiting for console activity")
    );
}

#[test]
fn the_recheck_under_the_launch_gate_starts_no_idle_helper() {
    let (f, fake, start) = armed();
    let before = fake.inputs.load(Ordering::SeqCst);
    tick_at(&f.host, start + 11 * MINUTE);
    assert_eq!(fake.sleeps(), 1);
    // The first snapshot's reading, seconds old; a launch never waits
    // for the helper.
    assert_eq!(fake.inputs.load(Ordering::SeqCst) - before, 1);
}

/// The fix thread's result, once it is done.
fn finished(f: &Fixture) -> Result<Applied, String> {
    let deadline = Instant::now() + Duration::from_secs(20);
    while locked(&f.host.wake.fix).running {
        assert!(Instant::now() < deadline, "the fix never finished");
        std::thread::sleep(Duration::from_millis(20));
    }
    locked(&f.host.wake.fix).last.clone().unwrap().1
}

#[test]
fn the_fix_reads_without_the_launch_lock_and_applies_the_offered_plan_under_it() {
    let f = Fixture::new();
    let fake = Fake::new().on(&f.host);
    let host = Arc::downgrade(&f.host);
    let seen = Arc::new(Mutex::new(Vec::new()));
    fake.hook({
        let seen = seen.clone();
        move |_, method| {
            if let Some(h) = host.upgrade() {
                let held = h.launch_transition.try_lock().is_err();
                seen.lock().unwrap().push((method, held));
            }
        }
    });
    let plan = fix_plan(&f.host, false).unwrap();
    assert!(!plan.is_empty());
    request_fix(&f.host, &plan).unwrap();
    let applied = finished(&f).unwrap();
    assert_eq!(*locked(&fake.fixes), std::slice::from_ref(&plan));
    assert_eq!(*locked(&fake.plans), std::slice::from_ref(&plan));
    assert_eq!(applied.changes, plan.changes());
    // The console's reading, the fix's own, then the apply.
    assert_eq!(
        *seen.lock().unwrap(),
        [
            ("read_nics", false),
            ("read_nics", false),
            ("run_fix", true)
        ]
    );
}

#[test]
fn a_fix_whose_adapters_changed_since_the_offer_is_refused() {
    let f = Fixture::new();
    let fake = Fake::new().on(&f.host);
    let plan = fix_plan(&f.host, false).unwrap();
    // Set by hand in Device Manager meanwhile.
    if let Ok(nics) = locked(&fake.nics).as_mut() {
        nics.adapters[0].magic_packet_only = Some(true);
    }
    request_fix(&f.host, &plan).unwrap();
    let error = finished(&f).unwrap_err();
    assert!(
        error.contains("changed since the fix was offered"),
        "{error}"
    );
    assert!(locked(&fake.fixes).is_empty() && locked(&fake.plans).is_empty());
}

#[test]
fn a_panicking_fix_ends_with_an_error_and_poisons_no_launch() {
    let f = Fixture::new();
    let fake = Fake::new().on(&f.host);
    let plan = fix_plan(&f.host, false).unwrap();
    fake.hook(|_, method| {
        if method == "read_nics" {
            panic!("the fake NIC reading crashed");
        }
    });
    request_fix(&f.host, &plan).unwrap();
    let error = finished(&f).unwrap_err();
    assert!(error.contains("stopped unexpectedly"), "{error}");
    // Under the launch lock.
    *locked(&fake.hook) = None;
    let plan = fix_plan(&f.host, false).unwrap();
    fake.hook(|_, method| {
        if method == "run_fix" {
            panic!("the fake child crashed");
        }
    });
    request_fix(&f.host, &plan).unwrap();
    let error = finished(&f).unwrap_err();
    assert!(error.contains("panicked"), "{error}");
    assert!(!f.host.launch_transition.is_poisoned());
    assert_eq!(fix_refusal(&f.host), None);
}

#[test]
fn after_a_resume_a_connecting_client_keeps_the_pc_awake_for_a_while() {
    let f = Fixture::new();
    let fake = Fake::new().on(&f.host);
    let start = Instant::now();
    let client = |at| *locked(&f.host.wake.client) = Some(at);
    let held = || fake.held.load(Ordering::SeqCst);
    let holds = || fake.holds.load(Ordering::SeqCst);
    client(start);
    tick_at(&f.host, start);
    tick_at(&f.host, start + idle::TICK);
    assert_eq!(holds(), 0, "no resume, no grace");
    // Woken by a magic packet; the client polls.
    *locked(&fake.slept) = Ok(60 * MINUTE);
    let resume = start + 2 * MINUTE;
    client(resume);
    tick_at(&f.host, resume);
    assert_eq!((held(), holds()), (1, 1));
    tick_at(&f.host, resume + idle::TICK);
    assert_eq!((held(), holds()), (1, 1));
    // A minute without a request lets Windows' timers run again.
    tick_at(&f.host, resume + Duration::from_secs(90));
    assert_eq!(held(), 0);
    client(resume + 2 * MINUTE);
    tick_at(&f.host, resume + 2 * MINUTE);
    assert_eq!((held(), holds()), (1, 2));
    // A stream holds the PC by itself.
    let session = f.session(f.client(0), Role::Stream);
    tick_at(&f.host, resume + 3 * MINUTE);
    assert_eq!(held(), 0);
    f.host.sessions.lock().unwrap().active.clear();
    drop(session);
    client(resume + 4 * MINUTE);
    tick_at(&f.host, resume + 4 * MINUTE);
    assert_eq!(
        (held(), holds()),
        (0, 2),
        "the grace is over for this resume"
    );
}

#[test]
fn a_client_request_starts_the_grace_without_waiting_for_a_tick() {
    let f = Fixture::new();
    let fake = Fake::new().on(&f.host);
    note_client_request(&f.host);
    assert_eq!(fake.holds.load(Ordering::SeqCst), 0);
    *locked(&fake.slept) = Ok(60 * MINUTE);
    note_client_request(&f.host);
    note_client_request(&f.host);
    assert_eq!(fake.holds.load(Ordering::SeqCst), 1);
    assert_eq!(fake.held.load(Ordering::SeqCst), 1);
}

#[test]
fn the_idle_helpers_exit_code_is_seconds_or_an_error() {
    assert_eq!(helper_input(IDLE_READ), Input::Idle(Duration::ZERO));
    assert_eq!(
        helper_input(IDLE_READ | 604_800),
        Input::Idle(Duration::from_secs(604_800))
    );
    let Input::Failed(error) = helper_input(IDLE_FAILED | 5) else {
        panic!("a failure is not idle time");
    };
    assert!(error.starts_with("the idle helper failed"), "{error}");
    assert!(matches!(helper_input(IDLE_FAILED | 1), Input::Failed(_)));
    // A panic, an argument error or a crash is not recent input.
    for code in [101, 2, 0, 0xC000_0005] {
        let Input::Failed(error) = helper_input(code) else {
            panic!("exit code {code:#x} is not idle time");
        };
        assert!(error.contains("exited with code"), "{error}");
    }
}

#[test]
fn apply_settings_gives_the_script_the_exact_plan() {
    let fake = Fake::new();
    let applied = apply_settings(&*fake, true).unwrap();
    // Wired adapters only; Fast Startup is on but inactive without hibernate.
    let expected = FixPlan {
        adapters: vec![AdapterFix {
            guid: WIRED.into(),
            pnp: "PCI\\VEN_10EC&DEV_8125\\01".into(),
            name: "Ethernet".into(),
            keywords: BTreeMap::new(),
            wake_enable: false,
            magic_packet_only: true,
        }],
        fast_startup_off: false,
        skipped: Vec::new(),
    };
    assert_eq!(*locked(&fake.plans), std::slice::from_ref(&expected));
    assert_eq!(applied.changes, expected.changes());
    assert!(applied.restarted.is_empty() && applied.errors.is_empty());
    // Hibernate on: Fast Startup is active, and the keyword fix restarts the adapter.
    let fake = Fake::new();
    locked(&fake.facts).as_mut().unwrap().hibernate = true;
    if let Ok(nics) = locked(&fake.nics).as_mut() {
        nics.adapters[0]
            .keywords
            .insert("*WAKEONMAGICPACKET".into(), "0".into());
    }
    let applied = apply_settings(&*fake, true).unwrap();
    let plan = locked(&fake.plans)[0].clone();
    assert!(plan.fast_startup_off);
    assert_eq!(
        plan.adapters[0].keywords,
        BTreeMap::from([("*WakeOnMagicPacket".to_string(), "1".to_string())])
    );
    assert_eq!(applied.restarted, [WIRED]);
    assert!(
        applied
            .changes
            .contains(&"Turn off Fast Startup".to_string())
    );
    // Without --fast-startup-off it stays.
    let fake = Fake::new();
    locked(&fake.facts).as_mut().unwrap().hibernate = true;
    apply_settings(&*fake, false).unwrap();
    assert!(!locked(&fake.plans)[0].fast_startup_off);
    // Nothing to change: nothing is applied.
    let fake = Fake::new();
    if let Ok(nics) = locked(&fake.nics).as_mut() {
        nics.adapters[0].magic_packet_only = Some(true);
    }
    let applied = apply_settings(&*fake, false).unwrap();
    assert!(applied.changes.is_empty());
    assert!(locked(&fake.plans).is_empty());
    // A failed read is an error, never an empty plan.
    let fake = Fake::new();
    *locked(&fake.nics) = Err("WMI refused access".into());
    assert!(apply_settings(&*fake, false).is_err());
}

#[test]
fn the_revision_rises_only_when_the_readiness_changes() {
    let f = Fixture::new();
    let fake = Fake::new().on(&f.host);
    let local = Some("192.168.4.20".parse().unwrap());
    let first = readiness(&f.host, false, local).unwrap();
    assert_eq!(first.revision, 1);
    assert_eq!(readiness(&f.host, false, None).unwrap().revision, 1);
    assert_eq!(fake.nic_reads.load(Ordering::SeqCst), 1, "cached");
    if let Ok(nics) = locked(&fake.nics).as_mut() {
        nics.adapters[0].magic_packet_only = Some(true);
    }
    assert_eq!(readiness(&f.host, true, local).unwrap().revision, 2);
    configure(&f, &[("sleep_after_idle_minutes", "30")]);
    assert_eq!(readiness(&f.host, false, local).unwrap().revision, 3);
}

#[test]
fn loopback_and_unreadable_nics_are_reported_not_hidden() {
    let f = Fixture::new();
    let fake = Fake::new().on(&f.host);
    // `/wake` over loopback matches /serverinfo's zeros, whatever the
    // default route.
    let wake = readiness(&f.host, false, Some("127.0.0.1".parse().unwrap())).unwrap();
    assert_eq!(wake.primary, None);
    assert!(wake.targets.iter().all(|target| !target.route));
    *locked(&fake.nics) = Err("WMI refused access".into());
    let wake = readiness(&f.host, true, None).unwrap();
    assert_eq!(wake.targets.len(), 2, "virtual adapters are never listed");
    assert_eq!(wake.targets[0].state, api::TargetState::Unknown);
    *locked(&fake.adapters) = Err("IP Helper failed".into());
    assert!(readiness(&f.host, false, None).is_err());
}

#[test]
fn a_console_on_this_pc_judges_the_default_route_adapter() {
    let f = Fixture::new();
    let fake = Fake::new().on(&f.host);
    let status = |local: &str| console_status(&f.host, false, Some(local.parse().unwrap()));
    let console = |local: &str| -> api::Wake {
        serde_json::from_value(status(local).unwrap()["wake"].clone()).unwrap()
    };
    let missing = |local: &str| status(local).unwrap()["primary_missing"].clone();
    let routed = |wake: &api::Wake| -> Vec<String> {
        let targets = wake.targets.iter().filter(|target| target.route);
        targets.map(|target| target.mac.clone()).collect()
    };
    const WIRED_MAC: &str = "02:7C:16:B4:13:4F";
    // https://localhost: the Ethernet adapter carries the default route.
    for local in ["127.0.0.1", "::1", "::ffff:127.0.0.1"] {
        let wake = console(local);
        assert_eq!(wake.primary.as_deref(), Some(WIRED_MAC), "{local}");
        assert_eq!(routed(&wake), [WIRED_MAC], "{local}");
        assert_eq!(missing(local), Value::Null, "{local}");
    }
    // A request from the network is judged by its own address, not the route.
    *locked(&fake.route) = Ok(None);
    assert_eq!(routed(&console("192.168.4.20")), [WIRED_MAC]);
    assert_eq!(missing("192.168.4.20"), Value::Null);
    // A virtual adapter (a VPN) carries it: the physical one under it.
    *locked(&fake.route) = Ok(Some([172, 30, 112, 1].into()));
    let wake = console("127.0.0.1");
    assert_eq!(wake.primary.as_deref(), Some(WIRED_MAC));
    assert!(routed(&wake).is_empty());
    // An IPv6-only network: the IPv6 default route's address names it.
    let ipv6: IpAddr = "2001:db8:4::20".parse().unwrap();
    if let Ok(adapters) = locked(&fake.adapters).as_mut() {
        adapters[0].addresses.push(ipv6);
    }
    *locked(&fake.route) = Ok(Some(ipv6));
    assert_eq!(routed(&console("::1")), [WIRED_MAC]);
    // No default route, or an unreadable one: no primary, and why.
    *locked(&fake.route) = Ok(None);
    let wake = console("127.0.0.1");
    assert_eq!(wake.primary, None);
    assert!(routed(&wake).is_empty());
    assert_eq!(
        missing("127.0.0.1"),
        json!({"reason": "no_route", "error": null})
    );
    *locked(&fake.route) = Err("IP Helper failed".into());
    assert_eq!(console("127.0.0.1").primary, None);
    assert_eq!(
        missing("127.0.0.1"),
        json!({"reason": "route_error", "error": "IP Helper failed"})
    );
    // An address no physical adapter owns, and none connected: unmapped,
    // from this PC or the network.
    *locked(&fake.route) = Ok(Some([10, 8, 0, 2].into()));
    if let Ok(adapters) = locked(&fake.adapters).as_mut() {
        adapters[0].connected = false;
    }
    for local in ["127.0.0.1", "10.8.0.2"] {
        assert_eq!(console(local).primary, None, "{local}");
        assert_eq!(
            missing(local),
            json!({"reason": "unmapped", "error": null}),
            "{local}"
        );
    }
}
