//! A driver function table exercises the real session owner and submission
//! path without requiring NVIDIA hardware. Hardware decode is a separate test.

use super::*;
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
};
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
    reject_initialize: Option<ApiVersion>,
    reject_output: bool,
    reject_reconfigure: bool,
    reject_invalidate: Option<u64>,
    reject_submit: bool,
    asynchronous: bool,
    no_rfi: bool,
    no_multiple_refs: bool,
    preset_fallback: bool,
    preset_hdr_metadata: bool,
    lock_busy: usize,
    need_more_input: bool,
    destroy_busy: bool,
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
    /// Each picture's QP delta map.
    maps: Vec<Option<Vec<i8>>>,
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
        Ok(state(|d| d.fence))
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

unsafe extern "C" fn create(table: *mut NV_ENCODE_API_FUNCTION_LIST) -> NVENCSTATUS {
    // SAFETY: `Session::open_version` passes its live, exclusive function table.
    let table = unsafe { &mut *table };
    assert_eq!(table.version, version(table.version).structure(2, false));
    table.nvEncOpenEncodeSessionEx = Some(open);
    table.nvEncGetEncodeCaps = Some(caps);
    table.nvEncGetEncodeGUIDCount = Some(guid_count);
    table.nvEncGetEncodeGUIDs = Some(guids);
    table.nvEncGetEncodePresetConfigEx = Some(preset_ex);
    table.nvEncGetEncodePresetConfig = Some(preset);
    table.nvEncInitializeEncoder = Some(initialize);
    table.nvEncRegisterResource = Some(register);
    table.nvEncUnregisterResource = Some(unregister);
    table.nvEncMapInputResource = Some(map);
    table.nvEncUnmapInputResource = Some(unmap);
    table.nvEncCreateBitstreamBuffer = Some(output);
    table.nvEncDestroyBitstreamBuffer = Some(destroy_output);
    table.nvEncRegisterAsyncEvent = Some(register_event);
    table.nvEncUnregisterAsyncEvent = Some(unregister_event);
    table.nvEncEncodePicture = Some(encode);
    table.nvEncLockBitstream = Some(lock);
    table.nvEncUnlockBitstream = Some(unlock);
    table.nvEncReconfigureEncoder = Some(reconfigure);
    table.nvEncInvalidateRefFrames = Some(invalidate);
    table.nvEncDestroyEncoder = Some(destroy);
    SUCCESS
}
unsafe extern "C" fn open(
    parameters: *mut NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS,
    raw: *mut *mut c_void,
) -> NVENCSTATUS {
    // SAFETY: The session passes a pointer to its live parameter struct for this call.
    let p = unsafe { &*parameters };
    let api = version(p.apiVersion);
    assert_eq!(p.version, api.structure(1, false));
    // SAFETY: `raw` points at the session's `raw` field, writable for this call.
    unsafe {
        *raw = Box::into_raw(Box::new(api)).cast();
    }
    state(|d| {
        d.opened.push(api);
        // SAFETY: `raw` was written just above and still points at the session's field.
        d.active.insert(unsafe { *raw } as usize);
        if d.reject_open == Some(api) {
            _NVENCSTATUS_NV_ENC_ERR_INVALID_VERSION
        } else {
            SUCCESS
        }
    })
}
unsafe extern "C" fn destroy(raw: *mut c_void) -> NVENCSTATUS {
    // SAFETY: `raw` is a live handle from `open`; it is freed only at the end of this call.
    let api = unsafe { handle(raw) };
    state(|d| {
        assert!(d.active.remove(&(raw as usize)));
        if d.destroy_busy {
            assert!(!d.mapped.is_empty());
            assert!(!d.resources.is_empty());
            assert!(!d.outputs.is_empty());
            assert!(!d.events.is_empty());
            assert!(!d.calls.iter().any(|call| matches!(
                *call,
                "unmap input" | "unregister event" | "destroy output" | "unregister input"
            )));
            // Model the driver stopping its worker and releasing native
            // registrations only when the session itself is destroyed.
            d.mapped.clear();
            d.resources.clear();
            d.outputs.clear();
            d.events.clear();
        }
        d.destroyed.push(api);
        d.calls.push("destroy session");
    });
    // SAFETY: `raw` came from Box::into_raw in `open`, and each session is destroyed once.
    drop(unsafe { Box::from_raw(raw.cast::<ApiVersion>()) });
    SUCCESS
}
unsafe extern "C" fn caps(
    raw: *mut c_void,
    _: GUID,
    parameters: *mut NV_ENC_CAPS_PARAM,
    value: *mut i32,
) -> NVENCSTATUS {
    // SAFETY: The session passes a pointer to its live parameter struct for this call.
    let p = unsafe { &*parameters };
    // SAFETY: `raw` is a live handle that `open` boxed and `destroy` has not freed.
    assert_eq!(p.version, unsafe { handle(raw) }.structure(1, false));
    // SAFETY: `value` points at the session's live local out-value.
    unsafe {
        *value = state(|d| match p.capsToQuery {
            _NV_ENC_CAPS_NV_ENC_CAPS_WIDTH_MAX | _NV_ENC_CAPS_NV_ENC_CAPS_HEIGHT_MAX => 8192,
            _NV_ENC_CAPS_NV_ENC_CAPS_ASYNC_ENCODE_SUPPORT => i32::from(d.asynchronous),
            _NV_ENC_CAPS_NV_ENC_CAPS_SUPPORT_REF_PIC_INVALIDATION => i32::from(!d.no_rfi),
            _NV_ENC_CAPS_NV_ENC_CAPS_SUPPORT_MULTIPLE_REF_FRAMES => i32::from(!d.no_multiple_refs),
            _NV_ENC_CAPS_NV_ENC_CAPS_NUM_ENCODER_ENGINES => d.engines.max(1),
            _NV_ENC_CAPS_NV_ENC_CAPS_SUPPORTED_RATECONTROL_MODES => d.rate_modes.unwrap_or(0x3f),
            _NV_ENC_CAPS_NV_ENC_CAPS_MB_PER_SEC_MAX => d.macroblocks,
            _NV_ENC_CAPS_NV_ENC_CAPS_SUPPORT_SUBFRAME_READBACK => i32::from(!d.no_subframe),
            _ => 1,
        });
    }
    SUCCESS
}
fn listed_codecs() -> Vec<GUID> {
    state(|d| d.codecs.clone())
        .unwrap_or(vec![0, 1, 2])
        .into_iter()
        .map(|codec| codec_guid(codec).unwrap())
        .collect()
}
unsafe extern "C" fn guid_count(_: *mut c_void, count: *mut u32) -> NVENCSTATUS {
    // SAFETY: `count` points at the session's live local out-value.
    unsafe { *count = listed_codecs().len() as u32 };
    SUCCESS
}
unsafe extern "C" fn guids(
    _: *mut c_void,
    out: *mut GUID,
    capacity: u32,
    count: *mut u32,
) -> NVENCSTATUS {
    let listed = listed_codecs();
    let written = listed.len().min(capacity as usize);
    // SAFETY: `out` holds `capacity` GUIDs and `count` is a live local out-value.
    unsafe {
        std::ptr::copy_nonoverlapping(listed.as_ptr(), out, written);
        *count = written as u32;
    }
    SUCCESS
}
unsafe extern "C" fn preset(
    raw: *mut c_void,
    codec: GUID,
    _: GUID,
    config: *mut NV_ENC_PRESET_CONFIG,
) -> NVENCSTATUS {
    // SAFETY: `raw` is a live handle that `open` boxed and `destroy` has not freed.
    let api = unsafe { handle(raw) };
    // SAFETY: The session passes an exclusive pointer to its live preset struct.
    let p = unsafe { &mut *config };
    assert_eq!(p.version, api.preset());
    assert_eq!(p.presetCfg.version, api.config());
    // A quality preset may enable B frames/lookahead. The host must override it.
    p.presetCfg.frameIntervalP = 4;
    p.presetCfg.rcParams.set_enableLookahead(1);
    p.presetCfg.rcParams.lookaheadDepth = 16;
    if api >= ApiVersion(13, 0) && state(|d| d.preset_hdr_metadata) {
        // SAFETY: The requested codec selects the corresponding plain-data union member.
        unsafe {
            if codec.Data1 == NV_ENC_CODEC_HEVC_GUID.Data1 {
                p.presetCfg
                    .encodeCodecConfig
                    .hevcConfig
                    .set_outputMasteringDisplay(1);
                p.presetCfg.encodeCodecConfig.hevcConfig.set_outputMaxCll(1);
            } else if codec.Data1 == NV_ENC_CODEC_AV1_GUID.Data1 {
                p.presetCfg
                    .encodeCodecConfig
                    .av1Config
                    .set_outputMasteringDisplay(1);
                p.presetCfg.encodeCodecConfig.av1Config.set_outputMaxCll(1);
            }
        }
    }
    state(|d| d.calls.push("preset"));
    SUCCESS
}
unsafe extern "C" fn preset_ex(
    raw: *mut c_void,
    codec: GUID,
    guid: GUID,
    tuning: NV_ENC_TUNING_INFO,
    config: *mut NV_ENC_PRESET_CONFIG,
) -> NVENCSTATUS {
    assert_eq!(
        tuning,
        NV_ENC_TUNING_INFO_NV_ENC_TUNING_INFO_ULTRA_LOW_LATENCY
    );
    if state(|d| d.preset_fallback) {
        _NVENCSTATUS_NV_ENC_ERR_UNSUPPORTED_PARAM
    } else {
        // SAFETY: The arguments are this call's own, valid as `preset` requires.
        unsafe { preset(raw, codec, guid, config) }
    }
}
unsafe extern "C" fn initialize(
    raw: *mut c_void,
    parameters: *mut NV_ENC_INITIALIZE_PARAMS,
) -> NVENCSTATUS {
    // SAFETY: The session passes a pointer to its live parameter struct for this call.
    let p = unsafe { *parameters };
    // SAFETY: `raw` is a live handle that `open` boxed and `destroy` has not freed.
    let api = unsafe { handle(raw) };
    assert_eq!(p.version, api.initialize());
    assert!(!p.encodeConfig.is_null());
    // SAFETY: `encodeConfig` is non-null (asserted above) and points at the session's boxed config.
    let config = unsafe { *p.encodeConfig };
    assert_eq!(config.version, api.config());
    state(|d| {
        d.initialized = Some((p, config));
        if d.reject_initialize == Some(api) {
            _NVENCSTATUS_NV_ENC_ERR_INVALID_VERSION
        } else {
            SUCCESS
        }
    })
}
unsafe extern "C" fn register(
    raw: *mut c_void,
    parameters: *mut NV_ENC_REGISTER_RESOURCE,
) -> NVENCSTATUS {
    // SAFETY: The session passes an exclusive pointer to its live parameter struct for this call.
    let p = unsafe { &mut *parameters };
    // SAFETY: `raw` is a live handle that `open` boxed and `destroy` has not freed.
    assert_eq!(p.version, unsafe { handle(raw) }.register());
    p.registeredResource = p.resourceToRegister;
    state(|d| {
        assert!(
            d.resources
                .insert(p.registeredResource as usize, p.bufferFormat)
                .is_none()
        );
        d.calls.push("register input");
    });
    SUCCESS
}
unsafe extern "C" fn unregister(_: *mut c_void, resource: NV_ENC_REGISTERED_PTR) -> NVENCSTATUS {
    state(|d| {
        assert!(!d.mapped.contains(&(resource as usize)));
        assert!(d.resources.remove(&(resource as usize)).is_some());
        d.calls.push("unregister input");
    });
    SUCCESS
}
unsafe extern "C" fn map(
    raw: *mut c_void,
    parameters: *mut NV_ENC_MAP_INPUT_RESOURCE,
) -> NVENCSTATUS {
    // SAFETY: The session passes an exclusive pointer to its live parameter struct for this call.
    let p = unsafe { &mut *parameters };
    // SAFETY: `raw` is a live handle that `open` boxed and `destroy` has not freed.
    assert_eq!(p.version, unsafe { handle(raw) }.structure(4, false));
    p.mappedResource = p.registeredResource;
    state(|d| {
        p.mappedBufferFmt = d.resources[&(p.registeredResource as usize)];
        assert!(d.mapped.insert(p.mappedResource as usize));
        d.calls.push("map input");
    });
    SUCCESS
}
unsafe extern "C" fn unmap(_: *mut c_void, resource: NV_ENC_INPUT_PTR) -> NVENCSTATUS {
    state(|d| {
        assert!(d.mapped.remove(&(resource as usize)));
        d.calls.push("unmap input");
    });
    SUCCESS
}
unsafe extern "C" fn output(
    raw: *mut c_void,
    parameters: *mut NV_ENC_CREATE_BITSTREAM_BUFFER,
) -> NVENCSTATUS {
    // SAFETY: The session passes an exclusive pointer to its live parameter struct for this call.
    let p = unsafe { &mut *parameters };
    // SAFETY: `raw` is a live handle that `open` boxed and `destroy` has not freed.
    assert_eq!(p.version, unsafe { handle(raw) }.structure(1, false));
    let mut buffer = Box::<Buffer>::default();
    p.bitstreamBuffer = (&mut *buffer as *mut Buffer).cast();
    state(|d| {
        d.outputs.insert(p.bitstreamBuffer as usize, buffer);
        d.calls.push("create output");
        if d.reject_output {
            _NVENCSTATUS_NV_ENC_ERR_OUT_OF_MEMORY
        } else {
            SUCCESS
        }
    })
}
unsafe extern "C" fn destroy_output(_: *mut c_void, output: NV_ENC_OUTPUT_PTR) -> NVENCSTATUS {
    state(|d| {
        let buffer = d.outputs.remove(&(output as usize)).unwrap();
        assert!(!buffer.locked);
        d.calls.push("destroy output");
    });
    SUCCESS
}
unsafe extern "C" fn register_event(
    raw: *mut c_void,
    parameters: *mut NV_ENC_EVENT_PARAMS,
) -> NVENCSTATUS {
    // SAFETY: The session passes a pointer to its live parameter struct for this call.
    let p = unsafe { &*parameters };
    // SAFETY: `raw` is a live handle that `open` boxed and `destroy` has not freed.
    assert_eq!(p.version, unsafe { handle(raw) }.event());
    state(|d| assert!(d.events.insert(p.completionEvent as usize)));
    SUCCESS
}
unsafe extern "C" fn unregister_event(
    raw: *mut c_void,
    parameters: *mut NV_ENC_EVENT_PARAMS,
) -> NVENCSTATUS {
    // SAFETY: The session passes a pointer to its live parameter struct for this call.
    let p = unsafe { &*parameters };
    // SAFETY: `raw` is a live handle that `open` boxed and `destroy` has not freed.
    assert_eq!(p.version, unsafe { handle(raw) }.event());
    state(|d| {
        assert!(d.events.remove(&(p.completionEvent as usize)));
        d.calls.push("unregister event");
    });
    SUCCESS
}
unsafe extern "C" fn encode(raw: *mut c_void, parameters: *mut NV_ENC_PIC_PARAMS) -> NVENCSTATUS {
    // SAFETY: The session passes a pointer to its live parameter struct for this call.
    let p = unsafe { &*parameters };
    // SAFETY: `raw` is a live handle that `open` boxed and `destroy` has not freed.
    assert_eq!(p.version, unsafe { handle(raw) }.picture());
    if p.encodePicFlags & _NV_ENC_PIC_FLAGS_NV_ENC_PIC_FLAG_EOS != 0 {
        return SUCCESS;
    }
    // SAFETY: `raw` is a live handle that `open` boxed and `destroy` has not freed.
    let api = unsafe { handle(raw) };
    state(|d| {
        let mut input = p.inputBuffer;
        let mut signal = None;
        if d.d3d12 {
            // SAFETY: A D3D12 session passes its slot's boxed resource structures, live while the
            // frame is pending.
            let (resource, output) = unsafe {
                (
                    &*(p.inputBuffer as *const NV_ENC_INPUT_RESOURCE_D3D12),
                    &*(p.outputBitstream as *const NV_ENC_OUTPUT_RESOURCE_D3D12),
                )
            };
            let version = api.structure(1, false);
            let (wait, done) = (resource.inputFencePoint, output.outputFencePoint);
            assert_eq!(
                [resource.version, output.version, wait.version, done.version],
                [version; 4]
            );
            assert_eq!(
                (wait.bWait(), wait.bSignal(), done.bWait(), done.bSignal()),
                (1, 0, 0, 1)
            );
            assert_eq!(done.pFence as usize, FAKE_FENCE);
            assert!(done.signalValue > d.fence_points.last().map_or(0, |point| point.2));
            assert!(d.mapped.contains(&(output.pOutputBuffer as usize)));
            assert_eq!(
                d.resources[&(output.pOutputBuffer as usize)],
                _NV_ENC_BUFFER_FORMAT_NV_ENC_BUFFER_FORMAT_U8
            );
            d.fence_points
                .push((wait.pFence as usize, wait.waitValue, done.signalValue));
            d.outputs.entry(p.outputBitstream as usize).or_default();
            input = resource.pInputBuffer;
            signal = Some(done.signalValue);
        }
        assert!(d.mapped.contains(&(input as usize)));
        if d.reject_submit {
            return _NVENCSTATUS_NV_ENC_ERR_GENERIC;
        }
        if let Some(value) = signal.filter(|_| !d.hold_fence) {
            d.fence = value;
        }
        let codec = state_codec(d);
        let buffer = d.outputs.get_mut(&(p.outputBitstream as usize)).unwrap();
        buffer.bytes = p.inputTimeStamp.to_le_bytes().to_vec();
        buffer.bytes.extend((8..d.picture).map(|i| i as u8));
        buffer.timestamp = p.inputTimeStamp;
        buffer.idr = p.encodePicFlags & _NV_ENC_PIC_FLAGS_NV_ENC_PIC_FLAG_FORCEIDR != 0;
        buffer.deltas = (!p.qpDeltaMap.is_null()).then(|| {
            // SAFETY: A non-null map holds `qpDeltaMapSize` bytes, live while the frame is pending.
            unsafe { std::slice::from_raw_parts(p.qpDeltaMap, p.qpDeltaMapSize as usize) }.to_vec()
        });
        d.maps.push(buffer.deltas.clone());
        let pointer = if codec == 1 {
            // SAFETY: An HEVC session fills this union member; it holds only integers and pointers.
            unsafe { p.codecPicParams.hevcPicParams.pMasteringDisplay }
        } else if codec == 2 {
            // SAFETY: An AV1 session fills this union member; it holds only integers and pointers.
            unsafe { p.codecPicParams.av1PicParams.pMasteringDisplay }
        } else {
            ptr::null_mut()
        };
        if !pointer.is_null() {
            // SAFETY: `pointer` is non-null and points at the slot's boxed metadata, live while the
            // frame is pending.
            buffer.metadata = Some((pointer, unsafe { (*pointer).maxLuma }));
        }
        let light = if codec == 1 {
            // SAFETY: An HEVC session fills this union member.
            unsafe { p.codecPicParams.hevcPicParams.pMaxCll }
        } else if codec == 2 {
            // SAFETY: An AV1 session fills this union member.
            unsafe { p.codecPicParams.av1PicParams.pMaxCll }
        } else {
            ptr::null_mut()
        };
        if !light.is_null() {
            // SAFETY: The pending slot owns this non-null metadata box.
            let values = unsafe { *light };
            buffer.light = Some((
                light,
                values.maxContentLightLevel,
                values.maxPicAverageLightLevel,
            ));
        }
        if !p.completionEvent.is_null() {
            assert!(d.events.contains(&(p.completionEvent as usize)));
            // SAFETY: `completionEvent` is the slot's live event, registered with this driver
            // (asserted above).
            unsafe {
                SetEvent(HANDLE(p.completionEvent)).unwrap();
            }
        }
        if d.need_more_input {
            _NVENCSTATUS_NV_ENC_ERR_NEED_MORE_INPUT
        } else {
            SUCCESS
        }
    })
}
unsafe extern "C" fn lock(raw: *mut c_void, parameters: *mut NV_ENC_LOCK_BITSTREAM) -> NVENCSTATUS {
    // SAFETY: The session passes an exclusive pointer to its live parameter struct for this call.
    let p = unsafe { &mut *parameters };
    // SAFETY: `raw` is a live handle that `open` boxed and `destroy` has not freed.
    assert_eq!(p.version, unsafe { handle(raw) }.lock());
    state(|d| {
        if d.lock_busy > 0 {
            d.lock_busy -= 1;
            return _NVENCSTATUS_NV_ENC_ERR_LOCK_BUSY;
        }
        let slices = configured_slices(d);
        let buffer = d.outputs.get_mut(&(p.outputBitstream as usize)).unwrap();
        if let Some((pointer, expected)) = buffer.metadata {
            assert_eq!(
                // SAFETY: The slot owning `pointer` is still pending, so its metadata box is live.
                unsafe { (*pointer).maxLuma },
                expected,
                "in-flight HDR metadata was overwritten"
            );
        }
        if let Some((pointer, cll, fall)) = buffer.light {
            // SAFETY: The slot owning the metadata is still pending.
            let values = unsafe { *pointer };
            assert_eq!(
                (values.maxContentLightLevel, values.maxPicAverageLightLevel),
                (cll, fall),
                "in-flight content light metadata was overwritten"
            );
        }
        assert!(!buffer.locked);
        buffer.locked = true;
        p.bitstreamBufferPtr = buffer.bytes.as_mut_ptr().cast();
        p.bitstreamSizeInBytes = buffer.bytes.len() as u32;
        if let Some((written, bytes)) = d.partial.pop_front() {
            assert_eq!(p.doNotWait(), 1, "a subframe lock does not wait");
            assert!(!p.sliceOffsets.is_null());
            p.numSlices = written;
            p.bitstreamSizeInBytes = bytes as u32;
            p.hwEncodeStatus = 1;
            for slice in 0..written as usize {
                let offset = if d.bad_offsets {
                    bytes
                } else {
                    slice * bytes / written as usize
                };
                // SAFETY: `sliceOffsets` holds an entry per macroblock, more than any test's
                // slices.
                unsafe { *p.sliceOffsets.add(slice) = offset as u32 };
            }
        } else {
            p.numSlices = slices;
            p.hwEncodeStatus = 2;
        }
        if !p.outputStatsPtr.is_null() {
            // Each block's QP is 30 moved by the picture's delta map.
            let size = std::mem::size_of::<NV_ENC_OUTPUT_STATS_BLOCK>();
            let blocks = p.outputStatsPtrSize as usize / size;
            for index in 0..blocks {
                // SAFETY: The session passes a buffer of `outputStatsPtrSize` bytes.
                let block = unsafe {
                    &mut *p
                        .outputStatsPtr
                        .cast::<NV_ENC_OUTPUT_STATS_BLOCK>()
                        .add(index)
                };
                // SAFETY: `raw` is a live handle that `open` boxed and `destroy` has not freed.
                assert_eq!(block.version, unsafe { handle(raw) }.structure(1, false));
                let delta = buffer.deltas.as_ref().map_or(0, |deltas| deltas[index]);
                block.QP = (30 + i32::from(delta)) as u8;
            }
        }
        p.outputTimeStamp = buffer.timestamp;
        p.pictureType = if buffer.idr {
            _NV_ENC_PIC_TYPE_NV_ENC_PIC_TYPE_IDR
        } else {
            _NV_ENC_PIC_TYPE_NV_ENC_PIC_TYPE_P
        };
        d.calls.push("lock output");
        SUCCESS
    })
}
unsafe extern "C" fn unlock(_: *mut c_void, output: NV_ENC_OUTPUT_PTR) -> NVENCSTATUS {
    state(|d| {
        let buffer = d.outputs.get_mut(&(output as usize)).unwrap();
        assert!(buffer.locked);
        buffer.locked = false;
        if d.d3d12 {
            // The READBACK buffer holds one picture at a time.
            d.outputs.remove(&(output as usize));
        }
        d.calls.push("unlock output");
    });
    SUCCESS
}
unsafe extern "C" fn reconfigure(
    raw: *mut c_void,
    parameters: *mut NV_ENC_RECONFIGURE_PARAMS,
) -> NVENCSTATUS {
    // SAFETY: The session passes a pointer to its live parameter struct for this call.
    let p = unsafe { *parameters };
    // SAFETY: `raw` is a live handle that `open` boxed and `destroy` has not freed.
    assert_eq!(p.version, unsafe { handle(raw) }.reconfigure());
    // SAFETY: `encodeConfig` is the boxed config `Session::bitrate` keeps alive for this call.
    let config = unsafe { *p.reInitEncodeParams.encodeConfig };
    state(|d| {
        d.reconfigured = Some((p, config));
        if d.reject_reconfigure {
            _NVENCSTATUS_NV_ENC_ERR_INVALID_PARAM
        } else {
            SUCCESS
        }
    })
}
unsafe extern "C" fn invalidate(_: *mut c_void, frame: u64) -> NVENCSTATUS {
    state(|d| {
        if d.reject_invalidate == Some(frame) {
            _NVENCSTATUS_NV_ENC_ERR_INVALID_PARAM
        } else {
            d.invalidated.push(frame);
            SUCCESS
        }
    })
}
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
    session.slot(Input {
        key,
        raw: key as *mut c_void,
        kind: _NV_ENC_INPUT_RESOURCE_TYPE_NV_ENC_INPUT_RESOURCE_TYPE_DIRECTX,
        pitch: 0,
        _texture: None,
        cuda: None,
        d3d12: Some(D3d12Input {
            _resource: None,
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

#[test]
fn rejected_versions_destroy_partial_sessions_and_keep_legacy_ten_bit_fields() -> Result<()> {
    reset();
    state(|d| {
        d.reject_open = Some(ApiVersion(13, 0));
        d.reject_initialize = Some(ApiVersion(12, 2));
        d.preset_fallback = true;
    });
    let config = Negotiated {
        codec: 1,
        hdr: true,
        rate_millihz: 119880,
        ..Default::default()
    };
    let encoder = session(&config, ApiVersion(13, 0))?;
    assert_eq!(encoder.api, ApiVersion(12, 1));
    // SAFETY: The session configured HEVC; the union member holds only integers and pointers.
    let format = unsafe { encoder.config.encodeCodecConfig.hevcConfig };
    assert_eq!(format.reserved3(), 2);
    assert_eq!(format.inputBitDepth, 0);
    assert_eq!(format.outputBitDepth, 0);
    assert_eq!(format.hevcVUIParameters.colourPrimaries, 9);
    assert_eq!(format.hevcVUIParameters.transferCharacteristics, 16);
    assert_eq!(encoder.initialize.frameRateNum, 119880);
    assert_eq!(encoder.initialize.frameRateDen, 1000);
    assert_eq!(encoder.config.frameIntervalP, 1);
    assert_eq!(encoder.config.rcParams.enableLookahead(), 0);
    state(|d| {
        assert_eq!(
            d.opened,
            vec![ApiVersion(13, 0), ApiVersion(12, 2), ApiVersion(12, 1)]
        );
        assert_eq!(d.destroyed, vec![ApiVersion(13, 0), ApiVersion(12, 2)]);
    });
    drop(encoder);
    assert_clean();
    reset();
    let av1 = session(
        &Negotiated {
            codec: 2,
            hdr: true,
            ..Default::default()
        },
        ApiVersion(12, 1),
    )?;
    // SAFETY: The session configured AV1; the union member holds only integers and pointers.
    let format = unsafe { av1.config.encodeCodecConfig.av1Config };
    assert_eq!(format.enableTemporalSVC(), 1);
    assert_eq!(format.reserved4(), 1);
    assert_eq!(format.inputBitDepth, 0);
    assert_eq!(format.outputBitDepth, 0);
    drop(av1);
    assert_clean();
    Ok(())
}
#[test]
fn completed_output_retains_inputs_and_async_event_survives_a_busy_lock() -> Result<()> {
    reset();
    state(|d| {
        d.asynchronous = true;
        d.need_more_input = true;
        d.lock_busy = 1;
    });
    let mut encoder = session(&Negotiated::default(), ApiVersion(11, 0))?;
    let slot = input(&mut encoder, 17)?;
    let presentation = Instant::now() - Duration::from_millis(10);
    encoder.submit(slot, None, true, presentation)?;
    assert!(encoder.poll()?.is_empty());
    assert_eq!(encoder.pending.len(), 1);
    state(|d| assert!(d.mapped.contains(&17)));
    assert!(encoder.submit(slot, None, false, Instant::now()).is_err());
    let result = encoder.poll()?;
    assert_eq!(result.len(), 1);
    assert!(result[0].idr);
    assert!(!result[0].after_invalidation);
    assert_eq!(result[0].bytes, 1u64.to_le_bytes());
    assert!(result[0].latency.is_some());
    assert_eq!(result[0].presentation, Some(presentation));
    state(|d| assert!(d.mapped.is_empty()));
    encoder.submit(slot, None, false, Instant::now())?;
    assert_eq!(encoder.poll()?[0].bytes, 2u64.to_le_bytes());
    drop(encoder);
    assert_clean();
    state(|d| {
        let position = |name| d.calls.iter().rposition(|v| *v == name).unwrap();
        assert!(position("unlock output") < position("unmap input"));
        assert!(position("unmap input") < position("unregister event"));
        assert!(position("destroy output") < position("unregister input"));
        assert!(position("unregister input") < position("destroy session"));
    });
    Ok(())
}
#[test]
fn timed_out_submission_retains_resources_until_native_session_destruction() -> Result<()> {
    reset();
    state(|d| {
        d.asynchronous = true;
        d.lock_busy = usize::MAX;
        d.destroy_busy = true;
    });
    let mut encoder = session(&Negotiated::default(), ApiVersion(13, 0))?;
    let slot = input(&mut encoder, 17)?;
    encoder.submit(slot, None, true, Instant::now())?;
    encoder.pending.front_mut().unwrap().started =
        Instant::now() - COMPLETION_TIMEOUT - Duration::from_millis(1);
    assert!(encoder.poll().is_err());
    drop(encoder);
    assert_clean();
    Ok(())
}
#[test]
fn partial_output_allocation_is_released_and_reference_caps_control_advertisement() -> Result<()> {
    reset();
    state(|d| d.reject_output = true);
    let mut encoder = session(&Negotiated::default(), ApiVersion(13, 0))?;
    assert!(input(&mut encoder, 17).is_err());
    drop(encoder);
    assert_clean();
    reset();
    state(|d| d.no_multiple_refs = true);
    let encoder = session(&Negotiated::default(), ApiVersion(13, 0))?;
    assert_eq!(encoder.retained, 1);
    assert!(!encoder.invalidation);
    drop(encoder);
    assert_clean();
    Ok(())
}
#[test]
fn loss_feedback_drains_pending_frames_and_only_confirms_successful_invalidation() -> Result<()> {
    reset();
    let mut encoder = session(&Negotiated::default(), ApiVersion(13, 0))?;
    let slot = input(&mut encoder, 17)?;
    for frame in 1..=12 {
        encoder.submit(slot, None, frame == 1, Instant::now())?;
        encoder.poll()?;
    }
    // Feedback can arrive while a later picture is still pending. Drain into
    // the output queue; the feedback range includes all dependent pictures.
    encoder.submit(slot, None, false, Instant::now())?;
    assert!(encoder.invalidate(11, 11)?);
    state(|d| assert_eq!(d.invalidated, vec![11, 12, 13]));
    assert_eq!(encoder.poll()?[0].bytes, 13u64.to_le_bytes());
    encoder.submit(slot, None, false, Instant::now())?;
    assert!(encoder.poll()?[0].after_invalidation);
    encoder.submit(slot, None, false, Instant::now())?;
    assert!(!encoder.poll()?[0].after_invalidation);
    assert!(encoder.invalidate(11, 12)?);
    state(|d| assert_eq!(d.invalidated.len(), 3));
    state(|d| d.reject_invalidate = Some(15));
    assert!(encoder.invalidate(14, 14).is_err());
    encoder.submit(slot, None, false, Instant::now())?;
    assert!(!encoder.poll()?[0].after_invalidation);
    encoder.submit(slot, None, true, Instant::now())?;
    assert!(encoder.poll()?[0].idr);
    assert!(encoder.invalidate(14, 15)?); // Old GOP feedback is already resolved.
    assert!(!encoder.invalidate(17, 17)?); // A lost current IDR needs another IDR.
    drop(encoder);
    assert_clean();
    Ok(())
}
#[test]
fn failed_bitrate_reconfigure_keeps_previous_config_then_higher_rate_requests_idr() -> Result<()> {
    reset();
    let stream = Negotiated {
        codec: 1,
        hdr: true,
        ..Default::default()
    };
    let mut encoder = session(&stream, ApiVersion(13, 0))?;
    let before = encoder.config.rcParams.averageBitRate;
    state(|d| d.reject_reconfigure = true);
    assert!(encoder.bitrate(40000).is_err());
    assert_eq!(encoder.config.rcParams.averageBitRate, before);
    assert_eq!(encoder.stream.bitrate_kbps, 20000);
    state(|d| d.reject_reconfigure = false);
    encoder.bitrate(40000)?;
    assert_eq!(encoder.config.rcParams.averageBitRate, 40_000_000);
    assert_eq!(
        encoder.initialize.encodeConfig,
        encoder.config.as_mut() as *mut _
    );
    state(|d| {
        let (parameters, config) = d.reconfigured.as_ref().unwrap();
        assert_eq!(parameters.forceIDR(), 1);
        assert_eq!(parameters.resetEncoder(), 1);
        assert_eq!(config.rcParams.maxBitRate, 40_000_000);
        assert_eq!(
            // SAFETY: HEVC was configured; the union member is plain integers and pointers.
            unsafe { config.encodeCodecConfig.hevcConfig }
                .hevcVUIParameters
                .transferCharacteristics,
            16
        );
    });
    assert!(encoder.force_idr);
    let slot = input(&mut encoder, 17)?;
    encoder.submit(slot, None, false, Instant::now())?;
    assert!(encoder.poll()?[0].idr);
    encoder.bitrate(10000)?;
    assert!(!encoder.force_idr);
    assert!(encoder.bitrate(0).is_err());
    assert!(encoder.bitrate(800001).is_err());
    drop(encoder);
    assert_clean();
    Ok(())
}
#[test]
fn hdr_metadata_uses_each_codecs_units_and_bounds_av1_chromaticities() {
    let source = butterpollo_core::hdr::Metadata {
        minimum: 20,
        max_cll: 900,
        max_fall: 400,
        ..Default::default()
    };
    let mut hevc = Metadata::new(1);
    hevc.update(source, 1);
    assert_eq!(
        hevc.encoded(),
        EncodedMetadata {
            primaries: [[35400, 14600], [8500, 39850], [6550, 2300]],
            white: [15635, 16450],
            maximum: 10_000_000,
            minimum: 20,
            max_cll: 900,
            max_fall: 400,
        }
    );
    let mut av1 = Metadata::new(2);
    av1.update(source, 2);
    assert_eq!(
        av1.encoded(),
        EncodedMetadata {
            primaries: [[46399, 19137], [11141, 52232], [8585, 3015]],
            white: [20493, 21561],
            maximum: 256_000,
            minimum: 33,
            max_cll: 900,
            max_fall: 400,
        }
    );
    av1.update(
        butterpollo_core::hdr::Metadata {
            primaries: [[u16::MAX; 2]; 3],
            white: [0, 50000],
            maximum_nits: u16::MAX,
            minimum: u16::MAX,
            ..source
        },
        2,
    );
    assert_eq!(av1.encoded().primaries, [[u16::MAX; 2]; 3]);
    assert_eq!(av1.encoded().white, [0, u16::MAX]);
    assert_eq!(av1.encoded().maximum, 16_776_960);
    assert_eq!(av1.encoded().minimum, 107_373);
}

#[test]
fn hdr_output_flags_and_submissions_respect_codec_api_and_sdr() -> Result<()> {
    for codec in [1, 2] {
        for api in [ApiVersion(13, 0), ApiVersion(12, 2), ApiVersion(12, 1)] {
            for hdr in [false, true] {
                for preset_hdr_metadata in [false, true] {
                    reset();
                    state(|d| d.preset_hdr_metadata = preset_hdr_metadata);
                    let mut encoder = session(
                        &Negotiated {
                            codec,
                            hdr,
                            ..Default::default()
                        },
                        api,
                    )?;
                    let enabled = hdr && api >= ApiVersion(13, 0);
                    // SAFETY: The configured codec selects its plain-data union member.
                    let (mastering, light) = unsafe {
                        if codec == 1 {
                            let config = encoder.config.encodeCodecConfig.hevcConfig;
                            (config.outputMasteringDisplay(), config.outputMaxCll())
                        } else {
                            let config = encoder.config.encodeCodecConfig.av1Config;
                            (config.outputMasteringDisplay(), config.outputMaxCll())
                        }
                    };
                    assert_eq!((mastering, light), (u32::from(enabled), u32::from(enabled)));
                    encoder.set_hdr_metadata(butterpollo_core::hdr::Metadata {
                        maximum_nits: 1400,
                        max_cll: 1300,
                        max_fall: 800,
                        ..Default::default()
                    });
                    let slot = input(&mut encoder, 17)?;
                    encoder.submit(slot, None, false, Instant::now())?;
                    state(|d| {
                        let buffer = d
                            .outputs
                            .get(&(encoder.slots[slot].output as usize))
                            .unwrap();
                        assert_eq!(buffer.metadata.is_some(), enabled);
                        assert_eq!(buffer.light.is_some(), enabled);
                    });
                    assert_eq!(encoder.poll()?[0].idr, enabled);
                    drop(encoder);
                    assert_clean();
                }
            }
        }
    }
    Ok(())
}

#[test]
fn hdr_metadata_is_snapshotted_until_the_gpu_submission_completes() -> Result<()> {
    for codec in [1, 2] {
        reset();
        state(|d| d.asynchronous = true);
        let mut encoder = session(
            &Negotiated {
                codec,
                hdr: true,
                ..Default::default()
            },
            ApiVersion(13, 0),
        )?;
        let first = butterpollo_core::hdr::Metadata {
            max_cll: 550,
            max_fall: 300,
            ..butterpollo_core::hdr::Metadata::display(600., 0.002, 300.)
        };
        let second = butterpollo_core::hdr::Metadata {
            max_cll: 1300,
            max_fall: 800,
            ..butterpollo_core::hdr::Metadata::display(1400., 0.001, 800.)
        };
        let slot = input(&mut encoder, 17)?;
        encoder.set_hdr_metadata(first);
        encoder.submit(slot, None, false, Instant::now())?;
        encoder.set_hdr_metadata(second);
        let scale = if codec == 2 { 256 } else { 10000 };
        assert_eq!(encoder.slots[slot].metadata.mastering.maxLuma, 600 * scale);
        assert_eq!(encoder.slots[slot].metadata.light.maxContentLightLevel, 550);
        assert_eq!(
            encoder.slots[slot].metadata.light.maxPicAverageLightLevel,
            300
        );
        assert!(encoder.poll()?[0].idr);
        encoder.submit(slot, None, false, Instant::now())?;
        assert_eq!(encoder.slots[slot].metadata.mastering.maxLuma, 1400 * scale);
        assert_eq!(
            encoder.slots[slot].metadata.light.maxContentLightLevel,
            1300
        );
        assert!(encoder.poll()?[0].idr);
        // Identical refreshes, control-only values and changes superseded before
        // submission must not produce another keyframe.
        encoder.set_hdr_metadata(second);
        encoder.set_hdr_metadata(first);
        encoder.set_hdr_metadata(butterpollo_core::hdr::Metadata {
            full_frame_nits: 900,
            ..second
        });
        encoder.submit(slot, None, false, Instant::now())?;
        assert!(!encoder.poll()?[0].idr);
        // Light-level-only updates also need a keyframe.
        encoder.set_hdr_metadata(butterpollo_core::hdr::Metadata {
            max_fall: 700,
            ..second
        });
        encoder.submit(slot, None, false, Instant::now())?;
        assert!(encoder.poll()?[0].idr);
        encoder.submit(slot, None, false, Instant::now())?;
        assert!(!encoder.poll()?[0].idr);
        drop(encoder);
        assert_clean();
    }
    Ok(())
}

#[test]
fn hdr_keyframe_retries_failed_submissions_and_consumes_simultaneous_requests() -> Result<()> {
    for codec in [1, 2] {
        reset();
        let mut encoder = session(
            &Negotiated {
                codec,
                hdr: true,
                ..Default::default()
            },
            ApiVersion(13, 0),
        )?;
        let slot = input(&mut encoder, 17)?;
        encoder.submit(slot, None, true, Instant::now())?;
        assert!(encoder.poll()?[0].idr);
        let submitted = encoder.submitted_metadata;
        encoder.set_hdr_metadata(butterpollo_core::hdr::Metadata {
            maximum_nits: 1400,
            max_cll: 1300,
            max_fall: 800,
            ..Default::default()
        });
        state(|d| d.reject_submit = true);
        assert!(encoder.submit(slot, None, false, Instant::now()).is_err());
        assert_eq!(encoder.submitted_metadata, submitted);
        assert!(encoder.force_idr);
        // Release the rejected registration before retrying; no frame was accepted.
        encoder.clear_inputs()?;
        state(|d| {
            d.reject_submit = false;
            d.need_more_input = true;
        });
        let slot = input(&mut encoder, 18)?;
        encoder.submit(slot, None, true, Instant::now())?;
        assert_eq!(encoder.submitted_metadata, Some(encoder.metadata.encoded()));
        assert!(!encoder.force_idr);
        assert!(encoder.poll()?[0].idr);
        encoder.submit(slot, None, false, Instant::now())?;
        assert!(!encoder.poll()?[0].idr);
        // An external IDR also consumes an internal bitrate/recovery request,
        // even when there is no metadata change to trigger it.
        encoder.force_idr = true;
        encoder.submit(slot, None, true, Instant::now())?;
        assert!(encoder.poll()?[0].idr);
        assert!(!encoder.force_idr);
        encoder.submit(slot, None, false, Instant::now())?;
        assert!(!encoder.poll()?[0].idr);
        drop(encoder);
        assert_clean();
    }
    Ok(())
}

fn check_decoded_hdr_metadata(
    frame: &serde_json::Value,
    expected: butterpollo_core::hdr::Metadata,
    codec: u8,
) -> Result<()> {
    let side_data = frame["side_data_list"]
        .as_array()
        .context("decoded frame has no HDR metadata")?;
    let mastering = side_data
        .iter()
        .find(|data| data["side_data_type"] == "Mastering display metadata")
        .context("decoded frame has no mastering metadata")?;
    let light = side_data
        .iter()
        .find(|data| data["side_data_type"] == "Content light level metadata")
        .context("decoded frame has no content light metadata")?;
    let check = |key: &str, expected: f64, unit: f64| -> Result<()> {
        let (num, den) = mastering[key]
            .as_str()
            .and_then(|s| s.split_once('/'))
            .with_context(|| format!("missing decoded {key}"))?;
        let actual = num.parse::<f64>()? / den.parse::<f64>()?;
        if !actual.is_finite() || (actual - expected).abs() > unit / 2. + 1e-9 {
            bail!("decoded {key}: expected {expected}, got {actual}");
        }
        Ok(())
    };
    let chroma_unit = if codec == 2 { 1. / 65536. } else { 1. / 50000. };
    for (keys, point) in [
        (["red_x", "red_y"], expected.primaries[0]),
        (["green_x", "green_y"], expected.primaries[1]),
        (["blue_x", "blue_y"], expected.primaries[2]),
        (["white_point_x", "white_point_y"], expected.white),
    ] {
        for (key, value) in keys.into_iter().zip(point) {
            check(key, f64::from(value) / 50000., chroma_unit)?;
        }
    }
    check(
        "max_luminance",
        f64::from(expected.maximum_nits),
        if codec == 2 { 1. / 256. } else { 1. / 10000. },
    )?;
    check(
        "min_luminance",
        f64::from(expected.minimum) / 10000.,
        if codec == 2 { 1. / 16384. } else { 1. / 10000. },
    )?;
    if light["max_content"].as_u64() != Some(u64::from(expected.max_cll))
        || light["max_average"].as_u64() != Some(u64::from(expected.max_fall))
    {
        bail!("decoded content light levels do not match the submitted metadata");
    }
    Ok(())
}

#[test]
fn decoded_hdr_check_rejects_wrong_av1_units_and_missing_light_metadata() -> Result<()> {
    let expected = butterpollo_core::hdr::Metadata {
        minimum: 20,
        max_cll: 900,
        max_fall: 400,
        ..Default::default()
    };
    let frame = serde_json::json!({"side_data_list": [
        {"side_data_type": "Mastering display metadata",
         "red_x": "46399/65536", "red_y": "19137/65536",
         "green_x": "11141/65536", "green_y": "52232/65536",
         "blue_x": "8585/65536", "blue_y": "3015/65536",
         "white_point_x": "20493/65536", "white_point_y": "21561/65536",
         "max_luminance": "256000/256", "min_luminance": "33/16384"},
        {"side_data_type": "Content light level metadata", "max_content": 900, "max_average": 400}
    ]});
    check_decoded_hdr_metadata(&frame, expected, 2)?;
    let mut wrong_units = frame.clone();
    wrong_units["side_data_list"][0]["max_luminance"] = "10000000/256".into();
    assert!(check_decoded_hdr_metadata(&wrong_units, expected, 2).is_err());
    let mut missing_light = frame;
    missing_light["side_data_list"]
        .as_array_mut()
        .unwrap()
        .pop();
    assert!(check_decoded_hdr_metadata(&missing_light, expected, 2).is_err());
    Ok(())
}

#[test]
#[ignore = "requires explicitly selected NVIDIA hardware and independent FFmpeg decoding"]
fn native_nvenc_loss_recovery_and_444_hdr_decode() -> Result<()> {
    use std::os::windows::process::CommandExt;
    if std::env::var("BUTTERPOLLO_TEST_NVENC").as_deref() != Ok("1") {
        bail!("set BUTTERPOLLO_TEST_NVENC=1 on an NVIDIA test machine");
    }
    let decoder =
        std::env::var_os("BUTTERPOLLO_TEST_FFMPEG").context("set BUTTERPOLLO_TEST_FFMPEG")?;
    let report = std::path::PathBuf::from(
        std::env::var_os("BUTTERPOLLO_TEST_NVENC_REPORT")
            .context("set BUTTERPOLLO_TEST_NVENC_REPORT")?,
    );
    let directory = report
        .parent()
        .context("report requires a parent directory")?;
    std::fs::create_dir_all(directory)?;
    let _com = crate::capture::ComGuard::new()?;
    let gpu = Device::new(&std::env::var("BUTTERPOLLO_TEST_NVENC_DISPLAY").unwrap_or_default())?;
    if !gpu.display.adapter.to_ascii_lowercase().contains("nvidia") {
        bail!("select an NVIDIA-attached output with BUTTERPOLLO_TEST_NVENC_DISPLAY");
    }
    let cases = std::env::var("BUTTERPOLLO_TEST_NVENC_CASES")
        .unwrap_or_else(|_| "h264,hevc-hdr,hevc444-sdr,hevc444-hdr".into());
    let mut reports = Vec::new();
    for name in cases.split(',').map(str::trim) {
        let (codec, hdr, yuv444) = match name {
            "h264" => (0, false, false),
            "hevc-hdr" => (1, true, false),
            "hevc444-sdr" => (1, false, true),
            "hevc444-hdr" => (1, true, true),
            "av1-hdr" => (2, true, false),
            "av1444-hdr" => (2, true, true),
            _ => bail!("unknown NVIDIA test case {name}"),
        };
        let config = Negotiated {
            width: 640,
            height: 480,
            codec,
            hdr,
            yuv444,
            bitrate_kbps: 20000,
            ..Default::default()
        };
        let mut encoder = Encoder::new_device_options(&config, gpu.clone(), &Config::default())?;
        let metadata_embedded = hdr && encoder.session.api >= ApiVersion(13, 0);
        let first_metadata = butterpollo_core::hdr::Metadata {
            minimum: 20,
            max_cll: 900,
            max_fall: 400,
            ..Default::default()
        };
        let second_metadata = butterpollo_core::hdr::Metadata {
            maximum_nits: 1400,
            minimum: 10,
            max_cll: 1300,
            max_fall: 800,
            ..first_metadata
        };
        encoder.set_hdr_metadata(first_metadata);
        let supported = encoder.supports_invalidation();
        let mut packets = Vec::new();
        for frame in 1..=64 {
            if frame == 40 {
                encoder.set_hdr_metadata(second_metadata);
            }
            if supported && matches!(frame, 7 | 19) {
                assert!(encoder.invalidate_ref_frames(frame - 2, frame - 1));
            }
            let stride = if hdr { 640 * 8 } else { 640 * 4 };
            let mut image = Image {
                width: 640,
                height: 480,
                stride,
                bytes: vec![0; stride * 480],
                pixel: if hdr { Pixel::RgbaF16 } else { Pixel::Bgra8 },
                captured: Instant::now(),
            };
            for (index, pixel) in image
                .bytes
                .chunks_exact_mut(if hdr { 8 } else { 4 })
                .enumerate()
            {
                if hdr {
                    for channel in 0..3 {
                        let value =
                            ((index / 640 + frame as usize * 3 + channel * 11) % 127) as f32 / 10.;
                        pixel[channel * 2..channel * 2 + 2]
                            .copy_from_slice(&half::f16::from_f32(value).to_bits().to_le_bytes());
                    }
                    pixel[6..].copy_from_slice(&half::f16::ONE.to_bits().to_le_bytes());
                } else {
                    pixel.copy_from_slice(&[
                        (index / 640) as u8,
                        (index + frame as usize * 3) as u8,
                        frame as u8,
                        255,
                    ]);
                }
            }
            packets.extend(encoder.encode(
                &image,
                frame == 1,
                if frame >= 33 { 25000 } else { 20000 },
            )?);
            let deadline = Instant::now() + Duration::from_secs(2);
            while encoder.pending() {
                packets.extend(encoder.poll()?);
                if Instant::now() >= deadline {
                    bail!("NVIDIA output fixture timed out");
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            assert_eq!(packets.len(), frame as usize);
        }
        if supported {
            assert!(packets[6].after_invalidation);
            assert!(packets[18].after_invalidation);
        }
        let format = match codec {
            0 => "h264",
            1 => "hevc",
            _ => "obu",
        };
        let path = directory.join(format!("nvenc-{}-{name}.{format}", std::process::id()));
        let bytes: Vec<_> = packets
            .iter()
            .enumerate()
            .filter(|(index, _)| !supported || !matches!(*index, 4 | 5 | 16 | 17))
            .flat_map(|(_, packet)| packet.bytes.iter().copied())
            .collect();
        std::fs::write(&path, bytes)?;
        let ffprobe = std::env::var_os("BUTTERPOLLO_TEST_FFPROBE")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from(&decoder).with_file_name("ffprobe.exe"));
        let probe = std::process::Command::new(ffprobe)
            .args([
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_frames",
                "-show_entries",
                "stream=codec_name,width,height,pix_fmt,color_transfer,color_primaries,color_space:frame=key_frame:frame_side_data",
                "-of",
                "json",
            ])
            .arg(&path)
            .creation_flags(0x08000000)
            .output()?;
        assert!(probe.status.success() && probe.stderr.is_empty());
        let info: serde_json::Value = serde_json::from_slice(&probe.stdout)?;
        let stream = &info["streams"][0];
        assert_eq!(stream["width"], 640);
        assert_eq!(stream["height"], 480);
        assert_eq!(
            stream["codec_name"],
            match codec {
                0 => "h264",
                1 => "hevc",
                _ => "av1",
            }
        );
        assert_eq!(
            stream["pix_fmt"],
            match (yuv444, hdr) {
                (false, false) => "yuv420p",
                (false, true) => "yuv420p10le",
                (true, false) => "yuv444p",
                (true, true) => "yuv444p10le",
            }
        );
        if hdr {
            assert_eq!(stream["color_transfer"], "smpte2084");
            assert_eq!(stream["color_primaries"], "bt2020");
            assert_eq!(stream["color_space"], "bt2020nc");
        }
        if metadata_embedded {
            assert!(
                packets[39].idr,
                "metadata change did not request a keyframe"
            );
            let frames = info["frames"]
                .as_array()
                .context("ffprobe returned no decoded frames")?;
            check_decoded_hdr_metadata(
                frames.first().context("no first frame")?,
                first_metadata,
                codec,
            )?;
            check_decoded_hdr_metadata(
                frames.last().context("no last frame")?,
                second_metadata,
                codec,
            )?;
        }
        let output = std::process::Command::new(&decoder)
            .args([
                "-hide_banner",
                "-nostdin",
                "-v",
                "error",
                "-xerror",
                "-err_detect",
                "explode",
                "-f",
                format,
                "-i",
            ])
            .arg(&path)
            .args([
                "-an",
                "-progress",
                "pipe:1",
                "-nostats",
                "-fps_mode",
                "passthrough",
                "-f",
                "null",
                "-",
            ])
            .creation_flags(0x08000000)
            .output()?;
        assert!(
            output.status.success() && output.stderr.is_empty(),
            "{name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let decoded = String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| line.strip_prefix("frame=")?.trim().parse::<u32>().ok())
            .next_back()
            .context("decoder reported no frames")?;
        assert_eq!(decoded, if supported { 60 } else { 64 });
        reports.push(serde_json::json!({"case":name,"codec":codec,"hdr":hdr,"yuv444":yuv444,"decoded":decoded,
            "native_rfi":supported,"recovery_frames":packets.iter().filter(|p|p.after_invalidation).count(),"bitrate_reconfigured":true,
            "hdr_metadata_before_and_after_change_verified":metadata_embedded}));
    }
    std::fs::write(report, serde_json::to_vec_pretty(&reports)?)?;
    Ok(())
}
#[test]
fn vbr_sets_its_peak_and_quality_keeps_them_on_bitrate_changes_and_falls_back_loudly() -> Result<()>
{
    reset();
    let stream = Negotiated {
        codec: 2,
        ..Default::default()
    };
    let mut encoder = session_with(
        &stream,
        ApiVersion(13, 0),
        "nvenc_rate_control = vbr\nnvenc_target_quality = 40\n",
    )?;
    let (_, config) = initialized();
    assert_eq!(
        config.rcParams.rateControlMode,
        _NV_ENC_PARAMS_RC_MODE_NV_ENC_PARAMS_RC_VBR
    );
    assert_eq!(config.rcParams.averageBitRate, 20_000_000);
    assert_eq!(config.rcParams.maxBitRate, 30_000_000);
    assert_eq!(config.rcParams.targetQuality, 40);
    encoder.bitrate(40000)?;
    state(|d| {
        let (_, config) = d.reconfigured.as_ref().unwrap();
        assert_eq!(config.rcParams.averageBitRate, 40_000_000);
        assert_eq!(config.rcParams.maxBitRate, 60_000_000);
    });
    drop(encoder);
    // A GPU without VBR encodes CBR, with a warning in the log.
    reset();
    state(|d| d.rate_modes = Some(_NV_ENC_PARAMS_RC_MODE_NV_ENC_PARAMS_RC_CBR as i32));
    let encoder = session_with(&stream, ApiVersion(13, 0), "nvenc_rate_control = vbr\n")?;
    assert_eq!(encoder.rate_control, RateControl::Cbr);
    let (_, config) = initialized();
    assert_eq!(
        config.rcParams.rateControlMode,
        _NV_ENC_PARAMS_RC_MODE_NV_ENC_PARAMS_RC_CBR
    );
    assert_eq!(config.rcParams.maxBitRate, config.rcParams.averageBitRate);
    drop(encoder);
    assert_clean();
    Ok(())
}
#[test]
fn spatial_aq_is_on_by_default_and_the_default_preset_stays_p1() -> Result<()> {
    reset();
    let encoder = session(&Negotiated::default(), ApiVersion(13, 0))?;
    let (parameters, config) = initialized();
    assert_eq!(config.rcParams.enableAQ(), 1);
    assert!(same_preset(parameters.presetGUID, 1));
    assert_eq!(encoder.budget.unwrap().effective, 1);
    drop(encoder);
    reset();
    let encoder = session_with(
        &Negotiated::default(),
        ApiVersion(13, 0),
        "nvenc_spatial_aq = false\n",
    )?;
    assert_eq!(initialized().1.rcParams.enableAQ(), 0);
    drop(encoder);
    assert_clean();
    Ok(())
}
#[test]
fn the_frame_budget_guard_lowers_a_slow_preset_unless_turned_off() -> Result<()> {
    let uhd = Negotiated {
        codec: 1,
        width: 3840,
        height: 2160,
        fps: 120,
        ..Default::default()
    };
    reset();
    let encoder = session_with(&uhd, ApiVersion(13, 0), "nvenc_preset = 4\n")?;
    assert!(same_preset(initialized().0.presetGUID, 1));
    assert_eq!(encoder.budget.unwrap().configured, 4);
    drop(encoder);
    // Two engines that split encoding may use leave P4 in budget, unless
    // split encoding is off.
    reset();
    state(|d| d.engines = 2);
    let encoder = session_with(&uhd, ApiVersion(13, 0), "nvenc_preset = 4\n")?;
    assert!(same_preset(initialized().0.presetGUID, 4));
    drop(encoder);
    reset();
    state(|d| d.engines = 2);
    let encoder = session_with(
        &uhd,
        ApiVersion(13, 0),
        "nvenc_preset = 4\nnvenc_split_encode = disabled\n",
    )?;
    assert!(same_preset(initialized().0.presetGUID, 1));
    drop(encoder);
    reset();
    let encoder = session_with(
        &uhd,
        ApiVersion(13, 0),
        "nvenc_preset = 4\nnvenc_frame_budget_guard = false\n",
    )?;
    assert!(same_preset(initialized().0.presetGUID, 4));
    assert!(encoder.budget.unwrap().estimated_ms > encoder.budget.unwrap().budget_ms);
    drop(encoder);
    assert_clean();
    Ok(())
}
#[test]
fn split_strips_and_the_slice_mode_follow_the_settings() -> Result<()> {
    let hevc = Negotiated {
        codec: 1,
        ..Default::default()
    };
    let slice_mode = |config: &NV_ENC_CONFIG| {
        // SAFETY: HEVC was configured; the union member is plain integers and pointers.
        let hevc = unsafe { config.encodeCodecConfig.hevcConfig };
        (hevc.sliceMode, hevc.sliceModeData)
    };
    reset();
    let encoder = session_with(
        &hevc,
        ApiVersion(13, 0),
        "nvenc_split_encode = two_strips\n",
    )?;
    let (parameters, config) = initialized();
    assert_eq!(parameters.splitEncodeMode(), 2);
    // One slice and split encoding allowed: no fixed slice mode.
    assert_eq!(slice_mode(&config), (0, 0));
    drop(encoder);
    reset();
    let encoder = session_with(&hevc, ApiVersion(13, 0), "nvenc_split_encode = disabled\n")?;
    let (parameters, config) = initialized();
    assert_eq!(parameters.splitEncodeMode(), SPLIT_DISABLED);
    assert_eq!(slice_mode(&config), (3, 1));
    drop(encoder);
    reset();
    let sliced = Negotiated {
        slices: 4,
        ..hevc.clone()
    };
    let encoder = session(&sliced, ApiVersion(13, 0))?;
    assert_eq!(slice_mode(&initialized().1), (3, 4));
    drop(encoder);
    reset();
    let encoder = session(&hevc, ApiVersion(12, 1))?;
    assert_eq!(slice_mode(&initialized().1), (3, 1));
    drop(encoder);
    assert_clean();
    Ok(())
}
#[test]
fn a_codec_the_gpu_lacks_is_refused_by_name_before_the_session_configures() -> Result<()> {
    reset();
    state(|d| d.codecs = Some(vec![0, 1]));
    let av1 = Negotiated {
        codec: 2,
        ..Default::default()
    };
    let error = format!("{:#}", session(&av1, ApiVersion(13, 0)).err().unwrap());
    assert!(error.contains("cannot encode AV1"), "{error}");
    assert!(error.contains("Select another codec"), "{error}");
    state(|d| assert!(d.initialized.is_none()));
    assert_clean();
    let hevc = Negotiated {
        codec: 1,
        ..Default::default()
    };
    drop(session(&hevc, ApiVersion(13, 0))?);
    assert_clean();
    Ok(())
}
#[test]
fn hlg_streams_signal_the_hlg_transfer_and_pq_streams_pq() -> Result<()> {
    for (hlg, transfer) in [(false, 16), (true, 18)] {
        reset();
        let mut stream = Negotiated {
            codec: 1,
            hdr: true,
            ..Default::default()
        };
        if hlg {
            // rubylight_protocol::hdr::HLG
            stream.hdr_format = 2;
        }
        assert_eq!(stream.hlg(), hlg);
        let encoder = session(&stream, ApiVersion(13, 0))?;
        // SAFETY: HEVC was configured; the union member is plain integers and pointers.
        let vui = unsafe { initialized().1.encodeCodecConfig.hevcConfig }.hevcVUIParameters;
        assert_eq!(vui.transferCharacteristics, transfer);
        drop(encoder);
    }
    assert_clean();
    Ok(())
}
#[test]
fn packed_444_codes_a_larger_sdr_picture_like_amf() {
    let stream = Negotiated {
        codec: 1,
        hdr: true,
        width: 1920,
        height: 1080,
        chroma_packing: 1,
        ..Default::default()
    };
    let plain = coded(&Negotiated::default());
    assert_eq!(plain.width, 1920);
    assert_eq!(plain.chroma_packing, 0);
    stream.validate().unwrap();
    let packed = coded(&stream);
    assert_eq!((packed.width, packed.height), stream.coded_size());
    assert!(!packed.hdr && packed.sdr_10bit && !packed.yuv444);
    assert_eq!(packed.csc_mode, 3);
    assert_eq!(packed.chroma_packing, 0);
    packed.validate().unwrap();
}
#[test]
fn throughput_names_map_to_their_codec_only() {
    assert_eq!(throughput_codec("MaxThroughput"), Some(0));
    assert_eq!(throughput_codec("HevcMaxThroughput"), Some(1));
    assert_eq!(throughput_codec("Av1CapMaxThroughput"), Some(2));
    assert_eq!(throughput_codec("MaxBitrate"), None);
    assert_eq!(codec_name(2), "AV1");
}
#[test]
fn the_frame_cap_lowers_the_vbv_under_it_and_a_steady_link_never_reconfigures() -> Result<()> {
    reset();
    let mut encoder = session(&Negotiated::default(), ApiVersion(13, 0))?;
    let ceiling = encoder.config.rcParams.vbvBufferSize;
    assert!(ceiling > 0);
    assert_eq!(encoder.vbv_ceiling, ceiling);
    let frame = u64::from(ceiling).div_ceil(8);
    assert_eq!(encoder.frame_cap(), Some(frame));
    // A cap over the one-frame VBV is already held: no reconfigure, and the
    // cap is the bound frames report.
    assert_eq!(encoder.set_frame_cap(frame * 3)?, Some(frame * 3));
    state(|d| assert!(d.reconfigured.is_none()));
    // A cap under it lowers the VBV, without a reset or a keyframe.
    assert_eq!(encoder.set_frame_cap(frame / 2)?, Some(frame / 2));
    assert_eq!(
        u64::from(encoder.config.rcParams.vbvBufferSize),
        frame / 2 * 8
    );
    assert_eq!(encoder.config.rcParams.averageBitRate, 20_000_000);
    state(|d| {
        let (parameters, config) = d.reconfigured.take().unwrap();
        assert_eq!(parameters.resetEncoder(), 0);
        assert_eq!(parameters.forceIDR(), 0);
        assert_eq!(u64::from(config.rcParams.vbvBufferSize), frame / 2 * 8);
    });
    assert!(!encoder.force_idr);
    // Jitter within a tenth leaves it.
    encoder.set_frame_cap(frame / 2 + frame / 40)?;
    encoder.set_frame_cap(frame / 2 - frame / 40)?;
    state(|d| assert!(d.reconfigured.is_none()));
    // The submitted frame carries the cap and the host's target.
    let slot = input(&mut encoder, 17)?;
    encoder.submit(slot, None, true, Instant::now())?;
    let output = encoder.poll()?;
    assert_eq!(output[0].frame_cap_target, Some(frame / 2 - frame / 40));
    assert_eq!(output[0].frame_cap_bytes, Some(frame / 2));
    // Back over the ceiling: the VBV returns to it at once.
    encoder.set_frame_cap(frame * 2)?;
    assert_eq!(encoder.config.rcParams.vbvBufferSize, ceiling);
    state(|d| assert!(d.reconfigured.take().is_some()));
    // A bitrate change moves the ceiling with the rate.
    encoder.bitrate(10000)?;
    assert_eq!(encoder.vbv_ceiling, ceiling / 2);
    assert_eq!(encoder.config.rcParams.vbvBufferSize, ceiling / 2);
    // A refused change is not retried for the same cap.
    state(|d| d.reject_reconfigure = true);
    assert!(encoder.set_frame_cap(frame / 8).is_err());
    state(|d| d.reconfigured = None);
    assert!(encoder.set_frame_cap(frame / 8).is_ok());
    state(|d| assert!(d.reconfigured.is_none()));
    drop(encoder);
    assert_clean();
    Ok(())
}
#[test]
fn a_five_frame_vbv_is_the_bound_until_the_host_caps_under_it() -> Result<()> {
    reset();
    let one = session(&Negotiated::default(), ApiVersion(13, 0))?;
    let single = u64::from(one.config.rcParams.vbvBufferSize);
    drop(one);
    reset();
    // "Reference quality" sets nvenc_vbv_increase = 400: five frames.
    let mut encoder = session_with(
        &Negotiated::default(),
        ApiVersion(13, 0),
        "nvenc_vbv_increase = 400\n",
    )?;
    let ceiling = encoder.config.rcParams.vbvBufferSize;
    assert!(u64::from(ceiling).abs_diff(single * 5) <= 5, "{ceiling}");
    let five = u64::from(ceiling).div_ceil(8);
    assert_eq!(encoder.frame_cap(), Some(five));
    // A cap over the five frames is the reported bound without a change.
    assert_eq!(encoder.set_frame_cap(five * 2)?, Some(five * 2));
    state(|d| assert!(d.reconfigured.is_none()));
    // A link that carries two frames' worth lowers the VBV to it.
    let link = single.div_ceil(8) * 2;
    assert_eq!(encoder.set_frame_cap(link)?, Some(link));
    assert_eq!(u64::from(encoder.config.rcParams.vbvBufferSize), link * 8);
    state(|d| assert!(d.reconfigured.take().is_some()));
    // Back over it: the five-frame VBV returns at once.
    assert_eq!(encoder.set_frame_cap(five + 1)?, Some(five + 1));
    assert_eq!(encoder.config.rcParams.vbvBufferSize, ceiling);
    drop(encoder);
    assert_clean();
    Ok(())
}
#[test]
fn a_frame_cap_applies_with_frames_in_flight_and_a_bitrate_rise_waits() -> Result<()> {
    reset();
    state(|d| d.asynchronous = true);
    let mut encoder = session(&Negotiated::default(), ApiVersion(13, 0))?;
    let frame = encoder.frame_cap().unwrap();
    let slot = input(&mut encoder, 17)?;
    encoder.submit(slot, None, true, Instant::now())?;
    // Lowering the VBV adds no wait for the frame in flight.
    encoder.set_frame_cap(frame / 2)?;
    state(|d| assert!(d.reconfigured.take().is_some()));
    assert_eq!(encoder.pending.len(), 1);
    // Neither does a lower bitrate.
    encoder.bitrate(10000)?;
    assert_eq!(encoder.pending.len(), 1);
    // A higher one resets the encoder, so the frame is collected first.
    encoder.bitrate(40000)?;
    assert!(encoder.pending.is_empty());
    state(|d| assert_eq!(d.reconfigured.take().unwrap().0.resetEncoder(), 1));
    assert_eq!(encoder.poll()?.len(), 1);
    drop(encoder);
    assert_clean();
    Ok(())
}
#[test]
fn a_frame_over_twice_its_cap_warns_at_most_every_ten_seconds() -> Result<()> {
    reset();
    let mut encoder = session(&Negotiated::default(), ApiVersion(13, 0))?;
    let cap = encoder.frame_cap().unwrap() as usize;
    let slot = input(&mut encoder, 17)?;
    state(|d| d.picture = cap * 2);
    encoder.submit(slot, None, true, Instant::now())?;
    assert_eq!(encoder.poll()?[0].bytes.len(), cap * 2);
    assert!(encoder.oversize_warned.is_none());
    state(|d| d.picture = cap * 2 + 1);
    encoder.submit(slot, None, false, Instant::now())?;
    encoder.poll()?;
    let warned = encoder.oversize_warned.unwrap();
    encoder.submit(slot, None, false, Instant::now())?;
    encoder.poll()?;
    assert_eq!(encoder.oversize_warned, Some(warned));
    let earlier = warned - OVERSIZE_WARNING_INTERVAL;
    encoder.oversize_warned = Some(earlier);
    encoder.submit(slot, None, false, Instant::now())?;
    encoder.poll()?;
    assert!(encoder.oversize_warned.unwrap() > earlier);
    drop(encoder);
    assert_clean();
    Ok(())
}
#[test]
fn a_d3d12_session_waits_for_the_conversion_on_the_gpu_and_signals_its_fence() -> Result<()> {
    reset();
    let stream = Negotiated::default();
    let mut encoder = d3d12_session(&stream, ApiVersion(13, 0), "")?;
    let (parameters, _) = initialized();
    // Completion comes from the output fence, not from events.
    assert_eq!(parameters.enableEncodeAsync, 0);
    assert_eq!(parameters.bufferFormat, buffer_format(&stream));
    let slot = d3d12_input(&mut encoder, 17)?;
    // The READBACK buffer is registered as a byte buffer for the bitstream.
    state(|d| {
        assert_eq!(
            d.resources[&(17 | 0x1000)],
            _NV_ENC_BUFFER_FORMAT_NV_ENC_BUFFER_FORMAT_U8
        );
        assert!(d.outputs.is_empty(), "no driver bitstream buffer");
    });
    encoder.submit(slot, converted(5), true, Instant::now())?;
    let output = encoder.poll()?;
    assert_eq!(output.len(), 1);
    assert!(output[0].idr);
    assert_eq!(output[0].bytes[..8], 1u64.to_le_bytes());
    state(|d| assert_eq!(d.fence_points, [(CONVERSION_FENCE, 5, 1)]));
    // Until the driver signals the frame's value, the poll does not lock.
    state(|d| d.hold_fence = true);
    encoder.submit(slot, converted(6), false, Instant::now())?;
    assert!(encoder.poll()?.is_empty());
    state(|d| assert!(!d.calls.ends_with(&["lock output"])));
    state(|d| d.fence = 2);
    assert_eq!(encoder.poll()?.len(), 1);
    // A frame the fence never reaches fails loudly.
    encoder.submit(slot, converted(7), false, Instant::now())?;
    encoder.pending.front_mut().unwrap().started = Instant::now() - COMPLETION_TIMEOUT * 2;
    let error = encoder.poll().err().unwrap();
    assert!(format!("{error:#}").contains("timed out"), "{error:#}");
    state(|d| d.fence = 3);
    encoder.pending.front_mut().unwrap().started = Instant::now();
    assert_eq!(encoder.poll()?.len(), 1);
    // A D3D12 frame without its conversion's fence, or a D3D11 input, is refused.
    let error = encoder
        .submit(slot, None, false, Instant::now())
        .err()
        .unwrap();
    assert!(
        format!("{error:#}").contains("conversion's fence"),
        "{error:#}"
    );
    let error = input(&mut encoder, 18).err().unwrap();
    assert!(format!("{error:#}").contains("D3D12"), "{error:#}");
    drop(encoder);
    assert_clean();
    Ok(())
}
#[test]
fn a_d3d12_scene_cut_is_encoded_again_after_the_same_conversion() -> Result<()> {
    reset();
    let mut encoder = d3d12_session(&Negotiated::default(), ApiVersion(13, 0), "")?;
    let slot = d3d12_input(&mut encoder, 17)?;
    encoder.submit(slot, converted(1), true, Instant::now())?;
    encoder.poll()?;
    encoder.cut_limit = Some(4);
    state(|d| d.picture = 24);
    encoder.submit(slot, converted(2), false, Instant::now())?;
    let output = encoder.poll()?;
    assert_eq!(output.len(), 1);
    assert!(output[0].reencoded && output[0].idr);
    // The keyframe waits for the same conversion and signals a new value.
    state(|d| {
        assert_eq!(
            d.fence_points[1..],
            [(CONVERSION_FENCE, 2, 2), (CONVERSION_FENCE, 2, 3)]
        )
    });
    drop(encoder);
    assert_clean();
    Ok(())
}
#[test]
fn the_d3d12_structures_have_the_api_12_0_layout() {
    // nv-codec-headers n12.0.16.1 and n12.1.14.0 declare the same versioned
    // fence point and resource structures (SDK 11.1 had a fence point of
    // only a fence and a value), so the D3D12 session may open at API 12.0.
    use std::mem::{offset_of, size_of};
    assert_eq!(offset_of!(NV_ENC_FENCE_POINT_D3D12, pFence), 8);
    assert_eq!(offset_of!(NV_ENC_FENCE_POINT_D3D12, waitValue), 16);
    assert_eq!(offset_of!(NV_ENC_FENCE_POINT_D3D12, signalValue), 24);
    assert_eq!(size_of::<NV_ENC_FENCE_POINT_D3D12>(), 64);
    assert_eq!(offset_of!(NV_ENC_INPUT_RESOURCE_D3D12, pInputBuffer), 8);
    assert_eq!(offset_of!(NV_ENC_INPUT_RESOURCE_D3D12, inputFencePoint), 16);
    assert_eq!(size_of::<NV_ENC_INPUT_RESOURCE_D3D12>(), 272);
    assert_eq!(offset_of!(NV_ENC_OUTPUT_RESOURCE_D3D12, pOutputBuffer), 8);
    assert_eq!(
        offset_of!(NV_ENC_OUTPUT_RESOURCE_D3D12, outputFencePoint),
        16
    );
    assert_eq!(size_of::<NV_ENC_OUTPUT_RESOURCE_D3D12>(), 272);
    let mut point = NV_ENC_FENCE_POINT_D3D12::default();
    point.set_bWait(1);
    point.set_bSignal(1);
    // SAFETY: The bit fields follow the three 8-byte fields as one `uint32_t`.
    let bits = unsafe {
        *std::ptr::from_ref(&point)
            .cast::<u8>()
            .add(32)
            .cast::<u32>()
    };
    assert_eq!(bits, 0b11);
}
#[test]
fn a_d3d12_session_needs_api_12_and_has_no_slices_or_weighted_prediction() -> Result<()> {
    reset();
    let error = d3d12_session(&Negotiated::default(), ApiVersion(11, 0), "")
        .err()
        .unwrap();
    assert!(format!("{error:#}").contains("522.25"), "{error:#}");
    reset();
    let hevc = Negotiated {
        codec: 1,
        ..Default::default()
    };
    let encoder = d3d12_session(
        &hevc,
        ApiVersion(13, 0),
        "video_slice_delivery = 2\nnvenc_weighted_prediction = true\n",
    )?;
    assert!(!encoder.subframe);
    let (parameters, _) = initialized();
    assert_eq!(parameters.enableWeightedPrediction(), 0);
    assert_eq!(parameters.enableSubFrameWrite(), 0);
    drop(encoder);
    assert_clean();
    // Each D3D12 candidate is API 12.0 or newer.
    reset();
    state(|d| d.reject_open = Some(ApiVersion(12, 0)));
    assert!(d3d12_session(&Negotiated::default(), ApiVersion(12, 0), "").is_err());
    state(|d| assert_eq!(d.opened, [ApiVersion(12, 0)]));
    assert_clean();
    Ok(())
}
#[test]
fn a_focus_map_rides_on_inter_frames_and_turns_on_without_a_reset() -> Result<()> {
    reset();
    let mut encoder = session(&Negotiated::default(), ApiVersion(13, 0))?;
    // SAFETY: Initialization filled the rate control.
    assert_eq!(
        initialized().1.rcParams.qpMapMode,
        _NV_ENC_QP_MAP_MODE_NV_ENC_QP_MAP_DISABLED
    );
    let slot = input(&mut encoder, 17)?;
    // Without the delta map on, a map is not passed.
    encoder.next_map = Some(vec![-3, 3]);
    encoder.submit(slot, None, true, Instant::now())?;
    encoder.poll()?;
    encoder.enable_qp_map()?;
    state(|d| {
        let (parameters, config) = d.reconfigured.take().unwrap();
        assert_eq!((parameters.resetEncoder(), parameters.forceIDR()), (0, 0));
        assert_eq!(
            config.rcParams.qpMapMode,
            _NV_ENC_QP_MAP_MODE_NV_ENC_QP_MAP_DELTA
        );
    });
    assert!(!encoder.force_idr);
    encoder.enable_qp_map()?;
    state(|d| assert!(d.reconfigured.is_none(), "turned on once"));
    // A keyframe carries no map; an inter frame carries its own, once.
    encoder.next_map = Some(vec![-3, 3]);
    encoder.submit(slot, None, true, Instant::now())?;
    encoder.poll()?;
    encoder.next_map = Some(vec![-3, 3]);
    encoder.submit(slot, None, false, Instant::now())?;
    encoder.poll()?;
    encoder.submit(slot, None, false, Instant::now())?;
    encoder.poll()?;
    state(|d| assert_eq!(d.maps, [None, None, Some(vec![-3, 3]), None]));
    // A frame asked as an inter frame becomes a keyframe when the encoder
    // must make one, and the keyframe drops the map.
    encoder.force_idr = true;
    assert!(encoder.keyframe(false));
    encoder.next_map = Some(vec![-3, 3]);
    encoder.submit(slot, None, false, Instant::now())?;
    assert!(encoder.poll()?[0].idr);
    state(|d| assert_eq!(d.maps.last(), Some(&None)));
    drop(encoder);
    assert_clean();
    Ok(())
}
#[test]
fn block_statistics_are_read_for_measured_frames_from_api_12_1() -> Result<()> {
    reset();
    let stream = Negotiated::default();
    // API 12.0 has no output statistics.
    let encoder = session_with(&stream, ApiVersion(12, 0), "encoder_focus_verify = true\n")?;
    assert!(!encoder.block_stats);
    assert_eq!(initialized().0.enableOutputStats(), 0);
    drop(encoder);
    reset();
    let mut tuning = Tuning::new(&Config::default(), &stream)?;
    tuning.qp_map = true;
    tuning.block_stats = true;
    let mut encoder = open_tuned(&stream, ApiVersion(13, 0), &tuning, None)?;
    assert!(encoder.block_stats && encoder.qp_map);
    let (parameters, config) = initialized();
    assert_eq!(parameters.enableOutputStats(), 1);
    assert_eq!(
        parameters.outputStatsLevel,
        _NV_ENC_OUTPUT_STATS_LEVEL_NV_ENC_OUTPUT_STATS_BLOCK_LEVEL
    );
    assert_eq!(
        config.rcParams.qpMapMode,
        _NV_ENC_QP_MAP_MODE_NV_ENC_QP_MAP_DELTA
    );
    let slot = input(&mut encoder, 17)?;
    encoder.submit(slot, None, true, Instant::now())?;
    encoder.poll()?;
    assert!(encoder.qp_grids.is_empty(), "only measured frames are read");
    let (columns, rows) = butterpollo_core::nvenc::delta_grid(0, 1920, 1080);
    let blocks = (columns * rows) as usize;
    let map = (0..blocks)
        .map(|index| if index % 2 == 0 { -3 } else { 3 })
        .collect::<Vec<i8>>();
    encoder.next_map = Some(map);
    encoder.measure_next = true;
    encoder.submit(slot, None, false, Instant::now())?;
    encoder.poll()?;
    let (frame, grid) = encoder.qp_grids.pop_front().unwrap();
    assert_eq!(frame, 2);
    assert_eq!((grid.width, grid.height), (columns, rows));
    assert_eq!(grid.values[..4], [27, 33, 27, 33]);
    drop(encoder);
    assert_clean();
    Ok(())
}
#[test]
fn the_focus_probe_verdict_needs_every_grid_and_a_two_qp_effect() {
    use crate::amf::focus::Grid;
    let half = |left: u32, right: u32| Grid {
        width: 4,
        height: 1,
        values: vec![left, left, right, right],
    };
    let frames = crate::amf::focus::PROBE_FRAMES as usize;
    let mut grids = vec![half(27, 33); frames];
    grids.extend(vec![half(33, 27); frames]);
    let probe = probe_verdict(0, grids.clone());
    assert_eq!(
        probe.support,
        encoder_focus::Support::Verified { delta: 6.0 }
    );
    assert_eq!(probe.control_delta, Some(0.0));
    assert_eq!(probe.feedback_grid, Some((4, 1)));
    // A map that moves QP by less than two is not focus.
    let mut weak = vec![half(30, 31); frames];
    weak.extend(vec![half(31, 30); frames]);
    assert!(matches!(
        probe_verdict(0, weak).support,
        encoder_focus::Support::Unsupported { .. }
    ));
    // AV1 needs the same in q-index.
    assert!(matches!(
        probe_verdict(2, grids.clone()).support,
        encoder_focus::Support::Unsupported { .. }
    ));
    // A missing grid leaves the effect unknown.
    grids.pop();
    assert!(matches!(
        probe_verdict(0, grids).support,
        encoder_focus::Support::Unsupported { .. }
    ));
}
#[test]
fn a_driver_controlled_vbv_reports_no_frame_cap() -> Result<()> {
    reset();
    let mut encoder = session(&Negotiated::default(), ApiVersion(13, 0))?;
    encoder.config.rcParams.vbvBufferSize = 0;
    encoder.vbv_ceiling = 0;
    assert_eq!(encoder.set_frame_cap(1000)?, None);
    state(|d| assert!(d.reconfigured.is_none()));
    drop(encoder);
    assert_clean();
    Ok(())
}
#[test]
fn an_oversize_scene_cut_alone_in_flight_is_encoded_again_as_a_keyframe() -> Result<()> {
    reset();
    let mut encoder = session(&Negotiated::default(), ApiVersion(13, 0))?;
    let slot = input(&mut encoder, 17)?;
    let other = input(&mut encoder, 18)?;
    // Every fake picture is 8 bytes: past a 4-byte limit.
    encoder.cut_limit = Some(4);
    encoder.submit(slot, None, true, Instant::now())?;
    let output = encoder.poll()?;
    assert!(output[0].idr && !output[0].reencoded, "a keyframe is sent");
    let presentation = Instant::now() - Duration::from_millis(5);
    encoder.submit(slot, None, false, presentation)?;
    let output = encoder.poll()?;
    assert_eq!(output.len(), 1);
    assert!(output[0].idr && output[0].reencoded);
    // The keyframe keeps the cut's number and presentation.
    assert_eq!(output[0].bytes, 2u64.to_le_bytes());
    assert_eq!(output[0].presentation, Some(presentation));
    assert_eq!(encoder.reencoded_cuts, 1);
    assert_eq!(encoder.next, 3);
    // A cut with a later frame in flight is sent: that frame references it.
    encoder.submit(slot, None, false, Instant::now())?;
    encoder.submit(other, None, false, Instant::now())?;
    let output = encoder.poll()?;
    assert!(!output[0].idr && !output[0].reencoded);
    assert_eq!(output[0].bytes, 3u64.to_le_bytes());
    assert!(output[1].reencoded, "the newest is alone once collected");
    assert_eq!(encoder.reencoded_cuts, 2);
    // Without a limit nothing is encoded again.
    encoder.cut_limit = None;
    encoder.submit(slot, None, false, Instant::now())?;
    assert!(!encoder.poll()?[0].idr);
    drop(encoder);
    assert_clean();
    Ok(())
}
#[test]
fn a_failed_scene_cut_reencode_sends_the_inter_frame_and_says_so() -> Result<()> {
    reset();
    let mut encoder = session(&Negotiated::default(), ApiVersion(13, 0))?;
    let slot = input(&mut encoder, 17)?;
    encoder.submit(slot, None, true, Instant::now())?;
    encoder.poll()?;
    encoder.cut_limit = Some(4);
    encoder.submit(slot, None, false, Instant::now())?;
    state(|d| d.reject_submit = true);
    let output = encoder.poll()?;
    assert_eq!(output.len(), 1);
    assert!(!output[0].idr && !output[0].reencoded);
    assert!(encoder.reencode_failure.take().is_some());
    assert_eq!(encoder.reencoded_cuts, 0);
    // The rejected keyframe left nothing mapped, and the next frame is one.
    state(|d| {
        assert!(d.mapped.is_empty());
        d.reject_submit = false;
    });
    assert!(encoder.force_idr);
    encoder.submit(slot, None, false, Instant::now())?;
    let output = encoder.poll()?;
    assert!(output[0].idr && !output[0].reencoded);
    drop(encoder);
    assert_clean();
    Ok(())
}
#[test]
fn keyframes_past_a_tenth_over_the_target_are_counted() -> Result<()> {
    reset();
    let mut encoder = session(&Negotiated::default(), ApiVersion(13, 0))?;
    let slot = input(&mut encoder, 17)?;
    encoder.frame_cap_target = Some(7);
    encoder.submit(slot, None, true, Instant::now())?;
    encoder.poll()?;
    assert_eq!(encoder.keyframes_over_cap, 1, "8 bytes against 7");
    encoder.frame_cap_target = Some(8);
    encoder.submit(slot, None, true, Instant::now())?;
    encoder
        .submit(slot, None, false, Instant::now())
        .unwrap_err();
    encoder.poll()?;
    assert_eq!(encoder.keyframes_over_cap, 1);
    drop(encoder);
    assert_clean();
    Ok(())
}
#[test]
fn slice_delivery_reads_p_frames_slice_by_slice_in_subframe_mode() -> Result<()> {
    reset();
    state(|d| {
        d.asynchronous = true;
        d.picture = 24;
    });
    let hevc = Negotiated {
        codec: 1,
        ..Default::default()
    };
    let mut encoder = session_with(&hevc, ApiVersion(13, 0), "video_slice_delivery = 2\n")?;
    assert!(encoder.subframe);
    let (parameters, config) = initialized();
    // Subframe readback runs only synchronously, and HEVC never splits it.
    assert_eq!(parameters.enableEncodeAsync, 0);
    assert_eq!(parameters.reportSliceOffsets(), 1);
    assert_eq!(parameters.enableSubFrameWrite(), 1);
    assert_eq!(parameters.splitEncodeMode(), SPLIT_DISABLED);
    // SAFETY: An HEVC session fills this union member.
    let codec = unsafe { config.encodeCodecConfig.hevcConfig };
    assert_eq!((codec.sliceMode, codec.sliceModeData), (3, 2));
    assert_eq!(encoder.slice_offsets.len(), 120 * 68);
    let slot = input(&mut encoder, 17)?;
    // A keyframe waits for all of its slices and leaves whole.
    state(|d| d.partial.push_back((1, 12)));
    encoder.submit(slot, None, true, Instant::now())?;
    assert!(encoder.poll()?.is_empty());
    let output = encoder.poll()?;
    assert_eq!(output.len(), 1);
    assert_eq!(output[0].unit, crate::encoder::Unit::Whole);
    assert!(output[0].idr);
    assert_eq!(output[0].bytes.len(), 24);
    // A P-frame leaves as NVENC writes it: nothing yet, the first of two
    // slices written, the rest.
    state(|d| d.partial.extend([(0, 0), (2, 20)]));
    encoder.submit(slot, None, false, Instant::now())?;
    assert!(encoder.poll()?.is_empty());
    let first = encoder.poll()?;
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].unit, crate::encoder::Unit::Slice { index: 0 });
    assert!(!first[0].idr);
    assert!(first[0].frame_cap_bytes.is_some());
    assert_eq!(first[0].bytes[..8], 2u64.to_le_bytes());
    let last = encoder.poll()?;
    assert_eq!(last.len(), 1);
    assert_eq!(last[0].unit, crate::encoder::Unit::Last { index: 1 });
    assert!(!last[0].idr);
    let mut picture = first[0].bytes.clone();
    picture.extend_from_slice(&last[0].bytes);
    assert_eq!(picture.len(), 24);
    assert_eq!(picture[8..], (8..24).map(|i| i as u8).collect::<Vec<_>>());
    // Slices already written leave together when the poll comes late; the
    // newest waits for the next poll.
    state(|d| d.partial.push_back((5, 20)));
    encoder.submit(slot, None, false, Instant::now())?;
    assert_eq!(encoder.poll()?[0].bytes.len(), 16);
    assert_eq!(encoder.poll()?[0].bytes.len(), 8);
    // A single slice written may be the frame's last, so it waits.
    state(|d| d.partial.push_back((1, 10)));
    encoder.submit(slot, None, false, Instant::now())?;
    assert!(encoder.poll()?.is_empty());
    assert_eq!(encoder.poll()?[0].unit, crate::encoder::Unit::Whole);
    // A frame done at its first poll leaves whole, with full FEC.
    encoder.submit(slot, None, false, Instant::now())?;
    let output = encoder.poll()?;
    assert_eq!(output[0].unit, crate::encoder::Unit::Whole);
    // A started frame ends as it began once slices are joined; the next is
    // waited for and leaves whole.
    state(|d| d.partial.push_back((2, 10)));
    encoder.submit(slot, None, false, Instant::now())?;
    assert_eq!(
        encoder.poll()?[0].unit,
        crate::encoder::Unit::Slice { index: 0 }
    );
    encoder.slices_joined = true;
    assert_eq!(
        encoder.poll()?[0].unit,
        crate::encoder::Unit::Last { index: 1 }
    );
    state(|d| d.partial.push_back((1, 10)));
    encoder.submit(slot, None, false, Instant::now())?;
    assert!(encoder.poll()?.is_empty());
    let output = encoder.poll()?;
    assert_eq!(output[0].unit, crate::encoder::Unit::Whole);
    assert_eq!(output[0].bytes.len(), 24);
    // As on AMF, slice delivery never encodes a scene cut again.
    encoder.slices_joined = false;
    encoder.cut_limit = Some(4);
    encoder.submit(slot, None, false, Instant::now())?;
    assert!(!encoder.poll()?[0].reencoded);
    assert_eq!(encoder.reencoded_cuts, 0);
    drop(encoder);
    assert_clean();
    Ok(())
}
#[test]
fn slice_output_that_shrinks_fails_and_a_stuck_frame_times_out() -> Result<()> {
    reset();
    state(|d| d.picture = 24);
    let mut encoder = session_with(
        &Negotiated::default(),
        ApiVersion(13, 0),
        "video_slice_delivery = 4\n",
    )?;
    // SAFETY: An H.264 session fills this union member.
    let codec = unsafe { initialized().1.encodeCodecConfig.h264Config };
    assert_eq!((codec.sliceMode, codec.sliceModeData), (3, 4));
    let slot = input(&mut encoder, 17)?;
    encoder.submit(slot, None, true, Instant::now())?;
    encoder.poll()?;
    state(|d| d.partial.extend([(2, 12), (2, 4)]));
    encoder.submit(slot, None, false, Instant::now())?;
    assert_eq!(encoder.poll()?[0].bytes.len(), 6);
    let error = encoder.poll().err().unwrap();
    assert!(
        format!("{error:#}").contains("slice output size"),
        "{error:#}"
    );
    // The bytes already sent stay sent; the frame is still in flight.
    encoder.pending.front_mut().unwrap().started = Instant::now() - COMPLETION_TIMEOUT * 2;
    state(|d| d.partial.push_back((3, 18)));
    let error = encoder.poll().err().unwrap();
    assert!(format!("{error:#}").contains("timed out"), "{error:#}");
    encoder.pending.front_mut().unwrap().started = Instant::now();
    encoder.poll()?;
    drop(encoder);
    assert_clean();
    Ok(())
}
#[test]
fn a_subframe_frame_ends_at_the_drivers_completion_and_bad_offsets_hold_it() -> Result<()> {
    reset();
    state(|d| d.picture = 24);
    let mut encoder = session_with(
        &Negotiated::default(),
        ApiVersion(13, 0),
        "video_slice_delivery = 4\n",
    )?;
    let slot = input(&mut encoder, 17)?;
    encoder.submit(slot, None, true, Instant::now())?;
    encoder.poll()?;
    // Every slice and every byte reported before the driver says the
    // picture is done: the slices but the newest leave early, and the last
    // unit carries the newest at the completion.
    state(|d| d.partial.push_back((4, 24)));
    encoder.submit(slot, None, false, Instant::now())?;
    let first = encoder.poll()?;
    assert_eq!(first[0].unit, crate::encoder::Unit::Slice { index: 0 });
    assert_eq!(first[0].bytes.len(), 18);
    let last = encoder.poll()?;
    assert_eq!(last[0].unit, crate::encoder::Unit::Last { index: 1 });
    assert_eq!(last[0].bytes.len(), 6);
    // Slice offsets past the bytes written send nothing early, once warned.
    state(|d| {
        d.bad_offsets = true;
        d.partial.push_back((2, 12));
    });
    encoder.submit(slot, None, false, Instant::now())?;
    assert!(encoder.poll()?.is_empty());
    assert!(encoder.offsets_warned);
    let output = encoder.poll()?;
    assert_eq!(output[0].unit, crate::encoder::Unit::Whole);
    assert_eq!(output[0].bytes.len(), 24);
    assert_eq!(encoder.subframe_traced, 3);
    let lock = NV_ENC_LOCK_BITSTREAM {
        numSlices: 3,
        bitstreamSizeInBytes: 30,
        ..Default::default()
    };
    assert!(slices_consistent(&lock, &[0, 10, 20, 0]));
    assert!(!slices_consistent(&lock, &[0, 10, 10, 0]));
    assert!(!slices_consistent(&lock, &[0, 10, 30, 0]));
    assert!(!slices_consistent(&lock, &[0, 10]));
    drop(encoder);
    assert_clean();
    Ok(())
}
#[test]
fn without_subframe_readback_slice_delivery_stays_off_and_events_stay() -> Result<()> {
    reset();
    state(|d| {
        d.asynchronous = true;
        d.no_subframe = true;
    });
    let stream = Negotiated {
        intra_refresh: true,
        ..Default::default()
    };
    let encoder = session_with(&stream, ApiVersion(13, 0), "video_slice_delivery = 2\n")?;
    assert!(!encoder.subframe);
    let (parameters, config) = initialized();
    assert_eq!(parameters.enableEncodeAsync, 1);
    assert_eq!(parameters.enableSubFrameWrite(), 0);
    assert_eq!(parameters.reportSliceOffsets(), 0);
    // SAFETY: An H.264 session fills this union member.
    let codec = unsafe { config.encodeCodecConfig.h264Config };
    assert_eq!(
        codec.enableIntraRefresh(),
        1,
        "the client's intra refresh stays"
    );
    drop(encoder);
    // With subframe readback, slice delivery takes intra refresh's place, as
    // on AMF.
    reset();
    let encoder = session_with(&stream, ApiVersion(13, 0), "video_slice_delivery = 2\n")?;
    assert!(encoder.subframe);
    // SAFETY: An H.264 session fills this union member.
    let codec = unsafe { initialized().1.encodeCodecConfig.h264Config };
    assert_eq!(codec.enableIntraRefresh(), 0);
    drop(encoder);
    // HEVC split encoding on several engines keeps the frame budget, and
    // cannot run beside subframe readback; H.264 never splits.
    let hevc = Negotiated {
        codec: 1,
        ..Default::default()
    };
    reset();
    state(|d| d.engines = 2);
    let encoder = session_with(&hevc, ApiVersion(13, 0), "video_slice_delivery = 2\n")?;
    assert!(!encoder.subframe);
    assert_eq!(initialized().0.splitEncodeMode(), 0);
    drop(encoder);
    reset();
    state(|d| d.engines = 2);
    let encoder = session_with(
        &hevc,
        ApiVersion(13, 0),
        "video_slice_delivery = 2\nnvenc_split_encode = disabled\n",
    )?;
    assert!(encoder.subframe);
    drop(encoder);
    reset();
    state(|d| d.engines = 2);
    let encoder = session_with(
        &Negotiated::default(),
        ApiVersion(13, 0),
        "video_slice_delivery = 2\n",
    )?;
    assert!(encoder.subframe);
    drop(encoder);
    // Off by default at this size: nothing changes.
    reset();
    state(|d| d.asynchronous = true);
    let encoder = session(&Negotiated::default(), ApiVersion(13, 0))?;
    assert!(!encoder.subframe);
    assert_eq!(initialized().0.enableEncodeAsync, 1);
    drop(encoder);
    assert_clean();
    Ok(())
}
