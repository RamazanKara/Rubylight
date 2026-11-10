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

/// The `eglCreateImageKHR` attributes of layer `index` of an NV12 picture of `width` x `height`:
/// chroma is half size, rounded up as SDL sizes its chroma texture. The modifier goes along when
/// EGL takes modifiers and the driver named one (a tiled surface read as linear is garbage).
pub fn layer_attributes(descriptor: &Descriptor, index: usize, width: u32, height: u32, modifiers: bool) -> Vec<i32> {
    let layer = &descriptor.layers[index];
    let object = &descriptor.objects[layer.object_index[0] as usize];
    let (width, height) = if index == 0 { (width, height) } else { (width.div_ceil(2), height.div_ceil(2)) };
    let mut attributes = vec![
        EGL_LINUX_DRM_FOURCC_EXT, layer.drm_format as i32,
        EGL_WIDTH, width as i32,
        EGL_HEIGHT, height as i32,
        EGL_IMAGE_PRESERVED_KHR, EGL_TRUE,
        EGL_DMA_BUF_PLANE0_FD_EXT, object.fd,
        EGL_DMA_BUF_PLANE0_OFFSET_EXT, layer.offset[0] as i32,
        EGL_DMA_BUF_PLANE0_PITCH_EXT, layer.pitch[0] as i32,
    ];
    if modifiers && object.drm_format_modifier != DRM_FORMAT_MOD_INVALID {
        attributes.extend([
            EGL_DMA_BUF_PLANE0_MODIFIER_LO_EXT, (object.drm_format_modifier & 0xffff_ffff) as u32 as i32,
            EGL_DMA_BUF_PLANE0_MODIFIER_HI_EXT, (object.drm_format_modifier >> 32) as u32 as i32,
        ]);
    }
    attributes.push(EGL_NONE);
    attributes
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

#[cfg(unix)]
pub use linux::*;

#[cfg(unix)]
mod linux {
    use super::*;
    use crate::media::ffi::{av_buffer_unref, AVBufferRef, AVFrame, AVHWDeviceContext, AVHWFramesContext};
    use sdl3_sys::everything::*;
    use std::ffi::{c_char, c_int, c_uint, c_void, CStr};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, OnceLock};

    const VA_SURFACE_ATTRIB_MEM_TYPE_DRM_PRIME_2: u32 = 0x4000_0000;
    const VA_EXPORT_SURFACE_READ_ONLY: u32 = 0x0001;
    const VA_EXPORT_SURFACE_SEPARATE_LAYERS: u32 = 0x0004;
    const EGL_EXTENSIONS: c_int = 0x3055;
    const EGL_LINUX_DMA_BUF_EXT: c_uint = 0x3270;
    const GL_TEXTURE_2D: c_uint = 0x0DE1;
    const GL_TEXTURE_BINDING_2D: c_uint = 0x8069;

    type SyncSurface = unsafe extern "C" fn(*mut c_void, c_uint) -> c_int;
    type ExportSurfaceHandle = unsafe extern "C" fn(*mut c_void, c_uint, u32, u32, *mut c_void) -> c_int;

    /// libva's two calls, from the libva.so.2 FFmpeg's VAAPI device has loaded already.
    struct Libva {
        sync: SyncSurface,
        export: ExportSurfaceHandle,
        _library: libloading::Library,
    }

    fn libva() -> Result<&'static Libva, String> {
        static LIBVA: OnceLock<Result<Libva, String>> = OnceLock::new();
        LIBVA
            .get_or_init(|| unsafe {
                // SAFETY: libva's exported functions, with their va.h signatures.
                let library = libloading::Library::new("libva.so.2").map_err(|e| format!("cannot load libva.so.2: {e}"))?;
                let sync = *library.get::<SyncSurface>(b"vaSyncSurface\0").map_err(|e| format!("vaSyncSurface: {e}"))?;
                let export = *library.get::<ExportSurfaceHandle>(b"vaExportSurfaceHandle\0").map_err(|e| format!("vaExportSurfaceHandle: {e}"))?;
                Ok(Libva { sync, export, _library: library })
            })
            .as_ref()
            .map_err(Clone::clone)
    }

    /// A decoded surface, and its DMA-BUFs once exported. The decoder thread syncs the surface
    /// (`export`); the DMA-BUFs are exported there too when imports are not cached, else only when
    /// the presenter has no import of the surface yet (`descriptor`). The file descriptors are
    /// closed when it drops; the surface itself stays the decoder's frame's.
    pub struct Export {
        display: *mut c_void,
        surface: c_uint,
        /// The picture's frames context, borrowed from the frame this export belongs to.
        frames: *mut AVBufferRef,
        generation: Generation,
        exported: std::cell::OnceCell<Result<Descriptor, String>>,
        /// Set when the presenter cannot import: the decoder maps its pictures to memory from then on.
        decline: Arc<AtomicBool>,
    }

    impl Export {
        pub fn decline(&self) {
            self.decline.store(true, Ordering::Relaxed);
        }

        /// The surface's id in its pool.
        pub fn surface(&self) -> u32 {
            self.surface
        }

        /// Its pool and the picture size (`SurfaceCache`'s key).
        pub fn generation(&self) -> Generation {
            self.generation
        }

        /// The picture's frames context, valid while the picture is.
        pub fn frames(&self) -> *mut AVBufferRef {
            self.frames
        }

        /// The surface's DMA-BUFs, exported on first use (on the caller's thread: libva's
        /// drivers lock their own state, and moonlight-qt exports on its render thread too).
        pub fn descriptor(&self) -> Result<&Descriptor, String> {
            // SAFETY: the display and surface of the live picture this export belongs to.
            self.exported.get_or_init(|| unsafe { export_surface(self.display, self.surface) }).as_ref().map_err(Clone::clone)
        }
    }

    impl Drop for Export {
        fn drop(&mut self) {
            let Some(Ok(descriptor)) = self.exported.get() else { return };
            for object in &descriptor.objects[..(descriptor.num_objects as usize).min(4)] {
                // SAFETY: descriptors vaExportSurfaceHandle opened for this export alone.
                unsafe { libc::close(object.fd) };
            }
        }
    }

    unsafe fn export_surface(display: *mut c_void, surface: c_uint) -> Result<Descriptor, String> {
        let va = libva()?;
        let mut descriptor = Descriptor::default();
        let status = (va.export)(
            display,
            surface,
            VA_SURFACE_ATTRIB_MEM_TYPE_DRM_PRIME_2,
            VA_EXPORT_SURFACE_READ_ONLY | VA_EXPORT_SURFACE_SEPARATE_LAYERS,
            (&mut descriptor as *mut Descriptor).cast(),
        );
        if status != 0 {
            return Err(format!("vaExportSurfaceHandle failed ({status:#x})"));
        }
        Ok(descriptor)
    }

    /// Syncs a VAAPI frame's surface once its decoding is done (moonlight-qt syncs first too),
    /// and exports it unless imports are cached (`caching`): then the presenter exports it only
    /// when it has no import of the surface yet.
    ///
    /// # Safety
    /// `frame` is a live `AV_PIX_FMT_VAAPI` frame with its frames context, which outlives the
    /// export (the export goes with the frame).
    pub unsafe fn export(frame: *const AVFrame, decline: &Arc<AtomicBool>) -> Result<Export, String> {
        let va = libva()?;
        let frame = &*frame;
        if frame.hw_frames_ctx.is_null() {
            return Err("the picture has no VAAPI frames context".into());
        }
        let frames = &*((*frame.hw_frames_ctx).data.cast::<AVHWFramesContext>());
        let device: &AVHWDeviceContext = &*frames.device_ctx;
        // AVVAAPIDeviceContext begins with the VADisplay.
        let display = *device.hwctx.cast::<*mut c_void>();
        let surface = frame.data[3] as usize as c_uint;
        let status = (va.sync)(display, surface);
        if status != 0 {
            return Err(format!("vaSyncSurface failed ({status:#x})"));
        }
        let generation = Generation { frames: (*frame.hw_frames_ctx).data as usize, width: frame.width.max(0) as u32, height: frame.height.max(0) as u32 };
        let export = Export { display, surface, frames: frame.hw_frames_ctx, generation, exported: std::cell::OnceCell::new(), decline: decline.clone() };
        if !caching() {
            export.descriptor()?;
        }
        Ok(export)
    }

    unsafe extern "C" {
        fn av_buffer_ref(reference: *const AVBufferRef) -> *mut AVBufferRef;
    }

    /// A reference to a frames context, which keeps it (and its address) alive while a cache
    /// holds imports of its surfaces.
    pub struct FramesRef(*mut AVBufferRef);

    impl FramesRef {
        pub const fn none() -> Self {
            Self(std::ptr::null_mut())
        }

        /// # Safety
        /// `frames` is a live frames context reference.
        pub unsafe fn of(frames: *mut AVBufferRef) -> Self {
            Self(if frames.is_null() { frames } else { av_buffer_ref(frames) })
        }
    }

    impl Drop for FramesRef {
        fn drop(&mut self) {
            // SAFETY: our own reference, or null.
            unsafe { av_buffer_unref(&mut self.0) };
        }
    }

    /// Whether pictures can be imported into `renderer`'s textures: SDL's OpenGL renderer on an
    /// EGL context (Wayland, or X11 with `SDL_HINT_VIDEO_FORCE_EGL`).
    pub fn renderer_imports(renderer: *mut SDL_Renderer) -> bool {
        // SAFETY: SDL's EGL query on the renderer's thread.
        imports_into(&crate::sdl::renderer_name(renderer)) && !unsafe { SDL_EGL_GetCurrentDisplay() }.is_null()
    }

    /// The render node VAAPI opens on for `renderer` (`display_vaapi_node`), said in the log once
    /// per answer: on SDL's OpenGL renderer the GPU its EGL display runs on, when that is not the
    /// default device; none for every other renderer and that case. On the renderer's thread.
    pub fn renderer_vaapi_node(renderer: *mut SDL_Renderer) -> Option<u32> {
        if !imports_into(&crate::sdl::renderer_name(renderer)) {
            return None;
        }
        let (node, line) = display_vaapi_node(egl_render_node(), &render_nodes());
        crate::vulkan_video::note(&line);
        node
    }

    /// The render nodes in /dev/dri, lowest first: FFmpeg's VAAPI opens the first one by default.
    fn render_nodes() -> Vec<u32> {
        let mut nodes: Vec<u32> = std::fs::read_dir("/dev/dri")
            .map(|entries| entries.filter_map(|entry| render_node_minor(&entry.ok()?.file_name().to_string_lossy())).collect())
            .unwrap_or_default();
        nodes.sort_unstable();
        nodes
    }

    const EGL_DEVICE_EXT: c_int = 0x322C;
    const EGL_DRM_DEVICE_FILE_EXT: c_int = 0x3233;
    const EGL_DRM_RENDER_NODE_FILE_EXT: c_int = 0x3377;

    type QueryDisplayAttrib = unsafe extern "C" fn(*mut c_void, c_int, *mut isize) -> c_uint;
    type QueryDeviceString = unsafe extern "C" fn(*mut c_void, c_int) -> *const c_char;

    fn owned(text: *const c_char) -> Option<String> {
        // SAFETY (caller): EGL's static string or null.
        (!text.is_null()).then(|| unsafe { CStr::from_ptr(text) }.to_string_lossy().into_owned())
    }

    fn listed(extensions: &Option<String>, name: &str) -> bool {
        extensions.as_deref().is_some_and(|extensions| extensions.split_ascii_whitespace().any(|e| e == name))
    }

    /// The render node of the GPU the current EGL display runs on (`EGL_EXT_device_query`): the
    /// one its device names (`EGL_EXT_device_drm_render_node`), or the one beside the card node
    /// it names (`EGL_EXT_device_drm`, older drivers), found in sysfs.
    fn egl_render_node() -> Result<u32, String> {
        // SAFETY: SDL's EGL lookups on the renderer's thread; each pointer is cast to its Khronos
        // signature, and the device is the display's.
        unsafe {
            let display = SDL_EGL_GetCurrentDisplay();
            if display.is_null() {
                return Err("the renderer's context is not an EGL context".into());
            }
            let query: QueryString = function(SDL_EGL_GetProcAddress(c"eglQueryString".as_ptr()), "eglQueryString")?;
            // EGL_NO_DISPLAY: the client extensions.
            let client = owned(query(std::ptr::null_mut(), EGL_EXTENSIONS));
            if !listed(&client, "EGL_EXT_device_query") && !listed(&client, "EGL_EXT_device_base") {
                return Err("EGL does not name its devices (no EGL_EXT_device_query)".into());
            }
            let query_display: QueryDisplayAttrib = function(SDL_EGL_GetProcAddress(c"eglQueryDisplayAttribEXT".as_ptr()), "eglQueryDisplayAttribEXT")?;
            let query_device: QueryDeviceString = function(SDL_EGL_GetProcAddress(c"eglQueryDeviceStringEXT".as_ptr()), "eglQueryDeviceStringEXT")?;
            let mut device = 0isize;
            if query_display(display, EGL_DEVICE_EXT, &mut device) == 0 || device == 0 {
                return Err("EGL does not name the display's device".into());
            }
            let device = device as *mut c_void;
            let extensions = owned(query_device(device, EGL_EXTENSIONS));
            if listed(&extensions, "EGL_EXT_device_drm_render_node") {
                if let Some(path) = owned(query_device(device, EGL_DRM_RENDER_NODE_FILE_EXT)) {
                    return render_node_minor(&path).ok_or_else(|| format!("EGL names {path}, not a render node"));
                }
            }
            if listed(&extensions, "EGL_EXT_device_drm") {
                if let Some(card) = owned(query_device(device, EGL_DRM_DEVICE_FILE_EXT)) {
                    return card_render_node(&card);
                }
            }
            Err(format!("the display's EGL device names no DRM node ({})", extensions.as_deref().unwrap_or("no device extensions")))
        }
    }

    /// The render node of the GPU whose card node is `card` (`/dev/dri/card1`): its sysfs device
    /// lists both.
    fn card_render_node(card: &str) -> Result<u32, String> {
        let name = card.rsplit('/').next().unwrap_or(card);
        let directory = format!("/sys/class/drm/{name}/device/drm");
        let entries = std::fs::read_dir(&directory).map_err(|e| format!("EGL names {card}, and {directory} cannot be read: {e}"))?;
        entries
            .filter_map(|entry| render_node_minor(&entry.ok()?.file_name().to_string_lossy()))
            .min()
            .ok_or_else(|| format!("EGL names {card}, which has no render node in {directory}"))
    }

    type CreateImage = unsafe extern "C" fn(*mut c_void, *mut c_void, c_uint, *mut c_void, *const i32) -> *mut c_void;
    type DestroyImage = unsafe extern "C" fn(*mut c_void, *mut c_void) -> c_uint;
    type QueryString = unsafe extern "C" fn(*mut c_void, c_int) -> *const c_char;
    type ImageTargetTexture = unsafe extern "C" fn(c_uint, *mut c_void);
    type BindTexture = unsafe extern "C" fn(c_uint, c_uint);
    type GetIntegerv = unsafe extern "C" fn(c_uint, *mut c_int);
    type GetError = unsafe extern "C" fn() -> c_uint;

    /// The EGL and GL entry points the import needs, and the images bound to the texture now:
    /// cached per decoder surface (`SurfaceCache`), or without the cache only the last picture's.
    pub struct Importer {
        display: *mut c_void,
        create: CreateImage,
        destroy: DestroyImage,
        target: ImageTargetTexture,
        bind: BindTexture,
        get_integer: GetIntegerv,
        get_error: GetError,
        modifiers: bool,
        images: Vec<*mut c_void>,
        caching: bool,
        cache: SurfaceCache<[*mut c_void; 2]>,
        /// The frames context whose surfaces the cache holds images of, kept alive with it.
        frames: FramesRef,
        /// Pictures imported, and of them the ones whose images were cached, for the stream's log.
        imports: (u64, u64),
    }

    unsafe fn function<T>(pointer: SDL_FunctionPointer, name: &str) -> Result<T, String> {
        // SAFETY (caller): T is the function's pointer type.
        pointer.map(|f| std::mem::transmute_copy(&f)).ok_or_else(|| format!("no {name}"))
    }

    impl Importer {
        /// The importer of the current EGL context, on the renderer's thread.
        pub fn new() -> Result<Self, String> {
            // SAFETY: SDL's EGL and GL lookups on the thread whose context is current; each
            // pointer is cast to its Khronos signature.
            unsafe {
                let display = SDL_EGL_GetCurrentDisplay();
                if display.is_null() {
                    return Err("the renderer's context is not an EGL context".into());
                }
                let query: QueryString = function(SDL_EGL_GetProcAddress(c"eglQueryString".as_ptr()), "eglQueryString")?;
                let extensions = query(display, EGL_EXTENSIONS);
                let extensions = if extensions.is_null() { String::new() } else { CStr::from_ptr(extensions).to_string_lossy().into_owned() };
                let has = |name: &str| extensions.split_ascii_whitespace().any(|e| e == name);
                if !has("EGL_EXT_image_dma_buf_import") {
                    return Err("EGL cannot import DMA-BUFs (no EGL_EXT_image_dma_buf_import)".into());
                }
                if !SDL_GL_ExtensionSupported(c"GL_OES_EGL_image".as_ptr()) {
                    return Err("GL cannot take EGL images (no GL_OES_EGL_image)".into());
                }
                Ok(Self {
                    display,
                    create: function(SDL_EGL_GetProcAddress(c"eglCreateImageKHR".as_ptr()), "eglCreateImageKHR")?,
                    destroy: function(SDL_EGL_GetProcAddress(c"eglDestroyImageKHR".as_ptr()), "eglDestroyImageKHR")?,
                    target: function(SDL_GL_GetProcAddress(c"glEGLImageTargetTexture2DOES".as_ptr()), "glEGLImageTargetTexture2DOES")?,
                    bind: function(SDL_GL_GetProcAddress(c"glBindTexture".as_ptr()), "glBindTexture")?,
                    get_integer: function(SDL_GL_GetProcAddress(c"glGetIntegerv".as_ptr()), "glGetIntegerv")?,
                    get_error: function(SDL_GL_GetProcAddress(c"glGetError".as_ptr()), "glGetError")?,
                    modifiers: has("EGL_EXT_image_dma_buf_import_modifiers"),
                    images: Vec::new(),
                    caching: caching(),
                    cache: SurfaceCache::new(CACHE_LIMIT),
                    frames: FramesRef::none(),
                    imports: (0, 0),
                })
            }
        }

        /// The EGL images of `export`'s two planes: the surface's cached ones (made and cached on
        /// its first picture, the pool's earlier ones dropped when the pool changed), or without
        /// the cache new ones. True when they came from the cache.
        unsafe fn images_of(&mut self, export: &Export, width: u32, height: u32) -> Result<([*mut c_void; 2], bool), String> {
            if !self.caching {
                return Ok((self.create_images(export.descriptor()?, width, height)?, false));
            }
            let (changed, gone) = self.cache.retarget(export.generation());
            // Destroyed EGL images stay in a texture they were bound to until it takes others.
            for images in gone {
                self.destroy_images(images);
            }
            if changed {
                self.frames = FramesRef::of(export.frames());
            }
            if let Some(&images) = self.cache.get(export.surface()) {
                return Ok((images, true));
            }
            let images = self.create_images(export.descriptor()?, width, height)?;
            for gone in self.cache.insert(export.surface(), images) {
                self.destroy_images(gone);
            }
            Ok((images, false))
        }

        unsafe fn create_images(&self, descriptor: &Descriptor, width: u32, height: u32) -> Result<[*mut c_void; 2], String> {
            check_nv12_layers(descriptor)?;
            let mut images = [std::ptr::null_mut(); 2];
            for index in 0..2 {
                let attributes = layer_attributes(descriptor, index, width, height, self.modifiers);
                images[index] = (self.create)(self.display, std::ptr::null_mut(), EGL_LINUX_DMA_BUF_EXT, std::ptr::null_mut(), attributes.as_ptr());
                if images[index].is_null() {
                    self.destroy_images(images);
                    let layer = &descriptor.layers[index];
                    let modifier = descriptor.objects[layer.object_index[0] as usize].drm_format_modifier;
                    return Err(format!("eglCreateImageKHR refused layer {index} ({}, modifier {modifier:#x})", fourcc_name(layer.drm_format)));
                }
            }
            Ok(images)
        }

        unsafe fn destroy_images(&self, images: [*mut c_void; 2]) {
            for image in images.into_iter().filter(|image| !image.is_null()) {
                (self.destroy)(self.display, image);
            }
        }

        /// Binds the exported picture's planes to `texture`, an NV12 texture of SDL's OpenGL
        /// renderer (`imports_into`) made for imports (`SDL_RENDER_OPENGL_NV12_RG_SHADER`).
        ///
        /// # Safety
        /// On the renderer's thread, with its context current; `texture` is live.
        pub unsafe fn import(&mut self, renderer: *mut SDL_Renderer, texture: *mut SDL_Texture, export: &Export, width: u32, height: u32) -> Result<(), String> {
            let properties = SDL_GetTextureProperties(texture);
            let mut names = [
                SDL_GetNumberProperty(properties, SDL_PROP_TEXTURE_OPENGL_TEXTURE_NUMBER, 0),
                SDL_GetNumberProperty(properties, SDL_PROP_TEXTURE_OPENGL_TEXTURE_UV_NUMBER, 0),
            ];
            if names[0] != 0 {
                let target = SDL_GetNumberProperty(properties, SDL_PROP_TEXTURE_OPENGL_TEXTURE_TARGET_NUMBER, 0);
                if target != i64::from(GL_TEXTURE_2D) {
                    return Err(format!("SDL's texture target is {target:#x}, not GL_TEXTURE_2D"));
                }
            } else {
                names = [
                    SDL_GetNumberProperty(properties, SDL_PROP_TEXTURE_OPENGLES2_TEXTURE_NUMBER, 0),
                    SDL_GetNumberProperty(properties, SDL_PROP_TEXTURE_OPENGLES2_TEXTURE_UV_NUMBER, 0),
                ];
            }
            if names.contains(&0) {
                return Err("the video texture has no GL textures for its planes".into());
            }
            // SDL's queued drawing still samples the planes as they are; it goes to GL first,
            // before a pool change destroys the images it may sample.
            SDL_FlushRenderer(renderer);
            let (images, cached) = self.images_of(export, width, height)?;
            let mut bound = 0;
            (self.get_integer)(GL_TEXTURE_BINDING_2D, &mut bound);
            while (self.get_error)() != 0 {}
            for (name, image) in names.iter().zip(&images) {
                (self.bind)(GL_TEXTURE_2D, *name as c_uint);
                (self.target)(GL_TEXTURE_2D, *image);
            }
            let error = (self.get_error)();
            (self.bind)(GL_TEXTURE_2D, bound as c_uint);
            if !self.caching {
                // The textures hold the new images now; the previous ones can go.
                for image in std::mem::replace(&mut self.images, images.to_vec()) {
                    (self.destroy)(self.display, image);
                }
            }
            if error != 0 {
                return Err(format!("glEGLImageTargetTexture2DOES failed ({error:#x})"));
            }
            self.imports.0 += 1;
            self.imports.1 += u64::from(cached);
            Ok(())
        }
    }

    impl Drop for Importer {
        fn drop(&mut self) {
            if self.caching && self.imports.0 > 0 {
                eprintln!(
                    "nightjar: VAAPI pictures imported into OpenGL: {} of {} from the per-surface cache",
                    self.imports.1, self.imports.0
                );
            }
            let cached = self.cache.drain();
            // SAFETY: images this importer made on its display.
            unsafe {
                for images in cached {
                    self.destroy_images(images);
                }
                for image in self.images.drain(..) {
                    (self.destroy)(self.display, image);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nv12(modifier: u64) -> Descriptor {
        let mut descriptor = Descriptor { fourcc: fourcc(b"NV12"), width: 1920, height: 1088, num_objects: 1, num_layers: 2, ..Default::default() };
        descriptor.objects[0] = Object { fd: 7, size: 3_200_000, drm_format_modifier: modifier };
        descriptor.layers[0] = Layer { drm_format: DRM_FORMAT_R8, num_planes: 1, pitch: [2048, 0, 0, 0], ..Default::default() };
        descriptor.layers[1] = Layer { drm_format: DRM_FORMAT_GR88, num_planes: 1, offset: [2048 * 1088, 0, 0, 0], pitch: [2048, 0, 0, 0], ..Default::default() };
        descriptor
    }

    #[test]
    fn the_descriptor_has_libvas_layout() {
        // va_drmcommon.h: objects at 16, num_layers at 80, layers at 84, 312 bytes in all.
        assert_eq!(std::mem::size_of::<Object>(), 16);
        assert_eq!(std::mem::size_of::<Layer>(), 56);
        assert_eq!(std::mem::offset_of!(Descriptor, objects), 16);
        assert_eq!(std::mem::offset_of!(Descriptor, num_layers), 80);
        assert_eq!(std::mem::offset_of!(Descriptor, layers), 84);
        assert_eq!(std::mem::size_of::<Descriptor>(), 312);
        assert_eq!(DRM_FORMAT_R8, 0x2020_3852);
        assert_eq!(DRM_FORMAT_GR88, 0x3838_5247);
    }

    #[test]
    fn only_nv12_in_two_one_plane_layers_is_imported() {
        assert!(check_nv12_layers(&nv12(0)).is_ok());
        let mut composed = nv12(0);
        composed.num_layers = 1;
        composed.layers[0] = Layer { drm_format: fourcc(b"NV12"), num_planes: 2, ..Default::default() };
        assert!(check_nv12_layers(&composed).unwrap_err().contains("1 layers"));
        let mut p010 = nv12(0);
        p010.layers[0].drm_format = fourcc(b"R16 ");
        p010.layers[1].drm_format = fourcc(b"GR32");
        assert_eq!(check_nv12_layers(&p010).unwrap_err(), "layer 0 is R16 with 1 planes, not R8");
        let mut stray = nv12(0);
        stray.layers[1].object_index[0] = 1;
        assert_eq!(check_nv12_layers(&stray).unwrap_err(), "layer 1 names object 1 of 1");
    }

    #[test]
    fn layer_attributes_size_chroma_by_half_rounded_up_and_carry_the_modifier() {
        let tiled = nv12(0x0100_0000_0000_0002);
        let luma = layer_attributes(&tiled, 0, 1919, 1081, true);
        assert_eq!(
            luma,
            [0x3271, DRM_FORMAT_R8 as i32, 0x3057, 1919, 0x3056, 1081, 0x30D2, 1, 0x3272, 7, 0x3273, 0, 0x3274, 2048, 0x3443, 2, 0x3444, 0x0100_0000, 0x3038]
        );
        let chroma = layer_attributes(&tiled, 1, 1919, 1081, true);
        assert_eq!(&chroma[..14], [0x3271, DRM_FORMAT_GR88 as i32, 0x3057, 960, 0x3056, 541, 0x30D2, 1, 0x3272, 7, 0x3273, 2048 * 1088, 0x3274, 2048]);
        // Without the modifiers extension, or with no modifier named, the driver's layout is implied.
        assert_eq!(layer_attributes(&tiled, 0, 1920, 1080, false).len(), 15);
        assert_eq!(layer_attributes(&nv12(DRM_FORMAT_MOD_INVALID), 0, 1920, 1080, true).len(), 15);
    }

    #[test]
    fn only_the_opengl_renderer_takes_imports() {
        assert!(imports_into("opengl"));
        // GLES2 reads the RG-shader hint per draw: imported chroma would be read from alpha.
        for other in ["opengles2", "gpu", "vulkan", "software", "direct3d11"] {
            assert!(!imports_into(other), "{other}");
        }
    }

    #[test]
    fn render_nodes_are_read_from_paths_and_sysfs_names() {
        assert_eq!(render_node_minor("/dev/dri/renderD129"), Some(129));
        assert_eq!(render_node_minor("renderD128"), Some(128));
        for other in ["/dev/dri/card1", "card0", "controlD64", "renderD", "/dev/dri/renderDx", "by-path"] {
            assert_eq!(render_node_minor(other), None, "{other}");
        }
    }

    /// A hybrid PC (a user's Iris Xe at renderD128 beside an RX 9060 XT at renderD129, the
    /// monitor on the Radeon): VAAPI opened on FFmpeg's default device, the Intel GPU, and the
    /// Radeon's EGL refused every picture's Y-tiled layout (modifier 0x0100000000000002), so each
    /// one went through memory and the decoder fell behind above 60 fps.
    #[test]
    fn vaapi_opens_on_the_opengl_renderers_gpu_when_it_is_not_the_default_device() {
        let (node, line) = display_vaapi_node(Ok(129), &[128, 129]);
        assert_eq!(node, Some(129));
        assert!(line.contains("/dev/dri/renderD129") && line.contains("default device /dev/dri/renderD128"), "{line}");
        // The display on the integrated GPU: the default device already is the renderer's.
        let (node, line) = display_vaapi_node(Ok(128), &[128, 129]);
        assert_eq!(node, None);
        assert!(line.contains("default device /dev/dri/renderD128, the OpenGL renderer's GPU"), "{line}");
    }

    /// One GPU, AMD's or Intel's, is FFmpeg's default device: VAAPI opens as it always has.
    #[test]
    fn a_single_gpu_keeps_the_default_device() {
        for single in [[128], [129]] {
            let (node, line) = display_vaapi_node(Ok(single[0]), &single);
            assert_eq!(node, None, "{line}");
            assert!(line.contains("the OpenGL renderer's GPU"), "{line}");
        }
    }

    /// Unknown or unlisted, the display's GPU changes nothing, and the log says why.
    #[test]
    fn an_unknown_display_gpu_keeps_the_default_device_and_says_why() {
        let (node, line) = display_vaapi_node(Err("EGL does not name its devices (no EGL_EXT_device_query)".into()), &[128, 129]);
        assert_eq!(node, None);
        assert!(line.contains("unknown (EGL does not name its devices (no EGL_EXT_device_query))"), "{line}");
        let (node, line) = display_vaapi_node(Ok(130), &[128, 129]);
        assert_eq!(node, None);
        assert!(line.contains("/dev/dri/renderD130, is not among the render nodes here (/dev/dri/renderD128, /dev/dri/renderD129)"), "{line}");
        assert_eq!(display_vaapi_node(Ok(128), &[]).0, None, "no render node listed (a sandbox without /dev/dri)");
    }

    #[test]
    fn imports_are_cached_unless_the_environment_says_0() {
        assert!(cache_switched_on(None));
        assert!(cache_switched_on(Some("1".as_ref())));
        assert!(!cache_switched_on(Some("0".as_ref())));
    }

    const POOL: Generation = Generation { frames: 0x1000, width: 1920, height: 1080 };

    /// A surface the decoder hands out again finds its import; the pool's other surfaces get
    /// their own, and nothing is given back while the pool stays.
    #[test]
    fn a_surface_seen_again_finds_its_import() {
        let mut cache = SurfaceCache::new(CACHE_LIMIT);
        assert_eq!(cache.retarget(POOL), (true, vec![]), "the first pool is a change, with nothing to drop");
        assert_eq!(cache.get(7), None);
        assert!(cache.insert(7, "seven").is_empty());
        assert!(cache.insert(8, "eight").is_empty());
        assert_eq!(cache.retarget(POOL), (false, vec![]), "the same pool keeps its imports");
        assert_eq!(cache.get(7), Some(&"seven"));
        assert_eq!(cache.get(8), Some(&"eight"));
        assert_eq!(cache.insert(7, "seven again"), ["seven"], "a surface imported anew gives its old import back");
        assert_eq!(cache.len(), 2);
    }

    /// A new frames context (a decoder reset) or another picture size drops every import, as
    /// its surfaces are other buffers even where their ids repeat.
    #[test]
    fn another_pool_or_size_drops_every_import() {
        let mut cache = SurfaceCache::new(CACHE_LIMIT);
        cache.retarget(POOL);
        cache.insert(1, 'a');
        cache.insert(2, 'b');
        let (changed, mut gone) = cache.retarget(Generation { frames: 0x2000, ..POOL });
        gone.sort_unstable();
        assert!(changed);
        assert_eq!(gone, ['a', 'b']);
        assert_eq!(cache.get(1), None, "surface 1 of the new pool is another buffer");
        cache.insert(1, 'c');
        let (changed, gone) = cache.retarget(Generation { width: 1280, height: 720, frames: 0x2000 });
        assert!(changed && gone == ['c'], "another size: the imports were made for the old one");
        assert!(cache.drain().is_empty());
    }

    /// More surfaces than the limit: the oldest import goes, never the one just made.
    #[test]
    fn the_cache_keeps_at_most_its_limit_oldest_out_first() {
        let mut cache = SurfaceCache::new(2);
        cache.retarget(POOL);
        assert!(cache.insert(1, 1).is_empty());
        assert!(cache.insert(2, 2).is_empty());
        assert_eq!(cache.insert(3, 3), [1]);
        assert_eq!((cache.get(1), cache.get(2), cache.get(3)), (None, Some(&2), Some(&3)));
        let mut rest = cache.drain();
        rest.sort_unstable();
        assert_eq!(rest, [2, 3]);
        assert_eq!(cache.retarget(POOL), (false, vec![]), "draining keeps the pool");
    }

    #[test]
    fn zero_copy_is_on_unless_the_environment_says_0() {
        assert!(enabled_by_environment(None));
        assert!(enabled_by_environment(Some("1".as_ref())));
        assert!(!enabled_by_environment(Some("0".as_ref())));
    }
}
