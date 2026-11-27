//! Instant Replay's lines on the Control Center's Clips page, under its Save clip and Start /
//! Stop recording cards: what the stream's DVR state (`nightjar_desktop_media::stream::dvr`)
//! says about each, and why the host cannot save while it cannot.

use egui::Ui;
use rubylight_protocol::stream_dvr::Op;

use crate::menu_style;
use crate::strings::tr;

/// One entry: what pressing it asks the host, its label, the line under it, whether it works,
/// and whether that line says why not.
pub type Entry<'a> = (Op, &'a str, &'a str, bool, bool);

/// The lines under the Clips page's cards, captioned Instant Replay: what the buffer holds, the
/// recording's time, or why the host cannot save (in the problem colour). Nothing without a line.
pub fn notes(ui: &mut Ui, entries: &[Entry]) {
    if entries.iter().all(|entry| entry.2.is_empty()) {
        return;
    }
    menu_style::caption(ui, tr("Instant Replay"));
    for &(_, _, detail, _, problem) in entries {
        match (detail.is_empty(), problem) {
            (true, _) => {}
            (false, true) => menu_style::problem(ui, detail),
            (false, false) => menu_style::note(ui, detail),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lines_draw_with_a_problem_and_without_any() {
        crate::theme::own_palette();
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let lines = [
            (Op::SaveClip, "Save clip", "Last 60 s ready", true, false),
            (Op::StartRecord, "Start recording", "Clips cannot be saved.", false, true),
        ];
        let none = [(Op::SaveClip, "Save clip", "", true, false), (Op::StartRecord, "Start recording", "", true, false)];
        for entries in [lines, none] {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                menu_style::show(ui.ctx(), "Control Center", "dvr-test", |ui| notes(ui, &entries));
            });
            // No renderer takes the textures here, and egui refuses to drop them unasked.
            output.textures_delta.clear();
        }
    }
}
