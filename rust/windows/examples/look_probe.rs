//! Test-owned fake mouse-look game with a known input response, for measuring Frame Warp
//! (docs/frame-warp.md). Not packaged. Requires an explicitly selected display and never
//! changes its resolution, refresh rate or HDR.
//!
//! A borderless window covers the display with a procedural outdoor world (sky and clouds,
//! ground, poles and towers) that turns with the mouse and the right stick, under a HUD that
//! never moves (crosshair, health bar, ammo counter, minimap) drawn in exact constant colours.
//! The response is known exactly. The frame drawn at time t shows the camera for the input as
//! of t - lag:
//!
//! ```text
//! camera = gain * mouse_sum(t - lag) + stick_gain * integral of stick_curve(stick) up to t - lag
//! ```
//!
//! t is the frame's render time, taken just before it is drawn and presented. The mouse sums
//! are raw input's relative moves, injected (SendInput, as the host's) or, unless
//! `--injected-only`, not; the window gets them unfocused (RIDEV_INPUTSINK) and ignores
//! absolute packets. The stick is XInput pad `--pad`'s right stick, polled once a frame and
//! held until the next poll, through the same stick_curve the host fits (XInput's axes: y is
//! up, so pushing up moves the camera down, as inverted look does). The content moves against
//! the camera, so a host that fits this game learns mouse_gain = -gain and stick_gain =
//! -stick_gain on both axes (picture x right, y down; scaled by the stream's width over the
//! display's), and a lag of `--lag-ms` plus the time from drawing a frame to its capture.
//! Camera y stops at half the screen height either way and drops the turn past it, as a game's
//! pitch does; x wraps around a world four screen widths wide.
//!
//! With `--injected-only` the camera turns only for SendInput's moves (no device, as the
//! host's are), so a real mouse moved during a run does not disturb it. OUT.mouse.csv still
//! lists every move with its injected flag; its running sums, like frames.csv's, are the ones
//! the camera used, to which a device move then adds nothing.
//!
//! Over everything, the HUD included, the top left carries motion_probe's barcode, drawn as it
//! draws it, so nightjar-cli's reader (top left at scale 1, its defaults) decodes each picture:
//! four rows 24 pixels tall from y = 8, a black reference at x 0-23 and a white one at x 24-47,
//! and bit n of each row's word at x = 64 + 16n (32 bits, 1 white). Row 0 is the frame number,
//! the frames.csv `frame` of the present that showed it (1 for the first, never 0); rows 1 and
//! 2 are the low and high words of the QueryPerformanceCounter ticks read just before that
//! frame is drawn (its render_qpc_us is the same reading in microseconds); row 3 is 0xB17E2212.
//! It changes every frame, so the host never takes it for HUD. `--barcode off` leaves it out.
//!
//! ```text
//! look_probe \\.\DISPLAYn OUT.json [--seconds N] [--fps current|HZ] [--gain PX_PER_COUNT]
//!     [--lag-ms MS] [--stick-gain PX_PER_SECOND] [--stick-deadzone F]
//!     [--stick-exponent 1|1.5|2|2.5|3] [--hud on|off] [--barcode on|off] [--pad 0-3]
//!     [--injected-only]
//! ```
//!
//! Defaults: 60 seconds; presents at the display's refresh (a rate in Hz paces them as
//! motion_probe does); gain 1 pixel per count on x and y (negative inverts); lag 50 ms; stick
//! gain 1200 pixels per second of full deflection; dead zone 0.2; exponent 1.5; HUD on;
//! barcode on; pad 0; every relative move counts. The run ends after `--seconds`, or on Escape
//! from any keyboard (raw input, so also when the window is not focused).
//!
//! Written at the end: OUT.json (configuration, counts, the model a host should learn) and,
//! beside it with the extension replaced, OUT.frames.csv (a row per present: frame,
//! present_qpc_us, input_time_us, mouse_sum_x, mouse_sum_y, stick_integral_x,
//! stick_integral_y, camera_x, camera_y, render_qpc_us), OUT.mouse.csv (every relative move)
//! and OUT.stick.csv (every stick change). Times are QueryPerformanceCounter microseconds,
//! input_time_us is render_qpc_us less the lag, camera_x is unwrapped (the picture shows it
//! modulo the world width), and stick integrals are seconds of full deflection.

#[cfg_attr(not(windows), allow(dead_code))]
mod look {
    use anyhow::{Context, Result, bail};
    use std::ops::RangeInclusive;
    use std::path::{Path, PathBuf};

    pub const USAGE: &str = r"usage: look_probe \\.\DISPLAYn OUT.json [--seconds N] [--fps current|HZ] [--gain PX_PER_COUNT] [--lag-ms MS] [--stick-gain PX_PER_SECOND] [--stick-deadzone F] [--stick-exponent 1|1.5|2|2.5|3] [--hud on|off] [--barcode on|off] [--pad 0-3] [--injected-only]";
    /// As `rubylight_protocol::frame_warp`'s: the exponents a model may carry.
    pub const STICK_EXPONENTS: [f32; 5] = [1.0, 1.5, 2.0, 2.5, 3.0];
    /// As `rubylight_protocol::frame_warp`'s: the largest dead zone a model may carry.
    pub const MAX_STICK_DEADZONE: f32 = 0.5;

    #[derive(Clone, Debug, PartialEq)]
    pub struct Config {
        pub display: String,
        pub out: PathBuf,
        pub seconds: u64,
        /// None: present at the display's refresh.
        pub fps: Option<u32>,
        /// Pixels per mouse count, x and y.
        pub gain: f64,
        pub lag_us: i64,
        /// Pixels per second of full deflection after the curve, x and y.
        pub stick_gain: f64,
        pub stick_deadzone: f32,
        pub stick_exponent: f32,
        pub hud: bool,
        /// motion_probe's barcode at the top left.
        pub barcode: bool,
        /// XInput user index.
        pub pad: u32,
        /// Only SendInput's relative moves turn the camera.
        pub injected_only: bool,
    }

    impl Config {
        pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Self> {
            let mut args = args.into_iter();
            let (Some(display), Some(out)) = (args.next(), args.next()) else {
                bail!("{USAGE}");
            };
            if !display.to_ascii_uppercase().starts_with(r"\\.\DISPLAY") {
                bail!(r"{display} is not a display name like \\.\DISPLAY2; {USAGE}");
            }
            if out.is_empty() || out.starts_with("--") {
                bail!("the second argument is the output JSON path; {USAGE}");
            }
            let mut config = Self {
                display,
                out: PathBuf::from(out),
                seconds: 60,
                fps: None,
                gain: 1.0,
                lag_us: 50_000,
                stick_gain: 1200.0,
                stick_deadzone: 0.2,
                stick_exponent: 1.5,
                hud: true,
                barcode: true,
                pad: 0,
                injected_only: false,
            };
            while let Some(flag) = args.next() {
                // The one switch without a value.
                if flag == "--injected-only" {
                    config.injected_only = true;
                    continue;
                }
                let Some(value) = args.next() else {
                    bail!("{flag} needs a value; {USAGE}");
                };
                match flag.as_str() {
                    "--seconds" => config.seconds = whole(&flag, &value, 1..=3600)?,
                    "--fps" if value == "current" => config.fps = None,
                    "--fps" => config.fps = Some(whole(&flag, &value, 1..=1000)? as u32),
                    "--gain" => config.gain = number(&flag, &value, -10_000.0..=10_000.0)?,
                    "--lag-ms" => {
                        config.lag_us =
                            (number(&flag, &value, 0.0..=1000.0)? * 1000.0).round() as i64
                    }
                    "--stick-gain" => {
                        config.stick_gain = number(&flag, &value, -100_000.0..=100_000.0)?
                    }
                    "--stick-deadzone" => {
                        let range = 0.0..=f64::from(MAX_STICK_DEADZONE);
                        config.stick_deadzone = number(&flag, &value, range)? as f32;
                    }
                    "--stick-exponent" => {
                        let exponent = number(&flag, &value, 1.0..=3.0)? as f32;
                        if !STICK_EXPONENTS.contains(&exponent) {
                            bail!("--stick-exponent {value}: must be 1, 1.5, 2, 2.5 or 3");
                        }
                        config.stick_exponent = exponent;
                    }
                    "--hud" => config.hud = on_off(&flag, &value)?,
                    "--barcode" => config.barcode = on_off(&flag, &value)?,
                    "--pad" => config.pad = whole(&flag, &value, 0..=3)? as u32,
                    _ => bail!("unknown option {flag}; {USAGE}"),
                }
            }
            Ok(config)
        }
    }

    fn on_off(flag: &str, value: &str) -> Result<bool> {
        match value {
            "on" => Ok(true),
            "off" => Ok(false),
            _ => bail!("{flag} {value}: must be on or off"),
        }
    }

    fn number(flag: &str, value: &str, range: RangeInclusive<f64>) -> Result<f64> {
        let parsed: f64 = value
            .parse()
            .with_context(|| format!("{flag} {value}: not a number"))?;
        // NaN and infinities fail the range too.
        if !range.contains(&parsed) {
            bail!(
                "{flag} {value}: must be {} to {}",
                range.start(),
                range.end()
            );
        }
        Ok(parsed)
    }

    fn whole(flag: &str, value: &str, range: RangeInclusive<u64>) -> Result<u64> {
        let parsed: u64 = value
            .parse()
            .with_context(|| format!("{flag} {value}: not a whole number"))?;
        if !range.contains(&parsed) {
            bail!(
                "{flag} {value}: must be {} to {}",
                range.start(),
                range.end()
            );
        }
        Ok(parsed)
    }

    /// `OUT.json`'s sibling `OUT.<kind>.csv`.
    pub fn sibling(out: &Path, kind: &str) -> PathBuf {
        out.with_extension(format!("{kind}.csv"))
    }

    pub fn qpc_us(ticks: i64, frequency: i64) -> i64 {
        (i128::from(ticks) * 1_000_000 / i128::from(frequency.max(1))) as i64
    }

    /// The curve the host fits, from the protocol crate through butterpollo-core.
    pub use butterpollo_core::frame_warp::stick_curve;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct MouseMove {
        pub at_us: i64,
        pub dx: i32,
        pub dy: i32,
        /// The running sums the camera uses, with this move if it counts.
        pub sum: [i64; 2],
        /// From SendInput (no device), as the host's are.
        pub injected: bool,
    }

    /// Every relative move with the running sums, oldest first.
    #[derive(Debug, Default)]
    pub struct MouseHistory {
        pub moves: Vec<MouseMove>,
        /// Only injected moves add to the sums.
        injected_only: bool,
    }

    impl MouseHistory {
        pub fn new(injected_only: bool) -> Self {
            Self {
                moves: Vec::new(),
                injected_only,
            }
        }

        pub fn push(&mut self, at_us: i64, dx: i32, dy: i32, injected: bool) {
            let (at_us, sum) = self.moves.last().map_or((at_us, [0, 0]), |last| {
                // Times never go back, so lookups can bisect.
                (at_us.max(last.at_us), last.sum)
            });
            let (add_x, add_y) = if injected || !self.injected_only {
                (i64::from(dx), i64::from(dy))
            } else {
                (0, 0)
            };
            self.moves.push(MouseMove {
                at_us,
                dx,
                dy,
                sum: [sum[0] + add_x, sum[1] + add_y],
                injected,
            });
        }

        /// The running sums as of `at_us`, moves at exactly `at_us` included.
        pub fn at(&self, at_us: i64) -> [i64; 2] {
            let after = self.moves.partition_point(|m| m.at_us <= at_us);
            after.checked_sub(1).map_or([0, 0], |i| self.moves[i].sum)
        }
    }

    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct StickSample {
        pub at_us: i64,
        /// XInput's packet number; None while the pad is missing (read as centred).
        pub packet: Option<u32>,
        pub stick: (i16, i16),
        pub curve: [f32; 2],
        /// The integral of the curve from the start to `at_us`.
        pub integral: [f64; 2],
    }

    /// The right stick as polled, each value held until the next, with its integral.
    #[derive(Debug)]
    pub struct StickHistory {
        pub samples: Vec<StickSample>,
        deadzone: f32,
        exponent: f32,
    }

    impl StickHistory {
        /// Starts centred at `start_us`, where the integral starts.
        pub fn new(start_us: i64, deadzone: f32, exponent: f32) -> Self {
            let start = StickSample {
                at_us: start_us,
                packet: None,
                stick: (0, 0),
                curve: [0.0; 2],
                integral: [0.0; 2],
            };
            Self {
                samples: vec![start],
                deadzone,
                exponent,
            }
        }

        pub fn push(&mut self, at_us: i64, packet: Option<u32>, stick: (i16, i16)) {
            let last = self.samples[self.samples.len() - 1];
            if last.packet.is_some() == packet.is_some() && last.stick == stick {
                return;
            }
            let at_us = at_us.max(last.at_us);
            let curve = match packet {
                Some(_) => stick_curve(stick.0, stick.1, self.deadzone, self.exponent),
                None => [0.0; 2],
            };
            self.samples.push(StickSample {
                at_us,
                packet,
                stick,
                curve,
                integral: held(&last, at_us),
            });
        }

        /// The integral of the curve from the start to `at_us`, in seconds of full deflection;
        /// zero before the start.
        pub fn integral(&self, at_us: i64) -> [f64; 2] {
            let after = self.samples.partition_point(|s| s.at_us <= at_us);
            after
                .checked_sub(1)
                .map_or([0.0; 2], |i| held(&self.samples[i], at_us))
        }
    }

    /// `sample`'s integral plus its curve held from it to `at_us`.
    fn held(sample: &StickSample, at_us: i64) -> [f64; 2] {
        let seconds = (at_us - sample.at_us).max(0) as f64 / 1e6;
        [
            sample.integral[0] + f64::from(sample.curve[0]) * seconds,
            sample.integral[1] + f64::from(sample.curve[1]) * seconds,
        ]
    }

    /// The camera in pixels: x unwrapped, y clamped.
    #[derive(Debug)]
    pub struct Camera {
        gain: f64,
        stick_gain: f64,
        y_limit: f64,
        raw_y: f64,
        pub x: f64,
        pub y: f64,
    }

    impl Camera {
        pub fn new(gain: f64, stick_gain: f64, y_limit: f64) -> Self {
            Self {
                gain,
                stick_gain,
                y_limit,
                raw_y: 0.0,
                x: 0.0,
                y: 0.0,
            }
        }

        /// Turns to the input as of the frame's input time: the mouse sums and the stick
        /// integral.
        pub fn update(&mut self, mouse: [i64; 2], stick: [f64; 2]) {
            self.x = self.gain * mouse[0] as f64 + self.stick_gain * stick[0];
            let raw_y = self.gain * mouse[1] as f64 + self.stick_gain * stick[1];
            // Pitch stops at the limit and drops the turn past it, so turning back moves at once.
            self.y = (self.y + raw_y - self.raw_y).clamp(-self.y_limit, self.y_limit);
            self.raw_y = raw_y;
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn parse(text: &str) -> Result<Config> {
            Config::parse(text.split_whitespace().map(str::to_owned))
        }

        #[test]
        fn arguments_have_defaults_and_bounds() {
            let config = parse(r"\\.\DISPLAY3 out/run.json").unwrap();
            assert_eq!(config.display, r"\\.\DISPLAY3");
            assert_eq!(config.out, PathBuf::from("out/run.json"));
            assert_eq!(
                (config.seconds, config.fps, config.lag_us),
                (60, None, 50_000)
            );
            assert_eq!((config.gain, config.stick_gain), (1.0, 1200.0));
            assert_eq!((config.stick_deadzone, config.stick_exponent), (0.2, 1.5));
            assert!(config.hud && config.barcode);
            assert_eq!(config.pad, 0);
            assert!(!config.injected_only);
            let config = parse(
                r"\\.\display3 o.json --seconds 30 --fps 90 --gain -2.5 --lag-ms 33.3 --stick-gain 800 --stick-deadzone 0 --stick-exponent 2 --hud off --injected-only --barcode off --pad 1",
            )
            .unwrap();
            assert_eq!(
                (config.seconds, config.fps, config.lag_us),
                (30, Some(90), 33_300)
            );
            assert_eq!((config.gain, config.stick_gain), (-2.5, 800.0));
            assert_eq!((config.stick_deadzone, config.stick_exponent), (0.0, 2.0));
            assert!(!config.hud && !config.barcode);
            assert_eq!(config.pad, 1);
            assert!(config.injected_only);
            let only = parse(r"\\.\DISPLAY1 o --injected-only").unwrap();
            assert!(only.injected_only);
            assert_eq!(parse(r"\\.\DISPLAY1 o --fps current").unwrap().fps, None);
            for bad in [
                "",
                r"\\.\DISPLAY1",
                "DISPLAY1 o.json",
                r"\\.\DISPLAY1 --seconds",
            ] {
                assert!(parse(bad).is_err(), "{bad}");
            }
            for bad in [
                "--seconds",
                "--seconds 0",
                "--seconds 3601",
                "--fps 0",
                "--fps fast",
                "--gain nan",
                "--gain inf",
                "--lag-ms -1",
                "--stick-deadzone 0.6",
                "--stick-exponent 1.2",
                "--hud maybe",
                "--barcode maybe",
                "--barcode",
                "--injected-only on",
                "--pad 4",
                "--speed 2",
            ] {
                assert!(
                    parse(&format!(r"\\.\DISPLAY1 o.json {bad}")).is_err(),
                    "{bad}"
                );
            }
        }

        #[test]
        fn outputs_sit_beside_the_report() {
            let frames = sibling(Path::new("out/run.json"), "frames");
            assert_eq!(frames, PathBuf::from("out/run.frames.csv"));
            assert_eq!(
                sibling(Path::new("run"), "mouse"),
                PathBuf::from("run.mouse.csv")
            );
            assert_eq!(qpc_us(30_000_000, 10_000_000), 3_000_000);
            assert_eq!(qpc_us(1, 3), 333_333);
        }

        // The protocol crate's own expectations for the original.
        #[test]
        fn stick_curve_matches_the_protocol() {
            assert_eq!(stick_curve(0, 0, 0.1, 1.0), [0.0, 0.0]);
            assert_eq!(stick_curve(3000, 0, 0.1, 1.0), [0.0, 0.0]);
            let full = stick_curve(32767, 0, 0.1, 2.0);
            assert!((full[0] - 1.0).abs() < 1e-5 && full[1] == 0.0);
            let left = stick_curve(-32768, 0, 0.0, 1.0);
            assert!((left[0] + 1.0).abs() < 1e-5);
            let half = stick_curve(16384, 0, 0.0, 1.5);
            assert!((half[0] - 0.353_6).abs() < 1e-3, "{half:?}");
            let diagonal = stick_curve(20000, 20000, 0.0, 1.0);
            assert!((diagonal[0] - diagonal[1]).abs() < 1e-6);
            assert!(diagonal[0] > 0.6 && diagonal[0] <= 0.7072);
            let corner = stick_curve(32767, 32767, 0.0, 1.0);
            let length = (corner[0] * corner[0] + corner[1] * corner[1]).sqrt();
            assert!((length - 1.0).abs() < 1e-5);
        }

        #[test]
        fn mouse_sums_are_looked_up_by_time() {
            let mut mouse = MouseHistory::default();
            assert_eq!(mouse.at(0), [0, 0]);
            mouse.push(100, 10, -2, true);
            mouse.push(200, 5, 1, false);
            // A late timestamp is held at the last one.
            mouse.push(150, 1, 1, true);
            assert_eq!(mouse.moves[2].at_us, 200);
            assert_eq!(mouse.at(99), [0, 0]);
            assert_eq!(mouse.at(100), [10, -2]);
            assert_eq!(mouse.at(199), [10, -2]);
            assert_eq!(mouse.at(200), [16, 0]);
            assert_eq!(mouse.at(i64::MAX), [16, 0]);
        }

        #[test]
        fn injected_only_sums_leave_device_moves_out_but_keep_them() {
            let mut mouse = MouseHistory::new(true);
            mouse.push(100, 10, -2, true);
            mouse.push(200, 5, 1, false);
            mouse.push(300, 1, 1, true);
            assert_eq!(mouse.moves.len(), 3);
            assert_eq!(mouse.moves[1].sum, [10, -2]);
            assert!(!mouse.moves[1].injected);
            assert_eq!(mouse.at(250), [10, -2]);
            assert_eq!(mouse.at(300), [11, -1]);
        }

        #[test]
        fn the_stick_integral_holds_each_poll_until_the_next() {
            let mut stick = StickHistory::new(1_000_000, 0.0, 1.0);
            assert_eq!(stick.integral(0), [0.0, 0.0]);
            // Full right from 1.5 s; the same stick again adds no sample.
            stick.push(1_500_000, Some(1), (32767, 0));
            stick.push(1_600_000, Some(2), (32767, 0));
            assert_eq!(stick.samples.len(), 2);
            // Full down from 2 s, then the pad is gone and reads as centred.
            stick.push(2_000_000, Some(3), (0, -32768));
            stick.push(2_250_000, None, (0, 0));
            let close =
                |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).abs() < 1e-4 && (a[1] - b[1]).abs() < 1e-4;
            assert!(close(stick.integral(1_500_000), [0.0, 0.0]));
            assert!(close(stick.integral(1_750_000), [0.25, 0.0]));
            assert!(close(stick.integral(2_000_000), [0.5, 0.0]));
            assert!(close(stick.integral(2_100_000), [0.5, -0.1]));
            assert!(close(stick.integral(3_000_000), [0.5, -0.25]));
        }

        #[test]
        fn the_camera_follows_both_inputs_and_pitch_stops_at_its_limit() {
            let mut camera = Camera::new(2.0, 100.0, 50.0);
            camera.update([10, 5], [0.5, 0.25]);
            assert_eq!((camera.x, camera.y), (70.0, 35.0));
            camera.update([10, 100], [0.5, 0.25]);
            assert_eq!(camera.y, 50.0);
            // Turning back moves at once: 225 to 205.
            camera.update([10, 90], [0.5, 0.25]);
            assert_eq!(camera.y, 30.0);
            camera.update([-10, -100], [0.0, 0.0]);
            assert_eq!((camera.x, camera.y), (-20.0, -50.0));
        }
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("This probe requires Windows.");
}

#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    probe::run()
}

#[cfg(windows)]
mod probe {
    use crate::look::{Camera, Config, MouseHistory, StickHistory, qpc_us, sibling};
    use anyhow::{Context, Result, bail};
    use butterpollo_windows::capture::{ComGuard, displays, enable_dpi_awareness};
    use std::{
        io::{BufWriter, Write},
        path::Path,
        sync::atomic::{AtomicBool, Ordering},
        time::{Duration, Instant},
    };
    use windows::{
        Win32::{
            Foundation::*,
            Graphics::{
                Direct3D::{D3D_DRIVER_TYPE_UNKNOWN, D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST, Fxc::*},
                Direct3D11::*,
                Dxgi::{Common::*, *},
            },
            System::{LibraryLoader::GetModuleHandleW, Performance::*, Threading::*},
            UI::{
                Input::{KeyboardAndMouse::VK_ESCAPE, *},
                WindowsAndMessaging::*,
            },
        },
        core::{Interface, PCSTR, PCWSTR, w},
    };

    const SHADER: &str = r#"
cbuffer Frame : register(b0) {
    float cameraX;      // wrapped into [0, worldWidth)
    float cameraY;      // pixels, down; 0 puts the horizon at the screen's centre
    float worldWidth;   // four screen widths; the world wraps there
    float screenWidth;
    float screenHeight;
    uint hud;
    uint frame;         // the frames.csv frame number, from 1
    uint barcode;
    uint ticksLow;      // QueryPerformanceCounter at render time
    uint ticksHigh;
    uint unused0;
    uint unused1;
};
float4 vertex(uint id : SV_VertexID) : SV_Position {
    float2 p = float2((id << 1) & 2, id & 2);
    return float4(p * float2(2, -2) + float2(-1, 1), 0, 1);
}
uint hash(uint x) {
    x ^= x >> 16; x *= 0x7feb352du;
    x ^= x >> 15; x *= 0x846ca68bu;
    x ^= x >> 16;
    return x;
}
float random(uint x, uint y, uint seed) {
    uint h = hash(x * 0x8da6b343u ^ hash(y * 0xd8163841u ^ seed * 0xcb1ab31fu));
    return float(h >> 8) / 16777215.0;
}
// Smooth value noise with cells of about `cell` pixels. A whole number of cells spans the
// world and the x lattice wraps there, so the world wraps without a seam. x is never negative.
float noise(float2 w, float cell, uint seed) {
    uint period = uint(max(1.0, round(worldWidth / cell)));
    float2 offset = float2(random(1, 2, seed), random(3, 4, seed)) * 61.0;
    float2 p = w * (float(period) / worldWidth) + offset;
    float2 i = floor(p);
    float2 f = p - i;
    float2 u = f * f * (3.0 - 2.0 * f);
    uint x0 = uint(i.x) % period;
    uint x1 = (x0 + 1) % period;
    uint y0 = asuint(int(i.y));
    uint y1 = asuint(int(i.y) + 1);
    float a = random(x0, y0, seed), b = random(x1, y0, seed);
    float c = random(x0, y1, seed), d = random(x1, y1, seed);
    return lerp(lerp(a, b, u.x), lerp(c, d, u.x), u.y);
}
float fbm(float2 w, float cell, uint seed, uint octaves) {
    float sum = 0, weight = 0, amplitude = 1;
    [loop] for (uint k = 0; k < octaves; k++) {
        sum += amplitude * noise(w, cell, seed + k * 977u);
        weight += amplitude;
        amplitude *= 0.5;
        cell *= 0.5;
    }
    return sum / weight;
}
float3 sky(float2 w) {
    float up = saturate(-w.y / screenHeight);
    float3 color = lerp(float3(0.72, 0.80, 0.87), float3(0.16, 0.36, 0.70), sqrt(up));
    // Clouds, flatter than wide.
    float cloud = fbm(float2(w.x, w.y * 2.5), 320.0, 11u, 4u);
    float cover = smoothstep(0.46, 0.72, cloud);
    float shade = 0.78 + 0.22 * noise(w, 40.0, 13u);
    color = lerp(color, float3(0.96, 0.96, 0.98) * shade, 0.9 * cover);
    // Faint grain, so open sky still has texture to match.
    return color * (0.94 + 0.12 * noise(w, 5.0, 17u));
}
float3 ground(float2 w) {
    float near = saturate(w.y / screenHeight);
    float patches = fbm(w, 288.0, 23u, 5u);
    float detail = fbm(w, 72.0, 29u, 3u);
    float3 grass = lerp(float3(0.14, 0.27, 0.07), float3(0.36, 0.48, 0.15), detail);
    float3 dirt = lerp(float3(0.28, 0.21, 0.12), float3(0.50, 0.40, 0.25), detail);
    float3 color = lerp(grass, dirt, smoothstep(0.40, 0.64, patches));
    color *= 0.78 + 0.44 * noise(w, 4.0, 31u);
    // Haze toward the horizon.
    return lerp(color, float3(0.66, 0.72, 0.76), 0.55 * (1.0 - smoothstep(0.0, 0.15, near)));
}
// Poles and towers standing just below the horizon, at most one in each slot of about 150
// pixels. A whole number of slots spans the world, so they wrap with it.
bool structure(float2 w, out float3 color) {
    color = 0;
    uint slots = uint(max(1.0, round(worldWidth / 150.0)));
    float slotWidth = worldWidth / float(slots);
    uint slot = min(uint(w.x / slotWidth), slots - 1);
    uint h = hash(slot * 0x9e3779b9u + 0x632be5abu);
    if ((h & 15u) < 7u) return false;
    bool tower = ((h >> 4) & 3u) == 0;
    float r1 = float((h >> 8) & 255u) / 255.0;
    float r2 = float((h >> 16) & 255u) / 255.0;
    float r3 = float(h >> 24) / 255.0;
    float width = tower ? lerp(36.0, 110.0, r1) : lerp(4.0, 16.0, r1);
    float height = screenHeight * (tower ? lerp(0.2, 0.8, r2) : lerp(0.08, 0.55, r2));
    float base = lerp(6.0, 0.1 * screenHeight, r3);
    uint g = hash(h);
    float left = float(slot) * slotWidth + (slotWidth - width) * float(g & 1023u) / 1023.0;
    float x = w.x - left;
    if (x < 0 || x >= width || w.y > base || w.y < base - height) return false;
    float tone = lerp(0.10, 0.50, float((g >> 10) & 255u) / 255.0);
    float3 tint = lerp(float3(1.0, 1.0, 1.0), float3(1.0, 0.90, 0.80), float((g >> 18) & 255u) / 255.0);
    // Lit from the left like a cylinder, weathered so it has texture of its own.
    float across = x / width;
    float light = 1.15 - 0.55 * across * across;
    float wear = 0.82 + 0.36 * noise(w, tower ? 10.0 : 4.0, h);
    color = tone * tint * light * wear;
    return true;
}
bool inside(int2 p, int2 at, int2 size) {
    return all(p >= at) && all(p < at + size);
}
// The crosshair's four arms and centre dot, each grown by `grow` pixels.
bool crosshair(int2 p, int2 c, int k, int grow) {
    int2 g = int2(grow, grow);
    return inside(p, c + int2(-13 * k, -k) - g, int2(10 * k, 2 * k) + 2 * g)
        || inside(p, c + int2(3 * k, -k) - g, int2(10 * k, 2 * k) + 2 * g)
        || inside(p, c + int2(-k, -13 * k) - g, int2(2 * k, 10 * k) + 2 * g)
        || inside(p, c + int2(-k, 3 * k) - g, int2(2 * k, 10 * k) + 2 * g)
        || inside(p, c + int2(-k, -k) - g, int2(2 * k, 2 * k) + 2 * g);
}
static const uint SEGMENTS[10] = { 0x3Fu, 0x06u, 0x5Bu, 0x4Fu, 0x66u, 0x6Du, 0x7Du, 0x07u, 0x7Fu, 0x6Fu };
// A seven-segment digit, 14k wide and 26k tall, at `at`.
bool digit(int2 p, int2 at, uint value, int k) {
    int2 q = p - at;
    int w = 14 * k, h = 26 * k, t = 3 * k, m = 13 * k - (t >> 1);
    uint s = SEGMENTS[value];
    return ((s & 1u) != 0 && inside(q, int2(t, 0), int2(w - 2 * t, t)))
        || ((s & 2u) != 0 && inside(q, int2(w - t, t), int2(t, m - t)))
        || ((s & 4u) != 0 && inside(q, int2(w - t, m + t), int2(t, h - m - 2 * t)))
        || ((s & 8u) != 0 && inside(q, int2(t, h - t), int2(w - 2 * t, t)))
        || ((s & 16u) != 0 && inside(q, int2(0, m + t), int2(t, h - m - 2 * t)))
        || ((s & 32u) != 0 && inside(q, int2(0, t), int2(t, m - t)))
        || ((s & 64u) != 0 && inside(q, int2(t, m), int2(w - 2 * t, t)));
}
// The HUD: fixed in screen space in exact colours, so it stays pixel-identical while the
// camera turns. Sizes are in units of k pixels, 1 per 540 rows.
bool overlay(int2 p, out float3 color) {
    color = 0;
    int width = int(screenWidth), height = int(screenHeight);
    int k = max(1, int(screenHeight / 540.0));
    int margin = 16 * k;
    int2 centre = int2(width >> 1, height >> 1);
    if (crosshair(p, centre, k, 0)) { color = float3(0.20, 1.00, 0.30); return true; }
    if (crosshair(p, centre, k, k)) { color = float3(0.02, 0.02, 0.02); return true; }
    // Health bar, bottom left, 72% full.
    int2 barSize = int2(200 * k, 16 * k);
    int2 barAt = int2(margin, height - margin - barSize.y);
    if (inside(p, barAt, barSize)) {
        int2 q = p - barAt;
        if (!inside(q, int2(2 * k, 2 * k), barSize - 4 * k)) color = float3(0.04, 0.04, 0.04);
        else if (q.x < 2 * k + int(uint((barSize.x - 4 * k) * 72) / 100u)) color = float3(0.86, 0.13, 0.10);
        else color = float3(0.28, 0.06, 0.05);
        return true;
    }
    // Ammo counter, bottom right: 30 | 90.
    int2 ammoSize = int2(112 * k, 44 * k);
    int2 ammoAt = int2(width - margin - ammoSize.x, height - margin - ammoSize.y);
    if (inside(p, ammoAt, ammoSize)) {
        int2 q = p - ammoAt;
        if (!inside(q, int2(2 * k, 2 * k), ammoSize - 4 * k)) color = float3(0.75, 0.75, 0.78);
        else if (digit(q, int2(10 * k, 9 * k), 3, k) || digit(q, int2(28 * k, 9 * k), 0, k)) color = float3(1.00, 0.84, 0.28);
        else if (inside(q, int2(52 * k, 8 * k), int2(2 * k, 28 * k))) color = float3(0.75, 0.75, 0.78);
        else if (digit(q, int2(62 * k, 9 * k), 9, k) || digit(q, int2(80 * k, 9 * k), 0, k)) color = float3(0.80, 0.80, 0.80);
        else color = float3(0.09, 0.10, 0.12);
        return true;
    }
    // Minimap, top right: a static map 170 units on a side.
    int mapSize = 170 * k;
    int2 mapAt = int2(width - margin - mapSize, margin);
    if (inside(p, mapAt, int2(mapSize, mapSize))) {
        int2 q = int2(uint2(p - mapAt) / uint(k));
        if (!inside(q, int2(2, 2), int2(166, 166))) color = float3(0.80, 0.80, 0.82);
        else if (inside(q, int2(80, 80), int2(10, 10))) color = float3(1.00, 1.00, 1.00);
        else if (inside(q, int2(120, 112), int2(48, 56))) color = float3(0.14, 0.30, 0.55);
        else if (inside(q, int2(30, 24), int2(34, 26)) || inside(q, int2(104, 20), int2(28, 44))
            || inside(q, int2(20, 108), int2(26, 40)) || inside(q, int2(78, 128), int2(30, 22)))
            color = float3(0.46, 0.46, 0.50);
        else if (inside(q, int2(2, 92), int2(166, 8)) || inside(q, int2(66, 2), int2(8, 166)))
            color = float3(0.36, 0.33, 0.27);
        else color = float3(0.13, 0.19, 0.12);
        return true;
    }
    return false;
}
// motion_probe's barcode, drawn as it draws it: four rows, sequence, QPC low, QPC high and a
// signature, with black and white references at x=12/36 for the reader.
bool stamp(float4 p, out float4 color) {
    color = float4(0, 0, 0, 1);
    if (p.y < 8 || p.y >= 104) return false;
    if (p.x < 24) return true;
    if (p.x < 48) { color = float4(1.25, 1.25, 1.25, 1); return true; }
    if (p.x >= 64 && p.x < 576) {
        uint row = uint(p.y - 8) / 24;
        uint word = row == 0 ? frame : row == 1 ? ticksLow : row == 2 ? ticksHigh : 0xB17E2212;
        float value = ((word >> (uint(p.x - 64) / 16)) & 1) * 1.25;
        color = float4(value, value, value, 1);
        return true;
    }
    return false;
}
float4 picture(float4 position : SV_Position) : SV_Target {
    float3 color;
    float4 bars;
    if (barcode != 0 && stamp(position, bars)) return bars;
    if (hud != 0 && overlay(int2(position.xy), color)) return float4(color, 1);
    // The world point under this pixel is the screen's plus the camera's, so the content
    // moves against the camera. World y 0 is the horizon.
    float2 w = position.xy + float2(cameraX, cameraY - 0.5 * screenHeight);
    if (w.x >= worldWidth) w.x -= worldWidth;
    if (structure(w, color)) return float4(color, 1);
    if (w.y < 0) return float4(sky(w), 1);
    return float4(ground(w), 1);
}
"#;

    struct Window(HWND);
    impl Drop for Window {
        fn drop(&mut self) {
            unsafe {
                let _ = DestroyWindow(self.0);
            }
        }
    }
    struct Handle(HANDLE);
    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
    static DISPLAY_CHANGED: AtomicBool = AtomicBool::new(false);

    unsafe extern "system" fn window(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        // A game hides the pointer over its picture; a moving pointer is neither scene nor HUD.
        if message == WM_SETCURSOR && (lparam.0 & 0xFFFF) as u32 == HTCLIENT {
            unsafe { SetCursor(None) };
            return LRESULT(1);
        }
        if message == WM_DISPLAYCHANGE {
            DISPLAY_CHANGED.store(true, Ordering::Relaxed);
        }
        unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
    }

    unsafe fn compile(entry: &'static [u8], target: &'static [u8]) -> Result<Vec<u8>> {
        unsafe {
            let mut code = None;
            let mut errors = None;
            let result = D3DCompile(
                SHADER.as_ptr().cast(),
                SHADER.len(),
                PCSTR::null(),
                None,
                None,
                PCSTR(entry.as_ptr()),
                PCSTR(target.as_ptr()),
                D3DCOMPILE_OPTIMIZATION_LEVEL3,
                0,
                &mut code,
                Some(&mut errors),
            );
            if let Err(error) = result {
                let details = errors
                    .map(|blob| {
                        String::from_utf8_lossy(std::slice::from_raw_parts(
                            blob.GetBufferPointer().cast(),
                            blob.GetBufferSize(),
                        ))
                        .into_owned()
                    })
                    .unwrap_or_default();
                bail!("look shader: {error}: {details}");
            }
            let code = code.context("shader compiler returned no bytecode")?;
            Ok(
                std::slice::from_raw_parts(code.GetBufferPointer().cast(), code.GetBufferSize())
                    .to_vec(),
            )
        }
    }

    fn now_ticks() -> i64 {
        let mut ticks = 0;
        // SAFETY: QueryPerformanceCounter writes one i64; it cannot fail since Windows XP.
        let _ = unsafe { QueryPerformanceCounter(&mut ticks) };
        ticks
    }

    fn now_us(frequency: i64) -> i64 {
        qpc_us(now_ticks(), frequency)
    }

    type GetState = unsafe extern "system" fn(u32, *mut [u32; 4]) -> u32;

    /// XInput from System32, loaded at run time as the host's own pad code does.
    struct XInput {
        _library: libloading::Library,
        get: GetState,
    }
    impl XInput {
        fn load() -> Result<Self> {
            let system = std::env::var("SystemRoot").context("SystemRoot is not set")?;
            // SAFETY: xinput1_4 from System32 has no loader requirements, the signature is
            // XInputGetState's (XINPUT_STATE is 16 bytes, 4-aligned), and the library is kept
            // as long as the pointer.
            unsafe {
                let library =
                    libloading::Library::new(format!(r"{system}\System32\xinput1_4.dll"))?;
                let get = *library.get::<GetState>(b"XInputGetState\0")?;
                Ok(Self {
                    _library: library,
                    get,
                })
            }
        }
        /// The packet number and right stick of pad `index`, or None when it is not connected.
        fn right_stick(&self, index: u32) -> Option<(u32, (i16, i16))> {
            let mut state = [0u32; 4];
            // SAFETY: `state` has XINPUT_STATE's size and alignment and outlives the call.
            let result = unsafe { (self.get)(index, &mut state) };
            // dwPacketNumber; wButtons and the triggers; the left stick; the right stick.
            (result == 0).then(|| {
                let right = state[3];
                (state[0], (right as u16 as i16, (right >> 16) as u16 as i16))
            })
        }
    }

    /// Raw input as it arrives, and the counts the report gives.
    struct Input {
        frequency: i64,
        mouse: MouseHistory,
        injected: u64,
        device: u64,
        absolute_ignored: u64,
        errors: u64,
        escape: bool,
    }
    impl Input {
        /// Handles every queued message. Raw input is read here, timestamped as it is handled,
        /// and still dispatched so DefWindowProc cleans up after it.
        fn pump(&mut self) -> Result<()> {
            let mut message = MSG::default();
            // SAFETY: message pumping on the thread that owns the window.
            unsafe {
                while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                    if message.message == WM_QUIT {
                        bail!("test window closed");
                    }
                    if message.message == WM_INPUT {
                        self.raw(HRAWINPUT(message.lParam.0 as *mut _));
                    }
                    let _ = TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
            Ok(())
        }

        fn raw(&mut self, handle: HRAWINPUT) {
            let at_us = now_us(self.frequency);
            let mut packet = RAWINPUT::default();
            let mut size = size_of::<RAWINPUT>() as u32;
            // SAFETY: `packet` has room for `size` bytes, which holds a mouse or keyboard
            // packet, the only devices registered; the handle is this WM_INPUT's.
            let read = unsafe {
                GetRawInputData(
                    handle,
                    RID_INPUT,
                    Some((&raw mut packet).cast()),
                    &mut size,
                    size_of::<RAWINPUTHEADER>() as u32,
                )
            };
            if read == 0 || read == u32::MAX {
                self.errors += 1;
                return;
            }
            if packet.header.dwType == RIM_TYPEMOUSE.0 {
                // SAFETY: the header says the packet is a mouse's.
                let mouse = unsafe { packet.data.mouse };
                if mouse.usFlags.0 & MOUSE_MOVE_ABSOLUTE.0 != 0 {
                    self.absolute_ignored += 1;
                } else if mouse.lLastX != 0 || mouse.lLastY != 0 {
                    // SendInput's input has no device.
                    let injected = packet.header.hDevice.0.is_null();
                    if injected {
                        self.injected += 1;
                    } else {
                        self.device += 1;
                    }
                    self.mouse.push(at_us, mouse.lLastX, mouse.lLastY, injected);
                }
            } else if packet.header.dwType == RIM_TYPEKEYBOARD.0 {
                // SAFETY: the header says the packet is a keyboard's.
                let key = unsafe { packet.data.keyboard };
                if key.VKey == VK_ESCAPE.0 && u32::from(key.Flags) & RI_KEY_BREAK == 0 {
                    self.escape = true;
                }
            }
        }
    }

    /// Waits for `handle` while handling messages as they arrive, so raw input is timestamped
    /// when it comes rather than once a frame. False on a timeout.
    fn wait_pumping(handle: HANDLE, timeout_ms: u32, input: &mut Input) -> Result<bool> {
        loop {
            // SAFETY: the handle is a live waitable object the caller owns.
            let result = unsafe {
                MsgWaitForMultipleObjectsEx(
                    Some(&[handle]),
                    timeout_ms,
                    QS_ALLINPUT,
                    MWMO_INPUTAVAILABLE,
                )
            };
            if result == WAIT_OBJECT_0 {
                return Ok(true);
            }
            if result == WAIT_TIMEOUT {
                return Ok(false);
            }
            if result.0 != WAIT_OBJECT_0.0 + 1 {
                bail!("wait failed: {}", windows::core::Error::from_thread());
            }
            input.pump()?;
        }
    }

    struct FrameRow {
        render_us: i64,
        present_us: i64,
        input_us: i64,
        mouse: [i64; 2],
        stick: [f64; 2],
        camera: [f64; 2],
    }

    /// A dead zone or exponent as given, without f32's tail.
    fn short(value: f32) -> f64 {
        (f64::from(value) * 1e6).round() / 1e6
    }

    fn write_csv(
        path: &Path,
        header: &str,
        rows: impl FnOnce(&mut dyn Write) -> std::io::Result<()>,
    ) -> Result<()> {
        let file = std::fs::File::create(path)
            .with_context(|| format!("cannot create {}", path.display()))?;
        let mut out = BufWriter::new(file);
        writeln!(out, "{header}")?;
        rows(&mut out)?;
        out.flush()?;
        Ok(())
    }

    pub fn run() -> Result<()> {
        let config = Config::parse(std::env::args().skip(1))?;
        if let Some(parent) = config.out.parent()
            && !parent.as_os_str().is_empty()
            && !parent.is_dir()
        {
            bail!("output folder {} does not exist", parent.display());
        }
        enable_dpi_awareness();
        let _com = ComGuard::new()?;
        let active = displays()?;
        let names: Vec<_> = active.iter().map(|d| d.display_name.clone()).collect();
        let mut display = active
            .into_iter()
            .find(|d| d.display_name.eq_ignore_ascii_case(&config.display))
            .with_context(|| {
                format!(
                    "{} is not an active display (active: {})",
                    config.display,
                    names.join(", ")
                )
            })?;
        if display.width < 640 || display.height < 360 {
            bail!("the look probe needs a display of at least 640 by 360");
        }
        let source_mode = butterpollo_windows::display::mode(&display.display_name)?;
        println!("LOOK stage=display name={}", display.display_name);
        let xinput = match XInput::load() {
            Ok(xinput) => Some(xinput),
            Err(error) => {
                println!("LOOK pad=unavailable error={error:#}");
                None
            }
        };
        let (width, height) = (display.width, display.height);
        let world_width = f64::from(width) * 4.0;
        let y_limit = f64::from(height) / 2.0;
        unsafe {
            let module = GetModuleHandleW(None)?;
            let class = WNDCLASSW {
                lpfnWndProc: Some(window),
                hInstance: HINSTANCE(module.0),
                lpszClassName: w!("ButterpolloLookProbe"),
                ..Default::default()
            };
            if RegisterClassW(&class) == 0 {
                bail!("cannot register test window");
            }
            // Never activated, as motion_probe's: it must not take the focus from anything.
            let window = Window(CreateWindowExW(
                WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
                w!("ButterpolloLookProbe"),
                w!("Rubylight test look"),
                WS_POPUP | WS_VISIBLE,
                display.x,
                display.y,
                width as i32,
                height as i32,
                None,
                None,
                Some(HINSTANCE(module.0)),
                None,
            )?);
            SetWindowPos(
                window.0,
                Some(HWND_TOPMOST),
                display.x,
                display.y,
                width as i32,
                height as i32,
                SWP_NOACTIVATE,
            )?;
            println!("LOOK stage=window");
            let factory: IDXGIFactory2 = CreateDXGIFactory1()?;
            let adapter = factory.EnumAdapters1(display.adapter_index)?;
            let mut device = None;
            let mut context = None;
            D3D11CreateDevice(
                &adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )?;
            let device = device.context("no D3D11 device")?;
            let context = context.context("no D3D11 context")?;
            let swap = factory.CreateSwapChainForHwnd(
                &device,
                window.0,
                &DXGI_SWAP_CHAIN_DESC1 {
                    Width: width,
                    Height: height,
                    Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    SampleDesc: DXGI_SAMPLE_DESC {
                        Count: 1,
                        Quality: 0,
                    },
                    BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                    BufferCount: 2,
                    SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
                    Scaling: DXGI_SCALING_STRETCH,
                    AlphaMode: DXGI_ALPHA_MODE_IGNORE,
                    Flags: DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT.0 as u32,
                    ..Default::default()
                },
                None,
                None,
            )?;
            let swap: IDXGISwapChain3 = swap.cast()?;
            swap.SetMaximumFrameLatency(1)?;
            let latency = Handle(swap.GetFrameLatencyWaitableObject());
            if latency.0.is_invalid() {
                bail!("swap chain has no presentation event");
            }
            println!("LOOK stage=swap-chain");
            let texture: ID3D11Texture2D = swap.GetBuffer(0)?;
            let mut target = None;
            device.CreateRenderTargetView(&texture, None, Some(&mut target))?;
            let mut vertex = None;
            let mut picture = None;
            device.CreateVertexShader(
                &compile(b"vertex\0", b"vs_5_0\0")?,
                None,
                Some(&mut vertex),
            )?;
            device.CreatePixelShader(
                &compile(b"picture\0", b"ps_5_0\0")?,
                None,
                Some(&mut picture),
            )?;
            let mut constants = None;
            device.CreateBuffer(
                &D3D11_BUFFER_DESC {
                    ByteWidth: 48,
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                    ..Default::default()
                },
                None,
                Some(&mut constants),
            )?;
            let constants = constants.context("no constant buffer")?;
            context.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            context.VSSetShader(vertex.as_ref(), None);
            context.PSSetShader(picture.as_ref(), None);
            context.PSSetConstantBuffers(0, Some(&[Some(constants.clone())]));
            context.RSSetViewports(Some(&[D3D11_VIEWPORT {
                Width: width as f32,
                Height: height as f32,
                MinDepth: 0.,
                MaxDepth: 1.,
                ..Default::default()
            }]));
            // Paces a fixed rate while still handling input.
            let timer = Handle(
                CreateWaitableTimerExW(
                    None,
                    PCWSTR::null(),
                    CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
                    TIMER_ALL_ACCESS.0,
                )
                .or_else(|_| CreateWaitableTimerExW(None, PCWSTR::null(), 0, TIMER_ALL_ACCESS.0))?,
            );
            let mut frequency = 0;
            QueryPerformanceFrequency(&mut frequency)?;
            // Mouse and keyboard, whether or not the window has the focus.
            let devices = [2, 6].map(|usage| RAWINPUTDEVICE {
                usUsagePage: 1,
                usUsage: usage,
                dwFlags: RIDEV_INPUTSINK,
                hwndTarget: window.0,
            });
            RegisterRawInputDevices(&devices, size_of::<RAWINPUTDEVICE>() as u32)
                .context("cannot register for raw input")?;
            println!("LOOK stage=render-ready");

            let start = Instant::now();
            let start_us = now_us(frequency);
            let mut input = Input {
                frequency,
                mouse: MouseHistory::new(config.injected_only),
                injected: 0,
                device: 0,
                absolute_ignored: 0,
                errors: 0,
                escape: false,
            };
            let mut stick =
                StickHistory::new(start_us, config.stick_deadzone, config.stick_exponent);
            let mut camera = Camera::new(config.gain, config.stick_gain, y_limit);
            let period = config
                .fps
                .map(|hz| Duration::from_secs_f64(1.0 / f64::from(hz)));
            let duration = Duration::from_secs(config.seconds);
            let mut frames = Vec::new();
            let mut next_frame = start;
            let mut progress_at = start + Duration::from_secs(5);
            let (mut pad_connected, mut pad_present) = (false, false);
            let mut pad_retry_at = start;
            let mut exit = "time";
            while start.elapsed() < duration {
                if !wait_pumping(latency.0, 1000, &mut input)? {
                    bail!("presentation event timed out");
                }
                if let Some(period) = period {
                    let remaining = next_frame.saturating_duration_since(Instant::now());
                    if !remaining.is_zero() {
                        // Relative, in 100 ns units.
                        let due = -(i64::try_from(remaining.as_nanos() / 100)
                            .unwrap_or(i64::MAX)
                            .max(1));
                        SetWaitableTimer(timer.0, &due, 0, None, None, false)?;
                        if !wait_pumping(timer.0, 5000, &mut input)? {
                            bail!("frame timer timed out");
                        }
                    }
                    // Bound catch-up after a stall to one frame, as motion_probe does.
                    next_frame = (next_frame + period).max(Instant::now());
                }
                input.pump()?;
                if input.escape {
                    exit = "escape";
                    break;
                }
                if DISPLAY_CHANGED.swap(false, Ordering::Relaxed)
                    && let Some(current) = displays()?
                        .into_iter()
                        .find(|d| d.display_name == display.display_name)
                {
                    if (current.width, current.height) != (width, height) {
                        bail!("the look probe's display changed size");
                    }
                    // A layout change can move the output without moving this popup.
                    if (current.x, current.y) != (display.x, display.y) {
                        SetWindowPos(
                            window.0,
                            None,
                            current.x,
                            current.y,
                            0,
                            0,
                            SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOSIZE,
                        )?;
                        display = current;
                    }
                }
                // The stick, polled once a frame as a game does. Polling a missing pad is
                // slow, so that is retried once a second.
                if let Some(xinput) = &xinput
                    && (pad_connected || Instant::now() >= pad_retry_at)
                {
                    let polled = xinput.right_stick(config.pad);
                    let at_us = now_us(frequency);
                    if polled.is_some() != pad_connected {
                        let state = if polled.is_some() {
                            "connected"
                        } else {
                            "missing"
                        };
                        println!("LOOK pad={} {state}", config.pad);
                    }
                    pad_connected = polled.is_some();
                    pad_present |= pad_connected;
                    match polled {
                        Some((packet, right)) => stick.push(at_us, Some(packet), right),
                        None => {
                            pad_retry_at = Instant::now() + Duration::from_secs(1);
                            stick.push(at_us, None, (0, 0));
                        }
                    }
                }
                // One reading for both the barcode's ticks and render_qpc_us.
                let render_ticks = now_ticks();
                let render_us = qpc_us(render_ticks, frequency);
                let input_us = render_us - config.lag_us;
                let mouse_sum = input.mouse.at(input_us);
                let integral = stick.integral(input_us);
                camera.update(mouse_sum, integral);
                // The barcode's sequence: this row's `frame` in frames.csv, from 1.
                let frame = frames.len() as u32 + 1;
                let values = [
                    (camera.x.rem_euclid(world_width) as f32).to_bits(),
                    (camera.y as f32).to_bits(),
                    (world_width as f32).to_bits(),
                    (width as f32).to_bits(),
                    (height as f32).to_bits(),
                    u32::from(config.hud),
                    frame,
                    u32::from(config.barcode),
                    render_ticks as u32,
                    (render_ticks as u64 >> 32) as u32,
                    0,
                    0,
                ];
                context.UpdateSubresource(&constants, 0, None, values.as_ptr().cast(), 0, 0);
                context.OMSetRenderTargets(Some(&[target.clone()]), None);
                context.Draw(3, 0);
                swap.Present(1, DXGI_PRESENT(0)).ok()?;
                let present_us = now_us(frequency);
                if frame == 1 {
                    println!("LOOK stage=first-present");
                }
                frames.push(FrameRow {
                    render_us,
                    present_us,
                    input_us,
                    mouse: mouse_sum,
                    stick: integral,
                    camera: [camera.x, camera.y],
                });
                if Instant::now() >= progress_at {
                    println!(
                        "LOOK_PROGRESS frames={frame} seconds={:.3} mouse_packets={}",
                        start.elapsed().as_secs_f64(),
                        input.injected + input.device
                    );
                    progress_at = Instant::now() + Duration::from_secs(5);
                }
            }
            let end_us = now_us(frequency);
            let seconds = start.elapsed().as_secs_f64();

            let frames_path = sibling(&config.out, "frames");
            let mouse_path = sibling(&config.out, "mouse");
            let stick_path = sibling(&config.out, "stick");
            write_csv(
                &frames_path,
                "frame,present_qpc_us,input_time_us,mouse_sum_x,mouse_sum_y,stick_integral_x,stick_integral_y,camera_x,camera_y,render_qpc_us",
                |out| {
                    for (i, f) in frames.iter().enumerate() {
                        writeln!(
                            out,
                            "{},{},{},{},{},{:.6},{:.6},{:.4},{:.4},{}",
                            i + 1,
                            f.present_us,
                            f.input_us,
                            f.mouse[0],
                            f.mouse[1],
                            f.stick[0],
                            f.stick[1],
                            f.camera[0],
                            f.camera[1],
                            f.render_us
                        )?;
                    }
                    Ok(())
                },
            )?;
            write_csv(&mouse_path, "qpc_us,dx,dy,sum_x,sum_y,injected", |out| {
                for m in &input.mouse.moves {
                    let injected = u8::from(m.injected);
                    writeln!(
                        out,
                        "{},{},{},{},{},{injected}",
                        m.at_us, m.dx, m.dy, m.sum[0], m.sum[1]
                    )?;
                }
                Ok(())
            })?;
            write_csv(
                &stick_path,
                "qpc_us,connected,packet,right_x,right_y,curve_x,curve_y,integral_x,integral_y",
                |out| {
                    for s in &stick.samples {
                        writeln!(
                            out,
                            "{},{},{},{},{},{:.6},{:.6},{:.6},{:.6}",
                            s.at_us,
                            u8::from(s.packet.is_some()),
                            s.packet.map_or(String::new(), |p| p.to_string()),
                            s.stick.0,
                            s.stick.1,
                            s.curve[0],
                            s.curve[1],
                            s.integral[0],
                            s.integral[1]
                        )?;
                    }
                    Ok(())
                },
            )?;
            let mouse_packets = input.injected + input.device;
            let report = serde_json::json!({
                "probe": "look_probe",
                "scope": "fake mouse-look game: camera = gain * mouse_sum(t - lag) + stick_gain * integral of stick_curve up to t - lag, t the frame's render time",
                "config": {
                    "display": config.display,
                    "seconds": config.seconds,
                    "fps": config.fps.map_or(serde_json::json!("current"), |hz| serde_json::json!(hz)),
                    "gain": config.gain,
                    "lag_ms": config.lag_us as f64 / 1000.0,
                    "stick_gain": config.stick_gain,
                    "stick_deadzone": short(config.stick_deadzone),
                    "stick_exponent": short(config.stick_exponent),
                    "hud": config.hud,
                    "barcode": config.barcode,
                    "pad": config.pad,
                    "injected_only": config.injected_only,
                },
                "display": display,
                "display_refresh_hz": source_mode.dmDisplayFrequency,
                "world_width": world_width,
                "camera_y_limit": y_limit,
                "qpc_frequency": frequency,
                "start_qpc_us": start_us,
                "end_qpc_us": end_us,
                "seconds": seconds,
                "exit": exit,
                "counts": {
                    "frames": frames.len(),
                    "mouse_packets": mouse_packets,
                    "mouse_injected_packets": input.injected,
                    "mouse_device_packets": input.device,
                    "absolute_packets_ignored": input.absolute_ignored,
                    "raw_input_errors": input.errors,
                    "pad_present": pad_present,
                    "stick_samples": stick.samples.len() - 1,
                },
                "expected_host_model": {
                    "mouse_gain": [-config.gain, -config.gain],
                    "stick_gain": [-config.stick_gain, -config.stick_gain],
                    "stick_deadzone": short(config.stick_deadzone),
                    "stick_exponent": short(config.stick_exponent),
                    "lag_us_at_least": config.lag_us,
                    "note": "content shift per input, display pixels, x right and y down; scale by stream width over display width; the host's lag adds the render-to-capture delay",
                },
                "files": {
                    "frames": frames_path.display().to_string(),
                    "mouse": mouse_path.display().to_string(),
                    "stick": stick_path.display().to_string(),
                },
            });
            std::fs::write(&config.out, serde_json::to_vec_pretty(&report)?)
                .with_context(|| format!("cannot write {}", config.out.display()))?;
            let last = frames.last().map_or([0.0; 2], |f| f.camera);
            println!(
                "LOOK frames={} seconds={seconds:.3} mouse_packets={mouse_packets} injected={} device={} absolute_ignored={} pad={} exit={exit} camera_x={:.1} camera_y={:.1} out={}",
                frames.len(),
                input.injected,
                input.device,
                input.absolute_ignored,
                if pad_present { "present" } else { "absent" },
                last[0],
                last[1],
                config.out.display()
            );
        }
        Ok(())
    }
}
