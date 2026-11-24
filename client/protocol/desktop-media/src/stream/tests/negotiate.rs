//! Unit tests for stream bitrate, format negotiation, color probing and PyroWave notices.

use super::super::*;

pub(super) fn display() -> Display {
    Display {
        width: 2560,
        height: 1440,
        refresh_hz: 143.9,
        refresh_reported: true,
    }
}

#[test]
fn metered_limit_caps_fixed_auto_and_pyrowave_bitrates() {
    let settings = Settings { codec: CodecPreference::PyroWave, ..Default::default() };
    let display = Display { width: 1920, height: 1080, refresh_hz: 60.0, refresh_reported: true };
    let (_, _, _, bitrate) = stream_mode(&settings, display);
    assert!(bitrate > 5_000);
    assert_eq!(capped_bitrate(bitrate, 5_000), 5_000);
    assert_eq!(capped_bitrate(4_000, 5_000), 4_000);
    assert_eq!(capped_bitrate(bitrate, 0), bitrate);
}

#[test]
fn only_a_new_stream_at_the_automatic_bitrate_tests_the_link_first() {
    let info = |paired: u8, version: &str| ServerInfo::parse(&format!("<root status_code=\"200\"><uniqueid>pc</uniqueid><state>SERVER_FREE</state><PairStatus>{paired}</PairStatus><BandwidthProbeVersion>{version}</BandwidthProbeVersion></root>"), 47989).unwrap();
    let offered = info(1, "1");
    assert!(!probes_first_bitrate(&Settings::default(), false, &offered), "the link test is off by default");
    let auto = Settings { link_probe: true, ..Default::default() };
    assert!(probes_first_bitrate(&auto, false, &offered));
    assert!(!probes_first_bitrate(&auto, true, &offered), "a resume keeps the session's bitrate");
    assert!(!probes_first_bitrate(&Settings { bitrate_kbps: 30_000, ..auto.clone() }, false, &offered));
    for codec in [CodecPreference::PyroWave, CodecPreference::PyroWave444] {
        assert!(!probes_first_bitrate(&Settings { codec, ..auto.clone() }, false, &offered));
    }
    assert!(!probes_first_bitrate(&auto, false, &info(1, "0")), "a PC that does not offer the test");
    assert!(!probes_first_bitrate(&auto, false, &info(0, "1")), "an unpaired PC");
}

#[test]
fn native_mode_follows_the_display() {
    let (w, h, fps, kbps) = stream_mode(&Settings::default(), display());
    assert_eq!((w, h, fps), (2560, 1440, 144));
    assert_eq!(kbps, config::default_bitrate_kbps(2560, 1440, 144));
    let settings = Settings {
        width: 1920,
        height: 1080,
        fps: 60,
        bitrate_kbps: 30000,
        ..Default::default()
    };
    assert_eq!(stream_mode(&settings, display()), (1920, 1080, 60, 30000));
    // PyroWave chosen: a chosen bitrate stays, below its floor too (the settings warn), and the
    // automatic one is PyroWave's default (what the settings show).
    let mut settings = Settings { codec: CodecPreference::PyroWave444, ..settings };
    assert_eq!(stream_mode(&settings, display()).3, 30000);
    settings.bitrate_kbps = 0;
    assert_eq!(stream_mode(&settings, display()).3, 398_132);
    settings.bitrate_kbps = 250_000;
    assert_eq!(stream_mode(&settings, display()).3, 250_000);
    settings.bitrate_kbps = 2_000_000;
    assert_eq!(stream_mode(&settings, display()).3, 2_000_000);
}

// User report: PyroWave started at the H.264 default (20 Mbps at 1080p60), far below its floor,
// because the automatic bitrate did not know the codec.
#[test]
fn a_pyrowave_stream_at_the_automatic_bitrate_starts_at_pyrowaves_default() {
    let auto = Settings::default();
    // With the link unknown the default stays within 900 Mbps, unless the floor is higher.
    for (w, h, fps, kbps) in [(1920, 1080, 60, 398_132), (2560, 1440, 120, 900_000), (3840, 2160, 60, 900_000), (3840, 2160, 120, 1_492_992)] {
        assert_eq!(start_bitrate(&auto, w, h, fps, true, 0), kbps, "{w}x{h}@{fps}");
        assert_eq!(start_bitrate(&auto, w, h, fps, false, 0), config::default_bitrate_kbps(w, h, fps));
        assert!(kbps >= nightjar_policy::pyrowave::minimum_kbps(w, h, fps));
        // The settings show the same for PyroWave chosen.
        let chosen = Settings { width: w, height: h, fps, codec: CodecPreference::PyroWave, ..Default::default() };
        assert_eq!(stream_mode(&chosen, display()).3, kbps);
    }
    // Chosen bitrates stay as they are, whatever the link, also below the floor (the settings warn).
    let chosen = |kbps| Settings { bitrate_kbps: kbps, ..Default::default() };
    assert_eq!(start_bitrate(&chosen(300_000), 1920, 1080, 60, true, 0), 300_000);
    assert_eq!(start_bitrate(&chosen(900_000), 3840, 2160, 60, true, 0), 900_000);
    assert_eq!(start_bitrate(&chosen(1_500_000), 3840, 2160, 60, true, 1000), 1_500_000);
    assert_eq!(start_bitrate(&chosen(20_000), 1920, 1080, 60, true, 0), 20_000);
    assert_eq!(start_bitrate(&chosen(20_000), 1920, 1080, 60, false, 0), 20_000);
}

// A default over gigabit broke a first PyroWave stream on ordinary Ethernet and Wi-Fi: the
// default stays within 90% of a known wired link, never below the floor.
#[test]
fn pyrowaves_default_stays_within_the_wired_link() {
    let auto = Settings::default();
    assert_eq!(start_bitrate(&auto, 3840, 2160, 60, true, 1000), 900_000);
    assert_eq!(start_bitrate(&auto, 3840, 2160, 60, true, 2500), 1_592_525);
    assert_eq!(start_bitrate(&auto, 2560, 1440, 120, true, 2500), 1_415_578);
    assert_eq!(start_bitrate(&auto, 1920, 1080, 60, true, 100), 186_624);
    assert_eq!(start_bitrate(&auto, 1920, 1080, 60, false, 100), config::default_bitrate_kbps(1920, 1080, 60));
    // 10G gets the full clean target at 4K60, 1G about 900 Mbps at 1440p120, unknown 900.
    assert_eq!(start_bitrate(&auto, 3840, 2160, 60, true, 10_000), 1_592_525);
    assert_eq!(start_bitrate(&auto, 2560, 1440, 120, true, 1000), 900_000);
    assert_eq!(start_bitrate(&auto, 2560, 1440, 120, true, 0), 900_000);
}

// The bitrate follows what the launch offers the PC, not the codec choice alone: a PyroWave
// choice the PC lacks streams HEVC at the conventional default, and any offer the PC can answer
// with PyroWave starts at PyroWave's (the automatic choice included, should it offer PyroWave).
#[test]
fn the_launch_starts_pyrowave_bitrates_only_when_the_offer_streams_pyrowave() {
    let with = SCM_H264 | SCM_HEVC | SCM_PYROWAVE;
    let offered = |codec, scm| offered_formats(&Settings { codec, ..Default::default() }, scm, &decodes_all()).formats;
    assert!(offers_pyrowave(offered(CodecPreference::PyroWave, with), with));
    assert!(offers_pyrowave(offered(CodecPreference::PyroWave444, SCM_H264 | SCM_PYROWAVE_444), SCM_H264 | SCM_PYROWAVE_444));
    assert!(!offers_pyrowave(offered(CodecPreference::PyroWave, SCM_H264 | SCM_HEVC), SCM_H264 | SCM_HEVC), "the PC has no PyroWave");
    assert!(!offers_pyrowave(offered(CodecPreference::PyroWave, 0), 0), "no trusted codec support: the handshake never picks PyroWave");
    assert!(!offers_pyrowave(offered(CodecPreference::Auto, with), with), "automatic offers HEVC and H.264 only");
    assert!(offers_pyrowave(VIDEO_FORMAT_H264 | VIDEO_FORMAT_H265 | VIDEO_FORMAT_PYROWAVE, with), "an automatic offer that has PyroWave");
    assert_eq!(start_bitrate(&Settings::default(), 1920, 1080, 60, offers_pyrowave(VIDEO_FORMAT_H264 | VIDEO_FORMAT_PYROWAVE, with), 0), 398_132);
}

// A chosen bitrate below PyroWave's minimum is kept; the bitrate setting warns, naming the
// minimum for the mode.
#[test]
fn a_chosen_bitrate_below_pyrowaves_minimum_is_warned_in_the_settings() {
    let low = Settings { codec: CodecPreference::PyroWave, bitrate_kbps: 20_000, ..Default::default() };
    assert_eq!(pyrowave_bitrate_warning(&low, 1920, 1080, 60).as_deref(),
        Some("Below the level where the project's test scenes kept detail for PyroWave: 187 Mbps or more at 1920x1080, 60 fps. Text and textures may lose detail; raise the bitrate or use HEVC or AV1."));
    let german = crate::strings::with_lang(crate::strings::Lang::De, || pyrowave_bitrate_warning(&low, 1920, 1080, 60));
    assert_eq!(german.as_deref(), Some("PyroWave liegt unter dem Niveau, auf dem die Testbilder des Projekts Details behielten: 187 Mbit/s oder mehr bei 1920x1080, 60 fps. Text und Texturen können Details verlieren; erhöhe die Bitrate oder nutze HEVC oder AV1."));
    assert_eq!(pyrowave_bitrate_warning(&Settings { bitrate_kbps: 186_624, ..low.clone() }, 1920, 1080, 60), None);
    assert_eq!(pyrowave_bitrate_warning(&Settings { bitrate_kbps: 0, ..low.clone() }, 1920, 1080, 60), None, "automatic");
    assert_eq!(pyrowave_bitrate_warning(&Settings { codec: CodecPreference::Hevc, ..low.clone() }, 1920, 1080, 60), None);
    assert!(pyrowave_bitrate_warning(&Settings { codec: CodecPreference::PyroWave444, ..low }, 1920, 1080, 60).is_some());
}

// The player hears before the first picture when a PyroWave mode asks for more than the link
// carries; the stream still starts, so the words name the numbers and the way out.
#[test]
fn a_pyrowave_link_notice_names_the_mode_the_rate_and_the_link() {
    let notice = nightjar_policy::pyrowave::link_notice(3840, 2160, 120, 0, 0)
        .expect("4K120 on an unknown (wireless) link");
    let text = connect::pyrowave_link_text(&notice, 3840, 2160, 120);
    assert_eq!(text, "PyroWave at 3840×2160, 120 fps asks for 1493 Mbps; that may be more than this link carries (900 Mbps). Lower the resolution or frame rate, or use HEVC or AV1.");
    let german = crate::strings::with_lang(crate::strings::Lang::De,
        || connect::pyrowave_link_text(&notice, 3840, 2160, 120));
    assert_eq!(german, "PyroWave bei 3840×2160 mit 120 fps verlangt 1493 Mbps; das kann mehr sein, als diese Verbindung trägt (900 Mbps). Senke die Auflösung oder Bildrate oder nutze HEVC oder AV1.");
    // 1080p60 is far under the unknown link's cap, and a chosen rate above it is named too.
    assert_eq!(nightjar_policy::pyrowave::link_notice(1920, 1080, 60, 0, 0), None);
    let chosen = nightjar_policy::pyrowave::link_notice(1920, 1080, 60, 0, 1_500_000).expect("a chosen 1.5 Gbps");
    assert!(connect::pyrowave_link_text(&chosen, 1920, 1080, 60).contains("1500 Mbps"));
}

// The low-bitrate notice follows the bitrate actually sent, after this client's cap and the
// host's, not the setting: a 100 Mbps limit at 1080p60 and the automatic 4K240 default (held at
// the 2 Gbps runtime maximum) both land below the mode's level.
#[test]
fn the_pyrowave_minimum_notice_follows_the_bitrate_actually_sent() {
    let automatic = Settings::default();
    let capped = capped_bitrate(start_bitrate(&automatic, 1920, 1080, 60, true, 0), 100_000);
    assert_eq!(capped, 100_000);
    let notice = pyrowave_minimum_notice(capped, 1920, 1080, 60).expect("100 Mbps is below 187");
    assert!(notice.contains("187 Mbps or more at 1920x1080, 60 fps"), "{notice}");
    let automatic_4k = start_bitrate(&Settings::default(), 3840, 2160, 240, true, 0);
    assert_eq!(automatic_4k, 2_000_000);
    assert!(pyrowave_minimum_notice(automatic_4k, 3840, 2160, 240).is_some());
    // At the level itself there is nothing to warn about, whatever the setting says.
    assert_eq!(pyrowave_minimum_notice(186_624, 1920, 1080, 60), None);
}

// A PyroWave choice the PC answers with HEVC shows no PyroWave notice, even though the launch
// offered it: the notices follow the negotiated format, not the offer.
#[test]
fn a_fallback_to_hevc_shows_no_pyrowave_notice() {
    let (link, minimum) = pyrowave_notices_if_stream(
        VIDEO_FORMAT_H265, Some("link".into()), Some("minimum".into()));
    assert_eq!((link, minimum), (None, None));
    let (link, minimum) = pyrowave_notices_if_stream(
        VIDEO_FORMAT_PYROWAVE, Some("link".into()), Some("minimum".into()));
    assert_eq!((link.as_deref(), minimum.as_deref()), (Some("link"), Some("minimum")));
}

#[test]
fn formats_respect_the_host() {
    let all = SCM_H264 | SCM_HEVC | SCM_HEVC_MAIN10 | SCM_AV1_MAIN8 | SCM_AV1_MAIN10;
    assert_eq!(
        video_formats(CodecPreference::Auto, false, all),
        VIDEO_FORMAT_H264 | VIDEO_FORMAT_H265
    );
    assert_eq!(
        video_formats(CodecPreference::Av1, true, all),
        VIDEO_FORMAT_H264 | VIDEO_FORMAT_H265 | VIDEO_FORMAT_H265_MAIN10 | VIDEO_FORMAT_AV1_MAIN8 | VIDEO_FORMAT_AV1_MAIN10
    );
    // A PC without AV1 streams HEVC, not H.264.
    assert_eq!(video_formats(CodecPreference::Av1, false, SCM_H264 | SCM_HEVC), VIDEO_FORMAT_H264 | VIDEO_FORMAT_H265);
    assert_eq!(video_formats(CodecPreference::Hevc, false, SCM_H264), VIDEO_FORMAT_H264);
    assert_eq!(video_formats(CodecPreference::H264, true, all), VIDEO_FORMAT_H264);
    assert_eq!(video_formats(CodecPreference::PyroWave, false, SCM_H264 | SCM_PYROWAVE), VIDEO_FORMAT_H264 | VIDEO_FORMAT_PYROWAVE);
    assert_eq!(video_formats(CodecPreference::PyroWave444, true, SCM_H264 | SCM_PYROWAVE_MAIN10_444), VIDEO_FORMAT_H264 | VIDEO_FORMAT_PYROWAVE_MAIN10_444);
    assert_eq!(video_formats(CodecPreference::PyroWave444, false, SCM_H264), VIDEO_FORMAT_H264);
}

#[test]
fn the_streamed_pyrowave_names_its_chroma_and_depth() {
    assert_eq!(codec_name(VIDEO_FORMAT_PYROWAVE), "PyroWave");
    assert_eq!(codec_name(VIDEO_FORMAT_PYROWAVE_444), "PyroWave 4:4:4");
    assert_eq!(codec_name(VIDEO_FORMAT_PYROWAVE_MAIN10), "PyroWave 10-bit");
    assert_eq!(codec_name(VIDEO_FORMAT_PYROWAVE_MAIN10_444), "PyroWave 10-bit 4:4:4");
    assert_eq!(codec_name(VIDEO_FORMAT_H265_REXT8_444), "HEVC", "other codecs keep their names");
}

const COLOR_SCM: u32 = SCM_H264 | SCM_HEVC | SCM_AV1_MAIN8 | SCM_HEVC_MAIN10 | SCM_AV1_MAIN10
    | SCM_H264_HIGH8_444 | SCM_HEVC_REXT8_444 | SCM_AV1_HIGH8_444;

fn decodes_all() -> (u32, String) {
    (color::YUV444_FORMATS | color::SDR10_FORMATS | color::YUV444_10_FORMATS, String::new())
}

/// A PC that encodes HEVC RExt 4:4:4 10-bit (NVENC) streams both settings at once to a device
/// that decodes and draws it, and says both; without it 4:4:4 wins as before.
#[test]
fn both_settings_stream_10_bit_4_4_4_where_the_pc_and_the_device_have_it() {
    let both = Settings { codec: CodecPreference::Hevc, prefer_yuv444: true, sdr_10bit: true, ..Default::default() };
    let scm = COLOR_SCM | SCM_HEVC_REXT10_444;
    let offer = offered_formats(&both, scm, &decodes_all());
    assert_eq!(offer.formats, VIDEO_FORMAT_H264 | VIDEO_FORMAT_H265 | VIDEO_FORMAT_H265_REXT10_444);
    assert!(launches_ten_bit_sdr(&both, &offer), "an SDR launch of a 10-bit format says hdrMode=0");
    assert_eq!(color_notice(&offer, VIDEO_FORMAT_H265_REXT10_444).as_deref(), Some("Streaming in YUV 4:4:4.\nStreaming 10-bit color (SDR)."));
    let without = (color::YUV444_FORMATS | color::SDR10_FORMATS, String::new());
    let offer = offered_formats(&both, scm, &without);
    assert_eq!(offer.formats, VIDEO_FORMAT_H264 | VIDEO_FORMAT_H265 | VIDEO_FORMAT_H265_REXT8_444);
}

#[test]
fn the_launch_offers_what_the_4_4_4_and_10_bit_settings_add() {
    let hevc = Settings { codec: CodecPreference::Hevc, ..Default::default() };
    let plain = offered_formats(&hevc, COLOR_SCM, &decodes_all());
    assert_eq!(plain.formats, video_formats(CodecPreference::Hevc, false, COLOR_SCM), "both off: the codec choice alone");
    assert_eq!(color_notice(&plain, VIDEO_FORMAT_H265), None);
    let yuv444 = offered_formats(&Settings { prefer_yuv444: true, ..hevc.clone() }, COLOR_SCM, &decodes_all());
    assert_eq!(yuv444.formats, VIDEO_FORMAT_H264 | VIDEO_FORMAT_H265 | VIDEO_FORMAT_H265_REXT8_444);
    assert_eq!(color_notice(&yuv444, VIDEO_FORMAT_H265_REXT8_444).as_deref(), Some("Streaming in YUV 4:4:4."));
    let sdr10 = offered_formats(&Settings { sdr_10bit: true, ..hevc.clone() }, COLOR_SCM, &decodes_all());
    assert_eq!(sdr10.formats, VIDEO_FORMAT_H264 | VIDEO_FORMAT_H265 | VIDEO_FORMAT_H265_MAIN10);
    assert_eq!(color_notice(&sdr10, VIDEO_FORMAT_H265_MAIN10).as_deref(), Some("Streaming 10-bit color (SDR)."));
    assert_eq!(color_notice(&sdr10, VIDEO_FORMAT_H265).as_deref(), Some("This PC didn’t accept 10-bit, so the stream is 8-bit."));
    // The probe's answer decides: what the decoder can't draw is not offered, and its reason is told.
    let none = (0, "HEVC 4:4:4: the decoder failed on the fixture".to_owned());
    let refused = offered_formats(&Settings { prefer_yuv444: true, ..hevc.clone() }, COLOR_SCM, &none);
    assert_eq!(refused.formats, plain.formats);
    assert_eq!(color_notice(&refused, VIDEO_FORMAT_H265).as_deref(),
        Some("Sharper colored text isn’t available for this stream: this device can’t decode or draw it (HEVC 4:4:4: the decoder failed on the fixture)."));
    // HDR takes the stream and needs no word; packed full chroma keeps its own offer.
    let hdr = offered_formats(&Settings { hdr: true, prefer_yuv444: true, sdr_10bit: true, ..hevc.clone() }, COLOR_SCM, &decodes_all());
    assert_eq!(hdr.formats, video_formats(CodecPreference::Hevc, true, COLOR_SCM));
    assert_eq!(color_notice(&hdr, VIDEO_FORMAT_H265_MAIN10), None);
    let packed = offered_formats(&Settings { experimental_packed_444: true, prefer_yuv444: true, ..hevc.clone() }, COLOR_SCM, &decodes_all());
    assert_eq!(packed.formats, video_formats(CodecPreference::Hevc, false, 0));
    assert_eq!(color_notice(&packed, VIDEO_FORMAT_H265).as_deref(),
        Some("Sharper colored text isn’t available for this stream: sharper colored text is on and has its own 4:4:4 and 10-bit handling."));
    // Both on: 4:4:4 wins and 10-bit says why.
    let both = offered_formats(&Settings { prefer_yuv444: true, sdr_10bit: true, ..hevc }, COLOR_SCM, &decodes_all());
    assert_eq!(color_notice(&both, VIDEO_FORMAT_H265_REXT8_444).as_deref(),
        Some("Streaming in YUV 4:4:4.\n10-bit SDR isn’t used, so the stream is 8-bit: sharper colored text is on, and the PC takes 4:4:4 or 10-bit, not both."));
}

#[test]
fn a_ten_bit_sdr_offer_launches_with_hdr_mode_zero() {
    // Hardware lane 4a: without hdrMode=0 the host streamed 10-bit SDR as HDR and the SDR
    // display ended the stream at once, for HEVC Main10 and AV1 alike.
    for codec in [CodecPreference::Hevc, CodecPreference::Av1] {
        let sdr10 = Settings { sdr_10bit: true, codec, ..Default::default() };
        let offer = offered_formats(&sdr10, COLOR_SCM, &decodes_all());
        assert!(launches_ten_bit_sdr(&sdr10, &offer), "{codec:?}");
        let request = LaunchRequest { ten_bit_sdr: launches_ten_bit_sdr(&sdr10, &offer), ..Default::default() };
        assert!(request.query(&nightjar_core::http::HostCapabilities::UNKNOWN, false).contains("&hdrMode=0&"));
        // HDR asks for HDR; plain 8-bit, and 10-bit the PC or the decoder can't do, say nothing.
        let hdr = Settings { hdr: true, ..sdr10.clone() };
        assert!(!launches_ten_bit_sdr(&hdr, &offered_formats(&hdr, COLOR_SCM, &decodes_all())));
        let plain = Settings { codec, ..Default::default() };
        assert!(!launches_ten_bit_sdr(&plain, &offered_formats(&plain, COLOR_SCM, &decodes_all())));
        assert!(!launches_ten_bit_sdr(&sdr10, &offered_formats(&sdr10, SCM_H264 | SCM_HEVC | SCM_AV1_MAIN8, &decodes_all())));
        assert!(!launches_ten_bit_sdr(&sdr10, &offered_formats(&sdr10, COLOR_SCM, &(0, "no".into()))));
    }
}

#[test]
fn the_decoder_is_checked_only_when_a_setting_reaches_it() {
    let on = Settings { prefer_yuv444: true, ..Default::default() };
    assert!(!probes_color(&Settings::default()));
    assert!(probes_color(&on));
    assert!(probes_color(&Settings { sdr_10bit: true, ..Default::default() }));
    assert!(!probes_color(&Settings { hdr: true, ..on.clone() }));
    assert!(!probes_color(&Settings { experimental_packed_444: true, ..on.clone() }));
    assert!(!probes_color(&Settings { codec: CodecPreference::PyroWave, ..on }));
}

/// The launch decoded HEVC and AV1 fixtures on the UI thread whatever the stream could use.
#[test]
fn the_base_probe_checks_only_formats_the_launch_can_offer() {
    let with = |codec, hdr| base_probe_formats(&Settings { codec, hdr, ..Default::default() });
    assert_eq!(with(CodecPreference::H264, true), 0, "H.264 is not checked");
    assert_eq!(with(CodecPreference::Hevc, false), VIDEO_FORMAT_H265, "AV1 only when chosen");
    assert_eq!(with(CodecPreference::Auto, true), VIDEO_FORMAT_H265 | VIDEO_FORMAT_H265_MAIN10);
    assert_eq!(with(CodecPreference::Av1, false), VIDEO_FORMAT_H265 | VIDEO_FORMAT_AV1_MAIN8, "HEVC is AV1's fallback");
    assert_eq!(with(CodecPreference::Av1, true), color_probe::BASE_FORMATS);
    assert_eq!(with(CodecPreference::PyroWave, false), VIDEO_FORMAT_H265, "the HEVC PyroWave falls back to");
    let packed = Settings { experimental_packed_444: true, codec: CodecPreference::Av1, ..Default::default() };
    assert_eq!(base_probe_formats(&packed), 0);
    // What is not checked is not offered either: each launch's offer stays within it.
    for codec in [CodecPreference::H264, CodecPreference::Hevc, CodecPreference::Av1, CodecPreference::Auto, CodecPreference::PyroWave] {
        for hdr in [false, true] {
            let settings = Settings { codec, hdr, ..Default::default() };
            let offered = offered_formats(&settings, 0, &decodes_all()).formats;
            assert_eq!(offered & color_probe::BASE_FORMATS & !base_probe_formats(&settings), 0, "{codec:?} hdr {hdr}");
        }
    }
    assert!(color_probe::probe_base(Hardware::None, 0, &mut color_probe::ProbeBudget::default()).decodable == color_probe::BASE_FORMATS, "software is never checked");
}

/// A hardware decoder that cannot decode HEVC Main10 or AV1 (`color_probe::probe_base`) is
/// never offered them, so the PC never streams what FFmpeg would decode in software or not at
/// all; H.264 always stays, and packed full chroma checks its own formats.
#[test]
fn plain_formats_the_hardware_decoder_cannot_decode_are_not_offered() {
    let all = SCM_H264 | SCM_HEVC | SCM_HEVC_MAIN10 | SCM_AV1_MAIN8 | SCM_AV1_MAIN10;
    let hdr = |codec| Settings { codec, hdr: true, ..Default::default() };
    let offered = |settings: &Settings, decodable| offered_formats_decoding(settings, all, &decodes_all(), decodable).formats;
    let eight_bit = VIDEO_FORMAT_H265 | VIDEO_FORMAT_AV1_MAIN8;
    assert_eq!(offered(&hdr(CodecPreference::Hevc), color_probe::BASE_FORMATS), VIDEO_FORMAT_H264 | VIDEO_FORMAT_H265 | VIDEO_FORMAT_H265_MAIN10);
    assert_eq!(offered(&hdr(CodecPreference::Hevc), eight_bit), VIDEO_FORMAT_H264 | VIDEO_FORMAT_H265, "no Main10 decoder");
    // NUC (Intel Gen 9.5): AV1 with HDR streamed H.264 8-bit though HEVC Main10 decodes.
    let hevc_all = VIDEO_FORMAT_H265 | VIDEO_FORMAT_H265_MAIN10;
    assert_eq!(offered(&hdr(CodecPreference::Av1), hevc_all), VIDEO_FORMAT_H264 | hevc_all, "no AV1 decoder at all");
    assert_eq!(offered(&hdr(CodecPreference::Av1), VIDEO_FORMAT_H265), VIDEO_FORMAT_H264 | VIDEO_FORMAT_H265, "no 10-bit decoder");
    assert_eq!(offered(&hdr(CodecPreference::Av1), hevc_all | VIDEO_FORMAT_AV1_MAIN8), VIDEO_FORMAT_H264 | hevc_all,
        "AV1 8-bit only: HEVC Main10 keeps HDR");
    assert_eq!(offered(&hdr(CodecPreference::Av1), VIDEO_FORMAT_H265 | VIDEO_FORMAT_AV1_MAIN8), VIDEO_FORMAT_H264 | VIDEO_FORMAT_H265 | VIDEO_FORMAT_AV1_MAIN8);
    assert_eq!(offered(&hdr(CodecPreference::Av1), color_probe::BASE_FORMATS), VIDEO_FORMAT_H264 | color_probe::BASE_FORMATS);
    assert_eq!(offered(&Settings::default(), 0), VIDEO_FORMAT_H264);
    let packed = Settings { experimental_packed_444: true, codec: CodecPreference::Hevc, ..Default::default() };
    assert_eq!(offered(&packed, 0), offered(&packed, color_probe::BASE_FORMATS));
    // `offered_formats` is the offer of a decoder that decodes them all.
    assert_eq!(offered_formats(&hdr(CodecPreference::Hevc), all, &decodes_all()).formats, offered(&hdr(CodecPreference::Hevc), color_probe::BASE_FORMATS));
    // Software decoding decodes them all, by the software decoder.
    let software = color_probe::Base::software();
    assert_eq!(software.decodable, color_probe::BASE_FORMATS);
    assert!(software.backends.iter().all(|&(_, backend)| backend == Hardware::None) && software.backends.len() == 4);
    assert_eq!(color_probe::probe_base(Hardware::None, color_probe::BASE_FORMATS, &mut color_probe::ProbeBudget::default()), software, "software decoding is not checked");
}

#[test]
fn a_codec_this_device_does_not_decode_is_told_with_the_one_streamed() {
    let all = SCM_H264 | SCM_HEVC | SCM_HEVC_MAIN10 | SCM_AV1_MAIN8 | SCM_AV1_MAIN10;
    let av1_hdr = Settings { codec: CodecPreference::Av1, hdr: true, ..Default::default() };
    let hevc_all = VIDEO_FORMAT_H265 | VIDEO_FORMAT_H265_MAIN10;
    let (log, notice) = codec_fallback(&av1_hdr, all, hevc_all, 0, VIDEO_FORMAT_H265_MAIN10).expect("told");
    assert_eq!(notice, "This device doesn’t decode AV1 10-bit, so Nightjar uses HEVC Main10.");
    assert!(log.contains("AV1 10-bit was chosen") && log.ends_with("streaming HEVC Main10"), "{log}");
    let av1 = Settings { hdr: false, ..av1_hdr.clone() };
    assert_eq!(codec_fallback(&av1, all, hevc_all, 0, VIDEO_FORMAT_H265).map(|(_, n)| n).as_deref(),
        Some("This device doesn’t decode AV1, so Nightjar uses HEVC."));
    let hevc = Settings { codec: CodecPreference::Hevc, ..Default::default() };
    assert_eq!(codec_fallback(&hevc, all, 0, 0, VIDEO_FORMAT_H264).map(|(_, n)| n).as_deref(),
        Some("This device doesn’t decode HEVC, so Nightjar uses H.264."));
    // Nothing to tell: it decodes, it streams, the PC doesn't encode it, or no codec was chosen.
    assert_eq!(codec_fallback(&av1_hdr, all, color_probe::BASE_FORMATS, 0, VIDEO_FORMAT_AV1_MAIN10), None);
    assert_eq!(codec_fallback(&av1_hdr, SCM_H264 | SCM_HEVC | SCM_HEVC_MAIN10, hevc_all, 0, VIDEO_FORMAT_H265_MAIN10), None);
    assert_eq!(codec_fallback(&Settings::default(), all, 0, 0, VIDEO_FORMAT_H264), None);
    let german = crate::strings::with_lang(crate::strings::Lang::De, || codec_fallback(&hevc, all, 0, 0, VIDEO_FORMAT_H264));
    assert_eq!(german.map(|(_, n)| n).as_deref(), Some("Dieses Gerät decodiert HEVC nicht, daher nutzt Nightjar H.264."));
}

/// Packed full chroma on AV1 offers HEVC too (beecb7aa1), so AV1 the PC or device has no
/// packed mode for streams HEVC packed where it was refused: the player is told.
#[test]
fn packed_full_chroma_on_av1_that_streams_hevc_is_told() {
    let packed = Settings { codec: CodecPreference::Av1, experimental_packed_444: true, ..Default::default() };
    assert_ne!(offered_formats(&packed, 0, &decodes_all()).formats & VIDEO_FORMAT_H265, 0, "HEVC is the packed fallback");
    let (log, notice) = codec_fallback(&packed, 0, 0, 0, VIDEO_FORMAT_H265).expect("told");
    assert_eq!(notice, "Sharper colored text can’t run AV1 with this PC and device, so Nightjar uses HEVC.");
    assert!(log.contains("full chroma") && log.ends_with("streaming HEVC"), "{log}");
    // HDR: HEVC Main10 packed ranks above AV1 8-bit packed, and says so.
    let hdr = Settings { hdr: true, ..packed.clone() };
    assert_eq!(codec_fallback(&hdr, 0, 0, 0, VIDEO_FORMAT_H265_MAIN10).map(|(_, n)| n).as_deref(),
        Some("Sharper colored text can’t run AV1 10-bit with this PC and device, so Nightjar uses HEVC Main10."));
    // AV1 packed at either depth, and HEVC packed chosen, need no word (8-bit has its own notice).
    assert_eq!(codec_fallback(&packed, 0, 0, 0, VIDEO_FORMAT_AV1_MAIN8), None);
    assert_eq!(codec_fallback(&hdr, 0, 0, 0, VIDEO_FORMAT_AV1_MAIN8), None);
    assert_eq!(codec_fallback(&Settings { codec: CodecPreference::Hevc, ..hdr }, 0, 0, 0, VIDEO_FORMAT_H265), None);
    let german = crate::strings::with_lang(crate::strings::Lang::De, || codec_fallback(&packed, 0, 0, 0, VIDEO_FORMAT_H265));
    assert_eq!(german.map(|(_, n)| n).as_deref(), Some("Schärferer farbiger Text kann AV1 mit diesem PC und Gerät nicht nutzen, daher nutzt Nightjar HEVC."));
}

/// AV1 whose 8-bit decodes but whose Main10 or 4:4:4 doesn't streamed 10-bit SDR and 4:4:4
/// refused as AV1 8-bit, though HEVC carried them; HDR already made way (Android's rule).
#[test]
fn ten_bit_sdr_and_4_4_4_that_only_hevc_carries_stream_hevc_and_say_why() {
    let base = VIDEO_FORMAT_H265 | VIDEO_FORMAT_AV1_MAIN8;
    let hevc_color = (color::YUV444_FORMATS | color::SDR10_FORMATS) & !(VIDEO_FORMAT_AV1_MAIN10 | VIDEO_FORMAT_AV1_HIGH8_444);
    let sdr10 = Settings { codec: CodecPreference::Av1, sdr_10bit: true, ..Default::default() };
    let offer = offered_formats_decoding(&sdr10, COLOR_SCM, &(hevc_color, String::new()), base);
    assert_eq!((offer.formats, offer.sdr10.clone()), (VIDEO_FORMAT_H264 | VIDEO_FORMAT_H265 | VIDEO_FORMAT_H265_MAIN10, Ok(())));
    assert_eq!(codec_fallback(&sdr10, COLOR_SCM, base, hevc_color, VIDEO_FORMAT_H265_MAIN10).map(|(_, n)| n).as_deref(),
        Some("This device doesn’t decode AV1 10-bit, so Nightjar uses HEVC Main10."));
    let yuv444 = Settings { sdr_10bit: false, prefer_yuv444: true, ..sdr10.clone() };
    let offer = offered_formats_decoding(&yuv444, COLOR_SCM, &(hevc_color, String::new()), base);
    assert_eq!((offer.formats, offer.yuv444.clone()), (VIDEO_FORMAT_H264 | VIDEO_FORMAT_H265 | VIDEO_FORMAT_H265_REXT8_444, Ok(())));
    assert_eq!(codec_fallback(&yuv444, COLOR_SCM, base, hevc_color, VIDEO_FORMAT_H265_REXT8_444).map(|(_, n)| n).as_deref(),
        Some("This device doesn’t decode AV1 4:4:4, so Nightjar uses HEVC 4:4:4."));
    // A PC without AV1 Main10 makes way too, unsaid as with HDR; AV1 that carries it keeps AV1.
    let no_av1_main10 = COLOR_SCM & !SCM_AV1_MAIN10;
    let offer = offered_formats_decoding(&sdr10, no_av1_main10, &decodes_all(), base);
    assert_eq!(offer.formats & (VIDEO_FORMAT_AV1_MAIN8 | VIDEO_FORMAT_H265_MAIN10), VIDEO_FORMAT_H265_MAIN10);
    assert_eq!(codec_fallback(&sdr10, no_av1_main10, base, color::YUV444_FORMATS | color::SDR10_FORMATS, VIDEO_FORMAT_H265_MAIN10), None);
    let offer = offered_formats_decoding(&sdr10, COLOR_SCM, &decodes_all(), base);
    assert_eq!(offer.formats, VIDEO_FORMAT_H264 | base | VIDEO_FORMAT_AV1_MAIN10);
    assert_eq!(codec_fallback(&sdr10, COLOR_SCM, base, hevc_color, VIDEO_FORMAT_H265), None, "8-bit HEVC is no color fallback");
}

/// 10-bit launches decoded the 4:4:4 fixtures too with 4:4:4 off (about 12 ms).
#[test]
fn the_color_probe_checks_only_formats_the_settings_can_add() {
    let with = |codec, prefer_yuv444, sdr_10bit| color_probe_formats(&Settings { codec, prefer_yuv444, sdr_10bit, ..Default::default() });
    assert_eq!(with(CodecPreference::Hevc, false, false), 0);
    assert_eq!(with(CodecPreference::Hevc, false, true), VIDEO_FORMAT_H265_MAIN10);
    assert_eq!(with(CodecPreference::Hevc, true, false), VIDEO_FORMAT_H264_HIGH8_444 | VIDEO_FORMAT_H265_REXT8_444);
    assert_eq!(with(CodecPreference::Av1, true, true), color::YUV444_FORMATS | color::SDR10_FORMATS | color::YUV444_10_FORMATS);
    assert_eq!(with(CodecPreference::Hevc, true, true) & color::YUV444_10_FORMATS, VIDEO_FORMAT_H265_REXT10_444, "both on ask for 10-bit 4:4:4");
    assert_eq!(with(CodecPreference::Hevc, true, false) & color::YUV444_10_FORMATS, 0);
    assert_eq!(with(CodecPreference::Hevc, false, true) & color::YUV444_10_FORMATS, 0);
    assert_eq!(with(CodecPreference::H264, true, true), VIDEO_FORMAT_H264_HIGH8_444);
    assert_eq!(color_probe_formats(&Settings { hdr: true, sdr_10bit: true, ..Default::default() }), 0, "HDR takes the stream");
    // Whatever the settings, the offer adds no 4:4:4 or 10-bit format the probe skipped.
    for codec in [CodecPreference::H264, CodecPreference::Hevc, CodecPreference::Av1, CodecPreference::Auto] {
        for (yuv444, sdr10) in [(false, true), (true, false), (true, true)] {
            let settings = Settings { codec, prefer_yuv444: yuv444, sdr_10bit: sdr10, ..Default::default() };
            let decodable = (color_probe_formats(&settings), String::new());
            assert_eq!(offered_formats(&settings, COLOR_SCM, &decodable).formats, offered_formats(&settings, COLOR_SCM, &decodes_all()).formats,
                "{codec:?} {yuv444} {sdr10}");
        }
    }
}

#[test]
fn color_notices_are_german_in_german() {
    let offer = offered_formats(&Settings { sdr_10bit: true, codec: CodecPreference::Hevc, ..Default::default() }, SCM_H264 | SCM_HEVC, &decodes_all());
    let text = crate::strings::with_lang(crate::strings::Lang::De, || color_notice(&offer, VIDEO_FORMAT_H265));
    assert_eq!(text.as_deref(), Some("10-Bit-SDR wird nicht verwendet, der Stream läuft mit 8 Bit: der PC codiert es für diesen Codec nicht."));
}

#[test]
fn a_chosen_pyrowave_that_does_not_stream_is_logged_and_told() {
    let fallback = |codec: CodecPreference, scm: u32, negotiated: u32| {
        let offered = offered_formats(&Settings { codec, ..Default::default() }, scm, &decodes_all()).formats;
        pyrowave_fallback(codec, (scm, scm), offered, negotiated)
    };
    // The PC lists its codecs, and PyroWave is not among them: it was never offered.
    let (log, notice) = fallback(CodecPreference::PyroWave, SCM_H264 | SCM_HEVC, VIDEO_FORMAT_H265).expect("a fallback");
    assert_eq!(log, "nightjar: PyroWave was chosen, but the host does not offer it (codec support 0x101, reported 0x101); streaming HEVC");
    assert_eq!(notice, "This PC doesn’t offer PyroWave, so Nightjar uses HEVC.");
    // Offered, and the PC picked another codec.
    let (log, notice) = fallback(CodecPreference::PyroWave444, 0, VIDEO_FORMAT_H264).expect("a fallback");
    assert_eq!(log, "nightjar: PyroWave was chosen, but the host reports no trusted codec support and chose another codec (codec support 0x0, reported 0x0); streaming H.264");
    assert_eq!(notice, "This PC doesn’t offer PyroWave, so Nightjar uses H.264.");
    let (log, _) = fallback(CodecPreference::PyroWave, SCM_H264 | SCM_HEVC | SCM_PYROWAVE, VIDEO_FORMAT_H265).expect("a fallback");
    assert!(log.contains("but the host chose another codec (codec support"), "{log}");
    // The log keeps what serverinfo said when the trusted value stripped PyroWave.
    let (log, _) = pyrowave_fallback(CodecPreference::PyroWave, (SCM_H264 | SCM_HEVC, SCM_H264 | SCM_HEVC | SCM_PYROWAVE),
        video_formats(CodecPreference::PyroWave, false, SCM_H264 | SCM_HEVC), VIDEO_FORMAT_H265).expect("a fallback");
    assert!(log.contains(&format!("(codec support 0x101, reported {:#x})", SCM_H264 | SCM_HEVC | SCM_PYROWAVE)), "{log}");
    // PyroWave streams, or was not chosen: nothing to say.
    assert_eq!(fallback(CodecPreference::PyroWave, SCM_H264 | SCM_PYROWAVE, VIDEO_FORMAT_PYROWAVE), None);
    assert_eq!(fallback(CodecPreference::PyroWave444, SCM_H264 | SCM_PYROWAVE, VIDEO_FORMAT_PYROWAVE), None);
    assert_eq!(fallback(CodecPreference::Auto, SCM_H264 | SCM_HEVC, VIDEO_FORMAT_H265), None);
    let german = crate::strings::with_lang(crate::strings::Lang::De, || {
        fallback(CodecPreference::PyroWave, SCM_H264 | SCM_HEVC, VIDEO_FORMAT_H265).map(|(_, notice)| notice)
    });
    assert_eq!(german.as_deref(), Some("Dieser PC bietet PyroWave nicht an, daher nutzt Nightjar HEVC."));
}

#[test]
fn the_pyrowave_fallback_is_told_once_per_pc_until_pyrowave_streams_again() {
    let text = "This PC doesn’t offer PyroWave, so Nightjar uses HEVC.";
    let mut pc = HostRecord::default();
    let mut other = HostRecord::default();
    assert_eq!(pyrowave_notice(&mut pc, true, Some(text), VIDEO_FORMAT_H265), (Some(text.to_owned()), true), "the first time");
    // A reconnect, automatic or from the menu, and every later stream from this PC: the log only.
    assert_eq!(pyrowave_notice(&mut pc, true, Some(text), VIDEO_FORMAT_H265), (None, false));
    // Kept with the PC's record, so a restart of Nightjar stays quiet too.
    let mut saved: HostRecord = serde_json::from_str(&serde_json::to_string(&pc).unwrap()).unwrap();
    assert_eq!(pyrowave_notice(&mut saved, true, Some(text), VIDEO_FORMAT_H265), (None, false));
    assert_eq!(pyrowave_notice(&mut other, true, Some(text), VIDEO_FORMAT_H265), (Some(text.to_owned()), true), "another PC is told");
    // Streams that say nothing of PyroWave leave it as it is.
    assert_eq!(pyrowave_notice(&mut pc, false, None, VIDEO_FORMAT_H265), (None, false));
    assert_eq!(pyrowave_notice(&mut pc, true, None, VIDEO_FORMAT_H265), (None, false));
    // PyroWave streams from the PC again: losing it later is told again.
    assert_eq!(pyrowave_notice(&mut pc, true, None, VIDEO_FORMAT_PYROWAVE), (None, true));
    assert_eq!(pyrowave_notice(&mut pc, true, None, VIDEO_FORMAT_PYROWAVE), (None, false), "nothing to save then");
    assert_eq!(pyrowave_notice(&mut pc, true, Some(text), VIDEO_FORMAT_H265), (Some(text.to_owned()), true));
    // A record saved before the notices existed reads with none shown.
    let old: HostRecord = serde_json::from_str(r#"{"name":"PC","address":"192.0.2.1"}"#).unwrap();
    assert!(old.notices.is_empty());
}
