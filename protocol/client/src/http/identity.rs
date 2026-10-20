//! The client's pairing identity: an RSA-2048 key and a self-signed certificate named
//! "NVIDIA GameStream Client", valid for 20 years, as moonlight-android's
//! AndroidCryptoProvider makes it, plus the unique ID sent with every request.

use super::{crypto, der, Error};
use rsa::pkcs1::DecodeRsaPrivateKey;
use rsa::pkcs8::{EncodePrivateKey, EncodePublicKey};

const COMMON_NAME: &str = "NVIDIA GameStream Client";
const VALID_SECONDS: i64 = 20 * 365 * 86_400 + 5 * 86_400;

/// A client identity. Persist [`ClientIdentity::certificate_pem`],
/// [`ClientIdentity::private_key_pem`] and [`ClientIdentity::unique_id`]; hosts know a paired
/// client by its certificate.
#[derive(Clone)]
pub struct ClientIdentity {
    unique_id: String,
    certificate_pem: String,
    private_key_pem: String,
    certificate_der: Vec<u8>,
    private_key_pkcs8: Vec<u8>,
    certificate_signature: Vec<u8>,
}

impl std::fmt::Debug for ClientIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientIdentity")
            .field("unique_id", &self.unique_id)
            .finish_non_exhaustive()
    }
}

impl ClientIdentity {
    /// A new key, certificate and random 16-hex-digit unique ID.
    pub fn generate() -> Result<Self, Error> {
        let key = rsa::RsaPrivateKey::new(&mut rand_core::OsRng, 2048)
            .map_err(|e| Error::InvalidIdentity(format!("RSA key generation failed: {e}")))?;
        let pkcs8 = key
            .to_pkcs8_der()
            .map_err(|e| Error::InvalidIdentity(e.to_string()))?;
        let public = rsa::RsaPublicKey::from(&key)
            .to_public_key_der()
            .map_err(|e| Error::InvalidIdentity(e.to_string()))?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        let certificate = self_signed_certificate(pkcs8.as_bytes(), public.as_bytes(), now)?;
        let unique_id = der::hex_encode(&crypto::random::<8>());
        Self::from_der(unique_id, certificate, pkcs8.as_bytes().to_vec())
    }

    /// A stored identity. The key may be PKCS#8 ("PRIVATE KEY") or PKCS#1 ("RSA PRIVATE KEY").
    pub fn from_pem(
        unique_id: impl Into<String>,
        certificate_pem: &str,
        private_key_pem: &str,
    ) -> Result<Self, Error> {
        let certificate = der::pem_decode(certificate_pem, "CERTIFICATE")?;
        let pkcs8 = match der::pem_decode(private_key_pem, "PRIVATE KEY") {
            Ok(pkcs8) => pkcs8,
            Err(_) => {
                let pkcs1 = der::pem_decode(private_key_pem, "RSA PRIVATE KEY")?;
                rsa::RsaPrivateKey::from_pkcs1_der(&pkcs1)
                    .and_then(|key| key.to_pkcs8_der().map_err(Into::into))
                    .map_err(|e| Error::InvalidIdentity(e.to_string()))?
                    .as_bytes()
                    .to_vec()
            }
        };
        Self::from_der(unique_id.into(), certificate, pkcs8)
    }

    /// A stored identity from DER: the certificate and a PKCS#8 key, as the Android app keeps them.
    pub fn from_der(
        unique_id: String,
        certificate_der: Vec<u8>,
        private_key_pkcs8: Vec<u8>,
    ) -> Result<Self, Error> {
        if unique_id.is_empty()
            || !unique_id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
        {
            return Err(Error::InvalidIdentity(
                "unique ID must be ASCII letters, digits, - or _".into(),
            ));
        }
        let parts = der::parse_certificate(&certificate_der)?;
        let key = ring::signature::RsaKeyPair::from_pkcs8(&private_key_pkcs8)
            .map_err(|e| Error::InvalidIdentity(format!("unusable RSA private key: {e}")))?;
        use ring::signature::KeyPair;
        if rsa_public_key(&parts.public_key)? != rsa_public_key(key.public_key().as_ref())? {
            return Err(Error::InvalidIdentity(
                "certificate does not match the private key".into(),
            ));
        }
        Ok(Self {
            unique_id,
            certificate_pem: der::pem_encode(&certificate_der, "CERTIFICATE"),
            private_key_pem: der::pem_encode(&private_key_pkcs8, "PRIVATE KEY"),
            certificate_der,
            private_key_pkcs8,
            certificate_signature: parts.signature,
        })
    }

    pub fn unique_id(&self) -> &str {
        &self.unique_id
    }

    /// PEM with Unix line endings; this exact text is sent to the host while pairing.
    pub fn certificate_pem(&self) -> &str {
        &self.certificate_pem
    }

    /// PKCS#8 PEM.
    pub fn private_key_pem(&self) -> &str {
        &self.private_key_pem
    }

    pub fn certificate_der(&self) -> &[u8] {
        &self.certificate_der
    }

    pub fn private_key_pkcs8(&self) -> &[u8] {
        &self.private_key_pkcs8
    }

    pub(crate) fn certificate_signature(&self) -> &[u8] {
        &self.certificate_signature
    }

    pub(crate) fn sign(&self, data: &[u8]) -> Result<Vec<u8>, Error> {
        crypto::sign(&self.private_key_pkcs8, data)
    }
}

/// (modulus, exponent) of a DER RSAPublicKey, with leading zero bytes stripped.
fn rsa_public_key(der_key: &[u8]) -> Result<(&[u8], &[u8]), Error> {
    let (tag, content, _, _) = der::read(der_key)?;
    if tag != der::TAG_SEQUENCE {
        return Err(Error::InvalidIdentity("certificate key is not RSA".into()));
    }
    let (_, modulus, _, rest) = der::read(content)?;
    let (_, exponent, _, _) = der::read(rest)?;
    let trim = |b: &[u8]| -> usize { b.iter().take_while(|&&x| x == 0).count() };
    Ok((&modulus[trim(modulus)..], &exponent[trim(exponent)..]))
}

fn self_signed_certificate(pkcs8: &[u8], spki: &[u8], now: i64) -> Result<Vec<u8>, Error> {
    let mut serial = crypto::random::<8>();
    // Positive and without a leading zero byte, so the INTEGER stays minimal.
    serial[0] = (serial[0] & 0x7f) | 0x01;
    let algorithm = der::sequence(&[
        &der::element(der::TAG_OID, der::OID_SHA256_WITH_RSA),
        &der::element(der::TAG_NULL, &[]),
    ]);
    let name = der::sequence(&[&der::element(
        der::TAG_SET,
        &der::sequence(&[
            &der::element(der::TAG_OID, der::OID_COMMON_NAME),
            &der::element(der::TAG_UTF8_STRING, COMMON_NAME.as_bytes()),
        ]),
    )]);
    let tbs = der::sequence(&[
        &der::element(der::TAG_CONTEXT_0, &der::element(der::TAG_INTEGER, &[2])),
        &der::element(der::TAG_INTEGER, &serial),
        &algorithm,
        &name,
        &der::sequence(&[&der::time(now), &der::time(now + VALID_SECONDS)]),
        &name,
        spki,
    ]);
    let signature = crypto::sign(pkcs8, &tbs)?;
    let mut bits = vec![0];
    bits.extend_from_slice(&signature);
    Ok(der::sequence(&[
        &tbs,
        &algorithm,
        &der::element(der::TAG_BIT_STRING, &bits),
    ]))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Generated once; RSA key generation is slow in unoptimized test builds.
    pub(crate) fn fixture() -> ClientIdentity {
        static IDENTITY: std::sync::OnceLock<ClientIdentity> = std::sync::OnceLock::new();
        IDENTITY
            .get_or_init(|| ClientIdentity::generate().unwrap())
            .clone()
    }

    #[test]
    fn generated_certificates_are_self_signed_and_reload() {
        let identity = fixture();
        assert_eq!(identity.unique_id().len(), 16);
        assert!(identity
            .certificate_pem()
            .starts_with("-----BEGIN CERTIFICATE-----\n"));
        assert!(!identity.certificate_pem().contains('\r'));
        let parts = der::parse_certificate(identity.certificate_der()).unwrap();
        assert_eq!(parts.key_algorithm, der::OID_RSA_ENCRYPTION);
        assert_eq!(parts.signature.len(), 256);
        // The signature covers the TBS certificate with the certificate's own key.
        let (_, content, _, _) = der::read(identity.certificate_der()).unwrap();
        let (_, _, tbs, _) = der::read(content).unwrap();
        assert!(crypto::verify(&parts, tbs, &parts.signature).unwrap());
        assert!(String::from_utf8_lossy(identity.certificate_der()).contains(COMMON_NAME));

        let reloaded = ClientIdentity::from_pem(
            identity.unique_id(),
            identity.certificate_pem(),
            identity.private_key_pem(),
        )
        .unwrap();
        assert_eq!(reloaded.certificate_der(), identity.certificate_der());
        let signature = reloaded.sign(b"data").unwrap();
        assert!(crypto::verify(&parts, b"data", &signature).unwrap());
    }

    #[test]
    fn mismatched_keys_and_bad_ids_are_refused() {
        let identity = fixture();
        assert!(ClientIdentity::from_der(
            "bad id".into(),
            identity.certificate_der().to_vec(),
            identity.private_key_pkcs8().to_vec()
        )
        .is_err());
        let other = rsa::RsaPrivateKey::new(&mut rand_core::OsRng, 1024).unwrap();
        let other = other.to_pkcs8_der().unwrap().as_bytes().to_vec();
        assert!(
            ClientIdentity::from_der("id".into(), identity.certificate_der().to_vec(), other)
                .is_err()
        );
    }
}
