//! The audio thread: capture, route upkeep, encode and send.
use super::audio_encoder::AudioEncoder;
use super::*;

/// The Opus layout a stream encodes: the one a WebRTC answer agreed to, or for GameStream the one
/// the launch asked for.
pub(crate) fn audio_layout(
    s: &Session,
    negotiated: Option<&butterpollo_media_policy::audio::OpusLayout>,
) -> Result<butterpollo_media_policy::audio::OpusLayout> {
    match negotiated {
        Some(layout) => Ok(layout.clone()),
        None => butterpollo_media_policy::audio::OpusLayout::select(
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
    negotiated: Option<&butterpollo_media_policy::audio::OpusLayout>,
) -> Result<(Opus, butterpollo_media_policy::audio::OpusLayout)> {
    let layout = audio_layout(s, negotiated)?;
    let opus = Opus::new_layout_duration(directory, &layout, s.config.audio_packet_ms)?;
    s.dvr.opus_layout(
        layout.channels,
        layout.streams,
        layout.coupled,
        &layout.mapping,
    );
    Ok((opus, layout))
}

type Route = butterpollo_win_io::audio_route::Route;
use butterpollo_win_io::audio_route::notify;

/// The `audio_surround` warning when the device has fewer channels (`applied`)
/// than the capture `requested`. They differ from the client's own layout only
/// for headphone surround, which captures surround for a stereo client.
fn surround_shortfall(requested: usize, applied: usize, client: usize) -> Option<String> {
    if applied >= requested {
        return None;
    }
    // A device the host does not format can have any channel count.
    let name = |channels| match channels {
        1 => "mono".into(),
        2 => "stereo".into(),
        6 => "5.1".into(),
        8 => "7.1".into(),
        12 => "7.1.4".into(),
        other => format!("{other} channels"),
    };
    let (wanted, got) = (name(requested), name(applied));
    Some(if client < requested {
        format!(
            "The PC's audio device took {got} instead of {wanted}; headphone surround mixes from {got}. Pick a device or driver that takes {wanted}."
        )
    } else {
        // Offer only a layout a client can choose.
        let choose = if matches!(applied, 2 | 6 | 8) {
            format!(", or choose {got}")
        } else {
            String::new()
        };
        format!(
            "The PC's audio device took {got} instead of {wanted}; the client hears {got}. Pick a device or driver that takes {wanted}{choose}."
        )
    })
}

/// The `audio_cadence_trace` ring, written to `logs\audio-cadence-<unix seconds>.csv` when the
/// audio thread ends, however it ends.
struct CadenceDump {
    trace: butterpollo_media_policy::audio_cadence::Trace,
    directory: std::path::PathBuf,
}

impl Drop for CadenceDump {
    fn drop(&mut self) {
        let seconds = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_secs());
        let path = self
            .directory
            .join("logs")
            .join(format!("audio-cadence-{seconds}.csv"));
        let written = std::fs::create_dir_all(path.parent().unwrap())
            .and_then(|()| std::fs::File::create(&path))
            .and_then(|file| {
                let mut out = std::io::BufWriter::new(file);
                self.trace.write_csv(&mut out)?;
                std::io::Write::flush(&mut out)
            });
        match written {
            Ok(()) => {
                tracing::info!(path = %path.display(), captured = self.trace.captured.len(), sent = self.trace.sent.len(), "audio cadence trace written")
            }
            Err(error) => {
                tracing::warn!(path = %path.display(), "audio cadence trace not written: {error}")
            }
        }
    }
}

/// Keep the streaming speakers the Windows default and find the device to
/// capture; the previous sink when Windows cannot say.
fn route_upkeep(
    route: &Route,
    config: &Config,
    warnings: &butterpollo_session::session::Warnings,
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

/// The audio route's upkeep, on its own thread: once a second and at once on
/// an endpoint change. Each round asks the Windows audio service for the
/// default devices, several COM calls; on the audio sender they held up the
/// read after them about once a second.
struct RouteUpkeep {
    sink: Arc<Mutex<String>>,
    /// Set when a round picks another sink, so the audio thread reads it at once.
    changed: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl RouteUpkeep {
    /// The first round runs on the caller, so capture opens on the right sink.
    fn start(
        route: Arc<Route>,
        config: Config,
        warnings: Arc<butterpollo_session::session::Warnings>,
    ) -> Result<Self> {
        let sink = Arc::new(Mutex::new(route_upkeep(
            &route,
            &config,
            &warnings,
            &route.sink,
        )));
        let stop = Arc::new(AtomicBool::new(false));
        let changed = Arc::new(AtomicBool::new(false));
        let thread = thread::Builder::new().name("audio route".into()).spawn({
            let sink = sink.clone();
            let changed = changed.clone();
            let stop = stop.clone();
            move || {
                let _com = match ComGuard::new() {
                    Ok(com) => com,
                    Err(error) => {
                        tracing::warn!(%error, "audio route upkeep unavailable");
                        warnings.set(
                            "audio_route",
                            format!(
                                "Audio route upkeep failed ({error:#}); playback device changes may not be tracked."
                            ),
                        );
                        return;
                    }
                };
                // Endpoint changes wake the next round at once; the poll stays the safety net.
                let upkeep = thread::current();
                let _watch = notify::EndpointWatch::new(Arc::new(move || upkeep.unpark()))
                    .inspect_err(|error| tracing::warn!("audio endpoint notifications unavailable; polling only: {error:#}"))
                    .ok();
                let mut last_round = Instant::now();
                loop {
                    notify::park_until_round(last_round);
                    if stop.load(Ordering::Acquire) {
                        break;
                    }
                    last_round = Instant::now();
                    let previous = sink.lock().unwrap().clone();
                    let selected = route_upkeep(&route, &config, &warnings, &previous);
                    let moved = selected != previous;
                    *sink.lock().unwrap() = selected;
                    if moved {
                        changed.store(true, Ordering::Release);
                    }
                }
            }
        })?;
        Ok(Self {
            sink,
            changed,
            stop,
            thread: Some(thread),
        })
    }
    fn sink(&self) -> String {
        self.sink.lock().unwrap().clone()
    }
    /// Whether a round picked another sink since the last call; one atomic load when not.
    fn take_changed(&self) -> bool {
        self.changed.load(Ordering::Relaxed) && self.changed.swap(false, Ordering::Acquire)
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
        let webrtc = if s.launch.transport == butterpollo_session::session::Transport::WebRtc {
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
        webrtc: Option<Box<dyn crate::webrtc_bridge::WebRtcAudioSink>>,
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
            Some(game_audio::GameAudio::start(
                h.clone(),
                s.clone(),
                config.clone(),
            )?)
        } else {
            None
        };
        let mut capture: Option<Loopback> = None;
        // Hidden: when each captured packet arrived and each went out, for support.
        let trace = config.boolean("audio_cadence_trace", false);
        let mut cadence = trace.then(|| CadenceDump {
            trace: butterpollo_media_policy::audio_cadence::Trace::new(),
            directory: h.directory.clone(),
        });
        let mut sink = String::new();
        let mut audio_check = Instant::now();
        let mut capture_opened = Instant::now();
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
                        .as_deref()
                        .map(crate::webrtc_bridge::WebRtcAudioSink::layout),
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
        let mut loss = butterpollo_media_policy::audio::HostLoss::default();
        // Packet deadlines, silence for a quiet endpoint, and where each packet sits in the
        // stream in 48 kHz samples; the WebRTC RTP clock follows that, not wall time.
        let mut pace = AudioPace::new(Instant::now(), interval, frames);
        let mut last_loss = None;
        let mut ping = audio_sink::PingWait::default();
        let mut upkeep: Option<RouteUpkeep> = None;
        let timer = butterpollo_win_graphics::timing::Timer::new()?;
        let silence = couch.silence(frames);
        // Each pass's packets, in room kept from pass to pass.
        let mut packets = audio_sink::Packets::default();
        // Passthrough codecs (32 ms AC3/E-AC3) tap audio haptics at 5 ms cadence
        // before accumulating full 1536-frame packets, avoiding Tap::KEEP truncation.
        let mut assembler = (frames > 480).then(|| {
            audio_sink::ChunkAssembler::new(audio_sink::HAPTICS_TAP_FRAMES, frames, client_channels)
        });
        while !s.stopping() && !h.stop.load(Ordering::Acquire) {
            // The wait runs to the earliest deadline, not on a fixed tick: the
            // next packet deadline, or the once-a-second device check when that
            // comes first. Every pass reads the stop flag, and no deadline sits
            // further out than the 50 ms silence threshold, so a stop is still
            // seen promptly.
            let now = Instant::now();
            let check = (!muted && !game_audio).then_some(audio_check);
            match capture
                .as_ref()
                .filter(|capturing| capturing.event_driven())
            {
                // Audio events wake the thread the moment audio arrives; the
                // deadline ends the wait only when none comes.
                Some(capturing) => {
                    capturing.wait(audio_sink::audio_wait(now, pace.due(), check, None))
                }
                None => timer.until(
                    now + audio_sink::audio_wait(
                        now,
                        pace.due(),
                        check,
                        (!muted).then_some(audio_sink::READ_POLL),
                    ),
                ),
            }
            #[cfg(debug_assertions)]
            if let Some(stall) = crate::soak_fault::audio_stall()? {
                thread::sleep(stall);
            }
            if let Some(game_capture) = &mut game_capture {
                game_capture.update(&s, &mut capture, &mut |mut old| {
                    let waited = old.since_drained();
                    audio_sink::retire(&mut old, waited, &mut loss, &mut pace, Instant::now());
                    if let Some(assembler) = &mut assembler {
                        pace.host_dropped(assembler.discard_frames());
                    }
                });
            }
            if upkeep.as_ref().is_some_and(RouteUpkeep::take_changed) {
                // The upkeep moved to another sink (an endpoint event): switch now, not at the next check.
                audio_check = Instant::now();
            }
            if !muted && !game_audio && Instant::now() >= audio_check {
                audio_check = Instant::now() + Duration::from_secs(1);
                if route.is_none() {
                    match butterpollo_win_io::audio_route::Route::acquire(
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
                            s.launch.warnings.event("audio_device_changed", "Audio capture device changed because the Windows default playback device changed. WASAPI is reopening and sound may briefly pause; select a fixed capture sink if the change was unintended.", butterpollo_session::session::EVENT_PERIOD);
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
                            if let Some(assembler) = &mut assembler {
                                pace.host_dropped(assembler.discard_frames());
                            }
                        }
                        sink = selected;
                    }
                    // Pinning the sink only disables following the Windows
                    // default. Even a fixed endpoint must reopen after a
                    // driver reset, format change or temporary disconnect.
                    if capture.is_none() {
                        let formatted = route.set_channels(capture_channels);
                        let opened = Loopback::new_sink_traced(capture_channels, &sink, trace);
                        match formatted {
                            Ok(applied) => {
                                // A sink the host does not format reports the request;
                                // Loopback maps whatever its mix format has to it.
                                let device =
                                    opened.as_ref().map_or(applied, |value| value.source().0);
                                match surround_shortfall(capture_channels, device, client_channels)
                                {
                                    Some(message) => {
                                        s.launch.warnings.set("audio_surround", message)
                                    }
                                    None => s.launch.warnings.clear("audio_surround"),
                                }
                            }
                            Err(error) => s.launch.warnings.set("audio_surround", format!("Virtual surround format could not be applied ({error:#}); channels may be downmixed. Select stereo or check the virtual audio driver.")),
                        }
                        match opened {
                            Ok(value) => {
                                tracing::info!(
                                    channels = capture_channels,
                                    source_channels = value.source().0,
                                    event_driven = value.event_driven(),
                                    "WASAPI audio capture started"
                                );
                                capture = Some(value);
                                capture_opened = Instant::now();
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
            ping.update(
                ready,
                || {
                    audio_sink::ping_timed_out(
                        &s,
                        &h.stop,
                        start,
                        crate::network::ping_timeout(&config),
                    )
                },
                &s.launch.warnings,
            );
            // Every whole packet the endpoint has delivered goes out at once, as
            // Sunshine sends them. On a fixed tick each waited up to a packet,
            // and a tick just before a late chunk sent silence in its place.
            let mut on_interrupted = |error: &anyhow::Error| {
                let recovery = if game_audio {
                    "sending silence until a verified game process can be captured again. No system audio is captured. Check the game's launch entry if this repeats."
                } else {
                    "reopening WASAPI and sending silence until it recovers. Check the playback device if this repeats."
                };
                s.launch.warnings.set(
                    "audio_capture",
                    format!("Audio capture interrupted ({error:#}); {recovery}"),
                );
                // The capture is gone: open it again on the next pass, not at the next
                // once-a-second check.
                audio_check = audio_sink::reopen_check(audio_check, capture_opened);
            };
            let pre_processed = if let Some(assembler) = &mut assembler {
                audio_sink::collect_chunked_into(
                    &mut capture,
                    assembler,
                    &mut loss,
                    &mut pace,
                    &silence,
                    Instant::now,
                    &mut on_interrupted,
                    |chunk| {
                        couch.process(&s.audio, &s.launch.warnings, chunk);
                        if ready {
                            s.audio_haptics.push(chunk, client_channels);
                        }
                    },
                    &mut packets,
                )
            } else {
                audio_sink::collect_into(
                    &mut capture,
                    frames,
                    &mut loss,
                    &mut pace,
                    &silence,
                    Instant::now,
                    &mut on_interrupted,
                    &mut packets,
                );
                0
            };
            if let (Some(cadence), Some(capturing)) = (&mut cadence, &mut capture) {
                capturing.drain_trace(&mut cadence.trace);
            }
            let now = Instant::now();
            if ready {
                output.begin_pass();
                for (index, (samples, &at)) in packets
                    .samples
                    .iter_mut()
                    .zip(&packets.positions)
                    .enumerate()
                {
                    if index >= pre_processed {
                        couch.process(&s.audio, &s.launch.warnings, samples);
                    }
                    for _ in 0..encoder.send(output.as_mut(), samples, now, at, &s.dvr)? {
                        loss.unsent();
                    }
                    if let Some(cadence) = &mut cadence {
                        cadence
                            .trace
                            .send(butterpollo_media_policy::audio_cadence::Sent {
                                send_ms: butterpollo_win_graphics::timing::qpc_ms(),
                                position: at,
                            });
                    }
                    // Haptics from game audio analyse a copy of what went out,
                    // on the control thread; one atomic load while unused.
                    if index >= pre_processed {
                        if frames > 480 {
                            for chunk in
                                samples.chunks(audio_sink::HAPTICS_TAP_FRAMES * client_channels)
                            {
                                s.audio_haptics.push(chunk, client_channels);
                            }
                        } else {
                            s.audio_haptics.push(samples, client_channels);
                        }
                    }
                }
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

#[cfg(test)]
mod tests {
    use super::audio_sink::{
        AudioCapture, AudioPace, ChunkAssembler, HAPTICS_TAP_FRAMES, Packets, collect_chunked_into,
    };
    use super::surround_shortfall;
    use anyhow::Result;
    use butterpollo_input_policy::audio_haptics::{Arbiter, Settings, Tap};
    use butterpollo_input_policy::haptics;
    use butterpollo_input_policy::input::Input;
    use butterpollo_media_policy::audio::HostLoss;
    use std::collections::VecDeque;
    use std::time::{Duration, Instant};

    #[test]
    fn chunked_passthrough_taps_audio_haptics_at_5ms_without_truncation() {
        struct FakeCapture {
            channels: usize,
            queued: VecDeque<f32>,
        }
        impl AudioCapture for FakeCapture {
            fn since_drained(&mut self) -> Option<Duration> {
                None
            }
            fn read_into(&mut self, frames: usize, out: &mut Vec<f32>) -> Result<bool> {
                let need = frames * self.channels;
                if self.queued.len() < need {
                    return Ok(false);
                }
                out.clear();
                out.extend(self.queued.drain(..need));
                Ok(true)
            }
            fn take_delivered(&mut self) -> Option<Duration> {
                None
            }
            fn buffer(&self) -> Duration {
                Duration::from_millis(20)
            }
            fn take_trimmed(&mut self) -> u64 {
                0
            }
            fn take_discarded(&mut self) -> u64 {
                (self.queued.len() / self.channels) as u64
            }
        }

        let channels = 6usize;
        let target_frames = 1536usize; // 32 ms AC3/E-AC3 passthrough
        let mut capture = Some(FakeCapture {
            channels,
            queued: VecDeque::new(),
        });
        let mut assembler = ChunkAssembler::new(HAPTICS_TAP_FRAMES, target_frames, channels);
        let mut loss = HostLoss::default();
        let base = Instant::now();
        let mut pace = AudioPace::new(base, Duration::from_millis(32), target_frames);
        let silence = vec![0.0; target_frames * channels];
        let mut packets = Packets::default();
        let tap = Tap::default();
        let mut arbiter = Arbiter::default();
        arbiter.set_settings(Settings {
            enabled: true,
            ..Settings::default()
        });
        arbiter.input(&Input::Arrival {
            id: 0,
            kind: 2,
            capabilities: haptics::CAPABILITY,
            buttons: 0,
        });
        assert!(arbiter.generate(true, &tap, base).is_empty());
        assert!(tap.wanted());

        let mut emitted_positions = Vec::new();
        let mut haptics_frames = 0usize;
        let mut sample_idx = 0usize;
        // Simulate 16 x 10 ms WASAPI wakes (= 160 ms = 5 complete 32 ms AC3 packets).
        for wake in 0..16u64 {
            let now = base + Duration::from_millis((wake + 1) * 10);
            if let Some(cap) = &mut capture {
                for _ in 0..480 {
                    let s =
                        0.2 * (std::f32::consts::TAU * 60.0 * sample_idx as f32 / 48_000.0).sin();
                    sample_idx += 1;
                    cap.queued.extend([s, s, 0.0, s, 0.0, 0.0]);
                }
            }
            let mut chunks_this_wake = 0usize;
            let captured = collect_chunked_into(
                &mut capture,
                &mut assembler,
                &mut loss,
                &mut pace,
                &silence,
                || now,
                |e| panic!("unexpected capture error: {e}"),
                |chunk| {
                    assert_eq!(chunk.len(), HAPTICS_TAP_FRAMES * channels);
                    tap.push(chunk, channels);
                    chunks_this_wake += 1;
                },
                &mut packets,
            );
            assert_eq!(chunks_this_wake, 2);
            assert_eq!(captured, packets.samples.len());
            emitted_positions.extend_from_slice(&packets.positions);
            for pkt in &packets.samples {
                assert_eq!(pkt.len(), target_frames * channels);
            }
            haptics_frames += arbiter.generate(true, &tap, now).len();
        }
        assert_eq!(emitted_positions, [0, 1536, 3072, 4608, 6144]);
        assert!(haptics_frames >= 25, "got only {haptics_frames} frames");
    }

    #[test]
    fn a_smaller_layout_than_requested_is_warned_about_and_names_both() {
        for requested in [2, 6, 8, 12] {
            assert_eq!(surround_shortfall(requested, requested, requested), None);
        }
        // Another stream on the shared sink asked for more.
        assert_eq!(surround_shortfall(6, 8, 6), None);
        assert_eq!(
            surround_shortfall(12, 6, 12).unwrap(),
            "The PC's audio device took 5.1 instead of 7.1.4; the client hears 5.1. Pick a device or driver that takes 7.1.4, or choose 5.1."
        );
        assert!(
            surround_shortfall(12, 8, 12)
                .unwrap()
                .contains("took 7.1 instead of 7.1.4")
        );
        // Headphone surround: the client is stereo, so it never hears 5.1.
        assert_eq!(
            surround_shortfall(8, 6, 2).unwrap(),
            "The PC's audio device took 5.1 instead of 7.1; headphone surround mixes from 5.1. Pick a device or driver that takes 7.1."
        );
        // A device the host does not format reports any count in its mix format.
        for (device, got) in [(1, "mono"), (4, "4 channels")] {
            assert_eq!(
                surround_shortfall(6, device, 6).unwrap(),
                format!(
                    "The PC's audio device took {got} instead of 5.1; the client hears {got}. Pick a device or driver that takes 5.1."
                )
            );
        }
    }
}
