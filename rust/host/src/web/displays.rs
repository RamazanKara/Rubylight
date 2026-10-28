//! Display layout, recovery, and HDR API handlers.

use crate::state::Shared;
use anyhow::Context;
use butterpollo_core::state;
use serde_json::{Value, json};

pub(super) fn handle(
    h: &Shared,
    method: &str,
    uri: &axum::http::Uri,
    data: &Value,
) -> anyhow::Result<Value> {
    let path = uri.path();
    Ok(match (method, path) {
        ("GET", "/api/display/golden_status") => crate::maintenance::golden_status(
            h,
            uri.query().is_some_and(|q| q.contains("compare_current=1")),
        )?,
        ("POST", "/api/display/export_golden") => crate::maintenance::capture_golden(h)?,
        ("POST", "/api/display/restore_golden") => crate::maintenance::restore_golden(h)?,
        ("POST", "/api/reset-display-device-persistence") => {
            reset_persistence(h, butterpollo_windows::display_recovery::reset)?
        }
        ("DELETE", "/api/display/golden") => {
            let path = h.directory.join("display-baseline-rust.json");
            let exists = path.exists();
            if exists {
                std::fs::remove_file(path)?;
            }
            state::atomic_write(
                &h.directory.join("display-baseline-disabled"),
                b"legacy baseline import disabled\n",
            )?;
            json!({"status":true,"deleted":exists})
        }
        ("POST", "/api/display/terminate_virtual") => {
            crate::runtime::release_displays(h);
            json!({"status":true})
        }
        ("GET", "/api/clients/display-layout") => crate::remote_display::snapshot(h)?,
        ("PUT", "/api/clients/display-layout") => crate::remote_display::save(h, data)?,
        ("GET", "/api/health/vulkan-hdr-layer") => butterpollo_windows::vulkan::status(
            h.config.read().unwrap().boolean("vulkan_hdr_layer", true),
        ),
        ("POST", "/api/health/vulkan-hdr-layer/register") => {
            butterpollo_windows::vulkan::register(true)?;
            butterpollo_windows::vulkan::status(
                h.config.read().unwrap().boolean("vulkan_hdr_layer", true),
            )
        }
        ("GET", "/api/clients/hdr-profiles") => {
            let path = std::path::PathBuf::from(
                std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into()),
            )
            .join("System32/spool/drivers/color");
            let mut profiles: Vec<_> = std::fs::read_dir(path)?.filter_map(Result::ok).filter(|e| e.path().extension().and_then(|s|s.to_str()).is_some_and(|s|s.eq_ignore_ascii_case("icc") || s.eq_ignore_ascii_case("icm"))).take(1024).map(|e|json!({"filename":e.file_name().to_string_lossy(),"added_ms":e.metadata().ok().and_then(|m|m.created().ok()).and_then(|t|t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d|d.as_millis()).unwrap_or(0)})).collect();
            profiles.sort_by_key(|p| std::cmp::Reverse(p["added_ms"].as_u64().unwrap_or(0)));
            json!({"status":true,"profiles":profiles})
        }
        ("GET", "/api/display-devices") => {
            let monitors = butterpollo_windows::input::on_input_desktop(
                butterpollo_windows::display::monitors,
            )
            .map_err(|error| anyhow::anyhow!("Windows display enumeration failed ({error:#}). If Windows is at the sign-in screen or an RDP session was disconnected, sign in to and unlock the console session, then retry"))?;
            serde_json::to_value(monitors)?
        }
        ("GET", "/api/framegen/edid-refresh") => {
            let query: std::collections::HashMap<_, _> =
                url::form_urlencoded::parse(uri.query().unwrap_or("").as_bytes())
                    .into_owned()
                    .collect();
            let hint = ["device_id", "device", "id", "display"]
                .into_iter()
                .find_map(|key| query.get(key).filter(|value| !value.trim().is_empty()))
                .context("device_id query parameter is required")?;
            let mut targets = query
                .get("targets")
                .map(|text| {
                    text.split(',')
                        .filter_map(|value| value.trim().parse::<u32>().ok())
                        .filter(|hz| *hz > 0 && *hz <= 4000)
                        .take(64)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            if targets.is_empty() {
                targets = vec![120, 180, 240, 288];
            }
            butterpollo_windows::display::edid_refresh(hint, &targets)?
        }
        _ => return Err(anyhow::anyhow!("unknown API endpoint")),
    })
}

/// Recovery puts the physical layout back and switches virtual displays off,
/// so it runs only while nothing streams, launches or tears down, with the
/// launch gate held so no launch starts meanwhile.
fn reset_persistence(
    h: &Shared,
    reset: impl FnOnce() -> anyhow::Result<()>,
) -> anyhow::Result<Value> {
    let Some(_gate) = crate::power_action::idle(h) else {
        anyhow::bail!("disconnect active sessions before resetting display persistence");
    };
    reset()?;
    Ok(json!({"status":true}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::test_support::Fixture;
    use std::time::Instant;

    #[test]
    fn display_persistence_resets_only_while_no_stream_starts_or_ends() {
        let f = Fixture::new();
        let resets = std::cell::Cell::new(0);
        let reset = || {
            resets.set(resets.get() + 1);
            Ok(())
        };
        // A launch holds the gate while its display is prepared.
        let launching = f.host.launch_transition.lock().unwrap();
        assert!(reset_persistence(&f.host, reset).is_err());
        drop(launching);
        f.host
            .sessions
            .lock()
            .unwrap()
            .teardown
            .insert("x".into(), Instant::now());
        assert!(reset_persistence(&f.host, reset).is_err());
        f.host.sessions.lock().unwrap().teardown.clear();
        f.host.monitors.lock().unwrap().insert(
            "x".into(),
            std::sync::Arc::new(butterpollo_windows::display::Retained::inert("x")),
        );
        assert!(reset_persistence(&f.host, reset).is_err());
        f.host.monitors.lock().unwrap().clear();
        assert_eq!(resets.get(), 0);
        assert_eq!(reset_persistence(&f.host, reset).unwrap()["status"], true);
        assert_eq!(resets.get(), 1);
        // The launch gate is free again afterwards.
        assert!(f.host.launch_transition.try_lock().is_ok());
    }
}
