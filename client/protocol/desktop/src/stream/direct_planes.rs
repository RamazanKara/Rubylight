//! Drawing a picture's planes (PyroWave's GPU decode, a software-decoded planar picture, a D3D11VA
//! 4:4:4 surface) straight into the back buffer on Direct3D 11, as Linux does (`planes_gpu.rs`
//! `draw_direct`): the plane shader converts and scales in the one draw SDL's back buffer takes,
//! instead of writing a full-size RGBA64_FLOAT texture which SDL then draws. The shader is
//! `planes_direct.hlsl` (appended to `pyrowave.hlsl` and `surface444.hlsl`); this module is its
//! constants, what SDL's draw of the texture would have applied (`Look`), and when the pictures
//! go this way. `NIGHTJAR_DIRECT_PLANES=0` keeps the texture.

use sdl3_sys::everything::*;

/// The switch's name.
pub const SWITCH: &str = "NIGHTJAR_DIRECT_PLANES";

/// Whether the planes may be drawn into the back buffer (the switch is not 0).
pub fn enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| wanted(std::env::var(SWITCH).ok().as_deref()))
}

fn wanted(setting: Option<&str>) -> bool {
    setting.map(str::trim) != Some("0")
}

/// How many plane sets the GPU PyroWave decoder gets for `queue_limit` pictures waiting: the
/// presenter holds the picture on screen in its set while it draws the planes straight into the
/// back buffer, so the ring has a set more.
pub fn plane_sets(queue_limit: usize) -> usize {
    queue_limit + 2 + usize::from(enabled())
}

/// What decides whether the current picture's planes are drawn into the back buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Conditions {
    /// `enabled()`.
    pub switch: bool,
    /// Drawing into the back buffer failed before: the stream keeps the texture.
    pub failed: bool,
    /// The renderer is Direct3D 11 and has a swap chain to take the draw.
    pub swap_chain: bool,
    /// Only some rows of the picture are drawn (a slice of the frame being received, which the
    /// raced presenter shows as it arrives): those go through the texture.
    pub partial: bool,
}

/// Whether a picture goes straight into the back buffer. The texture is only drawn later when
/// something reads it (an upscaler, AI enhancement, or SDL after a failed direct draw).
pub fn goes_direct(conditions: Conditions) -> bool {
    conditions.switch && !conditions.failed && conditions.swap_chain && !conditions.partial
}

/// The shader's constants (`Direct` in planes_direct.hlsl, register b1).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Constants {
    /// The rectangle the picture fills in back buffer pixels: x, y, width, height.
    pub destination: [f32; 4],
    /// The picture's visible size in the planes: x, y (z, w unused).
    pub picture: [f32; 4],
    pub output_scale: f32,
    /// SDL's tone map factors; 0 for no tone map.
    pub tonemap: [f32; 2],
    /// Bit 0: the output is sRGB, not scRGB.
    pub flags: u32,
    /// Frame Warp's row (`WarpDraw::row`); all zero draws the picture where it is.
    pub warp: [f32; 4],
    /// Its lens terms (`WarpDraw::lens_row`), read only beside a row that shifts.
    pub warp_lens: [f32; 4],
}

/// What SDL's draw of the RGBA64F linear picture texture into the back buffer does to it: its
/// tone map, when the texture's HDR headroom is above the output's, then its colour scale (the
/// output's SDR white times the renderer's colour scale), then on an sRGB output its sRGB
/// encoding and clamp. Read as `planes_gpu::SdlLook` reads it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    pub texture_headroom: f32,
    pub output_headroom: f32,
    pub sdr_white: f32,
    pub color_scale: f32,
    /// The output is scRGB (`SDL_COLORSPACE_SRGB_LINEAR`), not sRGB.
    pub linear_output: bool,
}

impl Look {
    /// What SDL's draw of `texture` into the renderer's back buffer applies.
    ///
    /// # Safety
    /// `renderer` and `texture` must be live, on the calling thread.
    pub unsafe fn of(renderer: *mut SDL_Renderer, texture: *mut SDL_Texture) -> Self {
        let props = SDL_GetRendererProperties(renderer);
        let mut color_scale = 1.0;
        SDL_GetRenderColorScale(renderer, &mut color_scale);
        Self {
            // SDL leaves a texture's headroom out when it is 0, its default for the linear format.
            texture_headroom: SDL_GetFloatProperty(SDL_GetTextureProperties(texture), SDL_PROP_TEXTURE_HDR_HEADROOM_FLOAT, 0.0),
            output_headroom: SDL_GetFloatProperty(props, SDL_PROP_RENDERER_HDR_HEADROOM_FLOAT, 0.0),
            sdr_white: SDL_GetFloatProperty(props, SDL_PROP_RENDERER_SDR_WHITE_POINT_FLOAT, 1.0),
            color_scale,
            linear_output: SDL_GetNumberProperty(props, SDL_PROP_RENDERER_OUTPUT_COLORSPACE_NUMBER, i64::from(SDL_COLORSPACE_SRGB.0)) == i64::from(SDL_COLORSPACE_SRGB_LINEAR.0),
        }
    }

    /// SDL's TONEMAP_CHROME factors (`CalculateAdvancedShaderConstants`), None without a tone map.
    pub fn tonemap(&self) -> Option<(f32, f32)> {
        (self.texture_headroom > self.output_headroom && self.output_headroom > 0.0)
            .then(|| (self.output_headroom / (self.texture_headroom * self.texture_headroom), 1.0 / self.output_headroom))
    }
}

/// The constants of a draw into `destination` (render coordinates) of a picture whose `visible`
/// part of the planes is shown.
pub fn constants(destination: &SDL_FRect, visible: (i32, i32), look: Look) -> Constants {
    let (tonemap1, tonemap2) = look.tonemap().unwrap_or((0.0, 0.0));
    Constants {
        destination: [destination.x, destination.y, destination.w.max(1.0), destination.h.max(1.0)],
        picture: [visible.0 as f32, visible.1 as f32, 0.0, 0.0],
        output_scale: look.sdr_white * look.color_scale,
        tonemap: [tonemap1, tonemap2],
        flags: u32::from(!look.linear_output),
        warp: [0.0; 4],
        warp_lens: nightjar_core::frame_warp::FLAT,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SDR: Look = Look { texture_headroom: 0.0, output_headroom: 1.0, sdr_white: 1.0, color_scale: 1.0, linear_output: false };

    #[test]
    fn the_constants_fill_the_shaders_cbuffer_exactly() {
        // Five float4 rows: destination, picture, then the scale, the two tone map factors and the
        // flags, then Frame Warp's two rows.
        assert_eq!(std::mem::size_of::<Constants>(), 80);
        assert_eq!(std::mem::offset_of!(Constants, picture), 16);
        assert_eq!(std::mem::offset_of!(Constants, output_scale), 32);
        assert_eq!(std::mem::offset_of!(Constants, tonemap), 36);
        assert_eq!(std::mem::offset_of!(Constants, flags), 44);
        assert_eq!(std::mem::offset_of!(Constants, warp), 48);
        assert_eq!(std::mem::offset_of!(Constants, warp_lens), 64);
        let shader = include_str!("../planes_direct.hlsl");
        assert!(shader.contains("float outputScale; float tonemap1; float tonemap2;"), "the shader's row 3 is scale, tonemap1, tonemap2, flags");
        let flags = shader.find("uint directFlags;").expect("the flags");
        let warp = shader.find("float4 warp;").expect("the warp row");
        assert!(flags < warp && !shader[flags + "uint directFlags;".len()..warp].contains(';'), "the warp row follows the flags");
        let lens = shader[warp..].trim_start_matches("float4 warp;").trim_start();
        assert!(lens.starts_with("float4 warpLens;") && lens.trim_start_matches("float4 warpLens;").trim_start().starts_with("};"),
            "the lens row follows it and ends the block");
        assert!(shader.contains("Texture2D<float> hudMask : register(t6);"));
        assert_eq!(constants(&SDL_FRect { x: 0.0, y: 0.0, w: 8.0, h: 8.0 }, (8, 8), SDR).warp, [0.0; 4], "no shift unless Frame Warp sets one");
    }

    #[test]
    fn eligibility_needs_the_switch_a_swap_chain_and_whole_pictures() {
        let all = Conditions { switch: true, failed: false, swap_chain: true, partial: false };
        assert!(goes_direct(all));
        assert!(!goes_direct(Conditions { switch: false, ..all }), "NIGHTJAR_DIRECT_PLANES=0 keeps the texture");
        assert!(!goes_direct(Conditions { failed: true, ..all }), "a failed draw keeps the texture for the stream");
        assert!(!goes_direct(Conditions { swap_chain: false, ..all }), "no Direct3D 11 swap chain, no direct draw");
        assert!(!goes_direct(Conditions { partial: true, ..all }), "the rows of a frame being received go through the texture");
    }

    #[test]
    fn the_switch_is_on_unless_it_is_zero() {
        assert!(wanted(None));
        assert!(wanted(Some("1")));
        assert!(wanted(Some("")));
        assert!(!wanted(Some("0")));
        assert!(!wanted(Some(" 0 ")));
    }

    #[test]
    fn an_sdr_srgb_output_encodes_and_an_scrgb_one_scales() {
        let rect = SDL_FRect { x: 10.0, y: 20.0, w: 1920.0, h: 1080.0 };
        let srgb = constants(&rect, (1920, 1080), SDR);
        assert_eq!(srgb.flags, 1);
        assert_eq!(srgb.destination, [10.0, 20.0, 1920.0, 1080.0]);
        assert_eq!(srgb.picture[..2], [1920.0, 1080.0]);
        assert_eq!(srgb.tonemap, [0.0, 0.0]);
        let scrgb = constants(&rect, (1920, 1080), Look { linear_output: true, sdr_white: 2.5, color_scale: 0.5, ..SDR });
        assert_eq!(scrgb.flags, 0);
        assert_eq!(scrgb.output_scale, 1.25);
    }

    #[test]
    fn the_tone_map_is_sdls() {
        let hdr = Look { texture_headroom: 12.0, output_headroom: 4.0, linear_output: true, ..SDR };
        assert_eq!(hdr.tonemap(), Some((4.0 / 144.0, 0.25)));
        assert_eq!(Look { output_headroom: 12.0, ..hdr }.tonemap(), None, "equal headroom needs no tone map");
        assert_eq!(Look { output_headroom: 0.0, ..hdr }.tonemap(), None, "SDL skips the tone map without an output headroom");
        assert_eq!(constants(&SDL_FRect { x: 0.0, y: 0.0, w: 0.0, h: 0.0 }, (2, 2), hdr).destination[2..], [1.0, 1.0], "an empty rectangle is one pixel");
    }

    #[test]
    fn the_ring_has_a_set_more_while_the_planes_are_drawn_at_present_time() {
        assert_eq!(plane_sets(2), if enabled() { 5 } else { 4 });
    }
}
