//! Whether and where Windows shows a display: the driver's lease answers,
//! the waits for a monitor or a mode, the heartbeat's decision, and finding
//! the display among Windows' monitors.
use super::*;

pub(super) fn lease_exists(result: Result<()>) -> Result<bool> {
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
pub(super) fn retry_busy<T>(
    mut attempt: impl FnMut() -> Result<T>,
    timeout: Duration,
) -> Result<T> {
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
pub(super) fn wait_listed(mut listed: impl FnMut() -> bool, timeout: Duration) -> Option<u32> {
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
pub(super) fn wait_for_monitor(
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
pub(super) enum Heartbeat {
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
pub(super) fn heartbeat(
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
pub(super) fn listed<'a>(
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
pub(super) fn cloned_with_physical(monitors: &[Monitor], display: &Monitor) -> bool {
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
pub(super) fn connected(routes: &[Route]) -> bool {
    Topology::query_all().is_ok_and(|all| {
        all.paths.iter().any(|p| {
            p.targetInfo.targetAvailable.as_bool()
                && routes.contains(&(p.targetInfo.adapterId, p.targetInfo.id))
        })
    })
}
/// Why a display Windows keeps duplicated on a physical display is refused.
pub(super) fn still_duplicated(activation: &str) -> String {
    format!(
        "virtual display stays duplicated with a physical display; moving it to a source of its own failed (switching it on: {activation})"
    )
}
/// The monitor Windows lists for the display's device path, under whatever
/// adapter and target it goes by now. The path is the display's identity; an
/// empty one names nothing.
pub(super) fn listed_by_path(monitors: Vec<Monitor>, path: &str) -> Option<Monitor> {
    monitors
        .into_iter()
        .find(|m| !path.is_empty() && m.monitor_device_path.eq_ignore_ascii_case(path))
}
/// Why the display did not come up, with what the last attempt to switch it
/// on came to.
pub(super) fn not_active(connected: bool, activation: &str) -> String {
    if connected {
        format!(
            "virtual display was connected but Windows kept it switched off (switching it on: {activation})"
        )
    } else {
        "virtual display did not become active before the deadline".into()
    }
}
