//! Unit tests for `pin_ui` lock screen, timeout and store state transitions.

use super::*;
use crate::pin_lock::StubPinStore;

fn lock(tag: &str) -> (PinLock, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("nightjar-pin-ui-{tag}-{}", std::process::id()));
    (PinLock::with_store(Arc::new(StubPinStore::new(&dir))), dir)
}

fn type_pin(lock: &mut PinLock, pin: &str) {
    for d in pin.bytes() {
        lock.key(PadKey::Digit(d - b'0'));
    }
}

/// Waits for the worker's answer and takes it, as the next frame would.
fn finish(lock: &mut PinLock) -> Vec<Command> {
    let mut commands = Vec::new();
    for _ in 0..500 {
        if lock.job.is_none() {
            break;
        }
        lock.poll(&mut commands);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(lock.job.is_none(), "the worker answered");
    commands
}

#[test]
fn setting_a_pin_twice_turns_the_app_lock_on_without_locking_now() {
    let (mut lock, dir) = lock("set");
    lock.open_settings();
    assert!(lock.page && lock.pad.is_none(), "no PIN yet: the page opens straight away");
    lock.pad = Some(Pad::new(Purpose::Choose, lock.store.as_ref()));
    type_pin(&mut lock, "2468");
    lock.key(PadKey::Confirm);
    assert!(matches!(lock.pad.as_ref().unwrap().purpose, Purpose::Confirm(_)));
    type_pin(&mut lock, "2469");
    assert_eq!(lock.pad.as_ref().unwrap().purpose, Purpose::Choose, "a mismatch starts over");
    assert!(lock.pad.as_ref().unwrap().error.is_some());
    type_pin(&mut lock, "2468");
    lock.key(PadKey::Confirm);
    type_pin(&mut lock, "2468");
    finish(&mut lock);
    assert!(lock.store.has_pin() && lock.store.mode() == Mode::App);
    assert!(!lock.app_locked() && lock.page, "the app stays open after setting it");
    // Leaving the window past the grace time locks it, and the right PIN opens it.
    lock.store.set_grace_minutes(0);
    lock.left();
    assert!(!lock.page, "the page asks again next time");
    lock.returned();
    assert!(lock.app_locked());
    type_pin(&mut lock, "1111");
    finish(&mut lock);
    assert!(lock.app_locked() && lock.pad.as_ref().unwrap().error.is_some(), "wrong PIN");
    type_pin(&mut lock, "2468");
    let commands = finish(&mut lock);
    assert!(!lock.app_locked() && matches!(commands[..], [Command::PinPassed]));
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn a_stream_prompt_grants_one_start_or_cancels_it() {
    let (mut lock, dir) = lock("stream");
    lock.store.set_pin(b"1357").unwrap();
    lock.store.set_mode(Mode::Selected);
    lock.store.set_pcs(vec!["PC-1".into()]);
    assert!(!lock.needs_pin_for_stream("pc-2"));
    assert!(lock.needs_pin_for_stream("pc-1"));
    lock.ask_for_stream("pc-1", "Desk");
    let mut commands = Vec::new();
    lock.cancel(&mut commands);
    assert!(matches!(commands[..], [Command::PinCancelled]) && lock.pad.is_none());
    lock.ask_for_stream("pc-1", "Desk");
    type_pin(&mut lock, "1357");
    assert!(matches!(finish(&mut lock)[..], [Command::PinPassed]));
    assert!(!lock.needs_pin_for_stream("pc-1"), "granted once");
    assert!(lock.needs_pin_for_stream("pc-1"), "and used up");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn a_damaged_lock_file_is_named_on_the_lock_screen_and_takes_no_tries() {
    let dir = std::env::temp_dir().join(format!("nightjar-pin-ui-damaged-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("pin_lock.json"), "{ not json").unwrap();
    let mut lock = PinLock::with_store(Arc::new(StubPinStore::damaged(&dir)));
    assert!(lock.app_locked());
    let problem = lock.store.problem().expect("damaged");
    let said = problem_text(&problem, lock.store.path());
    assert!(said.contains(&dir.join("pin_lock.json").display().to_string()), "the full path: {said}");
    assert!(said.contains("Delete this file to remove the PIN lock"), "{said}");
    type_pin(&mut lock, "1234");
    lock.key(PadKey::Confirm);
    assert!(lock.job.is_none() && lock.pad.as_ref().unwrap().digits.is_empty(), "no try is made or counted");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn removing_the_pin_asks_for_it_first() {
    let (mut lock, dir) = lock("remove");
    lock.store.set_pin(b"8642").unwrap();
    lock.store.set_mode(Mode::Streams);
    lock.open_settings();
    assert!(!lock.page && matches!(lock.pad.as_ref().unwrap().purpose, Purpose::Verify(After::Open)));
    type_pin(&mut lock, "8642");
    finish(&mut lock);
    assert!(lock.page);
    lock.pad = Some(Pad::new(Purpose::Verify(After::Remove), lock.store.as_ref()));
    type_pin(&mut lock, "8642");
    finish(&mut lock);
    assert!(!lock.store.has_pin() && lock.store.mode() == Mode::Off);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn pad_ui_accepts_shifted_digits_and_rejects_ctrl_alt_command() {
    let (mut lock, dir) = lock("shift-digits");
    lock.store.set_pin(b"1234").unwrap();
    lock.store.set_mode(Mode::App);
    lock.pad = Some(Pad::new(Purpose::App, lock.store.as_ref()));

    let ctx = egui::Context::default();
    crate::theme::own_palette();
    crate::theme::apply(&ctx);

    let key_ev = |key: egui::Key, modifiers: egui::Modifiers| egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    };

    // Ctrl/Alt/Command modified digits must be ignored; Shift+digits (AZERTY/Czech) must be accepted.
    let raw = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 700.0))),
        events: vec![
            key_ev(egui::Key::Num9, egui::Modifiers::CTRL),
            key_ev(egui::Key::Num8, egui::Modifiers::ALT),
            key_ev(egui::Key::Num7, egui::Modifiers::COMMAND),
            key_ev(egui::Key::Num1, egui::Modifiers::SHIFT),
            key_ev(egui::Key::Num2, egui::Modifiers::SHIFT),
            key_ev(egui::Key::Num3, egui::Modifiers::NONE),
            key_ev(egui::Key::Num4, egui::Modifiers::SHIFT),
        ],
        ..Default::default()
    };
    let mut commands = Vec::new();
    let mut out = ctx.run_ui(raw, |ui| {
        lock.pad_ui(ui, &mut commands);
    });
    out.textures_delta.clear();

    let done = finish(&mut lock);
    assert!(
        !lock.app_locked() && matches!(done[..], [Command::PinPassed]),
        "Shift+digits should unlock the PIN pad while Ctrl/Alt/Cmd digits are ignored"
    );
    std::fs::remove_dir_all(dir).ok();
}
