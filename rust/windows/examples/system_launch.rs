//! Start the staged test host as SYSTEM in the signed-in console session and wait for it. The virtual
//! display driver accepts only SYSTEM, and a plain scheduled task would run in session 0, where there
//! is no desktop to capture. The WebRTC end-to-end harness starts its test host through this, from a
//! one-shot `schtasks /RU SYSTEM` task (rust/tests/webrtc_client/host.cjs).
//!
//!   system_launch PROGRAM [ARGS...]
//!
//! PROGRAM must be the `butterpollo.exe` beside this launcher: it starts nothing else. It exits with
//! the program's own exit code. Run as anyone but SYSTEM, it refuses.
use anyhow::{Context, Result, ensure};
use butterpollo_windows::process::{Process, Target, console_session, is_system};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

/// Whether `program` is the `butterpollo.exe` in the launcher's own directory, spelled as Windows
/// would spell it (case aside): another program, or the same one reached by another path, is refused.
fn allowed(launcher: &Path, program: &Path) -> bool {
    launcher.parent().is_some_and(|dir| {
        dir.join("butterpollo.exe")
            .as_os_str()
            .eq_ignore_ascii_case(program.as_os_str())
    })
}

fn main() -> Result<()> {
    ensure!(is_system(), "system_launch must run as SYSTEM");
    let mut args = std::env::args_os().skip(1);
    let program = PathBuf::from(
        args.next()
            .context("usage: system_launch PROGRAM [ARGS...]")?,
    );
    let launcher = std::env::current_exe().context("the launcher's own path")?;
    ensure!(
        allowed(&launcher, &program),
        "system_launch starts only {}, not {}",
        launcher.with_file_name("butterpollo.exe").display(),
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
    fn starts_only_the_host_beside_it() {
        let launcher = Path::new(r"C:\tests\webrtc-host\system_launch.exe");
        assert!(allowed(
            launcher,
            Path::new(r"C:\tests\webrtc-host\butterpollo.exe")
        ));
        assert!(allowed(
            launcher,
            Path::new(r"C:\Tests\WebRTC-host\Butterpollo.EXE")
        ));
        for other in [
            r"C:\Windows\System32\cmd.exe",
            r"C:\tests\webrtc-host\other.exe",
            r"C:\tests\other\butterpollo.exe",
            r"C:\tests\webrtc-host\..\other\butterpollo.exe",
            r"C:\tests\webrtc-host\sub\butterpollo.exe",
            r"butterpollo.exe",
            r"C:\tests\webrtc-host\butterpollo.exe.cmd",
        ] {
            assert!(!allowed(launcher, Path::new(other)), "{other}");
        }
    }
}
