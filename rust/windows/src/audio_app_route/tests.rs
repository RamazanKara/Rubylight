use super::*;
use std::cell::RefCell;

const SINK: &str = r"\\?\SWD#MMDEVAPI#{steam}#{e6327cad-dcec-4949-ae8a-991e976a79d2}";
const HEADSET: &str = r"\\?\SWD#MMDEVAPI#{headset}#{e6327cad-dcec-4949-ae8a-991e976a79d2}";
const GAME: &str = r"e:\games\lego batman\legobatmanlotdk-win64-shipping.exe";

/// Windows as the routing sees it: each program's own device per role, and
/// the processes running now.
#[derive(Default)]
struct Fake {
    devices: RefCell<BTreeMap<(String, i32), String>>,
    running: RefCell<Vec<(u32, String)>>,
    /// A device Windows refuses (unplugged).
    gone: RefCell<Option<String>>,
    /// Windows answers S_OK and keeps nothing.
    ignores: std::cell::Cell<bool>,
    /// For each process listing, whether [`LISTED`] was free.
    listed: RefCell<Vec<bool>>,
    /// Processes that haven't opened their sound: Windows refuses both calls.
    silent: RefCell<std::collections::BTreeSet<u32>>,
}
/// The routes' lock a restore is checked against.
static LISTED: Mutex<State> = Mutex::new(State {
    active: BTreeMap::new(),
    closing: BTreeMap::new(),
    watching: false,
});
impl Fake {
    fn exe(&self, pid: u32) -> Result<String> {
        self.running
            .borrow()
            .iter()
            .find(|(p, _)| *p == pid)
            .map(|(_, exe)| exe.clone())
            .context("no such process")
    }
    fn device(&self, exe: &str, role: ERole) -> String {
        self.devices
            .borrow()
            .get(&(exe.into(), role.0))
            .cloned()
            .unwrap_or_default()
    }
}
impl Apps for Fake {
    fn get(&self, pid: u32, role: ERole) -> Result<String> {
        let exe = self.exe(pid)?;
        if self.silent.borrow().contains(&pid) {
            return Err(NoSoundYet.into());
        }
        Ok(self.device(&exe, role))
    }
    fn set(&self, pid: u32, role: ERole, device: &str) -> Result<()> {
        let exe = self.exe(pid)?;
        if self.silent.borrow().contains(&pid) {
            return Err(NoSoundYet.into());
        }
        anyhow::ensure!(
            self.gone.borrow().as_deref() != Some(device),
            "element not found"
        );
        if self.ignores.get() {
            return Ok(());
        }
        self.devices
            .borrow_mut()
            .insert((exe, role.0), device.into());
        Ok(())
    }
    fn running(&self, exes: &[&str]) -> Result<Vec<(u32, String)>> {
        self.listed.borrow_mut().push(LISTED.try_lock().is_ok());
        Ok(self
            .running
            .borrow()
            .iter()
            .filter(|(_, exe)| exes.iter().any(|e| same_program(e, exe)))
            .cloned()
            .collect())
    }
}

fn started(fake: &Fake, pid: u32) {
    fake.running.borrow_mut().push((pid, GAME.into()));
}
fn exited(fake: &Fake) {
    fake.running.borrow_mut().clear();
}

#[test]
fn the_game_plays_on_the_sink_and_goes_back_to_the_default_when_the_stream_ends() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let (fake, mut state) = (Fake::default(), State::default());
    started(&fake, 7);
    route_with(&fake, &mut state, directory.path(), 7, GAME, SINK)?;
    for role in ROLES {
        assert_eq!(fake.device(GAME, role), SINK);
    }
    assert!(!release_with(&fake, &mut state, directory.path(), GAME)?);
    for role in ROLES {
        assert_eq!(
            fake.device(GAME, role),
            "",
            "the game follows the default again"
        );
    }
    assert_eq!(load(directory.path())?, Journal::default());
    Ok(())
}

/// A session's end stops its game after every stream that plays it: the
/// game is put back while it runs, not left on the sink until it next
/// starts, and the streams letting go later change nothing.
#[test]
fn a_game_the_host_stops_is_put_back_before_it_stops() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let (fake, mut state) = (Fake::default(), State::default());
    started(&fake, 7);
    fake.set(7, eConsole, HEADSET)?;
    // The session's route and its stream's.
    route_with(&fake, &mut state, directory.path(), 7, GAME, SINK)?;
    route_with(&fake, &mut state, directory.path(), 7, GAME, SINK)?;
    assert!(!put_back_with(
        &fake,
        &mut state,
        directory.path(),
        &[GAME.into()]
    )?);
    assert_eq!(fake.device(GAME, eConsole), HEADSET);
    assert_eq!(fake.device(GAME, eMultimedia), "");
    assert_eq!(load(directory.path())?, Journal::default());
    exited(&fake);
    started(&fake, 9);
    fake.set(9, eMultimedia, HEADSET)?;
    for _ in 0..2 {
        assert!(!release_with(&fake, &mut state, directory.path(), GAME)?);
    }
    assert_eq!(fake.device(GAME, eMultimedia), HEADSET);
    Ok(())
}

/// A journal that can't be read is moved aside, its games kept for a
/// reset by hand, and never stops the next stream's route.
#[test]
fn an_unreadable_record_is_moved_aside() -> Result<()> {
    let directory = tempfile::tempdir()?;
    std::fs::write(
        path(directory.path()),
        b"{\"apps\": [{\"exe\": \"e:\\\\game.exe\"",
    )?;
    let (fake, mut state) = (Fake::default(), State::default());
    started(&fake, 7);
    route_with(&fake, &mut state, directory.path(), 7, GAME, SINK)?;
    assert_eq!(fake.device(GAME, eConsole), SINK);
    assert!(
        std::fs::read_to_string(directory.path().join("audio-app-recovery.unreadable.json"))?
            .contains("game.exe")
    );
    assert_eq!(load(directory.path())?.apps.len(), 1);
    Ok(())
}

/// A restore lists the processes, its slow part, without the routes' lock:
/// a stream's route never waits behind it, and a program a stream routed
/// meanwhile stays that stream's.
#[test]
fn a_restore_lists_the_processes_without_the_lock() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let fake = Fake::default();
    started(&fake, 7);
    route_with(
        &fake,
        &mut LISTED.lock().unwrap(),
        directory.path(),
        7,
        GAME,
        SINK,
    )?;
    assert!(release(&mut LISTED.lock().unwrap(), GAME));
    let (state, restored) = restore_unlocked(&fake, &LISTED, directory.path());
    assert!(!restored?);
    assert_eq!(*fake.listed.borrow(), [true]);
    assert_eq!(fake.device(GAME, eConsole), "");
    drop(state);
    let running = fake.running(&[GAME])?;
    // Routed again between the listing and the restore: left alone.
    route_with(
        &fake,
        &mut LISTED.lock().unwrap(),
        directory.path(),
        7,
        GAME,
        SINK,
    )?;
    let state = LISTED.lock().unwrap();
    assert!(!restore_running(&fake, &state, directory.path(), &running)?);
    assert_eq!(fake.device(GAME, eConsole), SINK);
    Ok(())
}

/// Between the put-back and the game's stop no stream routes it again (a
/// child it starts as it quits); once stopped, it is routed as before.
#[test]
fn a_game_the_host_stops_is_not_routed_again_until_it_stopped() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let (fake, mut state) = (Fake::default(), State::default());
    started(&fake, 7);
    let closing = Closing::mark(&mut state, vec![GAME.into()]);
    assert_eq!(
        route_with(&fake, &mut state, directory.path(), 7, GAME, SINK)?,
        Routed::Closing
    );
    assert_eq!(fake.device(GAME, eConsole), "");
    assert_eq!(load(directory.path())?, Journal::default());
    closing.unmark(&mut state);
    std::mem::forget(closing);
    assert_eq!(
        route_with(&fake, &mut state, directory.path(), 7, GAME, SINK)?,
        Routed::Sink
    );
    Ok(())
}

/// A second unreadable record never replaces the first one's list.
#[test]
fn unreadable_records_are_all_kept() -> Result<()> {
    let directory = tempfile::tempdir()?;
    for _ in 0..2 {
        std::fs::write(path(directory.path()), b"{")?;
        load(directory.path())?;
    }
    assert!(
        directory
            .path()
            .join("audio-app-recovery.unreadable.json")
            .exists()
    );
    assert!(
        directory
            .path()
            .join("audio-app-recovery.unreadable-2.json")
            .exists()
    );
    Ok(())
}

#[test]
fn a_game_that_exited_is_put_back_when_it_next_runs() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let (fake, mut state) = (Fake::default(), State::default());
    started(&fake, 7);
    fake.set(7, eConsole, HEADSET)?;
    route_with(&fake, &mut state, directory.path(), 7, GAME, SINK)?;
    exited(&fake);
    assert!(
        release_with(&fake, &mut state, directory.path(), GAME)?,
        "Windows keeps the sink for the program: the restore waits for it"
    );
    assert!(restore_with(&fake, &state, directory.path())?);
    started(&fake, 9);
    assert!(!restore_with(&fake, &state, directory.path())?);
    assert_eq!(fake.device(GAME, eConsole), HEADSET);
    assert_eq!(fake.device(GAME, eMultimedia), "");
    assert_eq!(load(directory.path())?, Journal::default());
    Ok(())
}

#[test]
fn after_a_crash_the_journal_alone_puts_the_game_back() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let fake = Fake::default();
    started(&fake, 7);
    fake.set(7, eMultimedia, HEADSET)?;
    route_with(
        &fake,
        &mut State::default(),
        directory.path(),
        7,
        GAME,
        SINK,
    )?;
    // The host died: a new one starts with no streams.
    assert!(!restore_with(&fake, &State::default(), directory.path())?);
    assert_eq!(fake.device(GAME, eMultimedia), HEADSET);
    assert_eq!(fake.device(GAME, eConsole), "");
    Ok(())
}

#[test]
fn a_new_stream_before_the_restore_keeps_the_users_device() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let (fake, mut state) = (Fake::default(), State::default());
    started(&fake, 7);
    fake.set(7, eConsole, HEADSET)?;
    route_with(&fake, &mut state, directory.path(), 7, GAME, SINK)?;
    exited(&fake);
    assert!(release_with(&fake, &mut state, directory.path(), GAME)?);
    // Started for the next stream: it reads the sink as the game's device.
    started(&fake, 9);
    route_with(&fake, &mut state, directory.path(), 9, GAME, SINK)?;
    assert_eq!(fake.device(GAME, eConsole), SINK);
    assert!(!release_with(&fake, &mut state, directory.path(), GAME)?);
    assert_eq!(fake.device(GAME, eConsole), HEADSET);
    Ok(())
}

#[test]
fn a_device_the_user_chose_during_the_stream_stays() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let (fake, mut state) = (Fake::default(), State::default());
    started(&fake, 7);
    route_with(&fake, &mut state, directory.path(), 7, GAME, SINK)?;
    fake.set(7, eConsole, HEADSET)?;
    assert!(!release_with(&fake, &mut state, directory.path(), GAME)?);
    assert_eq!(fake.device(GAME, eConsole), HEADSET);
    assert_eq!(fake.device(GAME, eMultimedia), "");
    Ok(())
}

#[test]
fn the_last_of_two_streams_of_one_game_puts_it_back() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let (fake, mut state) = (Fake::default(), State::default());
    started(&fake, 7);
    started(&fake, 8);
    route_with(&fake, &mut state, directory.path(), 7, GAME, SINK)?;
    route_with(&fake, &mut state, directory.path(), 8, GAME, SINK)?;
    assert!(!release_with(&fake, &mut state, directory.path(), GAME)?);
    assert_eq!(
        fake.device(GAME, eConsole),
        SINK,
        "the other stream still plays it"
    );
    assert!(
        !restore_with(&fake, &state, directory.path())?,
        "the watcher leaves a playing game alone"
    );
    assert_eq!(fake.device(GAME, eConsole), SINK);
    assert!(!release_with(&fake, &mut state, directory.path(), GAME)?);
    assert_eq!(fake.device(GAME, eConsole), "");
    Ok(())
}

#[test]
fn a_game_found_on_the_sink_without_a_record_goes_back_to_the_default() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let (fake, mut state) = (Fake::default(), State::default());
    started(&fake, 7);
    // The record was lost while the game was routed.
    fake.set(7, eConsole, &SINK.to_lowercase())?;
    route_with(&fake, &mut state, directory.path(), 7, GAME, SINK)?;
    assert!(!release_with(&fake, &mut state, directory.path(), GAME)?);
    assert_eq!(fake.device(GAME, eConsole), "");
    Ok(())
}

#[test]
fn a_device_that_is_gone_does_not_block_the_restore() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let (fake, mut state) = (Fake::default(), State::default());
    started(&fake, 7);
    fake.set(7, eConsole, HEADSET)?;
    route_with(&fake, &mut state, directory.path(), 7, GAME, SINK)?;
    *fake.gone.borrow_mut() = Some(HEADSET.into());
    assert!(!release_with(&fake, &mut state, directory.path(), GAME)?);
    assert_eq!(fake.device(GAME, eConsole), "", "it follows the default");
    assert_eq!(load(directory.path())?, Journal::default());
    Ok(())
}

#[test]
fn a_device_the_user_chose_is_kept_when_the_game_starts_again() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let (fake, mut state) = (Fake::default(), State::default());
    started(&fake, 7);
    route_with(&fake, &mut state, directory.path(), 7, GAME, SINK)?;
    fake.set(7, eConsole, HEADSET)?;
    exited(&fake);
    assert!(release_with(&fake, &mut state, directory.path(), GAME)?);
    started(&fake, 9);
    assert_eq!(
        route_with(&fake, &mut state, directory.path(), 9, GAME, SINK)?,
        Routed::Users(HEADSET.into())
    );
    assert_eq!(fake.device(GAME, eConsole), HEADSET);
    // The pending entry still puts the other role back.
    assert!(!restore_with(&fake, &state, directory.path())?);
    assert_eq!(fake.device(GAME, eMultimedia), "");
    assert_eq!(fake.device(GAME, eConsole), HEADSET);
    Ok(())
}

#[test]
fn a_route_windows_does_not_keep_fails_loudly_and_leaves_nothing() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let (fake, mut state) = (Fake::default(), State::default());
    started(&fake, 7);
    fake.ignores.set(true);
    let error = route_with(&fake, &mut state, directory.path(), 7, GAME, SINK).unwrap_err();
    assert!(
        format!("{error:#}").contains("not the virtual sink"),
        "{error:#}"
    );
    assert!(state.active.is_empty());
    assert_eq!(load(directory.path())?, Journal::default());
    Ok(())
}

#[test]
fn a_store_game_is_the_same_program_after_an_update() {
    let old = r"c:\program files\windowsapps\studio.game_1.0.2.0_x64__abc123\game\game.exe";
    let new = r"c:\program files\windowsapps\studio.game_1.0.3.0_x64__abc123\game\game.exe";
    let other = r"c:\program files\windowsapps\studio.other_1.0.3.0_x64__abc123\game\game.exe";
    assert!(same_program(old, new));
    assert!(!same_program(old, other));
    assert!(!same_program(
        GAME,
        r"d:\games\lego batman\legobatmanlotdk-win64-shipping.exe"
    ));
}

/// The PC's default devices: speakers for the game, a headset for voice.
fn speakers_and_headset() -> [Option<String>; 3] {
    [
        Some("{speakers}".into()),
        Some("{speakers}".into()),
        Some("{headset}".into()),
    ]
}

#[test]
fn a_game_that_opened_its_sound_before_the_route_is_reported() {
    let speakers = "{speakers}";
    let sink = "{steam}";
    let defaults = speakers_and_headset();
    let found = |pids: &[u32], sessions: &[(u32, String)]| {
        plays_elsewhere(pids, sessions, sink, &defaults, &[]).pids
    };
    let elsewhere = [(7, speakers.to_string())];
    assert_eq!(found(&[7], &elsewhere), [7]);
    let both = [
        (7, speakers.to_string()),
        (7, sink.to_string()),
        (8, sink.to_string()),
    ];
    assert_eq!(
        found(&[7, 8], &both),
        [7],
        "a second sound of the game still on the PC is reported beside one on the sink"
    );
    let on_sink = [(7, sink.to_string()), (8, sink.to_string())];
    assert!(found(&[7, 8], &on_sink).is_empty());
    assert!(
        found(&[9], &elsewhere).is_empty(),
        "another app is not the game"
    );
    assert!(found(&[7], &[]).is_empty(), "silent so far");
}

#[test]
fn voice_chat_on_its_own_communications_device_is_no_warning() {
    let sink = "{steam}";
    let voice = [(7, "{headset}".to_string()), (7, sink.to_string())];
    let found = plays_elsewhere(&[7], &voice, sink, &speakers_and_headset(), &[]);
    assert_eq!(found, OnThePc::default());
    let other = [(7, "{hdmi}".to_string())];
    assert_eq!(
        plays_elsewhere(&[7], &other, sink, &speakers_and_headset(), &[]),
        OnThePc::default(),
        "a device the PC doesn't play the game on is no one's"
    );
}

#[test]
fn the_games_own_device_counts_and_a_shared_communications_device_may_be_voice() {
    let sink = "{steam}";
    let own = [(7, "{hdmi}".to_string())];
    assert_eq!(
        plays_elsewhere(&[7], &own, sink, &speakers_and_headset(), &["{HDMI}", ""]),
        OnThePc {
            pids: vec![7],
            maybe_voice: false
        },
        "the game's own device from before the stream"
    );
    let shared = [
        Some("{speakers}".to_string()),
        Some("{speakers}".to_string()),
        Some("{speakers}".to_string()),
    ];
    let session = [(7, "{speakers}".to_string())];
    assert_eq!(
        plays_elsewhere(&[7], &session, sink, &shared, &[]),
        OnThePc {
            pids: vec![7],
            maybe_voice: true
        },
        "one device for both: the warning must allow for voice chat"
    );
    let on_default_sink = [Some(sink.to_string()), Some(sink.to_string()), None];
    assert_eq!(
        plays_elsewhere(
            &[7],
            &[(7, sink.to_string())],
            sink,
            &on_default_sink,
            &[sink]
        ),
        OnThePc::default(),
        "the sink is never the PC's"
    );
}

#[test]
fn a_per_app_device_names_its_endpoint() {
    assert_eq!(endpoint_id(HEADSET), "{headset}");
    assert_eq!(
        endpoint_id(&HEADSET.to_uppercase()),
        "{HEADSET}",
        "Windows' casing"
    );
    assert_eq!(endpoint_id(""), "");
}

/// Programs starting, as WMI tells them: each start also runs the program
/// in `fake`. `Err` ends the watch, as a broken subscription does.
/// A start of a process in `silent` leaves it without sound until the next
/// look that finds no start. Each look's timeout goes to `waited`.
struct Launches<'a> {
    fake: &'a Fake,
    events: std::collections::VecDeque<Result<Option<(u32, String)>>>,
    silent: &'a [u32],
    waited: Vec<Duration>,
}
impl<'a> Launches<'a> {
    fn new(fake: &'a Fake, events: Vec<Result<Option<(u32, String)>>>) -> Self {
        Self {
            fake,
            events: events.into(),
            silent: &[],
            waited: vec![],
        }
    }
}
impl Starts for Launches<'_> {
    fn next(&mut self, timeout: Duration) -> Result<Option<(u32, String)>> {
        self.waited.push(timeout);
        let event = self
            .events
            .pop_front()
            .unwrap_or_else(|| Err(anyhow::anyhow!("no more starts")))?;
        match &event {
            Some((pid, exe)) => {
                self.fake.running.borrow_mut().push((*pid, exe.clone()));
                if self.silent.contains(pid) {
                    self.fake.silent.borrow_mut().insert(*pid);
                }
            }
            None => self.fake.silent.borrow_mut().clear(),
        }
        Ok(event)
    }
}
/// A game routed to the sink that exited by itself: its restore waits.
fn quit_by_itself(fake: &Fake, directory: &Path) -> Result<Mutex<State>> {
    let mut state = State::default();
    started(fake, 7);
    fake.set(7, eConsole, HEADSET)?;
    route_with(fake, &mut state, directory, 7, GAME, SINK)?;
    exited(fake);
    assert!(release_with(fake, &mut state, directory, GAME)?);
    state.watching = true;
    Ok(Mutex::new(state))
}

#[test]
fn a_game_that_quit_by_itself_is_put_back_as_it_starts() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let fake = Fake::default();
    let state = quit_by_itself(&fake, directory.path())?;
    let mut starts = Launches::new(
        &fake,
        vec![
            Ok(Some((5, r"c:\windows\system32\notepad.exe".to_string()))),
            Ok(Some((9, GAME.to_string()))),
        ],
    );
    let listed = fake.listed.borrow().len();
    watch_starts(&fake, &state, directory.path(), &mut starts)?;
    assert_eq!(fake.device(GAME, eConsole), HEADSET);
    assert_eq!(fake.device(GAME, eMultimedia), "");
    assert_eq!(load(directory.path())?, Journal::default());
    assert!(!state.lock().unwrap().watching, "nothing waits: it ends");
    assert_eq!(
        fake.listed.borrow().len(),
        listed + 1,
        "only the first look lists processes: the start alone found the game"
    );
    Ok(())
}

#[test]
fn a_game_that_started_before_the_watch_is_found_by_its_first_look() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let fake = Fake::default();
    let state = quit_by_itself(&fake, directory.path())?;
    started(&fake, 9);
    let mut starts = Launches::new(&fake, vec![]);
    watch_starts(&fake, &state, directory.path(), &mut starts)?;
    assert_eq!(fake.device(GAME, eConsole), HEADSET);
    assert!(!state.lock().unwrap().watching);
    Ok(())
}

#[test]
fn a_broken_watch_of_starts_leaves_the_watcher_on_for_its_fallback() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let fake = Fake::default();
    let state = quit_by_itself(&fake, directory.path())?;
    let mut starts = Launches::new(&fake, vec![Ok(None), Err(anyhow::anyhow!("WMI stopped"))]);
    let error = watch_starts(&fake, &state, directory.path(), &mut starts).unwrap_err();
    assert_eq!(error.to_string(), "WMI stopped");
    assert!(state.lock().unwrap().watching, "the poll goes on from here");
    assert_eq!(load(directory.path())?.apps.len(), 1, "still waiting");
    assert_eq!(fake.device(GAME, eConsole), SINK);
    Ok(())
}

/// #59: Windows has no device for a process that hasn't opened its sound
/// (every game as it starts): the route changes and journals nothing, says
/// so, and works once the game opened it.
#[test]
fn a_game_without_sound_yet_is_routed_once_it_opened_it() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let (fake, mut state) = (Fake::default(), State::default());
    started(&fake, 7);
    fake.silent.borrow_mut().insert(7);
    let error = route_with(&fake, &mut state, directory.path(), 7, GAME, SINK).unwrap_err();
    assert!(no_sound_yet(&error), "{error:#}");
    assert_eq!(load(directory.path())?, Journal::default());
    assert!(state.active.is_empty());
    fake.silent.borrow_mut().clear();
    assert_eq!(
        route_with(&fake, &mut state, directory.path(), 7, GAME, SINK)?,
        Routed::Sink
    );
    assert_eq!(fake.device(GAME, eConsole), SINK);
    Ok(())
}

/// A game put back as it starts has no sound yet: it is looked at again
/// within [`WATCH`], not at the next sweep, and nothing is reported.
#[test]
fn a_game_that_starts_without_sound_is_put_back_once_it_opened_it() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let fake = Fake::default();
    let state = quit_by_itself(&fake, directory.path())?;
    let mut starts = Launches::new(&fake, vec![Ok(Some((9, GAME.to_string()))), Ok(None)]);
    starts.silent = &[9];
    watch_starts(&fake, &state, directory.path(), &mut starts)?;
    assert_eq!(starts.waited, [SWEEP, WATCH]);
    assert_eq!(fake.device(GAME, eConsole), HEADSET);
    assert_eq!(load(directory.path())?, Journal::default());
    assert!(!state.lock().unwrap().watching);
    Ok(())
}

/// E_INVALIDARG is "no sound yet" only for a process Windows lists no
/// sound of; otherwise it is a failure that says so. Sessions are listed
/// only then.
#[test]
fn windows_refusing_a_process_without_sound_waits_for_it() {
    use windows::Win32::Foundation::{E_ACCESSDENIED, E_INVALIDARG, S_OK};
    let unasked = || -> Result<Vec<u32>> { panic!("sessions listed") };
    assert!(refusal(S_OK, 7, unasked).is_ok());
    let denied = refusal(E_ACCESSDENIED, 7, unasked).unwrap_err();
    assert!(!no_sound_yet(&denied));
    assert!(no_sound_yet(
        &refusal(E_INVALIDARG, 7, || Ok(vec![3, 9])).unwrap_err()
    ));
    let playing = refusal(E_INVALIDARG, 7, || Ok(vec![7])).unwrap_err();
    assert!(!no_sound_yet(&playing));
    assert!(format!("{playing:#}").contains("although the game opened its sound"));
    let unlisted = refusal(E_INVALIDARG, 7, || anyhow::bail!("no audio service")).unwrap_err();
    assert!(!no_sound_yet(&unlisted));
}

#[test]
fn policy_factory_abi_matches_windows() {
    let pointer = size_of::<usize>();
    assert_eq!(std::mem::offset_of!(FactoryVtbl, set), (6 + 19) * pointer);
    assert_eq!(std::mem::offset_of!(FactoryVtbl, get), (6 + 20) * pointer);
    assert_eq!(size_of::<FactoryVtbl>(), (6 + 22) * pointer);
}

/// The real factory on this PC, with no change: Windows has no device for
/// the test before it opens its sound (#59), and reads its own device back
/// after.
#[test]
#[ignore = "needs the Windows audio service"]
fn windows_reads_a_process_device() -> Result<()> {
    let _com = crate::capture::ComGuard::new()?;
    let apps = Windows::new()?;
    let error = apps.get(std::process::id(), eMultimedia).unwrap_err();
    assert!(no_sound_yet(&error), "{error:#}");
    // A shared stream, opened and never started: silent.
    // SAFETY: COM is initialised by `_com`; every interface is owned here.
    let _client = unsafe {
        let enumerator: IMMDeviceEnumerator = windows::Win32::System::Com::CoCreateInstance(
            &MMDeviceEnumerator,
            None,
            windows::Win32::System::Com::CLSCTX_ALL,
        )?;
        let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
        let client: IAudioClient =
            device.Activate(windows::Win32::System::Com::CLSCTX_ALL, None)?;
        let format = client.GetMixFormat()?;
        client.Initialize(AUDCLNT_SHAREMODE_SHARED, 0, 10_000_000, 0, format, None)?;
        client
    };
    let device = apps.get(std::process::id(), eMultimedia)?;
    assert!(
        device.is_empty() || device.starts_with(MMDEVAPI),
        "{device}"
    );
    Ok(())
}
