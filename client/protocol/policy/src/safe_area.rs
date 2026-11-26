//! A phone's safe area for the stream: how far the picture keeps from rounded screen corners and
//! a camera cutout in Safe fit, and where a picture the player moved by hand (Picture position)
//! lands. This is the spec. Android runs its Java port (`com.limelight.ui.SafeAreaPolicy`, no JNI
//! on the layout path), held equal by replaying the traces in `tests/traces/safe_area` against it.
//!
//! Everything is integer pixels so both languages agree exactly. Rotations are Android's
//! `Surface.ROTATION_0..ROTATION_270` (0 to 3): the panel turned counter-clockwise by that many
//! quarter turns from its natural orientation, so a camera at the natural top is on the left in
//! rotation 1 and on the right in rotation 3.
//!
//! A hand offset is stored in the panel's natural orientation, in thousandths of the axis it moves
//! along, so it stays on the same physical side of the phone when the phone is turned over: a
//! picture moved away from a cutout in one landscape stays away from it in the other.

use serde_json::Value;

use crate::trace::{Args, Subject};

/// The furthest a hand offset moves the picture, in thousandths of the screen's side.
pub const MAX_OFFSET_PERMILLE: i32 = 150;

/// How far a rectangle's edges keep from a screen corner rounded with `radius` so its corner
/// stays on the lit panel: `r * (1 - 1/sqrt(2))`, rounded up (0.293 r).
pub fn corner_inset(radius: i32) -> i32 {
    if radius <= 0 {
        return 0;
    }
    ((i64::from(radius) * 293 + 999) / 1000) as i32
}

/// Safe fit's inset on one edge: past the cutout on that edge and clear of its two corners.
pub fn edge_inset(cutout: i32, corner_a: i32, corner_b: i32) -> i32 {
    cutout
        .max(0)
        .max(corner_inset(corner_a))
        .max(corner_inset(corner_b))
}

/// A stored offset limited to what the player can choose.
pub fn clamp_offset(permille: i32) -> i32 {
    permille.clamp(-MAX_OFFSET_PERMILLE, MAX_OFFSET_PERMILLE)
}

/// The pixels to move a picture along one axis: `permille` of the screen's side `window`, kept
/// so the picture (at `start`, `size` long, in the screen's coordinates) never leaves the screen.
/// A picture that covers the screen (Fill, Stretch) does not move.
pub fn offset_px(permille: i32, window: i32, start: i32, size: i32) -> i32 {
    if window <= 0 || size <= 0 || size >= window {
        return 0;
    }
    let raw = i64::from(clamp_offset(permille)) * i64::from(window) / 1000;
    let low = 0.min(-start);
    let high = 0.max(window - start - size);
    raw.clamp(i64::from(low), i64::from(high)) as i32
}

/// A natural-orientation offset seen on the turned screen, as (x, y).
pub fn to_screen(natural_x: i32, natural_y: i32, rotation: i32) -> (i32, i32) {
    match rotation {
        1 => (natural_y, -natural_x),
        2 => (-natural_x, -natural_y),
        3 => (-natural_y, natural_x),
        _ => (natural_x, natural_y),
    }
}

/// The natural-orientation offset of one seen on the turned screen, the inverse of [`to_screen`].
pub fn to_natural(screen_x: i32, screen_y: i32, rotation: i32) -> (i32, i32) {
    match rotation {
        1 => (-screen_y, screen_x),
        2 => (-screen_x, -screen_y),
        3 => (screen_y, -screen_x),
        _ => (screen_x, screen_y),
    }
}

struct Traced;

pub(crate) fn traced(new: &Value) -> Result<Box<dyn Subject>, String> {
    match new {
        Value::Null => Ok(Box::new(Traced)),
        other => Err(format!("safe_area has only static calls, got new {other}")),
    }
}

fn ints<const N: usize>(args: &Value) -> Result<[i32; N], String> {
    let args = Args::new(args, N)?;
    let mut values = [0; N];
    for (i, value) in values.iter_mut().enumerate() {
        *value = args.int(i)?;
    }
    Ok(values)
}

impl Subject for Traced {
    fn call(&mut self, method: &str, args: &Value) -> Result<Value, String> {
        let value = match method {
            "corner_inset" => corner_inset(ints::<1>(args)?[0]),
            "edge_inset" => {
                let [cutout, a, b] = ints(args)?;
                edge_inset(cutout, a, b)
            }
            "clamp_offset" => clamp_offset(ints::<1>(args)?[0]),
            "offset_px" => {
                let [permille, window, start, size] = ints(args)?;
                offset_px(permille, window, start, size)
            }
            "screen_offset_x" | "screen_offset_y" => {
                let [x, y, rotation] = ints(args)?;
                let (sx, sy) = to_screen(x, y, rotation);
                if method == "screen_offset_x" { sx } else { sy }
            }
            "natural_offset_x" | "natural_offset_y" => {
                let [x, y, rotation] = ints(args)?;
                let (nx, ny) = to_natural(x, y, rotation);
                if method == "natural_offset_x" { nx } else { ny }
            }
            _ => return Err(format!("unknown call {method}")),
        };
        Ok(Value::from(value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_corner_rect_touches_the_arc() {
        assert_eq!(corner_inset(0), 0);
        assert_eq!(corner_inset(-5), 0);
        assert_eq!(corner_inset(1), 1);
        // 100 * 0.2929 = 29.29: rounded up so the rectangle's corner is never outside the arc.
        assert_eq!(corner_inset(100), 30);
        assert_eq!(corner_inset(120), 36);
        for r in 1..400 {
            let d = f64::from(corner_inset(r));
            let from_centre = (f64::from(r) - d) * std::f64::consts::SQRT_2;
            assert!(from_centre <= f64::from(r) + 1e-9, "radius {r}");
        }
    }

    #[test]
    fn an_edge_keeps_clear_of_its_cutout_and_corners() {
        assert_eq!(edge_inset(0, 0, 0), 0);
        assert_eq!(edge_inset(-3, 0, 0), 0);
        assert_eq!(edge_inset(80, 100, 100), 80);
        assert_eq!(edge_inset(0, 100, 120), 36);
        assert_eq!(edge_inset(20, 120, 0), 36);
    }

    #[test]
    fn an_offset_moves_inside_the_screen_only() {
        // A 2400 px wide phone, a 16:9 picture 1920 px wide centred at 240.
        assert_eq!(offset_px(50, 2400, 240, 1920), 120);
        assert_eq!(offset_px(-50, 2400, 240, 1920), -120);
        assert_eq!(offset_px(150, 2400, 240, 1920), 240);
        assert_eq!(offset_px(1000, 2400, 240, 1920), 240);
        assert_eq!(offset_px(-150, 2400, 300, 1920), -300);
        // Truncates toward zero in both directions, like Java.
        assert_eq!(offset_px(1, 999, 0, 10), 0);
        assert_eq!(offset_px(-1, 1999, 900, 10), -1);
        // A covering picture never moves.
        assert_eq!(offset_px(100, 2400, 0, 2400), 0);
        assert_eq!(offset_px(100, 2400, -100, 2600), 0);
        assert_eq!(offset_px(100, 0, 0, 0), 0);
    }

    #[test]
    fn a_turned_offset_stays_on_the_same_physical_side() {
        for rotation in 0..4 {
            let (sx, sy) = to_screen(30, -70, rotation);
            assert_eq!(
                to_natural(sx, sy, rotation),
                (30, -70),
                "rotation {rotation}"
            );
        }
        // The camera at the natural top: away from it is natural +y, the right in rotation 1 and
        // the left in rotation 3.
        assert_eq!(to_screen(0, 40, 1), (40, 0));
        assert_eq!(to_screen(0, 40, 3), (-40, 0));
        assert_eq!(to_screen(0, 40, 2), (0, -40));
        assert_eq!(to_screen(5, 7, 9), (5, 7));
    }
}
