//! Drawing of the in-stream "Session and graphics" page (see `session_page` for what it does).
//! The buttons are plain egui widgets, so Tab, the arrow keys and a controller (mapped to the
//! arrow keys while the menu is open) reach every one of them.

use crate::strings::{en, tr, trf};
use egui::RichText;
use nightjar_core::http::native_api::radeon::{Kind, Value};
use nightjar_core::http::native_api::session::State as SessionState;

use crate::radeon::{Mode, Row};
use crate::session_page::{exit_failure_text, ExitState, HostExit, Hints, Page, State};
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

pub fn show(ui: &mut egui::Ui, page: &mut Page, title: &str, lines: &[(&'static str, String)], hints: Hints) -> Outcome {
    let ctx = ui.ctx().clone();
    let state = page.state();
    let mut outcome = Outcome::Stay;
    let working = matches!(state.exit, ExitState::Running(_));
    if matches!(state.exit, ExitState::Done(_)) {
        return Outcome::Leave;
    }

    ui.label(RichText::new(title).font(theme::semibold(theme::text::BODY)).color(theme::MOON));
    ui.label(RichText::new(session_text(&state)).color(theme::ON_SURFACE_VARIANT));
    if let Some(error) = &state.setup_error {
        ui.colored_label(theme::ERROR, error);
    }
    ui.separator();

    if let Some(exit) = page.confirming {
        ui.label(tr(exit.confirm()));
        ui.horizontal(|ui| {
            let cancel = ui.add_enabled(!working, egui::Button::new(tr("Cancel")));
            if ctx.memory(|m| m.focused().is_none()) {
                cancel.request_focus();
            }
            if cancel.clicked() {
                page.confirming = None;
            }
            if ui.add_enabled(!working, egui::Button::new(tr(exit.label()))).clicked() {
                page.confirming = None;
                page.exit(exit);
            }
        });
    } else {
        let resume = ui.add_enabled(!working, egui::Button::new(tr("Resume")));
        if ctx.memory(|m| m.focused().is_none()) {
            resume.request_focus();
        }
        if resume.clicked() {
            outcome = Outcome::Resume;
        }
        ui.label(RichText::new(tr("Return to the stream.")).color(theme::OUTLINE));
        if state.host_menu() {
            if ui.add_enabled(!working, egui::Button::new(tr("Open host menu"))).clicked() {
                outcome = Outcome::HostMenu;
            }
            ui.label(RichText::new(tr("Return to the stream and show the PC's own menu in it.")).color(theme::OUTLINE));
        }
        if ui.add_enabled(!working, egui::Button::new(tr("Disconnect"))).clicked() {
            outcome = Outcome::Disconnect;
        }
        ui.label(RichText::new(tr("Leave the stream. The game keeps running and the session stays suspended on the PC.")).color(theme::OUTLINE));
        for entry in state.actions(hints) {
            let enabled = !working && entry.blocked.is_none();
            if ui.add_enabled(enabled, egui::Button::new(tr(entry.exit.label()))).clicked() {
                page.confirming = Some(entry.exit);
            }
            let note = entry.blocked.unwrap_or_else(|| describe(entry.exit));
            ui.label(RichText::new(tr(note)).color(theme::OUTLINE));
        }
    }
    match &state.exit {
        ExitState::Running(exit) => {
            ui.label(RichText::new(format!("{}…", tr(exit.label()))).color(theme::ON_SURFACE_VARIANT));
        }
        ExitState::Failed(_, failure) => {
            ui.colored_label(theme::ERROR, exit_failure_text(failure));
        }
        ExitState::Idle | ExitState::Done(_) => {}
    }

    ui.separator();
    ui.label(RichText::new(tr("Connection")).font(theme::semibold(theme::text::BODY)));
    egui::Grid::new("session-facts").num_columns(2).spacing([14.0, 3.0]).show(ui, |ui| {
        for (key, value) in lines {
            ui.label(RichText::new(tr(key)).color(theme::OUTLINE));
            ui.label(value);
            ui.end_row();
        }
    });

    if !state.radeon.hidden {
        ui.separator();
        radeon_panel(ui, page, &state);
    }

    ui.separator();
    if ui.add_enabled(!working, egui::Button::new(tr("Back to stream controls"))).clicked() && outcome == Outcome::Stay {
        outcome = Outcome::Back;
    }
    outcome
}

fn radeon_panel(ui: &mut egui::Ui, page: &Page, state: &State) {
    ui.label(RichText::new(tr("Radeon graphics")).font(theme::semibold(theme::text::BODY)));
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
            if ui.add_enabled(enabled, egui::Button::new("+")).clicked() {
                send(row.step(1));
            }
            ui.label(row.value_text());
            if ui.add_enabled(enabled, egui::Button::new("−")).clicked() {
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
