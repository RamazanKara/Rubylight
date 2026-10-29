//! The stream doctor on the desktop: nightjar-policy's `StreamHealth` and `BitrateTrial`, the same
//! code Android's report runs (docs/stream-health.md). The stream feeds it one stats tick a
//! second with the host and display p95 times collected here; the stream menu shows its report
//! and offers the trial, and the launcher shows the report after a stream that had findings.

use nightjar_policy::health::{BitrateTrial, Finding, Report, StreamHealth, TrialOffer, TrialRefused};
use nightjar_policy::stats::{LateFrames, RecentP95, Stats};

use crate::strings::{tr, trf};

#[derive(Default)]
pub struct Doctor {
    health: StreamHealth,
    pub trial: BitrateTrial,
    /// The frame header's host processing time of the frames shown.
    header: RecentP95,
    /// The host timing reports' present to last packet sent.
    sent: RecentP95,
    /// Decoded to shown: the pacer's hold and the present.
    display: RecentP95,
    /// Frames that arrived late against the host's clock, this tick.
    late: LateFrames,
}

impl Doctor {
    /// A decoded frame: its RTP timestamp and when its first packet arrived (session clock), at
    /// the stream's frame rate.
    pub fn arrived(&mut self, rtp: u32, receive_us: u64, fps: u32) {
        self.late.push(rtp, receive_us, fps);
    }

    /// A picture was shown: its header's processing time (100 us units, 0 when the host sends
    /// none) and its display wait from `ready_us` to `presented_us` (session clock).
    pub fn shown(&mut self, host_latency: u16, ready_us: u64, presented_us: u64) {
        if host_latency != 0 {
            self.header.push(u64::from(host_latency) * 100);
        }
        if ready_us != 0 && presented_us >= ready_us {
            self.display.push(presented_us - ready_us);
        }
    }

    /// One of the host's timing reports: present to last packet sent, in microseconds.
    pub fn host_sent(&mut self, host_us: u32) {
        self.sent.push(u64::from(host_us));
    }

    /// Fills the tick's host and display p95 over the last two ticks, then starts the next one.
    /// As on Android, the host is judged by its present to last packet sent while it reports
    /// that, otherwise by the header's processing time.
    pub fn timings(&mut self, tick: &mut Stats) {
        tick.host_includes_send = self.sent.has_samples();
        let (sent, header) = (self.sent.tick(), self.header.tick());
        tick.host_p95_ms = if tick.host_includes_send { sent } else { header };
        tick.display_p95_ms = self.display.tick();
        tick.late_frames = Some(self.late.tick());
    }

    pub fn add(&mut self, tick: &Stats) {
        self.health.add(tick);
    }

    pub fn report(&mut self, now_ms: u64) -> Report {
        self.health.report(now_ms)
    }
}

/// A value of the evidence, or "no data yet".
fn measurement(value: f32) -> String {
    if value.is_finite() && value >= 0.0 {
        number(f64::from(value), 1)
    } else {
        tr("no data yet").to_owned()
    }
}

/// `value` with `decimals`, with a decimal comma in German.
fn number(value: f64, decimals: usize) -> String {
    let text = format!("{value:.decimals$}");
    if crate::strings::current() == crate::strings::Lang::De { text.replace('.', ",") } else { text }
}

/// A stall rhythm as Android words it: seconds below 90 s, else minutes to the half minute.
fn period(period_ms: u64) -> String {
    let seconds = (period_ms + 500) / 1000;
    if seconds < 90 {
        return trf("{} s", &[&seconds]);
    }
    let minutes = (seconds as f64 / 30.0).round() / 2.0;
    trf("{} min", &[&if minutes.fract() == 0.0 { format!("{minutes:.0}") } else { number(minutes, 1) }])
}

/// The report as the stream menu and the screen after the stream show it.
pub fn report_text(report: &Report) -> String {
    let mut text = trf("Active samples: {}. Evidence covers the last 30 seconds. Several stages can be slow at once; these observations suggest causes, not a guaranteed diagnosis. No video or personal information is recorded.", &[&report.samples]);
    if !report.recent || report.samples < 3 {
        text.push_str("\n\n");
        text.push_str(tr("Not enough recent video to assess. Play for a few seconds with the menu closed, then return just after a stutter."));
    } else if report.findings.is_empty() {
        text.push_str("\n\n");
        text.push_str(tr("No repeated bottleneck was observed in the available measurements. Short spikes and unmeasured stages can still cause a stutter."));
    }
    for finding in &report.findings {
        text.push_str("\n\n");
        match finding {
            Finding::Network => text.push_str(tr("Network: repeated frame loss or round-trip variation. Try a lower bitrate, a wired host or a less congested Wi-Fi connection.")),
            Finding::Decoder => text.push_str(tr("Decode: output repeatedly took at least one frame interval. Try a lower resolution or frame rate; lowering bitrate alone may not help.")),
            Finding::Host => text.push_str(tr("Host: the slower processing samples exceeded the frame interval. Compare game/GPU load and a lower game frame limit. This measurement includes more than encoding.")),
            Finding::Presentation => text.push_str(tr("Presentation: frames were released faster than they were shown, with long display waits. Compare frame-pacing modes and the screen refresh rate.")),
            Finding::Heat => text.push_str(tr("Temperature: this device reported high thermal pressure. Lower frame rate/resolution or disable expensive client processing; allow the device to cool.")),
            Finding::Bursts => text.push_str(&trf("Bursts: in {} of the last {} seconds, frames arrived late and then several at once, although the host sent them on time. Something between the host and this device held them back, often Wi-Fi power saving. Try a wired connection, or a 5 or 6 GHz network closer to the router. On a laptop, use mains power or turn off power saving for the Wi-Fi adapter.", &[&report.late_samples, &report.samples])),
            Finding::Periodic => text.push_str(&trf("Repeating stalls: {} network stalls in the last 20 minutes started about every {}. A steady rhythm like this usually comes from background Wi-Fi scans on this device or on the PC. To reduce them:\n• Connect this device or the PC by cable.\n• Stop tools that scan for Wi-Fi networks or nearby devices while streaming.", &[&report.stalls, &period(report.stall_period_ms)])),
        }
    }
    let evidence = [
        measurement(report.worst_loss_percent),
        measurement(report.worst_decode_ms),
        measurement(report.worst_host_p95_ms),
        measurement(report.worst_display_p95_ms),
    ];
    let args: Vec<&dyn std::fmt::Display> = evidence.iter().map(|value| value as &dyn std::fmt::Display).collect();
    text.push_str("\n\n");
    text.push_str(&if report.host_includes_send {
        trf("Largest observed values:\nFrame loss: {} %\nDecode: {} ms\nHost to network p95: {} ms\nDisplay wait p95: {} ms\nThese stages do not add up to input-to-screen latency.", &args)
    } else {
        trf("Largest observed values:\nFrame loss: {} %\nDecode: {} ms\nHost processing p95: {} ms\nDisplay wait p95: {} ms\nThese stages do not add up to input-to-screen latency.", &args)
    });
    text
}

/// The trial's help, under the report while a trial can be tried or undone.
pub fn trial_help() -> &'static str {
    tr("A bitrate trial changes only this session. Compare another 30 seconds of similar gameplay, then return here to undo it. Manual or automatic bitrate changes replace the trial.")
}

pub fn offer_label(offer: TrialOffer) -> Option<&'static str> {
    match offer {
        TrialOffer::None => None,
        TrialOffer::Try => Some(tr("Try a lower bitrate")),
        TrialOffer::Undo => Some(tr("Undo bitrate trial")),
    }
}

/// The report after a stream, when it found something worth a look.
pub fn summary(report: &Report) -> Option<String> {
    (report.samples >= 3 && !report.findings.is_empty()).then(|| report_text(report))
}

/// The launcher's message after a stream: why it ended (`ended`, translated here) and the
/// [`summary`], or whichever of them there is.
pub fn after_stream(ended: Option<String>, health: Option<String>) -> Option<(&'static str, String)> {
    match (ended, health) {
        (Some(ended), Some(health)) => Some((tr("The stream ended"), format!("{}\n\n{health}", tr(&ended)))),
        (Some(ended), None) => Some((tr("The stream ended"), tr(&ended).to_owned())),
        (None, Some(health)) => Some((tr("Why did my stream stutter?"), health)),
        (None, None) => None,
    }
}

/// The report for the log: findings and evidence, without the advice.
pub fn log_line(report: &Report) -> String {
    let findings: Vec<&str> = report.findings.iter().map(|finding| finding.name()).collect();
    format!(
        "{} over {} samples, worst loss {:.1} %, decode {:.1} ms, host p95 {:.1} ms{}, display p95 {:.1} ms, {} stalls{}, late frames in {} samples",
        if findings.is_empty() { "no findings".to_owned() } else { findings.join(",") },
        report.samples, report.worst_loss_percent, report.worst_decode_ms, report.worst_host_p95_ms,
        if report.host_includes_send { " (to network)" } else { "" }, report.worst_display_p95_ms, report.stalls,
        if report.stall_period_ms > 0 { format!(" every {} ms", report.stall_period_ms) } else { String::new() },
        report.late_samples,
    )
}

#[derive(Debug, PartialEq)]
pub enum TrialError {
    Refused(TrialRefused),
    /// The host could not be asked; the text says why.
    Failed(String),
}

/// What the worker brings back from a trial step: the live encoder bitrate it started from (0
/// when unknown) and the bitrate the host applied.
#[derive(Debug, PartialEq)]
pub struct TrialOutcome {
    pub undo: bool,
    pub live_kbps: u32,
    pub result: Result<u32, TrialError>,
}

/// One trial step off the main thread: `live` reads the host's live encoder bitrate, `set` asks
/// for a bitrate and returns the one applied. `trial` is a copy; [`finished`] applies the outcome
/// to the stream's own.
pub fn run_trial(
    mut trial: BitrateTrial,
    undo: bool,
    live: impl FnOnce() -> Result<u32, String>,
    set: impl FnOnce(u32) -> Result<u32, String>,
) -> TrialOutcome {
    let live_kbps = match live() {
        Ok(kbps) => kbps,
        Err(error) => return TrialOutcome { undo, live_kbps: 0, result: Err(TrialError::Failed(error)) },
    };
    let result = match trial.begin(undo, live_kbps) {
        Ok(target) => set(target).map_err(TrialError::Failed),
        Err(refused) => Err(TrialError::Refused(refused)),
    };
    TrialOutcome { undo, live_kbps, result }
}

/// Applies a trial step's outcome: the bitrate now in effect when it is known, and the toast.
pub fn finished(trial: &mut BitrateTrial, outcome: TrialOutcome) -> (Option<u32>, String) {
    match outcome.result {
        Ok(applied) => {
            trial.applied(outcome.undo, outcome.live_kbps, applied);
            (Some(applied), trf("Host applied {} Mbps", &[&number(f64::from(applied) / 1000.0, 1)]))
        }
        // The trial is kept: nothing changed, and the host may report its bitrate next time.
        Err(TrialError::Refused(TrialRefused::Unavailable)) => {
            (None, tr("The host did not report its current encoder bitrate. The trial was not applied.").to_owned())
        }
        Err(TrialError::Refused(refused)) => {
            trial.clear();
            let text = if refused == TrialRefused::Replaced {
                tr("The host bitrate has changed since the trial. Undo was cancelled to preserve that change.")
            } else {
                tr("The encoder is already at the minimum trial bitrate. No change was made.")
            };
            (Some(outcome.live_kbps), text.to_owned())
        }
        Err(TrialError::Failed(error)) => (None, error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strings::{set, Lang};

    fn tick(now_ms: u64, loss: f32) -> Stats {
        Stats {
            now_ms,
            recent_video: true,
            fps: 60,
            loss_percent: Some(loss),
            jitter_ms: Some(1.0),
            decode_ms: Some(2.0),
            received_fps: Some(60.0),
            released_fps: Some(60.0),
            shown_fps: Some(60.0),
            ..Stats::default()
        }
    }

    #[test]
    fn the_host_is_judged_by_its_reports_while_it_sends_them() {
        let mut doctor = Doctor::default();
        for units in [20u16, 30, 40] {
            doctor.shown(units, 1_000, 6_000);
        }
        doctor.shown(0, 0, 9_000);
        let mut first = Stats::default();
        doctor.timings(&mut first);
        assert_eq!((first.host_p95_ms, first.host_includes_send), (Some(4.0), false), "the header's 4 ms, 100 us units");
        assert_eq!(first.display_p95_ms, Some(5.0), "frames without a decode time are not counted");
        doctor.host_sent(12_000);
        let mut second = Stats::default();
        doctor.timings(&mut second);
        assert_eq!((second.host_p95_ms, second.host_includes_send), (Some(12.0), true));
        assert_eq!(second.display_p95_ms, Some(5.0), "the previous tick still counts");
        let mut third = Stats::default();
        doctor.timings(&mut third);
        assert_eq!((third.host_p95_ms, third.host_includes_send), (Some(12.0), true), "reports from the last two ticks");
        let mut fourth = Stats::default();
        doctor.timings(&mut fourth);
        assert_eq!((fourth.host_p95_ms, fourth.host_includes_send, fourth.display_p95_ms), (None, false, None));
    }

    #[test]
    fn a_lossy_stream_reports_the_network_and_offers_the_trial() {
        set(Lang::En);
        let mut doctor = Doctor::default();
        for n in 1..=5 {
            let mut stats = tick(n * 1000, 6.0);
            doctor.timings(&mut stats);
            doctor.add(&stats);
        }
        let report = doctor.report(5000);
        assert_eq!(report.findings, [Finding::Network]);
        assert!(report.can_suggest_bitrate());
        assert_eq!(doctor.trial.offer(true, 20_000, true, false, false), TrialOffer::Try);
        let text = report_text(&report);
        assert!(text.starts_with("Active samples: 5."), "{text}");
        assert!(text.contains("Network: repeated frame loss"), "{text}");
        assert!(text.contains("Frame loss: 6.0 %\nDecode: 2.0 ms\nHost processing p95: no data yet ms"), "{text}");
        assert_eq!(summary(&report).as_deref(), Some(text.as_str()));
        assert!(log_line(&report).starts_with("NETWORK over 5 samples, worst loss 6.0 %"));
    }

    #[test]
    fn a_clean_stream_has_no_summary_after_it() {
        set(Lang::En);
        let mut doctor = Doctor::default();
        for n in 1..=5 {
            doctor.add(&tick(n * 1000, 0.0));
        }
        let report = doctor.report(5000);
        assert!(report.findings.is_empty());
        assert_eq!(summary(&report), None);
        assert!(report_text(&report).contains("No repeated bottleneck"));
        let short = Doctor::default().report(1000);
        assert!(report_text(&short).contains("Not enough recent video"));
        assert_eq!(summary(&short), None);
    }

    #[test]
    fn the_report_reads_in_german_with_decimal_commas() {
        set(Lang::De);
        let mut doctor = Doctor::default();
        for n in 1..=5 {
            doctor.add(&Stats { host_p95_ms: Some(30.0), host_includes_send: true, ..tick(n * 1000, 2.5) });
        }
        let text = report_text(&doctor.report(5000));
        assert!(text.contains("Bildverlust: 2,5 %"), "{text}");
        assert!(text.contains("Host bis Netzwerk p95: 30,0 ms"), "{text}");
        assert!(text.contains("PC: langsamere Verarbeitungszeiten"), "{text}");
        assert_eq!(period(150_000), "2,5 min");
        assert_eq!(period(120_400), "2 min");
        assert_eq!(period(45_000), "45 s");
    }

    #[test]
    fn a_trial_lowers_by_a_fifth_and_undoes_to_the_live_rate_it_left() {
        set(Lang::En);
        let mut trial = BitrateTrial::default();
        let asked = std::cell::Cell::new(0);
        let outcome = run_trial(trial, false, || Ok(20_000), |kbps| { asked.set(kbps); Ok(kbps) });
        assert_eq!(asked.get(), 16_000);
        assert_eq!(finished(&mut trial, outcome), (Some(16_000), "Host applied 16.0 Mbps".to_owned()));
        assert_eq!(trial.offer(false, 16_000, true, false, false), TrialOffer::Undo);
        let outcome = run_trial(trial, true, || Ok(16_000), |kbps| { asked.set(kbps); Ok(kbps) });
        assert_eq!(asked.get(), 20_000);
        assert_eq!(finished(&mut trial, outcome), (Some(20_000), "Host applied 20.0 Mbps".to_owned()));
        assert_eq!(trial, BitrateTrial::default());
    }

    #[test]
    fn a_refused_or_failed_trial_says_why_and_never_asks_the_host() {
        set(Lang::En);
        let never = |_: u32| -> Result<u32, String> { panic!("the host must not be asked") };
        let mut trial = BitrateTrial::default();
        let unavailable = run_trial(trial, false, || Ok(0), never);
        assert_eq!(unavailable.result, Err(TrialError::Refused(TrialRefused::Unavailable)));
        assert!(finished(&mut trial, unavailable).1.contains("did not report its current encoder bitrate"));
        let minimum = run_trial(trial, false, || Ok(400), never);
        assert_eq!(finished(&mut trial, minimum), (Some(400), "The encoder is already at the minimum trial bitrate. No change was made.".to_owned()));
        let failed = run_trial(trial, false, || Err("The PC does not allow bitrate changes.".into()), never);
        assert_eq!(finished(&mut trial, failed), (None, "The PC does not allow bitrate changes.".to_owned()));
        // Something else changed the bitrate after a trial: undo keeps that change.
        trial.applied(false, 20_000, 16_000);
        let replaced = run_trial(trial, true, || Ok(12_000), never);
        let (kbps, text) = finished(&mut trial, replaced);
        assert_eq!(kbps, Some(12_000));
        assert!(text.contains("Undo was cancelled"), "{text}");
        assert_eq!(trial, BitrateTrial::default());
        // A host that applies something else is the trial's bitrate from then on.
        let outcome = run_trial(trial, false, || Ok(10_000), |_| Ok(8_500));
        finished(&mut trial, outcome);
        assert_eq!((trial.original_kbps(), trial.trial_kbps()), (10_000, 8_500));
        let failed_set = run_trial(trial, true, || Ok(8_500), |_| Err("timed out".into()));
        assert_eq!(finished(&mut trial, failed_set), (None, "timed out".to_owned()));
        assert_eq!(trial.trial_kbps(), 8_500, "a failed request keeps the trial to undo later");
    }

    #[test]
    fn the_report_follows_why_the_stream_ended() {
        set(Lang::De);
        assert_eq!(after_stream(None, None), None);
        assert_eq!(after_stream(None, Some("Netzwerk".into())), Some(("Warum hat der Stream geruckelt?", "Netzwerk".to_owned())));
        // The reason is translated before the report joins it; the dialog cannot translate both at once.
        let reason = "No video arrived from the PC. A firewall may be blocking UDP ports 47998-48000.";
        let (title, body) = after_stream(Some(reason.into()), Some("Netzwerk".into())).unwrap();
        assert_eq!(title, "Der Stream wurde beendet");
        assert_eq!(body, format!("{}\n\nNetzwerk", tr(reason)));
        assert_ne!(tr(reason), reason, "the reason has a German text");
        assert_eq!(after_stream(Some(reason.into()), None), Some(("Der Stream wurde beendet", tr(reason).to_owned())));
    }
}
