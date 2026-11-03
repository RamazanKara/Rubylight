//! Keyboard shortcuts: Nightjar's own keys and controller buttons ([`KEYS`], [`PAD`]), the one
//! list the key handling and the help lists share, and the user's saved key chords that the stream
//! controls send to the PC ([`Shortcuts`], in the Android app's file format).

use std::borrow::Cow;
use std::collections::HashSet;
use crate::strings::{en, tr, trf};
use std::io::Read;
use std::path::Path;

use nightjar_core::input::{InputError, InputQueue};
use nightjar_core::input::packets::modifier;
use sdl3_sys::everything::*;

pub use nightjar_policy::shortcuts::{key_code, Shortcut, ShortcutError, Shortcuts, HOLD_MS, MAX_BYTES};
use nightjar_policy::shortcuts::{self as policy, Bound, Client, HotkeyBindings, HotkeyError};

/// Why a shortcut file or chord was refused, in the user's language.
pub fn error_text(error: &ShortcutError) -> String {
    match error {
        ShortcutError::TooLarge => tr("Shortcut file is larger than 64 KiB.").into(),
        ShortcutError::Invalid(detail) => trf("Invalid shortcut file: {}", &[detail]),
        ShortcutError::Version => tr("Unsupported shortcut file version.").into(),
        ShortcutError::TooMany => tr("You can save up to 32 shortcuts.").into(),
        ShortcutError::BadId => tr("Invalid or repeated shortcut ID.").into(),
        ShortcutError::BadName => tr("Use a name between 1 and 80 characters without control characters.").into(),
        ShortcutError::ChordTooLong => tr("Use a chord of at most 256 characters.").into(),
        ShortcutError::ChordLength => tr("Use between 1 and 8 keys in each chord.").into(),
        ShortcutError::UnsupportedKey(key) => trf("Unsupported key: {}", &[key]),
        ShortcutError::RepeatedKey(key) => trf("Repeated key: {}", &[key]),
        ShortcutError::Gone => tr("Shortcut no longer exists.").into(),
    }
}

pub fn parse(text: &str) -> Result<Shortcuts, String> {
    Shortcuts::parse(text).map_err(|e| error_text(&e))
}

pub fn read(path: &Path) -> Result<Shortcuts, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut text = String::new();
    file.take((MAX_BYTES + 1) as u64).read_to_string(&mut text).map_err(|e| e.to_string())?;
    parse(&text)
}

/// Saves a chord typed in the launcher's editor (`CTRL + SHIFT + ESC`) over `id`, or as a new
/// shortcut.
pub fn save(shortcuts: &mut Shortcuts, id: Option<&str>, name: &str, chord: &str) -> Result<(), String> {
    shortcuts.save_chord(id, name, chord, "desktop").map(drop).map_err(|e| error_text(&e))
}

fn modifier(code: u16) -> u8 {
    match code {
        0xA0 | 0xA1 => modifier::SHIFT, 0xA2 | 0xA3 => modifier::CTRL,
        0xA4 | 0xA5 => modifier::ALT, 0x5B | 0x5C => modifier::META,
        _ => 0,
    }
}

/// A chord pressed on the PC, its keys down until [`release`] lets go of them.
#[derive(Debug, PartialEq, Eq)]
pub struct Held {
    /// The keys this chord pressed, in their order.
    keys: Vec<u16>,
    /// The modifiers the user held on the keyboard as it was pressed.
    held_modifiers: u8,
}

/// Presses a chord's keys, modifiers first, leaving out keys the user holds; [`release`] lets go of
/// them once it has been held for [`HOLD_MS`].
pub fn press(keys: &[String], held: &HashSet<u16>, queue: &mut InputQueue) -> Result<Held, InputError> {
    let mut keys: Vec<_> = keys.iter().map(|name| key_code(name).ok_or(InputError::Invalid)).collect::<Result<_, _>>()?;
    keys.retain(|key| !held.contains(key));
    keys.sort_by_key(|code| modifier(*code) == 0);
    if queue.is_holding() { return Err(InputError::Stopped); }
    // Reserve releases too, so a chord is never pressed without room to let go of it.
    if queue.len() + keys.len() * 2 > nightjar_core::input::queue::MAX_QUEUED { return Err(InputError::QueueFull); }
    let held_modifiers = held.iter().fold(0, |mask, code| mask | modifier(*code));
    let mut pressed = Held { keys: Vec::new(), held_modifiers };
    for key in keys {
        let modifiers = pressed.keys.iter().fold(held_modifiers | modifier(key), |mask, code| mask | modifier(*code));
        queue.send_keyboard(0x8000 | key, true, modifiers, 0)?;
        pressed.keys.push(key);
    }
    Ok(pressed)
}

/// Lets go of a held chord's keys in reverse. A key not taken stays in `chord` for another try.
pub fn release(chord: &mut Held, queue: &mut InputQueue) -> Result<(), InputError> {
    while let Some((&key, rest)) = chord.keys.split_last() {
        let modifiers = rest.iter().fold(chord.held_modifiers, |mask, code| mask | modifier(*code));
        queue.send_keyboard(0x8000 | key, false, modifiers, 0)?;
        chord.keys.pop();
    }
    Ok(())
}

// Nightjar's own keys and controller buttons.
//
// The one list of what Nightjar's keys and buttons do. The handlers look their keys up here
// (main.rs: the launcher's F11 and F12 and its controller navigation; stream/input.rs: the
// stream hotkeys; gamepad.rs: the stream controls combo), and the Help page and the stream's
// shortcut sheet draw these same entries, so no list names a key that does something else. The
// stream hotkeys come from nightjar-policy's table, which the Android app shares, with the
// user's changes ([`record_hotkey`]). The keys egui handles on the launcher's own pages (F2 on a
// card, Ctrl+N, the zoom, Delete) are listed here too; a test presses each of them and checks it
// does what this says.

/// What one of Nightjar's keys or buttons does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    Help,
    Move,
    Select,
    Back,
    Options,
    AddPc,
    Unstar,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    Fullscreen,
    OpenSettings,
    PreviousTab,
    NextTab,
    Screenshot,
    StreamControls,
    StreamShortcuts,
    Disconnect,
    Capture,
    MouseMode,
    Overlay,
    SendClipboard,
    FetchClipboard,
    Minimize,
    ControllerMouse,
}

impl Action {
    /// What the lists call it, in English (drawn through `tr`).
    pub fn label(self) -> &'static str {
        match self {
            Self::Help => en("Help"),
            Self::Move => en("Move between items"),
            Self::Select => en("Open or select"),
            Self::Back => en("Go back"),
            Self::Options => en("Options for a PC or game"),
            Self::AddPc => en("Add a PC (on the PCs page)"),
            Self::Unstar => en("Remove a favourite"),
            Self::ZoomIn => en("Zoom in"),
            Self::ZoomOut => en("Zoom out"),
            Self::ZoomReset => en("Reset zoom"),
            Self::Fullscreen => en("Full screen"),
            Self::OpenSettings => en("Settings"),
            Self::PreviousTab => en("Previous tab"),
            Self::NextTab => en("Next tab"),
            Self::Screenshot => en("Save a screenshot"),
            Self::StreamControls => en("Stream controls"),
            Self::StreamShortcuts => en("Shortcuts & help"),
            Self::Disconnect => en("Disconnect"),
            Self::Capture => en("Release or capture input"),
            Self::MouseMode => en("Switch mouse mode"),
            Self::Overlay => en("Performance overlay"),
            Self::SendClipboard => en("Send clipboard to PC"),
            Self::FetchClipboard => en("Copy PC clipboard here"),
            Self::Minimize => en("Minimize the window"),
            Self::ControllerMouse => en("Controller mouse"),
        }
    }

    pub fn from_hotkey(id: &str) -> Option<Action> {
        HOTKEY_ACTIONS.iter().find(|(hotkey, _)| *hotkey == id).map(|(_, action)| *action)
    }
}

/// The desktop's stream hotkeys by their ID in nightjar-policy's table.
const HOTKEY_ACTIONS: [(&str, Action); 10] = [
    ("stream_menu", Action::StreamControls),
    ("help", Action::StreamShortcuts),
    ("quit", Action::Disconnect),
    ("capture", Action::Capture),
    ("mouse_mode", Action::MouseMode),
    ("overlay", Action::Overlay),
    ("send_clipboard", Action::SendClipboard),
    ("fetch_clipboard", Action::FetchClipboard),
    ("fullscreen", Action::Fullscreen),
    ("minimize", Action::Minimize),
];

/// Where a key or button works.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Launcher,
    Stream,
    /// The launcher and a stream alike.
    Anywhere,
}

impl Scope {
    fn covers(self, scope: Scope) -> bool {
        self == scope || self == Scope::Anywhere
    }
}

/// The key a chord ends in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Tab,
    N,
    F1,
    F2,
    F10,
    F11,
    F12,
    Question,
    Plus,
    Minus,
    Zero,
    Escape,
    Enter,
    Delete,
    Arrows,
    /// Any key a stream hotkey may use, by its Windows virtual-key code (nightjar-policy's names).
    Vk(u16),
}

impl Key {
    /// The name on the key, in English (drawn through `tr`).
    pub fn cap(self) -> Cow<'static, str> {
        Cow::Borrowed(match self {
            Key::Tab => "Tab",
            Key::N => "N",
            Key::F1 => "F1",
            Key::F2 => "F2",
            Key::F10 => "F10",
            Key::F11 => "F11",
            Key::F12 => "F12",
            Key::Question => "?",
            Key::Plus => "+",
            Key::Minus => "−",
            Key::Zero => "0",
            Key::Escape => "Esc",
            Key::Enter => en("Enter"),
            Key::Delete => en("Del"),
            Key::Arrows => en("Arrow keys"),
            Key::Vk(code) => return Cow::Owned(policy::key_label(&policy::key_name(code))),
        })
    }

    /// The physical key SDL reports (stream chords are positional, as the PC's keys are).
    pub fn scancode(self) -> Option<SDL_Scancode> {
        Some(match self {
            Key::Tab => SDL_SCANCODE_TAB,
            Key::N => SDL_SCANCODE_N,
            Key::F1 => SDL_SCANCODE_F1,
            Key::F2 => SDL_SCANCODE_F2,
            Key::F10 => SDL_SCANCODE_F10,
            Key::F11 => SDL_SCANCODE_F11,
            Key::F12 => SDL_SCANCODE_F12,
            Key::Zero => SDL_SCANCODE_0,
            Key::Escape => SDL_SCANCODE_ESCAPE,
            Key::Enter => SDL_SCANCODE_RETURN,
            Key::Delete => SDL_SCANCODE_DELETE,
            // The first scancode that sends this key to the PC.
            Key::Vk(code) => {
                return (0..SDL_SCANCODE_COUNT.0).map(SDL_Scancode).find(|s| crate::keymap::virtual_key(*s, true).is_some_and(|(vk, _)| vk == code));
            }
            // Where these sit depends on the layout; the launcher reads them from egui.
            Key::Question | Key::Plus | Key::Minus | Key::Arrows => return None,
        })
    }

    /// The keys egui reports for it in the launcher.
    pub fn egui(self) -> &'static [egui::Key] {
        use egui::Key as E;
        match self {
            Key::Tab => &[E::Tab],
            Key::N => &[E::N],
            Key::F1 => &[E::F1],
            Key::F2 => &[E::F2],
            Key::F10 => &[E::F10],
            Key::F11 => &[E::F11],
            Key::F12 => &[E::F12],
            Key::Question => &[E::Questionmark],
            Key::Plus => &[E::Plus, E::Equals],
            Key::Minus => &[E::Minus],
            Key::Zero => &[E::Num0],
            Key::Escape => &[E::Escape],
            Key::Enter => &[E::Enter],
            Key::Delete => &[E::Delete],
            Key::Arrows => &[E::ArrowUp, E::ArrowDown, E::ArrowLeft, E::ArrowRight],
            // Stream hotkeys only.
            Key::Vk(_) => &[],
        }
    }
}

/// Modifiers held with a key, then the key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Chord {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub key: Key,
}

impl Chord {
    pub const fn key(key: Key) -> Self {
        Self { ctrl: false, alt: false, shift: false, key }
    }

    pub const fn ctrl(key: Key) -> Self {
        Self { ctrl: true, ..Self::key(key) }
    }

    pub const fn shift(key: Key) -> Self {
        Self { shift: true, ..Self::key(key) }
    }

    /// Ctrl+Alt+Shift and the key: Nightjar's own chords in a stream, which never reach the PC.
    pub const fn nightjar(key: Key) -> Self {
        Self { ctrl: true, alt: true, shift: true, key }
    }

    pub fn is_nightjar(self) -> bool {
        self.ctrl && self.alt && self.shift
    }

    /// Whether the chord types a character into a text field: a printable key without Ctrl or Alt.
    pub fn types(self) -> bool {
        !self.ctrl && !self.alt && matches!(self.key, Key::N
            | Key::Question | Key::Plus | Key::Minus | Key::Zero)
    }

    /// A stream hotkey's chord in nightjar-policy's saved form (Ctrl, Alt and Shift, then the key).
    pub fn of_keys(keys: &[String]) -> Self {
        let (modifiers, vk) = policy::hotkey_code(keys);
        Self { ctrl: modifiers & 0x02 != 0, alt: modifiers & 0x04 != 0, shift: modifiers & 0x01 != 0, key: Key::Vk(vk) }
    }

    /// The names on the keys, left to right, in English (drawn through `tr`).
    pub fn caps(self) -> Vec<Cow<'static, str>> {
        let mut caps = Vec::with_capacity(4);
        if self.ctrl { caps.push(Cow::Borrowed(en("Ctrl"))); }
        if self.alt { caps.push(Cow::Borrowed("Alt")); }
        if self.shift { caps.push(Cow::Borrowed(en("Shift"))); }
        caps.push(self.key.cap());
        caps
    }

    /// The chord as text in the language in use: "Ctrl+Alt+Shift+Q" ("Strg+Alt+Umschalt+Q").
    pub fn text(self) -> String {
        self.caps().iter().map(|cap| tr(cap)).collect::<Vec<_>>().join("+")
    }

    /// Whether SDL's modifier state holds this chord's modifiers (more may be held).
    pub fn held(self, mods: SDL_Keymod) -> bool {
        let has = |flag: SDL_Keymod| mods.0 & flag.0 != 0;
        (!self.ctrl || has(SDL_KMOD_CTRL)) && (!self.alt || has(SDL_KMOD_ALT)) && (!self.shift || has(SDL_KMOD_SHIFT))
    }

    /// Whether egui saw the chord this frame (more modifiers may be held).
    pub fn pressed(self, input: &egui::InputState) -> bool {
        let m = input.modifiers;
        (!self.ctrl || m.ctrl || m.command) && (!self.alt || m.alt) && (!self.shift || m.shift)
            && self.key.egui().iter().any(|key| input.key_pressed(*key))
    }
}

/// One action's keys in one place: any of `chords` does it.
#[derive(Clone, Debug)]
pub struct KeyBinding {
    pub action: Action,
    pub scope: Scope,
    pub chords: Cow<'static, [Chord]>,
}

const fn keys(action: Action, scope: Scope, chords: &'static [Chord]) -> KeyBinding {
    KeyBinding { action, scope, chords: Cow::Borrowed(chords) }
}

/// Every key Nightjar answers to itself besides the stream hotkeys ([`keys_in`] adds them), in
/// the order the lists show them.
pub const KEYS: &[KeyBinding] = &[
    keys(Action::Help, Scope::Launcher, &[Chord::key(Key::F1), Chord::key(Key::Question)]),
    keys(Action::Move, Scope::Launcher, &[Chord::key(Key::Arrows), Chord::key(Key::Tab)]),
    keys(Action::Select, Scope::Launcher, &[Chord::key(Key::Enter)]),
    keys(Action::Back, Scope::Launcher, &[Chord::key(Key::Escape)]),
    keys(Action::Options, Scope::Launcher, &[Chord::key(Key::F2), Chord::shift(Key::F10)]),
    keys(Action::AddPc, Scope::Launcher, &[Chord::ctrl(Key::N)]),
    keys(Action::Unstar, Scope::Launcher, &[Chord::key(Key::Delete)]),
    keys(Action::ZoomIn, Scope::Launcher, &[Chord::ctrl(Key::Plus)]),
    keys(Action::ZoomOut, Scope::Launcher, &[Chord::ctrl(Key::Minus)]),
    keys(Action::ZoomReset, Scope::Launcher, &[Chord::ctrl(Key::Zero)]),
    keys(Action::Fullscreen, Scope::Launcher, &[Chord::key(Key::F11)]),
    keys(Action::Screenshot, Scope::Anywhere, &[Chord::key(Key::F12)]),
];

// The user's stream hotkeys, as stream_hotkeys.json saves them. The key handling and the lists
// read them at each use, so a change applies at once, in a running stream too. Each test thread
// keeps its own (`with_hotkeys` after the tests), so a test that changes them changes no other
// test's keys.
#[cfg(not(test))]
static HOTKEYS: std::sync::RwLock<HotkeyBindings> = std::sync::RwLock::new(HotkeyBindings::new());

#[cfg(not(test))]
fn with_hotkeys<R>(f: impl FnOnce(&mut HotkeyBindings) -> R) -> R {
    f(&mut HOTKEYS.write().unwrap_or_else(|e| e.into_inner()))
}

/// The user's changes to the stream hotkeys.
pub fn hotkey_bindings() -> HotkeyBindings {
    with_hotkeys(|bindings| bindings.clone())
}

/// Replaces the user's changes (stream_hotkeys.json's, at start).
pub fn set_hotkeys(bindings: HotkeyBindings) {
    with_hotkeys(|now| *now = bindings);
}

/// Gives hotkey `id` a recorded chord; nothing changes when it is refused, and the refusal says
/// why (another action's chord names that action).
pub fn record_hotkey(id: &str, keys: &[String]) -> Result<(), String> {
    let mut bindings = hotkey_bindings();
    let keys = policy::check_hotkey(Client::Desktop, id, keys, &bindings).map_err(|e| hotkey_error_text(&e))?;
    bindings.insert(id.to_string(), policy::binding_text(Some(&keys)));
    set_hotkeys(bindings);
    Ok(())
}

pub fn turn_off_hotkey(id: &str) {
    let mut bindings = hotkey_bindings();
    bindings.insert(id.to_string(), policy::binding_text(None));
    set_hotkeys(bindings);
}

pub fn reset_hotkey(id: &str) {
    let mut bindings = hotkey_bindings();
    bindings.remove(id);
    set_hotkeys(bindings);
}

/// The desktop's stream hotkeys with the user's changes, in list order.
pub fn hotkeys() -> Vec<Bound> {
    policy::bound(Client::Desktop, &hotkey_bindings())
}

/// Why a recorded chord was refused, in the user's language.
pub fn hotkey_error_text(error: &HotkeyError) -> String {
    match error {
        HotkeyError::Taken(id) => {
            let action = Action::from_hotkey(id).map_or(*id, |action| tr(action.label()));
            trf("Already used by {}", &[&action])
        }
        HotkeyError::Length => tr("Use 2 to 4 keys.").into(),
        HotkeyError::Modifier => tr("Use Ctrl, Alt or Shift, without the Windows key.").into(),
        HotkeyError::OneKey => tr("Use one key besides Ctrl, Alt and Shift.").into(),
        HotkeyError::Reserved => tr("Windows keeps this combination.").into(),
        HotkeyError::UnsupportedKey(key) => trf("Unsupported key: {}", &[key]),
        HotkeyError::RepeatedKey(key) => trf("Repeated key: {}", &[key]),
        // The page lists only the desktop's own actions; these name a bug, in English.
        HotkeyError::UnknownAction(_) | HotkeyError::NotHere => format!("{error:?}"),
    }
}

/// The stream hotkeys as bindings: each one's chord, then its old ones; none while it is off.
fn hotkey_bindings_of(bound: &[Bound]) -> Vec<KeyBinding> {
    bound.iter().filter_map(|b| {
        let action = Action::from_hotkey(b.id)?;
        let chords: Vec<Chord> = b.keys.iter().chain(&b.old).map(|keys| Chord::of_keys(keys)).collect();
        (!chords.is_empty()).then_some(KeyBinding { action, scope: Scope::Stream, chords: Cow::Owned(chords) })
    }).collect()
}

/// The bindings that work in `scope`, in list order: in a stream, the stream hotkeys after the
/// keys that work anywhere.
pub fn keys_in(scope: Scope) -> Vec<KeyBinding> {
    let mut list: Vec<KeyBinding> = KEYS.iter().filter(|b| b.scope.covers(scope)).cloned().collect();
    if scope == Scope::Stream {
        list.extend(hotkey_bindings_of(&hotkeys()));
    }
    list
}

/// The stream hotkey a key press does: its key with exactly the chord's modifiers held.
fn hotkey_action(bound: &[Bound], scancode: SDL_Scancode, mods: SDL_Keymod) -> Option<Action> {
    let (vk, _) = crate::keymap::virtual_key(scancode, true)?;
    let has = |flag: SDL_Keymod| mods.0 & flag.0 != 0;
    let modifiers = [(SDL_KMOD_SHIFT, 0x01), (SDL_KMOD_CTRL, 0x02), (SDL_KMOD_ALT, 0x04), (SDL_KMOD_GUI, 0x08)]
        .into_iter().filter(|(flag, _)| has(*flag)).fold(0, |mask, (_, bit)| mask | bit);
    policy::hotkey_at(bound, modifiers, vk).and_then(|(id, _)| Action::from_hotkey(id))
}

/// What a key press does in `scope`, by its physical key and the modifiers held.
pub fn key_action(scope: Scope, scancode: SDL_Scancode, mods: SDL_Keymod) -> Option<Action> {
    if scope == Scope::Stream {
        if let Some(action) = hotkey_action(&hotkeys(), scancode, mods) {
            return Some(action);
        }
    }
    KEYS.iter().filter(|b| b.scope.covers(scope))
        .find(|b| b.chords.iter().any(|c| c.key.scancode() == Some(scancode) && c.held(mods))).map(|b| b.action)
}

/// The keys egui reported for a hotkey being recorded, in nightjar-policy's names: the modifiers
/// held, then the key. None for a key without a name there.
pub fn recorded_keys(key: egui::Key, modifiers: egui::Modifiers) -> Option<Vec<String>> {
    let name = egui_key_name(key)?;
    let mut keys = Vec::with_capacity(4);
    if modifiers.ctrl || modifiers.command { keys.push("CTRL".to_string()); }
    if modifiers.alt { keys.push("ALT".to_string()); }
    if modifiers.shift { keys.push("SHIFT".to_string()); }
    keys.push(name.to_string());
    Some(keys)
}

/// nightjar-policy's name for a key egui reports.
fn egui_key_name(key: egui::Key) -> Option<&'static str> {
    use egui::Key as E;
    const LETTERS: [(E, &str); 26] = [(E::A, "A"), (E::B, "B"), (E::C, "C"), (E::D, "D"), (E::E, "E"), (E::F, "F"), (E::G, "G"),
        (E::H, "H"), (E::I, "I"), (E::J, "J"), (E::K, "K"), (E::L, "L"), (E::M, "M"), (E::N, "N"), (E::O, "O"), (E::P, "P"),
        (E::Q, "Q"), (E::R, "R"), (E::S, "S"), (E::T, "T"), (E::U, "U"), (E::V, "V"), (E::W, "W"), (E::X, "X"), (E::Y, "Y"), (E::Z, "Z")];
    const DIGITS: [(E, &str); 10] = [(E::Num0, "0"), (E::Num1, "1"), (E::Num2, "2"), (E::Num3, "3"), (E::Num4, "4"),
        (E::Num5, "5"), (E::Num6, "6"), (E::Num7, "7"), (E::Num8, "8"), (E::Num9, "9")];
    const F_KEYS: [(E, &str); 12] = [(E::F1, "F1"), (E::F2, "F2"), (E::F3, "F3"), (E::F4, "F4"), (E::F5, "F5"), (E::F6, "F6"),
        (E::F7, "F7"), (E::F8, "F8"), (E::F9, "F9"), (E::F10, "F10"), (E::F11, "F11"), (E::F12, "F12")];
    if let Some((_, name)) = LETTERS.iter().chain(&DIGITS).chain(&F_KEYS).find(|(k, _)| *k == key) {
        return Some(name);
    }
    Some(match key {
        E::Escape => "ESC", E::Tab => "TAB", E::Enter => "ENTER", E::Space => "SPACE", E::Backspace => "BACKSPACE",
        E::Delete => "DELETE", E::Insert => "INSERT", E::Home => "HOME", E::End => "END", E::PageUp => "PAGE_UP",
        E::PageDown => "PAGE_DOWN", E::ArrowUp => "DPAD_UP", E::ArrowDown => "DPAD_DOWN", E::ArrowLeft => "DPAD_LEFT",
        E::ArrowRight => "DPAD_RIGHT", E::Minus => "MINUS", E::Equals | E::Plus => "EQUALS", E::Comma => "COMMA",
        E::Period => "PERIOD", E::Slash => "SLASH", E::Backslash => "BACKSLASH", E::Semicolon => "SEMICOLON",
        E::OpenBracket => "LEFT_BRACKET", E::CloseBracket => "RIGHT_BRACKET", E::Quote => "APOSTROPHE", E::Backtick => "GRAVE",
        _ => return None,
    })
}

/// The first chord for `action` in `scope`, for a hint beside a menu entry.
pub fn chord_for(scope: Scope, action: Action) -> Option<Chord> {
    keys_in(scope).into_iter().find(|b| b.action == action).and_then(|b| b.chords.first().copied())
}

/// Whether egui saw any of `action`'s chords for the launcher this frame. While a text field has
/// focus (`typing`), a chord that types a character (? alone) belongs to the field.
pub fn pressed(input: &egui::InputState, action: Action, typing: bool) -> bool {
    KEYS.iter()
        .filter(|b| b.scope.covers(Scope::Launcher) && b.action == action)
        .any(|b| b.chords.iter().any(|c| !(typing && c.types()) && c.pressed(input)))
}

/// A controller button by its place, as SDL names them; the names on it follow the controller
/// (`glyphs::Style`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    South,
    East,
    West,
    North,
    Start,
    Back,
    LeftShoulder,
    RightShoulder,
    DPad,
    LeftStick,
}

impl Button {
    pub fn from_sdl(button: SDL_GamepadButton) -> Option<Self> {
        Some(match button {
            SDL_GAMEPAD_BUTTON_SOUTH => Button::South,
            SDL_GAMEPAD_BUTTON_EAST => Button::East,
            SDL_GAMEPAD_BUTTON_WEST => Button::West,
            SDL_GAMEPAD_BUTTON_NORTH => Button::North,
            SDL_GAMEPAD_BUTTON_START => Button::Start,
            SDL_GAMEPAD_BUTTON_BACK => Button::Back,
            SDL_GAMEPAD_BUTTON_LEFT_SHOULDER => Button::LeftShoulder,
            SDL_GAMEPAD_BUTTON_RIGHT_SHOULDER => Button::RightShoulder,
            SDL_GAMEPAD_BUTTON_DPAD_UP | SDL_GAMEPAD_BUTTON_DPAD_DOWN | SDL_GAMEPAD_BUTTON_DPAD_LEFT | SDL_GAMEPAD_BUTTON_DPAD_RIGHT => Button::DPad,
            _ => return None,
        })
    }

    /// The button's bit in `1 << SDL_GamepadButton` masks; none for the D-pad and the stick.
    pub fn bit(self) -> u32 {
        let button = match self {
            Button::South => SDL_GAMEPAD_BUTTON_SOUTH,
            Button::East => SDL_GAMEPAD_BUTTON_EAST,
            Button::West => SDL_GAMEPAD_BUTTON_WEST,
            Button::North => SDL_GAMEPAD_BUTTON_NORTH,
            Button::Start => SDL_GAMEPAD_BUTTON_START,
            Button::Back => SDL_GAMEPAD_BUTTON_BACK,
            Button::LeftShoulder => SDL_GAMEPAD_BUTTON_LEFT_SHOULDER,
            Button::RightShoulder => SDL_GAMEPAD_BUTTON_RIGHT_SHOULDER,
            Button::DPad | Button::LeftStick => return 0,
        };
        1 << button.0
    }
}

/// How the buttons are pressed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Press {
    /// Any one of them.
    Any(&'static [Button]),
    /// All of them together.
    Together(&'static [Button]),
    /// The button held down for a moment.
    Hold(Button),
    /// The quit combo chosen in the settings (`gamepad::quit_mask`).
    QuitCombo,
}

#[derive(Clone, Copy, Debug)]
pub struct PadBinding {
    pub action: Action,
    pub scope: Scope,
    pub press: Press,
}

const fn pad(action: Action, scope: Scope, press: Press) -> PadBinding {
    PadBinding { action, scope, press }
}

/// Every controller button and combo Nightjar answers to itself, in the order the lists show them.
/// A and B trade places with the Nintendo layout setting ([`pad_action`]).
pub const PAD: &[PadBinding] = &[
    pad(Action::Move, Scope::Launcher, Press::Any(&[Button::DPad, Button::LeftStick])),
    pad(Action::Select, Scope::Launcher, Press::Any(&[Button::South])),
    pad(Action::Back, Scope::Launcher, Press::Any(&[Button::East, Button::Back])),
    pad(Action::Options, Scope::Launcher, Press::Any(&[Button::West])),
    pad(Action::Unstar, Scope::Launcher, Press::Any(&[Button::North])),
    pad(Action::OpenSettings, Scope::Launcher, Press::Any(&[Button::Start])),
    pad(Action::PreviousTab, Scope::Launcher, Press::Any(&[Button::LeftShoulder])),
    pad(Action::NextTab, Scope::Launcher, Press::Any(&[Button::RightShoulder])),
    pad(Action::StreamControls, Scope::Stream, Press::Together(&[Button::Start, Button::Back])),
    pad(Action::Disconnect, Scope::Stream, Press::QuitCombo),
    pad(Action::ControllerMouse, Scope::Stream, Press::Hold(Button::Start)),
];

pub fn pad_in(scope: Scope) -> impl Iterator<Item = &'static PadBinding> {
    PAD.iter().filter(move |b| b.scope.covers(scope))
}

/// What a button does in the launcher; `swap` is the Nintendo layout setting, with which the
/// south button goes back and the east one selects.
pub fn pad_action(button: Button, swap: bool) -> Option<Action> {
    let button = match (swap, button) {
        (true, Button::South) => Button::East,
        (true, Button::East) => Button::South,
        _ => button,
    };
    pad_in(Scope::Launcher).find(|b| matches!(b.press, Press::Any(buttons) if buttons.contains(&button))).map(|b| b.action)
}

/// The buttons that open the stream controls together, as a `1 << SDL_GamepadButton` mask.
pub fn stream_controls_mask() -> u32 {
    pad_in(Scope::Stream)
        .find_map(|b| match (b.action, b.press) {
            (Action::StreamControls, Press::Together(buttons)) => Some(buttons.iter().map(|b| b.bit()).fold(0, |a, b| a | b)),
            _ => None,
        })
        .unwrap_or(0)
}

/// The buttons of a quit combo, in the order the lists name them.
pub fn quit_buttons(combo: crate::config::GamepadQuitCombo) -> Vec<Button> {
    let mask = crate::gamepad::quit_mask(combo);
    [Button::Start, Button::Back, Button::LeftShoulder, Button::RightShoulder, Button::South, Button::East, Button::West, Button::North]
        .into_iter()
        .filter(|b| mask & b.bit() != 0)
        .collect()
}

// The test half of with_hotkeys follows this module: the strings check reads a file only up to
// its first test attribute.
#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;

    #[test]
    fn android_document_round_trips_and_refusals_are_worded_without_replacing_entries() {
        let text = r#"{"version":1,"data":[{"id":"desktop","name":"Task view","keys":["WIN","TAB"]}]}"#;
        let mut shortcuts = parse(text).unwrap();
        assert_eq!(parse(&shortcuts.encode()).unwrap(), shortcuts);
        let before = shortcuts.clone();
        assert_eq!(save(&mut shortcuts, Some("desktop"), "Bad", "CTRL + CTRL_LEFT"), Err(trf("Repeated key: {}", &[&"CTRL_LEFT"])));
        assert_eq!(shortcuts, before);
        assert_eq!(parse(&text.replace("\"version\":1", "\"version\":2")), Err(tr("Unsupported shortcut file version.").to_owned()));
        save(&mut shortcuts, None, "Portable aliases", "VK_LCONTROL + PRINTSCREEN + F01").unwrap();
        assert_eq!((shortcuts.data[1].id.as_str(), &shortcuts.data[1].keys[..]), ("desktop-1", &["CTRL".to_owned(), "SYSRQ".into(), "F1".into()][..]));
    }

    /// A chord is pressed, then let go of in reverse only by `release` (#63): pressed and released in
    /// the same instant, the Steam overlay's Shift + Tab never saw it held.
    #[test]
    fn chords_keep_held_keys_and_reject_a_full_queue_before_pressing_anything() {
        use nightjar_core::input::InputEvent;
        let caps = nightjar_core::control::HostCapabilities { app_version: [7, 1, 0, -1], ..Default::default() };
        let mut queue = InputQueue::new(&caps);
        let mut packets = Vec::new();
        let encoded = |events: &[(u16, bool, u8)]| events.iter().map(|&(key, down, modifiers)| {
            InputEvent::Keyboard { key_code: 0x8000 | key, down, modifiers, flags: 0 }.encode(true)
        }).collect::<Vec<_>>();
        queue.drain(std::time::Instant::now(), false, &mut packets);
        packets.clear();
        let held = HashSet::from([0xA2]);
        let mut chord = press(&["C".into(), "CTRL".into()], &held, &mut queue).unwrap();
        queue.drain(std::time::Instant::now(), false, &mut packets);
        assert_eq!(packets.iter().map(|p| p.data.clone()).collect::<Vec<_>>(), encoded(&[(0x43, true, modifier::CTRL)]));
        packets.clear();
        release(&mut chord, &mut queue).unwrap();
        queue.drain(std::time::Instant::now(), false, &mut packets);
        assert_eq!(packets.iter().map(|p| p.data.clone()).collect::<Vec<_>>(), encoded(&[(0x43, false, modifier::CTRL)]));
        packets.clear();
        let mut chord = press(&["TAB".into(), "ALT".into(), "CTRL".into()], &HashSet::new(), &mut queue).unwrap();
        queue.drain(std::time::Instant::now(), false, &mut packets);
        assert_eq!(packets.iter().map(|p| p.data.clone()).collect::<Vec<_>>(), encoded(&[(0xA4, true, 4), (0xA2, true, 6), (0x09, true, 6)]));
        packets.clear();
        release(&mut chord, &mut queue).unwrap();
        release(&mut chord, &mut queue).unwrap();
        queue.drain(std::time::Instant::now(), false, &mut packets);
        assert_eq!(packets.iter().map(|p| p.data.clone()).collect::<Vec<_>>(), encoded(&[(0x09, false, 6), (0xA2, false, 4), (0xA4, false, 0)]),
            "released in reverse, once");
        while queue.len() < nightjar_core::input::queue::MAX_QUEUED - 1 {
            queue.send_keyboard(0x8041, true, 0, 0).unwrap();
        }
        let before = queue.len();
        assert_eq!(press(&["ALT".into(), "TAB".into()], &HashSet::new(), &mut queue), Err(InputError::QueueFull));
        assert_eq!(queue.len(), before);
    }

    /// The Game Bar tiles send Windows' own chords: Win down, Alt down for Record that, G down, then
    /// up in reverse.
    #[test]
    fn the_game_bar_tiles_press_win_g_and_win_alt_g() {
        use nightjar_core::input::InputEvent;
        let caps = nightjar_core::control::HostCapabilities { app_version: [7, 1, 0, -1], ..Default::default() };
        let mut queue = InputQueue::new(&caps);
        let mut packets = Vec::new();
        queue.drain(std::time::Instant::now(), false, &mut packets);
        let built_in = |id: &str| nightjar_policy::shortcuts::BUILT_INS.iter().find(|b| b.id == id).unwrap().keys;
        let (win, alt) = (modifier::META, modifier::ALT);
        for (id, events) in [
            ("game_bar", vec![(0x5B, true, win), (0x47, true, win), (0x47, false, win), (0x5B, false, 0)]),
            ("record_that", vec![(0x5B, true, win), (0xA4, true, win | alt), (0x47, true, win | alt),
                (0x47, false, win | alt), (0xA4, false, win), (0x5B, false, 0)]),
        ] {
            packets.clear();
            let keys: Vec<String> = built_in(id).iter().map(|k| k.to_string()).collect();
            let mut chord = press(&keys, &HashSet::new(), &mut queue).unwrap();
            release(&mut chord, &mut queue).unwrap();
            queue.drain(std::time::Instant::now(), false, &mut packets);
            let expected: Vec<_> = events.iter().map(|&(key, down, modifiers)| {
                InputEvent::Keyboard { key_code: 0x8000 | key, down, modifiers, flags: 0 }.encode(true)
            }).collect();
            assert_eq!(packets.iter().map(|p| p.data.clone()).collect::<Vec<_>>(), expected, "{id}");
        }
    }

    /// The source of a handler, without its comments, for the checks below.
    fn code(source: &str) -> String {
        source.lines().map(|line| line.split("//").next().unwrap_or("")).collect::<Vec<_>>().join("
")
    }

    /// Every key in the list has a handler that matches on its action, so the help never lists a
    /// key that does nothing: the stream's chords in stream/input.rs, F12 in the stream and the
    /// launcher's F11 and F12 in main.rs, F1 and ? on the Help page. The launcher keys egui's pages
    /// handle themselves are pressed in `ui/help/tests.rs`.
    #[test]
    fn every_bound_key_has_a_handler() {
        let input = code(include_str!("stream/input.rs"));
        let main = code(include_str!("main.rs"));
        let help = code(include_str!("ui/help.rs"));
        for binding in KEYS.iter().cloned().chain(keys_in(Scope::Stream)) {
            let name = format!("Action::{:?}", binding.action);
            let handled = match binding.scope {
                Scope::Stream => input.contains(&name) || main.contains(&name),
                Scope::Anywhere => main.contains("Scope::Stream") && main.matches(&name).count() >= 2,
                Scope::Launcher => match binding.action {
                    Action::Fullscreen => main.contains(&name),
                    Action::Help => help.contains(&name),
                    // Pressed in ui/help/tests.rs.
                    Action::Move | Action::Select | Action::Back | Action::Options | Action::AddPc | Action::Unstar
                    | Action::ZoomIn | Action::ZoomOut | Action::ZoomReset => true,
                    _ => false,
                },
            };
            assert!(handled, "{:?} in {:?} has no handler", binding.action, binding.scope);
        }
        for binding in PAD {
            let name = format!("Action::{:?}", binding.action);
            let handled = match (binding.scope, binding.press) {
                (Scope::Launcher, Press::Any(_)) => binding.action == Action::Move || main.contains(&name),
                (Scope::Stream, Press::Together(_)) => include_str!("gamepad.rs").contains("stream_controls_mask()"),
                (Scope::Stream, Press::QuitCombo) => include_str!("gamepad.rs").contains("quit_mask(self.quit_combo)"),
                (Scope::Stream, Press::Hold(Button::Start)) => include_str!("controller_mouse.rs").contains("750"),
                _ => false,
            };
            assert!(handled, "{:?} {:?} has no handler", binding.action, binding.press);
        }
    }

    /// Each chord comes back as its action from SDL's key and modifiers, and only with all of its
    /// modifiers: a host's own Ctrl+Alt chords still go to the PC.
    #[test]
    fn chords_resolve_to_their_action_and_only_with_their_modifiers() {
        let ctrl_alt_shift = SDL_Keymod(SDL_KMOD_LCTRL.0 | SDL_KMOD_LALT.0 | SDL_KMOD_LSHIFT.0);
        let ctrl_alt = SDL_Keymod(SDL_KMOD_LCTRL.0 | SDL_KMOD_LALT.0);
        for binding in keys_in(Scope::Stream).into_iter().filter(|b| b.scope == Scope::Stream) {
            for chord in binding.chords.iter() {
                let code = chord.key.scancode().expect("a stream chord is a physical key");
                assert_eq!(key_action(Scope::Stream, code, ctrl_alt_shift), Some(binding.action), "{chord:?}");
                assert_eq!(key_action(Scope::Stream, code, ctrl_alt), None, "{chord:?} without Shift reaches the PC");
                assert_eq!(key_action(Scope::Launcher, code, ctrl_alt_shift).filter(|a| *a == binding.action), None, "{chord:?} is the stream's");
            }
        }
        assert_eq!(key_action(Scope::Stream, SDL_SCANCODE_F12, SDL_KMOD_NONE), Some(Action::Screenshot));
        assert_eq!(key_action(Scope::Launcher, SDL_SCANCODE_F12, SDL_KMOD_NONE), Some(Action::Screenshot));
        assert_eq!(key_action(Scope::Launcher, SDL_SCANCODE_F11, SDL_KMOD_NONE), Some(Action::Fullscreen));
        assert_eq!(key_action(Scope::Stream, SDL_SCANCODE_F11, SDL_KMOD_NONE), None, "F11 goes to the game");
        assert_eq!(key_action(Scope::Stream, SDL_SCANCODE_F1, SDL_KMOD_NONE), None, "F1 goes to the game");
        // No two bindings in one place share a chord.
        for scope in [Scope::Launcher, Scope::Stream] {
            let chords: Vec<Chord> = keys_in(scope).into_iter().flat_map(|b| b.chords.into_owned()).collect();
            for (n, a) in chords.iter().enumerate() {
                assert!(!chords[n + 1..].contains(a), "{a:?} is bound twice in {scope:?}");
            }
        }
    }

    #[test]
    fn controller_buttons_follow_the_list_and_the_nintendo_layout() {
        assert_eq!(pad_action(Button::South, false), Some(Action::Select));
        assert_eq!(pad_action(Button::East, false), Some(Action::Back));
        assert_eq!(pad_action(Button::South, true), Some(Action::Back));
        assert_eq!(pad_action(Button::East, true), Some(Action::Select));
        assert_eq!(pad_action(Button::Back, true), Some(Action::Back));
        assert_eq!(pad_action(Button::West, false), Some(Action::Options));
        assert_eq!(pad_action(Button::North, false), Some(Action::Unstar));
        assert_eq!(pad_action(Button::Start, false), Some(Action::OpenSettings));
        assert_eq!(pad_action(Button::LeftShoulder, false), Some(Action::PreviousTab));
        assert_eq!(pad_action(Button::RightShoulder, true), Some(Action::NextTab));
        assert_eq!(stream_controls_mask(), 1 << SDL_GAMEPAD_BUTTON_START.0 | 1 << SDL_GAMEPAD_BUTTON_BACK.0);
        use crate::config::GamepadQuitCombo;
        assert_eq!(quit_buttons(GamepadQuitCombo::StartSelectLbRb), [Button::Start, Button::Back, Button::LeftShoulder, Button::RightShoulder]);
        for combo in GamepadQuitCombo::ALL {
            let mask = quit_buttons(combo).iter().fold(0, |m, b| m | b.bit());
            assert_eq!(mask, crate::gamepad::quit_mask(combo), "{combo:?}");
        }
    }

    #[test]
    fn chords_read_in_the_language_in_use_and_typing_keys_stay_in_text_fields() {
        assert_eq!(Chord::of_keys(&["CTRL".into(), "ALT".into(), "SHIFT".into(), "Q".into()]).text(), "Ctrl+Alt+Shift+Q");
        crate::strings::set(crate::strings::Lang::De);
        assert_eq!(Chord::of_keys(&["CTRL".into(), "ALT".into(), "SHIFT".into(), "Q".into()]).text(), "Strg+Alt+Umschalt+Q");
        assert_eq!(Chord::key(Key::Delete).text(), "Entf");
        crate::strings::set(crate::strings::Lang::En);
        assert!(Chord::key(Key::Question).types());
        assert!(!Chord::key(Key::F1).types());
        assert!(!Chord::ctrl(Key::N).types());
        assert_eq!(Chord::of_keys(&["CTRL".into(), "ALT".into(), "SHIFT".into(), "TAB".into()]).text(), "Ctrl+Alt+Shift+Tab");
        assert_eq!(Chord::of_keys(&["CTRL".into(), "PAGE_UP".into()]).text(), "Ctrl+PgUp");
    }

    /// The desktop has a handler for each of its hotkeys in nightjar-policy's table, and the
    /// defaults are the Ctrl+Alt+Shift chords Moonlight's desktop client uses.
    #[test]
    fn every_desktop_hotkey_is_an_action_with_its_default_chord() {
        for hotkey in policy::hotkeys(Client::Desktop) {
            let action = Action::from_hotkey(hotkey.id).unwrap_or_else(|| panic!("{} has no action", hotkey.id));
            assert_eq!(HOTKEY_ACTIONS.iter().filter(|(_, a)| *a == action).count(), 1, "{action:?}");
        }
        assert_eq!(policy::hotkeys(Client::Desktop).count(), HOTKEY_ACTIONS.len());
        let ctrl_alt_shift = SDL_Keymod(SDL_KMOD_LCTRL.0 | SDL_KMOD_LALT.0 | SDL_KMOD_LSHIFT.0);
        for (scancode, action) in [(SDL_SCANCODE_TAB, Action::StreamControls), (SDL_SCANCODE_H, Action::StreamShortcuts),
            (SDL_SCANCODE_Q, Action::Disconnect), (SDL_SCANCODE_Z, Action::Capture), (SDL_SCANCODE_M, Action::MouseMode),
            (SDL_SCANCODE_S, Action::Overlay), (SDL_SCANCODE_V, Action::SendClipboard), (SDL_SCANCODE_C, Action::FetchClipboard),
            (SDL_SCANCODE_X, Action::Fullscreen), (SDL_SCANCODE_D, Action::Minimize)] {
            assert_eq!(key_action(Scope::Stream, scancode, ctrl_alt_shift), Some(action), "{action:?}");
        }
        // Windows held as well is another chord, which goes to the PC.
        assert_eq!(key_action(Scope::Stream, SDL_SCANCODE_Q, SDL_Keymod(ctrl_alt_shift.0 | SDL_KMOD_LGUI.0)), None);
    }

    /// A chord changed during a stream does the action from the next key press; the old one goes to
    /// the PC. Off takes the action out of the keys and the lists; Reset brings the default back.
    #[test]
    fn a_rebind_applies_at_once_and_off_and_reset_hold() {
        let ctrl_alt_shift = SDL_Keymod(SDL_KMOD_LCTRL.0 | SDL_KMOD_LALT.0 | SDL_KMOD_LSHIFT.0);
        record_hotkey("quit", &["CTRL".into(), "F9".into()]).unwrap();
        assert_eq!(key_action(Scope::Stream, SDL_SCANCODE_F9, SDL_KMOD_LCTRL), Some(Action::Disconnect));
        assert_eq!(key_action(Scope::Stream, SDL_SCANCODE_F9, SDL_KMOD_NONE), None, "F9 alone goes to the game");
        assert_eq!(key_action(Scope::Stream, SDL_SCANCODE_Q, ctrl_alt_shift), None);
        assert_eq!(chord_for(Scope::Stream, Action::Disconnect).map(Chord::text).as_deref(), Some("Ctrl+F9"));
        assert_eq!(record_hotkey("capture", &["F9".into(), "CTRL".into()]), Err(trf("Already used by {}", &[&tr(Action::Disconnect.label())])));
        assert_eq!(record_hotkey("capture", &["F9".into()]), Err(tr("Use 2 to 4 keys.").to_owned()));
        turn_off_hotkey("overlay");
        assert_eq!(key_action(Scope::Stream, SDL_SCANCODE_S, ctrl_alt_shift), None);
        assert!(!keys_in(Scope::Stream).iter().any(|b| b.action == Action::Overlay), "an action turned off is not listed");
        assert_eq!(hotkey_bindings().get("overlay").map(String::as_str), Some(policy::OFF));
        reset_hotkey("quit");
        reset_hotkey("overlay");
        assert!(hotkey_bindings().is_empty());
        assert_eq!(key_action(Scope::Stream, SDL_SCANCODE_Q, ctrl_alt_shift), Some(Action::Disconnect));
        assert_eq!(key_action(Scope::Stream, SDL_SCANCODE_S, ctrl_alt_shift), Some(Action::Overlay));
    }

    #[test]
    fn a_recorded_key_has_its_policy_name_with_the_modifiers_held() {
        let mods = egui::Modifiers { ctrl: true, alt: true, shift: true, ..Default::default() };
        assert_eq!(recorded_keys(egui::Key::Tab, mods), Some(vec!["CTRL".into(), "ALT".into(), "SHIFT".into(), "TAB".into()]));
        assert_eq!(recorded_keys(egui::Key::F5, egui::Modifiers::CTRL), Some(vec!["CTRL".into(), "F5".into()]));
        for key in egui::Key::ALL {
            if let Some(keys) = recorded_keys(*key, egui::Modifiers::NONE) {
                assert!(key_code(&keys[0]).is_some(), "{key:?} records {keys:?}");
            }
        }
    }
}

/// Each test thread's own stream hotkeys.
#[cfg(test)]
fn with_hotkeys<R>(f: impl FnOnce(&mut HotkeyBindings) -> R) -> R {
    thread_local! {
        static HOTKEYS: std::cell::RefCell<HotkeyBindings> = const { std::cell::RefCell::new(HotkeyBindings::new()) };
    }
    HOTKEYS.with(|hotkeys| f(&mut hotkeys.borrow_mut()))
}
