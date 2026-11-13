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
/// IMMEDIATE (tearing) for a VRR stream with V-Sync off ([`vrr_tearing`]).
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
        if let (true, true, Some((min_lum, max, reference))) = (gamma22, preferred.blending_bt2020(), preferred.luminances) {
            if u64::from(max) * 10_000 > u64::from(min_lum) && u64::from(reference) * 10_000 > u64::from(min_lum) {
                return Ok(HdrLabel::Blending { min_lum, max, reference });
            }
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
/// out to end (AdaptiveAsync), a picture that comes during one tearing instead of waiting. Only with
/// V-Sync off, the setting that accepts tearing; V-Sync on (the default) keeps MAILBOX as before.
pub fn vrr_tearing(vsync: bool, switched_on: bool) -> bool {
    !vsync && switched_on
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
            if let (Some((whose, described)), Some((_, _, reference))) = (described, preferred.luminances) {
                if reference != described {
                    hints.push(format!("{whose} description puts reference white at {described} nits and the output's is {reference}, so the compositor scales each frame"));
                }
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
mod wayland {
    use std::ffi::{c_char, c_int, c_void, CStr};
    use std::ptr::{null, null_mut};
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    use sdl3_sys::everything::*;

    use super::super::wl_presentation::wayland::{Api, Interface, Message, Proxy, Queue, Shared, DISPLAY_GET_REGISTRY, MARSHAL_FLAG_DESTROY, REGISTRY_BIND};
    use super::cm::*;
    use super::{
        brightness, content_peak, describe_label, env_on, hdr_label, set_label, Brightness, ColorSupport, DescriptionInfo, DmabufLog, HdrLabel, Label, PqParams, Seen,
        GAMMA22_PRESENT_ENV, PQ_DEFAULT_REFERENCE_NITS,
    };

    const fn message(name: &'static CStr, signature: &'static CStr, types: *const *const Interface) -> Message {
        Message { name: name.as_ptr(), signature: signature.as_ptr(), types }
    }

    const fn interface(name: &'static CStr, version: c_int, methods: &'static [Message], events: &'static [Message]) -> Interface {
        Interface {
            name: name.as_ptr(),
            version,
            method_count: methods.len() as c_int,
            methods: if methods.is_empty() { null() } else { methods.as_ptr() },
            event_count: events.len() as c_int,
            events: if events.is_empty() { null() } else { events.as_ptr() },
        }
    }

    // color-management-v1.xml and linux-dmabuf-v1.xml (versions 1 and 4): what wayland-scanner
    // would generate. Unused requests are listed so that each opcode is the protocol's.
    static NULLS: Shared<[*const Interface; 8]> = Shared([null(); 8]);

    static CM_SURFACE_TYPES: Shared<[*const Interface; 2]> = Shared([&CM_SURFACE_IF.0, null()]);
    static CM_FEEDBACK_TYPES: Shared<[*const Interface; 2]> = Shared([&CM_FEEDBACK_IF.0, null()]);
    static CM_PARAMS_TYPES: Shared<[*const Interface; 1]> = Shared([&PARAMS_IF.0]);
    static CM_REQUESTS: Shared<[Message; 7]> = Shared([
        message(c"destroy", c"", NULLS.0.as_ptr()),
        message(c"get_output", c"no", NULLS.0.as_ptr()),
        message(c"get_surface", c"no", CM_SURFACE_TYPES.0.as_ptr()),
        message(c"get_surface_feedback", c"no", CM_FEEDBACK_TYPES.0.as_ptr()),
        message(c"create_icc_creator", c"n", NULLS.0.as_ptr()),
        message(c"create_parametric_creator", c"n", CM_PARAMS_TYPES.0.as_ptr()),
        message(c"create_windows_scrgb", c"n", NULLS.0.as_ptr()),
    ]);
    static CM_EVENTS: Shared<[Message; 5]> = Shared([
        message(c"supported_intent", c"u", NULLS.0.as_ptr()),
        message(c"supported_feature", c"u", NULLS.0.as_ptr()),
        message(c"supported_tf_named", c"u", NULLS.0.as_ptr()),
        message(c"supported_primaries_named", c"u", NULLS.0.as_ptr()),
        message(c"done", c"", NULLS.0.as_ptr()),
    ]);
    static CM_IF: Shared<Interface> = Shared(interface(c"wp_color_manager_v1", 1, &CM_REQUESTS.0, &CM_EVENTS.0));
    const CM_DESTROY: u32 = 0;
    const CM_GET_SURFACE: u32 = 2;
    const CM_GET_SURFACE_FEEDBACK: u32 = 3;
    const CM_CREATE_PARAMETRIC_CREATOR: u32 = 5;

    static CM_SURFACE_REQUESTS: Shared<[Message; 3]> = Shared([
        message(c"destroy", c"", NULLS.0.as_ptr()),
        message(c"set_image_description", c"ou", NULLS.0.as_ptr()),
        message(c"unset_image_description", c"", NULLS.0.as_ptr()),
    ]);
    static CM_SURFACE_IF: Shared<Interface> = Shared(interface(c"wp_color_management_surface_v1", 1, &CM_SURFACE_REQUESTS.0, &[]));
    const SURFACE_DESTROY: u32 = 0;
    const SURFACE_SET_IMAGE_DESCRIPTION: u32 = 1;

    static IMAGE_TYPES: Shared<[*const Interface; 1]> = Shared([&IMAGE_IF.0]);
    static CM_FEEDBACK_REQUESTS: Shared<[Message; 3]> = Shared([
        message(c"destroy", c"", NULLS.0.as_ptr()),
        message(c"get_preferred", c"n", IMAGE_TYPES.0.as_ptr()),
        message(c"get_preferred_parametric", c"n", IMAGE_TYPES.0.as_ptr()),
    ]);
    static CM_FEEDBACK_EVENTS: Shared<[Message; 1]> = Shared([message(c"preferred_changed", c"u", NULLS.0.as_ptr())]);
    static CM_FEEDBACK_IF: Shared<Interface> = Shared(interface(c"wp_color_management_surface_feedback_v1", 1, &CM_FEEDBACK_REQUESTS.0, &CM_FEEDBACK_EVENTS.0));
    const FEEDBACK_DESTROY: u32 = 0;
    const FEEDBACK_GET_PREFERRED: u32 = 1;

    static INFO_TYPES: Shared<[*const Interface; 1]> = Shared([&INFO_IF.0]);
    static IMAGE_REQUESTS: Shared<[Message; 2]> = Shared([message(c"destroy", c"", NULLS.0.as_ptr()), message(c"get_information", c"n", INFO_TYPES.0.as_ptr())]);
    static IMAGE_EVENTS: Shared<[Message; 2]> = Shared([message(c"failed", c"us", NULLS.0.as_ptr()), message(c"ready", c"u", NULLS.0.as_ptr())]);
    static IMAGE_IF: Shared<Interface> = Shared(interface(c"wp_image_description_v1", 1, &IMAGE_REQUESTS.0, &IMAGE_EVENTS.0));
    const IMAGE_DESTROY: u32 = 0;
    const IMAGE_GET_INFORMATION: u32 = 1;

    static INFO_EVENTS: Shared<[Message; 11]> = Shared([
        message(c"done", c"", NULLS.0.as_ptr()),
        message(c"icc_file", c"hu", NULLS.0.as_ptr()),
        message(c"primaries", c"iiiiiiii", NULLS.0.as_ptr()),
        message(c"primaries_named", c"u", NULLS.0.as_ptr()),
        message(c"tf_power", c"u", NULLS.0.as_ptr()),
        message(c"tf_named", c"u", NULLS.0.as_ptr()),
        message(c"luminances", c"uuu", NULLS.0.as_ptr()),
        message(c"target_primaries", c"iiiiiiii", NULLS.0.as_ptr()),
        message(c"target_luminance", c"uu", NULLS.0.as_ptr()),
        message(c"target_max_cll", c"u", NULLS.0.as_ptr()),
        message(c"target_max_fall", c"u", NULLS.0.as_ptr()),
    ]);
    static INFO_IF: Shared<Interface> = Shared(interface(c"wp_image_description_info_v1", 1, &[], &INFO_EVENTS.0));

    static PARAMS_REQUESTS: Shared<[Message; 10]> = Shared([
        message(c"create", c"n", IMAGE_TYPES.0.as_ptr()),
        message(c"set_tf_named", c"u", NULLS.0.as_ptr()),
        message(c"set_tf_power", c"u", NULLS.0.as_ptr()),
        message(c"set_primaries_named", c"u", NULLS.0.as_ptr()),
        message(c"set_primaries", c"iiiiiiii", NULLS.0.as_ptr()),
        message(c"set_luminances", c"uuu", NULLS.0.as_ptr()),
        message(c"set_mastering_display_primaries", c"iiiiiiii", NULLS.0.as_ptr()),
        message(c"set_mastering_luminance", c"uu", NULLS.0.as_ptr()),
        message(c"set_max_cll", c"u", NULLS.0.as_ptr()),
        message(c"set_max_fall", c"u", NULLS.0.as_ptr()),
    ]);
    static PARAMS_IF: Shared<Interface> = Shared(interface(c"wp_image_description_creator_params_v1", 1, &PARAMS_REQUESTS.0, &[]));
    const PARAMS_CREATE: u32 = 0;
    const PARAMS_SET_TF_NAMED: u32 = 1;
    const PARAMS_SET_PRIMARIES_NAMED: u32 = 3;
    const PARAMS_SET_LUMINANCES: u32 = 5;
    const PARAMS_SET_MAX_CLL: u32 = 8;

    static DMABUF_FEEDBACK_TYPES: Shared<[*const Interface; 2]> = Shared([&DMABUF_FEEDBACK_IF.0, null()]);
    static DMABUF_REQUESTS: Shared<[Message; 4]> = Shared([
        message(c"destroy", c"", NULLS.0.as_ptr()),
        message(c"create_params", c"n", NULLS.0.as_ptr()),
        message(c"get_default_feedback", c"4n", DMABUF_FEEDBACK_TYPES.0.as_ptr()),
        message(c"get_surface_feedback", c"4no", DMABUF_FEEDBACK_TYPES.0.as_ptr()),
    ]);
    static DMABUF_EVENTS: Shared<[Message; 2]> = Shared([message(c"format", c"u", NULLS.0.as_ptr()), message(c"modifier", c"3uuu", NULLS.0.as_ptr())]);
    static DMABUF_IF: Shared<Interface> = Shared(interface(c"zwp_linux_dmabuf_v1", 4, &DMABUF_REQUESTS.0, &DMABUF_EVENTS.0));
    const DMABUF_DESTROY: u32 = 0;
    const DMABUF_GET_SURFACE_FEEDBACK: u32 = 3;

    static DMABUF_FEEDBACK_REQUESTS: Shared<[Message; 1]> = Shared([message(c"destroy", c"", NULLS.0.as_ptr())]);
    static DMABUF_FEEDBACK_EVENTS: Shared<[Message; 7]> = Shared([
        message(c"done", c"", NULLS.0.as_ptr()),
        message(c"format_table", c"hu", NULLS.0.as_ptr()),
        message(c"main_device", c"a", NULLS.0.as_ptr()),
        message(c"tranche_done", c"", NULLS.0.as_ptr()),
        message(c"tranche_target_device", c"a", NULLS.0.as_ptr()),
        message(c"tranche_formats", c"a", NULLS.0.as_ptr()),
        message(c"tranche_flags", c"u", NULLS.0.as_ptr()),
    ]);
    static DMABUF_FEEDBACK_IF: Shared<Interface> = Shared(interface(c"zwp_linux_dmabuf_feedback_v1", 4, &DMABUF_FEEDBACK_REQUESTS.0, &DMABUF_FEEDBACK_EVENTS.0));
    const DMABUF_FEEDBACK_DESTROY: u32 = 0;
    const TRANCHE_SCANOUT: u32 = 1;

    /// wl_array.
    #[repr(C)]
    struct WlArray {
        size: usize,
        alloc: usize,
        data: *mut c_void,
    }

    type Data = *mut c_void;
    type Nothing = unsafe extern "C" fn(Data, *mut Proxy);
    type OneU = unsafe extern "C" fn(Data, *mut Proxy, u32);
    type TwoU = unsafe extern "C" fn(Data, *mut Proxy, u32, u32);
    type ThreeU = unsafe extern "C" fn(Data, *mut Proxy, u32, u32, u32);
    type EightI = unsafe extern "C" fn(Data, *mut Proxy, i32, i32, i32, i32, i32, i32, i32, i32);
    type Fd = unsafe extern "C" fn(Data, *mut Proxy, i32, u32);
    type Array = unsafe extern "C" fn(Data, *mut Proxy, *mut WlArray);

    // Listeners, laid out as C does.
    #[repr(C)]
    struct RegistryListener {
        global: unsafe extern "C" fn(Data, *mut Proxy, u32, *const c_char, u32),
        global_remove: OneU,
    }
    #[repr(C)]
    struct CmListener {
        intent: OneU,
        feature: OneU,
        tf: OneU,
        primaries: OneU,
        done: Nothing,
    }
    #[repr(C)]
    struct ChangedListener {
        changed: OneU,
    }
    #[repr(C)]
    struct ImageListener {
        failed: unsafe extern "C" fn(Data, *mut Proxy, u32, *const c_char),
        ready: OneU,
    }
    #[repr(C)]
    struct InfoListener {
        done: Nothing,
        icc_file: Fd,
        primaries: EightI,
        primaries_named: OneU,
        tf_power: OneU,
        tf_named: OneU,
        luminances: ThreeU,
        target_primaries: EightI,
        target_luminance: TwoU,
        target_max_cll: OneU,
        target_max_fall: OneU,
    }
    #[repr(C)]
    struct DmabufListener {
        format: OneU,
        modifier: ThreeU,
    }
    #[repr(C)]
    struct DmabufFeedbackListener {
        done: Nothing,
        format_table: Fd,
        main_device: Array,
        tranche_done: Nothing,
        tranche_target_device: Array,
        tranche_formats: Array,
        tranche_flags: OneU,
    }

    struct RegistryData {
        globals: Vec<(u32, String, u32)>,
    }
    #[derive(Default)]
    struct CmData {
        support: ColorSupport,
        done: bool,
    }
    #[derive(Default)]
    struct ChangedData {
        changed: bool,
    }
    #[derive(Default)]
    struct ImageData {
        state: Option<Result<(), String>>,
    }
    #[derive(Default)]
    struct InfoData {
        info: DescriptionInfo,
        done: bool,
    }
    #[derive(Default)]
    struct DmabufData {
        table: Vec<(u32, u64)>,
        tranches: Vec<(bool, Vec<(u32, u64)>)>,
        flags: u32,
        indices: Vec<u16>,
        log: DmabufLog,
    }

    // SAFETY (all listeners): `data` is the box the listener was added with, which outlives its
    // proxy; libwayland calls them from the dispatch of our private queue, on the thread
    // dispatching it, never at the same time as anything else touching the box.
    unsafe extern "C" fn registry_global(data: Data, _: *mut Proxy, name: u32, interface: *const c_char, version: u32) {
        let data = unsafe { &mut *data.cast::<RegistryData>() };
        let interface = unsafe { CStr::from_ptr(interface) }.to_string_lossy().into_owned();
        data.globals.push((name, interface, version));
    }
    unsafe extern "C" fn ignore_u(_: Data, _: *mut Proxy, _: u32) {}
    unsafe extern "C" fn ignore_uuu(_: Data, _: *mut Proxy, _: u32, _: u32, _: u32) {}
    unsafe extern "C" fn ignore_array(_: Data, _: *mut Proxy, _: *mut WlArray) {}
    unsafe extern "C" fn cm_intent(data: Data, _: *mut Proxy, value: u32) {
        ColorSupport::add(unsafe { &mut (*data.cast::<CmData>()).support.intents }, value);
    }
    unsafe extern "C" fn cm_feature(data: Data, _: *mut Proxy, value: u32) {
        ColorSupport::add(unsafe { &mut (*data.cast::<CmData>()).support.features }, value);
    }
    unsafe extern "C" fn cm_tf(data: Data, _: *mut Proxy, value: u32) {
        ColorSupport::add(unsafe { &mut (*data.cast::<CmData>()).support.tfs }, value);
    }
    unsafe extern "C" fn cm_primaries(data: Data, _: *mut Proxy, value: u32) {
        ColorSupport::add(unsafe { &mut (*data.cast::<CmData>()).support.primaries }, value);
    }
    unsafe extern "C" fn cm_done(data: Data, _: *mut Proxy) {
        unsafe { (*data.cast::<CmData>()).done = true };
    }
    unsafe extern "C" fn preferred_changed(data: Data, _: *mut Proxy, _identity: u32) {
        unsafe { (*data.cast::<ChangedData>()).changed = true };
    }
    unsafe extern "C" fn image_failed(data: Data, _: *mut Proxy, cause: u32, message: *const c_char) {
        let message = if message.is_null() { String::new() } else { unsafe { CStr::from_ptr(message) }.to_string_lossy().into_owned() };
        unsafe { (*data.cast::<ImageData>()).state = Some(Err(format!("the compositor refused the description (cause {cause}: {message})"))) };
    }
    unsafe extern "C" fn image_ready(data: Data, _: *mut Proxy, _identity: u32) {
        unsafe { (*data.cast::<ImageData>()).state = Some(Ok(())) };
    }
    fn info<'a>(data: Data) -> &'a mut InfoData {
        // SAFETY: as for every listener.
        unsafe { &mut *data.cast::<InfoData>() }
    }
    unsafe extern "C" fn info_done(data: Data, _: *mut Proxy) {
        info(data).done = true;
    }
    unsafe extern "C" fn info_icc(data: Data, _: *mut Proxy, fd: i32, _size: u32) {
        info(data).info.icc = true;
        // SAFETY: the event hands us the descriptor.
        unsafe { libc::close(fd) };
    }
    #[allow(clippy::too_many_arguments)]
    unsafe extern "C" fn info_primaries(data: Data, _: *mut Proxy, rx: i32, ry: i32, gx: i32, gy: i32, bx: i32, by: i32, wx: i32, wy: i32) {
        info(data).info.primaries = Some([rx, ry, gx, gy, bx, by, wx, wy]);
    }
    unsafe extern "C" fn info_primaries_named(data: Data, _: *mut Proxy, value: u32) {
        info(data).info.primaries_named = Some(value);
    }
    unsafe extern "C" fn info_tf_power(data: Data, _: *mut Proxy, value: u32) {
        info(data).info.tf_power = Some(value);
    }
    unsafe extern "C" fn info_tf_named(data: Data, _: *mut Proxy, value: u32) {
        info(data).info.tf_named = Some(value);
    }
    unsafe extern "C" fn info_luminances(data: Data, _: *mut Proxy, min: u32, max: u32, reference: u32) {
        info(data).info.luminances = Some((min, max, reference));
    }
    #[allow(clippy::too_many_arguments)]
    unsafe extern "C" fn info_target_primaries(_: Data, _: *mut Proxy, _: i32, _: i32, _: i32, _: i32, _: i32, _: i32, _: i32, _: i32) {}
    unsafe extern "C" fn info_target_luminance(data: Data, _: *mut Proxy, min: u32, max: u32) {
        info(data).info.target_luminance = Some((min, max));
    }
    unsafe extern "C" fn info_target_max_cll(data: Data, _: *mut Proxy, value: u32) {
        info(data).info.target_max_cll = Some(value);
    }
    fn dmabuf<'a>(data: Data) -> &'a mut DmabufData {
        // SAFETY: as for every listener.
        unsafe { &mut *data.cast::<DmabufData>() }
    }
    unsafe extern "C" fn feedback_done(data: Data, _: *mut Proxy) {
        let data = dmabuf(data);
        let tranches = std::mem::take(&mut data.tranches);
        data.log.add(&tranches);
    }
    unsafe extern "C" fn feedback_format_table(data: Data, _: *mut Proxy, fd: i32, size: u32) {
        let data = dmabuf(data);
        data.table.clear();
        let size = size as usize;
        if size > 0 {
            // SAFETY: the compositor's table, read only, unmapped before the descriptor closes;
            // 16 bytes per entry: format (u32), padding, modifier (u64).
            unsafe {
                let map = libc::mmap(null_mut(), size, libc::PROT_READ, libc::MAP_PRIVATE, fd, 0);
                if map != libc::MAP_FAILED {
                    let bytes = std::slice::from_raw_parts(map.cast::<u8>(), size);
                    data.table = bytes
                        .as_chunks::<16>()
                        .0
                        .iter()
                        .map(|entry| (u32::from_ne_bytes(entry[..4].try_into().unwrap()), u64::from_ne_bytes(entry[8..].try_into().unwrap())))
                        .collect();
                    libc::munmap(map, size);
                }
            }
        }
        // SAFETY: the event hands us the descriptor.
        unsafe { libc::close(fd) };
    }
    unsafe extern "C" fn feedback_tranche_done(data: Data, _: *mut Proxy) {
        let data = dmabuf(data);
        let formats = data.indices.drain(..).filter_map(|index| data.table.get(usize::from(index)).copied()).collect();
        data.tranches.push((data.flags & TRANCHE_SCANOUT != 0, formats));
        data.flags = 0;
    }
    unsafe extern "C" fn feedback_tranche_formats(data: Data, _: *mut Proxy, array: *mut WlArray) {
        // SAFETY: libwayland's array of u16 indices, valid during the call.
        let indices = unsafe {
            let array = &*array;
            if array.data.is_null() { &[][..] } else { std::slice::from_raw_parts(array.data.cast::<u16>(), array.size / 2) }
        };
        dmabuf(data).indices.extend_from_slice(indices);
    }
    unsafe extern "C" fn feedback_tranche_flags(data: Data, _: *mut Proxy, flags: u32) {
        dmabuf(data).flags = flags;
    }

    static REGISTRY_LISTENER: RegistryListener = RegistryListener { global: registry_global, global_remove: ignore_u };
    static CM_LISTENER: CmListener = CmListener { intent: cm_intent, feature: cm_feature, tf: cm_tf, primaries: cm_primaries, done: cm_done };
    static CHANGED_LISTENER: ChangedListener = ChangedListener { changed: preferred_changed };
    static IMAGE_LISTENER: ImageListener = ImageListener { failed: image_failed, ready: image_ready };
    static INFO_LISTENER: InfoListener = InfoListener {
        done: info_done,
        icc_file: info_icc,
        primaries: info_primaries,
        primaries_named: info_primaries_named,
        tf_power: info_tf_power,
        tf_named: info_tf_named,
        luminances: info_luminances,
        target_primaries: info_target_primaries,
        target_luminance: info_target_luminance,
        target_max_cll: info_target_max_cll,
        target_max_fall: ignore_u,
    };
    static DMABUF_LISTENER: DmabufListener = DmabufListener { format: ignore_u, modifier: ignore_uuu };
    static DMABUF_FEEDBACK_LISTENER: DmabufFeedbackListener = DmabufFeedbackListener {
        done: feedback_done,
        format_table: feedback_format_table,
        main_device: ignore_array,
        tranche_done: feedback_tranche_done,
        tranche_target_device: ignore_array,
        tranche_formats: feedback_tranche_formats,
        tranche_flags: feedback_tranche_flags,
    };

    /// SDL's Wayland connection with a private event queue (as `wl_presentation::Feedback` has):
    /// SDL's event thread never dispatches what is made here. Dropped after what it made.
    struct Conn {
        api: Box<Api>,
        display: *mut c_void,
        surface: *mut Proxy,
        queue: *mut Queue,
        registry: *mut Proxy,
        registry_data: Box<RegistryData>,
    }

    impl Conn {
        fn open(window: *mut SDL_Window) -> Result<Self, String> {
            // SAFETY: a live window; the pointers are SDL's own Wayland objects.
            let (display, surface) = unsafe {
                let props = SDL_GetWindowProperties(window);
                (
                    SDL_GetPointerProperty(props, SDL_PROP_WINDOW_WAYLAND_DISPLAY_POINTER, null_mut()),
                    SDL_GetPointerProperty(props, SDL_PROP_WINDOW_WAYLAND_SURFACE_POINTER, null_mut()),
                )
            };
            if display.is_null() || surface.is_null() {
                return Err("the window is not a Wayland one".into());
            }
            let api = Box::new(Api::load()?);
            // SAFETY: SDL's live display; every object made here belongs to the private queue.
            unsafe {
                let queue = (api.create_queue)(display);
                if queue.is_null() {
                    return Err("no Wayland event queue".into());
                }
                let mut conn = Conn { api, display, surface, queue, registry: null_mut(), registry_data: Box::new(RegistryData { globals: Vec::new() }) };
                let wrapper = (conn.api.create_wrapper)(display);
                if wrapper.is_null() {
                    return Err("no Wayland display wrapper".into());
                }
                (conn.api.set_queue)(wrapper, queue);
                let version = (conn.api.get_version)(wrapper);
                conn.registry = (conn.api.marshal_flags)(wrapper, DISPLAY_GET_REGISTRY, conn.api.registry_interface, version, 0, null_mut::<c_void>());
                (conn.api.wrapper_destroy)(wrapper);
                if conn.registry.is_null() {
                    return Err("no Wayland registry".into());
                }
                let data: *mut RegistryData = &mut *conn.registry_data;
                conn.listen(conn.registry, &REGISTRY_LISTENER, data.cast());
                if !conn.roundtrip() {
                    return Err("the Wayland registry round trip failed".into());
                }
                Ok(conn)
            }
        }

        fn globals(&self) -> &[(u32, String, u32)] {
            &self.registry_data.globals
        }

        /// Binds the global `interface` names at its version, at most `max_version`.
        unsafe fn bind(&self, interface: &'static Interface, max_version: u32) -> Option<*mut Proxy> {
            // SAFETY: a static NUL-terminated name.
            let wanted = unsafe { CStr::from_ptr(interface.name) }.to_str().ok()?;
            let &(name, _, version) = self.globals().iter().find(|(_, have, _)| have == wanted)?;
            let version = version.min(max_version);
            // SAFETY: our live registry; wl_registry.bind's "usun".
            let bound = unsafe { (self.api.marshal_flags)(self.registry, REGISTRY_BIND, interface, version, 0, name, interface.name, version, null_mut::<c_void>()) };
            (!bound.is_null()).then_some(bound)
        }

        unsafe fn listen<L>(&self, proxy: *mut Proxy, listener: &'static L, data: Data) {
            // SAFETY: a live proxy of ours and the listener table of its interface.
            unsafe { (self.api.add_listener)(proxy, (listener as *const L).cast(), data) };
        }

        fn version(&self, proxy: *mut Proxy) -> u32 {
            // SAFETY: a live proxy of ours.
            unsafe { (self.api.get_version)(proxy) }
        }

        fn roundtrip(&self) -> bool {
            // SAFETY: SDL's live display and our queue.
            unsafe { (self.api.roundtrip_queue)(self.display, self.queue) >= 0 }
        }

        /// Reads what arrived without blocking and dispatches our queue.
        fn pump(&self) {
            // SAFETY: SDL's live display and our queue; the read protocol shared with SDL's thread.
            unsafe {
                if (self.api.prepare_read_queue)(self.display, self.queue) == 0 {
                    (self.api.flush)(self.display);
                    let mut fd = libc::pollfd { fd: (self.api.get_fd)(self.display), events: libc::POLLIN, revents: 0 };
                    if libc::poll(&mut fd, 1, 0) > 0 && fd.revents & libc::POLLIN != 0 {
                        (self.api.read_events)(self.display);
                    } else {
                        (self.api.cancel_read)(self.display);
                    }
                }
                (self.api.dispatch_queue_pending)(self.display, self.queue);
            }
        }

        fn flush(&self) {
            // SAFETY: SDL's live display.
            unsafe { (self.api.flush)(self.display) };
        }

        /// Sends `proxy`'s destructor request `opcode` (0 for all of these protocols); null is none.
        fn destroy(&self, proxy: *mut Proxy, opcode: u32) {
            if !proxy.is_null() {
                // SAFETY: a live proxy of ours, destroyed once.
                unsafe { (self.api.marshal_flags)(proxy, opcode, null(), (self.api.get_version)(proxy), MARSHAL_FLAG_DESTROY) };
            }
        }
    }

    impl Drop for Conn {
        fn drop(&mut self) {
            // SAFETY: our own objects, destroyed once, after those made from them.
            unsafe {
                if !self.registry.is_null() {
                    (self.api.destroy)(self.registry);
                }
                (self.api.flush)(self.display);
                (self.api.queue_destroy)(self.queue);
            }
        }
    }

    /// A proxy with its listener's box.
    struct Object<T> {
        proxy: *mut Proxy,
        data: Box<T>,
    }

    impl<T: Default> Object<T> {
        unsafe fn new<L>(conn: &Conn, proxy: *mut Proxy, listener: &'static L) -> Option<Self> {
            if proxy.is_null() {
                return None;
            }
            let mut data = Box::<T>::default();
            // SAFETY: the box outlives the proxy (destroyed through `Conn::destroy` first).
            unsafe { conn.listen(proxy, listener, (&mut *data as *mut T).cast()) };
            Some(Self { proxy, data })
        }

        fn data(&mut self) -> *mut T {
            &mut *self.data
        }
    }

    /// wp_color_manager_v1 at version 1, with what it supports.
    unsafe fn manager(conn: &Conn) -> Result<Object<CmData>, String> {
        let proxy = unsafe { conn.bind(&CM_IF.0, 1) }.ok_or("the compositor offers no wp_color_manager_v1")?;
        let mut manager = unsafe { Object::<CmData>::new(conn, proxy, &CM_LISTENER) }.ok_or("wp_color_manager_v1 could not be bound")?;
        let data = manager.data();
        // SAFETY: the box, read between dispatches.
        if !conn.roundtrip() || !unsafe { (*data).done } {
            conn.destroy(manager.proxy, CM_DESTROY);
            return Err("wp_color_manager_v1 told nothing it supports".into());
        }
        Ok(manager)
    }

    unsafe fn color_feedback(conn: &Conn, manager: &Object<CmData>) -> Result<Object<ChangedData>, String> {
        // SAFETY: a live manager; get_surface_feedback's "no" with SDL's surface.
        let proxy = unsafe { (conn.api.marshal_flags)(manager.proxy, CM_GET_SURFACE_FEEDBACK, &CM_FEEDBACK_IF.0, conn.version(manager.proxy), 0, null_mut::<c_void>(), conn.surface) };
        unsafe { Object::new(conn, proxy, &CHANGED_LISTENER) }.ok_or_else(|| "no colour surface feedback".into())
    }

    /// Waits a few round trips for an image description's ready or failed; on failure it is gone.
    unsafe fn ready(conn: &Conn, image: Option<Object<ImageData>>) -> Result<Object<ImageData>, String> {
        let mut image = image.ok_or("no image description")?;
        let data = image.data();
        for _ in 0..4 {
            // SAFETY: the box, read between dispatches.
            match unsafe { (*data).state.clone() } {
                Some(Ok(())) => return Ok(image),
                Some(Err(why)) => {
                    conn.destroy(image.proxy, IMAGE_DESTROY);
                    return Err(why);
                }
                None if !conn.roundtrip() => break,
                None => {}
            }
        }
        conn.destroy(image.proxy, IMAGE_DESTROY);
        Err("the compositor did not answer for the image description".into())
    }

    /// The compositor's preferred description for the surface, and what it says.
    unsafe fn preferred(conn: &Conn, feedback: &Object<ChangedData>) -> Result<(Object<ImageData>, DescriptionInfo), String> {
        // SAFETY: a live feedback object; get_preferred's "n".
        let proxy = unsafe { (conn.api.marshal_flags)(feedback.proxy, FEEDBACK_GET_PREFERRED, &IMAGE_IF.0, conn.version(feedback.proxy), 0, null_mut::<c_void>()) };
        let image = unsafe { ready(conn, Object::new(conn, proxy, &IMAGE_LISTENER)) }?;
        // SAFETY: a ready description; get_information's "n".
        let proxy = unsafe { (conn.api.marshal_flags)(image.proxy, IMAGE_GET_INFORMATION, &INFO_IF.0, conn.version(image.proxy), 0, null_mut::<c_void>()) };
        let Some(mut info) = (unsafe { Object::<InfoData>::new(conn, proxy, &INFO_LISTENER) }) else {
            conn.destroy(image.proxy, IMAGE_DESTROY);
            return Err("no description information".into());
        };
        let data = info.data();
        for _ in 0..4 {
            // SAFETY: the box, read between dispatches.
            if unsafe { (*data).done } || !conn.roundtrip() {
                break;
            }
        }
        // The info object ends with its done event; the client only frees its proxy.
        // SAFETY: our proxy, destroyed once.
        unsafe { (conn.api.destroy)(info.proxy) };
        // SAFETY: as above.
        let (done, described) = unsafe { ((*data).done, (*data).info) };
        if !done {
            conn.destroy(image.proxy, IMAGE_DESTROY);
            return Err("the compositor did not describe its preferred description".into());
        }
        Ok((image, described))
    }

    /// PQ BT.2020 with `params`' luminances.
    unsafe fn create_pq(conn: &Conn, manager: &Object<CmData>, params: PqParams) -> Result<Object<ImageData>, String> {
        let version = conn.version(manager.proxy);
        // SAFETY: a live manager whose features `hdr_label` checked; each request's signature.
        unsafe {
            let creator = (conn.api.marshal_flags)(manager.proxy, CM_CREATE_PARAMETRIC_CREATOR, &PARAMS_IF.0, version, 0, null_mut::<c_void>());
            if creator.is_null() {
                return Err("no parametric creator".into());
            }
            (conn.api.marshal_flags)(creator, PARAMS_SET_TF_NAMED, null(), version, 0, TF_ST2084_PQ);
            (conn.api.marshal_flags)(creator, PARAMS_SET_PRIMARIES_NAMED, null(), version, 0, PRIMARIES_BT2020);
            (conn.api.marshal_flags)(creator, PARAMS_SET_LUMINANCES, null(), version, 0, params.min_lum, params.max_lum, params.reference);
            if params.max_cll > 0 {
                (conn.api.marshal_flags)(creator, PARAMS_SET_MAX_CLL, null(), version, 0, params.max_cll);
            }
            // create is the creator's destructor.
            let proxy = (conn.api.marshal_flags)(creator, PARAMS_CREATE, &IMAGE_IF.0, version, MARSHAL_FLAG_DESTROY, null_mut::<c_void>());
            ready(conn, Object::new(conn, proxy, &IMAGE_LISTENER))
        }
    }

    /// Nightjar's description of a pass-through HDR10 swapchain's surface (see the module above).
    pub struct HdrDescription {
        window: usize,
        surface_object: *mut Proxy,
        image: Option<Object<ImageData>>,
        feedback: Option<Object<ChangedData>>,
        manager: Option<Object<CmData>>,
        label: Option<HdrLabel>,
        preferred: DescriptionInfo,
        /// What the label was made for: the brightness, the gamma 2.2 switch and the content's peak.
        made_for: Option<(Brightness, bool, Option<u32>)>,
        refreshed: Instant,
        pumps: u32,
        conn: Conn,
    }

    // SAFETY: libwayland's proxies may be used from any thread; the description is used from one
    // at a time (behind INSTALLED's lock).
    unsafe impl Send for HdrDescription {}

    /// How often the compositor's preferred description is read again after it said it changed
    /// (KWin says so on every step of a brightness animation).
    const REFRESH_EVERY: Duration = Duration::from_millis(500);

    /// What the description is made for now: the coming stream's brightness, the gamma 2.2 switch
    /// and the host's content peak.
    fn wanted() -> (Brightness, bool, Option<u32>) {
        (brightness(), env_on(GAMMA22_PRESENT_ENV), content_peak())
    }

    impl HdrDescription {
        /// Reads the compositor's preferred description for `window` and makes the matching one,
        /// ready to [`attach`](Self::attach); or why not.
        pub fn prepare(window: *mut SDL_Window) -> Result<Self, String> {
            let conn = Conn::open(window)?;
            let mut description = HdrDescription {
                window: window as usize,
                surface_object: null_mut(),
                image: None,
                feedback: None,
                manager: None,
                label: None,
                preferred: DescriptionInfo::default(),
                made_for: None,
                refreshed: Instant::now(),
                pumps: 0,
                conn,
            };
            // SAFETY: objects of our connection; a failure drops what was made (Drop).
            unsafe {
                description.manager = Some(manager(&description.conn)?);
                description.feedback = Some(color_feedback(&description.conn, description.manager.as_ref().unwrap())?);
                description.refresh()?;
            }
            Ok(description)
        }

        /// Reads the preferred description and makes ours from it, set on the surface when
        /// attached, with the present's encoding to match. Whether it changed.
        unsafe fn refresh(&mut self) -> Result<bool, String> {
            let (Some(manager), Some(feedback)) = (&self.manager, &self.feedback) else { return Err("no colour manager".into()) };
            let (preferred_image, info) = unsafe { preferred(&self.conn, feedback) }?;
            let made_for = wanted();
            let (brightness, gamma22, content_peak) = made_for;
            let label = match hdr_label(&manager.data.support, &info, brightness, gamma22, content_peak) {
                Ok(label) => label,
                Err(why) => {
                    self.conn.destroy(preferred_image.proxy, IMAGE_DESTROY);
                    return Err(why);
                }
            };
            let image = match label {
                HdrLabel::Preferred | HdrLabel::Blending { .. } => preferred_image,
                HdrLabel::Parametric(params) | HdrLabel::Desktop(params) => {
                    self.conn.destroy(preferred_image.proxy, IMAGE_DESTROY);
                    if self.label == Some(label) && self.image.is_some() {
                        self.preferred = info;
                        self.made_for = Some(made_for);
                        return Ok(false);
                    }
                    unsafe { create_pq(&self.conn, manager, params) }?
                }
            };
            if !self.surface_object.is_null() {
                // The present's encoding and the description change together, from the next frame.
                self.encode(label);
                // SAFETY: our colour surface and a ready description; "ou".
                unsafe { (self.conn.api.marshal_flags)(self.surface_object, SURFACE_SET_IMAGE_DESCRIPTION, null(), self.conn.version(self.surface_object), 0, image.proxy, INTENT_PERCEPTUAL) };
                self.conn.flush();
            }
            if let Some(old) = self.image.replace(image) {
                self.conn.destroy(old.proxy, IMAGE_DESTROY);
            }
            self.label = Some(label);
            self.preferred = info;
            self.made_for = Some(made_for);
            if !self.surface_object.is_null() {
                set_label(Some(self.log_label()));
            }
            Ok(true)
        }

        /// Hands the window's renderer the encoding `label` asks of its present.
        fn encode(&self, label: HdrLabel) {
            // SAFETY: SDL's window lookup; the renderer's properties take any thread.
            let renderer = unsafe { SDL_GetRenderer(self.window as *mut SDL_Window) };
            crate::sdl::set_present_encoding(renderer, label.encoding());
        }

        /// Gives the surface the description, from its next commit (the swapchain's first
        /// present), and the window's renderer the encoding to match. Only on a pass-through
        /// swapchain: Mesa then has no colour surface on it.
        pub fn attach(&mut self) {
            let (Some(manager), Some(image), Some(label)) = (&self.manager, &self.image, self.label) else { return };
            if !self.surface_object.is_null() {
                return;
            }
            self.encode(label);
            // SAFETY: a live manager, SDL's surface and a ready description.
            unsafe {
                let version = self.conn.version(manager.proxy);
                self.surface_object = (self.conn.api.marshal_flags)(manager.proxy, CM_GET_SURFACE, &CM_SURFACE_IF.0, version, 0, null_mut::<c_void>(), self.conn.surface);
                if !self.surface_object.is_null() {
                    (self.conn.api.marshal_flags)(self.surface_object, SURFACE_SET_IMAGE_DESCRIPTION, null(), version, 0, image.proxy, INTENT_PERCEPTUAL);
                }
            }
            self.conn.flush();
        }

        /// Follows the compositor's preferred description (another output, its brightness) and the
        /// stream's brightness; a line for the log when ours changed or could not.
        fn pump(&mut self) -> Option<String> {
            self.pumps = self.pumps.wrapping_add(1);
            if !self.pumps.is_multiple_of(8) {
                return None;
            }
            self.conn.pump();
            let feedback = self.feedback.as_mut()?;
            let changed = feedback.data();
            // SAFETY: the box, read between dispatches.
            let output_changed = unsafe { (*changed).changed } && self.refreshed.elapsed() >= REFRESH_EVERY;
            if !output_changed && self.made_for == Some(wanted()) {
                return None;
            }
            // SAFETY: as above.
            unsafe { (*changed).changed = false };
            self.refreshed = Instant::now();
            // SAFETY: objects of our connection.
            match unsafe { self.refresh() } {
                Ok(true) => Some(format!("HDR description follows the output: {}", self.describe())),
                Ok(false) => None,
                Err(why) => {
                    // Not again for this state: the next output change tries again.
                    self.made_for = Some(wanted());
                    Some(format!("HDR description stays {} ({why})", self.describe()))
                }
            }
        }

        pub fn describe(&self) -> String {
            self.label.map_or_else(|| "none".into(), |label| describe_label(label, &self.preferred))
        }

        /// The description for the end-of-stream line.
        pub fn log_label(&self) -> Label {
            Label::Ours {
                text: self.describe(),
                blending: matches!(self.label, Some(HdrLabel::Blending { .. })),
                reference: self.label.map_or(PQ_DEFAULT_REFERENCE_NITS as u32, |label| label.reference(&self.preferred)),
            }
        }

        /// The reference white the description declares, in nits.
        fn reference(&self) -> Option<f32> {
            self.label.map(|label| label.reference(&self.preferred) as f32)
        }
    }

    impl Drop for HdrDescription {
        fn drop(&mut self) {
            // Destroying the colour surface unsets the description from the surface's next commit.
            self.conn.destroy(self.surface_object, SURFACE_DESTROY);
            if let Some(image) = self.image.take() {
                self.conn.destroy(image.proxy, IMAGE_DESTROY);
            }
            if let Some(feedback) = self.feedback.take() {
                self.conn.destroy(feedback.proxy, FEEDBACK_DESTROY);
            }
            if let Some(manager) = self.manager.take() {
                self.conn.destroy(manager.proxy, CM_DESTROY);
            }
            // `conn` flushes and goes after this.
        }
    }

    static INSTALLED: Mutex<Option<HdrDescription>> = Mutex::new(None);

    /// Keeps `description` while its renderer lives.
    pub fn install(description: HdrDescription) {
        *INSTALLED.lock().unwrap_or_else(|e| e.into_inner()) = Some(description);
    }

    /// Drops the description of `window`, if it has one: before any renderer is made for it.
    pub fn uninstall(window: *mut SDL_Window) -> bool {
        let mut installed = INSTALLED.lock().unwrap_or_else(|e| e.into_inner());
        if installed.as_ref().is_some_and(|description| description.window == window as usize) {
            *installed = None;
            true
        } else {
            false
        }
    }

    /// The reference white Nightjar's description of `window` declares (nits), Some(None) without
    /// one. Never waits for the drawing thread: None while it holds the description.
    pub fn installed_reference(window: *mut SDL_Window) -> Option<Option<f32>> {
        let installed = INSTALLED.try_lock().ok()?;
        Some(installed.as_ref().filter(|description| description.window == window as usize).and_then(HdrDescription::reference))
    }

    /// Lets the installed description follow the output, from whichever thread draws (never
    /// waiting for the other).
    pub fn pump_installed() {
        let Ok(mut installed) = INSTALLED.try_lock() else { return };
        if let Some(line) = installed.as_mut().and_then(HdrDescription::pump) {
            eprintln!("nightjar: {line}");
        }
    }

    /// The stream's look at the compositor: its globals, the preferred description, and the
    /// surface's dmabuf feedback over the stream.
    pub struct Probe {
        seen: Seen,
        dmabuf: *mut Proxy,
        feedback: Option<Object<DmabufData>>,
        polls: u32,
        conn: Conn,
    }

    // SAFETY: as for HdrDescription; used from the presenting thread.
    unsafe impl Send for Probe {}

    impl Probe {
        pub fn open(window: *mut SDL_Window) -> Result<Self, String> {
            let conn = Conn::open(window)?;
            let mut probe = Probe { seen: Seen::default(), dmabuf: null_mut(), feedback: None, polls: 0, conn };
            for (_, interface, version) in probe.conn.globals() {
                probe.seen.globals.note(interface, *version);
            }
            // SAFETY: objects of our connection, destroyed in Drop.
            unsafe {
                probe.seen.preferred = probe.read_preferred();
                if probe.seen.globals.dmabuf >= 4 {
                    if let Some(dmabuf) = probe.conn.bind(&DMABUF_IF.0, 4) {
                        probe.dmabuf = dmabuf;
                        probe.conn.listen(dmabuf, &DMABUF_LISTENER, null_mut());
                        let proxy = (probe.conn.api.marshal_flags)(dmabuf, DMABUF_GET_SURFACE_FEEDBACK, &DMABUF_FEEDBACK_IF.0, 4, 0, null_mut::<c_void>(), probe.conn.surface);
                        probe.feedback = Object::new(&probe.conn, proxy, &DMABUF_FEEDBACK_LISTENER);
                        probe.conn.roundtrip();
                    }
                }
            }
            probe.sync();
            Ok(probe)
        }

        /// The preferred description, through objects made and dropped here.
        unsafe fn read_preferred(&self) -> Result<DescriptionInfo, String> {
            let manager = unsafe { manager(&self.conn) }?;
            let read = unsafe { color_feedback(&self.conn, &manager) }.and_then(|feedback| {
                let answer = unsafe { preferred(&self.conn, &feedback) };
                self.conn.destroy(feedback.proxy, FEEDBACK_DESTROY);
                answer.map(|(image, info)| {
                    self.conn.destroy(image.proxy, IMAGE_DESTROY);
                    info
                })
            });
            self.conn.destroy(manager.proxy, CM_DESTROY);
            read
        }

        fn sync(&mut self) {
            if let Some(feedback) = &self.feedback {
                self.seen.dmabuf = feedback.data.log.clone();
            }
        }

        /// Reads what arrived, every 16th call (a poll per call would be wasted).
        pub fn poll(&mut self) {
            self.polls = self.polls.wrapping_add(1);
            if self.polls.is_multiple_of(16) {
                self.conn.pump();
                self.sync();
            }
        }

        /// What was seen, up to now.
        pub fn seen(&mut self) -> &Seen {
            self.conn.pump();
            self.sync();
            &self.seen
        }
    }

    impl Drop for Probe {
        fn drop(&mut self) {
            if let Some(feedback) = self.feedback.take() {
                self.conn.destroy(feedback.proxy, DMABUF_FEEDBACK_DESTROY);
            }
            self.conn.destroy(self.dmabuf, DMABUF_DESTROY);
        }
    }
}

#[cfg(test)]
mod tests;
