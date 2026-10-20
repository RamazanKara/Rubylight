//! Showing decoded pictures: the video texture, and when each picture goes to the screen.
//!
//! Three pacing modes:
//! - host times (VRR): each picture is shown at the PC's frame time plus a steady delay
//!   ([`PresentClock`]), so the display refreshes at the game's cadence instead of the
//!   network's. Presents are immediate (tearing allowed) at the scheduled time.
//! - immediate: shown as soon as decoded (lowest latency; tears without VRR).
//! - vsync: the newest picture at each refresh.
//!
//! The D3D11 path copies the decoder's surface into the renderer's NV12 texture on the GPU
//! (as moonlight-qt's d3d11va renderer does when it cannot bind decoder surfaces directly);
//! other paths upload planes.

use std::collections::VecDeque;
use std::ffi::c_void;

use rubylight_protocol::present_clock::{PresentClock, Settings as ClockSettings};
use sdl3_sys::everything::*;

use super::pipeline::Decoded;
use crate::media::{self, FrameInfo};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PacingMode {
    HostTimes,
    Immediate,
    Vsync,
}

impl PacingMode {
    pub fn label(self) -> &'static str {
        match self {
            PacingMode::HostTimes => "VRR, PC frame times",
            PacingMode::Immediate => "Immediate",
            PacingMode::Vsync => "V-Sync",
        }
    }
}

/// A picture with its present time.
pub struct Pending {
    pub decoded: Decoded,
    pub present_us: u64,
    pub hold_us: i64,
    pub late: bool,
}

pub struct Pacer {
    pub mode: PacingMode,
    pub clock: PresentClock,
    pub pending: VecDeque<Pending>,
}

impl Pacer {
    /// `min_interval_us`: the display's shortest refresh interval.
    pub fn new(mode: PacingMode, min_interval_us: i64) -> Self {
        Self {
            mode,
            clock: PresentClock::new(ClockSettings {
                min_interval_us,
                ..Default::default()
            }),
            pending: VecDeque::new(),
        }
    }

    /// Takes newly decoded pictures, scheduling each once, in order.
    pub fn accept(&mut self, decoded: Decoded) {
        let (present_us, hold_us, late) = match self.mode {
            PacingMode::HostTimes => {
                let s = self.clock.schedule(decoded.rtp, decoded.ready_us);
                (s.present_us, s.hold_us, s.late)
            }
            _ => (decoded.ready_us, 0, false),
        };
        self.pending.push_back(Pending {
            decoded,
            present_us,
            hold_us,
            late,
        });
    }

    /// When the next picture is due, if one is waiting.
    pub fn next_due(&self) -> Option<u64> {
        self.pending.front().map(|p| p.present_us)
    }

    /// The newest picture due at `now_us`; older due ones are dropped (counted in the
    /// return's second value).
    pub fn take_due(&mut self, now_us: u64) -> (Option<Pending>, u32) {
        let mut chosen = None;
        let mut dropped = 0;
        while self.pending.front().is_some_and(|p| p.present_us <= now_us) {
            if chosen.is_some() {
                dropped += 1;
            }
            chosen = self.pending.pop_front();
        }
        (chosen, dropped)
    }
}

/// The renderer's texture holding the current picture.
pub struct VideoTexture {
    pub texture: *mut SDL_Texture,
    width: i32,
    height: i32,
    format: SDL_PixelFormat,
    colorspace: SDL_Colorspace,
    d3d11: *mut c_void,
}

impl Default for VideoTexture {
    fn default() -> Self {
        Self {
            texture: std::ptr::null_mut(),
            width: 0,
            height: 0,
            format: SDL_PIXELFORMAT_UNKNOWN,
            colorspace: SDL_COLORSPACE_UNKNOWN,
            d3d11: std::ptr::null_mut(),
        }
    }
}

/// FFmpeg's colour description as an SDL colorspace.
fn colorspace(info: &FrameInfo) -> SDL_Colorspace {
    // AVCOL_RANGE_JPEG = 2; AVCOL_SPC_BT709 = 1, BT470BG = 5, SMPTE170M = 6, BT2020_NCL = 9;
    // AVCOL_TRC_SMPTE2084 = 16.
    let full = info.color_range == 2;
    if info.color_trc == 16 {
        return SDL_COLORSPACE_HDR10;
    }
    match (info.color_space, full) {
        (9 | 10, false) => SDL_COLORSPACE_BT2020_LIMITED,
        (9 | 10, true) => SDL_COLORSPACE_BT2020_FULL,
        (5 | 6, false) => SDL_COLORSPACE_BT601_LIMITED,
        (5 | 6, true) => SDL_COLORSPACE_BT601_FULL,
        (_, true) => SDL_COLORSPACE_BT709_FULL,
        _ => SDL_COLORSPACE_BT709_LIMITED,
    }
}

impl VideoTexture {
    pub fn size(&self) -> (i32, i32) {
        (self.width, self.height)
    }

    fn ensure(
        &mut self,
        renderer: *mut SDL_Renderer,
        format: SDL_PixelFormat,
        width: i32,
        height: i32,
        space: SDL_Colorspace,
    ) -> bool {
        if !self.texture.is_null()
            && self.format == format
            && self.width == width
            && self.height == height
            && self.colorspace == space
        {
            return true;
        }
        self.destroy();
        // SAFETY: SDL property and texture calls on a live renderer.
        unsafe {
            let props = SDL_CreateProperties();
            SDL_SetNumberProperty(props, SDL_PROP_TEXTURE_CREATE_FORMAT_NUMBER, i64::from(format.0));
            SDL_SetNumberProperty(
                props,
                SDL_PROP_TEXTURE_CREATE_ACCESS_NUMBER,
                i64::from(SDL_TEXTUREACCESS_STATIC.0),
            );
            SDL_SetNumberProperty(props, SDL_PROP_TEXTURE_CREATE_WIDTH_NUMBER, i64::from(width));
            SDL_SetNumberProperty(props, SDL_PROP_TEXTURE_CREATE_HEIGHT_NUMBER, i64::from(height));
            SDL_SetNumberProperty(props, SDL_PROP_TEXTURE_CREATE_COLORSPACE_NUMBER, i64::from(space.0));
            self.texture = SDL_CreateTextureWithProperties(renderer, props);
            SDL_DestroyProperties(props);
            if self.texture.is_null() {
                return false;
            }
            SDL_SetTextureScaleMode(self.texture, SDL_SCALEMODE_LINEAR);
            self.d3d11 = SDL_GetPointerProperty(
                SDL_GetTextureProperties(self.texture),
                SDL_PROP_TEXTURE_D3D11_TEXTURE_POINTER,
                std::ptr::null_mut(),
            );
        }
        self.format = format;
        self.width = width;
        self.height = height;
        self.colorspace = space;
        true
    }

    /// Puts a decoded picture into the texture.
    pub fn upload(&mut self, renderer: *mut SDL_Renderer, frame: &media::Frame) -> bool {
        let info = frame.info();
        let space = colorspace(&info);
        let (w, h) = (info.width, info.height);
        // SAFETY: plane pointers and pitches come from the live AVFrame.
        unsafe {
            match info.kind {
                media::FRAME_D3D11 => {
                    let format = if info.ten_bit != 0 {
                        SDL_PIXELFORMAT_P010
                    } else {
                        SDL_PIXELFORMAT_NV12
                    };
                    self.ensure(renderer, format, w, h, space)
                        && !self.d3d11.is_null()
                        && frame.copy_to_d3d11(self.d3d11, w, h)
                }
                media::FRAME_NV12 | media::FRAME_P010 => {
                    let format = if info.kind == media::FRAME_P010 {
                        SDL_PIXELFORMAT_P010
                    } else {
                        SDL_PIXELFORMAT_NV12
                    };
                    self.ensure(renderer, format, w, h, space)
                        && SDL_UpdateNVTexture(
                            self.texture,
                            std::ptr::null(),
                            info.data[0],
                            info.linesize[0],
                            info.data[1],
                            info.linesize[1],
                        )
                }
                media::FRAME_YUV420P => {
                    self.ensure(renderer, SDL_PIXELFORMAT_IYUV, w, h, space)
                        && SDL_UpdateYUVTexture(
                            self.texture,
                            std::ptr::null(),
                            info.data[0],
                            info.linesize[0],
                            info.data[1],
                            info.linesize[1],
                            info.data[2],
                            info.linesize[2],
                        )
                }
                _ => false,
            }
        }
    }

    pub fn destroy(&mut self) {
        if !self.texture.is_null() {
            // SAFETY: a texture we created.
            unsafe { SDL_DestroyTexture(self.texture) };
        }
        *self = Self::default();
    }
}

/// The largest rectangle of the picture's aspect ratio inside `area`, centred.
pub fn letterbox(picture: (i32, i32), area: (i32, i32)) -> SDL_FRect {
    let (pw, ph) = (picture.0.max(1) as f32, picture.1.max(1) as f32);
    let (aw, ah) = (area.0 as f32, area.1 as f32);
    let scale = (aw / pw).min(ah / ph);
    let (w, h) = ((pw * scale).round(), (ph * scale).round());
    SDL_FRect {
        x: ((aw - w) / 2.0).floor(),
        y: ((ah - h) / 2.0).floor(),
        w,
        h,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letterbox_keeps_the_aspect_ratio() {
        let r = letterbox((1920, 1080), (2560, 1600));
        assert_eq!((r.x, r.y, r.w, r.h), (0.0, 80.0, 2560.0, 1440.0));
        let r = letterbox((1920, 1080), (1920, 1080));
        assert_eq!((r.x, r.y, r.w, r.h), (0.0, 0.0, 1920.0, 1080.0));
        let r = letterbox((1280, 1024), (1920, 1080));
        assert_eq!((r.w, r.h), (1350.0, 1080.0));
    }

    #[test]
    fn immediate_pacing_takes_the_newest_ready_picture() {
        let mut pacer = Pacer::new(PacingMode::Immediate, 0);
        assert!(pacer.next_due().is_none());
        let (none, dropped) = pacer.take_due(10);
        assert!(none.is_none() && dropped == 0);
    }
}
