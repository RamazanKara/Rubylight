//! Game-only audio (a background stream's game, or `audio_game_only`) leaves
//! the PC's default playback device alone, so the game played on the PC's
//! speakers as well as on the client (#59). The game's own process is played
//! on a virtual sink nobody hears instead (Windows' per-app output device, as
//! in Settings > Sound > Volume mixer); process loopback captures it there.
//!
//! Windows keeps an app's output device for its program, after it exits, so
//! the choice is journalled first and put back once the program runs again
//! when it exited mid-stream or the host crashed: right away when a process
//! of it still runs, else when it next starts (a watcher looks every
//! [`WATCH`]). A device the user picked for the game during the stream stays.

use anyhow::{Context, Result};
use butterpollo_core::config::Config;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    ffi::c_void,
    path::{Path, PathBuf},
    sync::Mutex,
    time::Duration,
};
use windows::{
    Win32::{Media::Audio::*, System::WinRT::RoGetActivationFactory},
    core::{GUID, HRESULT, HSTRING, IInspectable, IInspectable_Vtbl, Interface},
};

/// The roles a game's stream opens on; communications stays the user's.
const ROLES: [ERole; 2] = [eConsole, eMultimedia];
/// How often a pending restore looks for the program it waits for.
const WATCH: Duration = Duration::from_secs(2);
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

/// The calls the routing needs, so the journal logic runs against a fake.
trait Apps {
    /// The process's own playback device for `role`: "" when it follows
    /// the default.
    fn get(&self, pid: u32, role: ERole) -> Result<String>;
    /// "" puts the process back on the default.
    fn set(&self, pid: u32, role: ERole, device: &str) -> Result<()>;
    /// Running processes whose program path is one of `exes` (lower case).
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
        unsafe { (self.0.vtable().get)(self.0.as_raw(), pid, eRender, role, &mut device).ok()? };
        Ok(device.to_string_lossy())
    }
    fn set(&self, pid: u32, role: ERole, device: &str) -> Result<()> {
        let device = HSTRING::from(device);
        // SAFETY: the HSTRING is borrowed for the call (an empty one is null, which clears) and
        // outlives it; the callee does not take ownership.
        unsafe {
            let raw: *mut c_void = std::mem::transmute_copy(&device);
            (self.0.vtable().set)(self.0.as_raw(), pid, eRender, role, raw).ok()?;
        }
        Ok(())
    }
    fn running(&self, exes: &[&str]) -> Result<Vec<(u32, String)>> {
        let names: Vec<&str> = exes.iter().map(|exe| file_name(exe)).collect();
        Ok(crate::process::processes()?
            .into_iter()
            .filter(|p| names.iter().any(|n| p.name.eq_ignore_ascii_case(n)))
            .filter_map(|p| Some((p.pid, crate::process::image_path(p.pid)?.to_lowercase())))
            .filter(|(_, path)| exes.contains(&path.as_str()))
            .collect())
    }
}
fn file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
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
fn load(directory: &Path) -> Result<Journal> {
    let file = path(directory);
    match std::fs::read(&file) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .with_context(|| format!("{} can't be read: delete it", file.display())),
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
    watching: bool,
}
static STATE: Mutex<State> = Mutex::new(State {
    active: BTreeMap::new(),
    watching: false,
});

/// Play process `pid` (program `exe`, lower case) on `sink`.
fn route_with(
    apps: &dyn Apps,
    state: &mut State,
    directory: &Path,
    pid: u32,
    exe: &str,
    sink: &str,
) -> Result<()> {
    let mut journal = load(directory)?;
    // A pending entry keeps the user's device from before: the current one
    // is the sink this host set.
    match journal.apps.iter_mut().find(|app| app.exe == exe) {
        Some(app) => app.applied = sink.into(),
        None => {
            let mut before: [String; 2] = Default::default();
            for (slot, role) in before.iter_mut().zip(ROLES) {
                *slot = apps.get(pid, role)?;
            }
            journal.apps.push(App {
                exe: exe.into(),
                before,
                applied: sink.into(),
            });
        }
    }
    save(directory, &journal)?;
    let applied = ROLES
        .into_iter()
        .try_for_each(|role| apps.set(pid, role, sink));
    *state.active.entry(exe.into()).or_default() += 1;
    if let Err(error) = applied {
        release_with(apps, state, directory, exe)?;
        return Err(error);
    }
    Ok(())
}

/// One stream stopped playing `exe` on the sink; the last puts it back.
/// Returns whether a restore still waits for the program to run.
fn release_with(apps: &dyn Apps, state: &mut State, directory: &Path, exe: &str) -> Result<bool> {
    if let Some(users) = state.active.get_mut(exe) {
        *users -= 1;
        if *users > 0 {
            return Ok(false);
        }
        state.active.remove(exe);
    }
    restore_with(apps, state, directory)
}

/// Put back every journalled program no stream plays that runs now.
/// Returns whether one still waits to run (a stream's own program is put
/// back when it ends).
fn restore_with(apps: &dyn Apps, state: &State, directory: &Path) -> Result<bool> {
    let mut journal = load(directory)?;
    let waiting: Vec<&str> = journal
        .apps
        .iter()
        .filter(|app| !state.active.contains_key(&app.exe))
        .map(|app| app.exe.as_str())
        .collect();
    if waiting.is_empty() {
        return Ok(false);
    }
    let running = apps.running(&waiting)?;
    let mut failed = None;
    let mut restored = vec![];
    for app in &journal.apps {
        if state.active.contains_key(&app.exe) {
            continue;
        }
        let Some((pid, _)) = running.iter().find(|(_, exe)| *exe == app.exe) else {
            continue;
        };
        let result = ROLES
            .into_iter()
            .zip(&app.before)
            .try_for_each(|(role, before)| {
                // A device the user chose for the game meanwhile stays.
                if apps.get(*pid, role)?.eq_ignore_ascii_case(&app.applied) {
                    apps.set(*pid, role, before)?;
                }
                Ok(())
            });
        match result {
            Ok(()) => {
                tracing::info!(exe = app.exe, "game audio device restored");
                restored.push(app.exe.clone());
            }
            Err(error) => {
                tracing::warn!(exe = app.exe, error = %format!("{error:#}"), "game audio device could not be restored");
                failed.get_or_insert(error);
            }
        }
    }
    journal.apps.retain(|app| !restored.contains(&app.exe));
    if !restored.is_empty() {
        save(directory, &journal)?;
    }
    if let Some(error) = failed {
        return Err(error);
    }
    Ok(journal
        .apps
        .iter()
        .any(|app| !state.active.contains_key(&app.exe)))
}

/// The game's process played on a virtual sink while this lives.
pub struct AppRoute {
    directory: PathBuf,
    exe: String,
    /// The sink's name, for the log.
    pub sink: String,
}
impl AppRoute {
    /// Play process `pid` on the configured `virtual_sink`, else on Steam
    /// Streaming Speakers. Fails when there is neither.
    pub fn acquire(config: &Config, directory: &Path, pid: u32) -> Result<Self> {
        let _com = crate::capture::ComGuard::new()?;
        let available = crate::audio_route::endpoints()?;
        let configured = config.get("virtual_sink", "");
        let sink =
            if configured.is_empty() {
                available.iter().find(|endpoint| endpoint.virtual_sink).context(
                "there is no virtual playback device (Steam Streaming Speakers) to play it on",
            )?
            } else {
                crate::audio_route::find(&available, configured)
                    .context("the configured virtual audio sink is unavailable")?
            };
        let exe = crate::process::image_path(pid)
            .context("the game's program can't be read")?
            .to_lowercase();
        let apps = Windows::new()?;
        route_with(
            &apps,
            &mut STATE.lock().unwrap(),
            directory,
            pid,
            &exe,
            &format!("{MMDEVAPI}{}{RENDER_INTERFACE}", sink.id),
        )?;
        tracing::info!(
            exe,
            sink = sink.name,
            "game audio plays on the virtual sink"
        );
        Ok(Self {
            directory: directory.into(),
            exe,
            sink: sink.name.clone(),
        })
    }
}
impl Drop for AppRoute {
    fn drop(&mut self) {
        let result = crate::capture::ComGuard::new().and_then(|_com| {
            let apps = Windows::new()?;
            let mut state = STATE.lock().unwrap();
            release_with(&apps, &mut state, &self.directory, &self.exe)
        });
        match result {
            Ok(false) => {}
            Ok(true) => watch(&self.directory),
            Err(error) => {
                tracing::warn!(exe = self.exe, error = %format!("{error:#}"), "game audio device could not be restored; retrying while the host runs");
                watch(&self.directory);
            }
        }
    }
}

/// Restore what an interrupted stream left (host start), and keep looking
/// for programs that are not running yet.
pub fn recover(directory: &Path) -> Result<()> {
    let result = crate::capture::ComGuard::new().and_then(|_com| {
        let apps = Windows::new()?;
        restore_with(&apps, &STATE.lock().unwrap(), directory)
    });
    if !matches!(result, Ok(false)) {
        watch(directory);
    }
    result.map(|_| ())
}

/// One watcher per host: it ends once nothing waits.
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
            let mut reported = None;
            loop {
                std::thread::sleep(WATCH);
                let result = crate::capture::ComGuard::new().and_then(|_com| {
                    let apps = Windows::new()?;
                    let state = STATE.lock().unwrap();
                    restore_with(&apps, &state, &directory)
                });
                let mut state = STATE.lock().unwrap();
                match result {
                    Ok(true) => {}
                    Ok(false) => {
                        state.watching = false;
                        return;
                    }
                    Err(error) => {
                        let detail = format!("{error:#}");
                        if reported.as_ref() != Some(&detail) {
                            tracing::warn!(
                                error = detail,
                                "game audio devices could not be restored; retrying"
                            );
                            reported = Some(detail);
                        }
                    }
                }
            }
        });
    if let Err(error) = spawned {
        STATE.lock().unwrap().watching = false;
        tracing::warn!(error = %error, "game audio devices are restored at the next host start");
    }
}

#[cfg(test)]
mod tests;
