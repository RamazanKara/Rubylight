//! Host-to-client control messages decoded into [`ControlEvent`]s, as moonlight-common-c's
//! control receive thread and Nightjar's patches handle them.

use std::time::Instant;

use rubylight_protocol::control::{
    ClockSyncReply, HostTiming, HostTimingExtra, LinkNotice, HOST_TIMING_EXTRA_BYTES,
    HOST_TIMING_HEADER_BYTES, HOST_TIMING_MAX_RECORDS, HOST_TIMING_RECORD_BYTES,
    LINK_NOTICE_BYTES,
};
use rubylight_protocol::pipeline_rates::{PipelineRatesReport, PIPELINE_RATES_REPORT_BYTES};
use rubylight_protocol::rate_change::{RateChangeResult, RATE_CHANGE_RESULT_BYTES};
use rubylight_protocol::steam_controller::{SteamError, SteamHaptic, SteamSettings};
use rubylight_protocol::usb::UsbMessage;

use super::haptics::{self, Pcm};
use super::messages::CURSOR_SHAPE_MAX_EDGE;
use crate::roaming::RoamEvent;

/// Termination codes (`ML_ERROR_*`) besides the host's own reasons.
pub mod termination {
    /// The host ended the stream on purpose.
    pub const GRACEFUL: i32 = 0;
    /// The host ended the stream before the first frame arrived.
    pub const UNEXPECTED_EARLY: i32 = -102;
    pub const PROTECTED_CONTENT: i32 = -103;
    pub const FRAME_CONVERSION: i32 = -104;
    /// The host disconnected or went silent past the peer timeout, without a reason.
    pub const DISCONNECTED: i32 = -1;
}

/// Size of the `DS_EFFECT` payload of each trigger.
pub const ADAPTIVE_TRIGGER_EFFECT_BYTES: usize = 10;
/// `DS_EFFECT_RIGHT_TRIGGER` and `DS_EFFECT_LEFT_TRIGGER` in [`ControlEvent::AdaptiveTriggers`].
pub const ADAPTIVE_TRIGGER_RIGHT: u8 = 0x04;
pub const ADAPTIVE_TRIGGER_LEFT: u8 = 0x08;

/// The host display's HDR metadata (`SS_HDR_METADATA`), sent by Sunshine with HDR mode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HdrMetadata {
    /// Red, green, blue, as (x, y) normalized to 50 000.
    pub display_primaries: [(u16, u16); 3],
    pub white_point: (u16, u16),
    /// Nits.
    pub max_display_luminance: u16,
    /// Ten-thousandths of a nit.
    pub min_display_luminance: u16,
    pub max_content_light_level: u16,
    pub max_frame_average_light_level: u16,
    pub max_full_frame_luminance: u16,
}

impl HdrMetadata {
    /// Reads as many fields as present, leaving the rest 0, as the C does for older hosts.
    fn decode(data: &[u8]) -> Self {
        let mut words = data
            .chunks_exact(2)
            .map(|w| u16::from_le_bytes([w[0], w[1]]));
        let mut next = || words.next().unwrap_or(0);
        let mut metadata = Self::default();
        for primary in &mut metadata.display_primaries {
            *primary = (next(), next());
        }
        metadata.white_point = (next(), next());
        metadata.max_display_luminance = next();
        metadata.min_display_luminance = next();
        metadata.max_content_light_level = next();
        metadata.max_frame_average_light_level = next();
        metadata.max_full_frame_luminance = next();
        metadata
    }
}

/// A host cursor shape message (0x5541, docs/protocol/cursor-shape.md).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CursorShape {
    /// The host cursor is hidden.
    Hide,
    /// Draw the client's own default pointer.
    Default,
    /// Show the image cached under `id`.
    Show { id: u32 },
    /// Define `id` with this image and show it.
    Image {
        id: u32,
        width: u16,
        height: u16,
        hotspot_x: u16,
        hotspot_y: u16,
        /// The host display's scale in percent (100 = 96 DPI).
        scale_percent: u16,
        /// Straight-alpha sRGB RGBA, rows top to bottom.
        rgba: Vec<u8>,
    },
}

pub const CURSOR_SHAPE_HEADER_BYTES: usize = 8;
pub const CURSOR_SHAPE_IMAGE_HEADER_BYTES: usize = 12;
pub const CURSOR_SHAPE_MAX_PAYLOAD: usize = CURSOR_SHAPE_HEADER_BYTES
    + CURSOR_SHAPE_IMAGE_HEADER_BYTES
    + CURSOR_SHAPE_MAX_EDGE as usize * CURSOR_SHAPE_MAX_EDGE as usize * 4;

impl CursorShape {
    /// Validates every rule of the format; the error names the broken one.
    pub fn decode(data: &[u8]) -> Result<Self, &'static str> {
        let u16_at = |at: usize| u16::from_le_bytes([data[at], data[at + 1]]);
        if data.len() < CURSOR_SHAPE_HEADER_BYTES {
            return Err("short header");
        }
        if data[0] != 1 {
            return Err("unknown version");
        }
        if u16_at(2) != 0 {
            return Err("reserved header bytes set");
        }
        let id = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
        let simple = |shape: Self| {
            if data.len() != CURSOR_SHAPE_HEADER_BYTES {
                Err("wrong length")
            } else {
                Ok(shape)
            }
        };
        match data[1] {
            0 | 1 if id != 0 => Err("id set on HIDE or DEFAULT"),
            0 => simple(Self::Hide),
            1 => simple(Self::Default),
            2 | 3 if id == 0 => Err("zero id"),
            2 => simple(Self::Show { id }),
            3 => {
                if data.len() < CURSOR_SHAPE_HEADER_BYTES + CURSOR_SHAPE_IMAGE_HEADER_BYTES {
                    return Err("short image header");
                }
                let (width, height) = (u16_at(8), u16_at(10));
                let (hotspot_x, hotspot_y) = (u16_at(12), u16_at(14));
                let scale_percent = u16_at(18);
                let edges = 1..=CURSOR_SHAPE_MAX_EDGE;
                if !edges.contains(&width) || !edges.contains(&height) {
                    return Err("image size out of range");
                }
                if hotspot_x >= width || hotspot_y >= height {
                    return Err("hotspot outside the image");
                }
                if data[16] != 1 {
                    return Err("unknown pixel format");
                }
                if data[17] != 0 {
                    return Err("reserved image byte set");
                }
                if !(100..=400).contains(&scale_percent) {
                    return Err("scale out of range");
                }
                let start = CURSOR_SHAPE_HEADER_BYTES + CURSOR_SHAPE_IMAGE_HEADER_BYTES;
                if data.len() != start + usize::from(width) * usize::from(height) * 4 {
                    return Err("wrong length");
                }
                Ok(Self::Image {
                    id,
                    width,
                    height,
                    hotspot_x,
                    hotspot_y,
                    scale_percent,
                    rgba: data[start..].to_vec(),
                })
            }
            _ => Err("unknown kind"),
        }
    }
}

/// Connection quality from the video frame loss rate (`CONN_STATUS_*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionStatus {
    Okay,
    Poor,
}

/// Something the host told the client, or a change of the control stream's state.
#[derive(Clone, Debug, PartialEq)]
pub enum ControlEvent {
    Rumble {
        controller: u16,
        low_frequency: u16,
        high_frequency: u16,
    },
    RumbleTriggers {
        controller: u16,
        left: u16,
        right: u16,
    },
    /// Report `motion_type` (`LI_MOTION_TYPE_*`) at `report_rate_hz`, or stop at 0.
    SetMotionEventState {
        controller: u16,
        motion_type: u8,
        report_rate_hz: u16,
    },
    SetControllerLed {
        controller: u16,
        r: u8,
        g: u8,
        b: u8,
    },
    /// DualSense adaptive trigger effects, passed to the controller as they are.
    AdaptiveTriggers {
        controller: u16,
        /// [`ADAPTIVE_TRIGGER_RIGHT`] and/or [`ADAPTIVE_TRIGGER_LEFT`].
        event_flags: u8,
        type_left: u8,
        type_right: u8,
        left: [u8; ADAPTIVE_TRIGGER_EFFECT_BYTES],
        right: [u8; ADAPTIVE_TRIGGER_EFFECT_BYTES],
    },
    /// HDR mode changed; `metadata` comes from Sunshine hosts while HDR is on.
    HdrMode {
        enabled: bool,
        metadata: Option<HdrMetadata>,
    },
    /// Actuator PCM for a controller (0x5601).
    ControllerHaptics(Pcm),
    /// USB controller speaker PCM (0x5533).
    ControllerSpeaker(Pcm),
    /// USB controller audio status from the host (0x5535), 8 to 972 bytes as sent.
    ControllerAudioControl(Vec<u8>),
    /// Per-frame host timing (0x5536), each record with the extension an extended report
    /// carries for it (`None` for a v1 report or an extension that did not validate).
    HostTiming(Vec<(HostTiming, Option<HostTimingExtra>)>),
    /// A clock sync answer (0x5537) and when its datagram was handed to the state machine.
    ClockSync {
        reply: ClockSyncReply,
        received: Instant,
    },
    /// The host lowered, raised or restored the video bitrate for the network (0x5538), sent
    /// only after a [`ClientMessage::LinkNoticeRequest`](super::messages::ClientMessage).
    LinkNotice(LinkNotice),
    /// A host text field took or lost focus (0x553B), sent only after a
    /// [`ClientMessage::TextInputRequest`](super::messages::ClientMessage).
    TextInput(rubylight_protocol::text_input::TextInputNotice),
    /// The host's pipeline frame rates (0x553D), sent only after a
    /// [`ClientMessage::PipelineRatesRequest`](super::messages::ClientMessage).
    PipelineRates(PipelineRatesReport),
    /// What became of a [`ClientMessage::RateChange`](super::messages::ClientMessage) (0x553C),
    /// one answer per request, matched by its serial.
    RateChange(RateChangeResult),
    /// A cursor shape (0x5541), while shapes are requested.
    CursorShape(CursorShape),
    /// A cursor shape message that broke a rule of the format.
    CursorShapeRejected(&'static str),
    /// The host's Frame Warp model changed (0x5544), while Frame Warp is requested. Tags and
    /// HUD masks go straight to [`crate::frame_warp::FrameWarp`] and are not events.
    FrameWarpModel(rubylight_protocol::frame_warp::Model),
    /// A Steam Controller haptic report (0x5504), for the real controller's pads. Never
    /// coalesced: the host re-sends the same report to keep a buzz going.
    SteamHaptic(SteamHaptic),
    /// A Steam Controller settings write (0x5505), for the real controller. Never coalesced.
    SteamSettings(SteamSettings),
    /// A 0x5504 or 0x5505 message that broke its format.
    SteamRejected {
        message_type: u16,
        error: SteamError,
    },
    /// USB forwarding from the host (0x553A): a tunnel token, a status or a detach, for
    /// [`crate::usb::Forwarder::on_message`]. Never coalesced.
    Usb(UsbMessage),
    /// A clipboard frame from the host (0x5508), for
    /// [`ClipboardSync::received`](super::clipboard_sync::ClipboardSync::received). Never
    /// coalesced.
    ClipboardSync(rubylight_protocol::clipboard_sync::Frame),
    /// A 0x5508 payload of this many bytes that is not a v1 frame.
    ClipboardSyncRejected(usize),
    ConnectionStatus(ConnectionStatus),
    /// Client roaming started, moved the stream, gave up, or was not needed.
    Roam(RoamEvent),
    /// The stream ended: the host's reason, or [`termination::DISCONNECTED`].
    Terminated {
        error: i32,
    },
}

impl ControlEvent {
    /// Whether moonlight-common-c delivers it on its async callback thread, where runs of
    /// the same kind (and controller) collapse to the newest.
    fn coalesce_key(&self) -> Option<(u8, u16)> {
        match self {
            Self::Rumble { controller, .. } => Some((0, *controller)),
            Self::RumbleTriggers { controller, .. } => Some((1, *controller)),
            Self::SetControllerLed { controller, .. } => Some((2, *controller)),
            Self::HdrMode { .. } => Some((3, 0)),
            _ => None,
        }
    }
}

/// Drops each rumble, trigger rumble, LED or HDR event directly followed by a newer one of
/// the same kind for the same controller, as the C's async callback thread does with a
/// backlog. Everything else passes in order.
pub fn coalesce(events: &mut Vec<ControlEvent>) {
    let mut index = 0;
    while index + 1 < events.len() {
        match events[index].coalesce_key() {
            Some(key) if events[index + 1].coalesce_key() == Some(key) => {
                events.remove(index);
            }
            _ => index += 1,
        }
    }
}

/// The termination code for a 0x0109 payload: a big-endian HRESULT when it has four bytes,
/// GFE's little-endian 16-bit reason when two; `None` for a runt.
pub fn termination_error(payload: &[u8], saw_frame: bool) -> Option<i32> {
    let intended = if saw_frame {
        termination::GRACEFUL
    } else {
        termination::UNEXPECTED_EARLY
    };
    if payload.len() >= 4 {
        let code = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
        Some(match code {
            0x800e_9403 => termination::FRAME_CONVERSION,
            0x800e_9302 => termination::PROTECTED_CONTENT,
            0x8003_0023 => intended,
            other => other as i32,
        })
    } else if payload.len() >= 2 {
        let reason = u16::from_le_bytes([payload[0], payload[1]]);
        Some(if reason == 0x0100 {
            intended
        } else {
            i32::from(reason)
        })
    } else {
        None
    }
}

fn u16_at(p: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([p[at], p[at + 1]])
}

pub(crate) fn rumble(p: &[u8]) -> Option<ControlEvent> {
    (p.len() >= 10).then(|| ControlEvent::Rumble {
        controller: u16_at(p, 4),
        low_frequency: u16_at(p, 6),
        high_frequency: u16_at(p, 8),
    })
}

pub(crate) fn rumble_triggers(p: &[u8]) -> Option<ControlEvent> {
    (p.len() >= 6).then(|| ControlEvent::RumbleTriggers {
        controller: u16_at(p, 0),
        left: u16_at(p, 2),
        right: u16_at(p, 4),
    })
}

pub(crate) fn motion_event(p: &[u8]) -> Option<ControlEvent> {
    (p.len() >= 5).then(|| ControlEvent::SetMotionEventState {
        controller: u16_at(p, 0),
        report_rate_hz: u16_at(p, 2),
        motion_type: p[4],
    })
}

pub(crate) fn controller_led(p: &[u8]) -> Option<ControlEvent> {
    (p.len() >= 5).then(|| ControlEvent::SetControllerLed {
        controller: u16_at(p, 0),
        r: p[2],
        g: p[3],
        b: p[4],
    })
}

pub(crate) fn adaptive_triggers(p: &[u8]) -> Option<ControlEvent> {
    const E: usize = ADAPTIVE_TRIGGER_EFFECT_BYTES;
    if p.len() < 5 + 2 * E {
        return None;
    }
    let mut left = [0; E];
    let mut right = [0; E];
    left.copy_from_slice(&p[5..5 + E]);
    right.copy_from_slice(&p[5 + E..5 + 2 * E]);
    Some(ControlEvent::AdaptiveTriggers {
        controller: u16_at(p, 0),
        event_flags: p[2],
        type_left: p[3],
        type_right: p[4],
        left,
        right,
    })
}

/// HDR mode: an enable byte, then Sunshine's metadata.
pub(crate) fn hdr_mode(p: &[u8], sunshine: bool) -> Option<(bool, Option<HdrMetadata>)> {
    let (&enable, rest) = p.split_first()?;
    let metadata = sunshine.then(|| HdrMetadata::decode(rest));
    Some((enable != 0, metadata))
}

/// Only the host's kinds: a client never hears its own attach request.
pub(crate) fn usb(p: &[u8]) -> Option<ControlEvent> {
    UsbMessage::decode(p)
        .filter(|message| !matches!(message, UsbMessage::AttachRequest { .. }))
        .map(ControlEvent::Usb)
}

pub(crate) fn controller_haptics(p: &[u8]) -> Option<ControlEvent> {
    haptics::decode(p).map(ControlEvent::ControllerHaptics)
}

pub(crate) fn controller_speaker(p: &[u8]) -> Option<ControlEvent> {
    if !(8..=haptics::MAX_PAYLOAD).contains(&p.len()) {
        return None;
    }
    haptics::decode(p).map(ControlEvent::ControllerSpeaker)
}

pub(crate) fn controller_audio_control(p: &[u8]) -> Option<ControlEvent> {
    (8..=haptics::MAX_PAYLOAD)
        .contains(&p.len())
        .then(|| ControlEvent::ControllerAudioControl(p.to_vec()))
}

/// A host timing report: a 4-byte header and 1 to 32 records of 20 bytes, and for an extended
/// report 16 more bytes per record. The upper bound is the largest extended report the decoder
/// accepts (32 records), not the 17 a host sends, so the host's cap can rise later.
pub(crate) fn host_timing(p: &[u8]) -> Option<ControlEvent> {
    const MAX: usize = HOST_TIMING_HEADER_BYTES
        + HOST_TIMING_MAX_RECORDS * (HOST_TIMING_RECORD_BYTES + HOST_TIMING_EXTRA_BYTES);
    if !(HOST_TIMING_HEADER_BYTES + HOST_TIMING_RECORD_BYTES..=MAX).contains(&p.len()) {
        return None;
    }
    HostTiming::decode_report_with(p).map(ControlEvent::HostTiming)
}

/// Newer hosts may append fields; anything over 256 bytes is not a reply.
pub(crate) fn clock_sync(p: &[u8], received: Instant) -> Option<ControlEvent> {
    if !(rubylight_protocol::control::CLOCK_SYNC_REPLY_BYTES..=256).contains(&p.len()) {
        return None;
    }
    ClockSyncReply::decode(p).map(|reply| ControlEvent::ClockSync { reply, received })
}

/// A link notice: 12 bytes, and a newer host may append fields. Anything over 256 bytes is not
/// one, and a notice that breaks the format's rules is dropped.
pub(crate) fn link_notice(p: &[u8]) -> Option<ControlEvent> {
    if !(LINK_NOTICE_BYTES..=256).contains(&p.len()) {
        return None;
    }
    LinkNotice::decode(p).map(ControlEvent::LinkNotice)
}

/// A text input notice: its header and up to 4096 bytes of text; one that breaks the
/// format's rules is dropped.
pub(crate) fn text_input(p: &[u8]) -> Option<ControlEvent> {
    rubylight_protocol::text_input::TextInputNotice::decode(p).map(ControlEvent::TextInput)
}

/// A clipboard frame, or its rejection when it is not a v1 frame.
pub(crate) fn clipboard_sync(p: &[u8]) -> ControlEvent {
    match rubylight_protocol::clipboard_sync::Frame::decode(p) {
        Some(frame) => ControlEvent::ClipboardSync(frame),
        None => ControlEvent::ClipboardSyncRejected(p.len()),
    }
}

/// Pipeline frame rates: 28 bytes, and a newer host may append fields. Anything over 256
/// bytes is not a report.
pub(crate) fn pipeline_rates(p: &[u8]) -> Option<ControlEvent> {
    if !(PIPELINE_RATES_REPORT_BYTES..=256).contains(&p.len()) {
        return None;
    }
    PipelineRatesReport::decode(p).map(ControlEvent::PipelineRates)
}

/// A rate change result: 20 bytes, and a newer host may append fields. Anything over 256
/// bytes, or with a status this version does not know, is not a result.
pub(crate) fn rate_change(p: &[u8]) -> Option<ControlEvent> {
    if !(RATE_CHANGE_RESULT_BYTES..=256).contains(&p.len()) {
        return None;
    }
    RateChangeResult::decode(p).map(ControlEvent::RateChange)
}

pub(crate) fn cursor_shape(p: &[u8]) -> ControlEvent {
    if !(CURSOR_SHAPE_HEADER_BYTES..=CURSOR_SHAPE_MAX_PAYLOAD).contains(&p.len()) {
        return ControlEvent::CursorShapeRejected("message size out of range");
    }
    match CursorShape::decode(p) {
        Ok(shape) => ControlEvent::CursorShape(shape),
        Err(reason) => ControlEvent::CursorShapeRejected(reason),
    }
}

pub(crate) fn steam_haptic(p: &[u8]) -> ControlEvent {
    match SteamHaptic::decode(p) {
        Ok(haptic) => ControlEvent::SteamHaptic(haptic),
        Err(error) => ControlEvent::SteamRejected {
            message_type: super::wire::ptype::STEAM_HAPTIC,
            error,
        },
    }
}

pub(crate) fn steam_settings(p: &[u8]) -> ControlEvent {
    match SteamSettings::decode(p) {
        Ok(settings) => ControlEvent::SteamSettings(settings),
        Err(error) => ControlEvent::SteamRejected {
            message_type: super::wire::ptype::STEAM_SETTINGS,
            error,
        },
    }
}

#[cfg(test)]
mod tests;
