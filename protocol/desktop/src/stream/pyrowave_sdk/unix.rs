//! The Linux side of the SDK decoder (brief G3): libpyrowave-shared.so on the SDL GPU renderer's
//! own Vulkan device, decoding into images the presenter copies on the GPU (`zero_copy.rs`,
//! `for_renderer`), or, where that device cannot be shared, on a Vulkan device of its own,
//! decoding into CPU plane buffers.
//!
//! The readback: the SDK decodes on the GPU (`pyrowave_decoder_decode_cpu_buffer_async`), reads
//! the planes back into the buffers handed to `decode`, and the presenter uploads them to its own
//! textures. The readback is one memcpy per plane out of host-visible memory.
//!
//! The shared device: `decode_into` decodes into a plane set of `SharedImages` and waits on the
//! decode timeline, under the same watchdog, so a picture is just as final when it is handed on.
//!
//! `decode` blocks until the GPU is done and the planes are in the buffers. The SDK's `complete`
//! waits on its fence without a timeout (and `async` submits to the queue, which a lost device can
//! block too), so both run in one job under a `Watchdog`: past `timeout` (the stream's
//! `GPU_DECODE_TIMEOUT`, as on Windows) `decode` fails loudly and the decoder is `stalled`, which
//! means it is never called, destroyed or reused again and the plane buffers the stuck call may
//! still write to are the caller's to leak. One job is one hand-over per frame, as before.
//!
//! The SDK sets a thread up inside its entry points (the silent logger in each, the thread index
//! in `async`; `complete` only waits on the fence and copies, which consult neither), so the wait
//! thread needs no setup of its own: its first job's `async` does it for the thread's life.

use std::ffi::{c_char, c_void, CStr};
use std::ptr;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use ash::vk;

use super::watchdog::Watchdog;
use super::zero_copy::{self, SdlVulkan, ShareInfo, SharedImages};
use super::*;

/// The SDK's decoder, on the renderer's Vulkan device with its plane sets there (`shared`), or on
/// a device of its own choosing (the default one) with the plane buffers handed in per `decode`.
///
/// `Send` but not `Sync`: the SDK's entry points are not thread safe, so move it between
/// threads, never share it.
pub struct GpuDecoder {
    api: Arc<Api>,
    config: GpuConfig,
    device: RawDevice,
    decoder: RawDecoder,
    watchdog: Watchdog,
    timeout: Duration,
    shared: Option<Shared>,
    /// The last value a decode signalled on the decode timeline.
    decode_value: u64,
    /// The queue the decodes run on (`queue.rs`).
    queue: queue::DecodeQueue,
    /// When the last decode's submit returned (`submitted_at`), set on the wait thread.
    submitted: Arc<Mutex<Option<Instant>>>,
}

/// The renderer's device as the SDK shares it.
struct Shared {
    images: Arc<SharedImages>,
    /// What `pyrowave_create_device` was given, which the SDK device keeps pointing into.
    info: Box<ShareInfo>,
}

/// Holds the queue lock `info` hands the SDK (SDL's queue mutex) until dropped.
///
/// The SDK idles the shared device (vkDeviceWaitIdle, which needs the queue to itself) while
/// `pyrowave_create_device` sets the device up, before it is given the lock, and again when
/// `pyrowave_device_destroy` releases it, without the lock: so both run under it, or the
/// renderer's own submissions on the main thread race them. SDL's mutex is recursive, so the
/// SDK taking it again inside is fine.
struct QueueHeld<'a>(&'a SharedDeviceCreateInfo);

impl<'a> QueueHeld<'a> {
    /// # Safety
    /// `info`'s callbacks and userdata must be a live lock.
    unsafe fn new(info: &'a SharedDeviceCreateInfo) -> Self {
        if let Some(lock) = info.queue_lock_callback {
            lock(info.userdata);
        }
        Self(info)
    }
}

impl Drop for QueueHeld<'_> {
    fn drop(&mut self) {
        if let Some(unlock) = self.0.queue_unlock_callback {
            // SAFETY: `new` took the same lock with the same userdata.
            unsafe { unlock(self.0.userdata) };
        }
    }
}

/// `pyrowave_create_device` on a shared device, under its queue lock (`QueueHeld`).
///
/// # Safety
/// As `pyrowave_create_device`, with `info`'s lock live.
unsafe fn create_shared(create: unsafe extern "C" fn(*const SharedDeviceCreateInfo, *mut RawDevice) -> PyroResult, info: &SharedDeviceCreateInfo, device: &mut RawDevice) -> PyroResult {
    let _held = QueueHeld::new(info);
    create(info, device)
}

/// Destroys `decoder`, then `device` (each skipped when null), under the queue lock of `shared`
/// when the device is a shared one (`QueueHeld`).
///
/// # Safety
/// The handles must be live and made on each other, and `shared`'s lock live.
unsafe fn release(
    decoder_destroy: unsafe extern "C" fn(RawDecoder),
    device_destroy: unsafe extern "C" fn(RawDevice),
    decoder: RawDecoder,
    device: RawDevice,
    shared: Option<&SharedDeviceCreateInfo>,
) {
    let _held = shared.map(|info| QueueHeld::new(info));
    if !decoder.is_null() {
        decoder_destroy(decoder);
    }
    if !device.is_null() {
        device_destroy(device);
    }
}

/// A decode into a shared plane set, for the wait thread: `buffers` are images of `images`, which
/// the job keeps alive.
struct SharedDecode {
    api: Arc<Api>,
    decoder: RawDecoder,
    images: Arc<SharedImages>,
    buffers: GpuBuffers,
    after: u64,
    value: u64,
    wait: Duration,
    submitted: Arc<Mutex<Option<Instant>>>,
}

// SAFETY: As `Pending`: the decoder outlives the call (a stalled one is leaked).
unsafe impl Send for SharedDecode {}

impl SharedDecode {
    /// Submits the decode after the reuse timeline reached `after` (the presenter's last copy out
    /// of the set), with a signal of `value` on the decode timeline, and waits for that.
    fn run(self) -> Result<(), String> {
        let (decode, reuse) = self.images.semaphores();
        let acquire = GpuSyncOperation { images: ptr::null(), num_images: 0, sync: SyncPoint { semaphore: reuse, value: self.after } };
        let release = GpuSyncOperation { images: ptr::null(), num_images: 0, sync: SyncPoint { semaphore: decode, value: self.value } };
        // SAFETY: The decoder is live, the images outlive the job, and the owner thread does not
        // call the SDK while it waits on this job.
        let result = unsafe { (self.api.decoder_decode_gpu_buffer)(self.decoder, if self.after > 0 { &acquire } else { ptr::null() }, &release, &self.buffers) };
        check(result, "decoding PyroWave into the renderer's planes")?;
        mark_submitted(&self.submitted);
        self.images.decode_queued(self.value);
        self.images.wait_decode(self.value, self.wait)
    }
}

/// Notes on the wait thread that a decode's submit has returned (`GpuDecoder::submitted_at`).
fn mark_submitted(submitted: &Mutex<Option<Instant>>) {
    *submitted.lock().unwrap_or_else(PoisonError::into_inner) = Some(Instant::now());
}

/// What the wait thread needs of a decode: the decoder and the buffer description. The planes
/// they point into stay alive because a decode that is not finished in time is never freed.
struct Pending {
    api: Arc<Api>,
    decoder: RawDecoder,
    buffer: CpuBuffer,
    submitted: Arc<Mutex<Option<Instant>>>,
}

// SAFETY: Raw pointers into a decoder and buffers that outlive the call, see `Pending`; the SDK's
// `complete` takes no thread-local state (it waits on a fence and copies).
unsafe impl Send for Pending {}

impl Pending {
    /// Submits the decode and waits for it, on the wait thread. Context 0 is one of the SDK's two
    /// frame contexts.
    fn run(self) -> Result<(), String> {
        // SAFETY: The decoder is live (a stalled one is leaked) and `buffer` points into planes
        // that stay borrowed until this returns, or are leaked if it does not. The owner thread
        // does not call the SDK while it waits on this job.
        unsafe {
            check((self.api.decoder_decode_cpu_buffer_async)(self.decoder, &self.buffer, 0), "submitting the PyroWave decode")?;
            mark_submitted(&self.submitted);
            check((self.api.decoder_decode_cpu_buffer_complete)(self.decoder, &self.buffer, 0), "completing the PyroWave decode")
        }
    }
}

// SAFETY: Every raw handle is owned by this value, and the SDK may be called from any one thread
// at a time, which `&mut self` guarantees.
unsafe impl Send for GpuDecoder {}

impl GpuConfig {
    /// Bytes per sample of the CPU planes: 1 for 8-bit, 2 for the 16-bit UNORM planes of a 10-bit stream.
    pub fn sample_bytes(&self) -> usize {
        if self.ten_bit { 2 } else { 1 }
    }

    /// The size in bytes of the CPU buffer plane `plane` needs (tightly packed rows).
    pub fn plane_bytes(&self, plane: usize) -> usize {
        let (width, height) = self.plane_size(plane);
        width as usize * height as usize * self.sample_bytes()
    }

    fn cpu_format(&self) -> i32 {
        match (self.chroma444, self.ten_bit) {
            (false, false) => CPU_FORMAT_YUV420P,
            (true, false) => CPU_FORMAT_YUV444P,
            (false, true) => CPU_FORMAT_YUV420P16,
            (true, true) => CPU_FORMAT_YUV444P16,
        }
    }
}

impl GpuDecoder {
    /// With the SDK next to the executable (loaded once per process, see `Api::shared`).
    pub fn new(config: GpuConfig) -> Result<Self, String> {
        Self::with_api(Api::shared()?, config)
    }

    /// The decoder for the renderer whose SDL GPU device is `device`: on that device, decoding
    /// into images the presenter copies on the GPU (`share`), unless `NIGHTJAR_PYROWAVE_READBACK=1`
    /// or the device cannot be shared; then on a device of the SDK's choosing, reading the planes
    /// back (`new`). Which way and why goes to the log once; the queue the decodes run on
    /// (`queue::async_compute`) and why goes to it at every stream.
    pub fn for_renderer(device: *mut c_void, config: GpuConfig) -> Result<Self, String> {
        let api = Api::shared()?;
        let async_compute = queue::async_compute();
        let shared = if zero_copy::readback_forced(std::env::var_os(zero_copy::FORCE_READBACK).as_deref()) {
            Err(format!("{}=1", zero_copy::FORCE_READBACK))
        } else {
            Self::share(api.clone(), device, config, async_compute)
        };
        let decoder = match shared {
            Ok(decoder) => {
                zero_copy::note("PyroWave GPU decoding shares the renderer's Vulkan device, without a plane readback");
                decoder
            }
            Err(reason) => {
                zero_copy::note(&format!("PyroWave GPU decoding reads its planes back through the CPU: {reason}"));
                Self::with_queue(api, config, async_compute)?
            }
        };
        eprintln!("nightjar: {}", decoder.queue.line());
        Ok(decoder)
    }

    /// The queue the decodes run on.
    pub fn decode_queue(&self) -> &queue::DecodeQueue {
        &self.queue
    }

    /// The decoder on the renderer's Vulkan device (`device`, its SDL GPU device), with
    /// `config.slots` plane sets there, the decodes on the compute queue SDL made beside its own
    /// with `async_compute` (`queue::shared_device_queue`). Errors say why the device cannot be
    /// shared.
    pub fn share(api: Arc<Api>, device: *mut c_void, config: GpuConfig, async_compute: bool) -> Result<Self, String> {
        check_config(config)?;
        // SAFETY: `device` is the renderer's live GPU device, which outlives the stream; `s` owns
        // every handle made, so Drop releases them on any early return, the SDK device before the
        // structs it points into.
        unsafe {
            let sdl = SdlVulkan::of_device(device.cast())?;
            sdl.require(zero_copy::Features::required())?;
            let get_instance_proc_addr = zero_copy::get_instance_proc_addr()?;
            let decode_queue = queue::shared_device_queue(async_compute, sdl.compute);
            let compute = match (&decode_queue, sdl.compute) {
                (queue::DecodeQueue::Compute(_), queue::RendererCompute::Queue(handle, slot)) => Some((handle, slot)),
                _ => None,
            };
            // On another family the planes are shared by both (CONCURRENT), so the presenter's
            // copy on SDL's queue reads what the decode wrote without an ownership transfer.
            let images = SharedImages::new(&sdl, get_instance_proc_addr, config, compute.map(|(_, slot)| slot.family))?;
            let info = ShareInfo::new(&sdl, get_instance_proc_addr, compute);
            let watchdog = Watchdog::new()?;
            let mut s = Self { api, config, device: ptr::null_mut(), decoder: ptr::null_mut(), watchdog, timeout: crate::stream::pyrowave::GPU_DECODE_TIMEOUT, shared: None, decode_value: 0, queue: queue::DecodeQueue::Graphics(queue::SETTING_OFF.into()), submitted: Arc::default() };
            let result = create_shared(s.api.create_device, &info.create, &mut s.device);
            s.shared = Some(Shared { images, info });
            if result != SUCCESS {
                return Err(format!("the SDK cannot share the renderer's Vulkan device (PyroWave error {result}, {})", result_name(result)));
            }
            // Before the decoder. The decode waits for the reuse timeline SDL's queue signals and
            // signals the decode timeline, which the decoder waits for before the presenter copies.
            s.queue = queue::apply(&s.api, s.device, decode_queue);
            let result = (s.api.decoder_create)(
                &DecoderCreateInfo { device: s.device, width: config.width as i32, height: config.height as i32, chroma: i32::from(config.chroma444), fragment_path: false },
                &mut s.decoder,
            );
            if result != SUCCESS {
                return Err(format!("the SDK's decoder does not run on the renderer's Vulkan device (PyroWave error {result}, {})", result_name(result)));
            }
            Ok(s)
        }
    }

    /// The plane sets on the renderer's device, when the decoder shares it.
    pub fn images(&self) -> Option<&Arc<SharedImages>> {
        self.shared.as_ref().map(|shared| &shared.images)
    }

    /// How long `decode` waits for the GPU before it fails (`GPU_DECODE_TIMEOUT` unless changed).
    pub fn set_decode_timeout(&mut self, timeout: Duration) {
        self.timeout = timeout;
    }

    /// Whether a decode missed its deadline. The decoder refuses everything from then on, and the
    /// buffers of that decode must be leaked, not reused, as the stuck call may still write to them.
    pub fn stalled(&self) -> bool {
        self.watchdog.stalled()
    }

    /// `with_queue` on the graphics queue.
    pub fn with_api(api: Arc<Api>, config: GpuConfig) -> Result<Self, String> {
        Self::with_queue(api, config, false)
    }

    /// Creates the SDK device (the Vulkan driver's default adapter) and the decoder (compute
    /// path), the decodes on a separate compute queue with `async_compute` where the device has one
    /// (`queue::own_device_queue`). `config.slots` is only the number of plane sets the stream
    /// keeps; the buffers are the caller's. Errors name the step that failed.
    pub fn with_queue(api: Arc<Api>, config: GpuConfig, async_compute: bool) -> Result<Self, String> {
        check_config(config)?;
        let watchdog = Watchdog::new()?;
        let mut s = Self { api, config, device: ptr::null_mut(), decoder: ptr::null_mut(), watchdog, timeout: crate::stream::pyrowave::GPU_DECODE_TIMEOUT, shared: None, decode_value: 0, queue: queue::DecodeQueue::Graphics(queue::SETTING_OFF.into()), submitted: Arc::default() };
        // SAFETY: The out-pointers outlive each call, and `s` owns every handle made so Drop
        // releases them on any early return.
        unsafe {
            let result = (s.api.create_device_by_compat)(0, 0, ptr::null(), ptr::null(), ptr::null(), PRIORITY_MEDIUM, &mut s.device);
            if result != SUCCESS {
                return Err(format!("no PyroWave Vulkan device (PyroWave error {result}, {}): {}", result_name(result), diagnose_vulkan()));
            }
            // Before the decoder; the SDK reads the planes back after its own fence, on either queue.
            let (api, device) = (s.api.clone(), s.device);
            s.queue = queue::apply(&api, device, queue::own_device_queue(async_compute, || queue::sdk_families(&api, device)));
            let result = (s.api.decoder_create)(
                &DecoderCreateInfo { device: s.device, width: config.width as i32, height: config.height as i32, chroma: i32::from(config.chroma444), fragment_path: false },
                &mut s.decoder,
            );
            if result != SUCCESS {
                return Err(decoder_refused(result, &diagnose_vulkan()));
            }
        }
        Ok(s)
    }

    pub fn config(&self) -> GpuConfig {
        self.config
    }

    pub fn slot_count(&self) -> usize {
        self.config.slots
    }

    /// Throws away the packets queued for the frame in progress.
    pub fn clear(&mut self) {
        if self.stalled() { return; }
        // SAFETY: `decoder` is live while self is.
        unsafe { (self.api.decoder_clear)(self.decoder) }
    }

    /// Queues one record (or container packet) of the frame. CPU only: it copies into SDK staging.
    pub fn push_packet(&mut self, data: &[u8]) -> Result<(), String> {
        if self.stalled() { return Err("the PyroWave decoder is stalled".into()); }
        // SAFETY: `data` is a live slice for the call, which copies it.
        check(unsafe { (self.api.decoder_push_packet)(self.decoder, data.as_ptr().cast(), data.len()) }, "pushing a PyroWave packet")
    }

    /// Whether every block of the frame has been pushed (or, with `allow_partial`, enough of them).
    pub fn is_ready(&self, allow_partial: bool) -> bool {
        if self.stalled() { return false; }
        // SAFETY: `decoder` is live while self is.
        unsafe { (self.api.decoder_decode_is_ready)(self.decoder, allow_partial) }
    }

    /// Decodes the pushed records into `planes` (tightly packed rows, `GpuConfig::plane_bytes`
    /// each) and returns when they are final, or fails if the GPU needs longer than the decode
    /// timeout (then `stalled`). Missing blocks decode as zero, so a partial frame blurs and never
    /// corrupts.
    pub fn decode(&mut self, planes: [&mut [u8]; 3]) -> Result<(), String> {
        if self.stalled() { return Err("the PyroWave decoder is stalled: an earlier decode never finished".into()); }
        let config = self.config;
        let mut buffer = CpuBuffer {
            data: [ptr::null_mut(); 3],
            row_stride_in_bytes: [0; 3],
            plane_size_in_bytes: [0; 3],
            width: config.width as i32,
            height: config.height as i32,
            format: config.cpu_format(),
        };
        for (index, plane) in planes.into_iter().enumerate() {
            let (width, _) = config.plane_size(index);
            if plane.len() < config.plane_bytes(index) {
                return Err(format!("PyroWave plane {index} buffer holds {} bytes, the decode needs {}", plane.len(), config.plane_bytes(index)));
            }
            buffer.data[index] = plane.as_mut_ptr().cast();
            buffer.row_stride_in_bytes[index] = width as usize * config.sample_bytes();
            buffer.plane_size_in_bytes[index] = config.plane_bytes(index);
        }
        // `buffer` points into the three live slices, which stay borrowed until the decode ends
        // (or are leaked by the caller when `stalled`).
        let pending = Pending { api: self.api.clone(), decoder: self.decoder, buffer, submitted: self.unmarked() };
        self.watchdog.run(self.timeout, move || pending.run())
    }

    /// Decodes the pushed records into plane set `set` of `images` once the presenter's copy out
    /// of it that signalled `after` on the reuse timeline is done (0: none), and returns when the
    /// planes are final, or fails like `decode`.
    pub fn decode_into(&mut self, set: usize, after: u64) -> Result<(), String> {
        if self.stalled() { return Err("the PyroWave decoder is stalled: an earlier decode never finished".into()); }
        let Some(shared) = &self.shared else { return Err("the PyroWave decoder does not share the renderer's device".into()) };
        if set >= self.config.slots { return Err(format!("PyroWave plane set {set} of {}", self.config.slots)); }
        self.decode_value += 1;
        // The wait outlasts the watchdog, which gives up first and marks the decoder stalled.
        let job = SharedDecode {
            api: self.api.clone(), decoder: self.decoder, images: shared.images.clone(), buffers: shared.images.views(set),
            after, value: self.decode_value, wait: self.timeout * 2, submitted: self.unmarked(),
        };
        self.watchdog.run(self.timeout, move || job.run())
    }

    /// When the submit of the last decode returned (in `decode` and `decode_into`, which also wait
    /// for the GPU), None if it failed: the rest of the call is the wait for the GPU.
    pub fn submitted_at(&self) -> Option<Instant> {
        *self.submitted.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The submit time of the next decode, cleared, for its job to set.
    fn unmarked(&self) -> Arc<Mutex<Option<Instant>>> {
        *self.submitted.lock().unwrap_or_else(PoisonError::into_inner) = None;
        self.submitted.clone()
    }

    /// The SDK's GPU timestamps since the last reset, as the lines it reports.
    pub fn performance_stats(&self, reset: bool) -> Vec<String> {
        unsafe extern "C" fn collect(userdata: *mut c_void, message: *const c_char) {
            if !message.is_null() {
                // SAFETY: `userdata` is the Vec below and `message` a NUL-terminated string.
                unsafe { (*userdata.cast::<Vec<String>>()).push(CStr::from_ptr(message).to_string_lossy().into_owned()) };
            }
        }
        let mut lines: Vec<String> = Vec::new();
        // SAFETY: The callback runs inside this call, with `lines` alive.
        unsafe { (self.api.device_report_performance_stats)(self.device, collect, (&raw mut lines).cast(), reset) };
        lines
    }
}

impl Drop for GpuDecoder {
    fn drop(&mut self) {
        // A stalled device is hung: destroying the decoder would idle it and block here for good,
        // and the stuck call is still using it. Leak both, and what a shared device points into.
        if self.stalled() {
            std::mem::forget(self.shared.take());
            return;
        }
        // SAFETY: Each non-null handle was made on `self.device`, which goes last; the SDK idles
        // the GPU before destroying the decoder. A shared device's lock is SDL's, which outlives it.
        unsafe {
            let shared = self.shared.as_ref().map(|shared| &shared.info.create);
            release(self.api.decoder_destroy, self.api.device_destroy, self.decoder, self.device, shared);
        }
    }
}

fn check_config(config: GpuConfig) -> Result<(), String> {
    if config.slots == 0 || config.width == 0 || config.height == 0 || (!config.chroma444 && (config.width % 2 != 0 || config.height % 2 != 0)) {
        return Err(format!("PyroWave GPU decoding: bad picture {}x{} with {} plane sets", config.width, config.height, config.slots));
    }
    Ok(())
}

/// The decoder is offered only if this succeeds, with the library next to the executable. It
/// builds a minimal decoder, so every requirement is exercised for real: the library and its
/// ABI, a Vulkan device with the subgroup features, and the decoder's pipelines for the plane
/// format (16-bit planes with `ten_bit`). The error names the first piece that is missing.
///
/// The library loads once per process and the result is remembered per plane format, so asking
/// again (every settings screen, every stream start) costs nothing.
pub fn probe(ten_bit: bool) -> Result<(), String> {
    let api = Api::shared()?;
    PROBES.get_or_probe(Luid::default(), ten_bit, || probe_once(api, ten_bit))
}

/// `probe` with the library at `path`, neither kept nor remembered.
pub fn probe_in(path: &Path, ten_bit: bool) -> Result<(), String> {
    probe_once(Api::load_from(path)?, ten_bit)
}

fn probe_once(api: Arc<Api>, ten_bit: bool) -> Result<(), String> {
    GpuDecoder::with_api(api, GpuConfig { width: 128, height: 128, chroma444: false, ten_bit, slots: 1 }).map(drop)
}

/// Why the SDK may have refused a Vulkan device, read from the driver directly: the Vulkan
/// devices the loader lists and the first requirement of the decoder each one misses.
pub fn diagnose_vulkan() -> String {
    // SAFETY: Plain Vulkan calls on handles made here; the instance is destroyed before returning.
    unsafe {
        let entry = match ash::Entry::load() {
            Ok(entry) => entry,
            Err(e) => return format!("libvulkan.so.1 cannot be loaded, so there is no Vulkan driver (install the Vulkan loader and your GPU's Vulkan driver, e.g. mesa-vulkan-drivers): {e}"),
        };
        let app = vk::ApplicationInfo::default().api_version(vk::make_api_version(0, 1, 3, 0));
        let instance = match entry.create_instance(&vk::InstanceCreateInfo::default().application_info(&app), None) {
            Ok(instance) => instance,
            Err(e) => return format!("the Vulkan loader cannot create an instance ({e})"),
        };
        let found = describe_devices(&instance);
        instance.destroy_instance(None);
        found
    }
}

unsafe fn describe_devices(instance: &ash::Instance) -> String {
    let devices = match instance.enumerate_physical_devices() {
        Ok(devices) => devices,
        Err(e) => return format!("Vulkan lists no devices ({e})"),
    };
    if devices.is_empty() {
        return "Vulkan lists no devices (is a Vulkan driver for the GPU installed?)".to_owned();
    }
    let mut verdicts = Vec::new();
    for device in devices {
        let mut subgroup = vk::PhysicalDeviceSubgroupProperties::default();
        let mut properties = vk::PhysicalDeviceProperties2::default().push_next(&mut subgroup);
        instance.get_physical_device_properties2(device, &mut properties);
        let properties = properties.properties;
        let name = properties.device_name_as_c_str().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let version = properties.api_version;
        if version < vk::make_api_version(0, 1, 3, 0) {
            verdicts.push(format!("{name} supports Vulkan {}.{}, the decoder needs 1.3", vk::api_version_major(version), vk::api_version_minor(version)));
            continue;
        }
        let needed = vk::SubgroupFeatureFlags::BASIC
            | vk::SubgroupFeatureFlags::VOTE
            | vk::SubgroupFeatureFlags::ARITHMETIC
            | vk::SubgroupFeatureFlags::BALLOT
            | vk::SubgroupFeatureFlags::SHUFFLE
            | vk::SubgroupFeatureFlags::SHUFFLE_RELATIVE;
        let missing = needed & !subgroup.supported_operations;
        if !missing.is_empty() {
            verdicts.push(format!("{name} lacks subgroup operations (missing bits {:#x} of VkSubgroupFeatureFlags)", missing.as_raw()));
            continue;
        }
        let mut v13 = vk::PhysicalDeviceVulkan13Features::default();
        let mut features = vk::PhysicalDeviceFeatures2::default().push_next(&mut v13);
        instance.get_physical_device_features2(device, &mut features);
        if v13.subgroup_size_control == vk::FALSE {
            verdicts.push(format!("{name} lacks subgroup size control"));
            continue;
        }
        verdicts.push(format!("{name} meets the decoder's Vulkan requirements"));
    }
    verdicts.join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_planes_follow_the_chroma_format_and_sample_size() {
        let config = GpuConfig { width: 132, height: 98, chroma444: false, ten_bit: false, slots: 3 };
        assert_eq!([0, 1, 2].map(|p| config.plane_bytes(p)), [132 * 98, 66 * 49, 66 * 49]);
        let config = GpuConfig { width: 131, height: 97, chroma444: true, ten_bit: true, slots: 3 };
        assert_eq!([0, 1, 2].map(|p| config.plane_bytes(p)), [131 * 97 * 2; 3]);
        let formats = [(false, false), (true, false), (false, true), (true, true)]
            .map(|(chroma444, ten_bit)| GpuConfig { width: 8, height: 8, chroma444, ten_bit, slots: 1 }.cpu_format());
        assert_eq!(formats, [CPU_FORMAT_YUV420P, CPU_FORMAT_YUV444P, CPU_FORMAT_YUV420P16, CPU_FORMAT_YUV444P16]);
    }

    thread_local! {
        /// The fake queue lock's depth, and what the fake SDK saw: (call, depth at the call).
        static DEPTH: std::cell::Cell<i32> = const { std::cell::Cell::new(0) };
        static CALLS: std::cell::RefCell<Vec<(&'static str, i32)>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    unsafe extern "C" fn fake_lock(userdata: *mut c_void) {
        assert_eq!(userdata as usize, 0x5d1);
        DEPTH.with(|depth| depth.set(depth.get() + 1));
    }

    unsafe extern "C" fn fake_unlock(userdata: *mut c_void) {
        assert_eq!(userdata as usize, 0x5d1);
        DEPTH.with(|depth| depth.set(depth.get() - 1));
    }

    fn record(call: &'static str) {
        CALLS.with(|calls| calls.borrow_mut().push((call, DEPTH.with(|depth| depth.get()))));
    }

    unsafe extern "C" fn fake_create(_: *const SharedDeviceCreateInfo, device: *mut RawDevice) -> PyroResult {
        record("create_device");
        *device = 0x1 as RawDevice;
        SUCCESS
    }

    unsafe extern "C" fn fake_decoder_destroy(_: RawDecoder) { record("decoder_destroy"); }
    unsafe extern "C" fn fake_device_destroy(_: RawDevice) { record("device_destroy"); }

    fn calls() -> Vec<(&'static str, i32)> {
        CALLS.with(|calls| std::mem::take(&mut *calls.borrow_mut()))
    }

    // The SDK idles the shared device without its queue lock while it creates and destroys it
    // (Granite's init_frame_contexts and Context::destroy_device): both must run under SDL's lock.
    #[test]
    fn the_shared_device_is_created_and_destroyed_under_the_queue_lock() {
        unsafe extern "system" fn loader(_: vk::Instance, _: *const c_char) -> vk::PFN_vkVoidFunction { None }
        let info = SharedDeviceCreateInfo {
            get_instance_proc_addr: loader,
            instance: vk::Instance::null(),
            physical_device: vk::PhysicalDevice::null(),
            device: vk::Device::null(),
            instance_create_info: ptr::null(),
            device_create_info: ptr::null(),
            queue_info: ptr::null_mut(),
            queue_info_count: 0,
            queue_lock_callback: Some(fake_lock),
            queue_unlock_callback: Some(fake_unlock),
            userdata: 0x5d1 as *mut c_void,
        };
        let mut device: RawDevice = ptr::null_mut();
        unsafe {
            assert_eq!(create_shared(fake_create, &info, &mut device), SUCCESS);
            assert!(!device.is_null());
            release(fake_decoder_destroy, fake_device_destroy, 0x2 as RawDecoder, device, Some(&info));
        }
        assert_eq!(calls(), [("create_device", 1), ("decoder_destroy", 1), ("device_destroy", 1)]);
        assert_eq!(DEPTH.with(|depth| depth.get()), 0, "the lock is released after each");

        // A device of the SDK's own has no lock to take; null handles are skipped.
        unsafe {
            release(fake_decoder_destroy, fake_device_destroy, 0x2 as RawDecoder, 0x1 as RawDevice, None);
            release(fake_decoder_destroy, fake_device_destroy, ptr::null_mut(), ptr::null_mut(), Some(&info));
        }
        assert_eq!(calls(), [("decoder_destroy", 0), ("device_destroy", 0)]);
        assert_eq!(DEPTH.with(|depth| depth.get()), 0);
    }

    #[test]
    fn a_probe_without_the_library_names_the_file_and_where_it_was_expected() {
        let error = probe_in(Path::new("/nonexistent/libpyrowave-shared.so"), false).unwrap_err();
        assert!(error.contains("libpyrowave-shared.so") && error.contains("/nonexistent"), "{error}");
    }
}
