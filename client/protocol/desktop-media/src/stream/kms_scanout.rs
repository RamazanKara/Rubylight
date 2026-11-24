//! A stream's pictures straight on a display plane, drawing through KMS (`crate::kms`): each
//! decoded VA-API picture's DMA-BUFs become a framebuffer that an atomic commit puts on a plane
//! of the window's CRTC, with no copy and no draw. The overlay, the stream menu and notices go on
//! an ARGB8888 plane above it, painted by egui into memory with SDL's software renderer and copied
//! into one of two dumb buffers. The connector gets content type Game, `VRR_ENABLED` with the
//! stream's VRR setting, `max bpc` 10 for 10-bit, and for HDR the host's mastering metadata in
//! `HDR_OUTPUT_METADATA` with `Colorspace` BT2020_RGB; the picture plane gets its YCbCr encoding
//! and range. With V-Sync off a picture's commit is an async (tearing) flip where the kernel takes
//! one. Everything the stream changed is put back when it ends.
//!
//! It works on SDL's own DRM descriptor, on which SDL's OpenGL renderer holds DRM master; SDL
//! presents nothing while the planes show the stream. Where a step fails, the stream goes on
//! drawn by the renderer, and the log and a notice say why. Ported in its approach from
//! moonlight-qt's DRM renderer (moonlight-qt v6.2.0, GPL-3.0, Cameron Gutman and the Moonlight
//! contributors): SDL's descriptor, the plane and zpos choice, the connector properties.

use std::os::fd::RawFd;

use nightjar_core::control::HdrMetadata;
use sdl3_sys::everything::*;

use super::pipeline::Decoded;
use super::present::Pending;
use crate::kms::drm::{self, Card, DumbBuffer, Framebuffer, ModeInfo, Props};
use crate::kms::plan::{self, Layout, Object, Plane};

mod atomic;

/// The stream's scaling: a picture's size and the CRTC's to the picture's rect on it.
type Viewport<'a> = dyn Fn((i32, i32), (i32, i32)) -> (f32, f32, f32, f32) + 'a;

/// What a stream asks of the planes, from its settings.
#[derive(Clone, Copy, Debug)]
pub struct Wishes {
    /// V-Sync is off: pictures may tear in (async flips).
    pub tearing: bool,
    /// Tear-free VRR: a VRR stream's pictures are shown whole (`plan::may_tear`).
    pub tear_free: bool,
    /// The VRR setting.
    pub vrr: bool,
    /// The stream's frame rate, for the display mode.
    pub fps: u32,
}

/// Whether a stream on this renderer can go on planes: drawing through KMS, the Video planes
/// setting on, and an OpenGL renderer (SDL keeps DRM master; the GPU renderer's Vulkan display
/// takes it). Err says why not, for the log.
pub fn possible(video_planes: bool, renderer_name: &str) -> Result<(), String> {
    if !crate::kms::active() {
        return Err("Nightjar is not drawing straight to the display".into());
    }
    if !video_planes {
        return Err("the Video planes setting is off".into());
    }
    if !matches!(renderer_name, "opengl" | "opengles2") {
        return Err(format!("the {renderer_name:?} renderer has the display to itself (video planes need OpenGL)"));
    }
    Ok(())
}

/// What a stream's settings ask of the renderer that pictures on a plane would pass by: shader
/// upscaling, Frame Warp, and the 4:4:4 paths the renderer rebuilds or converts.
pub fn renderer_work(settings: &crate::config::Settings) -> Option<&'static str> {
    [
        // MetalFX is macOS's and never runs here: as Bilinear, as `kms::without_gpu` keeps it.
        (!matches!(settings.upscaling, crate::config::Upscaling::Bilinear | crate::config::Upscaling::MetalFx), "Upscaling"),
        (settings.frame_warp, "Frame Warp"),
        (settings.codec.is_pyrowave(), "PyroWave"),
        (settings.prefer_yuv444 || settings.experimental_packed_444, "Sharper colored text"),
    ]
    .into_iter()
    .find_map(|(on, what)| on.then_some(what))
}

/// Whether a stream with `settings` on this renderer goes on planes; Err says why not.
pub fn planned(settings: &crate::config::Settings, renderer_name: &str) -> Result<(), String> {
    possible(settings.video_planes, renderer_name)?;
    match renderer_work(settings) {
        Some(what) => Err(format!("{what} is drawn by the renderer")),
        None => Ok(()),
    }
}

/// SDL's DRM descriptor for `window` on the KMSDRM video driver.
fn sdl_fd(window: *mut SDL_Window) -> Option<RawFd> {
    // SAFETY: a live window and SDL's property name.
    let fd = unsafe { SDL_GetNumberProperty(SDL_GetWindowProperties(window), SDL_PROP_WINDOW_KMSDRM_DRM_FD_NUMBER, -1) };
    (fd >= 0).then_some(fd as RawFd)
}

/// The window's display size in pixels, as SDL has it.
fn display_size(window: *mut SDL_Window) -> (u32, u32) {
    // SAFETY: a live window; SDL returns a mode it owns, or null.
    unsafe {
        let mode = SDL_GetCurrentDisplayMode(SDL_GetDisplayForWindow(window));
        if mode.is_null() { (0, 0) } else { ((*mode).w.max(0) as u32, (*mode).h.max(0) as u32) }
    }
}

/// The card and the window's output on it, with the CRTC's mode and the connector's modes.
fn open_card(window: *mut SDL_Window) -> Result<(Card, plan::Output, ModeInfo, Vec<ModeInfo>), String> {
    let fd = sdl_fd(window).ok_or("SDL gives no DRM descriptor for the window")?;
    let card = Card::new(fd)?;
    let outputs = card.outputs().map_err(|e| format!("the outputs cannot be read: {e}"))?;
    let list: Vec<plan::Output> = outputs.iter().map(|(o, ..)| o.clone()).collect();
    let (w, h) = display_size(window);
    let chosen = plan::pick_output(&list, w, h)?.connector_id;
    let (output, mode, modes) = outputs.into_iter().find(|(o, ..)| o.connector_id == chosen).ok_or("the output went away")?;
    Ok((card, output, mode, modes))
}

/// The connector's HDR: it takes `HDR_OUTPUT_METADATA` and BT.2020, and the display's EDID says
/// it takes PQ. For the HDR check before a stream; Err says why not.
pub fn display_hdr(window: *mut SDL_Window) -> Result<(), String> {
    let (card, output, ..) = open_card(window)?;
    let connector = card.props(output.connector_id, drm::OBJECT_CONNECTOR).map_err(|e| e.to_string())?;
    let caps = caps_of(&connector, &Props::default());
    if !caps.hdr() {
        return Err(format!("{} has no HDR output (HDR_OUTPUT_METADATA and BT2020 colorimetry)", output.name));
    }
    let edid = connector.value("EDID").filter(|&id| id != 0).and_then(|id| card.blob(id as u32).ok()).unwrap_or_default();
    if !plan::edid_supports_pq(&edid) {
        return Err(format!("the display on {} does not say it takes HDR10 (PQ)", output.name));
    }
    Ok(())
}

fn caps_of(connector: &Props, crtc: &Props) -> plan::Caps {
    plan::Caps {
        vrr_enabled: crtc.has("VRR_ENABLED"),
        vrr_capable: connector.value("vrr_capable") == Some(1),
        content_type: connector.enum_value("content type", "Game").is_some(),
        hdr_metadata: connector.has("HDR_OUTPUT_METADATA"),
        colorspace_bt2020: connector.enum_value("Colorspace", "BT2020_RGB").is_some(),
        max_bpc: connector.get("max bpc").and_then(|(info, value)| info.range.map(|(_, top)| (top, *value))),
    }
}

/// The overlay plane's two buffers and which one is on the screen.
struct OverlayPlane {
    plane: u32,
    buffers: Vec<DumbBuffer>,
    front: usize,
    shown: bool,
    /// What the next commit does to it: Some(true) shows the other buffer, Some(false) hides it.
    change: Option<bool>,
    /// Last pixels copied into the overlay dumb buffer, so unchanged frames skip the WC memcpy
    /// and leave `only_fb = true` for async flips.
    last_canvas: Vec<u8>,
}

/// The card's state the stream started from: its planes, connector and CRTC with their values.
struct Snapshot {
    planes: Vec<(Plane, Props)>,
    connector: Props,
    crtc: Props,
    mode: ModeInfo,
    modes: Vec<ModeInfo>,
}

/// A stream on planes, once its first picture went up.
struct Engaged {
    snapshot: Snapshot,
    layout: Layout,
    format: (u32, u64),
    generation: Option<crate::dmabuf::Generation>,
    /// Framebuffers and cached layouts by VA surface; old pool's wait in `retired` for the next commit.
    fbs: Vec<(u32, Framebuffer, plan::FbLayout)>,
    retired: Vec<Framebuffer>,
    /// The framebuffer the next commit shows, and the one on the screen.
    next_fb: u32,
    shown_fb: u32,
    rect: plan::PlaneRect,
    shown_rect: Option<plan::PlaneRect>,
    /// The picture on the screen, kept from the decoder's pool until another replaces it.
    on_screen: Option<Pending>,
    overlay: OverlayPlane,
    background: Option<DumbBuffer>,
    hdr: Option<HdrMetadata>,
    /// Moonlight's colour space and range of the pictures.
    color: (i32, i32),
    blobs: Vec<u32>,
    /// Each property the stream set, with the value it had (put back at the end).
    restore: Vec<(u32, u32, u64)>,
    /// The stream's properties must go out again (HDR changed).
    props_dirty: bool,
    /// The stream's VRR is on at this display (`plan::vrr_engaged`).
    vrr_on: bool,
}

pub struct Scanout {
    card: Card,
    output: plan::Output,
    wishes: Wishes,
    async_cap: bool,
    async_failed: bool,
    commit_times: plan::CommitTimes,
    /// What a picture's commit is made of and handed the kernel as, kept from commit to commit.
    commit_sets: Vec<(u32, u32, u64)>,
    commit_buffers: plan::AtomicBuffers,
    /// The software renderer and its canvas the overlay is painted on.
    canvas: *mut SDL_Surface,
    overlay_renderer: *mut SDL_Renderer,
    engaged: Option<Engaged>,
    off: Option<String>,
}

impl Scanout {
    /// A stream's planes, armed: they take the stream from its first picture (`prepare`). None
    /// after a log line when they cannot be used.
    pub fn open(window: *mut SDL_Window, renderer_name: &str, settings: &crate::config::Settings, wishes: Wishes) -> Option<Self> {
        if let Err(why) = planned(settings, renderer_name) {
            if crate::kms::active() {
                eprintln!("nightjar: kms: the stream is drawn by the renderer: {why}");
            }
            return None;
        }
        match Self::try_open(window, wishes) {
            Ok(scanout) => Some(scanout),
            Err(why) => {
                eprintln!("nightjar: kms: video planes unavailable, the stream is drawn by the renderer: {why}");
                None
            }
        }
    }

    fn try_open(window: *mut SDL_Window, wishes: Wishes) -> Result<Self, String> {
        let (card, output, mode, _) = open_card(window)?;
        let async_cap = card.atomic_async();
        let (w, h) = (u32::from(mode.hdisplay), u32::from(mode.vdisplay));
        // SAFETY: a new surface of SDL's own and a software renderer on it, both freed in Drop.
        let (canvas, overlay_renderer) = unsafe {
            let canvas = SDL_CreateSurface(w as i32, h as i32, SDL_PIXELFORMAT_ARGB8888);
            if canvas.is_null() {
                return Err(format!("no overlay canvas: {}", crate::sdl::error()));
            }
            let renderer = SDL_CreateSoftwareRenderer(canvas);
            if renderer.is_null() {
                SDL_DestroySurface(canvas);
                return Err(format!("no overlay renderer: {}", crate::sdl::error()));
            }
            (canvas, renderer)
        };
        eprintln!(
            "nightjar: kms: video planes armed on {} ({w}x{h} at {:.2} Hz), async flips {}",
            output.name,
            mode.refresh_hz(),
            if async_cap { "available" } else { "unavailable" }
        );
        Ok(Self { card, output, wishes, async_cap, async_failed: false, commit_times: plan::CommitTimes::default(), commit_sets: Vec::new(), commit_buffers: plan::AtomicBuffers::default(), canvas, overlay_renderer, engaged: None, off: None })
    }

    /// The planes show the stream.
    pub fn engaged(&self) -> bool {
        self.engaged.is_some() && self.off.is_none()
    }

    /// The planes may still take the stream.
    pub fn usable(&self) -> bool {
        self.off.is_none()
    }

    /// Readies `decoded` for the next commit, engaging on the first picture. A picture that is
    /// not a VA-API DMA-BUF export, or fails, gives the planes up (the stream goes back to the
    /// renderer) and says why.
    pub fn prepare(&mut self, decoded: &Decoded, hdr: Option<HdrMetadata>, viewport: impl Fn((i32, i32), (i32, i32)) -> (f32, f32, f32, f32)) -> Result<(), String> {
        if let Some(why) = &self.off {
            return Err(why.clone());
        }
        let result = self.prepare_inner(decoded, hdr, &viewport);
        if let Err(why) = &result {
            self.give_up(why);
        }
        result
    }

    fn export_layout(export: &crate::dmabuf::Export) -> Result<(plan::FbLayout, [i32; 4], usize), String> {
        let descriptor = export.descriptor()?;
        let layout = plan::fb_layout(descriptor)?;
        let count = (descriptor.num_objects as usize).min(4);
        let mut fds = [0i32; 4];
        for (dst, obj) in fds[..count].iter_mut().zip(&descriptor.objects[..count]) {
            *dst = obj.fd;
        }
        Ok((layout, fds, count))
    }

    fn prepare_inner(&mut self, decoded: &Decoded, hdr: Option<HdrMetadata>, viewport: &Viewport<'_>) -> Result<(), String> {
        let frame = decoded.picture.frame().ok_or("the picture is not a decoded frame")?;
        let export = frame.dmabuf().ok_or("the picture is not a VA-API DMA-BUF export")?;
        let info = frame.info();
        let visible = decoded.visible.unwrap_or((info.width, info.height));
        let (surface, generation) = (export.surface(), export.generation());
        let color = (decoded.setup.color_space, decoded.setup.color_range);
        if self.engaged.is_none() {
            let (layout, fds, n) = Self::export_layout(export)?;
            if !(layout.modifier == plan::MOD_INVALID || layout.modifier == plan::MOD_LINEAR || self.card.modifiers()) {
                return Err("the card takes no framebuffer modifiers".into());
            }
            let fb = self.card.add_dmabuf_fb(&layout, &fds[..n]).map_err(|e| format!("the first picture's framebuffer cannot be made: {e}"))?;
            return self.engage(layout, fb, (surface, generation), visible, (hdr, color), viewport);
        }
        let card = &self.card;
        let engaged = self.engaged.as_mut().expect("engaged");
        // A new pool: its surfaces are other buffers.
        if engaged.generation != Some(generation) {
            engaged.retired.extend(engaged.fbs.drain(..).map(|(_, fb, _)| fb));
            engaged.generation = Some(generation);
        }
        let (fb, rect) = if let Some((_, fb, layout)) = engaged.fbs.iter().find(|(s, ..)| *s == surface) {
            (fb.id, picture_rect(&engaged.snapshot.mode, layout, visible, viewport))
        } else {
            let (layout, fds, n) = Self::export_layout(export)?;
            if engaged.format != (layout.format, layout.modifier) {
                return Err(format!("the pictures changed to {} with modifier {:#x}, which the plane was not chosen for", plan::fourcc_name(layout.format), layout.modifier));
            }
            let fb = card.add_dmabuf_fb(&layout, &fds[..n]).map_err(|e| format!("a picture's framebuffer cannot be made: {e}"))?;
            let (id, rect) = (fb.id, picture_rect(&engaged.snapshot.mode, &layout, visible, viewport));
            engaged.fbs.push((surface, fb, layout));
            (id, rect)
        };
        engaged.next_fb = fb;
        engaged.rect = rect;
        if (engaged.hdr, engaged.color) != (hdr, color) {
            (engaged.hdr, engaged.color) = (hdr, color);
            engaged.props_dirty = true;
        }
        Ok(())
    }

    /// The card's planes, connector, CRTC and modes now.
    fn read_state(&self) -> Result<Snapshot, String> {
        let card = &self.card;
        let planes = card.planes().map_err(|e| format!("the planes cannot be read: {e}"))?;
        let connector = card.props(self.output.connector_id, drm::OBJECT_CONNECTOR).map_err(|e| format!("the connector cannot be read: {e}"))?;
        let crtc = card.props(self.output.crtc_id, drm::OBJECT_CRTC).map_err(|e| format!("the CRTC cannot be read: {e}"))?;
        let (_, mode, modes) = card
            .outputs()
            .map_err(|e| format!("the outputs cannot be read: {e}"))?
            .into_iter()
            .find(|(o, ..)| o.connector_id == self.output.connector_id)
            .ok_or("the output is no longer driven")?;
        Ok(Snapshot { planes, connector, crtc, mode, modes })
    }

    /// The software renderer for this frame's overlay, its canvas cleared to transparent; None
    /// while the planes do not show the stream.
    pub fn overlay_renderer(&mut self) -> Option<*mut SDL_Renderer> {
        if !self.engaged() {
            return None;
        }
        // SAFETY: our own live software renderer.
        unsafe {
            SDL_SetRenderDrawBlendMode(self.overlay_renderer, SDL_BLENDMODE_NONE);
            SDL_SetRenderDrawColor(self.overlay_renderer, 0, 0, 0, 0);
            SDL_RenderClear(self.overlay_renderer);
        }
        Some(self.overlay_renderer)
    }

    /// The overlay drawn on the canvas: it goes into the buffer not on the screen, which the next
    /// commit shows, unless the plane is already shown with identical pixels. The canvas is
    /// premultiplied, as the plane's default blending takes it.
    pub fn overlay_drawn(&mut self) {
        if self.off.is_some() {
            return;
        }
        let Some(engaged) = self.engaged.as_mut() else { return };
        let back = 1 - engaged.overlay.front;
        let buffer = &engaged.overlay.buffers[back];
        // SAFETY: our renderer and canvas; the buffer's mapping is `pitch` x `height` bytes and the
        // canvas `pitch` x `h`; each row copies the narrower width of the two.
        let changed = unsafe {
            SDL_FlushRenderer(self.overlay_renderer);
            let canvas = &*self.canvas;
            let rows = (canvas.h.max(0) as u32).min(buffer.height) as usize;
            let bytes = ((canvas.w.max(0) as u32).min(buffer.width) * 4) as usize;
            let (from, from_pitch) = (canvas.pixels.cast::<u8>(), canvas.pitch.max(0) as usize);
            sync_overlay_bytes(
                OverlayPlanes {
                    src: from,
                    src_pitch: from_pitch,
                    dst: buffer.map,
                    dst_pitch: buffer.pitch as usize,
                    rows,
                    row_bytes: bytes,
                },
                engaged.overlay.shown,
                &mut engaged.overlay.last_canvas,
            )
        };
        engaged.overlay.change = changed.then_some(true);
    }

    /// Nothing to draw over the picture this frame: the overlay plane goes down if it was up.
    pub fn overlay_hidden(&mut self) {
        if let Some(engaged) = self.engaged.as_mut() {
            engaged.overlay.change = engaged.overlay.shown.then_some(false);
        }
    }

    /// Whether the upcoming [`Self::commit`] can block on a refresh (i.e. has changes to commit
    /// that will not go out as an async page flip).
    pub fn may_block(&self) -> bool {
        if self.off.is_some() {
            return false;
        }
        let Some(engaged) = self.engaged.as_ref() else {
            return false;
        };
        let rect_changed = engaged.shown_rect != Some(engaged.rect);
        let fb_changed = engaged.next_fb != engaged.shown_fb;
        let overlay_changed = engaged.overlay.change.is_some();
        if !engaged.props_dirty && !rect_changed && !fb_changed && !overlay_changed {
            return false;
        }
        let only_fb = !engaged.props_dirty && !rect_changed && !overlay_changed;
        let may_tear = plan::may_tear(self.wishes.tearing, engaged.vrr_on, self.wishes.tear_free);
        let video_on_primary = engaged.layout.video_on_primary(engaged.snapshot.planes.iter().map(|(p, _)| p));
        !plan::async_flip(may_tear, self.async_cap && !self.async_failed, video_on_primary, only_fb)
    }

    /// Shows what `prepare` and the overlay readied, in one atomic commit; blocking, as a V-Sync
    /// present, or an async flip with V-Sync off where only the picture changes and the stream is
    /// not a VRR one that asks for whole frames (`plan::may_tear`). Err gives the planes up
    /// (logged), and the stream goes back to the renderer.
    pub fn commit(&mut self) -> Result<(), String> {
        let result = self.commit_inner();
        if let Err(why) = &result {
            self.give_up(why);
        }
        result
    }

    fn commit_inner(&mut self) -> Result<(), String> {
        let Self { card, output, wishes, engaged, async_cap, async_failed, commit_times, off, commit_sets: sets, commit_buffers, .. } = self;
        if off.is_some() {
            return Ok(());
        }
        let Some(engaged) = engaged.as_mut() else { return Ok(()) };
        sets.clear();
        let mut only_fb = true;
        if engaged.props_dirty {
            Self::stream_sets(card, engaged, output, wishes, sets)?;
            only_fb = false;
        }
        let video = engaged.layout.video;
        if engaged.shown_rect != Some(engaged.rect) {
            let (fb, rect) = (engaged.next_fb, engaged.rect);
            Self::put_plane(engaged, output, sets, video, fb, Some(rect));
            only_fb = false;
        } else if engaged.next_fb != engaged.shown_fb {
            let fb = engaged.next_fb;
            Self::put(engaged, output, sets, Object::Plane(video), "FB_ID", u64::from(fb));
        }
        if let Some(show) = engaged.overlay.change {
            let full = Self::full_rect(engaged);
            let (plane, fb) = (engaged.overlay.plane, engaged.overlay.buffers[1 - engaged.overlay.front].fb);
            Self::put_plane(engaged, output, sets, plane, fb, show.then_some(full));
            only_fb = false;
        }
        if sets.is_empty() {
            return Ok(());
        }
        let may_tear = plan::may_tear(wishes.tearing, engaged.vrr_on, wishes.tear_free);
        let tear = plan::async_flip(may_tear, *async_cap && !*async_failed, engaged.layout.video_on_primary(engaged.snapshot.planes.iter().map(|(p, _)| p)), only_fb);
        let flags = if tear { drm::ATOMIC_PAGE_FLIP_ASYNC } else if only_fb { 0 } else { drm::ATOMIC_ALLOW_MODESET };
        let began = std::time::Instant::now();
        match card.commit_with(commit_buffers, sets, flags) {
            Ok(()) => {}
            Err(e) if tear => {
                eprintln!("nightjar: kms: the card refused an async flip ({e}); pictures wait for the refresh from here");
                *async_failed = true;
                card.commit_with(commit_buffers, sets, 0).map_err(|e| format!("the card refused a picture: {e}"))?;
            }
            Err(e) => return Err(format!("the card refused a commit: {e}")),
        }
        commit_times.add(began.elapsed());
        engaged.shown_fb = engaged.next_fb;
        engaged.shown_rect = Some(engaged.rect);
        engaged.props_dirty = false;
        if let Some(show) = engaged.overlay.change.take() {
            if show {
                engaged.overlay.front = 1 - engaged.overlay.front;
            }
            engaged.overlay.shown = show;
        }
        // The old pool's framebuffers are off the screen now.
        for fb in engaged.retired.drain(..) {
            card.remove_fb(fb);
        }
        Ok(())
    }

    /// The picture just committed stays out of the decoder's pool until another replaces it.
    pub fn keep(&mut self, picture: Pending) {
        if let Some(engaged) = self.engaged.as_mut() {
            engaged.on_screen = Some(picture);
        }
    }

    /// Frees what an engagement made, without a commit (nothing of it was shown).
    fn release(card: &Card, engaged: Engaged) {
        for (_, fb, _) in engaged.fbs {
            card.remove_fb(fb);
        }
        for fb in engaged.retired {
            card.remove_fb(fb);
        }
        for buffer in engaged.overlay.buffers {
            card.destroy_dumb(buffer);
        }
        if let Some(buffer) = engaged.background {
            card.destroy_dumb(buffer);
        }
        for blob in engaged.blobs {
            card.destroy_blob(blob);
        }
    }

    /// The planes stop for the rest of the stream (logged once): the display goes back as it was
    /// and the renderer draws the pictures.
    pub fn give_up(&mut self, why: &str) {
        if self.off.is_some() {
            return;
        }
        eprintln!("nightjar: kms: video planes stop, the stream is drawn by the renderer from here: {why}");
        self.off = Some(why.to_owned());
        self.restore();
    }

    /// Puts back what the stream changed, then frees its framebuffers and buffers.
    fn restore(&mut self) {
        if let Some(times) = std::mem::take(&mut self.commit_times).summary() {
            eprintln!("nightjar: kms: {times}");
        }
        let Some(mut engaged) = self.engaged.take() else { return };
        let sets: Vec<(u32, u32, u64)> = std::mem::take(&mut engaged.restore);
        if !sets.is_empty()
            && let Err(e) = self.card.commit(&sets, drm::ATOMIC_ALLOW_MODESET)
        {
            eprintln!("nightjar: kms: the display's state before the stream could not be put back: {e}");
        }
        // Off the screen now: the decoder may have its surface back.
        engaged.on_screen = None;
        Self::release(&self.card, engaged);
    }
}

impl Drop for Scanout {
    fn drop(&mut self) {
        self.restore();
        // SAFETY: our own renderer and canvas; the GUI let go of its textures on the renderer
        // before the stream ended (main.rs).
        unsafe {
            SDL_DestroyRenderer(self.overlay_renderer);
            SDL_DestroySurface(self.canvas);
        }
    }
}

struct OverlayPlanes {
    src: *const u8,
    src_pitch: usize,
    dst: *mut u8,
    dst_pitch: usize,
    rows: usize,
    row_bytes: usize,
}

/// Copies `rows` x `row_bytes` from `src` (stride `src_pitch`) to `dst` (stride `dst_pitch`) and
/// updates `cache` only when `!already_shown` or the pixels differ from `cache`.
///
/// # Safety
/// `planes.src` must be valid for `planes.rows` rows of `planes.row_bytes` at stride
/// `planes.src_pitch`, and `planes.dst` must be valid for `planes.rows` rows of
/// `planes.row_bytes` at stride `planes.dst_pitch`.
unsafe fn sync_overlay_bytes(
    planes: OverlayPlanes,
    already_shown: bool,
    cache: &mut Vec<u8>,
) -> bool {
    let OverlayPlanes { src, src_pitch, dst, dst_pitch, rows, row_bytes } = planes;
    let total = rows * row_bytes;
    // SAFETY: caller guarantees `src` covers `rows` rows of `row_bytes` bytes at `src_pitch`.
    let unchanged = already_shown
        && cache.len() == total
        && (0..rows).all(|row| {
            let line = unsafe { std::slice::from_raw_parts(src.add(row * src_pitch), row_bytes) };
            line == &cache[row * row_bytes..(row + 1) * row_bytes]
        });
    if unchanged {
        return false;
    }
    cache.resize(total, 0);
    for row in 0..rows {
        // SAFETY: caller guarantees `src` and `dst` cover `row_bytes` at their respective pitches.
        unsafe {
            let line = std::slice::from_raw_parts(src.add(row * src_pitch), row_bytes);
            cache[row * row_bytes..(row + 1) * row_bytes].copy_from_slice(line);
            std::ptr::copy_nonoverlapping(line.as_ptr(), dst.add(row * dst_pitch), row_bytes);
        }
    }
    true
}

/// Where a picture of `visible` size goes on the CRTC of `mode` (`viewport` is the stream's
/// scaling), from a framebuffer of `layout`.
fn picture_rect(mode: &ModeInfo, layout: &plan::FbLayout, visible: (i32, i32), viewport: &Viewport<'_>) -> plan::PlaneRect {
    let crtc = (u32::from(mode.hdisplay), u32::from(mode.vdisplay));
    let area = viewport(visible, (crtc.0 as i32, crtc.1 as i32));
    let picture = ((visible.0.max(1) as u32).min(layout.width), (visible.1.max(1) as u32).min(layout.height));
    plan::plane_rect(picture, area, crtc)
}

#[cfg(test)]
mod tests;
