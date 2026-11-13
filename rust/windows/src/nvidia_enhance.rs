//! The NVIDIA build's NGX adapter (`butterpollo_truehdr.dll`, built from
//! `rust/truehdr-runtime` with NVIDIA's RTX Video SDK) and the startup check
//! of what the GPU and driver offer. Only built with the `nvidia-proprietary`
//! feature; the signed builds carry none of it.
use anyhow::{Context, Result, bail};
use butterpollo_core::nvidia_enhance::{RTX_HDR, RTX_VIDEO};
use std::{ffi::c_void, path::PathBuf};
use windows::{
    Win32::{
        Foundation::HMODULE,
        Graphics::{
            Direct3D::D3D_DRIVER_TYPE_UNKNOWN,
            Direct3D11::*,
            Dxgi::{CreateDXGIFactory1, IDXGIFactory1},
        },
    },
    core::Interface,
};

/// The adapter's file, beside the host.
pub const ADAPTER: &str = "butterpollo_truehdr.dll";
/// The adapter ABI this host speaks.
const ABI: u32 = 2;
/// NVIDIA's PCI vendor id.
const NVIDIA: u32 = 0x10de;

/// Where the adapter is: the executable's folder.
fn adapter_path() -> Result<PathBuf> {
    Ok(std::env::current_exe()?
        .parent()
        .context("executable directory unavailable")?
        .join(ADAPTER))
}

/// Whether the adapter ships with this host.
pub fn installed() -> bool {
    adapter_path().is_ok_and(|path| path.exists())
}

/// Loads the adapter and checks its ABI.
pub(crate) fn load() -> Result<libloading::Library> {
    let path = adapter_path()?;
    // SAFETY: The DLL beside the executable is Rubylight's own adapter; its ABI function takes
    // nothing and returns a u32, and is checked before any other symbol is used.
    unsafe {
        let library = libloading::Library::new(&path)
            .with_context(|| format!("the NVIDIA adapter {} is unavailable", path.display()))?;
        let abi = library.get::<unsafe extern "C" fn() -> u32>(b"butterpollo_truehdr_abi\0")?;
        let found = abi();
        if found != ABI {
            bail!(
                "the NVIDIA adapter speaks ABI {found}, this host {ABI}; reinstall the NVIDIA build"
            );
        }
        Ok(library)
    }
}

/// An NGX error code in words.
pub(crate) fn describe(code: u32) -> String {
    match code {
        // The adapter's own codes.
        0xbad1_0001 => "NGX returned no parameters".into(),
        0xbad1_0002 => "the picture's format or size is not supported".into(),
        0xbad1_0003 => "the GPU or driver does not offer it".into(),
        0xbad1_0004 => "NGX could not create the feature".into(),
        // NVSDK_NGX_Result_FAIL_* from the RTX Video SDK's nvsdk_ngx_defs.h.
        0xbad0_0001 => "the GPU does not support it".into(),
        0xbad0_0002 => "NGX reported a platform error; update the NVIDIA driver".into(),
        0xbad0_0004 => "its NVIDIA runtime DLL is missing beside the host".into(),
        0xbad0_0007 => "NGX is not initialized".into(),
        0xbad0_0008 => "the picture's format is not supported".into(),
        0xbad0_000b => "NGX could not initialize the feature".into(),
        0xbad0_000c => "the NVIDIA driver is too old".into(),
        0xbad0_000d => "the GPU is out of memory".into(),
        code => format!("NGX error {code:#x}"),
    }
}

/// What the first NVIDIA GPU and its driver offer (`nvidia_enhance` bits),
/// and why each missing enhancement is missing. Takes a few hundred
/// milliseconds: run it once, off any stream.
pub fn probe() -> (u8, Vec<(&'static str, String)>) {
    let mut errors = Vec::new();
    let result = (|| -> Result<u8> {
        let library = load()?;
        let device = nvidia_device()?;
        // SAFETY: ABI 2's availability check takes a live D3D11 device, a feature number and a
        // writable error code; `library` stays loaded for the call.
        let available = unsafe {
            *library.get::<unsafe extern "C" fn(*mut c_void, u32, *mut u32) -> u32>(
                b"butterpollo_ngx_available\0",
            )?
        };
        let mut offered = 0;
        for (bit, feature, name) in [(RTX_HDR, 0, "rtx_hdr"), (RTX_VIDEO, 1, "rtx_video")] {
            let mut error = 0;
            // SAFETY: as above; `device` is live for the call.
            if unsafe { available(device.as_raw(), feature, &mut error) } == 1 {
                offered |= bit;
            } else {
                errors.push((name, describe(error)));
            }
        }
        Ok(offered)
    })();
    match result {
        Ok(offered) => (offered, errors),
        Err(error) => (
            0,
            vec![
                ("rtx_hdr", format!("{error:#}")),
                ("rtx_video", format!("{error:#}")),
            ],
        ),
    }
}

/// A D3D11 device on the first NVIDIA adapter.
fn nvidia_device() -> Result<ID3D11Device> {
    // SAFETY: The factory and adapters are live COM objects returned by these calls; the
    // out-parameter is a live local, unwrapped only after D3D11CreateDevice succeeded.
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
        for index in 0..32 {
            let Ok(adapter) = factory.EnumAdapters1(index) else {
                break;
            };
            if adapter.GetDesc1()?.VendorId != NVIDIA {
                continue;
            }
            let mut device = None;
            D3D11CreateDevice(
                &adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                None,
            )?;
            return device.context("D3D11 returned no device");
        }
        bail!("this PC has no NVIDIA GPU")
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn ngx_errors_read_as_words() {
        assert!(super::describe(0xbad0_0004).contains("DLL"));
        assert!(super::describe(0xbad0_000c).contains("driver"));
        assert!(super::describe(0xbad1_0003).contains("does not offer"));
        assert_eq!(super::describe(0x1234), "NGX error 0x1234");
    }
}
