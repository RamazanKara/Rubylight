//! Bound packet bursts without accumulating a catch-up burst after a late send.
use std::time::{Duration, Instant};

/// Whether to pace as a Wi-Fi route: the host's own route is wireless, or the
/// client says its route to the host is (`x-rl-video[0].clientWireless:1`).
/// The host cannot see a client behind an access point; absent means unknown.
pub fn route_is_wireless(host_route_wireless: bool, client_wireless: bool) -> bool {
    host_route_wireless || client_wireless
}

/// The route ceiling, before startup and loss-responsive pacing. Wired/unknown
/// routes allow up to 800 Mbps. Confirmed Wi-Fi retains twice the
/// encoder bitrate, allowing at least 25% headroom above packetized demand.
/// Explicit limits retain the stream-bitrate floor, subject to the physical link.
pub fn rate_bps(
    configured_kbps: i64,
    stream_kbps: u32,
    link_bps: u64,
    wireless: bool,
    needed_bps: u64,
) -> u64 {
    rate_bps_for_client(
        configured_kbps,
        stream_kbps,
        link_bps,
        wireless,
        needed_bps,
        0,
    )
}

/// [`rate_bps`] with the client's own link (`x-ss-video[0].pyrowaveLinkMbps`,
/// zero when it reports none, as stock Moonlight does). The cap is 4/5 of the
/// slower of the two NICs: a 1 GbE client stays at 800 Mbps, a 100 Mbps
/// client gets 80, so its first keyframe is not sent faster than its port.
pub fn rate_bps_for_client(
    configured_kbps: i64,
    stream_kbps: u32,
    link_bps: u64,
    wireless: bool,
    needed_bps: u64,
    client_link_bps: u64,
) -> u64 {
    let host_link_bps = link_bps;
    let link_bps = match (link_bps, client_link_bps) {
        (0, client) => client,
        (host, 0) => host,
        (host, client) => host.min(client),
    };
    let requested = if configured_kbps > 0 {
        (configured_kbps as u64)
            .saturating_mul(1000)
            .max(u64::from(stream_kbps) * 1100)
    } else if wireless {
        (u64::from(stream_kbps) * 2000)
            .max(needed_bps.saturating_add(needed_bps / 4))
            .clamp(1_000_000, 800_000_000)
    } else {
        800_000_000
    };
    if link_bps > 0 {
        let cap = link_bps.saturating_mul(4) / 5;
        let floor = client_floor_bps(stream_kbps, host_link_bps, client_link_bps);
        requested.min(cap.max(floor)).max(1)
    } else {
        requested
    }
}

/// A client that says its link is slower than the stream still has to get the
/// stream: pacing never falls below 1.1 times the encoder bitrate because of
/// the client's hint (the host's own link keeps its plain cap). Zero when the
/// hint is not what limits the pace.
pub fn client_floor_bps(stream_kbps: u32, host_link_bps: u64, client_link_bps: u64) -> u64 {
    let client_limits =
        client_link_bps > 0 && (host_link_bps == 0 || client_link_bps < host_link_bps);
    if client_limits {
        u64::from(stream_kbps) * 1100
    } else {
        0
    }
}

/// The client's hint is below what the encoder sends, so the floor decides.
pub fn client_link_below_stream(
    stream_kbps: u32,
    host_link_bps: u64,
    client_link_bps: u64,
) -> bool {
    client_floor_bps(stream_kbps, host_link_bps, client_link_bps)
        > client_link_bps.saturating_mul(4) / 5
}

/// What the client tells the host about its own path with each PyroWave
/// stream (`x-ss-video[0].pyrowaveLinkMbps` and `pyrowavePaceMbps`, as Nonary's
/// Moonlight and Nightjar send them).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClientPath {
    /// The client's wired link, which it reports with each PyroWave stream.
    pub link_bps: u64,
    /// The pace the client's calibration found loss-free.
    pub pace_bps: u64,
}

/// A measured client pace, either endpoint's link and an explicit cap are
/// ceilings, even when the requested stream cannot fit. A wired host alone
/// says nothing about the client's path: unmeasured and wireless paths start
/// 25% above wire demand instead of bursting at the host NIC's line rate.
pub fn pyrowave_rate_bps(
    configured_kbps: i64,
    stream_kbps: u32,
    link_bps: u64,
    demand_bps: u64,
    client: ClientPath,
) -> u64 {
    pyrowave_route_rate_bps(
        configured_kbps,
        stream_kbps,
        link_bps,
        false,
        demand_bps,
        client,
    )
}

pub fn pyrowave_route_rate_bps(
    configured_kbps: i64,
    stream_kbps: u32,
    link_bps: u64,
    wireless: bool,
    demand_bps: u64,
    client: ClientPath,
) -> u64 {
    let demand = {
        let needed = demand_bps.max(u64::from(stream_kbps) * 1100);
        (needed.saturating_mul(5) / 4).max(1_000_000)
    };
    let requested = if configured_kbps > 0 {
        (configured_kbps as u64).saturating_mul(1000)
    } else if client.pace_bps > 0 {
        client.pace_bps
    } else if client.link_bps > 0 && !wireless {
        client.link_bps.saturating_mul(95) / 100
    } else {
        demand
    };
    let mut rate = requested.max(1);
    for ceiling in [
        link_bps.saturating_mul(95) / 100,
        client.link_bps.saturating_mul(95) / 100,
        client.pace_bps,
    ] {
        if ceiling > 0 {
            rate = rate.min(ceiling);
        }
    }
    rate
}

/// PyroWave groups used by both calibration and streaming: at most one
/// millisecond on the wire, capped at one segmented UDP send.
pub fn pyrowave_batch_limit(bps: u64, packet: usize, ipv6: bool) -> usize {
    batch_payload_limit(bps, packet, ipv6, 65_507.min(packet.saturating_mul(64)))
}

/// Stable wire allowance for adaptation. Records include their headers and
/// critical parity in the encoder budget. Legacy containers can add one
/// four-byte length per eight-byte record, plus transport framing. Charge
/// IPv6/encryption overhead conservatively for both address families.
pub fn pyrowave_wire_rate_bps(
    kbps: u32,
    packet_size: usize,
    records: bool,
    period: Duration,
) -> u64 {
    let packet = packet_size.saturating_add(crate::pyrowave::WIRE_OVERHEAD) as f64;
    let minimum = (packet * 8. / period.as_secs_f64().max(0.000_001)).ceil() as u64;
    if records {
        return (u64::from(kbps) * 1000).max(minimum);
    }
    let payload = packet_size.saturating_sub(16).max(1) as f64;
    let bytes = (f64::from(kbps) * 125. * period.as_secs_f64()).ceil();
    // The legacy container's packet count adds four bytes before the
    // transport's eight-byte frame header. Both can cross a shard boundary.
    let packets = ((bytes * 1.5 + 12.) / payload).ceil();
    ((packets * packet * 8. / period.as_secs_f64().max(0.000_001)).ceil() as u64).max(minimum)
}

/// Whether an average packetized frame occupies at least a quarter-period
/// on the wire. Use wire demand, since FEC/minimum parity can dominate small
/// frames. Such streams need the dedicated sender to keep capture moving.
pub fn paced(bps: u64, needed_bps: u64) -> bool {
    bps <= needed_bps.saturating_mul(4)
}

pub fn report_rate(
    warnings: &crate::session::Warnings,
    bps: u64,
    needed_bps: u64,
    configured_kbps: i64,
) {
    if configured_kbps > 0 && bps > (configured_kbps as u64).saturating_mul(1000) {
        warnings.set("network_pacing_floor", format!("Requested pacing cap {configured_kbps} Kbps was raised to {:.1} Mbps by the encoder-bitrate floor. Bursts can exceed the requested cap; lower the encoder bitrate or choose a pacing cap with room for audio and FEC.", bps as f64 / 1_000_000.));
    } else {
        warnings.clear("network_pacing_floor");
    }
    if bps < needed_bps {
        warnings.set("network_pacing", format!("Network pacing is limited to {:.1} Mbps by the configured cap, measured client path or routed link, below the stream's wire budget. Sending may delay or replace frames; lower client bitrate/FEC or raise the pacing cap only if the network has headroom.", bps as f64 / 1_000_000.));
    } else {
        warnings.clear("network_pacing");
    }
}

/// Ethernet framing, inter-packet gap, IP and UDP, beyond the UDP payload.
pub fn overhead(ipv6: bool) -> usize {
    if ipv6 { 86 } else { 66 }
}

/// At most one millisecond of wire traffic per conventional-video batch,
/// further bounded by the user's payload limit. Always allow one datagram.
/// Small batches protect downstream queues even when the host NIC is fast.
pub fn batch_payload_limit(bps: u64, datagram: usize, ipv6: bool, maximum: usize) -> usize {
    let wire_packet = datagram.saturating_add(overhead(ipv6)) as u64;
    let packets = (bps / 8000 / wire_packet).max(1);
    packets
        .saturating_mul(datagram as u64)
        .min(maximum as u64)
        .max(datagram as u64) as usize
}

pub fn video_wire_rate_bps(
    packetizer: &crate::packet::VideoPacketizer,
    stream_kbps: u32,
    period: Duration,
    ipv6: bool,
) -> u64 {
    let payload = (f64::from(stream_kbps) * 125. * period.as_secs_f64()).ceil() as usize;
    video_frame_wire_rate_bps(packetizer, payload, period, ipv6)
}

/// Wire demand for one encoded frame per period. Sender selection must also
/// account for a large individual frame, even when the average fits inline.
pub fn video_frame_wire_rate_bps(
    packetizer: &crate::packet::VideoPacketizer,
    payload: usize,
    period: Duration,
    ipv6: bool,
) -> u64 {
    if payload == 0 {
        return 0;
    }
    let packets: usize = packetizer
        .block_layout(payload)
        .iter()
        .map(|(data, fec)| data + fec)
        .sum();
    let datagram = packetizer.packet_size + 16 + if packetizer.key.is_some() { 32 } else { 0 };
    (packets as f64 * (datagram + overhead(ipv6)) as f64 * 8. / period.as_secs_f64()).ceil() as u64
}

/// Encoded payload that fits roughly two frame periods at the current pace,
/// bounded to 2–8 nominal frames. The lower bound protects recovery quality
/// when the link cannot carry even that budget; it is not a latency guarantee.
///
/// Include parity and all packet headers, conservatively reserving encryption
/// overhead even for an unencrypted stream. Packet/FEC rounding and minimum
/// parity can add wire time, especially for tiny frames. The sender still
/// accounts for the actual packetized frame when pacing it.
pub fn recovery_frame_cap(
    pace_bps: u64,
    bitrate_kbps: u32,
    period: Duration,
    fec_percent: usize,
    packet_size: usize,
    ipv6: bool,
) -> u64 {
    let average = f64::from(bitrate_kbps) * 125. * period.as_secs_f64();
    let minimum = two_frames(bitrate_kbps, period);
    let maximum = ((average * 8.).floor() as u64).max(minimum);
    // Conventional packet_size includes the video headers but excludes the
    // extra 16-byte RTP allowance and optional 32-byte encryption wrapper.
    let payload = packet_size.saturating_sub(16).max(1) as f64;
    let wire = packet_size.saturating_add(48 + overhead(ipv6)) as f64;
    let expansion = wire / payload * (1. + fec_percent.min(100) as f64 / 100.);
    let budget = pace_bps as f64 / 8. * period.as_secs_f64() * 2. / expansion;
    (budget.floor() as u64).clamp(minimum, maximum)
}

/// [`recovery_frame_cap`]'s `cap` held to `fec_whole`, the largest frame that
/// keeps its full parity within Moonlight's four FEC blocks
/// ([`crate::packet::VideoPacketizer::fec_whole_payload_limit`]). A larger
/// recovery keyframe leaves with less parity or none, so one lost packet on a
/// clean route costs it, and losing it again latches the slow pace. Where two
/// nominal frames, `recovery_frame_cap`'s floor, already pass `fec_whole`, no
/// cap at the floor keeps full parity, so `cap` stays as it is.
pub fn fec_whole_frame_cap(cap: u64, fec_whole: usize, bitrate_kbps: u32, period: Duration) -> u64 {
    fec_whole_frame_limit(fec_whole, bitrate_kbps, period).map_or(cap, |limit| cap.min(limit))
}

/// The bound [`fec_whole_frame_cap`] holds a cap to: `fec_whole`, or none
/// where two nominal frames already pass it. An encoded frame above it loses
/// parity, so the encoder re-encodes an inter frame that large as a keyframe;
/// with no bound, a keyframe at the cap would lose it as well, so none is.
pub fn fec_whole_frame_limit(fec_whole: usize, bitrate_kbps: u32, period: Duration) -> Option<u64> {
    (two_frames(bitrate_kbps, period) <= fec_whole as u64).then_some(fec_whole as u64)
}

/// Two nominal frames at `bitrate_kbps`: the least [`recovery_frame_cap`]
/// gives, and the least the keyframe cap scale lowers a cap to.
pub fn two_frames(bitrate_kbps: u32, period: Duration) -> u64 {
    (f64::from(bitrate_kbps) * 125. * period.as_secs_f64() * 2.).ceil() as u64
}

/// Time spent in the send call or waking late that still counts toward the
/// batch's interval. It covers a typical 64 KB send (about 0.2 ms) plus timer
/// lateness; a longer stall or idle gap earns no more than this.
pub const SEND_SLACK: Duration = Duration::from_micros(500);

pub struct Pacer {
    due: Instant,
}
impl Pacer {
    pub fn new(now: Instant) -> Self {
        Self { due: now }
    }
    pub fn due(&self) -> Instant {
        self.due
    }
    /// Called immediately after a batch is sent. Up to `SEND_SLACK` of send
    /// time and timer overshoot consumes the interval; beyond that, a stall or
    /// idle gap earns no catch-up credit, so the next batch waits at least the
    /// interval less `SEND_SLACK`.
    /// Account for bytes on the wire, including one header per datagram.
    pub fn sent(
        &mut self,
        completed: Instant,
        payload: usize,
        packets: usize,
        ipv6: bool,
        bps: u64,
    ) {
        let bytes = payload.saturating_add(packets.saturating_mul(overhead(ipv6)));
        let interval = Duration::from_secs_f64(bytes as f64 * 8. / bps.max(1) as f64);
        self.due = (self.due + interval).max(completed + interval.saturating_sub(SEND_SLACK));
    }
}

#[cfg(test)]
mod tests;
