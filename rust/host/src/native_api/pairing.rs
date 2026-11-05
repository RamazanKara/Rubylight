//! Approve or deny another device's pairing request from a paired device
//! (`docs/native-api.md`, "Approve a device"). The PIN goes down the console's
//! path: the waiting request's `sender`, read by `nvhttp::do_pair` or the
//! WebRTC signalling. Moonlight's handshake derives its key from the PIN and
//! carries no check of it, so the host learns a PIN was wrong only from how the
//! new device's handshake ends, and that attempt is then over: the device starts
//! again with a new PIN. Wrong PINs are counted per device for that reason.
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
/// Wrong PINs that end a device's requests on this path.
const WRONG_PIN_LIMIT: u8 = 3;
/// How long those wrong PINs are remembered after the last one.
const WRONG_PIN_MEMORY: Duration = Duration::from_secs(300);
/// A little past the handshake's own limit, so a late finish is still capped.
const APPROVAL_LIFETIME: Duration = Duration::from_secs(310);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Outcome {
    Paired,
    WrongPin,
}
struct Approval {
    approver: String,
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
/// The approvals whose new device has not finished yet, by request id, and the
/// wrong PINs per device. A retry has a new request, so the count goes by the
/// address and name it keeps.
#[derive(Default)]
pub struct Approvals {
    running: HashMap<String, Approval>,
    wrong_pins: HashMap<(IpAddr, String), (u8, Instant)>,
}
impl Approvals {
    fn prune(&mut self) {
        self.running
            .retain(|_, approval| approval.created.elapsed() < APPROVAL_LIFETIME);
        self.wrong_pins
            .retain(|_, (_, last)| last.elapsed() < WRONG_PIN_MEMORY);
    }
    fn ended(&mut self, address: IpAddr, name: &str) -> bool {
        self.prune();
        self.wrong_pins
            .get(&(address, name.to_owned()))
            .is_some_and(|(count, _)| *count >= WRONG_PIN_LIMIT)
    }
    /// Counts a wrong PIN. True when this one ended the device's requests.
    fn strike(&mut self, address: IpAddr, name: &str) -> bool {
        let entry = self
            .wrong_pins
            .entry((address, name.to_owned()))
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
    pub(crate) fn wrong_pins_for(&self, name: &str) -> u8 {
        self.wrong_pins
            .iter()
            .find(|((_, device), _)| device == name)
            .map_or(0, |(_, (count, _))| *count)
    }
}
fn approvals(h: &Host) -> std::sync::MutexGuard<'_, Approvals> {
    h.approvals.lock().unwrap_or_else(PoisonError::into_inner)
}

fn loud_strike(h: &Host, address: IpAddr, device: &str) {
    if !approvals(h).strike(address, device) {
        return;
    }
    tracing::warn!(client = %device, %address, "Three wrong PINs from other devices: this device can now only be paired in the console");
    let (title, text) = crate::state::pairing_ended_notice(device);
    butterpollo_windows::tray::notify(&title, &text);
}

/// What a device gets when the request `id` was approved: never more than its
/// approver holds now. A request without an approver keeps `perm` and `extra`.
pub(crate) fn capped(
    h: &Host,
    state: &PairedState,
    id: &str,
    perm: u32,
    mut extra: BTreeMap<String, Value>,
) -> anyhow::Result<(u32, BTreeMap<String, Value>)> {
    let Some(uuid) = approvals(h).running.get(id).map(|a| a.approver.clone()) else {
        return Ok((perm, extra));
    };
    let approver = state
        .clients
        .iter()
        .find(|c| c.uuid == uuid && c.may_approve_pairing())
        .ok_or_else(|| {
            anyhow::anyhow!("the device that approved this pairing may no longer approve")
        })?;
    let perm = perm & approver.perm;
    // Only the console hands this one on.
    extra.remove(PAIRING_APPROVE_KEY);
    // The default follows Launch apps, so it takes an explicit false.
    if perm & LAUNCH_APPS != 0 && !approver.allows_coop_join() {
        extra.insert(coop::DEVICE_KEY.into(), false.into());
    }
    Ok((perm, extra))
}

/// The new device finished pairing. Its approver, if it had one, hears so.
pub(crate) fn paired(h: &Host, id: &str) -> Option<Approver> {
    let mut approvals = approvals(h);
    let mut approval = approvals.running.remove(id)?;
    approvals
        .wrong_pins
        .remove(&(approval.address, approval.device.clone()));
    drop(approvals);
    if let Some(outcome) = approval.outcome.take() {
        let _ = outcome.send(Outcome::Paired);
    }
    tracing::warn!(approver = %approval.approver_name, client = %approval.device, address = %approval.address, "Device paired, approved from another device");
    Some(Approver {
        uuid: approval.approver,
        name: approval.approver_name,
    })
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
    tracing::warn!(approver = %approval.approver_name, client = %approval.device, address = %approval.address, "Pairing approval failed: the PIN is not the one the device shows");
    loud_strike(h, approval.address, &approval.device);
    let _ = outcome.send(Outcome::WrongPin);
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
        .filter(|(_, p)| live(p) && !approvals.ended(p.address.to_canonical(), &p.name))
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
    if approvals(h).ended(address, &device) {
        return Err(Failure::new(
            ErrorCode::PairingEnded,
            format!("Three wrong PINs for {device}. Pair it in the Rubylight console on the PC."),
        ));
    }
    // A Moonlight device gets what a PIN typed in the console gives it by
    // default, under the approver's own bits. A WebRTC device keeps its own
    // default and takes no access choice, so an approver without it may not.
    let permission = if request.certificate.is_empty() {
        if !client.allows(LATER_DEVICE_PERMISSION) {
            return Err(super::permission_required(
                "List applications and View streams",
            ));
        }
        None
    } else {
        Some(PLAY_DEVICE_PERMISSION & client.perm)
    };
    let (outcome, receiver) = oneshot::channel();
    approvals(h).running.insert(
        body.id.clone(),
        Approval {
            approver: client.uuid.clone(),
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
        .send((body.pin.clone(), device.clone(), permission))
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
