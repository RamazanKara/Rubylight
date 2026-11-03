//! Game-only audio (a background stream's game, or `audio_game_only`) leaves
//! the PC's default playback device alone, so the game played on the PC's
//! speakers as well as on the client (#59). The game's own process is played
//! on a virtual sink nobody hears instead (Windows' per-app output device, as
//! in Settings > Sound > Volume mixer); process loopback captures it there.
//!
//! Windows keeps an app's output device for its program, after it exits, so
//! the choice is journalled first and put back once the program runs again
//! when it exited mid-stream or the host crashed: right away when a process
//! of it still runs, else as soon as it next starts (a watcher sees
//! processes start through WMI, [`Starts`]; without it, it
//! looks every [`WATCH`]). A device the user picked for the game during the
//! stream stays.
//! Windows sets a device only for a running process, so a host that stops
//! the game itself puts it back first ([`put_back`]); an uninstall can't put
//! back a game that isn't running (Volume mixer's Reset does).
//! Windows has no device for a process that hasn't opened its sound yet: it
//! refuses both calls with E_INVALIDARG ([`NoSoundYet`], #59: every game
//! routed as it started). Once the process opened it, a change moves the
//! sound it plays at once, so a route is tried again until then.

use anyhow::{Context, Result};
use butterpollo_core::config::Config;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    ffi::c_void,
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
    time::{Duration, Instant},
};
use windows::{
    Win32::{Media::Audio::*, System::WinRT::RoGetActivationFactory},
    core::{GUID, HRESULT, HSTRING, IInspectable, IInspectable_Vtbl, Interface},
};

/// The roles a game's stream opens on; communications stays the user's.
const ROLES: [ERole; 2] = [eConsole, eMultimedia];
/// How often a pending restore looks for the program it waits for when
/// process starts can't be watched.
const WATCH: Duration = Duration::from_secs(2);
/// How often a watcher that sees process starts still looks for every
/// program: for a restore that failed, or one a stream did meanwhile.
const SWEEP: Duration = Duration::from_secs(30);
const RENDER_INTERFACE: &str = "#{e6327cad-dcec-4949-ae8a-991e976a79d2}";
const MMDEVAPI: &str = r"\\?\SWD#MMDEVAPI#";

/// Windows.Media.Internal.AudioPolicyConfig's factory (EarTrumpet's and
/// Nucleus Co-op's `IAudioPolicyConfigFactory`): 19 methods this does not
/// call, then the per-process endpoint ones.
#[repr(C)]
struct FactoryVtbl {
    base: IInspectable_Vtbl,
    unused: [usize; 19],
    set: unsafe extern "system" fn(*mut c_void, u32, EDataFlow, ERole, *mut c_void) -> HRESULT,
    get: unsafe extern "system" fn(*mut c_void, u32, EDataFlow, ERole, *mut HSTRING) -> HRESULT,
    clear_all: usize,
}
/// The factory's interface from Windows 10 21H2 (build 21390) on.
#[repr(transparent)]
#[derive(Clone)]
struct Factory(IInspectable);
// SAFETY: FactoryVtbl mirrors the factory's slot layout (the offset test checks it) and Factory is
// a transparent IInspectable, so the IID names an interface with exactly this vtable.
unsafe impl Interface for Factory {
    type Vtable = FactoryVtbl;
    const IID: GUID = GUID::from_u128(0xab3d4648_e242_459f_b02f_541c70306324);
}
/// The same interface under its IID before 21H2.
#[repr(transparent)]
#[derive(Clone)]
struct EarlierFactory(IInspectable);
// SAFETY: as for Factory: the same vtable under the earlier IID.
unsafe impl Interface for EarlierFactory {
    type Vtable = FactoryVtbl;
    const IID: GUID = GUID::from_u128(0x2a59116d_6c4f_45e0_a74f_707e3fef9258);
}

/// The process hasn't opened its sound yet, so Windows has no device for it:
/// try again once it has ([`no_sound_yet`]).
#[derive(Debug)]
pub struct NoSoundYet;
impl std::fmt::Display for NoSoundYet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the game has not opened its sound yet")
    }
}
impl std::error::Error for NoSoundYet {}
/// Whether `error` is [`NoSoundYet`]: the route is tried again later.
pub fn no_sound_yet(error: &anyhow::Error) -> bool {
    error.is::<NoSoundYet>()
}
/// Windows' answer `result` for process `pid`: E_INVALIDARG is
/// [`NoSoundYet`] when `sessions` (asked only then) lists no sound of it.
fn refusal(result: HRESULT, pid: u32, sessions: impl FnOnce() -> Result<Vec<u32>>) -> Result<()> {
    if result != windows::Win32::Foundation::E_INVALIDARG {
        return Ok(result.ok()?);
    }
    match sessions() {
        Ok(pids) if !pids.contains(&pid) => Err(NoSoundYet.into()),
        Ok(_) => Err(windows::core::Error::from(result))
            .context("Windows refuses the game's audio device although the game opened its sound"),
        Err(error) => Err(windows::core::Error::from(result))
            .context(format!("its sound can't be listed: {error:#}")),
    }
}

/// The calls the routing needs, so the journal logic runs against a fake.
trait Apps {
    /// The process's own playback device for `role`: "" when it follows
    /// the default.
    fn get(&self, pid: u32, role: ERole) -> Result<String>;
    /// "" puts the process back on the default.
    fn set(&self, pid: u32, role: ERole, device: &str) -> Result<()>;
    /// Running processes of the programs `exes` ([`same_program`]), with
    /// their program paths in lower case.
    fn running(&self, exes: &[&str]) -> Result<Vec<(u32, String)>>;
}

struct Windows(Factory);
impl Windows {
    fn new() -> Result<Self> {
        let class = HSTRING::from("Windows.Media.Internal.AudioPolicyConfig");
        // SAFETY: RoGetActivationFactory only reads the class name and returns an owned interface
        // or an error; COM is initialised by the caller.
        let factory = unsafe {
            RoGetActivationFactory::<Factory>(&class)
                .or_else(|_| RoGetActivationFactory::<EarlierFactory>(&class).map(|f| Factory(f.0)))
        }
        .context("Windows' per-app audio device settings are unavailable")?;
        Ok(Self(factory))
    }
}
impl Apps for Windows {
    fn get(&self, pid: u32, role: ERole) -> Result<String> {
        let mut device = HSTRING::new();
        // SAFETY: `self.0` is a live factory whose slots match FactoryVtbl; `device` receives an
        // owned HSTRING.
        let result =
            unsafe { (self.0.vtable().get)(self.0.as_raw(), pid, eRender, role, &mut device) };
        refusal(result, pid, sound_pids)?;
        Ok(device.to_string_lossy())
    }
    fn set(&self, pid: u32, role: ERole, device: &str) -> Result<()> {
        let device = HSTRING::from(device);
        // SAFETY: the HSTRING is borrowed for the call (an empty one is null, which clears) and
        // outlives it; the callee does not take ownership.
        let result = unsafe {
            let raw: *mut c_void = std::mem::transmute_copy(&device);
            (self.0.vtable().set)(self.0.as_raw(), pid, eRender, role, raw)
        };
        refusal(result, pid, sound_pids)
    }
    fn running(&self, exes: &[&str]) -> Result<Vec<(u32, String)>> {
        let names: Vec<&str> = exes.iter().map(|exe| file_name(exe)).collect();
        Ok(crate::process::processes()?
            .into_iter()
            .filter(|p| names.iter().any(|n| p.name.eq_ignore_ascii_case(n)))
            .filter_map(|p| Some((p.pid, crate::process::image_path(p.pid)?.to_lowercase())))
            .filter(|(_, path)| exes.iter().any(|exe| same_program(exe, path)))
            .collect())
    }
}
/// Processes as they start, so a pending restore runs before the program
/// is likely to open its sound; a fake in the tests.
trait Starts {
    /// The next process to start within `timeout`, with its program path in
    /// lower case; `None` when none did.
    fn next(&mut self, timeout: Duration) -> Result<Option<(u32, String)>>;
}

/// [`Starts`] from WMI's process start trace, which needs an administrator,
/// as the host runs. On the thread that made it, which has COM initialised.
struct WindowsStarts(crate::wmi::Events);
impl WindowsStarts {
    fn subscribe() -> Result<Self> {
        let wmi = crate::wmi::Wmi::connect(r"root\cimv2")?;
        Ok(Self(wmi.events("SELECT * FROM Win32_ProcessStartTrace")?))
    }
}
impl Starts for WindowsStarts {
    fn next(&mut self, timeout: Duration) -> Result<Option<(u32, String)>> {
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let Some(event) = self.0.next(left)? else {
                return Ok(None);
            };
            // One that already ended has no device to put back.
            if let Some(pid) = crate::wmi::number(&event, "ProcessID")?
                && let Some(exe) = crate::process::image_path(pid)
            {
                return Ok(Some((pid, exe.to_lowercase())));
            }
        }
    }
}

fn file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}
/// A Store game's package family (`Name_PublisherId` from
/// `...\WindowsApps\Name_Version_Arch__PublisherId\...`) and the rest of
/// its path: an update changes the version in the folder name.
fn packaged(exe: &str) -> Option<(String, &str)> {
    let (_, inside) = exe.split_once(r"\windowsapps\")?;
    let (folder, rest) = inside.split_once('\\')?;
    let parts: Vec<&str> = folder.split('_').collect();
    let (name, publisher) = (parts.first()?, parts.last()?);
    (parts.len() >= 3).then(|| (format!("{name}_{publisher}"), rest))
}
/// Whether two lower-case program paths are one program: the same path,
/// or the same Store package's program in another version.
fn same_program(a: &str, b: &str) -> bool {
    a == b || matches!((packaged(a), packaged(b)), (Some(a), Some(b)) if a == b)
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct App {
    /// The program's path in lower case.
    exe: String,
    /// Its own device for each of [`ROLES`] before the stream: "" for none.
    before: [String; 2],
    applied: String,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Journal {
    apps: Vec<App>,
}
fn path(directory: &Path) -> PathBuf {
    directory.join("audio-app-recovery.json")
}
/// Where an unreadable journal is moved, its program list kept: a free
/// name, so an earlier one stays.
fn unreadable(directory: &Path) -> PathBuf {
    (1..)
        .map(|n| match n {
            1 => directory.join("audio-app-recovery.unreadable.json"),
            n => directory.join(format!("audio-app-recovery.unreadable-{n}.json")),
        })
        .find(|path| !path.exists())
        .expect("a free name")
}
fn load(directory: &Path) -> Result<Journal> {
    let file = path(directory);
    match std::fs::read(&file) {
        Ok(bytes) => match serde_json::from_slice(&bytes) {
            Ok(journal) => Ok(journal),
            // Never in the way of the next stream: moved aside, and the games
            // it lists are named for a reset by hand.
            Err(error) => {
                let aside = unreadable(directory);
                std::fs::rename(&file, &aside).with_context(|| {
                    format!(
                        "the record of the games' audio devices, {}, can't be read ({error}) or moved aside",
                        file.display()
                    )
                })?;
                tracing::warn!(
                    error = %error,
                    kept = %aside.display(),
                    "the record of the games' audio devices can't be read: the games it lists may still play on the virtual playback device; reset them in Settings > System > Sound > Volume mixer"
                );
                Ok(Journal::default())
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Journal::default()),
        Err(error) => Err(error).with_context(|| file.display().to_string()),
    }
}
fn save(directory: &Path, journal: &Journal) -> Result<()> {
    butterpollo_core::state::write_json(&path(directory), journal)
}

#[derive(Default)]
struct State {
    /// Streams playing each program (by path) on the sink right now.
    active: BTreeMap<String, usize>,
    /// Programs the host is stopping, put back already ([`put_back`]): not
    /// routed again until they stopped, by how many stops.
    closing: BTreeMap<String, usize>,
    watching: bool,
}
static STATE: Mutex<State> = Mutex::new(State {
    active: BTreeMap::new(),
    closing: BTreeMap::new(),
    watching: false,
});

/// How [`route_with`] left the process.
#[derive(Debug, PartialEq)]
enum Routed {
    /// On the sink, read back from Windows.
    Sink,
    /// On a device the user chose for the game during this stream: kept.
    Users(String),
    /// Left alone: the host is stopping the program.
    Closing,
}

/// Play process `pid` (program `exe`, lower case) on `sink`.
fn route_with(
    apps: &dyn Apps,
    state: &mut State,
    directory: &Path,
    pid: u32,
    exe: &str,
    sink: &str,
) -> Result<Routed> {
    if state
        .closing
        .keys()
        .any(|closing| same_program(closing, exe))
    {
        return Ok(Routed::Closing);
    }
    let mut journal = load(directory)?;
    let mut current: [String; 2] = Default::default();
    for (slot, role) in current.iter_mut().zip(ROLES) {
        *slot = apps.get(pid, role)?;
    }
    let is_sink = |device: &str| device.eq_ignore_ascii_case(sink);
    match journal
        .apps
        .iter_mut()
        .find(|app| same_program(&app.exe, exe))
    {
        Some(app) => {
            // The program waits for its restore, or another stream plays it:
            // its journalled device stays the user's. One the user chose for
            // it meanwhile (neither the sink nor that) is kept.
            if let Some(chosen) = current
                .iter()
                .zip(&app.before)
                .find(|(now, before)| !is_sink(now) && !now.eq_ignore_ascii_case(before))
                .map(|(now, _)| now.clone())
            {
                return Ok(Routed::Users(chosen));
            }
            app.applied = sink.into();
        }
        None => journal.apps.push(App {
            exe: exe.into(),
            // Already on the sink with no record of it (the record was
            // deleted): the game goes back to the default, never stays here.
            before: current.map(|device| {
                if is_sink(&device) {
                    String::new()
                } else {
                    device
                }
            }),
            applied: sink.into(),
        }),
    }
    save(directory, &journal)?;
    *state.active.entry(exe.into()).or_default() += 1;
    let applied = ROLES.into_iter().try_for_each(|role| {
        apps.set(pid, role, sink)?;
        let read = apps.get(pid, role)?;
        anyhow::ensure!(
            is_sink(&read),
            "Windows kept {} for the game, not the virtual sink",
            if read.is_empty() {
                "the default device"
            } else {
                read.as_str()
            }
        );
        Ok(())
    });
    if let Err(error) = applied {
        // Whatever this set goes back now, or when the program next runs.
        if let Err(restore) = release_with(apps, state, directory, exe) {
            tracing::warn!(exe, error = %format!("{restore:#}"), "game audio device could not be restored after a failed route");
        }
        // Not tried again as if nothing changed: it was journalled.
        if no_sound_yet(&error) {
            return Err(anyhow::anyhow!(
                "Windows lost the game's sound while moving it"
            ));
        }
        return Err(error);
    }
    Ok(Routed::Sink)
}

/// One stream stopped playing `exe` on the sink; the last puts it back.
/// Returns whether a restore still waits for the program to run.
fn release_with(apps: &dyn Apps, state: &mut State, directory: &Path, exe: &str) -> Result<bool> {
    if !release(state, exe) {
        return Ok(false);
    }
    restore_with(apps, state, directory)
}
/// One stream stopped playing `exe`: whether it was the last.
fn release(state: &mut State, exe: &str) -> bool {
    if let Some(users) = state.active.get_mut(exe) {
        *users -= 1;
        if *users > 0 {
            return false;
        }
        state.active.remove(exe);
    }
    true
}

/// Programs `exes` are about to be stopped with every stream that plays
/// them: put back now, while they run, whatever the streams' count.
/// Returns whether a restore still waits for a program to run.
#[cfg(test)]
fn put_back_with(
    apps: &dyn Apps,
    state: &mut State,
    directory: &Path,
    exes: &[String],
) -> Result<bool> {
    forget(state, exes);
    restore_with(apps, state, directory)
}
/// No stream plays programs `exes` any more.
fn forget(state: &mut State, exes: &[String]) {
    state
        .active
        .retain(|active, _| !exes.iter().any(|exe| same_program(active, exe)));
}

/// Put back every journalled program no stream plays that runs now.
/// Returns whether one still waits to run (a stream's own program is put
/// back when it ends).
fn restore_with(apps: &dyn Apps, state: &State, directory: &Path) -> Result<bool> {
    let waiting = waiting(state, &load(directory)?);
    if waiting.is_empty() {
        return Ok(false);
    }
    let running = apps.running(&waiting.iter().map(String::as_str).collect::<Vec<_>>())?;
    restore_running(apps, state, directory, &running)
}

/// The journalled programs no stream plays.
fn waiting(state: &State, journal: &Journal) -> Vec<String> {
    journal
        .apps
        .iter()
        .filter(|app| idle(state, app))
        .map(|app| app.exe.clone())
        .collect()
}
fn idle(state: &State, app: &App) -> bool {
    !state.active.keys().any(|exe| same_program(&app.exe, exe))
}

/// [`restore_with`] holding `state` only to change devices: the process
/// listing, its slow part, runs without it, so a stream's route never waits
/// behind it. A program a stream routed meanwhile is left to that stream.
/// The lock comes back for a decision that must follow the restore.
fn restore_unlocked<'a>(
    apps: &dyn Apps,
    state: &'a Mutex<State>,
    directory: &Path,
) -> (MutexGuard<'a, State>, Result<bool>) {
    let waiting = {
        let state = state.lock().unwrap();
        match load(directory) {
            Ok(journal) => waiting(&state, &journal),
            Err(error) => return (state, Err(error)),
        }
    };
    let running = if waiting.is_empty() {
        Ok(vec![])
    } else {
        apps.running(&waiting.iter().map(String::as_str).collect::<Vec<_>>())
    };
    let state = state.lock().unwrap();
    let result = running.and_then(|running| restore_running(apps, &state, directory, &running));
    (state, result)
}

/// [`restore_unlocked`] for this host's streams, on Windows.
fn restore_now(directory: &Path) -> (MutexGuard<'static, State>, Result<bool>) {
    match crate::capture::ComGuard::new().and_then(|com| Ok((com, Windows::new()?))) {
        Ok((_com, apps)) => restore_unlocked(&apps, &STATE, directory),
        Err(error) => (STATE.lock().unwrap(), Err(error)),
    }
}

/// Put back the journalled programs no stream plays that are `running`.
fn restore_running(
    apps: &dyn Apps,
    state: &State,
    directory: &Path,
    running: &[(u32, String)],
) -> Result<bool> {
    let mut journal = load(directory)?;
    let mut failed = None;
    let mut done = vec![];
    for app in journal.apps.iter().filter(|app| idle(state, app)) {
        let Some((pid, _)) = running.iter().find(|(_, exe)| same_program(&app.exe, exe)) else {
            continue;
        };
        let result = ROLES
            .into_iter()
            .zip(&app.before)
            .try_for_each(|(role, before)| -> Result<()> {
                // A device the user chose for the game meanwhile stays.
                if !apps.get(*pid, role)?.eq_ignore_ascii_case(&app.applied) {
                    return Ok(());
                }
                if let Err(error) = apps.set(*pid, role, before) {
                    // The device it had is gone (unplugged, uninstalled): it
                    // follows the default instead of waiting forever.
                    tracing::warn!(exe = app.exe, device = before, error = %format!("{error:#}"), "the game's own audio device is unavailable; it follows the default device again");
                    apps.set(*pid, role, "")?;
                }
                Ok(())
            });
        match result {
            Ok(()) => {
                tracing::info!(exe = app.exe, "game audio device restored");
                done.push(app.exe.clone());
            }
            // Just started: put back once it opened its sound, which
            // then moves at once ([`watch_starts`] looks again soon).
            Err(error) if no_sound_yet(&error) => {
                failed.get_or_insert(error);
            }
            Err(error) => {
                tracing::warn!(exe = app.exe, error = %format!("{error:#}"), "game audio device could not be restored");
                if failed.as_ref().is_none_or(no_sound_yet) {
                    failed = Some(error);
                }
            }
        }
    }
    journal.apps.retain(|app| !done.contains(&app.exe));
    if !done.is_empty() {
        save(directory, &journal)?;
    }
    if let Some(error) = failed {
        return Err(error);
    }
    Ok(journal.apps.iter().any(|app| idle(state, app)))
}

/// Where the game's sound reaches the PC ([`plays_elsewhere`]).
#[derive(Debug, Default, PartialEq)]
pub struct OnThePc {
    /// The game's processes playing there.
    pub pids: Vec<u32>,
    /// One plays on the PC's default communications device, which is also
    /// its default or the game's own device: that may be the game's voice
    /// chat, which stays on it ([`ROLES`]).
    pub maybe_voice: bool,
}

/// The game's processes (`pids`) with an active session (`sessions`: pid
/// and endpoint ID) where the PC plays the game: its default device for
/// [`ROLES`] (`defaults`: console, multimedia, communications) or the
/// game's own device (`own`), not the sink, even beside a session on the
/// sink. Windows moves no stream that is already open, so a game that
/// opened its sound before it was routed keeps it on the PC's device until
/// it opens it again. Another communications device is voice chat's, never
/// counted.
fn plays_elsewhere(
    pids: &[u32],
    sessions: &[(u32, String)],
    sink: &str,
    defaults: &[Option<String>; 3],
    own: &[&str],
) -> OnThePc {
    let [console, multimedia, communications] = defaults;
    let same = |a: &str, b: &str| a.eq_ignore_ascii_case(b);
    let watched: Vec<&str> = [console.as_deref(), multimedia.as_deref()]
        .into_iter()
        .flatten()
        .chain(own.iter().copied().filter(|device| !device.is_empty()))
        .filter(|device| !same(device, sink))
        .collect();
    let mut found = OnThePc::default();
    for (pid, endpoint) in sessions {
        if !pids.contains(pid) || !watched.iter().any(|device| same(device, endpoint)) {
            continue;
        }
        found.pids.push(*pid);
        found.maybe_voice |= communications
            .as_deref()
            .is_some_and(|device| same(device, endpoint));
    }
    found.pids.sort_unstable();
    found.pids.dedup();
    found
}

/// The endpoint ID in a per-app device (`\\?\SWD#MMDEVAPI#{id}#{interface}`).
fn endpoint_id(device: &str) -> &str {
    let lower = device.to_ascii_lowercase();
    let start = if lower.starts_with(&MMDEVAPI.to_ascii_lowercase()) {
        MMDEVAPI.len()
    } else {
        0
    };
    let end = if lower.ends_with(RENDER_INTERFACE) {
        device.len() - RENDER_INTERFACE.len()
    } else {
        device.len()
    };
    device.get(start..end.max(start)).unwrap_or(device)
}

/// Active playback sessions: process ID and endpoint ID.
fn active_sessions() -> Result<Vec<(u32, String)>> {
    sessions(true)
}
/// The processes with a playback session, active or not: those that opened
/// their sound.
fn sound_pids() -> Result<Vec<u32>> {
    Ok(sessions(false)?.into_iter().map(|(pid, _)| pid).collect())
}
/// Playback sessions (only active ones when `active`): process ID and
/// endpoint ID.
fn sessions(active: bool) -> Result<Vec<(u32, String)>> {
    let _com = crate::capture::ComGuard::new()?;
    // SAFETY: COM is initialised by `_com`; every interface used is live and owned here.
    unsafe {
        let enumerator: IMMDeviceEnumerator = windows::Win32::System::Com::CoCreateInstance(
            &MMDeviceEnumerator,
            None,
            windows::Win32::System::Com::CLSCTX_ALL,
        )?;
        let list = enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)?;
        let mut sessions = vec![];
        for index in 0..list.GetCount()? {
            let device = list.Item(index)?;
            let id = crate::audio_route::device_id(&device)?;
            let manager: IAudioSessionManager2 =
                device.Activate(windows::Win32::System::Com::CLSCTX_ALL, None)?;
            let all = manager.GetSessionEnumerator()?;
            for at in 0..all.GetCount()? {
                let control: IAudioSessionControl2 = all.GetSession(at)?.cast()?;
                if !active || control.GetState()? == AudioSessionStateActive {
                    sessions.push((control.GetProcessId().unwrap_or(0), id.clone()));
                }
            }
        }
        Ok(sessions)
    }
}

/// The game's process played on a virtual sink while this lives.
pub struct AppRoute {
    directory: PathBuf,
    exe: String,
    pid: u32,
    /// The sink's endpoint ID, and its name for messages.
    sink_id: String,
    pub sink: String,
    /// Kept on a device the user chose for the game, or not moved as the
    /// host stops the game (""): nothing to put back.
    users: Option<String>,
}
impl AppRoute {
    /// Play process `pid` on the configured `virtual_sink`, else on Steam
    /// Streaming Speakers. Fails when there is neither, or when Windows
    /// does not keep the choice; with [`NoSoundYet`], changing nothing, when
    /// the process hasn't opened its sound yet.
    pub fn acquire(config: &Config, directory: &Path, pid: u32) -> Result<Self> {
        let _com = crate::capture::ComGuard::new()?;
        let available = crate::audio_route::endpoints()?;
        let configured = config.get("virtual_sink", "");
        let sink = if configured.is_empty() {
            available.iter().find(|endpoint| endpoint.virtual_sink).context(
                "there is no virtual playback device: install Steam Streaming Speakers or set a virtual sink in Audio settings",
            )?
        } else {
            crate::audio_route::find(&available, configured)
                .context("the virtual sink set in Audio settings is unavailable")?
        };
        let exe = crate::process::image_path(pid)
            .context("the game's program can't be read")?
            .to_lowercase();
        let apps = Windows::new()?;
        let routed = route_with(
            &apps,
            &mut STATE.lock().unwrap(),
            directory,
            pid,
            &exe,
            &format!("{MMDEVAPI}{}{RENDER_INTERFACE}", sink.id),
        );
        let users = match routed {
            Ok(Routed::Sink) => {
                tracing::info!(
                    exe,
                    pid,
                    sink = sink.name,
                    "game audio plays on the virtual sink"
                );
                None
            }
            Ok(Routed::Closing) => {
                tracing::info!(
                    exe,
                    pid,
                    "game audio not moved: the host is stopping the game"
                );
                Some(String::new())
            }
            Ok(Routed::Users(device)) => {
                tracing::info!(
                    exe,
                    pid,
                    device,
                    "game audio stays on the device the user chose for it"
                );
                Some(device)
            }
            // Nothing changed or journalled yet: the caller tries again.
            Err(error) if no_sound_yet(&error) => return Err(error),
            Err(error) => {
                watch(directory);
                return Err(error);
            }
        };
        Ok(Self {
            directory: directory.into(),
            exe,
            pid,
            sink_id: sink.id.clone(),
            sink: sink.name.clone(),
            users,
        })
    }
    /// The routed program's path, in lower case.
    pub fn program(&self) -> &str {
        &self.exe
    }
    /// The device the user chose for the game, which it stays on.
    pub fn users_device(&self) -> Option<&str> {
        self.users.as_deref().filter(|device| !device.is_empty())
    }
    /// Which of these routes' processes, and of the game's processes left
    /// `unrouted`, play where the PC plays the game, not on the routes' sink
    /// ([`plays_elsewhere`]).
    pub fn plays_elsewhere(routes: &[&AppRoute], unrouted: &[u32]) -> Result<OnThePc> {
        let Some(first) = routes.iter().find(|route| route.users.is_none()) else {
            return Ok(OnThePc::default());
        };
        let pids: Vec<u32> = routes
            .iter()
            .filter(|r| r.users.is_none())
            .map(|r| r.pid)
            .chain(unrouted.iter().copied())
            .collect();
        let journal = load(&first.directory)?;
        let own: Vec<&str> = journal
            .apps
            .iter()
            .filter(|app| routes.iter().any(|r| same_program(&app.exe, &r.exe)))
            .flat_map(|app| app.before.iter().map(|device| endpoint_id(device)))
            .collect();
        let defaults = {
            let _com = crate::capture::ComGuard::new()?;
            crate::audio_route::defaults()?
        };
        Ok(plays_elsewhere(
            &pids,
            &active_sessions()?,
            &first.sink_id,
            &defaults,
            &own,
        ))
    }
}
impl Drop for AppRoute {
    fn drop(&mut self) {
        if self.users.is_some() {
            return;
        }
        if !release(&mut STATE.lock().unwrap(), &self.exe) {
            return;
        }
        let (state, result) = restore_now(&self.directory);
        drop(state);
        match result {
            Ok(false) => {}
            Ok(true) => watch(&self.directory),
            Err(error) if no_sound_yet(&error) => watch(&self.directory),
            Err(error) => {
                tracing::warn!(exe = self.exe, error = %format!("{error:#}"), "game audio device could not be restored; retrying while the host runs");
                watch(&self.directory);
            }
        }
    }
}

/// Restore what an interrupted stream left (host start), and keep looking
/// for programs that are not running yet.
/// An error leaves the watcher retrying.
pub fn recover(directory: &Path) -> Result<()> {
    match load(directory) {
        Ok(journal) if journal.apps.is_empty() => return Ok(()),
        Ok(_) => {}
        Err(error) => {
            watch(directory);
            return Err(error);
        }
    }
    let (state, result) = restore_now(directory);
    drop(state);
    if !matches!(result, Ok(false)) {
        watch(directory);
    }
    match result {
        // The watcher puts it back once it opened its sound.
        Err(error) if !no_sound_yet(&error) => Err(error),
        _ => Ok(()),
    }
}

/// The host is about to stop processes `pids` (a game, a session's
/// instances) and every stream playing them: their programs go back to
/// their own devices now, while Windows can still set them. A stream's
/// route that lets go later finds nothing left to put back. `pids` is
/// asked for the programs. Until the guard drops, once they stopped, no
/// stream routes them again.
pub fn put_back(directory: &Path, pids: impl FnOnce() -> Result<Vec<u32>>) -> Result<Closing> {
    let exes: Vec<String> = pids()?
        .iter()
        .filter_map(|&pid| crate::process::image_path(pid))
        .map(|exe| exe.to_lowercase())
        .collect();
    let closing = Closing::mark(&mut STATE.lock().unwrap(), exes);
    if load(directory)?.apps.is_empty() {
        return Ok(closing);
    }
    forget(&mut STATE.lock().unwrap(), &closing.exes);
    let (state, result) = restore_now(directory);
    drop(state);
    if !matches!(result, Ok(false)) {
        watch(directory);
    }
    match result {
        Err(error) if !no_sound_yet(&error) => Err(error),
        _ => Ok(closing),
    }
}

/// Programs the host stops: routed again once this drops.
pub struct Closing {
    exes: Vec<String>,
}
impl Closing {
    fn mark(state: &mut State, exes: Vec<String>) -> Self {
        for exe in &exes {
            *state.closing.entry(exe.clone()).or_default() += 1;
        }
        Self { exes }
    }
    fn unmark(&self, state: &mut State) {
        for exe in &self.exes {
            if let Some(count) = state.closing.get_mut(exe) {
                *count -= 1;
                if *count == 0 {
                    state.closing.remove(exe);
                }
            }
        }
    }
}
impl Drop for Closing {
    fn drop(&mut self) {
        self.unmark(&mut STATE.lock().unwrap());
    }
}

/// One watcher per host: it ends once nothing waits. It puts a program back
/// as it starts ([`watch_starts`]); when Windows can't tell it starts, it
/// looks every [`WATCH`], which can be after the game opened its sound.
fn watch(directory: &Path) {
    {
        let mut state = STATE.lock().unwrap();
        if state.watching {
            return;
        }
        state.watching = true;
    }
    let directory = directory.to_path_buf();
    let spawned = std::thread::Builder::new()
        .name("game audio restore".into())
        .spawn(move || {
            let watched = crate::capture::ComGuard::new().and_then(|com| {
                let apps = Windows::new()?;
                let mut starts = WindowsStarts::subscribe()
                    .context("watching programs start (WMI's Win32_ProcessStartTrace)")?;
                let watched = watch_starts(&apps, &STATE, &directory, &mut starts);
                drop((starts, apps, com));
                watched
            });
            if let Err(error) = watched {
                tracing::error!(
                    error = %format!("{error:#}"),
                    "game audio restore can't see games start: a game that quit by itself goes back to its own device up to 2 s after it next starts, and may play that run on the virtual playback device"
                );
                poll(&directory);
            }
        });
    if let Err(error) = spawned {
        STATE.lock().unwrap().watching = false;
        tracing::warn!(error = %error, "game audio devices are restored at the next host start");
    }
}

/// Watches `starts` until nothing waits: a waiting program is put back as
/// it starts, and every program every [`SWEEP`]. Returns the error that
/// ended the watch of starts, with the watcher still on; a failed restore is
/// told and tried again.
fn watch_starts(
    apps: &dyn Apps,
    state: &Mutex<State>,
    directory: &Path,
    starts: &mut dyn Starts,
) -> Result<()> {
    let mut reported = None;
    // Starts are watched already: this first look finds a program that
    // started since the restore that left it waiting, the watch any later.
    let mut sweep = true;
    // A program that started but hasn't opened its sound is looked at
    // again every [`WATCH`].
    let mut soon = false;
    loop {
        let (mut guard, result) = if std::mem::take(&mut sweep) {
            restore_unlocked(apps, state, directory)
        } else {
            match starts.next(if soon { WATCH } else { SWEEP })? {
                None => restore_unlocked(apps, state, directory),
                Some((pid, exe)) => match load(directory) {
                    Err(error) => (state.lock().unwrap(), Err(error)),
                    Ok(journal) => {
                        let guard = state.lock().unwrap();
                        // Any other program starting costs no device call.
                        if !journal
                            .apps
                            .iter()
                            .any(|app| idle(&guard, app) && same_program(&app.exe, &exe))
                        {
                            continue;
                        }
                        let result = restore_running(apps, &guard, directory, &[(pid, exe)]);
                        (guard, result)
                    }
                },
            }
        };
        soon = matches!(&result, Err(error) if no_sound_yet(error));
        // One lock from the restore's device changes to the decision to
        // stop: a stream ending meanwhile finds the watcher either running
        // or gone.
        if !waits_still(&mut guard, result, &mut reported) {
            return Ok(());
        }
    }
}

/// [`watch`] without process starts: a look every [`WATCH`].
fn poll(directory: &Path) {
    let mut reported = None;
    loop {
        std::thread::sleep(WATCH);
        let (mut state, result) = restore_now(directory);
        if !waits_still(&mut state, result, &mut reported) {
            return;
        }
    }
}

/// After a watcher's restore: whether it watches on. It ends when nothing
/// waits; an error is told once until it changes.
fn waits_still(state: &mut State, result: Result<bool>, reported: &mut Option<String>) -> bool {
    match result {
        Ok(true) => true,
        Ok(false) => {
            state.watching = false;
            false
        }
        // Not a failure: it waits for the game's sound.
        Err(error) if no_sound_yet(&error) => true,
        Err(error) => {
            let detail = format!("{error:#}");
            if reported.as_ref() != Some(&detail) {
                tracing::warn!(
                    error = detail,
                    "game audio devices could not be restored; retrying"
                );
                *reported = Some(detail);
            }
            true
        }
    }
}

#[cfg(test)]
mod tests;
