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
struct Watches {
    /// Launches preparing a display: no watch starts, as what it would write can land
    /// on their layout.
    held_off: usize,
    /// The host is stopping: no watch starts again.
    closed: bool,
    /// The thread answers whether it left a journal entry that can go ([`WakeWatcher::release`]).
    running: Option<(Arc<WatchStop>, std::thread::JoinHandle<bool>)>,
    journal_left: bool,
}
impl Watches {
    /// Stop the watch and wait for its thread, which then has dropped the stream's
    /// display and writes nothing more. A thread that panicked left the entry too.
    fn stop_running(&mut self) {
        if let Some((stop, thread)) = self.running.take() {
            stop.stop();
            self.journal_left |= thread.join().unwrap_or(true);
        }
    }
}
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
            journal_left: false,
        }))
    }
    /// Run `job` on its own thread; false when it may not, and it is dropped unrun. It
    /// answers whether it left a journal entry that can go. Never call this holding the
    /// arrangement's lock: a watch still running is waited for, and it may want that lock.
    fn start(&self, job: impl FnOnce(&WatchStop) -> bool + Send + 'static) -> bool {
        let mut watches = self.0.lock().unwrap();
        if watches.closed || watches.held_off != 0 {
            return false;
        }
        watches.stop_running();
        let stop = Arc::new(WatchStop::default());
        let stops = stop.clone();
        match std::thread::Builder::new()
            .name("wake-watch".into())
            .spawn(move || job(&stops))
        {
            Ok(thread) => {
                watches.running = Some((stop, thread));
                true
            }
            Err(error) => {
                tracing::warn!(%error, "the watch over a woken display could not start");
                false
            }
        }
    }
    /// No watch runs or starts until as many [`Self::release`]s.
    fn hold_off(&self) {
        let mut watches = self.0.lock().unwrap();
        watches.held_off += 1;
        watches.stop_running();
    }
    /// True for the last hold to end, when a watch held off or stopped left a journal
    /// entry that a lease may have taken over: if none did, it is to go.
    fn release(&self) -> bool {
        let mut watches = self.0.lock().unwrap();
        watches.held_off -= 1;
        watches.held_off == 0 && std::mem::take(&mut watches.journal_left) && !watches.closed
    }
    /// A stream's end that a hold kept from starting its watch left its journal entry.
    fn journal_left(&self) {
        let mut watches = self.0.lock().unwrap();
        if watches.held_off != 0 {
            watches.journal_left = true;
        }
    }
    /// For good: the host is stopping. What a stopped watch left stays in the journal,
    /// for the next start to put back.
    fn close(&self) {
        let mut watches = self.0.lock().unwrap();
        watches.closed = true;
        watches.stop_running();
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
        if !WAKE_WATCHER.release() {
            return;
        }
        // No lease took up the layout the watch left, as a window's stream or a failed
        // launch takes none: it must not turn up as the baseline of a stream much later.
        let state = state().lock().unwrap();
        if state.users == 0
            && let Err(error) = crate::display_recovery::arrangement(None)
        {
            tracing::warn!(error = %format!("{error:#}"), "display arrangement journal cleanup failed");
        }
    }
}
/// The host is stopping: no watch outlasts that, or starts.
pub fn close_wake_watch() {
    WAKE_WATCHER.close();
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
    let mut gone_since = None;
    loop {
        let now = elapsed();
        let present = read().ok().map(|c| woken.iter().all(|id| c.contains(id)));
        match (gone_since, present) {
            (None, Some(false)) => gone_since = Some(now),
            (Some(_), Some(true)) => return Wake::CameBack,
            _ => {}
        }
        match gone_since {
            None if now >= WAKE_WATCH => return Wake::Stayed,
            Some(at) if now.saturating_sub(at) >= WAKE_RETURN => return Wake::StayedAway,
            Some(_) => away(),
            None => {}
        }
        if sleep() {
            return Wake::Stopped;
        }
    }
}
/// What is on that the user's layout doesn't have, known physical (a stream's virtual
/// display is never one of them), and what stays on without it.
fn strays(before: &[Node], active: &[String], physical: &[String]) -> (Vec<String>, Vec<String>) {
    active.iter().cloned().partition(|id| {
        physical.contains(id) && !before.iter().any(|n| n.active && n.device_id == *id)
    })
}
/// Windows recalled the entry for what is connected while a woken display is away, and
/// that can have a display on that was off before the stream, such as a TV. Off again,
/// and saved so that the entry no longer has it on.
fn switch_off_strays(before: &[Node]) -> Result<()> {
    let active: Vec<_> = Topology::query()?
        .monitors()
        .into_iter()
        .map(|m| m.device_id)
        .collect();
    let (off, on) = strays(before, &active, &physical_displays()?);
    if off.is_empty() || on.is_empty() {
        return Ok(());
    }
    tracing::info!(displays = ?off, "Windows switched on a display the user's layout doesn't have while their monitor is away; switching it off");
    Topology::set_active(&on)?;
    Topology::save_current()
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
/// stopped with the user's layout put back: the journal entry it leaves can go then,
/// unless a lease takes it up. Kept until the end otherwise: a host that dies during the
/// watch has its next start put the layout back, and a stream that starts takes it as
/// its baseline, as what is on by then can be what Windows lit.
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
    let layout_back = restored.is_ok();
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
            layout_back
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
    let started = WAKE_WATCHER.start(move |stop| {
        tracing::info!(displays = ?end.woken, "keeping the stream's display in the background until the woken display has stayed or returned");
        crate::input::on_input_desktop(move || {
            let left = run_wake_watch(
                end,
                stop,
                connected,
                || switch_off_strays(&before.nodes),
                || forget_saved_layout(&before, &target, true),
                || before.restore(),
                || crate::display_recovery::arrangement(None),
            );
            drop(remains);
            left
        })
    });
    if !started && layout_back {
        WAKE_WATCHER.journal_left();
    }
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
    let failure = repair().err().map(|error| {
        failed_repair(
            "putting back the displays another stream's display left failed",
            error,
        )
    });
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
            repair_failure,
            remains: Mutex::default(),
        })
    }
    /// End the lease, and with it what `remains` of the stream: its display above all,
    /// which outlasts the lease only as long as the watch over a woken display needs it.
    /// That takes no one's time: this returns once the layout went back.
    pub fn end(self, remains: impl Send + 'static) {
        *self.remains.lock().unwrap() = Some(Box::new(remains));
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
        let watch = end_layout(&state, &target, saved).unwrap_or_else(|error| {
            tracing::warn!(error = %format!("{error:#}"), "display arrangement restoration remains pending");
            None
        });
        let before = state.before.take();
        state.persisted = false;
        state.persists_for.clear();
        state.applied.clear();
        state.key.clear();
        state.owners.clear();
        state.arrangement = None;
        state.retained.clear();
        drop(state);
        if let (Some(end), Some(before)) = (watch, before) {
            start_wake_watch(before, target, end, remains.take());
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
