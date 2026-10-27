//! Plain-language explanations for the failures the host log records.
//!
//! Help & diagnostics scans the tail of `butterpollo.log` with [`RULES`].
//! It is local text matching: no network and no model. Rules match the
//! host's own English messages and the numeric Windows, DXGI and AMF codes,
//! never the operating system's translated error text, so they hold on any
//! Windows language. Only `WARN` and `ERROR` lines are read, and each line
//! counts for the first rule it matches.

use serde::Serialize;

/// Where the fix for each finding is written up.
pub const TROUBLESHOOTING: &str =
    "https://github.com/RamazanKara/Rubylight/blob/main/docs/troubleshooting.md";
/// The longest log line a finding quotes.
const SAMPLE_CHARS: usize = 400;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Encoder,
    Capture,
    Network,
    Driver,
}

pub struct Rule {
    pub id: &'static str,
    pub category: Category,
    /// Every one of these must appear in the line.
    pub all: &'static [&'static str],
    /// At least one of these must appear; empty accepts any line.
    pub any: &'static [&'static str],
    /// A numeric `key=value` field the line must carry at this value or more.
    pub at_least: Option<(&'static str, u64)>,
    pub title: &'static str,
    /// What happened, for someone who has not read the code.
    pub explanation: &'static str,
    /// What to do about it.
    pub fix: &'static str,
    /// The heading anchor in docs/troubleshooting.md.
    pub anchor: &'static str,
}

const GPU_RESET: &str = "the-picture-freezes-and-the-gpu-resets-during-a-stream";
const NO_AMD: &str = "setup-or-the-host-says-this-pc-cannot-run-rubylight-30";
const CODEC: &str = "the-client-asks-for-a-codec-this-gpu-cannot-encode";
const BLACK: &str = "black-picture-no-display-or-wgc-fails";
const FREEZE: &str = "the-picture-freezes-while-audio-and-input-continue";
const NETWORK: &str = "the-stream-breaks-up-drops-or-times-out-on-the-network";
const PORT: &str = "the-console-will-not-open-or-a-port-is-occupied";
const OUTSIDE: &str = "clients-outside-your-network-cannot-connect";
const SEND: &str = "the-stream-disconnects-after-a-second-with-os-error-10035";
const DRIVER: &str = "a-driver-is-missing-or-out-of-date";
const MODE: &str = "windows-refuses-the-virtual-displays-requested-mode";
const DEADLINE: &str = "virtual-display-did-not-become-active-before-the-deadline";

/// Specific rules come before the general ones that would also match.
pub const RULES: &[Rule] = &[
    Rule {
        id: "encoder_hang",
        category: Category::Encoder,
        // Only AMF's own calls: a conversion shader also reports here, and a
        // PC that slept during one reports hours.
        all: &["slow encoder call", "call=\"AMF "],
        any: &[],
        at_least: Some(("elapsed_ms", 2000)),
        title: "The AMD encoder hung",
        explanation: "A call into AMD's video encoder took more than two seconds. The encoder hung inside the AMD driver, and Windows may reset the GPU after it.",
        fix: "If the Radeon shows Code 31 in Device Manager, restart Windows. Send AMD the report its Bug Report Tool offers, and keep the bitrate within what the network carries.",
        anchor: GPU_RESET,
    },
    Rule {
        id: "encoder_stall",
        category: Category::Encoder,
        all: &["stream thread stopped making progress", "phase=\"encoding\""],
        any: &[],
        // Half-second stalls are a busy GPU; an encoder hang lasts seconds.
        at_least: Some(("stalled_ms", 2000)),
        title: "Encoding stopped making progress",
        explanation: "The stream's encoder thread waited on the AMD driver for seconds. This is how an encoder hang starts.",
        fix: "If the picture froze and the GPU reset, restart Windows when the Radeon shows Code 31, and report the driver version and the time to AMD.",
        anchor: GPU_RESET,
    },
    Rule {
        id: "gpu_reset",
        category: Category::Driver,
        all: &[],
        any: &[
            "DXGI_ERROR_DEVICE_RESET",
            "DXGI_ERROR_DEVICE_REMOVED",
            "(0x887A0005)",
            "(0x887A0007)",
            "The GPU did not recover",
        ],
        at_least: None,
        title: "The GPU was reset",
        explanation: "Windows reset or removed the graphics adapter during a stream, usually after the driver's video encoder hung. Capture and encoding restart, and the picture freezes meanwhile.",
        fix: "Restart Windows if Device Manager shows the Radeon with Code 31. Try AMD's current Recommended driver if it happens again.",
        anchor: GPU_RESET,
    },
    Rule {
        id: "codec_unsupported",
        category: Category::Encoder,
        all: &[],
        any: &[
            "AMF_CODEC_NOT_SUPPORTED",
            "AMF_ENCODER_NOT_PRESENT",
            "no encoder for the request",
            "H.264 does not support HDR10",
        ],
        at_least: None,
        title: "This GPU cannot encode the requested codec",
        explanation: "The device asked for a codec, HDR or a size this Radeon's encoder does not have, for example AV1 on a Radeon whose encoder has no AV1.",
        fix: "Choose HEVC or H.264 in the client's settings, or turn HDR off for H.264. Home, Host readiness lists the codecs this GPU encodes.",
        anchor: CODEC,
    },
    Rule {
        id: "amd_driver_old",
        category: Category::Driver,
        all: &[],
        any: &[
            "AMFInit refused this host's AMF version",
            "amfrt64.dll is missing",
            "AMF setting unavailable",
            "AMF LTR unavailable",
            "DuplicateOutput1 unavailable",
        ],
        at_least: None,
        title: "The graphics driver lacks a feature",
        explanation: "The AMD driver refused a feature this host asks for, or its video encoder is missing. The stream runs with the driver's defaults, which can change quality or recovery.",
        fix: "Install the current AMD Radeon driver (the full Adrenalin package, not only Windows Update's driver) and restart Windows if it asks.",
        anchor: DRIVER,
    },
    Rule {
        id: "no_display",
        category: Category::Capture,
        all: &[],
        any: &[
            "requires an interactive desktop display and a hardware GPU",
            "no desktop display or hardware GPU",
        ],
        at_least: None,
        title: "No display to capture",
        explanation: "The host found no active desktop display on a hardware GPU: every monitor was off or asleep, Windows was at the sign-in screen, or only Remote Desktop's display was there.",
        fix: "Turn a monitor on or set up a virtual display in Settings, Display, sign in at the PC, then reconnect.",
        anchor: BLACK,
    },
    Rule {
        id: "encoder_start_failed",
        category: Category::Encoder,
        all: &[],
        any: &[
            "encoder capability initialization failed",
            "No video encoder available",
            "AMF could not start",
        ],
        at_least: None,
        title: "The AMD encoder did not start",
        explanation: "AMF, the encoder in AMD's driver, could not start. Common causes are Windows at the sign-in screen or a disconnected Remote Desktop session, a GPU reset, or a missing or broken driver.",
        fix: "Sign in at the PC and unlock it, then reconnect. If it keeps failing, update the AMD Radeon driver.",
        anchor: NO_AMD,
    },
    Rule {
        id: "encoder_stream_failure",
        category: Category::Encoder,
        all: &["AMF error"],
        any: &["session failed", "Stream ended"],
        at_least: None,
        title: "A stream ended on an encoder error",
        explanation: "AMD's encoder returned an error during a stream, and the stream ended.",
        fix: "Reconnect. If it repeats, note the time and the AMF error, and update the AMD Radeon driver.",
        anchor: NO_AMD,
    },
    Rule {
        id: "capture_access_lost",
        category: Category::Capture,
        all: &[],
        any: &["DXGI_ERROR_ACCESS_LOST", "(0x887A0026)"],
        at_least: None,
        title: "Windows took the screen away from capture",
        explanation: "Desktop Duplication lost the screen: a display mode change, a fullscreen switch, a UAC prompt or the lock screen. Capture reopens, with a short freeze.",
        fix: "Nothing, if it happens now and then. If it repeats during play, try WGC capture in Settings, Video, Capture method.",
        anchor: BLACK,
    },
    Rule {
        id: "protected_content",
        category: Category::Capture,
        all: &[],
        any: &["DXGI_ERROR_CANNOT_PROTECT_CONTENT", "(0x887A002A)"],
        at_least: None,
        title: "Protected video blocked capture",
        explanation: "Windows refuses to capture copy-protected video, such as some streaming apps and players. It streams as black.",
        fix: "Play protected video on the client device itself, or turn off hardware acceleration in that app or browser.",
        anchor: BLACK,
    },
    Rule {
        id: "capture_no_frame",
        category: Category::Capture,
        all: &[],
        any: &[
            "capture produced no GPU frame",
            "Desktop Duplication fallback also failed",
        ],
        at_least: None,
        title: "Capture produced no picture",
        explanation: "The selected display gave the host no picture, so the stream ended. The display may be off, asleep, or not the one with the desktop.",
        fix: "Check that the selected display is on and shows the desktop, in Settings, Display, then reconnect.",
        anchor: BLACK,
    },
    Rule {
        id: "wgc_unavailable",
        category: Category::Capture,
        all: &[],
        any: &[
            "WGC unavailable",
            "Windows Graphics Capture unavailable",
            "WGC helper stopped responding",
        ],
        at_least: None,
        title: "Windows Graphics Capture was unavailable",
        explanation: "WGC needs a signed-in, unlocked Windows desktop. The host captured with Desktop Duplication instead, where timing and HDR can differ.",
        fix: "Keep a Windows user signed in, unlock the desktop or dismiss the UAC prompt, then reconnect.",
        anchor: BLACK,
    },
    Rule {
        id: "capture_stall",
        category: Category::Capture,
        all: &["stream thread stopped making progress", "thread=\"capture\""],
        any: &[],
        at_least: None,
        title: "Capture stalled",
        explanation: "Capture stopped delivering pictures for a while. The picture froze while audio and input went on.",
        fix: "Compare WGC against Desktop Duplication in Settings, Video, Capture method, one reconnect each, and keep the log of the freeze.",
        anchor: FREEZE,
    },
    Rule {
        id: "port_in_use",
        category: Category::Network,
        all: &[],
        any: &[
            "(os error 10048)",
            "(os error 10013)",
            "Another program may be using the port",
        ],
        at_least: None,
        title: "A network port is taken",
        explanation: "The host could not open one of its ports: another streaming host or a second Rubylight uses it, or Windows reserved it.",
        fix: "Close Sunshine, Apollo, Vibepollo or another Rubylight, or choose another base port in Settings, Network.",
        anchor: PORT,
    },
    Rule {
        id: "upnp_failed",
        category: Category::Network,
        all: &["UPnP"],
        any: &["mapping failed", "mapping timed out", "already owned", "could not be renewed"],
        at_least: None,
        title: "The router did not forward the ports",
        explanation: "UPnP is on, but the router refused or did not answer the port forwarding request. Devices outside your network cannot reach this PC.",
        fix: "Turn on UPnP in the router, remove old mappings for these ports there, or forward the ports by hand.",
        anchor: OUTSIDE,
    },
    Rule {
        id: "client_timeout",
        category: Category::Network,
        all: &[],
        any: &[
            "client control stream timed out",
            "client audio ping timed out",
        ],
        at_least: None,
        title: "A device stopped answering",
        explanation: "The device stopped sending on the stream's UDP ports: it lost Wi-Fi, went to sleep, closed the app, or a firewall dropped the traffic.",
        fix: "Keep the device on the same network and awake, check that Windows Firewall allows Rubylight on private networks, then reconnect.",
        anchor: NETWORK,
    },
    Rule {
        id: "send_failed",
        category: Category::Network,
        all: &[],
        any: &[
            "UDP send failed",
            "after transient socket send failures",
            "(os error 10035)",
        ],
        at_least: None,
        title: "The network adapter dropped packets",
        explanation: "Windows' send buffer was full, so the host dropped packets instead of waiting. The device can stutter or ask for a new picture.",
        fix: "Lower the bitrate in the client, or check the host's network adapter and its driver.",
        anchor: SEND,
    },
    Rule {
        id: "network_loss",
        category: Category::Network,
        all: &[],
        any: &[
            "lost on the way to this device",
            "could not carry",
            "cannot carry the stream",
            "are not arriving; pacing video",
            "first keyframe was lost",
        ],
        at_least: None,
        title: "Video was lost on the network",
        explanation: "Video packets did not reach the device, usually on Wi-Fi or a busy link. The host paced or lowered the video to get it through.",
        fix: "Lower the bitrate, move closer to the access point or use 5 GHz, or connect the device by cable.",
        anchor: NETWORK,
    },
    Rule {
        id: "roaming",
        category: Category::Network,
        all: &["roaming reply not sent"],
        any: &[],
        at_least: None,
        title: "A device changed networks",
        explanation: "The device moved to another address during the stream, and the host could not answer it at the new one.",
        fix: "If the stream stopped, reconnect once the device is settled on its new network.",
        anchor: NETWORK,
    },
    Rule {
        id: "virtual_display_driver",
        category: Category::Driver,
        all: &[],
        any: &[
            "display driver is not installed",
            "virtual display driver is missing",
            "display driver recovery did not complete",
            "the Rubylight display driver speaks protocol",
        ],
        at_least: None,
        title: "The virtual display driver is missing or out of date",
        explanation: "Rubylight's display driver is not installed, waits for a restart, or is older than this host. Streams cannot use a virtual display.",
        fix: "Restart Windows if setup asked for it, or run Rubylight's setup again to reinstall the driver.",
        anchor: DRIVER,
    },
    Rule {
        id: "virtual_display_mode",
        category: Category::Driver,
        all: &[],
        any: &[
            "virtual display mode not applied",
            "Windows refused the requested display mode",
        ],
        at_least: None,
        title: "Windows refused the display mode",
        explanation: "Windows did not apply the resolution or refresh rate the device asked for, so the stream uses another mode and scales.",
        fix: "Pick a resolution and refresh rate the display offers, or a lower refresh rate, in the client.",
        anchor: MODE,
    },
    Rule {
        id: "virtual_display_deadline",
        category: Category::Driver,
        all: &["did not become active before the deadline"],
        any: &[],
        at_least: None,
        title: "The virtual display did not switch on",
        explanation: "Windows left the new virtual display off, often because it recalled a saved layout for the other displays.",
        fix: "Update Rubylight; current versions switch the display on themselves.",
        anchor: DEADLINE,
    },
];

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Finding {
    pub id: &'static str,
    pub category: Category,
    /// `error` when any matching line was an error, `warn` otherwise.
    pub level: &'static str,
    pub title: &'static str,
    pub explanation: &'static str,
    pub fix: &'static str,
    pub link: String,
    pub count: u64,
    pub first_seen: String,
    pub last_seen: String,
    /// The latest matching line, without its time and level.
    pub sample: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Analysis {
    /// Newest first.
    pub findings: Vec<Finding>,
    /// The first and last time in the text that was read.
    pub since: Option<String>,
    pub until: Option<String>,
}

pub fn analyze(text: &str) -> Analysis {
    analyze_with(RULES, text)
}

pub fn analyze_with(rules: &[Rule], text: &str) -> Analysis {
    let mut analysis = Analysis::default();
    let mut found: Vec<Option<Finding>> = (0..rules.len()).map(|_| None).collect();
    for line in text.lines() {
        let Some((time, level, rest)) = parse(line) else {
            continue;
        };
        if analysis.since.is_none() {
            analysis.since = Some(time.to_owned());
        }
        analysis.until = Some(time.to_owned());
        let level = match level {
            "WARN" => "warn",
            "ERROR" => "error",
            _ => continue,
        };
        let Some(index) = rules.iter().position(|rule| matches(rule, rest)) else {
            continue;
        };
        let rule = &rules[index];
        let finding = found[index].get_or_insert_with(|| Finding {
            id: rule.id,
            category: rule.category,
            level,
            title: rule.title,
            explanation: rule.explanation,
            fix: rule.fix,
            link: format!("{TROUBLESHOOTING}#{}", rule.anchor),
            count: 0,
            first_seen: time.to_owned(),
            last_seen: String::new(),
            sample: String::new(),
        });
        finding.count += 1;
        if level == "error" {
            finding.level = level;
        }
        finding.last_seen = time.to_owned();
        finding.sample = rest.chars().take(SAMPLE_CHARS).collect();
    }
    analysis.findings = found.into_iter().flatten().collect();
    // RFC 3339 UTC times sort as text.
    analysis
        .findings
        .sort_by(|a, b| b.last_seen.cmp(&a.last_seen).then(a.id.cmp(b.id)));
    analysis
}

/// The time, level and remainder of a `tracing` line:
/// `2026-10-06T17:52:03.632096Z ERROR butterpollo::stream: session failed ...`.
fn parse(line: &str) -> Option<(&str, &str, &str)> {
    let (time, rest) = line.split_once(' ')?;
    if time.len() < 20 || !time.as_bytes()[0].is_ascii_digit() || time.as_bytes()[10] != b'T' {
        return None;
    }
    let (level, rest) = rest.trim_start().split_once(' ')?;
    Some((time, level, rest))
}

fn matches(rule: &Rule, text: &str) -> bool {
    rule.all.iter().all(|needle| text.contains(needle))
        && (rule.any.is_empty() || rule.any.iter().any(|needle| text.contains(needle)))
        && rule
            .at_least
            .is_none_or(|(key, min)| field(text, key).is_some_and(|value| value >= min))
}

/// The number in a ` key=123` field.
fn field(text: &str, key: &str) -> Option<u64> {
    let start = text.find(&format!(" {key}="))? + key.len() + 2;
    let digits = text[start..]
        .find(|c: char| !c.is_ascii_digit())
        .map_or(&text[start..], |end| &text[start..start + end]);
    digits.parse().ok()
}

#[cfg(test)]
mod tests;
