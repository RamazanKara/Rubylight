//! URB transfer, ioctl intercept, unplug, and close tests for usbfs.

use super::*;

#[test]
fn control_bulk_and_interrupt_transfers_run_as_urbs_of_their_type() {
    let kernel = FakeKernel::new();
    let (mut driver, served) = serving(&kernel);

    let descriptor = driver.submit(0, wire::DIR_IN, 18, setup(0x80, 6, 0x0100, 18), &[], &[]);
    let (header, data, _) = driver.reply(true);
    assert_eq!(
        (status_of(&header), data.as_slice()),
        ((descriptor, 0), &descriptors()[..18])
    );

    let stored = driver.submit(0, wire::DIR_OUT, 3, setup(0x40, 1, 0, 3), &[1, 2, 3], &[]);
    assert_eq!(status_of(&driver.reply(false).0), (stored, 0));
    let read = driver.submit(0, wire::DIR_IN, 8, setup(0xc0, 1, 0, 8), &[], &[]);
    let (header, data, _) = driver.reply(true);
    assert_eq!((status_of(&header), data), ((read, 0), vec![1, 2, 3]));

    let stall = driver.submit(0, wire::DIR_IN, 2, setup(0x80, 0x55, 0, 2), &[], &[]);
    assert_eq!(status_of(&driver.reply(true).0), (stall, wire::EPIPE));

    // A bulk IN waits in the kernel while the OUT that feeds it runs beside it.
    let waiting = driver.submit(1, wire::DIR_IN, 64, [0; 8], &[], &[]);
    kernel.wait_parked(1);
    let sent = driver.submit(1, wire::DIR_OUT, 5, [0; 8], b"hello", &[]);
    // The OUT completes first and feeds the IN.
    assert!(
        matches!(driver.reply(false).0, Header::RetSubmit { base, length: 5, .. } if base.seqnum == sent)
    );
    let (header, data, _) = driver.reply(true);
    assert_eq!(
        (status_of(&header), data.as_slice()),
        ((waiting, 0), &b"hello"[..])
    );

    let report = driver.submit(2, wire::DIR_IN, 8, [0; 8], &[], &[]);
    kernel.wait_parked(1);
    kernel.push_report(&[7, 7]);
    let (header, data, _) = driver.reply(true);
    assert_eq!((status_of(&header), data), ((report, 0), vec![7, 7]));

    let unknown = driver.submit(4, wire::DIR_IN, 8, [0; 8], &[], &[]);
    assert_eq!(status_of(&driver.reply(true).0), (unknown, -2));

    assert_eq!(
        submits(&kernel),
        [
            "submit 2 0x00",
            "submit 2 0x00",
            "submit 2 0x00",
            "submit 2 0x00",
            "submit 3 0x81",
            "submit 3 0x01",
            "submit 1 0x82",
            "submit 3 0x84",
        ]
    );
    drop(driver);
    served.join().unwrap().unwrap();
}

#[test]
fn an_unlinked_urb_is_discarded_and_its_completion_dropped() {
    let kernel = FakeKernel::new();
    let (mut driver, served) = serving(&kernel);
    let waiting = driver.submit(2, wire::DIR_IN, 8, [0; 8], &[], &[]);
    kernel.wait_parked(1);
    let unlink = driver.unlink(waiting);
    assert_eq!(
        status_of(&driver.reply(false).0),
        (unlink, wire::ECONNRESET)
    );
    // The answer goes out before the device hears of it.
    let deadline = Instant::now() + Duration::from_secs(10);
    while !kernel.log().contains(&"discard".to_string()) {
        assert!(Instant::now() < deadline, "the URB was never discarded");
        std::thread::sleep(Duration::from_millis(2));
    }
    // The next report goes to the next transfer; nothing answers the discarded one.
    let next = driver.submit(2, wire::DIR_IN, 8, [0; 8], &[], &[]);
    kernel.wait_parked(1);
    kernel.push_report(&[9]);
    let (header, data, _) = driver.reply(true);
    assert_eq!((status_of(&header), data), ((next, 0), vec![9]));
    drop(driver);
    served.join().unwrap().unwrap();
}

#[test]
fn set_interface_goes_through_its_ioctl_and_isochronous_packets_pack_both_ways() {
    let kernel = FakeKernel::new();
    let (mut driver, served) = serving(&kernel);
    let alternate = driver.submit(0, wire::DIR_OUT, 0, setup(0x01, 11, 1, 0), &[], &[]);
    assert_eq!(status_of(&driver.reply(false).0), (alternate, 0));
    assert!(kernel.log().contains(&"set interface 0 1".to_string()));

    let asked: Vec<IsoPacket> = (0..4)
        .map(|i| IsoPacket {
            offset: i * 8,
            length: 8,
            actual_length: 0,
            status: 0,
        })
        .collect();
    let iso = driver.submit(3, wire::DIR_IN, 32, [0; 8], &[], &asked);
    let (header, data, packets) = driver.reply(true);
    assert_eq!(status_of(&header), (iso, 0));
    assert!(matches!(
        header,
        Header::RetSubmit {
            errors: 1,
            length: 9,
            ..
        }
    ));
    assert_eq!(data, [0, 0, 0, 1, 1, 1, 3, 3, 3]);
    let got: Vec<(u32, u32, u32, i32)> = packets
        .iter()
        .map(|p| (p.offset, p.length, p.actual_length, p.status))
        .collect();
    assert_eq!(
        got,
        [(0, 8, 3, 0), (8, 8, 3, 0), (16, 8, 0, EXDEV), (24, 8, 3, 0)]
    );

    // OUT packets sit at their offsets in USB/IP and back to back in usbfs.
    let out = [
        IsoPacket {
            offset: 0,
            length: 3,
            actual_length: 0,
            status: 0,
        },
        IsoPacket {
            offset: 8,
            length: 3,
            actual_length: 0,
            status: 0,
        },
    ];
    let data: Vec<u8> = (0..16).collect();
    let sent = driver.submit(3, wire::DIR_OUT, 16, [0; 8], &data, &out);
    let (header, _, packets) = driver.reply(false);
    assert_eq!(status_of(&header), (sent, 0));
    assert!(matches!(header, Header::RetSubmit { length: 6, .. }));
    assert_eq!(packets.len(), 2);
    assert_eq!(kernel.machine().iso_out, [0, 1, 2, 8, 9, 10]);
    drop(driver);
    served.join().unwrap().unwrap();
}

#[test]
fn isochronous_packets_whose_offsets_or_lengths_wrap_fail_and_reach_no_kernel() {
    let kernel = FakeKernel::new();
    let (mut driver, served) = serving(&kernel);
    let alternate = driver.submit(0, wire::DIR_OUT, 0, setup(0x01, 11, 1, 0), &[], &[]);
    assert_eq!(status_of(&driver.reply(false).0), (alternate, 0));
    let packet = |offset, length| IsoPacket {
        offset,
        length,
        actual_length: 0,
        status: 0,
    };
    // offset + length past u32: it wrapped to 2, inside the data.
    let data: Vec<u8> = (0..16).collect();
    let wraps = [packet(u32::MAX - 1, 4)];
    let out = driver.submit(3, wire::DIR_OUT, 16, [0; 8], &data, &wraps);
    assert_eq!(status_of(&driver.reply(false).0), (out, wire::EINVAL));
    // Lengths whose sum is past u32 (and a 32-bit usize).
    let huge = [packet(0, 0x8000_0000), packet(0, 0x8000_0010)];
    let iso = driver.submit(3, wire::DIR_IN, 16, [0; 8], &[], &huge);
    assert_eq!(status_of(&driver.reply(true).0), (iso, wire::EINVAL));
    assert!(submits(&kernel).is_empty());
    drop(driver);
    served.join().unwrap().unwrap();
}

#[test]
fn configuration_halts_and_resets_change_the_kernels_state_not_the_devices() {
    let kernel = FakeKernel::new();
    let device = Arc::new(UsbfsDevice::open(kernel.clone(), PATH).unwrap());
    let (tunnel, mut driver) = pipe();
    let serving = device.clone();
    let served = std::thread::spawn(move || serve(tunnel, serving));
    driver.import("1-5").unwrap();
    let opened = kernel.log().len();

    // The configuration Android chose already: nothing to do.
    let same = driver.submit(0, wire::DIR_OUT, 0, setup(0x00, 9, 1, 0), &[], &[]);
    assert_eq!(status_of(&driver.reply(false).0), (same, 0));
    assert_eq!(kernel.log().len(), opened);

    let halt = driver.submit(
        0,
        wire::DIR_OUT,
        0,
        with_index(setup(0x02, 1, 0, 0), 0x81),
        &[],
        &[],
    );
    assert_eq!(status_of(&driver.reply(false).0), (halt, 0));

    let other = driver.submit(0, wire::DIR_OUT, 0, setup(0x00, 9, 2, 0), &[], &[]);
    assert_eq!(status_of(&driver.reply(false).0), (other, 0));
    assert_eq!(
        (
            device.describe().configuration,
            device.describe().interfaces
        ),
        (2, 1)
    );

    let reset = driver.submit(
        0,
        wire::DIR_OUT,
        0,
        with_index(setup(0x23, 3, 4, 0), 1),
        &[],
        &[],
    );
    assert_eq!(status_of(&driver.reply(false).0), (reset, 0));
    assert_eq!(
        kernel.log()[opened..],
        [
            "clear halt 0x81",
            "release 0",
            "release 1",
            "set configuration 2",
            "claim 0",
            "reset",
            "claim 0",
        ]
    );
    assert!(submits(&kernel).is_empty());
    drop(driver);
    served.join().unwrap().unwrap();
}

#[test]
fn an_unplugged_device_fails_what_it_had_and_everything_after() {
    let kernel = FakeKernel::new();
    let (mut driver, served) = serving(&kernel);
    let waiting = driver.submit(1, wire::DIR_IN, 64, [0; 8], &[], &[]);
    kernel.wait_parked(1);
    kernel.unplug();
    let (seqnum, status) = status_of(&driver.reply(true).0);
    assert_eq!(seqnum, waiting);
    assert!(status == -108 || status == wire::ENODEV, "{status}");
    // Once the reaper saw it go, nothing reaches the kernel.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let before = submits(&kernel).len();
        let after = driver.submit(1, wire::DIR_OUT, 1, [0; 8], &[1], &[]);
        let (seqnum, status) = status_of(&driver.reply(false).0);
        assert_eq!(seqnum, after);
        assert_eq!(status, wire::ENODEV);
        if submits(&kernel).len() == before {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(driver);
    served.join().unwrap().unwrap();
}

#[test]
fn closing_discards_what_runs_before_the_descriptor_closes() {
    let kernel = FakeKernel::new();
    let (mut driver, served) = serving(&kernel);
    driver.submit(1, wire::DIR_IN, 64, [0; 8], &[], &[]);
    driver.submit(2, wire::DIR_IN, 8, [0; 8], &[], &[]);
    kernel.wait_parked(2);
    // The host closes the tunnel: serving ends and closes the device.
    drop(driver);
    served.join().unwrap().unwrap();
    let log = kernel.log();
    let close = log.iter().position(|e| e == "close").unwrap();
    assert_eq!(
        log[close - 5..close],
        ["discard", "discard", "release 0", "release 1", "connect 1"]
    );
    assert!(kernel.machine().parked.is_empty());
}

#[test]
fn unplugged_before_close_keeps_orphaned_urbs_until_close() {
    let kernel = FakeKernel::new();
    let device = Arc::new(UsbfsDevice::open(kernel.clone(), PATH).unwrap());
    let (tunnel, mut driver) = pipe();
    let serving = device.clone();
    let served = std::thread::spawn(move || serve(tunnel, serving));
    driver.import("1-5").unwrap();

    let waiting = driver.submit(1, wire::DIR_IN, 64, [0; 8], &[], &[]);
    kernel.wait_parked(1);
    {
        kernel.machine().gone = true;
        kernel.changed.notify_all();
    }
    let (seqnum, status) = status_of(&driver.reply(true).0);
    assert_eq!((seqnum, status), (waiting, wire::ENODEV));
    assert_eq!(device.inner.state().orphaned.len(), 1);
    assert!(!kernel.machine().closed);

    drop(driver);
    served.join().unwrap().unwrap();
    assert!(kernel.machine().closed);
    assert!(device.inner.state().orphaned.is_empty());
}

#[test]
fn concurrent_submit_and_close_never_submits_after_close() {
    for _ in 0..20 {
        let kernel = FakeKernel::new();
        let device = Arc::new(UsbfsDevice::open(kernel.clone(), PATH).unwrap());
        let (tunnel, mut driver) = pipe();
        let serving = device.clone();
        let served = std::thread::spawn(move || serve(tunnel, serving));
        driver.import("1-5").unwrap();

        let closing = device.clone();
        let closer = std::thread::spawn(move || closing.close());
        for _ in 0..16 {
            driver.submit(1, wire::DIR_OUT, 5, [0; 8], b"hello", &[]);
        }
        drop(driver);
        closer.join().unwrap();
        let _ = served.join().unwrap();

        let log = kernel.log();
        let close = log.iter().position(|e| e == "close").unwrap();
        assert!(
            !log[close + 1..].iter().any(|e| e.starts_with("submit ")),
            "kernel submit ran after close: {log:?}"
        );
        let state = device.inner.state();
        assert!(state.flights.is_empty());
        assert!(state.orphaned.is_empty());
    }
}
