use super::*;
use crate::steam_deck::{REPORT_PERIOD, SteamDeck};

#[test]
fn the_start_hook_runs_first_on_both_server_threads() {
    static STARTED: Mutex<Vec<String>> = Mutex::new(Vec::new());
    fn record() -> Box<dyn std::any::Any> {
        let name = std::thread::current().name().unwrap_or_default().to_owned();
        STARTED.lock().unwrap().push(name);
        Box::new(())
    }
    let export = Export::listen_started(SteamDeck::new("TEST"), REPORT_PERIOD, record).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while STARTED.lock().unwrap().len() < 2 {
        assert!(
            Instant::now() < deadline,
            "a server thread skipped the hook"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    drop(export);
    let mut started = STARTED.lock().unwrap().clone();
    started.sort();
    assert_eq!(started, ["usbip-accept", "usbip-reports"]);
}

#[test]
fn dropping_an_idle_export_does_not_need_a_loopback_connection() {
    for idle in [Duration::ZERO, READ_SLICE * 2] {
        let mut export = Export::listen(SteamDeck::new("TEST"), REPORT_PERIOD).unwrap();
        // An unreachable advertised port makes the old self-connect wake-up
        // fail on unrestricted machines too. Shutdown must use no connection.
        export.port = 0;
        std::thread::sleep(idle);
        let (done, finished) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            drop(export);
            done.send(()).unwrap();
        });
        finished.recv_timeout(Duration::from_secs(2)).unwrap();
        worker.join().unwrap();
    }
}

#[test]
fn composite_audio_round_trips_packet_descriptors_and_actuator_pcm() {
    let mut pad = crate::dualsense::DualSense::new(0, 3);
    pad.capabilities(crate::haptics::CAPABILITY);
    // A loaded machine can wake the report thread more than `MAX_AGE` late, and `listen`
    // then drops the audio as stale. This test is about the descriptors and the PCM.
    let export =
        Export::listen_with(pad, REPORT_PERIOD, Duration::from_secs(60), || Box::new(())).unwrap();
    let mut client = TcpStream::connect((Ipv4Addr::LOCALHOST, export.port())).unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut request = op_reply(OP_REQ_IMPORT, 0);
    request.extend_from_slice(b"1-1");
    request.resize(40, 0);
    client.write_all(&request).unwrap();
    let mut imported = [0; 320];
    client.read_exact(&mut imported).unwrap();
    assert_eq!(be32(&imported, 8 + 296), 3, "high speed");
    assert_eq!(&imported[8 + 300..8 + 304], &[5, 0x4c, 0x0c, 0xe6]);
    // Five 1 ms packets with deliberate padding between packets. USB/IP
    // offsets describe the transfer buffer, not concatenated audio.
    let packet_size = 384;
    let stride = 392;
    let length = stride * 5;
    let mut wire = vec![];
    for n in [CMD_SUBMIT, 7, 0x0001_0002, 0, 1, 0, length, 0, 5, 1] {
        wire.extend_from_slice(&n.to_be_bytes());
    }
    wire.extend_from_slice(&[0; 8]);
    let mut audio = vec![0xcc; length as usize];
    for packet in 0..5 {
        for frame in audio[packet * stride as usize..packet * stride as usize + packet_size]
            .as_chunks_mut::<8>()
            .0
        {
            frame.copy_from_slice(&[0x11, 0x22, 0x33, 0x44, 1, 0x80, 2, 0x7f]);
        }
    }
    wire.extend_from_slice(&audio);
    for p in 0..5 {
        for n in [p * stride, packet_size as u32, 0, 0] {
            wire.extend_from_slice(&n.to_be_bytes());
        }
    }
    // The export stamps the PCM after this; draining as of now keeps a
    // loaded test machine's delay out of the queue's 20 ms freshness limit.
    let submitted = std::time::Instant::now();
    client.write_all(&wire).unwrap();
    let mut reply = [0; 48 + 5 * 16];
    client.read_exact(&mut reply).unwrap();
    assert_eq!(be32(&reply, 0), RET_SUBMIT);
    assert_eq!(be32(&reply, 4), 7);
    assert_eq!(be32(&reply, 20), 0);
    assert_eq!(be32(&reply, 24), length);
    assert_eq!(be32(&reply, 32), 5);
    for (i, descriptor) in reply[48..].as_chunks::<16>().0.iter().enumerate() {
        assert_eq!(be32(descriptor, 0), i as u32 * stride);
        assert_eq!(be32(descriptor, 8), packet_size as u32);
        assert_eq!(be32(descriptor, 12), 0);
    }
    let frames = export.with(|pad| pad.take_pcm_at(submitted));
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].samples, [1, 0x80, 2, 0x7f].repeat(240));
    // The next HID control request still parses after isochronous metadata.
    submit(
        &mut client,
        8,
        0,
        true,
        3,
        [0xa1, 1, 0x85, 3, 3, 0, 3, 0],
        &[],
    );
    assert_eq!(
        self::reply(&mut client),
        (RET_SUBMIT, 8, 0, vec![0x85, 0, 0])
    );
}

fn import(port: u16) -> TcpStream {
    let mut client = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut request = op_reply(OP_REQ_IMPORT, 0);
    let mut busid = [0u8; 32];
    busid[..3].copy_from_slice(b"1-1");
    request.extend_from_slice(&busid);
    client.write_all(&request).unwrap();
    let mut reply = [0u8; 8 + 312];
    client.read_exact(&mut reply).unwrap();
    assert_eq!(&reply[..8], &op_reply(0x0003, 0)[..]);
    // idVendor and idProduct, big-endian, after path, busid and three u32s.
    assert_eq!(&reply[8 + 300..8 + 304], &[0x28, 0xde, 0x12, 0x05]);
    assert_eq!(reply[8 + 311], 3, "three interfaces");
    client
}
fn submit(
    client: &mut TcpStream,
    seqnum: u32,
    endpoint: u32,
    inward: bool,
    length: u32,
    setup: [u8; 8],
    data: &[u8],
) {
    let mut b = vec![];
    for n in [
        CMD_SUBMIT,
        seqnum,
        0x0001_0002,
        u32::from(inward),
        endpoint,
        0,
        length,
        0,
        0,
        0,
    ] {
        b.extend_from_slice(&n.to_be_bytes());
    }
    b.extend_from_slice(&setup);
    b.extend_from_slice(data);
    client.write_all(&b).unwrap();
}
/// A reply's command, seqnum, status and data.
fn reply(client: &mut TcpStream) -> (u32, u32, i32, Vec<u8>) {
    let mut h = [0u8; 48];
    client.read_exact(&mut h).unwrap();
    let command = be32(&h, 0);
    let length = if command == RET_SUBMIT {
        be32(&h, 24) as usize
    } else {
        0
    };
    let mut data = vec![0; length];
    // Only IN replies carry data; these tests only ask for IN data.
    client.read_exact(&mut data).unwrap();
    (command, be32(&h, 4), be32(&h, 20) as i32, data)
}

#[test]
fn a_client_imports_the_deck_reads_its_descriptors_and_gets_reports() {
    let export = Export::listen(SteamDeck::new("TEST"), REPORT_PERIOD).unwrap();
    let mut client = import(export.port());
    // GET_DESCRIPTOR(device), 18 bytes.
    submit(
        &mut client,
        1,
        0,
        true,
        18,
        [0x80, 6, 0, 1, 0, 0, 18, 0],
        &[],
    );
    let (command, seqnum, status, data) = reply(&mut client);
    assert_eq!((command, seqnum, status), (RET_SUBMIT, 1, 0));
    assert_eq!(&data[8..12], &[0xde, 0x28, 0x05, 0x12]);
    // A device qualifier stalls.
    submit(
        &mut client,
        2,
        0,
        true,
        10,
        [0x80, 6, 0, 6, 0, 0, 10, 0],
        &[],
    );
    assert_eq!(reply(&mut client).2, EPIPE);
    // SET_REPORT carries data out and returns none.
    let mut ask = [0u8; 64];
    ask[0] = 0xae;
    submit(
        &mut client,
        3,
        0,
        false,
        64,
        [0x21, 9, 0, 3, 2, 0, 64, 0],
        &ask,
    );
    let mut h = [0u8; 48];
    client.read_exact(&mut h).unwrap();
    assert_eq!((be32(&h, 4), be32(&h, 20), be32(&h, 24)), (3, 0, 64));
    assert!(export.imported());
    assert!(!export.polled(), "control requests are not reading reports");

    // The mouse endpoint never reports; the controller reports on its own.
    submit(&mut client, 4, 1, true, 8, [0; 8], &[]);
    submit(&mut client, 5, 3, true, 64, [0; 8], &[]);
    let (_, seqnum, status, data) = reply(&mut client);
    assert_eq!((seqnum, status, data.len()), (5, 0, 64));
    assert!(export.polled(), "an interrupt IN transfer was asked for");
    assert_eq!(&data[..4], &[1, 0, 9, 64]);

    // A change goes out at once.
    submit(&mut client, 6, 3, true, 64, [0; 8], &[]);
    let start = Instant::now();
    export.with(|deck| {
        deck.state.apply(&crate::input::Input::Controller {
            id: 0,
            active: 1,
            buttons: 0x1000,
            left_trigger: 0,
            right_trigger: 0,
            sticks: [0; 4],
        })
    });
    export.changed();
    let (_, seqnum, _, data) = reply(&mut client);
    assert_eq!(seqnum, 6);
    assert!(start.elapsed() < Duration::from_secs(1));
    // A periodic report may have raced the change; the next has it.
    let pressed = |data: &[u8]| data[8] & 0x80 != 0;
    if !pressed(&data) {
        submit(&mut client, 7, 3, true, 64, [0; 8], &[]);
        assert!(pressed(&reply(&mut client).3));
    }

    // Unlinking the held mouse transfer cancels it; an unknown one is gone already.
    let mut unlink = vec![];
    for n in [CMD_UNLINK, 8, 0x0001_0002, 0, 1, 4] {
        unlink.extend_from_slice(&n.to_be_bytes());
    }
    unlink.resize(48, 0);
    client.write_all(&unlink).unwrap();
    assert_eq!(reply(&mut client), (RET_UNLINK, 8, ECONNRESET, vec![]));
    unlink[20..24].copy_from_slice(&99u32.to_be_bytes());
    client.write_all(&unlink).unwrap();
    assert_eq!(reply(&mut client).2, 0);

    // Dropping the export disconnects the client promptly, though the
    // client keeps its end open.
    let start = Instant::now();
    drop(export);
    assert!(start.elapsed() < Duration::from_secs(2));
    let mut rest = vec![];
    match client.read_to_end(&mut rest) {
        Ok(_) => assert!(rest.is_empty()),
        Err(error) => assert!(!matches!(
            error.kind(),
            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
        )),
    }
}

#[test]
fn the_device_can_be_listed_and_attached_again_after_a_disconnect() {
    let export = Export::listen(SteamDeck::new("TEST"), REPORT_PERIOD).unwrap();
    let mut list = TcpStream::connect((Ipv4Addr::LOCALHOST, export.port())).unwrap();
    list.write_all(&op_reply(OP_REQ_DEVLIST, 0)).unwrap();
    let mut reply = vec![];
    list.read_to_end(&mut reply).unwrap();
    assert_eq!(reply.len(), 8 + 4 + 312 + 3 * 4);
    assert_eq!(&reply[8..12], &1u32.to_be_bytes());

    let first = import(export.port());
    drop(first);
    let mut second = import(export.port());
    submit(&mut second, 1, 3, true, 64, [0; 8], &[]);
    assert_eq!(reply_len(&mut second), 64);

    // A wrong bus ID is refused.
    let mut wrong = TcpStream::connect((Ipv4Addr::LOCALHOST, export.port())).unwrap();
    let mut request = op_reply(OP_REQ_IMPORT, 0);
    request.extend_from_slice(&[b'9'; 32]);
    wrong.write_all(&request).unwrap();
}
fn reply_len(client: &mut TcpStream) -> usize {
    reply(client).3.len()
}

/// A bulk IN transfer waiting before its command is answered once the
/// command arrives, after the command's own completion, and the held
/// HID transfer starts reporting once the command starts the reports.
#[test]
fn a_held_bulk_read_gets_the_answer_to_the_command_sent_after_it() {
    use crate::switch2::{REPORT_PERIOD as PERIOD, Switch2Pro};
    let export = Export::listen(Switch2Pro::new("TEST"), PERIOD).unwrap();
    let mut client = TcpStream::connect((Ipv4Addr::LOCALHOST, export.port())).unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut request = op_reply(OP_REQ_IMPORT, 0);
    let mut busid = [0u8; 32];
    busid[..3].copy_from_slice(b"1-1");
    request.extend_from_slice(&busid);
    client.write_all(&request).unwrap();
    let mut answer = [0u8; 8 + 312];
    client.read_exact(&mut answer).unwrap();
    assert_eq!(&answer[8 + 300..8 + 304], &[0x05, 0x7e, 0x20, 0x69]);
    assert_eq!(answer[8 + 311], 2, "two interfaces");
    // The HID report and the bulk reply wait: nothing is started yet.
    submit(&mut client, 1, 1, true, 64, [0; 8], &[]);
    submit(&mut client, 2, 2, true, 64, [0; 8], &[]);
    // Select report 0x05, then start the reports.
    let select = [
        0x03, 0x91, 0x00, 0x0a, 0x00, 0x04, 0x00, 0x00, 0x05, 0, 0, 0,
    ];
    submit(
        &mut client,
        3,
        2,
        false,
        select.len() as u32,
        [0; 8],
        &select,
    );
    // An OUT completion carries no data.
    let out_reply = |client: &mut TcpStream| {
        let mut h = [0u8; 48];
        client.read_exact(&mut h).unwrap();
        (be32(&h, 0), be32(&h, 4), be32(&h, 20) as i32)
    };
    assert_eq!(out_reply(&mut client), (RET_SUBMIT, 3, 0));
    let (_, seqnum, status, data) = reply(&mut client);
    assert_eq!((seqnum, status), (2, 0));
    assert_eq!(&data[..4], &[0x03, 0x01, 0x00, 0x0a]);
    let start = [
        0x03, 0x91, 0x00, 0x0d, 0x00, 0x08, 0x00, 0x00, 1, 0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
    ];
    submit(&mut client, 4, 2, false, start.len() as u32, [0; 8], &start);
    assert_eq!(out_reply(&mut client), (RET_SUBMIT, 4, 0));
    submit(&mut client, 5, 2, true, 64, [0; 8], &[]);
    let mut seen = std::collections::BTreeMap::new();
    for _ in 0..2 {
        let (_, seqnum, status, data) = reply(&mut client);
        assert_eq!(status, 0);
        seen.insert(seqnum, data);
    }
    assert_eq!(seen[&1][0], 0x05, "the held HID read reports");
    assert_eq!(&seen[&5][..4], &[0x03, 0x01, 0x00, 0x0d]);
}

/// A device that has a report only after a change, as a mouse or touchpad.
struct Quiet {
    report: Option<Vec<u8>>,
}
impl Device for Quiet {
    fn describe(&self) -> UsbDevice {
        UsbDevice {
            speed: 2,
            vendor: 0x1209,
            product: 1,
            release: 1,
            interfaces: vec![(3, 0, 2)],
        }
    }
    fn control(&mut self, _setup: &Setup, _data: &[u8]) -> Option<Vec<u8>> {
        Some(vec![])
    }
    fn interrupt(&mut self, _endpoint: u8) -> Option<Vec<u8>> {
        self.report.take()
    }
    fn interrupt_ready(&self, _endpoint: u8) -> bool {
        self.report.is_some()
    }
}

#[test]
fn an_event_driven_endpoint_is_naked_until_its_next_change() {
    let export = Export::listen(Quiet { report: None }, Duration::from_millis(1)).unwrap();
    let mut client = TcpStream::connect((Ipv4Addr::LOCALHOST, export.port())).unwrap();
    let mut request = op_reply(OP_REQ_IMPORT, 0);
    let mut busid = [0u8; 32];
    busid[..3].copy_from_slice(b"1-1");
    request.extend_from_slice(&busid);
    client.write_all(&request).unwrap();
    let mut imported = [0u8; 8 + 312];
    client.read_exact(&mut imported).unwrap();
    assert_eq!(&imported[..8], &op_reply(0x0003, 0)[..]);

    submit(&mut client, 1, 1, true, 8, [0; 8], &[]);
    // Many periods pass without a report: the poll is NAKed, not answered empty.
    client
        .set_read_timeout(Some(Duration::from_millis(50)))
        .unwrap();
    let mut byte = [0u8; 1];
    assert!(client.peek(&mut byte).is_err());

    export.with(|quiet| quiet.report = Some(vec![1, 2, 3]));
    export.changed();
    client
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    assert_eq!(reply(&mut client), (RET_SUBMIT, 1, 0, vec![1, 2, 3]));

    // The next poll waits again for the next change.
    submit(&mut client, 2, 1, true, 8, [0; 8], &[]);
    client
        .set_read_timeout(Some(Duration::from_millis(50)))
        .unwrap();
    assert!(client.peek(&mut byte).is_err());
    export.with(|quiet| quiet.report = Some(vec![4]));
    export.changed();
    client
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    assert_eq!(reply(&mut client), (RET_SUBMIT, 2, 0, vec![4]));
}
