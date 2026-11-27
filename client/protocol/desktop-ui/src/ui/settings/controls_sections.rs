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
    let mut open_section = ctx.open_section;
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
    // The controller editor: every controller setting in one place, the section after this one.
    rows.action("controller_editor");
    setting_row(ui, s, rows, &[], "Controller", "Every controller setting in one place: sticks, triggers, motion, vibration and buttons.", |ui, _| {
        let button = quiet_button(ui, "Open");
        if button.clicked() { open_section = Some(en("Controller")); }
        links.push(("controller-editor-link", button.clone()));
        button
    });
    if !screen_keyboard {
        rows.note(tr("Not used: this computer has no on-screen keyboard").to_owned(), *TERTIARY);
    }
    setting_row(ui, s, rows, &["keyboard_follows_text_fields"], "Keyboard follows text fields", "Opens the on-screen keyboard when you select a text field on the PC and closes it after. Needs a PC that reports text fields; turn off to open it yourself.", |ui, s| {
        toggle(ui, &mut s.keyboard_follows_text_fields)
    });
    setting_row(ui, s, rows, &["clipboard_sync"], "Clipboard sync", "Copy text on this device and paste it on the PC, or the other way round, while streaming. Everything you copy as text is sent; the PC must offer clipboard sync.", |ui, s| {
        toggle(ui, &mut s.clipboard_sync)
    });
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
        setting_row(ui, s, rows, &[], "Stream hotkeys", "Nightjar’s own key combinations in a stream, such as the one that opens the Control Center. Their keys never reach the PC; change one that a game or app needs.", |ui, _| {
            let hotkeys = quiet_button(ui, "Change");
            if hotkeys.clicked() { commands.push(Command::StreamHotkeys); }
            links.push(("stream-hotkeys-link", hotkeys.clone()));
            hotkeys
        });
    }
    if !layered {
        rows.action("keyboard_shortcuts");
        setting_row(ui, s, rows, &[], "Keyboard shortcuts", "Key combinations such as Alt+F4, shown as tiles in the Control Center and sent to the PC with one press.", |ui, _| {
            let shortcuts = quiet_button(ui, "Change");
            if shortcuts.clicked() { commands.push(Command::Shortcuts); }
            links.push(("shortcuts-link", shortcuts.clone()));
            shortcuts
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
    super::controller_section::draw_controller_section(ui, s, rows, layered, links, style, &mut open_controllers);

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
    setting_row(ui, s, rows, &["microphone"], "Send microphone to the PC", "Sends this device’s microphone to the PC, so you can talk in games and chat apps there. Mute it from the Control Center; needs PC support.", |ui, s| {
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
    // Never left out: a PC's, game's or profile's settings can show the overlay for theirs.
    rows.unused(s.overlay, || trf("Not used without {}", &[&tr("Performance overlay")]));
    setting_row(ui, s, rows, &["compact_overlay_device_info"], "Show battery and clock in compact overlay", "Add this device’s battery percentage and local time to the compact performance overlay.", |ui, s| {
        toggle(ui, &mut s.compact_overlay_device_info)
    });
    rows.always();
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
        setting_row(ui, s, rows, &["windowed_launcher"], "Windowed launcher", "Opens Nightjar’s menus in an ordinary window even when Window is Fullscreen or Borderless. Streams still use your Window setting. Turn it on if you move Nightjar between monitors before starting a stream.", |ui, s| {
            toggle(ui, &mut s.windowed_launcher)
        });
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
    ctx.open_section = open_section;
}
