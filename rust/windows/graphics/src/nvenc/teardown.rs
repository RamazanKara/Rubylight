//! NVENC session and encoder teardown, including D3D12/async EOS flushing,
//! completion waits, and safe retention on timeout or destroy error.

use super::{Encoder, Session, Slot, check_raw};
use anyhow::Result;
use butterpollo_media_policy::nvenc::ApiVersion;
use butterpollo_win_abi::nvenc::*;
use std::{
    ffi::c_void,
    ptr,
    time::{Duration, Instant},
};
use windows::Win32::{
    Foundation::{WAIT_FAILED, WAIT_OBJECT_0},
    System::Threading::WaitForSingleObject,
};

/// How long teardown waits for a D3D12 NVENC session's output fence or an
/// async NVENC session's EOS completion event before unregistering resources
/// or retaining them on a hung GPU/driver.
const CLOSE_WAIT: Duration = Duration::from_secs(2);

pub(super) fn cleanup_slot(
    functions: &NV_ENCODE_API_FUNCTION_LIST,
    raw: *mut c_void,
    api: ApiVersion,
    slot: &mut Slot,
) -> Result<()> {
    let check = |status, operation| check_raw(status, operation, functions, raw);
    // SAFETY: `raw` is the session these resources belong to; each is released only while still
    // held and then cleared, and the unregister entry existed when `event_registered` was set.
    unsafe {
        if !slot.mapped.is_null() {
            check(
                (functions.nvEncUnmapInputResource.unwrap())(raw, slot.mapped),
                "unmap input during cleanup",
            )?;
            slot.mapped = ptr::null_mut();
        }
        if let Some(resources) = slot.d3d12.as_mut() {
            if !resources.mapped.is_null() {
                check(
                    (functions.nvEncUnmapInputResource.unwrap())(raw, resources.mapped),
                    "unmap D3D12 output during cleanup",
                )?;
                resources.mapped = ptr::null_mut();
            }
            if !resources.registered.is_null() {
                check(
                    (functions.nvEncUnregisterResource.unwrap())(raw, resources.registered),
                    "unregister D3D12 output",
                )?;
                resources.registered = ptr::null_mut();
            }
            // The output is the slot's own structure, not a driver buffer.
            slot.output = ptr::null_mut();
        }
        if slot.event_registered {
            if let Some(signal) = slot.event.as_ref() {
                let mut event = NV_ENC_EVENT_PARAMS {
                    version: api.event(),
                    completionEvent: signal.raw().0,
                    ..Default::default()
                };
                check(
                    (functions.nvEncUnregisterAsyncEvent.unwrap())(raw, &mut event),
                    "unregister completion event",
                )?;
            }
            slot.event_registered = false;
        }
        if !slot.output.is_null() {
            check(
                (functions.nvEncDestroyBitstreamBuffer.unwrap())(raw, slot.output),
                "destroy output buffer",
            )?;
            slot.output = ptr::null_mut();
        }
        if !slot.registered.is_null() {
            check(
                (functions.nvEncUnregisterResource.unwrap())(raw, slot.registered),
                "unregister GPU input",
            )?;
            slot.registered = ptr::null_mut();
        }
    }
    Ok(())
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

impl Session {
    /// Wait up to `budget` for a D3D12 session's output fence to reach its last
    /// accepted submission before unregistering resources and destroying the
    /// encoder. Returns `true` once idle (or if the device is already removed)
    /// and `false` if GPU work may still be in flight.
    fn wait_d3d12_idle(&mut self, budget: Duration) -> bool {
        let Some(target) = self.d3d12.as_ref().map(|d3d12| d3d12.submitted) else {
            return true;
        };
        if target == 0 {
            return true;
        }
        let deadline = Instant::now() + budget;
        loop {
            let completed = match self.d3d12.as_ref().unwrap().fence.completed() {
                Ok(value) => value,
                // A removed device executes no more work; let cleanup run.
                Err(_) => return true,
            };
            if completed >= target {
                if !self.pending.is_empty() {
                    let _ = self.collect();
                }
                return true;
            }
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                return false;
            };
            if self.drain_event.is_none() {
                match crate::timing::Signal::new() {
                    Ok(event) => self.drain_event = Some(event),
                    Err(error) => {
                        tracing::warn!(%error, "NVENC D3D12 teardown wait event failed");
                        return false;
                    }
                }
            }
            let event = self.drain_event.as_ref().unwrap();
            let d3d12 = self.d3d12.as_ref().unwrap();
            let mut arm = self.drain_armed != target;
            if !arm && event.is_set() {
                if event.reset().is_err() {
                    return false;
                }
                match d3d12.fence.completed() {
                    Ok(value) if value >= target => {
                        if !self.pending.is_empty() {
                            let _ = self.collect();
                        }
                        return true;
                    }
                    Err(_) => return true,
                    Ok(_) => arm = true,
                }
            }
            if arm {
                if event.reset().is_err() {
                    return false;
                }
                if let Err(error) = d3d12.fence.notify(target, event.raw()) {
                    tracing::warn!(%error, target, "NVENC D3D12 teardown fence wait failed");
                    return false;
                }
                self.drain_armed = target;
            }
            let millis = left.as_millis().clamp(1, u32::MAX as u128) as u32;
            // SAFETY: `event` is a live event owned by this session.
            if unsafe { WaitForSingleObject(event.raw(), millis) } == WAIT_FAILED {
                tracing::warn!("NVENC D3D12 teardown fence wait failed");
                return false;
            }
        }
    }

    /// Destroy the driver session and release its inputs. Nothing runs on
    /// it after (`guard` refuses), and dropping it does nothing more.
    /// Returns `false` if driver resources had to be retained until host exit.
    pub(super) fn close(&mut self) -> bool {
        self.close_with_wait(CLOSE_WAIT)
    }

    pub(super) fn close_with_wait(&mut self, wait: Duration) -> bool {
        if self.raw.is_null() {
            return true;
        }
        let _guard = match self.guard() {
            Ok(guard) => guard,
            Err(error) => {
                tracing::warn!(%error, "CUDA device lost during NVENC teardown; attempting driver cleanup");
                None
            }
        };
        // nvEncodeAPI.h (`NvEncDestroyEncoder`): flush before freeing resources.
        // Synchronous sessions (including D3D12, where `enableEncodeAsync == 0`)
        // always send EOS even after all pending frames have been collected so
        // internal driver GPU work flushes before resources are unregistered.
        // Asynchronous sessions require a registered `completionEvent` on every
        // `nvEncEncodePicture`, including EOS, and must wait for that event to
        // signal before unregistering events or destroying resources.
        let mut eos_event_registered = false;
        let mut eos_idle = true;
        if self.initialized && (self.initialize.enableEncodeAsync == 0 || !self.pending.is_empty())
        {
            let mut eos_wait_handle = None;
            let completion_event = if self.initialize.enableEncodeAsync != 0 {
                if let Some(event) = self.slots.iter().enumerate().find_map(|(index, slot)| {
                    if !slot.event_registered
                        || self.pending.iter().any(|pending| pending.slot == index)
                    {
                        return None;
                    }
                    let event = slot.event.as_ref()?;
                    event.reset().ok()?;
                    Some(event)
                }) {
                    eos_wait_handle = Some(event.raw());
                    event.raw().0
                } else {
                    if self.drain_event.is_none() {
                        self.drain_event = crate::timing::Signal::new().ok();
                    }
                    if let (Some(event), Some(register)) = (
                        self.drain_event.as_ref(),
                        self.functions.nvEncRegisterAsyncEvent,
                    ) && event.reset().is_ok()
                    {
                        let mut params = NV_ENC_EVENT_PARAMS {
                            version: self.api.event(),
                            completionEvent: event.raw().0,
                            ..Default::default()
                        };
                        // SAFETY: `self.raw` is a live async session and `self.drain_event` stays
                        // open until unregistered below or retained on timeout/failure.
                        if unsafe { register(self.raw, &mut params) } == super::SUCCESS {
                            eos_event_registered = true;
                            eos_wait_handle = Some(event.raw());
                            event.raw().0
                        } else {
                            ptr::null_mut()
                        }
                    } else {
                        ptr::null_mut()
                    }
                }
            } else {
                ptr::null_mut()
            };
            let eos_submitted = if self.initialize.enableEncodeAsync == 0
                || !completion_event.is_null()
            {
                let mut eos = NV_ENC_PIC_PARAMS {
                    version: self.api.picture(),
                    encodePicFlags: _NV_ENC_PIC_FLAGS_NV_ENC_PIC_FLAG_EOS,
                    completionEvent: completion_event,
                    ..Default::default()
                };
                // SAFETY: `self.raw` is the non-null open session and `eos` is versioned.
                let status =
                    unsafe { (self.functions.nvEncEncodePicture.unwrap())(self.raw, &mut eos) };
                if status != super::SUCCESS {
                    tracing::debug!(
                        status,
                        "NVENC teardown EOS submission returned non-success status"
                    );
                }
                status == super::SUCCESS
            } else {
                tracing::warn!(
                    "NVENC async teardown could not obtain a registered EOS completion event; draining pending frames without EOS"
                );
                false
            };
            if !self.pending.is_empty() {
                let _ = self.drain();
            }
            if eos_submitted && let Some(handle) = eos_wait_handle {
                let millis = if wait.is_zero() {
                    0
                } else {
                    wait.as_millis().clamp(1, u32::MAX as u128) as u32
                };
                // SAFETY: `handle` is a live manual-reset Win32 event owned by `self.slots` or
                // `self.drain_event`, still open until unregistered below or retained on timeout.
                let waited = unsafe { WaitForSingleObject(handle, millis) };
                if waited != WAIT_OBJECT_0 {
                    eos_idle = false;
                    tracing::warn!(
                        wait_result = waited.0,
                        "NVENC async EOS completion event did not signal before teardown"
                    );
                }
            }
            if eos_idle
                && eos_event_registered
                && self.pending.is_empty()
                && let (Some(event), Some(unregister)) = (
                    self.drain_event.as_ref(),
                    self.functions.nvEncUnregisterAsyncEvent,
                )
            {
                let mut params = NV_ENC_EVENT_PARAMS {
                    version: self.api.event(),
                    completionEvent: event.raw().0,
                    ..Default::default()
                };
                // SAFETY: `params.completionEvent` was registered above, drain finished cleanly,
                // and any accepted EOS completion event has signaled.
                if unsafe { unregister(self.raw, &mut params) } != super::SUCCESS {
                    eos_idle = false;
                    tracing::warn!(
                        "NVENC async EOS completion event unregister failed; retaining event handle"
                    );
                }
            }
        }
        let d3d12_idle = self.wait_d3d12_idle(wait);
        if d3d12_idle && eos_idle {
            for (index, slot) in self.slots.iter_mut().enumerate() {
                // A failed drain does not establish completion. Let session
                // destruction stop the native worker before releasing its input,
                // output buffer or completion event.
                if self.pending.iter().any(|pending| pending.slot == index) {
                    continue;
                }
                if let Err(error) = cleanup_slot(&self.functions, self.raw, self.api, slot) {
                    tracing::warn!(%error, "NVENC input cleanup deferred to session destruction");
                }
            }
        }
        // SAFETY: `self.raw` is non-null (checked at the top) and is destroyed once, since it is
        // cleared below.
        let status = unsafe { (self.functions.nvEncDestroyEncoder.unwrap())(self.raw) };
        let destroy_error = self.check(status, "destroy session").err();
        let clean = destroy_error.is_none() && d3d12_idle && eos_idle;
        if !clean {
            // A failed native destroy, unfinished D3D12 output fence, or
            // unsignaled async EOS completion event leaves driver work's
            // lifetime unknown. Preserve the resources, completion events,
            // fence, wait event, context and driver DLL until host exit instead
            // of freeing or closing handles behind a still-running encoder.
            if let Some(error) = destroy_error {
                tracing::error!(%error, "NVENC cleanup failed; driver resources retained until host exit");
            } else if !eos_idle {
                tracing::error!(
                    "NVENC async EOS did not finish before teardown; retaining session resources and completion events until host exit"
                );
            } else {
                tracing::error!(
                    "NVENC D3D12 output fence did not finish before teardown; retaining session resources until host exit"
                );
            }
            std::mem::forget(std::mem::take(&mut self.slots));
            std::mem::forget(std::mem::take(&mut self.pending));
            std::mem::forget(self._runtime.clone());
            if let Some(context) = self.cuda.take() {
                std::mem::forget(context);
            }
            if let Some(d3d12) = self.d3d12.take() {
                std::mem::forget(d3d12);
            }
            if let Some(event) = self.drain_event.take() {
                std::mem::forget(event);
            }
        }
        self.raw = ptr::null_mut();
        // Drop CUDA allocations/textures only after the driver destroys the
        // session, including inputs whose explicit unregister was rejected.
        self.slots.clear();
        self.pending.clear();
        clean
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        self.close_session();
    }
}

impl Encoder {
    /// Drain any compute conversion work on the shared D3D12 device before
    /// closing the NVENC session, and retain `output_event` if the D3D12 output
    /// fence had to be retained so a late signal cannot touch a closed handle.
    pub(super) fn close_session(&mut self) {
        self.last_converted = None;
        if let Some(converter) = &self.compute {
            converter.wait_idle();
        }
        if !self.session.close()
            && let Some(event) = self.output_event.take_d3d12()
        {
            std::mem::forget(event);
        }
    }
}
