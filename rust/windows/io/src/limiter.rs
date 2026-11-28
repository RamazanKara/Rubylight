//! Stream-owned limiter transactions. Durable originals precede every mutation.
use crate::rtss;
use anyhow::{Context, Result, bail};
use butterpollo_config::config::Config;
use butterpollo_media_policy::framegen::{Policy, Provider, Rate};
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

mod follow;
pub mod hooks;

use follow::{
    Follow, change_clamped, change_followed, next_follow, queue_clamp_enabled, sync_flags,
};
pub use follow::{display_read_due, follow_display, follow_queue_clamp, following, queue_clamping};
#[cfg(test)]
use follow::{follows, no_limit, wanted_change};

#[derive(Clone, Serialize, Deserialize)]
struct RtssChange {
    root: PathBuf,
    before: BTreeMap<String, Option<u32>>,
    applied: BTreeMap<String, Option<u32>>,
    disabled: bool,
    /// What this host gave RTSS in place of `applied` while the stream's display was too
    /// slow for the cap ([`follow_display`]). A profile that holds these is the host's
    /// doing as well, so restoring replaces them like `applied`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stood_down: Option<BTreeMap<String, Option<u32>>>,
    /// Dynamic queue-clamp rate this host gave RTSS in place of `applied`
    /// ([`follow_queue_clamp`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    clamped: Option<BTreeMap<String, Option<u32>>>,
    /// Previously live queue-clamp rate retained in the recovery journal while a new clamp or
    /// stand-down is being applied to RTSS, so a failed write or crash mid-transition still
    /// recognizes the profile value as host-owned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    previous_clamped: Option<BTreeMap<String, Option<u32>>>,
}
impl RtssChange {
    /// Whether the profile's `key` still holds a value this host wrote. A user's edit made
    /// after it takes precedence.
    fn ours(&self, key: &str, current: &BTreeMap<String, Option<u32>>) -> bool {
        current.get(key) == self.applied.get(key)
            || self
                .stood_down
                .as_ref()
                .and_then(|values| values.get(key))
                .is_some_and(|value| current.get(key) == Some(value))
            || self
                .clamped
                .as_ref()
                .and_then(|values| values.get(key))
                .is_some_and(|value| current.get(key) == Some(value))
            || self
                .previous_clamped
                .as_ref()
                .and_then(|values| values.get(key))
                .is_some_and(|value| current.get(key) == Some(value))
    }
}
#[derive(Clone, Default, Serialize, Deserialize)]
struct Journal {
    version: u32,
    rtss: Option<RtssChange>,
    /// NVIDIA driver profile changes a 2.6 host journalled. 3.0 cannot restore
    /// them, but must not lose track of them silently; see `take_stale_nvidia`.
    #[serde(default)]
    nvidia: Vec<Json>,
}
#[derive(Default)]
struct State {
    users: usize,
    active: String,
    rate: Option<Rate>,
    directory: PathBuf,
    process: Option<(PathBuf, crate::process::Process)>,
    message: String,
    /// The automatic cap [`follow_display`] takes off and puts back.
    follow: Option<Follow>,
}
fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(Mutex::default)
}
fn path(directory: &Path) -> PathBuf {
    directory.join("frame-limiter-recovery.json")
}
fn load(directory: &Path) -> Result<Journal> {
    match std::fs::metadata(path(directory)) {
        Ok(m) if m.len() <= 1024 * 1024 => {
            let j: Journal = serde_json::from_slice(&std::fs::read(path(directory))?)?;
            if j.version != 1 {
                bail!("unsupported frame limiter recovery version");
            }
            Ok(j)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Journal {
            version: 1,
            ..Default::default()
        }),
        _ => bail!("frame limiter recovery is unreadable or exceeds its size limit"),
    }
}
fn persist(directory: &Path, journal: &Journal) -> Result<()> {
    hooks::persist_json(&path(directory), journal)
}
/// The NVIDIA driver settings a 2.6 host changed for a stream and, after a
/// crash, left in `frame-limiter-recovery.json` (a hybrid AMD and NVIDIA
/// laptop). 3.0 has no NVIDIA code to put them back, so name them for the user
/// and drop the entries, which would otherwise be retained and ignored forever.
/// The warning shows once; if the journal cannot be written it shows again at
/// the next start.
pub fn take_stale_nvidia(directory: &Path) -> Option<String> {
    let mut journal = match load(directory) {
        Ok(journal) => journal,
        Err(error) => {
            tracing::warn!(error = %format!("{error:#}"), "frame limiter recovery journal is unreadable");
            return None;
        }
    };
    let message = stale_nvidia_warning(&journal.nvidia)?;
    journal.nvidia.clear();
    if let Err(error) = butterpollo_session::state::write_json(&path(directory), &journal) {
        tracing::warn!(error = %format!("{error:#}"), "the stale NVIDIA entries could not be removed from the recovery journal");
    }
    Some(message)
}
fn stale_nvidia_warning(entries: &[Json]) -> Option<String> {
    if entries.is_empty() {
        return None;
    }
    let mut names = Vec::new();
    for id in entries.iter().filter_map(|entry| entry["id"].as_u64()) {
        let name = match id {
            0x1083_5002 => "Max Frame Rate".to_string(),
            0x00a8_79cf => "Vertical sync".to_string(),
            0xb0d3_84c0 | 0xb0cc_0875 | 0x0005_f543 | 0x1083_5000 | 0x20d6_90f8 => {
                "Smooth Motion".to_string()
            }
            other => format!("driver setting {other:#010x}"),
        };
        if !names.contains(&name) {
            names.push(name);
        }
    }
    let names = if names.is_empty() {
        "frame limit and vertical sync".to_string()
    } else {
        names.join(", ")
    };
    Some(format!(
        "A previous Rubylight 2.6 host stopped before it restored NVIDIA driver settings it changed for a stream ({names}). Rubylight 3.0 cannot restore them. Open NVIDIA Control Panel, Manage 3D settings, and reset the Global profile and the butterpollo.exe program profile."
    ))
}
/// Also used by the Rust crash watcher. Failures leave the journal intact.
pub fn recover(directory: &Path) -> Result<()> {
    restore(directory, &mut None)
}
/// `owned`: the RTSS this host started for the stream. Closing it ends every
/// live change, and RTSS reads the restored profile when it next starts, so
/// nothing is left to restore through its SDK (Vibepollo also closes it).
fn restore(directory: &Path, owned: &mut Option<(PathBuf, crate::process::Process)>) -> Result<()> {
    let mut journal = load(directory)?;
    if let Some(change) = journal.rtss.as_ref() {
        let missing_dir = rtss::is_local_path(&change.root)
            && std::fs::symlink_metadata(&change.root)
                .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound);
        if missing_dir && !rtss::running(&change.root) {
            tracing::info!(
                root = %change.root.display(),
                "RTSS installation folder is gone; clearing stale frame limiter recovery entry"
            );
            journal.rtss = None;
            butterpollo_session::state::write_json(&path(directory), &journal)?;
            return Ok(());
        }
        let text = rtss::read(&change.root)?;
        let current = rtss::properties(&text)?;
        let mut restore = BTreeMap::new();
        // A user edit made after our application takes precedence on restore.
        for key in rtss::KEYS {
            if change.ours(key, &current) {
                restore.insert(key.into(), change.before.get(key).copied().flatten());
            }
        }
        // Restore the durable profile even if RTSS's message loop is stalled.
        // Keep the journal until the helper also confirms the live state.
        if !restore.is_empty() {
            rtss::write_profile(&change.root, &restore)?;
        }
        if let Some((_, process)) = owned.take_if(|(root, _)| *root == change.root) {
            process
                .stop()
                .context("close the RTSS started for the stream")?;
        } else if rtss::available(&change.root) {
            let _process = rtss::start(&change.root)?;
            let before = rtss::wait_ready(&change.root)
                .context("ask RTSS for its state before restoring")?;
            let disabled = if before.flags & 4 == 0 {
                Some(change.disabled)
            } else {
                None
            };
            let restored = rtss::apply(&change.root, &restore, disabled)
                .context("give RTSS its original frame limit")?;
            let unsupported = confirmed(&restore, &restored.values, false)
                .map_err(|key| anyhow::anyhow!("RTSS did not confirm restoration of {key}"))?;
            warn_unsupported(&unsupported);
            if disabled.is_some() && (restored.flags & 4 != 0) != change.disabled {
                bail!("RTSS did not restore its original enable state");
            }
        } else if rtss::running(&change.root) {
            bail!(
                "RTSS is still running but its hooks are unavailable; retaining exclusive limiter ownership"
            );
        }
        journal.rtss = None;
        butterpollo_session::state::write_json(&path(directory), &journal)?;
    }
    Ok(())
}
/// Whether RTSS's reply (`reported`) holds each value of `wanted`: the
/// key it holds another value for, or the keys it doesn't report at all,
/// which this RTSS doesn't support (its profile file still has them, from
/// [`rtss::write_profile`]). A key originally left out (None) is RTSS's
/// default and can't be told apart. A limit RTSS states another way is the
/// same cap: 2997/25 as 11988/100, a denominator of 0 as 1, and any
/// denominator without a limit (Limit 0 is no cap). Otherwise such a
/// restoration stayed pending forever, holding the limiter at every stream.
/// An unreported `Limit` is never confirmed, and when `applying` a stream's
/// cap, neither is an unreported denominator other than 1.
fn confirmed(
    wanted: &BTreeMap<String, Option<u32>>,
    reported: &BTreeMap<String, Option<u32>>,
    applying: bool,
) -> std::result::Result<Vec<String>, String> {
    let value = |map: &BTreeMap<String, Option<u32>>, key: &str| map.get(key).copied().flatten();
    let same_cap = || {
        let (Some(limit), Some(denominator)) = (
            value(reported, "Limit"),
            value(reported, "LimitDenominator"),
        ) else {
            return false;
        };
        let wanted_limit = match wanted.get("Limit") {
            Some(wanted) => wanted.unwrap_or(0),
            None => limit,
        };
        let wanted_denominator = match wanted.get("LimitDenominator") {
            Some(wanted) => wanted.unwrap_or(1),
            None => denominator,
        };
        rtss::equivalent_cap(limit, denominator, wanted_limit, wanted_denominator)
    };
    let mut unsupported = Vec::new();
    for (key, want) in wanted {
        let Some(want) = *want else { continue };
        match value(reported, key) {
            // No cap is confirmed without the limit itself, and a stream's
            // fractional cap not without its denominator: RTSS could be
            // holding 2997 fps for 29.97.
            None if key == "Limit" => return Err(key.clone()),
            None if applying && key == "LimitDenominator" && want.max(1) != 1 => {
                return Err(key.clone());
            }
            None => unsupported.push(key.clone()),
            Some(got) if got == want => {}
            Some(_) if matches!(key.as_str(), "Limit" | "LimitDenominator") && same_cap() => {}
            Some(_) => return Err(key.clone()),
        }
    }
    Ok(unsupported)
}
/// Says once per host start which keys this RTSS doesn't report
/// ([`confirmed`]).
fn warn_unsupported(keys: &[String]) {
    static WARNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if !keys.is_empty() && !WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        tracing::warn!(
            keys = keys.join(", "),
            "this RTSS does not report these frame limiter settings, so Rubylight sets them in its Global profile but cannot confirm them live; RTSS 7.3.7 or newer reports them"
        );
    }
}
/// What the console says when a stream's cap went to RTSS but RTSS didn't
/// report `keys` back ([`confirmed`]).
fn unconfirmed_message(keys: &[String]) -> String {
    format!(
        "RTSS does not report {}, so Rubylight could not confirm them; RTSS 7.3.7 or newer reports them",
        keys.join(", ")
    )
}
/// Whether a stream that ends up with no limiter provider is a fault worth a
/// warning in the host log: the user asked for a cap (`frame_limiter_enable`,
/// or RTSS by name) or a provider failed. `frame_limiter_provider = none`
/// turns limiting off on purpose, and the automatic virtual-display cap on a
/// host without RTSS simply stays off; both are quiet, as the console's
/// warning ([`limiter_warning`]) already is.
fn missing_provider_is_a_fault(explicit: bool, provider: Provider, error: &str) -> bool {
    provider != Provider::None && (explicit || provider == Provider::Rtss || !error.is_empty())
}
/// Whether the configured limiter can apply a cap now: RTSS is installed for `auto` and `rtss`,
/// and `frame_limiter_provider = none` is no limiter at all.
pub fn provider_usable(config: &Config) -> bool {
    Provider::parse(config.get("frame_limiter_provider", "auto")) != Provider::None
        && rtss::available(&rtss::root(config))
}
/// Says once per host run, not once per launch, that a virtual display's automatic cap needs
/// RTSS. The host keeps that cap off instead ([`Policy::with_provider_available`]); a cap the
/// user asked for still says on every stream that it was not applied. A provider turned off on
/// purpose (`none`) says nothing.
pub fn warn_automatic_cap_needs_rtss_once(config: &Config) {
    if Provider::parse(config.get("frame_limiter_provider", "auto")) == Provider::None {
        return;
    }
    static WARNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if !WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        tracing::warn!(
            "the virtual display's automatic frame limit needs RivaTuner Statistics Server (RTSS), which is not installed, so games on it run uncapped; install RTSS to cap them at the stream's frame rate"
        );
    }
}
/// `explicit`: the user asked for a cap ("Limit every stream" or a named
/// provider), rather than the automatic virtual-display limit, which simply
/// stays off on a host without RTSS.
fn limiter_warning(
    enabled: bool,
    explicit: bool,
    active: &str,
    error: &str,
    rates: (Option<Rate>, Option<Rate>),
) -> Option<String> {
    if !enabled && rates.1.is_none() {
        return None;
    }
    if active == "none" || active.is_empty() {
        if error.is_empty() && !explicit {
            return None;
        }
        let cause = if error.is_empty() {
            String::new()
        } else {
            format!(" ({error})")
        };
        Some(format!(
            "Frame limiter not applied: no usable RTSS{cause}. Game frame pacing may be uneven; install RTSS or set an in-game frame limit."
        ))
    } else if !error.is_empty() {
        Some(format!(
            "Frame limiter settings were not fully applied ({error}). Check RTSS before relying on the requested cap."
        ))
    } else if let Some(rate) = rates.1
        && rates.0 != rates.1
    {
        Some(if rates.0.is_some() {
            format!(
                "Another stream owns the frame limiter at {rate} fps; this session did not apply its requested cap. Match the other stream or disconnect it before changing the global limit."
            )
        } else {
            format!(
                "Another stream's {rate} fps frame limit also applies to games in this session. Disconnect that stream to remove the cap."
            )
        })
    } else {
        None
    }
}

pub struct Lease;
impl Lease {
    pub fn warning(&self, policy: &Policy, explicit: bool) -> Option<String> {
        let state = state().lock().unwrap();
        limiter_warning(
            policy.enabled,
            policy.provider != Provider::None
                && (explicit || !policy.follows_display || policy.provider == Provider::Rtss),
            &state.active,
            &state.message,
            (policy.enabled.then_some(policy.rate), state.rate),
        )
    }
    pub fn acquire(directory: &Path, config: &Config, policy: &Policy) -> Result<Self> {
        let mut state = state().lock().unwrap();
        if state.users != 0 {
            state.users += 1;
            // Another stream joins: the cap is no longer one display's to stand down or clamp for.
            if state.follow.as_ref().is_some_and(|f| f.standing_down) {
                change_followed(&mut state, false, None);
            } else if state
                .follow
                .as_ref()
                .is_some_and(|f| f.clamped_rate.is_some())
            {
                change_clamped(&mut state, None);
            }
            if let Some(follow) = state.follow.as_mut() {
                follow.shared = true;
            }
            sync_flags(&state);
            return Ok(Self);
        }
        state.directory = directory.into();
        state.active = "none".into();
        state.rate = None;
        state.message.clear();
        state.follow = None;
        if let Err(error) = recover(directory) {
            state.active = "rtss".into();
            state.rate = None;
            state.message = format!("{error:#}");
            tracing::warn!(error = %state.message, "frame limiter recovery remains pending; retaining exclusive limiter ownership");
            state.users = 1;
            sync_flags(&state);
            return Ok(Self);
        }
        let mut journal = load(directory)?;
        let root = rtss::root(config);
        let use_rtss = policy.enabled
            && matches!(policy.provider, Provider::Auto | Provider::Rtss)
            && rtss::available(&root);
        if !use_rtss
            || state
                .process
                .as_ref()
                .is_some_and(|(owned_root, _)| owned_root != &root)
        {
            state.process.take();
        }
        if use_rtss {
            let applied = apply_cap(
                directory,
                &mut journal,
                &root,
                (policy.rate, policy.sync_limiter),
                &mut state.process,
            );
            match applied {
                Ok(unsupported) => {
                    state.active = "rtss".into();
                    state.rate = Some(policy.rate);
                    let clamp_on = queue_clamp_enabled(config);
                    state.follow = (policy.follows_display || clamp_on).then(|| Follow {
                        id: next_follow(),
                        root: root.clone(),
                        rate: policy.rate,
                        follows_display: policy.follows_display,
                        standing_down: false,
                        queue_clamp: clamp_on,
                        clamped_rate: None,
                        changing: false,
                        failed: false,
                        shared: false,
                    });
                    let version = rtss::version(&root);
                    if unsupported.is_empty() {
                        tracing::info!(rate = ?policy.rate.rational(), sync_limiter = policy.sync_limiter, rtss_version = ?version, profile_sdk = rtss::profile_sdk(version), "RTSS frame limit applied and verified");
                    } else {
                        state.message = unconfirmed_message(&unsupported);
                        tracing::warn!(rate = ?policy.rate.rational(), sync_limiter = policy.sync_limiter, rtss_version = ?version, profile_sdk = rtss::profile_sdk(version), "RTSS frame limit applied but not verified: {}", state.message);
                    }
                }
                Err(error) => {
                    state.message = format!("{error:#}");
                    tracing::warn!(root = %root.display(), error = %state.message, "RTSS frame limiter could not be applied");
                    if journal.rtss.is_some()
                        && let Err(error) = restore(directory, &mut state.process)
                    {
                        tracing::warn!(error = %format!("{error:#}"), "RTSS frame limiter could not be restored after the failure");
                        state.active = "rtss".into();
                    }
                }
            }
        }
        if state.active == "none" && policy.enabled {
            if missing_provider_is_a_fault(
                config.boolean("frame_limiter_enable", false) || !policy.follows_display,
                policy.provider,
                &state.message,
            ) {
                // A user's cap that no provider can apply is a fault, but it is the same fault
                // on every launch: say it once per host run, and keep the rest at DEBUG.
                static WARNED: std::sync::atomic::AtomicBool =
                    std::sync::atomic::AtomicBool::new(false);
                if WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                    tracing::debug!(provider = policy.provider.name(), rtss_root = %root.display(), rtss_available = rtss::available(&root), error = %state.message, "frame limiter has no available provider");
                } else {
                    tracing::warn!(provider = policy.provider.name(), rtss_root = %root.display(), rtss_available = rtss::available(&root), error = %state.message, "frame limiter has no available provider");
                }
            } else {
                tracing::debug!(provider = policy.provider.name(), rtss_root = %root.display(), rtss_available = rtss::available(&root), "frame limiter stays off: no provider was asked for or is available");
            }
        }
        state.users = 1;
        sync_flags(&state);
        Ok(Self)
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        let mut guard = state().lock().unwrap();
        let state = &mut *guard;
        state.users -= 1;
        if state.users != 0 {
            sync_flags(state);
            return;
        }
        state.follow = None;
        sync_flags(state);
        match restore(&state.directory, &mut state.process) {
            Ok(()) => {
                let _ = hooks::mark_external(false);
                state.active = "none".into();
                state.message.clear();
                state.process.take();
            }
            Err(error) => {
                state.message = format!("{error:#}");
                tracing::warn!(error = %state.message, "frame limiter restoration remains pending");
            }
        }
    }
}
/// Journal RTSS's values as they are and give it a stream's cap. `process`: the RTSS this
/// host started, if it had to.
fn apply_cap(
    directory: &Path,
    journal: &mut Journal,
    root: &Path,
    (rate, sync_limiter): (Rate, u32),
    process: &mut Option<(PathBuf, crate::process::Process)>,
) -> Result<Vec<String>> {
    // A failed restoration retains our process until recovery
    // succeeds. start() returns None for that still-running RTSS;
    // replacing its owner with None would kill it on reconnect.
    if let Some(started) = rtss::start(root)? {
        *process = Some((root.into(), started));
    }
    let reply = rtss::wait_ready(root)?;
    let text = rtss::read(root)?;
    let (numerator, denominator) = rate.rational();
    let change = RtssChange {
        root: root.into(),
        before: rtss::properties(&text)?,
        applied: BTreeMap::from([
            ("Limit".into(), Some(numerator)),
            ("LimitDenominator".into(), Some(denominator)),
            ("SyncLimiter".into(), Some(sync_limiter)),
        ]),
        disabled: reply.flags & 4 != 0,
        stood_down: None,
        clamped: None,
        previous_clamped: None,
    };
    journal.rtss = Some(change.clone());
    persist(directory, journal)?;
    let reply = rtss::apply(root, &change.applied, Some(false))?;
    if reply.flags & 4 != 0 {
        bail!("RTSS did not confirm the requested rational frame limit");
    }
    confirmed(&change.applied, &reply.values, true)
        .map_err(|_| anyhow::anyhow!("RTSS did not confirm the requested rational frame limit"))
}
/// The rate RTSS holds games to while a stream holds the frame limiter, if it limits.
pub fn active_rate() -> Option<Rate> {
    let state = state().lock().unwrap();
    (state.users > 0
        && state.active == "rtss"
        && !state.follow.as_ref().is_some_and(|f| f.standing_down))
    .then(|| {
        state
            .follow
            .as_ref()
            .and_then(|f| f.clamped_rate)
            .or(state.rate)
    })
    .flatten()
}
pub fn status(config: &Config) -> Json {
    let root = rtss::root(config);
    let configured = config.get("rtss_install_path", "");
    let (active, message) = {
        let state = state().lock().unwrap();
        (state.active.clone(), state.message.clone())
    };
    let available = rtss::available(&root);
    json!({"status":true,"enabled":config.boolean("frame_limiter_enable",false),
        "configured_provider":Provider::parse(config.get("frame_limiter_provider","auto")).name(),
        "active_provider":if active.is_empty(){"none"}else{&active},
        "rtss_available":available,"configured_path":configured,"path_configured":!configured.is_empty(),
        "resolved_path":root,"path_exists":root.is_dir(),"hooks_found":available,
        "rtss_version":rtss::version(&root).map(|v| v.map(|part| part.to_string()).join(".")),"profile_found":root.join("Profiles/Global").is_file(),
        "can_bootstrap_profile":available,"process_running":rtss::running(&root),
        "message":if message.is_empty(){"Limiter settings apply during streaming and restore after the last stream."}else{&message}})
}

#[cfg(test)]
mod tests;
