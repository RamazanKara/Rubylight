//! Frame Warp's edge fill and HUD probe on the desktop's presenter (docs/frame-warp.md): the
//! shaders' third Frame Warp row, the SDL renderer path's edge fill, the statistics line's
//! clauses for the gyro and the host's confidence, and why a stream runs without Frame Warp
//! (`Refusal`: a PC whose streaming app predates this Nightjar's Frame Warp is told to update).
//!
//! `nightjar_core::frame_warp::Warp::v2` is the row a picture is drawn with: the edge fill's
//! strength (0 to 1, how dark the strip a shift uncovers gets at its far edge), the HUD probe's
//! reach in mask cells (0 keeps a point whose shifted source lands on the HUD in place), then the
//! confidence and the meta's flags, which only the CPU reads (the confidence is already in the
//! shift). Every shader path takes it as `warpV2`, its third row after `warp` and `warpLens`
//! (`WarpDraw::v2_row`), and draws `frame_warp.hlsl`'s `frameWarpEdgeV2` and `frameWarpProbe`,
//! which follow `nightjar_core::frame_warp_v2::edge_v2` and `hud_probe`.
//!
//! The SDL renderer path draws the edge fill as triangles (`Fill`, which `Mesh` cuts along),
//! with the smoothstep of its darkening cut into `RAMP_CUTS` straight pieces. It does not probe
//! past the HUD: there a point whose shifted source lands on the HUD keeps its own pixel, because
//! where the probe ends depends on the cells beside each HUD cell along the shift, up to eight
//! copies of the HUD's rectangles cut against one another, which triangles cannot draw.

use nightjar_core::frame_warp::{Status, EDGE_RAMP};
use nightjar_core::frame_warp_v2::{edge_v2, meta_flags, EDGE_V2_REFLECT, MAX_HUD_GUARD};
use nightjar_core::http::{Availability, Reason};

use super::frame_warp::WarpDraw;

/// Straight pieces the SDL renderer path cuts the smoothstep of the edge fill's darkening into
/// on either side: its brightness is then within `0.75 / RAMP_CUTS²` of the strength of the
/// shaders' (1.2% at full strength).
pub const RAMP_CUTS: usize = 8;
/// The most places one axis bends under the edge fill (`Fill::bends`).
pub const MAX_BENDS: usize = 2 * RAMP_CUTS + 4;

impl WarpDraw {
    /// The shaders' `warpV2` row: the edge fill's strength (0 to 1) and the HUD probe's reach
    /// (whole mask cells, 0 to `MAX_HUD_GUARD`), then the confidence and the flags as they came.
    /// All zero for an identity draw, which the shaders skip.
    pub fn v2_row(&self) -> [f32; 4] {
        if self.is_identity() {
            return [0.0; 4];
        }
        let [strength, reach, confidence, flags] = self.v2.map(|v| if v.is_finite() { v } else { 0.0 });
        [strength.clamp(0.0, 1.0), reach.clamp(0.0, f32::from(MAX_HUD_GUARD)).floor(), confidence, flags]
    }
}

/// The edge fill as the SDL renderer path draws it: `edge_v2` with this strength, as the shaders.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Fill {
    pub strength: f32,
}

impl Fill {
    /// The fill a `warpV2` row (`WarpDraw::v2_row`) draws.
    pub fn of(row: [f32; 4]) -> Self {
        Self { strength: row[0] }
    }

    /// Where the shifted picture point `p` (fractions of the picture) samples it and how bright.
    pub fn edge(self, p: [f32; 2]) -> ([f32; 2], f32) {
        edge_v2(p, self.strength)
    }

    /// One axis of `edge`: where the shifted coordinate `v` samples and how bright that axis alone
    /// would draw it. The darker axis darkens a point, as `edge` does.
    pub fn axis(self, v: f32) -> (f32, f32) {
        let ([x, _], fade) = edge_v2([v, 0.5], self.strength);
        (x, fade)
    }

    /// Where one axis bends under the fill, as the shifted coordinate, the first `count` of them:
    /// the edges and where the far edge is held, where the sample bends, and while the fill
    /// darkens at all, its smoothstep cut into `RAMP_CUTS` pieces on either side.
    pub fn bends(self) -> ([f32; MAX_BENDS], usize) {
        let mut bends = [0.0; MAX_BENDS];
        bends[..4].copy_from_slice(&[-1.0 / EDGE_V2_REFLECT, 0.0, 1.0, 1.0 + 1.0 / EDGE_V2_REFLECT]);
        if self.strength <= 0.0 {
            return (bends, 4);
        }
        for cut in 1..=RAMP_CUTS {
            let ramp = EDGE_RAMP * cut as f32 / RAMP_CUTS as f32;
            (bends[2 + 2 * cut], bends[3 + 2 * cut]) = (-ramp, 1.0 + ramp);
        }
        (bends, MAX_BENDS)
    }

    /// The stretches of one axis, before the fill, that sample inside `a..b` of the picture with
    /// it: `a..b` itself and what the fill reflects into it from past either edge. A stretch that
    /// reaches an edge reaches out all the way past it, since the fill holds that edge beyond.
    pub fn ranges(self, a: f32, b: f32) -> [(f32, f32); 3] {
        let r = EDGE_V2_REFLECT;
        let below = if b >= 1.0 { f32::NEG_INFINITY } else { -b / r };
        let above = if a <= 0.0 { f32::INFINITY } else { 1.0 + (1.0 - a) / r };
        [(a, b), (below, -a / r), (1.0 + (1.0 - b) / r, above)]
    }
}

/// What the statistics line adds after a locked model's terms: the gyro's gain while the host has
/// locked it and the gyro's packets go out, then how sure the host is that the camera moves the
/// picture (the envelope the shift fades by).
pub fn locked_clauses(status: &Status) -> String {
    let v2 = status.v2;
    let mut line = String::new();
    if let Some(meta) = v2.meta.filter(|meta| meta.has(meta_flags::GYRO_LOCKED) && v2.gyro_packets > 0) {
        line.push_str(&format!(", gyro {:.0} px/rad", meta.gyro_gain[0].abs().max(meta.gyro_gain[1].abs())));
    }
    line.push_str(&format!(", {:.0}% sure", v2.confidence.clamp(0.0, 1.0) * 100.0));
    line
}

/// Why a played stream with the setting on runs without Frame Warp (`Facts::frame_warp_refused`),
/// for the notice (`frame_warp_notice`) and the statistics line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Refusal {
    /// Frame Warp runs, or the stream did not ask for it.
    #[default]
    None,
    /// The PC doesn't offer it.
    NotOffered,
    /// The PC's streaming app runs an older Frame Warp than this Nightjar does
    /// (`Reason::UpdateHost`): nothing was asked of it.
    UpdateHost,
}

impl Refusal {
    /// Why a stream that asked for Frame Warp runs without it, from what the PC says of it.
    pub fn of(availability: Availability) -> Self {
        if availability == Availability::Unsupported(Some(Reason::UpdateHost)) { Self::UpdateHost } else { Self::NotOffered }
    }

    /// The statistics overlay's Frame Warp line for a stream without it; None while it runs.
    pub fn line(self) -> Option<String> {
        match self {
            Self::None => None,
            Self::NotOffered => Some(super::frame_warp::status_line(None)),
            Self::UpdateHost => Some("needs the current version of Nightjar and of the PC’s streaming app".to_owned()),
        }
    }
}

/// Whether a shader's constant block, from its `warp` row on, holds Frame Warp's three rows in
/// order and ends with them, as every path's packing fills it (`v2_row` last).
#[cfg(test)]
pub(crate) fn rows_end_block(from_warp: &str) -> bool {
    let mut rest = from_warp;
    for row in ["float4 warp;", "float4 warpLens;", "float4 warpV2;"] {
        let Some(after) = rest.trim_start().strip_prefix(row) else { return false };
        rest = after;
    }
    let rest = rest.trim_start();
    rest.starts_with("};") || rest.starts_with("#endif")
}

#[cfg(test)]
mod tests;
