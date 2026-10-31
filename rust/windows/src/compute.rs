//! Capture copies and colour conversion on D3D12 compute queues.
//!
//! D3D11 work waits behind whatever a game has queued on the GPU's graphics
//! engine: beside a game using the whole GPU, the D3D11 colour conversion
//! took 7 ms instead of 0.9 ms and a plain copy 8 ms instead of 0.7 ms. A
//! compute queue runs next to the graphics engine and finished the same
//! work in 0.2-0.3 ms under that load (`examples/d3d12_probe.rs`,
//! `examples/gpu_load.rs`). Captured frames are copied into shared textures
//! on one compute queue ([`Handoff`]) and converted to the encoder's YUV
//! format on another ([`Converter`]); the encoder waits for the conversion's
//! fence on the GPU.

use anyhow::{Context, Result, bail};
use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};
use windows::{
    Win32::{
        Foundation::{CloseHandle, GENERIC_ALL, HANDLE, LUID},
        Graphics::{
            Direct3D::D3D_FEATURE_LEVEL_11_0,
            Direct3D11::{ID3D11Device5, ID3D11DeviceContext4, ID3D11Fence, ID3D11Texture2D},
            Direct3D12::*,
            Dxgi::{Common::*, *},
        },
    },
    core::{IUnknown, Interface, PCWSTR},
};

include!(concat!(env!("OUT_DIR"), "/shader_bytecode.rs"));

/// Whether captures and AMF encoders use the compute queue
/// (`gpu_compute_conversion`, on unless turned off).
pub fn enabled(config: &butterpollo_core::config::Config) -> bool {
    config.boolean("gpu_compute_conversion", true)
}
/// Whether the compute queue can open `texture` (an NT-handle shared one).
pub fn shareable(texture: &ID3D11Texture2D) -> bool {
    let mut desc = windows::Win32::Graphics::Direct3D11::D3D11_TEXTURE2D_DESC::default();
    // SAFETY: `texture` is a live texture and `desc` is a live local that GetDesc fills.
    unsafe { texture.GetDesc(&mut desc) };
    desc.MiscFlags
        & windows::Win32::Graphics::Direct3D11::D3D11_RESOURCE_MISC_SHARED_NTHANDLE.0 as u32
        != 0
}

/// A D3D12 device on the capture GPU with one compute queue and its fence.
pub struct Compute {
    pub device: ID3D12Device,
    pub queue: ID3D12CommandQueue,
    pub fence: ID3D12Fence,
    next: Mutex<u64>,
    /// D3D11 textures already opened here, by their COM pointer.
    opened: Mutex<HashMap<usize, (ID3D11Texture2D, ID3D12Resource)>>,
    /// Textures another process created and shared, opened from its handle
    /// (CreateSharedHandle cannot share them again); kept until forgotten.
    foreign: Mutex<HashMap<usize, (ID3D11Texture2D, ID3D12Resource)>>,
    pub priority: i32,
    /// Set when the queue cannot safely be reused after an ordering error or timeout.
    stuck: std::sync::atomic::AtomicBool,
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
// SAFETY: The erased interfaces are D3D12 objects, multithread-protected D3D11
// objects, or fences; they are only retained/released, never used via IUnknown.
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
        // SAFETY: Every caller supplies a COM D3D11/D3D12 interface, whose
        // vtable starts with IUnknown. Transfer a cloned reference without a
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
// D3D12 devices, queues and fences are free-threaded; the rest is locked.
// SAFETY: The D3D12 device, queue and fence are free-threaded, the cached D3D11 textures and D3D12
// resources are only touched under `opened`'s and `foreign`'s locks, and `next` is a mutex.
unsafe impl Send for Compute {}
// SAFETY: `&self` methods call only free-threaded D3D12 methods and change state only through the
// mutexes and the atomic.
unsafe impl Sync for Compute {}
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
fn ensure_queue_active(retired: &std::sync::atomic::AtomicBool) -> Result<()> {
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
static REALTIME: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub fn set_realtime(enabled: bool) {
    REALTIME.store(enabled, std::sync::atomic::Ordering::Relaxed);
}
pub fn realtime() -> bool {
    REALTIME.load(std::sync::atomic::Ordering::Relaxed)
}
/// A compute queue at the highest allowed priority Windows grants this process.
fn compute_queue(device: &ID3D12Device) -> Result<(ID3D12CommandQueue, i32)> {
    let realtime = REALTIME.load(std::sync::atomic::Ordering::Relaxed);
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
                .context("D3D12 device for compute conversion")?;
            let device = device.context("no D3D12 device")?;
            let (queue, priority) = compute_queue(&device)?;
            let fence = device.CreateFence(0, D3D12_FENCE_FLAG_NONE)?;
            tracing::info!(priority, "compute queue ready for colour conversion");
            Ok(Arc::new(Self {
                device,
                queue,
                fence,
                next: Mutex::new(0),
                opened: Mutex::new(HashMap::new()),
                foreign: Mutex::new(HashMap::new()),
                priority,
                stuck: std::sync::atomic::AtomicBool::new(false),
            }))
        }
    }
    /// The compute queue for the GPU a D3D11 device runs on, shared by the
    /// capture and the encoders on that GPU.
    pub fn for_device(
        device: &windows::Win32::Graphics::Direct3D11::ID3D11Device,
    ) -> Result<Arc<Self>> {
        type Shared = Vec<((u32, i32), std::sync::Weak<Compute>)>;
        static SHARED: Mutex<Shared> = Mutex::new(Vec::new());
        // SAFETY: `device` is a live D3D11 device, and GetAdapter and GetDesc only return owned
        // values.
        let luid = unsafe {
            let dxgi: IDXGIDevice = device.cast()?;
            dxgi.GetAdapter()?.GetDesc()?.AdapterLuid
        };
        let key = (luid.LowPart, luid.HighPart);
        let mut shared = SHARED.lock().unwrap();
        shared.retain(|(_, compute)| compute.strong_count() > 0);
        if let Some(compute) = shared
            .iter()
            .filter(|(owner, _)| *owner == key)
            .filter_map(|(_, compute)| compute.upgrade())
            .find(|compute| {
                !compute.stuck.load(std::sync::atomic::Ordering::Acquire) && compute.completed(0)
            })
        {
            return Ok(compute);
        }
        let compute = Self::new(luid)?;
        shared.push((key, Arc::downgrade(&compute)));
        Ok(compute)
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
            // The base interface by COM inheritance: no QueryInterface per frame.
            self.queue
                .ExecuteCommandLists(&[Some(ID3D12CommandList::clone(list))]);
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
                self.stuck.store(true, std::sync::atomic::Ordering::Release);
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
    /// not get there is not used again: [`Compute::for_device`] makes a new one.
    pub fn wait(&self, value: u64) -> Result<()> {
        let result = wait_fence(&self.fence, value);
        if result.is_err() {
            self.stuck.store(true, std::sync::atomic::Ordering::Release);
        }
        result
    }
    /// A shared D3D11 texture as a D3D12 resource on this device.
    pub fn open(&self, texture: &ID3D11Texture2D) -> Result<ID3D12Resource> {
        self.ensure_active()?;
        let key = texture.as_raw() as usize;
        if let Some((_, resource)) = self.foreign.lock().unwrap().get(&key) {
            return Ok(resource.clone());
        }
        let mut opened = self.opened.lock().unwrap();
        if let Some((_, resource)) = opened.get(&key) {
            return Ok(resource.clone());
        }
        // SAFETY: `texture` is a live D3D11 texture on this GPU, the NT handle is closed exactly
        // once after OpenSharedHandle has used it, and `resource` is a live local.
        unsafe {
            let shared: IDXGIResource1 = texture.cast()?;
            let handle = shared
                .CreateSharedHandle(
                    None,
                    DXGI_SHARED_RESOURCE_READ.0 | DXGI_SHARED_RESOURCE_WRITE.0,
                    None,
                )
                .context("sharing a captured texture with the compute queue")?;
            let mut resource: Option<ID3D12Resource> = None;
            let result = self.device.OpenSharedHandle(handle, &mut resource);
            let _ = CloseHandle(handle);
            result?;
            let resource = resource.context("no shared resource")?;
            // Captures recycle a few textures; start over if many came and
            // went. In-flight copies and conversions hold their own references,
            // so this cannot free memory the GPU still uses; the bound keeps
            // stale 4K HDR textures from pinning gigabytes of video memory.
            if opened.len() >= 24 {
                opened.clear();
            }
            opened.insert(key, (texture.clone(), resource.clone()));
            Ok(resource)
        }
    }
}

impl Compute {
    /// Open a texture another process shared through `handle`, so that
    /// `open` finds it until `forget`.
    pub(crate) fn import(&self, texture: &ID3D11Texture2D, handle: HANDLE) -> Result<()> {
        self.ensure_active()?;
        let mut resource: Option<ID3D12Resource> = None;
        // SAFETY: `handle` is a live NT handle to `texture` that the caller owns, and `resource` is
        // a live local.
        unsafe {
            self.device.OpenSharedHandle(handle, &mut resource)?;
        }
        self.foreign.lock().unwrap().insert(
            texture.as_raw() as usize,
            (
                texture.clone(),
                resource.context("no imported shared resource")?,
            ),
        );
        Ok(())
    }
    pub(crate) fn forget(&self, texture: &ID3D11Texture2D) {
        self.opened
            .lock()
            .unwrap()
            .remove(&(texture.as_raw() as usize));
        self.foreign
            .lock()
            .unwrap()
            .remove(&(texture.as_raw() as usize));
    }
}
/// A D3D12 fence the capture's D3D11 device can signal and wait for.
fn shared_fence(compute: &Compute, device: &ID3D11Device5) -> Result<(ID3D12Fence, ID3D11Fence)> {
    // SAFETY: The fence is created on the live `compute.device`, the NT handle is closed once after
    // `device` opened it, and `opened` is a live local.
    unsafe {
        let fence: ID3D12Fence = compute.device.CreateFence(0, D3D12_FENCE_FLAG_SHARED)?;
        let handle = compute
            .device
            .CreateSharedHandle(&fence, None, GENERIC_ALL.0, None)?;
        let mut opened: Option<ID3D11Fence> = None;
        let result = device.OpenSharedFence(handle, &mut opened);
        let _ = CloseHandle(handle);
        result.context("sharing a fence with the capture device")?;
        Ok((fence, opened.context("no shared fence")?))
    }
}
impl Compute {
    /// A fence on this device that other processes can open: the fence, an NT handle to it
    /// for them, and the fence opened on the capture's D3D11 `device`.
    pub(crate) fn share_fence(
        &self,
        device: &ID3D11Device5,
    ) -> Result<(ID3D12Fence, ID3D11Fence, std::os::windows::io::OwnedHandle)> {
        use std::os::windows::io::FromRawHandle;
        // SAFETY: The fence is created on the live `self.device`; the NT handle is owned at once,
        // `device` only borrows it, and `opened` is a live local.
        unsafe {
            let fence: ID3D12Fence = self.device.CreateFence(0, D3D12_FENCE_FLAG_SHARED)?;
            let handle = self
                .device
                .CreateSharedHandle(&fence, None, GENERIC_ALL.0, None)?;
            let handle = std::os::windows::io::OwnedHandle::from_raw_handle(handle.0);
            let mut opened: Option<ID3D11Fence> = None;
            device
                .OpenSharedFence(
                    HANDLE(std::os::windows::io::AsRawHandle::as_raw_handle(&handle)),
                    &mut opened,
                )
                .context("sharing a fence with the capture device")?;
            Ok((fence, opened.context("no shared fence")?, handle))
        }
    }

    /// A `width` x `height` texture in `format` that other processes can open, created on
    /// this device with no keyed mutex: an NT handle to it for them, and the texture opened
    /// on the capture's D3D11 `device`. [`Compute::open`] finds this device's resource for it
    /// until [`Compute::forget`].
    pub(crate) fn share_texture(
        &self,
        device: &windows::Win32::Graphics::Direct3D11::ID3D11Device1,
        width: u32,
        height: u32,
        format: DXGI_FORMAT,
    ) -> Result<(ID3D11Texture2D, std::os::windows::io::OwnedHandle)> {
        use std::os::windows::io::{AsRawHandle, FromRawHandle};
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
            // D3D11 opens a D3D12 texture only if it may be a render target: without the flag
            // OpenSharedResource1 fails with E_INVALIDARG (RX 7900 XT, October 16).
            // The host's compute and graphics readers, and a driver seed copy, may read
            // simultaneously on different devices. CAS ownership excludes writers but
            // does not itself permit concurrent D3D12 resource access.
            Flags: D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET
                | D3D12_RESOURCE_FLAG_ALLOW_SIMULTANEOUS_ACCESS,
            Alignment: 0,
        };
        // SAFETY: `desc` describes a plain single-sample 2D texture on the live `self.device`; the
        // NT handle is owned at once and `device` only borrows it.
        unsafe {
            let mut resource: Option<ID3D12Resource> = None;
            self.device.CreateCommittedResource(
                &D3D12_HEAP_PROPERTIES {
                    Type: D3D12_HEAP_TYPE_DEFAULT,
                    ..Default::default()
                },
                D3D12_HEAP_FLAG_SHARED,
                &desc,
                D3D12_RESOURCE_STATE_COMMON,
                None,
                &mut resource,
            )?;
            let resource = resource.context("no shared texture")?;
            let handle = self
                .device
                .CreateSharedHandle(&resource, None, GENERIC_ALL.0, None)?;
            let handle = std::os::windows::io::OwnedHandle::from_raw_handle(handle.0);
            let texture: ID3D11Texture2D = device
                .OpenSharedResource1(HANDLE(handle.as_raw_handle()))
                .context("opening a D3D12 texture on the capture device")?;
            self.foreign
                .lock()
                .unwrap()
                .insert(texture.as_raw() as usize, (texture.clone(), resource));
            Ok((texture, handle))
        }
    }
}

/// When a copied frame is complete: the copy queue's fence reaching `value`.
#[derive(Clone)]
pub struct Ready {
    pub(crate) fence: ID3D12Fence,
    pub(crate) value: u64,
    /// The D3D12 device whose queues can wait for the fence.
    device: usize,
    readers: Option<Arc<AtomicU64>>,
    /// The exact conversion timeline the capture copy queue waits for. A
    /// replacement queue can share the same D3D12 device, but never its values.
    reader_fence: Option<ID3D12Fence>,
}
// Fences are free-threaded.
// SAFETY: `fence` is a free-threaded ID3D12Fence, and `device` is an address compared, never
// dereferenced.
unsafe impl Send for Ready {}
// SAFETY: Shared use only reads `value` and calls free-threaded methods on the fence.
unsafe impl Sync for Ready {}
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
    /// Block until the copy is done.
    pub fn wait(&self) -> Result<()> {
        wait_fence(&self.fence, self.value)
    }
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

/// Lends compute work textures another process writes, in place, and hands
/// them back. The D3D11 device takes such a texture's keyed mutex, which
/// D3D12 cannot take, then signals `acquired`; compute work reading the
/// texture waits for that. Before the D3D11 device releases the mutex it
/// waits for all compute work submitted so far, so the writer never
/// overwrites a texture a conversion still reads.
///
/// The WGC helper's zero-copy path uses it, and so does the display driver's
/// default keyed-mutex frame ring. A fence ring needs no D3D11 step: the compute queue waits for
/// the driver's fence and signals its own (`capture::idd_push`).
pub(crate) struct Returns {
    compute: Arc<Compute>,
    acquired: (ID3D12Fence, ID3D11Fence),
    drained: (ID3D12Fence, ID3D11Fence),
    context: ID3D11DeviceContext4,
    value: u64,
    imported: Vec<ID3D11Texture2D>,
}
/// A failed ordering operation does not establish that a texture is no longer
/// read by the GPU. Keep it owned and reject further work on this queue.
fn return_after_reads(
    retired: &std::sync::atomic::AtomicBool,
    order: impl FnOnce() -> Result<()>,
    release: impl FnOnce() -> Result<()>,
) -> Result<()> {
    ensure_queue_active(retired)?;
    let result = order().and_then(|()| release());
    if result.is_err() {
        retired.store(true, Ordering::Release);
    }
    result
}
// SAFETY: The D3D11 context's device is multithread-protected (`capture::Device::create`), D3D12
// fences are free-threaded, and all else is used only through `&mut self`.
unsafe impl Send for Returns {}
impl Returns {
    pub(crate) fn new(compute: Arc<Compute>, gpu: &crate::capture::Device) -> Result<Self> {
        compute.ensure_active()?;
        let device: ID3D11Device5 = gpu.device.cast()?;
        Ok(Self {
            acquired: shared_fence(&compute, &device)?,
            drained: shared_fence(&compute, &device)?,
            compute,
            context: gpu.context.cast()?,
            value: 0,
            imported: Vec::new(),
        })
    }
    /// Make the shared texture behind `handle` readable by compute work.
    pub(crate) fn import(&mut self, texture: &ID3D11Texture2D, handle: HANDLE) -> Result<()> {
        self.compute.import(texture, handle)?;
        self.imported.push(texture.clone());
        Ok(())
    }
    /// The D3D11 device has just taken a texture's keyed mutex: when compute
    /// work may read it.
    pub(crate) fn acquired(&mut self) -> Result<Ready> {
        self.compute.ensure_active()?;
        self.value += 1;
        // SAFETY: The fence was opened on this context's device, which is multithread-protected.
        unsafe {
            self.context
                .Signal(&self.acquired.1, self.value)
                .inspect_err(|_| self.compute.retire())?;
            self.context.Flush();
        }
        Ok(Ready {
            fence: self.acquired.0.clone(),
            value: self.value,
            device: self.compute.device.as_raw() as usize,
            readers: None,
            reader_fence: None,
        })
    }
    /// Release `mutex` with `key` once all compute work submitted so far,
    /// including every conversion that read its texture, has finished.
    pub(crate) fn hand_back(&mut self, mutex: &IDXGIKeyedMutex, key: u64) -> Result<()> {
        self.value += 1;
        // SAFETY: Both fences are on the compute device and this context's device; the caller holds
        // `mutex` and releases it only here.
        let returned = return_after_reads(
            &self.compute.stuck,
            || unsafe {
                self.compute
                    .queue
                    .Signal(&self.drained.0, self.value)
                    .and_then(|()| self.context.Wait(&self.drained.1, self.value))
                    .context("order shared texture return after GPU readers")
            },
            || unsafe {
                mutex
                    .ReleaseSync(key)
                    .context("return shared texture mutex")
            },
        );
        // SAFETY: The context is live and multithread-protected. Flush an enqueued
        // wait/release even when a later operation failed; no failed slot is reused.
        unsafe { self.context.Flush() };
        returned
    }
}
impl Drop for Returns {
    fn drop(&mut self) {
        for texture in &self.imported {
            self.compute.forget(texture);
        }
    }
}

/// Copies Desktop Duplication frames on a compute queue of their own, in
/// order with the capture's D3D11 device but without the graphics engine.
///
/// Duplication hands a frame over while DWM's copy into it may still be
/// running; the D3D11 device waits for that copy through the frame's keyed
/// mutex, which D3D12 cannot take. Copying without that wait read stale or
/// torn frames (a fifth of them idle, over half beside a game;
/// `examples/ddx_sync_probe.rs`). So the D3D11 context signals a fence after
/// its keyed-mutex wait, the copy waits for that fence on the GPU, and the
/// D3D11 context waits for the copy before it hands the frame back to DWM.
/// Fence waits and signals take no time on the graphics engine, and nothing
/// blocks the capture thread: blocking it until DWM finished held the frame
/// from DWM meanwhile and halved the frame rate beside a game.
pub struct Handoff {
    compute: Arc<Compute>,
    queue: ID3D12CommandQueue,
    /// Signalled by the D3D11 context once it holds the frame.
    acquired: (ID3D12Fence, ID3D11Fence),
    /// Signalled by the copy queue once the frame is copied.
    copied: (ID3D12Fence, ID3D11Fence),
    context: ID3D11DeviceContext4,
    value: u64,
    /// Command lists, each with the value of the copy that last used it and
    /// the resources that copy reads and writes, kept until it has run.
    #[allow(clippy::type_complexity)]
    lists: Vec<(
        ID3D12CommandAllocator,
        ID3D12GraphicsCommandList,
        u64,
        [Option<ID3D12Resource>; 2],
    )>,
    /// Foreign textures belong to this capture, not the adapter-wide cache.
    /// Releasing a helper must release its imported resources immediately.
    imported: HashMap<usize, (ID3D11Texture2D, ID3D12Resource)>,
    opened: HashSet<usize>,
    readers: HashMap<usize, (ID3D11Texture2D, Arc<AtomicU64>)>,
}
// The context is only used by the capture that owns this, under the D3D11
// device's multithread protection; D3D12 objects are free-threaded.
// SAFETY: The D3D11 context's device is multithread-protected (`capture::Device::create`), and the
// command allocators and lists are used only through `&mut self` on one thread at a time.
unsafe impl Send for Handoff {}
impl Handoff {
    pub(crate) fn forget(&mut self, texture: &ID3D11Texture2D) {
        let key = texture.as_raw() as usize;
        self.readers.remove(&key);
        self.opened.remove(&key);
        self.compute.forget(texture);
    }
    pub(crate) fn device_removed(&self) -> Option<crate::device_loss::DeviceLost> {
        crate::device_loss::DeviceLost::d3d12(&self.compute.device)
    }
    /// A helper already created this texture's NT handle. Import that handle
    /// instead of calling CreateSharedHandle a second time on the resource.
    pub(crate) fn import_shared(
        &mut self,
        texture: &ID3D11Texture2D,
        handle: HANDLE,
    ) -> Result<()> {
        self.compute.ensure_active()?;
        let mut resource: Option<ID3D12Resource> = None;
        // SAFETY: `handle` is a live NT handle to `texture` that the caller owns, and `resource` is
        // a live local.
        unsafe {
            self.compute
                .device
                .OpenSharedHandle(handle, &mut resource)?;
        }
        self.imported.insert(
            texture.as_raw() as usize,
            (
                texture.clone(),
                resource.context("no imported shared resource")?,
            ),
        );
        Ok(())
    }
    fn open(&mut self, texture: &ID3D11Texture2D) -> Result<ID3D12Resource> {
        if let Some((_, resource)) = self.imported.get(&(texture.as_raw() as usize)) {
            Ok(resource.clone())
        } else {
            self.opened.insert(texture.as_raw() as usize);
            self.compute.open(texture)
        }
    }
    pub fn new(compute: Arc<Compute>, gpu: &crate::capture::Device) -> Result<Self> {
        compute.ensure_active()?;
        let device: ID3D11Device5 = gpu.device.cast()?;
        let (queue, _) = compute_queue(&compute.device)?;
        Ok(Self {
            acquired: shared_fence(&compute, &device)?,
            copied: shared_fence(&compute, &device)?,
            queue,
            compute,
            context: gpu.context.cast()?,
            value: 0,
            lists: Vec::new(),
            imported: HashMap::new(),
            opened: HashSet::new(),
            readers: HashMap::new(),
        })
    }
    /// Copy the frame the capture's D3D11 device holds into `destination`,
    /// a shared texture. The copy completes when the result is ready; D3D11
    /// work submitted after this call is ordered after it.
    pub fn copy(
        &mut self,
        destination: &ID3D11Texture2D,
        source: &ID3D11Texture2D,
    ) -> Result<Ready> {
        self.compute.ensure_active()?;
        let readers = self
            .readers
            .entry(destination.as_raw() as usize)
            .or_insert_with(|| (destination.clone(), Arc::new(AtomicU64::new(0))))
            .1
            .clone();
        let destination = self.open(destination)?;
        let source = self.open(source)?;
        let list = self.list()?;
        self.value += 1;
        let value = self.value;
        // SAFETY: The list's last copy has finished, `resources` keeps both textures alive until it
        // runs again, and the D3D11 context is multithread-protected.
        unsafe {
            let (allocator, commands, last, resources) = &mut self.lists[list];
            allocator.Reset()?;
            commands.Reset(&*allocator, None)?;
            commands.CopyResource(&destination, &source);
            // The adapter-wide cache may release its references while the copy
            // runs; freeing memory the GPU still reads can hang its engine.
            *resources = [Some(destination.clone()), Some(source.clone())];
            // Back to COMMON for the conversion queue and the D3D11 device.
            let mut barriers = [transition(
                &destination,
                D3D12_RESOURCE_STATE_COPY_DEST,
                D3D12_RESOURCE_STATE_COMMON,
            )];
            commands.ResourceBarrier(&barriers);
            drop_barriers(&mut barriers);
            commands.Close()?;
            let submitted = (|| -> Result<()> {
                self.compute.ensure_active()?;
                self.context.Signal(&self.acquired.1, value)?;
                self.context.Flush();
                self.queue.Wait(&self.acquired.0, value)?;
                // A conversion may still read the pool texture being reused.
                self.queue
                    .Wait(&self.compute.fence, readers.load(Ordering::Acquire))?;
                let commands = ID3D12CommandList::clone(commands);
                // Once submitted, a failed signal must not leave the list
                // looking idle (or waiting for an ordinary, never-sent value).
                *last = u64::MAX;
                self.queue.ExecuteCommandLists(&[Some(commands)]);
                self.queue.Signal(&self.copied.0, value)?;
                *last = value;
                // Releasing the frame to DWM, and D3D11 readers of the copy,
                // come after it. Submitted now, so release cannot overtake it.
                self.context.Wait(&self.copied.1, value)?;
                self.context.Flush();
                Ok(())
            })();
            submitted.inspect_err(|_| self.compute.retire())?;
        }
        Ok(Ready {
            fence: self.copied.0.clone(),
            value,
            device: self.compute.device.as_raw() as usize,
            readers: Some(readers),
            reader_fence: Some(self.compute.fence.clone()),
        })
    }
    /// A command list the copy queue has finished with.
    fn list(&mut self) -> Result<usize> {
        // SAFETY: `copied.0` is a live D3D12 fence owned by this handoff; GetCompletedValue only
        // reads it.
        let done = completed_value(unsafe { self.copied.0.GetCompletedValue() })?;
        if let Some(index) = self.lists.iter().position(|(_, _, last, _)| *last <= done) {
            return Ok(index);
        }
        if self.lists.len() < 8 {
            // SAFETY: `compute.device` is a live D3D12 device, and the new list records into the
            // allocator created with it just before.
            unsafe {
                let allocator: ID3D12CommandAllocator = self
                    .compute
                    .device
                    .CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_COMPUTE)?;
                let list: ID3D12GraphicsCommandList = self.compute.device.CreateCommandList(
                    0,
                    D3D12_COMMAND_LIST_TYPE_COMPUTE,
                    &allocator,
                    None,
                )?;
                list.Close()?;
                self.lists.push((allocator, list, 0, [None, None]));
            }
            return Ok(self.lists.len() - 1);
        }
        // Eight frames behind: DWM's copies are not finishing.
        bail!("capture copies stopped completing")
    }
}
impl Drop for Handoff {
    fn drop(&mut self) {
        // A final marker covers a copy whose normal Signal failed as well.
        // Keep all command storage and both sides of shared fences until it
        // completes; an expired CPU wait is never permission to free them.
        let mut work = RetainedWork::new(&self.compute, &self.queue);
        work.keep(&self.acquired.0);
        work.keep(&self.acquired.1);
        work.keep(&self.copied.0);
        work.keep(&self.copied.1);
        work.keep(&self.context);
        for (allocator, list, _, resources) in &self.lists {
            work.keep(allocator);
            work.keep(list);
            for resource in resources.iter().flatten() {
                work.keep(resource);
            }
        }
        for (texture, resource) in self.imported.values() {
            work.keep(texture);
            work.keep(resource);
        }
        for (texture, _) in self.readers.values() {
            work.keep(texture);
            self.compute.forget(texture);
        }
        self.compute
            .opened
            .lock()
            .unwrap()
            .retain(|key, _| !self.opened.contains(key));
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

fn shader(entry: &[u8]) -> Result<&'static [u8]> {
    shader_bytecode(entry, b"cs_5_0\0").context("compute shader missing from the build")
}
/// Constants, two source views (t0, t1) and `uavs` output views from
/// register `u{first}`, no output table for 0.
fn root_signature(device: &ID3D12Device, first: u32, uavs: u32) -> Result<ID3D12RootSignature> {
    root_signature_with(device, first, uavs, &[])
}
/// [`root_signature`] followed by `extra` parameters, from parameter 3 (2
/// without outputs).
fn root_signature_with(
    device: &ID3D12Device,
    first: u32,
    uavs: u32,
    extra: &[D3D12_ROOT_PARAMETER],
) -> Result<ID3D12RootSignature> {
    // SAFETY: `srv`, `uav` and `parameters` outlive the serialize call that reads them, and the
    // slice covers exactly the blob's GetBufferSize bytes while `blob` is alive.
    unsafe {
        let srv = D3D12_DESCRIPTOR_RANGE {
            RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_SRV,
            NumDescriptors: 2,
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
        let mut parameters = vec![
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
        ];
        if uavs > 0 {
            parameters.push(D3D12_ROOT_PARAMETER {
                ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
                Anonymous: D3D12_ROOT_PARAMETER_0 {
                    DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE {
                        NumDescriptorRanges: 1,
                        pDescriptorRanges: &uav,
                    },
                },
                ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
            });
        }
        parameters.extend_from_slice(extra);
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
    /// The compute queue's fence value after this conversion: its focus
    /// map's [`MapView::value`], a different timeline from `value`.
    pub conversion: u64,
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
    /// The source, cursor and panel image the slot's last conversion reads,
    /// kept until it has run: the adapter-wide cache may release its own
    /// references.
    inputs: [Option<ID3D12Resource>; 3],
    capture: Option<crate::capture::GpuImage>,
    outputs: Vec<ID3D12Resource>,
    waits: Vec<ID3D12Fence>,
    /// The HDR statistics pass reading the source after the conversion
    /// (`compute_hdr_stats.rs`): 0 for none, [`hdr_stats::QUEUED`] until it is
    /// submitted, `u64::MAX` while it is.
    stats: u64,
}
impl Slot {
    /// When the slot's submitted lists stop reading its inputs and views.
    fn last(&self) -> u64 {
        match self.stats {
            hdr_stats::QUEUED => self.fence,
            stats => self.fence.max(stats),
        }
    }
}
struct PointerUpload {
    fence: u64,
    _allocator: ID3D12CommandAllocator,
    _list: ID3D12GraphicsCommandList,
    _upload: ID3D12Resource,
    _texture: ID3D12Resource,
}
/// RGB to NV12 or P010 on the compute queue, with the same shader as the
/// D3D11 converter (scaling, letterboxing, HDR and the colour matrix).
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
    /// Bounded cache of immutable pointer textures for recurring animation shapes.
    pointers: crate::cursor::ShapeCache<ID3D12Resource>,
    uploads: Vec<PointerUpload>,
    /// Encoder focus, once [`Converter::enable_focus`] made it.
    focus: Option<Box<Focus>>,
    /// `pack_chroma` replaced `convert`, which focus cannot follow.
    packs_chroma: bool,
    /// The couch menu's pass, while an overlay is set.
    menu: menu::Menu,
    /// HDR statistics, once [`Converter::enable_hdr_stats`] made them.
    hdr_stats: Option<Box<hdr_stats::HdrStats>>,
}
// SAFETY: `mapped` and the focus and HDR statistics readback pointers point into buffers this
// converter owns, the D3D12 objects are free-threaded, and the command allocators and lists are
// used only through `&mut self`.
unsafe impl Send for Converter {}
const SLOTS: usize = 4;
/// Descriptors per slot: the source and pointer, up to three outputs, then
/// the couch menu's panel image ([`menu::VIEW`]).
const VIEWS: usize = 6;

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

impl Converter {
    pub(crate) fn pack_chroma(&mut self) -> Result<()> {
        anyhow::ensure!(
            self.focus.is_none(),
            "encoder focus does not support packed 4:4:4"
        );
        self.convert = pipeline(&self.compute.device, &self.root, b"chroma_pack_cs\0")?;
        self.packs_chroma = true;
        Ok(())
    }
    /// Probe both plane view formats and the actual planar allocation before
    /// AMF initializes its D3D12 context, so unsupported devices fall back once.
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
        // `constants` (SLOTS * 512 bytes) stays mapped at `mapped` until Unmap in Drop.
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
            let mut constants: Option<ID3D12Resource> = None;
            device.CreateCommittedResource(
                &D3D12_HEAP_PROPERTIES {
                    Type: D3D12_HEAP_TYPE_UPLOAD,
                    ..Default::default()
                },
                D3D12_HEAP_FLAG_NONE,
                // `Config` per slot, then `cbuffer Menu` per slot.
                &buffer_desc((SLOTS * 512) as u64),
                D3D12_RESOURCE_STATE_GENERIC_READ,
                None,
                &mut constants,
            )?;
            let constants = constants.context("no constant buffer")?;
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
                    inputs: [None, None, None],
                    capture: None,
                    outputs: Vec::new(),
                    waits: Vec::new(),
                    stats: 0,
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
                pointers: Default::default(),
                uploads: Vec::new(),
                focus: None,
                packs_chroma: false,
                menu: Default::default(),
                hdr_stats: None,
            })
        }
    }
    /// The pointer shape as a texture on this device, uploaded when it
    /// changes. The conversion follows the upload on the same queue.
    pub(crate) fn pointer(&mut self, cursor: &crate::cursor::Cursor) -> Result<ID3D12Resource> {
        self.compute.ensure_active()?;
        self.release_completed();
        if let Some(texture) =
            self.pointers
                .get(cursor.width, cursor.height, cursor.logic, &cursor.pixels)
        {
            return Ok(texture);
        }
        // SAFETY: `cursor.pixels` holds `width * height` RGBA pixels and `upload` `pitch * height`
        // bytes, so each row copy stays in both; `uploads` retains the copy's objects to its fence.
        unsafe {
            let device = &self.compute.device;
            let desc = D3D12_RESOURCE_DESC {
                Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
                Width: u64::from(cursor.width),
                Height: cursor.height,
                DepthOrArraySize: 1,
                MipLevels: 1,
                Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
                Flags: D3D12_RESOURCE_FLAG_NONE,
                Alignment: 0,
            };
            let mut texture: Option<ID3D12Resource> = None;
            device.CreateCommittedResource(
                &D3D12_HEAP_PROPERTIES {
                    Type: D3D12_HEAP_TYPE_DEFAULT,
                    ..Default::default()
                },
                D3D12_HEAP_FLAG_NONE,
                &desc,
                D3D12_RESOURCE_STATE_COMMON,
                None,
                &mut texture,
            )?;
            let texture = texture.context("no pointer texture")?;
            let pitch = (cursor.width * 4).div_ceil(256) * 256;
            let mut upload: Option<ID3D12Resource> = None;
            device.CreateCommittedResource(
                &D3D12_HEAP_PROPERTIES {
                    Type: D3D12_HEAP_TYPE_UPLOAD,
                    ..Default::default()
                },
                D3D12_HEAP_FLAG_NONE,
                &buffer_desc(u64::from(pitch * cursor.height)),
                D3D12_RESOURCE_STATE_GENERIC_READ,
                None,
                &mut upload,
            )?;
            let upload = upload.context("no pointer upload buffer")?;
            let mut mapped = std::ptr::null_mut();
            upload.Map(0, None, Some(&mut mapped))?;
            let row = (cursor.width * 4) as usize;
            for y in 0..cursor.height as usize {
                std::ptr::copy_nonoverlapping(
                    cursor.pixels.as_ptr().add(y * row),
                    mapped.cast::<u8>().add(y * pitch as usize),
                    row.min(cursor.pixels.len().saturating_sub(y * row)),
                );
            }
            upload.Unmap(0, None);
            let allocator: ID3D12CommandAllocator =
                device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_COMPUTE)?;
            let list: ID3D12GraphicsCommandList =
                device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_COMPUTE, &allocator, None)?;
            let destination = D3D12_TEXTURE_COPY_LOCATION {
                pResource: std::mem::ManuallyDrop::new(Some(texture.clone())),
                Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                    SubresourceIndex: 0,
                },
            };
            let source = D3D12_TEXTURE_COPY_LOCATION {
                pResource: std::mem::ManuallyDrop::new(Some(upload.clone())),
                Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                    PlacedFootprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
                        Offset: 0,
                        Footprint: D3D12_SUBRESOURCE_FOOTPRINT {
                            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                            Width: cursor.width,
                            Height: cursor.height,
                            Depth: 1,
                            RowPitch: pitch,
                        },
                    },
                },
            };
            list.CopyTextureRegion(&destination, 0, 0, 0, &source, None);
            drop(std::mem::ManuallyDrop::into_inner(destination.pResource));
            drop(std::mem::ManuallyDrop::into_inner(source.pResource));
            // This plain texture was implicitly promoted to COPY_DEST on a compute
            // queue, so it does not decay to COMMON after the upload. Every later
            // use samples the cached cursor; transition once before its first read.
            let mut barriers = [transition(
                &texture,
                D3D12_RESOURCE_STATE_COPY_DEST,
                D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE,
            )];
            list.ResourceBarrier(&barriers);
            drop_barriers(&mut barriers);
            list.Close()?;
            self.uploads.push(PointerUpload {
                // Own every submitted object before ExecuteCommandLists: its
                // following Signal may fail after work has already been queued.
                fence: u64::MAX,
                _allocator: allocator,
                _list: list.clone(),
                _upload: upload,
                _texture: texture.clone(),
            });
            let done = self.compute.execute(&list)?;
            self.uploads.last_mut().unwrap().fence = done;
            self.pointers.insert(
                cursor.width,
                cursor.height,
                cursor.logic,
                cursor.pixels.clone(),
                texture.clone(),
            );
            Ok(texture)
        }
    }
    pub fn compute(&self) -> &Arc<Compute> {
        &self.compute
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
    pub(crate) fn release_completed(&mut self) {
        self.uploads
            .retain(|upload| !self.compute.completed(upload.fence));
        for slot in &mut self.slots {
            // A queued statistics pass still reads the capture once submitted.
            if slot.stats != hdr_stats::QUEUED && self.compute.completed(slot.last()) {
                slot.capture = None;
                slot.inputs = [None, None, None];
                slot.outputs.clear();
                slot.waits.clear();
            }
        }
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
    /// Copy `values` into `slot`'s 256-byte part of the constant buffer.
    ///
    /// # Safety
    /// The slot's last conversion has finished, so the GPU no longer reads it.
    unsafe fn write_values(&self, slot: usize) {
        // SAFETY: `mapped` stays mapped for the converter's life and holds `SLOTS` 256-byte
        // slots; the 80 bytes of `values` fit one, and the caller keeps the GPU off it.
        unsafe {
            std::ptr::copy_nonoverlapping(
                self.values.as_ptr().cast::<u8>(),
                self.mapped.add(slot * 256),
                std::mem::size_of_val(&self.values),
            );
        }
    }
    /// Order this queue after the capture's copy behind `ready`: on the GPU
    /// when its fence is on this queue's device, else on the CPU.
    fn wait_capture(&self, ready: Option<&Ready>) -> Result<()> {
        match ready {
            // SAFETY: The guard checked that `ready.fence` belongs to this queue's device, and
            // `ready` holds it for the call.
            Some(ready) if ready.device == self.compute.device.as_raw() as usize => unsafe {
                self.compute
                    .queue
                    .Wait(&ready.fence, ready.value)
                    .inspect_err(|_| self.compute.retire())?;
            },
            Some(ready) => ready.wait()?,
            None => {}
        }
        Ok(())
    }
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
        // SAFETY: `compute.device` is a live D3D12 device and the description and out-parameter
        // outlive the call.
        unsafe {
            let mut texture: Option<ID3D12Resource> = None;
            self.compute
                .device
                .CreateCommittedResource(
                    &D3D12_HEAP_PROPERTIES {
                        Type: D3D12_HEAP_TYPE_DEFAULT,
                        ..Default::default()
                    },
                    D3D12_HEAP_FLAG_NONE,
                    &D3D12_RESOURCE_DESC {
                        Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
                        Width: u64::from(self.width),
                        Height: self.height,
                        DepthOrArraySize: 1,
                        MipLevels: 1,
                        Format: if self.ten_bit {
                            DXGI_FORMAT_P010
                        } else {
                            DXGI_FORMAT_NV12
                        },
                        SampleDesc: DXGI_SAMPLE_DESC {
                            Count: 1,
                            Quality: 0,
                        },
                        Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
                        Flags: D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
                        Alignment: 0,
                    },
                    D3D12_RESOURCE_STATE_COMMON,
                    None,
                    &mut texture,
                )
                .context("YUV texture on the compute device")?;
            let texture = texture.context("no YUV texture")?;
            crate::amf::gpu::set_resource_state(&texture, D3D12_RESOURCE_STATE_COMMON)?;
            self.targets.push(Target {
                texture: Arc::new(texture),
                fence: self.compute.device.CreateFence(0, D3D12_FENCE_FLAG_NONE)?,
                value: 0,
            });
            Ok(self.targets.len() - 1)
        }
    }
    /// Convert `source` (an RGB texture opened on this device) into a new
    /// YUV texture. The values follow the D3D11 converter's constants.
    pub fn convert(
        &mut self,
        source: &ID3D12Resource,
        format: DXGI_FORMAT,
        pointer: Option<&ID3D12Resource>,
        image: &crate::capture::GpuImage,
    ) -> Result<Converted> {
        self.compute.ensure_active()?;
        validate_colour_input(&self.values)?;
        let ready = image.ready.as_ref();
        let index = self.target()?;
        let target = self.targets[index].texture.clone();
        // AMF updates this private data when it reads a submitted texture. Its
        // native surface release and completion fence make the selected target
        // free; neither returns its resource state to COMMON.
        let state = crate::amf::gpu::resource_state(&target)?;
        ensure_compute_texture_state(state)?;
        let slot = self.slot;
        self.slot = (self.slot + 1) % SLOTS;
        self.compute.wait(self.slots[slot].last())?;
        self.slots[slot].capture = None;
        self.reset_hdr_stats(slot);
        let (entry, block) = match (self.packs_chroma, self.values[11]) {
            (false, _) => (&b"yuv420_menu_cs\0"[..], [2, 2]),
            (true, 2) => (&b"chroma_pack_menu_cs\0"[..], [2, 2]),
            (true, _) => (&b"chroma_pack_menu_cs\0"[..], [2, 1]),
        };
        let menu = self.menu_pass(slot, entry, 0, 2, block)?;
        // SAFETY: The slot's last conversion has finished (waited above), so its constants and
        // views may be rewritten, and every view and barrier names a live resource on this device.
        unsafe {
            self.write_values(slot);
            let device = &self.compute.device;
            let cpu = self.heap.GetCPUDescriptorHandleForHeapStart();
            let gpu = self.heap.GetGPUDescriptorHandleForHeapStart();
            let at = |index: usize| D3D12_CPU_DESCRIPTOR_HANDLE {
                ptr: cpu.ptr + (slot * VIEWS + index) * self.increment as usize,
            };
            device.CreateShaderResourceView(source, Some(&texture_srv(format)), at(0));
            device.CreateShaderResourceView(
                pointer,
                Some(&texture_srv(DXGI_FORMAT_R8G8B8A8_UNORM)),
                at(1),
            );
            let (luma_format, chroma_format) = if self.ten_bit {
                (DXGI_FORMAT_R16_UNORM, DXGI_FORMAT_R16G16_UNORM)
            } else {
                (DXGI_FORMAT_R8_UNORM, DXGI_FORMAT_R8G8_UNORM)
            };
            device.CreateUnorderedAccessView(
                target.as_ref(),
                None,
                Some(&texture_uav(luma_format, 0)),
                at(2),
            );
            device.CreateUnorderedAccessView(
                target.as_ref(),
                None,
                Some(&texture_uav(chroma_format, 1)),
                at(3),
            );
            let Slot {
                allocator, list, ..
            } = &self.slots[slot];
            let focus = self
                .focus
                .as_deref()
                .filter(|focus| focus.settings.is_some());
            let (root, convert) = focus.map_or((&self.root, &self.convert), |focus| {
                (&focus.root, &focus.convert)
            });
            allocator.Reset()?;
            list.Reset(allocator, convert)?;
            if let Some(menu) = &menu {
                menu.copy(list);
            }
            let mut barriers = [transition(
                target.as_ref(),
                state,
                D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
            )];
            list.ResourceBarrier(&barriers);
            drop_barriers(&mut barriers);
            list.SetComputeRootSignature(root);
            list.SetDescriptorHeaps(&[Some(self.heap.clone())]);
            list.SetComputeRootConstantBufferView(
                0,
                self.constants.GetGPUVirtualAddress() + (slot * 256) as u64,
            );
            let table = |index: usize| D3D12_GPU_DESCRIPTOR_HANDLE {
                ptr: gpu.ptr + ((slot * VIEWS + index) * self.increment as usize) as u64,
            };
            list.SetComputeRootDescriptorTable(1, table(0));
            list.SetComputeRootDescriptorTable(2, table(2));
            if let Some(focus) = focus {
                focus.bind(list);
            }
            list.Dispatch(
                (self.width / 2).div_ceil(8),
                (self.height / 2).div_ceil(8),
                1,
            );
            if let Some(focus) = focus {
                focus.record(list, slot);
            }
            if let Some(menu) = &menu {
                menu.record(list, self.constants.GetGPUVirtualAddress(), slot, table);
            }
            let mut barriers = [transition(
                target.as_ref(),
                D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                D3D12_RESOURCE_STATE_COMMON,
            )];
            list.ResourceBarrier(&barriers);
            drop_barriers(&mut barriers);
            list.Close()?;
        }
        // The slot's previous conversion has finished (waited above), so its
        // inputs can go; this one's stay until the slot comes round again.
        self.slots[slot].inputs = [
            Some(source.clone()),
            pointer.cloned(),
            menu.as_ref().map(|menu| menu.texture.clone()),
        ];
        self.slots[slot].capture = Some(image.clone());
        self.slots[slot].outputs.clear();
        self.slots[slot].waits = ready.map(|ready| ready.fence.clone()).into_iter().collect();
        self.slots[slot].fence = u64::MAX;
        // The capture's copy into `source` may still be waiting for DWM.
        self.compute.ensure_active()?;
        self.wait_capture(ready)?;
        let fence = self
            .compute
            .execute(&self.slots[slot].list)
            .inspect_err(|_| self.compute.retire())?;
        self.slots[slot].fence = fence;
        if let Some(menu) = &menu {
            self.menu_submitted(menu, fence);
        }
        // Only now, so a failed wait or submission leaves the age reset pending.
        if let Some(focus) = self.focus.as_deref_mut() {
            focus.recorded(slot);
        }
        if let Some(ready) = ready {
            ready.read_by(&self.compute, fence)?;
        }
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
        let converted = Converted {
            texture: target.texture.clone(),
            fence: target.fence.clone(),
            value: target.value,
            conversion: fence,
        };
        self.queue_hdr_stats(slot, fence);
        Ok(converted)
    }
    /// Convert `source` into PyroWave's Y, Cb and Cr planes, full size or
    /// with chroma halved (`values[11]`), in textures the encoder shares
    /// with Vulkan. The queue first waits for `target.fence` to reach
    /// `target.after`, when the encoder is done with the planes, then
    /// signals `target.done`.
    pub fn convert_planes(
        &mut self,
        source: &ID3D12Resource,
        format: DXGI_FORMAT,
        pointer: Option<&ID3D12Resource>,
        ready: Option<&Ready>,
        target: &Planes,
        image: Option<&crate::capture::GpuImage>,
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
        let slot = self.slot;
        self.slot = (self.slot + 1) % SLOTS;
        self.compute.wait(self.slots[slot].last())?;
        self.slots[slot].capture = None;
        self.reset_hdr_stats(slot);
        let menu = self.menu_pass(slot, b"pyro_menu_cs\0", 2, 3, [2, 2])?;
        self.slots[slot].inputs = [
            Some(source.clone()),
            pointer.cloned(),
            menu.as_ref().map(|menu| menu.texture.clone()),
        ];
        self.slots[slot].capture = image.cloned();
        self.slots[slot].outputs = target.planes.to_vec();
        self.slots[slot].waits = ready
            .map(|ready| ready.fence.clone())
            .into_iter()
            .chain(std::iter::once(target.fence.clone()))
            .collect();
        self.slots[slot].fence = u64::MAX;
        // The planes carry no map, so the slot's old one must not look like theirs.
        if let Some(focus) = self.focus.as_deref_mut() {
            focus.written[slot] = false;
        }
        // SAFETY: As in `convert`, the slot is idle; `target`'s planes are on `compute.device`.
        unsafe {
            self.write_values(slot);
            let device = &self.compute.device;
            let cpu = self.heap.GetCPUDescriptorHandleForHeapStart();
            let gpu = self.heap.GetGPUDescriptorHandleForHeapStart();
            let at = |index: usize| D3D12_CPU_DESCRIPTOR_HANDLE {
                ptr: cpu.ptr + (slot * VIEWS + index) * self.increment as usize,
            };
            device.CreateShaderResourceView(source, Some(&texture_srv(format)), at(0));
            device.CreateShaderResourceView(
                pointer,
                Some(&texture_srv(DXGI_FORMAT_R8G8B8A8_UNORM)),
                at(1),
            );
            for (index, plane) in target.planes.iter().enumerate() {
                device.CreateUnorderedAccessView(
                    plane,
                    None,
                    Some(&texture_uav(target.format, 0)),
                    at(2 + index),
                );
            }
            let Slot {
                allocator, list, ..
            } = &self.slots[slot];
            allocator.Reset()?;
            list.Reset(allocator, &planar)?;
            if let Some(menu) = &menu {
                menu.copy(list);
            }
            let mut barriers = target.planes.each_ref().map(|plane| {
                transition(
                    plane,
                    D3D12_RESOURCE_STATE_COMMON,
                    D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                )
            });
            list.ResourceBarrier(&barriers);
            drop_barriers(&mut barriers);
            list.SetComputeRootSignature(&root);
            list.SetDescriptorHeaps(&[Some(self.heap.clone())]);
            list.SetComputeRootConstantBufferView(
                0,
                self.constants.GetGPUVirtualAddress() + (slot * 256) as u64,
            );
            let table = |index: usize| D3D12_GPU_DESCRIPTOR_HANDLE {
                ptr: gpu.ptr + ((slot * VIEWS + index) * self.increment as usize) as u64,
            };
            list.SetComputeRootDescriptorTable(1, table(0));
            list.SetComputeRootDescriptorTable(2, table(2));
            list.Dispatch(
                (self.width / 2).div_ceil(8),
                (self.height / 2).div_ceil(8),
                1,
            );
            if let Some(menu) = &menu {
                menu.record(list, self.constants.GetGPUVirtualAddress(), slot, table);
            }
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
        self.compute.ensure_active()?;
        self.wait_capture(ready)?;
        // SAFETY: `target.fence` is opened on this queue's device and outlives the call.
        unsafe { self.compute.queue.Wait(&target.fence, target.after) }
            .inspect_err(|_| self.compute.retire())?;
        self.slots[slot].fence = self
            .compute
            .execute(&self.slots[slot].list)
            .inspect_err(|_| self.compute.retire())?;
        if let Some(menu) = &menu {
            self.menu_submitted(menu, self.slots[slot].fence);
        }
        if let Some(ready) = ready {
            ready.read_by(&self.compute, self.slots[slot].fence)?;
        }
        // SAFETY: `target.fence` is opened on this queue's device and outlives the call.
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
        let mut work = RetainedWork::new(&self.compute, &self.compute.queue);
        work.keep(&self.root);
        work.keep(&self.convert);
        if let Some((root, pipeline)) = &self.planar {
            work.keep(root);
            work.keep(pipeline);
        }
        work.keep(&self.heap);
        work.keep(&self.constants);
        for pointer in self.pointers.values() {
            work.keep(pointer);
        }
        for slot in &mut self.slots {
            work.keep(&slot.allocator);
            work.keep(&slot.list);
            for resource in slot.inputs.iter().flatten().chain(&slot.outputs) {
                work.keep(resource);
            }
            for fence in &slot.waits {
                work.keep(fence);
            }
            if let Some(capture) = slot.capture.take() {
                work.captures.push(capture);
            }
        }
        for upload in &self.uploads {
            work.keep(&upload._allocator);
            work.keep(&upload._list);
            work.keep(&upload._upload);
            work.keep(&upload._texture);
        }
        for target in &self.targets {
            work.keep(target.texture.as_ref());
            work.waits.push((target.fence.clone(), target.last()));
        }
        if let Some(focus) = &self.focus {
            focus.retain(&mut work);
        }
        self.menu.retain(&mut work);
        if let Some(stats) = &self.hdr_stats {
            stats.retain(&mut work);
        }
        work.finish(&self.compute);
        // SAFETY: `constants` was mapped once in `new`, and nothing writes through `mapped` after
        // this. Unmap ends CPU access; retained GPU work still owns the resource.
        unsafe { self.constants.Unmap(0, None) };
        if let Some(focus) = &self.focus {
            focus.unmap();
        }
        if let Some(stats) = &self.hdr_stats {
            stats.unmap();
        }
    }
}
// Encoder focus (docs/encoder-focus-design.md): the conversion also keeps a
// still-age per 16x16 block and writes the codec blocks' ROI importances,
// read back without a CPU wait. Off, `convert` records exactly what it did.

/// What the focus passes use for the next conversions.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FocusSettings {
    pub mode: butterpollo_core::encoder_focus::Mode,
    pub levels: butterpollo_core::encoder_focus::Levels,
    /// The cursor in target pixels, while it is in the picture.
    pub cursor: Option<(i32, i32)>,
}
/// A finished conversion's ROI map, one u32 importance per codec block.
pub struct MapView<'a> {
    /// The compute fence value of the conversion that wrote it.
    pub value: u64,
    /// Every block has the same importance, so the map would change nothing.
    pub uniform: bool,
    pub rows: &'a [u32],
    pub width: u32,
    pub height: u32,
}
/// The still-age detector's and map's GPU objects.
struct Focus {
    root: ID3D12RootSignature,
    convert: ID3D12PipelineState,
    map: ID3D12PipelineState,
    /// `{signature, age}` per 16x16 block.
    ages: ID3D12Resource,
    /// `{min, max}`, then the importances.
    importances: ID3D12Resource,
    /// One persistently mapped READBACK copy of `importances` per slot.
    readback: Vec<(ID3D12Resource, *const u32)>,
    block: u32,
    age_grid: (u32, u32),
    grid: (u32, u32),
    settings: Option<FocusSettings>,
    /// Whether each slot's last conversion wrote a map.
    written: [bool; SLOTS],
    /// The next focus conversion starts every age at 0.
    reset: bool,
}
/// Root parameters 3 to 5 of the focus root signature: the ages (u5), the
/// map (u6) and the `Focus` constants (b1).
fn focus_parameters() -> [D3D12_ROOT_PARAMETER; 3] {
    let uav = |register| D3D12_ROOT_PARAMETER {
        ParameterType: D3D12_ROOT_PARAMETER_TYPE_UAV,
        Anonymous: D3D12_ROOT_PARAMETER_0 {
            Descriptor: D3D12_ROOT_DESCRIPTOR {
                ShaderRegister: register,
                RegisterSpace: 0,
            },
        },
        ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
    };
    [
        uav(5),
        uav(6),
        D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
            Anonymous: D3D12_ROOT_PARAMETER_0 {
                Constants: D3D12_ROOT_CONSTANTS {
                    ShaderRegister: 1,
                    RegisterSpace: 0,
                    Num32BitValues: 16,
                },
            },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
        },
    ]
}
fn focus_buffer(
    device: &ID3D12Device,
    heap: D3D12_HEAP_TYPE,
    size: u64,
    state: D3D12_RESOURCE_STATES,
) -> Result<ID3D12Resource> {
    let mut desc = buffer_desc(size);
    if heap == D3D12_HEAP_TYPE_DEFAULT {
        desc.Flags = D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS;
    }
    let mut buffer: Option<ID3D12Resource> = None;
    // SAFETY: `device` is live and the descriptions and out-parameter outlive the call.
    unsafe {
        device.CreateCommittedResource(
            &D3D12_HEAP_PROPERTIES {
                Type: heap,
                ..Default::default()
            },
            D3D12_HEAP_FLAG_NONE,
            &desc,
            state,
            None,
            &mut buffer,
        )?;
    }
    buffer.context("no encoder focus buffer")
}
fn uav_barrier() -> D3D12_RESOURCE_BARRIER {
    D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_UAV,
        Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            UAV: std::mem::ManuallyDrop::new(D3D12_RESOURCE_UAV_BARRIER {
                pResource: std::mem::ManuallyDrop::new(None),
            }),
        },
    }
}
impl Focus {
    fn map_bytes(&self) -> u64 {
        4 * (2 + u64::from(self.grid.0) * u64::from(self.grid.1))
    }
    fn constants(&self) -> [u32; 16] {
        let settings = self.settings.expect("focus constants without settings");
        let levels = settings.levels;
        let cursor = settings
            .cursor
            .map(butterpollo_core::encoder_focus::clamp_cursor);
        let (x, y) = cursor.unwrap_or_default();
        [
            settings.mode.shader(),
            levels.moving,
            levels.still,
            levels.ramp,
            levels.centre,
            levels.ctb_still_256(),
            levels.noise_bits.min(8),
            self.block,
            self.age_grid.0,
            self.age_grid.1,
            self.grid.0,
            self.grid.1,
            x as u32,
            y as u32,
            u32::from(cursor.is_some()),
            u32::from(self.reset),
        ]
    }
    /// Bind the ages, the map and the constants for `yuv420_focus_cs`.
    ///
    /// # Safety
    /// `list` is open, with the focus root signature set.
    unsafe fn bind(&self, list: &ID3D12GraphicsCommandList) {
        let constants = self.constants();
        // SAFETY: Both buffers are live and in COMMON between conversions; the constants
        // outlive the call that copies them into the list.
        unsafe {
            let mut barriers = [
                transition(
                    &self.ages,
                    D3D12_RESOURCE_STATE_COMMON,
                    D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                ),
                transition(
                    &self.importances,
                    D3D12_RESOURCE_STATE_COMMON,
                    D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                ),
            ];
            list.ResourceBarrier(&barriers);
            drop_barriers(&mut barriers);
            list.SetComputeRootUnorderedAccessView(3, self.ages.GetGPUVirtualAddress());
            list.SetComputeRootUnorderedAccessView(4, self.importances.GetGPUVirtualAddress());
            list.SetComputeRoot32BitConstants(5, 16, constants.as_ptr().cast(), 0);
        }
    }
    /// After the conversion's dispatch: the map pass and its copy into the
    /// slot's readback buffer, so the slot's fence covers both.
    ///
    /// # Safety
    /// `list` is open and [`Focus::bind`] recorded into it.
    unsafe fn record(&self, list: &ID3D12GraphicsCommandList, slot: usize) {
        // SAFETY: The buffers are live, in the states named, and the copy fits both.
        unsafe {
            list.ResourceBarrier(&[uav_barrier()]);
            list.SetPipelineState(&self.map);
            list.Dispatch(self.grid.0.div_ceil(8), self.grid.1.div_ceil(8), 1);
            let mut barriers = [
                transition(
                    &self.ages,
                    D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                    D3D12_RESOURCE_STATE_COMMON,
                ),
                transition(
                    &self.importances,
                    D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                    D3D12_RESOURCE_STATE_COPY_SOURCE,
                ),
            ];
            list.ResourceBarrier(&barriers);
            drop_barriers(&mut barriers);
            list.CopyBufferRegion(
                &self.readback[slot].0,
                0,
                &self.importances,
                0,
                self.map_bytes(),
            );
            let mut barriers = [transition(
                &self.importances,
                D3D12_RESOURCE_STATE_COPY_SOURCE,
                D3D12_RESOURCE_STATE_COMMON,
            )];
            list.ResourceBarrier(&barriers);
            drop_barriers(&mut barriers);
        }
    }
    /// `slot`'s list is recorded and about to run.
    fn recorded(&mut self, slot: usize) {
        self.written[slot] = self.settings.is_some();
        self.reset &= !self.written[slot];
    }
    fn retain(&self, work: &mut RetainedWork) {
        work.keep(&self.root);
        work.keep(&self.convert);
        work.keep(&self.map);
        work.keep(&self.ages);
        work.keep(&self.importances);
        for (buffer, _) in &self.readback {
            work.keep(buffer);
        }
    }
    fn unmap(&self) {
        for (buffer, _) in &self.readback {
            // SAFETY: Each buffer was mapped once in `enable_focus`; nothing reads it after this.
            unsafe { buffer.Unmap(0, None) };
        }
    }
}
impl Converter {
    /// Make the encoder focus passes for codec blocks of `block` (16 or 64)
    /// pixels. Off until [`Converter::set_focus`].
    pub fn enable_focus(&mut self, block: u32) -> Result<()> {
        anyhow::ensure!(
            block == 16 || block == 64,
            "encoder focus block {block} is not 16 or 64"
        );
        anyhow::ensure!(
            !self.packs_chroma,
            "encoder focus does not support packed 4:4:4"
        );
        if let Some(focus) = &self.focus {
            anyhow::ensure!(
                focus.block == block,
                "encoder focus already uses {} pixel blocks",
                focus.block
            );
            return Ok(());
        }
        self.compute.ensure_active()?;
        let age_grid = butterpollo_core::encoder_focus::age_grid(self.width, self.height);
        let grid = (self.width.div_ceil(block), self.height.div_ceil(block));
        let device = &self.compute.device;
        let root = root_signature_with(device, 0, 2, &focus_parameters())?;
        let convert = pipeline(device, &root, b"yuv420_focus_cs\0")?;
        let map = pipeline(device, &root, b"focus_map_cs\0")?;
        let ages = focus_buffer(
            device,
            D3D12_HEAP_TYPE_DEFAULT,
            8 * u64::from(age_grid.0) * u64::from(age_grid.1),
            D3D12_RESOURCE_STATE_COMMON,
        )?;
        let map_bytes = 4 * (2 + u64::from(grid.0) * u64::from(grid.1));
        let importances = focus_buffer(
            device,
            D3D12_HEAP_TYPE_DEFAULT,
            map_bytes,
            D3D12_RESOURCE_STATE_COMMON,
        )?;
        let mut readback = Vec::with_capacity(SLOTS);
        for _ in 0..SLOTS {
            let buffer = focus_buffer(
                device,
                D3D12_HEAP_TYPE_READBACK,
                map_bytes,
                D3D12_RESOURCE_STATE_COPY_DEST,
            )?;
            let mut mapped = std::ptr::null_mut();
            // SAFETY: A READBACK buffer stays mapped for its life; `unmap` ends it in Drop.
            unsafe { buffer.Map(0, None, Some(&mut mapped))? };
            readback.push((buffer, mapped.cast_const().cast::<u32>()));
        }
        self.focus = Some(Box::new(Focus {
            root,
            convert,
            map,
            ages,
            importances,
            readback,
            block,
            age_grid,
            grid,
            settings: None,
            written: [false; SLOTS],
            reset: true,
        }));
        Ok(())
    }
    /// Switch the focus passes on with `settings`, or off with `None`. Ages
    /// restart when focus comes back on, and maps from before a switch are
    /// never returned.
    pub fn set_focus(&mut self, settings: Option<FocusSettings>) -> Result<()> {
        let focus = match (&mut self.focus, settings) {
            (Some(focus), _) => focus,
            (None, None) => return Ok(()),
            (None, Some(_)) => bail!("encoder focus was not enabled on this converter"),
        };
        if focus.settings.is_some() != settings.is_some() {
            focus.written = [false; SLOTS];
            focus.reset = true;
        }
        focus.settings = settings;
        Ok(())
    }
    /// The newest map whose conversion has finished and whose fence value is
    /// above `newer_than`. Only asks the fence: never waits.
    pub fn latest_map(&self, newer_than: u64) -> Option<MapView<'_>> {
        let focus = self.focus.as_deref()?;
        let (slot, value) = (0..SLOTS)
            .filter(|&slot| focus.written[slot])
            .map(|slot| (slot, self.slots[slot].fence))
            .filter(|&(_, value)| {
                value != u64::MAX && value > newer_than && self.compute.completed(value)
            })
            .max_by_key(|&(_, value)| value)?;
        let (width, height) = focus.grid;
        // SAFETY: The slot's copy has completed and the next one needs `&mut self`
        // (`convert`), so the mapped words stay unchanged while this borrow lives.
        let words = unsafe {
            std::slice::from_raw_parts(focus.readback[slot].1, 2 + width as usize * height as usize)
        };
        Some(MapView {
            value,
            uniform: words[0] >= words[1],
            rows: &words[2..],
            width,
            height,
        })
    }
}
#[cfg(test)]
#[path = "compute_focus_tests.rs"]
mod focus_tests;
#[path = "compute_hdr_stats.rs"]
pub(crate) mod hdr_stats;
#[path = "compute_menu.rs"]
mod menu;
pub use hdr_stats::{HDR_STATS_BINS, HdrStatsReadout};
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
/// A shader-resource view of a whole one-mip 2D texture.
fn texture_srv(format: DXGI_FORMAT) -> D3D12_SHADER_RESOURCE_VIEW_DESC {
    D3D12_SHADER_RESOURCE_VIEW_DESC {
        Format: format,
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
    }
}
/// An unordered-access view of `plane` of a 2D texture's first mip.
fn texture_uav(format: DXGI_FORMAT, plane: u32) -> D3D12_UNORDERED_ACCESS_VIEW_DESC {
    D3D12_UNORDERED_ACCESS_VIEW_DESC {
        Format: format,
        ViewDimension: D3D12_UAV_DIMENSION_TEXTURE2D,
        Anonymous: D3D12_UNORDERED_ACCESS_VIEW_DESC_0 {
            Texture2D: D3D12_TEX2D_UAV {
                MipSlice: 0,
                PlaneSlice: plane,
            },
        },
    }
}

#[cfg(test)]
mod tests;
