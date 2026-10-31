//! Wi-Fi in media streaming mode while a stream runs (`wlan_intf_opcode_media_streaming_mode`):
//! the driver keeps its power saving and background work from bunching the stream's packets.
//! Windows turns the mode off when the interface disconnects or this client's handle closes, and
//! keeps it while any client asks for it.

use std::ffi::c_void;

use windows::core::{BOOL, GUID};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::NetworkManagement::WiFi::{
    wlan_interface_state_connected, wlan_intf_opcode_media_streaming_mode, WlanCloseHandle, WlanEnumInterfaces,
    WlanFreeMemory, WlanOpenHandle, WlanSetInterface, WLAN_INTERFACE_INFO, WLAN_INTERFACE_INFO_LIST,
};

/// The mode on every connected Wi-Fi interface until dropped.
pub struct StreamingMode {
    handle: HANDLE,
    interfaces: Vec<GUID>,
}

impl StreamingMode {
    /// Turns the mode on; None without a connected Wi-Fi interface, or with
    /// `NIGHTJAR_WLAN_STREAMING=0` (measurement only). Says what it did in the log.
    pub fn begin() -> Option<Self> {
        if std::env::var("NIGHTJAR_WLAN_STREAMING").is_ok_and(|v| v.trim() == "0") {
            eprintln!("nightjar: Wi-Fi streaming mode left off (NIGHTJAR_WLAN_STREAMING=0)");
            return None;
        }
        let mut handle = HANDLE::default();
        let mut version = 0u32;
        // SAFETY: out pointers to locals; the handle is closed in Drop or below.
        let opened = unsafe { WlanOpenHandle(2, None, &mut version, &mut handle) };
        if opened != 0 {
            // No WLAN service (a desktop without Wi-Fi): nothing to do.
            return None;
        }
        let mut list: *mut WLAN_INTERFACE_INFO_LIST = std::ptr::null_mut();
        let mut interfaces = Vec::new();
        // SAFETY: the list WlanEnumInterfaces returns is read within its item count and freed.
        unsafe {
            if WlanEnumInterfaces(handle, None, &mut list) == 0 && !list.is_null() {
                let count = (*list).dwNumberOfItems as usize;
                // The list is a header and `count` items; the field is declared with one.
                let first = std::ptr::addr_of!((*list).InterfaceInfo).cast::<WLAN_INTERFACE_INFO>();
                let items = std::slice::from_raw_parts(first, count);
                for item in items.iter().filter(|i| i.isState == wlan_interface_state_connected) {
                    let on = BOOL::from(true);
                    let set = WlanSetInterface(
                        handle,
                        &item.InterfaceGuid,
                        wlan_intf_opcode_media_streaming_mode,
                        std::mem::size_of::<BOOL>() as u32,
                        &on as *const BOOL as *const c_void,
                        None,
                    );
                    let name = String::from_utf16_lossy(&item.strInterfaceDescription);
                    let name = name.trim_end_matches('\0');
                    if set == 0 {
                        eprintln!("nightjar: Wi-Fi streaming mode on for {name}");
                        interfaces.push(item.InterfaceGuid);
                    } else {
                        eprintln!("nightjar: Wi-Fi streaming mode refused for {name} (error {set})");
                    }
                }
            }
            if !list.is_null() {
                WlanFreeMemory(list as *const c_void);
            }
            if interfaces.is_empty() {
                WlanCloseHandle(handle, None);
                return None;
            }
        }
        Some(Self { handle, interfaces })
    }
}

impl Drop for StreamingMode {
    fn drop(&mut self) {
        let off = BOOL::from(false);
        // SAFETY: the open handle and interface GUIDs from begin().
        unsafe {
            for guid in &self.interfaces {
                WlanSetInterface(
                    self.handle,
                    guid,
                    wlan_intf_opcode_media_streaming_mode,
                    std::mem::size_of::<BOOL>() as u32,
                    &off as *const BOOL as *const c_void,
                    None,
                );
            }
            WlanCloseHandle(self.handle, None);
        }
    }
}
