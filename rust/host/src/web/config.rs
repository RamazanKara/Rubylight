//! Host configuration and metadata API handlers.

use crate::state::Shared;
use butterpollo_core::state;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

pub(super) fn handle(h: &Shared, method: &str, path: &str, data: &Value) -> anyhow::Result<Value> {
    Ok(match (method, path) {
        ("GET", "/api/config") => {
            let c = if h.port_override.is_some() {
                butterpollo_core::config::Config::load(&h.config_path)?
            } else {
                h.config.read().unwrap().clone()
            };
            let mut v = c.json();
            // The Webhooks page manages them, and their secrets never come back.
            if let Some(values) = v.as_object_mut() {
                values.remove("webhooks");
            }
            v["status"] = json!(true);
            v["platform"] = json!("windows");
            v["version"] = json!(env!("CARGO_PKG_VERSION"));
            v
        }
        ("POST" | "PATCH", "/api/config") => {
            let mut object = data
                .as_object()
                .ok_or_else(|| anyhow::anyhow!("configuration must be an object"))?
                .clone();
            for key in ["status", "platform", "version"] {
                object.remove(key);
            }
            if object.contains_key("webhooks") {
                anyhow::bail!(
                    "webhooks are saved on the Webhooks page (/api/webhooks), which keeps their secrets"
                );
            }
            if let Some(folder) = object.remove("nucleus_path") {
                choose_nucleus_folder(h, &folder, &object)?;
            }
            let _save = h.config_save.lock().unwrap();
            let mut next = if h.port_override.is_some() {
                butterpollo_core::config::Config::load(&h.config_path)?
            } else {
                h.config.read().unwrap().clone()
            };
            next.update(&object)?;
            next.check_saved(object.keys())?;
            state::atomic_write(&h.config_path, next.text().as_bytes())?;
            let warning =
                butterpollo_windows::vulkan::reconcile(next.boolean("vulkan_hdr_layer", true))
                    .err()
                    .map(|e| e.to_string());
            crate::logging::apply(next.log_level());
            if let Some(port) = h.port_override {
                next.values.insert("port".into(), port.to_string());
            }
            let before = std::mem::replace(&mut *h.config.write().unwrap(), next);
            h.config_version
                .fetch_add(1, std::sync::atomic::Ordering::Release);
            ended_usb(h);
            h.packed_settings_saved(&before);
            h.metadata.lock().unwrap().take();
            // The launchers to read changed: the last scan answers for the old ones.
            if object.keys().any(|key| key.starts_with("library_")) {
                crate::library::invalidate(h);
            }
            json!({"status":true,"restart_required":restart_required(object.keys()),"warning":warning})
        }
        ("GET", "/api/configLocale") => {
            json!({"status":true,"locale":h.config.read().unwrap().get("locale","en")})
        }
        ("GET", "/api/meta" | "/api/metadata") => metadata(
            h,
            butterpollo_windows::capture::gpus,
            butterpollo_windows::capture::displays,
        )?,
        ("POST", "/api/warnings/dismiss") => {
            let code = data.get("code").and_then(Value::as_str).unwrap_or_default();
            crate::host_warnings::dismiss(&h.directory, &h.warnings, code)?;
            h.metadata.lock().unwrap().take();
            json!({"status":true})
        }
        _ => return Err(anyhow::anyhow!("unknown API endpoint")),
    })
}

/// A save's `nucleus_path` (`folder`), kept as the console's Choose folder
/// keeps it ([`super::coop::choose`]): checked first, refused with the
/// reason and status the panels give (409 while a session runs), and what
/// sessions left in the folder before it put back. The save's other
/// settings (`rest`) are checked first, so a refused one keeps no folder.
fn choose_nucleus_folder(
    h: &Shared,
    folder: &Value,
    rest: &serde_json::Map<String, Value>,
) -> anyhow::Result<()> {
    let folder = match folder {
        Value::String(text) => text.trim(),
        Value::Null => "",
        _ => anyhow::bail!("nucleus_path must be the folder Nucleus Co-op was unpacked into"),
    };
    let current = crate::nucleus::configured(&h.config.read().unwrap());
    if folder == current {
        return Ok(());
    }
    let mut trial = h.config.read().unwrap().clone();
    trial.update(rest)?;
    trial.check_saved(rest.keys())?;
    super::coop::choose(h, folder)?;
    Ok(())
}

/// Settings a save applies to the running host. Every other key still answers
/// that it needs a restart. HEVC and AV1 support do: the startup probe alone
/// decides which of those codecs clients are offered.
const LIVE_KEYS: &[&str] = &["experimental_packed_444"];

/// Whether a save of `keys` takes a restart to apply in full, for the
/// console's "Some changes apply after the host restarts".
fn restart_required<'a>(mut keys: impl Iterator<Item = &'a String>) -> bool {
    keys.any(|key| !LIVE_KEYS.contains(&key.as_str()))
}

/// Saves one setting that a page manages itself: `next` gets the stored text
/// (None when unset) and returns the text to save, empty to unset it, or an
/// error that saves nothing. Serialized with the console's other saves.
pub(super) fn save_setting(
    h: &Shared,
    key: &str,
    next: impl FnOnce(Option<&str>) -> anyhow::Result<String>,
) -> anyhow::Result<()> {
    let _save = h.config_save.lock().unwrap();
    let mut config = if h.port_override.is_some() {
        butterpollo_core::config::Config::load(&h.config_path)?
    } else {
        h.config.read().unwrap().clone()
    };
    let value = next(config.values.get(key).map(String::as_str))?;
    // The configuration file has no empty value: the key goes instead.
    if value.trim().is_empty() {
        config.values.remove(key);
    } else {
        config.values.insert(key.to_owned(), value);
        config.check_saved([&key.to_owned()])?;
    }
    state::atomic_write(&h.config_path, config.text().as_bytes())?;
    if let Some(port) = h.port_override {
        config.values.insert("port".into(), port.to_string());
    }
    let before = std::mem::replace(&mut *h.config.write().unwrap(), config);
    h.config_version
        .fetch_add(1, std::sync::atomic::Ordering::Release);
    ended_usb(h);
    h.packed_settings_saved(&before);
    Ok(())
}

/// USB forwarding is a permission: turning it off detaches every device now,
/// as taking USB from one device does, not when each stream ends.
fn ended_usb(h: &Shared) {
    if !h.config.read().unwrap().usb_forwarding() {
        h.usb.disable();
    }
}

fn metadata(
    shared: &Shared,
    gpu_inventory: impl FnOnce() -> anyhow::Result<Vec<butterpollo_windows::capture::Gpu>>,
    display_inventory: impl FnOnce() -> anyhow::Result<Vec<butterpollo_windows::capture::Display>>,
) -> anyhow::Result<Value> {
    let mut cached = shared.metadata.lock().unwrap();
    if let Some((checked, value)) = cached.as_ref()
        && checked.elapsed() < Duration::from_secs(5)
    {
        return Ok(value.clone());
    }
    let _com = butterpollo_windows::capture::ComGuard::new()?;
    let codecs = shared.codecs.load(std::sync::atomic::Ordering::Acquire);
    let probing = shared
        .probing_codecs
        .load(std::sync::atomic::Ordering::Acquire);
    let virtual_display = butterpollo_windows::display::virtual_display_status();
    let capable = virtual_display["capable"].as_bool().unwrap_or(false);
    let audio = butterpollo_windows::audio_route::endpoints();
    let gpus = gpu_inventory();
    let displays = display_inventory();
    let config = shared.config.read().unwrap().clone();
    let port = config.ports()?.http;
    let address = |host: String| {
        if port == 47989 {
            host
        } else {
            format!("{host}:{port}")
        }
    };
    let addresses = butterpollo_windows::net::lan_addresses()
        .unwrap_or_default()
        .into_iter()
        .map(address)
        .collect::<Vec<_>>();
    let pc_address = addresses
        .first()
        .cloned()
        .unwrap_or_else(|| address(std::env::var("COMPUTERNAME").unwrap_or_default()));
    let enhancements = crate::nvidia_enhance::status(shared);
    let value = json!({"status":true,"platform":"windows","version":env!("CARGO_PKG_VERSION"),"branch":"codex/butterpollo-rust","host_name":crate::network::host_name(&config),"pc_address":pc_address,"pc_addresses":addresses,"paired_devices":shared.paired.read().unwrap().clients.len(),"enable_pairing":config.boolean("enable_pairing",true),"qr_pairing":config.qr_pairing(),"warnings":crate::host_warnings::visible(&shared.directory,&shared.warnings),"encoder_status":{"state":if probing {"checking"} else if codecs == 0 {"failed"} else {"ready"},"h264":codecs&1!=0,"hevc":codecs&0x100!=0,"av1":codecs&0x10000!=0,"pyrowave":codecs&0x800000!=0,"yuv444":butterpollo_core::codec_support::native_444_labels(codecs)},"capture_status":{"configured_backend":config.get("capture","auto"),"virtual_display_configured":config.virtual_display_mode(butterpollo_windows::display::virtual_display_os())!="disabled","displays":displays.as_ref().ok(),"error":displays.as_ref().err().map(|e|e.to_string())},"virtual_display":virtual_display,"gpus":gpus.as_ref().ok(),"gpu_error":gpus.as_ref().err().map(|e|e.to_string()),"audio_sinks":audio.as_ref().ok(),"audio_error":audio.as_ref().err().map(|e|e.to_string()),"audio_enabled":config.boolean("stream_audio",true),"features":{"rust_host":true,"hdr":true,"pyrowave":codecs&0x800000!=0,"virtual_display":capable,"rtx_hdr":enhancements["rtx_hdr"]==true,"rtx_video":enhancements["rtx_video"]==true},"nvidia_enhancements":enhancements,"credentials_exists":shared.credentials.read().unwrap().is_some(),"webrtc":shared.webrtc.status_json(&config)});
    *cached = Some((Instant::now(), value.clone()));
    Ok(value)
}

#[cfg(test)]
mod tests;
