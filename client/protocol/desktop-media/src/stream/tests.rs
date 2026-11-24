//! Unit tests for stream notices, overlay state, input forwarding, pacing and beam race.

mod negotiate;
mod teardown;

use super::*;

#[test]
fn a_notice_joins_the_one_still_showing_once_and_replaces_one_that_went() {
    use std::time::Duration;
    let planes = "Video planes are unavailable";
    let start = "Upscaling is off for this stream";
    assert_eq!(joined_toast(Some((start, Duration::from_millis(300))), planes), format!("{start}\n{planes}"));
    assert_eq!(joined_toast(Some((start, TOAST_SHOWN + Duration::from_millis(1))), planes), planes, "a notice that went is not kept");
    assert_eq!(joined_toast(None, planes), planes);
    let joined = format!("{start}\n{planes}");
    assert_eq!(joined_toast(Some((&joined, Duration::ZERO)), planes), joined, "the same notice again is not repeated");
}

#[test]
fn the_decode_time_after_streaming_reads_as_androids_toast() {
    assert_eq!(decode_time_text(0, 0, "HEVC"), None, "nothing decoded, nothing said");
    assert_eq!(decode_time_text(2_340 * 60, 60, "HEVC").as_deref(), Some("Average decode time: 2.3 ms [HEVC]"));
    assert_eq!(decode_time_text(12_600 * 10, 10, "AV1 10-bit").as_deref(), Some("Average decode time: 13 ms [AV1 10-bit]"));
    crate::strings::set(crate::strings::Lang::De);
    assert_eq!(decode_time_text(2_340 * 60, 60, "H.264").as_deref(), Some("Mittlere Dekodierzeit: 2,3 ms [H.264]"));
    crate::strings::set(crate::strings::Lang::En);
}

#[test]
fn the_latency_csv_has_the_statistics_header_and_rows() {
    let rows: VecDeque<String> = ["1000,60.000,60.000,60.000,0.000,2.000,1.500,0.200,3.000,9.000,20000\n".to_owned()].into();
    let csv = statistics_csv(&rows);
    let mut lines = csv.lines();
    assert_eq!(lines.next(), Some("elapsed_ms,received_fps,decoded_fps,shown_fps,loss_percent,rtt_ms,decode_ms,render_ms,host_to_network_ms,latency_ms,bitrate_kbps"));
    assert_eq!(lines.next(), Some("1000,60.000,60.000,60.000,0.000,2.000,1.500,0.200,3.000,9.000,20000"));
    assert_eq!(lines.next(), None);
    assert_eq!(LATENCY_CSV, "nightjar-latency.csv");
}

#[test]
fn an_overlay_change_names_only_what_the_user_changed() {
    // A PC's own Debug overlay in the top right, toggled off: the global mode and corner stay.
    let kept = (true, OverlayMode::Debug, OverlayCorner::TopRight);
    assert_eq!(overlay_change(kept, kept), None);
    assert_eq!(overlay_change(kept, (false, OverlayMode::Debug, OverlayCorner::TopRight)), Some((Some(false), None, None)));
    assert_eq!(overlay_change(kept, (true, OverlayMode::Statistics, OverlayCorner::TopLeft)),
        Some((None, Some(OverlayMode::Statistics), Some(OverlayCorner::TopLeft))));
}

/// HDR output gone mid-stream never ends it: the drawing goes to SDR once the loss has held for
/// a second, and back to HDR once HDR has held again; a flicker in between changes nothing.
#[test]
fn an_hdr_streams_output_change_is_followed_once_it_held_for_a_second() {
    let start = Instant::now();
    let at = |ms| start + Duration::from_millis(ms);
    let mut output = HdrOutput::new(true);
    assert_eq!(output.observe(true, at(0)), None);
    // A mode switch: HDR gone for half a second, then back.
    assert_eq!(output.observe(false, at(100)), None);
    assert_eq!(output.observe(false, at(600)), None);
    assert_eq!(output.observe(true, at(700)), None, "a flicker is not followed");
    assert_eq!(output.observe(false, at(1_500)), None, "the held time starts over");
    assert_eq!(output.observe(false, at(2_400)), None);
    assert_eq!(output.observe(false, at(2_500)), Some(false), "the window went to an SDR display");
    assert_eq!(output.observe(false, at(4_000)), None, "said once");
    assert_eq!(output.observe(true, at(4_100)), None);
    assert_eq!(output.observe(true, at(5_100)), Some(true), "and came back");
    assert_eq!(output.observe(true, at(9_000)), None);
}

#[test]
fn the_window_target_line_counts_direct_frames_against_presents() {
    assert_eq!(window_target_line(98, 100), "window target: 98 of 100 presents drawn straight into the swapchain, 2 through the back buffer");
    assert_eq!(window_target_line(0, 5), "window target: 0 of 5 presents drawn straight into the swapchain, 5 through the back buffer", "PQ output");
    assert_eq!(window_target_line(-3, 5), "window target: 0 of 5 presents drawn straight into the swapchain, 5 through the back buffer", "another renderer");
    assert_eq!(window_target_line(9, 5), "window target: 5 of 5 presents drawn straight into the swapchain, 0 through the back buffer", "launcher frames between the counts");
}

#[test]
fn the_window_target_line_counts_only_on_the_gpu_renderer() {
    assert_eq!(window_target_report("gpu", 98, 100), window_target_line(98, 100));
    assert_eq!(window_target_report("gpu", 0, 5), window_target_line(0, 5), "PQ output on the GPU renderer is a real back-buffer pass");
    assert_eq!(window_target_report("opengl", 0, 5), "window target: not counted on the opengl renderer, which draws through its own back buffer (5 presents)",
        "OpenGL has no swapchain image to draw into: no 0 of 5");
}

#[test]
fn the_window_target_count_survives_a_renderer_made_again_mid_stream() {
    let mut count = WindowTargetCount::default();
    // The launcher drew 40 frames on the first renderer before the stream.
    for frames in 40..43 {
        count.before_present(0x1000, frames);
        count.after_present(frames + 1);
    }
    assert_eq!((count.direct(), count.presents), (3, 3));
    // Made again for PQ: its count starts at zero, and PQ frames go through the back buffer.
    for _ in 0..2 {
        count.before_present(0x2000, 0);
        count.after_present(0);
    }
    assert_eq!((count.direct(), count.presents), (3, 5));
    // scRGB: straight into the swapchain again.
    for frames in 0..2 {
        count.before_present(0x3000, frames);
        count.after_present(frames + 1);
    }
    assert_eq!((count.direct(), count.presents), (5, 7));
    // Made again at the old address: a count below the last one is another renderer.
    count.before_present(0x3000, 0);
    count.after_present(1);
    assert_eq!((count.direct(), count.presents), (6, 8));
    assert_eq!(window_target_line(count.direct(), count.presents), "window target: 6 of 8 presents drawn straight into the swapchain, 2 through the back buffer");
}

#[test]
fn the_swapchain_metadata_changes_only_when_it_differs_and_goes_unknown_after_hdr() {
    assert_eq!(swapchain_metadata_change::<u8>(None, None), None, "an SDR stream sends nothing");
    assert_eq!(swapchain_metadata_change(Some(Some(1)), None), Some(Some(1)));
    assert_eq!(swapchain_metadata_change(Some(Some(1)), Some(Some(1))), None, "once");
    assert_eq!(swapchain_metadata_change(Some(Some(2)), Some(Some(1))), Some(Some(2)), "a new peak");
    assert_eq!(swapchain_metadata_change(None, Some(Some(2))), Some(None), "HDR stopped: unknown");
    assert_eq!(swapchain_metadata_change::<u8>(None, Some(None)), None, "unknown once");
    assert_eq!(swapchain_metadata_change(Some(None), Some(Some(2))), Some(None), "zeroed metadata from the host is unknown");
}
use super::input::*;

#[test]
fn a_shortcut_that_cannot_be_sent_says_why() {
    let keys = |list: &[&str]| list.iter().map(|k| k.to_string()).collect::<Vec<_>>();
    assert_eq!(shortcut_refusal(true, false, &keys(&["ALT", "TAB"])), None);
    assert_eq!(shortcut_refusal(false, true, &keys(&["ALT", "TAB"])),
        Some("Shortcut was not sent: the stream isn’t taking input right now."));
    assert!(shortcut_refusal(true, false, &keys(&["WIN", "D"])).unwrap().contains("Send system keys"));
    assert_eq!(shortcut_refusal(true, true, &keys(&["WIN", "D"])), None);
}

/// Desktop never asked for an HDR format, so no host sent it Dolby Vision RPUs and the draw
/// pass's RPU peak was unreachable. It asks as Android does, and stock hosts see nothing new.
#[test]
fn hdr_streams_ask_a_rubylight_host_for_dolby_vision_where_the_rpus_are_read() {
    use rubylight_protocol::hdr::{ALL, DV_101, DV_81, DV_84, HLG, PQ};
    let both = VIDEO_FORMAT_H265 | VIDEO_FORMAT_H265_MAIN10 | VIDEO_FORMAT_AV1_MAIN8 | VIDEO_FORMAT_AV1_MAIN10;
    let hevc = VIDEO_FORMAT_H265 | VIDEO_FORMAT_H265_MAIN10;
    let host = i32::from(ALL);
    // AV1 first, as the handshake takes it, then HEVC; each only where the host offers it.
    assert_eq!(hdr_format(true, host, both, false, true), DV_101);
    assert_eq!(hdr_format(true, host, hevc, false, true), DV_81);
    assert_eq!(hdr_format(true, i32::from(PQ | HLG | DV_81), both, false, true), DV_81);
    // Never profile 8.4 (an HLG base layer) nor HLG: every desktop HDR output takes PQ.
    assert_eq!(hdr_format(true, i32::from(PQ | HLG | DV_84), both, false, true), 0);
    // Stock HDR10 for a stock host (no HdrFormats: -1), an SDR stream, packed full chroma,
    // 8-bit formats only, and where nothing reads the RPUs (Linux, SDL drawing on Windows).
    assert_eq!(hdr_format(true, -1, both, false, true), 0);
    assert_eq!(hdr_format(true, 0, both, false, true), 0);
    assert_eq!(hdr_format(false, host, both, false, true), 0);
    assert_eq!(hdr_format(true, host, both, true, true), 0);
    assert_eq!(hdr_format(true, host, VIDEO_FORMAT_H265 | VIDEO_FORMAT_AV1_MAIN8, false, true), 0);
    assert_eq!(hdr_format(true, host, both, false, false), 0);
    #[cfg(not(windows))]
    assert!(!reads_dolby_vision(), "only the Windows draw pass reads them");
}

#[test]
fn the_doctor_offers_no_trial_while_the_client_hint_steers_the_bitrate() {
    let trial = nightjar_policy::health::BitrateTrial::default();
    let offer = |automatic| trial.offer(true, 20_000, true, automatic, false);
    let hint = rate_change::RateChange::new(20_000, 60_000, true, 0, 0);
    let manual = rate_change::RateChange::new(20_000, 60_000, false, 0, 0);
    assert_eq!(offer(bitrate_automatic(false, QualityMode::Off, Some(&hint))), TrialOffer::None);
    assert_eq!(offer(bitrate_automatic(false, QualityMode::Off, Some(&manual))), TrialOffer::Try);
    assert_eq!(offer(bitrate_automatic(false, QualityMode::Off, None)), TrialOffer::Try);
    assert_eq!(offer(bitrate_automatic(true, QualityMode::Off, None)), TrialOffer::None);
    assert_eq!(offer(bitrate_automatic(false, QualityMode::Motion, None)), TrialOffer::None);
}

#[test]
fn each_image_clipboard_refusal_says_its_own_reason() {
    use nightjar_core::http::{Error, ImageClipboardError as E};
    let texts = [
        image_error(E::TooLarge("20 MB".into()), true),
        image_error(E::NotPng("bad header".into()), true),
        image_error(E::NotPng("bad header".into()), false),
        image_error(E::Unsupported, true),
        image_error(E::Unsupported, false),
        image_error(E::Forbidden, false),
        image_error(E::Host(Error::Cancelled), false),
    ];
    assert!(texts[0].contains("too large") && texts[0].contains("20 MB"));
    assert!(texts[1].contains("this device") && texts[1].contains("bad header"));
    assert!(texts[2].contains("The PC sent"));
    assert!(texts[6].contains(&Error::Cancelled.to_string()));
    let distinct: HashSet<&String> = texts.iter().collect();
    assert_eq!(distinct.len(), texts.len(), "{texts:#?}");
}

#[test]
fn the_shortcut_chord_on_the_session_page_leaves_it_for_the_sheet() {
    assert_eq!(sheet_chord(false, None, None), SheetChord::Open(Sheet::FromChord));
    assert_eq!(sheet_chord(true, None, None), SheetChord::Open(Sheet::FromMenu));
    assert_eq!(sheet_chord(true, Some(Sheet::FromMenu), None), SheetChord::Close);
    assert_eq!(sheet_chord(true, Some(Sheet::FromChord), None), SheetChord::Close);
    // The sheet draws only without the Session page: the chord there shows it over the
    // controls, so Escape from it goes back to them.
    assert_eq!(sheet_chord(true, None, Some(false)), SheetChord::Open(Sheet::FromMenu));
    assert_eq!(sheet_chord(true, None, Some(true)), SheetChord::Stay, "a running End or Close game keeps its page");
}

#[test]
fn a_stream_behind_another_window_leaves_the_pointer_and_wheel_alone() {
    // capture_on_focus: captured before it comes to the front.
    assert!(!pointer_goes_out(true, false), "a pointer over the background window stays the local PC's");
    assert!(pointer_goes_out(true, true));
    assert!(!pointer_goes_out(false, true), "released");
}

#[test]
fn ctrl_alt_backspace_goes_to_the_pc_and_only_ctrl_alt_shift_stays_local() {
    use crate::shortcuts_runtime::ChordSdlExt as _;
    // Ctrl+Alt+Shift is Nightjar's own chord; every other one goes to the PC like any key.
    let local_chord = |mods| crate::shortcuts::Chord::nightjar(crate::shortcuts::Key::Tab).held(mods);
    let ctrl_alt = SDL_Keymod(SDL_KMOD_LCTRL.0 | SDL_KMOD_LALT.0);
    assert!(!local_chord(ctrl_alt), "a host's menu chord is not Nightjar's");
    for (system_keys, right_alt_as_meta) in [(false, false), (true, false), (false, true), (true, true)] {
        let sent = key_input(SDL_SCANCODE_BACKSPACE, ctrl_alt, system_keys, right_alt_as_meta).unwrap();
        assert_eq!((sent.code, sent.modifiers), (0x08, modifier::CTRL | modifier::ALT));
        // The modifier keys themselves go to the PC too.
        assert!(key_input(SDL_SCANCODE_LCTRL, ctrl_alt, system_keys, right_alt_as_meta).is_some());
        assert!(key_input(SDL_SCANCODE_LALT, ctrl_alt, system_keys, right_alt_as_meta).is_some());
    }
    assert!(local_chord(SDL_Keymod(ctrl_alt.0 | SDL_KMOD_LSHIFT.0)));
}

#[test]
fn the_wireless_hint_goes_only_to_a_rubylight_host_on_a_wireless_route_when_the_setting_is_on() {
    use nightjar_core::http::Family;
    assert!(tells_host_wireless(true, Family::Rubylight, true));
    assert!(!tells_host_wireless(false, Family::Rubylight, true));
    assert!(!tells_host_wireless(true, Family::Rubylight, false));
    for other in [Family::GameStream, Family::Sunshine, Family::Apollo, Family::Vibepollo, Family::Unknown] {
        assert!(!tells_host_wireless(true, other, true), "{other:?}");
    }
}

#[test]
fn the_link_speed_hint_is_left_out_when_the_setting_is_off_but_beam_race_and_pace_keep_the_real_speed() {
    // On: the host is told the routed speed.
    let on = link_hints(true, 2500, None, 2100, None);
    assert_eq!(on, LinkHints { link_mbps: 2500, sent_link_mbps: 2500, pace_mbps: 2100 });
    // Off: nothing is sent, the beam race still sees a wired link and the pace stays.
    let off = link_hints(false, 2500, None, 2100, None);
    assert_eq!(off, LinkHints { link_mbps: 2500, sent_link_mbps: 0, pace_mbps: 2100 });
    assert_eq!(beam_race_for(None, true, off.link_mbps, true), BeamRace::On);
    // An unknown speed is never sent, on or off.
    assert_eq!(link_hints(true, 0, None, 0, None).sent_link_mbps, 0);
    // A variable naming the speed wins over the setting, and the pace variable over the measurement.
    let forced = link_hints(false, 2500, Some(1000), 2100, Some(900));
    assert_eq!(forced, LinkHints { link_mbps: 1000, sent_link_mbps: 1000, pace_mbps: 900 });
}

#[test]
fn only_a_remote_monitor_session_is_view_only() {
    let settings = Settings::default();
    let config = |role| session_config(&settings, role, StreamConfig::default(), Location::Auto, true);
    let monitor = config(AppRole::RemoteMonitor);
    assert!(monitor.remote_monitor && !monitor.input_only);
    let stream = config(AppRole::Stream);
    assert!(!stream.remote_monitor && !stream.input_only);
    let input = config(AppRole::InputOnly);
    assert!(!input.remote_monitor && input.input_only);
}

#[test]
fn a_lost_or_reset_render_device_ends_the_stream_saying_why() {
    for kind in [SDL_EVENT_RENDER_DEVICE_RESET, SDL_EVENT_RENDER_DEVICE_LOST] {
        let reason = render_device_ended(kind).expect("ends the stream");
        assert!(!Exit::Ended(Some(reason.into())).retryable(true), "{reason}");
    }
    assert_eq!(render_device_ended(SDL_EVENT_RENDER_TARGETS_RESET), None);
}

#[test]
fn controllers_reach_the_host_only_when_on_and_not_from_a_remote_monitor() {
    let on = Settings { controllers: true, ..Settings::default() };
    let off = Settings { controllers: false, ..Settings::default() };
    assert!(sends_controllers(&on, AppRole::Stream));
    assert!(sends_controllers(&on, AppRole::InputOnly));
    assert!(!sends_controllers(&on, AppRole::RemoteMonitor));
    assert!(!sends_controllers(&off, AppRole::Stream));
}

/// A reconnect keeps the bitrate the stream settled on: an automatic cut is where automatic
/// bitrate starts again on an automatic reconnect, below the same ceiling; a bitrate the player
/// set is the plan's own and no cut is carried with it.
#[test]
fn a_reconnect_keeps_the_bitrate_the_stream_settled_on() {
    assert_eq!(reconnect_bitrate_start(true, true, false, 12_000), Some(12_000));
    assert_eq!(reconnect_bitrate_start(false, true, false, 12_000), None, "a Reconnect the player pressed");
    assert_eq!(reconnect_bitrate_start(true, false, false, 12_000), None, "automatic bitrate did not own the rate");
    assert_eq!(reconnect_bitrate_start(true, true, true, 12_000), None, "the player's bitrate is in the settings");
    assert_eq!(reconnect_bitrate_start(true, true, false, 0), None);
    assert_eq!(automatic_bitrate_start(30_000, Some(12_000)), 12_000);
    assert_eq!(automatic_bitrate_start(20_000, Some(30_000)), 20_000, "never above the encoder's rate");
    assert_eq!(automatic_bitrate_start(30_000, None), 30_000);
    assert_eq!(automatic_bitrate_start(30_000, Some(0)), 30_000);
}

#[test]
fn frames_waiting_for_the_decoder_reach_the_engine() {
    let config = |decode_queue_frames| {
        let settings = Settings { decode_queue_frames, ..Default::default() };
        session_config(&settings, AppRole::Stream, StreamConfig::default(), Location::Auto, true).decode_queue_bound
    };
    assert_eq!(config(15), 15);
    assert_eq!(config(5), 5);
}

#[test]
fn the_input_and_priority_settings_reach_the_engine() {
    let on = session_config(&Settings::default(), AppRole::Stream, StreamConfig::default(), Location::Local, true);
    assert!(on.unbatched_input && on.network_priority);
    // The desktop keeps the decode priority on (NIGHTJAR_DECODE_PRIORITY=0 for A/B runs).
    assert!(on.receive_decode_priority);
    assert_eq!((on.location, on.network_grace), (Location::Local, Duration::from_millis(3000)));
    for (unbatched_input, network_priority) in [(false, true), (true, false), (false, false)] {
        let settings = Settings { unbatched_input, network_priority, network_grace_ms: 0, ..Default::default() };
        let config = session_config(&settings, AppRole::Stream, StreamConfig::default(), Location::Auto, true);
        assert_eq!((config.unbatched_input, config.network_priority), (unbatched_input, network_priority));
        assert_eq!(config.network_grace, Duration::ZERO);
    }
}

#[test]
fn roaming_follows_the_settings_and_needs_mobile_data_without_the_watcher() {
    let on = session_config(&Settings::default(), AppRole::Stream, StreamConfig::default(), Location::Auto, true).roaming;
    assert_eq!(on, RoamingConfig { enabled: true, metered: false, platform_hints: true });
    let metered = Settings { roaming_metered: true, ..Default::default() };
    assert_eq!(roaming_config(&metered, true), RoamingConfig { enabled: true, metered: true, platform_hints: true });
    // Without the watcher a metered network looks like any other.
    assert!(!roaming_config(&Settings::default(), false).enabled);
    assert_eq!(roaming_config(&metered, false), RoamingConfig { enabled: true, metered: true, platform_hints: false });
    let off = Settings { client_roaming: false, roaming_metered: true, ..Default::default() };
    assert!(!roaming_config(&off, true).enabled && !roaming_config(&off, false).enabled);
}

#[test]
fn every_tearing_vrr_stream_checks_its_display_not_only_the_beam_race() {
    let checks = |settings: &Settings, tearing: bool, forced: Option<bool>| {
        let (pacing, vrr) = present::pacing(settings, tearing);
        checks_display_for_vrr(vrr, pacing, present::without_vrr(settings), forced)
    };
    // The default settings: VRR on, the swap chain tears, Real VRR.
    assert!(checks(&Settings::default(), true, None), "a fixed refresh display would tear on every present unchecked");
    let host_times = Settings { present_at_host_times: true, ..Default::default() };
    assert!(checks(&host_times, true, None));
    assert!(checks(&Settings { vsync: false, ..host_times.clone() }, true, None), "host times turn immediate");
    // Nothing to check: V-Sync already waits, VRR is off, the chain cannot tear, or V-Sync is
    // off in the settings and the stream is immediate already.
    assert!(!checks(&Settings::default(), false, None));
    let off = Settings { vrr: false, vsync: false, ..Default::default() };
    assert_eq!(present::pacing(&off, true).0, PacingMode::Immediate);
    assert!(!checks(&off, true, None), "tearing asked for without VRR is the user's choice");
    let shown_when_decoded = Settings { vsync: false, present_at_host_times: false, ..Default::default() };
    assert!(!checks(&shown_when_decoded, true, None), "V-Sync and Real VRR off: immediate either way");
    assert!(!checks(&Settings::default(), true, Some(false)), "NIGHTJAR_VRR_CHECK=0");
    assert!(checks(&Settings::default(), true, Some(true)));
}

/// The bug: with V-Sync off a VRR stream on Wayland stayed on MAILBOX on a fixed refresh
/// display, unchecked. The check now runs for its present mode.
#[test]
fn a_vrr_stream_without_v_sync_checks_its_display_for_the_present_mode() {
    // Real VRR off: frames shown when decoded, which needs no pacing check without V-Sync.
    let settings = Settings { vsync: false, present_at_host_times: false, ..Default::default() };
    let (pacing, vrr) = present::pacing(&settings, true);
    let pacing_check = checks_display_for_vrr(vrr, pacing, present::without_vrr(&settings), None);
    assert!(!pacing_check);
    assert!(checks_display_for_present_mode(vrr, pacing, pacing_check, true, None), "Wayland keeps MAILBOX for VRR");
    assert!(!checks_display_for_present_mode(vrr, pacing, pacing_check, false, None), "IMMEDIATE already (X11, gamescope)");
    assert!(!checks_display_for_present_mode(vrr, pacing, pacing_check, true, Some(false)), "NIGHTJAR_VRR_CHECK=0");
    assert!(!checks_display_for_present_mode(vrr, pacing, true, true, None), "the pacing check reads it already");
    assert!(!checks_display_for_present_mode(false, pacing, false, true, None), "no VRR, no MAILBOX");
    assert!(!checks_display_for_present_mode(vrr, PacingMode::Vsync, false, true, None), "V-Sync is FIFO");
    let vrr_pacing = present::VrrPacing::new(false, PacingMode::Immediate, present::VrrSync::Off, PacingMode::Immediate).with_present_check(true);
    assert_eq!(display_probe(false, &vrr_pacing), Some("display check"));
}

#[test]
fn the_scan_line_is_read_for_the_beam_race_the_display_check_and_vrr_sync_scan() {
    use present::{VrrPacing, VrrSync};
    let vrr = |check, sync| VrrPacing::new(check, PacingMode::Vsync, sync, PacingMode::Immediate);
    assert_eq!(display_probe(false, &vrr(true, VrrSync::Off)), Some("display check"), "every VRR stream that tears");
    assert_eq!(display_probe(true, &vrr(true, VrrSync::Off)), Some("beam race"));
    assert_eq!(display_probe(true, &vrr(false, VrrSync::Off)), Some("beam race"));
    // NIGHTJAR_VRR_SYNC=scan waits on the beam, so it reads the scan line with the check off.
    assert_eq!(display_probe(false, &vrr(false, VrrSync::Scan)), Some("display check"));
    assert_eq!(display_probe(false, &vrr(false, VrrSync::Blank)), Some("display check"), "blank reads the scan line too");
    assert_eq!(display_probe(false, &vrr(false, VrrSync::Present1)), None, "present1 reads no beam");
    assert_eq!(display_probe(false, &vrr(false, VrrSync::Off)), None, "a default stream without VRR does none of it");
}

#[test]
fn the_pacing_line_says_why_a_vrr_stream_waits_for_the_refresh() {
    assert_eq!(pacing_label(PacingMode::Vsync, true), "V-Sync: this display does not refresh on present");
    assert_eq!(pacing_label(PacingMode::Vsync, false), "V-Sync", "V-Sync the settings asked for");
    assert_eq!(pacing_label(PacingMode::Immediate, true), "Immediate: this display does not refresh on present", "V-Sync off in the settings");
    assert_eq!(pacing_label(PacingMode::Immediate, false), "Immediate");
}

#[test]
fn the_network_watcher_runs_for_the_stream_and_stops_with_it() {
    use std::sync::mpsc::RecvTimeoutError;
    let shared = Shared::new(Arc::new(|| {}), 8);
    // Kept whether the host roams or not: a lost network's return reconnects every stream.
    let (watch, ended) = crate::network::RoamWatch::idle();
    keep_roam_watch(&shared, Some(watch));
    assert!(lock(&shared.roam_watch).is_some());
    assert_eq!(ended.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty));
    lock(&shared.roam_watch).take();
    assert_eq!(ended.recv_timeout(Duration::from_secs(5)), Err(RecvTimeoutError::Disconnected));
}

#[test]
fn early_decode_without_records_is_reported_not_silent() {
    assert_eq!(early_state(false, true, true), EarlyState::Off);
    assert_eq!(early_state(true, false, false), EarlyState::Off);
    assert_eq!(early_state(true, true, true), EarlyState::On);
    // The 0-frames-decoded-ahead runs: asked for, PyroWave, but the host never offered records.
    assert_eq!(early_state(true, true, false), EarlyState::NoRecords);
}

#[test]
fn beam_race_line_names_a_host_that_sends_no_row_order() {
    assert_eq!(race_line(100, 300, 0), "100 frames in 300 slices, 0 not in row order");
    assert!(race_line(0, 0, 7).contains("not sending the row order"));
    assert!(!race_line(90, 270, 10).contains("not sending"));
}

#[test]
fn beam_race_needs_the_setting_and_a_wired_link() {
    // The environment override is for measurement runs; these cases are the settings.
    if std::env::var_os("NIGHTJAR_BEAM_RACE").is_some() { return; }
    assert_eq!(beam_race(false, 1000, true), BeamRace::Off);
    assert_eq!(beam_race(true, 1000, true), BeamRace::On);
    assert_eq!(beam_race(true, 0, true), BeamRace::NotWired);
}

#[test]
fn beam_race_needs_the_gpu_decoder_and_says_why_it_is_off() {
    // The forced value is the measurement override; the GPU decoder is needed either way.
    assert_eq!(beam_race_for(None, true, 1000, false), BeamRace::NoGpuDecoder);
    assert_eq!(beam_race_for(Some(true), false, 0, false), BeamRace::NoGpuDecoder);
    assert_eq!(beam_race_for(Some(true), false, 0, true), BeamRace::On);
    assert_eq!(beam_race_for(Some(false), true, 1000, true), BeamRace::Off);
    assert_eq!(beam_race_for(None, false, 1000, false), BeamRace::Off, "the setting off says nothing");
    assert!(BeamRace::NoGpuDecoder.reason().unwrap().contains("CPU"));
    assert!(BeamRace::NotWired.reason().unwrap().contains("wired"));
    assert_eq!((BeamRace::On.reason(), BeamRace::Off.reason()), (None, None));
}

#[test]
fn the_environment_switches_take_a_few_spellings_and_refuse_the_rest() {
    for on in ["1", "true", "TRUE", "yes", "On", " 1 "] { assert_eq!(flag_value(on), Some(true), "{on:?}"); }
    for off in ["0", "false", "No", "OFF"] { assert_eq!(flag_value(off), Some(false), "{off:?}"); }
    // Not a switch: said on stderr by `env_flag` and ignored, so the setting decides.
    for junk in ["", "2", "ture", "enable", "1 0"] { assert_eq!(flag_value(junk), None, "{junk:?}"); }
    assert_eq!(slices_override("4"), Some(4));
    assert_eq!(slices_override("7"), Some(MAX_RACE_SLICES));
    for bad in ["0", "8", "16", "four", "-1", ""] { assert_eq!(slices_override(bad), None, "{bad:?}"); }
    assert_eq!(beam_race_for(None, false, 1000, true), BeamRace::Off, "a refused value leaves the setting in charge");
}

#[test]
fn the_beam_race_off_asks_for_nothing() {
    // Whatever turned it off (the setting, `NIGHTJAR_BEAM_RACE=0`, a link or decoder that cannot race):
    // no row order from the host, no slices (so no probe, no slice handoff), early decode as the user set it.
    for beam in [BeamRace::Off, BeamRace::NotWired, BeamRace::NoGpuDecoder] {
        for early in [false, true] {
            assert_eq!(race_request(beam, early, 1080), RaceRequest { row_order: false, early_decode: early, slices: 0 }, "{beam:?}");
        }
    }
    for early in [EarlyState::Off, EarlyState::On, EarlyState::NoRecords] {
        assert!(!race_runs(0, early), "no slices asked for: the presenter never probes or follows a slice ({early:?})");
    }
    assert!(race_runs(4, EarlyState::On));
    assert!(!race_runs(4, EarlyState::NoRecords), "asked for, but the host sends no records to slice");
    let on = race_request(BeamRace::On, false, 1080);
    assert!(on.row_order && on.early_decode && on.slices > 0, "on asks for all of it: {on:?}");
    // The default settings never turn it on, on any link.
    assert!(!crate::config::Settings::default().beam_race);
    assert_eq!(beam_race_for(None, false, 1000, true), BeamRace::Off);
}

#[test]
fn beam_slices_are_four_or_three_at_4k() {
    if std::env::var_os("NIGHTJAR_BEAM_RACE_SLICES").is_some() { return; }
    assert_eq!(beam_slices(1080), 4);
    assert_eq!(beam_slices(2160), 3);
}
