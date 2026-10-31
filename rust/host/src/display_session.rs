//! Prepare display and game integrations before running application commands.
//! The same leases move from an authorized pending launch into its transport.
use crate::state::{Launch, Shared};
use anyhow::{Context, Result};
use butterpollo_core::{
    config::Config,
    display_policy::HostDisplay,
    framegen::{Policy, Rate},
    rtsp::Negotiated,
    session::{Preparation, Role, Warnings},
};
use butterpollo_windows::{
    display::{Guard, Retained},
    display_arrangement, hdr_profile, limiter, vulkan,
};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::Duration;

fn stream_mode(stream: &Negotiated) -> (u32, u32, u32, bool, bool) {
    (
        stream.width,
        stream.height,
        stream.fps_millihz(),
        stream.hdr,
        stream.vrr_low_latency,
    )
}

fn virtual_display_mode<'a>(
    config: &'a Config,
    client: &'a butterpollo_core::state::Client,
    app: Option<&'a butterpollo_core::state::App>,
    windows_11: bool,
) -> &'a str {
    if butterpollo_core::background::enabled(config) {
        return "per_client";
    }
    client
        .extra
        .get("virtual_display_mode")
        .and_then(serde_json::Value::as_str)
        // The old console stored "global" for the host setting.
        .filter(|s| !s.is_empty() && *s != "global")
        .or_else(|| {
            app.and_then(|app| app.extra.get("virtual-display-mode"))
                .and_then(serde_json::Value::as_str)
                .filter(|s| !s.is_empty())
        })
        .unwrap_or(config.virtual_display_mode(windows_11))
}

fn virtual_display_identity(
    config: &Config,
    launch: &Launch,
    app: Option<&butterpollo_core::state::App>,
) -> String {
    if !butterpollo_core::background::enabled(config)
        && app.is_some_and(|a| crate::process::app_bool(a, "use-app-identity", false))
    {
        let app_id = app
            .and_then(|a| a.extra.get("uuid"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("application");
        if app.is_some_and(|a| crate::process::app_bool(a, "per-client-app-identity", false)) {
            butterpollo_core::display_policy::app_client_identity(
                app_id,
                launch
                    .options
                    .get("uniqueid")
                    .filter(|value| uuid::Uuid::parse_str(value).is_ok())
                    .unwrap_or(&launch.client.uuid),
            )
        } else {
            app_id.to_owned()
        }
    } else {
        launch.client.uuid.clone()
    }
}
pub(crate) fn virtual_display_layout<'a>(
    config: &'a Config,
    client: &'a butterpollo_core::state::Client,
    app: Option<&'a butterpollo_core::state::App>,
    requested: Option<HostDisplay>,
) -> &'a str {
    // A background stream must not move the PC's windows or let its mouse
    // wander onto the device's display, whatever the device asks for.
    if butterpollo_core::background::enabled(config) {
        return butterpollo_core::background::LAYOUT;
    }
    if let Some(HostDisplay::Virtual(arrangement)) = requested {
        return arrangement.name();
    }
    let inherited = inherited_layout(config, app);
    client
        .extra
        .get("virtual_display_layout")
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty() && *s != "global")
        // A layout learned at the end of a stream stands in for the app or host
        // layout it replaced, until that layout changes (issue #40).
        .filter(|_| {
            client
                .extra
                .get(LEARNED_OVER)
                .and_then(serde_json::Value::as_str)
                .is_none_or(|over| over == inherited)
        })
        .unwrap_or(inherited)
}

/// The device setting naming the app or host layout a learned device layout replaced.
pub(crate) const LEARNED_OVER: &str = "virtual_display_layout_learned_over";
/// Set in the device settings when the user picked the device's layout in the
/// console. Only it protects a layout from being learned over: a layout saved
/// before it existed, or by a stream, stays learnable.
pub(crate) const SET_BY_USER: &str = "virtual_display_layout_set_by_user";
/// Whether the device's own virtual display layout is one the user set rather
/// than one learned from a stream.
fn has_user_layout(client: &butterpollo_core::state::Client) -> bool {
    client
        .extra
        .get("virtual_display_layout")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|s| !s.is_empty() && s != "global")
        && client
            .extra
            .get(SET_BY_USER)
            .and_then(serde_json::Value::as_bool)
            == Some(true)
}
/// The resolution a device with `remember_display_mode` keeps for its virtual
/// display, `WIDTHxHEIGHT`, from the last stream it was changed in Windows.
pub(crate) const REMEMBERED_RESOLUTION: &str = "remembered_resolution";
/// The Windows scale, in percent, a device with `remember_display_mode` keeps
/// for its virtual display, from the last stream it was changed in Windows.
pub(crate) const REMEMBERED_SCALE: &str = "remembered_scale";

/// The virtual display layout without the device's own: the app's, then the host's.
fn inherited_layout<'a>(
    config: &'a Config,
    app: Option<&'a butterpollo_core::state::App>,
) -> &'a str {
    app.and_then(|app| app.extra.get("virtual-display-layout"))
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty() && *s != "global")
        .or_else(|| {
            app.filter(|app| crate::process::app_bool(app, "virtual-display-primary", false))
                .map(|_| "extended_primary")
        })
        .unwrap_or(config.get("virtual_display_layout", "exclusive"))
}

/// What the client asked the PC's displays to do for this stream (the
/// `hostDisplay` launch parameter), or None to follow the host, app and
/// per-device settings. Only a stream chooses; a remote monitor keeps the
/// remote monitor layout, and Remote Input has no display.
pub fn client_display(launch: &Launch) -> Option<HostDisplay> {
    if launch.role != Role::Stream {
        return None;
    }
    let value = launch.options.get(HostDisplay::PARAMETER);
    let choice = value.and_then(|value| HostDisplay::parse(value));
    if let Some(value) = value
        && choice.is_none()
        && !value.trim().is_empty()
        && value.trim() != "default"
    {
        tracing::info!(value = %value, "unknown hostDisplay launch value; the display settings decide");
    }
    // `virtualDisplay=0` is not a choice: Apollo-compatible clients send it
    // whenever their own virtual display toggle is off, and Apollo reads it as
    // "not forced". Taking it as "physical" (2.5.6 to 2.5.9) overrode the host's
    // virtual display for those clients, which then mirrored the PC's monitor.
    choice
}

/// Resolution, refresh in millihertz and HDR a stream asked its display for.
type RequestedMode = (Option<(u32, u32)>, Option<u32>, Option<bool>);
/// Resolution, refresh in millihertz and HDR a display shows.
type AppliedMode = (u32, u32, u32, bool);
/// The resolution a stream set on its virtual display, and the one Windows shows.
type SizeChange = ((u32, u32), (u32, u32));

fn applied_mode(output: &str) -> Result<AppliedMode> {
    let topology = butterpollo_windows::display::Topology::query()?;
    let monitor = topology
        .monitors()
        .into_iter()
        .find(|m| m.matches(output))
        .context("selected display missing after preparation")?;
    let mode = butterpollo_windows::display::mode(output)?;
    let refresh = topology.refresh(&monitor.device_id)?;
    Ok((
        mode.dmPelsWidth,
        mode.dmPelsHeight,
        refresh.0,
        monitor.hdr_enabled,
    ))
}

/// What the display shows compared with the stream's request, on the card of
/// the launch that uses the display now. A retained game display moves to
/// the next launch, which sees the last finding at once.
#[derive(Clone)]
struct ModeReport(Arc<Mutex<ModeReportState>>);
struct ModeReportState {
    warnings: Arc<Warnings>,
    requested: RequestedMode,
    stream_rate: u32,
    last: Option<std::result::Result<AppliedMode, String>>,
    /// Why the display could not follow the stream's last turn, until a turn follows.
    unfollowed: Option<String>,
    /// A shared virtual display: another stream on it may have raised its
    /// refresh above the requested one, which serves this stream too.
    shared: bool,
}
impl ModeReportState {
    fn report(&self) {
        match &self.last {
            Some(Ok(actual)) => {
                self.warnings.clear("display_verify");
                let mut requested = self.requested;
                if self.shared
                    && requested
                        .1
                        .is_some_and(|rate| actual.2.saturating_add(500) >= rate)
                {
                    requested.1 = None;
                }
                butterpollo_core::display_policy::report_mode(
                    &self.warnings,
                    requested,
                    *actual,
                    self.stream_rate,
                );
            }
            Some(Err(error)) => self.warnings.set("display_verify", format!("Could not verify the applied display mode ({error}); refresh and HDR may differ from the request. Check Windows display settings and reconnect.")),
            None => {}
        }
        match &self.unfollowed {
            Some(error) => self.warnings.set("display_follow", format!("The display could not turn with the stream ({error}); the picture is scaled to fit instead. Check Windows display settings or turn the device back.")),
            None => self.warnings.clear("display_follow"),
        }
    }
}
impl ModeReport {
    fn new(warnings: Arc<Warnings>, requested: RequestedMode, stream_rate: u32) -> Self {
        Self(Arc::new(Mutex::new(ModeReportState {
            warnings,
            requested,
            stream_rate,
            last: None,
            unfollowed: None,
            shared: false,
        })))
    }
    /// The display is a shared virtual display: only a refresh below the
    /// requested one is reported.
    fn shared(self) -> Self {
        self.0.lock().unwrap().shared = true;
        self
    }
    fn publish(&self, actual: Result<AppliedMode>) {
        let mut state = self.0.lock().unwrap();
        state.last = Some(actual.map_err(|error| format!("{error:#}")));
        state.report();
    }
    fn attach(&self, warnings: Arc<Warnings>) {
        let mut state = self.0.lock().unwrap();
        state.warnings = warnings;
        state.report();
    }
    /// The stream asks for another rate than the launch the display was
    /// prepared for.
    fn retime(&self, requested: RequestedMode, stream_rate: u32) {
        let mut state = self.0.lock().unwrap();
        state.requested = requested;
        state.stream_rate = stream_rate;
    }
    /// Whether the display followed the stream's last turn.
    fn followed(&self, result: &Result<()>) {
        let mut state = self.0.lock().unwrap();
        state.unfollowed = result.as_ref().err().map(|error| format!("{error:#}"));
        state.report();
    }
    /// The display now runs at a resolution the user chose, which the stream keeps.
    fn adopt(&self, resolution: (u32, u32)) {
        self.0.lock().unwrap().requested.0 = Some(resolution);
    }
}

/// A display recovery's steps after the heartbeat brought the display back.
/// The layout and HDR profile are retried each second until they apply. The
/// stream's mode is set once per recovery, after the layout: a mode Windows
/// refuses is reported on the stream card, not retried against Windows.
struct Recovery {
    pending: bool,
    due: std::time::Instant,
    mode: bool,
}
impl Recovery {
    fn new() -> Self {
        Self {
            pending: false,
            due: std::time::Instant::now(),
            mode: false,
        }
    }
    fn start(&mut self, now: std::time::Instant) {
        self.pending = true;
        self.due = now;
        self.mode = true;
    }
    /// Whether to run the steps now; a failed attempt waits a second.
    fn due(&mut self, now: std::time::Instant) -> bool {
        if self.pending && now >= self.due {
            self.due = now + Duration::from_secs(1);
            return true;
        }
        false
    }
    fn take_mode(&mut self) -> bool {
        std::mem::take(&mut self.mode)
    }
    fn finish(&mut self) {
        self.pending = false;
    }
}

/// Only pending and connected streams own limiter changes. The retained game
/// display must not keep a global frame cap active after transport disconnects.
pub struct StreamPreparation {
    pub display: Arc<Ready>,
    _limiter: limiter::Lease,
}
impl Preparation<StreamPreparation> for StreamPreparation {
    fn prepared(&self) -> &Self {
        self
    }
    fn into_prepared(self: Box<Self>) -> Self {
        *self
    }
}
impl StreamPreparation {
    pub fn report_limiter(
        &self,
        warnings: &butterpollo_core::session::Warnings,
        config: &butterpollo_core::config::Config,
    ) {
        let explicit = config.boolean("frame_limiter_enable", false);
        let framegen = self.display.framegen.lock().unwrap().clone();
        if let Some(message) = self._limiter.warning(&framegen, explicit) {
            warnings.set("display_limiter", message);
        } else {
            warnings.clear("display_limiter");
        }
    }
}
impl Drop for StreamPreparation {
    fn drop(&mut self) {
        {
            // Capture the choice at the last transport's disconnect, before
            // a paused lease can observe unrelated local Win+P changes. The
            // state lock also serializes this boundary with resume's reapply.
            let mut prepared = self.display._state.lock().unwrap();
            if self.display.active.fetch_sub(1, Ordering::AcqRel) == 1 {
                prepared.keep_chosen_layout();
                prepared.keep_chosen_resolution();
                prepared.keep_chosen_scale();
            }
        }
        if let Some(retained) = &self.display.retained {
            retained.stream_stopped();
        }
    }
}

/// A game may keep its display across transport disconnects. The heartbeat
/// owns the resources, not the Ready Arc, so final teardown cannot form a cycle.
pub struct Ready {
    // Retain the leases until after the worker has joined. Streaming readers
    // use the published target, never the lock held over native display I/O.
    _state: Arc<Mutex<Prepared>>,
    target: CaptureTarget,
    mode: (u32, u32, u32, bool, bool),
    client_display: Option<HostDisplay>,
    virtual_layout: Option<String>,
    exclusive_layout: bool,
    /// One game window's stream (`Ready::window`): never kept for a
    /// reconnect, whose launch prepares and checks the window again.
    game_window: bool,
    /// The launch's policy, or the stream request's once `retime` raised it.
    framegen: Mutex<Policy>,
    mode_report: ModeReport,
    /// The size a stream turned to, for the heartbeat to give its display.
    turned: Arc<Mutex<Option<(u32, u32)>>>,
    stop: Arc<AtomicBool>,
    active: Arc<AtomicUsize>,
    retained: Option<Arc<Retained>>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl Ready {
    pub fn prepare((prepared, limiter): (Prepared, limiter::Lease)) -> Result<StreamPreparation> {
        let target = CaptureTarget::new(prepared.capture_target());
        let mode = prepared.mode;
        let client_display = prepared.client_display;
        let virtual_layout = prepared.virtual_layout.clone();
        let exclusive_layout = prepared
            ._arrangement
            .as_ref()
            .is_some_and(display_arrangement::Lease::exclusive);
        let framegen = prepared.framegen.clone();
        let mode_report = prepared.mode_report.clone();
        let retained = prepared._retained.clone();
        let state = Arc::new(Mutex::new(prepared));
        let stop = Arc::new(AtomicBool::new(false));
        let worker_state = state.clone();
        let worker_target = target.clone();
        let worker_stop = stop.clone();
        let turned = Arc::new(Mutex::new(None));
        let worker_turned = turned.clone();
        let active = Arc::new(AtomicUsize::new(1));
        let worker_active = active.clone();
        let worker = std::thread::Builder::new()
            .name("game-display-lease".into())
            .spawn(move || {
                while !worker_stop.load(Ordering::Acquire) {
                    // Windows refuses display configuration from the normal
                    // desktop while it is locked (issue #6).
                    butterpollo_windows::input::keep_on_input_desktop();
                    let mut prepared = worker_state.lock().unwrap();
                    let active = worker_active.load(Ordering::Acquire) != 0;
                    let result = prepared.feed(active);
                    let taken = worker_turned.lock().unwrap().take();
                    if let Some(to) = taken {
                        // The card says so; the colour conversion scales the picture meanwhile.
                        let followed = prepared.follow_shape(to);
                        prepared.mode_report.followed(&followed);
                    }
                    let tick = if active || result.is_err() || prepared.recovery.pending {
                        Duration::from_millis(100)
                    } else {
                        Duration::from_millis(250)
                    };
                    let current = prepared.capture_target();
                    // Publish a recreated target even when a later restoration
                    // step needs a retry. Serialize publication with resume so
                    // an older heartbeat cannot overwrite its new identity.
                    worker_target.publish(current);
                    drop(prepared);
                    if let Err(error) = result {
                        tracing::warn!(%error, "game display heartbeat failed");
                    }
                    std::thread::park_timeout(tick);
                }
            })?;
        if let Some(retained) = &retained {
            retained.stream_started();
        }
        let display = Arc::new(Self {
            _state: state,
            target,
            mode,
            client_display,
            virtual_layout,
            exclusive_layout,
            game_window: false,
            framegen: Mutex::new(framegen),
            mode_report,
            turned,
            stop,
            active,
            retained,
            worker: Some(worker),
        });
        Ok(StreamPreparation {
            display,
            _limiter: limiter,
        })
    }
    /// A stream of one game window, a Nucleus Co-op player's: no display is
    /// prepared, changed or kept, and no frame limit is set, which would
    /// hold back the PC user's games too. `follow` gives the window's
    /// capture target every 100 ms and the capture reopens on a new one. An
    /// error keeps the last target, whose capture then fails loudly, and
    /// says why (`capture_window`); it never becomes a display.
    pub fn window(
        h: &Shared,
        stream: &Negotiated,
        config: &Config,
        first: String,
        mut follow: Box<dyn FnMut() -> Result<String> + Send>,
        warnings: Arc<Warnings>,
    ) -> Result<StreamPreparation> {
        let mut framegen = Policy::resolve(
            config,
            Rate(stream.fps_millihz()),
            false,
            "none",
            false,
            false,
        )?;
        // Only WGC captures a single window.
        framegen.capture = "wgc".into();
        framegen.enabled = false;
        let limiter = limiter::Lease::acquire(&h.directory, config, &framegen)?;
        let target = CaptureTarget::new((first.clone(), 0));
        let mode_report =
            ModeReport::new(warnings.clone(), (None, None, None), stream.fps_millihz());
        let prepared = Prepared::bare(
            h,
            first.clone(),
            framegen.clone(),
            stream_mode(stream),
            mode_report.clone(),
        );
        let stop = Arc::new(AtomicBool::new(false));
        let worker = std::thread::Builder::new()
            .name("game-window".into())
            .spawn({
                let (target, stop) = (target.clone(), stop.clone());
                move || {
                    let mut last = first;
                    while !stop.load(Ordering::Acquire) {
                        match follow() {
                            Ok(next) => {
                                if next != last {
                                    tracing::info!(from = %last, to = %next, "the stream's game window was replaced");
                                    target.publish((next.clone(), 0));
                                    last = next;
                                }
                                warnings.clear("capture_window");
                            }
                            Err(error) => warnings.set(
                                "capture_window",
                                format!("{error:#}; the stream waits for it to return"),
                            ),
                        }
                        std::thread::park_timeout(Duration::from_millis(100));
                    }
                }
            })?;
        let display = Arc::new(Self {
            _state: Arc::new(Mutex::new(prepared)),
            target,
            mode: stream_mode(stream),
            client_display: None,
            virtual_layout: None,
            exclusive_layout: false,
            game_window: true,
            framegen: Mutex::new(framegen),
            mode_report,
            turned: Arc::new(Mutex::new(None)),
            stop,
            active: Arc::new(AtomicUsize::new(1)),
            retained: None,
            worker: Some(worker),
        });
        Ok(StreamPreparation {
            display,
            _limiter: limiter,
        })
    }
    pub fn resume(
        self: Arc<Self>,
        directory: &std::path::Path,
        config: &Config,
        warnings: Arc<Warnings>,
    ) -> Result<StreamPreparation> {
        let framegen = self.framegen.lock().unwrap().clone();
        let limiter = limiter::Lease::acquire(directory, config, &framegen)?;
        butterpollo_windows::input::on_input_desktop(|| -> Result<()> {
            let mut prepared = self._state.lock().unwrap();
            // A paused stream leaves local display changes alone. Resuming
            // must reactivate its target and restore the layout before capture.
            let fed = prepared.display.as_mut().map_or(Ok(false), Guard::resume);
            prepared.finish_feed(fed)?;
            if let Some(arrangement) = &prepared._arrangement {
                arrangement.reapply(&prepared.output, &prepared.remote_monitors())?;
                if let Some(display) = &prepared.display {
                    display.apply_virtual_mode("after resume layout");
                }
                self.mode_report.publish(applied_mode(&prepared.output));
            }
            prepared.put_scale_back()?;
            self.target.publish(prepared.capture_target());
            self.active.fetch_add(1, Ordering::AcqRel);
            Ok(())
        })?;
        self.mode_report.attach(warnings);
        if let Some(retained) = &self.retained {
            retained.stream_started();
        }
        if let Some(worker) = &self.worker {
            worker.thread().unpark();
        }
        Ok(StreamPreparation {
            display: self,
            _limiter: limiter,
        })
    }
    /// Whether the device's next stream may reuse this one. A game window's
    /// is not kept: resuming would skip the window's check, and its warnings
    /// would keep going to the first session.
    pub fn kept_for_reconnect(&self) -> bool {
        !self.game_window
    }
    pub fn matches(&self, stream: &Negotiated) -> bool {
        self.mode == stream_mode(stream)
    }
    /// A retained display must not hide settings changed while it was paused.
    pub fn serves(&self, requested: Option<HostDisplay>, layout: &str) -> bool {
        HostDisplay::reusable(self.client_display, requested)
            && self
                .virtual_layout
                .as_deref()
                .is_none_or(|old| old == layout)
    }
    pub fn output(&self) -> String {
        self.target.current().0
    }
    /// The arrangement actually shared with other streams, which may differ
    /// from this client's requested layout.
    pub fn exclusive_layout(&self) -> bool {
        self.exclusive_layout
    }
    pub fn capture_target(&self) -> (String, u64) {
        self.target.current()
    }
    pub fn capture(&self) -> String {
        self.framegen.lock().unwrap().capture.clone()
    }
    /// The stream turned between portrait and landscape (0x5532): its display
    /// follows on the heartbeat, off the stream loop, when the device's
    /// `dd_rotation_option` is auto.
    pub fn follow_shape(&self, size: (u32, u32)) {
        *self.turned.lock().unwrap() = Some(size);
        if let Some(worker) = &self.worker {
            worker.thread().unpark();
        }
    }
}

/// Only copies and publication hold this lock. Lease renewal, topology queries
/// and restoration must finish before publishing a new capture identity.
#[derive(Clone)]
struct CaptureTarget(Arc<Mutex<(String, u64)>>);
impl CaptureTarget {
    fn new(target: (String, u64)) -> Self {
        Self(Arc::new(Mutex::new(target)))
    }
    fn current(&self) -> (String, u64) {
        self.0.lock().unwrap().clone()
    }
    fn publish(&self, target: (String, u64)) {
        *self.0.lock().unwrap() = target;
    }
}
impl Drop for Ready {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker.thread().unpark();
            let _ = worker.join();
        }
        // Prepared drops the final monitor lease on the input desktop.
        self.retained.take();
    }
}

pub struct Prepared {
    pub display: Option<Guard>,
    pub output: String,
    pub framegen: Policy,
    // Restore topology while the selected monitor still exists, then remove it.
    _arrangement: Option<display_arrangement::Lease>,
    _activation: Option<display_arrangement::Activation>,
    _profile: Option<hdr_profile::Lease>,
    _retained: Option<Arc<Retained>>,
    _vulkan: Option<vulkan::Lease>,
    _golden: Option<GoldenLease>,
    mode: (u32, u32, u32, bool, bool),
    client_display: Option<HostDisplay>,
    virtual_layout: Option<String>,
    /// The device whose stream this is, when it keeps a layout the user chose
    /// during the stream: a stream on its own virtual display whose client did
    /// not choose the layout itself.
    layout_owner: Option<String>,
    /// The app or host layout a layout learned for the device replaces.
    inherited_layout: Option<String>,
    /// The device that keeps the resolution the user chose during the stream
    /// (`remember_display_mode`): a stream on its own virtual display without
    /// a display mode of its own.
    resolution_owner: Option<String>,
    /// The scale the display showed once the stream set up, when the device keeps the scale
    /// the user picks (`remember_display_mode` and no configured scale).
    scale_baseline: Option<u32>,
    /// How the stream's own display follows a stream that turns, when
    /// `dd_rotation_option` is auto.
    follow: Option<Follow>,
    /// A topology fixture keeps lifecycle tests independent of the real desktop.
    #[cfg(test)]
    layout_snapshot: Option<Vec<(String, bool)>>,
    /// The resolution the stream set and the one shown, for lifecycle tests.
    #[cfg(test)]
    size_snapshot: Option<SizeChange>,
    /// The scale the display shows, for lifecycle tests.
    #[cfg(test)]
    scale_snapshot: Option<u32>,
    /// The scales put back on the display, for lifecycle tests.
    #[cfg(test)]
    scales_set: Mutex<Vec<i64>>,
    revision: u64,
    recovery: Recovery,
    layout_watch: butterpollo_core::display_policy::LayoutWatch,
    /// Keep switching the physical monitors of an exclusive layout off while
    /// the stream is paused (`dd_hold_layout_while_paused`).
    hold_layout_while_paused: bool,
    mode_report: ModeReport,
    recovery_scale: i64,
    recovery_dimensions: (u32, u32),
    recovery_profile: Option<String>,
    host: std::sync::Weak<crate::state::Host>,
}

/// A device's own display mode (`display_mode`): width, height and refresh.
type DeviceMode = (u32, u32, Rate);

/// The frame limiter and the display refresh for `stream`: the stream's rate,
/// the app's frame generation, the device's own display mode, VRR and
/// Lossless Scaling. Also returns that display mode.
fn frame_policy(
    app: Option<&butterpollo_core::state::App>,
    launch: &Launch,
    stream: &Negotiated,
    config: &Config,
    virtual_mode: bool,
) -> Result<(Policy, Option<DeviceMode>)> {
    let option = |key: &str| {
        app.and_then(|a| a.extra.get(key))
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty())
    };
    let generation = option("frame-generation-mode")
        .or_else(|| option("frame-generation-provider"))
        .unwrap_or("none");
    let generation_enabled = if option("frame-generation-mode").is_some() {
        butterpollo_core::framegen::generation_provider(generation) != "none"
    } else {
        app.as_ref().is_some_and(|a| {
            [
                "frame-generation-enabled",
                "gen1-framegen-fix",
                "gen2-framegen-fix",
                "frame-generation-capture-fix",
            ]
            .iter()
            .any(|key| crate::process::app_bool(a, key, false))
        })
    };
    // A device's own display mode replaces the host's resolution and
    // refresh policies for its display; the stream keeps the rate the
    // client asked for.
    let device_mode = launch
        .client
        .extra
        .get("display_mode")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .and_then(|text| {
            let mode = butterpollo_core::display_policy::parse_display_mode(text);
            if mode.is_none() {
                tracing::warn!(
                    display_mode = text,
                    "ignoring a device display mode that is not WIDTHxHEIGHTxREFRESH"
                );
            }
            mode
        });
    let mut framegen = Policy::resolve(
        config,
        Rate(stream.fps_millihz()),
        virtual_mode,
        generation,
        generation_enabled,
        launch
            .client
            .extra
            .get("config_overrides")
            .and_then(serde_json::Value::as_object)
            .is_some_and(|o| o.contains_key("rtss_frame_limit_type"))
            || app
                .as_ref()
                .and_then(|a| a.extra.get("config-overrides"))
                .and_then(serde_json::Value::as_object)
                .is_some_and(|o| o.contains_key("rtss_frame_limit_type")),
    )?;
    if let Some((_, _, rate)) = device_mode {
        // A saved display mode must not cap a faster stream's game at its refresh rate.
        framegen.display_rate = butterpollo_core::framegen::display_rate(
            rate,
            virtual_mode,
            butterpollo_core::framegen::virtual_refresh(config),
        );
    }
    framegen = framegen.with_vrr(
        config,
        virtual_mode,
        stream.vrr_low_latency || launch.vrr_requested,
    );
    // Lossless Scaling doubles the frames: the game runs at its limit.
    if let Some(limit) = app
        .as_ref()
        .and_then(|a| serde_json::to_value(a).ok())
        .and_then(|a| {
            butterpollo_core::lossless::options(&a, config, f64::from(stream.fps_millihz()) / 1000.)
        })
        .and_then(|options| options.frame_limit)
    {
        framegen.rate = Rate(limit.saturating_mul(1000).min(1_000_000));
        framegen.enabled = true;
    }
    Ok((framegen, device_mode))
}

/// The scale a stream's display gets: the configured virtual display scale for a
/// virtual display or a remote monitor's retained one, and 0 (leave it alone) for a
/// physical display, whose scale belongs to the user.
fn virtual_display_scale(virtual_layout: bool, configured: i64) -> i64 {
    if virtual_layout { configured } else { 0 }
}

/// How a stream's display scale is set up ([`scale_setup`]).
#[derive(Debug, PartialEq)]
struct ScaleSetup {
    /// The scale for [`set_up_scale`]; 0 leaves it to a kept scale or to Windows.
    scale: i64,
    /// The scale is the device's own launch request (`rlDisplayScale`): Windows refusing it
    /// is a warning, as for a kept scale, while a host or device setting stays fatal.
    requested: bool,
    /// The device's scale was lowered to the largest the display's resolution allows.
    lowered: Option<String>,
    /// Read the scale Windows shows, to keep one picked in Windows during the stream.
    learns: bool,
}

impl ScaleSetup {
    /// The device's warning for this stream once the scale in force is known: Windows refused
    /// the scale it asked for (none is in force), else it was lowered.
    fn warning(&self, in_force: i64) -> Option<String> {
        if self.requested && in_force == 0 {
            return Some(format!(
                "Windows did not take the {}% display scaling the device asked for, so the virtual display keeps the scale Windows shows. Choose another PC display scaling on the device.",
                self.scale
            ));
        }
        self.lowered.clone()
    }
}

/// The scale a stream's display gets. On a virtual display (or a remote monitor's retained
/// one) the client's `rlDisplayScale` comes first, held to the `largest` scale Windows offers
/// at the display's resolution, then the merged `dd_virtual_display_scale` (device, app, host).
/// A physical display gets none here, and the client's scale is ignored for it. A scale is
/// learned from Windows only when the device `keeps` one and none is configured or requested.
fn scale_setup(
    config: &Config,
    options: &std::collections::BTreeMap<String, String>,
    virtual_layout: bool,
    keeps: bool,
    largest: impl FnOnce() -> Result<u32>,
) -> Result<ScaleSetup> {
    // Checked at /launch too, so a bad value never gets this far.
    let requested = butterpollo_core::display_policy::client_display_scale(options)?;
    if let Some(chosen) = requested.filter(|_| virtual_layout) {
        let (scale, lowered) = fit_client_scale(chosen, largest());
        return Ok(ScaleSetup {
            scale,
            requested: true,
            lowered,
            learns: false,
        });
    }
    let configured = config.integer("dd_virtual_display_scale", 0);
    Ok(ScaleSetup {
        scale: virtual_display_scale(virtual_layout, configured),
        requested: false,
        lowered: None,
        // A configured scale is the user's setting, so nothing is learned then.
        learns: keeps && configured == 0,
    })
}

/// A client's display scale, held to the largest Windows offers the display at
/// its resolution: a scale too large for it is lowered, with a warning, instead
/// of failing every stream. A range Windows does not report keeps the choice,
/// and a refusal is then a warning ([`ScaleSetup::warning`]).
fn fit_client_scale(chosen: i64, largest: Result<u32>) -> (i64, Option<String>) {
    match largest {
        Ok(largest) if chosen > i64::from(largest) => (
            i64::from(largest),
            Some(format!(
                "The device asked for {chosen}% display scaling, but Windows offers at most {largest}% at this resolution, so the virtual display uses {largest}%. Choose a smaller PC display scaling on the device, or a higher resolution."
            )),
        ),
        Ok(_) => (chosen, None),
        Err(error) => {
            tracing::warn!(scale = chosen, error = %format!("{error:#}"), "the display's scale range could not be read; asking Windows for the device's scale as chosen");
            (chosen, None)
        }
    }
}

/// Whether a paused stream keeps its layout's physical monitors off. Only an
/// exclusive layout switches them off, and only on request: by default a paused
/// stream leaves local display changes alone.
fn holds_layout_while_paused(config: &Config, exclusive: bool) -> bool {
    exclusive && config.boolean("dd_hold_layout_while_paused", false)
}

/// Puts the configured virtual display scale on `output`, whichever display it is: a stream's
/// own, or a remote monitor's retained one, which nothing else scales.
fn scale_display(
    output: &str,
    configured: i64,
    (width, height): (u32, u32),
    set: impl FnOnce(&str, i64, u32, u32) -> Result<()>,
) -> Result<()> {
    set(output, configured, width, height)
}

/// The ICC profile an HDR stream's display gets: the device's own choice,
/// else, when the host carries it over to a new virtual display, the
/// physical display's Windows HDR Calibration.
fn stream_profile(
    hdr: bool,
    chosen: Option<&str>,
    carry_calibration: bool,
    calibration: impl FnOnce() -> Option<String>,
) -> Option<String> {
    if !hdr {
        return None;
    }
    match chosen.filter(|name| !name.is_empty()) {
        Some(name) => Some(name.to_owned()),
        None if carry_calibration => calibration(),
        None => None,
    }
}

/// The Windows HDR Calibration of a physical HDR display, for the virtual
/// display that stands in for it. A stream without one says so.
fn physical_calibration() -> Option<String> {
    match hdr_profile::calibration() {
        Ok(Some(calibration)) => {
            tracing::info!(
                display = %calibration.display,
                profile = %calibration.profile,
                peak_nits = calibration.peak,
                "the virtual display gets the physical display's Windows HDR Calibration"
            );
            Some(calibration.profile)
        }
        Ok(None) => {
            tracing::warn!(
                "virtual_display_hdr_calibration is on, but no physical HDR display has a Windows HDR Calibration profile, so the virtual display streams uncalibrated. Run the Windows HDR Calibration app on the HDR display, or choose a profile for the device"
            );
            None
        }
        Err(error) => {
            tracing::warn!(
                error = %format!("{error:#}"),
                "the physical display's Windows HDR Calibration could not be read, so the virtual display streams uncalibrated"
            );
            None
        }
    }
}

/// The scale a device kept from Windows, in percent, when it keeps one (`remember`).
fn kept_scale(
    remember: bool,
    extra: &std::collections::BTreeMap<String, serde_json::Value>,
) -> Option<u32> {
    if !remember {
        return None;
    }
    extra
        .get(REMEMBERED_SCALE)
        .and_then(serde_json::Value::as_u64)
        .and_then(|percent| u32::try_from(percent).ok())
        .filter(|percent| *percent > 0)
}

/// Puts the scale on a stream's virtual display: the configured one when there is one, else
/// the one the device kept from Windows (`remember_display_mode`). A configured scale always
/// wins. The kept one is applied after the display's mode is set, so it sits on the mode the
/// device keeps, and Windows refusing it is a warning: the stream goes on at the scale Windows
/// shows. So is refusing a scale the device `requested` at launch; refusing a configured one
/// fails the stream. Returns the scale the display is in force at, for recovery to put back
/// (0 after a refusal: recovery does not retry it).
fn apply_virtual_scale(
    output: &str,
    configured: i64,
    requested: bool,
    remembered: Option<u32>,
    (width, height): (u32, u32),
    set: impl FnOnce(&str, i64, u32, u32) -> Result<()>,
) -> Result<i64> {
    match (configured, remembered) {
        (percent, _) if requested && percent != 0 => {
            match scale_display(output, percent, (width, height), set) {
                Ok(()) => Ok(percent),
                Err(error) => {
                    tracing::warn!(
                        scale = percent,
                        error = %format!("{error:#}"),
                        "Windows did not take the display scale the device asked for; the display keeps the scale Windows shows"
                    );
                    Ok(0)
                }
            }
        }
        (0, Some(percent)) => match set(output, i64::from(percent), width, height) {
            Ok(()) => Ok(i64::from(percent)),
            Err(error) => {
                tracing::warn!(
                    scale = percent,
                    error = %format!("{error:#}"),
                    "Windows did not take the display scale this device kept; the display keeps the scale Windows shows"
                );
                Ok(0)
            }
        },
        _ => scale_display(output, configured, (width, height), set).map(|()| configured),
    }
}

/// Sets the stream's display scale, then reads the one Windows shows as the baseline a scale
/// picked during the stream is compared with. It is read after the kept scale is applied, so a
/// scale Windows refused is never mistaken for one the user picked. `learns` is false when a
/// scale is configured or the device does not keep one: nothing is read then.
#[allow(clippy::too_many_arguments)]
fn set_up_scale(
    output: &str,
    configured: i64,
    requested: bool,
    remembered: Option<u32>,
    dimensions: (u32, u32),
    learns: bool,
    set: impl FnOnce(&str, i64, u32, u32) -> Result<()>,
    read: impl FnOnce(&str) -> Result<u32>,
) -> Result<(i64, Option<u32>)> {
    let in_force = apply_virtual_scale(output, configured, requested, remembered, dimensions, set)?;
    let baseline = if learns {
        read(output)
            .inspect_err(|error| {
                tracing::warn!(error = %format!("{error:#}"), "the display scale could not be read; a scale picked in Windows is not kept")
            })
            .ok()
    } else {
        None
    };
    Ok((in_force, baseline))
}

/// The Windows scale a physical display takes for a stream: the device's own
/// `dd_virtual_display_scale` override only. The host's setting and an app's
/// are for virtual displays; a physical display's scale is the user's unless
/// the device asks for one. 0 leaves it.
fn physical_scale(client: &butterpollo_core::state::Client) -> i64 {
    const KEY: &str = "dd_virtual_display_scale";
    let Some(value) = client
        .extra
        .get("config_overrides")
        .and_then(serde_json::Value::as_object)
        .and_then(|overrides| overrides.get(KEY))
    else {
        return 0;
    };
    let mut own = Config::default();
    // A value the host cannot use was reported when the device's settings
    // were applied to the stream; it leaves the scale as it is.
    if own
        .update(&serde_json::Map::from_iter([(
            KEY.to_owned(),
            value.clone(),
        )]))
        .is_err()
    {
        return 0;
    }
    own.integer(KEY, 0)
}

/// The size a stream's own virtual display is made at: `size` in the shape
/// `dd_rotation_option` gives it. It is made in that shape rather than turned.
fn virtual_display_size(config: &Config, size: (u32, u32), stream: (u32, u32)) -> (u32, u32) {
    butterpollo_core::display_policy::virtual_shape(
        config.dd_rotation_option(),
        config.dd_manual_rotation(),
        size,
        stream,
    )
}

/// How a stream turns its physical display (`dd_rotation_option`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct PhysicalTurn {
    /// The rotation in degrees it turns to, or None to leave it.
    degrees: Option<u16>,
    /// The resolution asked of the display before the turn, in the shape it
    /// has then.
    before: Option<(u32, u32)>,
    /// The resolution the desktop shows once it has turned.
    after: Option<(u32, u32)>,
}

/// The turn a stream gives its physical display. `read` gives the display's
/// rotation and desktop size; it is read only when the option can turn the
/// display, and a display that cannot be read fails the stream rather than
/// stream it unturned.
fn plan_physical_turn(
    config: &Config,
    stream: (u32, u32),
    resolution: Option<(u32, u32)>,
    read: impl FnOnce() -> Result<(u16, (u32, u32))>,
) -> Result<PhysicalTurn> {
    let unturned = PhysicalTurn {
        degrees: None,
        before: resolution,
        after: resolution,
    };
    let option = config.dd_rotation_option();
    if option == butterpollo_core::config::DisplayAdjust::Disabled {
        return Ok(unturned);
    }
    let (current, desktop) =
        read().context("the display's rotation could not be read for dd_rotation_option")?;
    let Some(degrees) = butterpollo_core::display_policy::physical_rotation(
        option,
        config.dd_manual_rotation(),
        current,
        desktop,
        stream,
    ) else {
        return Ok(unturned);
    };
    let before =
        resolution.map(|size| butterpollo_core::display_policy::mode_before_turn(size, desktop));
    // A quarter turn swaps the desktop's width and height.
    let quarter = (degrees + 360 - current) % 180 == 90;
    Ok(PhysicalTurn {
        degrees: Some(degrees),
        before,
        after: before.map(|(width, height)| {
            if quarter {
                (height, width)
            } else {
                (width, height)
            }
        }),
    })
}

/// What a stream changes on its physical display beside the mode, so tests
/// can stand in for the display lease.
trait PhysicalAdjust {
    fn rotate(&mut self, degrees: u16) -> Result<()>;
    fn scale(&mut self, configured: i64) -> Result<()>;
}
impl PhysicalAdjust for Guard {
    fn rotate(&mut self, degrees: u16) -> Result<()> {
        Guard::rotate(self, degrees)
    }
    fn scale(&mut self, configured: i64) -> Result<()> {
        Guard::scale(self, configured)
    }
}

/// Turns, then scales, a stream's physical display. The turn comes first: the
/// display's mode was set in its shape before the turn, and a recommended
/// scale is read off the turned desktop. The lease puts both back.
fn adjust_physical(lease: &mut impl PhysicalAdjust, turn: Option<u16>, scale: i64) -> Result<()> {
    if let Some(degrees) = turn {
        lease.rotate(degrees)?;
    }
    if scale != 0 {
        lease.scale(scale)?;
    }
    Ok(())
}

/// How a stream's own display follows a stream that turns between portrait
/// and landscape (`dd_rotation_option = auto`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Follow {
    /// Its virtual display changes shape.
    Virtual,
    /// Its physical display turns.
    Physical,
}

/// What following a turned stream needs of the display lease, so tests can
/// stand in for it.
trait TurnLease: PhysicalAdjust {
    fn virtual_size(&self) -> Option<(u32, u32)>;
    fn reshape_virtual(&self, width: u32, height: u32) -> Result<()>;
}
impl TurnLease for Guard {
    fn virtual_size(&self) -> Option<(u32, u32)> {
        Guard::virtual_size(self)
    }
    fn reshape_virtual(&self, width: u32, height: u32) -> Result<()> {
        Guard::reshape_virtual(self, width, height)
    }
}

/// Gives the stream's display the shape of a stream that turned to `to`: a
/// virtual display is reshaped, a physical one turned (`read` gives its
/// rotation and desktop size). Returns the desktop size it shows now, or None
/// when the display already has the stream's shape or keeps its own.
fn follow_turn(
    lease: &mut impl TurnLease,
    follow: Follow,
    to: (u32, u32),
    mut read: impl FnMut() -> Result<(u16, (u32, u32))>,
) -> Result<Option<(u32, u32)>> {
    use butterpollo_core::config::DisplayAdjust;
    match follow {
        Follow::Virtual => {
            let size = lease.virtual_size().context(
                "the virtual display is shared or not at the stream's size, so it keeps its shape",
            )?;
            let shaped =
                butterpollo_core::display_policy::virtual_shape(DisplayAdjust::Auto, 0, size, to);
            if shaped == size {
                return Ok(None);
            }
            lease.reshape_virtual(shaped.0, shaped.1)?;
            Ok(Some(shaped))
        }
        Follow::Physical => {
            let (current, desktop) =
                read().context("the display's rotation could not be read to follow the stream")?;
            let Some(degrees) = butterpollo_core::display_policy::physical_rotation(
                DisplayAdjust::Auto,
                0,
                current,
                desktop,
                to,
            ) else {
                return Ok(None);
            };
            lease.rotate(degrees)?;
            // A display another stream uses keeps its rotation.
            let (_, turned) = read().context("the turned display could not be read")?;
            Ok((turned != desktop).then_some(turned))
        }
    }
}

impl Prepared {
    /// Nothing prepared: a stream of `output` that holds no display, layout,
    /// profile or lease.
    fn bare(
        h: &Shared,
        output: String,
        framegen: Policy,
        mode: (u32, u32, u32, bool, bool),
        mode_report: ModeReport,
    ) -> Self {
        Self {
            display: None,
            output,
            framegen,
            _arrangement: None,
            _activation: None,
            _profile: None,
            _retained: None,
            _vulkan: None,
            _golden: None,
            mode,
            client_display: None,
            virtual_layout: None,
            layout_owner: None,
            inherited_layout: None,
            resolution_owner: None,
            scale_baseline: None,
            follow: None,
            #[cfg(test)]
            layout_snapshot: None,
            #[cfg(test)]
            size_snapshot: None,
            #[cfg(test)]
            scale_snapshot: None,
            #[cfg(test)]
            scales_set: Mutex::new(Vec::new()),
            revision: 0,
            recovery: Recovery::new(),
            layout_watch: Default::default(),
            // A game window holds no layout to keep while paused.
            hold_layout_while_paused: false,
            mode_report,
            recovery_scale: 0,
            recovery_dimensions: (mode.0, mode.1),
            recovery_profile: None,
            host: Arc::downgrade(h),
        }
    }
    /// Follows a stream that turned to `to` mid-stream when the device's
    /// display follows it. A display that cannot follow fails loudly and keeps
    /// streaming as it is: the colour conversion scales its picture.
    fn follow_shape(&mut self, to: (u32, u32)) -> Result<()> {
        let Some(follow) = self.follow else {
            return Ok(());
        };
        let output = self.output.clone();
        let Some(display) = self.display.as_mut() else {
            return Ok(());
        };
        let Some(size) = follow_turn(display, follow, to, || {
            butterpollo_windows::display::stream_display_rotation(&output)
        })?
        else {
            return Ok(());
        };
        tracing::info!(output = %self.output, width = size.0, height = size.1, "the stream's display turned with the stream");
        self.recovery_dimensions = size;
        self.mode_report.adopt(size);
        self.mode_report.publish(applied_mode(&self.output));
        Ok(())
    }
    fn capture_target(&self) -> (String, u64) {
        self._retained.as_ref().map_or_else(
            || (self.output.clone(), self.revision),
            |display| display.capture_target(),
        )
    }
    fn feed(&mut self, active: bool) -> Result<()> {
        // Renew the driver lease while paused, without reactivating a display
        // the user switched off through Win+P or restoring its mode/HDR.
        if !active {
            self.display.as_mut().map_or(Ok(()), Guard::keep_alive)?;
            // Opted in (issue #56): a monitor that wakes up or re-announces
            // itself over HDMI must not take the game while the device is away.
            return if self.hold_layout_while_paused {
                self.keep_layout()
            } else {
                Ok(())
            };
        }
        let fed = self.display.as_mut().map_or(Ok(false), Guard::feed);
        self.finish_feed(fed)
    }
    fn finish_feed(&mut self, fed: Result<bool>) -> Result<()> {
        // A heartbeat step that keeps failing, such as restoring a recovered
        // display's HDR, must not hold back the layout recovery below.
        if let (Ok(true), Some(display)) = (&fed, &self.display) {
            self.output = display.output.clone();
            self.revision = self.revision.wrapping_add(1);
            self.recovery.start(std::time::Instant::now());
        }
        // A remote monitor's own lease recreates its display; the layout and
        // HDR profile this stream applied follow the new one.
        if let Some(retained) = &self._retained {
            let (output, generation) = retained.capture_target();
            if generation != self.revision {
                self.output = output;
                self.revision = generation;
                self.recovery.start(std::time::Instant::now());
            }
        }
        if self.recovery.due(std::time::Instant::now()) {
            // The lease does not set a remote monitor's scale; it needs it again as well.
            scale_display(
                &self.output,
                self.recovery_scale,
                self.recovery_dimensions,
                butterpollo_windows::display::virtual_scale,
            )?;
            if let Some(arrangement) = &self._arrangement {
                arrangement.reapply(&self.output, &self.remote_monitors())?;
            }
            if self.recovery.take_mode() {
                if let Some(display) = &self.display {
                    display.apply_virtual_mode("after recovery layout");
                }
                self.mode_report.publish(applied_mode(&self.output));
            }
            if let Some(profile) = &self.recovery_profile {
                self._profile.take();
                self._profile = Some(hdr_profile::Lease::acquire(&self.output, profile)?);
            }
            self.recovery.finish();
        } else {
            self.keep_layout()?;
        }
        fed.map(|_| ())
    }
    fn remote_monitors(&self) -> Vec<String> {
        self.host
            .upgrade()
            .map(|h| {
                h.monitors
                    .lock()
                    .unwrap()
                    .values()
                    .map(|m| m.current_output())
                    .collect()
            })
            .unwrap_or_default()
    }
    /// Put the stream's layout back when Windows switches on a display it
    /// turned off. An exclusive-fullscreen game losing focus (the Win key,
    /// Alt+Tab) makes Windows recall its saved layout, which has the physical
    /// monitor on; the virtual display stays on, so the recovery above never
    /// starts.
    fn keep_layout(&mut self) -> Result<()> {
        let now = std::time::Instant::now();
        if self.recovery.pending || !self.layout_watch.due(now) {
            return Ok(());
        }
        let Some(arrangement) = &self._arrangement else {
            return Ok(());
        };
        let switched_on = match arrangement.switched_back_on() {
            Ok(switched_on) => switched_on,
            Err(error) => {
                self.layout_watch.failed(now);
                return Err(error);
            }
        };
        if !self.layout_watch.observe(!switched_on.is_empty(), now) {
            return Ok(());
        }
        tracing::info!(displays = ?switched_on, "displays the stream layout switched off came back on; reapplying it");
        let result = arrangement.reapply(&self.output, &self.remote_monitors());
        if result.is_err() {
            self.layout_watch.failed(now);
        }
        result
    }
    fn create(
        h: &Shared,
        launch: &Launch,
        stream: &Negotiated,
        config: &Config,
        physical_only: bool,
    ) -> Result<(Self, limiter::Lease)> {
        let app = crate::stream::launch_app(h, launch);
        let output_override = app
            .as_ref()
            .and_then(|a| a.extra.get("display-output"))
            .and_then(serde_json::Value::as_str)
            .or_else(|| {
                launch
                    .client
                    .extra
                    .get("output_name_override")
                    .and_then(serde_json::Value::as_str)
                    .filter(|s| !s.is_empty())
            });
        let output = output_override
            .unwrap_or(config.get("output_name", ""))
            .trim();
        let golden = if launch.role == Role::Stream
            && config.boolean("dd_always_restore_from_golden", false)
        {
            crate::maintenance::baseline(h)?
                // A saved layout naming displays that are gone (an old
                // monitor) would switch off the ones in use; keep the
                // layout from before the stream instead.
                .filter(|snapshot| {
                    let connected = snapshot.displays_connected();
                    if !connected {
                        tracing::info!("saved display baseline names displays that are not connected; restoring the layout from before the stream");
                    }
                    connected
                })
                .map(|snapshot| GoldenLease::new(h, snapshot, config))
                .transpose()?
        } else {
            None
        };
        let mode = virtual_display_mode(
            config,
            &launch.client,
            app.as_ref(),
            butterpollo_windows::display::virtual_display_os(),
        );
        let client_virtual = launch
            .options
            .get("virtualDisplay")
            .map(|value| value != "0");
        let client_display = client_display(launch);
        let background = butterpollo_core::background::enabled(config);
        let mut display_request = butterpollo_core::display_policy::VirtualDisplayRequest {
            client_requested: client_virtual == Some(true)
                || matches!(client_display, Some(HostDisplay::Virtual(_))),
            // Streaming the PC's own monitor would show the device whatever
            // the person at the PC is doing.
            client_physical: client_display == Some(HostDisplay::Physical) && !background,
            client_forced: background
                || launch
                    .client
                    .extra
                    .get("always_use_virtual_display")
                    .is_some_and(|v| v == true || v == "true"),
            app_requested: app.as_ref().is_some_and(|a| {
                crate::process::app_bool(a, "virtual-display", false)
                    || crate::process::app_bool(a, "virtual-screen", false)
            }),
            configured: mode != "disabled" || config.boolean("dd_activate_virtual_display", false),
            output_override,
            configured_output: config.get("output_name", ""),
            headless: false,
        };
        if !physical_only && !display_request.requested() {
            display_request.headless =
                butterpollo_windows::capture::displays().is_ok_and(|displays| displays.is_empty());
            if display_request.headless {
                tracing::info!(
                    "no display is active; this stream uses a virtual display although the settings or the client turn it off"
                );
            }
        }
        let virtual_mode = display_request.uses_virtual(
            physical_only,
            butterpollo_windows::display::ensure_virtual_display_available,
        );
        if background && !virtual_mode && launch.role == Role::Stream {
            anyhow::bail!(
                "background streaming needs Rubylight's virtual display, which is unavailable{}; it never streams the PC's own monitor",
                if physical_only {
                    " while Windows is locked"
                } else {
                    ""
                }
            );
        }
        if physical_only && display_request.requested() {
            launch.warnings.set("display_virtual", "Windows was locked and the virtual display could not be set up there, so this stream shows the physical display. Start the stream again after signing in to use the virtual display.");
        } else if display_request.requested()
            && !virtual_mode
            && let Some(reason) = butterpollo_core::os_gate::missing(
                butterpollo_windows::display::windows_build(),
                butterpollo_core::os_gate::Feature::VirtualDisplay,
            )
        {
            launch.warnings.set("display_virtual", format!("Using the physical display because this Windows cannot load the virtual display driver. {reason} Update to Windows 11 23H2 or later to use a virtual display."));
        } else if display_request.requested() && !virtual_mode {
            launch.warnings.set("display_virtual", "Using a physical display because the virtual display driver is unavailable. The desktop is visible locally and its refresh can limit fresh frames; repair the virtual display driver (run \"butterpollo-setup.exe --repair-drivers --now\" from the Rubylight folder in an administrator shell) or explicitly select the physical display.");
        } else {
            launch.warnings.clear("display_virtual");
        }
        let (framegen, device_mode) =
            frame_policy(app.as_ref(), launch, stream, config, virtual_mode)?;
        let limiter = limiter::Lease::acquire(&h.directory, config, &framegen)?;
        let vulkan = if stream.hdr && config.boolean("vulkan_hdr_layer", true) {
            Some(vulkan::Lease::acquire()?)
        } else {
            None
        };
        let id = virtual_display_identity(config, launch, app.as_ref());
        let shared_id = if mode == "shared" {
            let mut paired = h.paired.write().unwrap();
            let existing = paired.document["root"]["shared_virtual_display_guid"]
                .as_str()
                .filter(|value| uuid::Uuid::parse_str(value).is_ok())
                .map(str::to_owned);
            if let Some(id) = existing {
                id
            } else {
                let id = uuid::Uuid::new_v4().to_string();
                paired.document["root"]["shared_virtual_display_guid"] = id.clone().into();
                paired.save(&h.paired_path)?;
                id
            }
        } else {
            id
        };
        let stable_id = &shared_id;
        let app_scale = app
            .as_ref()
            .and_then(|a| a.extra.get("scale-factor"))
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(100);
        let client_scale = launch
            .options
            .get("scaleFactor")
            .and_then(|value| value.parse().ok())
            .unwrap_or(100);
        let (render_width, render_height) = butterpollo_core::display_policy::render_dimensions(
            stream.width,
            stream.height,
            client_scale,
            app_scale,
        );
        let mut request = config.display_request_rate(
            render_width,
            render_height,
            framegen.display_rate,
            stream.hdr,
            virtual_mode,
        )?;
        if let Some((width, height, _)) = device_mode
            && (virtual_mode || config.get("dd_configuration_option", "verify_only") != "disabled")
        {
            request.resolution = Some((width, height));
            request.refresh = Some(framegen.display_rate.0);
            request.prefer_highest = false;
        }
        // A resolution the device kept from Windows replaces only the
        // resolution: the refresh follows the stream as usual. A shared display
        // keeps the first stream's mode for all, and a display mode set for
        // the device comes first.
        let remember = launch.role == Role::Stream
            && virtual_mode
            && mode != "shared"
            && device_mode.is_none()
            && launch
                .client
                .extra
                .get("remember_display_mode")
                .is_some_and(|v| v == true || v == "true");
        if remember
            && let Some(resolution) = launch
                .client
                .extra
                .get(REMEMBERED_RESOLUTION)
                .and_then(serde_json::Value::as_str)
                .and_then(butterpollo_core::display_policy::parse_resolution)
        {
            request.resolution = Some(resolution);
        }
        let (width, height) = if virtual_mode {
            virtual_display_size(
                config,
                request.resolution.unwrap_or((render_width, render_height)),
                (stream.width, stream.height),
            )
        } else {
            (stream.width, stream.height)
        };
        let rate = if virtual_mode {
            Rate(request.refresh.unwrap_or(framegen.display_rate.0))
        } else {
            framegen.display_rate
        };
        let retained = if launch.role == Role::RemoteMonitor {
            Some(crate::remote_display::activate(
                h,
                &launch.client.uuid,
                stream,
            )?)
        } else {
            None
        };
        let activation = if !virtual_mode
            && retained.is_none()
            && matches!(
                config.get("dd_configuration_option", "verify_only"),
                "ensure_active" | "ensure_primary" | "ensure_only_display"
            ) {
            display_arrangement::Activation::acquire(output)?
        } else {
            None
        };
        // A physical display turns to the stream's shape (dd_rotation_option),
        // read once it is on. Its mode is asked for in the shape it has before
        // the turn, and its highest refresh looked up at that mode.
        let physical = !virtual_mode && retained.is_none();
        let follow = (config.dd_rotation_option() == butterpollo_core::config::DisplayAdjust::Auto
            && retained.is_none())
        .then_some(if virtual_mode {
            Follow::Virtual
        } else {
            Follow::Physical
        });
        let turn = if physical {
            plan_physical_turn(
                config,
                (stream.width, stream.height),
                request.resolution,
                || butterpollo_windows::display::stream_display_rotation(output),
            )?
        } else {
            PhysicalTurn {
                before: request.resolution,
                after: request.resolution,
                ..Default::default()
            }
        };
        if request.prefer_highest && !virtual_mode {
            request.refresh =
                Some(butterpollo_windows::display::highest_refresh(output, turn.before)?.0);
        }
        let virtual_options = butterpollo_windows::display::VirtualOptions {
            peak_nits: config.integer("hdr_peak_brightness", 1000).clamp(400, 2000) as u32,
            render_adapter: if virtual_mode {
                butterpollo_windows::display::configured_render_adapter(config)
            } else {
                None
            },
        };
        // The layout to return to after the stream, taken before the virtual
        // display exists: Windows can switch other displays on or off, change
        // the primary or retime one when it arrives.
        let original = if virtual_mode && retained.is_none() && launch.role == Role::Stream {
            butterpollo_windows::display::Snapshot::capture()
                .inspect_err(|error| {
                    tracing::warn!(error = %format!("{error:#}"), "display layout before the virtual display could not be read")
                })
                .ok()
        } else {
            None
        };
        // Read while the physical displays are still on: the virtual display
        // can turn them off, and Windows keeps a profile only on an active one.
        let recovery_profile = stream_profile(
            stream.hdr,
            launch
                .client
                .extra
                .get("hdr_profile")
                .and_then(serde_json::Value::as_str),
            virtual_mode
                && retained.is_none()
                && config.boolean("virtual_display_hdr_calibration", false),
            physical_calibration,
        );
        let mut display = if retained.is_none() {
            Some(Guard::new_virtual_options(
                output,
                virtual_mode,
                stable_id,
                width,
                height,
                rate,
                request.hdr,
                turn.before,
                request.refresh.map(Rate),
                &virtual_options,
            )?)
        } else {
            None
        };
        // A virtual display was made in the stream's shape above and is never
        // turned; a physical one turns and takes the device's own scale here.
        if physical && let Some(guard) = display.as_mut() {
            adjust_physical(guard, turn.degrees, physical_scale(&launch.client))?;
        }
        let output = retained
            .as_ref()
            .map(|d| d.current_output())
            .or_else(|| display.as_ref().map(|d| d.output.clone()))
            .context("display lease unavailable")?;
        // A remote monitor is a virtual display even when this device's
        // streams otherwise use a physical one, so it follows the virtual
        // display layout.
        let virtual_layout = virtual_mode || retained.is_some();
        // A remote monitor's retained display too: the lease never sets the scale.
        // The client's own scale (`rlDisplayScale`, checked at /launch) comes
        // before the device, app and host setting.
        let setup = scale_setup(
            config,
            &launch.options,
            virtual_layout,
            remember && display.is_some(),
            || butterpollo_windows::display::largest_scale(&output),
        )?;
        // Without a configured scale, the one the device kept from Windows.
        let remembered = kept_scale(remember, &launch.client.extra);
        let (scale, scale_baseline) = set_up_scale(
            &output,
            setup.scale,
            setup.requested,
            remembered,
            (width, height),
            setup.learns,
            butterpollo_windows::display::virtual_scale,
            butterpollo_windows::display::output_scale,
        )?;
        match setup.warning(scale) {
            Some(warning) => launch.warnings.set("display_scale", warning),
            None => launch.warnings.clear("display_scale"),
        }
        // The client's choice for this stream comes before the device, app
        // and host layouts.
        let selection = if virtual_layout {
            virtual_display_layout(
                config,
                &launch.client,
                app.as_ref(),
                client_display.filter(|_| virtual_mode),
            )
        } else {
            config.get("dd_configuration_option", "verify_only")
        };
        // An unknown layout falls back to the default: exclusive for a
        // virtual display, verify only (no arrangement) for a physical one.
        // A remote monitor takes which displays stay on and which is primary
        // from it; the remote monitor layout places it.
        let parsed = if !matches!(launch.role, Role::Stream | Role::RemoteMonitor)
            || matches!(selection, "disabled" | "verify_only")
        {
            None
        } else {
            butterpollo_core::display_policy::Arrangement::parse(selection)
                .inspect_err(|_| {
                    butterpollo_core::config::invalid(
                        if virtual_layout {
                            "virtual_display_layout"
                        } else {
                            "dd_configuration_option"
                        },
                        selection,
                    )
                })
                .ok()
                .or(virtual_layout
                    .then_some(butterpollo_core::display_policy::Arrangement::Exclusive))
                .and_then(|parsed| {
                    if launch.role == Role::RemoteMonitor {
                        parsed.for_remote_monitor()
                    } else {
                        Some(parsed)
                    }
                })
        };
        let arrangement = match parsed {
            Some(parsed) => {
                let retained: Vec<_> = h
                    .monitors
                    .lock()
                    .unwrap()
                    .values()
                    .map(|m| m.current_output())
                    .collect();
                match display_arrangement::Lease::acquire(
                    &output,
                    parsed,
                    &retained,
                    virtual_mode && display.is_some(),
                    original,
                ) {
                    Ok(lease) => Some(lease),
                    // The remote monitor still works where the remote monitor
                    // layout put it.
                    Err(error) if launch.role == Role::RemoteMonitor => {
                        tracing::warn!(error = %format!("{error:#}"), layout = selection, "remote monitor layout could not be applied");
                        launch.warnings.set("display_layout", format!("The virtual display layout ({selection}) could not be applied to this remote monitor ({error:#}), so the other displays stay as they are. Check Windows display settings and reconnect."));
                        None
                    }
                    Err(error) => return Err(error),
                }
            }
            _ => None,
        };
        if let Some(display) = &display {
            // Applying the layout can recall the display's saved mode.
            display.apply_virtual_mode("after layout");
        }
        // A layout Windows did not take is not one the user can be said to have chosen
        // at the end of the stream (issue #40: "extended" learned from an "extended +
        // primary" stream whose display never became primary, which then overrode every
        // host setting).
        let layout_took = !virtual_layout
            || arrangement.is_none()
            || butterpollo_windows::display::monitors()
                .map(|monitors| {
                    let active: Vec<_> = monitors
                        .into_iter()
                        .map(|m| (m.display_name, m.primary))
                        .collect();
                    match butterpollo_core::display_policy::chosen_layout(
                        selection, &output, &active,
                    ) {
                        None => true,
                        Some(actual) => {
                            tracing::warn!(
                                output = %output,
                                planned = selection,
                                actual,
                                "Windows did not apply the virtual display layout"
                            );
                            launch.warnings.set(
                                "display_layout",
                                format!("Windows did not apply the {selection} display layout; the stream's display is {actual}. Check that no other display tool rearranges displays, then reconnect."),
                            );
                            false
                        }
                    }
                })
                .unwrap_or(false);
        tracing::info!(
            client = %launch.client.name,
            role = ?launch.role,
            client_virtual_display = ?client_virtual,
            client_display = ?client_display,
            virtual_display_mode = mode,
            output_override = ?output_override,
            virtual_display = virtual_mode,
            output = %output,
            layout = selection,
            arrangement = ?parsed,
            requested_refresh_millihz = request.refresh.or(virtual_mode.then_some(rate.0)),
            limiter_rate = %framegen.rate,
            limiter_enabled = framegen.enabled,
            "stream display selection"
        );
        // A virtual display is created for the stream at exactly this mode.
        let requested = if virtual_mode {
            (Some((width, height)), Some(rate.0), request.hdr)
        } else {
            (turn.after, request.refresh, request.hdr)
        };
        let mut mode_report =
            ModeReport::new(launch.warnings.clone(), requested, stream.fps_millihz());
        if virtual_mode && mode == "shared" {
            mode_report = mode_report.shared();
        }
        mode_report.publish(applied_mode(&output));
        let profile = recovery_profile
            .as_ref()
            .map(|p| hdr_profile::Lease::acquire(&output, p))
            .transpose()?;
        // A remote monitor's display has its own generation; see `feed`.
        let revision = retained.as_ref().map_or(0, |r| r.capture_target().1);
        let owns_display = display.is_some();
        let hold_layout_while_paused = holds_layout_while_paused(
            config,
            arrangement
                .as_ref()
                .is_some_and(display_arrangement::Lease::exclusive),
        );
        Ok((
            Self {
                display,
                output,
                framegen,
                _arrangement: arrangement,
                _activation: activation,
                _profile: profile,
                _retained: retained,
                _vulkan: vulkan,
                _golden: golden,
                mode: stream_mode(stream),
                // A background stream's fixed layout is not the device's choice.
                layout_owner: (launch.role == Role::Stream
                    && virtual_mode
                    && !background
                    && client_display.is_none()
                    && layout_took)
                    .then(|| launch.client.uuid.clone()),
                client_display,
                virtual_layout: virtual_layout.then(|| selection.to_owned()),
                inherited_layout: virtual_layout
                    .then(|| inherited_layout(config, app.as_ref()).to_owned()),
                resolution_owner: (remember && owns_display).then(|| launch.client.uuid.clone()),
                scale_baseline,
                follow,
                #[cfg(test)]
                layout_snapshot: None,
                #[cfg(test)]
                size_snapshot: None,
                #[cfg(test)]
                scale_snapshot: None,
                #[cfg(test)]
                scales_set: Mutex::new(Vec::new()),
                revision,
                recovery: Recovery::new(),
                layout_watch: Default::default(),
                hold_layout_while_paused,
                mode_report,
                recovery_scale: scale,
                recovery_dimensions: (width, height),
                recovery_profile,
                host: Arc::downgrade(h),
            },
            limiter,
        ))
    }
}
impl Prepared {
    fn active_layout(&self) -> Result<Vec<(String, bool)>> {
        #[cfg(test)]
        if let Some(snapshot) = &self.layout_snapshot {
            return Ok(snapshot.clone());
        }
        Ok(butterpollo_windows::display::monitors()?
            .into_iter()
            .map(|m| (m.display_name, m.primary))
            .collect())
    }
    /// Keep the layout the user chose for the stream's display (Windows' "Show
    /// only on 2", say) as the device's own, when its last stream disconnects.
    /// A paused lease's eventual destruction must never learn a local layout.
    fn keep_chosen_layout(&self) {
        let (Some(uuid), Some(planned)) = (&self.layout_owner, &self.virtual_layout) else {
            return;
        };
        let Some(h) = self.host.upgrade() else {
            return;
        };
        let active = match self.active_layout() {
            Ok(monitors) => monitors,
            Err(error) => {
                tracing::debug!(%error, "the layout at the end of the stream could not be read");
                return;
            }
        };
        let Some(chosen) =
            butterpollo_core::display_policy::chosen_layout(planned, &self.output, &active)
        else {
            return;
        };
        let mut paired = h.paired.write().unwrap();
        let Some(client) = paired.clients.iter_mut().find(|c| c.uuid == *uuid) else {
            return;
        };
        // The layout the user set for the device, before or during the stream, is
        // theirs to change in the console; what the stream ended with (a game's
        // mode switch, Windows moving the primary) never replaces it (issue #40).
        if has_user_layout(client) {
            tracing::debug!(client = %client.name, planned = %planned, chosen, "the device has a display layout of its own; not learning the one the stream ended with");
            return;
        }
        tracing::info!(client = %client.name, planned = %planned, chosen, "keeping the display layout chosen during the stream for this device");
        client
            .extra
            .insert("virtual_display_layout".into(), serde_json::json!(chosen));
        client.extra.remove(SET_BY_USER);
        if let Some(inherited) = &self.inherited_layout {
            client
                .extra
                .insert(LEARNED_OVER.into(), serde_json::json!(inherited));
        }
        if let Err(error) = paired.save(&h.paired_path) {
            tracing::warn!(%error, "the device's display layout could not be saved");
        }
    }
    /// The resolution the stream sets on its own virtual display and the one
    /// Windows shows.
    fn size_change(&self) -> Option<SizeChange> {
        #[cfg(test)]
        if let Some(snapshot) = self.size_snapshot {
            return Some(snapshot);
        }
        let set = self.display.as_ref()?.virtual_size()?;
        match applied_mode(&self.output) {
            Ok((width, height, _, _)) => Some((set, (width, height))),
            Err(error) => {
                tracing::debug!(%error, "the display resolution at the end of the stream could not be read");
                None
            }
        }
    }
    /// Keep the resolution the user chose for the stream's display in Windows
    /// (1440p under a 4K stream, say) for the device when its last stream
    /// disconnects, and put it back on resume and recovery from now on. The
    /// refresh is not kept: it stays the stream's, so VRR and the virtual
    /// refresh multipliers keep working.
    fn keep_chosen_resolution(&mut self) {
        let Some(uuid) = self.resolution_owner.clone() else {
            return;
        };
        let Some(h) = self.host.upgrade() else {
            return;
        };
        let Some((set, (width, height))) = self.size_change() else {
            return;
        };
        if (width, height) == set || width == 0 || height == 0 {
            return;
        }
        if let Some(display) = &self.display {
            display.adopt_virtual_size(width, height);
        }
        self.recovery_dimensions = (width, height);
        self.mode_report.adopt((width, height));
        let mut paired = h.paired.write().unwrap();
        let Some(client) = paired.clients.iter_mut().find(|c| c.uuid == uuid) else {
            return;
        };
        let chosen = format!("{width}x{height}");
        tracing::info!(client = %client.name, resolution = %chosen, "keeping the display resolution chosen during the stream for this device");
        client
            .extra
            .insert(REMEMBERED_RESOLUTION.into(), serde_json::json!(chosen));
        if let Err(error) = paired.save(&h.paired_path) {
            tracing::warn!(%error, "the device's display resolution could not be saved");
        }
    }
    /// Puts the display's scale back as the stream left it, as resume does for the layout and
    /// the mode: the baseline a scale picked during the stream is compared with, else the
    /// configured scale. A paused display's scale changed at the PC is not the device's choice.
    fn put_scale_back(&self) -> Result<()> {
        let scale = self.scale_baseline.map_or(self.recovery_scale, i64::from);
        if scale == 0 {
            return Ok(());
        }
        #[cfg(test)]
        self.scales_set.lock().unwrap().push(scale);
        #[cfg(not(test))]
        let applied = scale_display(
            &self.output,
            scale,
            self.recovery_dimensions,
            butterpollo_windows::display::virtual_scale,
        );
        #[cfg(test)]
        let applied: Result<()> = Ok(());
        match (applied, self.scale_baseline) {
            // The scale a device keeps is not worth failing the resume for.
            (Err(error), Some(_)) => {
                tracing::warn!(error = %format!("{error:#}"), scale, "Windows did not take the display scale back after the pause");
                Ok(())
            }
            (applied, _) => applied,
        }
    }
    /// The scale the stream's display shows.
    fn scale_now(&self) -> Result<u32> {
        #[cfg(test)]
        if let Some(snapshot) = self.scale_snapshot {
            return Ok(snapshot);
        }
        butterpollo_windows::display::output_scale(&self.output)
    }
    /// Keep the scale the user chose for the stream's display in Windows for the
    /// device when its last stream disconnects, and put it back on recovery from
    /// now on. Only a scale Windows shows is kept, so one it refused never is.
    /// The refresh is not kept: the stream's rate always wins over a kept one.
    fn keep_chosen_scale(&mut self) {
        let (Some(uuid), Some(baseline)) = (self.resolution_owner.clone(), self.scale_baseline)
        else {
            return;
        };
        let Some(h) = self.host.upgrade() else {
            return;
        };
        let chosen = match self.scale_now() {
            Ok(chosen) => chosen,
            Err(error) => {
                tracing::debug!(%error, "the display scale at the end of the stream could not be read");
                return;
            }
        };
        if chosen == baseline || chosen == 0 {
            return;
        }
        self.scale_baseline = Some(chosen);
        self.recovery_scale = i64::from(chosen);
        let mut paired = h.paired.write().unwrap();
        let Some(client) = paired.clients.iter_mut().find(|c| c.uuid == uuid) else {
            return;
        };
        tracing::info!(client = %client.name, scale = chosen, "keeping the display scale chosen during the stream for this device");
        client
            .extra
            .insert(REMEMBERED_SCALE.into(), serde_json::json!(chosen));
        if let Err(error) = paired.save(&h.paired_path) {
            tracing::warn!(%error, "the device's display scale could not be saved");
        }
    }
}

impl Drop for Prepared {
    fn drop(&mut self) {
        // A stream ending while Windows is locked restores the layout too.
        butterpollo_windows::input::on_input_desktop(|| {
            self._profile.take();
            self._arrangement.take();
            self.display.take();
            self._activation.take();
            self._golden.take();
            self._retained.take();
            self._vulkan.take();
        });
    }
}

/// What `retime` could do with the launch's display.
enum Raise {
    /// No owned virtual display: a physical or shared one.
    NotOwned,
    /// Windows refused this refresh on the owned virtual display.
    Refused(Rate),
    Raised(u32),
}
/// The launch prepared the display for its own rate and the stream asks for
/// another, as Artemis's Warp frame pacing does (launch at 60, ask for 120 or
/// 240). An owned virtual display keeps running, so a game on it stays put:
/// its refresh is raised where the stream needs more, and the frame limiter
/// follows the stream's rate. One Windows will not raise is kept as it is.
/// Any other difference, or a display the host does not own, comes back as
/// `Err` to be prepared again.
pub fn retime(
    preparation: StreamPreparation,
    h: &Shared,
    launch: &Launch,
    stream: &Negotiated,
    config: &Config,
) -> Result<std::result::Result<StreamPreparation, StreamPreparation>> {
    let display = preparation.display.clone();
    let (width, height, launch_rate, hdr, vrr) = display.mode;
    if (width, height, hdr, vrr)
        != (
            stream.width,
            stream.height,
            stream.hdr,
            stream.vrr_low_latency,
        )
    {
        return Ok(Err(preparation));
    }
    let app = crate::stream::launch_app(h, launch);
    let (framegen, device_mode) = frame_policy(app.as_ref(), launch, stream, config, true)?;
    let raised = butterpollo_windows::input::on_input_desktop(|| -> Result<Raise> {
        let mut prepared = display._state.lock().unwrap();
        let Some(guard) = &prepared.display else {
            return Ok(Raise::NotOwned);
        };
        let (display_width, display_height) = prepared.recovery_dimensions;
        let request = config.display_request_rate(
            display_width,
            display_height,
            framegen.display_rate,
            stream.hdr,
            true,
        )?;
        // As for a new display: a device's own mode sets its refresh.
        let rate = if device_mode.is_some() {
            framegen.display_rate
        } else {
            Rate(request.refresh.unwrap_or(framegen.display_rate.0))
        };
        let Some(applied) = guard.raise_virtual_rate(rate) else {
            return Ok(Raise::Refused(rate));
        };
        prepared.framegen = framegen.clone();
        prepared.mode_report.retime(
            (
                Some((display_width, display_height)),
                Some(applied),
                request.hdr,
            ),
            stream.fps_millihz(),
        );
        prepared.mode_report.publish(applied_mode(&prepared.output));
        Ok(Raise::Raised(applied))
    })?;
    let refresh = match raised {
        Raise::Raised(refresh) => refresh,
        Raise::NotOwned => return Ok(Err(preparation)),
        // Preparing the display again would recreate the virtual display
        // under a game that is already running on it. Keep it; the stream
        // gets the frames its refresh gives, as before this change.
        Raise::Refused(rate) => {
            tracing::warn!(
                client = %launch.client.name,
                launch_millihz = launch_rate,
                stream_millihz = stream.fps_millihz(),
                wanted = %rate,
                "the virtual display could not be raised to the stream's rate; keeping it, so the stream runs at its refresh"
            );
            return Ok(Ok(preparation));
        }
    };
    tracing::info!(
        client = %launch.client.name,
        launch_millihz = launch_rate,
        stream_millihz = stream.fps_millihz(),
        display_millihz = refresh,
        limiter_rate = %framegen.rate,
        limiter_enabled = framegen.enabled,
        "the stream asks for another rate than its launch; the display and the frame limiter follow it"
    );
    *display.framegen.lock().unwrap() = framegen.clone();
    // The launch's preparation goes, with its limiter: a second lease would
    // leave the launch's rate applied.
    display.active.fetch_add(1, Ordering::AcqRel);
    if let Some(retained) = &display.retained {
        retained.stream_started();
    }
    drop(preparation);
    match limiter::Lease::acquire(&h.directory, config, &framegen) {
        Ok(limiter) => Ok(Ok(StreamPreparation {
            display,
            _limiter: limiter,
        })),
        Err(error) => {
            display.active.fetch_sub(1, Ordering::AcqRel);
            if let Some(retained) = &display.retained {
                retained.stream_stopped();
            }
            Err(error)
        }
    }
}

/// Prepare a stream's display. While Windows shows the lock or sign-in
/// screen it allows display configuration only from that screen's desktop,
/// so the work moves there (issue #6). If the virtual display or its layout
/// still cannot be set up, the stream shows the physical display, as
/// Vibepollo does while Windows is locked.
pub fn prepare_stream(
    h: &Shared,
    launch: &Launch,
    stream: &Negotiated,
    config: &Config,
) -> Result<StreamPreparation> {
    if let Some(target) = configured_window(config)? {
        configured_window_open(&target)?;
        tracing::info!(target = %target, "streaming the configured window");
        let name = target.to_string();
        // The window is checked every 100 ms: closed or minimised, the stream
        // says so while its capture fails (stream.rs gives up after 30 s).
        return Ready::window(
            h,
            stream,
            config,
            name.clone(),
            Box::new(move || configured_window_open(&target).map(|()| name.clone())),
            launch.warnings.clone(),
        );
    }
    let prepare = |physical_only: bool| {
        butterpollo_windows::input::on_input_desktop(|| {
            Ready::prepare(Prepared::create(h, launch, stream, config, physical_only)?)
        })
    };
    match prepare(false) {
        Err(error) if butterpollo_windows::input::secure_desktop_shown() => {
            tracing::warn!(error = %format!("{error:#}"), "stream display could not be prepared while Windows is locked; trying the physical display");
            prepare(true).map_err(|fallback| {
                tracing::warn!(error = %format!("{fallback:#}"), "physical display could not be prepared either");
                error
            })
        }
        result => result,
    }
}

/// One window streamed instead of a display: `output_name` set to
/// `window:<hwnd>/<pid>@<display>`, as the release harness does for its looped
/// game clip (e2e.py --content). Like a co-op player's window, no display is
/// prepared, changed or kept; a malformed target is refused, never streamed as
/// a display, and a window that closes fails the capture loudly. The target
/// must name the window's process: a handle written into the settings outlives
/// its window, and Windows gives it to the next one.
fn configured_window(
    config: &Config,
) -> Result<Option<butterpollo_windows::capture::WindowTarget>> {
    let output = config.get("output_name", "").trim();
    let target = butterpollo_windows::capture::WindowTarget::parse(output)
        .context("output_name is neither a display nor a window:<hwnd>/<pid>@<display> target")?;
    if let Some(target) = &target {
        anyhow::ensure!(
            target.process.is_some(),
            "output_name names the window {target} without its process: use window:<hwnd>/<pid>@<display>"
        );
    }
    Ok(target)
}

/// A closed, minimised, reused or misplaced configured window refuses the
/// stream with the reason.
fn configured_window_open(target: &butterpollo_windows::capture::WindowTarget) -> Result<()> {
    butterpollo_windows::capture::check_window(target)
        .with_context(|| format!("output_name names the window {target}, which cannot be streamed"))
}

/// Streams share the saved layout's restoration, like the arrangement and
/// display settings leases: the first records it for crash recovery and the
/// last restores it. One client's end must not switch off another client's
/// display or reset its mode and HDR, nor clear the journal under it.
struct Baseline<T> {
    users: usize,
    held: Option<T>,
}
impl<T> Baseline<T> {
    const fn new() -> Self {
        Self {
            users: 0,
            held: None,
        }
    }
    /// The first user records the layout; later users join it.
    fn acquire(&mut self, record: impl FnOnce() -> Result<T>) -> Result<()> {
        if self.users == 0 {
            self.held = Some(record()?);
        }
        self.users += 1;
        Ok(())
    }
    /// Only the last user restores the layout.
    fn release(&mut self, restore: impl FnOnce(T)) {
        self.users -= 1;
        if self.users == 0
            && let Some(held) = self.held.take()
        {
            restore(held);
        }
    }
}
type SavedLayout = (butterpollo_windows::display::Snapshot, Vec<String>);
static BASELINE: Mutex<Baseline<SavedLayout>> = Mutex::new(Baseline::new());

struct GoldenLease {
    host: std::sync::Weak<crate::state::Host>,
}
impl GoldenLease {
    fn new(
        h: &Shared,
        snapshot: butterpollo_windows::display::Snapshot,
        config: &Config,
    ) -> Result<Self> {
        let excluded = crate::maintenance::display_exclusions(config)?;
        BASELINE.lock().unwrap().acquire(|| {
            tracing::info!("saved display baseline selected for restoration after streaming");
            butterpollo_windows::display_recovery::baseline(Some((
                snapshot.clone(),
                excluded.clone(),
            )))?;
            Ok((snapshot, excluded))
        })?;
        Ok(Self {
            host: Arc::downgrade(h),
        })
    }
}
impl Drop for GoldenLease {
    fn drop(&mut self) {
        // Held through the restore, so a stream starting meanwhile records
        // its journal entry after this one is cleared, not before.
        BASELINE
            .lock()
            .unwrap()
            .release(|(snapshot, mut excluded)| {
                if let Some(host) = self.host.upgrade() {
                    let outputs: Vec<_> = host
                        .monitors
                        .lock()
                        .unwrap()
                        .values()
                        .map(|m| m.current_output())
                        .collect();
                    if let Ok(monitors) = butterpollo_windows::display::monitors() {
                        excluded.extend(
                            monitors
                                .iter()
                                .filter(|m| outputs.contains(&m.display_name))
                                .map(|m| m.device_id.clone()),
                        );
                    }
                }
                // Displays still in use stay as they are: a stream without
                // this lease, a paused game display or a launch being prepared.
                excluded.extend(butterpollo_windows::display::leased_displays());
                tracing::info!("restoring the saved display baseline");
                if let Err(error) = snapshot.restore_excluding(&excluded) {
                    tracing::warn!(%error,"saved display baseline restoration remains pending");
                } else {
                    let _ = butterpollo_windows::display_recovery::baseline(None);
                }
            });
    }
}

#[cfg(test)]
pub(crate) mod tests;
