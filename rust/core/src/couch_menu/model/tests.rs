use std::time::{Duration, Instant};

use super::*;
use crate::native_api::radeon::{Choice, Range};

/// The `couch_menu` catalogue in English and German, copied from the host's
/// catalogue rule (Latin-1 only, "..." never "…").
pub(crate) const CATALOGUE: &[(&str, &str, &str)] = &[
    ("title", "Host menu", "Host-Menü"),
    ("back_to_game", "Back to game", "Zurück zum Spiel"),
    ("radeon", "Radeon settings", "Radeon-Einstellungen"),
    (
        "restore_radeon",
        "Restore Radeon settings",
        "Radeon-Einstellungen zurücksetzen",
    ),
    ("back", "Back", "Zurück"),
    ("profile", "Profile: ", "Profil: "),
    (
        "disconnect",
        "Disconnect (game keeps running)",
        "Trennen (Spiel läuft weiter)",
    ),
    ("end_session", "End session", "Sitzung beenden"),
    ("close_game", "Close game", "Spiel schließen"),
    (
        "next_stream",
        "Applies from the next stream",
        "Gilt ab dem nächsten Stream",
    ),
    (
        "confirm",
        "Press again within 5 s to confirm",
        "Bestätigen: in 5 s erneut drücken",
    ),
    ("working", "Working...", "Wird ausgeführt..."),
    ("failed", "Failed: ", "Fehlgeschlagen: "),
    (
        "needs_host_control",
        "Needs host control",
        "Erfordert Host-Steuerung",
    ),
    ("on", "On", "Ein"),
    ("off", "Off", "Aus"),
    (
        "hint",
        "Enter/A: select  Esc/B: back",
        "Enter/A: wählen  Esc/B: zurück",
    ),
];

/// Strings the host puts into the [`Snapshot`]: preset and item labels, the
/// readings line.
pub(crate) const HOST_STRINGS: &[(&str, &str)] = &[
    ("Stream preset", "Stream-Voreinstellung"),
    ("Lowest latency", "Niedrigste Latenz"),
    ("Balanced", "Ausgewogen"),
    ("Reference quality", "Referenzqualität"),
    (
        "GPU readings are off (hardware_metrics)",
        "GPU-Werte sind aus (hardware_metrics)",
    ),
    ("Night mode", "Nachtmodus"),
    ("Headphone surround", "Kopfhörer-Surround"),
    ("Encoder focus", "Encoder-Fokus"),
    ("Still areas", "Ruhige Bereiche"),
    ("Still areas and centre", "Ruhige Bereiche und Mitte"),
    ("Automatic", "Automatisch"),
    ("Sleep after idle", "Ruhezustand nach Leerlauf"),
    ("15 min", "15 Min."),
    (
        "Not supported on this GPU",
        "Auf dieser GPU nicht unterstützt",
    ),
];

pub(crate) fn strings(german: bool) -> Strings {
    Strings::from_lookup(|path| {
        let key = path.strip_prefix("couch_menu/")?;
        let (_, en, de) = CATALOGUE.iter().find(|(k, ..)| *k == key)?;
        Some(if german { de } else { en }.to_string())
    })
    .unwrap()
}

fn choice(id: &str, label: &str, index: usize, options: &[(&str, &str)]) -> Setting {
    Setting {
        id: id.into(),
        label: label.into(),
        control: Control::Choice {
            index,
            options: options
                .iter()
                .map(|(value, label)| Choice {
                    value: value.to_string(),
                    label: label.to_string(),
                })
                .collect(),
        },
        availability: Availability::Ready,
        applies: Applies::Live,
        needs_host_control: false,
    }
}

fn toggle(id: &str, label: &str, on: bool) -> Setting {
    Setting {
        control: Control::Toggle(on),
        ..choice(id, label, 0, &[])
    }
}

fn preset() -> Setting {
    Setting {
        applies: Applies::NextStream,
        needs_host_control: true,
        ..choice(
            "stream_preset",
            "Stream preset",
            1,
            &[
                ("lowest_latency", "Lowest latency"),
                ("balanced", "Balanced"),
                ("reference", "Reference quality"),
            ],
        )
    }
}

/// A host with a Radeon page, the preset and one fake extension item.
pub(crate) fn snapshot() -> Snapshot {
    let integer = |id: &str, label: &str, value, unit: &str| Setting {
        control: Control::Integer {
            value,
            range: Range {
                min: 0,
                max: 100,
                step: 10,
            },
            unit: Some(unit.into()),
        },
        needs_host_control: true,
        ..choice(id, label, 0, &[])
    };
    Snapshot {
        status: "72 °C 210 W 98% 2560x1440 120 Hz".into(),
        host_control: true,
        radeon: Some(RadeonPage {
            gpu: 0,
            name: "AMD Radeon RX 7900 XT".into(),
            profile: Some("Rubylight".into()),
            settings: vec![
                Setting {
                    needs_host_control: true,
                    ..toggle("graphics.anti_lag", "Anti-Lag", true)
                },
                integer("graphics.sharpness", "Image sharpening", 80, "%"),
                Setting {
                    availability: Availability::Locked("Not supported on this GPU".into()),
                    ..toggle("graphics.chill", "Radeon Chill", false)
                },
            ],
        }),
        preset: Some(preset()),
        items: vec![toggle(items::AUDIO_NIGHT_MODE, "Night mode", false)],
    }
}

fn layout() -> Layout {
    Layout::for_stream(1920, 1080).unwrap()
}

struct Run {
    menu: Menu,
    snap: Snapshot,
    now: Instant,
}

impl Run {
    fn new(snap: Snapshot) -> Self {
        let mut run = Self {
            menu: Menu::new(strings(false)),
            snap,
            now: Instant::now(),
        };
        run.nav(Nav::Opened);
        run
    }
    fn nav(&mut self, nav: Nav) -> Option<Action> {
        self.menu.nav(nav, self.now, &self.snap, &layout())
    }
    fn after(&mut self, by: Duration) {
        self.now += by;
    }
    fn text(&self) -> Vec<String> {
        self.menu
            .lines(&self.snap)
            .into_iter()
            .map(|l| l.text.trim_end().to_owned())
            .collect()
    }
    fn focused(&self) -> String {
        let lines = self.menu.lines(&self.snap);
        let line = lines
            .iter()
            .find(|l| matches!(l.style, Style::Row { focus: true, .. }))
            .unwrap();
        line.text.clone()
    }
    fn message(&self) -> String {
        let lines = self.menu.lines(&self.snap);
        lines[lines.len() - 2].text.clone()
    }
    /// Moves the focus down to the row whose text starts with `label`.
    fn focus(&mut self, label: &str) {
        for _ in 0..32 {
            if self.focused().starts_with(label) {
                return;
            }
            self.nav(Nav::Down);
        }
        panic!("no row {label}");
    }
}

#[test]
fn couch_menu_strings_load_from_the_catalogue_and_fail_loudly() {
    assert_eq!(Strings::KEYS.len(), CATALOGUE.len());
    for (key, (catalogue, ..)) in Strings::KEYS.iter().zip(CATALOGUE) {
        assert_eq!(key, catalogue);
    }
    assert_eq!(strings(true).title, "Host-Menü");
    let missing = Strings::from_lookup(|path| (path != "couch_menu/hint").then(|| "x".into()));
    assert_eq!(
        missing,
        Err("couch menu: string couch_menu/hint is missing".into())
    );
    let ellipsis = Strings::from_lookup(|path| {
        Some(
            if path == "couch_menu/working" {
                "Working…"
            } else {
                "x"
            }
            .into(),
        )
    });
    assert_eq!(
        ellipsis,
        Err("couch menu: string couch_menu/working has '…', outside Latin-1".into())
    );
}

#[test]
fn couch_menu_font_covers_every_catalogue_string() {
    for (_, en, de) in CATALOGUE {
        for text in [en, de] {
            assert!(
                text.chars().count() <= COLUMNS as usize,
                "{text} is wider than a line"
            );
        }
    }
    let host = HOST_STRINGS.iter().flat_map(|(en, de)| [en, de]);
    let catalogue = CATALOGUE.iter().flat_map(|(_, en, de)| [en, de]);
    for text in catalogue.chain(host).chain(&["0123456789", "°%./:-"]) {
        assert_eq!(font::uncovered(text), None, "{text}");
    }
}

#[test]
fn couch_menu_model_main_page_rows_in_order() {
    let run = Run::new(snapshot());
    assert_eq!(
        run.text(),
        [
            "Host menu",
            "72 °C 210 W 98% 2560x1440 120 Hz",
            "Back to game",
            "Radeon settings                  >",
            "Stream preset             Balanced",
            "  Applies from the next stream",
            "Night mode                     Off",
            "Disconnect (game keeps running)",
            "End session",
            "Close game",
            "",
            "Enter/A: select  Esc/B: back",
        ]
    );
    assert!(run.focused().starts_with("Back to game"));
    // Without readings, Radeon or items, the page shrinks to what is left.
    let run = Run::new(Snapshot {
        status: "GPU readings are off (hardware_metrics)".into(),
        host_control: true,
        ..Snapshot::default()
    });
    assert_eq!(
        run.text()[1..6],
        [
            "GPU readings are off (hardware_...",
            "Back to game",
            "Disconnect (game keeps running)",
            "End session",
            "Close game",
        ],
        "a long line is cut to 34 columns"
    );
}

#[test]
fn couch_menu_model_locked_rows_keep_their_reason() {
    let mut run = Run::new(snapshot());
    run.nav(Nav::Down);
    run.nav(Nav::Accept);
    run.focus("Radeon Chill");
    assert_eq!(run.message(), "Not supported on this GPU");
    assert_eq!(
        run.focused().trim_end(),
        "Radeon Chill                   Off",
        "no arrows"
    );
    for nav in [Nav::Accept, Nav::Left, Nav::Right] {
        assert_eq!(run.nav(nav), None, "{nav:?}");
    }
    assert_eq!(
        run.message(),
        "Not supported on this GPU",
        "the reason stays"
    );

    // Without host control every row that needs it is locked with that reason.
    let mut run = Run::new(Snapshot {
        host_control: false,
        ..snapshot()
    });
    run.focus("Stream preset");
    assert_eq!(run.message(), "Needs host control");
    assert_eq!(run.nav(Nav::Right), None);
    run.focus("Night mode");
    assert_eq!(
        run.message(),
        "",
        "an item that does not need it stays ready"
    );
    assert!(run.nav(Nav::Accept).is_some());
    run.menu.finish(Ok(()));
    run.focus("Close game");
    assert_eq!(run.message(), "Needs host control");
    assert_eq!(run.nav(Nav::Accept), None);
    assert_eq!(run.nav(Nav::Accept), None);

    // A hidden item has no row at all.
    let mut snap = snapshot();
    snap.items[0].availability = Availability::Hidden;
    let run = Run::new(snap);
    assert!(!run.text().iter().any(|line| line.starts_with("Night mode")));
}

#[test]
fn couch_menu_model_confirm_window_is_5_s() {
    let mut run = Run::new(snapshot());
    run.focus("End session");
    assert_eq!(run.nav(Nav::Accept), None);
    assert_eq!(run.message(), "Press again within 5 s to confirm");
    assert_eq!(run.menu.next_due(), Some(run.now + CONFIRM));
    run.after(Duration::from_millis(4_999));
    assert_eq!(run.nav(Nav::Accept), Some(Action::EndSession));
    run.menu.finish(Ok(()));

    // At 5 s the window has shut: the press arms it again.
    run.focus("Close game");
    assert_eq!(run.nav(Nav::Accept), None);
    run.after(CONFIRM);
    assert!(run.menu.tick(run.now), "the expiry redraws");
    assert_eq!(run.message(), "");
    assert_eq!(run.menu.next_due(), None);
    assert_eq!(run.nav(Nav::Accept), None);
    assert_eq!(run.nav(Nav::Accept), Some(Action::CloseGame));
    run.menu.finish(Ok(()));

    // Moving away cancels it, and a confirm is per row.
    run.focus("End session");
    run.nav(Nav::Accept);
    run.nav(Nav::Down);
    assert_eq!(run.message(), "");
    assert_eq!(
        run.nav(Nav::Accept),
        None,
        "Close game needs its own two presses"
    );
    run.nav(Nav::Up);
    assert_eq!(run.nav(Nav::Accept), None, "End session starts over");
    assert_eq!(run.nav(Nav::Accept), Some(Action::EndSession));
}

#[test]
fn couch_menu_model_back_and_close() {
    let mut run = Run::new(snapshot());
    assert_eq!(run.nav(Nav::Accept), Some(Action::Close), "Back to game");
    assert!(!run.menu.busy(), "closing is not an action to wait for");
    assert_eq!(
        run.nav(Nav::Back),
        Some(Action::Close),
        "Back on the main page"
    );
    // Back on the Radeon page goes to the main page, focus on its row.
    run.nav(Nav::Down);
    run.nav(Nav::Accept);
    assert_eq!(run.text()[0], "Radeon settings");
    assert_eq!(run.text()[1], "Profile: Rubylight");
    assert_eq!(run.nav(Nav::Back), None);
    assert_eq!(run.text()[0], "Host menu");
    assert!(run.focused().starts_with("Radeon settings"));
    // So does its Back row.
    run.nav(Nav::Accept);
    assert!(run.focused().starts_with("Back"));
    assert_eq!(run.nav(Nav::Accept), None);
    assert!(run.focused().starts_with("Radeon settings"));
    // A tap outside the panel closes; one on its padding does nothing.
    assert_eq!(
        run.nav(Nav::Point { x: 0.05, y: 0.05 }),
        Some(Action::Close)
    );
    let rect = layout().panel(run.text().len() as u32);
    let (x, y) = (
        (rect.origin[0] + 4) as f32 / 1920.,
        (rect.origin[1] + 4) as f32 / 1080.,
    );
    assert_eq!(run.nav(Nav::Point { x, y }), None);
    // The Radeon page going away mid-page falls back to the main page.
    run.nav(Nav::Accept);
    run.snap.radeon = None;
    assert_eq!(run.text()[0], "Host menu");
    assert_eq!(run.nav(Nav::Back), Some(Action::Close));
    // Opened and Closed reset to the main page's first row.
    run.snap = snapshot();
    run.focus("End session");
    run.nav(Nav::Accept);
    run.nav(Nav::Closed);
    run.nav(Nav::Opened);
    assert!(run.focused().starts_with("Back to game"));
    assert_eq!(run.message(), "");
}

#[test]
fn couch_menu_model_changes_go_out_as_native_api_changes() {
    let mut run = Run::new(snapshot());
    let change = |target, id: &str, value| {
        Some(Action::Change {
            target,
            change: Change {
                id: id.into(),
                value,
            },
        })
    };
    run.focus("Stream preset");
    assert_eq!(run.focused(), "Stream preset         < Balanced >");
    let reference = Value::Choice("reference".into());
    assert_eq!(
        run.nav(Nav::Right),
        change(Target::Preset, "stream_preset", reference)
    );
    run.menu.finish(Ok(()));
    run.snap.preset.as_mut().unwrap().control = preset().control;
    let lowest = Value::Choice("lowest_latency".into());
    assert_eq!(
        run.nav(Nav::Left),
        change(Target::Preset, "stream_preset", lowest)
    );
    run.menu.finish(Ok(()));
    let Control::Choice { index, .. } = &mut run.snap.preset.as_mut().unwrap().control else {
        unreachable!()
    };
    *index = 0;
    assert_eq!(run.nav(Nav::Left), None, "the first choice stops Left");
    let balanced = Value::Choice("balanced".into());
    assert_eq!(
        run.nav(Nav::Accept),
        change(Target::Preset, "stream_preset", balanced)
    );
    run.menu.finish(Ok(()));

    run.focus("Night mode");
    let night = items::AUDIO_NIGHT_MODE;
    assert_eq!(run.nav(Nav::Left), None, "already off");
    assert_eq!(
        run.nav(Nav::Right),
        change(Target::Item, night, Value::Bool(true))
    );
    run.menu.finish(Ok(()));
    assert_eq!(
        run.nav(Nav::Accept),
        change(Target::Item, night, Value::Bool(true))
    );
    run.menu.finish(Ok(()));

    run.nav(Nav::Back);
    run.focus("Radeon settings");
    run.nav(Nav::Accept);
    run.focus("Image sharpening");
    assert_eq!(run.focused(), "Image sharpening          < 80 % >");
    let radeon = Target::Radeon { gpu: 0 };
    let sharp = "graphics.sharpness";
    assert_eq!(
        run.nav(Nav::Right),
        change(radeon, sharp, Value::Integer(90))
    );
    run.menu.finish(Ok(()));
    assert_eq!(
        run.nav(Nav::Left),
        change(radeon, sharp, Value::Integer(70))
    );
    run.menu.finish(Ok(()));
    let settings = &mut run.snap.radeon.as_mut().unwrap().settings;
    let Control::Integer { value, .. } = &mut settings[1].control else {
        unreachable!()
    };
    *value = 100;
    assert_eq!(run.nav(Nav::Right), None, "the top stops Right");
    assert_eq!(
        run.nav(Nav::Accept),
        change(radeon, sharp, Value::Integer(0)),
        "Accept wraps"
    );
    run.menu.finish(Ok(()));
    run.focus("Restore Radeon settings");
    assert_eq!(run.nav(Nav::Accept), Some(Action::RestoreRadeon { gpu: 0 }));
}

#[test]
fn couch_menu_model_working_and_failures() {
    let mut run = Run::new(snapshot());
    run.focus("Disconnect");
    assert_eq!(run.nav(Nav::Accept), Some(Action::Disconnect));
    assert!(run.menu.busy());
    assert_eq!(run.focused(), "Disconnect (game kee... Working...");
    assert_eq!(run.nav(Nav::Accept), None, "accepts wait for the action");
    run.focus("Night mode");
    assert_eq!(run.nav(Nav::Right), None);
    run.menu
        .finish(Err("access denied by the session owner".into()));
    assert!(!run.menu.busy());
    assert_eq!(run.message(), "Failed: access denied by the se...");
    run.nav(Nav::Down);
    assert_eq!(
        run.message(),
        "Failed: access denied by the se...",
        "until the next action"
    );
    run.nav(Nav::Up);
    assert!(run.nav(Nav::Accept).is_some());
    assert_eq!(run.message(), "");
}

#[test]
fn couch_menu_model_a_tap_focuses_and_accepts_its_row() {
    let mut run = Run::new(snapshot());
    let lines = run.text();
    let rect = layout().panel(lines.len() as u32);
    let at = |line: usize| Nav::Point {
        x: (rect.origin[0] + 100) as f32 / 1920.,
        y: (rect.origin[1] + 16 + line as u32 * 32 + 10) as f32 / 1080.,
    };
    let disconnect = lines
        .iter()
        .position(|l| l.starts_with("Disconnect"))
        .unwrap();
    assert_eq!(run.nav(at(disconnect)), Some(Action::Disconnect));
    assert!(run.focused().starts_with("Disconnect"));
    run.menu.finish(Ok(()));
    let note = lines
        .iter()
        .position(|l| l.starts_with("  Applies"))
        .unwrap();
    let balanced = Value::Choice("reference".into());
    assert_eq!(
        run.nav(at(note)),
        Some(Action::Change {
            target: Target::Preset,
            change: Change {
                id: "stream_preset".into(),
                value: balanced,
            },
        }),
        "a note belongs to its row"
    );
    run.menu.finish(Ok(()));
    let end = lines.iter().position(|l| l == "End session").unwrap();
    assert_eq!(run.nav(at(end)), None);
    assert_eq!(
        run.nav(at(end)),
        Some(Action::EndSession),
        "two taps confirm"
    );
    assert_eq!(run.nav(at(0)), None, "the title is no row");
}

#[test]
fn couch_menu_model_long_pages_scroll_to_the_focus() {
    let mut snap = snapshot();
    snap.items = (0..20)
        .map(|i| toggle(&format!("item{i}"), &format!("Item {i}"), false))
        .collect();
    let mut run = Run::new(snap);
    for _ in 0..30 {
        let lines = run.menu.lines(&run.snap);
        assert!(lines.len() <= MAX_LINES as usize);
        assert!(
            lines
                .iter()
                .any(|l| matches!(l.style, Style::Row { focus: true, .. }))
        );
        assert!(
            lines
                .iter()
                .all(|l| l.text.chars().count() <= COLUMNS as usize)
        );
        run.nav(Nav::Down);
    }
    assert!(Layout::for_stream(640, 360).is_ok());
}

#[test]
fn couch_menu_model_odd_host_values_do_not_panic_the_worker() {
    let mut snap = snapshot();
    snap.items = vec![
        choice("empty", "Empty choice", 3, &[]),
        Setting {
            control: Control::Integer {
                value: i64::MAX,
                range: Range {
                    min: 10,
                    max: 0,
                    step: 0,
                },
                unit: None,
            },
            ..choice("inverted", "Inverted range", 0, &[])
        },
    ];
    let mut run = Run::new(snap);
    run.focus("Empty choice");
    for nav in [Nav::Accept, Nav::Left, Nav::Right] {
        assert_eq!(run.nav(nav), None, "{nav:?}");
    }
    run.focus("Inverte");
    for nav in [Nav::Accept, Nav::Left, Nav::Right] {
        run.nav(nav);
        run.menu.finish(Ok(()));
    }
    assert!(run.text().len() <= MAX_LINES as usize);
}

#[test]
fn couch_menu_setting_from_a_native_api_setting() {
    let base = radeon::Setting {
        id: "encoder.focus".into(),
        group: "encoder".into(),
        kind: Kind::Choice,
        state: ControlState::Configurable,
        value: Some(Value::Choice("still".into())),
        range: None,
        options: vec![
            Choice {
                value: "off".into(),
                label: "Off".into(),
            },
            Choice {
                value: "still".into(),
                label: "Still areas".into(),
            },
        ],
        unit: None,
        restore: None,
        reason: None,
        message: None,
    };
    let row = Setting::from_radeon(&base, "Encoder focus".into());
    assert_eq!(row.id, "encoder.focus");
    assert_eq!(row.availability, Availability::Ready);
    assert!(matches!(row.control, Control::Choice { index: 1, .. }));
    let locked = Setting::from_radeon(
        &radeon::Setting {
            state: ControlState::ReadOnly,
            reason: Some("Set by the host".into()),
            ..base.clone()
        },
        "Encoder focus".into(),
    );
    assert_eq!(
        locked.availability,
        Availability::Locked("Set by the host".into())
    );
    let unknown = Setting::from_radeon(
        &radeon::Setting {
            state: ControlState::Unsupported,
            kind: Kind::Unknown,
            value: Some(Value::Integer(3)),
            ..base.clone()
        },
        "x".into(),
    );
    assert_eq!(unknown.control, Control::Text("3".into()));
    assert_eq!(
        unknown.availability,
        Availability::Locked("Unsupported".into())
    );
    let integer = Setting::from_radeon(
        &radeon::Setting {
            kind: Kind::Integer,
            value: Some(Value::Integer(40)),
            range: Some(Range {
                min: 0,
                max: 100,
                step: 5,
            }),
            unit: Some("%".into()),
            ..base
        },
        "x".into(),
    );
    assert!(matches!(
        integer.control,
        Control::Integer { value: 40, .. }
    ));
    assert_eq!(
        items::label_path(items::ENCODER_FOCUS),
        "couch_menu/items/encoder_focus"
    );
}
