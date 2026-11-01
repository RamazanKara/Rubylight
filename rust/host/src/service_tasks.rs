//! The Windows service's duties whose code changes with the host. The
//! service (rust/service) stays the same from release to release, so it runs
//! these as short `butterpollo.exe` tasks, as SYSTEM in its own session; each
//! logs to standard error, which the service copies into service.log, and
//! answers with its exit code (the contract in rust/service/src/tasks.rs):
//!
//! - `--restore-nucleus-at-console --config-dir PROFILE`: 0 nothing failed,
//!   1 a part failed.
//! - `--virtual-display-installed`: 0 the driver is there or switched off,
//!   1 it is missing (standard output says why).
use std::path::Path;

/// Log lines for the service: without a time (service.log adds its own) or
/// colour, at the level the service files them under.
fn log_to_stderr() {
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .without_time()
        .with_target(false)
        .with_max_level(tracing::Level::INFO)
        .try_init();
}

/// What a Nucleus Co-op session left for the user at the console, put back
/// ([`butterpollo_windows::service::restore_nucleus_layers`]).
pub fn restore_nucleus_exit_code(profile: &Path) -> i32 {
    log_to_stderr();
    if butterpollo_windows::service::restore_nucleus_layers(profile) {
        0
    } else {
        1
    }
}

/// Whether the virtual display driver counts as there for the service's
/// repair watch; when it is missing, standard output says why.
pub fn virtual_display_installed_exit_code() -> i32 {
    log_to_stderr();
    if butterpollo_windows::display::virtual_display_installed() {
        return 0;
    }
    let status = butterpollo_windows::display::virtual_display_status();
    println!("{}", status["reason"].as_str().unwrap_or(""));
    1
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    #[test]
    fn the_service_tasks_parse_as_the_service_starts_them() {
        let parse = |args: &[&str]| crate::Args::try_parse_from(args);
        let args = parse(&[
            "butterpollo",
            "--restore-nucleus-at-console",
            "--config-dir",
            r"C:\ProgramData\Butterpollo\config",
        ])
        .unwrap();
        assert!(args.restore_nucleus_at_console && !args.virtual_display_installed);
        assert!(
            parse(&["butterpollo", "--virtual-display-installed"])
                .unwrap()
                .virtual_display_installed
        );
        // The restore needs the profile.
        assert!(parse(&["butterpollo", "--restore-nucleus-at-console"]).is_err());
    }

    #[test]
    fn the_service_names_the_same_tasks_service_and_exit_codes() {
        // The service is a crate of its own, rust/service, built apart from
        // this workspace: what the two share is checked here.
        let tasks = include_str!("../../service/src/tasks.rs");
        for flag in [
            "\"--restore-nucleus-at-console\"",
            "\"--virtual-display-installed\"",
            "\"--config-dir\"",
        ] {
            assert!(tasks.contains(flag), "{flag}");
        }
        let process = include_str!("../../service/src/process.rs");
        assert!(process.contains("\"--service-stop-source\""));
        let supervisor = include_str!("../../service/src/supervisor.rs");
        assert!(supervisor.contains(&format!(
            "pub const NAME: &str = \"{}\";",
            butterpollo_windows::service::NAME
        )));
        assert!(supervisor.contains(&format!(
            "pub const RESTART_EXIT_CODE: u32 = {};",
            butterpollo_windows::service::RESTART_EXIT_CODE
        )));
        let main = include_str!("../../service/src/main.rs");
        assert!(main.contains("\"--display-self-test\""));
        assert!(main.contains("\"butterpollo.exe\""));
        // Update recovery stays in the service, compiled from core's file.
        assert!(main.contains("#[path = \"../../core/src/update_recovery.rs\"]"));
    }
}
