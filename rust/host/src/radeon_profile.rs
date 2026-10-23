//! A game's Radeon profile (plan item B5): when a stream launches an app that carries
//! `radeon-profile`, the host writes the profile's settings through the journaled Radeon
//! controls and the session restores them when it ends, as it does for a client's own writes
//! (`radeon_session`). The app's choice is validated when it is saved
//! (`butterpollo_core::radeon_profile`). The app's config is the admin's consent: the device
//! needs no host control for it.
//!
//! The write runs on a thread of its own, one at a time: every driver call starts a helper
//! process, and a stream does not wait for it. A failure is loud and the game still starts: it is
//! logged as an error and the session shows the profile as failed with the reason, so the client
//! can say so. Nothing is retried quietly, and nothing is applied that the driver did not take.
//!
//! Order matters when a launch takes the game over from another device: that device's Radeon
//! settings are put back first, so its restore cannot run after the new write and undo it, and
//! the journal takes the PC's own values as the originals. Changing profiles on one device puts
//! back only the settings the earlier profile wrote, not the device's own panel writes.
use crate::{radeon_session::READ_WAIT, session_notes::Held, state::Shared};
use butterpollo_core::{
    native_api::session::{ProfileState, RadeonProfile},
    radeon_profile::{self, Item, Selected},
    state::App,
};
use butterpollo_windows::radeon::{Change, model::Value};
use std::{
    sync::{Mutex, PoisonError},
    time::Duration,
};

/// The stream's frame rate from a launch `mode` (`WIDTHxHEIGHTxFPS`, the rate may have
/// decimals).
pub fn stream_fps(mode: &str) -> Option<u32> {
    let mut parts = mode.split('x');
    let (_, _, rate, None) = (parts.next()?, parts.next()?, parts.next()?, parts.next()) else {
        return None;
    };
    let rate: f64 = rate.trim().parse().ok()?;
    (rate.is_finite() && rate >= 1.0).then(|| rate.round() as u32)
}

fn change(item: &Item) -> Result<Change, String> {
    let value = match &item.value {
        serde_json::Value::Bool(on) => Value::Bool(*on),
        serde_json::Value::Number(number) if number.is_i64() => {
            Value::Int(number.as_i64().unwrap_or_default())
        }
        serde_json::Value::String(text) => Value::Choice(text.clone()),
        _ => return Err(format!("{}: not a boolean, whole number or text", item.id)),
    };
    Ok(Change {
        id: item.id.clone(),
        value,
    })
}

fn status(
    profile: &str,
    state: ProfileState,
    message: Option<String>,
    ids: &[String],
) -> RadeonProfile {
    RadeonProfile {
        profile: profile.to_owned(),
        state,
        message,
        settings: ids.to_vec(),
    }
}

/// One profile thread at a time. A newer launch's thread waits for the older one, finds that it
/// was superseded and writes nothing, or puts back what the older one wrote before it goes on.
static SERIAL: Mutex<()> = Mutex::new(());
/// How long a new game's settings wait for the restores of a device that lost its session.
const RELEASE_WAIT: Duration = Duration::from_secs(30);

/// The setting ids a held profile asked to write.
fn ids_of(held: &Held) -> Vec<String> {
    held.wanted.iter().map(|item| item.id.clone()).collect()
}

/// Run `work` on a thread of its own, behind the other profile threads. A stream never waits for
/// the driver: every call starts a helper process, and a restore can wait for the worker for
/// minutes.
fn background(h: &Shared, device: &str, work: impl FnOnce(&Shared) + Send + 'static) {
    let spawned = std::thread::Builder::new()
        .name("radeon-profile".into())
        .spawn({
            let h = h.clone();
            move || {
                {
                    let _one_at_a_time = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
                    work(&h);
                }
                h.radeon.forget();
                // The session may have ended while the driver wrote.
                h.session_tick();
            }
        });
    if let Err(error) = spawned {
        tracing::error!(device, %error, "the Radeon profile thread could not start; the settings stay journaled until the session ends");
    }
}

/// Put back an earlier profile's settings off the launch path.
fn put_back_earlier(h: &Shared, device: &str, earlier: Vec<String>) {
    if earlier.is_empty() {
        return;
    }
    let owner = device.to_owned();
    background(h, device, move |h| restore_earlier(h, &owner, &earlier));
}

/// A stream of `device` launched or resumed `app` at `fps`: apply its Radeon profile, and put
/// back an earlier game's when this one has none or another. Returns at once.
pub fn launched(h: &Shared, device: &str, app: Option<&App>, fps: Option<u32>) {
    let wanted = match app {
        Some(app) => radeon_profile::selected(&app.extra, fps),
        None => Ok(None),
    };
    let previous = h.notes.profile(device);
    // What the earlier game's profile wrote, to put back before another is written.
    let earlier: Vec<String> = previous.as_ref().map(ids_of).unwrap_or_default();
    let refuse = |profile: &str, message: String| {
        tracing::error!(profile, error = %message, "the app's Radeon profile is not applied");
        h.notes.fail_profile(device, profile, message);
        put_back_earlier(h, device, earlier.clone());
    };
    let selected = match wanted {
        Ok(selected) => selected,
        Err(error) => {
            // Saved apps are checked; this is a hand-edited apps file.
            let profile = app
                .and_then(|app| app.extra.get(radeon_profile::PROFILE))
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            refuse(profile, format!("{error:#}"));
            return;
        }
    };
    let Some(selected) = selected else {
        if previous.is_some() {
            h.notes.clear_profile(device);
            put_back_earlier(h, device, earlier);
        }
        return;
    };
    // A rate the stream did not state cannot be followed: refuse before anything is written.
    if let Some(id) = selected.without_stream_fps() {
        refuse(
            &selected.profile.id,
            format!(
                "{id} follows the stream's frame rate and this launch did not state one, so the profile was not applied"
            ),
        );
        return;
    }
    let Selected { profile, changes } = selected;
    let ids: Vec<String> = changes.iter().map(|item| item.id.clone()).collect();
    // The same app again (a resume, a reconnect) while its settings are in place or on their
    // way: nothing to do. After a session that ended they are gone, and so is `holds`.
    if let Some(previous) = &previous
        && previous.wanted == changes
        && previous.status.profile == profile.id
        && (previous.status.state == ProfileState::Applying
            || (previous.status.state == ProfileState::Applied && h.radeon.holds(device)))
    {
        return;
    }
    let generation = h.notes.begin_profile(
        device,
        status(&profile.id, ProfileState::Applying, None, &ids),
        changes.clone(),
    );
    let (device, profile) = (device.to_owned(), profile.id.clone());
    background(h, &device.clone(), move |h| {
        // A newer launch, or the end of the session, came while this waited.
        if !h.notes.is_current(&device, generation) {
            tracing::info!(device = %device, profile = %profile, "the Radeon profile was superseded before it was written");
            return;
        }
        // Another device's settings go back first: its restore must not run after this write
        // and undo it, and the journal must take the PC's own values as the originals.
        h.radeon
            .release_missing(&h.devices_after_launch(&device), RELEASE_WAIT);
        if !earlier.is_empty() {
            restore_earlier(h, &device, &earlier);
        }
        if !h.notes.is_current(&device, generation) {
            return;
        }
        let done = match apply(h, &device, changes) {
            Ok(()) => {
                tracing::info!(profile = %profile, settings = ?ids, "Radeon profile applied");
                status(&profile, ProfileState::Applied, None, &ids)
            }
            Err(message) => {
                tracing::error!(profile = %profile, error = %message, "the Radeon profile was not applied; the game runs with the settings it found");
                status(&profile, ProfileState::Failed, Some(message), &[])
            }
        };
        h.notes.finish_profile(&device, generation, done);
    });
}

/// Put back what an earlier profile of `device` wrote, and nothing else of the device's.
fn restore_earlier(h: &Shared, device: &str, ids: &[String]) {
    match h.radeon.restore_settings(device, ids) {
        Ok(report) if report.is_clean() => {}
        Ok(_) => tracing::error!(
            device,
            "the Radeon settings of the earlier game were not all put back; they stay journaled"
        ),
        Err(error) => tracing::error!(
            device,
            error = format!("{error:#}"),
            "the Radeon settings of the earlier game could not be put back; they stay journaled"
        ),
    }
}

fn apply(h: &Shared, device: &str, items: Vec<Item>) -> Result<(), String> {
    let worker = h
        .radeon
        .worker()
        .ok_or("this host has no Radeon controls")?;
    let adapter = h
        .config
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get("adapter_name", "")
        .to_owned();
    let gpu = worker
        .gpu(Some(&adapter), READ_WAIT)
        .map_err(|error| format!("the Radeon driver could not be read: {error}"))?
        .ok_or("this PC has no AMD GPU the Radeon controls can reach")?;
    // As the panel's own write does: a setting this GPU does not list is refused before any
    // write, and named.
    let (view, _) = crate::native_api::radeon::current_view(h);
    if let Some(listed) = view.gpus.iter().find(|listed| listed.id == gpu.id) {
        let missing: Vec<&str> = items
            .iter()
            .map(|item| item.id.as_str())
            .filter(|id| !listed.settings.iter().any(|setting| setting.id == *id))
            .collect();
        if !missing.is_empty() {
            return Err(format!(
                "this GPU has no Radeon setting {}",
                missing.join(", ")
            ));
        }
    }
    let changes = items.iter().map(change).collect::<Result<Vec<_>, _>>()?;
    h.radeon
        .apply(device, gpu.id, changes)
        .map(drop)
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stream_rate_comes_from_the_launch_mode() {
        assert_eq!(stream_fps("2560x1440x120"), Some(120));
        assert_eq!(stream_fps("1920x1080x59.94"), Some(60));
        assert_eq!(stream_fps("1920x1080x0"), None);
        assert_eq!(stream_fps("1920x1080"), None);
        assert_eq!(stream_fps("1x2x3x4"), None);
        assert_eq!(stream_fps(""), None);
        assert_eq!(stream_fps("x"), None);
    }

    #[test]
    fn items_become_driver_changes_or_name_the_setting() {
        let item = |value| Item {
            id: "a.b".into(),
            value,
        };
        assert_eq!(
            change(&item(serde_json::json!(true))).unwrap().value,
            Value::Bool(true)
        );
        assert_eq!(
            change(&item(serde_json::json!(7))).unwrap().value,
            Value::Int(7)
        );
        assert_eq!(
            change(&item(serde_json::json!("on"))).unwrap().value,
            Value::Choice("on".into())
        );
        assert!(
            change(&item(serde_json::json!(1.5)))
                .unwrap_err()
                .contains("a.b")
        );
    }
}
