use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::Duration;

use rubylight_protocol::usbip::{self as wire, Base, Header, IsoPacket};

use super::*;
use crate::usb::fake::{self, FakeDevice};
use crate::usb::{Blocking, TcpTunnel};

/// The host's driver end of a tunnel.
pub(crate) struct Driver {
    stream: TcpStream,
    seqnum: u32,
}

/// A connected tunnel and the driver end of it.
pub(crate) fn pipe() -> (Arc<TcpTunnel>, Driver) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (driver, _) = listener.accept().unwrap();
    driver
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    (
        Arc::new(TcpTunnel::new(client).unwrap()),
        Driver {
            stream: driver,
            seqnum: 0,
        },
    )
}

pub(crate) fn setup(request_type: u8, request: u8, value: u16, length: u16) -> [u8; 8] {
    let (value, length) = (value.to_le_bytes(), length.to_le_bytes());
    [
        request_type,
        request,
        value[0],
        value[1],
        0,
        0,
        length[0],
        length[1],
    ]
}

impl Driver {
    /// Imports `busid`: the device, or the reply's status.
    pub(crate) fn import(&mut self, busid: &str) -> Result<wire::Device, u32> {
        self.stream
            .write_all(&wire::import_request(busid).unwrap())
            .unwrap();
        let mut head = [0; wire::OP_HEADER_BYTES];
        self.stream.read_exact(&mut head).unwrap();
        let head = wire::OpHeader::decode(&head);
        assert_eq!(head.code, wire::OP_REP_IMPORT);
        if head.status != 0 {
            return Err(head.status);
        }
        let mut device = [0; wire::DEVICE_BYTES];
        self.stream.read_exact(&mut device).unwrap();
        Ok(wire::Device::decode(&device).unwrap())
    }

    /// Submits a transfer and returns its seqnum.
    pub(crate) fn submit(
        &mut self,
        ep: u32,
        direction: u32,
        length: u32,
        setup: [u8; 8],
        data: &[u8],
        iso: &[IsoPacket],
    ) -> u32 {
        self.seqnum += 1;
        let header = Header::Submit {
            base: Base {
                seqnum: self.seqnum,
                devid: 0x0001_0002,
                direction,
                ep,
            },
            flags: 0,
            length,
            start_frame: 0,
            packets: if iso.is_empty() {
                wire::NOT_ISO
            } else {
                iso.len() as u32
            },
            interval: 1,
            setup,
        };
        let mut message = header.encode().to_vec();
        message.extend_from_slice(data);
        for packet in iso {
            message.extend_from_slice(&packet.encode());
        }
        self.stream.write_all(&message).unwrap();
        self.seqnum
    }

    pub(crate) fn unlink(&mut self, target: u32) -> u32 {
        self.seqnum += 1;
        let header = Header::Unlink {
            base: Base {
                seqnum: self.seqnum,
                devid: 0x0001_0002,
                direction: 0,
                ep: 0,
            },
            unlink: target,
        };
        self.stream.write_all(&header.encode()).unwrap();
        self.seqnum
    }

    /// The next reply, its IN data and its packet descriptors. `inbound` says whether the
    /// transfer it answers is IN, as the driver knows from the seqnum.
    pub(crate) fn reply(&mut self, inbound: bool) -> (Header, Vec<u8>, Vec<IsoPacket>) {
        let mut head = [0; wire::HEADER_BYTES];
        self.stream.read_exact(&mut head).unwrap();
        let header = Header::decode(&head).unwrap();
        let (data, packets) = match header {
            Header::RetSubmit {
                length, packets, ..
            } => (if inbound { length } else { 0 }, wire::iso_packets(packets)),
            _ => (0, 0),
        };
        let mut body = vec![0; data as usize + packets as usize * wire::ISO_PACKET_BYTES];
        self.stream.read_exact(&mut body).unwrap();
        let iso = wire::iso_descriptors(&body, packets).unwrap().collect();
        body.truncate(data as usize);
        (header, body, iso)
    }

    /// Whether the client closed the tunnel.
    pub(crate) fn closed(&mut self) -> bool {
        let mut byte = [0];
        matches!(self.stream.read(&mut byte), Ok(0) | Err(_))
    }
}

fn status_of(header: &Header) -> (u32, i32) {
    match *header {
        Header::RetSubmit { base, status, .. } | Header::RetUnlink { base, status } => {
            (base.seqnum, status)
        }
        other => panic!("not a reply: {other:?}"),
    }
}

fn serving(device: FakeDevice) -> (Driver, std::thread::JoinHandle<Result<(), ServeError>>) {
    let (tunnel, driver) = pipe();
    let served = std::thread::spawn(move || serve(tunnel, Arc::new(Blocking::new(device))));
    (driver, served)
}

#[test]
fn a_served_device_imports_and_carries_control_bulk_and_interrupt_transfers() {
    let device = FakeDevice::new("1-2");
    let (mut driver, served) = serving(device.clone());
    let described = driver.import("1-2").unwrap();
    assert_eq!(
        (
            described.busid.as_str(),
            described.vendor,
            described.product,
            described.speed
        ),
        ("1-2", fake::VENDOR, fake::PRODUCT, 3)
    );

    let get = driver.submit(0, wire::DIR_IN, 64, setup(0x80, 6, 0x0100, 64), &[], &[]);
    let (header, data, _) = driver.reply(true);
    assert_eq!(status_of(&header), (get, 0));
    assert_eq!(data, fake::DEVICE_DESCRIPTOR);
    let set = driver.submit(0, wire::DIR_OUT, 0, setup(0x00, 9, 1, 0), &[], &[]);
    assert_eq!(status_of(&driver.reply(false).0), (set, 0));
    assert_eq!(device.configuration(), 1);
    // A request it does not know stalls.
    let odd = driver.submit(0, wire::DIR_IN, 8, setup(0xc0, 0x77, 0, 8), &[], &[]);
    assert_eq!(status_of(&driver.reply(true).0), (odd, wire::EPIPE));

    // An interrupt IN waits for a report while bulk goes on beside it.
    let report = driver.submit(fake::INTERRUPT as u32, wire::DIR_IN, 8, [0; 8], &[], &[]);
    let block: Vec<u8> = (0..100_000u32).map(|at| at as u8).collect();
    let out = driver.submit(
        fake::BULK as u32,
        wire::DIR_OUT,
        block.len() as u32,
        [0; 8],
        &block,
        &[],
    );
    let (header, data, _) = driver.reply(false);
    assert_eq!(status_of(&header), (out, 0));
    assert!(data.is_empty());
    assert!(matches!(
        header,
        Header::RetSubmit {
            length: 100_000,
            ..
        }
    ));
    let back = driver.submit(fake::BULK as u32, wire::DIR_IN, 200_000, [0; 8], &[], &[]);
    let (header, data, _) = driver.reply(true);
    assert_eq!(status_of(&header), (back, 0));
    assert!(data == block, "the loopback reads back what went out");
    device.push_report(vec![1, 2, 3]);
    let (header, data, _) = driver.reply(true);
    assert_eq!((status_of(&header), data), ((report, 0), vec![1, 2, 3]));

    drop(driver);
    assert!(
        served.join().unwrap().is_ok(),
        "the host closing is a clean end"
    );
    assert_eq!(device.closes(), 1, "and lets go of the device");
}

#[test]
fn an_unlinked_transfer_is_answered_once_and_never_completes() {
    let device = FakeDevice::new("1-2");
    let (mut driver, served) = serving(device.clone());
    driver.import("1-2").unwrap();
    let waiting = driver.submit(fake::INTERRUPT as u32, wire::DIR_IN, 8, [0; 8], &[], &[]);
    // It runs, waiting for a report, when the unlink comes.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while device.seen().is_empty() {
        assert!(
            std::time::Instant::now() < deadline,
            "the transfer never ran"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    let unlink = driver.unlink(waiting);
    assert_eq!(
        status_of(&driver.reply(false).0),
        (unlink, wire::ECONNRESET)
    );
    // The report it would have read goes to the next one, and nothing answers the first.
    device.push_report(vec![9]);
    let next = driver.submit(fake::INTERRUPT as u32, wire::DIR_IN, 8, [0; 8], &[], &[]);
    let (header, data, _) = driver.reply(true);
    assert_eq!((status_of(&header), data), ((next, 0), vec![9]));
    // An unlink after the answer cancels nothing.
    let late = driver.unlink(next);
    assert_eq!(status_of(&driver.reply(false).0), (late, 0));
    drop(driver);
    served.join().unwrap().unwrap();
}

#[test]
fn isochronous_in_comes_packed_with_its_descriptors_and_failures_keep_them() {
    let device = FakeDevice::new("1-2");
    let (mut driver, served) = serving(device.clone());
    driver.import("1-2").unwrap();
    let asked: Vec<IsoPacket> = (0..3)
        .map(|at| IsoPacket {
            offset: at * 64,
            length: 64,
            actual_length: 0,
            status: 0,
        })
        .collect();
    let iso = driver.submit(
        fake::ISOCHRONOUS as u32,
        wire::DIR_IN,
        192,
        [0; 8],
        &[],
        &asked,
    );
    let (header, data, packets) = driver.reply(true);
    assert_eq!(status_of(&header), (iso, 0));
    assert!(matches!(
        header,
        Header::RetSubmit {
            length: 12,
            packets: 3,
            ..
        }
    ));
    assert_eq!(data, [0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2]);
    assert!(packets
        .iter()
        .zip(&asked)
        .all(|(got, asked)| got.offset == asked.offset && got.actual_length == 4));

    device.unplug();
    let gone = driver.submit(
        fake::ISOCHRONOUS as u32,
        wire::DIR_IN,
        192,
        [0; 8],
        &[],
        &asked,
    );
    let (header, data, packets) = driver.reply(true);
    assert_eq!(status_of(&header), (gone, wire::ENODEV));
    assert!(data.is_empty());
    assert_eq!(
        packets.len(),
        3,
        "a failure keeps the packets the driver reads"
    );
    assert!(packets
        .iter()
        .all(|p| p.actual_length == 0 && p.status == wire::ENODEV));
    drop(driver);
    served.join().unwrap().unwrap();
}

#[test]
fn another_devices_import_is_refused() {
    let device = FakeDevice::new("1-2");
    let (mut driver, served) = serving(device.clone());
    assert_eq!(driver.import("1-3").err(), Some(1));
    assert!(matches!(
        served.join().unwrap(),
        Err(ServeError::Protocol(_))
    ));
    assert_eq!(device.closes(), 1);
}

/// Answers every transfer with more than it asked for.
struct Greedy;
impl UsbDevice for Greedy {
    fn describe(&self) -> wire::Device {
        wire::Device {
            busid: "1-1".into(),
            ..wire::Device::default()
        }
    }
    fn submit(&self, transfer: Transfer, done: Completion) {
        done.complete(Reply::read(vec![0; transfer.length as usize + 1]));
    }
    fn unlink(&self, _: u32) {}
    fn close(&self) {}
}

#[test]
fn a_reply_that_does_not_fit_its_transfer_closes_the_tunnel() {
    let (tunnel, mut driver) = pipe();
    let served = std::thread::spawn(move || serve(tunnel, Arc::new(Greedy)));
    driver.import("1-1").unwrap();
    driver.submit(1, wire::DIR_IN, 8, [0; 8], &[], &[]);
    assert!(
        driver.closed(),
        "nothing that does not fit reaches the driver"
    );
    assert!(matches!(served.join().unwrap(), Err(ServeError::Device(_))));
}

#[test]
fn a_dropped_completion_fails_its_transfer() {
    struct Forgetful;
    impl UsbDevice for Forgetful {
        fn describe(&self) -> wire::Device {
            wire::Device {
                busid: "1-1".into(),
                ..wire::Device::default()
            }
        }
        fn submit(&self, _: Transfer, done: Completion) {
            drop(done);
        }
        fn unlink(&self, _: u32) {}
        fn close(&self) {}
    }
    let (tunnel, mut driver) = pipe();
    let served = std::thread::spawn(move || serve(tunnel, Arc::new(Forgetful)));
    driver.import("1-1").unwrap();
    let lost = driver.submit(1, wire::DIR_IN, 8, [0; 8], &[], &[]);
    assert_eq!(status_of(&driver.reply(true).0), (lost, wire::ESHUTDOWN));
    drop(driver);
    served.join().unwrap().unwrap();
}
