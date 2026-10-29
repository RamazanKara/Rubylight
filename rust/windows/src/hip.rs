//! What the AMD driver's compute runtime (HIP) reports about each GPU: its
//! name, its architecture (`gfx1100`) and its compute units. Neural
//! Rendering is offered by this, not by a list of product names.
//!
//! Loading the runtime starts driver threads and allocates for every GPU, so
//! the host never does it in its own process: `butterpollo.exe
//! --neural-probe` calls [`gpus`] and exits.
use anyhow::{Context, Result, bail};
use butterpollo_core::neural::Gpu;
use std::ffi::{CStr, c_int, c_void};

/// The runtime the driver installs in System32; older drivers had version 6.
const RUNTIMES: [&str; 2] = ["amdhip64_7.dll", "amdhip64_6.dll"];
/// `hipDeviceProp_tR0600`: `name[256]` first, `gcnArchName[256]` at 1160
/// (hip_runtime_api.h). The runtime writes about 1.5 KB; the buffer is larger.
const PROPERTIES: usize = 4096;
const ARCH_AT: usize = 1160;
/// `hipDeviceAttributeMultiprocessorCount`.
const MULTIPROCESSOR_COUNT: c_int = 63;

#[repr(C, align(8))]
struct Properties([u8; PROPERTIES]);

/// Every GPU the runtime finds. RDNA's runtime counts work-group processors,
/// two compute units each.
pub fn gpus() -> Result<Vec<Gpu>> {
    let library = RUNTIMES
        .iter()
        .find_map(|name| crate::telemetry::system_library(name).ok())
        .context("the AMD driver's compute runtime (amdhip64) is not installed")?;
    type Count = unsafe extern "C" fn(*mut c_int) -> c_int;
    type GetProperties = unsafe extern "C" fn(*mut c_void, c_int) -> c_int;
    type Attribute = unsafe extern "C" fn(*mut c_int, c_int, c_int) -> c_int;
    // SAFETY: These exports and their C signatures are HIP's (hip_runtime_api.h); the symbols
    // are only used while `library` is loaded.
    let (count, properties, attribute) = unsafe {
        (
            *library.get::<Count>(b"hipGetDeviceCount\0")?,
            *library.get::<GetProperties>(b"hipGetDevicePropertiesR0600\0")?,
            *library.get::<Attribute>(b"hipDeviceGetAttribute\0")?,
        )
    };
    let mut devices = 0;
    // SAFETY: `devices` is a live, writable int.
    let status = unsafe { count(&mut devices) };
    if status != 0 {
        bail!("the AMD compute runtime finds no usable GPU (HIP error {status})");
    }
    let mut gpus = Vec::new();
    for device in 0..devices {
        let mut buffer = Box::new(Properties([0; PROPERTIES]));
        // SAFETY: The buffer is zeroed, 8-aligned and larger than hipDeviceProp_tR0600.
        let status = unsafe { properties(buffer.0.as_mut_ptr().cast(), device) };
        if status != 0 {
            bail!("the AMD compute runtime cannot describe GPU {device} (HIP error {status})");
        }
        let mut units = 0;
        // SAFETY: `units` is a live, writable int and `device` is below the count.
        let status = unsafe { attribute(&mut units, MULTIPROCESSOR_COUNT, device) };
        if status != 0 {
            bail!(
                "the AMD compute runtime cannot count GPU {device}'s compute units (HIP error {status})"
            );
        }
        let text = |at: usize| {
            CStr::from_bytes_until_nul(&buffer.0[at..at + 256])
                .map(|text| text.to_string_lossy().into_owned())
                .unwrap_or_default()
        };
        // "gfx1100" or "gfx1100:xnack-": the target features are not the architecture.
        let arch = text(ARCH_AT)
            .split(':')
            .next()
            .unwrap_or_default()
            .to_owned();
        if !arch.starts_with("gfx") {
            bail!("the AMD compute runtime reports no architecture for GPU {device}");
        }
        let units = units.max(0) as u32;
        gpus.push(Gpu {
            name: text(0),
            compute_units: if arch.starts_with("gfx1") && arch.len() == 7 {
                units * 2
            } else {
                units
            },
            arch,
        });
    }
    Ok(gpus)
}
