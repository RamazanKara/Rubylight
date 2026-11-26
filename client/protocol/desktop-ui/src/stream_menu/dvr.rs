//! The stream menu's Instant Replay group under the grid: Save clip and Start / Stop recording
//! as the stream's DVR state has them (`nightjar_desktop_media::stream::dvr`), each with the line
//! under it, and greyed out with the reason while the host cannot save. No button hints: the
//! rows are plain actions for touch, mouse and controller alike.

use egui::Ui;
use rubylight_protocol::stream_dvr::Op;

use crate::glyphs::Icon;
use crate::menu_style;
use crate::strings::tr;

/// One entry: what pressing it asks the host, its label, the line under it, whether it works,
/// and whether that line says why not.
pub type Entry<'a> = (Op, &'a str, &'a str, bool, bool);

/// Draws the group; the op of the entry pressed.
pub fn section(ui: &mut Ui, entries: &[Entry]) -> Option<Op> {
    let mut chosen = None;
    menu_style::caption(ui, tr("Instant Replay"));
    menu_style::group(ui, |ui| {
        for &(op, label, detail, enabled, problem) in entries {
            if ui.add_enabled_ui(enabled, |ui| menu_style::action(ui, icon(op), label, None)).inner.clicked() {
                chosen = Some(op);
            }
            match (detail.is_empty(), problem) {
                (true, _) => {}
                (false, true) => menu_style::problem(ui, detail),
                (false, false) => menu_style::note(ui, detail),
            }
        }
    });
    chosen
}

/// Save clip keeps the buffer; recording starts and stops like a player.
pub fn icon(op: Op) -> Icon {
    match op {
        Op::SaveClip | Op::QueryState => Icon::Download,
        Op::StartRecord | Op::StopRecord => Icon::Play,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_disabled_entry_is_never_chosen_and_the_group_draws_without_one() {
        crate::theme::own_palette();
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let mut chosen = Some(Op::QueryState);
        let entries = [
            (Op::SaveClip, "Save clip", "Last 60 s ready", true, false),
            (Op::StartRecord, "Start recording", "Clips cannot be saved.", false, true),
        ];
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            menu_style::show(ui.ctx(), "Stream menu", "dvr-test", |ui| chosen = section(ui, &entries));
        });
        // No renderer takes the textures here, and egui refuses to drop them unasked.
        output.textures_delta.clear();
        assert_eq!(chosen, None);
        assert_eq!(icon(Op::StopRecord), Icon::Play);
    }
}
