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
// (app/launcher.rs: the launcher's F11 and F12 and its controller navigation; stream/input.rs: the
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
            Self::StreamControls => en("Stream menu"),
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
    F,
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
            Key::F => "F",
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
            Key::Vk(code) => return Cow::Owned(vk_cap(code)),
        })
    }

    /// The physical key SDL reports (stream chords are positional, as the PC's keys are).
    pub fn scancode(self) -> Option<SDL_Scancode> {
        Some(match self {
            Key::Tab => SDL_SCANCODE_TAB,
            Key::N => SDL_SCANCODE_N,
            Key::F => SDL_SCANCODE_F,
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
            Key::F => &[E::F],
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

/// What a stream hotkey's key is called. It goes by its place on the keyboard, so a key that prints
/// is named as the layout in use has it there: QWERTY's Z is QWERTZ's Y and AZERTY's W. A layout
/// that agrees with QWERTY there keeps the usual name.
fn vk_cap(code: u16) -> String {
    let named = policy::key_label(&policy::key_name(code));
    let (Some(usual), Some(scancode)) = (qwerty_character(code), Key::Vk(code).scancode()) else { return named };
    match keycode_character(layout_keycode(scancode)) {
        Some(there) if capital(there) != capital(usual) => capital(there),
        _ => named,
    }
}

/// What the key with this virtual-key code prints on QWERTY; only keys that print have one.
fn qwerty_character(code: u16) -> Option<char> {
    Some(match code {
        0x30..=0x39 => char::from(code as u8),
        0x41..=0x5A => char::from(code as u8).to_ascii_lowercase(),
        0xBA => ';',
        0xBB => '=',
        0xBC => ',',
        0xBD => '-',
        0xBE => '.',
        0xBF => '/',
        0xC0 => '`',
        0xDB => '[',
        0xDC => '\\',
        0xDD => ']',
        0xDE => '\'',
        _ => return None,
    })
}

/// The character an SDL keycode stands for; none for a key that doesn't print, or a dead key's
/// accent that would have nothing to sit on.
fn keycode_character(keycode: SDL_Keycode) -> Option<char> {
    if keycode.0 & (SDLK_SCANCODE_MASK.0 | SDLK_EXTENDED_MASK.0) != 0 {
        return None;
    }
    char::from_u32(keycode.0).filter(|c| !c.is_control() && !c.is_whitespace() && !matches!(*c, '\u{0300}'..='\u{036F}'))
}

/// As a key's top shows it. ß stays ß: its capital is two letters.
fn capital(character: char) -> String {
    let mut upper = character.to_uppercase();
    match (upper.next(), upper.next()) {
        (Some(one), None) => one.to_string(),
        _ => character.to_string(),
    }
}

/// The key this place has in the layout in use, without modifiers. As for key events, so a Latin
/// letter stays on a Cyrillic layout and AZERTY's number row stays digits, both as printed on the
/// keys; SDL answers QWERTY until its keyboard is up.
#[cfg(not(test))]
fn layout_keycode(scancode: SDL_Scancode) -> SDL_Keycode {
    // SAFETY: only reads SDL's current keymap, and takes any scancode.
    unsafe { SDL_GetKeyFromScancode(scancode, SDL_KMOD_NONE, true) }
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
        !self.ctrl && !self.alt && matches!(self.key, Key::N | Key::F
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
    keys(Action::Options, Scope::Launcher, &[Chord::key(Key::F), Chord::key(Key::F2), Chord::shift(Key::F10)]),
    keys(Action::AddPc, Scope::Launcher, &[Chord::ctrl(Key::N)]),
    keys(Action::Unstar, Scope::Launcher, &[Chord::key(Key::Delete)]),
    keys(Action::ZoomIn, Scope::Launcher, &[Chord::ctrl(Key::Plus)]),
    keys(Action::ZoomOut, Scope::Launcher, &[Chord::ctrl(Key::Minus)]),
    keys(Action::ZoomReset, Scope::Launcher, &[Chord::ctrl(Key::Zero)]),
    keys(Action::Fullscreen, Scope::Launcher, &[Chord::key(Key::F11)]),
    keys(Action::Screenshot, Scope::Anywhere, &[Chord::key(Key::F12)]),
];

// The user's stream hotkeys, as stream_hotkeys.json saves them, and the lookup the key handling
// reads on every key press (the system keys hook's thread too), built once per change: a change
// applies at once, in a running stream too. Each test thread keeps its own (`read_hotkeys` after
// the tests), so a test that changes them changes no other test's keys.
struct HotkeyState {
    bindings: HotkeyBindings,
    bound: Vec<Bound>,
    /// Each chord's modifiers (Shift 1, Ctrl 2, Alt 4) and virtual-key code, and its action: the
    /// bound chords first, then the old ones.
    table: Vec<((u8, u16), Action)>,
}

impl HotkeyState {
    fn new(bindings: HotkeyBindings) -> Self {
        let bound = policy::bound(Client::Desktop, &bindings);
        let mut table = Vec::new();
        for b in &bound {
            if let (Some(keys), Some(action)) = (&b.keys, Action::from_hotkey(b.id)) {
                table.push((policy::hotkey_code(keys), action));
            }
        }
        for b in &bound {
            for keys in &b.old {
                let code = policy::hotkey_code(keys);
                if let Some(action) = Action::from_hotkey(b.id).filter(|_| !table.iter().any(|(c, _)| *c == code)) {
                    table.push((code, action));
                }
            }
        }
        Self { bindings, bound, table }
    }
}

#[cfg(not(test))]
static HOTKEYS: std::sync::LazyLock<std::sync::RwLock<HotkeyState>> =
    std::sync::LazyLock::new(|| std::sync::RwLock::new(HotkeyState::new(HotkeyBindings::new())));

#[cfg(not(test))]
fn read_hotkeys<R>(f: impl FnOnce(&HotkeyState) -> R) -> R {
    f(&HOTKEYS.read().unwrap_or_else(|e| e.into_inner()))
}

#[cfg(not(test))]
fn store_hotkeys(state: HotkeyState) {
    *HOTKEYS.write().unwrap_or_else(|e| e.into_inner()) = state;
}

/// The user's changes to the stream hotkeys.
pub fn hotkey_bindings() -> HotkeyBindings {
    read_hotkeys(|state| state.bindings.clone())
}

/// Replaces the user's changes (stream_hotkeys.json's, at start).
pub fn set_hotkeys(bindings: HotkeyBindings) {
    store_hotkeys(HotkeyState::new(bindings));
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

/// Turns hotkey `id` off; refused for the last keyboard way out of a captured stream.
pub fn turn_off_hotkey(id: &str) -> Result<(), String> {
    let mut bindings = hotkey_bindings();
    policy::check_off(Client::Desktop, id, &bindings).map_err(|e| hotkey_error_text(&e))?;
    bindings.insert(id.to_string(), policy::binding_text(None));
    set_hotkeys(bindings);
    Ok(())
}

/// Gives hotkey `id` its default back; refused, naming the action, while another one has it.
pub fn reset_hotkey(id: &str) -> Result<(), String> {
    let mut bindings = hotkey_bindings();
    policy::check_reset(Client::Desktop, id, &bindings).map_err(|e| hotkey_error_text(&e))?;
    bindings.remove(id);
    set_hotkeys(bindings);
    Ok(())
}

/// The desktop's stream hotkeys with the user's changes, in list order.
pub fn hotkeys() -> Vec<Bound> {
    read_hotkeys(|state| state.bound.clone())
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
        HotkeyError::Typing => tr("This combination is for typing.").into(),
        HotkeyError::LastWayOut => tr("Keep a hotkey for Stream menu or Disconnect.").into(),
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

/// The modifiers SDL reports held, in the input packets' bits (Shift 1, Ctrl 2, Alt 4, Windows 8).
fn modifier_bits(mods: SDL_Keymod) -> u8 {
    let has = |flag: SDL_Keymod| mods.0 & flag.0 != 0;
    [(SDL_KMOD_SHIFT, 0x01), (SDL_KMOD_CTRL, 0x02), (SDL_KMOD_ALT, 0x04), (SDL_KMOD_GUI, 0x08)]
        .into_iter().filter(|(flag, _)| has(*flag)).fold(0, |mask, (_, bit)| mask | bit)
}

/// The stream hotkey a key press does: its key with exactly the chord's modifiers held. Reads the
/// ready table under a read lock: no allocation or parsing per key.
fn hotkey_action(scancode: SDL_Scancode, mods: SDL_Keymod) -> Option<Action> {
    let (vk, _) = crate::keymap::virtual_key(scancode, true)?;
    let code = (modifier_bits(mods), vk);
    read_hotkeys(|state| state.table.iter().find(|(c, _)| *c == code).map(|(_, action)| *action))
}

/// What a key press does in `scope`, by its physical key and the modifiers held.
pub fn key_action(scope: Scope, scancode: SDL_Scancode, mods: SDL_Keymod) -> Option<Action> {
    if scope == Scope::Stream {
        if let Some(action) = hotkey_action(scancode, mods) {
            return Some(action);
        }
    }
    KEYS.iter().filter(|b| b.scope.covers(scope))
        .find(|b| b.chords.iter().any(|c| c.key.scancode() == Some(scancode) && c.held(mods))).map(|b| b.action)
}

/// A key pressed while a hotkey is recorded, in nightjar-policy's names: the modifiers held, then
/// the key by its place on the keyboard (SDL's scancode as the PC's virtual key, as the defaults and
/// the matching go; on QWERTZ the key labelled Z records as Y). None for a modifier alone, which
/// waits for the rest of the chord, or a key without a name there.
pub fn recorded_keys(scancode: SDL_Scancode, mods: SDL_Keymod) -> Option<Vec<String>> {
    let (vk, _) = crate::keymap::virtual_key(scancode, true)?;
    if policy::is_modifier(&policy::try_key_name(vk)?) {
        return None;
    }
    let held = modifier_bits(mods);
    let mut keys: Vec<String> = [(0x02, "CTRL"), (0x04, "ALT"), (0x01, "SHIFT"), (0x08, "WIN")]
        .into_iter().filter(|(bit, _)| held & bit != 0).map(|(_, name)| name.to_string()).collect();
    keys.push(policy::try_key_name(vk)?);
    Some(keys)
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
    pad(Action::Options, Scope::Launcher, Press::Any(&[Button::North, Button::West])),
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

// The test half of read_hotkeys and store_hotkeys follows this module: the strings check reads a file only up to
// its first test attribute.
#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests;

// Each test thread's own stream hotkeys.
#[cfg(test)]
thread_local! {
    static HOTKEYS: std::cell::RefCell<HotkeyState> = std::cell::RefCell::new(HotkeyState::new(HotkeyBindings::new()));
}

// Each test thread's keyboard layout: QWERTY but for the keys listed, whatever this PC has.
#[cfg(test)]
thread_local! {
    static LAYOUT: std::cell::Cell<&'static [(SDL_Scancode, SDL_Keycode)]> = const { std::cell::Cell::new(&[]) };
}

/// Sets this test thread's layout: the keys that differ from QWERTY.
#[cfg(test)]
pub fn use_layout(keys: &'static [(SDL_Scancode, SDL_Keycode)]) {
    LAYOUT.with(|layout| layout.set(keys));
}

#[cfg(test)]
fn layout_keycode(scancode: SDL_Scancode) -> SDL_Keycode {
    let qwerty = || crate::keymap::virtual_key(scancode, true).and_then(|(vk, _)| qwerty_character(vk)).map_or(SDLK_UNKNOWN, |c| SDL_Keycode(c as u32));
    LAYOUT.with(|layout| layout.get().iter().find(|(place, _)| *place == scancode).map_or_else(qwerty, |(_, keycode)| *keycode))
}

#[cfg(test)]
fn read_hotkeys<R>(f: impl FnOnce(&HotkeyState) -> R) -> R {
    HOTKEYS.with(|hotkeys| f(&hotkeys.borrow()))
}

#[cfg(test)]
fn store_hotkeys(state: HotkeyState) {
    HOTKEYS.with(|hotkeys| *hotkeys.borrow_mut() = state);
}
