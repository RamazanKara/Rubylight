//! VAAPI pictures without a CPU copy (Linux): the decoder thread exports each decoded surface as
//! DMA-BUFs, one layer per plane (`vaExportSurfaceHandle`), and the presenter makes an EGL image of
//! each layer and binds it to the GL texture of that plane in SDL's NV12 texture, which SDL then
//! draws as any other. Ported from moonlight-qt as Moonlight V+ ships it
//! (`EglImageFactory::exportVAImages`, `VAAPIRenderer::exportEGLImages`); SDL's testffmpeg binds the
//! images to SDL's own textures the same way. Either side failing falls back, logged once, to the
//! CPU mapping (`media::Frame::map_to_memory`).
#![cfg_attr(not(unix), allow(dead_code))]

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

/// Zero-copy is on unless `NIGHTJAR_DMABUF=0` asks for the CPU mapping (A/B measurements).
pub fn enabled_by_environment(value: Option<&std::ffi::OsStr>) -> bool {
    value.is_none_or(|value| value != "0")
}

#[cfg(unix)]
pub use linux::*;

#[cfg(unix)]
mod linux {
    use super::*;
    use crate::media::ffi::{AVFrame, AVHWDeviceContext, AVHWFramesContext};
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

    /// A decoded surface's DMA-BUFs. The file descriptors are closed when it drops; the surface
    /// itself stays the decoder's frame's.
    pub struct Export {
        pub descriptor: Descriptor,
        /// Set when the presenter cannot import: the decoder maps its pictures to memory from then on.
        decline: Arc<AtomicBool>,
    }

    impl Export {
        pub fn decline(&self) {
            self.decline.store(true, Ordering::Relaxed);
        }
    }

    impl Drop for Export {
        fn drop(&mut self) {
            for object in &self.descriptor.objects[..(self.descriptor.num_objects as usize).min(4)] {
                // SAFETY: descriptors vaExportSurfaceHandle opened for this export alone.
                unsafe { libc::close(object.fd) };
            }
        }
    }

    /// Exports a VAAPI frame's surface once its decoding is done (moonlight-qt syncs first too).
    ///
    /// # Safety
    /// `frame` is a live `AV_PIX_FMT_VAAPI` frame with its frames context.
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
        Ok(Export { descriptor, decline: decline.clone() })
    }

    /// Whether pictures can be imported into `renderer`'s textures: SDL's OpenGL or OpenGL ES
    /// renderer on an EGL context (Wayland, or X11 with `SDL_HINT_VIDEO_FORCE_EGL`).
    pub fn renderer_imports(renderer: *mut SDL_Renderer) -> bool {
        // SAFETY: SDL's EGL query on the renderer's thread.
        matches!(crate::sdl::renderer_name(renderer).as_str(), "opengl" | "opengles2")
            && !unsafe { SDL_EGL_GetCurrentDisplay() }.is_null()
    }

    type CreateImage = unsafe extern "C" fn(*mut c_void, *mut c_void, c_uint, *mut c_void, *const i32) -> *mut c_void;
    type DestroyImage = unsafe extern "C" fn(*mut c_void, *mut c_void) -> c_uint;
    type QueryString = unsafe extern "C" fn(*mut c_void, c_int) -> *const c_char;
    type ImageTargetTexture = unsafe extern "C" fn(c_uint, *mut c_void);
    type BindTexture = unsafe extern "C" fn(c_uint, c_uint);
    type GetIntegerv = unsafe extern "C" fn(c_uint, *mut c_int);
    type GetError = unsafe extern "C" fn() -> c_uint;

    /// The EGL and GL entry points the import needs, and the images bound to the texture now.
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
                })
            }
        }

        /// Binds the exported picture's planes to `texture`, an NV12 texture of SDL's OpenGL (ES)
        /// renderer made for imports (`SDL_RENDER_OPENGL_NV12_RG_SHADER`).
        ///
        /// # Safety
        /// On the renderer's thread, with its context current; `texture` is live.
        pub unsafe fn import(&mut self, renderer: *mut SDL_Renderer, texture: *mut SDL_Texture, export: &Export, width: u32, height: u32) -> Result<(), String> {
            check_nv12_layers(&export.descriptor)?;
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
            let mut images = Vec::with_capacity(2);
            for index in 0..2 {
                let attributes = layer_attributes(&export.descriptor, index, width, height, self.modifiers);
                let image = (self.create)(self.display, std::ptr::null_mut(), EGL_LINUX_DMA_BUF_EXT, std::ptr::null_mut(), attributes.as_ptr());
                if image.is_null() {
                    for image in images {
                        (self.destroy)(self.display, image);
                    }
                    let layer = &export.descriptor.layers[index];
                    let modifier = export.descriptor.objects[layer.object_index[0] as usize].drm_format_modifier;
                    return Err(format!("eglCreateImageKHR refused layer {index} ({}, modifier {modifier:#x})", fourcc_name(layer.drm_format)));
                }
                images.push(image);
            }
            // SDL's queued drawing still samples the planes as they are; it goes to GL first.
            SDL_FlushRenderer(renderer);
            let mut bound = 0;
            (self.get_integer)(GL_TEXTURE_BINDING_2D, &mut bound);
            while (self.get_error)() != 0 {}
            for (name, image) in names.iter().zip(&images) {
                (self.bind)(GL_TEXTURE_2D, *name as c_uint);
                (self.target)(GL_TEXTURE_2D, *image);
            }
            let error = (self.get_error)();
            (self.bind)(GL_TEXTURE_2D, bound as c_uint);
            // The textures hold the new images now; the previous ones can go.
            for image in std::mem::replace(&mut self.images, images) {
                (self.destroy)(self.display, image);
            }
            if error != 0 {
                return Err(format!("glEGLImageTargetTexture2DOES failed ({error:#x})"));
            }
            Ok(())
        }
    }

    impl Drop for Importer {
        fn drop(&mut self) {
            for image in self.images.drain(..) {
                // SAFETY: images this importer made on its display.
                unsafe { (self.destroy)(self.display, image) };
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
    fn zero_copy_is_on_unless_the_environment_says_0() {
        assert!(enabled_by_environment(None));
        assert!(enabled_by_environment(Some("1".as_ref())));
        assert!(!enabled_by_environment(Some("0".as_ref())));
    }
}
