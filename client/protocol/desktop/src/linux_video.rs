//! What the Linux video decoders are: the VA driver libva names and the decode profiles it lists,
//! the renderer's Vulkan device, its driver and the codecs Vulkan video decodes on it; and the
//! VA driver quirks the decoder order follows (moonlight-qt's vaapi.cpp vendor-string checks).
//! Each is said in the log once, as `vaapi:`, `vulkan:` and `decoder order:` lines.

use crate::media::Codec;

/// Set to 1, VA-API stays in the decoder order on a VA driver that wraps another API (the VDPAU
/// backend, nvidia-vaapi-driver), which the order otherwise leaves out.
pub const FORCE_VAAPI_ENVIRONMENT: &str = "NIGHTJAR_FORCE_VAAPI";

/// The VA drivers the decoder order tells apart, by the vendor string libva reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VaDriver {
    /// Mesa's Gallium driver (radeonsi, the d3d12 one in WSL, nouveau).
    Mesa,
    /// intel-media-driver, Broadwell and newer.
    IntelIhd,
    /// intel-vaapi-driver, the legacy Intel one.
    IntelI965,
    /// libva-vdpau-driver: VA-API over VDPAU, which moonlight-qt avoids too.
    VdpauWrapper,
    /// nvidia-vaapi-driver: VA-API over NVDEC, whose 10-bit pictures Vulkan cannot import and
    /// which can hang in vaSyncSurface.
    NvidiaWrapper,
    Other,
}

impl VaDriver {
    pub fn of(vendor: &str) -> Self {
        let lower = vendor.to_ascii_lowercase();
        if vendor.contains("Splitted-Desktop Systems VDPAU backend for VA-API") {
            Self::VdpauWrapper
        } else if lower.contains("va-api nvdec") {
            Self::NvidiaWrapper
        } else if vendor.contains("i965") {
            Self::IntelI965
        } else if vendor.contains("iHD") {
            Self::IntelIhd
        } else if lower.contains("gallium") {
            Self::Mesa
        } else {
            Self::Other
        }
    }
}

/// What the decoder order does with VA-API on this driver.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VaapiPlan {
    /// VA-API is left out of the order.
    pub skip: bool,
    /// Why, or a note about the driver; none for a driver with nothing to say.
    pub why: Option<String>,
}

/// The plan for the VA driver of `vendor` (or why VA-API does not open), `force` being
/// `FORCE_VAAPI_ENVIRONMENT`.
pub fn vaapi_plan(vendor: Result<&str, &str>, force: bool) -> VaapiPlan {
    let vendor = match vendor {
        Ok(vendor) => vendor,
        Err(error) => return VaapiPlan { skip: false, why: Some(format!("VA-API does not open: {error}")) },
    };
    let wrapper = match VaDriver::of(vendor) {
        VaDriver::VdpauWrapper => "VA-API is the VDPAU wrapper (libva-vdpau-driver)",
        VaDriver::NvidiaWrapper => "NVIDIA: VA-API is the nvidia-vaapi-driver wrapper",
        VaDriver::IntelI965 => {
            return VaapiPlan {
                skip: false,
                why: Some("Intel i965, the legacy VA driver: intel-media-driver (iHD) decodes HEVC Main10 and newer codecs on Broadwell and later".into()),
            }
        }
        VaDriver::Mesa | VaDriver::IntelIhd | VaDriver::Other => return VaapiPlan { skip: false, why: None },
    };
    if force {
        VaapiPlan { skip: false, why: Some(format!("{wrapper}, kept by {FORCE_VAAPI_ENVIRONMENT}=1")) }
    } else {
        VaapiPlan { skip: true, why: Some(format!("{wrapper}; {FORCE_VAAPI_ENVIRONMENT}=1 tries it")) }
    }
}

/// VAProfile values of va.h that streams use.
const VA_PROFILE_H264_MAIN: i32 = 6;
const VA_PROFILE_H264_HIGH: i32 = 7;
const VA_PROFILE_H264_CONSTRAINED_BASELINE: i32 = 13;
const VA_PROFILE_HEVC_MAIN: i32 = 17;
const VA_PROFILE_HEVC_MAIN10: i32 = 18;
const VA_PROFILE_HEVC_MAIN444: i32 = 26;
const VA_PROFILE_HEVC_MAIN444_10: i32 = 27;
const VA_PROFILE_AV1_PROFILE0: i32 = 32;
const VA_PROFILE_AV1_PROFILE1: i32 = 33;
/// `VA_RT_FORMAT_YUV420_10`, the bit of `VAConfigAttribRTFormat` for 10-bit 4:2:0.
const VA_RT_FORMAT_YUV420_10: u32 = 0x100;

/// A profile the VA driver decodes (`VAEntrypointVLD`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VaDecodeProfile {
    /// The VAProfile.
    pub profile: i32,
    /// `VAConfigAttribRTFormat`, the `VA_RT_FORMAT_*` bits it decodes into (0 when unknown).
    pub rt_format: u32,
    /// `VAConfigAttribMaxPictureWidth` and `Height`, when the driver says.
    pub max: Option<(u32, u32)>,
}

/// What libva says about the device VAAPI opens on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VaapiIdentity {
    /// The render node's minor, none for FFmpeg's default device.
    pub node: Option<u32>,
    pub vendor: String,
    pub profiles: Vec<VaDecodeProfile>,
}

impl VaapiIdentity {
    /// The largest picture the driver decodes in `profile`, when it says.
    #[cfg_attr(not(test), allow(dead_code))] // Packed 4:4:4's size check reads it next.
    pub fn max_picture(&self, profile: i32) -> Option<(u32, u32)> {
        self.profiles.iter().find(|found| found.profile == profile).and_then(|found| found.max)
    }

    /// Whether the driver decodes the stream codec of `name`, as the `vaapi:` line names it
    /// (h264, hevc, hevc10, av1, av1-10 ...).
    pub fn decodes(&self, name: &str) -> bool {
        named_profiles(&self.profiles).iter().any(|&(known, _)| known == name)
    }

    /// The `vaapi:` log line.
    pub fn line(&self) -> String {
        let node = self.node.map_or_else(|| "default device".to_owned(), |minor| format!("renderD{minor}"));
        let named = named_profiles(&self.profiles);
        if named.is_empty() {
            return format!("vaapi: {node} \"{}\", decodes none of the stream's profiles", self.vendor);
        }
        format!("vaapi: {node} \"{}\", profiles {}", self.vendor, sizes(&named, ", max ", ""))
    }
}

/// The stream codecs of `profiles` by name, each with its largest picture: h264, hevc, hevc10,
/// hevc444, hevc444-10, av1, av1-10 (Profile 0 decoding 10-bit), av1-444.
fn named_profiles(profiles: &[VaDecodeProfile]) -> Vec<(&'static str, Option<(u32, u32)>)> {
    let mut named = Vec::new();
    let mut add = |name, max| {
        if !named.iter().any(|&(known, _)| known == name) {
            named.push((name, max));
        }
    };
    for wanted in [
        VA_PROFILE_H264_HIGH,
        VA_PROFILE_H264_MAIN,
        VA_PROFILE_H264_CONSTRAINED_BASELINE,
        VA_PROFILE_HEVC_MAIN,
        VA_PROFILE_HEVC_MAIN10,
        VA_PROFILE_HEVC_MAIN444,
        VA_PROFILE_HEVC_MAIN444_10,
        VA_PROFILE_AV1_PROFILE0,
        VA_PROFILE_AV1_PROFILE1,
    ] {
        let Some(found) = profiles.iter().find(|found| found.profile == wanted) else { continue };
        match wanted {
            VA_PROFILE_H264_HIGH | VA_PROFILE_H264_MAIN | VA_PROFILE_H264_CONSTRAINED_BASELINE => add("h264", found.max),
            VA_PROFILE_HEVC_MAIN => add("hevc", found.max),
            VA_PROFILE_HEVC_MAIN10 => add("hevc10", found.max),
            VA_PROFILE_HEVC_MAIN444 => add("hevc444", found.max),
            VA_PROFILE_HEVC_MAIN444_10 => add("hevc444-10", found.max),
            VA_PROFILE_AV1_PROFILE0 => {
                add("av1", found.max);
                if found.rt_format & VA_RT_FORMAT_YUV420_10 != 0 {
                    add("av1-10", found.max);
                }
            }
            _ => add("av1-444", found.max),
        }
    }
    named
}

/// `names` joined by spaces, then the largest picture between `open` and `close` when every one has
/// the same, else each name with its own: "h264 hevc, max 4096x4096" (`open` ", max ") or
/// "h264 (max 4096x2304) hevc (max 8192x4352)".
fn sizes(names: &[(&str, Option<(u32, u32)>)], open: &str, close: &str) -> String {
    let first = names.first().and_then(|&(_, size)| size);
    if names.iter().all(|&(_, size)| size == first) {
        let joined: Vec<_> = names.iter().map(|&(name, _)| name).collect();
        return match first {
            Some((width, height)) => format!("{}{open}{width}x{height}{close}", joined.join(" ")),
            None => joined.join(" "),
        };
    }
    let each: Vec<_> = names
        .iter()
        .map(|&(name, size)| match size {
            Some((width, height)) => format!("{name} (max {width}x{height})"),
            None => name.to_owned(),
        })
        .collect();
    each.join(" ")
}

/// The renderer's Vulkan device as its driver describes it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VulkanIdentity {
    /// `VkPhysicalDeviceProperties::deviceName`.
    pub device: String,
    /// `VkPhysicalDeviceDriverProperties::driverName` and `driverInfo` (empty before Vulkan 1.2).
    pub driver: String,
    pub info: String,
    /// The codecs the GPU decodes with Vulkan video, each with `maxCodedExtent` when its
    /// capabilities are known.
    pub decode: Vec<(Codec, Option<(u32, u32)>)>,
    /// Whether the renderer's device was made with the video extensions, so it can decode them.
    pub enabled: bool,
}

impl VulkanIdentity {
    /// The `vulkan:` log line.
    pub fn line(&self) -> String {
        let driver = match (self.driver.is_empty(), self.info.is_empty()) {
            (true, _) => "driver unnamed".to_owned(),
            (false, true) => self.driver.clone(),
            (false, false) => format!("{} {}", self.driver, self.info),
        };
        let named: Vec<_> = self
            .decode
            .iter()
            .map(|&(codec, size)| {
                let name = match codec {
                    Codec::H264 => "h264",
                    Codec::Hevc => "hevc",
                    Codec::Av1 => "av1",
                };
                (name, size)
            })
            .collect();
        let decode = if named.is_empty() {
            "no video decode".to_owned()
        } else if self.enabled {
            format!("video decode {}", sizes(&named, " (max ", ")"))
        } else {
            format!("video decode {}, not enabled on the renderer's device", sizes(&named, " (max ", ")"))
        };
        format!("vulkan: {}, {driver}, {decode}", self.device)
    }
}

/// The `decoder order:` log line for `labels` and why.
pub fn order_line(labels: &[&str], why: &str) -> String {
    let order = if labels.is_empty() { "none".to_owned() } else { labels.join(", ") };
    format!("decoder order: {order} ({why})")
}

/// Logs `line` the first time it is said.
#[cfg(target_os = "linux")]
pub fn note(line: &str) {
    static NOTED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
    let mut noted = NOTED.lock().unwrap_or_else(|e| e.into_inner());
    if !noted.iter().any(|known| known == line) {
        eprintln!("nightjar: {line}");
        noted.push(line.to_owned());
    }
}

#[cfg(target_os = "linux")]
pub use queries::{vaapi_identity, vulkan_identity};

#[cfg(target_os = "linux")]
mod queries {
    use std::ffi::{c_char, c_int, c_void, CStr};
    use std::ptr;
    use std::sync::OnceLock;

    use ash::vk;
    use sdl3_sys::everything::*;

    use super::{VaDecodeProfile, VaapiIdentity, VulkanIdentity};
    use crate::media::ffi::{av_buffer_unref, av_hwdevice_ctx_create, AVHWDeviceContext, AV_HWDEVICE_TYPE_VAAPI};
    use crate::media::Codec;
    use crate::stream::pyrowave_sdk::zero_copy::{self, SdlVulkan};
    use crate::vulkan_video::{Extensions, EXTENSIONS_PROPERTY};

    const VA_STATUS_SUCCESS: c_int = 0;
    const VA_ENTRYPOINT_VLD: c_int = 1;
    const VA_CONFIG_ATTRIB_RT_FORMAT: c_int = 0;
    const VA_CONFIG_ATTRIB_MAX_PICTURE_WIDTH: c_int = 18;
    const VA_CONFIG_ATTRIB_MAX_PICTURE_HEIGHT: c_int = 19;
    const VA_ATTRIB_NOT_SUPPORTED: u32 = 0x8000_0000;

    /// va.h's `VAConfigAttrib`.
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct ConfigAttrib {
        kind: c_int,
        value: u32,
    }

    type QueryVendorString = unsafe extern "C" fn(*mut c_void) -> *const c_char;
    type MaxNum = unsafe extern "C" fn(*mut c_void) -> c_int;
    type QueryConfigProfiles = unsafe extern "C" fn(*mut c_void, *mut c_int, *mut c_int) -> c_int;
    type QueryConfigEntrypoints = unsafe extern "C" fn(*mut c_void, c_int, *mut c_int, *mut c_int) -> c_int;
    type GetConfigAttributes = unsafe extern "C" fn(*mut c_void, c_int, c_int, *mut ConfigAttrib, c_int) -> c_int;

    /// libva's queries, from the libva.so.2 FFmpeg's VAAPI device has loaded already.
    struct Libva {
        vendor: QueryVendorString,
        max_profiles: MaxNum,
        max_entrypoints: MaxNum,
        profiles: QueryConfigProfiles,
        entrypoints: QueryConfigEntrypoints,
        attributes: GetConfigAttributes,
        _library: libloading::Library,
    }

    fn libva() -> Result<&'static Libva, String> {
        static LIBVA: OnceLock<Result<Libva, String>> = OnceLock::new();
        LIBVA
            .get_or_init(|| unsafe {
                // SAFETY: libva's exported functions, with their va.h signatures.
                let library = libloading::Library::new("libva.so.2").map_err(|e| format!("cannot load libva.so.2: {e}"))?;
                macro_rules! function {
                    ($name:literal) => {
                        *library.get(concat!($name, "\0").as_bytes()).map_err(|e| format!("{}: {e}", $name))?
                    };
                }
                Ok(Libva {
                    vendor: function!("vaQueryVendorString"),
                    max_profiles: function!("vaMaxNumProfiles"),
                    max_entrypoints: function!("vaMaxNumEntrypoints"),
                    profiles: function!("vaQueryConfigProfiles"),
                    entrypoints: function!("vaQueryConfigEntrypoints"),
                    attributes: function!("vaGetConfigAttributes"),
                    _library: library,
                })
            })
            .as_ref()
            .map_err(Clone::clone)
    }

    /// Opens VAAPI on render node `node` (FFmpeg's default device for none), as the decoder does,
    /// and asks libva for its vendor string and the profiles it decodes.
    pub fn vaapi_identity(node: Option<u32>) -> Result<VaapiIdentity, String> {
        let path = node.map(|minor| std::ffi::CString::new(format!("/dev/dri/renderD{minor}")).expect("no NUL in a path of digits"));
        let mut device = ptr::null_mut();
        // SAFETY: FFmpeg makes a device of its own, freed below.
        let status = unsafe {
            av_hwdevice_ctx_create(&mut device, AV_HWDEVICE_TYPE_VAAPI, path.as_ref().map_or(ptr::null(), |path| path.as_ptr()), ptr::null_mut(), 0)
        };
        if status < 0 {
            return Err(crate::media::error(status));
        }
        // SAFETY: A VAAPI device context, whose AVVAAPIDeviceContext starts with its VADisplay.
        let identity = unsafe {
            let context = &*(*device).data.cast::<AVHWDeviceContext>();
            let display = *context.hwctx.cast::<*mut c_void>();
            query(display, node)
        };
        // SAFETY: The reference made above.
        unsafe { av_buffer_unref(&mut device) };
        identity
    }

    /// # Safety
    /// `display` must be an initialized VADisplay.
    unsafe fn query(display: *mut c_void, node: Option<u32>) -> Result<VaapiIdentity, String> {
        let va = libva()?;
        let vendor = (va.vendor)(display);
        let vendor = if vendor.is_null() { "unknown".to_owned() } else { CStr::from_ptr(vendor).to_string_lossy().into_owned() };
        let mut listed = vec![0; (va.max_profiles)(display).max(0) as usize];
        let mut count = 0;
        if listed.is_empty() || (va.profiles)(display, listed.as_mut_ptr(), &mut count) != VA_STATUS_SUCCESS {
            count = 0;
        }
        listed.truncate(count.max(0) as usize);
        let mut entrypoints = vec![0; (va.max_entrypoints)(display).max(0) as usize];
        let mut profiles = Vec::new();
        for profile in listed {
            let mut found = 0;
            if entrypoints.is_empty()
                || (va.entrypoints)(display, profile, entrypoints.as_mut_ptr(), &mut found) != VA_STATUS_SUCCESS
                || !entrypoints[..(found.max(0) as usize).min(entrypoints.len())].contains(&VA_ENTRYPOINT_VLD)
            {
                continue;
            }
            let mut attributes = [VA_CONFIG_ATTRIB_RT_FORMAT, VA_CONFIG_ATTRIB_MAX_PICTURE_WIDTH, VA_CONFIG_ATTRIB_MAX_PICTURE_HEIGHT]
                .map(|kind| ConfigAttrib { kind, value: VA_ATTRIB_NOT_SUPPORTED });
            if (va.attributes)(display, profile, VA_ENTRYPOINT_VLD, attributes.as_mut_ptr(), attributes.len() as c_int) != VA_STATUS_SUCCESS {
                attributes.iter_mut().for_each(|attribute| attribute.value = VA_ATTRIB_NOT_SUPPORTED);
            }
            let known = |attribute: ConfigAttrib| (attribute.value != VA_ATTRIB_NOT_SUPPORTED).then_some(attribute.value);
            let max = known(attributes[1]).zip(known(attributes[2]));
            profiles.push(VaDecodeProfile { profile, rt_format: known(attributes[0]).unwrap_or(0), max });
        }
        Ok(VaapiIdentity { node, vendor, profiles })
    }

    /// The renderer's Vulkan device: its name, driver and the codecs Vulkan video decodes on it.
    ///
    /// # Safety
    /// `gpu` must be null or a live SDL GPU device.
    pub unsafe fn vulkan_identity(gpu: *mut SDL_GPUDevice) -> Result<VulkanIdentity, String> {
        let sdl = SdlVulkan::of_device(gpu)?;
        let get_instance_proc_addr = zero_copy::get_instance_proc_addr()?;
        let instance = ash::Instance::load(&ash::StaticFn { get_instance_proc_addr }, sdl.instance);
        let mut driver = vk::PhysicalDeviceDriverProperties::default();
        let mut properties = vk::PhysicalDeviceProperties2::default().push_next(&mut driver);
        instance.get_physical_device_properties2(sdl.physical_device, &mut properties);
        let device = text(properties.properties.device_name_as_c_str());
        let (driver_name, info) = (text(driver.driver_name_as_c_str()), text(driver.driver_info_as_c_str()));
        let gpu_has = crate::vulkan_video::extensions_of(&instance, sdl.physical_device);
        let made_with = Extensions(SDL_GetNumberProperty(SDL_GetGPUDeviceProperties(gpu), EXTENSIONS_PROPERTY.as_ptr(), 0) as u32);
        let mut decode = Vec::new();
        for codec in [Codec::H264, Codec::Hevc, Codec::Av1] {
            if gpu_has.decodes(codec) {
                decode.push((codec, max_coded_extent(get_instance_proc_addr, sdl.instance, sdl.physical_device, codec)));
            }
        }
        let enabled = decode.iter().any(|&(codec, _)| made_with.decodes(codec));
        Ok(VulkanIdentity { device, driver: driver_name, info, decode, enabled })
    }

    fn text(name: Result<&CStr, std::ffi::FromBytesUntilNulError>) -> String {
        name.map(|name| name.to_string_lossy().into_owned()).unwrap_or_default()
    }

    /// `maxCodedExtent` of 8-bit 4:2:0 `codec` pictures (H.264 High, HEVC Main, AV1 Main), when
    /// the driver answers.
    ///
    /// # Safety
    /// `physical_device` must be a physical device of `instance`, made with `get_instance_proc_addr`.
    unsafe fn max_coded_extent(
        get_instance_proc_addr: vk::PFN_vkGetInstanceProcAddr,
        instance: vk::Instance,
        physical_device: vk::PhysicalDevice,
        codec: Codec,
    ) -> Option<(u32, u32)> {
        use vk::native::*;
        let fns = ash::khr::video_queue::InstanceFn::load(|name| std::mem::transmute(get_instance_proc_addr(instance, name.as_ptr())));
        let mut h264 = vk::VideoDecodeH264ProfileInfoKHR::default()
            .std_profile_idc(StdVideoH264ProfileIdc_STD_VIDEO_H264_PROFILE_IDC_HIGH)
            .picture_layout(vk::VideoDecodeH264PictureLayoutFlagsKHR::PROGRESSIVE);
        let mut h265 = vk::VideoDecodeH265ProfileInfoKHR::default().std_profile_idc(StdVideoH265ProfileIdc_STD_VIDEO_H265_PROFILE_IDC_MAIN);
        let mut av1 = vk::VideoDecodeAV1ProfileInfoKHR::default().std_profile(StdVideoAV1Profile_STD_VIDEO_AV1_PROFILE_MAIN);
        let profile = vk::VideoProfileInfoKHR::default()
            .chroma_subsampling(vk::VideoChromaSubsamplingFlagsKHR::TYPE_420)
            .luma_bit_depth(vk::VideoComponentBitDepthFlagsKHR::TYPE_8)
            .chroma_bit_depth(vk::VideoComponentBitDepthFlagsKHR::TYPE_8);
        let profile = match codec {
            Codec::H264 => profile.video_codec_operation(vk::VideoCodecOperationFlagsKHR::DECODE_H264).push_next(&mut h264),
            Codec::Hevc => profile.video_codec_operation(vk::VideoCodecOperationFlagsKHR::DECODE_H265).push_next(&mut h265),
            Codec::Av1 => profile.video_codec_operation(vk::VideoCodecOperationFlagsKHR::DECODE_AV1).push_next(&mut av1),
        };
        let mut h264_caps = vk::VideoDecodeH264CapabilitiesKHR::default();
        let mut h265_caps = vk::VideoDecodeH265CapabilitiesKHR::default();
        let mut av1_caps = vk::VideoDecodeAV1CapabilitiesKHR::default();
        let mut decode_caps = vk::VideoDecodeCapabilitiesKHR::default();
        let caps = vk::VideoCapabilitiesKHR::default().push_next(&mut decode_caps);
        let mut caps = match codec {
            Codec::H264 => caps.push_next(&mut h264_caps),
            Codec::Hevc => caps.push_next(&mut h265_caps),
            Codec::Av1 => caps.push_next(&mut av1_caps),
        };
        let result = (fns.get_physical_device_video_capabilities_khr)(physical_device, &profile, &mut caps);
        (result == vk::Result::SUCCESS).then_some((caps.max_coded_extent.width, caps.max_coded_extent.height))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Vendor strings as libva reports them: moonlight-qt's vaapi.cpp checks, Mesa's
    // "Mesa Gallium driver <version> for <GPU> (<chip>, ...)", and the Intel drivers' own.
    const RADEONSI: &str = "Mesa Gallium driver 24.2.8-1ubuntu1~24.04.1 for AMD Radeon RX 7900 XT (radeonsi, navi31, LLVM 19.1.1, DRM 3.57, 6.8.0-49-generic)";
    const D3D12: &str = "Mesa Gallium driver 24.2.8 for D3D12 (AMD Radeon RX 7900 XT)";
    const IHD: &str = "Intel iHD driver for Intel(R) Gen Graphics - 24.1.0 ()";
    const I965: &str = "Intel i965 driver for Intel(R) Coffee Lake - 2.4.1";
    const VDPAU: &str = "Splitted-Desktop Systems VDPAU backend for VA-API - 0.7.4";
    const NVDEC: &str = "VA-API NVDEC driver [direct backend]";

    #[test]
    fn vendor_strings_name_their_va_driver() {
        assert_eq!(VaDriver::of(RADEONSI), VaDriver::Mesa);
        assert_eq!(VaDriver::of(D3D12), VaDriver::Mesa);
        assert_eq!(VaDriver::of(IHD), VaDriver::IntelIhd);
        assert_eq!(VaDriver::of(I965), VaDriver::IntelI965);
        assert_eq!(VaDriver::of(VDPAU), VaDriver::VdpauWrapper);
        assert_eq!(VaDriver::of(NVDEC), VaDriver::NvidiaWrapper);
        assert_eq!(VaDriver::of("va-api nvdec driver [egl backend]"), VaDriver::NvidiaWrapper, "any case, as moonlight-qt");
        assert_eq!(VaDriver::of("unknown"), VaDriver::Other);
    }

    #[test]
    fn wrapper_drivers_leave_vaapi_out_unless_forced_and_i965_is_noted() {
        for vendor in [RADEONSI, D3D12, IHD] {
            assert_eq!(vaapi_plan(Ok(vendor), false), VaapiPlan { skip: false, why: None }, "{vendor}");
        }
        let nvidia = vaapi_plan(Ok(NVDEC), false);
        assert!(nvidia.skip);
        assert_eq!(nvidia.why.as_deref(), Some("NVIDIA: VA-API is the nvidia-vaapi-driver wrapper; NIGHTJAR_FORCE_VAAPI=1 tries it"));
        assert!(vaapi_plan(Ok(VDPAU), false).skip);
        for vendor in [NVDEC, VDPAU] {
            let forced = vaapi_plan(Ok(vendor), true);
            assert!(!forced.skip, "{vendor}");
            assert!(forced.why.unwrap().contains("kept by NIGHTJAR_FORCE_VAAPI=1"));
        }
        let i965 = vaapi_plan(Ok(I965), false);
        assert!(!i965.skip);
        assert!(i965.why.unwrap().contains("intel-media-driver (iHD)"));
        let closed = vaapi_plan(Err("Input/output error"), false);
        assert_eq!(closed, VaapiPlan { skip: false, why: Some("VA-API does not open: Input/output error".into()) }, "kept: the decoder says why it fails");
    }

    #[test]
    fn the_vaapi_line_names_the_node_vendor_profiles_and_largest_picture() {
        let decode = |profile, rt_format, max| VaDecodeProfile { profile, rt_format, max };
        let size = Some((4096, 4096));
        let identity = VaapiIdentity {
            node: Some(128),
            vendor: RADEONSI.into(),
            profiles: vec![
                decode(VA_PROFILE_H264_MAIN, 1, size),
                decode(VA_PROFILE_H264_HIGH, 1, size),
                decode(VA_PROFILE_HEVC_MAIN, 1, size),
                decode(VA_PROFILE_HEVC_MAIN10, 0x100, size),
                decode(VA_PROFILE_AV1_PROFILE0, 0x101, size),
                decode(2, 1, None),
            ],
        };
        assert_eq!(identity.line(), format!("vaapi: renderD128 \"{RADEONSI}\", profiles h264 hevc hevc10 av1 av1-10, max 4096x4096"));
        assert_eq!(identity.max_picture(VA_PROFILE_HEVC_MAIN10), size);
        assert_eq!(identity.max_picture(VA_PROFILE_AV1_PROFILE1), None);
        let older = VaapiIdentity {
            node: None,
            vendor: I965.into(),
            profiles: vec![decode(VA_PROFILE_H264_HIGH, 1, Some((4096, 2304))), decode(VA_PROFILE_HEVC_MAIN, 1, Some((8192, 8192))), decode(VA_PROFILE_AV1_PROFILE0, 1, None)],
        };
        assert_eq!(older.line(), format!("vaapi: default device \"{I965}\", profiles h264 (max 4096x2304) hevc (max 8192x8192) av1"));
        let none = VaapiIdentity { node: Some(129), vendor: VDPAU.into(), profiles: vec![] };
        assert_eq!(none.line(), format!("vaapi: renderD129 \"{VDPAU}\", decodes none of the stream's profiles"));
    }

    #[test]
    fn the_vulkan_line_names_device_driver_and_video_codecs() {
        let mut identity = VulkanIdentity {
            device: "AMD Radeon RX 7900 XT (RADV NAVI31)".into(),
            driver: "radv".into(),
            info: "Mesa 24.2.8".into(),
            decode: vec![(Codec::H264, Some((4096, 4096))), (Codec::Hevc, Some((4096, 4096))), (Codec::Av1, Some((4096, 4096)))],
            enabled: true,
        };
        assert_eq!(identity.line(), "vulkan: AMD Radeon RX 7900 XT (RADV NAVI31), radv Mesa 24.2.8, video decode h264 hevc av1 (max 4096x4096)");
        identity.decode[2].1 = Some((8192, 4352));
        identity.enabled = false;
        assert_eq!(
            identity.line(),
            "vulkan: AMD Radeon RX 7900 XT (RADV NAVI31), radv Mesa 24.2.8, video decode h264 (max 4096x4096) hevc (max 4096x4096) av1 (max 8192x4352), not enabled on the renderer's device"
        );
        let plain = VulkanIdentity { device: "llvmpipe (LLVM 19.1.1, 256 bits)".into(), ..VulkanIdentity::default() };
        assert_eq!(plain.line(), "vulkan: llvmpipe (LLVM 19.1.1, 256 bits), driver unnamed, no video decode");
    }

    #[test]
    fn the_flatpak_drops_the_hosts_va_driver_variables() {
        // A host's LIBVA_DRIVER_NAME or LIBVA_DRIVERS_PATH names drivers the sandbox lacks.
        let manifest = include_str!("../packaging/linux/flatpak/io.github.ramazankara.nightjar.yml.in");
        for name in ["LIBVA_DRIVER_NAME", "LIBVA_DRIVERS_PATH"] {
            assert!(manifest.lines().any(|l| l.trim() == format!("- --unset-env={name}")), "{name}");
        }
    }

    #[test]
    fn the_order_line_lists_backends_and_why() {
        assert_eq!(order_line(&["Vulkan", "NVDEC"], "NVIDIA: VA-API is the nvidia-vaapi-driver wrapper"), "decoder order: Vulkan, NVDEC (NVIDIA: VA-API is the nvidia-vaapi-driver wrapper)");
        assert_eq!(order_line(&[], "software"), "decoder order: none (software)");
    }
}
