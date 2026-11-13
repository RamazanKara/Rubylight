//! The BitTorrent mainline DHT (BEP 5) as a read-only client (BEP 43), for BEP 44 mutable
//! items: `get` and `put` of a value signed with an Ed25519 key, named by the SHA-1 of the key
//! and a salt. [`Lookup`] is the iterative search towards a target, without IO.

use crate::{bencode::Value, Millis};
use std::{
    cmp::Ordering,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
};

pub type Id = [u8; 20];
pub type Tid = [u8; 2];

/// The nodes a lookup ends at, and stores an item on (BEP 5's K).
pub const K: usize = 8;
/// Queries in flight at once.
pub const ALPHA: usize = 4;
/// How long a node has to answer.
pub const QUERY_TIMEOUT: Millis = 2_000;
/// The largest value BEP 44 stores.
pub const MAX_VALUE: usize = 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Node {
    pub id: Id,
    pub address: SocketAddr,
}

fn query(tid: Tid, name: &str, arguments: Value) -> Vec<u8> {
    Value::dict([
        ("t", Value::bytes(tid)),
        ("y", Value::bytes(*b"q")),
        ("q", Value::bytes(name.as_bytes())),
        ("a", arguments),
        // BEP 43: a read-only node, which others leave out of their routing tables.
        ("ro", Value::Int(1)),
    ])
    .encode()
}

pub fn ping(tid: Tid, id: &Id) -> Vec<u8> {
    query(tid, "ping", Value::dict([("id", Value::bytes(*id))]))
}

pub fn find_node(tid: Tid, id: &Id, target: &Id) -> Vec<u8> {
    query(
        tid,
        "find_node",
        Value::dict([
            ("id", Value::bytes(*id)),
            ("target", Value::bytes(*target)),
            ("want", want()),
        ]),
    )
}

fn want() -> Value {
    Value::List(vec![Value::bytes(*b"n4"), Value::bytes(*b"n6")])
}

/// A BEP 44 get; `seq` asks the node to leave out a value no newer than it.
pub fn get(tid: Tid, id: &Id, target: &Id, seq: Option<i64>) -> Vec<u8> {
    let mut arguments = vec![
        ("id", Value::bytes(*id)),
        ("target", Value::bytes(*target)),
        ("want", want()),
    ];
    if let Some(seq) = seq {
        arguments.push(("seq", Value::Int(seq)));
    }
    query(tid, "get", Value::dict(arguments))
}

/// A signed BEP 44 mutable item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub key: [u8; 32],
    pub salt: Vec<u8>,
    pub seq: i64,
    /// The value, a byte string in the DHT.
    pub value: Vec<u8>,
    pub signature: [u8; 64],
}

impl Item {
    /// The item's DHT name: SHA-1 of the public key and the salt.
    pub fn target(key: &[u8; 32], salt: &[u8]) -> Id {
        crate::crypto::sha1(&[key, salt])
    }
    /// Signs a value with the Ed25519 key `seed` makes.
    pub fn sign(seed: &[u8; 32], salt: Vec<u8>, seq: i64, value: Vec<u8>) -> Self {
        let signature = crate::crypto::sign(seed, &signable(&salt, seq, &value));
        Self {
            key: crate::crypto::public_key(seed),
            salt,
            seq,
            value,
            signature,
        }
    }
    pub fn verify(&self) -> bool {
        crate::crypto::verify(
            &self.key,
            &signable(&self.salt, self.seq, &self.value),
            &self.signature,
        )
    }
}

/// What BEP 44 signs: `4:salt<len>:<salt>` when there is a salt, then `3:seqi<seq>e1:v` and
/// the bencoded value.
pub fn signable(salt: &[u8], seq: i64, value: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(value.len() + salt.len() + 32);
    if !salt.is_empty() {
        out.extend_from_slice(b"4:salt");
        crate::bencode::encode_bytes(salt, &mut out);
    }
    out.extend_from_slice(b"3:seqi");
    out.extend_from_slice(seq.to_string().as_bytes());
    out.extend_from_slice(b"e1:v");
    crate::bencode::encode_bytes(value, &mut out);
    out
}

pub fn put(tid: Tid, id: &Id, token: &[u8], item: &Item) -> Vec<u8> {
    let mut arguments = vec![
        ("id", Value::bytes(*id)),
        ("token", Value::bytes(token)),
        ("k", Value::bytes(item.key)),
        ("seq", Value::Int(item.seq)),
        ("sig", Value::bytes(item.signature)),
        ("v", Value::bytes(item.value.clone())),
    ];
    if !item.salt.is_empty() {
        arguments.push(("salt", Value::bytes(item.salt.clone())));
    }
    query(tid, "put", Value::dict(arguments))
}

/// A KRPC message this client cares about: a reply or an error to one of its queries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    Reply {
        tid: Vec<u8>,
        reply: Reply,
    },
    Error {
        tid: Vec<u8>,
        code: i64,
    },
    /// A query from another node: a read-only node answers none.
    Query,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reply {
    pub id: Option<Id>,
    pub nodes: Vec<Node>,
    pub token: Option<Vec<u8>>,
    pub key: Option<[u8; 32]>,
    pub seq: Option<i64>,
    pub signature: Option<[u8; 64]>,
    pub value: Option<Vec<u8>>,
}

impl Reply {
    /// The mutable item in a get reply, for `salt`, if it is there and its signature holds.
    pub fn item(&self, salt: &[u8]) -> Option<Item> {
        let item = Item {
            key: self.key?,
            salt: salt.to_vec(),
            seq: self.seq?,
            value: self.value.clone()?,
            signature: self.signature?,
        };
        item.verify().then_some(item)
    }
}

/// Whether a datagram can be KRPC: a bencoded dictionary.
pub fn is_krpc(datagram: &[u8]) -> bool {
    datagram.first() == Some(&b'd')
}

pub fn parse(datagram: &[u8]) -> Option<Message> {
    let message = crate::bencode::decode(datagram)?;
    let tid = message.get(b"t")?.as_bytes()?.to_vec();
    match message.get(b"y")?.as_bytes()? {
        b"r" => {
            let r = message.get(b"r")?;
            let fixed = |key: &[u8]| r.get(key).and_then(Value::as_bytes);
            let mut nodes = Vec::new();
            if let Some(compact) = fixed(b"nodes") {
                nodes.extend(compact.chunks_exact(26).map(|chunk| Node {
                    id: chunk[..20].try_into().unwrap(),
                    address: SocketAddr::new(
                        IpAddr::V4(Ipv4Addr::new(chunk[20], chunk[21], chunk[22], chunk[23])),
                        u16::from_be_bytes([chunk[24], chunk[25]]),
                    ),
                }));
            }
            if let Some(compact) = fixed(b"nodes6") {
                nodes.extend(compact.chunks_exact(38).map(|chunk| Node {
                    id: chunk[..20].try_into().unwrap(),
                    address: SocketAddr::new(
                        IpAddr::V6(Ipv6Addr::from(
                            <[u8; 16]>::try_from(&chunk[20..36]).unwrap(),
                        )),
                        u16::from_be_bytes([chunk[36], chunk[37]]),
                    ),
                }));
            }
            nodes.retain(|node| node.address.port() != 0 && !node.address.ip().is_unspecified());
            Some(Message::Reply {
                tid,
                reply: Reply {
                    id: fixed(b"id").and_then(|id| id.try_into().ok()),
                    nodes,
                    token: fixed(b"token").map(<[u8]>::to_vec),
                    key: fixed(b"k").and_then(|key| key.try_into().ok()),
                    seq: r.get(b"seq").and_then(Value::as_int),
                    signature: fixed(b"sig").and_then(|sig| sig.try_into().ok()),
                    value: fixed(b"v").map(<[u8]>::to_vec),
                },
            })
        }
        b"e" => {
            let code = match message.get(b"e")? {
                Value::List(items) => items.first().and_then(Value::as_int).unwrap_or(0),
                _ => 0,
            };
            Some(Message::Error { tid, code })
        }
        b"q" => Some(Message::Query),
        _ => None,
    }
}

fn closer(target: &Id, a: &Id, b: &Id) -> Ordering {
    for i in 0..20 {
        let (x, y) = (a[i] ^ target[i], b[i] ^ target[i]);
        if x != y {
            return x.cmp(&y);
        }
    }
    Ordering::Equal
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum State {
    Fresh,
    Asked { tid: Tid, at: Millis },
    Answered { token: Option<Vec<u8>> },
    Failed,
}

#[derive(Clone, Debug)]
struct Candidate {
    /// None for a bootstrap address, whose id is unknown until it answers.
    id: Option<Id>,
    address: SocketAddr,
    state: State,
}

/// An iterative lookup towards `target`: query the closest nodes known, learn closer ones from
/// their answers, and stop once the [`K`] closest that answer have all answered.
pub struct Lookup {
    target: Id,
    candidates: Vec<Candidate>,
    next_tid: u16,
}

impl Lookup {
    /// Starts from `seeds`: bootstrap routers and nodes known from earlier lookups. `first_tid`
    /// keeps transaction ids apart between lookups sharing a socket.
    pub fn new(target: Id, seeds: &[SocketAddr], first_tid: u16) -> Self {
        let mut candidates: Vec<Candidate> = Vec::new();
        for &address in seeds {
            if !candidates
                .iter()
                .any(|candidate| candidate.address == address)
            {
                candidates.push(Candidate {
                    id: None,
                    address,
                    state: State::Fresh,
                });
            }
        }
        Self {
            target,
            candidates,
            next_tid: first_tid,
        }
    }

    pub fn target(&self) -> &Id {
        &self.target
    }

    fn sort(&mut self) {
        let target = self.target;
        // Unknown ids (bootstrap routers) after every known node.
        self.candidates.sort_by(|a, b| match (&a.id, &b.id) {
            (Some(x), Some(y)) => closer(&target, x, y),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        });
    }

    /// The queries to send now: (address, transaction id). Queries past [`QUERY_TIMEOUT`] fail.
    pub fn poll(&mut self, now: Millis) -> Vec<(SocketAddr, Tid)> {
        for candidate in &mut self.candidates {
            if let State::Asked { at, .. } = candidate.state {
                if now.saturating_sub(at) >= QUERY_TIMEOUT {
                    candidate.state = State::Failed;
                }
            }
        }
        self.sort();
        let mut in_flight = self
            .candidates
            .iter()
            .filter(|c| matches!(c.state, State::Asked { .. }))
            .count();
        let mut out = Vec::new();
        // Only the K closest that have not failed are worth asking.
        let mut considered = 0;
        for index in 0..self.candidates.len() {
            if in_flight >= ALPHA || considered >= K {
                break;
            }
            match self.candidates[index].state {
                State::Failed => continue,
                State::Fresh => {
                    let tid = self.next_tid.to_be_bytes();
                    self.next_tid = self.next_tid.wrapping_add(1);
                    self.candidates[index].state = State::Asked { tid, at: now };
                    out.push((self.candidates[index].address, tid));
                    in_flight += 1;
                }
                _ => {}
            }
            considered += 1;
        }
        out
    }

    /// Takes a reply; false when it answers none of this lookup's queries.
    pub fn on_reply(&mut self, tid: &[u8], from: SocketAddr, reply: &Reply) -> bool {
        let Some(index) = self.candidates.iter().position(|candidate| {
            candidate.address == from
                && matches!(&candidate.state, State::Asked { tid: asked, .. } if asked[..] == *tid)
        }) else {
            return false;
        };
        let candidate = &mut self.candidates[index];
        candidate.state = State::Answered {
            token: reply.token.clone(),
        };
        if candidate.id.is_none() {
            candidate.id = reply.id;
        }
        for node in &reply.nodes {
            if node.address.is_ipv4() != from.is_ipv4() {
                continue;
            }
            if !self
                .candidates
                .iter()
                .any(|known| known.address == node.address || known.id == Some(node.id))
            {
                self.candidates.push(Candidate {
                    id: Some(node.id),
                    address: node.address,
                    state: State::Fresh,
                });
            }
        }
        // Bounded memory: the closest few hundred are plenty.
        if self.candidates.len() > 256 {
            self.sort();
            self.candidates.truncate(256);
        }
        true
    }

    /// Marks a query failed early (an error reply).
    pub fn on_error(&mut self, tid: &[u8], from: SocketAddr) -> bool {
        match self.candidates.iter_mut().find(|candidate| {
            candidate.address == from
                && matches!(&candidate.state, State::Asked { tid: asked, .. } if asked[..] == *tid)
        }) {
            Some(candidate) => {
                candidate.state = State::Failed;
                true
            }
            None => false,
        }
    }

    /// Done when nothing is in flight and the K closest live nodes have answered.
    pub fn done(&mut self) -> bool {
        self.sort();
        let mut live = self
            .candidates
            .iter()
            .filter(|c| c.state != State::Failed)
            .take(K);
        live.all(|c| matches!(c.state, State::Answered { .. }))
            && !self
                .candidates
                .iter()
                .any(|c| matches!(c.state, State::Asked { .. }))
    }

    /// The closest nodes that answered with a write token, for a put.
    pub fn writable(&mut self) -> Vec<(SocketAddr, Vec<u8>)> {
        self.sort();
        self.candidates
            .iter()
            .filter_map(|c| match &c.state {
                State::Answered { token: Some(token) } if c.id.is_some() => {
                    Some((c.address, token.clone()))
                }
                _ => None,
            })
            .take(K)
            .collect()
    }

    /// Nodes that answered, to seed later lookups.
    pub fn responsive(&self) -> Vec<SocketAddr> {
        self.candidates
            .iter()
            .filter(|c| c.id.is_some() && matches!(c.state, State::Answered { .. }))
            .map(|c| c.address)
            .collect()
    }

    pub fn next_tid(&self) -> u16 {
        self.next_tid
    }
}

/// Keeps the newest valid item a lookup's replies carry.
#[derive(Default)]
pub struct Newest {
    pub item: Option<Item>,
}

impl Newest {
    pub fn offer(&mut self, reply: &Reply, salt: &[u8], key: &[u8; 32]) {
        if reply.key.as_ref() != Some(key) {
            return;
        }
        if let Some(item) = reply.item(salt) {
            if self
                .item
                .as_ref()
                .map_or(true, |known| item.seq > known.seq)
            {
                self.item = Some(item);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(byte: u8) -> Id {
        let mut id = [0; 20];
        id[0] = byte;
        id
    }

    fn address(n: u8) -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, n)), 6881)
    }

    fn compact(nodes: &[(Id, SocketAddr)]) -> Vec<u8> {
        let mut out = Vec::new();
        for (id, address) in nodes {
            out.extend_from_slice(id);
            if let SocketAddr::V4(v4) = address {
                out.extend_from_slice(&v4.ip().octets());
                out.extend_from_slice(&v4.port().to_be_bytes());
            }
        }
        out
    }

    #[test]
    fn queries_are_read_only_krpc() {
        let message = crate::bencode::decode(&get(*b"ab", &id(1), &id(2), Some(5))).unwrap();
        assert_eq!(message.get(b"q").unwrap().as_bytes(), Some(&b"get"[..]));
        assert_eq!(message.get(b"ro").unwrap().as_int(), Some(1));
        assert_eq!(
            message.get(b"a").unwrap().get(b"seq").unwrap().as_int(),
            Some(5)
        );
        assert!(is_krpc(&ping(*b"ab", &id(1))));
    }

    // BEP 44's test vector 1: the buffer signed for a mutable item with a salt.
    #[test]
    fn the_signed_buffer_follows_bep_44() {
        assert_eq!(
            signable(b"foobar", 1, b"Hello World!"),
            b"4:salt6:foobar3:seqi1e1:v12:Hello World!".to_vec()
        );
        assert_eq!(
            signable(b"", 1, b"Hello World!"),
            b"3:seqi1e1:v12:Hello World!".to_vec()
        );
    }

    #[test]
    fn items_sign_verify_and_travel_in_replies() {
        let seed = [5; 32];
        let item = Item::sign(&seed, b"salt".to_vec(), 9, b"value".to_vec());
        assert!(item.verify());
        let put_message = crate::bencode::decode(&put(*b"xy", &id(1), b"token", &item)).unwrap();
        assert_eq!(
            put_message
                .get(b"a")
                .unwrap()
                .get(b"salt")
                .unwrap()
                .as_bytes(),
            Some(&b"salt"[..])
        );

        let reply = Value::dict([
            ("t", Value::bytes(*b"xy")),
            ("y", Value::bytes(*b"r")),
            (
                "r",
                Value::dict([
                    ("id", Value::bytes(id(3))),
                    ("token", Value::bytes(*b"tok")),
                    ("k", Value::bytes(item.key)),
                    ("seq", Value::Int(9)),
                    ("sig", Value::bytes(item.signature)),
                    ("v", Value::bytes(*b"value")),
                    ("nodes", Value::bytes(compact(&[(id(4), address(4))]))),
                ]),
            ),
        ])
        .encode();
        let Some(Message::Reply { tid, reply }) = parse(&reply) else {
            panic!("not a reply")
        };
        assert_eq!(tid, b"xy");
        assert_eq!(
            reply.nodes,
            vec![Node {
                id: id(4),
                address: address(4)
            }]
        );
        assert_eq!(reply.item(b"salt"), Some(item.clone()));
        // Another salt names another item, whose signature does not hold.
        assert_eq!(reply.item(b"other"), None);
        let mut newest = Newest::default();
        newest.offer(&reply, b"salt", &item.key);
        assert_eq!(newest.item.as_ref().map(|item| item.seq), Some(9));
        newest.offer(&reply, b"salt", &[0; 32]);
        assert_eq!(newest.item.map(|item| item.seq), Some(9));
    }

    #[test]
    fn errors_and_queries_parse() {
        let error = b"d1:eli201e5:Errore1:t2:aa1:y1:ee";
        assert_eq!(
            parse(error),
            Some(Message::Error {
                tid: b"aa".to_vec(),
                code: 201
            })
        );
        assert_eq!(parse(&ping(*b"aa", &id(1))), Some(Message::Query));
        assert_eq!(parse(b"garbage"), None);
    }

    /// A small simulated DHT: node n (1..=40) knows the nodes n-2..n+2, and the item lives on
    /// the nodes closest to the target. The lookup has to walk there from one router.
    #[test]
    fn a_lookup_walks_to_the_closest_nodes_and_stops() {
        let target = id(30);
        let router = address(200);
        let mut lookup = Lookup::new(target, &[router], 0);
        let neighbours = |n: u8| -> Vec<Node> {
            (n.saturating_sub(2).max(1)..=(n + 2).min(40))
                .filter(|&m| m != n)
                .map(|m| Node {
                    id: id(m),
                    address: address(m),
                })
                .collect()
        };
        let mut now = 0;
        let mut rounds = 0;
        while !lookup.done() {
            rounds += 1;
            assert!(rounds < 100, "the lookup does not end");
            let queries = lookup.poll(now);
            for (to, tid) in queries {
                let n = if to == router {
                    1
                } else {
                    to.ip()
                        .to_string()
                        .rsplit('.')
                        .next()
                        .unwrap()
                        .parse()
                        .unwrap()
                };
                // Node 20 never answers: the lookup must step around it.
                if n == 20 {
                    continue;
                }
                let reply = Reply {
                    id: Some(id(n)),
                    nodes: if to == router {
                        vec![Node {
                            id: id(10),
                            address: address(10),
                        }]
                    } else {
                        neighbours(n)
                    },
                    token: Some(vec![n]),
                    ..Reply::default()
                };
                assert!(lookup.on_reply(&tid, to, &reply));
            }
            now += QUERY_TIMEOUT;
        }
        let writable: Vec<SocketAddr> = lookup
            .writable()
            .into_iter()
            .map(|(address, _)| address)
            .collect();
        assert!(writable.contains(&address(30)), "{writable:?}");
        assert!(!writable.contains(&address(20)));
        // A reply to no query of this lookup is not taken.
        assert!(!lookup.on_reply(b"zz", address(30), &Reply::default()));
    }
}
