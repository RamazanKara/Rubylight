//! Single-attempt desktop stream launch, preflight, session start, and `Facts` assembly,
//! extracted from `stream::connect` to keep module size bounded while keeping all i18n
//! string literals in `connect.rs`.

use std::sync::Arc;

use super::super::*;
use super::{
    BEAM_RACE_NO_GPU, BEAM_RACE_NOT_WIRED, BeamRace, EARLY_NO_RECORDS, EarlyState, beam_race,
    early_decode, early_state, race_request, race_runs, video_capabilities,
};
use super::{
    ConnectError, JumboPreflight, JumboSkip, Location, asks_replace, audio_configuration,
    check_resume, frame_warp_for, hint_env, http_cause, input_only_small_refused_text,
    jumbo_fallback_text, jumbo_preflight, keep_roam_watch, link_hints, location_for_route,
    location_override, not_paired_text, plays_714, preflight_then_launch, probes_first_bitrate,
    pyrowave_link_text, pyrowave_start_line, reset_packet_size, resume_packet_size, resume_refused,
    roam_watch, route_line, session_config, start_cause, start_failure_text, tells_host_wireless,
};
use nightjar_core::connect::preflight as core_preflight;

pub(crate) fn connect(
    plan: &mut LaunchPlan,
    shared: Arc<Shared>,
    interrupt: &Interrupt,
) -> Result<(Session, Facts), ConnectError> {
    let settings = &plan.settings;
    let role = if plan.mic_only {
        AppRole::InputOnly
    } else {
        plan.app.role(plan.small_stream_input_only)
    };
    let (width, height, fps, _) = stream_mode(settings, plan.display);
    let audio = audio_configuration(
        settings.audio,
        plan.info.capabilities.has(Feature::Surround714),
        settings.audio == AudioChannels::Surround714 && plays_714(),
    );
    let ri_key: [u8; 16] = random_bytes();
    let key_id_bytes: [u8; 4] = random_bytes();
    let ri_key_id = i32::from_be_bytes(key_id_bytes) & 0x7FFF_FFFF;
    let refresh_x100 = (plan.display.refresh_hz * 100.0).round() as u32;
    let mut request = LaunchRequest {
        app_id: plan.app.id,
        app_uuid: plan.app.uuid.clone(),
        role,
        width,
        height,
        fps,
        client_refresh_rate_x100: refresh_x100,
        vrr: plan.vrr,
        hdr: settings.hdr,
        audio,
        ri_key,
        ri_key_id,
        play_local_audio: settings.play_audio_on_host,
        sops: settings.sops,
        gamepad_mask: plan.gamepad_mask,
        playstation_mask: plan.playstation_mask,
        persist_gamepads: crate::gamepad::one_player(settings),
        confirm_replace: plan.confirm_replace,
        prepared_controllers: if role != AppRole::RemoteMonitor
            && plan.info.capabilities.has(Feature::ControllerPreparation)
        {
            plan.prepared_controllers.clone()
        } else {
            String::new()
        },
        virtual_display: settings.virtual_display,
        virtual_display_scale: settings.virtual_display_scale,
        virtual_display_dpi: settings.virtual_display_dpi,
        host_display: settings.host_display.into(),
        fec: Some(settings.error_correction.into()),
        small_stream_input_only: plan.small_stream_input_only,
        mic_only: plan.mic_only,
        rate_change: rate_change::asked_at_launch(role),
        clipboard_sync: settings.clipboard_sync && role != AppRole::RemoteMonitor,
        display_luminance: plan.display_luminance,
        rtx_hdr: settings.rtx_hdr,
        rtx_video: settings.rtx_video,
        ..Default::default()
    };
    if plan.record.server_certificate.is_none() {
        return Err(not_paired_text().into());
    }
    let client = super::host_client_for(&plan.identity, &plan.record, &plan.record.address)?
        .with_https_port(plan.info.https_port);
    let client = Arc::new(client);
    *lock(&shared.host_client) = Some(client.clone());
    if interrupt.is_interrupted() {
        client.cancel_pending_requests();
        return Err("Cancelled.".into());
    }
    if plan.resume {
        plan.info = client.server_info_https().map_err(|e| ConnectError {
            retryable: recovery::transient_http(&e),
            cause: http_cause(&e),
            message: format!("Checking the running app failed: {e}"),
            asks_replace: false,
            moved_to: None,
            packed_refused: false,
        })?;
        crate::hosts_types::merge_native(&mut plan.info, plan.native.as_ref());
        let running = plan.app.matches_running_app(
            plan.info.current_game,
            plan.info.current_game_uuid.as_deref(),
        );
        let offered =
            plan.info.capabilities.availability(Feature::ContinueHere) == Availability::Supported;
        check_resume(plan.join, plan.info.paired, running, offered, || {
            client.stream_move()
        })?;
    }
    let scm = plan
        .info
        .capabilities
        .trusted_codec_mode_support(plan.info.server_codec_mode_support);
    let offer = offered_formats_decoding(
        settings,
        scm,
        &plan.color_decodable,
        plan.base_decodable.decodable,
    );
    request.ten_bit_sdr = launches_ten_bit_sdr(settings, &offer);
    let route = crate::network::route(&plan.record.address);
    let toward = route.toward;
    let network = plan.network.toward(toward);
    let family = plan.info.capabilities.family();
    let client_wireless =
        tells_host_wireless(settings.tell_host_wireless, family, network.wireless());
    let routed_link_mbps = network.wired_link_mbps();
    if network.wireless() && !client_wireless {
        let why = if settings.tell_host_wireless {
            "the PC is not a Rubylight host"
        } else {
            "it is turned off in the settings"
        };
        eprintln!("nightjar: not telling the PC the route is wireless: {why}");
    }
    eprintln!(
        "nightjar: route to the PC: {}",
        route_line(
            toward,
            network.wireless(),
            routed_link_mbps,
            network.link_mbps
        )
    );
    let measured_pace = plan
        .pace
        .as_ref()
        .map_or(0, |pace| pace.for_network(&network, plan.host_link_mbps));
    let LinkHints {
        link_mbps,
        sent_link_mbps,
        pace_mbps,
    } = link_hints(
        settings.tell_host_link_speed,
        routed_link_mbps,
        hint_env("NIGHTJAR_PYROWAVE_LINK_MBPS"),
        measured_pace,
        hint_env("NIGHTJAR_PYROWAVE_PACE_MBPS"),
    );
    if link_mbps > 0 && sent_link_mbps == 0 {
        eprintln!(
            "nightjar: not telling the PC the link speed ({link_mbps} Mbps): it is turned off in the settings"
        );
    }
    let pyrowave = offers_pyrowave(offer.formats, scm);
    let bitrate = capped_bitrate(
        start_bitrate(settings, width, height, fps, pyrowave, link_mbps),
        plan.bitrate_limit_kbps,
    );
    if pyrowave && settings.bitrate_kbps == 0 {
        eprintln!(
            "nightjar: {}",
            pyrowave_start_line(bitrate, width, height, fps, link_mbps)
        );
    }
    let should_probe = probes_first_bitrate(settings, plan.resume, &plan.info);
    let bitrate =
        core_preflight::first_bitrate_kbps(bitrate, should_probe, || client.probe_link(&plan.info));
    if interrupt.is_interrupted() {
        client.cancel_pending_requests();
        return Err("Cancelled.".into());
    }
    let punched = plan
        .record
        .address
        .parse()
        .is_ok_and(nightjar_core::internet::is_tunnel);
    let (location, route_packet_size) =
        location_for_route(location_override(), route.vpn || punched);
    let mut packet_size = route_packet_size;
    let mut jumbo_fallback = None;
    let mut jumbo_passed = false;
    if settings.packet_size > 0 {
        packet_size = settings.packet_size as i32;
        eprintln!("nightjar: --packet-size {packet_size}");
    }
    let auto_reconnect = plan.resume && plan.recovery.is_some();
    let preflight = jumbo_preflight(
        location,
        plan.record.address.parse::<std::net::IpAddr>().ok(),
        plan.info.capabilities.max_video_packet(),
        plan.info.pyrowave_burst_probe,
        settings.packet_size,
        settings.jumbo_frames,
        auto_reconnect,
    );
    let launched = preflight_then_launch(
        || match preflight {
            JumboPreflight::Probe { candidate } => {
                let outcome = core_preflight::run_jumbo_probe_with_client(
                    &client,
                    candidate,
                    route_packet_size,
                    settings.packet_size,
                    settings.jumbo_frames,
                );
                packet_size = outcome.packet_size;
                jumbo_passed = outcome.passed;
                if outcome.fallback_notice {
                    jumbo_fallback = Some(jumbo_fallback_text());
                }
            }
            JumboPreflight::Skip(JumboSkip::NotAsked) => {}
            JumboPreflight::Skip(reason) => {
                eprintln!(
                    "nightjar: jumbo video packets {}; streaming normal packets",
                    reason.reason()
                );
                packet_size = if auto_reconnect && reason == JumboSkip::Resume {
                    resume_packet_size(
                        plan.negotiated_packet_size,
                        route_packet_size,
                        settings.packet_size,
                    )
                } else {
                    reset_packet_size(route_packet_size, settings.packet_size)
                };
            }
        },
        || {
            if interrupt.is_interrupted() {
                return Err(HttpError::Cancelled);
            }
            if plan.resume {
                client.resume(&request, &plan.info)
            } else {
                client.launch(&request, &plan.info)
            }
        },
    );
    if interrupt.is_interrupted() {
        return Err("Cancelled.".into());
    }
    let refused = |why: String| -> ConnectError {
        if plan.small_stream_input_only {
            input_only_small_refused_text(&why).into()
        } else {
            why.into()
        }
    };
    let launched = match launched {
        Err(e) if asks_replace(plan, role, &e) => {
            return Err(ConnectError {
                message: e.to_string(),
                retryable: false,
                asks_replace: true,
                moved_to: None,
                packed_refused: false,
                cause: None,
            });
        }
        Ok(Some(mut launched)) => {
            // Shown once the stream runs; an automatic reconnect does not show it again.
            if let Some(warning) = launched.warning.take().filter(|_| !auto_reconnect) {
                shared.notify(crate::stream::pipeline::Notice::LaunchWarning(warning));
            }
            launched
        }
        Ok(None) => return Err(refused("The PC declined to start the app.".into())),
        Err(nightjar_core::http::Error::Status { code, message }) => {
            if let Some(why) = plan
                .resume
                .then(|| resume_refused(plan.join, code, &message))
                .flatten()
            {
                return Err(why.into());
            }
            return Err(refused(format!(
                "The PC could not start the app ({code}: {message})."
            )));
        }
        Err(e) => {
            return Err(ConnectError {
                retryable: recovery::transient_http(&e),
                cause: http_cause(&e),
                message: format!("Starting the app failed: {e}"),
                asks_replace: false,
                moved_to: None,
                packed_refused: false,
            });
        }
    };
    if interrupt.is_interrupted() {
        return Err("Cancelled.".into());
    }
    let beam = beam_race(settings.beam_race, link_mbps, plan.gpu_decoder.is_some());
    match beam {
        BeamRace::NotWired => eprintln!("{BEAM_RACE_NOT_WIRED}"),
        BeamRace::NoGpuDecoder => eprintln!("{BEAM_RACE_NO_GPU}"),
        BeamRace::On => {
            eprintln!("nightjar: beam race is on (experimental): asking the host for the row order")
        }
        BeamRace::Off => {}
    }
    let race_ask = race_request(beam, early_decode(settings.early_decode), height);
    if punched && location == Location::Remote {
        eprintln!(
            "nightjar: streaming over a direct internet path: remote with {packet_size} byte packets"
        );
    } else if route.vpn && location == Location::Remote {
        eprintln!(
            "nightjar: the route to the PC is a VPN: streaming as remote with {packet_size} byte packets"
        );
    }
    let mut iv = [0u8; 16];
    iv[..4].copy_from_slice(&ri_key_id.to_be_bytes());
    let cores = std::thread::available_parallelism().map_or(2, |n| n.get());
    let video_capabilities = video_capabilities(
        &plan.base_decodable,
        plan.hardware == Hardware::None,
        super::env_flag("NIGHTJAR_DESKTOP_RFI").unwrap_or(true),
        super::env_flag("NIGHTJAR_DESKTOP_SLICES").unwrap_or(settings.software_slices),
        cores,
    );
    let stream = StreamConfig {
        width: width as i32,
        height: height as i32,
        fps: fps as i32,
        bitrate_kbps: bitrate as i32,
        packet_size,
        audio,
        supported_video_formats: offer.formats,
        experimental_packed_444: settings.experimental_packed_444,
        packed444_modes: plan.packed_modes,
        client_refresh_rate_x100: refresh_x100 as i32,
        color_space: if settings.hdr {
            COLORSPACE_REC_2020
        } else {
            COLORSPACE_REC_709
        },
        color_range: if settings.full_range {
            COLOR_RANGE_FULL
        } else {
            COLOR_RANGE_LIMITED
        },
        microphone: settings.microphone && role != AppRole::RemoteMonitor,
        audio_passthrough: pipeline::passthrough_codecs(
            settings.audio_passthrough,
            settings.mute_audio,
        ),
        haptics_pcm: settings.controllers
            && settings.controller_audio
            && role != AppRole::RemoteMonitor,
        pyrowave_link_mbps: sent_link_mbps as i32,
        pyrowave_pace_mbps: pace_mbps as i32,
        client_wireless,
        pyrowave_records_supported: plan.info.capabilities.has(Feature::PyroWaveRecords),
        pyrowave_row_order: race_ask.row_order,
        encryption_flags: ENCFLG_ALL,
        remote_input_aes_key: ri_key,
        remote_input_aes_iv: iv,
        video_capabilities,
        audio_capabilities: CAPABILITY_DIRECT_SUBMIT,
        video_fec: settings.error_correction.into(),
        hdr_format: asked_hdr_format(hdr_format(
            settings.hdr,
            plan.info
                .capabilities
                .evidence()
                .map_or(-1, |evidence| evidence.hdr_formats),
            offer.formats,
            settings.experimental_packed_444,
            reads_dolby_vision(),
        )),
        // The draw pass takes only peaks from the RPUs, so it takes 8.1 full range where the host
        // offers it; the ANNOUNCE asks only then (sdp).
        dolby_vision_full_range: settings.full_range,
        ..Default::default()
    };
    let host = HostInfo {
        address: plan.record.address.clone(),
        app_version: plan
            .info
            .app_version
            .clone()
            .unwrap_or_else(|| "7.1.431.-1".into()),
        rtsp_session_url: launched.rtsp_session_url,
        server_codec_mode_support: if scm != 0 { scm } else { SCM_H264 },
    };
    let (early, slices) = (race_ask.early_decode, race_ask.slices);
    let watch = roam_watch(settings, &plan.record.address, &shared);
    let roam_shared = shared.clone();
    let usb_ask_started = settings.usb_forwarding && plan.info.capabilities.has(Feature::NativeApi);
    if usb_ask_started {
        let usb_client = client.clone();
        let usb_shared = shared.clone();
        if let Err(err) = std::thread::Builder::new()
            .name("usb-caps".into())
            .spawn(move || {
                let res = usb_client
                    .native_capabilities_with_files()
                    .map_err(|e| e.to_string());
                *lock(&usb_shared.usb_native) = Some(res);
                usb_shared.wake();
            })
        {
            *lock(&shared.usb_native) = Some(Err(format!("usb-caps thread failed: {err}")));
            shared.wake();
        }
    }
    let passthrough = stream.audio_passthrough;
    let frame_warp = frame_warp_for(settings, role, &plan.info.capabilities);
    let mut config = session_config(
        settings,
        role,
        stream,
        location,
        watch.is_some() && settings.client_roaming,
    );
    config.frame_warp = frame_warp.clone();
    let session = Session::start(
        &host,
        config,
        Box::new(
            pipeline::Decoder::new(shared.clone(), plan.hardware)
                .with_early_decode(early)
                .with_beam_race(slices)
                .with_gpu_decoder(plan.gpu_decoder)
                .with_dmabuf(plan.dmabuf)
                .with_kms_planes(plan.kms_planes)
                .with_device(plan.device.clone()),
        ),
        pipeline::audio_renderer(shared.clone(), passthrough, settings.mute_audio),
        Arc::new(pipeline::Events::new(shared.clone())),
        interrupt,
    )
    .map_err(|e| ConnectError {
        retryable: recovery::transient_start(e.stage, e.code),
        cause: start_cause(e.stage),
        message: start_failure_text(format!("Could not start the stream: {e}"), e.stage, || {
            plan.recovery
                .is_none()
                .then(|| {
                    client
                        .server_info(false)
                        .ok()
                        .and_then(|info| info.last_failure)
                })
                .flatten()
        }),
        asks_replace: false,
        moved_to: None,
        packed_refused: nightjar_core::full_chroma::restarts_without(
            settings.experimental_packed_444,
            e.code,
        ),
    })?;
    let outcome = session.outcome();
    let (stream_width, stream_height, stream_fps) = (
        outcome.config.width as u32,
        outcome.config.height as u32,
        outcome.config.fps.max(1) as u32,
    );
    let link = nightjar_policy::pyrowave::link_notice(
        stream_width,
        stream_height,
        stream_fps,
        link_mbps,
        settings.bitrate_kbps,
    )
    .map(|notice| pyrowave_link_text(&notice, stream_width, stream_height, stream_fps));
    let minimum = pyrowave_minimum_notice(
        outcome.config.bitrate_kbps as u32,
        stream_width,
        stream_height,
        stream_fps,
    );
    let (pyrowave_link, pyrowave_minimum) =
        pyrowave_notices_if_stream(outcome.negotiated_video_format, link, minimum);
    if jumbo_fallback.is_none()
        && jumbo_passed
        && outcome.config.packet_size
            <= i32::from(rubylight_protocol::video_packet::STOCK_MAX_VIDEO_PACKET)
        && settings.jumbo_frames
    {
        jumbo_fallback = Some(jumbo_fallback_text());
    }
    keep_roam_watch(&roam_shared, watch);
    session.watch_host(plan.info.https_port, gateway::toward);
    let frame_warp = frame_warp.filter(|_| {
        outcome
            .host_control_messages
            .contains(&nightjar_core::frame_warp::FRAME_WARP_REQUEST_MESSAGE_TYPE)
    });
    let frame_warp_refused = settings.frame_warp && role == AppRole::Stream && frame_warp.is_none();
    let mut facts = Facts {
        frame_warp,
        frame_warp_refused,
        wlan_streaming: wlan::should_enable(network.wireless(), network.error.is_some()),
        role,
        width: outcome.config.width,
        height: outcome.config.height,
        fps: outcome.config.fps,
        bitrate_kbps: outcome.config.bitrate_kbps,
        packet_size: super::facts_packet_size(packet_size, outcome.config.packet_size),
        format: outcome.negotiated_video_format,
        host_timing: outcome
            .host_control_messages
            .contains(&HOST_TIMING_MESSAGE_TYPE),
        link_notice: outcome
            .host_control_messages
            .contains(&LINK_NOTICE_MESSAGE_TYPE),
        pipeline_rates: outcome
            .host_control_messages
            .contains(&PIPELINE_RATES_MESSAGE_TYPE),
        clock_sync: outcome
            .host_control_messages
            .contains(&CLOCK_SYNC_MESSAGE_TYPE),
        display_caps: outcome
            .host_control_messages
            .contains(&rubylight_protocol::control::DISPLAY_CAPS_MESSAGE_TYPE),
        cursor_shapes: plan.info.capabilities.has(Feature::CursorShape)
            && outcome.host_control_messages.contains(&0x5540),
        text_input: plan.info.capabilities.has(Feature::TextInput)
            && outcome
                .host_control_messages
                .contains(&rubylight_protocol::text_input::TEXT_INPUT_MESSAGE_TYPE)
            && role != AppRole::RemoteMonitor,
        controller_limit: session.input(|q| q.max_controllers()),
        keyboard_layout: plan.info.capabilities.has(Feature::ClientKeyboardLayout)
            && outcome
                .host_control_messages
                .contains(&rubylight_protocol::keyboard_layout::KEYBOARD_LAYOUT_MESSAGE_TYPE)
            && role != AppRole::RemoteMonitor,
        clipboard: plan.info.capabilities.has(Feature::Clipboard) && role != AppRole::RemoteMonitor,
        clipboard_sync: settings.clipboard_sync
            && outcome
                .host_control_messages
                .contains(&rubylight_protocol::clipboard_sync::CLIPBOARD_SYNC_MESSAGE_TYPE)
            && role != AppRole::RemoteMonitor,
        image_clipboard: plan.info.capabilities.has(Feature::ImageClipboard)
            && role != AppRole::RemoteMonitor,
        server_commands: if plan.info.capabilities.has(Feature::ServerCommands)
            && role != AppRole::RemoteMonitor
        {
            plan.info
                .server_commands
                .iter()
                .take(256)
                .cloned()
                .collect()
        } else {
            Vec::new()
        },
        runtime_bitrate: plan.info.capabilities.has(Feature::RuntimeBitrate)
            && role != AppRole::InputOnly,
        reconfigure: outcome
            .host_control_messages
            .contains(&RECONFIGURE_MESSAGE_TYPE)
            && !outcome.config.experimental_packed_444
            && outcome.negotiated_video_format & VIDEO_FORMAT_MASK_PYROWAVE == 0
            && role == AppRole::Stream,
        rate_change: rate_change::offered(&outcome.host_control_messages, role),
        usb: if !settings.usb_forwarding {
            Availability::Unknown
        } else if !usb_ask_started {
            crate::usb::availability(&plan.info.capabilities, None)
        } else if let Some(native) = lock(&shared.usb_native).take() {
            crate::usb::availability(&plan.info.capabilities, Some(native))
        } else {
            // Defer waiting on slow/stalled hosts to Streaming::pump so connect readiness never blocks on USB metadata.
            shared.usb_pending.store(true, std::sync::atomic::Ordering::Release);
            Availability::Unknown
        },
        touch: outcome.sunshine_feature_flags
            & nightjar_core::control::negotiation::feature::PEN_TOUCH_EVENTS
            != 0
            && role != AppRole::RemoteMonitor,
        phase_lock: outcome
            .host_control_messages
            .contains(&rubylight_protocol::phase_lock::REPORT_MESSAGE_TYPE),
        early_decode: early_state(
            early,
            outcome.negotiated_video_format & VIDEO_FORMAT_MASK_PYROWAVE != 0,
            outcome.pyrowave_records_enabled,
        ) == EarlyState::On,
        early_without_records: early_state(
            early,
            outcome.negotiated_video_format & VIDEO_FORMAT_MASK_PYROWAVE != 0,
            outcome.pyrowave_records_enabled,
        ) == EarlyState::NoRecords,
        beam_race_off: beam.reason(),
        race_slices: slices,
        pace_mbps,
        beam_race: race_runs(
            slices,
            early_state(
                early,
                outcome.negotiated_video_format & VIDEO_FORMAT_MASK_PYROWAVE != 0,
                outcome.pyrowave_records_enabled,
            ),
        ),
        color_notice: color_notice(&offer, outcome.negotiated_video_format),
        pyrowave_fallback: None,
        pyrowave_link,
        pyrowave_minimum,
        jumbo_fallback,
        codec_fallback: None,
        app_name: plan.app.name.clone(),
        host_name: plan.host_name.clone(),
    };
    let reported = plan.info.server_codec_mode_support;
    if let Some((log, notice)) = pyrowave_fallback(
        settings.codec,
        (scm, reported),
        offer.formats,
        outcome.negotiated_video_format,
    ) {
        eprintln!("{log}");
        facts.pyrowave_fallback = Some(notice);
    }
    if let Some((log, notice)) = codec_fallback(
        settings,
        scm,
        plan.base_decodable.decodable,
        plan.color_decodable.0,
        outcome.negotiated_video_format,
    ) {
        eprintln!("{log}");
        facts.codec_fallback = Some(notice);
    }
    if facts.early_without_records {
        eprintln!("{EARLY_NO_RECORDS}");
    }
    let pen_barrel_roll = plan.info.capabilities.has(Feature::PenBarrelRoll)
        && session.outcome().host_pen_barrel_roll;
    session.input(|q| q.set_pen_barrel_roll(pen_barrel_roll));
    let controller_identity = plan.info.capabilities.has(Feature::ControllerIdentity);
    session.input(|q| q.set_controller_identity(controller_identity));
    Ok((session, facts))
}

pub(crate) fn recent_game(plan: &LaunchPlan) -> Option<config::Recent> {
    let record = &plan.record;
    if record.unique_id.is_empty()
        || plan.mic_only
        || !plan.listed
        || plan.app.id < 0
        || plan.app.control != nightjar_core::http::AppControl::None
    {
        return None;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    Some(config::Recent {
        host: record.unique_id.clone(),
        install_id: plan.app.uuid.clone(),
        app_id: plan.app.id as u32,
        title: plan.app.name.clone(),
        at: now,
    })
}
