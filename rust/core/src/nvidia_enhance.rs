//! When the NVIDIA build's image enhancements apply, and with which values.
//!
//! RTX HDR (NVIDIA TrueHDR) turns an SDR game into HDR for a device that asks
//! for HDR. RTX Video enhancement runs NVIDIA's video super resolution at the
//! capture's own size, cleaning up and sharpening SDR and HDR pictures before
//! encoding. Both need Rubylight's separate NVIDIA build, an NVIDIA GPU and a
//! driver whose runtime reports the feature; this module only decides.
//!
//! The per-app enhancement design (an app setting, the stream's own choice
//! from the device, RTX HDR first when both are on, HDR pictures kept HDR by
//! enhancing an SDR proxy and adding back only the bounded change) follows
//! Foundation Sunshine's NVIDIA image enhancements (GPL-3.0).
use crate::config::Config;
use std::collections::BTreeMap;

pub use rubylight_protocol::nvidia_enhance::{
    ALL, RTX_HDR, RTX_HDR_PARAMETER, RTX_VIDEO, RTX_VIDEO_PARAMETER, SERVERINFO_FIELD,
    launch_choice, parse_offer,
};

/// The RTX HDR settings a device or app may set, under their host names.
pub const RTX_HDR_KEYS: &[&str] = &[
    "rtx_hdr",
    "rtx_hdr_sdr_brightness",
    "rtx_hdr_contrast",
    "rtx_hdr_saturation",
    "rtx_hdr_middle_gray",
];
/// The RTX Video enhancement settings a device or app may set.
pub const RTX_VIDEO_KEYS: &[&str] = &["rtx_video", "rtx_video_intensity"];
/// App settings (the app's own JSON names) and the host setting each sets.
pub const APP_KEYS: &[(&str, &str)] = &[
    ("rtx-hdr", "rtx_hdr"),
    ("rtx-hdr-sdr-brightness", "rtx_hdr_sdr_brightness"),
    ("rtx-hdr-contrast", "rtx_hdr_contrast"),
    ("rtx-hdr-saturation", "rtx_hdr_saturation"),
    ("rtx-hdr-middle-gray", "rtx_hdr_middle_gray"),
    ("rtx-video", "rtx_video"),
    ("rtx-video-intensity", "rtx_video_intensity"),
];
/// Every enhancement setting.
pub fn is_key(key: &str) -> bool {
    RTX_HDR_KEYS.contains(&key) || RTX_VIDEO_KEYS.contains(&key)
}

/// The configuration value marking `key` as chosen by a device, an app or the
/// stream's launch rather than inherited from the host.
pub fn marker(key: &str) -> String {
    format!("__runtime_override_{key}")
}

/// RTX HDR is an app, device or stream opt-in, as in Vibepollo: the host's
/// `rtx_hdr` alone never converts.
pub fn rtx_hdr_enabled(config: &Config) -> bool {
    config.boolean("rtx_hdr", false) && config.boolean(&marker("rtx_hdr"), false)
}

/// The runtime value carrying what this host's GPU and driver offer
/// (`RTX_HDR`, `RTX_VIDEO` bits), set by the host in each stream's
/// configuration; absent in a build without the NVIDIA enhancements.
pub const OFFERED_KEY: &str = "__nvidia_enhancements";

/// What the host offers, from its stream configuration.
pub fn offered(config: &Config) -> u8 {
    u8::try_from(config.integer(OFFERED_KEY, 0).clamp(0, i64::from(ALL))).unwrap_or(0) & ALL
}

/// RTX HDR is chosen and this host offers it: the display stays SDR for an
/// HDR stream and the picture is converted before encoding.
pub fn rtx_hdr_runs(config: &Config) -> bool {
    offered(config) & RTX_HDR != 0 && rtx_hdr_enabled(config)
}

/// RTX Video enhancement: the host's `rtx_video` is every app's default, as
/// Foundation's saved stream default is (off until turned on).
pub fn rtx_video_enabled(config: &Config) -> bool {
    config.boolean("rtx_video", false) && rtx_video_quality(config).is_some()
}

/// The quality level (NVIDIA's VSR level 1 Low to 4 Ultra) for the strength
/// `rtx_video_intensity`, 0 to 100 like Foundation's 0 to 1 intensity
/// (default 100, its 1.0): each quarter one level up. 0 leaves pictures
/// unchanged, so it runs nothing.
pub fn rtx_video_quality(config: &Config) -> Option<u32> {
    quality_for(config.integer("rtx_video_intensity", 100))
}

/// See [`rtx_video_quality`].
pub fn quality_for(intensity: i64) -> Option<u32> {
    match intensity.clamp(0, 100) {
        0 => None,
        value => Some(((value + 24) / 25) as u32),
    }
}

/// NGX's TrueHDR values: contrast and saturation (0 to 200, 100 neutral),
/// middle gray (10 to 100 nits) and peak luminance (400 to 1000). A peak over
/// 1000 nits is reached by scaling the output; middle gray is scaled down by
/// as much, so mid-tones keep their brightness.
pub fn rtx_parameters(config: &Config) -> [u32; 4] {
    let peak = peak_nits(config);
    let scale = (peak as f32 / 1000.).max(1.);
    [
        (config.integer("rtx_hdr_contrast", 0) + 100).clamp(0, 200) as u32,
        (config.integer("rtx_hdr_saturation", 0) + 100).clamp(0, 200) as u32,
        (config.integer("rtx_hdr_middle_gray", 50).clamp(10, 100) as f32 / scale)
            .round()
            .clamp(10., 100.) as u32,
        peak.min(1000),
    ]
}

/// The peak brightness of the device's screen, 400 to 2000 nits.
pub fn peak_nits(config: &Config) -> u32 {
    config.integer("hdr_peak_brightness", 1000).clamp(400, 2000) as u32
}

/// The encoder's SDR white (nits) and linear scale for an RTX HDR picture.
pub fn rtx_luminance(config: &Config) -> (f32, f32) {
    (
        100. + config.integer("rtx_hdr_sdr_brightness", 0).clamp(0, 100) as f32,
        (peak_nits(config) as f32 / 1000.).max(1.),
    )
}

/// Driver profile tuning supplies RTX HDR's values for the game in front,
/// where the app or device did not choose them, as NVIDIA's own overlay
/// does; the desktop always uses the neutral values. It never turns RTX HDR
/// on.
pub fn resolve(
    config: &Config,
    foreground_matches: bool,
    profile: &BTreeMap<String, i64>,
) -> Config {
    let mut result = config.clone();
    if !foreground_matches {
        for (key, value) in [
            ("rtx_hdr_contrast", 0),
            ("rtx_hdr_saturation", 0),
            ("rtx_hdr_middle_gray", 50),
        ] {
            if !config.boolean(&marker(key), false) {
                result.values.insert(key.into(), value.to_string());
            }
        }
    } else {
        for (key, value) in profile {
            if key != "rtx_hdr" && !config.boolean(&marker(key), false) {
                result.values.insert(key.clone(), value.to_string());
            }
        }
    }
    result
}

/// Sets `key` from a device, app or launch, marking it as chosen; `None`
/// (an emptied field) restores the inherited value.
pub fn choose(config: &mut Config, inherited: &Config, key: &str, value: Option<String>) {
    match value {
        Some(value) => {
            config.values.insert(key.into(), value);
            config.values.insert(marker(key), "true".into());
        }
        None => {
            match inherited.values.get(key) {
                Some(value) => config.values.insert(key.into(), value.clone()),
                None => config.values.remove(key),
            };
            config.values.remove(&marker(key));
        }
    }
}

/// The stream's own choices from its launch (`rlRtxHdr`, `rlRtxVideo`),
/// which replace the app's. Only what the host offers counts.
pub fn apply_launch(config: &mut Config, options: &BTreeMap<String, String>, offered: u8) {
    for (bit, parameter, key) in [
        (RTX_HDR, RTX_HDR_PARAMETER, "rtx_hdr"),
        (RTX_VIDEO, RTX_VIDEO_PARAMETER, "rtx_video"),
    ] {
        if offered & bit == 0 {
            continue;
        }
        if let Some(on) = launch_choice(options.get(parameter).map(String::as_str)) {
            config.values.insert(key.into(), on.to_string());
            config.values.insert(marker(key), "true".into());
        }
    }
}

/// What the capture delivers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// 8-bit SDR.
    Sdr,
    /// Linear FP16 scRGB, an HDR desktop.
    ScRgb,
    /// 10-bit PQ, which no enhancement reads.
    Pq10,
}

/// The one enhancement a stream runs before encoding.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Enhancement {
    #[default]
    None,
    RtxHdr,
    RtxVideo,
}

/// Which enhancement runs: RTX HDR when it is on, the device streams HDR and
/// the picture is SDR, as in Foundation where it takes the one pre-encode
/// slot first; otherwise RTX Video enhancement for an SDR or FP16 HDR
/// picture. `offered` is what the GPU and driver reported.
pub fn select(config: &Config, offered: u8, stream_hdr: bool, source: Source) -> Enhancement {
    if offered & RTX_HDR != 0 && rtx_hdr_enabled(config) && stream_hdr && source == Source::Sdr {
        Enhancement::RtxHdr
    } else if offered & RTX_VIDEO != 0
        && rtx_video_enabled(config)
        && matches!(source, Source::Sdr | Source::ScRgb)
    {
        Enhancement::RtxVideo
    } else {
        Enhancement::None
    }
}

/// Why an enhancement the stream asked for does not run, for the session's
/// warning; `None` when everything asked for runs or nothing was asked.
pub fn unavailable(config: &Config, offered: u8, source: Source) -> Option<String> {
    let mut reasons = Vec::new();
    if rtx_hdr_enabled(config) && offered & RTX_HDR == 0 {
        reasons.push("RTX HDR is on, but this PC's NVIDIA GPU or driver does not offer it.");
    }
    if rtx_video_enabled(config) {
        if offered & RTX_VIDEO == 0 {
            reasons.push(
                "RTX Video enhancement is on, but this PC's NVIDIA GPU or driver does not offer it.",
            );
        } else if source == Source::Pq10 {
            reasons.push("RTX Video enhancement is on, but this capture delivers 10-bit HDR, which it cannot read. Turn on the FP16 HDR capture or stream in SDR.");
        }
    }
    (!reasons.is_empty()).then(|| reasons.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(text: &str) -> Config {
        Config::parse(text).unwrap()
    }

    #[test]
    fn rtx_hdr_runs_only_where_the_host_offers_it() {
        let mut c = config(
            "rtx_hdr = true
",
        );
        c.values.insert(marker("rtx_hdr"), "true".into());
        assert!(rtx_hdr_enabled(&c));
        assert!(!rtx_hdr_runs(&c));
        c.values.insert(OFFERED_KEY.into(), RTX_VIDEO.to_string());
        assert!(!rtx_hdr_runs(&c));
        c.values.insert(OFFERED_KEY.into(), ALL.to_string());
        assert!(rtx_hdr_runs(&c));
        c.values.insert(OFFERED_KEY.into(), "999".into());
        assert_eq!(offered(&c), ALL);
    }

    #[test]
    fn rtx_hdr_needs_an_app_device_or_launch_opt_in() {
        let mut c = config("rtx_hdr = true\n");
        assert!(!rtx_hdr_enabled(&c));
        let inherited = c.clone();
        choose(&mut c, &inherited, "rtx_hdr", Some("true".into()));
        assert!(rtx_hdr_enabled(&c));
        choose(&mut c, &inherited, "rtx_hdr", None);
        assert!(!rtx_hdr_enabled(&c));
        assert!(c.boolean("rtx_hdr", false), "the host value is restored");
    }

    #[test]
    fn rtx_video_follows_the_host_default_and_its_intensity() {
        assert!(!rtx_video_enabled(&config("")));
        assert!(rtx_video_enabled(&config("rtx_video = true\n")));
        assert_eq!(rtx_video_quality(&config("")), Some(4));
        assert!(!rtx_video_enabled(&config(
            "rtx_video = true\nrtx_video_intensity = 0\n"
        )));
        for (intensity, level) in [
            (-5, None),
            (0, None),
            (1, Some(1)),
            (25, Some(1)),
            (26, Some(2)),
            (50, Some(2)),
            (51, Some(3)),
            (75, Some(3)),
            (76, Some(4)),
            (100, Some(4)),
            (400, Some(4)),
        ] {
            assert_eq!(quality_for(intensity), level, "{intensity}");
        }
    }

    #[test]
    fn launch_choices_replace_the_app_only_when_offered() {
        let options: BTreeMap<String, String> = [
            (RTX_HDR_PARAMETER.into(), "1".into()),
            (RTX_VIDEO_PARAMETER.into(), "0".into()),
        ]
        .into();
        let mut c = config("rtx_video = true\n");
        apply_launch(&mut c, &options, 0);
        assert!(!rtx_hdr_enabled(&c));
        assert!(rtx_video_enabled(&c));
        apply_launch(&mut c, &options, ALL);
        assert!(rtx_hdr_enabled(&c));
        assert!(!rtx_video_enabled(&c));
        let mut follow = config("rtx_video = true\n");
        apply_launch(&mut follow, &BTreeMap::new(), ALL);
        assert!(rtx_video_enabled(&follow));
    }

    #[test]
    fn rtx_hdr_goes_first_and_each_reads_only_what_it_can() {
        let both = {
            let mut c = config("rtx_video = true\n");
            let inherited = c.clone();
            choose(&mut c, &inherited, "rtx_hdr", Some("true".into()));
            c
        };
        use Enhancement as E;
        use Source as S;
        assert_eq!(select(&both, ALL, true, S::Sdr), E::RtxHdr);
        // An SDR stream, or an HDR desktop, is enhanced instead.
        assert_eq!(select(&both, ALL, false, S::Sdr), E::RtxVideo);
        assert_eq!(select(&both, ALL, true, S::ScRgb), E::RtxVideo);
        assert_eq!(select(&both, ALL, true, S::Pq10), E::None);
        assert_eq!(select(&both, RTX_VIDEO, true, S::Sdr), E::RtxVideo);
        assert_eq!(select(&both, RTX_HDR, false, S::Sdr), E::None);
        assert_eq!(select(&both, 0, true, S::Sdr), E::None);
        assert_eq!(select(&config(""), ALL, true, S::Sdr), E::None);
    }

    #[test]
    fn what_is_asked_for_and_cannot_run_is_said() {
        let video = config("rtx_video = true\n");
        assert_eq!(unavailable(&video, ALL, Source::Sdr), None);
        assert!(
            unavailable(&video, RTX_HDR, Source::Sdr)
                .unwrap()
                .contains("does not offer")
        );
        assert!(
            unavailable(&video, ALL, Source::Pq10)
                .unwrap()
                .contains("10-bit")
        );
        assert_eq!(unavailable(&config(""), 0, Source::Sdr), None);
    }

    #[test]
    fn parameters_scale_middle_gray_with_a_peak_over_1000_nits() {
        assert_eq!(rtx_parameters(&config("")), [100, 100, 50, 1000]);
        assert_eq!(
            rtx_parameters(&config(
                "rtx_hdr_contrast = 14\nrtx_hdr_saturation = -300\nrtx_hdr_middle_gray = 60\nhdr_peak_brightness = 1500\n"
            )),
            [114, 0, 40, 1000]
        );
        assert_eq!(
            rtx_parameters(&config("hdr_peak_brightness = 100\n"))[3],
            400
        );
        assert_eq!(
            rtx_luminance(&config(
                "rtx_hdr_sdr_brightness = 30\nhdr_peak_brightness = 2000\n"
            )),
            (130., 2.)
        );
    }

    #[test]
    fn driver_tuning_cannot_enable_hdr_or_override_explicit_choices() {
        let mut c = config("rtx_hdr = false\nrtx_hdr_contrast = 14\nhdr_peak_brightness = 1500\n");
        c.values.insert(marker("rtx_hdr_contrast"), "true".into());
        let profile = BTreeMap::from([
            ("rtx_hdr".into(), 1),
            ("rtx_hdr_contrast".into(), 27),
            ("rtx_hdr_saturation".into(), 51),
        ]);
        let resolved = resolve(&c, true, &profile);
        assert_eq!(resolved.integer("rtx_hdr_contrast", 0), 14);
        assert_eq!(resolved.integer("rtx_hdr_saturation", 0), 51);
        assert!(!resolved.boolean("rtx_hdr", false));
        let desktop = resolve(&config("rtx_hdr_saturation = 9\n"), false, &profile);
        assert_eq!(desktop.integer("rtx_hdr_saturation", 7), 0);
        assert_eq!(desktop.integer("rtx_hdr_middle_gray", 7), 50);
    }

    #[test]
    fn every_app_key_names_an_enhancement_setting() {
        for (_, key) in APP_KEYS {
            assert!(is_key(key), "{key}");
            assert!(crate::config::override_allowed(key), "{key}");
            assert!(!crate::config::removed(key), "{key}");
        }
    }
}
