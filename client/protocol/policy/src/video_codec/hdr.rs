//! Pure 10-bit SDR (`Sdr10Policy`), HDR10/HLG (`HdrPolicy`), and extended HDR format
//! (`HdrFormatPolicy`) decision logic.
//!
//! Uses `rubylight_protocol::hdr::*` directly for wire HDR format bitflags.

mod choice;
pub use choice::HdrFormatChoice;
use rubylight_protocol::hdr::{
    DOLBY_VISION, DOLBY_VISION_AV1, DOLBY_VISION_HEVC, DV_81, DV_84, DV_101, HLG, PQ, PYROWAVE_HLG,
};

use super::{
    VIDEO_FORMAT_AV1_MAIN10, VIDEO_FORMAT_H265_MAIN10, VIDEO_FORMAT_MASK_10BIT,
    VIDEO_FORMAT_MASK_AV1, VIDEO_FORMAT_MASK_H265, VIDEO_FORMAT_MASK_PYROWAVE,
    VIDEO_FORMAT_MASK_YUV444,
};

/// Canonical bitflags representing Android `Display.HdrCapabilities.HDR_TYPE_*`.
pub const DISPLAY_HDR_HDR10: u32 = 1 << 0;
pub const DISPLAY_HDR_HLG: u32 = 1 << 1;
pub const DISPLAY_HDR_DOLBY_VISION: u32 = 1 << 2;
pub const DISPLAY_HDR_HDR10_PLUS: u32 = 1 << 3;

/// Window dataspace probe call constants matching `WindowDataspace.CALL_*`.
pub const WINDOW_CALL_NONE: i32 = 0;
pub const WINDOW_CALL_PUBLIC: i32 = 1;
pub const WINDOW_CALL_PERFORM: i32 = 2;

/// Minimum Android API level (`Build.VERSION_CODES.UPSIDE_DOWN_CAKE`) with per-mode HDR lists.
pub const SDK_PER_MODE_HDR: i32 = 34;

/// Canonical color transfer normalized from `MediaFormat.COLOR_TRANSFER_*`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum ColorTransfer {
    None = 0,
    Pq = 1,
    Hlg = 2,
    Other = 3,
}

impl ColorTransfer {
    pub const fn from_code(code: i32) -> Self {
        match code {
            0 => Self::None,
            1 => Self::Pq,
            2 => Self::Hlg,
            _ => Self::Other,
        }
    }
}

/// Why 10-bit SDR is not offered, matching `Sdr10Policy.Refusal`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum Sdr10Refusal {
    None = 0,
    SettingOff = 1,
    HdrStream = 2,
    NoVideo = 3,
    NoMain10 = 4,
    PyroWave = 5,
    Packed444 = 6,
}

impl Sdr10Refusal {
    pub const fn from_ordinal(ord: i32) -> Option<Self> {
        match ord {
            0 => Some(Self::None),
            1 => Some(Self::SettingOff),
            2 => Some(Self::HdrStream),
            3 => Some(Self::NoVideo),
            4 => Some(Self::NoMain10),
            5 => Some(Self::PyroWave),
            6 => Some(Self::Packed444),
            _ => None,
        }
    }
}

/// Notice step on a 10-bit SDR host HDR report, matching `Sdr10Policy.HostHdrStep`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum HostHdrStep {
    None = 0,
    Say = 1,
    Rearm = 2,
}

/// Overlay status for 10-bit SDR, matching `Sdr10Policy.Status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum Sdr10Status {
    Quiet = 0,
    HostHdr = 1,
}

/// Why HDR is refused, matching `HdrPolicy.Refusal`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum HdrRefusal {
    None = 0,
    SettingOff = 1,
    DisplayWithoutHdr10 = 2,
    ModeWithoutHdr10 = 3,
    DecoderWithoutHdr10 = 4,
    HlgNeedsDirectOutput = 5,
}

/// Host mastering metadata state, matching `HdrPolicy.Metadata`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum HdrMetadataState {
    Absent = 0,
    Empty = 1,
    Usable = 2,
}

impl HdrMetadataState {
    pub const fn from_ordinal(ord: i32) -> Self {
        match ord {
            1 => Self::Empty,
            2 => Self::Usable,
            _ => Self::Absent,
        }
    }
}

/// Running HDR overlay status, matching `HdrPolicy.Status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum HdrStatus {
    Quiet = 0,
    Waiting = 1,
    NotNegotiated = 2,
    HostSdr = 3,
    NoMetadata = 4,
    OutputNotPq = 5,
    PqOnHlgScreen = 6,
    ShownAsSdr = 7,
}

impl HdrStatus {
    pub const fn from_ordinal(ord: i32) -> Option<Self> {
        match ord {
            0 => Some(Self::Quiet),
            1 => Some(Self::Waiting),
            2 => Some(Self::NotNegotiated),
            3 => Some(Self::HostSdr),
            4 => Some(Self::NoMetadata),
            5 => Some(Self::OutputNotPq),
            6 => Some(Self::PqOnHlgScreen),
            7 => Some(Self::ShownAsSdr),
            _ => None,
        }
    }
}

/// Who draws the decoder's picture, matching `HdrFormatPolicy.DrawnSurface`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum DrawnSurface {
    None = 0,
    PqOnly = 1,
    PqHlg = 2,
}

impl DrawnSurface {
    pub const fn from_ordinal(ord: i32) -> Self {
        match ord {
            1 => Self::PqOnly,
            2 => Self::PqHlg,
            _ => Self::None,
        }
    }
}

/// Window BT2020_HLG dataspace probe result, matching `HdrFormatPolicy.HlgWindow`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum HlgWindow {
    Public = 0,
    Perform = 1,
    Refused = 2,
}

impl HlgWindow {
    pub const fn from_ordinal(ord: i32) -> Option<Self> {
        match ord {
            0 => Some(Self::Public),
            1 => Some(Self::Perform),
            2 => Some(Self::Refused),
            _ => None,
        }
    }

    pub const fn of_call(call: i32) -> Self {
        match call {
            WINDOW_CALL_PUBLIC => Self::Public,
            WINDOW_CALL_PERFORM => Self::Perform,
            _ => Self::Refused,
        }
    }

    pub const fn accepted(self) -> bool {
        !matches!(self, Self::Refused)
    }
}

/// Drawn-surface HDR capability limit, matching `HdrFormatPolicy.DrawnLimit`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum DrawnLimit {
    None = 0,
    DolbyVision = 1,
    Hdr10Plus = 2,
}

// --- Sdr10Policy ---

pub const fn sdr10_refusal(
    media_stream: bool,
    enabled: bool,
    hdr_granted: bool,
    hevc_main10: bool,
    av1_main10: bool,
    pyro_wave_chosen: bool,
    packed444: bool,
) -> Sdr10Refusal {
    if !enabled {
        return Sdr10Refusal::SettingOff;
    }
    if !media_stream {
        return Sdr10Refusal::NoVideo;
    }
    if hdr_granted {
        return Sdr10Refusal::HdrStream;
    }
    if pyro_wave_chosen {
        return Sdr10Refusal::PyroWave;
    }
    if packed444 {
        return Sdr10Refusal::Packed444;
    }
    if hevc_main10 || av1_main10 {
        Sdr10Refusal::None
    } else {
        Sdr10Refusal::NoMain10
    }
}

pub const fn sdr10_worth_saying(refusal: Option<Sdr10Refusal>) -> bool {
    matches!(
        refusal,
        Some(Sdr10Refusal::NoMain10 | Sdr10Refusal::PyroWave | Sdr10Refusal::Packed444)
    )
}

pub const fn sdr10_decoder_choice_may_change(
    enabled: bool,
    hdr_requested: bool,
    pyro_wave_chosen: bool,
    packed444: bool,
) -> bool {
    enabled && !hdr_requested && !pyro_wave_chosen && !packed444
}

pub const fn sdr10_yuv444_wins(common_formats: u32) -> bool {
    (common_formats & VIDEO_FORMAT_MASK_YUV444 & !VIDEO_FORMAT_MASK_PYROWAVE) != 0
}

pub const fn sdr10_without_ten_bit(offered_formats: u32) -> u32 {
    offered_formats & !VIDEO_FORMAT_MASK_10BIT
}

pub const fn sdr10_host_hdr_step(
    ten_bit: bool,
    host_hdr: Option<bool>,
    previous: Option<bool>,
) -> HostHdrStep {
    if !ten_bit {
        return HostHdrStep::None;
    }
    let Some(hdr) = host_hdr else {
        return HostHdrStep::None;
    };
    if let Some(prev) = previous
        && prev == hdr
    {
        return HostHdrStep::None;
    }
    if hdr {
        HostHdrStep::Say
    } else {
        HostHdrStep::Rearm
    }
}

pub const fn sdr10_formats(
    refusal: Option<Sdr10Refusal>,
    hevc_main10: bool,
    av1_main10: bool,
) -> u32 {
    if !matches!(refusal, Some(Sdr10Refusal::None)) {
        return 0;
    }
    (if hevc_main10 {
        VIDEO_FORMAT_H265_MAIN10
    } else {
        0
    }) | (if av1_main10 {
        VIDEO_FORMAT_AV1_MAIN10
    } else {
        0
    })
}

pub const fn sdr10_status(ten_bit: bool, host_hdr: Option<bool>) -> Sdr10Status {
    if ten_bit && matches!(host_hdr, Some(true)) {
        Sdr10Status::HostHdr
    } else {
        Sdr10Status::Quiet
    }
}

pub const fn sdr10_overlay_color(
    ten_bit: bool,
    sdr10: bool,
    host_hdr: Option<bool>,
) -> &'static str {
    if matches!(host_hdr, Some(true)) {
        return "HDR";
    }
    if sdr10 && ten_bit {
        return "10-bit SDR";
    }
    if host_hdr.is_none() && ten_bit {
        "10-bit"
    } else {
        "SDR"
    }
}

pub const fn sdr10_assume_hdr(ten_bit: bool, sdr10: bool) -> bool {
    ten_bit && !sdr10
}

// --- HdrPolicy ---

pub const fn hdr_has_hdr10(display_mask: u32) -> bool {
    (display_mask & DISPLAY_HDR_HDR10) != 0
}

pub const fn hdr_hlg_only(display_mask: u32) -> bool {
    !hdr_has_hdr10(display_mask) && (display_mask & DISPLAY_HDR_HLG) != 0
}

pub const fn hdr_should_force_hlg_only(debug_hlg_only: bool, display_mask: u32) -> bool {
    debug_hlg_only && (hdr_has_hdr10(display_mask) || (display_mask & DISPLAY_HDR_HLG) != 0)
}

pub const fn hdr_display_refusal(
    sdk: i32,
    enabled: bool,
    display_mask: u32,
    any_mode_mask: u32,
    host_hdr_formats: u32,
    surface: DrawnSurface,
) -> HdrRefusal {
    if !enabled {
        return HdrRefusal::SettingOff;
    }
    if hdr_has_hdr10(display_mask) {
        return HdrRefusal::None;
    }
    let hlg = hdr_format_hlg_instead(host_hdr_formats, display_mask);
    if hlg && !matches!(surface, DrawnSurface::PqOnly) {
        return HdrRefusal::None;
    }
    if sdk >= SDK_PER_MODE_HDR && hdr_has_hdr10(any_mode_mask) {
        return HdrRefusal::ModeWithoutHdr10;
    }
    if hlg {
        HdrRefusal::HlgNeedsDirectOutput
    } else {
        HdrRefusal::DisplayWithoutHdr10
    }
}

pub const fn hdr_reports_hdr_output(display_mask: u32, stream_transfer: ColorTransfer) -> bool {
    hdr_has_hdr10(display_mask)
        || (hdr_hlg_only(display_mask) && matches!(stream_transfer, ColorTransfer::Hlg))
}

pub const fn hdr_decoder_refusal(
    hevc_main10_hdr10: bool,
    av1_main10: bool,
    pyro_wave: bool,
) -> HdrRefusal {
    if hevc_main10_hdr10 || av1_main10 || pyro_wave {
        HdrRefusal::None
    } else {
        HdrRefusal::DecoderWithoutHdr10
    }
}

pub fn hdr_metadata(wire: Option<&[u8]>) -> HdrMetadataState {
    let Some(wire) = wire else {
        return HdrMetadataState::Absent;
    };
    if wire.len() < 24 {
        return HdrMetadataState::Absent;
    }
    let primaries = wire[..12].iter().any(|&b| b != 0);
    let max_luminance = u16::from_le_bytes([wire[16], wire[17]]);
    if primaries && max_luminance > 0 {
        HdrMetadataState::Usable
    } else {
        HdrMetadataState::Empty
    }
}

pub const fn hdr_drawn_status(status: HdrStatus, tone_mapped: bool) -> HdrStatus {
    if tone_mapped && matches!(status, HdrStatus::Quiet | HdrStatus::NoMetadata) {
        HdrStatus::ShownAsSdr
    } else {
        status
    }
}

pub const fn hdr_hands_back_from_vulkan(hdr_reported: bool, tone_mapped: bool) -> bool {
    hdr_reported && tone_mapped
}

pub const fn hdr_hand_over_after_vulkan_stop(had_renderer: bool, stop_finished: bool) -> bool {
    !had_renderer || stop_finished
}

pub const fn hdr_status(
    asked: bool,
    ten_bit: bool,
    host_hdr: Option<bool>,
    metadata: HdrMetadataState,
    output_transfer: ColorTransfer,
    stream_transfer: ColorTransfer,
    hlg_only_screen: bool,
) -> HdrStatus {
    if !asked {
        return HdrStatus::Quiet;
    }
    if !ten_bit {
        return HdrStatus::NotNegotiated;
    }
    if hlg_only_screen && !matches!(stream_transfer, ColorTransfer::Hlg) {
        return HdrStatus::PqOnHlgScreen;
    }
    let Some(hdr) = host_hdr else {
        return HdrStatus::Waiting;
    };
    if !hdr {
        return HdrStatus::HostSdr;
    }
    if !matches!(output_transfer, ColorTransfer::None)
        && (output_transfer as i32) != (stream_transfer as i32)
    {
        return HdrStatus::OutputNotPq;
    }
    if matches!(stream_transfer, ColorTransfer::Hlg) {
        return HdrStatus::Quiet;
    }
    if matches!(metadata, HdrMetadataState::Usable) {
        HdrStatus::Quiet
    } else {
        HdrStatus::NoMetadata
    }
}

// --- HdrFormatPolicy ---

const DOLBY_VISION_ORDER: [u8; 3] = [DV_101, DV_81, DV_84];

pub const fn hdr_format_pick(
    host_formats: u32,
    display_mask: u32,
    dolby_vision_decoders: u32,
    video_formats: u32,
    surface: DrawnSurface,
) -> u32 {
    if matches!(surface, DrawnSurface::PqOnly) {
        return 0;
    }
    if matches!(surface, DrawnSurface::None) && (display_mask & DISPLAY_HDR_DOLBY_VISION) != 0 {
        let mut i = 0;
        while i < DOLBY_VISION_ORDER.len() {
            let fmt = DOLBY_VISION_ORDER[i] as u32;
            if (host_formats & dolby_vision_decoders & fmt) != 0
                && (video_formats & hdr_format_carriers(fmt)) != 0
            {
                return fmt;
            }
            i += 1;
        }
    }
    let mut format = 0;
    if (host_formats & (HLG as u32)) != 0
        && !hdr_has_hdr10(display_mask)
        && (display_mask & DISPLAY_HDR_HLG) != 0
    {
        format = HLG as u32;
    }
    if matches!(surface, DrawnSurface::None) {
        format
    } else {
        format & ((PQ | HLG) as u32)
    }
}

pub const fn hdr_format_vulkan_limit(
    host_formats: u32,
    display_mask: u32,
    dolby_vision_decoders: u32,
    video_formats: u32,
) -> DrawnLimit {
    if host_formats == 0 {
        return DrawnLimit::None;
    }
    if hdr_format_dolby_vision(hdr_format_pick(
        host_formats,
        display_mask,
        dolby_vision_decoders,
        video_formats,
        DrawnSurface::None,
    )) {
        DrawnLimit::DolbyVision
    } else {
        DrawnLimit::None
    }
}

pub const fn hdr_format_vulkan_needs_hlg_probe(
    host_formats: u32,
    display_mask: u32,
    surface: DrawnSurface,
) -> bool {
    matches!(surface, DrawnSurface::PqHlg)
        && hdr_format_pick(host_formats, display_mask, 0, 0, surface) == (HLG as u32)
}

pub const fn hdr_format_vulkan_surface(surface: DrawnSurface, window: HlgWindow) -> DrawnSurface {
    if matches!(surface, DrawnSurface::PqHlg) && !window.accepted() {
        DrawnSurface::PqOnly
    } else {
        surface
    }
}

pub const fn hdr_format_passes_hdr10_plus(display_mask: u32, transfer: ColorTransfer) -> bool {
    matches!(transfer, ColorTransfer::Pq) && (display_mask & DISPLAY_HDR_HDR10_PLUS) != 0
}

pub const fn hdr_format_pyrowave_surface(host_formats: u32) -> DrawnSurface {
    if (host_formats & (PYROWAVE_HLG as u32)) != 0 && (host_formats & (HLG as u32)) != 0 {
        DrawnSurface::PqHlg
    } else {
        DrawnSurface::PqOnly
    }
}

pub const fn hdr_format_pyrowave_surface_with_window(
    host_formats: u32,
    window: HlgWindow,
) -> DrawnSurface {
    if window.accepted() {
        hdr_format_pyrowave_surface(host_formats)
    } else {
        DrawnSurface::PqOnly
    }
}

pub const fn hdr_format_pyrowave_needs_hlg_probe(host_formats: u32, display_mask: u32) -> bool {
    hdr_format_pick(
        host_formats,
        display_mask,
        0,
        0,
        hdr_format_pyrowave_surface(host_formats),
    ) == (HLG as u32)
}

pub const fn hdr_format_pyrowave_hlg_refused_by_display(
    host_formats: u32,
    display_mask: u32,
    window: HlgWindow,
) -> bool {
    matches!(window, HlgWindow::Refused)
        && hdr_format_pyrowave_needs_hlg_probe(host_formats, display_mask)
}

pub const fn hdr_format_pyrowave_hlg_lost(
    announced_format: u32,
    window: Option<HlgWindow>,
    window_kept_hlg: bool,
    frames_stopped: bool,
) -> bool {
    let Some(w) = window else {
        return false;
    };
    announced_format == (HLG as u32) && w.accepted() && (!window_kept_hlg || frames_stopped)
}

pub const fn hdr_format_pyrowave_limit(
    host_formats: u32,
    display_mask: u32,
    dolby_vision_decoders: u32,
    format: u32,
) -> DrawnLimit {
    if host_formats == 0 {
        return DrawnLimit::None;
    }
    let carriers = VIDEO_FORMAT_H265_MAIN10 | VIDEO_FORMAT_AV1_MAIN10;
    if hdr_format_dolby_vision(hdr_format_pick(
        host_formats,
        display_mask,
        dolby_vision_decoders,
        carriers,
        DrawnSurface::None,
    )) {
        return DrawnLimit::DolbyVision;
    }
    if !hdr_format_transfer_is_hlg(format) && (display_mask & DISPLAY_HDR_HDR10_PLUS) != 0 {
        return DrawnLimit::Hdr10Plus;
    }
    DrawnLimit::None
}

pub const fn hdr_format_pyrowave_hlg_needs_host_update(
    host_formats: u32,
    display_mask: u32,
) -> bool {
    (host_formats & (PYROWAVE_HLG as u32)) == 0
        && !hdr_has_hdr10(display_mask)
        && hdr_format_hlg_instead(host_formats, display_mask)
}

pub const fn hdr_format_hlg_instead(host_formats: u32, display_mask: u32) -> bool {
    (host_formats & (HLG as u32)) != 0 && (display_mask & DISPLAY_HDR_HLG) != 0
}

pub const fn hdr_format_dolby_vision(format: u32) -> bool {
    (format & (DOLBY_VISION as u32)) != 0
}

pub const fn hdr_format_carriers(format: u32) -> u32 {
    if (format & (DOLBY_VISION_AV1 as u32)) != 0 {
        VIDEO_FORMAT_AV1_MAIN10
    } else if (format & (DOLBY_VISION_HEVC as u32)) != 0 {
        VIDEO_FORMAT_H265_MAIN10
    } else {
        0
    }
}

pub const fn hdr_format_video_formats(
    format: u32,
    mut video_formats: u32,
    automatic_codec: bool,
) -> u32 {
    let carriers = hdr_format_carriers(format);
    if !automatic_codec || carriers == 0 || (video_formats & carriers) == 0 {
        return video_formats;
    }
    if (video_formats & VIDEO_FORMAT_MASK_AV1 & carriers) == 0 {
        video_formats &= !VIDEO_FORMAT_MASK_AV1;
    }
    if (video_formats & VIDEO_FORMAT_MASK_H265 & carriers) == 0 {
        video_formats &= !VIDEO_FORMAT_MASK_H265;
    }
    video_formats
}

pub const fn hdr_format_codec_names(formats: u32) -> &'static str {
    let av1 = (formats & VIDEO_FORMAT_MASK_AV1) != 0;
    let hevc = (formats & VIDEO_FORMAT_MASK_H265) != 0;
    match (av1, hevc) {
        (true, true) => "AV1 and HEVC",
        (true, false) => "AV1",
        (false, true) => "HEVC",
        (false, false) => "nothing",
    }
}

pub const fn hdr_format_transfer_is_hlg(format: u32) -> bool {
    (format & ((HLG | DV_84) as u32)) != 0
}

pub const fn hdr_format_base_layer_name(format: u32) -> &'static str {
    if hdr_format_transfer_is_hlg(format) {
        "HLG"
    } else {
        "HDR10"
    }
}

pub const fn hdr_format_name(format: u32) -> &'static str {
    if format == HLG as u32 {
        "HLG"
    } else if format == DV_81 as u32 {
        "Dolby Vision 8.1"
    } else if format == DV_84 as u32 {
        "Dolby Vision 8.4"
    } else if format == DV_101 as u32 {
        "Dolby Vision 10.1"
    } else {
        "HDR10"
    }
}

pub fn hdr_format_overlay_word(
    format: u32,
    dolby_vision_decoding: bool,
    hdr10_plus: bool,
) -> String {
    if hdr10_plus && !hdr_format_dolby_vision(format) && !hdr_format_transfer_is_hlg(format) {
        return "HDR10+".to_owned();
    }
    if hdr_format_dolby_vision(format) {
        return if dolby_vision_decoding {
            hdr_format_name(format).to_owned()
        } else {
            format!(
                "{} (Dolby Vision decoder failed)",
                hdr_format_base_layer_name(format)
            )
        };
    }
    if format == u32::from(HLG) {
        "HLG".to_owned()
    } else {
        "HDR".to_owned()
    }
}
