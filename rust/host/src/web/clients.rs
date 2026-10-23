//! Paired-client management and pairing API handlers.

use crate::state::Shared;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

pub(crate) fn handle(h: &Shared, method: &str, path: &str, data: &Value) -> anyhow::Result<Value> {
    let text = |k: &str| data.get(k).and_then(Value::as_str).unwrap_or("");
    Ok(match (method, path) {
        ("GET", "/api/clients/list") => {
            let loss_routes = h.keyframe_loss_routes();
            let connected = h
                .sessions
                .lock()
                .unwrap()
                .active
                .values()
                .filter(|s| !s.stopping())
                .map(|s| s.launch.client.uuid.clone())
                .collect::<std::collections::HashSet<_>>();
            let clients: Vec<_> = h
                .paired
                .read()
                .unwrap()
                .clients
                .iter()
                .map(|c| {
                    let mut v = serde_json::to_value(c).unwrap();
                    v.as_object_mut().unwrap().remove("cert");
                    v["connected"] = json!(connected.contains(&c.uuid));
                    v["network_loss_routes"] = loss_routes
                        .get(&c.uuid)
                        .cloned()
                        .unwrap_or_else(|| json!([]));
                    v
                })
                .collect();
            json!({"status":true,"clients":clients,"named_certs":clients})
        }
        ("POST", "/api/clients/unpair") => {
            let canceled = h.sessions.lock().unwrap().request_stop(Some(text("uuid")));
            drop(canceled);
            crate::remote_display::disconnect(h, Some(text("uuid")));
            h.unpair_with_network_history(Some(text("uuid")))?;
            // The device is gone; its session may linger for the grace period, its Radeon
            // settings do not.
            h.radeon.release_device(text("uuid"));
            json!({"status":true})
        }
        ("POST", "/api/clients/unpair-all") => {
            let canceled = h.sessions.lock().unwrap().request_stop(None);
            drop(canceled);
            crate::remote_display::disconnect(h, None);
            h.unpair_with_network_history(None)?;
            h.radeon.release_all();
            json!({"status":true})
        }
        ("POST", "/api/clients/disconnect") => {
            let canceled = h.sessions.lock().unwrap().request_stop(Some(text("uuid")));
            drop(canceled);
            crate::remote_display::disconnect(h, Some(text("uuid")));
            json!({"status":true})
        }
        ("POST", "/api/clients/reset-network-history") => {
            h.reset_keyframe_loss(text("uuid"))?;
            json!({"status":true})
        }
        ("POST", "/api/clients/update") => {
            let mut state = h.paired.write().unwrap();
            let mut next = state.clone();
            let c = next
                .clients
                .iter_mut()
                .find(|c| c.uuid == text("uuid"))
                .ok_or_else(|| anyhow::anyhow!("client not found"))?;
            // Host control is granted only here, and only as a real boolean
            // (null clears it). Anything else is refused before any change.
            if data
                .get("host_control")
                .is_some_and(|v| !v.is_boolean() && !v.is_null())
            {
                anyhow::bail!("host_control must be true, false or null");
            }
            let previous_perm = c.perm;
            let previous_layout = c.extra.get("virtual_display_layout").cloned();
            if let Some(n) = data.get("name").and_then(Value::as_str) {
                butterpollo_core::pairing::validate_device_name(n)?;
                c.name = n.into();
            }
            if let Some(n) = data.get("perm").and_then(Value::as_u64) {
                c.perm = u32::try_from(n)? & 0x071f1f00;
            }
            if let Some(v) = data.get("enabled").and_then(Value::as_bool) {
                c.enabled = v;
            }
            for (k, v) in data
                .as_object()
                .ok_or_else(|| anyhow::anyhow!("client update must be an object"))?
            {
                if !matches!(
                    k.as_str(),
                    "uuid"
                        | "cert"
                        | "perm"
                        | "name"
                        | "enabled"
                        | "connected"
                        | "network_loss_routes"
                ) {
                    // null clears a per-device setting.
                    if v.is_null() {
                        c.extra.remove(k);
                    } else {
                        c.extra.insert(k.clone(), v.clone());
                    }
                }
            }
            // A layout the user picks is theirs, not one learned from a stream: the
            // editor says so when the field was touched (even if it ends on the same
            // value), and any other change of the layout through here is a user's too.
            // A device with no layout of its own has nothing to protect.
            let set_by_user = crate::display_session::SET_BY_USER;
            let touched = data.get(set_by_user).and_then(Value::as_bool) == Some(true);
            let layout = c.extra.get("virtual_display_layout");
            let changed = layout != previous_layout.as_ref();
            let has_layout = layout
                .and_then(Value::as_str)
                .is_some_and(|s| !s.is_empty() && s != "global");
            if has_layout && (touched || changed) {
                c.extra.insert(set_by_user.into(), Value::Bool(true));
                c.extra.remove(crate::display_session::LEARNED_OVER);
            } else if !has_layout {
                c.extra.remove(set_by_user);
                c.extra.remove(crate::display_session::LEARNED_OVER);
            }
            let revoked = data.get("enabled").and_then(Value::as_bool) == Some(false)
                || data
                    .get("perm")
                    .and_then(Value::as_u64)
                    .is_some_and(|p| previous_perm & !(p as u32) != 0);
            next.save(&h.paired_path)?;
            *state = next;
            drop(state);
            // Other changes apply from the device's next stream; disabling
            // it or removing permissions ends what it is doing now.
            if revoked {
                let canceled = h.sessions.lock().unwrap().request_stop(Some(text("uuid")));
                drop(canceled);
                crate::remote_display::disconnect(h, Some(text("uuid")));
            }
            // Turning a device off ends its claim on the host's Radeon settings. Removing a
            // permission does not: pending changes go back with the session.
            if data.get("enabled").and_then(Value::as_bool) == Some(false) {
                h.radeon.release_device(text("uuid"));
            }
            json!({"status":true,"disconnected":revoked})
        }
        ("GET", "/api/clients/pending") => {
            let pins = h.pins.lock().unwrap();
            json!({"status":true,"requests":pins
                .iter()
                .filter(|(_, p)| p.created.elapsed() < Duration::from_secs(300))
                .map(|(id, p)| json!({"uniqueid":id,"name":p.name,"age_seconds":p.created.elapsed().as_secs()}))
                .collect::<Vec<_>>()})
        }
        ("POST", "/api/otp") => {
            if !text("deviceName").is_empty() {
                butterpollo_core::pairing::validate_device_name(text("deviceName"))?;
            }
            let passphrase = text("passphrase");
            if passphrase.chars().count() < 4 {
                anyhow::bail!("passphrase must have at least four characters");
            }
            let pin = format!(
                "{:04}",
                rand::Rng::gen_range(&mut rand::thread_rng(), 0..10_000u16)
            );
            *h.otp.lock().unwrap() = Some(crate::state::OneTimePin {
                pin: pin.clone(),
                passphrase: passphrase.to_owned(),
                device_name: text("deviceName").to_owned(),
                created: Instant::now(),
                attempts: 0,
            });
            let config = h.config.read().unwrap().clone();
            json!({
                "status": true,
                "otp": pin,
                "ip": butterpollo_windows::net::lan_addresses().unwrap_or_default().first().cloned().unwrap_or_default(),
                "name": crate::network::host_name(&config),
                "message": "OTP created, effective within 3 minutes.",
            })
        }
        ("POST", "/api/pin") => {
            if !text("name").is_empty() {
                butterpollo_core::pairing::validate_device_name(text("name"))?;
            }
            let pin = text("pin");
            if pin.len() != 4 || !pin.bytes().all(|c| c.is_ascii_digit()) {
                anyhow::bail!("PIN must contain four digits");
            }
            let mut pins = h.pins.lock().unwrap();
            let key = if !text("uniqueid").is_empty() {
                text("uniqueid").to_owned()
            } else {
                if pins.len() != 1 {
                    anyhow::bail!("select one pending pairing request");
                }
                pins.keys()
                    .next()
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("no pending pairing request"))?
            };
            let p = pins
                .remove(&key)
                .ok_or_else(|| anyhow::anyhow!("pairing request no longer exists"))?;
            let name = if text("name").is_empty() {
                p.name
            } else {
                text("name").to_owned()
            };
            p.sender
                .send((pin.into(), name))
                .map_err(|_| anyhow::anyhow!("pairing request expired"))?;
            json!({"status":true})
        }
        _ => return Err(anyhow::anyhow!("unknown API endpoint")),
    })
}

#[cfg(test)]
mod tests {
    use super::handle;
    use crate::state::test_support::Fixture;
    use serde_json::json;

    /// What the console grants an ordinary device.
    const PERM: u32 = 0x071f1f00;

    #[test]
    fn turning_a_device_off_gives_up_its_radeon_settings_but_removing_a_permission_does_not() {
        let f = Fixture::new();
        let client = f.client(PERM);
        f.host.radeon.note_written(&client.uuid, &[]);
        // Removing a permission ends what the device is doing, not its claim on its settings:
        // they go back with the session.
        handle(
            &f.host,
            "POST",
            "/api/clients/update",
            &json!({"uuid": client.uuid, "perm": 0}),
        )
        .unwrap();
        assert!(f.host.radeon.holds(&client.uuid));
        handle(
            &f.host,
            "POST",
            "/api/clients/update",
            &json!({"uuid": client.uuid, "enabled": false}),
        )
        .unwrap();
        assert!(!f.host.radeon.holds(&client.uuid));
    }

    fn layout_settings(f: &Fixture) -> (Option<serde_json::Value>, bool, bool) {
        let paired = f.host.paired.read().unwrap();
        let extra = &paired.clients[0].extra;
        (
            extra.get("virtual_display_layout").cloned(),
            extra.contains_key(crate::display_session::SET_BY_USER),
            extra.contains_key(crate::display_session::LEARNED_OVER),
        )
    }
    fn update(f: &Fixture, data: serde_json::Value) {
        handle(&f.host, "POST", "/api/clients/update", &data).unwrap();
    }

    #[test]
    fn only_touching_the_layout_field_makes_a_learned_layout_the_users() {
        // Issue #40: a layout learned from a stream is not the user's until they
        // pick it, and a rename or another setting does not make it so.
        let f = Fixture::new();
        let client = f.client(PERM);
        let uuid = client.uuid;
        {
            let mut paired = f.host.paired.write().unwrap();
            let extra = &mut paired.clients[0].extra;
            extra.insert("virtual_display_layout".into(), "extended".into());
            extra.insert(
                crate::display_session::LEARNED_OVER.into(),
                "extended_primary".into(),
            );
        }
        update(
            &f,
            json!({"uuid": uuid, "name": "Renamed", "background_stream": true}),
        );
        assert_eq!(
            layout_settings(&f),
            (Some(json!("extended")), false, true),
            "a save that leaves the layout alone keeps it learned"
        );
        // The editor picked the value that was already there.
        update(
            &f,
            json!({"uuid": uuid, crate::display_session::SET_BY_USER: true}),
        );
        assert_eq!(layout_settings(&f), (Some(json!("extended")), true, false));
        // A rename does not take it back.
        update(&f, json!({"uuid": uuid, "name": "Again"}));
        assert_eq!(layout_settings(&f), (Some(json!("extended")), true, false));
        // Back to the host setting: nothing is left to protect or to yield.
        update(
            &f,
            json!({"uuid": uuid, "virtual_display_layout": null, crate::display_session::SET_BY_USER: null}),
        );
        assert_eq!(layout_settings(&f), (None, false, false));
    }

    #[test]
    fn a_layout_changed_through_the_console_api_is_the_users() {
        let f = Fixture::new();
        let client = f.client(PERM);
        update(
            &f,
            json!({"uuid": client.uuid, "virtual_display_layout": "exclusive"}),
        );
        assert_eq!(layout_settings(&f), (Some(json!("exclusive")), true, false));
    }

    #[test]
    fn unpairing_every_device_gives_up_all_radeon_settings() {
        let f = Fixture::new();
        let client = f.client(PERM);
        f.host.radeon.note_written(&client.uuid, &[]);
        f.host.radeon.note_written("another-device", &[]);
        handle(&f.host, "POST", "/api/clients/unpair-all", &json!({})).unwrap();
        assert!(!f.host.radeon.holds(&client.uuid));
        assert!(!f.host.radeon.holds("another-device"));
    }
}
