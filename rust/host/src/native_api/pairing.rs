//! Approve or deny another device's pairing request from a paired device
//! (`docs/native-api.md`, "Approve a device"). The PIN goes down the console's
//! path: the waiting request's `sender`, read by `nvhttp::do_pair` or the
//! WebRTC signalling. Moonlight's handshake derives its key from the PIN and
//! carries no check of it, so the host learns a PIN was wrong only from how the
//! new device's handshake ends, and that attempt is then over: the device starts
//! again with a new PIN. Wrong PINs are counted per address for that reason, as
//! the device chooses its name. Moonlight's `/unpair` after a wrong PIN comes
//! without a certificate, so anyone at the address could send it: it counts for
//! nothing here, and the approval ends when the wait does.
use super::{Failure, Reply, caller, json, permission_required, read_body};
use crate::{
    state::{Host, Shared},
    tls::Connection,
};
use axum::{
    Extension,
    extract::{Request, State},
    http::StatusCode,
};
use butterpollo_core::{
    native_api::{
        error::ErrorCode,
        pairing::{PairingApprove, PairingApproved, PairingDeny, PairingRequest, PairingRequests},
    },
    nucleus::coop,
    state::{
        Client, LATER_DEVICE_PERMISSION, LAUNCH_APPS, PAIRING_APPROVE_KEY, PLAY_DEVICE_PERMISSION,
        PairedState,
    },
};
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashMap},
    net::IpAddr,
    sync::PoisonError,
    time::{Duration, Instant},
};
use tokio::sync::oneshot;

/// How long an approval waits for the new device to finish. Below the 15 s a
/// client waits for an action's answer.
#[cfg(not(test))]
const HANDSHAKE_WAIT: Duration = Duration::from_secs(10);
#[cfg(test)]
const HANDSHAKE_WAIT: Duration = Duration::from_secs(2);
/// As long as a waiting request and a started handshake live.
const REQUEST_LIFETIME: Duration = Duration::from_secs(300);
/// Wrong PINs that end an address's requests on this path.
const WRONG_PIN_LIMIT: u8 = 3;
/// How long those wrong PINs are remembered after the last one.
const WRONG_PIN_MEMORY: Duration = Duration::from_secs(300);
/// How long an address waits for its next approval after a wrong PIN. A real
/// device takes about this long to be asked again and show a new PIN.
const WRONG_PIN_COOLDOWN: Duration = Duration::from_secs(30);
/// A little past the handshake's own limit, so a late finish is still capped.
const APPROVAL_LIFETIME: Duration = Duration::from_secs(310);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Outcome {
    Paired,
    WrongPin,
    /// The approver lost what this takes, named here. Not the new device's doing: no strike.
    ApproverRevoked(&'static str),
    /// The PIN was right but the host could not save the device: no strike either.
    NotSaved,
}
/// What an approver needs to hand on anything of Play.
pub(crate) const LIST_AND_VIEW: &str = "List applications and View streams";
/// Why [`capped`] refused: the device that approved lost `missing`.
#[derive(Debug)]
pub(crate) struct ApproverRevoked {
    pub missing: &'static str,
}
impl std::fmt::Display for ApproverRevoked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "the device that approved this pairing no longer holds {}",
            self.missing
        )
    }
}
impl std::error::Error for ApproverRevoked {}
/// A handshake that was right, whose device could not be saved.
#[derive(Debug)]
pub(crate) struct NotSaved;
impl std::fmt::Display for NotSaved {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the paired device could not be saved")
    }
}
impl std::error::Error for NotSaved {}
/// How an approval reports back, with what its log lines name. Who approved
/// lives on the handshake's session ([`PinEntry`](crate::state::PinEntry)), so
/// losing this entry loses the answer and never the cap.
struct Approval {
    approver_name: String,
    device: String,
    address: IpAddr,
    created: Instant,
    /// Taken when the wait gives up; a late finish still pairs under the cap.
    outcome: Option<oneshot::Sender<Outcome>>,
}
/// The device that approved a pairing, for the notice and the webhook.
pub(crate) struct Approver {
    pub uuid: String,
    pub name: String,
}
/// What the wrong PINs count against: the address, an IPv6 /64 as one. The name
/// is the device's to choose, so it can't be part of it.
fn counted_as(address: IpAddr) -> IpAddr {
    crate::webrtc::clients::peer_group(address.to_canonical())
}
/// The approvals whose new device has not finished yet, by request id, and the
/// wrong PINs per address. A retry has a new request, so the count goes by the
/// address it keeps.
#[derive(Default)]
pub struct Approvals {
    running: HashMap<String, Approval>,
    wrong_pins: HashMap<IpAddr, (u8, Instant)>,
}
impl Approvals {
    fn prune(&mut self) {
        self.running
            .retain(|_, approval| approval.created.elapsed() < APPROVAL_LIFETIME);
        self.wrong_pins
            .retain(|_, (_, last)| last.elapsed() < WRONG_PIN_MEMORY);
    }
    fn ended(&mut self, address: IpAddr) -> bool {
        self.prune();
        self.wrong_pins
            .get(&counted_as(address))
            .is_some_and(|(count, _)| *count >= WRONG_PIN_LIMIT)
    }
    /// The seconds left before this address may be approved again after a wrong
    /// PIN. A PIN typed for a look-alike can be worked out from its handshake,
    /// and the real device keeps showing it; the same PIN typed again at once
    /// must not reach the look-alike's next request.
    fn cooling(&mut self, address: IpAddr) -> Option<u64> {
        self.prune();
        let (_, last) = self.wrong_pins.get(&counted_as(address))?;
        let left = WRONG_PIN_COOLDOWN.checked_sub(last.elapsed())?;
        Some(left.as_secs() + 1)
    }
    /// Counts a wrong PIN. True when this one ended the address's requests.
    fn strike(&mut self, address: IpAddr) -> bool {
        let entry = self
            .wrong_pins
            .entry(counted_as(address))
            .or_insert((0, Instant::now()));
        *entry = (entry.0.saturating_add(1), Instant::now());
        entry.0 == WRONG_PIN_LIMIT
    }
}
#[cfg(test)]
impl Approvals {
    pub(crate) fn is_idle(&self) -> bool {
        self.running.is_empty()
    }
    pub(crate) fn wrong_pins_at(&self, address: &str) -> u8 {
        self.wrong_pins
            .get(&counted_as(address.parse().unwrap()))
            .map_or(0, |(count, _)| *count)
    }
    pub(crate) fn strike_at(&mut self, address: IpAddr) {
        self.strike(address);
    }
    /// The cooldown has passed, without waiting for it.
    pub(crate) fn cool_down(&mut self) {
        for (_, last) in self.wrong_pins.values_mut() {
            *last -= WRONG_PIN_COOLDOWN;
        }
    }
}
fn approvals(h: &Host) -> std::sync::MutexGuard<'_, Approvals> {
    h.approvals.lock().unwrap_or_else(PoisonError::into_inner)
}

fn loud_strike(h: &Host, address: IpAddr, device: &str) {
    if !approvals(h).strike(address) {
        return;
    }
    tracing::warn!(client = %device, %address, "Three wrong PINs from other devices: this address can now only be paired in the console");
    let (title, text) = crate::state::pairing_ended_notice(address);
    butterpollo_windows::tray::notify(&title, &text);
}

/// What a device gets for pairing, and who approved it.
pub(crate) struct Granted {
    pub perm: u32,
    pub extra: BTreeMap<String, Value>,
    pub approver: Option<Approver>,
}
/// What a device gets when `approver` (a uuid, from its session) typed its PIN:
/// never more than the approver holds now, and nothing if it may no longer
/// approve. Without an approver, as from the console, it keeps `perm` and `extra`.
pub(crate) fn capped(
    state: &PairedState,
    approver: Option<&str>,
    perm: u32,
    mut extra: BTreeMap<String, Value>,
) -> anyhow::Result<Granted> {
    let Some(uuid) = approver else {
        return Ok(Granted {
            perm,
            extra,
            approver: None,
        });
    };
    let approver = state
        .clients
        .iter()
        .find(|c| c.uuid == uuid && c.may_approve_pairing())
        .ok_or(ApproverRevoked {
            missing: "Approve new devices",
        })?;
    let perm = perm & approver.perm;
    // The approver's bits may have gone during the handshake: no dead device.
    if perm == 0 {
        return Err(ApproverRevoked {
            missing: LIST_AND_VIEW,
        }
        .into());
    }
    // Only the console hands this one on.
    extra.remove(PAIRING_APPROVE_KEY);
    // The default follows Launch apps, so it takes an explicit false.
    if perm & LAUNCH_APPS != 0 && !approver.allows_coop_join() {
        extra.insert(coop::DEVICE_KEY.into(), false.into());
    }
    Ok(Granted {
        perm,
        extra,
        approver: Some(Approver {
            uuid: approver.uuid.clone(),
            name: approver.name.clone(),
        }),
    })
}

/// A device finished pairing with an approver, who hears so if still waiting.
pub(crate) fn paired(h: &Host, id: &str, approver: &Approver, device: &str, address: IpAddr) {
    let address = address.to_canonical();
    let mut approvals = approvals(h);
    let running = approvals.running.remove(id);
    approvals.wrong_pins.remove(&counted_as(address));
    drop(approvals);
    if let Some(outcome) = running.and_then(|mut approval| approval.outcome.take()) {
        let _ = outcome.send(Outcome::Paired);
    }
    tracing::warn!(approver = %approver.name, client = %device, %address, "Device paired, approved from another device");
}

/// The owner paired a device at `address` without an approver, so its wrong PINs
/// no longer say anything: a home network isn't left waiting behind them.
pub(crate) fn paired_by_owner(h: &Host, address: IpAddr) {
    approvals(h)
        .wrong_pins
        .remove(&counted_as(address.to_canonical()));
}

/// The handshake of request `id` ended without a pairing. Does nothing to a
/// request no device approved, or one whose approval already gave up waiting.
pub(crate) fn failed(h: &Host, id: &str) {
    let Some(mut approval) = approvals(h).running.remove(id) else {
        return;
    };
    let Some(outcome) = approval.outcome.take() else {
        return;
    };
    tracing::warn!(approver = %approval.approver_name, client = %approval.device, address = %approval.address, "Pairing approval failed: the device did not prove it knows the PIN");
    loud_strike(h, approval.address, &approval.device);
    let _ = outcome.send(Outcome::WrongPin);
}

/// The handshake of request `id` was right, but [`capped`] refused it. The
/// approver hears why, and the device, which did nothing wrong, takes no strike.
pub(crate) fn approver_revoked(h: &Host, id: &str, missing: &'static str) {
    let Some(mut approval) = approvals(h).running.remove(id) else {
        tracing::warn!(
            missing,
            "Pairing refused: the device that approved it lost this"
        );
        return;
    };
    tracing::warn!(approver = %approval.approver_name, client = %approval.device, address = %approval.address, missing, "Pairing refused: the device that approved it lost this");
    if let Some(outcome) = approval.outcome.take() {
        let _ = outcome.send(Outcome::ApproverRevoked(missing));
    }
}

/// The handshake of request `id` was right, but its device could not be saved.
/// The save's own error is logged by the caller; the device takes no strike.
pub(crate) fn not_saved(h: &Host, id: &str) {
    let Some(mut approval) = approvals(h).running.remove(id) else {
        return;
    };
    tracing::warn!(approver = %approval.approver_name, client = %approval.device, address = %approval.address, "Pairing approval: the device could not be saved");
    if let Some(outcome) = approval.outcome.take() {
        let _ = outcome.send(Outcome::NotSaved);
    }
}

/// A request that can still be answered.
fn live(request: &crate::state::PendingPin) -> bool {
    request.created.elapsed() < REQUEST_LIFETIME && !request.sender.is_closed()
}
fn no_request() -> Failure {
    Failure::new(
        ErrorCode::NotFound,
        "This pairing request ended. Start pairing again on the new device.",
    )
}
fn invalid(error: serde_json::Error) -> Failure {
    Failure::new(ErrorCode::InvalidRequest, format!("Invalid body: {error}."))
}

/// Which requests the caller is offered, oldest first.
fn waiting(h: &Host) -> PairingRequests {
    let pins = h.pins.lock().unwrap_or_else(PoisonError::into_inner);
    let mut approvals = approvals(h);
    let mut requests: Vec<_> = pins
        .iter()
        .filter(|(_, p)| live(p) && !approvals.ended(p.address))
        .map(|(id, p)| PairingRequest {
            id: id.clone(),
            name: p.name.clone(),
            address: p.address.to_canonical().to_string(),
            age_seconds: p.created.elapsed().as_secs(),
        })
        .collect();
    requests.sort_by_key(|r| std::cmp::Reverse(r.age_seconds));
    PairingRequests { requests }
}

/// Host control or "Approve new devices". The refusal stays `permission_required`:
/// `host_control_required` would name a wider grant than this route needs.
fn require_approver(client: &Client) -> Result<(), Failure> {
    client
        .may_approve_pairing()
        .then_some(())
        .ok_or_else(|| permission_required("Approve new devices"))
}

/// `GET /pairing/requests`.
pub async fn requests(State(h): State<Shared>, Extension(c): Extension<Connection>) -> Reply {
    require_approver(&caller(&h, &c)?)?;
    Ok(json(StatusCode::OK, &waiting(&h)))
}

/// Sends the PIN to the waiting request, as the console does, and registers
/// the approval first so the handshake finds it.
fn send_pin(
    h: &Host,
    client: &Client,
    body: &PairingApprove,
) -> Result<(String, IpAddr, oneshot::Receiver<Outcome>), Failure> {
    let mut pins = h.pins.lock().unwrap_or_else(PoisonError::into_inner);
    let request = pins
        .get(&body.id)
        .filter(|p| live(p))
        .ok_or_else(no_request)?;
    let address = request.address.to_canonical();
    let device = request.name.clone();
    if approvals(h).ended(address) {
        return Err(Failure::new(
            ErrorCode::PairingEnded,
            format!(
                "Three wrong PINs from {address}. Pair {device} in the Rubylight console on the PC."
            ),
        ));
    }
    let cooling = approvals(h).cooling(address);
    if let Some(seconds) = cooling {
        return Err(Failure::new(
            ErrorCode::PairingCooldown,
            format!(
                "A wrong PIN just came from {address}. It can be approved again in {seconds} s."
            ),
        ));
    }
    // A Moonlight device gets what a PIN typed in the console gives it by
    // default, under the approver's own bits. A WebRTC device keeps its own
    // default and takes no access choice, so an approver without it may not.
    let permission = if request.certificate.is_empty() {
        if !client.allows(LATER_DEVICE_PERMISSION) {
            return Err(super::permission_required(LIST_AND_VIEW));
        }
        None
    } else {
        // Nothing of Play to hand on would store a device that can't connect.
        let play = PLAY_DEVICE_PERMISSION & client.perm;
        if play == 0 {
            return Err(permission_required(LIST_AND_VIEW));
        }
        Some(play)
    };
    let (outcome, receiver) = oneshot::channel();
    approvals(h).running.insert(
        body.id.clone(),
        Approval {
            approver_name: client.name.clone(),
            device: device.clone(),
            address,
            created: Instant::now(),
            outcome: Some(outcome),
        },
    );
    let request = pins.remove(&body.id).ok_or_else(no_request)?;
    if request
        .sender
        .send((
            body.pin.clone(),
            device.clone(),
            permission,
            Some(client.uuid.clone()),
        ))
        .is_err()
    {
        approvals(h).running.remove(&body.id);
        return Err(no_request());
    }
    Ok((device, address, receiver))
}

/// `POST /pairing/approve`: answers once the new device has finished, or
/// after [`HANDSHAKE_WAIT`].
pub async fn approve(
    State(h): State<Shared>,
    Extension(c): Extension<Connection>,
    request: Request,
) -> Reply {
    let client = caller(&h, &c)?;
    require_approver(&client)?;
    let body: PairingApprove =
        serde_json::from_slice(&read_body(request).await?).map_err(invalid)?;
    if body.pin.len() != 4 || !body.pin.bytes().all(|c| c.is_ascii_digit()) {
        return Err(Failure::new(
            ErrorCode::InvalidValue,
            "The PIN has four digits.",
        ));
    }
    let (device, address, receiver) = send_pin(&h, &client, &body)?;
    tracing::info!(approver = %client.name, client = %device, %address, "PIN sent for a pairing approved from another device");
    match tokio::time::timeout(HANDSHAKE_WAIT, receiver).await {
        Ok(Ok(Outcome::Paired)) => Ok(json(StatusCode::OK, &PairingApproved { name: device })),
        Ok(Ok(Outcome::WrongPin)) => Err(Failure::new(
            ErrorCode::WrongPin,
            format!("That is not the PIN {device} shows. It has a new one when it asks again."),
        )),
        Ok(Ok(Outcome::ApproverRevoked(missing))) => Err(permission_required(missing)),
        Ok(Ok(Outcome::NotSaved)) => Err(Failure::new(
            ErrorCode::PairingUnconfirmed,
            format!("The PC could not save {device}."),
        )),
        // The handshake went quiet. Moonlight may stop after a wrong PIN without
        // saying so, so it counts as one unless the device still finishes.
        Ok(Err(_)) | Err(_) => {
            let quiet = approvals(&h)
                .running
                .get_mut(&body.id)
                .is_some_and(|approval| approval.outcome.take().is_some());
            if quiet {
                tracing::warn!(approver = %client.name, client = %device, %address, "Pairing approval: the device did not finish");
                loud_strike(&h, address, &device);
            }
            Err(Failure::new(
                ErrorCode::PairingUnconfirmed,
                format!("{device} did not finish pairing. Check the PIN it shows."),
            ))
        }
    }
}

/// `POST /pairing/deny`: ends the request's wait, as removing it does.
pub async fn deny(
    State(h): State<Shared>,
    Extension(c): Extension<Connection>,
    request: Request,
) -> Reply {
    let client = caller(&h, &c)?;
    require_approver(&client)?;
    let body: PairingDeny = serde_json::from_slice(&read_body(request).await?).map_err(invalid)?;
    let removed = h
        .pins
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(&body.id)
        .ok_or_else(no_request)?;
    tracing::warn!(approver = %client.name, client = %removed.name, address = %removed.address, "Pairing request denied from another device");
    drop(removed);
    Ok(json(StatusCode::OK, &waiting(&h)))
}
