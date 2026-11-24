//! Linux VA-API DMA-BUF export (`vaExportSurfaceHandle`) and EGL/OpenGL texture import, plus the
//! macOS `Importer` VideoToolbox slot stub.

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
    // SAFETY: Caller guarantees `display` is a live `VADisplay` and `surface` belongs to it.
    let status = unsafe {
        (va.export)(
            display,
            surface,
            VA_SURFACE_ATTRIB_MEM_TYPE_DRM_PRIME_2,
            VA_EXPORT_SURFACE_READ_ONLY | VA_EXPORT_SURFACE_SEPARATE_LAYERS,
            (&mut descriptor as *mut Descriptor).cast(),
        )
    };
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
    // SAFETY: Caller guarantees `frame` is a live `AV_PIX_FMT_VAAPI` frame with its frames context.
    unsafe {
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
        // SAFETY: Caller guarantees `frames` is null or a live `AVBufferRef`.
        Self(if frames.is_null() { frames } else { unsafe { av_buffer_ref(frames) } })
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
        if listed(&extensions, "EGL_EXT_device_drm_render_node")
            && let Some(path) = owned(query_device(device, EGL_DRM_RENDER_NODE_FILE_EXT))
        {
            return render_node_minor(&path).ok_or_else(|| format!("EGL names {path}, not a render node"));
        }
        if listed(&extensions, "EGL_EXT_device_drm")
            && let Some(card) = owned(query_device(device, EGL_DRM_DEVICE_FILE_EXT))
        {
            return card_render_node(&card);
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

/// macOS holds its VideoToolbox `SDL_Texture` cache in `VideoTexture::importer` so `destroy()` drops it.
#[cfg(target_os = "macos")]
pub struct Importer { slot: Option<Box<dyn std::any::Any>> }
#[cfg(target_os = "macos")]
impl Importer {
    pub fn new() -> Result<Self, String> { Err("DMA-BUF EGL import is unavailable on macOS".into()) }
    /// # Safety
    /// Stub matching Linux `Importer::import`; never dereferences its arguments.
    pub unsafe fn import(&mut self, _: *mut SDL_Renderer, _: *mut SDL_Texture, _: &Export, _: u32, _: u32) -> Result<(), String> { Err("DMA-BUF EGL import is unavailable on macOS".into()) }
    pub fn macos_slot<T: 'static>(value: T) -> Self { Self { slot: Some(Box::new(value)) } }
    pub fn macos_slot_mut<T: 'static>(&mut self) -> Option<&mut T> { self.slot.as_deref_mut()?.downcast_mut() }
}

/// The EGL and GL entry points the import needs, and the images bound to the texture now:
/// cached per decoder surface (`SurfaceCache`), or without the cache only the last picture's.
#[cfg(target_os = "linux")]
pub struct Importer {
    display: *mut c_void, create: CreateImage, destroy: DestroyImage, target: ImageTargetTexture,
    bind: BindTexture, get_integer: GetIntegerv, get_error: GetError, modifiers: bool,
    images: Vec<*mut c_void>, caching: bool, cache: SurfaceCache<[*mut c_void; 2]>,
    /// The frames context whose surfaces the cache holds images of, kept alive with it.
    frames: FramesRef,
    /// Cached `(SDL_Texture ptr, [Y gl name, UV gl name])` to skip per-frame SDL property queries.
    gl_names: Option<(usize, [c_uint; 2])>,
    /// Pictures imported, and of them the ones whose images were cached, for the stream's log.
    imports: (u64, u64),
}

unsafe fn function<T>(pointer: SDL_FunctionPointer, name: &str) -> Result<T, String> {
    // SAFETY: Caller guarantees `T` matches the C function pointer signature of `pointer`.
    pointer.map(|f| unsafe { std::mem::transmute_copy(&f) }).ok_or_else(|| format!("no {name}"))
}

#[cfg(target_os = "linux")]
impl Importer {
    /// The importer of the current EGL context, on the renderer's thread.
    pub fn new() -> Result<Self, String> {
        // SAFETY: SDL's EGL and GL lookups on the thread whose context is current; each
        // pointer is cast to its Khronos signature.
        unsafe {
            let display = SDL_EGL_GetCurrentDisplay();
            if display.is_null() { return Err("the renderer's context is not an EGL context".into()); }
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
                images: Vec::new(), caching: caching(), cache: SurfaceCache::new(CACHE_LIMIT),
                frames: FramesRef::none(), gl_names: None, imports: (0, 0),
            })
        }
    }

    /// The EGL images of `export`'s two planes: the surface's cached ones (made and cached on
    /// its first picture, the pool's earlier ones dropped when the pool changed), or without
    /// the cache new ones. True when they came from the cache.
    unsafe fn images_of(&mut self, export: &Export, width: u32, height: u32) -> Result<([*mut c_void; 2], bool), String> {
        // SAFETY: Caller guarantees current EGL context and valid `export`.
        unsafe {
            if !self.caching { return Ok((self.create_images(export.descriptor()?, width, height)?, false)); }
            let (changed, gone) = self.cache.retarget(export.generation());
            // Destroyed EGL images stay in a texture they were bound to until it takes others.
            for images in gone { self.destroy_images(images); }
            if changed { self.frames = FramesRef::of(export.frames()); }
            if let Some(&images) = self.cache.get(export.surface()) { return Ok((images, true)); }
            let images = self.create_images(export.descriptor()?, width, height)?;
            for gone in self.cache.insert(export.surface(), images) { self.destroy_images(gone); }
            Ok((images, false))
        }
    }

    unsafe fn create_images(&self, descriptor: &Descriptor, width: u32, height: u32) -> Result<[*mut c_void; 2], String> {
        check_nv12_layers(descriptor)?;
        let mut images = [std::ptr::null_mut(); 2];
        // SAFETY: Caller guarantees current EGL context on `self.display` and open DMA-BUF fds in `descriptor`.
        unsafe {
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
        }
        Ok(images)
    }

    unsafe fn destroy_images(&self, images: [*mut c_void; 2]) {
        // SAFETY: Caller guarantees `images` were created on `self.display` and are unused.
        unsafe {
            for image in images.into_iter().filter(|image| !image.is_null()) { (self.destroy)(self.display, image); }
        }
    }

    /// Binds the exported picture's planes to `texture`, an NV12 texture of SDL's OpenGL
    /// renderer (`imports_into`) made for imports (`SDL_RENDER_OPENGL_NV12_RG_SHADER`).
    ///
    /// # Safety
    /// On the renderer's thread, with its context current; `texture` is live.
    pub unsafe fn import(&mut self, renderer: *mut SDL_Renderer, texture: *mut SDL_Texture, export: &Export, width: u32, height: u32) -> Result<(), String> {
        // SAFETY: Caller guarantees `renderer` and `texture` are live on this thread with EGL context current.
        unsafe {
            let names = match self.gl_names {
                Some((cached, names)) if cached == texture as usize => names,
                _ => {
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
                    if names.contains(&0) { return Err("the video texture has no GL textures for its planes".into()); }
                    let names = [names[0] as c_uint, names[1] as c_uint];
                    self.gl_names = Some((texture as usize, names));
                    names
                }
            };
            // SDL's queued drawing still samples the planes as they are; it goes to GL first,
            // before a pool change destroys the images it may sample.
            SDL_FlushRenderer(renderer);
            let (images, cached) = self.images_of(export, width, height)?;
            let mut bound = 0;
            (self.get_integer)(GL_TEXTURE_BINDING_2D, &mut bound);
            if !cached { while (self.get_error)() != 0 {} }
            for (&name, &image) in names.iter().zip(&images) {
                (self.bind)(GL_TEXTURE_2D, name);
                (self.target)(GL_TEXTURE_2D, image);
            }
            let error = if cached { 0 } else { (self.get_error)() };
            (self.bind)(GL_TEXTURE_2D, bound as c_uint);
            if !self.caching {
                // The textures hold the new images now; the previous ones can go.
                for image in std::mem::replace(&mut self.images, images.to_vec()) { (self.destroy)(self.display, image); }
            }
            if error != 0 { return Err(format!("glEGLImageTargetTexture2DOES failed ({error:#x})")); }
            self.imports.0 += 1;
            self.imports.1 += u64::from(cached);
            Ok(())
        }
    }
}

#[cfg(target_os = "linux")]
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
            for images in cached { self.destroy_images(images); }
            for image in self.images.drain(..) { (self.destroy)(self.display, image); }
        }
    }
}
