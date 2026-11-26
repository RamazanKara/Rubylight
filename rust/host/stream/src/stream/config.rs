//! A launch's effective configuration: host, device, app and stream-preset layers.
use super::*;
use butterpollo_session::config_ext::ConfigExt;

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
pub fn device_config(h: &Shared, client: &butterpollo_session::state::Client) -> Result<Config> {
    let mut config = h.config.read().unwrap().clone();
    device_layer(&mut config, client)?;
    butterpollo_media_policy::stream_preset::fill(&mut config);
    Ok(config)
}

/// Applies the device's overrides. Returns whether they chose the peak
/// brightness.
fn device_layer(config: &mut Config, client: &butterpollo_session::state::Client) -> Result<bool> {
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
pub fn launch_app(h: &Shared, launch: &Launch) -> Option<butterpollo_session::state::App> {
    h.apps
        .read()
        .unwrap()
        .iter()
        .find(|app| app.id() == launch.app_id || app.aliases.contains(&launch.app_id))
        .map(|app| {
            butterpollo_session::device_overrides::for_device(
                app,
                butterpollo_session::state::device_uuid(&launch.client.uuid),
            )
        })
}

/// Whether `launch`'s app starts through Steam ([`butterpollo_session::steam::launched_through_steam`]),
/// for its controllers' [`butterpollo_input_policy::input_policy::Policy::steam_app`]. A Nucleus Co-op
/// launch (a co-op entry or a background stream) never does, whatever its app: Nucleus Co-op
/// starts the game's .exe itself, a Steam game's too.
pub fn launched_through_steam(h: &Shared, launch: &Launch) -> bool {
    launch.nucleus.is_none()
        && launch_app(h, launch)
            .and_then(|app| serde_json::to_value(app).ok())
            .is_some_and(|app| butterpollo_session::steam::launched_through_steam(&app))
}

/// The stream's settings: host, then device, then app, then the app's block for the device.
pub fn effective_config(h: &Shared, launch: &Launch) -> Result<Config> {
    let mut config = h.config.read().unwrap().clone();
    let mut peak_chosen = device_layer(&mut config, &launch.client)?;
    let app = launch_app(h, launch);
    if let Some(app) = &app {
        for (source, target) in [
            ("gamepad", "gamepad"),
            ("dd-configuration-option", "dd_configuration_option"),
            ("prefer-10bit-sdr", "prefer_sdr_10bit"),
            // How long a touch or pen tap holds its left-button release back
            // (the host's `absolute_left_release_delay`), and whether native
            // pen/touch injection is used for this app (`native_pen_touch`).
            ("absolute-left-release-delay", "absolute_left_release_delay"),
            ("native-pen-touch", "native_pen_touch"),
            // Night mode, headphone surround and the couch menu for this app
            // (the console's Sound and couch menu panel).
            ("audio-night-mode", "audio_night_mode"),
            ("audio-headphone-surround", "audio_headphone_surround"),
            ("couch-menu", "couch_menu"),
            ("couch-menu-open-with", "couch_menu_open_with"),
            ("couch-menu-controller-chord", "couch_menu_controller_chord"),
            ("couch-menu-hold-ms", "couch_menu_hold_ms"),
            ("couch-menu-corner-hold-ms", "couch_menu_corner_hold_ms"),
            ("couch-menu-keyboard-chord", "couch_menu_keyboard_chord"),
            // Saved by Vibepollo and earlier Rubylight apps for RTX HDR;
            // the peak is still what a virtual display reports.
            ("rtx-hdr-peak-brightness", "hdr_peak_brightness"),
            // The app's window in place of a display (app_window.rs).
            (
                "capture-window",
                butterpollo_media_policy::app_window::SETTING,
            ),
            (
                "capture-window-exe",
                butterpollo_media_policy::app_window::EXE,
            ),
            (
                "capture-window-title",
                butterpollo_media_policy::app_window::TITLE,
            ),
            (
                "capture-window-closed",
                butterpollo_media_policy::app_window::CLOSED,
            ),
            (
                "capture-window-frame",
                butterpollo_media_policy::app_window::FRAME,
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
        for (source, target) in butterpollo_media_policy::nvidia_enhance::APP_KEYS {
            if let Some(value) = app
                .extra
                .get(*source)
                .filter(|value| !value.is_null() && value.as_str() != Some(""))
            {
                butterpollo_media_policy::nvidia_enhance::choose(
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
    butterpollo_media_policy::nvidia_enhance::apply_launch(&mut config, &launch.options, offered);
    // And its `rlHdrDynamic=0` turns HDR10+ and HDR Vivid off for this stream.
    butterpollo_media_policy::extensions::apply_hdr_dynamic(&mut config, &launch.options)?;
    config.values.insert(
        butterpollo_media_policy::nvidia_enhance::OFFERED_KEY.into(),
        offered.to_string(),
    );
    // Every explicit value of the host, the device and the app is in place;
    // the most specific preset any of them named fills the covered settings
    // that are still unset.
    butterpollo_media_policy::stream_preset::fill(&mut config);
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
        match butterpollo_win_display::hdr_profile::peak_luminance(selection) {
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
        for key in butterpollo_media_policy::app_window::KEYS {
            config.values.remove(key);
        }
    } else {
        app_window_capture(&mut config, &launch.warnings);
    }
    if launch.nucleus.is_some() {
        crate::nucleus::isolate(&mut config);
        config
            .values
            .remove(butterpollo_session::background::SETTING);
    } else if launch.role == Role::Stream
        && launch.background.unwrap_or_else(|| {
            butterpollo_session::background::enabled(&config)
                || butterpollo_session::background::requested(
                    &launch.client.extra,
                    app.as_ref().map(|app| &app.extra),
                )
        })
    {
        butterpollo_session::background::apply(&mut config);
    } else {
        config
            .values
            .remove(butterpollo_session::background::SETTING);
    }
    // WebRTC takes whole access units: the browser's depacketizer wants a frame, not the
    // slices a GameStream client would reassemble from the wire.
    if launch.transport == butterpollo_session::session::Transport::WebRtc {
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
fn app_window_capture(config: &mut Config, warnings: &butterpollo_session::session::Warnings) {
    if butterpollo_media_policy::app_window::settings(config).is_none() {
        warnings.clear(APP_WINDOW_METHOD);
        return;
    }
    let method = config.get("capture", "auto").trim().to_ascii_lowercase();
    let name = match method.as_str() {
        "ddx" | "dxgi" => "Desktop Duplication",
        "idd-push" => "direct driver push",
        "amd" => "AMD Direct Capture",
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
    // Overrides saved by an earlier release, not yet migrated on load, are
    // read under their current names and values.
    let mut layer = overrides.clone();
    butterpollo_config::config::migrate_overrides(&mut layer);
    // The retired bitrate, pacing and remote monitor switches the layer still
    // holds alone fold over the layers below, as the earlier switches combined.
    config.fold_layer(&mut layer);
    let overrides: serde_json::Map<String, serde_json::Value> = layer
        .iter()
        .filter(|(_, v)| v.as_str() != Some(""))
        // No preset of its own, like "": the layer below keeps its preset,
        // where a null would remove it and fall back to Balanced.
        .filter(|(k, v)| {
            !(k.as_str() == butterpollo_media_policy::stream_preset::SETTING && v.is_null())
        })
        .filter(|(k, _)| !butterpollo_config::config::removed(k))
        .map(|(k, v)| (butterpollo_config::config::current_name(k).to_owned(), v))
        .filter(|(k, _)| {
            // A retired HDR request without the layer's own Display HDR folds
            // over the layers below in `Config::update`.
            let allowed = k == butterpollo_config::config::HDR_REQUEST
                || butterpollo_config::config::override_allowed(k);
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
        if butterpollo_media_policy::nvidia_enhance::is_key(key) {
            if value.is_null() {
                butterpollo_media_policy::nvidia_enhance::choose(config, &inherited, key, None);
            } else {
                config.values.insert(
                    butterpollo_media_policy::nvidia_enhance::marker(key),
                    "true".into(),
                );
            }
        }
    }
    Ok(())
}

/// Caps `stream`'s coded picture at the host's `max_stream_resolution`, leaving
/// the display at the size the client asked for, and tells the device and the
/// console why when the picture is smaller. `stream` holds the requested size
/// when this is called (a launch's mode or an ANNOUNCE).
pub fn cap_stream_resolution(
    stream: &mut butterpollo_media_policy::rtsp::Negotiated,
    config: &Config,
    warnings: &butterpollo_session::session::Warnings,
) {
    let cap = config.max_stream_resolution();
    stream.set_display_size((stream.width, stream.height), cap);
    report_stream_resolution_cap(stream, cap, warnings);
}

/// Tells the device and the console that the host's cap codes a smaller picture
/// than the display keeps, and clears that reason when none does. Runs at the
/// connection and after a size change the client asked for, so a switch into
/// the cap takes the reason off the stream card.
pub fn report_stream_resolution_cap(
    stream: &butterpollo_media_policy::rtsp::Negotiated,
    cap: Option<(u32, u32)>,
    warnings: &butterpollo_session::session::Warnings,
) {
    let Some((cap_width, cap_height)) = cap.filter(|_| stream.capped()) else {
        warnings.clear("stream_resolution_cap");
        return;
    };
    let (display_width, display_height) = stream.display_size();
    tracing::info!(
        coded_width = stream.width,
        coded_height = stream.height,
        display_width,
        display_height,
        cap_width,
        cap_height,
        "stream resolution capped"
    );
    warnings.set(
        "stream_resolution_cap",
        format!(
            "This stream is {}x{} because Maximum stream resolution (max_stream_resolution) is {cap_width}x{cap_height}. The PC's display keeps {display_width}x{display_height}, so the picture is scaled to the smaller size; set max_stream_resolution to native for the size the device asks for.",
            stream.width, stream.height
        ),
    );
}

/// Typed accessors for stream settings read outside `stream/config.rs`.
pub(super) fn output_name(config: &Config) -> &str {
    config.get("output_name", "")
}

pub(super) fn gamepad_setting(config: &Config) -> &str {
    config.get("gamepad", "auto")
}

pub(super) fn audio_capture_mode_is_game(config: &Config) -> bool {
    config.get("audio_capture_mode", "system") == "game"
}

pub(super) fn stream_reconfigure_enabled(config: &Config) -> bool {
    config.boolean("stream_reconfigure", true)
}

pub(super) fn capture_setting(config: &Config) -> &str {
    config.get("capture", "")
}

pub(super) fn adapter_name(config: &Config) -> &str {
    config.get("adapter_name", "")
}

pub(super) fn adapter_pnp_id(config: &Config) -> &str {
    config.get("adapter_pnp_id", "")
}

pub(super) fn wgc_slot_aligned_publish(config: &Config) -> bool {
    config.boolean("wgc_slot_aligned_publish", false)
}

pub(super) fn gpu_compute_conversion(config: &Config) -> bool {
    config.boolean("gpu_compute_conversion", true)
}

pub(super) fn wgc_compute_copy(config: &Config) -> bool {
    config.boolean("wgc_compute_copy", true)
}

pub(super) fn wgc_user_helper(config: &Config) -> bool {
    config.boolean("wgc_user_helper", false)
}

pub(super) fn wgc_high_rate_capture(config: &Config) -> bool {
    config.boolean("wgc_high_rate_capture", false)
}

pub(super) fn wgc_drain_to_newest(config: &Config) -> bool {
    config.boolean("wgc_drain_to_newest", false)
}

pub(super) fn wgc_helper_streaming_scope(config: &Config) -> bool {
    config.boolean("wgc_helper_streaming_scope", false)
}

pub(super) fn capture_poll_interval_us(config: &Config) -> u64 {
    config
        .integer("capture_poll_interval_us", 500)
        .clamp(100, 1000) as u64
}

pub(super) fn capture_predictive_poll(config: &Config) -> bool {
    config.boolean("capture_predictive_poll", false)
}

pub(super) fn minimum_fps_target(config: &Config) -> &str {
    config.get("minimum_fps_target", "20")
}

pub(super) fn wgc_pacing_smoothing(config: &Config) -> bool {
    config.boolean("wgc_pacing_smoothing", true)
}

pub(super) fn frame_pacing_predictive(config: &Config) -> bool {
    config.boolean("frame_pacing_predictive", true)
}

pub(super) fn wgc_direct_encoder_input(config: &Config) -> bool {
    config.boolean("wgc_direct_encoder_input", true)
}

pub(super) fn video_send_thread(config: &Config) -> bool {
    config.boolean("video_send_thread", true)
}

pub(super) fn video_max_batch_size_kb(config: &Config) -> usize {
    match config.integer("video_max_batch_size_kb", 64) {
        16 => 16,
        32 => 32,
        _ => 64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn negotiated(width: u32, height: u32) -> butterpollo_media_policy::rtsp::Negotiated {
        butterpollo_media_policy::rtsp::Negotiated {
            width,
            height,
            ..Default::default()
        }
    }

    fn reason(warnings: &butterpollo_session::session::Warnings) -> Option<String> {
        warnings
            .snapshot()
            .into_iter()
            .find(|warning| warning.code == "stream_resolution_cap")
            .map(|warning| warning.message)
    }

    #[test]
    fn a_resolution_cap_codes_1440p_while_the_display_keeps_4k_and_says_why() {
        let warnings = butterpollo_session::session::Warnings::default();
        let config = Config::parse("max_stream_resolution = 2560x1440\n").unwrap();
        let mut capped = negotiated(3840, 2160);
        cap_stream_resolution(&mut capped, &config, &warnings);
        assert_eq!((capped.width, capped.height), (2560, 1440));
        assert_eq!(capped.display_size(), (3840, 2160));
        let text = reason(&warnings).expect("a capped stream says why");
        assert!(text.contains("2560x1440"), "{text}");
        assert!(text.contains("3840x2160"), "{text}");
        // Native, today's default, codes the request and clears the reason.
        let mut native = negotiated(3840, 2160);
        cap_stream_resolution(&mut native, &Config::default(), &warnings);
        assert_eq!((native.width, native.height), (3840, 2160));
        assert_eq!(native.display_size(), (3840, 2160));
        assert!(reason(&warnings).is_none());
        // A request the cap fits in is coded as it is, and keeps the display.
        let mut small = negotiated(1920, 1080);
        cap_stream_resolution(&mut small, &config, &warnings);
        assert_eq!((small.width, small.height), (1920, 1080));
        assert_eq!(small.display_size(), (1920, 1080));
        assert!(reason(&warnings).is_none());
    }

    #[test]
    fn a_size_change_into_the_cap_takes_the_reason_off_the_stream_card() {
        let warnings = butterpollo_session::session::Warnings::default();
        let cap = Some((2560, 1440));
        let mut capped = negotiated(3840, 2160);
        capped.set_display_size((3840, 2160), cap);
        report_stream_resolution_cap(&capped, cap, &warnings);
        assert!(reason(&warnings).is_some());
        // The client asks for a size the cap fits mid-stream: the reason no
        // longer applies when the switch lands.
        let mut smaller = negotiated(1920, 1080);
        smaller.set_display_size((1920, 1080), cap);
        report_stream_resolution_cap(&smaller, cap, &warnings);
        assert!(reason(&warnings).is_none());
        // A switch to another size the cap still shrinks rewrites it.
        let mut again = negotiated(3840, 2160);
        again.set_display_size((3840, 2160), cap);
        report_stream_resolution_cap(&again, cap, &warnings);
        let text = reason(&warnings).expect("capped again");
        assert!(text.contains("2560x1440"), "{text}");
        assert!(text.contains("3840x2160"), "{text}");
    }

    #[test]
    fn an_app_can_override_native_pen_touch_over_host_and_device() {
        let f = crate::state::test_support::Fixture::new();
        *f.host.config.write().unwrap() = Config::parse("native_pen_touch = true\n").unwrap();
        let mut client = f.client(u32::MAX);
        client.extra.insert(
            "config_overrides".into(),
            serde_json::json!({"native_pen_touch": true}),
        );
        // App turns native pen/touch off -> effective_config + input policy + pen barrel roll follow.
        f.host.apps.write().unwrap()[0]
            .extra
            .insert("native-pen-touch".into(), serde_json::json!(false));
        let launch = f.launch(client.clone(), Role::Stream);
        let config = effective_config(&f.host, &launch).unwrap();
        assert!(!config.boolean("native_pen_touch", true));
        assert!(!crate::extensions::offered(&f.host, &config).pen_barrel_roll);
        let policy = butterpollo_input_policy::input_policy::Policy::resolve(&config).unwrap();
        assert!(!policy.native_pen_touch);

        // Host off, app turns native pen/touch back on.
        *f.host.config.write().unwrap() = Config::parse("native_pen_touch = false\n").unwrap();
        client.extra.remove("config_overrides");
        f.host.apps.write().unwrap()[0]
            .extra
            .insert("native-pen-touch".into(), serde_json::json!(true));
        let config = effective_config(&f.host, &f.launch(client, Role::Stream)).unwrap();
        assert!(config.boolean("native_pen_touch", false));
        assert!(crate::extensions::offered(&f.host, &config).pen_barrel_roll);
        let policy = butterpollo_input_policy::input_policy::Policy::resolve(&config).unwrap();
        assert!(policy.native_pen_touch);
    }

    #[test]
    fn a_device_can_override_stream_audio_passthrough_and_sink() {
        let f = crate::state::test_support::Fixture::new();
        *f.host.config.write().unwrap() = Config::parse(
            "stream_audio = true\naudio_passthrough = false\naudio_sink = Host Speaker\n",
        )
        .unwrap();
        let mut client = f.client(u32::MAX);
        client.extra.insert(
            "config_overrides".into(),
            serde_json::json!({
                "stream_audio": false,
                "audio_passthrough": true,
                "audio_sink": "HDMI AVR",
            }),
        );
        let dev = device_config(&f.host, &client).unwrap();
        assert!(!dev.boolean("stream_audio", true));
        assert!(dev.audio_passthrough());
        assert_eq!(dev.get("audio_sink", ""), "HDMI AVR");
        let eff = effective_config(&f.host, &f.launch(client, Role::Stream)).unwrap();
        assert!(!eff.boolean("stream_audio", true));
        assert!(eff.audio_passthrough());
        assert_eq!(eff.get("audio_sink", ""), "HDMI AVR");
    }
}
