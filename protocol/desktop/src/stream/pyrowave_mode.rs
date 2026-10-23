//! Which PyroWave decoder a stream gets and whether PyroWave is offered at all (brief G2).
//!
//! `cpu` is the Rust decoder with GPU wavelets in the presenter; `gpu` is the SDK's Vulkan decoder
//! writing planes the presenter draws. Each mode checks only its own requirements: `cpu` never
//! loads the SDK's DLL, and `gpu` never falls back to `cpu`, so a missing piece is reported with
//! its reason instead of being papered over.

use crate::config::PyroWaveDecoder;

/// `Ok` when PyroWave can run in `mode`, else why not. Only the chosen mode's check runs.
pub fn availability(
    mode: PyroWaveDecoder,
    cpu: impl FnOnce() -> Result<(), String>,
    gpu: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    match mode {
        PyroWaveDecoder::Cpu => cpu(),
        PyroWaveDecoder::Gpu => gpu(),
    }
}

/// The `gpu` mode's requirements on the renderer's `ID3D11Device`: the SDK decoder and its plane
/// import (`pyrowave_sdk::probe`) and the plane shader the presenter draws with.
#[cfg(windows)]
pub fn gpu_probe(device: *mut std::ffi::c_void, ten_bit: bool) -> Result<(), String> {
    use windows::core::Interface;
    // SAFETY: A live device pointer from the renderer, only borrowed for the probe.
    let device = unsafe { windows::Win32::Graphics::Direct3D11::ID3D11Device::from_raw_borrowed(&device) }
        .ok_or("PyroWave GPU decoding needs the Direct3D 11 renderer")?;
    super::pyrowave_sdk::probe(Some(device), ten_bit)?;
    super::d3d11::PlanesPass::compiles()
}

#[cfg(not(windows))]
pub fn gpu_probe(_device: *mut std::ffi::c_void, _ten_bit: bool) -> Result<(), String> {
    Err("PyroWave GPU decoding needs Windows".to_owned())
}

/// `gpu_probe` for settings screens, which have no renderer: on the default adapter, run once on a
/// thread of its own (it loads the DLL and builds a Vulkan device). None while it runs.
pub fn settings_probe() -> Option<Result<(), String>> {
    static STATE: std::sync::Mutex<Option<Option<Result<(), String>>>> = std::sync::Mutex::new(None);
    let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
    match state.as_ref() {
        Some(done) => done.clone(),
        None => {
            *state = Some(None);
            std::thread::spawn(|| {
                let result = default_probe();
                *STATE.lock().unwrap_or_else(|e| e.into_inner()) = Some(Some(result));
            });
            None
        }
    }
}

#[cfg(windows)]
fn default_probe() -> Result<(), String> {
    super::pyrowave_sdk::probe(None, false)?;
    super::d3d11::PlanesPass::compiles()
}

#[cfg(not(windows))]
fn default_probe() -> Result<(), String> {
    gpu_probe(std::ptr::null_mut(), false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_mode_checks_only_the_cpu_decoder() {
        let ok = availability(PyroWaveDecoder::Cpu, || Ok(()), || panic!("the GPU probe must not run in cpu mode"));
        assert_eq!(ok, Ok(()));
        let failed = availability(PyroWaveDecoder::Cpu, || Err("no wavelet shaders".into()), || panic!("not in cpu mode"));
        assert_eq!(failed, Err("no wavelet shaders".to_owned()));
    }

    #[test]
    fn gpu_mode_with_a_failing_probe_offers_no_pyrowave_and_does_not_fall_back() {
        let result = availability(PyroWaveDecoder::Gpu, || panic!("gpu mode never falls back to the CPU decoder"), || Err("no Vulkan device for this adapter".into()));
        assert_eq!(result, Err("no Vulkan device for this adapter".to_owned()));
        assert_eq!(availability(PyroWaveDecoder::Gpu, || panic!("not in gpu mode"), || Ok(())), Ok(()));
    }

    #[cfg(windows)]
    #[test]
    fn cpu_mode_never_loads_the_sdk_dll() {
        let loads = super::super::pyrowave_sdk::Api::shared_loads();
        let _ = availability(PyroWaveDecoder::Cpu, || Ok(()), || super::super::pyrowave_sdk::probe(None, false));
        assert_eq!(super::super::pyrowave_sdk::Api::shared_loads(), loads);
    }

    #[cfg(not(windows))]
    #[test]
    fn gpu_mode_is_unavailable_off_windows() {
        assert!(gpu_probe(std::ptr::null_mut(), false).is_err());
    }
}
