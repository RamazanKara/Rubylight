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
/// Whether the device's own virtual display layout was set by the user (in the
/// device editor) rather than learned from a stream: it names a layout and
/// carries no [`LEARNED_OVER`] marker.
fn has_user_layout(client: &butterpollo_core::state::Client) -> bool {
    client
        .extra
        .get("virtual_display_layout")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|s| !s.is_empty() && s != "global")
        && !client.extra.contains_key(LEARNED_OVER)
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
}
impl ModeReportState {
    fn report(&self) {
        match &self.last {
            Some(Ok(actual)) => {
                self.warnings.clear("display_verify");
                butterpollo_core::display_policy::report_mode(
                    &self.warnings,
                    self.requested,
                    *actual,
                    self.stream_rate,
                );
            }
            Some(Err(error)) => self.warnings.set("display_verify", format!("Could not verify the applied display mode ({error}); refresh and HDR may differ from the request. Check Windows display settings and reconnect.")),
            None => {}
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
        })))
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
    /// The launch's policy, or the stream request's once `retime` raised it.
    framegen: Mutex<Policy>,
    mode_report: ModeReport,
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
            framegen: Mutex::new(framegen),
            mode_report,
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
/// shows. Returns the scale the display is in force at, for recovery to put back.
fn apply_virtual_scale(
    output: &str,
    configured: i64,
    remembered: Option<u32>,
    (width, height): (u32, u32),
    set: impl FnOnce(&str, i64, u32, u32) -> Result<()>,
) -> Result<i64> {
    match (configured, remembered) {
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
fn set_up_scale(
    output: &str,
    configured: i64,
    remembered: Option<u32>,
    dimensions: (u32, u32),
    learns: bool,
    set: impl FnOnce(&str, i64, u32, u32) -> Result<()>,
    read: impl FnOnce(&str) -> Result<u32>,
) -> Result<(i64, Option<u32>)> {
    let in_force = apply_virtual_scale(output, configured, remembered, dimensions, set)?;
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

impl Prepared {
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
            return self.display.as_mut().map_or(Ok(()), Guard::keep_alive);
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
        let app = h
            .apps
            .read()
            .unwrap()
            .iter()
            .find(|a| a.id() == launch.app_id || a.aliases.contains(&launch.app_id))
            .cloned();
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
            butterpollo_windows::display::windows_11(),
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
        } else if display_request.requested() && !virtual_mode {
            launch.warnings.set("display_virtual", "Using a physical display because the virtual display driver is unavailable. The desktop is visible locally and its refresh can limit fresh frames; repair the virtual display driver or explicitly select the physical display.");
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
        if request.prefer_highest && !virtual_mode {
            request.refresh =
                Some(butterpollo_windows::display::highest_refresh(output, request.resolution)?.0);
        }
        let (width, height) = if virtual_mode {
            request.resolution.unwrap_or((render_width, render_height))
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
        let display = if retained.is_none() {
            Some(Guard::new_virtual_options(
                output,
                virtual_mode,
                stable_id,
                width,
                height,
                rate,
                request.hdr,
                request.resolution,
                request.refresh.map(Rate),
                &virtual_options,
            )?)
        } else {
            None
        };
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
        let configured_scale = config.integer("dd_virtual_display_scale", 0);
        let scale = virtual_display_scale(virtual_layout, configured_scale);
        // Without a configured scale, the one the device kept from Windows.
        let remembered = kept_scale(remember, &launch.client.extra);
        // A configured scale is the user's setting, so nothing is learned then.
        let (scale, scale_baseline) = set_up_scale(
            &output,
            scale,
            remembered,
            (width, height),
            remember && display.is_some() && configured_scale == 0,
            butterpollo_windows::display::virtual_scale,
            butterpollo_windows::display::output_scale,
        )?;
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
            (request.resolution, request.refresh, request.hdr)
        };
        let mode_report = ModeReport::new(launch.warnings.clone(), requested, stream.fps_millihz());
        mode_report.publish(applied_mode(&output));
        let recovery_profile = if stream.hdr {
            launch
                .client
                .extra
                .get("hdr_profile")
                .and_then(serde_json::Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        } else {
            None
        };
        let profile = recovery_profile
            .as_ref()
            .map(|p| hdr_profile::Lease::acquire(&output, p))
            .transpose()?;
        // A remote monitor's display has its own generation; see `feed`.
        let revision = retained.as_ref().map_or(0, |r| r.capture_target().1);
        let owns_display = display.is_some();
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
    let app = h
        .apps
        .read()
        .unwrap()
        .iter()
        .find(|a| a.id() == launch.app_id || a.aliases.contains(&launch.app_id))
        .cloned();
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
pub(crate) mod tests {
    use super::*;
    use std::cell::Cell;
    use std::sync::mpsc;

    #[test]
    fn the_configured_scale_reaches_every_stream_display_including_remote_monitors() {
        let mut seen = None;
        let display = r"\\.\DISPLAY7";
        scale_display(
            display,
            125,
            (2560, 1440),
            |output, scale, width, height| {
                seen = Some((output.to_owned(), scale, width, height));
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(seen, Some((display.to_owned(), 125, 2560, 1440)));
        // A display that cannot be scaled is a loud failure, not a silent skip.
        assert!(scale_display("x", 125, (1, 1), |_, _, _, _| anyhow::bail!("no")).is_err());
        // A physical display keeps the user's own scale; a retained remote monitor
        // and a virtual display take the configured one.
        assert_eq!(virtual_display_scale(false, 125), 0);
        assert_eq!(virtual_display_scale(true, 125), 125);
    }

    /// A game display kept between streams, with no other holder, for tests
    /// of the session state and the grace period.
    pub(crate) fn retained_display(host: &Shared, config: &Config) -> Arc<Ready> {
        let prepared = test_prepared(host);
        let limiter = limiter::Lease::acquire(&host.directory, config, &prepared.framegen).unwrap();
        Ready::prepare((prepared, limiter)).unwrap().display.clone()
    }

    fn test_prepared(host: &Shared) -> Prepared {
        let framegen = Policy::resolve(
            &Config::default(),
            Rate(60_000),
            false,
            "none",
            false,
            false,
        )
        .unwrap();
        Prepared {
            display: None,
            output: String::new(),
            framegen,
            _arrangement: None,
            _activation: None,
            _profile: None,
            _retained: None,
            _vulkan: None,
            _golden: None,
            mode: stream_mode(&Negotiated::default()),
            client_display: None,
            virtual_layout: Some("extended".into()),
            layout_owner: None,
            inherited_layout: Some("extended".into()),
            resolution_owner: None,
            scale_baseline: None,
            layout_snapshot: None,
            size_snapshot: None,
            scale_snapshot: None,
            scales_set: Mutex::new(Vec::new()),
            revision: 0,
            recovery: Recovery::new(),
            layout_watch: Default::default(),
            mode_report: ModeReport::new(Arc::new(Warnings::default()), (None, None, None), 60_000),
            recovery_scale: 0,
            recovery_dimensions: (640, 480),
            recovery_profile: None,
            host: Arc::downgrade(host),
        }
    }

    #[test]
    fn layout_preferences_are_saved_at_disconnect_and_not_relearned_while_paused() {
        let fixture = crate::state::test_support::Fixture::new();
        let client = fixture.client(u32::MAX);
        let config = Config::parse("dd_config_revert_delay=0").unwrap();
        *fixture.host.config.write().unwrap() = config.clone();
        let mut prepared = test_prepared(&fixture.host);
        prepared.output = r"\\.\DISPLAY9".into();
        prepared.layout_owner = Some(client.uuid.clone());
        prepared.layout_snapshot = Some(vec![(prepared.output.clone(), true)]);
        let limiter =
            limiter::Lease::acquire(&fixture.host.directory, &config, &prepared.framegen).unwrap();
        let first = Ready::prepare((prepared, limiter)).unwrap();
        let display = first.display.clone();
        let second = display
            .clone()
            .resume(
                &fixture.host.directory,
                &config,
                Arc::new(Warnings::default()),
            )
            .unwrap();
        let chosen = || {
            fixture.host.paired.read().unwrap().clients[0]
                .extra
                .get("virtual_display_layout")
                .cloned()
        };
        drop(first);
        assert_eq!(chosen(), None);
        drop(second);
        assert_eq!(chosen(), Some(serde_json::json!("exclusive")));
        assert_eq!(
            fixture.host.paired.read().unwrap().clients[0]
                .extra
                .get(LEARNED_OVER),
            Some(&serde_json::json!("extended")),
            "the learned layout records the layout it replaced"
        );
        // A later local layout differs, but the paused reaper must not save it.
        display._state.lock().unwrap().layout_snapshot = Some(vec![
            (r"\\.\DISPLAY9".into(), true),
            (r"\\.\DISPLAY1".into(), false),
        ]);
        fixture
            .host
            .app_display
            .lock()
            .unwrap()
            .insert(client.uuid, (display, None));
        fixture.host.reap_paused_display();
        assert!(fixture.host.app_display.lock().unwrap().is_empty());
        assert_eq!(chosen(), Some(serde_json::json!("exclusive")));
    }

    #[test]
    fn a_resolution_chosen_during_the_stream_is_kept_for_the_device_at_disconnect() {
        let fixture = crate::state::test_support::Fixture::new();
        let client = fixture.client(u32::MAX);
        let config = Config::parse("dd_config_revert_delay=0").unwrap();
        *fixture.host.config.write().unwrap() = config.clone();
        let kept = || {
            let paired = fixture.host.paired.read().unwrap();
            let extra = &paired.clients[0].extra;
            assert_eq!(
                extra.get("display_mode"),
                None,
                "the refresh is never pinned"
            );
            extra.get(REMEMBERED_RESOLUTION).cloned()
        };
        let uhd = (3840, 2160);
        let qhd = (2560, 1440);
        for (owner, shown, expected) in [
            // Not opted in.
            (false, qhd, None),
            // Untouched: a refresh Windows lowered (VRR's 1000 Hz) is not a choice.
            (true, uhd, None),
            // Issue #36: 1440p picked in Windows under a 4K stream.
            (true, qhd, Some(serde_json::json!("2560x1440"))),
        ] {
            let mut prepared = test_prepared(&fixture.host);
            prepared.recovery_dimensions = uhd;
            prepared.resolution_owner = owner.then(|| client.uuid.clone());
            prepared.size_snapshot = Some((uhd, shown));
            let limiter =
                limiter::Lease::acquire(&fixture.host.directory, &config, &prepared.framegen)
                    .unwrap();
            let stream = Ready::prepare((prepared, limiter)).unwrap();
            let display = stream.display.clone();
            drop(stream);
            assert_eq!(kept(), expected);
            let dimensions = display._state.lock().unwrap().recovery_dimensions;
            assert_eq!(dimensions, if expected.is_some() { qhd } else { uhd });
        }
    }

    #[test]
    fn a_scale_chosen_during_the_stream_is_kept_for_the_device_at_disconnect() {
        let fixture = crate::state::test_support::Fixture::new();
        let client = fixture.client(u32::MAX);
        let config = Config::parse("dd_config_revert_delay=0").unwrap();
        *fixture.host.config.write().unwrap() = config.clone();
        let kept = || {
            let paired = fixture.host.paired.read().unwrap();
            let extra = &paired.clients[0].extra;
            assert_eq!(
                extra.get("display_mode"),
                None,
                "the refresh is never pinned"
            );
            extra.get(REMEMBERED_SCALE).cloned()
        };
        for (owner, baseline, shown, expected) in [
            // Not opted in: nothing is stored.
            (false, Some(100), 200, None),
            // Untouched.
            (true, Some(100), 100, None),
            // No baseline: a configured scale, or one Windows did not let us read.
            (true, None, 200, None),
            // Issue #36: 200% picked in Windows.
            (true, Some(100), 200, Some(serde_json::json!(200))),
        ] {
            let mut prepared = test_prepared(&fixture.host);
            prepared.resolution_owner = owner.then(|| client.uuid.clone());
            prepared.scale_baseline = baseline;
            prepared.scale_snapshot = Some(shown);
            let limiter =
                limiter::Lease::acquire(&fixture.host.directory, &config, &prepared.framegen)
                    .unwrap();
            let stream = Ready::prepare((prepared, limiter)).unwrap();
            let display = stream.display.clone();
            drop(stream);
            assert_eq!(kept(), expected);
            let recovery = display._state.lock().unwrap().recovery_scale;
            assert_eq!(recovery, if expected.is_some() { 200 } else { 0 });
        }
    }

    #[test]
    fn a_scale_windows_refused_is_a_warning_and_is_not_kept() {
        let fixture = crate::state::test_support::Fixture::new();
        let client = fixture.client(u32::MAX);
        let config = Config::parse("dd_config_revert_delay=0").unwrap();
        *fixture.host.config.write().unwrap() = config.clone();
        fixture.host.paired.write().unwrap().clients[0]
            .extra
            .insert(REMEMBERED_SCALE.into(), serde_json::json!(150));
        // Windows refuses the kept 150% and the display stays at 100%. The refusal does
        // not fail the stream, and the baseline is what Windows shows, not the refused 150.
        let (in_force, baseline) = set_up_scale(
            "out",
            0,
            Some(150),
            (2560, 1440),
            true,
            |_, _, _, _| anyhow::bail!("requested DPI scale is outside this display's range"),
            |_| Ok(100),
        )
        .unwrap();
        assert_eq!(in_force, 0, "recovery does not retry a refused scale");
        assert_eq!(baseline, Some(100));
        let mut prepared = test_prepared(&fixture.host);
        prepared.resolution_owner = Some(client.uuid.clone());
        prepared.scale_baseline = baseline;
        prepared.scale_snapshot = Some(100);
        let limiter =
            limiter::Lease::acquire(&fixture.host.directory, &config, &prepared.framegen).unwrap();
        drop(Ready::prepare((prepared, limiter)).unwrap());
        assert_eq!(
            fixture.host.paired.read().unwrap().clients[0]
                .extra
                .get(REMEMBERED_SCALE),
            Some(&serde_json::json!(150)),
            "the 100% Windows shows is not saved over the kept scale"
        );
    }

    #[test]
    fn the_scale_baseline_is_read_after_the_kept_scale_is_applied() {
        let log = std::cell::RefCell::new(Vec::new());
        let (in_force, baseline) = set_up_scale(
            "out",
            0,
            Some(200),
            (2560, 1440),
            true,
            |_, percent, _, _| {
                log.borrow_mut().push(format!("set {percent}"));
                Ok(())
            },
            |_| {
                log.borrow_mut().push("read".to_owned());
                Ok(200)
            },
        )
        .unwrap();
        assert_eq!(*log.borrow(), ["set 200", "read"]);
        assert_eq!((in_force, baseline), (200, Some(200)));
        // Nothing is read when a scale is configured or the device keeps none.
        let (_, baseline) = set_up_scale(
            "out",
            150,
            Some(200),
            (2560, 1440),
            false,
            |_, _, _, _| Ok(()),
            |_| panic!("the scale is not read"),
        )
        .unwrap();
        assert_eq!(baseline, None);
    }

    #[test]
    fn resuming_puts_the_scale_back_that_a_pause_may_have_changed() {
        let fixture = crate::state::test_support::Fixture::new();
        let config = Config::parse("dd_config_revert_delay=0").unwrap();
        *fixture.host.config.write().unwrap() = config.clone();
        for (baseline, recovery_scale, expected) in [
            // A device keeping the scale: the baseline, so a change at the PC while paused
            // is not learned at the next disconnect.
            (Some(200), 0, vec![200]),
            // A configured scale.
            (None, 150, vec![150]),
            // Nothing to put back.
            (None, 0, vec![]),
        ] {
            let mut prepared = test_prepared(&fixture.host);
            prepared.scale_baseline = baseline;
            prepared.recovery_scale = recovery_scale;
            let limiter =
                limiter::Lease::acquire(&fixture.host.directory, &config, &prepared.framegen)
                    .unwrap();
            let display = Ready::prepare((prepared, limiter)).unwrap().display.clone();
            let resumed = display
                .clone()
                .resume(
                    &fixture.host.directory,
                    &config,
                    Arc::new(Warnings::default()),
                )
                .unwrap();
            assert_eq!(
                *display._state.lock().unwrap().scales_set.lock().unwrap(),
                expected
            );
            drop(resumed);
        }
    }

    #[test]
    fn a_kept_scale_is_applied_unless_a_scale_is_configured() {
        let mut extra = std::collections::BTreeMap::new();
        extra.insert(REMEMBERED_SCALE.to_owned(), serde_json::json!(200));
        let applied = |configured: i64, remembered: Option<u32>| {
            let mut calls = Vec::new();
            let in_force = apply_virtual_scale(
                r"\\.\DISPLAY9",
                configured,
                remembered,
                (2560, 1440),
                |output, percent, width, height| {
                    calls.push((output.to_owned(), percent, width, height));
                    Ok(())
                },
            )
            .unwrap();
            (calls, in_force)
        };
        let call = |percent| vec![(r"\\.\DISPLAY9".to_owned(), percent, 2560, 1440)];
        // The kept scale is applied at the display's mode, which is the kept resolution.
        assert_eq!(applied(0, kept_scale(true, &extra)), (call(200), 200));
        // A configured scale, a percentage or recommended, wins and nothing is learned over it.
        assert_eq!(applied(150, kept_scale(true, &extra)), (call(150), 150));
        assert_eq!(applied(-1, kept_scale(true, &extra)), (call(-1), -1));
        // Nothing kept: the unconfigured scale 0 leaves Windows' own.
        assert_eq!(
            applied(0, kept_scale(true, &Default::default())),
            (call(0), 0)
        );
        // The option off: a scale left over is not applied.
        assert_eq!(kept_scale(false, &extra), None);
        assert_eq!(applied(0, kept_scale(false, &extra)), (call(0), 0));
        // Anything but a positive percentage is not a scale.
        for bad in [
            serde_json::json!("200"),
            serde_json::json!(0),
            serde_json::json!(-5),
        ] {
            extra.insert(REMEMBERED_SCALE.to_owned(), bad);
            assert_eq!(kept_scale(true, &extra), None);
        }
    }

    #[test]
    fn disconnect_reaping_waits_for_active_owners_and_releases_exclusive_layouts() {
        for (settings, exclusive, released) in [
            ("dd_config_revert_delay=0", false, true),
            (
                "dd_config_revert_on_disconnect=false\ndd_paused_virtual_display_timeout_secs=0\ndd_config_revert_delay=0",
                false,
                false,
            ),
            (
                "dd_config_revert_on_disconnect=false\ndd_paused_virtual_display_timeout_secs=0\ndd_config_revert_delay=0",
                true,
                true,
            ),
        ] {
            let fixture = crate::state::test_support::Fixture::new();
            let config = Config::parse(settings).unwrap();
            *fixture.host.config.write().unwrap() = config.clone();
            let prepared = test_prepared(&fixture.host);
            let limiter =
                limiter::Lease::acquire(&fixture.host.directory, &config, &prepared.framegen)
                    .unwrap();
            let mut stream = Ready::prepare((prepared, limiter)).unwrap();
            // Supply the arrangement result without touching Windows displays.
            Arc::get_mut(&mut stream.display).unwrap().exclusive_layout = exclusive;
            fixture
                .host
                .app_display
                .lock()
                .unwrap()
                .insert("client".into(), (stream.display.clone(), None));
            fixture.host.reap_paused_display();
            assert_eq!(fixture.host.app_display.lock().unwrap().len(), 1);
            drop(stream);
            fixture.host.reap_paused_display();
            assert_eq!(
                fixture.host.app_display.lock().unwrap().is_empty(),
                released,
                "{settings}, exclusive={exclusive}"
            );
        }
    }

    #[test]
    fn a_retained_display_stops_enforcing_layout_after_the_last_preparation_drops() {
        let fixture = crate::state::test_support::Fixture::new();
        let config = Config::default();
        let prepared = test_prepared(&fixture.host);
        let limiter =
            limiter::Lease::acquire(&fixture.host.directory, &config, &prepared.framegen).unwrap();
        let first = Ready::prepare((prepared, limiter)).unwrap();
        let display = first.display.clone();
        let mut changed = Config::parse("virtual_display_layout=extended").unwrap();
        let mut client = fixture.client(u32::MAX);
        let mut app: butterpollo_core::state::App =
            serde_json::from_value(serde_json::json!({"name":"Game"})).unwrap();
        assert!(display.serves(
            None,
            virtual_display_layout(&changed, &client, Some(&app), None)
        ));
        for layout in ["exclusive", "extended_primary"] {
            for source in ["host", "device", "app"] {
                match source {
                    "host" => {
                        changed
                            .values
                            .insert("virtual_display_layout".into(), layout.into());
                    }
                    "device" => {
                        client
                            .extra
                            .insert("virtual_display_layout".into(), layout.into());
                    }
                    _ => {
                        app.extra
                            .insert("virtual-display-layout".into(), layout.into());
                    }
                }
                assert!(
                    !display.serves(
                        None,
                        virtual_display_layout(&changed, &client, Some(&app), None)
                    ),
                    "{source}: {layout}"
                );
                changed
                    .values
                    .insert("virtual_display_layout".into(), "extended".into());
                client.extra.remove("virtual_display_layout");
                app.extra.remove("virtual-display-layout");
            }
        }
        assert_eq!(display.active.load(Ordering::Acquire), 1);
        let second = display
            .clone()
            .resume(
                &fixture.host.directory,
                &config,
                Arc::new(Warnings::default()),
            )
            .unwrap();
        assert_eq!(display.active.load(Ordering::Acquire), 2);
        drop(first);
        assert_eq!(display.active.load(Ordering::Acquire), 1);
        drop(second);
        assert_eq!(display.active.load(Ordering::Acquire), 0);
        let mut paused = display._state.lock().unwrap();
        paused.recovery.start(std::time::Instant::now());
        paused.feed(false).unwrap();
        assert!(paused.recovery.pending);
        drop(paused);
        drop(display);
    }

    #[test]
    #[ignore = "creates a virtual display, disables it while paused, and resumes it"]
    fn native_paused_game_leaves_a_disabled_display_off_until_resume() -> Result<()> {
        use butterpollo_windows::display::{Snapshot, Topology, VirtualOptions, monitors};

        let before = Snapshot::capture()?;
        let result = (|| -> Result<()> {
            let fixture = crate::state::test_support::Fixture::new();
            let config = Config::default();
            let guard = Guard::new_virtual_options(
                "",
                true,
                &format!("paused-game-test-{}", std::process::id()),
                1280,
                720,
                Rate(60_000),
                None,
                None,
                None,
                &VirtualOptions::default(),
            )?;
            let target = monitors()?
                .into_iter()
                .find(|monitor| monitor.matches(&guard.output))
                .context("test display unavailable")?;
            let mut prepared = test_prepared(&fixture.host);
            prepared.output = guard.output.clone();
            prepared.display = Some(guard);
            let limiter =
                limiter::Lease::acquire(&fixture.host.directory, &config, &prepared.framegen)?;
            let stream = Ready::prepare((prepared, limiter))?;
            let display = stream.display.clone();
            drop(stream);
            anyhow::ensure!(display.active.load(Ordering::Acquire) == 0);

            // The same operation as a local layout that excludes the virtual display.
            let kept: Vec<_> = monitors()?
                .into_iter()
                .filter(|monitor| monitor.device_id != target.device_id)
                .map(|monitor| monitor.device_id)
                .collect();
            anyhow::ensure!(!kept.is_empty(), "test needs another active display");
            Topology::set_active(&kept)?;
            let physical = Snapshot::capture()?;
            // Cross the driver's 10 s watchdog and its 3 s polling ticks.
            let deadline = std::time::Instant::now() + Duration::from_secs(16);
            while std::time::Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(250));
                anyhow::ensure!(
                    !monitors()?.iter().any(|m| m.device_id == target.device_id),
                    "paused game reactivated the display the user switched off"
                );
            }
            anyhow::ensure!(
                Topology::query_all()?
                    .monitors()
                    .iter()
                    .any(|m| m.device_id == target.device_id),
                "paused game's display lease expired"
            );
            anyhow::ensure!(
                serde_json::to_value(Snapshot::capture()?)? == serde_json::to_value(physical)?,
                "paused renewal changed the physical displays"
            );

            let resumed = display.clone().resume(
                &fixture.host.directory,
                &config,
                Arc::new(Warnings::default()),
            )?;
            let (output, generation) = display.capture_target();
            anyhow::ensure!(
                monitors()?
                    .iter()
                    .any(|m| m.device_id == target.device_id && m.matches(&output)),
                "resume returned before publishing the active capture target"
            );
            anyhow::ensure!(generation > 0, "resume did not publish the new generation");
            drop(resumed);
            drop(display);
            Ok(())
        })();
        before.restore()?;
        result
    }

    #[test]
    fn explicit_virtual_layouts_take_precedence_over_the_legacy_primary_flag() {
        use butterpollo_core::display_policy::Arrangement;
        let config = Config::default();
        let mut client: butterpollo_core::state::Client =
            serde_json::from_value(serde_json::json!({"name":"Device","uuid":"device","cert":""}))
                .unwrap();
        let mut app: butterpollo_core::state::App = serde_json::from_value(
            serde_json::json!({"name":"Game","virtual-display-primary":true}),
        )
        .unwrap();
        assert_eq!(
            virtual_display_layout(&config, &client, None, None),
            "exclusive"
        );
        assert_eq!(
            virtual_display_layout(&config, &client, Some(&app), None),
            "extended_primary"
        );
        app.extra
            .insert("virtual-display-layout".into(), "exclusive".into());
        assert_eq!(
            virtual_display_layout(&config, &client, Some(&app), None),
            "exclusive"
        );
        client
            .extra
            .insert("virtual_display_layout".into(), "extended".into());
        assert_eq!(
            virtual_display_layout(&config, &client, Some(&app), None),
            "extended"
        );
        client
            .extra
            .insert("virtual_display_layout".into(), "exclusive".into());
        assert_eq!(
            virtual_display_layout(&config, &client, Some(&app), None),
            "exclusive"
        );
        assert_eq!(
            virtual_display_layout(
                &config,
                &client,
                Some(&app),
                Some(HostDisplay::Virtual(Arrangement::Primary))
            ),
            "extended_primary"
        );
        for inherited in ["", "global"] {
            client
                .extra
                .insert("virtual_display_layout".into(), inherited.into());
            app.extra
                .insert("virtual-display-layout".into(), inherited.into());
            assert_eq!(
                virtual_display_layout(&config, &client, Some(&app), None),
                "extended_primary"
            );
            app.extra
                .insert("virtual-display-primary".into(), false.into());
            assert_eq!(
                virtual_display_layout(&config, &client, Some(&app), None),
                "exclusive"
            );
            app.extra
                .insert("virtual-display-primary".into(), true.into());
        }
    }
    /// One stream to the device (`planned` layout, `owner` as the stream sets it
    /// when Windows took the layout) that ends with the stream's display in the
    /// `ended` state; `during` runs while it is connected. Returns the device's
    /// layout settings afterwards.
    fn stream_then_disconnect(
        fixture: &crate::state::test_support::Fixture,
        client: &butterpollo_core::state::Client,
        owner: bool,
        ended: Vec<(String, bool)>,
        during: impl FnOnce(),
    ) -> (Option<serde_json::Value>, Option<serde_json::Value>) {
        let config = Config::parse("dd_config_revert_delay=0").unwrap();
        *fixture.host.config.write().unwrap() = config.clone();
        let mut prepared = test_prepared(&fixture.host);
        prepared.output = r"\.\DISPLAY9".into();
        prepared.virtual_layout = Some("extended_primary".into());
        prepared.inherited_layout = Some("extended_primary".into());
        prepared.layout_owner = owner.then(|| client.uuid.clone());
        prepared.layout_snapshot = Some(ended);
        let limiter =
            limiter::Lease::acquire(&fixture.host.directory, &config, &prepared.framegen).unwrap();
        let stream = Ready::prepare((prepared, limiter)).unwrap();
        during();
        drop(stream);
        let paired = fixture.host.paired.read().unwrap();
        let extra = &paired.clients[0].extra;
        (
            extra.get("virtual_display_layout").cloned(),
            extra.get(LEARNED_OVER).cloned(),
        )
    }
    /// The stream's display is on beside the PC's, and no longer the primary.
    fn windows_moved_the_primary() -> Vec<(String, bool)> {
        vec![
            (r"\.\DISPLAY9".into(), false),
            (r"\.\DISPLAY1".into(), true),
        ]
    }
    fn set_device_layout(fixture: &crate::state::test_support::Fixture, layout: &str) {
        let mut paired = fixture.host.paired.write().unwrap();
        paired.clients[0]
            .extra
            .insert("virtual_display_layout".into(), layout.into());
        paired.clients[0].extra.remove(LEARNED_OVER);
    }
    #[test]
    fn a_device_layout_the_user_set_survives_every_stream() {
        // Issue #40: the layout set under Devices was replaced by "extended" when
        // a later stream ended with Windows' primary elsewhere.
        for layout in ["exclusive", "extended_primary", "isolated"] {
            let fixture = crate::state::test_support::Fixture::new();
            let client = fixture.client(u32::MAX);
            set_device_layout(&fixture, layout);
            for _stream in 0..3 {
                let (kept, over) = stream_then_disconnect(
                    &fixture,
                    &client,
                    true,
                    windows_moved_the_primary(),
                    || {},
                );
                assert_eq!(kept, Some(serde_json::json!(layout)));
                assert_eq!(over, None);
            }
        }
    }
    #[test]
    fn a_device_layout_the_user_sets_during_a_stream_is_not_replaced_at_disconnect() {
        let fixture = crate::state::test_support::Fixture::new();
        let client = fixture.client(u32::MAX);
        let (kept, over) =
            stream_then_disconnect(&fixture, &client, true, windows_moved_the_primary(), || {
                set_device_layout(&fixture, "exclusive")
            });
        assert_eq!(kept, Some(serde_json::json!("exclusive")));
        assert_eq!(over, None);
    }
    #[test]
    fn a_device_layout_the_user_set_survives_a_reconnect() {
        let fixture = crate::state::test_support::Fixture::new();
        let client = fixture.client(u32::MAX);
        set_device_layout(&fixture, "extended_primary");
        let config = Config::parse("dd_config_revert_delay=0").unwrap();
        *fixture.host.config.write().unwrap() = config.clone();
        let mut prepared = test_prepared(&fixture.host);
        prepared.output = r"\.\DISPLAY9".into();
        prepared.virtual_layout = Some("extended_primary".into());
        prepared.layout_owner = Some(client.uuid.clone());
        prepared.layout_snapshot = Some(windows_moved_the_primary());
        let limiter =
            limiter::Lease::acquire(&fixture.host.directory, &config, &prepared.framegen).unwrap();
        let first = Ready::prepare((prepared, limiter)).unwrap();
        let second = first
            .display
            .clone()
            .resume(
                &fixture.host.directory,
                &config,
                Arc::new(Warnings::default()),
            )
            .unwrap();
        drop(first);
        drop(second);
        let paired = fixture.host.paired.read().unwrap();
        let extra = &paired.clients[0].extra;
        assert_eq!(
            extra.get("virtual_display_layout"),
            Some(&serde_json::json!("extended_primary"))
        );
        assert_eq!(extra.get(LEARNED_OVER), None);
    }
    #[test]
    fn a_layout_windows_refused_is_not_learned() {
        // The stream sets no layout owner when Windows did not take the layout.
        let fixture = crate::state::test_support::Fixture::new();
        let client = fixture.client(u32::MAX);
        let (kept, over) =
            stream_then_disconnect(&fixture, &client, false, windows_moved_the_primary(), || {});
        assert_eq!((kept, over), (None, None));
    }
    #[test]
    fn a_learned_device_layout_is_replaced_but_a_chosen_one_is_not_learned_over() {
        let fixture = crate::state::test_support::Fixture::new();
        let client = fixture.client(u32::MAX);
        // No device layout: the one the user left the stream in is learned.
        let (kept, over) =
            stream_then_disconnect(&fixture, &client, true, windows_moved_the_primary(), || {});
        assert_eq!(kept, Some(serde_json::json!("extended")));
        assert_eq!(over, Some(serde_json::json!("extended_primary")));
        // "global" is no layout of its own.
        set_device_layout(&fixture, "global");
        let (kept, _) =
            stream_then_disconnect(&fixture, &client, true, windows_moved_the_primary(), || {});
        assert_eq!(kept, Some(serde_json::json!("extended")));
        // A learned layout follows what the user leaves the next stream in.
        let only = vec![(r"\.\DISPLAY9".to_string(), true)];
        let (kept, over) = stream_then_disconnect(&fixture, &client, true, only, || {});
        assert_eq!(kept, Some(serde_json::json!("exclusive")));
        assert_eq!(over, Some(serde_json::json!("extended_primary")));
    }
    #[test]
    fn a_learned_device_layout_yields_when_the_host_layout_changes() {
        // Issue #40: "extended" was learned over the host's "extended_primary";
        // switching the host to "exclusive" must take effect.
        let mut client: butterpollo_core::state::Client =
            serde_json::from_value(serde_json::json!({"name":"Device","uuid":"device","cert":""}))
                .unwrap();
        client
            .extra
            .insert("virtual_display_layout".into(), "extended".into());
        client
            .extra
            .insert(LEARNED_OVER.into(), "extended_primary".into());
        let primary = Config::parse("virtual_display_layout=extended_primary").unwrap();
        let exclusive = Config::parse("virtual_display_layout=exclusive").unwrap();
        assert_eq!(
            virtual_display_layout(&primary, &client, None, None),
            "extended"
        );
        assert_eq!(
            virtual_display_layout(&exclusive, &client, None, None),
            "exclusive"
        );
        // A layout the user set for the device has no marker and always applies.
        client.extra.remove(LEARNED_OVER);
        assert_eq!(
            virtual_display_layout(&exclusive, &client, None, None),
            "extended"
        );
    }
    #[test]
    fn a_retained_display_matches_only_the_exact_mode_it_was_made_for() {
        let made = Negotiated {
            width: 2560,
            height: 1440,
            fps: 120,
            hdr: true,
            ..Default::default()
        };
        assert_eq!(stream_mode(&made), stream_mode(&made.clone()));
        for other in [
            Negotiated {
                width: 1920,
                ..made.clone()
            },
            Negotiated {
                fps: 60,
                ..made.clone()
            },
            Negotiated {
                rate_millihz: 119_880,
                ..made.clone()
            },
            Negotiated {
                hdr: false,
                ..made.clone()
            },
            Negotiated {
                vrr_low_latency: true,
                ..made.clone()
            },
        ] {
            assert_ne!(stream_mode(&made), stream_mode(&other));
        }
    }
    #[test]
    fn only_a_stream_takes_the_clients_display_choice() {
        use butterpollo_core::display_policy::Arrangement;
        let f = crate::state::test_support::Fixture::new();
        let client = f.client(u32::MAX);
        let launch = |role: Role, value: Option<&str>| {
            let mut launch = f.launch(client.clone(), role);
            if let Some(value) = value {
                launch
                    .options
                    .insert(HostDisplay::PARAMETER.into(), value.into());
            }
            client_display(&launch)
        };
        assert_eq!(launch(Role::Stream, None), None);
        assert_eq!(
            launch(Role::Stream, Some("physical")),
            Some(HostDisplay::Physical)
        );
        assert_eq!(
            launch(Role::Stream, Some("exclusive")),
            Some(HostDisplay::Virtual(Arrangement::Exclusive))
        );
        assert_eq!(
            launch(Role::Stream, Some("extended_primary")),
            Some(HostDisplay::Virtual(Arrangement::Primary))
        );
        for unknown in ["", "default", "mirror"] {
            assert_eq!(launch(Role::Stream, Some(unknown)), None, "{unknown}");
        }
        assert_eq!(launch(Role::RemoteMonitor, Some("physical")), None);
        assert_eq!(launch(Role::InputOnly, Some("exclusive")), None);
    }
    #[test]
    fn legacy_virtual_display_zero_leaves_the_host_settings_in_control() {
        use butterpollo_core::display_policy::Arrangement;
        let f = crate::state::test_support::Fixture::new();
        let client = f.client(u32::MAX);
        let mut launch = f.launch(client, Role::Stream);
        assert_eq!(client_display(&launch), None);
        launch.options.insert("virtualDisplay".into(), "1".into());
        assert_eq!(client_display(&launch), None);
        // Apollo-compatible clients send 0 whenever their toggle is off; the
        // host's virtual display must still apply.
        launch.options.insert("virtualDisplay".into(), "0".into());
        assert_eq!(client_display(&launch), None);
        // An explicit hostDisplay=physical still selects the physical display.
        launch
            .options
            .insert(HostDisplay::PARAMETER.into(), "physical".into());
        let physical = client_display(&launch);
        assert_eq!(physical, Some(HostDisplay::Physical));
        assert!(!HostDisplay::reusable(
            Some(HostDisplay::Virtual(Arrangement::Exclusive)),
            physical
        ));
        launch
            .options
            .insert(HostDisplay::PARAMETER.into(), "extended".into());
        assert_eq!(
            client_display(&launch),
            Some(HostDisplay::Virtual(Arrangement::Extended))
        );
        for role in [Role::RemoteMonitor, Role::InputOnly] {
            launch.role = role;
            assert_eq!(client_display(&launch), None);
        }
    }
    #[test]
    fn virtual_display_mode_keeps_device_app_host_precedence_and_legacy_global_inheritance() {
        let config = Config::parse("virtual_display_mode=shared").unwrap();
        let mut client: butterpollo_core::state::Client =
            serde_json::from_value(serde_json::json!({"name":"Device","uuid":"device","cert":""}))
                .unwrap();
        let app = serde_json::from_value(
            serde_json::json!({"name":"Game","virtual-display-mode":"per_client"}),
        )
        .unwrap();
        assert_eq!(
            virtual_display_mode(&Config::default(), &client, None, false),
            "disabled"
        );
        assert_eq!(
            virtual_display_mode(&Config::default(), &client, None, true),
            "per_client"
        );
        assert_eq!(virtual_display_mode(&config, &client, None, true), "shared");
        assert_eq!(
            virtual_display_mode(&config, &client, Some(&app), true),
            "per_client"
        );
        client
            .extra
            .insert("virtual_display_mode".into(), "disabled".into());
        assert_eq!(
            virtual_display_mode(&config, &client, Some(&app), true),
            "disabled"
        );
        for inherited in ["global", ""] {
            client
                .extra
                .insert("virtual_display_mode".into(), inherited.into());
            assert_eq!(
                virtual_display_mode(&config, &client, Some(&app), true),
                "per_client"
            );
            assert_eq!(virtual_display_mode(&config, &client, None, true), "shared");
        }
    }

    #[test]
    fn background_display_ignores_shared_and_app_identity_overrides() {
        let fixture = crate::state::test_support::Fixture::new();
        let mut config =
            Config::parse("virtual_display_mode=shared\nvirtual_display_layout=exclusive").unwrap();
        let mut launch = fixture.launch(fixture.client(u32::MAX), Role::Stream);
        launch.options.insert(
            "uniqueid".into(),
            "00000000-0000-0000-0000-000000000001".into(),
        );
        let app: butterpollo_core::state::App = serde_json::from_value(serde_json::json!({
            "name":"Game", "uuid":"app-display", "use-app-identity":true,
            "virtual-display-mode":"shared", "virtual-display-layout":"exclusive"
        }))
        .unwrap();
        assert_eq!(
            virtual_display_identity(&config, &launch, Some(&app)),
            "app-display"
        );
        butterpollo_core::background::apply(&mut config);
        for mode in ["shared", "disabled", "per_client"] {
            launch
                .client
                .extra
                .insert("virtual_display_mode".into(), mode.into());
            assert_eq!(
                virtual_display_mode(&config, &launch.client, Some(&app), true),
                "per_client"
            );
            assert_eq!(
                virtual_display_identity(&config, &launch, Some(&app)),
                launch.client.uuid
            );
            assert_eq!(
                virtual_display_layout(
                    &config,
                    &launch.client,
                    Some(&app),
                    Some(HostDisplay::Physical)
                ),
                butterpollo_core::background::LAYOUT
            );
        }
    }

    #[test]
    fn a_faster_stream_request_keeps_only_a_display_that_can_follow_it() {
        // Artemis's Warp launches at 60 and asks for 120. Without a virtual
        // display of its own to raise (a physical display here), the
        // launch's preparation goes back to be prepared again at 120; one
        // made for another size never serves the stream.
        let fixture = crate::state::test_support::Fixture::new();
        let config = Config::default();
        let launched = Negotiated {
            width: 2560,
            height: 1440,
            fps: 60,
            ..Default::default()
        };
        let policy = Policy::resolve(&config, Rate(60_000), false, "none", false, false).unwrap();
        let prepared = Prepared {
            display: None,
            output: String::new(),
            framegen: policy.clone(),
            _arrangement: None,
            _activation: None,
            _profile: None,
            _retained: None,
            _vulkan: None,
            _golden: None,
            mode: stream_mode(&launched),
            client_display: None,
            virtual_layout: None,
            layout_owner: None,
            inherited_layout: None,
            resolution_owner: None,
            scale_baseline: None,
            layout_snapshot: None,
            size_snapshot: None,
            scale_snapshot: None,
            scales_set: Mutex::new(Vec::new()),
            revision: 0,
            recovery: Recovery::new(),
            layout_watch: Default::default(),
            mode_report: ModeReport::new(Arc::new(Warnings::default()), (None, None, None), 60_000),
            recovery_scale: 0,
            recovery_dimensions: (2560, 1440),
            recovery_profile: None,
            host: Arc::downgrade(&fixture.host),
        };
        let limiter = limiter::Lease::acquire(&fixture.host.directory, &config, &policy).unwrap();
        let mut preparation = Ready::prepare((prepared, limiter)).unwrap();
        let display = preparation.display.clone();
        let launch = fixture.launch(fixture.client(u32::MAX), Role::Stream);
        for request in [
            Negotiated {
                fps: 120,
                ..launched.clone()
            },
            Negotiated {
                width: 1920,
                fps: 120,
                ..launched.clone()
            },
        ] {
            assert!(!display.matches(&request));
            preparation = retime(preparation, &fixture.host, &launch, &request, &config)
                .unwrap()
                .err()
                .expect("prepared again");
            assert_eq!(display.active.load(Ordering::Acquire), 1);
            assert_eq!(display.framegen.lock().unwrap().rate, Rate(60_000));
        }
        // The launch's own rate keeps the preparation as it is.
        assert!(display.matches(&launched));
        drop(preparation);
        assert_eq!(display.active.load(Ordering::Acquire), 0);
    }

    #[test]
    fn vrr_negotiated_after_launch_requires_new_display_preparation() {
        let mut launch = Negotiated {
            fps: 116,
            ..Default::default()
        };
        let negotiated =
            Negotiated::from_sdp(b"a=x-nv-video[0].maxFPS:116\na=x-ss-video[0].vrrLowLatency:1\n")
                .unwrap();
        assert_eq!(negotiated.fps_millihz(), 116_000);
        assert_ne!(stream_mode(&launch), stream_mode(&negotiated));
        launch.vrr_low_latency = true;
        assert_eq!(stream_mode(&launch), stream_mode(&negotiated));
    }

    fn codes(warnings: &Warnings) -> Vec<String> {
        warnings.snapshot().into_iter().map(|w| w.code).collect()
    }

    #[test]
    fn a_recovered_display_mode_is_reported_on_the_card_of_the_launch_using_it() {
        let first = Arc::new(Warnings::default());
        let report = ModeReport::new(
            first.clone(),
            (Some((3840, 2160)), Some(1_000_000), Some(true)),
            116_000,
        );
        report.publish(Ok((3840, 2160, 1_000_000, true)));
        assert!(codes(&first).is_empty());
        // Windows switched the display back on at its saved 60 Hz mode and
        // refused the stream's: the card says so, the stream continues.
        report.publish(Ok((3840, 2160, 60_000, true)));
        assert_eq!(codes(&first), ["display_refresh"]);
        assert!(first.snapshot()[0].message.contains("60.000 Hz"));
        // A resumed launch sees the current finding on its own card.
        let second = Arc::new(Warnings::default());
        report.attach(second.clone());
        assert_eq!(codes(&second), ["display_refresh"]);
        report.publish(Err(anyhow::anyhow!("display unavailable")));
        assert_eq!(codes(&second), ["display_refresh", "display_verify"]);
        report.publish(Ok((3840, 2160, 1_000_000, true)));
        assert!(codes(&second).is_empty());
        // Nothing found yet: attaching reports nothing.
        let fresh = ModeReport::new(first.clone(), (None, None, None), 60_000);
        let third = Arc::new(Warnings::default());
        fresh.attach(third.clone());
        assert!(codes(&third).is_empty());
    }

    #[test]
    fn a_recovery_sets_the_mode_once_while_its_layout_is_retried() {
        let now = std::time::Instant::now();
        let mut recovery = Recovery::new();
        assert!(!recovery.due(now));
        assert!(!recovery.take_mode());
        recovery.start(now);
        assert!(recovery.due(now));
        assert!(recovery.take_mode());
        // The layout or HDR profile failed: retried a second later, but a
        // refused mode is not requested from Windows again.
        assert!(!recovery.due(now + Duration::from_millis(100)));
        assert!(recovery.due(now + Duration::from_secs(1)));
        assert!(!recovery.take_mode());
        recovery.finish();
        assert!(!recovery.due(now + Duration::from_secs(5)));
        // The next recovery sets it again.
        recovery.start(now + Duration::from_secs(6));
        assert!(recovery.due(now + Duration::from_secs(6)));
        assert!(recovery.take_mode());
    }

    #[test]
    fn the_saved_layout_is_restored_once_after_the_last_stream() {
        let mut baseline = Baseline::new();
        let journal = Cell::new(false);
        let restores = Cell::new(0);
        baseline
            .acquire(|| {
                journal.set(true);
                Ok("layout")
            })
            .unwrap();
        baseline
            .acquire(|| panic!("a second stream must join the recorded layout"))
            .unwrap();
        let restore = |layout: &str| {
            assert_eq!(layout, "layout");
            restores.set(restores.get() + 1);
            journal.set(false);
        };
        baseline.release(restore);
        assert_eq!(restores.get(), 0);
        assert!(journal.get(), "the crash journal must stay pending");
        baseline.release(restore);
        assert_eq!(restores.get(), 1);
        assert!(!journal.get());
        // A failed record leaves no user behind; the next stream records again.
        assert!(baseline.acquire(|| anyhow::bail!("journal busy")).is_err());
        baseline.acquire(|| Ok("next")).unwrap();
        baseline.release(|layout| assert_eq!(layout, "next"));
    }

    #[test]
    fn capture_target_stays_readable_during_refresh_and_publishes_identity_together() {
        let target = CaptureTarget::new(("old-output".into(), 0));
        let publisher = target.clone();
        let (entered, waiting) = mpsc::channel();
        let (release, resume) = mpsc::channel();
        let refresh = std::thread::spawn(move || {
            // Stand in for a slow driver call: publication happens only after
            // maintenance returns, while current capture must remain usable.
            entered.send(()).unwrap();
            resume.recv().unwrap();
            publisher.publish(("new-output".into(), 1));
        });
        waiting.recv_timeout(Duration::from_secs(1)).unwrap();
        let reader = target.clone();
        let (read, result) = mpsc::channel();
        let read_thread = std::thread::spawn(move || read.send(reader.current()).unwrap());
        let current = result.recv_timeout(Duration::from_millis(100));
        release.send(()).unwrap();
        refresh.join().unwrap();
        read_thread.join().unwrap();
        assert_eq!(current.unwrap(), ("old-output".into(), 0));
        assert_eq!(target.current(), ("new-output".into(), 1));

        let publisher = target.clone();
        let refresh = std::thread::spawn(move || {
            for generation in 2..10000 {
                publisher.publish((format!("output-{generation}"), generation));
            }
        });
        while !refresh.is_finished() {
            let (output, generation) = target.current();
            if generation > 1 {
                assert_eq!(output, format!("output-{generation}"));
            }
        }
        refresh.join().unwrap();
        assert_eq!(target.current(), ("output-9999".into(), 9999));
    }
}
