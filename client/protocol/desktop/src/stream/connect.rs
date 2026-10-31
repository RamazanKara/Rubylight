//! Connecting a stream: the launch on a worker thread, its recovery and route checks, the
//! session configuration, and the decisions behind it (beam race, link hints, roaming, rejoin).

use super::*;

fn stage_text(stage: Stage) -> &'static str {
    match stage {
        Stage::PlatformInit | Stage::NameResolution => en("Finding the PC"),
        Stage::AudioStreamInit | Stage::RtspHandshake => en("Negotiating the stream"),
        Stage::ControlStreamInit | Stage::ControlStreamStart => en("Opening the control channel"),
        Stage::VideoStreamInit | Stage::VideoStreamStart => en("Starting video"),
        Stage::AudioStreamStart => en("Starting audio"),
        Stage::InputStreamInit | Stage::InputStreamStart => en("Connecting input"),
    }
}

pub fn termination_text(code: i32) -> Option<String> {
    use nightjar_core::session::error;
    Some(match code {
        error::GRACEFUL => return None,
        error::NO_VIDEO_TRAFFIC => {
            en("No video arrived from the PC. A firewall may be blocking UDP ports 47998-48000.").into()
        }
        error::NO_VIDEO_FRAME => en("Video arrived but no complete picture could be assembled.").into(),
        error::UNEXPECTED_EARLY => en("The PC ended the stream before it started.").into(),
        error::PROTECTED_CONTENT => en("The PC stopped streaming protected content.").into(),
        error::FRAME_CONVERSION => en("The PC could not convert the picture for streaming.").into(),
        error::DISCONNECTED => en("The connection to the PC was lost.").into(),
        other => trf("The stream ended unexpectedly (error {}).", &[&other]),
    })
}

/// Why a launch ended without a stream.
pub enum ConnectFailed {
    /// What to tell the user.
    Message(String),
    /// The PC asks the user to confirm closing the running app; the plan launches again with
    /// the answer.
    ConfirmReplace(Box<LaunchPlan>),
}

/// A launch and session start running on a worker thread.
pub struct Connecting {
    pub shared: Arc<Shared>,
    pub interrupt: Interrupt,
    pub stage: String,
    pub app_name: String,
    pub host_name: String,
    pub host_id: u64,
    pub pacing: PacingMode,
    pub settings: Settings,
    pub(super) plan: LaunchPlan,
    pub(super) result: Receiver<Result<(Session, Facts, LaunchPlan), ConnectError>>,
}

impl Connecting {
    /// The game this stream plays, for the recent list; None when the PC has no unique id yet
    /// or the stream is not a game (a remote monitor, input only).
    pub fn recent_game(&self) -> Option<config::Recent> {
        let record = &self.plan.record;
        if record.unique_id.is_empty() || self.plan.app.id < 0 || self.plan.app.control != nightjar_core::http::AppControl::None {
            return None;
        }
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        Some(config::Recent {
            host: record.unique_id.clone(),
            install_id: self.plan.app.uuid.clone(),
            app_id: self.plan.app.id as u32,
            title: self.plan.app.name.clone(),
            at: now,
        })
    }

    pub fn start(mut plan: LaunchPlan, waker: Waker) -> Self {
        if let Some(recovery) = &mut plan.recovery { recovery.begin(recovery::now_ms()); }
        // Host-time pacing schedules every picture; the other modes keep only the newest.
        let limit = if plan.pacing == PacingMode::HostTimes { 8 } else { 2 };
        let shared = Shared::new(waker.clone(), limit);
        let interrupt = Interrupt::default();
        let (sender, result) = channel();
        let app_name = plan.app.name.clone();
        let host_name = plan.host_name.clone();
        let (host_id, pacing, settings) = (plan.host_id, plan.pacing, plan.settings.clone());
        let worker_shared = shared.clone();
        let worker_interrupt = interrupt.clone();
        let mut worker_plan = plan.clone();
        let _ = std::thread::Builder::new()
            .name("connect".into())
            .spawn(move || {
                let result = connect_with_recovery(&mut worker_plan, worker_shared, &worker_interrupt)
                    .map(|(session, facts)| (session, facts, worker_plan));
                let _ = sender.send(result);
                waker();
            });
        Self {
            shared,
            interrupt,
            stage: en("Starting the app on the PC").into(),
            app_name,
            host_name,
            host_id,
            pacing,
            settings,
            plan,
            result,
        }
    }

    /// The stream presents for a VRR display (the setting is on and the swap chain tears).
    pub fn vrr(&self) -> bool { self.plan.vrr }

    /// Ok(Some) once streaming, Err on failure.
    pub fn poll(&mut self) -> Result<Option<(Session, Facts)>, ConnectFailed> {
        let expired = self.plan.recovery.as_ref().is_some_and(|r| r.remaining(recovery::now_ms()) == 0);
        if expired { self.cancel(); }
        let notices = std::mem::take(&mut *lock(&self.shared.notices));
        let mut keep = Vec::new();
        for notice in notices {
            match notice {
                Notice::Stage(stage) => self.stage = stage_text(stage).into(),
                Notice::Reconnecting(attempt) => {
                    keep.clear();
                    self.stage = trf("Reconnecting · attempt {} of 5", &[&attempt]);
                }
                // The rest is for the stream.
                other => keep.push(other),
            }
        }
        if !keep.is_empty() {
            let mut queue = lock(&self.shared.notices);
            keep.append(&mut queue);
            *queue = keep;
        }
        match self.result.try_recv() {
            Ok(Ok((session, facts, plan))) => {
                if expired {
                    lock(&self.shared.roam_watch).take();
                    session.stop();
                    Err(ConnectFailed::Message("Automatic recovery stopped after one minute.".into()))
                } else {
                    self.plan = plan;
                    Ok(Some((session, facts)))
                }
            }
            Ok(Err(e)) => Err(if expired {
                ConnectFailed::Message("Automatic recovery stopped after one minute.".into())
            } else if e.asks_replace {
                ConnectFailed::ConfirmReplace(Box::new(self.plan.clone()))
            } else {
                ConnectFailed::Message(e.message)
            }),
            Err(std::sync::mpsc::TryRecvError::Empty) => Ok(None),
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Err(ConnectFailed::Message("The connection attempt ended.".into())),
        }
    }

    pub fn cancel(&self) {
        self.interrupt.interrupt();
        if let Some(client) = &*lock(&self.shared.host_client) {
            client.cancel_pending_requests();
        }
    }
}

pub(super) struct ConnectError {
    pub(super) message: String,
    pub(super) retryable: bool,
    /// The PC asks the user to confirm closing the running app.
    pub(super) asks_replace: bool,
}

impl From<String> for ConnectError {
    fn from(message: String) -> Self { Self { message, retryable: false, asks_replace: false } }
}

impl From<&str> for ConnectError {
    fn from(message: &str) -> Self { message.to_owned().into() }
}

fn connect_with_recovery(plan: &mut LaunchPlan, shared: Arc<Shared>, interrupt: &Interrupt) -> Result<(Session, Facts), ConnectError> {
    loop {
        if let Some(recovery) = &mut plan.recovery {
            let delay = recovery.next_delay(recovery::now_ms()).ok_or("Automatic recovery exhausted its retry budget.")?;
            shared.notify(Notice::Reconnecting(recovery.attempts() as usize));
            let until = Instant::now() + Duration::from_millis(delay);
            while Instant::now() < until {
                if interrupt.is_interrupted() { return Err("Cancelled.".into()); }
                std::thread::sleep(Duration::from_millis(20).min(until.saturating_duration_since(Instant::now())));
            }
            let port = if plan.record.http_port == 0 { nightjar_core::http::DEFAULT_HTTP_PORT } else { plan.record.http_port };
            wait_for_route(&plan.record.address, port, recovery.remaining(recovery::now_ms()), interrupt)?;
        }
        match connect(plan, shared.clone(), interrupt) {
            Err(error) if error.retryable && plan.recovery.is_some() && !interrupt.is_interrupted() => {
                lock(&shared.frames).clear();
                shared.next_attempt();
            }
            result => return result,
        }
    }
}

/// Before a recovery attempt: returns once the PC is reachable, or when the recovery window ends
/// (the attempt then fails and the budget says so), Err when cancelled. A network is often back
/// before its address toward the PC (DHCP), so the attempt waits for a route instead of failing
/// at once and backing off.
fn wait_for_route(address: &str, port: u16, remaining_ms: u64, interrupt: &Interrupt) -> Result<(), ConnectError> {
    let started = Instant::now();
    let mut wait = recovery::RouteWait::new(recovery::now_ms() + remaining_ms);
    loop {
        if interrupt.is_interrupted() { return Err("Cancelled.".into()); }
        let checked = Instant::now();
        let problem = route_problem(address, port);
        match wait.next(recovery::now_ms(), problem.is_none()) {
            Some(0) => {
                if wait.unreachable_checks() > 0 {
                    eprintln!("nightjar: recovery: the PC is reachable again after {} ms", started.elapsed().as_millis());
                }
                return Ok(());
            }
            None => {
                eprintln!("nightjar: recovery: the PC stayed unreachable until the recovery window ended: {}",
                    problem.unwrap_or_default());
                return Ok(());
            }
            Some(delay) => {
                if wait.unreachable_checks() == 1 {
                    eprintln!("nightjar: recovery: no route to the PC yet ({}), checking every {} ms",
                        problem.unwrap_or_default(), recovery::ROUTE_RETRY_MS);
                }
                let until = checked + Duration::from_millis(delay);
                while Instant::now() < until {
                    if interrupt.is_interrupted() { return Err("Cancelled.".into()); }
                    std::thread::sleep(Duration::from_millis(20).min(until.saturating_duration_since(Instant::now())));
                }
            }
        }
    }
}

/// Why there is no route toward the PC's HTTP port yet ([`recovery::route_missing`], or the
/// name does not resolve), or None to go on with the attempt: an answer, refused included, and
/// also a check that timed out or failed otherwise.
fn route_problem(address: &str, port: u16) -> Option<String> {
    let remote = match (address, port).to_socket_addrs().map(|mut found| found.next()) {
        Ok(Some(remote)) => remote,
        Ok(None) => return Some(format!("{address} did not resolve")),
        Err(error) => return Some(error.to_string()),
    };
    match std::net::TcpStream::connect_timeout(&remote, Duration::from_millis(recovery::ROUTE_PROBE_TIMEOUT_MS)) {
        Err(error) if recovery::route_missing(&error) => Some(error.to_string()),
        _ => None,
    }
}

/// What early decode does on a stream. `NoRecords` is the failure the earlier measurements
/// hit unseen: a host whose version did not parse never offered records, so no chunk ever came.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EarlyState {
    Off,
    On,
    NoRecords,
}

const EARLY_NO_RECORDS: &str = "nightjar: early decode is on but the host sent no PyroWave records \
    (its version did not offer them, or the client did not ask), so frames are decoded whole";

pub(super) fn early_state(requested: bool, pyrowave: bool, records: bool) -> EarlyState {
    match (requested && pyrowave, records) {
        (false, _) => EarlyState::Off,
        (true, true) => EarlyState::On,
        (true, false) => EarlyState::NoRecords,
    }
}

/// What the beam race setting does on a stream: the client asks the host for the row order and
/// decodes the picture in slices as its bands complete. `NotWired` is the setting on a link that
/// is not known to be wired (the host's row order is for wired links), `NoGpuDecoder` the setting
/// when the stream decodes PyroWave on the CPU (only the GPU decoder slices a frame). Both are
/// reported, not silent, and neither asks the host for the row order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum BeamRace {
    Off,
    On,
    NotWired,
    NoGpuDecoder,
}

impl BeamRace {
    /// Why the setting did nothing, for the overlay.
    pub(super) fn reason(self) -> Option<&'static str> {
        match self {
            Self::NotWired => Some("off, the link is not a known wired one"),
            Self::NoGpuDecoder => Some("off, PyroWave is decoded on the CPU"),
            Self::Off | Self::On => None,
        }
    }
}

/// Everything the beam race setting asks of a stream, in one place: the row order from the host,
/// early decode (the slices are decoded from chunks) and the slice count. All of it is off
/// unless the decision is `On`, so a stream without the setting asks the host for nothing, starts
/// no scan line probe and decodes whole frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct RaceRequest {
    pub(super) row_order: bool,
    pub(super) early_decode: bool,
    pub(super) slices: u32,
}

pub(super) fn race_request(beam: BeamRace, early_setting: bool, height: u32) -> RaceRequest {
    let on = beam == BeamRace::On;
    RaceRequest { row_order: on, early_decode: early_setting || on, slices: if on { beam_slices(height) } else { 0 } }
}

/// Whether the race runs on the stream (the presenter probes the display, follows the slices and
/// races them): slices were asked for, and early decode has the records they come from. This is
/// the one bool the presenter checks; false makes it all a no-op.
pub(super) fn race_runs(slices: u32, early: EarlyState) -> bool {
    slices > 0 && early == EarlyState::On
}

const BEAM_RACE_NO_GPU: &str = "nightjar: beam race is on but PyroWave is decoded on the CPU, so the host is not asked \
    for the row order and frames are decoded whole";

const BEAM_RACE_NOT_WIRED: &str = "nightjar: beam race is on but the link is not a known wired one, so the host is not asked \
    for the row order and frames are decoded whole";

/// An on or off switch in an environment variable: `1`, `true`, `yes` or `on`, and `0`, `false`,
/// `no` or `off` (any case). Anything else is not a switch.
pub(super) fn flag_value(text: &str) -> Option<bool> {
    match text.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

/// The switch `name` names, if it is set. A value that is not a switch is said so on stderr and
/// ignored, never taken as on or off.
pub(super) fn env_flag(name: &str) -> Option<bool> {
    let text = std::env::var(name).ok()?;
    let value = flag_value(&text);
    if value.is_none() {
        eprintln!("nightjar: {name}={text:?} is not 1, true, yes, on, 0, false, no or off, so it is ignored");
    }
    value
}

/// The setting, unless `NIGHTJAR_BEAM_RACE` says on (whatever the link; for measurement runs) or
/// off. Needs the GPU decoder whatever asked.
pub(super) fn beam_race(setting: bool, link_mbps: u32, gpu_decoder: bool) -> BeamRace {
    beam_race_for(env_flag("NIGHTJAR_BEAM_RACE"), setting, link_mbps, gpu_decoder)
}

pub(super) fn beam_race_for(forced: Option<bool>, setting: bool, link_mbps: u32, gpu_decoder: bool) -> BeamRace {
    match forced {
        Some(false) => BeamRace::Off,
        Some(true) if gpu_decoder => BeamRace::On,
        Some(true) => BeamRace::NoGpuDecoder,
        _ if !setting => BeamRace::Off,
        _ if !gpu_decoder => BeamRace::NoGpuDecoder,
        _ if link_mbps == 0 => BeamRace::NotWired,
        _ => BeamRace::On,
    }
}

/// Most slices per frame: a frame has a present for each batch of slices and one for the picture,
/// and the timing CSV has room for `PRESENTS` of them.
pub(super) const MAX_RACE_SLICES: u32 = crate::timing_csv::PRESENTS as u32 - 1;

/// Slices per frame: 4, or 3 at 4K where each slice decode costs more (`NIGHTJAR_BEAM_RACE_SLICES`
/// overrides, for measurement runs, up to `MAX_RACE_SLICES`).
pub(super) fn beam_slices(height: u32) -> u32 {
    let default = if height >= 2000 { 3 } else { 4 };
    match std::env::var("NIGHTJAR_BEAM_RACE_SLICES") {
        Ok(text) => slices_override(&text).unwrap_or_else(|| {
            eprintln!("nightjar: NIGHTJAR_BEAM_RACE_SLICES={text:?} is not a number from 1 to {MAX_RACE_SLICES}, so {default} slices are used");
            default
        }),
        Err(_) => default,
    }
}

pub(super) fn slices_override(text: &str) -> Option<u32> {
    text.trim().parse().ok().filter(|n| (1..=MAX_RACE_SLICES).contains(n))
}

/// Whether to send `x-rl-video[0].clientWireless`: the setting is on, the route is wireless and the
/// PC says it is a Rubylight host, the only host that reads it.
pub(super) fn tells_host_wireless(setting: bool, family: nightjar_core::http::Family, wireless: bool) -> bool {
    setting && wireless && family == nightjar_core::http::Family::Rubylight
}

/// The value a network-hint variable names, if it names one (measurement runs on loopback, where
/// the real link is not the one under test). A value that is not a number is said so on stderr
/// and ignored.
pub(super) fn hint_env(name: &str) -> Option<u32> {
    let text = std::env::var(name).ok()?;
    let parsed = text.trim().parse().ok();
    if parsed.is_none() {
        eprintln!("nightjar: {name}={text:?} is not a number, so it is ignored");
    }
    parsed
}

/// `value`, unless the variable names another one.
pub(super) fn hint_override(name: &str, value: u32) -> u32 {
    hint_env(name).unwrap_or(value)
}

/// The link speed and pace the stream works with and what it tells the host.
#[derive(Debug, PartialEq)]
pub(super) struct LinkHints {
    /// The speed the beam-race check goes by, whatever the host is told.
    pub(super) link_mbps: u32,
    /// `x-ss-video[0].pyrowaveLinkMbps`; zero leaves the attribute out.
    pub(super) sent_link_mbps: u32,
    /// `x-ss-video[0].pyrowavePaceMbps`, calibrated or overridden.
    pub(super) pace_mbps: u32,
}

/// With "Tell the PC the link speed" off the host gets no speed, but the beam race and the
/// calibrated pace (measured and keyed on the real speed) still use it. A variable naming the
/// speed wins over the setting, so measurement runs keep their hint.
pub(super) fn link_hints(tell: bool, routed_mbps: u32, link_env: Option<u32>, measured_pace: u32, pace_env: Option<u32>) -> LinkHints {
    let link_mbps = link_env.unwrap_or(routed_mbps);
    LinkHints {
        link_mbps,
        sent_link_mbps: if tell || link_env.is_some() { link_mbps } else { 0 },
        pace_mbps: pace_env.unwrap_or(measured_pace),
    }
}

/// `Auto` (moonlight-common-c's choice from the address), unless `NIGHTJAR_STREAM_LOCATION` is
/// `local` or `remote`: loopback counts as remote there and gets 1024 byte packets, which is not
/// the packet size of a wired LAN stream, so measurement runs on loopback say `local`.
fn location_override() -> Location {
    match std::env::var("NIGHTJAR_STREAM_LOCATION").as_deref() {
        Ok("local") => Location::Local,
        Ok("remote") => Location::Remote,
        _ => Location::Auto,
    }
}

/// The location and video packet size for a route to the PC: `Auto` over a VPN
/// ([`crate::network::route`]) streams as remote with 1024 byte packets, whatever the PC's
/// address, as Android does for a VPN network and Moonlight V+ PC for its `RI_VPN`: a tunnel's
/// smaller MTU (WireGuard's 1420) would fragment every 1392 byte packet. Otherwise 1392, and the
/// engine decides `Auto` from the address.
pub(super) fn location_for_route(location: Location, vpn: bool) -> (Location, i32) {
    match location {
        Location::Auto if vpn => (Location::Remote, 1024),
        location => (location, 1392),
    }
}

/// The engine's session flags for this role and these settings. A Remote Monitor session is
/// view-only: the engine starts no input stream and does not wiggle the host's mouse.
/// `watching` says the network watcher runs and feeds `Session::network_changed`.
pub(super) fn session_config(settings: &Settings, role: AppRole, stream: StreamConfig, location: Location, watching: bool) -> SessionConfig {
    SessionConfig {
        stream,
        location,
        network_grace: Duration::from_millis(u64::from(settings.network_grace_ms)),
        input_only: role == AppRole::InputOnly,
        remote_monitor: role == AppRole::RemoteMonitor,
        unbatched_input: settings.unbatched_input,
        network_priority: settings.network_priority,
        roaming: roaming_config(settings, watching),
    }
}

/// Client roaming as set. With the watcher running its hints say which networks are metered;
/// without it the session cannot tell, so roaming then needs "Continue on mobile data" too.
pub(super) fn roaming_config(settings: &Settings, watching: bool) -> RoamingConfig {
    RoamingConfig {
        enabled: settings.client_roaming && (watching || settings.roaming_metered),
        metered: settings.roaming_metered,
        platform_hints: watching,
    }
}

/// Over the frozen picture while the stream's network is gone; the second when a reconnect follows.
pub(super) const NETWORK_LOST: &str = en("Network lost.");
pub(super) const NETWORK_LOST_RECONNECTING: &str = en("Network lost. Reconnecting as soon as it is back…");

/// The stream's network watcher toward the PC, feeding the stream through `Notice`s, or None
/// with why on stderr. It runs for every stream: a lost route that comes back where the
/// connection cannot carry on reconnects at once; with client roaming its changes are also the
/// session's hints.
pub(super) fn roam_watch(settings: &Settings, address: &str, shared: &Arc<Shared>) -> Option<crate::network::RoamWatch> {
    let unwatched = if !settings.client_roaming || settings.roaming_metered { "" } else { " and roaming is off" };
    let Some(remote) = (address, 47989).to_socket_addrs().ok().and_then(|mut found| found.next()) else {
        eprintln!("nightjar: roaming: {address} did not resolve, network changes go unwatched{unwatched}");
        return None;
    };
    let shared = Arc::downgrade(shared);
    let on_change = move |event| {
        let notice = match event {
            crate::network::LinkEvent::Lost => Notice::NetworkLost,
            crate::network::LinkEvent::Changed { metered, after_loss } => Notice::NetworkChanged(NetworkHint { metered }, after_loss),
        };
        shared.upgrade().map(|s| s.notify(notice)).is_some()
    };
    crate::network::RoamWatch::start(remote, on_change)
        .map_err(|e| eprintln!("nightjar: roaming: network changes go unwatched{unwatched}: {e}"))
        .ok()
}

/// Keeps the watcher for the stream. The session roams on its changes only when the host roams
/// and client roaming is on, and tells on every stream whether a lost network's return lets the
/// connection carry on.
pub(super) fn keep_roam_watch(shared: &Shared, watch: Option<crate::network::RoamWatch>) {
    *lock(&shared.roam_watch) = watch;
}

/// The setting, unless `NIGHTJAR_EARLY_DECODE` is `1` or `0` (for measurement runs).
pub(super) fn early_decode(setting: bool) -> bool {
    match std::env::var("NIGHTJAR_EARLY_DECODE").as_deref() {
        Ok("1") => true,
        Ok("0") => false,
        _ => setting,
    }
}

/// The plan that rejoins a stream of `role` that `plan` started: a resume of the running app,
/// never the answer to a replace prompt. None when there is no app to resume.
pub(super) fn rejoin(mut plan: LaunchPlan, role: AppRole) -> Option<LaunchPlan> {
    // Only the launch the user answered closes the running app; a reconnect never does.
    plan.confirm_replace = false;
    plan.resume = role == AppRole::Stream;
    if plan.resume && plan.app.control != nightjar_core::http::AppControl::None {
        if plan.info.current_game == 0 { return None; }
        plan.app.id = plan.info.current_game;
        plan.app.uuid = plan.info.current_game_uuid.clone().unwrap_or_default();
        plan.app.control = nightjar_core::http::AppControl::None;
    }
    Some(plan)
}

/// Whether a dropped stream of `role` reconnects on its own: the setting is on and the window is on
/// screen. A window without focus still shows the stream behind another one, so it reconnects; a
/// minimized or hidden one (`away`) waits for the user, as the Android app does in the background.
pub(super) fn reconnects_automatically(auto_reconnect: bool, away: bool, role: AppRole) -> bool {
    auto_reconnect && !away && role == AppRole::Stream
}

/// The plan that reconnects a dropped stream of `role` that `plan` started, on its own, or None
/// when it waits for the user ([`reconnects_automatically`]) or there is nothing to rejoin. Its
/// budget renews after `connected_ms` of a stable stream; `retry_now`: the first attempt starts
/// without its delay (it still spends one). `had_video`: the dropped stream or an earlier one of its
/// chain showed video, which the next connection carries.
pub(super) fn automatic_rejoin(plan: &LaunchPlan, role: AppRole, auto_reconnect: bool, away: bool, connected_ms: u64, retry_now: bool, had_video: bool) -> Option<LaunchPlan> {
    if !reconnects_automatically(auto_reconnect, away, role) { return None; }
    let mut plan = rejoin(plan.clone(), role)?;
    plan.video_shown |= had_video;
    let recovery = plan.recovery.get_or_insert_with(Default::default);
    recovery.reset_after(connected_ms);
    if retry_now { recovery.retry_now(); }
    Some(plan)
}

/// The lost-network notice: it promises a reconnect only when one will follow (`on_its_own`: a
/// dropped stream reconnects by itself, so the connection's timeout or a return it cannot carry
/// on over recovers it), not where a network drop tolerance holds the connection instead.
pub(super) fn lost_notice(settings: &Settings, on_its_own: bool) -> &'static str {
    if on_its_own && settings.network_grace_ms <= recovery::PEER_TIMEOUT_MS { NETWORK_LOST_RECONNECTING } else { NETWORK_LOST }
}

/// A network is back after the route was gone: whether the stream ends now to reconnect at once.
/// Only when the session says its connection cannot carry on (another address, not roamed): it
/// would only time out. Roamed, or the same address back, or no route yet, the stream goes on.
pub(super) fn reconnects_on_return(change: NetworkChange, metered: bool, settings: &Settings, on_its_own: bool) -> bool {
    on_its_own && recovery::reconnects_at_once(change == NetworkChange::CannotContinue, metered,
        settings.roaming_metered, settings.auto_reconnect, settings.network_grace_ms)
}

/// Whether a refused launch of `plan` is the PC's prompt to close the running app, which the
/// user answers. A launch that already confirmed is not asked again, nor is a resume.
pub(super) fn asks_replace(plan: &LaunchPlan, role: AppRole, error: &nightjar_core::http::Error) -> bool {
    !plan.resume && !plan.confirm_replace && role == AppRole::Stream && nightjar_core::http::asks_to_confirm_replace(error)
}

/// Whether this PC's default output plays 7.1.4 (12 channels or more): only Windows, through
/// WASAPI; SDL opens at most 8 channels, so Linux stays at 7.1.
pub fn plays_714() -> bool {
    #[cfg(windows)]
    return wasapi::plays_714();
    #[cfg(not(windows))]
    false
}

/// [`plays_714`] for the audio setting, which asks every frame it is shown.
pub fn offers_714() -> bool {
    #[cfg(windows)]
    return wasapi::offered();
    #[cfg(not(windows))]
    false
}

/// What the stream asks the PC for. 7.1.4 needs a PC that reports it in serverinfo
/// (`AudioLayouts`, which Rubylight sends while it streams 7.1.4) and an output that plays it;
/// otherwise 7.1, as a PC without 7.1.4 may answer 12 channels with no sound at all. Foundation
/// Sunshine isn't told apart from stock Sunshine (Moonlight V+ asks every PC), so it gets 7.1.
pub(super) fn audio_configuration(choice: AudioChannels, host_714: bool, output_714: bool) -> AudioConfiguration {
    match choice {
        AudioChannels::Stereo => AudioConfiguration::STEREO,
        AudioChannels::Surround51 => AudioConfiguration::SURROUND_51,
        AudioChannels::Surround71 => AudioConfiguration::SURROUND_71,
        AudioChannels::Surround714 if host_714 && output_714 => AudioConfiguration::SURROUND_714,
        AudioChannels::Surround714 => {
            let why = if host_714 { "this output doesn't play 12 channels" } else { "the PC doesn't report 7.1.4" };
            eprintln!("nightjar: audio: {why}; asking for 7.1");
            AudioConfiguration::SURROUND_71
        }
    }
}

pub(super) fn connect(plan: &mut LaunchPlan, shared: Arc<Shared>, interrupt: &Interrupt) -> Result<(Session, Facts), ConnectError> {
    let settings = &plan.settings;
    let role = plan.app.role(plan.small_stream_input_only);
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
        gamepad_mask: plan.gamepad_mask,
        confirm_replace: plan.confirm_replace,
        prepared_controllers: if role != AppRole::RemoteMonitor && plan.info.capabilities.has(Feature::ControllerPreparation) {
            plan.prepared_controllers.clone()
        } else { String::new() },
        virtual_display: settings.virtual_display,
        virtual_display_scale: settings.virtual_display_scale,
        virtual_display_dpi: settings.virtual_display_dpi,
        host_display: settings.host_display.into(),
        fec: Some(settings.error_correction.into()),
        small_stream_input_only: plan.small_stream_input_only,
        // The host lists 0x553C in DESCRIBE only when asked.
        rate_change: rate_change::asked_at_launch(role),
        clipboard_sync: settings.clipboard_sync && role != AppRole::RemoteMonitor,
        display_luminance: plan.display_luminance,
        ..Default::default()
    };
    let port = if plan.record.http_port == 0 {
        nightjar_core::http::DEFAULT_HTTP_PORT
    } else {
        plan.record.http_port
    };
    let pem = plan
        .record
        .server_certificate
        .as_deref()
        .ok_or_else(|| tr("This PC is not paired."))?;
    let client = HostClient::new(&plan.record.address, port, plan.identity.clone())
        .and_then(|c| c.with_server_certificate_pem(pem))
        .map_err(|e| e.to_string())?
        .with_https_port(plan.info.https_port);
    let client = Arc::new(client);
    *lock(&shared.host_client) = Some(client.clone());
    if interrupt.is_interrupted() {
        client.cancel_pending_requests();
        return Err("Cancelled.".into());
    }
    if plan.resume {
        plan.info = client.server_info_https().map_err(|e| ConnectError {
            retryable: recovery::transient_http(&e), message: format!("Checking the running app failed: {e}"), asks_replace: false,
        })?;
        crate::hosts::merge_native(&mut plan.info, plan.native.as_ref());
        if !plan.info.paired || !plan.app.matches_running_app(plan.info.current_game, plan.info.current_game_uuid.as_deref()) {
            return Err("The original app is no longer running on the PC. Choose an app to start a new stream.".into());
        }
    }
    let scm = plan
        .info
        .capabilities
        .trusted_codec_mode_support(plan.info.server_codec_mode_support);
    let offer = offered_formats(settings, scm, &plan.color_decodable);
    request.ten_bit_sdr = launches_ten_bit_sdr(settings, &offer);
    // The interface the system sends this host's traffic over decides, not the internet
    // connection; the connection is the fallback when that lookup tells nothing. Looked up before
    // the launch: PyroWave's default bitrate stays within this link.
    let route = crate::network::route(&plan.record.address);
    let toward = route.toward;
    let network = plan.network.toward(toward);
    let family = plan.info.capabilities.family();
    let client_wireless = tells_host_wireless(settings.tell_host_wireless, family, network.wireless());
    let routed_link_mbps = network.wired_link_mbps();
    if network.wireless() && !client_wireless {
        let why = if settings.tell_host_wireless { "the PC is not a Rubylight host" } else { "it is turned off in the settings" };
        eprintln!("nightjar: not telling the PC the route is wireless: {why}");
    }
    eprintln!(
        "nightjar: route to the PC: {} ({}), link {routed_link_mbps} Mbps",
        if network.wireless() { "wireless" } else { "wired or unknown" },
        match toward {
            Ok(_) => "from the route".to_string(),
            Err(why) => format!("from the internet connection, {why}"),
        },
    );
    let measured_pace = plan.pace.as_ref().map_or(0, |pace| pace.for_network(&network, plan.host_link_mbps));
    let LinkHints { link_mbps, sent_link_mbps, pace_mbps } = link_hints(
        settings.tell_host_link_speed,
        routed_link_mbps,
        hint_env("NIGHTJAR_PYROWAVE_LINK_MBPS"),
        measured_pace,
        hint_env("NIGHTJAR_PYROWAVE_PACE_MBPS"),
    );
    if link_mbps > 0 && sent_link_mbps == 0 {
        eprintln!("nightjar: not telling the PC the link speed ({link_mbps} Mbps): it is turned off in the settings");
    }
    let pyrowave = offers_pyrowave(offer.formats, scm);
    let bitrate = capped_bitrate(start_bitrate(settings, width, height, fps, pyrowave, link_mbps), plan.bitrate_limit_kbps);
    if pyrowave && settings.bitrate_kbps == 0 {
        eprintln!("nightjar: PyroWave: starting at {bitrate} kbps, its default for {width}x{height} at {fps} fps and a {link_mbps} Mbps link");
    }
    let bitrate = if probes_first_bitrate(settings, plan.resume, &plan.info) {
        match client.probe_link(&plan.info) {
            Ok(mbps) => {
                let first = nightjar_core::http::probe_start_kbps(bitrate, mbps);
                eprintln!("nightjar: bandwidth test {mbps:.1} Mbit/s, starting at {first} kbps");
                first
            }
            // The stream still starts, at the automatic bitrate, and says why it was not tested.
            Err(e) => {
                eprintln!("nightjar: bandwidth test failed, starting at {bitrate} kbps: {e}");
                bitrate
            }
        }
    } else { bitrate };
    if interrupt.is_interrupted() {
        client.cancel_pending_requests();
        return Err("Cancelled.".into());
    }
    let launched = if plan.resume {
        client.resume(&request, &plan.info)
    } else {
        client.launch(&request, &plan.info)
    };
    // A host that takes one stream at a time refuses the small stream while another one runs.
    let refused = |why: String| -> ConnectError {
        if plan.small_stream_input_only {
            format!("{why} {}", tr("Input-only on this PC uses a small ordinary stream, and the host refused it. Some hosts allow only one stream at a time: end the other stream or allow more in the host’s settings.")).into()
        } else { why.into() }
    };
    let launched = match launched {
        // The answer goes out with a new launch that says it confirms.
        Err(e) if asks_replace(plan, role, &e) => {
            return Err(ConnectError { message: e.to_string(), retryable: false, asks_replace: true })
        }
        Ok(Some(launched)) => launched,
        Ok(None) => return Err(refused("The PC declined to start the app.".into())),
        Err(nightjar_core::http::Error::Status { code, message }) => {
            return Err(refused(format!("The PC could not start the app ({code}: {message}).")))
        }
        Err(e) => return Err(ConnectError { retryable: recovery::transient_http(&e), message: format!("Starting the app failed: {e}"), asks_replace: false }),
    };
    if interrupt.is_interrupted() {
        return Err("Cancelled.".into());
    }
    let beam = beam_race(settings.beam_race, link_mbps, plan.gpu_decoder.is_some());
    match beam {
        BeamRace::NotWired => eprintln!("{BEAM_RACE_NOT_WIRED}"),
        BeamRace::NoGpuDecoder => eprintln!("{BEAM_RACE_NO_GPU}"),
        BeamRace::On => eprintln!("nightjar: beam race is on (experimental): asking the host for the row order"),
        BeamRace::Off => {}
    }
    let race_ask = race_request(beam, early_decode(settings.early_decode), height);
    let (location, packet_size) = location_for_route(location_override(), route.vpn);
    if route.vpn && location == Location::Remote {
        eprintln!("nightjar: the route to the PC is a VPN: streaming as remote with {packet_size} byte packets");
    }
    let mut iv = [0u8; 16];
    iv[..4].copy_from_slice(&ri_key_id.to_be_bytes());
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
        color_space: if settings.hdr { COLORSPACE_REC_2020 } else { COLORSPACE_REC_709 },
        color_range: if settings.full_range { COLOR_RANGE_FULL } else { COLOR_RANGE_LIMITED },
        microphone: settings.microphone && role != AppRole::RemoteMonitor,
        audio_passthrough: pipeline::passthrough_codecs(settings.audio_passthrough),
        haptics_pcm: settings.controllers && settings.controller_audio && role != AppRole::RemoteMonitor,
        pyrowave_link_mbps: sent_link_mbps as i32,
        pyrowave_pace_mbps: pace_mbps as i32,
        client_wireless,
        pyrowave_records_supported: plan.info.capabilities.has(Feature::PyroWaveRecords),
        pyrowave_row_order: race_ask.row_order,
        encryption_flags: ENCFLG_ALL,
        remote_input_aes_key: ri_key,
        remote_input_aes_iv: iv,
        video_capabilities: 0,
        audio_capabilities: CAPABILITY_DIRECT_SUBMIT,
        video_fec: settings.error_correction.into(),
        hdr_format: asked_hdr_format(hdr_format(
            settings.hdr,
            plan.info.capabilities.evidence().map_or(-1, |evidence| evidence.hdr_formats),
            offer.formats,
            settings.experimental_packed_444,
            reads_dolby_vision(),
        )),
        ..Default::default()
    };
    let host = HostInfo {
        address: plan.record.address.clone(),
        app_version: plan.info.app_version.clone().unwrap_or_else(|| "7.1.431.-1".into()),
        rtsp_session_url: launched.rtsp_session_url,
        server_codec_mode_support: if scm != 0 { scm } else { SCM_H264 },
    };
    // The slices are decoded from chunks, so the beam race needs early decode.
    let (early, slices) = (race_ask.early_decode, race_ask.slices);
    // Started before the handshake so a change during it still reaches the session.
    let watch = roam_watch(settings, &plan.record.address, &shared);
    let roam_shared = shared.clone();
    // Asked beside the handshake: the stream controls offer USB devices only when the PC does.
    let usb_ask = (settings.usb_forwarding && plan.info.capabilities.has(Feature::NativeApi)).then(|| {
        let client = client.clone();
        std::thread::spawn(move || client.native_capabilities_with_files().map_err(|e| e.to_string()))
    });
    let passthrough = stream.audio_passthrough;
    let session = Session::start(
        &host,
        session_config(settings, role, stream, location, watch.is_some() && settings.client_roaming),
        Box::new(pipeline::Decoder::new(shared.clone(), plan.hardware).with_early_decode(early).with_beam_race(slices).with_gpu_decoder(plan.gpu_decoder).with_dmabuf(plan.dmabuf)),
        Box::new(pipeline::Audio::new(shared.clone(), passthrough)),
        Arc::new(pipeline::Events::new(shared)),
        interrupt,
    )
    .map_err(|e| ConnectError { retryable: recovery::transient_start(e.stage, e.code), message: format!("Could not start the stream: {e}"), asks_replace: false })?;
    let outcome = session.outcome();
    keep_roam_watch(&roam_shared, watch);
    let mut facts = Facts {
        role,
        width: outcome.config.width,
        height: outcome.config.height,
        fps: outcome.config.fps,
        bitrate_kbps: outcome.config.bitrate_kbps,
        format: outcome.negotiated_video_format,
        host_timing: outcome.host_control_messages.contains(&HOST_TIMING_MESSAGE_TYPE),
        link_notice: outcome.host_control_messages.contains(&LINK_NOTICE_MESSAGE_TYPE),
        pipeline_rates: outcome.host_control_messages.contains(&PIPELINE_RATES_MESSAGE_TYPE),
        clock_sync: outcome.host_control_messages.contains(&CLOCK_SYNC_MESSAGE_TYPE),
        display_caps: outcome.host_control_messages.contains(&rubylight_protocol::control::DISPLAY_CAPS_MESSAGE_TYPE),
        cursor_shapes: plan.info.capabilities.has(Feature::CursorShape) && outcome.host_control_messages.contains(&0x5540),
        text_input: plan.info.capabilities.has(Feature::TextInput)
            && outcome.host_control_messages.contains(&rubylight_protocol::text_input::TEXT_INPUT_MESSAGE_TYPE)
            && role != AppRole::RemoteMonitor,
        keyboard_layout: plan.info.capabilities.has(Feature::ClientKeyboardLayout)
            && outcome.host_control_messages.contains(&rubylight_protocol::keyboard_layout::KEYBOARD_LAYOUT_MESSAGE_TYPE)
            && role != AppRole::RemoteMonitor,
        clipboard: plan.info.capabilities.has(Feature::Clipboard) && role != AppRole::RemoteMonitor,
        clipboard_sync: settings.clipboard_sync
            && outcome.host_control_messages.contains(&rubylight_protocol::clipboard_sync::CLIPBOARD_SYNC_MESSAGE_TYPE)
            && role != AppRole::RemoteMonitor,
        image_clipboard: plan.info.capabilities.has(Feature::ImageClipboard) && role != AppRole::RemoteMonitor,
        server_commands: if plan.info.capabilities.has(Feature::ServerCommands) && role != AppRole::RemoteMonitor {
            plan.info.server_commands.iter().take(256).cloned().collect()
        } else { Vec::new() },
        runtime_bitrate: plan.info.capabilities.has(Feature::RuntimeBitrate) && role != AppRole::InputOnly,
        reconfigure: outcome.host_control_messages.contains(&RECONFIGURE_MESSAGE_TYPE)
            && !outcome.config.experimental_packed_444 && outcome.negotiated_video_format & VIDEO_FORMAT_MASK_PYROWAVE == 0 && role == AppRole::Stream,
        rate_change: rate_change::offered(&outcome.host_control_messages, role),
        usb: if settings.usb_forwarding {
            let native = usb_ask.map(|ask| ask.join().unwrap_or_else(|_| Err("the request panicked".into())));
            crate::usb::availability(&plan.info.capabilities, native)
        } else { Availability::Unknown },
        touch: outcome.sunshine_feature_flags & nightjar_core::control::negotiation::feature::PEN_TOUCH_EVENTS != 0
            && role != AppRole::RemoteMonitor,
        phase_lock: outcome.host_control_messages.contains(&rubylight_protocol::phase_lock::REPORT_MESSAGE_TYPE),
        early_decode: early_state(early, outcome.negotiated_video_format & VIDEO_FORMAT_MASK_PYROWAVE != 0,
            outcome.pyrowave_records_enabled) == EarlyState::On,
        early_without_records: early_state(early, outcome.negotiated_video_format & VIDEO_FORMAT_MASK_PYROWAVE != 0,
            outcome.pyrowave_records_enabled) == EarlyState::NoRecords,
        beam_race_off: beam.reason(),
        race_slices: slices,
        pace_mbps,
        beam_race: race_runs(slices, early_state(early, outcome.negotiated_video_format & VIDEO_FORMAT_MASK_PYROWAVE != 0,
            outcome.pyrowave_records_enabled)),
        color_notice: color_notice(&offer, outcome.negotiated_video_format),
        pyrowave_fallback: None,
        app_name: plan.app.name.clone(),
        host_name: plan.host_name.clone(),
    };
    let reported = plan.info.server_codec_mode_support;
    if let Some((log, notice)) = pyrowave_fallback(settings.codec, (scm, reported), offer.formats, outcome.negotiated_video_format) {
        eprintln!("{log}");
        // Shown once per PC (`pyrowave_notice`); the log says it on every stream.
        facts.pyrowave_fallback = Some(notice);
    }
    if facts.early_without_records {
        eprintln!("{EARLY_NO_RECORDS}");
    }
    if settings.prefer_yuv444 || settings.sdr_10bit {
        eprintln!("nightjar: 4:4:4 {:?}, 10-bit SDR {:?}, offered {:#x}, streaming {:#x}", offer.yuv444, offer.sdr10, offer.formats, facts.format);
    }
    // A pen's barrel roll goes only to a host with both the serverinfo element and the
    // DESCRIBE attribute; other Sunshine-family hosts may send the attribute alone.
    let pen_barrel_roll = plan.info.capabilities.has(Feature::PenBarrelRoll) && session.outcome().host_pen_barrel_roll;
    session.input(|q| q.set_pen_barrel_roll(pen_barrel_roll));
    Ok((session, facts))
}
