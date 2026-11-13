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
//! sensitive field, cut to the client's `max_text` around the cursor ([`bound`]). The focused
//! element's id goes out only to a client that asked for element ids (`TextInputVersion` 2);
//! every other client gets the version 1 notice, byte for byte.
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
    pub const TOOL_TIP: i32 = 50022;
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
    /// The element is an edit inside a spinner: the text box of a number box (WinUI
    /// `NumberBox`, a WinForms `NumericUpDown`, a Win32 up-down buddy), which has the focus
    /// while the spinner around it says what the field holds.
    pub in_spinner: bool,
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
    } else if element.control_type == control_type::SPINNER
        || role == "spinbutton"
        || element.in_spinner
    {
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

/// Whether a focus event for `element` leaves the focus where it was, so the watcher reports
/// nothing: a tooltip shows over the field without taking its keys, and a window of the host's
/// own (`own`) is not the user's app. Either would lower the keyboard and raise it again a
/// moment later.
pub fn passes_over(element: &FocusedElement, own: bool) -> bool {
    own || element.control_type == control_type::TOOL_TIP
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
    /// Which element has the focus, never 0: the same while one element lives, another for
    /// another element. Two fields of one kind look the same without their text, and the
    /// client must still hear the move, or it keeps composing a word typed into the field
    /// before. A client that asked for element ids gets it, to keep its keyboard closed only
    /// for the field the user closed it on.
    pub element: u32,
    pub surrounding: Option<Surrounding>,
}

/// The [`Focused::element`] of an element with this UI Automation runtime id, never 0: the
/// runtime id stays the same while the element lives and differs between live elements, so
/// its FNV-1a hash does too, but for a 1 in 2^32 collision.
pub fn element_id(runtime_id: &[i32]) -> u32 {
    let hash = runtime_id
        .iter()
        .flat_map(|part| part.to_le_bytes())
        .fold(0x811C_9DC5_u32, |hash, byte| {
            (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193)
        });
    hash.max(1)
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
                Some(start.saturating_sub((max_bytes - (end - start)) / 2))
            }
            Some((start, _)) => Some(start),
            None => None,
        };
        match wanted_from {
            Some(wanted) => {
                let from = floor_boundary(text, wanted.min(text.len() - max_bytes));
                (from, floor_boundary(text, from + max_bytes))
            }
            // The end is kept whole: the window starts at the next character rather than
            // losing the last one to a start that fell inside a character.
            None => {
                let mut from = text.len() - max_bytes;
                while !text.is_char_boundary(from) {
                    from += 1;
                }
                (from, text.len())
            }
        }
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
    element: u32,
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
        // A client that did not ask gets version 1 notices, without the id.
        let ids = self.request.element_ids;
        let notice = match context {
            Some(Context {
                field,
                element,
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
                element: if ids { element } else { 0 },
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
                element: 0,
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
                    element: focused.element,
                    text,
                    selection: (start, end),
                }
            }
            _ => Context {
                field: focused.field,
                element: focused.element,
                text: String::new(),
                selection: (SELECTION_UNKNOWN, SELECTION_UNKNOWN),
            },
        }
    }
}

#[cfg(test)]
mod tests;
