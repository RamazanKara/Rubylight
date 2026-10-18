//! WDDM scheduling applies to this process only; no system HAGS setting changes.

use anyhow::{Context, Result, bail};
use libloading::Library;
use windows::Win32::{Foundation::LUID, System::Threading::GetCurrentProcess};
use windows::{
    Win32::Graphics::Dxgi::{IDXGIDevice, IDXGIDevice1},
    core::Interface,
};
#[repr(C)]
struct Open {
    luid: LUID,
    adapter: u32,
}
#[repr(C)]
struct Query {
    adapter: u32,
    kind: u32,
    data: *mut std::ffi::c_void,
    size: u32,
}
#[repr(C)]
struct Close {
    adapter: u32,
}
fn priority(vendor: u32, hags: bool, realtime_hags: bool) -> i32 {
    // The NVIDIA option applies only to NVIDIA adapters. Others use high
    // priority: process-wide realtime also raises the encoder and graphics
    // queues, even when realtime compute queues are disabled.
    if vendor == 0x10de && (!hags || realtime_hags) {
        5
    } else {
        4
    }
}
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
    // SAFETY: the symbol types and repr(C) structs match the D3DKMT exports (the test checks their
    // layout), every pointer passed is a live local, and `library` outlives each symbol.
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
        let desc = device.GetAdapter()?.GetDesc()?;
        let directory = std::env::var_os("SystemRoot").context("Windows directory missing")?;
        let library = Library::new(std::path::PathBuf::from(directory).join("System32/gdi32.dll"))?;
        let open = library
            .get::<unsafe extern "system" fn(*mut Open) -> i32>(b"D3DKMTOpenAdapterFromLuid\0")?;
        let query = library
            .get::<unsafe extern "system" fn(*mut Query) -> i32>(b"D3DKMTQueryAdapterInfo\0")?;
        let close =
            library.get::<unsafe extern "system" fn(*mut Close) -> i32>(b"D3DKMTCloseAdapter\0")?;
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
        let mut adapter = Open {
            luid: desc.AdapterLuid,
            adapter: 0,
        };
        let mut caps = 0u32;
        let hags = if open(&mut adapter) >= 0 {
            let mut info = Query {
                adapter: adapter.adapter,
                kind: 70,
                data: (&mut caps as *mut u32).cast(),
                size: 4,
            };
            let result = query(&mut info);
            close(&mut Close {
                adapter: adapter.adapter,
            });
            result >= 0 && caps & 2 != 0
        } else {
            false
        };
        let priority = priority(
            desc.VendorId,
            hags,
            config.boolean("nvenc_realtime_hags", true),
        );
        let mut priority = priority;
        let mut code = set(GetCurrentProcess().0, priority);
        // Realtime needs administrator rights, which the capture helper in
        // the user's session lacks: high is the next best, not the default.
        if code < 0 && priority > 4 {
            priority = 4;
            code = set(GetCurrentProcess().0, priority);
        }
        if code < 0 {
            bail!("GPU process scheduling priority was denied ({code:#x})");
        }
        state.previous.get_or_insert(original);
        state.configured = Some(priority);
        tracing::debug!(hags, priority, "GPU process scheduling configured");
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn non_nvidia_never_inherits_the_nvidia_realtime_policy() {
        for vendor in [0x1002, 0x8086, 0] {
            for hags in [false, true] {
                for realtime_hags in [false, true] {
                    assert_eq!(priority(vendor, hags, realtime_hags), 4);
                }
            }
        }
    }
    #[test]
    fn nvidia_hags_policy_and_kernel_abi_are_preserved() {
        assert_eq!(priority(0x10de, true, false), 4);
        assert_eq!(priority(0x10de, true, true), 5);
        assert_eq!(priority(0x10de, false, false), 5);
        assert_eq!(priority(0x10de, false, true), 5);
        assert_eq!(size_of::<Open>(), 12);
        assert_eq!(size_of::<Query>(), 24);
        assert_eq!(std::mem::offset_of!(Query, data), 8);
    }
}
