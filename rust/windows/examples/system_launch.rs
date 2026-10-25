//! Start a program as SYSTEM in the signed-in console session and wait for it. The virtual display
//! driver accepts only SYSTEM, and a plain scheduled task would run in session 0, where there is
//! no desktop to capture. The WebRTC end-to-end harness starts its test host through this, from a
//! one-shot `schtasks /RU SYSTEM` task (rust/tests/webrtc_client/host.cjs).
//!
//!   system_launch PROGRAM [ARGS...]
//!
//! It exits with the program's own exit code. Run as anyone but SYSTEM, it refuses.
use anyhow::{Context, Result, ensure};
use butterpollo_windows::process::{Process, Target, console_session, is_system};
use std::{path::PathBuf, time::Duration};

fn main() -> Result<()> {
    ensure!(is_system(), "system_launch must run as SYSTEM");
    let mut args = std::env::args_os().skip(1);
    let program = PathBuf::from(
        args.next()
            .context("usage: system_launch PROGRAM [ARGS...]")?,
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
