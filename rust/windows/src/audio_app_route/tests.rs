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
}
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
        self.devices
            .borrow_mut()
            .insert((exe, role.0), device.into());
        Ok(())
    }
    fn running(&self, exes: &[&str]) -> Result<Vec<(u32, String)>> {
        Ok(self
            .running
            .borrow()
            .iter()
            .filter(|(_, exe)| exes.contains(&exe.as_str()))
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
