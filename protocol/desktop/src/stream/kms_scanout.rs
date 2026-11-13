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
use crate::kms::plan::{self, Layout, Object, Plane, Value};

/// What a stream asks of the planes, from its settings.
#[derive(Clone, Copy, Debug)]
pub struct Wishes {
    /// V-Sync is off: pictures may tear in (async flips).
    pub tearing: bool,
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
        (settings.upscaling != crate::config::Upscaling::Bilinear, "Upscaling"),
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
    /// Framebuffers by VA surface; the ones of an old pool wait in `retired` for the next commit.
    fbs: Vec<(u32, Framebuffer)>,
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
}

pub struct Scanout {
    card: Card,
    output: plan::Output,
    wishes: Wishes,
    async_cap: bool,
    async_failed: bool,
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
        Ok(Self { card, output, wishes, async_cap, async_failed: false, canvas, overlay_renderer, engaged: None, off: None })
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

    fn prepare_inner(&mut self, decoded: &Decoded, hdr: Option<HdrMetadata>, viewport: &dyn Fn((i32, i32), (i32, i32)) -> (f32, f32, f32, f32)) -> Result<(), String> {
        let frame = decoded.picture.frame().ok_or("the picture is not a decoded frame")?;
        let export = frame.dmabuf().ok_or("the picture is not a VA-API DMA-BUF export")?;
        let info = frame.info();
        let visible = decoded.visible.unwrap_or((info.width, info.height));
        let descriptor = export.descriptor()?;
        let layout = plan::fb_layout(descriptor)?;
        let fds: Vec<i32> = descriptor.objects[..(descriptor.num_objects as usize).min(4)].iter().map(|o| o.fd).collect();
        let (surface, generation) = (export.surface(), export.generation());
        let color = (decoded.setup.color_space, decoded.setup.color_range);
        if self.engaged.is_none() {
            if !(layout.modifier == plan::MOD_INVALID || layout.modifier == plan::MOD_LINEAR || self.card.modifiers()) {
                return Err("the card takes no framebuffer modifiers".into());
            }
            let fb = self.card.add_dmabuf_fb(&layout, &fds).map_err(|e| format!("the first picture's framebuffer cannot be made: {e}"))?;
            return self.engage(&layout, fb, (surface, generation), visible, (hdr, color), viewport);
        }
        let card = &self.card;
        let engaged = self.engaged.as_mut().expect("engaged");
        if engaged.format != (layout.format, layout.modifier) {
            return Err(format!(
                "the pictures changed to {} with modifier {:#x}, which the plane was not chosen for",
                plan::fourcc_name(layout.format),
                layout.modifier
            ));
        }
        // A new pool: its surfaces are other buffers.
        if engaged.generation != Some(generation) {
            engaged.retired.extend(engaged.fbs.drain(..).map(|(_, fb)| fb));
            engaged.generation = Some(generation);
        }
        let fb = match engaged.fbs.iter().find(|(s, _)| *s == surface) {
            Some((_, fb)) => fb.id,
            None => {
                let fb = card.add_dmabuf_fb(&layout, &fds).map_err(|e| format!("a picture's framebuffer cannot be made: {e}"))?;
                let id = fb.id;
                engaged.fbs.push((surface, fb));
                id
            }
        };
        engaged.next_fb = fb;
        engaged.rect = picture_rect(&engaged.snapshot.mode, &layout, visible, viewport);
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

    /// The first picture (its framebuffer `fb`): the card's state now, the layout a test commit
    /// takes, the display mode for the stream's rate and the stream's properties, in one commit
    /// that shows the picture.
    fn engage(
        &mut self,
        layout: &plan::FbLayout,
        fb: Framebuffer,
        (surface, generation): (u32, crate::dmabuf::Generation),
        visible: (i32, i32),
        (hdr, color): (Option<HdrMetadata>, (i32, i32)),
        viewport: &dyn Fn((i32, i32), (i32, i32)) -> (f32, f32, f32, f32),
    ) -> Result<(), String> {
        let snapshot = match self.read_state() {
            Ok(snapshot) => snapshot,
            Err(why) => {
                self.card.remove_fb(fb);
                return Err(why);
            }
        };
        let card = &self.card;
        let plain: Vec<Plane> = snapshot.planes.iter().map(|(p, _)| p.clone()).collect();
        let layouts: Vec<Layout> = plan::candidates(&plain, self.output.crtc_index, layout.format, layout.modifier)
            .into_iter()
            .filter(|l| l.overlay.is_some())
            .collect();
        if layouts.is_empty() {
            card.remove_fb(fb);
            return Err(format!(
                "no two planes of {} take {} pictures with modifier {:#x} and an overlay above them",
                self.output.name,
                plan::fourcc_name(layout.format),
                layout.modifier
            ));
        }
        let (w, h) = (u32::from(snapshot.mode.hdisplay), u32::from(snapshot.mode.vdisplay));
        let overlay_buffers = match (0..2).map(|_| card.dumb_buffer(w, h, plan::ARGB8888)).collect::<std::io::Result<Vec<_>>>() {
            Ok(buffers) => buffers,
            Err(e) => {
                card.remove_fb(fb);
                return Err(format!("no overlay buffers: {e}"));
            }
        };
        let rect = picture_rect(&snapshot.mode, layout, visible, viewport);
        let next_fb = fb.id;
        let mut engaged = Engaged {
            snapshot,
            layout: layouts[0].clone(),
            format: (layout.format, layout.modifier),
            generation: Some(generation),
            fbs: vec![(surface, fb)],
            retired: Vec::new(),
            next_fb,
            shown_fb: 0,
            rect,
            shown_rect: None,
            on_screen: None,
            overlay: OverlayPlane { plane: 0, buffers: overlay_buffers, front: 0, shown: false, change: None },
            background: None,
            hdr,
            color,
            blobs: Vec::new(),
            restore: Vec::new(),
            props_dirty: true,
        };
        // The display mode for the stream's rate, of the size in use.
        let plan_modes: Vec<plan::Mode> = engaged.snapshot.modes.iter().map(ModeInfo::plan).collect();
        let current = engaged.snapshot.modes.iter().position(|m| *m == engaged.snapshot.mode);
        let wanted = plan::pick_mode(&plan_modes, current, self.wishes.fps).map(|i| engaged.snapshot.modes[i]);
        let original_mode = engaged.snapshot.mode;
        let mode_blob = match wanted.filter(|m| *m != engaged.snapshot.mode) {
            Some(m) => match card.mode_blob(&m) {
                Ok(blob) => {
                    eprintln!("nightjar: kms: display mode {}x{} at {:.2} Hz for {} fps", m.hdisplay, m.vdisplay, m.refresh_hz(), self.wishes.fps);
                    engaged.blobs.push(blob);
                    engaged.snapshot.mode = m;
                    Some(blob)
                }
                Err(e) => {
                    eprintln!("nightjar: kms: the display mode for {} fps cannot be made ({e}); it stays", self.wishes.fps);
                    None
                }
            },
            None => None,
        };
        let mut tried = Vec::new();
        let mut chosen = None;
        for candidate in &layouts {
            if candidate.background.is_some() && engaged.background.is_none() {
                match card.dumb_buffer(w, h, plan::XRGB8888) {
                    Ok(buffer) => engaged.background = Some(buffer),
                    Err(e) => {
                        tried.push(format!("picture on plane {} (no black background: {e})", candidate.video));
                        continue;
                    }
                }
            }
            engaged.layout = candidate.clone();
            engaged.overlay.plane = candidate.overlay.unwrap_or(0);
            // Tested with the overlay up, so showing it later fits the card's limits too.
            let tested = Self::engage_sets(card, &mut engaged, &self.output, &self.wishes, mode_blob, true)
                .and_then(|sets| card.commit(&sets, drm::ATOMIC_TEST_ONLY | drm::ATOMIC_ALLOW_MODESET).map_err(|e| e.to_string()));
            match tested {
                Ok(()) => {
                    chosen = Some(candidate.clone());
                    break;
                }
                Err(e) => tried.push(format!("picture on plane {} ({e})", candidate.video)),
            }
        }
        let Some(chosen) = chosen else {
            Self::release(card, engaged);
            return Err(format!("the card refused every layout: {}", tried.join(", ")));
        };
        engaged.overlay.plane = chosen.overlay.unwrap_or(0);
        engaged.layout = chosen;
        // The commit that takes the display with the first picture; the overlay plane stays down
        // until something shows. Only this commit's changes are put back at the end.
        engaged.restore.clear();
        let committed = Self::engage_sets(card, &mut engaged, &self.output, &self.wishes, mode_blob, false)
            .and_then(|sets| card.commit(&sets, drm::ATOMIC_ALLOW_MODESET).map_err(|e| format!("the card refused the stream's first commit: {e}")));
        if let Err(why) = committed {
            Self::release(card, engaged);
            return Err(why);
        }
        engaged.shown_fb = engaged.next_fb;
        engaged.shown_rect = Some(engaged.rect);
        engaged.props_dirty = false;
        // The mode goes back as a blob of its own: the one the CRTC had may be gone by then.
        if mode_blob.is_some() {
            let mode_prop = engaged.snapshot.crtc.get("MODE_ID").map(|(info, _)| info.id);
            match (mode_prop, card.mode_blob(&original_mode)) {
                (Some(prop), Ok(blob)) => {
                    engaged.blobs.push(blob);
                    for entry in engaged.restore.iter_mut().filter(|(o, p, _)| *o == self.output.crtc_id && *p == prop) {
                        entry.2 = u64::from(blob);
                    }
                }
                (_, Err(e)) => eprintln!("nightjar: kms: the display mode to go back to cannot be made ({e}); the old one is asked for"),
                (None, Ok(blob)) => card.destroy_blob(blob),
            }
        }
        eprintln!(
            "nightjar: kms: the stream shows on plane {}{}, the overlay on plane {}, {} {}; HDR {}, VRR {}",
            engaged.layout.video,
            engaged.layout.background.map_or(String::new(), |b| format!(" over a black plane {b}")),
            engaged.overlay.plane,
            plan::fourcc_name(layout.format),
            if layout.modifier == plan::MOD_INVALID { "implicit".to_owned() } else { format!("modifier {:#x}", layout.modifier) },
            if hdr.is_some() { "on" } else { "off" },
            if self.wishes.vrr { "asked for" } else { "off" },
        );
        self.engaged = Some(engaged);
        Ok(())
    }

    fn props_of(engaged: &Engaged, object: Object) -> Option<&Props> {
        match object {
            Object::Connector => Some(&engaged.snapshot.connector),
            Object::Crtc => Some(&engaged.snapshot.crtc),
            Object::Plane(id) => engaged.snapshot.planes.iter().find(|(p, _)| p.id == id).map(|(_, props)| props),
        }
    }

    fn object_id(output: &plan::Output, object: Object) -> u32 {
        match object {
            Object::Connector => output.connector_id,
            Object::Crtc => output.crtc_id,
            Object::Plane(id) => id,
        }
    }

    /// Appends `name` = `value` on `object` to `sets`, noting the value it had for the restore.
    /// A property the object lacks is left out.
    fn put(engaged: &mut Engaged, output: &plan::Output, sets: &mut Vec<(u32, u32, u64)>, object: Object, name: &str, value: u64) {
        let Some(props) = Self::props_of(engaged, object) else { return };
        let Some((info, before)) = props.get(name).map(|(info, v)| (info.id, *v)) else { return };
        let id = Self::object_id(output, object);
        if !engaged.restore.iter().any(|(o, p, _)| *o == id && *p == info) {
            engaged.restore.push((id, info, before));
        }
        sets.retain(|(o, p, _)| !(*o == id && *p == info));
        sets.push((id, info, value));
    }

    fn put_plane(engaged: &mut Engaged, output: &plan::Output, sets: &mut Vec<(u32, u32, u64)>, plane: u32, fb: u32, rect: Option<plan::PlaneRect>) {
        let object = Object::Plane(plane);
        match rect {
            Some(r) => {
                Self::put(engaged, output, sets, object, "FB_ID", u64::from(fb));
                Self::put(engaged, output, sets, object, "CRTC_ID", u64::from(output.crtc_id));
                Self::put(engaged, output, sets, object, "SRC_X", 0);
                Self::put(engaged, output, sets, object, "SRC_Y", 0);
                Self::put(engaged, output, sets, object, "SRC_W", u64::from(r.src_w) << 16);
                Self::put(engaged, output, sets, object, "SRC_H", u64::from(r.src_h) << 16);
                Self::put(engaged, output, sets, object, "CRTC_X", i64::from(r.crtc_x) as u64);
                Self::put(engaged, output, sets, object, "CRTC_Y", i64::from(r.crtc_y) as u64);
                Self::put(engaged, output, sets, object, "CRTC_W", u64::from(r.crtc_w));
                Self::put(engaged, output, sets, object, "CRTC_H", u64::from(r.crtc_h));
            }
            None => {
                Self::put(engaged, output, sets, object, "FB_ID", 0);
                Self::put(engaged, output, sets, object, "CRTC_ID", 0);
            }
        }
    }

    /// The stream's properties (`plan::stream_properties`) as sets, making the HDR blob.
    fn stream_sets(card: &Card, engaged: &mut Engaged, output: &plan::Output, wishes: &Wishes, sets: &mut Vec<(u32, u32, u64)>) -> Result<(), String> {
        let caps = caps_of(&engaged.snapshot.connector, &engaged.snapshot.crtc);
        let color_props = engaged.snapshot.planes.iter().any(|(p, _)| p.id == engaged.layout.video && p.color_props);
        let stream = plan::Stream {
            ten_bit: engaged.format.0 == plan::P010,
            hdr: engaged.hdr,
            vrr: wishes.vrr,
            color_space: engaged.color.0,
            color_range: engaged.color.1,
        };
        for set in plan::stream_properties(&caps, &stream, engaged.layout.video, color_props) {
            let value = match &set.value {
                Value::Number(n) => *n,
                Value::Enum(name) => match Self::props_of(engaged, set.object).and_then(|p| p.enum_value(set.name, name)) {
                    Some(v) => v,
                    None => continue,
                },
                Value::HdrBlob(None) => 0,
                Value::HdrBlob(Some(bytes)) => {
                    let blob = card.create_blob(bytes).map_err(|e| format!("the HDR metadata cannot be made: {e}"))?;
                    engaged.blobs.push(blob);
                    u64::from(blob)
                }
            };
            Self::put(engaged, output, sets, set.object, set.name, value);
        }
        Ok(())
    }

    /// The engaging commit: mode, stream properties, zpos, the picture plane with the first
    /// picture, the black background, the overlay (`overlay_up` for the test, else down), and
    /// every other plane on the CRTC down.
    fn engage_sets(card: &Card, engaged: &mut Engaged, output: &plan::Output, wishes: &Wishes, mode_blob: Option<u32>, overlay_up: bool) -> Result<Vec<(u32, u32, u64)>, String> {
        let mut sets = Vec::new();
        if let Some(blob) = mode_blob {
            Self::put(engaged, output, &mut sets, Object::Crtc, "MODE_ID", u64::from(blob));
            Self::put(engaged, output, &mut sets, Object::Crtc, "ACTIVE", 1);
            Self::put(engaged, output, &mut sets, Object::Connector, "CRTC_ID", u64::from(output.crtc_id));
        }
        Self::stream_sets(card, engaged, output, wishes, &mut sets)?;
        for (plane, z) in engaged.layout.zpos.clone() {
            Self::put(engaged, output, &mut sets, Object::Plane(plane), "zpos", z);
        }
        let full = Self::full_rect(engaged);
        let (video, rect, fb) = (engaged.layout.video, engaged.rect, engaged.next_fb);
        Self::put_plane(engaged, output, &mut sets, video, fb, Some(rect));
        if let (Some(plane), Some(fb)) = (engaged.layout.background, engaged.background.as_ref().map(|b| b.fb)) {
            Self::put_plane(engaged, output, &mut sets, plane, fb, Some(full));
        }
        let overlay = engaged.overlay.plane;
        let overlay_fb = engaged.overlay.buffers[0].fb;
        Self::put_plane(engaged, output, &mut sets, overlay, overlay_fb, overlay_up.then_some(full));
        let ours = [Some(video), engaged.layout.background, Some(overlay)];
        let others: Vec<u32> = engaged
            .snapshot
            .planes
            .iter()
            .filter(|(p, _)| p.crtc_id == output.crtc_id && p.kind != plan::PlaneKind::Cursor && !ours.contains(&Some(p.id)))
            .map(|(p, _)| p.id)
            .collect();
        for plane in others {
            Self::put_plane(engaged, output, &mut sets, plane, 0, None);
        }
        Ok(sets)
    }

    /// The whole CRTC, for the background and the overlay.
    fn full_rect(engaged: &Engaged) -> plan::PlaneRect {
        let (w, h) = (u32::from(engaged.snapshot.mode.hdisplay), u32::from(engaged.snapshot.mode.vdisplay));
        plan::PlaneRect { src_w: w, src_h: h, crtc_x: 0, crtc_y: 0, crtc_w: w, crtc_h: h }
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
    /// commit shows. The canvas is premultiplied, as the plane's default blending takes it.
    pub fn overlay_drawn(&mut self) {
        if self.off.is_some() {
            return;
        }
        let Some(engaged) = self.engaged.as_mut() else { return };
        let back = 1 - engaged.overlay.front;
        let buffer = &engaged.overlay.buffers[back];
        // SAFETY: our renderer and canvas; the buffer's mapping is `pitch` x `height` bytes and the
        // canvas `pitch` x `h`; each row copies the narrower width of the two.
        unsafe {
            SDL_FlushRenderer(self.overlay_renderer);
            let canvas = &*self.canvas;
            let rows = (canvas.h.max(0) as u32).min(buffer.height) as usize;
            let bytes = ((canvas.w.max(0) as u32).min(buffer.width) * 4) as usize;
            let (from, from_pitch) = (canvas.pixels.cast::<u8>(), canvas.pitch.max(0) as usize);
            for row in 0..rows {
                std::ptr::copy_nonoverlapping(from.add(row * from_pitch), buffer.map.add(row * buffer.pitch as usize), bytes);
            }
        }
        engaged.overlay.change = Some(true);
    }

    /// Nothing to draw over the picture this frame: the overlay plane goes down if it was up.
    pub fn overlay_hidden(&mut self) {
        if let Some(engaged) = self.engaged.as_mut() {
            engaged.overlay.change = engaged.overlay.shown.then_some(false);
        }
    }

    /// Shows what `prepare` and the overlay readied, in one atomic commit; blocking, as a V-Sync
    /// present, or an async flip with V-Sync off where only the picture changes. Err gives the
    /// planes up (logged), and the stream goes back to the renderer.
    pub fn commit(&mut self) -> Result<(), String> {
        let result = self.commit_inner();
        if let Err(why) = &result {
            self.give_up(why);
        }
        result
    }

    fn commit_inner(&mut self) -> Result<(), String> {
        let Self { card, output, wishes, engaged, async_cap, async_failed, off, .. } = self;
        if off.is_some() {
            return Ok(());
        }
        let Some(engaged) = engaged.as_mut() else { return Ok(()) };
        let mut sets = Vec::new();
        let mut only_fb = true;
        if engaged.props_dirty {
            Self::stream_sets(card, engaged, output, wishes, &mut sets)?;
            only_fb = false;
        }
        let video = engaged.layout.video;
        if engaged.shown_rect != Some(engaged.rect) {
            let (fb, rect) = (engaged.next_fb, engaged.rect);
            Self::put_plane(engaged, output, &mut sets, video, fb, Some(rect));
            only_fb = false;
        } else if engaged.next_fb != engaged.shown_fb {
            let fb = engaged.next_fb;
            Self::put(engaged, output, &mut sets, Object::Plane(video), "FB_ID", u64::from(fb));
        }
        if let Some(show) = engaged.overlay.change {
            let full = Self::full_rect(engaged);
            let (plane, fb) = (engaged.overlay.plane, engaged.overlay.buffers[1 - engaged.overlay.front].fb);
            Self::put_plane(engaged, output, &mut sets, plane, fb, show.then_some(full));
            only_fb = false;
        }
        if sets.is_empty() {
            return Ok(());
        }
        let planes: Vec<Plane> = engaged.snapshot.planes.iter().map(|(p, _)| p.clone()).collect();
        let tear = plan::async_flip(wishes.tearing, *async_cap && !*async_failed, engaged.layout.video_on_primary(&planes), only_fb);
        let flags = if tear { drm::ATOMIC_PAGE_FLIP_ASYNC } else if only_fb { 0 } else { drm::ATOMIC_ALLOW_MODESET };
        match card.commit(&sets, flags) {
            Ok(()) => {}
            Err(e) if tear => {
                eprintln!("nightjar: kms: the card refused an async flip ({e}); pictures wait for the refresh from here");
                *async_failed = true;
                card.commit(&sets, 0).map_err(|e| format!("the card refused a picture: {e}"))?;
            }
            Err(e) => return Err(format!("the card refused a commit: {e}")),
        }
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
        for (_, fb) in engaged.fbs {
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
        let Some(mut engaged) = self.engaged.take() else { return };
        let sets: Vec<(u32, u32, u64)> = engaged.restore.drain(..).collect();
        if !sets.is_empty() {
            if let Err(e) = self.card.commit(&sets, drm::ATOMIC_ALLOW_MODESET) {
                eprintln!("nightjar: kms: the display's state before the stream could not be put back: {e}");
            }
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

/// Where a picture of `visible` size goes on the CRTC of `mode` (`viewport` is the stream's
/// scaling), from a framebuffer of `layout`.
fn picture_rect(mode: &ModeInfo, layout: &plan::FbLayout, visible: (i32, i32), viewport: &dyn Fn((i32, i32), (i32, i32)) -> (f32, f32, f32, f32)) -> plan::PlaneRect {
    let crtc = (u32::from(mode.hdisplay), u32::from(mode.vdisplay));
    let area = viewport(visible, (crtc.0 as i32, crtc.1 as i32));
    let picture = ((visible.0.max(1) as u32).min(layout.width), (visible.1.max(1) as u32).min(layout.height));
    plan::plane_rect(picture, area, crtc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Settings, Upscaling};

    #[test]
    fn pictures_stay_with_the_renderer_where_it_has_work_on_them() {
        assert_eq!(renderer_work(&Settings::default()), None);
        assert_eq!(renderer_work(&Settings { upscaling: Upscaling::Fsr1, ..Settings::default() }), Some("Upscaling"));
        assert_eq!(renderer_work(&Settings { frame_warp: true, ..Settings::default() }), Some("Frame Warp"));
        assert_eq!(renderer_work(&Settings { prefer_yuv444: true, ..Settings::default() }), Some("Sharper colored text"));
    }

    #[test]
    fn planes_need_kms_the_setting_and_opengl() {
        // The tests never draw through KMS.
        assert!(possible(true, "opengl").unwrap_err().contains("not drawing straight to the display"));
        assert!(planned(&Settings::default(), "opengl").is_err());
    }
}
