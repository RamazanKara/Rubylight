//! Temporary exclusive/primary/isolated layouts with shared and crash restoration.
use crate::display::{Snapshot, Topology};
use anyhow::{Result, bail};
use butterpollo_core::{display_policy::Arrangement, topology::Node};
use std::sync::{
    Arc, Condvar, Mutex, OnceLock,
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
    /// This layout took up what a stopped watch left ([`WakeFollow`]).
    following: Option<WakeFollow>,
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
        restored_active(&mut before.nodes, node);
    }
    if !state.retained.iter().any(|r| r == id) {
        state.retained.push(id.to_owned());
    }
    true
}
/// `node` is on in the layout `nodes` goes back to.
fn restored_active(nodes: &mut Vec<Node>, node: &Node) {
    match nodes.iter_mut().find(|n| n.device_id == node.device_id) {
        Some(original) => original.active = true,
        None => nodes.push(node.clone()),
    }
}
/// `nodes` with the displays `kept` that are on in `current` on as well: they are held
/// by someone else, and a layout going back must leave them. To its right: where one is
/// now belongs to what is on now: in this layout that can be a gap, or on another display.
fn with_kept(mut nodes: Vec<Node>, current: &[Node], kept: &[String]) -> Vec<Node> {
    for node in current
        .iter()
        .filter(|n| n.active && kept.contains(&n.device_id))
    {
        if !nodes
            .iter()
            .any(|n| n.active && n.device_id == node.device_id)
        {
            nodes = butterpollo_core::display_policy::beside_the_users(&nodes, node);
        }
    }
    nodes
}
/// The layout a first lease goes back to. One an earlier stream left pending comes first,
/// which knows nothing of a remote monitor that arrived since: `retain` has no layout to
/// tell while no lease is held, and a restore would switch off a display still leased.
fn first_baseline(
    pending: Option<Snapshot>,
    original: Option<Snapshot>,
    current: &Snapshot,
    target: &str,
    retained: &[String],
    virtual_target: bool,
) -> Snapshot {
    match pending {
        Some(mut pending) => {
            pending.nodes = with_kept(pending.nodes, &current.nodes, retained);
            pending
        }
        None if virtual_target => {
            original_layout(original.unwrap_or_else(|| current.clone()), target)
        }
        None => current.clone(),
    }
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
/// The layout for the first stream's display. Windows can have switched the user's
/// displays off already, recalling the entry an earlier exclusive stream left, and a
/// layout composed from what is on then would not know them ([`keep_switched_off`]).
/// Exclusive only: under the others a display that is off has nothing to come back from.
fn first_layout(
    arrangement: Arrangement,
    current: &[Node],
    target: &str,
    retained: &[String],
    before: &[Node],
    virtual_target: bool,
) -> Result<Vec<Node>> {
    let mut desired = if virtual_target {
        arrangement.compose_after_hotplug(current, target, retained, before)?
    } else {
        arrangement.compose(current, target, retained)?
    };
    if arrangement == Arrangement::Exclusive {
        let off: Vec<Node> = before
            .iter()
            .filter(|n| n.active && !retained.contains(&n.device_id))
            .map(|n| Node {
                active: false,
                primary: false,
                ..n.clone()
            })
            .collect();
        keep_switched_off(&mut desired, &off);
    }
    Ok(desired)
}
/// How many streams' displays have left a layout other streams still hold.
static DEPARTURES: AtomicU64 = AtomicU64::new(0);
/// Compare it to see that a display left while this stream goes on.
pub fn departures() -> u64 {
    DEPARTURES.load(Ordering::Relaxed)
}
/// How many times a monitor's return had displays put back ([`lay_out_after_wake`]).
static WAKE_LAYOUTS: AtomicU64 = AtomicU64::new(0);
/// Compare it to see that this happened while this stream goes on: one heartbeat does it,
/// and it can have recalled the mode of any stream's display.
pub fn wake_layouts() -> u64 {
    WAKE_LAYOUTS.load(Ordering::Relaxed)
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
    let (mut desired, arriving) =
        compose_around_arrivals(state, arrangement, &current.nodes, &physical_displays()?)?;
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
/// The connected displays known to be physical (see [`arriving_displays`]).
fn physical_displays() -> Result<Vec<String>> {
    Ok(Topology::query_all()?
        .monitors()
        .into_iter()
        .filter(|m| m.is_known_physical())
        .map(|m| m.device_id)
        .collect())
}
/// The virtual displays on now that the held layout has no place for: one on its way
/// to a stream's lease, a remote monitor's not retained yet. The streams' heartbeats
/// lay out outside the launch that brings them, and a running stream must never lose
/// its display to that. A `physical` display is no such arrival: on with no place in
/// the layout, it is what Windows recalled when a display left (a TV that was off
/// before the stream), and the layout decides about it like any other.
fn arriving_displays(state: &State, current: &[Node], physical: &[String]) -> Vec<String> {
    current
        .iter()
        .filter(|n| {
            n.active
                && !physical.contains(&n.device_id)
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
    physical: &[String],
) -> Result<(Vec<Node>, Vec<String>)> {
    let targets: Vec<_> = state.owners.iter().map(|(t, _)| t.clone()).collect();
    let arriving = arriving_displays(state, current, physical);
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
        connected,
        Topology::set_active,
        || Ok(Snapshot::capture()?.nodes),
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
    !isolated(state)
        && !state
            .before
            .as_ref()
            .is_some_and(|before| disabled_original_displays(&before.nodes, &state.applied))
}
fn isolated(state: &State) -> bool {
    matches!(
        state.arrangement,
        Some(Arrangement::Isolated | Arrangement::PrimaryIsolated)
    )
}
/// The steps of [`forget_saved_layout`]. `set_active` drops a display with no path
/// left (a driver reset, a lost device) and switches on the rest at Windows' timing.
/// With the stream's display gone, what is connected is the user's displays alone:
/// saved now, that timing is what Windows recalls when a later virtual display
/// leaves, and it then reads as the user's own. So nothing is saved unless that
/// display is on. What is `connected` is read before anything switches on: a monitor
/// coming up can drop off that list for a moment, and left out of the layout for it
/// the layout would switch it off again.
fn put_entry_back(
    before: &Snapshot,
    target: &str,
    connected: impl FnOnce() -> Result<Vec<String>>,
    set_active: impl FnOnce(&[String]) -> Result<()>,
    read: impl FnOnce() -> Result<Vec<Node>>,
    restore: impl FnOnce(&Snapshot) -> Result<()>,
    save: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let connected = connected()?;
    let mut ids: Vec<_> = before
        .nodes
        .iter()
        .filter(|n| n.active && n.device_id != target)
        .map(|n| n.device_id.clone())
        .collect();
    ids.push(target.to_owned());
    set_active(&ids)?;
    let current = read()?;
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
    // Before the lock: stopping a watch holds the watcher across a wait for a thread that
    // wants this one.
    let settling = wake_watch_settling();
    let state = state().lock().unwrap();
    let pending = if state.users == 0 && settling {
        crate::display_recovery::pending_arrangement()?
    } else {
        None
    };
    let before = remote_monitor_baseline(state.users, settling, pending.as_ref(), before);
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
/// The layout a remote monitor's arrival goes by: the one from just before it, unless a
/// stopped watch left the user's: with their monitor away what was on then can be what
/// Windows lit, and an entry saved from that would light it for later streams. Only while
/// that is at work (`settling`): an entry stuck since then knows nothing of what the user
/// changed after it, and this repair saves.
fn remote_monitor_baseline<'a>(
    users: usize,
    settling: bool,
    pending: Option<&'a Snapshot>,
    before: &'a Snapshot,
) -> &'a Snapshot {
    pending.filter(|_| users == 0 && settling).unwrap_or(before)
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
    physical: &[String],
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
        keep_on: arriving_displays(state, current, physical),
        saves: repair_saves(state) && active_ids(current) == on,
        on,
    })
}
/// Another stream's display has gone and Windows has recalled its entry for the
/// displays that remain. On a layout that keeps the others on, that entry can have
/// one of them off or at another mode: put it back, and save it so that it stays.
fn undo_recall_after_departure(state: &State, arrangement: Arrangement) -> Result<()> {
    let current = Snapshot::capture()?.nodes;
    let Some(repair) = departure_repair(
        state,
        arrangement,
        &current,
        &connected()?,
        &physical_displays()?,
    ) else {
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
/// How long after a user's display is switched on a drop off the list is put down to
/// its waking. Measured: 0.3 to 1.6 s.
const WAKE_WATCH: std::time::Duration = std::time::Duration::from_secs(3);
/// How long a display that dropped gets to return. Measured: 3.1 to 6.0 s.
const WAKE_RETURN: std::time::Duration = std::time::Duration::from_secs(10);
const WAKE_POLL: std::time::Duration = std::time::Duration::from_millis(100);
/// How often a stream that took up a stopped watch's layout tries to lay it out once the
/// woken display showed, one heartbeat apart ([`Lease::follow_wake`]).
const FOLLOW_TRIES: usize = 8;
/// What a display switched on after the stream did ([`watch_wake`]).
#[derive(Debug, PartialEq)]
enum Wake {
    Stayed,
    CameBack,
    StayedAway,
    /// A launch or the host's stop ended the watch first: nothing more is known.
    Stopped,
}
/// Tells a watch to stop, and wakes it from its wait.
#[derive(Default)]
struct WatchStop {
    stopped: Mutex<bool>,
    changed: Condvar,
}
impl WatchStop {
    fn stop(&self) {
        *self.stopped.lock().unwrap() = true;
        self.changed.notify_all();
    }
    fn is_stopped(&self) -> bool {
        *self.stopped.lock().unwrap()
    }
    /// Wait out `time`. True when stopped, as soon as that happens.
    fn wait(&self, time: std::time::Duration) -> bool {
        let stopped = self.stopped.lock().unwrap();
        *self
            .changed
            .wait_timeout_while(stopped, time, |stopped| !*stopped)
            .unwrap()
            .0
    }
}
/// What a watch that did not finish leaves to the end of the holds ([`left_watch`]).
#[derive(Clone, Debug, PartialEq)]
struct LeftWatch {
    woken: Vec<String>,
    /// Whether the user's layout goes back ([`WakeEnd::restore`]).
    restore: bool,
}
struct RunningWatch {
    stop: Arc<WatchStop>,
    /// Answers whether it was stopped before it finished.
    thread: std::thread::JoinHandle<bool>,
    left: LeftWatch,
}
struct Watches {
    /// Launches preparing a display: no watch starts, as what it would write can land
    /// on their layout.
    held_off: usize,
    /// The host is stopping: no watch starts again.
    closed: bool,
    running: Option<RunningWatch>,
    left: Option<LeftWatch>,
}
impl Watches {
    /// Stop the watch and wait for its thread, which then has dropped the stream's
    /// display and writes nothing more. A thread that panicked did not finish either.
    fn stop_running(&mut self) {
        if let Some(RunningWatch { stop, thread, left }) = self.running.take() {
            stop.stop();
            if thread.join().unwrap_or(true) {
                self.left = Some(left);
            }
        }
    }
}
/// How [`WakeWatcher::start`] went.
#[derive(Debug, PartialEq)]
enum WatchStart {
    Started,
    /// Not now: what it was to do is for the holds' end, or for the next start.
    Refused,
    /// No thread: nothing comes back for its journal entry.
    NoThread,
}
/// The thread a watch runs on.
type WatchThread = Box<dyn FnOnce() -> bool + Send>;
/// The one watch that can run after a stream's end, off the threads that wait for that
/// end. Its lock is held while it waits for the thread, so that whoever gets past a stop
/// finds the display gone; the thread never takes this lock.
struct WakeWatcher(Mutex<Watches>);
impl WakeWatcher {
    const fn new() -> Self {
        Self(Mutex::new(Watches {
            held_off: 0,
            closed: false,
            running: None,
            left: None,
        }))
    }
    /// Run `job` on its own thread; when it may not, it is dropped unrun. It answers whether
    /// it was stopped before it finished. Never call this holding the arrangement's lock:
    /// a watch still running is waited for, and it may want that lock.
    fn start(
        &self,
        left: LeftWatch,
        job: impl FnOnce(&WatchStop) -> bool + Send + 'static,
    ) -> WatchStart {
        self.start_on(left, job, |thread| {
            std::thread::Builder::new()
                .name("wake-watch".into())
                .spawn(thread)
        })
    }
    fn start_on(
        &self,
        left: LeftWatch,
        job: impl FnOnce(&WatchStop) -> bool + Send + 'static,
        spawn: impl FnOnce(WatchThread) -> std::io::Result<std::thread::JoinHandle<bool>>,
    ) -> WatchStart {
        let mut watches = self.0.lock().unwrap();
        if watches.closed {
            return WatchStart::Refused;
        }
        if watches.held_off != 0 {
            // Here and not by the caller: `job` goes once this lock has, and its display
            // takes a while. The hold can end during that.
            watches.left = Some(left);
            return WatchStart::Refused;
        }
        watches.stop_running();
        watches.left = None;
        let stop = Arc::new(WatchStop::default());
        let stops = stop.clone();
        match spawn(Box::new(move || job(&stops))) {
            Ok(thread) => {
                watches.running = Some(RunningWatch { stop, thread, left });
                WatchStart::Started
            }
            Err(error) => {
                tracing::warn!(%error, "the watch over a woken display could not start");
                WatchStart::NoThread
            }
        }
    }
    /// No watch runs or starts until as many [`Self::release`]s.
    fn hold_off(&self) {
        let mut watches = self.0.lock().unwrap();
        watches.held_off += 1;
        watches.stop_running();
    }
    /// For the last hold to end: what a watch held off or stopped left. No journal entry
    /// goes on that alone ([`left_watch`]).
    fn release(&self) -> Option<LeftWatch> {
        let mut watches = self.0.lock().unwrap();
        watches.held_off -= 1;
        if watches.held_off != 0 || watches.closed {
            return None;
        }
        watches.left.take()
    }
    /// For good: the host is stopping. What a stopped watch left stays in the journal,
    /// for the next start to put back.
    fn close(&self) {
        let mut watches = self.0.lock().unwrap();
        watches.closed = true;
        watches.stop_running();
    }
    /// Whether a watch is at work, or left work for the holds' end.
    fn settling(&self) -> bool {
        let watches = self.0.lock().unwrap();
        watches.left.is_some()
            || watches
                .running
                .as_ref()
                .is_some_and(|watch| !watch.thread.is_finished())
    }
}
static WAKE_WATCHER: WakeWatcher = WakeWatcher::new();
/// Holds off the watch over a woken display while a launch prepares a display: a running
/// one drops the stream's display at once and writes nothing more, and none starts. Take
/// it before anything looks at the displays: a watched display still reads as live.
#[must_use]
pub struct WakeWatchOff(());
pub fn hold_off_wake_watch() -> WakeWatchOff {
    WAKE_WATCHER.hold_off();
    WakeWatchOff(())
}
impl Drop for WakeWatchOff {
    fn drop(&mut self) {
        if let Some(left) = WAKE_WATCHER.release() {
            start_left_watch(left);
        }
    }
}
/// The host is stopping: no watch outlasts that, or starts.
pub fn close_wake_watch() {
    WAKE_WATCHER.close();
}
/// Whether a stream's end is still at work on the displays, up to about 13 s after that
/// end, though no stream or teardown shows it. What is on can be anything until then:
/// a layout read now is nobody's, and one written now can be undone by it.
pub fn wake_watch_settling() -> bool {
    WAKE_WATCHER.settling()
}
/// The user's displays that are connected and off: switching them on wakes them. None
/// without the stream's own display, as no display is left to hold the layout then.
fn displays_to_wake(
    before: &[Node],
    active: &[Node],
    connected: &[String],
    target: &str,
) -> Vec<String> {
    if !connected.iter().any(|id| id == target) {
        return Vec::new();
    }
    let on_now = |id: &String| active.iter().any(|a| a.active && a.device_id == *id);
    before
        .iter()
        .filter(|n| {
            n.active
                && n.device_id != target
                && connected.contains(&n.device_id)
                && !on_now(&n.device_id)
        })
        .map(|n| n.device_id.clone())
        .collect()
}
/// What Windows could switch on if the stream's display went while a woken display is
/// away: `others`, what is connected besides Rubylight's displays, that isn't in the
/// user's layout. A display whose path can't be read counts: being wrong costs a watch.
fn displays_it_could_switch_on(before: &[Node], others: &[String]) -> Vec<String> {
    others
        .iter()
        .filter(|id| !before.iter().any(|n| n.active && n.device_id == **id))
        .cloned()
        .collect()
}
/// Watch `woken`, just switched on. A monitor that slept for want of a signal can show
/// a picture, drop off the list about a second later and return 3 to 6 s after that.
/// `away` runs at each look while one is gone. `read` failing means Windows is still
/// applying a change, which says nothing about them. `sleep` answers true when the watch
/// was stopped.
fn watch_wake(
    woken: &[String],
    mut read: impl FnMut() -> Result<Vec<String>>,
    mut elapsed: impl FnMut() -> std::time::Duration,
    mut sleep: impl FnMut() -> bool,
    mut away: impl FnMut(),
) -> Wake {
    let mut looks = WakeLooks::default();
    loop {
        let now = elapsed();
        let present = read().ok().map(|c| all_listed(woken, &c));
        match looks.look(now, present) {
            WakeLook::Done(wake) => return wake,
            WakeLook::Away => away(),
            WakeLook::Watching => {}
        }
        if sleep() {
            return Wake::Stopped;
        }
    }
}
fn all_listed(woken: &[String], connected: &[String]) -> bool {
    woken.iter().all(|id| connected.contains(id))
}
/// What one look at the displays says.
#[derive(Debug, PartialEq)]
enum WakeLook {
    Watching,
    Away,
    Done(Wake),
}
/// What the looks at a woken display have seen.
#[derive(Debug, Default)]
struct WakeLooks {
    gone_since: Option<std::time::Duration>,
}
impl WakeLooks {
    /// `present`: whether they are all listed; None when that can't be read.
    fn look(&mut self, now: std::time::Duration, present: Option<bool>) -> WakeLook {
        match (self.gone_since, present) {
            (None, Some(false)) => self.gone_since = Some(now),
            (Some(_), Some(true)) => return WakeLook::Done(Wake::CameBack),
            _ => {}
        }
        match self.gone_since {
            None if now >= WAKE_WATCH => WakeLook::Done(Wake::Stayed),
            Some(at) if now.saturating_sub(at) >= WAKE_RETURN => WakeLook::Done(Wake::StayedAway),
            Some(_) => WakeLook::Away,
            None => WakeLook::Watching,
        }
    }
}
/// What is on that the user's layout doesn't have, known physical (a stream's virtual
/// display is never one of them), and what stays on without it. `keep`: what someone
/// holds now, which a physical stream's display can be.
fn strays(
    before: &[Node],
    active: &[String],
    physical: &[String],
    keep: &[String],
) -> (Vec<String>, Vec<String>) {
    active.iter().cloned().partition(|id| {
        physical.contains(id)
            && !keep.contains(id)
            && !before.iter().any(|n| n.active && n.device_id == *id)
    })
}
/// Whether the entry for what is connected may have only `on` on. Not with a display of
/// the user's connected and off: Windows would recall that as their layout.
fn strays_off_saves(before: &[Node], on: &[String], connected: &[String]) -> bool {
    before
        .iter()
        .filter(|n| n.active && connected.contains(&n.device_id))
        .all(|n| on.contains(&n.device_id))
}
/// Windows recalled the entry for what is connected while a woken display is away, and
/// that can have a display on that was off before the stream, such as a TV. Off again,
/// and saved so that the entry no longer has it on, if it `saves`.
fn switch_off_strays(before: &[Node], keep: &[String], saves: bool) -> Result<()> {
    let active: Vec<_> = Topology::query()?
        .monitors()
        .into_iter()
        .map(|m| m.device_id)
        .collect();
    let (off, on) = strays(before, &active, &physical_displays()?, keep);
    if off.is_empty() || on.is_empty() {
        return Ok(());
    }
    tracing::info!(displays = ?off, "Windows switched on a display the user's layout doesn't have while their monitor is away; switching it off");
    Topology::set_active(&on)?;
    if saves && strays_off_saves(before, &on, &connected()?) {
        Topology::save_current()?;
    }
    Ok(())
}
/// Whether what is on is the user's layout again: each of their displays that is connected
/// on at its mode, the primary theirs, nothing else physical on but what is held (`keep`).
fn layout_is_back(
    before: &[Node],
    current: &[Node],
    connected: &[String],
    physical: &[String],
    keep: &[String],
) -> bool {
    let active: Vec<_> = active_ids(current).into_iter().collect();
    !butterpollo_core::display_policy::recalled_over(before, current, connected, &[], true)
        && strays(before, &active, physical, keep).0.is_empty()
}
/// The user's layout, for what a stopped watch left. What is already right isn't touched,
/// as applying a layout again can retime a display; what is held now stays.
fn put_left_layout_back(before: &Snapshot) -> Result<()> {
    let leased = crate::display::leased_displays();
    let current = Snapshot::capture()?.nodes;
    let connected = connected()?;
    if layout_is_back(
        &before.nodes,
        &current,
        &connected,
        &physical_displays()?,
        &leased,
    ) {
        return Ok(());
    }
    tracing::info!("putting the user's layout back after a stream's end was cut short");
    present_layout(before, &connected).restore_excluding(&leased)
}
/// `before` for what is `connected`, for when a watch is done waiting. With the display at
/// the origin gone for good, the others would have nothing there, which Windows refuses
/// (invalid parameter) for as long as it is gone, and the journal entry would sit.
fn present_layout(before: &Snapshot, connected: &[String]) -> Snapshot {
    let mut layout = before.clone();
    layout.nodes.retain(|n| connected.contains(&n.device_id));
    let on = || layout.nodes.iter().filter(|n| n.active);
    let at_origin = on().any(|n| (n.desired_position.x, n.desired_position.y) == (0, 0));
    let first = on()
        .map(|n| (n.desired_position.x, n.desired_position.y))
        .min();
    if let Some((x, y)) = first.filter(|_| !at_origin) {
        for node in &mut layout.nodes {
            node.desired_position.x = node.desired_position.x.saturating_sub(x);
            node.desired_position.y = node.desired_position.y.saturating_sub(y);
        }
    }
    layout
}
/// The watch's last part with no lease to take it up, the stream's display gone: a launch
/// that failed or took no lease (a configured window, a physical stream with no layout)
/// cut it short, or a stream's end during a launch never got one. With the monitor away
/// Windows can have lit what is left, so the layout goes back once it stayed or returned,
/// and only that clears the journal. If that fails the entry stays, for the next stream
/// or start. True when stopped before it finished: then the same goes for the next hold's
/// end. This runs beside what follows: nothing here delays a launch's answer.
fn left_watch(
    left: LeftWatch,
    stop: &WatchStop,
    read: impl FnMut() -> Result<Vec<String>>,
    switch_off: impl FnMut() -> Result<()>,
    restore: impl FnMut() -> Result<()>,
    forget: impl FnOnce() -> Result<()>,
) -> bool {
    run_wake_watch(
        WakeEnd {
            woken: left.woken,
            restore: left.restore,
            // No entry to put back with the stream's display gone, and whether the layout
            // is right is read when it is time to look ([`put_left_layout_back`]).
            first: Ok(()),
            restored: Err(anyhow::anyhow!("a stopped watch left it")),
        },
        stop,
        read,
        switch_off,
        || Ok(()),
        restore,
        forget,
    )
}
/// Start [`left_watch`]. A lease that took the journal entry up has it from here on.
fn start_left_watch(left: LeftWatch) {
    let job = left.clone();
    WAKE_WATCHER.start(left, move |stop| {
        crate::input::on_input_desktop(move || {
            let before = match watch_step(stop, crate::display_recovery::pending_arrangement) {
                None => return stop.is_stopped(),
                Some(Ok(Some(before))) => before,
                Some(Ok(None)) => return false,
                Some(Err(error)) => {
                    tracing::warn!(error = %format!("{error:#}"), "display arrangement restoration remains pending");
                    return false;
                }
            };
            left_watch(
                job,
                stop,
                connected,
                || switch_off_strays(&before.nodes, &crate::display::leased_displays(), true),
                || put_left_layout_back(&before),
                || crate::display_recovery::arrangement(None),
            )
        })
    });
}
/// What a non-exclusive stream does for what a stopped watch left. It took the layout over
/// with the woken display perhaps still to drop, or away: then no layout it composes knows
/// that display, what Windows lit in its place stays on, and the display may stay dark. So
/// its heartbeat looks on, as the watch did ([`Lease::follow_wake`]). Under an exclusive
/// layout there is nothing to do: everything else is off, and comes back as any recall.
#[derive(Debug)]
struct WakeFollow {
    woken: Vec<String>,
    started: std::time::Instant,
    looks: WakeLooks,
    tries: usize,
    /// The displays have been put back once ([`pass_after_wake`]).
    repaired: bool,
    done: bool,
    failed_strays: bool,
}
/// Whether a stream joining a layout came while a stopped watch's display is still looked
/// for. Once that is done, what is on is as anyone's.
fn joins_a_cut_watch(following: Option<&WakeFollow>) -> bool {
    following.is_some_and(|f| !f.done)
}
/// Follow up on a pending layout: the displays of `before` others don't hold, some of them
/// away (not `connected`) already, as a lease starts.
fn wake_follow(
    arrangement: Arrangement,
    before: &[Node],
    connected: &[String],
    target: &str,
    retained: &[String],
) -> Option<WakeFollow> {
    if arrangement == Arrangement::Exclusive {
        return None;
    }
    let woken: Vec<String> = before
        .iter()
        .filter(|n| n.active && n.device_id != target && !retained.contains(&n.device_id))
        .map(|n| n.device_id.clone())
        .collect();
    if woken.is_empty() {
        return None;
    }
    let looks = WakeLooks {
        gone_since: (!all_listed(&woken, connected)).then_some(std::time::Duration::ZERO),
    };
    Some(WakeFollow {
        woken,
        started: std::time::Instant::now(),
        looks,
        tries: 0,
        repaired: false,
        done: false,
        failed_strays: false,
    })
}
/// What a look at the woken display calls for.
#[derive(Debug, PartialEq)]
enum FollowStep {
    Nothing,
    SwitchOffStrays,
    LayOut,
}
impl WakeFollow {
    /// `now`: how long the stream's layout has been there.
    fn step(&mut self, now: std::time::Duration, connected: Option<&[String]>) -> FollowStep {
        if self.done {
            return FollowStep::Nothing;
        }
        let present = connected.map(|c| all_listed(&self.woken, c));
        match self.looks.look(now, present) {
            WakeLook::Watching => FollowStep::Nothing,
            WakeLook::Away => FollowStep::SwitchOffStrays,
            WakeLook::Done(Wake::StayedAway) => {
                tracing::warn!(displays = ?self.woken, "a display dropped off the list when it woke and did not come back in time");
                self.done = true;
                FollowStep::Nothing
            }
            WakeLook::Done(_) => FollowStep::LayOut,
        }
    }
    /// One try at the layout has gone. True while there is another.
    fn tried(&mut self, succeeded: bool) -> bool {
        self.tries += 1;
        self.done = succeeded || self.tries >= FOLLOW_TRIES;
        !self.done
    }
}
/// Whether the layout needs its displays put back before it is laid out: the woken display
/// off, at another mode or (`keeps_primary`) no longer primary, or Windows lit something
/// else. A monitor that came back as it was only has its place to get.
fn wake_layout_wrong(
    before: &[Node],
    current: &[Node],
    connected: &[String],
    physical: &[String],
    streams: &[String],
    keep: &[String],
    keeps_primary: bool,
) -> bool {
    let active: Vec<_> = active_ids(current).into_iter().collect();
    butterpollo_core::display_policy::recalled_over(
        before,
        current,
        connected,
        streams,
        keeps_primary,
    ) || !strays(before, &active, physical, keep).0.is_empty()
}
/// What a pass for a returned display leaves on beside the streams: what they and launches
/// and remote monitors (`leased`) hold, and what is on its way to one. And whether anything
/// is: then what is on is no entry to save ([`DepartureRepair::saves`]).
fn beside_the_streams(
    state: &State,
    current: &[Node],
    physical: &[String],
    leased: &[String],
) -> (Vec<String>, bool) {
    let arriving = arriving_displays(state, current, physical);
    let arrives = !arriving.is_empty();
    let mut keep: Vec<_> = state.owners.iter().map(|(t, _)| t.clone()).collect();
    keep.extend(state.retained.iter().cloned());
    keep.extend(leased.iter().cloned());
    keep.extend(arriving);
    (keep, arrives)
}
/// The user's layout around what is kept on, of which only the place is its business. HDR,
/// scale, rotation and mode there are what the streams set, not to be undone mid-stream.
/// Not excluded from the restore: left where it is, it can be where the user's monitor goes.
fn layout_around_streams(before: Snapshot, current: &[Node], keep: &[String]) -> Snapshot {
    let mut layout = Snapshot {
        nodes: with_kept(before.nodes, current, keep),
        ..before
    };
    for id in keep {
        layout.hdr.remove(id);
        layout.scale.remove(id);
        layout.rotation.remove(id);
        let now = current.iter().find(|n| n.active && n.device_id == *id);
        let node = layout.nodes.iter_mut().find(|n| n.device_id == *id);
        if let (Some(node), Some(now)) = (node, now) {
            node.mode = now.mode.clone();
        }
    }
    layout
}
/// One go at a returned display's layout. The repair runs once however many goes the layout
/// takes: a monitor refusing its saved mode or HDR gets no more passes on a running stream,
/// and the stream gets its layout anyway. What comes back is a failed repair, for its card.
fn pass_after_wake(
    repaired: &mut bool,
    wrong: bool,
    repair: impl FnOnce() -> Result<()>,
    lay_out: impl FnOnce() -> Result<()>,
) -> Result<Option<String>> {
    let due = wrong && !std::mem::replace(repaired, true);
    if due {
        WAKE_LAYOUTS.fetch_add(1, Ordering::Relaxed);
    }
    repair_then_lay_out(|| if due { repair() } else { Ok(()) }, lay_out)
}
/// The woken display stayed or returned: the layout the stream planned, with it at its
/// mode. What is right already isn't written, so this is one layout pass at most. Whether
/// the displays were put back, and what came of that if it failed.
fn lay_out_after_wake(
    state: &mut State,
    arrangement: Arrangement,
) -> Result<(bool, Option<String>)> {
    let Some(before) = state.before.clone() else {
        return Ok((false, None));
    };
    let targets: Vec<_> = state.owners.iter().map(|(t, _)| t.clone()).collect();
    let Some(target) = targets.first() else {
        return Ok((false, None));
    };
    let current = Snapshot::capture()?.nodes;
    let physical = physical_displays()?;
    let (keep, arrives) = beside_the_streams(
        state,
        &current,
        &physical,
        &crate::display::leased_displays(),
    );
    let wrong = wake_layout_wrong(
        &before.nodes,
        &current,
        &connected()?,
        &physical,
        &targets,
        &keep,
        arrangement.keeps_primary(),
    );
    // Saved as any repair is but for the woken display, which the layout never had off.
    let saves = !isolated(state) && !arrives;
    let was_repaired = state.following.as_ref().is_some_and(|f| f.repaired);
    let mut repaired = was_repaired;
    let pass = pass_after_wake(
        &mut repaired,
        wrong,
        || {
            tracing::info!(
                "a display a stopped watch was waiting for is back; putting back the layout around the stream"
            );
            let layout = layout_around_streams(before, &current, &keep);
            forget_saved_layout(&layout, target, saves)
        },
        || apply(state, arrangement, None),
    );
    if let Some(follow) = state.following.as_mut() {
        follow.repaired = repaired;
    }
    Ok((repaired && !was_repaired, pass?))
}
/// The user's displays to watch at a stream's end: the ones it wakes, if Windows could
/// switch on something else with the stream's display gone. With nothing else to switch
/// on there is nothing to protect, whatever a woken display does.
fn woken_to_watch(
    before: &[Node],
    active: impl FnOnce() -> Result<Vec<Node>>,
    connected: &[String],
    others: &[String],
    target: &str,
) -> Result<Vec<String>> {
    if displays_it_could_switch_on(before, others).is_empty() {
        return Ok(Vec::new());
    }
    Ok(displays_to_wake(before, &active()?, connected, target))
}
/// [`woken_to_watch`] for what is connected now.
fn displays_to_watch(before: &Snapshot, target: &str) -> Vec<String> {
    settled(|| {
        let monitors = Topology::query_all()?.monitors();
        let others: Vec<String> = monitors
            .iter()
            .filter(|m| !m.is_rubylight())
            .map(|m| m.device_id.clone())
            .collect();
        let connected: Vec<String> = monitors.into_iter().map(|m| m.device_id).collect();
        woken_to_watch(
            &before.nodes,
            || Ok(Snapshot::capture()?.nodes),
            &connected,
            &others,
            target,
        )
    })
    .unwrap_or_else(|error| {
        tracing::warn!(error = %format!("{error:#}"), "reading the displays at a stream's end failed; not watching them wake");
        Vec::new()
    })
}
/// What a stream's end leaves to its watch: the stream's display stays until `woken` have
/// shown they stay. Gone with one of them away, what is left connected can be a TV that
/// was off, which Windows then switches on alone. All this runs beside whatever comes
/// next, and whatever comes next stops it first ([`WakeWatcher`]).
struct WakeEnd {
    woken: Vec<String>,
    /// Whether the user's layout goes back after the entry.
    restore: bool,
    /// How the entry and that layout went at the stream's end.
    first: Result<()>,
    restored: Result<()>,
}
/// One write of a watch: under the arrangement's lock, and not once it was stopped or a
/// lease began. None then, with nothing written.
fn watch_step<T>(stop: &WatchStop, work: impl FnOnce() -> Result<T>) -> Option<Result<T>> {
    let state = state().lock().unwrap();
    if stop.is_stopped() || state.users != 0 {
        return None;
    }
    Some(work())
}
/// [`settled`] for a watch's write ([`watch_step`]): a stop ends it at once.
fn settled_step<T>(stop: &WatchStop, mut attempt: impl FnMut() -> Result<T>) -> Option<Result<T>> {
    for _ in 0..3 {
        match watch_step(stop, &mut attempt)? {
            Ok(value) => return Some(Ok(value)),
            Err(error) => {
                tracing::debug!(error = %format!("{error:#}"), "display layout change pending; retrying");
                if stop.wait(SETTLE_RETRY) {
                    return None;
                }
            }
        }
    }
    watch_step(stop, attempt)
}
/// The writes that follow the watch. One that dropped gets its entry and layout again
/// when it returns: the first try may have ended halfway, and an entry saved while it
/// was gone isn't its entry. What `put_back` and `restore` answer None to was stopped.
/// What came of the user's layout, None if stopped.
fn after_wake(
    woken: &[String],
    wake: Wake,
    first: Result<()>,
    restored: Result<()>,
    mut put_back: impl FnMut() -> Option<Result<()>>,
    mut restore: impl FnMut() -> Option<Result<()>>,
) -> Option<Result<()>> {
    let put = match wake {
        Wake::Stopped => return None,
        Wake::Stayed if first.is_ok() && restored.is_ok() => return Some(Ok(())),
        Wake::Stayed if first.is_ok() => first,
        Wake::Stayed => put_back()?,
        Wake::CameBack => {
            tracing::info!(displays = ?woken, "a display dropped off the list when it woke and came back; putting its layout back");
            put_back()?
        }
        Wake::StayedAway => {
            tracing::warn!(displays = ?woken, "a display dropped off the list when it woke and did not come back in time");
            first
        }
    };
    if let Err(error) = put {
        tracing::warn!(error = %format!("{error:#}"), "putting back the saved display layout failed");
    }
    restore()
}
/// A watch's thread, its displays and journal behind the arguments. True when it was
/// stopped before it finished ([`left_watch`]). The journal entry stays until the end: a
/// host that dies during the watch has its next start put the layout back, and a stream
/// that starts takes it as its baseline, as what is on by then can be what Windows lit.
fn run_wake_watch(
    end: WakeEnd,
    stop: &WatchStop,
    read: impl FnMut() -> Result<Vec<String>>,
    mut switch_off: impl FnMut() -> Result<()>,
    mut put_back: impl FnMut() -> Result<()>,
    mut restore: impl FnMut() -> Result<()>,
    forget: impl FnOnce() -> Result<()>,
) -> bool {
    let WakeEnd {
        woken,
        restore: restore_due,
        first,
        restored,
    } = end;
    let start = std::time::Instant::now();
    let mut failed_away = false;
    let wake = watch_wake(
        &woken,
        read,
        || start.elapsed(),
        || stop.wait(WAKE_POLL),
        || {
            if let Some(Err(error)) = watch_step(stop, &mut switch_off)
                && !std::mem::replace(&mut failed_away, true)
            {
                tracing::warn!(error = %format!("{error:#}"), "switching off a display Windows recalled while the user's monitor is away failed");
            }
        },
    );
    let layout = after_wake(
        &woken,
        wake,
        first,
        restored,
        || settled_step(stop, &mut put_back),
        || {
            if restore_due {
                settled_step(stop, &mut restore)
            } else {
                Some(Ok(()))
            }
        },
    );
    let cleared = layout.and_then(|result| match result {
        Ok(()) => watch_step(stop, forget),
        Err(error) => Some(Err(error)),
    });
    match cleared {
        Some(Ok(())) => false,
        Some(Err(error)) => {
            tracing::warn!(error = %format!("{error:#}"), "display arrangement restoration remains pending");
            false
        }
        None => {
            tracing::info!(displays = ?woken, "a launch or the host's stop ended the watch over a woken display");
            true
        }
    }
}
/// Start the watch, which owns what `remains` of the stream: its display above all.
fn start_wake_watch(
    before: Snapshot,
    target: String,
    end: WakeEnd,
    remains: Option<Box<dyn Send>>,
) {
    let layout_back = end.restored.is_ok();
    let left = LeftWatch {
        woken: end.woken.clone(),
        restore: end.restore,
    };
    let start = WAKE_WATCHER.start(left, move |stop| {
        tracing::info!(displays = ?end.woken, "keeping the stream's display in the background until the woken display has stayed or returned");
        crate::input::on_input_desktop(move || {
            let left = run_wake_watch(
                end,
                stop,
                connected,
                || switch_off_strays(&before.nodes, &[], true),
                || forget_saved_layout(&before, &target, true),
                || connected().and_then(|c| present_layout(&before, &c).restore()),
                || crate::display_recovery::arrangement(None),
            );
            drop(remains);
            left
        })
    });
    if entry_goes_unwatched(&start, layout_back) {
        let state = state().lock().unwrap();
        if state.users == 0
            && let Err(error) = crate::display_recovery::arrangement(None)
        {
            tracing::warn!(error = %format!("{error:#}"), "display arrangement journal cleanup failed");
        }
    }
}
/// With no thread for its watch nothing comes back for a stream's journal entry, which
/// would turn up as the baseline of a stream much later. It goes if the layout went back,
/// as at an end with no watch; if not, it is what the next stream or start puts back.
fn entry_goes_unwatched(start: &WatchStart, layout_back: bool) -> bool {
    *start == WatchStart::NoThread && layout_back
}
/// How long Windows gets to settle between tries ([`settled`]).
const SETTLE_RETRY: std::time::Duration = std::time::Duration::from_millis(250);
/// Retry while Windows is still applying another change, such as a virtual
/// display being removed: reading its mode fails until the change settles.
fn settled<T>(mut attempt: impl FnMut() -> Result<T>) -> Result<T> {
    for _ in 0..3 {
        match attempt() {
            Ok(value) => return Ok(value),
            Err(error) => {
                tracing::debug!(error = %format!("{error:#}"), "display layout change pending; retrying");
                std::thread::sleep(SETTLE_RETRY);
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
    /// Why what Windows recalled when this stream's display arrived could not be put back.
    repair_failure: Option<String>,
    /// What goes after the lease ([`Lease::end`]).
    remains: Mutex<Option<Box<dyn Send>>>,
    /// See [`Lease::after_cut_watch`].
    after_cut: bool,
}
/// Log a repair that failed, and return what the stream's card says about it.
fn failed_repair(what: &str, error: anyhow::Error) -> String {
    let error = format!("{error:#}");
    tracing::warn!(error = %error, "{what}");
    error
}
/// Repair what Windows recalled, then lay out. A repair that failed doesn't stop the layout;
/// it comes back for the stream's card. A layout that failed stays due for the next try.
fn repair_then_lay_out(
    repair: impl FnOnce() -> Result<()>,
    lay_out: impl FnOnce() -> Result<()>,
) -> Result<Option<String>> {
    let failure = repair()
        .err()
        .map(|error| failed_repair("putting back the displays Windows recalled failed", error));
    lay_out()?;
    Ok(failure)
}
impl Lease {
    pub fn exclusive(&self) -> bool {
        self.arrangement == Arrangement::Exclusive
    }
    /// The layout in force for this stream: the running streams', if it joined them.
    pub fn arrangement(&self) -> Arrangement {
        self.arrangement
    }
    /// Why what Windows recalled when this stream's display arrived could not be put back.
    pub fn repair_failure(&self) -> Option<&str> {
        self.repair_failure.as_deref()
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
            let after_cut = joins_a_cut_watch(state.following.as_ref());
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
                    repair_failure: None,
                    remains: Mutex::default(),
                    after_cut,
                });
            }
            // The displays that are on now can hold an entry a stream on another
            // layout left: the monitor off, or at another mode.
            let mut repair_failure = None;
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
                repair_failure = forget_saved_layout(original, &target_id, repair_saves(&state))
                    .err()
                    .map(|error| {
                        failed_repair("putting back the saved display layout failed", error)
                    });
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
                repair_failure,
                remains: Mutex::default(),
                after_cut,
            });
        }
        let mut current = Snapshot::capture()?;
        let pending = crate::display_recovery::pending_arrangement()?;
        let was_pending = pending.is_some();
        let before = first_baseline(
            pending,
            original,
            &current,
            &target_id,
            &retained,
            virtual_target,
        );
        // A host that stopped without putting back the layout it saved for
        // Windows to recall (a crash, a power cut), or a version that put it back
        // wrong, left the user's displays off, at another mode or no longer primary
        // whenever this virtual display arrives. Put it back, unless this stream
        // saves its own.
        let mut repair_failure = None;
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
                    repair_failure = Some(failed_repair(
                        "putting back the saved display layout failed",
                        error,
                    ));
                }
            }
        }
        let following = if was_pending {
            wake_follow(
                arrangement,
                &before.nodes,
                &connected()?,
                &target_id,
                &retained,
            )
        } else {
            None
        };
        if following.is_some() {
            // What Windows lit while the woken display is away, before anything is composed
            // around it. An exclusive layout takes it off by itself.
            let mut keep = retained.clone();
            keep.push(target_id.clone());
            match switch_off_strays(&before.nodes, &keep, true).and_then(|()| Snapshot::capture()) {
                Ok(recaptured) => current = recaptured,
                Err(error) => {
                    repair_failure = Some(failed_repair(
                        "switching off a display Windows recalled while the user's monitor is away failed",
                        error,
                    ));
                }
            }
        }
        let desired = first_layout(
            arrangement,
            &current.nodes,
            &target_id,
            &retained,
            &before.nodes,
            virtual_target,
        )?;
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
            // Only restoring that baseline permits clearing its journal, and with a
            // display of it away that isn't done: see `left_watch`.
            before.restore()?;
            if !was_pending || before.displays_connected() {
                crate::display_recovery::arrangement(None)?;
            }
            return Err(error);
        }
        let after_cut = following.is_some();
        state.following = following;
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
            repair_failure,
            remains: Mutex::default(),
            after_cut,
        })
    }
    /// End the lease, and with it what `remains` of the stream: its display above all,
    /// which outlasts the lease only as long as the watch over a woken display needs it.
    /// That takes no one's time: this returns once the layout went back.
    pub fn end(self, remains: impl Send + 'static) {
        *self.remains.lock().unwrap() = Some(Box::new(remains));
    }
    /// One look on the heartbeat at a display a stopped watch was waiting for, on a layout
    /// that took that up ([`WakeFollow`]). True when its return took a layout pass that can
    /// have recalled the stream's display's mode. An error is the last try's.
    pub fn follow_wake(&self) -> Result<bool> {
        let mut guard = state().lock().unwrap();
        let state = &mut *guard;
        let Some(follow) = state.following.as_mut().filter(|f| !f.done) else {
            return Ok(false);
        };
        let listed = connected().ok();
        let step = follow.step(follow.started.elapsed(), listed.as_deref());
        match step {
            FollowStep::Nothing => Ok(false),
            FollowStep::SwitchOffStrays => {
                let before = state.before.as_ref().map_or(&[][..], |b| &b.nodes);
                let switched = Snapshot::capture().and_then(|current| {
                    let (keep, arrives) = beside_the_streams(
                        state,
                        &current.nodes,
                        &physical_displays()?,
                        &crate::display::leased_displays(),
                    );
                    switch_off_strays(before, &keep, !arrives)
                });
                let follow = state.following.as_mut().expect("checked above");
                if let Err(error) = switched
                    && !std::mem::replace(&mut follow.failed_strays, true)
                {
                    tracing::warn!(error = %format!("{error:#}"), "switching off a display Windows recalled while the user's monitor is away failed");
                }
                Ok(false)
            }
            FollowStep::LayOut => {
                let arrangement = state.arrangement.unwrap_or(self.arrangement);
                let result = lay_out_after_wake(state, arrangement);
                let follow = state.following.as_mut().expect("checked above");
                match result {
                    Ok((put_back, failure)) => {
                        follow.tried(true);
                        match failure {
                            Some(failure) => Err(anyhow::anyhow!(failure)),
                            None => Ok(put_back),
                        }
                    }
                    Err(error) if follow.tried(false) => {
                        tracing::debug!(error = %format!("{error:#}"), "display layout change pending; retrying");
                        Ok(false)
                    }
                    Err(error) => Err(error),
                }
            }
        }
    }
    /// This stream took up what a stopped watch left, or joined while that was at work: what
    /// is on can have been up to Windows at any time in it, a monitor away for one. Nobody
    /// chose that. One joining later has a layout like any other.
    pub fn after_cut_watch(&self) -> bool {
        self.after_cut
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
    pub fn reapply_after_departure(
        &self,
        output: &str,
        retained: &[String],
    ) -> Result<Option<String>> {
        repair_then_lay_out(
            || {
                let state = state().lock().unwrap();
                let arrangement = state.arrangement.unwrap_or(self.arrangement);
                undo_recall_after_departure(&state, arrangement)
            },
            || self.reapply(output, retained),
        )
    }
}
/// The last lease on a layout ended: put back the entry it `saved` and the user's layout.
/// What to watch afterwards, in which case the journal entry stays for the watch to clear.
fn end_layout(state: &State, target: &str, saved: bool) -> Result<Option<WakeEnd>> {
    // Put the saved entry back even when the user changed the layout:
    // Windows recalls the entry for the remaining displays anyway once
    // the virtual display is removed.
    let restore = state
        .before
        .as_ref()
        .is_some_and(|before| disabled_original_displays(&before.nodes, &state.applied))
        || settled(|| unchanged(&state.applied))?;
    let Some(before) = state.before.as_ref() else {
        crate::display_recovery::arrangement(None)?;
        return Ok(None);
    };
    let woken = if saved {
        displays_to_watch(before, target)
    } else {
        Vec::new()
    };
    if !woken.is_empty() {
        // One try each: a display dropping now fails them, and the watch tries again.
        let first = forget_saved_layout(before, target, true);
        let restored = if restore { before.restore() } else { Ok(()) };
        return Ok(Some(WakeEnd {
            woken,
            restore,
            first,
            restored,
        }));
    }
    if saved && let Err(error) = settled(|| forget_saved_layout(before, target, true)) {
        tracing::warn!(error = %format!("{error:#}"), "putting back the saved display layout failed");
    }
    if restore {
        settled(|| before.restore())?;
    }
    crate::display_recovery::arrangement(None)?;
    Ok(None)
}
impl Drop for Lease {
    fn drop(&mut self) {
        // Declared before the lock, so dropped after it: removing a display takes a while.
        let mut remains = self.remains.get_mut().unwrap().take();
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
        let ended = end_layout(&state, &target, saved);
        let before = state.before.take();
        state.persisted = false;
        state.persists_for.clear();
        state.applied.clear();
        state.key.clear();
        state.owners.clear();
        state.arrangement = None;
        state.retained.clear();
        state.following = None;
        drop(state);
        match what_an_end_leaves(ended, before, &target, &crate::display::leased_displays()) {
            EndLeaves::Nothing => {}
            EndLeaves::Watch(before, end) => {
                start_wake_watch(before, target, end, remains.take());
            }
            EndLeaves::Left(left) => start_left_watch(left),
        }
    }
}
/// What the last lease's end leaves to a thread.
enum EndLeaves {
    Nothing,
    Watch(Snapshot, WakeEnd),
    Left(LeftWatch),
}
/// An end that failed goes to a watch as a cut one does ([`left_watch`]). What fails is a
/// display of the layout dropping as this switched it on: the others alone with nothing at
/// the origin, Windows answers invalid parameter until it returns. The journal entry must
/// not sit until the next stream takes it for a baseline, so the layout goes back then.
fn what_an_end_leaves(
    ended: Result<Option<WakeEnd>>,
    before: Option<Snapshot>,
    target: &str,
    held: &[String],
) -> EndLeaves {
    match (ended, before) {
        (Ok(Some(end)), Some(before)) => EndLeaves::Watch(before, end),
        (Ok(_), _) => EndLeaves::Nothing,
        (Err(error), Some(before)) => {
            tracing::warn!(error = %format!("{error:#}"), "the user's layout did not go back at a stream's end; trying again once its displays stayed or returned");
            EndLeaves::Left(LeftWatch {
                woken: before
                    .nodes
                    .iter()
                    .filter(|n| n.active && n.device_id != target && !held.contains(&n.device_id))
                    .map(|n| n.device_id.clone())
                    .collect(),
                restore: true,
            })
        }
        (Err(error), None) => {
            tracing::warn!(error = %format!("{error:#}"), "display arrangement restoration remains pending");
            EndLeaves::Nothing
        }
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
