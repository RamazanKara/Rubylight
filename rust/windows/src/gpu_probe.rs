//! A trivial GPU round trip on a graphics queue of its own: a fence signal on an empty queue,
//! which completes once the GPU gets to it. A stalled GPU holds it back; a host thread stuck on
//! its own work does not.
use crate::capture::Device;
use anyhow::Result;
use std::sync::atomic::{AtomicU64, Ordering};
use windows::Win32::Graphics::Direct3D12::*;

pub struct Probe {
    gpu: Device,
    queue: ID3D12CommandQueue,
    fence: ID3D12Fence,
    issued: AtomicU64,
}
impl Probe {
    /// On the adapter showing `display` (the primary's if it is gone).
    pub fn new(display: &str) -> Result<Self> {
        let gpu = Device::new(display)?;
        // SAFETY: `gpu.compute.device` is a live D3D12 device and the description outlives the
        // call.
        let (queue, fence) = unsafe {
            let device = &gpu.compute.device;
            let queue: ID3D12CommandQueue =
                device.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC {
                    Type: D3D12_COMMAND_LIST_TYPE_DIRECT,
                    ..Default::default()
                })?;
            let fence: ID3D12Fence = device.CreateFence(0, D3D12_FENCE_FLAG_NONE)?;
            (queue, fence)
        };
        Ok(Self {
            gpu,
            queue,
            fence,
            issued: AtomicU64::new(0),
        })
    }
    /// Submits the round trip to the GPU.
    pub fn issue(&self) {
        let value = self.issued.load(Ordering::Relaxed) + 1;
        // SAFETY: the queue and fence are on the same live device.
        if unsafe { self.queue.Signal(&self.fence, value) }.is_ok() {
            self.issued.store(value, Ordering::Relaxed);
        }
    }
    /// Whether the GPU has reached the round trip issued last.
    pub fn done(&self) -> Result<bool> {
        // SAFETY: a plain read of a live fence.
        let completed = unsafe { self.fence.GetCompletedValue() };
        // A removed device signals its fences to the maximum.
        if completed == u64::MAX
            && let Some(loss) = crate::device_loss::DeviceLost::d3d12(&self.gpu.compute.device)
        {
            return Err(loss.into());
        }
        Ok(completed >= self.issued.load(Ordering::Relaxed))
    }
}
