//! The host's text field notices (0x553B) and the keyboard layout report (0x553E).
//!
//! A raise starts SDL text input with the field's type where SDL can show an on-screen
//! keyboard (a Steam Deck, a tablet), so it opens over the stream; a lower, or opening the
//! menu, stops it. A physical keyboard is untouched: without on-screen keyboard support no text
//! input starts, and with it keys still go as keys, so only text that came without a key (a
//! touch keyboard's characters) goes as UTF-8. The notice has no caret position, so the IME
//! window keeps SDL's default place.

use std::ffi::CStr;

use rubylight_protocol::keyboard_layout::{KeyboardLayout, LayoutSource, MAX_TAG_BYTES};
use rubylight_protocol::text_input::{InputType, NoticeKind, TextInputNotice, TextInputRequest};
use sdl3_sys::everything::*;

/// How SDL is asked to set up the on-screen keyboard for a field.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Props {
    pub kind: SDL_TextInputType,
    pub capitalization: SDL_Capitalization,
    pub autocorrect: bool,
    pub multiline: bool,
}

/// By value: sdl3-sys's enums have no `Debug`.
impl std::fmt::Debug for Props {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Props")
            .field("kind", &self.kind.0)
            .field("capitalization", &self.capitalization.0)
            .field("autocorrect", &self.autocorrect)
            .field("multiline", &self.multiline)
            .finish()
    }
}

pub fn props(notice: &TextInputNotice) -> Props {
    let kind = match notice.input_type {
        InputType::Number | InputType::Decimal | InputType::Phone => SDL_TEXTINPUT_TYPE_NUMBER,
        InputType::Email => SDL_TEXTINPUT_TYPE_TEXT_EMAIL,
        InputType::Password => SDL_TEXTINPUT_TYPE_TEXT_PASSWORD_HIDDEN,
        InputType::Text | InputType::Url | InputType::Search | InputType::Multiline => SDL_TEXTINPUT_TYPE_TEXT,
    };
    let text = kind == SDL_TEXTINPUT_TYPE_TEXT;
    Props {
        kind,
        capitalization: if text && !notice.no_autocapitalize && !notice.sensitive {
            SDL_CAPITALIZE_SENTENCES
        } else {
            SDL_CAPITALIZE_NONE
        },
        autocorrect: text && !notice.no_autocorrect && !notice.sensitive,
        multiline: notice.input_type == InputType::Multiline,
    }
}

/// What a stream asks of a host that offers notices: raises and lowers, never the text around
/// the cursor, which nothing here shows.
pub fn request() -> TextInputRequest {
    TextInputRequest { notices: true, surrounding_text: false, max_text: 0 }
}

#[derive(Default)]
pub struct TextField {
    /// The field the host last raised, until it lowers it.
    focused: Option<Props>,
    /// What SDL text input was started with, if this started it.
    started: Option<Props>,
    /// A key that types went to the host since the last text event, so that text is its echo.
    key_typed: bool,
}

impl TextField {
    pub fn notice(&mut self, notice: &TextInputNotice) {
        match notice.kind {
            NoticeKind::Lower => {
                eprintln!("nightjar: text field: lower");
                self.focused = None;
            }
            NoticeKind::Raise | NoticeKind::Update => {
                eprintln!(
                    "nightjar: text field: {:?} {:?}{}",
                    notice.kind,
                    notice.input_type,
                    if notice.sensitive { " (sensitive)" } else { "" }
                );
                self.focused = Some(props(notice));
            }
        }
    }

    /// What SDL text input should be: the focused field's, unless the menu (egui owns text
    /// input then) is open or SDL has no on-screen keyboard here.
    fn wanted(&self, menu_visible: bool, screen_keyboard: bool) -> Option<Props> {
        self.focused.filter(|_| !menu_visible && screen_keyboard)
    }

    /// Starts, restarts or stops SDL text input to match; on the main thread.
    pub fn apply(&mut self, window: *mut SDL_Window, menu_visible: bool) {
        if self.focused.is_none() && self.started.is_none() {
            return;
        }
        // SAFETY: SDL keyboard calls on the main thread with a live window.
        unsafe {
            let wanted = self.wanted(menu_visible, SDL_HasScreenKeyboardSupport());
            if wanted == self.started {
                return;
            }
            if self.started.take().is_some() && !SDL_StopTextInput(window) {
                eprintln!("nightjar: text field: could not stop text input: {}", sdl_error());
            }
            let Some(props) = wanted else {
                return;
            };
            let id = SDL_CreateProperties();
            if id == 0 {
                eprintln!("nightjar: text field: could not start text input: {}", sdl_error());
                return;
            }
            SDL_SetNumberProperty(id, SDL_PROP_TEXTINPUT_TYPE_NUMBER, i64::from(props.kind.0));
            SDL_SetNumberProperty(id, SDL_PROP_TEXTINPUT_CAPITALIZATION_NUMBER, i64::from(props.capitalization.0));
            SDL_SetBooleanProperty(id, SDL_PROP_TEXTINPUT_AUTOCORRECT_BOOLEAN, props.autocorrect);
            SDL_SetBooleanProperty(id, SDL_PROP_TEXTINPUT_MULTILINE_BOOLEAN, props.multiline);
            let started = SDL_StartTextInputWithProperties(window, id);
            SDL_DestroyProperties(id);
            if started {
                self.started = Some(props);
                self.key_typed = false;
            } else {
                eprintln!("nightjar: text field: could not start text input: {}", sdl_error());
            }
        }
    }

    /// Stops the text input this started, at the end of the stream.
    pub fn stop(&mut self, window: *mut SDL_Window) {
        self.focused = None;
        self.apply(window, true);
    }

    /// A key went to the host (or repeats there); `types` when it can type a character, as
    /// [`types`] says.
    pub fn key_down(&mut self, types: bool) {
        self.key_typed |= types;
    }

    /// The text an SDL text event should send as UTF-8: only while this started text input,
    /// and only text no key typed already.
    pub fn text<'a>(&mut self, text: &'a str) -> Option<&'a str> {
        let echo = std::mem::take(&mut self.key_typed);
        (self.started.is_some() && !echo && !text.is_empty() && !text.chars().any(char::is_control)).then_some(text)
    }
}

fn sdl_error() -> String {
    // SAFETY: SDL returns a NUL-terminated string.
    unsafe { CStr::from_ptr(SDL_GetError()).to_string_lossy().into_owned() }
}

/// Whether a key can type a character with these modifiers: a printable keycode (not a
/// modifier, an arrow, Enter or Backspace), without Ctrl or the Windows key unless with Alt
/// (AltGr is Ctrl+Alt on Windows). A held key's repeats type too.
pub fn types(key: SDL_Keycode, held: SDL_Keymod) -> bool {
    let has = |flag: SDL_Keymod| held.0 & flag.0 != 0;
    key.0 & SDLK_SCANCODE_MASK.0 == 0
        && key.0 >= SDLK_SPACE.0
        && key != SDLK_DELETE
        && !has(SDL_KMOD_GUI)
        && (!has(SDL_KMOD_CTRL) || has(SDL_KMOD_ALT))
}

/// The keyboard layout this client types with, for hosts that take a report.
pub fn current_layout() -> Option<KeyboardLayout> {
    #[cfg(windows)]
    {
        windows_layout()
    }
    #[cfg(target_os = "linux")]
    {
        ["/etc/default/keyboard", "/etc/vconsole.conf"]
            .iter()
            .find_map(|path| std::fs::read_to_string(path).ok().and_then(|text| xkb_layout(&text)))
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        None
    }
}

#[cfg(windows)]
#[link(name = "user32")]
extern "system" {
    fn GetKeyboardLayoutNameW(name: *mut u16) -> i32;
}

#[cfg(windows)]
fn windows_layout() -> Option<KeyboardLayout> {
    let mut name = [0u16; 9];
    // SAFETY: KL_NAMELENGTH (9) wide characters, as the call requires.
    if unsafe { GetKeyboardLayoutNameW(name.as_mut_ptr()) } == 0 {
        eprintln!("nightjar: could not read the keyboard layout: {}", std::io::Error::last_os_error());
        return None;
    }
    let len = name.iter().position(|&c| c == 0).unwrap_or(name.len());
    klid_layout(&String::from_utf16_lossy(&name[..len]))
}

/// A Windows layout name such as `00000407` (German) as a report.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn klid_layout(name: &str) -> Option<KeyboardLayout> {
    if name.len() != 8 {
        return None;
    }
    let klid = u32::from_str_radix(name, 16).ok()?;
    Some(KeyboardLayout { source: LayoutSource::Klid, klid, tag: name.to_ascii_uppercase() })
}

/// The first layout and its variant from `/etc/default/keyboard` or `/etc/vconsole.conf`
/// (`XKBLAYOUT="de,us"`, `XKBVARIANT="nodeadkeys,"`), as `de(nodeadkeys)`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn xkb_layout(text: &str) -> Option<KeyboardLayout> {
    let value = |key: &str| {
        text.lines().filter_map(|line| line.trim().strip_prefix(key)?.strip_prefix('=')).next_back().map(|value| {
            let value = value.trim().trim_matches(|c| c == '"' || c == '\'');
            value.split(',').next().unwrap_or("").trim().to_owned()
        })
    };
    let layout = value("XKBLAYOUT").filter(|layout| !layout.is_empty())?;
    let tag = match value("XKBVARIANT").filter(|variant| !variant.is_empty()) {
        Some(variant) => format!("{layout}({variant})"),
        None => layout,
    };
    (tag.len() <= MAX_TAG_BYTES && tag.bytes().all(|b| b.is_ascii_graphic()))
        .then_some(KeyboardLayout { source: LayoutSource::Xkb, klid: 0, tag })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notice(kind: NoticeKind, input_type: InputType) -> TextInputNotice {
        TextInputNotice {
            kind,
            input_type,
            sensitive: false,
            no_autocorrect: false,
            no_autocapitalize: false,
            enter_action: false,
            serial: 1,
            selection_start: rubylight_protocol::text_input::SELECTION_UNKNOWN,
            selection_end: rubylight_protocol::text_input::SELECTION_UNKNOWN,
            text: String::new(),
        }
    }

    #[test]
    fn fields_set_up_the_keyboard() {
        let text = props(&notice(NoticeKind::Raise, InputType::Text));
        assert_eq!(text, Props { kind: SDL_TEXTINPUT_TYPE_TEXT, capitalization: SDL_CAPITALIZE_SENTENCES, autocorrect: true, multiline: false });
        let password = props(&notice(NoticeKind::Raise, InputType::Password));
        assert_eq!(password.kind.0, SDL_TEXTINPUT_TYPE_TEXT_PASSWORD_HIDDEN.0);
        assert!(!password.autocorrect, "nothing typed into a password is suggested");
        assert_eq!(password.capitalization.0, SDL_CAPITALIZE_NONE.0);
        for input_type in [InputType::Number, InputType::Decimal, InputType::Phone] {
            assert_eq!(props(&notice(NoticeKind::Raise, input_type)).kind.0, SDL_TEXTINPUT_TYPE_NUMBER.0);
        }
        assert_eq!(props(&notice(NoticeKind::Raise, InputType::Email)).kind.0, SDL_TEXTINPUT_TYPE_TEXT_EMAIL.0);
        assert!(props(&notice(NoticeKind::Raise, InputType::Multiline)).multiline);
        let mut quiet = notice(NoticeKind::Raise, InputType::Text);
        quiet.no_autocorrect = true;
        quiet.no_autocapitalize = true;
        assert_eq!(props(&quiet), Props { kind: SDL_TEXTINPUT_TYPE_TEXT, capitalization: SDL_CAPITALIZE_NONE, autocorrect: false, multiline: false });
        let mut sensitive = notice(NoticeKind::Raise, InputType::Text);
        sensitive.sensitive = true;
        assert!(!props(&sensitive).autocorrect);
    }

    #[test]
    fn the_request_asks_for_notices_only() {
        let wire = request().encode();
        assert_eq!(TextInputRequest::decode(&wire), Some(TextInputRequest { notices: true, surrounding_text: false, max_text: 0 }));
    }

    #[test]
    fn text_input_follows_the_field_but_not_over_the_menu() {
        let mut field = TextField::default();
        assert_eq!(field.wanted(false, true), None);
        field.notice(&notice(NoticeKind::Raise, InputType::Number));
        assert_eq!(field.wanted(false, true).map(|p| p.kind.0), Some(SDL_TEXTINPUT_TYPE_NUMBER.0));
        assert_eq!(field.wanted(true, true), None, "egui owns text input over the menu");
        assert_eq!(field.wanted(false, false), None, "no on-screen keyboard: a physical one types");
        field.notice(&notice(NoticeKind::Update, InputType::Password));
        assert_eq!(field.wanted(false, true).map(|p| p.kind.0), Some(SDL_TEXTINPUT_TYPE_TEXT_PASSWORD_HIDDEN.0));
        field.notice(&notice(NoticeKind::Lower, InputType::Text));
        assert_eq!(field.wanted(false, true), None);
    }

    #[test]
    fn only_text_without_a_key_goes_as_text() {
        let mut field = TextField::default();
        assert_eq!(field.text("a"), None, "text input this did not start");
        field.started = Some(props(&notice(NoticeKind::Raise, InputType::Text)));
        assert_eq!(field.text("ä"), Some("ä"), "a touch keyboard's character");
        field.key_down(false);
        assert_eq!(field.text("Ä"), Some("Ä"), "after Shift, still no key typed it");
        field.key_down(true);
        assert_eq!(field.text("a"), None, "the key already typed it");
        assert_eq!(field.text("b"), Some("b"), "one echo per key");
        assert_eq!(field.text("\r"), None);
        assert_eq!(field.text(""), None);
        // An arrow sets nothing, so a touch keyboard's next character still goes.
        field.key_down(types(SDLK_RIGHT, SDL_KMOD_NONE));
        assert_eq!(field.text("c"), Some("c"));
    }

    #[test]
    fn only_keys_that_type_count() {
        assert!(types(SDLK_A, SDL_KMOD_NONE));
        assert!(types(SDLK_A, SDL_KMOD_LSHIFT));
        assert!(types(SDLK_SPACE, SDL_KMOD_NONE));
        assert!(types(SDLK_Q, SDL_Keymod(SDL_KMOD_LCTRL.0 | SDL_KMOD_RALT.0)), "AltGr+Q types @ on a German layout");
        for key in [SDLK_LSHIFT, SDLK_RALT, SDLK_LGUI, SDLK_CAPSLOCK, SDLK_RIGHT, SDLK_F5, SDLK_RETURN, SDLK_BACKSPACE, SDLK_TAB, SDLK_ESCAPE, SDLK_DELETE] {
            assert!(!types(key, SDL_KMOD_NONE), "keycode {:#x}", key.0);
        }
        assert!(!types(SDLK_A, SDL_KMOD_LCTRL), "Ctrl+A is a shortcut");
        assert!(!types(SDLK_A, SDL_KMOD_LGUI));
    }

    #[test]
    fn layouts_are_read_for_the_report() {
        assert_eq!(klid_layout("00000407"), Some(KeyboardLayout { source: LayoutSource::Klid, klid: 0x0407, tag: "00000407".into() }));
        assert_eq!(klid_layout("0001040c").unwrap().klid, 0x0001_040C);
        assert_eq!(klid_layout("407"), None);
        assert_eq!(klid_layout("0000040G"), None);
        let debian = "# KEYBOARD CONFIGURATION FILE\nXKBMODEL=\"pc105\"\nXKBLAYOUT=\"de,us\"\nXKBVARIANT=\"nodeadkeys,\"\nXKBOPTIONS=\"\"\n";
        assert_eq!(xkb_layout(debian), Some(KeyboardLayout { source: LayoutSource::Xkb, klid: 0, tag: "de(nodeadkeys)".into() }));
        assert_eq!(xkb_layout("KEYMAP=de-latin1\nXKBLAYOUT=ch\n").unwrap().tag, "ch");
        assert_eq!(xkb_layout("XKBLAYOUT=\"fr\"\nXKBVARIANT=\"\"\n").unwrap().tag, "fr");
        assert_eq!(xkb_layout("KEYMAP=us\n"), None);
        assert_eq!(xkb_layout("XKBLAYOUT=\"\"\n"), None);
        assert_eq!(xkb_layout("XKBLAYOUT=\"de fr\"\n"), None, "not a tag the wire carries");
    }
}
