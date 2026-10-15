//! The control plane (`EvtIddCxDeviceIoControl`): the `rubylight_vdisplay_proto::control`
//! IOCTLs.
//!
//! From punktfunk's pf-vdisplay `control.rs` by Enrico Bühler (unom), MIT OR Apache-2.0.
//!
//! [`dispatch`] wraps the raw request in a [`Request`] token once and hands it to a handler
//! by value; completing consumes the token, so every path completes exactly once. Every verb
//! is scoped to the calling process, and every IOCTL counts as that process's heartbeat.

use rubylight_vdisplay_proto::control::{
    self, AddReply, AddRequest, CAP_FRAME_RING, CAP_HDR, CAP_SCOPED_CLEAR, CAP_UPDATE_MODES,
    ClearFrameRingRequest, INFO_MAGIC, InfoReply, RenderAdapterRequest, SessionRequest,
    SetFrameRingRequest, UpdateModesRequest,
};
use rubylight_vdisplay_proto::{PROTOCOL_VERSION, modes, ring};
use wdk_sys::{NTSTATUS, WDFREQUEST};

use crate::frame_ring::RingHandles;
use crate::wdf::Request;
use crate::{
    STATUS_BUFFER_TOO_SMALL, STATUS_INVALID_PARAMETER, STATUS_NOT_FOUND, STATUS_NOT_SUPPORTED,
    STATUS_SUCCESS,
};

/// Dispatch one control IOCTL and complete the request.
///
/// # Safety
/// `request` is the framework-provided request of an `EvtIddCxDeviceIoControl` call.
pub unsafe fn dispatch(request: WDFREQUEST, ioctl_code: u32) {
    // SAFETY: `request` is the live request of this callback, the contract `Request::new`
    // requires. Everything below is safe; the token owns completion.
    let request = unsafe { Request::new(request) };
    let owner = request.requestor_pid();
    crate::watchdog::ping(owner, request.file_object());
    match ioctl_code {
        control::IOCTL_GET_INFO => {
            let reply = InfoReply {
                magic: INFO_MAGIC,
                protocol_version: PROTOCOL_VERSION,
                watchdog_timeout_ms: crate::watchdog::WATCHDOG_TIMEOUT_S * 1000,
                max_monitors: modes::MAX_MONITOR_ID,
                max_ring_slots: ring::MAX_SLOTS as u32,
                caps: CAP_FRAME_RING | CAP_HDR | CAP_UPDATE_MODES | CAP_SCOPED_CLEAR,
                driver_version: driver_version(),
            };
            reply_with(request, &reply.encode());
        }
        control::IOCTL_ADD => add(owner, request),
        control::IOCTL_REMOVE => with_session(request, |s| {
            if crate::monitor::remove_monitor(owner, s) {
                STATUS_SUCCESS
            } else {
                STATUS_NOT_FOUND
            }
        }),
        control::IOCTL_UPDATE_MODES => {
            let st = match read(
                &request,
                UpdateModesRequest::SIZE,
                UpdateModesRequest::decode,
            ) {
                Ok(r) => crate::monitor::update_monitor_modes(
                    owner,
                    r.session_id,
                    r.width,
                    r.height,
                    r.refresh_mhz,
                ),
                Err(st) => st,
            };
            request.complete(st);
        }
        control::IOCTL_HEARTBEAT => with_session(request, |s| {
            // Session 0 only proves the caller alive; a named one must still exist.
            if s == 0 || crate::monitor::alive(owner, s) {
                STATUS_SUCCESS
            } else {
                STATUS_NOT_FOUND
            }
        }),
        control::IOCTL_SET_RENDER_ADAPTER => {
            let st = match read(
                &request,
                RenderAdapterRequest::SIZE,
                RenderAdapterRequest::decode,
            ) {
                Ok(r) => {
                    // As for ADD: a refused adapter init gets its second chance here.
                    if crate::adapter::adapter().is_none() {
                        crate::adapter::retry_init();
                    }
                    crate::adapter::set_render_adapter(owner, r.luid_low, r.luid_high)
                }
                Err(st) => st,
            };
            request.complete(st);
        }
        control::IOCTL_SET_FRAME_RING => set_frame_ring(owner, request),
        control::IOCTL_CLEAR_FRAME_RING => with_session(request, |s| {
            if crate::monitor::clear_frame_ring(owner, s) {
                STATUS_SUCCESS
            } else {
                STATUS_NOT_FOUND
            }
        }),
        control::IOCTL_CLEAR_FRAME_RING_GENERATION => {
            let st = match read(
                &request,
                ClearFrameRingRequest::SIZE,
                ClearFrameRingRequest::decode,
            ) {
                Ok(r) => {
                    if crate::monitor::clear_frame_ring_generation(
                        owner,
                        r.session_id,
                        r.generation,
                    ) {
                        STATUS_SUCCESS
                    } else {
                        STATUS_NOT_FOUND
                    }
                }
                Err(st) => st,
            };
            request.complete(st);
        }
        control::IOCTL_QUERY_MONITOR => {
            match read(&request, SessionRequest::SIZE, SessionRequest::decode) {
                Ok(r) => match crate::monitor::find_session(owner, r.session_id) {
                    Some(m) => reply_with(request, &m.status().encode()),
                    None => request.complete(STATUS_NOT_FOUND),
                },
                Err(st) => request.complete(st),
            }
        }
        control::IOCTL_CLEAR_ALL => {
            crate::monitor::clear_all(owner);
            request.complete(STATUS_SUCCESS);
        }
        control::IOCTL_DRAIN_LOG => {
            let lines = crate::log::drain(request.output_buffer_len());
            // Nothing queued completes with zero bytes rather than a zero-length copy.
            let st = if lines.is_empty() {
                STATUS_SUCCESS
            } else {
                request.copy_to_output(&lines)
            };
            request.complete(st);
        }
        _ => request.complete(STATUS_NOT_SUPPORTED),
    }
}

/// `major << 16 | minor << 8 | patch` of this build.
fn driver_version() -> u32 {
    let part = |s: &str| s.parse::<u32>().unwrap_or(0) & 0xFF;
    (part(env!("CARGO_PKG_VERSION_MAJOR")) << 16)
        | (part(env!("CARGO_PKG_VERSION_MINOR")) << 8)
        | part(env!("CARGO_PKG_VERSION_PATCH"))
}

/// Read an exactly-sized request. A buffer of any other length is `INVALID_PARAMETER`.
fn read<T, E>(
    request: &Request,
    size: usize,
    decode: impl FnOnce(&[u8]) -> Result<T, E>,
) -> Result<T, NTSTATUS> {
    let (bytes, len) = request.input_bytes(size)?;
    if len != size {
        return Err(STATUS_INVALID_PARAMETER);
    }
    decode(&bytes).map_err(|_| STATUS_INVALID_PARAMETER)
}

fn with_session(request: Request, act: impl FnOnce(u64) -> NTSTATUS) {
    let st = match read(&request, SessionRequest::SIZE, SessionRequest::decode) {
        Ok(r) => act(r.session_id),
        Err(st) => st,
    };
    request.complete(st);
}

/// Copy a reply into the output buffer and complete with its length.
fn reply_with(request: Request, bytes: &[u8]) {
    if request.output_buffer_len() < bytes.len() {
        request.complete(STATUS_BUFFER_TOO_SMALL);
        return;
    }
    let st = request.copy_to_output(bytes);
    request.complete(st);
}

/// `IOCTL_ADD`: create the caller's monitor and reply with where it landed.
fn add(owner: u32, request: Request) {
    let req = match read(&request, AddRequest::SIZE, AddRequest::decode) {
        Ok(r) => r,
        Err(st) => {
            request.complete(st);
            return;
        }
    };
    // A refused adapter init is never retried by PnP; the host's ADD retries are the second
    // chance.
    if crate::adapter::adapter().is_none() {
        crate::adapter::retry_init();
    }
    match crate::monitor::create_monitor(owner, &req) {
        Ok((monitor_id, target_id, luid_low, luid_high)) => {
            let reply = AddReply {
                adapter_luid_low: luid_low,
                adapter_luid_high: luid_high,
                target_id,
                monitor_id,
                // This WUDFHost, where the host duplicates the ring's handles. It is ours
                // alone (`ProcessSharingDisabled`) and dies with the device.
                wudf_pid: std::process::id(),
            };
            reply_with(request, &reply.encode());
        }
        Err(st) => request.complete(st),
    }
}

/// `IOCTL_SET_FRAME_RING`: adopt the ring's handles for the caller's monitor. Success means
/// the driver owns every handle in the request; any failure means it owns none.
fn set_frame_ring(owner: u32, request: Request) {
    let req = match read(
        &request,
        SetFrameRingRequest::SIZE,
        SetFrameRingRequest::decode,
    ) {
        Ok(r) => r,
        Err(st) => {
            request.complete(st);
            return;
        }
    };
    let Some(monitor) = crate::monitor::find_session(owner, req.session_id) else {
        request.complete(STATUS_NOT_FOUND);
        return;
    };
    let handles = match RingHandles::adopt(&req) {
        Ok(h) => std::sync::Arc::new(h),
        Err(st) => {
            request.complete(st);
            return;
        }
    };
    match monitor.set_ring(handles) {
        Ok(displaced) => {
            dbglog!(
                "[rlvd] SET_FRAME_RING: target {} ring {} ({} slots {}x{} format {})",
                monitor.target_id(),
                req.generation,
                req.slot_count,
                req.width,
                req.height,
                req.format
            );
            // The old ring's handles close once the worker lets go of it too.
            drop(displaced);
            request.complete(STATUS_SUCCESS);
        }
        Err(handles) => {
            // Torn down meanwhile: give the handles back for the host to reap.
            if let Ok(handles) = std::sync::Arc::try_unwrap(handles) {
                handles.into_unowned();
            }
            request.complete(STATUS_NOT_FOUND);
        }
    }
}
