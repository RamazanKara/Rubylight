//! JNI bridge for `com.limelight.nvstream.http.HostCapabilities` (`NightjarJni.capabilitiesCall`).
//!
//! Hydrates Java's immutable `HostCapabilities` value wrapper in a single JSON round-trip using
//! `nightjar_policy::http::{HostCapabilities, NativeCapabilities}` so host parsing, capability
//! rules, and `"v": 1` JSON validation live exclusively in Rust.

use jni::JNIEnv;
use jni::objects::{JClass, JString};
use jni::sys::jstring;
use nightjar_policy::http::{
    Availability, Evidence, Family, Feature, HostCapabilities, NativeCapabilities, Reason,
    parse_rubylight_version,
};
use rubylight_protocol::nvidia_enhance::{RTX_HDR, RTX_VIDEO};
use serde_json::{Map, Value, json};

use crate::native_api;

fn family_name(family: Family) -> &'static str {
    use Family::*;
    match family {
        GameStream => "GAMESTREAM",
        Sunshine => "SUNSHINE",
        Apollo => "APOLLO",
        Vibepollo => "VIBEPOLLO",
        Rubylight => "RUBYLIGHT",
        Unknown => "UNKNOWN",
    }
}

fn feature_key(feature: Feature) -> &'static str {
    match feature {
        Feature::Permissions => "permissions",
        Feature::OtpPairing => "otp_pairing",
        Feature::Clipboard => "clipboard",
        Feature::ServerCommands => "server_commands",
        Feature::AppUuids => "app_uuids",
        Feature::LaunchByUuid => "launch_by_uuid",
        Feature::VirtualDisplay => "virtual_display",
        Feature::VirtualDisplayHdr => "virtual_display_hdr",
        Feature::HostDisplayModes => "host_display_modes",
        Feature::DisplayScale => "display_scale",
        Feature::FractionalRefresh => "fractional_refresh",
        Feature::HostVrr => "host_vrr",
        Feature::RuntimeBitrate => "runtime_bitrate",
        Feature::PyroWave => "pyrowave",
        Feature::PyroWaveRecords => "pyrowave_records",
        Feature::PyroWaveBandwidthProbe => "pyrowave_bandwidth_probe",
        Feature::HardwareMetrics => "hardware_metrics",
        Feature::NetworkPath => "network_path",
        Feature::FrameLimiterStatus => "frame_limiter_status",
        Feature::ControllerPreparation => "controller_preparation",
        Feature::RemoteSessionTiles => "remote_session_tiles",
        Feature::HostConfirmsReplacement => "host_confirms_replacement",
        Feature::HostTiming => "host_timing",
        Feature::WebUi => "web_ui",
        Feature::CursorShape => "cursor_shape",
        Feature::FrameWarp => "frame_warp",
        Feature::NativeApi => "native_api",
        Feature::Sleep => "sleep",
        Feature::PcRestart => "pc_restart",
        Feature::PcShutdown => "pc_shutdown",
        Feature::HostUpdate => "host_update",
        Feature::PairingApprove => "pairing_approve",
        Feature::FileUpload => "file_upload",
        Feature::FileDownload => "file_download",
        Feature::DolbyVision => "dolby_vision",
        Feature::Hlg => "hlg",
        Feature::ClientFec => "client_fec",
        Feature::BandwidthProbe => "bandwidth_probe",
        Feature::UsbForwarding => "usb_forwarding",
        Feature::AudioPassthrough => "audio_passthrough",
        Feature::Surround714 => "surround_714",
        Feature::TextInput => "text_input",
        Feature::ClientKeyboardLayout => "client_keyboard_layout",
        Feature::PenBarrelRoll => "pen_barrel_roll",
        Feature::ControllerIdentity => "controller_identity",
        Feature::PipelineRates => "pipeline_rates",
        Feature::SecondScreen => "second_screen",
        Feature::ImageClipboard => "image_clipboard",
        Feature::SharedFolders => "shared_folders",
        Feature::Coop => "coop",
        Feature::NeuralFrameGeneration => "neural_frame_generation",
        Feature::MicOnly => "mic_only",
        Feature::ContinueHere => "continue_here",
        Feature::InternetPlay => "internet_play",
    }
}

fn reason_name(reason: Reason) -> &'static str {
    match reason {
        Reason::UpdateHost => "UPDATE_HOST",
        Reason::Pair => "PAIR",
        Reason::ViewPermission => "VIEW_PERMISSION",
        Reason::ClipboardPermission => "CLIPBOARD_PERMISSION",
        Reason::CommandPermission => "COMMAND_PERMISSION",
        Reason::NoCommands => "NO_COMMANDS",
        Reason::VirtualDisplayOff => "VIRTUAL_DISPLAY_OFF",
        Reason::VirtualDisplayDriver => "VIRTUAL_DISPLAY_DRIVER",
        Reason::HardwareMetricsOff => "HARDWARE_METRICS_OFF",
        Reason::SleepPermission => "SLEEP_PERMISSION",
        Reason::RestartShutdownPermission => "RESTART_SHUTDOWN_PERMISSION",
        Reason::PairingApprovePermission => "PAIRING_APPROVE_PERMISSION",
        Reason::FilePermission => "FILE_PERMISSION",
        Reason::UsbPermission => "USB_PERMISSION",
        Reason::FolderPermission => "FOLDER_PERMISSION",
    }
}

pub(crate) fn snapshot(caps: &HostCapabilities) -> Value {
    let mut features = Map::new();
    for &feature in &Feature::ALL {
        let entry = match caps.availability(feature) {
            Availability::Supported => json!({"state": "supported"}),
            Availability::Unknown => json!({"state": "unknown"}),
            Availability::Unsupported(reason) => json!({
                "state": "unsupported",
                "reason": reason.map(reason_name),
            }),
        };
        features.insert(feature_key(feature).to_owned(), entry);
    }
    json!({
        "json": caps.to_versioned_json(),
        "family": family_name(caps.family()),
        "host_version": caps.host_version(),
        "native_api_version": caps.native_api_version(),
        "hdr_formats": caps.hdr_formats(),
        "max_video_packet": caps.max_video_packet(),
        "nvidia_enhancements": caps.nvidia_enhancements(),
        "native_features": caps.native_features(),
        "offers_rtx_hdr": caps.offers_enhancement(RTX_HDR),
        "offers_rtx_video": caps.offers_enhancement(RTX_VIDEO),
        "enhancements": caps.enhancement_parameters(None, None),
        "features": Value::Object(features),
    })
}

fn parse_native(json_str: &str) -> Result<NativeCapabilities, String> {
    serde_json::from_str::<NativeCapabilities>(json_str)
        .map_err(|e| format!("invalid native capabilities: {e}"))
}

pub fn call(request: Option<&str>) -> String {
    let Some(request) = request else {
        return json!({"error": "invalid", "message": "request is null"}).to_string();
    };
    let req: Value = match serde_json::from_str(request) {
        Ok(value) => value,
        Err(e) => return json!({"error": "json", "message": e.to_string()}).to_string(),
    };
    match req.get("op").and_then(Value::as_str) {
        Some("unknown") => json!({"ok": snapshot(&HostCapabilities::UNKNOWN)}).to_string(),
        Some("from_xml") => {
            let xml = req.get("xml").and_then(Value::as_str).unwrap_or_default();
            match HostCapabilities::from_server_info_xml(xml) {
                Ok(caps) => json!({"ok": snapshot(&caps)}).to_string(),
                Err(e) => json!({"error": "malformed", "message": e.to_string()}).to_string(),
            }
        }
        Some("from_json") => {
            let Some(raw) = req.get("json").and_then(Value::as_str) else {
                return json!({"error": "json", "message": "missing capabilities json"})
                    .to_string();
            };
            match Evidence::from_versioned_json(raw) {
                Some(ev) => {
                    json!({"ok": snapshot(&HostCapabilities::from_evidence(ev))}).to_string()
                }
                None => {
                    json!({"error": "json", "message": "invalid capabilities json"}).to_string()
                }
            }
        }
        Some("with_native") => {
            let Some(native_raw) = req.get("native").and_then(Value::as_str) else {
                return json!({"error": "json", "message": "missing native json"}).to_string();
            };
            let native = match parse_native(native_raw) {
                Ok(n) => n,
                Err(msg) => return json!({"error": "json", "message": msg}).to_string(),
            };
            let base = match req.get("caps").and_then(Value::as_str) {
                Some(caps_raw) => HostCapabilities::from_versioned_json(caps_raw),
                None => HostCapabilities::UNKNOWN,
            };
            json!({"ok": snapshot(&base.with_native(&native))}).to_string()
        }
        Some("enhancement_parameters") => {
            let base = match req.get("caps").and_then(Value::as_str) {
                Some(caps_raw) => HostCapabilities::from_versioned_json(caps_raw),
                None => HostCapabilities::UNKNOWN,
            };
            let rtx_hdr = req.get("rtx_hdr").and_then(Value::as_bool);
            let rtx_video = req.get("rtx_video").and_then(Value::as_bool);
            json!({"ok": base.enhancement_parameters(rtx_hdr, rtx_video)}).to_string()
        }
        Some("parse_rubylight_version") => {
            let version = req
                .get("version")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let parsed = parse_rubylight_version(version).map(|v| json!([v[0], v[1], v[2], v[3]]));
            json!({"ok": parsed}).to_string()
        }
        _ => json!({"error": "invalid", "message": "unknown capabilities op"}).to_string(),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_nativeCapabilitiesCall(
    mut env: JNIEnv,
    _: JClass,
    request: JString,
) -> jstring {
    let request = native_api::string(&mut env, &request);
    let reply = call(request.as_deref());
    match env.new_string(reply) {
        Ok(s) => s.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_xml_and_json_round_trip_new_fields() {
        let xml = r#"<root status_code="200"><appversion>7.1.431.-1</appversion><PairStatus>1</PairStatus>
            <RustHostVersion>3.0.0</RustHostVersion><Permission>4294967295</Permission>
            <NativeApiVersion>1</NativeApiVersion>
        </root>"#;
        let out: Value = serde_json::from_str(&call(Some(
            &json!({"op": "from_xml", "xml": xml}).to_string(),
        )))
        .unwrap();
        let caps_json = out["ok"]["json"].as_str().unwrap();
        let native_json = json!({
            "api_version": 1,
            "host": {"name": "RL", "version": "3.0.0", "unique_id": "rl"},
            "features": ["power.restart", "power.shutdown", "update", "pairing.approve"],
            "permissions": {
                "list": true,
                "view": true,
                "launch": true,
                "host_control": true,
                "restart_shutdown": true,
                "pairing_approve": true
            }
        })
        .to_string();
        let out: Value = serde_json::from_str(&call(Some(
            &json!({"op": "with_native", "caps": caps_json, "native": native_json}).to_string(),
        )))
        .unwrap();
        let ok = &out["ok"];
        assert_eq!(ok["family"], "RUBYLIGHT");
        assert_eq!(ok["features"]["pc_restart"]["state"], "supported");
        assert_eq!(ok["features"]["pc_shutdown"]["state"], "supported");
        assert_eq!(ok["features"]["host_update"]["state"], "supported");
        assert_eq!(ok["features"]["pairing_approve"]["state"], "supported");

        let round_json = ok["json"].as_str().unwrap();
        let round: Value = serde_json::from_str(&call(Some(
            &json!({"op": "from_json", "json": round_json}).to_string(),
        )))
        .unwrap();
        assert_eq!(round["ok"]["features"]["pc_restart"]["state"], "supported");
    }

    #[test]
    fn with_native_on_unknown_still_validates_malformed_json() {
        let out: Value = serde_json::from_str(&call(Some(
            &json!({"op": "with_native", "caps": null, "native": "not json"}).to_string(),
        )))
        .unwrap();
        assert_eq!(out["error"], "json");
    }

    #[test]
    fn family_name_matches_expected_uppercase_variants() {
        assert_eq!(family_name(Family::GameStream), "GAMESTREAM");
        assert_eq!(family_name(Family::Sunshine), "SUNSHINE");
        assert_eq!(family_name(Family::Apollo), "APOLLO");
        assert_eq!(family_name(Family::Vibepollo), "VIBEPOLLO");
        assert_eq!(family_name(Family::Rubylight), "RUBYLIGHT");
        assert_eq!(family_name(Family::Unknown), "UNKNOWN");
    }
}
