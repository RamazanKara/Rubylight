//! Capture from Rubylight's own virtual display driver (rust/drivers/rubylight-vdisplay).
//!
//! The driver copies every frame Windows composes for its monitor into textures this session
//! creates and shares with it, stamps the frame's present QPC beside it and sets an event
//! (`rubylight_vdisplay_proto::ring`). A frame is one GPU copy in the driver.
//!
//! The ring is a fence ring (the driver reports `CAP_FENCE_RING`): its textures and two fences
//! are D3D12 objects created on the capture GPU's compute device and shared by NT handle. The
//! driver's D3D12 copy queue writes a texture and signals the written fence with the frame's
//! sequence. By default the compute conversion reads the texture in place, waiting on the GPU
//! for that fence, and the compute queue signals the released fence after the image's last
//! read. With `idd_push_zero_copy = false` a compute queue of its own waits for the written
//! fence, copies the texture into a pool texture the host owns, and signals the released fence
//! after its copy.
//!
//! A driver without `CAP_FENCE_RING` is one an update installed that runs only after Windows
//! restarts; streams are refused until then ([`crate::display::DriverRestartRequired`]).
//! A fence fault ends the capture with an error; nothing falls back to another transport.

use super::*;
use crate::display::rubylight_vdd::{Control, Owned, Remote, owned_display};
use butterpollo_core::virtual_output::{PushedFrame, PushedFrames};
use rubylight_vdisplay_proto::control::{
    CAP_SCOPED_CLEAR, MonitorStatus, SetFenceRingRequest, SetFrameRingRequest,
};
use rubylight_vdisplay_proto::ring::{self, Consumer, Counters, Header, Layout, SlotMeta};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use windows::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
use windows::Win32::Graphics::Direct3D12::{
    D3D12_RESOURCE_DESC, D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET,
    D3D12_RESOURCE_FLAG_ALLOW_SIMULTANEOUS_ACCESS, ID3D12CommandQueue, ID3D12Fence,
};
use windows::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_ALL_ACCESS, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile,
    PAGE_READWRITE, UnmapViewOfFile,
};

/// Shapes a ring may be rebuilt to before the first frame, when the driver reports the
/// surface differs from the guess it was opened with.
const RESHAPES_BEFORE_FIRST_FRAME: u32 = 3;
/// The driver stamps its swap-chain loop every pass (at most 125 ms apart while idle). This
/// much silence after frames flowed means the loop stopped.
const DRAIN_STALL: Duration = Duration::from_secs(3);
/// How long a ring may wait for its first frame before the capture gives up.
const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(10);
/// A live producer that acquires frames but cannot publish through fences fails sooner. A
/// display Windows has not started composing keeps its full startup allowance above.
const FENCE_HANDOFF_TIMEOUT: Duration = Duration::from_secs(2);
/// How often the driver's frame counters are logged (`idd_push=debug`).
const COUNTERS_EVERY: Duration = Duration::from_secs(5);

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

/// What stopped the fence transport, if it did. Every fault ends the capture with an error:
/// there is no other transport to continue on. A fault before the first frame also counts as a
/// ring failure; one after it makes the capture worker reopen with a fresh capture device.
fn fence_fault(
    status: u32,
    started: bool,
    elapsed: Duration,
    counters: Counters,
    ordering_failed: bool,
    queue_stuck: bool,
) -> Option<&'static str> {
    if queue_stuck {
        return Some("compute queue cannot be reused");
    }
    if ordering_failed {
        return Some("lost GPU ordering");
    }
    if status == ring::STATUS_OPEN_FAILED {
        return Some(if started {
            "shared resources stopped opening"
        } else {
            "could not open shared resources"
        });
    }
    // Shape changes and device/adapter loss follow their existing recovery paths. Silence
    // alone is not a fence fault: Windows may not have supplied any pixels yet, and the
    // first-frame allowance in `check` covers it.
    if !started
        && matches!(status, ring::STATUS_WAITING | ring::STATUS_STREAMING)
        && elapsed >= FENCE_HANDOFF_TIMEOUT
        && counters.acquired > 0
        && counters.pushed == 0
    {
        return Some("could not publish acquired frames");
    }
    None
}

/// The fence transport stopped (see [`fence_fault`]). The capture worker reopens after it, with a
/// frozen picture until frames resume; [`FenceFaults`] ends the stream when that keeps failing.
#[derive(Debug)]
pub struct FenceFault(pub &'static str);
impl std::fmt::Display for FenceFault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "GPU fence transport {}; recreate the capture device",
            self.0
        )
    }
}
impl std::error::Error for FenceFault {}

/// Fence faults in a row on one display's capture before the stream ends. The first fault gets
/// one reopen with a fresh device; the second, with no frame delivered in between, is a driver
/// or GPU that keeps failing (the limit the ring had before it fell back to another capture).
pub const FENCE_FAULTS_BEFORE_STOP: u32 = 2;

/// Consecutive fence faults of one display's capture, ended by a delivered frame. Without the
/// count, a ring that keeps faulting reopens forever while the picture stays frozen.
#[derive(Debug, Default)]
pub struct FenceFaults(u32);
impl FenceFaults {
    /// A frame arrived: the transport works.
    pub fn delivered(&mut self) {
        self.0 = 0;
    }
    /// Count `error` if it is a fence fault. The error to end the stream with once the limit
    /// is reached, naming the driver and how to repair it. A device loss (a TDR, a driver
    /// reset) is not counted whatever it wraps: the device-loss recovery handles it, and it
    /// starts the count again, as a fault before it says nothing about the recreated device.
    pub fn fault(&mut self, error: &anyhow::Error) -> Option<anyhow::Error> {
        if crate::device_loss::DeviceLost::from_error(error).is_some() {
            self.0 = 0;
            return None;
        }
        let fault = error.downcast_ref::<FenceFault>()?;
        self.0 += 1;
        (self.0 >= FENCE_FAULTS_BEFORE_STOP).then(|| {
            anyhow::anyhow!(
                "Rubylight's display driver keeps failing the GPU fence transport ({}; {} times with no frame in between). Restart Windows; if this persists, run Rubylight setup again to repair the display driver.",
                fault.0,
                self.0
            )
        })
    }
}

/// Microseconds from QPC `from` to `to`, zero when `to` is earlier or either is missing.
fn qpc_micros(from: u64, to: u64, frequency: i64) -> u64 {
    if from == 0 || to <= from || frequency <= 0 {
        return 0;
    }
    ((to - from) as u128 * 1_000_000 / frequency as u128) as u64
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
                // A size or colour-space change reopens the capture, so the encoder is
                // rebuilt for it.
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
    texture: Arc<windows::Win32::Graphics::Direct3D12::ID3D12Resource>,
    shared: OwnedHandle,
}

/// A fence ring's fences, and how its slots are freed. The session and every image lent from
/// the ring hold it, so a slot is freed even after the session closed.
pub(super) struct Fences {
    section: Arc<Section>,
    compute: Arc<crate::compute::Compute>,
    /// A failed ordering operation quarantines this ring. Never free a slot whose GPU
    /// readers may not be covered by its completion fence.
    failed: AtomicBool,
    /// The driver signals it with a frame's sequence once its copy is done.
    written: ID3D12Fence,
    /// Signalled with each slot's release value after the host's last read of the slot.
    released: ID3D12Fence,
    /// For the driver: `[written, released]`.
    handles: [OwnedHandle; 2],
    /// The queue that reads the ring and signals the released fence: the compute queue when
    /// the ring is read in place, else the copies' queue. Always the same one, so the values
    /// reach the fence in the order they were handed out, each after the reads before it.
    queue: ID3D12CommandQueue,
    /// The last release value handed out.
    value: Mutex<u64>,
}

impl Fences {
    fn fail_ordering(&self) {
        self.failed.store(true, Ordering::Release);
        if self.queue == self.compute.queue {
            // Earlier readers may have queued a written-fence wait whose matching signal is now
            // uncertain. A fresh capture device alone cannot retire it.
            self.compute.retire();
        }
    }

    fn new(
        compute: Arc<crate::compute::Compute>,
        section: Arc<Section>,
        queue: ID3D12CommandQueue,
    ) -> Result<Self> {
        let (written, written_handle) = compute.share_fence()?;
        let (released, released_handle) = compute.share_fence()?;
        Ok(Self {
            section,
            compute,
            failed: AtomicBool::new(false),
            written,
            released,
            handles: [written_handle, released_handle],
            queue,
            value: Mutex::new(0),
        })
    }

    /// Free `slot`, claimed for frame `seq`, behind every read of it submitted so far on the
    /// ring's queue.
    fn free(&self, slot: usize, seq: u32) -> Result<()> {
        let mut value = self.value.lock().unwrap();
        anyhow::ensure!(
            !self.failed.load(Ordering::Acquire),
            "frame ring ordering failed"
        );
        *value += 1;
        let release = *value;
        // SAFETY: the released fence and the queue are on the same D3D12 device and
        // free-threaded.
        let signalled = unsafe { self.queue.Signal(&self.released, release) };
        // A failed signal does not prove the reads completed. Keep the slot owned until
        // Session retires this ring; a later signal must not make an unsafe slot reusable.
        if let Err(error) = signalled {
            self.fail_ordering();
            return Err(error).context("free a Rubylight frame ring slot");
        }
        let freed = Consumer::release(&self.section.header(), slot, seq, release);
        drop(value);
        if !freed {
            self.fail_ordering();
        }
        anyhow::ensure!(
            freed,
            "Rubylight frame ring slot {slot} was not the host's to free"
        );
        Ok(())
    }

    /// When the driver's copy of frame `seq` is complete.
    fn written(&self, seq: u32) -> crate::compute::Ready {
        crate::compute::Ready::on(&self.compute, self.written.clone(), u64::from(seq))
    }
}

/// A fence-ring slot an image reads in place. The slot is freed when this drops, with the
/// image's last clone.
pub(crate) struct Lent {
    fences: Arc<Fences>,
    slot: usize,
    seq: u32,
}

impl Lent {
    /// A bounded reader wait failed. Retain this slot and let capture reopen
    /// without treating the unrelated compute queue as broken.
    pub(super) fn reject(&self) {
        self.fences.failed.store(true, Ordering::Release);
    }
}

impl Drop for Lent {
    fn drop(&mut self) {
        if let Err(error) = self.fences.free(self.slot, self.seq) {
            tracing::debug!(error = %format!("{error:#}"), slot = self.slot, "Rubylight frame ring slot retained after a GPU ordering error");
        }
    }
}

/// One fence ring: its header, textures and fences. The driver holds duplicates of every
/// handle until the ring is replaced or cleared.
struct Ring {
    section: Arc<Section>,
    slots: Vec<Slot>,
    layout: Layout,
    fences: Arc<Fences>,
}

impl Ring {
    /// A fence ring on the compute device: textures the driver copies into and the compute
    /// queue reads, and fences whose released value `queue` signals.
    fn fenced(
        compute: Arc<crate::compute::Compute>,
        layout: Layout,
        queue: ID3D12CommandQueue,
    ) -> Result<Self> {
        let section = Arc::new(Section::new()?);
        section.header().init(layout);
        let mut slots = Vec::with_capacity(layout.slot_count as usize);
        for _ in 0..layout.slot_count {
            // The driver still opens the texture on a D3D11 device, which accepts a D3D12 texture
            // only if it may be a render target: without the flag OpenSharedResource1 fails with
            // E_INVALIDARG (RX 7900 XT, October 16). The host's readers and a driver seed copy
            // may read simultaneously on different devices, and the fences order the writers,
            // which does not itself permit concurrent D3D12 access. Revisit both flags once the
            // D3D12 driver is required.
            let (texture, shared) = compute.share_texture(
                layout.width,
                layout.height,
                DXGI_FORMAT(layout.format as i32),
                D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET
                    | D3D12_RESOURCE_FLAG_ALLOW_SIMULTANEOUS_ACCESS,
            )?;
            slots.push(Slot {
                texture: Arc::new(texture),
                shared,
            });
        }
        let fences = Fences::new(compute, section.clone(), queue)?;
        Ok(Self {
            section,
            slots,
            layout,
            fences: Arc::new(fences),
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
    consumer: Consumer,
    /// Pool textures the ring is copied into (`idd_push_zero_copy = false`); None when the
    /// compute conversion reads ring textures in place.
    pool: Option<GpuPool>,
    frame: Arc<crate::timing::Signal>,
    staging: Option<crate::compute::Staging>,
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
    ) -> Result<Self> {
        let monitor = owned_display(name)?.with_context(|| {
            format!("{name} is not a display this host created with the Rubylight display driver")
        })?;
        let exclusive = Exclusive::claim(monitor.session_id).with_context(|| {
            format!("{name} is already captured by another capture of this host")
        })?;
        let control = Control::open()?;
        crate::display::fence_ring(control.info.caps)?;
        let status = control.query(monitor.session_id)?;
        // Before the compute queue is made: it takes its priority from this setting once.
        if let Err(error) = crate::gpu_priority::configure(config) {
            tracing::debug!(%error, "GPU priority remains at the process default");
        }
        let gpu = Device::for_display(
            name,
            config.get("adapter_name", ""),
            config.get("adapter_pnp_id", ""),
        )?;
        let adapter = gpu.luid();
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
        let remote = Remote::open(monitor.wudf_pid)?;
        let zero_copy = config.boolean("idd_push_zero_copy", true);
        let pool = if zero_copy {
            None
        } else {
            Some(GpuPool::new(gpu.compute.clone()).context("compute copies of the frame ring")?)
        };
        // Before the first frame the driver may not know the surface yet: guess from the
        // display's colour state, and rebuild once it reports the real one.
        let native_hdr = hdr
            || gpu
                .color_description()?
                .is_some_and(|d| d.color_space == DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020);
        let (width, height, format) = ring_shape(&status, native_hdr);
        let layout = Layout {
            slot_count: ring::DEFAULT_SLOTS as u32,
            width,
            height,
            format,
            generation: next_generation(status.ring_generation),
        };
        let frame = Arc::new(crate::timing::Signal::new()?);
        let queue = release_queue(&gpu, pool.as_ref());
        let ring = Ring::fenced(gpu.compute.clone(), layout, queue)?;
        let now = Instant::now();
        let mut session = Self {
            gpu,
            control,
            monitor,
            remote,
            adapter,
            consumer: Consumer::new(layout.generation, layout.slot_count),
            ring,
            pool,
            frame,
            staging: None,
            started: false,
            reshapes: 0,
            opened: now,
            next_check: now,
            counted: (now, Counters::default()),
            _exclusive: exclusive,
        };
        session.deliver()?;
        tracing::info!(
            output = %name,
            width,
            height,
            format,
            target = session.monitor.target,
            in_place = zero_copy,
            compute_priority = session.gpu.compute.priority,
            "capturing from the Rubylight display driver's frame push"
        );
        Ok(session)
    }

    /// Hand the current ring to the driver.
    fn deliver(&mut self) -> Result<()> {
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
        let [written, released] = &self.ring.fences.handles;
        let request = SetFenceRingRequest {
            ring: request,
            written_fence: delivery.send(HANDLE(written.as_raw_handle()))?,
            released_fence: delivery.send(HANDLE(released.as_raw_handle()))?,
        };
        request
            .validate()
            .map_err(|e| anyhow::anyhow!("frame ring request: {e}"))?;
        self.control
            .set_fence_ring(&request)
            .context("the Rubylight display driver refused the frame ring")?;
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
        let queue = release_queue(&self.gpu, self.pool.as_ref());
        self.ring = Ring::fenced(self.gpu.compute.clone(), layout, queue)?;
        self.consumer = Consumer::new(layout.generation, layout.slot_count);
        self.opened = Instant::now();
        self.deliver()
    }

    fn check(&mut self) -> Result<()> {
        if let Some(loss) = crate::device_loss::DeviceLost::d3d12(&self.gpu.compute.device) {
            return Err(loss.into());
        }
        let header = self.ring.section.header();
        let fences = &self.ring.fences;
        if let Some(fault) = fence_fault(
            header.status().0,
            self.started,
            self.opened.elapsed(),
            header.counters(),
            fences.failed.load(Ordering::Acquire),
            fences.compute.is_stuck(),
        ) {
            return Err(FenceFault(fault).into());
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
            self.check()?;
        }
        let Some((frame, mut image)) = self.take_newest()? else {
            return Ok(None);
        };
        (image.captured, image.present_stamp) =
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
    /// copied, the copy queue waits for that copy, copies and frees the slot behind its copy
    /// at once.
    fn take_fenced(&mut self, fences: &Arc<Fences>) -> Result<Option<(PushedFrame, GpuImage)>> {
        if self.pool.as_ref().is_some_and(|pool| !pool.has_capacity()) {
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
        let Some(pool) = &mut self.pool else {
            let lent = Lent {
                fences: fences.clone(),
                slot: index,
                seq,
            };
            let image = lend_fenced(&self.gpu, &slot.texture, lent, fences.written(seq))?;
            return Ok(Some((self.finish(&meta, taken), image)));
        };
        let copied = pool.copy(&self.gpu, &slot.texture, Some(&fences.written(seq)));
        // The copy is queued on the queue that signals the released fence, so freeing now
        // cannot let the driver overwrite pixels still being read.
        let freed = fences.free(index, seq);
        let image = copied?;
        freed?;
        let Some(image) = image else {
            return Ok(None);
        };
        Ok(Some((self.finish(&meta, taken), image)))
    }
}

/// The queue that signals a ring's released fence: the copies' queue when the ring is copied,
/// else the compute queue the conversion reads it on.
fn release_queue(gpu: &Device, pool: Option<&GpuPool>) -> ID3D12CommandQueue {
    match pool {
        Some(pool) => pool.copier.queue().clone(),
        None => gpu.compute.queue.clone(),
    }
}

impl PushedFrames for Session {
    type Image = GpuImage;

    fn take_newest(&mut self) -> Result<Option<(PushedFrame, GpuImage)>> {
        let fences = self.ring.fences.clone();
        self.take_fenced(&fences)
    }
}

fn pixel_of(
    texture: &windows::Win32::Graphics::Direct3D12::ID3D12Resource,
) -> Result<(D3D12_RESOURCE_DESC, Pixel)> {
    // SAFETY: `texture` is a live resource; GetDesc only reads it.
    let desc = unsafe { texture.GetDesc() };
    let pixel = match desc.Format {
        DXGI_FORMAT_B8G8R8A8_UNORM => Pixel::Bgra8,
        DXGI_FORMAT_R16G16B16A16_FLOAT => Pixel::RgbaF16,
        format => bail!("unsupported Rubylight frame ring format {format:?}"),
    };
    Ok((desc, pixel))
}

/// An image of a fence-ring texture, read in place: the slot stays `Reading` until the image's
/// last clone drops `lent`. GPU readers wait for `ready`, the driver's written fence.
fn lend_fenced(
    gpu: &Device,
    texture: &Arc<windows::Win32::Graphics::Direct3D12::ID3D12Resource>,
    lent: Lent,
    ready: crate::compute::Ready,
) -> Result<GpuImage> {
    let (desc, pixel) = pixel_of(texture)?;
    let now = Instant::now();
    Ok(GpuImage {
        width: u32::try_from(desc.Width)?,
        height: desc.Height,
        pixel,
        captured: now,
        present_stamp: None,
        acquired: now,
        published: now,
        gpu: gpu.clone(),
        texture: texture.clone(),
        ready: Some(ready),
        lease: Some(Arc::new(lent)),
    })
}

impl Drop for Session {
    fn drop(&mut self) {
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
    fn a_ring_that_keeps_faulting_ends_the_stream_and_a_frame_resets_the_count() {
        let fault = || anyhow::Error::from(FenceFault("lost GPU ordering"));
        let mut faults = FenceFaults::default();
        // One fault is reopened; a delivered frame forgives it, however often that repeats.
        for _ in 0..10 {
            assert!(faults.fault(&fault()).is_none());
            faults.delivered();
        }
        // A device loss wrapping a fence fault is not counted, and it forgives the faults before it.
        let lost = || {
            fault().context(crate::device_loss::DeviceLost(
                windows::Win32::Graphics::Dxgi::DXGI_ERROR_DEVICE_REMOVED.0,
            ))
        };
        for _ in 0..10 {
            assert!(faults.fault(&lost()).is_none());
        }
        assert!(faults.fault(&fault()).is_none());
        assert!(faults.fault(&lost()).is_none());
        for _ in 1..FENCE_FAULTS_BEFORE_STOP {
            assert!(faults.fault(&fault()).is_none(), "the count started again");
        }
        faults.delivered();
        // Other errors neither count nor end anything.
        assert!(
            faults
                .fault(&anyhow::anyhow!("capture interrupted"))
                .is_none()
        );
        for _ in 1..FENCE_FAULTS_BEFORE_STOP {
            assert!(faults.fault(&fault()).is_none());
        }
        let ended = faults.fault(&fault()).expect("the limit ends the stream");
        let ended = format!("{ended:#}");
        assert!(ended.contains("lost GPU ordering"), "{ended}");
        assert!(
            ended.contains("run Rubylight setup again to repair the display driver"),
            "{ended}"
        );
        const {
            assert!(
                FENCE_FAULTS_BEFORE_STOP >= 2,
                "the first fault must get its reopen"
            )
        };
    }

    #[test]
    fn a_fence_fault_ends_the_capture_instead_of_changing_transport() {
        let healthy = Counters::default();
        for (ordering_failed, queue_stuck, reason) in [
            (true, false, "lost GPU ordering"),
            (false, true, "compute queue cannot be reused"),
        ] {
            assert_eq!(
                fence_fault(
                    ring::STATUS_STREAMING,
                    true,
                    Duration::ZERO,
                    healthy,
                    ordering_failed,
                    queue_stuck
                ),
                Some(reason),
                "even a healthy driver heartbeat/device cannot make a stuck queue reusable"
            );
        }
        for (started, reason) in [
            (false, "could not open shared resources"),
            (true, "shared resources stopped opening"),
        ] {
            assert_eq!(
                fence_fault(
                    ring::STATUS_OPEN_FAILED,
                    started,
                    Duration::ZERO,
                    healthy,
                    false,
                    false
                ),
                Some(reason)
            );
        }
    }

    #[test]
    fn fence_startup_distinguishes_missing_pixels_from_failed_handoff() {
        let acquired = Counters {
            acquired: 1,
            ..Default::default()
        };
        let check = |elapsed, counters| {
            fence_fault(
                ring::STATUS_STREAMING,
                false,
                elapsed,
                counters,
                false,
                false,
            )
        };
        assert_eq!(
            check(FENCE_HANDOFF_TIMEOUT - Duration::from_millis(1), acquired),
            None
        );
        assert_eq!(
            check(FENCE_HANDOFF_TIMEOUT, acquired),
            Some("could not publish acquired frames")
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
            "a published frame is a working handoff"
        );
        assert_eq!(
            check(FIRST_FRAME_TIMEOUT, Counters::default()),
            None,
            "Windows may not have supplied pixels yet; the first-frame timeout reports it"
        );
        for status in [
            ring::STATUS_SIZE_MISMATCH,
            ring::STATUS_FORMAT_MISMATCH,
            ring::STATUS_DEVICE_LOST,
        ] {
            assert_eq!(
                fence_fault(status, false, FIRST_FRAME_TIMEOUT, acquired, false, false),
                None,
                "shape/device recovery must not identify a fence fault"
            );
        }
    }
}

#[cfg(test)]
mod fence_tests;
