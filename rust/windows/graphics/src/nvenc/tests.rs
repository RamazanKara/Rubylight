//! A driver function table exercises the real session owner and submission
//! path without requiring NVIDIA hardware. Hardware decode is a separate test.

use super::*;
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
};
use tuning_tests::native_444;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Threading::SetEvent;

#[derive(Default)]
struct Buffer {
    bytes: Vec<u8>,
    timestamp: u64,
    idr: bool,
    locked: bool,
    metadata: Option<(*const MASTERING_DISPLAY_INFO, u32)>,
    light: Option<(*const CONTENT_LIGHT_LEVEL, u16, u16)>,
    /// The picture's QP delta map, which the block statistics reflect.
    deltas: Option<Vec<i8>>,
}
#[derive(Default)]
struct Driver {
    opened: Vec<ApiVersion>,
    destroyed: Vec<ApiVersion>,
    active: HashSet<usize>,
    resources: HashMap<usize, NV_ENC_BUFFER_FORMAT>,
    mapped: HashSet<usize>,
    outputs: HashMap<usize, Box<Buffer>>,
    events: HashSet<usize>,
    calls: Vec<&'static str>,
    invalidated: Vec<u64>,
    initialized: Option<(NV_ENC_INITIALIZE_PARAMS, NV_ENC_CONFIG)>,
    reconfigured: Option<(NV_ENC_RECONFIGURE_PARAMS, NV_ENC_CONFIG)>,
    reject_open: Option<ApiVersion>,
    /// What every open answers, as a driver past its session limit does.
    open_status: Option<NVENCSTATUS>,
    reject_initialize: Option<ApiVersion>,
    /// D3D12 `nvEncInitializeEncoder` refuses this `bufferFormat` with `INVALID_PARAM`.
    reject_d3d12_format: Option<NV_ENC_BUFFER_FORMAT>,
    reject_output: bool,
    reject_reconfigure: bool,
    /// Reconfigures without a reset are refused.
    reject_in_place: bool,
    /// Unregistering an input fails and keeps it registered.
    reject_unregister: bool,
    /// Registering a completion event fails, after the slot made its handle.
    reject_register_event: bool,
    reject_invalidate: Option<u64>,
    reject_submit: bool,
    asynchronous: bool,
    no_rfi: bool,
    no_multiple_refs: bool,
    preset_fallback: bool,
    preset_hdr_metadata: bool,
    lock_busy: usize,
    /// Locks of a keyframe that return busy first.
    keyframe_busy: usize,
    need_more_input: bool,
    destroy_busy: bool,
    destroy_status: Option<NVENCSTATUS>,
    /// NV_ENC_CAPS_NUM_ENCODER_ENGINES; 0 reads as 1.
    engines: i32,
    /// NV_ENC_CAPS_SUPPORTED_RATECONTROL_MODES, when not every mode.
    rate_modes: Option<i32>,
    /// The codecs nvEncGetEncodeGUIDs lists, when not all three.
    codecs: Option<Vec<u8>>,
    /// NV_ENC_CAPS_MB_PER_SEC_MAX.
    macroblocks: i32,
    /// NV_ENC_CAPS_SUPPORT_SUBFRAME_READBACK is 0.
    no_subframe: bool,
    /// NV_ENC_CAPS_SUPPORT_DYN_BITRATE_CHANGE is 0.
    no_dynamic_bitrate: bool,
    /// NV_ENC_CAPS_SUPPORT_YUV444_ENCODE is 0.
    no_yuv444: bool,
    /// NV_ENC_CAPS_SUPPORT_10BIT_ENCODE is 0.
    no_10bit: bool,
    /// Bytes in each picture, at least the 8 of its timestamp.
    picture: usize,
    /// Subframe locks before a picture is done: the slices and bytes each
    /// returns.
    partial: VecDeque<(u32, usize)>,
    /// Subframe locks report slice offsets past the bytes written.
    bad_offsets: bool,
    /// A D3D12 session: pictures pass resource structures with fence
    /// points, and the driver signals the output fence, which reads `fence`,
    /// as each is written unless `hold_fence`.
    d3d12: bool,
    fence: u64,
    hold_fence: bool,
    /// Each D3D12 picture's wait fence, wait value and signal value.
    fence_points: Vec<(usize, u64, u64)>,
    /// Each `nvEncRegisterResource` call's optional D3D12 input wait fence and value.
    register_fence_points: Vec<Option<(usize, u64)>>,
    /// Each output fence value an event was armed for.
    notified: Vec<u64>,
    /// Reading the output fence fails, as on a removed device.
    removed: bool,
    /// Each picture's QP delta map.
    maps: Vec<Option<Vec<i8>>>,
    /// Each picture submission's parameters, as passed.
    pictures: Vec<NV_ENC_PIC_PARAMS>,
    /// Each EOS submission's parameters, as passed.
    eos_pictures: Vec<NV_ENC_PIC_PARAMS>,
    hold_event: bool,
    hold_eos_event: bool,
    eos_status: Option<NVENCSTATUS>,
    eos_delay_ms: u64,
    eos_signaled: Option<Arc<std::sync::atomic::AtomicBool>>,
    reconfigure_count: usize,
    lock_do_not_wait: Vec<u32>,
    cuda_streams: Vec<(usize, usize)>,
    reject_cuda_streams: bool,
}
/// The D3D12 output fence, whose value the fake driver keeps.
struct FakeFence;
const FAKE_FENCE: usize = 0xfe0;
/// The conversion fence D3D12 pictures wait for.
const CONVERSION_FENCE: usize = 0xc0;
impl Fence for FakeFence {
    fn raw(&self) -> *mut c_void {
        FAKE_FENCE as *mut c_void
    }
    fn completed(&self) -> Result<u64> {
        state(|d| {
            anyhow::ensure!(!d.removed, "the D3D12 device was removed");
            Ok(d.fence)
        })
    }
    fn notify(&self, value: u64, _event: HANDLE) -> Result<()> {
        state(|d| d.notified.push(value));
        Ok(())
    }
}
thread_local! { static DRIVER: RefCell<Driver> = RefCell::default(); }
fn state<T>(call: impl FnOnce(&mut Driver) -> T) -> T {
    DRIVER.with(|d| call(&mut d.borrow_mut()))
}
fn reset() {
    state(|d| *d = Driver::default());
}
fn version(value: u32) -> ApiVersion {
    ApiVersion((value & 0xff) as u8, (value >> 24 & 0xf) as u8)
}
unsafe fn handle(raw: *mut c_void) -> ApiVersion {
    // SAFETY: Callers pass a session handle that `open` boxed from an ApiVersion and `destroy` has
    // not freed.
    unsafe { *raw.cast::<ApiVersion>() }
}

use fake_driver::create;

fn session(config: &Negotiated, maximum: ApiVersion) -> Result<Session> {
    session_with(config, maximum, "")
}
fn session_with(config: &Negotiated, maximum: ApiVersion, settings: &str) -> Result<Session> {
    open_with(config, maximum, settings, None)
}
fn open_with(
    config: &Negotiated,
    maximum: ApiVersion,
    settings: &str,
    fence: Option<Rc<dyn Fence>>,
) -> Result<Session> {
    let tuning = Tuning::new(&Config::parse(settings)?, config)?;
    open_tuned(config, maximum, &tuning, fence)
}
fn open_tuned(
    config: &Negotiated,
    maximum: ApiVersion,
    tuning: &Tuning,
    fence: Option<Rc<dyn Fence>>,
) -> Result<Session> {
    Session::open(
        Arc::new(Runtime {
            _library: None,
            create,
            maximum,
        }),
        ptr::dangling_mut::<c_void>(),
        _NV_ENC_DEVICE_TYPE_NV_ENC_DEVICE_TYPE_DIRECTX,
        None,
        fence,
        config,
        tuning,
    )
}
/// A session on the D3D12 conversion's device.
fn d3d12_session(config: &Negotiated, maximum: ApiVersion, settings: &str) -> Result<Session> {
    state(|d| d.d3d12 = true);
    open_with(config, maximum, settings, Some(Rc::new(FakeFence)))
}
/// A D3D12 conversion target's slot, with its READBACK buffer.
fn d3d12_input(session: &mut Session, key: usize) -> Result<usize> {
    d3d12_input_waiting(session, key, None)
}
fn d3d12_input_waiting(
    session: &mut Session,
    key: usize,
    wait: Option<(*mut c_void, u64)>,
) -> Result<usize> {
    session.slot(Input {
        key,
        raw: key as *mut c_void,
        kind: _NV_ENC_INPUT_RESOURCE_TYPE_NV_ENC_INPUT_RESOURCE_TYPE_DIRECTX,
        pitch: 0,
        _texture: None,
        cuda: None,
        d3d12: Some(D3d12Input {
            _resource: None,
            wait,
            output: (key | 0x1000) as *mut c_void,
            size: 4096,
            _output: None,
        }),
    })
}
/// A frame of the D3D12 conversion, ready once its fence reaches `value`.
fn converted(value: u64) -> Option<Kept> {
    Some(Kept::D3d12 {
        _texture: None,
        _fence: None,
        fence: CONVERSION_FENCE as *mut c_void,
        value,
    })
}
fn initialized() -> (NV_ENC_INITIALIZE_PARAMS, NV_ENC_CONFIG) {
    state(|d| d.initialized.unwrap())
}
fn same_preset(guid: GUID, preset: usize) -> bool {
    same_guid(&guid, &preset_guid(preset))
}
fn input(session: &mut Session, key: usize) -> Result<usize> {
    session.slot(Input {
        key,
        raw: key as *mut c_void,
        kind: _NV_ENC_INPUT_RESOURCE_TYPE_NV_ENC_INPUT_RESOURCE_TYPE_DIRECTX,
        pitch: 0,
        _texture: None,
        cuda: None,
        d3d12: None,
    })
}
fn assert_clean() {
    state(|d| {
        assert!(d.active.is_empty());
        assert!(d.resources.is_empty());
        assert!(d.outputs.is_empty());
        assert!(d.events.is_empty());
        assert!(d.mapped.is_empty());
    });
}
/// The slices a frame has when the session reports slice offsets.
fn configured_slices(driver: &Driver) -> u32 {
    let (initialize, config) = driver.initialized.as_ref().unwrap();
    if initialize.reportSliceOffsets() == 0 {
        return 0;
    }
    match state_codec(driver) {
        // SAFETY: An H.264 session fills this union member; it holds only integers and pointers.
        0 => unsafe { config.encodeCodecConfig.h264Config.sliceModeData },
        // SAFETY: An HEVC session fills this union member.
        1 => unsafe { config.encodeCodecConfig.hevcConfig.sliceModeData },
        _ => 0,
    }
}
fn state_codec(driver: &Driver) -> u8 {
    let guid = driver.initialized.as_ref().unwrap().0.encodeGUID;
    if guid.Data1 == NV_ENC_CODEC_H264_GUID.Data1 {
        0
    } else if guid.Data1 == NV_ENC_CODEC_HEVC_GUID.Data1 {
        1
    } else {
        2
    }
}

mod d3d12_444;
mod d3d12_focus_tests;
mod d3d12_teardown;
pub(super) mod fake_driver;
mod hdr_tests;
mod lifecycle_tests;
mod native_tests;
mod opt_tests;
mod rate_cap_tests;
mod subframe_tests;
mod tuning_tests;
