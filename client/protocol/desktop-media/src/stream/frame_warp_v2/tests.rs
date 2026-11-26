//! The shaders' fill and probe row, the HLSL that draws it, the triangles' fill and the overlay's
//! clauses (`stream::frame_warp_v2`).

use super::*;
use crate::config::{HostRecord, Settings};
use crate::stream::connect::frame_warp_for;
use crate::stream::frame_warp::{frame_warp_notice, status_line, HLSL};
use nightjar_core::frame_warp::{Model, Warp};
use nightjar_core::frame_warp_v2::{FrameWarpV2Meta, Status as V2Status};
use nightjar_core::http::{AppRole, Feature, ServerInfo};

#[test]
fn the_row_goes_to_the_shaders_clamped_and_only_beside_a_shift() {
    let shifted = WarpDraw { shift: [0.01, 0.0], v2: [0.9, 2.0, 0.75, 33.0], ..WarpDraw::default() };
    assert_eq!(shifted.v2_row(), [0.9, 2.0, 0.75, 33.0], "the fill, the probe, the confidence and the flags");
    assert_eq!(WarpDraw { shift: [0.0, 0.0], ..shifted.clone() }.v2_row(), [0.0; 4], "an identity draw sends zeros, which no shader reads");
    let wild = WarpDraw { v2: [1.7, 20.5, f32::NAN, f32::INFINITY], ..shifted.clone() };
    assert_eq!(wild.v2_row(), [1.0, f32::from(MAX_HUD_GUARD), 0.0, 0.0], "the most strength and reach, nothing unknown");
    assert_eq!(WarpDraw { v2: [-0.5, 2.7, 1.0, 0.0], ..shifted }.v2_row(), [0.0, 2.0, 1.0, 0.0], "no negative strength, whole cells");
    let warp = Warp { pixels: [10.0, 0.0], uv: [0.01, 0.0], active: true, v2: [0.5, 3.0, 1.0, 32.0], ..Warp::default() };
    assert_eq!(WarpDraw::of(&warp, None).v2_row(), [0.5, 3.0, 1.0, 32.0], "the predictor's row is the one drawn");
    assert_eq!(WarpDraw::of(&Warp { active: false, ..warp }, None).v2_row(), [0.0; 4]);
}

#[test]
fn every_shader_fills_and_probes_as_the_cpu_references() {
    // `edge_v2`: back inside by EDGE_V2_REFLECT of the overshoot, darkened along a smoothstep
    // over EDGE_RAMP by up to the strength; no mirror and no fixed darkening left.
    for line in [
        format!("if (v < 0) return -v * {EDGE_V2_REFLECT};"),
        format!("if (v > size) return size - (v - size) * {EDGE_V2_REFLECT};"),
        format!("float d = saturate(max(outside.x, outside.y) / {EDGE_RAMP});"),
        "fade = 1 - saturate(warpV2.x) * d * d * (3 - 2 * d);".to_owned(),
        "return clamp(float2(frameWarpReflectV2(q.x, size.x), frameWarpReflectV2(q.y, size.y)), 0, size);".to_owned(),
        // `hud_probe`: up to MAX_HUD_GUARD cells back along the shift, one cell a step, each
        // point through the edge fill, the first off the HUD with its brightness.
        format!("for (int k = 1; k <= {MAX_HUD_GUARD}; k++) {{"),
        "if (k > warpV2.y) break;".to_owned(),
        "float2 step = direction / reach / warp.zw * size;".to_owned(),
        "float2 probe = frameWarpEdgeV2(unfilled - k * step, size, probeFade);".to_owned(),
        "if (reach <= 0 || any(warp.zw <= 0)) return source;".to_owned(),
        // The source is always filled, and probed while the row reaches a cell.
        "float2 source = frameWarpEdgeV2(unfilled, size, edgeFade);".to_owned(),
        "if (warpV2.y > 0) source = frameWarpProbe(unfilled, source, size, shift, edgeFade);".to_owned(),
        // An all-zero `warp` row draws the picture untouched.
        "if (all(warp == 0)) return q;".to_owned(),
    ] {
        assert!(HLSL.contains(&line), "frame_warp.hlsl lacks `{line}`");
    }
    for gone in ["float2 frameWarpEdge(", "abs(q)", "0.35"] {
        assert!(!HLSL.contains(gone), "frame_warp.hlsl still has `{gone}`");
    }
}

#[test]
fn the_triangles_cut_and_reflect_where_the_fill_bends() {
    let fill = Fill::of([0.9, 2.0, 1.0, 0.0]);
    assert_eq!(fill, Fill { strength: 0.9 });
    let (bends, count) = fill.bends();
    assert_eq!(count, MAX_BENDS);
    let mut sorted = bends[..count].to_vec();
    sorted.sort_by(f32::total_cmp);
    assert_eq!((sorted[0], sorted[count - 1]), (-1.0 / EDGE_V2_REFLECT, 1.0 + 1.0 / EDGE_V2_REFLECT), "where the far edge is held");
    assert!(sorted.windows(2).all(|w| w[0] < w[1]), "each bend once: {sorted:?}");
    assert!(sorted.contains(&0.0) && sorted.contains(&1.0) && sorted.contains(&-EDGE_RAMP) && sorted.contains(&(1.0 + EDGE_RAMP)));
    assert_eq!(Fill { strength: 0.0 }.bends().1, 4, "a fill that does not darken needs no ramp cuts");
    // An axis samples inside a..b exactly on the stretches `ranges` names, and a point is as
    // dark as its darker axis.
    for (a, b) in [(0.0, 0.1), (0.3, 0.45), (0.9, 1.0), (0.0, 1.0)] {
        let ranges = fill.ranges(a, b);
        for i in -500..1500 {
            let v = i as f32 / 997.0;
            let inside = (a..=b).contains(&fill.axis(v).0);
            let named = ranges.iter().any(|&(lo, hi)| v >= lo && v <= hi);
            assert_eq!(inside, named, "{v} for {a}..{b}: {:?} {ranges:?}", fill.axis(v));
        }
    }
    for p in [[-0.05, 0.5], [0.5, 1.03], [-0.02, 1.06], [1.2, -0.01], [0.4, 0.6]] {
        let (uv, fade) = fill.edge(p);
        assert_eq!(uv, [fill.axis(p[0]).0, fill.axis(p[1]).0]);
        assert_eq!(fade, fill.axis(p[0]).1.min(fill.axis(p[1]).1), "{p:?}");
    }
}

fn locked() -> Model {
    Model {
        generation: 1,
        mouse_locked: true,
        picture_width: 1000,
        picture_height: 500,
        mouse_gain: [-0.5, -0.25],
        max_offset: [200.0, 100.0],
        lag_us: 38_000,
        confidence: 0.94,
        ..Model::default()
    }
}

#[test]
fn the_overlay_line_says_how_sure_the_host_is_and_whether_the_gyro_turns_it() {
    let meta = FrameWarpV2Meta { flags: meta_flags::GYRO_LOCKED, gyro_gain: [-812.4, 300.0], ..FrameWarpV2Meta::default() };
    let gyro = V2Status { confidence: 0.957, meta: Some(meta), gyro_packets: 120 };
    let status = Status { model: Some(locked()), pixels: [3.0, 4.0], peak: 9.0, v2: gyro, ..Status::default() };
    assert_eq!(status_line(Some(&status)), "locked 0.50 px/count, gyro 812 px/rad, 96% sure, lag 38 ms, shift 5 px (peak 9)");
    let no_pad = Status { v2: V2Status { gyro_packets: 0, ..gyro }, ..status };
    assert_eq!(status_line(Some(&no_pad)), "locked 0.50 px/count, 96% sure, lag 38 ms, shift 5 px (peak 9)", "no gyro packets went out");
    let unlocked = Status { v2: V2Status { meta: Some(FrameWarpV2Meta { flags: 0, ..meta }), ..gyro }, ..status };
    assert_eq!(status_line(Some(&unlocked)), "locked 0.50 px/count, 96% sure, lag 38 ms, shift 5 px (peak 9)", "the host has not locked the gyro");
    let paused = Status { v2: V2Status { confidence: 0.0, meta: None, gyro_packets: 0 }, ..status };
    assert_eq!(status_line(Some(&paused)), "locked 0.50 px/count, 0% sure, lag 38 ms, shift 5 px (peak 9)", "a menu fades the shift out");
    let learning = Status { model: Some(Model { confidence: 0.42, ..Model::default() }), v2: gyro, ..Status::default() };
    assert_eq!(status_line(Some(&learning)), "learning (42% fit)");
    for line in [status_line(Some(&status)), status_line(Some(&learning)), status_line(None)] {
        assert!(!line.contains("Frame Warp 1") && !line.contains("Experimental"), "{line}");
    }
}

#[test]
fn a_pc_with_an_older_frame_warp_gets_none_and_the_player_is_told_to_update() {
    let xml = include_str!("../../../../../app/src/test/resources/protocol/rubylight-30-serverinfo.xml");
    let pc = |extra: &str| ServerInfo::parse(&xml.replacen("</root>", &format!("{extra}</root>"), 1), 47989).expect("the answer parses").capabilities;
    let settings = Settings { frame_warp: true, ..Settings::default() };
    // A PC before 7.0 lists only `FrameWarpVersion`: nothing is asked of it.
    let older = pc("<FrameWarpVersion>1</FrameWarpVersion>");
    assert!(frame_warp_for(&settings, AppRole::Stream, &older).is_none(), "no Frame Warp from an older PC");
    let refused = Refusal::of(older.availability(Feature::FrameWarp));
    assert_eq!(refused, Refusal::UpdateHost);
    let update = "Frame Warp needs the current version of Nightjar and of the PC’s streaming app, so this stream runs without it.";
    let mut record = HostRecord::default();
    assert_eq!(frame_warp_notice(&mut record, false, refused), (Some(update.to_owned()), true), "said once");
    assert_eq!(frame_warp_notice(&mut record, false, refused), (None, false));
    assert_eq!(refused.line().as_deref(), Some("needs the current version of Nightjar and of the PC’s streaming app"));
    // A PC that offers none is told so, and a current one runs it.
    let none = pc("");
    assert!(frame_warp_for(&settings, AppRole::Stream, &none).is_none());
    assert_eq!(Refusal::of(none.availability(Feature::FrameWarp)), Refusal::NotOffered);
    assert_eq!(Refusal::NotOffered.line().as_deref(), Some("not offered by the PC"));
    assert!(frame_warp_for(&settings, AppRole::Stream, &pc("<FrameWarpProtocol>2</FrameWarpProtocol>")).is_some());
    assert_eq!(Refusal::None.line(), None);
    for line in [update.to_owned(), refused.line().unwrap()] {
        assert!(!line.contains("Rubylight") && !line.contains("Sunshine") && !line.contains("Apollo"), "{line}");
    }
}
