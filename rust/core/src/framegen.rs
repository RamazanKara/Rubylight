//! The previous host's frame generation, capture and rational limiter policy.
//! No global config is mutated: each launch resolves its own inherited policy.
use crate::config::Config;
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rate(pub u32);
impl std::fmt::Display for Rate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.0.is_multiple_of(1000) {
            write!(f, "{}", self.0 / 1000)
        } else {
            write!(f, "{}.{:03}", self.0 / 1000, self.0 % 1000)
        }
    }
}
impl Rate {
    pub fn parse(value: &str) -> Result<Self> {
        let value = crate::config::number_text(value);
        let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
        if whole.is_empty()
            || !whole.bytes().all(|c| c.is_ascii_digit())
            || fraction.len() > 3
            || !fraction.bytes().all(|c| c.is_ascii_digit())
        {
            bail!("frame limit must be 0–1000 FPS with at most three decimal places");
        }
        let whole: u32 = whole.parse()?;
        let fraction = if fraction.is_empty() {
            0
        } else {
            fraction.parse::<u32>()? * 10u32.pow(3 - fraction.len() as u32)
        };
        let rate = whole
            .checked_mul(1000)
            .and_then(|r| r.checked_add(fraction))
            .filter(|r| *r <= 1_000_000)
            .ok_or_else(|| anyhow::anyhow!("frame limit is outside 0–1000 FPS"))?;
        Ok(Self(rate))
    }
    pub fn from_client(raw: u32) -> Self {
        Self(if raw < 1000 {
            raw.saturating_mul(1000)
        } else {
            raw
        })
    }
    pub fn rounded(self) -> u32 {
        self.0.saturating_add(500) / 1000
    }
    pub fn rational(self) -> (u32, u32) {
        let (mut a, mut b) = (self.0, 1000);
        while b != 0 {
            (a, b) = (b, a % b);
        }
        (self.0 / a, 1000 / a)
    }
    pub fn period(self) -> std::time::Duration {
        std::time::Duration::from_nanos(1_000_000_000_000 / u64::from(self.0.max(1)))
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Provider {
    None,
    Auto,
    Rtss,
}
impl Provider {
    pub fn parse(value: &str) -> Self {
        match normalize(value).as_str() {
            "none" | "disabled" => Self::None,
            "rtss" => Self::Rtss,
            _ => Self::Auto,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Auto => "auto",
            Self::Rtss => "rtss",
        }
    }
}
/// The configured frame limiting Moonlight extensions report in serverinfo:
/// whether limiting is on by default, whether virtual displays limit
/// automatically, and the manual limit in millihertz.
pub fn advertised(config: &Config, virtual_display_enabled: bool) -> (bool, bool, u32) {
    let virtual_limiter = virtual_refresh(config) != VirtualRefresh::Disabled;
    let manual = config.boolean("frame_limiter_enable", false)
        && Provider::parse(config.get("frame_limiter_provider", "auto")) != Provider::None;
    let limit = Rate::parse(config.get("frame_limiter_fps_limit", "0")).map_or(0, |r| r.0);
    (
        manual || (virtual_display_enabled && virtual_limiter),
        virtual_limiter,
        limit,
    )
}
fn normalize(value: &str) -> String {
    value
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .flat_map(char::to_lowercase)
        .collect()
}
/// How a virtual display's refresh follows the stream
/// (`frame_limiter_auto_virtual_framegen`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VirtualRefresh {
    /// Twice the stream rate.
    Legacy,
    /// Four times the stream rate.
    Enabled,
    /// A fixed 1000 Hz.
    Vrr,
    Disabled,
}
/// The setting with Vibepollo's spellings. The default is legacy rather
/// than Vibepollo's enabled: 2x measured lower latency here.
pub fn virtual_refresh(config: &Config) -> VirtualRefresh {
    let value = config.get("frame_limiter_auto_virtual_framegen", "legacy");
    match normalize(value).as_str() {
        "" | "legacy" | "2x" | "fixed2x" => VirtualRefresh::Legacy,
        "vrr" | "1000hz" | "1000" | "fixed1000hz" => VirtualRefresh::Vrr,
        "false" | "no" | "disable" | "disabled" | "off" | "0" => VirtualRefresh::Disabled,
        "enabled" | "enable" | "true" | "yes" | "on" | "1" | "smooth" | "smoother" => {
            VirtualRefresh::Enabled
        }
        _ => {
            crate::config::invalid("frame_limiter_auto_virtual_framegen", value);
            VirtualRefresh::Legacy
        }
    }
}
pub fn display_rate(rate: Rate, virtual_display: bool, mode: VirtualRefresh) -> Rate {
    match (virtual_display, mode) {
        (true, VirtualRefresh::Legacy) => Rate(rate.0.saturating_mul(2)),
        (true, VirtualRefresh::Enabled) => Rate(rate.0.saturating_mul(4)),
        (true, VirtualRefresh::Vrr) => Rate(1_000_000),
        _ => rate,
    }
}
pub fn generation_provider(value: &str) -> &'static str {
    match normalize(value).as_str() {
        "game" | "gameprovided" | "gameprovider" => "game-provided",
        _ => "none",
    }
}
#[derive(Clone, Debug)]
pub struct Policy {
    pub rate: Rate,
    pub display_rate: Rate,
    pub enabled: bool,
    pub provider: Provider,
    pub sync_limiter: u32,
    pub capture: String,
}
impl Policy {
    pub fn resolve(
        config: &Config,
        stream: Rate,
        virtual_display: bool,
        generation: &str,
        generation_enabled: bool,
        runtime_sync: bool,
    ) -> Result<Self> {
        let generation = generation_provider(generation);
        let framegen = generation_enabled || generation != "none";
        let mode = virtual_refresh(config);
        let automatic = virtual_display && mode != VirtualRefresh::Disabled;
        let provider = Provider::parse(config.get("frame_limiter_provider", "auto"));
        let overridden = Rate::parse(config.get("frame_limiter_fps_limit", "0"))?;
        let rate = if overridden.0 > 0 { overridden } else { stream };
        let allow_sync_override = config.boolean("rtss_allow_virtual_display_override", false);
        let auto_sync = automatic && (!allow_sync_override || generation == "game-provided");
        let sync = if !runtime_sync && (auto_sync || (!virtual_display && framegen)) {
            1
        } else {
            match normalize(config.get("rtss_frame_limit_type", "async")).as_str() {
                "frontedgesync" => 1,
                "backedgesync" => 2,
                _ => 0,
            }
        };
        let requested_capture = match config.get("capture", "").trim().to_ascii_lowercase() {
            // Vibepollo's constant-rate WGC variant.
            wgc if wgc == "wgcc" => "wgc".to_owned(),
            // `idd-push`: frames pushed by Rubylight's own virtual display driver.
            known
                if matches!(
                    known.as_str(),
                    "" | "auto" | "wgc" | "ddx" | "dxgi" | "idd-push"
                ) =>
            {
                known
            }
            other => {
                crate::config::invalid("capture", &other);
                String::new()
            }
        };
        let capture = if requested_capture.is_empty() || requested_capture == "auto" {
            // Resolve Auto before opening capture so the normal WGC path,
            // including the service's user helper and DDX fallback, is used.
            "wgc"
        } else {
            &requested_capture
        }
        .to_owned();
        Ok(Self {
            rate,
            display_rate: display_rate(stream, virtual_display, mode),
            enabled: config.boolean("frame_limiter_enable", false)
                || automatic
                || (!virtual_display && framegen),
            provider,
            sync_limiter: sync,
            capture,
        })
    }
    pub fn with_vrr(mut self, config: &Config, virtual_display: bool, requested: bool) -> Self {
        if virtual_display && requested && virtual_refresh(config) != VirtualRefresh::Disabled {
            self.display_rate = Rate(1_000_000);
            if matches!(config.get("capture", "auto"), "" | "auto") {
                self.capture = "wgc".into();
            }
        }
        self
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn advertised_limiter_flags_follow_manual_provider_virtual_policy_and_fractional_limits() {
        for (text, virtual_display, expected) in [
            ("", false, (false, true, 0)),
            ("", true, (true, true, 0)),
            (
                "frame_limiter_auto_virtual_framegen=disabled",
                true,
                (false, false, 0),
            ),
            (
                "frame_limiter_enable=true\nframe_limiter_provider=none",
                false,
                (false, true, 0),
            ),
            (
                "frame_limiter_enable=true\nframe_limiter_provider=rtss\nframe_limiter_fps_limit=59.94",
                false,
                (true, true, 59940),
            ),
        ] {
            assert_eq!(
                advertised(&Config::parse(text).unwrap(), virtual_display),
                expected,
                "{text}"
            );
        }
    }
    #[test]
    fn automatic_capture_prefers_wgc_on_physical_and_virtual_displays() {
        for text in ["", "capture=", "capture=auto", "capture= Auto "] {
            let config = Config::parse(text).unwrap();
            let original = config.values.clone();
            for virtual_display in [false, true] {
                for generation in ["none", "game-provided"] {
                    for rate in [Rate(59940), Rate(60000), Rate(120000)] {
                        for vrr in [false, true] {
                            let policy = Policy::resolve(
                                &config,
                                rate,
                                virtual_display,
                                generation,
                                false,
                                false,
                            )
                            .unwrap()
                            .with_vrr(&config, virtual_display, vrr);
                            assert_eq!(policy.capture, "wgc", "{text}, {generation}");
                            assert_eq!(policy.rate, rate);
                        }
                    }
                }
            }
            assert_eq!(config.values, original);
        }
    }
    #[test]
    fn explicit_capture_choices_survive_virtual_display_and_framegen_policy() {
        for (setting, expected) in [
            ("ddx", "ddx"),
            (" DDX ", "ddx"),
            ("dxgi", "dxgi"),
            ("DXGI", "dxgi"),
            ("wgc", "wgc"),
            ("WGC", "wgc"),
            ("wgcc", "wgc"),
            // Rubylight's own display driver pushes the frames.
            ("idd-push", "idd-push"),
            (" IDD-Push ", "idd-push"),
        ] {
            let config = Config::parse(&format!("capture={setting}")).unwrap();
            let original = config.values.clone();
            for virtual_display in [false, true] {
                for generation in ["none", "game-provided"] {
                    for vrr in [false, true] {
                        let policy = Policy::resolve(
                            &config,
                            Rate(59940),
                            virtual_display,
                            generation,
                            true,
                            false,
                        )
                        .unwrap()
                        .with_vrr(&config, virtual_display, vrr);
                        assert_eq!(policy.capture, expected, "{setting}, {generation}");
                    }
                }
            }
            assert_eq!(config.values, original);
        }
    }
    #[test]
    fn vrr_virtual_display_uses_fixed_1000_hz_without_changing_stream_rate() {
        let c = Config::default();
        let p = Policy::resolve(&c, Rate(119880), true, "none", false, false)
            .unwrap()
            .with_vrr(&c, true, true);
        assert_eq!(p.display_rate, Rate(1_000_000));
        assert_eq!(p.rate, Rate(119880));
        assert_eq!(p.capture, "wgc");
        let c = Config::parse("frame_limiter_auto_virtual_framegen=disabled\ncapture=ddx").unwrap();
        let p = Policy::resolve(&c, Rate(119880), true, "none", false, false)
            .unwrap()
            .with_vrr(&c, true, true);
        assert_eq!(p.display_rate, Rate(119880));
        assert_eq!(p.capture, "ddx");
        let c = Config::parse("frame_limiter_auto_virtual_framegen=fixed-1000-hz").unwrap();
        let p = Policy::resolve(&c, Rate(59940), false, "none", false, false)
            .unwrap()
            .with_vrr(&c, false, true);
        assert_eq!(p.display_rate, Rate(59940));
    }
    #[test]
    fn a_device_display_mode_keeps_the_streams_limiter_rate() {
        for (setting, limit) in [("", 116_000), ("frame_limiter_fps_limit=58", 58_000)] {
            for (mode, display) in [
                ("legacy", 120_000),
                ("enabled", 240_000),
                ("fixed-1000hz", 1_000_000),
                ("disabled", 60_000),
            ] {
                let c = Config::parse(&format!(
                    "{setting}\nframe_limiter_auto_virtual_framegen={mode}"
                ))
                .unwrap();
                for virtual_display in [false, true] {
                    let mut p =
                        Policy::resolve(&c, Rate(116_000), virtual_display, "none", false, false)
                            .unwrap();
                    p.display_rate =
                        display_rate(Rate(60_000), virtual_display, virtual_refresh(&c));
                    assert_eq!(p.rate, Rate(limit));
                    assert_eq!(
                        p.display_rate,
                        Rate(if virtual_display { display } else { 60_000 })
                    );
                    let p = p.with_vrr(&c, virtual_display, true);
                    assert_eq!(p.rate, Rate(limit));
                    assert_eq!(p.rate.rational(), (limit / 1000, 1));
                    assert_eq!(
                        p.display_rate,
                        Rate(if virtual_display && mode != "disabled" {
                            1_000_000
                        } else {
                            60_000
                        })
                    );
                }
            }
        }
    }
    #[test]
    fn quoted_frame_rates_are_read_like_the_console_shows_them() {
        assert_eq!(Rate::parse("\"59.94\"").unwrap(), Rate(59940));
        assert_eq!(Rate::parse(" \" 120 \" ").unwrap(), Rate(120_000));
        assert!(Rate::parse("\"59.9400\"").is_err());
        let c = Config::parse(
            "frame_limiter_enable=true\nframe_limiter_provider=rtss\nframe_limiter_fps_limit=\"30\"",
        )
        .unwrap();
        assert_eq!(advertised(&c, false), (true, true, 30_000));
        let p = Policy::resolve(&c, Rate(60_000), false, "none", false, false).unwrap();
        assert_eq!(p.rate, Rate(30_000));
        let c = Config::parse("dd_refresh_rate_option=manual\ndd_manual_refresh_rate=\"119.88\"")
            .unwrap();
        assert_eq!(
            c.display_request_rate(1920, 1080, Rate(60_000), false, false)
                .unwrap()
                .refresh,
            Some(119_880)
        );
    }
    #[test]
    fn fractional_rate_round_trip_and_bounds() {
        assert_eq!(Rate::parse("59.940").unwrap().rational(), (2997, 50));
        assert_eq!(Rate::parse("119.880").unwrap().rounded(), 120);
        assert_eq!(Rate(0).rational(), (0, 1));
        for bad in [
            "NaN",
            "-1",
            "1000.001",
            "59.9400",
            "1e2",
            "1.2.3",
            "4294967296",
        ] {
            assert!(Rate::parse(bad).is_err());
        }
    }
    #[test]
    fn inherited_framegen_keeps_explicit_capture_and_sync_contract() {
        let mut c = Config::parse("capture=ddx\nrtss_frame_limit_type=back_edge_sync\nrtss_allow_virtual_display_override=1").unwrap();
        let p = Policy::resolve(&c, Rate(59940), true, "game-provided", false, false).unwrap();
        assert_eq!(p.display_rate, Rate(119880));
        assert_eq!(p.sync_limiter, 1);
        assert_eq!(p.capture, "ddx");
        let p = Policy::resolve(&c, Rate(59940), true, "game-provided", false, true).unwrap();
        assert_eq!(p.sync_limiter, 2);
        c.values.insert(
            "frame_limiter_auto_virtual_framegen".into(),
            "enabled".into(),
        );
        assert_eq!(
            Policy::resolve(&c, Rate(59940), true, "game-provided", false, false)
                .unwrap()
                .display_rate,
            Rate(239760)
        );
    }
    #[test]
    fn virtual_refresh_reads_vibepollo_spellings() {
        for (value, mode) in [
            ("2x", VirtualRefresh::Legacy),
            ("fixed_2x", VirtualRefresh::Legacy),
            ("No", VirtualRefresh::Disabled),
            ("disable", VirtualRefresh::Disabled),
            ("smoother", VirtualRefresh::Enabled),
            ("1000hz", VirtualRefresh::Vrr),
            ("sideways", VirtualRefresh::Legacy),
        ] {
            let c = Config::parse(&format!("frame_limiter_auto_virtual_framegen={value}")).unwrap();
            assert_eq!(virtual_refresh(&c), mode, "{value}");
        }
        assert_eq!(virtual_refresh(&Config::default()), VirtualRefresh::Legacy);
        let c = Config::parse("frame_limiter_auto_virtual_framegen=fixed-2x").unwrap();
        let p = Policy::resolve(&c, Rate(60000), true, "none", false, false).unwrap();
        assert_eq!(p.display_rate, Rate(120000));
    }
}
