//! VAAPI pictures without a CPU copy (Linux): the decoder thread exports each decoded surface as
//! DMA-BUFs, one layer per plane (`vaExportSurfaceHandle`), and the presenter makes an EGL image of
//! each layer and binds it to the GL texture of that plane in SDL's NV12 texture, which SDL then
//! draws as any other. Ported from moonlight-qt as Moonlight V+ ships it
//! (`EglImageFactory::exportVAImages`, `VAAPIRenderer::exportEGLImages`); SDL's testffmpeg binds the
//! images to SDL's own textures the same way. Either side failing falls back, logged once, to the
//! CPU mapping (`media::Frame::map_to_memory`).
#![cfg_attr(not(target_os = "linux"), allow(dead_code, reason = "DMA-BUF presentation is Linux-specific"))]

/// `VADRMPRIMESurfaceDescriptor` (va/va_drmcommon.h).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Descriptor {
    pub fourcc: u32,
    pub width: u32,
    pub height: u32,
    pub num_objects: u32,
    pub objects: [Object; 4],
    pub num_layers: u32,
    pub layers: [Layer; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Object {
    pub fd: i32,
    pub size: u32,
    pub drm_format_modifier: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Layer {
    pub drm_format: u32,
    pub num_planes: u32,
    pub object_index: [u32; 4],
    pub offset: [u32; 4],
    pub pitch: [u32; 4],
}

const fn fourcc(code: &[u8; 4]) -> u32 {
    u32::from_le_bytes(*code)
}

/// The one-plane layers SDL's NV12 texture holds: luma in R8, chroma in GR88 (red U, green V).
pub const DRM_FORMAT_R8: u32 = fourcc(b"R8  ");
pub const DRM_FORMAT_GR88: u32 = fourcc(b"GR88");
const DRM_FORMAT_MOD_INVALID: u64 = 0x00ff_ffff_ffff_ffff;

const EGL_NONE: i32 = 0x3038;
const EGL_WIDTH: i32 = 0x3057;
const EGL_HEIGHT: i32 = 0x3056;
const EGL_TRUE: i32 = 1;
const EGL_IMAGE_PRESERVED_KHR: i32 = 0x30D2;
const EGL_LINUX_DRM_FOURCC_EXT: i32 = 0x3271;
const EGL_DMA_BUF_PLANE0_FD_EXT: i32 = 0x3272;
const EGL_DMA_BUF_PLANE0_OFFSET_EXT: i32 = 0x3273;
const EGL_DMA_BUF_PLANE0_PITCH_EXT: i32 = 0x3274;
const EGL_DMA_BUF_PLANE0_MODIFIER_LO_EXT: i32 = 0x3443;
const EGL_DMA_BUF_PLANE0_MODIFIER_HI_EXT: i32 = 0x3444;

/// Checks that `descriptor` is an NV12 picture exported as separate layers: R8 luma, then GR88
/// chroma, one plane each, in objects it lists.
pub fn check_nv12_layers(descriptor: &Descriptor) -> Result<(), String> {
    if descriptor.num_layers != 2 || descriptor.num_objects == 0 || descriptor.num_objects > 4 {
        return Err(format!("the export has {} layers in {} objects, not NV12's two planes", descriptor.num_layers, descriptor.num_objects));
    }
    for (index, (layer, wanted)) in descriptor.layers.iter().zip([DRM_FORMAT_R8, DRM_FORMAT_GR88]).enumerate() {
        if layer.drm_format != wanted || layer.num_planes != 1 {
            return Err(format!(
                "layer {index} is {} with {} planes, not {}",
                fourcc_name(layer.drm_format),
                layer.num_planes,
                fourcc_name(wanted)
            ));
        }
        if layer.object_index[0] >= descriptor.num_objects {
            return Err(format!("layer {index} names object {} of {}", layer.object_index[0], descriptor.num_objects));
        }
    }
    Ok(())
}

/// Stack-allocated `eglCreateImageKHR` attribute list (at most 18 key/value ints plus `EGL_NONE`).
#[derive(Clone, Copy, Debug)]
pub struct LayerAttributes { values: [i32; 21], len: usize }
impl std::ops::Deref for LayerAttributes {
    type Target = [i32];
    fn deref(&self) -> &[i32] { &self.values[..self.len] }
}

/// The `eglCreateImageKHR` attributes of layer `index` of an NV12 picture of `width` x `height`:
/// chroma is half size, rounded up as SDL sizes its chroma texture. The modifier goes along when
/// EGL takes modifiers and the driver named one (a tiled surface read as linear is garbage).
pub fn layer_attributes(descriptor: &Descriptor, index: usize, width: u32, height: u32, modifiers: bool) -> LayerAttributes {
    let layer = &descriptor.layers[index];
    let object = &descriptor.objects[layer.object_index[0] as usize];
    let (w, h) = if index == 0 { (width, height) } else { (width.div_ceil(2), height.div_ceil(2)) };
    let mut values = [
        EGL_LINUX_DRM_FOURCC_EXT, layer.drm_format as i32, EGL_WIDTH, w as i32, EGL_HEIGHT, h as i32,
        EGL_IMAGE_PRESERVED_KHR, EGL_TRUE, EGL_DMA_BUF_PLANE0_FD_EXT, object.fd,
        EGL_DMA_BUF_PLANE0_OFFSET_EXT, layer.offset[0] as i32, EGL_DMA_BUF_PLANE0_PITCH_EXT, layer.pitch[0] as i32,
        EGL_NONE, 0, 0, 0, 0, 0, 0,
    ];
    let mut len = 14;
    if modifiers && object.drm_format_modifier != DRM_FORMAT_MOD_INVALID {
        values[14..18].copy_from_slice(&[
            EGL_DMA_BUF_PLANE0_MODIFIER_LO_EXT, (object.drm_format_modifier & 0xffff_ffff) as u32 as i32,
            EGL_DMA_BUF_PLANE0_MODIFIER_HI_EXT, (object.drm_format_modifier >> 32) as u32 as i32,
        ]);
        len = 18;
    }
    values[len] = EGL_NONE;
    LayerAttributes { values, len: len + 1 }
}

fn fourcc_name(code: u32) -> String {
    let bytes = code.to_le_bytes();
    if bytes.iter().all(|b| b.is_ascii_graphic() || *b == b' ') {
        String::from_utf8_lossy(&bytes).trim_end().to_owned()
    } else {
        format!("{code:#010x}")
    }
}

/// The SDL renderers whose NV12 textures take imported planes: only the OpenGL one, which picks
/// the red-green chroma shader (`SDL_RENDER_OPENGL_NV12_RG_SHADER`) once, when it makes the
/// texture. OpenGL ES reads that hint at every draw, so its imported chroma would be sampled as
/// luminance-alpha (alpha of GR88 is 1: a colour cast with no error).
pub fn imports_into(renderer_name: &str) -> bool {
    renderer_name == "opengl"
}

/// Zero-copy is on unless `NIGHTJAR_DMABUF=0` asks for the CPU mapping (A/B measurements).
pub fn enabled_by_environment(value: Option<&std::ffi::OsStr>) -> bool {
    value.is_none_or(|value| value != "0")
}

/// Set to 0, every VAAPI picture is exported (`vaExportSurfaceHandle`) and imported anew, as
/// before, instead of once per decoder surface: the way out should a driver mishandle imports
/// that outlive a picture. It covers the OpenGL import (`Importer`) and Vulkan's
/// (`vulkan_video::VaapiMap`).
pub const CACHE_ENVIRONMENT: &str = "NIGHTJAR_DMABUF_CACHE";

/// What `CACHE_ENVIRONMENT` asks for: imports kept per surface, unless 0.
pub fn cache_switched_on(value: Option<&std::ffi::OsStr>) -> bool {
    value.is_none_or(|value| value != "0")
}

/// `cache_switched_on` of the environment, read once.
pub fn caching() -> bool {
    static CACHING: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *CACHING.get_or_init(|| cache_switched_on(std::env::var_os(CACHE_ENVIRONMENT).as_deref()))
}

/// Which pool's surfaces a cache holds imports of: the frames context (its address, kept alive by
/// a reference the cache holds, so no other context can take it) and the picture size the
/// imports were made for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Generation {
    pub frames: usize,
    pub width: u32,
    pub height: u32,
}

/// Imports of a decoder's surfaces (`T`: EGL images, or Vulkan images and their memory), one per
/// surface id, for the surfaces of one frames context. They stay as long as the pool does, so a
/// surface the decoder hands out again is not exported or imported again; a new pool (a decoder
/// reset, another size) drops them all.
pub struct SurfaceCache<T> {
    generation: Option<Generation>,
    entries: Vec<(u32, T)>,
    limit: usize,
}

/// More entries than a pool has surfaces (FFmpeg's VAAPI pools are a few tens): the oldest go.
pub const CACHE_LIMIT: usize = 64;

impl<T> SurfaceCache<T> {
    pub fn new(limit: usize) -> Self {
        Self { generation: None, entries: Vec::new(), limit: limit.max(1) }
    }

    /// Makes the cache one of `generation`'s surfaces; the entries of another one come back, for
    /// the caller to destroy, with whether the generation changed (the caller swaps its frames
    /// context reference then).
    pub fn retarget(&mut self, generation: Generation) -> (bool, Vec<T>) {
        if self.generation == Some(generation) {
            return (false, Vec::new());
        }
        self.generation = Some(generation);
        (true, self.drain())
    }

    pub fn get(&self, surface: u32) -> Option<&T> {
        self.entries.iter().find(|(id, _)| *id == surface).map(|(_, entry)| entry)
    }

    /// Keeps `entry` for `surface`; what that surface held before and, over the limit, the oldest
    /// entries come back, for the caller to destroy.
    pub fn insert(&mut self, surface: u32, entry: T) -> Vec<T> {
        let mut gone: Vec<T> = Vec::new();
        if let Some(at) = self.entries.iter().position(|(id, _)| *id == surface) {
            gone.push(self.entries.remove(at).1);
        }
        while self.entries.len() >= self.limit {
            gone.push(self.entries.remove(0).1);
        }
        self.entries.push((surface, entry));
        gone
    }

    /// Everything, for the caller to destroy; the generation stays.
    pub fn drain(&mut self) -> Vec<T> {
        self.entries.drain(..).map(|(_, entry)| entry).collect()
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.len()
    }
}

/// The minor of DRM render node `name`, a device path or a sysfs entry (`/dev/dri/renderD129`,
/// `renderD129`); none for any other node.
pub fn render_node_minor(name: &str) -> Option<u32> {
    name.rsplit('/').next()?.strip_prefix("renderD")?.parse().ok()
}

/// Where VAAPI opens for the OpenGL renderer, and the log line that says so. FFmpeg's default
/// device is the first render node (`nodes`, those in /dev/dri), on a hybrid PC often the
/// integrated GPU, whose tiled pictures the other GPU's EGL refuses to import, so every picture
/// would go through memory. VAAPI opens on `display`, the render node of the GPU the renderer's
/// EGL display runs on, when that is another one than the default; otherwise on the default
/// device (none), as it always has: when it is the display's GPU (every single-GPU PC), or when
/// the display's node is unknown (`display`'s error) or not among `nodes`.
pub fn display_vaapi_node(display: Result<u32, String>, nodes: &[u32]) -> (Option<u32>, String) {
    let path = |minor: u32| format!("/dev/dri/renderD{minor}");
    match display {
        Err(why) => (None, format!("VAAPI opens on the default device: the OpenGL renderer's GPU is unknown ({why})")),
        Ok(minor) if !nodes.contains(&minor) => {
            let listed: Vec<_> = nodes.iter().map(|&node| path(node)).collect();
            (None, format!("VAAPI opens on the default device: the OpenGL renderer's GPU, {}, is not among the render nodes here ({})", path(minor), listed.join(", ")))
        }
        Ok(minor) if nodes.iter().min() == Some(&minor) => {
            (None, format!("VAAPI opens on the default device {}, the OpenGL renderer's GPU", path(minor)))
        }
        Ok(minor) => (
            Some(minor),
            format!(
                "VAAPI opens on {}, the OpenGL renderer's GPU, instead of the default device {} (another GPU), so its pictures import without a copy",
                path(minor),
                nodes.iter().min().map_or_else(String::new, |&default| path(default))
            ),
        ),
    }
}

// Linux and macOS (unix): DMA-BUF export/EGL import on Linux, VideoToolbox slot stub on macOS.
#[cfg(unix)]
mod linux;
// Linux and macOS (unix): re-export platform DMA-BUF export and importer types.
#[cfg(unix)]
pub use linux::*;

#[cfg(test)]
mod tests;
