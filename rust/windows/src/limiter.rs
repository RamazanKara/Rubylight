//! Stream-owned limiter transactions. Durable originals precede every mutation.
use crate::rtss;
use anyhow::{Context, Result, bail};
use butterpollo_core::{
    config::Config,
    framegen::{Policy, Provider, Rate},
    state::write_json,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};
#[derive(Clone, Serialize, Deserialize)]
struct RtssChange {
    root: PathBuf,
    before: BTreeMap<String, Option<u32>>,
    applied: BTreeMap<String, Option<u32>>,
    disabled: bool,
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
    write_json(&path(directory), journal)?;
    crate::display_recovery::external(true)
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
    if let Err(error) = write_json(&path(directory), &journal) {
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
        "A previous Rubylight 2.6 host stopped before it restored NVIDIA driver settings it changed for a stream ({names}). Rubylight 3.0 is AMD-only and cannot restore them. Open NVIDIA Control Panel, Manage 3D settings, and reset the Global profile and the butterpollo.exe program profile."
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
        let text = rtss::read(&change.root)?;
        let current = rtss::properties(&text)?;
        let mut restore = BTreeMap::new();
        // A user edit made after our application takes precedence on restore.
        for key in rtss::KEYS {
            if current.get(key) == change.applied.get(key) {
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
            for (key, value) in &restore {
                if let Some(value) = value
                    && restored.values.get(key) != Some(&Some(*value))
                {
                    bail!("RTSS did not confirm restoration of {key}");
                }
            }
            if disabled.is_some() && (restored.flags & 4 != 0) != change.disabled {
                bail!("RTSS did not restore its original enable state");
            }
        } else if rtss::running(&change.root) {
            bail!(
                "RTSS is still running but its hooks are unavailable; retaining exclusive limiter ownership"
            );
        }
        journal.rtss = None;
        write_json(&path(directory), &journal)?;
    }
    Ok(())
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
            explicit || policy.provider == Provider::Rtss,
            &state.active,
            &state.message,
            (policy.enabled.then_some(policy.rate), state.rate),
        )
    }
    pub fn acquire(directory: &Path, config: &Config, policy: &Policy) -> Result<Self> {
        let mut state = state().lock().unwrap();
        if state.users != 0 {
            state.users += 1;
            return Ok(Self);
        }
        state.directory = directory.into();
        state.active = "none".into();
        state.rate = policy.enabled.then_some(policy.rate);
        state.message.clear();
        if let Err(error) = recover(directory) {
            state.active = "rtss".into();
            state.message = format!("{error:#}");
            tracing::warn!(error = %state.message, "frame limiter recovery remains pending; retaining exclusive limiter ownership");
            state.users = 1;
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
            let applied = (|| -> Result<()> {
                // A failed restoration retains our process until recovery
                // succeeds. start() returns None for that still-running RTSS;
                // replacing its owner with None would kill it on reconnect.
                if let Some(process) = rtss::start(&root)? {
                    state.process = Some((root.clone(), process));
                }
                let reply = rtss::wait_ready(&root)?;
                let text = rtss::read(&root)?;
                let (numerator, denominator) = policy.rate.rational();
                let change = RtssChange {
                    root: root.clone(),
                    before: rtss::properties(&text)?,
                    applied: BTreeMap::from([
                        ("Limit".into(), Some(numerator)),
                        ("LimitDenominator".into(), Some(denominator)),
                        ("SyncLimiter".into(), Some(policy.sync_limiter)),
                    ]),
                    disabled: reply.flags & 4 != 0,
                };
                journal.rtss = Some(change.clone());
                persist(directory, &journal)?;
                let reply = rtss::apply(&root, &change.applied, Some(false))?;
                if reply.flags & 4 != 0 || reply.values != change.applied {
                    bail!("RTSS did not confirm the requested rational frame limit");
                }
                Ok(())
            })();
            match applied {
                Ok(()) => {
                    state.active = "rtss".into();
                    let version = rtss::version(&root);
                    tracing::info!(rate = ?policy.rate.rational(), sync_limiter = policy.sync_limiter, rtss_version = ?version, profile_sdk = rtss::profile_sdk(version), "RTSS frame limit applied and verified");
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
            tracing::warn!(provider = policy.provider.name(), rtss_root = %root.display(), rtss_available = rtss::available(&root), error = %state.message, "frame limiter has no available provider");
        }
        state.users = 1;
        Ok(Self)
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        let mut guard = state().lock().unwrap();
        let state = &mut *guard;
        state.users -= 1;
        if state.users != 0 {
            return;
        }
        match restore(&state.directory, &mut state.process) {
            Ok(()) => {
                let _ = crate::display_recovery::external(false);
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
pub fn status(config: &Config) -> Json {
    let root = rtss::root(config);
    let available = rtss::available(&root);
    let state = state().lock().unwrap();
    let configured = config.get("rtss_install_path", "");
    json!({"status":true,"enabled":config.boolean("frame_limiter_enable",false),
        "configured_provider":Provider::parse(config.get("frame_limiter_provider","auto")).name(),
        "active_provider":if state.active.is_empty(){"none"}else{&state.active},
        "rtss_available":available,"configured_path":configured,"path_configured":!configured.is_empty(),
        "resolved_path":root,"path_exists":root.is_dir(),"hooks_found":available,
        "rtss_version":rtss::version(&root).map(|v| v.map(|part| part.to_string()).join(".")),"profile_found":root.join("Profiles/Global").is_file(),
        "can_bootstrap_profile":available,"process_running":rtss::running(&root),
        "message":if state.message.is_empty(){"Limiter settings apply during streaming and restore after the last stream."}else{&state.message}})
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_journal_with_nvidia_entries_is_not_dropped_silently() {
        let directory = std::env::temp_dir().join(format!("limiter-nvidia-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            path(&directory),
            r#"{"version":1,"rtss":null,"nvidia":[{"id":277041154,"before":{"effective":0,"override_value":null},"applied":60},{"id":11041231,"before":{"effective":0,"override_value":null},"applied":134497095}]}"#,
        )
        .unwrap();
        // The entries survive loading and are written back with the journal.
        let journal = load(&directory).unwrap();
        assert_eq!(journal.nvidia.len(), 2);
        let message = take_stale_nvidia(&directory).expect("a warning");
        assert!(message.contains("Max Frame Rate"), "{message}");
        assert!(message.contains("Vertical sync"), "{message}");
        assert!(message.contains("NVIDIA Control Panel"), "{message}");
        // Warned once: the entries are gone, the rest of the journal stays.
        assert!(load(&directory).unwrap().nvidia.is_empty());
        assert!(take_stale_nvidia(&directory).is_none());
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn limiter_reports_missing_and_failed_providers() {
        assert!(
            limiter_warning(true, true, "none", "", (None, None))
                .unwrap()
                .contains("not applied")
        );
        assert!(
            limiter_warning(true, true, "rtss", "recovery pending", (None, None))
                .unwrap()
                .contains("recovery pending")
        );
        assert!(limiter_warning(false, true, "none", "", (None, None)).is_none());
        // The automatic virtual-display cap on a host without RTSS is not a fault.
        assert!(limiter_warning(true, false, "none", "", (None, None)).is_none());
        assert!(
            limiter_warning(true, false, "none", "RTSS did not confirm", (None, None))
                .unwrap()
                .contains("(RTSS did not confirm)")
        );
        assert!(
            !limiter_warning(true, true, "none", "", (None, None))
                .unwrap()
                .contains("()")
        );
        assert!(limiter_warning(true, true, "rtss", "", (None, None)).is_none());
        assert!(
            limiter_warning(
                true,
                true,
                "rtss",
                "",
                (Some(Rate(116_000)), Some(Rate(60_000)))
            )
            .unwrap()
            .contains("60 fps")
        );
        assert!(limiter_warning(false, true, "rtss", "", (None, Some(Rate(60_000)))).is_some());
        assert!(
            limiter_warning(
                true,
                true,
                "rtss",
                "",
                (Some(Rate(60_000)), Some(Rate(60_000)))
            )
            .is_none()
        );
    }
}
