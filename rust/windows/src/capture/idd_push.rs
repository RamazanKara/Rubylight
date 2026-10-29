//! Capture from Rubylight's own virtual display driver (rust/drivers/rubylight-vdisplay).
//!
//! The driver copies every frame Windows composes for its monitor into textures this session
//! creates and shares with it, stamps the frame's present QPC beside it and sets an event
//! (`rubylight_vdisplay_proto::ring`). No WGC and no Desktop Duplication run: a frame is one
//! GPU copy in the driver.
//!
//! The default fence ring uses `CAP_FENCE_RING`: its textures and fences
//! are created on the capture GPU's D3D12 compute device. On AMD the compute conversion reads
//! the ring texture in place, waiting on the GPU for the driver's written fence, and the
//! compute queue signals the released fence once the conversion is done; no D3D11 work runs
//! for it. Elsewhere (and with `idd_push_zero_copy = false`) the capture's D3D11 device waits
//! for the written fence, copies the texture into the owned pool as every other capture's
//! frames are, and signals the released fence after its copy.
//!
//! The keyed-mutex ring remains available with `idd_push_fence_ring = false`. It is also used,
//! with a warning, when fence-ring initialization fails or the installed driver does not
//! support the fence ring.
//!
//! It runs for every automatic capture of a display this host created, and for
//! `capture = idd-push`; physical displays, and `capture = wgc` or `ddx`, keep WGC or Desktop
//! Duplication.

use super::*;
use crate::display::rubylight_vdd::{Control, Owned, Remote, owned_display};
use butterpollo_core::virtual_output::{PushedFrame, PushedFrames};
use rubylight_vdisplay_proto::control::{
    CAP_FENCE_RING, CAP_SCOPED_CLEAR, MonitorStatus, SetFenceRingRequest, SetFrameRingRequest,
};
use rubylight_vdisplay_proto::ring::{self, Consumer, Counters, Header, Kind, Layout, SlotMeta};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use windows::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE, WAIT_TIMEOUT};
use windows::Win32::Graphics::Direct3D12::ID3D12Fence;
use windows::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_ALL_ACCESS, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile,
    PAGE_READWRITE, UnmapViewOfFile,
};
use windows::core::HRESULT;

/// Shapes a ring may be rebuilt to before the first frame, when the driver reports the
/// surface differs from the guess it was opened with.
const RESHAPES_BEFORE_FIRST_FRAME: u32 = 3;
/// The driver stamps its swap-chain loop every pass (at most 125 ms apart while idle). This
/// much silence after frames flowed means the loop stopped.
const DRAIN_STALL: Duration = Duration::from_secs(3);
/// How long a ring may wait for its first frame before the capture gives up.
const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(10);
/// A live producer that acquires frames but cannot publish through fences gets the
/// standard transport sooner. A display Windows has not started composing keeps its
/// full startup allowance above.
const FENCE_HANDOFF_TIMEOUT: Duration = Duration::from_secs(2);
/// How often the driver's frame counters are logged (`idd_push=debug`).
const COUNTERS_EVERY: Duration = Duration::from_secs(5);

/// Rings of one monitor that may fail before their first frame before its captures use WGC.
const FAILURES_BEFORE_WGC: u32 = 2;

/// The monitors this process captures from. The driver keeps one ring per monitor, so a
/// second capture registering its own would take the frames from the first.
static CAPTURING: Mutex<Vec<u64>> = Mutex::new(Vec::new());

/// This process's capture of one monitor, released on drop.
struct Exclusive(u64);

impl Exclusive {
    fn claim(session_id: u64) -> Option<Self> {
        let mut capturing = CAPTURING.lock().unwrap();
        if capturing.contains(&session_id) {
            return None;
        }
        capturing.push(session_id);
        Some(Self(session_id))
    }
}

impl Drop for Exclusive {
    fn drop(&mut self) {
        CAPTURING.lock().unwrap().retain(|&id| id != self.0);
    }
}

/// Monitors whose rings failed before delivering a frame, in which [`FenceScope`], and how
/// often. A failure the driver reports after the ring was accepted (textures that do not
/// open, a surface that never settles, no frame at all) would repeat on every reopen, so after
/// [`FAILURES_BEFORE_WGC`] the monitor's captures use WGC until a ring delivers a frame or the
/// scope changes: the display is recreated, the driver restarts, or it renders on another
/// adapter. Session ids alone are persistent client identities, so a recreated display keeps
/// its id and starts clean only by its new registration.
#[derive(Default)]
struct Failures(Vec<(FenceScope, u32)>);

impl Failures {
    fn failed(&mut self, scope: FenceScope) -> u32 {
        match self
            .0
            .iter_mut()
            .find(|(at, _)| at.session == scope.session)
        {
            Some((at, count)) if *at == scope => {
                *count += 1;
                *count
            }
            Some(entry) => {
                *entry = (scope, 1);
                1
            }
            None => {
                self.0.push((scope, 1));
                1
            }
        }
    }

    fn delivered(&mut self, session_id: u64) {
        self.0.retain(|(at, _)| at.session != session_id);
    }

    /// The failures that send this monitor's captures to WGC in `scope`, if it has them.
    fn excluded(&self, scope: FenceScope) -> Option<u32> {
        self.0
            .iter()
            .find(|(at, count)| *at == scope && *count >= FAILURES_BEFORE_WGC)
            .map(|(_, count)| *count)
    }
}

static FAILURES: Mutex<Failures> = Mutex::new(Failures(Vec::new()));

/// A capture reopen shares this identity; a real display recreation, driver restart or
/// render-adapter change does not. Session ids alone are persistent client identities.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FenceScope {
    session: u64,
    registration: u64,
    driver_pid: u32,
    adapter: (u32, i32),
}

impl FenceScope {
    fn new(monitor: &Owned, adapter: (u32, i32)) -> Self {
        Self {
            session: monitor.session_id,
            registration: monitor.registration,
            driver_pid: monitor.wudf_pid,
            adapter,
        }
    }
}

/// Remember a rejected transport through capture reopen, independently of the generic
/// driver-to-WGC failures. A successful keyed frame must not enable the rejected path.
#[derive(Default)]
struct FenceFailures(Vec<FenceScope>);

impl FenceFailures {
    const LIMIT: usize = 64;

    fn failed(&mut self, scope: FenceScope) {
        self.clear(scope.session);
        if self.0.len() == Self::LIMIT {
            self.0.remove(0);
        }
        self.0.push(scope);
    }

    fn excluded(&self, scope: FenceScope) -> bool {
        self.0.contains(&scope)
    }

    fn clear(&mut self, session: u64) {
        self.0.retain(|entry| entry.session != session);
    }
}

static FENCE_FAILURES: Mutex<FenceFailures> = Mutex::new(FenceFailures(Vec::new()));

#[derive(Debug, PartialEq, Eq)]
enum FenceRecovery {
    /// No GPU work has read this ring yet, so its device can still serve a keyed ring.
    Keyed {
        reason: &'static str,
        remember: bool,
    },
    /// A context may already wait behind failed work. Reopening creates a fresh device.
    Reopen(&'static str),
}

fn fence_recovery(
    status: u32,
    started: bool,
    elapsed: Duration,
    counters: Counters,
    ordering_failed: bool,
    queue_stuck: bool,
) -> Option<FenceRecovery> {
    if queue_stuck {
        return Some(FenceRecovery::Reopen("compute queue cannot be reused"));
    }
    if ordering_failed {
        return Some(FenceRecovery::Reopen("lost GPU ordering"));
    }
    if status == ring::STATUS_OPEN_FAILED {
        return Some(if started {
            FenceRecovery::Reopen("shared resources stopped opening")
        } else {
            FenceRecovery::Keyed {
                reason: "could not open shared resources",
                remember: true,
            }
        });
    }
    // Shape changes and device/adapter loss follow their existing recovery paths.
    if !started && matches!(status, ring::STATUS_WAITING | ring::STATUS_STREAMING) {
        if elapsed >= FENCE_HANDOFF_TIMEOUT && counters.acquired > 0 && counters.pushed == 0 {
            return Some(FenceRecovery::Keyed {
                reason: "could not publish acquired frames",
                remember: true,
            });
        }
        if elapsed >= FIRST_FRAME_TIMEOUT {
            return Some(FenceRecovery::Keyed {
                reason: "did not deliver a frame",
                // Silence alone does not identify a fence problem: Windows may not have
                // supplied any pixels yet. A later capture may try fences again.
                remember: false,
            });
        }
    }
    None
}

/// Microseconds from QPC `from` to `to`, zero when `to` is earlier or either is missing.
fn qpc_micros(from: u64, to: u64, frequency: i64) -> u64 {
    if from == 0 || to <= from || frequency <= 0 {
        return 0;
    }
    ((to - from) as u128 * 1_000_000 / frequency as u128) as u64
}

/// Whether this capture should come from the driver: asked for by name, or automatic on a
/// display this host created. `capture = wgc` or `ddx` keeps those.
pub(super) fn wanted(kind: &str, capture_setting: &str, own_display: bool) -> bool {
    kind == "idd-push" || (matches!(capture_setting.trim(), "" | "auto") && own_display)
}

/// [`wanted`] for a stream on display `name`; the display lookup runs only when it decides.
pub(super) fn wanted_for(
    name: &str,
    kind: &str,
    config: &butterpollo_core::config::Config,
) -> bool {
    let setting = config.get("capture", "auto");
    wanted(kind, setting, false)
        || (wanted(kind, setting, true) && matches!(owned_display(name), Ok(Some(_))))
}

/// The ring to open: the surface the driver last composed when it reported one, otherwise
/// the monitor's mode in the format its colour state suggests.
fn ring_shape(status: &MonitorStatus, hdr: bool) -> (u32, u32, u32) {
    if status.surface_width > 0
        && status.surface_height > 0
        && ring::supported_format(status.surface_format)
    {
        return (
            status.surface_width,
            status.surface_height,
            status.surface_format,
        );
    }
    let format = if hdr {
        ring::FORMAT_RGBA16F
    } else {
        ring::FORMAT_BGRA8
    };
    (status.width, status.height, format)
}

/// The generation after `previous`: never 0 and within the publish token's 24 bits, so a
/// frame an earlier ring of the monitor left in its header is never taken from this one.
fn next_generation(previous: u32) -> u32 {
    (previous % ring::Token::GENERATION_MASK) + 1
}

/// What the header's driver status means for this capture.
#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    Fine,
    /// Retire the ring before its sequence or GPU fence could wrap.
    Renew,
    /// Rebuild the ring at the surface's real `(width, height, format)`.
    Reshape(u32, u32, u32),
    Fail(String),
}

fn verdict(status: (u32, u32), surface: (u32, u32, u32), started: bool) -> Verdict {
    match status.0 {
        ring::STATUS_WAITING | ring::STATUS_STREAMING => Verdict::Fine,
        ring::STATUS_RENEW_REQUIRED => Verdict::Renew,
        ring::STATUS_SIZE_MISMATCH | ring::STATUS_FORMAT_MISMATCH => {
            let (width, height, format) = surface;
            if started {
                // As with WGC, a size or colour-space change reopens the capture, so the
                // encoder is rebuilt for it.
                Verdict::Fail(format!(
                    "the Rubylight display changed to {width}x{height} format {format}"
                ))
            } else if width > 0 && height > 0 && ring::supported_format(format) {
                Verdict::Reshape(width, height, format)
            } else {
                Verdict::Fail(format!(
                    "the Rubylight display composes an unsupported surface ({width}x{height} format {format})"
                ))
            }
        }
        ring::STATUS_OPEN_FAILED => Verdict::Fail(format!(
            "the Rubylight display driver could not open the shared textures ({:#010x})",
            status.1
        )),
        ring::STATUS_WRONG_ADAPTER => Verdict::Fail(
            "the Rubylight display renders on another GPU than the capture opened on; select that GPU or let Windows choose".into(),
        ),
        ring::STATUS_DEVICE_LOST => {
            Verdict::Fail("the Rubylight display driver lost its GPU device".into())
        }
        other => Verdict::Fail(format!("unknown Rubylight display driver status {other}")),
    }
}

/// Whether the driver's drain loop has gone quiet: `beat` and `now` are QPC values.
fn stalled(beat: u64, now: i64, frequency: i64, limit: Duration) -> bool {
    if beat == 0 || now <= 0 || frequency <= 0 {
        return false;
    }
    let age = now.saturating_sub(beat as i64).max(0) as f64 / frequency as f64;
    age > limit.as_secs_f64()
}

fn qpc_now() -> i64 {
    let mut now = 0;
    // SAFETY: QueryPerformanceCounter writes only to the live local.
    let _ = unsafe { windows::Win32::System::Performance::QueryPerformanceCounter(&mut now) };
    now
}

/// Take a keyed mutex without waiting. AcquireSync's WAIT_TIMEOUT is a success HRESULT the
/// `Result` binding would report as `Ok`, so the raw code is inspected (as the WGC helper
/// transport does). Keyed-mutex ring only.
fn acquire(mutex: &IDXGIKeyedMutex, key: u64) -> Result<bool> {
    // SAFETY: `mutex` is a live keyed mutex; AcquireSync takes (this, key, timeout).
    let status = unsafe { (mutex.vtable().AcquireSync)(mutex.as_raw(), key, 0) };
    if status.0 == WAIT_TIMEOUT.0 as i32 {
        return Ok(false);
    }
    anyhow::ensure!(
        status == HRESULT(0),
        "Rubylight frame ring mutex failed: {status:?}"
    );
    Ok(true)
}

/// The shared header: an unnamed section mapped here, its handle duplicated to the driver.
struct Section {
    handle: OwnedHandle,
    base: *mut std::ffi::c_void,
}

// SAFETY: the view is only read and written through atomics (`Header`), from any thread, and
// unmapped once, in Drop; the handle is owned.
unsafe impl Send for Section {}
// SAFETY: as above.
unsafe impl Sync for Section {}

impl Section {
    fn new() -> Result<Self> {
        // SAFETY: an unnamed, pagefile-backed section of SECTION_BYTES; the handle is owned
        // below and the view unmapped in Drop.
        unsafe {
            let handle = CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                None,
                PAGE_READWRITE,
                0,
                ring::SECTION_BYTES as u32,
                PCWSTR::null(),
            )?;
            let handle = OwnedHandle::from_raw_handle(handle.0);
            let view = MapViewOfFile(
                HANDLE(handle.as_raw_handle()),
                FILE_MAP_ALL_ACCESS,
                0,
                0,
                ring::SECTION_BYTES,
            );
            anyhow::ensure!(!view.Value.is_null(), "map the frame ring header");
            Ok(Self {
                handle,
                base: view.Value,
            })
        }
    }

    fn header(&self) -> Header<'_> {
        // SAFETY: the view is page-aligned and SECTION_BYTES long, so it holds HEADER_WORDS
        // aligned u64 words; both processes access them only atomically.
        let words = unsafe {
            std::slice::from_raw_parts(self.base.cast::<AtomicU64>(), ring::HEADER_WORDS)
        };
        Header::new(words).expect("a section holds a whole header")
    }
}

impl Drop for Section {
    fn drop(&mut self) {
        // SAFETY: this value's own single view; the handle closes after, as a field.
        unsafe {
            let _ = UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS { Value: self.base });
        }
    }
}

struct Slot {
    texture: ID3D11Texture2D,
    /// Keyed-mutex ring only.
    mutex: Option<IDXGIKeyedMutex>,
    shared: OwnedHandle,
}

/// A fence ring's fences, and how its slots are freed. The session and every image lent from
/// the ring hold it, so a slot is freed even after the session closed.
pub(super) struct Fences {
    section: Arc<Section>,
    compute: Arc<crate::compute::Compute>,
    /// Imports stay available while any lent image can still be converted, even after the
    /// session replaces the ring. Also rolls back a partially constructed ring.
    imports: Mutex<Vec<ID3D11Texture2D>>,
    /// A failed ordering operation quarantines this ring. Never free a slot whose GPU
    /// readers may not be covered by its completion fence.
    failed: AtomicBool,
    /// The driver signals it with a frame's sequence once its copy is done.
    written: ID3D12Fence,
    written11: ID3D11Fence,
    /// Signalled with each slot's release value after the host's last read of the slot.
    released: ID3D12Fence,
    released11: ID3D11Fence,
    /// For the driver: `[written, released]`.
    handles: [OwnedHandle; 2],
    context: ID3D11DeviceContext4,
    /// D3D11 reads of a lent texture reach the compute queue through it, so the released
    /// signal follows them.
    bridge: (ID3D12Fence, ID3D11Fence),
    /// Whose queue signals the released fence: the compute queue when the ring is read in
    /// place, else the capture's D3D11 device. Always the same one, so the values reach the
    /// fence in the order they were handed out.
    on_compute: bool,
    /// The last release and bridge values handed out.
    values: Mutex<(u64, u64)>,
}

// SAFETY: the D3D12 fences and the compute queue are free-threaded, the D3D11 context's device
// is multithread-protected (`capture::Device::create`), and the values are locked.
unsafe impl Send for Fences {}
// SAFETY: as above.
unsafe impl Sync for Fences {}

impl Fences {
    fn fail_ordering(&self) {
        self.failed.store(true, Ordering::Release);
        if self.on_compute {
            // Earlier readers may have queued a written/bridge wait whose matching
            // signal is now uncertain. A fresh D3D11 device alone cannot retire it.
            self.compute.retire();
        }
    }

    fn order_d3d11(&self, seq: u32) -> Result<()> {
        // SAFETY: the shared fence is opened on this multithread-protected context's device.
        let waited = unsafe { self.context.Wait(&self.written11, u64::from(seq)) };
        if waited.is_err() {
            self.fail_ordering();
        }
        waited.context("order a driver frame's graphics reader")
    }

    fn new(
        gpu: &Device,
        compute: Arc<crate::compute::Compute>,
        section: Arc<Section>,
        on_compute: bool,
    ) -> Result<Self> {
        let device: ID3D11Device5 = gpu.device.cast()?;
        let (written, written11, written_handle) = compute.share_fence(&device)?;
        let (released, released11, released_handle) = compute.share_fence(&device)?;
        let (bridge, bridge11, _) = compute.share_fence(&device)?;
        Ok(Self {
            section,
            compute,
            imports: Mutex::new(Vec::new()),
            failed: AtomicBool::new(false),
            written,
            written11,
            released,
            released11,
            handles: [written_handle, released_handle],
            context: gpu.context.cast()?,
            bridge: (bridge, bridge11),
            on_compute,
            values: Mutex::new((0, 0)),
        })
    }

    /// Free `slot`, claimed for frame `seq`, behind every read of it submitted so far: on the
    /// compute queue for a ring read in place (and on the D3D11 device too when `d3d11_read`),
    /// else on the D3D11 device.
    fn free(&self, slot: usize, seq: u32, d3d11_read: bool) -> Result<()> {
        let mut values = self.values.lock().unwrap();
        anyhow::ensure!(
            !self.failed.load(Ordering::Acquire),
            "frame ring ordering failed"
        );
        values.0 += 1;
        let release = values.0;
        // SAFETY: every fence was created on the compute device and opened on this context's
        // device; the context is multithread-protected and the queue free-threaded.
        let signalled = unsafe {
            if self.on_compute {
                let bridged = if d3d11_read {
                    values.1 += 1;
                    self.context
                        .Signal(&self.bridge.1, values.1)
                        .map(|()| self.context.Flush())
                        .and_then(|()| self.compute.queue.Wait(&self.bridge.0, values.1))
                } else {
                    Ok(())
                };
                bridged.and_then(|()| self.compute.queue.Signal(&self.released, release))
            } else {
                self.context
                    .Signal(&self.released11, release)
                    .map(|()| self.context.Flush())
            }
        };
        // A failed signal does not prove the reads completed. Keep the slot owned until
        // Session retires this ring; a later signal must not make an unsafe slot reusable.
        if let Err(error) = signalled {
            self.fail_ordering();
            return Err(error).context("free a Rubylight frame ring slot");
        }
        let freed = Consumer::release(&self.section.header(), slot, seq, release);
        drop(values);
        if !freed {
            self.fail_ordering();
        }
        anyhow::ensure!(
            freed,
            "Rubylight frame ring slot {slot} was not the host's to free"
        );
        Ok(())
    }
}

impl Drop for Fences {
    fn drop(&mut self) {
        for texture in self.imports.get_mut().unwrap() {
            self.compute.forget(texture);
        }
    }
}

/// A fence-ring slot an image reads in place. The slot is freed when this drops, with the
/// image's last clone.
pub(crate) struct Lent {
    fences: Arc<Fences>,
    slot: usize,
    seq: u32,
    /// Whether a D3D11 reader waited for the driver's copy.
    d3d11: Mutex<bool>,
}

impl Lent {
    /// A bounded reader wait failed. Retain this slot and let capture reopen on the
    /// standard transport without treating the unrelated compute queue as broken.
    pub(super) fn reject(&self) {
        self.fences.failed.store(true, Ordering::Release);
    }

    /// Make the capture's D3D11 device wait for the driver's copy before any D3D11 read of
    /// the texture. Only readers other than the compute conversion need it.
    pub(super) fn order_d3d11(&self) -> Result<()> {
        let mut waited = self.d3d11.lock().unwrap();
        if !*waited {
            self.fences.order_d3d11(self.seq)?;
            *waited = true;
        }
        Ok(())
    }
}

impl Drop for Lent {
    fn drop(&mut self) {
        let d3d11 = *self.d3d11.lock().unwrap();
        if let Err(error) = self.fences.free(self.slot, self.seq, d3d11) {
            tracing::debug!(error = %format!("{error:#}"), slot = self.slot, "Rubylight frame ring slot retained after a GPU ordering error");
        }
    }
}

/// One ring: its header and textures, and a fence ring's fences. The driver holds
/// duplicates of every handle until the ring is replaced or cleared.
struct Ring {
    section: Arc<Section>,
    slots: Vec<Slot>,
    layout: Layout,
    /// `None` for a keyed-mutex ring.
    fences: Option<Arc<Fences>>,
}

impl Ring {
    /// A fence ring: its textures and fences are created on the compute device's D3D12, the
    /// textures opened on the capture's D3D11 device for its copies and readers.
    fn fenced(
        gpu: &Device,
        compute: Arc<crate::compute::Compute>,
        layout: Layout,
        read_in_place: bool,
    ) -> Result<Self> {
        let section = Arc::new(Section::new()?);
        section.header().init(layout);
        let device: ID3D11Device1 = gpu.device.cast()?;
        // Build the owner before importing textures: a later allocation/open failure must
        // remove every preceding import from Compute's cache as this partially built ring
        // drops, just as a complete ring does.
        let fences = Fences::new(gpu, compute.clone(), section.clone(), read_in_place)?;
        let mut ring = Self {
            section,
            slots: Vec::with_capacity(layout.slot_count as usize),
            layout,
            fences: Some(Arc::new(fences)),
        };
        for _ in 0..layout.slot_count {
            let (texture, shared) = compute.share_texture(
                &device,
                layout.width,
                layout.height,
                DXGI_FORMAT(layout.format as i32),
            )?;
            ring.fences
                .as_ref()
                .unwrap()
                .imports
                .lock()
                .unwrap()
                .push(texture.clone());
            ring.slots.push(Slot {
                texture,
                mutex: None,
                shared,
            });
        }
        Ok(ring)
    }

    /// The keyed-mutex ring, also compatible with drivers before `CAP_FENCE_RING`.
    fn keyed(gpu: &Device, layout: Layout) -> Result<Self> {
        let section = Arc::new(Section::new()?);
        section.header().init(layout);
        let desc = D3D11_TEXTURE2D_DESC {
            Width: layout.width,
            Height: layout.height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT(layout.format as i32),
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: (D3D11_RESOURCE_MISC_SHARED_NTHANDLE.0
                | D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX.0) as u32,
        };
        let mut slots = Vec::with_capacity(layout.slot_count as usize);
        for _ in 0..layout.slot_count {
            // SAFETY: `gpu.device` is live and `desc` describes a valid shareable texture; the
            // shared NT handle is owned by the slot.
            unsafe {
                let mut texture = None;
                gpu.device
                    .CreateTexture2D(&desc, None, Some(&mut texture))?;
                let texture = texture.context("frame ring texture missing")?;
                let shared = texture.cast::<IDXGIResource1>()?.CreateSharedHandle(
                    None,
                    DXGI_SHARED_RESOURCE_READ.0 | DXGI_SHARED_RESOURCE_WRITE.0,
                    None,
                )?;
                slots.push(Slot {
                    mutex: Some(texture.cast()?),
                    texture,
                    shared: OwnedHandle::from_raw_handle(shared.0),
                });
            }
        }
        Ok(Self {
            section,
            slots,
            layout,
            fences: None,
        })
    }
}

/// Handle values duplicated into the driver's process for one request, closed there again
/// unless the driver adopted them.
struct Delivery<'a> {
    remote: &'a Remote,
    values: Vec<u64>,
    adopted: bool,
}

impl Delivery<'_> {
    fn send(&mut self, handle: HANDLE) -> Result<u64> {
        let value = self.remote.duplicate(handle)?;
        self.values.push(value);
        Ok(value)
    }
}

impl Drop for Delivery<'_> {
    fn drop(&mut self) {
        if !self.adopted {
            for value in self.values.drain(..) {
                self.remote.close(value);
            }
        }
    }
}

pub struct Session {
    pub(super) gpu: Device,
    control: Control,
    monitor: Owned,
    remote: Remote,
    adapter: (u32, i32),
    ring: Ring,
    /// Where a fence ring's textures and fences live: the capture GPU's compute device.
    /// `None` when the driver takes only keyed-mutex rings.
    compute: Option<Arc<crate::compute::Compute>>,
    consumer: Consumer,
    pool: GpuPool,
    /// Lends keyed-mutex ring textures to the compute conversion instead of copying them
    /// (AMD). A fence ring lends without it.
    lender: Option<Arc<Mutex<crate::compute::Returns>>>,
    /// The compute conversion reads ring textures in place (AMD, `idd_push_zero_copy`).
    zero_copy: bool,
    frame: Arc<crate::timing::Signal>,
    staging: Option<ID3D11Texture2D>,
    started: bool,
    reshapes: u32,
    opened: Instant,
    next_check: Instant,
    counted: (Instant, Counters),
    /// Released after `Drop` cleared the ring.
    _exclusive: Exclusive,
}

impl Session {
    pub(super) fn new(
        name: &str,
        hdr: bool,
        config: &butterpollo_core::config::Config,
        warnings: Arc<butterpollo_core::session::Warnings>,
    ) -> Result<Self> {
        let monitor = owned_display(name)?.with_context(|| {
            format!("{name} is not a display this host created with the Rubylight display driver")
        })?;
        let exclusive = Exclusive::claim(monitor.session_id).with_context(|| {
            format!("another stream with different capture settings already captures {name} from the Rubylight display driver")
        })?;
        let control = Control::open()?;
        let status = control.query(monitor.session_id)?;
        let gpu = Device::for_display(
            name,
            config.get("adapter_name", ""),
            config.get("adapter_pnp_id", ""),
        )?;
        // SAFETY: the device is live; GetAdapter and GetDesc return owned values.
        let adapter = unsafe {
            let desc = gpu.device.cast::<IDXGIDevice>()?.GetAdapter()?.GetDesc()?;
            (desc.AdapterLuid.LowPart, desc.AdapterLuid.HighPart)
        };
        let render = (status.render_luid_low, status.render_luid_high);
        if render != (0, 0) && render != adapter {
            bail!(
                "the Rubylight display renders on GPU {:08x}:{:08x}, but capture opened on {:08x}:{:08x}",
                render.1,
                render.0,
                adapter.1,
                adapter.0
            );
        }
        if let Some(failures) = FAILURES
            .lock()
            .unwrap()
            .excluded(FenceScope::new(&monitor, adapter))
        {
            bail!(
                "its frame ring failed {failures} times before delivering a frame; capturing with WGC until the display is recreated, the driver restarts or it renders on another GPU"
            );
        }
        if let Err(error) = crate::gpu_priority::configure(&gpu, config) {
            tracing::debug!(%error, "GPU priority remains at the process default");
        }
        let remote = Remote::open(monitor.wudf_pid)?;
        let mut pool = GpuPool {
            warnings: warnings.clone(),
            ..Default::default()
        };
        if crate::compute::enabled(config) {
            match crate::compute::Compute::for_device(&gpu.device)
                .and_then(|compute| crate::compute::Handoff::new(compute, &gpu))
            {
                Ok(handoff) => pool.compute = Some(handoff),
                Err(error) => warnings.set("capture_compute", format!("Compute capture copies unavailable ({error:#}); copying on the graphics queue. Lower game GPU load or update the AMD driver if capture stutters.")),
            }
        }
        let zero_copy = pool.compute.is_some() && config.boolean("idd_push_zero_copy", true);
        let requested = config.idd_push_fence_ring();
        let scope = FenceScope::new(&monitor, adapter);
        let rejected = {
            let mut failures = FENCE_FAILURES.lock().unwrap();
            if !requested {
                failures.clear(monitor.session_id);
            }
            failures.excluded(scope)
        };
        let RingChoice { kind, warning } =
            choose_ring(control.info.caps, requested, rejected, zero_copy);
        warnings.clear("capture_fence_ring");
        if let Some(warning) = warning {
            tracing::warn!(output = %name, "{warning}");
            warnings.set("capture_fence_ring", warning);
        }
        // Before the first frame the driver may not know the surface yet: guess from the
        // display's colour state, and rebuild once it reports the real one.
        let native_hdr = hdr
            || gpu
                .color_description()?
                .is_some_and(|d| d.color_space == DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020);
        let (width, height, format) = ring_shape(&status, native_hdr);
        let layout = Layout {
            kind,
            slot_count: ring::DEFAULT_SLOTS as u32,
            width,
            height,
            format,
            generation: next_generation(status.ring_generation),
        };
        let frame = Arc::new(crate::timing::Signal::new()?);
        let ring = Self::make_ring(&gpu, layout, zero_copy, &warnings, scope)?;
        let compute = ring.fences.as_ref().map(|fences| fences.compute.clone());
        let now = Instant::now();
        let mut session = Self {
            gpu,
            control,
            monitor,
            remote,
            adapter,
            consumer: Consumer::new(layout.generation, layout.slot_count),
            ring,
            compute,
            pool,
            lender: None,
            zero_copy,
            frame,
            staging: None,
            started: false,
            reshapes: 0,
            opened: now,
            next_check: now,
            counted: (now, Counters::default()),
            _exclusive: exclusive,
        };
        if let Err(error) = session.deliver() {
            if session.ring.fences.is_none() {
                return Err(error);
            }
            session.use_keyed_ring(&format!("initialization failed: {error:#}"), true)?;
        }
        tracing::info!(
            output = %name,
            width,
            height,
            format,
            ring = ?session.ring.layout.kind,
            target = session.monitor.target,
            compute = session.pool.compute.is_some(),
            in_place = session.zero_copy,
            "capturing from the Rubylight display driver's frame push"
        );
        Ok(session)
    }

    /// A failed fence allocation never prevents the keyed-mutex capture path.
    fn make_ring(
        gpu: &Device,
        mut layout: Layout,
        zero_copy: bool,
        warnings: &butterpollo_core::session::Warnings,
        scope: FenceScope,
    ) -> Result<Ring> {
        if layout.kind == Kind::Fence {
            match crate::compute::Compute::for_device(&gpu.device)
                .and_then(|compute| Ring::fenced(gpu, compute, layout, zero_copy))
            {
                Ok(ring) => return Ok(ring),
                Err(error) => {
                    FENCE_FAILURES.lock().unwrap().failed(scope);
                    let message = format!(
                        "GPU fence transport unavailable ({error:#}); using the keyed-mutex driver transport."
                    );
                    tracing::warn!("{message}");
                    warnings.set("capture_fence_ring", message);
                }
            }
        }
        layout.kind = Kind::KeyedMutex;
        Ring::keyed(gpu, layout)
    }

    /// Replace a refused or unopened fence ring with the established transport in this
    /// session. Its generation changes, so a delayed completion from the old ring is ignored.
    fn use_keyed_ring(&mut self, reason: &str, remember: bool) -> Result<()> {
        if remember {
            FENCE_FAILURES
                .lock()
                .unwrap()
                .failed(FenceScope::new(&self.monitor, self.adapter));
        }
        let message =
            format!("GPU fence transport {reason}; using the keyed-mutex driver transport.");
        tracing::warn!("{message}");
        self.pool.warnings.set("capture_fence_ring", message);
        let mut layout = self.ring.layout;
        layout.kind = Kind::KeyedMutex;
        layout.generation = next_generation(layout.generation);
        self.ring = Ring::keyed(&self.gpu, layout)?;
        self.compute = None;
        self.consumer = Consumer::new(layout.generation, layout.slot_count);
        self.opened = Instant::now();
        self.started = false;
        self.deliver()
    }

    /// Never put a new ring behind an unresolved wait on the old capture context. The
    /// capture worker recreates its D3D11 device; the rejection makes that reopen keyed.
    fn reopen_keyed(&self, reason: &str) -> Result<()> {
        FENCE_FAILURES
            .lock()
            .unwrap()
            .failed(FenceScope::new(&self.monitor, self.adapter));
        if let Some(fences) = &self.ring.fences {
            fences.failed.store(true, Ordering::Release);
        }
        let message = format!(
            "GPU fence transport {reason}; reopening with the keyed-mutex driver transport."
        );
        tracing::warn!("{message}");
        self.pool.warnings.set("capture_fence_ring", message);
        bail!("GPU fence transport {reason}; recreate the capture device")
    }

    /// Hand the current ring to the driver, after lending a keyed-mutex ring's textures to
    /// the compute conversion or importing them into the compute copy.
    fn deliver(&mut self) -> Result<()> {
        self.lender = None;
        if self.ring.fences.is_none() && self.zero_copy {
            let lent = crate::compute::Compute::for_device(&self.gpu.device)
                .and_then(|compute| crate::compute::Returns::new(compute, &self.gpu))
                .and_then(|mut returns| {
                    for slot in &self.ring.slots {
                        returns.import(&slot.texture, HANDLE(slot.shared.as_raw_handle()))?;
                    }
                    Ok(returns)
                });
            match lent {
                Ok(returns) => self.lender = Some(Arc::new(Mutex::new(returns))),
                Err(error) => {
                    tracing::warn!(error = %format!("{error:#}"), "the Rubylight frame ring cannot be read in place; copying it");
                    self.zero_copy = false;
                }
            }
        }
        if !self.zero_copy
            && let Some(handoff) = &mut self.pool.compute
        {
            let imported = self.ring.slots.iter().try_for_each(|slot| {
                handoff.import_shared(&slot.texture, HANDLE(slot.shared.as_raw_handle()))
            });
            if let Err(error) = imported {
                self.pool.warnings.set("capture_compute", format!("The Rubylight frame ring cannot use compute copies ({error:#}); copying on the graphics queue."));
                self.pool.compute = None;
            }
        }
        let layout = self.ring.layout;
        let mut delivery = Delivery {
            remote: &self.remote,
            values: Vec::new(),
            adopted: false,
        };
        let mut textures = [0; ring::MAX_SLOTS];
        for (value, slot) in textures.iter_mut().zip(&self.ring.slots) {
            *value = delivery.send(HANDLE(slot.shared.as_raw_handle()))?;
        }
        let request = SetFrameRingRequest {
            session_id: self.monitor.session_id,
            generation: layout.generation,
            slot_count: layout.slot_count,
            width: layout.width,
            height: layout.height,
            format: layout.format,
            header_bytes: ring::SECTION_BYTES as u32,
            adapter_luid_low: self.adapter.0,
            adapter_luid_high: self.adapter.1,
            header_section: delivery.send(HANDLE(self.ring.section.handle.as_raw_handle()))?,
            event: delivery.send(self.frame.raw())?,
            textures,
        };
        let refused = "the Rubylight display driver refused the frame ring";
        match &self.ring.fences {
            Some(fences) => {
                let [written, released] = &fences.handles;
                let request = SetFenceRingRequest {
                    ring: request,
                    written_fence: delivery.send(HANDLE(written.as_raw_handle()))?,
                    released_fence: delivery.send(HANDLE(released.as_raw_handle()))?,
                };
                request
                    .validate()
                    .map_err(|e| anyhow::anyhow!("frame ring request: {e}"))?;
                self.control.set_fence_ring(&request).context(refused)?;
            }
            None => {
                request
                    .validate()
                    .map_err(|e| anyhow::anyhow!("frame ring request: {e}"))?;
                self.control.set_frame_ring(&request).context(refused)?;
            }
        }
        delivery.adopted = true;
        Ok(())
    }

    /// Rebuild the ring at the shape the driver reported, before any frame was taken.
    fn reshape(&mut self, width: u32, height: u32, format: u32) -> Result<()> {
        self.reshapes += 1;
        anyhow::ensure!(
            self.reshapes <= RESHAPES_BEFORE_FIRST_FRAME,
            "the Rubylight display surface keeps changing shape"
        );
        let mut layout = self.ring.layout;
        layout.width = width;
        layout.height = height;
        layout.format = format;
        layout.generation = next_generation(layout.generation);
        tracing::info!(
            width,
            height,
            format,
            "rebuilding the Rubylight frame ring for the composed surface"
        );
        self.ring = Self::make_ring(
            &self.gpu,
            layout,
            self.zero_copy,
            &self.pool.warnings,
            FenceScope::new(&self.monitor, self.adapter),
        )?;
        self.compute = self
            .ring
            .fences
            .as_ref()
            .map(|fences| fences.compute.clone());
        self.consumer = Consumer::new(layout.generation, layout.slot_count);
        self.opened = Instant::now();
        if let Err(error) = self.deliver() {
            if self.ring.fences.is_none() {
                return Err(error);
            }
            self.use_keyed_ring(&format!("ring rebuild failed: {error:#}"), true)?;
        }
        Ok(())
    }

    fn check(&mut self) -> Result<()> {
        if let Some(loss) = crate::device_loss::DeviceLost::d3d11(&self.gpu.device) {
            return Err(loss.into());
        }
        if let Some(loss) = self
            .compute
            .as_ref()
            .and_then(|compute| crate::device_loss::DeviceLost::d3d12(&compute.device))
        {
            return Err(loss.into());
        }
        let header = self.ring.section.header();
        if let Some(fences) = &self.ring.fences
            && let Some(recovery) = fence_recovery(
                header.status().0,
                self.started,
                self.opened.elapsed(),
                header.counters(),
                fences.failed.load(Ordering::Acquire),
                fences.compute.is_stuck(),
            )
        {
            return match recovery {
                FenceRecovery::Keyed { reason, remember } => self.use_keyed_ring(reason, remember),
                FenceRecovery::Reopen(reason) => self.reopen_keyed(reason),
            };
        }
        match verdict(header.status(), header.surface(), self.started) {
            Verdict::Fine => {}
            Verdict::Renew => {
                // Same shape, new resources and fences: all old frame readers keep their
                // own ownership until done. This is not an unstable-shape retry.
                let reshapes = self.reshapes;
                self.reshapes = 0;
                let layout = self.ring.layout;
                let renewed = self.reshape(layout.width, layout.height, layout.format);
                self.reshapes = reshapes;
                self.started = false;
                return renewed;
            }
            Verdict::Reshape(width, height, format) => return self.reshape(width, height, format),
            Verdict::Fail(message) => bail!(message),
        }
        let counters = header.counters();
        if self.counted.0.elapsed() >= COUNTERS_EVERY {
            let last = std::mem::replace(&mut self.counted, (Instant::now(), counters)).1;
            tracing::debug!(
                target: "idd_push",
                acquired = counters.acquired.saturating_sub(last.acquired),
                pushed = counters.pushed.saturating_sub(last.pushed),
                dropped = counters.dropped.saturating_sub(last.dropped),
                overwritten = counters.overwritten.saturating_sub(last.overwritten),
                "Rubylight frame ring counters"
            );
        }
        if self.started {
            anyhow::ensure!(
                !stalled(header.heartbeat(), qpc_now(), qpc_frequency(), DRAIN_STALL),
                "the Rubylight display driver stopped delivering frames"
            );
        } else {
            anyhow::ensure!(
                self.opened.elapsed() < FIRST_FRAME_TIMEOUT,
                "the Rubylight display driver delivered no frame"
            );
        }
        Ok(())
    }

    pub(super) fn frame_signal(&self) -> &crate::timing::Signal {
        &self.frame
    }
    pub(super) fn pending_frame(&self) -> bool {
        self.consumer
            .wanted(self.ring.section.header().latest())
            .is_some()
    }

    pub(super) fn next_frame(&mut self) -> Result<Option<Image>> {
        self.next_gpu()?
            .map(|image| image.readback(&mut self.staging))
            .transpose()
    }

    pub(super) fn next_gpu(&mut self) -> Result<Option<GpuImage>> {
        let now = Instant::now();
        if now >= self.next_check {
            self.next_check = now + Duration::from_millis(100);
            if let Err(error) = self.check() {
                if !self.started && crate::device_loss::DeviceLost::from_error(&error).is_none() {
                    let failures = FAILURES
                        .lock()
                        .unwrap()
                        .failed(FenceScope::new(&self.monitor, self.adapter));
                    tracing::warn!(error = %format!("{error:#}"), failures, "the Rubylight frame ring failed before its first frame");
                }
                return Err(error);
            }
        }
        let Some((frame, mut image)) = self.take_newest()? else {
            return Ok(None);
        };
        (image.captured, image.wgc_stamp) =
            qpc_timestamps(frame.presented().min(i64::MAX as u64) as i64);
        // This exact, immutable host timestamp also appears in the stream's claim
        // trace, joining driver identity to the selected image without changing
        // frame ownership or carrying diagnostic metadata through every backend.
        tracing::trace!(
            target: "idd_push",
            seq = frame.seq,
            frame = frame.frame_number,
            acquired_qpc = frame.acquired,
            scanout_qpc = frame.scanout,
            host_acquired = ?image.acquired,
            "image"
        );
        Ok(Some(image))
    }

    /// Record `meta`'s frame as consumed (taken from the ring at QPC `taken`).
    fn finish(&mut self, meta: &SlotMeta, taken: u64) -> PushedFrame {
        self.consumer.consumed(meta.seq);
        self.ring
            .section
            .header()
            .set_consumed(self.consumer.last());
        if !self.started {
            FAILURES.lock().unwrap().delivered(self.monitor.session_id);
        }
        self.started = true;
        let frequency = qpc_frequency();
        tracing::trace!(
            target: "idd_push",
            seq = meta.seq,
            frame = meta.frame_number,
            scanout_ahead_us = qpc_micros(meta.acquire_qpc, meta.present_qpc, frequency),
            driver_us = qpc_micros(meta.acquire_qpc, meta.publish_qpc, frequency),
            handoff_us = qpc_micros(meta.publish_qpc, taken, frequency),
            "frame"
        );
        PushedFrame {
            seq: meta.seq,
            frame_number: meta.frame_number,
            acquired: meta.acquire_qpc,
            scanout: meta.present_qpc,
        }
    }

    /// The newest frame of a fence ring. Read in place, the slot is the image's until its
    /// last clone drops, and the compute conversion waits for the driver's copy on the GPU;
    /// copied, the capture's D3D11 device waits for that copy, copies and frees the slot
    /// behind its copy at once.
    fn take_fenced(&mut self, fences: &Arc<Fences>) -> Result<Option<(PushedFrame, GpuImage)>> {
        if !self.zero_copy && !self.pool.has_capacity() {
            return Ok(None);
        }
        let header = self.ring.section.header();
        let Some((index, seq)) = self.consumer.claim(&header) else {
            // Nothing new, or the driver is refilling that slot with a newer frame; its
            // publish wakes the next poll.
            return Ok(None);
        };
        let taken = qpc_now().max(0) as u64;
        let meta = header.slot(index);
        let slot = &self.ring.slots[index];
        if self.zero_copy {
            let lent = Lent {
                fences: fences.clone(),
                slot: index,
                seq,
                d3d11: Mutex::new(false),
            };
            let ready =
                crate::compute::Ready::on(&fences.compute, fences.written.clone(), u64::from(seq));
            let image = lend_fenced(&self.gpu, &slot.texture, index, lent, ready)?;
            return Ok(Some((self.finish(&meta, taken), image)));
        }
        let copied = fences
            .order_d3d11(seq)
            .and_then(|()| self.pool.copy(&self.gpu, &slot.texture));
        // The copy is queued on this device (or ordered before it by the compute handoff),
        // so freeing now cannot let the driver overwrite pixels still being read.
        let freed = fences.free(index, seq, false);
        let image = copied?;
        freed?;
        let Some(image) = image else {
            return Ok(None);
        };
        Ok(Some((self.finish(&meta, taken), image)))
    }

    /// The newest frame of the default keyed-mutex ring.
    fn take_keyed(&mut self) -> Result<Option<(PushedFrame, GpuImage)>> {
        // Leave the newest frame published while every snapshot is held: the driver keeps
        // the texture updated, and it is taken once a snapshot frees. A lent texture needs
        // no snapshot.
        if self.lender.is_none() && !self.pool.has_capacity() {
            return Ok(None);
        }
        let header = self.ring.section.header();
        let Some(index) = self.consumer.wanted(header.latest()) else {
            return Ok(None);
        };
        let slot = &self.ring.slots[index];
        let mutex = slot.mutex.as_ref().context("a keyed-mutex ring's slot")?;
        if !acquire(mutex, ring::KEY_HOST)? {
            // The driver is refilling it with a newer frame; its publish wakes the next poll.
            return Ok(None);
        }
        let taken = qpc_now().max(0) as u64;
        let meta = header.slot(index);
        let image = if !self.consumer.fresh(&meta) {
            Ok(None)
        } else if let Some(lender) = &self.lender {
            match lend(&self.gpu, slot, mutex, index, lender.clone()) {
                // The mutex goes back with the image's last clone.
                Ok(image) => return Ok(Some((self.finish(&meta, taken), image))),
                Err(error) => Err(error),
            }
        } else {
            self.pool.copy(&self.gpu, &slot.texture)
        };
        // A copy is queued on this device (or ordered before it by the compute handoff), so
        // releasing now cannot let the driver overwrite pixels still being read.
        // SAFETY: this process took the mutex with KEY_HOST above; the context is this
        // device's and multithread-protected.
        unsafe {
            mutex.ReleaseSync(ring::KEY_DRIVER)?;
            self.gpu.context.Flush();
        }
        let Some(image) = image? else {
            return Ok(None);
        };
        Ok(Some((self.finish(&meta, taken), image)))
    }
}

/// The fence transport is the default, and is limited to AMD's in-place compute path. A
/// capture without it and an old installed driver use the keyed-mutex transport.
fn ring_kind(caps: u32, requested: bool, zero_copy: bool) -> Kind {
    if requested && zero_copy && caps & CAP_FENCE_RING != 0 {
        Kind::Fence
    } else {
        Kind::KeyedMutex
    }
}

/// The transport a session opens with, and why a requested fence ring was not used. Turning
/// the fence ring off explicitly is a choice and never has a reason.
#[derive(Debug, PartialEq, Eq)]
struct RingChoice {
    kind: Kind,
    warning: Option<&'static str>,
}

fn choose_ring(caps: u32, requested: bool, rejected: bool, zero_copy: bool) -> RingChoice {
    let kind = ring_kind(caps, requested && !rejected, zero_copy);
    let warning = if !requested || kind == Kind::Fence {
        None
    } else if rejected {
        Some(
            "Using the keyed-mutex driver transport after GPU fences failed on this display. Recreate the display or restart the host to retry.",
        )
    } else if caps & CAP_FENCE_RING == 0 {
        Some(
            "The installed Rubylight display driver does not support the GPU fence transport, so capture uses the keyed-mutex ring. Update the display driver, or set idd_push_fence_ring = false to use the keyed-mutex ring without this warning.",
        )
    } else {
        Some(
            "The GPU fence transport needs AMD compute capture with in-place reads, so capture uses the keyed-mutex ring. Enable gpu_compute_conversion and idd_push_zero_copy, or set idd_push_fence_ring = false to use the keyed-mutex ring without this warning.",
        )
    };
    RingChoice { kind, warning }
}

impl PushedFrames for Session {
    type Image = GpuImage;

    fn take_newest(&mut self) -> Result<Option<(PushedFrame, GpuImage)>> {
        match self.ring.fences.clone() {
            Some(fences) => self.take_fenced(&fences),
            None => self.take_keyed(),
        }
    }
}

fn pixel_of(texture: &ID3D11Texture2D) -> Result<(D3D11_TEXTURE2D_DESC, Pixel)> {
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    // SAFETY: `texture` is a live texture and `desc` a live local.
    unsafe { texture.GetDesc(&mut desc) };
    let pixel = match desc.Format {
        DXGI_FORMAT_B8G8R8A8_UNORM => Pixel::Bgra8,
        DXGI_FORMAT_R16G16B16A16_FLOAT => Pixel::RgbaF16,
        format => bail!("unsupported Rubylight frame ring format {format:?}"),
    };
    Ok((desc, pixel))
}

fn in_place(
    gpu: &Device,
    texture: &ID3D11Texture2D,
    ready: crate::compute::Ready,
    lease: bridge::Lease,
) -> Result<GpuImage> {
    let (desc, pixel) = pixel_of(texture)?;
    let now = Instant::now();
    Ok(GpuImage {
        cursor: None,
        width: desc.Width,
        height: desc.Height,
        pixel,
        captured: now,
        wgc_stamp: None,
        acquired: now,
        published: now,
        gpu: gpu.clone(),
        texture: Arc::new(texture.clone()),
        ready: Some(ready),
        lease: Some(Arc::new(lease)),
        focus: None,
    })
}

/// An image of fence-ring texture `index`, read in place: the slot stays `Reading` until the
/// image's last clone drops `lent`. Compute work waits for `ready`, the driver's written fence.
fn lend_fenced(
    gpu: &Device,
    texture: &ID3D11Texture2D,
    index: usize,
    lent: Lent,
    ready: crate::compute::Ready,
) -> Result<GpuImage> {
    in_place(gpu, texture, ready, bridge::Lease::fenced(index, lent))
}

/// An image of keyed-mutex ring texture `index`, read in place: this process holds its keyed
/// mutex (taken with KEY_HOST) until the image's lease hands it back with KEY_DRIVER.
fn lend(
    gpu: &Device,
    slot: &Slot,
    mutex: &IDXGIKeyedMutex,
    index: usize,
    returns: Arc<Mutex<crate::compute::Returns>>,
) -> Result<GpuImage> {
    // Checked before the D3D11 signal, so an unsupported texture hands nothing out.
    pixel_of(&slot.texture)?;
    let ready = returns.lock().unwrap().acquired()?;
    in_place(
        gpu,
        &slot.texture,
        ready,
        bridge::Lease::ring(index, mutex.clone(), returns),
    )
}

impl Drop for Session {
    fn drop(&mut self) {
        // A direct reader error can close the capture before its next periodic check.
        // Preserve the rejection so that reopening cannot immediately retry this ring.
        if self.ring.fences.as_ref().is_some_and(|fences| {
            fences.failed.load(Ordering::Acquire) || fences.compute.is_stuck()
        }) {
            FENCE_FAILURES
                .lock()
                .unwrap()
                .failed(FenceScope::new(&self.monitor, self.adapter));
        }
        // The driver closes its duplicates; ours close with the ring. Only this ring is
        // cleared: one registered since (another process's, say) stays.
        let cleared = if self.control.info.caps & CAP_SCOPED_CLEAR != 0 {
            self.control
                .clear_frame_ring_generation(self.monitor.session_id, self.ring.layout.generation)
        } else {
            self.control.clear_frame_ring(self.monitor.session_id)
        };
        if let Err(error) = cleared {
            tracing::debug!(error = %format!("{error:#}"), "Rubylight frame ring was already gone");
        }
    }
}

// SAFETY: the D3D11 objects live on a multithread-protected device, the section view is used
// only through atomics, and every handle is owned; the session moves between threads with
// the capture, never shared.
unsafe impl Send for Session {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_capture_per_monitor_registers_a_ring() {
        // Ids no real monitor of the test process has.
        let first = Exclusive::claim(u64::MAX - 1).expect("free");
        assert!(
            Exclusive::claim(u64::MAX - 1).is_none(),
            "a second ring would take the first capture's frames"
        );
        let other = Exclusive::claim(u64::MAX - 2).expect("another monitor is free");
        drop(first);
        assert!(Exclusive::claim(u64::MAX - 1).is_some(), "released on drop");
        drop(other);
    }

    #[test]
    fn rings_that_keep_failing_before_a_frame_send_the_monitor_to_wgc() {
        let mut failures = Failures::default();
        let scope = FenceScope {
            session: 1,
            registration: 5,
            driver_pid: 40,
            adapter: (7, 0),
        };
        let other_gpu = FenceScope {
            adapter: (8, 0),
            ..scope
        };
        assert_eq!(failures.failed(scope), 1);
        assert_eq!(failures.excluded(scope), None, "the first failure retries");
        assert_eq!(failures.failed(scope), FAILURES_BEFORE_WGC);
        assert_eq!(failures.excluded(scope), Some(FAILURES_BEFORE_WGC));
        let other_monitor = FenceScope {
            session: 2,
            ..scope
        };
        assert_eq!(failures.excluded(other_monitor), None, "another monitor");
        assert_eq!(failures.excluded(other_gpu), None, "another GPU retries");
        // Failing on another GPU starts the count again.
        assert_eq!(failures.failed(other_gpu), 1);
        assert_eq!(failures.excluded(scope), None);
        failures.failed(other_gpu);
        assert!(failures.excluded(other_gpu).is_some());
        failures.delivered(1);
        assert_eq!(
            failures.excluded(other_gpu),
            None,
            "a delivered frame clears it"
        );
    }

    #[test]
    fn a_recreated_display_or_restarted_driver_gets_its_frame_ring_back() {
        // A virtual display's session id is its client's, the same every time it is created.
        let mut failures = Failures::default();
        let scope = FenceScope {
            session: 1,
            registration: 5,
            driver_pid: 40,
            adapter: (7, 0),
        };
        for _ in 0..FAILURES_BEFORE_WGC {
            failures.failed(scope);
        }
        assert!(failures.excluded(scope).is_some(), "reopening keeps WGC");
        for fresh in [
            FenceScope {
                registration: 6,
                ..scope
            },
            FenceScope {
                driver_pid: 41,
                ..scope
            },
        ] {
            assert_eq!(failures.excluded(fresh), None);
            // A failure there counts from one.
            let mut again = Failures(failures.0.clone());
            assert_eq!(again.failed(fresh), 1);
            assert_eq!(again.excluded(scope), None);
        }
    }

    #[test]
    fn idd_push_runs_when_asked_or_on_the_hosts_own_display_in_auto() {
        assert!(wanted("idd-push", "wgc", false));
        assert!(wanted("wgc", "auto", true));
        assert!(wanted("wgc", "", true));
        // A physical display, or a display another program created.
        assert!(!wanted("wgc", "auto", false));
        // An explicit WGC or DDX choice is kept.
        assert!(!wanted("wgc", "wgc", true));
        assert!(!wanted("ddx", "ddx", true));
    }

    #[test]
    fn the_ring_follows_the_composed_surface_once_known() {
        let mut status = MonitorStatus {
            width: 2560,
            height: 1440,
            refresh_mhz: 240_000,
            ..Default::default()
        };
        assert_eq!(ring_shape(&status, false), (2560, 1440, ring::FORMAT_BGRA8));
        assert_eq!(
            ring_shape(&status, true),
            (2560, 1440, ring::FORMAT_RGBA16F)
        );
        status.surface_width = 1920;
        status.surface_height = 1080;
        status.surface_format = ring::FORMAT_RGBA16F;
        assert_eq!(
            ring_shape(&status, false),
            (1920, 1080, ring::FORMAT_RGBA16F)
        );
        // A surface format the ring cannot carry falls back to the mode.
        status.surface_format = 24;
        assert_eq!(ring_shape(&status, false), (2560, 1440, ring::FORMAT_BGRA8));
    }

    #[test]
    fn a_shape_change_rebuilds_before_the_first_frame_and_reopens_after() {
        let fp16 = (1920, 1080, ring::FORMAT_RGBA16F);
        assert_eq!(
            verdict((ring::STATUS_FORMAT_MISMATCH, 10), fp16, false),
            Verdict::Reshape(1920, 1080, ring::FORMAT_RGBA16F)
        );
        assert!(matches!(
            verdict((ring::STATUS_SIZE_MISMATCH, 0), fp16, true),
            Verdict::Fail(_)
        ));
        assert!(matches!(
            verdict((ring::STATUS_SIZE_MISMATCH, 0), (0, 0, 0), false),
            Verdict::Fail(_)
        ));
        assert_eq!(
            verdict((ring::STATUS_STREAMING, 0), fp16, true),
            Verdict::Fine
        );
        assert_eq!(
            verdict((ring::STATUS_WAITING, 0), (0, 0, 0), false),
            Verdict::Fine
        );
        assert_eq!(
            verdict((ring::STATUS_RENEW_REQUIRED, 0), fp16, true),
            Verdict::Renew
        );
        for status in [
            ring::STATUS_OPEN_FAILED,
            ring::STATUS_WRONG_ADAPTER,
            ring::STATUS_DEVICE_LOST,
            99,
        ] {
            assert!(matches!(
                verdict((status, 0), fp16, false),
                Verdict::Fail(_)
            ));
        }
    }

    #[test]
    fn ring_generations_are_new_nonzero_and_fit_the_token() {
        assert_eq!(next_generation(0), 1);
        assert_eq!(next_generation(4), 5);
        let last = ring::Token::GENERATION_MASK;
        assert_eq!(next_generation(last - 1), last);
        assert_eq!(next_generation(last), 1, "wraps past 24 bits, never to 0");
        assert_eq!(next_generation(u32::MAX), 1 + u32::MAX % last);
        for previous in [0, 1, last - 1, last, u32::MAX] {
            let next = next_generation(previous);
            assert!((1..=last).contains(&next), "{previous} -> {next}");
        }
    }

    #[test]
    fn qpc_spans_are_zero_when_reversed_or_missing() {
        assert_eq!(qpc_micros(1_000, 1_500, 10_000_000), 50);
        assert_eq!(qpc_micros(1_500, 1_000, 10_000_000), 0);
        assert_eq!(qpc_micros(0, 1_000, 10_000_000), 0);
    }

    #[test]
    fn a_quiet_drain_loop_counts_only_past_the_limit() {
        let frequency = 10_000_000;
        let limit = Duration::from_secs(3);
        assert!(!stalled(0, 50_000_000, frequency, limit), "never stamped");
        assert!(!stalled(40_000_000, 50_000_000, frequency, limit));
        assert!(stalled(10_000_000, 50_000_000, frequency, limit));
        // A stamp ahead of this process's clock read is not a stall.
        assert!(!stalled(60_000_000, 50_000_000, frequency, limit));
    }

    #[test]
    fn fence_rings_require_the_request_capability_and_in_place_compute() {
        assert_eq!(
            ring_kind(CAP_FENCE_RING | CAP_SCOPED_CLEAR, true, true),
            Kind::Fence
        );
        for (caps, requested, zero_copy) in [
            (CAP_FENCE_RING, false, true),
            (CAP_FENCE_RING, true, false),
            (CAP_SCOPED_CLEAR, true, true),
            (0, true, true),
        ] {
            assert_eq!(ring_kind(caps, requested, zero_copy), Kind::KeyedMutex);
        }
    }

    #[test]
    fn the_fence_ring_is_the_default_and_an_explicit_false_selects_the_keyed_ring() {
        let caps = CAP_FENCE_RING | CAP_SCOPED_CLEAR;
        let configured = |text: &str| {
            let config = butterpollo_core::config::Config::parse(text).unwrap();
            choose_ring(caps, config.idd_push_fence_ring(), false, true)
        };
        let fence = RingChoice {
            kind: Kind::Fence,
            warning: None,
        };
        assert_eq!(configured(""), fence);
        assert_eq!(configured("idd_push_fence_ring = true"), fence);
        // Choosing the keyed ring is not a fault: no warning.
        assert_eq!(
            configured("idd_push_fence_ring = false"),
            RingChoice {
                kind: Kind::KeyedMutex,
                warning: None
            }
        );
    }

    #[test]
    fn a_fence_ring_that_cannot_be_used_stays_keyed_and_says_why() {
        let driver = choose_ring(CAP_SCOPED_CLEAR, true, false, true);
        assert_eq!(driver.kind, Kind::KeyedMutex);
        let driver = driver.warning.expect("an old driver is reported");
        assert!(driver.contains("Update the display driver"), "{driver}");
        assert!(driver.contains("idd_push_fence_ring = false"), "{driver}");
        let compute = choose_ring(CAP_FENCE_RING, true, false, false);
        assert_eq!(compute.kind, Kind::KeyedMutex);
        let compute = compute.warning.expect("in-place compute is reported");
        assert!(compute.contains("gpu_compute_conversion"), "{compute}");
        assert!(compute.contains("idd_push_fence_ring = false"), "{compute}");
        let failed = choose_ring(CAP_FENCE_RING, true, true, true);
        assert_eq!(failed.kind, Kind::KeyedMutex);
        assert!(failed.warning.unwrap().contains("fences failed"));
        // Without the request there is nothing to explain, whatever else is missing.
        assert_eq!(
            choose_ring(0, false, false, false),
            RingChoice {
                kind: Kind::KeyedMutex,
                warning: None
            }
        );
    }

    #[test]
    fn a_failed_fence_transport_stays_keyed_through_capture_reopen() {
        let monitor = Owned {
            session_id: 41,
            adapter: (3, 0),
            target: 2,
            wudf_pid: 55,
            listed: None,
            registration: 7,
        };
        let scope = FenceScope::new(&monitor, (9, 0));
        let mut failures = FenceFailures::default();
        failures.failed(scope);
        assert_eq!(
            ring_kind(CAP_FENCE_RING, !failures.excluded(scope), true),
            Kind::KeyedMutex,
            "reopening the same capture must not retry the rejected transport"
        );
        for changed in [
            FenceScope {
                registration: 8,
                ..scope
            },
            FenceScope {
                driver_pid: 56,
                ..scope
            },
            FenceScope {
                adapter: (10, 0),
                ..scope
            },
            FenceScope {
                session: 42,
                ..scope
            },
        ] {
            assert!(!failures.excluded(changed), "new owner/device can retry");
        }
        failures.clear(monitor.session_id);
        assert!(
            !failures.excluded(scope),
            "an explicit keyed capture resets the opt-in"
        );
    }

    #[test]
    fn remembered_fence_failures_are_bounded_and_keep_recent_rejections() {
        let mut failures = FenceFailures::default();
        for session in 0..=FenceFailures::LIMIT as u64 {
            failures.failed(FenceScope {
                session,
                registration: 1,
                driver_pid: 2,
                adapter: (3, 0),
            });
        }
        assert_eq!(failures.0.len(), FenceFailures::LIMIT);
        assert_eq!(failures.0[0].session, 1);
        let latest = *failures.0.last().unwrap();
        failures.failed(latest);
        assert_eq!(failures.0.len(), FenceFailures::LIMIT);
        assert!(failures.excluded(latest));
    }

    #[test]
    fn fence_startup_recovery_distinguishes_missing_pixels_from_failed_handoff() {
        let waiting = ring::STATUS_WAITING;
        let idle = Counters::default();
        let acquired = Counters {
            acquired: 1,
            ..idle
        };
        let check =
            |elapsed, counters| fence_recovery(waiting, false, elapsed, counters, false, false);
        assert_eq!(
            check(FENCE_HANDOFF_TIMEOUT, idle),
            None,
            "Windows may still be starting the display"
        );
        assert_eq!(
            check(FENCE_HANDOFF_TIMEOUT - Duration::from_millis(1), acquired),
            None
        );
        assert_eq!(
            check(FENCE_HANDOFF_TIMEOUT, acquired),
            Some(FenceRecovery::Keyed {
                reason: "could not publish acquired frames",
                remember: true
            })
        );
        assert_eq!(
            check(FIRST_FRAME_TIMEOUT, idle),
            Some(FenceRecovery::Keyed {
                reason: "did not deliver a frame",
                remember: false
            }),
            "no producer activity alone must not blacklist fences"
        );
        assert_eq!(
            check(
                FENCE_HANDOFF_TIMEOUT,
                Counters {
                    pushed: 1,
                    ..acquired
                }
            ),
            None,
            "give a newly published frame to the consumer"
        );
        assert_eq!(
            fence_recovery(
                ring::STATUS_SIZE_MISMATCH,
                false,
                FIRST_FRAME_TIMEOUT,
                acquired,
                false,
                false
            ),
            None,
            "shape/device recovery must not identify a fence incompatibility"
        );
    }

    #[test]
    fn failed_fence_ordering_and_stalled_queues_reopen_the_capture_device() {
        for (ordering_failed, queue_stuck, reason) in [
            (true, false, "lost GPU ordering"),
            (false, true, "compute queue cannot be reused"),
        ] {
            assert_eq!(
                fence_recovery(
                    ring::STATUS_STREAMING,
                    true,
                    Duration::ZERO,
                    Counters::default(),
                    ordering_failed,
                    queue_stuck
                ),
                Some(FenceRecovery::Reopen(reason)),
                "even a healthy driver heartbeat/device cannot make a stuck queue reusable"
            );
        }
        assert!(matches!(
            fence_recovery(
                ring::STATUS_OPEN_FAILED,
                false,
                Duration::ZERO,
                Counters::default(),
                false,
                false
            ),
            Some(FenceRecovery::Keyed { remember: true, .. })
        ));
        assert!(matches!(
            fence_recovery(
                ring::STATUS_OPEN_FAILED,
                true,
                Duration::ZERO,
                Counters::default(),
                false,
                false
            ),
            Some(FenceRecovery::Reopen(_))
        ));
    }
}

#[cfg(test)]
mod fence_tests;
