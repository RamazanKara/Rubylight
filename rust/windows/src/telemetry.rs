//! On-demand hardware samples. The host owns polling and keeps driver calls off stream threads.
use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    ffi::{CStr, c_char, c_void},
    ptr,
};
use windows::Win32::{Foundation::FILETIME, System::Threading::GetSystemTimes};

pub use butterpollo_core::config::HARDWARE_METRICS as METRICS;

/// A driver DLL from System32 only, so no DLL earlier in the search order
/// stands in for it.
pub(crate) fn system_library(name: &str) -> Result<libloading::Library> {
    use windows::Win32::System::{
        LibraryLoader::LOAD_LIBRARY_SEARCH_SYSTEM32, SystemInformation::GetSystemDirectoryW,
    };
    let mut directory = [0u16; 32768];
    // SAFETY: `directory` is a live, writable UTF-16 buffer and the call writes at most its length.
    let length = unsafe { GetSystemDirectoryW(Some(&mut directory)) } as usize;
    if length == 0 || length >= directory.len() {
        bail!("cannot locate the Windows system directory");
    }
    let path = std::path::PathBuf::from(String::from_utf16(&directory[..length])?).join(name);
    // SAFETY: `path` is an absolute System32 path, so no search-order DLL is picked up; loading
    // runs only the installed driver's initialisers.
    Ok(unsafe {
        libloading::os::windows::Library::load_with_flags(path, LOAD_LIBRARY_SEARCH_SYSTEM32.0)?
    }
    .into())
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Sample {
    pub cpu: BTreeMap<String, f64>,
    pub gpus: Vec<GpuSample>,
}
#[derive(Clone, Debug, Default, Serialize)]
pub struct GpuSample {
    pub id: i32,
    pub name: String,
    pub metrics: BTreeMap<String, f64>,
}

pub struct Collector {
    selected: Vec<String>,
    cpu: Option<[u64; 3]>,
    amd: Option<Adlx>,
}
impl Collector {
    /// Construct, sample and drop on the host's single telemetry worker.
    pub fn new(selected: &[String]) -> Self {
        let amd = if selected.is_empty() || selected.iter().any(|s| s.starts_with("gpu_")) {
            match Adlx::open() {
                Ok(amd) => Some(amd),
                Err(error) => {
                    tracing::debug!(error = %format!("{error:#}"), "AMD hardware metrics unavailable");
                    None
                }
            }
        } else {
            None
        };
        Self {
            selected: selected.to_vec(),
            cpu: wanted(selected, "cpu_usage_percent")
                .then(cpu_times)
                .flatten(),
            amd,
        }
    }
    pub fn sample(&mut self) -> Sample {
        let mut sample = Sample::default();
        if wanted(&self.selected, "cpu_usage_percent") {
            let next = cpu_times();
            if let Some(value) = self
                .cpu
                .zip(next)
                .and_then(|(old, new)| cpu_usage(old, new))
            {
                sample.cpu.insert("cpu_usage_percent".into(), value);
            }
            self.cpu = next;
        }
        if let Some(amd) = &self.amd {
            sample.gpus = amd.sample(&self.selected);
        }
        sample
    }
}
fn wanted(selected: &[String], metric: &str) -> bool {
    selected.is_empty() || selected.iter().any(|name| name == metric)
}
fn cpu_times() -> Option<[u64; 3]> {
    let [mut idle, mut kernel, mut user] = [FILETIME::default(); 3];
    // SAFETY: Each output points to a live FILETIME for this synchronous call.
    unsafe {
        GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)).ok()?;
    }
    Some(
        [idle, kernel, user]
            .map(|time| u64::from(time.dwLowDateTime) | (u64::from(time.dwHighDateTime) << 32)),
    )
}
fn cpu_usage(old: [u64; 3], new: [u64; 3]) -> Option<f64> {
    let idle = new[0].checked_sub(old[0])?;
    let total = new[1]
        .checked_sub(old[1])?
        .checked_add(new[2].checked_sub(old[2])?)?;
    (total > 0 && idle <= total).then(|| 100. * (total - idle) as f64 / total as f64)
}

// AMD's public ADLX C ABI, independently bound from ISystem.h and IPerformanceMonitoring*.h:
// https://github.com/GPUOpen-LibrariesAndSDKs/ADLX/tree/32b5a740d42295c5dfe9026b9f52683da0f3af91/SDK/Include
// ADLX interfaces are reference counted except IADLXSystem, which ADLXTerminate owns.
// Negotiate SDK v1.0 (1.0.5.30); base bindings use its common prefix, and later
// methods are accessed only after QueryInterface accepts the versioned interface.
// https://github.com/GPUOpen-LibrariesAndSDKs/ADLX/blob/b98ec6c3a1ba2ea31ff392f54c684788e7834ce8/SDK/Include/ADLXVersion.h
pub(crate) const ADLX_VERSION: u64 = (1 << 48) | (5 << 16) | 30;
pub(crate) type Handle = *mut c_void;
pub(crate) type GetInterface = unsafe extern "system" fn(Handle, *mut Handle) -> i32;
pub(crate) type GetGpuInterface = unsafe extern "system" fn(Handle, Handle, *mut Handle) -> i32;
pub(crate) type GetInt = unsafe extern "system" fn(Handle, *mut i32) -> i32;
type GetDouble = unsafe extern "system" fn(Handle, *mut f64) -> i32;
pub(crate) type GetBool = unsafe extern "system" fn(Handle, *mut u8) -> i32;
#[repr(C)]
pub(crate) struct BaseVtable {
    pub(crate) acquire: usize,
    pub(crate) release: unsafe extern "system" fn(Handle) -> i32,
    pub(crate) query: unsafe extern "system" fn(Handle, *const u16, *mut Handle) -> i32,
}
#[repr(C)]
struct SystemVtable {
    hybrid: usize,
    gpus: GetInterface,
    unused: [usize; 7],
    performance: GetInterface,
}
#[repr(C)]
pub(crate) struct ListVtable {
    pub(crate) base: BaseVtable,
    pub(crate) size: unsafe extern "system" fn(Handle) -> u32,
    pub(crate) unused: [usize; 7],
    pub(crate) at: unsafe extern "system" fn(Handle, u32, *mut Handle) -> i32,
}
#[repr(C)]
struct GpuVtable {
    base: BaseVtable,
    unused1: [usize; 4],
    name: unsafe extern "system" fn(Handle, *mut *const c_char) -> i32,
    unused2: [usize; 3],
    vram: unsafe extern "system" fn(Handle, *mut u32) -> i32,
    unused3: [usize; 6],
    id: GetInt,
}
#[repr(C)]
struct PerformanceVtable {
    base: BaseVtable,
    unused1: [usize; 15],
    current: GetGpuInterface,
    unused2: [usize; 2],
    supported: GetGpuInterface,
}
#[repr(C)]
struct SupportVtable {
    base: BaseVtable,
    supported: [GetBool; 10],
}
#[repr(C)]
struct Support1Vtable {
    base: SupportVtable,
    unused: [usize; 12],
    memory_temperature: GetBool,
}
#[repr(C)]
struct Support3Vtable {
    base: Support1Vtable,
    unused: [usize; 7],
    fan_percent: GetBool,
}
#[repr(C)]
struct MetricsVtable {
    base: BaseVtable,
    timestamp: usize,
    usage: GetDouble,
    clock: GetInt,
    memory_clock: GetInt,
    temperature: GetDouble,
    hotspot: GetDouble,
    power: GetDouble,
    board_power: GetDouble,
    fan: GetInt,
    vram: GetInt,
    voltage: GetInt,
}
#[repr(C)]
struct Metrics1Vtable {
    base: MetricsVtable,
    intake: usize,
    memory_temperature: GetDouble,
}
#[repr(C)]
struct Metrics3Vtable {
    base: Metrics1Vtable,
    unused: [usize; 3],
    fan_percent: GetInt,
}

pub(crate) struct Interface(pub(crate) Handle);
impl Interface {
    pub(crate) fn returned(result: i32, handle: Handle) -> Option<Self> {
        (result == 0 && !handle.is_null()).then(|| Self(handle))
    }
    pub(crate) unsafe fn vtable<T>(&self) -> &T {
        // SAFETY: The caller selects the exact public ADLX vtable for this live interface.
        unsafe { &**self.0.cast::<*const T>() }
    }
    fn query(&self, name: &str) -> Option<Self> {
        let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        let mut out = ptr::null_mut();
        // SAFETY: Every owned interface starts with BaseVtable; name and output remain live.
        let result =
            unsafe { (self.vtable::<BaseVtable>().query)(self.0, name.as_ptr(), &mut out) };
        Self::returned(result, out)
    }
}
impl Drop for Interface {
    fn drop(&mut self) {
        // SAFETY: This owns exactly one ADLX reference, and the runtime outlives its interfaces.
        unsafe {
            (self.vtable::<BaseVtable>().release)(self.0);
        }
    }
}
struct Gpu {
    interface: Interface,
    id: i32,
    name: String,
    vram_mb: u32,
    supported: [bool; 12],
}
struct Adlx {
    gpus: Vec<Gpu>,
    performance: Option<Interface>,
    terminate: unsafe extern "C" fn() -> i32,
    _library: libloading::Library,
}
impl Adlx {
    fn open() -> Result<Self> {
        let library = system_library("amdadlx64.dll")?;
        // SAFETY: These exports and C signatures are specified by AMD's ADLX.h.
        let (initialize, terminate) = unsafe {
            (
                *library
                    .get::<unsafe extern "C" fn(u64, *mut Handle) -> i32>(b"ADLXInitialize\0")?,
                *library.get::<unsafe extern "C" fn() -> i32>(b"ADLXTerminate\0")?,
            )
        };
        let mut system = ptr::null_mut();
        // SAFETY: The runtime is loaded, the version matches the baseline ABI and system is writable.
        let result = unsafe { initialize(ADLX_VERSION, &mut system) };
        if !matches!(result, 0..=2) || system.is_null() {
            bail!("ADLX initialization failed ({result})");
        }
        let mut amd = Self {
            gpus: vec![],
            performance: None,
            terminate,
            _library: library,
        };
        // SAFETY: ADLXInitialize returned IADLXSystem, whose lifetime ends with ADLXTerminate.
        let table = unsafe { &**system.cast::<*const SystemVtable>() };
        let mut performance = ptr::null_mut();
        // SAFETY: The system and output are live and the method returns a reference-counted interface.
        let result = unsafe { (table.performance)(system, &mut performance) };
        amd.performance = Some(
            Interface::returned(result, performance)
                .context("ADLX performance monitoring unavailable")?,
        );
        let mut list = ptr::null_mut();
        // SAFETY: The system and output are live; GetGPUs returns IADLXGPUList.
        let result = unsafe { (table.gpus)(system, &mut list) };
        let list = Interface::returned(result, list).context("ADLX GPU enumeration failed")?;
        // SAFETY: list was returned by GetGPUs as IADLXGPUList.
        let table = unsafe { list.vtable::<ListVtable>() };
        // SAFETY: The list remains live through enumeration.
        let count = unsafe { (table.size)(list.0) };
        for index in 0..count {
            let mut gpu = ptr::null_mut();
            // SAFETY: index is within the list; output remains live and At_GPUList acquires a reference.
            let result = unsafe { (table.at)(list.0, index, &mut gpu) };
            let Some(interface) = Interface::returned(result, gpu) else {
                continue;
            };
            if let Some(gpu) = amd.gpu(interface) {
                amd.gpus.push(gpu);
            }
        }
        Ok(amd)
    }
    fn gpu(&self, interface: Interface) -> Option<Gpu> {
        let performance = self.performance.as_ref()?;
        let mut supported = ptr::null_mut();
        // SAFETY: Both interfaces have their documented types; output remains live.
        let result = unsafe {
            (performance.vtable::<PerformanceVtable>().supported)(
                performance.0,
                interface.0,
                &mut supported,
            )
        };
        let support = Interface::returned(result, supported)?;
        let mut gpu = Gpu {
            interface,
            id: 0,
            name: String::new(),
            vram_mb: 0,
            supported: [false; 12],
        };
        // SAFETY: GetGPUs returned this IADLXGPU; metadata outputs and borrowed strings remain live.
        unsafe {
            let table = gpu.interface.vtable::<GpuVtable>();
            if (table.id)(gpu.interface.0, &mut gpu.id) != 0 {
                return None;
            }
            let mut name = ptr::null();
            if (table.name)(gpu.interface.0, &mut name) == 0 && !name.is_null() {
                gpu.name = CStr::from_ptr(name).to_string_lossy().into_owned();
            }
            (table.vram)(gpu.interface.0, &mut gpu.vram_mb);
            for (i, get) in support
                .vtable::<SupportVtable>()
                .supported
                .iter()
                .enumerate()
            {
                let mut value = 0;
                gpu.supported[i] = get(support.0, &mut value) == 0 && value != 0;
            }
            if let Some(support) = support.query("IADLXGPUMetricsSupport1") {
                let mut value = 0;
                gpu.supported[10] =
                    (support.vtable::<Support1Vtable>().memory_temperature)(support.0, &mut value)
                        == 0
                        && value != 0;
            }
            if let Some(support) = support.query("IADLXGPUMetricsSupport3") {
                let mut value = 0;
                gpu.supported[11] =
                    (support.vtable::<Support3Vtable>().fan_percent)(support.0, &mut value) == 0
                        && value != 0;
            }
        }
        Some(gpu)
    }
    fn sample(&self, selected: &[String]) -> Vec<GpuSample> {
        let Some(performance) = &self.performance else {
            return vec![];
        };
        let mut output = vec![];
        for gpu in &self.gpus {
            let mut metrics = ptr::null_mut();
            // SAFETY: The live performance service and GPU interface belong to this runtime.
            let result = unsafe {
                (performance.vtable::<PerformanceVtable>().current)(
                    performance.0,
                    gpu.interface.0,
                    &mut metrics,
                )
            };
            let Some(metrics) = Interface::returned(result, metrics) else {
                continue;
            };
            output.push(gpu_sample(gpu, &metrics, selected));
        }
        output
    }
}
fn gpu_sample(gpu: &Gpu, metrics: &Interface, selected: &[String]) -> GpuSample {
    let mut sample = GpuSample {
        id: gpu.id,
        name: gpu.name.clone(),
        metrics: BTreeMap::new(),
    };
    let mut add = |name: &str, value: Option<f64>| {
        if wanted(selected, name)
            && let Some(value) = value.filter(|v| v.is_finite() && *v >= 0.)
        {
            sample.metrics.insert(name.into(), value);
        }
    };
    // SAFETY: Current metrics and queried extensions carry their exact documented vtables;
    // getters write initialized scalar outputs, and all references outlive these calls.
    unsafe {
        let table = metrics.vtable::<MetricsVtable>();
        for (index, name, get) in [
            (0, "gpu_usage_percent", table.usage),
            (3, "gpu_temperature_c", table.temperature),
            (4, "gpu_hotspot_temperature_c", table.hotspot),
            (5, "gpu_power_w", table.power),
            (6, "gpu_board_power_w", table.board_power),
        ] {
            if gpu.supported[index] && wanted(selected, name) {
                let mut value = 0.;
                add(name, ((get)(metrics.0, &mut value) == 0).then_some(value));
            }
        }
        for (index, name, get) in [
            (1, "gpu_clock_mhz", table.clock),
            (2, "gpu_memory_clock_mhz", table.memory_clock),
            (7, "gpu_fan_rpm", table.fan),
            (9, "gpu_voltage_mv", table.voltage),
        ] {
            if gpu.supported[index] && wanted(selected, name) {
                let mut value = 0;
                add(
                    name,
                    ((get)(metrics.0, &mut value) == 0).then_some(f64::from(value)),
                );
            }
        }
        if gpu.supported[8]
            && (wanted(selected, "gpu_vram_used_mb") || wanted(selected, "gpu_vram_usage_percent"))
        {
            let mut value = 0;
            if (table.vram)(metrics.0, &mut value) == 0 {
                add("gpu_vram_used_mb", Some(f64::from(value)));
                if gpu.vram_mb > 0 {
                    add(
                        "gpu_vram_usage_percent",
                        Some(100. * f64::from(value) / f64::from(gpu.vram_mb)),
                    );
                }
            }
        }
        if gpu.supported[10]
            && wanted(selected, "gpu_memory_temperature_c")
            && let Some(metrics) = metrics.query("IADLXGPUMetrics1")
        {
            let mut value = 0.;
            let result =
                (metrics.vtable::<Metrics1Vtable>().memory_temperature)(metrics.0, &mut value);
            add("gpu_memory_temperature_c", (result == 0).then_some(value));
        }
        if gpu.supported[11]
            && wanted(selected, "gpu_fan_percent")
            && let Some(metrics) = metrics.query("IADLXGPUMetrics3")
        {
            let mut value = 0;
            let result = (metrics.vtable::<Metrics3Vtable>().fan_percent)(metrics.0, &mut value);
            add("gpu_fan_percent", (result == 0).then_some(f64::from(value)));
        }
    }
    sample
}
impl Drop for Adlx {
    fn drop(&mut self) {
        self.gpus.clear();
        self.performance.take();
        // SAFETY: All interfaces have been released and the ADLX library remains loaded.
        unsafe {
            (self.terminate)();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[repr(C)]
    struct FakeMetrics {
        table: *const MetricsVtable,
        calls: std::cell::Cell<usize>,
    }
    unsafe extern "system" fn release(_: Handle) -> i32 {
        0
    }
    unsafe extern "system" fn query(_: Handle, _: *const u16, _: *mut Handle) -> i32 {
        6
    }
    unsafe extern "system" fn number(handle: Handle, out: *mut f64) -> i32 {
        // SAFETY: Tests pass a live FakeMetrics and writable scalar output.
        unsafe {
            let fake = &*handle.cast::<FakeMetrics>();
            fake.calls.set(fake.calls.get() + 1);
            *out = 75.;
        }
        0
    }
    unsafe extern "system" fn integer(_: Handle, out: *mut i32) -> i32 {
        // SAFETY: Tests pass a writable scalar output.
        unsafe {
            *out = 1024;
        }
        0
    }
    unsafe extern "system" fn invalid(_: Handle, out: *mut f64) -> i32 {
        // SAFETY: Tests pass a writable scalar output.
        unsafe {
            *out = f64::NAN;
        }
        0
    }
    unsafe extern "system" fn unavailable(_: Handle, _: *mut f64) -> i32 {
        12
    }
    unsafe extern "system" fn negative(_: Handle, out: *mut i32) -> i32 {
        // SAFETY: Tests pass a writable scalar output.
        unsafe {
            *out = -1;
        }
        0
    }
    #[test]
    fn adlx_sample_omits_unsupported_failed_invalid_and_unselected_metrics() {
        let table = MetricsVtable {
            base: BaseVtable {
                acquire: 0,
                release,
                query,
            },
            timestamp: 0,
            usage: number,
            clock: integer,
            memory_clock: integer,
            temperature: number,
            hotspot: invalid,
            power: unavailable,
            board_power: number,
            fan: integer,
            vram: integer,
            voltage: negative,
        };
        let mut fake = FakeMetrics {
            table: &table,
            calls: std::cell::Cell::new(0),
        };
        let handle = (&mut fake as *mut FakeMetrics).cast();
        let mut gpu = Gpu {
            interface: Interface(handle),
            id: 7,
            name: "test GPU".into(),
            vram_mb: 4096,
            supported: [true; 12],
        };
        gpu.supported[1] = false;
        gpu.supported[7] = false;
        let metrics = Interface(handle);
        assert_eq!(fake.calls.get(), 0);
        let sample = gpu_sample(&gpu, &metrics, &[]);
        assert_eq!(
            sample.metrics,
            BTreeMap::from([
                ("gpu_usage_percent".into(), 75.),
                ("gpu_temperature_c".into(), 75.),
                ("gpu_board_power_w".into(), 75.),
                ("gpu_memory_clock_mhz".into(), 1024.),
                ("gpu_vram_used_mb".into(), 1024.),
                ("gpu_vram_usage_percent".into(), 25.),
            ])
        );
        let before = fake.calls.get();
        let selected = gpu_sample(&gpu, &metrics, &["gpu_usage_percent".into()]);
        assert_eq!(
            selected.metrics,
            BTreeMap::from([("gpu_usage_percent".into(), 75.)])
        );
        assert_eq!(fake.calls.get(), before + 1);
        let before = fake.calls.get();
        assert!(
            gpu_sample(&gpu, &metrics, &["unknown".into()])
                .metrics
                .is_empty()
        );
        assert_eq!(fake.calls.get(), before);
        assert!(Interface::returned(6, ptr::null_mut()).is_none());
    }
    #[test]
    fn cpu_only_selection_does_not_initialize_adlx() {
        let collector = Collector::new(&["cpu_usage_percent".into()]);
        assert!(collector.amd.is_none());
    }
    #[test]
    fn cpu_usage_subtracts_idle_from_kernel_and_skips_unusable_deltas() {
        assert_eq!(
            cpu_usage([100, 200, 300], [120, 230, 330]),
            Some(100. * 40. / 60.)
        );
        assert_eq!(cpu_usage([100, 200, 300], [100, 200, 300]), None);
        assert_eq!(cpu_usage([100, 200, 300], [99, 230, 330]), None);
        assert_eq!(cpu_usage([100, 200, 300], [200, 230, 330]), None);
    }
    #[test]
    fn adlx_vtable_offsets_match_the_public_c_interfaces() {
        let p = size_of::<usize>();
        assert_eq!(size_of::<SupportVtable>(), 13 * p);
        assert_eq!(size_of::<MetricsVtable>(), 14 * p);
        assert_eq!(std::mem::offset_of!(SystemVtable, performance), 9 * p);
        assert_eq!(std::mem::offset_of!(ListVtable, at), 11 * p);
        assert_eq!(std::mem::offset_of!(GpuVtable, id), 18 * p);
        assert_eq!(std::mem::offset_of!(PerformanceVtable, current), 18 * p);
        assert_eq!(std::mem::offset_of!(PerformanceVtable, supported), 21 * p);
        assert_eq!(
            std::mem::offset_of!(Support1Vtable, memory_temperature),
            25 * p
        );
        assert_eq!(std::mem::offset_of!(Support3Vtable, fan_percent), 33 * p);
        assert_eq!(
            std::mem::offset_of!(Metrics1Vtable, memory_temperature),
            15 * p
        );
        assert_eq!(std::mem::offset_of!(Metrics3Vtable, fan_percent), 19 * p);
    }
}
