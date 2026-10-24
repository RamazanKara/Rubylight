//! Display self-test. `butterpollo-service.exe --display-self-test REPORT`, run as SYSTEM,
//! starts `butterpollo.exe --display-self-test REPORT` in the signed-in console session. The
//! desktop changes for about half a minute and is then restored.
//!
//! 1. A virtual display that Windows leaves off is switched on by the host,
//!    and the other displays keep their modes and positions.
//! 2. Two more virtual displays stand in for TVs in duplicate mode, as in
//!    issue #4, and a stream's virtual display still comes up.
//! 3. A saved layout with that clone group is restored after the group was
//!    broken up.
use super::topology::activate_target;
use super::*;
use serde_json::{Value, json};

/// Device, width, height, refresh and desktop position.
type Timing = (String, u32, u32, u32, i32, i32);

/// Sorted by device: Windows may list the same displays in another order
/// after one arrives.
fn timings(skip: impl Fn(&Monitor) -> bool) -> Result<Vec<Timing>> {
    let mut timings = monitors()?
        .iter()
        .filter(|m| !skip(m))
        .map(|m| {
            let mode = mode(&m.display_name)?;
            // SAFETY: mode() returns an initialized display DEVMODEW, whose display
            // union member contains dmPosition.
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
        .collect::<Result<Vec<_>>>()?;
    timings.sort();
    Ok(timings)
}

fn active_ids() -> Result<Vec<String>> {
    Ok(monitors()?.into_iter().map(|m| m.device_id).collect())
}

fn cloned(a: &str, b: &str) -> Result<bool> {
    Ok(Topology::query()?
        .clone_groups()
        .iter()
        .any(|group| group.iter().any(|id| id == a) && group.iter().any(|id| id == b)))
}

/// What Windows does when a layout it saved leaves the display off.
fn switch_off(target: &Monitor) -> Result<()> {
    let active = Topology::query()?;
    let on: Vec<_> = active
        .paths
        .iter()
        .filter(|p| !(p.targetInfo.adapterId == target.adapter && p.targetInfo.id == target.target))
        .copied()
        .collect();
    // SAFETY: The initialized paths and modes from QueryDisplayConfig remain live
    // throughout the synchronous call; filtering preserves their mode indices.
    check(unsafe {
        SetDisplayConfig(
            Some(&on),
            Some(&active.modes),
            SDC_APPLY | SDC_USE_SUPPLIED_DISPLAY_CONFIG | SDC_ALLOW_CHANGES,
        )
    })
}

/// The check and the test display's device, which disappears asynchronously.
fn switch_on() -> Result<(Value, String)> {
    let display = VirtualDisplay::create("butterpollo-self-test-a", 1280, 720, 60)?;
    let target = display
        .resolved_target
        .clone()
        .context("virtual display unresolved")?;
    // Windows lists it under another adapter and target while it is on than
    // while it is off on some systems.
    let routes = display.routes()?;
    let ours = |m: &Monitor| routes.contains(&(m.adapter, m.target));
    let before = timings(ours)?;
    switch_off(&target)?;
    anyhow::ensure!(
        !monitors()?.iter().any(ours),
        "the virtual display stayed on after switching it off"
    );
    let kept_timings = activate_target(&routes)?;
    let on = monitors()?.iter().any(ours);
    let after = timings(ours)?;
    let unchanged = after == before;
    let mut check = json!({
        "passed": on && unchanged,
        "switched_on": on,
        "kept_timings": kept_timings,
        "other_displays_unchanged": unchanged,
    });
    if !unchanged {
        check["timings"] = json!({"before": before, "after": after});
    }
    Ok((check, target.device_id))
}

/// Displays standing in for TVs; they stay, with their heartbeats, until this is dropped.
struct StandIns {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for StandIns {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Two more virtual displays, on and duplicated.
fn stand_in_tvs() -> Result<(String, String, StandIns)> {
    let mut tvs = Vec::new();
    for name in ["butterpollo-self-test-tv-a", "butterpollo-self-test-tv-b"] {
        tvs.push(VirtualDisplay::create(name, 1920, 1080, 60)?);
    }
    let ids = tvs
        .iter()
        .map(|tv| {
            tv.resolved_target
                .as_ref()
                .map(|m| m.device_id.clone())
                .context("stand-in TV unresolved")
        })
        .collect::<Result<Vec<_>>>()?;
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let feeding = stop.clone();
    let thread = std::thread::spawn(move || {
        while !feeding.load(std::sync::atomic::Ordering::Acquire) {
            for tv in &mut tvs {
                if let Err(error) = tv.feed() {
                    tracing::warn!(error = %format!("{error:#}"), "stand-in TV heartbeat failed");
                }
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    });
    let stand_ins = StandIns {
        stop,
        thread: Some(thread),
    };
    let (a, b) = (ids[0].clone(), ids[1].clone());
    let mut on = active_ids()?;
    on.extend([a.clone(), b.clone()]);
    on.sort();
    on.dedup();
    Topology::set_active(&on)?;
    Topology::query()?.restore_clone_groups(&[vec![a.clone(), b.clone()]])?;
    anyhow::ensure!(cloned(&a, &b)?, "the stand-in TVs could not be duplicated");
    Ok((a, b, stand_ins))
}

/// A dropped test display leaves asynchronously; Windows refuses a layout
/// that still names it (ERROR_INVALID_PARAMETER).
fn wait_gone(id: &str) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(15);
    while active_ids()?.iter().any(|d| d == id) {
        anyhow::ensure!(
            Instant::now() < deadline,
            "the test display did not go away"
        );
        std::thread::sleep(Duration::from_millis(250));
    }
    Ok(())
}

/// The check and the test display's device.
fn beside_duplicated_tvs(a: &str, b: &str) -> Result<(Value, String)> {
    let others = |m: &Monitor| m.device_id == a || m.device_id == b;
    let before = timings(others)?;
    let mut display = VirtualDisplay::create("butterpollo-self-test-b", 1280, 720, 60)?;
    let target = display
        .resolved_target
        .clone()
        .context("virtual display unresolved")?;
    let routes = display.routes()?;
    let ours = |m: &Monitor| routes.contains(&(m.adapter, m.target));
    let on = monitors()?.iter().any(ours);
    // The TVs may move to make room; everything else keeps its timing.
    let after = timings(|m| others(m) || ours(m))?;
    let unchanged = after == before;
    // Windows lays out the displays again when one arrives and may break up
    // the duplicate; that is reported, not failed.
    let still_cloned = cloned(a, b)?;
    // A stream's heartbeat finds the display it owns and keeps it, rather
    // than recreating it each second.
    let generation = display.generation;
    if let Some(due) = Instant::now().checked_sub(Duration::from_secs(1)) {
        display.last_feed = due;
    }
    display.feed()?;
    let kept = display.generation == generation;
    let mut check = json!({
        "passed": on && unchanged && kept,
        "stream_display_on": on,
        "switched_on_by_host": display.switched_on,
        "tvs_still_duplicated": still_cloned,
        "other_displays_unchanged": unchanged,
        "heartbeat_kept_display": kept,
    });
    if !unchanged {
        check["timings"] = json!({"before": before, "after": after});
    }
    Ok((check, target.device_id))
}

fn clone_layout_restored(a: &str, b: &str) -> Result<Value> {
    if !cloned(a, b)? {
        // As the stand-in TVs were first duplicated, and as a restore does.
        Topology::set_active(&active_ids()?).context("switching every display on")?;
        Topology::query()?
            .restore_clone_groups(&[vec![a.to_owned(), b.to_owned()]])
            .context("duplicating the stand-in TVs again")?;
    }
    let saved = Snapshot::capture()?;
    anyhow::ensure!(
        saved
            .clone_groups
            .iter()
            .any(|g| g.iter().any(|id| id == a) && g.iter().any(|id| id == b)),
        "the saved layout has no clone group"
    );
    // Give every display its own desktop, as set_active does.
    Topology::set_active(&active_ids()?).context("breaking up the clone group")?;
    let broken = !cloned(a, b)?;
    let restored = saved.restore();
    let rejoined = cloned(a, b)?;
    Ok(json!({
        "passed": broken && restored.is_ok() && rejoined,
        "clone_group_broken_first": broken,
        "restore": restored.err().map(|e| format!("{e:#}")),
        "clone_group_restored": rejoined,
    }))
}

/// Run the checks, restore the desktop and write `report`. True when every
/// check passed.
pub fn run(report: &std::path::Path) -> Result<bool> {
    let original = Snapshot::capture()?;
    let mut checks = serde_json::Map::new();
    let result = (|| -> Result<()> {
        let (check, test_display) = switch_on()?;
        checks.insert("switch_on".into(), check);
        // It leaves asynchronously; going during the next check, it would
        // count as another display that changed.
        wait_gone(&test_display)?;
        let (a, b, stand_ins) = stand_in_tvs()?;
        let (check, test_display) = beside_duplicated_tvs(&a, &b)?;
        checks.insert("beside_duplicated_tvs".into(), check);
        wait_gone(&test_display)?;
        checks.insert(
            "clone_layout_restored".into(),
            clone_layout_restored(&a, &b)?,
        );
        drop(stand_ins);
        Ok(())
    })();
    let mut cleanup = Vec::new();
    std::thread::sleep(Duration::from_secs(2));
    if let Err(error) = original.restore() {
        cleanup.push(format!("original layout: {error:#}"));
    }
    let passed = result.is_ok()
        && cleanup.is_empty()
        && checks.values().all(|check| check["passed"] == true);
    let document = json!({
        "version": env!("CARGO_PKG_VERSION"),
        "passed": passed,
        "checks": checks,
        "error": result.err().map(|e| format!("{e:#}")),
        "cleanup_errors": cleanup,
    });
    std::fs::write(report, serde_json::to_vec_pretty(&document)?)?;
    Ok(passed)
}
