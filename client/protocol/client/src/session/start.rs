//! Session startup sequence: name resolution, RTSP handshake, and control, video, audio and
//! input stream startup.
//!
//! Ported from moonlight-common-c's `Connection.c` (GPL-3.0, Cameron Gutman and contributors)
//! and Nightjar's `android_connection.c`.

use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::sync::atomic::Ordering;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use aes::cipher::KeyInit;

use super::shared::{io_code, rtsp_error_code, Microphone, Parts};
use super::{
    audio, error, net, roam, video, AudioRenderer, AudioSetup, HostInfo, Interrupt, Listener,
    Location, SessionConfig, Stage, StartError, VideoDecoder, VideoSetup, DECODE_UNIT_QUEUE_BOUND,
};
use crate::audio::{AudioConfig, AudioEncryption, AudioReceiver};
use crate::control::driver::{self, DriverConfig};
use crate::control::{ClientMessage, ControlEvent, HostCapabilities, SessionOptions};
use crate::input::microphone;
use crate::priority::Qos;
use crate::roaming::RoamEvent;
use crate::rtsp::{self, Handshake, HandshakeParams, RtspError};
use crate::sdp::{self, StreamConfig, StreamLocation};
use crate::video::{VideoConfig, VideoReceiver};

/// The local/remote decision and the packet size cap for `Auto`.
pub(super) fn decide_location(location: Location, remote: IpAddr, stream: &mut StreamConfig) {
    stream.location = match location {
        Location::Local => StreamLocation::Local,
        Location::Remote => StreamLocation::Remote,
        Location::Auto => {
            let nat64 = net::is_nat64(remote);
            if !nat64 && net::is_private(remote) {
                StreamLocation::Local
            } else {
                stream.packet_size = if remote.is_ipv4() || nat64 {
                    1024
                } else {
                    1184
                };
                StreamLocation::Remote
            }
        }
    };
}

/// Whether the session sends microphone audio to the host's `mic_port` (0: none offered).
/// `Err` says why not: a remote monitor is view-only and sends nothing from this client, and
/// without negotiated encryption the voice would go out in the clear.
pub(super) fn microphone_wanted(
    mic_port: u16,
    encryption_enabled: u32,
    remote_monitor: bool,
) -> Result<bool, &'static str> {
    if mic_port == 0 {
        Ok(false)
    } else if remote_monitor {
        Err("Microphone: off in a remote monitor session")
    } else if encryption_enabled & sdp::SS_ENC_MICROPHONE == 0 {
        Err("Microphone: encryption was not negotiated; not sending")
    } else {
        Ok(true)
    }
}

/// The microphone's socket; a failure costs only the microphone, and the listener hears why.
pub(super) fn open_microphone_socket(
    local: SocketAddr,
    network_priority: bool,
    listener: &dyn Listener,
) -> Option<UdpSocket> {
    match net::bind_udp(local, 0, Qos::Audio, network_priority, |m| listener.warn(m)) {
        Ok(socket) => Some(socket),
        Err(e) => {
            listener.warn(&format!("Microphone: could not open the socket: {e}"));
            None
        }
    }
}

pub(super) fn start(
    parts: &mut Parts,
    host: &HostInfo,
    mut config: SessionConfig,
    video_decoder: Box<dyn VideoDecoder>,
    audio_renderer: Box<dyn AudioRenderer>,
    listener: &dyn Listener,
    interrupt: &Interrupt,
) -> Result<(), StartError> {
    let fail = |stage: Stage, code: i32, message: String| StartError {
        stage,
        code,
        message,
    };
    let check_interrupt = |stage: Stage| {
        if interrupt.is_interrupted() {
            Err(fail(stage, error::FAILED, "interrupted".into()))
        } else {
            Ok(())
        }
    };
    if host.server_codec_mode_support == 0 {
        return Err(fail(
            Stage::PlatformInit,
            error::FAILED,
            "server_codec_mode_support must be set".into(),
        ));
    }

    listener.stage_starting(Stage::PlatformInit);
    listener.stage_complete(Stage::PlatformInit);

    listener.stage_starting(Stage::NameResolution);
    let rtsp_port = rtsp::rtsp_port_from_session_url(host.rtsp_session_url.as_deref());
    // The well-known HTTP ports always listen on a standard host; 48010 may not yet.
    let test_ports: &[u16] = if rtsp_port != rtsp::DEFAULT_RTSP_PORT {
        &[rtsp_port]
    } else {
        &[47984, 47989, 48010]
    };
    // The addresses are probed at once (`net::probe`). Only an alternate RTSP port is tried a
    // second time, a second later, as the C core does: the host may not listen on it yet,
    // while the standard ports always listen.
    let remote = net::resolve(&host.address, test_ports)
        .or_else(|e| {
            if rtsp_port == rtsp::DEFAULT_RTSP_PORT {
                return Err(e);
            }
            std::thread::sleep(Duration::from_secs(1));
            net::resolve(&host.address, test_ports)
        })
        .map_err(|e| fail(Stage::NameResolution, io_code(&e), e.to_string()))?;
    let local = net::local_address_toward(net::with_port(remote, rtsp_port))
        .map_err(|e| fail(Stage::NameResolution, io_code(&e), e.to_string()))?;
    parts.remote = Some(remote);
    parts.local = Some(local);
    listener.stage_complete(Stage::NameResolution);
    decide_location(config.location, remote.ip(), &mut config.stream);

    listener.stage_starting(Stage::AudioStreamInit);
    listener.stage_complete(Stage::AudioStreamInit);

    listener.stage_starting(Stage::RtspHandshake);
    let scope = match remote {
        SocketAddr::V6(v6) => v6.scope_id(),
        SocketAddr::V4(_) => 0,
    };
    let rtsp_fail = |e: RtspError| fail(Stage::RtspHandshake, rtsp_error_code(&e), e.to_string());
    let mut handshake = Handshake::new(HandshakeParams {
        address: remote.ip(),
        ipv6_scope_id: scope,
        app_version: host.app_version.clone(),
        server_codec_mode_support: host.server_codec_mode_support,
        rtsp_session_url: host.rtsp_session_url.clone(),
        config: config.stream.clone(),
    })
    .map_err(rtsp_fail)?;
    let mut rtsp_address = remote;
    rtsp_address.set_port(handshake.rtsp_port());
    let interrupted = interrupt.0.clone();
    while let Some(request) = handshake.next_request().map_err(rtsp_fail)? {
        let response = rtsp::transact(
            rtsp_address,
            &request,
            &interrupted,
            config.network_priority,
            |m| listener.warn(m),
        )
        .map_err(rtsp_fail)?;
        handshake.handle_response(&response).map_err(rtsp_fail)?;
        if parts.audio_socket.is_none()
            && let Some(port) = handshake.audio_port()
        {
            // notifyAudioPortNegotiationComplete: ping before PLAY.
            let socket = net::bind_udp(local, 0, Qos::Audio, config.network_priority, |m| {
                listener.warn(m)
            })
            .map_err(|e| fail(Stage::RtspHandshake, io_code(&e), e.to_string()))?;
            parts.audio_pinger = Some(
                net::Pinger::start(
                    "AudioPing",
                    &socket,
                    net::with_port(remote, port),
                    handshake.audio_ping_payload(),
                )
                .map_err(|e| fail(Stage::RtspHandshake, io_code(&e), e.to_string()))?,
            );
            parts.audio_socket = Some(socket);
        }
    }
    let outcome = handshake.finish().map_err(rtsp_fail)?;
    if let Some(reason) = outcome.packed_ten_bit_dropped() {
        listener.warn(&format!("Packed 4:4:4: {reason}; streaming 8-bit SDR"));
    }
    listener.host_control_messages(&outcome.host_control_messages);
    listener.host_packed444_modes(outcome.host_packed444_modes);
    listener.announced_hdr_format(outcome.hdr_format);
    listener.stage_complete(Stage::RtspHandshake);
    check_interrupt(Stage::RtspHandshake)?;

    listener.stage_starting(Stage::ControlStreamInit);
    let caps = HostCapabilities {
        app_version: outcome.app_version,
        feature_flags: outcome.sunshine_feature_flags,
        encryption_enabled: outcome.encryption_enabled,
        control_messages: outcome.host_control_messages.clone(),
        client_roaming: outcome.host_roaming,
    };
    if !caps.encrypted_control() {
        return Err(fail(
            Stage::ControlStreamInit,
            error::FAILED,
            "the host's control stream generation is not supported (GFE before 3.22)".into(),
        ));
    }
    let mut driver_config = DriverConfig::new(net::with_port(remote, outcome.control_port));
    // Bound where the media are: a roam moves them all together (`roam::Roamer`).
    let roaming = config.roaming.enabled && outcome.host_roaming;
    driver_config.local = Some(local);
    driver_config.roaming = config.roaming.enabled;
    if roaming {
        driver_config.roam_seq = roam::launch_seq(&outcome.config.remote_input_aes_key);
    }
    driver_config.connect_data = outcome.control_connect_data;
    driver_config.network_grace = config.network_grace;
    driver_config.unbatched_input = config.unbatched_input;
    driver_config.input_disabled = config.remote_monitor;
    driver_config.frame_warp = config.frame_warp.clone().filter(|_| !config.remote_monitor);
    driver_config.warn = driver::Warn::new({
        let shared = parts.shared.clone();
        move |message| shared.warn(message)
    });
    listener.stage_complete(Stage::ControlStreamInit);

    listener.stage_starting(Stage::VideoStreamInit);
    let stream = &outcome.config;
    let video_encrypted = outcome.encryption_enabled & sdp::SS_ENC_VIDEO != 0;
    let mut receiver = VideoReceiver::new(VideoConfig {
        format: outcome.negotiated_video_format,
        packet_size: stream.packet_size as usize,
        app_version: outcome.app_version,
        rfi: outcome.reference_frame_invalidation_enabled(),
        pyrowave_records: outcome.pyrowave_records_enabled,
        colorspace: stream.color_space as u8,
        encryption_key: video_encrypted.then_some(stream.remote_input_aes_key),
        early_chunks: video_decoder.wants_early_chunks(),
    });
    // The host runs congestion control on per-packet arrivals (0x5547).
    if caps.supports_control_message(
        rubylight_protocol::transport_feedback::TRANSPORT_FEEDBACK_MESSAGE_TYPE,
    ) {
        receiver.enable_transport_feedback();
        parts
            .shared
            .log("Video arrival feedback is on: the host adapts to the network from it");
    }
    let video_shared = Arc::new(video::VideoShared::with_queue_bound(
        receiver,
        config.decode_queue_bound,
    ));
    if video_shared.queue.bound() != DECODE_UNIT_QUEUE_BOUND {
        parts.shared.log(&format!(
            "Decode queue holds up to {} frames (setting); a decoder that takes frames directly has none",
            video_shared.queue.bound()
        ));
    }
    parts.video_shared = Some(video_shared.clone());
    listener.stage_complete(Stage::VideoStreamInit);

    listener.stage_starting(Stage::InputStreamInit);
    listener.stage_complete(Stage::InputStreamInit);

    listener.stage_starting(Stage::ControlStreamStart);
    let options = SessionOptions {
        reference_frame_invalidation: outcome.reference_frame_invalidation_enabled(),
        fec_summaries: outcome.fec_summaries,
    };
    let (client, control) =
        driver::connect(driver_config, caps, &stream.remote_input_aes_key, options).map_err(
            |e| {
                let code = match &e {
                    driver::ConnectError::Io(io) => io_code(io),
                    _ => error::FAILED,
                };
                fail(Stage::ControlStreamStart, code, e.to_string())
            },
        )?;
    parts.control = Some(control.clone());
    if let Some(warp) = config.frame_warp.as_ref().filter(|_| !config.remote_monitor) {
        let request = warp.request(true);
        match control.send(ClientMessage::FrameWarpRequest(request)) {
            Ok(()) => listener.log("Frame Warp: asked the host for its model, tags and HUD mask"),
            Err(error) => listener.log(&format!("Frame Warp: the host does not offer it ({error:?})")),
        }
    }
    let events = parts.shared.clone();
    let hdr_video = video_shared.clone();
    // Set once the streams run. Only the roamer moves sockets, so a roam that commits before
    // it exists left nothing behind.
    let roamed_by = Arc::new(OnceLock::<Arc<roam::Roamer>>::new());
    let roamed = roamed_by.clone();
    parts.control_thread = Some(
        std::thread::Builder::new()
            .name("ControlRecv".into())
            .spawn(move || {
                events.prioritize_thread();
                client.run(|event| match event {
                    ControlEvent::Terminated { error } => events.terminate(error),
                    event => {
                        if let ControlEvent::HdrMode { enabled, .. } = event {
                            hdr_video
                                .receiver
                                .lock()
                                .unwrap_or_else(|p| p.into_inner())
                                .set_hdr_active(enabled);
                        }
                        if let ControlEvent::Roam(RoamEvent::Roamed { .. }) = event
                            && let Some(roamer) = roamed.get()
                        {
                            roamer.roamed();
                        }
                        events.listener.control_event(event);
                    }
                })
            })
            .map_err(|e| fail(Stage::ControlStreamStart, io_code(&e), e.to_string()))?,
    );
    listener.stage_complete(Stage::ControlStreamStart);
    check_interrupt(Stage::ControlStreamStart)?;

    // Input-only sessions never set up or clean up the decoder and renderer. They still ping
    // the video port: Vibepollo and Apollo hosts run their video thread regardless and end
    // the session when the first ping does not arrive within ten seconds.
    listener.stage_starting(Stage::VideoStreamStart);
    let video_buffer = crate::video::recv_buffer_size(
        stream.bitrate_kbps.max(0) as u32,
        stream.packet_size as usize,
    );
    if config.input_only {
        // Pings are best effort: a refused or failed send is ignored, nothing reads replies.
        match net::bind_udp(local, 0, Qos::Video, config.network_priority, |m| {
            listener.warn(m)
        })
        .and_then(|socket| {
            net::Pinger::start(
                "VideoPing",
                &socket,
                net::with_port(remote, outcome.video_port),
                outcome.video_ping_payload,
            )
        }) {
            Ok(pinger) => parts.video_pinger = Some(pinger),
            Err(e) => listener.warn(&format!("Could not start the input-only video ping: {e}")),
        }
    } else {
        let mut decoder = video_decoder;
        let coded = if stream.experimental_packed_444 {
            rubylight_protocol::packed444::Geometry::new(stream.width as u32, stream.height as u32)
                .expect("validated packed geometry")
                .coded_size()
        } else {
            (stream.width as u32, stream.height as u32)
        };
        let setup = VideoSetup {
            format: outcome.negotiated_video_format,
            width: coded.0 as i32,
            height: coded.1 as i32,
            fps: stream.fps,
            chroma_packing: if stream.experimental_packed_444 {
                stream.packed444_version
            } else {
                0
            },
            logical_width: stream.width,
            logical_height: stream.height,
            color_space: stream.color_space,
            color_range: stream.color_range,
        };
        let video_fail = |code: i32, message: String| fail(Stage::VideoStreamStart, code, message);
        if let Err(code) = decoder.setup(&setup) {
            return Err(video_fail(code, "the video decoder setup failed".into()));
        }
        let socket = match net::bind_udp_sized(local, video_buffer, Qos::Video, config.network_priority, |m| {
            listener.warn(m)
        }) {
            Ok((socket, buffer)) => {
                if let Some(buffer) = buffer {
                    let text = buffer.describe("Video");
                    if buffer.short() { listener.warn(&text) } else { listener.log(&text) }
                }
                parts.video_receive_buffer = buffer;
                socket
            }
            Err(e) => {
                decoder.cleanup();
                return Err(video_fail(io_code(&e), e.to_string()));
            }
        };
        decoder.start();
        let start = video::VideoStart {
            socket,
            target: net::with_port(remote, outcome.video_port),
            ping_payload: outcome.video_ping_payload,
            direct_submit: outcome.video_capabilities & sdp::CAPABILITY_DIRECT_SUBMIT != 0,
            receive_coalescing: config.receive_coalescing,
            next_socket: parts.video_next_socket.clone(),
            fps: stream.fps,
        };
        parts.video = Some(
            video::VideoStream::start(
                start,
                decoder,
                video_shared,
                parts.shared.clone(),
                control.clone(),
            )
            .map_err(|e| video_fail(io_code(&e), e.to_string()))?,
        );
    }
    listener.stage_complete(Stage::VideoStreamStart);

    listener.stage_starting(Stage::AudioStreamStart);
    if !config.input_only {
        let mut renderer = audio_renderer;
        let codec = match outcome.audio_codec {
            0 => rubylight_protocol::audio_ext::CODEC_OPUS,
            codec => codec,
        };
        let duration = crate::audio::passthrough::packet_ms(
            codec,
            stream.audio.channel_count,
            u32::from(outcome.audio_packet_duration_ms),
        );
        parts.audio_shared.packet_ms.store(duration, Ordering::Relaxed);
        let opus = match (
            outcome.high_quality_surround_enabled,
            outcome.opus.high_quality,
        ) {
            (true, Some(high)) => high,
            _ => outcome.opus.normal,
        };
        let setup = AudioSetup {
            configuration: stream.audio,
            opus,
            codec,
            packet_duration_ms: duration,
            samples_per_frame: 48 * duration,
        };
        let audio_fail = |code: i32, message: String| fail(Stage::AudioStreamStart, code, message);
        if let Err(code) = renderer.init(&setup) {
            return Err(audio_fail(code, "the audio renderer init failed".into()));
        }
        renderer.start();
        let mut receiver = AudioReceiver::new(AudioConfig {
            packet_duration_ms: duration,
            app_version: outcome.app_version,
            encryption: outcome.audio_encryption_enabled.then_some(AudioEncryption {
                key: stream.remote_input_aes_key,
                iv: stream.remote_input_aes_iv,
            }),
        });
        if matches!(
            codec,
            rubylight_protocol::audio_ext::CODEC_AC3 | rubylight_protocol::audio_ext::CODEC_EAC3
        ) {
            receiver.assemble_frames();
        }
        let Some(socket) = parts.audio_socket.as_ref() else {
            parts.audio_renderer = Some(renderer);
            return Err(audio_fail(
                error::FAILED,
                "no audio port was negotiated".into(),
            ));
        };
        parts.audio = Some(
            audio::AudioStream::start(
                socket,
                parts.audio_next_socket.clone(),
                receiver,
                renderer,
                stream.audio_capabilities & sdp::CAPABILITY_DIRECT_SUBMIT != 0,
                parts.audio_shared.clone(),
                parts.shared.clone(),
            )
            .map_err(|e| audio_fail(io_code(&e), e.to_string()))?,
        );
    }
    listener.stage_complete(Stage::AudioStreamStart);

    listener.stage_starting(Stage::InputStreamStart);
    // The microphone comes up with the last stage; failing costs only the microphone.
    match microphone_wanted(
        outcome.mic_port,
        outcome.encryption_enabled,
        config.remote_monitor,
    ) {
        Ok(false) => {}
        Err(why) => listener.log(why),
        Ok(true) => {
            if let Some(socket) = open_microphone_socket(local, config.network_priority, listener) {
                *parts.microphone.lock().unwrap_or_else(|p| p.into_inner()) = Some(Microphone {
                    socket,
                    target: net::with_port(remote, outcome.mic_port),
                    cipher: aes::Aes128::new(&stream.remote_input_aes_key.into()),
                    key_id: u32::from_be_bytes(
                        stream.remote_input_aes_iv[..4].try_into().expect("4 bytes"),
                    ),
                    sequence: 0,
                    started: Instant::now(),
                    packet: [0u8; microphone::MAX_PACKET_BYTES],
                });
            }
        }
    }
    if control.roaming() {
        let video = parts
            .video
            .as_ref()
            .and_then(video::VideoStream::ping_handle)
            .or_else(|| parts.video_pinger.as_ref().map(net::Pinger::handle));
        let paths = vec![
            roam::MediaPath {
                qos: Qos::Video,
                buffer: video_buffer,
                ping: video,
                receive: parts.video.is_some().then(|| parts.video_next_socket.clone()),
            },
            roam::MediaPath {
                qos: Qos::Audio,
                buffer: 0,
                ping: parts.audio_pinger.as_ref().map(net::Pinger::handle),
                receive: parts.audio.is_some().then(|| parts.audio_next_socket.clone()),
            },
        ];
        let roamer = roam::Roamer::new(
            parts.shared.clone(),
            control.clone(),
            net::with_port(remote, rtsp_port),
            local,
            config.roaming,
            paths,
            parts.microphone.clone(),
        );
        parts.roam_poll = Some(
            roamer
                .start_poll()
                .map_err(|e| fail(Stage::InputStreamStart, io_code(&e), e.to_string()))?,
        );
        let _ = roamed_by.set(roamer.clone());
        parts.roamer = Some(roamer);
        listener.log("Roaming: on");
    } else if outcome.host_roaming && !config.roaming.enabled {
        listener.log("Roaming: off in the client's settings");
    }
    listener.stage_complete(Stage::InputStreamStart);
    parts.outcome = Some(outcome);

    // Wiggle the mouse to wake the host's display; a remote monitor sends no input at all.
    if !config.remote_monitor {
        let _ = control.with_input(|q| q.send_mouse_move(1, 1));
        std::thread::sleep(Duration::from_millis(10));
        let _ = control.with_input(|q| q.send_mouse_move(-1, -1));
        std::thread::sleep(Duration::from_millis(10));
    }

    listener.connection_started();
    Ok(())
}
