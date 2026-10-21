//! The floor this release runs on (`rust/release/floor.json`, butterpollo-core's `floor.rs`),
//! checked before setup changes anything. Below it setup refuses, names what it found and what
//! is required, and leaves an installed Rubylight as it is.
//!
//! The Windows build is read here. The GPU is judged by the package's own host
//! (`butterpollo.exe --floor-check`, which checks for RDNA 2 and asks AMF for the HEVC encoder), and the display
//! driver by the package's `install-local.ps1 -CheckOnly` (Smart App Control, S mode, App
//! Control): both run from a verified copy of the package in a folder only SYSTEM and
//! Administrators can write (never the user's %TEMP%), removed afterwards.
use crate::floor_rules::{Part, Pc, Unmet};
use crate::{log::line, payload::Payload, system};
use anyhow::{Context, Result, anyhow, bail};
use std::{path::Path, time::Duration};

/// The folder under the profile root the floor check unpacks the package into.
const FLOOR_STAGING: &str = "floor-check";

/// Where the floor check stages the package. `install::stage` locks it to SYSTEM and
/// Administrators; it is never in a folder a user process can write to.
fn staging_folder() -> std::path::PathBuf {
    system::program_data()
        .join("Butterpollo")
        .join(FLOOR_STAGING)
}

/// The last Rubylight for a PC below this floor.
pub const LAST_RELEASE_BELOW: &str = "2.6.x";

/// Refuse unless this PC meets the floor.
pub fn check(payload: &mut Payload) -> Result<()> {
    let missing = unmet(payload)?;
    if missing.is_empty() {
        line("floor: met");
        return Ok(());
    }
    for unmet in &missing {
        line(format!("floor: {:?}: {}", unmet.part, unmet.message));
    }
    Err(refusal(&missing))
}

/// The refusal for `missing`, as setup shows it.
pub fn refusal(missing: &[Unmet]) -> anyhow::Error {
    let reasons = missing
        .iter()
        .map(|unmet| format!("• {}", unmet.message))
        .collect::<Vec<_>>()
        .join("\n");
    anyhow!(
        "Rubylight {} cannot be installed on this PC:\n{reasons}\n\nNothing was changed. An installed Rubylight keeps working; {LAST_RELEASE_BELOW} is the last version for this PC.",
        env!("CARGO_PKG_VERSION")
    )
}

fn unmet(payload: &mut Payload) -> Result<Vec<Unmet>> {
    let floor = crate::floor_rules::this_release();
    if let Some(forced) = forced() {
        return Ok(vec![forced]);
    }
    let windows_build = system::registry_string(
        windows::Win32::System::Registry::HKEY_LOCAL_MACHINE,
        "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion",
        "CurrentBuildNumber",
    )
    .and_then(|build| build.trim().parse().ok());
    // The Windows build alone needs nothing from the package.
    let windows = crate::floor_rules::unmet(
        &floor,
        &Pc {
            windows_build,
            ..Pc::default()
        },
        false,
    );
    if let Some(windows) = windows.into_iter().find(|u| u.part == Part::Windows) {
        return Ok(vec![windows]);
    }
    // Setup is elevated and runs the staged host and script: stage them where
    // only SYSTEM and Administrators can write, never in the user's %TEMP%.
    let work = staging_folder();
    let result = (|| {
        crate::install::stage(payload, FLOOR_STAGING)
            .context("unpacking the package to check this PC")?;
        let mut missing = Vec::new();
        let mut pc = host_check(&work)?;
        pc.windows_build = windows_build;
        missing.extend(crate::floor_rules::unmet(&floor, &pc, false));
        if let Some(reason) = driver_check(&work)? {
            missing.push(Unmet {
                part: Part::DisplayDriver,
                message: format!("Windows refuses Rubylight's display driver here: {reason}"),
            });
        }
        Ok(missing)
    })();
    let _ = std::fs::remove_dir_all(&work);
    result
}

/// This PC as the package's host sees it.
fn host_check(package: &Path) -> Result<Pc> {
    let host = package.join("butterpollo.exe");
    let (code, output) = system::run(
        &host.display().to_string(),
        &["--floor-check"],
        Duration::from_secs(120),
    )
    .context("running the GPU check")?;
    if code != 0 {
        bail!(
            "the GPU check failed (exit code {code}): {}",
            output.trim().lines().last().unwrap_or("")
        );
    }
    let json = output
        .lines()
        .rev()
        .find(|line| line.trim_start().starts_with('{'))
        .context("the GPU check printed no result")?;
    line(format!("floor: host check {json}"));
    serde_json::from_str(json).context("reading the GPU check")
}

/// Why Windows would refuse the package's display driver, or `None`.
fn driver_check(package: &Path) -> Result<Option<String>> {
    let script = package.join(crate::install::RUBYLIGHT_DISPLAY_SCRIPT);
    let script = script.display().to_string();
    let (code, output) = system::run(
        &system::system32("WindowsPowerShell\\v1.0\\powershell.exe"),
        &[
            "-NoLogo",
            "-NonInteractive",
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
            &script,
            "-CheckOnly",
        ],
        Duration::from_secs(120),
    )
    .context("running the display driver check")?;
    driver_verdict(code, &output)
}

fn driver_verdict(code: i32, output: &str) -> Result<Option<String>> {
    if let Some(reason) = output
        .lines()
        .find_map(|line| line.split_once("DRIVER_UNSUPPORTED: "))
        .map(|(_, reason)| reason.trim().to_owned())
    {
        return Ok(Some(reason));
    }
    if code == 0 && output.lines().any(|line| line.trim() == "DRIVER_SUPPORTED") {
        return Ok(None);
    }
    bail!(
        "the display driver check failed (exit code {code}): {}",
        output.trim()
    )
}

/// A verdict forced for testing (`RUBYLIGHT_SETUP_FLOOR_TEST` = `windows`, `gpu` or `driver`),
/// in setup builds with the `floor-test` feature only.
#[cfg(feature = "floor-test")]
fn forced() -> Option<Unmet> {
    let part = match std::env::var("RUBYLIGHT_SETUP_FLOOR_TEST").ok()?.as_str() {
        "windows" => Part::Windows,
        "gpu" => Part::Gpu,
        "driver" => Part::DisplayDriver,
        _ => return None,
    };
    line(format!("floor: verdict forced for testing: {part:?}"));
    Some(Unmet {
        part,
        message: format!("{part:?} below the floor (forced for testing)"),
    })
}

#[cfg(not(feature = "floor-test"))]
fn forced() -> Option<Unmet> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_floor_check_stages_the_package_where_users_cannot_write() {
        let staging = staging_folder();
        assert!(staging.starts_with(system::program_data()));
        assert!(!staging.starts_with(std::env::temp_dir()));
        // The same locked-down staging the install uses, not a bare create_dir_all.
        let source = include_str!("floor.rs");
        let body = &source[source.find("fn unmet(").unwrap()..source.find("fn host_check").unwrap()];
        assert!(body.contains("crate::install::stage("));
        assert!(!body.contains("temp_dir"));
    }

    #[test]
    fn the_driver_check_reads_the_scripts_verdict_and_fails_loudly_otherwise() {
        assert_eq!(driver_verdict(0, "DRIVER_SUPPORTED\n").unwrap(), None);
        assert_eq!(
            driver_verdict(0, "DRIVER_UNSUPPORTED: Smart App Control is on.\n").unwrap(),
            Some("Smart App Control is on.".into())
        );
        assert!(driver_verdict(1, "Get-AuthenticodeSignature failed\n").is_err());
        assert!(driver_verdict(0, "").is_err());
    }

    #[test]
    fn the_refusal_names_each_reason_and_the_last_release() {
        let error = refusal(&[
            Unmet {
                part: Part::Windows,
                message:
                    "Rubylight needs Windows 11 build 26200 or newer; this PC runs build 22631."
                        .into(),
            },
            Unmet {
                part: Part::Gpu,
                message: "Rubylight needs an AMD Radeon GPU of the RX 6000 series or newer.".into(),
            },
        ])
        .to_string();
        assert!(error.contains("build 22631"));
        assert!(error.contains("RX 6000"));
        assert!(error.contains("Nothing was changed"));
        assert!(error.contains(LAST_RELEASE_BELOW));
    }

    #[cfg(not(feature = "floor-test"))]
    #[test]
    fn release_builds_cannot_force_a_verdict() {
        // SAFETY: test-only; no other thread reads the environment here.
        unsafe { std::env::set_var("RUBYLIGHT_SETUP_FLOOR_TEST", "gpu") };
        assert!(forced().is_none());
        // SAFETY: as above.
        unsafe { std::env::remove_var("RUBYLIGHT_SETUP_FLOOR_TEST") };
    }
}
