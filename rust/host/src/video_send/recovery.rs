//! Shared pacing decisions for encoder admission and both conventional send paths.
use crate::state::Session;
use butterpollo_core::{config::Config, network_pacing, packet::VideoPacketizer};
use butterpollo_windows::net::Link;
use std::{net::SocketAddr, time::Duration};

pub(crate) struct Pace {
    pub bps: u64,
    pub needed: u64,
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
    let needed = network_pacing::video_wire_rate_bps(packetizer, bitrate, period, peer.is_ipv6());
    // The client's own port, when it says so (Nightjar, for wired links).
    let client_link = u64::from(session.config.pyrowave_client_link_mbps) * 1_000_000;
    let base = network_pacing::rate_bps_for_client(
        config.integer("pacing_max_bitrate_kbps", 0),
        bitrate,
        route.bps,
        route.wireless,
        needed,
        client_link,
    );
    let base = adapt(base, needed);
    let safe = network_pacing::rate_bps(0, bitrate, route.bps, true, needed);
    Pace {
        bps: session.keyframe_loss_rate_bps(base, safe),
        needed,
    }
}

/// Once per stream, when the client's link hint is below the encoder bitrate
/// and the pace is floored at 1.1 times the bitrate instead.
pub(crate) fn warn_if_client_link_below_stream(session: &Session, route: Link) {
    let bitrate = session.bitrate.load(std::sync::atomic::Ordering::Acquire);
    let client_link_mbps = session.config.pyrowave_client_link_mbps;
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
    pub wire_bytes: u64,
    pub pace_bps: u64,
    pub elapsed: Duration,
    pub lost: bool,
}

pub(crate) fn report(session: &Session, config: &Config, frame: RecoveryFrame) {
    if !frame.idr && !frame.after_invalidation {
        return;
    }
    let pacing_reason = session.keyframe_loss_reason().unwrap_or_else(|| {
        if config.integer("pacing_max_bitrate_kbps", 0) > 0 {
            "configured pacing limit"
        } else if config.adaptive_packet_pacing(session.config.codec) {
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
mod tests {
    use super::*;
    use butterpollo_core::session::Role;
    use std::time::Instant;

    #[test]
    fn a_clients_link_hint_caps_the_startup_pace_for_conventional_codecs() {
        let fixture = crate::state::test_support::Fixture::new();
        let packetizer = VideoPacketizer {
            sequence: 0,
            iv_counter: 0,
            frame: 1,
            packet_size: 1392,
            fec_percent: 20,
            min_fec: 2,
            key: Some([0; 16]),
        };
        let period = Duration::from_secs_f64(1. / 60.);
        let peer = "192.0.2.9:12345".parse().unwrap();
        let route = Link {
            bps: 1_000_000_000,
            ..Default::default()
        };
        // A 1 GbE host: stock Moonlight (no hint) and a 1 GbE or 2.5 GbE
        // client stay at 800 Mbps; a 100 Mbps client is paced at 80.
        for (hint, expected) in [
            (0, 800_000_000),
            (1000, 800_000_000),
            (2500, 800_000_000),
            (100, 80_000_000),
        ] {
            let session = Session::new(
                fixture.launch(fixture.client(0), Role::Stream),
                butterpollo_core::rtsp::Negotiated {
                    pyrowave_client_link_mbps: hint,
                    ..Default::default()
                },
            );
            session
                .bitrate
                .store(40_000, std::sync::atomic::Ordering::Release);
            session.configure_keyframe_loss(true);
            let config = Config::default();
            assert_eq!(
                pace(&session, &config, &packetizer, period, peer, route).bps,
                expected,
                "hint {hint}"
            );
            assert_eq!(
                preview_pace(&session, &config, &packetizer, period, peer, route).bps,
                expected,
                "hint {hint}"
            );
        }
    }

    #[test]
    fn a_client_link_hint_below_the_bitrate_floors_the_pace_at_one_point_one_times_it() {
        let fixture = crate::state::test_support::Fixture::new();
        let packetizer = VideoPacketizer {
            sequence: 0,
            iv_counter: 0,
            frame: 1,
            packet_size: 1392,
            fec_percent: 20,
            min_fec: 2,
            key: Some([0; 16]),
        };
        let period = Duration::from_secs_f64(1. / 60.);
        let peer = "192.0.2.9:12345".parse().unwrap();
        let route = Link {
            bps: 1_000_000_000,
            ..Default::default()
        };
        let session = Session::new(
            fixture.launch(fixture.client(0), Role::Stream),
            butterpollo_core::rtsp::Negotiated {
                pyrowave_client_link_mbps: 100,
                ..Default::default()
            },
        );
        session
            .bitrate
            .store(150_000, std::sync::atomic::Ordering::Release);
        session.configure_keyframe_loss(true);
        let config = Config::default();
        assert_eq!(
            pace(&session, &config, &packetizer, period, peer, route).bps,
            165_000_000
        );
        assert_eq!(
            preview_pace(&session, &config, &packetizer, period, peer, route).bps,
            165_000_000
        );
        // A lighter stream is capped by the link as before.
        session
            .bitrate
            .store(50_000, std::sync::atomic::Ordering::Release);
        assert_eq!(
            pace(&session, &config, &packetizer, period, peer, route).bps,
            80_000_000
        );
    }

    #[test]
    fn the_first_picture_and_live_loss_use_the_same_pace_as_sending() {
        let fixture = crate::state::test_support::Fixture::new();
        let session = fixture.session(fixture.client(0), Role::Stream);
        session
            .bitrate
            .store(54_000, std::sync::atomic::Ordering::Release);
        session.configure_keyframe_loss(true);
        let config = Config::default();
        let packetizer = VideoPacketizer {
            sequence: 0,
            iv_counter: 0,
            frame: 1,
            packet_size: 1392,
            fec_percent: 20,
            min_fec: 2,
            key: Some([0; 16]),
        };
        let period = Duration::from_secs_f64(1. / 119.88);
        let peer = "127.0.0.1:12345".parse().unwrap();
        let route = Link {
            bps: 1_000_000_000,
            ..Default::default()
        };
        let fast = preview_pace(&session, &config, &packetizer, period, peer, route);
        assert_eq!(fast.bps, 800_000_000);
        // The first keyframe and the client's requests around it are the
        // stream's start; a keyframe lost after that is what slows it.
        session.keyframe_loss.lock().unwrap().sending_at(
            1,
            true,
            Instant::now()
                - butterpollo_core::keyframe_loss::STARTUP_WINDOW
                - Duration::from_millis(50),
        );
        session.request_recovery_keyframe();
        // Queries while waiting for capture must not consume the decoder's
        // finite keyframe history or count a picture as sent.
        session.keyframe_loss_bps(fast.bps, 108_000_000, 2, true);
        for _ in 0..100 {
            assert_eq!(
                preview_pace(&session, &config, &packetizer, period, peer, route).bps,
                fast.bps
            );
        }
        session.request_recovery_keyframe();
        let slow = preview_pace(&session, &config, &packetizer, period, peer, route);
        assert_eq!(slow.bps, 108_000_000);
        let cap = |bps| network_pacing::recovery_frame_cap(bps, 54_000, period, 20, 1392, false);
        assert!(cap(slow.bps) < cap(fast.bps));
        session.configure_remembered_keyframe_loss(true, true);
        assert_eq!(
            pace(&session, &config, &packetizer, period, peer, route).bps,
            slow.bps
        );
    }
}
