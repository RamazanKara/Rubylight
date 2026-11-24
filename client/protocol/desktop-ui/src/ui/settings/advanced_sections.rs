//! Settings rows for Advanced and Debug sections.

use super::*;

pub(super) fn draw_advanced_sections(
    ui: &mut Ui,
    commands: &mut Vec<Command>,
    s: &mut Settings,
    rows: &mut Rows<'_>,
    ctx: &mut SectionsCtx<'_, '_>,
) {
    let layered = ctx.layered;
    let links = &mut *ctx.links;
    let pace_pcs = ctx.pace_pcs;
    let test_pc = &mut *ctx.test_pc;
    let game_info_keys = &mut *ctx.game_info_keys;
    let mut open_page = ctx.open_page;
    let gpu_decoder_error = ctx.gpu_decoder_error.as_ref();
    rows.section(ui, en("Advanced"));
    // Windows PCs with two GPUs, for the whole app, not a PC or a profile.
    #[cfg(windows)]
    if let Some((power_saving_name, high_performance_name)) = (crate::platform_hooks::hooks().gpu_pair)().filter(|_| !layered) {
        setting_row(ui, s, rows, &["graphics_card"], "Graphics card", "The GPU that draws and decodes the stream where this device has two. Leave it on Automatic, which is Windows’ choice; the others set Windows’ GPU preference for Nightjar.", |ui, s| {
            use crate::config::GraphicsCard;
            let name = |choice: GraphicsCard| match choice {
                GraphicsCard::Automatic => tr(choice.label()).to_owned(),
                GraphicsCard::PowerSaving => format!("{} ({})", tr(choice.label()), power_saving_name),
                GraphicsCard::HighPerformance => format!("{} ({})", tr(choice.label()), high_performance_name),
            };
            Combo::from_id_salt("graphics_card")
                .selected_text(name(s.graphics_card))
                .show_ui(ui, |ui| {
                    for choice in GraphicsCard::ALL {
                        ui.selectable_value(&mut s.graphics_card, choice, name(choice));
                    }
                })
                .response
        });
    }
    // Linux only (Windows draws with Direct3D 11), and for the whole app, not a PC or a profile.
    if cfg!(target_os = "linux") && !layered {
        setting_row(ui, s, rows, &["renderer"], "Renderer", "On OpenGL, streams that need the GPU renderer (HDR, upscaling, PyroWave, 4:4:4) are refused. Leave it on Automatic, which switches to the GPU renderer (Vulkan) when a stream needs it.", |ui, s| {
            Combo::from_id_salt("renderer")
                .selected_text(tr(s.renderer.label()))
                .show_ui(ui, |ui| {
                    for choice in crate::config::Renderer::ALL {
                        ui.selectable_value(&mut s.renderer, choice, tr(choice.label()));
                    }
                })
                .response
        });
    }
    // Linux only, for the whole app: a console or kiosk box draws straight to the display (`kms`).
    if cfg!(target_os = "linux") && !layered {
        setting_row(ui, s, rows, &["display_output"], "Display output", "Where Nightjar shows itself. Automatic draws straight to the display when no desktop is running. Applies at the next start.", |ui, s| {
            Combo::from_id_salt("display_output")
                .selected_text(tr(s.display_output.label()))
                .show_ui(ui, |ui| {
                    for choice in crate::config::DisplayOutput::ALL {
                        ui.selectable_value(&mut s.display_output, choice, tr(choice.label()));
                    }
                })
                .response
        });
        // Only a stream drawn straight to the display has planes (kms::renderer_for).
        if s.display_output == crate::config::DisplayOutput::Desktop {
            unused_with(rows, tr(s.display_output.label()));
        }
        setting_row(ui, s, rows, &["video_planes"], "Video planes", "Draws the stream straight to the display, on a display plane of its own, with HDR and without a copy. Leave it on; off, the renderer draws the stream instead.", |ui, s| {
            toggle(ui, &mut s.video_planes)
        });
        setting_row(ui, s, rows, &["high_performance_gpu_power"], "High-performance GPU power while streaming", "Keeps an AMD graphics card at its highest clocks for the whole stream, so it cannot slow down between frames and show a frame late. Uses more power and shortens battery life. Changes nothing when the card's clocks are already set by hand. Applies from the next stream.", |ui, s| {
            toggle(ui, &mut s.high_performance_gpu_power)
        });
    }
    rows.group(en("Compatibility"));
    setting_row(ui, s, rows, &["sops"], "Game optimization hint", "Lets the PC adjust the game’s settings for the stream. Leave it on; turn it off if the PC changes a game’s settings and you want them kept.", |ui, s| toggle(ui, &mut s.sops));
    setting_row(ui, s, rows, &["decode_queue_frames"], "Frames waiting for the decoder", "Fewer frames drop the waiting ones sooner when the decoder falls behind, so delay cannot build up, but the picture stutters more often. Leave it at 15 unless the picture lags further and further behind on a slow device.", |ui, s| {
        let current = decode_queue_choice(s.decode_queue_frames);
        Combo::from_id_salt("decode-queue-frames").selected_text(decode_queue_name(current)).show_ui(ui, |ui| {
            for frames in DECODE_QUEUE_CHOICES {
                if ui.selectable_label(current == frames, decode_queue_name(frames)).clicked() {
                    s.decode_queue_frames = frames;
                }
            }
        }).response
    });
    // The player's own keys for SteamGridDB and IGDB, for the whole app: kept apart from the
    // settings (`crate::game_info`), taken in when a field is left.
    if !layered {
        rows.group(en("Artwork and game info"));
        let mut left = false;
        let mut key_field = |ui: &mut Ui, value: &mut String| {
            let response = ui.add(egui::TextEdit::singleline(value).desired_width(280.0).char_limit(200).font(regular(text::BODY)));
            left |= response.lost_focus();
            response
        };
        rows.action("steamgriddb_key");
        setting_row(ui, s, rows, &[], "SteamGridDB API key", "Covers from SteamGridDB for games your PC has none for. Your game names go to SteamGridDB; leave it empty to keep only your PC’s covers.", |ui, _| {
            key_field(ui, &mut game_info_keys.steamgriddb)
        });
        rows.action("igdb_client_id");
        setting_row(ui, s, rows, &[], "IGDB client ID", "Genres, themes and time to beat for each game, and library filters by genre and length, from IGDB. Your game names go to IGDB; leave it empty unless you have a Twitch developer app.", |ui, _| {
            key_field(ui, &mut game_info_keys.igdb_client_id)
        });
        rows.action("igdb_client_secret");
        // Game facts need both: the secret does nothing until the client ID is set.
        rows.applies(!game_info_keys.igdb_client_id.trim().is_empty(), || trf("Not used without {}", &[&tr("IGDB client ID")]));
        rows.held = !game_info_keys.igdb_client_secret.trim().is_empty();
        setting_row(ui, s, rows, &[], "IGDB client secret", "The client secret of the same Twitch developer app. Game facts need both.", |ui, _| {
            key_field(ui, &mut game_info_keys.igdb_client_secret)
        });
        if left {
            let trimmed = nightjar_policy::game_info::job::Keys {
                steamgriddb: game_info_keys.steamgriddb.trim().to_owned(),
                igdb_client_id: game_info_keys.igdb_client_id.trim().to_owned(),
                igdb_client_secret: game_info_keys.igdb_client_secret.trim().to_owned(),
            };
            commands.push(Command::SetGameInfoKeys(trimmed));
        }
    }
    rows.section(ui, en("Debug"));
    // Android's main rows first, in its order.
    setting_row(ui, s, rows, &["error_correction"], "Error correction", "FEC: extra data the PC adds to repair lost packets, taken from the picture’s bitrate. Leave it on PC’s setting; try Automatic if the picture breaks up often.", |ui, s| {
        Combo::from_id_salt("error-correction")
            .selected_text(error_correction_name(s.error_correction))
            .show_ui(ui, |ui| {
                let fixed = ErrorCorrection::PERCENTAGES.map(ErrorCorrection::Percent);
                for mode in [ErrorCorrection::Host, ErrorCorrection::Auto].into_iter().chain(fixed) {
                    ui.selectable_value(&mut s.error_correction, mode, error_correction_name(mode));
                }
            }).response
    });
    // One switch for both stored fields, as on Android; either one on reads On.
    setting_row(ui, s, rows, &["tell_host_wireless", "tell_host_link_speed"], "Tell the PC about this device’s network", "Tells the PC when this device is on Wi-Fi and how fast its wired port is, so it paces the stream for them from the first frame. Leave it on; turn it off only to compare.", |ui, s| {
        let mut on = s.tell_host_wireless || s.tell_host_link_speed;
        let response = toggle(ui, &mut on);
        if on != (s.tell_host_wireless || s.tell_host_link_speed) {
            s.tell_host_wireless = on;
            s.tell_host_link_speed = on;
        }
        response
    });
    // The engine sets socket and thread priorities on Linux and thread priorities on Windows.
    if cfg!(any(target_os = "linux", windows)) {
        setting_row(ui, s, rows, &["network_priority"], "Prioritize stream traffic", "Gives the stream’s network traffic and threads priority on this device. Leave it on; turn it off only to compare how the stream runs under load.", |ui, s| {
            toggle(ui, &mut s.network_priority)
        });
    }
    setting_row(ui, s, rows, &["unbatched_input"], "Instant mouse and pen input", "Sends mouse and pen movement at once instead of in batches, for less input delay. A high-rate mouse then uses more CPU and network; leave it on unless that matters.", |ui, s| {
        toggle(ui, &mut s.unbatched_input)
    });
    rows.group(en("PyroWave"));
    if let Some(reason) = &gpu_decoder_error {
        rows.note(trf("GPU decoding isn’t available here: {}", &[reason]), *TERTIARY);
    }
    setting_row(ui, s, rows, &["pyrowave_decoder"], "PyroWave decoder", if cfg!(target_os = "macos") { en("Decodes PyroWave on the GPU or the CPU. On a Mac, GPU needs Apple silicon and macOS 26 or later and copies each picture back through memory before it is drawn; where it can’t start, the CPU decodes and the stream says so. Leave it on CPU unless the CPU can’t keep up with the stream.") } else { en("Decodes PyroWave on the GPU or the CPU. GPU measured lower latency; where it can’t run, PyroWave is not offered until you choose CPU.") }, |ui, s| {
        Combo::from_id_salt("pyrowave-decoder")
            .selected_text(tr(s.pyrowave_decoder.label()))
            .show_ui(ui, |ui| {
                for mode in [crate::config::PyroWaveDecoder::Cpu, crate::config::PyroWaveDecoder::Gpu] {
                    ui.selectable_value(&mut s.pyrowave_decoder, mode, tr(mode.label()));
                }
            })
            .response
    });
    // A Mac's GPU decoder decodes on the graphics queue of a device of its own.
    rows.when(s.pyrowave_decoder == crate::config::PyroWaveDecoder::Gpu && !cfg!(target_os = "macos"));
    setting_row(ui, s, rows, &["pyrowave_async_compute"], "Decode PyroWave on a compute queue", "For comparing. Lets a PyroWave decode overlap the drawing of the previous frame on GPUs that can; it showed no clear gain on a Radeon 780M laptop. Leave it off.", |ui, s| {
        toggle(ui, &mut s.pyrowave_async_compute)
    });
    rows.always();
    rows.group(en("Decoder"));
    rows.when(!s.codec.is_pyrowave());
    setting_row(ui, s, rows, &["decoder"], "Video decoding", "Automatic decodes on the GPU (hardware decoding) and falls back by itself where it must; Software always decodes on the CPU. Leave it on Automatic.", |ui, s| {
        Combo::from_id_salt("decoder")
            .selected_text(decoder_name(s.decoder))
            .show_ui(ui, |ui| {
                for decoder in [DecoderPreference::Auto, DecoderPreference::Software] {
                    ui.selectable_value(&mut s.decoder, decoder, decoder_name(decoder));
                }
            })
            .response
    });
    rows.when(s.decoder == DecoderPreference::Software);
    setting_row(ui, s, rows, &["software_slices"], "Slices for software decoding", "For comparing. The PC splits each frame into slices that the CPU decodes in parallel; in our measurements it was worse than off. Leave it off.", |ui, s| {
        toggle(ui, &mut s.software_slices)
    });
    rows.always();
    // Vulkan video is Linux's.
    if cfg!(target_os = "linux") {
        if s.decoder == DecoderPreference::Software {
            unused_with(rows, decoder_name(s.decoder));
        }
        setting_row(ui, s, rows, &["vulkan_video_first"], "Vulkan video first", "Tries Vulkan video decoding before VAAPI. Leave it off: VAAPI avoids a copy too and recovers from a lost frame without a keyframe (a full new picture).", |ui, s| {
            toggle(ui, &mut s.vulkan_video_first)
        });
        if s.decoder == DecoderPreference::Software {
            unused_with(rows, decoder_name(s.decoder));
        }
        setting_row(ui, s, rows, &["vulkan_rfi"], "Vulkan video recovery", "For comparing. With Vulkan video decoding, repairs a lost frame without waiting for a keyframe (a full new picture). Leave it off.", |ui, s| {
            toggle(ui, &mut s.vulkan_rfi)
        });
    }
    // VideoToolbox is macOS's hardware decoder.
    if cfg!(target_os = "macos") {
        if s.decoder == DecoderPreference::Software {
            unused_with(rows, decoder_name(s.decoder));
        }
        setting_row(ui, s, rows, &["videotoolbox_rfi"], "VideoToolbox recovery", "For comparing. With VideoToolbox decoding, repairs a lost frame without waiting for a keyframe (a full new picture, about four times larger). Leave it off unless you are comparing.", |ui, s| {
            toggle(ui, &mut s.videotoolbox_rfi)
        });
    }
    rows.group("");
    if s.mouse_mode != MouseMode::Absolute {
        unused_with(rows, mouse_name(s.mouse_mode));
    }
    setting_row(ui, s, rows, &["cursor_shapes"], "Local cursor", "Draws the PC’s cursor here in Desktop pointer mode, without video delay.", |ui, s| {
        toggle(ui, &mut s.cursor_shapes)
    });
    rows.group(en("Diagnostics"));
    setting_row(ui, s, rows, &["link_probe"], "Test the link before starting", "A new stream at the automatic bitrate measures the connection first and starts no higher. Leave it off: in our measurements, streams then started lower, with more loss and freezes.", |ui, s| {
        toggle(ui, &mut s.link_probe)
    });
    // Where a PC measures it, or a pace was saved; a PC's own page also has it.
    if let Some(pc) = network_test_pc(pace_pcs, *test_pc) {
        let pace = pc.record.pace.as_ref();
        if let Some(pace) = pace {
            rows.note(trf("{} Mbps · {}", &[&pace.pace_mbps, &if pace.qualified { tr("confirmed") } else { tr("best available") }]), *ON_SURFACE_VARIANT);
        }
        rows.action("pyrowave_pace");
        rows.room((if pace_pcs.len() > 1 { TEST_PC_W } else { 0.0 }) + (if pace.is_some() { 80.0 } else { 0.0 }));
        setting_row(ui, s, rows, &[], "PyroWave frame pace", "Finds and saves the fastest pace at which a PC’s PyroWave frames arrive whole, for links that lose frame ends, such as a USB Ethernet dock. Takes one to three minutes.", |ui, _| {
            let online = pc.reach == Reach::Online;
            let mut action = None;
            if online && pc.info.as_ref().is_some_and(|i| i.pyrowave_burst_probe) {
                let button = quiet_button(ui, "Calibrate");
                if button.clicked() { commands.push(Command::NetworkTest { host: pc.id, calibrate: true }); }
                action = Some(button);
            } else if !online {
                action = Some(offline(ui));
            }
            if pace.is_some() {
                let clear = text_button(ui, "Clear");
                if clear.clicked() { commands.push(Command::ClearPace(pc.id)); }
                action.get_or_insert(clear);
            }
            if pace_pcs.len() > 1 { test_pc_choice(ui, "pace-pc", pace_pcs, pc, test_pc); }
            action.unwrap_or_else(|| offline(ui))
        });
    }
    if !layered {
        setting_row(ui, s, rows, &["post_stream_decode_time"], "Show decode time after streaming", "Shows the average decode time when a stream ends: this device’s decoding only, not the end-to-end latency.", |ui, s| {
            toggle(ui, &mut s.post_stream_decode_time)
        });
        rows.action("latency_benchmark");
        setting_row(ui, s, rows, &[], "Latency benchmark", "Tests this device’s decode and display timing without a PC.", |ui, _| {
            let button = quiet_button(ui, "Open");
            if button.clicked() { open_page = Some(Screen::LatencyBenchmark); }
            links.push(("latency-benchmark-link", button.clone()));
            button
        });
        rows.action("export_latency_csv");
        setting_row(ui, s, rows, &[], "Export latency CSV", "Saves the latest latency measurements.", |ui, _| {
            let button = quiet_button(ui, "Export");
            if button.clicked() { commands.push(Command::ExportLatencyCsv); }
            button
        });
    }
    ctx.open_page = open_page;
}
