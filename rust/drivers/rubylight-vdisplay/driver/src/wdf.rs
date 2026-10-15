//! The request token the control plane completes through. A callback turns its raw
//! `WDFREQUEST` into a [`Request`] once (the framework's validity guarantee is the contract);
//! every operation after that is safe, and completing consumes the token, so a request
//! cannot be completed twice or touched after completion.
//!
//! From punktfunk's pf-umdf-util `wdf.rs` by Enrico Bühler (unom), MIT OR Apache-2.0.

use wdk_sys::{NTSTATUS, WDFFILEOBJECT, WDFMEMORY, WDFREQUEST, call_unsafe_wdf_function_binding};

use crate::{STATUS_BUFFER_TOO_SMALL, STATUS_SUCCESS};

pub struct Request(WDFREQUEST);

impl Request {
    // unsafe-fn-no-op-ok: the constructor carries the contract every later call relies on.
    /// # Safety
    /// `raw` must be the live, framework-provided request of the current callback.
    pub unsafe fn new(raw: WDFREQUEST) -> Request {
        Request(raw)
    }

    /// The process that sent this request: it owns what the request creates.
    pub fn requestor_pid(&self) -> u32 {
        // SAFETY: `self.0` is the live callback request per `Request::new`'s contract.
        unsafe { call_unsafe_wdf_function_binding!(WdfRequestGetRequestorProcessId, self.0) }
    }

    /// The file object (the caller's device handle) the request arrived on.
    pub fn file_object(&self) -> WDFFILEOBJECT {
        // SAFETY: `self.0` is the live callback request per `Request::new`'s contract.
        unsafe { call_unsafe_wdf_function_binding!(WdfRequestGetFileObject, self.0) }
    }

    /// Complete with `status`; the framework owns the request afterwards.
    pub fn complete(self, status: NTSTATUS) {
        // SAFETY: `self.0` is the live, not yet completed callback request.
        unsafe { call_unsafe_wdf_function_binding!(WdfRequestComplete, self.0, status) };
    }

    /// Copy `src` into the buffered output and set the byte count. Returns the status to
    /// complete with.
    pub fn copy_to_output(&self, src: &[u8]) -> NTSTATUS {
        let mut mem: WDFMEMORY = core::ptr::null_mut();
        // SAFETY: `self.0` is the live callback request; `mem` receives the memory handle.
        let st = unsafe {
            call_unsafe_wdf_function_binding!(WdfRequestRetrieveOutputMemory, self.0, &mut mem)
        };
        if st < 0 {
            return st;
        }
        let mut len: usize = 0;
        // SAFETY: `mem` is the memory object just retrieved; `len` receives its size.
        let _ = unsafe { call_unsafe_wdf_function_binding!(WdfMemoryGetBuffer, mem, &mut len) };
        if len < src.len() {
            return STATUS_BUFFER_TOO_SMALL;
        }
        // SAFETY: `mem` is valid and at least `src.len()` bytes; `src` is a live borrow the
        // framework only reads.
        let st = unsafe {
            call_unsafe_wdf_function_binding!(
                WdfMemoryCopyFromBuffer,
                mem,
                0usize,
                src.as_ptr() as *mut core::ffi::c_void,
                src.len()
            )
        };
        if st < 0 {
            return st;
        }
        // SAFETY: `self.0` is the live callback request.
        unsafe {
            call_unsafe_wdf_function_binding!(WdfRequestSetInformation, self.0, src.len() as u64)
        };
        STATUS_SUCCESS
    }

    /// The input buffer: up to `cap` bytes copied out, and its true length.
    pub fn input_bytes(&self, cap: usize) -> Result<(Vec<u8>, usize), NTSTATUS> {
        let mut mem: WDFMEMORY = core::ptr::null_mut();
        // SAFETY: `self.0` is the live callback request; `mem` receives the memory handle.
        let st = unsafe {
            call_unsafe_wdf_function_binding!(WdfRequestRetrieveInputMemory, self.0, &mut mem)
        };
        if st < 0 {
            return Err(st);
        }
        let mut len: usize = 0;
        // SAFETY: `mem` is the memory object just retrieved; `len` receives its size.
        let p = unsafe { call_unsafe_wdf_function_binding!(WdfMemoryGetBuffer, mem, &mut len) }
            as *const u8;
        if p.is_null() {
            return Ok((Vec::new(), 0));
        }
        let n = len.min(cap);
        // SAFETY: `p` is valid for `len` bytes per `WdfMemoryGetBuffer`; `n <= len` are read.
        let bytes = unsafe { core::slice::from_raw_parts(p, n) }.to_vec();
        Ok((bytes, len))
    }

    /// The output buffer's length, 0 without one.
    pub fn output_buffer_len(&self) -> usize {
        let mut mem: WDFMEMORY = core::ptr::null_mut();
        // SAFETY: `self.0` is the live callback request; output memory is optional.
        if unsafe {
            call_unsafe_wdf_function_binding!(WdfRequestRetrieveOutputMemory, self.0, &mut mem)
        } < 0
        {
            return 0;
        }
        let mut len: usize = 0;
        // SAFETY: `mem` is the memory object just retrieved; `len` receives its size.
        let _ = unsafe { call_unsafe_wdf_function_binding!(WdfMemoryGetBuffer, mem, &mut len) };
        len
    }
}
