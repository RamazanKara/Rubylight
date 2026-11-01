//! Rubylight's Windows service, ApolloService: it runs the host,
//! `butterpollo.exe`, as SYSTEM in the console session, starts it again when
//! it exits or the session changes, and stops it with the service. What
//! changes with the host from release to release (putting back what a Nucleus
//! Co-op session left, checking the virtual display driver) runs as short
//! `butterpollo.exe` tasks ([`tasks`]), so this executable stays the same
//! until its own code does.
mod logfile;
mod process;
mod state;
mod supervisor;
mod tasks;
// Rolling back an interrupted update stays here rather than in a host task:
// the host may be what the update left half done (a new butterpollo.exe
// beside DLLs it can't load, or an old one without the task). These are
// core's own files, so setup and the service read the same record;
// rust/release/service_inputs.py counts them among the service's files.
#[path = "../../core/src/update_files.rs"]
mod update_files;
#[path = "../../core/src/update_recovery.rs"]
mod update_recovery;

/// This executable's version, in its version information and its log. Bumped
/// only when the service changes (rust/release/README.md); the release
/// version is the host's.
pub const SERVICE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// `%ProgramData%\Butterpollo\config`, the installed host's profile, as
/// `butterpollo_core::paths::installed_profile`. Falls back to
/// `C:\ProgramData` when the variable is unset, as it is for some service
/// accounts.
pub fn installed_profile() -> std::path::PathBuf {
    std::path::PathBuf::from(
        std::env::var_os("PROGRAMDATA").unwrap_or_else(|| r"C:\ProgramData".into()),
    )
    .join("Butterpollo")
    .join("config")
}

/// The virtual display driver accepts only SYSTEM. Run as SYSTEM, for
/// example from a scheduled task, `--display-self-test REPORT` starts the
/// host's display self-test in the signed-in console session and fails when
/// a check fails; the report says which.
fn display_self_test(report: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        process::is_system(),
        "the display self-test must run as SYSTEM"
    );
    let host = std::env::current_exe()?.with_file_name("butterpollo.exe");
    let test = process::Process::spawn(
        &host,
        &["--display-self-test".into(), report.into()],
        host.parent(),
        process::console_session(),
    )?;
    let code = test.wait(std::time::Duration::from_secs(300))?;
    anyhow::ensure!(code == 0, "display self-test failed ({code}); see {report}");
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() == Some("--display-self-test") {
        let report = args
            .next()
            .ok_or_else(|| anyhow::anyhow!("report path required"))?;
        return display_self_test(&report);
    }
    let directory = installed_profile().join("logs");
    std::fs::create_dir_all(&directory)?;
    let (writer, _guard) = tracing_appender::non_blocking::NonBlockingBuilder::default()
        .buffered_lines_limit(8192)
        .lossy(true)
        .finish(logfile::RotatingFile::open_default(
            &directory.join("service.log"),
        )?);
    tracing_subscriber::fmt()
        .with_writer(writer)
        .with_ansi(false)
        .with_max_level(tracing::Level::INFO)
        .init();
    supervisor::run()
}
