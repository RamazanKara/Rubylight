//! Virtual displays from Rubylight's display driver (`rubylight_vdd`): creation, modes and
//! heartbeats.

use super::modes::{
    ModeOutcome, Timing, current_timing, fallback_rate, format_timing, lists_rate, mode_outcome,
    offered_rates, supported_modes,
};
use super::rubylight_vdd::{self, Control};
use super::topology::{Route, activate_target, is_rubylight_monitor, is_virtual_monitor};
use super::*;
use butterpollo_core::virtual_output::{DisplayProvider, Mode};

/// How long a create retries while a previous host's display with the same identity departs.
const CREATE_RETRY: Duration = Duration::from_secs(10);
/// How long Windows has to list a mode the driver was asked to offer on a live display.
const MODE_LIST_WAIT: Duration = Duration::from_secs(2);
/// The Windows build number from the registry, or None when it cannot be read
/// (callers then treat the build as capable; see `butterpollo_core::os_gate`).
pub fn windows_build() -> Option<u32> {
    static BUILD: std::sync::OnceLock<Option<u32>> = std::sync::OnceLock::new();
    *BUILD
        // SAFETY: The registry names are terminated and the output size matches the live UTF-16 buffer.
        .get_or_init(|| unsafe {
            use windows::Win32::System::Registry::*;
            let mut text = [0u16; 32];
            let mut size = (text.len() * 2) as u32;
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                windows::core::w!("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion"),
                windows::core::w!("CurrentBuildNumber"),
                RRF_RT_REG_SZ,
                None,
                Some(text.as_mut_ptr().cast()),
                Some(&mut size),
            )
            .ok()
            .ok()?;
            wide(&text).trim().parse().ok()
        })
}
/// Whether this Windows ships the IddCx the virtual display driver requires
/// (1.10, Windows 11 23H2 and later). Below that the host defaults to the
/// physical display, as Vibepollo does on Windows 10, because the driver
/// cannot bind. The first call logs why, once.
pub fn virtual_display_os() -> bool {
    use butterpollo_core::os_gate::{Feature, missing, virtual_display};
    static LOGGED: std::sync::Once = std::sync::Once::new();
    let build = windows_build();
    if let Some(reason) = missing(build, Feature::VirtualDisplay) {
        LOGGED.call_once(|| tracing::info!("{reason}"));
    }
    virtual_display(build)
}
pub fn virtual_display_available() -> bool {
    Control::open().is_ok()
}
/// Whether the driver counts as there for the service's repair watch: its
/// interface opens, or its device is installed and switched off, which the user
/// may have done on purpose.
pub fn virtual_display_installed() -> bool {
    virtual_display_available() || super::driver_recovery::disabled()
}
/// Launch/resume may repair a confirmed failed devnode before choosing its display.
/// Console and server-info inspection must keep using the read-only availability query.
pub fn ensure_virtual_display_available() -> bool {
    Control::open_recovering().is_ok()
}
pub fn virtual_display_status() -> serde_json::Value {
    match Control::open() {
        Ok(control) => serde_json::json!({
            "capable": true,
            "ready": true,
            "reason": "",
            "protocol": "rubylight",
            "driver_version": format!(
                "{}.{}.{}",
                control.info.driver_version >> 16,
                (control.info.driver_version >> 8) & 0xff,
                control.info.driver_version & 0xff
            ),
        }),
        Err(error) => {
            serde_json::json!({"capable":false,"ready":false,"reason":driver_problem(&error),"protocol":"rubylight"})
        }
    }
}
/// What the console says when the driver cannot be opened.
fn driver_problem(error: &anyhow::Error) -> String {
    let recorded = crate::steam::registry_string(
        windows::Win32::System::Registry::HKEY_LOCAL_MACHINE,
        "SOFTWARE\\Rubylight\\VirtualDisplay",
        "Problem",
    );
    problem_text(error, recorded)
}
/// Why the driver is missing as setup recorded it (a Smart App Control, S mode or App Control
/// policy refusing the driver signed on the PC, or a failed install) comes first: it says what
/// the user can do. Otherwise the error explains it.
fn problem_text(error: &anyhow::Error, recorded: Option<String>) -> String {
    use windows::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_NO_MORE_ITEMS};
    if let Some(recorded) = recorded.filter(|text| !text.trim().is_empty()) {
        return recorded;
    }
    let code = error
        .downcast_ref::<windows::core::Error>()
        .map(|e| e.code());
    if code == Some(ERROR_NO_MORE_ITEMS.to_hresult()) {
        "Rubylight's display driver is not installed, or Windows must restart to finish installing it. The Rubylight service sets it up again a minute after it finds it missing, unless a stream is running; to do it now, run \"butterpollo-setup.exe --repair-drivers --now\" from the Rubylight folder in an administrator shell.".into()
    } else if code == Some(ERROR_ACCESS_DENIED.to_hresult()) {
        "Rubylight's display driver refused this process. Install Rubylight with its setup and let the service run the host.".into()
    } else {
        format!("{error:#}")
    }
}
#[derive(Clone, Debug)]
pub struct VirtualOptions {
    /// The peak brightness the display's EDID advertises for HDR.
    pub peak_nits: u32,
    /// The GPU to render the display on; None leaves the choice to Windows.
    pub render_adapter: Option<RenderAdapter>,
}
impl Default for VirtualOptions {
    fn default() -> Self {
        Self {
            peak_nits: 1000,
            render_adapter: None,
        }
    }
}
/// The GPU that renders the driver's displays: IddCx's preferred render
/// adapter. Windows otherwise picks one, on a hybrid PC often the integrated
/// GPU, and only that GPU can then duplicate the display.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderAdapter {
    pub name: String,
    pub luid: (u32, i32),
}
/// The GPU the Graphics card setting names, for virtual displays to render on
/// as Apollo and Vibepollo do. Automatic leaves the choice to Windows, except
/// on a PC with an AMD and a non-AMD GPU, where it is the AMD GPU: the stream
/// encodes only there, and the probe already counts on it.
pub fn configured_render_adapter(
    config: &butterpollo_core::config::Config,
) -> Option<RenderAdapter> {
    let (name, pnp_id) = crate::capture::stream_adapter(
        None,
        config.get("adapter_name", ""),
        config.get("adapter_pnp_id", ""),
    );
    let (name, pnp_id) = (name.as_str(), pnp_id.as_str());
    if name.is_empty() && pnp_id.is_empty() {
        return None;
    }
    let selected = crate::capture::gpus().and_then(|gpus| {
        Ok(
            crate::capture::select_gpu(&gpus, name, pnp_id)?.map(|gpu| RenderAdapter {
                name: gpu.name.clone(),
                luid: gpu.luid,
            }),
        )
    });
    selected
        .inspect_err(|error| {
        tracing::warn!(error = %format!("{error:#}"), "the selected GPU was not found; Windows picks the GPU that renders the virtual display")
    })
    .ok()
    .flatten()
}
/// Best effort: a driver that refuses leaves the display on the GPU Windows picks, and
/// capture then says it encodes there.
fn apply_render_adapter(control: &Control, options: &VirtualOptions) {
    let Some(adapter) = &options.render_adapter else {
        return;
    };
    match retry_busy(
        || control.set_render_adapter(adapter.luid),
        Duration::from_secs(1),
    ) {
        Ok(()) => {
            tracing::info!(gpu = %adapter.name, "virtual display renders on the selected GPU")
        }
        Err(error) => {
            tracing::warn!(gpu = %adapter.name, error = %format!("{error:#}"), "the display driver did not accept the selected GPU; Windows picks the GPU that renders the virtual display")
        }
    }
}
/// A monitor belongs to this process and departs when it stops sending heartbeats; the
/// display id is the driver's session id.
impl DisplayProvider for Control {
    fn renew(&self, id: u64) -> Result<()> {
        self.drain_log();
        self.heartbeat(id)
    }
    fn offer_mode(&self, id: u64, mode: Mode) {
        let mode = (mode.width, mode.height, mode.refresh_mhz);
        if let Err(error) = self.update_modes(id, mode) {
            tracing::warn!(error = %format!("{error:#}"), mode = %format_timing(mode), "the display driver did not offer the mode");
        }
    }
    fn remove(&self, id: u64) {
        if let Err(e) = lease_exists(retry_busy(
            || Control::remove(self, id),
            Duration::from_secs(1),
        )) {
            tracing::warn!(error=%e,"virtual display removal failed; the driver departs it when this host stops");
        }
        rubylight_vdd::unregister(id);
        self.drain_log();
    }
    fn pushes_frames(&self) -> bool {
        true
    }
}
pub struct VirtualDisplay {
    driver: Control,
    pub name: String,
    id: u64,
    /// Recovery checks and renewal have separate clocks: a paused owner of a
    /// shared display must not postpone another owner's active recovery.
    pub(super) last_feed: Instant,
    last_renew: Instant,
    mode: (u32, u32, u32),
    /// The mode the display was created with. `mode` follows the rate,
    /// rotation and size changes made in place, so a second stream that asks
    /// for the first one's launch size still joins ([`join`]).
    created: (u32, u32, u32),
    /// Windows showed `mode`'s resolution after it was last set, at that or a
    /// slower refresh it accepted.
    size_applied: std::sync::atomic::AtomicBool,
    options: VirtualOptions,
    pub(super) generation: u64,
    startup_protection: Option<hotplug::Protection>,
    pub(super) resolved_target: Option<Monitor>,
    /// The adapter and target the driver reported. Windows lists the display
    /// there while it is off, and may list it elsewhere while it is on.
    driver_route: Option<Route>,
    /// Windows left this display off and the host switched it on.
    pub switched_on: bool,
}
pub(super) type DisplayLease = std::sync::Arc<std::sync::Mutex<VirtualDisplay>>;
/// The displays streams lease, by id; a shared display has one entry for all.
type Leases<D> = std::collections::BTreeMap<String, std::sync::Weak<std::sync::Mutex<D>>>;
static DISPLAYS: std::sync::Mutex<Leases<VirtualDisplay>> =
    std::sync::Mutex::new(std::collections::BTreeMap::new());
/// The display `id` for a stream at this mode, and the refresh (millihertz)
/// it still has to be raised to when the stream joins a slower shared one:
/// [`joined_monitor`] does that once the display is on.
pub(super) fn display_lease(
    id: &str,
    width: u32,
    height: u32,
    fps: u32,
    options: &VirtualOptions,
) -> Result<(DisplayLease, Option<u32>)> {
    lease(
        &mut DISPLAYS.lock().unwrap(),
        id,
        (width, height, fps),
        || VirtualDisplay::create_options(id, width, height, fps, options.clone()),
    )
}
/// What leasing, joining and raising a virtual display need of it, so that
/// tests can stand in for the driver and Windows.
pub(super) trait SharedDisplay {
    /// The mode it was created with, and the one the host sets on it now.
    fn modes(&self) -> (Timing, Timing);
    /// The mode the host sets on it from now on.
    fn set_mode(&mut self, mode: Timing);
    /// Have the driver offer that mode; false, logged, when the driver refused.
    fn offer_mode(&self) -> bool;
    /// The refreshes (whole hertz) Windows lists now at that mode's size.
    fn listed_rates(&self) -> Vec<u32>;
    /// Set that mode in Windows; whether Windows shows it.
    fn apply(&self, stage: &str) -> bool;
    /// That mode's size, when Windows showed it after it was last set.
    fn applied_size(&self) -> Option<(u32, u32)>;
    /// Its monitor, switched back on when Windows left it off.
    fn switch_on(&mut self) -> Result<Monitor>;
}
impl SharedDisplay for VirtualDisplay {
    fn modes(&self) -> (Timing, Timing) {
        (self.created, self.mode)
    }
    fn set_mode(&mut self, mode: Timing) {
        self.mode = mode;
    }
    fn offer_mode(&self) -> bool {
        match self.driver.update_modes(self.id, self.mode) {
            Ok(()) => true,
            Err(error) => {
                tracing::warn!(output = %self.name, error = %format!("{error:#}"), mode = %format_timing(self.mode), "the display driver did not offer the mode");
                false
            }
        }
    }
    fn listed_rates(&self) -> Vec<u32> {
        offered_rates(&supported_modes(&self.name), self.mode.0, self.mode.1)
    }
    fn apply(&self, stage: &str) -> bool {
        self.apply_mode(stage)
    }
    fn applied_size(&self) -> Option<(u32, u32)> {
        VirtualDisplay::applied_size(self)
    }
    fn switch_on(&mut self) -> Result<Monitor> {
        let monitor = self.current_monitor()?;
        take_name(&mut self.name, &mut self.generation, &monitor.display_name);
        Ok(monitor)
    }
}
/// Take the name Windows gave a display it switched back on. A new name is a
/// new generation, as after a heartbeat switched it on: the guards of the
/// streams on it, a paused one too, reapply their layout and capture there.
/// A guard made after this reads the new generation, so it recovers nothing.
fn take_name(name: &mut String, generation: &mut u64, listed: &str) {
    if name != listed {
        *name = listed.into();
        *generation = generation.wrapping_add(1);
    }
}
fn lease<D: SharedDisplay>(
    displays: &mut Leases<D>,
    id: &str,
    requested: Timing,
    create: impl FnOnce() -> Result<D>,
) -> Result<(std::sync::Arc<std::sync::Mutex<D>>, Option<u32>)> {
    displays.retain(|_, lease| lease.strong_count() != 0);
    if let Some(display) = displays.get(id).and_then(std::sync::Weak::upgrade) {
        let (created, current) = display.lock().unwrap().modes();
        let needed = join(created, current, requested)?;
        return Ok((display, needed));
    }
    let display = std::sync::Arc::new(std::sync::Mutex::new(create()?));
    displays.insert(id.into(), std::sync::Arc::downgrade(&display));
    Ok((display, None))
}
/// The monitor of a leased display, switched back on first (a paused shared
/// display can be off), then raised to `needed`, the refresh a joining stream
/// asks for above the display's ([`display_lease`]). A raise Windows refuses
/// fails with why; the display keeps its mode for the streams on it.
pub(super) fn joined_monitor(
    display: &mut impl SharedDisplay,
    needed: Option<u32>,
) -> Result<Monitor> {
    let monitor = display.switch_on()?;
    if let Some(rate) = needed {
        let (_, before) = display.modes();
        if raise(display, rate).is_none() {
            bail!(
                "shared virtual display already uses a different mode: it runs at {:.3} Hz, and Windows refused the {:.3} Hz this stream asks for",
                f64::from(before.2) / 1000.,
                f64::from(rate) / 1000.
            );
        }
        tracing::info!(
            from = %format_timing(before),
            to_millihz = rate,
            "a joining stream raised the shared virtual display's refresh"
        );
    }
    Ok(monitor)
}
/// Raise a leased display's refresh in place for a stream that asks for more
/// than its launch did, keeping the display and what runs on it ([`raise`]).
/// A shared display too: a faster refresh serves the streams on it.
pub(super) fn raise_lease(
    display: &std::sync::Mutex<impl SharedDisplay>,
    rate: u32,
) -> Option<u32> {
    raise(&mut *display.lock().unwrap(), rate)
}
/// Raise the refresh in place to at least `rate`: the refresh (millihertz)
/// the display has now, or None when Windows refused it. A refusal puts the
/// previous mode back, since Windows can refuse the rate yet leave another
/// rate or size behind for the streams already on it. A rate Windows does not
/// list yet is not set: it would refuse it as a bad mode.
fn raise(display: &mut impl SharedDisplay, rate: u32) -> Option<u32> {
    let (_, previous) = display.modes();
    if rate <= previous.2 {
        return Some(previous.2);
    }
    display.set_mode((previous.0, previous.1, rate));
    if offer_listed(display, None) && display.apply("stream request") {
        return Some(rate);
    }
    display.set_mode(previous);
    display.apply("stream raise refused");
    None
}
/// Give the display another size at its refresh in place, for a stream that
/// turned between portrait and landscape: whether Windows shows that size,
/// at that refresh or a slower one it lists ([`fallback_rate`]). Windows can
/// list the size already (the driver's standard sizes, or an earlier turn) at
/// other refreshes, so the turn waits for its refresh or for the list Windows
/// shows at that size to change: the driver lists a size's refreshes at once.
/// Either way it is set then, and a slower listed refresh serves it. When
/// Windows does not show the size, the display goes back to its previous mode.
fn reshape(display: &mut impl SharedDisplay, width: u32, height: u32) -> bool {
    let (_, previous) = display.modes();
    display.set_mode((width, height, previous.2));
    let before = display.listed_rates();
    offer_listed(display, Some(&before));
    display.apply("stream turned");
    if display.applied_size() == Some((width, height)) {
        return true;
    }
    display.set_mode(previous);
    display.apply("stream turn refused");
    false
}
/// Have the driver offer the display's mode on the live display, and wait
/// until Windows lists it at its refresh, or for a turn until the refreshes
/// Windows lists at its size differ from `turn_from`: the driver publishes
/// the new list after it answers (16 ms later in the S1a run), and Windows
/// refuses a mode it does not list yet (DISP_CHANGE_BADMODE). A mode already
/// listed costs one look. A driver that refused is not waited for. False,
/// with why, when Windows does not list it.
fn offer_listed(display: &impl SharedDisplay, turn_from: Option<&[u32]>) -> bool {
    let (_, mode) = display.modes();
    let turn = turn_from.is_some();
    let listed = || {
        let rates = display.listed_rates();
        lists_rate(&rates, mode.2)
            || turn_from.is_some_and(|before| !rates.is_empty() && rates != before)
    };
    if !display.offer_mode() {
        return listed();
    }
    let started = Instant::now();
    match wait_listed(listed, MODE_LIST_WAIT) {
        Some(1) => true,
        Some(polls) => {
            tracing::info!(
                mode = %format_timing(mode),
                turn,
                polls,
                waited_ms = started.elapsed().as_millis() as u64,
                "Windows listed the mode the display driver offers"
            );
            true
        }
        None => {
            let listed_hz = display.listed_rates();
            let message = if turn {
                "Windows does not list the mode the display driver was asked to offer; the turn takes a refresh Windows lists at that size"
            } else {
                "Windows does not list the mode the display driver was asked to offer; the display keeps its mode"
            };
            tracing::warn!(
                mode = %format_timing(mode),
                turn,
                waited_ms = started.elapsed().as_millis() as u64,
                ?listed_hz,
                "{message}"
            );
            false
        }
    }
}
/// A stream joins a shared virtual display that was created at the size it
/// asks for, or that shows that size now: a turn or a size the user picked in
/// Windows moves `current`, and the encoder scales as it does on a shared
/// physical display. Any refresh at least the stream's serves it, since
/// capture and encode are paced per stream: a VRR stream's 1000 Hz display
/// serves a fixed 60 fps one. A stream that asks for more gets the refresh
/// the display has to be raised to (for the streams on it, too, a faster
/// refresh serves them). Another size fails with why.
fn join(created: Timing, current: Timing, requested: Timing) -> Result<Option<u32>> {
    let (width, height, rate) = requested;
    if (width, height) != (created.0, created.1) && (width, height) != (current.0, current.1) {
        bail!(
            "shared virtual display already uses a different mode: it is {}x{}, and this stream asks for {width}x{height}",
            current.0,
            current.1
        );
    }
    Ok((rate > current.2).then_some(rate))
}
fn lease_exists(result: Result<()>) -> Result<bool> {
    match result {
        Ok(()) => Ok(true),
        // The driver reports STATUS_NOT_FOUND for an expired/missing lease.
        // BUSY and other I/O failures say nothing about monitor ownership.
        Err(error)
            if error
                .downcast_ref::<windows::core::Error>()
                .is_some_and(|e| {
                    e.code() == windows::core::HRESULT::from_win32(ERROR_NOT_FOUND.0)
                }) =>
        {
            Ok(false)
        }
        Err(error) => Err(error),
    }
}
fn retry_busy<T>(mut attempt: impl FnMut() -> Result<T>, timeout: Duration) -> Result<T> {
    let deadline = Instant::now() + timeout;
    loop {
        match attempt() {
            Err(error)
                if error
                    .downcast_ref::<windows::core::Error>()
                    .is_some_and(|e| e.code() == ERROR_BUSY.to_hresult())
                    && Instant::now() < deadline =>
            {
                tracing::debug!(%error, "virtual display driver busy; retrying");
                std::thread::sleep(
                    Duration::from_millis(100)
                        .min(deadline.saturating_duration_since(Instant::now())),
                );
            }
            result => return result,
        }
    }
}
/// Poll until `listed` holds, a few milliseconds apart: how many polls that
/// took, or None once `timeout` passed. A mode already listed costs one poll.
fn wait_listed(mut listed: impl FnMut() -> bool, timeout: Duration) -> Option<u32> {
    let deadline = Instant::now() + timeout;
    let mut polls = 0;
    loop {
        polls += 1;
        if listed() {
            return Some(polls);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(
            Duration::from_millis(5).min(deadline.saturating_duration_since(Instant::now())),
        );
    }
}
fn wait_for_monitor(
    mut query: impl FnMut() -> Result<Option<Monitor>>,
    mut recover: impl FnMut() -> Result<()>,
    timeout: Duration,
) -> Result<Monitor> {
    let deadline = Instant::now() + timeout;
    let mut next_recovery = Instant::now();
    loop {
        if let Some(monitor) = query()? {
            return Ok(monitor);
        }
        anyhow::ensure!(
            Instant::now() < deadline,
            "owned virtual display unavailable after startup protection"
        );
        if Instant::now() >= next_recovery {
            recover()?;
            next_recovery = Instant::now() + Duration::from_secs(1);
        }
        std::thread::sleep(
            Duration::from_millis(50).min(deadline.saturating_duration_since(Instant::now())),
        );
    }
}
/// What a lease heartbeat does about the owned display.
#[derive(Debug, PartialEq)]
enum Heartbeat {
    /// Keep the current desktop while a game is paused, even if the lease
    /// disappeared. Resume repairs a missing lease before capture starts.
    Keep,
    /// Leased and on the desktop.
    Present,
    /// Leased and on the desktop, but Windows lists it under another adapter
    /// and target (and desktop name) than the ones the display knows.
    Relisted,
    /// Leased and connected, but Windows switched it off.
    SwitchOn,
    /// The driver no longer knows the lease.
    Recreate,
}
/// Only the driver's missing-lease answer permits creating the display again.
/// A failed renewal or topology query is retried at the next heartbeat: the
/// driver refuses to create a display that still exists (ERROR_BUSY), and a
/// lease that stays unrenewed expires and is then reported missing. A display
/// Windows shows by its device path under another pair is followed, never
/// switched on: its old routes are no longer available, so switching on
/// would fail every second while the stream's layout stays lost.
fn heartbeat(
    renewed: Result<bool>,
    recover: bool,
    active: impl FnOnce() -> Result<bool>,
    relisted: impl FnOnce() -> Result<bool>,
    connected: impl FnOnce() -> Result<bool>,
) -> Result<Heartbeat> {
    let exists = renewed?;
    if !recover {
        return Ok(Heartbeat::Keep);
    }
    if !exists {
        return Ok(Heartbeat::Recreate);
    }
    if active()? {
        return Ok(Heartbeat::Present);
    }
    if relisted()? {
        return Ok(Heartbeat::Relisted);
    }
    anyhow::ensure!(
        connected()?,
        "owned virtual display is temporarily absent from Windows topology"
    );
    Ok(Heartbeat::SwitchOn)
}
/// The monitor Windows lists for the display the driver created. The driver
/// reports the adapter and target IddCx gave it, and normally Windows lists
/// the display there. On some systems (the Ryzen 7 8845HS laptop with a
/// Radeon 780M) it lists it under the render adapter with another target id;
/// the monitor's own device path keeps the driver's target id as its UID. A
/// route without a device path cannot identify the display yet, and one that
/// another session's display goes by (`others`) is never taken.
fn listed<'a>(
    monitors: &'a [Monitor],
    adapter: LUID,
    target: u32,
    others: &[((u32, i32), u32)],
) -> Option<&'a Monitor> {
    let named = || {
        monitors
            .iter()
            .filter(|m| !m.monitor_device_path.is_empty())
    };
    named()
        .find(|m| m.adapter == adapter && m.target == target)
        .or_else(|| {
            named().find(|m| {
                is_virtual_monitor(&m.monitor_device_path, target)
                    && !others.contains(&((m.adapter.LowPart, m.adapter.HighPart), m.target))
            })
        })
}
/// Whether a display shares its desktop with a physical display: Windows
/// listed the new virtual display duplicated on the panel's source. A stream
/// mode set on that source would change the panel.
fn cloned_with_physical(monitors: &[Monitor], display: &Monitor) -> bool {
    monitors.iter().any(|m| {
        m.adapter == display.adapter
            && m.source == display.source
            && !m
                .monitor_device_path
                .eq_ignore_ascii_case(&display.monitor_device_path)
            && !is_rubylight_monitor(&m.monitor_device_path)
    })
}
/// Whether any of the display's routes is connected.
fn connected(routes: &[Route]) -> bool {
    Topology::query_all().is_ok_and(|all| {
        all.paths.iter().any(|p| {
            p.targetInfo.targetAvailable.as_bool()
                && routes.contains(&(p.targetInfo.adapterId, p.targetInfo.id))
        })
    })
}
/// Why a display Windows keeps duplicated on a physical display is refused.
fn still_duplicated(activation: &str) -> String {
    format!(
        "virtual display stays duplicated with a physical display; moving it to a source of its own failed (switching it on: {activation})"
    )
}
/// The monitor Windows lists for the display's device path, under whatever
/// adapter and target it goes by now. The path is the display's identity; an
/// empty one names nothing.
fn listed_by_path(monitors: Vec<Monitor>, path: &str) -> Option<Monitor> {
    monitors
        .into_iter()
        .find(|m| !path.is_empty() && m.monitor_device_path.eq_ignore_ascii_case(path))
}
/// Why the display did not come up, with what the last attempt to switch it
/// on came to.
fn not_active(connected: bool, activation: &str) -> String {
    if connected {
        format!(
            "virtual display was connected but Windows kept it switched off (switching it on: {activation})"
        )
    } else {
        "virtual display did not become active before the deadline".into()
    }
}
impl VirtualDisplay {
    pub fn create(stable_id: &str, width: u32, height: u32, fps: u32) -> Result<Self> {
        Self::create_rate(
            stable_id,
            width,
            height,
            fps.checked_mul(1000).context("refresh overflow")?,
        )
    }
    pub fn create_rate(stable_id: &str, width: u32, height: u32, fps: u32) -> Result<Self> {
        Self::create_options(stable_id, width, height, fps, VirtualOptions::default())
    }
    pub fn create_options(
        stable_id: &str,
        width: u32,
        height: u32,
        fps: u32,
        options: VirtualOptions,
    ) -> Result<Self> {
        if !(320..=7680).contains(&width)
            || !(200..=4320).contains(&height)
            || !(1000..=1_000_000).contains(&fps)
        {
            bail!("virtual display mode is outside the driver limits");
        }
        let driver = Control::open_recovering()?;
        let id = butterpollo_core::display_policy::virtual_display_id(stable_id);
        apply_render_adapter(&driver, &options);
        // A previous host's display can survive a session handover until its
        // removal completes or its heartbeat expires. Keep the same identity.
        let (result, startup_protection) = retry_busy(
            || {
                let protection = hotplug::Protection::capture()?;
                let result = driver.add_bytes(&rubylight_vdd::add_request(
                    id,
                    (width, height, fps),
                    options.peak_nits,
                ))?;
                Ok((result, protection))
            },
            CREATE_RETRY,
        )?;
        let mut display = Self {
            driver,
            name: String::new(),
            id,
            last_feed: Instant::now(),
            last_renew: Instant::now(),
            mode: (width, height, fps),
            created: (width, height, fps),
            size_applied: std::sync::atomic::AtomicBool::new(false),
            options,
            generation: 0,
            startup_protection: Some(startup_protection),
            resolved_target: None,
            driver_route: None,
            switched_on: false,
        };
        display.resolve(&result)?;
        display.check_hotplug("created")?;
        Ok(display)
    }
    /// The display path the driver's create reply names.
    fn identity(&self, result: &[u8]) -> Result<(LUID, u32)> {
        let reply = rubylight_vdisplay_proto::control::AddReply::decode(result)
            .map_err(|e| anyhow::anyhow!("invalid display driver reply: {e}"))?;
        rubylight_vdd::register(rubylight_vdd::Owned {
            session_id: self.id,
            adapter: (reply.adapter_luid_low, reply.adapter_luid_high),
            target: reply.target_id,
            wudf_pid: reply.wudf_pid,
            listed: None,
            registration: 0, // Assigned by register after this successful ADD.
        });
        Ok((
            LUID {
                LowPart: reply.adapter_luid_low,
                HighPart: reply.adapter_luid_high,
            },
            reply.target_id,
        ))
    }
    fn resolve(&mut self, result: &[u8]) -> Result<()> {
        let (luid, target) = self.identity(result)?;
        self.driver_route = Some((luid, target));
        let deadline = Instant::now() + Duration::from_secs(10);
        // Windows decides whether a display it has just connected joins the
        // desktop. A layout it saved for the same displays (duplicate, or one
        // screen only) can leave the new virtual display connected but off.
        // Vibepollo's display helper switches it on after a short grace; so
        // do we, without changing the other displays.
        let mut connected_since = None;
        let mut next_check = Instant::now();
        // What the last attempt to switch it on came to.
        let mut activation = String::from("not tried");
        // The display while Windows has it duplicated on a physical display.
        let mut duplicated: Option<Monitor> = None;
        let others = rubylight_vdd::others(self.id);
        while Instant::now() < deadline {
            if let Ok(monitors) = monitors()
                && let Some(m) = listed(&monitors, luid, target, &others)
            {
                if !cloned_with_physical(&monitors, m) {
                    return self.adopt(m.clone());
                }
                // Not ready: the stream's mode would change the physical
                // display. Give it a desktop of its own.
                duplicated = Some(m.clone());
            }
            if Instant::now() >= next_check {
                next_check = Instant::now() + Duration::from_millis(250);
                let mut routes = vec![(luid, target)];
                routes.extend(duplicated.iter().map(|m| (m.adapter, m.target)));
                if duplicated.is_some() || connected(&routes) {
                    let since = *connected_since.get_or_insert_with(Instant::now);
                    if since.elapsed() >= Duration::from_secs(1) {
                        next_check = Instant::now() + Duration::from_secs(1);
                        match activate_target(&routes) {
                            Ok(Some(kept_timings)) => {
                                activation = "Windows accepted the layout".into();
                                self.switched_on = true;
                                tracing::info!(
                                    kept_timings,
                                    duplicated = duplicated.is_some(),
                                    "switched the new virtual display on beside the current displays"
                                )
                            }
                            Ok(None) => activation = "no route to switch on".into(),
                            Err(error) => {
                                activation = format!("{error:#}");
                                tracing::warn!(
                                    error = activation,
                                    "could not switch on the new virtual display"
                                )
                            }
                        }
                    }
                }
            }
            if self.last_feed.elapsed() >= Duration::from_secs(1) {
                self.renew()?;
                self.last_feed = Instant::now();
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        if duplicated.is_some() {
            // Streaming it would set the stream's mode on the panel's source.
            bail!(still_duplicated(&activation));
        }
        bail!(not_active(connected_since.is_some(), &activation))
    }
    /// Take `m`, the monitor Windows lists for the new display, as the
    /// display's own.
    fn adopt(&mut self, m: Monitor) -> Result<()> {
        if let Some((luid, target)) = self.driver_route
            && (m.adapter, m.target) != (luid, target)
        {
            tracing::info!(
                driver_adapter = luid.LowPart,
                driver_target = target,
                listed_adapter = m.adapter.LowPart,
                listed_target = m.target,
                "Windows lists the new virtual display under another adapter and target than the driver reported"
            );
            rubylight_vdd::rehome(self.id, (m.adapter.LowPart, m.adapter.HighPart), m.target);
        }
        self.name = m.display_name.clone();
        self.resolved_target = Some(m);
        self.last_feed = Instant::now();
        Ok(())
    }
    pub(super) fn owns_monitor(&self, monitor: &Monitor) -> bool {
        self.resolved_target.as_ref().is_some_and(|owned| {
            self.routes()
                .is_ok_and(|routes| routes.contains(&(monitor.adapter, monitor.target)))
                && owned
                    .monitor_device_path
                    .eq_ignore_ascii_case(&monitor.monitor_device_path)
        })
    }
    /// Every adapter and target the display can be listed under.
    pub(super) fn routes(&self) -> Result<Vec<Route>> {
        let owned = self.hotplug_monitor()?;
        let mut routes = vec![(owned.adapter, owned.target)];
        routes.extend(self.driver_route.filter(|route| *route != routes[0]));
        Ok(routes)
    }
    fn hotplug_monitor(&self) -> Result<&Monitor> {
        // Resolve stores the target returned by our driver request. Never infer
        // ownership from a GDI display name that Windows could later reuse.
        self.resolved_target
            .as_ref()
            .context("owned virtual display unavailable during startup protection")
    }
    fn refresh_name(&mut self) -> Result<()> {
        self.name = self.current_monitor()?.display_name;
        Ok(())
    }
    pub(super) fn current_monitor(&mut self) -> Result<Monitor> {
        let owned = self.hotplug_monitor()?.clone();
        let routes = self.routes()?;
        // Windows can list the display under another pair after switching it
        // off and on: its device path stays, so that finds it.
        let monitor = wait_for_monitor(
            || Ok(listed_by_path(monitors()?, &owned.monitor_device_path)),
            || {
                // Windows can switch the owned target off during startup,
                // before the stream starts feeding its lease.
                self.renew()?;
                self.last_feed = Instant::now();
                // SAFETY: Topology::shows supplies initialized device-name
                // structures with matching header types and sizes.
                if Topology::query_all()?.shows(&routes, &owned, |header| unsafe {
                    DisplayConfigGetDeviceInfo(header)
                })? && let Some(kept_timings) = activate_target(&routes)?
                {
                    self.switched_on = true;
                    tracing::info!(
                        kept_timings,
                        "owned virtual display reactivated during startup"
                    );
                }
                Ok(())
            },
            Duration::from_secs(10),
        )?;
        if (monitor.adapter, monitor.target) != (owned.adapter, owned.target) {
            tracing::info!(
                from_adapter = owned.adapter.LowPart,
                from_target = owned.target,
                listed_adapter = monitor.adapter.LowPart,
                listed_target = monitor.target,
                "Windows lists the virtual display under another adapter and target than before"
            );
            rubylight_vdd::rehome(
                self.id,
                (monitor.adapter.LowPart, monitor.adapter.HighPart),
                monitor.target,
            );
            self.resolved_target = Some(monitor.clone());
        }
        Ok(monitor)
    }
    fn check_hotplug(&mut self, stage: &str) -> Result<()> {
        if let Some(protection) = &self.startup_protection {
            if let Err(error) = protection.check(self.hotplug_monitor()?, stage) {
                // Keeping another display off is a courtesy; a stream that
                // cannot start is worse than one that leaves it on.
                tracing::warn!(
                    error = format!("{error:#}"),
                    stage,
                    "could not keep inactive displays off; streaming without startup protection"
                );
                self.startup_protection = None;
            }
            self.refresh_name()?;
        }
        Ok(())
    }
    /// Finish only after virtual HDR settings have settled. Afterwards the
    /// user is free to change displays without a stream overriding them.
    pub(super) fn finish_hotplug(&mut self, stage: &str) -> Result<()> {
        if self.startup_protection.is_some() {
            let owned = self.hotplug_monitor()?.clone();
            let protection = self.startup_protection.as_mut().unwrap();
            if let Err(error) = protection.settle(&owned, stage) {
                // The guard covers startup only and is a courtesy to the other
                // displays: end it and keep streaming rather than fail.
                tracing::warn!(
                    error = format!("{error:#}"),
                    stage,
                    "virtual display startup protection ended before the layout settled"
                );
            }
            self.refresh_name()?;
            self.startup_protection = None;
        }
        self.apply_mode(stage);
        Ok(())
    }
    /// Set the stream's mode on the display. The driver offers it, but
    /// Windows picks the mode it saved for this display identity, or the
    /// EDID's preferred timing, which the driver caps at 60 Hz for 4K. It
    /// does so on arrival, when it switches the display back on and when a
    /// layout is applied. A refused mode is logged, not retried: the stream
    /// continues at the display's mode and its card reports the difference.
    /// Returns whether Windows shows the mode.
    pub(super) fn apply_mode(&self, stage: &str) -> bool {
        let (width, height, rate) = self.mode;
        let result = Topology::set_mode_rate(
            &self.name,
            width,
            height,
            butterpollo_core::framegen::Rate(rate),
        );
        let actual = current_timing(&self.name);
        let requested = format_timing(self.mode);
        let mut size_applied = true;
        let applied = match mode_outcome(self.mode, actual.as_ref().ok().copied()) {
            // Applied tolerates rational rates: log what Windows shows.
            ModeOutcome::Applied => {
                tracing::info!(
                    output = %self.name,
                    stage,
                    requested = %requested,
                    actual = %actual.as_ref().map_or(requested.clone(), |t| format_timing(*t)),
                    "virtual display mode applied"
                );
                true
            }
            outcome => {
                let actual_text = match outcome {
                    ModeOutcome::Kept(actual) => format_timing(actual),
                    _ => "unknown".into(),
                };
                let offered = offered_rates(&supported_modes(&self.name), width, height);
                tracing::warn!(
                    output = %self.name,
                    stage,
                    requested = %requested,
                    actual = %actual_text,
                    offered_hz = ?offered,
                    error = ?result.err().or(actual.err()).map(|e| format!("{e:#}")),
                    "virtual display mode not applied; streaming at the mode Windows chose"
                );
                // A refused refresh must not cost the resolution too (issue #40: 4K at
                // 1000 Hz for VRR refused, and Windows fell back to 1440p). A display at
                // the requested size keeps its refresh, as a refused raise does.
                let wrong_size =
                    !matches!(outcome, ModeOutcome::Kept((w, h, _)) if (w, h) == (width, height));
                size_applied = !wrong_size;
                if wrong_size && let Some(fallback) = fallback_rate(&offered, rate) {
                    let retried = Topology::set_mode_rate(
                        &self.name,
                        width,
                        height,
                        butterpollo_core::framegen::Rate(fallback),
                    );
                    let now = current_timing(&self.name).ok();
                    size_applied = now.is_some_and(|(w, h, _)| (w, h) == (width, height));
                    if size_applied {
                        tracing::info!(
                            output = %self.name,
                            stage,
                            requested = %requested,
                            actual = %now.map_or_else(String::new, format_timing),
                            "virtual display kept the requested resolution at a refresh Windows accepts"
                        );
                    } else {
                        tracing::warn!(
                            output = %self.name,
                            stage,
                            fallback_millihz = fallback,
                            error = ?retried.err().map(|e| format!("{e:#}")),
                            "virtual display refused the fallback refresh too"
                        );
                    }
                }
                false
            }
        };
        self.size_applied
            .store(size_applied, std::sync::atomic::Ordering::Relaxed);
        applied
    }
    /// Give the display another size at its refresh in place, for a stream
    /// that turned between portrait and landscape ([`reshape`]). False when
    /// Windows did not show that size; the display goes back to its previous
    /// mode then.
    pub(super) fn reshape(&mut self, width: u32, height: u32) -> bool {
        reshape(self, width, height)
    }
    /// Make a resolution the user chose in Windows the one later layouts,
    /// resumes and recoveries put back. Windows shows it, so the driver offers
    /// it. The refresh stays the stream's (VRR's 1000 Hz, say); one Windows
    /// refuses falls back at this resolution as for any other.
    pub(super) fn adopt_size(&mut self, width: u32, height: u32) {
        self.mode.0 = width;
        self.mode.1 = height;
        self.size_applied
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
    /// The stream's resolution, when Windows showed it after it was last set:
    /// a display at another one then is the user's choice, not a refusal.
    pub(super) fn applied_size(&self) -> Option<(u32, u32)> {
        self.size_applied
            .load(std::sync::atomic::Ordering::Relaxed)
            .then_some((self.mode.0, self.mode.1))
    }
    fn renew(&mut self) -> Result<()> {
        self.last_renew = Instant::now();
        self.driver.renew(self.id)
    }
    /// Keep a paused game's driver lease alive without changing the desktop.
    pub(super) fn keep_alive(&mut self) -> Result<()> {
        if self.last_renew.elapsed() >= Duration::from_secs(1) {
            self.maintain(false)?;
        }
        Ok(())
    }
    pub fn feed(&mut self) -> Result<()> {
        if self.last_feed.elapsed() >= Duration::from_secs(1) {
            self.recover_now()?;
        }
        Ok(())
    }
    /// Resume must activate a paused display before looking up its GDI name,
    /// even if its last renewal or active heartbeat was less than a second ago.
    pub(super) fn recover_now(&mut self) -> Result<()> {
        self.maintain(true)
    }
    fn maintain(&mut self, recover: bool) -> Result<()> {
        if recover {
            self.last_feed = Instant::now();
        }
        let renewed = lease_exists(self.renew());
        if renewed.is_err() {
            // Reopen a stale transport for the next heartbeat, without
            // mistaking its error for permission to create another monitor.
            if let Ok(driver) = Control::open_recovering() {
                self.driver = driver;
            }
        }
        // SAFETY: Topology::shows supplies a live source- or target-name structure with a matching header type and size.
        let device_info = |header: &mut DISPLAYCONFIG_DEVICE_INFO_HEADER| unsafe {
            DisplayConfigGetDeviceInfo(header)
        };
        // Without a resolved display the lookups below report that.
        let routes = self.routes().unwrap_or_default();
        match heartbeat(
            renewed,
            recover,
            || Topology::query()?.shows(&routes, self.hotplug_monitor()?, device_info),
            || {
                let path = &self.hotplug_monitor()?.monitor_device_path;
                Ok(listed_by_path(monitors()?, path).is_some())
            },
            || Topology::query_all()?.shows(&routes, self.hotplug_monitor()?, device_info),
        )? {
            Heartbeat::Keep | Heartbeat::Present => return Ok(()),
            Heartbeat::Relisted => {
                // Windows moved the active display to another adapter and
                // target, so its desktop name changed too. The name lookup
                // follows the device path and takes the new pair as the
                // display's routes; the guard then reapplies the stream's
                // layout and HDR on the new name.
                self.generation = self.generation.wrapping_add(1);
                self.refresh_name()?;
                tracing::info!(output=%self.name, "owned virtual display moved to another adapter or target");
                return Ok(());
            }
            Heartbeat::SwitchOn => {
                // A live lease can be switched off by a Windows layout
                // recall. Activate that same target; creating its ID again
                // returns BUSY.
                activate_target(&self.routes()?)?
                    .context("owned virtual display has no connected route yet")?;
                // Publish the change before the name lookup, which can lag
                // the switch. Once the display is on, later heartbeats find
                // it present, so a failed lookup must not leave the guard
                // unaware: it reapplies the stream's layout and HDR and
                // takes the display's new desktop name itself.
                self.generation = self.generation.wrapping_add(1);
                self.refresh_name()?;
                tracing::info!(output=%self.name, "owned virtual display reactivated");
                return Ok(());
            }
            Heartbeat::Recreate => {}
        }
        // Only a confirmed missing lease permits creation. Retain the
        // stream's descriptor and identity, never a recalled desktop mode.
        self.driver = Control::open_recovering()?;
        // A restarted driver has forgotten the render adapter.
        apply_render_adapter(&self.driver, &self.options);
        let protection = hotplug::Protection::capture()?;
        let response = self.driver.add_bytes(&rubylight_vdd::add_request(
            self.id,
            self.mode,
            self.options.peak_nits,
        ))?;
        self.startup_protection = Some(protection);
        self.resolve(&response)?;
        // Retain a resolved recovery even if protection needs a retry, so
        // Guard::feed still completes HDR/startup validation next time.
        self.generation = self.generation.wrapping_add(1);
        self.check_hotplug("recreated")?;
        tracing::info!(output=%self.name, "owned virtual display recovered");
        Ok(())
    }
}
impl Drop for VirtualDisplay {
    fn drop(&mut self) {
        DisplayProvider::remove(&self.driver, self.id);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    /// A shared virtual display as Windows treats it.
    #[derive(Debug)]
    struct Fake {
        created: Timing,
        mode: Timing,
        /// What Windows shows.
        shown: std::cell::Cell<Timing>,
        /// Off, as a paused display can be, refuses every mode.
        on: bool,
        /// Refused refreshes, after which Windows shows 60 Hz.
        refuses: Vec<u32>,
        /// The modes Windows lists, and so can set.
        listed: std::cell::RefCell<Vec<Timing>>,
        /// Offered modes Windows does not list yet.
        pending: std::cell::RefCell<Vec<Timing>>,
        /// The look at the display's modes after an offer in which Windows
        /// lists the offered modes; None never does.
        lists_after: Option<u32>,
        /// Looks at the display's modes since the last offer.
        looks: std::cell::Cell<u32>,
        /// The fastest refresh Windows lists an offered mode at, as on a PC
        /// that lists 4K only up to 240 Hz.
        fastest: u32,
        /// The driver refuses to offer a mode.
        driver_refuses: bool,
        log: std::cell::RefCell<Vec<String>>,
    }
    impl Fake {
        fn new(mode: Timing) -> Self {
            Self {
                created: mode,
                mode,
                shown: mode.into(),
                on: true,
                refuses: Vec::new(),
                listed: vec![mode].into(),
                pending: Default::default(),
                lists_after: Some(1),
                looks: 0.into(),
                fastest: u32::MAX,
                driver_refuses: false,
                log: Default::default(),
            }
        }
        /// The refreshes (whole hertz) Windows lists at the mode's size.
        fn rates(&self) -> Vec<u32> {
            self.listed
                .borrow()
                .iter()
                .filter(|listed| (listed.0, listed.1) == (self.mode.0, self.mode.1))
                .map(|listed| listed.2 / 1000)
                .collect()
        }
    }
    impl SharedDisplay for Fake {
        fn modes(&self) -> (Timing, Timing) {
            (self.created, self.mode)
        }
        fn set_mode(&mut self, mode: Timing) {
            self.mode = mode;
        }
        fn offer_mode(&self) -> bool {
            self.log.borrow_mut().push(format!("offer {}", self.mode.2));
            if self.driver_refuses {
                return false;
            }
            self.pending.borrow_mut().push(self.mode);
            self.looks.set(0);
            true
        }
        fn listed_rates(&self) -> Vec<u32> {
            self.looks.set(self.looks.get() + 1);
            if self
                .lists_after
                .is_some_and(|after| self.looks.get() >= after)
            {
                let offered: Vec<_> = self.pending.borrow_mut().drain(..).collect();
                self.listed.borrow_mut().extend(
                    offered
                        .into_iter()
                        .map(|(width, height, rate)| (width, height, rate.min(self.fastest))),
                );
            }
            self.rates()
        }
        fn apply(&self, stage: &str) -> bool {
            self.log.borrow_mut().push(stage.into());
            let (width, height, rate) = self.mode;
            if !self.on || self.refuses.contains(&rate) {
                self.shown.set((width, height, 60_000));
                return false;
            }
            if self.listed.borrow().contains(&self.mode) {
                self.shown.set(self.mode);
                return true;
            }
            // A rate Windows does not list is a bad mode: apply_mode keeps
            // the size at a slower listed rate, else Windows keeps its mode.
            if let Some(fallback) = super::fallback_rate(&self.rates(), rate) {
                self.shown.set((width, height, fallback));
            }
            false
        }
        fn applied_size(&self) -> Option<(u32, u32)> {
            let shown = self.shown.get();
            ((shown.0, shown.1) == (self.mode.0, self.mode.1)).then_some((shown.0, shown.1))
        }
        fn switch_on(&mut self) -> anyhow::Result<crate::display::Monitor> {
            self.on = true;
            self.log.borrow_mut().push("on".into());
            Ok(super::super::monitor("virtual", 1, false))
        }
    }
    /// Lease `id` for a stream at `requested`, creating `fresh` when nothing has it.
    fn lease(
        displays: &mut super::Leases<Fake>,
        requested: Timing,
        fresh: Option<Fake>,
    ) -> anyhow::Result<(Arc<Mutex<Fake>>, Option<u32>)> {
        super::lease(displays, "shared", requested, || {
            Ok(fresh.expect("a display was created"))
        })
    }
    /// Lease and switch on, as a stream's display guard does.
    fn stream(
        displays: &mut super::Leases<Fake>,
        requested: Timing,
        fresh: Option<Fake>,
    ) -> anyhow::Result<Arc<Mutex<Fake>>> {
        let (display, needed) = lease(displays, requested, fresh)?;
        super::joined_monitor(&mut *display.lock().unwrap(), needed)?;
        Ok(display)
    }

    #[test]
    fn a_second_stream_joins_a_shared_display_whose_mode_changed_in_place() {
        let launched = (1920, 1080, 60_000);
        // The first stream raised the rate, turned, or Windows took a size
        // the user picked: the launch mode and the shown one both join.
        for current in [
            (1920, 1080, 120_000),
            (1080, 1920, 60_000),
            (2560, 1440, 60_000),
        ] {
            assert_eq!(super::join(launched, current, launched).unwrap(), None);
            assert_eq!(super::join(launched, current, current).unwrap(), None);
            let error = super::join(launched, current, (1280, 720, 60_000))
                .unwrap_err()
                .to_string();
            assert!(error.contains("asks for 1280x720"), "{error}");
        }
        assert_eq!(super::join(launched, launched, launched).unwrap(), None);
    }

    #[test]
    fn a_vrr_stream_and_a_fixed_rate_stream_share_a_virtual_display_either_way() {
        // Nightjar desktop with VRR asks 1000 Hz, a phone at 60 fps 120 Hz.
        let vrr = (1920, 1080, 1_000_000);
        let fixed = (1920, 1080, 120_000);
        // VRR first: its 1000 Hz serves the 60 fps stream as it is.
        let mut displays = super::Leases::new();
        let first = stream(&mut displays, vrr, Some(Fake::new(vrr))).unwrap();
        let second = stream(&mut displays, fixed, None).unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(first.lock().unwrap().shown.get(), vrr);
        // Only switched on, by each stream: no mode was set.
        assert_eq!(*first.lock().unwrap().log.borrow(), ["on", "on"]);
        // The fixed rate first: the VRR stream raises the display, and keeps VRR.
        let mut displays = super::Leases::new();
        let first = stream(&mut displays, fixed, Some(Fake::new(fixed))).unwrap();
        let second = stream(&mut displays, vrr, None).unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(first.lock().unwrap().mode, vrr);
        assert_eq!(first.lock().unwrap().shown.get(), vrr);
        // Now at 1000 Hz, a third stream at either rate joins as it is.
        assert_eq!(lease(&mut displays, fixed, None).unwrap().1, None);
        assert_eq!(lease(&mut displays, vrr, None).unwrap().1, None);
        // Once every stream left, the next one gets a display of its own.
        drop((first, second));
        let fresh = stream(&mut displays, fixed, Some(Fake::new(fixed))).unwrap();
        assert_eq!(fresh.lock().unwrap().mode, fixed);
    }

    #[test]
    fn a_shared_virtual_display_that_cannot_serve_a_stream_refuses_it_with_why() {
        let fixed = (1920, 1080, 120_000);
        let mut displays = super::Leases::new();
        let first = stream(&mut displays, fixed, Some(Fake::new(fixed))).unwrap();
        // Another size, even at a rate the display serves.
        let error = lease(&mut displays, (2560, 1440, 60_000), None)
            .unwrap_err()
            .to_string();
        assert_eq!(
            error,
            "shared virtual display already uses a different mode: it is 1920x1080, and this stream asks for 2560x1440"
        );
        // A refresh Windows refuses to raise the display to.
        first.lock().unwrap().refuses.push(1_000_000);
        let error = stream(&mut displays, (1920, 1080, 1_000_000), None)
            .unwrap_err()
            .to_string();
        assert_eq!(
            error,
            "shared virtual display already uses a different mode: it runs at 120.000 Hz, and Windows refused the 1000.000 Hz this stream asks for"
        );
        // Windows left 60 Hz behind: the stream already on it gets its mode back.
        let display = first.lock().unwrap();
        assert_eq!(display.mode, fixed);
        assert_eq!(display.shown.get(), fixed);
        assert_eq!(
            display.log.borrow().last().map(String::as_str),
            Some("stream raise refused")
        );
    }

    #[test]
    fn a_stream_that_asks_for_more_than_its_launch_raises_a_shared_display_too() {
        // Two streams launched at 60 fps on one display; one asks for 240 Hz
        // at stream start.
        let fixed = (1920, 1080, 120_000);
        let mut displays = super::Leases::new();
        let first = stream(&mut displays, fixed, Some(Fake::new(fixed))).unwrap();
        let second = stream(&mut displays, fixed, None).unwrap();
        assert_eq!(super::raise_lease(&second, 240_000), Some(240_000));
        assert_eq!(first.lock().unwrap().shown.get(), (1920, 1080, 240_000));
        // Asking for less leaves the faster refresh; a refusal puts it back.
        assert_eq!(super::raise_lease(&first, 120_000), Some(240_000));
        first.lock().unwrap().refuses.push(1_000_000);
        assert_eq!(super::raise_lease(&first, 1_000_000), None);
        assert_eq!(second.lock().unwrap().shown.get(), (1920, 1080, 240_000));
    }

    #[test]
    fn a_display_switched_on_under_a_new_name_is_a_new_generation() {
        let (mut name, mut generation) = (String::from(r"\\.\DISPLAY3"), 4);
        // The same name: the paused stream's guard keeps its target.
        super::take_name(&mut name, &mut generation, r"\\.\DISPLAY3");
        assert_eq!((name.as_str(), generation), (r"\\.\DISPLAY3", 4));
        // A new one: its guard sees another generation and moves there.
        super::take_name(&mut name, &mut generation, r"\\.\DISPLAY5");
        assert_eq!((name.as_str(), generation), (r"\\.\DISPLAY5", 5));
    }

    #[test]
    fn a_stream_raises_a_paused_shared_display_only_once_it_is_on_again() {
        // The VRR device relaunches onto the display a disconnected phone
        // left paused, and Windows switched it off meanwhile.
        let fixed = (1920, 1080, 120_000);
        let mut displays = super::Leases::new();
        let paused = stream(&mut displays, fixed, Some(Fake::new(fixed))).unwrap();
        {
            let mut display = paused.lock().unwrap();
            display.on = false;
            display.log.borrow_mut().clear();
        }
        stream(&mut displays, (1920, 1080, 1_000_000), None).unwrap();
        let display = paused.lock().unwrap();
        assert_eq!(
            *display.log.borrow(),
            ["on", "offer 1000000", "stream request"]
        );
        assert_eq!(display.shown.get(), (1920, 1080, 1_000_000));
    }

    #[test]
    fn a_joining_stream_raises_a_shared_display_once_windows_lists_the_rate() {
        // S1a order B: the Retroid at 120 Hz first, then the VRR laptop at
        // 1000 Hz. The driver published the 1000 Hz list 16 ms after the host
        // set it, and Windows refused it as a bad mode.
        let fixed = (1920, 1080, 120_000);
        let vrr = (1920, 1080, 1_000_000);
        let mut displays = super::Leases::new();
        let first = stream(&mut displays, fixed, Some(Fake::new(fixed))).unwrap();
        {
            let mut display = first.lock().unwrap();
            display.lists_after = Some(4);
            display.log.borrow_mut().clear();
        }
        let second = stream(&mut displays, vrr, None).unwrap();
        let display = second.lock().unwrap();
        // Set only in the fourth look, once Windows listed it.
        assert_eq!(display.looks.get(), 4);
        assert_eq!(
            *display.log.borrow(),
            ["on", "offer 1000000", "stream request"]
        );
        assert_eq!(display.shown.get(), vrr);
    }

    #[test]
    fn a_mode_the_driver_refused_to_offer_is_not_waited_for() {
        let fixed = (1920, 1080, 120_000);
        let mut displays = super::Leases::new();
        let first = stream(&mut displays, fixed, Some(Fake::new(fixed))).unwrap();
        {
            let mut display = first.lock().unwrap();
            display.driver_refuses = true;
            display.log.borrow_mut().clear();
        }
        let started = Instant::now();
        assert_eq!(super::raise_lease(&first, 1_000_000), None);
        assert!(started.elapsed() < Duration::from_secs(1));
        {
            let display = first.lock().unwrap();
            assert_eq!(display.looks.get(), 1);
            assert_eq!(
                *display.log.borrow(),
                ["offer 1000000", "stream raise refused"]
            );
            assert_eq!(display.shown.get(), fixed);
        }
        // Windows lists it already: the raise needs no new list.
        first
            .lock()
            .unwrap()
            .listed
            .borrow_mut()
            .push((1920, 1080, 240_000));
        assert_eq!(super::raise_lease(&first, 240_000), Some(240_000));
        assert_eq!(first.lock().unwrap().shown.get(), (1920, 1080, 240_000));
    }

    #[test]
    fn a_turned_stream_keeps_its_size_at_a_slower_rate_windows_lists() {
        // A VRR stream at 4K and 1000 Hz turns to portrait on a PC that lists
        // 4K only up to 240 Hz. Windows lists the turned size in the third
        // look; the turn applies there at 240 Hz (issue #40).
        let landscape = (3840, 2160, 1_000_000);
        let mut display = Fake::new(landscape);
        display.fastest = 240_000;
        display.lists_after = Some(3);
        assert!(super::reshape(&mut display, 2160, 3840));
        assert_eq!(display.looks.get(), 3);
        assert_eq!(*display.log.borrow(), ["offer 1000000", "stream turned"]);
        assert_eq!(display.shown.get(), (2160, 3840, 240_000));
        assert_eq!(display.mode, (2160, 3840, 1_000_000));
        // Turned back: the landscape size is listed, so one look serves it.
        display.log.borrow_mut().clear();
        assert!(super::reshape(&mut display, 3840, 2160));
        assert_eq!(display.looks.get(), 1);
        assert_eq!(display.shown.get(), landscape);
    }

    #[test]
    fn a_turn_to_a_size_windows_lists_at_other_rates_waits_for_its_own() {
        // A portrait VRR stream turns to 1920x1080, which the driver lists at
        // 60 and 120 Hz from the start: those are not the turn's list.
        let portrait = (1080, 1920, 1_000_000);
        let landscape = (1920, 1080, 1_000_000);
        let standard = |display: &Fake| {
            display
                .listed
                .borrow_mut()
                .extend([(1920, 1080, 60_000), (1920, 1080, 120_000)]);
        };
        let mut display = Fake::new(portrait);
        standard(&display);
        display.lists_after = Some(3);
        assert!(super::reshape(&mut display, 1920, 1080));
        assert_eq!(display.looks.get(), 3);
        assert_eq!(display.shown.get(), landscape);
        // Where Windows lists the size only up to 480 Hz, the new list is
        // the turn's too, and the turn takes 480 Hz (issue #40).
        let mut display = Fake::new(portrait);
        standard(&display);
        display.lists_after = Some(3);
        display.fastest = 480_000;
        assert!(super::reshape(&mut display, 1920, 1080));
        assert_eq!(display.looks.get(), 3);
        assert_eq!(display.shown.get(), (1920, 1080, 480_000));
        // A list that never changes: after the wait the turn takes 120 Hz.
        let mut display = Fake::new(portrait);
        standard(&display);
        display.lists_after = None;
        assert!(super::reshape(&mut display, 1920, 1080));
        assert_eq!(display.shown.get(), (1920, 1080, 120_000));
    }

    #[test]
    fn a_rate_windows_never_lists_is_not_set_and_the_stream_is_refused_with_why() {
        let fixed = (1920, 1080, 120_000);
        let mut displays = super::Leases::new();
        let first = stream(&mut displays, fixed, Some(Fake::new(fixed))).unwrap();
        {
            let mut display = first.lock().unwrap();
            display.lists_after = None;
            display.log.borrow_mut().clear();
        }
        let error = stream(&mut displays, (1920, 1080, 1_000_000), None)
            .unwrap_err()
            .to_string();
        assert_eq!(
            error,
            "shared virtual display already uses a different mode: it runs at 120.000 Hz, and Windows refused the 1000.000 Hz this stream asks for"
        );
        let display = first.lock().unwrap();
        // The unlisted rate was never set; the stream on it keeps its mode.
        assert!(display.looks.get() > 1);
        assert_eq!(
            *display.log.borrow(),
            ["on", "offer 1000000", "stream raise refused"]
        );
        assert_eq!(display.mode, fixed);
        assert_eq!(display.shown.get(), fixed);
    }

    #[test]
    fn waiting_for_a_listed_mode_costs_one_look_when_it_is_listed_and_stops_in_time() {
        assert_eq!(super::wait_listed(|| true, MODE_LIST_WAIT), Some(1));
        let mut looks = 0;
        let polls = super::wait_listed(
            || {
                looks += 1;
                looks == 3
            },
            MODE_LIST_WAIT,
        );
        assert_eq!(polls, Some(3));
        let started = Instant::now();
        assert_eq!(
            super::wait_listed(|| false, Duration::from_millis(30)),
            None
        );
        let waited = started.elapsed();
        assert!(waited >= Duration::from_millis(30), "{waited:?}");
        assert!(waited < Duration::from_secs(1), "{waited:?}");
    }

    #[test]
    fn the_console_names_why_setup_did_not_install_the_driver() {
        use windows::Win32::Foundation::ERROR_NO_MORE_ITEMS;
        let missing =
            anyhow::Error::new(windows::core::Error::from(ERROR_NO_MORE_ITEMS.to_hresult()));
        let refused = "Smart App Control is on, and it refuses a display driver signed on this PC, so streams use a physical display.";
        assert_eq!(super::problem_text(&missing, Some(refused.into())), refused);
        // Nothing recorded, or an empty value: the error explains it.
        for recorded in [None, Some(" ".into())] {
            assert!(
                super::problem_text(&missing, recorded)
                    .starts_with("Rubylight's display driver is not installed")
            );
        }
    }
    use super::super::modes::timing_matches;
    use super::*;
    #[test]
    fn a_busy_driver_request_recovers_within_the_same_call() {
        let mut attempts = 0;
        let result = retry_busy(
            || {
                attempts += 1;
                if attempts < 3 {
                    Err(windows::core::Error::from_hresult(ERROR_BUSY.to_hresult()).into())
                } else {
                    Ok(vec![17, 23])
                }
            },
            Duration::from_secs(1),
        )
        .unwrap();
        assert_eq!(result, [17, 23]);
        assert_eq!(attempts, 3);
    }
    #[test]
    fn driver_retries_stop_at_the_deadline_and_preserve_other_errors() {
        for code in [
            ERROR_BUSY,
            ERROR_ACCESS_DENIED,
            ERROR_NOT_FOUND,
            ERROR_GEN_FAILURE,
        ] {
            let mut attempts = 0;
            let timeout = if code == ERROR_BUSY {
                Duration::ZERO
            } else {
                Duration::from_secs(1)
            };
            let error = retry_busy::<()>(
                || {
                    attempts += 1;
                    Err(windows::core::Error::from_hresult(code.to_hresult()).into())
                },
                timeout,
            )
            .unwrap_err();
            assert_eq!(attempts, 1);
            assert_eq!(
                error.downcast_ref::<windows::core::Error>().unwrap().code(),
                code.to_hresult()
            );
        }
    }
    #[test]
    fn an_owned_monitor_can_reappear_during_session_handover() {
        let mut attempts = 0;
        let found = wait_for_monitor(
            || {
                attempts += 1;
                Ok((attempts == 3).then(|| super::super::monitor("owned", 4, false)))
            },
            || Ok(()),
            Duration::from_secs(1),
        )
        .unwrap();
        assert_eq!(found.device_id, "owned");
        assert_eq!(attempts, 3);
    }
    #[test]
    fn startup_recovers_a_connected_monitor_instead_of_only_polling_it() {
        let renewals = std::cell::Cell::new(0);
        let found = wait_for_monitor(
            || Ok((renewals.get() == 2).then(|| super::super::monitor("owned", 4, false))),
            || {
                renewals.set(renewals.get() + 1);
                Ok(())
            },
            Duration::from_secs(2),
        )
        .unwrap();
        assert_eq!(found.device_id, "owned");
        assert_eq!(renewals.get(), 2);
    }
    #[test]
    fn startup_does_not_reactivate_a_present_monitor_or_hide_a_lease_error() {
        assert!(
            wait_for_monitor(
                || Ok(Some(super::super::monitor("owned", 4, false))),
                || panic!("an active display needs no recovery"),
                Duration::ZERO,
            )
            .is_ok()
        );
        let error = wait_for_monitor(
            || Ok(None),
            || Err(windows::core::Error::from_hresult(ERROR_ACCESS_DENIED.to_hresult()).into()),
            Duration::from_secs(1),
        )
        .err()
        .unwrap();
        assert_eq!(
            error.downcast_ref::<windows::core::Error>().unwrap().code(),
            ERROR_ACCESS_DENIED.to_hresult()
        );
    }
    #[test]
    fn an_absent_owned_monitor_or_failed_query_does_not_wait_forever() {
        let mut attempts = 0;
        assert!(
            wait_for_monitor(
                || {
                    attempts += 1;
                    Ok(None)
                },
                || panic!("expired lookup must not change the topology"),
                Duration::ZERO,
            )
            .is_err()
        );
        assert_eq!(attempts, 1);
        let error = wait_for_monitor(
            || Err(windows::core::Error::from_hresult(ERROR_ACCESS_DENIED.to_hresult()).into()),
            || panic!("failed query must not change the topology"),
            Duration::from_secs(1),
        )
        .err()
        .unwrap();
        assert_eq!(
            error.downcast_ref::<windows::core::Error>().unwrap().code(),
            ERROR_ACCESS_DENIED.to_hresult()
        );
    }
    #[test]
    fn only_a_missing_driver_lease_allows_recreation() {
        assert!(lease_exists(Ok(())).unwrap());
        for code in [
            ERROR_BUSY,
            ERROR_RETRY,
            ERROR_GEN_FAILURE,
            ERROR_ACCESS_DENIED,
            ERROR_INVALID_HANDLE,
        ] {
            let error =
                windows::core::Error::from_hresult(windows::core::HRESULT::from_win32(code.0));
            assert!(lease_exists(Err(error.into())).is_err(), "{code:?}");
            assert!(
                lease_exists(Ok(())).unwrap(),
                "the next heartbeat can succeed"
            );
        }
        let error = windows::core::Error::from_hresult(windows::core::HRESULT::from_win32(
            ERROR_NOT_FOUND.0,
        ));
        assert!(!lease_exists(Err(error.into())).unwrap());
    }
    #[test]
    fn heartbeat_recreates_only_a_missing_lease_and_switches_on_a_display_windows_turned_off() {
        let failure = |code: WIN32_ERROR| -> anyhow::Error {
            windows::core::Error::from_hresult(windows::core::HRESULT::from_win32(code.0)).into()
        };
        let busy = || lease_exists(Err(failure(ERROR_BUSY)));
        let missing = || lease_exists(Err(failure(ERROR_NOT_FOUND)));
        // The log this guards against: the lease renewed, Windows had switched
        // the display off, and a duplicate create returned ERROR_BUSY.
        assert_eq!(
            heartbeat(
                Ok(true),
                true,
                || Ok(true),
                || panic!("present needs no relisting lookup"),
                || panic!("present needs no full query")
            )
            .unwrap(),
            Heartbeat::Present
        );
        assert_eq!(
            heartbeat(Ok(true), true, || Ok(false), || Ok(false), || Ok(true)).unwrap(),
            Heartbeat::SwitchOn
        );
        // Issue 56: Windows showed the display under another adapter and
        // target, its old route stayed listed but unavailable, and switching
        // that route on failed every second. A display Windows shows is
        // followed, never switched on.
        assert_eq!(
            heartbeat(
                Ok(true),
                true,
                || Ok(false),
                || Ok(true),
                || panic!("a relisted display is not switched on")
            )
            .unwrap(),
            Heartbeat::Relisted
        );
        // Not yet shown anywhere, or a query failed: retry, never create.
        assert!(heartbeat(Ok(true), true, || Ok(false), || Ok(false), || Ok(false)).is_err());
        assert!(
            heartbeat(
                Ok(true),
                true,
                || Err(failure(ERROR_BUSY)),
                || panic!("no relisting lookup"),
                || panic!("no full query")
            )
            .is_err()
        );
        assert!(
            heartbeat(
                Ok(true),
                true,
                || Ok(false),
                || Err(failure(ERROR_BUSY)),
                || panic!("no full query")
            )
            .is_err()
        );
        assert!(
            heartbeat(
                Ok(true),
                true,
                || Ok(false),
                || Ok(false),
                || Err(failure(ERROR_BUSY))
            )
            .is_err()
        );
        // A renewal failure other than a missing lease is retried as well,
        // without touching the topology.
        assert!(
            heartbeat(
                busy(),
                true,
                || panic!("no topology query"),
                || panic!("no topology query"),
                || panic!("no topology query")
            )
            .is_err()
        );
        // A lease the driver no longer knows is a real loss: recreate at once,
        // whatever Windows still lists.
        assert_eq!(
            heartbeat(
                missing(),
                true,
                || panic!("no topology query"),
                || panic!("no topology query"),
                || panic!("no topology query")
            )
            .unwrap(),
            Heartbeat::Recreate
        );
    }
    #[test]
    fn a_paused_heartbeat_keeps_local_layout_changes_and_defers_recreation() {
        // A Win+P layout can leave our display connected but inactive; a
        // driver restart can remove its lease. Neither permits a paused game
        // to change the desktop. No topology lookup is needed in either case.
        for exists in [true, false] {
            assert_eq!(
                heartbeat(
                    Ok(exists),
                    false,
                    || panic!("paused renewal must not query the desktop"),
                    || panic!("paused renewal must not look the display up"),
                    || panic!("paused renewal must not query connected displays"),
                )
                .unwrap(),
                Heartbeat::Keep
            );
            assert_eq!(
                heartbeat(Ok(exists), true, || Ok(false), || Ok(false), || Ok(true)).unwrap(),
                if exists {
                    Heartbeat::SwitchOn
                } else {
                    Heartbeat::Recreate
                },
                "an active stream or resume still repairs the display"
            );
        }
        let error = heartbeat(
            Err(windows::core::Error::from_hresult(ERROR_ACCESS_DENIED.to_hresult()).into()),
            false,
            || panic!("no topology query"),
            || panic!("no topology query"),
            || panic!("no topology query"),
        )
        .unwrap_err();
        assert_eq!(
            error.downcast_ref::<windows::core::Error>().unwrap().code(),
            ERROR_ACCESS_DENIED.to_hresult(),
            "paused renewal must not hide transport failures"
        );
    }
    #[test]
    #[ignore = "creates a virtual display and briefly switches it off"]
    fn native_virtual_display_left_off_by_windows_is_switched_on_without_retiming_others()
    -> Result<()> {
        let mut display = VirtualDisplay::create(
            &format!("activation-test-{}", std::process::id()),
            1280,
            720,
            60,
        )?;
        let owned = display
            .resolved_target
            .clone()
            .context("virtual display unresolved")?;
        let routes = display.routes()?;
        let ours = |m: &Monitor| routes.contains(&(m.adapter, m.target));
        // Device, width, height, refresh and desktop position.
        type Timing = (String, u32, u32, u32, i32, i32);
        let timings = || -> Result<Vec<Timing>> {
            monitors()?
                .iter()
                .filter(|m| !ours(m))
                .map(|m| {
                    let mode = mode(&m.display_name)?;
                    // SAFETY: mode() returns a display DEVMODEW with its display position union field initialized.
                    let position = unsafe { mode.Anonymous1.Anonymous2.dmPosition };
                    Ok((
                        m.device_id.clone(),
                        mode.dmPelsWidth,
                        mode.dmPelsHeight,
                        mode.dmDisplayFrequency,
                        position.x,
                        position.y,
                    ))
                })
                .collect()
        };
        let before = timings()?;
        // What Windows does when a saved layout leaves the new display off.
        let active = Topology::query()?;
        let off: Vec<_> = active
            .paths
            .iter()
            .filter(|p| !routes.contains(&(p.targetInfo.adapterId, p.targetInfo.id)))
            .copied()
            .collect();
        // SAFETY: The retained paths and queried modes remain valid for this synchronous display configuration call.
        check(unsafe {
            SetDisplayConfig(
                Some(&off),
                Some(&active.modes),
                SDC_APPLY | SDC_USE_SUPPLIED_DISPLAY_CONFIG | SDC_ALLOW_CHANGES,
            )
        })?;
        anyhow::ensure!(
            !monitors()?.iter().any(ours),
            "virtual display stayed on after switching it off"
        );
        let recovered = display.current_monitor()?;
        assert!(display.owns_monitor(&recovered));
        assert!(display.switched_on);
        assert_eq!(display.generation, 0);
        assert_eq!(timings()?, before);
        // Exercise the established stream's heartbeat separately from startup.
        // SAFETY: The same retained paths and queried modes remain valid here.
        check(unsafe {
            SetDisplayConfig(
                Some(&off),
                Some(&active.modes),
                SDC_APPLY | SDC_USE_SUPPLIED_DISPLAY_CONFIG | SDC_ALLOW_CHANGES,
            )
        })?;
        display.last_feed = Instant::now() - Duration::from_secs(2);
        display.feed()?;
        anyhow::ensure!(monitors()?.iter().any(ours), "virtual display stayed off");
        assert_eq!(display.generation, 1);
        assert!(display.owns_monitor(&owned));
        assert_eq!(timings()?, before);
        println!(
            "switched on {} beside {} displays",
            display.name,
            before.len()
        );
        Ok(())
    }
    #[test]
    #[ignore = "creates and expires owned 4K virtual displays and changes their mode and HDR"]
    fn native_virtual_display_recovery_restores_the_requested_mode_and_mode_list() -> Result<()> {
        let _ = tracing_subscriber::fmt()
            .with_test_writer()
            .with_ansi(false)
            .try_init();
        let before = Snapshot::capture()?;
        let result = (|| -> Result<()> {
            for rate in [116_000, 1_000_000] {
                let mut display = VirtualDisplay::create_rate(
                    &format!("mode-recovery-test-{}-{rate}", std::process::id()),
                    3840,
                    2160,
                    rate,
                )
                .with_context(|| format!("create 4K{}", rate / 1000))?;
                display
                    .finish_hotplug("test startup")
                    .with_context(|| format!("settle 4K{}", rate / 1000))?;
                let supported = supported_modes(&display.name);
                anyhow::ensure!(supported.contains(&(3840, 2160, rate / 1000)));
                anyhow::ensure!(supported.contains(&(1920, 1080, 60)));
                // The driver's EDID prefers 4K60; creation must set the
                // stream's rate rather than leave Windows' choice.
                let created = current_timing(&display.name)?;
                anyhow::ensure!(
                    timing_matches((3840, 2160, rate), created),
                    "created at {}",
                    format_timing(created)
                );
                Topology::set_mode_rate(
                    &display.name,
                    2560,
                    1440,
                    butterpollo_core::framegen::Rate(60_000),
                )?;
                // A healthy heartbeat must allow a game's own mode change.
                display.last_feed = Instant::now() - Duration::from_secs(2);
                display.feed()?;
                anyhow::ensure!(mode(&display.name)?.dmPelsWidth == 2560);
                anyhow::ensure!(display.generation == 0);

                // What a driver restart does: the monitor departs under a live stream.
                display.driver.remove(display.id)?;
                let deadline = Instant::now() + Duration::from_secs(20);
                while display.generation == 0 {
                    let recovered = display.feed();
                    if Instant::now() >= deadline {
                        recovered?;
                        bail!("owned virtual display did not recover");
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                let owned = display.hotplug_monitor()?.clone();
                let routes = display.routes()?;
                set_hdr(&owned, true)?;
                display
                    .finish_hotplug("test recovery after HDR")
                    .with_context(|| format!("finish recovered 4K{}", rate / 1000))?;
                let actual = current_timing(&display.name)?;
                anyhow::ensure!(
                    timing_matches((3840, 2160, rate), actual),
                    "recovered at {}",
                    format_timing(actual)
                );
                let recovered = supported_modes(&display.name);
                anyhow::ensure!(
                    supported.iter().all(|mode| recovered.contains(mode)),
                    "recovery restricted the mode list"
                );
                println!(
                    "recovered {} at 3840x2160@{} with {} modes after a 2560x1440@60 recall",
                    display.name,
                    rate / 1000,
                    recovered.len()
                );
                drop(display);
                let deadline = Instant::now() + Duration::from_secs(10);
                while Topology::query_all()?.paths.iter().any(|path| {
                    routes.contains(&(path.targetInfo.adapterId, path.targetInfo.id))
                        && path.targetInfo.targetAvailable.as_bool()
                }) {
                    anyhow::ensure!(Instant::now() < deadline, "owned display did not depart");
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
            Ok(())
        })();
        before.restore()?;
        result?;
        anyhow::ensure!(
            serde_json::to_value(Snapshot::capture()?)? == serde_json::to_value(before)?,
            "the pre-test layout was not restored"
        );
        Ok(())
    }
    #[test]
    fn a_vrr_display_requests_1000_hz_in_the_drivers_millihertz_field() {
        for rate in [1_000_000, 116_000, 59_940] {
            let request = rubylight_vdd::add_request(7, (3840, 2160, rate), 1000);
            assert_eq!(
                (request.width, request.height, request.refresh_mhz),
                (3840, 2160, rate)
            );
            assert!(request.validate().is_ok());
        }
        // Windows takes the mode as a reduced rational.
        assert_eq!(
            butterpollo_core::framegen::Rate(1_000_000).rational(),
            (1000, 1)
        );
        assert_eq!(
            butterpollo_core::framegen::Rate(116_000).rational(),
            (116, 1)
        );
        assert_eq!(
            butterpollo_core::framegen::Rate(59_940).rational(),
            (2997, 50)
        );
    }
    #[test]
    fn a_selected_gpu_renders_the_virtual_display_and_automatic_leaves_it_to_windows() {
        // Issue #28: the integrated GPU rendered the virtual display while
        // the selected discrete Radeon was asked to duplicate it.
        assert_eq!(VirtualOptions::default().render_adapter, None);
        use butterpollo_core::config::Config;
        assert_eq!(configured_render_adapter(&Config::default()), None);
        // A GPU that is gone leaves the choice to Windows, never another GPU.
        let missing = Config::parse("adapter_name = No Such GPU\n").unwrap();
        assert_eq!(configured_render_adapter(&missing), None);
    }
    #[test]
    fn a_display_windows_lists_under_the_render_adapter_is_still_ours() {
        // From the Ryzen 7 8845HS laptop: the driver reported the IddCx adapter
        // and target 258, and Windows listed the display on the Radeon 780M
        // under target 117440770, with the driver's target id as the UID.
        let luid = |low| LUID {
            LowPart: low,
            HighPart: 0,
        };
        let listing = |path: &str, adapter: u32, target: u32| Monitor {
            monitor_device_path: path.into(),
            adapter: luid(adapter),
            target,
            ..crate::display::monitor("m", adapter, false)
        };
        let panel = listing(
            r"\\?\DISPLAY#LEN8ABA#5&277e72b4&0&UID256#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}",
            0x10687,
            256,
        );
        let ours = listing(
            r"\\?\DISPLAY#RBY0001#1&28a6823a&0&UID258#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}",
            0x10687,
            117_440_770,
        );
        let found = |monitors: &[Monitor], target| {
            listed(monitors, luid(942_669_753), target, &[]).map(|m| m.target)
        };
        assert_eq!(
            found(&[panel.clone(), ours.clone()], 258),
            Some(117_440_770)
        );
        // The UID is the driver's target id: another id is another display, and
        // the panel is never taken for it.
        assert_eq!(found(&[panel.clone(), ours.clone()], 259), None);
        assert_eq!(found(&[panel], 256), None);
        // The route the driver reported wins when Windows lists it there.
        let direct = listing(&ours.monitor_device_path, 942_669_753, 258);
        assert_eq!(found(&[ours.clone(), direct], 258), Some(258));
        // A route without a device path cannot identify the display yet.
        let unnamed = listing("", 942_669_753, 258);
        assert_eq!(found(std::slice::from_ref(&unnamed), 258), None);
        assert_eq!(found(&[unnamed, ours.clone()], 258), Some(117_440_770));
        // The pair another session's display goes by is never taken.
        let taken = [((0x10687, 0), 117_440_770)];
        assert_eq!(
            listed(std::slice::from_ref(&ours), luid(942_669_753), 258, &taken).map(|m| m.target),
            None
        );
    }
    #[test]
    fn a_display_is_followed_to_a_new_pair_by_its_device_path() {
        // Windows listed it at 117440770 before it was switched off and at
        // 117440783 after; the device path is the same.
        let listing = |path: &str, target| Monitor {
            monitor_device_path: path.into(),
            target,
            ..crate::display::monitor("m", 0x10687, false)
        };
        let path = r"\\?\DISPLAY#RBY0001#1&28a6823a&0&UID258#{g}";
        let panel = listing(r"\\?\DISPLAY#LEN8ABA#5&277e72b4&0&UID256#{g}", 256);
        let moved = listing(&path.to_lowercase(), 117_440_783);
        let found = listed_by_path(vec![panel.clone(), moved], path);
        assert_eq!(found.map(|m| m.target), Some(117_440_783));
        // Another display, or no path at all, is never taken for it.
        assert!(listed_by_path(vec![panel], path).is_none());
        assert!(listed_by_path(vec![listing("", 5)], "").is_none());
    }
    #[test]
    fn a_display_that_stays_duplicated_is_refused_with_the_reason() {
        let message = still_duplicated("no free desktop source for the virtual display");
        assert!(message.contains("duplicated with a physical display"));
        assert!(message.contains("no free desktop source"));
    }
    #[test]
    fn a_display_on_the_panels_source_is_duplicated_and_not_ready() {
        let listing = |path: &str, target, source| Monitor {
            monitor_device_path: path.into(),
            target,
            source,
            ..crate::display::monitor("m", 0x10687, false)
        };
        let panel = listing(r"\\?\DISPLAY#LEN8ABA#5&277e72b4&0&UID256#{g}", 256, 0);
        let ours = listing(
            r"\\?\DISPLAY#RBY0001#1&28a6823a&0&UID258#{g}",
            117_440_770,
            0,
        );
        let other_virtual = listing(
            r"\\?\DISPLAY#RBY0001#1&28a6823a&0&UID259#{g}",
            117_440_771,
            0,
        );
        assert!(cloned_with_physical(&[panel.clone(), ours.clone()], &ours));
        // Alone, on a source of its own, or duplicated with a virtual display only.
        assert!(!cloned_with_physical(std::slice::from_ref(&ours), &ours));
        let own_source = Monitor {
            source: 1,
            ..ours.clone()
        };
        assert!(!cloned_with_physical(
            &[panel.clone(), own_source.clone()],
            &own_source
        ));
        assert!(!cloned_with_physical(&[other_virtual, ours.clone()], &ours));
        // Another adapter's source 0 is another desktop source.
        let elsewhere = Monitor {
            adapter: LUID {
                LowPart: 1,
                HighPart: 0,
            },
            ..panel
        };
        assert!(!cloned_with_physical(&[elsewhere, ours.clone()], &ours));
    }
    #[test]
    fn virtual_monitor_paths_name_the_driver_target_in_their_uid() {
        use super::super::topology::is_virtual_monitor;
        let path =
            r"\\?\DISPLAY#RBY0001#1&28a6823a&0&UID258#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}";
        assert!(is_virtual_monitor(path, 258));
        assert!(is_virtual_monitor(&path.replace("RBY", "rby"), 258));
        assert!(!is_virtual_monitor(path, 25), "a prefix of the id");
        assert!(!is_virtual_monitor(path, 58), "a suffix of the id");
        assert!(is_virtual_monitor(&path.replace("UID", "uid"), 258));
        let longer = path.replace("UID258", "UID2580");
        assert!(!is_virtual_monitor(&longer, 258), "UID2580 is not 258");
        assert!(is_virtual_monitor(&longer, 2580));
        assert!(!is_virtual_monitor(path, 2580), "UID258 is not 2580");
        assert!(!is_virtual_monitor(&path.replace("RBY", "LEN"), 258));
        assert!(!is_virtual_monitor("", 258));
        assert!(
            !is_virtual_monitor("a#\u{e4}\u{e4}#UID258", 258),
            "non-ASCII maker"
        );
    }
    #[test]
    fn the_failure_says_what_switching_the_display_on_came_to() {
        assert_eq!(
            not_active(false, "not tried"),
            "virtual display did not become active before the deadline"
        );
        assert_eq!(
            not_active(true, "Windows refused the display layout"),
            "virtual display was connected but Windows kept it switched off (switching it on: Windows refused the display layout)"
        );
    }
}
