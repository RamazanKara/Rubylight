//! What a Linux machine needs for hardware decoding, said once per stream: the system as the
//! `linux:` line, and pure rules over it and the decoders (`crate::linux_video`) naming what is
//! missing and how to get it, for the log and the stream's start toast.

use crate::linux_video::{VaDriver, VaapiIdentity, VulkanIdentity};

/// The system around the decoders.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct System {
    /// `/etc/os-release`'s `ID` and `ID_LIKE` words, and its `PRETTY_NAME`.
    pub os_ids: Vec<String>,
    pub os_name: String,
    /// Inside a Flatpak: its runtime (`/.flatpak-info`), empty when not named.
    pub flatpak: Option<String>,
    /// The GL extensions the Flatpak runtime has (`/usr/lib/<arch>/GL`), such as `nvidia-550-54-14`.
    pub gl_extensions: Vec<String>,
    /// NVIDIA's kernel driver version (`/proc/driver/nvidia/version`).
    pub nvidia: Option<String>,
    /// `XDG_SESSION_TYPE` and `XDG_CURRENT_DESKTOP`.
    pub session: String,
}

impl System {
    /// The `linux:` log line.
    pub fn line(&self) -> String {
        let mut parts = vec![if self.os_name.is_empty() { "unknown distribution".to_owned() } else { self.os_name.clone() }];
        match &self.flatpak {
            Some(runtime) if runtime.is_empty() => parts.push("Flatpak".into()),
            Some(runtime) => parts.push(format!("Flatpak {runtime}")),
            None => {}
        }
        if let Some(version) = &self.nvidia {
            parts.push(format!("NVIDIA driver {version}"));
        }
        if !self.session.is_empty() {
            parts.push(self.session.clone());
        }
        format!("linux: {}", parts.join(", "))
    }

    fn is(&self, id: &str) -> bool {
        self.os_ids.iter().any(|known| known == id)
    }
}

/// `/etc/os-release`'s `ID` and `ID_LIKE` words and `PRETTY_NAME` (or `NAME`).
pub fn parse_os_release(text: &str) -> (Vec<String>, String) {
    let value = |key: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
            .map(|value| value.trim().trim_matches(|c| c == '"' || c == '\'').to_owned())
    };
    let mut ids: Vec<String> = value("ID").into_iter().collect();
    ids.extend(value("ID_LIKE").unwrap_or_default().split_whitespace().map(str::to_owned));
    (ids, value("PRETTY_NAME").or_else(|| value("NAME")).unwrap_or_default())
}

/// The runtime of `/.flatpak-info` (`runtime=runtime/org.freedesktop.Platform/x86_64/24.08`),
/// without its `runtime/` kind.
pub fn parse_flatpak_runtime(text: &str) -> String {
    text.lines()
        .find_map(|line| line.trim().strip_prefix("runtime="))
        .map(|runtime| runtime.strip_prefix("runtime/").unwrap_or(runtime).to_owned())
        .unwrap_or_default()
}

/// The version in `/proc/driver/nvidia/version`'s first line ("NVRM version: NVIDIA UNIX x86_64
/// Kernel Module  550.54.14  Thu Feb 22 ..."), the first word that is numbers and dots.
pub fn parse_nvidia_version(text: &str) -> Option<String> {
    text.lines().next()?.split_whitespace().find(|word| {
        word.contains('.') && word.chars().all(|c| c.is_ascii_digit() || c == '.') && word.starts_with(|c: char| c.is_ascii_digit())
    }).map(str::to_owned)
}

/// What is missing, found by [`diagnose`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Diagnosis {
    /// A Mesa VA driver without H.264 and HEVC (Fedora's stock Mesa, built without them).
    MesaWithoutCodecs { fedora: bool },
    /// NVIDIA's VA-API wrapper is left out and Vulkan video does not decode either: NVDEC copies
    /// each picture through memory.
    NvidiaWithoutVulkanVideo,
    /// A Flatpak on NVIDIA's driver without the runtime's GL extension for that version.
    FlatpakWithoutNvidiaGl { extension: String },
    /// Intel's legacy i965 VA driver without HEVC Main10.
    IntelI965,
}

impl Diagnosis {
    /// The log line, English.
    pub fn line(&self) -> String {
        match self {
            Self::MesaWithoutCodecs { fedora: true } => "diagnosis: Fedora's Mesa VA driver decodes neither H.264 nor HEVC; mesa-va-drivers-freeworld (RPM Fusion) or the Flatpak does".into(),
            Self::MesaWithoutCodecs { fedora: false } => "diagnosis: the Mesa VA driver decodes neither H.264 nor HEVC; the distribution's full Mesa VA drivers or the Flatpak do".into(),
            Self::NvidiaWithoutVulkanVideo => "diagnosis: NVIDIA without Vulkan video decoding: NVDEC decodes, its pictures copied through memory".into(),
            Self::FlatpakWithoutNvidiaGl { extension } => format!("diagnosis: the Flatpak runtime lacks {extension}, NVIDIA's driver for it; flatpak update installs it"),
            Self::IntelI965 => "diagnosis: Intel's legacy i965 VA driver decodes no HEVC Main10; intel-media-driver (iHD) does on Broadwell and newer".into(),
        }
    }

    /// The toast, in the language in use.
    pub fn toast(&self) -> String {
        use crate::strings::{tr, trf};
        match self {
            Self::MesaWithoutCodecs { fedora: true } => {
                tr("Fedora's Mesa leaves out H.264 and HEVC decoding. Install mesa-va-drivers-freeworld from RPM Fusion, or use the Nightjar Flatpak.").to_owned()
            }
            Self::MesaWithoutCodecs { fedora: false } => {
                tr("This system's VA-API driver decodes neither H.264 nor HEVC. Install your distribution's full Mesa VA drivers, or use the Nightjar Flatpak.").to_owned()
            }
            Self::NvidiaWithoutVulkanVideo => {
                tr("This NVIDIA driver has no Vulkan video decoding, so each picture is copied through memory. A newer NVIDIA driver adds it.").to_owned()
            }
            Self::FlatpakWithoutNvidiaGl { extension } => trf("The Flatpak lacks {}, the NVIDIA driver it needs. Run flatpak update to install it.", &[extension]),
            Self::IntelI965 => tr("Intel's legacy VA-API driver (i965) is in use. Install intel-media-driver for HEVC Main10 and AV1.").to_owned(),
        }
    }
}

/// What the machine is missing for hardware decoding, from `system`, the VA driver VAAPI opens
/// (or why it does not), the renderer's Vulkan device (none for another renderer or when it is
/// not known) and `NIGHTJAR_FORCE_VAAPI` (`forced`).
pub fn diagnose(system: &System, vaapi: Result<&VaapiIdentity, &str>, vulkan: Option<&VulkanIdentity>, forced: bool) -> Vec<Diagnosis> {
    let mut found = Vec::new();
    let vulkan_decodes = |codec| vulkan.is_some_and(|vulkan| vulkan.enabled && vulkan.decode.iter().any(|&(known, _)| known == codec));
    if let (Some(version), Some(_)) = (&system.nvidia, &system.flatpak) {
        let extension = format!("nvidia-{}", version.replace('.', "-"));
        if !system.gl_extensions.contains(&extension) {
            found.push(Diagnosis::FlatpakWithoutNvidiaGl { extension: format!("org.freedesktop.Platform.GL.{extension}") });
        }
    }
    if let Ok(identity) = vaapi {
        match VaDriver::of(&identity.vendor) {
            VaDriver::Mesa if !identity.decodes("h264") && !identity.decodes("hevc") && !vulkan_decodes(crate::media::Codec::Hevc) => {
                found.push(Diagnosis::MesaWithoutCodecs { fedora: system.is("fedora") && system.flatpak.is_none() });
            }
            VaDriver::NvidiaWrapper if !forced && !vulkan_decodes(crate::media::Codec::Hevc) => found.push(Diagnosis::NvidiaWithoutVulkanVideo),
            VaDriver::IntelI965 if !identity.decodes("hevc10") => found.push(Diagnosis::IntelI965),
            _ => {}
        }
    } else if system.nvidia.is_some() && system.flatpak.is_none() && vulkan.is_some() && !vulkan_decodes(crate::media::Codec::Hevc) {
        found.push(Diagnosis::NvidiaWithoutVulkanVideo);
    }
    found
}

/// Full chroma's packed modes (`rubylight_protocol::packed444::mode_bit`: HEVC 8-bit, 10-bit,
/// AV1 8-bit, 10-bit) whose packed picture of `coded` size the decoders here open, and when some
/// are dropped the largest picture they do: a mode is dropped when every decoder that says its
/// largest picture (VAAPI's `VAConfigAttribMaxPictureWidth/Height`, Vulkan video's
/// `maxCodedExtent`) says a smaller one; one that says nothing keeps it.
pub fn packed_modes_that_fit(modes: u8, coded: (u32, u32), vaapi: Option<&VaapiIdentity>, vulkan: Option<&VulkanIdentity>) -> (u8, Option<(u32, u32)>) {
    use crate::media::Codec;
    let mut kept = modes;
    let mut largest = None;
    for (bit, codec, profile) in [(1, Codec::Hevc, 17), (2, Codec::Hevc, 18), (4, Codec::Av1, 32), (8, Codec::Av1, 32)] {
        if modes & bit == 0 {
            continue;
        }
        let vulkan_max = vulkan.filter(|vulkan| vulkan.enabled).and_then(|vulkan| vulkan.decode.iter().find(|&&(known, _)| known == codec)?.1);
        let maxima: Vec<(u32, u32)> = vaapi.and_then(|vaapi| vaapi.max_picture(profile)).into_iter().chain(vulkan_max).collect();
        if maxima.is_empty() || maxima.iter().any(|&(width, height)| coded.0 <= width && coded.1 <= height) {
            continue;
        }
        kept &= !bit;
        largest = maxima.into_iter().max_by_key(|&(width, height)| u64::from(width) * u64::from(height)).max(largest);
    }
    (kept, (kept != modes).then_some(largest).flatten())
}

/// The log line for packed modes `from` cut to `to` because the packed `coded` picture is above
/// the decoder's `max`, and the toast when none is left.
pub fn packed_too_large(from: u8, to: u8, coded: (u32, u32), max: (u32, u32)) -> (String, Option<String>) {
    let (coded, max) = (format!("{}x{}", coded.0, coded.1), format!("{}x{}", max.0, max.1));
    let line = format!("full chroma: the packed {coded} picture is larger than the decoder's {max}, packed modes {from:#x} -> {to:#x}");
    let toast = (to == 0).then(|| crate::strings::trf("Full chroma needs a {0} picture, larger than this decoder's {1}, so the stream is 4:2:0. A lower resolution keeps full chroma.", &[&coded, &max]));
    (line, toast)
}

/// Full chroma's packed `modes` for a `width` x `height` stream cut to what the decoders here
/// open (`packed_modes_that_fit`); logs a cut and returns the toast when nothing is left.
#[cfg(target_os = "linux")]
pub fn packed_offer(hardware: crate::media::Hardware, modes: u8, width: u32, height: u32) -> (u8, Option<String>) {
    let crate::media::Hardware::Auto(gpu, _) = hardware else { return (modes, None) };
    if modes == 0 {
        return (0, None);
    }
    let Some(coded) = rubylight_protocol::packed444::Geometry::new(width, height).map(|geometry| geometry.coded_size()) else { return (modes, None) };
    let vaapi = crate::media::vaapi_device(crate::media::vaapi_node(hardware).minor()).ok();
    let forced = crate::stream::env_flag(crate::linux_video::FORCE_VAAPI_ENVIRONMENT) == Some(true);
    // A skipped wrapper driver decodes nothing here, so its sizes do not count.
    let vaapi = vaapi.filter(|identity| !crate::linux_video::vaapi_plan(Ok(&identity.vendor), forced).skip);
    // SAFETY: The renderer keeps its device alive while streams use it.
    let vulkan = if gpu.is_null() { None } else { unsafe { crate::linux_video::vulkan_identity(gpu.cast()) }.ok() };
    let (kept, largest) = packed_modes_that_fit(modes, coded, vaapi.as_ref(), vulkan.as_ref());
    let Some(max) = largest else { return (kept, None) };
    let (line, toast) = packed_too_large(modes, kept, coded, max);
    eprintln!("nightjar: {line}");
    (kept, toast)
}

/// The present mode SDL's GPU renderer picks (`GPU_SetVSync`): FIFO with V-Sync; without it
/// MAILBOX, else IMMEDIATE, else FIFO, as the window supports them.
pub fn gpu_present_mode(vsync: bool, mailbox: bool, immediate: bool) -> &'static str {
    match (vsync, mailbox, immediate) {
        (false, true, _) => "MAILBOX",
        (false, false, true) => "IMMEDIATE",
        _ => "FIFO",
    }
}

#[cfg(target_os = "linux")]
static PRESENT: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

/// How the stream presents, for the `video path:` line: set by [`stream_start`].
#[cfg(target_os = "linux")]
pub fn present() -> String {
    let present = PRESENT.lock().unwrap_or_else(|e| e.into_inner());
    if present.is_empty() { "present mode unknown".to_owned() } else { present.clone() }
}

/// At a stream's start: logs the `linux:` line, keeps `present` for the `video path:` line, and
/// for hardware decoding logs what the machine is missing; returns the toasts for it.
#[cfg(target_os = "linux")]
pub fn stream_start(hardware: crate::media::Hardware, present: String) -> Vec<String> {
    let system = read_system();
    eprintln!("nightjar: {}", system.line());
    *PRESENT.lock().unwrap_or_else(|e| e.into_inner()) = present;
    let crate::media::Hardware::Auto(gpu, _) = hardware else { return Vec::new() };
    let vaapi = crate::media::vaapi_device(crate::media::vaapi_node(hardware).minor());
    // SAFETY: The renderer keeps its device alive while streams use it.
    let vulkan = if gpu.is_null() { None } else { unsafe { crate::linux_video::vulkan_identity(gpu.cast()) }.ok() };
    let forced = crate::stream::env_flag(crate::linux_video::FORCE_VAAPI_ENVIRONMENT) == Some(true);
    diagnose(&system, vaapi.as_ref().map_err(String::as_str), vulkan.as_ref(), forced)
        .into_iter()
        .map(|diagnosis| {
            eprintln!("nightjar: {}", diagnosis.line());
            diagnosis.toast()
        })
        .collect()
}

/// Reads the system: the files above and the session's environment.
#[cfg(target_os = "linux")]
pub fn read_system() -> System {
    let read = |path: &str| std::fs::read_to_string(path).ok();
    let (os_ids, os_name) = read("/etc/os-release").or_else(|| read("/usr/lib/os-release")).map(|text| parse_os_release(&text)).unwrap_or_default();
    let flatpak = read("/.flatpak-info").map(|text| parse_flatpak_runtime(&text));
    let gl_extensions = ["/usr/lib/x86_64-linux-gnu/GL", "/usr/lib/aarch64-linux-gnu/GL"]
        .iter()
        .filter_map(|dir| std::fs::read_dir(dir).ok())
        .flatten()
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .collect();
    let nvidia = read("/proc/driver/nvidia/version").and_then(|text| parse_nvidia_version(&text));
    let env = |name| std::env::var(name).unwrap_or_default();
    let session = [env("XDG_SESSION_TYPE"), env("XDG_CURRENT_DESKTOP")].into_iter().filter(|part| !part.is_empty()).collect::<Vec<_>>().join(" ");
    System { os_ids, os_name, flatpak, gl_extensions, nvidia, session }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linux_video::VaDecodeProfile;
    use crate::media::Codec;

    const FEDORA: &str = "NAME=\"Fedora Linux\"\nVERSION=\"41 (Workstation Edition)\"\nID=fedora\nVERSION_ID=41\nPRETTY_NAME=\"Fedora Linux 41 (Workstation Edition)\"\n";
    const BAZZITE: &str = "NAME=\"Bazzite\"\nID=bazzite\nID_LIKE=\"fedora\"\nPRETTY_NAME=\"Bazzite 41\"\n";
    const NVRM: &str = "NVRM version: NVIDIA UNIX x86_64 Kernel Module  550.54.14  Thu Feb 22 01:44:30 UTC 2024\nGCC version:  gcc version 13.2.1\n";
    const NVRM_OPEN: &str = "NVRM version: NVIDIA UNIX Open Kernel Module for x86_64  565.77  Release Build  (dvs-builder@U16-I3-B03-4-3)\n";

    fn vaapi(vendor: &str, profiles: &[i32]) -> VaapiIdentity {
        VaapiIdentity { node: Some(128), vendor: vendor.into(), profiles: profiles.iter().map(|&profile| VaDecodeProfile { profile, rt_format: 1, max: None }).collect() }
    }

    fn vulkan(decode: &[Codec]) -> VulkanIdentity {
        VulkanIdentity { device: "GPU".into(), driver: "d".into(), info: String::new(), decode: decode.iter().map(|&codec| (codec, None)).collect(), enabled: true }
    }

    #[test]
    fn the_system_files_are_read_into_the_linux_line() {
        assert_eq!(parse_os_release(FEDORA), (vec!["fedora".into()], "Fedora Linux 41 (Workstation Edition)".into()));
        assert_eq!(parse_os_release(BAZZITE).0, ["bazzite", "fedora"]);
        assert_eq!(parse_flatpak_runtime("[Application]\nname=io.github.ramazankara.nightjar\nruntime=runtime/org.freedesktop.Platform/x86_64/24.08\n"), "org.freedesktop.Platform/x86_64/24.08");
        assert_eq!(parse_nvidia_version(NVRM).as_deref(), Some("550.54.14"));
        assert_eq!(parse_nvidia_version(NVRM_OPEN).as_deref(), Some("565.77"));
        assert_eq!(parse_nvidia_version("garbage"), None);
        let system = System {
            os_name: "Fedora Linux 41 (Workstation Edition)".into(),
            flatpak: Some("org.freedesktop.Platform/x86_64/24.08".into()),
            nvidia: Some("550.54.14".into()),
            session: "wayland KDE".into(),
            ..System::default()
        };
        assert_eq!(system.line(), "linux: Fedora Linux 41 (Workstation Edition), Flatpak org.freedesktop.Platform/x86_64/24.08, NVIDIA driver 550.54.14, wayland KDE");
        assert_eq!(System::default().line(), "linux: unknown distribution");
    }

    #[test]
    fn fedoras_stock_mesa_without_h264_and_hevc_names_freeworld_or_the_flatpak() {
        let (os_ids, _) = parse_os_release(BAZZITE);
        let fedora = System { os_ids, ..System::default() };
        // Fedora's stock Mesa lists AV1 and VP9 only.
        let stock = vaapi("Mesa Gallium driver 24.3.4 for AMD Radeon 780M (radeonsi, gfx1103_r1, LLVM 19.1.7, DRM 3.59, 6.12.11)", &[32, 19]);
        assert_eq!(diagnose(&fedora, Ok(&stock), None, false), [Diagnosis::MesaWithoutCodecs { fedora: true }]);
        assert_eq!(diagnose(&System::default(), Ok(&stock), None, false), [Diagnosis::MesaWithoutCodecs { fedora: false }]);
        assert!(diagnose(&fedora, Ok(&stock), Some(&vulkan(&[Codec::H264, Codec::Hevc])), false).is_empty(), "Vulkan video decodes them instead");
        let freeworld = vaapi(&stock.vendor, &[7, 17, 18, 32]);
        assert!(diagnose(&fedora, Ok(&freeworld), None, false).is_empty());
    }

    #[test]
    fn nvidia_without_vulkan_video_or_its_flatpak_gl_extension_is_named() {
        let wrapper = vaapi("VA-API NVDEC driver [direct backend]", &[7, 17, 18]);
        let nvidia = System { nvidia: Some("550.54.14".into()), ..System::default() };
        assert_eq!(diagnose(&nvidia, Ok(&wrapper), Some(&vulkan(&[])), false), [Diagnosis::NvidiaWithoutVulkanVideo]);
        assert!(diagnose(&nvidia, Ok(&wrapper), Some(&vulkan(&[Codec::H264, Codec::Hevc, Codec::Av1])), false).is_empty());
        assert!(diagnose(&nvidia, Ok(&wrapper), None, true).is_empty(), "NIGHTJAR_FORCE_VAAPI=1 decodes with the wrapper");
        assert_eq!(diagnose(&nvidia, Err("no device"), Some(&vulkan(&[])), false), [Diagnosis::NvidiaWithoutVulkanVideo]);
        let flatpak = System { flatpak: Some(String::new()), gl_extensions: vec!["default".into()], ..nvidia.clone() };
        let missing = Diagnosis::FlatpakWithoutNvidiaGl { extension: "org.freedesktop.Platform.GL.nvidia-550-54-14".into() };
        assert_eq!(diagnose(&flatpak, Err("no device"), None, false), std::slice::from_ref(&missing));
        assert!(missing.line().contains("flatpak update"));
        assert!(missing.toast().contains("org.freedesktop.Platform.GL.nvidia-550-54-14"), "the toast names the extension to install");
        let installed = System { gl_extensions: vec!["default".into(), "nvidia-550-54-14".into()], ..flatpak };
        assert!(diagnose(&installed, Err("no device"), Some(&vulkan(&[Codec::Hevc])), false).is_empty());
    }

    #[test]
    fn packed_modes_above_every_decoders_largest_picture_are_dropped() {
        let profile = |profile, max| VaDecodeProfile { profile, rt_format: 0x101, max };
        // Gen9.5-like VAAPI: HEVC up to 8192x8192, AV1 not at all; 4K packed is 3840x4320.
        let vaapi = VaapiIdentity { node: None, vendor: "Intel iHD driver".into(), profiles: vec![profile(17, Some((8192, 8192))), profile(18, Some((4096, 2304)))] };
        let (kept, largest) = packed_modes_that_fit(15, (3840, 4320), Some(&vaapi), None);
        assert_eq!(kept, 1 | 4 | 8, "HEVC 10-bit dropped; AV1, whose limit nobody says, kept");
        assert_eq!(largest, Some((4096, 2304)));
        assert_eq!(packed_modes_that_fit(15, (1920, 2160), Some(&vaapi), None), (15, None), "1080p packed fits");
        // Vulkan video taking it is enough.
        let vulkan = VulkanIdentity { decode: vec![(Codec::Hevc, Some((8192, 8192)))], enabled: true, ..VulkanIdentity::default() };
        assert_eq!(packed_modes_that_fit(2, (3840, 4320), Some(&vaapi), Some(&vulkan)), (2, None));
        let disabled = VulkanIdentity { enabled: false, ..vulkan };
        assert_eq!(packed_modes_that_fit(2, (3840, 4320), Some(&vaapi), Some(&disabled)).0, 0, "not on the renderer's device");
        let (line, toast) = packed_too_large(15, 13, (3840, 4320), (4096, 2304));
        assert_eq!(line, "full chroma: the packed 3840x4320 picture is larger than the decoder's 4096x2304, packed modes 0xf -> 0xd");
        assert!(toast.is_none(), "AV1 still streams in full chroma");
        assert!(packed_too_large(2, 0, (3840, 4320), (4096, 2304)).1.unwrap().contains("3840x4320"));
    }

    #[test]
    fn the_gpu_renderer_presents_in_sdls_order() {
        assert_eq!(gpu_present_mode(true, true, true), "FIFO");
        assert_eq!(gpu_present_mode(false, true, true), "MAILBOX");
        assert_eq!(gpu_present_mode(false, false, true), "IMMEDIATE");
        assert_eq!(gpu_present_mode(false, false, false), "FIFO");
    }

    #[test]
    fn intel_i965_without_hevc_main10_names_intel_media_driver() {
        let i965 = vaapi("Intel i965 driver for Intel(R) Skylake - 2.4.1", &[7, 17]);
        assert_eq!(diagnose(&System::default(), Ok(&i965), None, false), [Diagnosis::IntelI965]);
        let kaby_lake = vaapi("Intel i965 driver for Intel(R) Kaby Lake - 2.4.1", &[7, 17, 18]);
        assert!(diagnose(&System::default(), Ok(&kaby_lake), None, false).is_empty());
        let ihd = vaapi("Intel iHD driver for Intel(R) Gen Graphics - 24.1.0 ()", &[7, 17]);
        assert!(diagnose(&System::default(), Ok(&ihd), None, false).is_empty());
    }
}
