//! Clipboard sync (0x5508) with a Rubylight PC that offered it: text copied here goes on the
//! PC's clipboard, and the PC's copies come here. SDL says when the clipboard changes
//! (`SDL_EVENT_CLIPBOARD_UPDATE`), so nothing polls. The rules, Moonlight V+'s as the host
//! keeps them, are `nightjar_core::control::clipboard_sync`: the clipboard as it was when
//! the stream started stays here, the PC's copy written here is not sent back, and the PC's
//! echo of a copy is dropped. Images go by the stream controls' image buttons.

use std::time::Instant;

use nightjar_core::control::clipboard_sync::{ClipboardItem, ClipboardSync, Local, Received, Refused};
use nightjar_core::control::ClientMessage;
use rubylight_protocol::clipboard_sync::Frame;

use crate::strings::tr;

/// The clipboard here, text only.
pub trait Clipboard {
    /// The text, empty while there is none.
    fn text(&self) -> Result<String, String>;
    fn set_text(&mut self, text: &str) -> Result<(), String>;
}

/// SDL's clipboard. Main thread.
pub struct Sdl;

impl Clipboard for Sdl {
    fn text(&self) -> Result<String, String> {
        use sdl3_sys::everything::*;
        // SAFETY: SDL returns a NUL-terminated buffer we free, or null on failure.
        unsafe {
            if !SDL_HasClipboardText() {
                return Ok(String::new());
            }
            let text = SDL_GetClipboardText();
            if text.is_null() {
                return Err(crate::sdl::error());
            }
            let value = std::ffi::CStr::from_ptr(text).to_string_lossy().into_owned();
            SDL_free(text.cast());
            Ok(value)
        }
    }

    fn set_text(&mut self, text: &str) -> Result<(), String> {
        use sdl3_sys::everything::*;
        let text = std::ffi::CString::new(text).map_err(|e| e.to_string())?;
        // SAFETY: a NUL-terminated string SDL copies.
        if unsafe { SDL_SetClipboardText(text.as_ptr()) } {
            Ok(())
        } else {
            Err(crate::sdl::error())
        }
    }
}

/// What the stream does after a clipboard step.
#[derive(Debug, Default, PartialEq)]
pub struct Step {
    pub send: Option<ClientMessage>,
    /// Said once per kind of trouble.
    pub toast: Option<&'static str>,
}

/// One stream's clipboard sync.
pub struct Sync<C> {
    clipboard: C,
    rules: ClipboardSync,
    /// Kinds of trouble already toasted.
    told: u8,
}

const TOLD_LARGE: u8 = 1;
const TOLD_WRITE: u8 = 2;
const TOLD_READ: u8 = 4;
const TOLD_IMAGE: u8 = 8;
const TOLD_REFUSED: u8 = 16;

impl<C: Clipboard> Sync<C> {
    /// Starts the sync: the clipboard as it is stays here, and the hello to send starts the
    /// PC's copies.
    pub fn start(clipboard: C) -> (Self, ClientMessage) {
        let mut rules = ClipboardSync::new(false);
        match clipboard.text() {
            Ok(text) => rules.baseline((!text.is_empty()).then_some(ClipboardItem::Text(text))),
            Err(error) => eprintln!("nightjar: clipboard sync: could not read the clipboard at the start: {error}"),
        }
        let hello = rules.hello();
        (Self { clipboard, rules, told: 0 }, hello)
    }

    fn tell(&mut self, kind: u8, toast: &'static str) -> Option<&'static str> {
        let first = self.told & kind == 0;
        self.told |= kind;
        first.then_some(toast)
    }

    /// The clipboard here changed.
    pub fn changed(&mut self, now: Instant) -> Step {
        let text = match self.clipboard.text() {
            Ok(text) => text,
            Err(error) => {
                eprintln!("nightjar: clipboard sync: could not read the clipboard: {error}");
                let toast = self.tell(TOLD_READ, tr("Clipboard sync could not read this device's clipboard."));
                return Step { send: None, toast };
            }
        };
        match self.rules.local(ClipboardItem::Text(text), now) {
            Local::Send(message) => Step { send: Some(message), toast: None },
            Local::Skip => Step::default(),
            Local::TooLarge(bytes) => {
                eprintln!("nightjar: clipboard sync: a copy of {bytes} bytes is too large to send");
                let toast = self.tell(TOLD_LARGE, tr("This copy is too large for clipboard sync. Send it with Send clipboard to PC."));
                Step { send: None, toast }
            }
        }
    }

    /// A frame from the PC.
    pub fn received(&mut self, frame: Frame, now: Instant) -> Step {
        let refused = match self.rules.received(frame, now) {
            Received::Apply(ClipboardItem::Text(text)) => {
                return match self.clipboard.set_text(&text) {
                    Ok(()) => Step::default(),
                    Err(error) => {
                        eprintln!("nightjar: clipboard sync: could not put the PC's copy on the clipboard: {error}");
                        let toast = self.tell(TOLD_WRITE, tr("Clipboard sync could not put the PC's copy on this device's clipboard."));
                        Step { send: None, toast }
                    }
                };
            }
            // Images are off here, so the rules never apply one.
            Received::Apply(ClipboardItem::Png(_)) => Refused::Images,
            Received::Nothing => return Step::default(),
            Received::Refused(refused) => refused,
        };
        eprintln!("nightjar: clipboard sync: refused the PC's copy: {}", refused.reason());
        let toast = if refused == Refused::Images {
            self.tell(TOLD_IMAGE, tr("The PC copied an image. Clipboard sync carries text; use Copy PC image here (Experimental)."))
        } else {
            self.tell(TOLD_REFUSED, tr("Clipboard sync could not use a copy from the PC."))
        };
        Step { send: None, toast }
    }

    /// A 0x5508 message from the PC that was not a frame.
    pub fn rejected(&mut self, bytes: usize) -> Step {
        eprintln!("nightjar: clipboard sync: dropped a {bytes}-byte message that is not a clipboard frame");
        let toast = self.tell(TOLD_REFUSED, tr("Clipboard sync could not use a copy from the PC."));
        Step { send: None, toast }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rubylight_protocol::clipboard_sync::{KIND_PNG, KIND_TEXT, MAX_PAYLOAD_BYTES};

    #[derive(Default)]
    struct Fake {
        text: String,
        fail_read: bool,
        fail_write: bool,
        writes: u32,
    }

    impl Clipboard for &mut Fake {
        fn text(&self) -> Result<String, String> {
            if self.fail_read { Err("locked".into()) } else { Ok(self.text.clone()) }
        }
        fn set_text(&mut self, text: &str) -> Result<(), String> {
            if self.fail_write {
                return Err("locked".into());
            }
            self.writes += 1;
            self.text = text.into();
            Ok(())
        }
    }

    fn frame(step: Step) -> Frame {
        match step.send {
            Some(ClientMessage::ClipboardSync(frame)) => frame,
            other => panic!("{other:?}"),
        }
    }

    fn from_pc(token: u32, text: &str) -> Frame {
        Frame { kind: KIND_TEXT, token, payload: text.as_bytes().to_vec() }
    }

    #[test]
    fn the_start_sends_a_hello_and_keeps_the_clipboard_here() {
        let mut fake = Fake { text: "before".into(), ..Fake::default() };
        let (mut sync, hello) = Sync::start(&mut fake);
        assert!(matches!(hello, ClientMessage::ClipboardSync(ref f) if f.is_hello()));
        let now = Instant::now();
        // An update that changed nothing the sync cares about sends nothing.
        assert_eq!(sync.changed(now), Step::default());
        sync.clipboard.text = "copied".into();
        let sent = frame(sync.changed(now));
        assert_eq!((sent.kind, sent.payload.as_slice()), (KIND_TEXT, &b"copied"[..]));
    }

    #[test]
    fn the_pcs_copy_lands_here_and_does_not_go_back() {
        let mut fake = Fake::default();
        let (mut sync, _) = Sync::start(&mut fake);
        let now = Instant::now();
        assert_eq!(sync.received(from_pc(7, "from the pc"), now), Step::default());
        assert_eq!((sync.clipboard.text.as_str(), sync.clipboard.writes), ("from the pc", 1));
        // SDL reports the write as a clipboard update.
        assert_eq!(sync.changed(now), Step::default());
        // The PC's echo of a copy made here is dropped.
        sync.clipboard.text = "mine".into();
        let mine = frame(sync.changed(now));
        sync.clipboard.text = "newer".into();
        frame(sync.changed(now));
        assert_eq!(sync.received(mine, now), Step::default());
        assert_eq!((sync.clipboard.text.as_str(), sync.clipboard.writes), ("newer", 1));
    }

    #[test]
    fn trouble_is_said_once_per_kind() {
        let mut fake = Fake::default();
        let (mut sync, _) = Sync::start(&mut fake);
        let now = Instant::now();
        sync.clipboard.text = "x".repeat(MAX_PAYLOAD_BYTES + 1);
        let large = sync.changed(now);
        assert!(large.send.is_none() && large.toast.is_some());
        assert_eq!(sync.changed(now), Step::default());

        sync.clipboard.fail_write = true;
        assert!(sync.received(from_pc(1, "a"), now).toast.is_some());
        assert!(sync.received(from_pc(2, "b"), now).toast.is_none());

        let png = Frame { kind: KIND_PNG, token: 3, payload: vec![0x89] };
        assert!(sync.received(png.clone(), now).toast.is_some());
        assert!(sync.received(png, now).toast.is_none());

        assert!(sync.rejected(3).toast.is_some());
        assert!(sync.received(Frame { kind: 9, token: 4, payload: vec![1] }, now).toast.is_none());

        sync.clipboard.fail_read = true;
        assert!(sync.changed(now).toast.is_some());
        assert!(sync.changed(now).toast.is_none());
    }
}
