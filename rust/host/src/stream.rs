use crate::state::{Launch, Session, Shared};
use butterpollo_core::{controller_audio, haptics};
mod audio;
pub(crate) mod audio_encoder;
mod audio_haptics;
mod audio_sink;
mod background;
mod capture;
pub(crate) mod clipboard_sync;
mod config;
mod control;
mod couch_audio;
mod device_recovery;
#[cfg(test)]
mod encoder_tests;
mod game_audio;
mod keyboard_layout;
mod qos;
mod roaming;
pub(crate) mod slices;
mod video;
#[cfg(test)]
pub(crate) use audio::{audio_layout, open_audio_encoder};
use audio_encoder::AudioEncoder;
use audio_sink::{AudioPace, AudioSink, GameStreamAudio, WebRtcAudio};
use butterpollo_core::audio_passthrough::Codec;
#[cfg(test)]
use config::{apply_overrides, sets_peak};
pub(crate) use config::{device_config, effective_config, launch_app};
#[cfg(test)]
use control::{
    CONTROL_EVENT_LIMIT, ControlPeer, ControlSocket, FEEDBACK_CHANNEL, INPUT_OVERFLOW, INPUT_QUEUE,
    MOTION_EVENT_REQUEST, PadSends, RUMBLE_MARKER, RumbleForwarded, SWALLOWED_PER_RECEIVE,
    control_wait_timeout, drop_uninjectable_input, end_unattended_sessions, expects_enet_control,
    expire_controllers, first_steam_attributes_refusal, haptics_negotiated, motion_requests,
    pad_reports, queue_input, server_command,
};
pub(crate) use control::{RUMBLE_DATA, RUMBLE_TRIGGER_DATA, feedback_packets};
use qos::Tagged;
#[cfg(test)]
use video::{
    ENCODER_RECOVERY, EncoderOutput, EncoderRecovery, OUTPUT_POLL, encoder_progress,
    observe_foreground_game,
};
#[cfg(test)]
pub(crate) mod reconnect_tests;
use anyhow::{Context, Result};
use butterpollo_core::{
    config::Config,
    crypto,
    encoder_backlog::{self, Backpressure},
    input,
    packet::{AudioPacketizer, VideoPacketizer},
    session::Role,
    text_input::Watch as TextWatch,
};
use butterpollo_windows::device_loss::DeviceLost;
/// A WebRTC HDR stream carries the display's HDR or nothing: SDR content labelled HDR, or HDR
/// tone-mapped to SDR, is not what the client asked for, so the session ends with an error.
/// `display_in_hdr` is the capture's reading of the output's colour space (HDR10 or not); the
/// frames' pixel format cannot stand in for it, since FP16 frames come from SDR outputs too.
/// GameStream keeps its own behaviour for this case. `running` tells a display that left HDR
/// from one that never was in it.
pub(crate) fn require_hdr_source(
    s: &Session,
    display_in_hdr: impl FnOnce() -> Option<bool>,
    running: bool,
) -> Result<()> {
    if s.launch.transport != butterpollo_core::session::Transport::WebRtc || !s.config.hdr {
        return Ok(());
    }
    match display_in_hdr() {
        Some(true) => Ok(()),
        Some(false) if running => anyhow::bail!(
            "the display left HDR during the stream; the HDR stream ended instead of showing SDR as HDR. Keep Windows HDR on for this display, or start an SDR stream"
        ),
        Some(false) => anyhow::bail!(
            "HDR was requested but the display is not in HDR; turn on HDR for this display in Windows settings (or set the HDR policy to auto in the host's display settings), or start an SDR stream"
        ),
        // A reading that fails mid-stream is the capture's to report (it ends on a colour space
        // error itself); only a display seen leaving HDR ends the stream here.
        None if running => Ok(()),
        None => anyhow::bail!(
            "HDR was requested but Windows did not report the display's colour space, so the stream cannot tell HDR from SDR; start an SDR stream"
        ),
    }
}
/// Backoff only after reopening fails; resource release is acknowledged.
const RECOVERY_RETRY: Duration = Duration::from_millis(150);
/// How long capture waits for the stream's display to come back before it
/// shows another one. Switching a virtual display back on takes about a
/// second; recreating it takes up to the driver's 10 s arrival deadline.
const DISPLAY_RETURN_WAIT: Duration = Duration::from_secs(15);
/// Which display a capture reopen uses.
#[derive(Clone, Copy, Debug, PartialEq)]
enum ReopenOn {
    /// The stream's display.
    Stream,
    /// Not yet: the stream's display is not on the desktop.
    Wait,
    /// The primary display, after the stream's did not return in time.
    Primary,
}
/// Capture follows the stream's display. Windows can switch a virtual display
/// off (a game resetting the layout on exit); the heartbeat switches it back
/// on or recreates it and publishes it, possibly under a new name. Capture
/// waits for that and never quietly shows another desktop: only after
/// `DISPLAY_RETURN_WAIT` does a foreground stream open the primary display.
/// A background stream must never expose the local user's desktop.
fn reopen_on(
    present: bool,
    background: bool,
    missing_since: &mut Option<Instant>,
    now: Instant,
) -> ReopenOn {
    if present {
        *missing_since = None;
        return ReopenOn::Stream;
    }
    let since = *missing_since.get_or_insert(now);
    if !background && now.duration_since(since) >= DISPLAY_RETURN_WAIT {
        ReopenOn::Primary
    } else {
        ReopenOn::Wait
    }
}

use butterpollo_windows::{
    audio::{Loopback, Opus},
    capture::{Capture, ComGuard, GpuImage, Priority},
    encoder::Encoder,
    input::{Injector, PadReport},
    net::ReplySocket,
};
use rusty_enet::{Event, Host, HostSettings, Packet, PacketKind, PeerID, PeerState};
use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr, UdpSocket},
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

type Latest = capture::Latest<GpuImage>;
struct Source {
    warnings: Arc<butterpollo_core::session::Warnings>,
    latest: Arc<Latest>,
    /// The capture backend that opened, after every reopen: `auto` is planned
    /// as WGC, so the plan cannot say whether frames come from WGC or the
    /// display driver.
    backend: Arc<Mutex<&'static str>>,
    grid: Arc<Mutex<butterpollo_windows::capture::ClaimGrid>>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}
fn finish_worker(worker: thread::JoinHandle<()>, name: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !worker.is_finished() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    if worker.is_finished() {
        let _ = worker.join();
    } else {
        tracing::error!(
            worker = name,
            "teardown did not return within five seconds; detaching the driver worker"
        );
    }
}
impl Drop for Source {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.thread.take() {
            finish_worker(worker, "capture");
        }
    }
}
impl Source {
    fn backend(&self) -> &'static str {
        *self.backend.lock().unwrap()
    }
}
impl std::ops::Deref for Source {
    type Target = Latest;
    fn deref(&self) -> &Latest {
        &self.latest
    }
}
/// Each session's video and audio addresses from its pings, by launch ID. The stream and
/// audio threads look theirs up for every frame and packet, so a lookup copies no key.
#[derive(Default)]
pub(crate) struct Peers(HashMap<String, [Option<SocketAddr>; 2]>);
impl Peers {
    pub(crate) fn get(&self, id: &str, audio: bool) -> Option<SocketAddr> {
        self.0.get(id).and_then(|pair| pair[usize::from(audio)])
    }
    /// The address it replaced, if any.
    fn insert(&mut self, id: &str, audio: bool, peer: SocketAddr) -> Option<SocketAddr> {
        if let Some(pair) = self.0.get_mut(id) {
            return pair[usize::from(audio)].replace(peer);
        }
        let mut pair = [None; 2];
        pair[usize::from(audio)] = Some(peer);
        self.0.insert(id.to_owned(), pair);
        None
    }
    /// Forgets a session's addresses, returning each as (audio, address).
    fn remove(&mut self, id: &str) -> impl Iterator<Item = (bool, SocketAddr)> + use<> {
        let pair = self.0.remove(id).unwrap_or_default();
        [false, true]
            .into_iter()
            .zip(pair)
            .filter_map(|(audio, peer)| Some((audio, peer?)))
    }
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}
pub struct Media {
    video: Arc<ReplySocket>,
    audio: Arc<ReplySocket>,
    control_wake: Arc<UdpSocket>,
    control_wake_receiver: UdpSocket,
    /// The microphone receiver's counts; None when its port could not be bound.
    pub(crate) mic: Option<Arc<crate::mic::Counters>>,
    peers: Mutex<Peers>,
    captures: Mutex<HashMap<CaptureKey, Weak<Source>>>,
    control_port: u16,
    bind: IpAddr,
    /// Roaming seqs of the launches, kept across control worker restarts so no launch key
    /// repeats a nonce.
    roam_seqs: butterpollo_core::roaming::SeqMemory,
}
#[derive(Debug, PartialEq, Eq, Hash)]
struct CaptureKey {
    kind: String,
    output: String,
    hdr: bool,
    adapter: String,
    adapter_id: String,
    phase: String,
    compute: bool,
    wgc_compute: bool,
    wgc_user_helper: bool,
    wgc_high_rate: bool,
    wgc_drain: bool,
    wgc_helper_scheduling: bool,
    idd_fence_ring: bool,
    background: bool,
}
impl CaptureKey {
    fn new(kind: &str, output: &str, hdr: bool, config: &Config, phase: &str) -> Self {
        Self {
            kind: kind.into(),
            output: output.into(),
            hdr,
            adapter: config.get("adapter_name", "").into(),
            adapter_id: config.get("adapter_pnp_id", "").into(),
            phase: if config.boolean("wgc_slot_aligned_publish", false) {
                phase
            } else {
                ""
            }
            .into(),
            compute: config.boolean("gpu_compute_conversion", true),
            wgc_compute: config.boolean("wgc_compute_copy", true),
            wgc_user_helper: config.boolean("wgc_user_helper", false),
            wgc_high_rate: !matches!(kind, "ddx" | "dxgi")
                && config.boolean("wgc_high_rate_capture", false),
            wgc_drain: !matches!(kind, "ddx" | "dxgi")
                && config.boolean("wgc_drain_to_newest", false),
            wgc_helper_scheduling: !matches!(kind, "ddx" | "dxgi")
                && config.boolean("wgc_helper_streaming_scope", false),
            idd_fence_ring: config.idd_push_fence_ring(),
            background: butterpollo_core::background::enabled(config),
        }
    }
}
/// The first video ping from the path a client roamed to: the client needs a keyframe,
/// and what the link-rate controller learned about the old path no longer holds.
fn restart_on_new_path(s: &Session, c: &Config) {
    s.new_network_path();
    s.request_host_keyframe();
    let mut stream = s.config.clone();
    s.stream_mode().apply_to(&mut stream);
    s.rebuild_link_rate(&stream, c.link_rate_adaptation(), max_bitrate_kbps(s, c));
    configure_adaptive_bitrate(s, c);
}
/// The stream's video FEC at its start: the configured percentage is adaptive
/// FEC's ceiling (with `adaptive_fec` off, or a client that sends no FEC status,
/// it is used for every frame as before), unless the client chose its own with
/// `rlFecPercentage`.
fn configure_video_fec(s: &Session, c: &Config) -> Result<()> {
    let requested_fec = c.integer("fec_percentage", 20);
    if requested_fec != requested_fec.clamp(0, 100) {
        s.launch.warnings.set("network_fec_config", format!("FEC percentage {requested_fec} is outside 0-100; using {}%. Correct fec_percentage in Video settings.", requested_fec.clamp(0, 100)));
    }
    let fec_ceiling = if s.config.codec == 3 {
        c.integer("pyrowave_critical_fec_percentage", 20)
            .clamp(0, 255) as usize
    } else {
        requested_fec.clamp(0, 100) as usize
    };
    // A client's `rlFecPercentage` was checked at /launch.
    let choice = butterpollo_core::extensions::fec_choice(&s.launch.options)?;
    let maximum = c.fec_max_percentage();
    let start = s.configure_fec_choice(
        choice,
        c.boolean("adaptive_fec", true),
        fec_ceiling,
        maximum,
    );
    if choice != butterpollo_core::extensions::FecChoice::Host {
        tracing::info!(
            client = %s.launch.client.name,
            choice = %choice.as_param(),
            pyrowave = s.config.codec == 3,
            percent = start,
            maximum,
            "client fec choice"
        );
    }
    Ok(())
}
/// Host bitrate adaptation (`host_adaptive_bitrate`), from scratch.
fn configure_adaptive_bitrate(s: &Session, c: &Config) {
    s.configure_adaptive_bitrate(
        c.boolean("host_adaptive_bitrate", false),
        max_bitrate_kbps(s, c),
    );
}
/// The user's `max_bitrate`, within what the stream's codec can be asked for:
/// PyroWave goes to 2 Gbps, the others to 500 Mbps.
fn max_bitrate_kbps(s: &Session, c: &Config) -> u32 {
    let maximum = if s.config.codec == 3 {
        2_000_000
    } else {
        500_000
    };
    c.integer("max_bitrate", 0).clamp(0, maximum) as u32
}
/// The local address a client's datagrams arrived on, for logs.
fn describe_local(local: Option<IpAddr>) -> String {
    local.map_or_else(|| "unknown".into(), |local| local.to_string())
}
fn capture_config(config: &Config) -> Config {
    let mut capture = config.clone();
    // Keep the effective interval choice with the shared capture and its helper.
    // A 1 ms request skipped updates in the 120 FPS motion comparison; use
    // explicit zero at every rate unless the user opts into the legacy limit.
    capture.values.insert(
        "wgc_high_rate_capture".into(),
        config.boolean("wgc_high_rate_capture", false).to_string(),
    );
    capture
}
impl Media {
    pub fn new(h: Shared, bind: IpAddr) -> Result<Arc<Self>> {
        let ports = h.config.read().unwrap().ports()?;
        let video = ReplySocket::new(crate::network::udp((bind, ports.video).into())?)?;
        let audio = ReplySocket::new(crate::network::udp((bind, ports.audio).into())?)?;
        butterpollo_windows::net::configure_udp(&video)?;
        butterpollo_windows::net::configure_udp(&audio)?;
        video.set_nonblocking(true)?;
        audio.set_nonblocking(true)?;
        let control_wake_receiver = UdpSocket::bind("127.0.0.1:0")?;
        let control_wake = UdpSocket::bind("127.0.0.1:0")?;
        control_wake.connect(control_wake_receiver.local_addr()?)?;
        control_wake_receiver.connect(control_wake.local_addr()?)?;
        control_wake.set_nonblocking(true)?;
        control_wake_receiver.set_nonblocking(true)?;
        // A microphone port taken by another program only costs the microphone.
        let mic = crate::network::udp((bind, ports.mic).into())
            .and_then(|socket| crate::mic::spawn(h.clone(), socket))
            .inspect_err(|error| {
                tracing::warn!(port = ports.mic, error = %format!("{error:#}"), "client microphones unavailable")
            })
            .ok();
        let m = Arc::new(Self {
            video: Arc::new(video),
            audio: Arc::new(audio),
            control_wake: Arc::new(control_wake),
            control_wake_receiver,
            mic,
            peers: Mutex::default(),
            captures: Mutex::new(HashMap::new()),
            control_port: ports.control,
            bind,
            roam_seqs: Default::default(),
        });
        for is_audio in [false, true] {
            let socket = if is_audio {
                m.audio.clone()
            } else {
                m.video.clone()
            };
            let m = m.clone();
            let h = h.clone();
            thread::Builder::new()
                .name(if is_audio { "audio-ping" } else { "video-ping" }.into())
                .spawn(move || {
                    let mut b = [0; 2048];
                    let raw_socket = std::os::windows::io::AsRawSocket::as_raw_socket(&*socket);
                    while !h.stop.load(Ordering::Acquire) {
                        match butterpollo_windows::net::wait_readable(raw_socket, 250) {
                            Ok(true) => {}
                            Ok(false) => continue,
                            Err(e) => {
                                tracing::warn!(error=%e,"media ping wait failed");
                                thread::sleep(Duration::from_millis(250));
                                continue;
                            }
                        }
                        match socket.receive(&mut b) {
                            Ok((n, peer, local)) => {
                                let sessions = h.sessions.lock().unwrap();
                                let mut roamed = None;
                                for s in sessions.active.values() {
                                    if s.client_ip() != peer.ip().to_canonical() {
                                        continue;
                                    }
                                    let valid = (n >= 16
                                        && crypto::equal(&b[..16], s.launch.ping.as_bytes()))
                                        || (n == 4
                                            && &b[..4] == b"PING"
                                            && sessions
                                                .active
                                                .values()
                                                .filter(|s| {
                                                    s.client_ip() == peer.ip().to_canonical()
                                                })
                                                .count()
                                                == 1
                                            // After a roam only the session's own ping
                                            // proves a new address: behind a CGNAT
                                            // anyone may share the client's IP.
                                            && (s.roams() == 0
                                                || m.peers
                                                    .lock()
                                                    .unwrap()
                                                    .get(&s.launch.id, is_audio)
                                                    == Some(peer)));
                                    if valid {
                                        // Answer from the address the client reached:
                                        // Windows may route a reply out of a VPN address.
                                        socket.answer_from(peer, local);
                                        let previous = m.peers.lock().unwrap().insert(
                                            &s.launch.id,
                                            is_audio,
                                            peer,
                                        );
                                        if previous != Some(peer) {
                                            if let Some(previous) = previous {
                                                socket.forget(previous);
                                            }
                                            tracing::info!(
                                                client = %s.launch.client.name,
                                                %peer,
                                                local = %describe_local(local),
                                                stream = if is_audio { "audio" } else { "video" },
                                                "stream peer"
                                            );
                                            if !is_audio && s.take_new_path() {
                                                roamed = Some(s.clone());
                                            }
                                        }
                                        break;
                                    }
                                }
                                drop(sessions);
                                if let Some(s) = roamed {
                                    restart_on_new_path(&s, &h.config.read().unwrap());
                                }
                            }
                            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                            Err(e) => {
                                tracing::warn!(error=%e,"media ping receive failed");
                                thread::sleep(Duration::from_millis(10));
                            }
                        }
                    }
                })?;
        }
        let control = m.clone();
        thread::Builder::new()
            .name("control".into())
            .spawn(move || {
                // Input and control for every session pass through this
                // worker: restart it rather than leave later sessions without.
                while !h.stop.load(Ordering::Acquire) {
                    match control.control(h.clone()) {
                        Ok(()) => break,
                        Err(e) => {
                            tracing::error!(error = %format!("{e:#}"), "control worker stopped; restarting");
                            thread::sleep(Duration::from_secs(1));
                        }
                    }
                }
            })?;
        Ok(m)
    }
    fn capture(
        &self,
        kind: &str,
        hdr: bool,
        config: &Config,
        rate: butterpollo_core::framegen::Rate,
        phase: &str,
        prepared: Arc<crate::display_session::Ready>,
    ) -> Result<Arc<Source>> {
        let output = prepared.output();
        let aligned = config.boolean("wgc_slot_aligned_publish", false);
        let capture_config = capture_config(config);
        let key = CaptureKey::new(kind, &output, hdr, &capture_config, phase);
        let mut captures = self.captures.lock().unwrap();
        captures.retain(|_, source| source.strong_count() > 0);
        if let Some(existing) = captures.get(&key).and_then(Weak::upgrade)
            && existing.check().is_ok()
        {
            return Ok(existing);
        }
        let latest = Arc::new(Latest::new());
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let worker = latest.clone();
        let warnings = Arc::new(butterpollo_core::session::Warnings::default());
        let capture_warnings = warnings.clone();
        let grid = Arc::new(Mutex::new(butterpollo_windows::capture::ClaimGrid {
            anchor: Instant::now(),
            period: rate.period(),
        }));
        let worker_grid = grid.clone();
        let backend = Arc::new(Mutex::new(""));
        let worker_backend = backend.clone();
        let kind = kind.to_owned();
        let (started_tx, started_rx) = std::sync::mpsc::sync_channel(1);
        let thread = thread::Builder::new()
            .name("capture".into())
            .spawn(move || {
                let result = (|| -> Result<()> {
                    let _com = ComGuard::new()?;
                    let _priority = Priority::new();
                    let _display_awake = butterpollo_windows::timing::DisplayAwake::enter()
                        .inspect_err(|error| tracing::warn!(%error, "capture cannot keep the display awake"))
                        .ok();
                    let timer = butterpollo_windows::timing::Timer::new()?;
                    let mut target = prepared.capture_target();
                    // Duplicate the desktop that is showing, including the secure
                    // desktop of a UAC prompt or the lock screen.
                    butterpollo_windows::input::follow_input_desktop();
                    let mut capture =
                        match Capture::open_for_stream_reported(&target.0, &kind, hdr, &capture_config, capture_warnings.clone()) {
                            Ok(c) => c,
                            Err(e) => {
                                let _ = started_tx.send(Err(format!("{e:#}")));
                                return Ok(());
                            }
                        };
                    capture.set_claim_grid(worker_grid.clone(), aligned);
                    *worker_backend.lock().unwrap() = capture.backend();
                    tracing::info!(requested=%kind, backend=capture.backend(), output=%target.0, "capture backend opened");
                    // Re-create the capture on a new device. Windows keeps handing
                    // out the stale adapter while anything holds the old device, so
                    // every duplication made then loses access at once (a display
                    // arriving for another client does this). The old capture is
                    // dropped and consumers acknowledge releasing their frames
                    // and encoders before the new device is made.
                    // The capture, whether it shows the primary display in
                    // place of the stream's, which is gone, and the stream's
                    // display it was opened for. The display can be recreated
                    // while capture opens; the next check then sees that.
                    let reopen = |lost: Capture, target: &(String, u64)| -> Result<(Capture, bool, (String, u64))> {
                        let recovery_started = Instant::now();
                        // Desktop Duplication reports the pointer's shape only
                        // when it changes; the new capture starts from this one.
                        let mut pointer = lost.pointer();
                        worker.begin_recovery()?;
                        drop(lost);
                        let deadline = Instant::now() + Duration::from_secs(30);
                        while !worker.consumers_released() {
                            worker.check()?;
                            if worker_stop.load(Ordering::Acquire) {
                                anyhow::bail!("capture stopped while releasing resources");
                            }
                            anyhow::ensure!(Instant::now() < deadline, "streams did not release the lost capture device");
                            timer.until(Instant::now() + Duration::from_millis(2));
                        }
                        let release_ms = recovery_started.elapsed().as_millis();
                        let mut missing_since = None;
                        loop {
                            worker.check()?;
                            anyhow::ensure!(Instant::now() < deadline, "capture did not recover within 30 seconds");
                            if worker_stop.load(Ordering::Acquire) {
                                anyhow::bail!("capture stopped while recovering");
                            }
                            let next = prepared.capture_target();
                            // A UAC prompt or the lock screen switches the input
                            // desktop; duplication must be made on that desktop.
                            butterpollo_windows::input::follow_input_desktop();
                            let background = butterpollo_core::background::enabled(&capture_config);
                            let on = reopen_on(butterpollo_windows::capture::display_present(&next.0), background, &mut missing_since, Instant::now());
                            let opened = match on {
                                ReopenOn::Wait => {
                                    if background {
                                        capture_warnings.set("capture_display", "The background display disappeared. Waiting for it to recover; the PC's monitor is not captured.");
                                    }
                                    thread::sleep(RECOVERY_RETRY);
                                    continue;
                                }
                                ReopenOn::Stream => Capture::open_for_stream_reported(&next.0, &kind, hdr, &capture_config, capture_warnings.clone()),
                                // By name: the WGC helper checks it opened the display asked for.
                                ReopenOn::Primary => butterpollo_windows::capture::primary_display()
                                    .context("no display is on the desktop")
                                    .and_then(|primary| Capture::open_for_stream_reported(&primary, &kind, hdr, &capture_config, capture_warnings.clone())),
                            };
                            match opened {
                                Ok(mut recovered) => {
                                    if on == ReopenOn::Primary {
                                        capture_warnings.set("capture_display", format!("The stream's display {} disappeared and did not return within {} s; showing the primary display until it returns. A game or Windows changed the display layout; reconnect if the picture does not return.", next.0, DISPLAY_RETURN_WAIT.as_secs()));
                                    } else {
                                        capture_warnings.clear("capture_display");
                                    }
                                    if next != *target {
                                        tracing::info!(output = %next.0, "capture moved to the recreated display");
                                    }
                                    if let Some(pointer) = pointer.take()
                                        && let Err(error) = recovered.resume_pointer(pointer)
                                    {
                                        tracing::warn!(%error, "the pointer appears once it moves or changes");
                                    }
                                    tracing::info!(release_ms, elapsed_ms=recovery_started.elapsed().as_millis(), backend=recovered.backend(), "capture reopened after resource release");
                                    return Ok((recovered, on == ReopenOn::Primary, next));
                                }
                                Err(error) if Instant::now() >= deadline => return Err(error),
                                Err(_) => thread::sleep(RECOVERY_RETRY),
                            }
                        }
                    };
                    let _ = started_tx.send(Ok(()));
                    let progress = crate::stall_watch::register("capture", &output, None);
                    let mut check_target = Instant::now();
                    let mut user_desktop = kind == "wgc"
                        && butterpollo_windows::capture::wgc_desktop_available();
                    let mut on_primary = false;
                    let poll_interval = Duration::from_micros(
                        capture_config.integer("capture_poll_interval_us", 500).clamp(100, 1000) as u64,
                    );
                    let mut last_frame = Instant::now();
                    while !worker_stop.load(Ordering::Acquire) {
                        if Instant::now() >= check_target {
                            check_target = Instant::now() + Duration::from_millis(100);
                            let next = prepared.capture_target();
                            let available = kind == "wgc"
                                && butterpollo_windows::capture::wgc_desktop_available();
                            let return_to_wgc = available && !user_desktop && capture.backend() == "ddx";
                            user_desktop = available;
                            // Back from the primary display once the stream's returns.
                            let returned = on_primary && butterpollo_windows::capture::display_present(&next.0);
                            if worker.take_device_restart()? || next != target || return_to_wgc || returned {
                                let lost = std::mem::replace(&mut capture, Capture::Closed);
                                progress.mark(crate::stall_watch::Phase::Reopen);
                                (capture, on_primary, target) = match reopen(lost, &next) {
                                    Ok(capture) => capture,
                                    Err(_) if worker_stop.load(Ordering::Acquire) => return Ok(()),
                                    Err(error) => return Err(error),
                                };
                                capture.set_claim_grid(worker_grid.clone(), aligned);
                                *worker_backend.lock().unwrap() = capture.backend();
                                last_frame = Instant::now();
                            }
                        }
                        // Reset before the helper's announcements are read: one
                        // that comes after them ends the wait below.
                        if let Some(signal) = capture.frame_signal() {
                            signal.reset()?;
                        }
                        progress.mark(crate::stall_watch::Phase::Capture);
                        let captured = (|| {
                            #[cfg(debug_assertions)]
                            crate::soak_fault::check("DXGI_ERROR_ACCESS_LOST")?;
                            capture.next_gpu()
                        })();
                        match captured {
                            Ok(Some(mut image)) => {
                                last_frame = Instant::now();
                                capture_warnings.clear("capture_recovery");
                                let (captured, focus) = (image.captured, image.focus);
                                image.published = Instant::now();
                                worker.publish_focus(Arc::new(image), captured, focus)?;
                            }
                            Ok(None) => {
                                let interval = if capture_config.boolean("capture_predictive_poll", false)
                                    && capture.backend() == "ddx" {
                                    worker.poll_interval(poll_interval)
                                } else { poll_interval };
                                let interval = capture.wait_interval(last_frame.elapsed(), interval);
                                let deadline = Instant::now() + interval;
                                let until = capture.publication_deadline().unwrap_or(deadline).min(deadline);
                                match capture.frame_signal() {
                                    // Frame announcements interrupt both active polling
                                    // and the longer wait used by a static source.
                                    Some(signal) => {
                                        timer.until_or_signal(until, signal)?;
                                    }
                                    None => timer.until(until),
                                }
                            }
                            Err(e) => {
                                if let Some(loss) = DeviceLost::from_error(&e) {
                                    worker.device_lost(loss, &capture_warnings, Instant::now())?;
                                    worker.take_device_restart()?;
                                } else {
                                    capture_warnings.set("capture_recovery", format!("Capture interrupted ({e:#}); reopening capture, with a frozen picture until frames resume. If this repeats, keep the display mode stable and check the WGC helper and graphics driver."));
                                }
                                let lost = std::mem::replace(&mut capture, Capture::Closed);
                                progress.mark(crate::stall_watch::Phase::Reopen);
                                (capture, on_primary, target) = match reopen(lost, &target) {
                                    Ok(capture) => capture,
                                    Err(_) if worker_stop.load(Ordering::Acquire) => return Ok(()),
                                    Err(error) => return Err(error),
                                };
                                capture.set_claim_grid(worker_grid.clone(), aligned);
                                *worker_backend.lock().unwrap() = capture.backend();
                                last_frame = Instant::now();
                            }
                        }
                    }
                    Ok(())
                })();
                if let Err(e) = result {
                    if DeviceLost::from_error(&e).is_some() {
                        capture_warnings.set("gpu_recovery", format!("{e:#}"));
                    }
                    let _ = worker.fail(format!("{e:#}"));
                    tracing::error!(error=%format!("{e:#}"),"capture worker stopped");
                }
            })?;
        let latest = Arc::new(Source {
            warnings,
            latest,
            backend,
            grid,
            stop,
            thread: Some(thread),
        });
        started_rx
            .recv_timeout(Duration::from_secs(10))?
            .map_err(|e| anyhow::anyhow!(e))?;
        captures.insert(key, Arc::downgrade(&latest));
        Ok(latest)
    }
    pub fn start(self: &Arc<Self>, h: Shared, s: Arc<Session>) {
        // Raised first, so a session that ends at once can't end before it started.
        h.session_webhook("session.started", &s);
        let result = self.spawn_session(&h, &s);
        if let Err(e) = result {
            h.sessions.lock().unwrap().fail_start(&s);
            tracing::error!(error=%e,"could not start session worker");
            h.session_webhook("session.ended", &s);
        }
    }
    fn open_session_capture<'a>(
        &self,
        s: &Arc<Session>,
        c: &Config,
        prepared: &Arc<crate::display_session::Ready>,
        latest: &'a mut Option<Arc<Source>>,
    ) -> Result<(&'a mut Arc<Source>, Arc<capture::Consumer>)> {
        let latest = latest.insert(self.capture(
            &prepared.capture(),
            s.config.hdr,
            c,
            butterpollo_core::framegen::Rate(s.config.fps_millihz()),
            &s.launch.id,
            prepared.clone(),
        )?);
        *s.capture_warnings.write().unwrap() = latest.warnings.clone();
        let capture_wake = latest.subscribe(Arc::downgrade(s))?;
        capture_wake.wake_on_recovery(s);
        Ok((latest, capture_wake))
    }
    fn set_display_hdr_metadata(
        &self,
        s: &Session,
        display: butterpollo_core::hdr::Metadata,
    ) -> butterpollo_core::hdr::Metadata {
        let previous = s.hdr_metadata.read().unwrap().map(|m| m.wire(s.config.hdr));
        let metadata = s.set_display_hdr_metadata(display);
        if previous != Some(metadata.wire(s.config.hdr)) {
            let _ = self.control_wake.send(&[1]);
        }
        metadata
    }
    fn configure_encoder(
        &self,
        s: &Session,
        c: &Config,
        first: &GpuImage,
        prepared: &crate::display_session::Ready,
        encoder: &mut Option<Encoder>,
    ) -> Result<()> {
        *encoder = Some(Encoder::new_gpu_reported(
            &s.config,
            first,
            c,
            &s.launch.warnings,
        )?);
        *s.encoder.write().unwrap() = encoder.as_ref().unwrap().backend().into();
        let source_refresh_hz = butterpollo_windows::display::mode(&first.gpu.display.display_name)
            .ok()
            .map(|mode| mode.dmDisplayFrequency);
        tracing::info!(width=s.config.width,height=s.config.height,fps=f64::from(s.config.fps_millihz())/1000.,requested_fps=s.config.fps,bitrate_kbps=s.bitrate.load(Ordering::Relaxed),configured_bitrate_kbps=s.config.configured_bitrate_kbps,codec=s.config.codec,hdr=s.config.hdr,ten_bit=s.config.ten_bit(),full_range=s.config.full_range(),color_matrix=s.config.color_matrix(),vrr=s.config.vrr_low_latency,packet_size=s.config.packet_size,pyrowave_records=s.config.pyrowave_records,pyrowave_row_order=s.config.pyrowave_row_order,requested_capture=%prepared.capture(),requested_encoder=c.get("encoder","auto"),encoder=encoder.as_ref().unwrap().backend(),adapter=%first.gpu.display.adapter,source_width=first.width,source_height=first.height,source_refresh_hz,source_pixel=?first.pixel,"stream configured");
        if s.config.chroma_packing != 0 {
            let (coded_width, coded_height) = s.config.coded_size();
            tracing::info!(
                packing_version = s.config.chroma_packing,
                codec = s.config.codec,
                coded_width,
                coded_height,
                bit_depth = if s.config.ten_bit() { 10 } else { 8 },
                "packed 4:4:4 stream"
            );
        }
        let minimum = butterpollo_core::pyrowave::minimum_kbps(
            s.config.width,
            s.config.height,
            s.config.fps_millihz(),
        );
        let recommended = butterpollo_core::pyrowave::recommended_kbps(
            s.config.width,
            s.config.height,
            s.config.fps_millihz(),
        );
        let bitrate = s.bitrate.load(Ordering::Relaxed);
        if s.config.codec == 3 && bitrate < minimum {
            tracing::warn!(
                bitrate_kbps = bitrate,
                minimum_kbps = minimum,
                recommended_kbps = recommended,
                "PyroWave bitrate is too low: severe detail loss is likely. Raise the bitrate in Moonlight with network headroom, or use HEVC or AV1"
            );
        } else if s.config.codec == 3 && bitrate < recommended {
            tracing::warn!(
                bitrate_kbps = bitrate,
                minimum_kbps = minimum,
                recommended_kbps = recommended,
                "PyroWave bitrate is below recommended: text and textures may lose detail. Quality depends on the picture; raise the bitrate in Moonlight with network headroom, or use HEVC or AV1"
            );
        }
        // HDR metadata describes the captured source, independent of receiver brightness.
        let metadata = self.set_display_hdr_metadata(s, first.gpu.hdr_metadata());
        if let Some(encoder) = encoder.as_mut() {
            encoder.set_hdr_metadata(metadata);
            encoder.set_hdr_target(s.hdr_target_nits());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
