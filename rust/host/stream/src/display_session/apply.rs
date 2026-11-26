//! Stream display preparation, Windows mode application, and desktop-refusal retry.
use super::baseline::GoldenLease;
use super::lifecycle::{Ready, StreamPreparation};
use super::mode_report::ModeReport;
use super::policy::{
    client_display, display_ask, display_changes, frame_policy, inherited_layout,
    joined_hdr_mismatch, joined_report, layout_owner, layout_reason, profile_choice, stream_mode,
};
use super::scale::{
    holds_layout_while_paused, kept_scale, physical_calibration, physical_scale, scale_setup,
    set_up_scale, stream_profile, virtual_display_size,
};
use super::turn::{Follow, PhysicalTurn, adjust_physical, plan_physical_turn};
use super::virtual_dd::{
    acquire_and_verify_layout, remembered_resolution, resolve_virtual_identity, select_virtual_mode,
};
use super::window_target::{configured_window, configured_window_open};
use super::{AppliedMode, Prepared};
use crate::state::{Launch, Shared};
use anyhow::{Context, Result};
use butterpollo_config::config::Config;
use butterpollo_media_policy::{
    display_session_machine::{DisplaySessionMachine, should_retry_desktop_refused},
    framegen::Rate,
    rtsp::Negotiated,
};
use butterpollo_session::session::Role;
use butterpollo_win_display::{
    display::{self, Guard},
    display_arrangement, hdr_profile,
};
use butterpollo_win_graphics::vulkan;
use butterpollo_win_io::limiter;
use std::{sync::Arc, time::Duration};

pub(super) fn applied_mode(output: &str) -> Result<AppliedMode> {
    let topology = display::Topology::query()?;
    let monitor = topology
        .monitors()
        .into_iter()
        .find(|m| m.matches(output))
        .context("selected display missing after preparation")?;
    let mode = display::mode(output)?;
    let refresh = topology.refresh(&monitor.device_id)?;
    Ok((
        mode.dmPelsWidth,
        mode.dmPelsHeight,
        refresh.0,
        monitor.hdr_enabled,
    ))
}

/// Log a layout that could not be put back once a monitor returned, and return what the
/// stream's card says about it.
pub(super) fn failed_wake_layout(error: anyhow::Error) -> String {
    let error = format!("{error:#}");
    tracing::warn!(error = %error, "laying out the stream around a monitor that returned failed");
    error
}

/// Whether a stream's end is still at work on the displays, though no stream or teardown
/// shows it any more: what reads or writes the layout outside a launch waits for that.
pub fn displays_settling() -> bool {
    #[cfg(any(test, feature = "test-support"))]
    if super::test_support::DISPLAYS_SETTLING.with(std::cell::Cell::get) {
        return true;
    }
    display_arrangement::wake_watch_settling()
}

/// Every launch and re-prepare holds this: the end of the stream before it waits for
/// nothing, and what is left of that end stops ([`display_arrangement::WakeWatchOff`]).
pub(crate) fn hold_off_wake_watch() -> display_arrangement::WakeWatchOff {
    #[cfg(test)]
    super::tests::WAKE_WATCH_HOLDS.with(|holds| holds.set(holds.get() + 1));
    display_arrangement::hold_off_wake_watch()
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
    let _wake_watch = hold_off_wake_watch();
    if let Some(target) = configured_window(config)? {
        configured_window_open(&target)?;
        tracing::info!(target = %target, "streaming the configured window");
        let name = target.to_string();
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
        butterpollo_win_io::input::on_input_desktop(|| {
            Ready::prepare(Prepared::create(h, launch, stream, config, physical_only)?)
        })
    };
    let prepared = retry_refused_layout(|| prepare(false));
    match prepared {
        Err(error) if butterpollo_win_io::input::secure_desktop_shown() => {
            tracing::warn!(error = %format!("{error:#}"), "stream display could not be prepared while Windows is locked; trying the physical display");
            prepare(true).map_err(|fallback| {
                tracing::warn!(error = %format!("{fallback:#}"), "physical display could not be prepared either");
                error
            })
        }
        result => result,
    }
}

/// How long a launch waits before preparing its display again after Windows
/// refused the layout to this desktop.
const DESKTOP_REFUSED_RETRY: Duration = Duration::from_secs(2);

/// Runs `attempt`, and once more after `wait` when Windows `refused` the layout to this
/// desktop while no lock or sign-in screen is shown (`secure`).
pub(super) fn retry_refused<T>(
    mut attempt: impl FnMut() -> Result<T>,
    refused: impl Fn(&anyhow::Error) -> bool,
    secure: impl Fn() -> bool,
    wait: impl FnOnce(),
) -> Result<T> {
    match attempt() {
        Err(error) if should_retry_desktop_refused(refused(&error), secure()) => {
            tracing::warn!(
                error = %format!("{error:#}"),
                input_desktop = ?butterpollo_win_io::input::input_desktop_name(),
                "Windows refused the stream display's layout to this desktop; trying again"
            );
            wait();
            attempt()
        }
        result => result,
    }
}

/// [`retry_refused`] with Windows' own answers and [`DESKTOP_REFUSED_RETRY`]: a launch's
/// display and a kept display's resume alike.
pub(super) fn retry_refused_layout<T>(attempt: impl FnMut() -> Result<T>) -> Result<T> {
    retry_refused(
        attempt,
        butterpollo_win_io::input::desktop_refused,
        butterpollo_win_io::input::secure_desktop_shown,
        || std::thread::sleep(DESKTOP_REFUSED_RETRY),
    )
}

impl Prepared {
    pub(super) fn create(
        h: &Shared,
        launch: &Launch,
        stream: &Negotiated,
        config: &Config,
        physical_only: bool,
    ) -> Result<(Self, limiter::Lease)> {
        butterpollo_win_io::wake::wake_console_if_idle("launch");
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
        let client_display = client_display(launch);
        let background = butterpollo_session::background::enabled(config);
        let selection = select_virtual_mode(
            launch,
            app.as_ref(),
            config,
            client_display,
            background,
            output_override,
            output,
            physical_only,
        )?;
        let virtual_mode = selection.virtual_mode;
        let output = selection.output.as_str();
        let (framegen, device_mode) =
            frame_policy(app.as_ref(), launch, stream, config, virtual_mode)?;
        let limiter = limiter::Lease::acquire(&h.directory, config, &framegen)?;
        let vulkan = if stream.hdr && config.boolean("vulkan_hdr_layer", true) {
            Some(vulkan::Lease::acquire()?)
        } else {
            None
        };
        let vid = resolve_virtual_identity(
            h,
            launch,
            app.as_ref(),
            config,
            virtual_mode,
            selection.mode,
        )?;
        let joined_mode = vid.joined_mode;
        let shares = vid.shares;
        let shared_id = vid.shared_id;
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
        let (display_width, display_height) = stream.display_size();
        let (render_width, render_height) =
            butterpollo_media_policy::display_policy::render_dimensions(
                display_width,
                display_height,
                client_scale,
                app_scale,
            );
        let mut request = butterpollo_media_policy::display_policy::display_request_rate(
            config,
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
        let (remember, saved_res) =
            remembered_resolution(launch, virtual_mode, shares, device_mode.is_some());
        if let Some(resolution) = saved_res {
            request.resolution = Some(resolution);
        }
        let (width, height) = if virtual_mode {
            virtual_display_size(
                config,
                request.resolution.unwrap_or((render_width, render_height)),
                (display_width, display_height),
            )
        } else {
            (display_width, display_height)
        };
        let own_rate = if virtual_mode {
            Rate(request.refresh.unwrap_or(framegen.display_rate.0))
        } else {
            framegen.display_rate
        };
        let ((width, height), rate, display_hdr) =
            display_ask(joined_mode, (width, height), own_rate, request.hdr);
        struct RemoteMonitorRollback<'a> {
            host: &'a Shared,
            client: &'a str,
            armed: bool,
        }
        impl Drop for RemoteMonitorRollback<'_> {
            fn drop(&mut self) {
                if self.armed {
                    crate::remote_display::disconnect(self.host, Some(self.client));
                }
            }
        }
        let (retained, created_remote) = if launch.role == Role::RemoteMonitor {
            let (lease, created) =
                crate::remote_display::activate(h, &launch.client.uuid, stream, &launch.warnings)?;
            (Some(lease), created)
        } else {
            (None, false)
        };
        let mut remote_rollback = RemoteMonitorRollback {
            host: h,
            client: &launch.client.uuid,
            armed: created_remote,
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
        let physical = !virtual_mode && retained.is_none();
        let follow = (config.dd_rotation_option()
            == butterpollo_config::config::DisplayAdjust::Auto
            && retained.is_none()
            && joined_mode.is_none())
        .then_some(if virtual_mode {
            Follow::Virtual
        } else {
            Follow::Physical
        });
        let turn = if physical {
            plan_physical_turn(
                config,
                (display_width, display_height),
                request.resolution,
                || display::stream_display_rotation(output),
            )?
        } else {
            PhysicalTurn {
                before: request.resolution,
                after: request.resolution,
                ..Default::default()
            }
        };
        if request.prefer_highest && physical {
            request.refresh = Some(display::highest_refresh(output, turn.before)?.0);
        }
        let virtual_options = display::VirtualOptions {
            peak_nits: config.integer("hdr_peak_brightness", 1000).clamp(400, 2000) as u32,
            render_adapter: if virtual_mode {
                crate::win_bridge::configured_render_adapter(config)
            } else {
                None
            },
            render_resolver: virtual_mode
                .then(|| crate::win_bridge::render_resolver(config, Some(launch.warnings.clone()))),
            joins: joined_mode.is_some(),
        };
        let original = if virtual_mode && retained.is_none() && launch.role == Role::Stream {
            display::Snapshot::capture()
                .inspect_err(|error| {
                    tracing::warn!(error = %format!("{error:#}"), "display layout before the virtual display could not be read")
                })
                .ok()
        } else {
            None
        };
        let changes = display_changes(
            joined_mode.is_some(),
            stream.hdr,
            virtual_mode || retained.is_some(),
        );
        let recovery_profile = stream_profile(
            changes.hdr_profile,
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
                &shared_id,
                width,
                height,
                rate,
                display_hdr,
                turn.before,
                request.refresh.map(Rate),
                &virtual_options,
            )?)
        } else {
            None
        };
        if physical && let Some(guard) = display.as_mut() {
            adjust_physical(guard, turn.degrees, physical_scale(&launch.client))?;
        }
        let output = retained
            .as_ref()
            .map(|d| d.current_output())
            .or_else(|| display.as_ref().map(|d| d.output.clone()))
            .context("display lease unavailable")?;
        let virtual_layout = virtual_mode || retained.is_some();
        let setup = scale_setup(
            config,
            &launch.options,
            changes.scale,
            remember && display.is_some(),
            || display::largest_scale(&output),
        )?;
        let remembered = kept_scale(remember, &launch.client.extra);
        let (scale, scale_baseline) = set_up_scale(
            &output,
            setup.scale,
            setup.requested,
            remembered,
            (width, height),
            setup.learns,
            display::virtual_scale,
            display::output_scale,
        )?;
        match setup.warning(scale) {
            Some(warning) => launch.warnings.set("display_scale", warning),
            None => launch.warnings.clear("display_scale"),
        }
        let layout = acquire_and_verify_layout(
            h,
            launch,
            app.as_ref(),
            config,
            client_display,
            virtual_mode,
            virtual_layout,
            &output,
            display.as_ref(),
            changes.mode,
            original,
        )?;
        let in_force = layout
            .arrangement
            .as_ref()
            .map(display_arrangement::Lease::arrangement);
        tracing::info!(
            client = %launch.client.name,
            role = ?launch.role,
            client_virtual_display = ?selection.client_virtual,
            client_display = ?client_display,
            virtual_display_mode = selection.mode,
            output_override = ?output_override,
            virtual_display = virtual_mode,
            output = %output,
            layout = layout.selection,
            layout_reason = layout_reason(layout.monitors_kept, layout.configured).as_deref(),
            arrangement = ?layout.parsed,
            requested_refresh_millihz = request.refresh.or(virtual_mode.then_some(rate.0)),
            limiter_rate = %framegen.rate,
            limiter_enabled = framegen.enabled,
            "stream display selection"
        );
        let requested = if joined_mode.is_some() {
            joined_report(own_rate, request.hdr)
        } else if virtual_mode {
            (
                Some((width, height)),
                request.refresh.map(|_| rate.0),
                request.hdr,
            )
        } else {
            (turn.after, request.refresh, request.hdr)
        };
        let mut mode_report =
            ModeReport::new(launch.warnings.clone(), requested, stream.fps_millihz());
        mode_report.unrepaired_at_start(
            layout
                .arrangement
                .as_ref()
                .and_then(|lease| lease.repair_failure()),
        );
        if virtual_mode && shares {
            mode_report = mode_report.shared();
        }
        let applied = applied_mode(&output);
        if let Some(wanted) = joined_hdr_mismatch(
            joined_mode.is_some(),
            request.hdr,
            applied.as_ref().ok().map(|actual| actual.3),
        ) {
            tracing::warn!(
                client = %launch.client.name,
                output = %output,
                wanted,
                "this stream joined a running game's display, whose HDR stays as it is: an HDR stream carries the SDR picture without highlights, an SDR stream the HDR picture limited to SDR"
            );
        }
        mode_report.publish(applied);
        let revision = retained.as_ref().map_or(0, |r| r.capture_target().1);
        let owns_display = display.is_some();
        let hold_layout_while_paused = holds_layout_while_paused(
            config,
            layout
                .arrangement
                .as_ref()
                .is_some_and(display_arrangement::Lease::exclusive),
        );
        // Assemble `Prepared` before acquiring the HDR profile so an ICC failure
        // runs `Prepared::drop` (`lease.end((display, activation, golden))`)
        // instead of unwinding locals in reverse declaration order (T2-1).
        let mut prepared = Self {
            display,
            output,
            framegen,
            _arrangement: layout.arrangement,
            _activation: activation,
            _profile: None,
            _retained: retained,
            _vulkan: vulkan,
            _golden: golden,
            mode: stream_mode(stream),
            layout_owner: layout_owner(
                launch,
                virtual_mode && !background && client_display.is_none() && layout.layout_took,
                layout.monitors_kept,
                layout.parsed,
                in_force,
            ),
            client_display,
            virtual_layout: virtual_layout.then(|| layout.selection.to_owned()),
            inherited_layout: virtual_layout
                .then(|| inherited_layout(config, app.as_ref()).to_owned()),
            resolution_owner: (remember && owns_display).then(|| launch.client.uuid.clone()),
            scale_baseline,
            follow,
            identity: (virtual_mode && owns_display).then(|| shared_id.clone()),
            joined: joined_mode.is_some(),
            #[cfg(any(test, feature = "test-support"))]
            layout_snapshot: None,
            #[cfg(test)]
            size_snapshot: None,
            #[cfg(test)]
            scale_snapshot: None,
            #[cfg(test)]
            scales_set: std::sync::Mutex::new(Vec::new()),
            revision,
            machine: DisplaySessionMachine::launch(
                std::time::Instant::now(),
                display_arrangement::departures(),
                display_arrangement::wake_layouts(),
                layout.start_primary_mismatch,
                hold_layout_while_paused,
            ),
            mode_report,
            recovery_scale: scale,
            recovery_dimensions: (width, height),
            recovery_profile,
            virtual_mode,
            profile_choice: profile_choice(launch),
            host: Arc::downgrade(h),
        };
        prepared._profile = prepared
            .recovery_profile
            .as_ref()
            .map(|p| hdr_profile::Lease::acquire(&prepared.output, p))
            .transpose()?;
        remote_rollback.armed = false;
        Ok((prepared, limiter))
    }
}
