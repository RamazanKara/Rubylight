use std::sync::{Arc, Mutex};

/// A shared virtual display as Windows treats it.
#[derive(Debug)]
struct Fake {
    created: Timing,
    mode: Timing,
    /// What Windows shows.
    shown: std::cell::Cell<Timing>,
    /// Off, as a paused display can be, refuses every mode.
    on: bool,
    /// Refused refreshes, after which Windows shows 60 Hz.
    refuses: Vec<u32>,
    /// The modes Windows lists, and so can set.
    listed: std::cell::RefCell<Vec<Timing>>,
    /// Offered modes Windows does not list yet.
    pending: std::cell::RefCell<Vec<Timing>>,
    /// The look at the display's modes after an offer in which Windows
    /// lists the offered modes; None never does.
    lists_after: Option<u32>,
    /// Looks at the display's modes since the last offer.
    looks: std::cell::Cell<u32>,
    /// The fastest refresh Windows lists an offered mode at, as on a PC
    /// that lists 4K only up to 240 Hz.
    fastest: u32,
    /// The driver refuses to offer a mode.
    driver_refuses: bool,
    offered: Vec<Timing>,
    log: std::cell::RefCell<Vec<String>>,
}
impl Fake {
    fn new(mode: Timing) -> Self {
        Self {
            created: mode,
            mode,
            shown: mode.into(),
            on: true,
            refuses: Vec::new(),
            listed: vec![mode].into(),
            pending: Default::default(),
            lists_after: Some(1),
            looks: 0.into(),
            fastest: u32::MAX,
            driver_refuses: false,
            offered: vec![mode],
            log: Default::default(),
        }
    }
    /// The refreshes (whole hertz) Windows lists at the mode's size.
    fn rates(&self) -> Vec<u32> {
        self.listed
            .borrow()
            .iter()
            .filter(|listed| (listed.0, listed.1) == (self.mode.0, self.mode.1))
            .map(|listed| listed.2 / 1000)
            .collect()
    }
}
impl SharedDisplay for Fake {
    fn modes(&self) -> (Timing, Timing) {
        (self.created, self.mode)
    }
    fn set_mode(&mut self, mode: Timing) {
        self.mode = mode;
    }
    fn offer_mode(&self) -> bool {
        self.log.borrow_mut().push(format!("offer {}", self.mode.2));
        if self.driver_refuses {
            return false;
        }
        self.pending.borrow_mut().push(self.mode);
        self.looks.set(0);
        true
    }
    fn offered(&mut self) -> &mut Vec<Timing> {
        &mut self.offered
    }
    fn listed_rates(&self) -> Vec<u32> {
        self.looks.set(self.looks.get() + 1);
        if self
            .lists_after
            .is_some_and(|after| self.looks.get() >= after)
        {
            let offered: Vec<_> = self.pending.borrow_mut().drain(..).collect();
            self.listed.borrow_mut().extend(
                offered
                    .into_iter()
                    .map(|(width, height, rate)| (width, height, rate.min(self.fastest))),
            );
        }
        self.rates()
    }
    fn apply(&self, stage: &str) -> bool {
        self.log.borrow_mut().push(stage.into());
        let (width, height, rate) = self.mode;
        if !self.on || self.refuses.contains(&rate) {
            self.shown.set((width, height, 60_000));
            return false;
        }
        if self.listed.borrow().contains(&self.mode) {
            self.shown.set(self.mode);
            return true;
        }
        // A rate Windows does not list is a bad mode: apply_mode keeps
        // the size at a slower listed rate, else Windows keeps its mode.
        if let Some(fallback) = super::fallback_rate(&self.rates(), rate) {
            self.shown.set((width, height, fallback));
        }
        false
    }
    fn applied_size(&self) -> Option<(u32, u32)> {
        let shown = self.shown.get();
        ((shown.0, shown.1) == (self.mode.0, self.mode.1)).then_some((shown.0, shown.1))
    }
    fn switch_on(&mut self) -> anyhow::Result<crate::display::Monitor> {
        self.on = true;
        self.log.borrow_mut().push("on".into());
        Ok(super::super::monitor("virtual", 1, false))
    }
}
/// Lease `id` for a stream at `requested`, creating `fresh` when nothing has it.
fn lease(
    displays: &mut super::Leases<Fake>,
    requested: Timing,
    fresh: Option<Fake>,
) -> anyhow::Result<(Arc<Mutex<Fake>>, Option<u32>)> {
    super::lease(displays, "shared", requested, || {
        Ok(fresh.expect("a display was created"))
    })
}
/// Lease and switch on, as a stream's display guard does.
fn stream(
    displays: &mut super::Leases<Fake>,
    requested: Timing,
    fresh: Option<Fake>,
) -> anyhow::Result<Arc<Mutex<Fake>>> {
    let (display, needed) = lease(displays, requested, fresh)?;
    super::joined_monitor(&mut *display.lock().unwrap(), needed)?;
    Ok(display)
}

#[test]
fn a_second_stream_joins_a_shared_display_whose_mode_changed_in_place() {
    let launched = (1920, 1080, 60_000);
    // The first stream raised the rate, turned, or Windows took a size
    // the user picked: the launch mode and the shown one both join.
    for current in [
        (1920, 1080, 120_000),
        (1080, 1920, 60_000),
        (2560, 1440, 60_000),
    ] {
        assert_eq!(super::join(launched, current, launched).unwrap(), None);
        assert_eq!(super::join(launched, current, current).unwrap(), None);
        let error = super::join(launched, current, (1280, 720, 60_000))
            .unwrap_err()
            .to_string();
        assert!(error.contains("asks for 1280x720"), "{error}");
    }
    assert_eq!(super::join(launched, launched, launched).unwrap(), None);
}

#[test]
fn a_vrr_stream_and_a_fixed_rate_stream_share_a_virtual_display_either_way() {
    // Nightjar desktop with VRR asks 1000 Hz, a phone at 60 fps 120 Hz.
    let vrr = (1920, 1080, 1_000_000);
    let fixed = (1920, 1080, 120_000);
    // VRR first: its 1000 Hz serves the 60 fps stream as it is.
    let mut displays = super::Leases::new();
    let first = stream(&mut displays, vrr, Some(Fake::new(vrr))).unwrap();
    let second = stream(&mut displays, fixed, None).unwrap();
    assert!(Arc::ptr_eq(&first, &second));
    assert_eq!(first.lock().unwrap().shown.get(), vrr);
    // Only switched on, by each stream: no mode was set.
    assert_eq!(*first.lock().unwrap().log.borrow(), ["on", "on"]);
    // The fixed rate first: the VRR stream raises the display, and keeps VRR.
    let mut displays = super::Leases::new();
    let first = stream(&mut displays, fixed, Some(Fake::new(fixed))).unwrap();
    let second = stream(&mut displays, vrr, None).unwrap();
    assert!(Arc::ptr_eq(&first, &second));
    assert_eq!(first.lock().unwrap().mode, vrr);
    assert_eq!(first.lock().unwrap().shown.get(), vrr);
    // Now at 1000 Hz, a third stream at either rate joins as it is.
    assert_eq!(lease(&mut displays, fixed, None).unwrap().1, None);
    assert_eq!(lease(&mut displays, vrr, None).unwrap().1, None);
    // Once every stream left, the next one gets a display of its own.
    drop((first, second));
    let fresh = stream(&mut displays, fixed, Some(Fake::new(fixed))).unwrap();
    assert_eq!(fresh.lock().unwrap().mode, fixed);
}

#[test]
fn a_shared_virtual_display_that_cannot_serve_a_stream_refuses_it_with_why() {
    let fixed = (1920, 1080, 120_000);
    let mut displays = super::Leases::new();
    let first = stream(&mut displays, fixed, Some(Fake::new(fixed))).unwrap();
    // Another size, even at a rate the display serves.
    let error = lease(&mut displays, (2560, 1440, 60_000), None)
        .unwrap_err()
        .to_string();
    assert_eq!(
        error,
        "shared virtual display already uses a different mode: it is 1920x1080, and this stream asks for 2560x1440"
    );
    // A refresh Windows refuses to raise the display to.
    first.lock().unwrap().refuses.push(1_000_000);
    let error = stream(&mut displays, (1920, 1080, 1_000_000), None)
        .unwrap_err()
        .to_string();
    assert_eq!(
        error,
        "shared virtual display already uses a different mode: it runs at 120.000 Hz, and Windows refused the 1000.000 Hz this stream asks for"
    );
    // Windows left 60 Hz behind: the stream already on it gets its mode back.
    let display = first.lock().unwrap();
    assert_eq!(display.mode, fixed);
    assert_eq!(display.shown.get(), fixed);
    assert_eq!(
        display.log.borrow().last().map(String::as_str),
        Some("stream raise refused")
    );
}

#[test]
fn a_stream_that_asks_for_more_than_its_launch_raises_a_shared_display_too() {
    // Two streams launched at 60 fps on one display; one asks for 240 Hz
    // at stream start.
    let fixed = (1920, 1080, 120_000);
    let mut displays = super::Leases::new();
    let first = stream(&mut displays, fixed, Some(Fake::new(fixed))).unwrap();
    let second = stream(&mut displays, fixed, None).unwrap();
    assert_eq!(super::raise_lease(&second, 240_000), Some(240_000));
    assert_eq!(first.lock().unwrap().shown.get(), (1920, 1080, 240_000));
    // Asking for less leaves the faster refresh; a refusal puts it back.
    assert_eq!(super::raise_lease(&first, 120_000), Some(240_000));
    first.lock().unwrap().refuses.push(1_000_000);
    assert_eq!(super::raise_lease(&first, 1_000_000), None);
    assert_eq!(second.lock().unwrap().shown.get(), (1920, 1080, 240_000));
}

#[test]
fn a_display_switched_on_under_a_new_name_is_a_new_generation() {
    let (mut name, mut generation) = (String::from(r"\\.\DISPLAY3"), 4);
    // The same name: the paused stream's guard keeps its target.
    super::take_name(&mut name, &mut generation, r"\\.\DISPLAY3");
    assert_eq!((name.as_str(), generation), (r"\\.\DISPLAY3", 4));
    // A new one: its guard sees another generation and moves there.
    super::take_name(&mut name, &mut generation, r"\\.\DISPLAY5");
    assert_eq!((name.as_str(), generation), (r"\\.\DISPLAY5", 5));
}

#[test]
fn a_stream_raises_a_paused_shared_display_only_once_it_is_on_again() {
    // The VRR device relaunches onto the display a disconnected phone
    // left paused, and Windows switched it off meanwhile.
    let fixed = (1920, 1080, 120_000);
    let mut displays = super::Leases::new();
    let paused = stream(&mut displays, fixed, Some(Fake::new(fixed))).unwrap();
    {
        let mut display = paused.lock().unwrap();
        display.on = false;
        display.log.borrow_mut().clear();
    }
    stream(&mut displays, (1920, 1080, 1_000_000), None).unwrap();
    let display = paused.lock().unwrap();
    assert_eq!(
        *display.log.borrow(),
        ["on", "offer 1000000", "stream request"]
    );
    assert_eq!(display.shown.get(), (1920, 1080, 1_000_000));
}

#[test]
fn a_joining_stream_raises_a_shared_display_once_windows_lists_the_rate() {
    // S1a order B: the Retroid at 120 Hz first, then the VRR laptop at
    // 1000 Hz. The driver published the 1000 Hz list 16 ms after the host
    // set it, and Windows refused it as a bad mode.
    let fixed = (1920, 1080, 120_000);
    let vrr = (1920, 1080, 1_000_000);
    let mut displays = super::Leases::new();
    let first = stream(&mut displays, fixed, Some(Fake::new(fixed))).unwrap();
    {
        let mut display = first.lock().unwrap();
        display.lists_after = Some(4);
        display.log.borrow_mut().clear();
    }
    let second = stream(&mut displays, vrr, None).unwrap();
    let display = second.lock().unwrap();
    // Set only in the fourth look, once Windows listed it.
    assert_eq!(display.looks.get(), 4);
    assert_eq!(
        *display.log.borrow(),
        ["on", "offer 1000000", "stream request"]
    );
    assert_eq!(display.shown.get(), vrr);
}

#[test]
fn a_mode_the_driver_refused_to_offer_is_not_waited_for() {
    let fixed = (1920, 1080, 120_000);
    let mut displays = super::Leases::new();
    let first = stream(&mut displays, fixed, Some(Fake::new(fixed))).unwrap();
    {
        let mut display = first.lock().unwrap();
        display.driver_refuses = true;
        display.log.borrow_mut().clear();
    }
    let started = Instant::now();
    assert_eq!(super::raise_lease(&first, 1_000_000), None);
    assert!(started.elapsed() < Duration::from_secs(1));
    {
        let display = first.lock().unwrap();
        assert_eq!(display.looks.get(), 1);
        assert_eq!(
            *display.log.borrow(),
            ["offer 1000000", "stream raise refused"]
        );
        assert_eq!(display.shown.get(), fixed);
    }
    // Windows lists it already: the raise needs no new list.
    first
        .lock()
        .unwrap()
        .listed
        .borrow_mut()
        .push((1920, 1080, 240_000));
    assert_eq!(super::raise_lease(&first, 240_000), Some(240_000));
    assert_eq!(first.lock().unwrap().shown.get(), (1920, 1080, 240_000));
}

#[test]
fn a_turned_stream_keeps_its_size_at_a_slower_rate_windows_lists() {
    // A VRR stream at 4K and 1000 Hz turns to portrait on a PC that lists
    // 4K only up to 240 Hz. Windows lists the turned size in the third
    // look; the turn applies there at 240 Hz (issue #40).
    let landscape = (3840, 2160, 1_000_000);
    let mut display = Fake::new(landscape);
    display.fastest = 240_000;
    display.lists_after = Some(3);
    assert!(super::reshape(&mut display, 2160, 3840));
    assert_eq!(display.looks.get(), 3);
    assert_eq!(*display.log.borrow(), ["offer 1000000", "stream turned"]);
    assert_eq!(display.shown.get(), (2160, 3840, 240_000));
    assert_eq!(display.mode, (2160, 3840, 1_000_000));
    // Turned back: the landscape size is listed, so one look serves it.
    display.log.borrow_mut().clear();
    assert!(super::reshape(&mut display, 3840, 2160));
    assert_eq!(display.looks.get(), 1);
    assert_eq!(display.shown.get(), landscape);
}

#[test]
fn a_turn_to_a_size_windows_lists_at_other_rates_waits_for_its_own() {
    // A portrait VRR stream turns to 1920x1080, which the driver lists at
    // 60 and 120 Hz from the start: those are not the turn's list.
    let portrait = (1080, 1920, 1_000_000);
    let landscape = (1920, 1080, 1_000_000);
    let standard = |display: &Fake| {
        display
            .listed
            .borrow_mut()
            .extend([(1920, 1080, 60_000), (1920, 1080, 120_000)]);
    };
    let mut display = Fake::new(portrait);
    standard(&display);
    display.lists_after = Some(3);
    assert!(super::reshape(&mut display, 1920, 1080));
    assert_eq!(display.looks.get(), 3);
    assert_eq!(display.shown.get(), landscape);
    // Where Windows lists the size only up to 480 Hz, the new list is
    // the turn's too, and the turn takes 480 Hz (issue #40).
    let mut display = Fake::new(portrait);
    standard(&display);
    display.lists_after = Some(3);
    display.fastest = 480_000;
    assert!(super::reshape(&mut display, 1920, 1080));
    assert_eq!(display.looks.get(), 3);
    assert_eq!(display.shown.get(), (1920, 1080, 480_000));
    // A list that never changes: after the wait the turn takes 120 Hz.
    let mut display = Fake::new(portrait);
    standard(&display);
    display.lists_after = None;
    assert!(super::reshape(&mut display, 1920, 1080));
    assert_eq!(display.shown.get(), (1920, 1080, 120_000));
    // Turned back and again: the driver offered both modes already, so
    // neither turn waits, and the fallback serves the filtered one.
    for (width, height, shown) in [(1080, 1920, portrait), (1920, 1080, (1920, 1080, 120_000))] {
        let started = Instant::now();
        assert!(super::reshape(&mut display, width, height));
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(display.looks.get(), 1);
        assert_eq!(display.shown.get(), shown);
    }
    assert_eq!(display.offered, [portrait, landscape]);
}

#[test]
fn a_rate_windows_never_lists_is_not_set_and_the_stream_is_refused_with_why() {
    let fixed = (1920, 1080, 120_000);
    let mut displays = super::Leases::new();
    let first = stream(&mut displays, fixed, Some(Fake::new(fixed))).unwrap();
    {
        let mut display = first.lock().unwrap();
        display.lists_after = None;
        display.log.borrow_mut().clear();
    }
    let error = stream(&mut displays, (1920, 1080, 1_000_000), None)
        .unwrap_err()
        .to_string();
    assert_eq!(
        error,
        "shared virtual display already uses a different mode: it runs at 120.000 Hz, and Windows refused the 1000.000 Hz this stream asks for"
    );
    let display = first.lock().unwrap();
    // The unlisted rate was never set; the stream on it keeps its mode.
    assert!(display.looks.get() > 1);
    assert_eq!(
        *display.log.borrow(),
        ["on", "offer 1000000", "stream raise refused"]
    );
    assert_eq!(display.mode, fixed);
    assert_eq!(display.shown.get(), fixed);
}

#[test]
fn waiting_for_a_listed_mode_costs_one_look_when_it_is_listed_and_stops_in_time() {
    assert_eq!(super::wait_listed(|| true, MODE_LIST_WAIT), Some(1));
    let mut looks = 0;
    let polls = super::wait_listed(
        || {
            looks += 1;
            looks == 3
        },
        MODE_LIST_WAIT,
    );
    assert_eq!(polls, Some(3));
    let started = Instant::now();
    assert_eq!(
        super::wait_listed(|| false, Duration::from_millis(30)),
        None
    );
    let waited = started.elapsed();
    assert!(waited >= Duration::from_millis(30), "{waited:?}");
    assert!(waited < Duration::from_secs(1), "{waited:?}");
}

#[test]
fn the_console_names_why_setup_did_not_install_the_driver() {
    use windows::Win32::Foundation::ERROR_NO_MORE_ITEMS;
    let missing = anyhow::Error::new(windows::core::Error::from(ERROR_NO_MORE_ITEMS.to_hresult()));
    let refused = "Smart App Control is on, and it refuses a display driver signed on this PC, so streams use a physical display.";
    assert_eq!(super::problem_text(&missing, Some(refused.into())), refused);
    // Nothing recorded, or an empty value: the error explains it.
    for recorded in [None, Some(" ".into())] {
        assert!(
            super::problem_text(&missing, recorded)
                .starts_with("Rubylight's display driver is not installed")
        );
    }
}
use super::super::modes::timing_matches;
use super::*;
#[test]
fn a_busy_driver_request_recovers_within_the_same_call() {
    let mut attempts = 0;
    let result = retry_busy(
        || {
            attempts += 1;
            if attempts < 3 {
                Err(windows::core::Error::from_hresult(ERROR_BUSY.to_hresult()).into())
            } else {
                Ok(vec![17, 23])
            }
        },
        Duration::from_secs(1),
    )
    .unwrap();
    assert_eq!(result, [17, 23]);
    assert_eq!(attempts, 3);
}
#[test]
fn driver_retries_stop_at_the_deadline_and_preserve_other_errors() {
    for code in [
        ERROR_BUSY,
        ERROR_ACCESS_DENIED,
        ERROR_NOT_FOUND,
        ERROR_GEN_FAILURE,
    ] {
        let mut attempts = 0;
        let timeout = if code == ERROR_BUSY {
            Duration::ZERO
        } else {
            Duration::from_secs(1)
        };
        let error = retry_busy::<()>(
            || {
                attempts += 1;
                Err(windows::core::Error::from_hresult(code.to_hresult()).into())
            },
            timeout,
        )
        .unwrap_err();
        assert_eq!(attempts, 1);
        assert_eq!(
            error.downcast_ref::<windows::core::Error>().unwrap().code(),
            code.to_hresult()
        );
    }
}
#[test]
fn an_owned_monitor_can_reappear_during_session_handover() {
    let mut attempts = 0;
    let found = wait_for_monitor(
        || {
            attempts += 1;
            Ok((attempts == 3).then(|| super::super::monitor("owned", 4, false)))
        },
        || Ok(()),
        Duration::from_secs(1),
    )
    .unwrap();
    assert_eq!(found.device_id, "owned");
    assert_eq!(attempts, 3);
}
#[test]
fn startup_recovers_a_connected_monitor_instead_of_only_polling_it() {
    let renewals = std::cell::Cell::new(0);
    let found = wait_for_monitor(
        || Ok((renewals.get() == 2).then(|| super::super::monitor("owned", 4, false))),
        || {
            renewals.set(renewals.get() + 1);
            Ok(())
        },
        Duration::from_secs(2),
    )
    .unwrap();
    assert_eq!(found.device_id, "owned");
    assert_eq!(renewals.get(), 2);
}
#[test]
fn startup_does_not_reactivate_a_present_monitor_or_hide_a_lease_error() {
    assert!(
        wait_for_monitor(
            || Ok(Some(super::super::monitor("owned", 4, false))),
            || panic!("an active display needs no recovery"),
            Duration::ZERO,
        )
        .is_ok()
    );
    let error = wait_for_monitor(
        || Ok(None),
        || Err(windows::core::Error::from_hresult(ERROR_ACCESS_DENIED.to_hresult()).into()),
        Duration::from_secs(1),
    )
    .err()
    .unwrap();
    assert_eq!(
        error.downcast_ref::<windows::core::Error>().unwrap().code(),
        ERROR_ACCESS_DENIED.to_hresult()
    );
}
#[test]
fn an_absent_owned_monitor_or_failed_query_does_not_wait_forever() {
    let mut attempts = 0;
    assert!(
        wait_for_monitor(
            || {
                attempts += 1;
                Ok(None)
            },
            || panic!("expired lookup must not change the topology"),
            Duration::ZERO,
        )
        .is_err()
    );
    assert_eq!(attempts, 1);
    let error = wait_for_monitor(
        || Err(windows::core::Error::from_hresult(ERROR_ACCESS_DENIED.to_hresult()).into()),
        || panic!("failed query must not change the topology"),
        Duration::from_secs(1),
    )
    .err()
    .unwrap();
    assert_eq!(
        error.downcast_ref::<windows::core::Error>().unwrap().code(),
        ERROR_ACCESS_DENIED.to_hresult()
    );
}
#[test]
fn only_a_missing_driver_lease_allows_recreation() {
    assert!(lease_exists(Ok(())).unwrap());
    for code in [
        ERROR_BUSY,
        ERROR_RETRY,
        ERROR_GEN_FAILURE,
        ERROR_ACCESS_DENIED,
        ERROR_INVALID_HANDLE,
    ] {
        let error = windows::core::Error::from_hresult(windows::core::HRESULT::from_win32(code.0));
        assert!(lease_exists(Err(error.into())).is_err(), "{code:?}");
        assert!(
            lease_exists(Ok(())).unwrap(),
            "the next heartbeat can succeed"
        );
    }
    let error =
        windows::core::Error::from_hresult(windows::core::HRESULT::from_win32(ERROR_NOT_FOUND.0));
    assert!(!lease_exists(Err(error.into())).unwrap());
}
#[test]
fn heartbeat_recreates_only_a_missing_lease_and_switches_on_a_display_windows_turned_off() {
    let failure = |code: WIN32_ERROR| -> anyhow::Error {
        windows::core::Error::from_hresult(windows::core::HRESULT::from_win32(code.0)).into()
    };
    let busy = || lease_exists(Err(failure(ERROR_BUSY)));
    let missing = || lease_exists(Err(failure(ERROR_NOT_FOUND)));
    // The log this guards against: the lease renewed, Windows had switched
    // the display off, and a duplicate create returned ERROR_BUSY.
    assert_eq!(
        heartbeat(
            Ok(true),
            true,
            || Ok(true),
            || panic!("present needs no relisting lookup"),
            || panic!("present needs no full query")
        )
        .unwrap(),
        Heartbeat::Present
    );
    assert_eq!(
        heartbeat(Ok(true), true, || Ok(false), || Ok(false), || Ok(true)).unwrap(),
        Heartbeat::SwitchOn
    );
    // Issue 56: Windows showed the display under another adapter and
    // target, its old route stayed listed but unavailable, and switching
    // that route on failed every second. A display Windows shows is
    // followed, never switched on.
    assert_eq!(
        heartbeat(
            Ok(true),
            true,
            || Ok(false),
            || Ok(true),
            || panic!("a relisted display is not switched on")
        )
        .unwrap(),
        Heartbeat::Relisted
    );
    // Not yet shown anywhere, or a query failed: retry, never create.
    assert!(heartbeat(Ok(true), true, || Ok(false), || Ok(false), || Ok(false)).is_err());
    assert!(
        heartbeat(
            Ok(true),
            true,
            || Err(failure(ERROR_BUSY)),
            || panic!("no relisting lookup"),
            || panic!("no full query")
        )
        .is_err()
    );
    assert!(
        heartbeat(
            Ok(true),
            true,
            || Ok(false),
            || Err(failure(ERROR_BUSY)),
            || panic!("no full query")
        )
        .is_err()
    );
    assert!(
        heartbeat(
            Ok(true),
            true,
            || Ok(false),
            || Ok(false),
            || Err(failure(ERROR_BUSY))
        )
        .is_err()
    );
    // A renewal failure other than a missing lease is retried as well,
    // without touching the topology.
    assert!(
        heartbeat(
            busy(),
            true,
            || panic!("no topology query"),
            || panic!("no topology query"),
            || panic!("no topology query")
        )
        .is_err()
    );
    // A lease the driver no longer knows is a real loss: recreate at once,
    // whatever Windows still lists.
    assert_eq!(
        heartbeat(
            missing(),
            true,
            || panic!("no topology query"),
            || panic!("no topology query"),
            || panic!("no topology query")
        )
        .unwrap(),
        Heartbeat::Recreate
    );
}
#[test]
fn a_paused_heartbeat_keeps_local_layout_changes_and_defers_recreation() {
    // A Win+P layout can leave our display connected but inactive; a
    // driver restart can remove its lease. Neither permits a paused game
    // to change the desktop. No topology lookup is needed in either case.
    for exists in [true, false] {
        assert_eq!(
            heartbeat(
                Ok(exists),
                false,
                || panic!("paused renewal must not query the desktop"),
                || panic!("paused renewal must not look the display up"),
                || panic!("paused renewal must not query connected displays"),
            )
            .unwrap(),
            Heartbeat::Keep
        );
        assert_eq!(
            heartbeat(Ok(exists), true, || Ok(false), || Ok(false), || Ok(true)).unwrap(),
            if exists {
                Heartbeat::SwitchOn
            } else {
                Heartbeat::Recreate
            },
            "an active stream or resume still repairs the display"
        );
    }
    let error = heartbeat(
        Err(windows::core::Error::from_hresult(ERROR_ACCESS_DENIED.to_hresult()).into()),
        false,
        || panic!("no topology query"),
        || panic!("no topology query"),
        || panic!("no topology query"),
    )
    .unwrap_err();
    assert_eq!(
        error.downcast_ref::<windows::core::Error>().unwrap().code(),
        ERROR_ACCESS_DENIED.to_hresult(),
        "paused renewal must not hide transport failures"
    );
}
#[test]
#[ignore = "creates a virtual display and briefly switches it off"]
fn native_virtual_display_left_off_by_windows_is_switched_on_without_retiming_others() -> Result<()>
{
    let mut display = VirtualDisplay::create(
        &format!("activation-test-{}", std::process::id()),
        1280,
        720,
        60,
    )?;
    let owned = display
        .resolved_target
        .clone()
        .context("virtual display unresolved")?;
    let routes = display.routes()?;
    let ours = |m: &Monitor| routes.contains(&(m.adapter, m.target));
    // Device, width, height, refresh and desktop position.
    type Timing = (String, u32, u32, u32, i32, i32);
    let timings = || -> Result<Vec<Timing>> {
        monitors()?
            .iter()
            .filter(|m| !ours(m))
            .map(|m| {
                let mode = mode(&m.display_name)?;
                // SAFETY: mode() returns a display DEVMODEW with its display position union field initialized.
                let position = unsafe { mode.Anonymous1.Anonymous2.dmPosition };
                Ok((
                    m.device_id.clone(),
                    mode.dmPelsWidth,
                    mode.dmPelsHeight,
                    mode.dmDisplayFrequency,
                    position.x,
                    position.y,
                ))
            })
            .collect()
    };
    let before = timings()?;
    // What Windows does when a saved layout leaves the new display off.
    let active = Topology::query()?;
    let off: Vec<_> = active
        .paths
        .iter()
        .filter(|p| !routes.contains(&(p.targetInfo.adapterId, p.targetInfo.id)))
        .copied()
        .collect();
    // SAFETY: The retained paths and queried modes remain valid for this synchronous display configuration call.
    check(unsafe {
        SetDisplayConfig(
            Some(&off),
            Some(&active.modes),
            SDC_APPLY | SDC_USE_SUPPLIED_DISPLAY_CONFIG | SDC_ALLOW_CHANGES,
        )
    })?;
    anyhow::ensure!(
        !monitors()?.iter().any(ours),
        "virtual display stayed on after switching it off"
    );
    let recovered = display.current_monitor()?;
    assert!(display.owns_monitor(&recovered));
    assert!(display.switched_on);
    assert_eq!(display.generation, 0);
    assert_eq!(timings()?, before);
    // Exercise the established stream's heartbeat separately from startup.
    // SAFETY: The same retained paths and queried modes remain valid here.
    check(unsafe {
        SetDisplayConfig(
            Some(&off),
            Some(&active.modes),
            SDC_APPLY | SDC_USE_SUPPLIED_DISPLAY_CONFIG | SDC_ALLOW_CHANGES,
        )
    })?;
    display.last_feed = Instant::now() - Duration::from_secs(2);
    display.feed()?;
    anyhow::ensure!(monitors()?.iter().any(ours), "virtual display stayed off");
    assert_eq!(display.generation, 1);
    assert!(display.owns_monitor(&owned));
    assert_eq!(timings()?, before);
    println!(
        "switched on {} beside {} displays",
        display.name,
        before.len()
    );
    Ok(())
}
#[test]
#[ignore = "creates and expires owned 4K virtual displays and changes their mode and HDR"]
fn native_virtual_display_recovery_restores_the_requested_mode_and_mode_list() -> Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_ansi(false)
        .try_init();
    let before = Snapshot::capture()?;
    let result = (|| -> Result<()> {
        for rate in [116_000, 1_000_000] {
            let mut display = VirtualDisplay::create_rate(
                &format!("mode-recovery-test-{}-{rate}", std::process::id()),
                3840,
                2160,
                rate,
            )
            .with_context(|| format!("create 4K{}", rate / 1000))?;
            display
                .finish_hotplug("test startup")
                .with_context(|| format!("settle 4K{}", rate / 1000))?;
            let supported = supported_modes(&display.name);
            anyhow::ensure!(supported.contains(&(3840, 2160, rate / 1000)));
            anyhow::ensure!(supported.contains(&(1920, 1080, 60)));
            // The driver's EDID prefers 4K60; creation must set the
            // stream's rate rather than leave Windows' choice.
            let created = current_timing(&display.name)?;
            anyhow::ensure!(
                timing_matches((3840, 2160, rate), created),
                "created at {}",
                format_timing(created)
            );
            Topology::set_mode_rate(
                &display.name,
                2560,
                1440,
                butterpollo_core::framegen::Rate(60_000),
            )?;
            // A healthy heartbeat must allow a game's own mode change.
            display.last_feed = Instant::now() - Duration::from_secs(2);
            display.feed()?;
            anyhow::ensure!(mode(&display.name)?.dmPelsWidth == 2560);
            anyhow::ensure!(display.generation == 0);

            // What a driver restart does: the monitor departs under a live stream.
            display.driver.remove(display.id)?;
            let deadline = Instant::now() + Duration::from_secs(20);
            while display.generation == 0 {
                let recovered = display.feed();
                if Instant::now() >= deadline {
                    recovered?;
                    bail!("owned virtual display did not recover");
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            let owned = display.hotplug_monitor()?.clone();
            let routes = display.routes()?;
            set_hdr(&owned, true)?;
            display
                .finish_hotplug("test recovery after HDR")
                .with_context(|| format!("finish recovered 4K{}", rate / 1000))?;
            let actual = current_timing(&display.name)?;
            anyhow::ensure!(
                timing_matches((3840, 2160, rate), actual),
                "recovered at {}",
                format_timing(actual)
            );
            let recovered = supported_modes(&display.name);
            anyhow::ensure!(
                supported.iter().all(|mode| recovered.contains(mode)),
                "recovery restricted the mode list"
            );
            println!(
                "recovered {} at 3840x2160@{} with {} modes after a 2560x1440@60 recall",
                display.name,
                rate / 1000,
                recovered.len()
            );
            drop(display);
            let deadline = Instant::now() + Duration::from_secs(10);
            while Topology::query_all()?.paths.iter().any(|path| {
                routes.contains(&(path.targetInfo.adapterId, path.targetInfo.id))
                    && path.targetInfo.targetAvailable.as_bool()
            }) {
                anyhow::ensure!(Instant::now() < deadline, "owned display did not depart");
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        Ok(())
    })();
    before.restore()?;
    result?;
    anyhow::ensure!(
        serde_json::to_value(Snapshot::capture()?)? == serde_json::to_value(before)?,
        "the pre-test layout was not restored"
    );
    Ok(())
}
#[test]
fn a_vrr_display_requests_1000_hz_in_the_drivers_millihertz_field() {
    for rate in [1_000_000, 116_000, 59_940] {
        let request = rubylight_vdd::add_request(7, (3840, 2160, rate), 1000);
        assert_eq!(
            (request.width, request.height, request.refresh_mhz),
            (3840, 2160, rate)
        );
        assert!(request.validate().is_ok());
    }
    // Windows takes the mode as a reduced rational.
    assert_eq!(
        butterpollo_core::framegen::Rate(1_000_000).rational(),
        (1000, 1)
    );
    assert_eq!(
        butterpollo_core::framegen::Rate(116_000).rational(),
        (116, 1)
    );
    assert_eq!(
        butterpollo_core::framegen::Rate(59_940).rational(),
        (2997, 50)
    );
}
#[test]
fn a_selected_gpu_renders_the_virtual_display_and_automatic_leaves_it_to_windows() {
    // Issue #28: the integrated GPU rendered the virtual display while
    // the selected discrete Radeon was asked to duplicate it.
    assert_eq!(VirtualOptions::default().render_adapter, None);
    use butterpollo_core::config::Config;
    assert_eq!(configured_render_adapter(&Config::default()), None);
    // A GPU that is gone leaves the choice to Windows, never another GPU.
    let missing = Config::parse("adapter_name = No Such GPU\n").unwrap();
    assert_eq!(configured_render_adapter(&missing), None);
}
#[test]
fn a_display_windows_lists_under_the_render_adapter_is_still_ours() {
    // From the Ryzen 7 8845HS laptop: the driver reported the IddCx adapter
    // and target 258, and Windows listed the display on the Radeon 780M
    // under target 117440770, with the driver's target id as the UID.
    let luid = |low| LUID {
        LowPart: low,
        HighPart: 0,
    };
    let listing = |path: &str, adapter: u32, target: u32| Monitor {
        monitor_device_path: path.into(),
        adapter: luid(adapter),
        target,
        ..crate::display::monitor("m", adapter, false)
    };
    let panel = listing(
        r"\\?\DISPLAY#LEN8ABA#5&277e72b4&0&UID256#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}",
        0x10687,
        256,
    );
    let ours = listing(
        r"\\?\DISPLAY#RBY0001#1&28a6823a&0&UID258#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}",
        0x10687,
        117_440_770,
    );
    let found = |monitors: &[Monitor], target| {
        listed(monitors, luid(942_669_753), target, &[]).map(|m| m.target)
    };
    assert_eq!(
        found(&[panel.clone(), ours.clone()], 258),
        Some(117_440_770)
    );
    // The UID is the driver's target id: another id is another display, and
    // the panel is never taken for it.
    assert_eq!(found(&[panel.clone(), ours.clone()], 259), None);
    assert_eq!(found(&[panel], 256), None);
    // The route the driver reported wins when Windows lists it there.
    let direct = listing(&ours.monitor_device_path, 942_669_753, 258);
    assert_eq!(found(&[ours.clone(), direct], 258), Some(258));
    // A route without a device path cannot identify the display yet.
    let unnamed = listing("", 942_669_753, 258);
    assert_eq!(found(std::slice::from_ref(&unnamed), 258), None);
    assert_eq!(found(&[unnamed, ours.clone()], 258), Some(117_440_770));
    // The pair another session's display goes by is never taken.
    let taken = [((0x10687, 0), 117_440_770)];
    assert_eq!(
        listed(std::slice::from_ref(&ours), luid(942_669_753), 258, &taken).map(|m| m.target),
        None
    );
}
#[test]
fn a_display_is_followed_to_a_new_pair_by_its_device_path() {
    // Windows listed it at 117440770 before it was switched off and at
    // 117440783 after; the device path is the same.
    let listing = |path: &str, target| Monitor {
        monitor_device_path: path.into(),
        target,
        ..crate::display::monitor("m", 0x10687, false)
    };
    let path = r"\\?\DISPLAY#RBY0001#1&28a6823a&0&UID258#{g}";
    let panel = listing(r"\\?\DISPLAY#LEN8ABA#5&277e72b4&0&UID256#{g}", 256);
    let moved = listing(&path.to_lowercase(), 117_440_783);
    let found = listed_by_path(vec![panel.clone(), moved], path);
    assert_eq!(found.map(|m| m.target), Some(117_440_783));
    // Another display, or no path at all, is never taken for it.
    assert!(listed_by_path(vec![panel], path).is_none());
    assert!(listed_by_path(vec![listing("", 5)], "").is_none());
}
#[test]
fn a_display_that_stays_duplicated_is_refused_with_the_reason() {
    let message = still_duplicated("no free desktop source for the virtual display");
    assert!(message.contains("duplicated with a physical display"));
    assert!(message.contains("no free desktop source"));
}
#[test]
fn a_display_on_the_panels_source_is_duplicated_and_not_ready() {
    let listing = |path: &str, target, source| Monitor {
        monitor_device_path: path.into(),
        target,
        source,
        ..crate::display::monitor("m", 0x10687, false)
    };
    let panel = listing(r"\\?\DISPLAY#LEN8ABA#5&277e72b4&0&UID256#{g}", 256, 0);
    let ours = listing(
        r"\\?\DISPLAY#RBY0001#1&28a6823a&0&UID258#{g}",
        117_440_770,
        0,
    );
    let other_virtual = listing(
        r"\\?\DISPLAY#RBY0001#1&28a6823a&0&UID259#{g}",
        117_440_771,
        0,
    );
    assert!(cloned_with_physical(&[panel.clone(), ours.clone()], &ours));
    // Alone, on a source of its own, or duplicated with a virtual display only.
    assert!(!cloned_with_physical(std::slice::from_ref(&ours), &ours));
    let own_source = Monitor {
        source: 1,
        ..ours.clone()
    };
    assert!(!cloned_with_physical(
        &[panel.clone(), own_source.clone()],
        &own_source
    ));
    assert!(!cloned_with_physical(&[other_virtual, ours.clone()], &ours));
    // Another adapter's source 0 is another desktop source.
    let elsewhere = Monitor {
        adapter: LUID {
            LowPart: 1,
            HighPart: 0,
        },
        ..panel
    };
    assert!(!cloned_with_physical(&[elsewhere, ours.clone()], &ours));
}
#[test]
fn virtual_monitor_paths_name_the_driver_target_in_their_uid() {
    use super::super::topology::is_virtual_monitor;
    let path = r"\\?\DISPLAY#RBY0001#1&28a6823a&0&UID258#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}";
    assert!(is_virtual_monitor(path, 258));
    assert!(is_virtual_monitor(&path.replace("RBY", "rby"), 258));
    assert!(!is_virtual_monitor(path, 25), "a prefix of the id");
    assert!(!is_virtual_monitor(path, 58), "a suffix of the id");
    assert!(is_virtual_monitor(&path.replace("UID", "uid"), 258));
    let longer = path.replace("UID258", "UID2580");
    assert!(!is_virtual_monitor(&longer, 258), "UID2580 is not 258");
    assert!(is_virtual_monitor(&longer, 2580));
    assert!(!is_virtual_monitor(path, 2580), "UID258 is not 2580");
    assert!(!is_virtual_monitor(&path.replace("RBY", "LEN"), 258));
    assert!(!is_virtual_monitor("", 258));
    assert!(
        !is_virtual_monitor("a#\u{e4}\u{e4}#UID258", 258),
        "non-ASCII maker"
    );
}
#[test]
fn the_failure_says_what_switching_the_display_on_came_to() {
    assert_eq!(
        not_active(false, "not tried"),
        "virtual display did not become active before the deadline"
    );
    assert_eq!(
        not_active(true, "Windows refused the display layout"),
        "virtual display was connected but Windows kept it switched off (switching it on: Windows refused the display layout)"
    );
}
