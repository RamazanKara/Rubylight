//! Pairing with a PC and unpairing: by PIN, by link, and approving another device's request.

use super::*;

/// A four-digit PIN from the OS-seeded hasher keys.
pub fn random_pin() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos()),
    );
    format!("{:04}", hasher.finish() % 10_000)
}

/// Whether this record is paired with a certificate other than `certificate`. One that cannot be
/// read is another one.
pub(super) fn pins_another(record: &HostRecord, certificate: &[u8; 32]) -> bool {
    record
        .server_certificate
        .as_deref()
        .is_some_and(|pem| nightjar_core::http::certificate_sha256(pem).ok().as_ref() != Some(certificate))
}

/// What a pairing attempt that reached the host says to the user.
pub(super) fn pair_outcome(result: PairResult, one_time: bool) -> Result<String, String> {
    match result {
        PairResult::Paired { server_certificate_pem } => Ok(server_certificate_pem),
        PairResult::PinWrong if one_time => Err(PAIR_OTP_PIN_WRONG.into()),
        PairResult::PinWrong => Err(PAIR_PIN_WRONG.into()),
        PairResult::AlreadyInProgress => Err(PAIR_IN_PROGRESS.into()),
        PairResult::Failed(_) => Err(PAIR_DECLINED.into()),
    }
}

/// An unpair answer that leaves this device unpaired: done, or the PC never knew it.
pub fn now_unpaired(result: &Result<(), String>) -> bool {
    match result {
        Ok(()) => true,
        Err(e) => e == UNPAIR_NOT_PAIRED,
    }
}

pub(super) fn unpair_error(error: &nightjar_core::http::Error) -> String {
    use nightjar_core::http::Error;
    match error {
        Error::Status { code: 401, .. } | Error::NotPaired => UNPAIR_NOT_PAIRED.into(),
        _ => UNPAIR_FAILED.into(),
    }
}

/// A pairing request that failed on the way: a network problem, or whatever the client says.
pub(super) fn pair_error(error: &nightjar_core::http::Error) -> String {
    match error {
        nightjar_core::http::Error::Io(_) => PAIR_NETWORK.into(),
        // Pairing starts without a stored certificate, so only a link's fingerprint refuses one.
        nightjar_core::http::Error::CertificateMismatch => PAIR_LINK_MISMATCH.into(),
        other => describe(other),
    }
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
        let _ = self.sender.send(Event::Paired { id, request: self.pairing_generation, result });
        (self.waker)();
    }

    /// `id` was added for a link and now says it is a PC known here with a certificate other than
    /// the link names. Any web page can make that address the one tried, and nothing has proved
    /// who answered, so the known PC keeps its address and this record goes.
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
        // The PCs a link's certificate cannot be: known with another one. The PIN is not spent on
        // something that says it is one of them.
        let known_otherwise: Vec<String> = match &secret {
            Secret::Link(link) => self
                .list
                .iter()
                .filter(|host| !host.record.unique_id.is_empty() && pins_another(&host.record, &link.cert_sha256))
                .map(|host| host.record.unique_id.clone())
                .collect(),
            _ => Vec::new(),
        };
        let Some(identity) = self.identity.clone() else {
            return;
        };
        let Some(host) = self.get(id) else {
            return;
        };
        let mut record = host.connect_record();
        record.server_certificate = None;
        let client = match client_for(&identity, &record, &record.address) {
            Ok(client) => Arc::new(client),
            Err(e) => {
                let _ = self.sender.send(Event::Paired { id, request, result: Err(e) });
                return;
            }
        };
        self.pairing = Some((id, client.clone()));
        let one_time = !matches!(secret, Secret::Pin(_));
        self.spawn("pair", move || {
            let result = client
                .server_info_http(true)
                .and_then(|info| match &secret {
                    Secret::Link(_) if known_otherwise.contains(&info.unique_id) => Err(nightjar_core::http::Error::CertificateMismatch),
                    Secret::Pin(pin) => client.pair(&info, pin),
                    Secret::OneTime { pin, passphrase } => client.pair_with_passphrase(&info, pin, passphrase),
                    Secret::Link(link) => client.pair_with_link(&info, link),
                })
                .map_err(|e| pair_error(&e))
                .and_then(|result| pair_outcome(result, one_time));
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
        self.answer_pairing(id, request, name, true, |client, request| client.deny_pairing(request).map(|_| ()));
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
                .ok_or_else(|| en("Nightjar has no client identity yet. Restart Nightjar and try again.").to_owned())
                .and_then(|identity| client_for(&identity, &record, &record.address))
                .and_then(|client| call(&client, &request).map_err(|e| native_error(&e)));
            Some(Event::PairingAnswer { id, request, name, denied, result })
        });
    }

    /// Asks the PC to forget this device. The local certificate stays until the PC has
    /// answered (see `drain`), so a failed or unreachable request leaves the PC paired here.
    pub fn unpair(&mut self, id: u64) -> Result<(), String> {
        let Some(identity) = self.identity.clone() else {
            return Err(en("Nightjar has no client identity yet, so it cannot unpair. Restart Nightjar and try again.").into());
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
        let Some(host) = self.get_mut(id) else { return };
        host.record.server_certificate = None;
        host.apps = Apps::NotLoaded;
        host.art.clear();
        host.ambient.clear();
        host.ambient_glow.clear();
        host.art_versions.clear();
        host.art_sized.clear();
        host.native = Native::default();
        host.set_classic_apps(None);
        host.native_epoch += 1;
    }
}
