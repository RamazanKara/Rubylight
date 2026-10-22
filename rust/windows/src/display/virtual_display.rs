//! Virtual displays from Rubylight's display driver (`rubylight_vdd`): creation, modes and
//! heartbeats.

use super::modes::{
    ModeOutcome, current_timing, fallback_rate, format_timing, mode_outcome, offered_rates,
    supported_modes,
};
use super::rubylight_vdd::{self, Control};
use super::topology::activate_target;
use super::*;
use butterpollo_core::virtual_output::{DisplayProvider, Mode};

/// How long a create retries while a previous host's display with the same identity departs.
const CREATE_RETRY: Duration = Duration::from_secs(10);
/// Windows 11 (build 22000) or later. Vibepollo keeps hosts on Windows 10 on
/// the physical display unless configured otherwise: virtual-display capture
/// depends on Windows 11 capture features.
pub fn windows_11() -> bool {
    static BUILD: std::sync::OnceLock<Option<u32>> = std::sync::OnceLock::new();
    BUILD
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
        .is_none_or(|build| build >= 22000)
}
pub fn virtual_display_available() -> bool {
    Control::open().is_ok()
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
        "Rubylight's display driver is not installed, or Windows must restart to finish installing it. The Rubylight service sets it up again a minute after it starts; if it stays missing, run the Rubylight installer.".into()
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
    /// Windows showed `mode`'s resolution after it was last set, at that or a
    /// slower refresh it accepted.
    size_applied: std::sync::atomic::AtomicBool,
    options: VirtualOptions,
    pub(super) generation: u64,
    startup_protection: Option<hotplug::Protection>,
    pub(super) resolved_target: Option<Monitor>,
    /// Windows left this display off and the host switched it on.
    pub switched_on: bool,
}
pub(super) type DisplayLease = std::sync::Arc<std::sync::Mutex<VirtualDisplay>>;
static DISPLAYS: std::sync::Mutex<
    std::collections::BTreeMap<String, std::sync::Weak<std::sync::Mutex<VirtualDisplay>>>,
> = std::sync::Mutex::new(std::collections::BTreeMap::new());
pub(super) fn display_lease(
    id: &str,
    width: u32,
    height: u32,
    fps: u32,
    options: &VirtualOptions,
) -> Result<DisplayLease> {
    let mut displays = DISPLAYS.lock().unwrap();
    displays.retain(|_, lease| lease.strong_count() != 0);
    if let Some(display) = displays.get(id).and_then(std::sync::Weak::upgrade) {
        if display.lock().unwrap().mode != (width, height, fps) {
            bail!("shared virtual display already uses a different mode");
        }
        return Ok(display);
    }
    let display = std::sync::Arc::new(std::sync::Mutex::new(VirtualDisplay::create_options(
        id,
        width,
        height,
        fps,
        options.clone(),
    )?));
    displays.insert(id.into(), std::sync::Arc::downgrade(&display));
    Ok(display)
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
    /// Leased and connected, but Windows switched it off.
    SwitchOn,
    /// The driver no longer knows the lease.
    Recreate,
}
/// Only the driver's missing-lease answer permits creating the display again.
/// A failed renewal or topology query is retried at the next heartbeat: the
/// driver refuses to create a display that still exists (ERROR_BUSY), and a
/// lease that stays unrenewed expires and is then reported missing.
fn heartbeat(
    renewed: Result<bool>,
    recover: bool,
    active: impl FnOnce() -> Result<bool>,
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
    anyhow::ensure!(
        connected()?,
        "owned virtual display is temporarily absent from Windows topology"
    );
    Ok(Heartbeat::SwitchOn)
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
            size_applied: std::sync::atomic::AtomicBool::new(false),
            options,
            generation: 0,
            startup_protection: Some(startup_protection),
            resolved_target: None,
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
        let deadline = Instant::now() + Duration::from_secs(10);
        // Windows decides whether a display it has just connected joins the
        // desktop. A layout it saved for the same displays (duplicate, or one
        // screen only) can leave the new virtual display connected but off.
        // Vibepollo's display helper switches it on after a short grace; so
        // do we, without changing the other displays.
        let mut connected_since = None;
        let mut next_check = Instant::now();
        while Instant::now() < deadline {
            if let Ok(monitors) = monitors()
                && let Some(m) = monitors
                    .into_iter()
                    // Windows can report the active route before publishing its
                    // monitor interface. An empty path cannot identify our display.
                    .find(|m| {
                        m.adapter == luid && m.target == target && !m.monitor_device_path.is_empty()
                    })
            {
                self.name = m.display_name.clone();
                self.resolved_target = Some(m);
                self.last_feed = Instant::now();
                return Ok(());
            }
            if Instant::now() >= next_check {
                next_check = Instant::now() + Duration::from_millis(250);
                let connected = Topology::query_all().is_ok_and(|all| {
                    all.paths.iter().any(|p| {
                        p.targetInfo.adapterId == luid
                            && p.targetInfo.id == target
                            && p.targetInfo.targetAvailable.as_bool()
                    })
                });
                if connected {
                    let since = *connected_since.get_or_insert_with(Instant::now);
                    if since.elapsed() >= Duration::from_secs(1) {
                        next_check = Instant::now() + Duration::from_secs(1);
                        match activate_target(luid, target) {
                            Ok(Some(kept_timings)) => {
                                self.switched_on = true;
                                tracing::info!(
                                    kept_timings,
                                    "Windows left the new virtual display switched off; switched it on beside the current displays"
                                )
                            }
                            Ok(None) => {}
                            Err(error) => tracing::warn!(
                                error = format!("{error:#}"),
                                "could not switch on the new virtual display"
                            ),
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
        if connected_since.is_some() {
            bail!("virtual display was connected but Windows kept it switched off")
        }
        bail!("virtual display did not become active before the deadline")
    }
    pub(super) fn owns_monitor(&self, monitor: &Monitor) -> bool {
        self.resolved_target.as_ref().is_some_and(|owned| {
            owned.adapter == monitor.adapter
                && owned.target == monitor.target
                && owned
                    .monitor_device_path
                    .eq_ignore_ascii_case(&monitor.monitor_device_path)
        })
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
        wait_for_monitor(
            || {
                Ok(monitors()?.into_iter().find(|m| {
                    m.adapter == owned.adapter
                        && m.target == owned.target
                        && m.monitor_device_path
                            .eq_ignore_ascii_case(&owned.monitor_device_path)
                }))
            },
            || {
                // Windows can switch the owned target off during startup,
                // before the stream starts feeding its lease.
                self.renew()?;
                self.last_feed = Instant::now();
                // SAFETY: Topology::shows supplies initialized device-name
                // structures with matching header types and sizes.
                if Topology::query_all()?.shows(&owned, |header| unsafe {
                    DisplayConfigGetDeviceInfo(header)
                })? && let Some(kept_timings) = activate_target(owned.adapter, owned.target)?
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
        )
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
    /// Raise the refresh in place for a stream that asks for more than its
    /// launch did, keeping the display and what runs on it. Returns the
    /// refresh (millihertz) the display has now, at least `rate`, or None
    /// when Windows refused it and the display kept its previous mode.
    pub(super) fn raise_rate(&mut self, rate: u32) -> Option<u32> {
        if rate <= self.mode.2 {
            return Some(self.mode.2);
        }
        let previous = self.mode;
        self.mode.2 = rate;
        self.driver
            .offer_mode(self.id, Mode::new(self.mode.0, self.mode.1, self.mode.2));
        if self.apply_mode("stream request") {
            return Some(rate);
        }
        self.mode = previous;
        None
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
        match heartbeat(
            renewed,
            recover,
            || Topology::query()?.shows(self.hotplug_monitor()?, device_info),
            || Topology::query_all()?.shows(self.hotplug_monitor()?, device_info),
        )? {
            Heartbeat::Keep | Heartbeat::Present => return Ok(()),
            Heartbeat::SwitchOn => {
                // A live lease can be switched off by a Windows layout
                // recall. Activate that same target; creating its ID again
                // returns BUSY.
                let owned = self.hotplug_monitor()?;
                activate_target(owned.adapter, owned.target)?
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
                || panic!("present needs no full query")
            )
            .unwrap(),
            Heartbeat::Present
        );
        assert_eq!(
            heartbeat(Ok(true), true, || Ok(false), || Ok(true)).unwrap(),
            Heartbeat::SwitchOn
        );
        // Not yet shown anywhere, or a query failed: retry, never create.
        assert!(heartbeat(Ok(true), true, || Ok(false), || Ok(false)).is_err());
        assert!(
            heartbeat(
                Ok(true),
                true,
                || Err(failure(ERROR_BUSY)),
                || panic!("no full query")
            )
            .is_err()
        );
        assert!(heartbeat(Ok(true), true, || Ok(false), || Err(failure(ERROR_BUSY))).is_err());
        // A renewal failure other than a missing lease is retried as well,
        // without touching the topology.
        assert!(
            heartbeat(
                busy(),
                true,
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
                    || panic!("paused renewal must not query connected displays"),
                )
                .unwrap(),
                Heartbeat::Keep
            );
            assert_eq!(
                heartbeat(Ok(exists), true, || Ok(false), || Ok(true)).unwrap(),
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
        let ours = |m: &Monitor| m.adapter == owned.adapter && m.target == owned.target;
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
            .filter(|p| {
                !(p.targetInfo.adapterId == owned.adapter && p.targetInfo.id == owned.target)
            })
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
                    path.targetInfo.adapterId == owned.adapter
                        && path.targetInfo.id == owned.target
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
}
