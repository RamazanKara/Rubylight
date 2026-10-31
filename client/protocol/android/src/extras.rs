//! Platform-free pieces of the bridge: value conversions between the Java API (which mirrors
//! moonlight-common-c's) and nightjar-core, and Nightjar's per-stream extras that lived in
//! `android_control.c` and `android_microphone.c`. Unit-tested on the host.

use std::time::Duration;

use nightjar_core::control::{ClientMessage, CursorShape, HdrMetadata};
use nightjar_core::input::queue::InputError;
use nightjar_core::sdp::AudioConfiguration;
use nightjar_core::session::{Location, RoamingConfig};
use rubylight_protocol::control::DisplayCaps;
use rubylight_protocol::keyboard_layout::{KeyboardLayout, LayoutSource};
use rubylight_protocol::phase_lock::{self, SlackWindow};
use rubylight_protocol::present_clock::{Grid, GridSchedule};
use rubylight_protocol::rate_change::RateChangeRequest;
use rubylight_protocol::text_input::TextInputRequest;

/// `LI_ERR_UNSUPPORTED`: the Java side falls back (touch to mouse, for example).
pub const LI_ERR_UNSUPPORTED: i32 = -5501;

/// The C API's return codes for input calls.
pub fn input_result(result: Result<(), InputError>) -> i32 {
    match result {
        Ok(()) => 0,
        Err(InputError::Stopped) => -2,
        Err(InputError::Invalid) => -3,
        Err(InputError::Unsupported) => LI_ERR_UNSUPPORTED,
        Err(InputError::QueueFull) => -1,
    }
}

/// `MAKE_AUDIO_CONFIGURATION`.
pub fn audio_configuration_to_int(config: AudioConfiguration) -> i32 {
    (i32::from(config.channel_mask) << 16) | (i32::from(config.channel_count) << 8) | 0xCA
}

/// The inverse, checking the magic byte.
pub fn audio_configuration_from_int(value: i32) -> Option<AudioConfiguration> {
    (value & 0xFF == 0xCA).then_some(AudioConfiguration {
        channel_count: ((value >> 8) & 0xFF) as u8,
        channel_mask: ((value >> 16) & 0xFFFF) as u16,
    })
}

/// The passthrough codecs Java found the audio route plays, as `audio_ext::CODEC_*` bits: PCM,
/// AC3 and E-AC3 only, so a stray bit never asks the host for Opus as passthrough.
pub fn audio_passthrough(codecs: i32) -> u8 {
    use rubylight_protocol::audio_ext::{CODEC_AC3, CODEC_EAC3, CODEC_PCM};
    (codecs & 0xFF) as u8 & (CODEC_PCM | CODEC_AC3 | CODEC_EAC3)
}

/// `FecChoice.summaryPercentage()` back as the choice: -2 (or less) the host's own, -1 automatic,
/// else a fixed share, at most 100 as Foundation Sunshine takes it.
pub fn video_fec(percentage: i32) -> rubylight_protocol::fec_report::FecChoice {
    use rubylight_protocol::fec_report::FecChoice;
    match percentage {
        ..=-2 => FecChoice::Host,
        -1 => FecChoice::Auto,
        _ => FecChoice::Fixed(percentage.min(100) as u8),
    }
}

/// `STREAM_CFG_LOCAL`, `STREAM_CFG_REMOTE`, `STREAM_CFG_AUTO`.
pub fn location(streaming_remotely: i32) -> Location {
    match streaming_remotely {
        0 => Location::Local,
        1 => Location::Remote,
        _ => Location::Auto,
    }
}

/// "Client roaming (Experimental)" and "Continue on mobile data". The app reports every
/// default-network change and whether it is metered, so media never moves onto a network
/// it has not reported.
pub fn roaming_config(enabled: bool, metered: bool) -> RoamingConfig {
    RoamingConfig {
        enabled,
        metered,
        platform_hints: true,
    }
}

/// `SS_HDR_METADATA` as the C struct lays it out (13 little-endian words).
pub fn hdr_metadata_bytes(m: &HdrMetadata) -> [u8; 26] {
    let words = [
        m.display_primaries[0].0,
        m.display_primaries[0].1,
        m.display_primaries[1].0,
        m.display_primaries[1].1,
        m.display_primaries[2].0,
        m.display_primaries[2].1,
        m.white_point.0,
        m.white_point.1,
        m.max_display_luminance,
        m.min_display_luminance,
        m.max_content_light_level,
        m.max_frame_average_light_level,
        m.max_full_frame_luminance,
    ];
    let mut out = [0; 26];
    for (i, word) in words.iter().enumerate() {
        out[i * 2..i * 2 + 2].copy_from_slice(&word.to_le_bytes());
    }
    out
}

/// The 0x5541 message for a decoded shape, as the Java side parses it
/// (docs/protocol/cursor-shape.md).
pub fn cursor_shape_message(shape: &CursorShape) -> Vec<u8> {
    let header = |kind: u8, id: u32| {
        let mut out = vec![1, kind, 0, 0];
        out.extend_from_slice(&id.to_le_bytes());
        out
    };
    match shape {
        CursorShape::Hide => header(0, 0),
        CursorShape::Default => header(1, 0),
        CursorShape::Show { id } => header(2, *id),
        CursorShape::Image {
            id,
            width,
            height,
            hotspot_x,
            hotspot_y,
            scale_percent,
            rgba,
        } => {
            let mut out = header(3, *id);
            for word in [*width, *height, *hotspot_x, *hotspot_y] {
                out.extend_from_slice(&word.to_le_bytes());
            }
            out.extend_from_slice(&[1, 0]);
            out.extend_from_slice(&scale_percent.to_le_bytes());
            out.extend_from_slice(rgba);
            out
        }
    }
}

/// `getEstimatedRttInfo`: milliseconds in the top 32 bits, variance in the bottom, -1 without.
pub fn rtt_info(rtt: Option<(Duration, Duration)>) -> i64 {
    match rtt {
        Some((rtt, variance)) => {
            let ms = |d: Duration| (d.as_millis().min(u128::from(u32::MAX))) as u64;
            ((ms(rtt) << 32) | ms(variance)) as i64
        }
        None => -1,
    }
}

/// `rp_display_caps_encode`'s scaling: nits to the wire units, 0 for unknown.
pub fn display_caps(hdr: bool, max_nits: f32, max_average_nits: f32, min_nits: f32) -> DisplayCaps {
    let scale = |nits: f32, per_nit: f32| {
        if nits.is_finite() && nits > 0.0 {
            (nits * per_nit + 0.5).min(u32::MAX as f32) as u32
        } else {
            0
        }
    };
    DisplayCaps {
        hdr,
        max_centinits: scale(max_nits, 100.0),
        max_average_centinits: scale(max_average_nits, 100.0),
        min_decimillinits: scale(min_nits, 10_000.0),
    }
}

/// `sendControllerAudio`: a DualSense microphone block (0x5534) or an endpoint status
/// (0x5535), with the checks `android_control.c` makes.
pub fn controller_audio_message(kind: i32, payload: &[u8]) -> Option<ClientMessage> {
    if !(8..=nightjar_core::control::haptics::MAX_PAYLOAD).contains(&payload.len()) {
        return None;
    }
    let message = match kind {
        0x5534 => ClientMessage::ControllerMicrophone(payload.to_vec()),
        0x5535 => {
            if payload.len() != 8 || payload[..2] != [1, 1] || payload[5..] != [0, 0, 0] {
                return None;
            }
            ClientMessage::ControllerAudioStatus {
                controller: u16::from_le_bytes([payload[2], payload[3]]),
                flags: payload[4],
            }
        }
        _ => return None,
    };
    message.is_valid().then_some(message)
}

/// `sendTextInputRequest`: raise and lower notices (0x553B) on or off, with the focused
/// element's id from a host that sends it (`TextInputVersion` 2; an older one ignores the ask),
/// never the surrounding text, which Nightjar does not show.
pub fn text_input_request(enabled: bool) -> ClientMessage {
    ClientMessage::TextInputRequest(TextInputRequest {
        notices: enabled,
        surrounding_text: false,
        element_ids: enabled,
        max_text: 0,
    })
}

/// `sendRateChange`: a bitrate in kbps, a rate in mHz or both (0x553C), 0 keeping that value;
/// `None` for a negative value or a request the host would refuse as invalid.
pub fn rate_change_message(
    serial: i32,
    bitrate_kbps: i32,
    fps_millihz: i32,
) -> Option<ClientMessage> {
    let message = ClientMessage::RateChange(RateChangeRequest {
        serial: serial as u32,
        bitrate_kbps: u32::try_from(bitrate_kbps).ok()?,
        fps_millihz: u32::try_from(fps_millihz).ok()?,
    });
    message.is_valid().then_some(message)
}

/// `sendKeyboardLayout`: the device's BCP-47 layout tag (0x553E), or `None` for an empty tag
/// or one the wire cannot carry.
pub fn keyboard_layout_message(tag: &str) -> Option<ClientMessage> {
    if tag.is_empty() {
        return None;
    }
    let message = ClientMessage::KeyboardLayout(KeyboardLayout {
        source: LayoutSource::Bcp47,
        klid: 0,
        tag: tag.to_owned(),
    });
    message.is_valid().then_some(message)
}

/// The RTP timestamp back from moonlight-common-c's `presentationTimeUs`
/// (`floor(rtp * 1000 / 90)`), as `android_present_clock.c` recovers it.
pub fn rtp_from_presentation_us(us: u64) -> u32 {
    (us * 9).div_ceil(100) as u32
}

/// The vsync grid of `presentClockScheduleOnGrid` in microseconds, from `System.nanoTime()`
/// values (as `android_present_clock.c`).
pub fn grid_from_ns(anchor_ns: i64, interval_ns: i64, lead_ns: i64, slack_ns: i64) -> Grid {
    Grid {
        anchor_us: anchor_ns.div_euclid(1000),
        interval_us: (interval_ns + 500).div_euclid(1000),
        lead_us: lead_ns.div_euclid(1000),
        slack_us: slack_ns.div_euclid(1000),
    }
}

/// `presentClockScheduleOnGrid`'s out array: release time (ns), vsync (ns), vsync index, flags
/// (1 late, 2 shares the previous frame's vsync), hold (us), host-to-vsync latency (us, -1 without
/// clock sync) and how late the frame was (us). A frame that is not held keeps the ready time's
/// sub-microsecond part, so it is not released early.
pub fn grid_schedule_to_ns(
    s: &GridSchedule,
    ready_ns: i64,
    anchor_ns: i64,
    interval_ns: i64,
    lead_ns: i64,
) -> [i64; 7] {
    let vsync_ns = anchor_ns + s.vsync_index * interval_ns;
    let release_ns = if s.hold_us == 0 { ready_ns } else { (vsync_ns - lead_ns).max(ready_ns) };
    [
        release_ns,
        vsync_ns,
        s.vsync_index,
        i64::from(s.late) | (i64::from(s.shares_vsync) << 1),
        s.hold_us,
        s.latency_us.unwrap_or(-1),
        s.late_by_us,
    ]
}

/// Phase lock reports (`android_control.c`): frame slack is collected on the render path;
/// every half second a report is made from at least ten frames, and the background sender
/// sends the newest one if it is at most a second old.
pub struct PhaseLockReports {
    window: SlackWindow,
    last_report_ms: u64,
    pending: Option<(phase_lock::Report, u64)>,
    lead_us: i32,
}

pub const PHASE_LOCK_REPORT_INTERVAL_MS: u64 = 500;
const PHASE_LOCK_MIN_FRAMES: usize = 10;
const PHASE_LOCK_MAX_REPORT_AGE_MS: u64 = 1000;

impl Default for PhaseLockReports {
    fn default() -> Self {
        Self {
            window: SlackWindow::default(),
            last_report_ms: 0,
            pending: None,
            lead_us: i32::MIN,
        }
    }
}

impl PhaseLockReports {
    /// Adds a frame; true when a new report is ready for [`Self::take`].
    pub fn push(&mut self, slack_ns: i64, period_ns: i64, now_ms: u64) -> bool {
        if self.last_report_ms == 0 {
            self.last_report_ms = now_ms;
        }
        self.window.push(slack_ns, period_ns);
        if now_ms - self.last_report_ms < PHASE_LOCK_REPORT_INTERVAL_MS {
            return false;
        }
        let elapsed_ns = now_ms.saturating_sub(self.last_report_ms).min(u32::MAX as u64) as i64 * 1_000_000;
        self.last_report_ms = now_ms;
        match self.window.report_over(PHASE_LOCK_MIN_FRAMES, elapsed_ns) {
            Some(report) => {
                self.lead_us = report.lead_ns / 1000;
                self.pending = Some((report, now_ms));
                true
            }
            None => false,
        }
    }

    /// The pending report if it is fresh; the mailbox is emptied either way.
    pub fn take(&mut self, now_ms: u64) -> Option<phase_lock::Report> {
        let (report, at) = self.pending.take()?;
        (now_ms >= at && now_ms - at <= PHASE_LOCK_MAX_REPORT_AGE_MS).then_some(report)
    }

    /// The early edge of the last report in microseconds, `i32::MIN` before the first.
    pub fn lead_us(&self) -> i32 {
        self.lead_us
    }
}

/// Opus frame sizes at 48 kHz from 2.5 to 60 ms, the ones `sendMicrophonePcm` takes.
pub fn valid_microphone_frame(samples: usize) -> bool {
    matches!(samples, 120 | 240 | 480 | 960 | 1920 | 2880)
}

/// A longer gap between microphone frames (mute, a stall) restarts the encoder.
pub const MIC_RESET_GAP_MS: u64 = 200;

/// Whether to encrypt every stream: only with hardware AES and more than two cores, as
/// `callbacks.c` decides from cpufeatures.
pub fn has_fast_aes() -> bool {
    if std::thread::available_parallelism().map_or(1, |n| n.get()) <= 2 {
        return false;
    }
    cpu_has_aes()
}

#[cfg(target_arch = "aarch64")]
fn cpu_has_aes() -> bool {
    std::arch::is_aarch64_feature_detected!("aes")
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
fn cpu_has_aes() -> bool {
    std::arch::is_x86_feature_detected!("aes")
}

#[cfg(all(target_arch = "arm", target_os = "android"))]
fn cpu_has_aes() -> bool {
    const AT_HWCAP2: libc::c_ulong = 26;
    const HWCAP2_AES: libc::c_ulong = 1;
    unsafe { libc::getauxval(AT_HWCAP2) & HWCAP2_AES != 0 }
}

#[cfg(not(any(
    target_arch = "aarch64",
    target_arch = "x86",
    target_arch = "x86_64",
    all(target_arch = "arm", target_os = "android")
)))]
fn cpu_has_aes() -> bool {
    // Assume new architectures have crypto acceleration, as callbacks.c does.
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passthrough_keeps_only_the_codecs_java_can_route() {
        use rubylight_protocol::audio_ext::{CODEC_AC3, CODEC_EAC3, CODEC_OPUS, CODEC_PCM};
        assert_eq!(audio_passthrough(0), 0);
        assert_eq!(audio_passthrough(i32::from(CODEC_AC3 | CODEC_EAC3)), CODEC_AC3 | CODEC_EAC3);
        assert_eq!(audio_passthrough(i32::from(CODEC_OPUS | CODEC_PCM)), CODEC_PCM);
        assert_eq!(audio_passthrough(-1), CODEC_PCM | CODEC_AC3 | CODEC_EAC3);
        assert_eq!(audio_passthrough(0x100), 0);
    }

    #[test]
    fn the_video_fec_choice_comes_back_from_its_summary_percentage() {
        use rubylight_protocol::fec_report::FecChoice;
        for choice in [FecChoice::Host, FecChoice::Auto, FecChoice::Fixed(0), FecChoice::Fixed(30), FecChoice::Fixed(100)] {
            assert_eq!(video_fec(choice.summary_percentage()), choice);
        }
        assert_eq!(video_fec(i32::MIN), FecChoice::Host);
        assert_eq!(video_fec(255), FecChoice::Fixed(100));
    }

    #[test]
    fn the_roaming_settings_reach_the_session_with_platform_hints() {
        for (enabled, metered) in [(true, false), (true, true), (false, false), (false, true)] {
            assert_eq!(
                roaming_config(enabled, metered),
                RoamingConfig {
                    enabled,
                    metered,
                    platform_hints: true,
                }
            );
        }
    }

    #[test]
    fn text_input_and_layout_messages_reach_the_wire() {
        let on = text_input_request(true).payload();
        assert_eq!(
            TextInputRequest::decode(&on),
            Some(TextInputRequest {
                notices: true,
                surrounding_text: false,
                element_ids: true,
                max_text: 0,
            })
        );
        assert!(
            !TextInputRequest::decode(&text_input_request(false).payload())
                .unwrap()
                .notices
        );

        let german = keyboard_layout_message("de-DE").unwrap();
        assert_eq!(
            KeyboardLayout::decode(&german.payload()),
            Some(KeyboardLayout {
                source: LayoutSource::Bcp47,
                klid: 0,
                tag: "de-DE".into(),
            })
        );
        // Nothing to report, or a tag the wire refuses, sends nothing.
        assert_eq!(keyboard_layout_message(""), None);
        assert_eq!(keyboard_layout_message(&"a".repeat(65)), None);
        assert_eq!(keyboard_layout_message("de DE"), None);
    }

    #[test]
    fn rate_changes_go_out_as_asked_and_refuse_what_the_host_would() {
        let both = rate_change_message(-2, 40_000, 120_000).unwrap();
        assert_eq!(
            RateChangeRequest::decode(&both.payload()),
            Some(RateChangeRequest {
                serial: u32::MAX - 1,
                bitrate_kbps: 40_000,
                fps_millihz: 120_000,
            })
        );
        assert!(rate_change_message(1, 40_000, 0).is_some());
        assert!(rate_change_message(1, 0, 60_000).is_some());
        assert_eq!(rate_change_message(1, 0, 0), None);
        assert_eq!(rate_change_message(1, -1, 60_000), None);
        assert_eq!(rate_change_message(1, 40_000, -1), None);
        assert_eq!(rate_change_message(1, 40_000, 9_999), None);
        assert_eq!(rate_change_message(1, 0, 500_001), None);
    }

    #[test]
    fn audio_configurations_round_trip() {
        assert_eq!(
            audio_configuration_to_int(AudioConfiguration::STEREO),
            0x0003_02CA
        );
        assert_eq!(
            audio_configuration_to_int(AudioConfiguration::SURROUND_71),
            0x063F_08CA
        );
        assert_eq!(
            audio_configuration_from_int(0x003F_06CA),
            Some(AudioConfiguration::SURROUND_51)
        );
        assert_eq!(audio_configuration_from_int(0x0003_0200), None);
        // 7.1.4's mask sets the sign bit, as Java's MoonBridge.AUDIO_CONFIGURATION_714_SURROUND.
        let seven_one_four = audio_configuration_to_int(AudioConfiguration::SURROUND_714);
        assert_eq!(seven_one_four as u32, 0xF63F_0CCA);
        assert_eq!(
            audio_configuration_from_int(seven_one_four),
            Some(AudioConfiguration::SURROUND_714)
        );
    }

    #[test]
    fn input_errors_map_to_the_c_codes() {
        assert_eq!(input_result(Ok(())), 0);
        assert_eq!(input_result(Err(InputError::Unsupported)), -5501);
        assert_eq!(input_result(Err(InputError::Stopped)), -2);
        assert_eq!(input_result(Err(InputError::Invalid)), -3);
    }

    #[test]
    fn hdr_metadata_has_the_c_layout() {
        let m = HdrMetadata {
            display_primaries: [(1, 2), (3, 4), (5, 6)],
            white_point: (7, 8),
            max_display_luminance: 9,
            min_display_luminance: 10,
            max_content_light_level: 11,
            max_frame_average_light_level: 12,
            max_full_frame_luminance: 0x0d0e,
        };
        let bytes = hdr_metadata_bytes(&m);
        assert_eq!(&bytes[..4], &[1, 0, 2, 0]);
        assert_eq!(&bytes[12..16], &[7, 0, 8, 0]);
        assert_eq!(&bytes[24..], &[0x0e, 0x0d]);
    }

    #[test]
    fn cursor_shapes_encode_back_to_the_wire_form() {
        let shapes = [
            CursorShape::Hide,
            CursorShape::Default,
            CursorShape::Show { id: 7 },
            CursorShape::Image {
                id: 9,
                width: 2,
                height: 1,
                hotspot_x: 1,
                hotspot_y: 0,
                scale_percent: 150,
                rgba: vec![0xab; 8],
            },
        ];
        for shape in shapes {
            let message = cursor_shape_message(&shape);
            assert_eq!(CursorShape::decode(&message), Ok(shape));
        }
    }

    #[test]
    fn rtt_info_packs_like_the_c() {
        assert_eq!(rtt_info(None), -1);
        let packed = rtt_info(Some((Duration::from_millis(12), Duration::from_millis(3))));
        assert_eq!(packed, (12 << 32) | 3);
    }

    #[test]
    fn display_caps_scale_like_the_ffi() {
        let caps = display_caps(true, 1000.0, 400.0, 0.0005);
        assert_eq!(
            (
                caps.hdr,
                caps.max_centinits,
                caps.max_average_centinits,
                caps.min_decimillinits
            ),
            (true, 100_000, 40_000, 5)
        );
        assert_eq!(display_caps(false, f32::NAN, -1.0, 0.0).max_centinits, 0);
    }

    #[test]
    fn controller_audio_is_checked() {
        assert_eq!(
            controller_audio_message(0x5535, &[1, 1, 3, 0, 5, 0, 0, 0]),
            Some(ClientMessage::ControllerAudioStatus {
                controller: 3,
                flags: 5
            })
        );
        assert!(controller_audio_message(0x5535, &[1, 1, 16, 0, 5, 0, 0, 0]).is_none());
        assert!(controller_audio_message(0x5535, &[1, 1, 3, 0, 5, 0, 0, 1]).is_none());
        let pcm = nightjar_core::control::haptics::encode(2, 1, &[0; 8]).unwrap();
        assert!(controller_audio_message(0x5534, &pcm).is_some());
        assert!(controller_audio_message(0x5534, &pcm[..pcm.len() - 1]).is_none());
        assert!(controller_audio_message(0x5533, &pcm).is_none());
    }

    #[test]
    fn rtp_timestamps_survive_the_presentation_time() {
        for rtp in [0u32, 1, 89, 90, 12_345_678, u32::MAX] {
            let us = u64::from(rtp) * 1000 / 90;
            assert_eq!(rtp_from_presentation_us(us), rtp);
        }
    }

    #[test]
    fn phase_lock_reports_every_half_second() {
        let mut reports = PhaseLockReports::default();
        assert_eq!(reports.lead_us(), i32::MIN);
        let mut ready = false;
        for frame in 0..60u64 {
            ready |= reports.push(2_000_000, 16_666_667, 1000 + frame * 10);
        }
        assert!(ready);
        assert!(reports.lead_us() != i32::MIN);
        assert!(reports.take(1600).is_some());
        assert!(reports.take(1600).is_none());
        // Too few frames in a window make no report.
        let mut sparse = PhaseLockReports::default();
        assert!(!sparse.push(1, 16_666_667, 1));
        assert!(!sparse.push(1, 16_666_667, 600));
        // A stale report is dropped.
        let mut stale = PhaseLockReports::default();
        for frame in 0..60u64 {
            stale.push(2_000_000, 16_666_667, 1 + frame * 10);
        }
        assert!(stale.take(5000).is_none());
    }

    #[test]
    fn phase_lock_asks_for_more_room_at_one_frame_per_refresh() {
        // 60 fps on a 60 Hz screen: half a second holds 30 frames, one per refresh.
        let lead = |frame_ms: u64, period_ns: i64| {
            let mut reports = PhaseLockReports::default();
            let mut frame = 0;
            while !reports.push(2_000_000 + (frame % 10) as i64 * 400_000, period_ns, 1000 + frame * frame_ms) {
                frame += 1;
            }
            reports.lead_us()
        };
        let sixty = lead(16, 16_666_667);
        // The same slack, one frame per two refreshes of a 120 Hz screen: the plain edge.
        let half = lead(16, 8_333_333);
        assert_eq!(half, 2_000, "the 10th percentile");
        assert!(sixty < half, "{sixty} us at 60 Hz, {half} us at 120 Hz");
    }

    #[test]
    fn microphone_frames_are_opus_sizes() {
        assert!(valid_microphone_frame(960));
        assert!(!valid_microphone_frame(1000));
    }

    #[test]
    fn the_grid_is_converted_from_nanoseconds() {
        let grid = grid_from_ns(5_000_123_456, 16_666_667, 4_000_000, 8_333_333);
        assert_eq!(grid.anchor_us, 5_000_123);
        assert_eq!(grid.interval_us, 16_667, "rounded to the nearest microsecond");
        assert_eq!(grid.lead_us, 4_000);
        assert_eq!(grid.slack_us, 8_333);
        assert_eq!(grid_from_ns(0, 8_333_333, 0, 0).interval_us, 8_333);
        assert_eq!(grid_from_ns(0, 8_333_500, 0, 0).interval_us, 8_334);
    }

    #[test]
    fn a_grid_schedule_is_reported_in_nanoseconds() {
        use rubylight_protocol::present_clock::PresentClock;
        let mut clock = PresentClock::default();
        let (anchor_ns, interval_ns, lead_ns) = (10_000_000_000i64, 16_666_667i64, 4_000_000i64);
        let grid = grid_from_ns(anchor_ns, interval_ns, lead_ns, 0);
        // Ready 8 ms after the anchor's vsync: the next vsync is 8.67 ms away, release 4.67 ms
        // after ready.
        let ready_ns = anchor_ns + 8_000_000;
        let s = clock.schedule_on_grid(0, (ready_ns / 1000) as u64, grid);
        let out = grid_schedule_to_ns(&s, ready_ns, anchor_ns, interval_ns, lead_ns);
        assert_eq!(out[1], anchor_ns + interval_ns, "the next vsync");
        assert_eq!(out[2], 1);
        assert_eq!(out[0], out[1] - lead_ns, "released a lead before it");
        assert_eq!(out[3], 0);
        assert!((out[4] - (out[0] - ready_ns) / 1000).abs() <= 1, "the hold, to rounding");
        assert_eq!(out[5], -1, "no clock sync");
        assert_eq!(out[6], 0);
        // Ready 2 ms before the next vsync, inside the lead: released at the ready time, to the
        // nanosecond.
        let ready_ns = anchor_ns + 2 * interval_ns - 2_000_123;
        let grid = grid_from_ns(anchor_ns + interval_ns, interval_ns, lead_ns, 0);
        let s = clock.schedule_on_grid(1_500, (ready_ns / 1000) as u64, grid);
        let out = grid_schedule_to_ns(&s, ready_ns, anchor_ns + interval_ns, interval_ns, lead_ns);
        assert_eq!(out[0], ready_ns);
        assert_eq!(out[4], 0);
    }

    #[test]
    fn grid_flags_carry_late_and_shared() {
        let s = GridSchedule {
            release_us: 5_000,
            vsync_us: 9_000,
            vsync_index: 2,
            late: true,
            late_by_us: 700,
            shares_vsync: true,
            hold_us: 100,
            latency_us: Some(33),
        };
        let out = grid_schedule_to_ns(&s, 4_900_000, 0, 4_500_000, 4_000_000);
        assert_eq!(out, [5_000_000, 9_000_000, 2, 3, 100, 33, 700]);
    }
}
