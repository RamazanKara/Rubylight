//! The performance overlay's text in its three modes, as Android's `PerformanceOverlay`: Compact's
//! one line behind a health mark, Statistics' six lines, and Debug's every measurement, each with
//! the host's hardware readings when it sends them. The labels stay English, as the rest of the
//! overlay.

use nightjar_core::http::{HardwareMetrics, Readings};
use nightjar_policy::overlay::{health, Health};

/// One overlay window's measurements; `None` where nothing was measured.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Measures {
    /// Pictures shown on this screen per second.
    pub rendered_fps: Option<f32>,
    pub received_fps: Option<f32>,
    /// Video payload received, in Mbit/s.
    pub video_mbps: Option<f32>,
    /// Every video and audio datagram received, FEC parity included, in Mbit/s: the stream's
    /// bandwidth on the wire.
    pub wire_mbps: Option<f32>,
    pub loss_percent: Option<f32>,
    pub network_ms: Option<f32>,
    pub variance_ms: Option<f32>,
    /// The video header's processing time, the number Artemis and Moonlight show.
    pub host_ms: Option<f32>,
    /// The host timing reports' present to last packet sent: never in `host_ms`.
    pub host_to_network_ms: Option<f32>,
    pub decode_ms: Option<f32>,
    pub render_ms: Option<f32>,
}

/// The Compact line: the mark's health and the values after it, empty until one is measured.
#[derive(Clone, Debug, PartialEq)]
pub struct Compact {
    pub health: Health,
    pub text: String,
}

impl Default for Compact {
    fn default() -> Self {
        Self { health: Health::Unknown, text: String::new() }
    }
}

/// "119.9 FPS · 8 ms RTT · 0.1% loss": frames shown on this screen, the network round trip and the
/// video frames lost on the way, each once it was measured. The mark judges the shown rate, the
/// only one this device has.
pub fn compact(m: &Measures) -> Compact {
    let mut values = Vec::new();
    if let Some(fps) = m.rendered_fps {
        values.push(format!("{fps:.1} FPS"));
    }
    if let Some(ms) = m.network_ms {
        values.push(format!("{ms:.0} ms RTT"));
    }
    if let Some(loss) = m.loss_percent {
        values.push(format!("{loss:.1}% loss"));
    }
    Compact {
        health: health(m.rendered_fps, m.received_fps, m.network_ms, m.loss_percent, m.decode_ms),
        text: values.join(" · "),
    }
}

/// The mark's colour, as Android's `healthColor`.
pub fn health_rgb(health: Health) -> [u8; 3] {
    match health {
        Health::Good => [0x69, 0xdb, 0x7c],
        Health::Fair => [0xff, 0xc8, 0x57],
        Health::Poor => [0xff, 0x6b, 0x6b],
        Health::Unknown => [0x9a, 0xa0, 0xae],
    }
}

fn value(value: Option<f32>, decimals: usize) -> String {
    value.filter(|v| v.is_finite() && *v >= 0.0).map_or_else(|| "—".into(), |v| format!("{v:.decimals$}"))
}

/// Android's six statistics lines: the stream, frame rates, video payload and loss (and the
/// bandwidth on the wire once measured), the network, the host (and its present to sent once
/// reported, under its own label) and this device.
pub fn statistics(stream: &str, m: &Measures) -> Vec<(String, String)> {
    let mut lines = vec![
        ("Stream".into(), stream.to_owned()),
        ("Frames".into(), format!("{} rendered · {} received FPS", value(m.rendered_fps, 1), value(m.received_fps, 1))),
        ("Video".into(), format!("{} Mbps payload · {}% loss", value(m.video_mbps, 1), value(m.loss_percent, 2))),
    ];
    lines.extend(wire(m));
    lines.push(("Network".into(), format!("{} ms RTT (±{} ms)", value(m.network_ms, 0), value(m.variance_ms, 0))));
    lines.push(("Host processing".into(), format!("{} ms", value(m.host_ms, 1))));
    if m.host_to_network_ms.is_some() {
        lines.push(("Host to network".into(), format!("{} ms", value(m.host_to_network_ms, 1))));
    }
    lines.push(("Client".into(), format!("{} ms decode · {} ms render", value(m.decode_ms, 2), value(m.render_ms, 2))));
    lines
}

/// "On the wire: 53.0 Mbps with FEC and audio", in Statistics and Debug once measured.
pub fn wire(m: &Measures) -> Option<(String, String)> {
    m.wire_mbps.map(|mbps| ("On the wire".into(), format!("{} Mbps with FEC and audio", value(Some(mbps), 1))))
}

/// Video payload in Mbit/s from the bytes received over `secs`.
pub fn video_mbps(bytes: u64, secs: f64) -> Option<f32> {
    (secs > 0.0).then(|| (bytes as f64 * 8.0 / secs / 1_000_000.0) as f32)
}

fn reading(readings: &Readings, key: &str, decimals: usize) -> String {
    value(readings.get(key).map(|v| v as f32), decimals)
}

/// The host's readings, as Android's `withHardwareMetrics`: the CPU and each GPU's usage,
/// temperature and power in Statistics; the sample time and each GPU in full in Debug.
pub fn hardware(metrics: &HardwareMetrics, debug: bool) -> Vec<(String, String)> {
    let mut lines = vec![("Host CPU".into(), format!("{}%", reading(&metrics.cpu, "cpu_usage_percent", 1)))];
    if debug {
        lines.push(("Host sample".into(), format!("{} ms since Unix epoch · Interval: {} ms", metrics.sampled_at_unix_ms, metrics.interval_ms)));
    }
    for gpu in &metrics.gpus {
        let r = &gpu.metrics;
        if !debug {
            lines.push((format!("Host GPU {}", gpu.id), format!("{}% · {} °C · {} W",
                reading(r, "gpu_usage_percent", 1), reading(r, "gpu_temperature_c", 1), reading(r, "gpu_power_w", 1))));
            continue;
        }
        lines.push((format!("GPU {}", gpu.id), if gpu.name.is_empty() { "Unnamed GPU".into() } else { gpu.name.clone() }));
        lines.push(("Usage".into(), format!("{}% · GPU clock: {} MHz · Memory clock: {} MHz",
            reading(r, "gpu_usage_percent", 1), reading(r, "gpu_clock_mhz", 0), reading(r, "gpu_memory_clock_mhz", 0))));
        lines.push(("Temperatures".into(), format!("GPU: {} °C · Hotspot: {} °C · Memory: {} °C",
            reading(r, "gpu_temperature_c", 1), reading(r, "gpu_hotspot_temperature_c", 1), reading(r, "gpu_memory_temperature_c", 1))));
        lines.push(("Voltage".into(), format!("{} mV · Fan: {} RPM ({}%)",
            reading(r, "gpu_voltage_mv", 0), reading(r, "gpu_fan_rpm", 0), reading(r, "gpu_fan_percent", 1))));
        lines.push(("Power".into(), format!("GPU: {} W · Board: {} W", reading(r, "gpu_power_w", 1), reading(r, "gpu_board_power_w", 1))));
        lines.push(("VRAM".into(), format!("{} MB ({}%)", reading(r, "gpu_vram_used_mb", 0), reading(r, "gpu_vram_usage_percent", 1))));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn measured() -> Measures {
        Measures {
            rendered_fps: Some(119.94),
            received_fps: Some(120.0),
            video_mbps: Some(48.24),
            wire_mbps: None,
            loss_percent: Some(0.12),
            network_ms: Some(7.6),
            variance_ms: Some(1.2),
            host_ms: Some(3.24),
            host_to_network_ms: None,
            decode_ms: Some(1.234),
            render_ms: Some(0.5),
        }
    }

    #[test]
    fn compact_shows_what_was_measured_behind_the_health_mark() {
        let line = compact(&measured());
        assert_eq!(line, Compact { health: Health::Good, text: "119.9 FPS · 8 ms RTT · 0.1% loss".into() });
        assert_eq!(compact(&Measures::default()), Compact::default(), "nothing measured: waiting, grey");
        let early = compact(&Measures { network_ms: Some(45.0), ..Default::default() });
        assert_eq!((early.health, early.text.as_str()), (Health::Fair, "45 ms RTT"));
        let lossy = compact(&Measures { loss_percent: Some(6.0), ..measured() });
        assert_eq!(lossy.health, Health::Poor);
        assert_eq!(health_rgb(Health::Fair), [0xff, 0xc8, 0x57]);
        assert_eq!(health_rgb(Health::Unknown), [0x9a, 0xa0, 0xae]);
    }

    #[test]
    fn statistics_are_androids_six_lines_with_host_to_network_apart() {
        let lines = statistics("1920x1080 120 fps, HEVC", &measured());
        let values: Vec<&str> = lines.iter().map(|(_, v)| v.as_str()).collect();
        assert_eq!(values, [
            "1920x1080 120 fps, HEVC",
            "119.9 rendered · 120.0 received FPS",
            "48.2 Mbps payload · 0.12% loss",
            "8 ms RTT (±1 ms)",
            "3.2 ms",
            "1.23 ms decode · 0.50 ms render",
        ]);
        let reported = statistics("s", &Measures { host_to_network_ms: Some(9.5), ..measured() });
        assert_eq!(reported[5], ("Host to network".to_owned(), "9.5 ms".to_owned()));
        assert_eq!(reported[4].0, "Host processing", "the report never replaces Host processing");
        let empty = statistics("s", &Measures::default());
        assert_eq!(empty[3].1, "— ms RTT (±— ms)");
        assert_eq!(empty.len(), 6);
    }

    #[test]
    fn the_bandwidth_on_the_wire_follows_the_video_payload() {
        let lines = statistics("s", &Measures { wire_mbps: Some(53.04), ..measured() });
        assert_eq!(lines[3], ("On the wire".to_owned(), "53.0 Mbps with FEC and audio".to_owned()));
        assert_eq!(lines[4].0, "Network");
        assert_eq!(lines.len(), 7);
    }

    #[test]
    fn video_payload_is_bits_over_the_window() {
        assert_eq!(video_mbps(6_250_000, 1.0), Some(50.0));
        assert_eq!(video_mbps(6_250_000, 2.0), Some(25.0));
        assert_eq!(video_mbps(1, 0.0), None);
    }

    #[test]
    fn hardware_lines_follow_android_in_both_modes() {
        let sample = HardwareMetrics::parse(r#"{"version":1,"sampled_at_unix_ms":1791712800000,"interval_ms":1000,
            "cpu":{"cpu_usage_percent":41.6},
            "gpus":[{"id":0,"name":"","metrics":{"gpu_usage_percent":0,"gpu_temperature_c":61.5,"gpu_clock_mhz":2400}}]}"#).unwrap();
        assert_eq!(hardware(&sample, false), [
            ("Host CPU".to_owned(), "41.6%".to_owned()),
            ("Host GPU 0".to_owned(), "0.0% · 61.5 °C · — W".to_owned()),
        ]);
        let debug = hardware(&sample, true);
        assert_eq!(debug[1].1, "1791712800000 ms since Unix epoch · Interval: 1000 ms");
        assert_eq!(debug[2], ("GPU 0".to_owned(), "Unnamed GPU".to_owned()));
        assert_eq!(debug[3].1, "0.0% · GPU clock: 2400 MHz · Memory clock: — MHz");
        assert_eq!(debug.len(), 8);
    }
}
