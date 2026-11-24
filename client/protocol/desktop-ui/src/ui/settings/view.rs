//! Settings page view state, search matching, and Recommended simple-profile mapping.

use super::*;

/// The settings page's own state between frames.
#[derive(Default)]
pub(in crate::ui) struct SettingsView {
    /// The section shown while not searching; empty for the first.
    pub(in crate::ui) section: &'static str,
    /// The search, as typed.
    pub(in crate::ui) query: String,
    /// The search the rows showed last frame: another one starts its results at the top.
    pub(super) shown_query: String,
    /// The rows the search found last frame, per section.
    pub(in crate::ui) found: [usize; 9],
    /// Each section's fold is open (its "More … settings" chosen), until Nightjar closes.
    pub(in crate::ui) more: [bool; 9],
    /// The list's own fold is open ("More settings" chosen): Profiles and the sections of detail
    /// show under the simple ones. Only the user's own choice sets it, and it is kept in
    /// settings.json (`settings_more_open`) for the next start; a link or a search that shows a
    /// section behind the fold opens it for that visit alone (`rail_open`).
    pub(in crate::ui) rail_more: bool,
    /// The section the main settings last showed ("" before their first visit), and whether a
    /// PC's, game's or profile's settings showed since.
    pub(super) main_section: &'static str,
    pub(super) from_layered: bool,
    /// Another section was opened: its rows start at the top.
    pub(in crate::ui) to_top: bool,
    /// Reset all (or Clear all on a PC's, game's or profile's settings) waits for its confirmation.
    pub(in crate::ui) confirm_reset: bool,
    /// The search field had focus last frame, so Escape leaves it before it leaves the page.
    pub(super) search_focused: bool,
    /// The first row drawn takes focus next frame (the search field was left); with none drawn,
    /// the open section's entry does.
    pub(super) focus_rows: bool,
    /// Left was pressed last frame on this control in the rows, laid out there.
    pub(super) left_from: Option<(Id, Rect)>,
    /// The rows the last frame drew, by id (a setting's first key, an action's name), each with
    /// whether it sits behind its section's fold.
    pub(in crate::ui) drawn: Vec<(&'static str, bool)>,
    /// The PC chosen for the main settings' Connection test and PyroWave frame pace, until
    /// Nightjar closes.
    pub(in crate::ui) test_pc: Option<u64>,
    /// The display's HDR state was read for this visit to the Recommended section.
    pub(in crate::ui) hdr_probed: bool,
}

impl SettingsView {
    /// The page's state at start: the fold as the user last left it.
    pub(in crate::ui) fn remembered(rail_more: bool) -> Self {
        Self { rail_more, ..Self::default() }
    }

    pub(super) fn searching(&self) -> bool {
        !self.query.trim().is_empty()
    }

    pub(super) fn open(&mut self, section: &'static str) {
        if self.section != section || self.searching() {
            self.to_top = true;
        }
        if section == en("Recommended") {
            self.hdr_probed = false;
        }
        self.section = section;
        self.query.clear();
        self.confirm_reset = false;
    }
}

pub(super) fn search_id() -> Id {
    Id::unique("settings-search")
}

/// The search's words in lower case: a row is found when each of them is in its label, its
/// description, its section or its group.
pub(super) fn search_words(query: &str) -> Vec<String> {
    query.split_whitespace().map(str::to_lowercase).collect()
}

pub(super) fn found(words: &[String], texts: &[&str]) -> bool {
    let texts: Vec<String> = texts.iter().map(|t| t.to_lowercase()).collect();
    words.iter().all(|w| texts.iter().any(|t| t.contains(w.as_str())))
}

impl SettingsView {
    /// Whether the main settings' list shows what is behind "More settings": it was chosen, a
    /// search looks everywhere, or the open section is one of them (a link or a result led there).
    pub(in crate::ui) fn rail_open(&self) -> bool {
        self.rail_more || self.searching() || rail_folded(self.section)
    }

    /// "More settings" was chosen. Closing it while a section behind it is open goes back to the
    /// first section, so the list never hides what the rows show.
    pub(super) fn toggle_rail(&mut self) {
        if self.rail_open() {
            self.rail_more = false;
            if rail_folded(self.section) {
                self.open(RAIL_MAIN[0]);
            }
        } else {
            self.rail_more = true;
        }
    }
}

pub(super) fn section_index(name: &str) -> usize {
    SETTINGS_SECTIONS.iter().position(|s| *s == name).unwrap_or(0)
}

pub(super) fn simple_smoothness_name(value: simple::Smoothness) -> &'static str {
    match value {
        simple::Smoothness::Auto => en("Auto (recommended)"),
        simple::Smoothness::Smoothest => en("Smoothest"),
        simple::Smoothness::Balanced => en("Balanced"),
        simple::Smoothness::LowestDelay => en("Lowest delay"),
    }
}

pub(super) fn simple_picture_name(value: simple::Picture) -> &'static str {
    match value {
        simple::Picture::Auto => en("Auto (recommended)"),
        simple::Picture::Best => en("Best"),
        simple::Picture::SaveData => en("Save data"),
    }
}

pub(super) fn simple_hdr_name(value: simple::Hdr) -> &'static str {
    match value {
        simple::Hdr::Auto => en("Auto (recommended)"),
        simple::Hdr::Off => en("Off"),
    }
}

/// What the simple layer measured about this computer and screen (SimpleProfile).
pub(crate) fn simple_caps(display: &Display, hdr_display: bool, vrr: bool) -> simple::Capabilities {
    simple::Capabilities {
        display: simple::Display {
            width: display.width,
            height: display.height,
            refresh_hz: display.refresh_hz.round().max(0.0) as u32,
            hdr: hdr_display,
            vrr,
        },
        // The stream negotiates its codec and HDR with the PC, and its rate is the screen's own.
        decoder: simple::Decoder { h264: true, hevc: true, av1: true, hdr: true },
        host: simple::Host::default(),
    }
}

/// The stream's current settings, as the simple layer compares them. `vrr_offered` gates a saved VRR
/// on a machine whose swap chain cannot tear, as the Variable refresh rate row does.
pub(crate) fn simple_current(s: &Settings, display: &Display, _caps: &simple::Capabilities, vrr_offered: bool)
                             -> simple::Current {
    let (_, _, _, default_kbps) = stream_mode(&Settings { bitrate_kbps: 0, ..s.clone() }, *display);
    let fps = if s.fps == 0 || s.fps == display.refresh_hz.round() as u32 { None } else { Some(s.fps) };
    let pacing = match s.frame_pacing {
        FramePacing::Latency => Some(simple::Pacing::Latency),
        FramePacing::Balanced => Some(simple::Pacing::Balanced),
        FramePacing::Smoothness => Some(simple::Pacing::Smoothest),
        FramePacing::CapFps => None,
    };
    let bitrate_milli = match (s.bitrate_kbps, default_kbps) {
        (0, _) => None,
        (chosen, 0) => Some(chosen),
        (chosen, default) => Some((f64::from(chosen) * 1000.0 / f64::from(default)).round() as u32),
    };
    let codec = match s.codec {
        CodecPreference::Auto => simple::Codec::Auto,
        CodecPreference::H264 => simple::Codec::H264,
        CodecPreference::Hevc => simple::Codec::Hevc,
        CodecPreference::Av1 => simple::Codec::Av1,
        // PyroWave is no codec the simple layer offers, so these settings read as Custom.
        CodecPreference::PyroWave | CodecPreference::PyroWave444 => simple::Codec::H264,
    };
    // The HDR row shows what the stream does. A fresh install stores Auto, but the HDR switch it
    // never set leaves the stream SDR, so the row reads Off through the mapping (simple_hdr's
    // Auto is only a choice once HDR is actually asked for).
    simple::Current {
        native_resolution: s.width == 0 && s.height == 0,
        fps,
        pacing,
        vrr: s.vrr && vrr_offered,
        latency_switches: s.unbatched_input && s.network_priority,
        drop_late_frames: s.drop_late_frames,
        host_frame_times: s.host_frame_times,
        real_vrr: s.present_at_host_times,
        reduce_refresh: false,
        content_rate: false,
        bitrate_milli,
        codec,
        hdr: s.hdr,
        adaptive_bitrate: s.lower_bitrate.busy_controller(),
        adaptive_quality: match s.adaptive_quality {
            crate::config::QualityMode::Off => Some(simple::AdaptiveQuality::Off),
            crate::config::QualityMode::Battery => Some(simple::AdaptiveQuality::SavePower),
            crate::config::QualityMode::Motion | crate::config::QualityMode::Detail => None,
        },
        stored_smoothness: Some(s.simple_smoothness),
        stored_picture: Some(s.simple_picture),
        stored_hdr: Some(s.simple_hdr),
    }
}

/// Writes a simple profile's values into this computer's settings.
pub(crate) fn simple_apply(s: &mut Settings, display: &Display, caps: &simple::Capabilities, choices: simple::Choices) {
    let values = simple::values(choices, caps);
    (s.simple_smoothness, s.simple_picture, s.simple_hdr) = (choices.smoothness, choices.picture, choices.hdr);
    // The simple layer streams at this screen's own size and refresh rate.
    (s.width, s.height) = (0, 0);
    s.fps = values.fps.unwrap_or(0);
    s.frame_pacing = match values.pacing {
        simple::Pacing::Latency => FramePacing::Latency,
        simple::Pacing::Balanced => FramePacing::Balanced,
        simple::Pacing::Smoothest => FramePacing::Smoothness,
    };
    s.drop_late_frames = values.drop_late_frames;
    s.host_frame_times = values.host_frame_times;
    // The Variable refresh rate row reads its three keys together: writing only vrr and the host's
    // timing after "Fastest" would leave that row at Custom while these rows say Auto or Balanced.
    if values.vrr {
        set_vrr_choice(s, VrrChoice::On);
    } else {
        s.vrr = false;
    }
    if let Some(real_vrr) = values.real_vrr {
        s.present_at_host_times = real_vrr;
    }
    if values.latency_switches {
        s.unbatched_input = true;
        s.network_priority = true;
    }
    // Set the codec first: PyroWave's own bitrate is not the conventional codec's, and "Best" would
    // otherwise store hundreds of Mbps before switching to Auto.
    s.codec = match values.codec {
        simple::Codec::Auto => CodecPreference::Auto,
        simple::Codec::H264 => CodecPreference::H264,
        simple::Codec::Hevc => CodecPreference::Hevc,
        simple::Codec::Av1 => CodecPreference::Av1,
    };
    let (_, _, _, default_kbps) = stream_mode(&Settings { bitrate_kbps: 0, ..s.clone() }, *display);
    s.bitrate_kbps = match values.bitrate {
        simple::Bitrate::Default => 0,
        simple::Bitrate::Factor(milli) => (u64::from(default_kbps) * u64::from(milli) / 1000) as u32,
    };
    // Save data turns Nightjar's own lowering on, over an earlier Never; leaving it takes back that
    // lowering. Any other choice leaves Lower bitrate as it was; the page does not show that row.
    if values.adaptive_bitrate {
        s.lower_bitrate = LowerBitrate::WhenFramesLost;
    } else if s.lower_bitrate == LowerBitrate::WhenFramesLost {
        s.lower_bitrate = LowerBitrate::Off;
    }
    s.adaptive_quality = match values.adaptive_quality {
        simple::AdaptiveQuality::Off => crate::config::QualityMode::Off,
        simple::AdaptiveQuality::SavePower => crate::config::QualityMode::Battery,
    };
    // A pick in another row leaves an HDR switch the chain cannot turn into HDR as it is; Off and
    // Auto where the chain supports it are the stream's own states (simple_effective_hdr above).
    if values.hdr || choices.hdr == simple::Hdr::Off {
        s.hdr = values.hdr;
    }
}

/// The HDR choice the stream is using, for a pick in another Recommended row: Auto while the HDR
/// switch is on (the stream asks for HDR), Off while it is off. A pick in one row carries this so it
/// never turns HDR back on after the player turned it off under Video.
pub(crate) fn simple_effective_hdr(s: &Settings) -> simple::Hdr {
    if s.hdr { simple::Hdr::Auto } else { simple::Hdr::Off }
}

/// The simple layer's choices from the current settings, or the stored ones while they are Custom.
/// A Custom HDR choice still reads the stream's own switch, so picking another row keeps HDR as it is.
pub(crate) fn simple_state(s: &Settings, display: &Display, caps: &simple::Capabilities, vrr_offered: bool)
                           -> (simple::Choices, bool) {
    let stored = simple::Choices { smoothness: s.simple_smoothness, picture: s.simple_picture, hdr: s.simple_hdr };
    match simple::chosen(&simple_current(s, display, caps, vrr_offered), caps) {
        Some(choices) => (choices, false),
        None => (simple::Choices { hdr: simple_effective_hdr(s), ..stored }, true),
    }
}
