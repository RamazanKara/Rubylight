//! The Linux side of the SDK decoder (brief G3): libpyrowave-shared.so on its own Vulkan device,
//! decoding into CPU plane buffers.
//!
//! The Windows decoder shares D3D11 textures and fences with the renderer. SDL's GPU renderer on
//! Linux gives no such handles, and a decoder on a different Vulkan device than the renderer
//! (an iGPU decoding for a dGPU display, say) could not share memory anyway, so the planes cross
//! through the CPU: the SDK decodes on the GPU (`pyrowave_decoder_decode_cpu_buffer_async`), reads the
//! planes back into the buffers handed to `decode`, and the presenter uploads them to its own
//! textures. The readback is one memcpy per plane out of host-visible memory.
//!
//! `decode` blocks until the GPU is done and the planes are in the buffers: the SDK's `complete`
//! waits on its fence without a timeout, so a hung Vulkan device blocks the decoder thread
//! instead of ending the stream after `GPU_DECODE_TIMEOUT` as on Windows.

use std::ffi::{c_char, c_void, CStr};
use std::ptr;
use std::sync::Arc;

use ash::vk;

use super::*;

/// The SDK's decoder on a Vulkan device of its own choosing (the default one), with the plane
/// buffers handed in per `decode`.
///
/// `Send` but not `Sync`: the SDK's entry points are not thread safe, so move it between
/// threads, never share it.
pub struct GpuDecoder {
    api: Arc<Api>,
    config: GpuConfig,
    device: RawDevice,
    decoder: RawDecoder,
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

    /// Creates the SDK device (the Vulkan driver's default adapter) and the decoder (compute
    /// path). `config.slots` is only the number of plane sets the stream keeps; the buffers are
    /// the caller's. Errors name the step that failed.
    pub fn with_api(api: Arc<Api>, config: GpuConfig) -> Result<Self, String> {
        if config.slots == 0 || config.width == 0 || config.height == 0 || (!config.chroma444 && (config.width % 2 != 0 || config.height % 2 != 0)) {
            return Err(format!("PyroWave GPU decoding: bad picture {}x{} with {} plane sets", config.width, config.height, config.slots));
        }
        let mut s = Self { api, config, device: ptr::null_mut(), decoder: ptr::null_mut() };
        // SAFETY: The out-pointers outlive each call, and `s` owns every handle made so Drop
        // releases them on any early return.
        unsafe {
            let result = (s.api.create_device_by_compat)(0, 0, ptr::null(), ptr::null(), ptr::null(), PRIORITY_MEDIUM, &mut s.device);
            if result != SUCCESS {
                return Err(format!("no PyroWave Vulkan device (PyroWave error {result}, {}): {}", result_name(result), diagnose_vulkan()));
            }
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
        // SAFETY: `decoder` is live while self is.
        unsafe { (self.api.decoder_clear)(self.decoder) }
    }

    /// Queues one record (or container packet) of the frame. CPU only: it copies into SDK staging.
    pub fn push_packet(&mut self, data: &[u8]) -> Result<(), String> {
        // SAFETY: `data` is a live slice for the call, which copies it.
        check(unsafe { (self.api.decoder_push_packet)(self.decoder, data.as_ptr().cast(), data.len()) }, "pushing a PyroWave packet")
    }

    /// Whether every block of the frame has been pushed (or, with `allow_partial`, enough of them).
    pub fn is_ready(&self, allow_partial: bool) -> bool {
        // SAFETY: `decoder` is live while self is.
        unsafe { (self.api.decoder_decode_is_ready)(self.decoder, allow_partial) }
    }

    /// Decodes the pushed records into `planes` (tightly packed rows, `GpuConfig::plane_bytes`
    /// each) and returns when they are final. Missing blocks decode as zero, so a partial frame
    /// blurs and never corrupts.
    pub fn decode(&mut self, planes: [&mut [u8]; 3]) -> Result<(), String> {
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
        // SAFETY: `buffer` points into the three live slices, which stay borrowed until the
        // decode completes; context 0 is one of the SDK's two frame contexts.
        unsafe {
            check((self.api.decoder_decode_cpu_buffer_async)(self.decoder, &buffer, 0), "submitting the PyroWave decode")?;
            check((self.api.decoder_decode_cpu_buffer_complete)(self.decoder, &buffer, 0), "completing the PyroWave decode")
        }
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
        // SAFETY: Each non-null handle was made on `self.device`, which goes last; the SDK idles
        // the GPU before destroying the decoder.
        unsafe {
            if !self.decoder.is_null() {
                (self.api.decoder_destroy)(self.decoder);
            }
            if !self.device.is_null() {
                (self.api.device_destroy)(self.device);
            }
        }
    }
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

    #[test]
    fn a_probe_without_the_library_names_the_file_and_where_it_was_expected() {
        let error = probe_in(Path::new("/nonexistent/libpyrowave-shared.so"), false).unwrap_err();
        assert!(error.contains("libpyrowave-shared.so") && error.contains("/nonexistent"), "{error}");
    }
}
