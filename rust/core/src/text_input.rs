//! The device's keyboard follows the host's text fields (control message 0x553B,
//! `keyboard_follows_focus`, Experimental).
//!
//! A focus watcher (UI Automation on Windows) describes the focused element as a
//! [`FocusedElement`]; [`classify`] turns that into the kind of field, or none. The session
//! keeps only the newest focus ([`TextInputs::note`]) and the control loop
//! [`TextInputs::take`]s the notice to send, off the video and input paths: one per change,
//! at most one per [`INTERVAL`] with the newest winning, and a lower only once no field has
//! had the focus for [`LOWER_DELAY`], so moving between two fields does not flash the
//! keyboard. Nothing is sent to a client that did not ask, and nothing at all while
//! `keyboard_follows_focus` is off.
//!
//! The text around the cursor goes out only when the client asked for it,
//! `text_input_surrounding_text` is on and the field is not a password or otherwise
//! sensitive field, cut to the client's `max_text` around the cursor ([`bound`]).
pub use rubylight_protocol::text_input::{
    InputType, MAX_TEXT_BYTES, NoticeKind, SELECTION_UNKNOWN, TEXT_INPUT_MESSAGE_TYPE,
    TextInputNotice, TextInputRequest,
};
use std::time::{Duration, Instant};

/// The least time between two notices; a burst of focus changes sends the newest.
pub const INTERVAL: Duration = Duration::from_millis(50);
/// How long no field must have the focus before the keyboard is lowered.
pub const LOWER_DELAY: Duration = Duration::from_millis(150);

/// UI Automation control type ids (`UIA_*ControlTypeId`).
pub mod control_type {
    pub const COMBO_BOX: i32 = 50003;
    pub const EDIT: i32 = 50004;
    pub const SPINNER: i32 = 50016;
    pub const DOCUMENT: i32 = 50030;
}

/// What UI Automation says about the focused element.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FocusedElement {
    /// `UIA_ControlTypePropertyId`.
    pub control_type: i32,
    pub enabled: bool,
    /// `UIA_IsPasswordPropertyId`.
    pub password: bool,
    /// The value pattern's `IsReadOnly`; `None` when the element has no value pattern.
    pub read_only: Option<bool>,
    /// The element has a text pattern.
    pub text_pattern: bool,
    /// `UIA_AriaRolePropertyId`, as browsers fill it in.
    pub aria_role: String,
    pub name: String,
    pub automation_id: String,
    pub class_name: String,
    pub localized_type: String,
}

/// The keyboard a focused field wants.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Field {
    pub input_type: InputType,
    /// Never any text: a password or a field named like one.
    pub sensitive: bool,
    pub no_autocorrect: bool,
    pub no_autocapitalize: bool,
    /// Enter submits rather than starting a new line.
    pub enter_action: bool,
}

/// The field kind of the focused element, or `None` when it takes no typed text: a
/// disabled or read-only element, or anything that is not an edit control, an editable
/// document, combo box or spinner, or a browser text box.
pub fn classify(element: &FocusedElement) -> Option<Field> {
    if !element.enabled || element.read_only == Some(true) {
        return None;
    }
    let role = element.aria_role.to_ascii_lowercase();
    let editable = match element.control_type {
        control_type::EDIT => true,
        control_type::DOCUMENT | control_type::COMBO_BOX | control_type::SPINNER => {
            element.read_only == Some(false)
        }
        _ => {
            matches!(role.as_str(), "textbox" | "searchbox" | "spinbutton")
                && (element.read_only == Some(false) || element.text_pattern)
        }
    };
    if !editable {
        return None;
    }
    let hints = [
        &element.name,
        &element.automation_id,
        &element.class_name,
        &element.localized_type,
    ]
    .map(|hint| hint.to_lowercase())
    .join(" ");
    let named = |words: &[&str]| words.iter().any(|word| hints.contains(word));
    let input_type = if element.password || named(&["password", "passwort", "kennwort"]) {
        InputType::Password
    } else if element.control_type == control_type::SPINNER || role == "spinbutton" {
        InputType::Number
    } else if element.control_type == control_type::DOCUMENT {
        InputType::Multiline
    } else if named(&["e-mail", "email"]) {
        InputType::Email
    } else if named(&[
        "url",
        "address bar",
        "address and search bar",
        "enter address",
        "adressleiste",
        "omnibox",
    ]) {
        InputType::Url
    } else if role == "searchbox" || named(&["search", "suche"]) {
        InputType::Search
    } else if named(&["phone", "telefon"]) {
        InputType::Phone
    } else {
        InputType::Text
    };
    let exact = matches!(
        input_type,
        InputType::Password | InputType::Email | InputType::Url
    );
    Some(Field {
        input_type,
        sensitive: input_type == InputType::Password,
        no_autocorrect: exact || matches!(input_type, InputType::Number | InputType::Phone),
        no_autocapitalize: exact,
        enter_action: input_type != InputType::Multiline,
    })
}

/// The text around the cursor as the watcher read it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Surrounding {
    pub text: String,
    /// Start and end of the selection (the cursor when equal) in UTF-16 code units into
    /// `text`; `None` when the field does not say.
    pub selection: Option<(usize, usize)>,
}

/// The focused text field and, when the watcher read it, its text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Focused {
    pub field: Field,
    pub surrounding: Option<Surrounding>,
}

/// Cuts `surrounding` to at most `max_bytes` of UTF-8 around the selection, on character
/// boundaries, and returns the text with the selection in UTF-16 code units into it
/// ([`SELECTION_UNKNOWN`] when unknown). Without a selection the end of the text is kept,
/// where a single-line field has its cursor.
pub fn bound(surrounding: &Surrounding, max_bytes: usize) -> (String, u16, u16) {
    let text = surrounding.text.as_str();
    let byte_at = |units: usize| {
        let mut counted = 0;
        for (at, c) in text.char_indices() {
            if counted >= units {
                return at;
            }
            counted += c.len_utf16();
        }
        text.len()
    };
    let selection = surrounding.selection.map(|(start, end)| {
        let (start, end) = (start.min(end), start.max(end));
        (byte_at(start), byte_at(end))
    });
    let max_bytes = max_bytes.min(usize::from(MAX_TEXT_BYTES));
    let (from, to) = if text.len() <= max_bytes {
        (0, text.len())
    } else {
        let wanted_from = match selection {
            // Centre the selection; one longer than the window starts the window.
            Some((start, end)) if end - start < max_bytes => {
                start.saturating_sub((max_bytes - (end - start)) / 2)
            }
            Some((start, _)) => start,
            None => text.len() - max_bytes,
        };
        let from = floor_boundary(text, wanted_from.min(text.len() - max_bytes));
        (from, floor_boundary(text, from + max_bytes))
    };
    let window = &text[from..to];
    let units = |bytes: usize| -> u16 {
        let bytes = bytes.clamp(from, to) - from;
        let count = window[..bytes].encode_utf16().count();
        u16::try_from(count).unwrap_or(SELECTION_UNKNOWN - 1)
    };
    match selection {
        Some((start, end)) => (window.to_owned(), units(start), units(end)),
        None => (window.to_owned(), SELECTION_UNKNOWN, SELECTION_UNKNOWN),
    }
}

fn floor_boundary(text: &str, mut at: usize) -> usize {
    at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

/// What the focus watcher does after a client's request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Watch {
    /// Not at all: the client stopped asking, or the host does not offer text input.
    Off,
    /// Watch the focus, and read up to `max_text` bytes around the cursor when it is not 0.
    On { max_text: usize },
}

/// What one notice says, without its kind and serial: a change of this is an update.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Context {
    field: Field,
    text: String,
    selection: (u16, u16),
}

/// The session's text input state: what the client asked for, the newest focus and what
/// the client was last told.
#[derive(Debug, Default)]
pub struct TextInputs {
    request: TextInputRequest,
    /// `text_input_surrounding_text` when the client asked.
    surrounding_allowed: bool,
    latest: Option<Focused>,
    /// When the focus last left a text field.
    left_at: Option<Instant>,
    /// What the client was last told while its keyboard is up; `None` when lowered.
    raised: Option<Context>,
    sent_at: Option<Instant>,
    serial: u32,
}

impl TextInputs {
    /// Applies a client's request under the host's `keyboard_follows_focus` (`offered`) and
    /// `text_input_surrounding_text`. Returns what the watcher does now, or `None` for a
    /// malformed request, which changes nothing. Every valid request starts afresh: a client
    /// that asks while a field has the focus hears it at once.
    pub fn on_request(
        &mut self,
        payload: &[u8],
        offered: bool,
        surrounding_allowed: bool,
    ) -> Option<Watch> {
        let request = TextInputRequest::decode(payload)?;
        self.request = TextInputRequest {
            notices: request.notices && offered,
            ..request
        };
        self.surrounding_allowed = surrounding_allowed;
        self.raised = None;
        self.sent_at = None;
        if !self.request.notices {
            self.latest = None;
            self.left_at = None;
            return Some(Watch::Off);
        }
        Some(Watch::On {
            max_text: self.max_text(),
        })
    }

    /// Whether the client asked and the host offers it.
    pub fn watching(&self) -> bool {
        self.request.notices
    }

    /// The most text a notice carries now: 0 unless the client asked and the host allows it.
    fn max_text(&self) -> usize {
        if self.request.surrounding_text && self.surrounding_allowed {
            usize::from(self.request.max_text.min(MAX_TEXT_BYTES))
        } else {
            0
        }
    }

    /// The watcher's newest focus: a text field, or `None` when the focus left them.
    pub fn note(&mut self, focused: Option<Focused>, now: Instant) {
        if !self.request.notices {
            return;
        }
        if focused.is_none() && self.latest.is_some() {
            self.left_at = Some(now);
        }
        self.latest = focused;
    }

    /// The notice to send now, if any: a raise when a field gained the focus, an update when
    /// it changed while raised, a lower once no field has had the focus for
    /// [`LOWER_DELAY`]; at most one per [`INTERVAL`], and the newest focus wins.
    pub fn take(&mut self, now: Instant) -> Option<Vec<u8>> {
        if !self.request.notices {
            return None;
        }
        if self
            .sent_at
            .is_some_and(|at| now.saturating_duration_since(at) < INTERVAL)
        {
            return None;
        }
        let context = self.latest.as_ref().map(|focused| self.context(focused));
        let kind = match (&self.raised, &context) {
            (None, None) => return None,
            (None, Some(_)) => NoticeKind::Raise,
            (Some(raised), Some(context)) if raised == context => return None,
            (Some(_), Some(_)) => NoticeKind::Update,
            (Some(_), None) => {
                if self
                    .left_at
                    .is_some_and(|at| now.saturating_duration_since(at) < LOWER_DELAY)
                {
                    return None;
                }
                NoticeKind::Lower
            }
        };
        self.serial = self.serial.wrapping_add(1);
        self.sent_at = Some(now);
        self.raised = context.clone();
        let notice = match context {
            Some(Context {
                field,
                text,
                selection,
            }) => TextInputNotice {
                kind,
                input_type: field.input_type,
                sensitive: field.sensitive,
                no_autocorrect: field.no_autocorrect,
                no_autocapitalize: field.no_autocapitalize,
                enter_action: field.enter_action,
                serial: self.serial,
                selection_start: selection.0,
                selection_end: selection.1,
                text,
            },
            None => TextInputNotice {
                kind,
                input_type: InputType::Text,
                sensitive: false,
                no_autocorrect: false,
                no_autocapitalize: false,
                enter_action: false,
                serial: self.serial,
                selection_start: SELECTION_UNKNOWN,
                selection_end: SELECTION_UNKNOWN,
                text: String::new(),
            },
        };
        // `context` leaves out the text of sensitive fields and bounds the rest, so the
        // encoder takes every notice made here.
        let encoded = notice.encode();
        debug_assert!(encoded.is_some(), "an unencodable text input notice");
        encoded
    }

    fn context(&self, focused: &Focused) -> Context {
        let max_text = self.max_text();
        let private = focused.field.sensitive || focused.field.input_type == InputType::Password;
        match &focused.surrounding {
            Some(surrounding) if max_text > 0 && !private => {
                let (text, start, end) = bound(surrounding, max_text);
                Context {
                    field: focused.field,
                    text,
                    selection: (start, end),
                }
            }
            _ => Context {
                field: focused.field,
                text: String::new(),
                selection: (SELECTION_UNKNOWN, SELECTION_UNKNOWN),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edit(name: &str) -> FocusedElement {
        FocusedElement {
            control_type: control_type::EDIT,
            enabled: true,
            read_only: Some(false),
            text_pattern: true,
            name: name.into(),
            ..FocusedElement::default()
        }
    }

    fn kind(element: &FocusedElement) -> Option<InputType> {
        classify(element).map(|field| field.input_type)
    }

    #[test]
    fn fields_are_classified_from_uia() {
        assert_eq!(kind(&edit("Name")), Some(InputType::Text));
        assert_eq!(
            kind(&FocusedElement {
                password: true,
                ..edit("PIN")
            }),
            Some(InputType::Password)
        );
        assert_eq!(kind(&edit("Confirm password")), Some(InputType::Password));
        assert_eq!(kind(&edit("Email address")), Some(InputType::Email));
        assert_eq!(kind(&edit("Address and search bar")), Some(InputType::Url));
        assert_eq!(kind(&edit("Search")), Some(InputType::Search));
        assert_eq!(
            kind(&FocusedElement {
                aria_role: "searchbox".into(),
                ..edit("Find")
            }),
            Some(InputType::Search)
        );
        assert_eq!(kind(&edit("Phone")), Some(InputType::Phone));
        assert_eq!(
            kind(&FocusedElement {
                control_type: control_type::SPINNER,
                ..edit("Count")
            }),
            Some(InputType::Number)
        );
        assert_eq!(
            kind(&FocusedElement {
                control_type: control_type::DOCUMENT,
                class_name: "RichEditD2DPT".into(),
                ..edit("Text editor")
            }),
            Some(InputType::Multiline)
        );
        // A browser's text box that is a group or custom control.
        assert_eq!(
            kind(&FocusedElement {
                control_type: 50026,
                aria_role: "textbox".into(),
                read_only: None,
                ..edit("Message")
            }),
            Some(InputType::Text)
        );
    }

    #[test]
    fn elements_that_take_no_text_are_none() {
        for element in [
            // A button, a read-only page, a disabled or read-only edit, a document without
            // a value pattern (a web page), a list box named like a field.
            FocusedElement {
                control_type: 50000,
                ..edit("Search")
            },
            FocusedElement {
                control_type: control_type::DOCUMENT,
                read_only: Some(true),
                ..edit("Page")
            },
            FocusedElement {
                enabled: false,
                ..edit("Name")
            },
            FocusedElement {
                read_only: Some(true),
                ..edit("Name")
            },
            FocusedElement {
                control_type: control_type::DOCUMENT,
                read_only: None,
                ..edit("Page")
            },
            FocusedElement {
                control_type: 50008,
                ..edit("Email")
            },
            FocusedElement::default(),
        ] {
            assert_eq!(classify(&element), None, "{element:?}");
        }
    }

    #[test]
    fn keyboards_match_the_field() {
        let password = classify(&FocusedElement {
            password: true,
            ..edit("x")
        })
        .unwrap();
        assert!(password.sensitive && password.no_autocorrect && password.no_autocapitalize);
        let url = classify(&edit("URL")).unwrap();
        assert!(!url.sensitive && url.no_autocorrect && url.no_autocapitalize);
        assert!(url.enter_action);
        let text = classify(&edit("Name")).unwrap();
        assert!(!text.no_autocorrect && !text.no_autocapitalize && text.enter_action);
        let document = classify(&FocusedElement {
            control_type: control_type::DOCUMENT,
            ..edit("Body")
        })
        .unwrap();
        assert!(!document.enter_action);
    }

    fn around(text: &str, selection: Option<(usize, usize)>) -> Surrounding {
        Surrounding {
            text: text.into(),
            selection,
        }
    }

    #[test]
    fn text_is_bounded_around_the_cursor() {
        assert_eq!(
            bound(&around("hello", Some((5, 5))), 4096),
            ("hello".into(), 5, 5)
        );
        // Centred on the cursor, cut on a character boundary, the selection moved with it.
        let text = format!("{}|{}", "a".repeat(100), "b".repeat(100));
        let (cut, start, end) = bound(&around(&text, Some((100, 101))), 21);
        assert_eq!(cut, format!("{}|{}", "a".repeat(10), "b".repeat(10)));
        assert_eq!((start, end), (10, 11));
        // Multi-byte characters: never split, offsets in UTF-16 (the emoji is two units).
        let text = "äöü😀xyz";
        // After the emoji (5 units): the window ends with the text.
        assert_eq!(
            bound(&around(text, Some((5, 5))), 7),
            ("😀xyz".into(), 2, 2)
        );
        // After ü: the emoji does not fit whole and is left out.
        assert_eq!(bound(&around(text, Some((3, 3))), 7), ("öü".into(), 2, 2));
        // No selection: the end of the text, where a single-line field's cursor is.
        let (cut, start, end) = bound(&around("0123456789", None), 4);
        assert_eq!(
            (cut.as_str(), start, end),
            ("6789", SELECTION_UNKNOWN, SELECTION_UNKNOWN)
        );
        // A selection past the end of the text is clamped.
        assert_eq!(
            bound(&around("abc", Some((9, 2))), 10),
            ("abc".into(), 2, 3)
        );
        // Never more than the protocol's 4096 bytes.
        let (cut, _, _) = bound(&around(&"x".repeat(10_000), Some((0, 0))), 100_000);
        assert_eq!(cut.len(), usize::from(MAX_TEXT_BYTES));
        // A selection longer than the window starts it.
        let (cut, start, end) = bound(&around(&"y".repeat(50), Some((10, 40))), 8);
        assert_eq!((cut.len(), start, end), (8, 0, 8));
    }

    fn request(surrounding_text: bool, max_text: u16) -> [u8; 4] {
        TextInputRequest {
            notices: true,
            surrounding_text,
            max_text,
        }
        .encode()
    }

    fn focused(name: &str, text: &str) -> Option<Focused> {
        Some(Focused {
            field: classify(&edit(name)).unwrap(),
            surrounding: Some(around(text, Some((text.len(), text.len())))),
        })
    }

    fn notice(wire: Option<Vec<u8>>) -> TextInputNotice {
        TextInputNotice::decode(&wire.expect("a notice")).expect("a valid notice")
    }

    #[test]
    fn nothing_goes_to_a_client_that_did_not_ask_or_a_host_that_does_not_offer() {
        let now = Instant::now();
        let mut inputs = TextInputs::default();
        inputs.note(focused("Name", "x"), now);
        assert_eq!(inputs.take(now), None);
        assert!(!inputs.watching());
        // keyboard_follows_focus off: the request is refused.
        assert_eq!(
            inputs.on_request(&request(true, 64), false, true),
            Some(Watch::Off)
        );
        inputs.note(focused("Name", "x"), now);
        assert_eq!(inputs.take(now + INTERVAL), None);
        assert_eq!(inputs.on_request(&[0, 1, 0, 0], true, true), None);
        assert!(!inputs.watching());
    }

    #[test]
    fn raises_updates_and_lowers_are_coalesced() {
        let start = Instant::now();
        let mut inputs = TextInputs::default();
        assert_eq!(
            inputs.on_request(&request(false, 0), true, false),
            Some(Watch::On { max_text: 0 })
        );
        assert!(inputs.watching());
        // Lowered and nothing focused: nothing to say.
        assert_eq!(inputs.take(start), None);
        inputs.note(focused("Name", "x"), start);
        let raise = notice(inputs.take(start));
        assert_eq!(raise.kind, NoticeKind::Raise);
        assert_eq!(raise.input_type, InputType::Text);
        assert_eq!(raise.serial, 1);
        assert!(raise.text.is_empty(), "the client did not ask for text");
        // Unchanged: nothing more.
        assert_eq!(inputs.take(start + INTERVAL), None);
        // Three changes in one interval: only the newest goes out, after it.
        for name in ["Email", "Search", "URL"] {
            inputs.note(focused(name, "x"), start + Duration::from_millis(10));
        }
        assert_eq!(inputs.take(start + Duration::from_millis(20)), None);
        let update = notice(inputs.take(start + INTERVAL));
        assert_eq!(
            (update.kind, update.input_type, update.serial),
            (NoticeKind::Update, InputType::Url, 2)
        );
        // Leaving for a button and coming back within LOWER_DELAY lowers nothing.
        let left = start + INTERVAL * 2;
        inputs.note(None, left);
        assert_eq!(inputs.take(left), None);
        inputs.note(focused("URL", "x"), left + Duration::from_millis(100));
        assert_eq!(inputs.take(left + LOWER_DELAY), None);
        // Leaving for good lowers once LOWER_DELAY has passed.
        let left = left + LOWER_DELAY * 2;
        inputs.note(None, left);
        assert_eq!(
            inputs.take(left + LOWER_DELAY - Duration::from_millis(1)),
            None
        );
        let lower = notice(inputs.take(left + LOWER_DELAY));
        assert_eq!((lower.kind, lower.serial), (NoticeKind::Lower, 3));
        assert_eq!(inputs.take(left + LOWER_DELAY * 4), None);
    }

    #[test]
    fn surrounding_text_needs_the_client_the_setting_and_a_public_field() {
        let now = Instant::now();
        let later = now + INTERVAL;
        // Asked and allowed: bounded to the client's max_text.
        let mut inputs = TextInputs::default();
        assert_eq!(
            inputs.on_request(&request(true, 8), true, true),
            Some(Watch::On { max_text: 8 })
        );
        inputs.note(focused("Name", "hello world"), now);
        let raised = notice(inputs.take(now));
        assert_eq!(raised.text, "lo world");
        assert_eq!((raised.selection_start, raised.selection_end), (8, 8));
        // A password field never carries text, whatever the watcher read.
        inputs.note(
            Some(Focused {
                field: classify(&FocusedElement {
                    password: true,
                    ..edit("x")
                })
                .unwrap(),
                surrounding: Some(around("hunter2", Some((7, 7)))),
            }),
            later,
        );
        let password = notice(inputs.take(later));
        assert_eq!(password.input_type, InputType::Password);
        assert!(password.sensitive && password.text.is_empty());
        assert_eq!(password.selection_start, SELECTION_UNKNOWN);
        // text_input_surrounding_text off: no text even though the client asked.
        let mut inputs = TextInputs::default();
        assert_eq!(
            inputs.on_request(&request(true, 64), true, false),
            Some(Watch::On { max_text: 0 })
        );
        inputs.note(focused("Name", "hello"), now);
        assert!(notice(inputs.take(now)).text.is_empty());
        // The client did not ask: no text even though the host allows it.
        let mut inputs = TextInputs::default();
        inputs.on_request(&request(false, 64), true, true);
        inputs.note(focused("Name", "hello"), now);
        assert!(notice(inputs.take(now)).text.is_empty());
    }

    #[test]
    fn a_new_request_repeats_the_focus_and_stopping_forgets_it() {
        let now = Instant::now();
        let mut inputs = TextInputs::default();
        inputs.on_request(&request(false, 0), true, false);
        inputs.note(focused("Name", ""), now);
        assert_eq!(notice(inputs.take(now)).serial, 1);
        // Asked again at once: the focus is repeated as a raise, with a newer serial.
        inputs.on_request(&request(false, 0), true, false);
        let again = notice(inputs.take(now));
        assert_eq!((again.kind, again.serial), (NoticeKind::Raise, 2));
        // Stopped: nothing, also not a lower, and later focus is ignored.
        assert_eq!(
            inputs.on_request(&TextInputRequest::default().encode(), true, false),
            Some(Watch::Off)
        );
        inputs.note(None, now);
        inputs.note(focused("Name", ""), now);
        assert_eq!(inputs.take(now + LOWER_DELAY * 2), None);
        // Asked once more: the serial keeps counting, so the client's newest-wins check holds.
        inputs.on_request(&request(false, 0), true, false);
        inputs.note(focused("Name", ""), now);
        assert_eq!(notice(inputs.take(now + LOWER_DELAY * 2)).serial, 3);
    }
}
