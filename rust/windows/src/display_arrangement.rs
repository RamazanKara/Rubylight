//! Temporary exclusive/primary/isolated layouts with shared and crash restoration.
use crate::display::{Snapshot, Topology};
use anyhow::{Result, bail};
use butterpollo_core::{display_policy::Arrangement, topology::Node};
use std::sync::{
    Mutex, OnceLock,
    atomic::{AtomicU64, Ordering},
};
#[derive(Default)]
struct State {
    users: usize,
    key: String,
    before: Option<Snapshot>,
    applied: Vec<Node>,
    /// Streamed displays in the order their streams arrived, with their lease
    /// counts. The first is laid out as for a single stream.
    owners: Vec<(String, usize)>,
    arrangement: Option<Arrangement>,
    retained: Vec<String>,
    /// The layout is saved to the Windows display database (see `persist`).
    persisted: bool,
    /// The owners that layout is saved for: virtual displays made for streaming.
    /// A remote monitor or a physical display among the owners has none, and
    /// without one a save would change an entry nothing puts back.
    persists_for: Vec<String>,
}
fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(Mutex::default)
}
pub fn matches(nodes: &[Node], applied: &[Node]) -> bool {
    matches_connected(nodes, applied, None)
}
/// Like `matches`, but a display that is no longer connected (a TV switched
/// off at the wall, an unplugged monitor) does not count as a change by the
/// user: the rest of the layout is still the one the stream applied.
fn matches_connected(
    nodes: &[Node],
    applied: &[Node],
    connected: Option<&std::collections::BTreeSet<String>>,
) -> bool {
    let present = |id: &str| connected.is_none_or(|c| c.contains(id));
    let active: std::collections::BTreeSet<_> = nodes
        .iter()
        .filter(|n| n.active)
        .map(|n| n.device_id.as_str())
        .collect();
    let expected: std::collections::BTreeSet<_> = applied
        .iter()
        .filter(|n| n.active && present(&n.device_id))
        .map(|n| n.device_id.as_str())
        .collect();
    active == expected
        && nodes.iter().all(|n| {
            applied
                .iter()
                .any(|a| a.device_id == n.device_id && a.desired_position == n.desired_position)
        })
}
/// Whether the displays still have the layout a stream applied.
pub fn unchanged(applied: &[Node]) -> Result<bool> {
    let current = Topology::query()?.nodes()?;
    let connected = Topology::query_all()?
        .monitors()
        .into_iter()
        .map(|m| m.device_id)
        .collect();
    Ok(matches_connected(&current, applied, Some(&connected)))
}
/// An arrangement that disabled an original display must put the original
/// layout back even if Win+P or Windows recovery changed it in the meantime.
/// Non-exclusive arrangements continue to preserve later user changes.
pub fn disabled_original_displays(before: &[Node], applied: &[Node]) -> bool {
    before.iter().filter(|node| node.active).any(|original| {
        !applied
            .iter()
            .any(|node| node.device_id == original.device_id && node.active)
    })
}
pub fn restore_arrangement(before: &Snapshot, applied: &[Node]) -> Result<()> {
    if disabled_original_displays(&before.nodes, applied) || unchanged(applied)? {
        before.restore()?;
    }
    Ok(())
}
/// Tell the layouts streams hold that a remote monitor's display was
/// switched on (`on`) or is about to be removed. Otherwise a stream's end
/// finds a layout it did not apply, takes it for the user's own and leaves
/// the physical displays off, and a restore would switch the remote monitor
/// off. `output`: its GDI name or device id.
pub fn retain(output: &str, on: bool) -> Result<()> {
    let arranged = state().lock().unwrap().users != 0;
    let activated = activation_state().lock().unwrap().users != 0;
    if !arranged && !activated {
        return Ok(());
    }
    let id = crate::display::monitors()
        .ok()
        .and_then(|all| all.into_iter().find(|m| m.matches(output)))
        .map_or_else(|| output.to_owned(), |m| m.device_id);
    let current = if on {
        Topology::query()?.nodes()?
    } else {
        Vec::new()
    };
    let mut result = Ok(());
    {
        let mut state = state().lock().unwrap();
        if retain_in(&mut state, &current, &id, on)
            && let Some(before) = &state.before
        {
            result =
                crate::display_recovery::arrangement(Some((before.clone(), state.applied.clone())));
        }
    }
    let mut state = activation_state().lock().unwrap();
    if retain_in(&mut state, &current, &id, on)
        && let Some(before) = &state.before
    {
        result = result.and(crate::display_recovery::activation(Some((
            before.clone(),
            state.applied.clone(),
        ))));
    }
    result
}
/// Record `id` in a held layout. Switched on, it is expected active where it
/// is now and restored active with the layout from before the stream; going
/// away, it is only no longer kept on. Its node stays: a display that is gone
/// does not count as a change, and one still there until its lease ends
/// must not either. True when the recorded layout changed.
fn retain_in(state: &mut State, current: &[Node], id: &str, on: bool) -> bool {
    if state.users == 0 {
        return false;
    }
    if !on {
        state.retained.retain(|r| r != id);
        return false;
    }
    let Some(node) = current.iter().find(|n| n.device_id == id) else {
        return false;
    };
    match state.applied.iter_mut().find(|n| n.device_id == id) {
        Some(applied) => *applied = node.clone(),
        None => state.applied.push(node.clone()),
    }
    if let Some(before) = &mut state.before {
        match before.nodes.iter_mut().find(|n| n.device_id == id) {
            Some(original) => original.active = true,
            None => before.nodes.push(node.clone()),
        }
    }
    if !state.retained.iter().any(|r| r == id) {
        state.retained.push(id.to_owned());
    }
    true
}
fn connected() -> Result<Vec<String>> {
    Ok(Topology::query_all()?
        .monitors()
        .into_iter()
        .map(|m| m.device_id)
        .collect())
}
/// Keep the displays a held layout switched off in the newly composed one. A
/// snapshot lists only the displays that are on, so a layout composed while
/// they are off would forget them, and [`Lease::switched_back_on`] would no
/// longer see Windows switching them on again.
fn keep_switched_off(desired: &mut Vec<Node>, applied: &[Node]) {
    for node in applied.iter().filter(|node| !node.active) {
        if !desired.iter().any(|d| d.device_id == node.device_id) {
            desired.push(node.clone());
        }
    }
}
/// How many streams' displays have left a layout other streams still hold.
static DEPARTURES: AtomicU64 = AtomicU64::new(0);
/// Compare it to see that a display left while this stream goes on.
pub fn departures() -> u64 {
    DEPARTURES.load(Ordering::Relaxed)
}
/// When a display last left, with the windows minimised just before.
type Departure = (std::time::Instant, std::collections::BTreeSet<isize>);
static LAST_DEPARTURE: Mutex<Option<Departure>> = Mutex::new(None);
/// How long after a display left a window minimised since is put down to it:
/// later, the user is as likely to have done it.
const DEPARTURE_RECENT: std::time::Duration = std::time::Duration::from_secs(10);
/// Note a display leaving. One that follows another soon after keeps the first
/// reading: a window the first one minimised was not the user's doing.
fn note_departure(
    last: &mut Option<Departure>,
    now: std::time::Instant,
    minimized: impl FnOnce() -> std::collections::BTreeSet<isize>,
) {
    match last
        .as_mut()
        .filter(|(at, _)| now.saturating_duration_since(*at) < DEPARTURE_RECENT)
    {
        // The earlier reading stands: what the first display minimised is no user's
        // doing. The streams handle this departure later, so the time moves on.
        Some((at, _)) => *at = now,
        None => *last = Some((now, minimized())),
    }
}
fn recent_departure(
    last: &Option<Departure>,
    now: std::time::Instant,
) -> Option<&std::collections::BTreeSet<isize>> {
    last.as_ref()
        .filter(|(at, _)| now.saturating_duration_since(*at) < DEPARTURE_RECENT)
        .map(|(_, minimized)| minimized)
}
/// The windows that were minimised before a display left, while that departure
/// is recent: any other minimised window is its doing. None after that.
pub fn minimized_before_departure() -> Option<std::collections::BTreeSet<isize>> {
    recent_departure(&LAST_DEPARTURE.lock().unwrap(), std::time::Instant::now()).cloned()
}
/// The streams' displays that are not on (a snapshot lists only the ones that are).
fn targets_off(nodes: &[Node], targets: &[String]) -> Vec<String> {
    targets
        .iter()
        .filter(|t| !nodes.iter().any(|n| n.device_id == **t && n.active))
        .cloned()
        .collect()
}
fn active_ids(nodes: &[Node]) -> std::collections::BTreeSet<String> {
    nodes
        .iter()
        .filter(|n| n.active)
        .map(|n| n.device_id.clone())
        .collect()
}
/// Lay out the current owners' displays and record the result for recovery.
/// `leaving` is switched off unless it was on before streaming started.
fn apply(state: &mut State, arrangement: Arrangement, leaving: Option<&str>) -> Result<()> {
    let mut current = Snapshot::capture()?;
    let targets: Vec<_> = state.owners.iter().map(|(t, _)| t.clone()).collect();
    // Windows can drop a virtual display that has just arrived beside another
    // one while it rearranges them: on again, or there is nothing to lay out.
    let off = targets_off(&current.nodes, &targets);
    if !off.is_empty() {
        tracing::info!(displays = ?off, "switching a stream's display back on before laying it out");
        let mut ids = active_ids(&current.nodes);
        ids.extend(off);
        Topology::set_active(&ids.into_iter().collect::<Vec<_>>())?;
        current = Snapshot::capture()?;
    }
    let (mut desired, arriving) = compose_around_arrivals(state, arrangement, &current.nodes)?;
    if let Some(leaving) = leaving {
        let was_active = state.before.as_ref().is_some_and(|before| {
            before
                .nodes
                .iter()
                .any(|n| n.device_id == leaving && n.active)
        });
        if !was_active {
            for node in desired.iter_mut().filter(|n| n.device_id == leaving) {
                node.active = false;
                node.primary = false;
            }
        }
    }
    keep_switched_off(&mut desired, &state.applied);
    if let Some(before) = &state.before {
        crate::display_recovery::arrangement(Some((before.clone(), desired.clone())))?;
    }
    let ids = active_ids(&desired);
    if ids != active_ids(&current.nodes) {
        Topology::set_active(&ids.into_iter().collect::<Vec<_>>())?;
    }
    Topology::query()?.set_positions(
        &desired
            .iter()
            .filter(|n| n.active)
            .map(|n| (n.device_id.clone(), n.desired_position))
            .collect(),
    )?;
    settle_layout(state, desired, leaving, &arriving, Topology::save_current);
    Ok(())
}
/// The displays on now that the held layout has no place for: one on its way to a
/// stream's lease, a remote monitor's not retained yet, one the user just switched
/// on. The streams' heartbeats lay out outside the launch that brings them, and a
/// running stream must never lose its display to that.
fn arriving_displays(state: &State, current: &[Node]) -> Vec<String> {
    current
        .iter()
        .filter(|n| {
            n.active
                && !state.applied.iter().any(|a| a.device_id == n.device_id)
                && !state.owners.iter().any(|(id, _)| *id == n.device_id)
                && !state.retained.contains(&n.device_id)
        })
        .map(|n| n.device_id.clone())
        .collect()
}
/// The layout for the current owners with what is arriving left on, and what that is.
fn compose_around_arrivals(
    state: &State,
    arrangement: Arrangement,
    current: &[Node],
) -> Result<(Vec<Node>, Vec<String>)> {
    let targets: Vec<_> = state.owners.iter().map(|(t, _)| t.clone()).collect();
    let arriving = arriving_displays(state, current);
    let mut keep = state.retained.clone();
    keep.extend(arriving.iter().cloned());
    let desired = arrangement.compose_all_after_hotplug(
        current,
        &targets,
        &keep,
        state.before.as_ref().map_or(&[], |before| &before.nodes),
    )?;
    Ok((desired, arriving))
}
/// Record the layout just applied, and save it for Windows to recall if it may be.
/// What is `arriving` stays out of the record, as it is still arriving the next time
/// this lays out, and nothing is saved while it does: its own layout does that.
fn settle_layout(
    state: &mut State,
    mut desired: Vec<Node>,
    leaving: Option<&str>,
    arriving: &[String],
    save: impl FnOnce() -> Result<()>,
) {
    desired.retain(|n| !arriving.contains(&n.device_id));
    state.applied = desired;
    if arriving.is_empty()
        && saves_layout(state, leaving)
        && let Err(error) = save()
    {
        tracing::warn!(error = %format!("{error:#}"), "saving the stream layout for Windows to recall failed");
    }
}
/// Whether the layout just applied is saved. A display on its way out is still
/// connected, so saving now would file the layout without it under the entry for
/// both displays, and Windows would recall that the next time they meet: the first
/// stream's display off when the second one arrives. Nothing puts that entry back
/// at the end of the stream. Nor is it saved once no virtual display it persists
/// for is left: the entry would be for the user's displays, or for those and a
/// remote monitor's, with the user's off, and Windows would recall it when the
/// next stream's display goes.
fn saves_layout(state: &State, leaving: Option<&str>) -> bool {
    leaving.is_none() && state.persisted && !state.persists_for.is_empty()
}
/// A new display joins the owners: its entry is saved if it is a virtual display
/// on a layout that saves one.
fn add_owner(state: &mut State, target: &str, virtual_target: bool) {
    state.owners.push((target.to_owned(), 1));
    if state.persisted && virtual_target {
        state.persists_for.push(target.to_owned());
    }
}
/// Undo [`add_owner`]: the layout with that display could not be applied.
fn remove_added_owner(state: &mut State, target: &str) {
    state.owners.retain(|(t, _)| t != target);
    state.persists_for.retain(|id| id != target);
}
/// One lease on `target` ended. `left`: it was the display's last, and the display
/// is no longer an owner. `saved`: the layout was saved for it, so its end has an
/// entry to put back.
#[derive(Debug, PartialEq)]
struct Released {
    left: bool,
    saved: bool,
}
fn release_owner(state: &mut State, target: &str) -> Released {
    let saved = state.persisted && state.persists_for.iter().any(|id| id == target);
    let mut left = false;
    if let Some(index) = state.owners.iter().position(|(t, _)| t == target) {
        state.owners[index].1 -= 1;
        if state.owners[index].1 == 0 {
            state.owners.remove(index);
            state.persists_for.retain(|id| id != target);
            left = true;
        }
    }
    Released { left, saved }
}
/// A virtual display arrived beside the running streams' on a layout that keeps
/// its entry in the Windows display database. Settle that entry now, before
/// anything makes Windows recall it: one saved earlier can switch the running
/// stream's display off, or the new one.
pub fn display_arrived(device_id: &str) -> Result<()> {
    settle_arrival(
        &state().lock().unwrap(),
        device_id,
        Topology::set_active,
        Topology::save_current,
    )
}
/// Switch on what is on once `device_id` joined, and save that only as any other
/// layout is ([`saves_layout`]): with no virtual display it persists for left, nothing
/// would ever take that entry back, and Windows would recall the user's monitors off.
fn settle_arrival(
    state: &State,
    device_id: &str,
    mut set_active: impl FnMut(&[String]) -> Result<()>,
    mut save: impl FnMut() -> Result<()>,
) -> Result<()> {
    if state.users == 0 || !state.persisted {
        return Ok(());
    }
    let ids = arrived_layout(&state.owners, &state.retained, device_id);
    settled(|| {
        set_active(&ids)?;
        if saves_layout(state, None) {
            save()
        } else {
            Ok(())
        }
    })
}
/// What is on once `device_id` joined: the streams' displays, what they keep on, and it.
fn arrived_layout(owners: &[(String, usize)], retained: &[String], device_id: &str) -> Vec<String> {
    let mut ids: Vec<String> = owners.iter().map(|(id, _)| id.clone()).collect();
    ids.extend(retained.iter().cloned());
    ids.push(device_id.to_owned());
    ids.sort();
    ids.dedup();
    ids
}
/// Whether to save a stream's layout to the Windows display database. Windows
/// recalls the saved layout for the connected displays when an
/// exclusive-fullscreen game loses focus (the Win key, Alt+Tab), which would
/// switch the physical monitor back on. Only a layout with the stream's own
/// virtual display: the database entry is keyed by the connected displays, so
/// this changes only the entry for the user's displays plus that virtual
/// display, never the one for the user's displays alone. The entry is put
/// back when the last stream ends (`forget_saved_layout`).
fn persist(arrangement: Arrangement, virtual_target: bool) -> bool {
    virtual_target && arrangement == Arrangement::Exclusive
}
/// Put back the database entry a stream saved: the user's displays as they have
/// them with the virtual display beside them, as Windows lays out a new display.
/// Called before the user's layout is restored, while the virtual display is
/// still connected. Switching them on is not enough: the virtual display would
/// stay primary and a monitor Windows switches on gets Windows' choice of timing
/// (60 Hz on a 120 Hz monitor), and every later stream that keeps it on is handed
/// that entry.
fn forget_saved_layout(before: &Snapshot, target: &str, saves: bool) -> Result<()> {
    put_entry_back(
        before,
        target,
        Topology::set_active,
        || Ok((Snapshot::capture()?.nodes, connected()?)),
        Snapshot::restore,
        || {
            if saves {
                Topology::save_current()
            } else {
                Ok(())
            }
        },
    )
}
/// Whether a repair while streams hold a layout may save what it put back. The entry
/// is for the displays connected now. Saved while the held layout keeps a display of
/// the user's off, it has that display off, and unless the layout saves its own
/// ([`saves_layout`]) nothing takes that back: Windows recalls it when a later
/// virtual display leaves, and the monitor stays dark. An isolated layout saves
/// nothing either: its displays are 64000 apart on purpose, which is no entry for
/// Windows to hand a later stream.
fn repair_saves(state: &State) -> bool {
    if saves_layout(state, None) {
        return true;
    }
    let isolated = matches!(
        state.arrangement,
        Some(Arrangement::Isolated | Arrangement::PrimaryIsolated)
    );
    !isolated
        && !state
            .before
            .as_ref()
            .is_some_and(|before| disabled_original_displays(&before.nodes, &state.applied))
}
/// The steps of [`forget_saved_layout`]. `set_active` drops a display with no path
/// left (a driver reset, a lost device) and switches on the rest at Windows' timing.
/// With the stream's display gone, what is connected is the user's displays alone:
/// saved now, that timing is what Windows recalls when a later virtual display
/// leaves, and it then reads as the user's own. So nothing is saved unless that
/// display is on.
fn put_entry_back(
    before: &Snapshot,
    target: &str,
    set_active: impl FnOnce(&[String]) -> Result<()>,
    read: impl FnOnce() -> Result<(Vec<Node>, Vec<String>)>,
    restore: impl FnOnce(&Snapshot) -> Result<()>,
    save: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let mut ids: Vec<_> = before
        .nodes
        .iter()
        .filter(|n| n.active && n.device_id != target)
        .map(|n| n.device_id.clone())
        .collect();
    ids.push(target.to_owned());
    set_active(&ids)?;
    let (current, connected) = read()?;
    let layout = entry_layout(before, &current, &connected, target)?;
    let laid_out = restore(&layout);
    // A layout that only partly went back still beats the entry with them off,
    // and this entry is the stream's display's own.
    save()?;
    laid_out
}
/// `before` with the virtual display `target`, as it is in `current`, beside it. A
/// display no longer `connected` has no place in it: `before` keeps a departed remote
/// monitor's node, and the entry would have a gap where it was.
fn entry_layout(
    before: &Snapshot,
    current: &[Node],
    connected: &[String],
    target: &str,
) -> Result<Snapshot> {
    let target = current
        .iter()
        .find(|n| n.device_id == target && n.active)
        .ok_or_else(|| anyhow::anyhow!("the stream's display is off"))?;
    let present: Vec<_> = before
        .nodes
        .iter()
        .filter(|n| connected.contains(&n.device_id))
        .cloned()
        .collect();
    let mut layout = before.clone();
    layout.nodes = butterpollo_core::display_policy::beside_the_users(&present, target);
    Ok(layout)
}
/// Whether a virtual display's arrival handed a layout that keeps the other
/// displays on an entry from the Windows display database that does not: it can
/// come from an exclusive stream (a crash left its own in place, a second stream's
/// entry has nothing to put it back) or from a version that saved it wrong. One
/// that `saves` its own has no use for the entry.
fn recall_to_undo(
    saves: bool,
    arrangement: Arrangement,
    before: &[Node],
    current: &[Node],
    connected: &[String],
) -> bool {
    !saves
        && butterpollo_core::display_policy::recalled_over(
            before,
            current,
            connected,
            &[],
            arrangement.keeps_primary(),
        )
}
/// A remote monitor's display `output` has arrived, `before` being the layout from
/// just before it. On a layout that leaves the others alone it holds no lease to
/// look after them, so what Windows recalled for it is undone here. A layout that
/// does move the primary or switches them off applies that afterwards.
pub fn undo_recall_for_remote_monitor(before: &Snapshot, output: &str) -> Result<()> {
    let state = state().lock().unwrap();
    let Some(saves) = remote_monitor_repair(
        &state,
        &before.nodes,
        &Snapshot::capture()?.nodes,
        &connected()?,
    ) else {
        return Ok(());
    };
    let target = crate::display::monitors()?
        .into_iter()
        .find(|m| m.matches(output))
        .ok_or_else(|| anyhow::anyhow!("the remote monitor's display is gone"))?
        .device_id;
    tracing::info!(
        "Windows recalled a saved layout over the other displays for a remote monitor's display; putting it back"
    );
    forget_saved_layout(before, &target, saves)
}
/// Whether a remote monitor's arrival needs a repair, and if so whether it saves.
/// It is judged by the layout the streams hold; with none held, by one that leaves
/// the others alone, as its own applies afterwards.
fn remote_monitor_repair(
    state: &State,
    before: &[Node],
    current: &[Node],
    connected: &[String],
) -> Option<bool> {
    let held = state.arrangement.filter(|_| state.users != 0);
    recall_to_undo(
        false,
        held.unwrap_or(Arrangement::Extended),
        before,
        current,
        connected,
    )
    .then(|| repair_saves(state))
}
/// The layout that puts back what Windows recalled once another stream's display
/// has gone: `applied`, the layout from when it left, with what else is on as it is.
fn after_departure(applied: &[Node], before: Option<&Snapshot>) -> Snapshot {
    Snapshot {
        version: 1,
        nodes: applied.to_vec(),
        hdr: Default::default(),
        clone_groups: before.map_or_else(Vec::new, |before| before.clone_groups.clone()),
        scale: Default::default(),
        rotation: Default::default(),
    }
}
/// What putting back the layout a display left takes.
#[derive(Debug, PartialEq)]
struct DepartureRepair {
    /// See [`arriving_displays`]: the repair leaves these on.
    keep_on: Vec<String>,
    /// What the layout has on, of what is connected.
    on: std::collections::BTreeSet<String>,
    /// Whether to save it. Not when what is on differs from that layout: this can't
    /// tell Windows switching a monitor off from the user's Win+P in the same seconds,
    /// and what is arriving has its entry saved with its own layout. The first stream
    /// on such an entry repairs it.
    saves: bool,
}
/// Whether what is on now is what Windows recalled over the layout a display left, and
/// what putting that back takes. None on a layout that saves its own entry.
fn departure_repair(
    state: &State,
    arrangement: Arrangement,
    current: &[Node],
    connected: &[String],
) -> Option<DepartureRepair> {
    if state.persisted {
        return None;
    }
    let targets: Vec<_> = state.owners.iter().map(|(t, _)| t.clone()).collect();
    if !butterpollo_core::display_policy::recalled_over(
        &state.applied,
        current,
        connected,
        &targets,
        arrangement.keeps_primary(),
    ) {
        return None;
    }
    let on: std::collections::BTreeSet<String> = state
        .applied
        .iter()
        .filter(|n| n.active && connected.contains(&n.device_id))
        .map(|n| n.device_id.clone())
        .collect();
    Some(DepartureRepair {
        keep_on: arriving_displays(state, current),
        saves: repair_saves(state) && active_ids(current) == on,
        on,
    })
}
/// Another stream's display has gone and Windows has recalled its entry for the
/// displays that remain. On a layout that keeps the others on, that entry can have
/// one of them off or at another mode: put it back, and save it so that it stays.
fn undo_recall_after_departure(state: &State, arrangement: Arrangement) -> Result<()> {
    let current = Snapshot::capture()?.nodes;
    let Some(repair) = departure_repair(state, arrangement, &current, &connected()?) else {
        return Ok(());
    };
    tracing::info!(
        keep_on = ?repair.keep_on,
        saves = repair.saves,
        "Windows recalled a saved layout over the other displays when a stream's display left; putting it back"
    );
    after_departure(&state.applied, state.before.as_ref()).restore_excluding(&repair.keep_on)?;
    // Not if anything else changed while it went back.
    if repair.saves && active_ids(&Snapshot::capture()?.nodes) == repair.on {
        Topology::save_current()?;
    }
    Ok(())
}
/// Retry while Windows is still applying another change, such as a virtual
/// display being removed: reading its mode fails until the change settles.
fn settled<T>(mut attempt: impl FnMut() -> Result<T>) -> Result<T> {
    for _ in 0..3 {
        match attempt() {
            Ok(value) => return Ok(value),
            Err(error) => {
                tracing::debug!(error = %format!("{error:#}"), "display layout change pending; retrying");
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
        }
    }
    attempt()
}
/// The layout to restore after streaming: Windows extends the desktop onto a
/// new virtual display before the arrangement is taken, but restoring that
/// would leave a retained display as an invisible monitor beside the user's
/// own, and its settings fail once it is removed (os error 31).
fn original_layout(mut snapshot: Snapshot, target: &str) -> Snapshot {
    let others_active = snapshot
        .nodes
        .iter()
        .any(|n| n.active && n.device_id != target);
    if others_active {
        for node in snapshot.nodes.iter_mut().filter(|n| n.device_id == target) {
            node.active = false;
            node.primary = false;
        }
        snapshot.hdr.remove(target);
        snapshot.scale.remove(target);
        snapshot.rotation.remove(target);
    }
    snapshot
}
/// Whether the layout a display left is still as it was applied. The read fails
/// while Windows applies a change, as this departure's own can, and a read that
/// keeps failing says nothing of the user: the streams that go on are laid out.
fn left_as_applied(read: impl FnMut() -> Result<bool>) -> bool {
    settled(read).unwrap_or_else(|error| {
        tracing::warn!(error = %format!("{error:#}"), "reading the layout a display left failed; laying out the remaining streams");
        true
    })
}
/// A stream's display left while others go on. A layout still as it was applied
/// is laid out without it, and again once Windows has settled ([`departures`]).
/// Any other layout is the user's (Win+P, a primary picked in Windows), which
/// neither may override: only stop expecting this display.
fn display_left(
    state: &mut State,
    target: &str,
    as_applied: bool,
    apply: impl FnOnce(&mut State) -> Result<()>,
) -> Result<()> {
    if !as_applied {
        for node in state.applied.iter_mut().filter(|n| n.device_id == target) {
            node.active = false;
        }
        return Ok(());
    }
    note_departure(
        &mut LAST_DEPARTURE.lock().unwrap(),
        std::time::Instant::now(),
        crate::lossless::minimized_windows,
    );
    DEPARTURES.fetch_add(1, Ordering::Relaxed);
    apply(state)
}
pub struct Lease {
    arrangement: Arrangement,
    /// The streamed display; a recreated virtual display may change identity.
    target: Mutex<String>,
}
impl Lease {
    pub fn exclusive(&self) -> bool {
        self.arrangement == Arrangement::Exclusive
    }
    /// The layout in force for this stream: the running streams', if it joined them.
    pub fn arrangement(&self) -> Arrangement {
        self.arrangement
    }
    /// `virtual_target`: the streamed display is a virtual display made for
    /// streaming, so the user's own layout does not include it. `original`:
    /// the layout taken before that display was created; what Windows
    /// changed when it arrived is then undone with the rest.
    pub fn acquire(
        output: &str,
        arrangement: Arrangement,
        retained: &[String],
        virtual_target: bool,
        original: Option<Snapshot>,
    ) -> Result<Self> {
        let mut state = state().lock().unwrap();
        let monitors = crate::display::monitors()?;
        let target = monitors
            .iter()
            .find(|m| m.matches(output))
            .ok_or_else(|| anyhow::anyhow!("display arrangement target unavailable"))?;
        let target_id = target.device_id.clone();
        let retained: Vec<_> = monitors
            .iter()
            .filter(|m| retained.contains(&m.display_name) || retained.contains(&m.device_id))
            .map(|m| m.device_id.clone())
            .collect();
        if state.users != 0 {
            // Another stream owns the layout: join it rather than refuse.
            let owned = state.arrangement.unwrap_or(arrangement);
            if owned != arrangement {
                tracing::info!(requested = ?arrangement, active = ?owned, "joining the display arrangement of the running streams");
            }
            if let Some(owner) = state.owners.iter_mut().find(|(t, _)| *t == target_id) {
                owner.1 += 1;
                state.users += 1;
                return Ok(Self {
                    arrangement: owned,
                    target: Mutex::new(target_id),
                });
            }
            // The displays that are on now can hold an entry a stream on another
            // layout left: the monitor off, or at another mode.
            if virtual_target
                && let Some(original) = &original
                && recall_to_undo(
                    state.persisted,
                    owned,
                    &original.nodes,
                    &Snapshot::capture()?.nodes,
                    &connected()?,
                )
            {
                tracing::info!(
                    "Windows recalled a saved layout over the other displays for a second stream's display; putting it back"
                );
                if let Err(error) = forget_saved_layout(original, &target_id, repair_saves(&state))
                {
                    tracing::warn!(error = %format!("{error:#}"), "putting back the saved display layout failed");
                }
            }
            add_owner(&mut state, &target_id, virtual_target);
            let added: Vec<_> = retained
                .into_iter()
                .filter(|id| !state.retained.contains(id))
                .collect();
            state.retained.extend(added.iter().cloned());
            if let Err(error) = apply(&mut state, owned, None) {
                remove_added_owner(&mut state, &target_id);
                state.retained.retain(|id| !added.contains(id));
                return Err(error);
            }
            state.users += 1;
            return Ok(Self {
                arrangement: owned,
                target: Mutex::new(target_id),
            });
        }
        let mut current = Snapshot::capture()?;
        let before = if let Some(pending) = crate::display_recovery::pending_arrangement()? {
            pending
        } else if virtual_target {
            original_layout(original.unwrap_or_else(|| current.clone()), &target_id)
        } else {
            current.clone()
        };
        // A host that stopped without putting back the layout it saved for
        // Windows to recall (a crash, a power cut), or a version that put it back
        // wrong, left the user's displays off, at another mode or no longer primary
        // whenever this virtual display arrives. Put it back, unless this stream
        // saves its own.
        if virtual_target
            && recall_to_undo(
                persist(arrangement, virtual_target),
                arrangement,
                &before.nodes,
                &current.nodes,
                &connected()?,
            )
        {
            tracing::info!(
                "Windows recalled a saved layout over the user's displays for the virtual display; putting it back"
            );
            match forget_saved_layout(&before, &target_id, true).and_then(|()| Snapshot::capture())
            {
                Ok(recaptured) => current = recaptured,
                Err(error) => {
                    tracing::warn!(error = %format!("{error:#}"), "putting back the saved display layout failed");
                }
            }
        }
        let desired = if virtual_target {
            arrangement.compose_after_hotplug(
                &current.nodes,
                &target_id,
                &retained,
                &before.nodes,
            )?
        } else {
            arrangement.compose(&current.nodes, &target_id, &retained)?
        };
        crate::display_recovery::arrangement(Some((before.clone(), desired.clone())))?;
        let apply = (|| -> Result<()> {
            let ids = active_ids(&desired);
            if ids != active_ids(&current.nodes) {
                Topology::set_active(&ids.into_iter().collect::<Vec<_>>())?;
            }
            Topology::query()?.set_positions(
                &desired
                    .iter()
                    .filter(|n| n.active)
                    .map(|n| (n.device_id.clone(), n.desired_position))
                    .collect(),
            )
        })();
        if let Err(error) = apply {
            // A pending recovery may have supplied an older, healthy baseline.
            // Only restoring that baseline permits clearing its journal.
            before.restore()?;
            crate::display_recovery::arrangement(None)?;
            return Err(error);
        }
        state.before = Some(before);
        state.users = 1;
        state.key = format!("{arrangement:?}:{target_id}");
        state.owners.clear();
        state.persists_for.clear();
        state.persisted = persist(arrangement, virtual_target);
        add_owner(&mut state, &target_id, virtual_target);
        state.arrangement = Some(arrangement);
        state.retained = retained;
        settle_layout(&mut state, desired, None, &[], Topology::save_current);
        Ok(Self {
            arrangement,
            target: Mutex::new(target_id),
        })
    }
    /// Displays this stream's layout switched off that Windows has switched
    /// back on, such as when an exclusive-fullscreen game loses focus and
    /// Windows recalls its saved layout for the connected displays.
    pub fn switched_back_on(&self) -> Result<Vec<String>> {
        let active: Vec<_> = crate::display::monitors()?
            .into_iter()
            .map(|m| m.device_id)
            .collect();
        let state = state().lock().unwrap();
        let Some(before) = &state.before else {
            return Ok(Vec::new());
        };
        Ok(butterpollo_core::display_policy::switched_back_on(
            &before.nodes,
            &state.applied,
            &active,
        ))
    }
    /// Reapply the stream layout after an owned VDD is recreated, retaining the
    /// original restoration snapshot and recovery journal.
    pub fn reapply(&self, output: &str, retained: &[String]) -> Result<()> {
        let mut state = state().lock().unwrap();
        let monitors = crate::display::monitors()?;
        let mut owned = self.target.lock().unwrap();
        let target = reapplied_target(&monitors, output, &owned)
            .ok_or_else(|| anyhow::anyhow!("recovered arrangement target unavailable"))?;
        if *owned != target.device_id {
            for owner in state.owners.iter_mut().filter(|(t, _)| *t == *owned) {
                owner.0 = target.device_id.clone();
            }
            for id in state.persists_for.iter_mut().filter(|id| **id == *owned) {
                *id = target.device_id.clone();
            }
            *owned = target.device_id.clone();
        }
        for id in monitors
            .iter()
            .filter(|m| retained.contains(&m.display_name) || retained.contains(&m.device_id))
        {
            if !state.retained.contains(&id.device_id) {
                state.retained.push(id.device_id.clone());
            }
        }
        let arrangement = state.arrangement.unwrap_or(self.arrangement);
        apply(&mut state, arrangement, None)
    }
    /// [`Self::reapply`] once another stream's display has left and Windows has
    /// settled ([`departures`]). What it recalled for the displays that remain is
    /// undone first ([`departure_repair`]). That can also undo the user's change in
    /// the same seconds, which is then not saved; a change before the display left
    /// counts no departure.
    pub fn reapply_after_departure(&self, output: &str, retained: &[String]) -> Result<()> {
        {
            let state = state().lock().unwrap();
            let arrangement = state.arrangement.unwrap_or(self.arrangement);
            if let Err(error) = undo_recall_after_departure(&state, arrangement) {
                tracing::warn!(error = %format!("{error:#}"), "putting back the displays another stream's display left failed");
            }
        }
        self.reapply(output, retained)
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        let mut state = state().lock().unwrap();
        let target = self.target.lock().unwrap().clone();
        state.users -= 1;
        let Released { left, saved } = release_owner(&mut state, &target);
        if state.users != 0 {
            // Other streams continue: lay out their displays without this one.
            if left {
                let arrangement = state.arrangement.unwrap_or(self.arrangement);
                let as_applied = left_as_applied(|| unchanged(&state.applied));
                let result = display_left(&mut state, &target, as_applied, |state| {
                    apply(state, arrangement, Some(&target))
                });
                if let Err(error) = result {
                    tracing::warn!(%error, "display arrangement update for the remaining streams failed");
                }
            }
            return;
        }
        let restored = (|| -> Result<()> {
            // Put the saved entry back even when the user changed the layout:
            // Windows recalls the entry for the remaining displays anyway once
            // the virtual display is removed.
            let restore =
                state.before.as_ref().is_some_and(|before| {
                    disabled_original_displays(&before.nodes, &state.applied)
                }) || settled(|| unchanged(&state.applied))?;
            if saved
                && let Some(before) = &state.before
                && let Err(error) = settled(|| forget_saved_layout(before, &target, true))
            {
                tracing::warn!(error = %format!("{error:#}"), "putting back the saved display layout failed");
            }
            if restore && let Some(before) = &state.before {
                settled(|| before.restore())?;
            }
            crate::display_recovery::arrangement(None)
        })();
        if let Err(error) = restored {
            tracing::warn!(error = %format!("{error:#}"), "display arrangement restoration remains pending");
        }
        state.persisted = false;
        state.persists_for.clear();
        state.before = None;
        state.applied.clear();
        state.key.clear();
        state.owners.clear();
        state.arrangement = None;
        state.retained.clear();
    }
}

/// Activate a selected, connected physical target before capture or mode changes.
/// This lease is independent of the later primary/exclusive arrangement.
pub struct Activation;
fn activation_state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(Mutex::default)
}
impl Activation {
    pub fn acquire(output: &str) -> Result<Option<Self>> {
        if output.is_empty() {
            return Ok(None);
        }
        let mut state = activation_state().lock().unwrap();
        let available = Topology::query_all()?.monitors();
        let target = available
            .iter()
            .find(|monitor| monitor.matches(output))
            .ok_or_else(|| anyhow::anyhow!("selected display is not connected"))?;
        if state.users != 0 && state.key == target.device_id {
            state.users += 1;
            return Ok(Some(Self));
        }
        let before = Snapshot::capture()?;
        if before
            .nodes
            .iter()
            .any(|node| node.device_id == target.device_id && node.active)
        {
            return Ok(None);
        }
        if state.users != 0 {
            bail!("another stream owns a different display activation");
        }
        let mut ids: Vec<_> = before
            .nodes
            .iter()
            .filter(|node| node.active)
            .map(|node| node.device_id.clone())
            .collect();
        ids.push(target.device_id.clone());
        // Record the original layout before allowing Windows to activate a target.
        crate::display_recovery::activation(Some((before.clone(), vec![])))?;
        let applied = (|| -> Result<Vec<Node>> {
            Topology::set_active(&ids)?;
            Topology::query()?.nodes()
        })();
        let applied = match applied {
            Ok(applied) => applied,
            Err(error) => {
                before.restore()?;
                crate::display_recovery::activation(None)?;
                return Err(error);
            }
        };
        if let Err(error) =
            crate::display_recovery::activation(Some((before.clone(), applied.clone())))
        {
            before.restore()?;
            crate::display_recovery::activation(None)?;
            return Err(error);
        }
        state.before = Some(before);
        state.applied = applied;
        state.key = target.device_id.clone();
        state.users = 1;
        Ok(Some(Self))
    }
}
impl Drop for Activation {
    fn drop(&mut self) {
        let mut state = activation_state().lock().unwrap();
        state.users -= 1;
        if state.users != 0 {
            return;
        }
        let restored = (|| -> Result<()> {
            if unchanged(&state.applied)?
                && let Some(before) = &state.before
            {
                before.restore()?;
            }
            crate::display_recovery::activation(None)
        })();
        if let Err(error) = restored {
            tracing::warn!(%error, "display activation restoration remains pending");
        }
        state.before = None;
        state.applied.clear();
        state.key.clear();
    }
}
/// The stream's display among the active monitors: by the output name the
/// stream knows, else by the device id the lease holds. Windows can list a
/// display under another desktop name (issue 56) before the stream learns it.
fn reapplied_target<'a>(
    monitors: &'a [crate::display::Monitor],
    output: &str,
    owned: &str,
) -> Option<&'a crate::display::Monitor> {
    monitors.iter().find(|m| m.matches(output)).or_else(|| {
        monitors
            .iter()
            .find(|m| !owned.is_empty() && m.device_id == owned)
    })
}

#[cfg(test)]
mod tests;
