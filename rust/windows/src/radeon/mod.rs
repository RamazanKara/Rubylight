//! Radeon Control Center support (plan slice A5).
//!
//! Step 1 is a read-only probe of the ADLX 3D settings on the installed driver: for each setting
//! it records whether ADLX reports it supported, readable and writable, with its value, range or
//! options and dependencies, and maps it to one of the five states of decision 5
//! (`docs/native-api.md`). Nothing here writes a setting.
//!
//! `model` is the pure mapping layer (unit-tested on fakes), `adlx` the FFI that fills it.
//! `cargo run --example radeon_probe` prints the result as JSON.
pub mod adlx;
pub mod control;
#[cfg(test)]
mod hardware;
pub mod helper;
pub mod model;
pub mod owner;
pub mod worker;

pub use adlx::{AdlxBackend, AdlxInfo, DriverInfo, GpuReport, Probe, probe};
pub use control::{
    BackendError, Change, Controller, Entry, ErrorKind, GpuInfo, Journal, RadeonBackend,
    RestoreReport, Scope, WriteError,
};
pub use model::{Kind, Setting, State, Writable};

use serde::Serialize;

#[cfg(test)]
mod tests {
    #[test]
    fn startup_recovery_with_no_journal_does_nothing_and_needs_no_adlx() {
        use super::{control::JOURNAL_FILE, worker::*};
        struct NoDriver;
        impl super::RadeonBackend for NoDriver {
            fn read(&mut self, _: i32) -> Result<Vec<super::Setting>, super::BackendError> {
                panic!("an empty journal needs no driver read")
            }
            fn write(
                &mut self,
                _: i32,
                _: &str,
                _: &super::model::Value,
            ) -> Result<(), super::BackendError> {
                panic!("an empty journal needs no driver write")
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let worker = Worker::start(NoDriver, dir.path().join(JOURNAL_FILE), Timing::default());
        let end = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while worker.status().phase == Phase::Recovering {
            assert!(std::time::Instant::now() < end, "the recovery never ended");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(worker.status().phase, Phase::Ready);
        assert!(!dir.path().join(JOURNAL_FILE).exists());
    }
}

/// The Windows release the probe ran on, from the registry.
#[derive(Clone, Debug, Default, Serialize)]
pub struct WindowsInfo {
    pub product: Option<String>,
    pub display_version: Option<String>,
    pub build: Option<String>,
    /// Update build revision, so `build.ubr` reads like `26200.9550`.
    pub ubr: Option<u32>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Report {
    pub windows: WindowsInfo,
    #[serde(flatten)]
    pub probe: Probe,
}

/// Probe ADLX and record the Windows build next to it.
pub fn report() -> anyhow::Result<Report> {
    Ok(Report {
        windows: windows_info(),
        probe: probe()?,
    })
}

fn windows_info() -> WindowsInfo {
    use windows::{
        Win32::System::Registry::{
            HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW,
        },
        core::{PCWSTR, w},
    };
    const KEY: PCWSTR = w!("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion");
    let text = |name: PCWSTR| -> Option<String> {
        let mut buffer = [0u16; 128];
        let mut size = std::mem::size_of_val(&buffer) as u32;
        // SAFETY: The key and value names are terminated and `size` matches the live buffer.
        unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                KEY,
                name,
                RRF_RT_REG_SZ,
                None,
                Some(buffer.as_mut_ptr().cast()),
                Some(&mut size),
            )
            .ok()
            .ok()?;
        }
        let length = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
        String::from_utf16(&buffer[..length]).ok()
    };
    let mut ubr = 0u32;
    let mut size = size_of::<u32>() as u32;
    // SAFETY: The names are terminated and the output is a live u32 of the declared size.
    let ubr = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            KEY,
            w!("UBR"),
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut ubr).cast()),
            Some(&mut size),
        )
        .ok()
        .ok()
        .map(|()| ubr)
    };
    WindowsInfo {
        product: text(w!("ProductName")),
        display_version: text(w!("DisplayVersion")),
        build: text(w!("CurrentBuildNumber")),
        ubr,
    }
}
