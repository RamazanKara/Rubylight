//! A swap-chain worker's copy queue: the one queue that reads the swap chain's surfaces, which
//! IddCx 1.11 must be told about on every acquire (`IDARG_IN_RELEASEANDACQUIREBUFFER2::
//! pD3D12CommandQueue`) so it orders the OS's writes before these reads. Nothing else is
//! submitted to it, because the OS holds back whatever follows on that queue.
//!
//! A copy queue rather than a direct queue: the only work is `CopyResource` between textures
//! of one format, and the game's graphics queue is never in its way. Every resource a copy
//! queue touches decays to `COMMON` when its command list finishes, so no barriers are
//! recorded, and the host's compute queue promotes the textures it reads on its own.
//!
//! Command lists come from a small ring of allocators. One still on the GPU is never waited
//! for: the copy is refused and the frame dropped, because the worker runs inside the swap
//! chain's window, which DWM waits on.

use windows::Win32::Graphics::Direct3D12::{
    D3D12_COMMAND_LIST_TYPE_COPY, D3D12_COMMAND_QUEUE_DESC, D3D12_COMMAND_QUEUE_FLAG_NONE,
    D3D12_COMMAND_QUEUE_PRIORITY_GLOBAL_REALTIME, D3D12_COMMAND_QUEUE_PRIORITY_HIGH,
    D3D12_FENCE_FLAG_NONE, ID3D12CommandAllocator, ID3D12CommandList, ID3D12CommandQueue,
    ID3D12Device, ID3D12Fence, ID3D12GraphicsCommandList, ID3D12Resource,
};
use windows::core::{Error, Interface};

/// Copies in flight before one is refused: at 240 Hz this is 33 ms of GPU time.
const LISTS: usize = 8;

struct List {
    allocator: ID3D12CommandAllocator,
    list: ID3D12GraphicsCommandList,
    /// The copy queue's own fence value this list's last submission signals.
    done: u64,
}

pub struct Copier {
    queue: ID3D12CommandQueue,
    fence: ID3D12Fence,
    value: u64,
    lists: Vec<List>,
    next: usize,
}

/// Why [`Copier::copy`] submitted nothing.
#[derive(Debug)]
pub enum CopyError {
    /// The next command list is still on the GPU.
    Busy,
    /// Recording or submitting failed; the device may be lost.
    Failed(Error),
}

impl From<Error> for CopyError {
    fn from(e: Error) -> Self {
        Self::Failed(e)
    }
}

impl Copier {
    /// A copy queue on `device` at global realtime priority when `realtime` (falling back to
    /// high priority where the process may not raise it), so a GPU-saturating game does not
    /// starve the copy every frame depends on.
    pub fn new(device: &ID3D12Device, realtime: bool) -> Result<Self, Error> {
        let queue = |priority| {
            let desc = D3D12_COMMAND_QUEUE_DESC {
                Type: D3D12_COMMAND_LIST_TYPE_COPY,
                Priority: priority,
                Flags: D3D12_COMMAND_QUEUE_FLAG_NONE,
                NodeMask: 0,
            };
            // SAFETY: `desc` is a valid copy-queue description on the live device.
            unsafe { device.CreateCommandQueue::<ID3D12CommandQueue>(&desc) }
        };
        let high = D3D12_COMMAND_QUEUE_PRIORITY_HIGH.0;
        let queue = if realtime {
            match queue(D3D12_COMMAND_QUEUE_PRIORITY_GLOBAL_REALTIME.0) {
                Ok(q) => {
                    dbglog!("[rlvd] copy queue: global realtime priority");
                    q
                }
                Err(e) => {
                    dbglog!(
                        "[rlvd] copy queue: global realtime priority refused ({e:?}); high priority instead"
                    );
                    queue(high)?
                }
            }
        } else {
            dbglog!("[rlvd] copy queue: high priority (RLVD_NO_RT_GPU)");
            queue(high)?
        };
        // SAFETY: a plain fence creation on the live device.
        let fence: ID3D12Fence = unsafe { device.CreateFence(0, D3D12_FENCE_FLAG_NONE)? };
        let mut lists = Vec::with_capacity(LISTS);
        for _ in 0..LISTS {
            // SAFETY: plain object creation on the live device; the list is created on its
            // allocator and closed at once, so the first `copy` can reset both.
            let list = unsafe {
                let allocator: ID3D12CommandAllocator =
                    device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_COPY)?;
                let list: ID3D12GraphicsCommandList =
                    device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_COPY, &allocator, None)?;
                list.Close()?;
                List {
                    allocator,
                    list,
                    done: 0,
                }
            };
            lists.push(list);
        }
        Ok(Self {
            queue,
            fence,
            value: 0,
            lists,
            next: 0,
        })
    }

    pub fn queue(&self) -> &ID3D12CommandQueue {
        &self.queue
    }

    /// Copy each `(destination, source)` pair (same size and format) on the queue, then
    /// signal `signal` on it. Nothing is submitted on `Err`.
    pub fn copy(
        &mut self,
        pairs: &[(&ID3D12Resource, &ID3D12Resource)],
        signal: Option<(&ID3D12Fence, u64)>,
    ) -> Result<(), CopyError> {
        let entry = &mut self.lists[self.next];
        // SAFETY: a plain read of this queue's own fence.
        if unsafe { self.fence.GetCompletedValue() } < entry.done {
            return Err(CopyError::Busy);
        }
        // SAFETY: the allocator's last submission finished (checked above), and only this
        // worker records on it. Every resource is live for the call, and copy queues need no
        // barriers (module docs).
        unsafe {
            entry.allocator.Reset()?;
            entry.list.Reset(&entry.allocator, None)?;
            for (destination, source) in pairs {
                entry.list.CopyResource(*destination, *source);
            }
            entry.list.Close()?;
            self.queue
                .ExecuteCommandLists(&[Some(entry.list.cast::<ID3D12CommandList>()?)]);
        }
        // The list counts as in flight from here on, even if a signal below fails: it was
        // submitted, so its allocator is never reset under it.
        self.value += 1;
        entry.done = self.value;
        self.next = (self.next + 1) % LISTS;
        // SAFETY: plain signals on this queue, of a fence the caller opened on the same device
        // and of its own fence.
        unsafe {
            if let Some((fence, value)) = signal {
                self.queue.Signal(fence, value)?;
            }
            self.queue.Signal(&self.fence, self.value)?;
        }
        Ok(())
    }
}
