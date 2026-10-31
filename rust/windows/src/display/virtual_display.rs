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
use driver::apply_render_adapter;
#[cfg(test)]
use driver::problem_text;
pub use driver::{
    RenderAdapter, VirtualOptions, configured_render_adapter, ensure_virtual_display_available,
    virtual_display_available, virtual_display_installed, virtual_display_os,
    virtual_display_status, windows_build,
};

mod driver;

/// How long a create retries while a previous host's display with the same identity departs.
const CREATE_RETRY: Duration = Duration::from_secs(10);
/// How long Windows has to list a mode the driver was asked to offer on a live display.
const MODE_LIST_WAIT: Duration = Duration::from_secs(2);
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
    /// The modes the driver lists for the live display ([`SharedDisplay::offered`]).
    offered: Vec<Timing>,
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
    /// The modes the driver lists for it: the one it was created with, and
    /// those it offered since (its list is a union).
    fn offered(&mut self) -> &mut Vec<Timing>;
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
    fn offered(&mut self) -> &mut Vec<Timing> {
        &mut self.offered
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
/// listed costs one look. A driver that refused, or that offered the mode
/// before ([`SharedDisplay::offered`]), is not waited for: Windows' list does
/// not change then, and a turn back to a size Windows filters that refresh at
/// would stall every time. False, with why, when Windows does not list it.
fn offer_listed(display: &mut impl SharedDisplay, turn_from: Option<&[u32]>) -> bool {
    let (_, mode) = display.modes();
    let repeated = display.offered().contains(&mode);
    let accepted = display.offer_mode();
    if accepted && !repeated {
        display.offered().push(mode);
    }
    let display = &*display;
    let turn = turn_from.is_some();
    let listed = || {
        let rates = display.listed_rates();
        lists_rate(&rates, mode.2)
            || turn_from.is_some_and(|before| !rates.is_empty() && rates != before)
    };
    if !accepted || repeated {
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
            offered: vec![(width, height, fps)],
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
        // A new display: its driver list starts again from the mode it is created with.
        self.offered = vec![self.mode];
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
mod tests;
