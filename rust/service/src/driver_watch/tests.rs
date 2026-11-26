//! The driver watch's decisions, driven with a fake clock.
use super::*;

#[test]
fn a_check_that_keeps_failing_logs_once_then_on_a_change_or_as_it_clears() {
    let mut check = Repeated::default();
    assert!(!check.passed());
    assert!(check.failed("CM_Get_Device_ID_List failed (13)"));
    for _ in 0..60 {
        assert!(!check.failed("CM_Get_Device_ID_List failed (13)"));
    }
    assert!(check.failed("CM_Get_Device_ID_List failed (31)"));
    assert!(!check.failed("CM_Get_Device_ID_List failed (31)"));
    assert!(check.passed());
    assert!(!check.passed());
    // The same error after it cleared is news again.
    assert!(check.failed("CM_Get_Device_ID_List failed (31)"));
}
/// Drives a display driver's [`DriverWatch`] as the watch thread does, from
/// second 0 to `end`: `present` is the Plug and Play answer at a second,
/// `host` the host's answer to an ask. The seconds at which the host was
/// asked and a repair started.
fn drive(
    end: u64,
    present: impl Fn(u64) -> bool,
    mut host: impl FnMut(u64) -> Option<bool>,
) -> (Vec<u64>, Vec<u64>) {
    let start = Instant::now();
    let mut watch = DriverWatch::new(Driver::Display);
    let (mut asks, mut repairs) = (Vec::new(), Vec::new());
    let mut second = 0;
    while second < end {
        let now = start + Duration::from_secs(second);
        match watch.poll(present(second), now) {
            Step::Wait => {}
            Step::AskHost => {
                asks.push(second);
                assert!(asks.len() < 100, "the host is asked in a loop");
                watch.answered(host(second), now);
                continue;
            }
            Step::Repair => repairs.push(second),
        }
        second += watch.interval().as_secs();
    }
    (asks, repairs)
}
/// Drives the watches of both drivers through [`round`], as the watch thread
/// does: `present` is a driver's Plug and Play answer at a second, `host` the
/// host's answer to an ask. The seconds at which the host was asked, and each
/// setup run's second with the drivers it names.
fn drive_both(
    end: u64,
    present: impl Fn(Driver, u64) -> bool,
    mut host: impl FnMut(u64) -> Option<bool>,
) -> (Vec<u64>, Vec<(u64, Vec<Driver>)>) {
    let start = Instant::now();
    let mut watches: Vec<_> = Driver::ALL
        .into_iter()
        .map(|driver| (driver, DriverWatch::new(driver)))
        .collect();
    let (mut asks, mut runs) = (Vec::new(), Vec::new());
    let mut second = 0;
    while second < end {
        let now = start + Duration::from_secs(second);
        let due = round(
            &mut watches,
            |driver| Some(present(driver, second)),
            |driver| {
                assert_eq!(driver, Driver::Display, "only the display driver asks");
                asks.push(second);
                host(second)
            },
            || now,
        );
        if !due.is_empty() {
            runs.push((second, due.into_iter().map(|(driver, _)| driver).collect()));
        }
        second += watches
            .iter()
            .map(|(_, watch)| watch.interval().as_secs())
            .min()
            .unwrap();
    }
    (asks, runs)
}
fn spaced(seconds: &[u64], at_least: u64) -> bool {
    seconds.windows(2).all(|pair| pair[1] - pair[0] >= at_least)
}
#[test]
fn a_driver_removed_while_the_service_runs_is_repaired_after_a_minute() {
    // Present for ten minutes, then removed (pnputil /remove-device): the
    // host is asked once, at the start; the removal needs no host.
    let (asks, repairs) = drive(5000, |second| second < 600, |_| Some(true));
    assert_eq!(asks, [0]);
    // A repair that left it missing is not started again every few
    // seconds; the next try comes an hour later.
    assert_eq!(repairs, [660, 660 + 3600 + 60]);
    // A blip that heals within the minute never repairs; the next
    // removal gets its own minute.
    let present = |second| !(60..110).contains(&second) && second < 200;
    let (_, repairs) = drive(300, present, |_| Some(true));
    assert_eq!(repairs, [290]);
    // A minute apart while the driver is there, 5 seconds while it is not.
    let start = Instant::now();
    let mut watch = DriverWatch::new(Driver::Display);
    assert_eq!(watch.poll(true, start), Step::AskHost);
    watch.answered(Some(true), start);
    assert_eq!(watch.poll(true, start), Step::Wait);
    assert_eq!(watch.interval(), Duration::from_secs(60));
    watch.poll(false, start + Duration::from_secs(60));
    assert_eq!(watch.interval(), Duration::from_secs(5));
    assert_eq!(
        watch.poll(false, start + Duration::from_secs(120)),
        Step::Repair
    );
    // Missing through the quiet hour: back to a minute.
    assert_eq!(watch.interval(), Duration::from_secs(60));
}
#[test]
fn the_host_is_asked_once_unless_it_refuses_and_never_on_a_timer() {
    // A day with the driver there and usable: one ask, at the start.
    let (asks, repairs) = drive(86_400, |_| true, |_| Some(true));
    assert_eq!(asks, [0]);
    assert!(repairs.is_empty());
    // At boot the device comes up after the service: the ask waits for it.
    let (asks, repairs) = drive(86_400, |second| second >= 30, |_| Some(true));
    assert_eq!(asks, [30]);
    assert!(repairs.is_empty());
    // A day with a device the host keeps refusing (another protocol):
    // asked again a minute later before the repair, then once an hour.
    let (asks, repairs) = drive(86_400, |_| true, |_| Some(false));
    assert_eq!(&asks[..3], [0, 60, 60 + 3600 + 60]);
    assert_eq!(&repairs[..2], [60, 60 + 3600 + 60]);
    assert!(spaced(&asks[1..], 3600), "{asks:?}");
    assert!(asks.len() <= 25 && repairs.len() <= 24);
    // A refusal that passed (the device was still starting) repairs nothing.
    let (asks, repairs) = drive(86_400, |_| true, |second| Some(second > 0));
    assert_eq!(asks, [0, 60]);
    assert!(repairs.is_empty());
    // A host that cannot answer decides nothing: no repair from its
    // silence. It is asked again 10 minutes later, then once an hour.
    let (asks, repairs) = drive(86_400, |_| true, |_| None);
    assert_eq!(&asks[..4], [0, 600, 600 + 3600, 600 + 7200]);
    assert!(spaced(&asks[1..], 3600) && asks.len() <= 25, "{asks:?}");
    assert!(repairs.is_empty());
    // Once it answers, it is not asked again on a timer.
    let (asks, repairs) = drive(86_400, |_| true, |second| (second > 600).then_some(true));
    assert_eq!(asks, [0, 600, 4200]);
    assert!(repairs.is_empty());
    let (asks, repairs) = drive(86_400, |_| true, |second| (second > 0).then_some(true));
    assert_eq!(asks, [0, 600]);
    assert!(repairs.is_empty());
    // A refusal that comes after a failed ask repairs as any other.
    let (asks, repairs) = drive(86_400, |_| true, |second| (second > 0).then_some(false));
    assert_eq!(&asks[..3], [0, 600, 660]);
    assert_eq!(repairs[0], 660);
    // A device that comes and goes does not restart the backoff.
    let (asks, _) = drive(86_400, |second| second % 1200 < 900, |_| None);
    assert_eq!(&asks[..2], [0, 600]);
    assert!(spaced(&asks[1..], 3600), "{asks:?}");
    let (asks, repairs) = drive(86_400, |_| true, |second| (second == 0).then_some(false));
    assert!(repairs.is_empty());
    assert!(spaced(&asks, 60) && spaced(&asks[1..], 3600), "{asks:?}");
    // Whatever the answers, never twice within a minute.
    for host in [Some(true), Some(false), None] {
        let (asks, _) = drive(86_400, |second| (second / 600) % 2 == 0, |_| host);
        assert!(spaced(&asks, 60), "{host:?}: {asks:?}");
    }
}
#[test]
fn a_removed_gamepad_driver_is_repaired_without_asking_the_host() {
    use Driver::*;
    // The display driver there and usable all day; the gamepad's device
    // removed after ten minutes. The host is asked once, about the display.
    let (asks, runs) = drive_both(
        5000,
        |driver, second| driver == Display || second < 600,
        |_| Some(true),
    );
    assert_eq!(asks, [0]);
    assert_eq!(
        runs,
        [(660, vec![Gamepad]), (660 + 3600 + 60, vec![Gamepad])]
    );
    // A gamepad driver there all day: nothing to do, and never a question.
    let mut watch = DriverWatch::new(Gamepad);
    let start = Instant::now();
    for minute in 0..1440 {
        let now = start + Duration::from_secs(minute * 60);
        assert_eq!(watch.poll(true, now), Step::Wait);
        assert_eq!(watch.interval(), Duration::from_secs(60));
    }
}
#[test]
fn drivers_missing_together_are_repaired_in_one_setup_run() {
    use Driver::*;
    let (asks, runs) = drive_both(700, |_, second| second < 600, |_| Some(true));
    assert_eq!(asks, [0]);
    assert_eq!(runs, [(660, vec![Display, Gamepad])]);
    assert_eq!(
        repair_args(&[Display, Gamepad]),
        [
            "--repair-drivers",
            "--driver",
            "display",
            "--driver",
            "gamepad"
        ]
    );
    assert_eq!(
        repair_args(&[Gamepad]),
        ["--repair-drivers", "--driver", "gamepad"]
    );
}
#[test]
fn each_driver_keeps_its_own_quiet_hour() {
    use Driver::*;
    // The host keeps refusing the display device, which is repaired once an
    // hour; the gamepad's device goes later and is repaired a minute after,
    // not held back by the display driver's hour, and then keeps its own.
    let (_, runs) = drive_both(
        5000,
        |driver, second| driver == Display || second < 1000,
        |_| Some(false),
    );
    assert_eq!(
        runs,
        [
            (60, vec![Display]),
            (1080, vec![Gamepad]),
            (3720, vec![Display]),
            (4740, vec![Gamepad]),
        ]
    );
    // Plug and Play that cannot say decides nothing for that driver.
    let mut watches = vec![
        (Display, DriverWatch::new(Display)),
        (Gamepad, DriverWatch::new(Gamepad)),
    ];
    let start = Instant::now();
    for second in [0, 60, 120] {
        let now = start + Duration::from_secs(second);
        let due = round(
            &mut watches,
            |driver| (driver == Display).then_some(true),
            |_| Some(true),
            || now,
        );
        assert!(due.is_empty());
    }
}
#[test]
fn a_driver_left_out_at_install_starts_no_setup() {
    use Driver::*;
    let left_out = |text: &str| {
        let record: Value = serde_json::from_str(text).unwrap();
        (declined(&record, Display), declined(&record, Gamepad))
    };
    // Records setup wrote before the gamepad driver had an entry.
    assert_eq!(left_out(r#"{"declined":true}"#), (true, false));
    assert_eq!(
        left_out(r#"{"version":"6.4.3","unix":86400}"#),
        (false, false)
    );
    assert_eq!(left_out("null"), (false, false));
    // And with it.
    assert_eq!(left_out(r#"{"gamepad":{"declined":true}}"#), (false, true));
    assert_eq!(
        left_out(r#"{"declined":true,"gamepad":{"declined":true}}"#),
        (true, true)
    );
    assert_eq!(
        left_out(r#"{"version":"7.0.0","unix":1,"gamepad":{"version":"7.0.0","unix":2}}"#),
        (false, false)
    );
}
