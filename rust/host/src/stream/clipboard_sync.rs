//! Clipboard sync (0x5508), Moonlight V+'s message (`butterpollo_core::clipboard_sync`, after
//! https://github.com/qiin2333/moonlight-vplus, GPL-3.0): what the device copies goes on this
//! PC's clipboard, and what is copied here goes to the device, text and PNG images of up to
//! one control message each.
//!
//! The device's first frame starts its peer's worker: a Nightjar that asked
//! (`rlClipboardSync=1`) sends an empty hello, a V+ client, which never asks, its first copy.
//! The worker does the clipboard work off the control loop: it writes what arrived, and
//! watches the clipboard's change count for the PC's copies, which the loop sends. The
//! `/actions/clipboard` rules apply: "Write to the clipboard" for the device's copies, "Read
//! the clipboard" for the PC's, view access for either, images only while `clipboard_image`
//! is on, and nothing while the stream plays in the background.
use std::collections::VecDeque;
use std::net::UdpSocket;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use butterpollo_core::clipboard_image::ImageError;
use butterpollo_core::clipboard_sync::{Frame, KIND_PNG, KIND_REF, KIND_TEXT, MAX_PAYLOAD_BYTES};
use butterpollo_core::config::Config;
use butterpollo_core::session::Warnings;
use butterpollo_core::state::Client;

use crate::state::Session;

/// "Write to the clipboard": the device's copies.
const WRITE: u32 = 1 << 16;
/// "Read the clipboard": the PC's copies.
const READ: u32 = 1 << 17;
/// View access, either bit, as `/actions/clipboard` requires.
const VIEW: u32 = (1 << 25) | (1 << 26);
/// How often the worker looks at the clipboard's change count.
const POLL: Duration = Duration::from_millis(250);
/// How long a token this host sent marks a frame as its own coming back, as V+ keeps them.
const ECHO: Duration = Duration::from_secs(5);
/// The most tokens kept for [`ECHO`].
const ECHO_TOKENS: usize = 16;
/// The stream card's warning code.
const WARNING: &str = "clipboard_sync";

/// What a device may do with the clipboard on this stream.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Allowed {
    /// Its copies go on the PC's clipboard.
    pub write: bool,
    /// The PC's copies go to it.
    pub read: bool,
    /// The largest image either way while images are on.
    pub png: Option<usize>,
}
impl Allowed {
    pub fn any(&self) -> bool {
        self.write || self.read
    }
}

/// What `client` may do on a stream with the effective `config`.
pub(crate) fn allowed(config: &Config, client: &Client) -> Allowed {
    if client.perm & VIEW == 0 || butterpollo_core::background::enabled(config) {
        return Allowed::default();
    }
    let png = config.clipboard_image().then(|| {
        usize::try_from(config.clipboard_image_max_bytes())
            .unwrap_or(usize::MAX)
            .min(MAX_PAYLOAD_BYTES)
    });
    Allowed {
        write: client.allows(WRITE),
        read: client.allows(READ),
        png,
    }
}

/// One clipboard item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Item {
    Text(String),
    Png(Vec<u8>),
}
impl Item {
    fn frame(self, token: u32) -> Frame {
        let (kind, payload) = match self {
            Item::Text(text) => (KIND_TEXT, text.into_bytes()),
            Item::Png(png) => (KIND_PNG, png),
        };
        Frame {
            kind,
            token,
            payload,
        }
    }
}

/// Why a frame from the device was not put on the clipboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Refused {
    /// The device may neither read nor write the clipboard here.
    Nothing,
    /// The device may not write the clipboard.
    Write,
    /// `clipboard_image` is off.
    Images,
    /// An image over `clipboard_image_max_mb`.
    TooLarge,
    /// V+'s reference to a blob on its HTTPS routes, which this host does not serve.
    Reference,
    /// Text that is not UTF-8 or holds a NUL.
    Text,
    /// A kind V+ does not define.
    Kind(u8),
}
impl Refused {
    fn bit(self) -> u32 {
        match self {
            Refused::Nothing => 1,
            Refused::Write => 2,
            Refused::Images => 4,
            Refused::TooLarge => 8,
            Refused::Reference => 16,
            Refused::Text => 32,
            Refused::Kind(_) => 64,
        }
    }
    fn reason(self) -> &'static str {
        match self {
            Refused::Nothing => "the device may not read or write the clipboard on this stream",
            Refused::Write => "the device may not write the clipboard",
            Refused::Images => "the image clipboard is off on this host",
            Refused::TooLarge => "the image is over clipboard_image_max_mb",
            Refused::Reference => "blob references are not supported",
            Refused::Text => "the text is not UTF-8 or holds a NUL",
            Refused::Kind(_) => "the kind is unknown",
        }
    }
}

/// What a frame from the device asks for.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Accepted {
    /// Nothing: a hello, or a copy this host sent coming back.
    Nothing,
    /// Put this on the clipboard.
    Write(Item),
    /// Not a frame.
    Invalid,
    Refused(Refused),
}

/// A frame's item, `None` for a hello, which only starts the PC's copies.
fn inbound(frame: Frame, allowed: &Allowed) -> Result<Option<Item>, Refused> {
    if !allowed.any() {
        return Err(Refused::Nothing);
    }
    if frame.is_hello() {
        return Ok(None);
    }
    if !allowed.write {
        return Err(Refused::Write);
    }
    match frame.kind {
        KIND_TEXT => match String::from_utf8(frame.payload) {
            Ok(text) if !text.contains('\0') => Ok(Some(Item::Text(text))),
            _ => Err(Refused::Text),
        },
        KIND_PNG => match allowed.png {
            None => Err(Refused::Images),
            Some(max) if frame.payload.len() > max => Err(Refused::TooLarge),
            Some(_) => Ok(Some(Item::Png(frame.payload))),
        },
        KIND_REF => Err(Refused::Reference),
        kind => Err(Refused::Kind(kind)),
    }
}

/// The clipboard the worker uses: this PC's, or a fake in tests.
pub(crate) trait Clipboard: Send + 'static {
    /// The change count, which every new content raises.
    fn sequence(&self) -> u32;
    /// The text, empty while there is none.
    fn read(&self) -> Result<String>;
    /// The image as PNG of at most `max_bytes`, as `clipboard::read_png`.
    fn read_png(&self, max_bytes: usize) -> Result<Option<Vec<u8>>>;
    fn write(&self, text: &str) -> Result<()>;
    fn write_png(&self, png: &[u8]) -> Result<()>;
}

/// This PC's clipboard.
#[cfg(not(test))]
struct System;
#[cfg(not(test))]
impl Clipboard for System {
    fn sequence(&self) -> u32 {
        butterpollo_windows::clipboard::sequence()
    }
    fn read(&self) -> Result<String> {
        butterpollo_windows::clipboard::read()
    }
    fn read_png(&self, max_bytes: usize) -> Result<Option<Vec<u8>>> {
        butterpollo_windows::clipboard::read_png(max_bytes)
    }
    fn write(&self, text: &str) -> Result<()> {
        butterpollo_windows::clipboard::write(text)
    }
    fn write_png(&self, png: &[u8]) -> Result<()> {
        butterpollo_windows::clipboard::write_png(png)
    }
}
#[cfg(not(test))]
fn system() -> impl Clipboard {
    System
}
/// Tests never touch the clipboard of the PC running them.
#[cfg(test)]
fn system() -> impl Clipboard {
    tests::FAKE.clone()
}

/// The device's copies for the worker. Only the newest waits: it replaces
/// one the worker has not taken yet, so a device sending faster than the
/// clipboard takes them holds one copy here rather than all of them.
#[derive(Default)]
struct Inbox {
    /// The waiting copy, and whether the peer has gone.
    slot: Mutex<(Option<Item>, bool)>,
    ready: Condvar,
}
/// The peer has gone, and no copy waits.
#[derive(Debug, PartialEq)]
struct Closed;
impl Inbox {
    fn put(&self, item: Item) {
        self.slot.lock().unwrap().0 = Some(item);
        self.ready.notify_one();
    }
    fn close(&self) {
        self.slot.lock().unwrap().1 = true;
        self.ready.notify_one();
    }
    /// The waiting copy, after up to `timeout` for one.
    fn take(&self, timeout: Duration) -> Result<Option<Item>, Closed> {
        let slot = self.slot.lock().unwrap();
        let (mut slot, _) = self
            .ready
            .wait_timeout_while(slot, timeout, |(item, closed)| item.is_none() && !*closed)
            .unwrap();
        match slot.0.take() {
            Some(item) => Ok(Some(item)),
            None if slot.1 => Err(Closed),
            None => Ok(None),
        }
    }
}
/// The control loop's end of an [`Inbox`]: closes it when the peer goes,
/// which ends the worker.
struct Mailbox(Arc<Inbox>);
impl Drop for Mailbox {
    fn drop(&mut self) {
        self.0.close();
    }
}

/// A peer's clipboard sync, in the control loop.
#[derive(Default)]
pub(crate) struct PeerSync {
    /// The worker's inbox of the device's copies, from the first frame.
    worker: Option<Mailbox>,
    /// The PC's newest copy, for the loop to send.
    outbox: Arc<Mutex<Option<Frame>>>,
    /// Tokens of the PC's copies sent, with when.
    sent: VecDeque<(u32, Instant)>,
    /// [`Refused::bit`]s warned about once.
    warned: u32,
}
impl PeerSync {
    /// Takes a frame from the device (0x5508): starts the worker on the first, and puts its
    /// item on the clipboard. A refusal is warned about once per peer and kind.
    pub(crate) fn receive(
        &mut self,
        s: &Session,
        payload: &[u8],
        allowed: Allowed,
        wake: &Arc<UdpSocket>,
    ) {
        let accepted = self.accept(payload, &allowed, Instant::now());
        if !matches!(
            accepted,
            Accepted::Invalid | Accepted::Refused(Refused::Nothing)
        ) && self.worker.is_none()
        {
            tracing::info!(
                client = %s.launch.client.name,
                write = allowed.write,
                read = allowed.read,
                images = allowed.png.is_some(),
                "clipboard sync"
            );
            let items = Mailbox(Arc::default());
            let worker = items.0.clone();
            let watch = Watch::new(system(), allowed);
            let outbox = self.outbox.clone();
            let (client, warnings, wake) = (
                s.launch.client.name.clone(),
                s.launch.warnings.clone(),
                wake.clone(),
            );
            match std::thread::Builder::new()
                .name("clipboard sync".into())
                .spawn(move || run(watch, worker, outbox, wake, &client, &warnings))
            {
                Ok(_) => self.worker = Some(items),
                Err(e) => {
                    tracing::warn!(error = %e, "clipboard sync worker not started");
                    s.launch.warnings.set(
                        WARNING,
                        format!(
                            "Clipboard sync is off for this stream: its worker did not start ({e})."
                        ),
                    );
                }
            }
        }
        match accepted {
            Accepted::Nothing => {}
            Accepted::Invalid => {
                tracing::debug!(len = payload.len(), "invalid clipboard sync frame")
            }
            Accepted::Write(item) => {
                if let Some(worker) = &self.worker {
                    worker.0.put(item);
                }
            }
            Accepted::Refused(refused) => {
                if self.warned & refused.bit() != 0 {
                    return;
                }
                self.warned |= refused.bit();
                tracing::warn!(
                    client = %s.launch.client.name,
                    ?refused,
                    "clipboard sync refused: {}",
                    refused.reason()
                );
                if matches!(refused, Refused::Nothing | Refused::Write) {
                    s.launch.warnings.set(
                        WARNING,
                        "The device's copies do not reach this PC's clipboard: it lacks the clipboard permission here.",
                    );
                }
            }
        }
    }

    /// What a frame from the device asks for, `now` dropping old tokens.
    fn accept(&mut self, payload: &[u8], allowed: &Allowed, now: Instant) -> Accepted {
        let Some(frame) = Frame::decode(payload) else {
            return Accepted::Invalid;
        };
        self.sent
            .retain(|(_, at)| now.saturating_duration_since(*at) < ECHO);
        if self.sent.iter().any(|(token, _)| *token == frame.token) {
            return Accepted::Nothing;
        }
        match inbound(frame, allowed) {
            Ok(None) => Accepted::Nothing,
            Ok(Some(item)) => Accepted::Write(item),
            Err(refused) => Accepted::Refused(refused),
        }
    }

    /// The PC's newest copy as a frame to send, its token kept for [`ECHO`].
    pub(crate) fn take(&mut self, now: Instant) -> Option<Vec<u8>> {
        let frame = self.outbox.lock().unwrap().take()?;
        if self.sent.len() == ECHO_TOKENS {
            self.sent.pop_front();
        }
        self.sent.push_back((frame.token, now));
        frame.encode()
    }
}

/// The worker's view of the clipboard.
struct Watch<C> {
    clipboard: C,
    allowed: Allowed,
    /// The change count last seen; the clipboard's content then is not sent.
    seen: u32,
    /// The item last synced either way, which is not synced again.
    last: Option<Item>,
    /// Whether an item too large to send was warned about.
    warned_large: bool,
    /// Whether a copy kept back for want of the read permission was warned about.
    warned_unread: bool,
}
impl<C: Clipboard> Watch<C> {
    /// From the clipboard as it is now, which stays on this PC.
    fn new(clipboard: C, allowed: Allowed) -> Self {
        let seen = clipboard.sequence();
        Self {
            clipboard,
            allowed,
            seen,
            last: None,
            warned_large: false,
            warned_unread: false,
        }
    }

    /// Puts the device's copy on the clipboard, unless it is what was last synced.
    fn write(&mut self, item: Item) -> Result<()> {
        if self.last.as_ref() == Some(&item) {
            return Ok(());
        }
        match &item {
            Item::Text(text) => self.clipboard.write(text)?,
            Item::Png(png) => self.clipboard.write_png(png)?,
        }
        self.seen = self.clipboard.sequence();
        self.last = Some(item);
        Ok(())
    }

    /// The PC's copy since the last look, as a frame: its text, or else its image while
    /// images are on. Its own writes, what was last synced and empty clipboards give none.
    /// A new copy the device may not read is an [`Unread`] error, not read.
    fn poll(&mut self) -> Result<Option<Frame>> {
        let sequence = self.clipboard.sequence();
        if sequence == self.seen {
            return Ok(None);
        }
        self.seen = sequence;
        if !self.allowed.read {
            bail!(Unread);
        }
        let text = self.clipboard.read()?;
        let item = if !text.is_empty() {
            if text.len() > MAX_PAYLOAD_BYTES {
                bail!(Large(text.len()));
            }
            Item::Text(text)
        } else if let Some(max) = self.allowed.png {
            match self.clipboard.read_png(max) {
                Ok(Some(png)) => Item::Png(png),
                Ok(None) => return Ok(None),
                Err(e) if e.downcast_ref().is_some_and(ImageError::too_large) => bail!(Large(max)),
                Err(e) => return Err(e),
            }
        } else {
            return Ok(None);
        };
        if self.last.as_ref() == Some(&item) {
            return Ok(None);
        }
        self.last = Some(item.clone());
        Ok(Some(item.frame(sequence)))
    }
}

/// A copy too large for one frame: over this many bytes.
#[derive(Debug)]
struct Large(usize);
impl std::fmt::Display for Large {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the copy is over {} bytes", self.0)
    }
}
impl std::error::Error for Large {}

/// A copy on the PC kept back: the device lacks "Read the clipboard" here.
#[derive(Debug)]
struct Unread;
impl std::fmt::Display for Unread {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the device may not read the clipboard here")
    }
}
impl std::error::Error for Unread {}

/// The worker: until the peer goes, puts the device's copies on the clipboard and notes the
/// PC's in `outbox`, waking the control loop.
fn run<C: Clipboard>(
    mut watch: Watch<C>,
    items: Arc<Inbox>,
    outbox: Arc<Mutex<Option<Frame>>>,
    wake: Arc<UdpSocket>,
    client: &str,
    warnings: &Warnings,
) {
    // Failures are warned about once until the clipboard works again.
    let (mut write_failed, mut read_failed) = (false, false);
    loop {
        match items.take(POLL) {
            Ok(Some(item)) => match watch.write(item) {
                Ok(()) => write_failed = false,
                Err(e) if !write_failed => {
                    write_failed = true;
                    tracing::warn!(client, error = %format!("{e:#}"), "clipboard sync: the device's copy was not put on the clipboard");
                }
                Err(_) => {}
            },
            Ok(None) => {}
            Err(Closed) => return,
        }
        match watch.poll() {
            Ok(Some(frame)) => {
                read_failed = false;
                *outbox.lock().unwrap() = Some(frame);
                let _ = wake.send(&[1]);
            }
            Ok(None) => {}
            Err(e) if e.downcast_ref::<Large>().is_some() => {
                if !watch.warned_large {
                    watch.warned_large = true;
                    tracing::warn!(client, error = %e, "clipboard sync: the PC's copy is too large to send; use the device's clipboard button");
                    warnings.set(
                        WARNING,
                        format!("A copy on this PC did not reach the device: {e}. Larger ones go by the device's clipboard button."),
                    );
                }
            }
            Err(e) if e.downcast_ref::<Unread>().is_some() => {
                if !watch.warned_unread {
                    watch.warned_unread = true;
                    tracing::warn!(
                        client,
                        "clipboard sync refused: a copy on this PC was not sent: {e}"
                    );
                    warnings.set(
                        WARNING,
                        "This PC's copies do not reach the device: it lacks the permission to read the clipboard here.",
                    );
                }
            }
            Err(e) if !read_failed => {
                read_failed = true;
                tracing::warn!(client, error = %format!("{e:#}"), "clipboard sync: the PC's clipboard could not be read");
            }
            Err(_) => {}
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
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
        assert_eq!(sync.take(now).unwrap(), text(42, "pc"));
        assert_eq!(sync.take(now), None);
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
            sync.take(now);
        }
        assert_eq!(sync.sent.len(), ECHO_TOKENS);
        assert_eq!(sync.sent.front().unwrap().0, 101);
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
        let (items, worker) = mpsc::channel();
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
}
