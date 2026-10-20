//! The DER, PEM and base64 pieces pairing needs: reading the parts of an X.509 certificate
//! and writing the client's self-signed one.

use super::Error;

pub(crate) const TAG_INTEGER: u8 = 0x02;
pub(crate) const TAG_BIT_STRING: u8 = 0x03;
pub(crate) const TAG_NULL: u8 = 0x05;
pub(crate) const TAG_OID: u8 = 0x06;
pub(crate) const TAG_UTF8_STRING: u8 = 0x0c;
pub(crate) const TAG_UTC_TIME: u8 = 0x17;
pub(crate) const TAG_GENERALIZED_TIME: u8 = 0x18;
pub(crate) const TAG_SEQUENCE: u8 = 0x30;
pub(crate) const TAG_SET: u8 = 0x31;
pub(crate) const TAG_CONTEXT_0: u8 = 0xa0;

/// 1.2.840.113549.1.1.1
pub(crate) const OID_RSA_ENCRYPTION: &[u8] =
    &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01];
/// 1.2.840.113549.1.1.11
pub(crate) const OID_SHA256_WITH_RSA: &[u8] =
    &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0b];
/// 1.2.840.10045.2.1
pub(crate) const OID_EC_PUBLIC_KEY: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01];
/// 1.2.840.10045.3.1.7
pub(crate) const OID_P256: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07];
/// 1.3.132.0.34
pub(crate) const OID_P384: &[u8] = &[0x2b, 0x81, 0x04, 0x00, 0x22];
/// 2.5.4.3
pub(crate) const OID_COMMON_NAME: &[u8] = &[0x55, 0x04, 0x03];

fn malformed() -> Error {
    Error::Malformed("invalid DER certificate".into())
}

/// (tag, content, whole element, rest).
pub(crate) type Tlv<'a> = (u8, &'a [u8], &'a [u8], &'a [u8]);

/// One tag-length-value at the start of `input`.
pub(crate) fn read(input: &[u8]) -> Result<Tlv<'_>, Error> {
    let (&tag, after_tag) = input.split_first().ok_or_else(malformed)?;
    let (&first, mut rest) = after_tag.split_first().ok_or_else(malformed)?;
    let length = if first < 0x80 {
        first as usize
    } else {
        let count = (first & 0x7f) as usize;
        if count == 0 || count > 4 || rest.len() < count {
            return Err(malformed());
        }
        let mut length = 0usize;
        for &b in &rest[..count] {
            length = (length << 8) | b as usize;
        }
        rest = &rest[count..];
        length
    };
    if rest.len() < length {
        return Err(malformed());
    }
    let header = input.len() - rest.len();
    Ok((
        tag,
        &rest[..length],
        &input[..header + length],
        &rest[length..],
    ))
}

/// (content, rest) of an element that must have `tag`.
fn expect(input: &[u8], tag: u8) -> Result<(&[u8], &[u8]), Error> {
    let (actual, content, _, rest) = read(input)?;
    if actual != tag {
        return Err(malformed());
    }
    Ok((content, rest))
}

/// The parts of an X.509 certificate that pairing uses.
#[derive(Debug, Clone)]
pub(crate) struct CertificateParts {
    /// The signature value (the BIT STRING's bytes), which Moonlight pairing hashes.
    pub signature: Vec<u8>,
    pub key_algorithm: Vec<u8>,
    pub key_parameters: Option<Vec<u8>>,
    /// The subjectPublicKey BIT STRING's bytes: an RSAPublicKey or an EC point.
    pub public_key: Vec<u8>,
}

pub(crate) fn parse_certificate(der: &[u8]) -> Result<CertificateParts, Error> {
    let (certificate, rest) = expect(der, TAG_SEQUENCE)?;
    if !rest.is_empty() {
        return Err(malformed());
    }
    let (tbs, after_tbs) = expect(certificate, TAG_SEQUENCE)?;
    let (_, after_algorithm) = expect(after_tbs, TAG_SEQUENCE)?;
    let (signature, _) = expect(after_algorithm, TAG_BIT_STRING)?;
    let signature = bit_string(signature)?;

    let mut fields = tbs;
    if fields.first() == Some(&TAG_CONTEXT_0) {
        fields = read(fields)?.3;
    }
    // serialNumber, signature, issuer, validity, subject
    fields = expect(fields, TAG_INTEGER)?.1;
    for _ in 0..4 {
        fields = expect(fields, TAG_SEQUENCE)?.1;
    }
    let (spki, _) = expect(fields, TAG_SEQUENCE)?;
    let (algorithm, key) = expect(spki, TAG_SEQUENCE)?;
    let (key_algorithm, parameters) = expect(algorithm, TAG_OID)?;
    let key_parameters = match read(parameters) {
        Ok((TAG_OID, oid, _, _)) => Some(oid.to_vec()),
        _ => None,
    };
    let (public_key, _) = expect(key, TAG_BIT_STRING)?;
    Ok(CertificateParts {
        signature: signature.to_vec(),
        key_algorithm: key_algorithm.to_vec(),
        key_parameters,
        public_key: bit_string(public_key)?.to_vec(),
    })
}

fn bit_string(content: &[u8]) -> Result<&[u8], Error> {
    match content.split_first() {
        Some((0, bits)) => Ok(bits),
        _ => Err(malformed()),
    }
}

/// Appends a DER element.
pub(crate) fn write(out: &mut Vec<u8>, tag: u8, content: &[u8]) {
    out.push(tag);
    let length = content.len();
    if length < 0x80 {
        out.push(length as u8);
    } else {
        let bytes = length.to_be_bytes();
        let skip = bytes.iter().take_while(|&&b| b == 0).count();
        out.push(0x80 | (bytes.len() - skip) as u8);
        out.extend_from_slice(&bytes[skip..]);
    }
    out.extend_from_slice(content);
}

pub(crate) fn element(tag: u8, content: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(content.len() + 6);
    write(&mut out, tag, content);
    out
}

pub(crate) fn sequence(parts: &[&[u8]]) -> Vec<u8> {
    element(TAG_SEQUENCE, &parts.concat())
}

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub(crate) fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(BASE64[(n >> (18 - 6 * i)) as usize & 63] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

pub(crate) fn base64_decode(text: &str) -> Result<Vec<u8>, Error> {
    let invalid = || Error::Malformed("invalid base64".into());
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let mut accumulator = 0u32;
    let mut bits = 0;
    let mut padding = 0;
    for c in text.bytes() {
        let value = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => {
                padding += 1;
                continue;
            }
            b' ' | b'\t' | b'\r' | b'\n' => continue,
            _ => return Err(invalid()),
        };
        if padding > 0 {
            return Err(invalid());
        }
        accumulator = (accumulator << 6) | value as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((accumulator >> bits) as u8);
        }
    }
    if padding > 2 || bits >= 6 {
        return Err(invalid());
    }
    Ok(out)
}

/// The first `label` block of a PEM document.
pub(crate) fn pem_decode(text: &str, label: &str) -> Result<Vec<u8>, Error> {
    let begin = format!("-----BEGIN {label}-----");
    let end = format!("-----END {label}-----");
    let start = text
        .find(&begin)
        .ok_or_else(|| Error::Malformed(format!("no {label} in PEM")))?
        + begin.len();
    let stop = text[start..]
        .find(&end)
        .ok_or_else(|| Error::Malformed(format!("unterminated {label} PEM")))?;
    base64_decode(&text[start..start + stop])
}

/// PEM with Unix line endings, which hosts require for the pairing certificate.
pub(crate) fn pem_encode(der: &[u8], label: &str) -> String {
    let body = base64_encode(der);
    let mut out = format!("-----BEGIN {label}-----\n");
    for line in body.as_bytes().chunks(64) {
        out.push_str(std::str::from_utf8(line).unwrap());
        out.push('\n');
    }
    out.push_str(&format!("-----END {label}-----\n"));
    out
}

pub(crate) fn hex_encode(data: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(data.len() * 2);
    for &b in data {
        out.push(DIGITS[(b >> 4) as usize] as char);
        out.push(DIGITS[(b & 15) as usize] as char);
    }
    out
}

pub(crate) fn hex_decode(text: &str) -> Result<Vec<u8>, Error> {
    if text.len() % 2 != 0 {
        return Err(Error::Malformed(format!(
            "invalid pairing hex length: {}",
            text.len()
        )));
    }
    let digit = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
    text.as_bytes()
        .chunks(2)
        .map(|pair| match (digit(pair[0]), digit(pair[1])) {
            (Some(high), Some(low)) => Ok(high << 4 | low),
            _ => Err(Error::Malformed("invalid hex in pairing response".into())),
        })
        .collect()
}

/// UTCTime before 2050, GeneralizedTime after, as RFC 5280 requires.
pub(crate) fn time(unix_seconds: i64) -> Vec<u8> {
    let days = unix_seconds.div_euclid(86_400);
    let seconds = unix_seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let clock = format!(
        "{month:02}{day:02}{:02}{:02}{:02}Z",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60
    );
    if (1950..2050).contains(&year) {
        element(TAG_UTC_TIME, format!("{:02}{clock}", year % 100).as_bytes())
    } else {
        element(TAG_GENERALIZED_TIME, format!("{year:04}{clock}").as_bytes())
    }
}

/// Howard Hinnant's days-to-civil conversion.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trips_every_length() {
        let data: Vec<u8> = (0..=255).collect();
        for n in 0..40 {
            let encoded = base64_encode(&data[..n]);
            assert_eq!(base64_decode(&encoded).unwrap(), &data[..n]);
        }
        assert_eq!(base64_encode(b"Man"), "TWFu");
        assert_eq!(base64_encode(b"Ma"), "TWE=");
        assert!(base64_decode("TW=E").is_err());
    }

    #[test]
    fn times_switch_to_generalized_time_in_2050() {
        assert_eq!(time(0), element(TAG_UTC_TIME, b"700101000000Z"));
        // 2046-10-20 12:34:56
        assert_eq!(time(2_423_651_696), element(TAG_UTC_TIME, b"461020123456Z"));
        // 2050-01-01
        assert_eq!(
            time(2_524_608_000),
            element(TAG_GENERALIZED_TIME, b"20500101000000Z")
        );
    }

    #[test]
    fn long_lengths_round_trip() {
        let content = vec![7u8; 300];
        let encoded = element(TAG_SEQUENCE, &content);
        assert_eq!(&encoded[..4], &[0x30, 0x82, 0x01, 0x2c]);
        let (tag, parsed, whole, rest) = read(&encoded).unwrap();
        assert_eq!(
            (tag, parsed, whole.len(), rest.len()),
            (0x30, &content[..], 304, 0)
        );
    }

    #[test]
    fn hex_matches_the_java_client() {
        assert_eq!(hex_encode(&[0x00, 0xab, 0xff]), "00ABFF");
        assert_eq!(hex_decode("00abFF").unwrap(), [0x00, 0xab, 0xff]);
        assert!(hex_decode("abc").is_err());
        assert!(hex_decode("zz").is_err());
    }
}
