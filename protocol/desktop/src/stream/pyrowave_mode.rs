//! Which PyroWave decoder a stream gets and whether PyroWave is offered at all (brief G2).
//!
//! `cpu` is the Rust decoder with GPU wavelets in the presenter; `gpu` is the SDK's Vulkan decoder
//! writing planes the presenter draws. Each mode checks only its own requirements: `cpu` never
//! loads the SDK's library, and `gpu` never falls back to `cpu`, so a missing piece is reported with
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

/// The `gpu` mode's requirements on the renderer's `SDL_GPUDevice`: the SDK's own Vulkan device
/// and decoder (`pyrowave_sdk::probe`) and the plane shader the presenter draws with.
#[cfg(unix)]
pub fn gpu_probe(device: *mut std::ffi::c_void, ten_bit: bool) -> Result<(), String> {
    if device.is_null() {
        return Err("PyroWave GPU decoding needs the SDL GPU renderer".to_owned());
    }
    super::pyrowave_sdk::probe(ten_bit)?;
    // SAFETY: A live device pointer from the renderer; shader creation is thread-safe in SDL.
    unsafe { super::planes_gpu::PlanesPass::compiles(device.cast(), ten_bit) }
}

#[cfg(not(any(windows, unix)))]
pub fn gpu_probe(_device: *mut std::ffi::c_void, _ten_bit: bool) -> Result<(), String> {
    Err("PyroWave GPU decoding needs Windows or Linux".to_owned())
}

type ProbeState = std::sync::Mutex<Vec<(bool, Option<Result<(), String>>)>>;

fn settings_probe_state() -> &'static ProbeState {
    static STATE: ProbeState = std::sync::Mutex::new(Vec::new());
    &STATE
}

/// `gpu_probe` for settings screens, on the renderer's device (so its adapter's LUID) and the plane
/// format the stream would use (`ten_bit` is the HDR setting). It loads the DLL and builds a Vulkan
/// device, so it runs once per format on a thread of its own and the result is remembered. None
/// while it runs. `device` is the renderer's, which outlives the process's settings screens.
pub fn settings_probe(device: *mut std::ffi::c_void, ten_bit: bool) -> Option<Result<(), String>> {
    struct Raw(*mut std::ffi::c_void);
    // SAFETY: D3D11 devices are free-threaded and the renderer outlives the probe thread's use.
    unsafe impl Send for Raw {}
    let mut state = settings_probe_state().lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, done)) = state.iter().find(|(key, _)| *key == ten_bit) {
        return done.clone();
    }
    state.push((ten_bit, None));
    let device = Raw(device);
    std::thread::spawn(move || {
        let device = device;
        let result = gpu_probe(device.0, ten_bit);
        let mut state = settings_probe_state().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = state.iter_mut().find(|(key, _)| *key == ten_bit) { entry.1 = Some(result); }
    });
    None
}

/// Whether `settings_probe` has been asked for `ten_bit` yet (tests: settings views that do not
/// need the probe must not start it).
#[cfg(test)]
pub fn settings_probe_started(ten_bit: bool) -> bool {
    settings_probe_state().lock().unwrap_or_else(|e| e.into_inner()).iter().any(|(key, _)| *key == ten_bit)
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

    #[test]
    fn the_settings_probe_is_remembered_and_fails_loudly_without_a_renderer_device() {
        // A null device is "no Direct3D 11 renderer": the answer is that reason, never the default
        // adapter's. (The settings screen tests own the 10-bit key.)
        let done = (0..500).find_map(|_| {
            settings_probe(std::ptr::null_mut(), false).or_else(|| {
                std::thread::sleep(std::time::Duration::from_millis(10));
                None
            })
        });
        let done = done.expect("the settings probe never finished");
        assert!(done.clone().is_err());
        if cfg!(windows) {
            assert!(done.clone().unwrap_err().contains("Direct3D 11 renderer"));
        } else if cfg!(unix) {
            assert!(done.clone().unwrap_err().contains("SDL GPU renderer"));
        }
        assert!(settings_probe_started(false));
        assert_eq!(settings_probe(std::ptr::null_mut(), false), Some(done), "remembered");
    }

    #[cfg(windows)]
    #[test]
    fn cpu_mode_never_loads_the_sdk_dll() {
        let loads = super::super::pyrowave_sdk::Api::shared_loads();
        let _ = availability(PyroWaveDecoder::Cpu, || Ok(()), || super::super::pyrowave_sdk::probe(None, false));
        assert_eq!(super::super::pyrowave_sdk::Api::shared_loads(), loads);
    }

    #[cfg(unix)]
    #[test]
    fn cpu_mode_never_loads_the_sdk_library() {
        let loads = super::super::pyrowave_sdk::Api::shared_loads();
        let _ = availability(PyroWaveDecoder::Cpu, || Ok(()), || super::super::pyrowave_sdk::probe(false));
        assert_eq!(super::super::pyrowave_sdk::Api::shared_loads(), loads);
    }

    #[cfg(unix)]
    #[test]
    fn the_gpu_probe_without_a_renderer_device_says_so_before_loading_the_sdk() {
        let loads = super::super::pyrowave_sdk::Api::shared_loads();
        let error = gpu_probe(std::ptr::null_mut(), false).unwrap_err();
        assert!(error.contains("SDL GPU renderer"), "{error}");
        assert_eq!(super::super::pyrowave_sdk::Api::shared_loads(), loads);
    }
}
