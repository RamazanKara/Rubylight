use crate::state::{Launch, Shared};
use anyhow::{Context, Result, bail};
use butterpollo_crypto::crypto;
use butterpollo_media_policy::rtsp::{self, Negotiated};
use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

/// Each RTSP connection carries one request: the client has this long from
/// connecting to send it, and as long again to take the response.
const HANDSHAKE_DEADLINE: Duration = Duration::from_secs(10);

#[derive(Debug)]
struct UnauthorizedPeer;
impl std::fmt::Display for UnauthorizedPeer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("no authorized RTSP launch")
    }
}
impl std::error::Error for UnauthorizedPeer {}

const MAX_CONNECTIONS: usize = 64;

pub async fn serve(address: SocketAddr, h: Shared, media: Arc<crate::stream::Media>) -> Result<()> {
    let listener = crate::network::tcp(address)?;
    let configurations = Arc::new(Mutex::new(HashMap::new()));
    let microphones = Arc::new(Mutex::new(HashSet::new()));
    let connections = Arc::new(tokio::sync::Semaphore::new(MAX_CONNECTIONS));
    let refused_warn = Arc::new(Mutex::new(crate::network::Throttle::new()));
    let unauthorized_warn = Arc::new(Mutex::new(crate::network::Throttle::new()));
    tracing::debug!(%address,"RTSP listener ready");
    loop {
        let (socket, peer) = crate::network::accept(&listener).await;
        let Ok(permit) = connections.clone().try_acquire_owned() else {
            if let Some(skipped) = refused_warn.lock().unwrap().allow() {
                tracing::warn!(%peer, skipped, "RTSP connection rejected: connection cap reached");
            }
            continue;
        };
        let accepted = tokio::time::Instant::now();
        let h = h.clone();
        let media = media.clone();
        let configs = configurations.clone();
        let mics = microphones.clone();
        let unauthorized_warn = unauthorized_warn.clone();
        tokio::spawn(async move {
            let _permit = permit;
            if let Err(e) = connection(socket, peer, accepted, h, media, configs, mics).await {
                if e.downcast_ref::<UnauthorizedPeer>().is_some() {
                    if let Some(skipped) = unauthorized_warn.lock().unwrap().allow() {
                        tracing::warn!(%peer, skipped, error = %format!("{e:#}"), "RTSP request rejected");
                    }
                    return;
                }
                // A client closing a connection early is routine; anything else
                // prevents a stream from starting and must be visible.
                let closed = e.downcast_ref::<std::io::Error>().is_some_and(|e| {
                    matches!(
                        e.kind(),
                        std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::ConnectionReset
                    )
                });
                if closed {
                    tracing::debug!(%peer, error = %e, "RTSP connection closed");
                } else {
                    tracing::warn!(%peer, error = %format!("{e:#}"), "RTSP request failed");
                }
            }
        });
    }
}
async fn read_message(socket: &mut TcpStream, launches: &[Launch]) -> Result<(Launch, Vec<u8>)> {
    let mut first = [0; 4];
    socket.read_exact(&mut first).await?;
    let word = u32::from_be_bytes(first);
    if word & 0x80000000 != 0 {
        let size = (word & 0x7fffffff) as usize;
        if size > rtsp::MAX_MESSAGE {
            bail!("encrypted RTSP message too large");
        }
        let mut header = [0; 20];
        socket.read_exact(&mut header).await?;
        let mut encrypted = vec![0; size];
        socket.read_exact(&mut encrypted).await?;
        let mut iv = [0; 12];
        iv[..4].copy_from_slice(&u32::from_be_bytes(header[..4].try_into().unwrap()).to_le_bytes());
        iv[10] = b'C';
        iv[11] = b'R';
        let sequence = u32::from_be_bytes(header[..4].try_into().unwrap());
        for launch in launches.iter().filter(|l| l.rtsp_encrypted) {
            if let Ok(raw) = crypto::gcm_open(&launch.key, &iv, &header[4..], &encrypted) {
                if !launch.rtsp_received.lock().unwrap().accept(sequence) {
                    bail!("replayed RTSP message");
                }
                return Ok((launch.clone(), raw));
            }
        }
        bail!("RTSP authentication failed");
    }
    if launches.len() != 1 {
        bail!("ambiguous legacy RTSP peer; use encrypted RTSP");
    }
    let launch = &launches[0];
    if launch.rtsp_encrypted {
        bail!("unencrypted RTSP message for encrypted launch");
    }
    let mut message = rtsp::MessageBuffer::default();
    let mut complete = message.append(&first)?;
    let mut chunk = [0; 4096];
    while !complete {
        let read = socket.read(&mut chunk).await?;
        if read == 0 {
            return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof).into());
        }
        complete = message.append(&chunk[..read])?;
    }
    Ok((launch.clone(), message.into_bytes()))
}
async fn connection(
    mut socket: TcpStream,
    peer: SocketAddr,
    accepted: tokio::time::Instant,
    h: Shared,
    media: Arc<crate::stream::Media>,
    configs: Arc<Mutex<HashMap<String, Negotiated>>>,
    // Launches that set up the microphone stream, until their ANNOUNCE.
    mics: Arc<Mutex<HashSet<String>>>,
) -> Result<()> {
    socket.set_nodelay(true)?;
    let (launches, expired) = {
        let mut sessions = h.sessions.lock().unwrap();
        let expired = sessions.expire();
        let launches = sessions.rtsp_for_peer(peer.ip());
        (launches, expired)
    };
    drop(expired);
    if launches.is_empty() {
        return Err(UnauthorizedPeer.into());
    }
    #[cfg(any(test, feature = "test-support"))]
    if let Some(fixture) = &h.reconnect_fixture {
        fixture.before_rtsp_cleanup().await;
    }
    {
        // A launch may arrive after the authorization snapshot above. Keep
        // the session map locked through pruning so an older connection
        // cannot erase its ANNOUNCE or microphone SETUP state.
        let sessions = h.sessions.lock().unwrap();
        let live =
            |id: &String| sessions.pending.contains_key(id) || sessions.active.contains_key(id);
        configs.lock().unwrap().retain(|id, _| live(id));
        mics.lock().unwrap().retain(live);
    }

    {
        let deadline = handshake_deadline(&h);
        let (launch, raw) =
            tokio::time::timeout_at(accepted + deadline, read_message(&mut socket, &launches))
                .await
                .map_err(|_| {
                    anyhow::anyhow!(
                        "the RTSP client sent no complete request within {} ms of connecting",
                        deadline.as_millis()
                    )
                })??;
        let req = rtsp::Request::parse(&raw)?;
        let began = std::time::Instant::now();
        h.request_codec_probe();
        let mut headers = vec![];
        let mut body = Vec::new();
        let mut code = 200;
        let mut reason = "OK";
        // DESCRIBE offers, and ANNOUNCE accepts, AC3 and E-AC3 only where FFmpeg's encoder
        // opens for the launch's channel count, and an ANNOUNCE's pick is opened for its own:
        // each probe loads FFmpeg (once per codec and channel count), off the async task. An
        // SDP that doesn't parse is refused below. Without passthrough nothing but Opus is
        // offered, which needs no FFmpeg. A probe that can't run (FFmpeg's folder unknown, or
        // the probe panicked) refuses the request with a 500, never by dropping the connection.
        let mut dolby = 0;
        let mut audio_check = None;
        let mut probe_failed = None;
        if matches!(req.method.as_str(), "DESCRIBE" | "ANNOUNCE") {
            use crate::stream::audio_encoder::{check_announce, dolby_codecs, launch_channels};
            let directory = ffmpeg_directory(&h);
            let channels = launch_channels(&launch.options);
            let passthrough = crate::stream::effective_config(&h, &launch)
                .is_ok_and(|config| config.audio_passthrough());
            let announced = (req.method == "ANNOUNCE")
                .then(|| Negotiated::from_sdp(&req.body).ok())
                .flatten();
            #[cfg(any(test, feature = "test-support"))]
            let panics = h
                .reconnect_fixture
                .as_ref()
                .is_some_and(|fixture| fixture.ffmpeg_probe_panics());
            let probe = tokio::task::spawn_blocking(move || -> Result<_> {
                #[cfg(any(test, feature = "test-support"))]
                if panics {
                    panic!("the test's FFmpeg probe panics");
                }
                if !passthrough {
                    // The ANNOUNCE's offer check refuses any codec but Opus first.
                    return Ok((0, announced.map(|_| Ok(()))));
                }
                let directory = directory?;
                Ok((
                    dolby_codecs(&directory, channels),
                    announced.map(|n| check_announce(&directory, &n)),
                ))
            })
            .await
            .context("FFmpeg's passthrough audio probe did not finish")
            .and_then(|probed| probed);
            match probe {
                Ok(probed) => (dolby, audio_check) = probed,
                Err(error) => {
                    tracing::error!(
                        client = %launch.client.name,
                        method = %req.method,
                        error = %format!("{error:#}"),
                        "passthrough audio could not be probed: the request is refused"
                    );
                    probe_failed = Some(error);
                }
            }
        }
        let host_fault = probe_failed.is_some();
        let result: Result<()> = async {
            if let Some(error) = probe_failed.take() {
                return Err(error);
            }
            // The launch may have been replaced, started or expired since
            // the snapshot above; a live one is claimed by this request.
            h.sessions.lock().unwrap().claim(&launch.id)?;
            let ports = h.config.read().unwrap().ports()?;
            match req.method.as_str() {
                "OPTIONS" => headers.push((
                    "Public",
                    "OPTIONS, DESCRIBE, SETUP, ANNOUNCE, PLAY, TEARDOWN".to_owned(),
                )),
                "DESCRIBE" => {
                    let flags = h.codecs.load(std::sync::atomic::Ordering::Acquire);
                    let config = crate::stream::effective_config(&h, &launch)?;
                    let encryption_mode = crate::network::encryption_mode(&config, peer);
                    let input_capabilities = butterpollo_win_io::input::capabilities(&config);
                    if config.boolean("mouse", true)
                        && config.boolean("native_pen_touch", true)
                        && input_capabilities & 1 == 0
                    {
                        launch.warnings.set("input_touch_pen", "Native touch and pen unavailable: Windows does not expose synthetic pointer input. The client may emulate a mouse instead; update Windows or disable native touch/pen in Input settings.");
                    }
                    // Video encryption is offered unless turned off for this
                    // network, and audio and video are required where mandatory.
                    let mic = takes_mic(&config, &media);
                    // Probes are answered only for sessions offered roaming here.
                    launch.roaming_offered.store(
                        config.client_roaming(),
                        std::sync::atomic::Ordering::Release,
                    );
                    body = rtsp::describe_with_link_notice(
                        input_capabilities,
                        if encryption_mode == 0 { 5 } else { 7 },
                        if encryption_mode == 2 { 7 } else { 1 },
                        flags & 0x100 != 0,
                        flags & 0x10000 != 0,
                        flags & 0x800000 != 0,
                        mic,
                        config.client_roaming(),
                        offers_link_notice(&config),
                    )
                    .into_bytes();
                    // AMF recovers lost references unless long-term references
                    // are turned off; NVENC invalidates them natively when the
                    // probe found the driver can (flag 0x40000000).
                    let backend = *h.probed_encoder.lock().unwrap();
                    if flags & 0x40000000 != 0
                        && (backend == "nvenc"
                            || (backend == "amf"
                                && butterpollo_media_policy::encoder_policy::amf_offers_invalidation(
                                    &config,
                                )))
                    {
                        body.extend_from_slice(b"a=x-nv-video[0].refPicInvalidation:1\r\n");
                    }
                    if let Some(custom) = launch.options.get("surroundParams")
                        && butterpollo_media_policy::audio::OpusLayout::valid_custom(custom)
                    {
                        let at = body
                            .windows(9)
                            .position(|b| b == b"a=fmtp:97")
                            .unwrap_or(body.len());
                        body.splice(at..at, format!("a=fmtp:97 surround-params={custom}\r\na=fmtp:97 surround-params={custom}\r\n").bytes());
                    }
                    // Last: the lines after an m= line describe its stream.
                    if config.packed_444() {
                        let modes = h.packed_modes.load(std::sync::atomic::Ordering::Acquire);
                        if modes != 0 {
                            use rubylight_protocol::packed444;
                            body.extend_from_slice(
                                format!(
                                    "a={}:{}\r\na={}:{}\r\na={}:{}\r\n",
                                    packed444::VERSION_ATTRIBUTE,
                                    packed444::VERSION,
                                    packed444::MODES_ATTRIBUTE,
                                    modes,
                                    packed444::LAYERED_MODES_ATTRIBUTE,
                                    modes & packed444::LAYERED_MODES
                                )
                                .as_bytes(),
                            );
                        }
                    }
                    body.extend_from_slice(
                        crate::extensions::stream_offered(&h, &config, dolby, &launch)
                            .describe_lines()
                            .as_bytes(),
                    );
                    if mic {
                        body.extend_from_slice(butterpollo_session::mic::sdp(ports.mic).as_bytes());
                    }
                    headers.push(("Content-Type", "application/sdp".into()));
                }
                "SETUP" => {
                    let mut mic_setup = false;
                    let port = if req.target.contains("=audio") {
                        ports.audio
                    } else if req.target.contains("=video") {
                        ports.video
                    } else if req.target.contains("=control") {
                        ports.control
                    } else if req.target.contains("=mic")
                        && takes_mic(&crate::stream::effective_config(&h, &launch)?, &media)
                    {
                        mics.lock().unwrap().insert(launch.id.clone());
                        mic_setup = true;
                        ports.mic
                    } else {
                        bail!("unknown stream setup target")
                    };
                    headers.push(("Session", "DEADBEEFCAFE;timeout = 90".into()));
                    headers.push(("Transport", format!("server_port={port}")));
                    if mic_setup {
                        headers.push((
                            rubylight_protocol::mic::FRAMING_HEADER,
                            rubylight_protocol::mic::FRAMING_SINGLE_PKCS7.to_string(),
                        ));
                    }
                    if port == ports.control {
                        headers.push(("X-SS-Connect-Data", launch.connect_data.to_string()));
                    } else {
                        headers.push(("X-SS-Ping-Payload", launch.ping.clone()));
                    }
                }
                "ANNOUNCE" => {
                    let mut negotiated = Negotiated::from_sdp(&req.body)?;
                    negotiated.apply_launch_options(&launch.options);
                    if negotiated.audio_channels == 2
                        && let Some(host) = req.headers.get("host")
                    {
                        negotiated.audio_quality = !host.contains("0.0.0.0");
                    }
                    let config = crate::stream::effective_config(&h, &launch)?;
                    // A host-wide cap (`max_stream_resolution`) codes a smaller
                    // picture than the client asked for; the stream's display
                    // keeps the size it asked for. The device and console say why.
                    crate::stream::cap_stream_resolution(
                        &mut negotiated,
                        &config,
                        &launch.warnings,
                    );
                    // The client's own picks, before host policy turns HDR off.
                    crate::extensions::stream_offered(&h, &config, dolby, &launch)
                        .check_announce(&negotiated)?;
                    audio_check
                        .take()
                        .context("the ANNOUNCE's audio codec was not checked")??;
                    let encryption_mode = crate::network::encryption_mode(&config, peer);
                    let required_encryption = encryption_mode == 2;
                    // The FEC and audio share of the client's bitrate is expected;
                    // only a host cap is worth a warning.
                    let uncapped = butterpollo_media_policy::stream_policy::uncapped_bitrate_kbps(
                        &negotiated,
                        launch.requested_rate,
                        &config,
                    );
                    butterpollo_media_policy::stream_policy::apply(
                        &mut negotiated,
                        launch.requested_rate,
                        &config,
                    );
                    butterpollo_media_policy::stream_policy::report_bitrate(
                        &launch.warnings,
                        uncapped,
                        negotiated.bitrate_kbps,
                        &format!(
                            "Maximum bitrate (max_bitrate) is {} Kbps",
                            config.integer("max_bitrate", 0)
                        ),
                    );
                    butterpollo_media_policy::stream_policy::report_packet_size(&launch.warnings, &config);
                    // A ten-bit format with `hdrMode=0` is ten-bit SDR, packed 4:4:4
                    // included: its encoder codes SDR ten-bit either way and its
                    // startup probe asks for `sdr_10bit`.
                    if butterpollo_media_policy::stream_policy::infer_sdr_10bit(
                        &mut negotiated,
                        launch.ten_bit_sdr_requested(),
                    ) {
                        tracing::info!(
                            client = %launch.client.name,
                            codec = negotiated.codec,
                            packed = negotiated.chroma_packing != 0,
                            "ten-bit format requested with hdrMode=0; streaming ten-bit SDR"
                        );
                    }
                    if negotiated.chroma_packing == 0 {
                        butterpollo_media_policy::stream_policy::apply_color(&mut negotiated, &config);
                    }
                    butterpollo_media_policy::stream_policy::packed_drops_dolby_vision(
                        &mut negotiated,
                        &launch.warnings,
                    );
                    butterpollo_media_policy::stream_policy::packed_drops_hlg(
                        &mut negotiated,
                        &launch.warnings,
                    );
                    negotiated.validate()?;
                    negotiated.mic = mics.lock().unwrap().contains(&launch.id);
                    if negotiated.mic
                        && negotiated.encryption & butterpollo_session::mic::ENCRYPTION == 0
                    {
                        // As in Apollo: a microphone is never taken in the clear.
                        negotiated.mic = false;
                        launch.warnings.set("microphone", "Microphone off: the client sent it unencrypted. Update the client to one that encrypts its microphone.");
                    }
                    if !negotiated.mic {
                        negotiated.mic_single_pkcs7 = false;
                    }
                    negotiated.vrr_low_latency |= launch.vrr_requested;
                    let flags = h.codecs.load(std::sync::atomic::Ordering::Acquire);
                    if let Some(message) = rtsp::codec_warning(
                        config.get("encoder", "auto"),
                        negotiated.codec,
                        flags & 0x0780_0000 != 0,
                    ) {
                        launch.warnings.set("pyrowave_negotiation", message);
                    } else {
                        launch.warnings.clear("pyrowave_negotiation");
                    }
                    if negotiated.codec == 3 && !negotiated.pyrowave_records {
                        launch.warnings.set("pyrowave_fec", "PyroWave adaptive FEC was not negotiated by the client; recovery protection is reduced. Use a compatible Nonary Moonlight client, or HEVC/AV1 on a lossy network.");
                    }
                    let bit = match (negotiated.codec, negotiated.ten_bit()) {
                        (0, false) if negotiated.yuv444 => 0x40000,
                        (1, false) if negotiated.yuv444 => 0x80000,
                        (1, true) if negotiated.yuv444 => 0x100000,
                        (2, false) if negotiated.yuv444 => 0x200000,
                        (2, true) if negotiated.yuv444 => 0x400000,
                        (0, false) => 1,
                        (1, false) => 0x100,
                        (1, true) => 0x200,
                        (2, false) => 0x10000,
                        (2, true) => 0x20000,
                        (3, false) if negotiated.yuv444 => 0x1000000,
                        (3, true) if negotiated.yuv444 => 0x4000000,
                        (3, true) => 0x2000000,
                        (3, false) => 0x800000,
                        _ => 0,
                    };
                    match crate::network::unencrypted_internet_warning(
                        encryption_mode,
                        peer,
                        negotiated.encryption,
                    ) {
                        Some(message) => launch.warnings.set("stream_unencrypted", message),
                        None => launch.warnings.clear("stream_unencrypted"),
                    }
                    if required_encryption && negotiated.encryption & 6 != 6 {
                        code = 403;
                        reason = "Required audio and video encryption was not negotiated";
                        // The device reads why from `/serverinfo`, as for a refused launch.
                        h.record_launch_failure(&launch.client.uuid, &encryption_refusal(peer));
                    } else if (negotiated.ten_bit() && negotiated.codec == 0) || flags & bit == 0 {
                        code = 406;
                        reason = "Requested codec is unavailable";
                        // The RTSP reason line is fixed text: put the cause in
                        // the log and in the launch warning the console shows.
                        let detail = butterpollo_media_policy::codec_support::refusal_for(
                            &h.codec_missing.lock().unwrap(),
                            negotiated.codec,
                            negotiated.ten_bit(),
                            negotiated.yuv444,
                        );
                        tracing::warn!(codec = negotiated.codec, ten_bit = negotiated.ten_bit(), %detail, "refusing stream: requested codec is unavailable");
                        h.record_launch_failure(&launch.client.uuid, &detail);
                        note_codec_refusal(&launch.warnings, Some(detail));
                    } else {
                        if negotiated.chroma_packing != 0 {
                            anyhow::ensure!(
                                config.packed_444(),
                                "experimental packed 4:4:4 is disabled on the host"
                            );
                            let mode = rubylight_protocol::packed444::mode_bit(
                                negotiated.codec,
                                negotiated.ten_bit(),
                            );
                            anyhow::ensure!(
                                h.packed_modes.load(std::sync::atomic::Ordering::Acquire) & mode
                                    != 0,
                                "requested packed codec or bit depth failed the host hardware probe"
                            );
                            // Initialize the requested coded geometry and rate before accepting
                            // PLAY, serialized against optional probes and active video streams.
                            let probe_host = h.clone();
                            let probe_config = config.clone();
                            let probe_negotiated = negotiated.clone();
                            tokio::task::spawn_blocking(move || {
                                probe_host.probe_packed_for_announce(
                                    &probe_config,
                                    &probe_negotiated,
                                    |cfg, neg, cancel| {
                                        butterpollo_win_graphics::codec_probe::packed_with(
                                            cfg,
                                            neg,
                                            cancel,
                                            &crate::win_bridge::HostUserLauncher,
                                        )
                                    },
                                )
                            })
                            .await
                            .context("packed 4:4:4 probe did not finish")?
                            .with_context(|| {
                                format!(
                                    "packed 4:4:4 probe failed for {}x{} at {} fps, {}-bit {}",
                                    negotiated.width,
                                    negotiated.height,
                                    negotiated.fps,
                                    if negotiated.ten_bit() { 10 } else { 8 },
                                    if negotiated.codec == 2 { "AV1" } else { "HEVC" },
                                )
                            })?;
                        }
                        // A retry that asks for an available codec drops the
                        // refusal the earlier attempt left on this launch.
                        note_codec_refusal(&launch.warnings, None);
                        note_pyrowave_minimum(&launch.warnings, &negotiated);
                        h.clear_failure(&launch.client.uuid);
                        mics.lock().unwrap().remove(&launch.id);
                        configs
                            .lock()
                            .unwrap()
                            .insert(launch.id.clone(), negotiated);
                    }
                }
                // A client that plays each stream on its own plays the
                // microphone after the session has started.
                "PLAY" if req.target.contains("streamid=mic") => {}
                "PLAY" => {
                    let config = configs
                        .lock()
                        .unwrap()
                        .remove(&launch.id)
                        .context("ANNOUNCE required before PLAY")?;
                    if let Err(error) = crate::nvhttp::check_streaming_session(&h, "rtsp_play") {
                        let canceled = h.sessions.lock().unwrap().pending.remove(&launch.id);
                        mics.lock().unwrap().remove(&launch.id);
                        // Release only this pending attempt, outside session
                        // locks. The app accepted by HTTP remains resumable;
                        // other streams and app-scoped leases keep ownership.
                        drop(canceled);
                        return Err(error);
                    }
                    let session = h.sessions.lock().unwrap().start(launch.clone(), config)?;
                    media.start(h.clone(), session);
                }
                "TEARDOWN" => {
                    let canceled = h.sessions.lock().unwrap().request_stop(Some(&launch.id));
                    drop(canceled);
                }
                _ => {
                    code = 405;
                    reason = "Method Not Allowed";
                }
            }
            Ok(())
        }
        .await;
        tracing::info!(method = %req.method, ok = result.is_ok(), ms = began.elapsed().as_millis() as u64, "RTSP request timing");
        if let Err(error) = result {
            tracing::warn!(client = %launch.client.name, method = %req.method, error = %format!("{error:#}"), "RTSP request rejected");
            let unoffered_mic = req.method == "SETUP" && req.target.contains("=mic");
            let duplicate_play =
                req.method == "PLAY" && h.sessions.lock().unwrap().active.contains_key(&launch.id);
            if !unoffered_mic && !duplicate_play {
                h.record_launch_failure(&launch.client.uuid, &format!("{error:#}"));
            }
            (code, reason) = if host_fault {
                (500, "Internal Server Error")
            } else {
                (400, "Bad Request")
            };
            headers.clear();
            body = reason.as_bytes().to_vec();
        }
        if code != 200 {
            tracing::warn!(
                client = %launch.client.name,
                method = %req.method,
                code,
                reason,
                "RTSP request refused"
            );
        }
        let mut response = rtsp::response(req.cseq, code, reason, &headers, &body);
        if launch.rtsp_encrypted {
            let mut iv = [0; 12];
            let counter = launch
                .rtsp_counter
                .fetch_update(
                    std::sync::atomic::Ordering::AcqRel,
                    std::sync::atomic::Ordering::Acquire,
                    |n| n.checked_add(1),
                )
                .map_err(|_| anyhow::anyhow!("RTSP nonce exhausted"))?;
            iv[..4].copy_from_slice(&counter.to_le_bytes());
            iv[10] = b'H';
            iv[11] = b'R';
            let (tag, b) = crypto::gcm_seal(&launch.key, &iv, &response)?;
            response = ((b.len() as u32) | 0x80000000).to_be_bytes().to_vec();
            response.extend_from_slice(&counter.to_be_bytes());
            response.extend_from_slice(&tag);
            response.extend_from_slice(&b);
        }
        tokio::time::timeout(deadline, async {
            socket.write_all(&response).await?;
            socket.flush().await
        })
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "the RTSP client took no {} response within {} ms",
                req.method,
                deadline.as_millis()
            )
        })??;
        // Moonlight reads each response to EOF and opens a fresh TCP connection
        // for the next request. Close after the complete authenticated response.
        Ok(())
    }
}
/// [`HANDSHAKE_DEADLINE`], or the reconnect matrix's shorter one.
fn handshake_deadline(h: &Shared) -> Duration {
    #[cfg(any(test, feature = "test-support"))]
    if let Some(deadline) = h.reconnect_fixture.as_ref().and_then(|f| f.rtsp_deadline()) {
        return deadline;
    }
    let _ = h;
    HANDSHAKE_DEADLINE
}
/// The folder FFmpeg's libraries load from, or the reconnect matrix's failure to find it.
fn ffmpeg_directory(h: &Shared) -> Result<std::path::PathBuf> {
    #[cfg(any(test, feature = "test-support"))]
    if h.reconnect_fixture
        .as_ref()
        .is_some_and(|f| f.ffmpeg_directory_unknown())
    {
        bail!("the test host's folder is unknown");
    }
    let _ = h;
    crate::stream::audio_encoder::ffmpeg_directory()
}
/// Whether the host's `DESCRIBE` advertises `0x5538` link notices: only when at least one
/// host bitrate controller (`link_rate_adaptation` or `congestion_control`) is enabled.
pub(crate) fn offers_link_notice(config: &butterpollo_config::config::Config) -> bool {
    config.link_rate_adaptation() || config.congestion_control()
}
/// Whether the host takes client microphones on this stream.
fn takes_mic(config: &butterpollo_config::config::Config, media: &crate::stream::Media) -> bool {
    config.boolean("stream_mic", true) && media.mic.is_some()
}
/// Why a stream that must be encrypted was refused, as the device shows it: the words of the
/// same refusal at launch, for a device that asked for encryption there but not at ANNOUNCE.
fn encryption_refusal(peer: SocketAddr) -> String {
    let (network, setting) = if crate::network::peer_reach(peer) <= crate::network::Reach::Lan {
        ("the local network", "Local network encryption")
    } else {
        ("the internet", "Internet encryption")
    };
    format!(
        "this PC requires encrypted streams on {network} and this device did not turn encryption on: update its Moonlight app, or set {setting} to When the device supports it"
    )
}
/// Show why a launch was refused for its codec, or drop that refusal once a
/// later attempt on the same launch asks for something the host has.
fn note_codec_refusal(warnings: &butterpollo_session::session::Warnings, refusal: Option<String>) {
    match refusal {
        Some(detail) => warnings.set("codec_unavailable", detail),
        None => warnings.clear("codec_unavailable"),
    }
}
/// Notes a PyroWave stream below the bitrate where the project's test scenes kept detail: the
/// console card and the log say so, and the stream still starts. The floor is the same function
/// link-rate adaptation stops at, so the two agree, and a client asking exactly the floor is not
/// below it just because the picture gives up a few Mbps to sound and control.
pub(crate) fn note_pyrowave_minimum(
    warnings: &butterpollo_session::session::Warnings,
    stream: &Negotiated,
) {
    if stream.codec != 3 {
        warnings.clear("pyrowave_minimum");
        return;
    }
    let floor = butterpollo_media_policy::link_rate::pyrowave_floor_kbps(stream);
    let bitrate = stream.bitrate_kbps;
    if bitrate >= floor {
        warnings.clear("pyrowave_minimum");
        return;
    }
    let minimum = butterpollo_media_policy::pyrowave::minimum_kbps(
        stream.width,
        stream.height,
        stream.fps.saturating_mul(1000),
    );
    let detail = format!(
        "PyroWave at {}x{} at {} fps is below the level where the project's test scenes kept detail: {} Mbps or more, and this stream gets {} Mbps. Text and textures may lose detail; raise the client bitrate with network headroom, lower the resolution or frame rate, or use HEVC or AV1.",
        stream.width,
        stream.height,
        stream.fps,
        minimum.div_ceil(1000),
        bitrate.div_ceil(1000)
    );
    tracing::warn!(
        bitrate_kbps = bitrate,
        floor_kbps = floor,
        minimum_kbps = minimum,
        "PyroWave bitrate is below the level where the project's test scenes kept detail"
    );
    warnings.set("pyrowave_minimum", detail);
}

#[cfg(test)]
mod tests;
