//! Native NVIDIA NVENC encoder, called through NVIDIA's driver ABI.
//!
//! Supports NVENC API 11 to 13 and reference frame invalidation. GPU
//! resources stay owned until their output completes; 10-bit 4:4:4 input goes
//! through CUDA interop (`cuda`). The frame budget guard, VBR rate control,
//! fixed split-encode strips, the codec check and spatial AQ by default follow
//! Foundation Sunshine (AlkaidLab/foundation-sunshine, GPL-3.0, src/nvenc/).
//! HDR10+, HDR Vivid and Dolby Vision come from the D3D11 conversion's
//! statistics pass and are spliced in as with AMF (`amf::dynamic`).

mod cuda;

use crate::{
    capture::{Device, GpuImage, Image, Pixel},
    encoder::Encoded,
    nvenc_abi::*,
};
use anyhow::{Context, Result, bail};
use butterpollo_core::{
    config::Config,
    nvenc::{ApiVersion, Budget, RateControl, Recovery, References, SPLIT_DISABLED, Tuning},
    rtsp::Negotiated,
    session::Warnings,
};
use std::{
    collections::VecDeque,
    ffi::{CStr, c_void},
    ptr,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT},
        Graphics::Direct3D11::ID3D11Texture2D,
        System::{
            LibraryLoader::LOAD_LIBRARY_SEARCH_SYSTEM32,
            SystemInformation::GetSystemDirectoryW,
            Threading::{CreateEventW, WaitForSingleObject},
        },
    },
    core::Interface,
};

const SUCCESS: NVENCSTATUS = _NVENCSTATUS_NV_ENC_SUCCESS;
const MAX_PENDING: usize = 8;
const COMPLETION_TIMEOUT: Duration = Duration::from_millis(100);
const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;
/// The smallest VBV a bitrate change or a frame cap sets.
const MIN_VBV_BITS: u64 = 100_000;
/// The least time between warnings about a frame over twice its cap.
const OVERSIZE_WARNING_INTERVAL: Duration = Duration::from_secs(10);
/// Subframe frames whose locks are traced at debug level.
const TRACED_SUBFRAMES: u8 = 4;
type CreateInstance = unsafe extern "C" fn(*mut NV_ENCODE_API_FUNCTION_LIST) -> NVENCSTATUS;
type GetMaximum = unsafe extern "C" fn(*mut u32) -> NVENCSTATUS;

pub(crate) fn system_library(name: &str) -> Result<libloading::Library> {
    let mut directory = [0u16; 32768];
    // SAFETY: `directory` is a live, writable UTF-16 buffer and the call writes at most its length.
    let length = unsafe { GetSystemDirectoryW(Some(&mut directory)) } as usize;
    if length == 0 || length >= directory.len() {
        bail!("cannot locate the Windows system directory");
    }
    let path = std::path::PathBuf::from(String::from_utf16(&directory[..length])?).join(name);
    // SAFETY: `path` is an absolute System32 path, so no search-order DLL is picked up; loading
    // runs only the installed driver's initialisers.
    Ok(unsafe {
        libloading::os::windows::Library::load_with_flags(path, LOAD_LIBRARY_SEARCH_SYSTEM32.0)?
    }
    .into())
}
struct Runtime {
    _library: Option<libloading::Library>,
    create: CreateInstance,
    maximum: ApiVersion,
}
impl Runtime {
    fn load() -> Result<Arc<Self>> {
        // SAFETY: `library` moves into the Runtime, keeping the copied `create` pointer loaded;
        // both symbols use NVIDIA's documented C signatures.
        unsafe {
            let library =
                system_library("nvEncodeAPI64.dll").context("NVIDIA encode driver unavailable")?;
            let create = *library.get::<CreateInstance>(b"NvEncodeAPICreateInstance\0")?;
            let maximum = if let Ok(get) =
                library.get::<GetMaximum>(b"NvEncodeAPIGetMaxSupportedVersion\0")
            {
                let mut value = 0;
                check_raw(
                    get(&mut value),
                    "query maximum API",
                    &NV_ENCODE_API_FUNCTION_LIST::default(),
                    ptr::null_mut(),
                )?;
                ApiVersion::from_driver(value)
            } else {
                ApiVersion::COMPILED
            };
            Ok(Arc::new(Self {
                _library: Some(library),
                create,
                maximum,
            }))
        }
    }
}
#[derive(Debug)]
struct DriverError {
    status: NVENCSTATUS,
    operation: &'static str,
    detail: String,
}
impl std::fmt::Display for DriverError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "NVENC {} failed ({}): {}",
            self.operation, self.status, self.detail
        )
    }
}
impl std::error::Error for DriverError {}
fn check_raw(
    status: NVENCSTATUS,
    operation: &'static str,
    functions: &NV_ENCODE_API_FUNCTION_LIST,
    raw: *mut c_void,
) -> Result<()> {
    if status == SUCCESS {
        return Ok(());
    }
    let detail = if !raw.is_null() {
        functions
            .nvEncGetLastErrorString
            .and_then(|get| {
                // SAFETY: `raw` is non-null and every caller passes the live session that
                // `functions` was created for.
                let message = unsafe { get(raw) };
                (!message.is_null()).then(|| {
                    // SAFETY: The driver returns a NUL-terminated string that stays valid until the
                    // next call on `raw`.
                    unsafe { CStr::from_ptr(message) }
                        .to_string_lossy()
                        .into_owned()
                })
            })
            .unwrap_or_default()
    } else {
        String::new()
    };
    Err(DriverError {
        status,
        operation,
        detail,
    }
    .into())
}
fn retry_version(error: &anyhow::Error) -> bool {
    error.downcast_ref::<DriverError>().is_some_and(|e| {
        matches!(
            e.status,
            _NVENCSTATUS_NV_ENC_ERR_INVALID_VERSION | _NVENCSTATUS_NV_ENC_ERR_UNSUPPORTED_PARAM
        )
    })
}
fn validate(functions: &NV_ENCODE_API_FUNCTION_LIST) -> Result<()> {
    macro_rules! require { ($($entry:ident),+ $(,)?) => { $(
        if functions.$entry.is_none() { bail!(concat!("NVENC driver lacks ", stringify!($entry))); }
    )+ }; }
    require!(
        nvEncOpenEncodeSessionEx,
        nvEncGetEncodeCaps,
        nvEncInitializeEncoder,
        nvEncRegisterResource,
        nvEncUnregisterResource,
        nvEncMapInputResource,
        nvEncUnmapInputResource,
        nvEncCreateBitstreamBuffer,
        nvEncDestroyBitstreamBuffer,
        nvEncEncodePicture,
        nvEncLockBitstream,
        nvEncUnlockBitstream,
        nvEncDestroyEncoder
    );
    Ok(())
}
fn codec_guid(codec: u8) -> Result<GUID> {
    match codec {
        0 => Ok(NV_ENC_CODEC_H264_GUID),
        1 => Ok(NV_ENC_CODEC_HEVC_GUID),
        2 => Ok(NV_ENC_CODEC_AV1_GUID),
        _ => bail!("unsupported NVENC codec"),
    }
}
fn preset_guid(preset: usize) -> GUID {
    [
        NV_ENC_PRESET_P1_GUID,
        NV_ENC_PRESET_P2_GUID,
        NV_ENC_PRESET_P3_GUID,
        NV_ENC_PRESET_P4_GUID,
        NV_ENC_PRESET_P5_GUID,
        NV_ENC_PRESET_P6_GUID,
        NV_ENC_PRESET_P7_GUID,
    ][preset.clamp(1, 7) - 1]
}
fn buffer_format(config: &Negotiated) -> NV_ENC_BUFFER_FORMAT {
    match (config.yuv444, config.ten_bit()) {
        (false, false) => _NV_ENC_BUFFER_FORMAT_NV_ENC_BUFFER_FORMAT_NV12,
        (false, true) => _NV_ENC_BUFFER_FORMAT_NV_ENC_BUFFER_FORMAT_YUV420_10BIT,
        (true, false) => _NV_ENC_BUFFER_FORMAT_NV_ENC_BUFFER_FORMAT_AYUV,
        (true, true) => _NV_ENC_BUFFER_FORMAT_NV_ENC_BUFFER_FORMAT_YUV444_10BIT,
    }
}
fn vui(config: &Negotiated) -> NV_ENC_CONFIG_H264_VUI_PARAMETERS {
    let (primaries, transfer, matrix) = colors(config);
    NV_ENC_CONFIG_H264_VUI_PARAMETERS {
        videoSignalTypePresentFlag: 1,
        videoFormat: 5,
        videoFullRangeFlag: u32::from(config.full_range()),
        colourDescriptionPresentFlag: 1,
        colourPrimaries: primaries,
        transferCharacteristics: transfer,
        colourMatrix: matrix,
        chromaSampleLocationFlag: u32::from(!config.yuv444),
        bitstreamRestrictionFlag: 1,
        ..Default::default()
    }
}
fn colors(config: &Negotiated) -> (u32, u32, u32) {
    let (primaries, matrix, transfer) = match config.color_matrix() {
        0 => (6, 6, 6),
        2 => (9, 9, 14),
        _ => (1, 1, 1),
    };
    // PQ, or ARIB STD-B67 for HLG and Dolby Vision 8.4.
    let transfer = match (config.hdr, config.hlg()) {
        (true, true) => 18,
        (true, false) => 16,
        _ => transfer,
    };
    (primaries, transfer, matrix)
}
fn same_guid(a: &GUID, b: &GUID) -> bool {
    a.Data1 == b.Data1 && a.Data2 == b.Data2 && a.Data3 == b.Data3 && a.Data4 == b.Data4
}
/// The codec's name for a refusal.
fn codec_name(codec: u8) -> &'static str {
    match codec {
        0 => "H.264",
        1 => "HEVC",
        _ => "AV1",
    }
}
/// The NVENC capability behind AMF's throughput property names, for the
/// codec it names.
fn throughput_codec(name: &str) -> Option<u8> {
    match name {
        "MaxThroughput" => Some(0),
        "HevcMaxThroughput" => Some(1),
        "Av1CapMaxThroughput" => Some(2),
        _ => None,
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct EncodedMetadata {
    primaries: [[u16; 2]; 3],
    white: [u16; 2],
    maximum: u32,
    minimum: u32,
    max_cll: u16,
    max_fall: u16,
}
#[derive(Default)]
struct Metadata {
    mastering: Box<MASTERING_DISPLAY_INFO>,
    light: Box<CONTENT_LIGHT_LEVEL>,
}
impl Metadata {
    fn new(codec: u8) -> Self {
        let mut metadata = Self::default();
        metadata.update(butterpollo_core::hdr::Metadata::default(), codec);
        metadata
    }
    fn update(&mut self, metadata: butterpollo_core::hdr::Metadata, codec: u8) {
        // NVENC takes each codec's syntax units, not a common HDR10 unit.
        // AV1 uses 0.16 chromaticities, 24.8 maximum and 18.14 minimum nits;
        // HEVC uses 1/50000 chromaticities and 1/10000 nit luminance.
        let point = |[x, y]: [u16; 2]| {
            let scale = |value| {
                if codec == 2 {
                    ((u32::from(value) * 65536 + 25000) / 50000).min(u32::from(u16::MAX)) as u16
                } else {
                    value
                }
            };
            CHROMA_POINTS {
                x: scale(x),
                y: scale(y),
            }
        };
        self.mastering.r = point(metadata.primaries[0]);
        self.mastering.g = point(metadata.primaries[1]);
        self.mastering.b = point(metadata.primaries[2]);
        self.mastering.whitePoint = point(metadata.white);
        self.mastering.maxLuma =
            u32::from(metadata.maximum_nits) * if codec == 2 { 256 } else { 10000 };
        self.mastering.minLuma = if codec == 2 {
            (u32::from(metadata.minimum) * 16384 + 5000) / 10000
        } else {
            u32::from(metadata.minimum)
        };
        self.light.maxContentLightLevel = metadata.max_cll;
        self.light.maxPicAverageLightLevel = metadata.max_fall;
    }
    fn encoded(&self) -> EncodedMetadata {
        let point = |p: CHROMA_POINTS| [p.x, p.y];
        EncodedMetadata {
            primaries: [
                point(self.mastering.r),
                point(self.mastering.g),
                point(self.mastering.b),
            ],
            white: point(self.mastering.whitePoint),
            maximum: self.mastering.maxLuma,
            minimum: self.mastering.minLuma,
            max_cll: self.light.maxContentLightLevel,
            max_fall: self.light.maxPicAverageLightLevel,
        }
    }
}
struct Input {
    key: usize,
    raw: *mut c_void,
    kind: NV_ENC_INPUT_RESOURCE_TYPE,
    pitch: u32,
    _texture: Option<ID3D11Texture2D>,
    cuda: Option<cuda::Input>,
}
struct Slot {
    input: Input,
    metadata: Metadata,
    registered: NV_ENC_REGISTERED_PTR,
    mapped: NV_ENC_INPUT_PTR,
    output: NV_ENC_OUTPUT_PTR,
    event: HANDLE,
    event_registered: bool,
    signaled: bool,
}
impl Drop for Slot {
    fn drop(&mut self) {
        if !self.event.is_invalid() {
            // SAFETY: This slot alone owns the event that `Session::slot` created.
            let _ = unsafe { CloseHandle(self.event) };
        }
    }
}
struct Pending {
    slot: usize,
    frame: u64,
    started: Instant,
    presentation: Instant,
    after_invalidation: bool,
    /// Submitted as a keyframe.
    idr: bool,
    /// The keyframe encoded again in place of an oversize scene cut.
    reencoded: bool,
    /// [`Session::frame_cap`], [`Session::frame_cap_target`] and
    /// [`Session::cut_limit`] at submission.
    frame_cap_bytes: Option<u64>,
    frame_cap_target: Option<u64>,
    cut_limit: Option<u64>,
    /// With slice delivery, a P-frame that may leave slice by slice: not a
    /// keyframe or a recovery frame, and under a frame cap.
    streams: bool,
    /// The units of a frame that has begun to leave slice by slice.
    units: Option<Units>,
    _converted: Option<Arc<ID3D11Texture2D>>,
}
/// How far a frame sent slice by slice has left.
#[derive(Clone, Copy, Debug, Default)]
struct Units {
    /// Bytes of the picture already handed on.
    sent: usize,
    /// The index of the next unit.
    next: u32,
}
/// A frame submitted once more ([`Session::reencode`]): it keeps its number
/// and its encode time counts from the first submission, as AMF counts it.
#[derive(Clone, Copy)]
struct Again {
    frame: u64,
    started: Instant,
}
struct Session {
    raw: *mut c_void,
    functions: NV_ENCODE_API_FUNCTION_LIST,
    api: ApiVersion,
    _runtime: Arc<Runtime>,
    cuda: Option<Rc<cuda::Context>>,
    config: Box<NV_ENC_CONFIG>,
    initialize: Box<NV_ENC_INITIALIZE_PARAMS>,
    stream: Negotiated,
    metadata: Metadata,
    submitted_metadata: Option<EncodedMetadata>,
    slots: Vec<Slot>,
    pending: VecDeque<Pending>,
    ready: VecDeque<Encoded>,
    references: References,
    retained: u32,
    invalidation: bool,
    dynamic_bitrate: bool,
    /// The rate control the driver accepted, for bitrate changes.
    rate_control: RateControl,
    /// The frame budget guard's verdict on the configured preset.
    budget: Option<Budget>,
    /// The one-frame VBV, in bits, that the settings give at the current
    /// bitrate; [`Session::set_frame_cap`] lowers the session's VBV under it
    /// and never raises it above. 0 when the driver controls buffering.
    vbv_ceiling: u32,
    /// The last frame cap the VBV was set for, against small changes.
    cap_requested: Option<u64>,
    /// The host's last pace-based frame cap ([`Session::set_frame_cap`]).
    frame_cap_target: Option<u64>,
    /// The largest inter frame sent as encoded ([`Session::reencodes`]).
    cut_limit: Option<u64>,
    reencoded_cuts: u64,
    keyframes_over_cap: u64,
    /// When a frame over twice its cap was last reported.
    oversize_warned: Option<Instant>,
    /// Why encoding a scene cut again failed, until the encoder reports it.
    reencode_failure: Option<String>,
    /// Blocks a P-frame is sent in when slice delivery is on, else 0.
    slice_blocks: u8,
    /// NVENC writes each slice as it finishes (`enableSubFrameWrite`), and
    /// frames are read while they encode: slice delivery is on.
    subframe: bool,
    /// The slices of each frame in subframe mode.
    subframe_slices: u32,
    /// Subframe frames completed, for the first frames' lock trace.
    subframe_traced: u8,
    /// A subframe lock's slice offsets were inconsistent once.
    offsets_warned: bool,
    /// Slice delivery is off for the rest of the session
    /// ([`Encoder::join_slices`]).
    slices_joined: bool,
    /// `sliceOffsets` for a subframe lock, one entry per macroblock as the
    /// API asks.
    slice_offsets: Vec<u32>,
    /// A frame sent slice by slice came out a keyframe, and the log says so.
    unit_idr_reported: bool,
    next: u64,
    initialized: bool,
    force_idr: bool,
}
impl Session {
    fn set_hdr_metadata(&mut self, metadata: butterpollo_core::hdr::Metadata) {
        self.metadata.update(metadata, self.stream.codec);
    }
    fn open(
        runtime: Arc<Runtime>,
        device: *mut c_void,
        kind: NV_ENC_DEVICE_TYPE,
        cuda: Option<Rc<cuda::Context>>,
        stream: &Negotiated,
        tuning: &Tuning,
    ) -> Result<Self> {
        let _guard = cuda.as_ref().map(|ctx| ctx.enter()).transpose()?;
        let mut failures = Vec::new();
        for api in ApiVersion::candidates(stream.codec, runtime.maximum) {
            match Self::open_version(
                runtime.clone(),
                device,
                kind,
                cuda.clone(),
                stream,
                tuning,
                api,
            ) {
                Ok(session) => return Ok(session),
                Err(error) if retry_version(&error) => {
                    tracing::warn!(error = %format!("{error:#}"), major = api.0, minor = api.1, "NVENC rejected this driver API; retrying a reviewed older API. Update the NVIDIA driver if encoding remains unstable");
                    failures.push(format!("{}.{}: {error}", api.0, api.1))
                }
                Err(error) => return Err(error),
            }
        }
        bail!(
            "no reviewed NVENC driver API supports this stream: {}",
            failures.join("; ")
        )
    }
    fn open_version(
        runtime: Arc<Runtime>,
        device: *mut c_void,
        kind: NV_ENC_DEVICE_TYPE,
        cuda: Option<Rc<cuda::Context>>,
        stream: &Negotiated,
        tuning: &Tuning,
        api: ApiVersion,
    ) -> Result<Self> {
        let mut functions = NV_ENCODE_API_FUNCTION_LIST {
            version: api.structure(2, false),
            ..Default::default()
        };
        check_raw(
            // SAFETY: `create` is NvEncodeAPICreateInstance from the still-loaded DLL and
            // `functions` is versioned.
            unsafe { (runtime.create)(&mut functions) },
            "create function table",
            &functions,
            ptr::null_mut(),
        )?;
        validate(&functions)?;
        let mut session = Self {
            raw: ptr::null_mut(),
            functions,
            api,
            _runtime: runtime,
            cuda,
            config: Box::default(),
            initialize: Box::default(),
            stream: stream.clone(),
            metadata: Metadata::new(stream.codec),
            submitted_metadata: None,
            slots: Vec::new(),
            pending: VecDeque::new(),
            ready: VecDeque::new(),
            references: References::default(),
            retained: 1,
            invalidation: false,
            dynamic_bitrate: false,
            rate_control: RateControl::Cbr,
            budget: None,
            vbv_ceiling: 0,
            cap_requested: None,
            frame_cap_target: None,
            cut_limit: None,
            reencoded_cuts: 0,
            keyframes_over_cap: 0,
            oversize_warned: None,
            reencode_failure: None,
            slice_blocks: 0,
            subframe: false,
            subframe_slices: 0,
            subframe_traced: 0,
            offsets_warned: false,
            slices_joined: false,
            slice_offsets: Vec::new(),
            unit_idr_reported: false,
            next: 1,
            initialized: false,
            force_idr: false,
        };
        let mut open = NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS {
            version: api.structure(1, false),
            deviceType: kind,
            device,
            apiVersion: api.packed(),
            ..Default::default()
        };
        // The owner exists before the driver call, so even a partially opened
        // rejected version is destroyed before the next candidate is tried.
        // SAFETY: `validate` confirmed nvEncOpenEncodeSessionEx exists, `device` is the caller's
        // live D3D11 device or CUDA context, and both out-pointers are live locals.
        let status =
            unsafe { (functions.nvEncOpenEncodeSessionEx.unwrap())(&mut open, &mut session.raw) };
        session.check(status, "open session")?;
        if session.raw.is_null() {
            bail!("NVENC returned no session");
        }
        session.configure(tuning)?;
        Ok(session)
    }
    fn guard(&self) -> Result<Option<cuda::Guard>> {
        self.cuda.as_ref().map(|ctx| ctx.enter()).transpose()
    }
    fn check(&self, status: NVENCSTATUS, operation: &'static str) -> Result<()> {
        check_raw(status, operation, &self.functions, self.raw)
    }
    fn cap(&self, cap: NV_ENC_CAPS) -> Result<i32> {
        let mut parameters = NV_ENC_CAPS_PARAM {
            version: self.api.structure(1, false),
            capsToQuery: cap,
            ..Default::default()
        };
        let mut value = 0;
        self.check(
            // SAFETY: `self.raw` is the open session, `validate` checked nvEncGetEncodeCaps, and
            // outputs are locals.
            unsafe {
                (self.functions.nvEncGetEncodeCaps.unwrap())(
                    self.raw,
                    codec_guid(self.stream.codec)?,
                    &mut parameters,
                    &mut value,
                )
            },
            "query capability",
        )?;
        Ok(value)
    }
    fn optional_cap(&self, cap: NV_ENC_CAPS) -> bool {
        self.cap(cap).unwrap_or(0) > 0
    }
    /// A GPU whose NVENC lacks the stream's codec is refused by name, before
    /// any capability query fails less clearly. Drivers without the list
    /// leave it to the session.
    fn check_codec(&self) -> Result<()> {
        let (Some(count), Some(list)) = (
            self.functions.nvEncGetEncodeGUIDCount,
            self.functions.nvEncGetEncodeGUIDs,
        ) else {
            return Ok(());
        };
        let mut total = 0;
        // SAFETY: `self.raw` is the open session and `total` a live local.
        self.check(unsafe { count(self.raw, &mut total) }, "count codecs")?;
        let mut guids = vec![GUID::default(); total.min(64) as usize];
        let mut written = 0;
        self.check(
            // SAFETY: `guids` holds `guids.len()` writable GUIDs and `written` is a live local.
            unsafe {
                list(
                    self.raw,
                    guids.as_mut_ptr(),
                    guids.len() as u32,
                    &mut written,
                )
            },
            "list codecs",
        )?;
        let wanted = codec_guid(self.stream.codec)?;
        if !guids[..(written as usize).min(guids.len())]
            .iter()
            .any(|guid| same_guid(guid, &wanted))
        {
            bail!(
                "this NVIDIA GPU's NVENC cannot encode {}. Select another codec on the device",
                codec_name(self.stream.codec)
            );
        }
        Ok(())
    }
    fn configure(&mut self, tuning: &Tuning) -> Result<()> {
        let _guard = self.guard()?;
        self.stream.validate()?;
        self.check_codec()?;
        if self.stream.width > self.cap(_NV_ENC_CAPS_NV_ENC_CAPS_WIDTH_MAX)?.max(0) as u32
            || self.stream.height > self.cap(_NV_ENC_CAPS_NV_ENC_CAPS_HEIGHT_MAX)?.max(0) as u32
        {
            bail!("stream dimensions exceed NVENC capability");
        }
        if self.stream.ten_bit()
            && (!self.optional_cap(_NV_ENC_CAPS_NV_ENC_CAPS_SUPPORT_10BIT_ENCODE)
                || (self.stream.codec == 0 && !self.api.modern_depth()))
        {
            bail!("this NVENC driver/codec cannot encode 10-bit input");
        }
        if self.stream.yuv444 && !self.optional_cap(_NV_ENC_CAPS_NV_ENC_CAPS_SUPPORT_YUV444_ENCODE)
        {
            bail!("this NVENC codec does not support 4:4:4");
        }
        // Slice delivery reads a frame while it encodes. The driver writes
        // each slice as it finishes only in synchronous mode, polled without
        // waiting; every other stream keeps completion events.
        self.slice_blocks = tuning.slice_blocks;
        let readback = tuning.slice_blocks > 0
            && self.stream.codec < 2
            && self.optional_cap(_NV_ENC_CAPS_NV_ENC_CAPS_SUPPORT_SUBFRAME_READBACK);
        // HEVC split encoding and subframe readback exclude each other. On a
        // GPU with several NVENC engines split encoding keeps the frame budget
        // (`frame_budget`), so it is kept.
        let splits = self.stream.codec == 1
            && tuning.split != SPLIT_DISABLED
            && self
                .cap(_NV_ENC_CAPS_NV_ENC_CAPS_NUM_ENCODER_ENGINES)
                .unwrap_or(1)
                > 1;
        self.subframe = readback && !splits;
        if readback && splits {
            tracing::info!(
                blocks = tuning.slice_blocks,
                "video_slice_delivery is on, but HEVC split encoding uses this GPU's NVENC engines and cannot return slices as they finish; frames leave whole. Set nvenc_split_encode = disabled to send slices instead"
            );
        } else if tuning.slice_blocks > 0 && !self.subframe {
            tracing::warn!(
                blocks = tuning.slice_blocks,
                "video_slice_delivery is on, but this NVENC driver cannot return a frame's slices as they finish; frames leave whole. Update the NVIDIA driver"
            );
        }
        self.subframe_slices = if self.subframe {
            tuning.subframe_slices(self.stream.slices)
        } else {
            0
        };
        self.slice_offsets = if self.subframe {
            vec![0; (self.stream.width.div_ceil(16) * self.stream.height.div_ceil(16)) as usize]
        } else {
            Vec::new()
        };
        // HEVC split encoding cannot run in subframe mode.
        let split = if self.subframe && self.stream.codec == 1 {
            SPLIT_DISABLED
        } else {
            tuning.split
        };
        let asynchronous = !self.subframe
            && self.optional_cap(_NV_ENC_CAPS_NV_ENC_CAPS_ASYNC_ENCODE_SUPPORT)
            && self.functions.nvEncRegisterAsyncEvent.is_some()
            && self.functions.nvEncUnregisterAsyncEvent.is_some();
        self.retained = if self.optional_cap(_NV_ENC_CAPS_NV_ENC_CAPS_SUPPORT_MULTIPLE_REF_FRAMES) {
            if self.stream.references > 0 {
                self.stream.references
            } else if self.stream.codec == 2 {
                8
            } else {
                5
            }
        } else {
            1
        };
        self.invalidation = self.retained > 1
            && self.functions.nvEncInvalidateRefFrames.is_some()
            && self.optional_cap(_NV_ENC_CAPS_NV_ENC_CAPS_SUPPORT_REF_PIC_INVALIDATION);
        self.dynamic_bitrate = self.functions.nvEncReconfigureEncoder.is_some()
            && self.optional_cap(_NV_ENC_CAPS_NV_ENC_CAPS_SUPPORT_DYN_BITRATE_CHANGE);
        let mut preset = NV_ENC_PRESET_CONFIG {
            version: self.api.preset(),
            presetCfg: NV_ENC_CONFIG {
                version: self.api.config(),
                ..Default::default()
            },
            ..Default::default()
        };
        // Multi-engine throughput counts only when split encoding may use it.
        let engines = if split == SPLIT_DISABLED {
            1
        } else {
            self.cap(_NV_ENC_CAPS_NV_ENC_CAPS_NUM_ENCODER_ENGINES)
                .unwrap_or(1)
                .max(1) as u32
        };
        let budget = butterpollo_core::nvenc::frame_budget(
            tuning.preset,
            (self.stream.width, self.stream.height),
            f64::from(self.stream.fps_millihz()) / 1000.,
            engines,
            tuning.frame_budget_guard,
        );
        if budget.effective < budget.configured {
            tracing::warn!(
                configured = budget.configured,
                effective = budget.effective,
                budget_ms = budget.budget_ms,
                configured_ms = budget.configured_ms,
                engines,
                over_at_floor = budget.over_at_floor,
                "NVENC frame budget guard lowered the preset so encoding keeps up with the frame rate. Turn off nvenc_frame_budget_guard to keep the configured preset"
            );
        }
        self.budget = Some(budget);
        let guid = codec_guid(self.stream.codec)?;
        let preset_guid = preset_guid(budget.effective);
        let result = if let Some(get) = self.functions.nvEncGetEncodePresetConfigEx {
            // SAFETY: `get` is the driver's own preset entry, `self.raw` is the open session and
            // `preset` is versioned.
            unsafe {
                get(
                    self.raw,
                    guid,
                    preset_guid,
                    NV_ENC_TUNING_INFO_NV_ENC_TUNING_INFO_ULTRA_LOW_LATENCY,
                    &mut preset,
                )
            }
        } else {
            _NVENCSTATUS_NV_ENC_ERR_UNIMPLEMENTED
        };
        if result != SUCCESS {
            if let Some(get) = self.functions.nvEncGetEncodePresetConfig {
                tracing::warn!(
                    code = result,
                    "NVENC low-latency preset query failed; using a compatible preset with explicit low-latency settings. Update the NVIDIA driver if frame times worsen"
                );
                preset = NV_ENC_PRESET_CONFIG {
                    version: self.api.preset(),
                    presetCfg: NV_ENC_CONFIG {
                        version: self.api.config(),
                        ..Default::default()
                    },
                    ..Default::default()
                };
                self.check(
                    // SAFETY: `get` is the driver's legacy preset entry for the open session and
                    // `preset` is versioned.
                    unsafe { get(self.raw, guid, preset_guid, &mut preset) },
                    "query compatible preset",
                )?;
            } else {
                self.check(result, "query low latency preset")?;
            }
        }
        *self.config = preset.presetCfg;
        self.config.version = self.api.config();
        self.config.gopLength = u32::MAX;
        self.config.frameIntervalP = 1;
        self.config.frameFieldMode =
            _NV_ENC_PARAMS_FRAME_FIELD_MODE_NV_ENC_PARAMS_FRAME_FIELD_MODE_FRAME;
        let custom_vbv = self.optional_cap(_NV_ENC_CAPS_NV_ENC_CAPS_SUPPORT_CUSTOM_VBV_BUF_SIZE);
        let temporal_aq =
            tuning.temporal_aq && self.optional_cap(_NV_ENC_CAPS_NV_ENC_CAPS_SUPPORT_TEMPORAL_AQ);
        if !custom_vbv {
            tracing::warn!(
                "NVENC custom VBV unavailable; the driver controls buffering and may increase latency. Update the NVIDIA driver"
            );
        }
        if tuning.temporal_aq && !temporal_aq {
            tracing::warn!(
                "NVENC temporal AQ unavailable; encoding without the requested quality feature. Update the NVIDIA driver or disable temporal AQ"
            );
        }
        let vbr_supported = self
            .cap(_NV_ENC_CAPS_NV_ENC_CAPS_SUPPORTED_RATECONTROL_MODES)
            .unwrap_or(0)
            & _NV_ENC_PARAMS_RC_MODE_NV_ENC_PARAMS_RC_VBR as i32
            != 0;
        self.rate_control = match tuning.rate_control {
            RateControl::Vbr { .. } if !vbr_supported => {
                tracing::warn!(
                    "NVENC VBR rate control unavailable on this GPU; encoding at a constant bitrate. Update the NVIDIA driver or set nvenc_rate_control to cbr"
                );
                RateControl::Cbr
            }
            chosen => chosen,
        };
        let rate_control = self.rate_control;
        let rc = &mut self.config.rcParams;
        rc.version = self.api.structure(1, false);
        rc.averageBitRate = self.stream.bitrate_kbps * 1000;
        rc.maxBitRate = rate_control.max_bitrate(rc.averageBitRate);
        match rate_control {
            RateControl::Cbr => rc.rateControlMode = _NV_ENC_PARAMS_RC_MODE_NV_ENC_PARAMS_RC_CBR,
            RateControl::Vbr { target_quality } => {
                rc.rateControlMode = _NV_ENC_PARAMS_RC_MODE_NV_ENC_PARAMS_RC_VBR;
                rc.targetQuality = target_quality;
            }
        }
        rc.set_zeroReorderDelay(1);
        rc.set_enableLookahead(0);
        rc.lookaheadDepth = 0;
        rc.lowDelayKeyFrameScale = 1;
        rc.multiPass = tuning.multipass;
        rc.set_enableAQ(u32::from(tuning.spatial_aq));
        rc.set_enableTemporalAQ(u32::from(temporal_aq));
        if custom_vbv {
            rc.vbvBufferSize = tuning.vbv(self.stream.bitrate_kbps, self.stream.fps_millihz());
            self.vbv_ceiling = rc.vbvBufferSize;
        }
        if let Some(qp) = tuning.min_qp {
            rc.set_enableMinQP(1);
            rc.minQP = NV_ENC_QP {
                qpInterP: qp,
                qpInterB: qp,
                qpIntra: qp,
            };
        }
        let intra_refresh = self.stream.intra_refresh
            && !self.subframe
            && self.optional_cap(_NV_ENC_CAPS_NV_ENC_CAPS_SUPPORT_INTRA_REFRESH);
        let single_slice =
            intra_refresh && self.optional_cap(_NV_ENC_CAPS_NV_ENC_CAPS_SINGLE_SLICE_INTRA_REFRESH);
        if self.stream.intra_refresh && self.subframe {
            // As AMF does with slice delivery.
            tracing::warn!(
                "video_slice_delivery is on: intra refresh is off for this stream and a lost frame is recovered with a keyframe"
            );
        } else if self.stream.intra_refresh && !intra_refresh {
            tracing::warn!(
                "NVENC intra-refresh unavailable; keeping IDR recovery, which can cause larger recovery frames. Reduce bitrate on lossy links or update the NVIDIA driver"
            );
        }
        let ten = self.stream.ten_bit();
        let depth = if ten { 10 } else { 8 };
        let chroma = if self.stream.yuv444 { 3 } else { 1 };
        let metadata = self.stream.hdr && self.api >= ApiVersion(13, 0);
        let cabac = !tuning.cavlc && self.optional_cap(_NV_ENC_CAPS_NV_ENC_CAPS_SUPPORT_CABAC);
        let slice_mode = self.subframe || tuning.slice_mode(self.stream.slices, self.api);
        let slices = if self.subframe {
            self.subframe_slices
        } else {
            self.stream.slices.max(1)
        };
        if self.stream.hdr && !metadata {
            tracing::warn!(
                "NVENC driver API cannot embed HDR mastering metadata; only control-channel metadata is available. Update the NVIDIA driver if HDR tone mapping is wrong"
            );
        }
        if self.stream.codec == 0 && !tuning.cavlc && !cabac {
            tracing::warn!(
                "NVENC CABAC unavailable; using CAVLC with lower compression efficiency. Update the NVIDIA driver or select HEVC or AV1"
            );
        }
        match self.stream.codec {
            0 => {
                self.config.profileGUID = if self.stream.yuv444 {
                    NV_ENC_H264_PROFILE_HIGH_444_GUID
                } else if ten {
                    NV_ENC_H264_PROFILE_HIGH_10_GUID
                } else {
                    NV_ENC_H264_PROFILE_HIGH_GUID
                };
                // SAFETY: The preset was queried for the H.264 GUID, so this union member is
                // active; it holds only integers and raw pointers.
                let codec = unsafe { &mut self.config.encodeCodecConfig.h264Config };
                codec.set_repeatSPSPPS(1);
                codec.idrPeriod = u32::MAX;
                if slice_mode {
                    codec.sliceMode = 3;
                    codec.sliceModeData = slices;
                }
                codec.chromaFormatIDC = chroma;
                codec.set_enableFillerDataInsertion(u32::from(tuning.filler));
                codec.entropyCodingMode = if cabac {
                    _NV_ENC_H264_ENTROPY_CODING_MODE_NV_ENC_H264_ENTROPY_CODING_MODE_CABAC
                } else {
                    _NV_ENC_H264_ENTROPY_CODING_MODE_NV_ENC_H264_ENTROPY_CODING_MODE_CAVLC
                };
                codec.maxNumRefFrames = self.retained;
                codec.numRefL0 = 1;
                if self.api.modern_depth() {
                    codec.inputBitDepth = depth;
                    codec.outputBitDepth = depth;
                }
                codec.h264VUIParameters = vui(&self.stream);
                codec.set_enableIntraRefresh(u32::from(intra_refresh));
                codec.set_singleSliceIntraRefresh(u32::from(single_slice));
                if intra_refresh {
                    codec.intraRefreshPeriod = 300;
                    codec.intraRefreshCnt = 299;
                    codec.set_outputRecoveryPointSEI(1);
                }
            }
            1 => {
                self.config.profileGUID = if self.stream.yuv444 {
                    NV_ENC_HEVC_PROFILE_FREXT_GUID
                } else if ten {
                    NV_ENC_HEVC_PROFILE_MAIN10_GUID
                } else {
                    NV_ENC_HEVC_PROFILE_MAIN_GUID
                };
                // SAFETY: The preset was queried for the HEVC GUID, so this union member is active;
                // it holds only integers and raw pointers.
                let codec = unsafe { &mut self.config.encodeCodecConfig.hevcConfig };
                codec.set_repeatSPSPPS(1);
                codec.idrPeriod = u32::MAX;
                if slice_mode {
                    codec.sliceMode = 3;
                    codec.sliceModeData = slices;
                }
                codec.set_chromaFormatIDC(chroma);
                codec.set_enableFillerDataInsertion(u32::from(tuning.filler));
                codec.maxNumRefFramesInDPB = self.retained;
                codec.numRefL0 = 1;
                if self.api.modern_depth() {
                    codec.inputBitDepth = depth;
                    codec.outputBitDepth = depth;
                } else if ten {
                    codec.set_reserved3(2);
                }
                codec.hevcVUIParameters = vui(&self.stream);
                codec.set_enableIntraRefresh(u32::from(intra_refresh));
                codec.set_singleSliceIntraRefresh(u32::from(single_slice));
                if intra_refresh {
                    codec.intraRefreshPeriod = 300;
                    codec.intraRefreshCnt = 299;
                    codec.set_outputRecoveryPointSEI(1);
                }
                if self.api >= ApiVersion(13, 0) {
                    codec.set_outputMasteringDisplay(u32::from(metadata));
                    codec.set_outputMaxCll(u32::from(metadata));
                }
            }
            2 => {
                self.config.profileGUID = NV_ENC_AV1_PROFILE_MAIN_GUID;
                // SAFETY: The preset was queried for the AV1 GUID, so this union member is active;
                // it holds only integers and raw pointers.
                let codec = unsafe { &mut self.config.encodeCodecConfig.av1Config };
                codec.set_repeatSeqHdr(1);
                codec.idrPeriod = u32::MAX;
                codec.set_chromaFormatIDC(chroma);
                codec.set_enableBitstreamPadding(u32::from(tuning.filler));
                codec.maxNumRefFramesInDPB = self.retained;
                codec.numFwdRefs = 1;
                if self.api.modern_depth() {
                    codec.inputBitDepth = depth;
                    codec.outputBitDepth = depth;
                } else if ten {
                    codec.set_enableTemporalSVC(1);
                    codec.set_reserved4(1);
                }
                let (primaries, transfer, matrix) = colors(&self.stream);
                codec.colorPrimaries = primaries;
                codec.transferCharacteristics = transfer;
                codec.matrixCoefficients = matrix;
                codec.colorRange = u32::from(self.stream.full_range());
                codec.chromaSamplePosition = u32::from(!self.stream.yuv444);
                if self.stream.slices > 1 {
                    let power = (self.stream.slices as f64).log2();
                    codec.numTileRows = 1 << (power / 2.).ceil() as u32;
                    codec.numTileColumns = 1 << (power / 2.).floor() as u32;
                }
                if self.api >= ApiVersion(13, 0) {
                    codec.set_outputMasteringDisplay(u32::from(metadata));
                    codec.set_outputMaxCll(u32::from(metadata));
                }
            }
            _ => unreachable!(),
        }
        *self.initialize = NV_ENC_INITIALIZE_PARAMS {
            version: self.api.initialize(),
            encodeGUID: guid,
            presetGUID: preset_guid,
            encodeWidth: self.stream.width,
            encodeHeight: self.stream.height,
            darWidth: self.stream.width,
            darHeight: self.stream.height,
            frameRateNum: self.stream.fps_millihz(),
            frameRateDen: 1000,
            enableEncodeAsync: u32::from(asynchronous),
            enablePTD: 1,
            tuningInfo: NV_ENC_TUNING_INFO_NV_ENC_TUNING_INFO_ULTRA_LOW_LATENCY,
            encodeConfig: self.config.as_mut(),
            ..Default::default()
        };
        let weighted = tuning.weighted_prediction
            && self.optional_cap(_NV_ENC_CAPS_NV_ENC_CAPS_SUPPORT_WEIGHTED_PREDICTION);
        self.initialize
            .set_enableWeightedPrediction(u32::from(weighted));
        if self.stream.codec != 0
            && self.api >= ApiVersion(12, 1)
            && (!(self.stream.codec == 1 && weighted) || split == SPLIT_DISABLED)
        {
            self.initialize.set_splitEncodeMode(split);
        }
        self.initialize
            .set_reportSliceOffsets(u32::from(self.subframe));
        self.initialize
            .set_enableSubFrameWrite(u32::from(self.subframe));
        // SAFETY: `validate` checked nvEncInitializeEncoder; `initialize` and the config it points
        // to are boxed in `self`, so both addresses stay valid for the session.
        let status = unsafe {
            (self.functions.nvEncInitializeEncoder.unwrap())(self.raw, self.initialize.as_mut())
        };
        self.check(status, "initialize encoder")?;
        self.initialized = true;
        tracing::info!(
            api_major = self.api.0,
            api_minor = self.api.1,
            preset = budget.effective,
            rate_control = ?self.rate_control,
            split,
            asynchronous,
            subframe_slices = self.subframe_slices,
            references = self.retained,
            rfi = self.invalidation,
            width = self.stream.width,
            height = self.stream.height,
            ten_bit = ten,
            yuv444 = self.stream.yuv444,
            "Rust native NVENC initialized"
        );
        Ok(())
    }
    fn slot(&mut self, input: Input) -> Result<usize> {
        if self.slots.len() >= MAX_PENDING {
            bail!("NVENC input registration limit reached");
        }
        let _guard = self.guard()?;
        let index = self.slots.len();
        self.slots.push(Slot {
            input,
            metadata: Metadata::new(self.stream.codec),
            registered: ptr::null_mut(),
            mapped: ptr::null_mut(),
            output: ptr::null_mut(),
            event: HANDLE::default(),
            event_registered: false,
            signaled: false,
        });
        // Every partial acquisition is now owned by Session::drop.
        let mut register = NV_ENC_REGISTER_RESOURCE {
            version: self.api.register(),
            resourceType: self.slots[index].input.kind,
            width: self.stream.width,
            height: self.stream.height,
            pitch: self.slots[index].input.pitch,
            resourceToRegister: self.slots[index].input.raw,
            bufferFormat: buffer_format(&self.stream),
            bufferUsage: _NV_ENC_BUFFER_USAGE_NV_ENC_INPUT_IMAGE,
            ..Default::default()
        };
        // SAFETY: `validate` checked the entry; `input.raw` is kept alive by the slot's texture
        // clone or CUDA allocation until `cleanup_slot` unregisters it.
        let status =
            unsafe { (self.functions.nvEncRegisterResource.unwrap())(self.raw, &mut register) };
        self.slots[index].registered = register.registeredResource;
        self.check(status, "register GPU input")?;
        if register.registeredResource.is_null() {
            bail!("NVENC returned no registered resource");
        }
        let mut output = NV_ENC_CREATE_BITSTREAM_BUFFER {
            version: self.api.structure(1, false),
            ..Default::default()
        };
        // SAFETY: `validate` checked the entry and `output` is versioned; the buffer is owned by
        // this slot.
        let status =
            unsafe { (self.functions.nvEncCreateBitstreamBuffer.unwrap())(self.raw, &mut output) };
        self.slots[index].output = output.bitstreamBuffer;
        self.check(status, "create output buffer")?;
        if output.bitstreamBuffer.is_null() {
            bail!("NVENC returned no output buffer");
        }
        if self.initialize.enableEncodeAsync != 0 {
            // SAFETY: No pointers are passed; the handle is owned by the slot and closed in
            // `Slot::drop`.
            self.slots[index].event = unsafe { CreateEventW(None, false, false, None) }?;
            let mut event = NV_ENC_EVENT_PARAMS {
                version: self.api.event(),
                completionEvent: self.slots[index].event.0,
                ..Default::default()
            };
            self.check(
                // SAFETY: `enableEncodeAsync` is set only if nvEncRegisterAsyncEvent exists;
                // `event` is the slot's handle.
                unsafe { (self.functions.nvEncRegisterAsyncEvent.unwrap())(self.raw, &mut event) },
                "register completion event",
            )?;
            self.slots[index].event_registered = true;
        }
        Ok(index)
    }
    fn texture_slot(&mut self, texture: &ID3D11Texture2D) -> Result<usize> {
        let key = texture.as_raw() as usize;
        if let Some(index) = self.slots.iter().position(|slot| slot.input.key == key) {
            return Ok(index);
        }
        let input = if let Some(context) = &self.cuda {
            let cuda = cuda::Input::new(context, texture, self.stream.width, self.stream.height)?;
            Input {
                key,
                raw: cuda.pointer as usize as *mut c_void,
                kind: _NV_ENC_INPUT_RESOURCE_TYPE_NV_ENC_INPUT_RESOURCE_TYPE_CUDADEVICEPTR,
                pitch: cuda.pitch,
                _texture: None,
                cuda: Some(cuda),
            }
        } else {
            Input {
                key,
                raw: texture.as_raw(),
                kind: _NV_ENC_INPUT_RESOURCE_TYPE_NV_ENC_INPUT_RESOURCE_TYPE_DIRECTX,
                pitch: 0,
                _texture: Some(texture.clone()),
                cuda: None,
            }
        };
        self.slot(input)
    }
    fn submit(
        &mut self,
        slot: usize,
        texture: Option<Arc<ID3D11Texture2D>>,
        idr: bool,
        presentation: Instant,
    ) -> Result<()> {
        self.submit_frame(slot, texture, idr, presentation, None)
    }
    fn submit_frame(
        &mut self,
        slot: usize,
        texture: Option<Arc<ID3D11Texture2D>>,
        idr: bool,
        presentation: Instant,
        again: Option<Again>,
    ) -> Result<()> {
        let _guard = self.guard()?;
        let frame = again.map_or(self.next, |again| again.frame);
        if self.pending.len() >= MAX_PENDING || self.pending.iter().any(|p| p.slot == slot) {
            bail!("NVENC input is still in flight");
        }
        if let Some(input) = self.slots[slot].input.cuda.as_mut() {
            input.copy()?;
        }
        let mut map = NV_ENC_MAP_INPUT_RESOURCE {
            version: self.api.structure(4, false),
            registeredResource: self.slots[slot].registered,
            ..Default::default()
        };
        // SAFETY: `registered` is this slot's live registration and no pending frame has it mapped
        // (checked above).
        let status = unsafe { (self.functions.nvEncMapInputResource.unwrap())(self.raw, &mut map) };
        self.slots[slot].mapped = map.mappedResource;
        self.check(status, "map encoder input")?;
        if map.mappedResource.is_null() || map.mappedBufferFmt != buffer_format(&self.stream) {
            bail!("NVENC mapped an incompatible input");
        }
        let metadata = (self.stream.hdr && self.stream.codec != 0 && self.api >= ApiVersion(13, 0))
            .then(|| self.metadata.encoded());
        // The driver emits HDR metadata on IDR/keyframes. Compare only values
        // actually encoded, and keep the old snapshot until a submission is
        // accepted so a failed attempt cannot consume a metadata update.
        let idr = idr || self.force_idr || metadata != self.submitted_metadata;
        let mut picture = NV_ENC_PIC_PARAMS {
            version: self.api.picture(),
            inputWidth: self.stream.width,
            inputHeight: self.stream.height,
            inputPitch: self.slots[slot].input.pitch,
            frameIdx: frame as u32,
            inputTimeStamp: frame,
            inputBuffer: map.mappedResource,
            outputBitstream: self.slots[slot].output,
            completionEvent: self.slots[slot].event.0,
            bufferFmt: map.mappedBufferFmt,
            pictureStruct: _NV_ENC_PIC_STRUCT_NV_ENC_PIC_STRUCT_FRAME,
            encodePicFlags: if idr {
                _NV_ENC_PIC_FLAGS_NV_ENC_PIC_FLAG_FORCEIDR
                    | _NV_ENC_PIC_FLAGS_NV_ENC_PIC_FLAG_OUTPUT_SPSPPS
            } else {
                0
            },
            ..Default::default()
        };
        if self.stream.hdr && self.api >= ApiVersion(13, 0) {
            *self.slots[slot].metadata.mastering = *self.metadata.mastering;
            *self.slots[slot].metadata.light = *self.metadata.light;
            {
                if self.stream.codec == 1 {
                    picture.codecPicParams.hevcPicParams.pMasteringDisplay =
                        self.slots[slot].metadata.mastering.as_mut();
                    picture.codecPicParams.hevcPicParams.pMaxCll =
                        self.slots[slot].metadata.light.as_mut();
                } else if self.stream.codec == 2 {
                    picture.codecPicParams.av1PicParams.pMasteringDisplay =
                        self.slots[slot].metadata.mastering.as_mut();
                    picture.codecPicParams.av1PicParams.pMaxCll =
                        self.slots[slot].metadata.light.as_mut();
                }
            }
        }
        let started = again.map_or_else(Instant::now, |again| again.started);
        // SAFETY: `picture` points at the slot's mapped input, output buffer and boxed metadata,
        // which all stay alive while the slot is in `pending`.
        let status =
            unsafe { (self.functions.nvEncEncodePicture.unwrap())(self.raw, &mut picture) };
        if status != SUCCESS && status != _NVENCSTATUS_NV_ENC_ERR_NEED_MORE_INPUT {
            self.force_idr |= idr;
            // The input stays registered; only this frame's mapping goes.
            // SAFETY: `mapped` is the mapping made above, which the rejected picture never took.
            let _ = unsafe {
                (self.functions.nvEncUnmapInputResource.unwrap())(self.raw, self.slots[slot].mapped)
            };
            self.slots[slot].mapped = ptr::null_mut();
            self.check(status, "submit picture")?;
        }
        self.force_idr = false;
        self.submitted_metadata = metadata;
        let after_invalidation = self.references.confirmation(idr);
        self.slots[slot].signaled = false;
        self.pending.push_back(Pending {
            slot,
            frame,
            started,
            presentation,
            after_invalidation,
            idr,
            reencoded: again.is_some(),
            frame_cap_bytes: self.frame_cap(),
            frame_cap_target: self.frame_cap_target,
            cut_limit: self.cut_limit,
            streams: self.subframe && !idr && !after_invalidation && self.frame_cap().is_some(),
            units: None,
            _converted: texture,
        });
        if again.is_none() {
            self.next = self
                .next
                .checked_add(1)
                .context("NVENC frame index overflow")?;
        }
        Ok(())
    }
    /// The bound the session targets for every frame: its one-frame VBV, or
    /// the host's cap when that is larger, since the VBV aims under it.
    /// `None` when the driver controls buffering.
    fn frame_cap(&self) -> Option<u64> {
        let vbv = u64::from(self.config.rcParams.vbvBufferSize).div_ceil(8);
        (vbv > 0).then(|| vbv.max(self.frame_cap_target.unwrap_or(0)))
    }
    /// The host's pace-based frame cap, which AMF writes as its frame size
    /// limit. NVENC targets it for every frame, keyframes too, with
    /// `lowDelayKeyFrameScale` 1 and a one-frame VBV, as Sunshine does; the
    /// driver does not guarantee the bound, so `collect` warns about a frame
    /// over twice it. The VBV is under the cap on any link that carries the
    /// stream; a cap under the VBV lowers the VBV to it. Changes within a
    /// tenth are skipped and the VBV returns to its ceiling at once, so a
    /// steady link never reconfigures.
    fn set_frame_cap(&mut self, bytes: u64) -> Result<Option<u64>> {
        if bytes == 0 {
            return Ok(self.frame_cap());
        }
        self.frame_cap_target = Some(bytes);
        let current = self.config.rcParams.vbvBufferSize;
        if self.vbv_ceiling == 0 || !self.dynamic_bitrate || current == 0 {
            return Ok(self.frame_cap());
        }
        let wanted = bytes.saturating_mul(8).clamp(
            MIN_VBV_BITS,
            u64::from(self.vbv_ceiling.max(MIN_VBV_BITS as u32)),
        ) as u32;
        let small = u64::from(wanted.abs_diff(current)) * 10 <= u64::from(current);
        if wanted == current
            || (small && wanted != self.vbv_ceiling)
            || self.cap_requested == Some(bytes)
        {
            return Ok(self.frame_cap());
        }
        self.cap_requested = Some(bytes);
        self.reconfigure(self.config.rcParams.averageBitRate, wanted, "frame cap")?;
        Ok(self.frame_cap())
    }
    /// Whether `encoded`, `pending`'s output, goes back to NVENC as a keyframe
    /// instead of to the client: the rule AMF follows
    /// ([`crate::amf::reencodes_cut`]), for an inter frame past the stream's
    /// full-FEC bound that the VBV did not hold, while nothing submitted after
    /// it can reference it. Not with slice delivery, as on AMF.
    fn reencodes(&self, pending: &Pending, encoded: &Encoded) -> bool {
        !self.subframe
            && crate::amf::reencodes_cut(
                encoded.idr || pending.idr || pending.reencoded,
                encoded.bytes.len() as u64,
                pending.cut_limit,
                self.pending.is_empty(),
            )
    }
    /// Submit `cut`'s input again as a keyframe with its frame number. The
    /// keyframe resets the references, so a later repair names only it.
    fn reencode(&mut self, cut: &Pending) -> Result<()> {
        self.submit_frame(
            cut.slot,
            cut._converted.clone(),
            true,
            cut.presentation,
            Some(Again {
                frame: cut.frame,
                started: cut.started,
            }),
        )?;
        self.reencoded_cuts += 1;
        Ok(())
    }
    fn collect(&mut self) -> Result<()> {
        let _guard = self.guard()?;
        while let Some(pending) = self.pending.front() {
            let index = pending.slot;
            let slot = &mut self.slots[index];
            if !slot.event.is_invalid() && !slot.signaled {
                // SAFETY: `slot.event` is a valid handle (checked above) owned by the slot.
                match unsafe { WaitForSingleObject(slot.event, 0) } {
                    WAIT_OBJECT_0 => slot.signaled = true,
                    WAIT_TIMEOUT => {
                        if pending.started.elapsed() > COMPLETION_TIMEOUT {
                            bail!("NVENC completion timed out");
                        }
                        break;
                    }
                    _ => bail!("NVENC completion event failed"),
                }
            }
            let mut lock = NV_ENC_LOCK_BITSTREAM {
                version: self.api.lock(),
                outputBitstream: slot.output,
                sliceOffsets: if self.subframe {
                    self.slice_offsets.as_mut_ptr()
                } else {
                    ptr::null_mut()
                },
                ..Default::default()
            };
            // In subframe mode a lock that does not wait returns the slices
            // written so far.
            lock.set_doNotWait(u32::from(!slot.event.is_invalid() || self.subframe));
            // SAFETY: `slot.output` is the pending slot's bitstream buffer on this session,
            // `sliceOffsets` is null or holds an entry per macroblock, and `lock` is versioned.
            let status =
                unsafe { (self.functions.nvEncLockBitstream.unwrap())(self.raw, &mut lock) };
            if status == _NVENCSTATUS_NV_ENC_ERR_LOCK_BUSY {
                if pending.started.elapsed() > COMPLETION_TIMEOUT {
                    bail!("NVENC output lock timed out");
                }
                break;
            }
            self.check(status, "lock completed output")?;
            let size = lock.bitstreamSizeInBytes as usize;
            let sent = pending.units.map_or(0, |units| units.sent);
            if self.subframe && self.subframe_traced < TRACED_SUBFRAMES {
                tracing::debug!(
                    frame = pending.frame,
                    status = lock.hwEncodeStatus,
                    slices = lock.numSlices,
                    bytes = size,
                    "NVENC subframe lock"
                );
            }
            if self.subframe && !subframe_complete(&lock) {
                // Hand on the slices written since the last poll, if the
                // frame leaves slice by slice and the driver's slice offsets
                // hold together; otherwise wait for all of it.
                let streamed = pending.streams && (pending.units.is_some() || !self.slices_joined);
                let consistent = slices_consistent(&lock, &self.slice_offsets);
                let unit = (|| -> Result<Option<Vec<u8>>> {
                    if size > MAX_FRAME_BYTES
                        || size < sent
                        || (size > 0 && lock.bitstreamBufferPtr.is_null())
                    {
                        bail!("invalid NVENC slice output size");
                    }
                    if !streamed || size == sent || !consistent {
                        return Ok(None);
                    }
                    // SAFETY: The lock succeeded and the pointer is non-null with `size` bounded
                    // bytes written; the bytes stay valid until the unlock below.
                    let written = unsafe {
                        std::slice::from_raw_parts(lock.bitstreamBufferPtr.cast::<u8>(), size)
                    };
                    Ok(Some(written[sent..].to_vec()))
                })();
                self.check(
                    // SAFETY: `output` was locked by the successful nvEncLockBitstream above.
                    unsafe {
                        (self.functions.nvEncUnlockBitstream.unwrap())(
                            self.raw,
                            self.slots[index].output,
                        )
                    },
                    "unlock output",
                )?;
                let unit = unit?;
                if streamed && size > sent && !consistent && !self.offsets_warned {
                    self.offsets_warned = true;
                    tracing::warn!(
                        slices = lock.numSlices,
                        bytes = size,
                        "NVENC reported slice offsets that do not fit the bytes written; this frame waits to finish before it leaves. Report this with the host log."
                    );
                }
                let pending = self.pending.front_mut().unwrap();
                if let Some(bytes) = unit {
                    let units = pending.units.get_or_insert_default();
                    let index = units.next;
                    units.next += 1;
                    units.sent = size;
                    self.ready.push_back(Encoded {
                        bytes,
                        unit: crate::encoder::Unit::Slice { index },
                        idr: false,
                        after_invalidation: false,
                        frame_cap_bytes: pending.frame_cap_bytes,
                        reencoded: false,
                        frame_cap_target: pending.frame_cap_target,
                        latency: Some(pending.started.elapsed()),
                        presentation: Some(pending.presentation),
                    });
                }
                if pending.started.elapsed() > COMPLETION_TIMEOUT {
                    bail!("NVENC completion timed out");
                }
                break;
            }
            let result = (|| -> Result<(Encoded, bool)> {
                if lock.outputTimeStamp != pending.frame
                    || lock.bitstreamBufferPtr.is_null()
                    || size == 0
                    || size > MAX_FRAME_BYTES
                    || size <= sent
                {
                    bail!("invalid NVENC output timestamp or size");
                }
                let idr = lock.pictureType == _NV_ENC_PIC_TYPE_NV_ENC_PIC_TYPE_IDR;
                // A frame that has begun to leave slice by slice ends with its
                // last unit; any other leaves whole.
                let unit = pending.units.map_or(crate::encoder::Unit::Whole, |units| {
                    crate::encoder::Unit::Last { index: units.next }
                });
                let whole = unit == crate::encoder::Unit::Whole;
                // SAFETY: The lock succeeded and the pointer is non-null with a bounded size;
                // the bytes stay valid until the unlock below.
                let written = unsafe {
                    std::slice::from_raw_parts(lock.bitstreamBufferPtr.cast::<u8>(), size)
                };
                Ok((
                    Encoded {
                        bytes: written[sent..].to_vec(),
                        unit,
                        idr: whole && idr,
                        after_invalidation: whole && pending.after_invalidation && !idr,
                        frame_cap_bytes: pending.frame_cap_bytes,
                        reencoded: whole && pending.reencoded,
                        frame_cap_target: pending.frame_cap_target,
                        latency: Some(pending.started.elapsed()),
                        presentation: Some(pending.presentation),
                    },
                    idr,
                ))
            })();
            self.check(
                // SAFETY: `output` was locked by the successful nvEncLockBitstream above.
                unsafe {
                    (self.functions.nvEncUnlockBitstream.unwrap())(
                        self.raw,
                        self.slots[index].output,
                    )
                },
                "unlock output",
            )?;
            let (encoded, idr) = result?;
            if self.subframe {
                self.subframe_traced = self.subframe_traced.saturating_add(1);
            }
            if idr && encoded.unit != crate::encoder::Unit::Whole && !self.unit_idr_reported {
                // Only P-frames are sent slice by slice, and NVENC makes a
                // keyframe only when asked: say so, once, if one was.
                self.unit_idr_reported = true;
                tracing::error!(
                    frame = pending.frame,
                    "NVENC returned a keyframe for a frame sent slice by slice as a P-frame. Report this with the host log."
                );
            }
            self.check(
                // SAFETY: `mapped` is the input mapped for this frame in `submit`, and its output
                // has been collected.
                unsafe {
                    (self.functions.nvEncUnmapInputResource.unwrap())(
                        self.raw,
                        self.slots[index].mapped,
                    )
                },
                "release completed input",
            )?;
            self.slots[index].mapped = ptr::null_mut();
            let pending = self.pending.pop_front().unwrap();
            if self.reencodes(&pending, &encoded) {
                match self.reencode(&pending) {
                    // The loop collects the keyframe when it is done.
                    Ok(()) => continue,
                    Err(error) => {
                        let error = format!("{error:#}");
                        tracing::error!(%error, frame = pending.frame, bytes = encoded.bytes.len(), "NVENC could not encode an oversize scene cut again as a keyframe; sending the inter frame, which loses its FEC. Report this with the host log.");
                        self.reencode_failure = Some(error);
                    }
                }
            }
            if idr && crate::amf::over_cap_target(size as u64, pending.frame_cap_target) {
                self.keyframes_over_cap += 1;
            }
            if let Some(cap) = pending.frame_cap_bytes {
                if size as u64 > cap.saturating_mul(2)
                    && self
                        .oversize_warned
                        .is_none_or(|at| at.elapsed() >= OVERSIZE_WARNING_INTERVAL)
                {
                    self.oversize_warned = Some(Instant::now());
                    tracing::warn!(
                        bytes = size,
                        cap,
                        picture = if idr { "keyframe" } else { "inter" },
                        "NVENC wrote a frame over twice its frame cap; the one-frame VBV targets the cap but the driver does not guarantee it"
                    );
                }
            }
            self.references.completed(pending.frame, idr);
            self.ready.push_back(encoded);
        }
        Ok(())
    }
    fn drain(&mut self) -> Result<()> {
        let deadline = Instant::now() + COMPLETION_TIMEOUT;
        while !self.pending.is_empty() {
            self.collect()?;
            if self.pending.is_empty() {
                break;
            }
            if Instant::now() >= deadline {
                bail!("NVENC drain timed out");
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        Ok(())
    }
    fn clear_inputs(&mut self) -> Result<()> {
        self.drain()?;
        let _guard = self.guard()?;
        for slot in &mut self.slots {
            cleanup_slot(&self.functions, self.raw, self.api, slot)?;
        }
        self.slots.clear();
        Ok(())
    }
    fn bitrate(&mut self, bitrate: u32) -> Result<()> {
        if bitrate == self.stream.bitrate_kbps {
            return Ok(());
        }
        if bitrate == 0 || bitrate > 800_000 {
            bail!("NVENC bitrate out of range");
        }
        if !self.dynamic_bitrate {
            bail!("NVENC driver cannot change bitrate in this session");
        }
        let before = self.config.rcParams.averageBitRate;
        let after = bitrate * 1000;
        let scale = |bits: u32| {
            if bits > 0 && before > 0 {
                (u64::from(after) * u64::from(bits) / u64::from(before))
                    .clamp(MIN_VBV_BITS, u64::from(u32::MAX)) as u32
            } else {
                bits
            }
        };
        let vbv = scale(self.config.rcParams.vbvBufferSize);
        let ceiling = scale(self.vbv_ceiling);
        self.reconfigure(after, vbv, "change bitrate")?;
        self.vbv_ceiling = ceiling;
        // The next frame cap is measured against the new ceiling.
        self.cap_requested = None;
        self.stream.bitrate_kbps = bitrate;
        Ok(())
    }
    /// Reconfigure the rate control to `bitrate` bits per second and a VBV of
    /// `vbv` bits. A higher bitrate resets the encoder and forces a keyframe,
    /// after the frames in flight are collected. Any other change applies
    /// with frames in flight, as FFmpeg reconfigures, so a frame cap or a
    /// lower bitrate adds no wait.
    fn reconfigure(&mut self, bitrate: u32, vbv: u32, operation: &'static str) -> Result<()> {
        let before = self.config.rcParams.averageBitRate;
        if bitrate > before {
            self.drain()?;
        }
        let _guard = self.guard()?;
        let mut config = Box::new(*self.config);
        config.rcParams.averageBitRate = bitrate;
        config.rcParams.maxBitRate = self.rate_control.max_bitrate(bitrate);
        config.rcParams.vbvBufferSize = vbv;
        let mut reconfigure = NV_ENC_RECONFIGURE_PARAMS {
            version: self.api.reconfigure(),
            reInitEncodeParams: *self.initialize,
            ..Default::default()
        };
        reconfigure.reInitEncodeParams.encodeConfig = config.as_mut();
        reconfigure.set_resetEncoder(u32::from(bitrate > before));
        reconfigure.set_forceIDR(u32::from(bitrate > before));
        self.check(
            // SAFETY: `dynamic_bitrate` requires nvEncReconfigureEncoder, and the boxed `config` is
            // kept as `self.config` after the call.
            unsafe {
                (self.functions.nvEncReconfigureEncoder.unwrap())(self.raw, &mut reconfigure)
            },
            operation,
        )?;
        self.config = config;
        self.initialize.encodeConfig = self.config.as_mut();
        self.force_idr |= bitrate > before;
        Ok(())
    }
    fn invalidate(&mut self, first: u64, last: u64) -> Result<bool> {
        self.drain()?;
        let _guard = self.guard()?;
        match self
            .references
            .plan(first, last, self.retained, self.invalidation)
        {
            Recovery::AlreadyApplied => Ok(true),
            Recovery::Idr => Ok(false),
            Recovery::Invalidate { first, last } => {
                for frame in first..=last {
                    self.check(
                        // SAFETY: `plan` returns Invalidate only when `self.invalidation`, which
                        // requires this entry, is set.
                        unsafe {
                            (self.functions.nvEncInvalidateRefFrames.unwrap())(self.raw, frame)
                        },
                        "invalidate lost reference",
                    )?;
                }
                self.references.applied(first, last);
                Ok(true)
            }
        }
    }
    fn poll(&mut self) -> Result<Vec<Encoded>> {
        self.collect()?;
        Ok(self.ready.drain(..).collect())
    }
}
fn cleanup_slot(
    functions: &NV_ENCODE_API_FUNCTION_LIST,
    raw: *mut c_void,
    api: ApiVersion,
    slot: &mut Slot,
) -> Result<()> {
    let check = |status, operation| check_raw(status, operation, functions, raw);
    // SAFETY: `raw` is the session these resources belong to; each is released only while still
    // held and then cleared, and the unregister entry existed when `event_registered` was set.
    unsafe {
        if !slot.mapped.is_null() {
            check(
                (functions.nvEncUnmapInputResource.unwrap())(raw, slot.mapped),
                "unmap input during cleanup",
            )?;
            slot.mapped = ptr::null_mut();
        }
        if slot.event_registered {
            let mut event = NV_ENC_EVENT_PARAMS {
                version: api.event(),
                completionEvent: slot.event.0,
                ..Default::default()
            };
            check(
                (functions.nvEncUnregisterAsyncEvent.unwrap())(raw, &mut event),
                "unregister completion event",
            )?;
            slot.event_registered = false;
        }
        if !slot.output.is_null() {
            check(
                (functions.nvEncDestroyBitstreamBuffer.unwrap())(raw, slot.output),
                "destroy output buffer",
            )?;
            slot.output = ptr::null_mut();
        }
        if !slot.registered.is_null() {
            check(
                (functions.nvEncUnregisterResource.unwrap())(raw, slot.registered),
                "unregister GPU input",
            )?;
            slot.registered = ptr::null_mut();
        }
    }
    Ok(())
}
impl Drop for Session {
    fn drop(&mut self) {
        if self.raw.is_null() {
            return;
        }
        let _guard = match self.guard() {
            Ok(guard) => guard,
            Err(error) => {
                tracing::warn!(%error, "CUDA device lost during NVENC teardown; attempting driver cleanup");
                None
            }
        };
        if self.initialized && !self.pending.is_empty() {
            let mut eos = NV_ENC_PIC_PARAMS {
                version: self.api.picture(),
                encodePicFlags: _NV_ENC_PIC_FLAGS_NV_ENC_PIC_FLAG_EOS,
                ..Default::default()
            };
            // SAFETY: `self.raw` is the non-null open session and `eos` is versioned.
            let _ = unsafe { (self.functions.nvEncEncodePicture.unwrap())(self.raw, &mut eos) };
            let _ = self.drain();
        }
        for (index, slot) in self.slots.iter_mut().enumerate() {
            // A failed drain does not establish completion. Let session
            // destruction stop the native worker before releasing its input,
            // output buffer or completion event.
            if self.pending.iter().any(|pending| pending.slot == index) {
                continue;
            }
            if let Err(error) = cleanup_slot(&self.functions, self.raw, self.api, slot) {
                tracing::warn!(%error, "NVENC input cleanup deferred to session destruction");
            }
        }
        // SAFETY: `self.raw` is non-null (checked at the top) and is destroyed once, since it is
        // cleared below.
        let status = unsafe { (self.functions.nvEncDestroyEncoder.unwrap())(self.raw) };
        if let Err(error) = self.check(status, "destroy session") {
            // A failed native destroy leaves its async workers' lifetime
            // unknown. Preserve their input/context/driver owners until process
            // exit instead of freeing resources behind a still-running codec.
            tracing::error!(%error, "NVENC cleanup failed; driver resources retained until host exit");
            std::mem::forget(std::mem::take(&mut self.slots));
            std::mem::forget(std::mem::take(&mut self.pending));
            std::mem::forget(self._runtime.clone());
            if let Some(context) = self.cuda.take() {
                std::mem::forget(context);
            }
        }
        self.raw = ptr::null_mut();
        // Drop CUDA allocations/textures only after the driver destroys the
        // session, including inputs whose explicit unregister was rejected.
        self.slots.clear();
        self.pending.clear();
    }
}

/// What the session codes for `config`: a packed 4:4:4 stream is a larger
/// full-range BT.709 SDR 4:2:0 picture (`Negotiated::coded_size`), as AMF
/// codes it; the device unpacks it and takes HDR from the control channel.
fn coded(config: &Negotiated) -> Negotiated {
    let mut coded = config.clone();
    if config.chroma_packing != 0 {
        (coded.width, coded.height) = config.coded_size();
        coded.sdr_10bit = config.ten_bit();
        coded.hdr = false;
        coded.csc_mode = 3;
        coded.chroma_packing = 0;
    }
    coded
}
/// Whether a subframe lock holds the whole picture: `hwEncodeStatus` 2, the
/// completion NVIDIA's developer forum gives for subframe readback. The slice
/// count alone is not enough, since a driver may report every slice of the
/// mode (`sliceMode` 3) before the last is written. A frame that never
/// completes fails loudly at the completion timeout rather than leave cut
/// short.
fn subframe_complete(lock: &NV_ENC_LOCK_BITSTREAM) -> bool {
    lock.hwEncodeStatus == 2
}
/// Whether an unfinished subframe lock's slice offsets fit the bytes
/// written: at least one slice, offsets rising strictly and the last slice
/// starting inside the written size.
fn slices_consistent(lock: &NV_ENC_LOCK_BITSTREAM, offsets: &[u32]) -> bool {
    let count = lock.numSlices as usize;
    count > 0
        && count <= offsets.len()
        && offsets[..count].windows(2).all(|pair| pair[0] < pair[1])
        && offsets[count - 1] < lock.bitstreamSizeInBytes
}
/// Dynamic HDR metadata spliced into each picture or unit
/// ([`crate::amf::dynamic::output`]): before the first slice, the Dolby
/// Vision RPU after the last.
fn splice(dynamic: Option<&mut crate::amf::dynamic::Dynamic>, frame: &mut Encoded) {
    let (spliced, cap) = crate::amf::dynamic::output(
        dynamic,
        frame.unit,
        &frame.bytes,
        frame.frame_cap_bytes,
        frame.idr,
    );
    if let Some(bytes) = spliced {
        frame.bytes = bytes;
    }
    frame.frame_cap_bytes = cap;
}

pub struct Encoder {
    session: Session,
    gpu: Device,
    /// The stream as negotiated; the session codes [`coded`].
    config: Negotiated,
    converter: Option<crate::gpu_color::Converter>,
    source: Option<(u32, u32, Pixel)>,
    luminance: [f32; 2],
    /// The couch menu's panel, drawn into GPU frames by the conversion.
    pub overlay: Option<crate::overlay::Overlay>,
    /// The session's reporter, shared with the stream.
    pub(crate) warnings: Arc<Warnings>,
    dynamic: Option<Box<crate::amf::dynamic::Dynamic>>,
    /// The D3D11 conversion's statistics pass behind HDR10+, HDR Vivid and
    /// measured Dolby Vision light levels.
    stats: Option<Box<crate::gpu_color::HdrStats>>,
    last_submitted: Option<Instant>,
    /// The frame cap failed and its warning is up.
    cap_warned: bool,
}
impl Encoder {
    pub fn new_device_options(config: &Negotiated, gpu: Device, options: &Config) -> Result<Self> {
        Self::new_device_reported(config, gpu, options, &Arc::default())
    }
    pub fn new_device_reported(
        config: &Negotiated,
        gpu: Device,
        options: &Config,
        warnings: &Arc<Warnings>,
    ) -> Result<Self> {
        if config.chroma_packing != 0 {
            config.validate()?;
        }
        let coded = coded(config);
        let runtime = Runtime::load()?;
        let tuning = Tuning::new(options, &coded)?;
        let cuda = if coded.yuv444 && coded.ten_bit() {
            Some(cuda::Context::new(&gpu)?)
        } else {
            None
        };
        let (device, kind) = cuda
            .as_ref()
            .map(|context| (context.raw(), _NV_ENC_DEVICE_TYPE_NV_ENC_DEVICE_TYPE_CUDA))
            .unwrap_or((
                gpu.device.as_raw(),
                _NV_ENC_DEVICE_TYPE_NV_ENC_DEVICE_TYPE_DIRECTX,
            ));
        let session = Session::open(runtime, device, kind, cuda, &coded, &tuning)?;
        let metadata = gpu.hdr_metadata();
        let mut encoder = Self {
            session,
            gpu,
            config: config.clone(),
            converter: None,
            source: None,
            luminance: [100., 1.],
            overlay: None,
            warnings: warnings.clone(),
            dynamic: None,
            stats: None,
            last_submitted: None,
            cap_warned: false,
        };
        encoder.dynamic = encoder.enable_dynamic(options);
        encoder.set_hdr_metadata(metadata);
        Ok(encoder)
    }
    /// HDR10+ and HDR Vivid for an HDR stream with `hdr_dynamic_metadata`
    /// on, and Dolby Vision RPUs for a stream that picked it, as AMF sends
    /// them ([`crate::amf::dynamic::start`]). Asks with the negotiated
    /// stream, not the coded one: a packed 4:4:4 stream codes SDR.
    fn enable_dynamic(&mut self, options: &Config) -> Option<Box<crate::amf::dynamic::Dynamic>> {
        let wanted = crate::amf::dynamic::wanted(&self.config, options)?;
        let packed = self.config.chroma_packing != 0;
        if !packed {
            self.stats = Some(Box::new(crate::gpu_color::HdrStats::new(&self.gpu)));
        }
        crate::amf::dynamic::start(wanted, packed, self.stats.as_deref_mut(), &self.warnings)
    }
    pub fn set_luminance(&mut self, luminance: [f32; 2]) {
        self.luminance = luminance;
    }
    /// HDR10 static metadata for the bitstream (API 13 and newer) and as the
    /// stand-in of dynamic metadata.
    pub fn set_hdr_metadata(&mut self, metadata: butterpollo_core::hdr::Metadata) {
        if let Some(dynamic) = self.dynamic.as_deref_mut() {
            dynamic.set_metadata(metadata);
        }
        self.session.set_hdr_metadata(metadata);
    }
    /// The receiver's display peak, for dynamic metadata that names one.
    pub fn set_hdr_target(&mut self, nits: u16) {
        if let Some(dynamic) = self.dynamic.as_deref_mut() {
            dynamic.set_target(nits);
        }
    }
    pub fn set_next_frame(&mut self, frame: u64) {
        debug_assert!(self.session.pending.is_empty() && self.session.ready.is_empty());
        self.session.next = frame.max(1);
    }
    pub fn supports_invalidation(&self) -> bool {
        self.session.invalidation
    }
    pub fn invalidate_ref_frames(&mut self, first: u64, last: u64) -> bool {
        match self.session.invalidate(first, last) {
            Ok(applied) => applied,
            Err(error) => {
                tracing::warn!(%error, "NVENC reference recovery failed; requesting a larger IDR recovery frame. Reduce bitrate on lossy links or update the NVIDIA driver");
                self.session.force_idr = true;
                false
            }
        }
    }
    pub fn pending(&self) -> bool {
        !self.session.pending.is_empty() || !self.session.ready.is_empty()
    }
    /// Frames submitted and not yet encoded.
    pub fn backlog(&self) -> usize {
        self.session.pending.len()
    }
    /// When NVENC accepted the last submitted picture.
    pub fn last_submitted(&self) -> Option<Instant> {
        self.last_submitted
    }
    /// The GPU this encoder opened on.
    pub fn adapter(&self) -> &str {
        &self.gpu.display.adapter
    }
    pub fn device_removed(&self) -> Option<crate::device_loss::DeviceLost> {
        crate::device_loss::DeviceLost::d3d11(&self.gpu.device)
    }
    /// AMF's throughput names answered from `NV_ENC_CAPS_MB_PER_SEC_MAX`, in
    /// 16x16 macroblocks per second, for the session's own codec.
    pub fn capability(&self, name: &str) -> Option<i64> {
        if throughput_codec(name)? != self.session.stream.codec {
            return None;
        }
        let _guard = self.session.guard().ok()?;
        self.session
            .cap(_NV_ENC_CAPS_NV_ENC_CAPS_MB_PER_SEC_MAX)
            .ok()
            .filter(|rate| *rate > 0)
            .map(i64::from)
    }
    /// The frame budget guard's verdict on this session's preset.
    pub fn budget(&self) -> Option<Budget> {
        self.session.budget
    }
    /// The host's pace-based frame cap ([`Session::set_frame_cap`]); the
    /// bound frames are held to, or `None` when the driver controls buffering.
    pub fn set_frame_cap(&mut self, bytes: u64) -> Option<u64> {
        match self.session.set_frame_cap(bytes) {
            Ok(cap) => {
                if std::mem::take(&mut self.cap_warned) {
                    self.warnings.clear("encoder_frame_size");
                }
                cap
            }
            Err(error) => {
                self.cap_warned = true;
                tracing::warn!(error = %format!("{error:#}"), bytes, "NVENC could not hold frames to the frame cap; keeping its one-frame VBV");
                self.warnings.set("encoder_frame_size", format!("NVENC could not lower its frame size limit to {bytes} bytes ({error:#}); large frames may leave without full FEC. Update the NVIDIA driver."));
                self.session.frame_cap()
            }
        }
    }
    pub fn frame_cap(&self) -> Option<u64> {
        self.session.frame_cap()
    }
    /// The largest inter frame sent as encoded; a larger one is encoded again
    /// as a keyframe, as AMF does.
    pub fn set_cut_limit(&mut self, bytes: u64) {
        self.session.cut_limit = (bytes > 0).then_some(bytes);
    }
    /// Inter frames encoded again as keyframes since the encoder opened.
    pub fn reencoded_cuts(&self) -> u64 {
        self.session.reencoded_cuts
    }
    /// Keyframes more than 1.1 times the cap the host asked for.
    pub fn keyframes_over_cap(&self) -> u64 {
        self.session.keyframes_over_cap
    }
    /// Blocks a P-frame is sent in as NVENC writes its slices; 0 when
    /// `video_slice_delivery` is off, the driver has no subframe readback,
    /// and after [`Self::join_slices`].
    pub fn slice_delivery(&self) -> u8 {
        if self.session.subframe && !self.session.slices_joined {
            self.session.slice_blocks
        } else {
            0
        }
    }
    /// Return every later frame whole, from the next one on. A frame whose
    /// first unit has left is finished as it began.
    pub fn join_slices(&mut self) {
        self.session.slices_joined = true;
    }
    /// Snapshot only at failure/recreation, before dropping the driver state.
    pub fn log_stall(&self) {
        let oldest = self.session.pending.front();
        // SAFETY: the D3D11 device is owned by this encoder and outlives the
        // call; GetDeviceRemovedReason only reads its state.
        let removed = unsafe { self.gpu.device.GetDeviceRemovedReason() };
        tracing::warn!(
            in_flight = self.session.pending.len(),
            ready = self.session.ready.len(),
            oldest_frame = ?oldest.map(|pending| pending.frame),
            oldest_ms = ?oldest.map(|pending| pending.started.elapsed().as_millis()),
            next_frame = self.session.next,
            asynchronous = self.session.initialize.enableEncodeAsync != 0,
            last_submit = ?self.last_submitted.map(|at| at.elapsed().as_millis()),
            d3d11_removed = ?removed,
            "NVENC stall snapshot"
        );
    }
    pub fn poll(&mut self) -> Result<Vec<Encoded>> {
        let mut frames = self.session.poll()?;
        if let Some(error) = self.session.reencode_failure.take() {
            self.warnings.event("encoder_reencode", format!("NVENC could not encode an oversize scene cut again as a keyframe ({error}); the frame goes without full FEC and may freeze the picture briefly. Report this with the host log."), butterpollo_core::session::EVENT_PERIOD);
        }
        for frame in &mut frames {
            splice(self.dynamic.as_deref_mut(), frame);
        }
        Ok(frames)
    }
    pub fn accepts_gpu_device(&self, image: &GpuImage) -> bool {
        self.gpu.device.as_raw() == image.gpu.device.as_raw()
    }
    /// A CPU image, such as the codec probe's, goes through the same GPU
    /// conversion as a capture.
    pub fn encode(&mut self, image: &Image, idr: bool, bitrate: u32) -> Result<Vec<Encoded>> {
        anyhow::ensure!(
            self.overlay.is_none(),
            "couch menu: needs wgc_direct_encoder_input"
        );
        let gpu = GpuImage::upload(&self.gpu, image)?;
        self.encode_gpu(&gpu, idr, bitrate)
    }
    pub fn encode_gpu(
        &mut self,
        image: &GpuImage,
        idr: bool,
        bitrate: u32,
    ) -> Result<Vec<Encoded>> {
        if !self.accepts_gpu_device(image) {
            bail!("NVENC capture device changed; recreate the encoder");
        }
        self.session.collect()?;
        self.session.bitrate(bitrate)?;
        if self.session.pending.len() >= MAX_PENDING {
            self.session.drain()?;
        }
        let source = (image.width, image.height, image.pixel);
        if self.source != Some(source) {
            self.session.clear_inputs()?;
            self.converter = Some(crate::gpu_color::Converter::new(
                &self.gpu,
                &self.config,
                source,
            )?);
            self.source = Some(source);
        }
        let converter = self.converter.as_mut().unwrap();
        converter.set_luminance(self.luminance);
        converter.set_overlay(self.overlay.as_ref());
        let converted = converter.convert(image)?;
        // Submitted with `dynamic::sample`, once the frame went to NVENC.
        if let (Some(stats), Some(converter)) = (self.stats.as_deref_mut(), &self.converter) {
            stats.queue(image, converter);
        }
        let slot = self.session.texture_slot(converted.as_ref())?;
        self.session
            .submit(slot, Some(converted), idr, image.captured)?;
        self.last_submitted = Some(Instant::now());
        // Before the poll, so the frame it returns carries the newest
        // finished sample.
        crate::amf::dynamic::sample(&mut self.dynamic, self.stats.as_deref_mut(), &self.warnings);
        self.poll()
    }
}
#[cfg(test)]
mod tests;
