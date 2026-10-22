//! The one ADLX runtime of the process, shared by telemetry and the Radeon controls.
//!
//! `amdadlx64.dll` is loaded and `ADLXInitialize`d once and then kept for the life of the process.
//! `ADLXTerminate` is never called: cycling initialize/terminate faulted (access violation) within a
//! few hundred cycles on AMD Software 26.9.2, and two owners terminating each other's system
//! handle is the same bug waiting to happen. Every user takes the lock for the time it spends on
//! ADLX, so two threads never race a write or a read.
//!
//! In the host the Radeon settings reads and writes run in helper processes (`radeon::helper`),
//! not here, so telemetry's sampling never waits on settings IO; only the helper itself and the
//! hardware tests use the in-process `radeon::adlx` backend.
//!
//! Known limit: the system handle is kept across a driver reset or driver update. Telemetry used
//! to initialise and terminate its own handle for every new `Telemetry`, so a new stream got a
//! fresh one after a reset; now a stale handle makes samples fail (reported as missing metrics)
//! until the host restarts. Re-initialising is not an option (see above), so a refresh would
//! need the same helper-process treatment as the settings; it has not been measured whether a
//! reset really leaves this handle unusable.
use crate::telemetry::{ADLX_VERSION, Handle, system_library};
use anyhow::{Result, bail};
use std::{ptr, sync::Mutex};

struct Runtime {
    library: libloading::Library,
    system: Handle,
    /// What the first `ADLXInitialize` answered.
    initialize: i32,
}
// SAFETY: The system handle is only used while the mutex around it is held, and it is never
// terminated, so it stays valid for the life of the process.
unsafe impl Send for Runtime {}

static RUNTIME: Mutex<Option<Runtime>> = Mutex::new(None);

/// Exclusive use of the shared ADLX system. Drop it as soon as the ADLX calls are done.
pub(crate) struct Guard(std::sync::MutexGuard<'static, Option<Runtime>>);

impl Guard {
    fn runtime(&self) -> &Runtime {
        self.0.as_ref().expect("acquire() fills the runtime")
    }
    /// The `IADLXSystem`, which `ADLXTerminate` would own and nothing here ever ends.
    pub(crate) fn system(&self) -> Handle {
        self.runtime().system
    }
    pub(crate) fn library(&self) -> &libloading::Library {
        &self.runtime().library
    }
    /// The result of the process's one `ADLXInitialize` (0 ok, 1 already initialised, 2 other).
    pub(crate) fn initialize_result(&self) -> i32 {
        self.runtime().initialize
    }
}

/// Take the ADLX lock, loading and initialising the runtime on first use. A failure is not
/// remembered, so a driver that comes up later is picked up by the next call.
pub(crate) fn acquire() -> Result<Guard> {
    let mut slot = RUNTIME.lock().unwrap_or_else(|e| e.into_inner());
    if slot.is_none() {
        let library = system_library("amdadlx64.dll")?;
        // SAFETY: This export and its C signature are specified by AMD's ADLX.h.
        let initialize = unsafe {
            *library.get::<unsafe extern "C" fn(u64, *mut Handle) -> i32>(b"ADLXInitialize\0")?
        };
        let mut system = ptr::null_mut();
        // SAFETY: The runtime is loaded, the version is the baseline ABI the host negotiates and
        // `system` is writable.
        let result = unsafe { initialize(ADLX_VERSION, &mut system) };
        if !matches!(result, 0..=2) || system.is_null() {
            bail!("ADLX initialization failed ({result})");
        }
        *slot = Some(Runtime {
            library,
            system,
            initialize: result,
        });
    }
    Ok(Guard(slot))
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_runtime_is_never_terminated() {
        // Nothing may even look the export up: a lookup is the first step of calling it.
        let export = ["ADLX", "Terminate"].concat();
        for (name, source) in [
            ("telemetry", include_str!("telemetry.rs")),
            ("radeon", include_str!("radeon/adlx.rs")),
        ] {
            assert!(
                !source.contains(&format!("b\"{export}")),
                "{name} must not look up {export}"
            );
        }
    }
}
