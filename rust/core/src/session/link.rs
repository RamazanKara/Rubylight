//! What the session says about link-rate changes: the log lines' and the
//! client notice's bookkeeping.
use std::time::{Duration, Instant};

/// The session warning code while the link-rate controller holds the video
/// bitrate below the requested one.
pub const LINK_WARNING: &str = "network_bitrate_limited";

/// Update the bookkeeping for a change, under the lock, and return what to
/// say about it.
pub(super) fn note_link_change(
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
    // step down and the floors have their own WARN. A PyroWave stream with no
    // room under it reaches its floor again after every quiet spell: once.
    let no_room = link.rate.pyrowave_no_room();
    let warned = match change.kind {
        ChangeKind::StepDown => !was_limited,
        ChangeKind::Floor if no_room => !std::mem::replace(&mut link.floor_warned, true),
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
        warned,
        no_room,
        ceiling_kbps: link.rate.ceiling_kbps(),
        loss_share: link.loss_share,
        lowest_kbps: link.lowest_kbps,
        log_info,
    }
}

/// What to say about a link-rate change, worked out under the controller's
/// lock and said after it.
pub(super) struct LinkReport {
    pub(super) change: crate::link_rate::Change,
    pub(super) was_limited: bool,
    /// The change has its own WARN line.
    pub(super) warned: bool,
    /// PyroWave's floor is the stream's own bitrate.
    pub(super) no_room: bool,
    pub(super) ceiling_kbps: u32,
    pub(super) loss_share: f64,
    pub(super) lowest_kbps: u32,
    pub(super) log_info: bool,
}

/// What a client that asked for notices (0x5538) is told about a change.
pub(super) fn link_notice(report: &LinkReport) -> crate::link_notice::LinkNotice {
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
pub(super) fn mbps(kbps: u32) -> String {
    if kbps >= 10_000 {
        format!("{} Mbps", (f64::from(kbps) / 1000.).round())
    } else {
        format!("{:.1} Mbps", f64::from(kbps) / 1000.)
    }
}
