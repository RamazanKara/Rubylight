//! Typed wrappers over the wdk-sys IddCx DDIs, dispatched through the `IddFunctions` table
//! (indexed by `_IDDFUNCENUM::<Name>TableIndex`, with `IddDriverGlobals` as the implicit first
//! argument), the same model wdk-sys uses for WDF.
//!
//! From punktfunk's `wdk-iddcx` by Enrico Bühler (unom), MIT OR Apache-2.0, trimmed to the
//! DDIs the Rubylight driver calls (no hardware cursor, no remote-session adapter).
//!
//! Each DDI pins its `(_IDDFUNCENUM index, PFN_* type)` pair in exactly one macro invocation:
//! a wrong pairing is the only way the table dispatch can be undefined behaviour, so it is
//! never written twice. Every wrapper returns the raw `NTSTATUS`; the swap-chain DDIs return
//! an HRESULT-shaped value the caller classifies.
#![no_std]
#![allow(non_snake_case, clippy::missing_safety_doc)]

pub use wdk_sys::iddcx;

use wdk_sys::{NTSTATUS, PWDFDEVICE_INIT, WDFDEVICE};

/// `NT_SUCCESS`: an NTSTATUS is an error iff negative.
#[inline]
#[must_use]
pub const fn nt_success(status: NTSTATUS) -> bool {
    status >= 0
}

/// Read one typed DDI from IddCxStub's runtime-populated table. The binding declares a
/// zero-length array, so `wrapping_add` addresses the foreign tail without claiming it lies
/// inside that Rust object.
///
/// # Safety
/// `index` and `T` name the same DDI, and IddCxStub has populated that slot.
#[inline]
unsafe fn ddi<T: Copy>(index: i32) -> T {
    assert!(index >= 0, "negative IddCx DDI index");
    assert_eq!(
        core::mem::size_of::<T>(),
        core::mem::size_of::<iddcx::PFN_IDD_CX>(),
        "IddCx DDI type has the wrong width"
    );
    let table = (&raw const iddcx::IddFunctions).cast::<iddcx::PFN_IDD_CX>();
    let slot = table.wrapping_add(index as usize);
    // SAFETY: the contract says IddCxStub populated this slot with the matching `T`.
    unsafe { slot.cast::<T>().read() }
}

/// The IddCx driver globals IddCxStub sets, passed as the first argument of every DDI.
///
/// # Safety
/// Only valid once the IddCx runtime has loaded the driver.
#[inline]
unsafe fn globals() -> iddcx::PIDD_DRIVER_GLOBALS {
    // SAFETY: only the pointer value of the stub-provided global is read.
    unsafe { (&raw const iddcx::IddDriverGlobals).read() }
}

macro_rules! iddcx_ddi {
    (
        $(#[$meta:meta])*
        $name:ident ( $( $arg:ident : $aty:ty ),* $(,)? ) @ $idx:ident as $pfn:ident
    ) => {
        $(#[$meta])*
        ///
        /// # Safety
        /// Call only after IddCx loaded the driver; pointers must meet the IddCx contract.
        #[inline]
        pub unsafe fn $name( $( $arg: $aty ),* ) -> NTSTATUS {
            // SAFETY: `$idx`/`$pfn` are the matched table index and PFN type, pinned by this
            // invocation, and the table is populated once the driver is loaded.
            let f: iddcx::$pfn = unsafe { ddi(iddcx::_IDDFUNCENUM::$idx) };
            // SAFETY: reads the stub-provided globals pointer; valid after load.
            let g = unsafe { globals() };
            // SAFETY: a populated DDI called with the stub globals and caller-valid arguments.
            unsafe { (f.unwrap())(g, $( $arg ),* ) }
        }
    };
    (
        $(#[$meta:meta])*
        $name:ident ( $( $arg:ident : $aty:ty ),* $(,)? ) @ $idx:ident as $pfn:ident -> ()
    ) => {
        $(#[$meta])*
        ///
        /// # Safety
        /// Call only after IddCx loaded the driver; pointers must meet the IddCx contract.
        #[inline]
        pub unsafe fn $name( $( $arg: $aty ),* ) {
            // SAFETY: `$idx`/`$pfn` are the matched table index and PFN type, pinned by this
            // invocation, and the table is populated once the driver is loaded.
            let f: iddcx::$pfn = unsafe { ddi(iddcx::_IDDFUNCENUM::$idx) };
            // SAFETY: reads the stub-provided globals pointer; valid after load.
            let g = unsafe { globals() };
            // SAFETY: a populated DDI called with the stub globals and caller-valid arguments.
            unsafe { (f.unwrap())(g, $( $arg ),* ) }
        }
    };
}

iddcx_ddi!(
    /// Configure a `WDFDEVICE_INIT` for IddCx, before `WdfDeviceCreate`.
    IddCxDeviceInitConfig(device_init: PWDFDEVICE_INIT, config: *const iddcx::IDD_CX_CLIENT_CONFIG)
        @ IddCxDeviceInitConfigTableIndex as PFN_IDDCXDEVICEINITCONFIG
);
iddcx_ddi!(
    /// Finish IddCx device init, after `WdfDeviceCreate`.
    IddCxDeviceInitialize(device: WDFDEVICE)
        @ IddCxDeviceInitializeTableIndex as PFN_IDDCXDEVICEINITIALIZE
);
iddcx_ddi!(
    /// Create the WDDM adapter asynchronously; it arrives in `EvtIddCxAdapterInitFinished`.
    IddCxAdapterInitAsync(
        in_args: *const iddcx::IDARG_IN_ADAPTER_INIT,
        out_args: *mut iddcx::IDARG_OUT_ADAPTER_INIT,
    ) @ IddCxAdapterInitAsyncTableIndex as PFN_IDDCXADAPTERINITASYNC
);
iddcx_ddi!(
    /// Create a monitor on the adapter.
    IddCxMonitorCreate(
        adapter: iddcx::IDDCX_ADAPTER,
        in_args: *const iddcx::IDARG_IN_MONITORCREATE,
        out_args: *mut iddcx::IDARG_OUT_MONITORCREATE,
    ) @ IddCxMonitorCreateTableIndex as PFN_IDDCXMONITORCREATE
);
iddcx_ddi!(
    /// Plug a created monitor in; the OS target id and adapter LUID come back.
    IddCxMonitorArrival(
        monitor: iddcx::IDDCX_MONITOR,
        out_args: *mut iddcx::IDARG_OUT_MONITORARRIVAL,
    ) @ IddCxMonitorArrivalTableIndex as PFN_IDDCXMONITORARRIVAL
);
iddcx_ddi!(
    /// Unplug a monitor.
    IddCxMonitorDeparture(monitor: iddcx::IDDCX_MONITOR)
        @ IddCxMonitorDepartureTableIndex as PFN_IDDCXMONITORDEPARTURE
);
iddcx_ddi!(
    /// Set the preferred render adapter of the virtual adapter.
    IddCxAdapterSetRenderAdapter(
        adapter: iddcx::IDDCX_ADAPTER,
        in_args: *const iddcx::IDARG_IN_ADAPTERSETRENDERADAPTER,
    ) @ IddCxAdapterSetRenderAdapterTableIndex as PFN_IDDCXADAPTERSETRENDERADAPTER -> ()
);
iddcx_ddi!(
    /// Replace a live monitor's target modes (the HDR `*2` variant, IddCx 1.10) without a
    /// departure, so the host can then set a newly offered mode in place.
    IddCxMonitorUpdateModes2(
        monitor: iddcx::IDDCX_MONITOR,
        in_args: *const iddcx::IDARG_IN_UPDATEMODES2,
    ) @ IddCxMonitorUpdateModes2TableIndex as PFN_IDDCXMONITORUPDATEMODES2
);
iddcx_ddi!(
    /// Bind a D3D device to an assigned swap chain. HRESULT-shaped; a failure means the OS
    /// already unassigned it.
    IddCxSwapChainSetDevice(
        swap_chain: iddcx::IDDCX_SWAPCHAIN,
        in_args: *const iddcx::IDARG_IN_SWAPCHAINSETDEVICE,
    ) @ IddCxSwapChainSetDeviceTableIndex as PFN_IDDCXSWAPCHAINSETDEVICE
);
iddcx_ddi!(
    /// Release the previous frame and acquire the next (FP16-capable variant).
    /// HRESULT-shaped; `E_PENDING` means wait on the surface-available event.
    IddCxSwapChainReleaseAndAcquireBuffer2(
        swap_chain: iddcx::IDDCX_SWAPCHAIN,
        in_args: *mut iddcx::IDARG_IN_RELEASEANDACQUIREBUFFER2,
        out_args: *mut iddcx::IDARG_OUT_RELEASEANDACQUIREBUFFER2,
    ) @ IddCxSwapChainReleaseAndAcquireBuffer2TableIndex as PFN_IDDCXSWAPCHAINRELEASEANDACQUIREBUFFER2
);
iddcx_ddi!(
    /// Report the acquired frame processed. HRESULT-shaped.
    IddCxSwapChainFinishedProcessingFrame(swap_chain: iddcx::IDDCX_SWAPCHAIN)
        @ IddCxSwapChainFinishedProcessingFrameTableIndex as PFN_IDDCXSWAPCHAINFINISHEDPROCESSINGFRAME
);
iddcx_ddi!(
    /// Raise the swap chain's processing device to realtime GPU priority (IddCx 1.9), so
    /// frame processing outruns ordinary GPU contention. Best-effort at the call site.
    IddCxSetRealtimeGPUPriority(
        swap_chain: iddcx::IDDCX_SWAPCHAIN,
        in_args: *const iddcx::IDARG_IN_SETREALTIMEGPUPRIORITY,
    ) @ IddCxSetRealtimeGPUPriorityTableIndex as PFN_IDDCXSETREALTIMEGPUPRIORITY
);

iddcx_ddi!(
    /// The IddCx framework version the OS runs (`IDDCX_VERSION_*`, IddCx 1.2).
    IddCxGetVersion(out_args: *mut iddcx::IDARG_OUT_GETVERSION)
        @ IddCxGetVersionTableIndex as PFN_IDDCXGETVERSION
);

/// How many DDIs the running framework provides. Like the WDK's `IddFunctionCount`, it is
/// only valid when [`client_newer_than_framework`] is true; otherwise it is not set.
///
/// # Safety
/// Only valid once the IddCx runtime has loaded the driver.
#[must_use]
pub unsafe fn function_count() -> u32 {
    // SAFETY: reads the stub-populated count; valid after load.
    unsafe { (&raw const iddcx::IddFunctionCount).read() }
}

/// The WDK's `IddClientVersionHigherThanFramework`: the driver was built against a newer
/// IddCx than the framework it runs on, so [`function_count`] says which DDIs exist.
///
/// # Safety
/// Only valid once the IddCx runtime has loaded the driver.
#[must_use]
pub unsafe fn client_newer_than_framework() -> bool {
    // SAFETY: reads the stub-populated flag; valid after load.
    unsafe { (&raw const iddcx::IddClientVersionHigherThanFramework).read() != 0 }
}

/// The WDK's `IDD_IS_FUNCTION_AVAILABLE` for a DDI past the always-available ones: a driver
/// built for an IddCx no newer than the framework has every DDI of its own table, and a
/// newer one only those below the framework's count.
#[must_use]
pub const fn function_available(index: u32, client_newer: bool, count: u32) -> bool {
    !client_newer || index < count
}

/// `IDARG_OUT_FEATURES_SUPPORTED` from the IddCx 1.11 header, which the WDK these bindings
/// come from (IddCx 1.10) does not have.
#[repr(C)]
#[derive(Default)]
pub struct FeaturesSupported {
    pub size: u32,
    /// `IDDCX_DEVICE_FEATURES_1_11`.
    pub features_1_11: u32,
}

/// `IDDCX_DEVICE_FEATURES_1_11_D3D12`: the OS supports D3D12 swap chains.
pub const FEATURES_1_11_D3D12: u32 = 0x1;

/// `IddCxCheckOsFeatureSupportTableIndex` in the IddCx 1.11 header.
const CHECK_OS_FEATURE_SUPPORT_INDEX: u32 = 36;

/// `IddCxCheckOsFeatureSupport` (IddCx 1.11), or `None` when the driver cannot call it or the
/// running framework has no such DDI. IddCx requires the call before any adapter is created.
///
/// `stub_has_slot` says the linked `IddCxStub.lib` is IddCx 1.11 or newer, so its table has
/// the slot. The default build links the 1.10 stub, whose table ends at slot 35: reading slot
/// 36 there would run past it, so with `false` this returns `None` without touching the table.
///
/// # Safety
/// Only valid once the IddCx runtime has loaded the driver, before `IddCxAdapterInitAsync`.
/// `stub_has_slot` must be true only for a stub whose table holds slot 36.
pub unsafe fn check_os_feature_support(
    stub_has_slot: bool,
) -> Option<(NTSTATUS, FeaturesSupported)> {
    type Pfn = Option<
        unsafe extern "C" fn(iddcx::PIDD_DRIVER_GLOBALS, *mut FeaturesSupported) -> NTSTATUS,
    >;
    if !stub_has_slot {
        return None;
    }
    // SAFETY: valid after load (this function's contract).
    let (newer, count) = unsafe { (client_newer_than_framework(), function_count()) };
    if !function_available(CHECK_OS_FEATURE_SUPPORT_INDEX, newer, count) {
        return None;
    }
    let mut out = FeaturesSupported {
        size: core::mem::size_of::<FeaturesSupported>() as u32,
        features_1_11: 0,
    };
    // SAFETY: the stub's table holds the slot (`stub_has_slot`) and the framework provides it
    // (checked above); the 1.11 header pins its type to `PFN_IDDCXCHECKOSFEATURESUPPORT`,
    // which `Pfn` matches.
    let f: Pfn = unsafe { ddi(CHECK_OS_FEATURE_SUPPORT_INDEX as i32) };
    // SAFETY: reads the stub-provided globals pointer; valid after load.
    let g = unsafe { globals() };
    // SAFETY: a populated DDI called with the stub globals and a valid out block.
    let status = unsafe { (f?)(g, &mut out) };
    Some((status, out))
}

#[cfg(test)]
mod tests {
    use super::function_available;

    #[test]
    fn a_driver_not_newer_than_the_framework_has_every_ddi_of_its_table() {
        // The count is not set in this case; whatever it holds must not matter.
        assert!(function_available(36, false, 0));
        assert!(function_available(36, false, 36));
    }

    #[test]
    fn a_newer_driver_has_only_the_ddis_below_the_frameworks_count() {
        assert!(function_available(35, true, 36));
        assert!(!function_available(36, true, 36));
        assert!(function_available(36, true, 37));
        assert!(!function_available(36, true, 0));
    }
}

/// Every struct the driver builds or reads must stay `Sized`: this fails to compile if the
/// bindgen allowlist drops one or a field type stops resolving.
macro_rules! assert_sized {
    ($($t:ident),+ $(,)?) => { $( const _: usize = core::mem::size_of::<iddcx::$t>(); )+ };
}

assert_sized!(
    IDD_CX_CLIENT_CONFIG,
    IDARG_IN_ADAPTER_INIT,
    IDARG_OUT_ADAPTER_INIT,
    IDDCX_ADAPTER_CAPS,
    IDDCX_ENDPOINT_VERSION,
    IDARG_IN_MONITORCREATE,
    IDARG_OUT_MONITORCREATE,
    IDARG_OUT_MONITORARRIVAL,
    IDDCX_MONITOR_INFO,
    IDDCX_MONITOR_MODE,
    IDDCX_MONITOR_MODE2,
    IDDCX_TARGET_MODE,
    IDDCX_TARGET_MODE2,
    IDDCX_PATH,
    IDDCX_PATH2,
    IDARG_IN_PARSEMONITORDESCRIPTION,
    IDARG_OUT_PARSEMONITORDESCRIPTION,
    IDARG_IN_QUERYTARGETMODES,
    IDARG_OUT_QUERYTARGETMODES,
    IDARG_IN_COMMITMODES,
    IDARG_IN_SETSWAPCHAIN,
    IDARG_IN_ADAPTERSETRENDERADAPTER,
    IDARG_IN_RELEASEANDACQUIREBUFFER2,
    IDARG_OUT_RELEASEANDACQUIREBUFFER2,
    IDDCX_METADATA2,
);

/// Every client-config callback type must exist and stay a nullable `extern fn`.
macro_rules! assert_pfn {
    ($($t:ident),+ $(,)?) => { $( const _: iddcx::$t = None; )+ };
}

assert_pfn!(
    PFN_IDD_CX_DEVICE_IO_CONTROL,
    PFN_IDD_CX_ADAPTER_INIT_FINISHED,
    PFN_IDD_CX_ADAPTER_COMMIT_MODES,
    PFN_IDD_CX_ADAPTER_COMMIT_MODES2,
    PFN_IDD_CX_PARSE_MONITOR_DESCRIPTION,
    PFN_IDD_CX_PARSE_MONITOR_DESCRIPTION2,
    PFN_IDD_CX_MONITOR_GET_DEFAULT_DESCRIPTION_MODES,
    PFN_IDD_CX_MONITOR_QUERY_TARGET_MODES,
    PFN_IDD_CX_MONITOR_QUERY_TARGET_MODES2,
    PFN_IDD_CX_MONITOR_ASSIGN_SWAPCHAIN,
    PFN_IDD_CX_MONITOR_UNASSIGN_SWAPCHAIN,
    PFN_IDD_CX_MONITOR_SET_GAMMA_RAMP,
    PFN_IDD_CX_MONITOR_SET_DEFAULT_HDR_METADATA,
    PFN_IDD_CX_ADAPTER_QUERY_TARGET_INFO,
);

// The FP16 adapter flag and the high-colour target cap gate the whole `*2` callback set.
const _: u32 = iddcx::IDDCX_ADAPTER_FLAGS::IDDCX_ADAPTER_FLAGS_CAN_PROCESS_FP16;
const _: u32 = iddcx::IDDCX_TARGET_CAPS::IDDCX_TARGET_CAPS_HIGH_COLOR_SPACE;
