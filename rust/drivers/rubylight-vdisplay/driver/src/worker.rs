//! Handle and thread ownership for the driver's workers.
//!
//! From punktfunk's pf-vdisplay `worker.rs` by Enrico Bühler (unom), MIT OR Apache-2.0.
//!
//! A handle or mapping belongs to exactly one value whose `Drop` closes it once, and a thread
//! only ever borrows what its owner outlives. That rule is why a host-delivered handle can
//! never be closed twice, or handed to a DDI after its owner closed it.

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Memory::{MEMORY_MAPPED_VIEW_ADDRESS, UnmapViewOfFile};
use windows::Win32::System::Threading::{
    AvRevertMmThreadCharacteristics, AvSetMmThreadCharacteristicsW, CreateEventW, GetCurrentThread,
    SetThreadPriority, THREAD_PRIORITY_TIME_CRITICAL,
};
use windows::core::w;

/// Carries a raw handle across a thread spawn: a WDF/IddCx handle (`*mut T`) or a Win32
/// [`HANDLE`]. Rebind the whole wrapper inside the closure (`let x = x;`) before touching
/// `.0`, or a disjoint capture grabs the `!Send` field itself.
pub struct Sendable<T>(pub T);
// SAFETY: an opaque framework handle, never dereferenced in Rust; the owner that handed it
// over outlives the thread using it, and the DDIs it is passed to do their own locking.
unsafe impl<T> Send for Sendable<*mut T> {}
// SAFETY: a shared `&Sendable<*mut T>` yields only by-value copies of that opaque handle.
unsafe impl<T> Sync for Sendable<*mut T> {}
// SAFETY: a Win32 handle is a process-wide token, not thread-affine; the owner that handed it
// over closes it only after the thread using it has joined.
unsafe impl Send for Sendable<HANDLE> {}

/// A Win32 handle this process owns; `Drop` closes it exactly once.
pub struct OwnedHandle(HANDLE);
// SAFETY: a Win32 handle is a process-wide token; this wrapper is its sole closer and hands
// out only borrowed copies, so moving it between threads is sound.
unsafe impl Send for OwnedHandle {}
// SAFETY: as above; the OS serializes every operation on the handle itself.
unsafe impl Sync for OwnedHandle {}

impl OwnedHandle {
    /// An unnamed event; `None` when the OS refused, which callers treat as "run without".
    pub fn event(manual_reset: bool) -> Option<Self> {
        // SAFETY: plain event creation: unsignalled, unnamed, default security.
        let h = unsafe { CreateEventW(None, manual_reset, false, None) }.ok()?;
        Some(Self(h))
    }

    // unsafe-fn-no-op-ok: the marker is the transfer. A safe fn would let safe code adopt a
    // borrowed handle and close it a second time.
    /// Adopt `h`: from here on this value alone decides when it closes.
    ///
    /// # Safety
    /// `h` must be a live handle this process owns, and no other owner may close it.
    pub unsafe fn from_raw(h: HANDLE) -> Self {
        Self(h)
    }

    /// A borrowed copy, valid only while `self` lives.
    pub fn as_raw(&self) -> HANDLE {
        self.0
    }

    /// Give the handle up without closing it: it was never ours to close (a delivery the
    /// driver refused, which the host reaps in this process remotely).
    pub fn into_unowned(self) {
        core::mem::forget(self);
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: we adopted this handle and only hand out borrowed copies, so this is its
        // sole close.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// A mapped view and the section handle behind it. `Drop` unmaps, then closes the section
/// (field order), once.
pub struct OwnedView {
    base: *mut core::ffi::c_void,
    mapping: Option<OwnedHandle>,
}
// SAFETY: a mapped view is process-wide, not thread-affine; this value is its only unmapper.
unsafe impl Send for OwnedView {}
// SAFETY: shared access only reads the base address; every access through it is atomic.
unsafe impl Sync for OwnedView {}

impl OwnedView {
    // unsafe-fn-no-op-ok: the same transfer as OwnedHandle::from_raw, plus mapped exactly
    // once, because UnmapViewOfFile cannot tell a second view of the mapping from the first.
    /// Adopt the view at `base` over `mapping`.
    ///
    /// # Safety
    /// `base` must be a live `MapViewOfFile` result for `mapping`, mapped exactly once.
    pub unsafe fn from_raw(base: *mut core::ffi::c_void, mapping: OwnedHandle) -> Self {
        Self {
            base,
            mapping: Some(mapping),
        }
    }

    /// The base address, valid only while `self` lives.
    pub fn base(&self) -> *mut core::ffi::c_void {
        self.base
    }

    /// Unmap the view (ours) but give the section handle up unclosed (the host's).
    pub fn into_unowned(mut self) {
        if let Some(mapping) = self.mapping.take() {
            mapping.into_unowned();
        }
        // `self` drops here and unmaps.
    }
}

impl Drop for OwnedView {
    fn drop(&mut self) {
        // SAFETY: our own single mapping of `self.base`; the section handle (if still owned)
        // closes right after, when the field drops.
        unsafe {
            let _ = UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS { Value: self.base });
        }
    }
}

/// An MMCSS "Distribution" registration for the calling thread, reverted on drop. When MMCSS
/// declines under WUDFHost's token the thread falls back to TIME_CRITICAL; it spends its life
/// blocked on events, so it cannot starve others.
pub struct Mmcss(Option<HANDLE>);

impl Mmcss {
    pub fn distribution(what: &str) -> Self {
        let mut task = 0u32;
        // SAFETY: `w!` is a 'static NUL-terminated task name; `task` is a live out-param.
        let res = unsafe { AvSetMmThreadCharacteristicsW(w!("Distribution"), &mut task) };
        match res {
            Ok(h) => Self(Some(h)),
            Err(e) => {
                // SAFETY: `GetCurrentThread` is a pseudo-handle; this affects only this thread.
                let fallback =
                    unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_TIME_CRITICAL) };
                dbglog!(
                    "[rlvd] {what}: MMCSS declined ({e:?}); TIME_CRITICAL fallback ok={}",
                    fallback.is_ok()
                );
                Self(None)
            }
        }
    }
}

impl Drop for Mmcss {
    fn drop(&mut self) {
        if let Some(h) = self.0.take() {
            // SAFETY: the live characteristics handle `distribution` registered, reverted once.
            let _ = unsafe { AvRevertMmThreadCharacteristics(h) };
        }
    }
}
