//! `GET /pairing/requests`, `POST /pairing/approve` and `POST /pairing/deny`:
//! a device with Host control approves another device's pairing request from
//! the couch, while the host lists [`crate::feature::PAIRING_APPROVE`].
use serde::{Deserialize, Serialize};

/// `GET /pairing/requests`, and the answer of `POST /pairing/deny`. The
/// requests never carry a certificate.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairingRequests {
    pub requests: Vec<PairingRequest>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairingRequest {
    /// Names the request in an approve or deny.
    pub id: String,
    /// The name the device chose for itself.
    pub name: String,
    /// Where the request came from, which the device does not choose.
    pub address: String,
    pub age_seconds: u64,
}

/// The body of `POST /pairing/approve`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairingApprove {
    pub id: String,
    /// The four digits the new device shows.
    pub pin: String,
}

/// The body of `POST /pairing/deny`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairingDeny {
    pub id: String,
}

/// The answer of an approval once the new device has finished pairing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairingApproved {
    pub name: String,
}
