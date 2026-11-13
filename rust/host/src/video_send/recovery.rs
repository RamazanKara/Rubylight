//! Shared pacing decisions for encoder admission and both conventional send paths.
use crate::state::Session;
use butterpollo_core::{config::Config, network_pacing, packet::VideoPacketizer};
use butterpollo_windows::net::Link;
use std::{net::SocketAddr, time::Duration};

pub(crate) struct Pace {
    pub bps: u64,
    pub needed: u64,
}

/// Choose who paces this stream, once at its start. Returns whether
/// keyframe-loss pacing applies. Adaptive packet pacing is opt-in, or automatic
/// for a client that reports a wireless route; the two are mutually exclusive.
pub(crate) fn configure(session: &Session, config: &Config) -> bool {
    // FEC parity, for this stream and the ones after it: GFNI where the CPU has it.
    butterpollo_core::packet::use_gfni(config.boolean("fec_gfni", true));
    tracing::info!(
        gfni = butterpollo_core::packet::gfni_active(),
        "FEC parity path"
    );
    let (codec, wireless) = (session.config.codec, session.config.client_wireless);
    session.configure_automatic_pacing(config.adaptive_packet_pacing(codec, wireless));
    config.keyframe_loss_pacing(codec, wireless)
}

/// This query never records a sent frame. Only the sender advances loss history.
pub(crate) fn pace(
    session: &Session,
    config: &Config,
    packetizer: &VideoPacketizer,
    period: Duration,
    peer: SocketAddr,
    route: Link,
) -> Pace {
    select(
        session,
        config,
        packetizer,
        period,
        peer,
        route,
        |base, needed| session.video_pacing_bps(base, needed),
    )
}

/// Encoder admission previews the next rate without restarting the sender's
/// adaptation window when recovery FEC differs from ordinary-frame FEC.
pub(crate) fn preview_pace(
    session: &Session,
    config: &Config,
    packetizer: &VideoPacketizer,
    period: Duration,
    peer: SocketAddr,
    route: Link,
) -> Pace {
    select(
        session,
        config,
        packetizer,
        period,
        peer,
        route,
        |base, needed| session.preview_video_pacing_bps(base, needed),
    )
}

fn select(
    session: &Session,
    config: &Config,
    packetizer: &VideoPacketizer,
    period: Duration,
    peer: SocketAddr,
    route: Link,
    adapt: impl FnOnce(u64, u64) -> u64,
) -> Pace {
    let bitrate = session.bitrate.load(std::sync::atomic::Ordering::Acquire);
    let needed = network_pacing::video_wire_rate_bps(
        packetizer,
        bitrate,
        period,
        peer.ip().to_canonical().is_ipv6(),
    );
    // The client's own port, when it says so (Nightjar, for wired links).
    let client_link = u64::from(session.client_link_mbps()) * 1_000_000;
    let base = network_pacing::rate_bps_following_client(
        config.integer("pacing_max_bitrate_kbps", 0),
        bitrate,
        route.bps,
        network_pacing::route_is_wireless(
            route.wireless,
            session.client_wireless_sets_route_pacing(),
        ),
        needed,
        client_link,
        config.boolean("pacing_follow_client_link", false),
    );
    let base = adapt(base, needed);
    let safe = network_pacing::rate_bps(0, bitrate, route.bps, true, needed);
    Pace {
        bps: session.keyframe_loss_rate_bps(base, safe),
        needed,
    }
}

/// Whether the route cached for `routed` must be routed again to send to `peer`: none
/// was routed yet, or the client moved (a roam). Records `peer` as routed. Keyed on the
/// address rather than the session's roams, as a picture queued before a roam still
/// goes to the old one.
pub(crate) fn new_route(routed: &mut Option<SocketAddr>, peer: SocketAddr) -> bool {
    let new = *routed != Some(peer);
    *routed = Some(peer);
    new
}

/// Once per stream, when the client's link hint is below the encoder bitrate
/// and the pace is floored at 1.1 times the bitrate instead.
pub(crate) fn warn_if_client_link_below_stream(session: &Session, route: Link) {
    let bitrate = session.bitrate.load(std::sync::atomic::Ordering::Acquire);
    let client_link_mbps = session.client_link_mbps();
    if network_pacing::client_link_below_stream(
        bitrate,
        route.bps,
        u64::from(client_link_mbps) * 1_000_000,
    ) {
        tracing::warn!(
            stream_id = %session.launch.id,
            client_link_mbps,
            encoder_bitrate_kbps = bitrate,
            "the client reports a link slower than the encoder bitrate; pacing at 1.1 times the bitrate, not at the link"
        );
    }
}

/// Metadata belongs to the encoded picture, so a queued picture retains the cap
/// that was active when submitted even if pacing changes before it is sent.
pub(crate) struct RecoveryFrame {
    pub frame: u32,
    pub idr: bool,
    pub after_invalidation: bool,
    pub payload_bytes: usize,
    pub cap_bytes: Option<u64>,
    /// The cap the host asked the encoder for (AMF only).
    pub cap_target_bytes: Option<u64>,
    /// AMF encoded this keyframe again in place of an oversize scene cut.
    pub reencoded: bool,
    /// Too large for full FEC within Moonlight's four blocks.
    pub fec_limited: bool,
    pub wire_bytes: u64,
    pub pace_bps: u64,
    pub elapsed: Duration,
    pub lost: bool,
}

pub(crate) fn report(session: &Session, config: &Config, frame: RecoveryFrame) {
    if !frame.idr && !frame.after_invalidation {
        return;
    }
    if frame.idr && frame.fec_limited {
        session
            .stats
            .keyframes_past_fec
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    let pacing_reason = session.keyframe_loss_reason().unwrap_or_else(|| {
        if config.integer("pacing_max_bitrate_kbps", 0) > 0 {
            "configured pacing limit"
        } else if config
            .adaptive_packet_pacing(session.config.codec, session.config.client_wireless)
        {
            "adaptive packet pacing"
        } else if config.boolean("host_adaptive_bitrate", false) {
            "adaptive bitrate pacing"
        } else {
            "route pacing"
        }
    });
    tracing::info!(
        stream_id = %session.launch.id,
        frame = frame.frame,
        recovery_kind = if frame.idr { "keyframe" } else { "reference invalidation" },
        payload_bytes = frame.payload_bytes,
        cap_bytes = frame.cap_bytes,
        cap_target_bytes = frame.cap_target_bytes,
        reencoded = frame.reencoded,
        fec_limited = frame.fec_limited,
        pacing_bps = frame.pace_bps,
        wire_bytes = frame.wire_bytes,
        ideal_wire_ms = frame.wire_bytes as f64 * 8000. / frame.pace_bps.max(1) as f64,
        sender_work_ms = frame.elapsed.as_secs_f64() * 1000.,
        pacing_reason,
        lost_on_send = frame.lost,
        idr_requests = session.stats.idr_requests.load(std::sync::atomic::Ordering::Relaxed),
        send_loss_recoveries = session.stats.send_loss_recoveries.load(std::sync::atomic::Ordering::Relaxed),
        "video recovery frame"
    );
}

#[cfg(test)]
mod tests;
