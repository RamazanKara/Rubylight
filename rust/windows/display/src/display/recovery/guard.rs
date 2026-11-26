//! Per-stream display settings guard for physical and virtual displays.

use super::super::modes::supported_modes;
use super::super::virtual_display::{DisplayLease, display_lease, joined_monitor, raise_lease};
use super::super::*;
use super::{
    Plan, SETTINGS, adopt, choose, lease_hdr, lease_setting, plan, restore_mode_rate_with,
    restore_settings, rotation, set_rotation,
};

/// The driver's view of a virtual display while a stream waits for its first
/// frame: whether Windows assigned a swap chain, whether the ring is held, and
/// how many frames were composed and pushed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DisplayStatus {
    pub swap_chain: bool,
    pub ring: bool,
    pub frames_acquired: u64,
    pub frames_pushed: u64,
}

/// [`DisplayStatus`] beside whether the CCD path still shows the display.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DisplayDiagnostics {
    pub status: DisplayStatus,
    /// None when the CCD query failed, as it does while the lock screen owns
    /// the input desktop; false is a real "not on an active path".
    pub ccd_active: Option<bool>,
}

pub struct Guard {
    pub output: String,
    virtual_display: Option<DisplayLease>,
    identity: String,
    pub(super) generation: u64,
    hdr: Option<bool>,
    /// A recovered display's HDR and mode still to be restored.
    pub(super) restore_pending: bool,
}
impl Guard {
    #[allow(clippy::too_many_arguments)] // Native display lease parameters.
    pub fn new(
        output: &str,
        virtual_mode: bool,
        stable_id: &str,
        width: u32,
        height: u32,
        fps: u32,
        hdr: bool,
        physical_resolution: Option<(u32, u32)>,
        physical_refresh: Option<u32>,
    ) -> Result<Self> {
        Self::new_options(
            output,
            virtual_mode,
            stable_id,
            width,
            height,
            butterpollo_media_policy::framegen::Rate(
                fps.checked_mul(1000).context("refresh overflow")?,
            ),
            Some(hdr),
            physical_resolution,
            physical_refresh
                .map(|r| butterpollo_media_policy::framegen::Rate(r.saturating_mul(1000))),
        )
    }
    #[allow(clippy::too_many_arguments)]
    pub fn new_options(
        output: &str,
        virtual_mode: bool,
        stable_id: &str,
        width: u32,
        height: u32,
        rate: butterpollo_media_policy::framegen::Rate,
        hdr: Option<bool>,
        physical_resolution: Option<(u32, u32)>,
        physical_refresh: Option<butterpollo_media_policy::framegen::Rate>,
    ) -> Result<Self> {
        Self::new_virtual_options(
            output,
            virtual_mode,
            stable_id,
            width,
            height,
            rate,
            hdr,
            physical_resolution,
            physical_refresh,
            &VirtualOptions::default(),
        )
    }
    #[allow(clippy::too_many_arguments)]
    pub fn new_virtual_options(
        output: &str,
        virtual_mode: bool,
        stable_id: &str,
        width: u32,
        height: u32,
        rate: butterpollo_media_policy::framegen::Rate,
        hdr: Option<bool>,
        physical_resolution: Option<(u32, u32)>,
        physical_refresh: Option<butterpollo_media_policy::framegen::Rate>,
        options: &VirtualOptions,
    ) -> Result<Self> {
        let (virtual_display, needed) = if virtual_mode {
            let (lease, needed) = display_lease(stable_id, width, height, rate.0, options)?;
            (Some(lease), needed)
        } else {
            (None, None)
        };
        let chosen = if let Some(v) = &virtual_display {
            joined_monitor(&mut *crate::lock_unpoisoned(v), needed)?
        } else {
            let choices = monitors()?;
            choose(&choices, output)
                .context("display unavailable")?
                .clone()
        };
        let mut guard = Self {
            output: chosen.display_name.clone(),
            generation: virtual_display
                .as_ref()
                .map_or(0, |v| crate::lock_unpoisoned(v).generation),
            hdr,
            virtual_display,
            identity: chosen.device_id.clone(),
            restore_pending: false,
        };
        let result = (|| -> Result<()> {
            let mut all = crate::lock_unpoisoned(&SETTINGS);
            let settings = all.entry(guard.identity.clone()).or_default();
            settings.users += 1;
            // The first user takes over what an earlier stream left pending
            // on this display, so that its original values come back.
            let pending = if settings.users == 1 {
                crate::display_recovery::pending(&guard.identity)
            } else {
                Default::default()
            };
            if (physical_resolution.is_some() || physical_refresh.is_some())
                && guard.virtual_display.is_none()
            {
                let previous = mode(&guard.output)?;
                let previous_rate = Topology::query()?.refresh(&guard.identity)?;
                let current = (previous.dmPelsWidth, previous.dmPelsHeight, previous_rate.0);
                let supported = supported_modes(&guard.output);
                let requested = if supported.is_empty() {
                    let (width, height) = physical_resolution
                        .unwrap_or((previous.dmPelsWidth, previous.dmPelsHeight));
                    (width, height, physical_refresh.unwrap_or(previous_rate).0)
                } else {
                    butterpollo_media_policy::display_policy::physical_mode(
                        &supported,
                        (physical_resolution, physical_refresh.map(|rate| rate.0)),
                        current,
                    )
                };
                if let Some((width, height)) = physical_resolution
                    && (width, height) != (requested.0, requested.1)
                {
                    tracing::info!(
                        output = %guard.output,
                        "display has no {width}x{height} mode; keeping {}x{} and scaling the stream",
                        requested.0,
                        requested.1
                    );
                }
                let (width, height, fps) = requested;
                let applied = settings.mode.as_ref().map(|(_, applied)| applied);
                if plan(settings.users, applied, &current, &requested) == Plan::Keep {
                    // Another stream uses this display; keep its mode and let
                    // the encoder scale, rather than refuse the second stream.
                    let kept = *applied.unwrap_or(&current);
                    if kept != requested {
                        tracing::info!(
                            output = %guard.output,
                            "another stream uses this display at {}x{}; scaling this stream",
                            kept.0,
                            kept.1
                        );
                    }
                } else {
                    let original = pending.mode.map_or(current, |(original, _)| original);
                    crate::display_recovery::mode_rate_requested(
                        &guard.identity,
                        &guard.output,
                        current,
                        requested,
                    )?;
                    settings.mode = Some((original, requested));
                    let applied = Topology::set_mode_rate(
                        &guard.output,
                        width,
                        height,
                        butterpollo_media_policy::framegen::Rate(fps),
                    );
                    // Record what the display has now even when the rate
                    // was refused: a TV can take the resolution and keep
                    // its old rate, and teardown restores only the mode
                    // it finds recorded.
                    let actual = mode(&guard.output)?;
                    let actual_rate = Topology::query()?.refresh(&guard.identity)?;
                    let actual_mode = (actual.dmPelsWidth, actual.dmPelsHeight, actual_rate.0);
                    crate::display_recovery::mode_rate(
                        &guard.identity,
                        &guard.output,
                        current,
                        actual_mode,
                    )?;
                    settings.mode = Some((original, actual_mode));
                    applied?;
                }
            }
            // A stream that leaves the mode as it is, or requests none, takes
            // the earlier stream's change over: its end restores the original
            // if the display still has that change, as recovery would.
            if settings.mode.is_none() && guard.virtual_display.is_none() {
                settings.mode = pending
                    .mode
                    .filter(|(original, applied)| original != applied);
            }
            // The same for a rotation and a scale an earlier stream left: the
            // display is read only when one is pending.
            if guard.virtual_display.is_none() {
                if settings.rotation.is_none()
                    && let Some(original) = pending.rotation
                {
                    settings.rotation = adopt(Some(original), rotation(&guard.identity)?);
                }
                if settings.scale.is_none()
                    && let Some(original) = pending.scale
                {
                    settings.scale = adopt(Some(original), dpi_scale(&chosen)?);
                }
            }
            lease_hdr(
                settings,
                &chosen,
                hdr,
                pending.hdr,
                |before, applied| {
                    crate::display_recovery::hdr(&guard.identity, &guard.output, before, applied)
                },
                set_hdr,
            )
        })();
        result?;
        if let Some(display) = &guard.virtual_display {
            let mut display = crate::lock_unpoisoned(display);
            display.finish_hotplug("after settings")?;
            guard.output = display.name.clone();
        }
        Ok(guard)
    }
    /// Turn this stream's physical display to `degrees` clockwise when the
    /// stream is its sole user; teardown, or recovery after a crash, turns it
    /// back. A display another stream uses keeps its rotation.
    pub fn rotate(&mut self, degrees: u16) -> Result<()> {
        if self.virtual_display.is_some() {
            bail!("a virtual display is shaped for a stream, not turned");
        }
        let requested = butterpollo_media_policy::display_policy::rotation_code(degrees)
            .with_context(|| format!("{degrees}° is not a display rotation"))?;
        let current = rotation(&self.identity)?;
        let mut all = crate::lock_unpoisoned(&SETTINGS);
        let settings = all
            .get_mut(&self.identity)
            .context("display has no settings lease")?;
        let users = settings.users;
        let turned = lease_setting(
            users,
            &mut settings.rotation,
            current,
            requested,
            |before, applied| {
                crate::display_recovery::rotation(&self.identity, &self.output, before, applied)
            },
            |rotation| set_rotation(&self.identity, rotation),
        )?;
        if turned {
            tracing::info!(output = %self.output, degrees, "display turned for the stream");
        } else if current != requested {
            tracing::warn!(output = %self.output, degrees, "another stream uses this display; keeping its rotation and scaling this stream");
        }
        Ok(())
    }
    /// Give this stream's physical display the Windows scale
    /// `dd_virtual_display_scale` asks for (0 leaves it) when the stream is its
    /// sole user; teardown, or recovery after a crash, puts the user's back.
    pub fn scale(&mut self, configured: i64) -> Result<()> {
        if configured == 0 {
            return Ok(());
        }
        if self.virtual_display.is_some() {
            bail!("a virtual display's scale is set with the display, not leased");
        }
        let desktop = mode(&self.output)?;
        let requested = super::super::modes::scale_percent(
            configured,
            desktop.dmPelsWidth,
            desktop.dmPelsHeight,
        )?;
        let monitors = monitors()?;
        let monitor = monitors
            .iter()
            .find(|m| m.device_id == self.identity)
            .context("display is unavailable for scaling")?;
        let current = dpi_scale(monitor)?;
        let mut all = crate::lock_unpoisoned(&SETTINGS);
        let settings = all
            .get_mut(&self.identity)
            .context("display has no settings lease")?;
        let users = settings.users;
        let scaled = lease_setting(
            users,
            &mut settings.scale,
            current,
            requested,
            |before, requested| {
                crate::display_recovery::scale_requested(
                    &self.identity,
                    &self.output,
                    before,
                    requested,
                )
            },
            |percent| set_dpi_scale(monitor, percent),
        )?;
        if scaled {
            // Windows took the scale: recovery after a crash from now on puts
            // the original back only while the display still has it.
            crate::display_recovery::scale(&self.identity, &self.output, current, requested)?;
            tracing::info!(output = %self.output, scale = requested, "display scaled for the stream");
        } else if current != requested {
            tracing::warn!(output = %self.output, scale = current, "another stream uses this display; keeping its scale");
        }
        Ok(())
    }
    /// Set the stream's mode on an owned virtual display again, after a step
    /// that lets Windows recall another one, such as applying a layout. A
    /// refusal is logged; callers verify and report the mode themselves.
    pub fn apply_virtual_mode(&self, stage: &str) {
        if let Some(display) = &self.virtual_display {
            crate::lock_unpoisoned(display).apply_mode(stage);
        }
    }
    /// Whether this guard owns a virtual display rather than a physical one.
    pub fn is_virtual(&self) -> bool {
        self.virtual_display.is_some()
    }
    /// Raise this guard's own virtual display to at least `rate` without
    /// recreating it, returning the refresh (millihertz) it has now. A shared
    /// one is raised too, as for a stream that joins it. None without a
    /// virtual display, or when Windows refused the rate.
    pub fn raise_virtual_rate(
        &self,
        rate: butterpollo_media_policy::framegen::Rate,
    ) -> Option<u32> {
        raise_lease(self.virtual_display.as_ref()?, rate.0)
    }
    /// Give this guard's own virtual display another size at the stream's
    /// refresh without recreating it, for a stream that turned between
    /// portrait and landscape. A display another stream shares keeps its
    /// shape, and one Windows refused keeps its previous size: both fail.
    pub fn reshape_virtual(&self, width: u32, height: u32) -> Result<()> {
        let display = self
            .virtual_display
            .as_ref()
            .context("a physical display turns rather than changes shape")?;
        if std::sync::Arc::strong_count(display) > 1 {
            bail!("another stream shares this virtual display, so it keeps its shape");
        }
        if !crate::lock_unpoisoned(display).reshape(width, height) {
            bail!("Windows did not show {width}x{height} on the virtual display");
        }
        Ok(())
    }
    /// The resolution this guard sets on its own virtual display. None
    /// without one, when another stream shares it, or when Windows did not
    /// show that resolution after it was last set.
    pub fn virtual_size(&self) -> Option<(u32, u32)> {
        let display = self.virtual_display.as_ref()?;
        if std::sync::Arc::strong_count(display) > 1 {
            return None;
        }
        crate::lock_unpoisoned(display).applied_size()
    }
    /// Set this resolution, at the stream's refresh, from now on where the
    /// guard sets its virtual display's mode again: after a layout, a resume
    /// or a recovery.
    pub fn adopt_virtual_size(&self, width: u32, height: u32) {
        if let Some(display) = &self.virtual_display {
            crate::lock_unpoisoned(display).adopt_size(width, height);
        }
    }
    /// True when the display was recovered, or a recovered display's HDR and
    /// mode were restored late: capture and the stream's layout follow it.
    pub fn feed(&mut self) -> Result<bool> {
        if let Some(display) = &mut self.virtual_display {
            let mut display = crate::lock_unpoisoned(display);
            display.feed()?;
            if display.generation != self.generation {
                let chosen = monitors()?
                    .into_iter()
                    .find(|m| display.owns_monitor(m))
                    .context("recovered display unavailable")?;
                // A display switched back on can have another desktop name,
                // which the heartbeat's own lookup may not have caught.
                display.name = chosen.display_name.clone();
                {
                    let mut settings = crate::lock_unpoisoned(&SETTINGS);
                    if chosen.device_id != self.identity {
                        if let Some(old) = settings.get_mut(&self.identity) {
                            old.users -= 1;
                            if old.users == 0 {
                                settings.remove(&self.identity);
                            }
                        }
                        if !settings.contains_key(&self.identity) {
                            crate::display_recovery::release(&self.identity)?;
                        }
                        settings.entry(chosen.device_id.clone()).or_default().users += 1;
                        self.identity = chosen.device_id.clone();
                    }
                }
                let restored = restore_recovered(&mut display, &self.identity, self.hdr);
                // Capture moves to the display even when its HDR or mode
                // cannot be restored yet: the old one shows nothing, so the
                // picture would freeze until the restore succeeds.
                self.output = display.name.clone();
                self.generation = display.generation;
                self.restore_pending = restored.is_err();
                if let Err(error) = restored {
                    tracing::warn!(error = %format!("{error:#}"), output = %self.output, "recovered display's HDR and mode will be restored on the next heartbeat");
                }
                return Ok(true);
            }
            if self.restore_pending {
                restore_recovered(&mut display, &self.identity, self.hdr)?;
                self.restore_pending = false;
                self.output = display.name.clone();
                return Ok(true);
            }
        }
        Ok(false)
    }
    /// The driver's status for the owned virtual display, with whether the
    /// CCD path still shows it. None for a physical display.
    pub fn diagnostics(&self, _stage: &str) -> Result<Option<DisplayDiagnostics>> {
        let Some(display) = &self.virtual_display else {
            return Ok(None);
        };
        let display = crate::lock_unpoisoned(display);
        let status = display.status()?;
        let active = display.active().ok();
        Ok(Some(DisplayDiagnostics {
            status: DisplayStatus {
                swap_chain: status.flags & rubylight_vdisplay_proto::control::MONITOR_SWAP_CHAIN
                    != 0,
                ring: status.flags & rubylight_vdisplay_proto::control::MONITOR_RING != 0,
                frames_acquired: status.frames_acquired,
                frames_pushed: status.frames_pushed,
            },
            ccd_active: active,
        }))
    }
    /// Whether Windows assigned a swap chain to the owned virtual display;
    /// None for a physical display or a status read that failed.
    pub fn monitor_status(&self) -> Option<DisplayStatus> {
        let display = self.virtual_display.as_ref()?;
        let status = crate::lock_unpoisoned(display).status().ok()?;
        Some(DisplayStatus {
            swap_chain: status.flags & rubylight_vdisplay_proto::control::MONITOR_SWAP_CHAIN != 0,
            ring: status.flags & rubylight_vdisplay_proto::control::MONITOR_RING != 0,
            frames_acquired: status.frames_acquired,
            frames_pushed: status.frames_pushed,
        })
    }
    /// Forward the display driver's queued log lines, for a first-frame
    /// failure that the host could not explain from its own log.
    pub fn drain_display_log(&self) {
        if let Some(display) = &self.virtual_display {
            crate::lock_unpoisoned(display).drain_log();
        }
    }
    /// A paused game retains its display for reconnect, but must not switch
    /// it on or restore mode/HDR after the user changed the local layout.
    pub fn keep_alive(&mut self) -> Result<()> {
        if let Some(display) = &self.virtual_display {
            crate::lock_unpoisoned(display).keep_alive()?;
        }
        Ok(())
    }
    /// Repair and publish a paused display before its stream resumes.
    pub fn resume(&mut self) -> Result<bool> {
        if let Some(display) = &self.virtual_display {
            crate::lock_unpoisoned(display).recover_now()?;
        }
        self.feed()
    }
}
/// Bring a recovered virtual display's HDR state and mode back to the
/// stream's: Windows recalls what it saved for the display when it returns.
fn restore_recovered(
    display: &mut VirtualDisplay,
    identity: &str,
    hdr: Option<bool>,
) -> Result<()> {
    if let Some(hdr) = hdr {
        let chosen = monitors()?
            .into_iter()
            .find(|m| display.owns_monitor(m))
            .context("recovered display unavailable")?;
        let mut settings = crate::lock_unpoisoned(&SETTINGS);
        if let Some(color) = settings.get_mut(identity).and_then(|s| s.color.as_mut()) {
            color.0 = chosen.clone();
        }
        if chosen.hdr_enabled != hdr {
            crate::display_recovery::hdr(identity, &chosen.display_name, chosen.hdr_enabled, hdr)?;
            let state = settings
                .get_mut(identity)
                .context("recovered display has no settings lease")?;
            if state.color.is_none() {
                state.color = Some((chosen.clone(), chosen.hdr_enabled, hdr));
            }
            set_hdr(&chosen, hdr)?;
        }
    }
    display.finish_hotplug("after recovery settings")
}
impl Drop for Guard {
    fn drop(&mut self) {
        let mut all = crate::lock_unpoisoned(&SETTINGS);
        let Some(settings) = all.get_mut(&self.identity) else {
            return;
        };
        settings.users -= 1;
        if settings.users != 0 {
            return;
        }
        let settings = all.remove(&self.identity).unwrap();
        // A physical display that is off or unplugged now keeps its journal
        // entry: Windows remembers HDR and modes per display, and the next
        // stream on it or host start restores them once it is back.
        let now = monitors().ok();
        let restored = restore_settings(
            &settings,
            &self.identity,
            &self.output,
            self.virtual_display.is_some(),
            now.as_deref(),
            set_hdr,
            |previous, applied| {
                let current_monitors;
                let list = match now.as_deref() {
                    Some(list) => list,
                    None => {
                        current_monitors = monitors()?;
                        &current_monitors
                    }
                };
                restore_mode_rate_with(list, &self.identity, previous, applied)
            },
            |previous, applied| {
                crate::display_recovery::put_back(
                    rotation(&self.identity),
                    previous,
                    applied,
                    |previous| set_rotation(&self.identity, previous),
                )
            },
            |previous, applied| {
                let list;
                let monitors = match now.as_deref() {
                    Some(list) => list,
                    None => {
                        list = monitors()?;
                        &list
                    }
                };
                let monitor = monitors
                    .iter()
                    .find(|m| m.device_id == self.identity)
                    .context("display is unavailable for its scale")?;
                crate::display_recovery::put_back(
                    dpi_scale(monitor),
                    previous,
                    applied,
                    |previous| {
                        set_dpi_scale(monitor, previous)?;
                        crate::display_recovery::settle_scale(&self.identity, previous)
                    },
                )
            },
        );
        if restored && let Err(e) = crate::display_recovery::release(&self.identity) {
            tracing::warn!(error=%e,"display recovery journal cleanup failed");
        }
    }
}
