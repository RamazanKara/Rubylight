//! Stream sessions: launches, live sessions and their statistics, and the
//! user-facing warnings a session raises.
use crate::{rtsp::Negotiated, state::Client};
use anyhow::{Result, bail};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    net::IpAddr,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
#[derive(Clone, Copy, Serialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Stream,
    RemoteMonitor,
    InputOnly,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Warning {
    pub code: String,
    pub message: String,
}

/// How long a dropped packet, frame or input stays on the stream card after
/// it last happened.
pub const EVENT_PERIOD: Duration = Duration::from_secs(30);

/// Current warnings by condition code, each with an optional expiry.
type WarningEntries = BTreeMap<String, (String, Option<Instant>)>;

#[derive(Default)]
pub struct Warnings {
    entries: std::sync::Mutex<WarningEntries>,
    /// A bit per present code's hash: clearing an absent code, as per-frame
    /// recovery paths do on a healthy stream, takes no lock.
    present: AtomicU64,
}
fn warning_bit(code: &str) -> u64 {
    let hash = code.bytes().fold(0xcbf2_9ce4_8422_2325u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    });
    1 << (hash % 64)
}
fn warning_bits(entries: &WarningEntries) -> u64 {
    entries
        .keys()
        .fold(0, |bits, code| bits | warning_bit(code))
}
impl Warnings {
    /// Keep one current warning per condition, without flooding a retry loop.
    pub fn set(&self, code: &str, message: impl AsRef<str>) {
        self.insert(code, message.as_ref(), None);
    }
    /// A recurring event rather than a lasting condition: shown until
    /// `period` passes without it, and logged again only after that.
    pub fn event(&self, code: &str, message: impl AsRef<str>, period: Duration) {
        self.insert(code, message.as_ref(), Some(Instant::now() + period));
    }
    fn insert(&self, code: &str, message: &str, expires: Option<Instant>) {
        let now = Instant::now();
        let mut entries = self.entries.lock().unwrap();
        match entries.get_mut(code) {
            Some((current, until)) if current == message && until.is_none_or(|at| at > now) => {
                *until = expires;
            }
            _ => {
                tracing::warn!(code, "{message}");
                entries.insert(code.into(), (message.into(), expires));
                self.present.fetch_or(warning_bit(code), Ordering::Release);
            }
        }
    }
    pub fn clear(&self, code: &str) {
        if self.present.load(Ordering::Acquire) & warning_bit(code) == 0 {
            return;
        }
        let mut entries = self.entries.lock().unwrap();
        if entries.remove(code).is_some() {
            self.present
                .store(warning_bits(&entries), Ordering::Release);
        }
    }
    pub fn snapshot(&self) -> Vec<Warning> {
        let now = Instant::now();
        let mut entries = self.entries.lock().unwrap();
        let before = entries.len();
        entries.retain(|_, (_, until)| until.is_none_or(|at| at > now));
        if entries.len() != before {
            self.present
                .store(warning_bits(&entries), Ordering::Release);
        }
        entries
            .iter()
            .map(|(code, (message, _))| Warning {
                code: code.clone(),
                message: message.clone(),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn events_expire_and_clearing_absent_codes_skips_the_lock() {
        let warnings = Warnings::default();
        warnings.clear("encoder_recovery");
        assert_eq!(warnings.present.load(Ordering::Relaxed), 0);
        warnings.set("display_virtual", "Physical display in use");
        warnings.event("network_send", "Packets dropped", Duration::ZERO);
        // An expired event disappears; a lasting condition stays.
        assert_eq!(
            warnings
                .snapshot()
                .iter()
                .map(|w| w.code.as_str())
                .collect::<Vec<_>>(),
            ["display_virtual"]
        );
        assert_eq!(
            warnings.present.load(Ordering::Relaxed),
            warning_bit("display_virtual")
        );
        warnings.event("network_send", "Packets dropped", Duration::from_secs(60));
        assert_eq!(warnings.snapshot().len(), 2);
        warnings.clear("display_virtual");
        warnings.clear("network_send");
        assert!(warnings.snapshot().is_empty());
        assert_eq!(warnings.present.load(Ordering::Relaxed), 0);
    }
    #[test]
    fn warnings_survive_preparation_and_clear_after_recovery() {
        let launch = launch("warnings", Role::Stream);
        launch.warnings.set("display", "Physical display in use");
        let session = Session::new(launch.clone(), Negotiated::default());
        *session.encoder.write().unwrap() = "amf".into();
        let capture = Arc::new(Warnings::default());
        *session.capture_warnings.write().unwrap() = capture.clone();
        capture.set("capture", "Capture interrupted");
        launch.warnings.set("display", "Physical display in use");
        assert_eq!(session.info()["encoder"], "amf");
        assert_eq!(
            session.info()["warnings"],
            serde_json::json!([
                {"code":"display", "message":"Physical display in use"},
                {"code":"capture", "message":"Capture interrupted"}
            ])
        );
        capture.set("capture", "Capture recovering");
        assert_eq!(
            session.info()["warnings"][1]["message"],
            "Capture recovering"
        );
        capture.clear("capture");
        launch.warnings.clear("display");
        assert_eq!(session.info()["warnings"], serde_json::json!([]));
        let other = Session::new(
            super::tests::launch("other", Role::Stream),
            Negotiated::default(),
        );
        launch.warnings.set("audio", "Audio interrupted");
        assert_eq!(other.info()["warnings"], serde_json::json!([]));
    }
    #[test]
    fn client_fec_is_exposed_as_telemetry_without_changing_stream_decisions() {
        let session = Session::new(launch("fec", Role::Stream), Negotiated::default());
        session.idr.store(false, Ordering::Relaxed);
        session.stats.video_frame.store(8, Ordering::Release);
        let bitrate = session.bitrate.load(Ordering::Relaxed);
        let report = [
            0, 0, 0, 7, 0, 11, 0, 0, 0, 1, 0, 10, 0, 2, 0, 9, 0, 1, 20, 0, 1,
        ];
        session.record_fec_status(&report);
        session.record_fec_status(&report[..20]);
        let info = session.info();
        assert_eq!(info["performance"]["fec_reports"], 2);
        assert_eq!(info["performance"]["fec_recovered_frames"], 1);
        assert_eq!(info["performance"]["fec_invalid_reports"], 1);
        assert_eq!(session.bitrate.load(Ordering::Relaxed), bitrate);
        assert!(!session.idr.load(Ordering::Relaxed));
        assert!(session.invalidation.lock().unwrap().is_none());
        assert_eq!(info["idr_requests"], 0);
        assert_eq!(info["reference_invalidations"], 0);
        assert!(!session.stopping());
    }
    fn fec_report(frame: u32, received_data: u16, received_parity: u16) -> [u8; 21] {
        // Ten data and two parity packets at 20 %, one block.
        let mut report = [0; 21];
        report[..4].copy_from_slice(&frame.to_be_bytes());
        for (offset, word) in [(4, 11u16), (10, 10), (12, 2)] {
            report[offset..offset + 2].copy_from_slice(&word.to_be_bytes());
        }
        report[14..16].copy_from_slice(&received_data.to_be_bytes());
        report[16..18].copy_from_slice(&received_parity.to_be_bytes());
        report[18] = 20;
        report[20] = 1;
        report
    }
    #[test]
    fn host_bitrate_requires_valid_video_feedback_and_yields_to_the_client() {
        let s = Session::new(
            launch("host-abr", Role::Stream),
            Negotiated {
                bitrate_kbps: 20_000,
                configured_bitrate_kbps: 30_000,
                ..Default::default()
            },
        );
        s.configure_adaptive_bitrate(true, 0);
        let start = Instant::now();
        s.tick_adaptive_bitrate(start, Duration::from_millis(8));
        s.record_fec_status(&fec_report(1, 7, 1)); // no frame has been sent
        assert!(!s.adaptive_bitrate.lock().unwrap().active());
        s.stats.video_frame.store(10, Ordering::Release);
        s.record_fec_status(&fec_report(1, 7, 1));
        s.tick_adaptive_bitrate(start + Duration::from_secs(1), Duration::from_millis(8));
        assert!(s.bitrate.load(Ordering::Acquire) < 20_000);
        assert!(s.adaptive_pacing_bps(u64::MAX) < 30_000_000);
        let after_loss = s.info()["adaptive_bitrate"]["wire_budget_kbps"].clone();
        s.record_fec_status(&fec_report(1, 7, 1)); // same block is ignored
        s.tick_adaptive_bitrate(start + Duration::from_secs(2), Duration::from_millis(8));
        assert_eq!(s.info()["adaptive_bitrate"]["wire_budget_kbps"], after_loss);
        s.set_client_bitrate(17_000);
        s.record_fec_status(&fec_report(2, 7, 1));
        s.tick_adaptive_bitrate(start + Duration::from_secs(4), Duration::from_millis(200));
        assert_eq!(s.bitrate.load(Ordering::Acquire), 17_000);
        assert_eq!(s.adaptive_pacing_bps(800_000_000), 800_000_000);
        assert_eq!(s.info()["adaptive_bitrate"]["owner"], "client");
    }
    fn pacing_traffic(s: &Session, frames: u64) {
        s.stats.frames.store(frames, Ordering::Relaxed);
        s.stats.packets.store(frames * 70, Ordering::Relaxed);
        s.stats.bytes.store(frames * 83_334, Ordering::Relaxed);
        // The wire frame marker stores last_sent + 1, not last_sent.
        s.stats.video_frame.store(frames + 1, Ordering::Release);
    }
    #[test]
    fn automatic_pacing_uses_validated_new_losses_without_changing_stream_quality() {
        let s = Session::new(
            launch("automatic-pacing", Role::Stream),
            Negotiated {
                bitrate_kbps: 40_000,
                ..Default::default()
            },
        );
        s.configure_automatic_pacing(true);
        s.idr.store(false, Ordering::Relaxed);
        let mode = s.stream_mode();
        let start = Instant::now();
        assert_eq!(s.video_pacing_bps(800_000_000, 50_000_000), 80_000_000);
        s.tick_automatic_pacing(start, Duration::from_millis(2));
        s.record_fec_status(&fec_report(1, 7, 1)); // No sent frames yet.
        s.record_fec_status(&fec_report(2, 7, 1));
        pacing_traffic(&s, 30);
        s.record_fec_status(&fec_report(29, 7, 1)[..20]); // Bad length.
        s.record_fec_status(&fec_report(30, 11, 1)); // Impossible count.
        s.record_fec_status(&fec_report(31, 7, 1)); // Future frames.
        s.record_fec_status(&fec_report(32, 7, 1));
        s.record_fec_status(&fec_report(29, 7, 1));
        s.record_fec_status(&fec_report(29, 7, 1)); // Duplicate block.
        assert_eq!(
            s.tick_automatic_pacing(start + Duration::from_millis(500), Duration::from_millis(2)),
            None
        );
        // The invalid reports did not reserve these frame/block identities.
        pacing_traffic(&s, 60);
        s.record_fec_status(&fec_report(30, 7, 1));
        s.record_fec_status(&fec_report(31, 7, 1));
        assert_eq!(
            s.tick_automatic_pacing(start + Duration::from_secs(1), Duration::from_millis(2)),
            Some((80_000_000, 62_500_000))
        );
        pacing_traffic(&s, 90);
        // These unseen blocks are valid telemetry, but describe the old rate.
        s.record_fec_status(&fec_report(58, 7, 1));
        s.record_fec_status(&fec_report(60, 7, 1));
        assert_eq!(
            s.tick_automatic_pacing(
                start + Duration::from_millis(1500),
                Duration::from_millis(2)
            ),
            None
        );
        pacing_traffic(&s, 120);
        s.record_fec_status(&fec_report(119, 7, 1));
        s.record_fec_status(&fec_report(120, 7, 1));
        assert_eq!(
            s.tick_automatic_pacing(start + Duration::from_secs(2), Duration::from_millis(2)),
            None // Already at the live wire-demand floor.
        );
        assert_eq!(s.video_pacing_bps(800_000_000, 50_000_000), 62_500_000);
        assert_eq!(s.bitrate.load(Ordering::Acquire), 40_000);
        assert_eq!(s.stream_mode(), mode);
        assert!(!s.idr.load(Ordering::Relaxed));
        assert!(s.invalidation.lock().unwrap().is_none());
        assert_eq!(s.stats.idr_requests.load(Ordering::Relaxed), 0);
        assert_eq!(s.stats.send_loss_recoveries.load(Ordering::Relaxed), 0);
        assert!(!s.stopping());
        let performance = &s.info()["performance"];
        assert_eq!(performance["fec_invalid_reports"], 6);
        assert_eq!(performance["fec_duplicate_reports"], 1);
    }
    #[test]
    fn automatic_pacing_respects_client_bitrate_changes_and_updated_fec_wire_demand() {
        let s = Session::new(
            launch("pacing-client-bitrate", Role::Stream),
            Negotiated {
                bitrate_kbps: 40_000,
                configured_bitrate_kbps: 60_000,
                packet_size: 1392,
                min_fec: 2,
                ..Default::default()
            },
        );
        s.configure_automatic_pacing(true);
        s.configure_adaptive_bitrate(true, 0);
        s.configure_fec(false, 20);
        s.set_client_bitrate(40_000);
        s.idr.store(false, Ordering::Relaxed);
        let mode = s.stream_mode();
        let start = Instant::now();
        let mut packetizer = crate::packet::VideoPacketizer {
            sequence: 0,
            iv_counter: 0,
            frame: 1,
            packet_size: s.config.packet_size,
            fec_percent: s.fec_percent(),
            min_fec: s.config.min_fec,
            key: Some([0; 16]),
        };
        let needed = crate::network_pacing::video_wire_rate_bps(
            &packetizer,
            s.bitrate.load(Ordering::Acquire),
            mode.period(),
            false,
        );
        s.video_pacing_bps(800_000_000, needed);
        s.tick_automatic_pacing(start, Duration::from_millis(2));
        for interval in 1..=4 {
            let frame = interval * 30;
            pacing_traffic(&s, frame);
            s.record_fec_status(&fec_report(frame as u32 - 1, 7, 1));
            s.record_fec_status(&fec_report(frame as u32, 7, 1));
            let now = start + Duration::from_millis(interval * 500);
            let change = s.tick_automatic_pacing(now, Duration::from_millis(2));
            // Startup is already close to wire demand, so the first cut
            // reaches the floor; repeated loss must not push below it.
            assert_eq!(change.is_some(), interval == 1);
            assert_eq!(s.video_pacing_bps(800_000_000, needed), needed + needed / 4);
            assert_eq!(s.tick_adaptive_bitrate(now, Duration::from_millis(2)), None);
            assert_eq!(s.bitrate.load(Ordering::Acquire), 40_000);
        }
        let previous = s.video_pacing_bps(800_000_000, needed);
        assert_eq!(previous, needed + needed / 4);
        // The real /bitrate writer retains ownership. The sender supplies a
        // refreshed wire budget when bitrate or FEC changes between frames.
        s.set_client_bitrate(80_000);
        packetizer.fec_percent = s.configure_fec(false, 50);
        let needed = crate::network_pacing::video_wire_rate_bps(
            &packetizer,
            s.bitrate.load(Ordering::Acquire),
            mode.period(),
            false,
        );
        let raised = s.video_pacing_bps(800_000_000, needed);
        assert!(raised > previous);
        assert_eq!(raised, needed + needed / 4);
        pacing_traffic(&s, 150);
        s.record_fec_status(&fec_report(149, 7, 1));
        s.record_fec_status(&fec_report(150, 7, 1));
        let now = start + Duration::from_millis(2500);
        assert_eq!(
            s.tick_automatic_pacing(now, Duration::from_millis(200)),
            None
        );
        assert_eq!(
            s.tick_adaptive_bitrate(now, Duration::from_millis(200)),
            None
        );
        assert_eq!(s.bitrate.load(Ordering::Acquire), 80_000);
        assert_eq!(s.info()["adaptive_bitrate"]["owner"], "client");
        assert_eq!(s.stream_mode(), mode);
        assert!(!s.idr.load(Ordering::Relaxed));
    }
    #[test]
    fn automatic_pacing_configuration_and_pyrowave_bypass_preserve_the_base_rate() {
        for (enabled, codec) in [(false, 0), (false, 1), (true, 3)] {
            let s = Session::new(
                launch("pacing-bypass", Role::Stream),
                Negotiated {
                    codec,
                    ..Default::default()
                },
            );
            s.configure_automatic_pacing(enabled);
            assert_eq!(s.video_pacing_bps(950_000_000, 50_000_000), 950_000_000);
            let start = Instant::now();
            s.tick_automatic_pacing(start, Duration::from_millis(2));
            pacing_traffic(&s, 30);
            s.record_fec_status(&fec_report(29, 7, 1));
            s.record_fec_status(&fec_report(30, 7, 1));
            assert_eq!(
                s.tick_automatic_pacing(start + Duration::from_secs(1), Duration::from_millis(2)),
                None
            );
            assert_eq!(s.video_pacing_bps(950_000_000, 50_000_000), 950_000_000);
            let info = s.info();
            assert_eq!(info["packet_pacing"]["automatic"], false);
            assert_eq!(info["performance"]["fec_unrecoverable_frames"], 2);
        }
    }
    #[test]
    fn automatic_pacing_guards_first_frame_and_each_reconnect() {
        let start = Instant::now();
        let launch = launch("pacing-reconnect", Role::Stream);
        let negotiated = Negotiated {
            bitrate_kbps: 40_000,
            ..Default::default()
        };
        let first = Session::new(launch.clone(), negotiated.clone());
        first.configure_automatic_pacing(true);
        assert_eq!(first.stats.frames.load(Ordering::Relaxed), 0);
        assert_eq!(first.video_pacing_bps(800_000_000, 50_000_000), 80_000_000);
        first.tick_automatic_pacing(start, Duration::from_millis(2));
        let ping = [4, 0, 0, 0, 0, 0, 0, 0];
        for interval in 1..=30 {
            let now = start + Duration::from_millis(interval * 500);
            pacing_traffic(&first, interval * 30);
            first.record_control_ping(now, &ping);
            first.tick_automatic_pacing(now, Duration::from_millis(2));
            first.video_pacing_bps(800_000_000, 50_000_000);
        }
        assert_eq!(first.video_pacing_bps(800_000_000, 50_000_000), 800_000_000);
        assert_eq!(first.bitrate.load(Ordering::Acquire), 40_000);
        // A client can change its downstream connection without changing
        // its identity or host route. Never reuse an old fast rate at startup.
        let reconnect = Session::new(launch, negotiated);
        reconnect.configure_automatic_pacing(true);
        assert_eq!(reconnect.stats.frames.load(Ordering::Relaxed), 0);
        assert_eq!(
            reconnect.video_pacing_bps(800_000_000, 50_000_000),
            80_000_000
        );
        assert_eq!(first.video_pacing_bps(800_000_000, 50_000_000), 800_000_000);
        assert_eq!(reconnect.bitrate.load(Ordering::Acquire), 40_000);
    }
    #[test]
    fn adaptive_fec_follows_reports_and_stays_fixed_without_them_or_when_off() {
        let start = Instant::now();
        // A client that never reports: the configured percentage, for minutes.
        let quiet = Session::new(launch("quiet", Role::Stream), Negotiated::default());
        assert_eq!(quiet.configure_fec(true, 30), 30);
        for second in 0..300 {
            assert_eq!(quiet.tick_fec(start + Duration::from_secs(second)), None);
        }
        assert_eq!(quiet.fec_percent(), 30);
        assert_eq!(quiet.info()["fec_percent"], 30);
        // Off: reports are still counted, the percentage never moves.
        let off = Session::new(launch("off", Role::Stream), Negotiated::default());
        assert_eq!(off.configure_fec(false, 20), 20);
        off.stats.video_frame.store(100, Ordering::Release);
        off.record_fec_status(&fec_report(10, 10, 0));
        off.record_fec_status(&fec_report(11, 7, 1));
        for second in 0..120 {
            assert_eq!(off.tick_fec(start + Duration::from_secs(second)), None);
        }
        assert_eq!(off.fec_percent(), 20);
        assert_eq!(off.info()["performance"]["fec_unrecoverable_frames"], 1);
        // On: a clean link decays, a rebuilt frame raises, a lost one restores.
        let on = Session::new(launch("on", Role::Stream), Negotiated::default());
        assert_eq!(on.configure_fec(true, 20), 20);
        on.stats.video_frame.store(100, Ordering::Release);
        on.record_fec_status(&fec_report(10, 10, 0));
        let now = Instant::now();
        assert_eq!(on.tick_fec(now), None);
        let later = now + Duration::from_secs(60);
        on.tick_fec(later);
        assert!(on.fec_percent() < 20);
        let decayed = on.fec_percent();
        on.record_fec_status(&fec_report(20, 9, 1));
        assert!(on.fec_percent() > decayed);
        on.record_fec_status(&fec_report(21, 7, 1));
        assert_eq!(on.fec_percent(), 20);
        // Recovery frames keep the configured percentage on a decayed link.
        on.tick_fec(later + Duration::from_secs(120));
        let decayed = on.fec_percent();
        assert!(decayed < 20);
        assert_eq!(on.frame_fec_percent(false), decayed);
        assert_eq!(on.frame_fec_percent(true), 20);
        assert_eq!(off.frame_fec_percent(true), 20);
        assert_eq!(quiet.frame_fec_percent(false), 30);
        // PyroWave does not use it, and does not show it.
        let pyrowave = Session::new(
            launch("pyrowave-fec", Role::Stream),
            Negotiated {
                codec: 3,
                ..Default::default()
            },
        );
        assert!(pyrowave.info()["fec_percent"].is_null());
    }
    #[test]
    fn client_luminance_changes_never_relabel_hdr_content() {
        use rubylight_protocol::control::DisplayCaps;
        let display = crate::hdr::Metadata::display(1000., 0.01, 600.);
        for hdr in [false, true] {
            let s = Session::new(
                launch("hdr", Role::Stream),
                Negotiated {
                    hdr,
                    ..Default::default()
                },
            );
            let mut caps = DisplayCaps {
                hdr: true,
                max_centinits: 260_000,
                max_average_centinits: 120_000,
                min_decimillinits: 5,
            };
            // Early reports must not start HDR with the receiver's luminance.
            assert!(s.record_display_caps(&caps.encode()).unwrap().changed);
            assert!(s.hdr_metadata.read().unwrap().is_none());
            assert_eq!(s.set_display_hdr_metadata(display), display);
            // Brighter, dimmer, unknown and SDR receiver reports all preserve
            // the source metadata, preventing HDR control updates/codec restarts.
            for peak in [40_000, 100_000, 260_000, 0] {
                caps.max_centinits = peak;
                caps.max_average_centinits = 0;
                caps.hdr = peak != 0;
                s.record_display_caps(&caps.encode()).unwrap();
                assert_eq!(*s.hdr_metadata.read().unwrap(), Some(display));
                assert_eq!(s.set_display_hdr_metadata(display), display);
            }
            assert!(s.record_display_caps(&caps.encode()[..8]).is_none());
            assert_eq!(*s.hdr_metadata.read().unwrap(), Some(display));
            // A real source change must still reach the encoder and client.
            let new_source = crate::hdr::Metadata::display(1500., 0.001, 800.);
            assert_eq!(s.set_display_hdr_metadata(new_source), new_source);
            assert_eq!(*s.hdr_metadata.read().unwrap(), Some(new_source));
        }
    }
    #[test]
    fn a_launch_still_being_prepared_does_not_expire() {
        let mut sessions = Sessions::default();
        let mut slow = launch("slow", Role::Stream);
        slow.created = Instant::now() - Duration::from_secs(90);
        slow.preparing.store(true, Ordering::Release);
        sessions.pending.insert(slow.id.clone(), slow.clone());
        drop(sessions.expire());
        assert!(sessions.pending.contains_key("slow"));
        // Prepared: the client's 30 s count from then.
        slow.preparing.store(false, Ordering::Release);
        drop(sessions.expire());
        assert!(!sessions.pending.contains_key("slow"));
    }
    #[test]
    fn starting_rechecks_launch_expiry_after_rtsp_authorization() {
        let mut sessions = Sessions::default();
        let fresh = launch("expired-before-play", Role::Stream);
        sessions.pending.insert(fresh.id.clone(), fresh.clone());
        let authorized = sessions.pending_for_peer(fresh.peer).unwrap();
        // The RTSP connection can wait for input after taking its authorized
        // snapshot. PLAY must check the current launch, even before housekeeping.
        sessions.pending.get_mut(&fresh.id).unwrap().created =
            Instant::now() - Duration::from_secs(31);
        assert!(sessions.start(authorized, Negotiated::default()).is_err());
        assert!(sessions.active.is_empty());
        // Keep preparation ownership for expire() to return to its caller;
        // restoring display/audio resources must happen outside the session lock.
        assert!(sessions.pending.contains_key(&fresh.id));
        assert_eq!(sessions.expire().len(), 1);

        let fresh = launch("valid-play", Role::Stream);
        sessions.pending.insert(fresh.id.clone(), fresh.clone());
        assert!(sessions.start(fresh.clone(), Negotiated::default()).is_ok());
        assert!(sessions.start(fresh, Negotiated::default()).is_err());
    }
    #[test]
    fn rtsp_authorization_does_not_keep_canceled_preparations_alive() {
        use std::sync::{Mutex, Weak};
        struct Restore {
            sessions: Weak<Mutex<Sessions>>,
            restored: Arc<AtomicU32>,
        }
        impl Preparation<()> for Restore {
            fn prepared(&self) -> &() {
                &()
            }
            fn into_prepared(self: Box<Self>) {}
        }
        impl AudioPreparation<()> for Restore {
            fn route(&self) -> Arc<()> {
                Arc::new(())
            }
        }
        impl Drop for Restore {
            fn drop(&mut self) {
                assert!(self.sessions.upgrade().unwrap().try_lock().is_ok());
                self.restored.fetch_add(1, Ordering::Release);
            }
        }
        for expires in [false, true] {
            let sessions = Arc::new(Mutex::new(Sessions::default()));
            let restored = Arc::new(AtomicU32::new(0));
            let pending = launch("pending", Role::Stream);
            *pending.preparation.lock().unwrap() = Some(Box::new(Restore {
                sessions: Arc::downgrade(&sessions),
                restored: restored.clone(),
            }));
            *pending.audio_preparation.lock().unwrap() = Some(Box::new(Restore {
                sessions: Arc::downgrade(&sessions),
                restored: restored.clone(),
            }));
            let (authorized, canceled) = {
                let mut locked = sessions.lock().unwrap();
                let peer = pending.peer;
                locked.queue(pending).unwrap();
                let authorized = locked.rtsp_for_peer(peer);
                let canceled = if expires {
                    locked.pending.get_mut("pending").unwrap().created =
                        Instant::now() - Duration::from_secs(31);
                    locked.expire()
                } else {
                    locked.request_stop(Some("pending"))
                };
                (authorized, canceled)
            };
            drop(canceled);
            assert_eq!(
                restored.load(Ordering::Acquire),
                2,
                "an RTSP socket must not retain canceled display and audio preparation"
            );
            assert_eq!(authorized[0].id, "pending");
        }
    }
    #[test]
    fn play_consumes_the_current_pending_launch_with_its_preparation() {
        struct Prepared;
        impl Preparation<()> for Prepared {
            fn prepared(&self) -> &() {
                &()
            }
            fn into_prepared(self: Box<Self>) {}
        }
        let mut sessions = Sessions::default();
        let pending = launch("prepared", Role::Stream);
        let peer = pending.peer;
        *pending.preparation.lock().unwrap() = Some(Box::new(Prepared));
        sessions.queue(pending).unwrap();
        let authorized = sessions.rtsp_for_peer(peer).pop().unwrap();
        sessions
            .pending
            .get_mut("prepared")
            .unwrap()
            .options
            .insert("server_state".into(), "current".into());
        let session = sessions.start(authorized, Negotiated::default()).unwrap();
        assert_eq!(
            session
                .launch
                .options
                .get("server_state")
                .map(String::as_str),
            Some("current")
        );
        assert!(session.launch.preparation.lock().unwrap().is_some());
        assert!(sessions.pending.is_empty());
    }
    #[test]
    fn expired_launches_restore_resources_after_unlocking_sessions() {
        use std::sync::{Mutex, Weak};
        struct Restore {
            sessions: Weak<Mutex<Sessions>>,
            restored: Arc<AtomicBool>,
        }
        impl Preparation<()> for Restore {
            fn prepared(&self) -> &() {
                &()
            }
            fn into_prepared(self: Box<Self>) {}
        }
        impl Drop for Restore {
            fn drop(&mut self) {
                assert!(self.sessions.upgrade().unwrap().try_lock().is_ok());
                self.restored.store(true, Ordering::Release);
            }
        }
        let sessions = Arc::new(Mutex::new(Sessions::default()));
        let restored = Arc::new(AtomicBool::new(false));
        let mut expired = launch("expired", Role::Stream);
        expired.created = Instant::now() - Duration::from_secs(31);
        *expired.preparation.lock().unwrap() = Some(Box::new(Restore {
            sessions: Arc::downgrade(&sessions),
            restored: restored.clone(),
        }));
        let expired = {
            let mut locked = sessions.lock().unwrap();
            locked.pending.insert(expired.id.clone(), expired);
            let live = launch("live", Role::InputOnly);
            locked.queue(live.clone()).unwrap();
            assert_eq!(locked.pending_for_peer(live.peer).unwrap().id, "live");
            assert_eq!(locked.rtsp_for_peer(live.peer).len(), 1);
            let expired = locked.expire();
            assert_eq!(expired.len(), 1);
            assert_eq!(expired[0].id, "expired");
            assert!(locked.pending.contains_key("live"));
            assert!(!restored.load(Ordering::Acquire));
            expired
        };
        assert!(!restored.load(Ordering::Acquire));
        drop(expired);
        assert!(restored.load(Ordering::Acquire));
    }
    #[test]
    fn canceled_launches_restore_resources_after_unlocking_sessions() {
        use std::sync::{Mutex, Weak};
        struct Restore {
            sessions: Weak<Mutex<Sessions>>,
            restored: Arc<AtomicBool>,
        }
        impl Preparation<()> for Restore {
            fn prepared(&self) -> &() {
                &()
            }
            fn into_prepared(self: Box<Self>) {}
        }
        impl Drop for Restore {
            fn drop(&mut self) {
                assert!(self.sessions.upgrade().unwrap().try_lock().is_ok());
                self.restored.store(true, Ordering::Release);
            }
        }
        let cancellations: [fn(&mut Sessions) -> Vec<Launch>; 7] = [
            |s| s.supersede("client", Role::Stream),
            |s| s.request_stop(None),
            |s| s.request_stop(Some("pending")),
            |s| s.request_stop(Some("client")),
            |s| s.stop_role(Role::Stream, None),
            |s| s.stop_role(Role::Stream, Some("client")),
            |s| s.queue(launch("replacement", Role::Stream)).unwrap(),
        ];
        for cancel in cancellations {
            let sessions = Arc::new(Mutex::new(Sessions::default()));
            let restored = Arc::new(AtomicBool::new(false));
            let pending = launch("pending", Role::Stream);
            *pending.preparation.lock().unwrap() = Some(Box::new(Restore {
                sessions: Arc::downgrade(&sessions),
                restored: restored.clone(),
            }));
            let canceled = {
                let mut locked = sessions.lock().unwrap();
                locked.pending.insert(pending.id.clone(), pending);
                let canceled = cancel(&mut locked);
                assert_eq!(canceled.len(), 1);
                assert!(!locked.pending.contains_key("pending"));
                assert!(!restored.load(Ordering::Acquire));
                canceled
            };
            assert!(!restored.load(Ordering::Acquire));
            drop(canceled);
            assert!(restored.load(Ordering::Acquire));
        }
    }
    #[test]
    fn pending_launches_and_streams_keep_sessions_busy_until_they_expire_or_end() {
        let mut sessions = Sessions::default();
        assert!(sessions.idle());
        let mut pending = launch("pending", Role::Stream);
        sessions.pending.insert(pending.id.clone(), pending.clone());
        assert!(!sessions.idle());
        pending.created = Instant::now() - Duration::from_secs(31);
        sessions.pending.insert(pending.id.clone(), pending.clone());
        assert!(sessions.idle());
        assert!(sessions.owns_capture());
        pending.preparing.store(true, Ordering::Release);
        assert!(!sessions.idle());
        pending.preparing.store(false, Ordering::Release);
        let fresh = launch("fresh", Role::Stream);
        sessions.pending.insert(fresh.id.clone(), fresh.clone());
        sessions.start(fresh, Negotiated::default()).unwrap();
        assert!(!sessions.idle());
        sessions.active.clear();
        sessions.teardown.insert("ended".into(), Instant::now());
        assert!(!sessions.idle());
        sessions.teardown.clear();
        assert!(sessions.idle());
    }
    fn launch(id: &str, role: Role) -> Launch {
        Launch {
            id: id.into(),
            client: Client {
                name: "fixture".into(),
                cert: String::new(),
                uuid: "client".into(),
                perm: u32::MAX,
                enabled: true,
                extra: Default::default(),
            },
            peer: "127.0.0.1".parse().unwrap(),
            app_id: 1,
            key: [0; 16],
            key_id: 1,
            ping: "ping".into(),
            connect_data: 1,
            role,
            created: Instant::now(),
            rtsp_encrypted: true,
            rtsp_counter: Arc::new(AtomicU32::new(1)),
            rtsp_received: Default::default(),
            preparation: Default::default(),
            vrr_requested: false,
            host_audio: false,
            requested_rate: 0,
            options: Default::default(),
            audio_preparation: Default::default(),
            preparing: Default::default(),
            warnings: Default::default(),
        }
    }
    #[test]
    fn a_failed_stream_is_not_reported_as_a_normal_close() {
        let closed = Session::new(launch("closed", Role::Stream), Negotiated::default());
        closed.stop();
        assert_eq!(closed.termination_reason(), 0x8003_0023);
        let failed = Session::new(launch("failed", Role::Stream), Negotiated::default());
        failed.fail();
        assert!(failed.stopping());
        assert_eq!(failed.termination_reason(), 0x8000_4005);
    }
    #[test]
    fn a_worker_that_cannot_start_fails_without_leaving_an_active_session() {
        let mut sessions = Sessions::default();
        let failed = launch("failed", Role::Stream);
        sessions.queue(failed.clone()).unwrap();
        let failed = sessions.start(failed, Negotiated::default()).unwrap();
        let other = launch("other", Role::InputOnly);
        sessions.queue(other.clone()).unwrap();
        let other = sessions.start(other, Negotiated::default()).unwrap();
        sessions.fail_start(&failed);
        assert!(failed.failed());
        assert!(failed.stopping());
        assert_eq!(failed.termination_reason(), 0x8000_4005);
        assert!(!sessions.active.contains_key("failed"));
        assert!(sessions.active.contains_key("other"));
        assert!(!other.stopping());
        assert!(sessions.teardown.is_empty());
        let retry = launch("retry", Role::Stream);
        sessions.queue(retry.clone()).unwrap();
        sessions.start(retry, Negotiated::default()).unwrap();
    }
    #[test]
    fn teardown_leaves_active_while_resources_remain_owned() {
        use std::sync::{Mutex, Weak};
        struct Resource {
            sessions: Weak<Mutex<Sessions>>,
            released: Arc<AtomicBool>,
        }
        impl Preparation<()> for Resource {
            fn prepared(&self) -> &() {
                &()
            }
            fn into_prepared(self: Box<Self>) {}
        }
        impl Drop for Resource {
            fn drop(&mut self) {
                let sessions = self.sessions.upgrade().unwrap();
                let sessions = sessions.try_lock().unwrap();
                assert!(!sessions.active.contains_key("old"));
                assert!(sessions.teardown.contains_key("old"));
                self.released.store(true, Ordering::Release);
            }
        }
        let sessions = Arc::new(Mutex::new(Sessions::default()));
        let released = Arc::new(AtomicBool::new(false));
        let old = launch("old", Role::Stream);
        *old.preparation.lock().unwrap() = Some(Box::new(Resource {
            sessions: Arc::downgrade(&sessions),
            released: released.clone(),
        }));
        let old = {
            let mut sessions = sessions.lock().unwrap();
            sessions.queue(old.clone()).unwrap();
            sessions.start(old, Negotiated::default()).unwrap()
        };
        old.stop();
        {
            let mut sessions = sessions.lock().unwrap();
            sessions.begin_teardown(&old);
            assert!(sessions.active.is_empty());
            assert!(sessions.owns_capture());
            assert!(!sessions.idle());
            let mut next = launch("next", Role::Stream);
            next.app_id = 2;
            sessions.queue(next.clone()).unwrap();
            sessions.start(next, Negotiated::default()).unwrap();
            assert!(!released.load(Ordering::Acquire));
        }
        drop(old);
        assert!(released.load(Ordering::Acquire));
        let mut sessions = sessions.lock().unwrap();
        sessions.teardown.remove("old");
        sessions.active.clear();
        assert!(sessions.idle());
        assert!(!sessions.owns_capture());
    }
    #[test]
    fn only_pyrowave_sessions_report_quality_bitrates() {
        for codec in 0..=3 {
            let config = Negotiated {
                codec,
                rate_millihz: 59_940,
                ..Default::default()
            };
            let session = Session::new(launch("quality", Role::Stream), config.clone());
            let info = session.info();
            if codec == 3 {
                assert_eq!(
                    info["pyrowave_minimum_kbps"],
                    crate::pyrowave::minimum_kbps(config.width, config.height, 59_940)
                );
                assert_eq!(
                    info["pyrowave_recommended_kbps"],
                    crate::pyrowave::recommended_kbps(config.width, config.height, 59_940)
                );
            } else {
                assert!(info["pyrowave_minimum_kbps"].is_null());
                assert!(info["pyrowave_recommended_kbps"].is_null());
            }
            session.bitrate.store(100_000, Ordering::Relaxed);
            assert_eq!(session.info()["encoder_bitrate_kbps"], 100_000);
        }
    }
    #[test]
    fn a_lost_keyframe_paces_the_rest_of_the_stream_like_wifi() {
        const FAST: u64 = 800_000_000;
        const SAFE: u64 = 62_000_000;
        let s = Session::new(launch("onn", Role::Stream), Negotiated::default());
        s.configure_keyframe_loss(true);
        assert_eq!(s.keyframe_loss_bps(FAST, SAFE, 1, true), FAST);
        for frame in 2..=8 {
            assert_eq!(s.keyframe_loss_bps(FAST, SAFE, frame, false), FAST);
        }
        s.stats.video_frame.store(9, Ordering::Release);
        // A lost ordinary frame is left to recovery frames.
        s.record_fec_status(&fec_report(5, 7, 1));
        assert_eq!(s.keyframe_loss_bps(FAST, SAFE, 9, false), FAST);
        assert!(s.info()["warnings"].as_array().unwrap().is_empty());
        // The keyframe arrived short of what FEC repairs.
        s.record_fec_status(&fec_report(1, 7, 1));
        assert_eq!(s.keyframe_loss_bps(FAST, SAFE, 10, false), SAFE);
        assert_eq!(s.keyframe_loss_bps(FAST, SAFE, 11, true), SAFE);
        assert_eq!(s.info()["warnings"][0]["code"], "network_keyframe_loss");

        // Without reports: the client asks again for the keyframe it was sent.
        let s = Session::new(launch("old", Role::Stream), Negotiated::default());
        s.configure_keyframe_loss(true);
        s.keyframe_loss_bps(FAST, SAFE, 1, true);
        s.request_recovery_keyframe();
        assert_eq!(s.keyframe_loss_bps(FAST, SAFE, 2, true), FAST);
        s.request_recovery_keyframe();
        assert_eq!(s.keyframe_loss_bps(FAST, SAFE, 3, true), SAFE);
        assert_eq!(s.stats.idr_requests.load(Ordering::Relaxed), 2);

        // Off, and for PyroWave's intra-only frames, nothing changes.
        for (enabled, codec) in [(false, 1), (true, 3)] {
            let s = Session::new(
                launch("off", Role::Stream),
                Negotiated {
                    codec,
                    ..Default::default()
                },
            );
            s.configure_keyframe_loss(enabled);
            s.keyframe_loss_bps(FAST, SAFE, 1, true);
            s.stats.video_frame.store(2, Ordering::Release);
            s.record_fec_status(&fec_report(1, 0, 0));
            s.request_recovery_keyframe();
            s.keyframe_loss_bps(FAST, SAFE, 2, true);
            s.request_recovery_keyframe();
            assert_eq!(s.keyframe_loss_bps(FAST, SAFE, 3, false), FAST);
        }
    }
    #[test]
    fn the_console_shows_the_encoded_rate_beside_the_requested_one() {
        // Artemis's Warp launches at 60 fps and asks for 120 in the stream
        // request; with the launch frame rate on, 60 is encoded. The console
        // showed 120 as the stream's rate and target.
        let request = Negotiated {
            fps: 120,
            rate_millihz: 120_000,
            ..Default::default()
        };
        let mut config = request.clone();
        let limited = crate::config::Config::parse("limit_framerate=true\n").unwrap();
        crate::stream_policy::apply(&mut config, 60_000, &limited);
        let session = Session::new(launch("warp", Role::Stream), config.clone());
        let info = session.info();
        assert_eq!(
            (info["fps"].clone(), info["requested_fps"].clone()),
            (60.into(), 120.into())
        );
        // By default the stream runs at the request's rate.
        let mut following = request.clone();
        crate::stream_policy::apply(&mut following, 60_000, &crate::config::Config::default());
        let info = Session::new(launch("warp", Role::Stream), following).info();
        assert_eq!(
            (info["fps"].clone(), info["requested_fps"].clone()),
            (120.into(), 120.into())
        );
        // A client's own mode switch is what it asked for.
        let switch = crate::reconfigure::Mode {
            width: config.width,
            height: config.height,
            fps_millihz: 90_000,
        };
        let mut reconfigure = session.reconfigure.lock().unwrap();
        let now = Instant::now();
        reconfigure.on_request(now, switch);
        assert_eq!(
            reconfigure.take_due(now + Duration::from_secs(1)),
            Some(switch)
        );
        drop(reconfigure);
        let info = session.info();
        assert_eq!(
            (info["fps"].clone(), info["requested_fps"].clone()),
            (90.into(), 90.into())
        );
        let unlimited = Negotiated {
            fps: 120,
            rate_millihz: 119_880,
            ..Default::default()
        };
        let info = Session::new(launch("plain", Role::Stream), unlimited).info();
        assert_eq!(
            (info["fps"].clone(), info["requested_fps"].clone()),
            (120.into(), 120.into())
        );
    }
    #[test]
    fn pyrowave_feedback_cannot_force_frames_or_turn_invalidation_into_idr() {
        let session = Session::new(
            launch("pyrowave", Role::Stream),
            Negotiated {
                codec: 3,
                ..Default::default()
            },
        );
        session.idr.store(false, Ordering::Release);
        *session.recovery_wake.lock().unwrap() = Some(Box::new(|| {
            panic!("intra-only feedback must not wake the session");
        }));
        for (first, last) in [(1, 2), (0, 0), (4, 3)] {
            session.request_invalidation(first, last);
        }
        assert_eq!(
            session
                .stats
                .reference_invalidations
                .load(Ordering::Relaxed),
            3
        );
        assert_eq!(session.stats.idr_requests.load(Ordering::Relaxed), 0);
        assert!(session.invalidation.lock().unwrap().is_none());
        session.request_idr();
        assert_eq!(session.stats.idr_requests.load(Ordering::Relaxed), 1);
        assert!(!session.idr.load(Ordering::Acquire));
    }
    #[test]
    fn send_loss_recovery_forces_a_keyframe_without_counting_a_client_request() {
        for codec in 0..=3 {
            let session = Session::new(
                launch("send-loss", Role::Stream),
                Negotiated {
                    codec,
                    ..Default::default()
                },
            );
            session.idr.store(false, Ordering::Release);
            session.request_send_loss_recovery();
            // PyroWave frames are all intra; nothing to force.
            assert_eq!(session.idr.load(Ordering::Acquire), codec != 3);
            assert_eq!(session.stats.idr_requests.load(Ordering::Relaxed), 0);
            assert_eq!(session.info()["send_loss_recoveries"], 1);
        }
    }
    #[test]
    fn inter_frame_recovery_keeps_invalidation_ranges_and_idr_fallback() {
        for codec in 0..=2 {
            let session = Session::new(
                launch("inter-frame", Role::Stream),
                Negotiated {
                    codec,
                    ..Default::default()
                },
            );
            session.idr.store(false, Ordering::Release);
            session.request_invalidation(4, 6);
            session.request_invalidation(2, 3);
            assert_eq!(*session.invalidation.lock().unwrap(), Some((2, 6)));
            assert!(!session.idr.load(Ordering::Acquire));
            session.request_invalidation(0, 0);
            assert!(session.idr.load(Ordering::Acquire));
            assert_eq!(session.stats.idr_requests.load(Ordering::Relaxed), 1);
            assert_eq!(
                session
                    .stats
                    .reference_invalidations
                    .load(Ordering::Relaxed),
                3
            );
        }
    }
    #[test]
    fn independent_roles_and_targeted_teardown_preserve_other_sessions() {
        let mut sessions = Sessions::default();
        sessions.queue(launch("game", Role::Stream)).unwrap();
        sessions
            .queue(launch("monitor", Role::RemoteMonitor))
            .unwrap();
        sessions.queue(launch("input", Role::InputOnly)).unwrap();
        // A client launching again replaces its own earlier launch in that role.
        sessions
            .queue(launch("duplicate", Role::RemoteMonitor))
            .unwrap();
        assert!(!sessions.pending.contains_key("monitor"));
        assert!(sessions.pending.contains_key("duplicate"));
        let game = sessions
            .start(launch("game", Role::Stream), Negotiated::default())
            .unwrap();
        let input = sessions
            .start(launch("input", Role::InputOnly), Negotiated::default())
            .unwrap();
        drop(sessions.stop_role(Role::RemoteMonitor, Some("client")));
        assert!(sessions.pending.is_empty());
        assert!(!game.stopping());
        assert!(!input.stopping());
        drop(sessions.stop_role(Role::Stream, None));
        assert!(game.stopping());
        assert!(!input.stopping());
        drop(sessions.request_stop(Some("client")));
        assert!(input.stopping());
    }
    #[test]
    fn a_new_launch_replaces_an_unconnected_launch_and_stops_an_abandoned_stream() {
        let mut sessions = Sessions::default();
        // A launch whose RTSP never arrived must not block the next attempt.
        sessions.queue(launch("first", Role::Stream)).unwrap();
        sessions.queue(launch("retry", Role::Stream)).unwrap();
        assert_eq!(sessions.pending.keys().collect::<Vec<_>>(), ["retry"]);
        // A stream the client abandoned is stopped when it launches again.
        let old = sessions
            .start(launch("retry", Role::Stream), Negotiated::default())
            .unwrap();
        sessions.queue(launch("resume", Role::Stream)).unwrap();
        assert!(old.stopping());
        assert!(sessions.pending.contains_key("resume"));
        // Another client's stream is untouched.
        let mut other = launch("other", Role::Stream);
        other.client.uuid = "other-client".into();
        sessions.queue(other.clone()).unwrap();
        let other = sessions.start(other, Negotiated::default()).unwrap();
        sessions.queue(launch("again", Role::Stream)).unwrap();
        assert!(!other.stopping());
    }
}
/// A platform preparation that can be borrowed or moved into its stream.
pub trait Preparation<T>: Send {
    fn prepared(&self) -> &T;
    fn into_prepared(self: Box<Self>) -> T;
}

/// A prepared audio route shared with the app and its stream.
pub trait AudioPreparation<T>: Send + Sync {
    fn route(&self) -> Arc<T>;
}

pub struct Launch<P = (), A = ()> {
    pub warnings: Arc<Warnings>,
    pub id: String,
    pub client: Client,
    pub peer: IpAddr,
    pub app_id: u32,
    pub key: [u8; 16],
    pub key_id: u32,
    pub ping: String,
    pub connect_data: u32,
    pub role: Role,
    pub created: Instant,
    pub rtsp_encrypted: bool,
    pub rtsp_counter: Arc<AtomicU32>,
    pub rtsp_received: Arc<std::sync::Mutex<crate::packet::ReplayWindow>>,
    /// The platform host owns launch preparation. Keeping it on the launch
    /// gives expired, rejected and disconnected requests the same RAII teardown.
    pub preparation: Arc<std::sync::Mutex<Option<Box<dyn Preparation<P>>>>>,
    pub vrr_requested: bool,
    pub host_audio: bool,
    pub requested_rate: u32,
    pub options: BTreeMap<String, String>,
    pub audio_preparation: Arc<std::sync::Mutex<Option<Box<dyn AudioPreparation<A>>>>>,
    /// While the host still prepares the launch (displays, audio, the app's
    /// own commands, which can take minutes), it does not expire: the client
    /// connects only after the launch reply.
    pub preparing: Arc<AtomicBool>,
}
// Launch clones share the slots; the platform preparations need not be Clone.
impl<P, A> Clone for Launch<P, A> {
    fn clone(&self) -> Self {
        Self {
            warnings: self.warnings.clone(),
            id: self.id.clone(),
            client: self.client.clone(),
            peer: self.peer,
            app_id: self.app_id,
            key: self.key,
            key_id: self.key_id,
            ping: self.ping.clone(),
            connect_data: self.connect_data,
            role: self.role,
            created: self.created,
            rtsp_encrypted: self.rtsp_encrypted,
            rtsp_counter: self.rtsp_counter.clone(),
            rtsp_received: self.rtsp_received.clone(),
            preparation: self.preparation.clone(),
            vrr_requested: self.vrr_requested,
            host_audio: self.host_audio,
            requested_rate: self.requested_rate,
            options: self.options.clone(),
            audio_preparation: self.audio_preparation.clone(),
            preparing: self.preparing.clone(),
        }
    }
}
impl<P, A> Launch<P, A> {
    /// RTSP may wait for network input after authorization. Its identity
    /// snapshot must not keep a canceled launch's display, limiter or audio
    /// route alive while a replacement launch prepares new resources.
    fn rtsp_snapshot(&self) -> Self {
        let mut snapshot = self.clone();
        snapshot.preparation = Default::default();
        snapshot.audio_preparation = Default::default();
        snapshot
    }
    /// Still waiting for its client: being prepared, or prepared under 30 s ago.
    fn live(&self) -> bool {
        self.preparing.load(Ordering::Acquire) || self.created.elapsed() < Duration::from_secs(30)
    }
}
#[derive(Default)]
pub struct Stats {
    pub frames: AtomicU64,
    pub packets: AtomicU64,
    pub bytes: AtomicU64,
    pub idr_requests: AtomicU64,
    pub reference_invalidations: AtomicU64,
    /// Keyframes the host started itself after it failed to send a frame
    /// that FEC can repair; not counted in `idr_requests`.
    pub send_loss_recoveries: AtomicU64,
    pub latency_us: AtomicU64,
    pub frames_replaced: AtomicU64,
    /// Last frame offered to the video socket, plus one; zero means none yet.
    pub video_frame: AtomicU64,
    pub performance: std::sync::Mutex<crate::performance::Performance>,
}
pub struct Session<P = (), A = ()> {
    pub encoder: std::sync::RwLock<String>,
    pub capture_warnings: std::sync::RwLock<Arc<Warnings>>,
    pub launch: Launch<P, A>,
    pub config: Negotiated,
    pub stop: AtomicBool,
    /// Set before `stop` when the stream ends on an error, so the client is
    /// told it failed rather than that the host closed it.
    pub failed: AtomicBool,
    pub idr: AtomicBool,
    pub invalidation: std::sync::Mutex<Option<(u64, u64)>>,
    pub recovery_wake: std::sync::Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
    pub bitrate: AtomicU32,
    /// All bitrate writers take this lock, including client ownership handoff.
    pub adaptive_bitrate: std::sync::Mutex<crate::adaptive_bitrate::AdaptiveBitrate>,
    pub adaptive_video_pacing_bps: AtomicU64,
    pub automatic_pacing: std::sync::Mutex<crate::adaptive_pacing::AdaptivePacing>,
    /// Slower pacing once the client's link has dropped a keyframe.
    pub keyframe_loss: std::sync::Mutex<crate::keyframe_loss::KeyframeLoss>,
    /// FEC percentage for the next video frame; the video sender reads it per
    /// frame, and `adaptive_fec` moves it from the client's 0x5502 reports.
    pub fec_percent: AtomicU32,
    pub adaptive_fec: std::sync::Mutex<crate::adaptive_fec::AdaptiveFec>,
    /// Capture timing locked to the client's display, from its 0x5530 reports.
    pub phase_sync: std::sync::Mutex<crate::phase_sync::PhaseSync>,
    /// The stream's current mode and the client's 0x5532 requests to change it.
    pub reconfigure: std::sync::Mutex<crate::reconfigure::Reconfiguration>,
    pub stats: Stats,
    pub started: Instant,
    pub output: std::sync::RwLock<String>,
    /// The display's HDR metadata, once the encoder has it. Clients are told
    /// the HDR state only then, so they hear it once with the real values.
    pub hdr_metadata: std::sync::RwLock<Option<crate::hdr::Metadata>>,
    /// Source HDR metadata and diagnostic client display luminance (0x5531).
    pub hdr_source: std::sync::Mutex<crate::display_caps::HdrSource>,
    /// Per-picture present-to-sent reports for a client that asked (0x5536).
    pub host_timings: std::sync::Mutex<crate::host_timing::HostTimings>,
}
impl<P, A> Session<P, A> {
    pub fn new(launch: Launch<P, A>, config: Negotiated) -> Arc<Self> {
        let bitrate = config.bitrate_kbps;
        let mode = crate::reconfigure::Mode::of(&config);
        Arc::new(Self {
            encoder: Default::default(),
            capture_warnings: Default::default(),
            launch,
            config,
            stop: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            idr: AtomicBool::new(true),
            invalidation: Default::default(),
            recovery_wake: Default::default(),
            bitrate: AtomicU32::new(bitrate),
            adaptive_bitrate: Default::default(),
            adaptive_video_pacing_bps: AtomicU64::new(0),
            automatic_pacing: Default::default(),
            keyframe_loss: Default::default(),
            fec_percent: AtomicU32::new(
                crate::adaptive_fec::AdaptiveFec::default().percent() as u32
            ),
            adaptive_fec: Default::default(),
            phase_sync: Default::default(),
            reconfigure: std::sync::Mutex::new(crate::reconfigure::Reconfiguration::new(mode)),
            stats: Stats::default(),
            started: Instant::now(),
            output: std::sync::RwLock::new(String::new()),
            hdr_metadata: Default::default(),
            hdr_source: Default::default(),
            host_timings: Default::default(),
        })
    }
    pub fn stopping(&self) -> bool {
        self.stop.load(Ordering::Acquire)
    }
    /// The size and rate the stream runs at now: the negotiated ones until the
    /// client asks for others (0x5532).
    pub fn stream_mode(&self) -> crate::reconfigure::Mode {
        self.reconfigure.lock().unwrap().current()
    }
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Release)
    }
    pub fn fail(&self) {
        self.failed.store(true, Ordering::Release);
        self.stop();
    }
    pub fn failed(&self) -> bool {
        self.failed.load(Ordering::Acquire)
    }
    /// The termination reason the control stream sends. Moonlight closes
    /// quietly on 0x80030023 (the host closed the stream) and shows an error
    /// with the code otherwise; 0x80004005 is the generic failure HRESULT.
    pub fn termination_reason(&self) -> u32 {
        if self.failed() {
            0x8000_4005
        } else {
            0x8003_0023
        }
    }
    pub fn request_idr(&self) {
        self.stats.idr_requests.fetch_add(1, Ordering::Relaxed);
        // Intra-only frames already recover; feedback must not bypass cadence.
        if self.config.codec != 3 {
            self.idr.store(true, Ordering::Release);
            self.wake_recovery();
        }
    }
    /// A keyframe the client needs after losing frames: its request (0x0302),
    /// or an invalidation the encoder cannot repair with a reference frame.
    pub fn request_recovery_keyframe(&self) {
        let latched = self.keyframe_loss.lock().unwrap().requested(Instant::now());
        self.report_keyframe_loss(latched);
        self.request_idr();
    }
    /// The host could not send enough of a frame for the client to rebuild
    /// it, so the next frame is a keyframe. Moonlight, waiting for one,
    /// then recovers on that frame and sends no request of its own.
    pub fn request_send_loss_recovery(&self) {
        self.stats
            .send_loss_recoveries
            .fetch_add(1, Ordering::Relaxed);
        if self.config.codec != 3 {
            self.idr.store(true, Ordering::Release);
        }
    }
    pub fn request_invalidation(&self, first: u64, last: u64) {
        self.stats
            .reference_invalidations
            .fetch_add(1, Ordering::Relaxed);
        if self.config.codec == 3 {
            return;
        }
        if first == 0 || first > last {
            self.request_recovery_keyframe();
            return;
        }
        let mut pending = self.invalidation.lock().unwrap();
        *pending = Some(pending.map_or((first, last), |(a, b)| (a.min(first), b.max(last))));
        drop(pending);
        self.wake_recovery();
    }
    fn wake_recovery(&self) {
        if let Some(wake) = self.recovery_wake.lock().unwrap().as_ref() {
            wake();
        }
    }
    pub fn record_fec_status(&self, payload: &[u8]) {
        let last_sent = self
            .stats
            .video_frame
            .load(Ordering::Acquire)
            .checked_sub(1)
            .map(|v| v as u32);
        let block = self
            .stats
            .performance
            .lock()
            .unwrap()
            .record_fec_status(payload, last_sent);
        if let Some(block) = block {
            // Parsing and sent-frame/block validation above succeeded. The
            // additional controller fence rejects feedback from an older rate.
            let frame = u32::from_be_bytes(payload[..4].try_into().unwrap());
            if matches!(block, crate::adaptive_fec::Block::Unrecoverable) {
                let latched = self.keyframe_loss.lock().unwrap().unrecoverable(frame);
                self.report_keyframe_loss(latched);
            }
            let period = self.stream_mode().period();
            self.automatic_pacing.lock().unwrap().on_block(
                frame,
                last_sent.unwrap(),
                period,
                block,
            );
            self.adaptive_bitrate.lock().unwrap().on_block(block);
            let mut fec = self.adaptive_fec.lock().unwrap();
            if fec.enabled() {
                fec.on_block(Instant::now(), block);
                self.fec_percent
                    .store(fec.percent() as u32, Ordering::Release);
            }
        }
    }
    /// Starts the stream's FEC at the configured percentage (`fec_percentage`,
    /// 0-100), which adaptive FEC never exceeds. Off, or with a client that never
    /// reports, every frame uses the configured percentage. Returns it.
    pub fn configure_fec(&self, adaptive: bool, configured: usize) -> usize {
        let mut fec = self.adaptive_fec.lock().unwrap();
        *fec = crate::adaptive_fec::AdaptiveFec::new(adaptive, configured);
        let percent = fec.percent();
        self.fec_percent.store(percent as u32, Ordering::Release);
        percent
    }
    /// The FEC percentage for the next video frame.
    pub fn fec_percent(&self) -> usize {
        self.fec_percent.load(Ordering::Acquire) as usize
    }
    /// The FEC percentage for a video frame about to be packetized. A
    /// keyframe or the frame after a reference invalidation (`recovery`)
    /// repairs a loss the client is waiting on, and a keyframe is several
    /// frames' worth of packets: losing it costs another round trip and
    /// another keyframe. Such a frame gets the configured percentage, however
    /// far a clean link let adaptive FEC fall (to 5%) before the loss.
    pub fn frame_fec_percent(&self, recovery: bool) -> usize {
        let current = self.fec_percent();
        if recovery {
            current.max(self.adaptive_fec.lock().unwrap().ceiling())
        } else {
            current
        }
    }
    pub fn configure_adaptive_bitrate(&self, enabled: bool, maximum_kbps: u32) {
        self.adaptive_bitrate.lock().unwrap().configure(
            enabled,
            &self.config,
            maximum_kbps,
            self.fec_percent(),
        );
    }
    pub fn set_client_bitrate(&self, kbps: u32) {
        let mut controller = self.adaptive_bitrate.lock().unwrap();
        controller.client_takeover();
        self.adaptive_video_pacing_bps.store(0, Ordering::Release);
        self.bitrate.store(kbps, Ordering::Release);
        self.launch.warnings.clear("host_bitrate_budget");
    }
    pub fn adaptive_pacing_bps(&self, configured_bps: u64) -> u64 {
        let limit = self.adaptive_video_pacing_bps.load(Ordering::Acquire);
        if limit == 0 {
            configured_bps
        } else {
            configured_bps.min(limit)
        }
    }
    pub fn configure_automatic_pacing(&self, enabled: bool) {
        *self.automatic_pacing.lock().unwrap() =
            crate::adaptive_pacing::AdaptivePacing::new(enabled && self.config.codec != 3);
    }
    pub fn video_pacing_bps(&self, base: u64, needed: u64) -> u64 {
        let paced = self.automatic_pacing.lock().unwrap().rate(
            base,
            needed,
            self.bitrate.load(Ordering::Acquire),
        );
        self.adaptive_pacing_bps(paced)
    }
    /// Pacing slows for the session once the client loses a keyframe
    /// (`keyframe_loss_pacing`). PyroWave sends only intra frames, paced on
    /// their own.
    pub fn configure_keyframe_loss(&self, enabled: bool) {
        *self.keyframe_loss.lock().unwrap() =
            crate::keyframe_loss::KeyframeLoss::new(enabled && self.config.codec != 3);
    }
    fn report_keyframe_loss(&self, latched: Option<&'static str>) {
        if let Some(reason) = latched {
            tracing::warn!(stream_id = %self.launch.id, reason, "keyframes are not arriving; pacing video like a Wi-Fi route for the rest of the stream");
            self.launch.warnings.set("network_keyframe_loss", format!("Keyframes were lost on the way to this device ({reason}), so video is now paced at twice the bitrate, as on Wi-Fi. A port slower than the host or a shallow switch queue is the usual cause; a pacing limit below the device's link speed avoids the first losses."));
        }
    }
    /// The pacing rate for the frame about to be sent as `wire_frame`: the
    /// route policy's `base`, or after a lost keyframe at most `safe`, the
    /// Wi-Fi route's rate for this stream.
    pub fn keyframe_loss_bps(&self, base: u64, safe: u64, wire_frame: u32, keyframe: bool) -> u64 {
        let mut loss = self.keyframe_loss.lock().unwrap();
        loss.sending(wire_frame, keyframe);
        loss.bps(base, safe)
    }
    pub fn record_control_ping(&self, now: Instant, payload: &[u8]) {
        self.adaptive_bitrate.lock().unwrap().on_ping(now, payload);
        self.automatic_pacing.lock().unwrap().on_ping(now, payload);
    }
    pub fn tick_automatic_pacing(&self, now: Instant, rtt: Duration) -> Option<(u64, u64)> {
        let period = self.stream_mode().period();
        self.automatic_pacing.lock().unwrap().tick(
            now,
            crate::adaptive_pacing::Traffic {
                frames: self.stats.frames.load(Ordering::Relaxed),
                bytes: self.stats.bytes.load(Ordering::Relaxed),
                packets: self.stats.packets.load(Ordering::Relaxed),
                send_losses: self.stats.send_loss_recoveries.load(Ordering::Relaxed),
            },
            self.stats
                .video_frame
                .load(Ordering::Acquire)
                .checked_sub(1)
                .map(|v| v as u32),
            period,
            self.bitrate.load(Ordering::Acquire),
            rtt,
        )
    }
    pub fn reserve_controller_pcm(&self, streams: usize) {
        let mut controller = self.adaptive_bitrate.lock().unwrap();
        controller.reserve_controller_pcm(streams, self.fec_percent(), self.stream_mode().period());
        self.adaptive_video_pacing_bps
            .store(controller.video_pacing_bps(), Ordering::Release);
    }
    pub fn tick_adaptive_bitrate(&self, now: Instant, rtt: Duration) -> Option<(u32, u32)> {
        let mut controller = self.adaptive_bitrate.lock().unwrap();
        let previous = self.bitrate.load(Ordering::Acquire);
        let target = controller.tick(
            now,
            crate::adaptive_bitrate::Traffic {
                frames: self.stats.frames.load(Ordering::Relaxed),
                bytes: self.stats.bytes.load(Ordering::Relaxed),
                send_losses: self.stats.send_loss_recoveries.load(Ordering::Relaxed),
            },
            rtt,
            self.fec_percent(),
            self.stream_mode().period(),
            previous,
        );
        self.adaptive_video_pacing_bps
            .store(controller.video_pacing_bps(), Ordering::Release);
        if controller.budget_unusable() {
            self.launch.warnings.set("host_bitrate_budget", "Host bitrate adaptation cannot fit the negotiated audio and minimum video packets inside this network budget, so it is inactive. Use stereo audio, reduce FPS or raise the configured bitrate.");
        } else {
            self.launch.warnings.clear("host_bitrate_budget");
        }
        let target = target?;
        if target == previous {
            return None;
        }
        self.bitrate.store(target, Ordering::Release);
        Some((previous, target))
    }
    /// Lets adaptive FEC fall on a clean link; call regularly. Returns the
    /// `(old, new)` percentage to log, at most once a second.
    pub fn tick_fec(&self, now: Instant) -> Option<(usize, usize)> {
        let mut fec = self.adaptive_fec.lock().unwrap();
        if !fec.enabled() {
            return None;
        }
        fec.tick(now);
        self.fec_percent
            .store(fec.percent() as u32, Ordering::Release);
        fec.log_due(now)
    }
    /// Records the captured source's HDR metadata for the encoder and client.
    pub fn set_display_hdr_metadata(&self, display: crate::hdr::Metadata) -> crate::hdr::Metadata {
        self.hdr_source.lock().unwrap().display = Some(display);
        *self.hdr_metadata.write().unwrap() = Some(display);
        display
    }
    /// Record receiver capabilities for diagnostics. They describe the output
    /// panel, not the source pixels, so must not update stream HDR metadata.
    pub fn record_display_caps(&self, payload: &[u8]) -> Option<crate::display_caps::Update> {
        self.hdr_source.lock().unwrap().on_payload(payload)
    }
    pub fn info(&self) -> serde_json::Value {
        let mut warnings = self.launch.warnings.snapshot();
        warnings.extend(self.capture_warnings.read().unwrap().snapshot());
        // The size and rate the stream runs at: the negotiated ones until the
        // client switches modes (0x5532). The rate is the one encoded, which
        // `limit_framerate` takes from the launch request; the stream
        // request's own rate is `requested_fps`.
        let mode = self.stream_mode();
        let fps = mode.fps_millihz.saturating_add(500) / 1000;
        let (width, height) = (mode.width, mode.height);
        let requested_fps = if mode == crate::reconfigure::Mode::of(&self.config) {
            self.config.fps
        } else {
            fps
        };
        // Sorted after the lock the video sender records into is released.
        let performance = self
            .stats
            .performance
            .lock()
            .unwrap()
            .window(Instant::now());
        serde_json::json!({"warnings":warnings,"encoder":*self.encoder.read().unwrap(),"uuid":self.launch.client.uuid,"device_name":self.launch.client.name,"width":width,"height":height,"fps":fps,"requested_fps":requested_fps,"video_format":self.config.codec,"hdr":self.config.hdr,"vrr":self.config.vrr_low_latency,"encoder_bitrate_kbps":self.bitrate.load(Ordering::Relaxed),"adaptive_bitrate":self.adaptive_bitrate.lock().unwrap().snapshot(),"packet_pacing":self.automatic_pacing.lock().unwrap().snapshot(),"fec_percent":(self.config.codec != 3).then(|| self.fec_percent()),"pyrowave_minimum_kbps":(self.config.codec == 3).then(|| crate::pyrowave::minimum_kbps(self.config.width, self.config.height, self.config.fps_millihz())),"pyrowave_recommended_kbps":(self.config.codec == 3).then(|| crate::pyrowave::recommended_kbps(self.config.width, self.config.height, self.config.fps_millihz())),"audio_channels":self.config.audio_channels,"state":if self.stopping(){"STOPPING"}else{"RUNNING"},"frames_sent":self.stats.frames.load(Ordering::Relaxed),"frames_replaced":self.stats.frames_replaced.load(Ordering::Relaxed),"packets_sent":self.stats.packets.load(Ordering::Relaxed),"bytes_sent":self.stats.bytes.load(Ordering::Relaxed),"idr_requests":self.stats.idr_requests.load(Ordering::Relaxed),"reference_invalidations":self.stats.reference_invalidations.load(Ordering::Relaxed),"send_loss_recoveries":self.stats.send_loss_recoveries.load(Ordering::Relaxed),"encode_latency_ms":self.stats.latency_us.load(Ordering::Relaxed) as f64/1000.,"performance":performance.snapshot(),"uptime_seconds":self.started.elapsed().as_secs_f64(),"role":self.launch.role})
    }
}
pub struct Sessions<P = (), A = ()> {
    pub pending: BTreeMap<String, Launch<P, A>>,
    pub active: BTreeMap<String, Arc<Session<P, A>>>,
    pub teardown: BTreeMap<String, Instant>,
}
impl<P, A> Default for Sessions<P, A> {
    fn default() -> Self {
        Self {
            pending: BTreeMap::new(),
            active: BTreeMap::new(),
            teardown: BTreeMap::new(),
        }
    }
}
impl<P, A> Sessions<P, A> {
    #[must_use = "drop expired launches after releasing the sessions lock"]
    pub fn expire(&mut self) -> Vec<Launch<P, A>> {
        self.remove_pending(|p| !p.live())
    }
    fn remove_pending(
        &mut self,
        mut matches: impl FnMut(&Launch<P, A>) -> bool,
    ) -> Vec<Launch<P, A>> {
        let removed: Vec<_> = self
            .pending
            .iter()
            .filter(|(_, p)| matches(p))
            .map(|(id, _)| id.clone())
            .collect();
        removed
            .iter()
            .filter_map(|id| self.pending.remove(id))
            .collect()
    }
    /// Withdraw a client's launches and streams in one role. Moonlight starts a
    /// stream only after abandoning its previous one, which may never have
    /// connected or may not have timed out yet; either would otherwise block
    /// the new attempt. Returns the removed launches for restoration after unlocking.
    #[must_use = "drop canceled launches after releasing the sessions lock"]
    pub fn supersede(&mut self, client: &str, role: Role) -> Vec<Launch<P, A>> {
        self.stop_role(role, Some(client))
    }
    /// Queue a launch, replacing the same client's earlier launch in its role.
    /// Drop the returned launches after releasing the sessions lock.
    pub fn queue(&mut self, launch: Launch<P, A>) -> Result<Vec<Launch<P, A>>> {
        let replaced =
            |p: &Launch<P, A>| p.client.uuid == launch.client.uuid && p.role == launch.role;
        let streaming = self
            .active
            .values()
            .filter(|s| !s.stopping() && !replaced(&s.launch))
            .count();
        if self
            .pending
            .values()
            .filter(|p| p.live() && !replaced(p))
            .count()
            + streaming
            >= 16
        {
            bail!("session limit reached");
        }
        let mut canceled = self.supersede(&launch.client.uuid, launch.role);
        canceled.extend(self.pending.insert(launch.id.clone(), launch));
        Ok(canceled)
    }
    pub fn pending_for_peer(&self, peer: IpAddr) -> Result<Launch<P, A>> {
        let mut found = self.pending.values().filter(|p| p.peer == peer && p.live());
        let p = found.next().cloned();
        if found.next().is_some() {
            bail!("ambiguous pending session identity");
        }
        p.ok_or_else(|| anyhow::anyhow!("no authorized launch for RTSP peer"))
    }
    /// Authentication and negotiation snapshots only. `start` consumes the
    /// stored pending launch, including its platform preparation, at PLAY.
    pub fn rtsp_for_peer(&self, peer: IpAddr) -> Vec<Launch<P, A>> {
        self.pending
            .values()
            .filter(|p| p.live())
            .chain(
                self.active
                    .values()
                    .filter(|s| !s.stopping())
                    .map(|s| &s.launch),
            )
            .filter(|p| p.peer == peer)
            .map(Launch::rtsp_snapshot)
            .collect()
    }
    pub fn start(
        &mut self,
        launch: Launch<P, A>,
        config: Negotiated,
    ) -> Result<Arc<Session<P, A>>> {
        // RTSP authenticates before awaiting the request. Recheck the stored
        // launch at PLAY so a stale snapshot cannot revive an expired launch.
        // Leave expired preparations for expire() to release outside the lock.
        if self
            .pending
            .get(&launch.id)
            .is_none_or(|pending| !pending.live())
        {
            bail!("launch expired or already consumed");
        }
        let launch = self.pending.remove(&launch.id).unwrap();
        let s = Session::new(launch, config);
        self.active.insert(s.launch.id.clone(), s.clone());
        Ok(s)
    }
    pub fn begin_teardown(&mut self, session: &Session<P, A>) {
        self.active.remove(&session.launch.id);
        self.teardown
            .insert(session.launch.id.clone(), Instant::now());
    }
    pub fn fail_start(&mut self, session: &Session<P, A>) {
        session.fail();
        self.active.remove(&session.launch.id);
    }
    #[must_use = "drop canceled launches after releasing the sessions lock"]
    pub fn request_stop(&mut self, id: Option<&str>) -> Vec<Launch<P, A>> {
        let canceled =
            self.remove_pending(|p| id.is_none_or(|id| p.client.uuid == id || p.id == id));
        for s in self.active.values() {
            if id.is_none_or(|id| s.launch.client.uuid == id || s.launch.id == id) {
                s.stop();
            }
        }
        canceled
    }
    pub fn owns_capture(&self) -> bool {
        !self.pending.is_empty() || !self.active.is_empty() || !self.teardown.is_empty()
    }
    /// No stream runs, tears down or is about to start. Unlike
    /// `owns_capture`, an expired launch that nothing has removed yet, such
    /// as one the client abandoned, does not count.
    pub fn idle(&self) -> bool {
        self.active.is_empty()
            && self.teardown.is_empty()
            && !self.pending.values().any(Launch::live)
    }
    #[must_use = "drop canceled launches after releasing the sessions lock"]
    pub fn stop_role(&mut self, role: Role, client: Option<&str>) -> Vec<Launch<P, A>> {
        let canceled =
            self.remove_pending(|p| p.role == role && client.is_none_or(|id| p.client.uuid == id));
        for s in self.active.values() {
            if s.launch.role == role && client.is_none_or(|id| id == s.launch.client.uuid) {
                s.stop();
            }
        }
        canceled
    }
}
