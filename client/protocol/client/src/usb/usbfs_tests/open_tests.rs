//! Descriptor parsing, interface claiming, and speed detection tests for usbfs.

use super::*;

#[test]
fn the_descriptors_name_the_device_its_configurations_and_interfaces() {
    let parsed = Descriptors::parse(&descriptors()).unwrap();
    assert_eq!(
        (parsed.vendor, parsed.product, parsed.bcd_device),
        (0x1209, 0x0001, 0x0100)
    );
    assert_eq!(parsed.configurations.len(), 2);
    let first = parsed.configuration(1).unwrap();
    assert_eq!(
        first.interfaces,
        vec![
            Interface {
                number: 0,
                class: 0xff
            },
            Interface {
                number: 1,
                class: 3
            }
        ]
    );
    assert_eq!(
        first.endpoints,
        vec![(0x01, 2), (0x81, 2), (0x82, 3), (0x83, 1), (0x03, 1)]
    );
    // Cut anywhere inside, they fail rather than describe half a device.
    let raw = descriptors();
    for cut in [10, 20, 30, raw.len() - 3] {
        assert!(Descriptors::parse(&raw[..cut]).is_err(), "cut at {cut}");
    }
    let mut zero = raw.clone();
    zero[18 + 9] = 0;
    assert!(Descriptors::parse(&zero).is_err());
    assert_eq!(bus_and_device(PATH), Some((1, 5)));
    assert_eq!(bus_and_device("/dev/bus/usb/x/5"), None);
}

#[test]
fn opening_claims_every_interface_and_closing_gives_them_back_to_their_drivers() {
    let kernel = FakeKernel::new();
    let device = UsbfsDevice::open(kernel.clone(), PATH).unwrap();
    let described = device.describe();
    assert_eq!(
        described,
        wire::Device {
            path: PATH.into(),
            busid: "1-5".into(),
            busnum: 1,
            devnum: 5,
            speed: 3,
            vendor: 0x1209,
            product: 0x0001,
            bcd_device: 0x0100,
            class: 0,
            subclass: 0,
            protocol: 0,
            configuration: 1,
            configurations: 2,
            interfaces: 2,
        }
    );
    // Class 0 on the device: the first interface's.
    assert_eq!(device.class(), 0xff);
    assert_eq!(
        kernel.log(),
        ["claim 0", "claim 1", "disconnect 1", "claim 1"]
    );
    device.close();
    device.close();
    assert_eq!(
        kernel.log()[4..],
        ["release 0", "release 1", "connect 1", "close"]
    );
}

/// What a kernel before Linux 4.13 makes of a device: no USBDEVFS_GET_SPEED (ENOTTY).
fn speed_on_an_old_kernel(change: impl FnOnce(&mut Machine)) -> u32 {
    let kernel = FakeKernel::new();
    kernel.machine().no_speed = Some(Errno(25));
    change(&mut kernel.machine());
    let device = UsbfsDevice::open(kernel.clone(), PATH).unwrap();
    let speed = device.describe().speed;
    device.close();
    speed
}

#[test]
fn a_kernel_that_cannot_say_the_speed_has_it_read_from_the_device() {
    // An NVIDIA Shield (Linux 4.9) opened nothing: every device failed on its speed.
    // 64-byte endpoints and no device qualifier: full speed, as a controller's receiver is.
    assert_eq!(speed_on_an_old_kernel(|_| {}), 2);
    // The kernel answering "unknown" reads the same.
    let kernel = FakeKernel::new();
    kernel.machine().speed = 0;
    let device = UsbfsDevice::open(kernel.clone(), PATH).unwrap();
    assert_eq!(device.describe().speed, 2);
    device.close();
    // A device qualifier: it can run at high speed, so it does.
    assert_eq!(speed_on_an_old_kernel(|m| m.qualifier = true), 3);
    assert_eq!(speed_on_an_old_kernel(|m| m.slow = true), 1);

    // A 512-byte bulk endpoint exists at high speed only; no qualifier is needed.
    let mut bulk = descriptors();
    let at = 18 + 9 + 9;
    assert_eq!(bulk[at..at + 4], [7, 5, 0x01, 2]);
    bulk[at + 4..at + 6].copy_from_slice(&512u16.to_le_bytes());
    assert_eq!(speed_on_an_old_kernel(|m| m.raw = Some(bulk.clone())), 3);
    // So does an interrupt endpoint with three transactions a microframe.
    let mut interrupt = descriptors();
    let at = 18 + 9 + 9 + 7 + 7;
    assert_eq!(interrupt[at..at + 4], [7, 5, 0x82, 3]);
    interrupt[at + 4..at + 6].copy_from_slice(&(2u16 << 11 | 64).to_le_bytes());
    assert_eq!(speed_on_an_old_kernel(|m| m.raw = Some(interrupt.clone())), 3);

    // USB 3 with a 512-byte control endpoint is SuperSpeed; USB 1.1 never has a qualifier.
    let mut usb3 = descriptors();
    usb3[2..4].copy_from_slice(&0x0300u16.to_le_bytes());
    usb3[7] = 9;
    assert_eq!(speed_on_an_old_kernel(|m| m.raw = Some(usb3.clone())), 5);
    let mut usb1 = descriptors();
    usb1[2..4].copy_from_slice(&0x0110u16.to_le_bytes());
    assert_eq!(
        speed_on_an_old_kernel(|m| {
            m.raw = Some(usb1.clone());
            m.qualifier = true;
        }),
        2
    );

    assert_eq!(speed_of_megabits("1.5"), Some(1));
    assert_eq!(speed_of_megabits("12"), Some(2));
    assert_eq!(speed_of_megabits("480"), Some(3));
    assert_eq!(speed_of_megabits("5000"), Some(5));
    assert_eq!(speed_of_megabits("10000"), Some(6));
    assert_eq!(speed_of_megabits(""), None);
}

#[test]
fn a_device_that_cannot_be_opened_fails_loudly_and_closes() {
    let kernel = FakeKernel::new();
    let error = UsbfsDevice::open(kernel.clone(), "/proc/self/fd/9")
        .err()
        .unwrap();
    assert!(error.contains("not a USB device node"), "{error}");

    let kernel = FakeKernel::new();
    kernel.machine().refuse = Some(1);
    let error = UsbfsDevice::open(kernel.clone(), PATH).err().unwrap();
    assert!(error.contains("interface 1"), "{error}");
    assert_eq!(kernel.log(), ["claim 0", "claim 1", "release 0", "close"]);
}
