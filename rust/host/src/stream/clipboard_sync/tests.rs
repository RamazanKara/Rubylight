use super::*;
use std::sync::LazyLock;

/// An in-memory clipboard; a write raises its count as Windows does.
#[derive(Clone, Default)]
pub(crate) struct Fake(pub(crate) Arc<Mutex<FakeState>>);
#[derive(Default)]
pub(crate) struct FakeState {
    pub sequence: u32,
    pub text: String,
    pub png: Option<Vec<u8>>,
}
impl Fake {
    /// Copies `text` as a program on the PC would.
    pub(crate) fn copy(&self, text: &str) {
        let mut state = self.0.lock().unwrap();
        state.sequence += 1;
        state.text = text.into();
        state.png = None;
    }
    pub(crate) fn text(&self) -> String {
        self.0.lock().unwrap().text.clone()
    }
}
impl Clipboard for Fake {
    fn sequence(&self) -> u32 {
        self.0.lock().unwrap().sequence
    }
    fn read(&self) -> Result<String> {
        Ok(self.0.lock().unwrap().text.clone())
    }
    fn read_png(&self, max_bytes: usize) -> Result<Option<Vec<u8>>> {
        match self.0.lock().unwrap().png.clone() {
            Some(png) if png.len() > max_bytes => Err(ImageError::TooManyBytes {
                bytes: png.len(),
                max: max_bytes,
            }
            .into()),
            png => Ok(png),
        }
    }
    fn write(&self, text: &str) -> Result<()> {
        let mut state = self.0.lock().unwrap();
        state.sequence += 1;
        state.text = text.into();
        state.png = None;
        Ok(())
    }
    fn write_png(&self, png: &[u8]) -> Result<()> {
        let mut state = self.0.lock().unwrap();
        state.sequence += 1;
        state.text.clear();
        state.png = Some(png.to_vec());
        Ok(())
    }
}
/// The clipboard streams in tests use.
pub(crate) static FAKE: LazyLock<Fake> = LazyLock::new(Fake::default);

const ALL: Allowed = Allowed {
    write: true,
    read: true,
    png: Some(MAX_PAYLOAD_BYTES),
};

fn text(token: u32, text: &str) -> Vec<u8> {
    Item::Text(text.into()).frame(token).encode().unwrap()
}

fn client(perm: u32) -> Client {
    Client {
        name: "phone".into(),
        cert: String::new(),
        uuid: "1".into(),
        perm,
        enabled: true,
        extra: Default::default(),
    }
}

#[test]
fn the_clipboard_permissions_decide_each_way() {
    let config = Config::default();
    let view = 1 << 25;
    assert_eq!(
        allowed(&config, &client(view | WRITE | READ)),
        Allowed {
            write: true,
            read: true,
            png: Some(MAX_PAYLOAD_BYTES)
        }
    );
    let write_only = allowed(&config, &client(view | WRITE));
    assert!(write_only.write && !write_only.read);
    // Without view access, or playing in the background, nothing.
    assert!(!allowed(&config, &client(WRITE | READ)).any());
    let background = Config::parse("background_stream = true\n").unwrap();
    assert!(!allowed(&background, &client(u32::MAX)).any());
    let mut disabled = client(u32::MAX);
    disabled.enabled = false;
    assert!(!allowed(&config, &disabled).any());
    // Images follow clipboard_image and its limit.
    let images = Config::parse("clipboard_image = false\n").unwrap();
    assert_eq!(allowed(&images, &client(u32::MAX)).png, None);
    let small = Config::parse("clipboard_image_max_mb = 1\n").unwrap();
    assert_eq!(
        allowed(&small, &client(u32::MAX)).png,
        Some(MAX_PAYLOAD_BYTES)
    );
}

#[test]
fn frames_are_refused_by_permission_kind_and_size() {
    let mut sync = PeerSync::default();
    let now = Instant::now();
    let png = |len| Item::Png(vec![0x89; len]).frame(9).encode().unwrap();
    assert_eq!(
        sync.accept(&text(1, "hi"), &ALL, now),
        Accepted::Write(Item::Text("hi".into()))
    );
    assert_eq!(sync.accept(&text(1, ""), &ALL, now), Accepted::Nothing);
    assert_eq!(sync.accept(&[1, 1, 0], &ALL, now), Accepted::Invalid);
    let read_only = Allowed {
        write: false,
        ..ALL
    };
    // A device that may only read starts with a hello; its copies stay off the PC.
    assert_eq!(
        sync.accept(&text(1, ""), &read_only, now),
        Accepted::Nothing
    );
    assert_eq!(
        sync.accept(&text(1, "hi"), &read_only, now),
        Accepted::Refused(Refused::Write)
    );
    assert_eq!(
        sync.accept(&text(1, ""), &Allowed::default(), now),
        Accepted::Refused(Refused::Nothing)
    );
    assert_eq!(
        sync.accept(&text(1, "a\0b"), &ALL, now),
        Accepted::Refused(Refused::Text)
    );
    let mut latin1 = text(1, "x");
    latin1[10] = 0xff;
    assert_eq!(
        sync.accept(&latin1, &ALL, now),
        Accepted::Refused(Refused::Text)
    );
    assert_eq!(
        sync.accept(&png(4), &ALL, now),
        Accepted::Write(Item::Png(vec![0x89; 4]))
    );
    let small = Allowed {
        png: Some(3),
        ..ALL
    };
    assert_eq!(
        sync.accept(&png(4), &small, now),
        Accepted::Refused(Refused::TooLarge)
    );
    let no_images = Allowed { png: None, ..ALL };
    assert_eq!(
        sync.accept(&png(4), &no_images, now),
        Accepted::Refused(Refused::Images)
    );
    let mut reference = text(1, r#"{"type":"ref"}"#);
    reference[1] = KIND_REF;
    assert_eq!(
        sync.accept(&reference, &ALL, now),
        Accepted::Refused(Refused::Reference)
    );
    reference[1] = 7;
    assert_eq!(
        sync.accept(&reference, &ALL, now),
        Accepted::Refused(Refused::Kind(7))
    );
}

#[test]
fn a_copy_this_host_sent_coming_back_is_dropped_for_five_seconds() {
    let mut sync = PeerSync::default();
    let now = Instant::now();
    *sync.outbox.lock().unwrap() = Some(Item::Text("pc".into()).frame(42));
    assert_eq!(sync.take(now, || true).unwrap(), text(42, "pc"));
    assert_eq!(sync.take(now, || true), None);
    assert_eq!(sync.accept(&text(42, "pc"), &ALL, now), Accepted::Nothing);
    assert_eq!(
        sync.accept(&text(43, "pc"), &ALL, now),
        Accepted::Write(Item::Text("pc".into()))
    );
    assert_eq!(
        sync.accept(&text(42, "pc"), &ALL, now + ECHO),
        Accepted::Write(Item::Text("pc".into()))
    );
    // Only the newest tokens are kept.
    for token in 0..=ECHO_TOKENS as u32 {
        *sync.outbox.lock().unwrap() = Some(Item::Text("pc".into()).frame(100 + token));
        sync.take(now, || true);
    }
    assert_eq!(sync.sent.len(), ECHO_TOKENS);
    assert_eq!(sync.sent.front().unwrap().0, 101);
}

#[test]
fn a_copy_is_not_sent_once_the_device_may_no_longer_read_it() {
    let mut sync = PeerSync::default();
    let now = Instant::now();
    *sync.outbox.lock().unwrap() = Some(Item::Text("pc".into()).frame(7));
    // Made view only after the stream's first clipboard frame: asked again, refused.
    assert_eq!(sync.take(now, || false), None);
    assert!(sync.outbox.lock().unwrap().is_none(), "the copy is dropped");
    assert!(sync.sent.is_empty());
    *sync.outbox.lock().unwrap() = Some(Item::Text("again".into()).frame(8));
    assert_eq!(sync.take(now, || true).unwrap(), text(8, "again"));
}

#[test]
fn only_the_devices_newest_copy_waits_for_the_worker() {
    let items = Mailbox(Arc::default());
    let worker = items.0.clone();
    assert_eq!(worker.take(Duration::from_millis(10)), Ok(None));
    for n in 0..100 {
        items.0.put(Item::Text(format!("copy {n}")));
    }
    assert_eq!(
        worker.take(Duration::ZERO),
        Ok(Some(Item::Text("copy 99".into())))
    );
    assert_eq!(worker.take(Duration::ZERO), Ok(None));
    // A worker waiting for a copy gets it at once.
    let waiting = std::thread::spawn({
        let worker = worker.clone();
        move || worker.take(Duration::from_secs(10))
    });
    std::thread::sleep(Duration::from_millis(20));
    items.0.put(Item::Text("late".into()));
    assert_eq!(waiting.join().unwrap(), Ok(Some(Item::Text("late".into()))));
    // The peer going ends the worker once the copy waiting is written.
    items.0.put(Item::Text("last".into()));
    let waiting = std::thread::spawn({
        let worker = worker.clone();
        move || {
            (
                worker.take(Duration::from_secs(10)),
                worker.take(Duration::from_secs(10)),
            )
        }
    });
    drop(items);
    assert_eq!(
        waiting.join().unwrap(),
        (Ok(Some(Item::Text("last".into()))), Err(Closed))
    );
}

#[test]
fn the_watch_sends_only_the_pcs_new_copies() {
    let fake = Fake::default();
    fake.copy("before the stream");
    let mut watch = Watch::new(fake.clone(), ALL);
    assert_eq!(watch.poll().unwrap(), None);
    fake.copy("copied on the PC");
    let frame = watch.poll().unwrap().unwrap();
    assert_eq!(
        (frame.kind, frame.token, frame.payload.as_slice()),
        (KIND_TEXT, 2, &b"copied on the PC"[..])
    );
    assert_eq!(watch.poll().unwrap(), None);
    // The device's copy goes on the clipboard and is not sent back.
    watch.write(Item::Text("from the device".into())).unwrap();
    assert_eq!(fake.text(), "from the device");
    assert_eq!(watch.poll().unwrap(), None);
    // Nor is the same text copied again on the PC.
    fake.copy("from the device");
    assert_eq!(watch.poll().unwrap(), None);
    // An image goes while there is no text.
    fake.write_png(b"png").unwrap();
    assert_eq!(watch.poll().unwrap().unwrap().kind, KIND_PNG);
    // Too large for a frame: an error the worker warns about once.
    fake.copy(&"x".repeat(MAX_PAYLOAD_BYTES + 1));
    assert!(watch.poll().unwrap_err().downcast_ref::<Large>().is_some());
    let mut images = Watch::new(
        fake.clone(),
        Allowed {
            png: Some(2),
            ..ALL
        },
    );
    fake.write_png(b"png").unwrap();
    assert!(images.poll().unwrap_err().downcast_ref::<Large>().is_some());
    // A device that may only write gets nothing: each new copy is an error, unread.
    let mut write_only = Watch::new(fake.clone(), Allowed { read: false, ..ALL });
    assert_eq!(write_only.poll().unwrap(), None);
    fake.copy("secret");
    assert!(
        write_only
            .poll()
            .unwrap_err()
            .downcast_ref::<Unread>()
            .is_some()
    );
    assert_eq!(write_only.poll().unwrap(), None);
    // Its own copies still go on the clipboard, and are not taken for a copy of the PC's.
    write_only
        .write(Item::Text("from the device".into()))
        .unwrap();
    assert_eq!(fake.text(), "from the device");
    assert_eq!(write_only.poll().unwrap(), None);
}

#[test]
fn a_copy_kept_from_a_device_without_read_is_warned_on_the_stream_card_once() {
    let fake = Fake::default();
    let watch = Watch::new(fake.clone(), Allowed { read: false, ..ALL });
    let items = Mailbox(Arc::default());
    let worker = items.0.clone();
    let outbox = Arc::new(Mutex::new(None));
    let wake = Arc::new(UdpSocket::bind("127.0.0.1:0").unwrap());
    let warnings = Arc::new(Warnings::default());
    let thread = {
        let (outbox, warnings) = (outbox.clone(), warnings.clone());
        std::thread::spawn(move || run(watch, worker, outbox, wake, "phone", &warnings))
    };
    let card = || {
        warnings
            .snapshot()
            .into_iter()
            .filter(|w| w.code == WARNING)
            .map(|w| w.message)
            .collect::<Vec<_>>()
    };
    std::thread::sleep(POLL * 2);
    assert!(card().is_empty(), "no copy yet, no warning");
    fake.copy("copied on the PC");
    let deadline = Instant::now() + Duration::from_secs(5);
    while card().is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        card(),
        [
            "This PC's copies do not reach the device: it lacks the permission to read the clipboard here."
        ]
    );
    // Warned once: a cleared card stays clear on the next copy.
    warnings.clear(WARNING);
    fake.copy("another copy");
    std::thread::sleep(POLL * 3);
    assert!(card().is_empty());
    assert!(outbox.lock().unwrap().is_none(), "nothing was sent");
    drop(items);
    thread.join().unwrap();
}
