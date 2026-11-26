//! The finish screen after an interactive install: **Open Rubylight Console**
//! and **Done**, after waiting at most 3 s for the freshly started host.
//!
//! Strictly read-only and run only once the install has committed. The spec's
//! pairing QR and PIN are not shown here: the host hands them out only through
//! `POST /api/pairing/qr`, which needs a console sign-in (a first install has
//! none yet, and setup never reads or bypasses an existing one) and which
//! mints a new one-time PIN, so calling it would write host state. Until the
//! host offers a legitimate read-only path, the finish screen is the fallback
//! prompt; the console's Devices page shows the QR code.
use std::{
    net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream},
    time::{Duration, Instant},
};

/// How long setup waits for the host it just started.
pub const HOST_WAIT: Duration = Duration::from_secs(3);
/// Between two looks at the host.
const POLL: Duration = Duration::from_millis(250);
/// One look at the host's port.
const CONNECT: Duration = Duration::from_millis(200);

pub const HEADING: &str = "Rubylight is installed";
pub const OPEN_CONSOLE: &str = "Open Rubylight Console";
const STARTING: &str =
    "Rubylight is still starting. If the console does not load yet, try again in a minute.";

/// The host after an install, as the finish screen tells it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Host {
    /// Its console answers.
    Ready,
    /// Started, but it did not answer within [`HOST_WAIT`].
    Starting,
    /// Setup was told not to start it.
    NotStarted,
}

/// What the end of an install shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Finish {
    /// Quiet, unattended or CI: nothing, and the exit path is unchanged.
    Skip,
    /// The finish prompt with Open Rubylight Console and Done.
    Prompt(Host),
}

/// Decide the finish screen; `wait` looks for the host only when there is one to wait for.
pub fn plan(quiet: bool, started: bool, wait: impl FnOnce() -> bool) -> Finish {
    if quiet {
        return Finish::Skip;
    }
    if !started {
        return Finish::Prompt(Host::NotStarted);
    }
    Finish::Prompt(if wait() { Host::Ready } else { Host::Starting })
}

/// Look with `probe` until it answers or `limit` has passed since the first look;
/// `sleep` never takes the wait past `limit`.
pub fn wait_for(
    limit: Duration,
    poll: Duration,
    mut now: impl FnMut() -> Instant,
    mut sleep: impl FnMut(Duration),
    mut probe: impl FnMut() -> bool,
) -> bool {
    let start = now();
    loop {
        if probe() {
            return true;
        }
        let spent = now().saturating_duration_since(start);
        if spent >= limit {
            return false;
        }
        sleep(poll.min(limit - spent));
    }
}

/// Whether the host's console accepts a connection on this PC. Connect only (no listen,
/// nothing sent): loopback, so no firewall prompt, and nothing on the host changes.
fn console_answers(port: u16) -> bool {
    [
        SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
        SocketAddr::from((Ipv6Addr::LOCALHOST, port)),
    ]
    .iter()
    .any(|address| TcpStream::connect_timeout(address, CONNECT).is_ok())
}

/// The finish text: the install's summary, and a word on a host still starting.
pub fn text(summary: &str, host: Host) -> String {
    match host {
        Host::Starting => format!("{summary}\n\n{STARTING}"),
        Host::Ready | Host::NotStarted => summary.to_owned(),
    }
}

/// Show the finish screen unless setup is quiet; open the console if asked.
pub fn show_if_interactive(quiet: bool, started: bool, web_port: u16, summary: &str) {
    // The last look (two connects) still ends within HOST_WAIT.
    let limit = HOST_WAIT - 2 * CONNECT;
    let finish = plan(quiet, started, || {
        wait_for(limit, POLL, Instant::now, std::thread::sleep, || {
            console_answers(web_port)
        })
    });
    let Finish::Prompt(host) = finish else {
        return;
    };
    crate::log::line(format!("finish screen: host {host:?}"));
    if crate::ui::finished(
        crate::TITLE,
        HEADING,
        &text(summary, host),
        true,
        Some(OPEN_CONSOLE),
    ) {
        crate::open(&format!("https://localhost:{web_port}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn quiet_setup_skips_the_finish_screen_without_waiting() {
        assert_eq!(
            plan(true, true, || panic!("quiet setup never waits")),
            Finish::Skip
        );
        assert_eq!(plan(true, false, || unreachable!()), Finish::Skip);
    }

    #[test]
    fn interactive_setup_waits_only_for_a_started_host() {
        assert_eq!(
            plan(false, false, || panic!("no host to wait for")),
            Finish::Prompt(Host::NotStarted)
        );
        assert_eq!(plan(false, true, || true), Finish::Prompt(Host::Ready));
        assert_eq!(plan(false, true, || false), Finish::Prompt(Host::Starting));
    }

    /// A clock that moves only when the wait sleeps (and a little per look).
    fn run(answer_at: Option<u32>, limit: Duration) -> (bool, u32, Duration) {
        let start = Instant::now();
        let elapsed = Cell::new(Duration::ZERO);
        let looks = Cell::new(0u32);
        let answered = wait_for(
            limit,
            POLL,
            || start + elapsed.get(),
            |pause| elapsed.set(elapsed.get() + pause),
            || {
                looks.set(looks.get() + 1);
                elapsed.set(elapsed.get() + CONNECT);
                answer_at.is_some_and(|at| looks.get() >= at)
            },
        );
        (answered, looks.get(), elapsed.get())
    }

    #[test]
    fn the_wait_is_bounded_and_ends_as_soon_as_the_host_answers() {
        let (answered, looks, _) = run(Some(1), HOST_WAIT);
        assert!(answered);
        assert_eq!(looks, 1);
        let (answered, looks, _) = run(Some(3), HOST_WAIT);
        assert!(answered);
        assert_eq!(looks, 3);
        // Never answering: it gives up once the limit has passed, plus at most one look.
        let (answered, looks, elapsed) = run(None, HOST_WAIT);
        assert!(!answered);
        assert!(looks > 1);
        assert!(elapsed >= HOST_WAIT);
        assert!(elapsed <= HOST_WAIT + CONNECT, "{elapsed:?}");
    }

    #[test]
    fn a_host_still_starting_is_said_and_never_an_error() {
        assert_eq!(text("Installed.", Host::Ready), "Installed.");
        assert_eq!(text("Installed.", Host::NotStarted), "Installed.");
        let starting = text("Installed.", Host::Starting);
        assert!(starting.starts_with("Installed.\n\n"));
        assert!(starting.contains("still starting"));
        assert!(!starting.to_lowercase().contains("error"));
    }

    #[test]
    fn nothing_answering_on_a_closed_port_is_no_host() {
        // Port 1 on loopback is closed on any PC that runs these tests; connect only.
        assert!(!console_answers(1));
    }
}
