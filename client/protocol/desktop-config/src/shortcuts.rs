//! Keyboard shortcuts: Nightjar's own keys and controller buttons ([`KEYS`], [`PAD`]), the one
//! list the key handling and the help lists share, and the user's saved key chords that the stream
//! controls send to the PC (`Shortcuts`, in the Android app's file format).

use std::borrow::Cow;
use std::io::Read;
use std::path::Path;
use std::sync::OnceLock;

use crate::config::GamepadQuitCombo;
use crate::gamepad_types::{quit_mask, sdl_button_index};
use crate::strings::{en, tr, trf};

pub use nightjar_policy::shortcuts::{key_code, Shortcut, ShortcutError, Shortcuts, HOLD_MS, MAX_BYTES};
use nightjar_policy::shortcuts::{self as policy, Bound, Client, HotkeyBindings, HotkeyError};

type LayoutCharHook = fn(u16) -> Option<char>;

/// Optional layout resolver registered by the media/SDL layer (takes a Windows virtual-key code
/// and returns the character printed by that physical key in the active keyboard layout).
static LAYOUT_CHAR_HOOK: OnceLock<LayoutCharHook> = OnceLock::new();

thread_local! {
    /// Per-test-thread layout character resolver override (`use_layout` in runtime/help tests).
    static TEST_LAYOUT_CHAR: std::cell::Cell<Option<LayoutCharHook>> = const { std::cell::Cell::new(None) };
    /// Optional per-test-thread hotkey store isolation (activated by
    /// `isolate_thread_hotkeys_for_test` so parallel tests that rebind hotkeys never clash).
    static THREAD_HOTKEYS: std::cell::RefCell<Option<HotkeyState>> = const { std::cell::RefCell::new(None) };
}

/// Registers the SDL keyboard-layout character resolver used when formatting `Key::Vk` labels.
pub fn register_layout_char_hook(hook: LayoutCharHook) {
    let _ = LAYOUT_CHAR_HOOK.set(hook);
}

/// Overrides the keyboard-layout character resolver on the current thread (used by tests).
pub fn set_test_layout_char_hook(hook: Option<LayoutCharHook>) {
    TEST_LAYOUT_CHAR.with(|cell| cell.set(hook));
}

fn layout_char(code: u16) -> Option<char> {
    if let Some(hook) = TEST_LAYOUT_CHAR.with(|cell| cell.get()) {
        return hook(code);
    }
    LAYOUT_CHAR_HOOK.get().and_then(|hook| hook(code))
}

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
            Self::StreamControls => en("Control Center"),
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
pub const HOTKEY_ACTIONS: [(&str, Action); 10] = [
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
    pub fn covers(self, scope: Scope) -> bool {
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
}

/// What a stream hotkey's key is called. It goes by its place on the keyboard, so a key that prints
/// is named as the layout in use has it there: QWERTY's Z is QWERTZ's Y and AZERTY's W. A layout
/// that agrees with QWERTY there keeps the usual name.
fn vk_cap(code: u16) -> String {
    let named = policy::key_label(&policy::key_name(code));
    let Some(usual) = qwerty_character(code) else { return named };
    match layout_char(code) {
        Some(there) if capital(there) != capital(usual) => capital(there),
        _ => named,
    }
}

/// What the key with this virtual-key code prints on QWERTY; only keys that print have one.
pub fn qwerty_character(code: u16) -> Option<char> {
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

/// As a key's top shows it. ß stays ß: its capital is two letters.
fn capital(character: char) -> String {
    let mut upper = character.to_uppercase();
    match (upper.next(), upper.next()) {
        (Some(one), None) => one.to_string(),
        _ => character.to_string(),
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

/// The user's stream hotkeys and precomputed lookup table.
pub struct HotkeyState {
    pub bindings: HotkeyBindings,
    pub bound: Vec<Bound>,
    /// Each chord's modifiers (Shift 1, Ctrl 2, Alt 4) and virtual-key code, and its action: the
    /// bound chords first, then the old ones.
    pub table: Vec<((u8, u16), Action)>,
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

static HOTKEYS: std::sync::LazyLock<std::sync::RwLock<HotkeyState>> =
    std::sync::LazyLock::new(|| std::sync::RwLock::new(HotkeyState::new(HotkeyBindings::new())));

/// Enables isolated per-thread hotkey state on the current test thread.
pub fn isolate_thread_hotkeys_for_test() {
    THREAD_HOTKEYS.with(|cell| {
        *cell.borrow_mut() = Some(HotkeyState::new(HotkeyBindings::new()));
    });
}

/// Returns a snapshot of the active stream hotkey lookup table (`(modifiers, vk) -> Action`).
pub fn hotkey_table() -> Vec<((u8, u16), Action)> {
    read_hotkeys(|state| state.table.clone())
}

pub fn read_hotkeys<R>(f: impl FnOnce(&HotkeyState) -> R) -> R {
    THREAD_HOTKEYS.with(|cell| {
        if let Some(state) = cell.borrow().as_ref() {
            return f(state);
        }
        f(&HOTKEYS.read().unwrap_or_else(|e| e.into_inner()))
    })
}

fn store_hotkeys(state: HotkeyState) {
    let mut leftover = Some(state);
    THREAD_HOTKEYS.with(|cell| {
        let mut slot = cell.borrow_mut();
        if let Some(existing) = slot.as_mut() {
            *existing = leftover.take().unwrap();
        }
    });
    if let Some(state) = leftover {
        *HOTKEYS.write().unwrap_or_else(|e| e.into_inner()) = state;
    }
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
        HotkeyError::LastWayOut => tr("Keep a hotkey for Control Center or Disconnect.").into(),
        HotkeyError::UnsupportedKey(key) => trf("Unsupported key: {}", &[key]),
        HotkeyError::RepeatedKey(key) => trf("Repeated key: {}", &[key]),
        // The page lists only the desktop's own actions; these name a bug, in English.
        HotkeyError::UnknownAction(_) | HotkeyError::NotHere => format!("{error:?}"),
    }
}

/// The stream hotkeys as bindings: each one's chord, then its old ones; none while it is off.
fn hotkey_bindings_of(bound: &[Bound]) -> Vec<KeyBinding> {
    bound
        .iter()
        .filter_map(|b| {
            let action = Action::from_hotkey(b.id)?;
            let chords: Vec<Chord> = b.keys.iter().chain(&b.old).map(|keys| Chord::of_keys(keys)).collect();
            (!chords.is_empty()).then_some(KeyBinding { action, scope: Scope::Stream, chords: Cow::Owned(chords) })
        })
        .collect()
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

/// The first chord for `action` in `scope`, for a hint beside a menu entry.
pub fn chord_for(scope: Scope, action: Action) -> Option<Chord> {
    keys_in(scope).into_iter().find(|b| b.action == action).and_then(|b| b.chords.first().copied())
}

/// A controller button by its place, as SDL names them; the names on it follow the controller.
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
    /// The button's bit in `1 << SDL_GamepadButton` masks; none for the D-pad and the stick.
    pub fn bit(self) -> u32 {
        let index = match self {
            Button::South => sdl_button_index::SOUTH,
            Button::East => sdl_button_index::EAST,
            Button::West => sdl_button_index::WEST,
            Button::North => sdl_button_index::NORTH,
            Button::Start => sdl_button_index::START,
            Button::Back => sdl_button_index::BACK,
            Button::LeftShoulder => sdl_button_index::LEFT_SHOULDER,
            Button::RightShoulder => sdl_button_index::RIGHT_SHOULDER,
            Button::DPad | Button::LeftStick => return 0,
        };
        1 << index
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
    /// The quit combo chosen in the settings (`gamepad_types::quit_mask`).
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
pub fn quit_buttons(combo: GamepadQuitCombo) -> Vec<Button> {
    let mask = quit_mask(combo);
    [Button::Start, Button::Back, Button::LeftShoulder, Button::RightShoulder, Button::South, Button::East, Button::West, Button::North]
        .into_iter()
        .filter(|b| mask & b.bit() != 0)
        .collect()
}

#[cfg(test)]
mod tests;
