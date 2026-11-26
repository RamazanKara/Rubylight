//! Endpoint routing is shared by stream owners and restored after the last
//! owner, including host crashes. Capture-only sinks never alter defaults.

use anyhow::{Context, Result, bail};
use butterpollo_config::config::Config;
use butterpollo_session::session::AudioPreparation;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
#[cfg(test)]
use windows::core::GUID;
use windows::{Win32::Media::Audio::*, core::PCWSTR};

mod format;
pub mod hooks;
mod microphone;
pub mod notify;
mod policy;

use format::{
    FormatPolicy, apply_virtual_format, apply_virtual_format_with, sample_layout, valid_bits,
};
#[cfg(test)]
use format::{Sample, describe_format, fallback_channels, virtual_format, virtual_formats};
pub use format::{restore_sink_format, set_sink_channels};
use microphone::microphone_owns_recording;
pub use microphone::{
    MicrophoneFormats, MicrophoneRoute, SavedDefaults, SteamMicrophoneGain,
    install_steam_microphone, match_steam_microphone_format, recover_microphone, steam_microphone,
};
#[cfg(test)]
use microphone::{
    check_matched, matching_playback_format, microphone_path, microphone_pending,
    restore_saved_defaults, unity_cap,
};
#[cfg(test)]
use policy::PolicyVtbl;
pub use policy::{Endpoint, endpoints};
use policy::{Policy, ROLES, all_endpoints, defaults_for, device_name, device_names, host_owned};
pub(crate) use policy::{defaults, device_id, find};

#[derive(Clone, Serialize, Deserialize)]
struct FormatChange {
    id: String,
    before: Vec<u8>,
    applied: Vec<u8>,
}
#[derive(Clone, Default, Serialize, Deserialize)]
struct Journal {
    before: [Option<String>; 3],
    applied: String,
    format: Option<FormatChange>,
    /// The recording defaults at stream start, put back for roles that a host
    /// endpoint (Steam Streaming Microphone) took during the stream.
    #[serde(default)]
    capture_before: [Option<String>; 3],
}
impl Journal {
    fn pending(&self) -> bool {
        !self.applied.is_empty()
            || self.format.is_some()
            || self.capture_before.iter().any(Option::is_some)
    }
}
struct Active {
    directory: PathBuf,
    sink: String,
    virtual_sink: bool,
    users: usize,
    /// The channels each stream asked for; the sink gets the most.
    channels: BTreeMap<u64, usize>,
}
/// The speaker layout for streams sharing the sink: the most channels any of
/// them asked for. Capture mixes down for the others; following the last
/// stream instead made two clients reformat the sink in turn, and each
/// reformat interrupted the other's audio.
fn shared_channels(requests: &BTreeMap<u64, usize>) -> Option<usize> {
    requests.values().copied().max()
}
static ACTIVE: Mutex<Option<Active>> = Mutex::new(None);
pub struct Route {
    pub sink: String,
    directory: PathBuf,
    keep_default: bool,
    capture_only: bool,
    virtual_sink: bool,
    /// This stream's place among the users of the shared sink.
    id: u64,
}
static NEXT_ROUTE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
/// The id of a route that owns no device ([`Route::detached`]); real routes
/// start at 1.
const DETACHED: u64 = 0;
fn path(directory: &Path) -> PathBuf {
    directory.join("audio-recovery.json")
}
fn save(directory: &Path, journal: &Journal) -> Result<()> {
    butterpollo_session::state::write_json(&path(directory), journal)
}
fn restore(directory: &Path) -> Result<()> {
    let file = path(directory);
    if !file.exists() {
        return Ok(());
    }
    let _com = crate::os_helpers::ComGuard::new()?;
    let journal: Journal = serde_json::from_slice(&std::fs::read(&file)?)?;
    if !journal.pending() {
        return Ok(());
    }
    let policy = Policy::new()?;
    let all = all_endpoints();
    // A role on the endpoint this host set, or on a Steam endpoint, goes back to
    // the user's device. A Steam endpoint is never restored to. (`applied` is
    // the user's own device when the stream plays on the host, so it only
    // marks the roles to switch.)
    let steam = |id: &str| host_owned(&all, id);
    let switchable = |id: &str| id.eq_ignore_ascii_case(&journal.applied) || host_owned(&all, id);
    // Every role and the format are tried: one device that is unavailable
    // (Bluetooth off, a TV's audio not back yet) must not leave the others.
    // On failure the journal stays; a retry only changes roles still on a
    // host endpoint.
    let mut failed = None;
    let mut restored = 0;
    for (flow, direction, saved) in [
        (eRender, "playback", &journal.before),
        (eCapture, "recording", &journal.capture_before),
    ] {
        if flow == eCapture && microphone_owns_recording(directory) {
            continue;
        }
        let current = match defaults_for(flow) {
            Ok(current) => current,
            Err(error) => {
                failed.get_or_insert(error);
                continue;
            }
        };
        let targets = butterpollo_media_policy::audio_defaults::restore_targets(saved, steam);
        for (index, target) in
            butterpollo_media_policy::audio_defaults::switches(&current, &targets, switchable)
        {
            let role = butterpollo_media_policy::audio_defaults::ROLE_NAMES[index];
            match policy.set_default(&target, ROLES[index]) {
                Ok(()) => {
                    restored += 1;
                    tracing::info!(
                        direction,
                        role,
                        device = %device_name(&all, Some(&target)),
                        from = %device_name(&all, current[index].as_deref()),
                        "audio default restored"
                    );
                }
                Err(error) => {
                    tracing::warn!(
                        direction,
                        role,
                        device = %device_name(&all, Some(&target)),
                        error = %format!("{error:#}"),
                        "audio default could not be restored"
                    );
                    failed.get_or_insert(error.context("restoring the previous audio endpoint"));
                }
            }
        }
    }
    let playback = defaults_for(eRender)
        .map(|d| device_names(&all, &d))
        .unwrap_or_default();
    let recording = defaults_for(eCapture)
        .map(|d| device_names(&all, &d))
        .unwrap_or_default();
    tracing::info!(
        restored,
        playback = %playback,
        recording = %recording,
        "audio defaults after the stream (console | multimedia | communications)"
    );
    if let Some(format) = &journal.format {
        match policy.format(&format.id) {
            Ok(current) if current == format.applied => {
                if let Err(error) = policy.set_format(&format.id, &format.before) {
                    failed.get_or_insert(error);
                }
            }
            Ok(_) => {}
            Err(error) => {
                failed.get_or_insert(error);
            }
        }
    }
    if let Some(error) = failed {
        return Err(error);
    }
    save(directory, &Journal::default())
}
/// The journal of a restore that has not completed yet, if any.
fn pending(directory: &Path) -> Option<Journal> {
    let journal: Journal = serde_json::from_slice(&std::fs::read(path(directory)).ok()?).ok()?;
    journal.pending().then_some(journal)
}
/// The defaults to restore after this stream. When a restore is still
/// pending (the speakers were off when the last stream ended), a role that
/// is still on the endpoint this host set keeps the original from then: the
/// current default would be the virtual sink, and the user's speakers would
/// be lost for good.
fn originals(
    current: [Option<String>; 3],
    pending: Option<&Journal>,
    owned: impl Fn(&str) -> bool,
) -> [Option<String>; 3] {
    let Some(pending) = pending.filter(|journal| !journal.applied.is_empty()) else {
        return current;
    };
    butterpollo_media_policy::audio_defaults::with_pending(current, Some(&pending.before), |id| {
        id.eq_ignore_ascii_case(&pending.applied) || owned(id)
    })
}
pub fn recover(directory: &Path) -> Result<()> {
    let active = ACTIVE
        .try_lock()
        .map_err(|_| anyhow::anyhow!("audio routing is busy"))?;
    if active.is_some() {
        bail!("audio routing is in use");
    }
    restore(directory)
}
/// Install Steam Streaming Speakers when Steam provides them and none exist.
/// Best effort: streaming continues on another endpoint if this fails.
fn install_steam(config: &Config, endpoints: &[Endpoint]) -> bool {
    match try_install_steam(config, endpoints) {
        Ok(installed) => installed,
        Err(error) => {
            tracing::warn!(error = %format!("{error:#}"), "Steam Streaming Speakers could not be installed");
            false
        }
    }
}
fn try_install_steam(config: &Config, endpoints: &[Endpoint]) -> Result<bool> {
    if endpoints.iter().any(|endpoint| endpoint.virtual_sink)
        || !config.boolean("install_steam_audio_drivers", true)
    {
        return Ok(false);
    }
    let Some(directory) = std::env::var_os("CommonProgramFiles(x86)") else {
        return Ok(false);
    };
    let inf =
        PathBuf::from(directory).join("Steam/drivers/Windows10/x64/SteamStreamingSpeakers.inf");
    if !inf.is_file() {
        return Ok(false);
    }
    let file = crate::text::to_wide(&inf.to_string_lossy());
    // SAFETY: `file` is NUL-terminated and outlives the call.
    unsafe {
        use windows::Win32::Devices::DeviceAndDriverInstallation::{
            DIIRFLAG_FORCE_INF, DiInstallDriverW,
        };
        DiInstallDriverW(None, PCWSTR(file.as_ptr()), DIIRFLAG_FORCE_INF, None)?;
    }
    Ok(true)
}
fn virtual_sink_warning(
    host_audio: bool,
    capture_only: bool,
    virtual_sink: bool,
) -> Option<&'static str> {
    (!host_audio && !capture_only && !virtual_sink).then_some("Virtual audio sink unavailable; capturing a physical playback device. Sound may play on the host and surround channels may be lost. Select an installed virtual sink in Audio settings or enable host audio intentionally.")
}
impl AudioPreparation<Route> for Arc<Route> {
    fn route(&self) -> Arc<Route> {
        self.clone()
    }
}
impl Route {
    /// A route that owns no device and restores nothing when dropped. Only
    /// for tests of how long the host keeps a route (the `test-route`
    /// feature).
    #[cfg(feature = "test-route")]
    pub fn detached(directory: &Path) -> Self {
        Self {
            sink: String::new(),
            directory: directory.into(),
            keep_default: false,
            capture_only: true,
            virtual_sink: false,
            id: DETACHED,
        }
    }
    pub fn warning(&self, host_audio: bool) -> Option<&'static str> {
        virtual_sink_warning(host_audio, self.capture_only, self.virtual_sink)
    }
    pub fn acquire(
        config: &Config,
        directory: &Path,
        host_audio: bool,
        channels: usize,
    ) -> Result<Self> {
        let _com = crate::os_helpers::ComGuard::new()?;
        let mut active = ACTIVE.lock().unwrap();
        let capture_only = config.boolean("audio_sink_capture_only", false)
            && !config.get("audio_sink", "").is_empty()
            && config.get("virtual_sink", "").is_empty();
        if let Some(shared) = active.as_mut() {
            if shared.directory != directory {
                bail!("audio is owned by another host configuration");
            }
            shared.users += 1;
            return Ok(Self {
                sink: shared.sink.clone(),
                directory: directory.into(),
                keep_default: config.boolean("keep_sink_default", true),
                capture_only,
                virtual_sink: shared.virtual_sink,
                id: NEXT_ROUTE.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            });
        }
        let mut available = endpoints()?;
        // Taken before any driver installation, which can move the defaults.
        // Recording roles already on a host endpoint keep the user's choice.
        let pending = pending(directory);
        let all = all_endpoints();
        let owned = |id: &str| host_owned(&all, id);
        let before = butterpollo_media_policy::audio_defaults::restore_targets(
            &originals(defaults()?, pending.as_ref(), owned),
            owned,
        );
        let capture_before = if microphone_owns_recording(directory) {
            Default::default()
        } else {
            butterpollo_media_policy::audio_defaults::recording_targets(
                butterpollo_media_policy::audio_defaults::with_pending(
                    defaults_for(eCapture)?,
                    pending.as_ref().map(|journal| &journal.capture_before),
                    owned,
                ),
                owned,
            )
        };
        if !host_audio && !capture_only && install_steam(config, &available) {
            // The new endpoint appears shortly after installation.
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                available = endpoints()?;
                if available.iter().any(|endpoint| endpoint.virtual_sink)
                    || std::time::Instant::now() >= deadline
                {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
            let moved = defaults()?;
            if moved != before
                && let Ok(policy) = Policy::new()
            {
                for (index, role) in ROLES.into_iter().enumerate() {
                    if let Some(previous) = &before[index]
                        && moved[index].as_ref() != Some(previous)
                    {
                        let _ = policy.set_default(previous, role);
                    }
                }
                available = endpoints()?;
            }
        }
        let configured = config.get("virtual_sink", "");
        let configured = if configured.is_empty() {
            config.get("audio_sink", "")
        } else {
            configured
        };
        let default = available
            .iter()
            .find(|endpoint| endpoint.default)
            .context("no default playback endpoint")?;
        let selected = if capture_only {
            find(&available, configured)
                .context("configured capture-only audio sink is unavailable")?
        } else if !host_audio || !config.get("virtual_sink", "").is_empty() {
            if !config.get("virtual_sink", "").is_empty() {
                find(&available, configured)
                    .context("configured virtual audio sink is unavailable")?
            } else {
                available
                    .iter()
                    .find(|endpoint| endpoint.virtual_sink)
                    .or_else(|| find(&available, configured))
                    .unwrap_or(default)
            }
        } else if configured.is_empty() {
            default
        } else {
            find(&available, configured).context("configured audio sink is unavailable")?
        };
        let mut journal = Journal {
            before,
            applied: if capture_only {
                String::new()
            } else {
                selected.id.clone()
            },
            format: None,
            capture_before,
        };
        let managed_virtual = selected.virtual_sink || !config.get("virtual_sink", "").is_empty();
        if let Some(message) = virtual_sink_warning(host_audio, capture_only, managed_virtual) {
            tracing::warn!("{message}");
        }
        let policy = if capture_only {
            None
        } else {
            Some(Policy::new()?)
        };
        let mut applied_channels = if managed_virtual && !capture_only {
            None
        } else {
            Some(channels)
        };
        let result = (|| -> Result<()> {
            if managed_virtual && !capture_only {
                let policy = policy.as_ref().unwrap();
                let current = policy.format(&selected.id)?;
                // A pending restore's original format wins over the one this
                // host left, as for the default devices.
                journal.format = pending
                    .as_ref()
                    .and_then(|j| j.format.as_ref())
                    .filter(|format| format.id == selected.id && format.applied == current)
                    .cloned();
                let default_format = policy.format(&default.id)?;
                let bits = valid_bits(&default_format);
                match apply_virtual_format(
                    policy,
                    &selected.id,
                    channels,
                    bits,
                    directory,
                    &mut journal,
                ) {
                    Ok(applied) => {
                        if applied != channels {
                            tracing::warn!(
                                requested = channels,
                                applied,
                                sink = %selected.name,
                                "virtual audio sink rejected the requested surround layout; fell back to a lower surround layout"
                            );
                        }
                        applied_channels = Some(applied);
                    }
                    Err(error) if channels > 2 => {
                        // Do not fail Route::acquire or undo default switching
                        // when a virtual sink refuses surround (e.g. a stereo-only
                        // sink): keep journal.format unchanged and let capture open
                        // on the sink's native format with the audio_surround warning.
                        tracing::warn!(
                            requested = channels,
                            sink = %selected.name,
                            error = %format!("{error:#}"),
                            "virtual surround format could not be applied; continuing with the sink's current format"
                        );
                        applied_channels = None;
                    }
                    Err(error) => return Err(error),
                }
            }
            if !capture_only
                && journal
                    .before
                    .iter()
                    .any(|id| id.as_deref() != Some(&selected.id))
            {
                save(directory, &journal)?;
                hooks::mark_audio(true)?;
                for role in ROLES {
                    policy.as_ref().unwrap().set_default(&selected.id, role)?;
                }
            }
            save(directory, &journal)?;
            Ok(())
        })();
        if let Err(error) = result {
            if restore(directory).is_ok() {
                let _ = hooks::mark_audio(false);
            }
            return Err(error);
        }
        let id = NEXT_ROUTE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut channel_requests = BTreeMap::new();
        if let Some(applied) = applied_channels {
            channel_requests.insert(id, applied);
        }
        *active = Some(Active {
            directory: directory.into(),
            sink: selected.id.clone(),
            virtual_sink: managed_virtual,
            users: 1,
            channels: channel_requests,
        });
        Ok(Self {
            sink: selected.id.clone(),
            directory: directory.into(),
            keep_default: config.boolean("keep_sink_default", true),
            capture_only,
            virtual_sink: managed_virtual,
            id,
        })
    }
    /// Re-pin managed virtual speakers when keep_sink_default is enabled. Save
    /// the user's newly selected endpoint so teardown restores that choice.
    pub fn maintain_default(&self) -> Result<()> {
        let _active = ACTIVE.lock().unwrap();
        if !self.keep_default || self.capture_only || !self.virtual_sink {
            return Ok(());
        }
        let current = defaults()?;
        if current.iter().all(|id| id.as_deref() == Some(&self.sink)) {
            return Ok(());
        }
        let mut journal: Journal = serde_json::from_slice(&std::fs::read(path(&self.directory))?)?;
        // A device the user picked is restored after the stream. One Windows
        // made default on its own, such as Steam Streaming Microphone when its
        // driver arrives, is the host's and is not.
        let all = all_endpoints();
        let owned = |id: &str| id.eq_ignore_ascii_case(&self.sink) || host_owned(&all, id);
        if butterpollo_media_policy::audio_defaults::adopt(&mut journal.before, &current, owned) {
            tracing::info!(
                devices = %device_names(&all, &journal.before),
                "audio defaults to restore after the stream (console | multimedia | communications)"
            );
        }
        save(&self.directory, &journal)?;
        let policy = Policy::new()?;
        for role in ROLES {
            policy.set_default(&self.sink, role)?;
        }
        Ok(())
    }
    pub fn capture_sink(&self, config: &Config) -> Result<String> {
        if self.capture_only || self.virtual_sink || !config.boolean("auto_capture_sink", true) {
            return Ok(self.sink.clone());
        }
        let current = defaults()?;
        Ok(current[0].clone().unwrap_or_else(|| self.sink.clone()))
    }
    /// Format the managed sink for `channels` and return the channels it now
    /// has: fewer when the device refused them, more when another stream on it
    /// asked for more. A sink the host does not format returns `channels`.
    pub fn set_channels(&self, channels: usize) -> Result<usize> {
        let mut active = ACTIVE.lock().unwrap();
        if !self.virtual_sink || self.capture_only {
            return Ok(channels);
        }
        let policy = Policy::new()?;
        set_channels_with(
            &mut active,
            self.id,
            &self.sink,
            &self.directory,
            channels,
            &policy,
            hooks::mark_audio,
        )
    }
}

fn set_channels_with(
    active: &mut Option<Active>,
    route_id: u64,
    sink: &str,
    directory: &Path,
    channels: usize,
    policy: &dyn FormatPolicy,
    mark_audio: impl Fn(bool) -> Result<()>,
) -> Result<usize> {
    let target = match active.as_ref() {
        Some(shared) => {
            let mut next = shared.channels.clone();
            next.insert(route_id, channels);
            shared_channels(&next).unwrap_or(channels)
        }
        None => channels,
    };
    let current = policy.format(sink)?;
    let current_channels = usize::from(sample_layout(&current).0);
    if current_channels == target {
        if let Some(shared) = active.as_mut() {
            shared.channels.insert(route_id, channels);
        }
        return Ok(current_channels);
    }
    let mut journal: Journal = serde_json::from_slice(&std::fs::read(path(directory))?)?;
    if journal.format.is_none() {
        anyhow::bail!("virtual audio format is not claimed by this stream");
    }
    let applied = apply_virtual_format_with(
        policy,
        sink,
        target,
        valid_bits(&current),
        directory,
        &mut journal,
        mark_audio,
    )?;
    if let Some(shared) = active.as_mut() {
        shared.channels.insert(route_id, applied.min(channels));
    }
    Ok(applied)
}
impl Drop for Route {
    fn drop(&mut self) {
        if self.id == DETACHED {
            return;
        }
        let mut active = ACTIVE.lock().unwrap();
        let Some(shared) = active.as_mut() else {
            return;
        };
        shared.users -= 1;
        // The sink keeps its layout for the streams still on it.
        shared.channels.remove(&self.id);
        if shared.users != 0 {
            return;
        }
        if let Err(error) = restore(&shared.directory) {
            tracing::error!(%error, "audio defaults will be restored by crash recovery");
        } else {
            let _ = hooks::mark_audio(false);
        }
        active.take();
    }
}

#[cfg(test)]
mod tests;
