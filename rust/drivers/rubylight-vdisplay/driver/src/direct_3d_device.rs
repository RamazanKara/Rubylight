//! The render-side D3D12 device bound to IddCx swap chains, and the device the frame ring's
//! textures and fences open on.
//!
//! From punktfunk's pf-vdisplay `direct_3d_device.rs` by Enrico Bühler (unom), MIT OR
//! Apache-2.0, moved from D3D11 to D3D12 (IddCx 1.11's D3D12 swap chain).
//!
//! One device per render LUID, pooled across every swap-chain assignment ([`pooled_device`]):
//! creating one per assignment leaked a vendor driver thread set and VRAM on every reconnect.
//! A D3D12 device is free-threaded; each swap-chain worker records and submits on its own
//! queue ([`crate::copier::Copier`]).

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use windows::{
    Win32::{
        Foundation::{E_FAIL, LUID},
        Graphics::{
            Direct3D::D3D_FEATURE_LEVEL_11_0,
            Direct3D12::{D3D12CreateDevice, ID3D12Device},
            Dxgi::{CreateDXGIFactory2, DXGI_CREATE_FACTORY_FLAGS, IDXGIAdapter1, IDXGIFactory5},
        },
    },
    core::Error,
};

pub struct Direct3DDevice {
    _dxgi_factory: IDXGIFactory5,
    _adapter: IDXGIAdapter1,
    pub device: ID3D12Device,
    /// The adapter this device was created on.
    pub luid: LUID,
    /// Minted per successful `init`, so a TDR recreate on the same LUID is a new epoch.
    epoch: u32,
    /// Set once any worker saw this device removed; the pool then recreates.
    removed: AtomicBool,
}

impl Direct3DDevice {
    pub fn epoch(&self) -> u32 {
        self.epoch
    }

    pub fn is_removed(&self) -> bool {
        self.removed.load(Ordering::Acquire)
    }

    pub fn mark_removed(&self) {
        if !self.removed.swap(true, Ordering::AcqRel) {
            dbglog!(
                "[rlvd] D3D device epoch {} removed; workers stop, the pool recreates",
                self.epoch
            );
        }
    }

    fn init(adapter_luid: LUID) -> Result<Self, Error> {
        // SAFETY: a plain DXGI factory creation.
        let dxgi_factory =
            unsafe { CreateDXGIFactory2::<IDXGIFactory5>(DXGI_CREATE_FACTORY_FLAGS(0))? };
        // SAFETY: `dxgi_factory` is the live factory; the LUID is passed by value.
        let adapter = unsafe { dxgi_factory.EnumAdapterByLuid::<IDXGIAdapter1>(adapter_luid)? };
        let mut device: Option<ID3D12Device> = None;
        // The device only copies the swap chain's surfaces into the host's textures.
        // SAFETY: `adapter` is a live adapter; `device` is a live local checked below.
        unsafe { D3D12CreateDevice(&adapter, D3D_FEATURE_LEVEL_11_0, &mut device)? };
        let device = device.ok_or_else(|| Error::new(E_FAIL, "ID3D12Device not found"))?;
        let epoch = DEVICE_EPOCH.fetch_add(1, Ordering::AcqRel) + 1;
        dbglog!("[rlvd] Direct3DDevice::init OK (D3D12), epoch {epoch}");
        Ok(Self {
            _dxgi_factory: dxgi_factory,
            _adapter: adapter,
            device,
            luid: adapter_luid,
            epoch,
            removed: AtomicBool::new(false),
        })
    }
}

/// One device per render LUID, bounded so a hybrid PC's two adapters do not evict each other.
static DEVICE_POOL: Mutex<Vec<(i64, Arc<Direct3DDevice>)>> = Mutex::new(Vec::new());
const DEVICE_POOL_CAP: usize = 4;
static DEVICE_EPOCH: AtomicU32 = AtomicU32::new(0);

/// A LUID as one comparable number.
pub fn luid_key(luid: LUID) -> i64 {
    (i64::from(luid.HighPart) << 32) | i64::from(luid.LowPart)
}

/// Get or create the pooled device for `luid`, recreating one that was removed.
pub fn pooled_device(luid: LUID) -> Option<Arc<Direct3DDevice>> {
    let key = luid_key(luid);
    let mut pool = crate::registry::lock(&DEVICE_POOL);
    if let Some(pos) = pool.iter().position(|(k, _)| *k == key) {
        let dev = &pool[pos].1;
        // SAFETY: a plain status query on the live pooled device.
        let alive = !dev.is_removed() && unsafe { dev.device.GetDeviceRemovedReason() }.is_ok();
        if alive {
            return Some(dev.clone());
        }
        dev.mark_removed();
        pool.remove(pos);
    }
    match Direct3DDevice::init(luid) {
        Ok(d) => {
            let a = Arc::new(d);
            if pool.len() >= DEVICE_POOL_CAP {
                pool.remove(0);
            }
            pool.push((key, a.clone()));
            Some(a)
        }
        Err(e) => {
            dbglog!("[rlvd] pooled Direct3DDevice::init failed: {e:?}");
            None
        }
    }
}
