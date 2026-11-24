//! Direct scanout on Wayland: what lets a compositor put the stream's swapchain image on a display
//! plane itself instead of composing it into its own frame (a pass on the GPU the decoder shares,
//! and at least a refresh more before the glass), what Nightjar does for it, and why it did or did
//! not happen, for the end-of-stream log.
//!
//! What KWin needs (its source, Plasma 6.2 to 6.8 and master): a fullscreen, fully opaque window
//! with no effect on it and nothing over it (`WorkspaceScene::scanoutCandidates`, from 6.6
//! `isCandidate`), no software cursor, a dmabuf whose format and modifier the plane takes (else it
//! sends a scanout tranche in the surface's dmabuf feedback), and a colour pipeline from the
//! surface's description to the output that is the identity or that the display engine can do
//! (`ColorPipeline::create`, `EglGbmLayer::doImportScanoutBuffer`/`earlyScanoutChecks`). From 6.5
//! KWin blends in gamma 2.2 and hands a surface that as its preferred description, so a PQ surface
//! on an HDR output is converted unless the plane's colour pipeline does it
//! (`KWIN_DRM_USE_COLOR_PIPELINE`, off by default up to 6.8). KWin sets the presentation feedback's
//! zero-copy flag only from 6.8. Mutter and wlroots check the same kinds of things.
//!
//! Who owns what on the window's wl_surface: Mesa's Vulkan WSI (25.0 and later) makes its
//! wp_fifo_v1 and wp_commit_timer_v1, wp_tearing_control_v1 for IMMEDIATE (with the async hint),
//! wp_linux_drm_syncobj_surface_v1, and from 25.1 a wp_color_management_surface_v1 for every
//! colour space but PASS_THROUGH. Each is one per surface: a second one, Nightjar's, would end the
//! connection. So tearing comes from presenting IMMEDIATE ([`vrr_tearing`]), and Nightjar describes
//! the surface's colours itself only on an HDR10 swapchain the patched SDL made pass-through
//! (`vendor/sdl3-src/patches/wayland-scanout.patch`), with a description matched to the output
//! ([`hdr_label`]) instead of Mesa's PQ defaults (reference white 203 nits, peak from the host's
//! metadata), which the compositor scales and may tone map. The probe ([`Probe`]) only makes
//! objects a surface may have many of: dmabuf surface feedback and colour surface feedback.
//!
//! Each behaviour has an environment switch, on unless set to 0 ([`switched_on`]).

use std::sync::Mutex;

/// Nightjar's own HDR10 description on a pass-through swapchain.
pub const HDR_DESCRIPTION_ENV: &str = "NIGHTJAR_WAYLAND_HDR_DESCRIPTION";
/// Making the swapchain again when the compositor's dmabuf feedback asks for other modifiers.
pub const DMABUF_REALLOC_ENV: &str = "NIGHTJAR_WAYLAND_DMABUF_REALLOC";
/// The end-of-stream scanout line and the objects it reads.
pub const SCANOUT_PROBE_ENV: &str = "NIGHTJAR_WAYLAND_SCANOUT_PROBE";
/// The launcher's SDR white on a PQ renderer whose surface Mesa describes ([`launcher_color_scale`]).
pub const LAUNCHER_WHITE_ENV: &str = "NIGHTJAR_PQ_LAUNCHER_WHITE";
/// IMMEDIATE (tearing) for a VRR stream without Tear-free VRR ([`vrr_tearing`]).
pub const VRR_TEARING_ENV: &str = "NIGHTJAR_WAYLAND_VRR_TEARING";
/// Drawing in the compositor's gamma 2.2 blending space where it prefers one ([`HdrLabel::Blending`]).
pub const GAMMA22_PRESENT_ENV: &str = "NIGHTJAR_WAYLAND_GAMMA22_PRESENT";

/// How bright an HDR stream is under Nightjar's own description: the setting "HDR brightness
/// follows the desktop" (`hdr_desktop_brightness`), off by default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Brightness {
    /// As the PC sends it: PQ is absolute, so 203 nits are 203 nits, whatever the desktop's white.
    /// What HDR10 means and what direct scanout needs.
    Absolute,
    /// Scaled with the desktop's brightness, as the driver's description has the compositor do
    /// (reference white 203 nits to the desktop's): the look before Nightjar's description.
    Desktop,
}

impl Brightness {
    pub fn of_setting(follows_desktop: bool) -> Self {
        if follows_desktop { Brightness::Desktop } else { Brightness::Absolute }
    }
}

static DESKTOP_BRIGHTNESS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The coming stream's brightness (from its settings); an installed description follows it.
pub fn set_brightness(brightness: Brightness) {
    DESKTOP_BRIGHTNESS.store(brightness == Brightness::Desktop, std::sync::atomic::Ordering::Relaxed);
}

pub fn brightness() -> Brightness {
    Brightness::of_setting(DESKTOP_BRIGHTNESS.load(std::sync::atomic::Ordering::Relaxed))
}

/// The host's content light level as the swapchain's metadata has it (capped at the display's
/// peak), for a description that follows the desktop: 0 unknown.
static CONTENT_PEAK: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

pub fn set_content_peak(nits: Option<f32>) {
    let nits = nits.filter(|nits| nits.is_finite() && *nits >= 1.0).map_or(0, |nits| nits.round() as u32);
    CONTENT_PEAK.store(nits, std::sync::atomic::Ordering::Relaxed);
}

pub fn content_peak() -> Option<u32> {
    Some(CONTENT_PEAK.load(std::sync::atomic::Ordering::Relaxed)).filter(|&nits| nits > 0)
}

/// What the present writes into a pass-through HDR10 swapchain (`scanout_present.hlsl`, and
/// pyrowave.hlsl's flags 4096 and 8192 for a video-only frame).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Encoding {
    /// BT.2020 PQ.
    Pq,
    /// BT.2020 gamma 2.2 over `min_nits` to `max_nits`, the compositor's blending space.
    Gamma22 { min_nits: f32, max_nits: f32 },
}

impl Encoding {
    /// The renderer properties' values: the mode (0 PQ, 1 gamma 2.2), minimum and peak in nits.
    pub fn properties(self) -> (i64, f32, f32) {
        match self {
            Encoding::Pq => (0, 0.0, 0.0),
            Encoding::Gamma22 { min_nits, max_nits } => (1, min_nits, max_nits),
        }
    }

    /// The shaders' constants from the properties, as the patched SDL pushes them: the mode,
    /// the minimum, 1 / (peak - minimum), 0; an empty range is PQ.
    pub fn constants(mode: i64, min_nits: f32, max_nits: f32) -> [f32; 4] {
        if mode == 1 && max_nits > min_nits {
            [1.0, min_nits, 1.0 / (max_nits - min_nits), 0.0]
        } else {
            [0.0; 4]
        }
    }

    /// The gamma 2.2 code of `nits` (before the dither), as the shaders compute it: the tests'
    /// reference.
    #[cfg(test)]
    pub fn gamma22_code(nits: f32, min_nits: f32, max_nits: f32) -> f32 {
        ((nits - min_nits) / (max_nits - min_nits)).clamp(0.0, 1.0).powf(1.0 / 2.2)
    }
}

/// A switch's value: on unless it reads as off (`0`, `false`, `no`, `off`); unset is on. What
/// `env_on` decides, for the tests.
#[cfg(test)]
pub fn switched_on(value: Option<&str>) -> bool {
    value.and_then(super::connect::flag_value) != Some(false)
}

/// The switch `name` from the environment, read once per run; a value that is no switch is said
/// on stderr (`connect::env_flag`) and leaves it on.
pub fn env_on(name: &'static str) -> bool {
    static READ: Mutex<Vec<(&'static str, bool)>> = Mutex::new(Vec::new());
    let mut read = READ.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(&(_, on)) = read.iter().find(|(known, _)| *known == name) {
        return on;
    }
    let on = super::connect::env_flag(name) != Some(false);
    read.push((name, on));
    on
}

/// Whether an HDR WSI layer (`VK_hdr_layer`, turned on with `ENABLE_HDR_WSI`) sits between SDL and
/// the driver: it describes the surface itself.
pub fn hdr_wsi_layer(value: Option<&str>) -> bool {
    value.is_some_and(|value| !value.trim().is_empty() && super::connect::flag_value(value) != Some(false))
}

/// Why an HDR10 swapchain's colour description stays the driver's, before the compositor is asked
/// anything; None: Nightjar describes it where the compositor allows ([`hdr_label`]).
pub fn description_refused(wayland: bool, gamescope: bool, switched_on: bool, hdr_wsi_layer: bool) -> Option<&'static str> {
    if !wayland {
        Some("the window is not a Wayland one")
    } else if gamescope {
        Some("gamescope's WSI layer describes the swapchain")
    } else if !switched_on {
        Some("NIGHTJAR_WAYLAND_HDR_DESCRIPTION is 0")
    } else if hdr_wsi_layer {
        Some("an HDR WSI layer (ENABLE_HDR_WSI) describes the surface")
    } else {
        None
    }
}

/// color-management-v1's enum values Nightjar uses.
pub mod cm {
    pub const INTENT_PERCEPTUAL: u32 = 0;
    pub const FEATURE_PARAMETRIC: u32 = 1;
    pub const FEATURE_SET_LUMINANCES: u32 = 4;
    pub const PRIMARIES_SRGB: u32 = 1;
    pub const PRIMARIES_BT2020: u32 = 6;
    pub const PRIMARIES_DCI_P3: u32 = 8;
    pub const PRIMARIES_DISPLAY_P3: u32 = 9;
    pub const PRIMARIES_ADOBE_RGB: u32 = 10;
    pub const TF_BT1886: u32 = 1;
    pub const TF_GAMMA22: u32 = 2;
    pub const TF_EXT_LINEAR: u32 = 5;
    pub const TF_SRGB: u32 = 9;
    pub const TF_EXT_SRGB: u32 = 10;
    pub const TF_ST2084_PQ: u32 = 11;
    pub const TF_HLG: u32 = 13;
    pub const TF_COMPOUND_POWER_2_4: u32 = 14;
}

use cm::*;

/// What a compositor's wp_color_manager_v1 says it supports, one bit per enum value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ColorSupport {
    pub intents: u64,
    pub features: u64,
    pub tfs: u64,
    pub primaries: u64,
}

impl ColorSupport {
    pub fn add(set: &mut u64, value: u32) {
        if value < 64 {
            *set |= 1 << value;
        }
    }

    pub fn has(set: u64, value: u32) -> bool {
        value < 64 && set & 1 << value != 0
    }
}

/// An image description as wp_image_description_info_v1 tells it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DescriptionInfo {
    pub icc: bool,
    pub tf_named: Option<u32>,
    pub tf_power: Option<u32>,
    pub primaries_named: Option<u32>,
    /// Red, green, blue and white x, y, times 1 000 000.
    pub primaries: Option<[i32; 8]>,
    /// Minimum (cd/m² × 10 000), maximum and reference white (cd/m²) of the primary volume.
    pub luminances: Option<(u32, u32, u32)>,
    /// Minimum (× 10 000) and maximum of the target volume (the display's).
    pub target_luminance: Option<(u32, u32)>,
    pub target_max_cll: Option<u32>,
}

/// BT.2020's primaries and D65, as the protocol carries them.
const BT2020_XY: [i32; 8] = [708_000, 292_000, 170_000, 797_000, 131_000, 46_000, 312_700, 329_000];

pub fn tf_name(tf: u32) -> &'static str {
    match tf {
        TF_BT1886 => "BT.1886",
        TF_GAMMA22 => "gamma 2.2",
        TF_EXT_LINEAR => "linear",
        TF_SRGB => "sRGB",
        TF_EXT_SRGB => "extended sRGB",
        TF_ST2084_PQ => "PQ",
        TF_HLG => "HLG",
        TF_COMPOUND_POWER_2_4 => "power 2.4",
        _ => "another transfer function",
    }
}

fn primaries_name(primaries: u32) -> &'static str {
    match primaries {
        PRIMARIES_SRGB => "BT.709",
        PRIMARIES_BT2020 => "BT.2020",
        PRIMARIES_DCI_P3 => "DCI-P3",
        PRIMARIES_DISPLAY_P3 => "Display P3",
        PRIMARIES_ADOBE_RGB => "Adobe RGB",
        _ => "other primaries",
    }
}

impl DescriptionInfo {
    fn bt2020(&self) -> bool {
        self.primaries_named == Some(PRIMARIES_BT2020)
            || self.primaries.is_some_and(|xy| xy.iter().zip(BT2020_XY).all(|(have, want)| (have - want).abs() <= 1_000))
    }

    /// PQ with BT.2020 primaries: what the PQ present writes.
    pub fn pq_bt2020(&self) -> bool {
        !self.icc && self.tf_named == Some(TF_ST2084_PQ) && self.bt2020()
    }

    /// Gamma 2.2 with BT.2020 primaries: KWin's blending space on an HDR output from Plasma 6.5,
    /// which the scanout present can draw in.
    pub fn blending_bt2020(&self) -> bool {
        !self.icc && self.tf_named == Some(TF_GAMMA22) && self.bt2020()
    }

    /// The display's peak in cd/m²: the target volume's, else its MaxCLL, else the primary
    /// volume's maximum where that is not PQ's fixed 10 000.
    pub fn peak(&self) -> Option<u32> {
        self.target_luminance
            .map(|(_, max)| max)
            .filter(|&max| max > 0)
            .or(self.target_max_cll.filter(|&max| max > 0))
            .or_else(|| self.luminances.filter(|_| self.tf_named != Some(TF_ST2084_PQ)).map(|(_, max, _)| max).filter(|&max| max > 0))
    }

    /// "gamma 2.2 BT.2020, reference 203 nits, peak 700 nits, minimum 0.0050 nits".
    pub fn describe(&self) -> String {
        if self.icc {
            return "an ICC profile".into();
        }
        let tf = match (self.tf_named, self.tf_power) {
            (Some(tf), _) => tf_name(tf).to_owned(),
            (None, Some(power)) => format!("power {:.2}", f64::from(power) / 10_000.0),
            (None, None) => "an unnamed transfer function".to_owned(),
        };
        let primaries = match self.primaries_named {
            Some(named) => primaries_name(named),
            None if self.bt2020() => "BT.2020",
            None if self.primaries.is_some() => "custom primaries",
            None => "unnamed primaries",
        };
        let mut text = format!("{tf} {primaries}");
        if let Some((min, _, reference)) = self.luminances {
            text += &format!(", reference {reference} nits");
            if let Some(peak) = self.peak() {
                text += &format!(", peak {peak} nits");
            }
            text += &format!(", minimum {:.4} nits", f64::from(min) / 10_000.0);
        } else if let Some(peak) = self.peak() {
            text += &format!(", peak {peak} nits");
        }
        text
    }
}

/// The luminances of Nightjar's PQ description: set_luminances' minimum (× 10 000), maximum
/// (ignored for PQ, which takes minimum + 10 000) and reference white, and MaxCLL, which the
/// compositor takes for the content's peak (0: not sent).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PqParams {
    pub min_lum: u32,
    pub max_lum: u32,
    pub reference: u32,
    pub max_cll: u32,
}

/// How Nightjar describes its BT.2020 surface, and so what its present writes ([`HdrLabel::encoding`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HdrLabel {
    /// The compositor's preferred description itself, where that is its gamma 2.2 blending space
    /// in BT.2020 (KWin 6.5 and later on an HDR output), drawn in it: the surface needs no
    /// conversion, so the compositor can scan it out without a plane colour pipeline. The
    /// luminances are the preferred ones: minimum (× 10 000), peak and reference white.
    Blending { min_lum: u32, max: u32, reference: u32 },
    /// The compositor's preferred description itself, where that already is PQ BT.2020 (KWin 6.4
    /// prefers the output's own): a pipeline to the output with nothing to do.
    Preferred,
    /// PQ BT.2020 with the preferred description's minimum and reference white and the display's
    /// peak as MaxCLL: the compositor maps nothing but the transfer function, and tone maps nothing.
    Parametric(PqParams),
    /// PQ BT.2020 at the protocol's default reference, 203 nits, as the driver describes it, with
    /// the content's MaxCLL: the compositor scales the picture with the desktop's brightness
    /// ([`Brightness::Desktop`]).
    Desktop(PqParams),
}

impl HdrLabel {
    pub fn encoding(self) -> Encoding {
        match self {
            HdrLabel::Blending { min_lum, max, .. } => Encoding::Gamma22 { min_nits: min_lum as f32 / 10_000.0, max_nits: max as f32 },
            _ => Encoding::Pq,
        }
    }

    /// The reference white the description declares, in nits: what the compositor shows at its own.
    pub fn reference(self, preferred: &DescriptionInfo) -> u32 {
        match self {
            HdrLabel::Blending { reference, .. } => reference,
            HdrLabel::Preferred => preferred.luminances.map_or(PQ_DEFAULT_REFERENCE_NITS as u32, |(_, _, reference)| reference),
            HdrLabel::Parametric(p) | HdrLabel::Desktop(p) => p.reference,
        }
    }
}

/// The description for the surface of a pass-through HDR10 swapchain on an output whose compositor
/// prefers `preferred`, at `brightness`, or why there is none (then the driver's stays). `gamma22`:
/// a gamma 2.2 BT.2020 preferred description is drawn in ([`HdrLabel::Blending`]), by what the
/// compositor reports, not by its name. `content_peak`: the host's MaxCLL as the swapchain has it.
/// Checks what would end the connection first: a request the compositor does not offer,
/// luminances not above the minimum.
pub fn hdr_label(support: &ColorSupport, preferred: &DescriptionInfo, brightness: Brightness, gamma22: bool, content_peak: Option<u32>) -> Result<HdrLabel, String> {
    if !ColorSupport::has(support.intents, INTENT_PERCEPTUAL) {
        return Err("the compositor does not offer the perceptual intent".into());
    }
    if preferred.icc {
        return Err("the compositor's preferred description is an ICC profile".into());
    }
    if brightness == Brightness::Absolute {
        if let (true, true, Some((min_lum, max, reference))) = (gamma22, preferred.blending_bt2020(), preferred.luminances)
            && u64::from(max) * 10_000 > u64::from(min_lum) && u64::from(reference) * 10_000 > u64::from(min_lum) {
                return Ok(HdrLabel::Blending { min_lum, max, reference });
            }
        if preferred.pq_bt2020() {
            return Ok(HdrLabel::Preferred);
        }
    }
    for (feature, what) in [(FEATURE_PARAMETRIC, "parametric descriptions"), (FEATURE_SET_LUMINANCES, "set_luminances")] {
        if !ColorSupport::has(support.features, feature) {
            return Err(format!("the compositor does not offer {what}"));
        }
    }
    if !ColorSupport::has(support.tfs, TF_ST2084_PQ) {
        return Err("the compositor does not take PQ".into());
    }
    if !ColorSupport::has(support.primaries, PRIMARIES_BT2020) {
        return Err("the compositor does not take BT.2020 primaries".into());
    }
    if brightness == Brightness::Desktop {
        // The driver's description: the protocol's PQ defaults (minimum 0.005 nits, reference 203).
        let min_lum = 50;
        return Ok(HdrLabel::Desktop(PqParams { min_lum, max_lum: 10_001, reference: PQ_DEFAULT_REFERENCE_NITS as u32, max_cll: content_peak.unwrap_or(0) }));
    }
    let Some((min_lum, _, reference)) = preferred.luminances else {
        return Err("the compositor's preferred description has no luminances".into());
    };
    let Some(peak) = preferred.peak() else {
        return Err("the compositor's preferred description has no peak".into());
    };
    if u64::from(reference) * 10_000 <= u64::from(min_lum) || u64::from(peak) * 10_000 <= u64::from(min_lum) {
        return Err(format!("the compositor's preferred luminances are not above their minimum ({})", preferred.describe()));
    }
    Ok(HdrLabel::Parametric(PqParams { min_lum, max_lum: 10_000 + min_lum.div_ceil(10_000), reference, max_cll: peak }))
}

/// The log's words for Nightjar's description.
pub fn describe_label(label: HdrLabel, preferred: &DescriptionInfo) -> String {
    match label {
        HdrLabel::Preferred => format!("Nightjar's, the compositor's preferred one ({})", preferred.describe()),
        HdrLabel::Parametric(p) => format!("Nightjar's, PQ BT.2020 at the output's reference {} nits and peak {} nits", p.reference, p.max_cll),
        HdrLabel::Blending { .. } => format!("Nightjar's, the compositor's blending space drawn in gamma 2.2 ({})", preferred.describe()),
        HdrLabel::Desktop(p) if p.max_cll > 0 => format!("Nightjar's, PQ BT.2020 at the default reference {} nits with MaxCLL {} nits (brightness follows the desktop)", p.reference, p.max_cll),
        HdrLabel::Desktop(p) => format!("Nightjar's, PQ BT.2020 at the default reference {} nits (brightness follows the desktop)", p.reference),
    }
}

/// Who describes the PQ renderer's surface, for the end-of-stream line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Label {
    /// Nightjar, on a pass-through swapchain ([`describe_label`]): drawn in the compositor's
    /// blending space or not, and the reference white it declares (nits).
    Ours { text: String, blending: bool, reference: u32 },
    /// The driver (Mesa: PQ BT.2020 with the protocol's default reference, 203 nits), and why.
    Driver(String),
}

static LABEL: Mutex<Option<Label>> = Mutex::new(None);

/// What the last renderer made for the main window says about its surface's description; None
/// for one that is not PQ.
pub fn set_label(label: Option<Label>) {
    *LABEL.lock().unwrap_or_else(|e| e.into_inner()) = label;
}

pub fn label() -> Option<Label> {
    LABEL.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Reference white of a PQ description without luminances (color-management-v1, BT.2408).
pub const PQ_DEFAULT_REFERENCE_NITS: f32 = 203.0;

/// The colour scale the launcher draws with on a PQ renderer (`SDL_SetRenderColorScale`). The
/// renderer draws SDR white at the window's level (`renderer_white`, in 80-nit units), the
/// compositor's reference white. The surface's description declares a reference white
/// (`described_reference`, nits): the driver's (Mesa's) is the protocol's default of 203 nits, as
/// is Nightjar's when the brightness follows the desktop, and the compositor maps it to its own,
/// so what the launcher drew comes out scaled by reference/203, right only at 203 nits. The
/// launcher (`launcher`: no stream on screen) therefore draws its white at the described
/// reference, which the compositor shows as its own; where Nightjar's description declares the
/// output's own reference that is 1.0. The stream keeps 1.0 and looks as its description says.
/// Gamescope sets its levels itself. 1.0 for an unknown white, at most a factor of 10 either way.
pub fn launcher_color_scale(pq_output: bool, gamescope: bool, described_reference: f32, launcher: bool, switched_on: bool, renderer_white: f32) -> f32 {
    if !(pq_output && !gamescope && launcher && switched_on) {
        return 1.0;
    }
    let drawn = renderer_white * 80.0;
    if !(drawn.is_finite() && drawn > 0.0 && described_reference.is_finite() && described_reference > 0.0) {
        return 1.0;
    }
    (described_reference / drawn).clamp(0.1, 10.0)
}

/// Whether a VRR stream on a Wayland desktop compositor presents IMMEDIATE instead of MAILBOX:
/// Mesa then marks the surface's tearing hint async, and KWin flips without waiting for the scan
/// out to end (AdaptiveAsync), a picture that comes during one tearing instead of waiting. Only
/// without Tear-free VRR (`vrr_tear_free`), the setting that asks for whole frames under VRR on
/// every platform: with it (the default) the stream keeps MAILBOX, which the compositor shows
/// whole. V-Sync is not asked: its row is hidden while VRR is on.
pub fn vrr_tearing(tear_free: bool, switched_on: bool) -> bool {
    !tear_free && switched_on
}

/// The swapchain's tearing for the log, from SDL's present mode number and whether the compositor
/// offers wp_tearing_control_v1 (Mesa offers IMMEDIATE only then, and marks it async).
pub fn tearing_state(present_mode: Option<i64>, offered: bool) -> String {
    match present_mode {
        Some(1) if offered => "on (Immediate, the driver's async hint)".into(),
        Some(1) => "Immediate, but the compositor offers no tearing control".into(),
        Some(2) => format!("off (Mailbox){}", if offered { "" } else { "; the compositor offers no tearing control" }),
        Some(0) => "off (V-Sync)".into(),
        _ => "unknown".into(),
    }
}

/// The swapchain formats SDL's GPU renderer uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwapFormat {
    Bgra8,
    Rgba8,
    Rgb10a2,
    Rgba16f,
}

pub const fn fourcc(code: [u8; 4]) -> u32 {
    u32::from_le_bytes(code)
}

/// The DRM format Mesa's Wayland WSI gives a swapchain of `format`: without alpha for an opaque
/// one (`wl_drm_format_for_vk_format`).
pub fn drm_fourcc(format: SwapFormat, opaque: bool) -> u32 {
    fourcc(match (format, opaque) {
        (SwapFormat::Bgra8, true) => *b"XR24",
        (SwapFormat::Bgra8, false) => *b"AR24",
        (SwapFormat::Rgba8, true) => *b"XB24",
        (SwapFormat::Rgba8, false) => *b"AB24",
        (SwapFormat::Rgb10a2, true) => *b"XB30",
        (SwapFormat::Rgb10a2, false) => *b"AB30",
        (SwapFormat::Rgba16f, true) => *b"XB4H",
        (SwapFormat::Rgba16f, false) => *b"AB4H",
    })
}

pub fn fourcc_name(code: u32) -> String {
    code.to_le_bytes().iter().map(|&b| if b.is_ascii_graphic() || b == b' ' { b as char } else { '?' }).collect::<String>().trim_end().to_owned()
}

/// The Wayland globals that bear on scanout, tearing and present timing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Globals {
    pub color_manager: u32,
    pub xx_color_manager: u32,
    pub frog_color: bool,
    pub tearing_control: bool,
    pub fifo: bool,
    pub commit_timing: bool,
    pub dmabuf: u32,
    pub syncobj: bool,
}

impl Globals {
    pub fn note(&mut self, interface: &str, version: u32) {
        match interface {
            "wp_color_manager_v1" => self.color_manager = self.color_manager.max(version),
            "xx_color_manager_v4" => self.xx_color_manager = self.xx_color_manager.max(version),
            "frog_color_management_factory_v1" => self.frog_color = true,
            "wp_tearing_control_manager_v1" => self.tearing_control = true,
            "wp_fifo_manager_v1" => self.fifo = true,
            "wp_commit_timing_manager_v1" => self.commit_timing = true,
            "zwp_linux_dmabuf_v1" => self.dmabuf = self.dmabuf.max(version),
            "wp_linux_drm_syncobj_manager_v1" => self.syncobj = true,
            _ => {}
        }
    }

    /// "offers color-manager v1, tearing-control, dmabuf v5; not fifo, commit-timing".
    pub fn describe(&self) -> String {
        let versioned = |name: &str, version: u32| (version > 0).then(|| format!("{name} v{version}"));
        let flag = |name: &str, on: bool| on.then(|| name.to_owned());
        let all = [
            ("color-manager", versioned("color-manager", self.color_manager)),
            ("xx-color-manager", versioned("xx-color-manager", self.xx_color_manager)),
            ("frog-color", flag("frog-color", self.frog_color)),
            ("tearing-control", flag("tearing-control", self.tearing_control)),
            ("fifo", flag("fifo", self.fifo)),
            ("commit-timing", flag("commit-timing", self.commit_timing)),
            ("dmabuf", versioned("dmabuf", self.dmabuf)),
            ("explicit-sync", flag("explicit-sync", self.syncobj)),
        ];
        let offered: Vec<String> = all.iter().filter_map(|(_, have)| have.clone()).collect();
        // The older colour protocols are only worth naming when they are there.
        let missing: Vec<&str> = all.iter().filter(|(name, have)| have.is_none() && !matches!(*name, "xx-color-manager" | "frog-color")).map(|(name, _)| *name).collect();
        let offered = if offered.is_empty() { "offers none of the protocols looked for".to_owned() } else { format!("offers {}", offered.join(", ")) };
        if missing.is_empty() { offered } else { format!("{offered}; not {}", missing.join(", ")) }
    }
}

/// What the surface's dmabuf feedback said over the stream.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DmabufLog {
    /// Feedback sets received (`done` events).
    pub feedbacks: u32,
    /// Of them, those with a scanout tranche.
    pub with_scanout: u32,
    /// The last scanout tranche's formats and modifiers.
    pub scanout: Vec<(u32, u64)>,
    /// The last set had one.
    pub scanout_now: bool,
}

impl DmabufLog {
    /// One feedback set: its tranches, each with its scanout flag and formats.
    pub fn add(&mut self, tranches: &[(bool, Vec<(u32, u64)>)]) {
        self.feedbacks += 1;
        let scanout: Vec<(u32, u64)> = tranches.iter().filter(|(scanout, _)| *scanout).flat_map(|(_, formats)| formats.iter().copied()).collect();
        self.scanout_now = !scanout.is_empty();
        if self.scanout_now {
            self.with_scanout += 1;
            self.scanout = scanout;
        }
    }

    /// A scanout tranche came and the swapchain's format is not in it.
    pub fn format_missing(&self, fourcc: Option<u32>) -> bool {
        self.with_scanout > 0 && fourcc.is_some_and(|format| !self.scanout.iter().any(|(have, _)| *have == format))
    }

    pub fn describe(&self, fourcc: Option<u32>) -> String {
        if self.feedbacks == 0 {
            return "none received".into();
        }
        if self.with_scanout == 0 {
            return format!("no scanout tranche in {} sets (a compositor sends one when only the buffer's format or modifier stands in the way)", self.feedbacks);
        }
        let format = match fourcc {
            Some(format) => {
                let modifiers = self.scanout.iter().filter(|(have, _)| *have == format).count();
                if modifiers > 0 { format!("{} in it with {modifiers} modifiers", fourcc_name(format)) } else { format!("{} not in it", fourcc_name(format)) }
            }
            None => "the swapchain's format unknown".into(),
        };
        format!("a scanout tranche in {} of {} sets ({} at the end), {format}", self.with_scanout, self.feedbacks, if self.scanout_now { "still there" } else { "withdrawn" })
    }
}

/// What the probe read from the compositor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seen {
    pub globals: Globals,
    pub preferred: Result<DescriptionInfo, String>,
    pub dmabuf: DmabufLog,
}

impl Default for Seen {
    fn default() -> Self {
        Self { globals: Globals::default(), preferred: Err("not asked".into()), dmabuf: DmabufLog::default() }
    }
}

/// The swapchain as SDL has it: its DRM format, present mode (SDL_GPUPresentMode's number), how
/// often it was made again for the dmabuf feedback, and whether its HDR10 is pass-through.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SwapFacts {
    pub fourcc: Option<u32>,
    pub present_mode: Option<i64>,
    pub recreates: i64,
    pub pass_through: bool,
}

/// The end-of-stream scanout line.
pub struct Report<'a> {
    pub seen: &'a Seen,
    pub label: Option<&'a Label>,
    pub facts: SwapFacts,
    /// The presentation feedback reported zero-copy at least once.
    pub zero_copy_seen: bool,
    /// The desktop is KDE's (`XDG_CURRENT_DESKTOP`), for KWin's own hints.
    pub kde: bool,
}

impl Report<'_> {
    pub fn line(&self) -> String {
        let preferred = match &self.seen.preferred {
            Ok(info) => info.describe(),
            Err(why) => format!("unknown ({why})"),
        };
        let description = match self.label {
            Some(Label::Ours { text, .. }) => text.clone(),
            Some(Label::Driver(why)) => format!("the driver's, PQ BT.2020 at the default reference {PQ_DEFAULT_REFERENCE_NITS:.0} nits ({why})"),
            None => "not PQ (the driver's, if any)".into(),
        };
        let mut format = self.facts.fourcc.map_or_else(|| "unknown".to_owned(), fourcc_name);
        if self.facts.pass_through {
            format += " (HDR10 pass-through)";
        }
        format!(
            "scanout: the compositor {}; it prefers {preferred}; HDR description {description}; dmabuf feedback {}; swapchain {format}, made again {} times for it; tearing {}; {}",
            self.seen.globals.describe(),
            self.seen.dmabuf.describe(self.facts.fourcc),
            self.facts.recreates,
            tearing_state(self.facts.present_mode, self.seen.globals.tearing_control),
            self.hints(),
        )
    }

    /// What stands against direct scanout as far as Nightjar can see.
    fn hints(&self) -> String {
        let mut hints = Vec::new();
        // The surface is PQ unless Nightjar draws it in the compositor's blending space.
        let (pq, described) = match self.label {
            Some(Label::Driver(_)) => (true, Some(("the driver's", PQ_DEFAULT_REFERENCE_NITS as u32))),
            Some(Label::Ours { blending, reference, .. }) => (!blending, Some(("Nightjar's", *reference))),
            None => (false, None),
        };
        if let Ok(preferred) = &self.seen.preferred {
            if let (true, Some(tf)) = (pq, preferred.tf_named.filter(|&tf| tf != TF_ST2084_PQ)) {
                let kwin = if self.kde { " (KWin 6.6 and later: KWIN_DRM_USE_COLOR_PIPELINE=1, with a kernel that has plane colour pipelines)" } else { "" };
                let gamma22 = if tf == TF_GAMMA22 && !env_on(GAMMA22_PRESENT_ENV) { "; NIGHTJAR_WAYLAND_GAMMA22_PRESENT is 0" } else { "" };
                hints.push(format!("the surface is PQ and the compositor blends in {}, so it converts each frame unless the display engine does{kwin}{gamma22}", tf_name(tf)));
            }
            if let (Some((whose, described)), Some((_, _, reference))) = (described, preferred.luminances)
                && reference != described {
                    hints.push(format!("{whose} description puts reference white at {described} nits and the output's is {reference}, so the compositor scales each frame"));
                }
        }
        if self.seen.dmabuf.format_missing(self.facts.fourcc) {
            hints.push("the swapchain's format is not in the scanout tranche".into());
        }
        if self.facts.fourcc.is_some_and(|f| f == fourcc(*b"XB4H") || f == fourcc(*b"AB4H")) {
            hints.push("an fp16 swapchain is converted for the display".into());
        }
        if self.kde && !self.zero_copy_seen {
            hints.push("KWin reports zero-copy only from Plasma 6.8, so 0% says nothing before it".into());
        }
        if hints.is_empty() { "nothing seen against direct scanout".into() } else { format!("against direct scanout: {}", hints.join("; ")) }
    }
}

#[cfg(target_os = "linux")]
pub use wayland::{install, installed_reference, pump_installed, uninstall, HdrDescription, Probe};

/// Nightjar's own HDR description of the window: [`HdrDescription::prepare`] reads the compositor's
/// preferred description and makes the matching one before the PQ renderer, whose swapchain is
/// then made pass-through; [`HdrDescription::attach`] gives it to the surface before the first
/// present, and [`install`](wayland::install) keeps it while that renderer lives. Every renderer
/// made for the window drops it first ([`uninstall`]), before its swapchain could bring Mesa's.
#[cfg(target_os = "linux")]
mod wayland;

#[cfg(test)]
mod tests;
