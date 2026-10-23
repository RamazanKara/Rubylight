//! A game's Radeon profile (plan item B5): when a stream launches an app that carries
//! `radeon-profile`, the host writes the profile's settings through the journaled Radeon
//! controls and the session restores them when it ends, as it does for a client's own writes
//! (`radeon_session`). The app's choice is validated when it is saved
//! (`butterpollo_core::radeon_profile`).
//!
//! The write runs on a thread of its own: every driver call starts a helper process, and a
//! stream does not wait for it. A failure is loud and the game still starts: it is logged as an
//! error and the session shows the profile as failed with the reason, so the client can say so.
//! Nothing is retried quietly, and nothing is applied that the driver did not take.
use crate::{radeon_session::READ_WAIT, state::Shared};
use butterpollo_core::{
    native_api::session::{ProfileState, RadeonProfile},
    radeon_profile::{self, Item, Selected},
    state::App,
};
use butterpollo_windows::radeon::{Change, model::Value};

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

/// A stream of `device` launched or resumed `app` at `fps`: apply its Radeon profile, and put
/// back an earlier game's when this one has none or another. Returns at once.
pub fn launched(h: &Shared, device: &str, app: Option<&App>, fps: Option<u32>) {
    let wanted = match app {
        Some(app) => radeon_profile::selected(&app.extra, fps),
        None => Ok(None),
    };
    let previous = h.notes.profile(device);
    let selected = match wanted {
        Ok(selected) => selected,
        Err(error) => {
            // Saved apps are checked; this is a hand-edited apps file.
            let name = app.map_or(String::new(), |app| app.name.clone());
            tracing::error!(app = %name, error = format!("{error:#}"), "the app's Radeon profile cannot be read; it is not applied");
            let profile = app
                .and_then(|app| app.extra.get(radeon_profile::PROFILE))
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned();
            if previous.is_some() {
                restore_earlier(h, device);
            }
            h.notes.fail_profile(device, &profile, format!("{error:#}"));
            return;
        }
    };
    let Some(Selected { profile, changes }) = selected else {
        if previous.is_some() {
            h.notes.clear_profile(device);
            restore_earlier(h, device);
        }
        return;
    };
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
    let spawned = std::thread::Builder::new()
        .name("radeon-profile".into())
        .spawn({
            let (h, device, profile) = (h.clone(), device.clone(), profile.clone());
            move || {
                if previous.is_some() {
                    restore_earlier(&h, &device);
                }
                let result = apply(&h, &device, changes);
                let done = match result {
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
                h.radeon.forget();
                // The session may have ended while the driver wrote.
                h.session_tick();
            }
        });
    if let Err(error) = spawned {
        let message = format!("the Radeon profile thread could not start: {error}");
        tracing::error!(%message);
        h.notes.finish_profile(
            &device,
            generation,
            status(&profile, ProfileState::Failed, Some(message), &[]),
        );
    }
}

/// Put back what an earlier profile of `device` changed.
fn restore_earlier(h: &Shared, device: &str) {
    match h.radeon.restore_now(device) {
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
