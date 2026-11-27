//! Unit tests for SDL3 keyboard and gamepad shortcut dispatch.

use super::*;
use crate::shortcuts::{
    chord_for, hotkey_bindings, hotkeys, isolate_thread_hotkeys_for_test, keys_in, record_hotkey,
    reset_hotkey, set_hotkeys, turn_off_hotkey, Action, Chord, Key, Scope, HOTKEY_ACTIONS,
};
use crate::strings::{tr, trf};
use nightjar_policy::shortcuts::{self as policy, Client, HotkeyBindings};

/// A chord is pressed, then let go of in reverse only by `release` (#63): pressed and released in
/// the same instant, the Steam overlay's Shift + Tab never saw it held.
#[test]
fn chords_keep_held_keys_and_reject_a_full_queue_before_pressing_anything() {
    use nightjar_core::input::InputEvent;
    let caps = nightjar_core::control::HostCapabilities { app_version: [7, 1, 0, -1], ..Default::default() };
    let mut queue = InputQueue::new(&caps);
    let mut packets = Vec::new();
    let encoded = |events: &[(u16, bool, u8)]| {
        events
            .iter()
            .map(|&(key, down, modifiers)| InputEvent::Keyboard { key_code: 0x8000 | key, down, modifiers, flags: 0 }.encode(true))
            .collect::<Vec<_>>()
    };
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
    assert_eq!(
        packets.iter().map(|p| p.data.clone()).collect::<Vec<_>>(),
        encoded(&[(0x09, false, 6), (0xA2, false, 4), (0xA4, false, 0)]),
        "released in reverse, once"
    );
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

/// Each chord comes back as its action from SDL's key and modifiers, and only with all of its
/// modifiers: a host's own Ctrl+Alt chords still go to the PC.
#[test]
fn chords_resolve_to_their_action_and_only_with_their_modifiers() {
    isolate_thread_hotkeys_for_test();
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

/// The desktop has a handler for each of its hotkeys in nightjar-policy's table, and the
/// defaults are the Ctrl+Alt+Shift chords Moonlight's desktop client uses.
#[test]
fn every_desktop_hotkey_is_an_action_with_its_default_chord() {
    isolate_thread_hotkeys_for_test();
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
    isolate_thread_hotkeys_for_test();
    let ctrl_alt_shift = SDL_Keymod(SDL_KMOD_LCTRL.0 | SDL_KMOD_LALT.0 | SDL_KMOD_LSHIFT.0);
    record_hotkey("quit", &["CTRL".into(), "F9".into()]).unwrap();
    assert_eq!(key_action(Scope::Stream, SDL_SCANCODE_F9, SDL_KMOD_LCTRL), Some(Action::Disconnect));
    assert_eq!(key_action(Scope::Stream, SDL_SCANCODE_F9, SDL_KMOD_NONE), None, "F9 alone goes to the game");
    assert_eq!(key_action(Scope::Stream, SDL_SCANCODE_Q, ctrl_alt_shift), None);
    assert_eq!(chord_for(Scope::Stream, Action::Disconnect).map(Chord::text).as_deref(), Some("Ctrl+F9"));
    assert_eq!(record_hotkey("capture", &["F9".into(), "CTRL".into()]), Err(trf("Already used by {}", &[&tr(Action::Disconnect.label())])));
    assert_eq!(record_hotkey("capture", &["F9".into(), "CTRL".into(), "ALT".into(), "SHIFT".into(), "Q".into()]), Err(tr("Use 2 to 4 keys.").to_owned()));
    turn_off_hotkey("overlay").unwrap();
    assert_eq!(key_action(Scope::Stream, SDL_SCANCODE_S, ctrl_alt_shift), None);
    assert!(!keys_in(Scope::Stream).iter().any(|b| b.action == Action::Overlay), "an action turned off is not listed");
    assert_eq!(hotkey_bindings().get("overlay").map(String::as_str), Some(policy::OFF));
    reset_hotkey("quit").unwrap();
    reset_hotkey("overlay").unwrap();
    assert!(hotkey_bindings().is_empty());
    assert_eq!(key_action(Scope::Stream, SDL_SCANCODE_Q, ctrl_alt_shift), Some(Action::Disconnect));
    assert_eq!(key_action(Scope::Stream, SDL_SCANCODE_S, ctrl_alt_shift), Some(Action::Overlay));
}

/// Reset brings back a default another action took since, Off then reuse alike: refused, naming
/// that action, and nothing changes.
#[test]
fn a_reset_to_a_chord_another_action_took_is_refused() {
    isolate_thread_hotkeys_for_test();
    let ctrl_alt_shift = SDL_Keymod(SDL_KMOD_LCTRL.0 | SDL_KMOD_LALT.0 | SDL_KMOD_LSHIFT.0);
    record_hotkey("quit", &["CTRL".into(), "F9".into()]).unwrap();
    record_hotkey("capture", &["CTRL".into(), "ALT".into(), "SHIFT".into(), "Q".into()]).unwrap();
    let before = hotkey_bindings();
    assert_eq!(reset_hotkey("quit"), Err(trf("Already used by {}", &[&tr(Action::Capture.label())])));
    assert_eq!(hotkey_bindings(), before);
    assert_eq!(key_action(Scope::Stream, SDL_SCANCODE_Q, ctrl_alt_shift), Some(Action::Capture));
    assert_eq!(key_action(Scope::Stream, SDL_SCANCODE_F9, SDL_KMOD_LCTRL), Some(Action::Disconnect));
    set_hotkeys(HotkeyBindings::new());
    turn_off_hotkey("stream_menu").unwrap();
    record_hotkey("quit", &["CTRL".into(), "ALT".into(), "SHIFT".into(), "TAB".into()]).unwrap();
    assert_eq!(reset_hotkey("stream_menu"), Err(trf("Already used by {}", &[&tr(Action::Disconnect.label())])));
    assert_eq!(key_action(Scope::Stream, SDL_SCANCODE_TAB, ctrl_alt_shift), Some(Action::Disconnect));
    set_hotkeys(HotkeyBindings::new());
}

/// A file with two actions on one chord: the later one has none, and the list says which has it.
#[test]
fn a_repeated_chord_leaves_the_later_action_without_one() {
    isolate_thread_hotkeys_for_test();
    let ctrl_alt_shift = SDL_Keymod(SDL_KMOD_LCTRL.0 | SDL_KMOD_LALT.0 | SDL_KMOD_LSHIFT.0);
    set_hotkeys(HotkeyBindings::from([("capture".to_string(), "CTRL+ALT+SHIFT+Q".to_string())]));
    assert_eq!(key_action(Scope::Stream, SDL_SCANCODE_Q, ctrl_alt_shift), Some(Action::Disconnect));
    assert_eq!(key_action(Scope::Stream, SDL_SCANCODE_Z, ctrl_alt_shift), None);
    assert_eq!(hotkeys().iter().find(|b| b.id == "capture").unwrap().taken, Some("quit"));
    assert!(!keys_in(Scope::Stream).iter().any(|b| b.action == Action::Capture));
    set_hotkeys(HotkeyBindings::new());
}

/// The desktop keeps a keyboard way out of a captured stream: the last of Stream controls and
/// Disconnect stays on.
#[test]
fn the_last_way_out_cannot_be_turned_off() {
    isolate_thread_hotkeys_for_test();
    turn_off_hotkey("quit").unwrap();
    assert_eq!(turn_off_hotkey("stream_menu"), Err(tr("Keep a hotkey for Control Center or Disconnect.").to_owned()));
    assert!(hotkeys().iter().find(|b| b.id == "stream_menu").unwrap().keys.is_some());
    for id in ["help", "capture", "mouse_mode", "overlay", "send_clipboard", "fetch_clipboard", "fullscreen", "minimize"] {
        turn_off_hotkey(id).unwrap();
    }
    assert_eq!(key_action(Scope::Stream, SDL_SCANCODE_TAB, SDL_Keymod(SDL_KMOD_LCTRL.0 | SDL_KMOD_LALT.0 | SDL_KMOD_LSHIFT.0)), Some(Action::StreamControls));
    set_hotkeys(HotkeyBindings::new());
}

/// The dialog records from SDL's key: F1 to F24, Shift with a digit, punctuation and the numpad
/// all arrive, by their place on the keyboard.
#[test]
fn a_recorded_key_is_its_place_with_the_modifiers_held() {
    isolate_thread_hotkeys_for_test();
    let ctrl_alt_shift = SDL_Keymod(SDL_KMOD_LCTRL.0 | SDL_KMOD_LALT.0 | SDL_KMOD_LSHIFT.0);
    let names = |k: &[&str]| Some(k.iter().map(|s| s.to_string()).collect::<Vec<_>>());
    assert_eq!(recorded_keys(SDL_SCANCODE_TAB, ctrl_alt_shift), names(&["CTRL", "ALT", "SHIFT", "TAB"]));
    assert_eq!(recorded_keys(SDL_SCANCODE_F9, SDL_KMOD_LCTRL), names(&["CTRL", "F9"]));
    assert_eq!(recorded_keys(SDL_SCANCODE_F12, SDL_KMOD_RCTRL), names(&["CTRL", "F12"]));
    assert_eq!(recorded_keys(SDL_SCANCODE_1, SDL_KMOD_LSHIFT), names(&["SHIFT", "1"]));
    assert_eq!(recorded_keys(SDL_SCANCODE_SLASH, SDL_KMOD_LALT), names(&["ALT", "SLASH"]));
    assert_eq!(recorded_keys(SDL_SCANCODE_KP_5, SDL_KMOD_LCTRL), names(&["CTRL", "NUMPAD_5"]));
    assert_eq!(recorded_keys(SDL_SCANCODE_Q, SDL_Keymod(SDL_KMOD_LGUI.0 | SDL_KMOD_LCTRL.0)), names(&["CTRL", "WIN", "Q"]));
    assert_eq!(recorded_keys(SDL_SCANCODE_F3, SDL_KMOD_NONE), names(&["F3"]));
    assert_eq!(recorded_keys(SDL_SCANCODE_LCTRL, SDL_KMOD_LCTRL), None, "a modifier alone waits");
    assert_eq!(recorded_keys(SDL_SCANCODE_RSHIFT, SDL_KMOD_RSHIFT), None);
    // QWERTZ: the key labelled Y sits where QWERTY has Z. Recorded there, it is matched there.
    let qwertz_y = SDL_SCANCODE_Z;
    let keys = recorded_keys(qwertz_y, ctrl_alt_shift).unwrap();
    assert_eq!(keys, ["CTRL", "ALT", "SHIFT", "Z"]);
    turn_off_hotkey("capture").unwrap();
    record_hotkey("overlay", &keys).unwrap();
    assert_eq!(key_action(Scope::Stream, qwertz_y, ctrl_alt_shift), Some(Action::Overlay));
    assert_eq!(key_action(Scope::Stream, SDL_SCANCODE_Y, ctrl_alt_shift), None);
    set_hotkeys(HotkeyBindings::new());
}

fn stream_key(action: Action) -> String {
    chord_for(Scope::Stream, action).unwrap().key.cap().into_owned()
}

/// QWERTY reads as it always did, punctuation by its name too.
#[test]
fn qwerty_keeps_every_name() {
    use_layout(&[]);
    assert_eq!(chord_for(Scope::Stream, Action::Capture).map(Chord::text).as_deref(), Some("Ctrl+Alt+Shift+Z"));
    for (action, key) in [(Action::StreamControls, "Tab"), (Action::StreamShortcuts, "H"), (Action::Disconnect, "Q"), (Action::Capture, "Z"), (Action::MouseMode, "M"),
        (Action::Overlay, "S"), (Action::SendClipboard, "V"), (Action::FetchClipboard, "C"), (Action::Fullscreen, "X"), (Action::Minimize, "D")] {
        assert_eq!(stream_key(action), key, "{action:?}");
    }
    for (name, label) in [("SLASH", "SLASH"), ("SEMICOLON", "SEMICOLON"), ("1", "1"), ("F9", "F9"), ("NUMPAD_5", "Num 5"), ("ESC", "Esc")] {
        assert_eq!(Key::Vk(policy::key_code(name).unwrap()).cap(), label);
    }
}

/// QWERTZ: the chord stays where it was, its text names the key the user has there. So does the
/// page that changes them, which shows a saved chord the same way.
#[test]
fn qwertz_names_the_key_at_the_chords_place() {
    use_layout(&[(SDL_SCANCODE_Z, SDLK_Y), (SDL_SCANCODE_Y, SDLK_Z), (SDL_SCANCODE_SLASH, SDLK_MINUS), (SDL_SCANCODE_MINUS, SDL_Keycode(0xDF)), (SDL_SCANCODE_SEMICOLON, SDL_Keycode(0xF6))]);
    assert_eq!(chord_for(Scope::Stream, Action::Capture).map(Chord::text).as_deref(), Some("Ctrl+Alt+Shift+Y"));
    let ctrl_alt_shift = SDL_Keymod(SDL_KMOD_LCTRL.0 | SDL_KMOD_LALT.0 | SDL_KMOD_LSHIFT.0);
    assert_eq!(key_action(Scope::Stream, SDL_SCANCODE_Z, ctrl_alt_shift), Some(Action::Capture), "the binding keeps its place");
    assert_eq!(key_action(Scope::Stream, SDL_SCANCODE_Y, ctrl_alt_shift), None);
    assert_eq!(Chord::of_keys(&["CTRL".into(), "Y".into()]).text(), "Ctrl+Z");
    assert_eq!(Chord::of_keys(&["CTRL".into(), "SLASH".into()]).text(), "Ctrl+-");
    assert_eq!(Chord::of_keys(&["CTRL".into(), "SEMICOLON".into()]).text(), "Ctrl+Ö");
    assert_eq!(Chord::of_keys(&["CTRL".into(), "MINUS".into()]).text(), "Ctrl+ß", "ß has no one-letter capital");
    assert_eq!(stream_key(Action::Disconnect), "Q");
    use_layout(&[]);
}

/// Every letter that moves, not only Z: AZERTY swaps A and Q, W and Z, and has M beside L.
#[test]
fn azerty_names_every_letter_that_moved() {
    use_layout(&[(SDL_SCANCODE_Q, SDLK_A), (SDL_SCANCODE_A, SDLK_Q), (SDL_SCANCODE_W, SDLK_Z), (SDL_SCANCODE_Z, SDLK_W), (SDL_SCANCODE_M, SDLK_COMMA), (SDL_SCANCODE_SEMICOLON, SDLK_M)]);
    assert_eq!(stream_key(Action::Disconnect), "A");
    assert_eq!(stream_key(Action::Capture), "W");
    assert_eq!(stream_key(Action::MouseMode), ",");
    assert_eq!(Key::Vk(policy::key_code("SEMICOLON").unwrap()).cap(), "M");
    assert_eq!(stream_key(Action::StreamControls), "Tab", "keys that don't print keep their names");
    use_layout(&[]);
}

/// A key that prints nothing there, or only a dead key's accent, keeps its usual name.
#[test]
fn a_layout_key_without_a_character_keeps_the_usual_name() {
    use_layout(&[(SDL_SCANCODE_Z, SDLK_F1), (SDL_SCANCODE_Q, SDLK_UNKNOWN), (SDL_SCANCODE_S, SDL_Keycode(0x0301)), (SDL_SCANCODE_C, SDLK_SPACE)]);
    for (action, key) in [(Action::Capture, "Z"), (Action::Disconnect, "Q"), (Action::Overlay, "S"), (Action::FetchClipboard, "C")] {
        assert_eq!(stream_key(action), key);
    }
    use_layout(&[]);
}
