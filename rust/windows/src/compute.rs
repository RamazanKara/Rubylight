//! The host's GPU work, all on D3D12 compute queues of the capture GPU.
//!
//! Work on the graphics engine waits behind whatever a game has queued there:
//! beside a game using the whole GPU, a graphics-queue colour conversion took
//! 7 ms instead of 0.9 ms and a plain copy 8 ms instead of 0.7 ms. A compute
//! queue runs next to the graphics engine and finished the same work in
//! 0.2-0.3 ms under that load (`examples/d3d12_probe.rs`, `examples/gpu_load.rs`).
//!
//! Every frame stays on D3D12 from the display driver to the encoder. The
//! driver's copy queue writes a frame ring texture and signals its written
//! fence; [`Converter`] waits for that fence on the GPU, converts the texture
//! to the encoder's YUV format and signals the encoder's fence. A ring that is
//! copied rather than read in place (`idd_push_zero_copy = false`) is copied
//! into a texture the host owns on a compute queue of its own ([`Copier`]).
//! CPU images (codec probes, tools) are uploaded with [`upload`] and read back
//! with [`read_planes`].
//!
//! Resources live in `D3D12_RESOURCE_STATE_COMMON` between command lists:
//! sources are read through implicit promotion (which decays again at the end
//! of the command list), and anything a list writes is transitioned back to
//! COMMON before the list ends.

use anyhow::{Context, Result, bail};
use std::{
    collections::HashMap,
    os::windows::io::{FromRawHandle, OwnedHandle},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};
use windows::{
    Win32::{
        Foundation::{CloseHandle, GENERIC_ALL, LUID},
        Graphics::{
            Direct3D::D3D_FEATURE_LEVEL_11_0,
            Direct3D12::*,
            Dxgi::{Common::*, *},
        },
    },
    core::{IUnknown, Interface, PCWSTR},
};

include!(concat!(env!("OUT_DIR"), "/shader_bytecode.rs"));

/// A D3D12 device on the capture GPU with one compute queue and its fence.
pub struct Compute {
    pub device: ID3D12Device,
    pub queue: ID3D12CommandQueue,
    pub fence: ID3D12Fence,
    next: Mutex<u64>,
    pub priority: i32,
    luid: LUID,
    /// Set when the queue cannot safely be reused after an ordering error or timeout.
    stuck: AtomicBool,
}

/// Resources whose last GPU access could not be proved complete. In particular,
/// an ExecuteCommandLists followed by a failed Signal has no usable old fence
/// value. A fresh tail marker covers that work too; if even that signal fails,
/// only device removal makes releasing its references safe.
struct RetainedWork {
    device: ID3D12Device,
    adapter: (u32, i32),
    tail: Option<ID3D12Fence>,
    waits: Vec<(ID3D12Fence, u64)>,
    objects: Vec<IUnknown>,
    captures: Vec<crate::capture::GpuImage>,
}
// SAFETY: The erased interfaces are D3D12 objects or fences; they are only
// retained/released, never used via IUnknown.
unsafe impl Send for RetainedWork {}

static RETAINED: Mutex<Vec<RetainedWork>> = Mutex::new(Vec::new());

impl RetainedWork {
    fn new(compute: &Compute, queue: &ID3D12CommandQueue) -> Self {
        // SAFETY: Both objects are live and on the same device. This final
        // marker submits no new resource access, including on a retired queue.
        let (luid, tail) = unsafe {
            (
                compute.device.GetAdapterLuid(),
                compute
                    .device
                    .CreateFence::<ID3D12Fence>(0, D3D12_FENCE_FLAG_NONE)
                    .and_then(|fence| queue.Signal(&fence, 1).map(|()| fence))
                    .ok(),
            )
        };
        let mut work = Self {
            device: compute.device.clone(),
            adapter: (luid.LowPart, luid.HighPart),
            tail,
            waits: Vec::new(),
            objects: Vec::new(),
            captures: Vec::new(),
        };
        work.keep(queue);
        work.keep(&compute.fence);
        work
    }

    fn keep(&mut self, object: &impl Interface) {
        // SAFETY: Every caller supplies a COM D3D12 interface, whose vtable
        // starts with IUnknown. Transfer a cloned reference without a
        // fallible QueryInterface call during failure cleanup; this erased
        // reference is only retained and released, never used as a GPU object.
        self.objects
            .push(unsafe { IUnknown::from_raw(object.clone().into_raw()) });
    }

    fn complete(&self) -> bool {
        if crate::device_loss::DeviceLost::d3d12(&self.device).is_some() {
            return true;
        }
        // SAFETY: Every queried fence is retained by this bundle. Device
        // removal is handled above, not mistaken for a completed UINT64_MAX.
        unsafe {
            self.tail.as_ref().is_some_and(|fence| {
                completed_value(fence.GetCompletedValue()).is_ok_and(|done| done >= 1)
            }) && self.waits.iter().all(|(fence, value)| {
                completed_value(fence.GetCompletedValue()).is_ok_and(|done| done >= *value)
            })
        }
    }

    fn finish(self, compute: &Compute) {
        if let Some(fence) = &self.tail {
            let _ = wait_fence(fence, 1);
        }
        if !self.complete() {
            compute.retire();
            tracing::warn!(
                retained_objects = self.objects.len(),
                "retaining unfinished GPU resources; compute recovery waits for this adapter to drain"
            );
            RETAINED.lock().unwrap().push(self);
        }
    }
}

/// Do not accumulate a fresh set of video-memory allocations on every retry
/// behind a permanently hung queue. Existing users have bounded pools; no new
/// compute generation is admitted on this adapter until its retained work ends.
fn allow_compute_generation(luid: LUID) -> Result<()> {
    let (complete, blocked) = {
        let mut retained = RETAINED.lock().unwrap();
        let mut complete = Vec::new();
        let mut index = 0;
        while index < retained.len() {
            if retained[index].complete() {
                complete.push(retained.swap_remove(index));
            } else {
                index += 1;
            }
        }
        let blocked = retained
            .iter()
            .any(|work| work.adapter == (luid.LowPart, luid.HighPart));
        (complete, blocked)
    };
    // Releasing a capture lease can run its own teardown; never do it under
    // the retention lock.
    drop(complete);
    anyhow::ensure!(
        !blocked,
        "unfinished GPU resources on this adapter; compute conversion is unavailable until they drain"
    );
    Ok(())
}
fn completed_value(value: u64) -> Result<u64> {
    // D3D12 signals UINT64_MAX on device removal, including shared fences.
    if value == u64::MAX {
        return Err(crate::device_loss::DeviceLost(DXGI_ERROR_DEVICE_REMOVED.0).into());
    }
    Ok(value)
}
/// AMF owns these states while encoding, but the next transition runs on a
/// compute command list. Never pass a video/graphics-only state to that queue.
/// See DirectX-Specs/CPUEfficiency, "State support by command-list type".
fn ensure_compute_texture_state(state: D3D12_RESOURCE_STATES) -> Result<()> {
    let read = D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE | D3D12_RESOURCE_STATE_COPY_SOURCE;
    let valid = state.0 & !read.0 == 0
        || state == D3D12_RESOURCE_STATE_UNORDERED_ACCESS
        || state == D3D12_RESOURCE_STATE_COPY_DEST;
    anyhow::ensure!(
        valid,
        "AMF input resource state {:#x} cannot transition on the compute queue",
        state.0 as u32
    );
    Ok(())
}
fn ensure_queue_active(retired: &AtomicBool) -> Result<()> {
    anyhow::ensure!(
        !retired.load(Ordering::Acquire),
        "shared compute queue is retired; reopen capture"
    );
    Ok(())
}
/// Wait on the CPU until `fence` reaches `value`, for at most two seconds.
/// The copies wait for fences another device signals; after that device is
/// lost they may never complete, and an unbounded wait froze the stream for
/// good with no error to recover from.
fn wait_fence(fence: &ID3D12Fence, value: u64) -> Result<()> {
    use windows::Win32::System::Threading::*;
    // SAFETY: `fence` is a live D3D12 fence, and the event is created here, used only by
    // SetEventOnCompletion and the wait, and closed once.
    let result = (|| unsafe {
        if completed_value(fence.GetCompletedValue())? >= value {
            return Ok(());
        }
        let event = CreateEventW(None, false, false, PCWSTR::null())?;
        let waited = fence.SetEventOnCompletion(value, event).and_then(|()| {
            let waited = WaitForSingleObject(event, 2000);
            if waited == windows::Win32::Foundation::WAIT_FAILED {
                Err(windows::core::Error::from_thread())
            } else {
                Ok(waited)
            }
        });
        let _ = CloseHandle(event);
        waited?;
        // Device removal wakes the event too; check the fence after any wake.
        if completed_value(fence.GetCompletedValue())? < value {
            bail!("GPU work did not finish within two seconds");
        }
        Ok(())
    })();
    result.map_err(|error| {
        let mut device: Option<ID3D12Device> = None;
        // SAFETY: The fence owns its device and the out-parameter is a live local.
        let _ = unsafe { fence.GetDevice(&mut device) };
        match device
            .as_ref()
            .and_then(crate::device_loss::DeviceLost::d3d12)
        {
            Some(loss) => error.context(loss),
            None => error,
        }
    })
}
/// Whether the copy and conversion queues ask for global realtime priority
/// (`compute_queue_realtime`). Off by default: a realtime queue pre-empts the
/// desktop compositor that every captured frame comes from, and an HDR stream
/// on an RX 7900 XT with it ended in an AMD engine timeout the driver could
/// not reset. High priority still runs ahead of a game's normal work.
static REALTIME: AtomicBool = AtomicBool::new(false);
pub fn set_realtime(enabled: bool) {
    REALTIME.store(enabled, Ordering::Relaxed);
}
pub fn realtime() -> bool {
    REALTIME.load(Ordering::Relaxed)
}
/// A compute queue at the highest allowed priority Windows grants this process.
fn compute_queue(device: &ID3D12Device) -> Result<(ID3D12CommandQueue, i32)> {
    let realtime = REALTIME.load(Ordering::Relaxed);
    for priority in [
        D3D12_COMMAND_QUEUE_PRIORITY_GLOBAL_REALTIME.0,
        D3D12_COMMAND_QUEUE_PRIORITY_HIGH.0,
        D3D12_COMMAND_QUEUE_PRIORITY_NORMAL.0,
    ]
    .into_iter()
    .filter(|priority| realtime || *priority != D3D12_COMMAND_QUEUE_PRIORITY_GLOBAL_REALTIME.0)
    {
        // SAFETY: `device` is a live D3D12 device and the queue description outlives the call.
        if let Ok(queue) = unsafe {
            device.CreateCommandQueue::<ID3D12CommandQueue>(&D3D12_COMMAND_QUEUE_DESC {
                Type: D3D12_COMMAND_LIST_TYPE_COMPUTE,
                Priority: priority,
                Flags: D3D12_COMMAND_QUEUE_FLAG_NONE,
                NodeMask: 0,
            })
        } {
            return Ok((queue, priority));
        }
    }
    bail!("no D3D12 compute queue")
}
impl Compute {
    /// A compute queue on the adapter with `luid`.
    pub fn new(luid: LUID) -> Result<Arc<Self>> {
        allow_compute_generation(luid)?;
        // SAFETY: `adapter` comes from a live DXGI factory, the device out-parameter is a live
        // local checked for None, and the queue and fence are created on that device.
        unsafe {
            let factory: IDXGIFactory4 = CreateDXGIFactory1()?;
            let adapter: IDXGIAdapter1 = factory.EnumAdapterByLuid(luid)?;
            let mut device: Option<ID3D12Device> = None;
            D3D12CreateDevice(&adapter, D3D_FEATURE_LEVEL_11_0, &mut device)
                .context("D3D12 device on the capture GPU")?;
            let device = device.context("no D3D12 device")?;
            let (queue, priority) = compute_queue(&device)?;
            let fence = device.CreateFence(0, D3D12_FENCE_FLAG_NONE)?;
            tracing::info!(priority, "compute queue ready for colour conversion");
            Ok(Arc::new(Self {
                device,
                queue,
                fence,
                next: Mutex::new(0),
                priority,
                luid,
                stuck: AtomicBool::new(false),
            }))
        }
    }
    /// The compute queue for the adapter with `luid`, shared by the capture and the encoders
    /// on that GPU. One that stopped finishing work is replaced.
    pub fn for_adapter(luid: LUID) -> Result<Arc<Self>> {
        type Shared = Vec<((u32, i32), std::sync::Weak<Compute>)>;
        static SHARED: Mutex<Shared> = Mutex::new(Vec::new());
        let key = (luid.LowPart, luid.HighPart);
        let mut shared = SHARED.lock().unwrap();
        shared.retain(|(_, compute)| compute.strong_count() > 0);
        if let Some(compute) = shared
            .iter()
            .filter(|(owner, _)| *owner == key)
            .filter_map(|(_, compute)| compute.upgrade())
            .find(|compute| !compute.stuck.load(Ordering::Acquire) && compute.completed(0))
        {
            return Ok(compute);
        }
        let compute = Self::new(luid)?;
        shared.push((key, Arc::downgrade(&compute)));
        Ok(compute)
    }
    /// The adapter this device runs on.
    pub fn luid(&self) -> LUID {
        self.luid
    }
    /// Signal the fence after the work submitted so far; returns its value.
    #[cfg(test)]
    fn signal(&self) -> Result<u64> {
        let mut next = self.next.lock().unwrap();
        // Advance only once the signal is queued: a value nothing will ever
        // signal would hold every later wait on it.
        // SAFETY: `queue` and `fence` were created on the same D3D12 device and live as long as
        // `self`.
        unsafe { self.queue.Signal(&self.fence, *next + 1) }.inspect_err(|_| self.retire())?;
        *next += 1;
        Ok(*next)
    }
    /// Run a closed command list; returns the fence value that follows it.
    pub(crate) fn execute(&self, list: &ID3D12GraphicsCommandList) -> Result<u64> {
        let mut next = self.next.lock().unwrap();
        self.ensure_active()?;
        // SAFETY: Callers pass a closed compute command list recorded on `self.device`, the queue's
        // device, and keep what it references alive until the returned fence value.
        unsafe {
            self.queue
                .ExecuteCommandLists(&[Some(list.cast::<ID3D12CommandList>()?)]);
        }
        // Keep execution and its monotonically increasing completion signal
        // together even when several encoders share this queue.
        // SAFETY: The queue and fence belong to this live device.
        unsafe { self.queue.Signal(&self.fence, *next + 1) }.inspect_err(|_| self.retire())?;
        *next += 1;
        Ok(*next)
    }
    pub fn completed(&self, value: u64) -> bool {
        // SAFETY: `fence` is a live D3D12 fence owned by `self`; GetCompletedValue only reads it.
        match completed_value(unsafe { self.fence.GetCompletedValue() }) {
            Ok(done) => done >= value,
            Err(_) => {
                self.stuck.store(true, Ordering::Release);
                false
            }
        }
    }
    /// A wait or enqueue failed even if the device still reports itself healthy. Captures
    /// must reopen before submitting more work behind a queue with uncertain ordering.
    pub(crate) fn is_stuck(&self) -> bool {
        self.stuck.load(Ordering::Acquire)
    }
    /// Reject new work after an ordering failure, even when old fence values
    /// have completed. Completion alone does not make a retired queue reusable.
    /// This does not cancel calls already in progress; their resource ownership
    /// and completion fences still apply, and retirement must never wait on them.
    fn ensure_active(&self) -> Result<()> {
        ensure_queue_active(&self.stuck)
    }
    /// Do not give this queue to a new capture or encoder. Work already submitted may
    /// still wait on an external fence after an enqueue error prevented its completion.
    pub(crate) fn retire(&self) {
        self.stuck.store(true, Ordering::Release);
    }
    /// Wait on the CPU until the queue has passed `value`. A queue that does
    /// not get there is not used again: [`Compute::for_adapter`] makes a new one.
    pub fn wait(&self, value: u64) -> Result<()> {
        let result = wait_fence(&self.fence, value);
        if result.is_err() {
            self.stuck.store(true, Ordering::Release);
        }
        result
    }
    /// A fence on this device that other processes and APIs can open, with an NT handle to it.
    pub(crate) fn share_fence(&self) -> Result<(ID3D12Fence, OwnedHandle)> {
        // SAFETY: The fence is created on the live `self.device`, and the NT handle is owned at
        // once.
        unsafe {
            let fence: ID3D12Fence = self.device.CreateFence(0, D3D12_FENCE_FLAG_SHARED)?;
            let handle = self
                .device
                .CreateSharedHandle(&fence, None, GENERIC_ALL.0, None)?;
            Ok((fence, OwnedHandle::from_raw_handle(handle.0)))
        }
    }
    /// A `width` x `height` single-sample 2D texture in `format` in the COMMON state.
    pub(crate) fn texture(
        &self,
        width: u32,
        height: u32,
        format: DXGI_FORMAT,
        flags: D3D12_RESOURCE_FLAGS,
        heap: D3D12_HEAP_FLAGS,
    ) -> Result<ID3D12Resource> {
        let desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
            Width: u64::from(width),
            Height: height,
            DepthOrArraySize: 1,
            MipLevels: 1,
            Format: format,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
            Flags: flags,
            Alignment: 0,
        };
        let mut resource: Option<ID3D12Resource> = None;
        // SAFETY: `desc` describes a plain single-sample 2D texture on the live `self.device`, and
        // `resource` is a live local.
        unsafe {
            self.device.CreateCommittedResource(
                &D3D12_HEAP_PROPERTIES {
                    Type: D3D12_HEAP_TYPE_DEFAULT,
                    ..Default::default()
                },
                heap,
                &desc,
                D3D12_RESOURCE_STATE_COMMON,
                None,
                &mut resource,
            )
        }
        .with_context(|| format!("a {width}x{height} {format:?} texture on the compute device"))?;
        resource.context("no texture")
    }
    /// A texture as [`Compute::texture`] makes it that other processes and APIs can open, with
    /// an NT handle to it. It has no keyed mutex: fences order every access.
    pub(crate) fn share_texture(
        &self,
        width: u32,
        height: u32,
        format: DXGI_FORMAT,
        flags: D3D12_RESOURCE_FLAGS,
    ) -> Result<(ID3D12Resource, OwnedHandle)> {
        let resource = self.texture(width, height, format, flags, D3D12_HEAP_FLAG_SHARED)?;
        // SAFETY: `resource` is a live shared resource on `self.device`; the NT handle is owned at
        // once.
        unsafe {
            let handle = self
                .device
                .CreateSharedHandle(&resource, None, GENERIC_ALL.0, None)?;
            Ok((resource, OwnedHandle::from_raw_handle(handle.0)))
        }
    }
    /// Whether `resource` was created on, or opened by, this device.
    pub(crate) fn owns(&self, resource: &ID3D12Resource) -> bool {
        let mut device: Option<ID3D12Device> = None;
        // SAFETY: `resource` is live and GetDevice only writes the live local.
        let _ = unsafe { resource.GetDevice(&mut device) };
        device.is_some_and(|device| device.as_raw() == self.device.as_raw())
    }
}

/// When a GPU write into a frame is complete: `fence` reaching `value`.
#[derive(Clone)]
pub struct Ready {
    pub(crate) fence: ID3D12Fence,
    pub(crate) value: u64,
    /// The D3D12 device whose queues can wait for the fence.
    device: usize,
    /// The last compute fence value of a conversion reading the frame, for a copy that
    /// reuses its texture.
    readers: Option<Arc<AtomicU64>>,
    /// The exact conversion timeline the capture copy queue waits for. A
    /// replacement queue can share the same D3D12 device, but never its values.
    reader_fence: Option<ID3D12Fence>,
}

impl Ready {
    /// Work on `compute`'s queues waits for `fence`, one of its device's, to reach `value`.
    pub(crate) fn on(compute: &Compute, fence: ID3D12Fence, value: u64) -> Self {
        Self {
            fence,
            value,
            device: compute.device.as_raw() as usize,
            readers: None,
            reader_fence: None,
        }
    }
    /// Block until the write is done.
    pub fn wait(&self) -> Result<()> {
        wait_fence(&self.fence, self.value)
    }
    /// Make `queue`, one of `compute`'s, wait for the write on the GPU; a fence of another
    /// device is waited for here.
    fn order(&self, compute: &Compute, queue: &ID3D12CommandQueue) -> Result<()> {
        if self.device == compute.device.as_raw() as usize {
            // SAFETY: The fence belongs to the queue's device and `self` holds it for the call.
            unsafe { queue.Wait(&self.fence, self.value) }.inspect_err(|_| compute.retire())?;
            Ok(())
        } else {
            self.wait()
        }
    }
    /// Record that `compute`'s queue reads the frame until its fence reaches `value`.
    fn read_by(&self, compute: &Compute, value: u64) -> Result<()> {
        if let Some(readers) = &self.readers {
            if self.reader_fence.as_ref() == Some(&compute.fence) {
                readers.fetch_max(value, Ordering::Release);
            } else {
                // A different queue's value means nothing on the capture's
                // reader fence. Finish the exceptional cross-queue read before
                // handing ownership back instead of publishing a false value.
                compute.wait(value)?;
            }
        }
        Ok(())
    }
}

/// A command allocator and its list, with the value of the fence that marks the end of its
/// last submission and the resources that submission uses.
struct Recorder {
    allocator: ID3D12CommandAllocator,
    list: ID3D12GraphicsCommandList,
    last: u64,
    keep: Vec<ID3D12Resource>,
}
impl Recorder {
    fn new(device: &ID3D12Device, kind: D3D12_COMMAND_LIST_TYPE) -> Result<Self> {
        // SAFETY: `device` is a live D3D12 device, and the list records into the allocator
        // created with it just before; it is closed until the first reset.
        unsafe {
            let allocator: ID3D12CommandAllocator = device.CreateCommandAllocator(kind)?;
            let list: ID3D12GraphicsCommandList =
                device.CreateCommandList(0, kind, &allocator, None)?;
            list.Close()?;
            Ok(Self {
                allocator,
                list,
                last: 0,
                keep: Vec::new(),
            })
        }
    }
    /// Open the list for recording.
    ///
    /// # Safety
    ///
    /// The GPU must have finished the list's last submission.
    unsafe fn reset(&mut self) -> Result<&ID3D12GraphicsCommandList> {
        self.keep.clear();
        // SAFETY: The caller guarantees the GPU no longer executes the allocator's commands.
        unsafe {
            self.allocator.Reset()?;
            self.list.Reset(&self.allocator, None)?;
        }
        Ok(&self.list)
    }
}

/// Copies frames on a compute queue of its own: a frame ring that is copied rather than read
/// in place (`idd_push_zero_copy = false`).
///
/// Each copy waits on the GPU for the write it copies (the display driver's written fence)
/// and for the conversions still reading the destination's previous frame, and signals its
/// own fence when done. The capture frees the ring slot by signalling the ring's released
/// fence on [`Copier::queue`], after the copy.
pub struct Copier {
    compute: Arc<Compute>,
    queue: ID3D12CommandQueue,
    /// Signalled by the queue once a copy is done.
    copied: ID3D12Fence,
    value: u64,
    lists: Vec<Recorder>,
    /// The conversions reading each destination, by its address. Only the address: a
    /// reference here would keep the pool from ever seeing its texture free.
    readers: HashMap<usize, Arc<AtomicU64>>,
}
impl Copier {
    pub fn new(compute: Arc<Compute>) -> Result<Self> {
        let (queue, _) = compute_queue(&compute.device)?;
        // SAFETY: `compute.device` is a live D3D12 device.
        let copied = unsafe { compute.device.CreateFence(0, D3D12_FENCE_FLAG_NONE)? };
        Ok(Self {
            compute,
            queue,
            copied,
            value: 0,
            lists: Vec::new(),
            readers: HashMap::new(),
        })
    }
    /// The queue the copies run on.
    pub(crate) fn queue(&self) -> &ID3D12CommandQueue {
        &self.queue
    }
    pub(crate) fn forget(&mut self, destination: &ID3D12Resource) {
        self.readers.remove(&(destination.as_raw() as usize));
    }
    /// Copy `source` into `destination` (same size and format, both on this device) once
    /// `after` is reached. The copy is complete when the returned value is.
    pub fn copy(
        &mut self,
        destination: &Arc<ID3D12Resource>,
        source: &ID3D12Resource,
        after: Option<&Ready>,
    ) -> Result<Ready> {
        self.compute.ensure_active()?;
        anyhow::ensure!(
            self.compute.owns(destination) && self.compute.owns(source),
            "a frame copy between GPU devices"
        );
        let readers = self
            .readers
            .entry(destination.as_raw() as usize)
            .or_default()
            .clone();
        let index = self.list()?;
        self.value += 1;
        let value = self.value;
        let recorder = &mut self.lists[index];
        // SAFETY: The recorder's last copy has finished (`list`), `keep` holds both textures until
        // it is reset again, and every barrier names a live resource on this device.
        unsafe {
            let list = recorder.reset()?;
            list.CopyResource(destination.as_ref(), source);
            // The destination was promoted to COPY_DEST, which does not decay on a compute
            // queue: back to COMMON for the conversion and readers.
            let mut barriers = [transition(
                destination,
                D3D12_RESOURCE_STATE_COPY_DEST,
                D3D12_RESOURCE_STATE_COMMON,
            )];
            list.ResourceBarrier(&barriers);
            drop_barriers(&mut barriers);
            list.Close()?;
            recorder.keep = vec![destination.as_ref().clone(), source.clone()];
            let submitted = (|| -> Result<()> {
                self.compute.ensure_active()?;
                if let Some(after) = after {
                    after.order(&self.compute, &self.queue)?;
                }
                // A conversion may still read the texture being reused.
                self.queue
                    .Wait(&self.compute.fence, readers.load(Ordering::Acquire))?;
                let commands = recorder.list.cast::<ID3D12CommandList>()?;
                // Once submitted, a failed signal must not leave the list
                // looking idle (or waiting for an ordinary, never-sent value).
                recorder.last = u64::MAX;
                self.queue.ExecuteCommandLists(&[Some(commands)]);
                self.queue.Signal(&self.copied, value)?;
                recorder.last = value;
                Ok(())
            })();
            submitted.inspect_err(|_| self.compute.retire())?;
        }
        Ok(Ready {
            fence: self.copied.clone(),
            value,
            reader_fence: Some(self.compute.fence.clone()),
            device: self.compute.device.as_raw() as usize,
            readers: Some(readers),
        })
    }
    /// A command list the queue has finished with.
    fn list(&mut self) -> Result<usize> {
        // SAFETY: `copied` is a live D3D12 fence owned by this copier; GetCompletedValue only
        // reads it.
        let done = completed_value(unsafe { self.copied.GetCompletedValue() })?;
        if let Some(index) = self.lists.iter().position(|recorder| recorder.last <= done) {
            return Ok(index);
        }
        if self.lists.len() < 8 {
            self.lists.push(Recorder::new(
                &self.compute.device,
                D3D12_COMMAND_LIST_TYPE_COMPUTE,
            )?);
            return Ok(self.lists.len() - 1);
        }
        // Eight frames behind: the copies are not finishing.
        bail!("capture copies stopped completing")
    }
}
impl Drop for Copier {
    fn drop(&mut self) {
        // A final marker covers a copy whose normal Signal failed as well.
        // Keep all command storage and the copy fence until it completes; an
        // expired CPU wait is never permission to free what the queue may
        // still copy into or read.
        let mut work = RetainedWork::new(&self.compute, &self.queue);
        work.keep(&self.copied);
        for recorder in &self.lists {
            work.keep(&recorder.allocator);
            work.keep(&recorder.list);
            for resource in &recorder.keep {
                work.keep(resource);
            }
        }
        work.finish(&self.compute);
    }
}

pub(crate) fn format(pixel: crate::capture::Pixel) -> DXGI_FORMAT {
    match pixel {
        crate::capture::Pixel::Bgra8 => DXGI_FORMAT_B8G8R8A8_UNORM,
        crate::capture::Pixel::RgbaF16 => DXGI_FORMAT_R16G16B16A16_FLOAT,
        crate::capture::Pixel::Rgba10Pq => DXGI_FORMAT_R10G10B10A2_UNORM,
    }
}
pub(crate) fn pixel(format: DXGI_FORMAT) -> Result<crate::capture::Pixel> {
    Ok(match format {
        DXGI_FORMAT_B8G8R8A8_UNORM => crate::capture::Pixel::Bgra8,
        DXGI_FORMAT_R16G16B16A16_FLOAT => crate::capture::Pixel::RgbaF16,
        DXGI_FORMAT_R10G10B10A2_UNORM => crate::capture::Pixel::Rgba10Pq,
        _ => bail!("unsupported GPU frame format {format:?}"),
    })
}

/// A copy location for `resource`; [`drop_location`] releases its reference.
fn location(
    resource: &ID3D12Resource,
    footprint: Option<D3D12_PLACED_SUBRESOURCE_FOOTPRINT>,
    subresource: u32,
) -> D3D12_TEXTURE_COPY_LOCATION {
    D3D12_TEXTURE_COPY_LOCATION {
        pResource: std::mem::ManuallyDrop::new(Some(resource.clone())),
        Type: if footprint.is_some() {
            D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT
        } else {
            D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX
        },
        Anonymous: match footprint {
            Some(footprint) => D3D12_TEXTURE_COPY_LOCATION_0 {
                PlacedFootprint: footprint,
            },
            None => D3D12_TEXTURE_COPY_LOCATION_0 {
                SubresourceIndex: subresource,
            },
        },
    }
}
fn drop_location(location: D3D12_TEXTURE_COPY_LOCATION) {
    drop(std::mem::ManuallyDrop::into_inner(location.pResource));
}
/// A committed buffer of `size` bytes on `heap` in `state`.
fn buffer(
    device: &ID3D12Device,
    heap: D3D12_HEAP_TYPE,
    size: u64,
    state: D3D12_RESOURCE_STATES,
) -> Result<ID3D12Resource> {
    let mut buffer: Option<ID3D12Resource> = None;
    // SAFETY: `device` is live and the descriptions and out-parameter outlive the call.
    unsafe {
        device.CreateCommittedResource(
            &D3D12_HEAP_PROPERTIES {
                Type: heap,
                ..Default::default()
            },
            D3D12_HEAP_FLAG_NONE,
            &buffer_desc(size),
            state,
            None,
            &mut buffer,
        )?;
    }
    buffer.context("no buffer")
}

/// Upload the rows of `bytes`, `stride` apart, into a new texture in `format` with `flags` on
/// `compute`'s device; returns it and the compute fence value of the copy, which has finished
/// when this returns.
pub(crate) fn upload(
    compute: &Compute,
    (width, height): (u32, u32),
    format: DXGI_FORMAT,
    flags: D3D12_RESOURCE_FLAGS,
    bytes: &[u8],
    stride: usize,
) -> Result<(ID3D12Resource, u64)> {
    let texture = compute.texture(width, height, format, flags, D3D12_HEAP_FLAG_NONE)?;
    let mut footprint = D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default();
    let (mut rows, mut row, mut total) = (0u32, 0u64, 0u64);
    // SAFETY: `texture` is live and every out-parameter is a live local sized for one
    // subresource.
    unsafe {
        compute.device.GetCopyableFootprints(
            &texture.GetDesc(),
            0,
            1,
            0,
            Some(&mut footprint),
            Some(&mut rows),
            Some(&mut row),
            Some(&mut total),
        );
    }
    let row = row as usize;
    anyhow::ensure!(
        rows == height && stride >= row && bytes.len() >= stride * (height as usize - 1) + row,
        "an image upload smaller than its texture"
    );
    let staging = buffer(
        &compute.device,
        D3D12_HEAP_TYPE_UPLOAD,
        total,
        D3D12_RESOURCE_STATE_GENERIC_READ,
    )?;
    let mut recorder = Recorder::new(&compute.device, D3D12_COMMAND_LIST_TYPE_COMPUTE)?;
    // SAFETY: The mapped buffer holds `total` bytes, and every row written lies within the
    // footprint GetCopyableFootprints laid out in it (checked above); the recorder is new, and
    // the queue is waited for before the staging buffer and recorder go.
    unsafe {
        let mut mapped = std::ptr::null_mut();
        staging.Map(0, None, Some(&mut mapped))?;
        for y in 0..height as usize {
            std::ptr::copy_nonoverlapping(
                bytes.as_ptr().add(y * stride),
                mapped
                    .cast::<u8>()
                    .add(footprint.Offset as usize + y * footprint.Footprint.RowPitch as usize),
                row,
            );
        }
        staging.Unmap(0, None);
        let list = recorder.reset()?;
        let destination = location(&texture, None, 0);
        let source = location(&staging, Some(footprint), 0);
        list.CopyTextureRegion(&destination, 0, 0, 0, &source, None);
        drop_location(destination);
        drop_location(source);
        let mut barriers = [transition(
            &texture,
            D3D12_RESOURCE_STATE_COPY_DEST,
            D3D12_RESOURCE_STATE_COMMON,
        )];
        list.ResourceBarrier(&barriers);
        drop_barriers(&mut barriers);
        list.Close()?;
        let value = compute.execute(&recorder.list)?;
        compute.wait(value)?;
        Ok((texture, value))
    }
}

/// A CPU-readable buffer and a command list for [`read_planes`], kept between reads.
pub struct Staging {
    buffer: ID3D12Resource,
    capacity: u64,
    recorder: Recorder,
    device: usize,
}

/// The rows of each plane of `texture`, without padding, once `ready` is reached.
pub(crate) fn read_planes(
    compute: &Compute,
    texture: &ID3D12Resource,
    ready: Option<&Ready>,
    staging: &mut Option<Staging>,
) -> Result<Vec<Vec<u8>>> {
    anyhow::ensure!(compute.owns(texture), "a readback from another GPU device");
    // SAFETY: `texture` is a live resource.
    let desc = unsafe { texture.GetDesc() };
    let planes = if matches!(desc.Format, DXGI_FORMAT_NV12 | DXGI_FORMAT_P010) {
        2
    } else {
        1
    };
    let mut footprints = [D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default(); 2];
    let mut rows = [0u32; 2];
    let mut sizes = [0u64; 2];
    let mut total = 0u64;
    // SAFETY: Each output array holds at least `planes` entries.
    unsafe {
        compute.device.GetCopyableFootprints(
            &desc,
            0,
            planes,
            0,
            Some(footprints.as_mut_ptr()),
            Some(rows.as_mut_ptr()),
            Some(sizes.as_mut_ptr()),
            Some(&mut total),
        );
    }
    let device = compute.device.as_raw() as usize;
    if staging
        .as_ref()
        .is_none_or(|staging| staging.capacity < total || staging.device != device)
    {
        *staging = Some(Staging {
            buffer: buffer(
                &compute.device,
                D3D12_HEAP_TYPE_READBACK,
                total,
                D3D12_RESOURCE_STATE_COPY_DEST,
            )?,
            capacity: total,
            recorder: Recorder::new(&compute.device, D3D12_COMMAND_LIST_TYPE_COMPUTE)?,
            device,
        });
    }
    let staging = staging.as_mut().context("no staging buffer")?;
    // SAFETY: The staging recorder's last read was waited for before it returned, the buffer
    // holds `total` bytes laid out as `footprints`, and every row read lies within it.
    unsafe {
        let list = staging.recorder.reset()?;
        for (plane, footprint) in footprints.iter().take(planes as usize).enumerate() {
            let destination = location(&staging.buffer, Some(*footprint), 0);
            let source = location(texture, None, plane as u32);
            list.CopyTextureRegion(&destination, 0, 0, 0, &source, None);
            drop_location(destination);
            drop_location(source);
        }
        list.Close()?;
        if let Some(ready) = ready {
            ready.order(compute, &compute.queue)?;
        }
        let value = compute.execute(&staging.recorder.list)?;
        compute.wait(value)?;
        let mut mapped = std::ptr::null_mut();
        staging.buffer.Map(0, None, Some(&mut mapped))?;
        let mut result = Vec::with_capacity(planes as usize);
        for plane in 0..planes as usize {
            let (footprint, row) = (footprints[plane], sizes[plane] as usize);
            let mut bytes = Vec::with_capacity(row * rows[plane] as usize);
            for y in 0..rows[plane] as usize {
                bytes.extend_from_slice(std::slice::from_raw_parts(
                    mapped
                        .cast::<u8>()
                        .add(footprint.Offset as usize + y * footprint.Footprint.RowPitch as usize),
                    row,
                ));
            }
            result.push(bytes);
        }
        staging.buffer.Unmap(0, None);
        Ok(result)
    }
}

/// `cbuffer Config` in color.hlsl, for a `source` (width, height, pixel) converted to the
/// stream.
pub(crate) fn constants(
    config: &butterpollo_core::rtsp::Negotiated,
    source: (u32, u32, crate::capture::Pixel),
    luminance: [f32; 2],
) -> [u32; 20] {
    let mut values = [0; 20];
    values[..11].copy_from_slice(&[
        source.0,
        source.1,
        config.width,
        config.height,
        match source.2 {
            crate::capture::Pixel::Bgra8 => 0,
            crate::capture::Pixel::RgbaF16 => 1,
            crate::capture::Pixel::Rgba10Pq => 2,
        },
        u32::from(config.hdr),
        u32::from(config.color_matrix()),
        u32::from(config.full_range()),
        u32::from(config.ten_bit()),
        (luminance[0] / 80.).to_bits(),
        luminance[1].to_bits(),
    ]);
    values[11] = u32::from(config.chroma_packing);
    values
}

fn validate_colour_input(values: &[u32; 20]) -> Result<()> {
    // cbuffer Config: pixel=4, hdr=5. scRGB/FP16 has an SDR path, PQ does not.
    anyhow::ensure!(
        values[4] != 2 || values[5] != 0,
        "HDR surface supplied to an SDR converter"
    );
    Ok(())
}

fn require_format_support(
    device: &ID3D12Device,
    format: DXGI_FORMAT,
    support1: D3D12_FORMAT_SUPPORT1,
    support2: D3D12_FORMAT_SUPPORT2,
) -> Result<()> {
    let mut support = D3D12_FEATURE_DATA_FORMAT_SUPPORT {
        Format: format,
        ..Default::default()
    };
    // SAFETY: The feature selector matches the live initialized structure,
    // whose exact size is supplied. This is a read-only device capability query.
    unsafe {
        device.CheckFeatureSupport(
            D3D12_FEATURE_FORMAT_SUPPORT,
            (&mut support as *mut D3D12_FEATURE_DATA_FORMAT_SUPPORT).cast(),
            std::mem::size_of_val(&support) as u32,
        )?;
    }
    anyhow::ensure!(
        support.Support1.contains(support1) && support.Support2.contains(support2),
        "D3D12 format {format:?} does not support compute YUV writes"
    );
    Ok(())
}

fn shader(entry: &[u8]) -> Result<&'static [u8]> {
    shader_bytecode(entry, b"cs_5_0\0").context("compute shader missing from the build")
}
/// Constants, one source view (t0) and `uavs` output views from
/// register `u{first}`.
fn root_signature(device: &ID3D12Device, first: u32, uavs: u32) -> Result<ID3D12RootSignature> {
    // SAFETY: `srv`, `uav` and `parameters` outlive the serialize call that reads them, and the
    // slice covers exactly the blob's GetBufferSize bytes while `blob` is alive.
    unsafe {
        let srv = D3D12_DESCRIPTOR_RANGE {
            RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_SRV,
            NumDescriptors: 1,
            BaseShaderRegister: 0,
            RegisterSpace: 0,
            OffsetInDescriptorsFromTableStart: 0,
        };
        let uav = D3D12_DESCRIPTOR_RANGE {
            RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_UAV,
            NumDescriptors: uavs,
            BaseShaderRegister: first,
            ..srv
        };
        let parameters = [
            D3D12_ROOT_PARAMETER {
                ParameterType: D3D12_ROOT_PARAMETER_TYPE_CBV,
                Anonymous: D3D12_ROOT_PARAMETER_0 {
                    Descriptor: D3D12_ROOT_DESCRIPTOR {
                        ShaderRegister: 0,
                        RegisterSpace: 0,
                    },
                },
                ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
            },
            D3D12_ROOT_PARAMETER {
                ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
                Anonymous: D3D12_ROOT_PARAMETER_0 {
                    DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE {
                        NumDescriptorRanges: 1,
                        pDescriptorRanges: &srv,
                    },
                },
                ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
            },
            D3D12_ROOT_PARAMETER {
                ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
                Anonymous: D3D12_ROOT_PARAMETER_0 {
                    DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE {
                        NumDescriptorRanges: 1,
                        pDescriptorRanges: &uav,
                    },
                },
                ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
            },
        ];
        let mut blob = None;
        D3D12SerializeRootSignature(
            &D3D12_ROOT_SIGNATURE_DESC {
                NumParameters: parameters.len() as u32,
                pParameters: parameters.as_ptr(),
                ..Default::default()
            },
            D3D_ROOT_SIGNATURE_VERSION_1,
            &mut blob,
            None,
        )?;
        let blob = blob.context("no root signature")?;
        Ok(device.CreateRootSignature(
            0,
            std::slice::from_raw_parts(blob.GetBufferPointer().cast::<u8>(), blob.GetBufferSize()),
        )?)
    }
}
fn pipeline(
    device: &ID3D12Device,
    root: &ID3D12RootSignature,
    entry: &[u8],
) -> Result<ID3D12PipelineState> {
    let code = shader(entry)?;
    // SAFETY: `code` is the build's cs_5_0 bytecode with its exact length, and `root` is a live
    // root signature on `device`, borrowed without adding a reference to the descriptor.
    unsafe {
        Ok(
            device.CreateComputePipelineState(&D3D12_COMPUTE_PIPELINE_STATE_DESC {
                pRootSignature: std::mem::transmute_copy(root),
                CS: D3D12_SHADER_BYTECODE {
                    pShaderBytecode: code.as_ptr().cast(),
                    BytecodeLength: code.len(),
                },
                ..Default::default()
            })?,
        )
    }
}
/// One converted picture, ready for the encoder once `fence` reaches `value`.
pub struct Converted {
    pub texture: Arc<ID3D12Resource>,
    pub fence: ID3D12Fence,
    pub value: u64,
}
impl Converted {
    /// Block until the conversion is done.
    pub fn wait(&self) -> Result<()> {
        wait_fence(&self.fence, self.value)
    }
}
/// An output texture with a fence of its own. The encoder waits for the
/// fence and, in AMF's case, signals it again once it is done reading, so
/// the fence must not be shared with anything else.
struct Target {
    texture: Arc<ID3D12Resource>,
    fence: ID3D12Fence,
    value: u64,
}
impl Target {
    /// The value the fence was last set to wait for, by us or the encoder.
    fn last(&self) -> u64 {
        crate::amf::gpu::fence_value(&self.fence).map_or(self.value, |v| v.max(self.value))
    }
    /// Free when nothing holds the texture and nothing will still signal
    /// its fence.
    fn free(&self) -> Result<bool> {
        // SAFETY: `fence` is a live D3D12 fence owned by this target; GetCompletedValue only reads
        // it.
        let done = completed_value(unsafe { self.fence.GetCompletedValue() })?;
        Ok(Arc::strong_count(&self.texture) == 1 && done >= self.last())
    }
}
struct Slot {
    allocator: ID3D12CommandAllocator,
    list: ID3D12GraphicsCommandList,
    fence: u64,
    /// The frame the slot's last conversion reads, kept until it has run: its texture, and a
    /// frame ring slot it may hold, must outlive the read.
    capture: Option<crate::capture::GpuImage>,
    outputs: Vec<ID3D12Resource>,
    waits: Vec<ID3D12Fence>,
}
/// RGB to NV12 or P010, or PyroWave's three planes, on the compute queue: scaling,
/// letterboxing, HDR and the colour matrix (color.hlsl).
pub struct Converter {
    compute: Arc<Compute>,
    root: ID3D12RootSignature,
    /// Luma and chroma in one pass over 2x2 blocks.
    convert: ID3D12PipelineState,
    /// PyroWave's three planes, made when first used.
    planar: Option<(ID3D12RootSignature, ID3D12PipelineState)>,
    heap: ID3D12DescriptorHeap,
    increment: u32,
    constants: ID3D12Resource,
    mapped: *mut u8,
    slots: Vec<Slot>,
    slot: usize,
    pub values: [u32; 20],
    width: u32,
    height: u32,
    ten_bit: bool,
    targets: Vec<Target>,
}
// SAFETY: `mapped` points into the upload buffer `constants` this converter owns, the D3D12 objects
// are free-threaded, and the command allocators and lists are used only through `&mut self`.
unsafe impl Send for Converter {}
const SLOTS: usize = 4;
/// Descriptors per slot: the source, then up to three outputs.
const VIEWS: usize = 4;
impl Converter {
    pub(crate) fn pack_chroma(&mut self) -> Result<()> {
        self.convert = pipeline(&self.compute.device, &self.root, b"chroma_pack_cs\0")?;
        Ok(())
    }
    /// Probe both plane view formats and the actual planar allocation before
    /// AMF initializes its D3D12 context, so unsupported devices fail once.
    /// PyroWave uses separate R8/R16 planes and does not need this YUV probe.
    pub(crate) fn prepare_yuv(&mut self) -> Result<()> {
        self.compute.ensure_active()?;
        let (texture, planes) = if self.ten_bit {
            (
                DXGI_FORMAT_P010,
                [DXGI_FORMAT_R16_UNORM, DXGI_FORMAT_R16G16_UNORM],
            )
        } else {
            (
                DXGI_FORMAT_NV12,
                [DXGI_FORMAT_R8_UNORM, DXGI_FORMAT_R8G8_UNORM],
            )
        };
        require_format_support(
            &self.compute.device,
            texture,
            D3D12_FORMAT_SUPPORT1_TEXTURE2D,
            D3D12_FORMAT_SUPPORT2_NONE,
        )?;
        for plane in planes {
            require_format_support(
                &self.compute.device,
                plane,
                D3D12_FORMAT_SUPPORT1_TYPED_UNORDERED_ACCESS_VIEW,
                D3D12_FORMAT_SUPPORT2_UAV_TYPED_STORE,
            )?;
        }
        self.target()?;
        Ok(())
    }
    pub fn new(compute: Arc<Compute>, width: u32, height: u32, ten_bit: bool) -> Result<Self> {
        compute.ensure_active()?;
        if width == 0 || height == 0 || !width.is_multiple_of(2) || !height.is_multiple_of(2) {
            bail!("compute conversion needs even, nonzero dimensions");
        }
        // SAFETY: Every call is on the live `compute.device` with descriptions that outlive it, and
        // `constants` (SLOTS * 256 bytes) stays mapped at `mapped` until Unmap in Drop.
        unsafe {
            let device = &compute.device;
            let root = root_signature(device, 0, 2)?;
            let convert = pipeline(device, &root, b"yuv420_cs\0")?;
            let heap: ID3D12DescriptorHeap =
                device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                    Type: D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
                    NumDescriptors: (SLOTS * VIEWS) as u32,
                    Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
                    NodeMask: 0,
                })?;
            let increment =
                device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV);
            let constants = buffer(
                device,
                D3D12_HEAP_TYPE_UPLOAD,
                (SLOTS * 256) as u64,
                D3D12_RESOURCE_STATE_GENERIC_READ,
            )?;
            let mut mapped = std::ptr::null_mut();
            constants.Map(0, None, Some(&mut mapped))?;
            let mut slots = Vec::with_capacity(SLOTS);
            for _ in 0..SLOTS {
                let allocator: ID3D12CommandAllocator =
                    device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_COMPUTE)?;
                let list: ID3D12GraphicsCommandList = device.CreateCommandList(
                    0,
                    D3D12_COMMAND_LIST_TYPE_COMPUTE,
                    &allocator,
                    None,
                )?;
                list.Close()?;
                slots.push(Slot {
                    allocator,
                    list,
                    fence: 0,
                    capture: None,
                    outputs: Vec::new(),
                    waits: Vec::new(),
                });
            }
            Ok(Self {
                compute,
                root,
                convert,
                planar: None,
                heap,
                increment,
                constants,
                mapped: mapped.cast(),
                slots,
                slot: 0,
                values: [0; 20],
                width,
                height,
                ten_bit,
                targets: vec![],
            })
        }
    }
    pub fn compute(&self) -> &Arc<Compute> {
        &self.compute
    }
    pub(crate) fn release_completed(&mut self) {
        for slot in &mut self.slots {
            if self.compute.completed(slot.fence) {
                slot.capture = None;
                slot.outputs.clear();
                slot.waits.clear();
            }
        }
    }
    #[cfg(test)]
    pub(crate) fn test_amf_target_states(&self) -> Vec<Option<u32>> {
        self.targets
            .iter()
            .map(|target| {
                crate::amf::gpu::resource_state(&target.texture)
                    .ok()
                    .map(|state| state.0 as u32)
            })
            .collect()
    }
    pub(crate) fn log_stall(&self) {
        // SAFETY: These live devices and fences are only queried, never waited on.
        let (completed, removed) = unsafe {
            (
                self.compute.fence.GetCompletedValue(),
                self.compute.device.GetDeviceRemovedReason(),
            )
        };
        tracing::warn!(
            priority = self.compute.priority,
            conversion_submitted = self.slots.iter().map(|slot| slot.fence).max().unwrap_or(0),
            conversion_completed = completed,
            d3d12_removed = ?removed,
            "AMF compute progress at stall"
        );
        for (index, target) in self.targets.iter().enumerate() {
            // SAFETY: `target` owns this live fence; the query does not wait.
            let completed = unsafe { target.fence.GetCompletedValue() };
            tracing::warn!(
                index,
                conversion_value = target.value,
                amf_value = ?crate::amf::gpu::fence_value(&target.fence),
                completed,
                owners = Arc::strong_count(&target.texture),
                "AMF surface fence at stall"
            );
        }
    }
    /// A free output texture's index.
    fn target(&mut self) -> Result<usize> {
        for (index, target) in self.targets.iter().enumerate() {
            if target.free()? {
                return Ok(index);
            }
        }
        if self.targets.len() >= 8 {
            // AMF can release its surface reference just before its GPU
            // completion signal. Wait for such a target instead of declaring
            // the bounded pool exhausted and triggering keyframe recovery.
            if let Some((index, target)) = self
                .targets
                .iter()
                .enumerate()
                .find(|(_, target)| Arc::strong_count(&target.texture) == 1)
            {
                wait_fence(&target.fence, target.last()).inspect_err(|_| self.compute.retire())?;
                return Ok(index);
            }
            bail!("compute conversion queue reached its bounded limit");
        }
        let texture = self.compute.texture(
            self.width,
            self.height,
            if self.ten_bit {
                DXGI_FORMAT_P010
            } else {
                DXGI_FORMAT_NV12
            },
            D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
            D3D12_HEAP_FLAG_NONE,
        )?;
        crate::amf::gpu::set_resource_state(&texture, D3D12_RESOURCE_STATE_COMMON)?;
        self.targets.push(Target {
            texture: Arc::new(texture),
            // SAFETY: `compute.device` is a live D3D12 device.
            fence: unsafe { self.compute.device.CreateFence(0, D3D12_FENCE_FLAG_NONE)? },
            value: 0,
        });
        Ok(self.targets.len() - 1)
    }
    /// The next slot, idle, with `values` in its constants and `image`'s texture in its source
    /// view.
    fn begin(&mut self, image: &crate::capture::GpuImage) -> Result<usize> {
        anyhow::ensure!(
            self.compute.owns(&image.texture),
            "a frame from another GPU device than the conversion's"
        );
        let slot = self.slot;
        self.slot = (self.slot + 1) % SLOTS;
        self.compute.wait(self.slots[slot].fence)?;
        self.slots[slot].capture = None;
        // SAFETY: The slot's last conversion has finished (waited above), the 80 bytes of `values`
        // fit its 256-byte slot, and the view names the live source on this device.
        unsafe {
            std::ptr::copy_nonoverlapping(
                self.values.as_ptr().cast::<u8>(),
                self.mapped.add(slot * 256),
                std::mem::size_of_val(&self.values),
            );
            self.compute.device.CreateShaderResourceView(
                image.texture.as_ref(),
                Some(&D3D12_SHADER_RESOURCE_VIEW_DESC {
                    Format: format(image.pixel),
                    ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
                    Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
                    Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                        Texture2D: D3D12_TEX2D_SRV {
                            MostDetailedMip: 0,
                            MipLevels: 1,
                            PlaneSlice: 0,
                            ResourceMinLODClamp: 0.,
                        },
                    },
                }),
                self.view(slot, 0),
            );
        }
        Ok(slot)
    }
    fn view(&self, slot: usize, index: usize) -> D3D12_CPU_DESCRIPTOR_HANDLE {
        // SAFETY: `heap` is a live descriptor heap owned by the converter.
        let cpu = unsafe { self.heap.GetCPUDescriptorHandleForHeapStart() };
        D3D12_CPU_DESCRIPTOR_HANDLE {
            ptr: cpu.ptr + (slot * VIEWS + index) * self.increment as usize,
        }
    }
    /// Bind `slot`'s constants and views and dispatch over 2x2 blocks.
    ///
    /// # Safety
    ///
    /// `list` must be the slot's list, open, with `root` and the pipeline state matching it.
    unsafe fn dispatch(
        &self,
        slot: usize,
        list: &ID3D12GraphicsCommandList,
        root: &ID3D12RootSignature,
    ) {
        // SAFETY: The caller passes the slot's open list; the heap and constants are live and the
        // tables address this slot's descriptors.
        unsafe {
            let gpu = self.heap.GetGPUDescriptorHandleForHeapStart();
            let table = |index: usize| D3D12_GPU_DESCRIPTOR_HANDLE {
                ptr: gpu.ptr + ((slot * VIEWS + index) * self.increment as usize) as u64,
            };
            list.SetComputeRootSignature(root);
            list.SetDescriptorHeaps(&[Some(self.heap.clone())]);
            list.SetComputeRootConstantBufferView(
                0,
                self.constants.GetGPUVirtualAddress() + (slot * 256) as u64,
            );
            list.SetComputeRootDescriptorTable(1, table(0));
            list.SetComputeRootDescriptorTable(2, table(1));
            list.Dispatch(
                (self.width / 2).div_ceil(8),
                (self.height / 2).div_ceil(8),
                1,
            );
        }
    }
    /// Submit the slot's closed list after the GPU waits for `image`'s write (and for `also`),
    /// and keep the image, `outputs` and the fences waited for until the conversion has run.
    fn submit(
        &mut self,
        slot: usize,
        image: &crate::capture::GpuImage,
        outputs: Vec<ID3D12Resource>,
        also: Option<(&ID3D12Fence, u64)>,
    ) -> Result<u64> {
        // The slot's previous conversion has finished (`begin`), so its inputs can go; this
        // one's stay until the slot comes round again. Until the fence below replaces
        // u64::MAX, a failure leaves the slot looking busy and everything it names held.
        self.slots[slot].capture = Some(image.clone());
        self.slots[slot].outputs = outputs;
        self.slots[slot].waits = image
            .ready
            .as_ref()
            .map(|ready| ready.fence.clone())
            .into_iter()
            .chain(also.map(|(fence, _)| fence.clone()))
            .collect();
        self.slots[slot].fence = u64::MAX;
        self.compute.ensure_active()?;
        // The capture's write into the source may still be waiting for the display driver.
        if let Some(ready) = &image.ready {
            ready.order(&self.compute, &self.compute.queue)?;
        }
        if let Some((fence, value)) = also {
            // SAFETY: The fence is on this queue's device and the caller holds it for the call.
            unsafe { self.compute.queue.Wait(fence, value) }
                .inspect_err(|_| self.compute.retire())?;
        }
        let fence = self
            .compute
            .execute(&self.slots[slot].list)
            .inspect_err(|_| self.compute.retire())?;
        self.slots[slot].fence = fence;
        if let Some(ready) = &image.ready {
            ready.read_by(&self.compute, fence)?;
        }
        Ok(fence)
    }
    /// Convert `image` (on this converter's device) into a new YUV texture.
    pub fn convert(&mut self, image: &crate::capture::GpuImage) -> Result<Converted> {
        self.compute.ensure_active()?;
        validate_colour_input(&self.values)?;
        let index = self.target()?;
        let target = self.targets[index].texture.clone();
        // AMF updates this private data when it reads a submitted texture. Its
        // native surface release and completion fence make the selected target
        // free; neither returns its resource state to COMMON.
        let state = crate::amf::gpu::resource_state(&target)?;
        ensure_compute_texture_state(state)?;
        let slot = self.begin(image)?;
        let (luma_format, chroma_format) = if self.ten_bit {
            (DXGI_FORMAT_R16_UNORM, DXGI_FORMAT_R16G16_UNORM)
        } else {
            (DXGI_FORMAT_R8_UNORM, DXGI_FORMAT_R8G8_UNORM)
        };
        // SAFETY: The slot is idle (`begin`), and every view and barrier names a live resource on
        // this device.
        unsafe {
            for (view, (format, plane)) in [(luma_format, 0), (chroma_format, 1)]
                .into_iter()
                .enumerate()
            {
                self.compute.device.CreateUnorderedAccessView(
                    target.as_ref(),
                    None,
                    Some(&D3D12_UNORDERED_ACCESS_VIEW_DESC {
                        Format: format,
                        ViewDimension: D3D12_UAV_DIMENSION_TEXTURE2D,
                        Anonymous: D3D12_UNORDERED_ACCESS_VIEW_DESC_0 {
                            Texture2D: D3D12_TEX2D_UAV {
                                MipSlice: 0,
                                PlaneSlice: plane,
                            },
                        },
                    }),
                    self.view(slot, 1 + view),
                );
            }
            let Slot {
                allocator, list, ..
            } = &self.slots[slot];
            allocator.Reset()?;
            list.Reset(allocator, &self.convert)?;
            let mut barriers = [transition(
                target.as_ref(),
                state,
                D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
            )];
            list.ResourceBarrier(&barriers);
            drop_barriers(&mut barriers);
            self.dispatch(slot, list, &self.root);
            let mut barriers = [transition(
                target.as_ref(),
                D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                D3D12_RESOURCE_STATE_COMMON,
            )];
            list.ResourceBarrier(&barriers);
            drop_barriers(&mut barriers);
            list.Close()?;
        }
        self.submit(slot, image, Vec::new(), None)?;
        // The encoder's signal for this texture, after the conversion.
        let target = &mut self.targets[index];
        let value = target.last() + 1;
        // SAFETY: `queue` and `target.fence` were created on the same D3D12 device and outlive the
        // call.
        unsafe {
            self.compute
                .queue
                .Signal(&target.fence, value)
                .inspect_err(|_| self.compute.retire())?;
        }
        target.value = value;
        Ok(Converted {
            texture: target.texture.clone(),
            fence: target.fence.clone(),
            value: target.value,
        })
    }
    /// Convert `image` into PyroWave's Y, Cb and Cr planes, full size or
    /// with chroma halved (`values[11]`), in textures the encoder shares
    /// with Vulkan. The queue first waits for `target.fence` to reach
    /// `target.after`, when the encoder is done with the planes, then
    /// signals `target.done`.
    pub fn convert_planes(
        &mut self,
        image: &crate::capture::GpuImage,
        target: &Planes,
    ) -> Result<()> {
        self.compute.ensure_active()?;
        self.release_completed();
        validate_colour_input(&self.values)?;
        let (root, planar) = match &self.planar {
            Some(planar) => planar.clone(),
            None => {
                let root = root_signature(&self.compute.device, 2, 3)?;
                let planar = pipeline(&self.compute.device, &root, b"pyro_cs\0")?;
                self.planar = Some((root.clone(), planar.clone()));
                (root, planar)
            }
        };
        let slot = self.begin(image)?;
        // SAFETY: The slot is idle (`begin`); `target`'s planes and fence are on this device.
        unsafe {
            for (index, plane) in target.planes.iter().enumerate() {
                self.compute.device.CreateUnorderedAccessView(
                    plane,
                    None,
                    Some(&D3D12_UNORDERED_ACCESS_VIEW_DESC {
                        Format: target.format,
                        ViewDimension: D3D12_UAV_DIMENSION_TEXTURE2D,
                        Anonymous: D3D12_UNORDERED_ACCESS_VIEW_DESC_0 {
                            Texture2D: D3D12_TEX2D_UAV {
                                MipSlice: 0,
                                PlaneSlice: 0,
                            },
                        },
                    }),
                    self.view(slot, 1 + index),
                );
            }
            let Slot {
                allocator, list, ..
            } = &self.slots[slot];
            allocator.Reset()?;
            list.Reset(allocator, &planar)?;
            let mut barriers = target.planes.each_ref().map(|plane| {
                transition(
                    plane,
                    D3D12_RESOURCE_STATE_COMMON,
                    D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                )
            });
            list.ResourceBarrier(&barriers);
            drop_barriers(&mut barriers);
            self.dispatch(slot, list, &root);
            let mut barriers = target.planes.each_ref().map(|plane| {
                transition(
                    plane,
                    D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                    D3D12_RESOURCE_STATE_COMMON,
                )
            });
            list.ResourceBarrier(&barriers);
            drop_barriers(&mut barriers);
            list.Close()?;
        }
        // The encoder's Vulkan queue may still read the planes.
        self.submit(
            slot,
            image,
            target.planes.to_vec(),
            Some((&target.fence, target.after)),
        )?;
        // SAFETY: `target.fence` is on this queue's device and outlives the call.
        unsafe { self.compute.queue.Signal(&target.fence, target.done) }
            .inspect_err(|_| self.compute.retire())?;
        Ok(())
    }
}
/// PyroWave's planes on the compute device and the fence the encoder's
/// Vulkan queue shares, for [`Converter::convert_planes`].
pub struct Planes {
    pub planes: [ID3D12Resource; 3],
    /// `DXGI_FORMAT_R16_UNORM` for ten-bit streams, else `R8_UNORM`.
    pub format: DXGI_FORMAT,
    pub fence: ID3D12Fence,
    pub after: u64,
    pub done: u64,
}
impl Drop for Converter {
    fn drop(&mut self) {
        // The queue may still read the constants and write the targets. A wait that
        // expires is no permission to free them: keep them until a marker proves it.
        let mut work = RetainedWork::new(&self.compute, &self.compute.queue);
        work.keep(&self.root);
        work.keep(&self.convert);
        if let Some((root, pipeline)) = &self.planar {
            work.keep(root);
            work.keep(pipeline);
        }
        work.keep(&self.heap);
        work.keep(&self.constants);
        for slot in &mut self.slots {
            work.keep(&slot.allocator);
            work.keep(&slot.list);
            for resource in &slot.outputs {
                work.keep(resource);
            }
            for fence in &slot.waits {
                work.keep(fence);
            }
            if let Some(capture) = slot.capture.take() {
                work.captures.push(capture);
            }
        }
        for target in &self.targets {
            work.keep(target.texture.as_ref());
            work.waits.push((target.fence.clone(), target.last()));
        }
        work.finish(&self.compute);
        // SAFETY: `constants` was mapped once in `new`, and nothing writes through `mapped` after
        // this. Unmap ends CPU access; retained GPU work still owns the resource.
        unsafe { self.constants.Unmap(0, None) };
    }
}
/// A transition of every subresource; `drop_barriers` releases its reference.
fn transition(
    resource: &ID3D12Resource,
    before: D3D12_RESOURCE_STATES,
    after: D3D12_RESOURCE_STATES,
) -> D3D12_RESOURCE_BARRIER {
    D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
        Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            Transition: std::mem::ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                pResource: std::mem::ManuallyDrop::new(Some(resource.clone())),
                Subresource: D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                StateBefore: before,
                StateAfter: after,
            }),
        },
    }
}
/// Release the resource references a transition barrier holds.
fn drop_barriers(barriers: &mut [D3D12_RESOURCE_BARRIER]) {
    for barrier in barriers {
        // SAFETY: Callers pass only barriers from `transition`, once each, so `Transition` is the
        // active field and still holds the one reference released here.
        unsafe {
            let mut transition = std::mem::ManuallyDrop::take(&mut barrier.Anonymous.Transition);
            std::mem::ManuallyDrop::drop(&mut transition.pResource);
        }
    }
}
fn buffer_desc(size: u64) -> D3D12_RESOURCE_DESC {
    D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Width: size,
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: DXGI_FORMAT_UNKNOWN,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        Flags: D3D12_RESOURCE_FLAG_NONE,
        Alignment: 0,
    }
}

#[cfg(test)]
mod tests;
