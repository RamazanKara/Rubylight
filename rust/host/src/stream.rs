use crate::state::{Launch, Session, Shared};
use butterpollo_core::{controller_audio, haptics};
mod background;
mod capture;
mod device_recovery;
#[cfg(test)]
mod encoder_tests;
mod game_audio;
mod qos;
use qos::Tagged;
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
};
use butterpollo_windows::device_loss::DeviceLost;
fn server_command(
    h: &Shared,
    client: &butterpollo_core::state::Client,
    payload: &[u8],
    authenticated: bool,
    command_at: &mut Option<Instant>,
) -> Option<(String, bool)> {
    // Legacy plaintext control is not proof that the sender owns the stream key.
    if !authenticated || !client.allows(1 << 20) || payload.len() != 1 {
        return None;
    }
    if h.current_app
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|app| !app.allow_client_commands)
        || command_at.is_some_and(|last| last.elapsed() < Duration::from_secs(1))
    {
        return None;
    }
    *command_at = Some(Instant::now());
    let commands: serde_json::Value =
        serde_json::from_str(h.config.read().unwrap().get("server_cmd", "[]")).unwrap_or_default();
    let command = commands.as_array()?.get(payload[0] as usize)?;
    Some((
        command.get("cmd")?.as_str()?.to_owned(),
        command
            .get("elevated")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
    ))
}
/// How often completed encoder output is collected while a frame is in flight.
const OUTPUT_POLL: Duration = Duration::from_micros(100);
#[derive(Clone, Copy)]
enum AdmissionWait {
    Grid,
    Capture,
    FreshRate,
    Repeat,
    Encoder,
    Sender,
}
/// TRACE-only elapsed time in admission branches, including any output polling
/// and sending they perform. One aggregate per claim avoids logging every poll.
#[derive(Default)]
struct AdmissionTrace {
    elapsed: [Duration; 6],
    passes: [u32; 6],
}
impl AdmissionTrace {
    fn measure<T>(trace: &mut Option<Self>, reason: AdmissionWait, run: impl FnOnce() -> T) -> T {
        let started = trace.as_ref().map(|_| Instant::now());
        let result = run();
        if let (Some(trace), Some(started)) = (trace, started) {
            let index = reason as usize;
            trace.elapsed[index] += started.elapsed();
            trace.passes[index] = trace.passes[index].saturating_add(1);
        }
        result
    }
    fn micros(&self, reason: AdmissionWait) -> u64 {
        self.elapsed[reason as usize]
            .as_micros()
            .min(u128::from(u64::MAX)) as u64
    }
}
/// How long an encoder that fails mid-stream is recreated before the session
/// gives up: a GPU busy with a game or a driver reset costs frames, not the
/// stream. Moonlight keeps waiting with a frozen picture meanwhile.
const ENCODER_RECOVERY: Duration = Duration::from_secs(20);
const CONTROL_EVENT_LIMIT: usize = 512;
fn control_wait_timeout(remaining: Duration, events: usize) -> i32 {
    if events == CONTROL_EVENT_LIMIT {
        0
    } else {
        remaining.as_micros().div_ceil(1000) as i32
    }
}
fn encoder_progress(
    failing: &mut Option<Instant>,
    produced_frame: bool,
    now: Instant,
) -> Result<()> {
    if produced_frame {
        if let Some(since) = *failing {
            tracing::info!(
                recovery_ms = now.duration_since(since).as_millis(),
                "encoder output resumed"
            );
        }
        *failing = None;
    } else if failing.is_some_and(|since| now.duration_since(since) >= ENCODER_RECOVERY) {
        anyhow::bail!("the encoder did not produce a frame during recovery");
    }
    Ok(())
}
// Keep recovery testable without constructing a GPU or an AMF runtime.
trait EncoderOutput {
    fn poll(&mut self) -> Result<Vec<butterpollo_windows::encoder::Encoded>>;
    fn pending(&self) -> bool;
    fn backlog(&self) -> usize;
    fn backlog_limit(&self) -> usize;
    fn log_stall(&self);
    fn device_removed(&self) -> Option<DeviceLost> {
        None
    }
}
impl EncoderOutput for Encoder {
    fn poll(&mut self) -> Result<Vec<butterpollo_windows::encoder::Encoded>> {
        self.poll()
    }
    fn pending(&self) -> bool {
        self.pending()
    }
    fn backlog(&self) -> usize {
        self.backlog()
    }
    fn backlog_limit(&self) -> usize {
        self.backlog_limit()
    }
    fn log_stall(&self) {
        self.log_stall();
    }
    fn device_removed(&self) -> Option<DeviceLost> {
        self.device_removed()
    }
}
#[derive(Default)]
struct EncoderRecovery {
    failing: Option<Instant>,
    backlog_since: Option<Instant>,
    recreations: u32,
    device_loss: Option<DeviceLost>,
    progress: Option<Arc<crate::stall_watch::Progress>>,
    #[cfg(any(debug_assertions, test))]
    stall: crate::soak_fault::EncoderStall,
}
impl EncoderRecovery {
    fn pending(&self, encoder: &impl EncoderOutput) -> bool {
        if self.device_loss.is_some() {
            return false;
        }
        #[cfg(any(debug_assertions, test))]
        if !self.stall.held.is_empty() {
            return true;
        }
        encoder.pending()
    }
    fn backlog(&self, encoder: &impl EncoderOutput) -> usize {
        let backlog = encoder.backlog();
        #[cfg(any(debug_assertions, test))]
        let backlog = backlog + self.stall.held.len();
        backlog
    }
    fn output(
        &mut self,
        output: Vec<butterpollo_windows::encoder::Encoded>,
        now: impl Fn() -> Instant,
    ) -> Result<Vec<butterpollo_windows::encoder::Encoded>> {
        #[cfg(any(debug_assertions, test))]
        let output = self.stall.output(output, now())?;
        encoder_progress(&mut self.failing, !output.is_empty(), now())?;
        Ok(output)
    }
    /// A recreated codec cannot send pictures retained from its predecessor.
    fn recreated(&mut self, session: &Session) {
        #[cfg(any(debug_assertions, test))]
        self.stall.held.clear();
        session.request_idr();
    }
    /// Output the encoder has finished. Only failures lasting the recovery
    /// budget end the stream; the next frame recreates a dropped encoder.
    fn collect(
        &mut self,
        encoder: &mut Option<impl EncoderOutput>,
        warnings: &butterpollo_core::session::Warnings,
        now: impl Fn() -> Instant,
    ) -> Result<Vec<butterpollo_windows::encoder::Encoded>> {
        let Some(active) = encoder.as_mut() else {
            return Ok(vec![]);
        };
        if let Some(progress) = &self.progress {
            progress.mark(crate::stall_watch::Phase::Output);
        }
        match active.poll() {
            Ok(output) => self.output(output, now),
            Err(error) => {
                if let Some(loss) = active
                    .device_removed()
                    .or_else(|| DeviceLost::from_error(&error))
                {
                    self.device_loss = Some(loss);
                    self.failing.get_or_insert_with(&now);
                    return Ok(vec![]);
                }
                let since = *self.failing.get_or_insert_with(&now);
                if now().duration_since(since) >= ENCODER_RECOVERY {
                    return Err(error.context("the encoder kept failing"));
                }
                warnings.set("encoder_recovery", format!("Encoder output failed ({error:#}); recreating the same encoder while the picture freezes. Lower game GPU load or update the graphics driver if this repeats."));
                active.log_stall();
                *encoder = None;
                Ok(vec![])
            }
        }
    }
    fn poll_full(
        &mut self,
        encoder: &mut Option<impl EncoderOutput>,
        warnings: &butterpollo_core::session::Warnings,
        now: impl Fn() -> Instant,
    ) -> Result<Option<Vec<butterpollo_windows::encoder::Encoded>>> {
        if self.failing.is_none() {
            self.recreations = 0;
        }
        let limit = butterpollo_core::stream_policy::encoder_stall_limit(self.recreations);
        let backlog_since = *self.backlog_since.get_or_insert_with(&now);
        // The host can be descheduled while the GPU finishes its work. Poll
        // before declaring a stall, or a late wake destroys healthy output
        // and unnecessarily restarts the encoder.
        let output = self.collect(encoder, warnings, &now)?;
        if !output.is_empty() {
            self.backlog_since = None;
            return Ok(Some(output));
        }
        // collect can have dropped a failed encoder or requested device
        // recovery. Let the session handle that before looking for a stall.
        if encoder.is_none() || self.device_loss.is_some() {
            self.backlog_since = None;
            return Ok(None);
        }
        if now().duration_since(backlog_since) >= limit {
            self.backlog_since = None;
            if let Some(loss) = encoder.as_ref().unwrap().device_removed() {
                self.device_loss = Some(loss);
                self.failing.get_or_insert_with(&now);
                return Ok(None);
            }
            let since = *self.failing.get_or_insert_with(&now);
            if now().duration_since(since) >= ENCODER_RECOVERY {
                anyhow::bail!("the encoder stopped returning frames");
            }
            warnings.set("encoder_recovery", format!("The encoder returned no frame for {} ms; recreating the same encoder while the picture freezes. Lower game GPU load or update the graphics driver if this repeats.", limit.as_millis()));
            tracing::warn!(
                wait_ms = limit.as_millis(),
                recreation = self.recreations + 1,
                recovery_ms = now().duration_since(since).as_millis(),
                "encoder stall recovery"
            );
            encoder.as_ref().unwrap().log_stall();
            self.recreations += 1;
            *encoder = None;
            return Ok(None);
        }
        Ok(Some(output))
    }
}
/// How long after an AMF reference-invalidation recovery the session sends a
/// keyframe anyway, in case the client's decoder could not follow it.
const RFI_CONFIRM_KEYFRAME: Duration = Duration::from_secs(1);
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
/// `DISPLAY_RETURN_WAIT` does it open the primary display, with a warning.
fn reopen_on(present: bool, missing_since: &mut Option<Instant>, now: Instant) -> ReopenOn {
    if present {
        *missing_since = None;
        return ReopenOn::Stream;
    }
    let since = *missing_since.get_or_insert(now);
    if now.duration_since(since) >= DISPLAY_RETURN_WAIT {
        ReopenOn::Primary
    } else {
        ReopenOn::Wait
    }
}
/// The interval to the frame after one captured at `at` while the client sends phase
/// lock reports: its refresh with the phase correction. `None` otherwise, and then the
/// caller keeps its original pacing.
fn phase_interval(s: &Session, at: Instant, period: Duration) -> Option<Duration> {
    let mut sync = s.phase_sync.lock().unwrap();
    let interval = sync.interval(at, period)?;
    if sync.log_due(at) {
        tracing::info!(
            client = %s.launch.client.name,
            interval_ns = interval.as_nanos() as u64,
            stream_period_ns = period.as_nanos() as u64,
            "phase lock applied"
        );
    }
    Some(interval)
}

/// Whether `overrides` set the peak brightness, under its name before or
/// after it moved out of RTX HDR.
fn sets_peak(overrides: &serde_json::Map<String, serde_json::Value>) -> bool {
    ["hdr_peak_brightness", "rtx_hdr_peak_brightness"]
        .iter()
        .any(|key| {
            overrides
                .get(*key)
                .is_some_and(|value| !value.is_null() && value.as_str() != Some(""))
        })
}

pub(crate) fn effective_config(h: &Shared, launch: &Launch) -> Result<Config> {
    let mut config = h.config.read().unwrap().clone();
    let mut peak_chosen = false;
    if let Some(overrides) = launch
        .client
        .extra
        .get("config_overrides")
        .and_then(serde_json::Value::as_object)
    {
        peak_chosen |= sets_peak(overrides);
        apply_overrides(&mut config, overrides)?;
    }
    if let Some(value) = launch
        .client
        .extra
        .get("prefer_10bit_sdr")
        .filter(|v| !v.is_null() && v.as_str() != Some(""))
    {
        config.values.insert(
            "prefer_sdr_10bit".into(),
            value
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| value.to_string()),
        );
    }
    let app = h
        .apps
        .read()
        .unwrap()
        .iter()
        .find(|app| app.id() == launch.app_id || app.aliases.contains(&launch.app_id))
        .cloned();
    if let Some(app) = &app {
        for (source, target) in [
            ("gamepad", "gamepad"),
            ("dd-configuration-option", "dd_configuration_option"),
            ("prefer-10bit-sdr", "prefer_sdr_10bit"),
            // Saved by Vibepollo and earlier Rubylight apps for RTX HDR;
            // the peak is still what a virtual display reports.
            ("rtx-hdr-peak-brightness", "hdr_peak_brightness"),
        ] {
            if let Some(value) = app
                .extra
                .get(source)
                .filter(|value| !value.is_null() && value.as_str() != Some(""))
            {
                config.values.insert(
                    target.into(),
                    value
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| value.to_string()),
                );
                peak_chosen |= target == "hdr_peak_brightness";
            }
        }
    }
    if let Some(overrides) = app
        .as_ref()
        .and_then(|app| app.extra.get("config-overrides"))
        .and_then(serde_json::Value::as_object)
    {
        peak_chosen |= sets_peak(overrides);
        apply_overrides(&mut config, overrides)?;
    }
    // A device's HDR calibration gives its screen's peak unless the device
    // or app chose one.
    if !peak_chosen
        && let Some(selection) = launch
            .client
            .extra
            .get("hdr_profile")
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty())
    {
        match butterpollo_windows::hdr_profile::peak_luminance(selection) {
            Ok(Some(peak)) => {
                config.values.insert(
                    "hdr_peak_brightness".into(),
                    peak.clamp(400, 2000).to_string(),
                );
            }
            Ok(None) => tracing::debug!(%selection, "HDR calibration has no MHC2 peak"),
            Err(error) => tracing::debug!(%error, %selection, "HDR calibration peak unavailable"),
        }
    }
    // Last, so no override hands the device's keyboard and mouse back to the
    // desktop the person at the PC is using.
    if launch.role == Role::Stream
        && butterpollo_core::background::requested(
            &launch.client.extra,
            app.as_ref().map(|app| &app.extra),
        )
    {
        butterpollo_core::background::apply(&mut config);
    }
    Ok(config)
}
fn apply_overrides(
    config: &mut Config,
    overrides: &serde_json::Map<String, serde_json::Value>,
) -> Result<()> {
    let overrides: serde_json::Map<String, serde_json::Value> = overrides
        .iter()
        .filter(|(_, v)| v.as_str() != Some(""))
        .filter(|(k, _)| !butterpollo_core::config::removed(k))
        .map(|(k, v)| (butterpollo_core::config::current_name(k).to_owned(), v))
        .filter(|(k, _)| {
            let allowed = butterpollo_core::config::override_allowed(k);
            if !allowed {
                tracing::warn!(key = %k, "ignoring an override of a host-wide setting");
            }
            allowed
        })
        .map(|(k, v)| (k, v.clone()))
        .collect();
    // One at a time: an override the host cannot use (an imported frame
    // limit of -1) is skipped, as in the C++ host, instead of failing every
    // stream of the device or app, or the running stream when it is edited.
    let mut overrides = overrides;
    overrides.retain(|key, value| {
        let single = serde_json::Map::from_iter([(key.clone(), value.clone())]);
        match config.update(&single) {
            Ok(()) => true,
            Err(error) => {
                static WARNED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
                let warning = format!("{key}={value}");
                let mut warned = WARNED.lock().unwrap();
                if !warned.contains(&warning) && warned.len() < 64 {
                    warned.push(warning);
                    tracing::warn!(key, %value, error = format!("{error:#}"), "override ignored");
                }
                false
            }
        }
    });
    Ok(())
}
use butterpollo_windows::{
    audio::{Loopback, Opus},
    capture::{Capture, ComGuard, GpuImage, Priority},
    encoder::Encoder,
    input::{Injector, PadReport},
};
use rusty_enet::{Event, Host, HostSettings, Packet, PacketKind, PeerID};
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
pub struct Media {
    video: Arc<UdpSocket>,
    audio: Arc<UdpSocket>,
    control_wake: Arc<UdpSocket>,
    control_wake_receiver: UdpSocket,
    /// The microphone receiver's counts; None when its port could not be bound.
    pub(crate) mic: Option<Arc<crate::mic::Counters>>,
    peers: Mutex<HashMap<(String, bool), SocketAddr>>,
    captures: Mutex<HashMap<CaptureKey, Weak<Source>>>,
    control_port: u16,
    bind: IpAddr,
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
            idd_fence_ring: config.boolean("idd_push_fence_ring", false),
        }
    }
}
/// ENet's UDP adapter ends the control worker on any error but WouldBlock,
/// and nothing restarted it: a Wi-Fi roam (WSAENETUNREACH), an ICMP reply
/// (WSAENETRESET) or an oversized datagram (WSAEMSGSIZE) left every later
/// session without input or control. These lose one datagram; ENet resends
/// what was reliable.
struct ControlSocket {
    socket: UdpSocket,
    /// Whether datagrams are held back, and those held, in order: ENet
    /// sends a packet's acknowledgement before it returns the packet, and
    /// that sendto delayed every input by its cost.
    holding: bool,
    held: Vec<(SocketAddr, Vec<u8>)>,
}
impl ControlSocket {
    fn new(socket: UdpSocket) -> Self {
        Self {
            socket,
            holding: false,
            held: Vec::new(),
        }
    }
    /// Hold back what ENet sends until release().
    fn hold(&mut self) {
        self.holding = true;
    }
    /// Send what was held, in order, and stop holding. A datagram that is
    /// lost is logged; any other error ends the worker, as it would have
    /// inside service().
    fn release(&mut self) -> std::io::Result<()> {
        self.holding = false;
        for (address, buffer) in self.held.drain(..) {
            if let Err(error) = rusty_enet::Socket::send(&mut self.socket, address, &buffer) {
                if !butterpollo_windows::net::datagram_lost(&error) {
                    return Err(error);
                }
                tracing::debug!(%error, %address, "control datagram dropped");
            }
        }
        Ok(())
    }
}
impl rusty_enet::Socket for ControlSocket {
    type Address = SocketAddr;
    type Error = std::io::Error;
    fn init(&mut self, options: rusty_enet::SocketOptions) -> std::io::Result<()> {
        rusty_enet::Socket::init(&mut self.socket, options)
    }
    fn send(&mut self, address: SocketAddr, buffer: &[u8]) -> std::io::Result<usize> {
        if self.holding {
            self.held.push((address, buffer.to_vec()));
            return Ok(buffer.len());
        }
        match rusty_enet::Socket::send(&mut self.socket, address, buffer) {
            Err(error) if butterpollo_windows::net::datagram_lost(&error) => Ok(0),
            result => result,
        }
    }
    fn receive(
        &mut self,
        buffer: &mut [u8; rusty_enet::MTU_MAX],
    ) -> std::io::Result<Option<(SocketAddr, rusty_enet::PacketReceived)>> {
        match rusty_enet::Socket::receive(&mut self.socket, buffer) {
            Err(error) if butterpollo_windows::net::datagram_lost(&error) => Ok(None),
            result => result,
        }
    }
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
        let video = crate::network::udp((bind, ports.video).into())?;
        let audio = crate::network::udp((bind, ports.audio).into())?;
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
            peers: Mutex::new(HashMap::new()),
            captures: Mutex::new(HashMap::new()),
            control_port: ports.control,
            bind,
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
                        match socket.recv_from(&mut b) {
                            Ok((n, peer)) => {
                                let sessions = h.sessions.lock().unwrap();
                                for s in sessions.active.values() {
                                    if s.launch.peer != peer.ip().to_canonical() {
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
                                                    s.launch.peer == peer.ip().to_canonical()
                                                })
                                                .count()
                                                == 1);
                                    if valid {
                                        m.peers
                                            .lock()
                                            .unwrap()
                                            .insert((s.launch.id.clone(), is_audio), peer);
                                        break;
                                    }
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
                            let on = reopen_on(butterpollo_windows::capture::display_present(&next.0), &mut missing_since, Instant::now());
                            let opened = match on {
                                ReopenOn::Wait => {
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
                                let captured = image.captured;
                                image.published = Instant::now();
                                worker.publish_captured(Arc::new(image), captured)?;
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
        let result = self.spawn_session(&h, &s);
        if let Err(e) = result {
            h.sessions.lock().unwrap().fail_start(&s);
            tracing::error!(error=%e,"could not start session worker");
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
    ) -> Result<Option<&'static str>> {
        *encoder = Some(Encoder::new_gpu_reported(
            &s.config,
            c.get("encoder", "auto"),
            first,
            c,
            &s.launch.warnings,
        )?);
        *s.encoder.write().unwrap() = encoder.as_ref().unwrap().backend().into();
        // A rebuilt encoder keeps the hardware family the stream
        // started with: while a GPU recovers, "auto" would fall
        // through to a software encoder and stay there.
        let pinned_backend = encoder
            .as_ref()
            .filter(|e| e.hardware())
            .map(Encoder::backend);
        let source_refresh_hz = butterpollo_windows::display::mode(&first.gpu.display.display_name)
            .ok()
            .map(|mode| mode.dmDisplayFrequency);
        tracing::info!(width=s.config.width,height=s.config.height,fps=f64::from(s.config.fps_millihz())/1000.,requested_fps=s.config.fps,bitrate_kbps=s.bitrate.load(Ordering::Relaxed),configured_bitrate_kbps=s.config.configured_bitrate_kbps,codec=s.config.codec,hdr=s.config.hdr,full_range=s.config.full_range(),color_matrix=s.config.color_matrix(),vrr=s.config.vrr_low_latency,requested_capture=%prepared.capture(),requested_encoder=c.get("encoder","auto"),encoder=encoder.as_ref().unwrap().backend(),adapter=%first.gpu.display.adapter,source_width=first.width,source_height=first.height,source_refresh_hz,source_pixel=?first.pixel,"stream configured");
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
        }
        Ok(pinned_backend)
    }
    fn spawn_audio(
        self: &Arc<Self>,
        h: &Shared,
        s: &Arc<Session>,
    ) -> std::io::Result<thread::JoinHandle<()>> {
        let audio_m = self.clone();
        let audio_h = h.clone();
        let audio_s = s.clone();
        thread::Builder::new().name("audio".into()).spawn(move || {
            if let Err(e) = audio_m.audio(audio_h, audio_s.clone()) {
                audio_s.launch.warnings.set("audio_stopped", format!("Audio stopped ({e:#}); video is still running without sound. Check the playback device and network, then reconnect."));
            }
        })
    }
    fn spawn_session(
        self: &Arc<Self>,
        h: &Shared,
        s: &Arc<Session>,
    ) -> std::io::Result<thread::JoinHandle<()>> {
        let m = self.clone();
        let worker_h = h.clone();
        let worker_s = s.clone();
        thread::Builder::new()
            .name("session".into())
            .spawn(move || {
                let h = worker_h;
                let s = worker_s;
                tracing::info!(client=%s.launch.client.name,"CLIENT CONNECTED");
                let paired_h = h.clone();
                let client_uuid = s.launch.client.uuid.clone();
                let last_seen = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                if let Err(error) = thread::Builder::new().name("client last seen".into()).spawn(move || {
                    let mut paired = paired_h.paired.write().unwrap();
                    if let Some(client) = paired
                        .clients
                        .iter_mut()
                        .find(|c| c.uuid == client_uuid)
                    {
                        let last_seen = last_seen.max(client.extra.get("last_seen").and_then(serde_json::Value::as_u64).unwrap_or(0));
                        client.extra.insert(
                            "last_seen".into(),
                            serde_json::json!(last_seen),
                        );
                        if let Err(error) = paired.save(&paired_h.paired_path) {
                            tracing::warn!(%error,"client last-seen persistence failed");
                        }
                    }
                }) {
                    tracing::warn!(%error, "client last-seen worker failed");
                }
                let mut com = None;
                let mut stream_preparation = None;
                let mut latest = None;
                let mut encoder = None;
                let mut client_commands = None;
                let mut audio = None;
                let mut pyrowave_sender = None;
                let mut keyframe_loss_memory = None;
                #[cfg(test)]
                let mut fixture_preparation = None;
                let result = (|| -> Result<()> {
                    #[cfg(test)]
                    if let Some(fixture) = &h.reconnect_fixture {
                        fixture_preparation = s.launch.preparation.lock().unwrap().take();
                        return fixture.stream(&m, &h, &s);
                    }
                    com = Some(ComGuard::new()?);
                    let _priority = Priority::new();
                    let _streaming = butterpollo_windows::timing::StreamingScope::enter();
                    let mut config_version = h.config_version.load(Ordering::Acquire);
                    let mut c = effective_config(&h, &s.launch)?;
                    if s.config.vrr_low_latency {
                        c.values.insert("wgc_slot_aligned_publish".into(), "false".into());
                    }
                    let timer = butterpollo_windows::timing::Timer::new()?;
                    let output = s
                        .launch
                        .client
                        .extra
                        .get("output_name_override")
                        .and_then(serde_json::Value::as_str)
                        .filter(|s| !s.is_empty())
                        .unwrap_or(c.get("output_name", ""));
                    if s.launch.role == Role::InputOnly {
                        let monitors = butterpollo_windows::input::on_input_desktop(
                            butterpollo_windows::display::monitors,
                        )?;
                        let monitor = monitors
                            .iter()
                            .find(|m| m.matches(output))
                            .or_else(|| monitors.iter().find(|m| m.primary))
                            .context("input display unavailable")?;
                        *s.output.write().unwrap() = monitor.display_name.clone();
                        client_commands = Some(crate::process::ClientCommands::start(&h, &s)?);
                        while !s.stopping() && !h.stop.load(Ordering::Acquire) {
                            thread::sleep(Duration::from_millis(10));
                        }
                        return Ok(());
                    }
                    let initial = s
                        .launch
                        .preparation
                        .lock()
                        .unwrap()
                        .take();
                    // A stream request at another rate than the launch keeps an
                    // owned virtual display and raises its refresh where needed.
                    let initial = match initial {
                        Some(p) if !p.prepared().display.matches(&s.config) => Some(
                            crate::display_session::retime(p.into_prepared(), &h, &s.launch, &s.config, &c)?,
                        ),
                        Some(p) => Some(Ok(p.into_prepared())),
                        None => None,
                    };
                    let stream_preparation = stream_preparation.insert(match initial {
                        Some(Ok(prepared)) => prepared,
                        previous => {
                            if previous.is_some() {
                                tracing::info!(client = %s.launch.client.name, width = s.config.width, height = s.config.height, fps_millihz = s.config.fps_millihz(), hdr = s.config.hdr, "negotiated mode differs from launch; preparing display again");
                            }
                            drop(previous);
                            let previous = h.app_display.lock().unwrap().remove(&s.launch.client.uuid);
                            drop(previous);
                            crate::display_session::prepare_stream(
                                &h, &s.launch, &s.config, &c,
                            )?
                        }
                    });
                    stream_preparation.report_limiter(&s.launch.warnings, &c);
                    let prepared = stream_preparation.display.clone();
                    if s.launch.role == Role::Stream {
                        let mut displays = h.app_display.lock().unwrap();
                        if !s.stopping() {
                            displays.insert(s.launch.client.uuid.clone(), (prepared.clone(), None));
                        }
                    }
                    let output = prepared.output();
                    *s.output.write().unwrap() = output.clone();
                    let (latest, capture_wake) = m.open_session_capture(&s, &c, &prepared, &mut latest)?;
                    let first = {
                        let deadline = Instant::now() + Duration::from_secs(10);
                        loop {
                            // No frame, encoder or filter is owned during startup.
                            latest.release_generation(&capture_wake);
                            if let Some(image) = latest.current()? { break image; }
                            if s.stopping() || h.stop.load(Ordering::Acquire) {
                                return Ok(());
                            }
                            if Instant::now() >= deadline {
                                anyhow::bail!("capture produced no GPU frame on {} using {} within 10 seconds; check that the selected display is powered on, or select a virtual display for headless streaming", prepared.output(), prepared.capture());
                            }
                            if let Some(image) = latest.wait_for_frame(&timer, &capture_wake, (Instant::now() + Duration::from_millis(50)).min(deadline))? { break image; }
                        }
                    };
                    let pinned_backend = m.configure_encoder(&s, &c, &first, &prepared, &mut encoder)?;
                    drop(first);
                    let mut staging = None;
                    client_commands = Some(crate::process::ClientCommands::start(&h, &s)?);
                    audio = Some(m.spawn_audio(&h, &s)?);
                    let requested_fec = c.integer("fec_percentage", 20);
                    if requested_fec != requested_fec.clamp(0, 100) {
                        s.launch.warnings.set("network_fec_config", format!("FEC percentage {requested_fec} is outside 0-100; using {}%. Correct fec_percentage in Video settings.", requested_fec.clamp(0, 100)));
                    }
                    // The configured percentage is adaptive FEC's ceiling; with
                    // `adaptive_fec` off, or a client that sends no FEC status, it
                    // is used for every frame as before.
                    let fec_ceiling = if s.config.codec == 3 {
                        c.integer("pyrowave_critical_fec_percentage", 20).clamp(0, 255) as usize
                    } else { requested_fec.clamp(0, 100) as usize };
                    s.configure_fec(c.boolean("adaptive_fec", true), fec_ceiling);
                    s.configure_adaptive_bitrate(c.boolean("host_adaptive_bitrate", false), c.integer("max_bitrate", 0).clamp(0, if s.config.codec == 3 { 2_000_000 } else { 500_000 }) as u32);
                    // Packet adaptation is opt-in. Explicit pacing and host
                    // bitrate adaptation keep their own policies.
                    let automatic_pacing = c.adaptive_packet_pacing(s.config.codec);
                    s.configure_automatic_pacing(automatic_pacing);
                    keyframe_loss_memory = Some(h.configure_keyframe_loss(&s, c.keyframe_loss_pacing()));
                    let mut packetizer = VideoPacketizer {
                        sequence: 0,
                        iv_counter: 0,
                        frame: 1,
                        packet_size: s.config.packet_size,
                        fec_percent: c.integer("fec_percentage", 20).clamp(0, 100) as usize,
                        min_fec: s.config.min_fec,
                        key: if s.config.encryption & 2 != 0 {
                            Some(s.launch.key)
                        } else {
                            None
                        },
                    };
                    let next_wire_frame = std::cell::Cell::new(u64::from(packetizer.frame));
                    // New pictures claimed and not yet sent, for the stage timings.
                    let claims = std::cell::RefCell::new(butterpollo_core::performance::Claims::default());
                    let start = Instant::now();
                    *s.media_epoch.write().unwrap() = Some(start);
                    let mut present_stamper = (s.config.codec != 3 && prepared.capture() == "wgc").then(butterpollo_windows::present_timing::Stamper::default);
                    pyrowave_sender = if s.config.codec == 3 { Some(crate::pyrowave_send::Sender::new(m.video.clone(),s.clone(),c.clone(),h.clone(),start,prepared.capture() == "wgc")?) } else { None };
                    // The stream's size and rate. A client whose display changes can ask
                    // for others mid-stream (0x5532); the encoder is rebuilt from this copy.
                    let mut stream = s.config.clone();
                    let mut rates = butterpollo_core::reconfigure::Rates::new(&stream, c.get("minimum_fps_target", "20"));
                    // The rate pacing follows, and the period the send path reads.
                    let mut paced_rate = stream.fps_millihz();
                    let stream_period = std::cell::Cell::new(rates.period);
                    // A mode change being applied: the mode before it, and since when.
                    let mut switching: Option<(butterpollo_core::reconfigure::Mode, Instant)> = None;
                    let mut cadence = butterpollo_core::stream_policy::Cadence::new(Instant::now(), rates.period, c.boolean("wgc_pacing_smoothing", true));
                    // VRR claims each frame as it arrives, but no faster than the
                    // stream rate. The encoder budgets every frame from that rate,
                    // and the VRR virtual display runs at 1000 Hz: uncapped, a game
                    // or desktop faster than the stream starved every frame of
                    // bits (a blurry picture, fringed text) and flooded the link.
                    // Generic predictive waits stay off. A stable surplus of WGC
                    // compositions can still be aligned with the game's updates.
                    let vrr = s.config.vrr_low_latency;
                    let arrival_pacing = vrr
                        || butterpollo_core::stream_policy::Pacing::from_config(&c) == butterpollo_core::stream_policy::Pacing::Arrival;
                    let source_phase_eligible = vrr || c.boolean("frame_pacing_predictive", true);
                    let new_pacer = |period: Duration| {
                        let predictive = !vrr && c.boolean("frame_pacing_predictive", true);
                        butterpollo_core::stream_policy::Pacer::new(Instant::now(), period)
                            .with_prediction(predictive)
                            // Source phase follows the opened backend, set per image below.
                            .with_source_phase(false)
                            .with_spacing(if vrr { 0.5 } else { 0.75 })
                    };
                    let mut pacer = new_pacer(rates.period);
                    // The claim grid's period from before a phase lock, restored when it ends.
                    let mut unlocked_grid_period: Option<Duration> = None;
                    let due = cadence.deadline();
                    let mut last_stamp = start;
                    let mut live_at = due;
                    let mut rebuild_encoder = false;
                    // Where this thread is, should it stop making progress.
                    let progress = crate::stall_watch::register(
                        "session",
                        &s.launch.id,
                        Some(crate::stall_watch::GpuProbe::start(&output, &s.launch.id)),
                    );
                    // Since when encoding has failed without a frame getting through.
                    let mut recovery = EncoderRecovery {
                        progress: Some(progress.clone()),
                        ..Default::default()
                    };
                    // Separate encoder failures in this session, each counted once.
                    let (mut failures, mut counted_failure) = (0u32, None);
                    let mut base_config = c.clone();
                    let mut runtime_config = c.clone();
                    let mut calibration_due = s.launch.client.extra.get("hdr_profile")
                        .and_then(serde_json::Value::as_str).filter(|profile| !profile.is_empty())
                        .map(|_| Instant::now() + Duration::from_secs(30));
                    // The fullscreen game on this display, for quitting a game
                    // that a store client started outside the app's processes.
                    let mut quit_scan: Option<butterpollo_windows::foreground::Tracker> = None;
                    let mut quit_scan_due = Instant::now() + Duration::from_secs(1);
                    let _background_windows = if s.launch.role == Role::Stream
                        && butterpollo_core::background::enabled(&c)
                    {
                        background::GameWindows::start(h.clone(), s.clone())
                            .inspect_err(|error| tracing::warn!(%error, "background stream window placement unavailable"))
                            .ok()
                    } else {
                        None
                    };
                    let mut metadata_due = Instant::now() + Duration::from_secs(1);
                    // A keyframe a while after an AMF reference-invalidation recovery.
                    let mut confirm_keyframe: Option<Instant> = None;
                    let mut timing_due = Instant::now() + Duration::from_secs(5);
                    let mut last_image: Option<Arc<GpuImage>> = None;
                    let mut encoded_at = Instant::now();
                    // Keep the legacy age split beside WGC's signed raw stamp
                    // offset: a future stamp must not look like instant delivery.
                    let mut claim_ages: Vec<(u64, u64)> = Vec::with_capacity(1024);
                    let mut wgc_stamp_ages: Vec<f64> = Vec::with_capacity(1024);
                    // The 5 s timing summary is an information log, kept on by
                    // default: a user's ordinary log must show where a stream
                    // lost time. With logging below information, skip collecting
                    // its samples and computing its percentiles.
                    let mut timings_logged = tracing::enabled!(tracing::Level::INFO);
                    // The first pacing decision for the newest fresh frame, kept
                    // for the per-claim trace.
                    let mut first_seen: Option<(usize, Instant, Option<Duration>, Option<Instant>)> = None;
                    let mut video_qos = Tagged::default();
                    let mut batch = butterpollo_windows::net::Batch::default();
                    let mut packets = Vec::new();
                    let trace_send = tracing::enabled!(target: "pacing", tracing::Level::TRACE);
                    let mut admission_trace = trace_send.then(AdmissionTrace::default);
                    let mut trace_attempt = 0_u64;
                    batch.waits = trace_send.then(Default::default);
                    let mut network_pacer = butterpollo_core::network_pacing::Pacer::new(Instant::now());
                    let mut link = None;
                    let mut link_due = Instant::now();
                    let mut reported_pacing = None;
                    // Encoder admission needs the route before its very first
                    // picture, independently of the asynchronous sender's cache.
                    let mut encode_route = None;
                    let mut encode_route_due = Instant::now();
                    let mut recovery_packetizer = packetizer.clone();
                    let mut fec_reported = None;
                    // A frame that holds the stream thread this long is logged
                    // with where the time went, at most every five seconds.
                    const SLOW_SEND: Duration = Duration::from_millis(100);
                    let mut slow_send_reported: Option<Instant> = None;
                    let send_outage = butterpollo_core::stream_policy::SendOutage::from_env();
                    // On a link paced near the stream bitrate, sending a frame
                    // takes a good part of a period; a sender thread lets the
                    // next picture be claimed meanwhile. On a fast link the
                    // inline send is shorter than the handoff costs.
                    let video_sender = std::cell::OnceCell::<crate::video_send::Sender>::new();
                    let send_thread = c.boolean("video_send_thread", true);
                    let mut sender_decided = false;
                    // An inline frame was paced near the stream bitrate, as after
                    // a lost keyframe: later frames move to the sender thread.
                    let mut inline_paced = false;
                    let mut send_loss = butterpollo_core::stream_policy::SendLossRecovery::default();
                    let batch_kb = match c.integer("video_max_batch_size_kb", 64) {
                        16 => 16,
                        32 => 32,
                        _ => 64,
                    };
                    let mut send_frames = |output: Vec<butterpollo_windows::encoder::Encoded>,
                                           peer: std::net::SocketAddr,
                                           call_latency: Duration,
                                           source: &Source,
                                           processing_started: Instant|
                     -> Result<()> {
                        progress.mark(crate::stall_watch::Phase::Send);
                        if !output.is_empty() {
                            s.launch.warnings.clear("encoder_recovery");
                            source.device_recovered(&source.warnings);
                        }
                        if let Some(sender) = &pyrowave_sender { return sender.submit(output,peer,call_latency,processing_started); }
                        // Feedback can lower the rate between outputs. Move to
                        // the sender before that first slow frame blocks capture.
                        let loss_paced = send_thread && video_sender.get().is_none() && s.keyframe_loss_active();
                        if send_thread && (!sender_decided || inline_paced || loss_paced) {
                            let mid_stream = std::mem::take(&mut inline_paced) || (sender_decided && loss_paced);
                            sender_decided = true;
                            let route = *link.get_or_insert_with(|| butterpollo_windows::net::routed_link(peer));
                            packetizer.fec_percent = s.fec_percent();
                            let bitrate = s.bitrate.load(Ordering::Relaxed);
                            let crate::video_send::recovery::Pace { bps, needed } =
                                crate::video_send::recovery::pace(&s, &c, &packetizer, stream_period.get(), peer, route);
                            if mid_stream || automatic_pacing || c.boolean("host_adaptive_bitrate", false) || butterpollo_core::network_pacing::paced(bps, needed) {
                                // The sender continues the inline sends' frame,
                                // packet and IV numbering; nothing sends inline
                                // after this.
                                let handoff = crate::video_send::Handoff {
                                    packetizer: packetizer.clone(),
                                    present_stamper: present_stamper.take(),
                                    last_stamp,
                                    pacing_due: network_pacer.due(),
                                    send_loss: std::mem::take(&mut send_loss),
                                };
                                let _ = video_sender.set(crate::video_send::Sender::new(m.video.clone(), s.clone(), c.clone(), h.clone(), start, handoff)?);
                                tracing::info!(pacing_bps = bps, bitrate_kbps = bitrate, wire_budget_bps = needed, automatic_pacing, mid_stream, wire_frame = packetizer.frame, "video frames are sent on their own thread for current or adaptive pacing");
                            }
                        }
                        if let Some(sender) = video_sender.get() { return sender.submit_tick(output, peer, call_latency, &mut claims.borrow_mut(), processing_started); }
                        let polled = Instant::now();
                        let micros = |d: Duration| d.as_micros().min(u128::from(u64::MAX)) as u64;
                        'frames: for frame in output {
                            let frame_started = Instant::now();
                            let encode = frame.latency.unwrap_or(call_latency);
                            let latency = micros(encode);
                            s.stats.latency_us.store(latency, Ordering::Relaxed);
                            let claimed = polled.checked_sub(encode).unwrap_or(polled);
                            let captured = frame.presentation.unwrap_or(claimed);
                            let age = micros(claimed.saturating_duration_since(captured));
                            let claim = claims.borrow_mut().take(frame.presentation);
                            let stamp_started = Instant::now();
                            let stamp = present_stamper.as_mut().map_or(captured, |stamper| stamper.stamp(captured, &prepared.output())).max(last_stamp + Duration::from_nanos(11_112));
                            let stamp_time = stamp_started.elapsed();
                            last_stamp = stamp;
                            // Wrap like the previous host; a saturating cast
                            // froze the clock after 13.25 hours.
                            let timestamp = (stamp.saturating_duration_since(start).as_secs_f64() * 90000.) as u64 as u32;
                            if frame.bytes.is_empty() {
                                continue;
                            }
                            packetizer.fec_percent = s.frame_fec_percent(frame.idr || frame.after_invalidation);
                            let wire_frame = packetizer.frame;
                            let block_layout = packetizer.block_layout(frame.bytes.len());
                            let fec_limited = packetizer.fec_limited(frame.bytes.len());
                            let packet_bytes = packetizer.packet_size + 16 + if packetizer.key.is_some() { 32 } else { 0 };
                            let route = *link.get_or_insert_with(|| butterpollo_windows::net::routed_link(peer));
                            let bitrate = s.bitrate.load(Ordering::Relaxed);
                            let crate::video_send::recovery::Pace { bps, needed } =
                                crate::video_send::recovery::pace(&s, &c, &packetizer, stream_period.get(), peer, route);
                            let safe = butterpollo_core::network_pacing::rate_bps(0, bitrate, route.bps, true, needed);
                            let bps = s.keyframe_loss_bps(bps, safe, wire_frame, frame.idr);
                            inline_paced |= butterpollo_core::network_pacing::paced(bps, needed);
                            if Instant::now() >= link_due {
                                butterpollo_core::network_pacing::report_rate(&s.launch.warnings, bps, needed, c.integer("pacing_max_bitrate_kbps", 0));
                                if reported_pacing != Some(bps) {
                                    tracing::info!(pacing_bps=bps, wire_budget_bps=needed, link_bps=route.bps, configured_kbps=c.integer("pacing_max_bitrate_kbps", 0), "network pacing selected");
                                    reported_pacing = Some(bps);
                                }
                            }
                            // A frame beyond Moonlight's packet limit (very high
                            // bitrates) costs that frame and a keyframe, not the
                            // session.
                            let packetize_started = Instant::now();
                            let processing = micros(packetize_started.saturating_duration_since(processing_started));
                            let prepared_blocks = packetizer.conventional_blocks(&frame.bytes,frame.idr,frame.after_invalidation,timestamp,processing);
                            if let Err(error) = &prepared_blocks {
                                s.launch.warnings.event("network_frame", format!("Encoded video frame dropped ({error:#}); requesting a recovery frame. Lower bitrate or resolution to stay within Moonlight's packet limit."), butterpollo_core::session::EVENT_PERIOD);
                                s.request_idr();
                                drop(prepared_blocks);
                                next_wire_frame.set(u64::from(packetizer.frame));
                                continue;
                            }
                            let (packet_count, mut next_block) = prepared_blocks?;
                            let mut packetize_time = packetize_started.elapsed();
                            tracing::trace!(
                                target: "pacing",
                                stream_id = %s.launch.id,
                                frame = wire_frame,
                                presentation = ?frame.presentation,
                                "packetized picture"
                            );
                            // A keyframe, as at the start of every stream, may be too
                            // large for FEC; only ordinary frames make this worth showing.
                            if !frame.idr
                                && fec_limited
                                && fec_reported.is_none_or(|at: Instant| at.elapsed() >= Duration::from_secs(1))
                            {
                                fec_reported = Some(Instant::now());
                                s.launch.warnings.event("network_fec", "FEC was reduced or omitted for large video frames because they exceed Moonlight's four-block limit. Packet loss in those frames is harder to recover; lower bitrate or resolution to keep full FEC protection.", butterpollo_core::session::EVENT_PERIOD);
                            }
                            let frame_bytes = (packet_count * packet_bytes) as u64;
                            let dropped = batch.dropped;
                            let waits_before = batch.waits;
                            let mut pacing_wait = Duration::ZERO;
                            let mut send_time = Duration::ZERO;
                            let mut batches = 0;
                            let mut first_send = None;
                            let mut last_send = None;
                            let mut first_sent = None;
                            let mut lost = butterpollo_core::stream_policy::SendLoss::new(block_layout);
                            let mut abandoned = 0;
                            let mut sent_packets = 0;
                            'blocks: loop {
                                let block_started = Instant::now();
                                let block_count = match next_block(&mut packets) {
                                    Ok(count) => count,
                                    Err(error) => {
                                        s.launch.warnings.event("network_frame", format!("Encoded video frame dropped ({error:#}); requesting a recovery frame. Lower bitrate or resolution to stay within Moonlight's packet limit."), butterpollo_core::session::EVENT_PERIOD);
                                        s.request_idr();
                                        drop(next_block);
                                        next_wire_frame.set(u64::from(packetizer.frame));
                                        continue 'frames;
                                    }
                                };
                                packetize_time += block_started.elapsed();
                                if block_count == 0 { break; }
                                next_wire_frame.set(u64::from(wire_frame.wrapping_add(1)));
                                s.stats.video_frame.store(u64::from(wire_frame) + 1, Ordering::Release);
                                let mut remaining = &packets[..block_count];
                                while !remaining.is_empty() {
                                    if s.stopping() || h.stop.load(Ordering::Acquire) {
                                        return Ok(());
                                    }
                                    let now = Instant::now();
                                    if network_pacer.due() > now {
                                        timer.until_paced(network_pacer.due());
                                        pacing_wait += now.elapsed();
                                    }
                                    let budget = butterpollo_core::network_pacing::batch_payload_limit(
                                        bps, remaining[0].len(), peer.is_ipv6(), batch_kb * 1024,
                                    );
                                    let count =
                                        butterpollo_windows::net::Batch::count(remaining, budget);
                                    let send_started = Instant::now();
                                    let refused = batch.dropped;
                                    let bytes = match send_outage.filter(|outage| outage.active(start, Instant::now())) {
                                        // Lost after the socket: counted as sent, only the client sees it.
                                        Some(outage) if outage.in_air() => remaining[..count].iter().map(Vec::len).sum(),
                                        Some(_) => {
                                            batch.refuse(count);
                                            0
                                        }
                                        None => batch.send(&m.video, &remaining[..count], peer)?,
                                    };
                                    let finished = Instant::now();
                                    first_sent.get_or_insert(finished);
                                    send_time += finished.duration_since(send_started);
                                    if trace_send {
                                        first_send.get_or_insert(send_started);
                                        last_send = Some(finished);
                                        batches += 1;
                                    }
                                    remaining = &remaining[count..];
                                    sent_packets += count;
                                    network_pacer.sent(Instant::now(), bytes, if bytes > 0 { count } else { 0 }, peer.ip().to_canonical().is_ipv6(), bps);
                                    s.stats.packets.fetch_add(count as u64, Ordering::Relaxed);
                                    s.stats.bytes.fetch_add(bytes as u64, Ordering::Relaxed);
                                    // The socket refused more of this frame than FEC
                                    // repairs: the client cannot decode it. Stop
                                    // spending the link on it, and make the next
                                    // frame the keyframe the client will wait for.
                                    if lost.sent(count, (batch.dropped - refused) as usize) {
                                        abandoned = packet_count - sent_packets;
                                        if send_loss.lost(Instant::now(), stream_period.get(), lost.reached_socket()) {
                                            s.request_send_loss_recovery();
                                        }
                                        break 'blocks;
                                    }
                                }
                            }
                            drop(next_block);
                            if lost.unrecoverable() {
                                tracing::debug!(frame = packetizer.frame.wrapping_sub(1), idr = frame.idr, abandoned, "video frame lost on send; the rest of it was not sent");
                            } else {
                                send_loss.delivered(frame.idr);
                            }
                            if batch.dropped != dropped {
                                s.launch.warnings.event("network_send", "Video packets were dropped by the host after transient socket send failures. You may see stutter or recovery frames; lower bitrate and check the network adapter. The log includes the socket error code.", butterpollo_core::session::EVENT_PERIOD);
                            }
                            s.stats.frames.fetch_add(1, Ordering::Relaxed);
                            let sent = Instant::now();
                            crate::video_send::recovery::report(&s, &c, crate::video_send::recovery::RecoveryFrame {
                                frame: wire_frame, idr: frame.idr, after_invalidation: frame.after_invalidation,
                                payload_bytes: frame.bytes.len(), cap_bytes: frame.frame_cap_bytes,
                                wire_bytes: frame_bytes.saturating_add((packet_count * butterpollo_core::network_pacing::overhead(peer.is_ipv6())) as u64),
                                pace_bps: bps, elapsed: sent.saturating_duration_since(frame_started), lost: lost.unrecoverable(),
                            });
                            if let (Some(first), Some(last), Some(before), Some(after)) = (first_send, last_send, waits_before, batch.waits) {
                                tracing::trace!(target: "pacing", frame=packetizer.frame.wrapping_sub(1), packets=packet_count, batches,
                                    pacing_wait_us=micros(pacing_wait), send_us=micros(send_time),
                                    first_send_us=micros(first.saturating_duration_since(start)), last_send_us=micros(last.saturating_duration_since(start)),
                                    writable_waits=after.count-before.count, writable_wait_us=micros(after.elapsed-before.elapsed),
                                    dropped=batch.dropped-dropped, stream_id=%s.launch.id, "send");
                            }
                            {
                                let mut performance = s.stats.performance.lock().unwrap();
                                performance.record_timing(sent,butterpollo_core::performance::Timing{period:stream_period.get(),encode:latency,host:processing,age,sent:micros(sent.saturating_duration_since(claimed))},frame_bytes);
                                // A frame abandoned on send never had its last packet sent.
                                if let Some(claim) = claim.filter(|_| !lost.unrecoverable()) {
                                    let stamps = claim.sent(polled, packetize_started, first_sent.unwrap_or(sent), sent);
                                    performance.record_stages(&stamps, processing);
                                    s.host_timings.lock().unwrap().record(wire_frame, &stamps);
                                }
                            }
                            let record_time = sent.elapsed();
                            // The interface lookup takes a moment: refresh the
                            // link speed after the frame is out, for the next one.
                            let link_started = Instant::now();
                            if Instant::now() >= link_due {
                                link = Some(butterpollo_windows::net::routed_link(peer));
                                link_due = Instant::now() + Duration::from_secs(2);
                            }
                            let link_time = link_started.elapsed();
                            let took = frame_started.elapsed();
                            if took >= SLOW_SEND && slow_send_reported.is_none_or(|at| at.elapsed() >= Duration::from_secs(5)) {
                                slow_send_reported = Some(Instant::now());
                                let ms = |d: Duration| (d.as_secs_f64() * 10_000.).round() / 10.;
                                tracing::warn!(
                                    frame_ms = ms(took), socket_ms = ms(send_time), pacing_ms = ms(pacing_wait),
                                    timestamp_ms = ms(stamp_time), packetize_ms = ms(packetize_time),
                                    timings_ms = ms(record_time), link_ms = ms(link_time),
                                    packets = packet_count, bytes = frame_bytes, pacing_bps = bps, idr = frame.idr,
                                    "a video frame held the stream thread while it was sent; socket_ms is time in Windows' send calls (network driver and adapter), the other parts are the host's own work"
                                );
                            }
                        }
                        Ok(())
                    };
                    (|| -> Result<()> {
                        while !s.stopping() && !h.stop.load(Ordering::Acquire) {
                            progress.mark(crate::stall_watch::Phase::Loop);
                            if let Some(sender) = video_sender.get() { sender.backlog()?; }
                            if let Some(loss) = recovery.device_loss.take() {
                                latest.latest.device_lost(loss, &latest.warnings, Instant::now())?;
                                encoder = None;
                            }
                            if Instant::now() >= live_at {
                                live_at = Instant::now() + Duration::from_millis(250);
                                let version = h.config_version.load(Ordering::Acquire);
                                if version != config_version || calibration_due.is_some_and(|due| Instant::now() >= due) {
                                    base_config = effective_config(&h, &s.launch)?;
                                    if s.config.vrr_low_latency {
                                        base_config.values.insert("wgc_slot_aligned_publish".into(), "false".into());
                                    }
                                    config_version = version;
                                    runtime_config = base_config.clone();
                                    if let Some(due) = &mut calibration_due {
                                        *due = Instant::now() + Duration::from_secs(30);
                                    }
                                }
                                *s.output.write().unwrap() = prepared.output();
                                if Instant::now() >= timing_due {
                                    timing_due = Instant::now() + Duration::from_secs(5);
                                    if timings_logged {
                                        // Sorted after the lock the sender records into is released.
                                        let window = s.stats.performance.lock().unwrap().window(Instant::now());
                                        let timing = window.snapshot();
                                        let ms = |key: &str| timing[key].as_f64().unwrap_or(0.);
                                        let split = |pick: fn(&(u64, u64)) -> u64| {
                                            let mut values: Vec<u64> = claim_ages.iter().map(pick).collect();
                                            values.sort_unstable();
                                            let mean = values.iter().sum::<u64>() as f64 / values.len().max(1) as f64 / 1000.;
                                            let p95 = values.get(values.len().saturating_sub(1) * 95 / 100).copied().unwrap_or(0) as f64 / 1000.;
                                            (mean, p95)
                                        };
                                        let (detect_mean_ms, detect_p95_ms) = split(|age| age.0);
                                        let (claim_wait_mean_ms, claim_wait_p95_ms) = split(|age| age.1);
                                        wgc_stamp_ages.sort_by(f64::total_cmp);
                                        let wgc_stamp_frames = wgc_stamp_ages.len();
                                        let wgc_stamp_future_frames = wgc_stamp_ages.iter().filter(|age| **age < 0.).count();
                                        let wgc_stamp_to_host_mean_ms = (wgc_stamp_frames > 0).then(|| wgc_stamp_ages.iter().sum::<f64>() / wgc_stamp_frames as f64);
                                        let wgc_stamp_to_host_p95_ms = wgc_stamp_ages.get(wgc_stamp_frames.saturating_sub(1) * 95 / 100).copied();
                                        // New pictures' stages over the last five seconds: mean/p95/p99 ms.
                                        let stage = |name: &str| {
                                            let ms = |key: &str| timing["stages"][name][key].as_f64().unwrap_or(0.);
                                            format!("{:.2}/{:.2}/{:.2}", ms("mean_ms"), ms("p95_ms"), ms("p99_ms"))
                                        };

                                        tracing::info!(
                                            fps=ms("fps"),
                                            host_mean_ms=ms("host_processing_mean_ms"),
                                            host_p95_ms=ms("host_processing_p95_ms"),
                                            host_p99_ms=ms("host_processing_p99_ms"),
                                            host_max_ms=ms("host_processing_max_ms"),
                                            encode_mean_ms=ms("encode_mean_ms"),
                                            encode_p95_ms=ms("encode_p95_ms"),
                                            encode_p99_ms=ms("encode_p99_ms"),
                                            frame_age_mean_ms=ms("frame_age_mean_ms"),
                                            frame_age_p95_ms=ms("frame_age_p95_ms"),
                                            present_to_send_mean_ms=ms("present_to_send_mean_ms"),
                                            present_to_send_p99_ms=ms("present_to_send_p99_ms"),
                                            present_to_sent_ms=%stage("present_to_sent"),
                                            stage_capture_ms=%stage("capture"),
                                            stage_hold_ms=%stage("hold"),
                                            stage_claim_ms=%stage("claim"),
                                            stage_convert_ms=%stage("convert"),
                                            stage_encode_ms=%stage("encode"),
                                            stage_queue_ms=%stage("queue"),
                                            stage_packetize_ms=%stage("packetize"),
                                            stage_send_queue_ms=%stage("send_queue"),
                                            stage_send_ms=%stage("send"),
                                            moonlight_host_ms=%stage("moonlight_host"),
                                            stage_frames=timing["stages"]["frames"].as_u64().unwrap_or(0),
                                            detect_mean_ms,
                                            detect_p95_ms,
                                            claim_wait_mean_ms,
                                            claim_wait_p95_ms,
                                            wgc_stamp_to_host_mean_ms,
                                            wgc_stamp_to_host_p95_ms,
                                            wgc_stamp_frames,
                                            wgc_stamp_future_frames,

                                            send_interval_p95_ms=timing["send_interval_p95_ms"].as_f64().unwrap_or(0.),
                                            send_interval_p99_ms=timing["send_interval_p99_ms"].as_f64().unwrap_or(0.),
                                            send_interval_max_ms=timing["send_interval_max_ms"].as_f64().unwrap_or(0.),
                                            // Keep reference feedback distinct from IDR recovery.
                                            idr_requests=s.stats.idr_requests.load(Ordering::Relaxed),
                                            reference_invalidations=s.stats.reference_invalidations.load(Ordering::Relaxed),
                                            send_loss_recoveries=s.stats.send_loss_recoveries.load(Ordering::Relaxed),
                                            fec_reports=timing["fec_reports"].as_u64().unwrap_or(0),
                                            fec_invalid_reports=timing["fec_invalid_reports"].as_u64().unwrap_or(0),
                                            fec_duplicate_reports=timing["fec_duplicate_reports"].as_u64().unwrap_or(0),
                                            fec_recovered_frames=timing["fec_recovered_frames"].as_u64().unwrap_or(0),
                                            fec_unrecoverable_frames=timing["fec_unrecoverable_frames"].as_u64().unwrap_or(0),
                                            fec_missing_packets=timing["fec_missing_packets"].as_u64().unwrap_or(0),
                                            bitrate_kbps=s.bitrate.load(Ordering::Relaxed),
                                            sent_mbps=ms("bitrate_mbps"),
                                            "stream timings"
                                        );
                                    }
                                    claim_ages.clear();
                                    wgc_stamp_ages.clear();
                                    timings_logged = tracing::enabled!(tracing::Level::INFO);
                                }
                            }
                            // The client's display changed and it asked for another size or
                            // rate (0x5532). Its latest request is applied here, between
                            // frames: the encoder is rebuilt at the new size and starts it
                            // with a keyframe. The display keeps its mode; the colour
                            // conversion scales its picture to the new size.
                            let requested = s.reconfigure.lock().unwrap().take_due(Instant::now());
                            if let Some(to) = requested {
                                let from = butterpollo_core::reconfigure::Mode::of(&stream);
                                match butterpollo_core::reconfigure::refusal(&stream, runtime_config.boolean("stream_reconfigure", true)) {
                                    Some(reason) => {
                                        s.reconfigure.lock().unwrap().refused(from);
                                        tracing::info!(client = %s.launch.client.name, from = %from, to = %to, reason, "reconfigure refused");
                                    }
                                    None => {
                                        tracing::info!(client = %s.launch.client.name, from = %from, to = %to, "reconfigure requested");
                                        to.apply_to(&mut stream);
                                        switching.get_or_insert((from, Instant::now()));
                                        rebuild_encoder = true;
                                    }
                                }
                            }
                            // Pacing follows the stream's rate: a new rate starts a new
                            // cadence, and a phase lock locks again at it.
                            if stream.fps_millihz() != paced_rate {
                                paced_rate = stream.fps_millihz();
                                rates = butterpollo_core::reconfigure::Rates::new(&stream, c.get("minimum_fps_target", "20"));
                                stream_period.set(rates.period);
                                cadence = butterpollo_core::stream_policy::Cadence::new(Instant::now(), rates.period, c.boolean("wgc_pacing_smoothing", true));
                                pacer = new_pacer(rates.period);
                                unlocked_grid_period = None;
                                {
                                    let mut grid = latest.grid.lock().unwrap();
                                    grid.period = rates.period;
                                    grid.anchor = Instant::now();
                                }
                                s.phase_sync.lock().unwrap().reset();
                            }
                            let period = stream_period.get();
                            latest.check()?;
                            let peer = m
                                .peers
                                .lock()
                                .unwrap()
                                .get(&(s.launch.id.clone(), false))
                                .copied();
                            let Some(peer) = peer else {
                                if start.elapsed() > crate::network::ping_timeout(&c) {
                                    anyhow::bail!("client video ping timed out");
                                }
                                timer.until(Instant::now() + Duration::from_millis(1));
                                continue;
                            };
                            if s.config.video_qos {
                                video_qos.follow(&m.video, peer, false);
                            }
                            let now = Instant::now();
                            let due = cadence.deadline();
                            if !arrival_pacing && now < due {
                                AdmissionTrace::measure(&mut admission_trace, AdmissionWait::Grid, || -> Result<()> {
                                while Instant::now() < due {
                                    if encoder.as_ref().is_some_and(|e| recovery.pending(e)) {
                                        let poll_tick = Instant::now();
                                        send_frames(recovery.collect(&mut encoder, &s.launch.warnings, Instant::now)?, peer, Duration::ZERO, latest, poll_tick)?;
                                        if encoder.as_ref().is_some_and(|e| recovery.pending(e)) {
                                            timer.until(
                                                (Instant::now() + OUTPUT_POLL)
                                                    .min(due),
                                            );
                                        }
                                    } else {
                                        timer.until(due);
                                    }
                                }
                                Ok(())
                                })?;
                            }
                            progress.mark(crate::stall_watch::Phase::Picture);
                            let capture_tick = Instant::now();
                            let image = AdmissionTrace::measure(&mut admission_trace, AdmissionWait::Capture, || {
                                latest.wait_for_frame(&timer, &capture_wake, Instant::now() + period.min(Duration::from_millis(50)))
                            })?;
                            let Some(mut image) = image else {
                                // The capture is being re-created on a new device.
                                // Release everything on the old one so Windows can
                                // give the new capture a current adapter.
                                if encoder.take().is_some() {
                                    tracing::info!(client = %s.launch.client.name, "releasing the encoder while the capture recovers");
                                }
                                last_image = None;
                                staging = None;
                                pacer.reset_source_phase();
                                latest.release_generation(&capture_wake);
                                continue;
                            };
                            // Resolution changes or DXGI loss can recreate the capture device.
                            rebuild_encoder |= encoder
                                .as_ref()
                                .is_none_or(|encoder| !encoder.accepts_gpu_device(&image));
                            if rebuild_encoder {
                                pacer.reset_source_phase();
                            }
                            // A reopen can change the backend, e.g. to WGC after the
                            // driver's frame push failed.
                            pacer.set_source_phase(source_phase_eligible && butterpollo_core::stream_policy::source_phase(&c, latest.backend()));
                            if arrival_pacing {
                                pacer.observe_source(image.captured);
                                pacer.observe_source_rate(latest.source_rate());
                            }
                            let fresh = last_image.as_ref().is_none_or(|previous| !Arc::ptr_eq(previous, &image));
                            if arrival_pacing
                                && fresh
                                && !rebuild_encoder
                                && !s.idr.load(Ordering::Acquire)
                                && s.invalidation.lock().unwrap().is_none()
                                && let interval = latest.source_interval()
                                && let butterpollo_core::stream_policy::Pace::WaitUntil(deadline) =
                                    pacer.decide(Instant::now(), image.captured, interval)
                            {
                                AdmissionTrace::measure(&mut admission_trace, AdmissionWait::FreshRate, || -> Result<()> {
                                let key = Arc::as_ptr(&image) as usize;
                                if first_seen.is_none_or(|(seen, ..)| seen != key) {
                                    first_seen = Some((key, Instant::now(), interval, Some(deadline)));
                                }
                                // A poll can block for the encoder's 1 ms query
                                // timeout; close to the claim it would overshoot.
                                if encoder.as_ref().is_some_and(|e| recovery.pending(e))
                                    && deadline.saturating_duration_since(Instant::now()) >= Duration::from_millis(1)
                                {
                                    send_frames(recovery.collect(&mut encoder, &s.launch.warnings, Instant::now)?, peer, Duration::ZERO, latest, capture_tick)?;
                                }
                                let now = Instant::now();
                                let until = if encoder.as_ref().is_some_and(|e| recovery.pending(e))
                                    && deadline.saturating_duration_since(now) >= Duration::from_millis(1)
                                {
                                    deadline.min(now + OUTPUT_POLL)
                                } else {
                                    deadline
                                };
                                // The claim itself is met precisely; a poll for the
                                // encoder's output is not worth spinning for.
                                if until == deadline {
                                    latest.wait_if_current_precise(&timer, &capture_wake, &image, until)?;
                                } else {
                                    latest.wait_if_current(&timer, &capture_wake, &image, until)?;
                                }
                                Ok(())
                                })?;
                                continue;
                            }
                            let repeat_due = if arrival_pacing {
                                pacer.repeat_deadline(encoded_at + rates.static_period, image.captured, latest.source_interval())
                            } else {
                                encoded_at + rates.static_period
                            };
                            // A recovery request on a moving picture rides the
                            // next new frame; only a still one is encoded again
                            // at once. Grid pacing serves it at the next slot.
                            let recovering = s.idr.load(Ordering::Acquire) || s.invalidation.lock().unwrap().is_some();
                            let reencode_at = if !recovering {
                                repeat_due
                            } else if arrival_pacing {
                                butterpollo_core::stream_policy::reencode_at(true, image.captured, period, latest.source_interval(), repeat_due)
                            } else {
                                Instant::now()
                            };
                            if !rebuild_encoder
                                && (s.config.vrr_low_latency || rates.limit_static_rate || arrival_pacing)
                                && last_image
                                    .as_ref()
                                    .is_some_and(|previous| Arc::ptr_eq(previous, &image))
                                && Instant::now() < reencode_at
                            {
                                AdmissionTrace::measure(&mut admission_trace, AdmissionWait::Repeat, || -> Result<()> {
                                if encoder.as_ref().is_some_and(|e| recovery.pending(e)) { send_frames(recovery.collect(&mut encoder, &s.launch.warnings, Instant::now)?, peer, Duration::ZERO, latest, capture_tick)?; }
                                let wait = if encoder.as_ref().is_some_and(|e| recovery.pending(e)) { OUTPUT_POLL } else { period };
                                latest.wait_if_current(&timer, &capture_wake, &image, Instant::now() + wait.min(reencode_at.saturating_duration_since(Instant::now())))?;
                                Ok(())
                                })?;
                                continue;
                            }
                            // A picture claimed while the encoder is behind only
                            // waits in its queue: take its output first, then
                            // claim the newest picture.
                            // A frame still being sent on a paced link counts too.
                            // A frame waiting behind it is skipped on a keyframe
                            // request: the client would discard it, and the
                            // keyframe would wait for it.
                            let recovery_lead = encoder.as_ref().and_then(Encoder::recovery_lead).map(|lead| lead.min(period));
                            let sending = match video_sender.get() {
                                Some(sender) => {
                                    if s.idr.load(Ordering::Acquire) { sender.skip_pending()?; }
                                    // Claim the newest picture near the end of an IDR,
                                    // only when this encoder demonstrated a safe lead.
                                    sender.claim_backlog(recovery_lead)?
                                },
                                None => 0,
                            };
                            let encoding = encoder.as_ref().map_or(0, |e| recovery.backlog(e));
                            let encoding_limit = encoder.as_ref().map_or(encoder_backlog::DEFAULT_LIMIT, |encoder| {
                                if video_sender.get().is_some() { encoder.recovery_backlog_limit() } else { EncoderOutput::backlog_limit(encoder) }
                            });
                            let pressure = encoder_backlog::backpressure(encoding, sending, encoding_limit);
                            if !rebuild_encoder && pressure != Backpressure::None {
                                if pressure == Backpressure::Sender {
                                    AdmissionTrace::measure(&mut admission_trace, AdmissionWait::Sender, || -> Result<()> {
                                    // Network occupancy must not start encoder stall recovery.
                                    recovery.backlog_since = None;
                                    if encoder.as_ref().is_some_and(|e| recovery.pending(e)) {
                                        send_frames(recovery.collect(&mut encoder, &s.launch.warnings, Instant::now)?, peer, Duration::ZERO, latest, capture_tick)?;
                                    } else {
                                        video_sender.get().unwrap().wait_for_capacity(encoder_backlog::total_limit(encoding_limit), period, recovery_lead)?;
                                    }
                                    Ok(())
                                    })?;
                                    continue;
                                }
                                // An encoder that returns nothing for 250 ms is
                                // recreated, as a queue that never drained was;
                                // a shorter stall on a saturated GPU only makes the
                                // stream choppy. A recreated one that is still
                                // silent gets longer each time.
                                AdmissionTrace::measure(&mut admission_trace, AdmissionWait::Encoder, || -> Result<()> {
                                    if let Some(output) = recovery.poll_full(&mut encoder, &s.launch.warnings, Instant::now)? {
                                        send_frames(output, peer, Duration::ZERO, latest, capture_tick)?;
                                    }
                                    Ok(())
                                })?;
                                continue;
                            }
                            recovery.backlog_since = None;
                            if s.launch.role == Role::Stream && Instant::now() >= quit_scan_due {
                                quit_scan_due = Instant::now() + Duration::from_secs(1);
                                // The scan runs on its own thread; this reads its last result.
                                if let Some((program, pid)) = quit_scan
                                    .get_or_insert_with(butterpollo_windows::foreground::Tracker::default)
                                    .poll_process(&[], &image.gpu.display)
                                    && let Some(app) = h.current_app.lock().unwrap().as_mut()
                                    // Only the stream of the client that launched the app:
                                    // another client's display shows its own programs.
                                    // An app started from the console has no owner.
                                    && (app.owner.is_empty() || app.owner == s.launch.client.uuid)
                                {
                                    app.observe_foreground(pid, &program);
                                }
                            }
                            let rebuilt = rebuild_encoder;
                            if rebuild_encoder {
                                encoder = None;
                                // Some drivers accept the compute path at creation and
                                // fail on it later, every time: after a second failure
                                // in the session, convert on the graphics queue. One
                                // failure (a game holding the GPU, a driver reset) must
                                // not cost the rest of the session the slower path,
                                // 6-9 ms a frame beside a GPU-bound game.
                                if let Some(since) = recovery.failing
                                    && counted_failure != Some(since)
                                {
                                    counted_failure = Some(since);
                                    failures += 1;
                                }
                                let mut tuning = c.clone();
                                if failures >= 2 {
                                    tuning.values.insert("gpu_compute_conversion".into(), "false".into());
                                    if butterpollo_windows::compute::enabled(&c) && matches!(pinned_backend, Some("amf" | "pyrowave")) {
                                        s.launch.warnings.set("encoder_compute_recovery", "Compute conversion disabled after repeated encoder failures; using the graphics queue for the rest of this session. A busy game can delay frames; lower game GPU load or update the AMD driver, then reconnect to retry compute.");
                                    }
                                }
                                progress.mark(crate::stall_watch::Phase::EncoderCreate);
                                match Encoder::new_gpu_reported(
                                    &stream,
                                    pinned_backend.unwrap_or(c.get("encoder", "auto")),
                                    &image,
                                    &tuning,
                                    &s.launch.warnings,
                                ) {
                                    Ok(created) => {
                                        *s.encoder.write().unwrap() = created.backend().into();
                                        encoder = Some(created);
                                    },
                                    Err(error) => {
                                        if let Some(loss) = DeviceLost::d3d11(&image.gpu.device).or_else(|| DeviceLost::from_error(&error)) {
                                            recovery.device_loss = Some(loss);
                                            recovery.failing.get_or_insert_with(Instant::now);
                                            continue;
                                        }
                                        // The encoder cannot make the size or rate the client
                                        // asked for: the stream goes back to the mode it had.
                                        if let Some((from, _)) = switching.take() {
                                            let to = butterpollo_core::reconfigure::Mode::of(&stream);
                                            from.apply_to(&mut stream);
                                            s.reconfigure.lock().unwrap().refused(from);
                                            tracing::info!(client = %s.launch.client.name, from = %from, to = %to, error = %format!("{error:#}"), "reconfigure refused: the encoder cannot make this mode; keeping the previous one");
                                            continue;
                                        }
                                        let since = *recovery.failing.get_or_insert_with(Instant::now);
                                        if since.elapsed() >= ENCODER_RECOVERY {
                                            return Err(error.context("the encoder could not be recreated"));
                                        }
                                        s.launch.warnings.set("encoder_recovery", format!("Encoder recreation failed ({error:#}); retrying the same backend while the picture freezes. Check the driver and lower game GPU load; the session will fail if frames do not resume."));
                                        timer.until(Instant::now() + Duration::from_millis(100));
                                        continue;
                                    }
                                }
                                // Encoder creation can outlast several source frames.
                                let Some(current) = latest.current()? else { continue };
                                if !encoder.as_ref().unwrap().accepts_gpu_device(&current) {
                                    continue;
                                }
                                image = current;
                                if arrival_pacing {
                                    pacer.observe_source(image.captured);
                                }
                                metadata_due = Instant::now();
                                staging = None;
                                recovery.recreated(&s);
                                rebuild_encoder = false;
                            }
                            let active = encoder
                                .as_mut()
                                .expect("a missing encoder is rebuilt above");
                            if rebuilt {
                                active.set_next_frame(match video_sender.get() {
                                    Some(sender) => sender.next_wire_frame()?,
                                    None => next_wire_frame.get(),
                                });
                            }
                            let begin = Instant::now();
                            let fresh = last_image.as_ref().is_none_or(|previous| !Arc::ptr_eq(previous, &image));
                            if let Some(admission) = admission_trace.as_mut() {
                                trace_attempt = trace_attempt.wrapping_add(1);
                                let us = |at: Instant| at.saturating_duration_since(start).as_micros() as u64;
                                let key = Arc::as_ptr(&image) as usize;
                                let (seen, interval, deadline) = match first_seen {
                                    Some((k, seen, interval, deadline)) if k == key => (seen, interval, deadline),
                                    _ => (begin, latest.source_interval(), None),
                                };
                                let (capture_id, selected_is_latest, latest_capture_id) = latest.trace_selection(&image);
                                tracing::trace!(
                                    target: "pacing",
                                    capture_id,
                                    attempt = trace_attempt,
                                    host_acquired = ?image.acquired,
                                    presented = us(image.captured),
                                    acquired = us(image.acquired),
                                    published = us(image.published),
                                    seen = us(seen),
                                    claim = us(begin),
                                    interval = interval.map_or(0, |i| i.as_micros() as u64),
                                    deadline = deadline.map_or(0, us),
                                    source_id = latest.trace_source_id(),
                                    stream_id = %s.launch.id,
                                    fresh,
                                    choice = if fresh { "fresh" } else if recovering { "recovery_repeat" } else { "static_repeat" },
                                    selected_is_latest,
                                    latest_capture_id,
                                    encoder_pending = encoding,
                                    sender_pending = sending,
                                    encoder_limit = encoding_limit,
                                    rebuilt,
                                    grid_us = admission.micros(AdmissionWait::Grid),
                                    capture_check_us = admission.micros(AdmissionWait::Capture),
                                    fresh_rate_us = admission.micros(AdmissionWait::FreshRate),
                                    repeat_us = admission.micros(AdmissionWait::Repeat),
                                    encoder_pressure_us = admission.micros(AdmissionWait::Encoder),
                                    sender_pressure_us = admission.micros(AdmissionWait::Sender),
                                    admission_passes = ?admission.passes,
                                    "claim"
                                );
                                *admission = AdmissionTrace::default();
                            }
                            if timings_logged && fresh && claim_ages.len() < 4096 {
                                let micros = |d: Duration| d.as_micros().min(u128::from(u64::MAX)) as u64;
                                claim_ages.push((
                                    micros(image.acquired.saturating_duration_since(image.captured)),
                                    micros(begin.saturating_duration_since(image.acquired)),
                                ));
                                if let Some(stamp) = image.wgc_stamp {
                                    let age = if image.acquired >= stamp {
                                        image.acquired.duration_since(stamp).as_secs_f64()
                                    } else {
                                        -stamp.duration_since(image.acquired).as_secs_f64()
                                    };
                                    wgc_stamp_ages.push(age * 1000.);
                                }
                            }
                            if Instant::now() >= metadata_due {
                                let metadata = m.set_display_hdr_metadata(&s, image.gpu.hdr_metadata());
                                active.set_hdr_metadata(metadata);
                                metadata_due = Instant::now() + Duration::from_secs(1);
                            }
                            let invalidation = s.invalidation.lock().unwrap().take();
                            if let Some((first, last)) = invalidation {
                                if !active.invalidate_ref_frames(first, last) {
                                    s.request_recovery_keyframe();
                                } else if active.needs_recovery_confirmation() {
                                    // A decoder that cannot follow the long-term
                                    // reference stays frozen, and Moonlight does not
                                    // ask again: a keyframe a second later bounds that.
                                    confirm_keyframe.get_or_insert(Instant::now() + RFI_CONFIRM_KEYFRAME);
                                }
                            }
                            if confirm_keyframe.is_some_and(|at| Instant::now() >= at) {
                                s.idr.store(true, Ordering::Release);
                            }
                            let idr = s.idr.swap(false, Ordering::AcqRel);
                            if idr {
                                confirm_keyframe = None;
                            }
                            let recovering = idr || invalidation.is_some();
                            let bitrate = s.bitrate.load(Ordering::Acquire);
                            if s.config.codec != 3 {
                                let now = Instant::now();
                                if encode_route.is_none_or(|(address, _)| address != peer.ip()) || now >= encode_route_due {
                                    encode_route = Some((peer.ip(), butterpollo_windows::net::routed_link(peer)));
                                    encode_route_due = now + Duration::from_secs(2);
                                }
                                // Budget for the parity a recovery picture uses,
                                // even while ordinary frames use adaptive FEC.
                                recovery_packetizer.fec_percent = s.frame_fec_percent(true);
                                let pace = crate::video_send::recovery::preview_pace(&s, &c, &recovery_packetizer, period, peer, encode_route.unwrap().1);
                                let cap = butterpollo_core::network_pacing::recovery_frame_cap(
                                    pace.bps, bitrate, period, recovery_packetizer.fec_percent,
                                    recovery_packetizer.packet_size, peer.is_ipv6(),
                                );
                                let _ = active.set_frame_cap(cap);
                            }
                            active.set_repeat(!fresh);
                            let mut presented_image = image.as_ref().clone();
                            if !fresh { presented_image.captured = Instant::now(); }
                            progress.mark(crate::stall_watch::Phase::Encode);
                            let encode_trace_started = trace_send.then(Instant::now);
                            let encoded = (|| -> Result<Vec<butterpollo_windows::encoder::Encoded>> {
                                #[cfg(debug_assertions)]
                                crate::soak_fault::check("encoder failure")?;
                                #[cfg(debug_assertions)]
                                crate::soak_fault::check("DXGI_ERROR_DEVICE_REMOVED")?;
                                Ok(if c.boolean("wgc_direct_encoder_input", true) {                                    active.encode_gpu(&presented_image, idr, bitrate)?
                                } else {
                                    active.encode(
                                        &presented_image.readback(&mut staging)?,
                                        idr,
                                        bitrate,
                                    )?
                                })
                            })();
                            if let Some(at) = encode_trace_started {
                                tracing::trace!(
                                    target: "pacing",
                                    stream_id = %s.launch.id,
                                    attempt = trace_attempt,
                                    host_acquired = ?image.acquired,
                                    submitted_presentation = ?presented_image.captured,
                                    encode_call_us = at.elapsed().as_micros().min(u128::from(u64::MAX)) as u64,
                                    submitted = active.last_submitted().filter(|submitted| *submitted >= at).map_or(0, |submitted| submitted.saturating_duration_since(start).as_micros() as u64),
                                    outputs = encoded.as_ref().map_or(0, Vec::len),
                                    ok = encoded.is_ok(),
                                    idr,
                                    fresh,
                                    "encode returned"
                                );
                            }
                            let output = match encoded {
                                Ok(output) => {
                                    recovery.output(output, Instant::now)?
                                }
                                Err(error) => {
                                    if let Some(loss) = active.device_removed()
                                        .or_else(|| DeviceLost::d3d11(&image.gpu.device))
                                        .or_else(|| DeviceLost::from_error(&error))
                                    {
                                        recovery.device_loss = Some(loss);
                                        recovery.failing.get_or_insert_with(Instant::now);
                                        continue;
                                    }
                                    // The first frame at a size the client asked for failed:
                                    // go back to the mode the stream had, as when the encoder
                                    // cannot be created at it.
                                    if let Some((from, _)) = switching.take() {
                                        let to = butterpollo_core::reconfigure::Mode::of(&stream);
                                        from.apply_to(&mut stream);
                                        s.reconfigure.lock().unwrap().refused(from);
                                        tracing::info!(client = %s.launch.client.name, from = %from, to = %to, error = %format!("{error:#}"), "reconfigure refused: the encoder failed at this mode; keeping the previous one");
                                        rebuild_encoder = true;
                                        continue;
                                    }
                                    // A stalled or reset GPU costs these frames and a
                                    // keyframe; the rebuild requests it. Only failures
                                    // that keep coming end the session.
                                    let since = *recovery.failing.get_or_insert_with(Instant::now);
                                    if since.elapsed() >= ENCODER_RECOVERY {
                                        return Err(error.context("the encoder kept failing"));
                                    }
                                    s.launch.warnings.set("encoder_recovery", format!("Encoding failed ({error:#}); recreating the same encoder while the picture freezes. Lower game GPU load or update the graphics driver if this repeats."));
                                    active.log_stall();
                                    rebuild_encoder = true;
                                    continue;
                                }
                            };
                            if rebuilt && let Some((from, started)) = switching.take() {
                                tracing::info!(
                                    client = %s.launch.client.name,
                                    from = %from,
                                    to = %butterpollo_core::reconfigure::Mode::of(&stream),
                                    switch_ms = started.elapsed().as_millis() as u64,
                                    source_width = image.width,
                                    source_height = image.height,
                                    "reconfigure"
                                );
                            }
                            let call_latency = begin.elapsed();
                            if fresh && pyrowave_sender.is_none() {
                                claims.borrow_mut().push(butterpollo_core::performance::Claim {
                                    key: presented_image.captured,
                                    presented: image.captured,
                                    acquired: image.acquired,
                                    published: image.published,
                                    claimed: begin,
                                    submitted: active.last_submitted().filter(|at| *at >= begin).unwrap_or(begin),
                                });
                            }
                            // Repeat deadlines start at submission, so encoder work
                            // does not extend the interval between static frames.
                            encoded_at = begin;
                            if arrival_pacing {
                                // New pictures and static repeats count toward the
                                // stream rate: a repeat that spent nothing let the
                                // game's next frame follow it at once, so VRR
                                // streams went past the rate whenever a frame came
                                // late. An unchanged picture encoded again for the
                                // client's recovery request does not, or a client
                                // losing packets left every game frame late.
                                if butterpollo_core::stream_policy::counts_toward_rate(fresh, recovering) {
                                    // Phase lock: holding frames here would only move the
                                    // wait from the client to the host, so the lock sets
                                    // the rate cap to the client's refresh instead. VRR
                                    // displays follow each frame and need no lock.
                                    match (!vrr).then(|| phase_interval(&s, begin, period)).flatten() {
                                        Some(interval) => {
                                            let mut grid = latest.grid.lock().unwrap();
                                            unlocked_grid_period.get_or_insert(grid.period);
                                            grid.period = interval;
                                            pacer.set_period(interval);
                                        }
                                        None => {
                                            if let Some(original) = unlocked_grid_period.take() {
                                                latest.grid.lock().unwrap().period = original;
                                                pacer.set_period(period);
                                            }
                                        }
                                    }
                                    pacer.claimed(begin);
                                }
                                latest.grid.lock().unwrap().anchor = pacer.allowed_at(begin);
                            } else {
                                // Phase lock: the grid follows the client's refresh, at a
                                // phase that gets frames there just before its latch.
                                // Without reports this is the original fixed-period path.
                                match phase_interval(&s, begin, period) {
                                    Some(interval) => {
                                        cadence.submitted_after(begin, interval);
                                        let mut grid = latest.grid.lock().unwrap();
                                        unlocked_grid_period.get_or_insert(grid.period);
                                        grid.anchor = cadence.deadline();
                                        grid.period = interval;
                                    }
                                    None => {
                                        cadence.submitted(begin);
                                        let mut grid = latest.grid.lock().unwrap();
                                        if let Some(original) = unlocked_grid_period.take() {
                                            grid.period = original;
                                        }
                                        grid.anchor = cadence.deadline();
                                    }
                                }
                            }
                            last_image = Some(image);
                            // An allocation can later reuse this image's address;
                            // its trace observation belongs only to this submission.
                            first_seen = None;
                            send_frames(output, peer, call_latency, latest, capture_tick)?;
                        }
                        Ok(())
                    })()
                })();
                if let Err(e) = result {
                    s.launch.warnings.set("stream_failure", format!("Stream ended: {e:#}"));
                    s.fail();
                    tracing::error!(error=%format!("{e:#}"),client=%s.launch.client.name,"session failed; check the reported encoder, capture or socket error before reconnecting");
                }
                s.stop();
                // A failed stream must not count as a healthy fast-rate probe.
                drop(keyframe_loss_memory);
                h.sessions.lock().unwrap().begin_teardown(&s);
                m.peers
                    .lock()
                    .unwrap()
                    .retain(|(id, _), _| id != &s.launch.id);
                // Keep capture and display leases owned until the encoder has
                // terminated, even if a driver blocks its drop indefinitely.
                if let Some(audio) = audio {
                    finish_worker(audio, "audio");
                }
                drop(pyrowave_sender);
                drop(client_commands);
                drop(encoder);
                drop(latest);
                drop(stream_preparation);
                #[cfg(test)]
                drop(fixture_preparation);
                drop(com);
                if s.launch.role == Role::RemoteMonitor
                    && h.config
                        .read()
                        .unwrap()
                        .boolean("remote_monitor_disconnect_on_stream_end", false)
                {
                    crate::remote_display::disconnect(&h, Some(&s.launch.client.uuid));
                }
                h.sessions.lock().unwrap().teardown.remove(&s.launch.id);
                tracing::info!(client=%s.launch.client.name,"CLIENT DISCONNECTED");
            })
    }
    fn audio(&self, h: Shared, s: Arc<Session>) -> Result<()> {
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
        let directory = std::env::current_exe()?.parent().unwrap().to_owned();
        let layout = butterpollo_core::audio::OpusLayout::select(
            s.config.audio_channels as usize,
            s.config.audio_quality,
            s.launch.options.get("surroundParams").map(String::as_str),
        )?;
        let mut opus = Opus::new_layout_duration(&directory, &layout, s.config.audio_packet_ms)?;
        let mut p = AudioPacketizer::new(
            s.launch.key,
            s.launch.key_id,
            s.config.encryption & 4 != 0,
            u32::from(s.config.audio_packet_ms),
        );
        let frames = 48 * usize::from(s.config.audio_packet_ms);
        let start = Instant::now();
        let interval = Duration::from_millis(u64::from(s.config.audio_packet_ms));
        // When audio last arrived, and when an idle endpoint's next silence is due.
        let mut heard = Instant::now();
        let mut audio_qos = Tagged::default();
        let mut loss = butterpollo_core::audio::HostLoss::default();
        let mut last_loss = None;
        let mut upkeep: Option<RouteUpkeep> = None;
        let mut next = Instant::now();
        let timer = butterpollo_windows::timing::Timer::new()?;
        let silence = vec![0.; frames * s.config.audio_channels as usize];
        while !s.stopping() && !h.stop.load(Ordering::Acquire) {
            match capture
                .as_ref()
                .filter(|capturing| capturing.event_driven())
            {
                // Audio events wake immediately. Before the silence threshold,
                // an expired packet deadline must not turn this into a 1 ms poll.
                Some(capturing) => capturing.wait(
                    next.max(heard + Duration::from_millis(50))
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
                game_capture.update(&s, &mut capture);
            }
            if !muted && !game_audio && Instant::now() >= audio_check {
                audio_check = Instant::now() + Duration::from_secs(1);
                if route.is_none() {
                    match butterpollo_windows::audio_route::Route::acquire(
                        &config,
                        &h.directory,
                        s.launch.host_audio,
                        s.config.audio_channels as usize,
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
                        capture = None;
                        sink = selected;
                    }
                    // Pinning the sink only disables following the Windows
                    // default. Even a fixed endpoint must reopen after a
                    // driver reset, format change or temporary disconnect.
                    if capture.is_none() {
                        match route.set_channels(s.config.audio_channels as usize) {
                            Ok(()) => s.launch.warnings.clear("audio_surround"),
                            Err(error) => s.launch.warnings.set("audio_surround", format!("Virtual surround format could not be applied ({error:#}); channels may be downmixed. Select stereo or check the virtual audio driver.")),
                        }
                        match Loopback::new_sink(s.config.audio_channels as usize, &sink) {
                            Ok(value) => {
                                tracing::info!(
                                    channels = s.config.audio_channels,
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
            let peer = self
                .peers
                .lock()
                .unwrap()
                .get(&(s.launch.id.clone(), true))
                .copied();
            // Every whole packet the endpoint has delivered goes out at once, as
            // Sunshine sends them. On a fixed tick each waited up to a packet,
            // and a tick just before a late chunk sent silence in its place.
            let mut packets = Vec::new();
            let waited = capture.as_mut().and_then(Loopback::since_drained);
            while let Some(capturing) = capture.as_mut() {
                match capturing.read(frames) {
                    Ok(Some(samples)) => packets.push(samples),
                    Ok(None) => break,
                    Err(error) => {
                        let recovery = if game_audio {
                            "sending silence until a verified game process can be captured again. No system audio is captured. Check the game's launch entry if this repeats."
                        } else {
                            "reopening WASAPI and sending silence until it recovers. Check the playback device if this repeats."
                        };
                        s.launch.warnings.set(
                            "audio_capture",
                            format!("Audio capture interrupted ({error:#}); {recovery}"),
                        );
                        capture = None;
                    }
                }
            }
            if let Some(capturing) = capture.as_mut() {
                let delivered = capturing.take_delivered();
                if let Some(waited) = waited {
                    let lost = loss.read(waited, delivered, capturing.buffer());
                    // A skip no late read explains is the endpoint falling
                    // quiet and starting again.
                    let skipped = delivered.unwrap_or_default();
                    if waited > capturing.buffer() || !skipped.is_zero() {
                        tracing::debug!(
                            waited_ms = waited.as_secs_f64() * 1000.,
                            skipped_ms = skipped.as_secs_f64() * 1000.,
                            delivered = delivered.is_some(),
                            lost,
                            "audio sender read late or Windows skipped audio"
                        );
                    }
                }
            }
            let now = Instant::now();
            if !packets.is_empty() {
                heard = now;
                next = now + interval;
            } else if now >= next
                && now.saturating_duration_since(heard) >= Duration::from_millis(50)
            {
                // WASAPI delivers roughly 10 ms chunks; allow for scheduling jitter
                // before treating a quiet capture queue as an idle endpoint.
                packets.push(silence.clone());
                next += interval;
                if next < now {
                    next = now + interval;
                }
            }
            if let Some(peer) = peer {
                if s.config.audio_qos {
                    audio_qos.follow(&self.audio, peer, true);
                }
                for samples in &packets {
                    for packet in p.encode(&opus.encode(samples)?)? {
                        // A lost audio packet is concealed by the client; only a
                        // broken socket stops the audio.
                        if !butterpollo_windows::net::send_datagram(&self.audio, &packet, peer)? {
                            loss.unsent();
                        }
                    }
                }
            } else if start.elapsed() > crate::network::ping_timeout(&config) {
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
    fn control(&self, h: Shared) -> Result<()> {
        let socket = crate::network::udp((self.bind, self.control_port).into())?;
        butterpollo_windows::net::configure_udp(&socket)?;
        let raw_socket = std::os::windows::io::AsRawSocket::as_raw_socket(&socket);
        let raw_wake =
            std::os::windows::io::AsRawSocket::as_raw_socket(&self.control_wake_receiver);
        let mut host = Host::new(
            ControlSocket::new(socket),
            HostSettings {
                peer_limit: 32,
                // Moonlight opens 48 channels (gamepads from 0x10, motion
                // from 0x20) and folds those above the host's limit onto
                // channel 0, where one lost ping holds up all controller
                // input. The previous host allowed the protocol maximum.
                channel_limit: 255,
                ..Default::default()
            },
        )?;
        let _com = ComGuard::new()?;
        let mut priority = None;
        let (commands, pending_commands) =
            std::sync::mpsc::sync_channel::<(Arc<Session>, String, bool)>(32);
        thread::Builder::new()
            .name("server commands".into())
            .spawn(move || {
                for (session, value, elevated) in pending_commands {
                    if session.stopping() {
                        continue;
                    }
                    if let Err(error) = butterpollo_windows::process::Process::shell_detached(
                        &value,
                        None,
                        elevated,
                        &Default::default(),
                    ) {
                        tracing::warn!(%error, "configured server command failed");
                    }
                }
            })?;
        let mut peers: HashMap<PeerID, ControlPeer> = HashMap::new();
        // Active sessions without a control peer, since when.
        let mut unattended: HashMap<String, Instant> = HashMap::new();
        // Keep the snapshot's allocation across high-rate input wakes. Sessions
        // are keyed by their immutable launch ID, so the IDs need no copies.
        let mut active_sessions: Vec<Arc<Session>> = Vec::new();
        let input_timer = butterpollo_windows::timing::Timer::new()?;
        let mut feedback_at = Instant::now();
        let mut controller_cleanup = Instant::now();
        while !h.stop.load(Ordering::Acquire) {
            while self.control_wake_receiver.recv(&mut [0]).is_ok() {}
            if controller_cleanup.elapsed() >= Duration::from_secs(1) {
                let app_running = h.current_app.lock().unwrap().is_some();
                h.controller_sessions.expire(app_running);
                controller_cleanup = Instant::now();
            }
            let ping_timeout = crate::network::ping_timeout(&h.config.read().unwrap());
            #[cfg(test)]
            if let Some(fixture) = &h.reconnect_fixture {
                fixture.reset_peer(&mut host, &peers);
            }
            let mut disconnected = Vec::new();
            // Acknowledgements and anything else ENet sends wait until this
            // pass's input is applied.
            host.socket_mut().hold();
            // Bound each pass so a busy input peer cannot starve cleanup or feedback.
            let mut serviced = 0;
            for _ in 0..CONTROL_EVENT_LIMIT {
                let Some(event) = host.service()? else { break };
                serviced += 1;
                match event {
                    Event::Connect { peer, data } => {
                        let Some(address) = peer.address() else {
                            peer.disconnect_now(0);
                            continue;
                        };
                        let sessions = h.sessions.lock().unwrap();
                        let candidates: Vec<_> = sessions
                            .active
                            .values()
                            .filter(|s| !s.stopping())
                            .map(|s| &s.launch)
                            .chain(sessions.pending.values())
                            .filter(|l| l.peer == address.ip().to_canonical())
                            .collect();
                        let launch = candidates
                            .iter()
                            .find(|l| data == l.connect_data)
                            .copied()
                            .or_else(|| {
                                if data == 0 && candidates.len() == 1 {
                                    Some(candidates[0])
                                } else {
                                    None
                                }
                            });
                        if let Some(l) = launch {
                            if peers.values().any(|p| p.id == l.id) {
                                peer.disconnect_now(0);
                                continue;
                            }
                            // ENet's own peer timeout (about 5 s) would end a
                            // stream on a Wi-Fi drop the session itself
                            // tolerates; let ping_timeout decide instead.
                            let timeout =
                                u32::try_from(ping_timeout.as_millis()).unwrap_or(u32::MAX);
                            peer.set_timeout(32, timeout, timeout.max(30_000));
                            peers.insert(
                                peer.id(),
                                ControlPeer {
                                    #[cfg(test)]
                                    _fixture_input: h.reconnect_fixture.as_ref().map(|f| f.input()),
                                    id: l.id.clone(),
                                    injector: None,
                                    injector_tried: false,
                                    sequence: 0,
                                    received: Default::default(),
                                    hdr_metadata: None,
                                    legacy: butterpollo_core::packet::LegacyInput::new(l.key_id),
                                    seen: Instant::now(),
                                    input_released: false,
                                    // The session can finish before the first
                                    // control sweep. Retain it while the active
                                    // map is locked so its termination reason
                                    // survives the worker moving to teardown.
                                    session: sessions.active.get(&l.id).cloned(),
                                    inputs: vec![],
                                    denied: 0,
                                    controller_audio: [0; 16],
                                    controller_pcm: Default::default(),
                                    command_at: None,
                                },
                            );
                        } else {
                            peer.disconnect_now(0);
                        }
                        #[cfg(test)]
                        if let Some(fixture) = &h.reconnect_fixture
                            && let Some(connected) = peers.get(&peer.id())
                        {
                            let id = connected.id.clone();
                            drop(sessions);
                            fixture.control_connected(&h, &id);
                        }
                    }
                    Event::Disconnect { peer, .. } => {
                        let id = peer.id();
                        if let Some(p) = peers.remove(&id) {
                            let session = h.sessions.lock().unwrap().active.get(&p.id).cloned();
                            let canceled = h.sessions.lock().unwrap().request_stop(Some(&p.id));
                            drop(canceled);
                            if let Some(session) = session
                                && session.launch.role == Role::RemoteMonitor
                                && h.config.read().unwrap().boolean(
                                    "remote_monitor_disconnect_on_client_disconnect",
                                    false,
                                )
                            {
                                crate::remote_display::disconnect(
                                    &h,
                                    Some(&session.launch.client.uuid),
                                );
                            }
                            disconnected.push(p);
                        }
                    }
                    Event::Receive { peer, packet, .. } => {
                        let Some(p) = peers.get_mut(&peer.id()) else {
                            continue;
                        };
                        let s = h.sessions.lock().unwrap().active.get(&p.id).cloned();
                        let Some(s) = s else { continue };
                        p.session = Some(s.clone());
                        let b = packet.data();
                        if b.len() < 2 || b.len() > 65536 {
                            continue;
                        }
                        let encrypted = b[..2] == [1, 0];
                        if s.config.encryption & 1 != 0 && !encrypted {
                            continue;
                        }
                        let parsed = if encrypted {
                            butterpollo_core::packet::decrypt_control(
                                &s.launch.key,
                                b,
                                s.config.encryption & 1 != 0,
                            )
                            .and_then(|(seq, kind, payload)| {
                                if !p.received.accept(seq) {
                                    anyhow::bail!("replayed control message");
                                }
                                Ok((kind, payload))
                            })
                        } else {
                            butterpollo_core::packet::control_header(b)
                                .map(|(kind, payload)| (kind, payload.to_vec()))
                        };
                        let (kind, payload) = match parsed {
                            Ok(v) => v,
                            Err(e) => {
                                tracing::debug!(error=%e,"invalid control packet");
                                continue;
                            }
                        };
                        p.seen = Instant::now();
                        p.input_released = false;
                        match kind {
                            0x0200 => s.record_control_ping(Instant::now(), &payload),
                            controller_audio::CONTROL | controller_audio::MICROPHONE
                                if encrypted
                                    && haptics_negotiated(&s.config, peer.channel_count()) =>
                            {
                                if !s.launch.client.allows(1 << 8) {
                                    continue;
                                }
                                if kind == controller_audio::CONTROL {
                                    if let Some((id, flags)) = controller_audio::status(&payload) {
                                        let Ok(config) = effective_config(&h, &s.launch) else {
                                            continue;
                                        };
                                        let policy = controller_audio::status_policy(
                                            &config,
                                            s.launch.role,
                                            s.launch.client.perm & ((1 << 25) | (1 << 26)) != 0,
                                            flags,
                                        );
                                        if flags & controller_audio::PLAYBACK != 0
                                            && policy.device_flags & controller_audio::PLAYBACK == 0
                                            && config.get("audio_capture_mode", "system") == "game"
                                        {
                                            s.launch.warnings.set("audio_controller_scope", "Game-only audio suppresses dedicated USB controller speaker sound because that endpoint cannot identify its source process. The normal captured game stream still routes to a connected controller headset; haptics and microphone settings remain independent.");
                                        }
                                        let mut state = payload.clone();
                                        state[4] = policy.device_flags;
                                        if let Some(i) = &p.injector {
                                            i.controller_audio(id, kind, &state);
                                        }
                                        p.controller_audio[id as usize] = policy.device_flags;
                                        state[1] = 3; // Acknowledge host microphone policy.
                                        // Auto may have attached a VHF pad without audio.
                                        // The client's periodic status retries after USB
                                        // enumeration; retain its desired device policy.
                                        state[4] = policy.acknowledged_flags(
                                            p.injector
                                                .as_ref()
                                                .is_some_and(|i| i.controller_audio_ready(id)),
                                        );
                                        if let Ok(message) = p.encrypt(&s, kind, &state) {
                                            let _ = peer.send(
                                                FEEDBACK_CHANNEL,
                                                &Packet::new(message, PacketKind::Reliable),
                                            );
                                        }
                                    }
                                } else if let Some((id, _, _)) = controller_audio::pcm(&payload)
                                    && p.controller_audio[id as usize]
                                        & (controller_audio::CAPTURE | controller_audio::MUTED)
                                        == controller_audio::CAPTURE
                                    && let Some(i) = &p.injector
                                {
                                    p.controller_pcm.observe(id, kind, Instant::now());
                                    i.controller_audio(id, kind, &payload);
                                }
                            }
                            0x3000 => {
                                if let Some((value, elevated)) = server_command(
                                    &h,
                                    &s.launch.client,
                                    &payload,
                                    encrypted,
                                    &mut p.command_at,
                                ) && let Err(e) = commands.try_send((s.clone(), value, elevated))
                                {
                                    tracing::warn!(error=%e, "configured server command failed");
                                }
                            }
                            // SS_RFI_REQUEST: first frame, a reserved word, last
                            // frame, reserved words; older clients sent two
                            // 64-bit indices, whose low words sit at the same
                            // offsets. Requiring 8 bytes turned every request
                            // into a keyframe.
                            0x0301 if payload.len() >= 12 => {
                                s.request_invalidation(
                                    u64::from(u32::from_le_bytes(payload[..4].try_into().unwrap())),
                                    u64::from(u32::from_le_bytes(
                                        payload[8..12].try_into().unwrap(),
                                    )),
                                );
                            }
                            0x0301 => s.request_invalidation(0, 0),
                            0x0302 => s.request_recovery_keyframe(),
                            // This dispatch receives only client-to-host packets;
                            // 0x5502 in the other direction is controller feedback.
                            0x5502 => s.record_fec_status(&payload),
                            butterpollo_core::phase_sync::REPORT_MESSAGE_TYPE => {
                                match s
                                    .phase_sync
                                    .lock()
                                    .unwrap()
                                    .on_payload(Instant::now(), &payload)
                                {
                                    Some(report) => tracing::debug!(
                                        lead_ns = report.lead_ns,
                                        spread_ns = report.spread_ns,
                                        period_ns = report.period_ns,
                                        frames = report.frames,
                                        "phase lock report"
                                    ),
                                    None => tracing::debug!(
                                        len = payload.len(),
                                        "invalid phase lock report"
                                    ),
                                }
                            }
                            // Answered at once, unreliable: a lost exchange is one sample.
                            butterpollo_core::clock_sync::CLOCK_SYNC_MESSAGE_TYPE => {
                                let epoch = *s.media_epoch.read().unwrap();
                                if let Some(reply) =
                                    butterpollo_core::clock_sync::reply(epoch, p.seen, &payload)
                                    && let Ok(message) = p.encrypt(&s, kind, &reply)
                                {
                                    let _ = peer.send(
                                        FEEDBACK_CHANNEL,
                                        &Packet::new(
                                            message,
                                            PacketKind::AlwaysUnreliable { sequenced: false },
                                        ),
                                    );
                                }
                            }
                            butterpollo_core::host_timing::HOST_TIMING_MESSAGE_TYPE => {
                                match s.host_timings.lock().unwrap().on_request(&payload) {
                                    Some(enabled) => tracing::info!(
                                        client = %s.launch.client.name,
                                        enabled,
                                        "host timing reports"
                                    ),
                                    None => tracing::debug!(
                                        len = payload.len(),
                                        "invalid host timing request"
                                    ),
                                }
                            }
                            butterpollo_core::display_caps::DISPLAY_CAPS_MESSAGE_TYPE => {
                                match s.record_display_caps(&payload) {
                                    Some(update) if update.changed => {
                                        tracing::info!(
                                            client = %s.launch.client.name,
                                            hdr = update.caps.hdr,
                                            peak_nits = update.caps.max_nits().unwrap_or(0),
                                            average_nits = update.caps.max_average_nits().unwrap_or(0),
                                            black_decimillinits = update.caps.min_decimillinits,
                                            applied = "diagnostic only; source HDR metadata retained",
                                            "display caps"
                                        );
                                    }
                                    Some(_) => tracing::debug!("display caps unchanged"),
                                    None => {
                                        tracing::debug!(len = payload.len(), "invalid display caps")
                                    }
                                }
                            }
                            // The session loop applies it once the client stops sending.
                            butterpollo_core::reconfigure::RECONFIGURE_MESSAGE_TYPE => {
                                let outcome = s
                                    .reconfigure
                                    .lock()
                                    .unwrap()
                                    .on_payload(Instant::now(), &payload);
                                tracing::debug!(
                                    ?outcome,
                                    len = payload.len(),
                                    "reconfigure request"
                                );
                            }
                            0x0109 => {
                                if encrypted {
                                    s.stop();
                                }
                            }
                            0x0206 => {
                                let raw = if encrypted {
                                    Ok(payload)
                                } else {
                                    p.legacy.open(&s.launch.key, &payload)
                                };
                                let event = raw.and_then(|raw| input::decode(&raw));
                                match event {
                                    // Say once per kind that the device may not
                                    // send it: dropped silently, a phone's
                                    // controller looked undetected.
                                    Ok(event)
                                        if !s.launch.client.allows(event.required_permission()) =>
                                    {
                                        let permission = event.required_permission();
                                        if p.denied & permission == 0 {
                                            p.denied |= permission;
                                            let (code, message) =
                                                event.denied_warning(&s.launch.client.name);
                                            s.launch.warnings.set(&code, message);
                                        }
                                    }
                                    Ok(mut event) => {
                                        if let input::Input::Arrival { capabilities, .. } =
                                            &mut event
                                            && !haptics_negotiated(&s.config, peer.channel_count())
                                        {
                                            *capabilities &= !haptics::CAPABILITY;
                                        }
                                        if let input::Input::Arrival { capabilities, .. } = &event
                                            && let Some(hint) =
                                                butterpollo_core::input_policy::steam_input_hint(
                                                    &s.launch.client.name,
                                                    *capabilities,
                                                )
                                        {
                                            s.launch.warnings.set("input_steam_input", hint);
                                        }
                                        if p.inputs.last_mut().is_some_and(|last| {
                                            last.merge(&event) == input::Batch::Merged
                                        }) {
                                            continue;
                                        }
                                        if p.inputs.len() < 512 {
                                            p.inputs.push(event);
                                        } else {
                                            s.stop();
                                        }
                                    }
                                    Err(e) => tracing::debug!(error=%e,"invalid input packet"),
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            let poll_feedback = Instant::now() >= feedback_at;
            if poll_feedback {
                feedback_at = Instant::now() + Duration::from_millis(8);
            }
            let mut remove = vec![];
            for (peer_id, p) in &mut peers {
                #[cfg(test)]
                if h.reconnect_fixture.is_some() {
                    p.injector_tried = true;
                }
                let sessions = h.sessions.lock().unwrap();
                let s = sessions
                    .active
                    .get(&p.id)
                    .cloned()
                    .or_else(|| p.session.clone());
                let pending = sessions.pending.contains_key(&p.id);
                drop(sessions);
                if let Some(s) = s {
                    p.session = Some(s.clone());
                    let timed_out = p.seen.elapsed() > ping_timeout;
                    if s.stopping() || timed_out {
                        if timed_out && !s.stopping() {
                            tracing::warn!(client=%s.launch.client.name,"client control stream timed out");
                            s.fail();
                        }
                        let reason = s.termination_reason();
                        if let Ok(message) = p.encrypt(&s, 0x0109, &reason.to_be_bytes()) {
                            let peer = host.peer_mut(*peer_id);
                            let _ = peer.send(0, &Packet::new(message, PacketKind::Reliable));
                            peer.disconnect_later(0);
                        } else {
                            host.peer_mut(*peer_id).disconnect_now(0);
                        }
                        s.stop();
                        remove.push(*peer_id);
                        continue;
                    }
                    if !p.input_released && p.seen.elapsed() >= Duration::from_secs(1) {
                        #[cfg(test)]
                        if let Some(fixture) = &h.reconnect_fixture {
                            fixture.input_releases.fetch_add(1, Ordering::Release);
                        }
                        if let Some(injector) = &mut p.injector {
                            injector.release_all();
                        }
                        p.input_released = true;
                    }
                    // Sent once the encoder has the display's metadata, as
                    // Sunshine does, and again only if it changes. Moonlight
                    // for Xbox sets the TV's HDMI mode on every message, so an
                    // earlier one with placeholder values switched it twice
                    // (issue #11).
                    let metadata = s.hdr_metadata.read().unwrap().map(|m| m.wire(s.config.hdr));
                    if let Some(metadata) = metadata
                        && p.hdr_metadata != Some(metadata)
                        && let Ok(message) = p.encrypt(&s, 0x010e, &metadata)
                        && host
                            .peer_mut(*peer_id)
                            .send(0, &Packet::new(message, PacketKind::Reliable))
                            .is_ok()
                    {
                        p.hdr_metadata = Some(metadata);
                    }
                    // Present-to-sent reports for a client that asked (0x5536).
                    // Unreliable: a lost one costs the client those samples only.
                    loop {
                        let report = s.host_timings.lock().unwrap().take_report();
                        let Some(report) = report else { break };
                        if let Ok(message) = p.encrypt(
                            &s,
                            butterpollo_core::host_timing::HOST_TIMING_MESSAGE_TYPE,
                            &report,
                        ) {
                            let _ = host.peer_mut(*peer_id).send(
                                0,
                                &Packet::new(
                                    message,
                                    PacketKind::AlwaysUnreliable { sequenced: false },
                                ),
                            );
                        }
                    }
                    if poll_feedback && let Some((old, new)) = s.tick_fec(Instant::now()) {
                        tracing::info!(
                            client = %s.launch.client.name,
                            old_percent = old,
                            new_percent = new,
                            "adaptive fec"
                        );
                    }
                    if poll_feedback
                        && let Some((old, new)) = s.tick_automatic_pacing(
                            Instant::now(),
                            host.peer(*peer_id).round_trip_time(),
                        )
                    {
                        tracing::info!(client = %s.launch.client.name, old_bps = old, new_bps = new, "automatic video burst pacing; encoder bitrate unchanged");
                    }
                    if poll_feedback
                        && let Some((old, new)) = s.tick_adaptive_bitrate(
                            Instant::now(),
                            host.peer(*peer_id).round_trip_time(),
                        )
                    {
                        tracing::info!(client = %s.launch.client.name, old_kbps = old, new_kbps = new, "host adaptive bitrate");
                    }
                    // Made before input comes; after a failure, again when it
                    // does. Making it with the first input held that input up.
                    if p.injector.is_none() && (!p.inputs.is_empty() || !p.injector_tried) {
                        p.injector_tried = true;
                        let c = match effective_config(&h, &s.launch) {
                            Ok(c) => c,
                            Err(e) => {
                                tracing::warn!(error = %format!("{e:#}"), "input configuration unavailable");
                                p.inputs.clear();
                                continue;
                            }
                        };
                        let output = s.output.read().unwrap().clone();
                        let pads = match h.controller_sessions.acquire(&s.launch, &c) {
                            Ok(pads) => pads,
                            Err(error) => {
                                s.launch.warnings.set("input_initialization", format!("Controller recovery is still in progress ({error:#}); retrying."));
                                p.injector_tried = false;
                                continue;
                            }
                        };
                        match Injector::new_options_reported(
                            if output.is_empty() {
                                c.get("output_name", "")
                            } else {
                                &output
                            },
                            c.get("gamepad", "auto"),
                            &c,
                            s.launch.warnings.clone(),
                        ) {
                            Ok(mut i) => {
                                i.use_gamepads(pads);
                                // The client can announce USB audio before input
                                // initialization finishes. Apply its scoped flags
                                // to the acquired (possibly retained) devices now.
                                for (id, flags) in p.controller_audio.iter().copied().enumerate() {
                                    if flags != 0 {
                                        i.controller_audio(id as u16, controller_audio::CONTROL,
                                            &[1, 1, id as u8, 0, flags, 0, 0, 0]);
                                    }
                                }
                                s.launch.warnings.clear("input_initialization");
                                i.set_feedback_waker(self.control_wake.clone());
                                i.set_stream_size(s.config.width, s.config.height);
                                p.injector = Some(i);
                            }
                            Err(e) => s.launch.warnings.set("input_initialization", format!("Input initialization failed ({e:#}); controls are unavailable. Check Input settings and the selected display, then reconnect.")),
                        }
                    }
                    if let Some(i) = &mut p.injector {
                        // The stream's display can be created or renamed after
                        // input began; absolute input follows it.
                        i.set_output(&s.output.read().unwrap());
                        // So does the stream's size, which the client can change (0x5532).
                        let mode = s.stream_mode();
                        i.set_stream_size(mode.width, mode.height);
                    }
                    if let Some(i) = &mut p.injector {
                        for e in i.apply_all(&p.inputs) {
                            s.launch.warnings.event("input_injection", format!("Input injection failed ({e:#}); keyboard, mouse, touch or pen actions may be missing. Unlock the desktop and check Windows input permissions or disable native touch/pen if unsupported."), butterpollo_core::session::EVENT_PERIOD);
                        }
                    }
                    p.inputs.clear();
                    if !poll_feedback
                        && let Some(i) = &mut p.injector
                        && let Err(e) = i.due()
                    {
                        s.launch.warnings.event("input_repeat", format!("Input release or repeat failed ({e:#}); held controls may not update. Unlock the desktop or reconnect."), butterpollo_core::session::EVENT_PERIOD);
                    }
                    if poll_feedback
                        && let Some(i) = &mut p.injector
                        && let Err(e) = i.refresh()
                    {
                        s.launch.warnings.event("input_pointer", format!("Touch/pen pointer refresh failed ({e:#}); pointer updates may be missing. Unlock the desktop or disable native touch/pen if Windows does not support it."), butterpollo_core::session::EVENT_PERIOD);
                    }
                    // The gamepad thread polls feedback every 8 ms; pass on
                    // what it found, and the sensors an arrived pad has.
                    let mut messages = Vec::new();
                    let mut waveforms = Vec::new();
                    if let Some(i) = &p.injector {
                        for report in i.gamepad_reports() {
                            match report {
                                PadReport::Speaker(frame) => {
                                    if p.controller_audio[frame.id as usize]
                                        & controller_audio::PLAYBACK
                                        != 0
                                        && frame.created.elapsed() <= haptics::MAX_AGE
                                        && let Some(payload) = frame.payload()
                                    {
                                        p.controller_pcm.observe(
                                            frame.id,
                                            controller_audio::SPEAKER,
                                            Instant::now(),
                                        );
                                        waveforms.push((controller_audio::SPEAKER, payload));
                                    }
                                }
                                PadReport::Haptics(frame) => {
                                    if i.feedback_allowed(haptics::PACKET_TYPE)
                                        && haptics_negotiated(
                                            &s.config,
                                            host.peer(*peer_id).channel_count(),
                                        )
                                        && frame.created.elapsed() <= haptics::MAX_AGE
                                        && let Some(payload) = frame.payload()
                                    {
                                        p.controller_pcm.observe(
                                            frame.id,
                                            haptics::PACKET_TYPE,
                                            Instant::now(),
                                        );
                                        waveforms.push((haptics::PACKET_TYPE, payload));
                                    }
                                }
                                PadReport::Feedback { id, kind, data } => messages.extend(
                                    feedback_packets(id, kind, &data)
                                        .into_iter()
                                        .filter(|(kind, _)| i.feedback_allowed(*kind)),
                                ),
                                PadReport::Motion { id, capabilities } => {
                                    messages.extend(motion_requests(id, capabilities))
                                }
                            }
                        }
                    }
                    s.reserve_controller_pcm(p.controller_pcm.streams(Instant::now()));
                    for (kind, payload) in waveforms {
                        if let Ok(message) = p.encrypt(&s, kind, &payload) {
                            let _ = host.peer_mut(*peer_id).send(
                                haptics::CHANNEL,
                                &Packet::new(
                                    message,
                                    PacketKind::AlwaysUnreliable { sequenced: false },
                                ),
                            );
                        }
                    }
                    for (kind, payload) in messages {
                        if let Ok(message) = p.encrypt(&s, kind, &payload) {
                            let _ = host.peer_mut(*peer_id).send(
                                FEEDBACK_CHANNEL,
                                &Packet::new(message, PacketKind::Reliable),
                            );
                        }
                    }
                } else if !pending || p.seen.elapsed() > ping_timeout {
                    host.peer_mut(*peer_id).disconnect_now(0);
                    remove.push(*peer_id);
                }
            }
            host.socket_mut().release()?;
            // Release held input now; gamepad workers unplug their pads asynchronously.
            drop(disconnected);
            for peer in remove {
                peers.remove(&peer);
            }
            // A session whose client never connected control, or whose peer
            // this worker lost when it restarted, would otherwise hold the
            // encoder, capture and display forever (Sunshine times it out).
            active_sessions.extend(h.sessions.lock().unwrap().active.values().cloned());
            unattended.retain(|id, _| active_sessions.iter().any(|s| &s.launch.id == id));
            for s in active_sessions.drain(..) {
                let id = &s.launch.id;
                if s.stopping() || peers.values().any(|p| &p.id == id) {
                    unattended.remove(id);
                    continue;
                }
                let since = match unattended.get(id) {
                    Some(since) => *since,
                    None => {
                        let since = Instant::now();
                        unattended.insert(id.clone(), since);
                        since
                    }
                };
                if since.elapsed() > ping_timeout {
                    tracing::warn!(client=%s.launch.client.name,"session has no control connection; ending it");
                    s.fail();
                    s.stop();
                }
            }
            host.flush();
            if peers.is_empty() {
                priority = None;
            } else if priority.is_none() {
                priority = Some(Priority::input());
            }
            let now = Instant::now();
            let due = if peers.is_empty() {
                now + Duration::from_millis(250)
            } else {
                peers
                    .values()
                    .filter_map(|p| p.injector.as_ref()?.next_due())
                    .fold(feedback_at, Instant::min)
            };
            let timeout = control_wait_timeout(due.saturating_duration_since(now), serviced);
            if butterpollo_windows::net::wait_readable_or_wake(raw_socket, raw_wake, timeout)
                .is_err()
            {
                // A polling error must not spin the loop.
                input_timer.until(now + Duration::from_millis(timeout.max(1) as u64));
            }
        }
        for (peer, p) in peers {
            host.peer_mut(peer).disconnect_now(0);
            drop(p);
        }
        Ok(())
    }
}
struct ControlPeer {
    #[cfg(test)]
    _fixture_input: Option<reconnect_tests::Slot>,
    id: String,
    injector: Option<Injector>,
    /// Whether making the injector was tried.
    injector_tried: bool,
    sequence: u32,
    received: butterpollo_core::packet::ReplayWindow,
    hdr_metadata: Option<[u8; 27]>,
    legacy: butterpollo_core::packet::LegacyInput,
    seen: Instant,
    input_released: bool,
    session: Option<Arc<Session>>,
    inputs: Vec<input::Input>,
    /// Input permissions this device lacked and was warned about.
    denied: u32,
    controller_audio: [u8; 16],
    controller_pcm: controller_audio::PcmTraffic,
    command_at: Option<Instant>,
}
impl Drop for ControlPeer {
    fn drop(&mut self) {
        if let Some(injector) = &mut self.injector {
            injector.detach_gamepads();
        }
    }
}
impl ControlPeer {
    fn encrypt(&mut self, s: &Session, kind: u16, payload: &[u8]) -> Result<Vec<u8>> {
        let seq = self.sequence;
        self.sequence = seq.checked_add(1).context("control nonce exhausted")?;
        butterpollo_core::packet::encrypted_control(
            &s.launch.key,
            seq,
            kind,
            payload,
            s.config.encryption & 1 != 0,
        )
    }
}
const MOTION_EVENT_REQUEST: u16 = 0x5501;
const RUMBLE_TRIGGER_DATA: u16 = 0x5500;
const RUMBLE_DATA: u16 = 0x010b;
const RUMBLE_MARKER: u32 = 0x00c0ffee;
// Channel 0 is available even when an older Moonlight peer negotiates only
// one channel. Sending on channel 1 then fails with InvalidChannel.
const FEEDBACK_CHANNEL: u8 = 0;

/// Ask the client for an arrived pad's accelerometer (kind 1) and gyroscope
/// (kind 2) at 200 Hz, when it has them.
fn motion_requests(id: u8, capabilities: u16) -> Vec<(u16, Vec<u8>)> {
    [(0x10, 1), (0x20, 2)]
        .into_iter()
        .filter(|(sensor, _)| capabilities & sensor != 0)
        .map(|(_, kind)| {
            let mut payload = u16::from(id).to_le_bytes().to_vec();
            payload.extend_from_slice(&200u16.to_le_bytes());
            payload.push(kind);
            (MOTION_EVENT_REQUEST, payload)
        })
        .collect()
}
fn haptics_negotiated(config: &butterpollo_core::rtsp::Negotiated, channels: usize) -> bool {
    config.encryption & 1 != 0
        && config.moonlight_features & haptics::FEATURE != 0
        && channels > usize::from(haptics::CHANNEL)
}
fn feedback_packets(id: u16, kind: u16, data: &[u8]) -> Vec<(u16, Vec<u8>)> {
    if kind == controller_audio::CONTROL && data.len() == 12 {
        return vec![(kind, data.to_vec())];
    }
    if (kind == RUMBLE_DATA && data.len() == 4)
        || (kind == 0x5502 && data.len() == 3)
        || (kind == 0x5503 && data.len() == 23)
    {
        let mut payload = if kind == RUMBLE_DATA {
            RUMBLE_MARKER.to_le_bytes().to_vec()
        } else {
            vec![]
        };
        payload.extend_from_slice(&id.to_le_bytes());
        payload.extend_from_slice(data);
        return vec![(kind, payload)];
    }
    let mut result = vec![];
    if matches!(kind, 4 | 5) && data.len() >= 8 {
        let mut rumble = RUMBLE_MARKER.to_le_bytes().to_vec();
        rumble.extend_from_slice(&id.to_le_bytes());
        rumble.extend_from_slice(&data[..4]);
        result.push((RUMBLE_DATA, rumble));
        if kind == 4 {
            result.push((
                RUMBLE_TRIGGER_DATA,
                [&id.to_le_bytes()[..], &data[4..8]].concat(),
            ));
        } else if kind == 5 && data[7] & 1 != 0 {
            result.push((0x5502, [&id.to_le_bytes()[..], &data[4..7]].concat()));
        }
        if kind == 5 && data.len() == 32 && data[7] & 2 != 0 {
            // moonlight-common-c: id, event flags (0x08 left, 0x04 right),
            // left type, right type, then each trigger's ten parameters.
            // The driver reports both triggers at once.
            let mut trigger = id.to_le_bytes().to_vec();
            trigger.extend_from_slice(&[0x0c, data[10], data[21]]);
            trigger.extend_from_slice(&data[11..21]);
            trigger.extend_from_slice(&data[22..32]);
            result.push((0x5503, trigger));
        }
    }
    result
}

#[cfg(test)]
mod tests {

    #[test]
    fn overrides_saved_for_rtx_hdr_keep_only_the_peak_brightness() {
        let mut config = Config::parse("port = 47989\n").unwrap();
        let overrides = serde_json::json!({
            "rtx_hdr": true,
            "rtx_hdr_contrast": 14,
            "rtx_hdr_peak_brightness": 1200,
            "encoder": "software",
        });
        let overrides = overrides.as_object().unwrap();
        assert!(super::sets_peak(overrides));
        super::apply_overrides(&mut config, overrides).unwrap();
        assert_eq!(config.integer("hdr_peak_brightness", 0), 1200);
        assert_eq!(config.get("encoder", ""), "software");
        assert!(!config.values.contains_key("rtx_hdr"));
        assert!(!config.values.contains_key("rtx_hdr_contrast"));
        assert!(!super::sets_peak(
            serde_json::json!({"hdr_peak_brightness": ""})
                .as_object()
                .unwrap()
        ));
    }
    #[test]
    fn waveform_haptics_require_negotiation_and_encrypted_multichannel_control() {
        let mut config = butterpollo_core::rtsp::Negotiated::from_sdp(
            b"a=x-ml-general.featureFlags:7\na=x-ss-general.encryptionEnabled:1\n",
        )
        .unwrap();
        assert!(haptics_negotiated(&config, 9));
        assert!(!haptics_negotiated(&config, 8));
        config.encryption = 0;
        assert!(!haptics_negotiated(&config, 9));
        config.encryption = 1;
        config.moonlight_features = 3;
        assert!(!haptics_negotiated(&config, 9));
        assert!(!haptics_negotiated(&Default::default(), 9));
    }
    #[test]
    fn composite_feedback_preserves_partial_trigger_flags_and_rumble_layout() {
        let mut effect = vec![0; 23];
        effect[..3].copy_from_slice(&[4, 1, 0]);
        let packet = feedback_packets(2, 0x5503, &effect).remove(0);
        assert_eq!(&packet.1[..5], &[2, 0, 4, 1, 0]);
        let rumble = feedback_packets(2, RUMBLE_DATA, &[0, 9, 0, 7]).remove(0);
        assert_eq!(&rumble.1, &[0xee, 0xff, 0xc0, 0, 2, 0, 0, 9, 0, 7]);
    }
    #[test]
    fn control_backlog_is_serviced_without_the_feedback_delay() {
        for queued in [600, 1100] {
            let mut remaining = queued;
            while remaining >= super::CONTROL_EVENT_LIMIT {
                assert_eq!(
                    super::control_wait_timeout(
                        Duration::from_millis(8),
                        super::CONTROL_EVENT_LIMIT
                    ),
                    0
                );
                remaining -= super::CONTROL_EVENT_LIMIT;
            }
            assert_eq!(
                super::control_wait_timeout(Duration::from_millis(8), remaining),
                8
            );
        }
        assert_eq!(super::control_wait_timeout(Duration::ZERO, 0), 0);
        assert_eq!(
            super::control_wait_timeout(Duration::from_micros(100), 0),
            1
        );
    }
    use super::*;
    #[test]
    fn ignored_config_values_and_host_wide_overrides_are_logged() {
        use crate::state::test_support::Fixture;
        let f = Fixture::new();
        let path = f.host.directory.join("warnings.log");
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::WARN)
            .without_time()
            .with_ansi(false)
            .with_target(false)
            .with_writer(std::sync::Mutex::new(std::fs::File::create(&path).unwrap()))
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            let mut config = Config::parse("parity_number=not-a-number").unwrap();
            assert_eq!(config.integer("parity_number", 7), 7);
            apply_overrides(
                &mut config,
                serde_json::json!({"port":50000}).as_object().unwrap(),
            )
            .unwrap();
            assert!(!config.values.contains_key("port"));
        });
        let log = std::fs::read_to_string(path).unwrap();
        assert!(log.contains("parity_number"), "{log}");
        assert!(log.contains("not-a-number"), "{log}");
        assert!(
            log.contains("port") && log.contains("ignoring an override"),
            "{log}"
        );
    }
    #[test]
    fn server_commands_require_permission_app_opt_in_and_a_valid_index_and_are_throttled() {
        use crate::state::test_support::Fixture;
        use serde_json::json;
        let f = Fixture::new();
        let mut client = f.client(1 << 20);
        f.host.config.write().unwrap().values.insert("server_cmd".into(), json!([
            {"name":"First","cmd":"echo first"}, {"name":"Second","cmd":"echo second","elevated":true}
        ]).to_string());
        let mut at = None;
        assert_eq!(server_command(&f.host, &client, &[1], false, &mut at), None);
        assert!(at.is_none());
        assert_eq!(
            server_command(&f.host, &client, &[1], true, &mut at),
            Some(("echo second".into(), true))
        );
        assert!(at.is_some());
        assert_eq!(server_command(&f.host, &client, &[0], true, &mut at), None);
        at = Some(Instant::now() - Duration::from_secs(2));
        assert_eq!(
            server_command(&f.host, &client, &[0], true, &mut at),
            Some(("echo first".into(), false))
        );
        for payload in [vec![], vec![0, 1], vec![2]] {
            at = None;
            assert_eq!(
                server_command(&f.host, &client, &payload, true, &mut at),
                None
            );
        }
        for (permission, enabled) in [(0, true), (1 << 20, false)] {
            client.perm = permission;
            client.enabled = enabled;
            at = None;
            assert_eq!(server_command(&f.host, &client, &[0], true, &mut at), None);
            assert!(at.is_none());
        }
        client.perm = 1 << 20;
        client.enabled = true;
        let app =
            serde_json::from_value(json!({"name":"No commands","allow-client-commands":false}))
                .unwrap();
        *f.host.current_app.lock().unwrap() =
            Some(crate::process::RunningApp::with_environment(&app, Default::default()).unwrap());
        assert_eq!(server_command(&f.host, &client, &[0], true, &mut at), None);
        assert!(at.is_none());
    }
    #[test]
    fn app_overrides_win_over_device_and_host_without_changing_saved_configuration() {
        use crate::state::test_support::Fixture;
        use serde_json::json;
        let f = Fixture::new();
        *f.host.config.write().unwrap() =
            Config::parse("max_bitrate=10000\ncapture=ddx\nport=47989\nkeyboard=true").unwrap();
        let original = f.host.config.read().unwrap().values.clone();
        let mut client = f.client(u32::MAX);
        client.extra.insert(
            "config_overrides".into(),
            json!({"max_bitrate":20000,"capture":"wgc","port":50000,"keyboard":false,"host_adaptive_pacing":true}),
        );
        let launch = f.launch(client, Role::Stream);
        let device = effective_config(&f.host, &launch).unwrap();
        assert_eq!(device.integer("max_bitrate", 0), 20000);
        assert_eq!(device.get("capture", ""), "wgc");
        assert!(!device.boolean("keyboard", true));
        assert!(device.adaptive_packet_pacing(0));
        assert!(!f.host.config.read().unwrap().adaptive_packet_pacing(0));
        assert_eq!(device.integer("port", 0), 47989);
        f.host.apps.write().unwrap()[0].extra.insert(
            "config-overrides".into(),
            json!({"max_bitrate":30000,"capture":"","port":51000,"keyboard":true,"host_adaptive_pacing":false}),
        );
        let app = effective_config(&f.host, &launch).unwrap();
        assert_eq!(app.integer("max_bitrate", 0), 30000);
        assert_eq!(app.get("capture", ""), "wgc");
        assert!(app.boolean("keyboard", false));
        assert!(!app.adaptive_packet_pacing(0));
        assert_eq!(app.integer("port", 0), 47989);
        assert_eq!(f.host.config.read().unwrap().values, original);
        assert_eq!(
            launch.client.extra["config_overrides"]["max_bitrate"],
            20000
        );
        assert_eq!(
            launch.client.extra["config_overrides"]["host_adaptive_pacing"],
            true
        );
        assert_eq!(
            f.host.apps.read().unwrap()[0].extra["config-overrides"]["port"],
            51000
        );
    }
    #[test]
    fn a_background_stream_keeps_keyboard_and_mouse_off_whatever_overrides_say() {
        use crate::state::test_support::Fixture;
        use serde_json::json;
        let f = Fixture::new();
        let mut client = f.client(u32::MAX);
        client.extra.insert(
            "config_overrides".into(),
            json!({"keyboard":true,"mouse":true,"audio_capture_mode":"system"}),
        );
        let plain = effective_config(&f.host, &f.launch(client.clone(), Role::Stream)).unwrap();
        assert!(!butterpollo_core::background::enabled(&plain));
        assert!(plain.boolean("keyboard", false));
        // From the app.
        f.host.apps.write().unwrap()[0]
            .extra
            .insert("background-stream".into(), json!(true));
        let launch = f.launch(client.clone(), Role::Stream);
        let app = effective_config(&f.host, &launch).unwrap();
        assert!(butterpollo_core::background::enabled(&app));
        assert!(!app.boolean("keyboard", true));
        assert!(!app.boolean("mouse", true));
        assert_eq!(app.get("audio_capture_mode", ""), "game");
        assert_eq!(
            crate::display_session::virtual_display_layout(
                &app,
                &launch.client,
                None,
                Some(butterpollo_core::display_policy::HostDisplay::Virtual(
                    butterpollo_core::display_policy::Arrangement::Exclusive
                )),
            ),
            "extended_isolated"
        );
        // From the device, for every app; only streams take it.
        f.host.apps.write().unwrap()[0]
            .extra
            .remove("background-stream");
        client.extra.insert("background_stream".into(), json!(true));
        let device = effective_config(&f.host, &f.launch(client.clone(), Role::Stream)).unwrap();
        assert!(butterpollo_core::background::enabled(&device));
        let input = effective_config(&f.host, &f.launch(client, Role::InputOnly)).unwrap();
        assert!(!butterpollo_core::background::enabled(&input));
    }
    #[test]
    fn capture_waits_for_its_display_and_shows_the_primary_only_after_the_timeout() {
        let start = Instant::now();
        let mut missing = None;
        assert_eq!(reopen_on(true, &mut missing, start), ReopenOn::Stream);
        // Windows switched the virtual display off: capture waits for the
        // heartbeat to bring it back instead of opening the physical desktop.
        assert_eq!(reopen_on(false, &mut missing, start), ReopenOn::Wait);
        let almost = start + DISPLAY_RETURN_WAIT - Duration::from_millis(1);
        assert_eq!(reopen_on(false, &mut missing, almost), ReopenOn::Wait);
        assert_eq!(
            reopen_on(false, &mut missing, start + DISPLAY_RETURN_WAIT),
            ReopenOn::Primary
        );
        // It came back under another name: the wait starts over next time.
        let back = start + Duration::from_secs(20);
        assert_eq!(reopen_on(true, &mut missing, back), ReopenOn::Stream);
        assert_eq!(missing, None);
        assert_eq!(reopen_on(false, &mut missing, back), ReopenOn::Wait);
    }
    #[test]
    fn pad_feedback_forwards_rumble_and_each_family_extra() {
        let rumble = (
            RUMBLE_DATA,
            vec![0xee, 0xff, 0xc0, 0, 2, 0, 255, 255, 128, 128],
        );
        // Xbox pads add trigger rumble.
        let xbox = feedback_packets(2, 4, &[255, 255, 128, 128, 1, 2, 3, 4]);
        assert_eq!(
            xbox,
            vec![
                rumble.clone(),
                (RUMBLE_TRIGGER_DATA, vec![2, 0, 1, 2, 3, 4])
            ]
        );
        // PlayStation pads add the lightbar only when the report sets it.
        let ds4 = feedback_packets(2, 5, &[255, 255, 128, 128, 12, 34, 56, 1]);
        assert_eq!(ds4, vec![rumble.clone(), (0x5502, vec![2, 0, 12, 34, 56])]);
        assert_eq!(
            feedback_packets(2, 5, &[255, 255, 128, 128, 12, 34, 56, 0]),
            vec![rumble]
        );
        // Reports of no known kind send nothing.
        assert!(feedback_packets(2, 3, &[255; 8]).is_empty());
    }
    #[test]
    fn vhf_rumble_and_stop_keep_the_strengths_in_moonlight_packets() {
        for (kind, size) in [(4, 8), (5, 32)] {
            for strengths in [[0x00, 0x80, 0x00, 0x40], [0; 4]] {
                let mut data = vec![0; size];
                data[..4].copy_from_slice(&strengths);
                let packets = feedback_packets(0, kind, &data);
                assert_eq!(packets[0].0, RUMBLE_DATA);
                assert_eq!(RUMBLE_DATA, 0x010b);
                assert_eq!(&packets[0].1[..4], &RUMBLE_MARKER.to_le_bytes());
                assert_eq!(&packets[0].1[4..], &[&[0, 0][..], &strengths].concat());
            }
        }
    }
    #[test]
    fn rumble_reaches_a_single_channel_moonlight_peer() {
        let settings = || HostSettings {
            peer_limit: 1,
            channel_limit: 1,
            ..Default::default()
        };
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let address = socket.local_addr().unwrap();
        let mut host = Host::new(ControlSocket::new(socket), settings()).unwrap();
        let mut client = Host::new(UdpSocket::bind("127.0.0.1:0").unwrap(), settings()).unwrap();
        client.connect(address, 1, 0).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !host.peer(PeerID(0)).connected() || !client.peer(PeerID(0)).connected() {
            host.service().unwrap();
            client.service().unwrap();
            assert!(Instant::now() < deadline, "ENet connect timed out");
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(host.peer(PeerID(0)).channel_count(), 1);
        // The previous channel silently discarded every feedback message.
        assert_eq!(
            host.peer_mut(PeerID(0))
                .send(1, &Packet::new(&[][..], PacketKind::Reliable)),
            Err(rusty_enet::error::PeerSendError::InvalidChannel)
        );
        let key = [0x42; 16];
        for v2 in [false, true] {
            for strengths in [[0x00, 0x80, 0x00, 0x40], [0; 4]] {
                let mut data = vec![0; 8];
                data[..4].copy_from_slice(&strengths);
                let (kind, payload) = feedback_packets(2, 4, &data).remove(0);
                let message =
                    butterpollo_core::packet::encrypted_control(&key, 7, kind, &payload, v2)
                        .unwrap();
                host.socket_mut().hold();
                host.peer_mut(PeerID(0))
                    .send(
                        FEEDBACK_CHANNEL,
                        &Packet::new(message, PacketKind::Reliable),
                    )
                    .unwrap();
                host.flush();
                host.socket_mut().release().unwrap();
                let deadline = Instant::now() + Duration::from_secs(2);
                let received = loop {
                    host.service().unwrap();
                    if let Some(Event::Receive {
                        channel_id, packet, ..
                    }) = client.service().unwrap()
                    {
                        assert_eq!(channel_id, 0);
                        break packet;
                    }
                    assert!(Instant::now() < deadline, "rumble delivery timed out");
                    std::thread::sleep(Duration::from_millis(1));
                };
                let bytes = received.data();
                let mut iv = vec![0; if v2 { 12 } else { 16 }];
                iv[0] = 7;
                if v2 {
                    iv[10..].copy_from_slice(b"HC");
                }
                let plain =
                    butterpollo_core::crypto::gcm_open(&key, &iv, &bytes[8..24], &bytes[24..])
                        .unwrap();
                // Moonlight's decrypted V2 header, marker, controller ID,
                // then the low- and high-frequency strengths, all LE.
                assert_eq!(
                    &plain[..10],
                    &[0x0b, 0x01, 10, 0, 0xee, 0xff, 0xc0, 0, 2, 0]
                );
                assert_eq!(&plain[10..], &strengths);
            }
        }
    }
    #[test]
    fn an_override_the_host_cannot_use_is_skipped_and_the_rest_apply() {
        let mut config = Config::default();
        let overrides =
            serde_json::json!({"frame_limiter_fps_limit": "-1", "fec_percentage": "30"});
        apply_overrides(&mut config, overrides.as_object().unwrap()).unwrap();
        assert_eq!(config.get("fec_percentage", ""), "30");
        assert!(!config.values.contains_key("frame_limiter_fps_limit"));
    }
    #[test]
    fn accepting_input_without_output_cannot_extend_encoder_recovery() {
        let start = Instant::now();
        let mut failing = Some(start);
        for seconds in 0..5 {
            encoder_progress(&mut failing, false, start + Duration::from_secs(seconds)).unwrap();
            assert_eq!(failing, Some(start));
        }
        assert!(encoder_progress(&mut failing, false, start + ENCODER_RECOVERY).is_err());
    }
    #[test]
    fn completed_output_ends_recovery_and_allows_a_later_independent_failure() {
        let start = Instant::now();
        let mut failing = Some(start);
        encoder_progress(&mut failing, true, start + Duration::from_secs(4)).unwrap();
        assert_eq!(failing, None);
        encoder_progress(&mut failing, false, start + ENCODER_RECOVERY).unwrap();
        let later = start + ENCODER_RECOVERY + Duration::from_secs(1);
        failing = Some(later);
        encoder_progress(&mut failing, false, later + Duration::from_secs(1)).unwrap();
        assert!(encoder_progress(&mut failing, false, later + ENCODER_RECOVERY).is_err());
    }
    #[test]
    fn wgc_interval_is_unrestricted_by_default_and_preserves_overrides() {
        let default = Config::default();
        assert!(!capture_config(&default).boolean("wgc_high_rate_capture", true));
        for (setting, expected) in [("true", true), ("false", false)] {
            let config = Config::parse(&format!("wgc_high_rate_capture = {setting}")).unwrap();
            let original = config.values.clone();
            assert_eq!(
                capture_config(&config).boolean("wgc_high_rate_capture", !expected),
                expected
            );
            assert_eq!(config.values, original);
        }
        assert!(!default.values.contains_key("wgc_high_rate_capture"));
    }
    #[test]
    fn shared_capture_separates_explicit_wgc_intervals_without_splitting_ddx() {
        let key = |kind, setting: &str| {
            CaptureKey::new(
                kind,
                "display",
                false,
                &capture_config(&Config::parse(setting).unwrap()),
                "",
            )
        };
        for kind in ["wgc", "auto"] {
            assert_eq!(key(kind, ""), key(kind, "wgc_high_rate_capture=false"));
            assert_ne!(key(kind, ""), key(kind, "wgc_high_rate_capture=true"));
            assert_ne!(key(kind, ""), key(kind, "wgc_drain_to_newest=true"));
            assert_ne!(key(kind, ""), key(kind, "wgc_helper_streaming_scope=true"));
        }
        for kind in ["ddx", "dxgi"] {
            assert_eq!(key(kind, ""), key(kind, "wgc_high_rate_capture=true"));
            assert_eq!(key(kind, ""), key(kind, "wgc_high_rate_capture=false"));
            assert_eq!(key(kind, ""), key(kind, "wgc_drain_to_newest=true"));
            assert_eq!(key(kind, ""), key(kind, "wgc_helper_streaming_scope=true"));
        }
    }
    #[test]
    fn shared_capture_respects_compute_overrides_and_independent_frame_phases() {
        let config = Config::default();
        let key =
            |config: &Config, phase: &str| CaptureKey::new("wgc", "display", false, config, phase);
        let default = key(&config, "first");
        assert_eq!(default, key(&config, "second"));
        let mut helper = config.clone();
        helper
            .values
            .insert("wgc_user_helper".into(), "true".into());
        assert_ne!(default, key(&helper, "first"));
        let mut fenced = config.clone();
        fenced
            .values
            .insert("idd_push_fence_ring".into(), "true".into());
        assert_ne!(default, key(&fenced, "first"));
        let mut changed = config.clone();
        changed
            .values
            .insert("wgc_compute_copy".into(), "false".into());
        assert_ne!(default, key(&changed, "first"));
        changed
            .values
            .insert("wgc_compute_copy".into(), "true".into());
        assert_eq!(default, key(&changed, "first"));
        changed
            .values
            .insert("gpu_compute_conversion".into(), "false".into());
        assert_ne!(default, key(&changed, "first"));
        changed
            .values
            .insert("wgc_slot_aligned_publish".into(), "true".into());
        assert_ne!(key(&changed, "first"), key(&changed, "second"));
    }
    #[test]
    fn dualsense_trigger_effects_use_the_moonlight_layout() {
        let mut data = vec![0u8; 32];
        data[..4].copy_from_slice(&[1, 2, 3, 4]);
        data[7] = 2;
        data[10] = 0x21;
        for (i, byte) in data[11..21].iter_mut().enumerate() {
            *byte = i as u8 + 1;
        }
        data[21] = 0x26;
        for (i, byte) in data[22..32].iter_mut().enumerate() {
            *byte = i as u8 + 11;
        }
        let packets = feedback_packets(1, 5, &data);
        let (_, trigger) = packets.iter().find(|(kind, _)| *kind == 0x5503).unwrap();
        let mut expected = vec![1, 0, 0x0c, 0x21, 0x26];
        expected.extend(1..=20);
        assert_eq!(trigger, &expected);
        assert!(packets.iter().any(|(kind, _)| *kind == RUMBLE_DATA));
    }
    #[test]
    fn held_control_datagrams_go_out_in_order_once_released() {
        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        let address = receiver.local_addr().unwrap();
        let mut socket = ControlSocket::new(UdpSocket::bind("127.0.0.1:0").unwrap());
        let mut buffer = [0u8; 64];
        socket.hold();
        // The middle one is too large for UDP: it is lost, the rest go out.
        for datagram in [&b"one"[..], &[0; 70_000], b"two"] {
            assert_eq!(
                rusty_enet::Socket::send(&mut socket, address, datagram).unwrap(),
                datagram.len()
            );
        }
        receiver.set_nonblocking(true).unwrap();
        assert!(receiver.recv_from(&mut buffer).is_err());
        socket.release().unwrap();
        receiver.set_nonblocking(false).unwrap();
        receiver
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut received = Vec::new();
        for _ in 0..2 {
            let (length, _) = receiver.recv_from(&mut buffer).unwrap();
            received.push(buffer[..length].to_vec());
        }
        assert_eq!(received, [b"one".to_vec(), b"two".to_vec()]);
        // Released: sent at once again.
        rusty_enet::Socket::send(&mut socket, address, b"three").unwrap();
        let (length, _) = receiver.recv_from(&mut buffer).unwrap();
        assert_eq!(&buffer[..length], b"three");
    }
    #[test]
    fn an_arrived_pad_is_asked_for_the_sensors_it_has() {
        assert_eq!(
            motion_requests(3, 0x30),
            [
                (MOTION_EVENT_REQUEST, vec![3, 0, 200, 0, 1]),
                (MOTION_EVENT_REQUEST, vec![3, 0, 200, 0, 2])
            ]
        );
        assert_eq!(
            motion_requests(1, 0x20),
            [(MOTION_EVENT_REQUEST, vec![1, 0, 200, 0, 2])]
        );
        assert!(motion_requests(1, 0x0f).is_empty());
    }
}
