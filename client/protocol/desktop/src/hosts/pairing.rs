//! Pairing with a PC and unpairing: by PIN, by link, and approving another device's request.
//!
//! Core pairing classification and execution live in `nightjar_core::hosts::pairing`; this
//! module translates outcomes into Desktop UI constants (`PAIR_*`, `UNPAIR_*`) and [`Event`].

use std::sync::OnceLock;

use super::*;

use nightjar_core::hosts::client_pool::HostClientPool;
pub use nightjar_core::hosts::pairing::random_pin;
use nightjar_policy::hosts::{PairFailureKind, UnpairFailureKind};

type PoolMap = HashMap<String, (Arc<ClientIdentity>, Arc<HostClientPool>)>;
static SHARED_POOLS: OnceLock<Mutex<PoolMap>> = OnceLock::new();

/// Returns the shared [`HostClientPool`] for `identity`, refreshing its identity on rotation.
pub(super) fn shared_pool_for(identity: &Arc<ClientIdentity>) -> Arc<HostClientPool> {
    let map = SHARED_POOLS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = map.lock().unwrap_or_else(|e| e.into_inner());
    let key = identity.unique_id().to_owned();
    if let Some((cached_id, pool)) = guard.get_mut(&key) {
        if !Arc::ptr_eq(cached_id, identity) {
            *cached_id = Arc::clone(identity);
            pool.set_identity(Arc::clone(identity));
        }
        return Arc::clone(pool);
    }
    let pool = Arc::new(HostClientPool::new(Arc::clone(identity)));
    guard.insert(key, (Arc::clone(identity), Arc::clone(&pool)));
    pool
}

/// Whether this record is paired with a certificate other than `certificate`.
pub(super) fn pins_another(record: &HostRecord, certificate: &[u8; 32]) -> bool {
    nightjar_core::hosts::pairing::pins_another_pem(record.server_certificate.as_deref(), certificate)
}

fn format_pair_failure(kind: PairFailureKind) -> String {
    match kind {
        PairFailureKind::PinWrong => PAIR_PIN_WRONG.into(),
        PairFailureKind::OneTimePinWrong => PAIR_OTP_PIN_WRONG.into(),
        PairFailureKind::LinkPinWrong => PAIR_OTP_PIN_WRONG.into(),
        PairFailureKind::AlreadyInProgress => PAIR_IN_PROGRESS.into(),
        PairFailureKind::ServerUnverified => PAIR_UNVERIFIED.into(),
        PairFailureKind::Declined => PAIR_DECLINED.into(),
        PairFailureKind::LinkCertMismatch => PAIR_LINK_MISMATCH.into(),
        PairFailureKind::Network => PAIR_NETWORK.into(),
        PairFailureKind::InGame => PAIR_IN_GAME.into(),
        PairFailureKind::Refused | PairFailureKind::Failed => PAIR_DECLINED.into(),
    }
}

/// What a pairing attempt that reached the host says to the user.
#[cfg(test)]
pub(super) fn pair_outcome(result: PairResult, one_time: bool) -> Result<String, String> {
    nightjar_core::hosts::pairing::pair_outcome_kind(result, false, one_time)
        .map_err(format_pair_failure)
}

/// An unpair answer that leaves this device unpaired: done, or the PC never knew it.
pub fn now_unpaired(result: &Result<(), String>) -> bool {
    match result {
        Ok(()) => true,
        Err(e) => e == UNPAIR_NOT_PAIRED,
    }
}

pub(super) fn unpair_error(error: &nightjar_core::http::Error) -> String {
    match nightjar_core::hosts::pairing::unpair_error_kind(error) {
        UnpairFailureKind::NotPaired => UNPAIR_NOT_PAIRED.into(),
        _ => UNPAIR_FAILED.into(),
    }
}

/// A pairing request that failed on the way: a network problem, or whatever the client says.
#[cfg(test)]
pub(super) fn pair_error(error: &nightjar_core::http::Error) -> String {
    nightjar_core::hosts::pairing::pair_error_kind(error)
        .map(format_pair_failure)
        .unwrap_or_else(|| describe(error))
}

impl Hosts {
    /// Starts pairing with `pin`; the host's user types it in its web UI.
    pub fn pair(&mut self, id: u64, pin: String, passphrase: Option<String>) {
        self.start_pairing(
            id,
            match passphrase {
                Some(passphrase) => Secret::OneTime { pin, passphrase },
                None => Secret::Pin(pin),
            },
        );
    }

    /// Pairs with a QR pairing link (Experimental): its one-time PIN, refused when the PC's
    /// certificate is not the link's.
    pub fn pair_link(&mut self, id: u64, link: PairingUri) {
        if self.get(id).is_some_and(|host| pins_another(&host.record, &link.cert_sha256)) {
            self.refuse_link(id);
            return;
        }
        self.link_claims.insert(id, link.cert_sha256);
        self.start_pairing(id, Secret::Link(Box::new(link)));
    }

    /// Ends the pairing at `id`, if one runs, with what a link to another certificate gets.
    fn refuse_link(&mut self, id: u64) {
        self.cancel_pairing();
        let result = Err(PAIR_LINK_MISMATCH.into());
        let _ = self.sender.send(Event::Paired {
            id,
            request: self.pairing_generation,
            result,
        });
        (self.waker)();
    }

    pub(super) fn drop_link_claiming_known_pc(&mut self, id: u64) {
        eprintln!("nightjar: a pairing link's address claims a known PC with another certificate");
        if self.pairing.as_ref().is_some_and(|(pairing, _)| *pairing == id) {
            self.refuse_link(id);
        }
        self.remove(id);
    }

    fn start_pairing(&mut self, id: u64, secret: Secret) {
        self.cancel_pairing();
        let request = self.pairing_generation;
        let known_otherwise: Vec<String> = match &secret {
            Secret::Link(link) => self
                .list
                .iter()
                .filter(|host| {
                    !host.record.unique_id.is_empty()
                        && pins_another(&host.record, &link.cert_sha256)
                })
                .map(|host| host.record.unique_id.clone())
                .collect(),
            _ => Vec::new(),
        };
        let Some(identity) = self.identity.clone() else {
            let _ = self.sender.send(Event::Paired {
                id,
                request,
                result: Err(tr(
                    "This device’s pairing key is not ready yet. Try again in a moment.",
                )
                .to_owned()),
            });
            (self.waker)();
            return;
        };
        let Some(host) = self.get(id) else {
            let _ = self.sender.send(Event::Paired {
                id,
                request,
                result: Err(tr("This PC is no longer in your list.").to_owned()),
            });
            (self.waker)();
            return;
        };
        let mut record = host.connect_record();
        record.server_certificate = None;
        let client = match client_for(&identity, &record, &record.address) {
            Ok(client) => Arc::new(client),
            Err(e) => {
                let _ = self.sender.send(Event::Paired {
                    id,
                    request,
                    result: Err(e),
                });
                (self.waker)();
                return;
            }
        };
        self.pairing = Some((id, client.clone()));
        let core_secret = match secret {
            Secret::Pin(pin) => nightjar_core::hosts::pairing::PairSecret::Pin(pin),
            Secret::OneTime { pin, passphrase } => {
                nightjar_core::hosts::pairing::PairSecret::OneTime { pin, passphrase }
            }
            Secret::Link(link) => nightjar_core::hosts::pairing::PairSecret::Link(link),
        };
        self.spawn("pair", move || {
            let result =
                nightjar_core::hosts::pairing::execute_pair(&client, &core_secret, &known_otherwise)
                    .map_err(|err| match err {
                        Ok(kind) => format_pair_failure(kind),
                        Err(e) => describe(&e),
                    });
            Some(Event::Paired { id, request, result })
        });
    }

    pub fn cancel_pairing(&mut self) {
        self.pairing_generation += 1;
        if let Some((_, client)) = self.pairing.take() {
            client.cancel_pending_requests();
        }
    }

    /// Sends the PIN a waiting device shows. The answer takes as long as that device's handshake.
    pub fn approve_pairing(&self, id: u64, request: String, name: String, pin: String) {
        self.answer_pairing(id, request, name, false, move |client, request| {
            client.approve_pairing(request, &pin).map(|_| ())
        });
    }

    /// Ends a waiting device's request.
    pub fn deny_pairing(&self, id: u64, request: String, name: String) {
        self.answer_pairing(id, request, name, true, |client, request| {
            client.deny_pairing(request).map(|_| ())
        });
    }

    fn answer_pairing(
        &self,
        id: u64,
        request: String,
        name: String,
        denied: bool,
        call: impl FnOnce(&HostClient, &str) -> Result<(), nightjar_core::http::Error> + Send + 'static,
    ) {
        let Some(host) = self.get(id) else { return };
        let record = host.connect_record();
        let identity = self.identity.clone();
        self.spawn("approve-device", move || {
            let result = identity
                .ok_or_else(|| {
                    en("Nightjar has no client identity yet. Restart Nightjar and try again.")
                        .to_owned()
                })
                .and_then(|identity| client_for(&identity, &record, &record.address))
                .and_then(|client| call(&client, &request).map_err(|e| native_error(&e)));
            Some(Event::PairingAnswer {
                id,
                request,
                name,
                denied,
                result,
            })
        });
    }

    /// Asks the PC to forget this device.
    pub fn unpair(&mut self, id: u64) -> Result<(), String> {
        let Some(identity) = self.identity.clone() else {
            return Err(en(
                "Nightjar has no client identity yet, so it cannot unpair. Restart Nightjar and try again.",
            )
            .into());
        };
        let Some(host) = self.get(id) else {
            return Ok(());
        };
        let record = host.connect_record();
        self.spawn("unpair", move || {
            let result = client_for(&identity, &record, &record.address)
                .map_err(|_| UNPAIR_FAILED.to_string())
                .and_then(|client| client.unpair().map_err(|e| unpair_error(&e)));
            Some(Event::Unpaired { id, result })
        });
        Ok(())
    }

    /// The PC no longer lists this device: drop the certificate and what needed it.
    pub(super) fn forget_pairing(&mut self, id: u64) {
        let uid = {
            let Some(host) = self.get_mut(id) else { return };
            host.record.server_certificate = None;
            host.apps = Apps::NotLoaded;
            Arc::make_mut(&mut host.art).clear();
            Arc::make_mut(&mut host.ambient).clear();
            Arc::make_mut(&mut host.ambient_glow).clear();
            Arc::make_mut(&mut host.art_versions).clear();
            Arc::make_mut(&mut host.art_sized).clear();
            Arc::make_mut(&mut host.art_retry).clear();
            host.native = Native::default();
            host.set_classic_apps(None);
            host.native_epoch += 1;
            host.record.unique_id.clone()
        };
        if !uid.is_empty() && !self.list.iter().any(|h| h.id != id && h.paired() && h.record.unique_id == uid) {
            self.wake_relay_keys.forget(&uid);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_for_reuses_pooled_clients_and_rejects_bad_pem() {
        let identity = Arc::new(ClientIdentity::generate().unwrap());
        let pool = shared_pool_for(&identity);
        pool.clear();

        let mut record = HostRecord {
            unique_id: "u-pool".into(),
            name: "PoolPC".into(),
            address: "192.0.2.15".into(),
            http_port: 47989,
            https_port: 47984,
            ..Default::default()
        };
        let c1 = client_for(&identity, &record, &record.address).unwrap();
        drop(c1);
        assert_eq!(pool.idle_len(), 1);

        // Second borrow reuses the idle pooled client cleanly and returns it again on drop.
        let c2 = client_for(&identity, &record, &record.address).unwrap();
        assert_eq!(pool.idle_len(), 0);
        drop(c2);
        assert_eq!(pool.idle_len(), 1);

        record.server_certificate = Some("not valid pem".into());
        assert!(client_for(&identity, &record, &record.address).is_err());
    }
}
