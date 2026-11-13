//! Recent, authenticated route loss history. Disk writes happen at stream
//! boundaries, never from packet delivery or the control feedback callback.
use super::{Host, Session, Shared};
use anyhow::{Context, Result};
use butterpollo_core::state::{Client, PairedState};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    net::IpAddr,
    sync::{Arc, atomic::Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const RETENTION: u64 = 24 * 60 * 60;
/// One remembered session, then the fast rate is tried again.
const PROBE_EVERY: u32 = 2;
const MAX_ROUTES: usize = 256;
const FIELD: &str = "keyframe_loss_routes";
/// Only loss that lasted and that the client reported is remembered. Records
/// from before this field existed (stale 24 h latches, mostly from a rough
/// start) are not, nor are rc.4's and older `"sustained"` ones, which also
/// held decoder re-requests and the host's own send loss.
const REPORTED: &str = "reported";

/// What a history is about: the client, its address, the host interface the
/// route leaves by and the client's own link when it reports one. A host that
/// switches network adapter, or a client that moves to another port, starts fresh.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Scope {
    address: IpAddr,
    /// The host's outgoing interface (LUID); zero when unknown.
    interface: u64,
    /// `x-ss-video[0].pyrowaveLinkMbps`; zero when the client sends none.
    client_link_mbps: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Route {
    client_id: String,
    address: IpAddr,
    observed_at: u64,
    /// Distinguishes simultaneous evidence from the history a probe tested.
    revision: String,
    #[serde(default)]
    remembered_sessions: u32,
    /// `"reported"`; anything else, or nothing, is not remembered.
    #[serde(default)]
    kind: String,
    #[serde(default)]
    interface: u64,
    #[serde(default)]
    client_link_mbps: u32,
}

impl Route {
    fn matches(&self, client_id: &str, scope: &Scope) -> bool {
        self.client_id == client_id
            && self.address == scope.address.to_canonical()
            && self.interface == scope.interface
            && self.client_link_mbps == scope.client_link_mbps
    }
    fn current(&self, now: u64) -> bool {
        // A clock moved backwards must not make a route persist indefinitely.
        self.observed_at <= now && now - self.observed_at < RETENTION
    }
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn read(state: &PairedState, now: u64) -> Vec<Route> {
    // Invalid optional cache entries must not invalidate a paired identity.
    let mut routes: Vec<Route> = state.document["root"][FIELD]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| serde_json::from_value::<Route>(v.clone()).ok())
        .filter(|r| {
            r.kind == REPORTED
                && r.current(now)
                && state.clients.iter().any(|c| c.uuid == r.client_id)
        })
        .collect();
    for route in &mut routes {
        route.address = route.address.to_canonical();
    }
    routes.sort_by_key(|r| std::cmp::Reverse(r.observed_at));
    let mut seen = BTreeSet::new();
    routes.retain(|r| {
        seen.insert((
            r.client_id.clone(),
            r.address,
            r.interface,
            r.client_link_mbps,
        ))
    });
    routes.truncate(MAX_ROUTES);
    routes
}
fn save(h: &Host, state: &mut PairedState, routes: &[Route]) -> Result<()> {
    let mut next = state.clone();
    next.document["root"][FIELD] = serde_json::to_value(routes)?;
    next.save(&h.paired_path)?;
    *state = next;
    Ok(())
}

struct Token {
    client: Client,
    scope: Scope,
    epoch: u64,
    /// Only a completed fast probe can forget the exact history it tested.
    probe_revision: Option<String>,
}

/// Keep this guard until the stream loop ends. Its drop records fresh loss or
/// a completed fast probe without holding the session registry lock.
pub(crate) struct Guard {
    host: Shared,
    session: Arc<Session>,
    token: Option<Token>,
    started: Instant,
}
impl Drop for Guard {
    fn drop(&mut self) {
        let Some(token) = self.token.take() else {
            return;
        };
        // The history is the launch address's; a client that roamed sent the stream's
        // loss, or its clean probe, over another path.
        if self.session.roams() > 0 {
            return;
        }
        // A start-up latch that lifted is not remembered, but it is not a
        // clean probe either.
        let observed = self.session.keyframe_loss_sustained_at();
        let healthy_probe = token.probe_revision.is_some()
            && !self.session.keyframe_loss_ever_latched()
            && !self.session.failed()
            && self.started.elapsed() >= Duration::from_secs(5)
            && self.session.stats.frames.load(Ordering::Relaxed) >= 120;
        if observed.is_none() && !healthy_probe {
            return;
        }
        let now = now();
        let observed_at = observed.map(|at| now.saturating_sub(at.elapsed().as_secs()));
        if let Err(error) = self
            .host
            .finish_keyframe_loss(token, observed_at, healthy_probe, now)
        {
            tracing::warn!(%error, "could not save the device's video loss history");
        }
    }
}

impl Host {
    pub(crate) fn configure_keyframe_loss(
        self: &Arc<Self>,
        session: &Arc<Session>,
        enabled: bool,
    ) -> Guard {
        // The host's outgoing interface: asked of Windows before any lock is held.
        let interface = if enabled && session.config.codec != 3 {
            butterpollo_windows::net::routed_link(std::net::SocketAddr::new(session.launch.peer, 0))
                .interface
        } else {
            0
        };
        self.configure_keyframe_loss_on(session, enabled, interface)
    }

    pub(crate) fn configure_keyframe_loss_on(
        self: &Arc<Self>,
        session: &Arc<Session>,
        enabled: bool,
        interface: u64,
    ) -> Guard {
        let mut guard = Guard {
            host: self.clone(),
            session: session.clone(),
            token: None,
            started: Instant::now(),
        };
        if !enabled || session.config.codec == 3 {
            session.configure_keyframe_loss(false);
            return guard;
        }
        let epochs = self.keyframe_loss_epochs.lock().unwrap();
        let mut state = self.paired.write().unwrap();
        // The history is the paired device's: its second screen streams over the same path.
        let client = &Client {
            uuid: butterpollo_core::state::device_uuid(&session.launch.client.uuid).to_owned(),
            ..session.launch.client.clone()
        };
        // Use the TLS-authenticated launch identity, never a name or a UUID
        // supplied by a later packet. A removed/replaced certificate cannot
        // acquire or recreate another pairing's history.
        if !state
            .clients
            .iter()
            .any(|c| c.uuid == client.uuid && c.cert == client.cert)
        {
            session.configure_keyframe_loss(true);
            return guard;
        }
        let scope = Scope {
            address: session.launch.peer.to_canonical(),
            interface,
            client_link_mbps: session.config.pyrowave_client_link_mbps,
        };
        let address = scope.address;
        let mut routes = read(&state, now());
        let mut remembered = false;
        let mut probe_revision = None;
        if let Some(route) = routes.iter_mut().find(|r| r.matches(&client.uuid, &scope)) {
            route.remembered_sessions = route.remembered_sessions.saturating_add(1);
            if route.remembered_sessions >= PROBE_EVERY {
                probe_revision = Some(route.revision.clone());
                route.remembered_sessions = 0;
            } else {
                remembered = true;
            }
            if let Err(error) = save(self, &mut state, &routes) {
                tracing::warn!(%error, "could not save the video pacing probe count");
                // An unwritable profile must not turn every reconnect into a
                // fast probe because the old counter remains at four.
                remembered = true;
                probe_revision = None;
            }
        }
        if probe_revision.is_some() {
            tracing::info!(client = %client.name, %address, interface, "testing the fast video pacing rate after a remembered session");
        }
        session.configure_remembered_keyframe_loss(true, remembered);
        guard.token = Some(Token {
            client: client.clone(),
            scope,
            epoch: *epochs.get(&client.uuid).unwrap_or(&0),
            probe_revision,
        });
        guard
    }

    fn finish_keyframe_loss(
        &self,
        token: Token,
        observed_at: Option<u64>,
        healthy_probe: bool,
        now: u64,
    ) -> Result<()> {
        // One lock order everywhere: epoch map, then paired state. In
        // particular no disk write can race a reset or overwrite client edits.
        let epochs = self.keyframe_loss_epochs.lock().unwrap();
        if *epochs.get(&token.client.uuid).unwrap_or(&0) != token.epoch {
            return Ok(());
        }
        let mut state = self.paired.write().unwrap();
        if !state
            .clients
            .iter()
            .any(|c| c.uuid == token.client.uuid && c.cert == token.client.cert)
        {
            return Ok(());
        }
        let mut routes = read(&state, now);
        if let Some(observed_at) = observed_at.filter(|at| *at <= now && now - at < RETENTION) {
            if let Some(route) = routes
                .iter_mut()
                .find(|r| r.matches(&token.client.uuid, &token.scope))
            {
                if observed_at >= route.observed_at {
                    route.observed_at = observed_at;
                    route.revision = uuid::Uuid::new_v4().to_string();
                    route.remembered_sessions = 0;
                }
            } else {
                routes.push(Route {
                    client_id: token.client.uuid,
                    address: token.scope.address,
                    observed_at,
                    revision: uuid::Uuid::new_v4().to_string(),
                    remembered_sessions: 0,
                    kind: REPORTED.into(),
                    interface: token.scope.interface,
                    client_link_mbps: token.scope.client_link_mbps,
                });
            }
            routes.sort_by_key(|r| std::cmp::Reverse(r.observed_at));
            routes.truncate(MAX_ROUTES);
        } else if healthy_probe {
            routes.retain(|r| {
                !r.matches(&token.client.uuid, &token.scope)
                    || Some(&r.revision) != token.probe_revision.as_ref()
            });
        } else {
            return Ok(());
        }
        save(self, &mut state, &routes)
    }

    pub(crate) fn reset_keyframe_loss(&self, client_id: &str) -> Result<()> {
        let mut epochs = self.keyframe_loss_epochs.lock().unwrap();
        let mut state = self.paired.write().unwrap();
        state
            .clients
            .iter()
            .find(|c| c.uuid == client_id)
            .context("client not found")?;
        let mut routes = read(&state, now());
        routes.retain(|r| r.client_id != client_id);
        save(self, &mut state, &routes)?;
        let epoch = epochs.entry(client_id.to_owned()).or_default();
        *epoch = epoch.wrapping_add(1);
        Ok(())
    }

    /// Removing a pairing and its route cache is one atomic state write. The
    /// old idempotent unpair behavior also covers missing/skipped clients.
    pub(crate) fn unpair_with_network_history(&self, client_id: Option<&str>) -> Result<()> {
        let mut epochs = self.keyframe_loss_epochs.lock().unwrap();
        let mut state = self.paired.write().unwrap();
        let ids: Vec<_> = match client_id {
            Some(id) => vec![id.to_owned()],
            None => state.clients.iter().map(|c| c.uuid.clone()).collect(),
        };
        // The devices that go, for the webhooks once nothing is locked.
        let removed: Vec<_> = state
            .clients
            .iter()
            .filter(|c| ids.contains(&c.uuid))
            .map(|c| crate::webhooks::device(&c.uuid, &c.name))
            .collect();
        let mut routes = read(&state, now());
        routes.retain(|r| client_id.is_some_and(|id| r.client_id != id));
        let mut next = state.clone();
        next.document["root"][FIELD] = serde_json::to_value(routes)?;
        if let Some(id) = client_id {
            next.remove(&self.paired_path, id)?;
        } else {
            next.clear();
            next.save(&self.paired_path)?;
        }
        *state = next;
        let mut agents = self.client_agents.lock().unwrap();
        for id in ids {
            agents.remove(&id);
            let epoch = epochs.entry(id).or_default();
            *epoch = epoch.wrapping_add(1);
        }
        drop((agents, state, epochs));
        for device in removed {
            self.webhook("pairing.unpaired", serde_json::json!({"device": device}));
        }
        Ok(())
    }

    pub(crate) fn keyframe_loss_routes(&self) -> BTreeMap<String, Value> {
        let now = now();
        let state = self.paired.read().unwrap();
        let mut clients = BTreeMap::<String, Vec<Value>>::new();
        for route in read(&state, now) {
            clients.entry(route.client_id).or_default().push(json!({
                "address": route.address,
                "expires_in_seconds": RETENTION - (now - route.observed_at),
                "sessions_until_probe": PROBE_EVERY.saturating_sub(route.remembered_sessions),
            }));
        }
        clients
            .into_iter()
            .map(|(id, routes)| (id, json!(routes)))
            .collect()
    }
}

#[cfg(test)]
mod tests;
