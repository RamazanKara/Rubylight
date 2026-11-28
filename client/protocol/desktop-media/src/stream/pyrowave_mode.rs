//! Which PyroWave decoder a stream gets and whether PyroWave is offered at all (brief G2).
//!
//! `cpu` is the Rust decoder with GPU wavelets in the presenter; `gpu` is the SDK's Vulkan decoder
//! writing planes the presenter draws. Each mode checks only its own requirements: `cpu` never
//! loads the SDK's library, and on Windows and Linux `gpu` never falls back to `cpu`, so a missing
//! piece is reported with its reason instead of being papered over.
//!
//! On macOS `gpu` decodes through the Vulkan loader and Mesa's KosmicKrisp driver bundled in
//! Nightjar.app, which run on Apple silicon with macOS 26 or later only; elsewhere, or when the
//! driver or the decoder does not start, the stream decodes on the processor and says why
//! (`mac_choice`). The Mac's default is the processor (`config::PyroWaveDecoder`); `gpu` is a
//! choice until a Mac measures it.

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

/// The Vulkan loader in `Contents/Frameworks` and KosmicKrisp's manifest in
/// `Contents/Resources/vulkan/icd.d`, as LunarG lays a Mac app bundle out.
pub const LOADER_NAME: &str = "libvulkan.1.dylib";
pub const ICD_NAME: &str = "kosmickrisp_icd.json";

/// The loader and manifest of the bundle whose executable is `exe` (`Contents/MacOS/nightjar`).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn bundle_paths(exe: &std::path::Path) -> Option<(std::path::PathBuf, std::path::PathBuf)> {
    let contents = exe.parent()?.parent()?;
    Some((contents.join("Frameworks").join(LOADER_NAME), contents.join("Resources/vulkan/icd.d").join(ICD_NAME)))
}

/// The first macOS whose Metal 4 KosmicKrisp needs.
pub const KOSMICKRISP_MACOS: u32 = 26;

/// What decodes PyroWave on a Mac.
#[derive(Debug, PartialEq, Eq)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub enum MacChoice {
    /// The SDK's decoder on the bundled Vulkan driver.
    Gpu,
    /// The Rust decoder; with the reason when the setting asked for the GPU.
    Cpu(Option<String>),
}

/// The decoder for a Mac with `arm64` (Apple silicon) and macOS `major`: the GPU decoder where the
/// setting asks for it and `probe` (the bundled driver and the decoder starting) passes, else the
/// processor, with the reason the GPU decoder does not run.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn mac_choice(setting: PyroWaveDecoder, arm64: bool, major: u32, probe: impl FnOnce() -> Result<(), String>) -> MacChoice {
    match setting {
        PyroWaveDecoder::Cpu => MacChoice::Cpu(None),
        PyroWaveDecoder::Gpu if !arm64 => MacChoice::Cpu(Some("GPU decoding needs a Mac with Apple silicon".to_owned())),
        PyroWaveDecoder::Gpu if major < KOSMICKRISP_MACOS => {
            MacChoice::Cpu(Some(format!("GPU decoding needs macOS {KOSMICKRISP_MACOS} or later, this Mac runs macOS {major}")))
        }
        PyroWaveDecoder::Gpu => match probe() {
            Ok(()) => MacChoice::Gpu,
            Err(reason) => MacChoice::Cpu(Some(reason)),
        },
    }
}

/// This Mac's macOS major version (`kern.osproductversion`), 0 when unknown.
#[cfg(target_os = "macos")]
pub fn macos_major() -> u32 {
    let mut buffer = [0u8; 32];
    let mut size = buffer.len();
    // SAFETY: sysctlbyname writes at most `size` bytes into `buffer` and the length into `size`.
    let result = unsafe { libc::sysctlbyname(c"kern.osproductversion".as_ptr(), buffer.as_mut_ptr().cast(), &mut size, std::ptr::null_mut(), 0) };
    if result != 0 {
        return 0;
    }
    let text = std::str::from_utf8(&buffer[..size.min(buffer.len())]).unwrap_or("").trim_end_matches('\0');
    text.split('.').next().and_then(|major| major.parse().ok()).unwrap_or(0)
}

/// `mac_choice` for this Mac and `setting`, the probe being `gpu_probe` on the renderer's `device`:
/// a plane shader that does not build is then the processor's reason, shown as the stream's start
/// notice, where a probe without the device found out only once the stream asked for a picture.
#[cfg(target_os = "macos")]
pub fn this_mac(setting: PyroWaveDecoder, ten_bit: bool, device: *mut std::ffi::c_void) -> MacChoice {
    mac_choice(setting, cfg!(target_arch = "aarch64"), macos_major(), || gpu_probe(device, ten_bit))
}

/// `nightjar --check-pyrowave <report file>`: what a package check needs to know without a window
/// (the Windows exe has no console). It writes `load:` (the SDK library next to the executable
/// was found, loaded and has a supported ABI) and `probe:` (a decoder builds on the default GPU)
/// lines to `report` and returns the exit code: 0 when both pass, 2 when only the probe failed
/// (no usable GPU, as on a CI runner), 1 when the library did not load.
pub fn package_check(report: &std::path::Path) -> i32 {
    let dir = super::pyrowave_sdk::dll_path().and_then(|dll| dll.parent().map(std::path::Path::to_path_buf)).unwrap_or_default();
    #[cfg(target_os = "macos")]
    {
        let arm64 = cfg!(target_arch = "aarch64");
        let major = macos_major();
        eprintln!("nightjar: macOS {major}, {}", if arm64 { "Apple silicon" } else { "Intel" });
    }
    package_check_at(&dir, report)
}

/// `package_check` looking for the library in `dir` (the executable's folder in the real check;
/// the probe, which only runs once the library loaded, always uses the executable's folder).
fn package_check_at(dir: &std::path::Path, report: &std::path::Path) -> i32 {
    let load = super::pyrowave_sdk::Api::load_from(&dir.join(super::pyrowave_sdk::DLL_NAME)).map(drop);
    let probe = match &load {
        Ok(()) => {
            #[cfg(windows)]
            let result = super::pyrowave_sdk::probe(None, false);
            #[cfg(target_os = "linux")]
            let result = super::pyrowave_sdk::probe(false);
            #[cfg(target_os = "macos")]
            let result = match mac_choice(PyroWaveDecoder::Gpu, cfg!(target_arch = "aarch64"), macos_major(), || super::pyrowave_sdk::probe(false)) {
                MacChoice::Gpu => Ok(()),
                MacChoice::Cpu(reason) => Err(format!("the processor decodes: {}", reason.unwrap_or_default())),
            };
            result
        }
        Err(_) => Err("skipped: the library did not load".to_owned()),
    };
    let line = |name: &str, result: &Result<(), String>| format!("{name}: {}\n", result.as_ref().map_or_else(|e| e.replace('\n', " "), |()| "ok".to_owned()));
    let text = format!("{}{}", line("load", &load), line("probe", &probe));
    if let Err(e) = std::fs::write(report, &text) {
        eprintln!("nightjar: cannot write {}: {e}", report.display());
    }
    eprint!("{text}");
    match (load, probe) {
        (Ok(()), Ok(())) => 0,
        (Ok(()), Err(_)) => 2,
        (Err(_), _) => 1,
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
#[cfg(target_os = "linux")]
pub fn gpu_probe(device: *mut std::ffi::c_void, ten_bit: bool) -> Result<(), String> {
    if device.is_null() {
        return Err("PyroWave GPU decoding needs the SDL GPU renderer".to_owned());
    }
    super::pyrowave_sdk::probe(ten_bit)?;
    // SAFETY: A live device pointer from the renderer; shader creation is thread-safe in SDL.
    unsafe { super::planes_gpu::PlanesPass::compiles(device.cast(), ten_bit) }
}

/// Settings probe on Linux when `Renderer::Auto` is active on an OpenGL launcher: probes
/// `pyrowave_sdk::probe` directly so PyroWave stays visible in Settings → Video codec, deferring
/// `PlanesPass::compiles` until `switch_for_stream` creates the SDL GPU renderer.
#[cfg(target_os = "linux")]
fn gpu_probe_deferred(device: *mut std::ffi::c_void, ten_bit: bool) -> Result<(), String> {
    if device.is_null() {
        return super::pyrowave_sdk::probe(ten_bit);
    }
    gpu_probe(device, ten_bit)
}

/// The `gpu` mode's requirements on a Mac: the bundled Vulkan driver, a device of the decoder's own
/// on it and the SDK decoder there (`pyrowave_sdk::probe`); the planes the decoder reads back are
/// drawn with the Metal plane shader the processor's software pictures use already, which is built
/// here whenever the caller has the renderer's device.
#[cfg(target_os = "macos")]
pub fn gpu_probe(device: *mut std::ffi::c_void, ten_bit: bool) -> Result<(), String> {
    super::pyrowave_sdk::probe(ten_bit)?;
    if device.is_null() {
        // `this_mac` checks before a renderer exists; the launch's own probe passes the device.
        return Ok(());
    }
    // SAFETY: A live device pointer from the renderer; shader creation is thread-safe in SDL.
    unsafe { super::planes_gpu::PlanesPass::compiles(device.cast(), ten_bit) }
}

type ProbeEntry = ((bool, bool), Option<Result<(), String>>);

/// The settings probes' results by `(ten_bit, deferred_gpu)`, None while one runs, and the
/// threads that run them.
#[derive(Default)]
struct Probes {
    results: Vec<ProbeEntry>,
    running: Vec<std::thread::JoinHandle<()>>,
}

type ProbeState = std::sync::Mutex<Probes>;

fn settings_probe_state() -> &'static ProbeState {
    static STATE: ProbeState = std::sync::Mutex::new(Probes { results: Vec::new(), running: Vec::new() });
    &STATE
}

/// Whether a settings probe on a null device should defer `PlanesPass::compiles` until
/// `switch_for_stream` switches Linux's OpenGL launcher to the SDL GPU renderer.
pub fn defer_gpu_probe(device: *mut std::ffi::c_void, auto_renderer: bool) -> bool {
    cfg!(target_os = "linux") && auto_renderer && device.is_null()
}

/// `gpu_probe` for settings screens, on the renderer's device (so its adapter's LUID) and the plane
/// format the stream would use (`ten_bit` is the HDR setting). It loads the DLL and builds a Vulkan
/// device, so it runs once per format on a thread of its own and the result is remembered. None
/// while it runs. `device` is the renderer's, which outlives the probe: a new renderer (Linux) waits
/// for it first ([`settings_probe_reset`]).
pub fn settings_probe(device: *mut std::ffi::c_void, ten_bit: bool) -> Option<Result<(), String>> {
    settings_probe_with_auto(device, ten_bit, false)
}

/// [`settings_probe`] with `auto_renderer`: when `true` on Linux and `device` is null (OpenGL
/// launcher with `Renderer::Auto`), probes `pyrowave_sdk::probe(ten_bit)` directly and defers the
/// plane-shader compile check until `switch_for_stream` creates the GPU renderer.
pub fn settings_probe_with_auto(
    device: *mut std::ffi::c_void,
    ten_bit: bool,
    auto_renderer: bool,
) -> Option<Result<(), String>> {
    let deferred = defer_gpu_probe(device, auto_renderer);
    #[cfg(target_os = "linux")]
    let probe: fn(*mut std::ffi::c_void, bool) -> Result<(), String> =
        if deferred { gpu_probe_deferred } else { gpu_probe };
    #[cfg(not(target_os = "linux"))]
    let probe: fn(*mut std::ffi::c_void, bool) -> Result<(), String> = gpu_probe;
    // Hold a reference on the D3D11 device for the probe thread's lifetime on Windows so a
    // driver reset or renderer remake cannot free it underneath the probe.
    let hold = crate::media::DeviceRef::of_device(device);
    probe_in(settings_probe_state(), device, hold, (ten_bit, deferred), probe)
}

/// Before the renderer goes (Linux's switch to the GPU renderer or back): waits for a probe still
/// on its device, and forgets what the probes found on it, so the new device is probed again.
pub fn settings_probe_reset() {
    reset_in(settings_probe_state());
}

fn probe_in(
    state: &'static ProbeState,
    device: *mut std::ffi::c_void,
    hold: crate::media::DeviceRef,
    key: (bool, bool),
    probe: fn(*mut std::ffi::c_void, bool) -> Result<(), String>,
) -> Option<Result<(), String>> {
    let ten_bit = key.0;
    struct Raw(*mut std::ffi::c_void);
    #[cfg(windows)]
    // SAFETY: D3D11 devices are free-threaded and `hold` (`DeviceRef`) keeps the COM reference
    // alive for the probe thread's entire run.
    unsafe impl Send for Raw {}
    #[cfg(unix)]
    // SAFETY: The pointer is the renderer's SDL_GPUDevice, which outlives the probe thread (a new
    // renderer joins it first, `settings_probe_reset`), and the probe thread only borrows it to
    // create (and release) a shader module through `SDL_CreateGPUShader`. That relies on SDL's rule
    // that GPU device-level calls other than the ones on a command buffer may be made from any
    // thread, and on the Vulkan backend guarding its own bookkeeping for them (shader creation
    // touches no state the render thread's command buffers use). Nothing else of the device is
    // used from the probe thread.
    unsafe impl Send for Raw {}
    let mut probes = state.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, done)) = probes.results.iter().find(|(k, _)| *k == key) {
        return done.clone();
    }
    probes.results.push((key, None));
    let device = Raw(device);
    match std::thread::Builder::new().name("pyrowave-probe".into()).spawn(move || {
        let _hold = hold;
        let device = device;
        let result = probe(device.0, ten_bit);
        let mut probes = state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = probes.results.iter_mut().find(|(k, _)| *k == key) { entry.1 = Some(result); }
    }) {
        Ok(thread) => {
            probes.running.retain(|running| !running.is_finished());
            probes.running.push(thread);
            None
        }
        Err(error) => {
            let result = Err(format!("could not start PyroWave settings probe: {error}"));
            if let Some(entry) = probes.results.iter_mut().find(|(k, _)| *k == key) {
                entry.1 = Some(result.clone());
            }
            Some(result)
        }
    }
}

fn reset_in(state: &'static ProbeState) {
    // Joined unlocked: a probe that ends takes the lock to leave its result.
    let running = std::mem::take(&mut state.lock().unwrap_or_else(|e| e.into_inner()).running);
    for thread in running {
        if thread.join().is_err() {
            eprintln!("nightjar: the PyroWave settings probe panicked");
        }
    }
    state.lock().unwrap_or_else(|e| e.into_inner()).results.clear();
}

/// Whether `settings_probe` has been asked for `ten_bit` yet (tests: settings views that do not
/// need the probe must not start it).
#[cfg(test)]
pub fn settings_probe_started(ten_bit: bool) -> bool {
    settings_probe_state().lock().unwrap_or_else(|e| e.into_inner()).results.iter().any(|((k, _), _)| *k == ten_bit)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Linux's switch to the GPU renderer: a probe still on the old device ends before it goes, and
    /// what was found on it (an OpenGL start has no device: "needs the SDL GPU renderer") is not
    /// kept for the new one.
    #[test]
    fn a_new_renderer_waits_for_the_probe_and_probes_its_device_again() {
        static STATE: ProbeState = std::sync::Mutex::new(Probes { results: Vec::new(), running: Vec::new() });
        static DEVICES: std::sync::Mutex<Vec<usize>> = std::sync::Mutex::new(Vec::new());
        fn slow(device: *mut std::ffi::c_void, _: bool) -> Result<(), String> {
            std::thread::sleep(std::time::Duration::from_millis(50));
            DEVICES.lock().unwrap().push(device as usize);
            if device.is_null() { Err("needs the SDL GPU renderer".into()) } else { Ok(()) }
        }
        let opengl = std::ptr::null_mut();
        assert_eq!(probe_in(&STATE, opengl, crate::media::DeviceRef::default(), (false, false), slow), None, "it runs on its own thread");
        reset_in(&STATE);
        assert_eq!(*DEVICES.lock().unwrap(), [0], "the reset waited for it");
        assert!(STATE.lock().unwrap().results.is_empty(), "and forgot what it found");
        let gpu = 0x60 as *mut std::ffi::c_void;
        assert_eq!(probe_in(&STATE, gpu, crate::media::DeviceRef::default(), (false, false), slow), None, "probed again, on the new device");
        reset_in(&STATE);
        assert_eq!(*DEVICES.lock().unwrap(), [0, 0x60]);
        assert_eq!(probe_in(&STATE, gpu, crate::media::DeviceRef::default(), (false, false), slow), None);
        while STATE.lock().unwrap().results[0].1.is_none() {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(probe_in(&STATE, gpu, crate::media::DeviceRef::default(), (false, false), slow), Some(Ok(())), "kept until the renderer changes");
        reset_in(&STATE);
    }

    #[test]
    fn deferred_and_strict_null_device_probes_do_not_poison_each_other() {
        static STATE: ProbeState = std::sync::Mutex::new(Probes { results: Vec::new(), running: Vec::new() });
        fn strict_null(device: *mut std::ffi::c_void, _: bool) -> Result<(), String> {
            if device.is_null() { Err("PyroWave GPU decoding needs the SDL GPU renderer".into()) } else { Ok(()) }
        }
        fn deferred_null(_: *mut std::ffi::c_void, _: bool) -> Result<(), String> {
            Ok(())
        }
        assert_eq!(
            defer_gpu_probe(std::ptr::null_mut(), true),
            cfg!(target_os = "linux"),
            "only Linux OpenGL launcher with Renderer::Auto defers shader compilation"
        );
        assert!(!defer_gpu_probe(std::ptr::null_mut(), false));
        assert!(!defer_gpu_probe(0x60 as *mut std::ffi::c_void, true));

        // Strict OpenGL failure under (false, false) must not cache-hit when Auto probes under (false, true).
        let _ = probe_in(&STATE, std::ptr::null_mut(), crate::media::DeviceRef::default(), (false, false), strict_null);
        while STATE.lock().unwrap().results.iter().find(|(k, _)| *k == (false, false)).and_then(|e| e.1.as_ref()).is_none() {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(probe_in(&STATE, std::ptr::null_mut(), crate::media::DeviceRef::default(), (false, false), strict_null).unwrap().is_err());

        let _ = probe_in(&STATE, std::ptr::null_mut(), crate::media::DeviceRef::default(), (false, true), deferred_null);
        while STATE.lock().unwrap().results.iter().find(|(k, _)| *k == (false, true)).and_then(|e| e.1.as_ref()).is_none() {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(
            probe_in(&STATE, std::ptr::null_mut(), crate::media::DeviceRef::default(), (false, true), deferred_null),
            Some(Ok(()))
        );
        reset_in(&STATE);
    }

    #[cfg(windows)]
    #[test]
    fn windows_probe_holds_d3d11_device_alive_past_callers_release() {
        use windows::core::Interface;
        use windows::Win32::Foundation::HMODULE;
        use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_WARP, D3D_FEATURE_LEVEL_11_0};
        use windows::Win32::Graphics::Direct3D11::{D3D11CreateDevice, ID3D11Device, D3D11_CREATE_DEVICE_FLAG, D3D11_SDK_VERSION};

        static STATE: ProbeState = std::sync::Mutex::new(Probes { results: Vec::new(), running: Vec::new() });
        static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        static RELEASE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

        fn hold_and_check(raw: *mut std::ffi::c_void, _: bool) -> Result<(), String> {
            STARTED.store(true, std::sync::atomic::Ordering::Release);
            while !RELEASE.load(std::sync::atomic::Ordering::Acquire) {
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            // SAFETY: `DeviceRef` held inside `probe_in` must keep the D3D11 device alive even
            // after the caller dropped its own reference.
            unsafe {
                let device = ID3D11Device::from_raw_borrowed(&raw).ok_or("null device")?;
                let _ = device.GetDeviceRemovedReason();
            }
            Ok(())
        }

        let mut device = None;
        unsafe {
            D3D11CreateDevice(
                None, D3D_DRIVER_TYPE_WARP, HMODULE::default(), D3D11_CREATE_DEVICE_FLAG(0),
                Some(&[D3D_FEATURE_LEVEL_11_0]), D3D11_SDK_VERSION, Some(&mut device), None, None,
            ).unwrap();
        }
        let device = device.unwrap();
        let raw = device.as_raw();
        let hold = crate::media::DeviceRef::of_device(raw);
        assert_eq!(probe_in(&STATE, raw, hold, (false, false), hold_and_check), None);
        while !STARTED.load(std::sync::atomic::Ordering::Acquire) {
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        // Caller drops its own reference while the probe thread is still running.
        drop(device);
        RELEASE.store(true, std::sync::atomic::Ordering::Release);
        reset_in(&STATE);
    }

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
        } else if cfg!(target_os = "linux") {
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

    #[cfg(target_os = "linux")]
    #[test]
    fn cpu_mode_never_loads_the_sdk_library() {
        let loads = super::super::pyrowave_sdk::Api::shared_loads();
        let _ = availability(PyroWaveDecoder::Cpu, || Ok(()), || super::super::pyrowave_sdk::probe(false));
        assert_eq!(super::super::pyrowave_sdk::Api::shared_loads(), loads);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_gpu_probe_without_a_renderer_device_says_so_before_loading_the_sdk() {
        let loads = super::super::pyrowave_sdk::Api::shared_loads();
        let error = gpu_probe(std::ptr::null_mut(), false).unwrap_err();
        assert!(error.contains("SDL GPU renderer"), "{error}");
        assert_eq!(super::super::pyrowave_sdk::Api::shared_loads(), loads);
    }

    #[test]
    fn a_mac_decodes_on_the_gpu_only_with_apple_silicon_macos_26_and_a_passing_probe() {
        use PyroWaveDecoder::{Cpu, Gpu};
        let never = || -> Result<(), String> { panic!("the probe runs only where the GPU decoder can") };
        assert_eq!(mac_choice(Cpu, true, 26, never), MacChoice::Cpu(None), "a CPU choice is kept and probes nothing");
        assert_eq!(mac_choice(Gpu, true, 26, || Ok(())), MacChoice::Gpu);
        assert_eq!(mac_choice(Gpu, true, 27, || Ok(())), MacChoice::Gpu);
        let MacChoice::Cpu(Some(intel)) = mac_choice(Gpu, false, 26, never) else { panic!("Intel decodes on the CPU") };
        assert!(intel.contains("Apple silicon"), "{intel}");
        let MacChoice::Cpu(Some(old)) = mac_choice(Gpu, true, 15, never) else { panic!("macOS 15 decodes on the CPU") };
        assert!(old.contains("macOS 26") && old.contains("macOS 15"), "{old}");
        assert_eq!(mac_choice(Gpu, true, 26, || Err("the bundled Vulkan driver finds no GPU it supports".into())),
            MacChoice::Cpu(Some("the bundled Vulkan driver finds no GPU it supports".into())), "the probe's reason is kept");
    }

    #[test]
    fn a_mac_plane_shader_that_does_not_build_decodes_on_the_processor_and_is_named() {
        // `gpu_probe` hands the plane shader's error to `mac_choice` when it has the renderer's
        // device; that is the processor fallback the launch shows as its start notice, not the
        // refused stream a probe without the device allowed.
        let reason = "the plane shader cannot be created: its Metal pass does not compile";
        assert_eq!(mac_choice(PyroWaveDecoder::Gpu, true, 26, || Err(reason.to_owned())),
            MacChoice::Cpu(Some(reason.to_owned())), "a shader error is the processor's reason");
    }

    #[test]
    fn the_bundle_keeps_the_loader_in_frameworks_and_the_manifest_in_resources() {
        let exe = std::path::Path::new("/Applications/Nightjar.app/Contents/MacOS/nightjar");
        let (loader, icd) = bundle_paths(exe).unwrap();
        assert_eq!(loader, std::path::Path::new("/Applications/Nightjar.app/Contents/Frameworks/libvulkan.1.dylib"));
        assert_eq!(icd, std::path::Path::new("/Applications/Nightjar.app/Contents/Resources/vulkan/icd.d/kosmickrisp_icd.json"));
    }

    #[test]
    fn the_package_check_reports_a_folder_without_the_library() {
        // An empty folder, not the test executable's: ignored GPU tests may leave an SDK copy there.
        let dir = std::env::temp_dir().join(format!("nightjar-package-check-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let report = dir.join("report.txt");
        let code = package_check_at(&dir, &report);
        let text = std::fs::read_to_string(&report).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(code, 1, "{text}");
        assert!(text.starts_with("load: ") && text.contains("not found next to the executable"), "{text}");
        assert!(text.contains("probe: skipped"), "{text}");
    }
}
