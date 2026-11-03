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
}
/// The routes' lock a restore is checked against.
static LISTED: Mutex<State> = Mutex::new(State {
    active: BTreeMap::new(),
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
        Ok(self.device(&self.exe(pid)?, role))
    }
    fn set(&self, pid: u32, role: ERole, device: &str) -> Result<()> {
        let exe = self.exe(pid)?;
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
    assert!(std::fs::read_to_string(unreadable(directory.path()))?.contains("game.exe"));
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

#[test]
fn a_game_that_opened_its_sound_before_the_route_is_reported() {
    let speakers = "{speakers}";
    let sink = "{steam}";
    let elsewhere = [(7, speakers.to_string())];
    assert!(plays_elsewhere(&[7], &elsewhere, sink));
    let both = [(7, speakers.to_string()), (8, sink.to_string())];
    assert!(
        !plays_elsewhere(&[7, 8], &both, sink),
        "the game's sound process is on the sink"
    );
    assert!(
        !plays_elsewhere(&[9], &elsewhere, sink),
        "another app is not the game"
    );
    assert!(!plays_elsewhere(&[7], &[], sink), "silent so far");
}

#[test]
fn policy_factory_abi_matches_windows() {
    let pointer = size_of::<usize>();
    assert_eq!(std::mem::offset_of!(FactoryVtbl, set), (6 + 19) * pointer);
    assert_eq!(std::mem::offset_of!(FactoryVtbl, get), (6 + 20) * pointer);
    assert_eq!(size_of::<FactoryVtbl>(), (6 + 22) * pointer);
}

/// The real factory on this PC, with no change: Windows reads the test's
/// own device back.
#[test]
#[ignore = "needs the Windows audio service"]
fn windows_reads_a_process_device() -> Result<()> {
    let _com = crate::capture::ComGuard::new()?;
    let apps = Windows::new()?;
    let device = apps.get(std::process::id(), eMultimedia)?;
    assert!(
        device.is_empty() || device.starts_with(MMDEVAPI),
        "{device}"
    );
    Ok(())
}
