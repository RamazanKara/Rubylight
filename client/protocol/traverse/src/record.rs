//! The rendezvous records and every key internet play derives from a paired device's secret.
//!
//! The host makes one 32-byte secret per paired device and gives it to that device over the
//! paired HTTPS connection (`/internet-play/v1`). Both derive from it, with HKDF:
//!
//! - an Ed25519 key per role, which signs that role's DHT item, so only the two can write it;
//! - per hour ([`WINDOW_SECONDS`]) a salt per role, which names the item: the DHT never sees the
//!   same name for long, nor one name for two devices;
//! - the key that encrypts the records, so DHT nodes see neither address nor port;
//! - per connection, from both sides' session nonces, the punched path's tag and keys.
//!
//! The host publishes its record (public endpoints, its ports, a session nonce) for every device;
//! a device that wants in publishes its own (endpoints, a fresh nonce, and the host session it
//! read), and both punch at once.

use crate::{
    crypto::{self, Aead, Key},
    dht::{Id, Item},
};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

pub const VERSION: u8 = 1;
/// A salt is valid for this many seconds; readers also try the previous window.
pub const WINDOW_SECONDS: u64 = 3600;
/// How old a device's record may be for the host to answer it.
pub const CLIENT_FRESH_SECONDS: u64 = 120;
/// The most endpoints a record carries.
pub const MAX_ENDPOINTS: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Host = 1,
    Client = 2,
}

pub fn window(unix_seconds: u64) -> u64 {
    unix_seconds / WINDOW_SECONDS
}

/// The keys of one paired device.
#[derive(Clone)]
pub struct Keys {
    secret: Key,
}

/// The tag and keys of one punched path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PathKeys {
    pub tag: [u8; 8],
    pub host_to_client: Key,
    pub client_to_host: Key,
}

impl Keys {
    pub fn new(secret: Key) -> Self {
        Self { secret }
    }
    pub fn seed(&self, role: Role) -> Key {
        crypto::derive_key(&self.secret, b"dht signing", &[role as u8])
    }
    pub fn public(&self, role: Role) -> [u8; 32] {
        crypto::public_key(&self.seed(role))
    }
    pub fn salt(&self, role: Role, window: u64) -> Vec<u8> {
        let mut context = vec![role as u8];
        context.extend_from_slice(&window.to_be_bytes());
        let mut salt = vec![0; 16];
        crypto::derive(&self.secret, b"dht salt", &context, &mut salt);
        salt
    }
    pub fn target(&self, role: Role, window: u64) -> Id {
        Item::target(&self.public(role), &self.salt(role, window))
    }
    fn record_key(&self) -> Key {
        crypto::derive_key(&self.secret, b"record", &[])
    }
    /// The tag a device's punches carry for a host session: how the host finds the device.
    pub fn device_tag(&self, host_session: &[u8; 16]) -> [u8; 8] {
        let mut tag = [0; 8];
        crypto::derive(&self.secret, b"path tag", host_session, &mut tag);
        tag
    }
    pub fn path(&self, host_session: &[u8; 16], client_session: &[u8; 16]) -> PathKeys {
        let mut context = host_session.to_vec();
        context.extend_from_slice(client_session);
        PathKeys {
            tag: self.device_tag(host_session),
            host_to_client: crypto::derive_key(&self.secret, b"path host to client", &context),
            client_to_host: crypto::derive_key(&self.secret, b"path client to host", &context),
        }
    }
}

/// The host's ports, which a device binds on loopback for the tunnel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ports {
    pub http: u16,
    pub https: u16,
    pub rtsp: u16,
    pub video: u16,
    pub audio: u16,
    pub control: u16,
    pub mic: u16,
}

impl Ports {
    fn list(&self) -> [u16; 7] {
        [
            self.http,
            self.https,
            self.rtsp,
            self.video,
            self.audio,
            self.control,
            self.mic,
        ]
    }
    fn from_list(list: [u16; 7]) -> Self {
        let [http, https, rtsp, video, audio, control, mic] = list;
        Self {
            http,
            https,
            rtsp,
            video,
            audio,
            control,
            mic,
        }
    }
}

/// What the NAT in front of a side does, as far as it knows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Nat {
    #[default]
    Unknown = 0,
    /// The address is public: no NAT.
    None = 1,
    /// Endpoint-independent mapping: the STUN address is the one peers see.
    Cone = 2,
    /// Endpoint-dependent mapping: peers see other ports.
    Symmetric = 3,
}

impl Nat {
    fn from_byte(byte: u8) -> Self {
        match byte {
            1 => Nat::None,
            2 => Nat::Cone,
            3 => Nat::Symmetric,
            _ => Nat::Unknown,
        }
    }
    pub fn from_mapping(mapping: crate::stun::Mapping) -> Self {
        match mapping {
            crate::stun::Mapping::None => Nat::None,
            crate::stun::Mapping::Independent => Nat::Cone,
            crate::stun::Mapping::Dependent => Nat::Symmetric,
            crate::stun::Mapping::Unknown => Nat::Unknown,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub role: Role,
    /// Unix seconds when it was written; also the DHT item's sequence number.
    pub time: u64,
    /// This side's session nonce.
    pub session: [u8; 16],
    /// A device's record: the host session it answers. Zero in the host's.
    pub peer_session: [u8; 16],
    pub nat: Nat,
    pub endpoints: Vec<SocketAddr>,
    /// The host's ports; None in a device's record.
    pub ports: Option<Ports>,
}

impl Record {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = vec![VERSION, self.role as u8];
        out.extend_from_slice(&self.time.to_be_bytes());
        out.extend_from_slice(&self.session);
        out.extend_from_slice(&self.peer_session);
        out.push(self.nat as u8);
        let endpoints = &self.endpoints[..self.endpoints.len().min(MAX_ENDPOINTS)];
        out.push(endpoints.len() as u8);
        for endpoint in endpoints {
            match endpoint.ip() {
                IpAddr::V4(ip) => {
                    out.push(4);
                    out.extend_from_slice(&ip.octets());
                }
                IpAddr::V6(ip) => {
                    out.push(6);
                    out.extend_from_slice(&ip.octets());
                }
            }
            out.extend_from_slice(&endpoint.port().to_be_bytes());
        }
        match self.ports {
            Some(ports) => {
                out.push(1);
                ports
                    .list()
                    .iter()
                    .for_each(|port| out.extend_from_slice(&port.to_be_bytes()));
            }
            None => out.push(0),
        }
        out
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let mut reader = Reader { bytes, at: 0 };
        if reader.byte()? != VERSION {
            return None;
        }
        let role = match reader.byte()? {
            1 => Role::Host,
            2 => Role::Client,
            _ => return None,
        };
        let time = u64::from_be_bytes(reader.take(8)?.try_into().unwrap());
        let session = reader.take(16)?.try_into().unwrap();
        let peer_session = reader.take(16)?.try_into().unwrap();
        let nat = Nat::from_byte(reader.byte()?);
        let count = reader.byte()? as usize;
        if count > MAX_ENDPOINTS {
            return None;
        }
        let mut endpoints = Vec::with_capacity(count);
        for _ in 0..count {
            let ip = match reader.byte()? {
                4 => IpAddr::V4(Ipv4Addr::from(
                    <[u8; 4]>::try_from(reader.take(4)?).unwrap(),
                )),
                6 => IpAddr::V6(Ipv6Addr::from(
                    <[u8; 16]>::try_from(reader.take(16)?).unwrap(),
                )),
                _ => return None,
            };
            let port = u16::from_be_bytes(reader.take(2)?.try_into().unwrap());
            endpoints.push(SocketAddr::new(ip, port));
        }
        let ports = match reader.byte()? {
            0 => None,
            1 => {
                let mut list = [0u16; 7];
                for port in &mut list {
                    *port = u16::from_be_bytes(reader.take(2)?.try_into().unwrap());
                }
                Some(Ports::from_list(list))
            }
            _ => return None,
        };
        // Later versions may append fields; this one reads what it knows.
        Some(Self {
            role,
            time,
            session,
            peer_session,
            nat,
            endpoints,
            ports,
        })
    }

    /// The sealed form stored in the DHT: a random nonce, then the encrypted record.
    pub fn seal(&self, keys: &Keys, nonce: [u8; 12]) -> Vec<u8> {
        let mut body = self.encode();
        Aead::new(&keys.record_key()).seal(nonce, &aad(self.role), &mut body);
        let mut out = nonce.to_vec();
        out.extend_from_slice(&body);
        out
    }

    /// Opens a sealed record of `role`; None when it is not one, or not this device's.
    pub fn open(keys: &Keys, role: Role, sealed: &[u8]) -> Option<Self> {
        if sealed.len() < 12 + crypto::TAG_BYTES {
            return None;
        }
        let nonce: [u8; 12] = sealed[..12].try_into().unwrap();
        let mut body = sealed[12..].to_vec();
        if !Aead::new(&keys.record_key()).open(nonce, &aad(role), &mut body) {
            return None;
        }
        Self::decode(&body).filter(|record| record.role == role)
    }

    /// The signed DHT item for this record in `window`.
    pub fn item(&self, keys: &Keys, window: u64, nonce: [u8; 12]) -> Item {
        Item::sign(
            &keys.seed(self.role),
            keys.salt(self.role, window),
            self.time as i64,
            self.seal(keys, nonce),
        )
    }

    /// The record in a DHT item of `role` for `window`, when the item is this device's.
    pub fn from_item(keys: &Keys, role: Role, window: u64, item: &Item) -> Option<Self> {
        if item.key != keys.public(role) || item.salt != keys.salt(role, window) || !item.verify() {
            return None;
        }
        Self::open(keys, role, &item.value).filter(|record| record.time as i64 == item.seq)
    }
}

fn aad(role: Role) -> [u8; 13] {
    let mut aad = *b"rl-ip-record\0";
    aad[12] = role as u8;
    aad
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let slice = self.bytes.get(self.at..self.at.checked_add(n)?)?;
        self.at += n;
        Some(slice)
    }
    fn byte(&mut self) -> Option<u8> {
        self.take(1).map(|slice| slice[0])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host_record() -> Record {
        Record {
            role: Role::Host,
            time: 1_800_000_000,
            session: [1; 16],
            peer_session: [0; 16],
            nat: Nat::Cone,
            endpoints: vec![
                "203.0.113.5:40000".parse().unwrap(),
                "[2001:db8::5]:40000".parse().unwrap(),
            ],
            ports: Some(Ports {
                http: 47989,
                https: 47984,
                rtsp: 48010,
                video: 47998,
                audio: 48000,
                control: 47999,
                mic: 48001,
            }),
        }
    }

    #[test]
    fn records_round_trip() {
        let record = host_record();
        assert_eq!(Record::decode(&record.encode()), Some(record));
        assert_eq!(Record::decode(&[2, 1]), None);
        assert_eq!(Record::decode(&host_record().encode()[..20]), None);
    }

    #[test]
    fn only_the_paired_device_opens_and_signs_records() {
        let keys = Keys::new([9; 32]);
        let other = Keys::new([10; 32]);
        let record = host_record();
        let sealed = record.seal(&keys, [3; 12]);
        assert_eq!(
            Record::open(&keys, Role::Host, &sealed),
            Some(record.clone())
        );
        assert_eq!(Record::open(&other, Role::Host, &sealed), None);
        // A host record does not pass as a device's.
        assert_eq!(Record::open(&keys, Role::Client, &sealed), None);

        let window = window(record.time);
        let item = record.item(&keys, window, [4; 12]);
        assert_eq!(
            Item::target(&item.key, &item.salt),
            keys.target(Role::Host, window)
        );
        assert_eq!(
            Record::from_item(&keys, Role::Host, window, &item),
            Some(record.clone())
        );
        assert_eq!(
            Record::from_item(&keys, Role::Host, window + 1, &item),
            None
        );
        assert_eq!(Record::from_item(&other, Role::Host, window, &item), None);
        // A re-signed item whose sequence number disagrees with the record is refused.
        let replayed = Item::sign(
            &keys.seed(Role::Host),
            item.salt.clone(),
            item.seq + 1,
            item.value.clone(),
        );
        assert_eq!(
            Record::from_item(&keys, Role::Host, window, &replayed),
            None
        );
    }

    #[test]
    fn names_and_paths_differ_per_device_window_and_session() {
        let a = Keys::new([1; 32]);
        let b = Keys::new([2; 32]);
        assert_ne!(a.target(Role::Host, 5), b.target(Role::Host, 5));
        assert_ne!(a.target(Role::Host, 5), a.target(Role::Host, 6));
        assert_ne!(a.target(Role::Host, 5), a.target(Role::Client, 5));
        let path = a.path(&[1; 16], &[2; 16]);
        assert_eq!(path, a.path(&[1; 16], &[2; 16]));
        assert_ne!(path.host_to_client, path.client_to_host);
        assert_ne!(
            path.host_to_client,
            a.path(&[1; 16], &[3; 16]).host_to_client
        );
        assert_eq!(path.tag, a.path(&[1; 16], &[3; 16]).tag);
        assert_ne!(path.tag, b.path(&[1; 16], &[2; 16]).tag);
        // A record fits a BEP 44 value with room to spare.
        let mut big = host_record();
        big.endpoints = vec!["[2001:db8::1]:1".parse().unwrap(); MAX_ENDPOINTS];
        assert!(big.seal(&a, [0; 12]).len() + 3 < crate::dht::MAX_VALUE);
    }
}
