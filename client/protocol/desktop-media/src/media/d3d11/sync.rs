//! Reuse-slot selection and DXGI device-removal helpers for [`super::DecodeBridge`].

use windows::Win32::Graphics::Direct3D11::ID3D11Device;
use windows::Win32::Graphics::Dxgi::{
    DXGI_ERROR_DEVICE_HUNG, DXGI_ERROR_DEVICE_REMOVED, DXGI_ERROR_DEVICE_RESET,
    DXGI_ERROR_DRIVER_INTERNAL_ERROR,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SlotChoice {
    Reuse(usize),
    Alloc,
    Exhausted,
}

/// Picks a free shared texture slot without allowing `decode_queue` to wait on an unflushed
/// `render_queue.Signal` fence value (`reuse_value > flushed`), preventing GPU scheduler deadlocks
/// when leases are released off the presenting thread.
pub(super) fn choose_reuse_slot(
    slots: &[(bool, u64)],
    completed: u64,
    flushed: u64,
    max_slots: usize,
    hard_cap: usize,
) -> SlotChoice {
    if let Some(index) = slots
        .iter()
        .position(|&(in_use, reuse)| !in_use && reuse <= completed)
    {
        return SlotChoice::Reuse(index);
    }
    if slots.len() < max_slots {
        return SlotChoice::Alloc;
    }
    if let Some((index, _)) = slots
        .iter()
        .enumerate()
        .filter(|&(_, &(in_use, reuse))| !in_use && reuse <= flushed)
        .min_by_key(|(_, (_, reuse))| *reuse)
    {
        return SlotChoice::Reuse(index);
    }
    if slots.len() < hard_cap {
        return SlotChoice::Alloc;
    }
    SlotChoice::Exhausted
}

/// Returns whether `code` is a DXGI device-removal / TDR / reset error HRESULT.
pub(super) fn is_device_removed_code(code: i32) -> bool {
    code == DXGI_ERROR_DEVICE_REMOVED.0
        || code == DXGI_ERROR_DEVICE_HUNG.0
        || code == DXGI_ERROR_DEVICE_RESET.0
        || code == DXGI_ERROR_DRIVER_INTERNAL_ERROR.0
}

/// Reads `ID3D11Device::GetDeviceRemovedReason()` on a failure path.
pub(in crate::media) fn device_removed_reason(device: &ID3D11Device) -> Option<String> {
    // SAFETY: `GetDeviceRemovedReason` only inspects device removal status.
    unsafe { device.GetDeviceRemovedReason() }
        .err()
        .map(|e| e.to_string())
}

/// Interprets `ID3D11Fence::GetCompletedValue()`, treating `u64::MAX` as D3D11 device removal.
pub(super) fn fence_completed_ready(completed: u64, target: u64) -> Result<bool, String> {
    if completed == u64::MAX {
        return Err("shared D3D11 fence reported device removal".into());
    }
    Ok(completed >= target)
}
