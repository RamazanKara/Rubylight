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
    // The focused text box of a number box is an edit; the spinner is its parent.
    assert_eq!(
        kind(&FocusedElement {
            in_spinner: true,
            ..edit("Custom scaling")
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

/// A tooltip over the field or the host's own window takes no keys from the field: the
/// watcher reports nothing, so no lower and raise go out for it.
#[test]
fn tooltips_and_the_hosts_own_windows_leave_the_focus_where_it_was() {
    let tooltip = FocusedElement {
        control_type: control_type::TOOL_TIP,
        ..FocusedElement::default()
    };
    assert!(passes_over(&tooltip, false));
    assert!(passes_over(&FocusedElement::default(), true));
    assert!(passes_over(&edit("Name"), true));
    let button = FocusedElement {
        control_type: 50000,
        ..edit("OK")
    };
    assert!(
        !passes_over(&button, false),
        "another app's button takes the focus"
    );
    assert!(!passes_over(&edit("Name"), false));
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
    // The end is kept whole when the window would start inside a character.
    assert_eq!(
        bound(&around("\u{e9}\u{e9}\u{e9}", None), 5).0,
        "\u{e9}\u{e9}"
    );
    assert_eq!(bound(&around("a\u{1f600}b", None), 3).0, "b");
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
        element_ids: false,
        max_text,
    }
    .encode()
}

fn request_ids() -> [u8; 4] {
    TextInputRequest {
        notices: true,
        element_ids: true,
        ..TextInputRequest::default()
    }
    .encode()
}

fn focused(name: &str, text: &str) -> Option<Focused> {
    in_element(name, text, 1)
}

fn in_element(name: &str, text: &str, element: u32) -> Option<Focused> {
    Some(Focused {
        field: classify(&edit(name)).unwrap(),
        element,
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
            element: 2,
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

/// Two fields of one kind look the same to the client when it asked for no text; moving
/// from one to the other is still an update, so it stops composing the word it was.
#[test]
fn moving_to_another_field_of_the_same_kind_is_an_update() {
    let now = Instant::now();
    let mut inputs = TextInputs::default();
    inputs.on_request(&request(false, 0), true, false);
    inputs.note(in_element("First name", "", 1), now);
    assert_eq!(notice(inputs.take(now)).kind, NoticeKind::Raise);
    // The same element again, as UI Automation reports a refocus: nothing.
    inputs.note(in_element("First name", "", 1), now + INTERVAL);
    assert_eq!(inputs.take(now + INTERVAL), None);
    inputs.note(in_element("Last name", "", 2), now + INTERVAL);
    let update = notice(inputs.take(now + INTERVAL));
    assert_eq!(
        (update.kind, update.input_type, update.serial),
        (NoticeKind::Update, InputType::Text, 2)
    );
    assert!(update.text.is_empty());
}

#[test]
fn element_ids_are_stable_per_runtime_id_and_never_0() {
    // An edit box's runtime id: UiaAppendRuntimeId (3), the window, then the provider's.
    let first = element_id(&[42, 0x0004_0A3C, 4, 1]);
    assert_eq!(first, element_id(&[42, 0x0004_0A3C, 4, 1]));
    assert_ne!(first, element_id(&[42, 0x0004_0A3C, 4, 2]));
    assert_ne!(first, element_id(&[42, 0x0004_0A3D, 4, 1]));
    assert_ne!(element_id(&[1, 2]), element_id(&[2, 1]));
    assert_ne!(element_id(&[]), 0);
    assert_eq!(element_id(&[]), 0x811C_9DC5, "FNV-1a of nothing");
}

/// A client that asked for element ids hears which element has the focus, the same id
/// again when it comes back to it; every other client gets version 1 notices.
#[test]
fn element_ids_go_only_to_a_client_that_asked() {
    let now = Instant::now();
    let mut inputs = TextInputs::default();
    inputs.on_request(&request_ids(), true, false);
    inputs.note(in_element("First name", "", 0x1234_5678), now);
    let wire = inputs.take(now).expect("a raise");
    assert_eq!(wire[0], 2);
    let raise = TextInputNotice::decode(&wire).unwrap();
    assert_eq!(
        (raise.kind, raise.element),
        (NoticeKind::Raise, 0x1234_5678)
    );
    let later = now + INTERVAL;
    inputs.note(in_element("Last name", "", 99), later);
    assert_eq!(notice(inputs.take(later)).element, 99);
    let back = later + INTERVAL;
    inputs.note(in_element("First name", "", 0x1234_5678), back);
    let update = notice(inputs.take(back));
    assert_eq!(
        (update.kind, update.element),
        (NoticeKind::Update, 0x1234_5678)
    );
    // A lower names no element and is version 1.
    let left = back + INTERVAL;
    inputs.note(None, left);
    let wire = inputs.take(left + LOWER_DELAY).expect("a lower");
    assert_eq!(wire[0], 1);
    assert_eq!(TextInputNotice::decode(&wire).unwrap().element, 0);

    // Not asked: the same focus goes out as version 1, without the id.
    let mut inputs = TextInputs::default();
    inputs.on_request(&request(false, 0), true, false);
    inputs.note(in_element("First name", "", 0x1234_5678), now);
    let wire = inputs.take(now).expect("a raise");
    assert_eq!(wire[0], 1);
    assert_eq!(
        wire.len(),
        rubylight_protocol::text_input::TEXT_INPUT_NOTICE_HEADER_BYTES
    );
    assert_eq!(TextInputNotice::decode(&wire).unwrap().element, 0);
}
