//! Game detail page rows on the art: neural rendering, settings, facts, profiles, and switch painting.

use super::*;

/// A switch on a tint: moon when on, a paper thumb.
fn paint_switch(ui: &Ui, out: &mut Vec<Shape>, rect: Rect, id: Id, on: bool, enabled: bool) {
    let ctx = ui.ctx();
    let along = spring::animate(ctx, id, if on { 1.0 } else { 0.0 }, SWITCH_TUNE);
    let r = rect.height() / 2.0;
    let off: Color32 = if high_contrast() {
        *SURFACE
    } else {
        shade::TINT_TRACK_FILL.get()
    };
    let lit: Color32 = *MOON;
    let track = off.lerp_to_gamma(lit, along.clamp(0.0, 1.0));
    let dim = if enabled { 1.0 } else { 0.4 };
    glass::solid(out, ctx, rect, r, track.gamma_multiply(dim));
    if high_contrast() {
        glass::rim(out, ctx, rect, r, 1.5, &|_| t::HC_COLOR_RIM);
    }
    let reach = rect.width() / 2.0 - r;
    let center = rect.center() + vec2(egui::lerp(-reach..=reach, along), 0.0);
    out.push(Shape::circle_filled(
        center + vec2(0.0, 1.0),
        THUMB_R,
        Color32::from_black_alpha(50),
    ));
    out.push(Shape::circle_filled(
        center,
        THUMB_R,
        knob().gamma_multiply(dim),
    ));
}

impl Launcher {
    /// The rows on the art: stream settings, what happens on leaving, hiding, and the same game on
    /// other PCs.
    pub(super) fn game_rows(
        &self,
        view: &View<'_>,
        host: &Host,
        source: &Source<'_>,
        game: &rubylight_native_api::library::Game,
        state: &PlayState,
        sources: &[Source<'_>],
    ) -> Vec<ArtRow> {
        use nightjar_policy::launch::{self, InputOnlyBlocker};
        let classic = state.classic.as_ref();
        let kind = self.network.kind(&self.profiles.home_wifi);
        let mut settings = host
            .record
            .settings(&self.profiles.settings(&self.settings, kind));
        let own = classic.and_then(|a| host.record.app_settings(&a.uuid, a.id));
        if let Some(overrides) = own {
            settings = overrides.apply(&settings);
        }
        let (w, h, fps, kbps) = stream_mode(&settings, self.display);
        let mut summary = console::stream_summary(w, h, fps, kbps);
        if let Some(profile) = self
            .profiles
            .selected(kind)
            .and_then(|id| self.profiles.entries.iter().find(|p| p.id == id))
        {
            summary.push_str("  \u{b7}  ");
            summary.push_str(&profile.name);
        }
        for from in summary_sources(
            self.profiles.profile(kind),
            host.record.settings.as_ref(),
            own,
        ) {
            summary.push_str("  \u{b7}  ");
            summary.push_str(&from);
        }
        let mut rows = vec![ArtRow {
            key: "settings".into(),
            label: tr("Stream settings").to_owned(),
            sub: Some((summary, *ON_SURFACE_VARIANT)),
            said: None,
            wrap: false,
            enabled: classic.is_some(),
            act: classic.map_or(RowAct::Nothing, |app| RowAct::Settings(app.clone())),
        }];
        if !self.profiles.entries.is_empty() {
            let decided = self.profiles.automatic
                && match kind {
                    crate::network_types::Kind::Mobile => self.profiles.mobile,
                    crate::network_types::Kind::HomeWifi => self.profiles.home,
                    crate::network_types::Kind::Wifi => self.profiles.wifi,
                    crate::network_types::Kind::Other => None,
                }
                .is_some();
            let shown = if decided {
                self.profiles.selected(kind)
            } else {
                self.profiles.active
            };
            let name = self
                .profiles
                .entries
                .iter()
                .find(|p| Some(p.id) == shown)
                .map_or(tr("Main settings"), |p| p.name.as_str())
                .to_owned();
            rows.push(ArtRow {
                key: "profile".into(),
                label: tr("Active profile").to_owned(),
                sub: Some((name, *ON_SURFACE_VARIANT)),
                said: None,
                wrap: false,
                enabled: true,
                act: RowAct::Profile { decided },
            });
        }
        if let Some(app) = classic
            && launch::offers_quit_on_leave(
                host.paired(),
                policy_control(app.control),
                policy_role(app.role(false)),
            ) {
                let on = host.record.quits_on_leave(&app.uuid, app.id);
                rows.push(ArtRow {
                    key: "quit-on-leave".into(),
                    label: tr("Quit game when I leave").to_owned(),
                    sub: None,
                    said: Some(
                        if on {
                            tr("On: Disconnect also quits it on the PC")
                        } else {
                            tr("Off: Disconnect leaves it running on the PC")
                        }
                        .to_owned(),
                    ),
                    wrap: false,
                    enabled: true,
                    act: RowAct::QuitOnLeave {
                        uuid: app.uuid.clone(),
                        id: app.id,
                        on,
                    },
                });
            }
        if !host.record.unique_id.is_empty() {
            let on = console::is_hidden(source, &game.install_id, game.app_id, &self.hidden_games);
            let target = GameTarget::Library {
                app: game.app_id,
                favourite: false,
            };
            rows.push(ArtRow {
                key: "hide".into(),
                label: tr("Hide game").to_owned(),
                sub: None,
                said: None,
                wrap: false,
                enabled: true,
                act: RowAct::Hide {
                    games: hide_keys_of(host, target, sources),
                    on,
                },
            });
        }
        if let Some(app) = classic
            && let Some(blocker) =
                small_stream_offer(host, app, state.resumes, self.settings.input_only_any_host)
            {
                let (note, ink) = match blocker {
                    Some(InputOnlyBlocker::Pair) => (tr("Pair with this PC first").to_owned(), *ERROR),
                    Some(InputOnlyBlocker::ViewPermission) => (tr("Allow viewing streams for this device on the PC").to_owned(), *ERROR),
                    Some(InputOnlyBlocker::InputPermission) => (tr("Allow input for this device on the PC").to_owned(), *ERROR),
                    None => (
                        trf("Video still streams at the smallest size, about {} Mbit/s ({} MB an hour), but isn’t shown", &[
                            &format!("{:.1}", f64::from(launch::estimated_kbps()) / 1000.0),
                            &launch::estimated_megabytes_per_hour().to_string(),
                        ]),
                        *ON_SURFACE_VARIANT,
                    ),
                };
                rows.push(ArtRow {
                    key: "input-only".into(),
                    label: tr("Input-only").to_owned(),
                    sub: Some((note, ink)),
                    said: None,
                    wrap: false,
                    enabled: blocker.is_none() && state.can_play,
                    act: RowAct::InputOnly {
                        app: app.id,
                        resume: state.resumes,
                    },
                });
            }
        if offers_neural(host) {
            rows.push(self.neural_row(host, game.app_id));
        }
        // Hidden or not: the other PCs' copies are named either way.
        let merged = self.library.get(sources, &self.hidden_games);
        let others = merged
            .all()
            .iter()
            .find(|e| {
                e.copies
                    .iter()
                    .any(|c| c.host == host.id && c.app_id == game.app_id)
            })
            .map(|e| {
                e.copies
                    .iter()
                    .filter(|c| !(c.host == host.id && c.app_id == game.app_id))
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for copy in others {
            // A copy there that is not installed opens, but says so first.
            let name = if copy.installed {
                copy.host_name.clone()
            } else {
                trf("{}, not installed", &[&copy.host_name])
            };
            rows.push(ArtRow {
                key: format!("copy:{}", copy.host),
                label: tr("Also on").to_owned(),
                sub: Some((name, *ON_SURFACE_VARIANT)),
                said: None,
                wrap: false,
                enabled: true,
                act: RowAct::Copy {
                    host: copy.host,
                    app: copy.app_id,
                },
            });
        }
        let _ = view;
        rows
    }

    /// The group of rows at the top of `area`. Returns its foot.
    pub(super) fn paint_rows(
        &mut self,
        ui: &mut Ui,
        rows: &[ArtRow],
        area: Rect,
        host: &Host,
        commands: &mut Vec<Command>,
    ) -> f32 {
        let ctx = ui.ctx().clone();
        let width = area.width();
        // Set first: the group's height is what its words need.
        let set: Vec<_> = rows
            .iter()
            .map(|row| {
                let trailing = if row.switch().is_some() {
                    SWITCH.x
                } else {
                    16.0
                };
                let room = width - 2.0 * ROW_PAD - trailing - 12.0;
                let ink = if row.enabled { *ON_SURFACE } else { *OUTLINE };
                let mut label = egui::text::LayoutJob::simple(
                    row.label.clone(),
                    semibold(t::TYPE_BODY_SIZE),
                    ink,
                    room,
                );
                label.wrap.max_rows = 2;
                label.wrap.overflow_character = Some('\u{2026}');
                let label = ui.painter().layout_job(label);
                let sub = row.sub.as_ref().map(|(words, sub_ink)| {
                    let job = if row.wrap {
                        egui::text::LayoutJob::simple(
                            words.clone(),
                            regular(t::TYPE_SUB_SIZE),
                            *sub_ink,
                            room,
                        )
                    } else {
                        let mut job = egui::text::LayoutJob::simple_singleline(
                            words.clone(),
                            regular(t::TYPE_SUB_SIZE),
                            *sub_ink,
                        );
                        job.wrap = egui::text::TextWrapping::truncate_at_width(room);
                        job
                    };
                    ui.painter().layout_job(job)
                });
                let words = label.size().y + sub.as_ref().map_or(0.0, |s| s.size().y + 2.0);
                let h = (words + 2.0 * ROW_V_PAD).max(t::SIZE_ROW_HEIGHT);
                (label, sub, h)
            })
            .collect();
        let total: f32 = set.iter().map(|s| s.2).sum();
        let group = Rect::from_min_size(area.min, vec2(width, total));
        let mut out = Vec::new();
        glass::group(
            &mut out,
            &ctx,
            group,
            t::RADIUS_GROUP,
            shade::TINT_GROUP_ON_ART_FILL.get(),
        );
        ui.painter().extend(out);
        let mut y = group.top();
        for (n, (row, (label, sub, h))) in rows.iter().zip(set).enumerate() {
            let rect = Rect::from_min_size(pos2(group.left(), y), vec2(width, h));
            let id = Id::unique(("game-row", &row.key));
            let sense = if row.enabled {
                Sense::click()
            } else {
                Sense::hover()
            };
            let response = ui.interact(rect, id, sense);
            let said = row
                .said
                .clone()
                .or_else(|| row.sub.as_ref().map(|(w, _)| w.clone()))
                .unwrap_or_default();
            response.widget_info(|| match row.switch() {
                Some(on) => egui::WidgetInfo::selected(
                    egui::Role::Switch,
                    row.enabled,
                    on,
                    row.label.clone(),
                ),
                None => {
                    egui::WidgetInfo::labeled(egui::Role::Button, row.enabled, row.label.clone())
                }
            });
            if !said.is_empty() {
                name_for_readers(ui, &response, &row.label, &said);
            }
            let feel = glass::feel(&ctx, &response, 1.0, 1.0);
            let inner = rect.shrink2(vec2(4.0, 3.0));
            let mut out = Vec::new();
            if row.enabled {
                lift_shapes(
                    &mut out,
                    &ctx,
                    inner,
                    ROW_INNER_R,
                    feel.hovered,
                    feel.pressed,
                );
            }
            if n > 0 {
                out.push(Shape::hline(
                    rect.left() + ROW_PAD..=rect.right(),
                    y,
                    Stroke::new(1.0, shade::TINT_SEPARATOR.get()),
                ));
            }
            let words_h = label.size().y + sub.as_ref().map_or(0.0, |s| s.size().y + 2.0);
            let mut text_y = rect.center().y - words_h / 2.0;
            let cut = sub.as_ref().is_some_and(|s| s.elided);
            let ink = if row.enabled { *ON_SURFACE } else { *OUTLINE };
            let label_h = label.size().y;
            out.push(Shape::galley(
                pos2(rect.left() + ROW_PAD, text_y),
                label,
                ink,
            ));
            text_y += label_h + 2.0;
            if let Some(sub) = sub {
                let sub_ink = row.sub.as_ref().map_or(*ON_SURFACE_VARIANT, |s| s.1);
                out.push(Shape::galley(
                    pos2(rect.left() + ROW_PAD, text_y),
                    sub,
                    sub_ink,
                ));
            }
            match row.switch() {
                Some(on) => {
                    let track = Rect::from_min_size(
                        pos2(
                            rect.right() - ROW_PAD - SWITCH.x,
                            rect.center().y - SWITCH.y / 2.0,
                        ),
                        SWITCH,
                    );
                    paint_switch(ui, &mut out, track, id.with("switch"), on, row.enabled);
                }
                None if matches!(row.act, RowAct::Nothing) => {}
                None => {
                    let tip = pos2(rect.right() - ROW_PAD - 3.0, rect.center().y);
                    let ink: Color32 = if row.enabled {
                        *ON_SURFACE_VARIANT
                    } else {
                        *OUTLINE
                    };
                    out.push(Shape::line(
                        vec![tip + vec2(-5.0, -6.0), tip, tip + vec2(-5.0, 6.0)],
                        Stroke::new(1.8, ink),
                    ));
                }
            }
            if feel.focused {
                glass::focus(&mut out, &ctx, inner, ROW_INNER_R, false);
            }
            ui.painter().extend(out);
            if cut {
                let full = row.sub.as_ref().map_or_else(String::new, |s| s.0.clone());
                let _ = response.clone().on_hover_text(full);
            }
            self.focus_item(&response, &row.key, false);
            if response.clicked() {
                match &row.act {
                    RowAct::Settings(app) => commands.push(Command::AppSettings {
                        host: host.id,
                        app: app.clone(),
                    }),
                    RowAct::QuitOnLeave { uuid, id, on } => {
                        commands.push(Command::SetQuitOnLeave {
                            host: host.id,
                            uuid: uuid.clone(),
                            id: *id,
                            enabled: !on,
                        })
                    }
                    RowAct::Hide { games, on } => commands.push(Command::SetHidden {
                        games: games.clone(),
                        hidden: !on,
                    }),
                    RowAct::InputOnly { app, resume } => commands.push(Command::LaunchInputOnly {
                        host: host.id,
                        app: *app,
                        resume: *resume,
                    }),
                    RowAct::Copy { host, app } => self.go(Screen::Game {
                        host: *host,
                        app: *app,
                    }),
                    RowAct::Profile { decided } => {
                        if *decided {
                            self.open(Screen::Profiles);
                        }
                    }
                    RowAct::Neural { game, on, accepted } => {
                        if *on {
                            self.neural.write(host.id, *game, false, false);
                        } else if *accepted {
                            self.neural.write(host.id, *game, true, false);
                        } else {
                            self.ask(Dialog::ConfirmNeural {
                                host: host.id,
                                game: *game,
                            });
                        }
                    }
                    RowAct::Nothing => {}
                }
            }
            if matches!(row.act, RowAct::Profile { decided: false }) {
                // The click above opens the list (the popup toggles on it); a pick is what the
                // Profiles page's Active profile does.
                let active = self.profiles.active;
                let choices: Vec<(Option<u64>, String)> =
                    std::iter::once((None, tr("Main settings").to_owned()))
                        .chain(
                            self.profiles
                                .entries
                                .iter()
                                .map(|p| (Some(p.id), p.name.clone())),
                        )
                        .collect();
                let mut picked = None;
                let _ = egui::Popup::menu(&response).show(|ui| {
                    for (id, name) in choices {
                        if ui.selectable_label(active == id, name).clicked() {
                            picked = Some(id);
                        }
                    }
                });
                if let Some(id) = picked
                    && id != active {
                        self.profiles.active = id;
                        commands.push(Command::SaveProfiles);
                    }
            }
            y += h;
        }
        y
    }
}

const ROW_INNER_R: f32 = 16.0;
