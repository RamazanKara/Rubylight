//! Damped springs for one value at a time. Motion steps on real frame time, retargets from where
//! the value is and how fast it moves, and asks for frames only until it settles, so an idle window
//! stays idle. With reduced motion a value jumps to its target.

use egui::{Context, Id};

use super::design_tokens as t;

/// A spring's tuning: mass 1, stiffness and damping ratio from the design tokens.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tune {
    pub stiffness: f32,
    pub ratio: f32,
}

pub const PRESS: Tune = Tune {
    stiffness: t::MOTION_PRESS_STIFFNESS,
    ratio: t::MOTION_PRESS_RATIO,
};
pub const FOCUS: Tune = Tune {
    stiffness: t::MOTION_FOCUS_STIFFNESS,
    ratio: t::MOTION_FOCUS_RATIO,
};
pub const TAB_PILL: Tune = Tune {
    stiffness: t::MOTION_TAB_PILL_STIFFNESS,
    ratio: t::MOTION_TAB_PILL_RATIO,
};

/// One animated value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spring {
    pub pos: f32,
    pub vel: f32,
    pub target: f32,
}

/// The longest step taken at once: stiff springs stay stable at any frame time.
const SUBSTEP: f32 = 1.0 / 240.0;
/// A stall counts as this long at most, so a value resumes where it was rather than teleporting.
const LONGEST_FRAME: f32 = 1.0 / 15.0;
/// Closer than this and slower than [`REST_SPEED`], in the value's own units, it has arrived.
const REST_DISTANCE: f32 = 0.0015;
const REST_SPEED: f32 = 0.015;

impl Spring {
    pub fn at(value: f32) -> Self {
        Self {
            pos: value,
            vel: 0.0,
            target: value,
        }
    }

    pub fn settled(&self) -> bool {
        (self.pos - self.target).abs() < REST_DISTANCE * self.scale()
            && self.vel.abs() < REST_SPEED * self.scale()
    }

    /// Pixels move in larger units than a 0..1 scale: rest is relative to the value's size.
    fn scale(&self) -> f32 {
        self.target.abs().max(1.0)
    }

    /// Advances `dt` seconds of real time. The target may have changed since the last step: the
    /// value carries on from its position and velocity.
    pub fn step(&mut self, dt: f32, tune: Tune) {
        let omega = tune.stiffness.max(0.0).sqrt();
        let damping = 2.0 * tune.ratio * omega;
        let mut left = dt.clamp(0.0, LONGEST_FRAME);
        while left > 0.0 {
            let h = left.min(SUBSTEP);
            self.vel += (tune.stiffness * (self.target - self.pos) - damping * self.vel) * h;
            self.pos += self.vel * h;
            left -= h;
        }
        if self.settled() {
            *self = Self::at(self.target);
        }
    }
}

#[derive(Clone, Copy)]
struct Stored {
    spring: Spring,
    time: f64,
}

/// Whether this frame's motion is on: the look's reduced motion sets egui's animation time to 0.
pub fn motion(ctx: &Context) -> bool {
    ctx.global_style().animation_time > 0.0
}

/// The value of the spring `id` this frame, moving toward `target`. The first call starts at the
/// target. Asks for the next frame only while it moves.
pub fn animate(ctx: &Context, id: Id, target: f32, tune: Tune) -> f32 {
    let id = id.with("spring");
    let now = ctx.input(|i| i.time);
    let mut stored = ctx.data(|d| d.get_temp::<Stored>(id)).unwrap_or(Stored {
        spring: Spring::at(target),
        time: now,
    });
    if motion(ctx) {
        stored.spring.target = target;
        stored.spring.step((now - stored.time) as f32, tune);
    } else {
        stored.spring = Spring::at(target);
    }
    stored.time = now;
    ctx.data_mut(|d| d.insert_temp(id, stored));
    if !stored.spring.settled() {
        ctx.request_repaint();
    }
    stored.spring.pos
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_spring_arrives_and_rests_exactly_on_its_target() {
        let mut s = Spring::at(0.0);
        s.target = 1.0;
        for _ in 0..240 {
            s.step(1.0 / 60.0, FOCUS);
        }
        assert_eq!(s, Spring::at(1.0));
        assert!(s.settled());
    }

    #[test]
    fn an_underdamped_spring_overshoots_and_a_critical_one_does_not() {
        let peak = |tune| {
            let mut s = Spring::at(0.0);
            s.target = 1.0;
            let mut peak = 0.0f32;
            for _ in 0..240 {
                s.step(1.0 / 120.0, tune);
                peak = peak.max(s.pos);
            }
            peak
        };
        assert!(peak(FOCUS) > 1.02, "focus {}", peak(FOCUS));
        assert!(peak(PRESS) <= 1.0 + REST_DISTANCE, "press {}", peak(PRESS));
    }

    #[test]
    fn retargeting_keeps_position_and_velocity() {
        let mut s = Spring::at(0.0);
        s.target = 100.0;
        for _ in 0..6 {
            s.step(1.0 / 120.0, TAB_PILL);
        }
        let (pos, vel) = (s.pos, s.vel);
        assert!(vel > 0.0);
        s.target = -100.0;
        // The first step back still moves forward: it has momentum, it does not teleport.
        s.step(1.0 / 240.0, TAB_PILL);
        assert!(s.pos > pos, "{} after {pos}", s.pos);
        assert!(s.vel < vel);
    }

    #[test]
    fn the_same_time_takes_the_value_the_same_way_at_any_frame_rate() {
        let run = |frames: u32, dt: f32| {
            let mut s = Spring::at(0.0);
            s.target = 1.0;
            for _ in 0..frames {
                s.step(dt, FOCUS);
            }
            s.pos
        };
        assert!((run(24, 1.0 / 240.0) - run(6, 1.0 / 60.0)).abs() < 0.02);
    }

    #[test]
    fn a_settled_spring_asks_for_no_frames_and_reduced_motion_jumps() {
        let ctx = Context::default();
        let run = |ctx: &Context, target: f32, time: f64| {
            let mut value = 0.0;
            let output = ctx.run_ui(
                egui::RawInput {
                    time: Some(time),
                    ..Default::default()
                },
                |ui| {
                    value = animate(ui.ctx(), Id::unique("x"), target, FOCUS);
                },
            );
            let mut delta = output.textures_delta;
            delta.clear();
            value
        };
        assert_eq!(run(&ctx, 1.0, 0.0), 1.0, "starts at its target");
        run(&ctx, 1.0, 0.016);
        run(&ctx, 1.0, 0.032);
        assert!(!ctx.has_requested_repaint(), "idle stays idle");
        let moved = run(&ctx, 2.0, 0.048);
        assert!(moved < 2.0);
        assert!(
            ctx.has_requested_repaint(),
            "moving asks for the next frame"
        );
        let mut style = (*ctx.global_style()).clone();
        style.animation_time = 0.0;
        ctx.set_global_style(style);
        assert_eq!(
            run(&ctx, 5.0, 0.064),
            5.0,
            "reduced motion goes straight there"
        );
    }
}
