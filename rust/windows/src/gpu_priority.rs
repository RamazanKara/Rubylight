//! WDDM scheduling applies to this process only; no system HAGS setting changes.

use anyhow::{Context, Result, bail};
use libloading::Library;
use windows::Win32::System::Threading::GetCurrentProcess;
use windows::{
    Win32::Graphics::Dxgi::{IDXGIDevice, IDXGIDevice1},
    core::Interface,
};
/// High GPU scheduling for the host's process. Process-wide realtime would also raise the encoder
/// and graphics queues, even when realtime compute queues are disabled.
const HIGH: i32 = 4;
struct ProcessPriority {
    previous: Option<i32>,
    configured: Option<i32>,
}
static PROCESS_PRIORITY: std::sync::Mutex<ProcessPriority> =
    std::sync::Mutex::new(ProcessPriority {
        previous: None,
        configured: None,
    });

pub(crate) fn resume() -> Result<()> {
    let mut state = PROCESS_PRIORITY.lock().unwrap();
    let Some(priority) = state.configured else {
        return Ok(());
    };
    let directory = std::env::var_os("SystemRoot").context("Windows directory missing")?;
    // SAFETY: The exports have the documented D3DKMT signatures, the output is live, and the
    // library outlives the calls.
    unsafe {
        let library = Library::new(std::path::PathBuf::from(directory).join("System32/gdi32.dll"))?;
        let set = library.get::<unsafe extern "system" fn(*mut std::ffi::c_void, i32) -> i32>(
            b"D3DKMTSetProcessSchedulingPriorityClass\0",
        )?;
        let mut original = 2;
        if state.previous.is_none()
            && let Ok(get) =
                library.get::<unsafe extern "system" fn(*mut std::ffi::c_void, *mut i32) -> i32>(
                    b"D3DKMTGetProcessSchedulingPriorityClass\0",
                )
        {
            let _ = get(GetCurrentProcess().0, &mut original);
        }
        let code = set(GetCurrentProcess().0, priority);
        if code < 0 {
            bail!("resuming GPU process scheduling priority was denied ({code:#x})");
        }
        state.previous.get_or_insert(original);
    }
    Ok(())
}

pub(crate) fn restore() -> Result<()> {
    let mut state = PROCESS_PRIORITY.lock().unwrap();
    let Some(priority) = state.previous else {
        return Ok(());
    };
    let directory = std::env::var_os("SystemRoot").context("Windows directory missing")?;
    // SAFETY: The export has the documented D3DKMT signature and the library outlives the call.
    unsafe {
        let library = Library::new(std::path::PathBuf::from(directory).join("System32/gdi32.dll"))?;
        let set = library.get::<unsafe extern "system" fn(*mut std::ffi::c_void, i32) -> i32>(
            b"D3DKMTSetProcessSchedulingPriorityClass\0",
        )?;
        let code = set(GetCurrentProcess().0, priority);
        if code < 0 {
            bail!("restoring GPU process scheduling priority was denied ({code:#x})");
        }
    }
    state.previous = None;
    Ok(())
}
pub fn configure(
    gpu: &crate::capture::Device,
    config: &butterpollo_core::config::Config,
) -> Result<()> {
    crate::compute::set_realtime(config.boolean("compute_queue_realtime", false));
    // SAFETY: the symbol types match the D3DKMT exports, every pointer passed is a live local, and
    // `library` outlives each symbol.
    unsafe {
        let device = gpu.device.cast::<IDXGIDevice>()?;
        // These device settings are independent of the privileged process
        // scheduling class. Preserve them even if that later request is denied.
        if let Err(error) = device.SetGPUThreadPriority(7) {
            tracing::debug!(%error, "capture GPU thread priority remains at its default");
        }
        if let Err(error) = gpu.device.cast::<IDXGIDevice1>()?.SetMaximumFrameLatency(1) {
            tracing::debug!(%error, "capture GPU frame latency remains at its default");
        }
        let directory = std::env::var_os("SystemRoot").context("Windows directory missing")?;
        let library = Library::new(std::path::PathBuf::from(directory).join("System32/gdi32.dll"))?;
        let set = library.get::<unsafe extern "system" fn(*mut std::ffi::c_void, i32) -> i32>(
            b"D3DKMTSetProcessSchedulingPriorityClass\0",
        )?;
        let mut state = PROCESS_PRIORITY.lock().unwrap();
        let mut original = 2;
        if state.previous.is_none()
            && let Ok(get) =
                library.get::<unsafe extern "system" fn(*mut std::ffi::c_void, *mut i32) -> i32>(
                    b"D3DKMTGetProcessSchedulingPriorityClass\0",
                )
        {
            let _ = get(GetCurrentProcess().0, &mut original);
        }
        let priority = HIGH;
        let code = set(GetCurrentProcess().0, priority);
        if code < 0 {
            bail!("GPU process scheduling priority was denied ({code:#x})");
        }
        state.previous.get_or_insert(original);
        state.configured = Some(priority);
        tracing::debug!(priority, "GPU process scheduling configured");
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn process_scheduling_is_high_never_realtime() {
        // Realtime (5) would raise the encoder and graphics queues process-wide.
        assert_eq!(super::HIGH, 4);
    }
}
