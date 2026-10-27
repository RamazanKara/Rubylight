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

fn driver_name(service: &str) -> &str {
    if service.is_empty() { "no driver" } else { service }
}

/// None when libusb can open the device at `ports`: WinUSB holds it, or every interface of a
/// composite one. Otherwise which driver holds it, for the user.
pub fn blocked(nodes: &[Node], vid: u16, pid: u16, ports: &[u8]) -> Option<String> {
    let here = |n: &&Node| n.vid == vid && n.pid == pid && n.ports == ports;
    let devices: Vec<&Node> = nodes.iter().filter(here).filter(|n| n.interface.is_none()).collect();
    if devices.is_empty() {
        return Some("Windows lists no driver for it".into());
    }
    let mut drivers = Vec::new();
    for device in devices {
        if is_winusb(&device.service) { continue; }
        if device.service.eq_ignore_ascii_case("usbccgp") {
            let mut interfaces: Vec<&Node> = nodes.iter().filter(here).filter(|n| n.interface.is_some()).collect();
            interfaces.sort_by_key(|n| n.interface);
            if interfaces.is_empty() { drivers.push("usbccgp".to_owned()); }
            for interface in interfaces.into_iter().filter(|n| !is_winusb(&n.service)) {
                drivers.push(format!("interface {}: {}", interface.interface.unwrap_or_default(), driver_name(&interface.service)));
            }
        } else {
            drivers.push(driver_name(&device.service).to_owned());
        }
    }
    (!drivers.is_empty()).then(|| drivers.join(", "))
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
        let units: Vec<u16> = buffer.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
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
            nodes.push(Node { vid, pid, interface, ports, service, description });
        }
        let _ = SetupDiDestroyDeviceInfoList(set);
        Ok(nodes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(interface: Option<u8>, ports: &[u8], service: &str) -> Node {
        Node { vid: 0x1209, pid: 0x0001, interface, ports: ports.to_vec(), service: service.into(), description: String::new() }
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
    fn only_winusb_opens() {
        assert_eq!(blocked(&[node(None, &[2], "WinUSB")], 0x1209, 0x0001, &[2]), None);
        assert_eq!(blocked(&[node(None, &[2], "USBSTOR")], 0x1209, 0x0001, &[2]).as_deref(), Some("USBSTOR"));
        assert_eq!(blocked(&[node(None, &[2], "")], 0x1209, 0x0001, &[2]).as_deref(), Some("no driver"));
        // Another port, or nothing Windows knows: never offered as if it would work.
        assert_eq!(blocked(&[node(None, &[3], "WinUSB")], 0x1209, 0x0001, &[2]).as_deref(), Some("Windows lists no driver for it"));
    }

    #[test]
    fn a_composite_device_opens_only_when_every_interface_has_winusb() {
        let mut nodes = vec![node(None, &[1, 4], "usbccgp"), node(Some(0), &[1, 4], "WinUSB"), node(Some(1), &[1, 4], "WinUSB")];
        assert_eq!(blocked(&nodes, 0x1209, 0x0001, &[1, 4]), None);
        nodes.push(node(Some(3), &[1, 4], "HidUsb"));
        assert_eq!(blocked(&nodes, 0x1209, 0x0001, &[1, 4]).as_deref(), Some("interface 3: HidUsb"));
        assert_eq!(blocked(&nodes[..1], 0x1209, 0x0001, &[1, 4]).as_deref(), Some("usbccgp"));
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
