//! Parallel host polling across direct address slots and internet-play fallback,
//! `is_site_local_host` resolution, Desktop address ordering, and `/internet-play/v1` refresh.

use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};

use nightjar_policy::hosts::{
    is_site_local_ip, is_tunnel_address, should_clear_cert_on_unpair, CandidateSlot, PollErrorKind,
    ProbeCandidate, SharedHostFields,
};

use crate::http::{
    Error, HostApiError, HostCapabilities, HostClient, HostClientCanceller, InfoPath, InternetPlay,
    ServerInfo,
};
use crate::internet::{self, Tried};

use super::client_pool::HostClientPool;

/// Successful single-address `/serverinfo` poll.
#[derive(Clone, Debug)]
pub struct PollIpSuccess {
    pub candidate: ProbeCandidate,
    pub info: ServerInfo,
    pub path: InfoPath,
    pub should_clear_cert: bool,
}

/// Failed single-address `/serverinfo` poll.
#[derive(Clone, Debug)]
pub struct PollIpError {
    pub candidate: ProbeCandidate,
    pub kind: PollErrorKind,
    pub message: Option<String>,
}

/// Aggregate result of polling direct candidates in precedence order.
#[derive(Clone, Debug, Default)]
pub struct DirectPollOutcome {
    pub winner: Option<PollIpSuccess>,
    pub another_certificate: bool,
    pub tried: Tried,
    pub last_error: Option<PollErrorKind>,
}

#[derive(Default)]
struct PollCancelState {
    next_id: u64,
    cancellers: Vec<(u64, HostClientCanceller)>,
    wakers: Vec<(u64, Box<dyn FnOnce() + Send>)>,
}

#[derive(Default)]
struct PollCancelInner {
    cancelled: AtomicBool,
    state: Mutex<PollCancelState>,
}

/// Thread-safe cancellation scope tracking active [`HostClientCanceller`]s for a poll pass.
#[derive(Clone, Default)]
pub struct PollCancel(Arc<PollCancelInner>);

/// RAII guard unregistering a [`HostClientCanceller`] or wake callback from [`PollCancel`] on drop.
pub struct PollCancelGuard {
    inner: Arc<PollCancelInner>,
    id: u64,
}

impl Drop for PollCancelGuard {
    fn drop(&mut self) {
        let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
        state.cancellers.retain(|(id, _)| *id != self.id);
        state.wakers.retain(|(id, _)| *id != self.id);
    }
}

impl PollCancel {
    /// Creates an uncancelled [`PollCancel`] scope.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether [`Self::cancel`] has been called on this scope.
    pub fn is_cancelled(&self) -> bool {
        self.0.cancelled.load(Ordering::Acquire)
    }

    pub(super) fn ptr_eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    /// Registers `canceller` until the returned guard is dropped, or cancels immediately if already
    /// cancelled.
    pub fn register(&self, canceller: HostClientCanceller) -> Option<PollCancelGuard> {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        if self.0.cancelled.load(Ordering::Acquire) {
            drop(state);
            canceller.cancel();
            return None;
        }
        let id = state.next_id;
        state.next_id = state.next_id.wrapping_add(1);
        state.cancellers.push((id, canceller));
        Some(PollCancelGuard {
            inner: Arc::clone(&self.0),
            id,
        })
    }

    /// Registers a non-blocking wake callback invoked once if this scope is cancelled.
    fn register_wake(&self, wake: impl FnOnce() + Send + 'static) -> Option<PollCancelGuard> {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        if self.0.cancelled.load(Ordering::Acquire) {
            drop(state);
            wake();
            return None;
        }
        let id = state.next_id;
        state.next_id = state.next_id.wrapping_add(1);
        state.wakers.push((id, Box::new(wake)));
        Some(PollCancelGuard {
            inner: Arc::clone(&self.0),
            id,
        })
    }

    /// Marks this scope cancelled and aborts every currently registered [`HostClientCanceller`].
    ///
    /// Holds the registration mutex across `canceller.cancel()` so a finishing worker's
    /// [`PollCancelGuard::drop`] cannot return its client to the pool until cancellation finishes.
    /// Registered cancellers must be fast and non-blocking (never wait on the worker thread).
    pub fn cancel(&self) {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        self.0.cancelled.store(true, Ordering::Release);
        for (_, canceller) in state.cancellers.drain(..) {
            canceller.cancel();
        }
        for (_, wake) in state.wakers.drain(..) {
            wake();
        }
    }
}

/// Resolves whether `host` points at a site-local / private / link-local address.
///
/// IP literals are classified directly without network I/O; hostnames resolve via DNS.
pub fn is_site_local_host(host: &str) -> bool {
    let trimmed = host.trim();
    let unbracketed = trimmed
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(trimmed);
    let without_scope = match unbracketed.split_once('%') {
        Some((addr, _)) if addr.contains(':') => addr,
        _ => unbracketed,
    };
    if let Ok(ip) = without_scope.parse::<IpAddr>() {
        return is_site_local_ip(ip);
    }
    let Ok(addrs) = (trimmed, 0).to_socket_addrs() else {
        return false;
    };
    addrs.into_iter().any(|sa| is_site_local_ip(sa.ip()))
}

/// Polls a single candidate using a borrowed [`HostClient`] from `pool`.
pub fn try_poll_ip(
    pool: &HostClientPool,
    candidate: &ProbeCandidate,
    expected_uuid: Option<&str>,
    cert_der: Option<&[u8]>,
    likely_online: bool,
) -> Result<PollIpSuccess, PollIpError> {
    try_poll_ip_cancellable(pool, candidate, expected_uuid, cert_der, likely_online, None)
}

/// Cancellable variant of [`try_poll_ip`], registering the borrowed client with `cancel`.
pub fn try_poll_ip_cancellable(
    pool: &HostClientPool,
    candidate: &ProbeCandidate,
    expected_uuid: Option<&str>,
    cert_der: Option<&[u8]>,
    likely_online: bool,
    cancel: Option<&PollCancel>,
) -> Result<PollIpSuccess, PollIpError> {
    if cancel.is_some_and(PollCancel::is_cancelled) {
        return Err(PollIpError {
            candidate: candidate.clone(),
            kind: PollErrorKind::Cancelled,
            message: Some("cancelled".into()),
        });
    }
    let (client, pool_gen) = match pool.take(
        &candidate.address,
        candidate.port,
        candidate.https_port,
        cert_der,
    ) {
        Ok(taken) => taken,
        Err(err) => {
            return Err(PollIpError {
                candidate: candidate.clone(),
                kind: classify_http_error(&err),
                message: Some(err.to_string()),
            });
        }
    };
    let guard = if let Some(scope) = cancel {
        match scope.register(client.canceller()) {
            Some(g) => Some(g),
            None => {
                pool.give_back(&candidate.address, candidate.port, cert_der, pool_gen, client);
                return Err(PollIpError {
                    candidate: candidate.clone(),
                    kind: PollErrorKind::Cancelled,
                    message: Some("cancelled".into()),
                });
            }
        }
    } else {
        None
    };
    let result = try_poll_ip_with_client(&client, candidate, expected_uuid, cert_der.is_some(), likely_online);
    drop(guard);
    pool.give_back(&candidate.address, candidate.port, cert_der, pool_gen, client);
    result
}

/// Polls a single candidate on a provided [`HostClient`].
pub fn try_poll_ip_with_client(
    client: &HostClient,
    candidate: &ProbeCandidate,
    expected_uuid: Option<&str>,
    has_pinned_cert: bool,
    likely_online: bool,
) -> Result<PollIpSuccess, PollIpError> {
    match client.server_info_checked(likely_online) {
        Ok((info, path)) => {
            if let Some(expected) = expected_uuid
                && !expected.is_empty()
                && info.unique_id != expected
            {
                return Err(PollIpError {
                    candidate: candidate.clone(),
                    kind: PollErrorKind::UuidMismatch {
                        expected: expected.to_owned(),
                        actual: info.unique_id,
                    },
                    message: Some("Host uniqueid mismatch".into()),
                });
            }
            let should_clear_cert = has_pinned_cert
                && should_clear_cert_on_unpair(
                    info.paired,
                    matches!(path, InfoPath::Plain),
                    !matches!(path, InfoPath::CertificateMismatch),
                );
            Ok(PollIpSuccess {
                candidate: candidate.clone(),
                info,
                path,
                should_clear_cert,
            })
        }
        Err(err) => Err(PollIpError {
            candidate: candidate.clone(),
            kind: classify_http_error(&err),
            message: Some(err.to_string()),
        }),
    }
}

fn classify_http_error(err: &Error) -> PollErrorKind {
    match err {
        Error::CertificateMismatch => PollErrorKind::CertificateMismatch,
        Error::Cancelled => PollErrorKind::Cancelled,
        Error::Status { code, .. } => PollErrorKind::Http(*code as u16),
        Error::NotFound(_) => PollErrorKind::Http(404),
        Error::Malformed(_) => PollErrorKind::Malformed,
        _ => PollErrorKind::Unreachable,
    }
}

fn record_tried(tried: &mut Tried, candidate: &ProbeCandidate) {
    match candidate.slot {
        CandidateSlot::Local | CandidateSlot::Manual => tried.lan = true,
        CandidateSlot::Ipv6 => tried.ipv6 = true,
        CandidateSlot::Remote => {
            if is_tunnel_address(&candidate.address) {
                // Tunneled path is tracked as punched, not direct WAN.
            } else if candidate.address.contains(':') {
                tried.ipv6 = true;
            } else {
                tried.mapped = true;
            }
        }
        CandidateSlot::Pinned => {
            if candidate.address.contains(':') {
                tried.ipv6 = true;
            } else if is_site_local_host(&candidate.address) {
                tried.lan = true;
            } else {
                tried.mapped = true;
            }
        }
    }
}

/// Polls direct `candidates` concurrently while strictly preserving precedence order (`0..N`).
///
/// As soon as candidate `i` has succeeded and every higher-precedence candidate `< i` has
/// finished, all lower-precedence in-flight requests (`> i`) are cancelled via
/// [`HostClientCanceller::cancel`] and the winning response is returned immediately.
pub fn parallel_poll_direct(
    pool: &Arc<HostClientPool>,
    candidates: &[ProbeCandidate],
    expected_uuid: Option<&str>,
    cert_der: Option<&[u8]>,
    likely_online: bool,
) -> DirectPollOutcome {
    parallel_poll_direct_cancellable(pool, candidates, expected_uuid, cert_der, likely_online, None)
}

/// Cancellable variant of [`parallel_poll_direct`], aborting all in-flight candidate sockets when
/// `cancel` triggers.
pub fn parallel_poll_direct_cancellable(
    pool: &Arc<HostClientPool>,
    candidates: &[ProbeCandidate],
    expected_uuid: Option<&str>,
    cert_der: Option<&[u8]>,
    likely_online: bool,
    cancel: Option<&PollCancel>,
) -> DirectPollOutcome {
    let mut outcome = DirectPollOutcome::default();
    if candidates.is_empty() {
        return outcome;
    }
    for cand in candidates {
        record_tried(&mut outcome.tried, cand);
    }
    if candidates.len() == 1 {
        let cand = &candidates[0];
        match try_poll_ip_cancellable(pool, cand, expected_uuid, cert_der, likely_online, cancel) {
            Ok(win) => outcome.winner = Some(win),
            Err(err) => {
                outcome.another_certificate = matches!(err.kind, PollErrorKind::CertificateMismatch);
                outcome.last_error = Some(err.kind);
            }
        }
        return outcome;
    }

    let (tx, rx) = mpsc::channel::<Option<(usize, Result<PollIpSuccess, PollIpError>)>>();
    let _wake_guard = cancel.and_then(|scope| {
        let tx_wake = tx.clone();
        scope.register_wake(move || {
            let _ = tx_wake.send(None);
        })
    });
    let n = candidates.len();
    let mut results: Vec<Option<Result<PollIpSuccess, PollIpError>>> = vec![None; n];
    // Per-worker cancel scope synchronized with `pool.give_back` so a winner's cancellation
    // never fires after a loser's client has already been returned to and reused from the pool.
    let mut slot_cancels: Vec<Option<PollCancel>> = Vec::with_capacity(n);

    for (idx, cand) in candidates.iter().enumerate() {
        if cancel.is_some_and(PollCancel::is_cancelled) {
            slot_cancels.push(None);
            results[idx] = Some(Err(PollIpError {
                candidate: cand.clone(),
                kind: PollErrorKind::Cancelled,
                message: Some("cancelled".into()),
            }));
            continue;
        }
        let taken = pool.take(&cand.address, cand.port, cand.https_port, cert_der);
        match taken {
            Ok((client, pool_gen)) => {
                let guard = if let Some(scope) = cancel {
                    match scope.register(client.canceller()) {
                        Some(g) => Some(g),
                        None => {
                            pool.give_back(&cand.address, cand.port, cert_der, pool_gen, client);
                            slot_cancels.push(None);
                            results[idx] = Some(Err(PollIpError {
                                candidate: cand.clone(),
                                kind: PollErrorKind::Cancelled,
                                message: Some("cancelled".into()),
                            }));
                            continue;
                        }
                    }
                } else {
                    None
                };
                let slot_cancel = PollCancel::new();
                let slot_guard = slot_cancel.register(client.canceller());
                slot_cancels.push(Some(slot_cancel));
                let pool = Arc::clone(pool);
                let cand_bg = cand.clone();
                let expected = expected_uuid.map(ToOwned::to_owned);
                let cert = cert_der.map(ToOwned::to_owned);
                let tx = tx.clone();
                if let Err(err) = std::thread::Builder::new()
                    .name(format!("nj-poll-{idx}"))
                    .spawn(move || {
                        let _cancel_guard = guard;
                        let _slot_guard = slot_guard;
                        let res = try_poll_ip_with_client(
                            &client,
                            &cand_bg,
                            expected.as_deref(),
                            cert.is_some(),
                            likely_online,
                        );
                        drop(_slot_guard);
                        drop(_cancel_guard);
                        pool.give_back(&cand_bg.address, cand_bg.port, cert.as_deref(), pool_gen, client);
                        let _ = tx.send(Some((idx, res)));
                    })
                {
                    results[idx] = Some(Err(PollIpError {
                        candidate: cand.clone(),
                        kind: PollErrorKind::Unreachable,
                        message: Some(err.to_string()),
                    }));
                }
            }
            Err(err) => {
                slot_cancels.push(None);
                results[idx] = Some(Err(PollIpError {
                    candidate: cand.clone(),
                    kind: classify_http_error(&err),
                    message: Some(err.to_string()),
                }));
            }
        }
    }
    drop(tx);

    loop {
        let mut winner_idx = None;
        for (i, slot) in results.iter().enumerate() {
            match slot {
                None => break,
                Some(Ok(_)) => {
                    winner_idx = Some(i);
                    break;
                }
                Some(Err(err)) => {
                    if matches!(err.kind, PollErrorKind::CertificateMismatch) {
                        outcome.another_certificate = true;
                    }
                    outcome.last_error = Some(err.kind.clone());
                }
            }
        }
        if let Some(win_i) = winner_idx {
            for (j, cancel) in slot_cancels.iter().enumerate() {
                if j > win_i
                    && results[j].is_none()
                    && let Some(c) = cancel
                {
                    c.cancel();
                }
            }
            if let Some(Ok(win)) = results[win_i].take() {
                outcome.winner = Some(win);
            }
            return outcome;
        }
        if results.iter().all(Option::is_some) {
            return outcome;
        }
        if cancel.is_some_and(PollCancel::is_cancelled) {
            for (j, slot_cancel) in slot_cancels.iter().enumerate() {
                if results[j].is_none()
                    && let Some(c) = slot_cancel
                {
                    c.cancel();
                }
            }
            outcome.last_error = Some(PollErrorKind::Cancelled);
            return outcome;
        }
        match rx.recv() {
            Ok(Some((idx, res))) => results[idx] = Some(res),
            Ok(None) => {
                for (j, slot_cancel) in slot_cancels.iter().enumerate() {
                    if results[j].is_none()
                        && let Some(c) = slot_cancel
                    {
                        c.cancel();
                    }
                }
                outcome.last_error = Some(PollErrorKind::Cancelled);
                return outcome;
            }
            Err(_) => return outcome,
        }
    }
}

/// Failure details from [`poll_over_the_internet`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InternetPollFailure {
    pub reason: String,
    pub another_certificate: bool,
}

/// Probes WAN candidates from `internet_play_json` (IPv6 first, then UPnP/mapped IPv4) before
/// optionally gating only [`internet::punch`] on `allow_punch`, computing `unreachable` on failure.
#[allow(clippy::too_many_arguments)]
pub fn poll_over_the_internet(
    pool: &HostClientPool,
    uuid: &str,
    internet_play_json: &str,
    expected_uuid: Option<&str>,
    cert_der: Option<&[u8]>,
    direct_candidates: &[ProbeCandidate],
    tried: Tried,
    likely_online: bool,
    allow_punch: bool,
) -> Result<(SocketAddr, PollIpSuccess), InternetPollFailure> {
    poll_over_the_internet_cancellable(
        pool,
        uuid,
        internet_play_json,
        expected_uuid,
        cert_der,
        direct_candidates,
        tried,
        likely_online,
        allow_punch,
        None,
    )
}

/// Cancellable variant of [`poll_over_the_internet`].
#[allow(clippy::too_many_arguments)]
pub fn poll_over_the_internet_cancellable(
    pool: &HostClientPool,
    uuid: &str,
    internet_play_json: &str,
    expected_uuid: Option<&str>,
    cert_der: Option<&[u8]>,
    direct_candidates: &[ProbeCandidate],
    mut tried: Tried,
    likely_online: bool,
    allow_punch: bool,
    cancel: Option<&PollCancel>,
) -> Result<(SocketAddr, PollIpSuccess), InternetPollFailure> {
    let is_cancelled = || cancel.is_some_and(PollCancel::is_cancelled);
    if is_cancelled() {
        return Err(InternetPollFailure {
            reason: "cancelled".into(),
            another_certificate: false,
        });
    }
    let reply = InternetPlay::parse(internet_play_json).map_err(|e| InternetPollFailure {
        reason: e.to_string(),
        another_certificate: false,
    })?;
    let mut another_certificate = false;

    for ip in &reply.ipv6 {
        if is_cancelled() {
            return Err(InternetPollFailure {
                reason: "cancelled".into(),
                another_certificate,
            });
        }
        let addr_str = ip.to_string();
        tried.ipv6 = true;
        if direct_candidates
            .iter()
            .any(|d| d.address == addr_str && d.port == reply.ports.http)
        {
            continue;
        }
        let candidate = ProbeCandidate {
            slot: CandidateSlot::Ipv6,
            address: addr_str,
            port: reply.ports.http,
            https_port: reply.ports.https,
        };
        match try_poll_ip_cancellable(pool, &candidate, expected_uuid, cert_der, likely_online, cancel) {
            Ok(win) => return Ok((SocketAddr::new(IpAddr::V6(*ip), candidate.port), win)),
            Err(err) => {
                if matches!(err.kind, PollErrorKind::CertificateMismatch) {
                    another_certificate = true;
                }
            }
        }
    }

    if is_cancelled() {
        return Err(InternetPollFailure {
            reason: "cancelled".into(),
            another_certificate,
        });
    }
    if reply.mapped_ports
        && let Some(ip) = reply.external_ipv4
    {
        let addr_str = ip.to_string();
        tried.mapped = true;
        if !direct_candidates
            .iter()
            .any(|d| d.address == addr_str && d.port == reply.ports.http)
        {
            let candidate = ProbeCandidate {
                slot: CandidateSlot::Remote,
                address: addr_str,
                port: reply.ports.http,
                https_port: reply.ports.https,
            };
            match try_poll_ip_cancellable(pool, &candidate, expected_uuid, cert_der, likely_online, cancel) {
                Ok(win) => return Ok((SocketAddr::new(IpAddr::V4(ip), candidate.port), win)),
                Err(err) => {
                    if matches!(err.kind, PollErrorKind::CertificateMismatch) {
                        another_certificate = true;
                    }
                }
            }
        }
    }

    if is_cancelled() {
        return Err(InternetPollFailure {
            reason: "cancelled".into(),
            another_certificate,
        });
    }
    if !allow_punch {
        return Err(InternetPollFailure {
            reason: internet::unreachable(&reply, tried, None),
            another_certificate,
        });
    }

    let addr = internet::punch_cancellable(uuid, &reply, tried, is_cancelled).map_err(|reason| {
        InternetPollFailure {
            reason,
            another_certificate,
        }
    })?;
    let candidate = ProbeCandidate {
        slot: CandidateSlot::Remote,
        address: addr.ip().to_string(),
        port: addr.port(),
        https_port: reply.ports.https,
    };
    match try_poll_ip_cancellable(pool, &candidate, expected_uuid, cert_der, likely_online, cancel) {
        Ok(win) => Ok((addr, win)),
        Err(err) => {
            if matches!(err.kind, PollErrorKind::CertificateMismatch) {
                another_certificate = true;
            }
            let msg = err
                .message
                .unwrap_or_else(|| "internet poll failed".into());
            Err(InternetPollFailure {
                reason: internet::unreachable(&reply, tried, Some(msg)),
                another_certificate,
            })
        }
    }
}

/// Computes `"lan"`, `"ipv6"`, `"mapped"`, or `"punched"` for an active endpoint given
/// local/manual LAN endpoints and stored `/internet-play/v1` reply.
pub fn compute_internet_path(
    active: Option<&nightjar_policy::hosts::EndpointSlot>,
    local: Option<&nightjar_policy::hosts::EndpointSlot>,
    manual: Option<&nightjar_policy::hosts::EndpointSlot>,
    reply: Option<&InternetPlay>,
) -> Option<String> {
    let active_addr = active?.address.parse::<IpAddr>().ok()?;
    let lan: Vec<IpAddr> = local
        .into_iter()
        .chain(manual)
        .filter_map(|ep| ep.address.parse::<IpAddr>().ok())
        .collect();
    Some(internet::path_of(active_addr, &lan, reply).key().to_owned())
}

/// Fetches a fresh `/internet-play/v1` reply from the host over HTTPS.
pub fn refresh_internet_play(
    pool: &HostClientPool,
    address: &str,
    http_port: u16,
    https_port: u16,
    cert_der: &[u8],
    capabilities: &HostCapabilities,
) -> Result<String, HostApiError> {
    refresh_internet_play_cancellable(pool, address, http_port, https_port, cert_der, capabilities, None)
}

/// Cancellable variant of [`refresh_internet_play`].
pub fn refresh_internet_play_cancellable(
    pool: &HostClientPool,
    address: &str,
    http_port: u16,
    https_port: u16,
    cert_der: &[u8],
    capabilities: &HostCapabilities,
    cancel: Option<&PollCancel>,
) -> Result<String, HostApiError> {
    if cancel.is_some_and(PollCancel::is_cancelled) {
        return Err(HostApiError::Host(Error::Cancelled));
    }
    let (client, pool_gen) = pool.take(address, http_port, https_port, Some(cert_der))?;
    let guard = if let Some(scope) = cancel {
        match scope.register(client.canceller()) {
            Some(g) => Some(g),
            None => {
                pool.give_back(address, http_port, Some(cert_der), pool_gen, client);
                return Err(HostApiError::Host(Error::Cancelled));
            }
        }
    } else {
        None
    };
    let res = client.internet_play(capabilities).map(|(raw, _)| raw);
    drop(guard);
    pool.give_back(address, http_port, Some(cert_der), pool_gen, client);
    res
}

/// Parses stored `internet_play` JSON cleanly.
pub fn parse_internet_reply(raw: Option<&str>) -> Option<InternetPlay> {
    raw.and_then(|text| InternetPlay::parse(text).ok())
}

/// Builds the ordered poll candidate addresses for a Desktop host entry.
///
/// Pinned addresses poll alone; otherwise active + other non-remote addresses run before
/// IPv6, stored remote, and mapped IPv4 internet candidates.
pub fn desktop_poll_addresses(
    shared: &SharedHostFields,
    reply: Option<&InternetPlay>,
) -> Vec<String> {
    if let Some(pinned) = &shared.pinned_address {
        return vec![pinned.clone()];
    }
    let remote = shared.remote_address.as_ref();
    let mut addresses: Vec<String> = std::iter::once(&shared.address)
        .chain(&shared.other_addresses)
        .filter(|a| !a.is_empty() && Some(*a) != remote)
        .cloned()
        .collect();

    let mut away: Vec<String> = Vec::new();
    if let Some(reply) = reply {
        away.extend(reply.ipv6.iter().map(ToString::to_string));
    }
    away.extend(remote.cloned());
    if let Some(mapped) = reply
        .filter(|r| r.mapped_ports)
        .and_then(|r| r.external_ipv4)
    {
        away.push(mapped.to_string());
    }
    for address in away {
        if !address.is_empty() && !addresses.contains(&address) {
            addresses.push(address);
        }
    }
    addresses
}

/// Whether `address` reaches a Desktop host from outside its home network.
pub fn away_address(
    shared: &SharedHostFields,
    reply: Option<&InternetPlay>,
    address: &str,
) -> bool {
    if shared.remote_address.as_deref() == Some(address) {
        return true;
    }
    let Ok(ip) = address.parse::<IpAddr>() else {
        return false;
    };
    if internet::is_tunnel(ip) {
        return true;
    }
    reply.is_some_and(|r| match ip {
        IpAddr::V6(ip6) => r.ipv6.contains(&ip6),
        IpAddr::V4(ip4) => r.external_ipv4 == Some(ip4),
    })
}
