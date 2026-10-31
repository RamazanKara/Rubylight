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
use lease::reshape;
pub(super) use lease::{DisplayLease, display_lease, joined_monitor, raise_lease};
#[cfg(test)]
use lease::{Leases, SharedDisplay, join, lease, take_name};
use presence::{
    Heartbeat, cloned_with_physical, connected, heartbeat, lease_exists, listed, listed_by_path,
    not_active, retry_busy, still_duplicated, wait_for_monitor, wait_listed,
};

mod driver;
mod lease;
mod presence;

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
