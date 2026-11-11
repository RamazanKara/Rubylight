//! Drawing of the in-stream "Session and graphics" page (see `session_page` for what it does).
//! It is drawn with the stream menu's rows (`menu_style`), so Tab, the arrow keys and a controller
//! (mapped to the arrow keys while the menu is open) reach every one of them, and the ways to end
//! the session sit apart in the error colour.

use crate::strings::{en, tr, trf};
use egui::RichText;
use nightjar_core::http::native_api::radeon::{Kind, Value};
use nightjar_core::http::native_api::session::State as SessionState;

use crate::radeon::{Mode, Row};
use crate::session_page::{exit_failure_text, ExitState, HostExit, Hints, Page, State};
use crate::glyphs::Icon;
use crate::menu_style;
use crate::theme;

/// What the page asks the stream to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Stay,
    /// Back to the stream.
    Resume,
    /// Back to the stream, then ask the host to open its own menu in it.
    HostMenu,
    /// Leave the stream; the game keeps running.
    Disconnect,
    /// Back to the stream controls.
    Back,
    /// The host ended the session or the game: leave the stream.
    Leave,
}

pub fn session_text(state: &State) -> String {
    let Some(session) = &state.session else {
        return match &state.session_error {
            Some(error) => trf("Session unknown: {}", &[error]),
            None => tr("Reading the session…").to_owned(),
        };
    };
    let phase = match session.state {
        SessionState::Idle | SessionState::Unknown => tr("No session"),
        SessionState::Preparing => tr("Preparing"),
        SessionState::Launching => tr("Launching"),
        SessionState::Streaming => tr("Streaming"),
        SessionState::Suspended => tr("Suspended"),
        SessionState::Stopping => tr("Ending"),
    };
    if session.game_running { trf("{}. The game is running.", &[&phase]) } else { format!("{phase}.") }
}

fn describe(exit: HostExit) -> &'static str {
    match exit {
        HostExit::End => en("Ends the stream. The game keeps running on the PC."),
        HostExit::CloseGame => en("Ends the stream and closes the game."),
        HostExit::QuitApp => en("Quits the game on the PC."),
    }
}

/// The page under its header (the caller's, with the game's line).
pub fn show(ui: &mut egui::Ui, page: &mut Page, lines: &[(&'static str, String)], hints: Hints) -> Outcome {
    let ctx = ui.ctx().clone();
    let state = page.state();
    let mut outcome = Outcome::Stay;
    let working = matches!(state.exit, ExitState::Running(_));
    if matches!(state.exit, ExitState::Done(_)) {
        return Outcome::Leave;
    }

    ui.label(RichText::new(session_text(&state)).font(theme::regular(theme::text::LABEL)).color(theme::ON_SURFACE_VARIANT));
    if let Some(error) = &state.setup_error {
        ui.label(RichText::new(error).font(theme::regular(theme::text::LABEL)).color(theme::ERROR));
    }
    ui.add_space(theme::space::L);

    if let Some(exit) = page.confirming {
        menu_style::text_block(ui, tr(exit.confirm()));
        ui.add_space(theme::space::S);
        ui.horizontal(|ui| {
            let cancel = ui.add_enabled_ui(!working, |ui| menu_style::button(ui, tr("Cancel"))).inner;
            if ctx.memory(|m| m.focused().is_none()) {
                cancel.request_focus();
            }
            if cancel.clicked() {
                page.confirming = None;
            }
            if ui.add_enabled_ui(!working, |ui| menu_style::button(ui, tr(exit.label()))).inner.clicked() {
                page.confirming = None;
                page.exit(exit);
            }
        });
    } else {
        let resume = ui.add_enabled_ui(!working, |ui| menu_style::primary(ui, tr("Resume"), None)).inner;
        if ctx.memory(|m| m.focused().is_none()) {
            resume.request_focus();
        }
        if resume.clicked() {
            outcome = Outcome::Resume;
        }
        ui.add_space(theme::space::XS);
        ui.add_enabled_ui(!working, |ui| {
            if state.host_menu() {
                if menu_style::action(ui, Icon::Screen, tr("Open host menu"), None).clicked() {
                    outcome = Outcome::HostMenu;
                }
                menu_style::note(ui, tr("Return to the stream and show the PC's own menu in it."));
            }
            if menu_style::action(ui, Icon::Power, tr("Disconnect"), None).clicked() {
                outcome = Outcome::Disconnect;
            }
            menu_style::note(ui, tr("Leave the stream. The game keeps running and the session stays suspended on the PC."));
        });
        let actions = state.actions(hints);
        if !actions.is_empty() {
            menu_style::divider(ui);
        }
        for entry in actions {
            let enabled = !working && entry.blocked.is_none();
            if ui.add_enabled_ui(enabled, |ui| menu_style::danger(ui, Icon::Power, tr(entry.exit.label()), None)).inner.clicked() {
                page.confirming = Some(entry.exit);
            }
            let note = entry.blocked.unwrap_or_else(|| describe(entry.exit));
            menu_style::note(ui, tr(note));
        }
    }
    match &state.exit {
        ExitState::Running(exit) => {
            menu_style::note(ui, &format!("{}…", tr(exit.label())));
        }
        ExitState::Failed(_, failure) => {
            ui.label(RichText::new(exit_failure_text(failure)).font(theme::regular(theme::text::LABEL)).color(theme::ERROR));
        }
        ExitState::Idle | ExitState::Done(_) => {}
    }

    menu_style::caption(ui, tr("Connection"));
    egui::Frame::new().inner_margin(egui::Margin::symmetric(menu_style::PAD as i8, 0)).show(ui, |ui| {
        egui::Grid::new("session-facts").num_columns(2).spacing([14.0, 4.0]).show(ui, |ui| {
            for (key, value) in lines {
                ui.label(RichText::new(tr(key)).font(theme::regular(theme::text::LABEL)).color(theme::ON_SURFACE_VARIANT));
                ui.label(RichText::new(value).font(theme::regular(theme::text::LABEL)).color(theme::ON_SURFACE));
                ui.end_row();
            }
        });
    });

    if !state.radeon.hidden {
        radeon_panel(ui, page, &state);
    }

    menu_style::divider(ui);
    // Escape or B on this page goes back to the controls (main.rs), so the key sits here.
    let back = ui.add_enabled_ui(!working, |ui| menu_style::action_keys(ui, Icon::Sliders, tr("Back to stream controls"), Some(crate::shortcuts::Key::Escape.cap().as_ref())));
    if back.inner.clicked() && outcome == Outcome::Stay {
        outcome = Outcome::Back;
    }
    outcome
}

fn radeon_panel(ui: &mut egui::Ui, page: &Page, state: &State) {
    menu_style::caption(ui, tr("Graphics settings"));
    let Some(panel) = &state.radeon.panel else {
        let text = match &state.radeon.load_failure {
            Some(failure) => failure.text(),
            None => tr("Reading the settings…").to_owned(),
        };
        ui.label(RichText::new(text).color(theme::ON_SURFACE_VARIANT));
        return;
    };
    if let Some(text) = panel.notice.text() {
        ui.label(RichText::new(tr(text)).color(theme::ON_SURFACE_VARIANT));
    }
    if let Some(message) = &panel.message {
        ui.label(RichText::new(message).color(theme::OUTLINE));
    }
    if panel.gpus.iter().all(|g| g.rows.is_empty()) {
        ui.label(RichText::new(tr("The host reports no settings.")).color(theme::OUTLINE));
    }
    let failure = state.radeon.write_failure.as_ref();
    for gpu in &panel.gpus {
        if gpu.rows.is_empty() {
            continue;
        }
        if panel.gpus.len() > 1 || !gpu.name.is_empty() {
            let name = if gpu.driver.is_empty() { gpu.name.clone() } else { trf("{} (driver {})", &[&gpu.name, &gpu.driver]) };
            ui.label(RichText::new(name).color(theme::OUTLINE));
        }
        for row in &gpu.rows {
            draw_row(ui, page, state, row);
            if let Some(failure) = failure.filter(|f| f.setting.as_deref() == Some(row.id.as_str())) {
                ui.colored_label(theme::ERROR, failure.text());
            }
        }
    }
    // A failure for a setting this panel does not list still has to be seen.
    if let Some(failure) = failure.filter(|f| f.setting.as_deref().is_none_or(|id| panel.find(id).is_none())) {
        ui.colored_label(theme::ERROR, failure.text());
    }
}

fn draw_row(ui: &mut egui::Ui, page: &Page, state: &State, row: &Row) {
    let pending = state.radeon.pending.as_deref() == Some(row.id.as_str());
    ui.horizontal(|ui| {
        ui.label(row.title());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| match row.mode {
            Mode::Edit => edit(ui, page, state, row, pending),
            Mode::Value => {
                ui.label(RichText::new(row.value_text()).color(theme::ON_SURFACE_VARIANT));
            }
            Mode::ExternalSetup | Mode::Unsupported | Mode::Unavailable => {
                ui.label(RichText::new(match row.mode {
                    Mode::ExternalSetup => tr("Needs setup"),
                    Mode::Unsupported => tr("Not supported"),
                    _ => tr("Unavailable"),
                }).color(theme::OUTLINE));
            }
        });
    });
    if let Some(note) = row.state_note() {
        ui.label(RichText::new(note).color(theme::OUTLINE));
    }
    if pending {
        ui.label(RichText::new(tr("Applying…")).color(theme::ON_SURFACE_VARIANT));
    }
    if let Some(hint) = row.restore_hint() {
        ui.label(RichText::new(hint).color(theme::OUTLINE));
    }
}

fn edit(ui: &mut egui::Ui, page: &Page, state: &State, row: &Row, pending: bool) {
    let enabled = state.can_write(row) && !pending;
    let send = |value: Option<Value>| {
        if let Some(value) = value {
            page.write(row, value);
        }
    };
    match row.kind {
        // Right to left: the last widget added sits leftmost.
        Kind::Integer => {
            // Screen readers would say only "plus" and "minus": these name the setting.
            let raise = ui.add_enabled(enabled, egui::Button::new("+"));
            raise.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, trf("Raise {}", &[&row.title()])));
            if raise.clicked() {
                send(row.step(1));
            }
            ui.label(row.value_text());
            let lower = ui.add_enabled(enabled, egui::Button::new("−"));
            lower.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, trf("Lower {}", &[&row.title()])));
            if lower.clicked() {
                send(row.step(-1));
            }
        }
        // A toggle flips and a choice cycles to the next option, as on Android.
        _ => {
            if ui.add_enabled(enabled, egui::Button::new(row.value_text())).clicked() {
                send(row.step(1));
            }
        }
    }
}
