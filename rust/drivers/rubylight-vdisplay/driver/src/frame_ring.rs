//! The driver's half of the frame ring (`rubylight_vdisplay_proto::ring`): the handles the
//! host handed over, and the textures (and a fence ring's fences) opened from them on the
//! rendering device.
//!
//! [`RingHandles`] is adopted in the `IOCTL_SET_FENCE_RING` or `IOCTL_SET_FRAME_RING` call: it
//! maps the header and takes ownership of every handle, or takes none of them. [`OpenRing`] is
//! built by the swap-chain worker on its own device, because only that device can copy the
//! swap chain's surface into the textures, and is rebuilt when the ring or the device changes.
//!
//! A fence ring's slots belong to whoever the header's state word names; the driver signals
//! the written fence after its copy and reuses a slot once the host's released fence passed
//! the value recorded in it. The default keyed-mutex ring hands each texture over with its
//! keyed mutex instead, including for hosts before `CAP_FENCE_RING`.

use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

use rubylight_vdisplay_proto::control::SetFrameRingRequest;
use rubylight_vdisplay_proto::ring::{self, Claim, Header, Kind, Picker, SlotMeta, Token};
use wdk_sys::NTSTATUS;
use windows::Win32::Foundation::{HANDLE, LUID};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX, D3D11_TEXTURE2D_DESC, ID3D11Device1, ID3D11Device5,
    ID3D11DeviceContext4, ID3D11Fence, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::IDXGIKeyedMutex;
use windows::Win32::System::Memory::{FILE_MAP_READ, FILE_MAP_WRITE, MapViewOfFile};
use windows::Win32::System::Threading::SetEvent;
use windows::core::Interface;

use crate::direct_3d_device::{Direct3DDevice, luid_key};
use crate::worker::{OwnedHandle, OwnedView};

/// A ring the host handed over. Owns the header view, the event, the texture handles and a
/// fence ring's two fence handles, and closes them when the last holder (the monitor or the
/// worker) drops it.
pub struct RingHandles {
    pub generation: u32,
    pub slot_count: usize,
    pub width: u32,
    pub height: u32,
    pub format: u32,
    /// The adapter the host created the textures on.
    pub adapter: LUID,
    view: OwnedView,
    event: OwnedHandle,
    textures: Vec<OwnedHandle>,
    /// A fence ring's written and released fences; `None` for a keyed-mutex ring.
    fences: Option<(OwnedHandle, OwnedHandle)>,
    /// The producer's sequence and which slot carries which publish. It lives as long as
    /// the ring, not the worker: a swap chain reassigned onto the same ring continues the
    /// sequence the host's `Consumer` expects, where a fresh one would restart at 1 and
    /// every publish would read as already consumed. The written fence follows the same
    /// sequence, so it only ever rises.
    picker: Mutex<Picker>,
}

fn handle(value: u64) -> HANDLE {
    HANDLE(value as usize as *mut core::ffi::c_void)
}

impl RingHandles {
    /// Adopt a validated request's handles, plus a fence ring's `(written, released)` fence
    /// handles. The header is mapped first, and only a mapping that succeeds adopts anything:
    /// on `Err` no handle is owned (or closed) here, and the host reaps them all.
    pub fn adopt(req: &SetFrameRingRequest, fences: Option<(u64, u64)>) -> Result<Self, NTSTATUS> {
        let section = handle(req.header_section);
        // SAFETY: `section` is a handle value the host duplicated into this process. A value
        // that is not a section of at least HEADER_BYTES fails the call and maps nothing.
        let base = unsafe {
            MapViewOfFile(
                section,
                FILE_MAP_READ | FILE_MAP_WRITE,
                0,
                0,
                ring::HEADER_BYTES,
            )
        };
        if base.Value.is_null() {
            dbglog!("[rlvd] SET_FRAME_RING: header section did not map; nothing adopted");
            return Err(crate::STATUS_INVALID_PARAMETER);
        }
        // From here on every handle in the request is ours: the IOCTL succeeds.
        // SAFETY: `base` is the one view of `section` just mapped, and the host transferred
        // the section handle with the request.
        let view = unsafe { OwnedView::from_raw(base.Value, OwnedHandle::from_raw(section)) };
        // SAFETY: transferred with the request, as above.
        let event = unsafe { OwnedHandle::from_raw(handle(req.event)) };
        let slot_count = req.slot_count as usize;
        let textures = req.textures[..slot_count]
            .iter()
            // SAFETY: transferred with the request, as above.
            .map(|&h| unsafe { OwnedHandle::from_raw(handle(h)) })
            .collect();
        // SAFETY: transferred with the request, as above.
        let fences = fences.map(|(written, released)| unsafe {
            (
                OwnedHandle::from_raw(handle(written)),
                OwnedHandle::from_raw(handle(released)),
            )
        });
        let handles = Self {
            generation: req.generation,
            slot_count,
            width: req.width,
            height: req.height,
            format: req.format,
            adapter: LUID {
                LowPart: req.adapter_luid_low,
                HighPart: req.adapter_luid_high,
            },
            view,
            event,
            textures,
            fences,
            picker: Mutex::new(Picker::new(slot_count)),
        };
        handles.header().set_status(ring::STATUS_WAITING, 0);
        Ok(handles)
    }

    /// Give every handle back unclosed (the IOCTL fails after all); only our view of the
    /// header is unmapped.
    pub fn into_unowned(self) {
        self.view.into_unowned();
        self.event.into_unowned();
        for t in self.textures {
            t.into_unowned();
        }
        if let Some((written, released)) = self.fences {
            written.into_unowned();
            released.into_unowned();
        }
    }

    pub fn kind(&self) -> Kind {
        if self.fences.is_some() {
            Kind::Fence
        } else {
            Kind::KeyedMutex
        }
    }

    pub fn header(&self) -> Header<'_> {
        // The view is page-aligned and HEADER_BYTES long, so it holds HEADER_WORDS aligned
        // u64 words; AtomicU64 has u64's layout, and both processes access them atomically.
        #[allow(clippy::cast_ptr_alignment)]
        let words = self.view.base().cast::<AtomicU64>().cast_const();
        // SAFETY: as above; the view lives as long as `self`, which the slice borrows.
        let words = unsafe { core::slice::from_raw_parts(words, ring::HEADER_WORDS) };
        Header::new(words).expect("the mapped header holds HEADER_WORDS words")
    }

    fn signal(&self) {
        // SAFETY: our own event handle, open while `self` lives. A host that passed another
        // object type only makes this fail.
        let _ = unsafe { SetEvent(self.event.as_raw()) };
    }
}

pub use crate::frame_retry::{Frame, Pushed};

struct Slot {
    texture: ID3D11Texture2D,
    /// Keyed-mutex ring only.
    mutex: Option<IDXGIKeyedMutex>,
}

/// A fence ring's fences opened on the worker's device.
struct Fences {
    written: ID3D11Fence,
    released: ID3D11Fence,
    context: ID3D11DeviceContext4,
}

/// The ring's textures opened on one device.
pub struct OpenRing {
    handles: Arc<RingHandles>,
    slots: Vec<Slot>,
    fences: Option<Fences>,
}

/// Why a ring did not open: a `ring::STATUS_*` and its detail for the header.
pub type OpenError = (u32, u32);

/// Take `mutex` with `key` without waiting. `AcquireSync`'s WAIT_TIMEOUT and WAIT_ABANDONED
/// are success HRESULTs the `Result` binding would turn into `Ok`, so only `S_OK` counts.
fn try_acquire(mutex: &IDXGIKeyedMutex, key: u64) -> bool {
    // SAFETY: `mutex` is a live keyed mutex; the vtable slot takes (this, key, milliseconds).
    let hr = unsafe { (Interface::vtable(mutex).AcquireSync)(Interface::as_raw(mutex), key, 0) };
    hr.0 == 0
}

pub fn qpc_now() -> u64 {
    let mut now = 0i64;
    // SAFETY: writes only the live local.
    let _ = unsafe { windows::Win32::System::Performance::QueryPerformanceCounter(&mut now) };
    now.max(0) as u64
}

impl OpenRing {
    /// Open `handles`' textures (and fences) on `device`, checking each against the ring's
    /// shape.
    pub fn open(handles: Arc<RingHandles>, device: &Direct3DDevice) -> Result<Self, OpenError> {
        if luid_key(handles.adapter) != luid_key(device.luid) {
            return Err((ring::STATUS_WRONG_ADAPTER, device.luid.LowPart));
        }
        let failed = |e: windows::core::Error| (ring::STATUS_OPEN_FAILED, e.code().0 as u32);
        let fenced = handles.fences.is_some();
        let device1: ID3D11Device1 = device.device.cast().map_err(failed)?;
        let mut slots = Vec::with_capacity(handles.slot_count);
        for texture in &handles.textures {
            // SAFETY: `texture` is a shared NT handle the host transferred, open while
            // `handles` lives; a handle of any other kind fails the call.
            let texture: ID3D11Texture2D =
                unsafe { device1.OpenSharedResource1(texture.as_raw()) }.map_err(failed)?;
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            // SAFETY: `texture` is live; `desc` is a live local.
            unsafe { texture.GetDesc(&mut desc) };
            let keyed = desc.MiscFlags & D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX.0 as u32 != 0;
            // A fence ring's textures have no keyed mutex to take; a keyed-mutex ring's must.
            if (desc.Width, desc.Height, desc.Format.0 as u32)
                != (handles.width, handles.height, handles.format)
                || desc.ArraySize != 1
                || desc.SampleDesc.Count != 1
                || keyed == fenced
            {
                return Err((ring::STATUS_OPEN_FAILED, 0));
            }
            let mutex = if fenced {
                None
            } else {
                Some(texture.cast::<IDXGIKeyedMutex>().map_err(failed)?)
            };
            slots.push(Slot { texture, mutex });
        }
        let fences = match &handles.fences {
            Some((written, released)) => {
                let device5: ID3D11Device5 = device.device.cast().map_err(failed)?;
                let open = |h: &OwnedHandle| {
                    let mut fence: Option<ID3D11Fence> = None;
                    // SAFETY: `h` is a shared fence handle the host transferred, open while
                    // `handles` lives; `fence` is a live local.
                    unsafe { device5.OpenSharedFence(h.as_raw(), &mut fence) }.map_err(failed)?;
                    fence.ok_or((ring::STATUS_OPEN_FAILED, 0))
                };
                Some(Fences {
                    written: open(written)?,
                    released: open(released)?,
                    context: device.device_context.cast().map_err(failed)?,
                })
            }
            None => None,
        };
        Ok(Self {
            handles,
            slots,
            fences,
        })
    }

    /// Whether this is still the monitor's ring.
    pub fn is(&self, handles: &Arc<RingHandles>) -> bool {
        Arc::ptr_eq(&self.handles, handles)
    }

    pub fn header(&self) -> Header<'_> {
        self.handles.header()
    }

    /// Whether a surface of `desc`'s size and format fits this ring.
    pub fn fits(&self, desc: &D3D11_TEXTURE2D_DESC) -> bool {
        (desc.Width, desc.Height, desc.Format.0 as u32)
            == (self.handles.width, self.handles.height, self.handles.format)
    }

    /// Copy `surface` into a free texture, record its timing and publish it. Never waits on
    /// the host: with no texture to take the frame is dropped and counted.
    pub fn push(
        &mut self,
        device: &Direct3DDevice,
        surface: &ID3D11Texture2D,
        desc: &D3D11_TEXTURE2D_DESC,
        frame: Frame,
    ) -> Pushed {
        self.handles.header().count_acquired();
        self.publish(device, surface, desc, frame, true)
    }

    /// [`Self::push`] for a frame the swap chain did not just deliver: the retained image a
    /// new ring is seeded with.
    pub fn seed(
        &mut self,
        device: &Direct3DDevice,
        image: &ID3D11Texture2D,
        desc: &D3D11_TEXTURE2D_DESC,
        frame: Frame,
    ) -> Pushed {
        self.publish(device, image, desc, frame, true)
    }

    /// Try an already-retained frame again without counting another acquisition or drop.
    /// A successful publication still counts once; failed polls are not additional frames.
    pub fn retry(
        &mut self,
        device: &Direct3DDevice,
        image: &ID3D11Texture2D,
        desc: &D3D11_TEXTURE2D_DESC,
        frame: Frame,
    ) -> Pushed {
        self.publish(device, image, desc, frame, false)
    }

    /// Copy the newest published texture into `into` (same size and format) and return its
    /// timing. In a keyed-mutex ring it is skipped, never waited for, while the host
    /// holds that texture; in a fence ring only this producer writes, so a texture the host
    /// may be reading is read here too, after this device's own copy into it.
    pub fn copy_newest(&self, device: &Direct3DDevice, into: &ID3D11Texture2D) -> Option<Frame> {
        let index = self
            .handles
            .picker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .newest()?;
        let slot = &self.slots[index];
        // Consumed textures are back under KEY_DRIVER, unconsumed ones under KEY_HOST; the
        // mutex goes back with the key it was taken with, so the host's view is unchanged.
        let key = match &slot.mutex {
            Some(mutex) => Some(
                [ring::KEY_DRIVER, ring::KEY_HOST]
                    .into_iter()
                    .find(|&key| try_acquire(mutex, key))?,
            ),
            None => None,
        };
        let meta = self.handles.header().slot(index);
        // SAFETY: both textures are live on this device with the ring's size and format (the
        // caller checked `into` with `fits`); this thread holds the slot's mutex, or the slot
        // is a fence ring's, which only this thread writes.
        unsafe { device.device_context.CopyResource(into, &slot.texture) };
        if let (Some(mutex), Some(key)) = (&slot.mutex, key) {
            // SAFETY: taken above with `key` on this thread.
            let _ = unsafe { mutex.ReleaseSync(key) };
        }
        Some(Frame {
            present_qpc: meta.present_qpc,
            acquire_qpc: meta.acquire_qpc,
            frame_number: meta.frame_number,
        })
    }

    /// Whether `surface` fits; a mismatch is reported in the header for the host to rebuild.
    fn check_shape(&self, desc: &D3D11_TEXTURE2D_DESC) -> bool {
        let header = self.handles.header();
        header.set_surface(desc.Width, desc.Height, desc.Format.0 as u32);
        if (desc.Width, desc.Height) != (self.handles.width, self.handles.height) {
            header.set_status(ring::STATUS_SIZE_MISMATCH, 0);
            return false;
        }
        if desc.Format.0 as u32 != self.handles.format {
            header.set_status(ring::STATUS_FORMAT_MISMATCH, desc.Format.0 as u32);
            return false;
        }
        true
    }

    fn publish(
        &mut self,
        device: &Direct3DDevice,
        surface: &ID3D11Texture2D,
        desc: &D3D11_TEXTURE2D_DESC,
        frame: Frame,
        count_drop: bool,
    ) -> Pushed {
        if !self.check_shape(desc) {
            return Pushed::Mismatch;
        }
        // Only the one worker attached to this ring pushes, so the lock is uncontended; a
        // worker that panicked under it leaves a picker that is still consistent.
        let handles = self.handles.clone();
        let mut picker = handles
            .picker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let pushed = match &self.fences {
            Some(fences) => {
                self.publish_fenced(fences, &mut picker, device, surface, frame, count_drop)
            }
            None => self.publish_keyed(&mut picker, device, surface, frame, count_drop),
        };
        if pushed == Pushed::Published {
            let header = handles.header();
            header.set_status(ring::STATUS_STREAMING, 0);
            handles.signal();
        }
        pushed
    }

    fn meta(seq: u32, frame: Frame) -> SlotMeta {
        SlotMeta {
            seq: u64::from(seq),
            present_qpc: frame.present_qpc,
            acquire_qpc: frame.acquire_qpc,
            publish_qpc: qpc_now(),
            frame_number: frame.frame_number,
        }
    }

    fn device_failed(device: &Direct3DDevice, header: &Header<'_>) -> Pushed {
        // SAFETY: a plain status query on the live device.
        if unsafe { device.device.GetDeviceRemovedReason() }.is_err() {
            device.mark_removed();
            header.set_status(ring::STATUS_DEVICE_LOST, 0);
            return Pushed::DeviceLost;
        }
        Pushed::Dropped
    }

    fn publish_fenced(
        &self,
        fences: &Fences,
        picker: &mut Picker,
        device: &Direct3DDevice,
        surface: &ID3D11Texture2D,
        frame: Frame,
        count_drop: bool,
    ) -> Pushed {
        let header = self.handles.header();
        if picker.needs_renewal() {
            header.set_status(ring::STATUS_RENEW_REQUIRED, 0);
            self.handles.signal();
            return Pushed::Dropped;
        }
        // The host's reads of a slot are done once the released fence passed the value it
        // recorded there. Only read, never waited for. A host whose device was lost leaves
        // the fence at u64::MAX, which frees every slot.
        // SAFETY: a plain read of a fence opened on this device.
        let released = unsafe { fences.released.GetCompletedValue() };
        let Some(claim) = picker.claim_fenced(&header, released) else {
            if count_drop {
                header.count_dropped();
            }
            return Pushed::Dropped;
        };
        if matches!(claim, Claim::Reclaimed(_)) {
            header.count_overwritten();
        }
        let index = claim.slot();
        let seq = picker.next_seq();
        // SAFETY: both textures are live on this device, same size and format (checked
        // above and at open); the slot is ours in `Writing`, and the context is
        // multithread-protected.
        unsafe {
            device
                .device_context
                .CopyResource(&self.slots[index].texture, surface)
        };
        // The host's reads of this slot wait on the GPU for this signal; the flush submits it
        // now instead of at the next batch.
        // SAFETY: the fence and the context are this device's.
        let signalled = unsafe { fences.context.Signal(&fences.written, u64::from(seq)) };
        // SAFETY: a plain flush of the multithread-protected immediate context.
        unsafe { device.device_context.Flush() };
        if let Err(e) = signalled {
            dbglog!("[rlvd] frame ring: written fence signal failed: {e:?}");
            picker.abandon_fenced(&header, index, seq);
            return Self::device_failed(device, &header);
        }
        header.write_slot(index, Self::meta(seq, frame));
        if !picker.publish_fenced(&header, index, seq) {
            dbglog!("[rlvd] frame ring: slot {index} changed under the driver; frame dropped");
            if count_drop {
                header.count_dropped();
            }
            return Pushed::Dropped;
        }
        header.publish(Token {
            generation: self.handles.generation,
            seq,
            slot: index as u8,
        });
        Pushed::Published
    }

    /// Publish through the default keyed-mutex transport.
    fn publish_keyed(
        &self,
        picker: &mut Picker,
        device: &Direct3DDevice,
        surface: &ID3D11Texture2D,
        frame: Frame,
        count_drop: bool,
    ) -> Pushed {
        let header = self.handles.header();
        let slots = &self.slots;
        let mutex = |s: usize| slots[s].mutex.as_ref().expect("a keyed-mutex ring's slot");
        let Some(claim) = picker.claim(|s, key| try_acquire(mutex(s), key)) else {
            if count_drop {
                header.count_dropped();
            }
            return Pushed::Dropped;
        };
        if matches!(claim, Claim::Reclaimed(_)) {
            header.count_overwritten();
        }
        let index = claim.slot();
        // SAFETY: both textures are live on this device, same size and format (checked
        // above and at open); the context is multithread-protected.
        unsafe {
            device
                .device_context
                .CopyResource(&slots[index].texture, surface)
        };
        let seq = picker.next_seq();
        header.write_slot(index, Self::meta(seq, frame));
        // The release orders the host's acquire after the copy on the GPU; the flush submits
        // it now instead of at the next batch.
        // SAFETY: this thread took the mutex in `claim`; the context is this device's.
        let released = unsafe { mutex(index).ReleaseSync(ring::KEY_HOST) };
        // SAFETY: a plain flush of the multithread-protected immediate context.
        unsafe { device.device_context.Flush() };
        if let Err(e) = released {
            dbglog!("[rlvd] frame ring: ReleaseSync failed: {e:?}");
            return Self::device_failed(device, &header);
        }
        picker.published(index, seq);
        header.publish(Token {
            generation: self.handles.generation,
            seq,
            slot: index as u8,
        });
        Pushed::Published
    }
}
