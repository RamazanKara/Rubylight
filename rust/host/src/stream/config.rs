//! A launch's effective configuration: host, device, app and stream-preset layers.
use super::*;

/// Whether `overrides` set the peak brightness, under its name before or
/// after it moved out of RTX HDR.
pub(super) fn sets_peak(overrides: &serde_json::Map<String, serde_json::Value>) -> bool {
    ["hdr_peak_brightness", "rtx_hdr_peak_brightness"]
        .iter()
        .any(|key| {
            overrides
                .get(*key)
                .is_some_and(|value| !value.is_null() && value.as_str() != Some(""))
        })
}

/// The host configuration with `client`'s own settings, for answers outside
/// a stream (the native capabilities): no app has its say yet.
pub(crate) fn device_config(
    h: &Shared,
    client: &butterpollo_core::state::Client,
) -> Result<Config> {
    let mut config = h.config.read().unwrap().clone();
    device_layer(&mut config, client)?;
    butterpollo_core::stream_preset::fill(&mut config);
    Ok(config)
}

/// Applies the device's overrides. Returns whether they chose the peak
/// brightness.
fn device_layer(config: &mut Config, client: &butterpollo_core::state::Client) -> Result<bool> {
    let mut peak_chosen = false;
    if let Some(overrides) = client
        .extra
        .get("config_overrides")
        .and_then(serde_json::Value::as_object)
    {
        peak_chosen |= sets_peak(overrides);
        apply_overrides(config, overrides)?;
    }
    if let Some(value) = client
        .extra
        .get("prefer_10bit_sdr")
        .filter(|v| !v.is_null() && v.as_str() != Some(""))
    {
        config.values.insert(
            "prefer_sdr_10bit".into(),
            value
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| value.to_string()),
        );
    }
    Ok(peak_chosen)
}

/// The app `launch` streams as its device streams it: the device's block in the app's
/// `device-overrides` laid over the app's own settings (app × device, then app).
pub(crate) fn launch_app(h: &Shared, launch: &Launch) -> Option<butterpollo_core::state::App> {
    h.apps
        .read()
        .unwrap()
        .iter()
        .find(|app| app.id() == launch.app_id || app.aliases.contains(&launch.app_id))
        .map(|app| {
            butterpollo_core::device_overrides::for_device(
                app,
                butterpollo_core::state::device_uuid(&launch.client.uuid),
            )
        })
}

/// The stream's settings: host, then device, then app, then the app's block for the device.
pub(crate) fn effective_config(h: &Shared, launch: &Launch) -> Result<Config> {
    let mut config = h.config.read().unwrap().clone();
    let mut peak_chosen = device_layer(&mut config, &launch.client)?;
    let app = launch_app(h, launch);
    if let Some(app) = &app {
        for (source, target) in [
            ("gamepad", "gamepad"),
            ("dd-configuration-option", "dd_configuration_option"),
            ("prefer-10bit-sdr", "prefer_sdr_10bit"),
            // Saved by Vibepollo and earlier Rubylight apps for RTX HDR;
            // the peak is still what a virtual display reports.
            ("rtx-hdr-peak-brightness", "hdr_peak_brightness"),
            // The app's window in place of a display (app_window.rs).
            ("capture-window", butterpollo_core::app_window::SETTING),
            ("capture-window-exe", butterpollo_core::app_window::EXE),
            ("capture-window-title", butterpollo_core::app_window::TITLE),
            (
                "capture-window-closed",
                butterpollo_core::app_window::CLOSED,
            ),
        ] {
            if let Some(value) = app
                .extra
                .get(source)
                .filter(|value| !value.is_null() && value.as_str() != Some(""))
            {
                config.values.insert(
                    target.into(),
                    value
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| value.to_string()),
                );
                peak_chosen |= target == "hdr_peak_brightness";
            }
        }
        // The NVIDIA image enhancements, marked as chosen: RTX HDR runs only
        // for an app, device or stream that chose it.
        let inherited = config.clone();
        for (source, target) in butterpollo_core::nvidia_enhance::APP_KEYS {
            if let Some(value) = app
                .extra
                .get(*source)
                .filter(|value| !value.is_null() && value.as_str() != Some(""))
            {
                butterpollo_core::nvidia_enhance::choose(
                    &mut config,
                    &inherited,
                    target,
                    Some(
                        value
                            .as_str()
                            .map(str::to_owned)
                            .unwrap_or_else(|| value.to_string()),
                    ),
                );
            }
        }
    }
    if let Some(overrides) = app
        .as_ref()
        .and_then(|app| app.extra.get("config-overrides"))
        .and_then(serde_json::Value::as_object)
    {
        peak_chosen |= sets_peak(overrides);
        apply_overrides(&mut config, overrides)?;
    }
    // The device's choice for this stream (`rlRtxHdr`, `rlRtxVideo`) replaces
    // the app's, for what this host offers.
    let offered = h
        .nvidia_enhancements
        .load(std::sync::atomic::Ordering::Acquire);
    butterpollo_core::nvidia_enhance::apply_launch(&mut config, &launch.options, offered);
    config.values.insert(
        butterpollo_core::nvidia_enhance::OFFERED_KEY.into(),
        offered.to_string(),
    );
    // Every explicit value of the host, the device and the app is in place;
    // the most specific preset any of them named fills the covered settings
    // that are still unset.
    butterpollo_core::stream_preset::fill(&mut config);
    // A device's HDR calibration gives its screen's peak unless the device
    // or app chose one.
    if !peak_chosen
        && let Some(selection) = launch
            .client
            .extra
            .get("hdr_profile")
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty())
    {
        match butterpollo_windows::hdr_profile::peak_luminance(selection) {
            Ok(Some(peak)) => {
                config.values.insert(
                    "hdr_peak_brightness".into(),
                    peak.clamp(400, 2000).to_string(),
                );
            }
            Ok(None) => tracing::debug!(%selection, "HDR calibration has no MHC2 peak"),
            Err(error) => tracing::debug!(%error, %selection, "HDR calibration peak unavailable"),
        }
    }
    // Last, so no override hands the device's keyboard and mouse back to the
    // desktop the person at the PC is using.
    config.values.remove(crate::nucleus::PLAYER);
    if launch.nucleus.is_some() || launch.role != Role::Stream {
        // A co-op player's stream is its game window already; a stream that
        // shows no app has no window to find.
        for key in butterpollo_core::app_window::KEYS {
            config.values.remove(key);
        }
    } else {
        app_window_capture(&mut config, &launch.warnings);
    }
    if launch.nucleus.is_some() {
        crate::nucleus::isolate(&mut config);
        config.values.remove(butterpollo_core::background::SETTING);
    } else if launch.role == Role::Stream
        && launch.background.unwrap_or_else(|| {
            butterpollo_core::background::enabled(&config)
                || butterpollo_core::background::requested(
                    &launch.client.extra,
                    app.as_ref().map(|app| &app.extra),
                )
        })
    {
        butterpollo_core::background::apply(&mut config);
    } else {
        config.values.remove(butterpollo_core::background::SETTING);
    }
    // WebRTC takes whole access units: the browser's depacketizer wants a frame, not the
    // slices a GameStream client would reassemble from the wire.
    if launch.transport == butterpollo_core::session::Transport::WebRtc {
        config
            .values
            .insert("video_slice_delivery".into(), "off".into());
    }
    Ok(config)
}

/// The warning of a stream whose capture method cannot capture a window.
const APP_WINDOW_METHOD: &str = "capture_app_window_method";

/// A stream of the app's window captures with Windows Graphics Capture, the
/// only method that captures one window; one set to another method uses WGC
/// and says so (Automatic already is WGC for this).
fn app_window_capture(config: &mut Config, warnings: &butterpollo_core::session::Warnings) {
    if butterpollo_core::app_window::settings(config).is_none() {
        warnings.clear(APP_WINDOW_METHOD);
        return;
    }
    let method = config.get("capture", "auto").trim().to_ascii_lowercase();
    let name = match method.as_str() {
        "ddx" | "dxgi" => "Desktop Duplication",
        "idd-push" => "direct driver push",
        _ => {
            warnings.clear(APP_WINDOW_METHOD);
            return;
        }
    };
    tracing::warn!(capture = %method, "the app's window is captured with WGC in place of the capture method set");
    warnings.set(
        APP_WINDOW_METHOD,
        format!("This app's window is captured with Windows Graphics Capture: {name}, the capture method set, captures whole displays only."),
    );
    config.values.insert("capture".into(), "wgc".into());
}

pub(super) fn apply_overrides(
    config: &mut Config,
    overrides: &serde_json::Map<String, serde_json::Value>,
) -> Result<()> {
    let overrides: serde_json::Map<String, serde_json::Value> = overrides
        .iter()
        .filter(|(_, v)| v.as_str() != Some(""))
        // No preset of its own, like "": the layer below keeps its preset,
        // where a null would remove it and fall back to Balanced.
        .filter(|(k, v)| !(k.as_str() == butterpollo_core::stream_preset::SETTING && v.is_null()))
        .filter(|(k, _)| !butterpollo_core::config::removed(k))
        .map(|(k, v)| (butterpollo_core::config::current_name(k).to_owned(), v))
        .filter(|(k, _)| {
            let allowed = butterpollo_core::config::override_allowed(k);
            if !allowed {
                tracing::warn!(key = %k, "ignoring an override of a host-wide setting");
            }
            allowed
        })
        .map(|(k, v)| (k, v.clone()))
        .collect();
    // One at a time: an override the host cannot use (an imported frame
    // limit of -1) is skipped, as in the C++ host, instead of failing every
    // stream of the device or app, or the running stream when it is edited.
    let inherited = config.clone();
    let mut overrides = overrides;
    overrides.retain(|key, value| {
        let single = serde_json::Map::from_iter([(key.clone(), value.clone())]);
        match config.update(&single) {
            Ok(()) => true,
            Err(error) => {
                static WARNED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
                let warning = format!("{key}={value}");
                let mut warned = WARNED.lock().unwrap();
                if !warned.contains(&warning) && warned.len() < 64 {
                    warned.push(warning);
                    tracing::warn!(key, %value, error = format!("{error:#}"), "override ignored");
                }
                false
            }
        }
    });
    // A device or app that sets an enhancement chose it (`nvidia_enhance::marker`);
    // a null restores the inherited value.
    for (key, value) in &overrides {
        if butterpollo_core::nvidia_enhance::is_key(key) {
            if value.is_null() {
                butterpollo_core::nvidia_enhance::choose(config, &inherited, key, None);
            } else {
                config
                    .values
                    .insert(butterpollo_core::nvidia_enhance::marker(key), "true".into());
            }
        }
    }
    Ok(())
}
