//! Captured textures on the compute queue: shared fences, the Desktop Duplication copy (`Handoff`), the textures lent and handed back (`Returns`) and when a copy is ready (`Ready`).

use super::*;

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
    pub(super) device: usize,
    pub(super) readers: Option<Arc<AtomicU64>>,
    /// The exact conversion timeline the capture copy queue waits for. A
    /// replacement queue can share the same D3D12 device, but never its values.
    pub(super) reader_fence: Option<ID3D12Fence>,
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
    pub(super) fn read_by(&self, compute: &Compute, value: u64) -> Result<()> {
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
    pub(super) value: u64,
    imported: Vec<ID3D11Texture2D>,
}
/// A failed ordering operation does not establish that a texture is no longer
/// read by the GPU. Keep it owned and reject further work on this queue.
pub(super) fn return_after_reads(
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
    pub(super) queue: ID3D12CommandQueue,
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
    pub(super) readers: HashMap<usize, (ID3D11Texture2D, Arc<AtomicU64>)>,
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
