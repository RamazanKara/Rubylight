//! Relay-mode tests for the USB/IP server.

use super::*;

/// A device the kernel serves: [`UsbDevice::relay`] connects to `server`, counting connections.
struct Relayed {
    server: std::net::SocketAddr,
    connects: AtomicUsize,
    closes: AtomicUsize,
}
impl UsbDevice for Relayed {
    fn describe(&self) -> wire::Device {
        wire::Device {
            busid: "1-2".into(),
            vendor: 0x046d,
            ..wire::Device::default()
        }
    }
    fn submit(&self, _: Transfer, _: Completion) {
        panic!("a relayed device takes no transfers");
    }
    fn unlink(&self, _: u32) {}
    fn close(&self) {
        self.closes.fetch_add(1, Ordering::SeqCst);
    }
    fn relay(&self) -> Option<std::io::Result<Arc<dyn Tunnel>>> {
        self.connects.fetch_add(1, Ordering::SeqCst);
        Some(
            TcpStream::connect(self.server)
                .and_then(TcpTunnel::new)
                .map(|t| Arc::new(t) as Arc<dyn Tunnel>),
        )
    }
}

/// The kernel's end: echoes every byte until the client closes, and returns them.
fn kernel() -> (std::net::SocketAddr, std::thread::JoinHandle<Vec<u8>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut got = Vec::new();
        let mut buffer = [0; 1024];
        loop {
            match stream.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    got.extend_from_slice(&buffer[..n]);
                    stream.write_all(&buffer[..n]).unwrap();
                }
            }
        }
        got
    });
    (address, server)
}

fn relaying(
    server: std::net::SocketAddr,
) -> (
    Arc<Relayed>,
    Driver,
    std::thread::JoinHandle<Result<(), ServeError>>,
) {
    let device = Arc::new(Relayed {
        server,
        connects: AtomicUsize::new(0),
        closes: AtomicUsize::new(0),
    });
    let (tunnel, driver) = pipe();
    let served = {
        let device = device.clone();
        std::thread::spawn(move || serve(tunnel, device))
    };
    (device, driver, served)
}

#[test]
fn a_relayed_device_answers_the_import_itself_and_relays_every_byte_after() {
    let (address, server) = kernel();
    let (device, mut driver, served) = relaying(address);
    let imported = driver.import("1-2").unwrap();
    assert_eq!(imported.vendor, 0x046d, "the device as it describes itself");
    driver.stream.write_all(b"urb bytes").unwrap();
    let mut echoed = [0; 9];
    driver.stream.read_exact(&mut echoed).unwrap();
    assert_eq!(&echoed, b"urb bytes");
    drop(driver);
    served.join().unwrap().unwrap();
    assert_eq!(
        server.join().unwrap(),
        b"urb bytes",
        "the kernel sees only what follows the import"
    );
    assert_eq!(device.closes.load(Ordering::SeqCst), 1);
}

#[test]
fn a_relayed_import_of_another_device_never_reaches_the_kernel() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (device, mut driver, served) = relaying(listener.local_addr().unwrap());
    assert_eq!(driver.import("1-3").err(), Some(1));
    assert!(matches!(
        served.join().unwrap(),
        Err(ServeError::Protocol(_))
    ));
    assert_eq!(device.connects.load(Ordering::SeqCst), 0);
    assert_eq!(device.closes.load(Ordering::SeqCst), 1);
}

#[test]
fn a_relay_that_cannot_connect_refuses_the_import() {
    let address = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    let (device, mut driver, served) = relaying(address);
    assert_eq!(driver.import("1-2").err(), Some(1));
    assert!(matches!(served.join().unwrap(), Err(ServeError::Device(_))));
    assert_eq!(device.closes.load(Ordering::SeqCst), 1);
}

#[test]
fn the_kernel_closing_ends_the_relay_and_closes_the_tunnel() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    // The device went away: the kernel closes the connection at once.
    let server = std::thread::spawn(move || drop(listener.accept().unwrap()));
    let (device, mut driver, served) = relaying(address);
    driver.import("1-2").unwrap();
    server.join().unwrap();
    assert!(driver.closed(), "the host sees the tunnel close");
    served.join().unwrap().unwrap();
    assert_eq!(device.closes.load(Ordering::SeqCst), 1);
}
