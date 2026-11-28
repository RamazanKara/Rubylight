//! Shared pacing decisions for encoder admission and both conventional send paths.
use crate::state::Session;
use butterpollo_config::config::Config;
use butterpollo_media_policy::{
    network_pacing, packet::VideoPacketizer, stream_policy::SendLossRecovery,
};
use butterpollo_win_io::net::{BatchDropDelta, Link};
use std::{
    net::SocketAddr,
    time::{Duration, Instant},
};

pub(crate) const SEND_CONGESTION_WARNING: &str = "The PC could not send the video fast enough (its CPU or network adapter fell behind), so video packets were dropped. You may see stutter or recovery frames; close other work on the PC, or lower the bitrate, resolution or frame rate. The log includes the socket error code.";
pub(crate) const ROUTE_UNAVAILABLE_WARNING: &str = "The network route to this device is unavailable, so video packets could not be sent. Streaming will resume once the network connection returns. The log includes the socket error code.";

/// Handle a video block or batch that could not recover within FEC. When every drop in the
/// batch happened because the route to the client is down, suppress keyframe storms and remember
/// the outage until a frame gets through again.
pub(crate) fn on_block_unrecoverable(
    session: &Session,
    send_loss: &mut SendLossRecovery,
    now: Instant,
    period: Duration,
    reached_socket: bool,
    non_route_unrecoverable: bool,
    batch_drops: BatchDropDelta,
) {
    if !non_route_unrecoverable && batch_drops.route_unavailable > 0 {
        send_loss.route_unavailable();
    } else if send_loss.lost(now, period, reached_socket) {
        session.request_send_loss_recovery();
    }
}

#[derive(Clone, Copy)]
pub(crate) struct FrameDrop {
    pub wire_frame: u32,
    pub idr: bool,
    pub non_route_unrecoverable: bool,
    pub reached_socket: bool,
    pub abandoned: usize,
    pub drops: BatchDropDelta,
}

/// Account for frame-level send drops at the end of a frame: avoid keyframe-loss pacing latches
/// and congestion warnings when drops came only from an unreachable network route, and request
/// one recovery keyframe once a non-keyframe delivers after the route returns.
pub(crate) fn finish_frame_drops(
    session: &Session,
    send_loss: &mut SendLossRecovery,
    frame: FrameDrop,
) {
    let FrameDrop {
        wire_frame,
        idr,
        non_route_unrecoverable,
        reached_socket,
        abandoned,
        drops,
    } = frame;
    let unrecoverable = non_route_unrecoverable || drops.route_unavailable > 0;
    if unrecoverable {
        if !non_route_unrecoverable {
            session.keyframe_abandoned_route(wire_frame, reached_socket);
            send_loss.route_unavailable();
            tracing::debug!(
                frame = wire_frame,
                idr,
                abandoned,
                "video frame dropped while network route is unavailable"
            );
        } else {
            session.keyframe_abandoned_as(wire_frame, drops.refused > 0);
            tracing::debug!(
                frame = wire_frame,
                idr,
                abandoned,
                "video frame lost on send; the rest of it was not sent"
            );
        }
    } else if drops.route_unavailable == 0 && send_loss.delivered(idr) {
        session.request_host_keyframe();
    }
    if drops.refused > 0 && (non_route_unrecoverable || drops.route_unavailable == 0) {
        session.launch.warnings.event(
            "network_send",
            SEND_CONGESTION_WARNING,
            butterpollo_session::session::EVENT_PERIOD,
        );
    } else if drops.route_unavailable > 0 {
        session.launch.warnings.event(
            "network_send",
            ROUTE_UNAVAILABLE_WARNING,
            butterpollo_session::session::EVENT_PERIOD,
        );
    }
}

/// Classify an incomplete streamed frame abandoned before its last part finishes:
/// route-unavailable-only drops stay out of host send-loss counters and latches.
pub(crate) fn abandon_incomplete_streamed(
    session: &Session,
    send_loss: &mut SendLossRecovery,
    wire_frame: u32,
    non_route_unrecoverable: bool,
    reached_socket: bool,
    drops: BatchDropDelta,
) {
    if !non_route_unrecoverable && drops.route_unavailable > 0 {
        session.keyframe_abandoned_route(wire_frame, reached_socket);
        send_loss.route_unavailable();
    } else {
        session.keyframe_abandoned_as(wire_frame, false);
    }
}

/// Include the next batch's due time and all remaining bytes. Counting even
/// the final batch's wire interval deliberately opens admission a little late.
pub(crate) fn recovery_remaining(
    now: Instant,
    due: Instant,
    wire_bytes: usize,
    bps: u64,
) -> Duration {
    due.saturating_duration_since(now)
        + Duration::from_secs_f64(wire_bytes as f64 * 8. / bps.max(1) as f64)
}

pub(crate) struct Pace {
    pub bps: u64,
    pub needed: u64,
}

/// Choose who paces this stream, once at its start. Returns whether
/// keyframe-loss pacing applies. Adaptive packet pacing is opt-in, or automatic
/// for a client that reports a wireless route; the two are mutually exclusive.
pub(crate) fn configure(session: &Session, config: &Config) -> bool {
    // FEC parity, for this stream and the ones after it: GFNI where the CPU has it.
    butterpollo_media_policy::packet::use_gfni(config.boolean("fec_gfni", true));
    tracing::info!(
        gfni = butterpollo_media_policy::packet::gfni_active(),
        "FEC parity path"
    );
    let (codec, wireless) = (session.config.codec, session.config.client_wireless);
    session.configure_automatic_pacing(config.adaptive_packet_pacing(codec, wireless));
    session.configure_adaptive_pacing_on_loss(config.adaptive_pacing_after_loss(codec, wireless));
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
    // Use the session's steady-state FEC percentage instead of the per-frame
    // mutated `packetizer.fec_percent` (recovery IDRs / statistical FEC) so
    // per-frame parity swings do not churn `AdaptivePacing`'s wire floor.
    let steady = VideoPacketizer {
        sequence: packetizer.sequence,
        iv_counter: packetizer.iv_counter,
        frame: packetizer.frame,
        packet_size: packetizer.packet_size,
        fec_percent: session.fec_percent(),
        min_fec: packetizer.min_fec,
        key: packetizer.key,
    };
    select(
        session,
        config,
        &steady,
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
        config.manual_pacing_kbps().unwrap_or(0),
        bitrate,
        route.bps,
        network_pacing::route_is_wireless(
            route.wireless,
            session.client_wireless_sets_route_pacing(),
        ),
        needed,
        client_link,
        config.boolean(
            "pacing_follow_client_link",
            network_pacing::follow_client_link_default(false),
        ),
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

pub(crate) fn pacing_reason(session: &Session, config: &Config) -> &'static str {
    if session.automatic_pacing_enabled() && session.keyframe_loss_active() {
        "adaptive packet pacing after video loss"
    } else {
        session.keyframe_loss_reason().unwrap_or_else(|| {
            if config.manual_pacing_kbps().is_some() {
                "configured pacing limit"
            } else if config
                .adaptive_packet_pacing(session.config.codec, session.config.client_wireless)
            {
                "adaptive packet pacing"
            } else {
                "route pacing"
            }
        })
    }
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
    let pacing_reason = pacing_reason(session, config);
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
