use super::*;
use butterpollo_core::topology::{Kind, Mode, Position};
fn node(id: &str, active: bool, primary: bool) -> Node {
    Node {
        id: id.into(),
        label: id.into(),
        kind: Kind::Physical,
        active,
        primary,
        desired_position: Position { x: 0, y: 0 },
        mode: Mode {
            width: 1920,
            height: 1080,
            refresh_hz: 60.,
        },
        device_id: id.into(),
    }
}
fn snapshot(nodes: Vec<Node>) -> Snapshot {
    Snapshot {
        version: 1,
        hdr: nodes.iter().map(|n| (n.device_id.clone(), true)).collect(),
        scale: nodes.iter().map(|n| (n.device_id.clone(), 125)).collect(),
        rotation: Default::default(),
        clone_groups: vec![],
        nodes,
    }
}
#[test]
fn a_display_the_layout_switched_off_is_still_watched_after_another_display_left() {
    // Exclusive, two streams: the physical monitor is off, so the snapshot a
    // later layout is composed from does not list it.
    let before = vec![node("physical", true, true)];
    let applied = vec![
        node("physical", false, false),
        node("a", true, true),
        node("b", true, false),
    ];
    let mut desired = vec![node("a", false, false), node("b", true, true)];
    keep_switched_off(&mut desired, &applied);
    assert_eq!(desired.len(), 3);
    // Windows switches it on when the other virtual display is removed.
    assert_eq!(
        butterpollo_core::display_policy::switched_back_on(
            &before,
            &desired,
            &["physical".to_owned(), "b".to_owned()],
        ),
        ["physical"]
    );
    // One the new layout has an answer for keeps that answer.
    let mut desired = vec![node("physical", true, true)];
    keep_switched_off(&mut desired, &[node("physical", false, false)]);
    assert!(desired[0].active);
    assert_eq!(desired.len(), 1);
}
#[test]
fn a_stream_end_wakes_only_the_users_displays_that_are_off_and_connected() {
    let before = [
        node("monitor", true, true),
        node("unplugged", true, false),
        node("dormant", false, false),
        node("a", true, false),
    ];
    let connected = ["monitor", "dormant", "a"].map(String::from);
    let stream_only = [node("a", true, true)];
    assert_eq!(
        displays_to_wake(&before, &stream_only, &connected, "a"),
        ["monitor"]
    );
    // On already, as when the layout left it on: nothing wakes, and the end doesn't wait.
    let both = [node("a", true, true), node("monitor", true, false)];
    assert!(displays_to_wake(&before, &both, &connected, "a").is_empty());
    // With the stream's display gone there is nothing to hold the layout with.
    let no_stream = ["monitor", "dormant"].map(String::from);
    assert!(displays_to_wake(&before, &[], &no_stream, "a").is_empty());
}
/// [`watch_wake`] on a clock that a sleep moves on: what it decided, how often it looked
/// after the display that is away, and when.
fn watched(
    connected_at: impl Fn(std::time::Duration) -> Result<Vec<String>>,
) -> (Wake, usize, std::time::Duration) {
    let now = std::cell::Cell::new(std::time::Duration::ZERO);
    let mut away = 0;
    let wake = watch_wake(
        &["monitor".to_owned()],
        || connected_at(now.get()),
        || now.get(),
        || {
            now.set(now.get() + WAKE_POLL);
            false
        },
        || away += 1,
    );
    (wake, away, now.get())
}
fn listed(monitor: bool) -> Vec<String> {
    let mut ids = vec!["a".to_owned(), "tv".to_owned()];
    if monitor {
        ids.push("monitor".to_owned());
    }
    ids
}
#[test]
fn a_woken_display_that_stays_ends_the_watch_at_its_end() {
    let (wake, away, at) = watched(|_| Ok(listed(true)));
    assert_eq!(wake, Wake::Stayed);
    assert_eq!(away, 0);
    assert_eq!(at, WAKE_WATCH);
}
#[test]
fn a_woken_display_that_drops_is_waited_for_until_it_returns() {
    use std::time::Duration;
    // The monitor measured: gone a second after it came on, back about 3.2 s later.
    let (wake, away, at) = watched(|now| {
        Ok(listed(
            !(Duration::from_secs(1)..Duration::from_millis(4200)).contains(&now),
        ))
    });
    assert_eq!(wake, Wake::CameBack);
    assert!(away > 0, "what Windows recalls meanwhile is looked after");
    assert_eq!(at, Duration::from_millis(4200));
    // Past the end of the watch too: the wait is for the return, not the watch.
    assert!(at > WAKE_WATCH);
}
#[test]
fn a_woken_display_that_never_returns_ends_the_wait_loudly() {
    use std::time::Duration;
    let (wake, _, at) = watched(|now| Ok(listed(now < Duration::from_secs(1))));
    assert_eq!(wake, Wake::StayedAway);
    assert_eq!(at, Duration::from_secs(1) + WAKE_RETURN);
}
#[test]
fn a_read_that_fails_says_nothing_of_a_woken_display() {
    use std::time::Duration;
    // Windows is still applying the change: neither a drop nor a return.
    let (wake, away, _) = watched(|now| {
        if now < Duration::from_secs(1) {
            anyhow::bail!("pending")
        }
        Ok(listed(true))
    });
    assert_eq!((wake, away), (Wake::Stayed, 0));
    let (wake, _, at) = watched(|now| {
        if now < Duration::from_secs(1) {
            Ok(listed(true))
        } else if now < Duration::from_secs(2) {
            Ok(listed(false))
        } else if now < Duration::from_secs(3) {
            anyhow::bail!("pending")
        } else {
            Ok(listed(true))
        }
    });
    assert_eq!(wake, Wake::CameBack);
    assert_eq!(at, Duration::from_secs(3));
}
#[test]
fn a_display_that_came_back_gets_its_layout_again() {
    // (entry written again, layout written again, what came of the layout)
    let run = |first_fails: bool, restored_fails: bool, wake: Wake| {
        let (puts, restores) = (std::cell::Cell::new(0), std::cell::Cell::new(0));
        let failed = |fails: bool| {
            if fails {
                anyhow::bail!("the monitor left halfway");
            }
            Ok(())
        };
        let result = after_wake(
            &["monitor".to_owned()],
            wake,
            failed(first_fails),
            failed(restored_fails),
            || {
                puts.set(puts.get() + 1);
                Some(Ok(()))
            },
            || {
                restores.set(restores.get() + 1);
                Some(Ok(()))
            },
        );
        (puts.get(), restores.get(), result.map(|r| r.is_ok()))
    };
    // The first try went well but the display dropped: its entry may be for the rest.
    assert_eq!(run(false, false, Wake::CameBack), (1, 1, Some(true)));
    assert_eq!(run(true, true, Wake::CameBack), (1, 1, Some(true)));
    // It stayed and all went well: nothing to write.
    assert_eq!(run(false, false, Wake::Stayed), (0, 0, Some(true)));
    // Still applying some other change: tried again, the entry only if that failed,
    // and the layout after it either way, as the entry's try switches displays on.
    assert_eq!(run(true, false, Wake::Stayed), (1, 1, Some(true)));
    assert_eq!(run(false, true, Wake::Stayed), (0, 1, Some(true)));
    // Gone for good: the entry is what the first try came to, and the layout has a last go.
    assert_eq!(run(false, false, Wake::StayedAway), (0, 1, Some(true)));
    assert_eq!(run(true, false, Wake::StayedAway), (0, 1, Some(true)));
    // Stopped: nothing at all, whatever went before.
    assert_eq!(run(true, true, Wake::Stopped), (0, 0, None));
}
#[test]
fn a_write_stopped_halfway_through_its_steps_leaves_the_rest_unwritten() {
    let restores = std::cell::Cell::new(0);
    let result = after_wake(
        &["monitor".to_owned()],
        Wake::CameBack,
        Ok(()),
        Ok(()),
        || None,
        || {
            restores.set(restores.get() + 1);
            Some(Ok(()))
        },
    );
    assert!(result.is_none());
    assert_eq!(restores.get(), 0);
    // A failed entry doesn't stop the layout: that is what the journal waits for.
    let result = after_wake(
        &["monitor".to_owned()],
        Wake::CameBack,
        Ok(()),
        Ok(()),
        || Some(Err(anyhow::anyhow!("still applying"))),
        || Some(Err(anyhow::anyhow!("not back"))),
    );
    assert!(result.is_some_and(|r| r.is_err()));
}
/// Tests that lease or lock the arrangement's own state, which is the process's.
static SHARED_STATE: Mutex<()> = Mutex::new(());
/// Says so when dropped: a stream's display in a test.
struct FakeDisplay(Arc<std::sync::atomic::AtomicBool>);
impl FakeDisplay {
    fn new() -> (Self, Arc<std::sync::atomic::AtomicBool>) {
        let gone = Arc::new(std::sync::atomic::AtomicBool::new(false));
        (Self(gone.clone()), gone)
    }
}
impl Drop for FakeDisplay {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}
fn gone(flag: &std::sync::atomic::AtomicBool) -> bool {
    flag.load(Ordering::SeqCst)
}
fn wake_end(first: Result<()>, restored: Result<()>) -> WakeEnd {
    WakeEnd {
        woken: vec!["monitor".to_owned()],
        restore: true,
        first,
        restored,
    }
}
/// What a watch wrote, in order.
type Writes = Arc<Mutex<Vec<&'static str>>>;
/// [`run_wake_watch`] with its writes noted and displays as `read` says.
fn noted_watch(
    end: WakeEnd,
    stop: &WatchStop,
    writes: &Writes,
    read: impl FnMut() -> Result<Vec<String>>,
) -> bool {
    let note = |what: &'static str| {
        let writes = writes.clone();
        move || {
            writes.lock().unwrap().push(what);
            Ok(())
        }
    };
    let forget = note("journal");
    run_wake_watch(
        end,
        stop,
        read,
        note("strays"),
        note("entry"),
        note("layout"),
        forget,
    )
}
/// A slow watch on a monitor that is there, 3 s from anything happening, on `watcher`.
fn start_slow_watch(
    watcher: &WakeWatcher,
    writes: &Writes,
    display: FakeDisplay,
    restored: Result<()>,
) -> bool {
    let writes = writes.clone();
    let start = watcher.start(watched_left(), move |stop| {
        let left = noted_watch(wake_end(Ok(()), restored), stop, &writes, || {
            Ok(listed(true))
        });
        drop(display);
        left
    });
    start == WatchStart::Started
}
/// What a watch of [`wake_end`] leaves if it doesn't finish.
fn watched_left() -> LeftWatch {
    LeftWatch {
        woken: vec!["monitor".to_owned()],
        restore: true,
    }
}
/// Far under the watch, whose shortest run is `WAKE_WATCH`, and far over a stop's own
/// cost when the machine is busy building.
const STOP_BOUND: std::time::Duration = std::time::Duration::from_secs(1);
#[test]
fn a_launch_gets_past_a_slow_watch_at_once_and_it_writes_nothing() {
    assert!(STOP_BOUND < WAKE_WATCH);
    let watcher = WakeWatcher::new();
    let writes = Writes::default();
    let (display, display_gone) = FakeDisplay::new();
    assert!(start_slow_watch(&watcher, &writes, display, Ok(())));
    let launch = std::time::Instant::now();
    watcher.hold_off();
    assert!(launch.elapsed() < STOP_BOUND, "{:?}", launch.elapsed());
    // Past the stop the stream's display is gone: a new one on its slot can't meet it.
    assert!(gone(&display_gone));
    assert!(writes.lock().unwrap().is_empty());
    // It isn't done: what it was to look after goes to the end of the hold.
    assert_eq!(watcher.release(), Some(watched_left()));
}
#[test]
fn a_watch_stopped_as_the_monitor_returns_writes_nothing_and_keeps_its_journal_entry() {
    let _shared = SHARED_STATE.lock().unwrap();
    let stop = WatchStop::default();
    let writes = Writes::default();
    let looks = std::cell::Cell::new(0);
    let left = noted_watch(wake_end(Ok(()), Ok(())), &stop, &writes, || {
        looks.set(looks.get() + 1);
        if looks.get() == 1 {
            return Ok(listed(false));
        }
        // The return and the launch at the same moment: it writes for no one.
        stop.stop();
        Ok(listed(true))
    });
    assert!(left);
    assert_eq!(looks.get(), 2);
    // Only what it looked after while the monitor was away, before the stop.
    assert_eq!(*writes.lock().unwrap(), ["strays"]);
    // Stopped with the monitor away: that isn't looked after for anyone either.
    let stop = WatchStop::default();
    let writes = Writes::default();
    let left = noted_watch(wake_end(Ok(()), Ok(())), &stop, &writes, || {
        stop.stop();
        Ok(listed(false))
    });
    assert!(left);
    assert!(writes.lock().unwrap().is_empty());
}
#[test]
fn a_stop_during_a_write_ends_the_writes_after_it() {
    let _shared = SHARED_STATE.lock().unwrap();
    let writes = Writes::default();
    let looks = std::cell::Cell::new(0);
    let read = || {
        looks.set(looks.get() + 1);
        Ok(listed(looks.get() % 2 == 0))
    };
    let note = |what: &'static str| writes.lock().unwrap().push(what);
    let stop = WatchStop::default();
    let left = run_wake_watch(
        wake_end(Ok(()), Ok(())),
        &stop,
        read,
        || Ok(()),
        || {
            note("entry");
            stop.stop();
            Ok(())
        },
        || {
            note("layout");
            Ok(())
        },
        || {
            note("journal");
            Ok(())
        },
    );
    assert!(left);
    assert_eq!(*writes.lock().unwrap(), ["entry"]);
    writes.lock().unwrap().clear();
    let stop = WatchStop::default();
    let left = run_wake_watch(
        wake_end(Ok(()), Ok(())),
        &stop,
        read,
        || Ok(()),
        || {
            note("entry");
            Ok(())
        },
        || {
            note("layout");
            stop.stop();
            Ok(())
        },
        || {
            note("journal");
            Ok(())
        },
    );
    assert!(left);
    assert_eq!(*writes.lock().unwrap(), ["entry", "layout"]);
}
#[test]
fn a_stop_ends_a_watchs_wait_at_once() {
    let stop = Arc::new(WatchStop::default());
    let waiting = {
        let stop = stop.clone();
        std::thread::spawn(move || stop.wait(std::time::Duration::from_secs(30)))
    };
    std::thread::sleep(std::time::Duration::from_millis(50));
    let stopping = std::time::Instant::now();
    stop.stop();
    assert!(waiting.join().unwrap());
    assert!(stopping.elapsed() < STOP_BOUND, "{:?}", stopping.elapsed());
}
#[test]
fn a_watch_that_starts_ends_the_one_before_it() {
    let watcher = WakeWatcher::new();
    let writes = Writes::default();
    let (first, first_gone) = FakeDisplay::new();
    assert!(start_slow_watch(&watcher, &writes, first, Ok(())));
    let (second, second_gone) = FakeDisplay::new();
    let started = std::time::Instant::now();
    assert!(start_slow_watch(&watcher, &writes, second, Ok(())));
    assert!(started.elapsed() < STOP_BOUND, "{:?}", started.elapsed());
    assert!(gone(&first_gone));
    assert!(!gone(&second_gone));
    watcher.close();
    assert!(gone(&second_gone));
    assert!(writes.lock().unwrap().is_empty());
}
#[test]
fn a_watch_stopped_with_the_layout_not_back_leaves_that_to_the_end_of_the_hold_too() {
    let watcher = WakeWatcher::new();
    let writes = Writes::default();
    let (display, _) = FakeDisplay::new();
    assert!(start_slow_watch(
        &watcher,
        &writes,
        display,
        Err(anyhow::anyhow!("the monitor left halfway"))
    ));
    watcher.hold_off();
    assert_eq!(watcher.release(), Some(watched_left()));
}
/// [`left_watch`] with its writes noted and displays as `read` says.
fn noted_left_watch(
    stop: &WatchStop,
    writes: &Writes,
    read: impl FnMut() -> Result<Vec<String>>,
    restore: impl FnMut() -> Result<()>,
) -> bool {
    let note = |what: &'static str| {
        let writes = writes.clone();
        move || {
            writes.lock().unwrap().push(what);
            Ok(())
        }
    };
    left_watch(
        watched_left(),
        stop,
        read,
        note("strays"),
        restore,
        note("journal"),
    )
}
#[test]
fn a_launch_with_no_lease_has_the_layout_go_back_when_the_monitor_does_and_only_then_the_entry() {
    let _shared = SHARED_STATE.lock().unwrap();
    let stop = WatchStop::default();
    let writes = Writes::default();
    let looks = std::cell::Cell::new(0);
    let left = noted_left_watch(
        &stop,
        &writes,
        || {
            looks.set(looks.get() + 1);
            // Nothing is on but what Windows lit, away and away, and then it is back.
            assert_eq!(
                writes
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|w| **w != "strays")
                    .count(),
                0,
                "nothing but strays while it is away"
            );
            Ok(listed(looks.get() >= 3))
        },
        || {
            writes.lock().unwrap().push("layout");
            Ok(())
        },
    );
    assert!(!left);
    assert_eq!(
        *writes.lock().unwrap(),
        ["strays", "strays", "layout", "journal"]
    );
    // One that never dropped has its look all the same: what is on may still be wrong by
    // then, gone and returned while the launch took its time ([`layout_is_back`]).
    let writes = Writes::default();
    let left = noted_left_watch(
        &stop,
        &writes,
        || Ok(listed(true)),
        || {
            writes.lock().unwrap().push("layout");
            Ok(())
        },
    );
    assert!(!left);
    assert_eq!(*writes.lock().unwrap(), ["layout", "journal"]);
}
#[test]
fn a_layout_that_did_not_go_back_keeps_its_journal_entry_however_the_hold_ended() {
    let _shared = SHARED_STATE.lock().unwrap();
    let stop = WatchStop::default();
    let writes = Writes::default();
    let looks = std::cell::Cell::new(0);
    let left = noted_left_watch(
        &stop,
        &writes,
        || {
            looks.set(looks.get() + 1);
            Ok(listed(looks.get() != 1))
        },
        || anyhow::bail!("the monitor left again"),
    );
    assert_eq!(*writes.lock().unwrap(), ["strays"]);
    writes.lock().unwrap().clear();
    // Finished: the next stream or start has it, as after any restore that failed.
    assert!(!left);
    assert!(writes.lock().unwrap().is_empty());
}
#[test]
fn a_left_watch_stopped_by_the_next_launch_writes_nothing_and_is_left_again() {
    let _shared = SHARED_STATE.lock().unwrap();
    let stop = WatchStop::default();
    let writes = Writes::default();
    let left = noted_left_watch(
        &stop,
        &writes,
        || {
            stop.stop();
            Ok(listed(true))
        },
        || {
            writes.lock().unwrap().push("layout");
            Ok(())
        },
    );
    assert!(left);
    assert!(writes.lock().unwrap().is_empty());
}
#[test]
fn a_left_watch_changes_nothing_for_a_lease_that_began_and_runs_again_after_a_hold() {
    let _shared = SHARED_STATE.lock().unwrap();
    state().lock().unwrap().users = 1;
    let stop = WatchStop::default();
    let writes = Writes::default();
    let looks = std::cell::Cell::new(0);
    let left = noted_left_watch(
        &stop,
        &writes,
        || {
            looks.set(looks.get() + 1);
            Ok(listed(looks.get() != 1))
        },
        || {
            writes.lock().unwrap().push("layout");
            Ok(())
        },
    );
    state().lock().unwrap().users = 0;
    assert!(left);
    assert!(writes.lock().unwrap().is_empty());
    // A watcher runs one on what a stopped one left, and a new hold stops that in turn.
    let watcher = WakeWatcher::new();
    let (display, _) = FakeDisplay::new();
    assert!(start_slow_watch(&watcher, &writes, display, Ok(())));
    watcher.hold_off();
    let left = watcher.release().unwrap();
    let again = {
        let writes = writes.clone();
        watcher.start(left, move |stop| {
            noted_left_watch(
                stop,
                &writes,
                || Ok(listed(true)),
                || {
                    writes.lock().unwrap().push("layout");
                    Ok(())
                },
            )
        })
    };
    assert_eq!(again, WatchStart::Started);
    assert!(watcher.settling());
    watcher.hold_off();
    assert_eq!(watcher.release(), Some(watched_left()));
    assert!(writes.lock().unwrap().is_empty());
}
#[test]
fn what_a_watch_left_is_known_before_its_display_goes() {
    /// The display of a watch a hold refused: while it goes, which takes a while, that
    /// hold ends. The end of the hold has to know then.
    struct EndsTheHold(Arc<WakeWatcher>, Arc<Mutex<Option<LeftWatch>>>);
    impl Drop for EndsTheHold {
        fn drop(&mut self) {
            *self.1.lock().unwrap() = self.0.release();
        }
    }
    let watcher = Arc::new(WakeWatcher::new());
    watcher.hold_off();
    let found = Arc::new(Mutex::new(None));
    let display = EndsTheHold(watcher.clone(), found.clone());
    let start = watcher.start(watched_left(), move |_| {
        drop(display);
        false
    });
    assert_eq!(start, WatchStart::Refused);
    assert_eq!(*found.lock().unwrap(), Some(watched_left()));
    // The next hold finds nothing of it.
    watcher.hold_off();
    assert!(watcher.release().is_none());
}
#[test]
fn a_watch_with_no_thread_leaves_its_entry_only_if_the_layout_is_not_back() {
    let watcher = WakeWatcher::new();
    let (display, display_gone) = FakeDisplay::new();
    let start = watcher.start_on(
        watched_left(),
        move |_| {
            drop(display);
            false
        },
        |_| Err(std::io::Error::other("no thread")),
    );
    assert_eq!(start, WatchStart::NoThread);
    assert!(gone(&display_gone));
    assert!(watcher.0.lock().unwrap().running.is_none());
    assert!(entry_goes_unwatched(&start, true));
    assert!(!entry_goes_unwatched(&start, false));
    // Refused, the end of the hold or the next start has it.
    assert!(!entry_goes_unwatched(&WatchStart::Refused, true));
    assert!(!entry_goes_unwatched(&WatchStart::Started, true));
}
#[test]
fn the_displays_count_as_settling_while_a_watch_works_or_left_work() {
    let watcher = WakeWatcher::new();
    assert!(!watcher.settling());
    let writes = Writes::default();
    let (display, _) = FakeDisplay::new();
    assert!(start_slow_watch(&watcher, &writes, display, Ok(())));
    assert!(watcher.settling());
    watcher.hold_off();
    assert!(watcher.settling(), "what it left is not looked after yet");
    assert!(watcher.release().is_some());
    assert!(!watcher.settling());
    // One that finished isn't.
    assert_eq!(
        watcher.start(watched_left(), |_| false),
        WatchStart::Started
    );
    while watcher
        .0
        .lock()
        .unwrap()
        .running
        .as_ref()
        .is_some_and(|w| !w.thread.is_finished())
    {
        std::thread::yield_now();
    }
    assert!(!watcher.settling());
}
fn at_hz(id: &str, active: bool, primary: bool, refresh_hz: f64) -> Node {
    let mut node = node(id, active, primary);
    node.mode.refresh_hz = refresh_hz;
    node
}
#[test]
fn a_layout_is_back_only_with_the_monitor_on_at_its_mode_and_nothing_else_lit() {
    let before = [at_hz("monitor", true, true, 120.)];
    let both = ["monitor", "tv"].map(String::from);
    let tv_only = ["tv".to_owned()];
    let monitor = at_hz("monitor", true, true, 120.);
    let back = |current: &[Node], connected: &[String], keep: &[String]| {
        layout_is_back(&before, current, connected, &both, keep)
    };
    assert!(back(&[monitor.clone()], &both, &[]));
    // What Windows lit with the monitor away and then left on.
    assert!(!back(
        &[monitor.clone(), at_hz("tv", true, false, 60.)],
        &both,
        &[]
    ));
    // The monitor connected and left off, or at Windows' timing, or no longer primary.
    assert!(!back(&[at_hz("tv", true, true, 60.)], &both, &[]));
    assert!(!back(&[at_hz("monitor", true, true, 60.)], &both, &[]));
    assert!(!back(
        &[
            at_hz("monitor", true, false, 120.),
            at_hz("a", true, true, 60.)
        ],
        &both,
        &[]
    ));
    // A display someone holds is no stray, and one away can't be asked for.
    assert!(back(
        &[monitor.clone(), at_hz("tv", true, false, 60.)],
        &both,
        &["tv".to_owned()]
    ));
    assert!(back(&[], &tv_only, &[]));
    // A display not known physical (a stream's) is no stray either.
    assert!(back(&[monitor, at_hz("rm", true, false, 60.)], &both, &[]));
}
#[test]
fn a_pending_baseline_keeps_a_remote_monitor_that_arrived_since_on() {
    let pending = snapshot(vec![node("monitor", true, true), node("old", false, false)]);
    let current = snapshot(vec![
        node("rm", true, false),
        node("old", true, false),
        node("tv", true, true),
        node("v", true, false),
    ]);
    let retained = ["rm", "old", "gone"].map(String::from);
    let before = first_baseline(
        Some(pending.clone()),
        None,
        &current,
        "rm",
        &retained,
        false,
    );
    let on: Vec<_> = before
        .nodes
        .iter()
        .filter(|n| n.active)
        .map(|n| n.device_id.as_str())
        .collect();
    // In a layout that goes back, the leased one stays on. The stray and the stream's
    // aren't taken in, and a retained display that isn't on has no node to take.
    assert_eq!(on, ["monitor", "rm", "old"]);
    assert_eq!(before.nodes.len(), 3);
    // With nothing pending the layout is the one found or the one from before the display.
    let found = first_baseline(None, None, &current, "v", &retained, false);
    assert_eq!(found.nodes.len(), current.nodes.len());
    let original = snapshot(vec![node("monitor", true, true), node("v", true, false)]);
    let made = first_baseline(None, Some(original), &current, "v", &retained, true);
    assert!(!made.nodes.iter().any(|n| n.device_id == "v" && n.active));
    let no_original = first_baseline(None, None, &current, "v", &retained, true);
    assert_eq!(no_original.nodes.len(), current.nodes.len());
    assert!(
        !no_original
            .nodes
            .iter()
            .any(|n| n.device_id == "v" && n.active)
    );
}
#[test]
fn only_a_layout_that_keeps_the_others_on_follows_what_a_stopped_watch_left() {
    let before = [node("monitor", true, true), node("rm", true, false)];
    let connected = ["monitor", "tv", "b", "rm"].map(String::from);
    let retained = ["rm".to_owned()];
    let follow = |arrangement| wake_follow(arrangement, &before, &connected, "b", &retained);
    assert!(follow(Arrangement::Exclusive).is_none());
    for arrangement in [
        Arrangement::Extended,
        Arrangement::Primary,
        Arrangement::Isolated,
        Arrangement::PrimaryIsolated,
    ] {
        let follow = follow(arrangement).unwrap();
        // The user's own: not what others hold, which comes and goes on its own.
        assert_eq!(follow.woken, ["monitor"]);
        assert_eq!(follow.looks.gone_since, None);
    }
    // Away as the lease starts: returning is all it can do.
    let away = ["tv", "b", "rm"].map(String::from);
    let follow = wake_follow(Arrangement::Extended, &before, &away, "b", &retained).unwrap();
    assert_eq!(follow.looks.gone_since, Some(std::time::Duration::ZERO));
    // Nothing of the user's to wait for.
    let nothing = [node("rm", true, false), node("b", true, true)];
    assert!(wake_follow(Arrangement::Extended, &nothing, &connected, "b", &retained).is_none());
}
#[test]
fn a_followed_display_gets_strays_switched_off_while_away_and_one_layout_when_back() {
    let before = [node("monitor", true, true)];
    let at = std::time::Duration::from_millis;
    let with = ["monitor", "b"].map(String::from);
    let without = ["b".to_owned()];
    // Away as the stream started.
    let mut follow = wake_follow(Arrangement::Extended, &before, &without, "b", &[]).unwrap();
    assert_eq!(
        follow.step(at(250), Some(&without)),
        FollowStep::SwitchOffStrays
    );
    // A read that fails says nothing of it.
    assert_eq!(follow.step(at(500), None), FollowStep::SwitchOffStrays);
    assert_eq!(follow.step(at(750), Some(&with)), FollowStep::LayOut);
    // Windows still applying a change: another try on the next look, a bounded number.
    assert!(follow.tried(false));
    assert_eq!(follow.step(at(1000), Some(&with)), FollowStep::LayOut);
    assert!(!follow.tried(true));
    assert_eq!(follow.step(at(1250), Some(&with)), FollowStep::Nothing);
    let mut follow = wake_follow(Arrangement::Extended, &before, &without, "b", &[]).unwrap();
    for _ in 1..FOLLOW_TRIES {
        assert!(follow.tried(false));
    }
    assert!(!follow.tried(false));
    assert_eq!(follow.step(at(250), Some(&with)), FollowStep::Nothing);
    // There as the stream started: it can still drop, which is watched as ever.
    let mut follow = wake_follow(Arrangement::Extended, &before, &with, "b", &[]).unwrap();
    assert_eq!(follow.step(at(250), Some(&with)), FollowStep::Nothing);
    assert_eq!(
        follow.step(at(500), Some(&without)),
        FollowStep::SwitchOffStrays
    );
    assert_eq!(follow.step(at(5000), Some(&with)), FollowStep::LayOut);
    // Or it stays: that is looked at too, as it can have gone and returned between looks.
    let mut follow = wake_follow(Arrangement::Extended, &before, &with, "b", &[]).unwrap();
    assert_eq!(follow.step(WAKE_WATCH, Some(&with)), FollowStep::LayOut);
    // Gone for good, the stream goes on as it is and asks no more.
    let mut follow = wake_follow(Arrangement::Extended, &before, &without, "b", &[]).unwrap();
    assert_eq!(
        follow.step(WAKE_RETURN, Some(&without)),
        FollowStep::Nothing
    );
    assert_eq!(
        follow.step(WAKE_RETURN + at(250), Some(&with)),
        FollowStep::Nothing
    );
}
#[test]
fn a_returned_monitor_needs_its_displays_put_back_only_if_they_are_not_as_they_were() {
    let before = [at_hz("monitor", true, true, 120.)];
    let connected = ["monitor", "tv", "b"].map(String::from);
    let physical = ["monitor", "tv"].map(String::from);
    let streams = ["b".to_owned()];
    let wrong = |current: &[Node], keep: &[String], keeps_primary: bool| {
        wake_layout_wrong(
            &before,
            current,
            &connected,
            &physical,
            &streams,
            keep,
            keeps_primary,
        )
    };
    let b = at_hz("b", true, false, 60.);
    let keep = streams.clone();
    // As it was: no write but its place in the layout, so the stream isn't disturbed.
    assert!(!wrong(
        &[at_hz("monitor", true, true, 120.), b.clone()],
        &keep,
        true
    ));
    // Windows' timing.
    assert!(wrong(
        &[at_hz("monitor", true, true, 60.), b.clone()],
        &keep,
        true
    ));
    // Back and dark.
    assert!(wrong(std::slice::from_ref(&b), &keep, true));
    // Windows lit the TV when it returned.
    assert!(wrong(
        &[
            at_hz("monitor", true, true, 120.),
            at_hz("tv", true, false, 60.),
            b.clone()
        ],
        &keep,
        true
    ));
    // The primary counts only on a layout that keeps it.
    let primary_b = at_hz("b", true, true, 60.);
    let moved = [at_hz("monitor", true, false, 120.), primary_b];
    assert!(wrong(&moved, &keep, true));
    assert!(!wrong(&moved, &keep, false));
    // A physical display the stream holds is no stray.
    let keep_tv = ["b".to_owned(), "tv".to_owned()];
    assert!(!wrong(
        &[
            at_hz("monitor", true, true, 120.),
            at_hz("tv", true, false, 60.),
            b
        ],
        &keep_tv,
        true
    ));
}
#[test]
fn switching_off_a_stray_saves_only_an_entry_with_the_users_displays_on() {
    let before = [node("monitor", true, true), node("second", true, false)];
    let on = ["b", "second"].map(String::from);
    // The monitor away: the entry is for what is connected, and has nothing wrong.
    assert!(strays_off_saves(
        &before,
        &on,
        &["second", "tv", "b"].map(String::from)
    ));
    // Connected and off: saved, Windows would recall it off from now on.
    assert!(!strays_off_saves(
        &before,
        &on,
        &["monitor", "second", "tv", "b"].map(String::from)
    ));
}
#[test]
fn laying_out_around_a_returned_monitor_keeps_what_others_hold_on() {
    let current = [
        node("a", true, true),
        node("b", true, false),
        node("rm", true, false),
        node("tv", true, false),
    ];
    let keep = ["a", "b", "rm"].map(String::from);
    let nodes = with_kept(vec![node("monitor", true, true)], &current, &keep);
    let on: Vec<_> = nodes.iter().map(|n| n.device_id.as_str()).collect();
    assert_eq!(on, ["monitor", "a", "b", "rm"]);
}
#[test]
fn a_display_kept_in_a_layout_going_back_goes_to_its_right_and_not_where_it_is_now() {
    // Measured: a remote monitor beside what Windows lit, a TV 3840 wide: in a layout with
    // the user's monitor, 2560 wide, that is a gap.
    let mut monitor = node("monitor", true, true);
    monitor.mode.width = 2560;
    let mut rm = node("rm", true, false);
    rm.mode.width = 1280;
    rm.desired_position = Position { x: 3840, y: 0 };
    let mut held = node("held", true, false);
    held.mode.width = 1280;
    held.desired_position = Position { x: 7000, y: 0 };
    let mut there = held.clone();
    there.desired_position = Position { x: 2560, y: 0 };
    let keep = ["rm", "held"].map(String::from);
    let nodes = with_kept(vec![monitor.clone(), there], &[rm.clone(), held], &keep);
    let place = |id: &str| {
        let node = nodes.iter().find(|n| n.device_id == id).unwrap();
        (node.desired_position.x, node.primary)
    };
    assert_eq!(place("monitor"), (0, true));
    // Already on in the layout: it stays where the layout has it.
    assert_eq!(place("held"), (2560, false));
    assert_eq!(place("rm"), (3840, false));
    // Off in the layout: where it was doesn't count either.
    let mut off = rm.clone();
    off.active = false;
    off.desired_position = Position { x: 9000, y: 0 };
    let nodes = with_kept(vec![monitor, off], &[rm], &keep);
    let rm = nodes.iter().find(|n| n.device_id == "rm").unwrap();
    assert!(rm.active);
    assert_eq!(rm.desired_position.x, 2560);
    assert_eq!(nodes.len(), 2);
}
#[test]
fn a_remote_monitor_in_the_gap_goes_by_the_users_layout_and_not_what_windows_lit() {
    let lit = snapshot(vec![node("tv", true, true)]);
    let users = snapshot(vec![node("monitor", true, true)]);
    let by = |count, pending| {
        remote_monitor_baseline(count, pending, &lit).nodes[0]
            .device_id
            .clone()
    };
    assert_eq!(by(0, Some(&users)), "monitor");
    // Nothing left: the layout from just before it. A lease held: its own entry is no news.
    assert_eq!(by(0, None), "tv");
    assert_eq!(by(1, Some(&users)), "tv");
}
#[test]
fn a_lease_ending_clears_what_it_followed() {
    let _shared = SHARED_STATE.lock().unwrap();
    let before = [node("monitor", true, true)];
    {
        let mut state = state().lock().unwrap();
        state.users = 1;
        state.owners = vec![("a".to_owned(), 1)];
        state.following = wake_follow(Arrangement::Extended, &before, &[], "a", &[]);
    }
    let lease = Lease {
        arrangement: Arrangement::Extended,
        target: Mutex::new("a".to_owned()),
        repair_failure: None,
        remains: Mutex::default(),
    };
    assert!(lease.after_cut_watch());
    drop(lease);
    assert!(state().lock().unwrap().following.is_none());
}
#[test]
fn a_watch_left_alone_writes_after_the_return_and_only_then_clears_its_journal_entry() {
    let _shared = SHARED_STATE.lock().unwrap();
    let stop = WatchStop::default();
    let writes = Writes::default();
    let looks = std::cell::Cell::new(0);
    let left = noted_watch(wake_end(Ok(()), Ok(())), &stop, &writes, || {
        looks.set(looks.get() + 1);
        Ok(listed(looks.get() != 1))
    });
    assert!(!left);
    assert_eq!(
        *writes.lock().unwrap(),
        ["strays", "entry", "layout", "journal"]
    );
    // A layout that didn't go back keeps the journal entry, for the next start.
    let writes = Writes::default();
    looks.set(0);
    let left = run_wake_watch(
        wake_end(Ok(()), Ok(())),
        &stop,
        || {
            looks.set(looks.get() + 1);
            Ok(listed(looks.get() != 1))
        },
        || Ok(()),
        || Ok(()),
        || anyhow::bail!("not back"),
        || {
            writes.lock().unwrap().push("journal");
            Ok(())
        },
    );
    assert!(!left);
    assert!(writes.lock().unwrap().is_empty());
    // A layout the stream's end didn't owe stays out of it: the user changed it.
    let writes = Writes::default();
    looks.set(0);
    let mut end = wake_end(Ok(()), Ok(()));
    end.restore = false;
    let left = noted_watch(end, &stop, &writes, || {
        looks.set(looks.get() + 1);
        Ok(listed(looks.get() != 1))
    });
    assert!(!left);
    assert_eq!(*writes.lock().unwrap(), ["strays", "entry", "journal"]);
}
#[test]
fn the_hosts_stop_ends_a_watch_for_good_and_keeps_its_journal_entry() {
    let watcher = WakeWatcher::new();
    let writes = Writes::default();
    let (display, display_gone) = FakeDisplay::new();
    assert!(start_slow_watch(&watcher, &writes, display, Ok(())));
    let stopping = std::time::Instant::now();
    watcher.close();
    assert!(stopping.elapsed() < STOP_BOUND, "{:?}", stopping.elapsed());
    assert!(gone(&display_gone));
    assert!(writes.lock().unwrap().is_empty());
    // Nothing starts from here on, a stream ending on the way out included, and a
    // stream's end loses no time on it.
    let (display, display_gone) = FakeDisplay::new();
    assert!(!start_slow_watch(&watcher, &writes, display, Ok(())));
    assert!(gone(&display_gone));
    // The journal entry stays for the next start: nothing clears it, and a hold's end
    // doesn't open the watcher again.
    watcher.close();
    watcher.hold_off();
    assert!(watcher.release().is_none());
    let (display, _) = FakeDisplay::new();
    assert!(!start_slow_watch(&watcher, &writes, display, Ok(())));
}
#[test]
fn stopping_a_watch_again_or_with_none_running_is_safe() {
    let watcher = WakeWatcher::new();
    // None ever ran.
    watcher.hold_off();
    assert!(watcher.release().is_none());
    let writes = Writes::default();
    let (display, _) = FakeDisplay::new();
    assert!(start_slow_watch(&watcher, &writes, display, Ok(())));
    // Two launches, the second while the first prepares: each gets past, and the entry
    // is judged once, when the last ends.
    watcher.hold_off();
    watcher.hold_off();
    assert!(watcher.release().is_none());
    assert!(watcher.release().is_some());
    // Started again once they have ended, and a watch that finished is nothing to stop.
    assert_eq!(
        watcher.start(watched_left(), |_| false),
        WatchStart::Started
    );
    while watcher
        .0
        .lock()
        .unwrap()
        .running
        .as_ref()
        .is_some_and(|w| !w.thread.is_finished())
    {
        std::thread::yield_now();
    }
    watcher.hold_off();
    assert!(watcher.release().is_none(), "it finished and left nothing");
}
#[test]
fn a_stream_ending_during_a_launch_starts_no_watch() {
    let watcher = WakeWatcher::new();
    let writes = Writes::default();
    watcher.hold_off();
    let (display, display_gone) = FakeDisplay::new();
    assert!(!start_slow_watch(&watcher, &writes, display, Ok(())));
    // Its display goes at once, and the watch it did not get ran nothing.
    assert!(gone(&display_gone));
    assert!(watcher.0.lock().unwrap().running.is_none());
    assert!(writes.lock().unwrap().is_empty());
    assert_eq!(watcher.release(), Some(watched_left()));
    // Only once: a later launch finds nothing to judge.
    watcher.hold_off();
    assert!(watcher.release().is_none());
}
#[test]
fn a_watch_writes_only_under_the_arrangements_lock_and_not_for_a_stream_that_began() {
    let _shared = SHARED_STATE.lock().unwrap();
    let stop = WatchStop::default();
    let wrote = std::cell::Cell::new(0);
    let write = || {
        wrote.set(wrote.get() + 1);
        // Held by the step, so no lease can begin in the middle of its write.
        assert!(state().try_lock().is_err());
        Ok(())
    };
    assert!(watch_step(&stop, write).is_some());
    assert_eq!(wrote.get(), 1);
    state().lock().unwrap().users = 1;
    let began = watch_step(&stop, write);
    state().lock().unwrap().users = 0;
    assert!(began.is_none());
    stop.stop();
    assert!(watch_step(&stop, write).is_none());
    assert_eq!(wrote.get(), 1);
}
#[test]
fn a_watchs_retries_end_at_a_stop_and_try_again_otherwise() {
    let _shared = SHARED_STATE.lock().unwrap();
    let stop = WatchStop::default();
    let tries = std::cell::Cell::new(0);
    let result = settled_step(&stop, || {
        tries.set(tries.get() + 1);
        if tries.get() < 3 {
            anyhow::bail!("still applying");
        }
        Ok(())
    });
    assert!(result.is_some_and(|r| r.is_ok()));
    assert_eq!(tries.get(), 3);
    // A stop during the wait between tries: no further try, and no wait for its end.
    let tries = std::cell::Cell::new(0);
    let started = std::time::Instant::now();
    let result = settled_step(&stop, || -> Result<()> {
        tries.set(tries.get() + 1);
        stop.stop();
        anyhow::bail!("still applying")
    });
    assert!(result.is_none());
    assert_eq!(tries.get(), 1);
    assert!(started.elapsed() < SETTLE_RETRY);
}
#[test]
fn only_a_display_outside_the_users_layout_makes_a_stream_end_watch() {
    let before = [node("monitor", true, true), node("dormant", false, false)];
    // A PC with its monitor and nothing else: nothing Windows could switch on instead.
    let others = ["monitor".to_owned()];
    assert!(displays_it_could_switch_on(&before, &others).is_empty());
    // A TV that is off, listed or not in the layout from before the stream.
    let others = ["monitor", "dormant", "tv"].map(String::from);
    assert_eq!(
        displays_it_could_switch_on(&before, &others),
        ["dormant", "tv"]
    );
}
#[test]
fn a_pc_with_nothing_else_connected_watches_nothing_and_reads_no_further() {
    let before = [node("monitor", true, true)];
    let connected = ["monitor", "a"].map(String::from);
    let others = ["monitor".to_owned()];
    let unread = || -> Result<Vec<Node>> { panic!("nothing to protect, nothing to read") };
    let watched = woken_to_watch(&before, unread, &connected, &others, "a").unwrap();
    assert!(watched.is_empty());
    // With a TV that is off the same end has the monitor to watch.
    let connected = ["monitor", "tv", "a"].map(String::from);
    let others = ["monitor", "tv"].map(String::from);
    let active = || Ok(vec![node("a", true, true)]);
    let watched = woken_to_watch(&before, active, &connected, &others, "a").unwrap();
    assert_eq!(watched, ["monitor"]);
}
#[test]
fn a_lease_that_ends_beside_others_drops_what_remains_of_it_without_its_lock() {
    /// Removing a display takes a while: not with the arrangement's lock held, and only
    /// once the lease has gone.
    struct Remains(Arc<std::sync::atomic::AtomicBool>);
    impl Drop for Remains {
        fn drop(&mut self) {
            assert_eq!(state().try_lock().map(|s| s.users).ok(), Some(1));
            self.0.store(true, Ordering::SeqCst);
        }
    }
    let _shared = SHARED_STATE.lock().unwrap();
    {
        let mut state = state().lock().unwrap();
        state.users = 2;
        state.owners = vec![("a".to_owned(), 2)];
    }
    let flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let lease = Lease {
        arrangement: Arrangement::Exclusive,
        target: Mutex::new("a".to_owned()),
        repair_failure: None,
        remains: Mutex::default(),
    };
    lease.end(Remains(flag.clone()));
    assert!(gone(&flag));
    let mut state = state().lock().unwrap();
    assert_eq!(state.users, 1);
    state.users = 0;
    state.owners.clear();
}
#[test]
fn only_a_physical_display_off_before_the_stream_is_switched_off_while_the_monitor_is_away() {
    let before = [node("monitor", true, true), node("second", true, false)];
    let active = ["a", "c", "second", "tv"].map(String::from);
    // `noid` isn't known to be physical: it can be a stream's, and stays.
    let physical = ["monitor", "second", "tv"].map(String::from);
    let (off, on) = strays(&before, &active, &physical, &[]);
    assert_eq!(off, ["tv"]);
    assert_eq!(on, ["a", "c", "second"]);
    // A physical display a stream holds stays.
    let (off, on) = strays(&before, &active, &physical, &["tv".to_owned()]);
    assert!(off.is_empty());
    assert_eq!(on.len(), 4);
}
#[test]
fn a_first_exclusive_layout_knows_the_monitor_windows_already_switched_off() {
    // Windows recalled the entry an earlier stream left: the virtual display alone
    // when the layout is composed, the monitor already off.
    let before = [node("monitor", true, true)];
    let current = [node("a", true, true)];
    let desired = first_layout(Arrangement::Exclusive, &current, "a", &[], &before, true).unwrap();
    let monitor = desired.iter().find(|n| n.device_id == "monitor").unwrap();
    assert!(!monitor.active && !monitor.primary);
    // So its coming back is seen, and it is no arrival to leave on.
    assert_eq!(
        butterpollo_core::display_policy::switched_back_on(
            &before,
            &desired,
            &["monitor".to_owned(), "a".to_owned()],
        ),
        ["monitor"]
    );
    let mut state = holding(Arrangement::Exclusive, desired, true);
    state.owners = vec![("a".to_owned(), 1)];
    let on = [node("monitor", true, false), node("a", true, true)];
    assert!(arriving_displays(&state, &on, &[]).is_empty());
    // A display the stream keeps on is not one to switch off.
    let retained = ["monitor".to_owned()];
    let kept = first_layout(
        Arrangement::Exclusive,
        &current,
        "a",
        &retained,
        &before,
        true,
    )
    .unwrap();
    assert!(!kept.iter().any(|n| n.device_id == "monitor"));
    // Nor does a layout that leaves the others on: one that is off stays no concern.
    let extended = first_layout(Arrangement::Extended, &current, "a", &[], &before, true).unwrap();
    assert!(!extended.iter().any(|n| n.device_id == "monitor"));
}
/// A layout that saves its entry, as the first stream's virtual display leaves it.
fn saving_state(first: &str) -> State {
    let mut state = State {
        persisted: persist(Arrangement::Exclusive, true),
        ..Default::default()
    };
    add_owner(&mut state, first, true);
    state
}
/// Lay out as `apply` ends: true when the layout was saved.
fn saves(state: &mut State, leaving: Option<&str>) -> bool {
    let saved = std::cell::Cell::new(false);
    settle_layout(state, Vec::new(), leaving, &[], || {
        saved.set(true);
        Ok(())
    });
    saved.get()
}
#[test]
fn a_layout_without_a_display_on_its_way_out_is_not_saved_under_its_entry() {
    let mut state = saving_state("a");
    add_owner(&mut state, "b", true);
    assert!(!saves(&mut state, Some("a")));
    assert!(saves(&mut state, None));
}
#[test]
fn a_remote_monitor_left_alone_saves_no_entry_that_keeps_the_monitors_off() {
    // A exclusive beside remote monitor M.
    let mut state = saving_state("a");
    add_owner(&mut state, "m", false);
    assert!(saves(&mut state, None));
    // A ends. Saved now, {P, m} would have P off, and Windows recalls it when
    // C's display goes while M's retained display stays.
    assert_eq!(
        release_owner(&mut state, "a"),
        Released {
            left: true,
            saved: true
        }
    );
    assert!(!saves(&mut state, None));
    // C arrives: its entry, {P, c, m}, is saved, and put back when it ends.
    add_owner(&mut state, "c", true);
    assert!(saves(&mut state, None));
    assert!(!release_owner(&mut state, "m").saved);
    assert!(saves(&mut state, None));
    assert!(release_owner(&mut state, "c").saved);
    assert!(!saves(&mut state, None));
}
#[test]
fn a_physical_display_left_alone_never_writes_the_users_own_entry() {
    let mut state = saving_state("a");
    add_owner(&mut state, "p2", false);
    release_owner(&mut state, "a");
    assert!(!saves(&mut state, None));
    // Its end puts nothing back either: the entry for the user's displays is theirs.
    assert_eq!(
        release_owner(&mut state, "p2"),
        Released {
            left: true,
            saved: false
        }
    );
}
#[test]
fn a_stream_sharing_a_display_keeps_its_entry_until_the_last_lease() {
    let mut state = saving_state("a");
    state.owners[0].1 += 1;
    assert!(!release_owner(&mut state, "a").left);
    assert!(saves(&mut state, None));
    assert!(release_owner(&mut state, "a").left);
    assert!(state.owners.is_empty());
    assert!(!saves(&mut state, None));
}
#[test]
fn a_layout_that_saves_nothing_does_not_start_to_for_a_later_virtual_display() {
    let mut state = State::default();
    add_owner(&mut state, "m", false);
    add_owner(&mut state, "c", true);
    assert!(!saves(&mut state, None));
    assert!(!release_owner(&mut state, "c").saved);
}
#[test]
fn a_display_that_could_not_be_laid_out_is_no_owner_and_saves_nothing() {
    let mut state = saving_state("a");
    add_owner(&mut state, "b", true);
    remove_added_owner(&mut state, "b");
    assert_eq!(state.owners, [("a".to_owned(), 1)]);
    assert_eq!(state.persists_for, ["a"]);
}
#[test]
fn a_display_leaving_a_layout_the_user_changed_lays_nothing_out_again() {
    let mut state = State {
        applied: vec![node("a", true, true), node("phys", true, false)],
        ..Default::default()
    };
    let before = departures();
    let mut applied = false;
    display_left(&mut state, "a", false, |_| {
        applied = true;
        Ok(())
    })
    .unwrap();
    // Win+P or a primary picked in Windows stays, now and after Windows settles.
    assert!(!applied);
    assert_eq!(departures(), before);
    assert!(!state.applied[0].active);
    assert!(state.applied[1].active);
    // Still as applied: laid out now, and again once Windows has settled.
    display_left(&mut state, "a", true, |_| {
        applied = true;
        Ok(())
    })
    .unwrap();
    assert!(applied);
    assert_eq!(departures(), before + 1);
    assert!(minimized_before_departure().is_some());
}
#[test]
fn only_a_recent_departure_explains_a_minimised_window() {
    let start = std::time::Instant::now();
    let windows = |ids: &[isize]| {
        ids.iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>()
    };
    let mut last = None;
    note_departure(&mut last, start, || windows(&[1]));
    assert_eq!(recent_departure(&last, start), Some(&windows(&[1])));
    // A second display soon after: what the first minimised was not the user's doing.
    let second = start + std::time::Duration::from_secs(9);
    note_departure(&mut last, second, || windows(&[1, 2]));
    assert_eq!(recent_departure(&last, second), Some(&windows(&[1])));
    // The streams handle it once Windows settled, past the first display's window.
    let handled = second + std::time::Duration::from_millis(1500);
    assert!(handled > start + DEPARTURE_RECENT);
    assert_eq!(recent_departure(&last, handled), Some(&windows(&[1])));
    // A stream that was paused meanwhile comes back too late to tell.
    let late = second + DEPARTURE_RECENT;
    assert_eq!(recent_departure(&last, late), None);
    note_departure(&mut last, late, || windows(&[3]));
    assert_eq!(recent_departure(&last, late), Some(&windows(&[3])));
}
#[test]
fn a_read_of_the_layout_settles_and_a_failed_one_is_no_user_change() {
    let reads = |last| {
        let mut reads = [Err(anyhow::anyhow!("Windows is applying")), Ok(last)].into_iter();
        left_as_applied(|| reads.next().unwrap())
    };
    // Windows applying a change: the read after it is the answer, either way.
    assert!(reads(true));
    assert!(!reads(false));
    // It never answered: nothing says the user changed the layout.
    assert!(left_as_applied(|| anyhow::bail!("Windows is applying")));
}
#[test]
fn a_display_arriving_beside_a_remote_monitor_left_alone_saves_no_entry() {
    let arrive = |state: &State| {
        let on = std::cell::RefCell::new(Vec::new());
        let saved = std::cell::Cell::new(0);
        settle_arrival(
            state,
            "m2",
            |ids| {
                *on.borrow_mut() = ids.to_vec();
                Ok(())
            },
            || {
                saved.set(saved.get() + 1);
                Ok(())
            },
        )
        .unwrap();
        (on.into_inner(), saved.get())
    };
    // A exclusive beside remote monitor M: a second one arriving settles the entry.
    let mut state = saving_state("a");
    add_owner(&mut state, "m", false);
    state.users = 2;
    assert_eq!(
        arrive(&state),
        (vec!["a".into(), "m".into(), "m2".into()], 1)
    );
    // A ended. The displays still go on as they should, but {P, m, m2} with P off is
    // no entry to save: nothing would forget it, and Windows would recall P dark.
    release_owner(&mut state, "a");
    state.users = 1;
    assert_eq!(arrive(&state), (vec!["m".into(), "m2".into()], 0));
    // No stream left to arrive beside.
    state.users = 0;
    assert_eq!(arrive(&state), (Vec::<String>::new(), 0));
}
#[test]
fn a_display_arriving_beside_a_stream_is_on_with_it_and_what_it_keeps() {
    let owners = [("a".to_owned(), 1), ("kept".to_owned(), 2)];
    assert_eq!(
        arrived_layout(&owners, &["monitor".to_owned(), "a".to_owned()], "b"),
        ["a", "b", "kept", "monitor"]
    );
}
#[test]
fn a_streams_display_windows_dropped_is_switched_back_on_before_the_layout() {
    let targets = ["a".to_owned(), "b".to_owned()];
    assert_eq!(
        targets_off(
            &[node("physical", true, true), node("a", true, false)],
            &targets
        ),
        ["b"]
    );
    assert!(targets_off(&[node("a", true, true), node("b", true, false)], &targets).is_empty());
    assert_eq!(
        targets_off(&[node("a", true, true), node("b", false, false)], &targets),
        ["b"]
    );
}
#[test]
fn reapplying_follows_a_display_windows_listed_under_another_name() {
    let monitor = |device_id: &str, display_name: &str| crate::display::Monitor {
        device_id: device_id.into(),
        monitor_device_path: String::new(),
        display_name: display_name.into(),
        friendly_name: String::new(),
        hdr_supported: false,
        hdr_enabled: false,
        primary: false,
        adapter: Default::default(),
        target: 0,
        source: 0,
    };
    let monitors = [
        monitor("SAM7454", r"\\.\DISPLAY1"),
        monitor("RBY0001", r"\\.\DISPLAY25"),
    ];
    // Issue 56: the stream still knew DISPLAY23.
    let target = reapplied_target(&monitors, r"\\.\DISPLAY23", "RBY0001").unwrap();
    assert_eq!(target.display_name, r"\\.\DISPLAY25");
    assert_eq!(
        reapplied_target(&monitors, r"\\.\DISPLAY1", "RBY0001")
            .unwrap()
            .device_id,
        "SAM7454",
        "a known output name wins"
    );
    assert!(reapplied_target(&monitors, r"\\.\DISPLAY23", "").is_none());
    assert!(reapplied_target(&monitors, r"\\.\DISPLAY23", "RBY0002").is_none());
}
#[test]
fn disabling_an_original_display_requires_restoration_after_a_competing_layout_change() {
    let before = vec![node("physical", true, true), node("tv", true, false)];
    let exclusive = vec![
        node("physical", false, false),
        node("tv", false, false),
        node("vdd", true, true),
    ];
    let user_recovery = vec![node("physical", true, true), node("vdd", true, false)];
    assert!(!matches(&user_recovery, &exclusive));
    assert!(disabled_original_displays(&before, &exclusive));
    let extended = vec![
        node("physical", true, true),
        node("tv", true, false),
        node("vdd", true, false),
    ];
    assert!(!disabled_original_displays(&before, &extended));
    // A display that was already off must not cause a forced restore.
    assert!(!disabled_original_displays(
        &[node("physical", false, false)],
        &exclusive
    ));
    // Adopting an earlier failed restoration can mean the physical
    // target is already absent from the new stream's active snapshot.
    assert!(disabled_original_displays(
        &before,
        &[node("vdd", true, true)]
    ));
}
#[test]
fn an_unplugged_display_does_not_keep_the_layout_from_being_restored() {
    let mut tv = node("tv", true, false);
    tv.desired_position = Position { x: 1920, y: 0 };
    let applied = vec![node("monitor", true, true), tv];
    let current = vec![node("monitor", true, true)];
    let without_tv: std::collections::BTreeSet<String> = ["monitor".to_string()].into();
    assert!(matches_connected(&current, &applied, Some(&without_tv)));
    // Still connected but switched off: the user changed the layout.
    let with_tv: std::collections::BTreeSet<String> =
        ["monitor".to_string(), "tv".to_string()].into();
    assert!(!matches_connected(&current, &applied, Some(&with_tv)));
    assert!(!matches(&current, &applied));
    assert!(matches(&applied, &applied));
}
#[test]
fn a_remote_monitor_switched_on_mid_stream_keeps_the_streams_layout() {
    let mut state = State {
        users: 1,
        before: Some(snapshot(vec![node("phys", true, true)])),
        applied: vec![node("vdd", true, true), node("phys", false, false)],
        ..Default::default()
    };
    let mut remote = node("remote", true, false);
    remote.desired_position = Position { x: 1920, y: 0 };
    let current = vec![node("vdd", true, true), remote];
    let connected: std::collections::BTreeSet<String> =
        ["vdd", "phys", "remote"].map(String::from).into();
    assert!(!matches_connected(
        &current,
        &state.applied,
        Some(&connected)
    ));
    assert!(retain_in(&mut state, &current, "remote", true));
    assert!(matches_connected(
        &current,
        &state.applied,
        Some(&connected)
    ));
    // The restore keeps it on beside the user's own displays.
    let before = state.before.as_ref().unwrap();
    assert!(
        before
            .nodes
            .iter()
            .any(|n| n.device_id == "remote" && n.active)
    );
    assert!(
        before
            .nodes
            .iter()
            .any(|n| n.device_id == "phys" && n.active)
    );
    assert_eq!(state.retained, ["remote"]);
    // Disconnected: no longer kept on, and its removal is no change.
    assert!(!retain_in(&mut state, &[], "remote", false));
    assert!(state.retained.is_empty());
    let current = vec![node("vdd", true, true)];
    let connected: std::collections::BTreeSet<String> = ["vdd", "phys"].map(String::from).into();
    assert!(matches_connected(
        &current,
        &state.applied,
        Some(&connected)
    ));
    // Without a stream holding a layout there is nothing to record.
    let mut idle = State::default();
    assert!(!retain_in(
        &mut idle,
        &[node("remote", true, false)],
        "remote",
        true
    ));
    assert!(idle.applied.is_empty() && idle.retained.is_empty());
}
#[test]
fn a_saved_layout_left_behind_by_a_crash_is_noticed_when_the_virtual_display_arrives() {
    let before = vec![node("phys", true, true), node("tv", false, false)];
    let connected = ["phys", "tv", "vdd"].map(String::from);
    let extended_stream = |current: &[Node], connected: &[String]| {
        recall_to_undo(
            persist(Arrangement::Extended, true),
            Arrangement::Extended,
            &before,
            current,
            connected,
        )
    };
    // Windows recalled a stream's saved layout as the display arrived.
    assert!(extended_stream(&[node("vdd", true, true)], &connected));
    // As Windows lays out a new display: everything stays on.
    let extended = [node("phys", true, true), node("vdd", true, false)];
    assert!(!extended_stream(&extended, &connected));
    // A display unplugged since, or off before the stream, is no sign.
    let unplugged = ["tv", "vdd"].map(String::from);
    assert!(!extended_stream(&[node("vdd", true, true)], &unplugged));
    assert!(persist(Arrangement::Exclusive, true));
    assert!(!persist(Arrangement::Exclusive, false));
    assert!(!persist(Arrangement::Extended, true));
}
#[test]
fn a_saved_layout_is_undone_for_a_layout_that_keeps_the_monitor_and_not_for_one_that_saves_its_own()
{
    let before = [node("phys", true, true)];
    let connected = ["phys", "vdd"].map(String::from);
    // What an exclusive stream's end used to save: the virtual display primary and
    // the monitor at the timing Windows picked. Measured: 2560x1440 at 60, not 120.
    let mut slow = node("phys", true, false);
    slow.mode.refresh_hz = 30.;
    let recalled = [slow, node("vdd", true, true)];
    let undo =
        |saves, arrangement| recall_to_undo(saves, arrangement, &before, &recalled, &connected);
    for arrangement in [
        Arrangement::Extended,
        Arrangement::Primary,
        Arrangement::Isolated,
        Arrangement::PrimaryIsolated,
    ] {
        assert!(
            undo(persist(arrangement, true), arrangement),
            "{arrangement:?}"
        );
    }
    assert!(
        !undo(
            persist(Arrangement::Exclusive, true),
            Arrangement::Exclusive
        ),
        "an exclusive stream switches the monitor off anyway"
    );
    // A second stream on a layout that saves its own: `display_arrived` settled it.
    assert!(!undo(true, Arrangement::Exclusive));
}
#[test]
fn the_entry_a_stream_puts_back_has_the_users_displays_as_they_had_them() {
    let mut monitor = node("monitor", true, true);
    monitor.mode.width = 2560;
    monitor.mode.refresh_hz = 120.;
    let before = snapshot(vec![monitor.clone(), node("vdd", false, false)]);
    // The exclusive layout, with the monitor just switched on by Windows.
    let mut retimed = monitor.clone();
    retimed.mode.refresh_hz = 60.;
    retimed.primary = false;
    retimed.desired_position = Position { x: 1280, y: 0 };
    let mut vdd = node("vdd", true, true);
    vdd.mode.width = 1280;
    let current = [vdd, retimed];
    let connected = ["monitor", "vdd"].map(String::from);
    let layout = entry_layout(&before, &current, &connected, "vdd").unwrap();
    let find = |id: &str| layout.nodes.iter().find(|n| n.device_id == id).unwrap();
    let monitor_now = find("monitor");
    assert_eq!(monitor_now.mode.refresh_hz, 120.);
    assert_eq!(monitor_now.desired_position, Position { x: 0, y: 0 });
    assert!(monitor_now.active && monitor_now.primary);
    let vdd_now = find("vdd");
    assert!(vdd_now.active && !vdd_now.primary);
    assert_eq!(vdd_now.desired_position, Position { x: 2560, y: 0 });
    assert_eq!(vdd_now.mode.width, 1280);
    assert_eq!(layout.nodes.len(), 2);
    // The user's HDR and scale go with them.
    assert_eq!(layout.hdr, before.hdr);
    assert_eq!(layout.scale, before.scale);
    // Nothing to save for a display that is off.
    assert!(entry_layout(&before, &[node("vdd", false, false)], &connected, "vdd").is_err());
}
#[test]
fn a_display_that_is_gone_leaves_no_gap_in_the_entry() {
    let mut monitor = node("monitor", true, true);
    monitor.mode.width = 2560;
    // A remote monitor that left during the stream: `retain_in` keeps its node on.
    let mut gone = node("remote", true, false);
    gone.desired_position = Position { x: 2560, y: 0 };
    let before = snapshot(vec![monitor, gone]);
    let current = [node("vdd", true, true)];
    let connected = ["monitor", "vdd"].map(String::from);
    let layout = entry_layout(&before, &current, &connected, "vdd").unwrap();
    assert!(!layout.nodes.iter().any(|n| n.device_id == "remote"));
    let vdd = layout.nodes.iter().find(|n| n.device_id == "vdd").unwrap();
    assert_eq!(vdd.desired_position, Position { x: 2560, y: 0 });
    // Still connected, it keeps its place and the entry goes beyond it.
    let connected = ["monitor", "remote", "vdd"].map(String::from);
    let layout = entry_layout(&before, &current, &connected, "vdd").unwrap();
    let vdd = layout.nodes.iter().find(|n| n.device_id == "vdd").unwrap();
    assert_eq!(vdd.desired_position, Position { x: 4480, y: 0 });
}
#[test]
fn a_layout_that_keeps_its_displays_apart_saves_no_entry() {
    for arrangement in [Arrangement::Isolated, Arrangement::PrimaryIsolated] {
        let mut monitor = node("monitor", true, true);
        monitor.mode.refresh_hz = 120.;
        let mut apart = node("a", true, false);
        apart.desired_position = Position { x: 64000, y: 64000 };
        let mut state = holding(arrangement, vec![monitor, apart], false);
        state.owners = vec![("a".to_owned(), 1)];
        assert!(!repair_saves(&state), "{arrangement:?}");
        // The repair at a departure still puts the monitor back, unsaved.
        let connected = ["monitor", "a"].map(String::from);
        let repair = departure_repair(
            &state,
            arrangement,
            &[node("monitor", true, true), node("a", true, false)],
            &connected,
            &[],
        )
        .unwrap();
        assert!(!repair.saves, "{arrangement:?}");
    }
}
/// Run [`put_entry_back`] with the stream's display as `current` has it: the steps
/// taken, in order, and its result.
fn entry_steps(
    current: Result<Vec<Node>>,
    restored: Result<()>,
) -> (Vec<&'static str>, Result<()>) {
    let steps = std::cell::RefCell::new(Vec::new());
    let before = snapshot(vec![node("monitor", true, true)]);
    let result = put_entry_back(
        &before,
        "vdd",
        || {
            steps.borrow_mut().push("connected");
            Ok(vec!["monitor".to_owned(), "vdd".to_owned()])
        },
        |ids| {
            assert_eq!(ids, ["monitor", "vdd"]);
            steps.borrow_mut().push("on");
            Ok(())
        },
        || current,
        |layout| {
            assert!(
                layout
                    .nodes
                    .iter()
                    .any(|n| n.device_id == "vdd" && n.active)
            );
            steps.borrow_mut().push("restore");
            restored
        },
        || {
            steps.borrow_mut().push("save");
            Ok(())
        },
    );
    (steps.into_inner(), result)
}
#[test]
fn a_stream_display_that_is_gone_saves_no_entry_for_the_users_displays() {
    // A driver reset: `set_active` dropped it and switched on the monitor at
    // Windows' timing. That is not what to file for the monitor alone.
    let (steps, result) = entry_steps(Ok(vec![node("monitor", true, false)]), Ok(()));
    assert_eq!(steps, ["connected", "on"]);
    assert!(result.is_err());
    let (steps, result) = entry_steps(
        Ok(vec![
            node("monitor", true, false),
            node("vdd", false, false),
        ]),
        Ok(()),
    );
    assert_eq!(steps, ["connected", "on"]);
    assert!(result.is_err());
    // A read that fails says nothing of it either: `settled` tries again.
    let (steps, result) = entry_steps(Err(anyhow::anyhow!("pending")), Ok(()));
    assert_eq!(steps, ["connected", "on"]);
    assert!(result.is_err());
}
#[test]
fn a_monitor_dropping_off_the_list_as_it_comes_on_stays_in_its_entry() {
    let connected = std::cell::RefCell::new(vec!["monitor".to_owned(), "vdd".to_owned()]);
    let laid_out = std::cell::RefCell::new(Vec::new());
    let before = snapshot(vec![node("monitor", true, true)]);
    put_entry_back(
        &before,
        "vdd",
        || Ok(connected.borrow().clone()),
        |_| {
            // A monitor off for a while comes up: for a moment Windows doesn't list it.
            connected.borrow_mut().retain(|id| id != "monitor");
            Ok(())
        },
        || Ok(vec![node("vdd", true, true)]),
        |layout| {
            let on = layout.nodes.iter().filter(|n| n.active);
            *laid_out.borrow_mut() = on.map(|n| n.device_id.clone()).collect();
            Ok(())
        },
        || Ok(()),
    )
    .unwrap();
    assert_eq!(*laid_out.borrow(), ["monitor", "vdd"]);
}
#[test]
fn the_stream_displays_entry_is_saved_after_its_layout_and_even_if_that_only_partly_went_back() {
    let on = || vec![node("monitor", true, false), node("vdd", true, true)];
    let (steps, result) = entry_steps(Ok(on()), Ok(()));
    assert_eq!(steps, ["connected", "on", "restore", "save"]);
    assert!(result.is_ok());
    let (steps, result) = entry_steps(Ok(on()), Err(anyhow::anyhow!("mode refused")));
    assert_eq!(steps, ["connected", "on", "restore", "save"]);
    assert!(result.is_err());
}
/// Streams holding `arrangement`: the monitor on before them, `applied` now.
fn holding(arrangement: Arrangement, applied: Vec<Node>, persisted: bool) -> State {
    let mut state = State {
        users: 1,
        arrangement: Some(arrangement),
        before: Some(snapshot(vec![node("monitor", true, true)])),
        applied,
        persisted,
        ..Default::default()
    };
    add_owner(&mut state, "r1", persisted);
    state
}
#[test]
fn a_repair_that_failed_still_lays_out_and_comes_back_for_the_card() {
    let laid_out = std::cell::Cell::new(false);
    let lay_out = || {
        laid_out.set(true);
        Ok(())
    };
    let failure = repair_then_lay_out(
        || Err(anyhow::anyhow!("refused").context("putting it back")),
        lay_out,
    )
    .unwrap();
    assert_eq!(failure.as_deref(), Some("putting it back: refused"));
    assert!(laid_out.get());
    // A repair that went through has nothing to say.
    assert_eq!(repair_then_lay_out(|| Ok(()), || Ok(())).unwrap(), None);
    // A layout that failed stays an error: the watch tries again.
    assert!(repair_then_lay_out(|| Ok(()), || anyhow::bail!("no")).is_err());
    assert!(repair_then_lay_out(|| anyhow::bail!("refused"), || anyhow::bail!("no")).is_err());
    // The repair comes first: the layout then lays out what it put back.
    let order = std::cell::RefCell::new(Vec::new());
    repair_then_lay_out(
        || {
            order.borrow_mut().push("repair");
            Ok(())
        },
        || {
            order.borrow_mut().push("lay out");
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(*order.borrow(), ["repair", "lay out"]);
}
fn exclusive_lease() -> State {
    holding(
        Arrangement::Exclusive,
        vec![node("monitor", false, false), node("r1", true, true)],
        false,
    )
}
#[test]
fn a_repair_under_a_layout_that_keeps_the_monitor_off_saves_no_entry_with_it_off() {
    // A remote monitor's exclusive lease, or a physical display's: nothing puts an
    // entry back, so saved it would keep the monitor dark on a later layout.
    assert!(!repair_saves(&exclusive_lease()));
    // A layout that leaves the monitor on saves what the repair put back.
    let extended = holding(
        Arrangement::Extended,
        vec![node("monitor", true, true), node("r1", true, false)],
        false,
    );
    assert!(repair_saves(&extended));
    // One that saves its own has its end to put that entry back.
    let saving = holding(
        Arrangement::Exclusive,
        vec![node("monitor", false, false), node("r1", true, true)],
        true,
    );
    assert!(repair_saves(&saving));
    // Nothing held: the entry is for the user's displays as they are.
    assert!(repair_saves(&State::default()));
}
#[test]
fn a_second_remote_monitor_under_an_exclusive_lease_is_judged_by_that_lease() {
    let state = exclusive_lease();
    let connected = ["monitor", "r1", "r2"].map(String::from);
    let before = [node("r1", true, true)];
    // The arrival made the monitor primary: exclusive has no primary to keep, and
    // its own layout puts that right.
    let moved = [
        node("monitor", true, true),
        node("r1", true, false),
        node("r2", true, false),
    ];
    assert_eq!(
        remote_monitor_repair(&state, &before, &moved, &connected),
        None
    );
    // The arrival retimed R1: repaired, and not saved with the monitor off.
    let mut slow = node("r1", true, true);
    slow.mode.refresh_hz = 30.;
    let retimed = [slow, node("r2", true, false)];
    assert_eq!(
        remote_monitor_repair(&state, &before, &retimed, &connected),
        Some(false)
    );
    // No lease: the extended check, and the entry is the user's as they had it.
    let idle = State::default();
    let before = [node("monitor", true, true)];
    let recalled = [node("monitor", true, false), node("r2", true, true)];
    assert_eq!(
        remote_monitor_repair(&idle, &before, &recalled, &connected),
        Some(true)
    );
    // A lease that ended is none.
    let mut ended = exclusive_lease();
    ended.users = 0;
    ended.before = None;
    assert_eq!(
        remote_monitor_repair(&ended, &before, &recalled, &connected),
        Some(true)
    );
}
/// Extended, stream `a` on beside the monitor at 120 Hz, as B's display left them.
fn left_by_a_departure() -> State {
    let mut monitor = node("monitor", true, true);
    monitor.mode.refresh_hz = 120.;
    let mut state = holding(
        Arrangement::Extended,
        vec![monitor, node("a", true, false)],
        false,
    );
    state.owners = vec![("a".to_owned(), 1)];
    state
}
fn at_60(id: &str, primary: bool) -> Node {
    node(id, true, primary)
}
#[test]
fn a_display_on_its_way_to_a_stream_stays_on_through_a_departures_repair() {
    let state = left_by_a_departure();
    let connected = ["monitor", "a", "c"].map(String::from);
    // C launched in the settle window: its display is on, and in no layout yet.
    let current = [
        at_60("monitor", true),
        node("a", true, false),
        node("c", true, false),
    ];
    let repair =
        departure_repair(&state, Arrangement::Extended, &current, &connected, &[]).unwrap();
    assert_eq!(repair.keep_on, ["c"]);
    assert!(!repair.saves, "the entry is the arriving display's to save");
}
#[test]
fn a_physical_display_windows_recalled_is_not_an_arriving_display() {
    // Exclusive, a display just left: Windows recalled an entry with the TV on, which
    // was off before the stream and so in no layout, while C's display is on its way.
    let mut state = holding(
        Arrangement::Exclusive,
        vec![node("monitor", false, false), node("a", true, true)],
        true,
    );
    state.owners = vec![("a".to_owned(), 1)];
    state.persists_for = vec!["a".to_owned()];
    let physical = ["monitor", "tv"].map(String::from);
    let current = [
        node("tv", true, false),
        node("a", true, true),
        node("c", true, false),
    ];
    // C stays on, the TV doesn't: left on it would stay on for the stream's life.
    assert_eq!(arriving_displays(&state, &current, &physical), ["c"]);
    let (desired, arriving) =
        compose_around_arrivals(&state, Arrangement::Exclusive, &current, &physical).unwrap();
    assert_eq!(arriving, ["c"]);
    let on = |id: &str| desired.iter().any(|n| n.device_id == id && n.active);
    assert!(on("a") && on("c"));
    assert!(!on("tv"));
    // With no virtual display arriving, what the pass lays out is saved: the entry no
    // longer has the TV on when Windows next recalls it.
    let current = [node("tv", true, false), node("a", true, true)];
    let (desired, arriving) =
        compose_around_arrivals(&state, Arrangement::Exclusive, &current, &physical).unwrap();
    assert!(arriving.is_empty());
    let saved = std::cell::Cell::new(false);
    settle_layout(&mut state, desired, None, &arriving, || {
        saved.set(true);
        Ok(())
    });
    assert!(saved.get());
    // A display whose path can't be read for the moment isn't known to be physical,
    // and might be a stream's: it stays on.
    let current = [node("noid", true, false), node("a", true, true)];
    state.applied = vec![node("a", true, true)];
    assert_eq!(arriving_displays(&state, &current, &physical), ["noid"]);
}
#[test]
fn a_repair_leaves_a_virtual_display_on_and_not_a_physical_one() {
    let state = left_by_a_departure();
    let connected = ["monitor", "a", "c", "tv"].map(String::from);
    let current = [
        at_60("monitor", true),
        node("a", true, false),
        node("c", true, false),
        node("tv", true, false),
    ];
    let physical = ["monitor", "tv"].map(String::from);
    let repair = departure_repair(
        &state,
        Arrangement::Extended,
        &current,
        &connected,
        &physical,
    )
    .unwrap();
    assert_eq!(repair.keep_on, ["c"]);
}
#[test]
fn only_a_repair_that_finds_the_same_displays_on_is_saved() {
    let state = left_by_a_departure();
    let connected = ["monitor", "a"].map(String::from);
    let run = |current: &[Node]| {
        departure_repair(&state, Arrangement::Extended, current, &connected, &[])
    };
    // Windows retimed the monitor: put back, and the entry healed.
    let repair = run(&[at_60("monitor", true), node("a", true, false)]).unwrap();
    assert!(repair.saves && repair.keep_on.is_empty());
    assert_eq!(
        repair.on.iter().map(String::as_str).collect::<Vec<_>>(),
        ["a", "monitor"]
    );
    // The monitor is off: Windows' entry or the user's Win+P, which can't be told
    // apart. On again, and not filed as if the user's choice was known.
    let repair = run(&[node("a", true, true)]).unwrap();
    assert!(!repair.saves);
    assert!(repair.on.contains("monitor"));
    // As it was left: nothing to do.
    let mut same = node("monitor", true, true);
    same.mode.refresh_hz = 120.;
    assert_eq!(run(&[same, node("a", true, false)]), None);
    // A display gone since is no change in what is on.
    let mut tv = node("tv", true, false);
    tv.desired_position = Position { x: 1920, y: 0 };
    let mut with_tv = left_by_a_departure();
    with_tv.applied.push(tv);
    let repair = departure_repair(
        &with_tv,
        Arrangement::Extended,
        &[at_60("monitor", true), node("a", true, false)],
        &connected,
        &[],
    )
    .unwrap();
    assert!(repair.saves);
    // A layout that saves its own entry has nothing to repair here.
    let mut saving = left_by_a_departure();
    saving.persisted = true;
    assert_eq!(
        departure_repair(
            &saving,
            Arrangement::Exclusive,
            &[at_60("monitor", true), node("a", true, false)],
            &connected,
            &[],
        ),
        None
    );
}
#[test]
fn a_departures_repair_under_a_layout_with_the_monitor_off_saves_nothing() {
    let mut state = left_by_a_departure();
    state.applied = vec![node("monitor", false, false), node("a", true, true)];
    // A second display the layout keeps on, retimed: that is what gets repaired.
    let mut kept = node("kept", true, false);
    kept.mode.refresh_hz = 120.;
    state.applied.push(kept);
    let connected = ["monitor", "a", "kept"].map(String::from);
    let repair = departure_repair(
        &state,
        Arrangement::Exclusive,
        &[node("a", true, true), at_60("kept", false)],
        &connected,
        &[],
    )
    .unwrap();
    assert!(!repair.saves);
}
#[test]
fn a_layout_lays_out_around_what_is_arriving_and_keeps_it_out_of_its_record() {
    // Exclusive, B just left, A stays: the monitor came back on, and C's display is
    // on its way. Switching it off now fails C's launch.
    let mut state = holding(
        Arrangement::Exclusive,
        vec![node("monitor", false, false), node("a", true, true)],
        true,
    );
    state.owners = vec![("a".to_owned(), 1)];
    state.persists_for = vec!["a".to_owned()];
    let current = [
        node("monitor", true, false),
        node("a", true, true),
        node("c", true, false),
    ];
    let (desired, arriving) =
        compose_around_arrivals(&state, Arrangement::Exclusive, &current, &[]).unwrap();
    assert_eq!(arriving, ["c"]);
    let on = |id: &str| desired.iter().any(|n| n.device_id == id && n.active);
    assert!(on("a") && on("c"));
    assert!(!on("monitor"));
    let saved = std::cell::Cell::new(false);
    settle_layout(&mut state, desired.clone(), None, &arriving, || {
        saved.set(true);
        Ok(())
    });
    assert!(!saved.get(), "C's own layout saves the entry with it in");
    assert!(!state.applied.iter().any(|n| n.device_id == "c"));
    // So it still counts as arriving at the next pass, and no less on.
    assert_eq!(arriving_displays(&state, &current, &[]), ["c"]);
    // Once C holds a lease it is no longer arriving, nor is a retained display.
    add_owner(&mut state, "c", true);
    assert!(arriving_displays(&state, &current, &[]).is_empty());
    state.owners.pop();
    state.retained.push("c".to_owned());
    assert!(arriving_displays(&state, &current, &[]).is_empty());
}
#[test]
fn what_windows_recalled_after_a_display_left_goes_back_to_the_layout_it_left() {
    let mut monitor = node("monitor", true, true);
    monitor.mode.refresh_hz = 120.;
    let mut left = node("a", false, false);
    left.desired_position = Position { x: 1920, y: 0 };
    let mut stays = node("b", true, false);
    stays.desired_position = Position { x: 3840, y: 0 };
    let applied = vec![monitor.clone(), left, stays];
    let mut before = snapshot(vec![monitor]);
    before.clone_groups = vec![vec!["monitor".to_owned(), "twin".to_owned()]];
    let layout = after_departure(&applied, Some(&before));
    let on = |nodes: &[Node]| -> Vec<(String, bool, i32, f64)> {
        nodes
            .iter()
            .map(|n| {
                (
                    n.device_id.clone(),
                    n.active,
                    n.desired_position.x,
                    n.mode.refresh_hz,
                )
            })
            .collect()
    };
    assert_eq!(on(&layout.nodes), on(&applied));
    assert_eq!(layout.clone_groups, before.clone_groups);
    // The user's other settings may have changed since: none of them is touched.
    assert!(layout.hdr.is_empty() && layout.scale.is_empty() && layout.rotation.is_empty());
    assert!(after_departure(&applied, None).clone_groups.is_empty());
}
#[test]
fn the_original_layout_leaves_out_the_streams_virtual_display() {
    let layout = original_layout(
        snapshot(vec![node("monitor", true, true), node("vdd", true, false)]),
        "vdd",
    );
    let vdd = layout.nodes.iter().find(|n| n.device_id == "vdd").unwrap();
    assert!(!vdd.active && !vdd.primary);
    assert!(
        layout
            .nodes
            .iter()
            .any(|n| n.device_id == "monitor" && n.active)
    );
    assert!(!layout.hdr.contains_key("vdd") && !layout.scale.contains_key("vdd"));
    assert!(layout.hdr.contains_key("monitor"));
    // With no other active display the layout cannot exclude it.
    let only = original_layout(
        snapshot(vec![node("monitor", false, false), node("vdd", true, true)]),
        "vdd",
    );
    assert!(only.nodes.iter().any(|n| n.device_id == "vdd" && n.active));
}
