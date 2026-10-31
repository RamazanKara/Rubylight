//! The settings page: its sections, rows and menus, which rows show folded, and the names its
//! menus use.

use super::*;

/// The settings sections in the order Android lists them (App only on the main settings).
pub(super) const SETTINGS_SECTIONS: [&str; 7] = [en("Video"), en("Display & image"), en("Controls"), en("Audio"), en("Streaming"), en("App"), en("Advanced")];

/// The Resolution menu after Native: Android's 16:9 sizes with common PC and laptop ones, then
/// 16:10, then ultrawide. Custom… takes any other size.
pub(super) const RESOLUTIONS: [(u32, u32); 15] = [
    (640, 360), (854, 480), (1280, 720), (1366, 768), (1600, 900), (1920, 1080), (2560, 1440), (3840, 2160),
    (1280, 800), (1920, 1200), (2560, 1600), (3840, 2400),
    (2560, 1080), (3440, 1440), (5120, 1440),
];

/// The Frame rate menu after Display: Android's 30-120 and common monitor rates. Custom… takes
/// any rate in `CUSTOM_FPS`, as the stream menu's rate does.
pub(super) const FRAME_RATES: [u32; 7] = [30, 60, 90, 120, 144, 165, 240];
pub(super) const CUSTOM_FPS: std::ops::RangeInclusive<u32> = 10..=500;

impl Launcher {
    pub(super) fn settings_page(&mut self, ui: &mut Ui, view: &View<'_>, commands: &mut Vec<Command>) {
        let display = self.display;
        let tearing = self.tearing_supported;
        let layered = self.screen != Screen::Settings;
        if layered {
            ui.label(match self.screen {
                Screen::ProfileSettings(_) => tr("Unchecked settings follow your main settings."),
                Screen::HostSettings(_) => tr("Unchecked settings follow the active profile and main settings."),
                _ => tr("Unchecked settings follow this PC, the active profile, and main settings."),
            });
            if text_button(ui, "Reset all to inherited").clicked() {
                self.edit_overrides = Overrides::default();
                self.edit_settings = self.inherited_settings.clone();
            }
        } else {
            let active = self.profiles.entries.iter().find(|p| Some(p.id) == self.profiles.active)
                .map_or(tr("Main settings"), |p| p.name.as_str());
            let about = trf("Active: {}. PCs and games can override individual settings.", &[&active]);
            let manage = ui.scope(|ui| {
                ui.set_max_width(SETTINGS_W);
                row(ui, "Profiles", &about, |ui| quiet_button(ui, "Manage profiles"))
            }).inner;
            self.focus_item(&manage, "profiles-link", false);
            if manage.clicked() {
                commands.push(Command::SaveSettings);
                self.open(Screen::Profiles);
                return;
            }
        }
        let mut jump: Option<&'static str> = None;
        ui.horizontal_wrapped(|ui| {
            ui.set_max_width(SETTINGS_W);
            for name in SETTINGS_SECTIONS {
                // A profile or PC override has no app-wide section.
                if layered && name == "App" {
                    continue;
                }
                if chip(ui, name).clicked() {
                    jump = Some(name);
                }
            }
        });
        ui.add_space(6.0);
        let mut links: Vec<(&str, Response)> = Vec::new();
        let test_host = if let Screen::HostSettings(id) = self.screen { view.hosts.get(id) } else { None };
        let inherited = self.inherited_settings.clone();
        let overrides = &mut self.edit_overrides;
        let mut rows = Rows { layer: layered.then_some((&inherited, overrides)), unfolded: &mut self.unfolded, section: "", folded: 0 };
        let screen = self.screen;
        let (custom_size_on, custom_fps_on) = (&mut self.custom_size, &mut self.custom_fps);
        let s = if layered { &mut self.edit_settings } else { &mut self.settings };
        let before = s.clone();
        // Coming back from a sub-page, the link that opened it takes focus instead.
        let focus_first = self.focus_screen && self.restore_key.is_none();
        if focus_first {
            self.focus_screen = false;
        }
        scroll_column(ui, "settings", SETTINGS_W, |ui| {
            ui.set_max_width(SETTINGS_W);
            rows.section(ui, "Video", &mut jump);
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
            let first = setting_row(ui, s, &mut rows, &["width", "height"], "Resolution", "The stream's size. Native matches this display.", |ui, s| {
                // Taller than egui's default menu; it scrolls where the window is shorter.
                egui::ComboBox::from_id_salt("resolution")
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
            if let Some(first) = first.filter(|_| focus_first) {
                first.request_focus();
            }
            // Read again after the menu: a row drawn at Native or Display would clamp it to its range.
            // Stepping onto a preset keeps the row.
            if s.width != 0 && (*custom_size_on == Some(screen) || !RESOLUTIONS.contains(&(s.width, s.height))) {
                let before = (s.width, s.height);
                setting_row(ui, s, &mut rows, &["width", "height"], "Custom resolution", "Width and height in pixels.", |ui, s| {
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
            setting_row(ui, s, &mut rows, &["fps"], "Frame rate", "Display matches this monitor's refresh rate.", |ui, s| {
                egui::ComboBox::from_id_salt("fps")
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
                setting_row(ui, s, &mut rows, &["fps"], "Custom frame rate", "Frames per second.", |ui, s| {
                    step_value(ui, &mut s.fps, CUSTOM_FPS, 1.0, |d| d.suffix(" fps"))
                });
                if s.fps != before { *custom_fps_on = Some(screen); }
            }
            let (w, h, fps, bitrate) = stream_mode(s, display);
            let recommended = stream_mode(&Settings { bitrate_kbps: 0, ..s.clone() }, display).3;
            let mut automatic = s.bitrate_kbps == 0;
            setting_row(ui, s, &mut rows, &["bitrate_kbps"], "Automatic bitrate",
                &trf("{} Mbps for {}x{} at {} fps.", &[&format!("{:.0}", f64::from(recommended) / 1000.0), &w, &h, &fps]),
                |ui, s| {
                    let r = toggle(ui, &mut automatic);
                    if r.changed() {
                        s.bitrate_kbps = if automatic { 0 } else { recommended };
                    }
                    r
                },
            );
            if !automatic {
                let mut mbps = bitrate as f32 / 1000.0;
                // A chosen bitrate below PyroWave's minimum stays; the row says what it costs.
                let warning = pyrowave_bitrate_warning(s, w, h, fps);
                let note = match &warning {
                    Some(warning) => warning.as_str(),
                    None if s.codec.is_pyrowave() => en("PyroWave needs a high bitrate: below its minimum the picture breaks up."),
                    None => en("Higher is sharper and needs a faster network."),
                };
                setting_row(ui, s, &mut rows, &["bitrate_kbps"], "Bitrate",
                    note,
                    |ui, s| {
                        let r = ui.add(
                            egui::Slider::new(&mut mbps, 0.5..=2000.0)
                                .logarithmic(true)
                                .suffix(" Mbps")
                                .max_decimals(1)
                                .step_by(0.5),
                        );
                        if r.changed() {
                            s.bitrate_kbps = (mbps * 1000.0) as u32;
                        }
                        r
                    },
                );
            }
            setting_row(ui, s, &mut rows, &["adaptive_bitrate"], "Device controls bitrate", "Reduce bitrate during sustained congestion and recover gradually. Requires a compatible host.", |ui, s| {
                toggle(ui, &mut s.adaptive_bitrate)
            });
            setting_row(ui, s, &mut rows, &["adaptive_quality"], "Automatic quality", "Adjust resolution and frame rate under sustained load. Requires a host that supports live stream changes.", |ui, s| {
                egui::ComboBox::from_id_salt("quality").selected_text(tr(s.adaptive_quality.label())).show_ui(ui, |ui| {
                    for mode in [crate::config::QualityMode::Off, crate::config::QualityMode::Motion, crate::config::QualityMode::Detail, crate::config::QualityMode::Battery] {
                        ui.selectable_value(&mut s.adaptive_quality, mode, tr(mode.label()));
                    }
                }).response
            });
            if s.adaptive_quality != crate::config::QualityMode::Off {
                setting_row(ui, s, &mut rows, &["quality_min_short_edge"], "Minimum resolution", "Shortest edge, in pixels. Never increases the selected stream size.", |ui, s| {
                    step_value(ui, &mut s.quality_min_short_edge, 256..=2160, 8.0, |d| d.speed(2).suffix(" px"))
                });
                setting_row(ui, s, &mut rows, &["quality_min_fps"], "Minimum frame rate", "Automatic quality stays above this rate.", |ui, s| {
                    step_value(ui, &mut s.quality_min_fps, 15..=240, 5.0, |d| d.suffix(" fps"))
                });
            }
            // The GPU decoder's probe runs in the background, on the renderer's adapter and the HDR
            // plane format, and only when it matters: PyroWave is selected or the codec menu is
            // open. PyroWave is not offered while it fails, and the reason is shown with the
            // decoder setting.
            let probe_device = self.gpu_device.map_or(std::ptr::null_mut(), |d| d.0);
            let (gpu_mode, ten_bit) = (s.pyrowave_decoder == crate::config::PyroWaveDecoder::Gpu, s.hdr);
            let gpu_probe = move |ui: &Ui| {
                if !gpu_mode { return None; }
                let probe = crate::stream::pyrowave_mode::settings_probe(probe_device, ten_bit);
                if probe.is_none() { ui.ctx().request_repaint_after(std::time::Duration::from_millis(200)); }
                probe.and_then(Result::err)
            };
            let gpu_decoder_error = if s.codec.is_pyrowave() { gpu_probe(ui) } else { None };
            setting_row(ui, s, &mut rows, &["codec"], "Codec", "Automatic picks HEVC, then H.264. PyroWave needs GPU rendering; restart after selecting it on Linux.", |ui, s| {
                egui::ComboBox::from_id_salt("codec")
                    .selected_text(match s.codec {
                        CodecPreference::Auto => tr("Automatic"),
                        CodecPreference::H264 => "H.264",
                        CodecPreference::Hevc => "HEVC",
                        CodecPreference::Av1 => "AV1",
                        CodecPreference::PyroWave => "PyroWave 4:2:0",
                        CodecPreference::PyroWave444 => "PyroWave 4:4:4",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut s.codec, CodecPreference::Auto, tr("Automatic"));
                        ui.selectable_value(&mut s.codec, CodecPreference::H264, "H.264");
                        ui.selectable_value(&mut s.codec, CodecPreference::Hevc, "HEVC");
                        ui.selectable_value(&mut s.codec, CodecPreference::Av1, "AV1");
                        let menu_error = gpu_probe(ui);
                        if menu_error.is_none() {
                            ui.selectable_value(&mut s.codec, CodecPreference::PyroWave, "PyroWave 4:2:0");
                            ui.selectable_value(&mut s.codec, CodecPreference::PyroWave444, "PyroWave 4:4:4");
                        }
                    })
                    .response
            });
            let upscaling_help = if s.upscaling == crate::config::Upscaling::Ai {
                en("Uses the GPU's AI video upscaling, for SDR and HDR. On NVIDIA with an HDR display it also turns SDR streams into HDR. Costs GPU power.")
            } else {
                en("Enhance smaller SDR streams. HDR uses bilinear. Restart after enabling on Linux.")
            };
            setting_row(ui, s, &mut rows, &["upscaling"], "Upscaling", upscaling_help, |ui, s| {
                egui::ComboBox::from_id_salt("upscaling").selected_text(tr(s.upscaling.label())).show_ui(ui, |ui| {
                    // AI enhancement is offered only where the GPU's probe finds the vendor's super
                    // resolution or RTX Video HDR. It loads the AMF runtime, so it runs once per
                    // renderer device, in the background, when the menu is first opened.
                    let probe = crate::stream::enhance::settings_probe(probe_device);
                    if probe.is_none() { ui.ctx().request_repaint_after(std::time::Duration::from_millis(200)); }
                    let ai_offered = probe.flatten().is_some();
                    for mode in [crate::config::Upscaling::Bilinear, crate::config::Upscaling::Fsr1, crate::config::Upscaling::Sgsr1, crate::config::Upscaling::Ai] {
                        if mode == crate::config::Upscaling::Ai && !ai_offered && s.upscaling != mode { continue; }
                        ui.selectable_value(&mut s.upscaling, mode, tr(mode.label()));
                    }
                }).response
            });
            if s.upscaling.is_shader() {
                setting_row(ui, s, &mut rows, &["upscale_sharpness"], "Sharpness", "Extra sharpening applied by the upscaler.", |ui, s| {
                    ui.add(egui::Slider::new(&mut s.upscale_sharpness, 0..=100).suffix("%"))
                });
            }
            setting_row(ui, s, &mut rows, &["hdr"], "HDR",
                "Streams 10-bit HDR to an HDR-enabled display. Enable HDR in the operating system first.",
                |ui, s| toggle(ui, &mut s.hdr),
            );
            setting_row(ui, s, &mut rows, &["prefer_yuv444"], "Prefer YUV 4:4:4",
                "Sharper colored text when the PC and this device support it. HDR takes priority. Restart Nightjar after changing this setting on Linux.",
                |ui, s| ui.add_enabled_ui(!s.codec.is_pyrowave() && !s.experimental_packed_444, |ui| toggle(ui, &mut s.prefer_yuv444)).inner);
            setting_row(ui, s, &mut rows, &["sdr_10bit"], "10-bit color (SDR)",
                "Streams SDR in 10-bit HEVC or AV1 to reduce banding. Needs a Main10 decoder on this device; the PC’s display must be in SDR. Restart Nightjar after changing this setting on Linux.",
                |ui, s| ui.add_enabled_ui(!s.codec.is_pyrowave() && !s.experimental_packed_444, |ui| toggle(ui, &mut s.sdr_10bit)).inner);
            setting_row(ui, s, &mut rows, &["decoder"], "Decoder", "Automatic uses hardware decoding when available. PyroWave always uses GPU wavelets.", |ui, s| {
                ui.add_enabled_ui(!s.codec.is_pyrowave(), |ui| egui::ComboBox::from_id_salt("decoder")
                    .selected_text(match s.decoder {
                        DecoderPreference::Auto => tr("Automatic"),
                        DecoderPreference::Software => tr("Software"),
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut s.decoder, DecoderPreference::Auto, tr("Automatic"));
                        ui.selectable_value(&mut s.decoder, DecoderPreference::Software, tr("Software"));
                    })
                    .response).inner
            });
            let decoder_note = match &gpu_decoder_error {
                Some(reason) => trf("CPU decodes on the CPU and reconstructs on the GPU. GPU decodes with the PyroWave SDK and never falls back to CPU. GPU decoding is unavailable here: {}", &[reason]),
                None => tr("CPU decodes on the CPU and reconstructs on the GPU. GPU decodes with the PyroWave SDK and never falls back to CPU.").to_owned(),
            };
            setting_row(ui, s, &mut rows, &["pyrowave_decoder"], "PyroWave decoder", &decoder_note, |ui, s| {
                egui::ComboBox::from_id_salt("pyrowave-decoder")
                    .selected_text(tr(s.pyrowave_decoder.label()))
                    .show_ui(ui, |ui| {
                        for mode in [crate::config::PyroWaveDecoder::Cpu, crate::config::PyroWaveDecoder::Gpu] {
                            ui.selectable_value(&mut s.pyrowave_decoder, mode, tr(mode.label()));
                        }
                    })
                    .response
            });

            rows.section(ui, "Display & image", &mut jump);
            setting_row(ui, s, &mut rows, &["video_scaling"], "Video size", "Fit shows the whole stream; Fill crops the edges; Native uses one display pixel per stream pixel.", |ui, s| {
                use crate::config::VideoScaling;
                egui::ComboBox::from_id_salt("video-scaling").selected_text(tr(s.video_scaling.label())).show_ui(ui, |ui| {
                    for mode in [VideoScaling::Fit, VideoScaling::Fill, VideoScaling::Stretch, VideoScaling::Native] {
                        ui.selectable_value(&mut s.video_scaling, mode, tr(mode.label()));
                    }
                }).response
            });
            setting_row(ui, s, &mut rows, &["align_video_top"], "Align video to top", "Moves Fit and Native video to the top of the window.", |ui, s| toggle(ui, &mut s.align_video_top));
            setting_row(ui, s, &mut rows, &["virtual_display"], "Virtual display", "Use a separate display on hosts that support it.", |ui, s| toggle(ui, &mut s.virtual_display));
            setting_row(ui, s, &mut rows, &["host_display"], "Display arrangement", "Applied only to normal streams on hosts that support display modes.", |ui, s| {
                egui::ComboBox::from_id_salt("host-display")
                    .selected_text(tr(host_display_name(s.host_display)))
                    .show_ui(ui, |ui| {
                        for mode in [HostDisplay::HostDefault, HostDisplay::Exclusive, HostDisplay::ExtendedPrimary,
                            HostDisplay::Extended, HostDisplay::ExtendedIsolated, HostDisplay::ExtendedPrimaryIsolated, HostDisplay::Physical] {
                            ui.selectable_value(&mut s.host_display, mode, tr(host_display_name(mode)));
                        }
                    }).response
            });
            setting_row(ui, s, &mut rows, &["virtual_display_scale"], "PC render resolution", "Percent of the stream resolution that the PC renders at. The size of the transmitted video does not change. To send fewer pixels and upscale on this device, lower Resolution.", |ui, s| {
                ui.add(egui::Slider::new(&mut s.virtual_display_scale, crate::config::RENDER_SCALE).step_by(5.0).suffix("%"))
            });
            // A PC's own settings offer it only if its host reads rlDisplayScale (DisplayScale).
            let dpi_gate = display_scale_gate(test_host.and_then(|h| h.info.as_ref())
                .map_or(nightjar_core::http::Availability::Unknown, |i| i.capabilities.availability(nightjar_core::http::Feature::DisplayScale)));
            if let Some(note) = dpi_gate {
                let description = match note {
                    Some(note) => format!("{} {}", tr("Text and interface size on the PC’s virtual display."), tr(note)),
                    None => tr("Text and interface size on the PC’s virtual display.").to_owned(),
                };
                ui.add_enabled_ui(note.is_none(), |ui| {
                    setting_row(ui, s, &mut rows, &["virtual_display_dpi"], "PC display scaling", &description, |ui, s| {
                        egui::ComboBox::from_id_salt("virtual-display-dpi")
                            .selected_text(display_scale_name(s.virtual_display_dpi))
                            .show_ui(ui, |ui| {
                                for percent in crate::config::DISPLAY_SCALES {
                                    ui.selectable_value(&mut s.virtual_display_dpi, percent, display_scale_name(percent));
                                }
                            }).response
                    });
                });
            }
            setting_row(ui, s, &mut rows, &["window_mode"], "Window", "Borderless covers the display without changing its mode.", |ui, s| {
                egui::ComboBox::from_id_salt("window")
                    .selected_text(match s.window_mode {
                        WindowMode::Fullscreen => tr("Fullscreen"),
                        WindowMode::Borderless => tr("Borderless"),
                        WindowMode::Windowed => tr("Windowed"),
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut s.window_mode, WindowMode::Fullscreen, tr("Fullscreen"));
                        ui.selectable_value(&mut s.window_mode, WindowMode::Borderless, tr("Borderless"));
                        ui.selectable_value(&mut s.window_mode, WindowMode::Windowed, tr("Windowed"));
                    })
                    .response
            });
            let vrr_note = if tearing || cfg!(not(windows)) {
                en("G-Sync, FreeSync or HDMI VRR is on for this display.")
            } else {
                en("This system cannot present with tearing, which VRR needs here.")
            };
            setting_row(ui, s, &mut rows, &["vrr"], "Variable refresh rate", vrr_note, |ui, s| toggle(ui, &mut s.vrr));
            let immediate_packed = cfg!(windows) && s.experimental_packed_444 && tearing;
            ui.add_enabled_ui(s.vrr && !immediate_packed, |ui| {
                setting_row(ui, s, &mut rows, &["present_at_host_times"], "Show frames at the PC's frame times",
                    if immediate_packed {
                        en("Full chroma presents frames immediately with VRR, without the extra timing delay.")
                    } else {
                        en("Each frame appears at the moment the game made it, plus a steady delay, so motion stays even.")
                    },
                    |ui, s| toggle(ui, &mut s.present_at_host_times),
                );
            });
            ui.add_enabled_ui(!s.vrr, |ui| {
                setting_row(ui, s, &mut rows, &["frame_pacing"], "Frame pacing",
                    "Without VRR: Lowest latency shows the newest frame at once. Balanced waits for the display's refresh. With an FPS limit, frames that come early wait for their turn. Smoothest video shows every frame in turn and adds up to a frame of delay.",
                    |ui, s| {
                        egui::ComboBox::from_id_salt("frame-pacing").selected_text(tr(s.frame_pacing.label())).show_ui(ui, |ui| {
                            for mode in crate::config::FramePacing::ALL {
                                ui.selectable_value(&mut s.frame_pacing, mode, tr(mode.label()));
                            }
                        }).response
                    });
            });
            ui.add_enabled_ui(!s.vrr && !s.frame_pacing.forces_vsync(), |ui| {
                setting_row(ui, s, &mut rows, &["vsync"], "V-Sync", "Without VRR: wait for the display's refresh, no tearing.", |ui, s| {
                    toggle(ui, &mut s.vsync)
                });
            });

            rows.section(ui, "Controls", &mut jump);
            if !layered {
                row(ui, "Input customization", "Controller mappings, stick dead zones and saved keyboard chords.", |ui| {
                    ui.horizontal(|ui| {
                        let controllers = quiet_button(ui, "Controllers");
                        if controllers.clicked() { commands.push(Command::ControllerSettings); }
                        let shortcuts = quiet_button(ui, "Shortcuts");
                        if shortcuts.clicked() { commands.push(Command::Shortcuts); }
                        links.push(("controllers-link", controllers));
                        links.push(("shortcuts-link", shortcuts.clone()));
                        shortcuts
                    }).inner
                });
            }
            setting_row(ui, s, &mut rows, &["mouse_mode"], "Mouse", "Captured suits games; desktop pointer suits remote work. Ctrl+Alt+Shift+M switches.", |ui, s| {
                egui::ComboBox::from_id_salt("mouse")
                    .selected_text(match s.mouse_mode {
                        MouseMode::Capture => tr("Captured"),
                        MouseMode::Absolute => tr("Desktop pointer"),
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut s.mouse_mode, MouseMode::Capture, tr("Captured"));
                        ui.selectable_value(&mut s.mouse_mode, MouseMode::Absolute, tr("Desktop pointer"));
                    })
                    .response
            });
            setting_row(ui, s, &mut rows, &["capture_system_keys"], "Send system keys",
                "Alt+Tab, the Windows key and similar go to the PC while input is captured.",
                |ui, s| toggle(ui, &mut s.capture_system_keys),
            );
            setting_row(ui, s, &mut rows, &["cursor_shapes"], "Local cursor", "Show the PC's cursor without video delay in desktop pointer mode when the host supports it.", |ui, s| {
                toggle(ui, &mut s.cursor_shapes)
            });
            setting_row(ui, s, &mut rows, &["mouse_speed"], "Mouse speed", "Sensitivity of the captured pointer.", |ui, s| ui.add(egui::Slider::new(&mut s.mouse_speed, 25..=400).suffix("%")));
            setting_row(ui, s, &mut rows, &["mouse_nav_buttons"], "Mouse navigation buttons", "Send the mouse's Back and Forward buttons to the PC.", |ui, s| toggle(ui, &mut s.mouse_nav_buttons));
            if cfg!(windows) {
                setting_row(ui, s, &mut rows, &["native_touchpad"], "Touchpad gestures on the PC", "Send the touchpad's fingers to a PC that takes them, so its gestures work there. Other PCs get the pointer as before.", |ui, s| toggle(ui, &mut s.native_touchpad));
            }
            setting_row(ui, s, &mut rows, &["right_alt_as_meta"], "Right Alt as Windows key", "Use Right Alt for Windows-key shortcuts on the PC.", |ui, s| toggle(ui, &mut s.right_alt_as_meta));
            setting_row(ui, s, &mut rows, &["controllers"], "Controllers", "Every connected controller becomes a controller on the PC.", |ui, s| {
                toggle(ui, &mut s.controllers)
            });
            ui.add_enabled_ui(s.controllers, |ui| {
                setting_row(ui, s, &mut rows, &["back_opens_menu"], "Back opens stream controls", "Use the controller's Back / Select button to open the menu.", |ui, s| toggle(ui, &mut s.back_opens_menu));
                setting_row(ui, s, &mut rows, &["gamepad_quit_combo"], "Controller quit combo", "Hold these buttons to leave the stream. Pick another if a game or your controller needs the default.", |ui, s| {
                    let style = crate::glyphs::Style::of(view.pads);
                    egui::ComboBox::from_id_salt("gamepad-quit-combo").selected_text(style.quit_combo(s.gamepad_quit_combo)).show_ui(ui, |ui| {
                        for combo in crate::config::GamepadQuitCombo::ALL {
                            ui.selectable_value(&mut s.gamepad_quit_combo, combo, style.quit_combo(combo));
                        }
                    }).response
                });
                setting_row(ui, s, &mut rows, &["controller_mouse"], "Controller mouse mode", "Hold Start for 750 ms, then release. A/B click and the D-pad scrolls.", |ui, s| toggle(ui, &mut s.controller_mouse));
                if s.controller_mouse {
                    setting_row(ui, s, &mut rows, &["analog_scrolling"], "Stick for scrolling", "The other stick moves the pointer. Off lets both sticks move it.", |ui, s| {
                        use crate::config::AnalogScrolling;
                        egui::ComboBox::from_id_salt("analog-scrolling").selected_text(match s.analog_scrolling { AnalogScrolling::Off => tr("Off"), AnalogScrolling::Left => tr("Left"), AnalogScrolling::Right => tr("Right") }).show_ui(ui, |ui| {
                            ui.selectable_value(&mut s.analog_scrolling, AnalogScrolling::Off, tr("Off"));
                            ui.selectable_value(&mut s.analog_scrolling, AnalogScrolling::Left, tr("Left"));
                            ui.selectable_value(&mut s.analog_scrolling, AnalogScrolling::Right, tr("Right"));
                        }).response
                    });
                    setting_row(ui, s, &mut rows, &["mouse_fixed_step"], "Precise controller mouse", "Hold X for fixed pixel steps. Press Y to double the step from 1 to 128 pixels.", |ui, s| toggle(ui, &mut s.mouse_fixed_step));
                }
                setting_row(ui, s, &mut rows, &["gamepad_touchpad_as_mouse"], "Controller touchpad as mouse", "Move with one finger, scroll with two. Press the pad for a left click, or with two fingers for a right click.", |ui, s| toggle(ui, &mut s.gamepad_touchpad_as_mouse));
                setting_row(ui, s, &mut rows, &["disable_rumble"], "Disable vibration", "Turn off controller rumble, trigger rumble, and waveform haptics.", |ui, s| toggle(ui, &mut s.disable_rumble));
                setting_row(ui, s, &mut rows, &["swap_face_buttons"], "Nintendo button layout", "Swap A/B and X/Y.", |ui, s| toggle(ui, &mut s.swap_face_buttons));
                setting_row(ui, s, &mut rows, &["controller_motion"], "Motion controls", "Send gyro and accelerometer data when a game asks for it.", |ui, s| {
                    toggle(ui, &mut s.controller_motion)
                });
                setting_row(ui, s, &mut rows, &["controller_audio"], "DualSense USB audio", "Use waveform haptics, speaker, headset and microphone. Choose devices in stream controls; microphone forwarding also needs Microphone enabled.", |ui, s| {
                    toggle(ui, &mut s.controller_audio)
                });
                setting_row(ui, s, &mut rows, &["background_controllers"], "Input in the background", "Keep controllers working when Nightjar is not focused.", |ui, s| {
                    toggle(ui, &mut s.background_controllers)
                });
            });
            setting_row(ui, s, &mut rows, &["clipboard_sync"], "Clipboard sync", "Text copied here goes on the PC's clipboard, and text copied on the PC comes here. The PC must allow this device to read and write its clipboard.", |ui, s| {
                toggle(ui, &mut s.clipboard_sync)
            });
            rows.section(ui, "Audio", &mut jump);
            setting_row(ui, s, &mut rows, &["audio"], "Channels", "Surround needs a host and output device that support it.", |ui, s| {
                egui::ComboBox::from_id_salt("audio")
                    .selected_text(match s.audio {
                        AudioChannels::Stereo => tr("Stereo"),
                        AudioChannels::Surround51 => tr("5.1 surround"),
                        AudioChannels::Surround71 => tr("7.1 surround"),
                        AudioChannels::Surround714 => tr("7.1.4 surround"),
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut s.audio, AudioChannels::Stereo, tr("Stereo"));
                        ui.selectable_value(&mut s.audio, AudioChannels::Surround51, tr("5.1 surround"));
                        ui.selectable_value(&mut s.audio, AudioChannels::Surround71, tr("7.1 surround"));
                        if crate::stream::offers_714() {
                            ui.selectable_value(&mut s.audio, AudioChannels::Surround714, tr("7.1.4 surround"));
                        }
                    })
                    .response
            });
            setting_row(ui, s, &mut rows, &["audio_passthrough"], "Audio passthrough", "Ask a Rubylight PC for uncompressed PCM audio instead of Opus: lossless, at 1.5 Mbps for stereo up to 6 Mbps for 7.1. Other PCs keep Opus.", |ui, s| {
                toggle(ui, &mut s.audio_passthrough)
            });
            setting_row(ui, s, &mut rows, &["play_audio_on_host"], "Play audio on the PC too", "Keep sound on the PC's speakers while streaming.", |ui, s| {
                toggle(ui, &mut s.play_audio_on_host)
            });
            setting_row(ui, s, &mut rows, &["microphone"], "Microphone", "Send this device's default microphone to a compatible host while streaming.", |ui, s| {
                toggle(ui, &mut s.microphone)
            });
            ui.add_enabled_ui(s.microphone, |ui| {
                setting_row(ui, s, &mut rows, &["mic_echo_cancel"], "Echo cancellation",
                    "Keeps the stream's sound from the speakers out of the microphone. On Windows, Automatic turns it off when the sound plays through headphones or a headset. On Linux it needs PipeWire's or PulseAudio's echo-cancel module.",
                    |ui, s| {
                        egui::ComboBox::from_id_salt("mic-echo-cancel").selected_text(tr(s.mic_echo_cancel.label())).show_ui(ui, |ui| {
                            for mode in crate::config::MicEcho::ALL {
                                ui.selectable_value(&mut s.mic_echo_cancel, mode, tr(mode.label()));
                            }
                        }).response
                    });
            });

            rows.section(ui, "Streaming", &mut jump);
            setting_row(ui, s, &mut rows, &["overlay", "overlay_mode"], "Performance overlay",
                "Compact shows one line with the stream's health, Statistics the frame rates, network, host and decode times, Debug every measurement. Ctrl+Alt+Shift+S and the stream menu change it too.", |ui, s| {
                let shown = if s.overlay { tr(s.overlay_mode.label()) } else { tr("Off") };
                egui::ComboBox::from_id_salt("overlay-mode").selected_text(shown).show_ui(ui, |ui| {
                    if ui.selectable_label(!s.overlay, tr("Off")).clicked() { s.overlay = false; }
                    for mode in crate::config::OverlayMode::ALL {
                        if ui.selectable_label(s.overlay && s.overlay_mode == mode, tr(mode.label())).clicked() {
                            (s.overlay, s.overlay_mode) = (true, mode);
                        }
                    }
                }).response
            });
            ui.add_enabled_ui(s.overlay, |ui| {
                setting_row(ui, s, &mut rows, &["overlay_position"], "Overlay position", "The corner the performance overlay sits in.", |ui, s| {
                    egui::ComboBox::from_id_salt("overlay-position").selected_text(tr(s.overlay_position.label())).show_ui(ui, |ui| {
                        for corner in crate::config::OverlayCorner::ALL {
                            ui.selectable_value(&mut s.overlay_position, corner, tr(corner.label()));
                        }
                    }).response
                });
            });
            if let Some(host) = test_host {
                let online = host.paired() && host.reach == Reach::Online;
                row(ui, "Bandwidth", "Receive 32 MiB from this PC. Uses network data and requires a supported paired host.", |ui| {
                    let enabled = online && host.bandwidth_test();
                    let button = ui.add_enabled(enabled, egui::Button::new(tr("Test speed")));
                    if button.clicked() { commands.push(Command::NetworkTest { host: host.id, calibrate: false }); }
                    button
                });
                let note = host.record.pace.as_ref().map_or_else(|| en("Find a frame pace for this network. May take several minutes and uses substantial network data; stop other streams first.").into(),
                    |p| trf("Saved {} Mbps · {}. Recalibrate after changing the network or stream bitrate.", &[&p.pace_mbps, &if p.qualified { tr("confirmed") } else { tr("best available") }]));
                row(ui, "PyroWave frame pace", &note, |ui| {
                    ui.horizontal(|ui| {
                        let enabled = online && host.info.as_ref().is_some_and(|i| i.pyrowave_burst_probe);
                        let button = ui.add_enabled(enabled, egui::Button::new(tr("Calibrate")));
                        if button.clicked() { commands.push(Command::NetworkTest { host: host.id, calibrate: true }); }
                        if host.record.pace.is_some() && text_button(ui, "Clear").clicked() { commands.push(Command::ClearPace(host.id)); }
                        button
                    }).inner
                });
            }
            setting_row(ui, s, &mut rows, &["auto_reconnect"], "Recover interrupted streams", "Try to resume the same running app up to five times within one minute.", |ui, s| {
                toggle(ui, &mut s.auto_reconnect)
            });
            setting_row(ui, s, &mut rows, &["error_correction"], "Error correction", "How much recovery data the PC adds to the video. Automatic follows this device's losses. PCs that don't take this choice use their own setting.", |ui, s| {
                egui::ComboBox::from_id_salt("error-correction")
                    .selected_text(error_correction_name(s.error_correction))
                    .show_ui(ui, |ui| {
                        let fixed = ErrorCorrection::PERCENTAGES.map(ErrorCorrection::Percent);
                        for mode in [ErrorCorrection::Host, ErrorCorrection::Auto].into_iter().chain(fixed) {
                            ui.selectable_value(&mut s.error_correction, mode, error_correction_name(mode));
                        }
                    }).response
            });
            setting_row(ui, s, &mut rows, &["network_grace_ms"], "Network interruption grace", "Keep the session alive through brief network interruptions.", |ui, s| {
                egui::ComboBox::from_id_salt("network-grace")
                    .selected_text(trf("{} seconds", &[&(s.network_grace_ms / 1000)]))
                    .show_ui(ui, |ui| {
                        for ms in [0, 3000, 5000, 10000, 15000] {
                            ui.selectable_value(&mut s.network_grace_ms, ms, trf("{} seconds", &[&(ms / 1000)]));
                        }
                    }).response
            });
            setting_row(ui, s, &mut rows, &["client_roaming"], "Client roaming", "Keep the stream running when this device changes networks. Works with Rubylight PCs that support it.", |ui, s| {
                toggle(ui, &mut s.client_roaming)
            });
            if s.client_roaming {
                setting_row(ui, s, &mut rows, &["roaming_metered"], "Continue on mobile data", "Let the stream move onto mobile data or another metered network. Streaming uses a lot of data.", |ui, s| {
                    toggle(ui, &mut s.roaming_metered)
                });
            }
            setting_row(ui, s, &mut rows, &["usb_forwarding"], "USB forwarding", "Attach USB devices plugged into this computer to the PC from the stream controls. The PC must allow USB devices for this client. On Windows a device needs the WinUSB driver.", |ui, s| {
                toggle(ui, &mut s.usb_forwarding)
            });
            setting_row(ui, s, &mut rows, &["unbatched_input"], "Instant mouse and pen input", "Send mouse and pen movements immediately.", |ui, s| {
                toggle(ui, &mut s.unbatched_input)
            });
            // The engine sets socket and thread priorities on Linux only; on Windows the switch would do nothing.
            if cfg!(target_os = "linux") {
                setting_row(ui, s, &mut rows, &["network_priority"], "Prioritize stream traffic", "Give stream traffic priority on this device.", |ui, s| {
                    toggle(ui, &mut s.network_priority)
                });
            }

            if !layered {
                rows.section(ui, "App", &mut jump);
                row(ui, "Open on Home", "Start on the games to continue and your library. Off starts on the list of PCs.", |ui| {
                    toggle(ui, &mut s.console_home)
                });
                row(ui, "Quick resume", "Clicking a PC that is running a game goes straight back into the game instead of opening its library.", |ui| {
                    toggle(ui, &mut s.quick_resume)
                });
                if rows.shows(&["input_only_any_host"]) {
                    row(ui, "Input-only on any PC", "Adds Input-only to a game’s menu on PCs whose host has no Input-only mode of its own. The PC still sends video and audio at the smallest size, under 1 Mbit/s; Nightjar doesn’t show or play them. Hosts with their own mode keep using it.", |ui| {
                        toggle(ui, &mut s.input_only_any_host)
                    });
                }
                row(ui, "PIN lock", "Ask for a PIN to open Nightjar or to start a stream.", |ui| {
                    let set = self.pin.as_ref().is_some_and(|p| p.store.has_pin());
                    let button = quiet_button(ui, if set { en("Change PIN lock") } else { en("Set up") });
                    if button.clicked() {
                        if let Some(pin) = self.pin.as_mut() { pin.open_settings(); }
                    }
                    button
                });
                row(ui, "Language", "The language of the launcher and the stream menu. System default follows the operating system.", |ui| {
                    egui::ComboBox::from_id_salt("language")
                        .selected_text(match s.language {
                            Language::System => tr("System default"),
                            Language::English => "English",
                            Language::German => "Deutsch",
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut s.language, Language::System, tr("System default"));
                            ui.selectable_value(&mut s.language, Language::English, "English");
                            ui.selectable_value(&mut s.language, Language::German, "Deutsch");
                        })
                        .response
                });
                if cfg!(windows) && rows.shows(&["check_updates"]) {
                    row(ui, "Check for updates", "Once per launch, asks GitHub whether a newer Nightjar release is out and says so here. Nothing is downloaded or installed.", |ui| {
                        toggle(ui, &mut s.check_updates)
                    });
                }
                if rows.shows(&["ui_zoom"]) {
                    row(ui, "UI zoom", "Makes everything bigger or smaller. Ctrl+= and Ctrl+- change it, Ctrl+0 resets it.", |ui| {
                        egui::ComboBox::from_id_salt("ui-zoom")
                            .selected_text(format!("{}%", crate::config::nearest_zoom(s.ui_zoom)))
                            .show_ui(ui, |ui| {
                                for step in UI_ZOOM_STEPS {
                                    ui.selectable_value(&mut s.ui_zoom, step, format!("{step}%"));
                                }
                            })
                            .response
                    });
                }
                row(ui, "Logs", "Nightjar keeps a log of the last two runs. Attach nightjar.log, and nightjar.prev.log if the problem was before a restart, to a bug report.", |ui| {
                    let button = quiet_button(ui, "Open logs folder");
                    if button.clicked() { commands.push(Command::OpenLogs); }
                    button
                });
                if rows.shows(&["high_contrast"]) {
                    row(ui, "High contrast", "Black surfaces, white text and thicker outlines and focus rings.", |ui| {
                        toggle(ui, &mut s.high_contrast)
                    });
                }
                if rows.shows(&["reduce_motion"]) {
                    row(ui, "Reduce motion", "Stops fades, slides and the spinning wheel. Follow system uses the Windows animation setting; other systems do not report one, so motion stays on.", |ui| {
                        egui::ComboBox::from_id_salt("reduce-motion")
                            .selected_text(match s.reduce_motion {
                                ReduceMotion::System => tr("Follow system"),
                                ReduceMotion::Reduce => tr("Reduce"),
                                ReduceMotion::Full => tr("Full motion"),
                            })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut s.reduce_motion, ReduceMotion::System, tr("Follow system"));
                                ui.selectable_value(&mut s.reduce_motion, ReduceMotion::Reduce, tr("Reduce"));
                                ui.selectable_value(&mut s.reduce_motion, ReduceMotion::Full, tr("Full motion"));
                            })
                            .response
                    });
                }
            }
            rows.section(ui, "Advanced", &mut jump);
            setting_row(ui, s, &mut rows, &["full_range"], "Full color range", "Request the full RGB range from the host.", |ui, s| toggle(ui, &mut s.full_range));
            setting_row(ui, s, &mut rows, &["client_hint_bitrate"], "Loss-based bitrate (Rust engine)", "Lower the bitrate when frames are lost and raise it again after clean seconds, from the next frame on. Works with Rubylight PCs that take mid-stream rate changes, and replaces Device controls bitrate there.", |ui, s| {
                toggle(ui, &mut s.client_hint_bitrate)
            });
            setting_row(ui, s, &mut rows, &["experimental_packed_444"], "Full chroma", "Requires the host option and HEVC or AV1. Restart Nightjar after changing this setting on Linux.",
                |ui, s| ui.add_enabled_ui(!s.codec.is_pyrowave(), |ui| toggle(ui, &mut s.experimental_packed_444)).inner);
            rows.group(ui, "Diagnostics");
            setting_row(ui, s, &mut rows, &["tell_host_wireless"], "Tell the PC when on Wi-Fi", "A Rubylight PC then paces the stream for Wi-Fi from the start. Other PCs are not told.", |ui, s| {
                toggle(ui, &mut s.tell_host_wireless)
            });
            setting_row(ui, s, &mut rows, &["tell_host_link_speed"], "Tell the PC the link speed", "Lets the PC limit its sending rate to this device's wired port speed.", |ui, s| {
                toggle(ui, &mut s.tell_host_link_speed)
            });
            setting_row(ui, s, &mut rows, &["link_probe"], "Link test before the first bitrate", "A new stream at the automatic bitrate tests the link to a Rubylight PC first and starts no higher than it holds.", |ui, s| {
                toggle(ui, &mut s.link_probe)
            });
            rows.finish(ui);
            ui.add_space(18.0);
            ui.label(
                RichText::new(trf(
                    "While streaming: Ctrl+Alt+Shift+Q leaves, +Z releases input, +X toggles fullscreen, +D minimizes. \
                     On a controller, {} leaves.",
                    &[&crate::glyphs::Style::of(view.pads).quit_combo(s.gamepad_quit_combo)],
                ))
                .color(OUTLINE)
                .font(regular(text::CAPTION)),
            );
            ui.add_space(24.0);
        });
        if *s != before {
            self.settings_dirty = true;
        }
        for (key, response) in links {
            self.focus_item(&response, key, false);
        }
    }
}

/// How a PC's own settings offer PC display scaling, as Android's per-PC "dpi" row: None hides
/// it on a host that can never read `rlDisplayScale`; Some(note) draws it, turned off with the
/// note on a host that could (an older Rubylight, a virtual display that is off or not ready).
/// The main settings and a PC not reached yet offer it; the launch sends it only to hosts with
/// `Feature::DisplayScale`.
pub(super) fn display_scale_gate(availability: nightjar_core::http::Availability) -> Option<Option<&'static str>> {
    use nightjar_core::http::{Availability, Reason};
    match availability {
        Availability::Supported | Availability::Unknown => Some(None),
        Availability::Unsupported(Some(Reason::UpdateHost)) => Some(Some(en("Update the host software to use this."))),
        Availability::Unsupported(Some(Reason::VirtualDisplayOff)) => Some(Some(en("Turn on the virtual display in the host’s settings."))),
        Availability::Unsupported(Some(Reason::VirtualDisplayDriver)) => Some(Some(en("The host’s virtual display driver isn’t ready."))),
        Availability::Unsupported(_) => None,
    }
}

pub(super) fn display_scale_name(percent: u32) -> String {
    if percent == 0 { tr("PC’s own").into() } else { format!("{percent}%") }
}

pub(super) fn host_display_name(mode: HostDisplay) -> &'static str {
    match mode {
        HostDisplay::HostDefault => en("Host default"),
        HostDisplay::Exclusive => en("Virtual display only"),
        HostDisplay::ExtendedPrimary => en("Extend as primary"),
        HostDisplay::Extended => en("Extend"),
        HostDisplay::ExtendedIsolated => en("Extend, isolated"),
        HostDisplay::ExtendedPrimaryIsolated => en("Extend as primary, isolated"),
        HostDisplay::Physical => en("Physical display"),
    }
}

pub(super) fn error_correction_name(mode: ErrorCorrection) -> String {
    match mode {
        ErrorCorrection::Host => tr("PC's setting").to_owned(),
        ErrorCorrection::Auto => tr("Automatic").to_owned(),
        ErrorCorrection::Percent(percentage) => trf("{} %", &[&percentage]),
    }
}

/// The main rows of each section, as `client/app/src/main/res/values/settings_tiers.xml` lists them
/// by Android preference key, in `SETTINGS_SECTIONS` order. A test keeps the two equal.
pub(super) const MAIN_KEYS: [(&str, &[&str]); 7] = [
    ("video", &["list_resolution", "list_fps", "seekbar_bitrate_kbps", "checkbox_auto_bitrate", "list_mobile_bitrate_kbps", "video_format", "checkbox_enable_hdr"]),
    ("display", &["checkbox_stretch_video", "stream_orientation"]),
    ("input", &["checkbox_multi_controller", "controller_button_mapping", "controller_test", "checkbox_flip_face_buttons", "checkbox_touchscreen_trackpad", "checkbox_show_onscreen_controls", "checkbox_keyboard_follows_text_fields", "checkbox_clipboard_sync"]),
    ("audio", &["list_audio_config", "checkbox_host_audio", "checkbox_enable_mic", "checkbox_mic_button"]),
    ("streaming", &["checkbox_enable_perf_overlay", "checkbox_auto_reconnect"]),
    ("app", &["checkbox_always_dark", "list_languages", "checkbox_console_home", "checkbox_quick_resume", "pin_lock", "help", "report_problem", "check_updates_now"]),
    ("advanced", &["checkbox_rust_engine", "low_latency_decoding", "low_latency_customize", "gpu_driver", "checkbox_gpu_max_clocks", "checkbox_texture_view", "checkbox_vulkan_render", "checkbox_full_range"]),
];

/// Desktop rows and the Android preference each one matches. A row is main when its match is.
pub(super) const ANDROID_KEYS: [(&str, &str); 35] = [
    ("width", "list_resolution"), ("fps", "list_fps"), ("bitrate_kbps", "seekbar_bitrate_kbps"),
    ("adaptive_bitrate", "checkbox_auto_bitrate"), ("codec", "video_format"), ("hdr", "checkbox_enable_hdr"),
    ("video_scaling", "checkbox_stretch_video"), ("controllers", "checkbox_multi_controller"),
    ("swap_face_buttons", "checkbox_flip_face_buttons"), ("clipboard_sync", "checkbox_clipboard_sync"),
    ("audio", "list_audio_config"), ("play_audio_on_host", "checkbox_host_audio"), ("microphone", "checkbox_enable_mic"),
    ("overlay", "checkbox_enable_perf_overlay"), ("auto_reconnect", "checkbox_auto_reconnect"),
    ("console_home", "checkbox_console_home"), ("quick_resume", "checkbox_quick_resume"), ("language", "list_languages"),
    ("full_range", "checkbox_full_range"),
    ("client_hint_bitrate", "checkbox_rate_change_bitrate"), ("experimental_packed_444", "experimental_packed_444"),
    ("tell_host_wireless", "checkbox_tell_host_wireless"), ("tell_host_link_speed", "checkbox_tell_host_link_speed"),
    ("link_probe", "checkbox_link_probe"), ("client_roaming", "checkbox_client_roaming"),
    ("audio_passthrough", "checkbox_audio_passthrough"), ("usb_forwarding", "checkbox_usb_forwarding"),
    ("unbatched_input", "checkbox_unbatched_input"), ("network_priority", "checkbox_network_priority"),
    ("input_only_any_host", "checkbox_input_only_any_host"),
    ("gamepad_quit_combo", "list_gamepad_quit_combo"), ("check_updates", "checkbox_check_updates"),
    ("frame_pacing", "frame_pacing"), ("mic_echo_cancel", "list_mic_echo_cancel"),
    ("pin_lock", "pin_lock"),
];

/// Main rows Android has no match for.
pub(super) const DESKTOP_MAIN: [&str; 2] = ["window_mode", "mouse_mode"];

pub(super) fn main_row(key: &str) -> bool {
    DESKTOP_MAIN.contains(&key)
        || ANDROID_KEYS.iter().any(|&(desktop, android)| desktop == key && MAIN_KEYS.iter().any(|(_, main)| main.contains(&android)))
}

/// The settings page's rows as it draws them: each section shows its main rows and folds the rest
/// behind a "More … settings" button. A folded row a PC or profile overrides stays in view.
pub(super) struct Rows<'a> {
    layer: Option<(&'a Settings, &'a mut Overrides)>,
    unfolded: &'a mut std::collections::BTreeSet<&'static str>,
    section: &'static str,
    /// The section's folded rows so far, drawn or not.
    folded: usize,
}

impl Rows<'_> {
    fn open(&self) -> bool {
        self.unfolded.contains(self.section)
    }

    /// Whether the row for `keys` is drawn.
    fn shows(&mut self, keys: &[&str]) -> bool {
        if keys.first().is_some_and(|k| main_row(k)) {
            return true;
        }
        self.folded += 1;
        self.open() || self.layer.as_ref().is_some_and(|(_, overrides)| overrides.has(keys))
    }

    /// Ends the section before, then starts `title`.
    fn section(&mut self, ui: &mut Ui, title: &'static str, jump: &mut Option<&'static str>) {
        self.finish(ui);
        section_at(ui, title, jump);
        (self.section, self.folded) = (title, 0);
    }

    /// A group heading inside the fold, such as Advanced's Diagnostics.
    fn group(&self, ui: &mut Ui, title: &str) {
        if self.open() {
            ui.add_space(6.0);
            ui.label(RichText::new(tr(title)).font(semibold(text::LABEL)).color(ON_SURFACE_VARIANT));
            ui.add_space(4.0);
        }
    }

    /// The section's fold button, when it has folded rows.
    fn finish(&mut self, ui: &mut Ui) {
        if self.folded == 0 {
            return;
        }
        let open = self.open();
        let label = if open { trf("Fewer {} settings", &[&tr(self.section)]) } else { trf("More {} settings ({})", &[&tr(self.section), &self.folded]) };
        let button = fold_row(ui, &label, open);
        if button.clicked() && !self.unfolded.remove(self.section) {
            self.unfolded.insert(self.section);
        }
        ui.add_space(6.0);
        self.folded = 0;
    }
}

/// A section's "More … settings" switch: a row like the settings it folds, its chevron pointing
/// down while they are hidden and up while they show.
pub(super) fn fold_row(ui: &mut Ui, label: &str, open: bool) -> Response {
    let width = ui.available_width().min(SETTINGS_W);
    let (rect, response) = ui.allocate_exact_size(vec2(width, CONTROL_HEIGHT + 4.0), Sense::click());
    response.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, open, label));
    let painter = ui.painter_at(rect.expand(6.0));
    let lit = response.hovered() || response.has_focus();
    painter.rect_filled(rect, CornerRadius::same(radius::CONTROL), if lit { SURFACE_HIGH } else { SURFACE });
    let galley = painter.layout_no_wrap(label.to_owned(), semibold(text::BODY), *ON_SURFACE);
    painter.galley(Pos2::new(rect.left() + 18.0, rect.center().y - galley.size().y / 2.0), galley, *ON_SURFACE);
    let c = Pos2::new(rect.right() - 18.0 - 6.0, rect.center().y);
    let (tip, wing) = if open { (-3.0, 3.0) } else { (3.0, -3.0) };
    let stroke = Stroke::new(2.0, *ON_SURFACE_VARIANT);
    painter.line_segment([c + vec2(-6.0, wing), c + vec2(0.0, tip)], stroke);
    painter.line_segment([c + vec2(0.0, tip), c + vec2(6.0, wing)], stroke);
    focus_ring(ui, rect, &response, radius::CONTROL);
    ui.add_space(2.0);
    response
}

/// A setting's row, or None while its section folds it away.
pub(super) fn setting_row(
    ui: &mut Ui,
    settings: &mut Settings,
    rows: &mut Rows<'_>,
    keys: &[&str],
    label: &str,
    description: &str,
    control: impl FnOnce(&mut Ui, &mut Settings) -> Response,
) -> Option<Response> {
    if !rows.shows(keys) {
        return None;
    }
    let Some((inherited, overrides)) = rows.layer.as_mut() else {
        return Some(row(ui, label, description, |ui| control(ui, settings)));
    };
    let inherited = *inherited;
    Some(row(ui, label, description, |ui| {
        ui.with_layout(Layout::top_down(Align::Max), |ui| {
            ui.set_width(240.0);
            let mut own = overrides.has(keys);
            let response = ui.checkbox(&mut own, tr("Override"));
            if response.changed() && !own {
                overrides.clear(keys);
                *settings = overrides.apply(inherited);
            }
            let value = ui.add_enabled_ui(own, |ui| control(ui, settings)).inner;
            name_for_readers(ui, &value, label, description);
            if own { overrides.capture(settings, keys); }
            response.union(value)
        }).inner
    }))
}
