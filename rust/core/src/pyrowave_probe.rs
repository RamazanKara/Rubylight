//! The paired PyroWave UDP probe, as Vibeshine serves it (`PyroWaveUdpProbeVersion`,
//! `PyroWaveUdpHandshakeVersion` and `PyroWaveUdpProbeBurstVersion` 1): a client
//! measures which rate, and which pace for whole frames, reaches it without loss.
//! The host sends for two seconds; the client counts what arrived.
use std::time::Duration;

pub const DURATION: Duration = Duration::from_secs(2);
/// The token a probe's packets carry, as 32 lowercase hex characters.
pub const TOKEN_LEN: usize = 32;
/// Bytes each packet is charged on the wire beyond its packet size: RTP,
/// encryption, UDP, IPv6 and Ethernet preamble, gap and FCS, as a stream is.
const WIRE_OVERHEAD: u64 = 134;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub kbps: u32,
    pub port: u16,
    pub packet_size: usize,
    pub token: String,
    /// Wait for the client's token on the host's socket before sending, so
    /// both firewalls have outbound state (`handshake=1`).
    pub handshake: bool,
    /// Send whole frames at this rate, each at `pace_kbps` in 1 ms groups.
    pub burst: Option<Burst>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Burst {
    pub fps: u32,
    pub pace_kbps: u32,
}

/// The probe a query asks for, or None for any value out of range: 5-3000
/// Mbps, an unprivileged port, 256-1392 byte packets, and for a burst 10-500
/// fps at a pace no lower than the rate.
pub fn parse<'a>(arg: impl Fn(&str) -> Option<&'a str>) -> Option<Request> {
    let number = |key: &str| -> Option<u32> {
        let value = arg(key)?;
        (!value.is_empty() && value.len() <= 7 && value.bytes().all(|b| b.is_ascii_digit()))
            .then(|| value.parse().ok())
            .flatten()
    };
    let kbps = number("kbps").filter(|k| (5_000..=3_000_000).contains(k))?;
    let port = number("port").filter(|p| (1024..=65535).contains(p))? as u16;
    let packet_size = number("packetsize").filter(|s| (256..=1392).contains(s))? as usize;
    let token = arg("token")?;
    if token.len() != TOKEN_LEN
        || !token
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    {
        return None;
    }
    let handshake = match arg("handshake").unwrap_or("0") {
        "0" => false,
        "1" => true,
        _ => return None,
    };
    let burst = if arg("fps").is_some() || arg("pacekbps").is_some() {
        let fps = number("fps").filter(|f| (10..=500).contains(f))?;
        let pace_kbps = number("pacekbps").filter(|p| *p >= kbps && *p <= 9_999_999)?;
        Some(Burst { fps, pace_kbps })
    } else {
        None
    };
    Some(Request {
        kbps,
        port,
        packet_size,
        token: token.to_owned(),
        handshake,
        burst,
    })
}

/// Packets the probe plans to send in its two seconds.
pub fn expected(kbps: u32, packet_size: usize) -> u32 {
    (u64::from(kbps) * DURATION.as_millis() as u64 / (8 * (packet_size as u64 + WIRE_OVERHEAD)))
        as u32
}

/// A probe packet: the token, then the big-endian sequence number, then filler,
/// `packet_size + 48` bytes in all.
pub fn packet(token: &str, packet_size: usize) -> Vec<u8> {
    let mut packet = vec![0x5a; packet_size + 48];
    packet[..TOKEN_LEN].copy_from_slice(token.as_bytes());
    packet
}
pub fn set_sequence(packet: &mut [u8], sequence: u32) {
    packet[TOKEN_LEN..TOKEN_LEN + 4].copy_from_slice(&sequence.to_be_bytes());
}

/// Packets per 1 ms group at `pace_bps`, charging UDP, IP and Ethernet bytes.
fn group(pace_bps: u64, payload_bytes: usize, ipv6: bool) -> u32 {
    (crate::network_pacing::pyrowave_batch_limit(pace_bps, payload_bytes, ipv6) / payload_bytes)
        as u32
}

/// When to send up to which sequence number: (offset from the start, packets
/// sent by then). Without a burst, each millisecond sends its even share; with
/// one, `fps * 2` frames each start on their cadence and leave in 1 ms groups
/// at the pace.
pub fn schedule(request: &Request, ipv6: bool) -> Vec<(Duration, u32)> {
    let total = expected(request.kbps, request.packet_size);
    let millis = DURATION.as_millis() as u32;
    let Some(burst) = request.burst else {
        return (0..millis)
            .map(|tick| {
                let end = (u64::from(total) * u64::from(tick + 1) / u64::from(millis)) as u32;
                (Duration::from_millis(u64::from(tick)), end)
            })
            .collect();
    };
    let frames = (burst.fps * 2).max(1);
    let group = group(
        u64::from(burst.pace_kbps) * 1000,
        request.packet_size + 48,
        ipv6,
    );
    let mut steps = vec![];
    for frame in 0..frames {
        let start = DURATION * frame / frames;
        let mut sent = (u64::from(total) * u64::from(frame) / u64::from(frames)) as u32;
        let end = (u64::from(total) * u64::from(frame + 1) / u64::from(frames)) as u32;
        let mut offset = Duration::ZERO;
        while sent < end {
            let next = end.min(sent + group);
            steps.push((start + offset, next));
            let wire = request.packet_size + 48 + crate::network_pacing::overhead(ipv6);
            offset += Duration::from_secs_f64(
                f64::from(next - sent) * wire as f64 * 8. / (f64::from(burst.pace_kbps) * 1000.),
            );
            sent = next;
        }
    }
    steps
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn query(pairs: &[(&str, &str)]) -> Option<Request> {
        let map: HashMap<&str, &str> = pairs.iter().copied().collect();
        parse(|key| map.get(key).copied())
    }
    const TOKEN: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn a_probe_takes_only_the_ranges_vibeshine_accepts() {
        let base = [
            ("kbps", "100000"),
            ("port", "50000"),
            ("packetsize", "1392"),
            ("token", TOKEN),
        ];
        let request = query(&base).unwrap();
        assert_eq!(
            (request.kbps, request.port, request.packet_size),
            (100_000, 50_000, 1392)
        );
        assert!(!request.handshake && request.burst.is_none());
        for (key, bad) in [
            ("kbps", "4999"),
            ("kbps", "3000001"),
            ("kbps", "-5000"),
            ("port", "1023"),
            ("packetsize", "1393"),
            ("packetsize", "255"),
            ("token", "0123456789ABCDEF0123456789ABCDEF"),
            ("token", "0123"),
        ] {
            let mut pairs = base.to_vec();
            pairs.retain(|(k, _)| *k != key);
            pairs.push((key, bad));
            assert_eq!(query(&pairs), None, "{key}={bad}");
        }
        let mut handshake = base.to_vec();
        handshake.push(("handshake", "1"));
        assert!(query(&handshake).unwrap().handshake);
        handshake.pop();
        handshake.push(("handshake", "2"));
        assert_eq!(query(&handshake), None);
        // A burst needs both values, and a pace that carries the rate.
        let mut burst = base.to_vec();
        burst.push(("fps", "120"));
        assert_eq!(query(&burst), None);
        burst.push(("pacekbps", "99999"));
        assert_eq!(query(&burst), None);
        burst.pop();
        burst.push(("pacekbps", "1000000"));
        assert_eq!(
            query(&burst).unwrap().burst,
            Some(Burst {
                fps: 120,
                pace_kbps: 1_000_000
            })
        );
    }

    #[test]
    fn packets_carry_the_token_and_a_big_endian_sequence() {
        let mut packet = packet(TOKEN, 1392);
        assert_eq!(packet.len(), 1440);
        set_sequence(&mut packet, 0x0102_0304);
        assert_eq!(&packet[..32], TOKEN.as_bytes());
        assert_eq!(&packet[32..36], &[1, 2, 3, 4]);
        assert!(packet[36..].iter().all(|b| *b == 0x5a));
    }

    #[test]
    fn an_even_probe_sends_each_millisecond_its_share() {
        let request = query(&[
            ("kbps", "100000"),
            ("port", "50000"),
            ("packetsize", "1392"),
            ("token", TOKEN),
        ])
        .unwrap();
        // 100 Mbps for 2 s at 1526 wire bytes a packet.
        assert_eq!(expected(100_000, 1392), 16_382);
        let steps = schedule(&request, false);
        assert_eq!(steps.len(), 2000);
        assert_eq!(steps[0], (Duration::ZERO, 8));
        assert_eq!(steps[1999], (Duration::from_millis(1999), 16_382));
        assert!(steps.windows(2).all(|w| w[0].1 <= w[1].1));
    }

    #[test]
    fn a_burst_probe_sends_whole_frames_at_the_pace() {
        let request = query(&[
            ("kbps", "100000"),
            ("port", "50000"),
            ("packetsize", "1392"),
            ("token", TOKEN),
            ("fps", "120"),
            ("pacekbps", "1000000"),
        ])
        .unwrap();
        let steps = schedule(&request, false);
        // 240 frames of about 68 packets, split at the live sender's 64 KiB
        // batch bound. The next group follows its actual wire cost.
        assert_eq!(steps.len(), 480);
        assert_eq!(steps[0].1, 45);
        assert_eq!(steps[1].0, Duration::from_nanos(542_160));
        assert_eq!(steps[2].0, DURATION / 240);
        assert_eq!(steps.last().unwrap().1, expected(100_000, 1392));
        // At 100 Mbps a batch is at most one millisecond of wire traffic.
        let slow = Request {
            burst: Some(Burst {
                fps: 120,
                pace_kbps: 100_000,
            }),
            ..request
        };
        let steps = schedule(&slow, false);
        assert_eq!(steps[0], (Duration::ZERO, 8));
        assert_eq!(steps[1], (Duration::from_nanos(963_840), 16));
        assert_eq!(steps.last().unwrap().1, expected(100_000, 1392));
    }

    #[test]
    fn calibrated_bursts_match_live_batch_bounds_without_losing_high_rate_capacity() {
        for ipv6 in [false, true] {
            for (kbps, pace_kbps, packet_size) in [
                (5_000, 5_000, 256),
                (400_000, 1_000_000, 1392),
                (3_000_000, 9_000_000, 1024),
            ] {
                let request = Request {
                    kbps,
                    port: 50_000,
                    packet_size,
                    token: TOKEN.into(),
                    handshake: false,
                    burst: Some(Burst { fps: 60, pace_kbps }),
                };
                let steps = schedule(&request, ipv6);
                let maximum = crate::network_pacing::pyrowave_batch_limit(
                    u64::from(pace_kbps) * 1000,
                    packet_size + 48,
                    ipv6,
                ) / (packet_size + 48);
                let mut previous = 0;
                for (_, end) in &steps {
                    assert!((*end - previous) as usize <= maximum);
                    previous = *end;
                }
                assert!(steps.windows(2).all(|w| w[0].0 <= w[1].0));
                assert!(steps.last().unwrap().0 < DURATION);
                assert_eq!(previous, expected(kbps, packet_size));
            }
        }
    }
}
