//! Settings rows for Recommended, Video, and Display & image sections.

use super::*;

pub(super) fn draw_video_sections(
    ui: &mut Ui,
    view: &View<'_>,
    s: &mut Settings,
    rows: &mut Rows<'_>,
    ctx: &mut SectionsCtx<'_, '_>,
) {
    let display = ctx.display;
    let wayland = ctx.wayland;
    let open = ctx.open;
    let vrr_offered = ctx.vrr_offered;
    let mobile_shown = ctx.mobile_shown;
    let mut mobile_limit = ctx.mobile_limit;
    let custom_size_on = &mut *ctx.custom_size_on;
    let custom_fps_on = &mut *ctx.custom_fps_on;
    let probe_device = ctx.probe_device;
    let (gpu_mode, ten_bit) = (ctx.gpu_mode, ctx.ten_bit);
    let gpu_probe = move |ui: &Ui| run_gpu_probe(ui, probe_device, gpu_mode, ten_bit);
    let screen = ctx.screen;
    if open == en("Recommended") {
        rows.section(ui, en("Recommended"));
        if !ctx.hdr_probed {
            if let Some(probe) = ctx.hdr_display_probe {
                ctx.hdr_display = probe();
            }
            ctx.hdr_probed = true;
        }
        let caps = simple_caps(&display, ctx.hdr_display, vrr_offered);
        // Stream smoothness
        let (mut choices, custom) = simple_state(s, &display, &caps, vrr_offered);
        let mut smoothness = choices.smoothness;
        let smoothness_text = if custom { tr("Custom") } else { tr(simple_smoothness_name(smoothness)) };
        setting_row(ui, s, rows, &["simple_smoothness"], "Stream smoothness",
            "Auto follows the screen and decoder: the newest picture at each refresh, with variable refresh rate where the screen offers it. Smoothest holds every frame for a steady cadence with a little more delay; Lowest delay never waits. Leave it on Auto unless a game judders or feels slow.",
            |ui, s| {
                let mut picked = None;
                let response = Combo::from_id_salt("simple-smoothness").selected_text(smoothness_text).show_ui(ui, |ui| {
                    for each in simple::Smoothness::ALL {
                        if ui.selectable_value(&mut smoothness, each, tr(simple_smoothness_name(each))).clicked() {
                            picked = Some(each);
                        }
                    }
                }).response;
                if let Some(value) = picked {
                    choices.smoothness = value;
                    simple_apply(s, &display, &caps, choices);
                }
                response
            });
        // Picture quality
        let (mut choices, custom) = simple_state(s, &display, &caps, vrr_offered);
        let mut picture = choices.picture;
        let picture_text = if custom { tr("Custom") } else { tr(simple_picture_name(picture)) };
        setting_row(ui, s, rows, &["simple_picture"], "Picture quality",
            "Sets the bitrate: Best spends more network for a sharper picture, Save data uses less and lowers quality when the link struggles. Leave it on Auto unless you need the extra sharpness or the lower data use.",
            |ui, s| {
                let mut picked = None;
                let response = Combo::from_id_salt("simple-picture").selected_text(picture_text).show_ui(ui, |ui| {
                    for each in simple::Picture::ALL {
                        if ui.selectable_value(&mut picture, each, tr(simple_picture_name(each))).clicked() {
                            picked = Some(each);
                        }
                    }
                }).response;
                if let Some(value) = picked {
                    choices.picture = value;
                    simple_apply(s, &display, &caps, choices);
                }
                response
            });
        // The screen the simple layer streams at: this one, read-only.
        rows.action("simple_screen");
        setting_row(ui, s, rows, &[], "Screen",
            "This device's own resolution and refresh rate. Lower it under Video if the stream stutters.",
            |ui, _| ui.label(RichText::new(trf("{0}×{1} at {2} Hz", &[&display.width, &display.height, &(display.refresh_hz.round() as u32)])).color(*ON_SURFACE)));
        // HDR
        let (mut choices, custom) = simple_state(s, &display, &caps, vrr_offered);
        let mut hdr = choices.hdr;
        let hdr_text = if custom { tr("Custom") } else { tr(simple_hdr_name(hdr)) };
        setting_row(ui, s, rows, &["simple_hdr"], "HDR",
            "Brighter highlights and richer color on an HDR screen. Auto turns it on only where this screen, the decoder and the PC support it; leave it on Auto unless HDR looks washed out.",
            |ui, s| {
                let mut picked = None;
                let response = Combo::from_id_salt("simple-hdr").selected_text(hdr_text).show_ui(ui, |ui| {
                    for each in simple::Hdr::ALL {
                        if ui.selectable_value(&mut hdr, each, tr(simple_hdr_name(each))).clicked() {
                            picked = Some(each);
                        }
                    }
                }).response;
                if let Some(value) = picked {
                    choices.hdr = value;
                    simple_apply(s, &display, &caps, choices);
                }
                response
            });
        // Console mode: the existing Home start screen, not a second launcher.
        rows.action("simple_console");
        let mut console = start_screen(s) == StartScreen::Home;
        setting_row(ui, s, rows, &[], "Console mode",
            "Opens straight into your games, full screen, where a controller or remote can pick one. Leave it on for a TV or handheld; turn it off to open the list of PCs.",
            |ui, s| {
                let response = toggle(ui, &mut console);
                if response.changed() {
                    set_start_screen(s, if console { StartScreen::Home } else { StartScreen::Pcs });
                }
                response
            });
        // Back to recommended
        rows.action("simple_back");
        let mut back = false;
        setting_row(ui, s, rows, &[], "Back to recommended",
            "Returns the three choices above to what this device does best; the settings they stand for change with them.",
            |ui, _| {
                let response = text_button(ui, "Back to recommended");
                back = response.clicked();
                response
            });
        if back {
            let recommended = simple::recommended(&caps);
            simple_apply(s, &display, &caps, recommended);
        }
    }

    rows.section(ui, en("Video"));
    let native = trf("Native ({}x{})", &[&display.width, &display.height]);
    let custom_size = s.width != 0 && (*custom_size_on == Some(screen) || !RESOLUTIONS.contains(&(s.width, s.height)));
    let current = if s.width == 0 {
        native.clone()
    } else if custom_size {
        trf("Custom ({}x{})", &[&s.width, &s.height])
    } else {
        format!("{}x{}", s.width, s.height)
    };
    let mut chose = None;
    setting_row(ui, s, rows, &["width", "height"], "Resolution", "The picture size the PC sends. Higher is sharper and needs more network and decoding power; lower it if the stream stutters.", |ui, s| {
        // Taller than egui's default menu; it scrolls where the window is shorter.
        Combo::from_id_salt("resolution")
            .selected_text(current)
            .height(640.0)
            .show_ui(ui, |ui| {
                let mut v = (s.width, s.height);
                if ui.selectable_value(&mut v, (0, 0), native).clicked() {
                    ((s.width, s.height), chose) = (v, Some(false));
                }
                for (w, h) in RESOLUTIONS {
                    let mut v = (s.width, s.height);
                    if ui.selectable_value(&mut v, (w, h), format!("{w}x{h}")).clicked() {
                        ((s.width, s.height), chose) = (v, Some(false));
                    }
                }
                if ui.selectable_label(custom_size, tr("Custom…")).clicked() {
                    if s.width == 0 { (s.width, s.height) = (display.width, display.height); }
                    chose = Some(true);
                }
            })
            .response
    });
    if let Some(custom) = chose { *custom_size_on = custom.then_some(screen); }
    // Read again after the menu: a row drawn at Native or Display would clamp it to its range.
    // Stepping onto a preset keeps the row.
    if s.width != 0 && (*custom_size_on == Some(screen) || !RESOLUTIONS.contains(&(s.width, s.height))) {
        let before = (s.width, s.height);
        setting_row(ui, s, rows, &["width", "height"], "Custom resolution", "Width and height in pixels.", |ui, s| {
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                step_value(ui, &mut s.width, 320..=7680, 16.0, |d| d);
                ui.label("×");
                step_value(ui, &mut s.height, 240..=4320, 16.0, |d| d);
            }).response
        });
        if (s.width, s.height) != before { *custom_size_on = Some(screen); }
    }
    let display_fps = display.refresh_hz.round() as u32;
    let custom_fps = s.fps != 0 && (*custom_fps_on == Some(screen) || !FRAME_RATES.contains(&s.fps));
    let fps_text = if s.fps == 0 {
        trf("Display ({} Hz)", &[&display_fps])
    } else if custom_fps {
        trf("Custom ({} fps)", &[&s.fps])
    } else {
        trf("{} fps", &[&s.fps])
    };
    let mut chose = None;
    setting_row(ui, s, rows, &["fps"], "Frame rate", "Frames per second (FPS). Higher looks smoother and needs a faster network and more decoding power; lower it if the stream stutters.", |ui, s| {
        Combo::from_id_salt("fps")
            .selected_text(fps_text)
            .height(640.0)
            .show_ui(ui, |ui| {
                if ui.selectable_value(&mut s.fps, 0, trf("Display ({} Hz)", &[&display_fps])).clicked() { chose = Some(false); }
                for fps in FRAME_RATES {
                    if ui.selectable_value(&mut s.fps, fps, trf("{} fps", &[&fps])).clicked() { chose = Some(false); }
                }
                if ui.selectable_label(custom_fps, tr("Custom…")).clicked() {
                    if s.fps == 0 { s.fps = display_fps.clamp(*CUSTOM_FPS.start(), *CUSTOM_FPS.end()); }
                    chose = Some(true);
                }
            })
            .response
    });
    if let Some(custom) = chose { *custom_fps_on = custom.then_some(screen); }
    if s.fps != 0 && (*custom_fps_on == Some(screen) || !FRAME_RATES.contains(&s.fps)) {
        let before = s.fps;
        setting_row(ui, s, rows, &["fps"], "Custom frame rate", "Pictures per second.", |ui, s| {
            step_value(ui, &mut s.fps, CUSTOM_FPS, 1.0, |d| d.suffix(" fps"))
        });
        if s.fps != before { *custom_fps_on = Some(screen); }
    }
    let (w, h, fps, bitrate) = stream_mode(s, display);
    let recommended = stream_mode(&Settings { bitrate_kbps: 0, ..s.clone() }, display).3;
    let mut automatic = s.bitrate_kbps == 0;
    setting_row(ui, s, rows, &["bitrate_kbps"], "Automatic bitrate", "Nightjar picks the bitrate for the resolution, frame rate and codec, and follows them when they change. Turn it off to set the bitrate yourself.", |ui, s| {
        let r = toggle(ui, &mut automatic);
        if r.changed() {
            s.bitrate_kbps = if automatic { 0 } else { recommended };
        }
        r
    });
    if !automatic {
        let mut mbps = bitrate as f32 / 1000.0;
        // A chosen bitrate below PyroWave's minimum stays; the row says what it costs.
        if let Some(warning) = pyrowave_bitrate_warning(s, w, h, fps) {
            rows.note(warning, *TERTIARY);
        }
        setting_row(ui, s, rows, &["bitrate_kbps"], "Bitrate", "Video data per second. Higher is sharper and needs a faster network; lower it if the picture stutters or breaks up.", |ui, s| {
            let r = log_slider(ui, &mut mbps, 0.5..=2000.0, 0.5, mbps_text,
                |rail| rail.logarithmic(true).step_by(0.5),
                |field| field.suffix(" Mbps").max_decimals(1).speed(0.5));
            if r.changed() {
                s.bitrate_kbps = (mbps * 1000.0) as u32;
            }
            r
        });
    }
    setting_row(ui, s, rows, &["lower_bitrate"], "Lower bitrate", "Never: keep my bitrate can freeze or break up the picture on a link that cannot carry it, and works only with a PC that accepts bitrate changes; choose it when the picture turns soft on a link that can carry more. By default the PC lowers the bitrate when frames are lost that it cannot repair and raises it again afterwards; Lower it sooner lowers it earlier, with a softer picture as the cost.", |ui, s| {
        let current = s.lower_bitrate;
        Combo::from_id_salt("lower-bitrate").selected_text(tr(lower_bitrate_name(current))).show_ui(ui, |ui| {
            for each in LowerBitrate::ALL {
                ui.selectable_value(&mut s.lower_bitrate, each, tr(lower_bitrate_name(each)));
            }
        }).response
    });
    if mobile_shown {
        rows.action("mobile_bitrate");
        setting_row(ui, s, rows, &[], "Bitrate on mobile data", "The highest bitrate a stream uses on mobile data or a metered network, such as a phone hotspot: less data used, softer picture. Off uses the same bitrate on every network.", |ui, _| {
            Combo::from_id_salt("mobile-bitrate").selected_text(mobile_bitrate_name(mobile_limit)).show_ui(ui, |ui| {
                // A cap set to another value on the Profiles page stays on the list.
                let listed = MOBILE_BITRATES_KBPS.contains(&mobile_limit);
                for kbps in MOBILE_BITRATES_KBPS.into_iter().chain((!listed).then_some(mobile_limit)) {
                    ui.selectable_value(&mut mobile_limit, kbps, mobile_bitrate_name(kbps));
                }
            }).response
        });
    }
    setting_row(ui, s, rows, &["codec"], "Video codec", "How the video is compressed. Leave it on Automatic unless the picture stalls or breaks up; then try H.264, the most compatible codec.", |ui, s| {
        Combo::from_id_salt("codec")
            .selected_text(codec_name(s.codec))
            .show_ui(ui, |ui| {
                for codec in [CodecPreference::Auto, CodecPreference::H264, CodecPreference::Hevc, CodecPreference::Av1] {
                    ui.selectable_value(&mut s.codec, codec, codec_name(codec));
                }
                // A Mac decodes PyroWave on the processor where the GPU decoder cannot.
                let menu_error = if cfg!(target_os = "macos") { None } else { gpu_probe(ui) };
                if menu_error.is_none() {
                    for codec in [CodecPreference::PyroWave, CodecPreference::PyroWave444] {
                        ui.selectable_value(&mut s.codec, codec, codec_name(codec));
                    }
                }
            })
            .response
    });
    setting_row(ui, s, rows, &["hdr"], "HDR", "Brighter highlights and richer color on an HDR screen. Turn it on if this screen shows HDR and HDR is on for the PC’s display.", |ui, s| toggle(ui, &mut s.hdr));
    // PyroWave's 4:4:4 is a codec of its own.
    rows.when(!s.codec.is_pyrowave());
    setting_row(ui, s, rows, &["prefer_yuv444", "experimental_packed_444"], "Sharper colored text", "Full color detail (4:4:4), so colored text and thin lines stay crisp. Turn it on if they look blurry; it takes more decoding power and needs PC and device support.", |ui, s| {
        let mut on = sharp_color(s);
        let response = Combo::from_id_salt("sharp-color").selected_text(sharp_color_name(on)).show_ui(ui, |ui| {
            for choice in [false, true] {
                ui.selectable_value(&mut on, choice, sharp_color_name(choice));
            }
        }).response;
        if on != sharp_color(s) {
            (s.prefer_yuv444, s.experimental_packed_444) = (on, on);
        }
        response
    });
    rows.when(!s.hdr);
    setting_row(ui, s, rows, &["sdr_10bit"], "Less color banding", "Streams non-HDR video in 10-bit, so skies and gradients show fewer bands. Turn it on if you see banding with HDR off; needs HEVC or AV1 and PC support.", |ui, s| toggle(ui, &mut s.sdr_10bit));
    rows.always();
    setting_row(ui, s, rows, &["frame_warp"], "Frame Warp",
        "Shows mouse and right-stick camera turns on the next refresh by shifting the picture; the HUD stays in place, and edges are approximate while turning. Needs PC support.",
        |ui, s| toggle(ui, &mut s.frame_warp));
    setting_row(ui, s, rows, &["adaptive_quality"], "Automatic quality", "Lowers the resolution or frame rate while the network or this device can’t keep up, and raises it again after. The mode decides which goes first; needs PC support.", |ui, s| {
        Combo::from_id_salt("quality").selected_text(tr(s.adaptive_quality.label())).show_ui(ui, |ui| {
            for mode in [crate::config::QualityMode::Off, crate::config::QualityMode::Motion, crate::config::QualityMode::Detail, crate::config::QualityMode::Battery] {
                ui.selectable_value(&mut s.adaptive_quality, mode, tr(mode.label()));
            }
        }).response
    });
    // Never left out: the stream menu's Stream size turns Automatic quality on for one stream, and a
    // PC's, game's or profile's settings for theirs, with these minimums (stream::mod).
    let quality = s.adaptive_quality != crate::config::QualityMode::Off;
    let without_quality = || trf("Not used without {}", &[&tr("Automatic quality")]);
    rows.unused(quality, without_quality);
    setting_row(ui, s, rows, &["quality_min_short_edge"], "Minimum resolution", "The lowest resolution Automatic quality goes to.", |ui, s| {
        Combo::from_id_salt("quality-min-size").selected_text(short_edge_name(s.quality_min_short_edge)).show_ui(ui, |ui| {
            for edge in nightjar_policy::quality::MIN_SHORT_EDGE_CHOICES {
                ui.selectable_value(&mut s.quality_min_short_edge, edge, short_edge_name(edge));
            }
        }).response
    });
    rows.unused(quality, without_quality);
    setting_row(ui, s, rows, &["quality_min_fps"], "Minimum frame rate", "The lowest frame rate Automatic quality goes to.", |ui, s| {
        Combo::from_id_salt("quality-min-fps").selected_text(trf("{} fps", &[&s.quality_min_fps])).show_ui(ui, |ui| {
            for fps in nightjar_policy::quality::MIN_FPS_CHOICES {
                ui.selectable_value(&mut s.quality_min_fps, fps, trf("{} fps", &[&fps]));
            }
        }).response
    });
    rows.always();
    // Only a Wayland desktop's HDR description has the choice; Windows maps HDR itself.
    if cfg!(target_os = "linux") {
        rows.when(s.hdr);
        if !wayland {
            rows.note(tr("Not used outside a Wayland desktop").to_owned(), *TERTIARY);
        }
        setting_row(ui, s, rows, &["hdr_desktop_brightness"], "HDR brightness follows the desktop", "HDR streams get brighter or dimmer with the desktop’s brightness setting. Leave it off to show them at the brightness the PC sends.", |ui, s| {
            toggle(ui, &mut s.hdr_desktop_brightness)
        });
        rows.always();
    }
    // A PC's own settings and a game's on it offer its image enhancements where its host
    // reports them (NvidiaEnhancements); they override the PC's app setting for this
    // device's streams.
    let enhancements = if let Screen::HostSettings(id) | Screen::AppSettings(id) = screen { view.hosts.get(id) } else { None }
        .and_then(|h| h.info.as_ref())
        .map_or(0, |i| i.capabilities.nvidia_enhancements());
    rows.when(enhancements & rubylight_protocol::nvidia_enhance::RTX_HDR != 0);
    setting_row(ui, s, rows, &["rtx_hdr"], "RTX HDR", "Turns the PC game’s SDR picture into HDR in HDR streams.", |ui, s| {
        enhancement_choice(ui, "rtx-hdr", &mut s.rtx_hdr)
    });
    rows.when(enhancements & rubylight_protocol::nvidia_enhance::RTX_VIDEO != 0);
    setting_row(ui, s, rows, &["rtx_video"], "RTX Video", "Cleans up and sharpens the picture on the PC before it is encoded.", |ui, s| {
        enhancement_choice(ui, "rtx-video", &mut s.rtx_video)
    });
    rows.always();

    rows.section(ui, en("Display & image"));
    setting_row(ui, s, rows, &["window_mode"], "Window", "Fullscreen switches the display to the stream’s resolution and refresh rate where it can; Borderless covers the display without changing it; Windowed is an ordinary window.", |ui, s| {
        Combo::from_id_salt("window")
            .selected_text(window_name(s.window_mode))
            .show_ui(ui, |ui| {
                for mode in [WindowMode::Fullscreen, WindowMode::Borderless, WindowMode::Windowed] {
                    ui.selectable_value(&mut s.window_mode, mode, window_name(mode));
                }
            })
            .response
    });
    setting_row(ui, s, rows, &["video_scaling"], "Video size", "How the picture fits a screen of another shape: Fit shows all of it with bars, Fill crops the edges, Stretch distorts it, 1:1 pixels shows it at its own size.", |ui, s| {
        use crate::config::VideoScaling;
        Combo::from_id_salt("video-scaling").selected_text(tr(s.video_scaling.label())).show_ui(ui, |ui| {
            for mode in [VideoScaling::Fit, VideoScaling::Fill, VideoScaling::Stretch, VideoScaling::Native] {
                ui.selectable_value(&mut s.video_scaling, mode, tr(mode.label()));
            }
        }).response
    });
    // One choice for VRR's three keys (`VrrChoice`). With Sharper colored text packed on
    // Windows, VRR shows each frame at once whatever the choice: the stream leaves host
    // times out, and the choice stays for the streams without it.
    rows.when(vrr_offered);
    setting_row(ui, s, rows, &["vrr", "present_at_host_times", "vrr_tear_free"], "Variable refresh rate", "VRR: on a G-Sync, FreeSync or adaptive-refresh screen, frames are shown without waiting for the screen’s refresh. On shows each frame when the game drew it, and whole, so motion stays even; that adds about 1 to 4 ms of latency. Fastest shows each frame the moment it arrives, which can tear. Frame pacing is not used while VRR is on.", |ui, s| {
        let current = vrr_choice(s);
        let mut choice = current;
        let response = Combo::from_id_salt("vrr").selected_text(vrr_choice_name(current)).show_ui(ui, |ui| {
            // Custom is never offered: it only says a mix the file holds.
            for each in VrrChoice::OFFERED {
                ui.selectable_value(&mut choice, each, vrr_choice_name(each));
            }
        }).response;
        if choice != current {
            set_vrr_choice(s, choice);
        }
        response
    });
    // Tear-free VRR's wait for the scan line is Windows'; Variable refresh rate's On asks for it.
    if cfg!(windows) {
        rows.when(s.vrr && s.vrr_tear_free && vrr_offered);
        setting_row(ui, s, rows, &["vrr_wait"], "Whole frames under VRR", "Automatic shows each picture whole by waiting for the display’s scan line, and switches to Windows V-Sync on a display that does not refresh on present; Nightjar’s wait keeps the scan line’s wait, which can tear there, and presents with sync interval 1 when that wait would cost the next picture its turn, so the display shows it whole at the next refresh. Windows V-Sync can add up to one refresh of delay, so leave Automatic unless frames still tear.", |ui, s| {
            let before = s.vrr_wait;
            let response = Combo::from_id_salt("vrr-wait")
                .selected_text(tr(s.vrr_wait.label()))
                .show_ui(ui, |ui| {
                    for wait in crate::config::VrrWait::ALL {
                        ui.selectable_value(&mut s.vrr_wait, wait, tr(wait.label()));
                    }
                })
                .response;
            // Nightjar's wait does not take the display check's switch to Windows V-Sync,
            // so the choice forgets every display Automatic found not refreshing on
            // present (`scan_untrusted.json`).
            if before != crate::config::VrrWait::Scan && s.vrr_wait == crate::config::VrrWait::Scan {
                (crate::platform_hooks::hooks().clear_scan_sync)();
            }
            response
        });
    }
    rows.always();
    let vrr = s.vrr && vrr_offered;
    rows.when(!vrr);
    // A main row beside Variable refresh rate, as on Android. One choice over its three keys (`FramePacingChoice`, Android's own): Balanced's newest
    // frame or every frame, and the PC's frame times, which take over from the pacing.
    setting_row(ui, s, rows, &["frame_pacing", "drop_late_frames", "host_frame_times"], "Frame pacing", "Lowest latency shows frames at once; the other choices wait for the screen’s refresh for steadier motion at more delay, keeping every frame adds more, and PC’s frame times shows each frame when the game drew it, a few milliseconds later. With V-Sync on, frames are timed to your screen where the PC supports it, sent to arrive just before each refresh; leave Lowest latency unless motion judders.", |ui, s| {
        let current = frame_pacing_choice(s);
        let mut choice = current;
        let response = Combo::from_id_salt("frame-pacing").selected_text(frame_pacing_choice_name(current)).show_ui(ui, |ui| {
            for each in FramePacingChoice::ALL {
                ui.selectable_value(&mut choice, each, frame_pacing_choice_name(each));
            }
        }).response;
        if choice != current {
            set_frame_pacing_choice(s, choice);
        }
        response
    });
    rows.group(en("Refresh rate"));
    // VRR off, as Frame pacing; a pacing that waits for the refresh keeps it on, and so do
    // the PC's frame times.
    // One row for V-Sync and Fractional V-Sync: Off, On, or On with an even cadence. The even cadence
    // holds nothing with VRR, with the PC's frame times, which place each picture themselves
    // (`present::fractional_vsync_stands_aside`), or where the display says not when the last
    // picture was shown (`Streaming::held_times`), and says so there (client/AGENTS.md); the row
    // is left out then unless changed, and while V-Sync is off the even cadence is not in play.
    let cadence_in_play = s.vsync || s.frame_pacing.forces_vsync() || vrr || s.host_frame_times;
    if cadence_in_play {
        if vrr {
            unused_with(rows, tr("Variable refresh rate"));
        } else if s.host_frame_times {
            unused_with(rows, frame_pacing_choice_name(FramePacingChoice::HostTimes));
        } else if !fractional_vsync_applies(&video_driver()) {
            rows.note(tr("Not used: this desktop doesn’t report when pictures are shown").to_owned(), *TERTIARY);
        } else if let Some(reason) = fractional_vsync_display_reason(s, &display, ctx.fullscreen_modes, cfg!(windows), (crate::platform_hooks::hooks().kms_active)()) {
            rows.note(reason, *TERTIARY);
        }
    }
    setting_row(ui, s, rows, &["vsync", "fractional_vsync"], "V-Sync", &format!("{} {}", tr("Waits for the screen’s refresh, so the picture never tears; the wait adds latency. Turn it off for the lowest latency if tearing doesn’t bother you."), tr("On a screen at 2 to 4 times the stream’s frame rate (60 fps on 120 Hz), shows every frame for the same number of refreshes, so motion stays even. Each frame waits for its turn, which adds delay; turn it on if motion judders with V-Sync.")), |ui, s| {
        let current = vsync_choice(s);
        let mut choice = current;
        let response = Combo::from_id_salt("vsync").selected_text(vsync_name(current)).show_ui(ui, |ui| {
            for each in VSYNC_CHOICES {
                ui.selectable_value(&mut choice, each, vsync_name(each));
            }
        }).response;
        if choice != current {
            set_vsync_choice(s, choice);
        }
        response
    });
    rows.always();
    rows.group(en("Image"));
    setting_row(ui, s, rows, &["upscaling"], "Upscaling", &if cfg!(target_os = "macos") { format!("{} {}", tr("How a stream smaller than the screen is enlarged. FSR and SGSR sharpen edges at some GPU time and battery. HDR streams are enlarged without them."), tr("MetalFX sharpens too and adds a few milliseconds of delay.")) } else { tr("How a stream smaller than the screen is enlarged. FSR and SGSR sharpen edges at some GPU time and battery. HDR streams are enlarged without them.").to_owned() }, |ui, s| {
        Combo::from_id_salt("upscaling").selected_text(tr(s.upscaling.label())).show_ui(ui, |ui| {
            // AI enhancement is offered only where the GPU's probe finds the vendor's super
            // resolution or RTX Video HDR. It loads the AMF runtime, so it runs once per
            // renderer device, in the background, when the menu is first opened.
            let probe = (crate::platform_hooks::hooks().enhance_probe)(probe_device);
            if probe.is_none() { ui.ctx().request_repaint_after(std::time::Duration::from_millis(200)); }
            let ai_offered = probe == Some(true);
            for mode in [crate::config::Upscaling::Bilinear, crate::config::Upscaling::Fsr1, crate::config::Upscaling::Sgsr1, crate::config::Upscaling::Ai, crate::config::Upscaling::MetalFx] {
                if mode == crate::config::Upscaling::Ai && !ai_offered && s.upscaling != mode { continue; }
                if mode == crate::config::Upscaling::MetalFx && !(crate::platform_hooks::hooks().metalfx_supported)() && s.upscaling != mode { continue; }
                ui.selectable_value(&mut s.upscaling, mode, tr(mode.label()));
            }
        }).response
    });
    let upscaler = tr(s.upscaling.label());
    // Never left out: a PC's, game's or profile's upscaler that sharpens takes this sharpness.
    rows.unused(s.upscaling.is_shader(), || trf("Not used with {}", &[&upscaler]));
    setting_row(ui, s, rows, &["upscale_sharpness"], "Upscaling sharpness", "How much sharpening FSR or SGSR adds. Raise it for soft edges; lower it if edges show halos.", |ui, s| {
        slider(ui, &mut s.upscale_sharpness, 0..=100, 5.0, percent, |rail| rail, |field| field.suffix("%"))
    });
    rows.always();
    rows.when(matches!(s.video_scaling, crate::config::VideoScaling::Fit | crate::config::VideoScaling::Native));
    setting_row(ui, s, rows, &["align_video_top"], "Video position", "Where a Fit or 1:1 picture sits: center or top. Top leaves the space below the picture free.", |ui, s| {
        Combo::from_id_salt("video-position").selected_text(video_position_name(s.align_video_top)).show_ui(ui, |ui| {
            for top in [false, true] {
                ui.selectable_value(&mut s.align_video_top, top, video_position_name(top));
            }
        }).response
    });
    rows.always();
    setting_row(ui, s, rows, &["full_range"], "Full color range", "Asks for full-range video, for full shadow and highlight detail. Blacks crush or highlights clip unless the PC and the screen both use full range; leave it off unless you know both do.", |ui, s| toggle(ui, &mut s.full_range));
    rows.group(en("PC display"));
    // A PC's own settings offer the arrangements only if its host takes them (HostDisplayModes);
    // the virtual display switch alone decides elsewhere.
    let test_host = if let Screen::HostSettings(id) = screen { view.hosts.get(id) } else { None };
    let availability = |feature| test_host.and_then(|h| h.info.as_ref())
        .map_or(nightjar_policy::http::Availability::Unknown, |i| i.capabilities.availability(feature));
    let modes = display_scale_gate(availability(nightjar_policy::http::Feature::HostDisplayModes));
    // Such a PC streams as the switch says: a saved arrangement that implies otherwise says why
    // until a choice is made here.
    if !modes && s.host_display != HostDisplay::HostDefault
        && nightjar_policy::http::HostDisplay::from(s.host_display).requests_virtual_display(s.virtual_display) != s.virtual_display {
        rows.note(trf("This PC doesn’t take display arrangements: it streams as shown here, not as {}", &[&tr(host_display_name(s.host_display))]), *TERTIARY);
    }
    setting_row(ui, s, rows, &["host_display", "virtual_display"], "Display arrangement", "The PC streams from a virtual display that matches the stream, or from its own monitor. The PC’s own setting keeps the PC’s display settings, Virtual display leaves the arrangement to the PC, and the other choices place it beside the PC’s monitors or switch them off during the stream. Needs PC support.", |ui, s| {
        let current = display_choice(s, modes);
        // Taller than egui's default menu, so all eight choices show.
        Combo::from_id_salt("host-display")
            .selected_text(tr(display_choice_name(current)))
            .height(640.0)
            .show_ui(ui, |ui| {
                for choice in DISPLAY_CHOICES.into_iter().filter(|c| modes || c.0 == HostDisplay::HostDefault) {
                    if ui.selectable_label(choice == current, tr(display_choice_name(choice))).clicked() {
                        pick_display_choice(s, choice);
                    }
                }
            }).response
    });
    rows.when(display_choice(s, modes).1);
    setting_row(ui, s, rows, &["virtual_display_scale"], "PC render resolution", "The size the PC renders at, as a percent of the stream resolution; the video sent keeps its size. Above 100 the PC draws more pixels than it sends, below fewer.", |ui, s| {
        slider(ui, &mut s.virtual_display_scale, crate::config::RENDER_SCALE, 5.0, percent, |rail| rail.step_by(5.0), |field| field.suffix("%"))
    });
    rows.always();
    // A PC's own settings offer it only if its host reads rlDisplayScale (DisplayScale).
    rows.when(display_scale_gate(availability(nightjar_policy::http::Feature::DisplayScale)));
    setting_row(ui, s, rows, &["virtual_display_dpi"], "PC display scaling", "Text and interface size on the PC’s virtual display; the resolution stays. Raise it if text is too small on this screen. PC’s own keeps the PC’s setting.", |ui, s| {
        Combo::from_id_salt("virtual-display-dpi")
            .selected_text(display_scale_name(s.virtual_display_dpi))
            .show_ui(ui, |ui| {
                for percent in crate::config::DISPLAY_SCALES {
                    ui.selectable_value(&mut s.virtual_display_dpi, percent, display_scale_name(percent));
                }
            }).response
    });
    rows.always();
    ctx.mobile_limit = mobile_limit;
}
