//! Raw Wayland color-management-v1 and linux-dmabuf-v1 wire descriptors, listeners, and connection wrapper.

use std::ffi::{c_char, c_int, c_void, CStr};
use std::ptr::{null, null_mut};

use sdl3_sys::everything::*;

use crate::stream::wl_presentation::wayland::{
    Api, Interface, Message, Proxy, Queue, Shared, DISPLAY_GET_REGISTRY, MARSHAL_FLAG_DESTROY,
    REGISTRY_BIND,
};
use super::super::cm::{PRIMARIES_BT2020, TF_ST2084_PQ};
use super::super::{ColorSupport, DescriptionInfo, DmabufLog, PqParams};

const fn message(name: &'static CStr, signature: &'static CStr, types: *const *const Interface) -> Message {
    Message { name: name.as_ptr(), signature: signature.as_ptr(), types }
}

const fn interface(name: &'static CStr, version: c_int, methods: &'static [Message], events: &'static [Message]) -> Interface {
    Interface {
        name: name.as_ptr(),
        version,
        method_count: methods.len() as c_int,
        methods: if methods.is_empty() { null() } else { methods.as_ptr() },
        event_count: events.len() as c_int,
        events: if events.is_empty() { null() } else { events.as_ptr() },
    }
}

// color-management-v1.xml and linux-dmabuf-v1.xml (versions 1 and 4): what wayland-scanner
// would generate. Unused requests are listed so that each opcode is the protocol's.
static NULLS: Shared<[*const Interface; 8]> = Shared([null(); 8]);

static CM_SURFACE_TYPES: Shared<[*const Interface; 2]> = Shared([&CM_SURFACE_IF.0, null()]);
static CM_FEEDBACK_TYPES: Shared<[*const Interface; 2]> = Shared([&CM_FEEDBACK_IF.0, null()]);
static CM_PARAMS_TYPES: Shared<[*const Interface; 1]> = Shared([&PARAMS_IF.0]);
static CM_REQUESTS: Shared<[Message; 7]> = Shared([
    message(c"destroy", c"", NULLS.0.as_ptr()),
    message(c"get_output", c"no", NULLS.0.as_ptr()),
    message(c"get_surface", c"no", CM_SURFACE_TYPES.0.as_ptr()),
    message(c"get_surface_feedback", c"no", CM_FEEDBACK_TYPES.0.as_ptr()),
    message(c"create_icc_creator", c"n", NULLS.0.as_ptr()),
    message(c"create_parametric_creator", c"n", CM_PARAMS_TYPES.0.as_ptr()),
    message(c"create_windows_scrgb", c"n", NULLS.0.as_ptr()),
]);
static CM_EVENTS: Shared<[Message; 5]> = Shared([
    message(c"supported_intent", c"u", NULLS.0.as_ptr()),
    message(c"supported_feature", c"u", NULLS.0.as_ptr()),
    message(c"supported_tf_named", c"u", NULLS.0.as_ptr()),
    message(c"supported_primaries_named", c"u", NULLS.0.as_ptr()),
    message(c"done", c"", NULLS.0.as_ptr()),
]);
static CM_IF: Shared<Interface> = Shared(interface(c"wp_color_manager_v1", 1, &CM_REQUESTS.0, &CM_EVENTS.0));
pub(super) const CM_DESTROY: u32 = 0;
pub(super) const CM_GET_SURFACE: u32 = 2;
const CM_GET_SURFACE_FEEDBACK: u32 = 3;
const CM_CREATE_PARAMETRIC_CREATOR: u32 = 5;

static CM_SURFACE_REQUESTS: Shared<[Message; 3]> = Shared([
    message(c"destroy", c"", NULLS.0.as_ptr()),
    message(c"set_image_description", c"ou", NULLS.0.as_ptr()),
    message(c"unset_image_description", c"", NULLS.0.as_ptr()),
]);
pub(super) static CM_SURFACE_IF: Shared<Interface> = Shared(interface(c"wp_color_management_surface_v1", 1, &CM_SURFACE_REQUESTS.0, &[]));
pub(super) const SURFACE_DESTROY: u32 = 0;
pub(super) const SURFACE_SET_IMAGE_DESCRIPTION: u32 = 1;

static IMAGE_TYPES: Shared<[*const Interface; 1]> = Shared([&IMAGE_IF.0]);
static CM_FEEDBACK_REQUESTS: Shared<[Message; 3]> = Shared([
    message(c"destroy", c"", NULLS.0.as_ptr()),
    message(c"get_preferred", c"n", IMAGE_TYPES.0.as_ptr()),
    message(c"get_preferred_parametric", c"n", IMAGE_TYPES.0.as_ptr()),
]);
static CM_FEEDBACK_EVENTS: Shared<[Message; 1]> = Shared([message(c"preferred_changed", c"u", NULLS.0.as_ptr())]);
static CM_FEEDBACK_IF: Shared<Interface> = Shared(interface(c"wp_color_management_surface_feedback_v1", 1, &CM_FEEDBACK_REQUESTS.0, &CM_FEEDBACK_EVENTS.0));
pub(super) const FEEDBACK_DESTROY: u32 = 0;
pub(super) const FEEDBACK_GET_PREFERRED: u32 = 1;

static INFO_TYPES: Shared<[*const Interface; 1]> = Shared([&INFO_IF.0]);
static IMAGE_REQUESTS: Shared<[Message; 2]> = Shared([message(c"destroy", c"", NULLS.0.as_ptr()), message(c"get_information", c"n", INFO_TYPES.0.as_ptr())]);
static IMAGE_EVENTS: Shared<[Message; 2]> = Shared([message(c"failed", c"us", NULLS.0.as_ptr()), message(c"ready", c"u", NULLS.0.as_ptr())]);
pub(super) static IMAGE_IF: Shared<Interface> = Shared(interface(c"wp_image_description_v1", 1, &IMAGE_REQUESTS.0, &IMAGE_EVENTS.0));
pub(super) const IMAGE_DESTROY: u32 = 0;
pub(super) const IMAGE_GET_INFORMATION: u32 = 1;

static INFO_EVENTS: Shared<[Message; 11]> = Shared([
    message(c"done", c"", NULLS.0.as_ptr()),
    message(c"icc_file", c"hu", NULLS.0.as_ptr()),
    message(c"primaries", c"iiiiiiii", NULLS.0.as_ptr()),
    message(c"primaries_named", c"u", NULLS.0.as_ptr()),
    message(c"tf_power", c"u", NULLS.0.as_ptr()),
    message(c"tf_named", c"u", NULLS.0.as_ptr()),
    message(c"luminances", c"uuu", NULLS.0.as_ptr()),
    message(c"target_primaries", c"iiiiiiii", NULLS.0.as_ptr()),
    message(c"target_luminance", c"uu", NULLS.0.as_ptr()),
    message(c"target_max_cll", c"u", NULLS.0.as_ptr()),
    message(c"target_max_fall", c"u", NULLS.0.as_ptr()),
]);
pub(super) static INFO_IF: Shared<Interface> = Shared(interface(c"wp_image_description_info_v1", 1, &[], &INFO_EVENTS.0));

static PARAMS_REQUESTS: Shared<[Message; 10]> = Shared([
    message(c"create", c"n", IMAGE_TYPES.0.as_ptr()),
    message(c"set_tf_named", c"u", NULLS.0.as_ptr()),
    message(c"set_tf_power", c"u", NULLS.0.as_ptr()),
    message(c"set_primaries_named", c"u", NULLS.0.as_ptr()),
    message(c"set_primaries", c"iiiiiiii", NULLS.0.as_ptr()),
    message(c"set_luminances", c"uuu", NULLS.0.as_ptr()),
    message(c"set_mastering_display_primaries", c"iiiiiiii", NULLS.0.as_ptr()),
    message(c"set_mastering_luminance", c"uu", NULLS.0.as_ptr()),
    message(c"set_max_cll", c"u", NULLS.0.as_ptr()),
    message(c"set_max_fall", c"u", NULLS.0.as_ptr()),
]);
static PARAMS_IF: Shared<Interface> = Shared(interface(c"wp_image_description_creator_params_v1", 1, &PARAMS_REQUESTS.0, &[]));
const PARAMS_CREATE: u32 = 0;
const PARAMS_SET_TF_NAMED: u32 = 1;
const PARAMS_SET_PRIMARIES_NAMED: u32 = 3;
const PARAMS_SET_LUMINANCES: u32 = 5;
const PARAMS_SET_MAX_CLL: u32 = 8;

static DMABUF_FEEDBACK_TYPES: Shared<[*const Interface; 2]> = Shared([&DMABUF_FEEDBACK_IF.0, null()]);
static DMABUF_REQUESTS: Shared<[Message; 4]> = Shared([
    message(c"destroy", c"", NULLS.0.as_ptr()),
    message(c"create_params", c"n", NULLS.0.as_ptr()),
    message(c"get_default_feedback", c"4n", DMABUF_FEEDBACK_TYPES.0.as_ptr()),
    message(c"get_surface_feedback", c"4no", DMABUF_FEEDBACK_TYPES.0.as_ptr()),
]);
static DMABUF_EVENTS: Shared<[Message; 2]> = Shared([message(c"format", c"u", NULLS.0.as_ptr()), message(c"modifier", c"3uuu", NULLS.0.as_ptr())]);
pub(super) static DMABUF_IF: Shared<Interface> = Shared(interface(c"zwp_linux_dmabuf_v1", 4, &DMABUF_REQUESTS.0, &DMABUF_EVENTS.0));
pub(super) const DMABUF_DESTROY: u32 = 0;
pub(super) const DMABUF_GET_SURFACE_FEEDBACK: u32 = 3;

static DMABUF_FEEDBACK_REQUESTS: Shared<[Message; 1]> = Shared([message(c"destroy", c"", NULLS.0.as_ptr())]);
static DMABUF_FEEDBACK_EVENTS: Shared<[Message; 7]> = Shared([
    message(c"done", c"", NULLS.0.as_ptr()),
    message(c"format_table", c"hu", NULLS.0.as_ptr()),
    message(c"main_device", c"a", NULLS.0.as_ptr()),
    message(c"tranche_done", c"", NULLS.0.as_ptr()),
    message(c"tranche_target_device", c"a", NULLS.0.as_ptr()),
    message(c"tranche_formats", c"a", NULLS.0.as_ptr()),
    message(c"tranche_flags", c"u", NULLS.0.as_ptr()),
]);
pub(super) static DMABUF_FEEDBACK_IF: Shared<Interface> = Shared(interface(c"zwp_linux_dmabuf_feedback_v1", 4, &DMABUF_FEEDBACK_REQUESTS.0, &DMABUF_FEEDBACK_EVENTS.0));
pub(super) const DMABUF_FEEDBACK_DESTROY: u32 = 0;
const TRANCHE_SCANOUT: u32 = 1;

/// wl_array.
#[repr(C)]
struct WlArray {
    size: usize,
    alloc: usize,
    data: *mut c_void,
}

type Data = *mut c_void;
type Nothing = unsafe extern "C" fn(Data, *mut Proxy);
type OneU = unsafe extern "C" fn(Data, *mut Proxy, u32);
type TwoU = unsafe extern "C" fn(Data, *mut Proxy, u32, u32);
type ThreeU = unsafe extern "C" fn(Data, *mut Proxy, u32, u32, u32);
type EightI = unsafe extern "C" fn(Data, *mut Proxy, i32, i32, i32, i32, i32, i32, i32, i32);
type Fd = unsafe extern "C" fn(Data, *mut Proxy, i32, u32);
type Array = unsafe extern "C" fn(Data, *mut Proxy, *mut WlArray);

// Listeners, laid out as C does.
#[repr(C)]
struct RegistryListener {
    global: unsafe extern "C" fn(Data, *mut Proxy, u32, *const c_char, u32),
    global_remove: OneU,
}
#[repr(C)]
struct CmListener {
    intent: OneU,
    feature: OneU,
    tf: OneU,
    primaries: OneU,
    done: Nothing,
}
#[repr(C)]
struct ChangedListener {
    changed: OneU,
}
#[repr(C)]
pub(super) struct ImageListener {
    failed: unsafe extern "C" fn(Data, *mut Proxy, u32, *const c_char),
    ready: OneU,
}
#[repr(C)]
pub(super) struct InfoListener {
    done: Nothing,
    icc_file: Fd,
    primaries: EightI,
    primaries_named: OneU,
    tf_power: OneU,
    tf_named: OneU,
    luminances: ThreeU,
    target_primaries: EightI,
    target_luminance: TwoU,
    target_max_cll: OneU,
    target_max_fall: OneU,
}
#[repr(C)]
pub(super) struct DmabufListener {
    format: OneU,
    modifier: ThreeU,
}
#[repr(C)]
pub(super) struct DmabufFeedbackListener {
    done: Nothing,
    format_table: Fd,
    main_device: Array,
    tranche_done: Nothing,
    tranche_target_device: Array,
    tranche_formats: Array,
    tranche_flags: OneU,
}

struct RegistryData {
    globals: Vec<(u32, String, u32)>,
}
#[derive(Default)]
pub(super) struct CmData {
    pub(super) support: ColorSupport,
    done: bool,
}
#[derive(Default)]
pub(super) struct ChangedData {
    pub(super) changed: bool,
}
#[derive(Default)]
pub(super) struct ImageData {
    pub(super) state: Option<Result<(), String>>,
}
#[derive(Default)]
pub(super) struct InfoData {
    pub(super) info: DescriptionInfo,
    pub(super) done: bool,
}
#[derive(Default)]
pub(super) struct DmabufData {
    table: Vec<(u32, u64)>,
    tranches: Vec<(bool, Vec<(u32, u64)>)>,
    flags: u32,
    indices: Vec<u16>,
    pub(super) log: DmabufLog,
}

// SAFETY (all listeners): `data` is the box the listener was added with, which outlives its
// proxy; libwayland calls them from the dispatch of our private queue, on the thread
// dispatching it, never at the same time as anything else touching the box.
unsafe extern "C" fn registry_global(data: Data, _: *mut Proxy, name: u32, interface: *const c_char, version: u32) {
    let data = unsafe { &mut *data.cast::<RegistryData>() };
    let interface = unsafe { CStr::from_ptr(interface) }.to_string_lossy().into_owned();
    data.globals.push((name, interface, version));
}
unsafe extern "C" fn ignore_u(_: Data, _: *mut Proxy, _: u32) {}
unsafe extern "C" fn ignore_uuu(_: Data, _: *mut Proxy, _: u32, _: u32, _: u32) {}
unsafe extern "C" fn ignore_array(_: Data, _: *mut Proxy, _: *mut WlArray) {}
unsafe extern "C" fn cm_intent(data: Data, _: *mut Proxy, value: u32) {
    ColorSupport::add(unsafe { &mut (*data.cast::<CmData>()).support.intents }, value);
}
unsafe extern "C" fn cm_feature(data: Data, _: *mut Proxy, value: u32) {
    ColorSupport::add(unsafe { &mut (*data.cast::<CmData>()).support.features }, value);
}
unsafe extern "C" fn cm_tf(data: Data, _: *mut Proxy, value: u32) {
    ColorSupport::add(unsafe { &mut (*data.cast::<CmData>()).support.tfs }, value);
}
unsafe extern "C" fn cm_primaries(data: Data, _: *mut Proxy, value: u32) {
    ColorSupport::add(unsafe { &mut (*data.cast::<CmData>()).support.primaries }, value);
}
unsafe extern "C" fn cm_done(data: Data, _: *mut Proxy) {
    unsafe { (*data.cast::<CmData>()).done = true };
}
unsafe extern "C" fn preferred_changed(data: Data, _: *mut Proxy, _identity: u32) {
    unsafe { (*data.cast::<ChangedData>()).changed = true };
}
unsafe extern "C" fn image_failed(data: Data, _: *mut Proxy, cause: u32, message: *const c_char) {
    let message = if message.is_null() { String::new() } else { unsafe { CStr::from_ptr(message) }.to_string_lossy().into_owned() };
    unsafe { (*data.cast::<ImageData>()).state = Some(Err(format!("the compositor refused the description (cause {cause}: {message})"))) };
}
unsafe extern "C" fn image_ready(data: Data, _: *mut Proxy, _identity: u32) {
    unsafe { (*data.cast::<ImageData>()).state = Some(Ok(())) };
}
fn info<'a>(data: Data) -> &'a mut InfoData {
    // SAFETY: as for every listener.
    unsafe { &mut *data.cast::<InfoData>() }
}
unsafe extern "C" fn info_done(data: Data, _: *mut Proxy) {
    info(data).done = true;
}
unsafe extern "C" fn info_icc(data: Data, _: *mut Proxy, fd: i32, _size: u32) {
    info(data).info.icc = true;
    // SAFETY: the event hands us the descriptor.
    unsafe { libc::close(fd) };
}
#[allow(clippy::too_many_arguments)]
unsafe extern "C" fn info_primaries(data: Data, _: *mut Proxy, rx: i32, ry: i32, gx: i32, gy: i32, bx: i32, by: i32, wx: i32, wy: i32) {
    info(data).info.primaries = Some([rx, ry, gx, gy, bx, by, wx, wy]);
}
unsafe extern "C" fn info_primaries_named(data: Data, _: *mut Proxy, value: u32) {
    info(data).info.primaries_named = Some(value);
}
unsafe extern "C" fn info_tf_power(data: Data, _: *mut Proxy, value: u32) {
    info(data).info.tf_power = Some(value);
}
unsafe extern "C" fn info_tf_named(data: Data, _: *mut Proxy, value: u32) {
    info(data).info.tf_named = Some(value);
}
unsafe extern "C" fn info_luminances(data: Data, _: *mut Proxy, min: u32, max: u32, reference: u32) {
    info(data).info.luminances = Some((min, max, reference));
}
#[allow(clippy::too_many_arguments)]
unsafe extern "C" fn info_target_primaries(_: Data, _: *mut Proxy, _: i32, _: i32, _: i32, _: i32, _: i32, _: i32, _: i32, _: i32) {}
unsafe extern "C" fn info_target_luminance(data: Data, _: *mut Proxy, min: u32, max: u32) {
    info(data).info.target_luminance = Some((min, max));
}
unsafe extern "C" fn info_target_max_cll(data: Data, _: *mut Proxy, value: u32) {
    info(data).info.target_max_cll = Some(value);
}
fn dmabuf<'a>(data: Data) -> &'a mut DmabufData {
    // SAFETY: as for every listener.
    unsafe { &mut *data.cast::<DmabufData>() }
}
unsafe extern "C" fn feedback_done(data: Data, _: *mut Proxy) {
    let data = dmabuf(data);
    let tranches = std::mem::take(&mut data.tranches);
    data.log.add(&tranches);
}
unsafe extern "C" fn feedback_format_table(data: Data, _: *mut Proxy, fd: i32, size: u32) {
    let data = dmabuf(data);
    data.table.clear();
    let size = size as usize;
    if size > 0 {
        // SAFETY: the compositor's table, read only, unmapped before the descriptor closes;
        // 16 bytes per entry: format (u32), padding, modifier (u64).
        unsafe {
            let map = libc::mmap(null_mut(), size, libc::PROT_READ, libc::MAP_PRIVATE, fd, 0);
            if map != libc::MAP_FAILED {
                let bytes = std::slice::from_raw_parts(map.cast::<u8>(), size);
                data.table = bytes
                    .as_chunks::<16>()
                    .0
                    .iter()
                    .map(|entry| (u32::from_ne_bytes(entry[..4].try_into().unwrap()), u64::from_ne_bytes(entry[8..].try_into().unwrap())))
                    .collect();
                libc::munmap(map, size);
            }
        }
    }
    // SAFETY: the event hands us the descriptor.
    unsafe { libc::close(fd) };
}
unsafe extern "C" fn feedback_tranche_done(data: Data, _: *mut Proxy) {
    let data = dmabuf(data);
    let formats = data.indices.drain(..).filter_map(|index| data.table.get(usize::from(index)).copied()).collect();
    data.tranches.push((data.flags & TRANCHE_SCANOUT != 0, formats));
    data.flags = 0;
}
unsafe extern "C" fn feedback_tranche_formats(data: Data, _: *mut Proxy, array: *mut WlArray) {
    // SAFETY: libwayland's array of u16 indices, valid during the call.
    let indices = unsafe {
        let array = &*array;
        if array.data.is_null() { &[][..] } else { std::slice::from_raw_parts(array.data.cast::<u16>(), array.size / 2) }
    };
    dmabuf(data).indices.extend_from_slice(indices);
}
unsafe extern "C" fn feedback_tranche_flags(data: Data, _: *mut Proxy, flags: u32) {
    dmabuf(data).flags = flags;
}

static REGISTRY_LISTENER: RegistryListener = RegistryListener { global: registry_global, global_remove: ignore_u };
static CM_LISTENER: CmListener = CmListener { intent: cm_intent, feature: cm_feature, tf: cm_tf, primaries: cm_primaries, done: cm_done };
static CHANGED_LISTENER: ChangedListener = ChangedListener { changed: preferred_changed };
pub(super) static IMAGE_LISTENER: ImageListener = ImageListener { failed: image_failed, ready: image_ready };
pub(super) static INFO_LISTENER: InfoListener = InfoListener {
    done: info_done,
    icc_file: info_icc,
    primaries: info_primaries,
    primaries_named: info_primaries_named,
    tf_power: info_tf_power,
    tf_named: info_tf_named,
    luminances: info_luminances,
    target_primaries: info_target_primaries,
    target_luminance: info_target_luminance,
    target_max_cll: info_target_max_cll,
    target_max_fall: ignore_u,
};
pub(super) static DMABUF_LISTENER: DmabufListener = DmabufListener { format: ignore_u, modifier: ignore_uuu };
pub(super) static DMABUF_FEEDBACK_LISTENER: DmabufFeedbackListener = DmabufFeedbackListener {
    done: feedback_done,
    format_table: feedback_format_table,
    main_device: ignore_array,
    tranche_done: feedback_tranche_done,
    tranche_target_device: ignore_array,
    tranche_formats: feedback_tranche_formats,
    tranche_flags: feedback_tranche_flags,
};

/// SDL's Wayland connection with a private event queue (as `wl_presentation::Feedback` has):
/// SDL's event thread never dispatches what is made here. Dropped after what it made.
pub(super) struct Conn {
    pub(super) api: Box<Api>,
    display: *mut c_void,
    pub(super) surface: *mut Proxy,
    queue: *mut Queue,
    registry: *mut Proxy,
    registry_data: Box<RegistryData>,
}

impl Conn {
    pub(super) fn open(window: *mut SDL_Window) -> Result<Self, String> {
        // SAFETY: a live window; the pointers are SDL's own Wayland objects.
        let (display, surface) = unsafe {
            let props = SDL_GetWindowProperties(window);
            (
                SDL_GetPointerProperty(props, SDL_PROP_WINDOW_WAYLAND_DISPLAY_POINTER, null_mut()),
                SDL_GetPointerProperty(props, SDL_PROP_WINDOW_WAYLAND_SURFACE_POINTER, null_mut()),
            )
        };
        if display.is_null() || surface.is_null() {
            return Err("the window is not a Wayland one".into());
        }
        let api = Box::new(Api::load()?);
        // SAFETY: SDL's live display; every object made here belongs to the private queue.
        unsafe {
            let queue = (api.create_queue)(display);
            if queue.is_null() {
                return Err("no Wayland event queue".into());
            }
            let mut conn = Conn { api, display, surface, queue, registry: null_mut(), registry_data: Box::new(RegistryData { globals: Vec::new() }) };
            let wrapper = (conn.api.create_wrapper)(display);
            if wrapper.is_null() {
                return Err("no Wayland display wrapper".into());
            }
            (conn.api.set_queue)(wrapper, queue);
            let version = (conn.api.get_version)(wrapper);
            conn.registry = (conn.api.marshal_flags)(wrapper, DISPLAY_GET_REGISTRY, conn.api.registry_interface, version, 0, null_mut::<c_void>());
            (conn.api.wrapper_destroy)(wrapper);
            if conn.registry.is_null() {
                return Err("no Wayland registry".into());
            }
            let data: *mut RegistryData = &mut *conn.registry_data;
            conn.listen(conn.registry, &REGISTRY_LISTENER, data.cast());
            if !conn.roundtrip() {
                return Err("the Wayland registry round trip failed".into());
            }
            Ok(conn)
        }
    }

    pub(super) fn globals(&self) -> &[(u32, String, u32)] {
        &self.registry_data.globals
    }

    /// Binds the global `interface` names at its version, at most `max_version`.
    ///
    /// # Safety
    /// `self.registry` must be live and `interface` a valid Wayland interface descriptor.
    pub(super) unsafe fn bind(&self, interface: &'static Interface, max_version: u32) -> Option<*mut Proxy> {
        // SAFETY: a static NUL-terminated name.
        let wanted = unsafe { CStr::from_ptr(interface.name) }.to_str().ok()?;
        let &(name, _, version) = self.globals().iter().find(|(_, have, _)| have == wanted)?;
        let version = version.min(max_version);
        // SAFETY: our live registry; wl_registry.bind's "usun".
        let bound = unsafe { (self.api.marshal_flags)(self.registry, REGISTRY_BIND, interface, version, 0, name, interface.name, version, null_mut::<c_void>()) };
        (!bound.is_null()).then_some(bound)
    }

    /// # Safety
    /// `proxy` must be live and `listener`/`data` valid for its interface and lifetime.
    pub(super) unsafe fn listen<L>(&self, proxy: *mut Proxy, listener: &'static L, data: *mut c_void) {
        // SAFETY: a live proxy of ours and the listener table of its interface.
        unsafe { (self.api.add_listener)(proxy, (listener as *const L).cast(), data) };
    }

    pub(super) fn version(&self, proxy: *mut Proxy) -> u32 {
        // SAFETY: a live proxy of ours.
        unsafe { (self.api.get_version)(proxy) }
    }

    pub(super) fn roundtrip(&self) -> bool {
        // SAFETY: SDL's live display and our queue.
        unsafe { (self.api.roundtrip_queue)(self.display, self.queue) >= 0 }
    }

    /// Reads what arrived without blocking and dispatches our queue.
    pub(super) fn pump(&self) {
        // SAFETY: SDL's live display and our queue; the read protocol shared with SDL's thread.
        unsafe {
            if (self.api.prepare_read_queue)(self.display, self.queue) == 0 {
                (self.api.flush)(self.display);
                let mut fd = libc::pollfd { fd: (self.api.get_fd)(self.display), events: libc::POLLIN, revents: 0 };
                if libc::poll(&mut fd, 1, 0) > 0 && fd.revents & libc::POLLIN != 0 {
                    (self.api.read_events)(self.display);
                } else {
                    (self.api.cancel_read)(self.display);
                }
            }
            (self.api.dispatch_queue_pending)(self.display, self.queue);
        }
    }

    pub(super) fn flush(&self) {
        // SAFETY: SDL's live display.
        unsafe { (self.api.flush)(self.display) };
    }

    /// Sends `proxy`'s destructor request `opcode` (0 for all of these protocols); null is none.
    pub(super) fn destroy(&self, proxy: *mut Proxy, opcode: u32) {
        if !proxy.is_null() {
            // SAFETY: a live proxy of ours, destroyed once.
            unsafe { (self.api.marshal_flags)(proxy, opcode, null(), (self.api.get_version)(proxy), MARSHAL_FLAG_DESTROY) };
        }
    }
}

impl Drop for Conn {
    fn drop(&mut self) {
        // SAFETY: our own objects, destroyed once, after those made from them.
        unsafe {
            if !self.registry.is_null() {
                (self.api.destroy)(self.registry);
            }
            (self.api.flush)(self.display);
            (self.api.queue_destroy)(self.queue);
        }
    }
}

/// A proxy with its listener's box.
pub(super) struct Object<T> {
    pub(super) proxy: *mut Proxy,
    pub(super) data: Box<T>,
}

impl<T: Default> Object<T> {
    /// # Safety
    /// `proxy` must be null or live on `conn` and match `listener`'s interface.
    pub(super) unsafe fn new<L>(conn: &Conn, proxy: *mut Proxy, listener: &'static L) -> Option<Self> {
        if proxy.is_null() {
            return None;
        }
        let mut data = Box::<T>::default();
        // SAFETY: the box outlives the proxy (destroyed through `Conn::destroy` first).
        unsafe { conn.listen(proxy, listener, (&mut *data as *mut T).cast()) };
        Some(Self { proxy, data })
    }

    pub(super) fn data(&mut self) -> *mut T {
        &mut *self.data
    }
}

/// wp_color_manager_v1 at version 1, with what it supports.
///
/// # Safety
/// `conn` must hold a live Wayland display and registry.
pub(super) unsafe fn manager(conn: &Conn) -> Result<Object<CmData>, String> {
    let proxy = unsafe { conn.bind(&CM_IF.0, 1) }.ok_or("the compositor offers no wp_color_manager_v1")?;
    let mut manager = unsafe { Object::<CmData>::new(conn, proxy, &CM_LISTENER) }.ok_or("wp_color_manager_v1 could not be bound")?;
    let data = manager.data();
    // SAFETY: the box, read between dispatches.
    if !conn.roundtrip() || !unsafe { (*data).done } {
        conn.destroy(manager.proxy, CM_DESTROY);
        return Err("wp_color_manager_v1 told nothing it supports".into());
    }
    Ok(manager)
}

/// # Safety
/// `conn` and `manager` must be live Wayland objects on the same queue.
pub(super) unsafe fn color_feedback(conn: &Conn, manager: &Object<CmData>) -> Result<Object<ChangedData>, String> {
    // SAFETY: a live manager; get_surface_feedback's "no" with SDL's surface.
    let proxy = unsafe { (conn.api.marshal_flags)(manager.proxy, CM_GET_SURFACE_FEEDBACK, &CM_FEEDBACK_IF.0, conn.version(manager.proxy), 0, null_mut::<c_void>(), conn.surface) };
    unsafe { Object::new(conn, proxy, &CHANGED_LISTENER) }.ok_or_else(|| "no colour surface feedback".into())
}

/// Waits a few round trips for an image description's ready or failed; on failure it is gone.
unsafe fn ready(conn: &Conn, image: Option<Object<ImageData>>) -> Result<Object<ImageData>, String> {
    let mut image = image.ok_or("no image description")?;
    let data = image.data();
    for _ in 0..4 {
        // SAFETY: the box, read between dispatches.
        match unsafe { (*data).state.clone() } {
            Some(Ok(())) => return Ok(image),
            Some(Err(why)) => {
                conn.destroy(image.proxy, IMAGE_DESTROY);
                return Err(why);
            }
            None if !conn.roundtrip() => break,
            None => {}
        }
    }
    conn.destroy(image.proxy, IMAGE_DESTROY);
    Err("the compositor did not answer for the image description".into())
}

/// The compositor's preferred description for the surface, and what it says.
///
/// # Safety
/// `conn` and `feedback` must be live Wayland objects on the same queue.
pub(super) unsafe fn preferred(conn: &Conn, feedback: &Object<ChangedData>) -> Result<(Object<ImageData>, DescriptionInfo), String> {
    // SAFETY: a live feedback object; get_preferred's "n".
    let proxy = unsafe { (conn.api.marshal_flags)(feedback.proxy, FEEDBACK_GET_PREFERRED, &IMAGE_IF.0, conn.version(feedback.proxy), 0, null_mut::<c_void>()) };
    let image = unsafe { ready(conn, Object::new(conn, proxy, &IMAGE_LISTENER)) }?;
    // SAFETY: a ready description; get_information's "n".
    let proxy = unsafe { (conn.api.marshal_flags)(image.proxy, IMAGE_GET_INFORMATION, &INFO_IF.0, conn.version(image.proxy), 0, null_mut::<c_void>()) };
    let Some(mut info) = (unsafe { Object::<InfoData>::new(conn, proxy, &INFO_LISTENER) }) else {
        conn.destroy(image.proxy, IMAGE_DESTROY);
        return Err("no description information".into());
    };
    let data = info.data();
    for _ in 0..4 {
        // SAFETY: the box, read between dispatches.
        if unsafe { (*data).done } || !conn.roundtrip() {
            break;
        }
    }
    // The info object ends with its done event; the client only frees its proxy.
    // SAFETY: our proxy, destroyed once.
    unsafe { (conn.api.destroy)(info.proxy) };
    // SAFETY: as above.
    let (done, described) = unsafe { ((*data).done, (*data).info) };
    if !done {
        conn.destroy(image.proxy, IMAGE_DESTROY);
        return Err("the compositor did not describe its preferred description".into());
    }
    Ok((image, described))
}

/// Asks for a PQ BT.2020 description with `params`' luminances; its ready or failed comes later.
///
/// # Safety
/// `conn` and `manager` must be live, with features checked by `hdr_label`.
pub(super) unsafe fn create_pq(conn: &Conn, manager: &Object<CmData>, params: PqParams) -> Result<Object<ImageData>, String> {
    let version = conn.version(manager.proxy);
    // SAFETY: a live manager whose features `hdr_label` checked; each request's signature.
    unsafe {
        let creator = (conn.api.marshal_flags)(manager.proxy, CM_CREATE_PARAMETRIC_CREATOR, &PARAMS_IF.0, version, 0, null_mut::<c_void>());
        if creator.is_null() {
            return Err("no parametric creator".into());
        }
        (conn.api.marshal_flags)(creator, PARAMS_SET_TF_NAMED, null(), version, 0, TF_ST2084_PQ);
        (conn.api.marshal_flags)(creator, PARAMS_SET_PRIMARIES_NAMED, null(), version, 0, PRIMARIES_BT2020);
        (conn.api.marshal_flags)(creator, PARAMS_SET_LUMINANCES, null(), version, 0, params.min_lum, params.max_lum, params.reference);
        if params.max_cll > 0 {
            (conn.api.marshal_flags)(creator, PARAMS_SET_MAX_CLL, null(), version, 0, params.max_cll);
        }
        // create is the creator's destructor.
        let proxy = (conn.api.marshal_flags)(creator, PARAMS_CREATE, &IMAGE_IF.0, version, MARSHAL_FLAG_DESTROY, null_mut::<c_void>());
        Object::new(conn, proxy, &IMAGE_LISTENER).ok_or_else(|| "no image description".into())
    }
}
