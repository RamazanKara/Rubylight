//! What Nightjar asks of DRM KMS when it draws straight to the display, worked out from what the
//! card reports and nothing else, so it runs (and is tested) without a card: which display mode a
//! stream's rate fits, which planes show the picture and the overlay in which order, how a decoded
//! picture's DMA-BUFs make one framebuffer, and the connector, CRTC and plane properties a stream
//! sets (HDR metadata, colorimetry, content type, VRR, bit depth, the picture's YCbCr encoding).
//!
//! The choices follow moonlight-qt's DRM renderer (`app/streaming/video/ffmpeg-renderers/drm.cpp`,
//! moonlight-qt v6.2.0, GPL-3.0, by Cameron Gutman and the Moonlight contributors): a plane above
//! every active one on the CRTC for the picture, overlay planes above that, `content type` Game,
//! `VRR_ENABLED` with the stream's pacing, `max bpc` raised to 10 for a 10-bit stream, and the
//! HDR static metadata in `HDR_OUTPUT_METADATA` with `Colorspace` BT2020_RGB.

use crate::dmabuf::Descriptor;

/// A DRM fourcc from its four characters.
pub const fn fourcc(code: &[u8; 4]) -> u32 {
    u32::from_le_bytes(*code)
}

pub const NV12: u32 = fourcc(b"NV12");
pub const P010: u32 = fourcc(b"P010");
pub const ARGB8888: u32 = fourcc(b"AR24");
pub const XRGB8888: u32 = fourcc(b"XR24");
/// `DRM_FORMAT_MOD_LINEAR`.
pub const MOD_LINEAR: u64 = 0;
/// `DRM_FORMAT_MOD_INVALID`: a buffer whose layout the driver knows without a modifier.
pub const MOD_INVALID: u64 = 0x00ff_ffff_ffff_ffff;

/// A fourcc as its characters, for the log.
pub fn fourcc_name(code: u32) -> String {
    code.to_le_bytes().iter().map(|&b| if b.is_ascii_graphic() { b as char } else { '?' }).collect()
}

// ---------------------------------------------------------------------------------------------
// Display modes

/// One of a connector's modes.
#[derive(Clone, Debug, PartialEq)]
pub struct Mode {
    pub width: u32,
    pub height: u32,
    pub refresh_hz: f64,
    pub interlaced: bool,
    /// The connector's preferred mode (`DRM_MODE_TYPE_PREFERRED`).
    pub preferred: bool,
}

/// How far a refresh may be from a whole multiple of the stream's rate and still count as one
/// (59.94 against 60).
const RATE_TOLERANCE_HZ: f64 = 0.5;

/// The multiple of `fps` that `refresh_hz` is (1 for the same rate, 2 for twice it, up to 4), or
/// None when it is none of them.
pub fn rate_multiple(refresh_hz: f64, fps: u32) -> Option<u32> {
    if fps == 0 || refresh_hz <= 0.0 {
        return None;
    }
    (1..=4).find(|&k| (refresh_hz - f64::from(fps) * f64::from(k)).abs() <= RATE_TOLERANCE_HZ * f64::from(k))
}

/// The mode a stream at `fps` shows in: of the modes at the size of the one in use (`current`,
/// else the connector's preferred one, else the largest), the one whose refresh is the stream's
/// rate, else its smallest multiple; the mode in use when none is. The size never changes, so the
/// desktop's resolution stays, as does the launcher's layout; the plane scales the picture.
/// Interlaced modes are never chosen.
pub fn pick_mode(modes: &[Mode], current: Option<usize>, fps: u32) -> Option<usize> {
    let base = current
        .filter(|&i| i < modes.len())
        .or_else(|| modes.iter().position(|m| m.preferred && !m.interlaced))
        .or_else(|| {
            modes.iter().enumerate().filter(|(_, m)| !m.interlaced).max_by_key(|(_, m)| u64::from(m.width) * u64::from(m.height)).map(|(i, _)| i)
        })?;
    let (w, h) = (modes[base].width, modes[base].height);
    let fitting = modes
        .iter()
        .enumerate()
        .filter(|(_, m)| m.width == w && m.height == h && !m.interlaced)
        .filter_map(|(i, m)| rate_multiple(m.refresh_hz, fps).map(|k| (i, k, m)))
        // The smallest multiple, then the closest to it.
        .min_by(|a, b| {
            a.1.cmp(&b.1).then_with(|| {
                let off = |(_, k, m): &(usize, u32, &Mode)| (m.refresh_hz - f64::from(fps) * f64::from(*k)).abs();
                off(a).total_cmp(&off(b))
            })
        })
        .map(|(i, ..)| i);
    // The mode in use already fits as well as the best: keep it, no modeset.
    match fitting {
        Some(best) if rate_multiple(modes[base].refresh_hz, fps) == rate_multiple(modes[best].refresh_hz, fps) => Some(base),
        Some(best) => Some(best),
        None => Some(base),
    }
}

// ---------------------------------------------------------------------------------------------
// Outputs

/// A connector with the CRTC that drives it.
#[derive(Clone, Debug, PartialEq)]
pub struct Output {
    pub connector_id: u32,
    /// The kernel's name, `HDMI-A-1`.
    pub name: String,
    pub crtc_id: u32,
    /// The CRTC's index among the card's CRTCs (planes name the CRTCs they can show on by it).
    pub crtc_index: u32,
    pub width: u32,
    pub height: u32,
}

/// The output that shows SDL's window: the only one driven, or among several the one of the
/// window's display size (`width` x `height`). Anything ambiguous is said, not guessed.
pub fn pick_output(outputs: &[Output], width: u32, height: u32) -> Result<&Output, String> {
    match outputs {
        [] => Err("no connector is driven by a CRTC".into()),
        [only] => Ok(only),
        many => {
            let sized: Vec<&Output> = many.iter().filter(|o| o.width == width && o.height == height).collect();
            match sized.as_slice() {
                [one] => Ok(one),
                found => Err(format!(
                    "{} of {} driven outputs ({}) are {width}x{height}, the window's display",
                    found.len(),
                    many.len(),
                    many.iter().map(|o| format!("{} {}x{}", o.name, o.width, o.height)).collect::<Vec<_>>().join(", ")
                )),
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Planes

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaneKind {
    Overlay,
    Primary,
    Cursor,
}

impl PlaneKind {
    /// The plane `type` property's value.
    pub fn of(value: u64) -> Self {
        match value {
            1 => Self::Primary,
            2 => Self::Cursor,
            _ => Self::Overlay,
        }
    }
}

/// A plane's `zpos`: its range and value, and whether it can be set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Zpos {
    pub min: u64,
    pub max: u64,
    pub value: u64,
    pub immutable: bool,
}

/// A plane as the card reports it.
#[derive(Clone, Debug, PartialEq)]
pub struct Plane {
    pub id: u32,
    pub kind: PlaneKind,
    /// Bit n: the plane can show on the CRTC of index n.
    pub possible_crtcs: u32,
    /// The formats with the modifiers each takes (`IN_FORMATS`); a format with no modifiers was
    /// listed without `IN_FORMATS`, and takes linear or implicit buffers.
    pub formats: Vec<(u32, Vec<u64>)>,
    pub zpos: Option<Zpos>,
    /// The CRTC the plane shows on now, 0 for none.
    pub crtc_id: u32,
    /// `COLOR_ENCODING` and `COLOR_RANGE` are there (a YCbCr-capable plane).
    pub color_props: bool,
}

impl Plane {
    pub fn on_crtc(&self, crtc_index: u32) -> bool {
        crtc_index < 32 && self.possible_crtcs & (1 << crtc_index) != 0
    }

    /// Whether the plane scans out `format` with `modifier` (`MOD_INVALID`: an implicit layout).
    pub fn takes(&self, format: u32, modifier: u64) -> bool {
        self.formats.iter().any(|(f, mods)| {
            *f == format && ((mods.is_empty() && (modifier == MOD_LINEAR || modifier == MOD_INVALID)) || mods.contains(&modifier))
        })
    }

    fn z(&self, fallback: u64) -> u64 {
        self.zpos.map_or(fallback, |z| z.value)
    }
}

/// Which planes show what, and the `zpos` values to set (plane, value) for that order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    /// The picture's plane.
    pub video: u32,
    /// The primary plane, filled black under a picture on another plane, so nothing of the
    /// launcher shows around it.
    pub background: Option<u32>,
    /// An ARGB8888 plane above the picture for the overlay, the stream menu and notices.
    pub overlay: Option<u32>,
    pub zpos: Vec<(u32, u64)>,
}

impl Layout {
    /// The picture is on the primary plane (async flips change only the primary plane's FB).
    pub fn video_on_primary(&self, planes: &[Plane]) -> bool {
        planes.iter().any(|p| p.id == self.video && p.kind == PlaneKind::Primary)
    }
}

/// `zpos` values for planes bottom to top: each above the last, within its range; a plane that
/// cannot be set keeps its own value and must already be in order. None when they cannot be.
fn stack(order: &[&Plane]) -> Option<Vec<(u32, u64)>> {
    if order.iter().all(|p| p.zpos.is_none()) {
        // No zpos on this card (then on none of its planes): the order is the planes' own, by
        // index, primary under overlays, which the candidates already follow.
        return Some(Vec::new());
    }
    let mut sets = Vec::new();
    let mut below: Option<u64> = None;
    for plane in order {
        let z = plane.zpos?;
        let wanted = below.map_or(z.min, |b| b + 1);
        let value = if z.immutable {
            if below.is_some_and(|b| z.value <= b) { return None; }
            z.value
        } else {
            if wanted > z.max { return None; }
            let value = wanted.max(z.min);
            if value != z.value { sets.push((plane.id, value)); }
            value
        };
        below = Some(value);
    }
    Some(sets)
}

/// The layouts to try, best first, for pictures of `format` and `modifier` on the CRTC of index
/// `crtc_index`; the presenter takes the first a test commit accepts. The picture goes on the
/// primary plane where it takes the format (it covers the screen and is always shown), else on an
/// overlay plane over a black primary. The overlay goes on an ARGB8888 overlay plane above it;
/// without one a layout is still given, and the presenter says that the overlay cannot show.
pub fn candidates(planes: &[Plane], crtc_index: u32, format: u32, modifier: u64) -> Vec<Layout> {
    let usable: Vec<&Plane> = planes.iter().filter(|p| p.on_crtc(crtc_index) && p.kind != PlaneKind::Cursor).collect();
    let primary = usable.iter().copied().find(|p| p.kind == PlaneKind::Primary);
    let mut found: Vec<Layout> = Vec::new();
    // 1. The picture on the primary plane, the overlay above it.
    if let Some(primary) = primary.filter(|p| p.takes(format, modifier)) {
        add_layouts(&usable, vec![primary], primary, None, &mut found);
    }
    // 2. The picture on an overlay plane over a black primary, the overlay above it. Planes
    // lower on the stack first, so another one is left above them.
    let mut videos: Vec<&Plane> = usable.iter().copied().filter(|p| p.kind == PlaneKind::Overlay && p.takes(format, modifier)).collect();
    videos.sort_by_key(|p| (p.z(0), p.id));
    for video in videos {
        let below: Vec<&Plane> = primary.into_iter().chain([video]).collect();
        add_layouts(&usable, below, video, primary, &mut found);
    }
    // Every layout with an overlay plane before any without one (a stable sort keeps the order).
    found.sort_by_key(|layout| layout.overlay.is_none());
    found
}

/// The first free ARGB8888 overlay plane that can go above `below` (bottom to top); without zpos
/// on the card, a plane of a higher id is taken to be above.
fn overlay_for<'a>(usable: &[&'a Plane], below: &[&'a Plane]) -> Option<&'a Plane> {
    usable
        .iter()
        .copied()
        .filter(|p| p.kind == PlaneKind::Overlay && !below.iter().any(|b| b.id == p.id))
        .filter(|p| p.takes(ARGB8888, MOD_LINEAR) || p.takes(ARGB8888, MOD_INVALID))
        .filter(|p| p.zpos.is_some() || below.last().is_none_or(|top| p.id > top.id))
        .find(|&p| {
            let mut order = below.to_vec();
            order.push(p);
            stack(&order).is_some()
        })
}

/// The layout of `below` (bottom to top, the picture's plane last) with an overlay plane above
/// it, and the same without one; each once, and only where its order can be set.
fn add_layouts<'a>(usable: &[&'a Plane], below: Vec<&'a Plane>, video: &Plane, background: Option<&Plane>, found: &mut Vec<Layout>) {
    let overlay = overlay_for(usable, &below);
    let mut with = below.clone();
    with.extend(overlay);
    for (order, overlay) in [(with, overlay), (below, None)] {
        if let Some(zpos) = stack(&order) {
            let layout = Layout { video: video.id, background: background.map(|p| p.id), overlay: overlay.map(|p| p.id), zpos };
            if !found.contains(&layout) {
                found.push(layout);
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Framebuffers

/// One plane of a framebuffer: the DMA-BUF object it is in, its offset and pitch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FbPlane {
    pub object: usize,
    pub offset: u32,
    pub pitch: u32,
}

/// A framebuffer made of a decoded picture's DMA-BUFs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FbLayout {
    pub format: u32,
    pub width: u32,
    pub height: u32,
    /// The objects' modifier; `MOD_INVALID` for an implicit layout.
    pub modifier: u64,
    pub planes: Vec<FbPlane>,
}

/// The format a VAAPI surface's layers make together: its own fourcc where the export composed
/// it, else the one its layer formats (R8 + GR88, R16 + GR1616) stand for.
fn composed_format(descriptor: &Descriptor) -> u32 {
    let layers = &descriptor.layers[..(descriptor.num_layers as usize).min(4)];
    match layers {
        [one] => one.drm_format,
        [y, uv] if y.drm_format == fourcc(b"R8  ") && uv.drm_format == fourcc(b"GR88") => NV12,
        [y, uv] if y.drm_format == fourcc(b"R16 ") && uv.drm_format == fourcc(b"GR32") => P010,
        _ => descriptor.fourcc,
    }
}

/// The framebuffer of an exported picture (`dmabuf::Descriptor`), its layers' planes in order, at
/// the surface's (coded) size; the plane shows the stream's part of it. All objects must share one
/// modifier, as the kernel's framebuffers need.
pub fn fb_layout(descriptor: &Descriptor) -> Result<FbLayout, String> {
    let objects = (descriptor.num_objects as usize).min(4);
    if objects == 0 { return Err("the picture has no DMA-BUF".into()); }
    let modifier = descriptor.objects[0].drm_format_modifier;
    if descriptor.objects[..objects].iter().any(|o| o.drm_format_modifier != modifier) {
        return Err("the picture's planes have different modifiers".into());
    }
    let mut planes = Vec::new();
    for layer in &descriptor.layers[..(descriptor.num_layers as usize).min(4)] {
        for p in 0..(layer.num_planes as usize).min(4) {
            let object = layer.object_index[p] as usize;
            if object >= objects { return Err(format!("a plane names object {object} of {objects}")); }
            planes.push(FbPlane { object, offset: layer.offset[p], pitch: layer.pitch[p] });
        }
    }
    if planes.is_empty() || planes.len() > 4 { return Err(format!("the picture has {} planes", planes.len())); }
    let format = composed_format(descriptor);
    let expected = match format { NV12 | P010 => 2, _ => planes.len() };
    if planes.len() != expected { return Err(format!("{} has {} planes, not {expected}", fourcc_name(format), planes.len())); }
    if descriptor.width == 0 || descriptor.height == 0 { return Err("the picture has no size".into()); }
    Ok(FbLayout { format, width: descriptor.width, height: descriptor.height, modifier, planes })
}

// ---------------------------------------------------------------------------------------------
// IN_FORMATS

/// The formats and modifiers of an `IN_FORMATS` blob (`struct drm_format_modifier_blob`): a header
/// of six u32, the formats (u32) at `formats_offset`, and at `modifiers_offset` 24-byte entries,
/// each a 64-bit mask of the 64 formats from its `offset` that take its modifier.
pub fn parse_in_formats(blob: &[u8]) -> Result<Vec<(u32, Vec<u64>)>, String> {
    let u32_at = |at: usize| blob.get(at..at + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap_or_default()));
    let u64_at = |at: usize| blob.get(at..at + 8).map(|b| u64::from_le_bytes(b.try_into().unwrap_or_default()));
    let short = || "the IN_FORMATS blob is shorter than it says".to_owned();
    let version = u32_at(0).ok_or_else(short)?;
    if version != 1 { return Err(format!("IN_FORMATS version {version}")); }
    let count_formats = u32_at(8).ok_or_else(short)? as usize;
    let formats_offset = u32_at(12).ok_or_else(short)? as usize;
    let count_modifiers = u32_at(16).ok_or_else(short)? as usize;
    let modifiers_offset = u32_at(20).ok_or_else(short)? as usize;
    let mut formats: Vec<(u32, Vec<u64>)> = (0..count_formats)
        .map(|i| u32_at(formats_offset + 4 * i).map(|f| (f, Vec::new())).ok_or_else(short))
        .collect::<Result<_, _>>()?;
    for m in 0..count_modifiers {
        let at = modifiers_offset + 24 * m;
        let mask = u64_at(at).ok_or_else(short)?;
        let offset = u32_at(at + 8).ok_or_else(short)? as usize;
        let modifier = u64_at(at + 16).ok_or_else(short)?;
        for bit in 0..64 {
            if mask & (1 << bit) != 0 {
                if let Some((_, mods)) = formats.get_mut(offset + bit) {
                    mods.push(modifier);
                }
            }
        }
    }
    Ok(formats)
}

// ---------------------------------------------------------------------------------------------
// HDR metadata

/// Whether a display's EDID says it takes HDR10: a CTA-861 extension block with an HDR static
/// metadata data block (extended tag 6) whose EOTFs include SMPTE ST 2084 (bit 2).
pub fn edid_supports_pq(edid: &[u8]) -> bool {
    const BLOCK: usize = 128;
    edid.chunks_exact(BLOCK).skip(1).filter(|block| block[0] == 0x02).any(|block| {
        // Data blocks run from byte 4 to the offset of the detailed timings (byte 2).
        let end = usize::from(block[2]).clamp(4, BLOCK - 1);
        let mut at = 4;
        while at < end {
            let (tag, len) = (block[at] >> 5, usize::from(block[at] & 0x1f));
            if tag == 7 && len >= 2 && at + 2 < BLOCK && block[at + 1] == 6 && block[at + 2] & 0b100 != 0 {
                return true;
            }
            at += 1 + len;
        }
        false
    })
}

/// `struct hdr_output_metadata` as the kernel takes it for `HDR_OUTPUT_METADATA`: static metadata
/// type 1 with the PQ transfer (HDMI_EOTF_SMPTE_ST2084), 32 bytes, little-endian. The host's
/// primaries are already in the infoframe's 0.00002 units, its luminances in nits and 0.0001 nits.
pub fn hdr_output_metadata(metadata: &nightjar_core::control::HdrMetadata) -> [u8; 32] {
    const EOTF_SMPTE_ST2084: u8 = 2;
    const STATIC_METADATA_TYPE1: u8 = 0;
    let mut blob = [0u8; 32];
    blob[0..4].copy_from_slice(&u32::from(STATIC_METADATA_TYPE1).to_le_bytes());
    blob[4] = EOTF_SMPTE_ST2084;
    blob[5] = STATIC_METADATA_TYPE1;
    let mut at = 6;
    let mut put = |value: u16| {
        blob[at..at + 2].copy_from_slice(&value.to_le_bytes());
        at += 2;
    };
    for (x, y) in metadata.display_primaries {
        put(x);
        put(y);
    }
    put(metadata.white_point.0);
    put(metadata.white_point.1);
    put(metadata.max_display_luminance);
    put(metadata.min_display_luminance);
    put(metadata.max_content_light_level);
    put(metadata.max_frame_average_light_level);
    blob
}

// ---------------------------------------------------------------------------------------------
// Properties

/// The object a property is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Object {
    Connector,
    Crtc,
    Plane(u32),
}

/// A property's value: a number, or the name of one of its enum values.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Number(u64),
    Enum(&'static str),
    /// The HDR metadata blob, made when committed; `None` clears it.
    HdrBlob(Option<[u8; 32]>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Set {
    pub object: Object,
    pub name: &'static str,
    pub value: Value,
}

/// What the connector and CRTC have, as far as a stream's properties go.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Caps {
    pub vrr_enabled: bool,
    pub vrr_capable: bool,
    pub content_type: bool,
    pub hdr_metadata: bool,
    /// The `Colorspace` enum has BT2020_RGB.
    pub colorspace_bt2020: bool,
    /// `max bpc`: its range's top and its value now.
    pub max_bpc: Option<(u64, u64)>,
}

impl Caps {
    /// HDR can be shown: the connector takes metadata and BT.2020 colorimetry.
    pub fn hdr(&self) -> bool {
        self.hdr_metadata && self.colorspace_bt2020
    }
}

/// What a stream asks of the display.
#[derive(Clone, Debug, PartialEq)]
pub struct Stream {
    pub ten_bit: bool,
    /// HDR metadata when the stream is HDR (PQ); None for SDR.
    pub hdr: Option<nightjar_core::control::HdrMetadata>,
    /// The pacing lets the display follow the stream (VRR).
    pub vrr: bool,
    /// Moonlight's colour space: 0 BT.601, 1 BT.709, 2 BT.2020.
    pub color_space: i32,
    /// 0 limited, 1 full.
    pub color_range: i32,
}

/// `COLOR_ENCODING`'s value for a colour space.
pub fn color_encoding(color_space: i32, hdr: bool) -> &'static str {
    match color_space {
        _ if hdr => "ITU-R BT.2020 YCbCr",
        2 => "ITU-R BT.2020 YCbCr",
        1 => "ITU-R BT.709 YCbCr",
        _ => "ITU-R BT.601 YCbCr",
    }
}

pub fn color_range(full: bool) -> &'static str {
    if full { "YCbCr full range" } else { "YCbCr limited range" }
}

/// The properties a stream sets on the connector, CRTC and picture plane (`video_plane`, with
/// `color_props` where it has the YCbCr ones). Each is left out where the card lacks it; HDR asked
/// for where the connector cannot show it is for the caller to say (`Caps::hdr`).
pub fn stream_properties(caps: &Caps, stream: &Stream, video_plane: u32, color_props: bool) -> Vec<Set> {
    let mut sets = Vec::new();
    let mut set = |object, name, value| sets.push(Set { object, name, value });
    // HDMI's content type Game, which televisions take as their low-latency (game) mode.
    if caps.content_type {
        set(Object::Connector, "content type", Value::Enum("Game"));
    }
    if caps.vrr_enabled {
        set(Object::Crtc, "VRR_ENABLED", Value::Number(u64::from(stream.vrr && caps.vrr_capable)));
    }
    if let Some((top, now)) = caps.max_bpc {
        // Raised to 10 for a 10-bit stream when lower; never lowered (moonlight-qt's rule).
        if stream.ten_bit && now < 10 {
            set(Object::Connector, "max bpc", Value::Number(10.min(top)));
        }
    }
    let hdr = stream.hdr.filter(|_| caps.hdr());
    if caps.hdr_metadata {
        set(Object::Connector, "HDR_OUTPUT_METADATA", Value::HdrBlob(hdr.as_ref().map(hdr_output_metadata)));
    }
    if caps.colorspace_bt2020 {
        set(Object::Connector, "Colorspace", Value::Enum(if hdr.is_some() { "BT2020_RGB" } else { "Default" }));
    }
    if color_props {
        set(Object::Plane(video_plane), "COLOR_ENCODING", Value::Enum(color_encoding(stream.color_space, hdr.is_some())));
        set(Object::Plane(video_plane), "COLOR_RANGE", Value::Enum(color_range(stream.color_range != 0)));
    }
    sets
}

/// Whether a picture's commit can be an async (tearing) flip: V-Sync is off, the kernel takes
/// async atomic commits (`DRM_CAP_ATOMIC_ASYNC_PAGE_FLIP`), and the commit changes only the
/// primary plane's framebuffer, which is all the kernel lets an async commit change.
pub fn async_flip(tearing: bool, cap: bool, video_on_primary: bool, only_fb_changes: bool) -> bool {
    tearing && cap && video_on_primary && only_fb_changes
}

/// A plane's source and destination: the shown part of the picture (16.16 fixed point is the
/// caller's) and where it goes on the CRTC.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlaneRect {
    pub src_w: u32,
    pub src_h: u32,
    pub crtc_x: i32,
    pub crtc_y: i32,
    pub crtc_w: u32,
    pub crtc_h: u32,
}

/// The rectangle (x, y, w, h, as `present::viewport` gives it) rounded to whole pixels, inside
/// the CRTC.
pub fn plane_rect(picture: (u32, u32), viewport: (f32, f32, f32, f32), crtc: (u32, u32)) -> PlaneRect {
    let x = viewport.0.round().max(0.0) as i32;
    let y = viewport.1.round().max(0.0) as i32;
    let w = (viewport.2.round().max(1.0) as u32).min(crtc.0.saturating_sub(x as u32).max(1));
    let h = (viewport.3.round().max(1.0) as u32).min(crtc.1.saturating_sub(y as u32).max(1));
    PlaneRect { src_w: picture.0, src_h: picture.1, crtc_x: x, crtc_y: y, crtc_w: w, crtc_h: h }
}

#[cfg(test)]
#[path = "plan_tests.rs"]
mod tests;
