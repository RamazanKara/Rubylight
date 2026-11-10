//! What an AMD GPU's clocks did during a stream, for the log: a thread reads amdgpu's sysfs files
//! (`pp_dpm_sclk`, `pp_dpm_mclk`, `gpu_busy_percent`) once a second, and the stream's end logs the
//! shader and memory clocks against the fastest level, with the performance level and the
//! platform profile. A decode that takes several times longer than on the same GPU elsewhere can
//! then be told apart from a GPU held at low clocks (a power profile, a TDP limit or bursty work
//! the governor does not clock up for). Linux only; the parsing is tested everywhere. Reading the
//! files needs no rights; inside Flatpak `/sys/class` is visible, and a file that cannot be read is
//! left out of the line.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

const DRM: &str = "/sys/class/drm";
const PLATFORM_PROFILE: &str = "/sys/firmware/acpi/platform_profile";
const PERIOD: Duration = Duration::from_secs(1);
/// About an hour of samples; later ones are not kept.
const MAX_SAMPLES: usize = 3600;
const AMD_VENDOR: &str = "0x1002";

/// One reading of the card's state.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Sample {
    pub shader_mhz: Option<u32>,
    pub memory_mhz: Option<u32>,
    pub busy_percent: Option<u32>,
}

/// The clock of the current level (the line marked `*`) of a `pp_dpm_*` file, whose lines read
/// `1: 2700Mhz *`.
pub fn current_mhz(text: &str) -> Option<u32> {
    text.lines().find(|line| line.trim_end().ends_with('*')).and_then(level_mhz)
}

/// The fastest level of a `pp_dpm_*` file.
pub fn max_mhz(text: &str) -> Option<u32> {
    text.lines().filter_map(level_mhz).max()
}

fn level_mhz(line: &str) -> Option<u32> {
    let (_, rest) = line.split_once(':')?;
    let rest = rest.trim();
    let digits = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
    rest[..digits].parse().ok()
}

/// The amdgpu cards of `drm` (a `/sys/class/drm`), by name: `cardN` entries whose device is AMD's
/// and has the clock levels.
pub fn amd_cards(drm: &Path) -> Vec<(String, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(drm) else { return Vec::new() };
    let mut cards: Vec<(String, PathBuf)> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let numbered = name.strip_prefix("card").is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
            let device = entry.path().join("device");
            let amd = std::fs::read_to_string(device.join("vendor")).is_ok_and(|v| v.trim() == AMD_VENDOR);
            (numbered && amd && device.join("pp_dpm_sclk").exists()).then_some((name, device))
        })
        .collect();
    cards.sort();
    cards
}

/// One reading of `device` (a card's `device` folder).
pub fn sample(device: &Path) -> Sample {
    let read = |file: &str| std::fs::read_to_string(device.join(file)).ok();
    Sample {
        shader_mhz: read("pp_dpm_sclk").as_deref().and_then(current_mhz),
        memory_mhz: read("pp_dpm_mclk").as_deref().and_then(current_mhz),
        busy_percent: read("gpu_busy_percent").and_then(|text| text.trim().parse().ok()),
    }
}

fn percentile(values: &mut [u32], p: usize) -> Option<u32> {
    values.sort_unstable();
    (!values.is_empty()).then(|| values[(values.len() - 1) * p / 100])
}

/// What the card did: shader and memory clock p50 and lowest against the fastest level, busy p50,
/// the performance level (`power_dpm_force_performance_level`) and the platform profile.
pub fn summary(card: &str, samples: &[Sample], shader_max: Option<u32>, memory_max: Option<u32>, level: Option<&str>, profile: Option<&str>) -> String {
    let column = |pick: fn(&Sample) -> Option<u32>| -> Vec<u32> { samples.iter().filter_map(pick).collect() };
    let clock = |name: &str, mut values: Vec<u32>, max: Option<u32>| -> Option<String> {
        let p50 = percentile(&mut values, 50)?;
        let max = max.map(|max| format!(" of {max}")).unwrap_or_default();
        Some(format!("{name} p50 {p50} MHz (lowest {}){max}", values[0]))
    };
    let mut parts: Vec<String> = Vec::new();
    parts.extend(clock("shader clock", column(|s| s.shader_mhz), shader_max));
    parts.extend(clock("memory clock", column(|s| s.memory_mhz), memory_max));
    if let Some(busy) = percentile(&mut column(|s| s.busy_percent), 50) {
        parts.push(format!("busy p50 {busy} %"));
    }
    parts.push(format!("performance level {}", level.unwrap_or("unreadable")));
    parts.push(format!("platform profile {}", profile.unwrap_or("unreadable")));
    format!("amdgpu {card} over {} samples a second apart: {}", samples.len(), parts.join(", "))
}

struct Shared {
    samples: Mutex<Vec<Sample>>,
    stop: AtomicBool,
}

/// Samples the first amdgpu card once a second until dropped.
pub struct Sampler {
    card: String,
    device: PathBuf,
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl Sampler {
    /// The sampler of the first amdgpu card, None without one (or when the thread cannot start).
    pub fn start() -> Option<Self> {
        let (card, device) = amd_cards(Path::new(DRM)).into_iter().next()?;
        let shared = Arc::new(Shared { samples: Mutex::new(Vec::new()), stop: AtomicBool::new(false) });
        let (thread_shared, thread_device) = (shared.clone(), device.clone());
        let thread = std::thread::Builder::new()
            .name("gpu-clocks".into())
            .spawn(move || loop {
                std::thread::park_timeout(PERIOD);
                if thread_shared.stop.load(Ordering::Acquire) { break; }
                let reading = sample(&thread_device);
                let mut samples = thread_shared.samples.lock().unwrap_or_else(|e| e.into_inner());
                if samples.len() < MAX_SAMPLES { samples.push(reading); }
            })
            .ok()?;
        Some(Self { card, device, shared, thread: Some(thread) })
    }

    /// The line for the log, None before the first sample.
    pub fn summary(&self) -> Option<String> {
        let samples = self.shared.samples.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if samples.is_empty() { return None; }
        let read = |path: PathBuf| std::fs::read_to_string(path).ok().map(|text| text.trim().to_owned());
        let shader_max = read(self.device.join("pp_dpm_sclk")).as_deref().and_then(max_mhz);
        let memory_max = read(self.device.join("pp_dpm_mclk")).as_deref().and_then(max_mhz);
        let level = read(self.device.join("power_dpm_force_performance_level"));
        let profile = read(PathBuf::from(PLATFORM_PROFILE));
        Some(summary(&self.card, &samples, shader_max, memory_max, level.as_deref(), profile.as_deref()))
    }
}

impl Drop for Sampler {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            thread.thread().unpark();
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_current_and_fastest_levels_are_read_from_the_dpm_files() {
        let sclk = "0: 400Mhz\n1: 1523Mhz *\n2: 2700Mhz\n";
        assert_eq!(current_mhz(sclk), Some(1523));
        assert_eq!(max_mhz(sclk), Some(2700));
        // Newer kernels list a sleep level first, and some write MHz.
        let with_sleep = "S: 19Mhz *\n0: 800MHz\n1: 2800MHz\n";
        assert_eq!(current_mhz(with_sleep), Some(19));
        assert_eq!(max_mhz(with_sleep), Some(2800));
        assert_eq!(current_mhz("0: 400Mhz\n1: 2700Mhz\n"), None, "no level marked");
        assert_eq!(current_mhz(""), None);
        assert_eq!(max_mhz("garbage"), None);
    }

    #[test]
    fn the_summary_gives_the_median_and_lowest_clock_against_the_fastest_level() {
        let samples = [
            Sample { shader_mhz: Some(800), memory_mhz: Some(2800), busy_percent: Some(99) },
            Sample { shader_mhz: Some(1000), memory_mhz: Some(2800), busy_percent: Some(98) },
            Sample { shader_mhz: Some(900), memory_mhz: None, busy_percent: Some(100) },
        ];
        assert_eq!(
            summary("card1", &samples, Some(2700), Some(2800), Some("auto"), Some("low-power")),
            "amdgpu card1 over 3 samples a second apart: shader clock p50 900 MHz (lowest 800) of 2700, \
             memory clock p50 2800 MHz (lowest 2800) of 2800, busy p50 99 %, performance level auto, platform profile low-power"
        );
        let unread = [Sample::default()];
        assert_eq!(summary("card0", &unread, None, None, None, None),
            "amdgpu card0 over 1 samples a second apart: performance level unreadable, platform profile unreadable");
    }

    #[test]
    fn only_numbered_amd_cards_with_clock_levels_are_sampled() {
        let root = std::env::temp_dir().join(format!("nj-gpu-clocks-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let card = |name: &str, vendor: &str, levels: bool| {
            let device = root.join(name).join("device");
            std::fs::create_dir_all(&device).unwrap();
            std::fs::write(device.join("vendor"), format!("{vendor}\n")).unwrap();
            if levels {
                std::fs::write(device.join("pp_dpm_sclk"), "0: 400Mhz\n1: 2700Mhz *\n").unwrap();
                std::fs::write(device.join("gpu_busy_percent"), "42\n").unwrap();
            }
        };
        card("card1", AMD_VENDOR, true);
        card("card0", "0x8086", true);
        card("card1-DP-1", AMD_VENDOR, true);
        card("card2", AMD_VENDOR, false);
        card("renderD128", AMD_VENDOR, true);
        let cards = amd_cards(&root);
        assert_eq!(cards.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>(), ["card1"]);
        assert_eq!(sample(&cards[0].1), Sample { shader_mhz: Some(2700), memory_mhz: None, busy_percent: Some(42) });
        assert!(amd_cards(&root.join("missing")).is_empty());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_sampler_stops_when_dropped() {
        // No amdgpu card on most test machines: then there is no sampler at all.
        if let Some(sampler) = Sampler::start() {
            drop(sampler);
        }
    }
}
