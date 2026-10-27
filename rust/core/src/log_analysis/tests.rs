use super::*;

const T: &str = "2026-10-10T12:00:47.149150Z";

fn only(line: &str) -> Option<&'static str> {
    let analysis = analyze(line);
    assert!(analysis.findings.len() <= 1, "{line}");
    analysis.findings.first().map(|finding| finding.id)
}

/// One line per rule, in the shape the host writes it. Most are copied
/// from a real host log, with German Windows error text where it had one.
const LINES: &[(&str, &str)] = &[
    (
        "encoder_hang",
        "2026-10-10T12:00:47.149150Z  WARN butterpollo_windows::amf: slow encoder call call=\"AMF SubmitInput\" elapsed_ms=34160",
    ),
    (
        "encoder_stall",
        "2026-10-10T12:00:15.002113Z  WARN butterpollo::stall_watch: stream thread stopped making progress thread=\"session\" stream_id=9f494955-1d87-4f7e-a80e-f0e8c0c0fdeb phase=\"encoding\" stalled_ms=3012 gpu_query_outstanding_ms=2990",
    ),
    (
        "gpu_reset",
        "2026-10-10T12:00:47.150002Z  WARN butterpollo_core::session: GPU device lost: DXGI_ERROR_DEVICE_RESET (0x887A0007). Rebuilding capture and encoder; the picture may freeze for up to 30 seconds.",
    ),
    (
        "gpu_reset",
        "2026-10-10T12:01:17.232444Z  WARN butterpollo_core::session: The GPU did not recover within 30 seconds; the stream has ended. Check Device Manager; if the adapter remains unavailable (Code 31), reboot Windows before reconnecting.",
    ),
    (
        "codec_unsupported",
        "2026-10-12T19:20:01.000000Z ERROR butterpollo::stream: session failed; check the reported encoder, capture or socket error before reconnecting error=AMF error AMF_CODEC_NOT_SUPPORTED (18) (this GPU or driver has no encoder for the request) client=SM-F946B",
    ),
    (
        "amd_driver_old",
        "2026-10-12T19:20:02.000000Z  WARN butterpollo_windows::amf: AMF setting unavailable; retaining driver default. Check the named setting and update the AMD driver if quality or cadence differs property=HevcEnableVBAQ",
    ),
    (
        "no_display",
        "2026-10-22T13:37:42.162737Z  WARN butterpollo_core::session: No video encoder available: no desktop display or hardware GPU. Retrying. code=\"video_encoder\"",
    ),
    (
        "encoder_start_failed",
        "2026-10-06T17:50:11.416050Z  WARN butterpollo::state: encoder capability initialization failed error=AMF error 1 codec=1 hdr=false",
    ),
    (
        "encoder_start_failed",
        "2026-10-06T17:50:12.416050Z  WARN butterpollo_core::session: No video encoder available: AMF error 1. Retrying. code=\"video_encoder\"",
    ),
    (
        "encoder_stream_failure",
        "2026-10-06T17:52:03.632096Z ERROR butterpollo::stream: session failed error=AMF error 1 client=Legacy Moonlight Client",
    ),
    (
        "capture_access_lost",
        "2026-10-03T18:42:40.788653Z  WARN butterpollo::stream: capture restarting error=Der Schlüsselmutex wurde abgebrochen. (0x887A0026) output=\\\\.\\DISPLAY28 backend=\"ddx\"",
    ),
    (
        "protected_content",
        "2026-10-03T18:45:00.000000Z  WARN butterpollo::stream: capture restarting error=Der Inhalt kann nicht geschützt werden. (0x887A002A) output=\\\\.\\DISPLAY1 backend=\"ddx\"",
    ),
    (
        "capture_no_frame",
        "2026-10-08T09:13:44.120000Z ERROR butterpollo::stream: session failed; check the reported encoder, capture or socket error before reconnecting error=capture produced no GPU frame on \\\\.\\DISPLAY3 using wgc within 5 seconds; check that the selected display is on client=tablet",
    ),
    (
        "wgc_unavailable",
        "2026-10-08T09:10:00.000000Z  WARN butterpollo_core::session: Capturing with Desktop Duplication: WGC unavailable (WGC requires the unlocked user desktop). Capture timing and HDR may differ.",
    ),
    (
        "wgc_unavailable",
        "2026-10-08T09:11:00.000000Z  WARN butterpollo::stream: capture restarting error=WGC helper stopped responding output=\\\\.\\DISPLAY3 backend=\"wgc\"",
    ),
    (
        "capture_stall",
        "2026-10-09T21:02:10.551000Z  WARN butterpollo::stall_watch: stream thread stopped making progress thread=\"capture\" stream_id=\\\\.\\DISPLAY28 phase=\"reopening capture\" stalled_ms=2040",
    ),
    (
        "port_in_use",
        "2026-10-02T05:37:03.590000Z ERROR butterpollo: host failed error=bind 0.0.0.0:47989: Normalerweise darf jede Socketadresse (Protokoll, Netzwerkadresse oder Anschluss) nur jeweils einmal verwendet werden. (os error 10048)",
    ),
    (
        "upnp_failed",
        "2026-10-02T05:37:13.000000Z  WARN butterpollo::network: UPnP mapping timed out error=deadline has elapsed port=47984",
    ),
    (
        "upnp_failed",
        "2026-10-02T05:37:14.000000Z  WARN butterpollo::network: UPnP port is already owned by another mapping; remove it on the router so clients outside your network reach this PC port=47989 protocol=Tcp",
    ),
    (
        "client_timeout",
        "2026-10-14T20:31:05.000000Z  WARN butterpollo::stream: client control stream timed out client=SM-F946B",
    ),
    (
        "client_timeout",
        "2026-10-14T20:31:06.000000Z  WARN butterpollo_core::session: Audio stopped (client audio ping timed out); video is still running without sound. Check the playback device and network, then reconnect.",
    ),
    (
        "send_failed",
        "2026-10-07T12:37:06.661614Z  WARN butterpollo_windows::net: UDP send failed; packets dropped and the client may stutter. Lower bitrate and check the network adapter code=10035 dropped=45",
    ),
    (
        "network_loss",
        "2026-10-15T18:00:00.000000Z  WARN butterpollo_core::session: Video frames were lost on the way to this device (a keyframe arrived incomplete), so video is now paced like Wi-Fi.",
    ),
    (
        "network_loss",
        "2026-10-15T18:00:01.000000Z  WARN butterpollo_core::session: The network to this device could not carry 150 Mbps of video (12 % of packets lost). The host lowered the bitrate.",
    ),
    (
        "roaming",
        "2026-10-16T08:00:00.000000Z  WARN butterpollo::stream::roaming: roaming reply not sent error=Ein Socketvorgang bezog sich auf ein nicht verfügbares Netzwerk. (os error 10051) to=192.168.4.31:47999",
    ),
    (
        "virtual_display_driver",
        "2026-10-17T10:00:00.000000Z  WARN butterpollo_windows::service: the virtual display driver is missing; setting it up again",
    ),
    (
        "virtual_display_driver",
        "2026-10-17T10:00:01.000000Z ERROR butterpollo::nvhttp: Moonlight session launch failed error=the Rubylight display driver speaks protocol 3, this host 4; reinstall Rubylight app_id=1 resume=false",
    ),
    (
        "virtual_display_mode",
        "2026-10-17T10:00:02.000000Z  WARN butterpollo_windows::display::virtual_display: virtual display mode not applied requested=3840x2160@240 actual=3840x2160@120 error=Windows refused the requested display mode (-2)",
    ),
    (
        "virtual_display_deadline",
        "2026-10-17T10:00:03.000000Z  WARN butterpollo::nvhttp: Moonlight session launch failed error=Virtual display did not become active before the deadline app_id=1 resume=false",
    ),
];

#[test]
fn every_rule_explains_a_real_looking_line() {
    for (id, line) in LINES {
        assert_eq!(only(line), Some(*id), "{line}");
    }
    for rule in RULES {
        assert!(
            LINES.iter().any(|(id, _)| *id == rule.id),
            "{} has no test line",
            rule.id
        );
    }
}

#[test]
fn rule_ids_are_unique_and_texts_are_filled_in() {
    let mut ids = std::collections::BTreeSet::new();
    for rule in RULES {
        assert!(ids.insert(rule.id), "{} twice", rule.id);
        assert!(
            !rule.all.is_empty() || !rule.any.is_empty(),
            "{} matches every line",
            rule.id
        );
        for text in [rule.title, rule.explanation, rule.fix, rule.anchor] {
            assert!(!text.trim().is_empty(), "{}", rule.id);
        }
    }
}

/// GitHub's heading anchors: lower case, spaces to hyphens, other punctuation dropped.
fn anchor(heading: &str) -> String {
    heading
        .trim()
        .to_lowercase()
        .chars()
        .filter_map(|c| match c {
            ' ' => Some('-'),
            c if c.is_alphanumeric() || c == '-' || c == '_' => Some(c),
            _ => None,
        })
        .collect()
}

#[test]
fn every_fix_link_names_a_heading_in_the_troubleshooting_guide() {
    let guide = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/troubleshooting.md"),
    )
    .unwrap();
    let anchors: Vec<_> = guide
        .lines()
        .filter_map(|line| line.strip_prefix("## "))
        .map(anchor)
        .collect();
    assert!(anchors.contains(&"black-picture-no-display-or-wgc-fails".to_owned()));
    for rule in RULES {
        assert!(
            anchors.iter().any(|a| a == rule.anchor),
            "{}: no heading for #{}",
            rule.id,
            rule.anchor
        );
    }
}

#[test]
fn ordinary_and_unrelated_lines_find_nothing() {
    for line in [
        // Too short to be the hang.
        "2026-10-10T14:37:07.355876Z  WARN butterpollo_windows::amf: slow encoder call call=\"compute conversion\" elapsed_ms=63",
        // A conversion shader is no AMF call, and a PC that slept reports hours.
        "2026-10-21T00:48:41.605829Z  WARN butterpollo_windows::amf: slow encoder call call=\"compute conversion\" elapsed_ms=6860818",
        // A half-second encoding stall is a busy GPU, not a hang.
        "2026-10-26T23:32:21.229472Z  WARN butterpollo::stall_watch: stream thread stopped making progress thread=\"session\" stream_id=9f986184 phase=\"encoding\" stalled_ms=509 gpu_query_last_ms=1",
        // A matching message at INFO is not a failure.
        "2026-10-10T14:37:07.355876Z  INFO butterpollo::stream: client control stream timed out client=x",
        "2026-10-02T05:37:03.588446Z  INFO butterpollo::tls: HTTP listener ready address=0.0.0.0:47989 tls=false",
        "2026-10-07T12:37:06.661614Z  WARN butterpollo_windows::limiter: frame limiter has no available provider",
        // Not a log line.
        "  UDP send failed continued on a wrapped line",
        "",
        "WARN",
    ] {
        assert_eq!(only(line), None, "{line}");
    }
}

#[test]
fn repeats_count_once_with_the_first_and_last_time() {
    let text = [
        "2026-10-03T18:42:40.788653Z  WARN butterpollo::stream: capture restarting error=x (0x887A0026) output=a",
        "2026-10-03T18:42:41.000000Z  INFO butterpollo::stream: capture backend opened",
        "2026-10-03T21:10:56.173336Z  WARN butterpollo::stream: capture restarting error=y (0x887A0026) output=b",
        "2026-10-06T17:52:03.632096Z ERROR butterpollo::stream: session failed error=AMF error 1 client=c",
        "2026-10-06T17:52:04.000000Z  WARN butterpollo_core::session: Stream ended: AMF error 1 code=\"stream_failure\"",
    ]
    .join("\n");
    let analysis = analyze(&text);
    assert_eq!(
        analysis.since.as_deref(),
        Some("2026-10-03T18:42:40.788653Z")
    );
    assert_eq!(
        analysis.until.as_deref(),
        Some("2026-10-06T17:52:04.000000Z")
    );
    let ids: Vec<_> = analysis.findings.iter().map(|f| f.id).collect();
    assert_eq!(
        ids,
        ["encoder_stream_failure", "capture_access_lost"],
        "newest first"
    );
    let encoder = &analysis.findings[0];
    assert_eq!((encoder.count, encoder.level), (2, "error"));
    let capture = &analysis.findings[1];
    assert_eq!(capture.count, 2);
    assert_eq!(capture.level, "warn");
    assert_eq!(capture.first_seen, "2026-10-03T18:42:40.788653Z");
    assert_eq!(capture.last_seen, "2026-10-03T21:10:56.173336Z");
    assert_eq!(
        capture.sample,
        "butterpollo::stream: capture restarting error=y (0x887A0026) output=b"
    );
    assert_eq!(
        capture.link,
        format!("{TROUBLESHOOTING}#black-picture-no-display-or-wgc-fails")
    );
}

#[test]
fn a_line_counts_for_its_first_rule_only() {
    // A codec error is also an encoder stream failure; the specific rule wins.
    let line = format!(
        "{T} ERROR butterpollo::stream: session failed error=AMF error AMF_ENCODER_NOT_PRESENT (24) client=c"
    );
    assert_eq!(only(&line), Some("codec_unsupported"));
    // The hang rule needs the field as a number at the threshold.
    let at = format!(
        "{T}  WARN butterpollo_windows::amf: slow encoder call call=\"AMF Init\" elapsed_ms=2000"
    );
    assert_eq!(only(&at), Some("encoder_hang"));
    let text = format!(
        "{T}  WARN butterpollo_windows::amf: slow encoder call call=\"AMF Init\" elapsed_ms=fast"
    );
    assert_eq!(only(&text), None);
}

#[test]
fn long_lines_are_quoted_shortened_on_a_character_boundary() {
    let line = format!("{T}  WARN x: UDP send failed {}", "ü".repeat(1000));
    let finding = &analyze(&line).findings[0];
    assert_eq!(finding.sample.chars().count(), SAMPLE_CHARS);
}
