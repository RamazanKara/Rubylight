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
                // A device streaming only its second screen is connected too.
                .map(|s| butterpollo_core::state::device_uuid(&s.launch.client.uuid).to_owned())
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
            crate::nvhttp::end_device(h, text("uuid"));
            h.unpair_with_network_history(Some(text("uuid")))?;
            h.webrtc.clients_changed();
            // The device is gone; its session may linger for the grace period, its Radeon
            // settings and game file keys do not, its second screen's neither.
            crate::nvhttp::release_device(h, text("uuid"));
            json!({"status":true})
        }
        ("POST", "/api/clients/unpair-all") => {
            let canceled = h.sessions.lock().unwrap().request_stop(None);
            drop(canceled);
            crate::remote_display::disconnect(h, None);
            h.unpair_with_network_history(None)?;
            h.webrtc.clients_changed();
            h.radeon.release_all();
            h.game_files.release_all(&h.directory);
            json!({"status":true})
        }
        ("POST", "/api/clients/disconnect") => {
            crate::nvhttp::end_device(h, text("uuid"));
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
            // Host control, Approve new devices, Sleep PC, Restart and shut down
            // PC, USB forwarding and Join co-op are granted only here, and only as
            // a real boolean (null clears it). Anything else is refused before any
            // change.
            for key in [
                "host_control",
                butterpollo_core::state::PAIRING_APPROVE_KEY,
                "allow_sleep",
                "allow_restart_shutdown",
                "allow_usb",
                "coop_join",
            ] {
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
            // Connect and disconnect commands the host could not read would be
            // skipped at every stream: refuse them here instead.
            butterpollo_core::connection_commands::check_update(data)
                .map_err(anyhow::Error::msg)?;
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
                // The editor sends the name back with any change: a device paired
                // before a character was refused can still have its access changed.
                if n != c.name {
                    butterpollo_core::pairing::validate_device_name(n)?;
                }
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
                        // Forgetting what the device kept forgets its second screen's too.
                        if butterpollo_core::state::PER_SCREEN_SETTINGS.contains(&k.as_str()) {
                            c.extra.remove(&format!(
                                "{k}{}",
                                butterpollo_core::state::SECOND_SCREEN_SUFFIX
                            ));
                        }
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
                crate::nvhttp::end_device(h, text("uuid"));
            }
            // Turning a device off ends its claim on the host's Radeon settings and game file
            // keys. Removing a permission does not: pending changes go back with the session.
            if data.get("enabled").and_then(Value::as_bool) == Some(false) {
                crate::nvhttp::release_device(h, text("uuid"));
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
                    // The name is the device's to choose; the address is not.
                    "address": p.address.to_canonical().to_string(),
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
            let permission = butterpollo_core::state::pairing_access(text("access"))?;
            let pin = typed_one_time_pin(h, passphrase, text("deviceName"), permission);
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
            let permission = butterpollo_core::state::pairing_access(text("access"))?;
            // A WebRTC device keeps its own default.
            if permission.is_some() && pins.get(&key).is_some_and(|p| p.certificate.is_empty()) {
                anyhow::bail!("access applies only to Moonlight pairing requests");
            }
            let p = pins
                .remove(&key)
                .ok_or_else(|| anyhow::anyhow!("pairing request no longer exists"))?;
            let name = if text("name").is_empty() {
                p.name
            } else {
                text("name").to_owned()
            };
            p.sender
                .send((pin.into(), name, permission, None))
                .map_err(|_| anyhow::anyhow!("pairing request expired"))?;
            json!({"status":true})
        }
        ("POST", "/api/pairing/qr") => pairing_qr(
            h,
            text("address"),
            butterpollo_core::state::pairing_access(text("access"))?,
            butterpollo_windows::net::lan_addresses()?,
        )?,
        _ => return Err(anyhow::anyhow!("unknown API endpoint")),
    })
}

fn new_one_time_pin(
    passphrase: &str,
    device_name: &str,
    permission: Option<u32>,
) -> crate::state::OneTimePin {
    crate::state::OneTimePin {
        pin: format!(
            "{:04}",
            rand::Rng::gen_range(&mut rand::thread_rng(), 0..10_000u16)
        ),
        passphrase: passphrase.to_owned(),
        device_name: device_name.to_owned(),
        created: Instant::now(),
        misses: Default::default(),
        permission,
    }
}

/// Replaces the typed (Artemis) one-time PIN, valid 180 s and used once, and
/// returns it. The QR code's PIN stays.
fn typed_one_time_pin(
    h: &Shared,
    passphrase: &str,
    device_name: &str,
    permission: Option<u32>,
) -> String {
    let otp = new_one_time_pin(passphrase, device_name, permission);
    let pin = otp.pin.clone();
    h.otp.lock().unwrap().typed = Some(otp);
    pin
}

/// A QR code's PIN is shown again while it has this long left, so two
/// console tabs show the same code instead of replacing each other's.
const QR_REUSE_LEFT: Duration = Duration::from_secs(60);

/// A pairing QR code per LAN address (`rubylight://pair?...`, see
/// `butterpollo_core::pairing_uri`), all for one one-time PIN with a
/// passphrase the host picks: the live one while it has [`QR_REUSE_LEFT`],
/// else a new one. `permission` is the access the device gets, also for a
/// code shown again. Refused while `qr_pairing` is off; `address` puts that
/// address first.
pub(crate) fn pairing_qr(
    h: &Shared,
    address: &str,
    permission: Option<u32>,
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
    let (pin, passphrase, left) = {
        let mut otp = h.otp.lock().unwrap();
        let reuse = otp.qr.as_ref().is_some_and(|qr| {
            qr.created.elapsed() + QR_REUSE_LEFT <= crate::state::ONE_TIME_PIN_LIFETIME
        });
        if !reuse {
            let passphrase = hex::encode(butterpollo_core::crypto::random::<8>());
            otp.qr = Some(new_one_time_pin(&passphrase, "", permission));
        }
        let qr = otp.qr.as_mut().unwrap();
        qr.permission = permission;
        let left = crate::state::ONE_TIME_PIN_LIFETIME.saturating_sub(qr.created.elapsed());
        (qr.pin.clone(), qr.passphrase.clone(), left)
    };
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
        "expires_in_seconds": left.as_millis().div_ceil(1000),
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
mod tests;
