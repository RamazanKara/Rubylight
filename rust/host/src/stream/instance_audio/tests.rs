use super::*;

fn id(pid: u32) -> Identity {
    Identity { pid, started: 1 }
}

/// A route of a test's program.
#[derive(Debug, PartialEq)]
struct Route(u32, &'static str);
impl Program for Route {
    fn program(&self) -> &str {
        self.1
    }
}
fn game(pid: u32) -> Option<Result<Route>> {
    Some(Ok(Route(
        pid,
        r"c:\nucleus\content\game\instance0\game.exe",
    )))
}

/// A co-op launch plays game-only audio whatever the host's default: its
/// instance moves unless the stream plays on the PC.
#[test]
fn a_coop_player_plays_on_the_pc_only_when_they_ask() {
    assert!(!plays_on_pc(&Config::default(), false));
    assert!(plays_on_pc(&Config::default(), true));
    let muted = Config::parse("stream_audio = false\n").unwrap();
    assert!(plays_on_pc(&muted, false));
}

/// #59: an instance moves in the pass that finds it, before its stream
/// looks for it; a process it starts moves with it, a failed route is
/// not retried each pass, and a gone process lets go.
#[test]
fn a_session_moves_its_instances_as_it_finds_them() {
    let mut early = Early::<u32>::default();
    let mut asked = Vec::new();
    fn pass(early: &mut Early<u32>, trees: &[BTreeSet<Identity>], asked: &mut Vec<u32>) {
        early.update(trees, &mut |pid| {
            asked.push(pid);
            match pid {
                // Windows' own program.
                30 => None,
                31 => Some(Err(anyhow::anyhow!("no virtual playback device"))),
                pid => Some(Ok(pid)),
            }
        })
    }
    pass(&mut early, &[BTreeSet::new()], &mut asked);
    assert!(early.routed.is_empty());
    pass(&mut early, &[BTreeSet::from([id(20)])], &mut asked);
    assert_eq!(early.routed[&id(20)], (0, Some(20)));
    let tree = BTreeSet::from([id(20), id(21), id(30), id(31)]);
    pass(&mut early, std::slice::from_ref(&tree), &mut asked);
    pass(&mut early, &[tree], &mut asked);
    assert_eq!(asked, [20, 21, 30, 31]);
    assert_eq!(early.routed[&id(21)], (0, Some(21)));
    assert_eq!(early.routed[&id(30)], (0, None));
    assert_eq!(early.routed[&id(31)], (0, None));
    pass(&mut early, &[BTreeSet::from([id(21)])], &mut asked);
    assert_eq!(early.routed.keys().copied().collect::<Vec<_>>(), [id(21)]);
}

/// The session's pass never waits for Windows: a route that takes long
/// runs on the routes' own thread, which then routes only the latest
/// instances. The session's end waits until they went back.
#[test]
fn the_session_pass_never_waits_for_a_route() {
    let (release, blocked) = mpsc::channel::<()>();
    let blocked = Mutex::new(blocked);
    let (asked_tx, asked) = mpsc::channel();
    let (back_tx, back) = mpsc::channel();
    let routes = InstanceRoutes::start(
        move |pid| {
            asked_tx.send(pid).unwrap();
            if pid == 20 {
                blocked.lock().unwrap().recv().unwrap();
            }
            game(pid)
        },
        move |pids| {
            back_tx.send(pids.to_vec()).unwrap();
            None
        },
        false,
    )
    .unwrap();
    let started = std::time::Instant::now();
    routes.found(&[BTreeSet::from([id(20)])]);
    assert_eq!(asked.recv_timeout(Duration::from_secs(5)).unwrap(), 20);
    // While pid 20's route hangs, the passes go on.
    routes.found(&[BTreeSet::from([id(20), id(21)])]);
    routes.found(&[BTreeSet::from([id(20), id(22)])]);
    routes.found(&[BTreeSet::from([id(20), id(22)])]);
    assert!(started.elapsed() < Duration::from_secs(1));
    release.send(()).unwrap();
    assert_eq!(asked.recv_timeout(Duration::from_secs(5)).unwrap(), 22);
    routes.stopping(&[20, 22]);
    assert_eq!(back.try_recv().unwrap(), [20, 22]);
    drop(routes);
    assert!(asked.try_recv().is_err(), "pid 21 was gone before its turn");
}

/// Each co-op player's choice holds: an instance is routed before its
/// player is known, and goes back to the PC for one who plays there
/// (told it does from the game's next start, as its sound opened on the
/// sink); one not started yet just stays. A player who doesn't plays on
/// the sink again.
#[test]
fn a_player_who_plays_on_the_pc_gets_their_instance_back() {
    let mut early = Early::<Route>::default();
    let (one, two) = (BTreeSet::from([id(20)]), BTreeSet::from([id(21)]));
    let trees = [one, two.clone()];
    let other = |pid| {
        Some(Ok(Route(
            pid,
            r"c:\nucleus\content\game\instance1\game.exe",
        )))
    };
    let mut acquire = |pid| if pid == 20 { game(pid) } else { other(pid) };
    early.update(&trees, &mut acquire);
    assert_eq!(early.routed.len(), 2);
    assert_eq!(early.on_pc(1, true), Some(FROM_NEXT_START));
    early.update(&trees, &mut acquire);
    assert_eq!(early.routed.keys().copied().collect::<Vec<_>>(), [id(20)]);
    assert_eq!(early.on_pc(1, true), None, "said once");
    assert_eq!(early.on_pc(2, true), None, "not started: plays on the PC");
    assert_eq!(early.on_pc(1, false), None);
    early.update(&trees, &mut acquire);
    assert_eq!(early.routed[&id(21)].0, 1);
    // One program for every instance: Windows can't tell them apart.
    let mut shared = Early::<Route>::default();
    shared.update(&trees, &mut game);
    assert_eq!(shared.on_pc(1, true), Some(SHARED_PROGRAM));
}

/// The first player's choice is known before the instances start: their
/// own instance is never routed.
#[test]
fn the_first_player_who_plays_on_the_pc_is_never_routed() {
    let (asked_tx, asked) = mpsc::channel();
    let routes = InstanceRoutes::start(
        move |pid| {
            asked_tx.send(pid).unwrap();
            game(pid)
        },
        |_| None,
        true,
    )
    .unwrap();
    routes.found(&[BTreeSet::from([id(20)]), BTreeSet::from([id(21)])]);
    assert_eq!(asked.recv_timeout(Duration::from_secs(5)).unwrap(), 21);
    assert_eq!(routes.on_pc(1, true), Some(FROM_NEXT_START));
    drop(routes);
    assert!(asked.try_recv().is_err());
}

/// A pass's instances that came just before a player's choice are
/// routed first, not skipped: the session never sends them again.
#[test]
fn instances_just_before_a_choice_are_routed() {
    let (hold, held) = mpsc::channel::<()>();
    let (asked_tx, asked) = mpsc::channel();
    let routes = InstanceRoutes::start(
        move |pid| {
            asked_tx.send(pid).unwrap();
            if pid == 20 {
                held.recv().unwrap();
            }
            game(pid)
        },
        |_| None,
        false,
    )
    .unwrap();
    routes.found(&[BTreeSet::from([id(20)])]);
    assert_eq!(asked.recv_timeout(Duration::from_secs(5)).unwrap(), 20);
    routes.found(&[BTreeSet::from([id(20)]), BTreeSet::from([id(21)])]);
    let choice = std::thread::scope(|scope| {
        let choice = scope.spawn(|| routes.on_pc(0, false));
        // The choice is queued behind the pass before the route ends.
        std::thread::sleep(Duration::from_millis(50));
        hold.send(()).unwrap();
        choice.join().unwrap()
    });
    assert_eq!(choice, None);
    assert_eq!(asked.recv_timeout(Duration::from_secs(5)).unwrap(), 21);
}

fn silent() -> Option<Result<Route>> {
    Some(Err(butterpollo_windows::audio_app_route::NoSoundYet.into()))
}

/// #59: an instance routed as it starts hasn't opened its sound, which
/// Windows refuses: it is routed again on its own, with no new pass and
/// no warning, until it has.
#[test]
fn an_instance_without_sound_yet_is_routed_again_until_it_opened_it() {
    let (asked_tx, asked) = mpsc::channel();
    let opened = Arc::new(Mutex::new(false));
    let open = opened.clone();
    let routes = InstanceRoutes::start(
        move |pid| {
            let route = if *open.lock().unwrap() {
                game(pid)
            } else {
                silent()
            };
            asked_tx.send(pid).unwrap();
            route
        },
        |_| None,
        false,
    )
    .unwrap();
    routes.found(&[BTreeSet::from([id(20)])]);
    let wait = RETRY[RETRY.len() - 1] + Duration::from_secs(1);
    assert_eq!(asked.recv_timeout(wait).unwrap(), 20);
    assert_eq!(asked.recv_timeout(wait).unwrap(), 20, "tried again");
    *opened.lock().unwrap() = true;
    assert_eq!(asked.recv_timeout(wait).unwrap(), 20, "routed");
    let after = asked.recv_timeout(wait);
    assert!(after.is_err(), "routed: not tried again ({after:?})");
}

/// The session's stop ends the tries: a stopping instance is put back
/// and never routed again, nor after it stopped (its pid may be another
/// process's by then), until the session finds its instances anew; and
/// none once the handles dropped.
#[test]
fn the_session_stop_ends_the_tries() {
    let (asked_tx, asked) = mpsc::channel();
    let routes = InstanceRoutes::start(
        move |pid| {
            asked_tx.send(pid).unwrap();
            silent()
        },
        |_| None,
        false,
    )
    .unwrap();
    let instances = [BTreeSet::from([id(20)])];
    routes.found(&instances);
    let wait = RETRY[2] + Duration::from_millis(500);
    assert_eq!(asked.recv_timeout(wait).unwrap(), 20);
    routes.stopping(&[20]);
    // A try may have been under way as the stop came.
    let _ = asked.try_recv();
    assert!(asked.recv_timeout(wait).is_err(), "no try while stopping");
    routes.stopped();
    assert!(asked.recv_timeout(wait).is_err(), "no try once stopped");
    routes.found(&instances);
    assert_eq!(asked.recv_timeout(wait).unwrap(), 20, "found anew");
    drop(routes);
    let _ = asked.try_recv();
    assert!(asked.recv_timeout(wait).is_err(), "no try once dropped");
}

/// A process that never opens its sound (a launcher's helper) is tried
/// less and less often ([`RETRY`]): each try lists the PC's playback
/// devices. After [`SILENT_FOR`] that is said, once. One that opens it
/// is routed at its next try, and a gone one waits for nothing.
#[test]
fn a_process_that_never_opens_its_sound_is_tried_less_often() {
    let mut early = Early::<u32>::default();
    let tree = [BTreeSet::from([id(20)])];
    let t0 = Instant::now();
    let mut tries = Vec::new();
    let mut said = Vec::new();
    let opened = false;
    let mut at = Duration::ZERO;
    while at < Duration::from_secs(60) {
        let now = t0 + at;
        early.update_at(
            &tree,
            &mut |pid| {
                tries.push(at.as_millis());
                if opened {
                    Some(Ok(pid))
                } else {
                    Some(Err(butterpollo_windows::audio_app_route::NoSoundYet.into()))
                }
            },
            now,
        );
        if early.waiting.said() && said.is_empty() {
            said.push(at.as_millis());
        }
        at += Duration::from_millis(100);
    }
    let mut expected = vec![0, 500, 1500, 3500];
    while *expected.last().unwrap() + 5000 < 60_000 {
        expected.push(expected.last().unwrap() + 5000);
    }
    assert_eq!(tries, expected, "0.5, 1, 2, then every 5 s");
    assert_eq!(said, [33_500], "the first try past 30 s says it");
    let now = t0 + Duration::from_secs(70);
    assert_eq!(early.waiting.next(now), Some(Duration::ZERO), "due");
    let opened = true;
    early.update_at(
        &tree,
        &mut |pid| {
            assert!(opened);
            Some(Ok(pid))
        },
        now,
    );
    assert_eq!(early.routed[&id(20)], (0, Some(20)));
    assert_eq!(early.waiting.next(now), None);
    let mut gone = Early::<u32>::default();
    gone.update_at(
        &tree,
        &mut |_| Some(Err(butterpollo_windows::audio_app_route::NoSoundYet.into())),
        t0,
    );
    assert!(gone.waiting.next(t0).is_some());
    gone.update_at(&[BTreeSet::new()], &mut |_| unreachable!(), t0);
    assert_eq!(gone.waiting.next(t0), None);
}

/// Dropping the last handle never waits for a route in progress.
#[test]
fn dropping_the_routes_never_waits_for_windows() {
    let (hold, held) = mpsc::channel::<()>();
    let (asked_tx, asked) = mpsc::channel();
    let routes = InstanceRoutes::start(
        move |pid| {
            asked_tx.send(pid).unwrap();
            held.recv().unwrap();
            game(pid)
        },
        |_| None,
        false,
    )
    .unwrap();
    routes.found(&[BTreeSet::from([id(20)])]);
    asked.recv_timeout(Duration::from_secs(5)).unwrap();
    let dropped = std::time::Instant::now();
    drop(routes);
    assert!(dropped.elapsed() < Duration::from_secs(1));
    hold.send(()).unwrap();
}
