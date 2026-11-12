//! Stream presets, Android's `StreamPreset`: one-tap sets of resolution, frame rate, bitrate, frame
//! pacing, upscaling, VRR and performance settings. The codec, HDR and 4:4:4 choices stay as they
//! are, and a PC's profile still takes priority over them.
//!
//! What both platforms share is here: which values a preset sets and when the settings read as
//! one. Each platform maps them onto its own settings. Resolution, bitrate, VRR and upscaling go
//! back to their defaults: on Android this screen's resolution, the codec's bitrate, VRR off and
//! no upscaling; on desktop Native resolution, Automatic bitrate, VRR where the display has it and
//! bilinear scaling.

/// A stream preset.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Preset {
    /// This screen's resolution at its top refresh rate, with every latency setting on.
    Native,
    /// This screen's resolution at 30 fps, with the panel slowed to match and the GPU left alone.
    BatterySaver,
}

/// Frame pacing as a preset sets it (Android's `frame_pacing` values).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pacing {
    /// The newest picture at each refresh: the lowest latency, and the default.
    Latency,
    /// No more pictures a second than the stream's frame rate.
    CapFps,
}

impl Pacing {
    /// Android's saved `frame_pacing` value.
    pub fn key(self) -> &'static str {
        match self {
            Self::Latency => "latency",
            Self::CapFps => "cap-fps",
        }
    }
}

/// Battery saver's frame rate.
pub const BATTERY_SAVER_FPS: u32 = 30;

/// The power settings only Android has: they make a preset Custom only where they exist.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Power {
    /// Slow the panel to the stream's frame rate (`checkbox_reduce_refresh_rate`).
    pub reduce_refresh: bool,
    /// Ask the decoder for its performance mode (`checkbox_codec_performance`).
    pub codec_performance: bool,
    /// Keep the GPU at its top clocks (`checkbox_gpu_max_clocks`).
    pub gpu_max_clocks: bool,
}

/// The settings a preset is compared with, as the platform reads them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Current {
    /// The resolution is this screen's own.
    pub native_resolution: bool,
    /// The frame rate follows the screen's top refresh rate.
    pub fps_follows_screen: bool,
    /// The fixed frame rate, where it doesn't follow the screen.
    pub fps: u32,
    /// The bitrate is the default for this resolution, frame rate and codec.
    pub default_bitrate: bool,
    /// VRR is at its default, or not offered on this display (then any saved choice reads as
    /// the default, as on Android).
    pub default_vrr: bool,
    /// Upscaling is at its default.
    pub default_upscaling: bool,
    /// The frame pacing, where it is one a preset sets.
    pub pacing: Option<Pacing>,
    /// Every latency switch this platform has is on.
    pub latency_switches_on: bool,
    /// Android's power settings; `None` on a platform without them.
    pub power: Option<Power>,
}

impl Preset {
    pub const ALL: [Self; 2] = [Self::Native, Self::BatterySaver];

    /// Its name, as the chips show it.
    pub fn label(self) -> &'static str {
        match self {
            Self::Native => "Native (recommended)",
            Self::BatterySaver => "Battery saver",
        }
    }

    /// The frame rate it streams at: `None` for the screen's own top refresh rate
    /// ([`stream_frame_rate`]).
    pub fn fps(self) -> Option<u32> {
        match self {
            Self::Native => None,
            Self::BatterySaver => Some(BATTERY_SAVER_FPS),
        }
    }

    pub fn pacing(self) -> Pacing {
        match self {
            Self::Native => Pacing::Latency,
            Self::BatterySaver => Pacing::CapFps,
        }
    }

    /// Whether it turns every latency switch back on. Battery saver leaves them as they are.
    pub fn sets_latency_switches(self) -> bool {
        self == Self::Native
    }

    /// Android's power settings it sets: Battery saver slows the panel, drops the codec's
    /// performance mode and the GPU's top clocks; Native undoes the first two and leaves the
    /// clocks, which are opt-in, alone (`None` there).
    pub fn power(self) -> (bool, bool, Option<bool>) {
        match self {
            Self::Native => (false, true, None),
            Self::BatterySaver => (true, false, Some(false)),
        }
    }

    /// Whether these settings are this preset's.
    pub fn matches(self, current: &Current) -> bool {
        let fps = match self.fps() {
            None => current.fps_follows_screen,
            Some(fps) => !current.fps_follows_screen && current.fps == fps,
        };
        if !current.native_resolution
            || !fps
            || !current.default_bitrate
            || !current.default_vrr
            || !current.default_upscaling
            || current.pacing != Some(self.pacing())
        {
            return false;
        }
        if let Some(power) = current.power {
            let (reduce_refresh, codec_performance, gpu_max_clocks) = self.power();
            if power.reduce_refresh != reduce_refresh
                || power.codec_performance != codec_performance
                || gpu_max_clocks.is_some_and(|on| power.gpu_max_clocks != on)
            {
                return false;
            }
        }
        !self.sets_latency_switches() || current.latency_switches_on
    }
}

/// The preset these settings are, or `None` for Custom.
pub fn chosen(current: &Current) -> Option<Preset> {
    Preset::ALL.into_iter().find(|preset| preset.matches(current))
}

/// The screen's top refresh rate as a stream frame rate, Android's
/// `DisplayFrameRatePolicy.streamFrameRate`: rounded, in the host's 1–1000 Hz launch range.
pub fn stream_frame_rate(panel_hz: f32) -> u32 {
    if panel_hz.is_nan() {
        return 1;
    }
    panel_hz.round().clamp(1.0, 1000.0) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn native() -> Current {
        Current {
            native_resolution: true,
            fps_follows_screen: true,
            fps: 0,
            default_bitrate: true,
            default_vrr: true,
            default_upscaling: true,
            pacing: Some(Pacing::Latency),
            latency_switches_on: true,
            power: None,
        }
    }

    fn battery() -> Current {
        Current { fps_follows_screen: false, fps: 30, pacing: Some(Pacing::CapFps), latency_switches_on: false, ..native() }
    }

    #[test]
    fn the_defaults_are_native() {
        assert_eq!(chosen(&native()), Some(Preset::Native));
        assert_eq!(chosen(&battery()), Some(Preset::BatterySaver));
    }

    #[test]
    fn any_preset_value_changed_reads_as_custom() {
        let changes: [fn(&mut Current); 7] = [
            |c| c.native_resolution = false,
            |c| c.fps_follows_screen = false,
            |c| c.default_bitrate = false,
            |c| c.default_vrr = false,
            |c| c.default_upscaling = false,
            |c| c.pacing = None,
            |c| c.latency_switches_on = false,
        ];
        for change in changes {
            let mut current = native();
            change(&mut current);
            if current.fps_follows_screen {
                assert_eq!(chosen(&current), None, "{current:?}");
            } else {
                // A fixed rate other than 30 is no preset either.
                current.fps = 60;
                assert_eq!(chosen(&current), None, "{current:?}");
            }
        }
    }

    #[test]
    fn battery_saver_leaves_the_latency_switches_alone() {
        assert!(Preset::BatterySaver.matches(&Current { latency_switches_on: true, ..battery() }));
        assert!(!Preset::BatterySaver.matches(&Current { fps: 60, ..battery() }));
        assert!(Preset::Native.sets_latency_switches());
        assert!(!Preset::BatterySaver.sets_latency_switches());
    }

    #[test]
    fn android_power_settings_count_only_where_they_exist() {
        let native_power = Power { reduce_refresh: false, codec_performance: true, gpu_max_clocks: true };
        assert!(Preset::Native.matches(&Current { power: Some(native_power), ..native() }), "GPU clocks are Native's to leave");
        assert!(!Preset::Native.matches(&Current { power: Some(Power { reduce_refresh: true, ..native_power }), ..native() }));
        let saver = Power { reduce_refresh: true, codec_performance: false, gpu_max_clocks: false };
        assert!(Preset::BatterySaver.matches(&Current { power: Some(saver), ..battery() }));
        assert!(!Preset::BatterySaver.matches(&Current { power: Some(Power { gpu_max_clocks: true, ..saver }), ..battery() }));
    }

    #[test]
    fn frame_rates_follow_android() {
        assert_eq!(Preset::Native.fps(), None);
        assert_eq!(Preset::BatterySaver.fps(), Some(30));
        assert_eq!(Preset::Native.pacing().key(), "latency");
        assert_eq!(Preset::BatterySaver.pacing().key(), "cap-fps");
        assert_eq!(stream_frame_rate(119.88), 120);
        assert_eq!(stream_frame_rate(59.94), 60);
        assert_eq!(stream_frame_rate(0.0), 1);
        assert_eq!(stream_frame_rate(5000.0), 1000);
        assert_eq!(stream_frame_rate(f32::NAN), 1);
    }
}
