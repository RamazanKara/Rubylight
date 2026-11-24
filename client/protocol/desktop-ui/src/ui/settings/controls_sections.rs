//! Settings rows for Controls, Audio, Streaming, and App sections.

use super::*;

pub(super) fn draw_controls_sections(
    ui: &mut Ui,
    commands: &mut Vec<Command>,
    s: &mut Settings,
    rows: &mut Rows<'_>,
    ctx: &mut SectionsCtx<'_, '_>,
) {
    let screen_keyboard = ctx.screen_keyboard;
    let layered = ctx.layered;
    let links = &mut *ctx.links;
    let jumbo_offered = ctx.jumbo_offered;
    let connection_pcs = ctx.connection_pcs;
    let open_game = ctx.open_game;
    let confirm_reset = &mut *ctx.confirm_reset;
    let test_pc = &mut *ctx.test_pc;
    let mut reset_all = ctx.reset_all;
    let mut open_controllers = ctx.open_controllers;
    let mut open_help = ctx.open_help;
    let mut open_page = ctx.open_page;
    let style = ctx.style;
    rows.section(ui, en("Controls"));
    setting_row(ui, s, rows, &["mouse_mode"], "Mouse mode", "Captured keeps the pointer in the stream and sends relative movement, which games with mouse look need. Desktop pointer moves like an ordinary mouse, for desktop work.", |ui, s| {
        Combo::from_id_salt("mouse")
            .selected_text(mouse_name(s.mouse_mode))
            .show_ui(ui, |ui| {
                for mode in [MouseMode::Capture, MouseMode::Absolute] {
                    ui.selectable_value(&mut s.mouse_mode, mode, mouse_name(mode));
                }
            })
            .response
    });
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
            if button.clicked() { open_controllers = Some(false); }
            links.push(("controllers-link", button.clone()));
            button
        });
        rows.action("controller_test");
        setting_row(ui, s, rows, &[], "Controller test", "Shows the buttons, sticks and triggers a controller sends.", |ui, _| {
            let button = quiet_button(ui, "Test");
            if button.clicked() { open_controllers = Some(true); }
            links.push(("controller-test-link", button.clone()));
            button
        });
    }
    if !screen_keyboard {
        rows.note(tr("Not used: this computer has no on-screen keyboard").to_owned(), *TERTIARY);
    }
    setting_row(ui, s, rows, &["keyboard_follows_text_fields"], "Keyboard follows text fields", "Opens the on-screen keyboard when you select a text field on the PC and closes it after. Needs a PC that reports text fields; turn off to open it yourself.", |ui, s| {
        toggle(ui, &mut s.keyboard_follows_text_fields)
    });
    setting_row(ui, s, rows, &["clipboard_sync"], "Clipboard sync", "Copy text on this device and paste it on the PC, or the other way round, while streaming. Everything you copy as text is sent; the PC must offer clipboard sync.", |ui, s| {
        toggle(ui, &mut s.clipboard_sync)
    });
    rows.group(en("Controllers"));
    rows.when(s.controllers);
    setting_row(ui, s, rows, &["swap_face_buttons"], "Nintendo button layout", "Swaps A with B and X with Y. Turn on if a Nintendo-style controller’s buttons should send the letters printed on them.", |ui, s| toggle(ui, &mut s.swap_face_buttons));
    setting_row(ui, s, rows, &["gamepad_quit_combo"], "Buttons to leave the stream", "Hold these buttons together to leave the stream from the controller, without opening the stream menu. Pick a combination your games don’t use.", |ui, s| {
        Combo::from_id_salt("gamepad-quit-combo").selected_text(style.quit_combo(s.gamepad_quit_combo)).show_ui(ui, |ui| {
            for combo in crate::config::GamepadQuitCombo::ALL {
                ui.selectable_value(&mut s.gamepad_quit_combo, combo, style.quit_combo(combo));
            }
        }).response
    });
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
    setting_row(ui, s, rows, &["controller_mouse"], "Controller mouse mode", "Holding Start switches the controller between gamepad and mouse. Turn it off if that happens by accident in a game; the stream menu’s Mouse tile still switches it.", |ui, s| toggle(ui, &mut s.controller_mouse));
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
    rows.group(en("Stream menu"));
    rows.when(s.controllers);
    setting_row(ui, s, rows, &["guide_hold_ms"], "Hold Guide to open the stream menu", "How long Guide is held to open the stream menu; a shorter press goes to the PC. Choose Off if a game or the PC needs a long Guide press.", |ui, s| {
        Combo::from_id_salt("guide-hold").selected_text(guide_hold_name(s.guide_hold_ms)).show_ui(ui, |ui| {
            for ms in crate::config::GUIDE_HOLD_MS {
                ui.selectable_value(&mut s.guide_hold_ms, ms, guide_hold_name(ms));
            }
        }).response
    });
    setting_row(ui, s, rows, &["back_opens_menu", "back_as_guide"], "Back button", "What the controller’s or handheld’s Back button does: Back reaches the PC as Back, Stream menu opens the stream menu, Guide sends Guide (Home or PS) to the PC. Leave it on Back if your games use that button.", |ui, s| {
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
    setting_row(ui, s, rows, &["stream_menu_buttons"], "Y in the stream menu", "What this button runs while the stream menu is open. Default runs the tile marked with this button in its corner; an action that leaves the stream, such as Disconnect, asks first.", |ui, s| stream_menu_button(ui, s, Hotkey::Y));
    setting_row(ui, s, rows, &["stream_menu_buttons"], "X in the stream menu", "What this button runs while the stream menu is open. Default runs the tile marked with this button in its corner; an action that leaves the stream, such as Disconnect, asks first.", |ui, s| stream_menu_button(ui, s, Hotkey::X));
    setting_row(ui, s, rows, &["stream_menu_buttons"], "LB in the stream menu", "What this button runs while the stream menu is open. Default runs the tile marked with this button in its corner; an action that leaves the stream, such as Disconnect, asks first.", |ui, s| stream_menu_button(ui, s, Hotkey::Lb));
    setting_row(ui, s, rows, &["stream_menu_buttons"], "RB in the stream menu", "What this button runs while the stream menu is open. Default runs the tile marked with this button in its corner; an action that leaves the stream, such as Disconnect, asks first.", |ui, s| stream_menu_button(ui, s, Hotkey::Rb));
    rows.always();
    if !layered {
        rows.action("keyboard_shortcuts");
        setting_row(ui, s, rows, &[], "Keyboard shortcuts", "Key combinations such as Alt+F4, shown as tiles in the stream menu and sent to the PC with one press.", |ui, _| {
            let shortcuts = quiet_button(ui, "Change");
            if shortcuts.clicked() { commands.push(Command::Shortcuts); }
            links.push(("shortcuts-link", shortcuts.clone()));
            shortcuts
        });
    }
    rows.group(en("Touch & mouse"));
    if s.mouse_mode != MouseMode::Capture {
        unused_with(rows, mouse_name(s.mouse_mode));
    }
    setting_row(ui, s, rows, &["mouse_speed"], "Mouse speed", "How fast the mouse moves the PC’s pointer in the Captured mouse mode.", |ui, s| slider(ui, &mut s.mouse_speed, 25..=400, 5.0, percent, |rail| rail, |field| field.suffix("%")));
    setting_row(ui, s, rows, &["mouse_nav_buttons"], "Mouse back/forward buttons", "Sends the mouse’s Back and Forward buttons to the PC.", |ui, s| toggle(ui, &mut s.mouse_nav_buttons));
    setting_row(ui, s, rows, &["native_touchpad"], "Touchpad gestures on the PC", "A physical touchpad’s fingers go to the PC, so its own gestures work there: two-finger scroll, three- and four-finger swipes. A PC that can’t take them gets the pointer as before.", |ui, s| toggle(ui, &mut s.native_touchpad));
    rows.group(en("Keyboard"));
    if !layered {
        rows.action("stream_hotkeys");
        setting_row(ui, s, rows, &[], "Stream hotkeys", "Nightjar’s own key combinations in a stream, such as the one that opens the stream menu. Their keys never reach the PC; change one that a game or app needs.", |ui, _| {
            let hotkeys = quiet_button(ui, "Change");
            if hotkeys.clicked() { commands.push(Command::StreamHotkeys); }
            links.push(("stream-hotkeys-link", hotkeys.clone()));
            hotkeys
        });
    }
    setting_row(ui, s, rows, &["capture_system_keys", "system_keys_fullscreen_only"], "Send system keys", "Alt+Tab, the Windows key and similar go to the PC instead of this device; In full screen keeps them here while the stream is in a window. Choose Off to keep those shortcuts on this device.", |ui, s| {
        let current = SystemKeys::from_saved(s.capture_system_keys, s.system_keys_fullscreen_only);
        let mut choice = current;
        let response = Combo::from_id_salt("system-keys").selected_text(tr(system_keys_name(current))).show_ui(ui, |ui| {
            for each in SystemKeys::ALL {
                ui.selectable_value(&mut choice, each, tr(system_keys_name(each)));
            }
        }).response;
        if choice != current {
            (s.capture_system_keys, s.system_keys_fullscreen_only) = choice.to_saved();
        }
        response
    });
    setting_row(ui, s, rows, &["right_alt_as_meta"], "Right Alt as Windows key", "Right Alt sends the Windows key, which a Mac uses as Command. Leave it off if you type characters with AltGr: Right Alt then no longer works as AltGr.", |ui, s| toggle(ui, &mut s.right_alt_as_meta));
    rows.group(en("USB"));
    rows.when(s.controllers);
    setting_row(ui, s, rows, &["controller_audio"], "USB DualSense audio and haptics", "A wired DualSense plays the game’s haptics (waveform vibration), and its speaker, headset and microphone work with the PC. Needs a PC that supports them.", |ui, s| {
        toggle(ui, &mut s.controller_audio)
    });
    rows.always();
    setting_row(ui, s, rows, &["usb_forwarding"], "Forward USB devices", "Lets the stream menu hand a USB device plugged in here, such as a drawing tablet or security key, to the PC. Needs a PC that offers USB forwarding.", |ui, s| {
        toggle(ui, &mut s.usb_forwarding)
    });

    rows.section(ui, en("Audio"));
    setting_row(ui, s, rows, &["audio"], "Channels", "Stereo, or 5.1, 7.1 or 7.1.4 surround. Use Stereo for headphones or two speakers; pick surround only if your speakers and the PC are set up for it.", |ui, s| {
        Combo::from_id_salt("audio")
            .selected_text(audio_name(s.audio))
            .show_ui(ui, |ui| {
                for channels in [AudioChannels::Stereo, AudioChannels::Surround51, AudioChannels::Surround71] {
                    ui.selectable_value(&mut s.audio, channels, audio_name(channels));
                }
                if (crate::platform_hooks::hooks().offers_714)() {
                    ui.selectable_value(&mut s.audio, AudioChannels::Surround714, audio_name(AudioChannels::Surround714));
                }
            })
            .response
    });
    setting_row(ui, s, rows, &["play_audio_on_host"], "Play audio on the PC too", "Keeps sound on the PC’s speakers while streaming.", |ui, s| {
        toggle(ui, &mut s.play_audio_on_host)
    });
    setting_row(ui, s, rows, &["microphone"], "Send microphone to the PC", "Sends this device’s microphone to the PC, so you can talk in games and chat apps there. Mute it from the stream menu; needs PC support.", |ui, s| {
        toggle(ui, &mut s.microphone)
    });
    rows.when(s.microphone);
    setting_row(ui, s, rows, &["mic_echo_cancel"], "Echo cancellation", "Keeps the stream’s sound from the speakers out of the microphone. Leave it on Automatic, which turns it off with headphones; pick On if the PC still hears its own sound.", |ui, s| {
        Combo::from_id_salt("mic-echo-cancel").selected_text(tr(s.mic_echo_cancel.label())).show_ui(ui, |ui| {
            for mode in crate::config::MicEcho::ALL {
                ui.selectable_value(&mut s.mic_echo_cancel, mode, tr(mode.label()));
            }
        }).response
    });
    setting_row(ui, s, rows, &["mic_noise_suppression"], "Noise suppression", "Filters fans and other steady background noise out of the microphone and adds about 7 ms of microphone delay. Leave it on.", |ui, s| {
        toggle(ui, &mut s.mic_noise_suppression)
    });
    setting_row(ui, s, rows, &["mic_auto_gain"], "Automatic gain", "Evens out quiet and loud speech. Turn it on if others hear your voice swing between too quiet and too loud.", |ui, s| {
        toggle(ui, &mut s.mic_auto_gain)
    });
    setting_row(ui, s, rows, &["mic_gain"], "Microphone gain", "Makes the microphone quieter or louder before it is sent. Raise it if others hear you too quietly, lower it if too loudly.", |ui, s| {
        let step = f64::from(crate::config::MIC_GAIN_STEP);
        slider(ui, &mut s.mic_gain, crate::config::MIC_GAIN, step, percent, |rail| rail.step_by(step), |field| field.suffix("%"))
    });
    rows.always();
    setting_row(ui, s, rows, &["mute_audio"], "Mute audio", "Plays no stream sound on this device, for example while the PC’s speakers play it. The microphone and “Play audio on the PC too” keep working.", |ui, s| toggle(ui, &mut s.mute_audio));
    setting_row(ui, s, rows, &["audio_passthrough"], "Audio passthrough", "The PC sends uncompressed (PCM) sound, played as is: about 1.5 Mbps for stereo, up to 6 Mbps for 7.1. Leave it off on a slow network; needs PC support.", |ui, s| {
        toggle(ui, &mut s.audio_passthrough)
    });

    rows.section(ui, en("Streaming"));
    setting_row(ui, s, rows, &["overlay", "overlay_mode"], "Performance overlay", "Shows how the stream is doing. Compact is one line with FPS, network round trip and frame loss; Statistics and Debug add detail for troubleshooting.", |ui, s| {
        let shown = if s.overlay { tr(s.overlay_mode.label()) } else { tr("Off") };
        Combo::from_id_salt("overlay-mode").selected_text(shown).show_ui(ui, |ui| {
            if ui.selectable_label(!s.overlay, tr("Off")).clicked() { s.overlay = false; }
            for mode in crate::config::OverlayMode::ALL {
                if ui.selectable_label(s.overlay && s.overlay_mode == mode, tr(mode.label())).clicked() {
                    (s.overlay, s.overlay_mode) = (true, mode);
                }
            }
        }).response
    });
    // Never left out: a PC's, game's or profile's settings can show the overlay for theirs.
    rows.unused(s.overlay, || trf("Not used without {}", &[&tr("Performance overlay")]));
    setting_row(ui, s, rows, &["overlay_position"], "Overlay position", "The corner the performance overlay sits in.", |ui, s| {
        Combo::from_id_salt("overlay-position").selected_text(tr(s.overlay_position.label())).show_ui(ui, |ui| {
            for corner in crate::config::OverlayCorner::ALL {
                ui.selectable_value(&mut s.overlay_position, corner, tr(corner.label()));
            }
        }).response
    });
    rows.always();
    setting_row(ui, s, rows, &["auto_reconnect"], "Recover interrupted streams", "Reconnects to the same game after a dropped connection, up to five times a minute; it never starts or replaces a game. Leave it on.", |ui, s| {
        toggle(ui, &mut s.auto_reconnect)
    });
    // A PC's own page tests that PC; the main settings test the paired one chosen. Every
    // paired PC has ping, jitter and loss; the speed where it has a test (`network_test`).
    if let Some(pc) = network_test_pc(connection_pcs, *test_pc) {
        rows.action("connection_test");
        rows.room(if connection_pcs.len() > 1 { TEST_PC_W } else { 0.0 });
        setting_row(ui, s, rows, &[], "Connection test", "Measures ping, jitter and loss, and download speed where the PC supports it.", |ui, _| {
            let action = if pc.reach == Reach::Online {
                let button = quiet_button(ui, "Test");
                if button.clicked() { commands.push(Command::NetworkTest { host: pc.id, calibrate: false }); }
                button
            } else {
                offline(ui)
            };
            if connection_pcs.len() > 1 { test_pc_choice(ui, "speed-test-pc", connection_pcs, pc, test_pc); }
            action
        });
    }
    rows.group(en("Connection"));
    setting_row(ui, s, rows, &["client_roaming", "roaming_metered"], "Keep streaming when the network changes", "The stream moves to the new address when this device switches networks, instead of waiting for the old one; Any network also moves it onto mobile data or another metered network, which uses a lot of data. Leave it on Except mobile data; needs PC support.", |ui, s| {
        let current = Roaming::from_saved(s.client_roaming, s.roaming_metered);
        let mut choice = current;
        let response = Combo::from_id_salt("roaming").selected_text(tr(roaming_name(current))).show_ui(ui, |ui| {
            for each in Roaming::ALL {
                ui.selectable_value(&mut choice, each, tr(roaming_name(each)));
            }
        }).response;
        if choice != current {
            (s.client_roaming, s.roaming_metered) = choice.to_saved();
        }
        response
    });
    setting_row(ui, s, rows, &["network_grace_ms"], "Wait through network dropouts", "How long a silent network is waited out before the stream ends; the picture freezes meanwhile. Raise it if short dropouts end your streams; the PC must wait as long too.", |ui, s| {
        use nightjar_policy::recovery::{network_grace_choice_s, NETWORK_GRACE_CHOICES_S};
        let current = network_grace_choice_s(s.network_grace_ms);
        // Off keeps a value that is already off; another choice is off at the default.
        let off_ms = if current == 0 { s.network_grace_ms } else { Settings::default().network_grace_ms };
        Combo::from_id_salt("network-grace").selected_text(network_grace_name(current)).show_ui(ui, |ui| {
            for seconds in NETWORK_GRACE_CHOICES_S {
                if ui.selectable_label(current == seconds, network_grace_name(seconds)).clicked() {
                    s.network_grace_ms = if seconds == 0 { off_ms } else { seconds * 1000 };
                }
            }
        }).response
    });
    // Receive coalescing on Windows, UDP_GRO on Linux; macOS's engine does neither.
    if cfg!(any(target_os = "linux", windows)) {
        setting_row(ui, s, rows, &["receive_coalescing"], "Group incoming video packets", "Takes several video packets from the network at once, which saves work at high bitrates. Leave it on; turn it off if the picture turns blocky or black.", |ui, s| {
            toggle(ui, &mut s.receive_coalescing)
        });
    }
    // Only a PC that offers more than stock packets can use jumbo frames; on the main
    // settings, one saved PC that does is enough to show the row.
    rows.when(jumbo_offered);
    setting_row(ui, s, rows, &["jumbo_frames"], "Jumbo video packets", "Sends video in fewer, larger packets when the PC and every link carry 9000-byte jumbo frames. Each stream starts a few seconds later while Nightjar tests the network, and uses normal packets if the test fails. Leave it off unless your wired network is set up for jumbo frames.", |ui, s| {
        toggle(ui, &mut s.jumbo_frames)
    });
    rows.always();
    setting_row(ui, s, rows, &["internet_play"], "Play over the internet", "Away from home, reach a PC that offers it over a direct internet path, without forwarding ports. Connect once on the PC's network first. Your network is always tried first.", |ui, s| {
        toggle(ui, &mut s.internet_play)
    });
    setting_row(ui, s, rows, &["hide_connection_warnings"], "Hide connection warnings", "Hides the warnings about a poor connection during streams. Turn it on once your connection is set up and you no longer need them.", |ui, s| {
        toggle(ui, &mut s.hide_connection_warnings)
    });
    rows.group("");
    setting_row(ui, s, rows, &["dual_screen"], "Dual screen mode", "Streams the same game to a second screen too, in a stream of its own; sound and controllers stay with the first. Needs a second screen and PC support.", |ui, s| {
        toggle(ui, &mut s.dual_screen)
    });
    // Never left out: a PC's, game's or profile's settings can turn Dual screen mode on for theirs.
    rows.unused(s.dual_screen, || trf("Not used without {}", &[&tr("Dual screen mode")]));
    let own_size = tr("Second screen’s own size");
    setting_row(ui, s, rows, &["second_screen_width", "second_screen_height"], "Second screen resolution", "The resolution of the second screen’s stream.", |ui, s| {
        let shown = if s.second_screen_width == 0 { own_size.to_owned() } else { format!("{}x{}", s.second_screen_width, s.second_screen_height) };
        Combo::from_id_salt("second-screen-resolution").selected_text(shown).show_ui(ui, |ui| {
            let mut v = (s.second_screen_width, s.second_screen_height);
            ui.selectable_value(&mut v, (0, 0), own_size);
            for (w, h) in SECOND_SCREEN_RESOLUTIONS {
                ui.selectable_value(&mut v, (w, h), format!("{w}x{h}"));
            }
            (s.second_screen_width, s.second_screen_height) = v;
        }).response
    });
    rows.always();
    // A game's menu reads it from the main settings.
    if !layered {
        setting_row(ui, s, rows, &["input_only_any_host"], "Input-only on any PC", "Offers Input-only on PCs without that mode: this device only sends input, for a game shown on another screen. The PC still sends a small stream, under 1 Mbps.", |ui, s| {
            toggle(ui, &mut s.input_only_any_host)
        });
    }

    if !layered {
        rows.section(ui, en("App"));
        setting_row(ui, s, rows, &["theme"], "Theme", "System follows the system’s light or dark setting; Light stays light even when the system is dark; OLED black uses pure black surfaces.", |ui, s| {
            Combo::from_id_salt("theme").selected_text(tr(s.theme.label())).show_ui(ui, |ui| {
                for theme in crate::config::Theme::ALL {
                    ui.selectable_value(&mut s.theme, theme, tr(theme.label()));
                }
            }).response
        });
        setting_row(ui, s, rows, &["language"], "Language", "The language of Nightjar’s menus.", |ui, s| {
            Combo::from_id_salt("language")
                .selected_text(drawable_language_name(ui, s.language))
                .show_ui(ui, |ui| {
                    for language in Language::ALL {
                        ui.selectable_value(&mut s.language, language, drawable_language_name(ui, language));
                    }
                })
                .response
        });
        setting_row(ui, s, rows, &["open_on_start", "console_home"], "When Nightjar opens", "What Nightjar shows first: Home, your PCs, or a 3-second countdown into your last or a chosen game, which you can cancel.", |ui, s| {
            let current = start_screen(s);
            let mut choice = current;
            let name = |screen: StartScreen| start_screen_name(screen, open_game.as_deref());
            let response = Combo::from_id_salt("open-on-start").selected_text(name(current)).show_ui(ui, |ui| {
                // A chosen game is on the list once a game was chosen.
                for each in StartScreen::ALL.into_iter().filter(|each| *each != StartScreen::Chosen || open_game.is_some() || current == StartScreen::Chosen) {
                    ui.selectable_value(&mut choice, each, name(each));
                }
            }).response;
            if choice != current {
                set_start_screen(s, choice);
            }
            response
        });
        setting_row(ui, s, rows, &["quick_resume"], "Quick resume", "Selecting a PC that runs a game goes straight back into it, skipping the library. While it is on, open the library from the PC’s menu.", |ui, s| {
            toggle(ui, &mut s.quick_resume)
        });
        rows.action("pin_lock");
        setting_row(ui, s, rows, &[], "PIN lock", "Asks for a PIN to open Nightjar or to start a stream.", |ui, _| {
            let set = ctx.pin.as_ref().is_some_and(|p| p.store.has_pin());
            let button = quiet_button(ui, if set { en("Change PIN lock") } else { en("Set up") });
            if button.clicked()
                && let Some(pin) = ctx.pin.as_mut() { pin.open_settings(); }
            button
        });
        rows.action("help");
        setting_row(ui, s, rows, &[], "Help", "Setup, pairing and common fixes.", |ui, _| {
            let button = quiet_button(ui, "Open help");
            open_help = button.clicked();
            button
        });
        rows.action("report_problem");
        setting_row(ui, s, rows, &[], "Report a problem", "Saves a report with personal details removed.", |ui, _| {
            let button = quiet_button(ui, "Save report");
            if button.clicked() { commands.push(Command::ReportProblem); }
            button
        });
        if (crate::platform_hooks::hooks().self_update_checks)() {
            rows.action("check_updates_now");
            setting_row(ui, s, rows, &[], "Check for updates now", "Looks for a newer version now.", |ui, _| {
                let button = quiet_button(ui, "Check now");
                if button.clicked() { commands.push(Command::CheckUpdatesNow); }
                button
            });
            setting_row(ui, s, rows, &["check_updates"], "Check for updates automatically", "Looks for a newer version when Nightjar opens; nothing installs by itself.", |ui, s| {
                toggle(ui, &mut s.check_updates)
            });
        }
        if let Some(on) = ctx.sign_in {
            rows.action("sign_in");
            setting_row(ui, s, rows, &[], "Start Nightjar when I sign in", "Nightjar opens by itself after you sign in.", |ui, _| {
                let mut wanted = on;
                let response = toggle(ui, &mut wanted);
                if wanted != on {
                    commands.push(Command::SetSignIn(wanted));
                }
                response
            });
        }
        // Home's card size, which a PC's library and All games follow too: Small or Medium.
        setting_row(ui, s, rows, &["home_card_size"], "Use small box art", "Fits more games on screen in a PC’s library.", |ui, s| {
            let mut small = s.home_card_size == crate::config::CardSize::Small;
            let response = toggle(ui, &mut small);
            if small != (s.home_card_size == crate::config::CardSize::Small) {
                s.home_card_size = if small { crate::config::CardSize::Small } else { crate::config::CardSize::Medium };
            }
            response
        });
        setting_row(ui, s, rows, &["show_debug_settings"], "Show debug settings", "Shows the Debug section, with switches for comparing settings or ruling out a cause. Leave it off unless support asks for it or you want to compare; a Debug setting you changed shows either way.", |ui, s| {
            toggle(ui, &mut s.show_debug_settings)
        });
        rows.action("about_app");
        setting_row(ui, s, rows, &[], "About", "Version, licences and credits.", |ui, _| {
            let button = quiet_button(ui, "Open");
            if button.clicked() { open_page = Some(Screen::About); }
            links.push(("about-link", button.clone()));
            button
        });
        rows.action("logs");
        setting_row(ui, s, rows, &[], "Logs", "Opens the folder with Nightjar’s logs.", |ui, _| {
            let button = quiet_button(ui, "Open logs folder");
            if button.clicked() { commands.push(Command::OpenLogs); }
            button
        });
        let reset_label = if *confirm_reset {
            rows.note(tr("Every setting on this page goes back to its default. Your language, PCs, games, profiles, PIN lock, controller buttons, shortcuts and anything not on this page stay as they are. This can’t be undone.").to_owned(), *TERTIARY);
            en("Reset all settings?")
        } else {
            en("Reset all settings")
        };
        rows.action("reset_all");
        setting_row(ui, s, rows, &[], reset_label, "Puts every setting back to its default; PCs, games, profiles and language stay.", |ui, _| {
            if *confirm_reset {
                if primary_button(ui, "Reset all").clicked() {
                    reset_all = true;
                    *confirm_reset = false;
                }
                let cancel = quiet_button(ui, "Cancel");
                if cancel.clicked() { *confirm_reset = false; }
                cancel
            } else {
                let button = quiet_button(ui, "Reset all…");
                if button.clicked() { *confirm_reset = true; }
                button
            }
        });
        rows.group(en("Accessibility"));
        setting_row(ui, s, rows, &["ui_zoom"], "Text and button size", "Makes Nightjar’s menus bigger or smaller; the stream is not scaled. Auto follows the screen’s resolution.", |ui, s| {
            let name = |zoom: u32| if zoom == 0 { tr("Auto (follows the screen)").to_owned() } else { format!("{}%", crate::config::nearest_zoom(zoom)) };
            Combo::from_id_salt("ui-zoom")
                .selected_text(name(s.ui_zoom))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut s.ui_zoom, 0, name(0));
                    for step in UI_ZOOM_STEPS {
                        ui.selectable_value(&mut s.ui_zoom, step, name(step));
                    }
                })
                .response
        });
        setting_row(ui, s, rows, &["high_contrast"], "High contrast", "Black surfaces, stronger text and thicker focus rings.", |ui, s| {
            toggle(ui, &mut s.high_contrast)
        });
        setting_row(ui, s, rows, &["reduce_motion"], "Reduce motion", "Turns off fades, slides and zooms; Follow system uses the system’s setting.", |ui, s| {
            Combo::from_id_salt("reduce-motion")
                .selected_text(reduce_motion_name(s.reduce_motion))
                .show_ui(ui, |ui| {
                    for mode in [ReduceMotion::System, ReduceMotion::Reduce, ReduceMotion::Full] {
                        ui.selectable_value(&mut s.reduce_motion, mode, reduce_motion_name(mode));
                    }
                })
                .response
        });
    }
    ctx.reset_all = reset_all;
    ctx.open_controllers = open_controllers;
    ctx.open_help = open_help;
    ctx.open_page = open_page;
}
