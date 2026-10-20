//! PIN pairing, the five steps of moonlight-android's PairingManager.java: the salted PIN
//! key, the server certificate, both challenges and the signed secrets, then the pair
//! challenge over pinned HTTPS.

use super::crypto::{self, PairingHash};
use super::der;
use super::identity::ClientIdentity;
use super::xml;
use super::Error;

/// How a pairing attempt ended, when the host answered every request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PairResult {
    /// Paired; keep the certificate and pin it for HTTPS to this host.
    Paired {
        server_certificate_pem: String,
    },
    /// The host's proof did not match: the PIN typed on the host was not this one.
    PinWrong,
    /// Another device is pairing (the host sent no certificate).
    AlreadyInProgress,
    Failed(PairFailure),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PairFailure {
    /// The host answered `paired` 0 at this step.
    Declined(PairStep),
    /// The host's pairing secret was not signed by its certificate's key (a man in the middle).
    ServerSignature,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PairStep {
    ServerCertificate,
    ClientChallenge,
    ServerChallengeResponse,
    ClientPairingSecret,
    PairChallenge,
}

/// The requests pairing makes; `HostClient` implements them over HTTP.
pub(crate) trait PairingTransport {
    /// `pair?updateState=1&{query}` over plain HTTP; `read_timeout` false waits indefinitely.
    fn pair_command(&mut self, query: &str, read_timeout: bool) -> Result<String, Error>;
    /// `pair?updateState=1&phrase=pairchallenge` over HTTPS with the pinned certificate.
    fn pair_challenge(&mut self) -> Result<String, Error>;
    fn pin_server_certificate(&mut self, der: &[u8]) -> Result<(), Error>;
    /// Unpair over HTTPS (the client certificate is registered by then).
    fn unpair(&mut self) -> Result<(), Error>;
    /// Unpair over HTTP, which ends an unfinished pairing on the host.
    fn cancel_pairing(&mut self) -> Result<(), Error>;
}

/// The OTP hash Apollo and Artemis use: SHA-256 of PIN, salt as hex and passphrase.
pub(crate) fn one_time_pin_auth(pin: &str, salt: &[u8], passphrase: &str) -> String {
    der::hex_encode(&crypto::sha256(
        format!("{pin}{}{passphrase}", der::hex_encode(salt)).as_bytes(),
    ))
}

fn paired(reply: &str) -> Result<(xml::Element, bool), Error> {
    let root = xml::parse_reply(reply)?;
    let paired = root.required_text("paired")? == "1";
    Ok((root, paired))
}

pub(crate) fn pair(
    transport: &mut impl PairingTransport,
    identity: &ClientIdentity,
    server_major_version: i32,
    pin: &str,
    passphrase: Option<&str>,
) -> Result<PairResult, Error> {
    let hash = PairingHash::for_generation(server_major_version);
    let salt = crypto::random::<16>();
    let mut salted = salt.to_vec();
    salted.extend_from_slice(pin.as_bytes());
    let key: [u8; 16] = hash.hash(&salted)[..16].try_into().unwrap();

    // Without a passphrase this waits for the user to enter the PIN on the host.
    let mut query = format!(
        "phrase=getservercert&salt={}&clientcert={}",
        der::hex_encode(&salt),
        der::hex_encode(identity.certificate_pem().as_bytes())
    );
    if let Some(passphrase) = passphrase {
        query.push_str(&format!(
            "&otpauth={}",
            one_time_pin_auth(pin, &salt, passphrase)
        ));
    }
    let (root, ok) = paired(&transport.pair_command(&query, passphrase.is_some())?)?;
    if !ok {
        return Ok(PairResult::Failed(PairFailure::Declined(
            PairStep::ServerCertificate,
        )));
    }
    let result = finish(transport, identity, hash, &key, &root);
    if !matches!(result, Ok(PairResult::Paired { .. })) {
        // Cleanup must not replace the original result.
        let _ = transport.cancel_pairing();
    }
    result
}

fn finish(
    transport: &mut impl PairingTransport,
    identity: &ClientIdentity,
    hash: PairingHash,
    key: &[u8; 16],
    certificate_reply: &xml::Element,
) -> Result<PairResult, Error> {
    // Another client pairing at the same time gets no certificate.
    let Some(plain) = certificate_reply.find_text("plaincert") else {
        return Ok(PairResult::AlreadyInProgress);
    };
    let pem = String::from_utf8(der::hex_decode(&plain)?)
        .map_err(|_| Error::Malformed("Invalid pairing certificate".into()))?;
    let server_der = der::pem_decode(&pem, "CERTIFICATE")
        .map_err(|_| Error::Malformed("Invalid pairing certificate".into()))?;
    let server = der::parse_certificate(&server_der)?;
    transport.pin_server_certificate(&server_der)?;

    let challenge = crypto::random::<16>();
    let (root, ok) = paired(&transport.pair_command(
        &format!(
            "clientchallenge={}",
            der::hex_encode(&crypto::aes_ecb(key, &challenge, true))
        ),
        true,
    )?)?;
    if !ok {
        return Ok(PairResult::Failed(PairFailure::Declined(
            PairStep::ClientChallenge,
        )));
    }
    let encrypted = der::hex_decode(&root.required_text("challengeresponse")?)?;
    if encrypted.len() != (hash.len() + 16 + 15) & !15 {
        return Err(Error::Malformed(
            "Invalid encrypted pairing challenge length".into(),
        ));
    }
    let decrypted = crypto::aes_ecb(key, &encrypted, false);
    let server_response = &decrypted[..hash.len()];
    let server_challenge = &decrypted[hash.len()..hash.len() + 16];

    let client_secret = crypto::random::<16>();
    let proof = hash.hash(
        &[
            server_challenge,
            identity.certificate_signature(),
            &client_secret,
        ]
        .concat(),
    );
    let (root, ok) = paired(&transport.pair_command(
        &format!(
            "serverchallengeresp={}",
            der::hex_encode(&crypto::aes_ecb(key, &proof, true))
        ),
        true,
    )?)?;
    if !ok {
        return Ok(PairResult::Failed(PairFailure::Declined(
            PairStep::ServerChallengeResponse,
        )));
    }
    let server_secret_reply = der::hex_decode(&root.required_text("pairingsecret")?)?;
    if server_secret_reply.len() <= 16 {
        return Err(Error::Malformed("Invalid pairing secret length".into()));
    }
    let (server_secret, server_signature) = server_secret_reply.split_at(16);
    if !crypto::verify(&server, server_secret, server_signature)? {
        return Ok(PairResult::Failed(PairFailure::ServerSignature));
    }
    let expected = hash.hash(&[&challenge[..], &server.signature, server_secret].concat());
    if expected != server_response {
        return Ok(PairResult::PinWrong);
    }

    let mut client_pairing_secret = client_secret.to_vec();
    client_pairing_secret.extend_from_slice(&identity.sign(&client_secret)?);
    let (_, ok) = paired(&transport.pair_command(
        &format!(
            "clientpairingsecret={}",
            der::hex_encode(&client_pairing_secret)
        ),
        true,
    )?)?;
    if !ok {
        return Ok(PairResult::Failed(PairFailure::Declined(
            PairStep::ClientPairingSecret,
        )));
    }

    // Hosts show the client as paired only after this first HTTPS request.
    let (_, ok) = paired(&transport.pair_challenge()?)?;
    if !ok {
        let _ = transport.unpair();
        return Ok(PairResult::Failed(PairFailure::Declined(
            PairStep::PairChallenge,
        )));
    }
    Ok(PairResult::Paired {
        server_certificate_pem: pem,
    })
}

#[cfg(test)]
mod tests {
    use super::super::identity::tests::fixture;
    use super::*;
    use std::collections::HashMap;

    /// A host side written from the Rubylight host's pairing steps, with our own crypto.
    struct Host {
        identity: ClientIdentity,
        pin: String,
        key: Option<[u8; 16]>,
        server_secret: [u8; 16],
        server_challenge: [u8; 16],
        client_hash: Vec<u8>,
        client_certificate: Option<Vec<u8>>,
        pinned: Option<Vec<u8>>,
        log: Vec<String>,
        paired: bool,
    }

    fn query(text: &str) -> HashMap<String, String> {
        text.split('&')
            .filter_map(|kv| kv.split_once('='))
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect()
    }

    fn reply(fields: &[(&str, String)]) -> String {
        let mut s = "<root status_code=\"200\">".to_owned();
        for (k, v) in fields {
            s.push_str(&format!("<{k}>{v}</{k}>"));
        }
        s + "</root>"
    }

    impl PairingTransport for Host {
        fn pair_command(&mut self, text: &str, _read_timeout: bool) -> Result<String, Error> {
            let args = query(text);
            let key = |h: &Host| h.key.unwrap();
            if args.get("phrase").map(String::as_str) == Some("getservercert") {
                self.log.push("getservercert".into());
                let salt = der::hex_decode(&args["salt"]).unwrap();
                let pem = String::from_utf8(der::hex_decode(&args["clientcert"]).unwrap()).unwrap();
                self.client_certificate = Some(der::pem_decode(&pem, "CERTIFICATE").unwrap());
                let mut salted = salt.clone();
                salted.extend_from_slice(self.pin.as_bytes());
                self.key = Some(crypto::sha256(&salted)[..16].try_into().unwrap());
                return Ok(reply(&[
                    ("paired", "1".into()),
                    (
                        "plaincert",
                        der::hex_encode(self.identity.certificate_pem().as_bytes()),
                    ),
                ]));
            }
            if let Some(v) = args.get("clientchallenge") {
                self.log.push("clientchallenge".into());
                let challenge = crypto::aes_ecb(&key(self), &der::hex_decode(v).unwrap(), false);
                let signature = &der::parse_certificate(self.identity.certificate_der())
                    .unwrap()
                    .signature;
                let mut response =
                    crypto::sha256(&[&challenge[..], signature, &self.server_secret].concat());
                response.extend_from_slice(&self.server_challenge);
                let encrypted = crypto::aes_ecb(&key(self), &response, true);
                return Ok(reply(&[
                    ("paired", "1".into()),
                    ("challengeresponse", der::hex_encode(&encrypted)),
                ]));
            }
            if let Some(v) = args.get("serverchallengeresp") {
                self.log.push("serverchallengeresp".into());
                self.client_hash = crypto::aes_ecb(&key(self), &der::hex_decode(v).unwrap(), false);
                let mut secret = self.server_secret.to_vec();
                secret.extend_from_slice(&self.identity.sign(&self.server_secret).unwrap());
                return Ok(reply(&[
                    ("paired", "1".into()),
                    ("pairingsecret", der::hex_encode(&secret)),
                ]));
            }
            if let Some(v) = args.get("clientpairingsecret") {
                self.log.push("clientpairingsecret".into());
                let secret = der::hex_decode(v).unwrap();
                let client =
                    der::parse_certificate(self.client_certificate.as_ref().unwrap()).unwrap();
                let expected = crypto::sha256(
                    &[&self.server_challenge[..], &client.signature, &secret[..16]].concat(),
                );
                let ok = expected == self.client_hash
                    && crypto::verify(&client, &secret[..16], &secret[16..]).unwrap();
                self.paired = ok;
                return Ok(reply(&[("paired", if ok { "1" } else { "0" }.into())]));
            }
            panic!("unexpected pairing request {text}");
        }

        fn pair_challenge(&mut self) -> Result<String, Error> {
            self.log.push("pairchallenge".into());
            assert_eq!(
                self.pinned.as_deref(),
                Some(self.identity.certificate_der())
            );
            Ok(reply(&[("paired", "1".into())]))
        }

        fn pin_server_certificate(&mut self, der: &[u8]) -> Result<(), Error> {
            self.pinned = Some(der.to_vec());
            Ok(())
        }

        fn unpair(&mut self) -> Result<(), Error> {
            self.log.push("unpair".into());
            Ok(())
        }

        fn cancel_pairing(&mut self) -> Result<(), Error> {
            self.log.push("cancel".into());
            Ok(())
        }
    }

    fn host(pin: &str) -> Host {
        Host {
            identity: fixture(),
            pin: pin.into(),
            key: None,
            server_secret: [3; 16],
            server_challenge: [4; 16],
            client_hash: Vec::new(),
            client_certificate: None,
            pinned: None,
            log: Vec::new(),
            paired: false,
        }
    }

    #[test]
    fn pairs_and_pins_the_host_certificate() {
        let client = fixture();
        let mut host = host("1234");
        let result = pair(&mut host, &client, 7, "1234", None).unwrap();
        assert_eq!(
            result,
            PairResult::Paired {
                server_certificate_pem: host.identity.certificate_pem().to_owned()
            }
        );
        assert!(host.paired);
        assert_eq!(
            host.log,
            [
                "getservercert",
                "clientchallenge",
                "serverchallengeresp",
                "clientpairingsecret",
                "pairchallenge"
            ]
        );
    }

    #[test]
    fn a_wrong_pin_is_reported_and_the_pairing_cancelled() {
        let mut host = host("1234");
        assert_eq!(
            pair(&mut host, &fixture(), 7, "4321", None).unwrap(),
            PairResult::PinWrong
        );
        assert_eq!(
            host.log,
            [
                "getservercert",
                "clientchallenge",
                "serverchallengeresp",
                "cancel"
            ]
        );
        assert!(!host.paired);
    }

    #[test]
    fn otp_hash_matches_the_host() {
        // Rubylight: hex::encode_upper(sha256(format!("{pin}{salt_hex}{passphrase}"))).
        let salt = [0xab; 16];
        let expected = der::hex_encode(&crypto::sha256(
            format!("1234{}secret", "AB".repeat(16)).as_bytes(),
        ));
        assert_eq!(one_time_pin_auth("1234", &salt, "secret"), expected);
    }
}
