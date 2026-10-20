//! The host's `/serverinfo` reply, read as NvHTTP.getComputerDetails does, including the
//! Sunshine, Apollo, Vibepollo and Rubylight extensions.

use super::capabilities::{Evidence, Family, HostCapabilities, PYROWAVE_BANDWIDTH_PROBE_BYTES};
use super::xml::{self, Element};
use super::{Error, DEFAULT_HTTPS_PORT};

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
    pub capabilities: HostCapabilities,
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
        let root = xml::parse_reply(raw)?;
        let text = |name: &str| root.find_text(name);

        let unique_id = root.required_text("uniqueid")?;
        if unique_id == "." || unique_id == ".." || unique_id.contains(['/', '\\', ':']) {
            return Err(Error::Malformed("Invalid host uniqueid".into()));
        }
        let state = root.required_text("state")?;
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
            paired: root.required_text("PairStatus")? == "1",
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
            capabilities,
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
}

pub(crate) fn parse_version_quad(version: &str) -> Result<[i32; 4], Error> {
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
pub(crate) mod tests {
    use super::super::capabilities::{Availability, Feature, Reason};
    use super::*;

    /// A paired Rubylight reply, field for field as rust/host/src/nvhttp.rs writes it.
    pub(crate) const RUBYLIGHT: &str = "<?xml version=\"1.0\" encoding=\"utf-8\"?><root status_code=\"200\">\
        <NetworkPathVersion>1</NetworkPathVersion><HardwareMetricsVersion>0</HardwareMetricsVersion>\
        <hostname>Gaming PC</hostname><appversion>7.1.431.-1</appversion><GfeVersion>3.23.0.74</GfeVersion>\
        <uniqueid>0123-ABCD</uniqueid><HttpsPort>47984</HttpsPort><ExternalPort>47989</ExternalPort>\
        <PairStatus>1</PairStatus><currentgame>5</currentgame><currentgameuuid>uuid-5</currentgameuuid>\
        <state>SUNSHINE_SERVER_BUSY</state><LocalIP>192.168.1.20</LocalIP><mac>AA:BB:CC:DD:EE:FF</mac>\
        <MaxLumaPixelsHEVC>1869449984</MaxLumaPixelsHEVC><ServerCodecModeSupport>8457985</ServerCodecModeSupport>\
        <RustHostVersion>2.2.21</RustHostVersion><RustHostEncoderBitrateKbps>40000</RustHostEncoderBitrateKbps>\
        <RustHostInitialEncoderBitrateKbps>50000</RustHostInitialEncoderBitrateKbps>\
        <RustHostSessionCount></RustHostSessionCount><PyroWaveHostLinkMbps>2500</PyroWaveHostLinkMbps>\
        <PyroWaveBandwidthProbeBytes>33554432</PyroWaveBandwidthProbeBytes>\
        <PyroWaveUdpProbeVersion>1</PyroWaveUdpProbeVersion><PyroWaveUdpHandshakeVersion>1</PyroWaveUdpHandshakeVersion>\
        <PyroWaveUdpProbeBurstVersion>1</PyroWaveUdpProbeBurstVersion><PyroWaveWireBudgetVersion>1</PyroWaveWireBudgetVersion>\
        <PyroWaveCriticalFecPercentage>20</PyroWaveCriticalFecPercentage><PyroWaveMinParityShards>2</PyroWaveMinParityShards>\
        <Permission>119480064</Permission><FrameLimiterSupported>1</FrameLimiterSupported>\
        <FrameLimiterEnabled>1</FrameLimiterEnabled><VirtualDisplayFrameLimiterEnabled>0</VirtualDisplayFrameLimiterEnabled>\
        <FrameLimiterFpsLimitMilliHz>119880</FrameLimiterFpsLimitMilliHz><VirtualDisplayCapable>true</VirtualDisplayCapable>\
        <VirtualDisplayDriverReady>false</VirtualDisplayDriverReady><ControllerPreparation>1</ControllerPreparation>\
        <VirtualDisplayHDRCapable>true</VirtualDisplayHDRCapable><ServerCommand>Sleep</ServerCommand>\
        <ServerCommand></ServerCommand></root>";

    #[test]
    fn rubylight_reply() {
        let info = ServerInfo::parse(RUBYLIGHT, 47989).unwrap();
        assert_eq!(info.hostname, "Gaming PC");
        assert_eq!(info.unique_id, "0123-ABCD");
        assert_eq!(info.version_quad().unwrap(), [7, 1, 431, -1]);
        assert_eq!((info.https_port, info.external_port), (47984, 47989));
        assert!(info.paired && !info.session_idle);
        assert_eq!(
            (info.current_game, info.current_game_uuid.as_deref()),
            (5, Some("uuid-5"))
        );
        assert_eq!(info.mac_address.as_deref(), Some("AA:BB:CC:DD:EE:FF"));
        assert_eq!(info.server_codec_mode_support, 8_457_985);
        assert_eq!(info.permission, Some(119_480_064));
        assert_eq!(info.server_commands, ["Sleep", ""]);
        assert!(info.encoder_bitrate_supported);
        assert_eq!(
            (info.encoder_bitrate_kbps, info.initial_encoder_bitrate_kbps),
            (40_000, 50_000)
        );
        assert!(info.frame_limiter_enabled && !info.virtual_display_frame_limiter_enabled);
        assert_eq!(info.frame_limiter_fps_limit_millihz, 119_880);
        assert_eq!(info.pyrowave_host_link_mbps, 2500);
        assert!(info.pyrowave_burst_probe);
        assert_eq!(
            (
                info.pyrowave_critical_fec_percentage,
                info.pyrowave_min_parity_shards
            ),
            (20, 2)
        );
        assert!(!info.nvidia_server() && info.supports_4k());

        let caps = &info.capabilities;
        assert_eq!(caps.family(), Family::Rubylight);
        assert_eq!(caps.host_version(), Some("2.2.21"));
        for feature in [
            Feature::Permissions,
            Feature::OtpPairing,
            Feature::Clipboard,
            Feature::ServerCommands,
            Feature::AppUuids,
            Feature::LaunchByUuid,
            Feature::FractionalRefresh,
            Feature::HostVrr,
            Feature::RuntimeBitrate,
            Feature::PyroWave,
            Feature::PyroWaveRecords,
            Feature::PyroWaveBandwidthProbe,
            Feature::NetworkPath,
            Feature::FrameLimiterStatus,
            Feature::ControllerPreparation,
            Feature::RemoteSessionTiles,
            Feature::HostTiming,
            Feature::WebUi,
        ] {
            assert!(caps.has(feature), "{feature:?}");
        }
        assert_eq!(
            caps.availability(Feature::VirtualDisplay),
            Availability::Unsupported(Some(Reason::VirtualDisplayDriver))
        );
        assert_eq!(
            caps.availability(Feature::HardwareMetrics),
            Availability::Unsupported(Some(Reason::HardwareMetricsOff))
        );
        assert_eq!(
            caps.availability(Feature::CursorShape),
            Availability::Unsupported(None)
        );
        assert_eq!(info.trusted_codec_mode_support(), 8_457_985);
    }

    #[test]
    fn sunshine_and_gamestream_replies() {
        let sunshine = "<root status_code=\"200\"><hostname></hostname><appversion>7.1.431.-1</appversion>\
            <uniqueid>abc</uniqueid><HttpsPort>0</HttpsPort><PairStatus>0</PairStatus><currentgame>0</currentgame>\
            <state>SUNSHINE_SERVER_FREE</state><ServerCodecModeSupport>25167617</ServerCodecModeSupport></root>";
        let info = ServerInfo::parse(sunshine, 47989).unwrap();
        assert_eq!(info.hostname, "UNKNOWN");
        assert_eq!((info.https_port, info.external_port), (47984, 47989));
        assert!(info.session_idle && !info.paired);
        assert_eq!(info.capabilities.family(), Family::Sunshine);
        assert!(!info.capabilities.has(Feature::FractionalRefresh));
        // PyroWave bits from a host that does not show PyroWave are not trusted.
        assert_eq!(info.trusted_codec_mode_support(), 25_167_617 & !0x0780_0000);

        let gfe = "<root status_code=\"200\"><appversion>7.1.431.0</appversion><GfeVersion>2.1.0</GfeVersion>\
            <uniqueid>gfe</uniqueid><PairStatus>1</PairStatus><currentgame>9</currentgame>\
            <state>MJOLNIR_STATE_SERVER_AVAILABLE</state></root>";
        let info = ServerInfo::parse(gfe, 47989).unwrap();
        assert!(info.nvidia_server() && !info.supports_4k());
        assert_eq!(
            info.current_game, 0,
            "only a busy host reports a running game"
        );
        assert!(!info.session_idle);
        assert_eq!(
            info.capabilities.availability(Feature::WebUi),
            Availability::Unsupported(None)
        );
    }

    #[test]
    fn invalid_replies() {
        let ok = "<root status_code=\"200\"><uniqueid>x</uniqueid><PairStatus>1</PairStatus><state>S</state></root>";
        assert!(ServerInfo::parse(ok, 1).is_ok());
        for bad in [
            ok.replace("<uniqueid>x", "<uniqueid>a/b"),
            ok.replace("<state>S</state>", ""),
            ok.replace("<PairStatus>1</PairStatus>", ""),
            ok.replace("<state>S", "<state>X_SERVER_BUSY"),
        ] {
            assert!(ServerInfo::parse(&bad, 1).is_err(), "{bad}");
        }
        assert_eq!(parse_version_quad("7.1.431.-1").unwrap(), [7, 1, 431, -1]);
        assert!(parse_version_quad("7.1.431").is_err());
        assert!(parse_version_quad("7.-1.431.0").is_err());
    }
}
