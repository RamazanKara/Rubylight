//! Start the staged test host as SYSTEM in the signed-in console session and wait for it. The virtual
//! display driver accepts only SYSTEM, and a plain scheduled task would run in session 0, where there
//! is no desktop to capture. The WebRTC end-to-end harness starts its test host through this, from a
//! one-shot `schtasks /RU SYSTEM` task (rust/tests/webrtc_client/host.cjs). rust/release/elevation.ps1
//! installs it in C:\ProgramData\ButterpolloRelease, where only administrators and SYSTEM can change it.
//!
//!   system_launch PROGRAM [ARGS...]
//!
//! PROGRAM must be the staged test host, C:\tests\webrtc-host\butterpollo.exe: it starts nothing
//! else. That folder is the user's, so the host and its DLLs there still run as SYSTEM whatever they
//! are. It exits with the program's own exit code. Run as anyone but SYSTEM, it refuses.
use anyhow::{Context, Result, ensure};
use butterpollo_windows::process::{Process, Target, console_session, is_system};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

/// The only program it starts.
const STAGED_HOST: &str = r"C:\tests\webrtc-host\butterpollo.exe";

/// Whether `program` is the staged test host, spelled as Windows would spell it (case aside):
/// another program, or the same one reached by another path, is refused.
fn allowed(program: &Path) -> bool {
    program.as_os_str().eq_ignore_ascii_case(STAGED_HOST)
}

fn main() -> Result<()> {
    ensure!(is_system(), "system_launch must run as SYSTEM");
    let mut args = std::env::args_os().skip(1);
    let program = PathBuf::from(
        args.next()
            .context("usage: system_launch PROGRAM [ARGS...]")?,
    );
    ensure!(
        allowed(&program),
        "system_launch starts only {STAGED_HOST}, not {}",
        program.display()
    );
    let args: Vec<_> = args.collect();
    let child = Process::spawn(
        &program,
        &args,
        program.parent(),
        Target::SystemSession(console_session()),
        &Default::default(),
        true,
    )?;
    // A day: the harness stops the host itself and ends the task.
    let code = child.wait(Duration::from_secs(24 * 3600))?;
    std::process::exit(code as i32);
}

#[cfg(test)]
mod tests {
    use super::allowed;
    use std::path::Path;

    #[test]
    fn starts_only_the_staged_host() {
        assert!(allowed(Path::new(r"C:\tests\webrtc-host\butterpollo.exe")));
        assert!(allowed(Path::new(r"C:\Tests\WebRTC-host\Butterpollo.EXE")));
        for other in [
            r"C:\ProgramData\ButterpolloRelease\butterpollo.exe",
            r"C:\Windows\System32\cmd.exe",
            r"C:\tests\webrtc-host\other.exe",
            r"C:\tests\other\butterpollo.exe",
            r"C:\tests\webrtc-host\..\other\butterpollo.exe",
            r"C:\tests\webrtc-host\sub\butterpollo.exe",
            r"butterpollo.exe",
            r"C:\tests\webrtc-host\butterpollo.exe.cmd",
        ] {
            assert!(!allowed(Path::new(other)), "{other}");
        }
    }
}
