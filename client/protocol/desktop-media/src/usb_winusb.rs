//! Which Windows driver holds each USB device: libusb reaches a device only through WinUSB, so
//! the picker names every other driver instead of offering a device that would fail to open.
//! Read from Windows' device list (SetupAPI), without opening any device.

/// One USB device node Windows lists: a device, or one interface of a composite device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    pub vid: u16,
    pub pid: u16,
    /// The interface (`MI_xx`) of a composite device's child node; None for the device itself.
    pub interface: Option<u8>,
    /// The hub ports from the controller down, as libusb's port numbers.
    pub ports: Vec<u8>,
    /// The driver service, empty when no driver is installed.
    pub service: String,
    /// The product string the device reported, empty when Windows has none.
    pub description: String,
    /// Windows' device instance id (`USB\VID_054C&PID_0CE6\...`), as usbipd-win names it too.
    pub instance_id: String,
}

/// `USB\VID_1209&PID_0001&MI_02\...`: the vendor, product and interface.
pub fn parse_instance_id(id: &str) -> Option<(u16, u16, Option<u8>)> {
    let mut parts = id.split('\\');
    if !parts.next()?.eq_ignore_ascii_case("USB") { return None; }
    let (mut vid, mut pid, mut interface) = (None, None, None);
    for field in parts.next()?.split('&') {
        let upper = field.to_ascii_uppercase();
        if let Some(hex) = upper.strip_prefix("VID_") { vid = u16::from_str_radix(hex, 16).ok(); }
        else if let Some(hex) = upper.strip_prefix("PID_") { pid = u16::from_str_radix(hex, 16).ok(); }
        else if let Some(hex) = upper.strip_prefix("MI_") { interface = Some(u8::from_str_radix(hex, 16).ok()?); }
    }
    Some((vid?, pid?, interface))
}

/// `PCIROOT(0)#PCI(1400)#USBROOT(0)#USB(2)#USB(1)#USBMI(0)`: the ports after the root hub.
pub fn parse_location_path(path: &str) -> Option<Vec<u8>> {
    let (_, below_root) = path.split_once("#USBROOT(")?;
    let mut ports = Vec::new();
    for step in below_root.split('#').skip(1) {
        if let Some(port) = step.strip_prefix("USB(").and_then(|s| s.strip_suffix(')')) {
            ports.push(port.parse().ok()?);
        } else if !step.starts_with("USBMI(") {
            return None;
        }
    }
    (!ports.is_empty()).then_some(ports)
}

/// The ports of a node from its location paths. An interface node also lists a path as if it
/// were a device on its own port (`#USB(8)#USB(8)`); the `USBMI` one is its own.
pub fn location(paths: &[String], interface: bool) -> Option<Vec<u8>> {
    let own = paths.iter().filter(|p| !p.starts_with("ACPI(")).find(|p| !interface || p.contains("#USBMI("));
    own.or_else(|| paths.first()).and_then(|p| parse_location_path(p))
}

fn is_winusb(service: &str) -> bool {
    service.eq_ignore_ascii_case("WinUSB")
}

fn is_libusbk(service: &str) -> bool {
    service.eq_ignore_ascii_case("libusbK")
}

fn is_supported_driver(service: &str) -> bool {
    is_winusb(service) || is_libusbk(service)
}

fn driver_name(service: &str) -> &str {
    if service.is_empty() { "no driver" } else { service }
}

/// None when Nightjar can open the device at `ports`: WinUSB (or an already-installed libusbK)
/// holds it, or every interface of a composite one. Otherwise which driver holds it, for the user.
pub fn blocked(nodes: &[Node], vid: u16, pid: u16, ports: &[u8]) -> Option<String> {
    let here = |n: &&Node| n.vid == vid && n.pid == pid && n.ports == ports;
    let devices: Vec<&Node> = nodes.iter().filter(here).filter(|n| n.interface.is_none()).collect();
    if devices.is_empty() {
        return Some("Windows lists no driver for it".into());
    }
    let mut drivers = Vec::new();
    for device in devices {
        if is_supported_driver(&device.service) { continue; }
        if device.service.eq_ignore_ascii_case("usbccgp") {
            let mut interfaces: Vec<&Node> = nodes.iter().filter(here).filter(|n| n.interface.is_some()).collect();
            interfaces.sort_by_key(|n| n.interface);
            if interfaces.is_empty() { drivers.push("usbccgp".to_owned()); }
            for interface in interfaces.into_iter().filter(|n| !is_supported_driver(&n.service)) {
                drivers.push(format!("interface {}: {}", interface.interface.unwrap_or_default(), driver_name(&interface.service)));
            }
        } else {
            drivers.push(driver_name(&device.service).to_owned());
        }
    }
    (!drivers.is_empty()).then(|| drivers.join(", "))
}

/// True when the device at `ports` is bound exclusively to Microsoft's inbox `WinUSB` driver
/// (either directly or on every interface of a `usbccgp` composite device), so isochronous
/// transfers should open it directly through inbox `winusb.dll`.
pub fn uses_winusb(nodes: &[Node], vid: u16, pid: u16, ports: &[u8]) -> bool {
    let here = |n: &&Node| n.vid == vid && n.pid == pid && n.ports == ports;
    let devices: Vec<&Node> = nodes.iter().filter(here).filter(|n| n.interface.is_none()).collect();
    if devices.is_empty() {
        return false;
    }
    let mut saw_winusb = false;
    for device in devices {
        if is_winusb(&device.service) {
            saw_winusb = true;
        } else if device.service.eq_ignore_ascii_case("usbccgp") {
            let interfaces: Vec<&Node> = nodes.iter().filter(here).filter(|n| n.interface.is_some()).collect();
            if interfaces.is_empty() || !interfaces.iter().all(|n| is_winusb(&n.service)) {
                return false;
            }
            saw_winusb = true;
        } else {
            return false;
        }
    }
    saw_winusb
}

/// Parses a registry GUID string (`{xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx}` with optional braces)
/// into `(data1, data2, data3, data4)`.
pub fn parse_guid(text: &str) -> Option<(u32, u16, u16, [u8; 8])> {
    let trimmed = text.trim().trim_start_matches('{').trim_end_matches('}');
    let mut parts = trimmed.split('-');
    let d1 = parts.next().filter(|s| s.len() == 8)?;
    let d2 = parts.next().filter(|s| s.len() == 4)?;
    let d3 = parts.next().filter(|s| s.len() == 4)?;
    let d4a = parts.next().filter(|s| s.len() == 4)?;
    let d4b = parts.next().filter(|s| s.len() == 12)?;
    if parts.next().is_some() {
        return None;
    }
    let data1 = u32::from_str_radix(d1, 16).ok()?;
    let data2 = u16::from_str_radix(d2, 16).ok()?;
    let data3 = u16::from_str_radix(d3, 16).ok()?;
    let mut data4 = [0u8; 8];
    let tail = format!("{d4a}{d4b}");
    for (i, slot) in data4.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&tail[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some((data1, data2, data3, data4))
}

/// Windows' device instance id of the device (not one of its interfaces) at `ports`.
pub fn instance_id(nodes: &[Node], vid: u16, pid: u16, ports: &[u8]) -> Option<String> {
    nodes.iter()
        .find(|n| n.vid == vid && n.pid == pid && n.ports == ports && n.interface.is_none() && !n.instance_id.is_empty())
        .map(|n| n.instance_id.clone())
}

/// The product string Windows has for the device at `ports`, if any.
pub fn description(nodes: &[Node], vid: u16, pid: u16, ports: &[u8]) -> Option<String> {
    nodes.iter()
        .find(|n| n.vid == vid && n.pid == pid && n.ports == ports && n.interface.is_none() && !n.description.is_empty())
        .map(|n| n.description.clone())
}

/// Every USB node Windows has present now.
#[cfg(windows)]
pub fn nodes() -> Result<Vec<Node>, String> {
    use windows::core::w;
    use windows::Win32::Devices::DeviceAndDriverInstallation::*;
    use windows::Win32::Devices::Properties::{DEVPKEY_Device_BusReportedDeviceDesc, DEVPROPTYPE};

    fn wide(buffer: &[u8]) -> Vec<String> {
        let units: Vec<u16> = buffer.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
        units.split(|&u| u == 0).filter(|s| !s.is_empty()).map(String::from_utf16_lossy).collect()
    }

    unsafe {
        let set = SetupDiGetClassDevsW(None, w!("USB"), None, DIGCF_ALLCLASSES | DIGCF_PRESENT)
            .map_err(|e| format!("Windows' USB device list could not be read: {e}"))?;
        let mut nodes = Vec::new();
        let mut index = 0;
        loop {
            let mut data = SP_DEVINFO_DATA { cbSize: std::mem::size_of::<SP_DEVINFO_DATA>() as u32, ..Default::default() };
            if SetupDiEnumDeviceInfo(set, index, &mut data).is_err() { break; }
            index += 1;
            let mut id = [0u16; 512];
            if SetupDiGetDeviceInstanceIdW(set, &data, Some(&mut id), None).is_err() { continue; }
            let id = String::from_utf16_lossy(&id[..id.iter().position(|&u| u == 0).unwrap_or(id.len())]);
            let Some((vid, pid, interface)) = parse_instance_id(&id) else { continue };
            let mut buffer = vec![0u8; 4096];
            let mut property = |key| {
                buffer.fill(0);
                SetupDiGetDeviceRegistryPropertyW(set, &data, key, None, Some(&mut buffer), None).ok().map(|()| wide(&buffer)).unwrap_or_default()
            };
            let Some(ports) = location(&property(SPDRP_LOCATION_PATHS), interface.is_some()) else { continue };
            let service = property(SPDRP_SERVICE).into_iter().next().unwrap_or_default();
            let mut kind = DEVPROPTYPE::default();
            buffer.fill(0);
            let description = SetupDiGetDevicePropertyW(set, &data, &DEVPKEY_Device_BusReportedDeviceDesc, &mut kind, Some(&mut buffer), None, 0)
                .ok().and_then(|()| wide(&buffer).into_iter().next()).unwrap_or_default();
            nodes.push(Node { vid, pid, interface, ports, service, description, instance_id: id });
        }
        let _ = SetupDiDestroyDeviceInfoList(set);
        Ok(nodes)
    }
}

/// SetupAPI device interface paths (`\\?\usb#...`) for opening a `WinUSB` device (or every
/// `WinUSB` child interface of a `usbccgp` composite device) directly through `winusb.dll`.
#[cfg(windows)]
pub fn interface_paths(vid: u16, pid: u16, ports: &[u8]) -> Result<Vec<String>, String> {
    use windows::core::{w, GUID, PCWSTR};
    use windows::Win32::Devices::DeviceAndDriverInstallation::*;
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{RegCloseKey, RegQueryValueExW, KEY_READ};

    const GUID_DEVINTERFACE_USB_DEVICE: GUID = GUID::from_values(
        0xA5DC_BF10,
        0x6530,
        0x11D2,
        [0x90, 0x1F, 0x00, 0xC0, 0x4F, 0xB9, 0x51, 0xED],
    );

    fn wide_strings(buffer: &[u8]) -> Vec<String> {
        let units: Vec<u16> = buffer.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
        units.split(|&u| u == 0).filter(|s| !s.is_empty()).map(String::from_utf16_lossy).collect()
    }

    unsafe fn reg_guids(set: HDEVINFO, data: &SP_DEVINFO_DATA) -> Vec<GUID> {
        let mut guids = Vec::new();
        // SAFETY: `set` and `data` come from a live SetupAPI enumeration in `interface_paths`.
        unsafe {
            let Ok(key) = SetupDiOpenDevRegKey(set, data, DICS_FLAG_GLOBAL.0, 0, DIREG_DEV, KEY_READ.0) else {
                return guids;
            };
            for name in [w!("DeviceInterfaceGUIDs"), w!("DeviceInterfaceGUID")] {
                let mut buf = vec![0u8; 4096];
                let mut size = buf.len() as u32;
                let status = RegQueryValueExW(key, name, None, None, Some(buf.as_mut_ptr()), Some(&mut size));
                if status == ERROR_SUCCESS {
                    let len = (size as usize).min(buf.len());
                    for text in wide_strings(&buf[..len]) {
                        if let Some((d1, d2, d3, d4)) = parse_guid(&text) {
                            guids.push(GUID::from_values(d1, d2, d3, d4));
                        }
                    }
                    if !guids.is_empty() {
                        break;
                    }
                }
            }
            let _ = RegCloseKey(key);
        }
        guids
    }

    unsafe fn path_for_guid(instance_id: &[u16], guid: &GUID) -> Option<String> {
        // SAFETY: `instance_id` is NUL-terminated UTF-16 and every `HDEVINFO` handle is destroyed before return.
        unsafe {
            let if_set = SetupDiGetClassDevsW(
                Some(guid),
                PCWSTR(instance_id.as_ptr()),
                None,
                DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
            )
            .ok()?;
            let mut result = None;
            let mut if_data = SP_DEVICE_INTERFACE_DATA {
                cbSize: std::mem::size_of::<SP_DEVICE_INTERFACE_DATA>() as u32,
                ..Default::default()
            };
            if SetupDiEnumDeviceInterfaces(if_set, None, guid, 0, &mut if_data).is_ok() {
                let mut required = 0u32;
                let _ = SetupDiGetDeviceInterfaceDetailW(if_set, &if_data, None, 0, Some(&mut required), None);
                let min_size = std::mem::size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>();
                if (required as usize) >= min_size {
                    let mut detail_buf = vec![0u32; (required as usize).div_ceil(4)];
                    let detail_ptr = detail_buf.as_mut_ptr().cast::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>();
                    (*detail_ptr).cbSize = min_size as u32;
                    if SetupDiGetDeviceInterfaceDetailW(
                        if_set,
                        &if_data,
                        Some(detail_ptr),
                        required,
                        None,
                        None,
                    )
                    .is_ok()
                    {
                        let offset = std::mem::offset_of!(SP_DEVICE_INTERFACE_DETAIL_DATA_W, DevicePath);
                        let bytes = std::slice::from_raw_parts(
                            detail_buf.as_ptr().cast::<u8>(),
                            required as usize,
                        );
                        if let Some(path) = wide_strings(&bytes[offset..]).into_iter().next() {
                            result = Some(path);
                        }
                    }
                }
            }
            let _ = SetupDiDestroyDeviceInfoList(if_set);
            result
        }
    }

    unsafe {
        let set = SetupDiGetClassDevsW(None, w!("USB"), None, DIGCF_ALLCLASSES | DIGCF_PRESENT)
            .map_err(|e| format!("Windows' USB device list could not be read: {e}"))?;
        let mut whole_paths = Vec::new();
        let mut iface_paths: Vec<(u8, String)> = Vec::new();
        let mut index = 0;
        loop {
            let mut data = SP_DEVINFO_DATA { cbSize: std::mem::size_of::<SP_DEVINFO_DATA>() as u32, ..Default::default() };
            if SetupDiEnumDeviceInfo(set, index, &mut data).is_err() { break; }
            index += 1;
            let mut id_buf = [0u16; 512];
            if SetupDiGetDeviceInstanceIdW(set, &data, Some(&mut id_buf), None).is_err() { continue; }
            let nul = id_buf.iter().position(|&u| u == 0).unwrap_or(id_buf.len());
            let id = String::from_utf16_lossy(&id_buf[..nul]);
            let Some((v, p, iface)) = parse_instance_id(&id) else { continue };
            if (v, p) != (vid, pid) { continue; }
            let mut prop_buf = vec![0u8; 4096];
            let mut property = |key| {
                prop_buf.fill(0);
                SetupDiGetDeviceRegistryPropertyW(set, &data, key, None, Some(&mut prop_buf), None)
                    .ok()
                    .map(|()| wide_strings(&prop_buf))
                    .unwrap_or_default()
            };
            let Some(node_ports) = location(&property(SPDRP_LOCATION_PATHS), iface.is_some()) else { continue };
            if node_ports != ports { continue; }
            let service = property(SPDRP_SERVICE).into_iter().next().unwrap_or_default();
            if !is_winusb(&service) { continue; }

            let mut guids = reg_guids(set, &data);
            guids.push(GUID_DEVINTERFACE_USB_DEVICE);
            let mut id_wide = id_buf[..nul].to_vec();
            id_wide.push(0);
            if let Some(path) = guids.iter().find_map(|g| path_for_guid(&id_wide, g)) {
                match iface {
                    None => whole_paths.push(path),
                    Some(mi) => iface_paths.push((mi, path)),
                }
            }
        }
        let _ = SetupDiDestroyDeviceInfoList(set);
        let mut out = if !whole_paths.is_empty() {
            whole_paths
        } else {
            iface_paths.sort_by_key(|(mi, _)| *mi);
            iface_paths.into_iter().map(|(_, p)| p).collect()
        };
        out.dedup();
        if out.is_empty() {
            return Err("Windows listed no WinUSB interface path for this device.".to_owned());
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(interface: Option<u8>, ports: &[u8], service: &str) -> Node {
        Node { vid: 0x1209, pid: 0x0001, interface, ports: ports.to_vec(), service: service.into(), description: String::new(), instance_id: String::new() }
    }

    #[test]
    fn instance_ids_give_vendor_product_and_interface() {
        assert_eq!(parse_instance_id(r"USB\VID_0781&PID_5581\4C530001"), Some((0x0781, 0x5581, None)));
        assert_eq!(parse_instance_id(r"USB\VID_054C&PID_0CE6&MI_03\7&1&0&0003"), Some((0x054c, 0x0ce6, Some(3))));
        assert_eq!(parse_instance_id(r"USB\ROOT_HUB30\4&1"), None);
        assert_eq!(parse_instance_id(r"HID\VID_054C&PID_0CE6\1"), None);
    }

    #[test]
    fn location_paths_give_the_ports_below_the_root_hub() {
        assert_eq!(parse_location_path("PCIROOT(0)#PCI(0801)#PCI(0003)#USBROOT(0)#USB(2)"), Some(vec![2]));
        assert_eq!(parse_location_path("PCIROOT(0)#PCI(1400)#USBROOT(0)#USB(4)#USB(1)#USBMI(0)"), Some(vec![4, 1]));
        assert_eq!(parse_location_path("PCIROOT(0)#PCI(1400)#USBROOT(0)"), None);
        assert_eq!(parse_location_path("ACPI(_SB_)#ACPI(PCI0)"), None);
    }

    #[test]
    fn an_interface_takes_its_own_location_path() {
        let paths = |list: &[&str]| list.iter().map(|p| p.to_string()).collect::<Vec<_>>();
        let composite = paths(&["PCIROOT(0)#PCI(0102)#PCI(0000)#USBROOT(0)#USB(8)#USB(8)", "PCIROOT(0)#PCI(0102)#PCI(0000)#USBROOT(0)#USB(8)#USBMI(0)", "ACPI(_SB_)#ACPI(PCI0)#ACPI(RHUB)#ACPI(POT8)#USBMI(0)"]);
        assert_eq!(location(&composite, true), Some(vec![8]));
        let device = paths(&["PCIROOT(0)#PCI(0102)#PCI(0000)#USBROOT(0)#USB(10)", "ACPI(_SB_)#ACPI(PCI0)#ACPI(RHUB)#ACPI(PO10)"]);
        assert_eq!(location(&device, false), Some(vec![10]));
        assert_eq!(location(&[], false), None);
    }

    #[test]
    fn winusb_and_libusbk_both_open_and_winusb_routing_distinguishes_them() {
        assert_eq!(blocked(&[node(None, &[2], "WinUSB")], 0x1209, 0x0001, &[2]), None);
        assert!(uses_winusb(&[node(None, &[2], "WinUSB")], 0x1209, 0x0001, &[2]));
        assert_eq!(blocked(&[node(None, &[2], "libusbK")], 0x1209, 0x0001, &[2]), None);
        assert!(!uses_winusb(&[node(None, &[2], "libusbK")], 0x1209, 0x0001, &[2]));
        assert_eq!(blocked(&[node(None, &[2], "USBSTOR")], 0x1209, 0x0001, &[2]).as_deref(), Some("USBSTOR"));
        assert!(!uses_winusb(&[node(None, &[2], "USBSTOR")], 0x1209, 0x0001, &[2]));
        assert_eq!(blocked(&[node(None, &[2], "")], 0x1209, 0x0001, &[2]).as_deref(), Some("no driver"));
        // Another port, or nothing Windows knows: never offered as if it would work.
        assert_eq!(blocked(&[node(None, &[3], "WinUSB")], 0x1209, 0x0001, &[2]).as_deref(), Some("Windows lists no driver for it"));
    }

    #[test]
    fn a_composite_device_opens_when_every_interface_has_winusb_or_libusbk() {
        let mut nodes = vec![node(None, &[1, 4], "usbccgp"), node(Some(0), &[1, 4], "WinUSB"), node(Some(1), &[1, 4], "WinUSB")];
        assert_eq!(blocked(&nodes, 0x1209, 0x0001, &[1, 4]), None);
        assert!(uses_winusb(&nodes, 0x1209, 0x0001, &[1, 4]));
        // Mixed WinUSB + libusbK opens through libusb, not pure WinUSB.
        let mixed = vec![node(None, &[1, 4], "usbccgp"), node(Some(0), &[1, 4], "WinUSB"), node(Some(1), &[1, 4], "libusbK")];
        assert_eq!(blocked(&mixed, 0x1209, 0x0001, &[1, 4]), None);
        assert!(!uses_winusb(&mixed, 0x1209, 0x0001, &[1, 4]));
        nodes.push(node(Some(3), &[1, 4], "HidUsb"));
        assert_eq!(blocked(&nodes, 0x1209, 0x0001, &[1, 4]).as_deref(), Some("interface 3: HidUsb"));
        assert!(!uses_winusb(&nodes, 0x1209, 0x0001, &[1, 4]));
        assert_eq!(blocked(&nodes[..1], 0x1209, 0x0001, &[1, 4]).as_deref(), Some("usbccgp"));
    }

    #[test]
    fn registry_guids_parse_with_or_without_braces() {
        assert_eq!(
            parse_guid("{A5DCBF10-6530-11D2-901F-00C04FB951ED}"),
            Some((0xA5DC_BF10, 0x6530, 0x11D2, [0x90, 0x1F, 0x00, 0xC0, 0x4F, 0xB9, 0x51, 0xED]))
        );
        assert_eq!(
            parse_guid("da812bff-12c3-46a2-8e2b-dbd3b7834c43"),
            Some((0xda81_2bff, 0x12c3, 0x46a2, [0x8e, 0x2b, 0xdb, 0xd3, 0xb7, 0x83, 0x4c, 0x43]))
        );
        assert_eq!(parse_guid("not-a-guid"), None);
        assert_eq!(parse_guid("{A5DCBF10-6530-11D2-901F-00C04FB951ED-extra}"), None);
    }

    #[test]
    fn the_name_is_the_device_nodes_own() {
        let mut device = node(None, &[10], "USBSTOR");
        device.description = "STORE N GO".into();
        let mut interface = node(Some(0), &[10], "WinUSB");
        interface.description = "Interface".into();
        let nodes = [interface, node(None, &[11], "WinUSB"), device];
        assert_eq!(description(&nodes, 0x1209, 0x0001, &[10]).as_deref(), Some("STORE N GO"));
        assert_eq!(description(&nodes, 0x1209, 0x0001, &[11]), None);
    }

    #[cfg(windows)]
    #[test]
    fn windows_lists_usb_nodes_without_opening_them() {
        // Every node it lists parses; a PC without USB devices lists none.
        for node in nodes().unwrap() {
            assert!(!node.ports.is_empty(), "{node:?}");
        }
    }
}
