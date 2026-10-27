//! USB forwarding between Nightjar and this host: Nightjar's forwarder serves its fake device
//! over the native API's TLS tunnel, the host relays it to a fake usbip-win2 driver, and the
//! test drives the driver's end as the kernel would. Attach requests, tokens, statuses and
//! detaches go over the stream's encrypted control channel.
use super::*;
use crate::usb_forward::tests::FakeVhci;
use butterpollo_core::usbip_wire::{self as wire, Base, Header};
use nightjar_core::{
    control::{ClientMessage, SendError},
    usb::{
        Blocking, Forwarder, Phase, UsbMessage, UsbState,
        fake::{self, FakeDevice},
    },
};
use std::{
    io::{Read, Write},
    net::TcpStream,
    sync::mpsc,
};

/// The driver's end of the relay once it imported the device.
struct Driver {
    stream: TcpStream,
    seqnum: u32,
}
impl Driver {
    fn submit(
        &mut self,
        ep: u8,
        direction: u32,
        length: u32,
        setup: [u8; 8],
        data: &[u8],
    ) -> Result<u32> {
        self.seqnum += 1;
        let header = Header::Submit {
            base: Base {
                seqnum: self.seqnum,
                devid: 0x0001_0002,
                direction,
                ep: ep.into(),
            },
            flags: 0,
            length,
            start_frame: 0,
            packets: wire::NOT_ISO,
            interval: 1,
            setup,
        };
        let mut message = header.encode().to_vec();
        message.extend_from_slice(data);
        self.stream.write_all(&message)?;
        Ok(self.seqnum)
    }

    fn unlink(&mut self, target: u32) -> Result<u32> {
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
        self.stream.write_all(&header.encode())?;
        Ok(self.seqnum)
    }

    /// The next reply's seqnum, status and IN data. `inbound` says whether it answers an IN
    /// transfer, as the driver knows from the seqnum.
    fn reply(&mut self, inbound: bool) -> Result<(u32, i32, Vec<u8>)> {
        let mut head = [0; wire::HEADER_BYTES];
        self.stream.read_exact(&mut head)?;
        let (seqnum, status, length) = match Header::decode(&head).context("a USB/IP header")? {
            Header::RetSubmit {
                base,
                status,
                length,
                ..
            } => (base.seqnum, status, if inbound { length } else { 0 }),
            Header::RetUnlink { base, status } => (base.seqnum, status, 0),
            other => anyhow::bail!("not a reply: {other:?}"),
        };
        let mut data = vec![0; length as usize];
        self.stream.read_exact(&mut data)?;
        Ok((seqnum, status, data))
    }

    /// Whether the host closed the driver's connection.
    fn closed(&mut self) -> bool {
        matches!(self.stream.read(&mut [0]), Ok(0) | Err(_))
    }
}

fn setup(request_type: u8, request: u8, value: u16, length: u16) -> [u8; 8] {
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

/// Nightjar's forwarder for the stream in `live`, given the host's USB messages as a platform
/// gives them: through its listener.
fn forwarder(
    client: &Arc<HostClient>,
    live: &Arc<Mutex<Option<Stream>>>,
    recorder: &Recorder,
) -> Arc<Forwarder> {
    let live = live.clone();
    let forwarder = Arc::new(Forwarder::for_host(
        client.clone(),
        move |message: ClientMessage| match &*live.lock().unwrap() {
            Some(stream) => stream.send(message),
            None => Err(SendError::Stopped),
        },
        || {},
    ));
    recorder.0.lock().unwrap().usb = Some(forwarder.clone());
    forwarder
}

/// The user picks `device`: Nightjar asks, opens the tunnel with the host's token and serves
/// it; the driver imports it. Its request id and the driver's end.
async fn attach(
    forwarder: &Forwarder,
    connections: &mpsc::Receiver<TcpStream>,
    device: &FakeDevice,
) -> Result<(u32, Driver)> {
    let id = forwarder.attach(Arc::new(Blocking::new(device.clone())), "Fake")?;
    let stream = connections.recv_timeout(LOADED)?;
    stream.set_read_timeout(Some(LOADED))?;
    until("the attachment at Nightjar", LOADED, || {
        forwarder
            .list()
            .iter()
            .any(|a| a.request_id == id && a.phase == Phase::Attached)
    })
    .await;
    Ok((id, Driver { stream, seqnum: 0 }))
}

/// Waits until the host let go of everything, the driver's connection closed and the device
/// closed at Nightjar.
async fn let_go(h: &Harness, driver: &mut Driver, device: &FakeDevice) {
    until("the host's list to empty", LOADED, || {
        h.h.usb.list().is_empty()
    })
    .await;
    assert!(driver.closed(), "the relay closed the driver's connection");
    until("Nightjar to let go of the device", LOADED, || {
        device.closes() == 1
    })
    .await;
}

/// Nightjar forwards a device the user picked over real TLS: control, bulk (past the 64 KiB a
/// TLS record holds) and interrupt transfers and an unlink go through the relay. The
/// attachment ends when the user detaches it, when Nightjar's tunnel closes while the stream
/// goes on, and with the stream.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nightjar_forwards_a_usb_device_and_every_ending_detaches_it() -> Result<()> {
    let h = Harness::configured(10_000, 1, "usb_forwarding=enabled\n").await?;
    let (vhci, connections) = FakeVhci::new(true);
    h.h.usb.set_vhci(vhci.clone());
    let client = Arc::new(device(&h, u32::MAX, &[("allow_usb", true)])?);
    let (_launch, session, stream, recorder) = stream(&h, &client).await?;
    assert!(
        stream.outcome().host_control_messages.contains(&ptype::USB),
        "the host announces 0x553A"
    );
    let live = Arc::new(Mutex::new(Some(stream)));
    let usb = forwarder(&client, &live, &recorder);

    let first = FakeDevice::new("1-4");
    let (id, mut driver) = attach(&usb, &connections, &first).await?;
    let listed = h.h.usb.list();
    assert_eq!(
        (listed.len(), listed[0].state, listed[0].port),
        (1, "attached", Some(3))
    );
    assert_eq!(
        (listed[0].busid.as_str(), listed[0].vid, listed[0].pid),
        ("1-4", fake::VENDOR, fake::PRODUCT)
    );

    let get = driver.submit(0, wire::DIR_IN, 64, setup(0x80, 6, 0x0100, 64), &[])?;
    assert_eq!(
        driver.reply(true)?,
        (get, 0, fake::DEVICE_DESCRIPTOR.to_vec())
    );
    let set = driver.submit(0, wire::DIR_OUT, 0, setup(0x00, 9, 1, 0), &[])?;
    assert_eq!(driver.reply(false)?, (set, 0, Vec::new()));
    assert_eq!(first.configuration(), 1);

    let block: Vec<u8> = (0..100_000u32).map(|at| at as u8).collect();
    let out = driver.submit(
        fake::BULK,
        wire::DIR_OUT,
        block.len() as u32,
        [0; 8],
        &block,
    )?;
    assert_eq!(driver.reply(false)?, (out, 0, Vec::new()));
    let back = driver.submit(fake::BULK, wire::DIR_IN, 200_000, [0; 8], &[])?;
    let (seqnum, status, data) = driver.reply(true)?;
    assert_eq!((seqnum, status), (back, 0));
    assert!(data == block, "the bulk loopback reads back what went out");

    let waiting = driver.submit(fake::INTERRUPT, wire::DIR_IN, 8, [0; 8], &[])?;
    until("the interrupt transfer at the device", LOADED, || {
        first.seen().iter().any(|&(ep, _, _)| ep == fake::INTERRUPT)
    })
    .await;
    let unlink = driver.unlink(waiting)?;
    assert_eq!(driver.reply(false)?, (unlink, wire::ECONNRESET, Vec::new()));
    let report = driver.submit(fake::INTERRUPT, wire::DIR_IN, 8, [0; 8], &[])?;
    first.push_report(vec![1, 2, 3]);
    assert_eq!(driver.reply(true)?, (report, 0, vec![1, 2, 3]));

    // The user detaches it.
    assert!(usb.detach(id));
    let_go(&h, &mut driver, &first).await;
    assert_eq!(vhci.detached(), [(3, "1-4".to_owned())]);

    // Nightjar closes its tunnel without a word on the control stream: the host detaches the
    // device and the stream goes on.
    let second = FakeDevice::new("1-5");
    let (id, mut driver) = attach(&usb, &connections, &second).await?;
    usb.end();
    let_go(&h, &mut driver, &second).await;
    assert_eq!(vhci.detached()[1..], [(3, "1-5".to_owned())]);
    until("the host's status for the dropped tunnel", LOADED, || {
        recorder.saw(|e| matches!(e, ControlEvent::Usb(UsbMessage::Status { request_id, .. }) if *request_id == id))
    })
    .await;
    assert!(!session.stopping());

    // The stream ends with a device attached: the host detaches it.
    let usb = forwarder(&client, &live, &recorder);
    let third = FakeDevice::new("1-6");
    let (id, mut driver) = attach(&usb, &connections, &third).await?;
    let stream = live.lock().unwrap().take().unwrap();
    stream.stop();
    let_go(&h, &mut driver, &third).await;
    assert_eq!(vhci.detached()[2..], [(3, "1-6".to_owned())]);
    usb.end();
    assert!(
        !usb.list()
            .iter()
            .any(|a| a.request_id == id && a.is_active())
    );
    // Nightjar ends its tunnels with a TLS close_notify: none of it was a failure to the host.
    assert!(!recorder.saw(|e| matches!(
        e,
        ControlEvent::Usb(UsbMessage::Status {
            state: UsbState::Failed,
            ..
        })
    )));
    Ok(())
}
