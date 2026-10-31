//! The audio thread: capture, route upkeep, encode and send.
use super::*;

/// The Opus layout a stream encodes: the one a WebRTC answer agreed to, or for GameStream the one
/// the launch asked for.
pub(crate) fn audio_layout(
    s: &Session,
    negotiated: Option<&butterpollo_core::audio::OpusLayout>,
) -> Result<butterpollo_core::audio::OpusLayout> {
    match negotiated {
        Some(layout) => Ok(layout.clone()),
        None => butterpollo_core::audio::OpusLayout::select(
            s.config.audio_channels as usize,
            s.config.audio_quality,
            s.launch.options.get("surroundParams").map(String::as_str),
        ),
    }
}

/// The audio thread's encoder (`libopus-0.dll` from `directory`) for [`audio_layout`].
pub(crate) fn open_audio_encoder(
    directory: &std::path::Path,
    s: &Session,
    negotiated: Option<&butterpollo_core::audio::OpusLayout>,
) -> Result<(Opus, butterpollo_core::audio::OpusLayout)> {
    let layout = audio_layout(s, negotiated)?;
    let opus = Opus::new_layout_duration(directory, &layout, s.config.audio_packet_ms)?;
    Ok((opus, layout))
}

type Route = butterpollo_windows::audio_route::Route;

/// Keep the streaming speakers the Windows default and find the device to
/// capture; the previous sink when Windows cannot say.
fn route_upkeep(
    route: &Route,
    config: &Config,
    warnings: &butterpollo_core::session::Warnings,
    previous: &str,
) -> String {
    match route.maintain_default() {
        Ok(()) => warnings.clear("audio_default"),
        Err(error) => warnings.set("audio_default", format!("Could not keep the streaming playback device as default ({error:#}); game audio may go to another device. Check the Windows default playback device.")),
    }
    match route.capture_sink(config) {
        Ok(selected) => {
            warnings.clear("audio_device_query");
            selected
        }
        Err(error) => {
            warnings.set("audio_device_query", format!("Could not find the current audio capture device ({error:#}); retaining the previous route. Check the Windows default playback device if sound is missing."));
            previous.to_owned()
        }
    }
}

/// The audio route's once-a-second upkeep, on its own thread. Each round
/// asks the Windows audio service for the default devices, several COM calls;
/// on the audio sender they held up the read after them about once a second.
struct RouteUpkeep {
    sink: Arc<Mutex<String>>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl RouteUpkeep {
    /// The first round runs on the caller, so capture opens on the right sink.
    fn start(
        route: Arc<Route>,
        config: Config,
        warnings: Arc<butterpollo_core::session::Warnings>,
    ) -> Result<Self> {
        let sink = Arc::new(Mutex::new(route_upkeep(
            &route,
            &config,
            &warnings,
            &route.sink,
        )));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = thread::Builder::new().name("audio route".into()).spawn({
            let sink = sink.clone();
            let stop = stop.clone();
            move || {
                let _com = match ComGuard::new() {
                    Ok(com) => com,
                    Err(error) => {
                        tracing::warn!(%error, "audio route upkeep unavailable");
                        return;
                    }
                };
                loop {
                    thread::park_timeout(Duration::from_secs(1));
                    if stop.load(Ordering::Acquire) {
                        break;
                    }
                    let previous = sink.lock().unwrap().clone();
                    let selected = route_upkeep(&route, &config, &warnings, &previous);
                    *sink.lock().unwrap() = selected;
                }
            }
        })?;
        Ok(Self {
            sink,
            stop,
            thread: Some(thread),
        })
    }
    fn sink(&self) -> String {
        self.sink.lock().unwrap().clone()
    }
}

impl Drop for RouteUpkeep {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.thread.take() {
            worker.thread().unpark();
            finish_worker(worker, "audio route");
        }
    }
}

impl Media {
    pub(super) fn spawn_audio(
        self: &Arc<Self>,
        h: &Shared,
        s: &Arc<Session>,
    ) -> Result<thread::JoinHandle<()>> {
        // The video worker takes the media link later, so the audio thread gets its own handle on
        // the audio side now. A WebRTC session whose answer left no layout cannot stream sound.
        let webrtc = if s.launch.transport == butterpollo_core::session::Transport::WebRtc {
            Some(
                h.webrtc_links
                    .audio_sender(&s.launch.id)
                    .context("the WebRTC audio link is missing for this session")?,
            )
        } else {
            None
        };
        let audio_m = self.clone();
        let audio_h = h.clone();
        let audio_s = s.clone();
        Ok(thread::Builder::new().name("audio".into()).spawn(move || {
            if let Err(e) = audio_m.audio(audio_h, audio_s.clone(), webrtc) {
                audio_s.launch.warnings.set("audio_stopped", format!("Audio stopped ({e:#}); video is still running without sound. Check the playback device and network, then reconnect."));
            }
        })?)
    }
    fn audio(
        &self,
        h: Shared,
        s: Arc<Session>,
        webrtc: Option<crate::webrtc::media::AudioSender>,
    ) -> Result<()> {
        let _com = ComGuard::new()?;
        let _priority = Priority::input();
        let config = effective_config(&h, &s.launch)?;
        let game_audio = config.get("audio_capture_mode", "system") == "game";
        anyhow::ensure!(
            matches!(
                config.get("audio_capture_mode", "system"),
                "system" | "game"
            ),
            "unknown audio capture mode; choose System audio or Game only in Audio settings"
        );
        let muted = !config.boolean("stream_audio", true)
            || (s.launch.role == Role::RemoteMonitor
                && config.boolean("remote_monitor_mute_audio", false));
        // Night mode and headphone surround; with both off the packets pass untouched.
        let mut couch =
            couch_audio::CouchAudio::start(&s, &config, &h.audio_memory, game_audio, muted)?;
        let capture_channels = couch.capture_channels();
        let mut route = s
            .launch
            .audio_preparation
            .lock()
            .unwrap()
            .as_ref()
            .map(|route| route.route());
        let mut game_capture = if game_audio && !muted {
            Some(game_audio::GameAudio::start(h.clone(), s.clone())?)
        } else {
            None
        };
        let mut capture: Option<Loopback> = None;
        let mut sink = String::new();
        let mut audio_check = Instant::now();
        let mut audio_error: Option<String> = None;
        let directory = audio_encoder::ffmpeg_directory()?;
        // A GameStream client may pick PCM, AC3 or E-AC3 in its ANNOUNCE; WebRTC is Opus.
        let codec = match webrtc {
            Some(_) => Codec::Opus,
            None => Codec::from_announce(s.config.audio_codec, s.config.audio_channels)?,
        };
        let packet_ms = codec.packet_ms(s.config.audio_channels, s.config.audio_packet_ms)?;
        let mut encoder = match codec {
            Codec::Opus => AudioEncoder::Opus(
                open_audio_encoder(
                    &directory,
                    &s,
                    webrtc
                        .as_ref()
                        .map(crate::webrtc::media::AudioSender::layout),
                )?
                .0,
            ),
            codec => {
                tracing::info!(
                    codec = codec.name(),
                    channels = s.config.audio_channels,
                    packet_ms,
                    "audio passthrough"
                );
                AudioEncoder::passthrough(&directory, codec, s.config.audio_channels)?
            }
        };
        let mut output: Box<dyn AudioSink + '_> = match webrtc {
            Some(sender) => Box::new(WebRtcAudio(sender)),
            None => Box::new(GameStreamAudio::new(
                &self.audio,
                &self.peers,
                &s.launch.id,
                AudioPacketizer::new(
                    s.launch.key,
                    s.launch.key_id,
                    s.config.encryption & 4 != 0,
                    u32::from(packet_ms),
                ),
                s.config.audio_qos,
            )),
        };
        let frames = 48 * usize::from(packet_ms);
        let client_channels = usize::from(s.config.audio_channels);
        let start = Instant::now();
        let interval = Duration::from_millis(u64::from(packet_ms));
        let mut loss = butterpollo_core::audio::HostLoss::default();
        // Packet deadlines, silence for a quiet endpoint, and where each packet sits in the
        // stream in 48 kHz samples; the WebRTC RTP clock follows that, not wall time.
        let mut pace = AudioPace::new(Instant::now(), interval, frames);
        let mut last_loss = None;
        let mut upkeep: Option<RouteUpkeep> = None;
        let timer = butterpollo_windows::timing::Timer::new()?;
        let silence = couch.silence(frames);
        while !s.stopping() && !h.stop.load(Ordering::Acquire) {
            match capture
                .as_ref()
                .filter(|capturing| capturing.event_driven())
            {
                // Audio events wake immediately. Before the silence threshold,
                // an expired packet deadline must not turn this into a 1 ms poll.
                Some(capturing) => capturing.wait(
                    pace.due()
                        .saturating_duration_since(Instant::now())
                        .clamp(Duration::from_millis(1), Duration::from_millis(5)),
                ),
                None => timer.until(Instant::now() + Duration::from_millis(1)),
            }
            #[cfg(debug_assertions)]
            if let Some(stall) = crate::soak_fault::audio_stall()? {
                thread::sleep(stall);
            }
            if let Some(game_capture) = &mut game_capture {
                game_capture.update(&s, &mut capture, &mut |mut old| {
                    let waited = old.since_drained();
                    audio_sink::retire(&mut old, waited, &mut loss, &mut pace, Instant::now());
                });
            }
            if !muted && !game_audio && Instant::now() >= audio_check {
                audio_check = Instant::now() + Duration::from_secs(1);
                if route.is_none() {
                    match butterpollo_windows::audio_route::Route::acquire(
                        &config,
                        &h.directory,
                        s.launch.host_audio,
                        capture_channels,
                    ) {
                        Ok(value) => {
                            let value = Arc::new(value);
                            if s.launch.role == Role::Stream
                                && h.current_app.lock().unwrap().is_some()
                            {
                                *h.app_audio.lock().unwrap() = Some(value.clone());
                            }
                            route = Some(value);
                        }
                        Err(error) => {
                            let detail = format!("{error:#}");
                            if audio_error.as_ref() != Some(&detail) {
                                s.launch.warnings.set("audio_route", format!("Audio routing failed ({detail}); sending silence while retrying. Select an available playback or virtual audio device in Audio settings."));
                                audio_error = Some(detail);
                            }
                        }
                    }
                }
                if let Some(route) = &route {
                    s.launch.warnings.clear("audio_route");
                    if let Some(message) = route.warning(s.launch.host_audio) {
                        s.launch.warnings.set("audio_virtual_sink", message);
                    } else {
                        s.launch.warnings.clear("audio_virtual_sink");
                    }
                    let selected = match &upkeep {
                        Some(upkeep) => upkeep.sink(),
                        None => upkeep
                            .insert(RouteUpkeep::start(
                                route.clone(),
                                config.clone(),
                                s.launch.warnings.clone(),
                            )?)
                            .sink(),
                    };
                    if selected != sink {
                        if !sink.is_empty() {
                            s.launch.warnings.event("audio_device_changed", "Audio capture device changed because the Windows default playback device changed. WASAPI is reopening and sound may briefly pause; select a fixed capture sink if the change was unintended.", butterpollo_core::session::EVENT_PERIOD);
                        }
                        if let Some(mut old) = capture.take() {
                            let waited = old.since_drained();
                            audio_sink::retire(
                                &mut old,
                                waited,
                                &mut loss,
                                &mut pace,
                                Instant::now(),
                            );
                        }
                        sink = selected;
                    }
                    // Pinning the sink only disables following the Windows
                    // default. Even a fixed endpoint must reopen after a
                    // driver reset, format change or temporary disconnect.
                    if capture.is_none() {
                        match route.set_channels(capture_channels) {
                            Ok(()) => s.launch.warnings.clear("audio_surround"),
                            Err(error) => s.launch.warnings.set("audio_surround", format!("Virtual surround format could not be applied ({error:#}); channels may be downmixed. Select stereo or check the virtual audio driver.")),
                        }
                        match Loopback::new_sink(capture_channels, &sink) {
                            Ok(value) => {
                                tracing::info!(
                                    channels = capture_channels,
                                    source_channels = value.source().0,
                                    event_driven = value.event_driven(),
                                    "WASAPI audio capture started"
                                );
                                capture = Some(value);
                                s.launch.warnings.clear("audio_capture");
                                audio_error = None;
                            }
                            Err(error) => {
                                let detail = format!("{error:#}");
                                if audio_error.as_ref() != Some(&detail) {
                                    s.launch.warnings.set("audio_capture", format!("Audio capture failed ({detail}); sending silence while retrying. Check that the playback device is available and reconnect if it does not recover."));
                                    audio_error = Some(detail);
                                }
                            }
                        }
                    }
                }
            }
            if let Some(capturing) = &capture {
                couch.set_source(capturing.source())?;
            }
            let ready = output.ready();
            // Every whole packet the endpoint has delivered goes out at once, as
            // Sunshine sends them. On a fixed tick each waited up to a packet,
            // and a tick just before a late chunk sent silence in its place.
            let (mut packets, positions) = audio_sink::collect(
                &mut capture,
                frames,
                &mut loss,
                &mut pace,
                &silence,
                Instant::now,
                |error| {
                    let recovery = if game_audio {
                        "sending silence until a verified game process can be captured again. No system audio is captured. Check the game's launch entry if this repeats."
                    } else {
                        "reopening WASAPI and sending silence until it recovers. Check the playback device if this repeats."
                    };
                    s.launch.warnings.set(
                        "audio_capture",
                        format!("Audio capture interrupted ({error:#}); {recovery}"),
                    );
                },
            );
            let now = Instant::now();
            if ready {
                for (samples, at) in packets.iter_mut().zip(positions) {
                    couch.process(&s.audio, &s.launch.warnings, samples);
                    for _ in 0..encoder.send(output.as_mut(), samples, now, at)? {
                        loss.unsent();
                    }
                    // Haptics from game audio analyse a copy of what went out,
                    // on the control thread; one atomic load while unused.
                    s.audio_haptics.push(samples, client_channels);
                }
            } else if audio_sink::ping_timed_out(
                &s,
                &h.stop,
                start,
                crate::network::ping_timeout(&config),
            ) {
                anyhow::bail!("client audio ping timed out");
            }
            let now = Instant::now();
            if let Some(report) = loss.report(now) {
                last_loss = Some(now);
                s.launch.warnings.set("audio_loss", report.message());
            } else if last_loss.is_some_and(|at| now.duration_since(at) >= Duration::from_secs(10))
            {
                s.launch.warnings.clear("audio_loss");
                last_loss = None;
            }
        }
        Ok(())
    }
}
