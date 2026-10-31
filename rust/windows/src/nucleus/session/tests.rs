use super::super::instances::tests::{Fake, ON_SCREEN, fake, process, window};
use super::super::pads::tests::{Fake as FakeXInput, Recorder};
use super::*;
use butterpollo_core::nucleus::window::{Rect, Window};
use butterpollo_core::steam::Process;

#[test]
fn webview2_keeps_its_data_in_the_users_own_folder() {
    // WebView2's browser runs without administrator rights and could not
    // write Nucleus' Administrators-only webview\cache (#59).
    let local = Path::new(r"C:\Users\Player\AppData\Local");
    let environment = webview_environment(local, &BTreeMap::new());
    assert_eq!(environment.len(), 1);
    assert_eq!(
        environment["WEBVIEW2_USER_DATA_FOLDER"],
        r"C:\Users\Player\AppData\Local\Butterpollo\nucleus-webview"
    );
}

#[test]
fn a_steam_games_app_id_joins_webview2s_folder() {
    // Nucleus needs both: WebView2's folder for its window, and the app
    // id so a Steam game it starts by its .exe doesn't restart via Steam.
    let local = Path::new(r"C:\Users\Player\AppData\Local");
    let environment = webview_environment(local, &steam_environment(Some(400)));
    assert_eq!(
        environment.keys().collect::<Vec<_>>(),
        ["SteamAppId", "SteamGameId", "WEBVIEW2_USER_DATA_FOLDER"]
    );
    assert_eq!(environment["SteamAppId"], "400");
}

#[test]
fn the_log_says_what_rights_nucleus_really_has() {
    assert_eq!(rights_note(false, &Ok(false)), None);
    assert_eq!(rights_note(true, &Ok(true)), None);
    // An administrator with User Account Control off: a "user" run
    // gets their full token.
    let uac_off = rights_note(false, &Ok(true)).unwrap();
    assert!(
        uac_off.contains("runs with administrator rights all the same"),
        "{uac_off}"
    );
    assert!(
        rights_note(true, &Ok(false))
            .unwrap()
            .contains("without administrator rights")
    );
    let unread = rights_note(false, &Err(anyhow::anyhow!("access denied"))).unwrap();
    assert!(unread.ends_with(": access denied"), "{unread}");
}

#[test]
fn the_users_own_copy_starts_as_them_with_the_elevated_ones_environment() {
    let local = Path::new(r"C:\Users\Player\AppData\Local");
    let environment = webview_environment(local, &steam_environment(Some(400)));
    let copy = Path::new(r"C:\ProgramData\Butterpollo\nucleus-user\S-1-5-21-7\0123456789abcdef");
    let user = launch_of(copy, "Rubylight-game", &environment, Run::User);
    assert_eq!(
        user,
        Launch {
            program: copy.join("NucleusCoop.exe"),
            folder: copy.to_path_buf(),
            arguments: vec!["Rubylight-game".into()],
            elevated: false,
            environment: BTreeMap::from([
                ("SteamAppId".to_string(), "400".to_string()),
                ("SteamGameId".to_string(), "400".to_string()),
                (
                    "WEBVIEW2_USER_DATA_FOLDER".to_string(),
                    r"C:\Users\Player\AppData\Local\Butterpollo\nucleus-webview".to_string()
                ),
            ]),
        }
    );
    // Nothing lowers the game's rights behind its back (__COMPAT_LAYER):
    // the same environment as elevated, only the token differs.
    let host = Path::new(r"C:\ProgramData\Butterpollo\nucleus\2.4.2");
    let elevated = launch_of(host, "Rubylight-game", &environment, Run::Elevated);
    assert!(elevated.elevated);
    assert_eq!(elevated.environment, user.environment);
}

#[test]
fn an_account_without_administrator_rights_is_refused_before_nucleus_starts() {
    // A standard user's Nucleus Co-op hung on its loading screen: it
    // could not write its WebView2 cache into the Administrators-only folder.
    for refused in [Refused::Open, Refused::Game] {
        assert!(require_administrator(Account::Elevates, refused).is_ok());
        let words = format!(
            "{:#}",
            require_administrator(Account::Standard, refused).unwrap_err()
        );
        assert_eq!(
            words,
            "Nucleus Co-op needs administrator rights, and Windows doesn't give this account any: sign in to Windows on this PC as an administrator"
        );
    }
    // An administrator under Windows' Administrator protection is told
    // what can't run and what still does: never to sign in as an
    // administrator, which doesn't help, nor to turn it off.
    let open = format!(
        "{:#}",
        require_administrator(Account::Protected, Refused::Open).unwrap_err()
    );
    assert_eq!(
        open,
        "Nucleus Co-op needs administrator rights, and Windows' Administrator protection keeps them from Rubylight, so Nucleus Co-op can't open on this PC; games Rubylight adds itself still stream if they need no administrator rights"
    );
    // The console's words before the button is pressed.
    assert_eq!(open, needs_administrator(Account::Protected));
    let game = format!(
        "{:#}",
        require_administrator(Account::Protected, Refused::Game).unwrap_err()
    );
    assert_eq!(
        game,
        "Nucleus Co-op needs administrator rights, and Windows' Administrator protection keeps them from Rubylight, so Nucleus Co-op can't run this game on this PC; games Rubylight adds itself still stream if they need no administrator rights"
    );
}

/// A fake desktop the test changes while the session runs.
#[derive(Clone)]
struct Screen(Arc<Mutex<Fake>>);
impl Desktop for Screen {
    fn processes(&mut self) -> Result<Vec<Process>> {
        self.0.lock().unwrap().processes()
    }
    fn image_path(&mut self, pid: u32) -> Option<String> {
        self.0.lock().unwrap().image_path(pid)
    }
    fn windows(&mut self) -> Result<Vec<Window>> {
        self.0.lock().unwrap().windows()
    }
    fn monitors(&mut self) -> Result<Vec<Rect>> {
        self.0.lock().unwrap().monitors()
    }
    fn virtual_screen(&mut self) -> Rect {
        self.0.lock().unwrap().virtual_screen()
    }
    fn place(&mut self, window: u64, to: Rect) -> Result<()> {
        self.0.lock().unwrap().place(window, to)
    }
    fn unthrottle(&mut self, processes: &BTreeSet<u32>) {
        self.0.lock().unwrap().unthrottle(processes)
    }
    fn label(&mut self, processes: &BTreeSet<u32>, text: &str) -> Result<Option<u64>> {
        self.0.lock().unwrap().label(processes, text)
    }
    fn click(&mut self, label: u64) -> Result<()> {
        self.0.lock().unwrap().click(label)
    }
    fn message_boxes(&mut self, processes: &BTreeSet<u32>) -> Result<Vec<MessageBox>> {
        self.0.lock().unwrap().message_boxes(processes)
    }
    fn close_forms(&mut self, processes: &BTreeSet<u32>) -> Result<Vec<MessageBox>> {
        self.0.lock().unwrap().close_forms(processes)
    }
}

const NUCLEUS: u32 = 10;
const SINCE: u64 = 100;

#[derive(Clone, Default)]
struct Probe {
    exit: Arc<Mutex<Option<u32>>>,
    stopped: Arc<Mutex<Option<BTreeMap<u32, u64>>>>,
    launched: Arc<Mutex<Vec<String>>>,
    environments: Arc<Mutex<Vec<BTreeMap<String, String>>>>,
    /// The user's compatibility settings.
    layers: super::super::layers::tests::Map,
    /// Why Nucleus does not start, when it does not.
    refused: Arc<Mutex<Option<String>>>,
    /// The folder each launch started Nucleus from.
    folders: Arc<Mutex<Vec<PathBuf>>>,
    /// What was written through the launcher's account, and when it ran.
    writes: Arc<Mutex<usize>>,
}
struct FakeNucleus {
    probe: Probe,
    screen: Screen,
}
impl Nucleus for FakeNucleus {
    fn since(&self) -> u64 {
        SINCE
    }
    fn exit_code(&mut self) -> Result<Option<u32>> {
        Ok(*self.probe.exit.lock().unwrap())
    }
    fn job(&mut self) -> Result<BTreeSet<u32>> {
        // Nucleus and every process it started, as its job would hold them.
        let desktop = self.screen.0.lock().unwrap();
        let exited = self.probe.exit.lock().unwrap().is_some();
        Ok(desktop
            .processes
            .iter()
            .filter(|p| (p.pid == NUCLEUS && !exited) || p.parent == NUCLEUS)
            .map(|p| p.pid)
            .collect())
    }
    fn stop(&mut self, instances: &BTreeMap<u32, u64>) {
        *self.probe.stopped.lock().unwrap() = Some(instances.clone());
    }
}
impl Launcher for (Probe, Screen) {
    fn launch(
        &mut self,
        folder: &Path,
        guid: &str,
        environment: &BTreeMap<String, String>,
    ) -> Result<Box<dyn Nucleus>> {
        if let Some(why) = self.0.refused.lock().unwrap().clone() {
            bail!(why);
        }
        self.0.folders.lock().unwrap().push(folder.to_path_buf());
        self.0.launched.lock().unwrap().push(guid.into());
        self.0
            .environments
            .lock()
            .unwrap()
            .push(environment.clone());
        self.1
            .0
            .lock()
            .unwrap()
            .processes
            .push(process(NUCLEUS, 1, SINCE, EXECUTABLE));
        Ok(Box::new(FakeNucleus {
            probe: self.0.clone(),
            screen: self.1.clone(),
        }))
    }
}

fn handler() -> Handler {
    Handler {
        file: "Game.js".into(),
        guid: "Game".into(),
        name: "Game".into(),
        executable: "Game.exe".into(),
        max_players: 4,
        supports_keyboard: false,
        hides_taskbar: false,
        own: false,
        owner: None,
        admin: Vec::new(),
    }
}

fn kind(players: usize) -> coop::Kind {
    if players == 1 {
        coop::Kind::Background
    } else {
        coop::Kind::Coop
    }
}

fn runner(players: usize, screen: &Screen, probe: &Probe, t0: Instant) -> Runner {
    let pads: Vec<u8> = (0..players as u8).collect();
    let session =
        coop::Session::new(1, kind(players), "Game", "Game", &pads, ("phone", "Phone")).unwrap();
    screen
        .0
        .lock()
        .unwrap()
        .processes
        .push(process(NUCLEUS, 1, SINCE, EXECUTABLE));
    Runner {
        shared: Arc::new(Shared {
            session: Mutex::new(session),
            sizes: Mutex::new(vec![(1280, 720); players]),
            roots: Mutex::new(vec![None; players]),
        }),
        desktop: Box::new(screen.clone()),
        nucleus: Box::new(FakeNucleus {
            probe: probe.clone(),
            screen: screen.clone(),
        }),
        keeper: Keeper::new(players, &["Game.exe".into()], r"C:\Nucleus\content", SINCE),
        title: "Game".into(),
        players,
        clicked: false,
        clicked_at: None,
        hid: false,
        closed: BTreeSet::new(),
        boxed: false,
        last_form: None,
        unclicked: None,
        showing: Vec::new(),
        logged: Vec::new(),
        before: BTreeSet::new(),
        shown: false,
        unseen: None,
        crashlog: None,
        game: None,
        reader: None,
        progress: (false, 0, 0),
        windows: vec![None; players],
        deadline: t0 + START_TIMEOUT,
        exited: false,
        failed: 0,
        game_folder: None,
        own_game: None,
        inject_by: None,
        exited_at: None,
        settings: None,
        layers: None,
    }
}

fn screen() -> Screen {
    Screen(Arc::new(Mutex::new(Fake {
        obey: true,
        ..fake(vec![], vec![])
    })))
}

/// Nucleus starts player `slot`'s instance with a window on the monitor.
fn instance(screen: &Screen, slot: u32) {
    let mut desktop = screen.0.lock().unwrap();
    let pid = 20 + slot;
    desktop.processes.push(process(
        pid,
        NUCLEUS,
        SINCE + 1 + u64::from(slot),
        "Game.exe",
    ));
    desktop
        .windows
        .push(window(u64::from(pid) * 10, pid, ON_SCREEN));
}

fn label(screen: &Screen) {
    screen
        .0
        .lock()
        .unwrap()
        .labels
        .push((NUCLEUS, TITLE.into(), 7));
}

const HINT: &str = "open Nucleus Co-op and load the profile Rubylight once";

#[test]
fn the_profile_is_clicked_and_the_session_runs_with_every_window() {
    let t0 = Instant::now();
    let (screen, probe) = (screen(), Probe::default());
    let mut runner = runner(2, &screen, &probe, t0);
    assert_eq!(runner.step(t0), None);
    assert!(screen.0.lock().unwrap().clicks.is_empty());
    label(&screen);
    assert_eq!(runner.step(t0), None);
    assert_eq!(screen.0.lock().unwrap().clicks, vec![7]);
    // Clicked once only.
    assert_eq!(runner.step(t0), None);
    assert_eq!(screen.0.lock().unwrap().clicks.len(), 1);
    instance(&screen, 0);
    assert_eq!(runner.step(t0), None);
    assert_eq!(runner.shared.session.lock().unwrap().phase, Phase::Starting);
    instance(&screen, 1);
    assert_eq!(runner.step(t0), None);
    let session = runner.shared.session.lock().unwrap().clone();
    assert_eq!(session.phase, Phase::Running);
    assert_eq!(session.slots[0].window, Some(200));
    assert_eq!(session.slots[1].window, Some(210));
    assert!(runner.shared.roots.lock().unwrap()[1].is_some_and(|r| r.pid == 21));
    // Both windows were parked beside the desktop.
    let placed = screen.0.lock().unwrap().placed.clone();
    assert!(
        placed
            .iter()
            .any(|(id, to)| *id == 210 && i64::from(to.x) > ON_SCREEN.right())
    );
    // Running, the session never times out.
    assert_eq!(runner.step(t0 + 10 * START_TIMEOUT), None);
}

/// #59: a click Nucleus gets while it still makes its setup screen does
/// not load the profile. While it lists the profile and no instance
/// runs, the profile is clicked again; never once it hides the list to
/// play, nor once an instance runs.
#[test]
fn the_profile_is_clicked_again_while_nucleus_still_lists_it() {
    let t0 = Instant::now();
    let clicks = |screen: &Screen| screen.0.lock().unwrap().clicks.len();
    let (screen, probe) = (screen(), Probe::default());
    let mut runner = runner(1, &screen, &probe, t0);
    label(&screen);
    assert_eq!(runner.step(t0), None);
    assert_eq!(runner.step(t0 + RECLICK - PERIOD), None);
    assert_eq!(clicks(&screen), 1);
    assert_eq!(runner.step(t0 + RECLICK), None);
    assert_eq!(clicks(&screen), 2);
    // Nucleus plays: the list is gone. A list that comes back before
    // the instance is not clicked: that could start the game twice.
    screen.0.lock().unwrap().labels.clear();
    assert_eq!(runner.step(t0 + 2 * RECLICK), None);
    label(&screen);
    assert_eq!(runner.step(t0 + 3 * RECLICK), None);
    assert_eq!(runner.step(t0 + 5 * RECLICK), None);
    assert_eq!(clicks(&screen), 2);
    instance(&screen, 0);
    assert_eq!(runner.step(t0 + 6 * RECLICK), None);
    assert_eq!(runner.shared.session.lock().unwrap().phase, Phase::Running);
    assert_eq!(runner.step(t0 + 8 * RECLICK), None);
    assert_eq!(clicks(&screen), 2);
}

/// What Nucleus 2.4.2 shows when it can't inject ProtoInput's hooks,
/// into a running game ([`generic::INJECT_FAILED`]'s first) or one it
/// starts (its second).
fn refused(text: &str) -> MessageBox {
    MessageBox {
        window: 500,
        caption: "Error".into(),
        text: text.into(),
    }
}

/// #59: a game that refuses ProtoInput's hooks gets Nucleus' box, and
/// Nucleus ends it once the box is answered. Rubylight's own handler's
/// session ends saying so, and the game's next handler injects them the
/// next way: as Nucleus starts it (refused before its process runs),
/// then from one of its threads, then none. A hub handler's game is
/// not kept.
#[test]
fn a_game_that_refuses_the_hooks_ends_the_session_and_gets_the_next_way() {
    let [runtime, startup] = generic::INJECT_FAILED;
    let exe = r"D:\Games\Black Flag\ACBlackFlag.exe";
    let another = "start the stream again, and Rubylight has Nucleus Co-op hook it another way";
    let none = "start the stream again, and Rubylight starts it without them";
    let host = tempfile::tempdir().unwrap();
    // The way it was started, the box Nucleus shows, whether the game's
    // process runs by then, and what Rubylight does next.
    let ladder = [
        (Inject::Runtime, runtime, true, Inject::Startup, another),
        (Inject::Startup, startup, false, Inject::Stealth, another),
        (Inject::Stealth, runtime, true, Inject::None, none),
    ];
    for (way, text, runs, next, then) in ladder {
        for own in [false, true] {
            assert_eq!(files::inject_for(host.path(), exe).unwrap(), way);
            let t0 = Instant::now();
            let (screen, probe) = (screen(), Probe::default());
            let mut runner = runner(1, &screen, &probe, t0);
            runner.own_game = own.then(|| (host.path().to_path_buf(), exe.to_string()));
            label(&screen);
            assert_eq!(runner.step(t0), None);
            if runs {
                instance(&screen, 0);
                assert_eq!(runner.step(t0), None);
                assert_eq!(runner.shared.session.lock().unwrap().phase, Phase::Running);
            }
            screen
                .0
                .lock()
                .unwrap()
                .boxes
                .push((NUCLEUS, refused(text)));
            let end = runner.step(t0 + PERIOD);
            if !own {
                assert_eq!(end, None, "{way:?}");
                continue;
            }
            let why = format!(
                "Game refused Nucleus Co-op's input hooks (it showed \"Error: {text}\"), and Nucleus Co-op ends a game it can't hook: {then}"
            );
            // The start fails; a game that ran crashed.
            let wanted = if runs {
                End::Crashed(why)
            } else {
                End::Failed(why)
            };
            assert_eq!(end, Some(wanted), "{way:?}");
            assert_eq!(files::inject_for(host.path(), exe).unwrap(), next);
        }
    }
    // By its path, in any case; other games keep the first way.
    assert_eq!(
        files::inject_for(host.path(), &exe.to_uppercase()).unwrap(),
        Inject::None
    );
    assert_eq!(
        files::inject_for(host.path(), r"D:\Games\Other\Game.exe").unwrap(),
        Inject::Runtime
    );

    // Nucleus injects into the running game right after it starts: a
    // minute after its window shows, the boxes are no longer looked
    // through for it.
    let t0 = Instant::now();
    let host = tempfile::tempdir().unwrap();
    let (screen, probe) = (screen(), Probe::default());
    let mut runner = runner(1, &screen, &probe, t0);
    runner.own_game = Some((host.path().to_path_buf(), exe.to_string()));
    label(&screen);
    assert_eq!(runner.step(t0), None);
    instance(&screen, 0);
    assert_eq!(runner.step(t0), None);
    screen
        .0
        .lock()
        .unwrap()
        .boxes
        .push((NUCLEUS, refused(runtime)));
    assert_eq!(runner.step(t0 + START_TIMEOUT), None);
    assert_eq!(
        files::inject_for(host.path(), exe).unwrap(),
        Inject::Runtime
    );
}

/// #59: a game Nucleus fails to inject into as it starts it runs
/// suspended for a moment, is ended, and only then does Nucleus show its
/// box: the instance that exited waits for the box, and the game gets
/// the next way. Without a box, it exited. A hub handler's instance is
/// not held.
#[test]
fn an_instance_ended_before_the_startup_box_shows_is_a_refusal() {
    let [_, startup] = generic::INJECT_FAILED;
    let exe = r"D:\Games\Black Flag\ACBlackFlag.exe";
    let suspended = |screen: &Screen| {
        screen
            .0
            .lock()
            .unwrap()
            .processes
            .push(process(20, NUCLEUS, SINCE + 1, "Game.exe"));
    };
    let ended = |screen: &Screen| screen.0.lock().unwrap().processes.retain(|p| p.pid != 20);
    for (own, box_shows) in [(true, true), (true, false), (false, false)] {
        let t0 = Instant::now();
        let host = tempfile::tempdir().unwrap();
        files::refuse_inject(host.path(), exe).unwrap();
        let (screen, probe) = (screen(), Probe::default());
        let mut runner = runner(1, &screen, &probe, t0);
        runner.own_game = own.then(|| (host.path().to_path_buf(), exe.to_string()));
        label(&screen);
        assert_eq!(runner.step(t0), None);
        suspended(&screen);
        assert_eq!(runner.step(t0 + PERIOD), None);
        assert_eq!(
            runner.shared.roots.lock().unwrap().iter().flatten().count(),
            1
        );
        ended(&screen);
        let exited = Some(End::Failed("Player 1's instance of Game exited".into()));
        let end = runner.step(t0 + 2 * PERIOD);
        if !own {
            assert_eq!(end, exited);
            continue;
        }
        assert_eq!(end, None);
        if !box_shows {
            assert_eq!(runner.step(t0 + 2 * PERIOD + REFUSAL_GRACE / 2), None);
            assert_eq!(runner.step(t0 + 2 * PERIOD + REFUSAL_GRACE), exited);
            assert_eq!(
                files::inject_for(host.path(), exe).unwrap(),
                Inject::Startup
            );
            continue;
        }
        screen
            .0
            .lock()
            .unwrap()
            .boxes
            .push((NUCLEUS, refused(startup)));
        let end = runner.step(t0 + 3 * PERIOD);
        assert!(
            matches!(&end, Some(End::Failed(why)) if why.contains("hook it another way")),
            "{end:?}"
        );
        assert_eq!(
            files::inject_for(host.path(), exe).unwrap(),
            Inject::Stealth
        );
    }
}

/// #59: a box that was up before Nucleus started is an earlier
/// session's, not this game's.
#[test]
fn a_refused_hooks_box_from_before_the_start_is_not_this_games() {
    let refused = refused(generic::INJECT_FAILED[0]);
    let exe = r"D:\Games\Black Flag\ACBlackFlag.exe";
    let t0 = Instant::now();
    let host = tempfile::tempdir().unwrap();
    let (screen, probe) = (screen(), Probe::default());
    let mut runner = runner(1, &screen, &probe, t0);
    runner.own_game = Some((host.path().to_path_buf(), exe.to_string()));
    runner.before = BTreeSet::from([refused.window]);
    label(&screen);
    assert_eq!(runner.step(t0), None);
    instance(&screen, 0);
    assert_eq!(runner.step(t0), None);
    screen.0.lock().unwrap().boxes.push((NUCLEUS, refused));
    assert_eq!(runner.step(t0 + PERIOD), None);
    assert_eq!(
        files::inject_for(host.path(), exe).unwrap(),
        Inject::Runtime
    );
}

/// #59: the stream of a game started without the hooks it refused every
/// way tells its player that the PC's keyboard, mouse and pads reach it
/// too; one still hooked another way is not told. A hub handler's game,
/// whose handler Rubylight doesn't write, is not told.
#[test]
fn a_game_started_without_the_hooks_it_refused_is_told() {
    let host = tempfile::tempdir().unwrap();
    let exe = r"D:\Games\Black Flag\ACBlackFlag.exe";
    let hub = handler();
    let own = Handler {
        own: true,
        ..handler()
    };
    let start = |handler| Start {
        id: 1,
        kind: kind(1),
        folder: host.path(),
        user_copy: None,
        known_list: true,
        handler,
        exe_path: exe,
        steam_app: None,
        title: "Black Flag",
        players: 1,
        owner: ("phone", "Phone"),
        size: (1280, 720),
    };
    for _ in [Inject::Runtime, Inject::Startup, Inject::Stealth] {
        assert_eq!(unhooked(&start(&own)).unwrap(), None);
        files::refuse_inject(host.path(), exe).unwrap();
    }
    assert_eq!(
        unhooked(&start(&own)).unwrap().as_deref(),
        Some(
            "Black Flag refused every way Nucleus Co-op injects its input hooks, so it runs without them: a keyboard, mouse or pad on the PC reaches it too"
        )
    );
    assert_eq!(unhooked(&start(&hub)).unwrap(), None);
}

/// A box Nucleus shows after the click (its End-key reminder) holds the
/// start: no click goes through it until it is answered.
#[test]
fn a_box_nucleus_shows_holds_the_second_click() {
    let t0 = Instant::now();
    let clicks = |screen: &Screen| screen.0.lock().unwrap().clicks.len();
    let (screen, probe) = (screen(), Probe::default());
    let mut runner = runner(1, &screen, &probe, t0);
    label(&screen);
    assert_eq!(runner.step(t0), None);
    let reminder = MessageBox {
        window: 400,
        caption: "Nucleus Co-op".into(),
        text: "Press End to unlock the inputs.".into(),
    };
    screen.0.lock().unwrap().boxes.push((NUCLEUS, reminder));
    assert_eq!(runner.step(t0 + RECLICK), None);
    assert_eq!(runner.step(t0 + 2 * RECLICK), None);
    assert_eq!(clicks(&screen), 1);
    screen.0.lock().unwrap().boxes.clear();
    assert_eq!(runner.step(t0 + 3 * RECLICK), None);
    assert_eq!(clicks(&screen), 2);
}

/// Looking for the list again after the click goes through the passes
/// that may fail, not ending the start at once.
#[test]
fn a_failed_look_for_the_list_after_the_click_fails_the_pass() {
    let t0 = Instant::now();
    let (screen, probe) = (screen(), Probe::default());
    let mut runner = runner(1, &screen, &probe, t0);
    label(&screen);
    assert_eq!(runner.step(t0), None);
    // Two entries named alike: the fake's lookup fails.
    label(&screen);
    for pass in 1..FAILED_PASSES {
        assert_eq!(runner.step(t0 + pass * PERIOD), None);
    }
    assert_eq!(
        runner.step(t0 + FAILED_PASSES * PERIOD),
        Some(End::Failed(
            "looking for Nucleus Co-op's profile list: two labels".into()
        ))
    );
}

/// #59: Nucleus 2.4.2 answers a click that comes before its setup screen
/// is made with "The profile can't be loaded", a form that disables its
/// window until someone closes it, so no later click lands. The session
/// closes it and clicks the profile again; one Nucleus shows before any
/// click is left alone.
#[test]
fn a_box_nucleus_shows_after_the_click_is_closed_and_the_profile_clicked_again() {
    let t0 = Instant::now();
    let (screen, probe) = (screen(), Probe::default());
    let mut runner = runner(1, &screen, &probe, t0);
    let form = |covered: bool| {
        let mut desktop = screen.0.lock().unwrap();
        desktop.forms.push((NUCLEUS, unloaded(300)));
        desktop.covered = covered;
    };
    form(true);
    assert_eq!(runner.step(t0), None);
    assert!(screen.0.lock().unwrap().closed.is_empty());
    screen.0.lock().unwrap().forms.clear();
    screen.0.lock().unwrap().covered = false;
    label(&screen);
    assert_eq!(runner.step(t0 + PERIOD), None);
    assert_eq!(screen.0.lock().unwrap().clicks.len(), 1);
    // The click failed: the box covers the window.
    form(true);
    assert_eq!(runner.step(t0 + 2 * PERIOD), None);
    assert_eq!(screen.0.lock().unwrap().closed, [300]);
    screen.0.lock().unwrap().covered = false;
    assert_eq!(runner.step(t0 + PERIOD + RECLICK), None);
    assert_eq!(screen.0.lock().unwrap().clicks.len(), 2);
    instance(&screen, 0);
    assert_eq!(runner.step(t0 + PERIOD + RECLICK + PERIOD), None);
    assert_eq!(runner.shared.session.lock().unwrap().phase, Phase::Running);

    // A box after every click ends the start saying so, counting each
    // box once though it takes passes to close.
    let (screen, probe) = (self::screen(), Probe::default());
    let mut runner = self::runner(1, &screen, &probe, t0);
    label(&screen);
    let mut at = t0;
    while at < t0 + START_TIMEOUT {
        let window = 300 + screen.0.lock().unwrap().clicks.len() as u64;
        screen
            .0
            .lock()
            .unwrap()
            .forms
            .push((NUCLEUS, unloaded(window)));
        assert_eq!(runner.step(at), None);
        at += PERIOD;
    }
    let (clicks, closed) = {
        let desktop = screen.0.lock().unwrap();
        let distinct: BTreeSet<u64> = desktop.closed.iter().copied().collect();
        (desktop.clicks.len(), distinct.len())
    };
    assert!(clicks > 2);
    assert!(screen.0.lock().unwrap().closed.len() > closed);
    assert_eq!(
        runner.step(at),
        Some(End::Failed(format!(
            "Nucleus Co-op did not start Game: after the last click on the profile Rubylight it showed \"The profile can't be loaded.\" over its window ({closed} closed since the launch)"
        )))
    );

    // One early box, then a wait for a controller: the wait is why.
    let (screen, probe) = (self::screen(), Probe::default());
    let mut runner = self::runner(1, &screen, &probe, t0);
    label(&screen);
    assert_eq!(runner.step(t0), None);
    screen
        .0
        .lock()
        .unwrap()
        .forms
        .push((NUCLEUS, unloaded(300)));
    assert_eq!(runner.step(t0 + PERIOD), None);
    assert_eq!(runner.step(t0 + RECLICK), None);
    assert_eq!(screen.0.lock().unwrap().clicks.len(), 2);
    assert_eq!(
            runner.step(t0 + RECLICK + START_TIMEOUT),
            Some(End::Failed(
                "Nucleus Co-op did not start Game: it still shows its setup screen with the profile Rubylight, which starts the game once every player has a controller there".into()
            ))
        );
}

/// What Nucleus 2.4.2 shows for a click before its setup screen is made.
fn unloaded(window: u64) -> MessageBox {
    MessageBox {
        window,
        caption: String::new(),
        text: "The profile can't be loaded.".into(),
    }
}

/// #59: a profile Nucleus loaded but never played says so, not to load
/// it by hand: Nucleus plays it once each player has a controller.
#[test]
fn a_profile_nucleus_does_not_play_says_where_it_stopped() {
    let t0 = Instant::now();
    let (screen, probe) = (screen(), Probe::default());
    let mut runner = runner(1, &screen, &probe, t0);
    label(&screen);
    assert_eq!(runner.step(t0), None);
    assert_eq!(runner.step(t0 + RECLICK), None);
    assert_eq!(
            runner.step(t0 + START_TIMEOUT),
            Some(End::Failed(
                "Nucleus Co-op did not start Game: it still shows its setup screen with the profile Rubylight, which starts the game once every player has a controller there".into()
            ))
        );
    assert!(screen.0.lock().unwrap().clicks.len() > 1);
    // It hid the list, and no instance came.
    let (screen, probe) = (self::screen(), Probe::default());
    let mut runner = self::runner(1, &screen, &probe, t0);
    label(&screen);
    assert_eq!(runner.step(t0), None);
    screen.0.lock().unwrap().labels.clear();
    assert_eq!(runner.step(t0 + RECLICK), None);
    assert_eq!(
        runner.step(t0 + START_TIMEOUT),
        Some(End::Failed(
            "Nucleus Co-op did not start Game after loading the profile Rubylight".into()
        ))
    );
}

#[test]
fn no_profile_entry_in_time_fails_says_what_to_do_and_stops_nucleus() {
    let t0 = Instant::now();
    let (screen, probe) = (screen(), Probe::default());
    let mut runner = runner(1, &screen, &probe, t0);
    assert_eq!(runner.step(t0 + START_TIMEOUT / 2), None);
    let end = runner.step(t0 + START_TIMEOUT).unwrap();
    assert_eq!(end, End::Failed("Nucleus Co-op showed no window".into()));
    let shared = runner.shared.clone();
    runner.finish(end.clone());
    assert_eq!(shared.session.lock().unwrap().phase, Phase::Ended(end));
    assert_eq!(*probe.stopped.lock().unwrap(), Some(BTreeMap::new()));
}

#[test]
fn a_message_box_nucleus_shows_instead_of_the_profile_is_named() {
    // Nucleus' check of a Documents folder in OneDrive waits for an
    // answer before its window opens; nobody gives it during a
    // background stream (#59).
    let t0 = Instant::now();
    let (screen, probe) = (screen(), Probe::default());
    let mut runner = runner(1, &screen, &probe, t0);
    let shown = MessageBox {
        window: 1,
        caption: "OneDrive must be disabled".into(),
        text: "Your Documents folder\r\nis in OneDrive.".into(),
    };
    screen
        .0
        .lock()
        .unwrap()
        .boxes
        .push((NUCLEUS, shown.clone()));
    // Another program's box is not Nucleus'.
    screen.0.lock().unwrap().boxes.push((
        99,
        MessageBox {
            window: 2,
            caption: "Other".into(),
            text: String::new(),
        },
    ));
    assert_eq!(runner.step(t0), None);
    assert_eq!(runner.step(t0), None);
    assert_eq!(runner.logged, [shown]);
    assert_eq!(
            runner.step(t0 + START_TIMEOUT),
            Some(End::Failed(
                "Nucleus Co-op shows \"OneDrive must be disabled: Your Documents folder is in OneDrive.\" on the PC's screen".into()
            ))
        );
}

#[test]
fn a_window_without_the_profile_is_named_with_the_sessions_crash() {
    let t0 = Instant::now();
    let (screen, probe) = (screen(), Probe::default());
    let mut runner = runner(1, &screen, &probe, t0);
    let folder = tempfile::tempdir().unwrap();
    let log = folder.path().join("crashlog.txt");
    std::fs::write(&log, "[01.10.2026 10:00:00] System.Exception: earlier\n").unwrap();
    runner.crashlog = Some(Crashlog::at_launch(folder.path(), None));
    let mut later = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
    std::io::Write::write_all(
        &mut later,
        b"[29.10.2026 12:00:00] System.NullReferenceException: no game\n   at ChangeGame()\n",
    )
    .unwrap();
    screen
        .0
        .lock()
        .unwrap()
        .windows
        .push(window(5, NUCLEUS, ON_SCREEN));
    assert_eq!(runner.step(t0), None);
    assert_eq!(
        runner.step(t0 + START_TIMEOUT),
        Some(End::Failed(format!(
            "Nucleus Co-op shows no profile Rubylight for Game; {HINT}. Its crashlog.txt says: [29.10.2026 12:00:00] System.NullReferenceException: no game"
        )))
    );
}

#[test]
fn a_game_nucleus_does_not_list_is_named_with_its_list() {
    let t0 = Instant::now();
    let (screen, probe) = (screen(), Probe::default());
    let folder = tempfile::tempdir().unwrap();
    let exe = folder.path().join("Game.exe");
    std::fs::write(&exe, b"MZ").unwrap();
    let setup =
        butterpollo_core::nucleus::generic::Setup::new(&exe.to_string_lossy(), "Game", None);
    let own = files::write_own(folder.path(), &setup).unwrap();
    screen
        .0
        .lock()
        .unwrap()
        .windows
        .push(window(5, NUCLEUS, ON_SCREEN));
    let timeout = || {
        let mut runner = runner(1, &screen, &probe, t0);
        runner.game = Some((folder.path().to_path_buf(), own.guid.clone()));
        assert_eq!(runner.step(t0), None);
        runner.step(t0 + START_TIMEOUT)
    };
    // Listed: the profile is what's missing.
    assert_eq!(
        timeout(),
        Some(End::Failed(format!(
            "Nucleus Co-op shows no profile Rubylight for Game; {HINT}"
        )))
    );
    // Not in the list Nucleus reads (#59).
    let list = files::list(folder.path());
    std::fs::remove_file(&list).unwrap();
    assert_eq!(
        timeout(),
        Some(End::Failed(format!(
            "Nucleus Co-op does not list Game: {} has no entry for it with its handler and an .exe that is there",
            list.display()
        )))
    );
}

#[test]
fn only_crashes_written_after_the_launch_are_quoted() {
    let folder = tempfile::tempdir().unwrap();
    let log = folder.path().join("crashlog.txt");
    // No file at the launch or after it.
    let none = Crashlog::at_launch(folder.path(), None);
    assert_eq!(none.from, 0);
    assert_eq!(none.newest(None), None);
    std::fs::write(&log, "[1] System.Exception: before\n   at Old()\n").unwrap();
    let crashlog = Crashlog::at_launch(folder.path(), None);
    assert_eq!(crashlog.newest(None), None);
    let append = |text: &str| {
        let mut file = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
        std::io::Write::write_all(&mut file, text.as_bytes()).unwrap();
    };
    append("[2] System.Exception: first\n   at A()\n[3] System.Exception: second\n   at B()\n\n");
    assert_eq!(
        crashlog.newest(None).as_deref(),
        Some("[3] System.Exception: second")
    );
    // A file started again since is read whole.
    std::fs::write(&log, "[4] new\n").unwrap();
    assert_eq!(crashlog.newest(None).as_deref(), Some("[4] new"));
    // A long line is cut.
    append(&format!("[5] {}\n", "x".repeat(400)));
    let cut = Crashlog::at_launch(folder.path(), None);
    append(&format!("[6] {}\n", "y".repeat(400)));
    let line = cut.newest(None).unwrap();
    assert!(
        line.starts_with("[6] yyy") && line.ends_with("..."),
        "{line}"
    );
    assert_eq!(line.chars().count(), Crashlog::LIMIT + 3);
    // None written since the launch: nothing is quoted.
    assert_eq!(Crashlog::at_launch(folder.path(), None).newest(None), None);
    // Of a huge one, only its end is read, where the newest is.
    let huge = Crashlog::at_launch(folder.path(), None);
    append(&"   at Deep()\n".repeat(1 << 16));
    append("[7] System.Exception: last\n   at C()\n");
    assert_eq!(
        huge.newest(None).as_deref(),
        Some("[7] System.Exception: last")
    );
    assert!(huge.since_launch().unwrap().len() as u64 <= Crashlog::TAIL);
}

#[test]
fn a_users_crashlog_is_read_as_them() {
    // Standing in for the user: refused what is granted to this account
    // by name.
    let user = crate::user_folder::tests::without_own_sid();
    let sid = crate::process::own_sid().unwrap();
    let data = tempfile::tempdir().unwrap();
    let theirs = data.path().join("theirs");
    crate::process::create_folder(&theirs, "D:P(A;OICI;FA;;;WD)").unwrap();
    let crashlog = Crashlog::at_launch(&theirs, Some(&user));
    std::fs::write(
        theirs.join("crashlog.txt"),
        "[1] System.Exception: theirs\n",
    )
    .unwrap();
    assert_eq!(
        crashlog.newest(Some(&user)).as_deref(),
        Some("[1] System.Exception: theirs")
    );
    // What the user can't read, the host doesn't read for them, though
    // it could.
    let hidden = data.path().join("hidden");
    crate::process::create_folder(&hidden, &format!("D:P(A;OICI;FA;;;{sid})")).unwrap();
    let crashlog = Crashlog::at_launch(&hidden, Some(&user));
    std::fs::write(
        hidden.join("crashlog.txt"),
        "[1] System.Exception: hidden\n",
    )
    .unwrap();
    assert_eq!(crashlog.newest(Some(&user)), None);
    assert_eq!(
        crashlog.newest(None).as_deref(),
        Some("[1] System.Exception: hidden")
    );
}

#[test]
fn a_covered_profile_list_is_named_when_it_times_out() {
    let t0 = Instant::now();
    let (screen, probe) = (screen(), Probe::default());
    let mut runner = runner(1, &screen, &probe, t0);
    label(&screen);
    screen.0.lock().unwrap().covered = true;
    assert_eq!(runner.step(t0), None);
    // Tried again on every pass.
    assert_eq!(runner.step(t0), None);
    assert_eq!(
        runner.step(t0 + START_TIMEOUT),
        Some(End::Failed(
            "Nucleus Co-op did not start Game: covered".into()
        ))
    );
}

fn reminder() -> MessageBox {
    MessageBox {
        window: 3,
        caption: "Nucleus Co-op".into(),
        text: "Press the End key to unlock the input.".into(),
    }
}
const REMINDED: &str = "Nucleus Co-op shows \"Nucleus Co-op: Press the End key to unlock the input.\" on the PC's screen";

#[test]
fn only_a_box_windows_shows_after_the_launch_is_named() {
    use super::super::instances::tests::CSRSS;
    let t0 = Instant::now();
    let (screen, probe) = (screen(), Probe::default());
    let other = MessageBox {
        window: 4,
        caption: "Explorer.EXE - Application Error".into(),
        text: "The instruction referenced memory that could not be read.".into(),
    };
    screen.0.lock().unwrap().boxes.push((CSRSS, other.clone()));
    let mut runner = runner(1, &screen, &probe, t0);
    // What start keeps: the boxes up before Nucleus Co-op ran.
    runner.before = screen
        .clone()
        .message_boxes(&BTreeSet::new())
        .unwrap()
        .into_iter()
        .map(|shown| shown.window)
        .collect();
    screen
        .0
        .lock()
        .unwrap()
        .windows
        .push(window(5, NUCLEUS, ON_SCREEN));
    assert_eq!(runner.step(t0), None);
    assert_eq!(runner.logged, []);
    // Windows shows one for Nucleus Co-op (DefaultDesktopOnly) later.
    let unreadable = MessageBox {
        window: 6,
        caption: "Nucleus Co-op".into(),
        text: "Unable to load settings.".into(),
    };
    screen
        .0
        .lock()
        .unwrap()
        .boxes
        .push((CSRSS, unreadable.clone()));
    assert_eq!(runner.step(t0), None);
    assert_eq!(runner.logged, [unreadable]);
    assert_eq!(
        runner.step(t0 + START_TIMEOUT),
        Some(End::Failed(
            "Nucleus Co-op shows \"Nucleus Co-op: Unable to load settings.\" on the PC's screen"
                .into()
        ))
    );
}

#[test]
fn a_start_that_cannot_list_the_boxes_up_before_nucleus_is_refused() {
    let folder = tempfile::tempdir().unwrap();
    let (screen, probe) = (screen(), Probe::default());
    screen.0.lock().unwrap().unlisted = true;
    let (running, _) = start_with(folder.path(), 1, [false; 4], &screen, &probe);
    let error = format!("{:#}", running.err().unwrap());
    assert!(
        error.contains("message boxes on the PC's screen before Nucleus Co-op starts"),
        "{error}"
    );
    assert!(probe.launched.lock().unwrap().is_empty());
}

#[test]
fn a_box_covering_the_profile_list_is_named_instead_of_the_cover() {
    let t0 = Instant::now();
    let (screen, probe) = (screen(), Probe::default());
    let mut runner = runner(1, &screen, &probe, t0);
    label(&screen);
    {
        let mut desktop = screen.0.lock().unwrap();
        desktop.covered = true;
        desktop.boxes.push((NUCLEUS, reminder()));
    }
    assert_eq!(runner.step(t0), None);
    assert_eq!(runner.logged, [reminder()]);
    assert_eq!(
        runner.step(t0 + START_TIMEOUT),
        Some(End::Failed(REMINDED.into()))
    );
}

#[test]
fn a_box_nucleus_shows_after_the_click_is_named() {
    let t0 = Instant::now();
    let (screen, probe) = (screen(), Probe::default());
    let mut runner = runner(1, &screen, &probe, t0);
    label(&screen);
    assert_eq!(runner.step(t0), None);
    assert_eq!(screen.0.lock().unwrap().clicks, vec![7]);
    screen.0.lock().unwrap().boxes.push((NUCLEUS, reminder()));
    assert_eq!(runner.step(t0), None);
    assert_eq!(runner.logged, [reminder()]);
    assert_eq!(
        runner.step(t0 + START_TIMEOUT),
        Some(End::Failed(REMINDED.into()))
    );
}

#[test]
fn a_box_gone_from_the_list_is_not_quoted_and_a_failed_list_is_named() {
    let t0 = Instant::now();
    let (screen, probe) = (screen(), Probe::default());
    let mut runner = runner(1, &screen, &probe, t0);
    screen.0.lock().unwrap().boxes.push((NUCLEUS, reminder()));
    assert_eq!(runner.step(t0), None);
    assert_eq!(runner.showing, [reminder()]);
    // The box may be gone; a list that failed doesn't say it's still up.
    screen.0.lock().unwrap().unlisted = true;
    assert_eq!(runner.step(t0), None);
    assert_eq!(runner.showing, []);
    assert_eq!(
        runner.step(t0 + START_TIMEOUT),
        Some(End::Failed(
            "Nucleus Co-op's windows could not be listed: access denied".into()
        ))
    );
}

#[test]
fn each_step_nucleus_makes_gives_it_the_timeout_again() {
    let t0 = Instant::now();
    let (screen, probe) = (screen(), Probe::default());
    let mut runner = runner(2, &screen, &probe, t0);
    label(&screen);
    let clicked = t0 + START_TIMEOUT / 2;
    assert_eq!(runner.step(clicked), None);
    // Past the first deadline, but the click came since.
    let first = clicked + START_TIMEOUT / 2 + PERIOD;
    instance(&screen, 0);
    assert_eq!(runner.step(first), None);
    assert_eq!(runner.step(first + START_TIMEOUT - PERIOD), None);
    assert_eq!(
        runner.step(first + START_TIMEOUT),
        Some(End::Failed(
            "Nucleus Co-op started 1 of 2 instances of Game".into()
        ))
    );
}

#[test]
fn an_instance_without_a_window_is_named() {
    let t0 = Instant::now();
    let (screen, probe) = (screen(), Probe::default());
    let mut runner = runner(1, &screen, &probe, t0);
    label(&screen);
    instance(&screen, 0);
    screen.0.lock().unwrap().windows.clear();
    assert_eq!(runner.step(t0), None);
    assert_eq!(
        runner.step(t0 + START_TIMEOUT),
        Some(End::Failed(
            "Player 1's instance of Game showed no window".into()
        ))
    );
}

#[test]
fn nucleus_exiting_before_the_game_fails_with_its_code() {
    let t0 = Instant::now();
    let (screen, probe) = (screen(), Probe::default());
    let mut runner = runner(1, &screen, &probe, t0);
    assert_eq!(runner.step(t0), None);
    *probe.exit.lock().unwrap() = Some(3);
    assert_eq!(
        runner.step(t0),
        Some(End::Failed(
            "Nucleus Co-op exited with code 3 before starting Game".into()
        ))
    );
}

#[test]
fn nucleus_exiting_after_the_instances_started_leaves_them_running() {
    let t0 = Instant::now();
    let (screen, probe) = (screen(), Probe::default());
    let mut runner = runner(1, &screen, &probe, t0);
    label(&screen);
    instance(&screen, 0);
    assert_eq!(runner.step(t0), None);
    *probe.exit.lock().unwrap() = Some(0);
    assert_eq!(runner.step(t0), None);
    assert_eq!(runner.step(t0), None);
    assert_eq!(runner.shared.session.lock().unwrap().phase, Phase::Running);
}

#[test]
fn an_instance_that_exits_ends_the_session_and_everything_stops() {
    let t0 = Instant::now();
    let (screen, probe) = (screen(), Probe::default());
    let mut runner = runner(2, &screen, &probe, t0);
    label(&screen);
    instance(&screen, 0);
    instance(&screen, 1);
    assert_eq!(runner.step(t0), None);
    let shared = runner.shared.clone();
    assert_eq!(shared.session.lock().unwrap().phase, Phase::Running);
    screen.0.lock().unwrap().processes.retain(|p| p.pid != 21);
    let end = runner.step(t0).unwrap();
    assert_eq!(
        end,
        End::Crashed("Player 2's instance of Game exited".into())
    );
    runner.finish(end.clone());
    assert_eq!(shared.session.lock().unwrap().phase, Phase::Ended(end));
    // Player 1's instance is stopped with Nucleus.
    assert_eq!(
        *probe.stopped.lock().unwrap(),
        Some(BTreeMap::from([(20, SINCE + 1)]))
    );
}

#[test]
fn an_instance_that_exits_while_starting_fails_the_launch() {
    let t0 = Instant::now();
    let (screen, probe) = (screen(), Probe::default());
    let mut runner = runner(2, &screen, &probe, t0);
    label(&screen);
    instance(&screen, 0);
    assert_eq!(runner.step(t0), None);
    screen.0.lock().unwrap().processes.retain(|p| p.pid != 20);
    assert_eq!(
        runner.step(t0),
        Some(End::Failed("Player 1's instance of Game exited".into()))
    );
}

#[test]
fn a_failed_start_walks_the_game_folder_again() {
    use super::super::elevation;
    let t0 = Instant::now();
    let game = tempfile::tempdir().unwrap();
    // A walk in the clock tick the folder was made in is not kept.
    elevation::next_tick();
    elevation::folder_need(game.path(), None).unwrap();
    assert!(elevation::is_kept(game.path()));
    let (screen, probe) = (screen(), Probe::default());
    // A session stopped, or one that crashed once it ran: it stays.
    for end in [End::Ended, End::Crashed("kicked".into())] {
        let mut runner = runner(1, &screen, &probe, t0);
        runner.game_folder = Some(game.path().to_path_buf());
        runner.finish(end);
        assert!(elevation::is_kept(game.path()));
    }
    // One that failed to start: walked again next time.
    let mut runner = runner(1, &screen, &probe, t0);
    runner.game_folder = Some(game.path().to_path_buf());
    runner.finish(End::Failed("Nucleus Co-op did not start Game".into()));
    assert!(!elevation::is_kept(game.path()));
    // Nor one Nucleus never started for.
    elevation::folder_need(game.path(), None).unwrap();
    assert!(elevation::is_kept(game.path()));
    let exe = game.path().join("Game.exe");
    let folder = tempfile::tempdir().unwrap();
    *probe.refused.lock().unwrap() = Some("access denied".into());
    let (running, _) = start_steam(
        folder.path(),
        1,
        [false; 4],
        &self::screen(),
        &probe,
        None,
        exe.to_str().unwrap(),
    );
    assert_eq!(running.err().unwrap().to_string(), "access denied");
    assert!(!elevation::is_kept(game.path()));
}

#[test]
fn a_session_ended_by_the_host_keeps_its_reason() {
    let t0 = Instant::now();
    let (screen, probe) = (screen(), Probe::default());
    let mut runner = runner(1, &screen, &probe, t0);
    let shared = runner.shared.clone();
    shared
        .session
        .lock()
        .unwrap()
        .end(End::Crashed("kicked".into()));
    let end = runner.step(t0).unwrap();
    runner.finish(End::Ended);
    assert_eq!(end, End::Crashed("kicked".into()));
    assert_eq!(
        shared.session.lock().unwrap().phase,
        Phase::Ended(End::Crashed("kicked".into()))
    );
    assert!(probe.stopped.lock().unwrap().is_some());
}

#[test]
fn a_window_that_cannot_be_parked_ends_the_session_after_some_passes() {
    let t0 = Instant::now();
    let (screen, probe) = (screen(), Probe::default());
    let mut runner = runner(1, &screen, &probe, t0);
    *runner.shared.sizes.lock().unwrap() = vec![(40000, 720)];
    label(&screen);
    instance(&screen, 0);
    for _ in 1..FAILED_PASSES {
        assert_eq!(runner.step(t0), None);
    }
    let end = runner.step(t0).unwrap();
    assert!(
        matches!(&end, End::Failed(why) if why.contains("does not fit beside the PC's desktop")),
        "{end:?}"
    );
}

#[test]
fn a_good_pass_resets_the_failed_passes() {
    let t0 = Instant::now();
    let (screen, probe) = (screen(), Probe::default());
    let mut runner = runner(1, &screen, &probe, t0);
    label(&screen);
    instance(&screen, 0);
    for _ in 0..2 {
        *runner.shared.sizes.lock().unwrap() = vec![(40000, 720)];
        for _ in 1..FAILED_PASSES {
            assert_eq!(runner.step(t0), None);
        }
        *runner.shared.sizes.lock().unwrap() = vec![(1280, 720)];
        assert_eq!(runner.step(t0), None);
    }
    assert_eq!(runner.shared.session.lock().unwrap().phase, Phase::Running);
}

#[test]
fn several_profile_entries_fail_at_once() {
    let t0 = Instant::now();
    let (screen, probe) = (screen(), Probe::default());
    let mut runner = runner(1, &screen, &probe, t0);
    label(&screen);
    label(&screen);
    assert_eq!(runner.step(t0), Some(End::Failed("two labels".into())));
}

#[test]
fn per_player_audio_needs_build_20348() {
    let limit = audio_limit(Some(19045)).unwrap();
    assert!(
        limit.starts_with(
            "Per-player audio needs Windows build 20348 or newer and this PC runs build 19045"
        ),
        "{limit}"
    );
    assert_eq!(audio_limit(Some(20348)), None);
    assert_eq!(audio_limit(None), None);
}

/// The session's pads: a recorder the test keeps reading.
struct Recorded(Arc<Recorder>);
impl Pads for Recorded {
    fn send(&self, event: Input) {
        self.0.send(event)
    }
    fn wait_ready(&self, mask: u16, timeout: Duration) -> bool {
        self.0.wait_ready(mask, timeout)
    }
    fn reports(&self) -> Vec<PadReport> {
        self.0.reports()
    }
}

/// The game the sessions start.
const GAME: &str = r"D:\Games\Game\Game.exe";

fn start_with(
    folder: &Path,
    players: usize,
    taken: [bool; 4],
    screen: &Screen,
    probe: &Probe,
) -> (Result<Running>, Arc<Recorder>) {
    start_steam(folder, players, taken, screen, probe, None, GAME)
}

impl Launcher for (Probe, Screen, ()) {
    fn launch(
        &mut self,
        folder: &Path,
        guid: &str,
        environment: &BTreeMap<String, String>,
    ) -> Result<Box<dyn Nucleus>> {
        (self.0.clone(), self.1.clone()).launch(folder, guid, environment)
    }
    fn write(&self, work: &mut dyn FnMut() -> Result<()>) -> Result<()> {
        *self.0.writes.lock().unwrap() += 1;
        work()
    }
}

fn start_steam(
    folder: &Path,
    players: usize,
    taken: [bool; 4],
    screen: &Screen,
    probe: &Probe,
    steam_app: Option<u32>,
    exe_path: &str,
) -> (Result<Running>, Arc<Recorder>) {
    let recorder = Arc::new(Recorder::default());
    let handler = handler();
    let mut xinput = FakeXInput {
        pads: &recorder,
        taken,
        hidden: false,
        intruder: false,
    };
    let request = Start {
        id: 1,
        kind: kind(players),
        folder,
        user_copy: None,
        known_list: true,
        handler: &handler,
        exe_path,
        steam_app,
        title: "Game",
        players,
        owner: ("phone", "Phone"),
        size: (1280, 720),
    };
    let running = start(
        &request,
        Box::new(Recorded(recorder.clone())),
        &mut xinput,
        Box::new(screen.clone()),
        Box::new(probe.layers.clone()),
        &mut (probe.clone(), screen.clone()),
    );
    (running, recorder)
}

/// Wait until the session thread got `running` to `phase`.
fn wait_for(running: &Running, phase: impl Fn(&Phase) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !running.with_session(|s| phase(&s.phase)) {
        assert!(
            Instant::now() < deadline,
            "{:?}",
            running.with_session(|s| s.phase.clone())
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_session_starts_runs_and_stops_everything() {
    let folder = tempfile::tempdir().unwrap();
    let (screen, probe) = (screen(), Probe::default());
    label(&screen);
    let (running, recorder) = start_with(
        folder.path(),
        2,
        [false, true, false, false],
        &screen,
        &probe,
    );
    let running = running.unwrap();
    let error = running.capture_target(0, r"\\.\DISPLAY1").unwrap_err();
    assert_eq!(error.to_string(), "Player 1's game window is not open");
    // One pad per player, skipping the PC's own at index 1.
    assert_eq!(
        running.with_session(|s| s.slots.iter().map(|s| s.pad).collect::<Vec<_>>()),
        [0, 2]
    );
    assert_eq!(recorder.sent.lock().unwrap().len(), 2);
    assert_eq!(*probe.launched.lock().unwrap(), ["Game"]);
    // Found from no Steam app: Nucleus gets the user's environment alone.
    assert_eq!(*probe.environments.lock().unwrap(), [BTreeMap::new()]);
    let profile = std::fs::read_to_string(
        folder
            .path()
            .join("game profiles")
            .join("Game")
            .join("Profile[1].json"),
    )
    .unwrap();
    assert!(profile.contains(TITLE), "{profile}");
    assert!(folder.path().join("Settings.ini").is_file());
    // Nucleus loads the profile and starts both instances.
    let deadline = Instant::now() + Duration::from_secs(10);
    while screen.0.lock().unwrap().clicks.is_empty() {
        assert!(Instant::now() < deadline, "the profile was never clicked");
        std::thread::sleep(Duration::from_millis(10));
    }
    instance(&screen, 0);
    instance(&screen, 1);
    wait_for(&running, |p| *p == Phase::Running);
    assert_eq!(running.windows(), [Some(200), Some(210)]);
    // Each player's stream captures that player's window, which capture
    // reads back as the same window and display.
    let target = running.capture_target(1, r"\\.\DISPLAY2").unwrap();
    assert_eq!(target, r"window:210@\\.\DISPLAY2");
    assert_eq!(
        crate::capture::WindowTarget::parse(&target).unwrap(),
        Some(crate::capture::WindowTarget::new(210, r"\\.\DISPLAY2"))
    );
    assert_eq!(
        running.capture_target(0, r"\\.\DISPLAY1").unwrap(),
        r"window:200@\\.\DISPLAY1"
    );
    let error = running.capture_target(2, r"\\.\DISPLAY1").unwrap_err();
    assert_eq!(error.to_string(), "the session has 2 players, not 3");
    // Player 2's game replaced its window: the target follows it.
    {
        let mut desktop = screen.0.lock().unwrap();
        desktop.windows.retain(|w| w.id != 210);
        desktop.windows.push(window(211, 21, ON_SCREEN));
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    while running.capture_target(1, r"\\.\DISPLAY2").ok().as_deref()
        != Some(r"window:211@\\.\DISPLAY2")
    {
        assert!(Instant::now() < deadline, "{:?}", running.windows());
        std::thread::sleep(Duration::from_millis(10));
    }
    running.stop();
    assert_eq!(
        running.with_session(|s| s.phase.clone()),
        Phase::Ended(End::Ended)
    );
    assert_eq!(
        *probe.stopped.lock().unwrap(),
        Some(BTreeMap::from([(20, SINCE + 1), (21, SINCE + 2)]))
    );
}

#[test]
fn a_game_found_from_steam_starts_with_its_app_id() {
    let folder = tempfile::tempdir().unwrap();
    let (screen, probe) = (screen(), Probe::default());
    let (running, _) = start_steam(
        folder.path(),
        1,
        [false; 4],
        &screen,
        &probe,
        Some(242050),
        GAME,
    );
    running.unwrap().stop();
    let app = || "242050".to_string();
    assert_eq!(
        *probe.environments.lock().unwrap(),
        [BTreeMap::from([
            ("SteamAppId".to_string(), app()),
            ("SteamGameId".to_string(), app()),
        ])]
    );
    assert_eq!(steam_environment(None), BTreeMap::new());
}

#[test]
fn a_session_from_the_users_own_copy_writes_and_runs_there() {
    let host = tempfile::tempdir().unwrap();
    let copy = tempfile::tempdir().unwrap();
    // A favourites-only list in the copy, which would hide the game.
    let ini = copy.path().join("Settings.ini");
    std::fs::write(&ini, "[Dev]\r\nShowFavoriteOnly=True\r\n").unwrap();
    let favourites = || {
        butterpollo_core::nucleus::profile::ini_value(
            &std::fs::read(&ini).unwrap(),
            "Dev",
            "ShowFavoriteOnly",
        )
        .unwrap()
    };
    let (screen, probe) = (screen(), Probe::default());
    label(&screen);
    let recorder = Arc::new(Recorder::default());
    let handler = handler();
    let mut xinput = FakeXInput {
        pads: &recorder,
        taken: [false; 4],
        hidden: false,
        intruder: false,
    };
    let request = Start {
        id: 1,
        kind: kind(1),
        folder: host.path(),
        user_copy: Some(copy.path()),
        known_list: true,
        handler: &handler,
        exe_path: r"D:\Games\Game\Game.exe",
        steam_app: None,
        title: "Game",
        players: 1,
        owner: ("phone", "Phone"),
        size: (1280, 720),
    };
    let running = start(
        &request,
        Box::new(Recorded(recorder.clone())),
        &mut xinput,
        Box::new(screen.clone()),
        Box::new(probe.layers.clone()),
        &mut (probe.clone(), screen.clone(), ()),
    )
    .unwrap();
    // Settings and profile in the copy, written as the user; Nucleus
    // started from it; the compatibility settings' file in the host's.
    assert_eq!(*probe.writes.lock().unwrap(), 1);
    assert!(copy.path().join("Settings.ini").is_file());
    assert!(copy.path().join("game profiles").join("Game").is_dir());
    assert!(!host.path().join("Settings.ini").exists());
    assert!(!host.path().join("game profiles").exists());
    assert_eq!(*probe.folders.lock().unwrap(), [copy.path().to_path_buf()]);
    let journal: Vec<_> = std::fs::read_dir(host.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    assert!(
        journal
            .iter()
            .any(|name| name.starts_with("Rubylight-layers-")),
        "{journal:?}"
    );
    // The copy's settings are the session's, written as the user with
    // no journal: nothing in the copy is put back as the host.
    assert_eq!(favourites(), b"False");
    // A start that times out quotes the crashlog of the copy Nucleus
    // runs from, not the host's.
    assert_eq!(running.crashlog, copy.path().join("crashlog.txt"));
    let journals =
        || [host.path(), copy.path()].map(|folder| folder.join("Rubylight-settings.json").exists());
    assert_eq!(journals(), [false, false]);
    running.stop();
    assert_eq!(favourites(), b"False");
    assert_eq!(journals(), [false, false]);
    assert_eq!(*probe.writes.lock().unwrap(), 1);
}

#[test]
fn the_games_compatibility_settings_are_back_after_a_session_or_a_failed_start() {
    use super::super::layers::tests::text;
    let folder = tempfile::tempdir().unwrap();
    let (screen, probe) = (screen(), Probe::default());
    let game = r"D:\Games\Game\Game.exe";
    let set = |value: &str| {
        probe
            .layers
            .values
            .lock()
            .unwrap()
            .insert(game.into(), text(value))
    };
    set("~ RUNASADMIN");
    let ini = folder.path().join("Settings.ini");
    std::fs::write(&ini, "[Dev]\r\nShowFavoriteOnly=True\r\n").unwrap();
    let favourites = || {
        let text = std::fs::read(&ini).unwrap();
        butterpollo_core::nucleus::profile::ini_value(&text, "Dev", "ShowFavoriteOnly").unwrap()
    };
    let (running, _) = start_with(folder.path(), 1, [false; 4], &screen, &probe);
    let running = running.unwrap();
    assert_eq!(favourites(), b"False");
    // Nucleus sets the instance's DPI mode as it starts it.
    set("~ HIGHDPIAWARE");
    running.stop();
    // The user's own Nucleus settings are back too.
    assert_eq!(favourites(), b"True");
    assert_eq!(
        *probe.layers.values.lock().unwrap(),
        BTreeMap::from([(game.to_string(), text("~ RUNASADMIN"))])
    );
    // Nucleus did not start: nothing left behind either.
    *probe.refused.lock().unwrap() = Some("access denied".into());
    let (running, _) = start_with(folder.path(), 1, [false; 4], &self::screen(), &probe);
    assert_eq!(running.err().unwrap().to_string(), "access denied");
    assert_eq!(
        *probe.layers.values.lock().unwrap(),
        BTreeMap::from([(game.to_string(), text("~ RUNASADMIN"))])
    );
    assert_eq!(favourites(), b"True");
    // No journal of the settings is left either.
    let mut files: Vec<_> = std::fs::read_dir(folder.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    files.sort();
    assert_eq!(files, ["Settings.ini", "game profiles"]);
}

#[test]
fn a_session_that_fails_on_its_own_stops_nucleus() {
    let folder = tempfile::tempdir().unwrap();
    let (screen, probe) = (screen(), Probe::default());
    let (running, _) = start_with(folder.path(), 1, [false; 4], &screen, &probe);
    let running = running.unwrap();
    *probe.exit.lock().unwrap() = Some(1);
    wait_for(&running, |p| matches!(p, Phase::Ended(_)));
    drop(running);
    assert!(probe.stopped.lock().unwrap().is_some());
}

#[test]
fn a_running_nucleus_or_too_few_controller_slots_refuse_the_start() {
    let folder = tempfile::tempdir().unwrap();
    let (screen, probe) = (screen(), Probe::default());
    screen
        .0
        .lock()
        .unwrap()
        .processes
        .push(process(42, 1, 1, "nucleuscoop.exe"));
    let error = start_with(folder.path(), 1, [false; 4], &screen, &probe)
        .0
        .err()
        .unwrap()
        .to_string();
    assert!(
        error.starts_with("Nucleus Co-op is already running on the PC (process 42)"),
        "{error}"
    );
    let screen = self::screen();
    let (result, recorder) =
        start_with(folder.path(), 2, [true, true, true, false], &screen, &probe);
    let error = result.err().unwrap().to_string();
    assert!(
        error.starts_with(
            "2 players need 2 free XInput controller slots and the PC's own controllers leave 1"
        ),
        "{error}"
    );
    // Nothing was plugged, written or started.
    assert!(recorder.sent.lock().unwrap().is_empty());
    assert!(!folder.path().join("game profiles").exists());
    assert!(probe.launched.lock().unwrap().is_empty());
}

fn state(id: u16, active: u16, buttons: u32) -> Input {
    Input::Controller {
        id,
        active,
        buttons,
        left_trigger: 0,
        right_trigger: 0,
        sticks: [0; 4],
    }
}

#[test]
fn each_device_reaches_its_own_slot_s_pad_only() {
    let folder = tempfile::tempdir().unwrap();
    let (screen, probe) = (screen(), Probe::default());
    let (running, recorder) = start_with(folder.path(), 2, [false; 4], &screen, &probe);
    let running = running.unwrap();
    assert_eq!(
        running.with_session(|s| s.join("laptop", "Laptop", true)),
        Ok(coop::Joined::Seated(1))
    );
    recorder.sent.lock().unwrap().clear();
    assert_eq!(running.input("laptop", state(0, 1, 0x1000)), Ok(()));
    assert_eq!(running.input("phone", state(0, 1, 0x10)), Ok(()));
    assert_eq!(
        *recorder.sent.lock().unwrap(),
        [state(1, 0b11, 0x1000), state(0, 0b11, 0x10)]
    );
    // Keyboards, second controllers, strangers and away players reach nothing.
    let key = Input::Keyboard {
        key: 0x41,
        modifiers: 0,
        flags: 0,
        down: true,
    };
    assert_eq!(running.input("phone", key), Err(Dropped::KeyboardMouse));
    assert_eq!(
        running.input("phone", state(1, 0b11, 0)),
        Err(Dropped::SecondController)
    );
    assert_eq!(running.input("tv", state(0, 1, 0)), Err(Dropped::Ignored));
    running.with_session(|s| s.leave("laptop", Instant::now()));
    assert_eq!(
        running.input("laptop", state(0, 1, 0)),
        Err(Dropped::Ignored)
    );
    assert_eq!(recorder.sent.lock().unwrap().len(), 2);
    // Pad 1's rumble goes to the laptop's first controller.
    recorder.reports.lock().unwrap().push(PadReport::Feedback {
        id: 1,
        kind: 1,
        data: vec![9],
    });
    assert_eq!(
        running.feedback(),
        [(
            1,
            PadReport::Feedback {
                id: 0,
                kind: 1,
                data: vec![9]
            }
        )]
    );
    running.resize(1, (1920, 1080)).unwrap();
    assert_eq!(running.shared.sizes.lock().unwrap()[1], (1920, 1080));
    assert!(running.resize(2, (1920, 1080)).is_err());
}

#[test]
fn audio_is_refused_on_old_builds_and_before_the_instance_runs() {
    let folder = tempfile::tempdir().unwrap();
    let (screen, probe) = (screen(), Probe::default());
    let running = start_with(folder.path(), 1, [false; 4], &screen, &probe)
        .0
        .unwrap();
    let error = running.audio_on(Some(19045), 0).err().unwrap().to_string();
    assert!(
        error.starts_with("Per-player audio needs Windows build 20348"),
        "{error}"
    );
    let error = running.audio_on(Some(26100), 0).err().unwrap().to_string();
    assert_eq!(error, "Player 1's game instance is not running");
}

/// The archive downloaded beforehand, for the install's own checks.
struct Downloaded(std::path::PathBuf);
impl super::super::install::Fetch for Downloaded {
    fn fetch(&mut self, _url: &str, limit: u64) -> Result<Vec<u8>> {
        let bytes =
            std::fs::read(&self.0).with_context(|| format!("reading {}", self.0.display()))?;
        ensure!(
            bytes.len() as u64 <= limit,
            "{} is too large",
            self.0.display()
        );
        Ok(bytes)
    }
}

fn env<T: std::str::FromStr>(name: &str, default: T) -> Result<T>
where
    T::Err: std::error::Error + Send + Sync + 'static,
{
    std::env::var(name).map_or(Ok(default), |v| {
        v.parse().with_context(|| format!("{name}={v}"))
    })
}

/// A real session on this PC (hw-queue.md, "Nucleus Co-op session"):
/// Nucleus Co-op installed below `RUBYLIGHT_NUCLEUS_DATA` (from
/// `RUBYLIGHT_NUCLEUS_ZIP`) or at `RUBYLIGHT_NUCLEUS_PATH`, the game of
/// handler `RUBYLIGHT_NUCLEUS_HANDLER` started for
/// `RUBYLIGHT_NUCLEUS_PLAYERS`, held `RUBYLIGHT_NUCLEUS_HOLD` seconds and
/// stopped; `RUBYLIGHT_NUCLEUS_STEAM_APP` is the Steam app a game was
/// found from. The user's compatibility settings must be as before
/// afterwards. Run as SYSTEM, like the service.
#[test]
#[ignore = "requires Nucleus Co-op, a handler with its game, the virtual gamepad driver and SYSTEM"]
fn a_session_on_this_pc() -> Result<()> {
    let data = std::path::PathBuf::from(
        std::env::var("RUBYLIGHT_NUCLEUS_DATA").context("set RUBYLIGHT_NUCLEUS_DATA")?,
    );
    let configured = std::env::var("RUBYLIGHT_NUCLEUS_PATH").unwrap_or_default();
    let folder = match std::env::var("RUBYLIGHT_NUCLEUS_ZIP") {
        Ok(zip) => super::super::install::ensure(&data, &configured, &mut Downloaded(zip.into()))?,
        Err(_) => super::super::install::installed(&data, &configured)?
            .context("Nucleus Co-op is not installed: set RUBYLIGHT_NUCLEUS_ZIP")?,
    };
    let steam_app: Option<u32> = std::env::var("RUBYLIGHT_NUCLEUS_STEAM_APP")
        .ok()
        .map(|app| app.parse().context("RUBYLIGHT_NUCLEUS_STEAM_APP"))
        .transpose()?;
    // A hub handler and the game the user added with it. Rubylight's own
    // handler goes through admission: the host's nucleus::tests::
    // a_background_stream_without_a_hub_handler_on_this_pc.
    let file = std::env::var("RUBYLIGHT_NUCLEUS_HANDLER")
        .context("set RUBYLIGHT_NUCLEUS_HANDLER to a hub handler's file")?;
    let path = folder.join("handlers").join(&file);
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let handler =
        butterpollo_core::nucleus::handler::parse(&file, &text).map_err(anyhow::Error::msg)?;
    let exe_path = files::catalog(&folder, true, None)?
        .games
        .into_iter()
        .find(|game| game.guid == handler.guid)
        .with_context(|| format!("add {}'s game in Nucleus Co-op first", handler.name))?
        .exe_path;
    println!("handler {} for {}", handler.file, handler.executable);
    let layers_before = layers::UserHive::signed_in()?.values()?;
    let players = env("RUBYLIGHT_NUCLEUS_PLAYERS", 1usize)?;
    let hold = Duration::from_secs(env("RUBYLIGHT_NUCLEUS_HOLD", 20u64)?);
    let since = crate::process::now();
    let warnings = Arc::new(butterpollo_core::session::Warnings::default());
    let request = Start {
        id: 1,
        kind: kind(players),
        folder: &folder,
        user_copy: None,
        known_list: true,
        handler: &handler,
        exe_path: &exe_path,
        steam_app,
        title: &handler.name,
        players,
        owner: ("hw-queue", "hw-queue"),
        size: (1280, 720),
    };
    let policy = butterpollo_core::input_policy::Policy::resolve(&Default::default())?;
    let running = start_on_pc(&request, policy, warnings.clone())?;
    println!(
        "pads {:?}",
        running.with_session(|s| s.slots.iter().map(|s| s.pad).collect::<Vec<_>>())
    );
    let started = Instant::now();
    loop {
        match running.with_session(|s| s.phase.clone()) {
            Phase::Running => break,
            Phase::Ended(why) => bail!("{why}"),
            Phase::Starting => std::thread::sleep(PERIOD),
        }
    }
    println!(
        "running after {:?}, windows {:?}",
        started.elapsed(),
        running.windows()
    );
    // Each parked window streams through WGC, on the primary display's GPU.
    let display = crate::capture::primary_display().context("no display")?;
    let _com = crate::capture::ComGuard::new()?;
    for slot in 0..players {
        let target = running.capture_target(slot, &display)?;
        let mut capture = crate::capture::Capture::open_for_stream_reported(
            &target,
            "wgc",
            false,
            &Default::default(),
            warnings.clone(),
        )?;
        let deadline = Instant::now() + Duration::from_secs(5);
        let image = loop {
            if let Some(image) = capture.next_gpu()? {
                break image;
            }
            ensure!(
                Instant::now() < deadline,
                "Player {}'s window gave no frame within 5 s",
                slot + 1
            );
            std::thread::sleep(Duration::from_millis(5));
        };
        println!(
            "player {} capture {target}: {} {}x{}",
            slot + 1,
            capture.backend(),
            image.width,
            image.height
        );
    }
    for slot in 0..players {
        match running.audio(slot) {
            Ok(audio) => println!("player {} audio pid {}", slot + 1, audio.identity.pid),
            Err(error) => println!("player {} audio: {error:#}", slot + 1),
        }
    }
    // The first player's pad: A held, then released, through its slot.
    for buttons in [0x1000, 0] {
        running
            .input("hw-queue", state(0, 1, buttons))
            .map_err(|dropped| anyhow::anyhow!("the pad input was dropped: {dropped:?}"))?;
        std::thread::sleep(Duration::from_millis(200));
    }
    println!("pad reports {:?}", running.pads.reports());
    std::thread::sleep(hold);
    let phase = running.with_session(|s| s.phase.clone());
    ensure!(phase == Phase::Running, "after {hold:?}: {phase:?}");
    let stopping = Instant::now();
    running.stop();
    println!("stopped in {:?}", stopping.elapsed());
    for warning in warnings.snapshot() {
        println!("warning {}: {}", warning.code, warning.message);
    }
    let left: Vec<String> = Win32::default()
        .processes()?
        .into_iter()
        .filter(|p| p.started >= since)
        .filter(|p| {
            p.name.eq_ignore_ascii_case(EXECUTABLE)
                || p.name.eq_ignore_ascii_case(&handler.executable)
        })
        .map(|p| format!("{} {}", p.name, p.pid))
        .collect();
    ensure!(left.is_empty(), "still running after the stop: {left:?}");
    let layers_after = layers::UserHive::signed_in()?.values()?;
    ensure!(
        layers_after == layers_before,
        "the compatibility settings changed: {layers_before:?} before, {layers_after:?} after"
    );
    println!("compatibility settings as before");
    Ok(())
}
