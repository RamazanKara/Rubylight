//! The driver's half of the frame ring (`rubylight_vdisplay_proto::ring`): the handles the
//! host handed over, and the textures and fences opened from them on the rendering device.
//!
//! [`RingHandles`] is adopted in the `IOCTL_SET_FENCE_RING` call: it maps the header and takes
//! ownership of every handle, or takes none of them. [`OpenRing`] is built by the swap-chain
//! worker on its own device, because only that device can copy the swap chain's surface into
//! the textures, and is rebuilt when the ring or the device changes.
//!
//! A slot belongs to whoever the header's state word names. The driver copies on its copy
//! queue ([`Copier`]), signals the written fence with the slot's sequence after the copy on
//! the same queue, and reuses a slot once the host's released fence passed the value recorded
//! in it. The older keyed-mutex ring (`IOCTL_SET_FRAME_RING`) is refused at the IOCTL.

use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

use rubylight_vdisplay_proto::control::SetFrameRingRequest;
use rubylight_vdisplay_proto::ring::{self, Claim, Header, Picker, SlotMeta, Token};
use wdk_sys::NTSTATUS;
use windows::Win32::Foundation::{HANDLE, LUID};
use windows::Win32::Graphics::Direct3D12::{
    D3D12_RESOURCE_DESC, D3D12_RESOURCE_DIMENSION_TEXTURE2D, ID3D12Fence, ID3D12Resource,
};
use windows::Win32::System::Memory::{FILE_MAP_READ, FILE_MAP_WRITE, MapViewOfFile};
use windows::Win32::System::Threading::SetEvent;

use crate::copier::{Copier, CopyError};
use crate::direct_3d_device::{Direct3DDevice, luid_key};
use crate::worker::{OwnedHandle, OwnedView};

/// A ring the host handed over. Owns the header view, the event, the texture handles and the
/// two fence handles, and closes them when the last holder (the monitor or the worker) drops
/// it.
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
    /// The written and released fences.
    fences: (OwnedHandle, OwnedHandle),
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
    /// Adopt a validated request's handles, plus its `(written, released)` fence handles. The
    /// header is mapped first, and only a mapping that succeeds adopts anything: on `Err` no
    /// handle is owned (or closed) here, and the host reaps them all.
    pub fn adopt(req: &SetFrameRingRequest, fences: (u64, u64)) -> Result<Self, NTSTATUS> {
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
            dbglog!("[rlvd] SET_FENCE_RING: header section did not map; nothing adopted");
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
        let fences = unsafe {
            (
                OwnedHandle::from_raw(handle(fences.0)),
                OwnedHandle::from_raw(handle(fences.1)),
            )
        };
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
        self.fences.0.into_unowned();
        self.fences.1.into_unowned();
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

/// One frame as the swap chain delivered it.
#[derive(Clone, Copy)]
pub struct Frame {
    pub present_qpc: u64,
    pub acquire_qpc: u64,
    pub frame_number: u64,
}

/// What [`OpenRing::push`] did with a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pushed {
    Published,
    /// Every texture was held by the host, or the copy queue was still busy.
    Dropped,
    /// The surface is not the ring's size or format; the host rebuilds the ring.
    Mismatch,
    /// The device failed under the copy; the worker exits for a reassignment.
    DeviceLost,
}

/// The ring's fences opened on the worker's device.
struct Fences {
    written: ID3D12Fence,
    released: ID3D12Fence,
}

/// The ring's textures opened on one device.
pub struct OpenRing {
    handles: Arc<RingHandles>,
    slots: Vec<ID3D12Resource>,
    fences: Fences,
}

/// Why a ring did not open: a `ring::STATUS_*` and its detail for the header.
pub type OpenError = (u32, u32);

pub fn qpc_now() -> u64 {
    let mut now = 0i64;
    // SAFETY: writes only the live local.
    let _ = unsafe { windows::Win32::System::Performance::QueryPerformanceCounter(&mut now) };
    now.max(0) as u64
}

/// A texture's `(width, height, format)`. Texture widths fit in 32 bits.
pub fn shape(desc: &D3D12_RESOURCE_DESC) -> (u32, u32, u32) {
    (desc.Width as u32, desc.Height, desc.Format.0 as u32)
}

impl OpenRing {
    /// Open `handles`' textures and fences on `device`, checking each texture against the
    /// ring's shape.
    pub fn open(handles: Arc<RingHandles>, device: &Direct3DDevice) -> Result<Self, OpenError> {
        if luid_key(handles.adapter) != luid_key(device.luid) {
            return Err((ring::STATUS_WRONG_ADAPTER, device.luid.LowPart));
        }
        let failed = |e: windows::core::Error| (ring::STATUS_OPEN_FAILED, e.code().0 as u32);
        let mut slots = Vec::with_capacity(handles.slot_count);
        for texture in &handles.textures {
            let mut opened: Option<ID3D12Resource> = None;
            // SAFETY: `texture` is a shared NT handle the host transferred, open while
            // `handles` lives; a handle of any other kind fails the call. `opened` is a live
            // local.
            unsafe {
                device
                    .device
                    .OpenSharedHandle(texture.as_raw(), &mut opened)
            }
            .map_err(failed)?;
            let texture = opened.ok_or((ring::STATUS_OPEN_FAILED, 0))?;
            // SAFETY: a plain query of the live resource.
            let desc = unsafe { texture.GetDesc() };
            if desc.Dimension != D3D12_RESOURCE_DIMENSION_TEXTURE2D
                || desc.Width > u64::from(u32::MAX)
                || shape(&desc) != (handles.width, handles.height, handles.format)
                || desc.DepthOrArraySize != 1
                || desc.MipLevels != 1
                || desc.SampleDesc.Count != 1
            {
                return Err((ring::STATUS_OPEN_FAILED, 0));
            }
            slots.push(texture);
        }
        let open = |h: &OwnedHandle| -> Result<ID3D12Fence, OpenError> {
            let mut fence: Option<ID3D12Fence> = None;
            // SAFETY: `h` is a shared fence handle the host transferred, open while `handles`
            // lives; a handle of any other kind fails the call. `fence` is a live local.
            unsafe { device.device.OpenSharedHandle(h.as_raw(), &mut fence) }.map_err(failed)?;
            fence.ok_or((ring::STATUS_OPEN_FAILED, 0))
        };
        let fences = Fences {
            written: open(&handles.fences.0)?,
            released: open(&handles.fences.1)?,
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
    pub fn fits(&self, desc: &D3D12_RESOURCE_DESC) -> bool {
        shape(desc) == (self.handles.width, self.handles.height, self.handles.format)
    }

    /// Copy `surface` into a free texture, record its timing and publish it. Never waits on
    /// the host: with no texture to take the frame is dropped and counted.
    pub fn push(
        &mut self,
        device: &Direct3DDevice,
        copier: &mut Copier,
        surface: &ID3D12Resource,
        desc: &D3D12_RESOURCE_DESC,
        frame: Frame,
    ) -> Pushed {
        self.handles.header().count_acquired();
        self.publish(device, copier, surface, desc, frame)
    }

    /// [`Self::push`] for a frame the swap chain did not just deliver: the retained image a
    /// new ring is seeded with.
    pub fn seed(
        &mut self,
        device: &Direct3DDevice,
        copier: &mut Copier,
        image: &ID3D12Resource,
        desc: &D3D12_RESOURCE_DESC,
        frame: Frame,
    ) -> Pushed {
        self.publish(device, copier, image, desc, frame)
    }

    /// Copy the newest published texture into `into` (same size and format) and return its
    /// frame number. Only this producer writes the slots, and its queue orders this read
    /// after its own copy into the slot; the host may be reading the slot too.
    pub fn copy_newest(&self, copier: &mut Copier, into: &ID3D12Resource) -> Option<u64> {
        let index = self
            .handles
            .picker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .newest()?;
        let frame_number = self.handles.header().slot(index).frame_number;
        match copier.copy(&[(into, &self.slots[index])], None) {
            Ok(()) => Some(frame_number),
            Err(e) => {
                dbglog!("[rlvd] frame ring: copying the newest frame back failed: {e:?}");
                None
            }
        }
    }

    /// Whether `surface` fits; a mismatch is reported in the header for the host to rebuild.
    fn check_shape(&self, desc: &D3D12_RESOURCE_DESC) -> bool {
        let header = self.handles.header();
        let (width, height, format) = shape(desc);
        header.set_surface(width, height, format);
        if (width, height) != (self.handles.width, self.handles.height) {
            header.set_status(ring::STATUS_SIZE_MISMATCH, 0);
            return false;
        }
        if format != self.handles.format {
            header.set_status(ring::STATUS_FORMAT_MISMATCH, format);
            return false;
        }
        true
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

    fn publish(
        &mut self,
        device: &Direct3DDevice,
        copier: &mut Copier,
        surface: &ID3D12Resource,
        desc: &D3D12_RESOURCE_DESC,
        frame: Frame,
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
        let header = handles.header();
        // The host's reads of a slot are done once the released fence passed the value it
        // recorded there. Only read, never waited for. A host whose device was lost leaves
        // the fence at u64::MAX, which frees every slot.
        // SAFETY: a plain read of a fence opened on this device.
        let released = unsafe { self.fences.released.GetCompletedValue() };
        let Some(claim) = picker.claim_fenced(&header, released) else {
            header.count_dropped();
            return Pushed::Dropped;
        };
        if matches!(claim, Claim::Reclaimed(_)) {
            header.count_overwritten();
        }
        let index = claim.slot();
        let seq = picker.next_seq();
        // The host's reads of this slot wait on the GPU for the written fence, which the copy
        // queue signals after the copy.
        match copier.copy(
            &[(&self.slots[index], surface)],
            Some((&self.fences.written, u64::from(seq))),
        ) {
            Ok(()) => {}
            Err(CopyError::Busy) => {
                picker.abandon_fenced(&header, index, seq);
                header.count_dropped();
                return Pushed::Dropped;
            }
            Err(CopyError::Failed(e)) => {
                dbglog!("[rlvd] frame ring: copy or written fence signal failed: {e:?}");
                picker.abandon_fenced(&header, index, seq);
                return Self::device_failed(device, &header);
            }
        }
        header.write_slot(index, Self::meta(seq, frame));
        if !picker.publish_fenced(&header, index, seq) {
            dbglog!("[rlvd] frame ring: slot {index} changed under the driver; frame dropped");
            header.count_dropped();
            return Pushed::Dropped;
        }
        header.publish(Token {
            generation: self.handles.generation,
            seq,
            slot: index as u8,
        });
        header.set_status(ring::STATUS_STREAMING, 0);
        handles.signal();
        Pushed::Published
    }
}
