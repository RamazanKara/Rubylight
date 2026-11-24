//! Encrypted RTSP framing (`rtspenc://`) and TCP request/response transport.

use super::{Handshake, HandshakeParams, RtspError, RtspOutcome};
use crate::priority::{self, Qos};
use ring::aead;
use std::io::{ErrorKind, Read, Write};
use std::net::{IpAddr, SocketAddr, SocketAddrV6, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const RECEIVE_TIMEOUT: Duration = Duration::from_secs(15);
const RETRY_DELAY: Duration = Duration::from_millis(500);
const MAX_RESPONSE_SIZE: usize = 1024 * 1024;

pub(super) const ENCRYPTED_RTSP_BIT: u32 = 0x8000_0000;
pub(super) const ENCRYPTED_HEADER: usize = 24;

/// Sunshine's encrypted RTSP: AES-128-GCM with the remote input key. Each message carries
/// its length with the top bit set, a sequence number and the tag; the IV is the sequence
/// number (little-endian) with 'C' or 'H' and 'R' in its last two bytes.
pub(super) struct RtspCipher {
    key: aead::LessSafeKey,
    sequence: u32,
}

pub(super) fn rtsp_iv(sequence: u32, origin: u8) -> aead::Nonce {
    let mut iv = [0u8; 12];
    iv[..4].copy_from_slice(&sequence.to_le_bytes());
    iv[10] = origin;
    iv[11] = b'R';
    aead::Nonce::assume_unique_for_key(iv)
}

impl RtspCipher {
    pub(super) fn new(key: &[u8; 16]) -> Self {
        let key = aead::UnboundKey::new(&aead::AES_128_GCM, key).expect("AES-128 key length");
        Self {
            key: aead::LessSafeKey::new(key),
            sequence: 0,
        }
    }

    pub(super) fn seal(&mut self, plain: &[u8]) -> Result<Vec<u8>, RtspError> {
        self.sequence = self.sequence.wrapping_add(1);
        let mut body = plain.to_vec();
        let tag = self
            .key
            .seal_in_place_separate_tag(rtsp_iv(self.sequence, b'C'), aead::Aad::empty(), &mut body)
            .map_err(|_| RtspError::Encryption("RTSP encryption failed".into()))?;
        let mut out = Vec::with_capacity(ENCRYPTED_HEADER + body.len());
        out.extend_from_slice(&(ENCRYPTED_RTSP_BIT | plain.len() as u32).to_be_bytes());
        out.extend_from_slice(&self.sequence.to_be_bytes());
        out.extend_from_slice(tag.as_ref());
        out.extend_from_slice(&body);
        Ok(out)
    }

    pub(super) fn open(&self, raw: &[u8]) -> Result<Vec<u8>, RtspError> {
        let fail = |what: &str| RtspError::Encryption(what.into());
        if raw.len() <= ENCRYPTED_HEADER {
            return Err(fail("RTSP encrypted header too small"));
        }
        let type_and_length = u32::from_be_bytes(raw[..4].try_into().unwrap());
        if type_and_length & ENCRYPTED_RTSP_BIT == 0 {
            return Err(fail("rejecting unencrypted RTSP message"));
        }
        let length = (type_and_length & !ENCRYPTED_RTSP_BIT) as usize;
        if length + ENCRYPTED_HEADER != raw.len() {
            return Err(fail("encrypted RTSP message length mismatch"));
        }
        let sequence = u32::from_be_bytes(raw[4..8].try_into().unwrap());
        let mut body = raw[ENCRYPTED_HEADER..].to_vec();
        body.extend_from_slice(&raw[8..24]);
        let plain = self
            .key
            .open_in_place(rtsp_iv(sequence, b'H'), aead::Aad::empty(), &mut body)
            .map_err(|_| fail("failed to decrypt RTSP response"))?;
        Ok(plain.to_vec())
    }
}

/// Runs the handshake over TCP. `interrupted` aborts it from another thread.
pub fn perform_handshake(
    params: HandshakeParams,
    interrupted: &AtomicBool,
) -> Result<RtspOutcome, RtspError> {
    let scope = params.ipv6_scope_id;
    let mut handshake = Handshake::new(params)?;
    let address = match handshake.address() {
        IpAddr::V6(v6) => SocketAddr::V6(SocketAddrV6::new(v6, handshake.rtsp_port(), 0, scope)),
        v4 => SocketAddr::new(v4, handshake.rtsp_port()),
    };
    while let Some(request) = handshake.next_request()? {
        let response = transact(address, &request, interrupted, false, |_| {})?;
        handshake.handle_response(&response)?;
    }
    handshake.finish()
}

/// One request on its own connection, read until the host closes it. Connection refusals
/// are retried for 10 seconds: GFE 3.22 answers /launch before it listens for RTSP.
pub fn transact(
    address: SocketAddr,
    request: &[u8],
    interrupted: &AtomicBool,
    network_priority: bool,
    warn: impl Fn(&str),
) -> Result<Vec<u8>, RtspError> {
    let check = || {
        if interrupted.load(Ordering::SeqCst) {
            Err(RtspError::Interrupted)
        } else {
            Ok(())
        }
    };
    let mut retries = 0;
    let mut socket = loop {
        check()?;
        match TcpStream::connect_timeout(&address, CONNECT_TIMEOUT) {
            Ok(socket) => break socket,
            Err(e) if e.kind() == ErrorKind::ConnectionRefused && retries < 20 => {
                retries += 1;
                std::thread::sleep(RETRY_DELAY);
            }
            Err(e) => return Err(e.into()),
        }
    };
    socket.set_nodelay(true)?;
    // "Prioritize stream traffic" raises this connection like the rest, as the former C engine did.
    if let Err(e) = priority::prioritize_socket(&socket, Qos::BestEffort, network_priority) {
        warn(&format!("Could not set the RTSP socket priority: {e}"));
    }
    socket.write_all(request)?;
    let deadline = Instant::now() + RECEIVE_TIMEOUT;
    let mut response = Vec::new();
    let mut chunk = [0u8; 16 * 1024];
    loop {
        check()?;
        let now = Instant::now();
        if now >= deadline {
            return Err(RtspError::Io(ErrorKind::TimedOut.into()));
        }
        socket.set_read_timeout(Some((deadline - now).min(Duration::from_millis(100))))?;
        match socket.read(&mut chunk) {
            Ok(0) => return Ok(response),
            Ok(n) => {
                if response.len() + n > MAX_RESPONSE_SIZE {
                    return Err(RtspError::Malformed(
                        "RTSP response exceeded maximum allowed size".into(),
                    ));
                }
                response.extend_from_slice(&chunk[..n]);
            }
            Err(e)
                if matches!(
                    e.kind(),
                    ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted
                ) => {}
            Err(e) => return Err(e.into()),
        }
    }
}
