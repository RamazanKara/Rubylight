//! A strict ISO BMFF reader for the muxer tests: parses the box tree (containers, sample
//! descriptions and sample entries) and asserts that every size adds up.

#[derive(Debug)]
pub(crate) struct Node {
    pub kind: [u8; 4],
    pub start: usize,
    pub body: usize,
    pub end: usize,
    pub children: Vec<Node>,
}

impl Node {
    pub fn kind(&self) -> &str {
        std::str::from_utf8(&self.kind).unwrap()
    }

    pub fn payload<'a>(&self, data: &'a [u8]) -> &'a [u8] {
        &data[self.body..self.end]
    }

    pub fn child(&self, kind: &str) -> &Node {
        self.children
            .iter()
            .find(|node| node.kind() == kind)
            .unwrap_or_else(|| panic!("no {kind} in {}", self.kind()))
    }

    pub fn all(&self, kind: &str) -> Vec<&Node> {
        self.children
            .iter()
            .filter(|node| node.kind() == kind)
            .collect()
    }

    pub fn path(&self, path: &[&str]) -> &Node {
        path.iter().fold(self, |node, kind| node.child(kind))
    }

    pub fn has(&self, kind: &str) -> bool {
        self.children.iter().any(|node| node.kind() == kind)
    }
}

/// Where children start inside a payload, for boxes that hold boxes.
fn children_at(kind: &[u8; 4]) -> Option<usize> {
    match kind {
        b"moov" | b"trak" | b"mdia" | b"minf" | b"dinf" | b"stbl" | b"mvex" | b"moof" | b"traf" => {
            Some(0)
        }
        b"stsd" | b"dref" => Some(8),
        b"avc1" | b"hvc1" | b"av01" => Some(78),
        b"Opus" => Some(28),
        _ => None,
    }
}

pub(crate) fn parse(data: &[u8]) -> Vec<Node> {
    parse_range(data, 0, data.len())
}

fn parse_range(data: &[u8], from: usize, to: usize) -> Vec<Node> {
    let mut nodes = Vec::new();
    let mut pos = from;
    while pos < to {
        assert!(pos + 8 <= to, "torn box header at {pos}");
        let mut size = be32(data, pos) as usize;
        let mut body = pos + 8;
        if size == 1 {
            size = be64(data, pos + 8) as usize;
            body += 8;
        }
        assert!(size >= body - pos, "box at {pos} smaller than its header");
        let end = pos + size;
        assert!(
            end <= to,
            "box at {pos} runs past its parent ({end} > {to})"
        );
        let mut kind = [0u8; 4];
        kind.copy_from_slice(&data[pos + 4..pos + 8]);
        let children = match children_at(&kind) {
            Some(skip) => parse_range(data, body + skip, end),
            None => Vec::new(),
        };
        nodes.push(Node {
            kind,
            start: pos,
            body,
            end,
            children,
        });
        pos = end;
    }
    assert_eq!(pos, to, "children overrun");
    nodes
}

pub(crate) fn be16(data: &[u8], at: usize) -> u16 {
    u16::from_be_bytes([data[at], data[at + 1]])
}

pub(crate) fn be32(data: &[u8], at: usize) -> u32 {
    u32::from_be_bytes(data[at..at + 4].try_into().unwrap())
}

pub(crate) fn be64(data: &[u8], at: usize) -> u64 {
    u64::from_be_bytes(data[at..at + 8].try_into().unwrap())
}

pub(crate) fn kinds(nodes: &[Node]) -> Vec<&str> {
    nodes.iter().map(Node::kind).collect()
}

/// A `trun`: data offset and (duration, size, flags) per sample.
pub(crate) struct Trun {
    pub data_offset: i32,
    pub samples: Vec<(u32, u32, Option<u32>)>,
}

pub(crate) fn trun(data: &[u8], node: &Node) -> Trun {
    let payload = node.payload(data);
    let flags = be32(payload, 0) & 0x00FF_FFFF;
    assert_eq!(
        flags & 0x301,
        0x301,
        "data offset, durations and sizes present"
    );
    let count = be32(payload, 4) as usize;
    let data_offset = be32(payload, 8) as i32;
    let with_flags = flags & 0x400 != 0;
    let entry = if with_flags { 12 } else { 8 };
    assert_eq!(payload.len(), 12 + count * entry);
    let samples = (0..count)
        .map(|index| {
            let at = 12 + index * entry;
            let flags = with_flags.then(|| be32(payload, at + 8));
            (be32(payload, at), be32(payload, at + 4), flags)
        })
        .collect();
    Trun {
        data_offset,
        samples,
    }
}

/// `tfdt` base media decode time (version 1).
pub(crate) fn tfdt(data: &[u8], traf: &Node) -> u64 {
    let payload = traf.child("tfdt").payload(data);
    assert_eq!(payload[0], 1, "64-bit tfdt");
    be64(payload, 4)
}

/// `tfhd` track id; asserts default-base-is-moof.
pub(crate) fn tfhd_track(data: &[u8], traf: &Node) -> u32 {
    let payload = traf.child("tfhd").payload(data);
    assert_eq!(be32(payload, 0) & 0x00FF_FFFF, 0x0002_0000);
    be32(payload, 4)
}
