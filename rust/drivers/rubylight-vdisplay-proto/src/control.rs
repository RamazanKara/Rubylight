//! The control plane: `DeviceIoControl` codes on the driver's device interface and the
//! fixed-layout structs each one carries.
//!
//! Every struct is little-endian with no padding and encodes by hand: [`AddRequest::encode`]
//! writes exactly [`AddRequest::SIZE`] bytes and `decode` takes exactly that many. Host and
//! driver ship together and check [`crate::PROTOCOL_VERSION`] over [`IOCTL_GET_INFO`] first,
//! so there is no prefix-compatible growth: a request of the wrong size is refused.
//!
//! Every verb is scoped to the calling process, as in punktfunk's protocol v8: a monitor
//! answers only the process whose [`IOCTL_ADD`] created it, and departs when that process
//! closes its last handle to the device or sends nothing for the watchdog window.

use crate::{modes, ring};

/// `FILE_DEVICE_UNKNOWN`.
pub const DEVICE_TYPE: u32 = 0x22;

/// `CTL_CODE(FILE_DEVICE_UNKNOWN, function, METHOD_BUFFERED, FILE_ANY_ACCESS)`. The device's
/// DACL already limits who may open it to SYSTEM and administrators.
#[must_use]
pub const fn ctl_code(function: u32) -> u32 {
    (DEVICE_TYPE << 16) | (function << 2)
}

/// The function number inside an IOCTL code.
#[must_use]
pub const fn function_of(code: u32) -> u32 {
    (code >> 2) & 0xFFF
}

// Rubylight's own function space at 0xB00, apart from libvirtualdisplay's and punktfunk's,
// which both start at 0x900.
/// Version handshake. No input; output [`InfoReply`].
pub const IOCTL_GET_INFO: u32 = ctl_code(0xB00);
/// Create and plug in a monitor. Input [`AddRequest`], output [`AddReply`].
pub const IOCTL_ADD: u32 = ctl_code(0xB01);
/// Unplug a monitor. Input [`SessionRequest`]. `STATUS_NOT_FOUND` when it is not there.
pub const IOCTL_REMOVE: u32 = ctl_code(0xB02);
/// Lead a live monitor's mode list with a new mode, in place. Input [`UpdateModesRequest`].
pub const IOCTL_UPDATE_MODES: u32 = ctl_code(0xB03);
/// Keep the caller's monitors alive. Input [`SessionRequest`]: a non-zero session also
/// answers `STATUS_NOT_FOUND` when that monitor is gone, which is the lease check the host's
/// heartbeat relies on. Session `0` only proves the caller alive.
pub const IOCTL_HEARTBEAT: u32 = ctl_code(0xB04);
/// Pin the GPU that renders every monitor. Input [`RenderAdapterRequest`].
pub const IOCTL_SET_RENDER_ADAPTER: u32 = ctl_code(0xB05);
/// Hand a monitor its frame ring. Input [`SetFrameRingRequest`]. On success the driver owns
/// every handle in it; on failure it owns none and the host closes them remotely.
pub const IOCTL_SET_FRAME_RING: u32 = ctl_code(0xB06);
/// Drop a monitor's frame ring; the driver closes its handles. Input [`SessionRequest`].
pub const IOCTL_CLEAR_FRAME_RING: u32 = ctl_code(0xB07);
/// A monitor's state. Input [`SessionRequest`], output [`MonitorStatus`].
pub const IOCTL_QUERY_MONITOR: u32 = ctl_code(0xB08);
/// Unplug every monitor of the caller. No input.
pub const IOCTL_CLEAR_ALL: u32 = ctl_code(0xB09);
/// Take the driver's pending log lines as [`log_lines`] records. No input.
pub const IOCTL_DRAIN_LOG: u32 = ctl_code(0xB0A);
/// Drop a monitor's frame ring only while it is still the ring of the given generation, so a
/// capture closing its ring never clears the one that replaced it. Input
/// [`ClearFrameRingRequest`]; `STATUS_NOT_FOUND` when that ring is not the monitor's. Present
/// when the driver reports [`CAP_SCOPED_CLEAR`].
pub const IOCTL_CLEAR_FRAME_RING_GENERATION: u32 = ctl_code(0xB0B);
/// Hand a monitor a fence ring ([`ring::Kind::Fence`]). Input [`SetFenceRingRequest`]; owns
/// its handles as [`IOCTL_SET_FRAME_RING`] does. Present when the driver reports
/// [`CAP_FENCE_RING`].
pub const IOCTL_SET_FENCE_RING: u32 = ctl_code(0xB0C);

/// Every IOCTL this protocol defines, for the uniqueness test and the driver's dispatch.
pub const ALL_IOCTLS: [u32; 13] = [
    IOCTL_GET_INFO,
    IOCTL_ADD,
    IOCTL_REMOVE,
    IOCTL_UPDATE_MODES,
    IOCTL_HEARTBEAT,
    IOCTL_SET_RENDER_ADAPTER,
    IOCTL_SET_FRAME_RING,
    IOCTL_CLEAR_FRAME_RING,
    IOCTL_QUERY_MONITOR,
    IOCTL_CLEAR_ALL,
    IOCTL_DRAIN_LOG,
    IOCTL_CLEAR_FRAME_RING_GENERATION,
    IOCTL_SET_FENCE_RING,
];

/// NTSTATUS values the driver completes requests with, and the Win32 error each becomes in
/// the host's `DeviceIoControl`.
pub mod status {
    pub const SUCCESS: i32 = 0;
    pub const INVALID_PARAMETER: i32 = 0xC000_000Du32 as i32;
    pub const NOT_FOUND: i32 = 0xC000_0225u32 as i32;
    pub const NOT_SUPPORTED: i32 = 0xC000_00BBu32 as i32;
    pub const BUFFER_TOO_SMALL: i32 = 0xC000_0023u32 as i32;
    pub const ACCESS_DENIED: i32 = 0xC000_0022u32 as i32;
    pub const INSUFFICIENT_RESOURCES: i32 = 0xC000_009Au32 as i32;
    /// A warning status, so `DeviceIoControl` still fails, with `ERROR_BUSY`.
    pub const DEVICE_BUSY: i32 = 0x8000_0011u32 as i32;

    /// `ERROR_NOT_FOUND`: the monitor or lease is gone.
    pub const WIN32_NOT_FOUND: u32 = 1168;
    /// `ERROR_BUSY`: retry; another process still holds the requested monitor id.
    pub const WIN32_BUSY: u32 = 170;
    /// `ERROR_NOT_SUPPORTED`: the driver lacks the verb.
    pub const WIN32_NOT_SUPPORTED: u32 = 50;
}

/// Why bytes are not a request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WireError {
    /// The buffer is not exactly the struct's size.
    Size { expected: usize, got: usize },
    /// A field is out of range; the text names it.
    Invalid(&'static str),
}

impl core::fmt::Display for WireError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Size { expected, got } => write!(f, "expected {expected} bytes, got {got}"),
            Self::Invalid(field) => write!(f, "invalid {field}"),
        }
    }
}

/// Fixed-size little-endian writer: each struct's `encode` writes its fields in order and
/// `finish` checks the count, so a field added without growing `SIZE` fails at once.
struct Writer<const N: usize> {
    buf: [u8; N],
    at: usize,
}

impl<const N: usize> Writer<N> {
    fn new() -> Self {
        Self { buf: [0; N], at: 0 }
    }
    fn bytes(mut self, v: &[u8]) -> Self {
        self.buf[self.at..self.at + v.len()].copy_from_slice(v);
        self.at += v.len();
        self
    }
    fn u32(self, v: u32) -> Self {
        self.bytes(&v.to_le_bytes())
    }
    fn i32(self, v: i32) -> Self {
        self.bytes(&v.to_le_bytes())
    }
    fn u64(self, v: u64) -> Self {
        self.bytes(&v.to_le_bytes())
    }
    fn finish(self) -> [u8; N] {
        assert_eq!(self.at, N, "wire struct wrote the wrong number of bytes");
        self.buf
    }
}

/// The reading half of [`Writer`]. Construction checks the exact size, so the field reads
/// cannot run past the end.
struct Reader<'a> {
    buf: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn new(buf: &'a [u8], expected: usize) -> Result<Self, WireError> {
        if buf.len() != expected {
            return Err(WireError::Size {
                expected,
                got: buf.len(),
            });
        }
        Ok(Self { buf, at: 0 })
    }
    fn take<const N: usize>(&mut self) -> [u8; N] {
        let mut out = [0; N];
        out.copy_from_slice(&self.buf[self.at..self.at + N]);
        self.at += N;
        out
    }
    fn u32(&mut self) -> u32 {
        u32::from_le_bytes(self.take())
    }
    fn i32(&mut self) -> i32 {
        i32::from_le_bytes(self.take())
    }
    fn u64(&mut self) -> u64 {
        u64::from_le_bytes(self.take())
    }
}

/// [`InfoReply::magic`]: `"RLVD"`.
pub const INFO_MAGIC: u32 = u32::from_le_bytes(*b"RLVD");
/// [`InfoReply::caps`]: frames are pushed through [`IOCTL_SET_FRAME_RING`].
pub const CAP_FRAME_RING: u32 = 1 << 0;
/// [`InfoReply::caps`]: monitors advertise 10-bit wire formats, so Windows offers HDR, and
/// FP16 desktops reach the ring as `R16G16B16A16_FLOAT`.
pub const CAP_HDR: u32 = 1 << 1;
/// [`InfoReply::caps`]: [`IOCTL_UPDATE_MODES`] changes a live monitor's modes in place.
pub const CAP_UPDATE_MODES: u32 = 1 << 2;
/// [`InfoReply::caps`]: [`IOCTL_CLEAR_FRAME_RING_GENERATION`] clears only the ring it names.
pub const CAP_SCOPED_CLEAR: u32 = 1 << 3;
/// [`InfoReply::caps`]: [`IOCTL_SET_FENCE_RING`] takes a fence ring, whose slots the header's
/// state words and two shared fences own instead of keyed mutexes. A host keeps sending
/// keyed-mutex rings to a driver without it, and a driver with it still takes them from a
/// host that sends one.
pub const CAP_FENCE_RING: u32 = 1 << 4;

/// [`IOCTL_GET_INFO`] reply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InfoReply {
    pub magic: u32,
    pub protocol_version: u32,
    /// A caller that sends no IOCTL for this long loses its monitors.
    pub watchdog_timeout_ms: u32,
    pub max_monitors: u32,
    pub max_ring_slots: u32,
    pub caps: u32,
    /// `major << 16 | minor << 8 | patch` of the driver build.
    pub driver_version: u32,
}

impl InfoReply {
    pub const SIZE: usize = 32;

    #[must_use]
    pub fn encode(&self) -> [u8; Self::SIZE] {
        Writer::new()
            .u32(self.magic)
            .u32(self.protocol_version)
            .u32(self.watchdog_timeout_ms)
            .u32(self.max_monitors)
            .u32(self.max_ring_slots)
            .u32(self.caps)
            .u32(self.driver_version)
            .u32(0)
            .finish()
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, WireError> {
        let mut r = Reader::new(bytes, Self::SIZE)?;
        let reply = Self {
            magic: r.u32(),
            protocol_version: r.u32(),
            watchdog_timeout_ms: r.u32(),
            max_monitors: r.u32(),
            max_ring_slots: r.u32(),
            caps: r.u32(),
            driver_version: r.u32(),
        };
        if reply.magic != INFO_MAGIC {
            return Err(WireError::Invalid("driver magic"));
        }
        Ok(reply)
    }

    /// Whether a host speaking [`crate::PROTOCOL_VERSION`] may drive this driver.
    #[must_use]
    pub fn compatible(&self) -> bool {
        self.protocol_version == crate::PROTOCOL_VERSION
    }
}

/// [`AddRequest::flags`]: fail with `STATUS_DEVICE_BUSY` instead of taking another id when
/// the preferred monitor id is still held by another process. A host restarting over its
/// predecessor's monitor retries until that one is reaped, so Windows sees the same monitor
/// identity (and keeps its per-monitor settings) across the handover.
pub const ADD_FLAG_EXACT_ID: u32 = 1 << 0;
const ADD_FLAGS: u32 = ADD_FLAG_EXACT_ID;

/// [`IOCTL_ADD`] input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AddRequest {
    /// The caller's key for this monitor; never `0`. A second ADD with a live key replaces
    /// that monitor.
    pub session_id: u64,
    pub width: u32,
    pub height: u32,
    /// Refresh in millihertz, so 59.94 Hz survives.
    pub refresh_mhz: u32,
    /// `1..=15` names the monitor (EDID serial, connector index, container id) so Windows
    /// keeps its settings across reconnects; `0` takes the lowest free id.
    pub preferred_monitor_id: u32,
    /// `ADD_FLAG_*`; unknown bits are refused.
    pub flags: u32,
    /// Client peak luminance for the EDID HDR block; `0` = unknown, keep the defaults.
    pub max_luminance_nits: u32,
    pub max_frame_avg_nits: u32,
    pub min_luminance_millinits: u32,
}

impl AddRequest {
    pub const SIZE: usize = 48;

    #[must_use]
    pub fn encode(&self) -> [u8; Self::SIZE] {
        Writer::new()
            .u64(self.session_id)
            .u32(self.width)
            .u32(self.height)
            .u32(self.refresh_mhz)
            .u32(self.preferred_monitor_id)
            .u32(self.flags)
            .u32(self.max_luminance_nits)
            .u32(self.max_frame_avg_nits)
            .u32(self.min_luminance_millinits)
            .u64(0)
            .finish()
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, WireError> {
        let mut r = Reader::new(bytes, Self::SIZE)?;
        let request = Self {
            session_id: r.u64(),
            width: r.u32(),
            height: r.u32(),
            refresh_mhz: r.u32(),
            preferred_monitor_id: r.u32(),
            flags: r.u32(),
            max_luminance_nits: r.u32(),
            max_frame_avg_nits: r.u32(),
            min_luminance_millinits: r.u32(),
        };
        if r.u64() != 0 {
            return Err(WireError::Invalid("reserved"));
        }
        request.validate()?;
        Ok(request)
    }

    pub fn validate(&self) -> Result<(), WireError> {
        if self.session_id == 0 {
            return Err(WireError::Invalid("session id"));
        }
        if !modes::valid_mode(self.width, self.height, self.refresh_mhz) {
            return Err(WireError::Invalid("mode"));
        }
        if self.preferred_monitor_id > modes::MAX_MONITOR_ID {
            return Err(WireError::Invalid("monitor id"));
        }
        if self.flags & !ADD_FLAGS != 0 {
            return Err(WireError::Invalid("flags"));
        }
        Ok(())
    }
}

/// [`IOCTL_ADD`] output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AddReply {
    /// The IddCx adapter's LUID, which owns the OS target.
    pub adapter_luid_low: u32,
    pub adapter_luid_high: i32,
    /// The OS target id; with the adapter LUID it names the display path.
    pub target_id: u32,
    /// The id the monitor took.
    pub monitor_id: u32,
    /// The WUDFHost process the frame-ring handles are duplicated into. Per ADD, so a
    /// restarted WUDFHost cannot leave the host duplicating into a dead process.
    pub wudf_pid: u32,
}

impl AddReply {
    pub const SIZE: usize = 32;

    #[must_use]
    pub fn encode(&self) -> [u8; Self::SIZE] {
        Writer::new()
            .u32(self.adapter_luid_low)
            .i32(self.adapter_luid_high)
            .u32(self.target_id)
            .u32(self.monitor_id)
            .u32(self.wudf_pid)
            .u32(0)
            .u64(0)
            .finish()
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, WireError> {
        let mut r = Reader::new(bytes, Self::SIZE)?;
        let reply = Self {
            adapter_luid_low: r.u32(),
            adapter_luid_high: r.i32(),
            target_id: r.u32(),
            monitor_id: r.u32(),
            wudf_pid: r.u32(),
        };
        if reply.target_id == 0 || reply.wudf_pid == 0 {
            return Err(WireError::Invalid("arrival"));
        }
        Ok(reply)
    }
}

/// Input of the verbs that only name a monitor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionRequest {
    pub session_id: u64,
}

impl SessionRequest {
    pub const SIZE: usize = 8;

    #[must_use]
    pub fn encode(&self) -> [u8; Self::SIZE] {
        Writer::new().u64(self.session_id).finish()
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, WireError> {
        let mut r = Reader::new(bytes, Self::SIZE)?;
        Ok(Self {
            session_id: r.u64(),
        })
    }
}

/// [`IOCTL_CLEAR_FRAME_RING_GENERATION`] input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClearFrameRingRequest {
    pub session_id: u64,
    /// The [`SetFrameRingRequest::generation`] of the ring to drop.
    pub generation: u32,
}

impl ClearFrameRingRequest {
    pub const SIZE: usize = 16;

    #[must_use]
    pub fn encode(&self) -> [u8; Self::SIZE] {
        Writer::new()
            .u64(self.session_id)
            .u32(self.generation)
            .u32(0)
            .finish()
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, WireError> {
        let mut r = Reader::new(bytes, Self::SIZE)?;
        let request = Self {
            session_id: r.u64(),
            generation: r.u32(),
        };
        if request.generation == 0 {
            return Err(WireError::Invalid("generation"));
        }
        Ok(request)
    }
}

/// [`IOCTL_UPDATE_MODES`] input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UpdateModesRequest {
    pub session_id: u64,
    pub width: u32,
    pub height: u32,
    pub refresh_mhz: u32,
}

impl UpdateModesRequest {
    pub const SIZE: usize = 24;

    #[must_use]
    pub fn encode(&self) -> [u8; Self::SIZE] {
        Writer::new()
            .u64(self.session_id)
            .u32(self.width)
            .u32(self.height)
            .u32(self.refresh_mhz)
            .u32(0)
            .finish()
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, WireError> {
        let mut r = Reader::new(bytes, Self::SIZE)?;
        let request = Self {
            session_id: r.u64(),
            width: r.u32(),
            height: r.u32(),
            refresh_mhz: r.u32(),
        };
        if r.u32() != 0 {
            return Err(WireError::Invalid("reserved"));
        }
        if !modes::valid_mode(request.width, request.height, request.refresh_mhz) {
            return Err(WireError::Invalid("mode"));
        }
        Ok(request)
    }
}

/// [`IOCTL_SET_RENDER_ADAPTER`] input, a LUID split like `windows`' `LUID`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RenderAdapterRequest {
    pub luid_low: u32,
    pub luid_high: i32,
}

impl RenderAdapterRequest {
    pub const SIZE: usize = 8;

    #[must_use]
    pub fn encode(&self) -> [u8; Self::SIZE] {
        Writer::new()
            .u32(self.luid_low)
            .i32(self.luid_high)
            .finish()
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, WireError> {
        let mut r = Reader::new(bytes, Self::SIZE)?;
        let request = Self {
            luid_low: r.u32(),
            luid_high: r.i32(),
        };
        if request.luid_low == 0 && request.luid_high == 0 {
            return Err(WireError::Invalid("adapter"));
        }
        Ok(request)
    }
}

/// [`IOCTL_SET_FRAME_RING`] input. Every handle is a value in the driver's WUDFHost,
/// already duplicated there by the host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SetFrameRingRequest {
    pub session_id: u64,
    /// Non-zero and new for every ring of this monitor. It is stamped into each publish
    /// token, so a frame an old ring left behind is never read from the new one.
    pub generation: u32,
    pub slot_count: u32,
    pub width: u32,
    pub height: u32,
    /// A `DXGI_FORMAT`: [`ring::FORMAT_BGRA8`] or [`ring::FORMAT_RGBA16F`].
    pub format: u32,
    /// Bytes the host mapped for the header section, at least [`ring::HEADER_BYTES`].
    pub header_bytes: u32,
    /// The adapter the textures were created on, which must be the one rendering the
    /// monitor.
    pub adapter_luid_low: u32,
    pub adapter_luid_high: i32,
    /// The header section (`CreateFileMapping`), mapped read-write by the driver.
    pub header_section: u64,
    /// The event the driver sets after each publish.
    pub event: u64,
    /// Shared NT handles of `slot_count` keyed-mutex textures; the rest are `0`.
    pub textures: [u64; ring::MAX_SLOTS],
}

impl SetFrameRingRequest {
    pub const SIZE: usize = 128;

    #[must_use]
    pub fn encode(&self) -> [u8; Self::SIZE] {
        let mut w = Writer::new()
            .u64(self.session_id)
            .u32(self.generation)
            .u32(self.slot_count)
            .u32(self.width)
            .u32(self.height)
            .u32(self.format)
            .u32(self.header_bytes)
            .u32(self.adapter_luid_low)
            .i32(self.adapter_luid_high)
            .u64(self.header_section)
            .u64(self.event);
        for texture in self.textures {
            w = w.u64(texture);
        }
        w.u64(0).finish()
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, WireError> {
        let mut r = Reader::new(bytes, Self::SIZE)?;
        let mut request = Self {
            session_id: r.u64(),
            generation: r.u32(),
            slot_count: r.u32(),
            width: r.u32(),
            height: r.u32(),
            format: r.u32(),
            header_bytes: r.u32(),
            adapter_luid_low: r.u32(),
            adapter_luid_high: r.i32(),
            header_section: r.u64(),
            event: r.u64(),
            textures: [0; ring::MAX_SLOTS],
        };
        for texture in &mut request.textures {
            *texture = r.u64();
        }
        if r.u64() != 0 {
            return Err(WireError::Invalid("reserved"));
        }
        request.validate()?;
        Ok(request)
    }

    pub fn validate(&self) -> Result<(), WireError> {
        if self.session_id == 0 {
            return Err(WireError::Invalid("session id"));
        }
        if self.generation == 0 || self.generation > ring::Token::GENERATION_MASK {
            return Err(WireError::Invalid("generation"));
        }
        let slots = self.slot_count as usize;
        if !(ring::MIN_SLOTS..=ring::MAX_SLOTS).contains(&slots) {
            return Err(WireError::Invalid("slot count"));
        }
        if !(1..=modes::MAX_DIMENSION).contains(&self.width)
            || !(1..=modes::MAX_DIMENSION).contains(&self.height)
        {
            return Err(WireError::Invalid("size"));
        }
        if !ring::supported_format(self.format) {
            return Err(WireError::Invalid("format"));
        }
        if (self.header_bytes as usize) < ring::HEADER_BYTES {
            return Err(WireError::Invalid("header size"));
        }
        if self.header_section == 0 || self.event == 0 {
            return Err(WireError::Invalid("handle"));
        }
        let (used, unused) = self.textures.split_at(slots);
        if used.contains(&0) || unused.iter().any(|&t| t != 0) {
            return Err(WireError::Invalid("textures"));
        }
        Ok(())
    }

    /// Every handle value the request carries, for the host to close remotely when the
    /// driver refused it.
    pub fn handles(&self) -> impl Iterator<Item = u64> + '_ {
        [self.header_section, self.event]
            .into_iter()
            .chain(self.textures.iter().copied())
            .filter(|&h| h != 0)
    }
}

/// [`IOCTL_SET_FENCE_RING`] input: a ring of plain shared textures (no keyed mutex) and the
/// two shared fences that order access to them, every handle a value in the driver's WUDFHost.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SetFenceRingRequest {
    /// The ring as a keyed-mutex request would describe it.
    pub ring: SetFrameRingRequest,
    /// The driver signals it with a frame's sequence once that frame's copy is done.
    pub written_fence: u64,
    /// The host signals it once its reads of a slot are done, with the value it records in
    /// the slot's state.
    pub released_fence: u64,
}

impl SetFenceRingRequest {
    pub const SIZE: usize = SetFrameRingRequest::SIZE + 24;

    #[must_use]
    pub fn encode(&self) -> [u8; Self::SIZE] {
        Writer::new()
            .bytes(&self.ring.encode())
            .u64(self.written_fence)
            .u64(self.released_fence)
            .u64(0)
            .finish()
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, WireError> {
        let mut r = Reader::new(bytes, Self::SIZE)?;
        let ring = SetFrameRingRequest::decode(&r.take::<{ SetFrameRingRequest::SIZE }>())?;
        let request = Self {
            ring,
            written_fence: r.u64(),
            released_fence: r.u64(),
        };
        if r.u64() != 0 {
            return Err(WireError::Invalid("reserved"));
        }
        request.validate()?;
        Ok(request)
    }

    pub fn validate(&self) -> Result<(), WireError> {
        self.ring.validate()?;
        if self.written_fence == 0
            || self.released_fence == 0
            || self.written_fence == self.released_fence
        {
            return Err(WireError::Invalid("fences"));
        }
        if self
            .handles()
            .enumerate()
            .any(|(index, handle)| self.handles().take(index).any(|earlier| earlier == handle))
        {
            return Err(WireError::Invalid("duplicate handles"));
        }
        Ok(())
    }

    /// Every handle value the request carries, for the host to close remotely when the
    /// driver refused it.
    pub fn handles(&self) -> impl Iterator<Item = u64> + '_ {
        self.ring
            .handles()
            .chain([self.written_fence, self.released_fence])
            .filter(|&h| h != 0)
    }
}

/// [`MonitorStatus::flags`]: arrived and visible to Windows.
pub const MONITOR_ARRIVED: u32 = 1 << 0;
/// [`MonitorStatus::flags`]: Windows assigned a swap chain, so frames are being composed.
pub const MONITOR_SWAP_CHAIN: u32 = 1 << 1;
/// [`MonitorStatus::flags`]: the driver holds a frame ring for it.
pub const MONITOR_RING: u32 = 1 << 2;
/// [`MonitorStatus::flags`]: that ring's textures are open on the rendering device.
pub const MONITOR_RING_OPEN: u32 = 1 << 3;

/// [`IOCTL_QUERY_MONITOR`] output.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct MonitorStatus {
    pub target_id: u32,
    pub monitor_id: u32,
    /// The GPU of the last swap-chain assignment; zero before the first.
    pub render_luid_low: u32,
    pub render_luid_high: i32,
    /// `MONITOR_*`.
    pub flags: u32,
    /// The last composed surface; zero before the first frame.
    pub surface_width: u32,
    pub surface_height: u32,
    pub surface_format: u32,
    pub frames_acquired: u64,
    pub frames_pushed: u64,
    pub ring_generation: u32,
    /// The mode the monitor's list leads with.
    pub width: u32,
    pub height: u32,
    pub refresh_mhz: u32,
}

impl MonitorStatus {
    pub const SIZE: usize = 64;

    #[must_use]
    pub fn encode(&self) -> [u8; Self::SIZE] {
        Writer::new()
            .u32(self.target_id)
            .u32(self.monitor_id)
            .u32(self.render_luid_low)
            .i32(self.render_luid_high)
            .u32(self.flags)
            .u32(self.surface_width)
            .u32(self.surface_height)
            .u32(self.surface_format)
            .u64(self.frames_acquired)
            .u64(self.frames_pushed)
            .u32(self.ring_generation)
            .u32(self.width)
            .u32(self.height)
            .u32(self.refresh_mhz)
            .finish()
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, WireError> {
        let mut r = Reader::new(bytes, Self::SIZE)?;
        Ok(Self {
            target_id: r.u32(),
            monitor_id: r.u32(),
            render_luid_low: r.u32(),
            render_luid_high: r.i32(),
            flags: r.u32(),
            surface_width: r.u32(),
            surface_height: r.u32(),
            surface_format: r.u32(),
            frames_acquired: r.u64(),
            frames_pushed: r.u64(),
            ring_generation: r.u32(),
            width: r.u32(),
            height: r.u32(),
            refresh_mhz: r.u32(),
        })
    }
}

/// Severity byte a [`log_lines`] record opens with. Unknown bytes read as [`LOG_INFO`].
pub const LOG_ERROR: u8 = b'E';
/// See [`LOG_ERROR`].
pub const LOG_WARN: u8 = b'W';
/// See [`LOG_ERROR`].
pub const LOG_INFO: u8 = b'I';

/// Append one [`IOCTL_DRAIN_LOG`] record: the severity byte, the text, `\n`. Newlines in the
/// text become spaces, because the separator is the framing.
pub fn write_log_record(out: &mut alloc::vec::Vec<u8>, level: u8, text: &str) {
    out.push(level);
    out.extend(
        text.as_bytes()
            .iter()
            .map(|&b| if b == b'\n' { b' ' } else { b }),
    );
    out.push(b'\n');
}

/// The records of a drained buffer as `(severity, text)`. An empty or non-UTF-8 record is
/// skipped.
pub fn log_lines(buf: &[u8]) -> impl Iterator<Item = (u8, &str)> {
    buf.split(|&b| b == b'\n')
        .filter(|r| !r.is_empty())
        .filter_map(|r| Some((r[0], core::str::from_utf8(&r[1..]).ok()?)))
}

#[cfg(test)]
mod tests;
