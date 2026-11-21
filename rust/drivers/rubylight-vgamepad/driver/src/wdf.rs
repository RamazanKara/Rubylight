//! Ownership token around a live `WDFREQUEST`, ensuring each IOCTL completes once.
//!
//! Derived from Rubylight `rubylight-vdisplay` / `pf-umdf-util` `wdf.rs`.

use wdk_sys::{NTSTATUS, WDFMEMORY, WDFREQUEST, call_unsafe_wdf_function_binding};

use crate::STATUS_BUFFER_TOO_SMALL;

pub struct Request(WDFREQUEST);

impl Request {
    /// Wrap a framework request belonging to the current `EvtIoDeviceControl` callback.
    ///
    /// # Safety
    /// `raw` must be a live, incomplete `WDFREQUEST` owned by this callback invocation.
    pub unsafe fn new(raw: WDFREQUEST) -> Self {
        Self(raw)
    }

    /// Raw pointer value identifying the caller's open file handle (owner token).
    pub fn file_object_key(&self) -> usize {
        // SAFETY: `self.0` is the live callback request per `Request::new`'s contract.
        let file = unsafe { call_unsafe_wdf_function_binding!(WdfRequestGetFileObject, self.0) };
        file as usize
    }

    /// Complete the request with `status` and zero information bytes.
    pub fn complete(self, status: NTSTATUS) {
        // SAFETY: `self.0` is incomplete and owned by this token.
        unsafe { call_unsafe_wdf_function_binding!(WdfRequestComplete, self.0, status) };
    }

    /// Complete the request with `status` and written byte count `info`.
    pub fn complete_with_info(self, status: NTSTATUS, info: usize) {
        // SAFETY: `self.0` is incomplete and owned by this token.
        unsafe {
            call_unsafe_wdf_function_binding!(
                WdfRequestCompleteWithInformation,
                self.0,
                status,
                info as u64
            )
        };
    }

    /// Read the input buffer up to `cap` bytes, returning `(bytes, total_input_len)`.
    pub fn input_bytes(&self, cap: usize) -> Result<(Vec<u8>, usize), NTSTATUS> {
        let mut mem: WDFMEMORY = core::ptr::null_mut();
        // SAFETY: `self.0` is the live callback request; `mem` receives the input handle.
        let st = unsafe {
            call_unsafe_wdf_function_binding!(WdfRequestRetrieveInputMemory, self.0, &mut mem)
        };
        if st < 0 {
            return Err(st);
        }
        let mut len: usize = 0;
        // SAFETY: `mem` is valid from `WdfRequestRetrieveInputMemory`; `len` receives its size.
        let ptr = unsafe { call_unsafe_wdf_function_binding!(WdfMemoryGetBuffer, mem, &mut len) }
            .cast_const()
            .cast::<u8>();
        if ptr.is_null() {
            return Ok((Vec::new(), 0));
        }
        let take = len.min(cap);
        // SAFETY: `ptr` points to `len` readable bytes while `self.0` stays uncompleted.
        let bytes = unsafe { core::slice::from_raw_parts(ptr, take) }.to_vec();
        Ok((bytes, len))
    }

    /// Verify that the request's output buffer can hold at least `min_len` bytes.
    pub fn check_output_len(&self, min_len: usize) -> Result<(), NTSTATUS> {
        let mut mem: WDFMEMORY = core::ptr::null_mut();
        // SAFETY: `self.0` is the live callback request; `mem` receives the output handle.
        let st = unsafe {
            call_unsafe_wdf_function_binding!(WdfRequestRetrieveOutputMemory, self.0, &mut mem)
        };
        if st < 0 {
            return Err(st);
        }
        let mut len: usize = 0;
        // SAFETY: `mem` is valid; `len` receives its byte size.
        let _ = unsafe { call_unsafe_wdf_function_binding!(WdfMemoryGetBuffer, mem, &mut len) };
        if len < min_len {
            return Err(STATUS_BUFFER_TOO_SMALL);
        }
        Ok(())
    }

    /// Copy `src` into the output buffer and return an error status if retrieval or copy fails,
    /// along with the byte count written.
    pub fn copy_to_output(&self, src: &[u8]) -> Result<usize, NTSTATUS> {
        let mut mem: WDFMEMORY = core::ptr::null_mut();
        // SAFETY: `self.0` is the live callback request; `mem` receives the output handle.
        let st = unsafe {
            call_unsafe_wdf_function_binding!(WdfRequestRetrieveOutputMemory, self.0, &mut mem)
        };
        if st < 0 {
            return Err(st);
        }
        let mut len: usize = 0;
        // SAFETY: `mem` is valid; `len` receives its byte size.
        let _ = unsafe { call_unsafe_wdf_function_binding!(WdfMemoryGetBuffer, mem, &mut len) };
        if len < src.len() {
            return Err(STATUS_BUFFER_TOO_SMALL);
        }
        // SAFETY: `mem` holds at least `src.len()` writable bytes and `src` is a live slice.
        let st = unsafe {
            call_unsafe_wdf_function_binding!(
                WdfMemoryCopyFromBuffer,
                mem,
                0usize,
                src.as_ptr().cast_mut().cast(),
                src.len()
            )
        };
        if st < 0 {
            return Err(st);
        }
        Ok(src.len())
    }
}
