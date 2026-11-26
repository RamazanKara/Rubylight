//! The host's `/serverinfo` reply, read as NvHTTP.getComputerDetails does, including the
//! Sunshine, Apollo, Vibepollo and Rubylight extensions.

use super::capabilities::{Evidence, Family, HostCapabilities, PYROWAVE_BANDWIDTH_PROBE_BYTES};
use super::xml::{self, Element};
use super::{DEFAULT_HTTPS_PORT, Error};

/// One serverinfo reply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerInfo {
    /// The reply as received, for callers that cache it.
    pub raw: String,
    /// "UNKNOWN" when the host sends none.
    pub hostname: String,
    pub unique_id: String,
    pub app_version: Option<String>,
    pub gfe_version: Option<String>,
    pub gpu_type: Option<String>,
    pub https_port: u16,
    /// Sunshine's dynamic HTTP WAN port; the queried port when absent.
    pub external_port: u16,
    pub mac_address: Option<String>,
    pub local_ip: Option<String>,
    pub external_ip: Option<String>,
    pub paired: bool,
    pub state: String,
    /// The host reports no session ("..._SERVER_FREE"); an unknown state is not idle.
    pub session_idle: bool,
    /// Zero unless the host is busy (GFE keeps `currentgame` set after a game ends).
    pub current_game: i32,
    pub current_game_uuid: Option<String>,
    pub server_codec_mode_support: u32,
    pub max_luma_pixels_h264: u64,
    pub max_luma_pixels_hevc: u64,
    /// Apollo-family client permissions; None when the host reports none.
    pub permission: Option<u32>,
    /// Server command names by wire index; empty names keep their index.
    pub server_commands: Vec<String>,
    pub encoder_bitrate_supported: bool,
    pub encoder_bitrate_kbps: u32,
    pub initial_encoder_bitrate_kbps: u32,
    pub frame_limiter_enabled: bool,
    pub virtual_display_frame_limiter_enabled: bool,
    pub frame_limiter_fps_limit_millihz: u32,
    pub pyrowave_host_link_mbps: u32,
    pub pyrowave_bandwidth_probe_bytes: u32,
    /// The host serves Vibeshine's UDP burst probe with handshake and wire budget.
    pub pyrowave_burst_probe: bool,
    pub pyrowave_critical_fec_percentage: u32,
    pub pyrowave_min_parity_shards: u32,
    /// The packed full chroma modes the host offers (Rubylight's `Packed444Modes`,
    /// [`crate::full_chroma`]); None when it does not say.
    pub packed444_modes: Option<u8>,
    pub capabilities: HostCapabilities,
    /// Why this device's latest stream on the host failed, in the host's words (Rubylight's
    /// `RustHostLastFailure`); `None` when it does not say.
    pub last_failure: Option<String>,
}

/// Long.parseLong without trimming, 0 when out of [0, max].
fn unsigned(value: Option<&str>, max: u64) -> u64 {
    match value.map(str::parse::<i64>) {
        Some(Ok(v)) if v >= 0 && (v as u64) <= max => v as u64,
        _ => 0,
    }
}

impl ServerInfo {
    /// Parses a reply; `http_port` is the port it was requested on.
    pub fn parse(raw: &str, http_port: u16) -> Result<Self, Error> {
        Self::parse_inner(raw, http_port, true)
    }

    /// Parses a partial `<root status_code="...">...</root>` fragment where `uniqueid`, `state`,
    /// and `PairStatus` may be omitted (used by field/capability extractors and unit tests).
    pub fn parse_partial(raw: &str, http_port: u16) -> Result<Self, Error> {
        Self::parse_inner(raw, http_port, false)
    }

    fn parse_inner(raw: &str, http_port: u16, strict: bool) -> Result<Self, Error> {
        let root = xml::parse_reply(raw)?;
        let text = |name: &str| root.find_text(name);

        let unique_id = if strict {
            root.required_text("uniqueid")?
        } else {
            text("uniqueid").unwrap_or_default()
        };
        if !unique_id.is_empty()
            && (unique_id == "." || unique_id == ".." || unique_id.contains(['/', '\\', ':']))
        {
            return Err(Error::Malformed("Invalid host uniqueid".into()));
        }
        let state = if strict {
            root.required_text("state")?
        } else {
            text("state").unwrap_or_default()
        };
        let current_game = if state.ends_with("_SERVER_BUSY") {
            root.required_text("currentgame")?
                .parse()
                .map_err(|_| Error::Malformed("Malformed current game ID".into()))?
        } else {
            0
        };
        let port = |name: &str, default: u16| -> u16 {
            text(name)
                .and_then(|p| p.parse::<i32>().ok())
                .filter(|p| (1..=65535).contains(p))
                .map_or(default, |p| p as u16)
        };
        let capabilities = HostCapabilities::from_evidence(Evidence::parse(&root));
        let (permission, server_commands, encoder, initial) = depth_two_fields(&root);
        let flag = |name: &str| text(name).as_deref() == Some("1");
        let paired = if strict {
            root.required_text("PairStatus")? == "1"
        } else {
            text("PairStatus").as_deref() == Some("1")
        };
        Ok(Self {
            hostname: text("hostname").unwrap_or_else(|| "UNKNOWN".into()),
            unique_id,
            app_version: text("appversion"),
            gfe_version: text("GfeVersion"),
            gpu_type: text("gputype"),
            https_port: port("HttpsPort", DEFAULT_HTTPS_PORT),
            external_port: port("ExternalPort", http_port),
            mac_address: text("mac"),
            local_ip: text("LocalIP"),
            external_ip: text("ExternalIP"),
            paired,
            session_idle: state.ends_with("_SERVER_FREE"),
            state,
            current_game,
            current_game_uuid: text("currentgameuuid"),
            server_codec_mode_support: unsigned(
                text("ServerCodecModeSupport").as_deref(),
                0xFFFF_FFFF,
            ) as u32,
            max_luma_pixels_h264: unsigned(text("MaxLumaPixelsH264").as_deref(), i64::MAX as u64),
            max_luma_pixels_hevc: unsigned(text("MaxLumaPixelsHEVC").as_deref(), i64::MAX as u64),
            permission,
            server_commands,
            encoder_bitrate_supported: encoder.is_some() && initial.is_some(),
            encoder_bitrate_kbps: encoder.unwrap_or(0),
            initial_encoder_bitrate_kbps: initial.unwrap_or(0),
            frame_limiter_enabled: flag("FrameLimiterEnabled"),
            virtual_display_frame_limiter_enabled: flag("VirtualDisplayFrameLimiterEnabled"),
            frame_limiter_fps_limit_millihz: unsigned(
                text("FrameLimiterFpsLimitMilliHz").as_deref(),
                i32::MAX as u64,
            ) as u32,
            pyrowave_host_link_mbps: unsigned(
                text("PyroWaveHostLinkMbps").as_deref(),
                i32::MAX as u64,
            ) as u32,
            pyrowave_bandwidth_probe_bytes: unsigned(
                text("PyroWaveBandwidthProbeBytes").as_deref(),
                PYROWAVE_BANDWIDTH_PROBE_BYTES as u64,
            ) as u32,
            pyrowave_burst_probe: flag("PyroWaveUdpProbeVersion")
                && flag("PyroWaveUdpHandshakeVersion")
                && flag("PyroWaveUdpProbeBurstVersion")
                && flag("PyroWaveWireBudgetVersion"),
            pyrowave_critical_fec_percentage: unsigned(
                text("PyroWaveCriticalFecPercentage").as_deref(),
                100,
            ) as u32,
            pyrowave_min_parity_shards: unsigned(text("PyroWaveMinParityShards").as_deref(), 255)
                as u32,
            packed444_modes: crate::full_chroma::read_serverinfo_modes(
                text("Packed444Modes").as_deref(),
            ),
            capabilities,
            last_failure: text("RustHostLastFailure")
                .map(|reason| reason.trim().to_owned())
                .filter(|reason| !reason.is_empty()),
            raw: raw.to_owned(),
        })
    }

    /// The `appversion` quad, strict as NvHTTP.getServerAppVersionQuad: four integers, only
    /// the last may be -1 (Sunshine and its forks).
    pub fn version_quad(&self) -> Result<[i32; 4], Error> {
        let version = self
            .app_version
            .as_deref()
            .ok_or_else(|| Error::Malformed("Missing server version field".into()))?;
        parse_version_quad(version)
    }

    pub fn major_version(&self) -> Result<i32, Error> {
        Ok(self.version_quad()?[0])
    }

    /// NVIDIA GameStream rather than Sunshine or a fork.
    pub fn nvidia_server(&self) -> bool {
        self.capabilities.family() == Family::GameStream
    }

    /// GFE 3.x and every third-party host allow 4K; GfeVersion is missing on very old GFE.
    pub fn supports_4k(&self) -> bool {
        self.gfe_version
            .as_deref()
            .is_some_and(|v| !v.starts_with("2."))
    }

    /// The codec support to negotiate with (PyroWave bits only from a PyroWave host).
    pub fn trusted_codec_mode_support(&self) -> u32 {
        self.capabilities
            .trusted_codec_mode_support(self.server_codec_mode_support)
    }

    /// Constructs a synthetic `ServerInfo` from known fields without XML round-tripping.
    pub fn synthetic(
        paired: bool,
        current_game: i32,
        current_game_uuid: &str,
        session_idle: bool,
        has_bandwidth_probe: bool,
        host_confirms_replacement: bool,
        permission: Option<u32>,
    ) -> Self {
        let state = if session_idle {
            "SUNSHINE_SERVER_FREE"
        } else {
            "SUNSHINE_SERVER_BUSY"
        };
        let evidence = Evidence {
            paired,
            rust_host_version: host_confirms_replacement.then(|| "3.0.0".to_owned()),
            permission: permission.map_or(-1, i64::from),
            hardware_metrics_version: -1,
            network_path_version: -1,
            cursor_shape_version: -1,
            frame_warp_version: -1,
            frame_warp_protocol: -1,
            native_api_version: -1,
            power_action_version: -1,
            hdr_formats: -1,
            fec_control_version: -1,
            bandwidth_probe_version: if has_bandwidth_probe { 1 } else { -1 },
            audio_codecs: -1,
            audio_layouts: -1,
            text_input_version: -1,
            keyboard_layout_version: -1,
            pen_barrel_roll_version: -1,
            controller_identity_version: -1,
            pipeline_rates_version: -1,
            display_scale_version: -1,
            second_screen_version: -1,
            internet_play_version: -1,
            current_game_uuid: true,
            ..Default::default()
        };
        Self {
            raw: String::new(),
            hostname: "UNKNOWN".to_owned(),
            unique_id: String::new(),
            app_version: None,
            gfe_version: None,
            gpu_type: None,
            https_port: DEFAULT_HTTPS_PORT,
            external_port: super::DEFAULT_HTTP_PORT,
            mac_address: None,
            local_ip: None,
            external_ip: None,
            paired,
            state: state.to_owned(),
            session_idle,
            current_game: if session_idle { 0 } else { current_game },
            current_game_uuid: (!current_game_uuid.is_empty())
                .then(|| current_game_uuid.to_owned()),
            server_codec_mode_support: 0,
            max_luma_pixels_h264: 0,
            max_luma_pixels_hevc: 0,
            permission,
            server_commands: Vec::new(),
            encoder_bitrate_supported: false,
            encoder_bitrate_kbps: 0,
            initial_encoder_bitrate_kbps: 0,
            frame_limiter_enabled: false,
            virtual_display_frame_limiter_enabled: false,
            frame_limiter_fps_limit_millihz: 0,
            pyrowave_host_link_mbps: 0,
            pyrowave_bandwidth_probe_bytes: 0,
            pyrowave_burst_probe: false,
            pyrowave_critical_fec_percentage: 0,
            pyrowave_min_parity_shards: 0,
            packed444_modes: None,
            capabilities: HostCapabilities::from_evidence(evidence),
            last_failure: None,
        }
    }
}

pub fn parse_version_quad(version: &str) -> Result<[i32; 4], Error> {
    let malformed = || Error::Malformed(format!("Malformed server version field: {version}"));
    let parts: Vec<&str> = version.split('.').collect();
    if parts.len() != 4 {
        return Err(malformed());
    }
    let mut quad = [0; 4];
    for (i, part) in parts.iter().enumerate() {
        let value: i32 = part.parse().map_err(|_| malformed())?;
        if value < 0 && !(i == 3 && value == -1) {
            return Err(malformed());
        }
        quad[i] = value;
    }
    Ok(quad)
}

/// Permission, ServerCommand and the Rubylight encoder rates, read only as direct children.
fn depth_two_fields(root: &Element) -> (Option<u32>, Vec<String>, Option<u32>, Option<u32>) {
    let mut permission = None;
    let mut commands = Vec::new();
    let mut encoder = None;
    let mut initial = None;
    let bitrate = |text: String| unsigned(Some(text.trim()), 2_000_000) as u32;
    for field in root.elements() {
        match field.name.as_str() {
            "Permission" => {
                permission = Some(unsigned(Some(field.text().trim()), 0xFFFF_FFFF) as u32)
            }
            "RustHostEncoderBitrateKbps" => encoder = Some(bitrate(field.text())),
            "RustHostInitialEncoderBitrateKbps" => initial = Some(bitrate(field.text())),
            "ServerCommand" => commands.push(field.text()),
            _ => {}
        }
    }
    (permission, commands, encoder, initial)
}

#[cfg(test)]
pub mod tests;
