//! Quick Suspend's crash recovery in the host binary: the watchdog helper's
//! entry point (`--quick-suspend-watch HOST_PID --config-dir PROFILE`) and the
//! recovery a host runs as it starts, before it serves a stream
//! ([`butterpollo_win_io::process::suspend_watch`]).
use butterpollo_session::quick_suspend::Recovery;
use butterpollo_win_io::process::suspend_watch;
use std::path::Path;

/// Log what a recovery did, for the host that wrote the journal (`title`).
fn report(title: &str, recovery: &Recovery) {
    let game = if title.is_empty() { "a game" } else { title };
    if !recovery.resumed.is_empty() {
        tracing::warn!(
            processes = recovery.resumed.join(", "),
            "quick_suspend_recovered: Rubylight stopped while {game} was suspended; the game was resumed"
        );
    }
    for (record, error) in &recovery.failed {
        tracing::error!(
            pid = record.pid,
            program = record.name,
            error,
            "a process Quick Suspend paused could not be resumed; it is tried again at the next start"
        );
    }
}

/// As a host starts: resume what a host that stopped mid-suspend left frozen.
pub fn at_start(profile: &Path) {
    match suspend_watch::recover_at_start(profile) {
        Ok(Some((journal, recovery))) => report(&journal.app_title, &recovery),
        Ok(None) => {}
        Err(error) => tracing::error!(
            error = format!("{error:#}"),
            "the Quick Suspend journal could not be recovered"
        ),
    }
}

/// The watchdog helper: exits 0 when the host resumed everything itself or
/// the helper did, 1 when something stayed frozen or failed. It logs to
/// `logs/quick-suspend-watch.log` in the profile, as no host log is open here.
pub fn watch_exit_code(host_pid: u32, profile: &Path) -> i32 {
    let logs = profile.join("logs");
    if std::fs::create_dir_all(&logs).is_ok()
        && let Ok(log) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(logs.join("quick-suspend-watch.log"))
    {
        let _ = tracing_subscriber::fmt()
            .with_writer(std::sync::Mutex::new(log))
            .with_ansi(false)
            .with_max_level(tracing::Level::INFO)
            .try_init();
    }
    match suspend_watch::wait_and_recover(host_pid, profile) {
        Ok(None) => 0,
        Ok(Some(recovery)) => {
            report("", &recovery);
            i32::from(!recovery.complete())
        }
        Err(error) => {
            tracing::error!(
                error = format!("{error:#}"),
                "the Quick Suspend watchdog failed; the next host start resumes the game"
            );
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    #[test]
    fn the_watchdog_parses_as_the_host_starts_it() {
        let parse = |args: &[&str]| crate::cli::Args::try_parse_from(args);
        let args = parse(&[
            "butterpollo",
            "--quick-suspend-watch",
            "4100",
            "--config-dir",
            r"C:\ProgramData\Butterpollo\config",
        ])
        .unwrap();
        assert_eq!(args.quick_suspend_watch, Some(4100));
        // It needs the profile whose journal it watches.
        assert!(parse(&["butterpollo", "--quick-suspend-watch", "4100"]).is_err());
        // The helper the host starts passes these exact arguments.
        let watch = include_str!("../../windows/io/src/process/suspend_watch.rs");
        assert!(watch.contains(".arg(\"--quick-suspend-watch\")"));
        assert!(watch.contains(".arg(\"--config-dir\")"));
    }

    #[test]
    fn a_host_recovers_before_it_serves_a_stream() {
        let server = include_str!("server.rs");
        let recovery = server.find("suspend_recovery::at_start(").unwrap();
        let listener = server.find("let guest_listener").unwrap();
        assert!(recovery < listener);
    }
}
