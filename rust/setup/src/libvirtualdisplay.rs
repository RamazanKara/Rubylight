//! Removes the libvirtualdisplay (Sunshine/Vibepollo) virtual display driver
//! that Rubylight installed before it moved to its own driver: the
//! `Root\SunshineVirtualDisplay` device, its driver packages and its Vulkan
//! HDR layer registration. Setup runs it on an install or update once
//! Rubylight's own device runs, and the uninstall with the drivers; with
//! nothing left it changes nothing. Elevated.
//!
//! The SignPath Foundation certificate the old driver added to
//! TrustedPublisher stays: the virtual gamepad driver is signed with it too.
//!
//! The logic works on [`Windows`], which tests fake; [`Native`] is Windows.
use crate::{log::line, system};
use anyhow::{Context, Result, bail};
use std::time::Duration;

const HARDWARE_ID: &str = r"Root\SunshineVirtualDisplay";
/// The catalog its INF names, which the driver store's copy keeps.
const CATALOG: &str = "SunshineVirtualDisplayDriver.cat";
const HDR_LAYER: &str = "VkLayer_sunshine_hdr.json";
const LAYERS: &str = r"SOFTWARE\Khronos\Vulkan\ImplicitLayers";
const PNPUTIL_TIMEOUT: Duration = Duration::from_secs(600);
/// pnputil's exit code for a change that completes when Windows restarts (a
/// display still in use).
const RESTART: i32 = 3010;

/// A registry view of HKLM.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Native64,
    Wow32,
}

/// What [`remove`] asks Windows.
pub trait Windows {
    /// The instance ids of every device, present or not, that lists
    /// `hardware_id` (in any case) among its hardware ids.
    fn devices(&mut self, hardware_id: &str) -> Result<Vec<String>>;
    /// Every `oem*.inf` of `%SystemRoot%\INF`, by file name, with its text.
    fn oem_infs(&mut self) -> Result<Vec<(String, String)>>;
    /// pnputil's exit code.
    fn pnputil(&mut self, args: &[&str]) -> Result<i32>;
    /// The value names of the implicit Vulkan layers in `view`; `None` when
    /// the key isn't there.
    fn layers(&mut self, view: View) -> Result<Option<Vec<String>>>;
    fn delete_layer(&mut self, view: View, name: &str) -> Result<()>;
}

/// What [`remove`] did.
#[derive(Debug, Default)]
pub struct Removal {
    /// Windows finishes the removal when it restarts.
    pub restart: bool,
    /// The step that failed; the ones after it did not run.
    pub error: Option<anyhow::Error>,
}

/// Removes the device, then the driver packages, then the HDR layer.
pub fn remove(windows: &mut impl Windows) -> Removal {
    let mut restart = false;
    let error = steps(windows, &mut restart).err();
    Removal { restart, error }
}

fn steps(windows: &mut impl Windows, restart: &mut bool) -> Result<()> {
    for device in windows.devices(HARDWARE_ID)? {
        line(format!("Removing the libvirtualdisplay device {device}."));
        pnputil(windows, &["/remove-device", &device], restart)?;
    }
    for inf in packages(&windows.oem_infs()?) {
        line(format!(
            "Removing the libvirtualdisplay driver package {inf}."
        ));
        pnputil(
            windows,
            &["/delete-driver", &inf, "/uninstall", "/force"],
            restart,
        )?;
    }
    // Both registry views; Rubylight registers its own layer.
    for view in [View::Native64, View::Wow32] {
        let Some(names) = windows.layers(view)? else {
            continue;
        };
        for name in names
            .iter()
            .filter(|name| file_name(name).eq_ignore_ascii_case(HDR_LAYER))
        {
            windows
                .delete_layer(view, name)
                .with_context(|| format!("removing the Vulkan layer {name}"))?;
        }
    }
    Ok(())
}

fn pnputil(windows: &mut impl Windows, args: &[&str], restart: &mut bool) -> Result<()> {
    let code = windows.pnputil(args)?;
    if code == RESTART {
        *restart = true;
    } else if code != 0 {
        bail!("pnputil {} failed with exit code {code}", args.join(" "));
    }
    Ok(())
}

/// The driver store's names of libvirtualdisplay's packages. pnputil's own
/// listing is localized; each INF names its original catalog.
fn packages(infs: &[(String, String)]) -> Vec<String> {
    infs.iter()
        .filter(|(_, text)| names_catalog(text))
        .map(|(name, _)| name.clone())
        .collect()
}

/// Whether a line of `text` reads `CatalogFile = SunshineVirtualDisplayDriver.cat`,
/// with any spacing and case, and anything after it.
fn names_catalog(text: &str) -> bool {
    text.lines().any(|line| {
        line.split_once('=').is_some_and(|(key, value)| {
            key.trim().eq_ignore_ascii_case("CatalogFile")
                && value
                    .trim_start()
                    .get(..CATALOG.len())
                    .is_some_and(|value| value.eq_ignore_ascii_case(CATALOG))
        })
    })
}

/// An INF's text: UTF-16 with its byte order mark, else UTF-8 or ANSI, where
/// the ASCII keys read the same.
fn inf_text(bytes: &[u8]) -> String {
    let utf16 = |bytes: &[u8], read: fn([u8; 2]) -> u16| {
        let units: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| read(*pair))
            .collect();
        String::from_utf16_lossy(&units)
    };
    match bytes {
        [0xFF, 0xFE, rest @ ..] => utf16(rest, u16::from_le_bytes),
        [0xFE, 0xFF, rest @ ..] => utf16(rest, u16::from_be_bytes),
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

/// The last component of a layer's path, as `Path.GetFileName` reads it.
fn file_name(path: &str) -> &str {
    path.rsplit(['\\', '/', ':']).next().unwrap_or(path)
}

/// Windows: SetupAPI for the devices, pnputil for the removals, the registry
/// for the layer.
pub struct Native;

impl Windows for Native {
    fn devices(&mut self, hardware_id: &str) -> Result<Vec<String>> {
        Ok(all_devices()?
            .into_iter()
            .filter(|(_, ids)| ids.iter().any(|id| id.eq_ignore_ascii_case(hardware_id)))
            .map(|(instance, _)| instance)
            .collect())
    }
    fn oem_infs(&mut self) -> Result<Vec<(String, String)>> {
        let root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
        let folder = std::path::Path::new(&root).join("INF");
        let mut infs = Vec::new();
        for entry in
            std::fs::read_dir(&folder).with_context(|| format!("listing {}", folder.display()))?
        {
            let entry = entry.with_context(|| format!("listing {}", folder.display()))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let lower = name.to_ascii_lowercase();
            if !(lower.starts_with("oem") && lower.ends_with(".inf")) || !entry.path().is_file() {
                continue;
            }
            let bytes = std::fs::read(entry.path())
                .with_context(|| format!("reading {}", entry.path().display()))?;
            infs.push((name, inf_text(&bytes)));
        }
        Ok(infs)
    }
    fn pnputil(&mut self, args: &[&str]) -> Result<i32> {
        Ok(system::run(&system::system32("pnputil.exe"), args, PNPUTIL_TIMEOUT)?.0)
    }
    fn layers(&mut self, view: View) -> Result<Option<Vec<String>>> {
        use windows::Win32::{
            Foundation::{ERROR_NO_MORE_ITEMS, ERROR_SUCCESS},
            System::Registry::{KEY_QUERY_VALUE, RegEnumValueW},
        };
        let Some(key) = layers_key(view, KEY_QUERY_VALUE)? else {
            return Ok(None);
        };
        let mut names = Vec::new();
        for index in 0.. {
            // A value name is at most 16383 characters.
            let mut name = vec![0u16; 16384];
            let mut length = name.len() as u32;
            // SAFETY: `key` is open and `length` is the capacity of `name` in characters.
            let status = unsafe {
                RegEnumValueW(
                    key.0,
                    index,
                    Some(windows::core::PWSTR(name.as_mut_ptr())),
                    &mut length,
                    None,
                    None,
                    None,
                    None,
                )
            };
            if status == ERROR_NO_MORE_ITEMS {
                break;
            }
            if status != ERROR_SUCCESS {
                return Err(windows::core::Error::from(status.to_hresult()))
                    .context("listing the Vulkan layers");
            }
            names.push(String::from_utf16_lossy(&name[..length as usize]));
        }
        Ok(Some(names))
    }
    fn delete_layer(&mut self, view: View, name: &str) -> Result<()> {
        use windows::Win32::System::Registry::{KEY_SET_VALUE, RegDeleteValueW};
        let key = layers_key(view, KEY_SET_VALUE)?.context("the Vulkan layer key is gone")?;
        let name = system::wide(name);
        // SAFETY: `key` is open and `name` is NUL-terminated UTF-16.
        unsafe { RegDeleteValueW(key.0, windows::core::PCWSTR(name.as_ptr())) }
            .ok()
            .map_err(Into::into)
    }
}

/// An open registry key, closed when dropped.
struct Key(windows::Win32::System::Registry::HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: The key was opened here and is closed only here.
        let _ = unsafe { windows::Win32::System::Registry::RegCloseKey(self.0) };
    }
}

/// The implicit layer key in `view`, `None` when it isn't there.
fn layers_key(
    view: View,
    access: windows::Win32::System::Registry::REG_SAM_FLAGS,
) -> Result<Option<Key>> {
    use windows::Win32::{
        Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS},
        System::Registry::{
            HKEY, HKEY_LOCAL_MACHINE, KEY_WOW64_32KEY, KEY_WOW64_64KEY, RegOpenKeyExW,
        },
    };
    let view = match view {
        View::Native64 => KEY_WOW64_64KEY,
        View::Wow32 => KEY_WOW64_32KEY,
    };
    let path = system::wide(LAYERS);
    let mut key = HKEY::default();
    // SAFETY: `path` is NUL-terminated UTF-16 and `key` is a live local the call fills.
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            windows::core::PCWSTR(path.as_ptr()),
            None,
            access | view,
            &mut key,
        )
    };
    match status {
        ERROR_SUCCESS => Ok(Some(Key(key))),
        ERROR_FILE_NOT_FOUND => Ok(None),
        status => Err(windows::core::Error::from(status.to_hresult()))
            .with_context(|| format!("opening HKLM\\{LAYERS}")),
    }
}

/// The devices that could be read. One that can't is logged and skipped, as
/// the script this replaced went on past it: it can't be ours to remove.
fn readable(
    devices: impl IntoIterator<Item = Result<(String, Vec<String>)>>,
) -> Vec<(String, Vec<String>)> {
    devices
        .into_iter()
        .filter_map(|device| {
            device
                .map_err(|error| line(format!("warning: a device was skipped: {error:#}")))
                .ok()
        })
        .collect()
}

/// Every device, present or not, by instance id with its hardware ids. Only
/// listing them fails; a device that can't be read is skipped.
fn all_devices() -> Result<Vec<(String, Vec<String>)>> {
    use windows::Win32::Devices::DeviceAndDriverInstallation::{
        DIGCF_ALLCLASSES, HDEVINFO, SP_DEVINFO_DATA, SetupDiDestroyDeviceInfoList,
        SetupDiEnumDeviceInfo, SetupDiGetClassDevsW,
    };
    use windows::Win32::Foundation::ERROR_NO_MORE_ITEMS;
    struct Set(HDEVINFO);
    impl Drop for Set {
        fn drop(&mut self) {
            // SAFETY: This value owns the set and destroys it once.
            let _ = unsafe { SetupDiDestroyDeviceInfoList(self.0) };
        }
    }
    // SAFETY: No class, enumerator or window: every device of every class.
    let set = Set(unsafe {
        SetupDiGetClassDevsW(None, windows::core::PCWSTR::null(), None, DIGCF_ALLCLASSES)
    }
    .context("listing the devices")?);
    let mut devices = Vec::new();
    for index in 0.. {
        let mut device = SP_DEVINFO_DATA {
            cbSize: size_of::<SP_DEVINFO_DATA>() as u32,
            ..Default::default()
        };
        // SAFETY: A correctly sized output structure and a live device set.
        match unsafe { SetupDiEnumDeviceInfo(set.0, index, &mut device) } {
            Ok(()) => {}
            Err(error) if error.code() == ERROR_NO_MORE_ITEMS.to_hresult() => break,
            Err(error) => return Err(error).context("listing the devices"),
        }
        devices.push(device_ids(set.0, &device).with_context(|| format!("device {index}")));
    }
    Ok(readable(devices))
}

/// The instance id and hardware ids of `device` in the live set `set`.
fn device_ids(
    set: windows::Win32::Devices::DeviceAndDriverInstallation::HDEVINFO,
    device: &windows::Win32::Devices::DeviceAndDriverInstallation::SP_DEVINFO_DATA,
) -> Result<(String, Vec<String>)> {
    use windows::Win32::{
        Devices::DeviceAndDriverInstallation::{
            SPDRP_HARDWAREID, SetupDiGetDeviceInstanceIdW, SetupDiGetDeviceRegistryPropertyW,
        },
        Foundation::{ERROR_INSUFFICIENT_BUFFER, ERROR_INVALID_DATA},
    };
    let mut size = 0u32;
    // SAFETY: The device belongs to the live set; this call only asks the size.
    match unsafe { SetupDiGetDeviceInstanceIdW(set, device, None, Some(&mut size)) } {
        Err(error) if error.code() == ERROR_INSUFFICIENT_BUFFER.to_hresult() => {}
        Err(error) => return Err(error).context("asking the size of its instance id"),
        Ok(()) => bail!("it has no instance id"),
    }
    let mut instance = vec![0u16; size as usize];
    // SAFETY: The device belongs to the live set and `instance` has the size asked.
    unsafe { SetupDiGetDeviceInstanceIdW(set, device, Some(&mut instance), None) }
        .context("reading its instance id")?;
    let instance = String::from_utf16_lossy(&instance)
        .trim_end_matches('\0')
        .to_owned();
    let mut size = 0u32;
    // SAFETY: As above; this call only asks the size.
    let asked = unsafe {
        SetupDiGetDeviceRegistryPropertyW(
            set,
            device,
            SPDRP_HARDWAREID,
            None,
            None,
            Some(&mut size),
        )
    };
    match asked {
        Err(error) if error.code() == ERROR_INSUFFICIENT_BUFFER.to_hresult() => {}
        // A device without hardware ids lists none.
        Err(error) if error.code() == ERROR_INVALID_DATA.to_hresult() => {
            return Ok((instance, Vec::new()));
        }
        Err(error) => {
            return Err(error).with_context(|| format!("reading the hardware ids of {instance}"));
        }
        Ok(()) => {}
    }
    let mut bytes = vec![0u8; size as usize];
    // SAFETY: As above, with a buffer of the size asked.
    unsafe {
        SetupDiGetDeviceRegistryPropertyW(
            set,
            device,
            SPDRP_HARDWAREID,
            None,
            Some(&mut bytes),
            None,
        )
    }
    .with_context(|| format!("reading the hardware ids of {instance}"))?;
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .collect();
    let ids = String::from_utf16_lossy(&units)
        .split('\0')
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .collect();
    Ok((instance, ids))
}

#[cfg(test)]
#[path = "libvirtualdisplay_tests.rs"]
mod tests;
