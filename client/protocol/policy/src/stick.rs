//! The on-screen analog stick: how a finger maps to a deflection and where the knob is drawn.
//! This is the spec. Android's on-screen controls run its Java port
//! (`com.limelight.binding.input.StickMath`, no JNI on the input or draw path), held bit-equal by
//! replaying the traces in `tests/traces/stick` against it.
//!
//! Sizes are in pixels of a square control of side `size`. Deflections run from -1 to 1 with up
//! positive; screen coordinates grow down.
//!
//! The knob shows the value sent: at full deflection its centre sits on the base's rim, so half of
//! it reaches past the base, like a real stick seen from above. The finger's travel for full
//! deflection stays [`travel`], shorter than the base's radius, so the knob leads the thumb and
//! stays visible beside it.

use serde_json::Value;

use crate::trace::{Args, Subject};

/// The knob's radius as a share of the control's side.
const KNOB: f32 = 0.2;
/// How far the finger moves for full deflection, as a share of the control's side.
const TRAVEL: f32 = 0.3;

/// How far the finger moves from the stick's centre for full deflection.
pub fn travel(size: f32) -> f32 {
    size * TRAVEL
}

/// The knob's radius.
pub fn knob_radius(size: f32) -> f32 {
    size * KNOB
}

/// The base's radius: the control's half side, less the rim's stroke so the rim stays inside.
pub fn base_radius(size: f32, stroke: f32) -> f32 {
    size / 2.0 - stroke
}

/// How far past each edge of the control's box the knob and its shadow can reach, so the view that
/// draws the knob is this much larger on every side. `shadow` is how far the shadow reaches past
/// the knob's edge.
pub fn overhang(size: f32, stroke: f32, shadow: f32) -> f32 {
    (base_radius(size, stroke) + knob_radius(size) + shadow - size / 2.0).max(0.0)
}

/// The deflection for a finger `dx` right and `dy` up of the stick's centre, with full deflection
/// at `radius` (at least 1 pixel). Within `deadzone` (a share of `radius`) it is 0; past it the
/// length rescales so every value from just above 0 to 1 stays reachable, and a finger past
/// `radius` holds full deflection in its direction.
pub fn deflection(dx: f32, dy: f32, radius: f32, deadzone: f32) -> (f32, f32) {
    let radius = radius.max(1.0);
    let (dx, dy) = (dx / radius, dy / radius);
    let length = (dx * dx + dy * dy).sqrt();
    if length <= deadzone || length == 0.0 {
        return (0.0, 0.0);
    }
    let scaled = (length.min(1.0) - deadzone) / (1.0 - deadzone);
    (dx / length * scaled, dy / length * scaled)
}

/// The knob's drawn centre for deflection (`x`, `y`). It moves from `origin` (the base's centre, or
/// where a floating stick's thumb landed) by the deflection times the base's radius `base`, and
/// never past the rim around `center`: at full deflection of a centred stick the knob's centre sits
/// exactly on the rim.
pub fn knob_center(
    center: (f32, f32),
    origin: (f32, f32),
    x: f32,
    y: f32,
    base: f32,
) -> (f32, f32) {
    let (kx, ky) = (origin.0 + x * base, origin.1 - y * base);
    let (ox, oy) = (kx - center.0, ky - center.1);
    let off = (ox * ox + oy * oy).sqrt();
    if off > base && off > 0.0 {
        (center.0 + ox / off * base, center.1 + oy / off * base)
    } else {
        (kx, ky)
    }
}

/// Static functions only, so the traces have `"new": null`.
pub(crate) struct Traced;

pub(crate) fn traced(args: &Value) -> Result<Box<dyn Subject>, String> {
    if !args.is_null() {
        return Err("static functions: new must be null".into());
    }
    Ok(Box::new(Traced))
}

fn floats(args: &Value, count: usize) -> Result<Vec<f32>, String> {
    Args::new(args, count)?;
    (0..count)
        .map(|i| {
            args[i]
                .as_f64()
                .map(|v| v as f32)
                .ok_or_else(|| format!("argument {i} is not a number: {}", args[i]))
        })
        .collect()
}

impl Subject for Traced {
    fn call(&mut self, method: &str, args: &Value) -> Result<Value, String> {
        let value = match method {
            "travel" => travel(floats(args, 1)?[0]),
            "knob_radius" => knob_radius(floats(args, 1)?[0]),
            "base_radius" => {
                let a = floats(args, 2)?;
                base_radius(a[0], a[1])
            }
            "overhang" => {
                let a = floats(args, 3)?;
                overhang(a[0], a[1], a[2])
            }
            "deflection_x" | "deflection_y" => {
                let a = floats(args, 4)?;
                let (x, y) = deflection(a[0], a[1], a[2], a[3]);
                if method == "deflection_x" {
                    x
                } else {
                    y
                }
            }
            "knob_x" | "knob_y" => {
                let a = floats(args, 7)?;
                let (x, y) = knob_center((a[0], a[1]), (a[2], a[3]), a[4], a[5], a[6]);
                if method == "knob_x" {
                    x
                } else {
                    y
                }
            }
            _ => return Err(format!("unknown call {method}")),
        };
        Ok(Value::from(f64::from(value)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: f32 = 300.0;
    const STROKE: f32 = 4.5;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn full_deflection_puts_the_knob_centre_on_the_rim() {
        let base = base_radius(SIZE, STROKE);
        let c = (SIZE / 2.0, SIZE / 2.0);
        for degrees in (0..360).step_by(15) {
            let angle = (degrees as f32).to_radians();
            let (x, y) = deflection(
                angle.cos() * travel(SIZE),
                angle.sin() * travel(SIZE),
                travel(SIZE),
                0.08,
            );
            let (kx, ky) = knob_center(c, c, x, y, base);
            let off = ((kx - c.0).powi(2) + (ky - c.1).powi(2)).sqrt();
            assert!(
                close(off, base),
                "{degrees}°: knob {off} from the centre, rim {base}"
            );
            // Up is positive in the deflection and negative on screen.
            assert!(close(kx - c.0, angle.cos() * base) && close(c.1 - ky, angle.sin() * base));
        }
    }

    #[test]
    fn half_the_knob_reaches_past_the_base_and_the_box_holds_it() {
        let base = base_radius(SIZE, STROKE);
        let knob = knob_radius(SIZE);
        let shadow = 6.0;
        let reach = base + knob + shadow;
        let o = overhang(SIZE, STROKE, shadow);
        // The knob's outer half is outside the rim.
        assert!(base + knob > base && close(base + knob - base, knob));
        // The drawing box (the control plus the overhang on each side) holds the knob and its shadow.
        assert!(
            close(SIZE / 2.0 + o, reach),
            "overhang {o} for reach {reach}"
        );
        assert!(o > 0.0);
        // At rest the knob stays inside the control.
        assert!(knob < SIZE / 2.0);
    }

    #[test]
    fn a_finger_past_the_rim_holds_full_deflection() {
        let r = travel(SIZE);
        for far in [r, base_radius(SIZE, STROKE), SIZE, 10.0 * SIZE] {
            let (x, y) = deflection(-far * 0.6, far * 0.8, r, 0.08);
            assert!(close((x * x + y * y).sqrt(), 1.0), "{far}: {x}, {y}");
            assert!(close(x, -0.6) && close(y, 0.8));
        }
        let c = (SIZE / 2.0, SIZE / 2.0);
        let (kx, ky) = knob_center(c, c, 1.0, 0.0, base_radius(SIZE, STROKE));
        assert!(close(kx, c.0 + base_radius(SIZE, STROKE)) && close(ky, c.1));
    }

    #[test]
    fn deadzone_and_curve_are_unchanged() {
        let r = 100.0;
        // Inside the deadzone nothing moves.
        assert_eq!(deflection(7.0, 0.0, r, 0.08), (0.0, 0.0));
        assert_eq!(deflection(0.0, 0.0, r, 0.08), (0.0, 0.0));
        // Past it the length rescales linearly to reach 1 at the radius.
        let (x, _) = deflection(54.0, 0.0, r, 0.08);
        assert!(close(x, (0.54 - 0.08) / 0.92));
        let (x, y) = deflection(0.0, -100.0, r, 0.0);
        assert_eq!((x, y), (0.0, -1.0));
        // A radius below a pixel is taken as one.
        assert_eq!(deflection(0.5, 0.0, 0.0, 0.0), (0.5, 0.0));
    }

    #[test]
    fn a_floating_knob_starts_at_the_thumb_and_stays_within_the_rim() {
        let base = base_radius(SIZE, STROKE);
        let c = (SIZE / 2.0, SIZE / 2.0);
        let landed = (c.0 + 60.0, c.1 - 20.0);
        assert_eq!(knob_center(c, landed, 0.0, 0.0, base), landed);
        // Pushed outward from an off-centre landing, it stops on the rim.
        let (kx, ky) = knob_center(c, landed, 1.0, 0.0, base);
        let off = ((kx - c.0).powi(2) + (ky - c.1).powi(2)).sqrt();
        assert!(close(off, base));
        // Pushed back across, it may cross the centre freely.
        let (kx, _) = knob_center(c, landed, -1.0, 0.0, base);
        assert!(close(kx, landed.0 - base));
    }
}
