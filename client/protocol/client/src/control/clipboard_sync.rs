//! Clipboard sync (0x5508) on the client side: what is copied on the device goes to the PC,
//! and the PC's copies go on the device's clipboard. The frame and the echo rules are
//! Moonlight V+'s (`ClipboardSyncManager.kt` in https://github.com/qiin2333/moonlight-vplus,
//! GPL-3.0), as the Rubylight host keeps them: every copy sent carries a fresh token, a frame
//! that comes back with one sent in the last [`ECHO`] is dropped, and the item last synced
//! either way is not synced again, so writing the PC's copy to the device's clipboard does
//! not send it back.
//!
//! The app asks with [`rubylight_protocol::clipboard_sync::LAUNCH_PARAMETER`] at launch; when
//! the host announced 0x5508 it sends [`ClipboardSync::hello`] once, then [`ClipboardSync::local`]
//! for each copy on the device, and hands every
//! [`ControlEvent::ClipboardSync`](super::ControlEvent::ClipboardSync) to
//! [`ClipboardSync::received`]. The app does the clipboard work; this is only the rules.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use rand_core::{OsRng, RngCore};
use rubylight_protocol::clipboard_sync::{Frame, KIND_PNG, KIND_REF, KIND_TEXT, MAX_PAYLOAD_BYTES};

use super::ClientMessage;

/// How long a token sent marks a frame as the device's own copy coming back, as V+ and the
/// host keep them.
pub const ECHO: Duration = Duration::from_secs(5);
/// The most tokens kept for [`ECHO`].
pub const ECHO_TOKENS: usize = 16;

/// One clipboard item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClipboardItem {
    /// UTF-8 text without a NUL.
    Text(String),
    /// A PNG image.
    Png(Vec<u8>),
}

impl ClipboardItem {
    fn len(&self) -> usize {
        match self {
            Self::Text(text) => text.len(),
            Self::Png(png) => png.len(),
        }
    }
}

/// What to do with a copy on the device.
#[derive(Clone, Debug, PartialEq)]
pub enum Local {
    /// Send this to the host.
    Send(ClientMessage),
    /// Nothing to send: empty, an image while images are off, or what was last synced
    /// (often the host's copy just written here).
    Skip,
    /// The copy is over one frame's [`MAX_PAYLOAD_BYTES`] by this many bytes in all.
    TooLarge(usize),
}

/// What to do with a frame from the host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Received {
    /// Put this on the device's clipboard.
    Apply(ClipboardItem),
    /// Nothing: the device's own copy coming back, what was last synced, or a hello.
    Nothing,
    /// A frame this client does not put on the clipboard.
    Refused(Refused),
}

/// Why a frame from the host was not put on the device's clipboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// A PNG while this client syncs only text.
    Images,
    /// V+'s reference to a blob on its HTTPS routes, which Rubylight does not serve.
    Reference,
    /// Text that is not UTF-8 or holds a NUL.
    Text,
    /// A kind V+ does not define.
    Kind(u8),
}

impl Refused {
    /// The reason, for the log.
    pub fn reason(self) -> &'static str {
        match self {
            Self::Images => "images are not synced on this device",
            Self::Reference => "blob references are not supported",
            Self::Text => "the text is not UTF-8 or holds a NUL",
            Self::Kind(_) => "the kind is unknown",
        }
    }
}

/// One stream's clipboard sync rules; a new stream starts a new one.
#[derive(Debug)]
pub struct ClipboardSync {
    /// Whether PNG images go both ways, or only text.
    images: bool,
    /// Tokens of the device's copies sent, with when.
    sent: VecDeque<(u32, Instant)>,
    /// The item last synced either way.
    last: Option<ClipboardItem>,
}

impl ClipboardSync {
    pub fn new(images: bool) -> Self {
        Self {
            images,
            sent: VecDeque::new(),
            last: None,
        }
    }

    pub fn images(&self) -> bool {
        self.images
    }

    /// The empty text frame that starts the host's copies; nothing is copied.
    pub fn hello(&self) -> ClientMessage {
        ClientMessage::ClipboardSync(Frame::hello(OsRng.next_u32()))
    }

    /// Notes the device's clipboard as it is without sending it: at stream start, as the host
    /// keeps the PC's clipboard to itself until something new is copied.
    pub fn baseline(&mut self, item: Option<ClipboardItem>) {
        self.last = item;
    }

    /// A copy on the device at `now`.
    pub fn local(&mut self, item: ClipboardItem, now: Instant) -> Local {
        let empty = match &item {
            ClipboardItem::Text(text) => text.is_empty() || text.contains('\0'),
            ClipboardItem::Png(png) => png.is_empty() || !self.images,
        };
        if empty || self.last.as_ref() == Some(&item) {
            return Local::Skip;
        }
        if item.len() > MAX_PAYLOAD_BYTES {
            return Local::TooLarge(item.len());
        }
        self.prune(now);
        let token = loop {
            // A token still kept would make the host's copy with it look like an echo.
            let token = OsRng.next_u32();
            if !self.sent.iter().any(|(sent, _)| *sent == token) {
                break token;
            }
        };
        if self.sent.len() == ECHO_TOKENS {
            self.sent.pop_front();
        }
        self.sent.push_back((token, now));
        let (kind, payload) = match item.clone() {
            ClipboardItem::Text(text) => (KIND_TEXT, text.into_bytes()),
            ClipboardItem::Png(png) => (KIND_PNG, png),
        };
        self.last = Some(item);
        Local::Send(ClientMessage::ClipboardSync(Frame {
            kind,
            token,
            payload,
        }))
    }

    /// A frame from the host at `now`.
    pub fn received(&mut self, frame: Frame, now: Instant) -> Received {
        self.prune(now);
        if let Some(index) = self
            .sent
            .iter()
            .position(|(token, _)| *token == frame.token)
        {
            self.sent.remove(index);
            return Received::Nothing;
        }
        if frame.is_hello() {
            return Received::Nothing;
        }
        let item = match frame.kind {
            KIND_TEXT => match String::from_utf8(frame.payload) {
                Ok(text) if !text.contains('\0') => ClipboardItem::Text(text),
                _ => return Received::Refused(Refused::Text),
            },
            KIND_PNG if self.images => ClipboardItem::Png(frame.payload),
            KIND_PNG => return Received::Refused(Refused::Images),
            KIND_REF => return Received::Refused(Refused::Reference),
            kind => return Received::Refused(Refused::Kind(kind)),
        };
        if self.last.as_ref() == Some(&item) {
            return Received::Nothing;
        }
        self.last = Some(item.clone());
        Received::Apply(item)
    }

    fn prune(&mut self, now: Instant) {
        self.sent
            .retain(|(_, at)| now.saturating_duration_since(*at) < ECHO);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> ClipboardItem {
        ClipboardItem::Text(s.into())
    }

    fn sent(local: Local) -> Frame {
        match local {
            Local::Send(ClientMessage::ClipboardSync(frame)) => frame,
            other => panic!("{other:?}"),
        }
    }

    fn from_host(kind: u8, token: u32, payload: &[u8]) -> Frame {
        Frame {
            kind,
            token,
            payload: payload.to_vec(),
        }
    }

    #[test]
    fn a_copy_goes_out_once_as_text_with_a_fresh_token() {
        let now = Instant::now();
        let mut sync = ClipboardSync::new(false);
        let frame = sent(sync.local(text("hi"), now));
        assert_eq!(
            (frame.kind, frame.payload.as_slice()),
            (KIND_TEXT, &b"hi"[..])
        );
        // The same copy again, as a focus regain re-reads it, is not sent again.
        assert_eq!(sync.local(text("hi"), now), Local::Skip);
        let other = sent(sync.local(text("ho"), now));
        assert_ne!(other.token, frame.token);
    }

    #[test]
    fn the_hello_is_an_empty_text_frame() {
        let ClientMessage::ClipboardSync(hello) = ClipboardSync::new(false).hello() else {
            panic!();
        };
        assert!(hello.is_hello());
    }

    #[test]
    fn the_hosts_echo_of_a_copy_is_dropped_for_five_seconds() {
        let now = Instant::now();
        let mut sync = ClipboardSync::new(false);
        let frame = sent(sync.local(text("hi"), now));
        assert_eq!(
            sync.received(frame.clone(), now + ECHO / 2),
            Received::Nothing
        );
        // A token is used up by its echo; a later frame with it is the host's own copy.
        let mut again = frame.clone();
        again.payload = b"new".to_vec();
        assert_eq!(
            sync.received(again, now + ECHO / 2),
            Received::Apply(text("new"))
        );
        // After five seconds a token no longer marks an echo.
        let late = sent(sync.local(text("late"), now));
        let mut changed = late.clone();
        changed.payload = b"changed".to_vec();
        assert_eq!(
            sync.received(changed, now + ECHO),
            Received::Apply(text("changed"))
        );
    }

    #[test]
    fn only_sixteen_tokens_are_kept() {
        let now = Instant::now();
        let mut sync = ClipboardSync::new(false);
        let first = sent(sync.local(text("0"), now));
        for n in 1..=ECHO_TOKENS {
            sent(sync.local(text(&n.to_string()), now));
        }
        let mut back = first;
        back.payload = b"from the pc".to_vec();
        assert_eq!(
            sync.received(back, now),
            Received::Apply(text("from the pc"))
        );
    }

    #[test]
    fn the_hosts_copy_written_here_is_not_sent_back() {
        let now = Instant::now();
        let mut sync = ClipboardSync::new(false);
        assert_eq!(
            sync.received(from_host(KIND_TEXT, 42, b"pc"), now),
            Received::Apply(text("pc"))
        );
        // The device's clipboard listener fires for the write.
        assert_eq!(sync.local(text("pc"), now), Local::Skip);
        // The same copy from the host again changes nothing.
        assert_eq!(
            sync.received(from_host(KIND_TEXT, 43, b"pc"), now),
            Received::Nothing
        );
        assert!(matches!(sync.local(text("phone"), now), Local::Send(_)));
    }

    #[test]
    fn the_clipboard_at_stream_start_stays_on_the_device() {
        let now = Instant::now();
        let mut sync = ClipboardSync::new(false);
        sync.baseline(Some(text("before")));
        assert_eq!(sync.local(text("before"), now), Local::Skip);
        assert!(matches!(sync.local(text("after"), now), Local::Send(_)));
    }

    #[test]
    fn empty_large_and_image_copies_follow_the_rules() {
        let now = Instant::now();
        let mut sync = ClipboardSync::new(false);
        assert_eq!(
            sync.local(text(""), now),
            Local::Skip,
            "empty text is a hello"
        );
        assert_eq!(sync.local(text("a\0b"), now), Local::Skip);
        assert_eq!(sync.local(ClipboardItem::Png(vec![1]), now), Local::Skip);
        let big = "x".repeat(MAX_PAYLOAD_BYTES + 1);
        assert_eq!(
            sync.local(text(&big), now),
            Local::TooLarge(MAX_PAYLOAD_BYTES + 1)
        );
        assert!(matches!(
            sync.local(text(&big[1..]), now),
            Local::Send(message) if message.is_valid()
        ));
        let mut images = ClipboardSync::new(true);
        let png = sent(images.local(ClipboardItem::Png(vec![0x89, b'P']), now));
        assert_eq!(png.kind, KIND_PNG);
        assert_eq!(images.local(ClipboardItem::Png(vec![]), now), Local::Skip);
    }

    #[test]
    fn frames_this_client_cannot_use_are_refused() {
        let now = Instant::now();
        let mut sync = ClipboardSync::new(false);
        for (frame, refused) in [
            (from_host(KIND_PNG, 1, &[0x89]), Refused::Images),
            (from_host(KIND_REF, 2, b"{}"), Refused::Reference),
            (from_host(KIND_TEXT, 3, &[0xff]), Refused::Text),
            (from_host(KIND_TEXT, 4, b"a\0"), Refused::Text),
            (from_host(9, 5, b"?"), Refused::Kind(9)),
        ] {
            assert_eq!(sync.received(frame, now), Received::Refused(refused));
            assert!(!refused.reason().is_empty());
        }
        assert_eq!(sync.received(Frame::hello(6), now), Received::Nothing);
        let mut images = ClipboardSync::new(true);
        assert_eq!(
            images.received(from_host(KIND_PNG, 1, &[0x89]), now),
            Received::Apply(ClipboardItem::Png(vec![0x89]))
        );
    }
}
