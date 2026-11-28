//! Microphone audio wire framing, SDP offer, and encryption shared by host and client.
use crate::crypto;
use anyhow::{Result, bail};

/// The launch option of a microphone-only session (`rlMicOnly=1`, with
/// `input_only=1`): the device's microphone plays on the host and nothing
/// else goes either way, no picture, sound or input.
pub const ONLY_OPTION: &str = "rlMicOnly";

/// `x-ss-general.encryption*` bit for microphone packets (`SS_ENC_MICROPHONE`).
pub const ENCRYPTION: u32 = 0x08;
pub const PACKET_TYPE_OPUS: u8 = 0x61;
pub const MAGIC: u32 = 0x1234_5678;
pub const HEADER: usize = 12;
/// The port is the base port plus this, after video (9), control (10) and
/// audio (11), as in Apollo.
pub const PORT_OFFSET: u16 = 12;
/// Samples per second; the stream is mono.
pub const RATE: usize = 48_000;

/// The `DESCRIBE` lines that offer the stream. Clients look for the
/// `rtpmap` line to decide whether the host takes a microphone.
pub fn sdp(port: u16) -> String {
    format!(
        "m=audio {port} RTP/AVP 96\r\na=rtpmap:96 opus/48000/1\r\na=fmtp:96 minptime=10;useinbandfec=1\r\n"
    )
}

#[derive(Debug, PartialEq, Eq)]
pub struct Packet<'a> {
    pub sequence: u16,
    /// The client's clock in milliseconds; only for logs.
    pub timestamp: u32,
    pub payload: &'a [u8],
}

/// A microphone datagram's header and payload, or `None` for anything else.
pub fn parse(datagram: &[u8]) -> Option<Packet<'_>> {
    if datagram.len() <= HEADER
        || datagram[1] != PACKET_TYPE_OPUS
        || u32::from_le_bytes(datagram[8..12].try_into().unwrap()) != MAGIC
    {
        return None;
    }
    Some(Packet {
        sequence: u16::from_le_bytes([datagram[2], datagram[3]]),
        timestamp: u32::from_le_bytes(datagram[4..8].try_into().unwrap()),
        payload: &datagram[HEADER..],
    })
}

pub fn iv(key_id: u32, sequence: u16) -> [u8; 16] {
    let mut iv = [0; 16];
    iv[..4].copy_from_slice(&key_id.wrapping_add(u32::from(sequence)).to_be_bytes());
    iv
}

/// The Opus packet inside an encrypted payload.
pub fn open(key: &[u8; 16], key_id: u32, packet: &Packet) -> Result<Vec<u8>> {
    let mut opus = Vec::new();
    open_into(&crypto::CbcKey::new(key), key_id, packet, &mut opus)?;
    Ok(opus)
}

/// [`open`] using a pre-expanded [`crypto::CbcKey`] and reusing `out` across packets.
pub fn open_into<'a>(
    key: &crypto::CbcKey,
    key_id: u32,
    packet: &Packet,
    out: &'a mut Vec<u8>,
) -> Result<&'a [u8]> {
    if packet.payload.is_empty() {
        out.clear();
        bail!("empty microphone payload");
    }
    key.open_into(&iv(key_id, packet.sequence), packet.payload, out)?;
    if let Some(&last) = out.last()
        && out.len().is_multiple_of(16)
    {
        let pad = usize::from(last);
        if (1..16).contains(&pad) && out[out.len() - pad..].iter().all(|&b| b == last) {
            out.truncate(out.len() - pad);
        }
    }
    if out.is_empty() {
        bail!("empty microphone packet");
    }
    Ok(out.as_slice())
}

/// The Opus packet inside a single-PKCS#7 encrypted payload (negotiated via `x-rl-mic.framing:1`).
pub fn open_single(key: &[u8; 16], key_id: u32, packet: &Packet) -> Result<Vec<u8>> {
    let mut opus = Vec::new();
    open_single_into(&crypto::CbcKey::new(key), key_id, packet, &mut opus)?;
    Ok(opus)
}

/// [`open_single`] using a pre-expanded [`crypto::CbcKey`] and reusing `out` across packets.
/// Unlike [`open_into`], performs no heuristic inner-padding strip so 16-byte-aligned Opus frames
/// ending in `0x01` or `[k; k]` round-trip bit-identically.
pub fn open_single_into<'a>(
    key: &crypto::CbcKey,
    key_id: u32,
    packet: &Packet,
    out: &'a mut Vec<u8>,
) -> Result<&'a [u8]> {
    if packet.payload.is_empty() {
        out.clear();
        bail!("empty microphone payload");
    }
    key.open_into(&iv(key_id, packet.sequence), packet.payload, out)?;
    if out.is_empty() {
        bail!("empty microphone packet");
    }
    Ok(out.as_slice())
}

/// An encrypted datagram as a legacy double-padded client sends it, padding included, for tests
/// and probes.
pub fn seal(key: &[u8; 16], key_id: u32, sequence: u16, timestamp: u32, opus: &[u8]) -> Vec<u8> {
    let mut datagram = vec![0, PACKET_TYPE_OPUS];
    datagram.extend_from_slice(&sequence.to_le_bytes());
    datagram.extend_from_slice(&timestamp.to_le_bytes());
    datagram.extend_from_slice(&MAGIC.to_le_bytes());
    let mut padded = opus.to_vec();
    let pad = (16 - opus.len() % 16) % 16;
    padded.resize(opus.len() + pad, pad as u8);
    datagram.extend_from_slice(&crypto::cbc_seal(key, &iv(key_id, sequence), &padded));
    datagram
}

/// An encrypted datagram using single standard PKCS#7 framing (`x-rl-mic.framing:1`).
pub fn seal_single(
    key: &[u8; 16],
    key_id: u32,
    sequence: u16,
    timestamp: u32,
    opus: &[u8],
) -> Vec<u8> {
    let mut datagram = vec![0, PACKET_TYPE_OPUS];
    datagram.extend_from_slice(&sequence.to_le_bytes());
    datagram.extend_from_slice(&timestamp.to_le_bytes());
    datagram.extend_from_slice(&MAGIC.to_le_bytes());
    datagram.extend_from_slice(&crypto::cbc_seal(key, &iv(key_id, sequence), opus));
    datagram
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_into_reuses_buffer_and_matches_open() {
        let key = [0x33u8; 16];
        let cbc = crypto::CbcKey::new(&key);
        let mut out = Vec::with_capacity(128);
        let ptr = out.as_ptr();
        for (seq, len) in [(1u16, 1usize), (2, 15), (3, 16), (4, 17), (5, 47)] {
            let opus: Vec<u8> = (0..len).map(|i| (i * 7 + 0x20) as u8).collect();
            let dg = seal(&key, 0x1234, seq, 100, &opus);
            let pkt = parse(&dg).unwrap();
            let expected = open(&key, 0x1234, &pkt).unwrap();
            let got = open_into(&cbc, 0x1234, &pkt, &mut out).unwrap();
            assert_eq!(got, expected.as_slice());
            assert_eq!(got, opus.as_slice());
            assert_eq!(out.as_ptr(), ptr);
        }
    }

    #[test]
    fn single_pkcs7_preserves_block_aligned_opus_ending_in_padding_bytes() {
        let key = [0x44u8; 16];
        let cbc = crypto::CbcKey::new(&key);
        let mut out = Vec::with_capacity(128);
        for pad in 1u8..16 {
            let mut opus = vec![0xaa; 32];
            let tail = 32 - usize::from(pad);
            opus[tail..].fill(pad);
            let dg = seal_single(&key, 0x5678, u16::from(pad), 200, &opus);
            let pkt = parse(&dg).unwrap();
            let got = open_single_into(&cbc, 0x5678, &pkt, &mut out).unwrap();
            assert_eq!(got, opus.as_slice(), "pad {pad}");
            assert_eq!(open_single(&key, 0x5678, &pkt).unwrap(), opus);
        }
    }
}
