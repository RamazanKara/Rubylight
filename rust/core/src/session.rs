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
        atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering},
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

/// The session warning code while the link-rate controller holds the video
/// bitrate below the requested one.
pub const LINK_WARNING: &str = "network_bitrate_limited";

/// Update the bookkeeping for a change, under the lock, and return what to
/// say about it.
fn note_link_change(
    link: &mut crate::link_control::LinkControl,
    now: Instant,
    change: crate::link_rate::Change,
) -> LinkReport {
    use crate::link_rate::ChangeKind;
    let was_limited = std::mem::replace(&mut link.limited, change.kind != ChangeKind::Full);
    match change.kind {
        ChangeKind::StepDown | ChangeKind::Floor | ChangeKind::EncoderFloor => {
            if change.evidence.loss_share > 0. {
                link.loss_share = change.evidence.loss_share;
            }
            link.lowest_kbps = match link.lowest_kbps {
                0 => change.to_kbps,
                lowest => lowest.min(change.to_kbps),
            };
        }
        ChangeKind::Full => link.lowest_kbps = 0,
        ChangeKind::Probe | ChangeKind::ProbeFailed => {}
    }
    // Routine changes go to the log at most once a second; the first
    // step down and the floors have their own WARN.
    let warned = match change.kind {
        ChangeKind::StepDown => !was_limited,
        ChangeKind::Floor | ChangeKind::EncoderFloor => true,
        _ => false,
    };
    let log_info = !warned
        && link
            .logged
            .is_none_or(|at| now.saturating_duration_since(at) >= Duration::from_secs(1));
    if log_info {
        link.logged = Some(now);
    }
    LinkReport {
        change,
        was_limited,
        ceiling_kbps: link.rate.ceiling_kbps(),
        loss_share: link.loss_share,
        lowest_kbps: link.lowest_kbps,
        log_info,
    }
}

/// What to say about a link-rate change, worked out under the controller's
/// lock and said after it.
struct LinkReport {
    change: crate::link_rate::Change,
    was_limited: bool,
    ceiling_kbps: u32,
    loss_share: f64,
    lowest_kbps: u32,
    log_info: bool,
}

/// What a client that asked for notices (0x5538) is told about a change.
fn link_notice(report: &LinkReport) -> crate::link_notice::LinkNotice {
    use crate::link_notice::{LinkNotice, LinkReason, LinkState};
    use crate::link_rate::ChangeKind;
    let (state, reason) = match report.change.kind {
        ChangeKind::Full => (LinkState::Full, LinkReason::None),
        ChangeKind::Floor | ChangeKind::EncoderFloor => (LinkState::Floor, LinkReason::Network),
        ChangeKind::StepDown | ChangeKind::Probe | ChangeKind::ProbeFailed => {
            (LinkState::Limited, LinkReason::Network)
        }
    };
    let current_kbps = report.change.to_kbps.max(1);
    LinkNotice {
        state,
        reason,
        ceiling_kbps: report.ceiling_kbps.max(current_kbps),
        current_kbps,
    }
}

/// A bitrate for a sentence: "15 Mbps", "4.5 Mbps".
fn mbps(kbps: u32) -> String {
    if kbps >= 10_000 {
        format!("{} Mbps", (f64::from(kbps) / 1000.).round())
    } else {
        format!("{:.1} Mbps", f64::from(kbps) / 1000.)
    }
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
        capture.set("capture", "Desktop Duplication in use");
        launch.warnings.set("display", "Physical display in use");
        assert_eq!(session.info()["encoder"], "amf");
        assert_eq!(
            session.info()["warnings"],
            serde_json::json!([
                {"code":"display", "message":"Physical display in use"},
                {"code":"capture", "message":"Desktop Duplication in use"}
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
    fn a_roam_moves_the_client_and_restarts_the_media_ping_wait() {
        let mut launch = launch("roam", Role::Stream);
        launch.peer = "::ffff:192.168.1.20".parse().unwrap();
        let session = Session::new(launch, Negotiated::default());
        let start = Instant::now();
        assert_eq!(
            session.client_ip(),
            "192.168.1.20".parse::<IpAddr>().unwrap()
        );
        assert_eq!(session.media_wait_start(start), start);
        assert!(!session.take_new_path());
        let at = start + Duration::from_secs(60);
        session.roamed("::ffff:10.0.0.7".parse().unwrap(), at);
        assert_eq!(session.client_ip(), "10.0.0.7".parse::<IpAddr>().unwrap());
        assert_eq!(session.roams(), 1);
        assert_eq!(session.media_wait_start(start), at);
        assert!(session.take_new_path(), "the first ping from the new path");
        assert!(!session.take_new_path(), "once");
    }
    #[test]
    fn a_new_path_forgets_the_old_paths_pacing_estimate_and_keyframe_loss() {
        let session = Session::new(launch("new-path", Role::Stream), Negotiated::default());
        session.configure_automatic_pacing(true);
        session.configure_keyframe_loss(true);
        let lose = |session: &Session| {
            for frame in 10..=12 {
                session.keyframe_loss.lock().unwrap().unrecoverable(
                    frame,
                    frame,
                    Duration::from_millis(16),
                    Instant::now(),
                );
            }
        };
        lose(&session);
        assert!(session.keyframe_loss_active());
        session.video_pacing_bps(100_000_000, 10_000_000);
        let ceiling = |session: &Session| {
            session.automatic_pacing.lock().unwrap().snapshot()["ceiling_kbps"].clone()
        };
        assert_ne!(ceiling(&session), 0);
        session.configure_adaptive_bitrate(true, 0);
        {
            let mut bitrate = session.adaptive_bitrate.lock().unwrap();
            bitrate.on_block(crate::adaptive_fec::Block::Clean);
            let start = Instant::now();
            for i in 0..2 {
                let now = start + Duration::from_secs(i);
                bitrate.on_ping(now, &[4, 0, 0, 0, 0, 0, 0, 0]);
                bitrate.tick(
                    now,
                    Default::default(),
                    Duration::from_millis(2),
                    20,
                    Duration::from_millis(16),
                    20_000,
                );
            }
            assert_eq!(bitrate.minimum_rtt(), Some(Duration::from_millis(2)));
        }
        session.new_network_path();
        assert!(!session.keyframe_loss_active());
        assert_eq!(ceiling(&session), 0);
        assert_eq!(session.adaptive_bitrate.lock().unwrap().minimum_rtt(), None);
        assert!(session.automatic_pacing_enabled());
        // The guard still watches the new path.
        lose(&session);
        assert!(session.keyframe_loss_active());
    }
    #[test]
    fn a_roam_drops_the_clients_hints_about_the_old_path() {
        let session = Session::new(
            launch("hints", Role::Stream),
            Negotiated {
                codec: 3,
                bitrate_kbps: 200_000,
                configured_bitrate_kbps: 250_000,
                pyrowave_client_link_mbps: 100,
                pyrowave_client_pace_mbps: 150,
                ..Default::default()
            },
        );
        let ceiling = |session: &Session| {
            session.configure_adaptive_bitrate(true, 0);
            session.adaptive_bitrate.lock().unwrap().snapshot()["wire_ceiling_kbps"].clone()
        };
        assert_eq!(
            (session.client_link_mbps(), session.client_pace_mbps()),
            (100, 150)
        );
        assert_eq!(ceiling(&session), 95_000);
        session.roamed("10.0.0.7".parse().unwrap(), Instant::now());
        assert_eq!(
            (session.client_link_mbps(), session.client_pace_mbps()),
            (0, 0)
        );
        assert_eq!(ceiling(&session), 250_000);
    }
    #[test]
    fn the_console_gets_pipeline_rates_once_a_window_is_complete() {
        let session = Session::new(launch("rates", Role::Stream), Negotiated::default());
        assert!(session.info()["pipeline_rates"].is_null());
        session.stats.pipeline.captured(1, 2);
        session.stats.pipeline.submitted(true);
        session.stats.frames.fetch_add(1, Ordering::Relaxed);
        assert_eq!(
            session.pipeline_counts(),
            crate::pipeline_rates::Counts {
                source: 1,
                capture: 2,
                submit: 1,
                output: 1,
                repeat: 1,
            }
        );
        std::thread::sleep(crate::pipeline_rates::CONSOLE_WINDOW);
        let rates = &session.info()["pipeline_rates"];
        assert!(rates["window_ms"].as_u64().unwrap() >= 1000, "{rates}");
        assert!(rates["capture_mhz"].as_u64().unwrap() > 0, "{rates}");
        assert!(rates["repeat_mhz"].as_u64().unwrap() > 0, "{rates}");
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
    /// A session with the link controller on, `frames` frames of 100 packets
    /// sent 17 ms apart ending now, and the wire frame marker past them.
    fn link_session(frames: u32, enabled: bool) -> (Arc<Session>, Instant) {
        let s = Session::new(
            launch("link-rate", Role::Stream),
            Negotiated {
                bitrate_kbps: 20_000,
                configured_bitrate_kbps: 20_000,
                ..Default::default()
            },
        );
        s.configure_link_rate(enabled, 0);
        let end = Instant::now();
        for frame in 1..=frames {
            let at = end - Duration::from_millis(17 * u64::from(frames - frame));
            s.record_link_sent_at(frame, 100, 112_600, at);
        }
        s.stats
            .video_frame
            .store(u64::from(frames) + 10, Ordering::Release);
        (s, end)
    }
    fn lose_frames(s: &Session, frames: std::ops::RangeInclusive<u32>) {
        for frame in frames {
            s.record_fec_status(&fec_report(frame, 7, 1));
        }
    }
    #[test]
    fn a_burst_of_lost_frames_lowers_the_bitrate_and_says_so() {
        let (s, end) = link_session(60, true);
        assert_eq!(s.info()["link_rate"]["state"], "full");
        lose_frames(&s, 50..=54);
        let (from, to) = s
            .tick_link_rate(end + Duration::from_millis(200), Duration::from_millis(10))
            .expect("the controller lowers the bitrate");
        assert_eq!(from, 20_000);
        assert!((20_000 * 40 / 100..20_000 * 85 / 100).contains(&to), "{to}");
        assert_eq!(s.bitrate.load(Ordering::Acquire), to);
        let info = s.info();
        assert_eq!(info["link_rate"]["state"], "limited");
        let warning = info["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .find(|w| w["code"] == LINK_WARNING)
            .expect("the console gets a warning")
            .clone();
        assert!(
            warning["message"].as_str().unwrap().contains("Mbps"),
            "{warning}"
        );
        // Later ticks do not change the warning or the bitrate while it holds.
        assert_eq!(
            s.tick_link_rate(end + Duration::from_millis(300), Duration::from_millis(10)),
            None
        );
    }
    fn ask_for_notices(s: &Session) {
        use crate::link_notice::LinkNoticeRequest;
        assert_eq!(
            s.link_notices
                .lock()
                .unwrap()
                .on_request(&LinkNoticeRequest { enabled: true }.encode()),
            Some(true)
        );
    }
    fn notice_at(s: &Session, at: Instant) -> Option<crate::link_notice::LinkNotice> {
        let bytes = s.link_notices.lock().unwrap().take(at)?;
        Some(crate::link_notice::LinkNotice::decode(&bytes).expect("the host sends valid notices"))
    }
    #[test]
    fn a_client_that_asked_is_told_when_the_bitrate_is_lowered_and_when_it_is_back() {
        use crate::link_notice::{LinkReason, LinkState};
        let (s, end) = link_session(60, true);
        ask_for_notices(&s);
        let at = end + Duration::from_millis(200);
        assert_eq!(notice_at(&s, at), None, "nothing to say at the full rate");
        lose_frames(&s, 50..=54);
        let (_, to) = s
            .tick_link_rate(at, Duration::from_millis(10))
            .expect("the controller lowers the bitrate");
        let notice = notice_at(&s, at).expect("the step down is announced");
        assert_eq!(notice.state, LinkState::Limited);
        assert_eq!(notice.reason, LinkReason::Network);
        assert_eq!((notice.ceiling_kbps, notice.current_kbps), (20_000, to));
        assert_eq!(notice_at(&s, at + Duration::from_millis(8)), None, "once");
        // A reconfigure puts the requested bitrate back and the client hears it.
        s.rebuild_link_rate(&s.config, true, 0);
        assert_eq!(s.bitrate.load(Ordering::Acquire), 20_000);
        let notice = notice_at(&s, at + Duration::from_secs(1)).expect("the return is announced");
        assert_eq!(notice.state, LinkState::Full);
        assert_eq!(notice.reason, LinkReason::None);
        assert_eq!((notice.ceiling_kbps, notice.current_kbps), (20_000, 20_000));
    }
    #[test]
    fn a_client_that_did_not_ask_is_told_nothing() {
        let (s, end) = link_session(60, true);
        lose_frames(&s, 50..=54);
        let at = end + Duration::from_millis(200);
        s.tick_link_rate(at, Duration::from_millis(10))
            .expect("the controller lowers the bitrate");
        assert_eq!(notice_at(&s, at), None);
        assert_eq!(notice_at(&s, at + Duration::from_secs(5)), None);
    }
    #[test]
    fn a_client_that_takes_the_bitrate_over_is_told_it_is_no_longer_lowered() {
        use crate::link_notice::LinkState;
        let (s, end) = link_session(60, true);
        ask_for_notices(&s);
        lose_frames(&s, 50..=54);
        let at = end + Duration::from_millis(200);
        s.tick_link_rate(at, Duration::from_millis(10)).unwrap();
        assert_eq!(notice_at(&s, at).unwrap().state, LinkState::Limited);
        s.set_client_bitrate(12_000);
        let notice = notice_at(&s, at + Duration::from_secs(1)).expect("cleared");
        assert_eq!(notice.state, LinkState::Full);
        assert_eq!((notice.ceiling_kbps, notice.current_kbps), (12_000, 12_000));
    }
    #[test]
    fn every_controller_change_has_its_notice() {
        use crate::link_notice::{LinkReason, LinkState};
        use crate::link_rate::{Change, ChangeKind, Evidence};
        for (kind, state, reason) in [
            (
                ChangeKind::StepDown,
                LinkState::Limited,
                LinkReason::Network,
            ),
            (ChangeKind::Probe, LinkState::Limited, LinkReason::Network),
            (
                ChangeKind::ProbeFailed,
                LinkState::Limited,
                LinkReason::Network,
            ),
            (ChangeKind::Floor, LinkState::Floor, LinkReason::Network),
            (
                ChangeKind::EncoderFloor,
                LinkState::Floor,
                LinkReason::Network,
            ),
            (ChangeKind::Full, LinkState::Full, LinkReason::None),
        ] {
            let to_kbps = if kind == ChangeKind::Full {
                20_000
            } else {
                5_000
            };
            let notice = link_notice(&LinkReport {
                change: Change {
                    kind,
                    reason: "test",
                    from_kbps: 10_000,
                    to_kbps,
                    delivered_kbps: 0,
                    evidence: Evidence::default(),
                },
                was_limited: true,
                ceiling_kbps: 20_000,
                loss_share: 0.,
                lowest_kbps: 5_000,
                log_info: false,
            });
            assert_eq!(
                (
                    notice.state,
                    notice.reason,
                    notice.ceiling_kbps,
                    notice.current_kbps
                ),
                (state, reason, 20_000, to_kbps),
                "{kind:?}"
            );
        }
    }
    #[test]
    fn a_healthy_session_keeps_its_bitrate_and_has_no_warning() {
        let (s, end) = link_session(120, true);
        // Frames that FEC repaired are not loss the controller acts on.
        for frame in (10..120).step_by(7) {
            s.record_fec_status(&fec_report(frame, 9, 1));
        }
        for ms in (0..3000).step_by(8) {
            assert_eq!(
                s.tick_link_rate(end + Duration::from_millis(ms), Duration::from_millis(10)),
                None
            );
        }
        assert_eq!(s.bitrate.load(Ordering::Acquire), 20_000);
        let info = s.info();
        assert_eq!(info["link_rate"]["state"], "full");
        assert!(
            info["warnings"]
                .as_array()
                .unwrap()
                .iter()
                .all(|w| w["code"] != LINK_WARNING)
        );
    }
    #[test]
    fn the_controller_is_off_when_disabled_or_for_pyrowave() {
        let (s, end) = link_session(60, false);
        lose_frames(&s, 50..=54);
        assert_eq!(
            s.tick_link_rate(end + Duration::from_millis(200), Duration::ZERO),
            None
        );
        assert_eq!(s.bitrate.load(Ordering::Acquire), 20_000);
        assert_eq!(s.info()["link_rate"]["state"], "off");

        let p = Session::new(
            launch("link-pyrowave", Role::Stream),
            Negotiated {
                codec: 3,
                bitrate_kbps: 200_000,
                ..Default::default()
            },
        );
        p.configure_link_rate(true, 0);
        p.record_link_sent(1, 100, 112_600);
        assert_eq!(p.link_sent.head(), 0, "nothing is recorded for PyroWave");
        assert_eq!(p.info()["link_rate"]["reason"], "pyrowave");
    }
    #[test]
    fn a_client_that_sets_the_bitrate_clears_the_warning_and_restores_nothing() {
        let (s, end) = link_session(60, true);
        lose_frames(&s, 50..=54);
        s.tick_link_rate(end + Duration::from_millis(200), Duration::from_millis(10))
            .unwrap();
        assert!(
            s.launch
                .warnings
                .snapshot()
                .iter()
                .any(|w| w.code == LINK_WARNING)
        );
        s.set_client_bitrate(12_000);
        assert_eq!(s.bitrate.load(Ordering::Acquire), 12_000);
        assert!(
            s.launch
                .warnings
                .snapshot()
                .iter()
                .all(|w| w.code != LINK_WARNING)
        );
        assert_eq!(s.info()["link_rate"]["reason"], "client_controlled");
        lose_frames(&s, 55..=59);
        assert_eq!(
            s.tick_link_rate(end + Duration::from_secs(1), Duration::from_millis(10)),
            None
        );
        assert_eq!(s.bitrate.load(Ordering::Acquire), 12_000);
        // A reconfigure keeps the client in charge and does not bring back the
        // bitrate the host had lowered.
        s.rebuild_link_rate(&s.config, true, 0);
        assert_eq!(s.bitrate.load(Ordering::Acquire), 12_000);
        assert_eq!(s.info()["link_rate"]["reason"], "client_controlled");
    }
    /// One block of a client FEC report: `data` and `parity` packets, of which
    /// `received` arrived (data first).
    fn fec_block(frame: u32, block: u8, blocks: u8, data: u16, received: u16) -> [u8; 21] {
        let parity = (u32::from(data) * 20).div_ceil(100) as u16;
        let mut report = [0; 21];
        report[..4].copy_from_slice(&frame.to_be_bytes());
        for (offset, word) in [(10, data), (12, parity), (14, received.min(data))] {
            report[offset..offset + 2].copy_from_slice(&word.to_be_bytes());
        }
        report[16..18].copy_from_slice(&received.saturating_sub(data).to_be_bytes());
        report[18] = 20;
        report[19] = block;
        report[20] = blocks;
        report
    }
    #[test]
    fn a_keyframe_the_host_abandoned_is_not_counted_again_as_network_loss() {
        // Two 300-packet keyframes in two loss windows: every packet of both
        // is missing at the client, 7 % of what was sent in each window.
        for abandoned in [true, false] {
            let s = Session::new(
                launch("link-abandoned", Role::Stream),
                Negotiated {
                    bitrate_kbps: 20_000,
                    configured_bitrate_kbps: 20_000,
                    ..Default::default()
                },
            );
            s.configure_link_rate(true, 0);
            s.configure_keyframe_loss(true);
            s.stats.video_frame.store(500, Ordering::Release);
            let base = Instant::now();
            let rtt = Duration::from_millis(10);
            assert_eq!(s.tick_link_rate(base, rtt), None);
            let mut cut = None;
            for window in 0..2u32 {
                let first = 1 + window * 41;
                for frame in first..first + 40 {
                    let at = base
                        + Duration::from_millis(
                            u64::from(window) * 600 + u64::from(frame - first) * 10,
                        );
                    s.record_link_sent_at(frame, 100, 112_600, at);
                }
                let keyframe = first + 40;
                s.record_link_sent_at(
                    keyframe,
                    300,
                    337_800,
                    base + Duration::from_millis(u64::from(window) * 600 + 400),
                );
                if abandoned {
                    s.keyframe_loss.lock().unwrap().sending(keyframe, true);
                    s.keyframe_abandoned(keyframe);
                }
                s.record_fec_status(&fec_block(keyframe, 0, 2, 125, 0));
                s.record_fec_status(&fec_block(keyframe, 1, 2, 125, 0));
                cut = s.tick_link_rate(
                    base + Duration::from_millis(u64::from(window + 1) * 600),
                    rtt,
                );
            }
            assert_eq!(
                cut.is_some(),
                !abandoned,
                "abandoned {abandoned}: {}",
                s.link_rate_snapshot()
            );
            if abandoned {
                assert_eq!(s.link_rate_snapshot()["state"], "full");
            }
        }
    }
    #[test]
    fn pacing_counts_as_fast_only_when_the_applied_rate_is_above_the_wifi_rate() {
        let s = Session::new(launch("link-pace", Role::Stream), Negotiated::default());
        s.configure_keyframe_loss(true);
        assert_eq!(
            s.keyframe_loss_bps(800_000_000, 62_000_000, 1, false),
            800_000_000
        );
        assert!(s.pace_was_fast.load(Ordering::Relaxed));
        // Once slowed to the Wi-Fi rate the route's own rate is not what ran.
        s.configure_remembered_keyframe_loss(true, true);
        assert_eq!(
            s.keyframe_loss_bps(800_000_000, 62_000_000, 2, false),
            62_000_000
        );
        assert!(!s.pace_was_fast.load(Ordering::Relaxed));
    }
    const FAST: u64 = 800_000_000;
    const SAFE: u64 = 62_000_000;
    const FRAME: Duration = Duration::from_micros(16_667);
    fn fence_at(s: &Session) -> Option<Instant> {
        s.link_rate.lock().unwrap().rate.fence_at()
    }
    /// A link session whose first keyframe (wire frame 1) was lost at `t0`,
    /// with the pace `base` requested for every frame.
    fn first_keyframe_lost(base: u64, t0: Instant) -> Arc<Session> {
        let (s, _) = link_session(60, true);
        s.configure_keyframe_loss(true);
        s.keyframe_loss_bps_at(base, SAFE, 1, true, t0);
        s.keyframe_loss_bps_at(base, SAFE, 2, false, t0 + FRAME);
        let latched = s
            .keyframe_loss
            .lock()
            .unwrap()
            .unrecoverable(1, 2, FRAME, t0 + Duration::from_millis(100))
            .unwrap();
        assert_eq!(latched.kind, crate::keyframe_loss::LatchKind::Startup);
        s.report_keyframe_loss(Some(latched), None);
        s
    }
    #[test]
    fn a_latch_confirmed_by_more_evidence_does_not_start_a_second_fence() {
        let t0 = Instant::now();
        let s = first_keyframe_lost(FAST, t0);
        let first = fence_at(&s).expect("the first latch is a reaction to fast pacing");
        // Three more distinct frames confirm the same latch; it is not new.
        let at = t0 + Duration::from_secs(1);
        let confirmed = {
            let mut loss = s.keyframe_loss.lock().unwrap();
            // More reports about the same keyframe are not new evidence.
            assert!(loss.unrecoverable(1, 2, FRAME, at).is_none());
            assert!(loss.abandoned(1, FRAME, at).is_none());
            for frame in 3..=4 {
                loss.sending_at(frame, false, at);
                assert!(loss.unrecoverable(frame, 4, FRAME, at).is_none());
            }
            loss.sending_at(5, false, at);
            loss.unrecoverable(5, 5, FRAME, at)
        }
        .expect("more evidence confirms the latch");
        assert!(confirmed.confirmed);
        std::thread::sleep(Duration::from_millis(5));
        s.report_keyframe_loss(Some(confirmed), None);
        assert_eq!(fence_at(&s), Some(first));
    }
    #[test]
    fn a_latch_is_a_reaction_to_fast_pacing_only_if_fast_pacing_was_applied() {
        // The route allows 800 Mbps, but the frame went out at 50 Mbps (the
        // client's own link): the latch cannot be the whole fix.
        let s = first_keyframe_lost(50_000_000, Instant::now());
        assert!(!s.pace_was_fast.load(Ordering::Relaxed));
        assert_eq!(fence_at(&s), None);
        // The same loss after frames that really went out fast starts a fence.
        let s = first_keyframe_lost(FAST, Instant::now());
        assert!(fence_at(&s).is_some());
        // A frame paced at the Wi-Fi rate because of the latch is not fast,
        // whatever the route would have allowed.
        assert_eq!(
            s.keyframe_loss_bps_at(FAST, SAFE, 3, false, Instant::now()),
            SAFE
        );
        assert!(!s.pace_was_fast.load(Ordering::Relaxed));
    }
    #[test]
    fn a_lifted_start_up_latch_gives_link_rate_one_fence_for_the_faster_pace() {
        for (base, fenced) in [(FAST, true), (50_000_000, false)] {
            let t0 = Instant::now();
            let s = first_keyframe_lost(base, t0);
            let held = fence_at(&s);
            assert_eq!(held.is_some(), fenced);
            let mut frame = 3;
            while s.keyframe_loss_active() {
                let at = t0 + FRAME * frame;
                s.keyframe_loss_bps_at(base, SAFE, frame, false, at);
                frame += 1;
                assert!(frame < 2_000, "the latch never lifted");
            }
            assert_eq!(s.pace_was_fast.load(Ordering::Relaxed), base > SAFE);
            if fenced {
                let lifted = fence_at(&s).unwrap();
                assert!(Some(lifted) > held, "the lift starts a fence");
            } else {
                assert_eq!(fence_at(&s), None, "the pace did not change");
            }
            assert!(s.launch.warnings.snapshot().is_empty());
        }
    }
    fn link_warning(s: &Session) -> Option<String> {
        s.launch
            .warnings
            .snapshot()
            .into_iter()
            .find(|w| w.code == LINK_WARNING)
            .map(|w| w.message)
    }
    #[test]
    fn the_warning_and_the_console_agree_on_the_bitrate_and_nothing_is_said_under_the_lock() {
        let (s, end) = link_session(60, true);
        lose_frames(&s, 50..=54);
        // The change is worked out under the lock; the warning is not set
        // until the lock is released.
        let now = end + Duration::from_millis(200);
        let mut link = s.link_rate.lock().unwrap();
        link.drain(&s.link_sent);
        link.rate
            .tick(
                now,
                Duration::from_millis(10),
                s.fec_percent(),
                s.stream_mode().period(),
                20_000,
                true,
            )
            .expect("the controller lowers the bitrate");
        let change = link.rate.take_change().unwrap();
        let report = note_link_change(&mut link, now, change);
        drop(link);
        assert_eq!(link_warning(&s), None);
        s.announce_link_change(report);
        let message = link_warning(&s).expect("the console gets a warning");
        let lowest = s.link_rate.lock().unwrap().lowest_kbps;
        // "As low as", never a bare "lowered the bitrate to X", which the
        // badge's current rate would contradict once the host probes upward.
        assert!(
            message.contains(&format!("as low as {} so far", mbps(lowest))),
            "{message}"
        );
        assert!(!message.contains("bitrate to"), "{message}");
        assert!(message.contains("20 Mbps"), "{message}");
    }
    #[test]
    fn an_encoder_floor_says_so_without_claiming_the_lowest_bitrate_is_too_high() {
        let s = Session::new(launch("link-floor", Role::Stream), Negotiated::default());
        let change = crate::link_rate::Change {
            kind: crate::link_rate::ChangeKind::EncoderFloor,
            reason: "encoder_floor",
            from_kbps: 9_000,
            to_kbps: 14_000,
            delivered_kbps: 17_000,
            evidence: Default::default(),
        };
        let mut link = s.link_rate.lock().unwrap();
        let report = note_link_change(&mut link, Instant::now(), change);
        drop(link);
        s.announce_link_change(report);
        let message = link_warning(&s).unwrap();
        assert!(message.contains("14 Mbps"), "{message}");
        assert!(message.contains("does not go lower"), "{message}");
        assert!(!message.contains("cannot carry even"), "{message}");
    }
    #[test]
    fn a_queue_cut_says_the_network_held_video_back_not_that_packets_were_lost() {
        let s = Session::new(launch("link-queue", Role::Stream), Negotiated::default());
        let change = crate::link_rate::Change {
            kind: crate::link_rate::ChangeKind::StepDown,
            reason: "queue",
            from_kbps: 36_000,
            to_kbps: 31_700,
            delivered_kbps: 0,
            evidence: Default::default(),
        };
        let mut link = s.link_rate.lock().unwrap();
        let report = note_link_change(&mut link, Instant::now(), change);
        drop(link);
        s.announce_link_change(report);
        let message = link_warning(&s).expect("the console gets a warning");
        assert!(
            message.contains("holding video back in a growing queue"),
            "{message}"
        );
        assert!(!message.contains("lost"), "{message}");
        assert!(message.contains("as low as"), "{message}");
    }
    #[test]
    fn keyframe_requests_count_only_for_a_client_that_sends_no_fec_status() {
        for reports_fec in [false, true] {
            let (s, end) = link_session(60, true);
            if reports_fec {
                s.record_fec_status(&fec_report(40, 10, 2));
            }
            for _ in 0..4 {
                s.request_client_keyframe();
            }
            let cut = s.tick_link_rate(end + Duration::from_millis(200), Duration::from_millis(10));
            assert_eq!(cut.is_some(), !reports_fec, "reports_fec {reports_fec}");
        }
    }
    fn pacing_traffic(s: &Session, frames: u64) {
        s.stats.frames.store(frames, Ordering::Relaxed);
        s.stats.packets.store(frames * 70, Ordering::Relaxed);
        s.stats.bytes.store(frames * 83_334, Ordering::Relaxed);
        // The wire frame marker stores last_sent + 1, not last_sent.
        s.stats.video_frame.store(frames + 1, Ordering::Release);
    }
    #[test]
    fn pyrowave_loss_controls_adapt_without_forcing_extra_frames() {
        for records in [false, true] {
            let s = Session::new(
                launch("pyrowave-loss", Role::Stream),
                Negotiated {
                    codec: 3,
                    pyrowave_records: records,
                    bitrate_kbps: 200_000,
                    configured_bitrate_kbps: 250_000,
                    pyrowave_client_pace_mbps: 150,
                    ..Default::default()
                },
            );
            s.idr.store(false, Ordering::Release);
            s.configure_keyframe_loss(true);
            s.configure_adaptive_bitrate(true, 0);
            let start = Instant::now();
            s.tick_adaptive_bitrate(start, Duration::from_millis(5));
            s.keyframe_loss_bps(1_000_000_000, 250_000_000, 1, true);
            s.stats.video_frame.store(2, Ordering::Release);
            s.record_fec_status(&fec_report(1, 7, 1));
            assert_eq!(
                s.keyframe_loss_bps(1_000_000_000, 250_000_000, 2, true),
                250_000_000
            );
            let (_, target) = s
                .tick_adaptive_bitrate(start + Duration::from_secs(1), Duration::from_millis(5))
                .unwrap();
            assert!(target < 150_000);
            assert!(s.adaptive_pacing_bps(u64::MAX) <= 150_000_000);
            assert!(!s.idr.load(Ordering::Acquire));
            assert!(s.invalidation.lock().unwrap().is_none());
            s.set_client_bitrate(180_000);
            s.record_fec_status(&fec_report(2, 7, 1));
            s.tick_adaptive_bitrate(start + Duration::from_secs(3), Duration::from_millis(5));
            assert_eq!(s.bitrate.load(Ordering::Acquire), 180_000);
        }
    }

    #[test]
    fn pyrowave_packet_adaptation_responds_to_distinct_lost_frames() {
        let s = Session::new(
            launch("pyrowave-pacing", Role::Stream),
            Negotiated {
                codec: 3,
                bitrate_kbps: 40_000,
                ..Default::default()
            },
        );
        s.configure_automatic_pacing(true);
        let start = Instant::now();
        assert_eq!(s.video_pacing_bps(150_000_000, 50_000_000), 80_000_000);
        s.tick_automatic_pacing(start, Duration::from_millis(2));
        pacing_traffic(&s, 60);
        s.record_fec_status(&fec_report(59, 7, 1));
        s.record_fec_status(&fec_report(60, 7, 1));
        assert_eq!(
            s.tick_automatic_pacing(start + Duration::from_secs(1), Duration::from_millis(2)),
            Some((80_000_000, 62_500_000))
        );
        assert_eq!(s.bitrate.load(Ordering::Acquire), 40_000);
        // A measured path below demand remains the hard ceiling.
        assert_eq!(s.video_pacing_bps(30_000_000, 50_000_000), 30_000_000);
    }

    #[test]
    fn a_wireless_client_stream_starts_at_the_controllers_seed_and_climbs_toward_the_route_ceiling()
    {
        let config = crate::config::Config::default();
        for (wireless, expected) in [(true, 126_000_000), (false, 800_000_000)] {
            let negotiated = Negotiated {
                codec: 1,
                bitrate_kbps: 63_000,
                client_wireless: wireless,
                ..Default::default()
            };
            let s = Session::new(launch("wireless-start", Role::Stream), negotiated);
            s.configure_automatic_pacing(
                config.adaptive_packet_pacing(s.config.codec, s.config.client_wireless),
            );
            // The sender passes the host route's base (800 Mbps on a wired 1 GbE
            // host) to the controller: its seed gives the conservative start.
            assert_eq!(s.automatic_pacing_enabled(), wireless);
            assert!(!s.client_wireless_sets_route_pacing());
            assert_eq!(s.video_pacing_bps(800_000_000, 75_000_000), expected);
            if !wireless {
                continue;
            }
            // Clean feedback lets the rate climb above twice the bitrate, in
            // steps of at most 12.5%, and never past the route ceiling.
            let start = Instant::now();
            s.tick_automatic_pacing(start, Duration::from_millis(2));
            let mut rate = expected;
            for step in 1..=40u64 {
                let now = start + Duration::from_millis(500 * step);
                pacing_traffic(&s, 30 * step);
                s.record_control_ping(now, &[4, 0, 0, 0, 0, 0, 0, 0]);
                s.tick_automatic_pacing(now, Duration::from_millis(2));
                let next = s.video_pacing_bps(800_000_000, 75_000_000);
                assert!(next >= rate && next <= rate + rate / 8, "{rate} -> {next}");
                assert!(next <= 800_000_000);
                rate = next;
            }
            assert!(rate > 2 * 63_000_000, "never climbed: {rate}");
        }
        // Without the controller the hint sets the route pacing instead.
        for (setting, codec) in [
            ("wireless_adaptive_pacing=false", 1),
            ("", 3),
            ("pacing_max_bitrate_kbps=80000", 1),
            ("host_adaptive_bitrate=true", 1),
        ] {
            let config = crate::config::Config::parse(setting).unwrap();
            let s = Session::new(
                launch("wireless-hint", Role::Stream),
                Negotiated {
                    codec,
                    client_wireless: true,
                    ..Default::default()
                },
            );
            s.configure_automatic_pacing(config.adaptive_packet_pacing(codec, true));
            assert!(
                s.client_wireless_sets_route_pacing(),
                "{setting} codec {codec}"
            );
        }
        let wired = Session::new(launch("wired", Role::Stream), Negotiated::default());
        assert!(!wired.client_wireless_sets_route_pacing());
    }

    #[test]
    fn pyrowave_fec_adapts_for_records_and_legacy_clients() {
        for records in [false, true] {
            let s = Session::new(
                launch("pyrowave-fec", Role::Stream),
                Negotiated {
                    codec: 3,
                    pyrowave_records: records,
                    ..Default::default()
                },
            );
            assert_eq!(s.configure_fec(true, 200), 200);
            s.stats.video_frame.store(3, Ordering::Release);
            s.record_fec_status(&fec_report(1, 9, 1));
            s.tick_fec(
                Instant::now()
                    + crate::adaptive_fec::AdaptiveFec::CLEAN_HOLD
                    + Duration::from_secs(1),
            );
            assert_eq!(s.fec_percent(), 199);
            s.record_fec_status(&fec_report(2, 7, 1));
            assert_eq!(s.fec_percent(), 200);
        }
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
    fn disabled_automatic_pacing_preserves_the_base_rate() {
        for (enabled, codec) in [(false, 0), (false, 1), (false, 3)] {
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
    fn a_client_fec_share_splits_the_encoder_budget_only_at_a_fence() {
        use crate::extensions::FecChoice;
        use crate::statistical_fec::FecReport;
        let report = |seq, missing| {
            FecReport {
                seq,
                data_shards: 900,
                parity_shards: 180,
                missing_shards: missing,
                unrecovered_blocks: 0,
                interval_ms: 1000,
            }
            .encode()
        };
        // The host's own FEC: the encoder gets the whole bitrate, as before.
        let host = Session::new(launch("host", Role::Stream), Negotiated::default());
        host.configure_fec_choice(FecChoice::Host, true, 20, 50);
        host.bitrate.store(12_000, Ordering::Release);
        assert_eq!(host.encoder_bitrate(true), 12_000);

        // 12 Mbps at 60 fps: about 25 data shards a frame.
        let stream = Negotiated {
            packet_size: 1024,
            fps: 60,
            ..Default::default()
        };
        let auto = Session::new(launch("auto", Role::Stream), stream);
        assert_eq!(auto.configure_fec_choice(FecChoice::Auto, true, 20, 40), 20);
        auto.bitrate.store(12_000, Ordering::Release);
        assert_eq!(auto.encoder_bitrate(false), 10_000, "12 Mbps at 20 % FEC");
        auto.record_fec_report(&report(0, 0)).unwrap();
        assert_eq!(auto.fec_percent(), 0);
        assert_eq!(auto.encoder_bitrate(false), 10_000, "no fence, no step");
        assert_eq!(
            auto.encoder_bitrate(true),
            12_000,
            "a clean link: all video"
        );
        auto.record_fec_report(&report(1, 540)).unwrap();
        assert_eq!(auto.fec_percent(), 40);
        assert_eq!(auto.encoder_bitrate(false), 12_000);
        assert_eq!(auto.encoder_bitrate(true), 8_571, "12 Mbps × 100 / 140");
        // A new budget (a client /bitrate) applies at once, at the latched share.
        auto.bitrate.store(7_000, Ordering::Release);
        assert_eq!(auto.encoder_bitrate(false), 5_000);

        let fixed = Session::new(launch("fixed", Role::Stream), Negotiated::default());
        assert_eq!(
            fixed.configure_fec_choice(FecChoice::Fixed(90), true, 20, 50),
            50
        );
        fixed.bitrate.store(12_000, Ordering::Release);
        assert_eq!(fixed.encoder_bitrate(true), 8_000);
        // Back to the host's FEC on a new configuration: no split.
        fixed.configure_fec_choice(FecChoice::Host, true, 20, 50);
        assert_eq!(fixed.encoder_bitrate(true), 12_000);
        assert_eq!(fec_split_kbps(0, 50), 0);
        assert_eq!(fec_split_kbps(1, 255), 1);
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
        // PyroWave reports its own configured and adapted protection.
        let pyrowave = Session::new(
            launch("pyrowave-fec", Role::Stream),
            Negotiated {
                codec: 3,
                ..Default::default()
            },
        );
        pyrowave.configure_fec(true, 200);
        assert_eq!(pyrowave.info()["fec_percent"], 200);
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
    fn a_launch_takes_rtsp_only_once_prepared_and_starts_one_session() {
        let mut sessions = Sessions::default();
        let early = launch("early", Role::Stream);
        early.preparing.store(true, Ordering::Release);
        sessions.pending.insert(early.id.clone(), early.clone());
        // Its client connects only after the reply: nothing may claim or
        // start it while the display and app are still being prepared.
        assert!(sessions.rtsp_for_peer(early.peer).is_empty());
        assert!(sessions.pending_for_peer(early.peer).is_err());
        let refused = sessions.claim("early").unwrap_err().to_string();
        assert!(refused.contains("still being prepared"), "{refused}");
        assert!(
            sessions
                .start(early.clone(), Negotiated::default())
                .is_err()
        );
        assert!(sessions.active.is_empty() && sessions.pending.contains_key("early"));
        drop(sessions.expire());
        assert!(sessions.pending.contains_key("early"));

        early.preparing.store(false, Ordering::Release);
        assert_eq!(sessions.rtsp_for_peer(early.peer).len(), 1);
        sessions.claim("early").unwrap();
        let session = sessions
            .start(early.clone(), Negotiated::default())
            .unwrap();
        // A second PLAY, or a claim of the consumed launch, starts nothing.
        assert!(sessions.start(early, Negotiated::default()).is_err());
        assert_eq!(sessions.active.len(), 1);
        // The running stream's own requests (TEARDOWN) still pass.
        sessions.claim("early").unwrap();
        sessions.begin_teardown(&session);
        assert!(sessions.claim("early").is_err());
    }
    #[test]
    fn a_claimed_handshake_outlives_the_30_s_by_its_lease_up_to_the_limit() {
        let mut sessions = Sessions::default();
        let claimed = launch("claimed", Role::Stream);
        let unclaimed = launch("unclaimed", Role::Stream);
        sessions.pending.insert(claimed.id.clone(), claimed.clone());
        sessions
            .pending
            .insert(unclaimed.id.clone(), unclaimed.clone());
        sessions.claim("claimed").unwrap();
        let until = sessions.pending["claimed"].claimed_until.unwrap();
        assert!(until > Instant::now() + RTSP_LEASE - Duration::from_secs(1));
        // 31 s after the reply: the claimed handshake goes on, the other expired.
        for id in ["claimed", "unclaimed"] {
            sessions.pending.get_mut(id).unwrap().created =
                Instant::now() - Duration::from_secs(31);
        }
        let expired = sessions.expire();
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0].id, "unclaimed");
        assert!(sessions.claim("unclaimed").is_err());
        sessions.claim("claimed").unwrap();
        // A lease never reaches past the limit, and an ended lease expires.
        let at = sessions.pending["claimed"].created;
        assert_eq!(
            sessions.pending["claimed"].lease(at + CLAIM_LIMIT - Duration::from_secs(1)),
            at + CLAIM_LIMIT
        );
        sessions.pending.get_mut("claimed").unwrap().claimed_until =
            Some(Instant::now() - Duration::from_millis(1));
        let ended = sessions.claim("claimed").unwrap_err().to_string();
        assert!(ended.contains("expired"), "{ended}");
        sessions.pending.get_mut("claimed").unwrap().claimed_until =
            Some(Instant::now() + RTSP_LEASE);
        sessions.pending.get_mut("claimed").unwrap().created = Instant::now() - CLAIM_LIMIT;
        assert!(sessions.start(claimed, Negotiated::default()).is_err());
        assert_eq!(sessions.expire().len(), 1);
        assert!(sessions.pending.is_empty() && sessions.active.is_empty());
    }
    #[test]
    fn a_reconnect_replaces_a_claimed_launch_which_then_starts_nothing() {
        let mut sessions = Sessions::default();
        let first = launch("first", Role::Stream);
        drop(sessions.queue(first.clone()).unwrap());
        sessions.claim("first").unwrap();
        // The same device launches again while the first handshake is under way.
        let replaced = sessions.queue(launch("second", Role::Stream)).unwrap();
        assert_eq!(replaced.len(), 1);
        assert_eq!(replaced[0].id, "first");
        assert!(sessions.claim("first").is_err());
        assert!(sessions.start(first, Negotiated::default()).is_err());
        sessions.claim("second").unwrap();
        let second = sessions.pending["second"].clone();
        sessions.start(second, Negotiated::default()).unwrap();
        assert_eq!(sessions.active.keys().collect::<Vec<_>>(), ["second"]);
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
            background: None,
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
            transport: Default::default(),
            roaming_offered: Default::default(),
            claimed_until: None,
            warnings: Default::default(),
        }
    }
    #[test]
    fn a_ten_bit_format_is_sdr_only_when_the_query_says_hdr_mode_zero() {
        // Launch and resume carry the same query, so one table covers both.
        for (hdr_mode, sdr) in [(None, false), (Some("0"), true), (Some("1"), false)] {
            let mut launch = launch("color", Role::Stream);
            if let Some(mode) = hdr_mode {
                launch.options.insert("hdrMode".into(), mode.into());
            }
            assert_eq!(launch.ten_bit_sdr_requested(), sdr, "hdrMode={hdr_mode:?}");
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
        // The stream's first keyframe alone: held for now, not remembered.
        assert!(s.keyframe_loss_ever_latched());
        assert!(s.keyframe_loss_observed_at().is_some());
        assert!(s.keyframe_loss_sustained_at().is_none());

        // Without reports: the client asks again for the keyframe it was sent.
        let s = Session::new(launch("old", Role::Stream), Negotiated::default());
        s.configure_keyframe_loss(true);
        s.keyframe_loss_bps(FAST, SAFE, 1, true);
        s.keyframe_loss.lock().unwrap().settle();
        s.request_recovery_keyframe();
        assert_eq!(s.keyframe_loss_bps(FAST, SAFE, 2, true), FAST);
        s.request_recovery_keyframe();
        assert_eq!(s.keyframe_loss_bps(FAST, SAFE, 3, true), SAFE);
        assert_eq!(s.stats.idr_requests.load(Ordering::Relaxed), 2);

        // Explicitly disabled policies preserve the base rate for every codec.
        for (enabled, codec) in [(false, 1), (false, 3)] {
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
    fn repeated_distinct_unrecoverable_frames_reduce_pacing_without_changing_bitrate() {
        const FAST: u64 = 800_000_000;
        const SAFE: u64 = 62_000_000;
        let s = Session::new(launch("loss", Role::Stream), Negotiated::default());
        s.configure_keyframe_loss(true);
        s.stats.video_frame.store(7, Ordering::Release);
        let bitrate = s.bitrate.load(Ordering::Relaxed);
        s.record_fec_status(&fec_report(1, 10, 0)); // Clean.
        s.record_fec_status(&fec_report(2, 9, 1)); // Recovered.
        // Interleaved failed blocks from two frames are still only two losses.
        for (frame, block) in [(4, 0), (5, 0), (4, 1), (5, 1), (4, 2), (5, 2)] {
            let mut report = fec_report(frame, 7, 1);
            report[19] = block;
            report[20] = 4;
            s.record_fec_status(&report);
            s.record_fec_status(&report); // Duplicate block.
            assert!(!s.keyframe_loss_active());
            assert_eq!(s.keyframe_loss_rate_bps(FAST, SAFE), FAST);
        }
        s.record_fec_status(&fec_report(6, 7, 1));
        assert!(s.keyframe_loss_active());
        assert_eq!(s.keyframe_loss_rate_bps(FAST, SAFE), SAFE);
        assert_eq!(s.bitrate.load(Ordering::Relaxed), bitrate);
        assert_eq!(s.stats.idr_requests.load(Ordering::Relaxed), 0);
        assert!(
            s.info()["warnings"][0]["message"]
                .as_str()
                .unwrap()
                .contains("three recent")
        );
    }
    #[test]
    fn repeated_loss_guard_preserves_disabled_manual_and_adaptive_owners() {
        for text in [
            "keyframe_loss_pacing=false",
            "pacing_max_bitrate_kbps=80000",
            "host_adaptive_pacing=true",
            "host_adaptive_bitrate=true",
        ] {
            let config = crate::config::Config::parse(text).unwrap();
            let s = Session::new(launch("owner", Role::Stream), Negotiated::default());
            s.configure_keyframe_loss(config.keyframe_loss_pacing(0, false));
            s.stats.video_frame.store(4, Ordering::Release);
            for frame in 1..=3 {
                s.record_fec_status(&fec_report(frame, 7, 1));
            }
            assert!(!s.keyframe_loss_active(), "{text}");
            assert!(
                s.info()["warnings"].as_array().unwrap().is_empty(),
                "{text}"
            );
        }
    }
    #[test]
    fn a_fresh_probe_or_disabled_policy_clears_the_previous_launch_warning() {
        let s = Session::new(launch("remembered", Role::Stream), Negotiated::default());
        for enabled in [true, false] {
            s.configure_remembered_keyframe_loss(true, true);
            assert_eq!(
                s.keyframe_loss_reason(),
                Some("remembered from the previous session")
            );
            assert!(!s.info()["warnings"].as_array().unwrap().is_empty());
            s.configure_remembered_keyframe_loss(enabled, false);
            assert!(!s.keyframe_loss_active());
            assert!(s.info()["warnings"].as_array().unwrap().is_empty());
        }
        s.configure_remembered_keyframe_loss(false, true);
        assert!(!s.keyframe_loss_active());
    }
    #[test]
    fn sender_pacing_queries_do_not_add_sent_frames_to_keyframe_history() {
        const FAST: u64 = 800_000_000;
        const SAFE: u64 = 62_000_000;
        let s = Session::new(launch("query", Role::Stream), Negotiated::default());
        s.configure_keyframe_loss(true);
        s.keyframe_loss_bps(FAST, SAFE, 1, true);
        s.keyframe_loss.lock().unwrap().settle();
        s.request_recovery_keyframe();
        s.keyframe_loss_bps(FAST, SAFE, 2, true);
        for _ in 0..crate::keyframe_loss::DECODE_QUEUE + 1 {
            assert!(!s.keyframe_loss_active());
            assert_eq!(s.keyframe_loss_rate_bps(FAST, SAFE), FAST);
        }
        // No ordinary frame followed that keyframe: repeated queries cannot
        // turn a lost-keyframe request into an apparent decode queue overflow.
        s.request_recovery_keyframe();
        assert!(s.keyframe_loss_active());
        assert_eq!(s.keyframe_loss_rate_bps(FAST, SAFE), SAFE);
    }
    /// A stream sending frames 1..=40 on a good link; `FAST`/`SAFE` as above.
    fn steady_stream(name: &str) -> std::sync::Arc<Session> {
        let s = Session::new(launch(name, Role::Stream), Negotiated::default());
        s.configure_keyframe_loss(true);
        s.keyframe_loss_bps(800_000_000, 62_000_000, 1, true);
        for frame in 2..=40 {
            s.keyframe_loss_bps(800_000_000, 62_000_000, frame, false);
        }
        s.stats.video_frame.store(41, Ordering::Release);
        s
    }
    #[test]
    fn the_hosts_own_keyframe_and_the_clients_reaction_to_it_do_not_slow_the_stream() {
        // Alt+Tab, a bitrate change or an encoder restart: the host sends a
        // keyframe nobody asked for, the client resets and asks for one,
        // twice. No packet was lost (the issue behind Wi-Fi pacing for 24 h).
        const FAST: u64 = 800_000_000;
        const SAFE: u64 = 62_000_000;
        let s = steady_stream("alt-tab");
        s.keyframe_loss_bps(FAST, SAFE, 41, true);
        s.request_recovery_keyframe();
        s.keyframe_loss_bps(FAST, SAFE, 42, true);
        s.request_recovery_keyframe();
        for frame in 43..=45 {
            s.keyframe_loss_bps(FAST, SAFE, frame, false);
        }
        s.stats.video_frame.store(46, Ordering::Release);
        for frame in 43..=45 {
            s.record_fec_status(&fec_report(frame, 7, 1));
        }
        assert!(!s.keyframe_loss_active());
        assert!(s.keyframe_loss_observed_at().is_none());
        assert_eq!(s.keyframe_loss_rate_bps(FAST, SAFE), FAST);
        assert!(s.info()["warnings"].as_array().unwrap().is_empty());
        // The keyframe itself arriving incomplete is still network loss.
        s.record_fec_status(&fec_report(41, 7, 1));
        assert!(s.keyframe_loss_active());
        assert!(s.keyframe_loss_observed_at().is_some());
    }
    #[test]
    fn the_start_of_a_stream_does_not_slow_it_but_a_lost_first_keyframe_does() {
        // Logged at the start of real streams: the client asks for a
        // keyframe, gets frame 1, and asks again ~100 ms later before it has
        // shown anything. Nothing was lost.
        const FAST: u64 = 800_000_000;
        const SAFE: u64 = 62_000_000;
        let start = |name: &str| {
            let s = Session::new(launch(name, Role::Stream), Negotiated::default());
            s.configure_keyframe_loss(true);
            s.request_recovery_keyframe();
            s.keyframe_loss_bps(FAST, SAFE, 1, true);
            s.request_recovery_keyframe();
            s.keyframe_loss_bps(FAST, SAFE, 2, true);
            s.request_recovery_keyframe();
            for frame in 3..=10 {
                s.keyframe_loss_bps(FAST, SAFE, frame, false);
            }
            s.stats.video_frame.store(11, Ordering::Release);
            s
        };
        let s = start("startup");
        assert!(!s.keyframe_loss_active());
        assert!(s.keyframe_loss_observed_at().is_none());
        assert_eq!(s.keyframe_loss_rate_bps(FAST, SAFE), FAST);
        assert!(s.info()["warnings"].as_array().unwrap().is_empty());
        // Frames reported after the second keyframe, inside the window, are
        // the same start; the first keyframe reported incomplete is a loss.
        s.record_fec_status(&fec_report(5, 7, 1));
        assert!(!s.keyframe_loss_active());
        s.record_fec_status(&fec_report(2, 7, 1));
        assert!(s.keyframe_loss_active());
    }
    #[test]
    fn frames_the_host_could_not_send_slow_the_stream_and_the_clients_report_adds_nothing() {
        // Send-buffer pressure at 800 Mbps (error 10055) is loss: three frames
        // within two seconds slow the stream, named as lost on send.
        const FAST: u64 = 800_000_000;
        const SAFE: u64 = 62_000_000;
        let s = steady_stream("lost-on-send");
        for frame in 41..=43 {
            s.keyframe_loss_bps(FAST, SAFE, frame, false);
            s.keyframe_abandoned(frame);
        }
        assert!(s.keyframe_loss_active());
        assert_eq!(
            s.keyframe_loss_reason(),
            Some("three recent video frames were lost on send within two seconds")
        );
        assert_eq!(s.keyframe_loss_bps(FAST, SAFE, 44, false), SAFE);
        // Two such frames are not enough, and the client reporting them is the
        // same loss, not more of it.
        let s = steady_stream("lost-on-send-twice");
        for frame in 41..=42 {
            s.keyframe_loss_bps(FAST, SAFE, frame, false);
            s.keyframe_abandoned(frame);
        }
        s.stats.video_frame.store(44, Ordering::Release);
        for frame in 41..=42 {
            s.record_fec_status(&fec_report(frame, 7, 1));
        }
        assert!(!s.keyframe_loss_active());
    }

    #[test]
    fn a_keyframe_the_host_could_not_send_whole_slows_the_stream() {
        const FAST: u64 = 800_000_000;
        const SAFE: u64 = 62_000_000;
        let s = steady_stream("keyframe-lost-on-send");
        s.keyframe_loss_bps(FAST, SAFE, 41, true);
        s.keyframe_abandoned(41);
        assert_eq!(
            s.keyframe_loss_reason(),
            Some("a keyframe was lost on send")
        );
    }

    #[test]
    fn the_keyframe_after_a_loss_on_send_does_not_hide_the_losses_around_it() {
        // Not a change the host made on its own, so no window follows it and
        // the client's reports of later frames still count.
        const FAST: u64 = 800_000_000;
        const SAFE: u64 = 62_000_000;
        let s = steady_stream("send-loss-recovery");
        s.keyframe_loss.lock().unwrap().settle();
        s.request_send_loss_recovery();
        s.keyframe_loss_bps(FAST, SAFE, 41, true);
        s.stats.video_frame.store(60, Ordering::Release);
        for frame in [45, 50, 55] {
            s.record_fec_status(&fec_report(frame, 7, 1));
        }
        assert!(s.keyframe_loss_active());
    }

    #[test]
    fn a_request_just_before_the_hosts_keyframe_is_not_the_one_asked_for_again() {
        const FAST: u64 = 800_000_000;
        const SAFE: u64 = 62_000_000;
        let s = steady_stream("request-then-host-keyframe");
        s.request_recovery_keyframe();
        s.request_host_keyframe();
        s.keyframe_loss_bps(FAST, SAFE, 41, true);
        s.keyframe_loss_bps(FAST, SAFE, 42, false);
        s.request_recovery_keyframe();
        assert!(!s.keyframe_loss_active());
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
    fn a_sent_picture_feeds_the_client_report_and_the_record_sink_alike() {
        use crate::{host_timing::HostTimingRequest, performance::Stamps, timing_record};
        use std::time::Duration;
        let session = Session::new(launch("record", Role::Stream), Negotiated::default());
        let epoch = Instant::now();
        let us = |n: u64| epoch + Duration::from_micros(n);
        let stamps = Stamps {
            presented: us(100),
            acquired: us(200),
            published: us(250),
            claimed: us(300),
            submitted: us(500),
            encoded: us(2_500),
            packetized: us(2_600),
            prepared: us(2_700),
            first_sent: us(2_700),
            last_sent: us(2_900),
            streamed: None,
        };
        // Neither a client report nor a sink: nothing is kept.
        session.record_sent(1, 9_000, &stamps, epoch);
        assert_eq!(session.host_timings.lock().unwrap().take_report(), None);

        let (sink, rows) = timing_record::channel(8);
        *session.timing_record.write().unwrap() = Some(Arc::new(sink));
        session
            .host_timings
            .lock()
            .unwrap()
            .on_request(&HostTimingRequest { enabled: true }.encode());
        session.record_sent(2, 9_000, &stamps, epoch);
        let report = session.host_timings.lock().unwrap().take_report().unwrap();
        let (record, extra) = crate::host_timing::HostTiming::decode_report_with(&report)
            .unwrap()
            .remove(0);
        assert_eq!((record.frame, record.host_us), (2, 2_800));
        assert_eq!(extra.unwrap().presented_at_us, 100);
        let row = rows.try_recv().unwrap();
        assert_eq!(row.wire_frame, 2);
        assert_eq!(row.rtp_us, 100_000);
        assert_eq!(row.points[0], 100);
        assert_eq!(row.points[9], 2_900);
        assert!(rows.try_recv().is_err(), "the first picture had no sink");
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
    fn slice_delivery_overflows_are_counted_in_the_session_info() {
        let session = Session::new(launch("overflow", Role::Stream), Negotiated::default());
        assert_eq!(session.info()["slice_delivery_overflows"], 0);
        session
            .stats
            .slice_delivery_overflows
            .fetch_add(1, Ordering::Relaxed);
        assert_eq!(session.info()["slice_delivery_overflows"], 1);
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
    fn webrtc_launch(id: &str, client: &str) -> Launch {
        let mut launch = launch(id, Role::Stream);
        launch.transport = Transport::WebRtc;
        launch.client.uuid = client.into();
        launch
    }
    #[test]
    fn a_session_of_either_transport_ends_the_other_one() {
        // A WebRTC session replaces a Moonlight stream and a pending Moonlight launch.
        let mut sessions = Sessions::default();
        sessions.queue(launch("moonlight", Role::Stream)).unwrap();
        let moonlight = sessions
            .start(launch("moonlight", Role::Stream), Negotiated::default())
            .unwrap();
        sessions.queue(launch("pending", Role::Stream)).unwrap();
        let canceled = sessions.supersede_other_transport(Role::Stream, Transport::WebRtc);
        assert_eq!(canceled.len(), 1);
        assert!(moonlight.stopping());
        assert!(sessions.pending.is_empty());
        // And a Moonlight launch replaces a WebRTC stream, from any device.
        sessions.queue(webrtc_launch("web", "tv")).unwrap();
        let web = sessions
            .start(webrtc_launch("web", "tv"), Negotiated::default())
            .unwrap();
        let canceled = sessions.supersede_other_transport(Role::Stream, Transport::GameStream);
        assert!(canceled.is_empty());
        assert!(web.stopping());
        // Sessions of the transport asking are left alone.
        sessions.queue(webrtc_launch("web2", "tv")).unwrap();
        let web2 = sessions
            .start(webrtc_launch("web2", "tv"), Negotiated::default())
            .unwrap();
        let canceled = sessions.supersede_other_transport(Role::Stream, Transport::WebRtc);
        assert!(canceled.is_empty());
        assert!(!web2.stopping());
        // Another role is untouched too.
        sessions
            .queue(launch("monitor", Role::RemoteMonitor))
            .unwrap();
        let canceled = sessions.supersede_other_transport(Role::Stream, Transport::GameStream);
        assert!(canceled.is_empty());
        assert!(sessions.pending.contains_key("monitor"));
    }
    #[test]
    fn a_webrtc_stream_is_held_by_another_device_only_while_it_lives() {
        let mut sessions = Sessions::default();
        assert!(!sessions.held_by_other(Role::Stream, Transport::WebRtc, "tv"));
        sessions.queue(webrtc_launch("web", "tv")).unwrap();
        assert!(!sessions.held_by_other(Role::Stream, Transport::WebRtc, "tv"));
        assert!(sessions.held_by_other(Role::Stream, Transport::WebRtc, "phone"));
        // A Moonlight launch is not a WebRTC one.
        assert!(!sessions.held_by_other(Role::Stream, Transport::GameStream, "phone"));
        let web = sessions
            .start(webrtc_launch("web", "tv"), Negotiated::default())
            .unwrap();
        assert!(sessions.held_by_other(Role::Stream, Transport::WebRtc, "phone"));
        web.stop();
        assert!(!sessions.held_by_other(Role::Stream, Transport::WebRtc, "phone"));
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

/// How a session's media reaches the client. GameStream is Moonlight's RTSP, RTP and ENet;
/// WebRTC is the signalling server's session (see `webrtc::session` in the host).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Transport {
    #[default]
    GameStream,
    WebRtc,
}

pub struct Launch<P = (), A = ()> {
    pub warnings: Arc<Warnings>,
    pub id: String,
    pub client: Client,
    pub peer: IpAddr,
    pub app_id: u32,
    /// Fixed before publishing the launch so live configuration edits cannot change isolation.
    pub background: Option<bool>,
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
    pub transport: Transport,
    /// The DESCRIBE answer offered client roaming (`crate::roaming::SDP_ATTRIBUTE`).
    pub roaming_offered: Arc<AtomicBool>,
    /// Until when an RTSP handshake holds this launch past its 30 s
    /// ([`Sessions::claim`]); `None` until its first RTSP request.
    pub claimed_until: Option<Instant>,
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
            background: self.background,
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
            transport: self.transport,
            roaming_offered: self.roaming_offered.clone(),
            claimed_until: self.claimed_until,
        }
    }
}
impl<P, A> Launch<P, A> {
    /// Whether a 10-bit format negotiated by RTSP is 10-bit SDR. The protocol
    /// has no flag for it, so a client says so with `hdrMode=0` on /launch or
    /// /resume. Without `hdrMode` the format stays HDR, as older and
    /// third-party clients expect.
    pub fn ten_bit_sdr_requested(&self) -> bool {
        self.options.get("hdrMode").is_some_and(|v| v == "0")
    }
    /// RTSP may wait for network input after authorization. Its identity
    /// snapshot must not keep a canceled launch's display, limiter or audio
    /// route alive while a replacement launch prepares new resources.
    fn rtsp_snapshot(&self) -> Self {
        let mut snapshot = self.clone();
        snapshot.preparation = Default::default();
        snapshot.audio_preparation = Default::default();
        snapshot
    }
    /// Still waiting for its client: being prepared, or able to take its
    /// client's RTSP requests ([`Launch::claimable`]).
    fn live(&self) -> bool {
        self.preparing.load(Ordering::Acquire) || self.claimable().is_ok()
    }
    /// Whether RTSP may use this launch now: only once the host has prepared
    /// it and sent the reply (the client connects only then), and only within
    /// [`LAUNCH_WAIT`] of that, or while a claimed handshake's lease runs,
    /// never past [`CLAIM_LIMIT`].
    fn claimable(&self) -> Result<()> {
        if self.preparing.load(Ordering::Acquire) {
            bail!("the launch is still being prepared; the device connects after its reply");
        }
        let age = self.created.elapsed();
        let leased = self
            .claimed_until
            .is_some_and(|until| Instant::now() < until && age < CLAIM_LIMIT);
        if age >= LAUNCH_WAIT && !leased {
            bail!(
                "the launch expired: the device did not finish its RTSP handshake within {} s",
                age.as_secs()
            );
        }
        Ok(())
    }
}
/// How long a prepared launch waits for its client's first RTSP request.
pub const LAUNCH_WAIT: Duration = Duration::from_secs(30);
/// How long each RTSP request on a claimed launch keeps it waiting for the next.
pub const RTSP_LEASE: Duration = Duration::from_secs(10);
/// The longest a claimed launch waits for PLAY, counted from its reply.
pub const CLAIM_LIMIT: Duration = Duration::from_secs(60);
impl<P, A> Launch<P, A> {
    /// The lease an RTSP request at `now` gives this launch.
    fn lease(&self, now: Instant) -> Instant {
        (now + RTSP_LEASE).min(self.created + CLAIM_LIMIT)
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
    /// Streamed video frames whose encoder output passed the byte cap their
    /// block plan relied on (`video_slice_delivery`); each turned it off.
    pub slice_delivery_overflows: AtomicU64,
    pub latency_us: AtomicU64,
    pub frames_replaced: AtomicU64,
    /// Last frame offered to the video socket, plus one; zero means none yet.
    pub video_frame: AtomicU64,
    pub performance: std::sync::Mutex<crate::performance::Performance>,
    /// The stages `frames` doesn't count, for the pipeline frame rates.
    pub pipeline: crate::pipeline_rates::Counters,
    /// The console's pipeline frame rates, over `pipeline_rates::CONSOLE_WINDOW`.
    pub pipeline_console: std::sync::Mutex<crate::pipeline_rates::Sampler>,
}
/// The encoder's share of a video `budget` that also carries `fec` percent
/// parity: budget × 100 / (100 + fec), at least 1 Kbps of a non-zero budget.
fn fec_split_kbps(budget: u32, fec: u32) -> u32 {
    if budget == 0 {
        return 0;
    }
    (u64::from(budget) * 100 / (100 + u64::from(fec))).max(1) as u32
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
    /// Set by the stream worker once the capture's first frame passed the checks a WebRTC HDR
    /// stream needs (the display is in HDR). The transport holds back its "streaming" status
    /// until then, so a refused stream is never announced first.
    pub source_checked: AtomicBool,
    pub idr: AtomicBool,
    pub invalidation: std::sync::Mutex<Option<(u64, u64)>>,
    pub recovery_wake: std::sync::Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
    pub bitrate: AtomicU32,
    /// All bitrate writers take this lock, including client ownership handoff.
    pub adaptive_bitrate: std::sync::Mutex<crate::adaptive_bitrate::AdaptiveBitrate>,
    /// Lowers the encoder bitrate when the link cannot carry the stream; see
    /// [`crate::link_rate`]. Off for PyroWave, a client that sets the bitrate
    /// and `host_adaptive_bitrate`.
    pub link_rate: std::sync::Mutex<crate::link_control::LinkControl>,
    /// What the video sender sent, for `link_rate`; one store per frame.
    pub link_sent: crate::link_control::SentLog,
    /// The route policy paced the last frame faster than the Wi-Fi rate, so
    /// a latch that slows it may be the whole fix (see `LinkRate::on_latch`).
    pace_was_fast: AtomicBool,
    pub adaptive_video_pacing_bps: AtomicU64,
    pub automatic_pacing: std::sync::Mutex<crate::adaptive_pacing::AdaptivePacing>,
    /// Whether `automatic_pacing` is enabled, set with it at stream start.
    automatic_pacing_on: AtomicBool,
    /// Slower pacing once the client's link has dropped a keyframe.
    pub keyframe_loss: std::sync::Mutex<crate::keyframe_loss::KeyframeLoss>,
    /// FEC percentage for the next video frame; the video sender reads it per
    /// frame, and `adaptive_fec` moves it from the client's 0x5502 reports.
    pub fec_percent: AtomicU32,
    pub adaptive_fec: std::sync::Mutex<crate::adaptive_fec::AdaptiveFec>,
    /// Statistical FEC from the client's 0x5539 reports, for a client that
    /// launched with `rlFecPercentage=auto`; `adaptive_fec` is off then.
    pub client_fec: std::sync::Mutex<Option<crate::statistical_fec::StatisticalFec>>,
    /// Whether `client_fec` is set: frames skip its lock otherwise.
    client_fec_on: AtomicBool,
    /// Whether the encoder's bitrate leaves room for the client-chosen FEC
    /// share in `fec_split`, latched at the last fence; see [`Self::encoder_bitrate`].
    fec_split_on: AtomicBool,
    fec_split: AtomicU32,
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
    /// Tells a client that asked (0x5538) when the host holds the bitrate down for the
    /// network; fed by `tick_link_rate`, sent by the control loop.
    pub link_notices: std::sync::Mutex<crate::link_notice::LinkNotices>,
    /// Pipeline frame rates for a client that asked (0x553D), sent by the control loop.
    pub pipeline_rates: std::sync::Mutex<crate::pipeline_rates::Reporter>,
    /// Zero of the video stream's RTP clock, set when its sender starts; clock sync
    /// replies (0x5537) are on this clock.
    pub media_epoch: std::sync::RwLock<Option<Instant>>,
    /// The host-side timing record (`BUTTERPOLLO_TIMING_RECORD`), when it is on.
    pub timing_record: std::sync::RwLock<Option<Arc<crate::timing_record::Sink>>>,
    /// The requested `encoder_focus` ([`crate::encoder_focus::Requested::wire`]). The stream
    /// stores the configured value at start; the encoder reads it per frame.
    pub encoder_focus: AtomicU8,
    /// What the encoder runs, written by the encoder thread on change.
    pub encoder_focus_state: std::sync::Mutex<crate::encoder_focus::FocusState>,
    /// Where the client is after a roam (`crate::roaming`); see [`Self::roamed`].
    roam: std::sync::Mutex<Roam>,
    /// Roams so far, for senders that keep state about the client's path.
    roams: AtomicU32,
    /// Night mode and headphone surround: the live choice and why either
    /// doesn't apply. Written through `audio_dsp::{state, apply}`.
    pub audio: crate::audio_dsp::Controls,
    /// The sent audio's copy for haptics from game audio, filled only while a
    /// controller wants it (`audio_haptics`). Shared so a control peer that
    /// wanted it can stop the copy when it goes.
    pub audio_haptics: Arc<crate::audio_haptics::Tap>,
    /// The host-drawn couch menu: open flag, revision and panel, shared by the
    /// control loop, the menu worker, the video loop and the native API.
    pub couch_menu: Arc<crate::couch_menu::Shared>,
}
/// The client's address after its roams.
struct Roam {
    client: IpAddr,
    at: Option<Instant>,
    /// No video ping has come from the path the client roamed to yet.
    new_path: bool,
}
impl<P, A> Session<P, A> {
    /// Request an encoder focus mode, such as from the couch menu; the
    /// encoder resolves it on its next frame.
    pub fn set_encoder_focus(&self, requested: crate::encoder_focus::Requested) {
        self.encoder_focus
            .store(requested.wire(), std::sync::atomic::Ordering::Relaxed);
    }
    /// Notes a new picture whose last packet was just handed to the socket: for a client
    /// that asked for host timing (0x5536) and for the host-side record. `rtp_timestamp` is
    /// the picture's RTP timestamp on the wire and `epoch` the media clock's start.
    pub fn record_sent(
        &self,
        wire_frame: u32,
        rtp_timestamp: u32,
        stamps: &crate::performance::Stamps,
        epoch: Instant,
    ) {
        self.host_timings
            .lock()
            .unwrap()
            .record(wire_frame, stamps, epoch);
        if let Some(sink) = &*self.timing_record.read().unwrap() {
            sink.record(crate::timing_record::Row::new(
                wire_frame,
                rtp_timestamp,
                stamps,
                epoch,
            ));
        }
    }
    pub fn new(launch: Launch<P, A>, config: Negotiated) -> Arc<Self> {
        let launch_peer = launch.peer.to_canonical();
        let bitrate = config.bitrate_kbps;
        let mode = crate::reconfigure::Mode::of(&config);
        let started = Instant::now();
        Arc::new(Self {
            encoder: Default::default(),
            capture_warnings: Default::default(),
            launch,
            config,
            stop: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            source_checked: AtomicBool::new(false),
            idr: AtomicBool::new(true),
            invalidation: Default::default(),
            recovery_wake: Default::default(),
            bitrate: AtomicU32::new(bitrate),
            adaptive_bitrate: Default::default(),
            link_rate: Default::default(),
            link_sent: crate::link_control::SentLog::new(started),
            pace_was_fast: AtomicBool::new(false),
            adaptive_video_pacing_bps: AtomicU64::new(0),
            automatic_pacing: Default::default(),
            automatic_pacing_on: AtomicBool::new(false),
            keyframe_loss: Default::default(),
            fec_percent: AtomicU32::new(
                crate::adaptive_fec::AdaptiveFec::default().percent() as u32
            ),
            adaptive_fec: Default::default(),
            client_fec: Default::default(),
            client_fec_on: AtomicBool::new(false),
            fec_split_on: AtomicBool::new(false),
            fec_split: AtomicU32::new(0),
            phase_sync: Default::default(),
            reconfigure: std::sync::Mutex::new(crate::reconfigure::Reconfiguration::new(mode)),
            stats: Stats::default(),
            started,
            output: std::sync::RwLock::new(String::new()),
            hdr_metadata: Default::default(),
            hdr_source: Default::default(),
            host_timings: Default::default(),
            link_notices: Default::default(),
            pipeline_rates: Default::default(),
            media_epoch: Default::default(),
            timing_record: Default::default(),
            encoder_focus: AtomicU8::new(crate::encoder_focus::Requested::Off.wire()),
            encoder_focus_state: Default::default(),
            roam: std::sync::Mutex::new(Roam {
                client: launch_peer,
                at: None,
                new_path: false,
            }),
            roams: AtomicU32::new(0),
            audio: Default::default(),
            audio_haptics: Default::default(),
            // The control peer stores what the stream allows when it
            // connects; WebRTC sessions have none.
            couch_menu: Arc::new(crate::couch_menu::Shared::refusing(
                "couch menu: this stream's control connection has not started",
            )),
        })
    }
    /// The client's IP: the launch's, or the one it last roamed to.
    pub fn client_ip(&self) -> IpAddr {
        self.roam.lock().unwrap().client
    }
    /// The client proved it moved to `client` (`crate::roaming`). Its media pings come
    /// from there from now on.
    pub fn roamed(&self, client: IpAddr, now: Instant) {
        *self.roam.lock().unwrap() = Roam {
            client: client.to_canonical(),
            at: Some(now),
            new_path: true,
        };
        self.roams.fetch_add(1, Ordering::AcqRel);
    }
    /// How often the client roamed.
    pub fn roams(&self) -> u32 {
        self.roams.load(Ordering::Acquire)
    }
    /// The client's link hint (`pyrowaveLinkMbps`), 0 once it roamed: the hint came
    /// with the ANNOUNCE and describes the path the client left.
    pub fn client_link_mbps(&self) -> u32 {
        if self.roams() == 0 {
            self.config.pyrowave_client_link_mbps
        } else {
            0
        }
    }
    /// The client's pace hint (`pyrowavePaceMbps`), 0 once it roamed, as
    /// [`Self::client_link_mbps`].
    pub fn client_pace_mbps(&self) -> u32 {
        if self.roams() == 0 {
            self.config.pyrowave_client_pace_mbps
        } else {
            0
        }
    }
    /// True once per roam, for the first video ping from the new path.
    pub fn take_new_path(&self) -> bool {
        std::mem::take(&mut self.roam.lock().unwrap().new_path)
    }
    /// The client's video now takes the path it roamed to: what pacing learned about
    /// the old path no longer holds. The automatic pacing estimate starts over, host
    /// bitrate adaptation forgets the old path's RTT baseline, and the keyframe-loss
    /// guard starts over without the old path's latch.
    pub fn new_network_path(&self) {
        self.configure_automatic_pacing(self.automatic_pacing_enabled());
        self.adaptive_bitrate.lock().unwrap().new_path();
        {
            let mut loss = self.keyframe_loss.lock().unwrap();
            *loss = crate::keyframe_loss::KeyframeLoss::new(loss.enabled());
        }
        self.launch.warnings.clear("network_keyframe_loss");
    }
    /// Since when a stream without its client's media ping has waited for one: the
    /// stream's `start`, or the last roam, which drops the old path's media addresses.
    pub fn media_wait_start(&self, start: Instant) -> Instant {
        self.roam
            .lock()
            .unwrap()
            .at
            .map_or(start, |at| at.max(start))
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
    /// A keyframe the host makes on its own initiative (a recreated encoder,
    /// a frame beyond the packet limit). Whatever the client requests around
    /// it is a reaction to the host, not evidence about the network.
    pub fn request_host_keyframe(&self) {
        self.keyframe_loss.lock().unwrap().host_keyframe();
        self.request_idr();
    }
    /// A keyframe a second after a reference invalidation the decoder may not
    /// have followed. It answers loss, so it is counted like loss.
    pub fn request_confirming_keyframe(&self) {
        self.keyframe_loss.lock().unwrap().loss_keyframe();
        self.idr.store(true, Ordering::Release);
    }
    /// A keyframe the client needs after losing frames: its request (0x0302),
    /// or an invalidation the encoder cannot repair with a reference frame.
    pub fn request_recovery_keyframe(&self) {
        let (latched, ignored) = {
            let mut loss = self.keyframe_loss.lock().unwrap();
            let latched = loss.requested(Instant::now());
            if self.config.codec == 3 {
                // Intra-only: no keyframe will answer it.
                loss.unanswerable();
            }
            (latched, loss.take_ignored())
        };
        self.report_keyframe_loss(latched, ignored);
        self.request_idr();
    }
    /// The client asked for a keyframe (0x0302): the same, and evidence for
    /// the bitrate controller when it is the network and not the host or the
    /// client's decode queue that caused the request.
    pub fn request_client_keyframe(&self) {
        self.link_request();
        self.request_recovery_keyframe();
    }
    /// The host could not send enough of a frame for the client to rebuild
    /// it, so the next frame is a keyframe. Moonlight, waiting for one,
    /// then recovers on that frame and sends no request of its own.
    pub fn request_send_loss_recovery(&self) {
        self.stats
            .send_loss_recoveries
            .fetch_add(1, Ordering::Relaxed);
        if self.config.codec != 3 {
            self.keyframe_loss.lock().unwrap().loss_keyframe();
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
        self.link_request();
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
            let period = self.stream_mode().period();
            self.link_status(payload, frame, block);
            if matches!(block, crate::adaptive_fec::Block::Unrecoverable) {
                let (latched, ignored) = {
                    let mut loss = self.keyframe_loss.lock().unwrap();
                    let latched =
                        loss.unrecoverable(frame, last_sent.unwrap(), period, Instant::now());
                    (latched, loss.take_ignored())
                };
                self.report_keyframe_loss(latched, ignored);
            }
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
        *fec = if self.config.codec == 3 {
            crate::adaptive_fec::AdaptiveFec::pyrowave(adaptive, configured)
        } else {
            crate::adaptive_fec::AdaptiveFec::new(adaptive, configured)
        };
        let percent = fec.percent();
        self.fec_percent.store(percent as u32, Ordering::Release);
        percent
    }
    /// Starts the stream's FEC as the client chose it with `rlFecPercentage`:
    /// the host's own (`adaptive` and `configured`, as [`Self::configure_fec`]),
    /// a fixed share capped at `maximum` (`fec_max_percentage`), or statistical
    /// FEC up to `maximum` that keeps `configured` until the client's reports
    /// arrive. PyroWave keeps its own protection. Returns the first percentage.
    pub fn configure_fec_choice(
        &self,
        choice: crate::extensions::FecChoice,
        adaptive: bool,
        configured: usize,
        maximum: u8,
    ) -> usize {
        use crate::extensions::FecChoice;
        let mut client = self.client_fec.lock().unwrap();
        *client = None;
        self.client_fec_on.store(false, Ordering::Release);
        self.fec_split_on.store(false, Ordering::Release);
        if self.config.codec == 3 || choice == FecChoice::Host {
            drop(client);
            return self.configure_fec(adaptive, configured);
        }
        let fallback = configured.min(255) as u8;
        let percent = match choice {
            FecChoice::Fixed(percent) => percent.min(maximum),
            _ => {
                *client = Some(crate::statistical_fec::StatisticalFec::new(
                    maximum,
                    fallback,
                    self.config.min_fec,
                    Instant::now(),
                ));
                self.client_fec_on.store(true, Ordering::Release);
                fallback
            }
        };
        *self.adaptive_fec.lock().unwrap() = crate::adaptive_fec::AdaptiveFec::fixed(percent);
        self.fec_percent
            .store(u32::from(percent), Ordering::Release);
        self.fec_split.store(u32::from(percent), Ordering::Release);
        self.fec_split_on.store(true, Ordering::Release);
        usize::from(percent)
    }
    /// The encoder's bitrate for the next frame. With a client-chosen FEC
    /// share F, `bitrate` is the video budget and the encoder gets
    /// budget × 100 / (100 + F), so video and parity together fit it. F moves
    /// only at a `fence`, an IDR or a new encoder, so FEC alone never steps
    /// the rate inside a GOP. Host bitrate adaptation already budgets FEC:
    /// while it runs, the encoder gets `bitrate` as it is.
    ///
    /// The split follows Foundation Sunshine's `encoder_bitrate_from_total_bitrate`
    /// in `src/video.cpp` (AlkaidLab/foundation-sunshine 880f1ef, GPL-3).
    pub fn encoder_bitrate(&self, fence: bool) -> u32 {
        let budget = self.bitrate.load(Ordering::Acquire);
        if !self.fec_split_on.load(Ordering::Acquire) {
            return budget;
        }
        if fence {
            let share = if self.adaptive_bitrate.lock().unwrap().active() {
                0
            } else {
                self.fec_percent() as u32
            };
            self.fec_split.store(share, Ordering::Release);
        }
        fec_split_kbps(budget, self.fec_split.load(Ordering::Acquire))
    }
    /// Takes a 0x5539 FEC report: the report and whether statistical FEC
    /// used it (only under `rlFecPercentage=auto`), or why it was refused.
    pub fn record_fec_report(
        &self,
        payload: &[u8],
    ) -> std::result::Result<(crate::statistical_fec::FecReport, bool), String> {
        let report = crate::statistical_fec::FecReport::decode(payload)
            .ok_or_else(|| format!("malformed ({} bytes)", payload.len()))?;
        let mut client = self.client_fec.lock().unwrap();
        let Some(fec) = client.as_mut() else {
            return Ok((report, false));
        };
        let now = Instant::now();
        fec.report(now, &report).map_err(|e| e.to_string())?;
        let percent = fec.percent(now, self.typical_fec_shards());
        self.fec_percent.store(percent as u32, Ordering::Release);
        Ok((report, true))
    }
    /// Data shards of an average frame at the current bitrate: statistical
    /// FEC's percentage for frames sent in parts and for pacing estimates.
    fn typical_fec_shards(&self) -> usize {
        let bytes = u64::from(self.bitrate.load(Ordering::Relaxed)) * 1000
            / 8
            / u64::from(self.config.fps.max(1));
        self.fec_shards(bytes as usize)
    }
    /// Data shards of a video frame of `payload_bytes`, as the packetizer splits it.
    fn fec_shards(&self, payload_bytes: usize) -> usize {
        (payload_bytes + 8).div_ceil(self.config.packet_size.saturating_sub(16).max(1))
    }
    /// The FEC percentage for a whole video frame of `payload_bytes` about to
    /// be packetized: under `rlFecPercentage=auto` the statistical choice for
    /// its size, otherwise [`Self::frame_fec_percent`].
    pub fn video_fec_percent(&self, recovery: bool, payload_bytes: usize) -> usize {
        if self.client_fec_on.load(Ordering::Acquire)
            && let Some(fec) = self.client_fec.lock().unwrap().as_ref()
        {
            return fec.percent(Instant::now(), self.fec_shards(payload_bytes));
        }
        self.frame_fec_percent(recovery)
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
    /// Configures host bitrate adaptation; again after a roam, to forget the old path
    /// and the client's hints about it.
    pub fn configure_adaptive_bitrate(&self, enabled: bool, maximum_kbps: u32) {
        let mut stream = self.config.clone();
        stream.pyrowave_client_link_mbps = self.client_link_mbps();
        stream.pyrowave_client_pace_mbps = self.client_pace_mbps();
        self.adaptive_bitrate.lock().unwrap().configure(
            enabled,
            &stream,
            maximum_kbps,
            self.fec_percent(),
        );
    }
    pub fn set_client_bitrate(&self, kbps: u32) {
        let mut controller = self.adaptive_bitrate.lock().unwrap();
        controller.client_takeover();
        self.adaptive_video_pacing_bps.store(0, Ordering::Release);
        {
            // The link controller's own writes take this lock too, so none
            // lands after the client's value.
            let mut link = self.link_rate.lock().unwrap();
            link.takeover();
            self.link_sent.set_live(false);
            self.bitrate.store(kbps, Ordering::Release);
        }
        self.launch.warnings.clear("host_bitrate_budget");
        self.launch.warnings.clear(LINK_WARNING);
        self.link_notices.lock().unwrap().note_full(kbps);
    }
    pub fn adaptive_pacing_bps(&self, configured_bps: u64) -> u64 {
        let limit = self.adaptive_video_pacing_bps.load(Ordering::Acquire);
        if limit == 0 {
            configured_bps
        } else {
            configured_bps.min(limit)
        }
    }
    /// Starts the link-rate controller for the stream: `enabled` is
    /// `Config::link_rate_adaptation`, `maximum_kbps` the user's `max_bitrate`.
    pub fn configure_link_rate(&self, enabled: bool, maximum_kbps: u32) {
        self.rebuild_link_rate(&self.config, enabled, maximum_kbps);
    }
    /// A controller for a stream whose mode changed (0x5532). A bitrate it had
    /// lowered goes back to the requested one; what it learned about the link
    /// is forgotten, and a client that took the bitrate over keeps it.
    pub fn rebuild_link_rate(&self, stream: &Negotiated, enabled: bool, maximum_kbps: u32) {
        let mut link = self.link_rate.lock().unwrap();
        if let Some(kbps) = link.restore.take() {
            self.bitrate.store(kbps, Ordering::Release);
        }
        let rate = link
            .rate
            .rebuild(enabled, stream, maximum_kbps, self.fec_percent());
        self.link_sent
            .set_live(rate.state() != crate::link_rate::State::Off);
        *link = crate::link_control::LinkControl::new(rate, self.link_sent.head());
        drop(link);
        self.launch.warnings.clear(LINK_WARNING);
        self.link_notices
            .lock()
            .unwrap()
            .note_full(self.bitrate.load(Ordering::Acquire));
    }
    /// A frame the video sender sent: its wire frame number, packets (data and
    /// parity) and wire bytes. Lock-free, so any sender path may call it.
    pub fn record_link_sent(&self, wire_frame: u32, packets: usize, wire_bytes: u64) {
        self.record_link_sent_at(wire_frame, packets, wire_bytes, Instant::now());
    }
    fn record_link_sent_at(&self, wire_frame: u32, packets: usize, wire_bytes: u64, at: Instant) {
        self.link_sent.record(
            wire_frame,
            packets.min(u32::MAX as usize) as u32,
            wire_bytes,
            at,
        );
    }
    /// A client request for a keyframe or reference invalidation (E4).
    fn link_request(&self) {
        if !self.link_sent.live() {
            return;
        }
        let now = Instant::now();
        let network = self.keyframe_loss.lock().unwrap().network_request(now);
        let mut link = self.link_rate.lock().unwrap();
        link.drain(&self.link_sent);
        link.rate.on_keyframe_request(network, now);
    }
    /// One validated, deduplicated FEC block (E1 and E2).
    fn link_status(&self, payload: &[u8], frame: u32, block: crate::adaptive_fec::Block) {
        if !self.link_sent.live() {
            return;
        }
        let Ok(status) = crate::fec_status::Status::parse(payload) else {
            return;
        };
        let now = Instant::now();
        let (network, abandoned, keyframe) = {
            let loss = self.keyframe_loss.lock().unwrap();
            (
                matches!(block, crate::adaptive_fec::Block::Unrecoverable)
                    && loss.network_loss(frame, now),
                loss.is_abandoned(frame),
                loss.is_keyframe(frame),
            )
        };
        let mut link = self.link_rate.lock().unwrap();
        link.drain(&self.link_sent);
        // Packets of a frame the host stopped sending are its own loss,
        // counted once on send (E3), not as the network's (E2).
        if !abandoned {
            link.rate.on_status(&status, now);
        }
        if network {
            link.rate.on_unrecoverable(frame, keyframe, now);
        }
    }
    /// The running counts behind the pipeline frame rates.
    pub fn pipeline_counts(&self) -> crate::pipeline_rates::Counts {
        self.stats
            .pipeline
            .counts(self.stats.frames.load(Ordering::Relaxed))
    }

    /// The console's pipeline frame rates; `None` until a window is complete.
    fn console_pipeline_rates(&self) -> Option<crate::pipeline_rates::PipelineRates> {
        let counts = self.pipeline_counts();
        self.stats
            .pipeline_console
            .lock()
            .unwrap()
            .sample(
                Instant::now(),
                counts,
                crate::pipeline_rates::CONSOLE_WINDOW,
            )
            .as_ref()
            .map(crate::pipeline_rates::PipelineRates::from)
    }

    /// The link-rate state for the session API and the `stream timings` line.
    pub fn link_rate_snapshot(&self) -> serde_json::Value {
        self.link_rate.lock().unwrap().rate.snapshot(Instant::now())
    }
    /// Call from the feedback tick. Applies the controller's bitrate and says
    /// so, loudly: a WARN on leaving the requested rate, a session warning for
    /// the console, an INFO for later changes. Returns `(old, new)` kbps when
    /// the encoder bitrate changed.
    pub fn tick_link_rate(&self, now: Instant, rtt: Duration) -> Option<(u32, u32)> {
        if !self.link_sent.live() {
            return None;
        }
        let mut link = self.link_rate.lock().unwrap();
        link.drain(&self.link_sent);
        let period = self.stream_mode().period();
        let current = self.bitrate.load(Ordering::Acquire);
        let busy = link.busy(
            now,
            self.stats.frames.load(Ordering::Relaxed),
            self.stats.bytes.load(Ordering::Relaxed),
            period,
            current,
        );
        let target = link
            .rate
            .tick(now, rtt, self.fec_percent(), period, current, busy);
        let applied = match target {
            Some(kbps) => {
                link.restore.get_or_insert(current);
                kbps
            }
            None => link.restore.take().unwrap_or(current),
        };
        if applied != current {
            self.bitrate.store(applied, Ordering::Release);
        }
        // Say it once the lock is released: logging and the warning list must
        // not run under it.
        let report = link
            .rate
            .take_change()
            .map(|change| note_link_change(&mut link, now, change));
        drop(link);
        if let Some(report) = report {
            self.link_notices.lock().unwrap().note(link_notice(&report));
            self.announce_link_change(report);
        }
        (applied != current).then_some((current, applied))
    }
    /// The WARN, the console warning or the INFO for a change.
    fn announce_link_change(&self, report: LinkReport) {
        use crate::link_rate::ChangeKind;
        let LinkReport {
            change,
            was_limited,
            ceiling_kbps: ceiling,
            loss_share,
            lowest_kbps,
            log_info,
        } = report;
        let e = change.evidence;
        let kind = match change.kind {
            ChangeKind::StepDown => "step_down",
            ChangeKind::Probe => "probe",
            ChangeKind::ProbeFailed => "probe_failed",
            ChangeKind::Floor => "floor",
            ChangeKind::EncoderFloor => "encoder_floor",
            ChangeKind::Full => "full",
        };
        match change.kind {
            ChangeKind::StepDown | ChangeKind::Floor | ChangeKind::EncoderFloor => {
                let message = match change.kind {
                    ChangeKind::Floor => format!(
                        "The network to this device cannot carry even {} of video, the lowest bitrate the host uses, so the picture will keep breaking up. Check the network path (Wi-Fi signal, cable, router) or lower the resolution or frame rate in Moonlight.",
                        mbps(change.to_kbps)
                    ),
                    ChangeKind::EncoderFloor => format!(
                        "The network to this device still loses video at {}, and the encoder does not go lower for this picture, so the host stopped lowering the bitrate there and the picture will keep breaking up. Check the network path (Wi-Fi signal, cable, router) or lower the resolution or frame rate in Moonlight.",
                        mbps(change.to_kbps)
                    ),
                    _ => {
                        let loss = if change.reason == "queue" {
                            "the network was holding video back in a growing queue".to_string()
                        } else if loss_share > 0. {
                            format!("{:.0} % of packets lost", loss_share * 100.)
                        } else {
                            "video frames were lost".to_string()
                        };
                        format!(
                            "The network to this device could not carry {} of video ({loss}). The host lowered the bitrate, as low as {} so far, and raises it again as the link allows. Lower the bitrate in Moonlight to avoid this.",
                            mbps(ceiling),
                            mbps(lowest_kbps)
                        )
                    }
                };
                self.launch.warnings.set(LINK_WARNING, message);
            }
            ChangeKind::Full => self.launch.warnings.clear(LINK_WARNING),
            ChangeKind::Probe | ChangeKind::ProbeFailed => {}
        }
        match change.kind {
            ChangeKind::StepDown if !was_limited => tracing::warn!(
                stream_id = %self.launch.id,
                link_rate_change = kind,
                reason = change.reason,
                from_kbps = change.from_kbps,
                to_kbps = change.to_kbps,
                ceiling_kbps = ceiling,
                delivered_kbps = change.delivered_kbps,
                unrecoverable_frames = e.unrecoverable_frames,
                send_losses = e.send_losses,
                keyframe_requests = e.keyframe_requests,
                loss_windows = e.loss_windows,
                loss_share_percent = e.loss_share * 100.,
                "the network to this device cannot carry the stream; lowering the video bitrate"
            ),
            ChangeKind::Floor => tracing::warn!(
                stream_id = %self.launch.id,
                link_rate_change = kind,
                reason = change.reason,
                from_kbps = change.from_kbps,
                to_kbps = change.to_kbps,
                delivered_kbps = change.delivered_kbps,
                unrecoverable_frames = e.unrecoverable_frames,
                send_losses = e.send_losses,
                keyframe_requests = e.keyframe_requests,
                loss_share_percent = e.loss_share * 100.,
                "the network to this device cannot carry the stream even at the lowest video bitrate"
            ),
            ChangeKind::EncoderFloor => tracing::warn!(
                stream_id = %self.launch.id,
                link_rate_change = kind,
                from_kbps = change.from_kbps,
                to_kbps = change.to_kbps,
                delivered_kbps = change.delivered_kbps,
                "the encoder sends more than the video bitrate asked for; treating what it sends as its lowest rate"
            ),
            _ if log_info => {
                if change.kind == ChangeKind::Full {
                    tracing::info!(
                        stream_id = %self.launch.id,
                        link_rate_change = kind,
                        to_kbps = change.to_kbps,
                        "video bitrate back at the requested rate"
                    );
                } else {
                    tracing::info!(
                        stream_id = %self.launch.id,
                        link_rate_change = kind,
                        reason = change.reason,
                        from_kbps = change.from_kbps,
                        to_kbps = change.to_kbps,
                        delivered_kbps = change.delivered_kbps,
                        "video bitrate changed to follow the network"
                    );
                }
            }
            _ => {}
        }
    }
    pub fn configure_automatic_pacing(&self, enabled: bool) {
        *self.automatic_pacing.lock().unwrap() =
            crate::adaptive_pacing::AdaptivePacing::new(enabled);
        self.automatic_pacing_on.store(enabled, Ordering::Release);
    }
    /// Whether the client reports a wireless route (`x-rl-video[0].clientWireless`)
    /// and no adaptive controller owns this stream. Then the route base is
    /// paced as Wi-Fi from the first frame. When the controller owns the stream
    /// the base stays the host route's, so the controller starts conservatively
    /// at its own seed and can probe up toward the real ceiling.
    pub fn client_wireless_sets_route_pacing(&self) -> bool {
        self.config.client_wireless && !self.automatic_pacing_enabled()
    }
    /// Read per frame, so it is the flag set at stream start, not the mutex.
    pub fn automatic_pacing_enabled(&self) -> bool {
        self.automatic_pacing_on.load(Ordering::Acquire)
    }
    pub fn video_pacing_bps(&self, base: u64, needed: u64) -> u64 {
        let paced = self.automatic_pacing.lock().unwrap().rate(
            base,
            needed,
            self.bitrate.load(Ordering::Acquire),
        );
        self.adaptive_pacing_bps(paced)
    }
    /// Encoder-side budget queries must not change the sender's feedback
    /// state when recovery parity differs from the current ordinary-frame FEC.
    pub fn preview_video_pacing_bps(&self, base: u64, needed: u64) -> u64 {
        let paced = self.automatic_pacing.lock().unwrap().preview_rate(
            base,
            needed,
            self.bitrate.load(Ordering::Acquire),
        );
        self.adaptive_pacing_bps(paced)
    }
    /// Pacing slows for the session once the client loses a keyframe or
    /// repeatedly reports unrecoverable ordinary frames
    /// (`keyframe_loss_pacing`), including PyroWave's independent intra frames.
    pub fn configure_keyframe_loss(&self, enabled: bool) {
        self.configure_remembered_keyframe_loss(enabled, false);
    }
    /// Restore only an eligible route's recent loss history. Explicit pacing,
    /// disabled adaptation and PyroWave retain their own policies.
    pub fn configure_remembered_keyframe_loss(&self, enabled: bool, remembered: bool) {
        let remembered = remembered && enabled && self.config.codec != 3;
        *self.keyframe_loss.lock().unwrap() = if remembered {
            crate::keyframe_loss::KeyframeLoss::remembered("remembered from the previous session")
        } else {
            crate::keyframe_loss::KeyframeLoss::new(enabled)
        };
        if remembered {
            tracing::info!(stream_id = %self.launch.id, "pacing video like Wi-Fi from the first frame: remembered from the previous session");
            self.launch.warnings.set("network_keyframe_loss", "This device lost video frames in a previous stream at this address. Video starts at the Wi-Fi pacing rate to limit packet bursts without reducing the encoder bitrate. This memory expires after 24 hours and the host periodically tests the fast rate. Reset it in Devices after changing the network.");
        } else {
            self.launch.warnings.clear("network_keyframe_loss");
        }
    }
    fn report_keyframe_loss(
        &self,
        latched: Option<crate::keyframe_loss::Latched>,
        ignored: Option<String>,
    ) {
        if let Some(why) = ignored {
            tracing::info!(stream_id = %self.launch.id, why, "video loss evidence not counted: the host caused it");
        }
        if let Some(crate::keyframe_loss::Latched {
            reason,
            detail,
            kind,
            confirmed,
        }) = latched
        {
            if self.link_sent.live() {
                let mut link = self.link_rate.lock().unwrap();
                link.drain(&self.link_sent);
                // A provisional latch confirmed by more evidence is the same
                // latch, already counted when it began.
                link.rate.on_latch(
                    Instant::now(),
                    self.pace_was_fast.load(Ordering::Relaxed),
                    !confirmed,
                );
            }
            if confirmed {
                tracing::warn!(stream_id = %self.launch.id, reason, detail, "more video loss after the start-up keyframe; pacing video like a Wi-Fi route for the rest of the stream");
            } else if kind == crate::keyframe_loss::LatchKind::Startup {
                tracing::warn!(stream_id = %self.launch.id, reason, detail, "the stream's first keyframe was lost; pacing video like a Wi-Fi route until it has been clean for 10 s");
            } else {
                tracing::warn!(stream_id = %self.launch.id, reason, detail, "video frames are not arriving; pacing video like a Wi-Fi route for the rest of the stream");
            }
            if kind == crate::keyframe_loss::LatchKind::Startup {
                self.launch.warnings.set("network_keyframe_loss", format!("The first video frame was lost on the way to this device ({reason}), so video is paced like Wi-Fi for now. This limits packet bursts without reducing the encoder bitrate. The host returns to the normal rate after 10 seconds without further loss."));
            } else {
                self.launch.warnings.set("network_keyframe_loss", format!("Video frames were lost on the way to this device ({reason}), so video is now paced like Wi-Fi for the rest of the stream. This limits packet bursts without reducing the encoder bitrate. Check the network path if loss continues."));
            }
        }
    }
    /// Read the loss guard without recording a frame that has not been sent.
    /// Sender selection uses this before deciding whether to send inline.
    pub fn keyframe_loss_rate_bps(&self, base: u64, safe: u64) -> u64 {
        self.keyframe_loss.lock().unwrap().bps(base, safe)
    }
    /// The host stopped sending this frame part way: loss on send. It counts
    /// toward slower pacing under its own reason, and the client's report of
    /// the same frame is not counted again.
    pub fn keyframe_abandoned(&self, wire_frame: u32) {
        let period = self.stream_mode().period();
        let (latched, ignored) = {
            let mut loss = self.keyframe_loss.lock().unwrap();
            (
                loss.abandoned(wire_frame, period, Instant::now()),
                loss.take_ignored(),
            )
        };
        if self.link_sent.live() {
            let mut link = self.link_rate.lock().unwrap();
            link.drain(&self.link_sent);
            link.rate.on_send_loss(wire_frame, Instant::now());
        }
        self.report_keyframe_loss(latched, ignored);
    }
    pub fn keyframe_loss_active(&self) -> bool {
        self.keyframe_loss.lock().unwrap().latched()
    }
    pub fn keyframe_loss_reason(&self) -> Option<&'static str> {
        self.keyframe_loss.lock().unwrap().reason()
    }
    pub fn keyframe_loss_observed_at(&self) -> Option<Instant> {
        self.keyframe_loss.lock().unwrap().observed_at()
    }
    /// When loss last proved sustained: the only loss the next session
    /// inherits. A provisional (start-up) latch has none.
    pub fn keyframe_loss_sustained_at(&self) -> Option<Instant> {
        self.keyframe_loss.lock().unwrap().sustained_at()
    }
    /// Any latch happened in this stream, even a provisional one that lifted.
    pub fn keyframe_loss_ever_latched(&self) -> bool {
        self.keyframe_loss.lock().unwrap().ever_latched()
    }
    /// The pacing rate for the frame about to be sent as `wire_frame`: the
    /// route policy's `base`, or after confirmed losses at most `safe`, the
    /// Wi-Fi route's rate for this stream.
    pub fn keyframe_loss_bps(&self, base: u64, safe: u64, wire_frame: u32, keyframe: bool) -> u64 {
        self.keyframe_loss_bps_at(base, safe, wire_frame, keyframe, Instant::now())
    }
    fn keyframe_loss_bps_at(
        &self,
        base: u64,
        safe: u64,
        wire_frame: u32,
        keyframe: bool,
        now: Instant,
    ) -> u64 {
        let (bps, lifted) = {
            let mut loss = self.keyframe_loss.lock().unwrap();
            loss.sending_at(wire_frame, keyframe, now);
            (loss.bps(base, safe), loss.take_lifted())
        };
        // Fast is what was applied to this frame, not what the route allows.
        let fast = bps > safe;
        self.pace_was_fast.store(fast, Ordering::Relaxed);
        if keyframe && self.link_sent.live() {
            self.link_rate
                .lock()
                .unwrap()
                .rate
                .on_keyframe_sent(wire_frame, now);
        }
        if let Some(why) = lifted {
            tracing::info!(stream_id = %self.launch.id, why, "returning to the route's pacing rate: the start-up loss did not recur");
            self.launch.warnings.clear("network_keyframe_loss");
            if self.link_sent.live() {
                let mut link = self.link_rate.lock().unwrap();
                link.drain(&self.link_sent);
                link.rate.on_unlatch(now, fast);
            }
        }
        bps
    }
    pub fn record_control_ping(&self, now: Instant, payload: &[u8]) {
        self.adaptive_bitrate.lock().unwrap().on_ping(now, payload);
        if self.link_sent.live() {
            self.link_rate.lock().unwrap().rate.on_ping(now, payload);
        }
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
        if let Some(fec) = self.client_fec.lock().unwrap().as_mut() {
            // Reports that stopped hand the stream back to the host's percentage.
            let percent = fec.percent(now, self.typical_fec_shards());
            self.fec_percent.store(percent as u32, Ordering::Release);
            return fec.log_due(now, percent);
        }
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
    /// panel, not the source pixels, so must not update stream HDR metadata;
    /// only the receiver display peak the encoder is told follows them.
    pub fn record_display_caps(&self, payload: &[u8]) -> Option<crate::display_caps::Update> {
        self.hdr_source.lock().unwrap().on_payload(payload)
    }
    /// The receiver's display peak, for dynamic metadata that names a target
    /// display ([`crate::display_caps::HdrSource::target_nits`]).
    pub fn hdr_target_nits(&self) -> u16 {
        self.hdr_source.lock().unwrap().target_nits()
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
        serde_json::json!({"warnings":warnings,"encoder":*self.encoder.read().unwrap(),"uuid":self.launch.client.uuid,"device_name":self.launch.client.name,"width":width,"height":height,"fps":fps,"requested_fps":requested_fps,"video_format":self.config.codec,"hdr":self.config.hdr,"vrr":self.config.vrr_low_latency,"encoder_bitrate_kbps":self.bitrate.load(Ordering::Relaxed),"adaptive_bitrate":self.adaptive_bitrate.lock().unwrap().snapshot(),"link_rate":self.link_rate_snapshot(),"packet_pacing":self.automatic_pacing.lock().unwrap().snapshot(),"fec_percent":self.fec_percent(),"pyrowave_minimum_kbps":(self.config.codec == 3).then(|| crate::pyrowave::minimum_kbps(self.config.width, self.config.height, self.config.fps_millihz())),"pyrowave_recommended_kbps":(self.config.codec == 3).then(|| crate::pyrowave::recommended_kbps(self.config.width, self.config.height, self.config.fps_millihz())),"audio_channels":self.config.audio_channels,"state":if self.stopping(){"STOPPING"}else{"RUNNING"},"frames_sent":self.stats.frames.load(Ordering::Relaxed),"frames_replaced":self.stats.frames_replaced.load(Ordering::Relaxed),"packets_sent":self.stats.packets.load(Ordering::Relaxed),"bytes_sent":self.stats.bytes.load(Ordering::Relaxed),"idr_requests":self.stats.idr_requests.load(Ordering::Relaxed),"reference_invalidations":self.stats.reference_invalidations.load(Ordering::Relaxed),"send_loss_recoveries":self.stats.send_loss_recoveries.load(Ordering::Relaxed),"slice_delivery_overflows":self.stats.slice_delivery_overflows.load(Ordering::Relaxed),"encode_latency_ms":self.stats.latency_us.load(Ordering::Relaxed) as f64/1000.,"performance":performance.snapshot(),"pipeline_rates":self.console_pipeline_rates(),"uptime_seconds":self.started.elapsed().as_secs_f64(),"role":self.launch.role})
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
    /// Remove the launches whose clients never started their streams, each
    /// with a warning: the device got a launch reply and then gave up.
    #[must_use = "drop expired launches after releasing the sessions lock"]
    pub fn expire(&mut self) -> Vec<Launch<P, A>> {
        let expired = self.remove_pending(|p| !p.live());
        for launch in &expired {
            tracing::warn!(
                client = %launch.client.name,
                id = %launch.id,
                claimed = launch.claimed_until.is_some(),
                "launch expired before its stream started"
            );
        }
        expired
    }
    /// An authenticated RTSP request for launch `id` claims it: the launch
    /// then waits [`RTSP_LEASE`] for the next request, even past
    /// [`LAUNCH_WAIT`], up to [`CLAIM_LIMIT`]. A launch still being prepared,
    /// expired, replaced or already started is refused; the request of a
    /// running stream (its TEARDOWN) passes. Only [`Sessions::start`]
    /// consumes a launch, once.
    pub fn claim(&mut self, id: &str) -> Result<()> {
        let Some(launch) = self.pending.get_mut(id) else {
            if self.active.get(id).is_some_and(|s| !s.stopping()) {
                return Ok(());
            }
            bail!("the launch was replaced, canceled or already started");
        };
        launch.claimable()?;
        launch.claimed_until = Some(launch.lease(Instant::now()));
        Ok(())
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
        let mut found = self
            .pending
            .values()
            .filter(|p| p.peer == peer && p.claimable().is_ok());
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
            .filter(|p| p.claimable().is_ok())
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
        // launch at PLAY so a stale snapshot cannot revive an expired launch,
        // or start one still being prepared. Leave expired preparations for
        // expire() to release outside the lock.
        match self.pending.get(&launch.id) {
            None => bail!("launch expired or already consumed"),
            Some(pending) => pending.claimable()?,
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
    /// A stream launch waits for its client, or is still being prepared.
    pub fn stream_pending(&self) -> bool {
        self.pending
            .values()
            .any(|p| p.role == Role::Stream && p.live())
    }
    /// The devices with a running (not stopping) stream session.
    pub fn streaming_devices(&self) -> std::collections::BTreeSet<String> {
        self.active
            .values()
            .filter(|s| s.launch.role == Role::Stream && !s.stopping())
            .map(|s| s.launch.client.uuid.clone())
            .collect()
    }
    /// A new session over one transport ends the sessions of the other in the same role: a
    /// WebRTC stream and a Moonlight stream never run together, whichever came first. Sessions
    /// of the same transport follow the per-client rules of `supersede`.
    #[must_use = "drop canceled launches after releasing the sessions lock"]
    pub fn supersede_other_transport(
        &mut self,
        role: Role,
        transport: Transport,
    ) -> Vec<Launch<P, A>> {
        let canceled = self.remove_pending(|p| p.role == role && p.transport != transport);
        for s in self.active.values() {
            if s.launch.role == role && s.launch.transport != transport {
                s.stop();
            }
        }
        canceled
    }
    /// Whether another device has a live session of this transport and role: pending, or running
    /// and not stopping.
    pub fn held_by_other(&self, role: Role, transport: Transport, client: &str) -> bool {
        let other = |l: &Launch<P, A>| {
            l.role == role && l.transport == transport && l.client.uuid != client
        };
        self.pending.values().any(|p| p.live() && other(p))
            || self
                .active
                .values()
                .any(|s| !s.stopping() && other(&s.launch))
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
