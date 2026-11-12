//! The launcher's icons, drawn as vector strokes on a 24-unit grid (Material's), so they stay sharp
//! at every scale and take any role's colour, high contrast included.

use std::f32::consts::{PI, TAU};

use std::cell::RefCell;

use egui::{Color32, CornerRadius, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, Vec2, vec2};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Search,
    Down,
    Gamepad,
    StarOn,
    Play,
    Grid,
    Stop,
    Link,
    Unlink,
    Key,
    Power,
    Moon,
    Folder,
    Sliders,
    Info,
    Help,
    Pencil,
    Refresh,
    Trash,
    Star,
    StarOff,
    Eye,
    EyeOff,
    Monitor,
    Gear,
    Plus,
    Dots,
    Back,
    Mic,
    Lock,
    Video,
    Speaker,
    Network,
    Terminal,
    App,
    Close,
    Check,
    Backspace,
}

/// Draws `icon` into the square `rect` in `color`.
pub fn paint(painter: &Painter, icon: Icon, rect: Rect, color: Color32) {
    painter.extend(shapes(icon, rect, color));
}

/// The shapes of `icon` in the square `rect`.
pub fn shapes(icon: Icon, rect: Rect, color: Color32) -> Vec<Shape> {
    let out = RefCell::new(Vec::new());
    let painter = Collect(&out);
    let s = rect.width().min(rect.height()) / 24.0;
    let origin = rect.center() - Vec2::splat(12.0 * s);
    let p = |x: f32, y: f32| origin + vec2(x, y) * s;
    let stroke = Stroke::new((1.8 * s).max(1.2), color);
    let line = |points: Vec<Pos2>| painter.add(Shape::line(points, stroke));
    let closed = |points: Vec<Pos2>| painter.add(Shape::closed_line(points, stroke));
    let arc = |cx: f32, cy: f32, r: f32, from: f32, to: f32| -> Vec<Pos2> {
        let steps = ((to - from).abs() / TAU * 48.0).ceil().max(4.0) as usize;
        (0..=steps).map(|i| from + (to - from) * i as f32 / steps as f32).map(|a| p(cx + r * a.cos(), cy + r * a.sin())).collect()
    };
    let dot = |x: f32, y: f32, r: f32| painter.circle_filled(p(x, y), r * s, color);
    let ring = |x: f32, y: f32, r: f32| painter.circle_stroke(p(x, y), r * s, stroke);
    match icon {
        Icon::Video => {
            painter.rect_stroke(Rect::from_min_max(p(2.8, 6.2), p(15.4, 17.8)), CornerRadius::same((3.0 * s) as u8), stroke, StrokeKind::Middle);
            closed(vec![p(15.4, 11.0), p(21.2, 7.4), p(21.2, 16.6), p(15.4, 13.0)]);
        }
        // Android's ic_terminal: a window with a prompt.
        Icon::Terminal => {
            closed(vec![p(3.0, 4.0), p(21.0, 4.0), p(21.0, 20.0), p(3.0, 20.0)]);
            line(vec![p(6.0, 8.0), p(10.0, 12.0), p(6.0, 16.0)]);
            line(vec![p(13.0, 16.0), p(18.0, 16.0)]);
        }
        Icon::Speaker => {
            closed(vec![p(3.4, 9.4), p(7.4, 9.4), p(12.2, 5.2), p(12.2, 18.8), p(7.4, 14.6), p(3.4, 14.6)]);
            line(arc(12.2, 12.0, 4.4, -0.8, 0.8));
            line(arc(12.2, 12.0, 8.2, -0.8, 0.8));
        }
        // Android's ic_network: bars rising from a baseline, for Ethernet as much as Wi-Fi.
        Icon::Network => {
            line(vec![p(3.0, 18.0), p(6.0, 18.0)]);
            closed(vec![p(8.0, 14.0), p(11.0, 14.0), p(11.0, 18.0), p(8.0, 18.0)]);
            closed(vec![p(13.0, 10.0), p(16.0, 10.0), p(16.0, 18.0), p(13.0, 18.0)]);
            closed(vec![p(18.0, 5.0), p(21.0, 5.0), p(21.0, 18.0), p(18.0, 18.0)]);
        }
        Icon::App => {
            painter.rect_stroke(Rect::from_min_max(p(3.6, 3.6), p(20.4, 20.4)), CornerRadius::same((5.0 * s) as u8), stroke, StrokeKind::Middle);
            line(vec![p(8.0, 12.0), p(16.0, 12.0)]);
        }
        Icon::Search => {
            ring(10.8, 10.8, 6.4);
            line(vec![p(15.6, 15.6), p(20.4, 20.4)]);
        }
        Icon::Down => {
            line(vec![p(5.5, 9.5), p(12.0, 16.0), p(18.5, 9.5)]);
        }
        Icon::Gamepad => {
            painter.rect_stroke(Rect::from_min_max(p(2.8, 7.0), p(21.2, 18.0)), CornerRadius::same((5.0 * s) as u8), stroke, StrokeKind::Middle);
            line(vec![p(6.4, 12.5), p(10.2, 12.5)]);
            line(vec![p(8.3, 10.6), p(8.3, 14.4)]);
            dot(15.4, 14.0, 1.1);
            dot(18.0, 11.2, 1.1);
        }
        Icon::StarOn => {
            // Five kites from the middle: a star is not convex.
            let at = star(&p);
            let middle = p(12.0, 12.8);
            for i in (0..10).step_by(2) {
                let kite = vec![middle, at[(i + 9) % 10], at[i], at[(i + 1) % 10]];
                painter.add(Shape::convex_polygon(kite, color, stroke));
            }
        }
        Icon::Play => {
            painter.add(Shape::convex_polygon(vec![p(7.5, 4.5), p(19.5, 12.0), p(7.5, 19.5)], color, Stroke::NONE));
        }
        Icon::Grid => {
            for (x, y) in [(4.0, 4.0), (13.5, 4.0), (4.0, 13.5), (13.5, 13.5)] {
                painter.rect_stroke(
                    Rect::from_min_size(p(x, y), Vec2::splat(6.5 * s)),
                    CornerRadius::same((1.5 * s) as u8),
                    stroke,
                    StrokeKind::Middle,
                );
            }
        }
        Icon::Stop => {
            painter.rect_filled(Rect::from_min_max(p(6.0, 6.0), p(18.0, 18.0)), CornerRadius::same((2.0 * s) as u8), color);
        }
        Icon::Link => {
            // Two links of a chain, slanted.
            for (cx, cy) in [(9.0, 15.0), (15.0, 9.0)] {
                closed(stadium(&p, cx, cy, 5.2, 2.6, -PI / 4.0));
            }
        }
        Icon::Unlink => {
            for (cx, cy) in [(7.5, 16.5), (16.5, 7.5)] {
                closed(stadium(&p, cx, cy, 3.4, 2.6, -PI / 4.0));
            }
            line(vec![p(10.5, 10.5), p(13.5, 13.5)]);
        }
        Icon::Key => {
            ring(7.5, 12.0, 4.0);
            line(vec![p(11.5, 12.0), p(21.0, 12.0)]);
            line(vec![p(17.5, 12.0), p(17.5, 15.5)]);
            line(vec![p(20.5, 12.0), p(20.5, 14.5)]);
        }
        Icon::Power => {
            line(arc(12.0, 13.0, 7.5, -PI / 2.0 + 0.75, 1.5 * PI - 0.75));
            line(vec![p(12.0, 3.5), p(12.0, 11.5)]);
        }
        Icon::Moon => closed(crescent(&p)),
        Icon::Folder => closed(vec![p(3.0, 6.0), p(9.5, 6.0), p(11.5, 8.5), p(21.0, 8.5), p(21.0, 19.0), p(3.0, 19.0)]),
        // Android's ic_tune: two tracks, each broken by its knob.
        Icon::Sliders => {
            line(vec![p(4.0, 7.0), p(13.0, 7.0)]);
            line(vec![p(17.0, 7.0), p(20.0, 7.0)]);
            line(vec![p(15.0, 5.0), p(15.0, 9.0)]);
            line(vec![p(4.0, 17.0), p(7.0, 17.0)]);
            line(vec![p(11.0, 17.0), p(20.0, 17.0)]);
            line(vec![p(9.0, 15.0), p(9.0, 19.0)]);
        }
        Icon::Info => {
            ring(12.0, 12.0, 9.0);
            line(vec![p(12.0, 11.0), p(12.0, 16.5)]);
            dot(12.0, 7.8, 1.25);
        }
        Icon::Help => {
            ring(12.0, 12.0, 9.0);
            let mut hook = arc(12.0, 10.0, 2.8, PI, TAU + 0.9);
            hook.push(p(12.0, 14.0));
            line(hook);
            dot(12.0, 17.0, 1.25);
        }
        Icon::Pencil => {
            closed(vec![p(4.5, 19.5), p(5.5, 15.5), p(15.5, 5.5), p(18.5, 8.5), p(8.5, 18.5)]);
            line(vec![p(13.0, 8.0), p(16.0, 11.0)]);
        }
        Icon::Refresh => {
            let (from, to) = (-PI / 2.0 + 0.6, -PI / 2.0 + 0.6 + 1.62 * PI);
            line(arc(12.0, 12.0, 7.0, from, to));
            // The arrowhead at the arc's end, along its direction.
            let end = vec2(12.0 + 7.0 * to.cos(), 12.0 + 7.0 * to.sin());
            let along = vec2(-to.sin(), to.cos());
            let out = vec2(to.cos(), to.sin());
            let tip = end + along * 2.6;
            let a = end + out * 2.6 - along * 0.6;
            let b = end - out * 2.6 - along * 0.6;
            painter.add(Shape::convex_polygon(vec![p(tip.x, tip.y), p(a.x, a.y), p(b.x, b.y)], color, Stroke::NONE));
        }
        Icon::Trash => {
            line(vec![p(4.0, 6.5), p(20.0, 6.5)]);
            line(vec![p(9.5, 6.5), p(9.5, 4.0), p(14.5, 4.0), p(14.5, 6.5)]);
            closed(vec![p(6.0, 6.5), p(7.0, 20.0), p(17.0, 20.0), p(18.0, 6.5)]);
            line(vec![p(10.0, 10.0), p(10.0, 16.5)]);
            line(vec![p(14.0, 10.0), p(14.0, 16.5)]);
        }
        Icon::Star => closed(star(&p)),
        Icon::StarOff => {
            closed(star(&p));
            line(vec![p(3.5, 3.5), p(20.5, 20.5)]);
        }
        Icon::Eye | Icon::EyeOff => {
            let lid = |flip: f32| (0..=12).map(|n| n as f32 / 12.0).map(|t| p(2.5 + 19.0 * t, 12.0 + flip * 7.0 * (PI * t).sin())).collect::<Vec<_>>();
            line(lid(-1.0));
            line(lid(1.0));
            line(arc(12.0, 12.0, 2.8, 0.0, TAU));
            if icon == Icon::EyeOff {
                line(vec![p(3.5, 3.5), p(20.5, 20.5)]);
            }
        }
        Icon::Monitor => {
            painter.rect_stroke(
                Rect::from_min_max(p(3.0, 4.5), p(21.0, 16.0)),
                CornerRadius::same((2.0 * s) as u8),
                stroke,
                StrokeKind::Middle,
            );
            line(vec![p(12.0, 16.0), p(12.0, 19.5)]);
            line(vec![p(7.5, 19.5), p(16.5, 19.5)]);
        }
        Icon::Gear => {
            closed(gear(&p));
            ring(12.0, 12.0, 3.0);
        }
        Icon::Close => {
            line(vec![p(6.0, 6.0), p(18.0, 18.0)]);
            line(vec![p(18.0, 6.0), p(6.0, 18.0)]);
        }
        Icon::Check => line(vec![p(5.0, 12.5), p(10.0, 17.5), p(19.0, 7.0)]),
        Icon::Backspace => {
            // A key shape pointing left, a cross in it.
            line(vec![p(9.0, 5.5), p(21.0, 5.5), p(21.0, 18.5), p(9.0, 18.5), p(3.0, 12.0), p(9.0, 5.5)]);
            line(vec![p(12.0, 9.5), p(17.0, 14.5)]);
            line(vec![p(17.0, 9.5), p(12.0, 14.5)]);
        }
        Icon::Plus => {
            line(vec![p(12.0, 5.0), p(12.0, 19.0)]);
            line(vec![p(5.0, 12.0), p(19.0, 12.0)]);
        }
        Icon::Dots => {
            for y in [5.5, 12.0, 18.5] {
                dot(12.0, y, 2.0);
            }
        }
        Icon::Back => line(vec![p(15.0, 5.0), p(8.0, 12.0), p(15.0, 19.0)]),
        Icon::Lock => {
            // A body and its shackle.
            painter.rect_stroke(
                Rect::from_min_max(p(5.0, 11.0), p(19.0, 21.0)),
                CornerRadius::same((2.5 * s) as u8),
                stroke,
                StrokeKind::Middle,
            );
            let mut shackle = vec![p(8.0, 11.0)];
            shackle.extend(arc(12.0, 7.0, 4.0, PI, 2.0 * PI));
            shackle.push(p(16.0, 11.0));
            line(shackle);
        }
        Icon::Mic => {
            // A capsule on a stand.
            closed(stadium(&p, 12.0, 9.0, 3.2, 3.2, PI / 2.0));
            line(arc(12.0, 10.5, 6.5, 0.0, PI));
            line(vec![p(12.0, 17.0), p(12.0, 20.5)]);
        }
    };
    out.into_inner()
}

/// Gathers shapes with a painter's calls, so [`shapes`] reads like drawing code.
struct Collect<'a>(&'a RefCell<Vec<Shape>>);

impl Collect<'_> {
    fn add(&self, shape: Shape) {
        self.0.borrow_mut().push(shape);
    }
    fn circle_filled(&self, center: Pos2, radius: f32, color: Color32) {
        self.add(Shape::circle_filled(center, radius, color));
    }
    fn circle_stroke(&self, center: Pos2, radius: f32, stroke: Stroke) {
        self.add(Shape::circle_stroke(center, radius, stroke));
    }
    fn rect_filled(&self, rect: Rect, radius: CornerRadius, color: Color32) {
        self.add(Shape::rect_filled(rect, radius, color));
    }
    fn rect_stroke(&self, rect: Rect, radius: CornerRadius, stroke: Stroke, kind: StrokeKind) {
        self.add(Shape::rect_stroke(rect, radius, stroke, kind));
    }
}

/// A rounded bar (a chain link) around (cx, cy): `half` its half length, `r` its end radius.
fn stadium(p: &impl Fn(f32, f32) -> Pos2, cx: f32, cy: f32, half: f32, r: f32, angle: f32) -> Vec<Pos2> {
    let (along, across) = (vec2(angle.cos(), angle.sin()), vec2(-angle.sin(), angle.cos()));
    let mut points = Vec::new();
    for (end, from) in [(half, -PI / 2.0), (-half, PI / 2.0)] {
        for i in 0..=12 {
            let a = from + PI * i as f32 / 12.0;
            let at = vec2(cx, cy) + along * (end + r * a.cos()) + across * (r * a.sin());
            points.push(p(at.x, at.y));
        }
    }
    points
}

/// A crescent: the arc of a disc that another, offset disc leaves uncovered, and that disc's arc
/// inside it.
fn crescent(p: &impl Fn(f32, f32) -> Pos2) -> Vec<Pos2> {
    let (c1, r1) = (vec2(11.0, 13.0), 8.0_f32);
    let (c2, r2) = (vec2(16.0, 8.0), 6.5_f32);
    let d = (c2 - c1).length();
    let a = (r1 * r1 - r2 * r2 + d * d) / (2.0 * d);
    let toward = (c2 - c1).angle();
    let alpha = (a / r1).clamp(-1.0, 1.0).acos();
    let mut points = Vec::new();
    // The outer edge: the big disc's arc away from the small one.
    let (from, to) = (toward + alpha, toward + TAU - alpha);
    for i in 0..=40 {
        let t = from + (to - from) * i as f32 / 40.0;
        points.push(c1 + r1 * vec2(t.cos(), t.sin()));
    }
    // The inner edge: the small disc's arc from where the outer one ended back to where it began,
    // the way that runs inside the big disc.
    let (start, end) = (*points.last().unwrap(), points[0]);
    let s = (start - c2).angle();
    let mut e = (end - c2).angle();
    let mid = |s: f32, e: f32| {
        let m = (s + e) / 2.0;
        c2 + r2 * vec2(m.cos(), m.sin())
    };
    if e < s {
        e += TAU;
    }
    let (s, e) = if (mid(s, e) - c1).length() < r1 { (s, e) } else { (s + TAU, e) };
    for i in 1..40 {
        let t = s + (e - s) * i as f32 / 40.0;
        points.push(c2 + r2 * vec2(t.cos(), t.sin()));
    }
    points.into_iter().map(|v| p(v.x, v.y)).collect()
}

fn star(p: &impl Fn(f32, f32) -> Pos2) -> Vec<Pos2> {
    (0..10)
        .map(|i| {
            let r = if i % 2 == 0 { 9.0 } else { 4.0 };
            let a = -PI / 2.0 + i as f32 * PI / 5.0;
            p(12.0 + r * a.cos(), 12.8 + r * a.sin())
        })
        .collect()
}

fn gear(p: &impl Fn(f32, f32) -> Pos2) -> Vec<Pos2> {
    let teeth = 8;
    let mut points = Vec::new();
    for i in 0..teeth {
        let a = i as f32 * TAU / teeth as f32;
        let w = TAU / teeth as f32;
        for (offset, r) in [(-0.30, 6.6), (-0.17, 9.0), (0.17, 9.0), (0.30, 6.6)] {
            let t = a + offset * w * 1.4;
            points.push(p(12.0 + r * t.cos(), 12.0 + r * t.sin()));
        }
    }
    points
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Icon; 29] = [
        Icon::Search,
        Icon::Down,
        Icon::Gamepad,
        Icon::StarOn,
        Icon::Play,
        Icon::Grid,
        Icon::Stop,
        Icon::Link,
        Icon::Unlink,
        Icon::Key,
        Icon::Power,
        Icon::Moon,
        Icon::Folder,
        Icon::Sliders,
        Icon::Info,
        Icon::Help,
        Icon::Pencil,
        Icon::Refresh,
        Icon::Trash,
        Icon::Star,
        Icon::StarOff,
        Icon::Monitor,
        Icon::Gear,
        Icon::Plus,
        Icon::Dots,
        Icon::Back,
        Icon::Lock,
        Icon::Network,
        Icon::Terminal,
    ];

    #[test]
    fn every_icon_draws_inside_its_square() {
        for size in [16.0, 24.0, 40.0] {
            let rect = Rect::from_min_size(Pos2::new(100.0, 100.0), Vec2::splat(size));
            for icon in ALL {
                let shapes = shapes(icon, rect, Color32::WHITE);
                assert!(!shapes.is_empty(), "{icon:?} draws nothing");
                let bounds = shapes.iter().fold(Rect::NOTHING, |b, s| b.union(s.visual_bounding_rect()));
                assert!(rect.expand(1.0).contains_rect(bounds), "{icon:?} at {size} spills out of its square: {bounds:?}");
            }
        }
    }
}
