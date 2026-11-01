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
/// High GPU scheduling for the host's process. Process-wide realtime would also raise the encoder
/// and graphics queues, even when realtime compute queues are disabled.
const HIGH: i32 = 4;
/// Realtime GPU scheduling, for NVENC on an NVIDIA adapter.
const REALTIME: i32 = 5;
/// `KMTQAITYPE_WDDM_2_7_CAPS`, whose bit 1 says hardware-accelerated GPU
/// scheduling is on.
const WDDM_2_7_CAPS: u32 = 70;
/// The process's GPU scheduling class. An NVIDIA adapter runs at realtime,
/// Sunshine's NVENC policy that Rubylight carried before 3.0, unless
/// `nvenc_realtime_hags` is off while hardware-accelerated GPU scheduling is
/// on: at high, NVENC falls behind a game that fills the GPU. Every other
/// adapter, AMD included, stays at high.
fn priority(vendor: u32, hags: bool, realtime_hags: bool) -> i32 {
    if butterpollo_core::encoder_gpu::is_nvidia(vendor) && (!hags || realtime_hags) {
        REALTIME
    } else {
        HIGH
    }
}
/// Whether hardware-accelerated GPU scheduling is on for the adapter `luid`.
///
/// # Safety
/// The symbols are gdi32's D3DKMT exports with these signatures.
unsafe fn hags(
    luid: LUID,
    open: unsafe extern "system" fn(*mut Open) -> i32,
    query: unsafe extern "system" fn(*mut Query) -> i32,
    close: unsafe extern "system" fn(*mut Close) -> i32,
) -> bool {
    let mut adapter = Open { luid, adapter: 0 };
    // SAFETY: Per the contract; every pointer is a live local, and the adapter handle is closed
    // once after the query.
    unsafe {
        if open(&mut adapter) < 0 {
            return false;
        }
        let mut caps = 0u32;
        let mut info = Query {
            adapter: adapter.adapter,
            kind: WDDM_2_7_CAPS,
            data: (&mut caps as *mut u32).cast(),
            size: 4,
        };
        let result = query(&mut info);
        close(&mut Close {
            adapter: adapter.adapter,
        });
        result >= 0 && caps & 2 != 0
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
        let directory = std::env::var_os("SystemRoot").context("Windows directory missing")?;
        let library = Library::new(std::path::PathBuf::from(directory).join("System32/gdi32.dll"))?;
        let set = library.get::<unsafe extern "system" fn(*mut std::ffi::c_void, i32) -> i32>(
            b"D3DKMTSetProcessSchedulingPriorityClass\0",
        )?;
        let hags = if butterpollo_core::encoder_gpu::is_nvidia(gpu.vendor) {
            let open = library.get::<unsafe extern "system" fn(*mut Open) -> i32>(
                b"D3DKMTOpenAdapterFromLuid\0",
            )?;
            let query = library
                .get::<unsafe extern "system" fn(*mut Query) -> i32>(b"D3DKMTQueryAdapterInfo\0")?;
            let close = library
                .get::<unsafe extern "system" fn(*mut Close) -> i32>(b"D3DKMTCloseAdapter\0")?;
            hags(
                device.GetAdapter()?.GetDesc()?.AdapterLuid,
                *open,
                *query,
                *close,
            )
        } else {
            false
        };
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
        let mut priority = priority(
            gpu.vendor,
            hags,
            config.boolean("nvenc_realtime_hags", true),
        );
        let mut code = set(GetCurrentProcess().0, priority);
        // Realtime needs administrator rights, which the capture helper in
        // the user's session lacks: high is the next best, not the default.
        if code < 0 && priority > HIGH {
            priority = HIGH;
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
    fn amd_and_other_adapters_stay_high_never_realtime() {
        // Realtime (5) would raise AMF's and the game's queues process-wide.
        assert_eq!(HIGH, 4);
        for vendor in [0x1002, 0x8086, 0] {
            for hags in [false, true] {
                for realtime_hags in [false, true] {
                    assert_eq!(priority(vendor, hags, realtime_hags), HIGH);
                }
            }
        }
    }
    #[test]
    fn nvidia_runs_realtime_unless_turned_off_under_hags() {
        assert_eq!(REALTIME, 5);
        assert_eq!(priority(0x10de, false, true), REALTIME);
        assert_eq!(priority(0x10de, false, false), REALTIME);
        assert_eq!(priority(0x10de, true, true), REALTIME);
        assert_eq!(priority(0x10de, true, false), HIGH);
    }
    #[test]
    fn d3dkmt_structs_match_the_kernel_layout() {
        assert_eq!(std::mem::size_of::<Open>(), 12);
        assert_eq!(std::mem::size_of::<Query>(), 24);
        assert_eq!(std::mem::offset_of!(Query, data), 8);
        assert_eq!(std::mem::offset_of!(Query, size), 16);
        assert_eq!(std::mem::size_of::<Close>(), 4);
    }
}
