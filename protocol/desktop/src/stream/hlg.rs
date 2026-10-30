//! HLG (BT.2100) for the display at hand, shared by the Windows draw pass (`direct`) and the
//! plane shader (`present::planar_parameters`).

/// HLG's reference white in nits on its 1000-nit reference display (ITU-R BT.2408).
const HLG_REFERENCE_WHITE: f32 = 203.0;

/// The display an HLG picture is drawn for: BT.2100's OOTF peak and system gamma, and the white
/// the result is relative to, all in nits.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HlgDisplay { pub peak: f32, pub gamma: f32, pub white: f32 }

/// HLG on an output with `headroom` over an SDR white of `white_nits`: an HDR output takes its
/// own peak, as BT.2100 renders HLG for the display at hand. An SDR output takes the 1000-nit
/// reference display with HLG's reference white at SDR white, and the SDR tone map (as PQ gets)
/// brings the highlights into it. The gamma follows BT.2100 (1.2 at 1000 nits), not below 1 for
/// dim displays, as mpv's OOTF does.
pub fn hlg_display(headroom: f32, white_nits: f32) -> HlgDisplay {
    let (peak, white) = if headroom > 1.0 && white_nits.is_finite() && white_nits > 0.0 && (headroom * white_nits).is_finite() {
        ((headroom * white_nits).min(10000.0), white_nits)
    } else {
        (1000.0, HLG_REFERENCE_WHITE)
    };
    HlgDisplay { peak, gamma: (1.2 + 0.42 * (peak / 1000.0).log10()).max(1.0), white }
}

/// Display light in nits of HLG codes `rgb` (BT.2020) on `display`: the inverse OETF to scene
/// light, then the OOTF `peak * Ys^(gamma - 1) * E` (BT.2100, black level 0). The shader's
/// `hlgToNits`; the tests hold the two together.
#[cfg(all(test, windows))]
pub fn hlg_nits(rgb: [f64; 3], display: HlgDisplay) -> [f64; 3] {
    let (a, b, c) = (0.17883277, 0.28466892, 0.55991073);
    let scene = rgb.map(|v| {
        let v = v.clamp(0.0, 1.0);
        if v <= 0.5 { v * v / 3.0 } else { (((v - c) / a).exp() + b) / 12.0 }
    });
    let luma = 0.2627 * scene[0] + 0.6780 * scene[1] + 0.0593 * scene[2];
    let gain = f64::from(display.peak) * if luma > 0.0 { luma.powf(f64::from(display.gamma) - 1.0) } else { 0.0 };
    scene.map(|e| gain * e)
}
