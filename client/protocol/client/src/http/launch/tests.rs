use super::super::capabilities::{Availability, Reason};
use super::super::serverinfo::{tests::RUBYLIGHT, ServerInfo};
use super::*;

fn rubylight() -> HostCapabilities {
    rubylight_host(true)
}

/// The Rubylight fixture, with or without `DisplayScaleVersion`.
fn rubylight_host(display_scale: bool) -> HostCapabilities {
    // The fixture's virtual display driver is not ready; make it ready.
    let mut xml = RUBYLIGHT.replace(
        "<VirtualDisplayDriverReady>false</VirtualDisplayDriverReady>",
        "<VirtualDisplayDriverReady>true</VirtualDisplayDriverReady>",
    );
    if display_scale {
        xml = xml.replace("</root>", "<DisplayScaleVersion>1</DisplayScaleVersion></root>");
    }
    ServerInfo::parse(&xml, 47989).unwrap().capabilities
}

fn request() -> LaunchRequest {
    LaunchRequest {
        app_id: 881_448_767,
        ri_key: core::array::from_fn(|i| i as u8),
        ri_key_id: -2_147_483_525,
        ..Default::default()
    }
}

const KEY: &str = "000102030405060708090A0B0C0D0E0F";

#[test]
fn plain_sunshine_launch() {
    let caps = HostCapabilities::UNKNOWN;
    assert_eq!(
        request().query(&caps, false),
        format!(
            "appid=881448767&mode=1920x1080x60&additionalStates=1&sops=1&rikey={KEY}&rikeyid=-2147483525\
                 &localAudioPlayMode=0&surroundAudioInfo=196610&remoteControllersBitmap=0&gcmap=0&gcpersist=0&corever=1"
        )
    );
}

#[test]
fn ten_bit_sdr_says_hdr_mode_zero_and_hdr_still_wins() {
    // Without hdrMode=0 a host streams the 10-bit format as HDR (hardware lane 4a: every
    // 10-bit SDR stream to desktop Nightjar ended at once on an SDR display).
    for caps in [HostCapabilities::UNKNOWN, rubylight()] {
        let sdr = LaunchRequest {
            ten_bit_sdr: true,
            ..request()
        };
        assert_eq!(
            sdr.query(&caps, false),
            request()
                .query(&caps, false)
                .replace("&localAudioPlayMode=", "&hdrMode=0&localAudioPlayMode=")
        );
        let hdr = LaunchRequest { hdr: true, ..sdr.clone() };
        let q = hdr.query(&caps, false);
        assert!(q.contains("&hdrMode=1&") && !q.contains("hdrMode=0"), "{q}");
        let input = LaunchRequest {
            role: AppRole::InputOnly,
            ..sdr
        };
        assert!(!input.query(&caps, false).contains("hdrMode"));
    }
    assert!(!request().query(&rubylight(), false).contains("hdrMode"));
}

#[test]
fn the_display_scale_goes_when_chosen_with_or_without_the_virtual_display_switch() {
    let query = |virtual_display, dpi| {
        LaunchRequest { virtual_display, virtual_display_dpi: dpi, ..request() }.query(&rubylight(), false)
    };
    assert!(!query(true, 0).contains("rlDisplayScale"), "0 keeps the PC's own scale");
    assert!(query(true, 200).contains("&scaleFactor=100&rlDisplayScale=200"), "the render resolution stays 100 %");
    // The host makes a virtual display of its own by default (per_client), so the switch
    // being off does not mean a physical display; the host ignores the scale on one.
    let off = query(false, 200);
    assert!(off.contains("&rlDisplayScale=200&") && !off.contains("virtualDisplay"), "{off}");
    // An input-only launch has no display.
    let input_only = LaunchRequest { role: AppRole::InputOnly, virtual_display_dpi: 200, ..request() }.query(&rubylight(), false);
    assert!(!input_only.contains("rlDisplayScale"), "{input_only}");
}

#[test]
fn hosts_without_display_scale_version_get_no_display_scale() {
    // Older Rubylight hosts (2.6.x, 3.0.0-rc.5) ignore rlDisplayScale.
    let request = LaunchRequest { virtual_display: true, virtual_display_dpi: 200, ..request() };
    let older = rubylight_host(false);
    assert_eq!(
        older.availability(Feature::DisplayScale),
        Availability::Unsupported(Some(Reason::UpdateHost))
    );
    let query = request.query(&older, false);
    assert!(query.contains("&virtualDisplay=1&scaleFactor=100&"), "{query}");
    assert!(!query.contains("rlDisplayScale"), "{query}");
    assert!(request.query(&rubylight_host(true), false).contains("&rlDisplayScale=200&"));
}

#[test]
fn rubylight_launch_with_every_extension() {
    let request = LaunchRequest {
        app_uuid: "u 1".into(),
        width: 2560,
        height: 1440,
        fps: 60,
        client_refresh_rate_x100: 5994,
        virtual_display: true,
        virtual_display_scale: 150,
        virtual_display_dpi: 175,
        host_display: HostDisplay::ExtendedPrimary,
        vrr: true,
        hdr: true,
        play_local_audio: true,
        audio: AudioConfiguration::SURROUND_71,
        prepared_controllers: "0:dualsense".into(),
        gamepad_mask: 3,
        persist_gamepads: true,
        ..request()
    };
    assert_eq!(
        request.query(&rubylight(), false),
        format!(
            "appid=881448767&appuuid=u%201&mode=2560x1440x59.94&virtualDisplay=1&scaleFactor=150\
                 &rlDisplayScale=175&hostDisplay=extended_primary&clientVrrRequested=1&vrr=1&additionalStates=1&sops=1\
                 &rikey={KEY}&rikeyid=-2147483525&hdrMode=1&clientHdrCapVersion=0\
                 &clientHdrCapSupportedFlagsInUint32=0&clientHdrCapMetaDataId=NV_STATIC_METADATA_TYPE_1\
                 &clientHdrCapDisplayData=0x0x0x0x0x0x0x0x0x0x0&localAudioPlayMode=1\
                 &surroundAudioInfo=104792072&rlControllers=0:dualsense&remoteControllersBitmap=3&gcmap=3\
                 &gcpersist=1&rlConfirmReplace=0&corever=1"
        )
    );
    assert_eq!(request.stream_frame_rate_millihz(&rubylight(), 60), 59_940);
    assert_eq!(request.stream_frame_rate_millihz(&rubylight(), 30), 30_000);
}

#[test]
fn clipboard_sync_is_asked_only_of_a_rubylight_host() {
    let asked = LaunchRequest {
        clipboard_sync: true,
        ..request()
    };
    assert_eq!(
        asked.query(&rubylight(), false),
        request()
            .query(&rubylight(), false)
            .replace("&corever=1", "&rlClipboardSync=1&corever=1")
    );
    // Other hosts get exactly today's query.
    let caps = HostCapabilities::UNKNOWN;
    assert_eq!(asked.query(&caps, false), request().query(&caps, false));
    assert!(!request()
        .query(&rubylight(), false)
        .contains("rlClipboardSync"));
}

#[test]
fn image_enhancement_choices_go_only_for_what_the_host_offers() {
    let chosen = LaunchRequest {
        rtx_hdr: Some(true),
        rtx_video: Some(false),
        ..request()
    };
    // A host without the field, and other hosts, get exactly today's query.
    assert_eq!(rubylight().nvidia_enhancements(), 0);
    assert_eq!(chosen.query(&rubylight(), false), request().query(&rubylight(), false));
    let caps = HostCapabilities::UNKNOWN;
    assert_eq!(chosen.query(&caps, false), request().query(&caps, false));
    // A host offering only RTX Video gets only that choice.
    let video_only = ServerInfo::parse(
        &RUBYLIGHT.replace("</root>", "<NvidiaEnhancements>2</NvidiaEnhancements></root>"),
        47989,
    )
    .unwrap()
    .capabilities;
    assert_eq!(video_only.nvidia_enhancements(), 2);
    let query = chosen.query(&video_only, false);
    assert!(query.contains("&rlRtxVideo=0&") && !query.contains("rlRtxHdr"), "{query}");
    // Both, and following the app sends nothing.
    let both = ServerInfo::parse(
        &RUBYLIGHT.replace("</root>", "<NvidiaEnhancements>3</NvidiaEnhancements></root>"),
        47989,
    )
    .unwrap()
    .capabilities;
    let query = chosen.query(&both, false);
    assert!(query.contains("&rlRtxHdr=1&rlRtxVideo=0&"), "{query}");
    assert_eq!(request().query(&both, false), request().query(&rubylight(), false));
    // An input-only session encodes no picture.
    let input_only = LaunchRequest { role: AppRole::InputOnly, ..chosen };
    assert!(!input_only.query(&both, false).contains("rlRtx"));
    // Unknown bits and junk read as nothing more.
    for (text, mask) in [("255", 3), ("x", 0), ("-1", 0)] {
        let caps = ServerInfo::parse(
            &RUBYLIGHT.replace("</root>", &format!("<NvidiaEnhancements>{text}</NvidiaEnhancements></root>")),
            47989,
        )
        .unwrap()
        .capabilities;
        assert_eq!(caps.nvidia_enhancements(), mask, "{text}");
    }
}

#[test]
fn the_fec_choice_goes_only_to_a_host_that_takes_it() {
    let xml = RUBYLIGHT.replace("</root>", "<FecControlVersion>1</FecControlVersion></root>");
    assert_ne!(xml, RUBYLIGHT);
    let fec_host = ServerInfo::parse(&xml, 47989).unwrap().capabilities;
    assert!(fec_host.has(Feature::ClientFec));
    let plain = request().query(&fec_host, false);
    assert!(!plain.contains("rlFecPercentage"), "no choice, no parameter");
    for (choice, value) in [
        (FecChoice::Host, "host"),
        (FecChoice::Auto, "auto"),
        (FecChoice::Fixed(0), "0"),
        (FecChoice::Fixed(255), "255"),
    ] {
        let asked = LaunchRequest {
            fec: Some(choice),
            ..request()
        };
        assert_eq!(
            asked.query(&fec_host, false),
            plain.replace("&corever=1", &format!("&rlFecPercentage={value}&corever=1"))
        );
        // Hosts without the extension get exactly today's query.
        for caps in [HostCapabilities::UNKNOWN, rubylight()] {
            assert_eq!(asked.query(&caps, false), request().query(&caps, false));
        }
    }
}

#[test]
fn rate_changes_are_asked_for_at_launch_only_when_wanted() {
    for caps in [HostCapabilities::UNKNOWN, rubylight()] {
        let plain = request().query(&caps, false);
        assert!(!plain.contains("rlRateChange"));
        let asked = LaunchRequest {
            rate_change: true,
            fec: Some(FecChoice::Auto),
            ..request()
        };
        let fec = LaunchRequest {
            fec: Some(FecChoice::Auto),
            ..request()
        };
        assert_eq!(
            asked.query(&caps, false),
            fec.query(&caps, false)
                .replace("&corever=1", "&rlRateChange=1&corever=1")
        );
    }
}

#[test]
fn only_a_rubylight_host_hears_whether_the_launch_confirms_the_replace() {
    let confirmed = LaunchRequest {
        confirm_replace: true,
        ..request()
    };
    let plain = request().query(&rubylight(), false);
    assert!(
        plain.contains("&gcpersist=0&rlConfirmReplace=0&corever=1"),
        "{plain}"
    );
    assert_eq!(
        confirmed.query(&rubylight(), false),
        plain.replace("rlConfirmReplace=0", "rlConfirmReplace=1")
    );
    // Vibepollo and the rest keep their 60-second relaunch rule and today's query.
    let caps = HostCapabilities::UNKNOWN;
    assert_ne!(caps.family(), Family::Rubylight);
    assert_eq!(confirmed.query(&caps, false), request().query(&caps, false));
    assert!(!request().query(&caps, false).contains(CONFIRM_REPLACE));
}

#[test]
fn display_luminance_goes_to_other_hosts_at_launch_and_to_rubylight_by_control_message() {
    let lit = LaunchRequest {
        display_luminance: DisplayLuminance::from_display(1000.0, 0.005, 400.0, f32::NAN),
        ..request()
    };
    let other = HostCapabilities::UNKNOWN;
    assert!(lit.query(&other, false).ends_with(
        "&gcpersist=0&minBrightness=0.005&maxBrightness=1000.0&maxAverageBrightness=400.0&corever=1"
    ), "{}", lit.query(&other, false));
    assert_eq!(lit.query(&rubylight(), false), request().query(&rubylight(), false));
    let input = LaunchRequest { role: AppRole::InputOnly, ..lit.clone() };
    assert!(!input.query(&other, false).contains("Brightness"));
    let small = LaunchRequest { small_stream_input_only: true, ..input };
    assert!(!small.query(&other, false).contains("Brightness"));
    // Unknown luminance: nothing, and the host picks its own.
    assert_eq!(request().query(&other, false), lit.query(&other, false).replace(
        "&minBrightness=0.005&maxBrightness=1000.0&maxAverageBrightness=400.0", ""));
}

#[test]
fn only_the_replace_prompt_asks_to_confirm() {
    let status = |code, message: &str| Error::Status {
        code,
        message: message.into(),
    };
    assert!(asks_to_confirm_replace(&status(410, REPLACE_PROMPT)));
    // 410 also reports finished actions; other codes are refusals.
    assert!(!asks_to_confirm_replace(&status(
        410,
        "Application terminated"
    )));
    assert!(!asks_to_confirm_replace(&status(409, REPLACE_PROMPT)));
    assert!(!asks_to_confirm_replace(&Error::NotFound(
        REPLACE_PROMPT.into()
    )));
}

#[test]
fn roles_and_gamestream_quirks() {
    let monitor = LaunchRequest {
        role: AppRole::RemoteMonitor,
        gamepad_mask: 1,
        persist_gamepads: true,
        vrr: true,
        ..request()
    };
    let q = monitor.query(&rubylight(), false);
    assert!(q.contains("&remote_monitor=1&mode="));
    assert!(q.contains("&remoteControllersBitmap=0&gcmap=0&gcpersist=0"));
    let input = LaunchRequest {
        role: AppRole::InputOnly,
        hdr: true,
        vrr: true,
        ..request()
    };
    let q = input.query(&rubylight(), false);
    assert!(q.contains("&input_only=1") && !q.contains("hdrMode") && !q.contains("Vrr"));
    assert!(!q.contains("rlMicOnly"));
    let mic = LaunchRequest {
        mic_only: true,
        ..input.clone()
    };
    assert!(mic
        .query(&rubylight(), false)
        .contains("&input_only=1&rlMicOnly=1&mode="));
    let small = LaunchRequest {
        small_stream_input_only: true,
        ..input
    };
    assert!(!small.query(&rubylight(), false).contains("input_only"));

    let gfe = LaunchRequest {
        width: 2560,
        height: 1440,
        fps: 120,
        ..request()
    };
    let q = gfe.query(&HostCapabilities::UNKNOWN, true);
    assert!(q.contains("&mode=2560x1440x0&") && q.contains("&sops=0&"));
}

#[test]
fn launch_replies() {
    let ok = "<root status_code=\"200\"><gamesession>1</gamesession><sessionUrl0>rtspenc://10.0.0.2:48010</sessionUrl0></root>";
    assert_eq!(
        parse_launch_reply(ok, false).unwrap(),
        Some(Launched {
            rtsp_session_url: Some("rtspenc://10.0.0.2:48010".into())
        })
    );
    assert_eq!(
        parse_launch_reply("<root status_code=\"200\"><resume>0</resume></root>", true)
            .unwrap(),
        None
    );
    assert!(parse_launch_reply(ok, true).is_err());
    assert!(matches!(
        parse_launch_reply("<root status_code=\"503\" status_message=\"busy\"><gamesession>0</gamesession></root>", false),
        Err(Error::Status { code: 503, .. })
    ));
}
