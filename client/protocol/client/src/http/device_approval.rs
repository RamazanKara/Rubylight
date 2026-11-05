//! Approving a device from this one (native `GET /pairing/requests`, `POST /pairing/approve` and
//! `/pairing/deny`), offered while the host lists the feature `pairing.approve` and this device
//! holds Host control. The pinned native API types predate them, so they live here.
//!
//! A refusal's `code` (`wrong_pin`, `pairing_unconfirmed`, `pairing_ended`) is newer than the pinned
//! `ErrorCode` too and reads as `Unknown`; its message, in the host's words, is what is shown.

use serde::{Deserialize, Serialize};

pub const FEATURE_PAIRING_APPROVE: &str = "pairing.approve";

/// `GET /pairing/requests`, and the answer to a denial: who still waits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairingRequests {
    pub requests: Vec<PairingRequest>,
}

/// One waiting device. `name` is the device's own choice; `address` is what the host saw.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairingRequest {
    pub id: String,
    pub name: String,
    pub address: String,
    pub age_seconds: u64,
}

/// The body of `POST /pairing/approve`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairingApprove {
    pub id: String,
    pub pin: String,
}

/// The body of `POST /pairing/deny`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairingDeny {
    pub id: String,
}

/// The answer to an approval: the device that has now paired.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairingApproved {
    pub name: String,
}

/// A Moonlight PIN: four digits, which is all the host accepts too.
pub fn valid_pairing_pin(pin: &str) -> bool {
    pin.len() == 4 && pin.bytes().all(|b| b.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pin_is_four_ascii_digits() {
        assert!(valid_pairing_pin("0123"));
        for pin in ["", "123", "12345", "12a4", "١٢٣٤"] {
            assert!(!valid_pairing_pin(pin), "{pin}");
        }
    }

    #[test]
    fn requests_read_the_hosts_fixture_shape() {
        let read: PairingRequests = serde_json::from_str(
            r#"{"requests":[{"id":"a1","name":"TV","address":"192.168.4.116","age_seconds":12}]}"#,
        )
        .unwrap();
        assert_eq!(read.requests[0].name, "TV");
        assert_eq!(read.requests[0].age_seconds, 12);
    }
}
