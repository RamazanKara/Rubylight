//! The Controller section, the controller editor: every controller setting in one place. Its main rows
//! (Controllers, the buttons page and its test, the dead zones, motion and vibration), then the
//! Controllers, Stream menu and USB groups. Android's Controller section shows the same rows
//! (`StreamSettings.SECTIONS`, "controller"); Controls opens it with its Controller row.

use super::*;

pub(super) fn draw_controller_section(
    ui: &mut Ui,
    s: &mut Settings,
    rows: &mut Rows<'_>,
    layered: bool,
    links: &mut Vec<(&'static str, Response)>,
    style: crate::glyphs::Style,
    open_controllers: &mut Option<bool>,
) {
    rows.section(ui, en("Controller"));
    setting_row(ui, s, rows, &["controllers", "multi_controller"], "Controllers", "A player each makes every controller its own player and suits almost everyone. One player makes every controller player 1; Off sends no controller to the PC.", |ui, s| {
        let current = Controllers::from_saved(s.controllers, s.multi_controller);
        let mut choice = current;
        let response = Combo::from_id_salt("controllers").selected_text(tr(controllers_name(current))).show_ui(ui, |ui| {
            for each in Controllers::ALL {
                ui.selectable_value(&mut choice, each, tr(controllers_name(each)));
            }
        }).response;
        if choice != current {
            (s.controllers, s.multi_controller) = choice.to_saved(s.multi_controller);
        }
        response
    });
    if !layered {
        rows.action("controller_buttons");
        setting_row(ui, s, rows, &[], "Controller buttons", "Remaps a controller model’s buttons and sticks and sets the type the PC sees (Xbox, PlayStation or Nintendo), for when a game shows the wrong buttons.", |ui, _| {
            let button = quiet_button(ui, "Controller buttons");
            if button.clicked() { *open_controllers = Some(false); }
            links.push(("controllers-link", button.clone()));
            button
        });
        rows.action("controller_test");
        setting_row(ui, s, rows, &[], "Controller test", "Shows the buttons, sticks and triggers a controller sends.", |ui, _| {
            let button = quiet_button(ui, "Test");
            if button.clicked() { *open_controllers = Some(true); }
            links.push(("controller-test-link", button.clone()));
            button
        });
    }
    // Main rows while controllers reach the PC, then the rest of the group behind the fold.
    rows.when(s.controllers);
    // For every controller whose buttons page sets none of its own.
    setting_row(ui, s, rows, &["deadzone"], "Stick dead zone", "Ignores small stick movements near the center. Raise it if a resting stick drifts; lower it so small movements register. Games can add their own dead zone on top.", |ui, s| {
        slider(ui, &mut s.deadzone, crate::config::DEADZONE, 1.0, percent, |rail| rail, |field| field.suffix("%"))
    });
    setting_row(ui, s, rows, &["trigger_deadzone"], "Trigger dead zone", "Ignores light trigger presses. Raise it if a resting trigger presses by itself; lower it so light presses register. Games can add their own dead zone on top.", |ui, s| {
        slider(ui, &mut s.trigger_deadzone, 0..=u32::from(crate::gamepad_types::MAX_TRIGGER_DEADZONE), 1.0, percent, |rail| rail, |field| field.suffix("%"))
    });
    setting_row(ui, s, rows, &["controller_motion"], "Motion controls", "Sends the controller’s gyro and accelerometer to games, for motion aiming. The PC may then treat it as a PlayStation controller; turn it off if games show the wrong buttons.", |ui, s| toggle(ui, &mut s.controller_motion));
    // Off stops every vibration a game asks for; the others shape it. Stored in the two fields as before.
    setting_row(ui, s, rows, &["disable_rumble", "rumble_feel"], "Vibration", "Strong makes weak vibration strong enough for a small motor to show; Subtle caps strong vibration and softens weak. Either one changes the balance of weak and strong rumble the game chose, so leave it on Standard unless vibration is too faint or too harsh.", |ui, s| {
        let current = vibration_choice(s);
        let mut choice = current;
        let response = Combo::from_id_salt("vibration").selected_text(vibration_name(current)).show_ui(ui, |ui| {
            for each in VIBRATION_CHOICES {
                ui.selectable_value(&mut choice, each, vibration_name(each));
            }
        }).response;
        if choice != current {
            set_vibration_choice(s, choice);
        }
        response
    });
    rows.group(en("Controllers"));
    rows.when(s.controllers);
    setting_row(ui, s, rows, &["swap_face_buttons"], "Nintendo button layout", "Swaps A with B and X with Y. Turn on if a Nintendo-style controller’s buttons should send the letters printed on them.", |ui, s| toggle(ui, &mut s.swap_face_buttons));
    setting_row(ui, s, rows, &["gamepad_quit_combo"], "Buttons to leave the stream", "Hold these buttons together to leave the stream from the controller, without opening the Control Center. Pick a combination your games don’t use.", |ui, s| {
        Combo::from_id_salt("gamepad-quit-combo").selected_text(style.quit_combo(s.gamepad_quit_combo)).show_ui(ui, |ui| {
            for combo in crate::config::GamepadQuitCombo::ALL {
                ui.selectable_value(&mut s.gamepad_quit_combo, combo, style.quit_combo(combo));
            }
        }).response
    });
    setting_row(ui, s, rows, &["controller_mouse"], "Controller mouse mode", "Holding Start switches the controller between gamepad and mouse. Turn it off if that happens by accident in a game; the Control Center’s Mouse tile still switches it.", |ui, s| toggle(ui, &mut s.controller_mouse));
    rows.when(s.controller_mouse);
    setting_row(ui, s, rows, &["analog_scrolling"], "Stick for scrolling", "The stick that scrolls in controller mouse mode; the other stick moves the pointer. Without a scroll stick, both move the pointer.", |ui, s| {
        use crate::config::AnalogScrolling;
        Combo::from_id_salt("analog-scrolling").selected_text(scrolling_name(s.analog_scrolling)).show_ui(ui, |ui| {
            for stick in [AnalogScrolling::Off, AnalogScrolling::Left, AnalogScrolling::Right] {
                ui.selectable_value(&mut s.analog_scrolling, stick, scrolling_name(stick));
            }
        }).response
    });
    setting_row(ui, s, rows, &["mouse_fixed_step"], "Precise controller mouse", "In controller mouse mode, holding X moves the pointer in fixed pixel steps, for small targets. Each press of Y doubles the step, from 1 to 128 pixels.", |ui, s| toggle(ui, &mut s.mouse_fixed_step));
    rows.always();
    rows.when(s.controllers);
    setting_row(ui, s, rows, &["gamepad_touchpad_as_mouse"], "Controller touchpad as mouse", "The controller’s touchpad (DualSense and similar) moves the PC’s pointer like a mouse. Games then no longer get its touches; leave it off for games that use the touchpad.", |ui, s| toggle(ui, &mut s.gamepad_touchpad_as_mouse));
    setting_row(ui, s, rows, &["background_controllers"], "Input in the background", "Controllers keep working in the stream while another window is active. Presses meant for another app then also reach the PC; turn on only to play with Nightjar in the background.", |ui, s| {
        toggle(ui, &mut s.background_controllers)
    });
    rows.always();
    // How the stream menu opens and what it holds, as Android's Stream menu group. Android's
    // floating menu button has no desktop row: a window has a pointer and keys to open the menu.
    rows.group(en("Control Center"));
    rows.when(s.controllers);
    setting_row(ui, s, rows, &["guide_hold_ms"], "Hold Guide to open the Control Center", "How long Guide is held to open the Control Center; a shorter press goes to the PC. Choose Off if a game or the PC needs a long Guide press.", |ui, s| {
        Combo::from_id_salt("guide-hold").selected_text(guide_hold_name(s.guide_hold_ms)).show_ui(ui, |ui| {
            for ms in crate::config::GUIDE_HOLD_MS {
                ui.selectable_value(&mut s.guide_hold_ms, ms, guide_hold_name(ms));
            }
        }).response
    });
    setting_row(ui, s, rows, &["back_opens_menu", "back_as_guide"], "Back button", "What the controller’s or handheld’s Back button does: Back reaches the PC as Back, Control Center opens the Control Center, Guide sends Guide (Home or PS) to the PC. Leave it on Back if your games use that button.", |ui, s| {
        let current = BackButton::from_saved(s.back_opens_menu, s.back_as_guide);
        let mut choice = current;
        let response = Combo::from_id_salt("back-button").selected_text(tr(back_button_name(current))).show_ui(ui, |ui| {
            for each in BackButton::ALL {
                ui.selectable_value(&mut choice, each, tr(back_button_name(each)));
            }
        }).response;
        if choice != current {
            (s.back_opens_menu, s.back_as_guide) = choice.to_saved();
        }
        response
    });
    for (hotkey, label) in [
        (Hotkey::Y, en("Y in the Control Center")),
        (Hotkey::X, en("X in the Control Center")),
        (Hotkey::Lb, en("LB in the Control Center")),
        (Hotkey::Rb, en("RB in the Control Center")),
    ] {
        let differs = s.stream_menu_buttons.get(&hotkey) != rows.defaults.stream_menu_buttons.get(&hotkey);
        rows.defer_reset();
        rows.custom_reset(en("Default"), differs);
        setting_row(ui, s, rows, &["stream_menu_buttons"], label, "What this button runs while the Control Center is open. Default runs the tile marked with this button in its corner; an action that leaves the stream, such as Disconnect, asks first.", |ui, s| stream_menu_button(ui, s, hotkey));
        if rows.take_reset() {
            match rows.defaults.stream_menu_buttons.get(&hotkey) {
                Some(v) => {
                    s.stream_menu_buttons.insert(hotkey, v.clone());
                }
                None => {
                    s.stream_menu_buttons.remove(&hotkey);
                }
            }
        }
    }
    rows.always();
    rows.group(en("USB"));
    rows.when(s.controllers);
    setting_row(ui, s, rows, &["controller_audio"], "USB DualSense audio and haptics", "A wired DualSense plays the game’s haptics (waveform vibration), and its speaker, headset and microphone work with the PC. Needs a PC that supports them.", |ui, s| {
        toggle(ui, &mut s.controller_audio)
    });
    rows.always();
    setting_row(ui, s, rows, &["usb_forwarding"], "Forward USB devices", "Lets the Control Center hand a USB device plugged in here, such as a drawing tablet or security key, to the PC. Needs a PC that offers USB forwarding.", |ui, s| {
        toggle(ui, &mut s.usb_forwarding)
    });
}
