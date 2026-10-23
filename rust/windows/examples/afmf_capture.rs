//! Captures a display while `afmf_scene` runs on it and checks each captured
//! picture's marker band, to tell pictures the app rendered from ones the driver
//! generated (B7). Capture only; changes no display setting.
//!
//! usage: afmf_capture DISPLAY wgc|ddx SECONDS REPORT.json
//!
//! A picture is `rendered` when its frame number decodes and the bar sits exactly
//! where that number puts it, `generated` when the bar is elsewhere or blended,
//! and `unreadable` when no marker band is found (another window, a black frame).
#[cfg(not(windows))]
fn main() {
    eprintln!("This probe requires Windows.");
}

/// What one captured picture's marker band says.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum Verdict {
    Rendered(u32),
    Generated,
    Unreadable,
}

/// Pixels the bar moves per frame and its width; the same as `afmf_scene`.
const STEP: u32 = 48;
const BAR: u32 = 64;
const BAR_ROW: usize = 8;
const BIT_ROW: usize = 36;
const BITS: usize = 16;
const BIT_WIDTH: usize = 24;
const BIT_LEFT: usize = 16;

fn level(bgra: &[u8], stride: usize, x: usize, y: usize) -> (u8, u8, u8) {
    let at = y * stride + x * 4;
    (bgra[at + 2], bgra[at + 1], bgra[at])
}

/// Reads the marker band of a BGRA picture of `width` by `height`. Levels are taken
/// relative to the brightest bar pixel, because a display in HDR mode shows SDR white
/// dimmer than 255 and the capture keeps that.
fn judge(bgra: &[u8], stride: usize, width: usize, height: usize) -> Verdict {
    if width < 640 || height < 64 || bgra.len() < stride * height {
        return Verdict::Unreadable;
    }
    let peak = (0..width)
        .map(|x| level(bgra, stride, x, BAR_ROW).0)
        .max()
        .unwrap_or(0) as f32;
    if peak < 60. {
        return Verdict::Unreadable;
    }
    let relative = |(r, g, b): (u8, u8, u8)| (r as f32 / peak, g as f32 / peak, b as f32 / peak);
    let mut white = Vec::new();
    let mut blended = 0;
    for x in 0..width {
        let (r, g, b) = relative(level(bgra, stride, x, BAR_ROW));
        if r > 0.85 && g > 0.85 && b > 0.85 {
            white.push(x);
        } else if r > 0.1 || g > 0.1 || b > 0.45 {
            // The background is (0, 0, 0.3); a mix with the bar leaves something else.
            blended += 1;
        }
    }
    // The scene's dark blue, or this is another window (a bar is 64 pixels, not a row).
    if white.len() > width / 4 {
        return Verdict::Unreadable;
    }
    let (r, g, b) = relative(level(bgra, stride, 2, BAR_ROW));
    if !white.contains(&2) && !(r < 0.1 && g < 0.1 && (0.15..0.45).contains(&b)) {
        return Verdict::Unreadable;
    }
    let mut code = 0u32;
    let mut crisp = true;
    for bit in 0..BITS {
        let (r, g, b) = relative(level(
            bgra,
            stride,
            BIT_LEFT + bit * BIT_WIDTH + BIT_WIDTH / 2,
            BIT_ROW,
        ));
        if r > 0.85 && g > 0.85 && b > 0.85 {
            code |= 1 << bit;
        } else if r > 0.1 || g > 0.1 || b > 0.1 {
            crisp = false;
        }
    }
    let Some(&first) = white.first() else {
        return Verdict::Unreadable;
    };
    let contiguous = white
        .last()
        .is_some_and(|&last| last - first + 1 == white.len());
    let at = (code.wrapping_mul(STEP)) % (width as u32 - BAR);
    if crisp && contiguous && blended <= 2 && white.len() == BAR as usize && first as u32 == at {
        Verdict::Rendered(code)
    } else {
        Verdict::Generated
    }
}

#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    use anyhow::{Context, bail};
    use butterpollo_windows::capture::{Capture, ComGuard, enable_dpi_awareness};
    use std::time::{Duration, Instant};
    let mut args = std::env::args().skip(1);
    let usage = "usage: afmf_capture DISPLAY wgc|ddx SECONDS REPORT.json";
    let display = args.next().context(usage)?;
    let kind = args.next().context(usage)?;
    let seconds: u64 = args.next().context(usage)?.parse()?;
    let report = args.next().context(usage)?;
    if !matches!(kind.as_str(), "wgc" | "ddx") || !(1..=600).contains(&seconds) {
        bail!("{usage}; seconds 1..600");
    }
    enable_dpi_awareness();
    let _com = ComGuard::new()?;
    let mut capture = Capture::new(&display, &kind)?;
    let start = Instant::now();
    let mut pictures = Vec::new();
    let mut idle = 0u64;
    let mut sample = None;
    while start.elapsed() < Duration::from_secs(seconds) {
        match capture.next_frame()? {
            Some(image) => {
                let verdict = judge(
                    &image.bytes,
                    image.stride,
                    image.width as usize,
                    image.height as usize,
                );
                if sample.is_none() && start.elapsed() >= Duration::from_secs(2) {
                    // The marker band of one picture, for checking the judge by eye.
                    let rows = 64.min(image.height as usize);
                    sample = Some((
                        image.width,
                        image.bytes[..rows * image.stride].to_vec(),
                        image.stride,
                    ));
                }
                pictures.push((start.elapsed().as_secs_f64() * 1000., verdict));
            }
            None => {
                idle += 1;
                std::thread::sleep(Duration::from_micros(500));
            }
        }
    }
    let elapsed = start.elapsed().as_secs_f64();
    // Skip the first second: window and capture start-up.
    let steady: Vec<_> = pictures.iter().filter(|(t, _)| *t >= 1000.).collect();
    let count = |want: fn(&Verdict) -> bool| steady.iter().filter(|(_, v)| want(v)).count();
    let rendered = count(|v| matches!(v, Verdict::Rendered(_)));
    let generated = count(|v| *v == Verdict::Generated);
    let unreadable = count(|v| *v == Verdict::Unreadable);
    // Distinct app frames seen, and how many numbers the capture skipped.
    let mut codes: Vec<u32> = steady
        .iter()
        .filter_map(|(_, v)| match v {
            Verdict::Rendered(code) => Some(*code),
            _ => None,
        })
        .collect();
    codes.dedup();
    let skipped = codes
        .windows(2)
        .map(|pair| pair[1].wrapping_sub(pair[0]).saturating_sub(1) as u64)
        .filter(|gap| *gap < 1000)
        .sum::<u64>();
    let window = (elapsed - 1.).max(0.001);
    let data = serde_json::json!({
        "scope":"pictures a capture delivered while afmf_scene ran; says nothing about the screen",
        "display":display,"capture":kind,"seconds":elapsed,"idle_polls":idle,
        "captured_pictures":steady.len(),"captured_fps":steady.len() as f64 / window,
        "rendered":rendered,"generated":generated,"unreadable":unreadable,
        "distinct_app_frames":codes.len(),"app_frames_skipped":skipped,
        "pictures":pictures.iter().map(|(t, v)| serde_json::json!({
            "ms":t,"verdict":format!("{v:?}")
        })).collect::<Vec<_>>()
    });
    std::fs::write(&report, serde_json::to_vec_pretty(&data)?)?;
    if let Some((width, bytes, stride)) = sample {
        std::fs::write(format!("{report}.band-{width}x{stride}.bgra"), bytes)?;
    }
    println!(
        "AFMF_CAPTURE capture={kind} pictures={} fps={:.2} rendered={rendered} generated={generated} unreadable={unreadable} distinct_app_frames={} skipped={skipped}",
        steady.len(),
        steady.len() as f64 / window,
        codes.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIDTH: usize = 1920;

    /// What `afmf_scene` draws in its first 48 rows for `frame`.
    fn scene(frame: u32, bar_at: Option<u32>) -> Vec<u8> {
        scene_at(frame, bar_at, 1.)
    }

    /// The same band as a display in HDR mode shows it: scaled by `gain`.
    fn scene_at(frame: u32, bar_at: Option<u32>, gain: f32) -> Vec<u8> {
        let mut bgra = vec![0u8; WIDTH * 64 * 4];
        let stride = WIDTH * 4;
        let mut put = |x: usize, y: usize, rgb: [u8; 3]| {
            let at = y * stride + x * 4;
            bgra[at] = (rgb[2] as f32 * gain).round() as u8;
            bgra[at + 1] = (rgb[1] as f32 * gain).round() as u8;
            bgra[at + 2] = (rgb[0] as f32 * gain).round() as u8;
            bgra[at + 3] = 255;
        };
        let bar = bar_at.unwrap_or(frame * STEP % (WIDTH as u32 - BAR));
        for y in 0..24 {
            for x in 0..WIDTH {
                let on = (bar..bar + BAR).contains(&(x as u32));
                put(x, y, if on { [255; 3] } else { [0, 0, 77] });
            }
        }
        for y in 24..48 {
            for bit in 0..BITS {
                let value = if frame >> bit & 1 == 1 { 255 } else { 0 };
                for x in BIT_LEFT + bit * BIT_WIDTH..BIT_LEFT + (bit + 1) * BIT_WIDTH {
                    put(x, y, [value; 3]);
                }
            }
        }
        bgra
    }

    #[test]
    fn a_rendered_picture_is_recognised_with_its_number() {
        for frame in [1, 2, 39, 40, 1000, 65535] {
            let bgra = scene(frame, None);
            assert_eq!(
                judge(&bgra, WIDTH * 4, WIDTH, 64),
                Verdict::Rendered(frame),
                "frame {frame}"
            );
        }
    }

    #[test]
    fn a_dimmed_display_is_read_the_same() {
        // The 780M's HDR-mode panel showed 255 as 141 and 77 as 42.
        for frame in [1, 2, 39, 1000] {
            let bgra = scene_at(frame, None, 141. / 255.);
            assert_eq!(
                judge(&bgra, WIDTH * 4, WIDTH, 64),
                Verdict::Rendered(frame),
                "frame {frame}"
            );
        }
        let between = scene_at(7, Some(7 * STEP - STEP / 2), 141. / 255.);
        assert_eq!(judge(&between, WIDTH * 4, WIDTH, 64), Verdict::Generated);
    }

    #[test]
    fn a_bar_between_two_frames_is_generated() {
        let bgra = scene(7, Some(7 * STEP - STEP / 2));
        assert_eq!(judge(&bgra, WIDTH * 4, WIDTH, 64), Verdict::Generated);
    }

    #[test]
    fn a_blended_bar_is_generated() {
        let mut bgra = scene(7, None);
        // The previous bar position at half intensity, as a blend would leave it.
        for x in 6 * STEP as usize..6 * STEP as usize + BAR as usize {
            let at = BAR_ROW * WIDTH * 4 + x * 4;
            if bgra[at] < 100 {
                bgra[at..at + 3].copy_from_slice(&[150, 128, 128]);
            }
        }
        assert_eq!(judge(&bgra, WIDTH * 4, WIDTH, 64), Verdict::Generated);
    }

    #[test]
    fn another_window_is_unreadable() {
        let bgra = vec![200u8; WIDTH * 64 * 4];
        assert_eq!(judge(&bgra, WIDTH * 4, WIDTH, 64), Verdict::Unreadable);
        let black = vec![0u8; WIDTH * 64 * 4];
        assert_eq!(judge(&black, WIDTH * 4, WIDTH, 64), Verdict::Unreadable);
    }
}
