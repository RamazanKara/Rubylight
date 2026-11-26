//! `/pair` and `/unpair`: Moonlight's pairing handshake, one device at a time.

use super::*;

pub(super) async fn pair(
    State(h): State<Shared>,
    Extension(connection): Extension<Connection>,
    Query(args): Query<Args>,
) -> Response {
    match do_pair(h, &args, connection.peer.ip().to_canonical()).await {
        Ok(fields) => xml(
            200,
            &fields
                .iter()
                .map(|(k, v)| (k.as_str(), v.clone()))
                .collect::<Vec<_>>(),
            None,
        ),
        Err(e) => xml(400, &[("paired", "0".into())], Some(e.to_string())),
    }
}
/// Whether a pairing request has to wait for another device's: Moonlight's
/// apps share one unique ID, so only the certificate tells devices apart. The
/// same device asking again replaces its own request, as after a wrong PIN.
pub(super) fn another_device_pairing(existing: Option<(&str, bool)>, certificate: &str) -> bool {
    existing.is_some_and(|(theirs, active)| active && theirs != certificate)
}
/// How often one peer (an IPv4 address or an IPv6 /64) raises the tray's pairing notice. The
/// console lists every request; a device asking again and again doesn't flood the desktop.
const NOTICE_EVERY: Duration = Duration::from_secs(30);
static NOTICES: std::sync::Mutex<std::collections::BTreeMap<std::net::IpAddr, Instant>> =
    std::sync::Mutex::new(std::collections::BTreeMap::new());

/// Whether `peer` may raise the tray's pairing notice at `now`, and if so, starts its wait. Only a
/// notice that shows starts the wait, so a peer asking every 20 seconds is told every 30.
pub(super) fn may_notify(
    notices: &mut std::collections::BTreeMap<std::net::IpAddr, Instant>,
    peer: std::net::IpAddr,
    now: Instant,
) -> bool {
    notices.retain(|_, at| now.saturating_duration_since(*at) < NOTICE_EVERY);
    let group = crate::network::peer_group(peer.to_canonical());
    if notices.contains_key(&group) {
        return false;
    }
    notices.insert(group, now);
    true
}

/// Whether two peers count as one for the pairing limits: the same IPv4 address, or the same IPv6
/// /64, whose privacy addresses one device rotates through.
fn same_peer(a: std::net::IpAddr, b: std::net::IpAddr) -> bool {
    let group = |ip: std::net::IpAddr| crate::network::peer_group(ip.to_canonical());
    group(a) == group(b)
}

pub fn pairing_id(id: &str, peer: std::net::IpAddr) -> String {
    format!(
        "{}_{}",
        hex::encode(crypto::hash(id.as_bytes())),
        hex::encode(crypto::hash(peer.to_canonical().to_string().as_bytes()))
    )
}
pub async fn do_pair(
    h: Shared,
    args: &Args,
    peer: std::net::IpAddr,
) -> Result<Vec<(String, String)>> {
    let id = args.get("uniqueid").context("missing uniqueid")?.clone();
    if id.is_empty()
        || id.len() > 256
        || !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
    {
        bail!("invalid uniqueid");
    }
    // Moonlight shares wire IDs across devices; one LAN peer must not reserve
    // that ID for every other device waiting to pair.
    let id = pairing_id(&id, peer);
    if !h.config.read().unwrap().boolean("enable_pairing", true) {
        bail!("pairing is disabled");
    }
    if args.get("phrase").is_some_and(|s| s == "getservercert") {
        let text = args
            .get("clientcert")
            .context("missing client certificate")?;
        if text.len() > 32768 {
            bail!("certificate too large");
        }
        let certificate = String::from_utf8(hex::decode(text)?)?;
        crypto::verify_pairing_certificate(&certificate)?;
        let salt = hex::decode(args.get("salt").context("missing salt")?)?;
        if salt.len() < 16 || salt.len() > 32 {
            bail!("invalid salt length");
        }
        let name = args
            .get("devicename")
            .cloned()
            .unwrap_or("Moonlight Client".into());
        butterpollo_crypto::pairing::validate_device_name(&name)?;
        // Another device's request still waiting for its PIN, or in the
        // seconds of its handshake, is not replaced: the PIN typed for it
        // would reach this device instead.
        let waiting = h.pins.lock().unwrap().get(&id).map(|p| {
            (
                p.certificate.clone(),
                p.created.elapsed() < Duration::from_secs(300) && !p.sender.is_closed(),
            )
        });
        let handshaking = h.pairings.lock().unwrap().sessions.get(&id).map(|p| {
            (
                p.certificate.clone(),
                p.created.elapsed() < Duration::from_secs(30),
            )
        });
        if another_device_pairing(
            waiting.as_ref().map(|(c, a)| (c.as_str(), *a)),
            &certificate,
        ) || another_device_pairing(
            handshaking.as_ref().map(|(c, a)| (c.as_str(), *a)),
            &certificate,
        ) {
            bail!("another device is pairing with this PC; finish or cancel that first");
        }
        // A new certificate request replaces this device's unfinished
        // pairing, as in the previous host. Moonlight abandons a pairing after
        // a wrong PIN without telling the host, and its apps share one unique
        // ID, so the stale session refused every later attempt until restart.
        h.pairings.lock().unwrap().sessions.remove(&id);
        crate::pairing_approvals::failed(&h, &id);
        if h.pairings
            .lock()
            .unwrap()
            .sessions
            .values()
            .filter(|p| {
                p.peer.is_some_and(|theirs| same_peer(theirs, peer))
                    && p.created.elapsed() < Duration::from_secs(300)
            })
            .count()
            >= 2
        {
            bail!("pairing request limit reached for this address");
        }
        if let Some(auth) = args.get("otpauth") {
            let salt_text = args.get("salt").map_or("", String::as_str);
            // An invite's PIN (`crate::invites`): 128 bits derived from the link's secret. It
            // is spent only when the pairing completes, so a dropped handshake can try again.
            if let Some((invite, pin)) = crate::invites::matching(&h, auth, salt_text) {
                tracing::info!(client=%name, invite, %peer, "Pairing from an invite");
                let mut pair = Pairing::for_invite(id, name, certificate, &salt, &pin, invite)?;
                pair.peer = Some(peer);
                h.pairings.lock().unwrap().insert(pair)?;
                return Ok(vec![
                    ("paired".into(), "1".into()),
                    (
                        "plaincert".into(),
                        hex::encode(h.identity.certificate.as_bytes()),
                    ),
                ]);
            }
            // One-time PIN pairing (Artemis). A wrong hash still gets an
            // ordinary answer, with a random PIN that fails the next step.
            // The QR code's PIN and the one typed in Artemis are tried both.
            let matched = h.otp.lock().unwrap().take_matching(auth, salt_text, peer)?;
            let (pin, name, permission) = if let Some(otp) = matched {
                tracing::info!(client=%name, "Pairing with a one-time PIN");
                let name = if otp.device_name.is_empty() {
                    name
                } else {
                    otp.device_name
                };
                (otp.pin, name, otp.permission)
            } else {
                tracing::warn!(client=%name, "One-time PIN pairing failed");
                (
                    format!(
                        "{:04}",
                        rand::Rng::gen_range(&mut rand::thread_rng(), 0..10_000u16)
                    ),
                    name,
                    None,
                )
            };
            let mut pair = Pairing::new(id, name, certificate, &salt, &pin)?;
            pair.peer = Some(peer);
            pair.permission = permission;
            h.pairings.lock().unwrap().insert(pair)?;
            return Ok(vec![
                ("paired".into(), "1".into()),
                (
                    "plaincert".into(),
                    hex::encode(h.identity.certificate.as_bytes()),
                ),
            ]);
        }
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let created = Instant::now();
        {
            let mut pins = h.pins.lock().unwrap();
            pins.retain(|_, p| p.created.elapsed() < Duration::from_secs(300));
            // Replacing an older request ends its wait for the PIN.
            pins.remove(&id);
            if pins.len() >= 32 {
                bail!("pairing request limit reached");
            }
            if pins
                .values()
                .filter(|pending| same_peer(pending.address, peer))
                .count()
                >= 2
            {
                bail!("pairing request limit reached for this address");
            }
            pins.insert(
                id.clone(),
                PendingPin {
                    name: name.clone(),
                    certificate: certificate.clone(),
                    address: peer,
                    created,
                    sender,
                },
            );
        }
        struct PinGuard {
            h: Shared,
            id: String,
            created: Instant,
        }
        impl Drop for PinGuard {
            fn drop(&mut self) {
                // Only this request's entry: a newer request may have replaced it.
                let mut pins = self.h.pins.lock().unwrap_or_else(|e| e.into_inner());
                if pins.get(&self.id).is_none_or(|p| p.created == self.created) {
                    pins.remove(&self.id);
                    butterpollo_win_io::tray::pairing_request_ended(&self.id);
                }
            }
        }
        let guard = PinGuard {
            h: h.clone(),
            id: id.clone(),
            created,
        };
        tracing::info!(client=%name,%peer,"Pairing PIN required in the web interface");
        if may_notify(&mut NOTICES.lock().unwrap(), peer, Instant::now()) {
            let (title, text) = crate::state::pairing_notice(&name, peer);
            butterpollo_win_io::tray::notify_pairing(&id, &title, &text);
        }
        let response = tokio::time::timeout(Duration::from_secs(300), receiver).await;
        drop(guard);
        let (pin, name, permission, approver) = response
            .context("PIN entry timed out")?
            .context("pairing cancelled")?;
        let mut pair = Pairing::new(id, name, certificate, &salt, &pin)?;
        pair.peer = Some(peer);
        pair.permission = permission;
        pair.approver = approver;
        h.pairings.lock().unwrap().insert(pair)?;
        return Ok(vec![
            ("paired".into(), "1".into()),
            (
                "plaincert".into(),
                hex::encode(h.identity.certificate.as_bytes()),
            ),
        ]);
    }
    if args.get("phrase").is_some_and(|s| s == "pairchallenge") {
        return Ok(vec![("paired".into(), "1".into())]);
    }
    // A malformed step is refused before the pairing is touched: from any
    // other device it must not end someone else's pairing.
    enum Step {
        Challenge(Vec<u8>),
        Response(Vec<u8>),
        Secret(Vec<u8>),
    }
    let step = if let Some(v) = args.get("clientchallenge") {
        if v.len() != 32 {
            bail!("invalid challenge length");
        }
        Step::Challenge(hex::decode(v)?)
    } else if let Some(v) = args.get("serverchallengeresp") {
        if v.len() != 64 {
            bail!("invalid challenge response length");
        }
        Step::Response(hex::decode(v)?)
    } else if let Some(v) = args.get("clientpairingsecret") {
        if v.len() != 544 {
            bail!("invalid pairing secret length");
        }
        Step::Secret(hex::decode(v)?)
    } else {
        bail!("unknown pairing phase")
    };
    let mut pairings = h.pairings.lock().unwrap();
    pairings.expire();
    let p = pairings
        .sessions
        .get_mut(&id)
        .context("no pending pairing")?;
    if p.peer.is_some_and(|started| started != peer) {
        bail!("this pairing was started from another address");
    }
    let result = (|| -> Result<Vec<(String, String)>> {
        match &step {
            Step::Challenge(challenge) => {
                let response = p.client_challenge(&h.identity, challenge)?;
                Ok(vec![
                    ("paired".into(), "1".into()),
                    ("challengeresponse".into(), hex::encode(response)),
                ])
            }
            Step::Response(response) => {
                let response = p.server_response(&h.identity, response)?;
                Ok(vec![
                    ("paired".into(), "1".into()),
                    ("pairingsecret".into(), hex::encode(response)),
                ])
            }
            Step::Secret(secret) if p.invite.is_some() => {
                p.finish(secret)?;
                finish_invite(&h, p, peer)
            }
            Step::Secret(secret) => {
                p.finish(secret)?;
                let mut state = h.paired.write().unwrap();
                let perm = state.paired_device_permission(p.permission);
                let extra = state.paired_device_extra(perm, p.permission);
                let crate::pairing_approvals::Granted {
                    perm,
                    extra,
                    approver,
                } = crate::pairing_approvals::capped(&state, p.approver.as_deref(), perm, extra)?;
                let uuid = uuid::Uuid::new_v4().to_string();
                state
                    .add(
                        &h.paired_path,
                        Client {
                            name: p.name.clone(),
                            cert: p.certificate.clone(),
                            uuid: uuid.clone(),
                            perm,
                            enabled: true,
                            extra,
                        },
                    )
                    .map_err(|error| {
                        // Moonlight hears only the generic words below.
                        tracing::error!(error = %format!("{error:#}"), "a Moonlight device could not be saved");
                        error.context(crate::pairing_approvals::NotSaved)
                    })?;
                drop(state);
                tracing::info!(client=%p.name,"Client paired and saved");
                let mut paired = serde_json::json!({"device": crate::webhooks::device(&uuid, &p.name), "transport": "moonlight"});
                if let Some(approver) = &approver {
                    crate::pairing_approvals::paired(&h, &id, approver, &p.name, peer);
                    paired["approved_by"] = crate::webhooks::device(&approver.uuid, &approver.name);
                } else {
                    crate::pairing_approvals::paired_by_owner(&h, peer);
                }
                h.webhook("pairing.paired", paired);
                let (title, text) = match &approver {
                    Some(approver) => {
                        crate::state::pairing_approved_notice(&approver.name, &p.name)
                    }
                    None => (
                        "Device paired".to_owned(),
                        format!("{} can now stream from this PC.", p.name),
                    ),
                };
                butterpollo_win_io::tray::notify(&title, &text);
                Ok(vec![("paired".into(), "1".into())])
            }
        }
    })();
    if result.is_err() || matches!(step, Step::Secret(_)) {
        pairings.sessions.remove(&id);
    }
    if let Err(error) = &result {
        if let Some(revoked) = error.downcast_ref::<crate::pairing_approvals::ApproverRevoked>() {
            crate::pairing_approvals::approver_revoked(&h, &id, revoked.missing);
        } else if error.is::<crate::pairing_approvals::NotSaved>() {
            crate::pairing_approvals::not_saved(&h, &id);
        } else {
            // Any other step that fails still counts, malformed ones too: the PIN can
            // be worked out from an earlier reply, so a device must not choose to fail
            // in a way that costs it nothing.
            crate::pairing_approvals::failed(&h, &id);
        }
    }
    result
}

/// The end of a pairing that answered an invite: the invite is spent, and the device is paired as
/// an invited guest (`butterpollo_session::invites`), never with a first device's rights. A device
/// already paired on its own keeps its rights.
fn finish_invite(
    h: &Shared,
    p: &butterpollo_crypto::pairing::Pairing,
    peer: std::net::IpAddr,
) -> Result<Vec<(String, String)>> {
    let invite = p.invite.context("not an invite's pairing")?;
    let mut state = h.paired.write().unwrap();
    let same = |c: &&Client| c.cert.trim() == p.certificate.trim();
    let existing = state.clients.iter().find(same).map(|c| c.uuid.clone());
    let uuid = existing
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let (perm, guest) = crate::invites::entitle(h, &state, invite)?;
    let before = state.clone();
    let mut extra = std::collections::BTreeMap::new();
    extra.insert(
        butterpollo_session::invites::MARKER.to_owned(),
        guest.marker(),
    );
    state
        .add(
            &h.paired_path,
            Client {
                name: p.name.clone(),
                cert: p.certificate.clone(),
                uuid: uuid.clone(),
                perm,
                enabled: true,
                extra,
            },
        )
        .map_err(|error| {
            tracing::error!(error = %format!("{error:#}"), "an invited device could not be saved");
            error.context(crate::pairing_approvals::NotSaved)
        })?;
    if existing.is_some() {
        let mut next = state.clone();
        if let Some(client) = next.clients.iter_mut().find(|c| c.uuid == uuid)
            && crate::invites::apply(client, perm, guest)
        {
            next.save(&h.paired_path)?;
            *state = next;
        }
    }
    // Spent only now: a device that could not be saved leaves the link usable. `paired` is still
    // held, so no other pairing took it meanwhile; one taken back or ended in between pairs
    // nothing.
    if let Err(error) = crate::invites::spend(h, invite, &uuid, &p.name) {
        // In memory whatever the disk says: a save that fails here must not leave the device
        // paired for a link that was taken back.
        let saved = before.save(&h.paired_path);
        *state = before;
        if let Err(save_error) = saved {
            tracing::error!(error = %format!("{save_error:#}"), "an unspent invite's device could not be removed from the saved list");
        }
        return Err(error);
    }
    drop(state);
    tracing::info!(client=%p.name, invite, %peer, until = guest.until, "device paired from an invite");
    crate::internet_play::refresh(h);
    h.webhook(
        "pairing.paired",
        serde_json::json!({"device": crate::webhooks::device(&uuid, &p.name), "transport": "invite"}),
    );
    butterpollo_win_io::tray::notify(
        "Your invite was used",
        &format!("{} can join the game you play when you let it in.", p.name),
    );
    Ok(vec![("paired".into(), "1".into())])
}

pub(super) async fn unpair(
    State(h): State<Shared>,
    Extension(connection): Extension<Connection>,
) -> Response {
    // Only a paired client's own certificate can unpair it; anything else is
    // answered like Vibepollo, with nothing removed. That goes for an approval
    // in flight too: Moonlight sends this after a wrong PIN, but without a
    // certificate, so it proves nothing about who is asking.
    let Ok(client) = authenticated(&h, &connection, 0) else {
        return xml(200, &[("unpaired", "0".into())], None);
    };
    // From either of its screens, the device goes, with both screens' streams.
    let device = butterpollo_session::state::device_uuid(&client.uuid);
    let removed = h.unpair_with_network_history(Some(device));
    if let Err(e) = removed {
        return xml(500, &[("unpaired", "0".into())], Some(e.to_string()));
    }
    end_device(&h, device);
    release_device(&h, device);
    xml(200, &[("unpaired", "1".into())], None)
}

#[cfg(test)]
mod notice_tests {
    use super::*;

    #[test]
    fn one_peer_raises_one_tray_notice_per_30_seconds_across_its_ipv6_prefix() {
        let mut notices = std::collections::BTreeMap::new();
        let start = Instant::now();
        let ip = |text: &str| text.parse::<std::net::IpAddr>().unwrap();
        assert!(may_notify(&mut notices, ip("fd00:1:2:3::1"), start));
        // A rotated privacy address of the same /64, and the same address again, wait.
        let soon = start + Duration::from_secs(5);
        assert!(!may_notify(&mut notices, ip("fd00:1:2:3:aaaa::9"), soon));
        assert!(!may_notify(&mut notices, ip("fd00:1:2:3::1"), soon));
        // Another prefix and an IPv4 peer have their own wait, however the address is written.
        assert!(may_notify(&mut notices, ip("fd00:1:2:4::1"), soon));
        assert!(may_notify(&mut notices, ip("::ffff:192.168.4.9"), soon));
        assert!(!may_notify(&mut notices, ip("192.168.4.9"), soon));
        assert!(may_notify(
            &mut notices,
            ip("fd00:1:2:3::2"),
            start + NOTICE_EVERY
        ));
    }

    #[test]
    fn the_pairing_limits_count_an_ipv6_prefix_as_one_peer() {
        let ip = |text: &str| text.parse::<std::net::IpAddr>().unwrap();
        assert!(same_peer(ip("fd00:1:2:3::1"), ip("fd00:1:2:3:ffff::2")));
        assert!(!same_peer(ip("fd00:1:2:3::1"), ip("fd00:1:2:4::1")));
        assert!(same_peer(ip("::ffff:10.0.0.2"), ip("10.0.0.2")));
        assert!(!same_peer(ip("10.0.0.2"), ip("10.0.0.3")));
    }
}
