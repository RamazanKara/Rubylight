//! USB descriptor parsing, speed fallback detection, and usbfs device-node path parsing.

use super::kernel::Kernel;

/// One interface of a configuration, at alternate setting 0.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Interface {
    pub number: u8,
    pub class: u8,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Configuration {
    pub value: u8,
    pub interfaces: Vec<Interface>,
    /// Every endpoint address of every alternate setting, with its transfer type.
    pub endpoints: Vec<(u8, u8)>,
}

/// What the descriptors say about the device.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Descriptors {
    pub vendor: u16,
    pub product: u16,
    pub bcd_device: u16,
    pub class: u8,
    pub subclass: u8,
    pub protocol: u8,
    pub configurations: Vec<Configuration>,
}

impl Descriptors {
    /// Parses what usbfs's `read` returns: the device descriptor, then each configuration's.
    pub fn parse(raw: &[u8]) -> Result<Self, String> {
        let device = raw
            .get(..18)
            .filter(|d| d[0] >= 18 && d[1] == 1)
            .ok_or("no device descriptor")?;
        let mut parsed = Self {
            bcd_device: u16::from_le_bytes([device[12], device[13]]),
            vendor: u16::from_le_bytes([device[8], device[9]]),
            product: u16::from_le_bytes([device[10], device[11]]),
            class: device[4],
            subclass: device[5],
            protocol: device[6],
            configurations: Vec::new(),
        };
        let mut at = usize::from(device[0]);
        while at < raw.len() {
            let head = raw
                .get(at..at + 9)
                .ok_or("a cut configuration descriptor")?;
            if head[1] != 2 || head[0] < 9 {
                return Err(format!(
                    "descriptor type {} where a configuration belongs",
                    head[1]
                ));
            }
            let total = usize::from(u16::from_le_bytes([head[2], head[3]]));
            let body = raw
                .get(at..at + total.max(9))
                .ok_or("a configuration longer than the descriptors")?;
            parsed.configurations.push(configuration(body)?);
            at += total.max(9);
        }
        Ok(parsed)
    }

    pub fn configuration(&self, value: u8) -> Option<&Configuration> {
        self.configurations.iter().find(|c| c.value == value)
    }
}

fn configuration(body: &[u8]) -> Result<Configuration, String> {
    let mut parsed = Configuration {
        value: body[5],
        ..Configuration::default()
    };
    let mut at = usize::from(body[0]);
    while at + 2 <= body.len() {
        let length = usize::from(body[at]);
        if length < 2 || at + length > body.len() {
            return Err(format!(
                "a descriptor of {length} bytes in configuration {}",
                parsed.value
            ));
        }
        let descriptor = &body[at..at + length];
        match descriptor[1] {
            4 if length >= 9 => {
                if !parsed.interfaces.iter().any(|i| i.number == descriptor[2]) {
                    parsed.interfaces.push(Interface {
                        number: descriptor[2],
                        class: descriptor[5],
                    });
                }
            }
            5 if length >= 7
                && !parsed
                    .endpoints
                    .iter()
                    .any(|(address, _)| *address == descriptor[2]) =>
            {
                parsed.endpoints.push((descriptor[2], descriptor[3] & 3));
            }
            _ => {}
        }
        at += length;
    }
    Ok(parsed)
}

const SPEED_LOW: u32 = 1;
const SPEED_FULL: u32 = 2;
const SPEED_HIGH: u32 = 3;
const SPEED_SUPER: u32 = 5;
const SPEED_SUPER_PLUS: u32 = 6;

/// The speed of a device whose kernel cannot say it (USBDEVFS_GET_SPEED is Linux 4.13's),
/// from `raw`, its descriptors as parsed: low speed from the kernel's connect info, SuperSpeed
/// from its 512-byte control endpoint, high speed from an endpoint only high speed allows or
/// from a device qualifier, which a full-speed-only device does not have.
pub(super) fn guessed_speed<K: Kernel>(kernel: &K, raw: &[u8]) -> u32 {
    if kernel.low_speed().unwrap_or(false) {
        return SPEED_LOW;
    }
    let bcd_usb = u16::from_le_bytes([raw[2], raw[3]]);
    if bcd_usb >= 0x0300 && raw[7] == 9 {
        return SPEED_SUPER;
    }
    if bcd_usb < 0x0200 {
        return SPEED_FULL;
    }
    if has_high_speed_endpoint(raw) {
        return SPEED_HIGH;
    }
    // On a high-speed port a device that can run at high speed does.
    let mut qualifier = [0u8; 10];
    match kernel.control_in(0x80, 6, 0x0600, 0, &mut qualifier) {
        Ok(read) if read >= 2 && qualifier[1] == 6 => SPEED_HIGH,
        _ => SPEED_FULL,
    }
}

/// `enum usb_device_speed` for sysfs's `speed` attribute, megabits a second.
pub fn speed_of_megabits(megabits: &str) -> Option<u32> {
    match megabits {
        "1.5" => Some(SPEED_LOW),
        "12" => Some(SPEED_FULL),
        "480" => Some(SPEED_HIGH),
        "5000" => Some(SPEED_SUPER),
        "10000" | "20000" => Some(SPEED_SUPER_PLUS),
        _ => None,
    }
}

/// True when an endpoint is larger than full speed allows: bulk or interrupt over 64 bytes,
/// isochronous over 1023, or more than one transaction a microframe.
fn has_high_speed_endpoint(raw: &[u8]) -> bool {
    let mut at = usize::from(raw[0]);
    while at + 2 <= raw.len() {
        let length = usize::from(raw[at]);
        if length < 2 || at + length > raw.len() {
            return false;
        }
        let descriptor = &raw[at..at + length];
        if descriptor[1] == 5 && length >= 7 {
            let packet = u16::from_le_bytes([descriptor[4], descriptor[5]]);
            let size = packet & 0x7ff;
            let larger = match descriptor[3] & 3 {
                1 => size > 1023,
                2 | 3 => size > 64,
                _ => false,
            };
            if larger || (packet >> 11) & 3 != 0 {
                return true;
            }
        }
        at += length;
    }
    false
}

/// Bus and device number of an Android or Linux device node, `/dev/bus/usb/001/005`.
pub fn bus_and_device(path: &str) -> Option<(u32, u32)> {
    let mut parts = path.rsplit('/');
    let device = parts.next()?.parse().ok()?;
    let bus = parts.next()?.parse().ok()?;
    Some((bus, device))
}

pub(super) fn interface_count(descriptors: &Descriptors, configuration: u8) -> u8 {
    descriptors
        .configuration(configuration)
        .map_or(0, |c| c.interfaces.len().min(255) as u8)
}
