//! Host configuration and metadata API handlers.

use butterpollo_session::{config_ext::ConfigExt, state, steamgriddb as sgdb};
use butterpollo_stream::state::Shared;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

pub(super) fn handle(h: &Shared, method: &str, path: &str, data: &Value) -> anyhow::Result<Value> {
    Ok(match (method, path) {
        ("GET", "/api/config") => {
            let c = if h.port_override.is_some() {
                butterpollo_config::config::Config::load(&h.config_path)?
            } else {
                h.config.read().unwrap().clone()
            };
            let mut v = c.json();
            // The Webhooks page manages them, and their secrets never come back.
            if let Some(values) = v.as_object_mut() {
                values.remove("webhooks");
                // The SteamGridDB key never comes back either: a saved one shows as the mask.
                values.remove(sgdb::KEY_SETTING);
                if butterpollo_stream::steamgriddb::has_key(h) {
                    values.insert(sgdb::KEY_SETTING.into(), json!(sgdb::MASK));
                }
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
            // The SteamGridDB key is kept in the administrators-only credentials folder, never in
            // sunshine.conf, which every local user can read. The mask GET showed keeps it.
            let steamgriddb_key = match object.remove(sgdb::KEY_SETTING) {
                None => None,
                Some(Value::String(text)) if text.trim() == sgdb::MASK => None,
                Some(Value::Null) => Some(String::new()),
                Some(Value::String(text)) => {
                    sgdb::check_key(&text).map_err(|message| anyhow::anyhow!(message))?;
                    Some(text.trim().to_owned())
                }
                Some(_) => anyhow::bail!("{} must be text", sgdb::KEY_SETTING),
            };
            if steamgriddb_key.is_some() {
                // A copy written into sunshine.conf by hand goes.
                object.insert(sgdb::KEY_SETTING.into(), Value::Null);
            }
            if object.contains_key("webhooks") {
                anyhow::bail!(
                    "webhooks are saved on the Webhooks page (/api/webhooks), which keeps their secrets"
                );
            }
            let nucleus_folder = match object.remove("nucleus_path") {
                Some(Value::String(text)) => {
                    let trimmed = text.trim().to_owned();
                    super::coop::validate_choice(h, &trimmed)?;
                    let current =
                        butterpollo_stream::nucleus::configured(&h.config.read().unwrap());
                    if trimmed == current {
                        None
                    } else {
                        Some(trimmed)
                    }
                }
                Some(Value::Null) => {
                    let current =
                        butterpollo_stream::nucleus::configured(&h.config.read().unwrap());
                    (!current.is_empty()).then(String::new)
                }
                Some(_) => {
                    anyhow::bail!(
                        "nucleus_path must be the folder the co-op helper was unpacked into"
                    )
                }
                None => None,
            };
            if let Some(folder) = &nucleus_folder {
                object.insert("nucleus_path".into(), Value::String(folder.clone()));
            }
            let _save = h.config_save.lock().unwrap();
            let mut next = if h.port_override.is_some() {
                butterpollo_config::config::Config::load(&h.config_path)?
            } else {
                h.config.read().unwrap().clone()
            };
            next.update(&object)?;
            next.check_saved(object.keys())?;
            // A key typed into sunshine.conf by hand moves out at any save.
            // A failed move keeps the key there and still saves the other settings.
            if let Err(error) = butterpollo_stream::steamgriddb::take_conf_key(h, &mut next) {
                tracing::warn!(
                    error = %format!("{error:#}"),
                    "the SteamGridDB API key could not be moved out of sunshine.conf"
                );
            }
            // Apply the Nucleus folder change together with (or only if) writing
            // the config succeeds, never mutating the folder before save.
            if let Some(folder) = &nucleus_folder {
                let path = &h.config_path;
                let text = next.text();
                super::coop::choose_with(h, folder, || {
                    state::atomic_write(path, text.as_bytes()).map_err(|error| {
                        butterpollo_stream::nucleus::Refusal {
                            status: 500,
                            message: format!("Saving configuration failed: {error:#}"),
                        }
                    })
                })?;
            } else {
                state::atomic_write(&h.config_path, next.text().as_bytes())?;
            }
            let mut warnings = Vec::new();
            if let Some(key) = &steamgriddb_key {
                match butterpollo_stream::steamgriddb::store_key(h, Some(key)) {
                    Ok(()) => butterpollo_stream::steamgriddb::key_changed(h),
                    Err(error) if key.is_empty() => warnings.push(format!(
                        "The settings were saved, but the SteamGridDB API key could not be removed ({error:#}); it is still in use."
                    )),
                    Err(error) => warnings.push(format!(
                        "The settings were saved, but the SteamGridDB API key was not stored ({error:#}). Enter it again."
                    )),
                }
            }
            if let Err(error) =
                butterpollo_win_graphics::vulkan::reconcile(next.boolean("vulkan_hdr_layer", true))
            {
                warnings.push(error.to_string());
            }
            let warning = (!warnings.is_empty()).then(|| warnings.join(" "));
            butterpollo_stream::logging::apply(next.log_level());
            if let Some(port) = h.port_override {
                next.values.insert("port".into(), port.to_string());
            }
            let before = std::mem::replace(&mut *h.config.write().unwrap(), next);
            h.config_version
                .fetch_add(1, std::sync::atomic::Ordering::Release);
            ended_usb(h);
            h.packed_settings_saved(&before);
            // Stream DVR turned on during a stream arms it now.
            butterpollo_stream::stream::dvr::arm_owner(h);
            h.metadata.lock().unwrap().take();
            // The idle loop follows some settings by their own deadlines (the
            // update interval, the restore hotkey): a save wakes it to look
            // at once.
            h.wake_maintenance();
            // The launchers to read changed: the last scan answers for the old ones.
            if object.keys().any(|key| key.starts_with("library_")) {
                butterpollo_stream::library::invalidate(h);
            }
            json!({"status":true,"restart_required":restart_required(object.keys()),"warning":warning})
        }
        ("GET", "/api/configLocale") => {
            json!({"status":true,"locale":h.config.read().unwrap().get("locale","en")})
        }
        ("GET", "/api/meta" | "/api/metadata") => metadata(
            h,
            butterpollo_win_graphics::capture::gpus,
            butterpollo_win_graphics::capture::displays,
        )?,
        ("POST", "/api/warnings/dismiss") => {
            let code = data.get("code").and_then(Value::as_str).unwrap_or_default();
            butterpollo_stream::host_warnings::dismiss(&h.directory, &h.warnings, code)?;
            h.metadata.lock().unwrap().take();
            json!({"status":true})
        }
        _ => return Err(anyhow::anyhow!("unknown API endpoint")),
    })
}

/// The settings the host reads only when it starts, by their current names.
/// Every other setting is read where it is used, so a save reaches the next
/// stream, request or check. The console marks the same settings "Applies
/// after a restart" (`SettingsSchema.test.mjs` compares the two lists). A
/// setting also read at startup for part of what it does stays here.
const RESTART_KEYS: &[&str] = &[
    // The listeners: their ports and the address they bind to.
    "port",
    "address_family",
    "bind_address",
    // Local network discovery advertises the name it started with.
    "enable_discovery",
    "sunshine_name",
    // Port mapping maps the ports chosen when it started.
    "upnp",
    "stream_mic",
    "origin_web_ui_allowed",
    "internet_play",
    "internet_play_stun_servers",
    "webrtc_server",
    "webrtc_port",
    "webrtc_media_port",
    "enable_dsu_server",
    "dsu_server_port",
    "dsu_server_lan",
    "system_tray",
    "hide_tray_controls",
    "log_path",
    // The startup probe alone decides which codecs clients are offered, on the
    // GPU these choose; the driver settings panel follows that GPU too.
    "hevc_mode",
    "av1_mode",
    "pyrowave",
    "encoder",
    "adapter_name",
    "adapter_pnp_id",
    "output_name",
    // NVIDIA's power profile applies before the host opens a GPU device.
    "nvenc_latency_over_power",
    // The profile's files, opened once.
    "file_state",
    "credentials_file",
    "file_apps",
    "vibeshine_file_state",
    "cert",
    "pkey",
];

/// Whether a save of `keys` takes a restart to apply in full, for the
/// console's "Some changes apply after the host restarts".
fn restart_required<'a>(mut keys: impl Iterator<Item = &'a String>) -> bool {
    keys.any(|key| RESTART_KEYS.contains(&butterpollo_config::config::current_name(key)))
}

/// Saves one setting that a page manages itself: `next` gets the stored text
/// (None when unset) and returns the text to save, empty to unset it, or an
/// error that saves nothing. Serialized with the console's other saves.
pub(super) fn save_setting(
    h: &Shared,
    key: &str,
    next: impl FnOnce(Option<&str>) -> anyhow::Result<String>,
) -> anyhow::Result<()> {
    save_setting_tx(h, key, |s| Ok((next(s)?, ())), |()| Ok(()), |_| (), |_| ())
}

/// Like [`save_setting`], holding `config_save` across paired sidecar file preparation,
/// rollback if writing `sunshine.conf` fails, and post-commit cleanup.
pub(super) fn save_setting_tx<P, T>(
    h: &Shared,
    key: &str,
    next: impl FnOnce(Option<&str>) -> anyhow::Result<(String, P)>,
    prepare: impl FnOnce(P) -> anyhow::Result<T>,
    rollback: impl FnOnce(T),
    after_commit: impl FnOnce(T),
) -> anyhow::Result<()> {
    let _save = h.config_save.lock().unwrap();
    let mut config = if h.port_override.is_some() {
        butterpollo_config::config::Config::load(&h.config_path)?
    } else {
        h.config.read().unwrap().clone()
    };
    let (value, prepared_arg) = next(config.values.get(key).map(String::as_str))?;
    // The configuration file has no empty value: the key goes instead.
    if value.trim().is_empty() {
        config.values.remove(key);
    } else {
        config.values.insert(key.to_owned(), value);
        config.check_saved([&key.to_owned()])?;
    }
    let staged = prepare(prepared_arg)?;
    if let Err(error) = state::atomic_write(&h.config_path, config.text().as_bytes()) {
        rollback(staged);
        return Err(error);
    }
    if let Some(port) = h.port_override {
        config.values.insert("port".into(), port.to_string());
    }
    let before = std::mem::replace(&mut *h.config.write().unwrap(), config);
    h.config_version
        .fetch_add(1, std::sync::atomic::Ordering::Release);
    ended_usb(h);
    h.packed_settings_saved(&before);
    butterpollo_stream::stream::dvr::arm_owner(h);
    // The idle loop follows some settings by their own deadlines (the update
    // interval, the restore hotkey): a save wakes it to look at once.
    h.wake_maintenance();
    after_commit(staged);
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
    gpu_inventory: impl FnOnce() -> anyhow::Result<Vec<butterpollo_win_graphics::capture::Gpu>>,
    display_inventory: impl FnOnce() -> anyhow::Result<Vec<butterpollo_win_graphics::capture::Display>>,
) -> anyhow::Result<Value> {
    let mut cached = shared.metadata.lock().unwrap();
    if let Some((checked, value)) = cached.as_ref()
        && checked.elapsed() < Duration::from_secs(5)
    {
        return Ok(value.clone());
    }
    let _com = butterpollo_win_graphics::capture::ComGuard::new()?;
    let codecs = shared.codecs.load(std::sync::atomic::Ordering::Acquire);
    let probing = shared
        .probing_codecs
        .load(std::sync::atomic::Ordering::Acquire);
    let virtual_display = butterpollo_win_display::display::virtual_display_status();
    let capable = virtual_display["capable"].as_bool().unwrap_or(false);
    let audio = butterpollo_win_io::audio_route::endpoints();
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
    let addresses = butterpollo_win_io::net::lan_addresses()
        .unwrap_or_default()
        .into_iter()
        .map(address)
        .collect::<Vec<_>>();
    let pc_address = addresses
        .first()
        .cloned()
        .unwrap_or_else(|| address(std::env::var("COMPUTERNAME").unwrap_or_default()));
    let webrtc_status = shared.webrtc_service().map_or_else(
        || butterpollo_stream::webrtc_bridge::default_status_json(&config),
        |s| s.status_json(&config),
    );
    let enhancements = butterpollo_stream::nvidia_enhance::status(shared);
    butterpollo_stream::host_warnings::automatic_gpu(&shared.warnings, &config);
    let value = json!({"status":true,"platform":"windows","version":env!("CARGO_PKG_VERSION"),"branch":"codex/butterpollo-rust","host_name":butterpollo_stream::network::host_name(&config),"pc_address":pc_address,"pc_addresses":addresses,"paired_devices":shared.paired.read().unwrap().clients.len(),"enable_pairing":config.boolean("enable_pairing",true),"qr_pairing":config.qr_pairing(),"warnings":butterpollo_stream::host_warnings::visible(&shared.directory,&shared.warnings),"encoder_status":{"state":if probing {"checking"} else if codecs == 0 {"failed"} else {"ready"},"h264":codecs&1!=0,"hevc":codecs&0x100!=0,"av1":codecs&0x10000!=0,"pyrowave":codecs&0x800000!=0,"yuv444":butterpollo_media_policy::codec_support::native_444_labels(codecs)},"capture_status":{"configured_backend":config.get("capture","auto"),"virtual_display_configured":config.virtual_display_mode(butterpollo_win_display::display::virtual_display_os())!="disabled","displays":displays.as_ref().ok(),"error":displays.as_ref().err().map(|e|e.to_string())},"virtual_display":virtual_display,"gpus":gpus.as_ref().ok(),"gpu_error":gpus.as_ref().err().map(|e|e.to_string()),"audio_sinks":audio.as_ref().ok(),"audio_error":audio.as_ref().err().map(|e|e.to_string()),"audio_enabled":config.boolean("stream_audio",true),"features":{"rust_host":true,"hdr":true,"pyrowave":codecs&0x800000!=0,"virtual_display":capable,"virtual_display_os":butterpollo_win_display::display::virtual_display_os(),"rtx_hdr":enhancements["rtx_hdr"]==true,"rtx_video":enhancements["rtx_video"]==true},"nvidia_enhancements":enhancements,"credentials_exists":shared.credentials.read().unwrap().is_some(),"webrtc":webrtc_status});
    *cached = Some((Instant::now(), value.clone()));
    Ok(value)
}

#[cfg(test)]
mod tests;
