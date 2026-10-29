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
    let needed = network_pacing::video_wire_rate_bps(packetizer, bitrate, period, peer.is_ipv6());
    // The client's own port, when it says so (Nightjar, for wired links).
    let client_link = u64::from(session.client_link_mbps()) * 1_000_000;
    let base = network_pacing::rate_bps_for_client(
        config.integer("pacing_max_bitrate_kbps", 0),
        bitrate,
        route.bps,
        network_pacing::route_is_wireless(
            route.wireless,
            session.client_wireless_sets_route_pacing(),
        ),
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
mod tests {
    use super::*;
    use butterpollo_core::session::Role;
    use std::time::Instant;

    /// #59: a keyframe's line says whether AMF re-encoded it, the cap the host
    /// asked for and whether it lost FEC; keyframes past FEC are counted.
    #[test]
    fn recovery_keyframes_count_the_ones_past_fec() {
        let fixture = crate::state::test_support::Fixture::new();
        let session = Session::new(
            fixture.launch(fixture.client(0), Role::Stream),
            butterpollo_core::rtsp::Negotiated::default(),
        );
        let config = Config::default();
        let frame = |idr: bool, fec_limited: bool| RecoveryFrame {
            frame: 1001,
            idr,
            after_invalidation: !idr,
            payload_bytes: 1_908_217,
            cap_bytes: Some(1_166_893),
            cap_target_bytes: Some(1_166_893),
            reencoded: true,
            fec_limited,
            wire_bytes: 1_960_000,
            pace_bps: 800_000_000,
            elapsed: Duration::from_millis(20),
            lost: false,
        };
        let past_fec = || {
            session
                .stats
                .keyframes_past_fec
                .load(std::sync::atomic::Ordering::Relaxed)
        };
        report(&session, &config, frame(true, true));
        assert_eq!(past_fec(), 1);
        report(&session, &config, frame(true, false));
        report(&session, &config, frame(false, true));
        assert_eq!(past_fec(), 1);
        assert_eq!(session.info()["keyframes_past_fec"], 1);
    }

    /// A roam routes the first picture sent to the new address, even when a picture
    /// queued before it went to the old one after the roam.
    #[test]
    fn a_route_follows_the_address_it_sends_to() {
        let (old, new): (SocketAddr, SocketAddr) = (
            "192.0.2.1:9".parse().unwrap(),
            "198.51.100.4:9".parse().unwrap(),
        );
        let mut routed = None;
        assert!(new_route(&mut routed, old), "the first picture");
        assert!(!new_route(&mut routed, old));
        // The session roamed, but this picture was already queued for `old`.
        assert!(!new_route(&mut routed, old));
        assert!(
            new_route(&mut routed, new),
            "the first picture on the new path"
        );
        assert!(!new_route(&mut routed, new));
        assert!(new_route(&mut routed, old), "a roam back");
    }

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
            // The hint was about the path a roaming client left.
            session.roamed("198.51.100.4".parse().unwrap(), Instant::now());
            assert_eq!(
                pace(&session, &config, &packetizer, period, peer, route).bps,
                800_000_000,
                "hint {hint} after a roam"
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

    /// A 40 Mbps stream on a wired 1 GbE host route, as a client that does or
    /// does not report a wireless route. `configure` is what the stream start
    /// calls; it decides whether the adaptive controller owns the stream.
    fn wireless_hint_stream(
        fixture: &crate::state::test_support::Fixture,
        settings: &str,
        codec: u8,
        client_wireless: bool,
    ) -> (std::sync::Arc<Session>, Config, bool) {
        let session = Session::new(
            fixture.launch(fixture.client(0), Role::Stream),
            butterpollo_core::rtsp::Negotiated {
                codec,
                client_wireless,
                ..Default::default()
            },
        );
        session
            .bitrate
            .store(40_000, std::sync::atomic::Ordering::Release);
        let config = Config::parse(settings).unwrap();
        let keyframe_loss = configure(&session, &config);
        session.configure_keyframe_loss(keyframe_loss);
        (session, config, keyframe_loss)
    }

    #[test]
    fn the_wireless_hint_sets_the_route_pace_only_when_no_controller_owns_the_stream() {
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
            wireless: false,
            ..Default::default()
        };
        let needed = network_pacing::video_wire_rate_bps(&packetizer, 40_000, period, false);
        let wifi = network_pacing::rate_bps(0, 40_000, route.bps, true, needed);
        assert_eq!(wifi, 80_000_000);
        // No controller: the hint paces like a Wi-Fi host, without it 800 Mbps.
        for (settings, codec) in [
            ("wireless_adaptive_pacing=false", 1),
            ("", 3),
            (
                "pacing_max_bitrate_kbps=0\nwireless_adaptive_pacing=false",
                0,
            ),
            ("host_adaptive_bitrate=true", 2),
        ] {
            for (hint, expected) in [(true, wifi), (false, 800_000_000)] {
                let (session, config, _) = wireless_hint_stream(&fixture, settings, codec, hint);
                assert!(!session.automatic_pacing_enabled(), "{settings}");
                let what = format!("{settings} codec {codec} hint {hint}");
                assert_eq!(
                    pace(&session, &config, &packetizer, period, peer, route).bps,
                    expected,
                    "{what}"
                );
                assert_eq!(
                    preview_pace(&session, &config, &packetizer, period, peer, route).bps,
                    expected,
                    "{what}"
                );
            }
        }
        // A manual limit keeps its own policy with or without the hint.
        let (session, config, _) =
            wireless_hint_stream(&fixture, "pacing_max_bitrate_kbps=300000", 1, true);
        assert_eq!(
            pace(&session, &config, &packetizer, period, peer, route).bps,
            300_000_000
        );
        // The controller owns it: base and ceiling stay the host route's
        // 800 Mbps and the start is the controller's seed, exactly as with
        // `host_adaptive_pacing` on a client that reports nothing.
        let (owned, owned_config, owned_keyframe_loss) =
            wireless_hint_stream(&fixture, "", 1, true);
        let (host_opt_in, host_config, _) =
            wireless_hint_stream(&fixture, "host_adaptive_pacing=true", 1, false);
        assert!(owned.automatic_pacing_enabled() && host_opt_in.automatic_pacing_enabled());
        assert!(!owned.client_wireless_sets_route_pacing());
        assert!(!owned_keyframe_loss);
        let seed = pace(&owned, &owned_config, &packetizer, period, peer, route);
        assert_eq!(seed.bps, 80_000_000);
        assert_eq!(
            seed.bps,
            pace(&host_opt_in, &host_config, &packetizer, period, peer, route).bps
        );
        assert_eq!(
            preview_pace(&owned, &owned_config, &packetizer, period, peer, route).bps,
            seed.bps
        );
        assert_eq!(
            owned.automatic_pacing.lock().unwrap().snapshot()["ceiling_kbps"],
            800_000
        );
        // The encoder's recovery-frame cap is what `host_adaptive_pacing`
        // streams get, not the smaller one of a Wi-Fi host.
        let cap = |s: &Session, c: &Config| {
            let pace = preview_pace(s, c, &packetizer, period, peer, route);
            network_pacing::recovery_frame_cap(pace.bps, 40_000, period, 20, 1392, false)
        };
        assert_eq!(cap(&owned, &owned_config), cap(&host_opt_in, &host_config));
        // PyroWave is not owned by the controller unless it is opted in.
        let (pyrowave, _, _) = wireless_hint_stream(&fixture, "", 3, true);
        assert!(pyrowave.client_wireless_sets_route_pacing());
        let (pyrowave, _, _) = wireless_hint_stream(&fixture, "host_adaptive_pacing=true", 3, true);
        assert!(!pyrowave.client_wireless_sets_route_pacing());
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
