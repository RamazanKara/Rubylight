//! Paired-client management and pairing API handlers.

use crate::state::Shared;
use butterpollo_core::native_api::folders::FolderAccess;
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
            let agents = h.client_agents.lock().unwrap().clone();
            let clients: Vec<_> = h
                .paired
                .read()
                .unwrap()
                .clients
                .iter()
                .map(|c| {
                    let mut v = serde_json::to_value(c).unwrap();
                    v.as_object_mut().unwrap().remove("cert");
                    // The secret of a WebRTC device never leaves the host; the
                    // console gets the device ID, which marks the transport.
                    if let Some((client_id, _)) = c.webrtc() {
                        v["webrtc"] = json!({"clientId": client_id});
                    }
                    v["connected"] = json!(connected.contains(&c.uuid));
                    v["network_loss_routes"] = loss_routes
                        .get(&c.uuid)
                        .cloned()
                        .unwrap_or_else(|| json!([]));
                    let agent = agents.get(&c.uuid).map(String::as_str);
                    v["client_agent"] = json!(agent);
                    v["client_warning"] =
                        client_warning(butterpollo_core::client_warnings::BROKEN_CLIENTS, agent);
                    v
                })
                .collect();
            // Sleep PC is the PC's: the console shows why it is unavailable
            // instead of the device's toggle.
            json!({
                "status": true,
                "clients": clients,
                "named_certs": clients,
                "sleep_unavailable": h.power.unavailable(),
            })
        }
        ("POST", "/api/clients/unpair") => {
            let canceled = h.sessions.lock().unwrap().request_stop(Some(text("uuid")));
            drop(canceled);
            crate::remote_display::disconnect(h, Some(text("uuid")));
            h.unpair_with_network_history(Some(text("uuid")))?;
            h.webrtc.clients_changed();
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
            h.webrtc.clients_changed();
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
            // Host control, Sleep PC, USB forwarding and Join co-op are granted
            // only here, and only as a real boolean (null clears it). Anything
            // else is refused before any change.
            for key in ["host_control", "allow_sleep", "allow_usb", "coop_join"] {
                if data
                    .get(key)
                    .is_some_and(|v| !v.is_boolean() && !v.is_null())
                {
                    anyhow::bail!("{key} must be true, false or null");
                }
            }
            if let Some(grants) = data.get("shared_folders") {
                check_folder_grants(grants)?;
            }
            // A preset the host does not know is refused here, as it is for the
            // host's own setting, before anything is saved.
            if let Some(overrides) = data.get("config_overrides") {
                butterpollo_core::stream_preset::check_overrides(overrides)?;
            }
            let previous_perm = c.perm;
            let previous_usb = c.allows_usb();
            let previous_coop_join = c.allows_coop_join();
            let previous_folders = folder_grants(c);
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
                        // The WebRTC credentials are written only by pairing.
                        | "webrtc"
                        | "perm"
                        | "name"
                        | "enabled"
                        | "connected"
                        | "network_loss_routes"
                        | "client_agent"
                        | "client_warning"
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
                    .is_some_and(|p| previous_perm & !(p as u32) != 0)
                // USB forwarding, Join co-op and folder grants are permissions too:
                // losing one, or write going back to read, ends what the device is doing.
                || (previous_usb && !c.allows_usb())
                || (previous_coop_join && !c.allows_coop_join())
                || previous_folders
                    .iter()
                    .any(|(id, access)| match (access, c.folder_access(id)) {
                        (_, None) => true,
                        (FolderAccess::Write, Some(now)) => now != FolderAccess::Write,
                        _ => false,
                    });
            next.save(&h.paired_path)?;
            *state = next;
            drop(state);
            h.webrtc.clients_changed();
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
                .map(|(id, p)| json!({
                    "uniqueid": id,
                    "name": p.name,
                    "age_seconds": p.created.elapsed().as_secs(),
                    // A WebRTC request has no certificate.
                    "webrtc": p.certificate.is_empty(),
                }))
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
            let pin = one_time_pin(h, passphrase, text("deviceName"));
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
        ("POST", "/api/pairing/qr") => pairing_qr(
            h,
            text("address"),
            butterpollo_windows::net::lan_addresses()?,
        )?,
        _ => return Err(anyhow::anyhow!("unknown API endpoint")),
    })
}

/// Replaces the one-time PIN (valid 180 s, used once) and returns it.
fn one_time_pin(h: &Shared, passphrase: &str, device_name: &str) -> String {
    let pin = format!(
        "{:04}",
        rand::Rng::gen_range(&mut rand::thread_rng(), 0..10_000u16)
    );
    *h.otp.lock().unwrap() = Some(crate::state::OneTimePin {
        pin: pin.clone(),
        passphrase: passphrase.to_owned(),
        device_name: device_name.to_owned(),
        created: Instant::now(),
        misses: Default::default(),
    });
    pin
}

/// A pairing QR code per LAN address (`rubylight://pair?...`, see
/// `butterpollo_core::pairing_uri`), all for one new one-time PIN with a
/// passphrase the host picks. Refused while `qr_pairing` is off; `address`
/// puts that address first.
pub(crate) fn pairing_qr(
    h: &Shared,
    address: &str,
    mut addresses: Vec<String>,
) -> anyhow::Result<Value> {
    use sha2::Digest;
    let config = h.config.read().unwrap().clone();
    if !config.qr_pairing() {
        anyhow::bail!("pairing with a QR code is off: turn on Pair with a QR code in Settings");
    }
    if !address.is_empty() {
        let at = addresses
            .iter()
            .position(|known| known == address)
            .ok_or_else(|| anyhow::anyhow!("{address} is not an address of this PC"))?;
        let chosen = addresses.remove(at);
        addresses.insert(0, chosen);
    }
    if addresses.is_empty() {
        anyhow::bail!("this PC has no network address a device could reach");
    }
    let passphrase = hex::encode(butterpollo_core::crypto::random::<8>());
    let pin = one_time_pin(h, &passphrase, "");
    let mut invitation = butterpollo_core::pairing_uri::PairingUri {
        host: String::new(),
        port: config.port()?,
        otp: pin,
        passphrase,
        unique_id: h.paired.read().unwrap().unique_id.clone(),
        cert_sha256: sha2::Sha256::digest(&h.identity.der).into(),
        name: crate::network::host_name(&config),
    };
    let mut codes = Vec::new();
    for address in addresses {
        invitation.host = address.clone();
        let uri = invitation.to_uri();
        codes.push(json!({"address": address, "svg": qr_svg(&uri)?, "uri": uri}));
    }
    Ok(json!({
        "status": true,
        "codes": codes,
        "name": invitation.name,
        "expires_in_seconds": 180,
    }))
}

/// `text` as a QR code in SVG, black on white with the standard 4-module border.
pub(super) fn qr_svg(text: &str) -> anyhow::Result<String> {
    use qrcodegen::{QrCode, QrCodeEcc};
    let code = QrCode::encode_text(text, QrCodeEcc::Medium)
        .map_err(|_| anyhow::anyhow!("the pairing link is too long for a QR code"))?;
    let border = 4;
    let size = code.size() + 2 * border;
    let mut path = String::new();
    for y in 0..code.size() {
        for x in 0..code.size() {
            if code.get_module(x, y) {
                path.push_str(&format!("M{},{}h1v1h-1z", x + border, y + border));
            }
        }
    }
    Ok(format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {size} {size}\" shape-rendering=\"crispEdges\"><rect width=\"100%\" height=\"100%\" fill=\"#fff\"/><path d=\"{path}\" fill=\"#000\"/></svg>"
    ))
}

/// The folders this device holds a grant for, as `Client::folder_access` reads them.
fn folder_grants(c: &butterpollo_core::state::Client) -> Vec<(String, FolderAccess)> {
    c.extra
        .get("shared_folders")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|grants| grants.keys())
        .filter_map(|id| Some((id.clone(), c.folder_access(id)?)))
        .collect()
}

/// `shared_folders` as the update takes it: null, which clears the grants, or
/// an object from folder id to `"read"` or `"write"` (`Client::folder_access`).
fn check_folder_grants(grants: &Value) -> anyhow::Result<()> {
    let valid = grants.is_null()
        || grants.as_object().is_some_and(|grants| {
            grants.iter().all(|(id, access)| {
                butterpollo_core::config::valid_folder_id(id)
                    && matches!(access.as_str(), Some("read" | "write"))
            })
        });
    if !valid {
        anyhow::bail!("shared_folders must map folder ids to \"read\" or \"write\", or be null");
    }
    Ok(())
}

/// The console's warning for a client build on `list`; it never refuses one.
fn client_warning(
    list: &[butterpollo_core::client_warnings::BrokenClient],
    agent: Option<&str>,
) -> Value {
    agent
        .and_then(|agent| butterpollo_core::client_warnings::warning_in(list, agent))
        .map_or(
            Value::Null,
            |broken| json!({"problem": broken.problem, "link": broken.link}),
        )
}

#[cfg(test)]
mod tests {
    use super::handle;
    use crate::state::test_support::Fixture;
    use butterpollo_core::power_action::SleepStates;
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

    #[test]
    fn taking_away_usb_join_co_op_or_a_folder_grant_disconnects_the_device() {
        let f = Fixture::new();
        let client = f.client(PERM);
        let disconnects = |change: serde_json::Value| {
            let mut data = change;
            data["uuid"] = json!(client.uuid);
            let answer = handle(&f.host, "POST", "/api/clients/update", &data).unwrap();
            answer["disconnected"] == json!(true)
        };
        // Granting is not a revocation.
        assert!(!disconnects(json!({"allow_usb": true})));
        assert!(!disconnects(json!({"name": "Renamed"})));
        assert!(disconnects(json!({"allow_usb": false})));
        assert!(!disconnects(json!({"allow_usb": false})));
        assert!(!disconnects(json!({"allow_usb": true})));
        assert!(disconnects(json!({"allow_usb": null})));
        // Join co-op too: a co-op player loses its stream and cannot rejoin.
        assert!(!disconnects(json!({"coop_join": true})));
        assert!(disconnects(json!({"coop_join": false})));
        assert!(!disconnects(json!({"coop_join": null})));
        assert!(!disconnects(
            json!({"shared_folders": {"games": "write", "mods": "read"}})
        ));
        // Another grant, or the same ones again, take nothing away.
        assert!(!disconnects(
            json!({"shared_folders": {"games": "write", "mods": "read", "saves": "read"}})
        ));
        assert!(!disconnects(
            json!({"shared_folders": {"games": "write", "mods": "read", "saves": "read"}})
        ));
        assert!(!disconnects(
            json!({"shared_folders": {"games": "write", "mods": "write", "saves": "read"}})
        ));
        // Write back to read, a folder dropped, and all of them cleared.
        assert!(disconnects(
            json!({"shared_folders": {"games": "read", "mods": "write", "saves": "read"}})
        ));
        assert!(disconnects(
            json!({"shared_folders": {"games": "read", "mods": "write"}})
        ));
        assert!(disconnects(json!({"shared_folders": null})));
        assert!(!disconnects(json!({"shared_folders": null})));
    }

    #[test]
    fn join_co_op_is_granted_only_as_a_boolean() {
        let f = Fixture::new();
        let client = f.client(PERM);
        let update = |value: serde_json::Value| {
            handle(
                &f.host,
                "POST",
                "/api/clients/update",
                &json!({"uuid": client.uuid, "coop_join": value}),
            )
        };
        let allowed = || f.host.paired.read().unwrap().clients[0].allows_coop_join();
        let error = update(json!("yes")).unwrap_err();
        assert_eq!(error.to_string(), "coop_join must be true, false or null");
        assert!(!allowed());
        update(json!(true)).unwrap();
        assert!(allowed());
        update(json!(false)).unwrap();
        assert!(!allowed());
        update(json!(true)).unwrap();
        update(serde_json::Value::Null).unwrap();
        assert!(!allowed());
    }

    #[test]
    fn a_device_cannot_save_a_preset_the_host_does_not_know() {
        let f = Fixture::new();
        let client = f.client(PERM);
        let save = |overrides: serde_json::Value| {
            handle(
                &f.host,
                "POST",
                "/api/clients/update",
                &json!({"uuid": client.uuid, "config_overrides": overrides}),
            )
        };
        let stored = || {
            f.host.paired.read().unwrap().clients[0]
                .extra
                .get("config_overrides")
                .cloned()
        };
        save(json!({"stream_preset": "Lowest-Latency", "max_bitrate": 20000})).unwrap();
        let good = stored();
        for bad in [
            json!("fastest"),
            json!(3),
            json!(true),
            json!(["reference"]),
        ] {
            let error = save(json!({"stream_preset": bad, "max_bitrate": 1})).unwrap_err();
            assert!(error.to_string().contains("stream_preset"), "{error}");
            assert_eq!(stored(), good, "{bad}");
        }
        // Clearing it is fine.
        save(json!({"stream_preset": null})).unwrap();
        save(json!({"stream_preset": ""})).unwrap();
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
    fn the_device_list_says_why_sleep_pc_is_unavailable() {
        let f = Fixture::new();
        f.client(PERM);
        let reason = |states| {
            f.host
                .power
                .set_backend(std::sync::Arc::new(crate::power_action::FixedStates(
                    states,
                )));
            handle(&f.host, "GET", "/api/clients/list", &json!({})).unwrap()["sleep_unavailable"]
                .clone()
        };
        let desktop = SleepStates {
            s3: true,
            hibernate: true,
            ..Default::default()
        };
        assert_eq!(reason(desktop), json!(null));
        for hibernate in [true, false] {
            let laptop = SleepStates {
                modern_standby: true,
                hibernate,
                ..Default::default()
            };
            assert_eq!(reason(laptop), json!(laptop.unavailable().unwrap()));
        }
        assert_eq!(
            reason(SleepStates {
                modern_standby: true,
                hibernate: true,
                ..Default::default()
            }),
            json!("this PC only has Modern Standby, so Windows would hibernate it")
        );
    }

    #[test]
    fn the_device_list_names_the_client_and_warns_only_about_listed_builds() {
        let f = Fixture::new();
        let client = f.client(PERM);
        let listed = || handle(&f.host, "GET", "/api/clients/list", &json!({})).unwrap();
        assert_eq!(listed()["clients"][0]["client_agent"], json!(null));
        assert_eq!(listed()["clients"][0]["client_warning"], json!(null));
        f.host
            .client_agents
            .lock()
            .unwrap()
            .insert(client.uuid.clone(), "Nightjar/3.2.0".into());
        assert_eq!(listed()["clients"][0]["client_agent"], "Nightjar/3.2.0");
        // The shipped list names no build, so nothing is flagged.
        assert_eq!(listed()["clients"][0]["client_warning"], json!(null));
        let list = [butterpollo_core::client_warnings::BrokenClient {
            product: "Nightjar",
            from: (3, 2, 0),
            fixed: (3, 2, 1),
            problem: "freezes",
            link: "https://github.com/RamazanKara/Rubylight/issues/0",
        }];
        assert_eq!(
            super::client_warning(&list, Some("Nightjar/3.2.0")),
            json!({"problem": "freezes", "link": "https://github.com/RamazanKara/Rubylight/issues/0"})
        );
        assert_eq!(
            super::client_warning(&list, Some("Nightjar/3.2.1")),
            json!(null)
        );
        assert_eq!(super::client_warning(&list, None), json!(null));
        // What the host saw is not a setting the console can write.
        update(
            &f,
            json!({"uuid": client.uuid, "client_agent": "x", "client_warning": {"problem": "y"}}),
        );
        {
            let paired = f.host.paired.read().unwrap();
            assert!(!paired.clients[0].extra.contains_key("client_agent"));
            assert!(!paired.clients[0].extra.contains_key("client_warning"));
        }
        // An unpaired device's client is forgotten with it.
        handle(
            &f.host,
            "POST",
            "/api/clients/unpair",
            &json!({"uuid": client.uuid}),
        )
        .unwrap();
        assert!(f.host.client_agents.lock().unwrap().is_empty());
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

    #[test]
    fn the_pairing_qr_code_is_off_until_turned_on() {
        let f = Fixture::new();
        let error = super::pairing_qr(&f.host, "", vec!["192.168.1.5".into()]).unwrap_err();
        assert!(error.to_string().contains("off"), "{error}");
        assert!(f.host.otp.lock().unwrap().is_none());
    }

    #[test]
    fn the_pairing_qr_code_holds_a_one_time_pin_and_the_host_certificate() {
        use sha2::Digest;
        let f = Fixture::new();
        f.host
            .config
            .write()
            .unwrap()
            .values
            .insert("qr_pairing".into(), "true".into());
        let addresses = vec!["192.168.1.5".to_owned(), "10.0.0.2".to_owned()];
        let answer = super::pairing_qr(&f.host, "10.0.0.2", addresses.clone()).unwrap();
        let codes = answer["codes"].as_array().unwrap();
        // The chosen address comes first.
        assert_eq!(codes[0]["address"], "10.0.0.2");
        assert_eq!(codes[1]["address"], "192.168.1.5");
        assert_eq!(answer["expires_in_seconds"], 180);
        let (pin, passphrase, device_name) = {
            let otp = f.host.otp.lock().unwrap();
            let otp = otp.as_ref().unwrap();
            (
                otp.pin.clone(),
                otp.passphrase.clone(),
                otp.device_name.clone(),
            )
        };
        for code in codes {
            let uri = code["uri"].as_str().unwrap();
            let parsed = butterpollo_core::pairing_uri::PairingUri::parse(uri).unwrap();
            assert_eq!(parsed.host, code["address"].as_str().unwrap());
            assert_eq!(parsed.port, f.host.config.read().unwrap().port().unwrap());
            assert_eq!((&parsed.otp, &parsed.passphrase), (&pin, &passphrase));
            assert_eq!(parsed.otp.len(), 4);
            assert_eq!(parsed.passphrase.len(), 16);
            assert_eq!(
                parsed.cert_sha256,
                <[u8; 32]>::from(sha2::Sha256::digest(&f.host.identity.der))
            );
            assert_eq!(parsed.unique_id, f.host.paired.read().unwrap().unique_id);
            assert!(code["svg"].as_str().unwrap().starts_with("<svg"));
        }
        // The client names itself, and a new code replaces the PIN.
        assert_eq!(device_name, "");
        super::pairing_qr(&f.host, "", addresses).unwrap();
        let next = f
            .host
            .otp
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .passphrase
            .clone();
        assert_ne!(next, passphrase);

        let error = super::pairing_qr(&f.host, "172.16.0.1", vec!["10.0.0.2".into()]).unwrap_err();
        assert!(
            error.to_string().contains("not an address of this PC"),
            "{error}"
        );
        assert!(super::pairing_qr(&f.host, "", Vec::new()).is_err());
    }

    #[test]
    fn a_qr_code_renders_as_dark_squares_on_white() {
        let svg = super::qr_svg("rubylight://pair?v=1").unwrap();
        assert!(svg.starts_with("<svg"), "{svg}");
        assert!(
            svg.contains("fill=\"#fff\"") || svg.contains("fill=\"white\""),
            "{svg}"
        );
        assert!(svg.matches('M').count() > 50);
    }
}
