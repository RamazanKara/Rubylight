//! XML serverinfo and Rubylight version parsing for capability [`Evidence`].

use super::super::xml::Element;
use super::{Evidence, PYROWAVE_BANDWIDTH_PROBE_BYTES, PYROWAVE_CODEC_BITS};

fn unsigned(value: &str, max: u64) -> u64 {
    match value.trim().parse::<i64>() {
        Ok(v) if v >= 0 && (v as u64) <= max => v as u64,
        _ => 0,
    }
}

fn flag(value: &str) -> Option<bool> {
    let value = value.trim();
    if value == "1" || value.eq_ignore_ascii_case("true") {
        Some(true)
    } else if value == "0" || value.eq_ignore_ascii_case("false") {
        Some(false)
    } else {
        None
    }
}

/// `MaxVideoPacket` and `a=x-rl-max-packet-size`: the largest video packet offered, rounded down
/// to a multiple of 16 and capped at [`rubylight_protocol::video_packet::MAX_VIDEO_PACKET`], or 0
/// when it is no more than stock Moonlight's 1400 bytes.
pub(crate) fn video_packet_offer(value: &str) -> u16 {
    rubylight_protocol::video_packet::parse_offer(value)
}

/// "2.2.21", "2.0.0-rc.19" or "3.0.0-beta.1" as [major, minor, patch, pre-release], a release
/// after its pre-releases, which rank alpha, beta, rc (the stage in the millions of the last part).
pub fn parse_rubylight_version(version: &str) -> Option<[u32; 4]> {
    let mut rest = version;
    let number = |rest: &mut &str| -> Option<u32> {
        let len = rest.bytes().take_while(u8::is_ascii_digit).count();
        if len == 0 || (len > 1 && rest.starts_with('0')) {
            return None;
        }
        let value: u32 = rest[..len].parse().ok().filter(|&v| v <= 100_000)?;
        *rest = &rest[len..];
        Some(value)
    };
    let mut parts = [0, 0, 0, u32::MAX];
    for (i, part) in parts.iter_mut().take(3).enumerate() {
        *part = number(&mut rest)?;
        if i < 2 {
            rest = rest.strip_prefix('.')?;
        }
    }
    // The 3.0 hosts report "3.0.0-beta.N". The number stops at 100000, below one stage.
    const STAGE: u32 = 1_000_000;
    if let Some((stage, after)) =
        ["-alpha.", "-beta.", "-rc."]
            .iter()
            .enumerate()
            .find_map(|(stage, tag)| {
                rest.strip_prefix(tag)
                    .map(|after| (stage as u32 + 1, after))
            })
    {
        rest = after;
        parts[3] = stage * STAGE + number(&mut rest).filter(|&pre| pre > 0)?;
    }
    if let Some(build) = rest.strip_prefix('+') {
        let len = build
            .bytes()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == b'.' || *c == b'-')
            .count();
        if len == 0 {
            return None;
        }
        rest = &build[len..];
    }
    rest.is_empty().then_some(parts)
}

impl Evidence {
    pub fn parse_xml(xml: &str) -> Result<Self, super::super::Error> {
        Ok(Self::parse(&super::super::xml::parse(xml)?))
    }

    pub fn parse(root: &Element) -> Self {
        let mut e = Evidence {
            permission: -1,
            hardware_metrics_version: -1,
            network_path_version: -1,
            cursor_shape_version: -1,
            frame_warp_version: -1,
            frame_warp_protocol: -1,
            native_api_version: -1,
            power_action_version: -1,
            hdr_formats: -1,
            fec_control_version: -1,
            bandwidth_probe_version: -1,
            audio_codecs: -1,
            audio_layouts: -1,
            text_input_version: -1,
            keyboard_layout_version: -1,
            pen_barrel_roll_version: -1,
            controller_identity_version: -1,
            pipeline_rates_version: -1,
            display_scale_version: -1,
            second_screen_version: -1,
            nvidia_enhancements: 0,
            internet_play_version: -1,
            ..Default::default()
        };
        let (mut encoder, mut initial_encoder) = (false, false);
        for field in root.elements() {
            let text = field.text();
            match field.name.as_str() {
                // The MJOLNIR codename was used by GFE but never by any third-party server.
                "state" => e.game_stream = text.contains("MJOLNIR"),
                "appversion" => e.sunshine_version = text.trim().ends_with(".-1"),
                "PairStatus" => e.paired = text.trim() == "1",
                "RustHostVersion" => e.rust_host_version = Some(text.trim().to_owned()),
                "Permission" => e.permission = unsigned(&text, 0xFFFF_FFFF) as i64,
                "ServerCommand" => e.server_commands += 1,
                "VirtualDisplayCapable" => e.virtual_display_capable = flag(&text),
                "VirtualDisplayDriverReady" => e.virtual_display_driver_ready = flag(&text),
                "VirtualDisplayHDRCapable" => e.virtual_display_hdr_capable = flag(&text),
                "FrameLimiterSupported" => e.frame_limiter = Some(flag(&text) == Some(true)),
                "PyroWaveHostLinkMbps" => e.pyrowave_fields = true,
                "PyroWaveBandwidthProbeBytes" => {
                    e.pyrowave_fields = true;
                    e.pyrowave_probe_bytes =
                        unsigned(&text, PYROWAVE_BANDWIDTH_PROBE_BYTES as u64) as u32;
                }
                "ServerCodecModeSupport" => {
                    e.pyrowave_codecs = unsigned(&text, 0xFFFF_FFFF) as u32 & PYROWAVE_CODEC_BITS
                }
                "RustHostEncoderBitrateKbps" => encoder = true,
                "RustHostInitialEncoderBitrateKbps" => initial_encoder = true,
                "HardwareMetricsVersion" => {
                    e.hardware_metrics_version = i32::from(text.trim() == "1")
                }
                "NetworkPathVersion" => e.network_path_version = i32::from(text.trim() == "1"),
                "ControllerPreparation" => e.controller_preparation = text.trim() == "1",
                "CursorShapeVersion" => e.cursor_shape_version = unsigned(&text, 0xFFFF) as i32,
                "FrameWarpVersion" => e.frame_warp_version = unsigned(&text, 0xFFFF) as i32,
                "FrameWarpProtocol" => e.frame_warp_protocol = unsigned(&text, 0xFFFF) as i32,
                "HdrFormats" => e.hdr_formats = unsigned(&text, 0xFF) as i32,
                "FecControlVersion" => e.fec_control_version = unsigned(&text, 0xFFFF) as i32,
                "BandwidthProbeVersion" => {
                    e.bandwidth_probe_version = unsigned(&text, 0xFFFF) as i32
                }
                "AudioCodecs" => e.audio_codecs = unsigned(&text, 0xFF) as i32,
                "AudioLayouts" => e.audio_layouts = unsigned(&text, 0xFF) as i32,
                "TextInputVersion" => e.text_input_version = unsigned(&text, 0xFFFF) as i32,
                "KeyboardLayoutVersion" => {
                    e.keyboard_layout_version = unsigned(&text, 0xFFFF) as i32
                }
                "PenBarrelRollVersion" => {
                    e.pen_barrel_roll_version = unsigned(&text, 0xFFFF) as i32
                }
                "ControllerIdentityVersion" => {
                    e.controller_identity_version = unsigned(&text, 0xFFFF) as i32
                }
                "PipelineRatesVersion" => e.pipeline_rates_version = unsigned(&text, 0xFFFF) as i32,
                "DisplayScaleVersion" => e.display_scale_version = unsigned(&text, 0xFFFF) as i32,
                "SecondScreenVersion" => e.second_screen_version = unsigned(&text, 0xFFFF) as i32,
                "NvidiaEnhancements" => {
                    e.nvidia_enhancements =
                        rubylight_protocol::nvidia_enhance::parse_offer(Some(&text))
                }
                rubylight_protocol::hdr::DYNAMIC_SERVERINFO_FIELD => {
                    e.hdr_dynamic_control = unsigned(&text, 0xFFFF) > 0
                }
                "RustHostInternetPlayVersion" => {
                    e.internet_play_version = unsigned(&text, 0xFFFF) as i32
                }
                rubylight_protocol::video_packet::SERVERINFO_FIELD => {
                    e.max_video_packet = video_packet_offer(&text)
                }
                "NativeApiVersion" => e.native_api_version = unsigned(&text, 0xFFFF) as i32,
                "PowerActionVersion" => e.power_action_version = unsigned(&text, 0xFFFF) as i32,
                "SleepSupported" => e.sleep_supported = flag(&text) == Some(true),
                "SleepAllowed" => e.sleep_allowed = flag(&text) == Some(true),
                "currentgameuuid" => e.current_game_uuid = true,
                _ => {}
            }
        }
        e.encoder_bitrate = encoder && initial_encoder;
        if e.rust_host_version.as_deref() == Some("") {
            e.rust_host_version = None;
        }
        e
    }
}
