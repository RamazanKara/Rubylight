//! Replays recorded stream legs through [`PresentClock`] as a display that refreshes when a frame
//! comes: each frame's ready time (`decoder_output_ns`) and host stamp (`pts_us`) from Nightjar's
//! timing CSV of a leg that showed frames when decoded. Prints how long frames were held and how
//! far the shown intervals are from the stamps' intervals, pooled over the legs.
//!
//!   cargo run --release -p rubylight-protocol --example present_replay -- LEG.csv [LEG.csv ...]

use rubylight_protocol::present_clock::{PresentClock, Settings};

/// (ready µs, host stamp µs) per frame in the order shown, without the first five seconds.
fn load(path: &str) -> Vec<(u64, u64)> {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let mut lines = text.lines();
    let header: Vec<&str> = lines.next().unwrap_or("").split(',').collect();
    let column = |name: &str| header.iter().position(|h| *h == name).unwrap_or_else(|| panic!("{path}: no {name} column"));
    let (ready, stamp) = (column("decoder_output_ns"), column("pts_us"));
    let mut frames: Vec<(u64, u64)> = lines
        .filter_map(|line| {
            let cells: Vec<&str> = line.split(',').collect();
            Some((cells.get(ready)?.parse::<u64>().ok()? / 1000, cells.get(stamp)?.parse().ok()?))
        })
        .collect();
    frames.sort_unstable();
    let start = frames.first().map_or(0, |f| f.0);
    frames.retain(|f| f.0 - start >= 5_000_000);
    frames
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    sorted[((p / 100.0 * (sorted.len() - 1) as f64).round() as usize).min(sorted.len() - 1)]
}

fn main() {
    let (mut holds, mut errors, mut late) = (Vec::new(), Vec::new(), 0u64);
    for path in std::env::args().skip(1) {
        let frames = load(&path);
        if frames.len() < 500 {
            continue;
        }
        let mut clock = PresentClock::new(Settings::adaptive(0));
        let mut last: Option<(u64, u64)> = None;
        for &(ready, stamp) in &frames {
            let shown = clock.schedule((stamp * 9 / 100) as u32, ready);
            holds.push(shown.hold_us as f64 / 1e3);
            if let Some((last_shown, last_stamp)) = last {
                let game = (stamp as f64 - last_stamp as f64) / 1e3;
                if game > 0.0 && game <= 100.0 {
                    errors.push((shown.present_us as f64 - last_shown as f64) / 1e3 - game);
                }
            }
            last = Some((shown.present_us, stamp));
        }
        late += clock.late_frames;
    }
    if holds.is_empty() {
        eprintln!("no leg with 500 frames or more");
        std::process::exit(2);
    }
    let frames = holds.len() as f64;
    let mean = holds.iter().sum::<f64>() / frames;
    holds.sort_by(f64::total_cmp);
    let off = |ms: f64| 100.0 * errors.iter().filter(|e| e.abs() > ms).count() as f64 / errors.len().max(1) as f64;
    let rms = (errors.iter().map(|e| e * e).sum::<f64>() / errors.len().max(1) as f64).sqrt();
    println!(
        "hold mean {mean:.2} p50 {:.2} p95 {:.2} ms | intervals off the stamps' >1 ms {:.1}% >2 ms {:.1}% rms {rms:.2} ms | late {:.1}% | {} frames",
        percentile(&holds, 50.0),
        percentile(&holds, 95.0),
        off(1.0),
        off(2.0),
        100.0 * late as f64 / frames,
        holds.len(),
    );
}
