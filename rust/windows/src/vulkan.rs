//! Per-user Vulkan layer registration and reference-counted HDR activation.

use crate::text::to_wide;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};
use windows::{
    Win32::{
        Foundation::*,
        Security::{
            Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW,
            PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES,
        },
        System::{Registry::*, Threading::*},
    },
    core::PCWSTR,
};
const KEY: &str = "Software\\Khronos\\Vulkan\\ImplicitLayers";
const MANIFEST: &str = "VkLayer_butterpollo_hdr.json";
fn manifest_in(directory: &Path) -> PathBuf {
    // The Windows Vulkan loader resolves library_path using backslashes in the
    // registry value name. A mixed "vulkan-layer/VkLayer..." path makes it look
    // for the DLL one directory above the manifest, even though Windows can
    // open the manifest itself with either separator.
    directory.join("vulkan-layer").join(MANIFEST)
}
fn legacy_manifest(path: &Path) -> Option<PathBuf> {
    let legacy = path
        .parent()
        .and_then(Path::parent)
        .expect("layer manifest has an installation directory")
        .join(format!("vulkan-layer/{MANIFEST}"));
    // PathBuf::join already normalized the old suffix for verbatim paths.
    // Never remove that registration: it is the replacement itself.
    (legacy.as_os_str() != path.as_os_str()).then_some(legacy)
}
pub fn manifest() -> Result<PathBuf> {
    Ok(manifest_in(
        std::env::current_exe()?
            .parent()
            .context("executable directory missing")?,
    ))
}
fn registration(root: HKEY, subkey: &str, path: &Path) -> Option<u32> {
    let name = to_wide(&path.to_string_lossy());
    let key = to_wide(subkey);
    let mut data = 0u32;
    let mut size = size_of::<u32>() as u32;
    // SAFETY: `key` and `name` are NUL-terminated and outlive the calls, and `data` is the 4-byte
    // buffer `size` names. A fresh size is used for each query.
    unsafe {
        RegGetValueW(
            root,
            PCWSTR(key.as_ptr()),
            PCWSTR(name.as_ptr()),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut data as *mut u32).cast()),
            Some(&mut size),
        )
        .is_ok()
        .then_some(data)
    }
}
pub fn installed(path: &Path) -> bool {
    [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE]
        .into_iter()
        .any(|root| registration(root, KEY, path) == Some(0))
}
pub fn register(enabled: bool) -> Result<()> {
    let path = manifest()?;
    if enabled
        && (!path.is_file()
            || !path
                .with_file_name("butterpollo_vulkan_layer.dll")
                .is_file())
    {
        bail!("the Rust Vulkan HDR layer is missing from this installation");
    }
    let (root, other) = if crate::process::is_system() {
        (HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER)
    } else {
        (HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE)
    };
    write_registration(root, KEY, &path, enabled)?;
    // Repair an older registration of this same installation in the other
    // scope when accessible. Leave other installations and layers alone.
    if let Some(legacy) = legacy_manifest(&path)
        && registration(other, KEY, &legacy).is_some()
        && let Err(error) = write_registration(other, KEY, &path, enabled)
    {
        tracing::warn!(%error, "could not repair the older Vulkan HDR registration in the other registry scope");
    }
    Ok(())
}
fn write_registration(root: HKEY, subkey: &str, path: &Path, enabled: bool) -> Result<()> {
    let key = to_wide(subkey);
    let name = to_wide(&path.to_string_lossy());
    let legacy = legacy_manifest(path).map(|path| to_wide(&path.to_string_lossy()));
    // SAFETY: `key`, `name` and `value` outlive the calls, and `handle` is closed once, only after
    // it was opened successfully.
    unsafe {
        let mut handle = HKEY::default();
        RegCreateKeyExW(
            root,
            PCWSTR(key.as_ptr()),
            None,
            None,
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut handle,
            None,
        )
        .ok()?;
        let value = u32::from(!enabled).to_le_bytes();
        let result = RegSetValueExW(handle, PCWSTR(name.as_ptr()), None, REG_DWORD, Some(&value));
        // Commit the replacement before removing the broken value. Keeping
        // both enabled lets the loader select the broken duplicate first.
        let result = result.ok().and_then(|()| {
            let Some(legacy) = &legacy else {
                return Ok(());
            };
            let removed = RegDeleteValueW(handle, PCWSTR(legacy.as_ptr()));
            if removed == ERROR_FILE_NOT_FOUND {
                Ok(())
            } else {
                removed.ok()
            }
        });
        let _ = RegCloseKey(handle);
        result?;
    }
    Ok(())
}
pub fn reconcile(enabled: bool) -> Result<()> {
    let path = manifest()?;
    if enabled
        || installed(&path)
        || legacy_manifest(&path).is_some_and(|legacy| installed(&legacy))
    {
        register(enabled)?;
    }
    Ok(())
}
pub fn status(enabled: bool) -> Value {
    let path = manifest().unwrap_or_default();
    json!({"status":true,"installed":installed(&path),"enabled":enabled,
        "available":path.is_file() && path.with_file_name("butterpollo_vulkan_layer.dll").is_file(),
        "manifest":path,"active":EVENTS.get().is_some_and(|e| e.lock().is_ok_and(|e| e.users > 0))})
}
struct Events {
    users: usize,
    handles: Vec<HANDLE>,
}
// Named kernel events are thread safe; ownership and refcounts use the mutex.
// SAFETY: Events owns its handles, closes them only in Drop, and is reached only through the EVENTS
// mutex; kernel event handles may be used from any thread.
unsafe impl Send for Events {}
impl Drop for Events {
    fn drop(&mut self) {
        for handle in &self.handles {
            // SAFETY: Each handle came from CreateEventW, is owned by Events, and is closed once,
            // here.
            unsafe {
                let _ = CloseHandle(*handle);
            }
        }
    }
}
static EVENTS: OnceLock<Mutex<Events>> = OnceLock::new();
pub struct Lease;
impl Lease {
    pub fn acquire() -> Result<Self> {
        let events = EVENTS.get_or_init(|| {
            Mutex::new(Events {
                users: 0,
                handles: vec![],
            })
        });
        let mut events = events.lock().unwrap();
        if events.handles.is_empty() {
            let descriptor_text =
                to_wide("D:P(A;;GA;;;OW)(A;;GA;;;SY)(A;;GA;;;BA)(A;;0x00100000;;;WD)");
            let mut descriptor = PSECURITY_DESCRIPTOR::default();
            // SAFETY: `descriptor_text` is NUL-terminated and outlives the call; `descriptor`
            // receives a LocalAlloc'd descriptor that is freed below.
            unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    PCWSTR(descriptor_text.as_ptr()),
                    1,
                    &mut descriptor,
                    None,
                )?;
            }
            let security = SECURITY_ATTRIBUTES {
                nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: descriptor.0,
                bInheritHandle: false.into(),
            };
            for name in [
                "Global\\ButterpolloRustVirtualHdrActive",
                "Local\\ButterpolloRustVirtualHdrActive",
            ] {
                let name = to_wide(name);
                // SAFETY: `security` points at the live descriptor and `name` is NUL-terminated;
                // both outlive the call.
                let created =
                    unsafe { CreateEventW(Some(&security), true, false, PCWSTR(name.as_ptr())) };
                if let Ok(handle) = created {
                    events.handles.push(handle);
                }
            }
            // SAFETY: `descriptor` was allocated by the conversion above and is no longer used.
            unsafe {
                let _ = LocalFree(Some(HLOCAL(descriptor.0)));
            }
            if events.handles.is_empty() {
                bail!("cannot create the Vulkan HDR activation event");
            }
        }
        if events.users == 0 {
            for handle in &events.handles {
                // SAFETY: The handle is owned by Events, which the held lock keeps alive and open.
                unsafe {
                    SetEvent(*handle)?;
                }
            }
        }
        events.users += 1;
        Ok(Self)
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        if let Some(events) = EVENTS.get() {
            let mut events = events.lock().unwrap();
            events.users -= 1;
            if events.users == 0 {
                for handle in &events.handles {
                    // SAFETY: The handle is owned by Events, which the held lock keeps alive and
                    // open.
                    unsafe {
                        let _ = ResetEvent(*handle);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct RegistryFixture(String);
    impl RegistryFixture {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            Self(format!(
                "Software\\RubylightTests\\Vulkan-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ))
        }
        fn set(&self, path: &Path, value: u32) {
            let key = to_wide(&self.0);
            let name = to_wide(&path.to_string_lossy());
            // SAFETY: all strings and the four-byte value outlive the calls;
            // the opened key is private to this test and is closed below.
            unsafe {
                let mut handle = HKEY::default();
                RegCreateKeyExW(
                    HKEY_CURRENT_USER,
                    PCWSTR(key.as_ptr()),
                    None,
                    None,
                    REG_OPTION_NON_VOLATILE,
                    KEY_SET_VALUE,
                    None,
                    &mut handle,
                    None,
                )
                .ok()
                .unwrap();
                let result = RegSetValueExW(
                    handle,
                    PCWSTR(name.as_ptr()),
                    None,
                    REG_DWORD,
                    Some(&value.to_le_bytes()),
                );
                RegCloseKey(handle).ok().unwrap();
                result.ok().unwrap();
            }
        }
        fn get(&self, path: &Path) -> Option<u32> {
            registration(HKEY_CURRENT_USER, &self.0, path)
        }
        fn write(&self, path: &Path, enabled: bool) {
            write_registration(HKEY_CURRENT_USER, &self.0, path, enabled).unwrap();
        }
    }
    impl Drop for RegistryFixture {
        fn drop(&mut self) {
            let key = to_wide(&self.0);
            // SAFETY: this deletes only the fixture's private test key.
            unsafe {
                let _ = RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(key.as_ptr()));
            }
        }
    }

    #[test]
    fn manifest_uses_windows_separators_for_loader_relative_dll_resolution() {
        for directory in [
            r"C:\Program Files\Rubylight",
            r"\\?\C:\Games\Rubylight",
            r"\\server\share\Rubylight",
        ] {
            let path = manifest_in(Path::new(directory));
            assert_eq!(
                path.to_str().unwrap(),
                format!(r"{directory}\vulkan-layer\VkLayer_butterpollo_hdr.json")
            );
            if directory.starts_with(r"\\?\") {
                assert!(legacy_manifest(&path).is_none());
                continue;
            }
            let legacy = legacy_manifest(&path).unwrap();
            assert_eq!(
                legacy.to_str().unwrap(),
                format!(r"{directory}\vulkan-layer/VkLayer_butterpollo_hdr.json")
            );
            // Windows Path equality treats both separators alike; registry
            // value names and Vulkan's relative DLL resolver do not.
            assert_ne!(path.as_os_str(), legacy.as_os_str());
        }
    }

    #[test]
    fn registration_replaces_the_broken_duplicate_and_preserves_other_layers() {
        let registry = RegistryFixture::new();
        let path = manifest_in(Path::new(r"C:\Program Files\Rubylight"));
        let legacy = legacy_manifest(&path).unwrap();
        let other =
            legacy_manifest(&manifest_in(Path::new(r"C:\Program Files\Rubylight2"))).unwrap();
        registry.set(&legacy, 0);
        registry.set(&other, 0);
        registry.set(
            Path::new(r"C:\games\Steam\SteamOverlayVulkanLayer64.json"),
            0,
        );
        registry.write(&path, true);
        assert_eq!(registry.get(&path), Some(0));
        assert_eq!(registry.get(&legacy), None);
        assert_eq!(registry.get(&other), Some(0));
        assert_eq!(
            registry.get(Path::new(r"C:\games\Steam\SteamOverlayVulkanLayer64.json")),
            Some(0)
        );
        // Startup reconciliation and the maintenance action are repeatable.
        registry.write(&path, true);
        assert_eq!(registry.get(&path), Some(0));
    }

    #[test]
    fn disabling_an_older_registration_does_not_leave_it_enabled() {
        let registry = RegistryFixture::new();
        let path = manifest_in(Path::new(r"C:\Rubylight"));
        let legacy = legacy_manifest(&path).unwrap();
        registry.set(&legacy, 0);
        registry.write(&path, false);
        assert_eq!(registry.get(&path), Some(1));
        assert_eq!(registry.get(&legacy), None);
        registry.write(&path, false);
        assert_eq!(registry.get(&path), Some(1));
        registry.write(&path, true);
        assert_eq!(registry.get(&path), Some(0));
    }

    #[test]
    fn verbatim_registration_is_not_deleted_as_its_own_legacy_path() {
        let registry = RegistryFixture::new();
        let path = manifest_in(Path::new(r"\\?\C:\Program Files\Rubylight"));
        registry.write(&path, true);
        assert_eq!(registry.get(&path), Some(0));
        registry.write(&path, false);
        assert_eq!(registry.get(&path), Some(1));
    }
}
