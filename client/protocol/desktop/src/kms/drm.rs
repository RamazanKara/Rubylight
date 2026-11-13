//! DRM KMS through the kernel's ioctls, raw (`drm.h`, `drm_mode.h`), as `stream::drm_vrr` reads
//! them, so no libdrm is linked: the card's connectors, CRTCs, planes and their properties, atomic
//! commits, property blobs, framebuffers of DMA-BUFs and dumb buffers. Everything works on a file
//! descriptor the caller owns (SDL's, which holds DRM master on the KMSDRM video driver).

use std::collections::HashMap;
use std::os::fd::RawFd;

use super::plan::{self, Mode, Output, Plane, PlaneKind, Zpos};

// ---------------------------------------------------------------------------------------------
// The kernel's structs

#[repr(C)]
#[derive(Default)]
struct CardRes {
    fb_id_ptr: u64,
    crtc_id_ptr: u64,
    connector_id_ptr: u64,
    encoder_id_ptr: u64,
    count_fbs: u32,
    count_crtcs: u32,
    count_connectors: u32,
    count_encoders: u32,
    min_width: u32,
    max_width: u32,
    min_height: u32,
    max_height: u32,
}

/// `struct drm_mode_modeinfo`.
#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct ModeInfo {
    pub clock: u32,
    pub hdisplay: u16,
    pub hsync_start: u16,
    pub hsync_end: u16,
    pub htotal: u16,
    pub hskew: u16,
    pub vdisplay: u16,
    pub vsync_start: u16,
    pub vsync_end: u16,
    pub vtotal: u16,
    pub vscan: u16,
    pub vrefresh: u32,
    pub flags: u32,
    pub kind: u32,
    pub name: [u8; 32],
}

impl ModeInfo {
    const INTERLACE: u32 = 1 << 4;
    const DBLSCAN: u32 = 1 << 5;
    const TYPE_PREFERRED: u32 = 1 << 3;

    /// Its refresh from its timings, as the kernel's `drm_mode_vrefresh` works it out.
    pub fn refresh_hz(&self) -> f64 {
        if self.htotal == 0 || self.vtotal == 0 {
            return f64::from(self.vrefresh);
        }
        let mut hz = f64::from(self.clock) * 1000.0 / (f64::from(self.htotal) * f64::from(self.vtotal));
        if self.flags & Self::INTERLACE != 0 { hz *= 2.0; }
        if self.flags & Self::DBLSCAN != 0 { hz /= 2.0; }
        if self.vscan > 1 { hz /= f64::from(self.vscan); }
        hz
    }

    pub fn plan(&self) -> Mode {
        Mode {
            width: u32::from(self.hdisplay),
            height: u32::from(self.vdisplay),
            refresh_hz: self.refresh_hz(),
            interlaced: self.flags & Self::INTERLACE != 0,
            preferred: self.kind & Self::TYPE_PREFERRED != 0,
        }
    }

    fn bytes(&self) -> &[u8] {
        // SAFETY: a plain repr(C) struct of integers without padding (asserted below).
        unsafe { std::slice::from_raw_parts((self as *const Self).cast::<u8>(), size_of::<Self>()) }
    }
}

#[repr(C)]
#[derive(Default)]
struct Crtc {
    set_connectors_ptr: u64,
    count_connectors: u32,
    crtc_id: u32,
    fb_id: u32,
    x: u32,
    y: u32,
    gamma_size: u32,
    mode_valid: u32,
    mode: ModeInfo,
}

#[repr(C)]
#[derive(Default)]
struct Encoder {
    encoder_id: u32,
    encoder_type: u32,
    crtc_id: u32,
    possible_crtcs: u32,
    possible_clones: u32,
}

#[repr(C)]
#[derive(Default)]
struct Connector {
    encoders_ptr: u64,
    modes_ptr: u64,
    props_ptr: u64,
    prop_values_ptr: u64,
    count_modes: u32,
    count_props: u32,
    count_encoders: u32,
    encoder_id: u32,
    connector_id: u32,
    connector_type: u32,
    connector_type_id: u32,
    connection: u32,
    mm_width: u32,
    mm_height: u32,
    subpixel: u32,
    pad: u32,
}

#[repr(C)]
#[derive(Default)]
struct ObjProperties {
    props_ptr: u64,
    prop_values_ptr: u64,
    count_props: u32,
    obj_id: u32,
    obj_type: u32,
}

#[repr(C)]
#[derive(Default)]
struct Property {
    values_ptr: u64,
    enum_blob_ptr: u64,
    prop_id: u32,
    flags: u32,
    name: [u8; 32],
    count_values: u32,
    count_enum_blobs: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct PropertyEnum {
    value: u64,
    name: [u8; 32],
}

#[repr(C)]
#[derive(Default)]
struct GetBlob {
    blob_id: u32,
    length: u32,
    data: u64,
}

#[repr(C)]
#[derive(Default)]
struct PlaneRes {
    plane_id_ptr: u64,
    count_planes: u32,
}

#[repr(C)]
#[derive(Default)]
struct GetPlane {
    plane_id: u32,
    crtc_id: u32,
    fb_id: u32,
    possible_crtcs: u32,
    gamma_size: u32,
    count_format_types: u32,
    format_type_ptr: u64,
}

#[repr(C)]
#[derive(Default)]
struct FbCmd2 {
    fb_id: u32,
    width: u32,
    height: u32,
    pixel_format: u32,
    flags: u32,
    handles: [u32; 4],
    pitches: [u32; 4],
    offsets: [u32; 4],
    modifier: [u64; 4],
}

#[repr(C)]
#[derive(Default)]
struct Atomic {
    flags: u32,
    count_objs: u32,
    objs_ptr: u64,
    count_props_ptr: u64,
    props_ptr: u64,
    prop_values_ptr: u64,
    reserved: u64,
    user_data: u64,
}

#[repr(C)]
#[derive(Default)]
struct CreateBlob {
    data: u64,
    length: u32,
    blob_id: u32,
}

#[repr(C)]
#[derive(Default)]
struct DestroyBlob {
    blob_id: u32,
}

#[repr(C)]
#[derive(Default)]
struct PrimeHandle {
    handle: u32,
    flags: u32,
    fd: i32,
}

#[repr(C)]
#[derive(Default)]
struct GemClose {
    handle: u32,
    pad: u32,
}

#[repr(C)]
#[derive(Default)]
struct CreateDumb {
    height: u32,
    width: u32,
    bpp: u32,
    flags: u32,
    handle: u32,
    pitch: u32,
    size: u64,
}

#[repr(C)]
#[derive(Default)]
struct MapDumb {
    handle: u32,
    pad: u32,
    offset: u64,
}

#[repr(C)]
#[derive(Default)]
struct DestroyDumb {
    handle: u32,
}

#[repr(C)]
#[derive(Default)]
struct Cap {
    capability: u64,
    value: u64,
}

// The kernel's layouts; a wrong size would be a different ioctl number.
const _: () = assert!(size_of::<CardRes>() == 64);
const _: () = assert!(size_of::<ModeInfo>() == 68);
const _: () = assert!(size_of::<Crtc>() == 104);
const _: () = assert!(size_of::<Encoder>() == 20);
const _: () = assert!(size_of::<Connector>() == 80);
const _: () = assert!(size_of::<ObjProperties>() == 32);
const _: () = assert!(size_of::<Property>() == 64);
const _: () = assert!(size_of::<PropertyEnum>() == 40);
const _: () = assert!(size_of::<GetBlob>() == 16);
const _: () = assert!(size_of::<PlaneRes>() == 16);
const _: () = assert!(size_of::<GetPlane>() == 32);
const _: () = assert!(size_of::<FbCmd2>() == 104);
const _: () = assert!(size_of::<Atomic>() == 56);
const _: () = assert!(size_of::<CreateBlob>() == 16);
const _: () = assert!(size_of::<DestroyBlob>() == 4);
const _: () = assert!(size_of::<PrimeHandle>() == 12);
const _: () = assert!(size_of::<GemClose>() == 8);
const _: () = assert!(size_of::<CreateDumb>() == 32);
const _: () = assert!(size_of::<MapDumb>() == 16);
const _: () = assert!(size_of::<DestroyDumb>() == 4);
const _: () = assert!(size_of::<Cap>() == 16);

/// `DRM_IOWR(nr, T)`: read and write, type 'd'.
const fn iowr<T>(nr: u32) -> u32 { (3 << 30) | ((size_of::<T>() as u32) << 16) | (0x64 << 8) | nr }
/// `DRM_IOW(nr, T)`: write only.
const fn iow<T>(nr: u32) -> u32 { (1 << 30) | ((size_of::<T>() as u32) << 16) | (0x64 << 8) | nr }

const GEM_CLOSE: u32 = iow::<GemClose>(0x09);
const GET_CAP: u32 = iowr::<Cap>(0x0c);
const SET_CLIENT_CAP: u32 = iow::<Cap>(0x0d);
const PRIME_FD_TO_HANDLE: u32 = iowr::<PrimeHandle>(0x2e);
const GETRESOURCES: u32 = iowr::<CardRes>(0xA0);
const GETCRTC: u32 = iowr::<Crtc>(0xA1);
const GETENCODER: u32 = iowr::<Encoder>(0xA6);
const GETCONNECTOR: u32 = iowr::<Connector>(0xA7);
const GETPROPERTY: u32 = iowr::<Property>(0xAA);
const GETPROPBLOB: u32 = iowr::<GetBlob>(0xAC);
const RMFB: u32 = iowr::<u32>(0xAF);
const CREATE_DUMB: u32 = iowr::<CreateDumb>(0xB2);
const MAP_DUMB: u32 = iowr::<MapDumb>(0xB3);
const DESTROY_DUMB: u32 = iowr::<DestroyDumb>(0xB4);
const GETPLANERESOURCES: u32 = iowr::<PlaneRes>(0xB5);
const GETPLANE: u32 = iowr::<GetPlane>(0xB6);
const ADDFB2: u32 = iowr::<FbCmd2>(0xB8);
const OBJ_GETPROPERTIES: u32 = iowr::<ObjProperties>(0xB9);
const ATOMIC: u32 = iowr::<Atomic>(0xBC);
const CREATEPROPBLOB: u32 = iowr::<CreateBlob>(0xBD);
const DESTROYPROPBLOB: u32 = iowr::<DestroyBlob>(0xBE);

pub const OBJECT_CRTC: u32 = 0xcccc_cccc;
pub const OBJECT_CONNECTOR: u32 = 0xc0c0_c0c0;
pub const OBJECT_PLANE: u32 = 0xeeee_eeee;
const CONNECTED: u32 = 1;

const CLIENT_CAP_UNIVERSAL_PLANES: u64 = 2;
const CLIENT_CAP_ATOMIC: u64 = 3;
const CAP_ADDFB2_MODIFIERS: u64 = 0x10;
const CAP_ATOMIC_ASYNC_PAGE_FLIP: u64 = 0x15;

const PROP_RANGE: u32 = 1 << 1;
const PROP_IMMUTABLE: u32 = 1 << 2;
const PROP_ENUM: u32 = 1 << 3;
const PROP_BITMASK: u32 = 1 << 5;
const PROP_SIGNED_RANGE: u32 = 2 << 6;

pub const ATOMIC_PAGE_FLIP_ASYNC: u32 = 0x02;
pub const ATOMIC_TEST_ONLY: u32 = 0x0100;
pub const ATOMIC_ALLOW_MODESET: u32 = 0x0400;
const FB_MODIFIERS: u32 = 1 << 1;

fn ioctl<T>(fd: RawFd, request: u32, arg: &mut T) -> std::io::Result<()> {
    loop {
        // SAFETY: `arg` is the request's struct, and the pointers in it address buffers that live
        // through the call with the counts it gives.
        if unsafe { libc::ioctl(fd, request as _, arg as *mut T) } == 0 { return Ok(()); }
        let error = std::io::Error::last_os_error();
        if !matches!(error.raw_os_error(), Some(libc::EINTR | libc::EAGAIN)) { return Err(error); }
    }
}

fn name_of(raw: &[u8; 32]) -> String {
    let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
    String::from_utf8_lossy(&raw[..end]).into_owned()
}

/// The kernel's name of a connector type (`drm_connector_enum_list`).
fn connector_type_name(kind: u32) -> &'static str {
    match kind {
        1 => "VGA", 2 => "DVI-I", 3 => "DVI-D", 4 => "DVI-A", 5 => "Composite", 6 => "SVIDEO", 7 => "LVDS", 8 => "Component",
        9 => "DIN", 10 => "DP", 11 => "HDMI-A", 12 => "HDMI-B", 13 => "TV", 14 => "eDP", 15 => "Virtual", 16 => "DSI",
        17 => "DPI", 18 => "Writeback", 19 => "SPI", 20 => "USB", _ => "Unknown",
    }
}

// ---------------------------------------------------------------------------------------------
// Properties

/// A property's definition: its id and what values it takes.
#[derive(Clone, Debug)]
pub struct PropInfo {
    pub id: u32,
    pub name: String,
    pub immutable: bool,
    /// An enum's (or bitmask's) names with their values.
    pub enums: Vec<(String, u64)>,
    /// A range's bounds.
    pub range: Option<(u64, u64)>,
}

/// An object's properties by name, with their values now.
#[derive(Clone, Debug, Default)]
pub struct Props {
    pub values: HashMap<String, (PropInfo, u64)>,
}

impl Props {
    pub fn get(&self, name: &str) -> Option<&(PropInfo, u64)> { self.values.get(name) }
    pub fn value(&self, name: &str) -> Option<u64> { self.values.get(name).map(|(_, v)| *v) }
    pub fn has(&self, name: &str) -> bool { self.values.contains_key(name) }

    /// The value of the enum `name` called `value`.
    pub fn enum_value(&self, name: &str, value: &str) -> Option<u64> {
        self.values.get(name)?.0.enums.iter().find(|(n, _)| n == value).map(|(_, v)| *v)
    }
}

// ---------------------------------------------------------------------------------------------
// The card

/// A DRM card through a file descriptor the caller owns and keeps open.
pub struct Card {
    fd: RawFd,
    /// Property definitions by id, read once.
    infos: std::cell::RefCell<HashMap<u32, PropInfo>>,
}

/// Something the card holds for this process that goes with it: a framebuffer and the GEM
/// handles of its buffers.
pub struct Framebuffer {
    pub id: u32,
    handles: Vec<u32>,
}

/// A dumb buffer mapped into memory.
pub struct DumbBuffer {
    pub fb: u32,
    handle: u32,
    pub pitch: u32,
    pub width: u32,
    pub height: u32,
    pub map: *mut u8,
    size: usize,
}

/// How many connectors have a display connected, on card `index` (`SDL_KMSDRM_DEVICE_INDEX`) or
/// every card in /dev/dri; None when no card could be read. Reads only: no DRM master needed.
pub fn connected_displays(index: Option<u32>) -> Option<usize> {
    use std::os::fd::AsRawFd;
    let mut read_any = false;
    let mut count = 0;
    for entry in std::fs::read_dir("/dev/dri").ok()?.flatten() {
        let name = entry.file_name();
        let Some(number) = name.to_str().and_then(|n| n.strip_prefix("card")).and_then(|n| n.parse::<u32>().ok()) else { continue };
        if index.is_some_and(|wanted| wanted != number) { continue; }
        let Ok(file) = std::fs::File::open(entry.path()) else { continue };
        let card = Card { fd: file.as_raw_fd(), infos: Default::default() };
        let Ok((_, connectors)) = card.resources() else { continue };
        read_any = true;
        count += connectors.iter().filter(|&&id| card.connector(id).is_ok_and(|(conn, _)| conn.connection == CONNECTED)).count();
    }
    read_any.then_some(count)
}

impl Card {
    /// The card behind `fd`, with atomic modesetting and universal planes asked for (SDL 3.4
    /// turns them off on its descriptor whenever it drops DRM master).
    pub fn new(fd: RawFd) -> Result<Self, String> {
        let card = Self { fd, infos: Default::default() };
        card.set_client_cap(CLIENT_CAP_UNIVERSAL_PLANES, 1).map_err(|e| format!("universal planes: {e}"))?;
        card.set_client_cap(CLIENT_CAP_ATOMIC, 1).map_err(|e| format!("atomic modesetting is unsupported: {e}"))?;
        Ok(card)
    }

    fn set_client_cap(&self, capability: u64, value: u64) -> std::io::Result<()> {
        let mut cap = Cap { capability, value };
        ioctl(self.fd, SET_CLIENT_CAP, &mut cap)
    }

    fn cap(&self, capability: u64) -> bool {
        let mut cap = Cap { capability, value: 0 };
        ioctl(self.fd, GET_CAP, &mut cap).is_ok() && cap.value != 0
    }

    /// Framebuffers take modifiers.
    pub fn modifiers(&self) -> bool { self.cap(CAP_ADDFB2_MODIFIERS) }

    /// Atomic commits can be async (tearing) flips (Linux 6.8).
    pub fn atomic_async(&self) -> bool { self.cap(CAP_ATOMIC_ASYNC_PAGE_FLIP) }

    fn info(&self, id: u32) -> std::io::Result<PropInfo> {
        if let Some(info) = self.infos.borrow().get(&id) { return Ok(info.clone()); }
        let mut query = Property { prop_id: id, ..Default::default() };
        ioctl(self.fd, GETPROPERTY, &mut query)?;
        let mut values = vec![0u64; query.count_values as usize];
        let mut enums = vec![PropertyEnum::default(); query.count_enum_blobs as usize];
        query.values_ptr = values.as_mut_ptr() as u64;
        query.enum_blob_ptr = enums.as_mut_ptr() as u64;
        ioctl(self.fd, GETPROPERTY, &mut query)?;
        values.truncate(query.count_values as usize);
        let flags = query.flags;
        let is_enum = flags & (PROP_ENUM | PROP_BITMASK) != 0;
        let enum_count = (query.count_enum_blobs as usize).min(enums.len());
        let info = PropInfo {
            id,
            name: name_of(&query.name),
            immutable: flags & PROP_IMMUTABLE != 0,
            enums: if is_enum { enums[..enum_count].iter().map(|e| (name_of(&e.name), e.value)).collect() } else { Vec::new() },
            range: (flags & PROP_RANGE != 0 || flags & 0x0000_ffc0 == PROP_SIGNED_RANGE)
                .then(|| (values.first().copied().unwrap_or(0), values.get(1).copied().unwrap_or(0))),
        };
        self.infos.borrow_mut().insert(id, info.clone());
        Ok(info)
    }

    /// An object's properties by name (`kind`: one of the OBJECT_ constants).
    pub fn props(&self, id: u32, kind: u32) -> std::io::Result<Props> {
        let mut query = ObjProperties { obj_id: id, obj_type: kind, ..Default::default() };
        ioctl(self.fd, OBJ_GETPROPERTIES, &mut query)?;
        let mut ids = vec![0u32; query.count_props as usize];
        let mut values = vec![0u64; ids.len()];
        query.props_ptr = ids.as_mut_ptr() as u64;
        query.prop_values_ptr = values.as_mut_ptr() as u64;
        ioctl(self.fd, OBJ_GETPROPERTIES, &mut query)?;
        let count = (query.count_props as usize).min(ids.len());
        let mut props = Props::default();
        for (&prop, &value) in ids[..count].iter().zip(&values[..count]) {
            let info = self.info(prop)?;
            props.values.insert(info.name.clone(), (info, value));
        }
        Ok(props)
    }

    /// A property blob's bytes.
    pub fn blob(&self, id: u32) -> std::io::Result<Vec<u8>> {
        let mut query = GetBlob { blob_id: id, ..Default::default() };
        ioctl(self.fd, GETPROPBLOB, &mut query)?;
        let mut data = vec![0u8; query.length as usize];
        query.data = data.as_mut_ptr() as u64;
        ioctl(self.fd, GETPROPBLOB, &mut query)?;
        data.truncate(query.length as usize);
        Ok(data)
    }

    pub fn create_blob(&self, data: &[u8]) -> std::io::Result<u32> {
        let mut create = CreateBlob { data: data.as_ptr() as u64, length: data.len() as u32, blob_id: 0 };
        ioctl(self.fd, CREATEPROPBLOB, &mut create)?;
        Ok(create.blob_id)
    }

    pub fn destroy_blob(&self, id: u32) {
        if id != 0 {
            let mut destroy = DestroyBlob { blob_id: id };
            let _ = ioctl(self.fd, DESTROYPROPBLOB, &mut destroy);
        }
    }

    /// A mode as a blob for a CRTC's `MODE_ID`.
    pub fn mode_blob(&self, mode: &ModeInfo) -> std::io::Result<u32> {
        self.create_blob(mode.bytes())
    }

    fn resources(&self) -> std::io::Result<(Vec<u32>, Vec<u32>)> {
        let mut res = CardRes::default();
        ioctl(self.fd, GETRESOURCES, &mut res)?;
        let mut crtcs = vec![0u32; res.count_crtcs as usize];
        let mut connectors = vec![0u32; res.count_connectors as usize];
        let mut encoders = vec![0u32; res.count_encoders as usize];
        let mut fbs = vec![0u32; res.count_fbs as usize];
        res.crtc_id_ptr = crtcs.as_mut_ptr() as u64;
        res.connector_id_ptr = connectors.as_mut_ptr() as u64;
        res.encoder_id_ptr = encoders.as_mut_ptr() as u64;
        res.fb_id_ptr = fbs.as_mut_ptr() as u64;
        ioctl(self.fd, GETRESOURCES, &mut res)?;
        crtcs.truncate(res.count_crtcs as usize);
        connectors.truncate(res.count_connectors as usize);
        Ok((crtcs, connectors))
    }

    /// A connector's modes and its encoder, connected or not.
    fn connector(&self, id: u32) -> std::io::Result<(Connector, Vec<ModeInfo>)> {
        let mut conn = Connector { connector_id: id, ..Default::default() };
        ioctl(self.fd, GETCONNECTOR, &mut conn)?;
        let mut modes = vec![ModeInfo::default(); conn.count_modes as usize];
        let mut encoders = vec![0u32; conn.count_encoders as usize];
        let mut props = vec![0u32; conn.count_props as usize];
        let mut values = vec![0u64; conn.count_props as usize];
        conn.modes_ptr = modes.as_mut_ptr() as u64;
        conn.encoders_ptr = encoders.as_mut_ptr() as u64;
        conn.props_ptr = props.as_mut_ptr() as u64;
        conn.prop_values_ptr = values.as_mut_ptr() as u64;
        ioctl(self.fd, GETCONNECTOR, &mut conn)?;
        modes.truncate(conn.count_modes as usize);
        Ok((conn, modes))
    }

    /// The connectors driven by a CRTC, with the CRTC's mode, and each one's mode list.
    pub fn outputs(&self) -> std::io::Result<Vec<(Output, ModeInfo, Vec<ModeInfo>)>> {
        let (crtcs, connectors) = self.resources()?;
        let mut found = Vec::new();
        for id in connectors {
            let (conn, modes) = self.connector(id)?;
            if conn.connection != CONNECTED || conn.encoder_id == 0 { continue; }
            let mut encoder = Encoder { encoder_id: conn.encoder_id, ..Default::default() };
            if ioctl(self.fd, GETENCODER, &mut encoder).is_err() || encoder.crtc_id == 0 { continue; }
            let mut crtc = Crtc { crtc_id: encoder.crtc_id, ..Default::default() };
            ioctl(self.fd, GETCRTC, &mut crtc)?;
            if crtc.mode_valid == 0 { continue; }
            let Some(index) = crtcs.iter().position(|&c| c == crtc.crtc_id) else { continue };
            let output = Output {
                connector_id: id,
                name: format!("{}-{}", connector_type_name(conn.connector_type), conn.connector_type_id),
                crtc_id: crtc.crtc_id,
                crtc_index: index as u32,
                width: u32::from(crtc.mode.hdisplay),
                height: u32::from(crtc.mode.vdisplay),
            };
            found.push((output, crtc.mode, modes));
        }
        Ok(found)
    }

    /// Every plane with what it takes (`IN_FORMATS` where it has it) and its zpos.
    pub fn planes(&self) -> std::io::Result<Vec<(Plane, Props)>> {
        let mut res = PlaneRes::default();
        ioctl(self.fd, GETPLANERESOURCES, &mut res)?;
        let mut ids = vec![0u32; res.count_planes as usize];
        res.plane_id_ptr = ids.as_mut_ptr() as u64;
        ioctl(self.fd, GETPLANERESOURCES, &mut res)?;
        ids.truncate(res.count_planes as usize);
        let mut planes = Vec::new();
        for id in ids {
            let mut query = GetPlane { plane_id: id, ..Default::default() };
            ioctl(self.fd, GETPLANE, &mut query)?;
            let mut formats = vec![0u32; query.count_format_types as usize];
            query.format_type_ptr = formats.as_mut_ptr() as u64;
            ioctl(self.fd, GETPLANE, &mut query)?;
            formats.truncate(query.count_format_types as usize);
            let props = self.props(id, OBJECT_PLANE)?;
            let listed = props
                .value("IN_FORMATS")
                .filter(|&blob| blob != 0)
                .and_then(|blob| self.blob(blob as u32).ok())
                .and_then(|bytes| plan::parse_in_formats(&bytes).ok());
            let formats = listed.unwrap_or_else(|| formats.into_iter().map(|f| (f, Vec::new())).collect());
            let zpos = props.get("zpos").map(|(info, value)| {
                let (min, max) = info.range.unwrap_or((*value, *value));
                Zpos { min, max, value: *value, immutable: info.immutable }
            });
            let plane = Plane {
                id,
                kind: PlaneKind::of(props.value("type").unwrap_or(0)),
                possible_crtcs: query.possible_crtcs,
                formats,
                zpos,
                crtc_id: query.crtc_id,
                color_props: props.has("COLOR_ENCODING") && props.has("COLOR_RANGE"),
            };
            planes.push((plane, props));
        }
        Ok(planes)
    }

    /// A framebuffer of DMA-BUFs (`fds` the objects of `layout`): each object becomes a GEM handle
    /// of this card, closed with the framebuffer.
    pub fn add_dmabuf_fb(&self, layout: &plan::FbLayout, fds: &[i32]) -> std::io::Result<Framebuffer> {
        let mut handles: Vec<u32> = Vec::new();
        let mut object_handles = Vec::new();
        for &fd in fds {
            let mut prime = PrimeHandle { fd, ..Default::default() };
            if let Err(error) = ioctl(self.fd, PRIME_FD_TO_HANDLE, &mut prime) {
                self.close_handles(&handles);
                return Err(error);
            }
            if !handles.contains(&prime.handle) { handles.push(prime.handle); }
            object_handles.push(prime.handle);
        }
        let mut cmd = FbCmd2 { width: layout.width, height: layout.height, pixel_format: layout.format, ..Default::default() };
        let modifiers = layout.modifier != plan::MOD_INVALID;
        if modifiers { cmd.flags = FB_MODIFIERS; }
        for (i, plane) in layout.planes.iter().enumerate().take(4) {
            let Some(&handle) = object_handles.get(plane.object) else {
                self.close_handles(&handles);
                return Err(std::io::Error::other(format!("no object {}", plane.object)));
            };
            cmd.handles[i] = handle;
            cmd.pitches[i] = plane.pitch;
            cmd.offsets[i] = plane.offset;
            if modifiers { cmd.modifier[i] = layout.modifier; }
        }
        if let Err(error) = ioctl(self.fd, ADDFB2, &mut cmd) {
            self.close_handles(&handles);
            return Err(error);
        }
        Ok(Framebuffer { id: cmd.fb_id, handles })
    }

    fn close_handles(&self, handles: &[u32]) {
        for &handle in handles {
            let mut close = GemClose { handle, pad: 0 };
            let _ = ioctl(self.fd, GEM_CLOSE, &mut close);
        }
    }

    pub fn remove_fb(&self, fb: Framebuffer) {
        let mut id = fb.id;
        let _ = ioctl(self.fd, RMFB, &mut id);
        self.close_handles(&fb.handles);
    }

    /// A 32-bit dumb buffer of `format` (ARGB8888 or XRGB8888) mapped into memory, cleared.
    pub fn dumb_buffer(&self, width: u32, height: u32, format: u32) -> std::io::Result<DumbBuffer> {
        let mut create = CreateDumb { width, height, bpp: 32, ..Default::default() };
        ioctl(self.fd, CREATE_DUMB, &mut create)?;
        let destroy = |handle: u32| {
            let mut d = DestroyDumb { handle };
            let _ = ioctl(self.fd, DESTROY_DUMB, &mut d);
        };
        let mut map = MapDumb { handle: create.handle, ..Default::default() };
        if let Err(error) = ioctl(self.fd, MAP_DUMB, &mut map) {
            destroy(create.handle);
            return Err(error);
        }
        // SAFETY: mapping the dumb buffer at the offset the kernel gave for it, its whole size.
        let pointer = unsafe {
            libc::mmap(std::ptr::null_mut(), create.size as usize, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_SHARED, self.fd, map.offset as libc::off_t)
        };
        if pointer == libc::MAP_FAILED {
            let error = std::io::Error::last_os_error();
            destroy(create.handle);
            return Err(error);
        }
        // SAFETY: the mapping just made, `size` bytes.
        unsafe { std::ptr::write_bytes(pointer.cast::<u8>(), 0, create.size as usize) };
        let mut cmd = FbCmd2 { width, height, pixel_format: format, ..Default::default() };
        cmd.handles[0] = create.handle;
        cmd.pitches[0] = create.pitch;
        if let Err(error) = ioctl(self.fd, ADDFB2, &mut cmd) {
            // SAFETY: the mapping made above.
            unsafe { libc::munmap(pointer, create.size as usize) };
            destroy(create.handle);
            return Err(error);
        }
        Ok(DumbBuffer { fb: cmd.fb_id, handle: create.handle, pitch: create.pitch, width, height, map: pointer.cast(), size: create.size as usize })
    }

    pub fn destroy_dumb(&self, buffer: DumbBuffer) {
        let mut fb = buffer.fb;
        let _ = ioctl(self.fd, RMFB, &mut fb);
        // SAFETY: the buffer's own mapping, unused from here.
        unsafe { libc::munmap(buffer.map.cast(), buffer.size) };
        let mut destroy = DestroyDumb { handle: buffer.handle };
        let _ = ioctl(self.fd, DESTROY_DUMB, &mut destroy);
    }

    /// One atomic commit of `sets` (object, property, value), with `flags` (TEST_ONLY,
    /// ALLOW_MODESET, PAGE_FLIP_ASYNC). Blocking: it returns once the commit is on the screen,
    /// as a V-Sync present returns.
    pub fn commit(&self, sets: &[(u32, u32, u64)], flags: u32) -> std::io::Result<()> {
        // Grouped by object, in the order objects first appear.
        let mut objects: Vec<u32> = Vec::new();
        for (object, ..) in sets {
            if !objects.contains(object) { objects.push(*object); }
        }
        let mut counts = Vec::with_capacity(objects.len());
        let mut props = Vec::with_capacity(sets.len());
        let mut values = Vec::with_capacity(sets.len());
        for object in &objects {
            let mine: Vec<&(u32, u32, u64)> = sets.iter().filter(|(o, ..)| o == object).collect();
            counts.push(mine.len() as u32);
            for (_, prop, value) in mine {
                props.push(*prop);
                values.push(*value);
            }
        }
        let mut atomic = Atomic {
            flags,
            count_objs: objects.len() as u32,
            objs_ptr: objects.as_ptr() as u64,
            count_props_ptr: counts.as_ptr() as u64,
            props_ptr: props.as_ptr() as u64,
            prop_values_ptr: values.as_ptr() as u64,
            ..Default::default()
        };
        ioctl(self.fd, ATOMIC, &mut atomic)
    }
}
