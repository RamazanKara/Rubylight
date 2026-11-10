//! The host's sessions: launches waiting for their clients, live streams
//! and the ones being torn down.
use super::{Launch, Role, Session, Transport};
use crate::rtsp::Negotiated;
use anyhow::{Result, bail};
use std::{collections::BTreeMap, net::IpAddr, sync::Arc, time::Instant};

pub struct Sessions<P = (), A = ()> {
    pub pending: BTreeMap<String, Launch<P, A>>,
    pub active: BTreeMap<String, Arc<Session<P, A>>>,
    pub teardown: BTreeMap<String, Instant>,
    /// Told when a launch is queued or a stream starts ([`Sessions::with_waker`]).
    waker: Option<Arc<dyn Fn() + Send + Sync>>,
}
impl<P, A> Default for Sessions<P, A> {
    fn default() -> Self {
        Self {
            pending: BTreeMap::new(),
            active: BTreeMap::new(),
            teardown: BTreeMap::new(),
            waker: None,
        }
    }
}
impl<P, A> Sessions<P, A> {
    /// `waker` is called, with the sessions locked, whenever a launch is queued or a
    /// stream starts: a loop that idles while the host has no sessions wakes for them.
    pub fn with_waker(mut self, waker: impl Fn() + Send + Sync + 'static) -> Self {
        self.waker = Some(Arc::new(waker));
        self
    }
    fn wake(&self) {
        if let Some(waker) = &self.waker {
            waker();
        }
    }
    /// Remove the launches whose clients never started their streams, each
    /// with a warning: the device got a launch reply and then gave up.
    #[must_use = "drop expired launches after releasing the sessions lock"]
    pub fn expire(&mut self) -> Vec<Launch<P, A>> {
        let expired = self.remove_pending(|p| !p.live());
        for launch in &expired {
            tracing::warn!(
                client = %launch.client.name,
                id = %launch.id,
                claimed = launch.claimed_until.is_some(),
                "launch expired before its stream started"
            );
        }
        expired
    }
    /// An authenticated RTSP request for launch `id` claims it: the launch
    /// then waits [`RTSP_LEASE`] for the next request, even past
    /// [`LAUNCH_WAIT`], up to [`CLAIM_LIMIT`]. A launch still being prepared,
    /// expired, replaced or already started is refused; the request of a
    /// running stream (its TEARDOWN) passes. Only [`Sessions::start`]
    /// consumes a launch, once.
    pub fn claim(&mut self, id: &str) -> Result<()> {
        let Some(launch) = self.pending.get_mut(id) else {
            if self.active.get(id).is_some_and(|s| !s.stopping()) {
                return Ok(());
            }
            bail!("the launch was replaced, canceled or already started");
        };
        launch.claimable()?;
        launch.claimed_until = Some(launch.lease(Instant::now()));
        Ok(())
    }
    fn remove_pending(
        &mut self,
        mut matches: impl FnMut(&Launch<P, A>) -> bool,
    ) -> Vec<Launch<P, A>> {
        let removed: Vec<_> = self
            .pending
            .iter()
            .filter(|(_, p)| matches(p))
            .map(|(id, _)| id.clone())
            .collect();
        removed
            .iter()
            .filter_map(|id| self.pending.remove(id))
            .collect()
    }
    /// Withdraw a client's launches and streams in one role. Moonlight starts a
    /// stream only after abandoning its previous one, which may never have
    /// connected or may not have timed out yet; either would otherwise block
    /// the new attempt. Returns the removed launches for restoration after unlocking.
    #[must_use = "drop canceled launches after releasing the sessions lock"]
    pub fn supersede(&mut self, client: &str, role: Role) -> Vec<Launch<P, A>> {
        self.stop_role(role, Some(client))
    }
    /// Queue a launch, replacing the same client's earlier launch in its role.
    /// Drop the returned launches after releasing the sessions lock.
    pub fn queue(&mut self, launch: Launch<P, A>) -> Result<Vec<Launch<P, A>>> {
        let replaced =
            |p: &Launch<P, A>| p.client.uuid == launch.client.uuid && p.role == launch.role;
        let streaming = self
            .active
            .values()
            .filter(|s| !s.stopping() && !replaced(&s.launch))
            .count();
        if self
            .pending
            .values()
            .filter(|p| p.live() && !replaced(p))
            .count()
            + streaming
            >= 16
        {
            bail!("session limit reached");
        }
        let mut canceled = self.supersede(&launch.client.uuid, launch.role);
        canceled.extend(self.pending.insert(launch.id.clone(), launch));
        self.wake();
        Ok(canceled)
    }
    pub fn pending_for_peer(&self, peer: IpAddr) -> Result<Launch<P, A>> {
        let mut found = self
            .pending
            .values()
            .filter(|p| p.peer == peer && p.claimable().is_ok());
        let p = found.next().cloned();
        if found.next().is_some() {
            bail!("ambiguous pending session identity");
        }
        p.ok_or_else(|| anyhow::anyhow!("no authorized launch for RTSP peer"))
    }
    /// Authentication and negotiation snapshots only. `start` consumes the
    /// stored pending launch, including its platform preparation, at PLAY.
    pub fn rtsp_for_peer(&self, peer: IpAddr) -> Vec<Launch<P, A>> {
        self.pending
            .values()
            .filter(|p| p.claimable().is_ok())
            .chain(
                self.active
                    .values()
                    .filter(|s| !s.stopping())
                    .map(|s| &s.launch),
            )
            .filter(|p| p.peer == peer)
            .map(Launch::rtsp_snapshot)
            .collect()
    }
    pub fn start(
        &mut self,
        launch: Launch<P, A>,
        config: Negotiated,
    ) -> Result<Arc<Session<P, A>>> {
        // RTSP authenticates before awaiting the request. Recheck the stored
        // launch at PLAY so a stale snapshot cannot revive an expired launch,
        // or start one still being prepared. Leave expired preparations for
        // expire() to release outside the lock.
        match self.pending.get(&launch.id) {
            None => bail!("launch expired or already consumed"),
            Some(pending) => pending.claimable()?,
        }
        let launch = self.pending.remove(&launch.id).unwrap();
        let s = Session::new(launch, config);
        self.active.insert(s.launch.id.clone(), s.clone());
        self.wake();
        Ok(s)
    }
    pub fn begin_teardown(&mut self, session: &Session<P, A>) {
        self.active.remove(&session.launch.id);
        self.teardown
            .insert(session.launch.id.clone(), Instant::now());
    }
    pub fn fail_start(&mut self, session: &Session<P, A>, reason: impl Into<String>) {
        session.fail(reason);
        self.active.remove(&session.launch.id);
    }
    #[must_use = "drop canceled launches after releasing the sessions lock"]
    pub fn request_stop(&mut self, id: Option<&str>) -> Vec<Launch<P, A>> {
        let canceled =
            self.remove_pending(|p| id.is_none_or(|id| p.client.uuid == id || p.id == id));
        for s in self.active.values() {
            if id.is_none_or(|id| s.launch.client.uuid == id || s.launch.id == id) {
                s.stop();
            }
        }
        canceled
    }
    pub fn owns_capture(&self) -> bool {
        !self.pending.is_empty() || !self.active.is_empty() || !self.teardown.is_empty()
    }
    /// No stream runs, tears down or is about to start. Unlike
    /// `owns_capture`, an expired launch that nothing has removed yet, such
    /// as one the client abandoned, does not count.
    pub fn idle(&self) -> bool {
        self.active.is_empty()
            && self.teardown.is_empty()
            && !self.pending.values().any(Launch::live)
    }
    /// A stream launch waits for its client, or is still being prepared.
    pub fn stream_pending(&self) -> bool {
        self.pending
            .values()
            .any(|p| p.role == Role::Stream && p.live())
    }
    /// The devices with a running (not stopping) stream session.
    pub fn streaming_devices(&self) -> std::collections::BTreeSet<String> {
        self.active
            .values()
            .filter(|s| s.launch.role == Role::Stream && !s.stopping())
            .map(|s| s.launch.client.uuid.clone())
            .collect()
    }
    /// A new session over one transport ends the sessions of the other in the same role: a
    /// WebRTC stream and a Moonlight stream never run together, whichever came first. Sessions
    /// of the same transport follow the per-client rules of `supersede`. A Nucleus Co-op stream
    /// streams a game window of its own and is never ended this way.
    #[must_use = "drop canceled launches after releasing the sessions lock"]
    pub fn supersede_other_transport(
        &mut self,
        role: Role,
        transport: Transport,
    ) -> Vec<Launch<P, A>> {
        let other =
            |l: &Launch<P, A>| l.role == role && l.transport != transport && l.nucleus.is_none();
        let canceled = self.remove_pending(&other);
        for s in self.active.values() {
            if other(&s.launch) {
                s.stop();
            }
        }
        canceled
    }
    /// Whether another device has a live session of this transport and role: pending, or running
    /// and not stopping.
    pub fn held_by_other(&self, role: Role, transport: Transport, client: &str) -> bool {
        let other = |l: &Launch<P, A>| {
            l.role == role && l.transport == transport && l.client.uuid != client
        };
        self.pending.values().any(|p| p.live() && other(p))
            || self
                .active
                .values()
                .any(|s| !s.stopping() && other(&s.launch))
    }
    /// The devices with a running plain stream, by name: `Role::Stream` and not Nucleus Co-op's
    /// or a background one. These are the streams Continue here moves.
    pub fn plain_streams(&self) -> BTreeMap<String, String> {
        self.active
            .values()
            .filter(|s| !s.stopping() && s.launch.plain_stream())
            .map(|s| (s.launch.client.uuid.clone(), s.launch.client.name.clone()))
            .collect()
    }
    /// The devices whose plain stream is about to start, by name.
    pub fn plain_launches(&self) -> BTreeMap<String, String> {
        self.pending
            .values()
            .filter(|p| p.live() && p.plain_stream())
            .map(|p| (p.client.uuid.clone(), p.client.name.clone()))
            .collect()
    }
    /// Whether another device's plain WebRTC stream runs: the one guests watch.
    pub fn plain_webrtc_of_other(&self, client: &str) -> bool {
        self.active.values().any(|s| {
            !s.stopping()
                && s.launch.plain_stream()
                && s.launch.transport == Transport::WebRtc
                && s.launch.client.uuid != client
        })
    }
    /// Whether another device's plain WebRTC stream runs or is about to: what a Moonlight stream of
    /// this device would end (`supersede_other_transport`).
    pub fn plain_webrtc_held_by_other(&self, client: &str) -> bool {
        self.plain_webrtc_of_other(client)
            || self.pending.values().any(|p| {
                p.live()
                    && p.plain_stream()
                    && p.transport == Transport::WebRtc
                    && p.client.uuid != client
            })
    }
    /// Continue here: stop every other device's plain stream and launch. A Nucleus Co-op or
    /// background stream, an Input-only or Remote Monitor session and the caller's own stay.
    #[must_use = "drop canceled launches after releasing the sessions lock"]
    pub fn stop_plain_streams_of_others(&mut self, client: &str) -> Vec<Launch<P, A>> {
        let other = |l: &Launch<P, A>| l.plain_stream() && l.client.uuid != client;
        let canceled = self.remove_pending(other);
        for s in self.active.values() {
            if other(&s.launch) {
                s.stop();
            }
        }
        canceled
    }
    #[must_use = "drop canceled launches after releasing the sessions lock"]
    pub fn stop_role(&mut self, role: Role, client: Option<&str>) -> Vec<Launch<P, A>> {
        let canceled =
            self.remove_pending(|p| p.role == role && client.is_none_or(|id| p.client.uuid == id));
        for s in self.active.values() {
            if s.launch.role == role && client.is_none_or(|id| id == s.launch.client.uuid) {
                s.stop();
            }
        }
        canceled
    }
}
