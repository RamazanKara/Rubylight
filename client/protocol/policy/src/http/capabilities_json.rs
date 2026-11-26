//! Versioned `"v": 1` camelCase JSON serialization for [`Evidence`] and [`HostCapabilities`],
//! matching the host-capability cache format stored by Nightjar Android.

use super::capabilities::{Evidence, HostCapabilities, PYROWAVE_CODEC_BITS, video_packet_offer};
use serde_json::{Map, Value};

const JSON_VERSION: i64 = 1;

impl Evidence {
    /// Serializes this evidence to the versioned `"v": 1` camelCase JSON cache format.
    pub fn to_versioned_json(&self) -> String {
        let mut map = Map::new();
        map.insert("v".into(), Value::from(JSON_VERSION));
        map.insert("gameStream".into(), Value::from(self.game_stream));
        map.insert("sunshineVersion".into(), Value::from(self.sunshine_version));
        map.insert("paired".into(), Value::from(self.paired));
        map.insert("permission".into(), Value::from(self.permission));
        map.insert("serverCommands".into(), Value::from(self.server_commands));
        map.insert("pyroWaveFields".into(), Value::from(self.pyrowave_fields));
        map.insert(
            "pyroWaveProbeBytes".into(),
            Value::from(self.pyrowave_probe_bytes),
        );
        map.insert("pyroWaveCodecs".into(), Value::from(self.pyrowave_codecs));
        map.insert("encoderBitrate".into(), Value::from(self.encoder_bitrate));
        map.insert(
            "hardwareMetricsVersion".into(),
            Value::from(self.hardware_metrics_version),
        );
        map.insert(
            "networkPathVersion".into(),
            Value::from(self.network_path_version),
        );
        map.insert(
            "controllerPreparation".into(),
            Value::from(self.controller_preparation),
        );
        map.insert(
            "currentGameUuid".into(),
            Value::from(self.current_game_uuid),
        );
        map.insert(
            "cursorShapeVersion".into(),
            Value::from(self.cursor_shape_version),
        );
        map.insert(
            "frameWarpVersion".into(),
            Value::from(self.frame_warp_version),
        );
        map.insert(
            "frameWarpProtocol".into(),
            Value::from(self.frame_warp_protocol),
        );
        map.insert(
            "nativeApiVersion".into(),
            Value::from(self.native_api_version),
        );
        map.insert(
            "powerActionVersion".into(),
            Value::from(self.power_action_version),
        );
        map.insert("sleepSupported".into(), Value::from(self.sleep_supported));
        map.insert("sleepAllowed".into(), Value::from(self.sleep_allowed));
        map.insert(
            "nativeCapabilities".into(),
            Value::from(self.native_capabilities),
        );
        map.insert("filesUpload".into(), Value::from(self.files_upload));
        map.insert("filesDownload".into(), Value::from(self.files_download));
        map.insert(
            "fileUploadPermission".into(),
            Value::from(self.file_upload_permission),
        );
        map.insert(
            "fileDownloadPermission".into(),
            Value::from(self.file_download_permission),
        );
        map.insert("hdrFormats".into(), Value::from(self.hdr_formats));
        map.insert(
            "fecControlVersion".into(),
            Value::from(self.fec_control_version),
        );
        map.insert(
            "bandwidthProbeVersion".into(),
            Value::from(self.bandwidth_probe_version),
        );
        map.insert("audioCodecs".into(), Value::from(self.audio_codecs));
        map.insert("audioLayouts".into(), Value::from(self.audio_layouts));
        map.insert(
            "textInputVersion".into(),
            Value::from(self.text_input_version),
        );
        map.insert(
            "keyboardLayoutVersion".into(),
            Value::from(self.keyboard_layout_version),
        );
        map.insert(
            "penBarrelRollVersion".into(),
            Value::from(self.pen_barrel_roll_version),
        );
        map.insert(
            "controllerIdentityVersion".into(),
            Value::from(self.controller_identity_version),
        );
        map.insert(
            "pipelineRatesVersion".into(),
            Value::from(self.pipeline_rates_version),
        );
        map.insert(
            "displayScaleVersion".into(),
            Value::from(self.display_scale_version),
        );
        map.insert(
            "secondScreenVersion".into(),
            Value::from(self.second_screen_version),
        );
        map.insert(
            "nvidiaEnhancements".into(),
            Value::from(self.nvidia_enhancements),
        );
        map.insert("maxVideoPacket".into(), Value::from(self.max_video_packet));
        map.insert(
            "internetPlayVersion".into(),
            Value::from(self.internet_play_version),
        );
        map.insert("usbOffered".into(), Value::from(self.usb_offered));
        map.insert("usbPermission".into(), Value::from(self.usb_permission));
        map.insert(
            "imageClipboardOffered".into(),
            Value::from(self.image_clipboard_offered),
        );
        map.insert("foldersOffered".into(), Value::from(self.folders_offered));
        map.insert(
            "folderPermission".into(),
            Value::from(self.folder_permission),
        );
        map.insert("coopOffered".into(), Value::from(self.coop_offered));
        map.insert("micOnlyOffered".into(), Value::from(self.mic_only_offered));
        map.insert("neuralOffered".into(), Value::from(self.neural_offered));
        map.insert("viewPermission".into(), Value::from(self.view_permission));
        map.insert("restartOffered".into(), Value::from(self.restart_offered));
        map.insert("shutdownOffered".into(), Value::from(self.shutdown_offered));
        map.insert(
            "restartShutdownPermission".into(),
            Value::from(self.restart_shutdown_permission),
        );
        map.insert("updateOffered".into(), Value::from(self.update_offered));
        map.insert("pairingOffered".into(), Value::from(self.pairing_offered));
        map.insert(
            "pairingPermission".into(),
            Value::from(self.pairing_permission),
        );
        map.insert(
            "continueHereOffered".into(),
            Value::from(self.continue_here_offered),
        );
        if let Some(version) = &self.rust_host_version {
            map.insert("rustHostVersion".into(), Value::from(version.as_str()));
        }
        if let Some(v) = self.frame_limiter {
            map.insert("frameLimiter".into(), Value::from(v));
        }
        if let Some(v) = self.virtual_display_capable {
            map.insert("virtualDisplayCapable".into(), Value::from(v));
        }
        if let Some(v) = self.virtual_display_driver_ready {
            map.insert("virtualDisplayDriverReady".into(), Value::from(v));
        }
        if let Some(v) = self.virtual_display_hdr_capable {
            map.insert("virtualDisplayHdrCapable".into(), Value::from(v));
        }
        Value::Object(map).to_string()
    }

    /// Parses a versioned `"v": 1` cache JSON, returning `None` if missing required fields,
    /// malformed, or of an unsupported version.
    pub fn from_versioned_json(json: &str) -> Option<Self> {
        let Value::Object(obj) = serde_json::from_str::<Value>(json).ok()? else {
            return None;
        };
        if obj.get("v").and_then(Value::as_i64) != Some(JSON_VERSION) {
            return None;
        }
        let req_bool = |k: &str| obj.get(k)?.as_bool();
        let req_i64 = |k: &str| obj.get(k)?.as_i64();
        let req_i32 = |k: &str| i32::try_from(req_i64(k)?).ok();
        let opt_i32 = |k: &str, default: i32| -> Option<i32> {
            match obj.get(k) {
                None => Some(default),
                Some(v) => i32::try_from(v.as_i64()?).ok(),
            }
        };
        let opt_bool = |k: &str| -> Option<bool> {
            match obj.get(k) {
                None => Some(false),
                Some(v) => v.as_bool(),
            }
        };
        let opt_tri = |k: &str| -> Option<Option<bool>> {
            match obj.get(k) {
                None => Some(None),
                Some(v) => Some(Some(v.as_bool()?)),
            }
        };
        let rust_host_version = match obj.get("rustHostVersion") {
            None => None,
            Some(v) => {
                let s = v.as_str()?;
                if s.is_empty() {
                    return None;
                }
                Some(s.to_owned())
            }
        };
        let permission = req_i64("permission")?;
        if !(-1..=0xFFFF_FFFF).contains(&permission) {
            return None;
        }
        let server_commands = u32::try_from(req_i64("serverCommands")?).ok()?;
        let pyrowave_probe_bytes = u32::try_from(req_i64("pyroWaveProbeBytes")?).ok()?;
        let pyrowave_codecs = (req_i64("pyroWaveCodecs")? as u32) & PYROWAVE_CODEC_BITS;
        let max_video_packet = match obj.get("maxVideoPacket") {
            None => 0,
            Some(Value::Number(n)) => video_packet_offer(&n.to_string()),
            Some(Value::String(s)) => video_packet_offer(s),
            Some(_) => 0,
        };
        let nvidia_enhancements =
            (opt_i32("nvidiaEnhancements", 0)? as u8) & rubylight_protocol::nvidia_enhance::ALL;

        Some(Evidence {
            game_stream: req_bool("gameStream")?,
            sunshine_version: req_bool("sunshineVersion")?,
            paired: req_bool("paired")?,
            rust_host_version,
            permission,
            server_commands,
            virtual_display_capable: opt_tri("virtualDisplayCapable")?,
            virtual_display_driver_ready: opt_tri("virtualDisplayDriverReady")?,
            virtual_display_hdr_capable: opt_tri("virtualDisplayHdrCapable")?,
            frame_limiter: opt_tri("frameLimiter")?,
            pyrowave_fields: req_bool("pyroWaveFields")?,
            pyrowave_probe_bytes,
            pyrowave_codecs,
            encoder_bitrate: req_bool("encoderBitrate")?,
            hardware_metrics_version: req_i32("hardwareMetricsVersion")?,
            network_path_version: req_i32("networkPathVersion")?,
            controller_preparation: req_bool("controllerPreparation")?,
            current_game_uuid: req_bool("currentGameUuid")?,
            cursor_shape_version: opt_i32("cursorShapeVersion", -1)?,
            frame_warp_version: opt_i32("frameWarpVersion", -1)?,
            frame_warp_protocol: opt_i32("frameWarpProtocol", -1)?,
            native_api_version: opt_i32("nativeApiVersion", -1)?,
            power_action_version: opt_i32("powerActionVersion", -1)?,
            sleep_supported: opt_bool("sleepSupported")?,
            sleep_allowed: opt_bool("sleepAllowed")?,
            native_capabilities: opt_bool("nativeCapabilities")?,
            files_upload: opt_bool("filesUpload")?,
            files_download: opt_bool("filesDownload")?,
            file_upload_permission: opt_bool("fileUploadPermission")?,
            file_download_permission: opt_bool("fileDownloadPermission")?,
            hdr_formats: opt_i32("hdrFormats", -1)?,
            fec_control_version: opt_i32("fecControlVersion", -1)?,
            bandwidth_probe_version: opt_i32("bandwidthProbeVersion", -1)?,
            audio_codecs: opt_i32("audioCodecs", -1)?,
            audio_layouts: opt_i32("audioLayouts", -1)?,
            text_input_version: opt_i32("textInputVersion", -1)?,
            keyboard_layout_version: opt_i32("keyboardLayoutVersion", -1)?,
            pen_barrel_roll_version: opt_i32("penBarrelRollVersion", -1)?,
            controller_identity_version: opt_i32("controllerIdentityVersion", -1)?,
            pipeline_rates_version: opt_i32("pipelineRatesVersion", -1)?,
            display_scale_version: opt_i32("displayScaleVersion", -1)?,
            second_screen_version: opt_i32("secondScreenVersion", -1)?,
            nvidia_enhancements,
            internet_play_version: opt_i32("internetPlayVersion", -1)?,
            usb_offered: opt_bool("usbOffered")?,
            usb_permission: opt_bool("usbPermission")?,
            image_clipboard_offered: opt_bool("imageClipboardOffered")?,
            folders_offered: opt_bool("foldersOffered")?,
            folder_permission: opt_bool("folderPermission")?,
            coop_offered: opt_bool("coopOffered")?,
            neural_offered: opt_bool("neuralOffered")?,
            view_permission: opt_bool("viewPermission")?,
            mic_only_offered: opt_bool("micOnlyOffered")?,
            restart_offered: opt_bool("restartOffered")?,
            shutdown_offered: opt_bool("shutdownOffered")?,
            restart_shutdown_permission: opt_bool("restartShutdownPermission")?,
            update_offered: opt_bool("updateOffered")?,
            pairing_offered: opt_bool("pairingOffered")?,
            pairing_permission: opt_bool("pairingPermission")?,
            continue_here_offered: opt_bool("continueHereOffered")?,
            max_video_packet,
        })
    }
}

impl HostCapabilities {
    pub fn to_versioned_json(&self) -> Option<String> {
        self.evidence().map(Evidence::to_versioned_json)
    }

    pub fn from_versioned_json(json: &str) -> Self {
        Evidence::from_versioned_json(json).map_or(Self::UNKNOWN, Self::from_evidence)
    }
}
