//! Away wake and LAN Wake-on-LAN coordination helpers for `App`: handles `WakeRelayDone`,
//! keeps `waking` alive while `poll_one` confirms `Reach::Online` after `Outcome::Awake`,
//! and advances each `wake_step` tick cleanly.

use super::super::*;

pub(crate) struct WakeContext<'a> {
    pub(crate) hosts: &'a mut Hosts,
    pub(crate) waking: &'a mut wol::Wakes,
    pub(crate) away: &'a mut AwayWakeUi,
    pub(crate) launcher: &'a mut ui::Launcher,
}

pub(crate) fn wake_step_at(ctx: &mut WakeContext<'_>, now: Instant) {
    let Some(waking) = ctx.waking.current() else {
        return;
    };
    let (id, run) = (waking.host, waking.run);
    let Some(host) = ctx.hosts.get(id) else {
        ctx.hosts.cancel_wake();
        ctx.waking.end();
        ctx.away.clear();
        return;
    };
    let (host_reach, host_name) = (host.reach, host.display_name().to_owned());
    if ctx.away.active {
        if host_reach == hosts::Reach::Online {
            ctx.hosts.cancel_wake();
            ctx.waking.end();
            ctx.away.clear();
            return;
        }
        if ctx.away.awake_until.is_some_and(|deadline| now >= deadline) {
            ctx.hosts.cancel_wake();
            ctx.waking.end();
            ctx.away.clear();
            ctx.launcher.ask(Dialog::WakeFailed {
                host: id,
                title: tr("Could not wake the PC").to_owned(),
                body: trf("{} did not answer in time.", &[&host_name]),
            });
        }
        return;
    }
    match waking.wait.next(host_reach == hosts::Reach::Online, now) {
        wol::Step::Send => ctx.hosts.wake(id, run),
        wol::Step::Wait => {}
        wol::Step::Online => {
            ctx.waking.end();
            ctx.away.clear();
        }
        wol::Step::TimedOut => {
            if let Some(status) = ctx.hosts.start_away_wake_fallback(id, run) {
                ctx.away.begin(status);
                return;
            }
            let hint = ctx.hosts.no_relay_hint(id);
            ctx.waking.end();
            ctx.away.clear();
            let mut body = trf(
                "{} did not answer within {} seconds. Check that Wake-on-LAN is on in its firmware and its network adapter settings.",
                &[&host_name, &wol::WakeWait::TIMEOUT.as_secs()],
            );
            if let Some(hint) = hint {
                body.push_str("\n\n");
                body.push_str(&hint);
            }
            ctx.launcher.notify(tr("The PC did not wake up"), body);
        }
    }
}

pub(crate) fn on_wake_relay_done_at(
    ctx: &mut WakeContext<'_>,
    id: u64,
    run: u64,
    name: &str,
    outcome: hosts::wake_relay::Outcome,
    relays: &[String],
    now: Instant,
) {
    if !ctx
        .waking
        .current()
        .is_some_and(|w| w.host == id && w.run == run)
    {
        return;
    }
    if outcome == hosts::wake_relay::Outcome::Awake {
        if ctx
            .hosts
            .get(id)
            .is_some_and(|h| h.reach == hosts::Reach::Online)
        {
            ctx.waking.end();
            ctx.away.clear();
        } else {
            let status = hosts::wake_relay::outcome_message(&outcome, name, relays)
                .unwrap_or_else(|| trf("{} did not answer in time.", &[&name]));
            ctx.away.mark_awake(status, now + WAKE_RELAY_POLL_TIMEOUT);
        }
        return;
    }
    if ctx.waking.failed(id, run) {
        ctx.away.clear();
        if let Some(body) = hosts::wake_relay::outcome_message(&outcome, name, relays) {
            if outcome.offers_retry() {
                ctx.launcher.ask(Dialog::WakeFailed {
                    host: id,
                    title: tr("Could not wake the PC").to_owned(),
                    body,
                });
            } else {
                ctx.launcher.notify(tr("Could not wake the PC"), body);
            }
        }
    }
}
