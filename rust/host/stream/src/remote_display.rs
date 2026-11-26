use crate::state::Shared;
use anyhow::{Context, Result, bail};
use butterpollo_media_policy::{
    rtsp::Negotiated,
    topology::{Kind, Layout, Node},
};
use butterpollo_session::session::{Role, Warnings};
use butterpollo_win_display::{
    display::{self, Retained},
    display_arrangement,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};

pub fn layout(h: &Shared) -> Result<Layout> {
    let state = h.paired.read().unwrap();
    match state.document["root"].get("remote_display_layout") {
        Some(Value::String(s)) => Ok(serde_json::from_str(s)?),
        Some(value) => Ok(serde_json::from_value(value.clone())?),
        None => Ok(Layout::default()),
    }
}
fn nodes(h: &Shared) -> Result<Vec<Node>> {
    let monitors = display::monitors()?;
    let retained = h.monitors.lock().unwrap();
    let paired = h.paired.read().unwrap();
    let mut nodes = display::baseline_nodes()?;
    for (id, lease) in retained.iter() {
        let monitor = monitors
            .iter()
            .find(|m| m.display_name == lease.current_output())
            .context("retained display disappeared")?;
        let node = nodes
            .iter_mut()
            .find(|n| n.device_id == monitor.device_id)
            .context("retained display mode unavailable")?;
        node.kind = Kind::Client;
        node.id = id.clone();
        node.label = paired
            .clients
            .iter()
            .find(|c| c.uuid == butterpollo_session::state::device_uuid(id))
            .map_or(id.clone(), |c| c.name.clone());
    }
    Ok(nodes)
}
pub fn snapshot(h: &Shared) -> Result<Value> {
    let layout = layout(h)?;
    let current = nodes(h)?;
    let desired = layout.compose(&current)?;
    let nodes: Vec<_> = desired
        .iter()
        .map(|n| {
            let mut value = serde_json::to_value(n).unwrap();
            value["current_position"] = current
                .iter()
                .find(|old| old.id == n.id)
                .map(|old| json!(old.desired_position))
                .unwrap_or(Value::Null);
            value
        })
        .collect();
    let clients: Vec<_> = h
        .paired
        .read()
        .unwrap()
        .clients
        .iter()
        .map(|c| json!({"uuid":c.uuid,"name":c.name,"enabled":c.enabled}))
        .collect();
    let runtime: serde_json::Map<_,_> = h.monitors.lock().unwrap().keys().map(|id|(id.clone(),json!({"lifecycle":"ready","ready":true,"retryable":false,"lease_held":true,"warning":"","plaintext_rtsp_warning":""}))).collect();
    Ok(
        json!({"status":true,"version":1,"layout":layout,"capacity":{"max":4,"used":runtime.len()},"warnings":[],"nodes":nodes,"clients":clients,"runtime":runtime}),
    )
}
pub fn save(h: &Shared, value: &Value) -> Result<Value> {
    let next: Layout = serde_json::from_value(value.get("layout").unwrap_or(value).clone())?;
    let clients = h
        .paired
        .read()
        .unwrap()
        .clients
        .iter()
        .map(|c| c.uuid.clone())
        .collect::<Vec<_>>();
    let physical = nodes(h)?
        .into_iter()
        .filter(|n| n.kind == Kind::Physical)
        .map(|n| n.id)
        .collect::<Vec<_>>();
    next.validate(&clients, &physical)?;
    let mut state = h.paired.write().unwrap();
    let mut document = state.clone();
    document.document["root"]["remote_display_layout"] = json!(serde_json::to_string(&next)?);
    document.save(&h.paired_path)?;
    *state = document;
    Ok(json!({"status":true,"applies_on_next_activation":true,"layout":next}))
}
/// What a remote monitor's arrival changed stays changed: the launch's card says so.
fn report_repair(warnings: &Warnings, result: Result<()>) {
    if let Err(error) = result {
        let error = format!("{error:#}");
        tracing::warn!(error = %error, "putting back the displays a remote monitor's arrival changed failed");
        warnings.set("display_monitor_repair", format!("Windows recalled a saved display layout when the remote monitor arrived and it could not be put back ({error}), so a monitor can stay off or at another refresh rate. Check Windows display settings and reconnect."));
    }
}
/// The remote monitors being made, by client, and whether a disconnect cancelled one: the
/// virtual display is made outside the monitors lock, which `/applist`, `/serverinfo` and the
/// maintenance loop read. Taken under the monitors lock, never the other way round.
#[derive(Default)]
pub struct Creating {
    clients: Mutex<BTreeMap<String, bool>>,
    done: Condvar,
}
impl Creating {
    fn locked(&self) -> MutexGuard<'_, BTreeMap<String, bool>> {
        self.clients.lock().unwrap_or_else(PoisonError::into_inner)
    }
    /// A disconnect of `client` (None: every client) while its monitor is being made: it is
    /// removed as soon as it is there. Called under the monitors lock.
    pub fn cancel(&self, client: Option<&str>) {
        for (id, cancelled) in self.locked().iter_mut() {
            if client.is_none_or(|client| client == id.as_str()) {
                *cancelled = true;
            }
        }
    }
}
/// A remote monitor's place, held while it is made.
struct Slot<'a> {
    h: &'a Shared,
    client: String,
}
impl Drop for Slot<'_> {
    fn drop(&mut self) {
        self.h.monitors_creating.locked().remove(&self.client);
        self.h.monitors_creating.done.notify_all();
    }
}
/// What [`reserve`] found for a client.
enum Reserved<'a> {
    /// Its monitor is there already.
    Existing(Arc<Retained>),
    /// None: the caller makes it and [`publish`]es it.
    Slot(Slot<'a>),
}
/// The monitor of `client` in `mode`, or the place to make it in. A second request while it
/// is made waits for it, then takes it as made.
fn reserve<'a>(h: &'a Shared, client: &str, mode: (u32, u32, u32, bool)) -> Result<Reserved<'a>> {
    loop {
        let monitors = h.monitors.lock().unwrap();
        if let Some(existing) = monitors.get(client) {
            if existing.mode != mode {
                bail!("retained monitor uses a different mode; disconnect it before changing mode");
            }
            return Ok(Reserved::Existing(existing.clone()));
        }
        let mut creating = h.monitors_creating.locked();
        if creating.contains_key(client) {
            drop(monitors);
            drop(
                h.monitors_creating
                    .done
                    .wait_while(creating, |creating| creating.contains_key(client))
                    .unwrap_or_else(PoisonError::into_inner),
            );
            continue;
        }
        if monitors.len() + creating.len() >= 4 {
            bail!("remote monitor capacity reached");
        }
        creating.insert(client.to_owned(), false);
        return Ok(Reserved::Slot(Slot {
            h,
            client: client.to_owned(),
        }));
    }
}
/// The monitor made in `slot` goes in, unless a disconnect cancelled it meanwhile: then it is
/// dropped, outside the lock, which removes it from Windows.
fn publish(slot: Slot, lease: Arc<Retained>) -> Result<Arc<Retained>> {
    let h = slot.h;
    let mut monitors = h.monitors.lock().unwrap();
    let cancelled = h
        .monitors_creating
        .locked()
        .get(&slot.client)
        .copied()
        .unwrap_or(true);
    if cancelled {
        drop(slot);
        drop(monitors);
        drop(lease);
        bail!("the remote monitor's device disconnected while its display was being made");
    }
    monitors.insert(slot.client.clone(), lease.clone());
    // Waiters find it in the map once the slot goes.
    drop(slot);
    drop(monitors);
    Ok(lease)
}
/// The remote monitors' displays, once none is still being made: a stream laying out its
/// displays keeps them on.
pub(crate) fn outputs(h: &Shared) -> Vec<String> {
    loop {
        drop(
            h.monitors_creating
                .done
                .wait_while(h.monitors_creating.locked(), |creating| {
                    !creating.is_empty()
                })
                .unwrap_or_else(PoisonError::into_inner),
        );
        let monitors = h.monitors.lock().unwrap();
        if h.monitors_creating.locked().is_empty() {
            return monitors.values().map(|m| m.current_output()).collect();
        }
    }
}
pub fn activate(
    h: &Shared,
    client: &str,
    config: &Negotiated,
    warnings: &Warnings,
) -> Result<(Arc<Retained>, bool)> {
    let (display_width, display_height) = config.display_size();
    let mode = (
        display_width,
        display_height,
        config.fps_millihz(),
        config.hdr,
    );
    let slot = match reserve(h, client, mode)? {
        Reserved::Existing(existing) => return Ok((existing, false)),
        Reserved::Slot(slot) => slot,
    };
    let identity = h
        .paired
        .read()
        .unwrap()
        .clients
        .iter()
        .find(|c| c.uuid == butterpollo_session::state::device_uuid(client))
        .cloned();
    let options = display::VirtualOptions {
        peak_nits: identity
            .as_ref()
            .and_then(|c| c.extra.get("hdr_profile"))
            .and_then(Value::as_str)
            .and_then(|p| {
                butterpollo_win_display::hdr_profile::peak_luminance(p)
                    .ok()
                    .flatten()
            })
            .unwrap_or(1000)
            .clamp(400, 2000),
        render_adapter: crate::win_bridge::configured_render_adapter(&h.config.read().unwrap()),
        render_resolver: Some(crate::win_bridge::render_resolver(
            &h.config.read().unwrap(),
            None,
        )),
        joins: false,
    };
    // Read while the display is not there yet: Windows can recall a saved layout
    // over the others when it arrives.
    let before = display::Snapshot::capture()
        .inspect_err(|error| {
            tracing::warn!(error = %format!("{error:#}"), "display layout before the remote monitor could not be read")
        })
        .ok();
    let host = Arc::downgrade(h);
    let lease = Arc::new(Retained::create_options(
        client,
        display_width,
        display_height,
        butterpollo_media_policy::framegen::Rate(config.fps_millihz()),
        config.hdr,
        &options,
        Some(Box::new(move || {
            if let Some(h) = host.upgrade() {
                let _layout = layout_turn();
                let desired = layout(&h)?.compose(&nodes(&h)?)?;
                display::apply_layout(&desired)?;
            }
            Ok(())
        })),
    )?);
    let lease = publish(slot, lease)?;
    if let Some(before) = &before {
        report_repair(
            warnings,
            display_arrangement::undo_recall_for_remote_monitor(before, &lease.current_output()),
        );
    }
    let result = {
        // Released before the disconnect below, which takes its own turn.
        let _layout = layout_turn();
        (|| -> Result<()> { display::apply_layout(&layout(h)?.compose(&nodes(h)?)?) })()
    };
    if let Err(error) = result {
        disconnect(h, Some(client));
        return Err(error);
    }
    let still_registered = || {
        h.monitors
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(client)
            .is_some_and(|m| Arc::ptr_eq(m, &lease))
    };
    if !still_registered() {
        anyhow::bail!("remote monitor disconnected during activation");
    }
    // A stream holding a display layout must count this display as part of
    // it, or its end leaves the physical displays off.
    if let Err(error) = display_arrangement::retain(&lease.current_output(), true) {
        tracing::warn!(error = %format!("{error:#}"), "remote monitor is not part of the stream's display layout");
    }
    // Re-check in case disconnect raced with retain(..., true).
    if !still_registered() {
        let _ = display_arrangement::retain(&lease.current_output(), false);
        anyhow::bail!("remote monitor disconnected during activation");
    }
    Ok((lease, true))
}
/// One display layout at a time, each from the monitors there at that moment: a monitor ending,
/// one arriving and a feeder's recovery each run on a thread of their own, and would otherwise
/// lay the displays out from each other's half-done state. Never taken under the monitors lock.
static LAYOUT: std::sync::Mutex<()> = std::sync::Mutex::new(());
fn layout_turn() -> std::sync::MutexGuard<'static, ()> {
    LAYOUT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
pub fn disconnect(h: &Shared, client: Option<&str>) {
    let canceled = h
        .sessions
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .stop_role(Role::RemoteMonitor, client);
    drop(canceled);
    let mut monitors = h.monitors.lock().unwrap_or_else(PoisonError::into_inner);
    h.monitors_creating.cancel(client);
    let (kept, removed): (
        std::collections::BTreeMap<_, _>,
        std::collections::BTreeMap<_, _>,
    ) = std::mem::take(&mut *monitors)
        .into_iter()
        .partition(|(id, _)| client.is_some_and(|c| c != id));
    *monitors = kept;
    drop(monitors);
    // While their displays still exist to be named.
    for lease in removed.values() {
        if let Err(error) = display_arrangement::retain(&lease.current_output(), false) {
            tracing::warn!(error = %format!("{error:#}"), "remote monitor is still kept on by the stream's display layout");
        }
    }
    // Dropping the last lease joins its feeder, whose recovery callback takes
    // the monitors lock to lay out the displays: never drop one under it.
    drop(removed);
    let _layout = layout_turn();
    // From what is left now, not from what this call left behind.
    let empty = h.monitors.lock().unwrap().is_empty();
    let result = if empty {
        display::restore_positions()
    } else {
        layout(h)
            .and_then(|layout| layout.compose(&nodes(h)?))
            .and_then(|nodes| display::apply_layout(&nodes))
    };
    if let Err(error) = result {
        tracing::warn!(%error,"remote display topology restoration failed");
    }
}

#[cfg(test)]
mod tests;
