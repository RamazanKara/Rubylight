//! Native spatial scaling of VideoToolbox surfaces. HDR stays on the existing renderer.
use crate::media::FrameInfo;
use sdl3_sys::everything::*;
use std::{ffi::c_void, ptr};

const RING: usize = 4;

/// SDL 3.4 exposes the renderer's shared `id<MTLCommandQueue>` under this property name.
const PROP_RENDERER_METAL_COMMAND_QUEUE_POINTER: &std::ffi::CStr =
    c"SDL.renderer.metal.command_queue";

unsafe extern "C" {
    fn nj_metalfx_supported() -> bool;
    fn nj_metalfx_new(
        layer: *mut c_void,
        shared_queue: *mut c_void,
        iw: i32,
        ih: i32,
        ow: i32,
        oh: i32,
    ) -> *mut c_void;
    fn nj_metalfx_draw(
        context: *mut c_void,
        input: *mut c_void,
        parameters: *const f32,
        gpu_ms: *mut f64,
    ) -> i32;
    fn nj_metalfx_slot_buffer(context: *mut c_void, slot: i32) -> *mut c_void;
    fn nj_metalfx_free(context: *mut c_void);
}

pub fn supported() -> bool {
    static SUPPORTED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *SUPPORTED.get_or_init(|| unsafe { nj_metalfx_supported() })
}

/// What decides, per present, whether MetalFX draws the picture.
pub struct Conditions {
    /// A creation or submission failed earlier in the stream.
    pub failed: bool,
    pub hdr: bool,
    /// Frame Warp runs for the stream (not this present's shift, which comes and goes).
    pub frame_warp: bool,
    /// SDL's native Metal renderer draws the stream; 4:4:4, PyroWave and similar formats take
    /// SDL's GPU renderer, which has no Metal layer.
    pub metal_renderer: bool,
    /// A VideoToolbox picture (not software decoding).
    pub hardware: bool,
    /// BT.2020 colour: SDL tags it as a YCbCr BT.2020 picture, and it has no RGB space that
    /// matches, so it keeps SDL's own conversion.
    pub bt2020: bool,
    pub input: (i32, i32),
    pub output: (i32, i32),
}

/// Why MetalFX does not draw the picture, as the overlay names it; None when it does.
pub fn blocker(c: &Conditions) -> Option<&'static str> {
    if c.failed { return Some("MetalFX unavailable"); }
    if c.hdr { return Some("HDR"); }
    if c.frame_warp { return Some("Frame Warp is on"); }
    // Before the renderer: software decoding of 10-bit SDR also takes SDL's GPU renderer.
    if !c.hardware { return Some("not a hardware-decoded picture"); }
    if !c.metal_renderer { return Some("4:4:4 or PyroWave needs SDL's GPU renderer"); }
    if c.bt2020 { return Some("BT.2020 colour"); }
    let (input, output) = (c.input, c.output);
    if output.0 < input.0 || output.1 < input.1 || output == input { return Some("no upscale needed"); }
    if output.0 > 16384 || output.1 > 16384 { return Some("output larger than 16384 pixels"); }
    None
}

pub struct Spatial {
    context: *mut c_void,
    slots: [*mut SDL_Texture; RING],
    current: Option<usize>,
    input: (i32, i32),
    output: (i32, i32),
    pub gpu_ms: f64,
}

impl Spatial {
    pub fn new(renderer: *mut SDL_Renderer, input: (i32, i32), output: (i32, i32)) -> Option<Self> {
        unsafe {
            let props = SDL_GetRendererProperties(renderer);
            let shared_queue = if props.0 != 0 {
                SDL_GetPointerProperty(
                    props,
                    PROP_RENDERER_METAL_COMMAND_QUEUE_POINTER.as_ptr(),
                    ptr::null_mut(),
                )
            } else {
                ptr::null_mut()
            };
            if shared_queue.is_null() {
                static WARNED: std::sync::Once = std::sync::Once::new();
                WARNED.call_once(|| {
                    eprintln!(
                        "nightjar: MetalFX: SDL Metal renderer did not expose {}; falling back to a private command queue (synchronous wait per frame)",
                        PROP_RENDERER_METAL_COMMAND_QUEUE_POINTER.to_string_lossy()
                    );
                });
            }
            let context = nj_metalfx_new(
                SDL_GetRenderMetalLayer(renderer),
                shared_queue,
                input.0,
                input.1,
                output.0,
                output.1,
            );
            (!context.is_null()).then_some(Self {
                context,
                slots: [ptr::null_mut(); RING],
                current: None,
                input,
                output,
                gpu_ms: 0.0,
            })
        }
    }
    pub fn matches(&self, input: (i32, i32), output: (i32, i32)) -> bool {
        self.input == input && self.output == output
    }

    /// The last scaled picture, which a present without a new picture draws again (`fresh` is
    /// false): the pass and its wait for the GPU would cost about five milliseconds and draw the
    /// same picture.
    pub fn output(&self) -> Option<*mut SDL_Texture> {
        self.current
            .map(|slot| self.slots[slot])
            .filter(|texture| !texture.is_null())
    }

    /// Submits MetalFX scaling into the next ring slot. When sharing SDL's command queue, this
    /// encodes asynchronously and retains `buffer` until GPU completion; otherwise it waits
    /// synchronously for the fallback private command queue.
    pub unsafe fn draw(
        &mut self,
        renderer: *mut SDL_Renderer,
        buffer: *mut c_void,
        info: &FrameInfo,
    ) -> Option<*mut SDL_Texture> {
        let parameters = conversion(info);
        // SAFETY: `buffer` is a valid `CVPixelBufferRef` (retained on the async shared queue until completion).
        unsafe {
            let slot = nj_metalfx_draw(self.context, buffer, parameters.as_ptr(), &mut self.gpu_ms);
            if slot < 0 || slot as usize >= RING {
                return None;
            }
            let index = slot as usize;
            if self.slots[index].is_null() {
                let output = nj_metalfx_slot_buffer(self.context, slot);
                if output.is_null() {
                    return None;
                }
                let props = SDL_CreateProperties();
                SDL_SetNumberProperty(
                    props,
                    SDL_PROP_TEXTURE_CREATE_FORMAT_NUMBER,
                    SDL_PIXELFORMAT_BGRA32.0.into(),
                );
                SDL_SetNumberProperty(
                    props,
                    SDL_PROP_TEXTURE_CREATE_WIDTH_NUMBER,
                    self.output.0.into(),
                );
                SDL_SetNumberProperty(
                    props,
                    SDL_PROP_TEXTURE_CREATE_HEIGHT_NUMBER,
                    self.output.1.into(),
                );
                SDL_SetNumberProperty(
                    props,
                    SDL_PROP_TEXTURE_CREATE_COLORSPACE_NUMBER,
                    SDL_COLORSPACE_SRGB.0.into(),
                );
                SDL_SetPointerProperty(
                    props,
                    SDL_PROP_TEXTURE_CREATE_METAL_PIXELBUFFER_POINTER,
                    output,
                );
                let texture = SDL_CreateTextureWithProperties(renderer, props);
                SDL_DestroyProperties(props);
                if texture.is_null() {
                    return None;
                }
                SDL_SetTextureBlendMode(texture, SDL_BLENDMODE_NONE);
                self.slots[index] = texture;
            }
            self.current = Some(index);
            Some(self.slots[index])
        }
    }
}

impl Drop for Spatial {
    fn drop(&mut self) {
        // Destroy pooled SDL textures before freeing `self.context`, whose ring owns their CVPixelBuffers.
        unsafe {
            for texture in self.slots {
                if !texture.is_null() {
                    SDL_DestroyTexture(texture);
                }
            }
            nj_metalfx_free(self.context);
        }
    }
}

// YUV -> nonlinear RGB, matching the frame's matrix/range: BT.601 or BT.709 (BT.2020 pictures
// keep SDL's path, `blocker`). P010 samples occupy the high ten bits of a normalized 16-bit
// texture (not a normalized ten-bit texture).
fn conversion(info: &FrameInfo) -> [f32; 8] {
    let (kr, kb) = match info.color_space {
        5 | 6 => (0.299, 0.114),
        _ => (0.2126, 0.0722),
    };
    let kg = 1.0 - kr - kb;
    let unit = if info.ten_bit != 0 {
        64.0 / 65535.0
    } else {
        1.0 / 255.0
    };
    let scale = if info.ten_bit != 0 { 4.0 } else { 1.0 };
    let (offset, yscale, cscale) = if info.color_range == 2 {
        (
            0.0,
            1.0 / ((256.0 * scale - 1.0) * unit),
            1.0 / ((256.0 * scale - 1.0) * unit),
        )
    } else {
        (
            16.0 * scale * unit,
            1.0 / (219.0 * scale * unit),
            1.0 / (224.0 * scale * unit),
        )
    };
    [
        offset,
        yscale,
        128.0 * scale * unit,
        cscale,
        2.0 * (1.0 - kr),
        -2.0 * kb * (1.0 - kb) / kg,
        -2.0 * kr * (1.0 - kr) / kg,
        2.0 * (1.0 - kb),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(color_space: i32, color_range: i32, ten_bit: i32) -> FrameInfo {
        FrameInfo { color_space, color_range, ten_bit, ..unsafe { std::mem::zeroed() } }
    }

    /// The kernel's luma: (sample - offset) * scale.
    fn luma(p: &[f32; 8], sample: f32) -> f32 { (sample - p[0]) * p[1] }
    fn chroma(p: &[f32; 8], sample: f32) -> f32 { (sample - p[2]) * p[3] }

    fn close(a: f32, b: f32) -> bool { (a - b).abs() < 1e-3 }

    #[test]
    fn eight_bit_video_and_full_range_map_black_white_and_neutral_chroma() {
        let video = conversion(&info(1, 1, 0));
        assert!(close(luma(&video, 16.0 / 255.0), 0.0) && close(luma(&video, 235.0 / 255.0), 1.0));
        assert!(close(chroma(&video, 128.0 / 255.0), 0.0) && close(chroma(&video, 240.0 / 255.0), 0.5));
        let full = conversion(&info(1, 2, 0));
        assert!(close(luma(&full, 0.0), 0.0) && close(luma(&full, 1.0), 1.0));
        assert!(close(chroma(&full, 128.0 / 255.0), 0.0));
    }

    #[test]
    fn p010_samples_sit_in_the_high_ten_bits_of_a_sixteen_bit_texture() {
        // A ten-bit value v arrives as (v << 6) / 65535.
        let sample = |v: f32| v * 64.0 / 65535.0;
        let video = conversion(&info(1, 1, 1));
        assert!(close(luma(&video, sample(64.0)), 0.0) && close(luma(&video, sample(940.0)), 1.0));
        assert!(close(chroma(&video, sample(512.0)), 0.0) && close(chroma(&video, sample(960.0)), 0.5));
        let full = conversion(&info(1, 2, 1));
        assert!(close(luma(&full, sample(0.0)), 0.0) && close(luma(&full, sample(1023.0)), 1.0));
    }

    #[test]
    fn matrices_are_bt601_for_its_spaces_and_bt709_otherwise() {
        // [Cr->R, Cb->G, Cr->G, Cb->B] of each standard.
        for (space, expected) in [(5, [1.402, -0.3441, -0.7141, 1.772]), (6, [1.402, -0.3441, -0.7141, 1.772]),
            (1, [1.5748, -0.1873, -0.4681, 1.8556]), (2, [1.5748, -0.1873, -0.4681, 1.8556])] {
            let p = conversion(&info(space, 1, 0));
            for (got, want) in p[4..].iter().zip(expected) {
                assert!(close(*got, want), "space {space}: {:?} against {expected:?}", &p[4..]);
            }
        }
    }

    fn drawable() -> Conditions {
        Conditions { failed: false, hdr: false, frame_warp: false, metal_renderer: true, hardware: true, bt2020: false,
            input: (1920, 1080), output: (2880, 1620) }
    }

    #[test]
    fn an_enlarged_hardware_sdr_picture_is_drawn_and_each_exclusion_names_itself() {
        assert_eq!(blocker(&drawable()), None);
        let named = |change: fn(&mut Conditions)| { let mut c = drawable(); change(&mut c); blocker(&c) };
        assert_eq!(named(|c| c.failed = true), Some("MetalFX unavailable"));
        assert_eq!(named(|c| c.hdr = true), Some("HDR"));
        assert_eq!(named(|c| c.frame_warp = true), Some("Frame Warp is on"));
        assert_eq!(named(|c| c.metal_renderer = false), Some("4:4:4 or PyroWave needs SDL's GPU renderer"));
        assert_eq!(named(|c| c.hardware = false), Some("not a hardware-decoded picture"));
        assert_eq!(named(|c| c.bt2020 = true), Some("BT.2020 colour"));
        // Software-decoded 10-bit SDR takes SDL's GPU renderer too; its decoding is the reason.
        assert_eq!(named(|c| { c.hardware = false; c.metal_renderer = false; }), Some("not a hardware-decoded picture"));
        assert_eq!(named(|c| c.output = c.input), Some("no upscale needed"));
        assert_eq!(named(|c| c.output = (1280, 720)), Some("no upscale needed"));
        assert_eq!(named(|c| c.output = (2880, 1000)), Some("no upscale needed"));
        assert_eq!(named(|c| c.output = (20000, 11250)), Some("output larger than 16384 pixels"));
    }

    #[test]
    fn vendored_sdl3_metal_patch_exposes_command_queue_property() {
        let patch = include_str!("../../../vendor/sdl3-src/patches/metal-command-queue.patch");
        let prop = PROP_RENDERER_METAL_COMMAND_QUEUE_POINTER.to_str().unwrap();
        assert!(patch.contains("src/render/metal/SDL_render_metal.m"));
        assert!(patch.contains(&format!("\"{prop}\"")), "patch must expose {prop}");
        assert!(patch.contains("data.mtlcmdqueue"));
    }
}
