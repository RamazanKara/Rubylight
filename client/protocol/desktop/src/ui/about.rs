//! Settings → App → About: the version, the licence and the credits, with Android's About line
//! ("Version … · Based on Moonlight (GPL-3.0)") and its Source and licenses link. The credits are
//! the client's, as `client/README.md` and the desktop README name them: Moonlight first, which
//! Android's About names.

use super::*;

/// Where Source and licenses goes, as on Android.
pub const SOURCE: &str = "https://github.com/RamazanKara/nightjar";

/// The app's name, a name in every language.
const NAME: &str = "Nightjar";

/// The copyright line: a legal notice, the same in every language.
const COPYRIGHT: &str = "Copyright © 2026 RamazanKara · GPL-3.0-only";

/// One project Nightjar comes from or builds on.
pub(super) struct Credit {
    pub name: &'static str,
    /// Its authors; "and contributors" follows where `contributors`.
    pub by: &'static str,
    pub contributors: bool,
    pub licence: &'static str,
}

/// The projects Nightjar is a fork of or ports from, all GPL-3.0, Moonlight first.
pub(super) const CREDITS: [Credit; 9] = [
    Credit { name: "Moonlight Android", by: "Cameron Gutman, Diego Waxemberg, Aaron Neyer, Andrew Hennessy", contributors: true, licence: "GPL-3.0" },
    Credit { name: "moonlight-qt", by: "Cameron Gutman", contributors: true, licence: "GPL-3.0" },
    Credit { name: "moonlight-common-c", by: "Cameron Gutman", contributors: true, licence: "GPL-3.0" },
    Credit { name: "Moonlight V+ · Moonlight V+ PC", by: "qiin2333", contributors: true, licence: "GPL-3.0" },
    Credit { name: "Artemis Android", by: "ClassicOldSong", contributors: false, licence: "GPL-3.0" },
    Credit { name: "Moonlight X", by: "MoreOrLessSoftware", contributors: false, licence: "GPL-3.0" },
    Credit { name: "moonlight-noir", by: "derflacco", contributors: false, licence: "GPL-3.0" },
    Credit { name: "moonlight-qt (Nonary)", by: "Nonary (Chase Payne)", contributors: false, licence: "GPL-3.0" },
    Credit { name: "moonlight-common-c (Nonary)", by: "Nonary", contributors: false, licence: "GPL-3.0" },
];

/// The components the desktop client ships, each under its own licence (the packages carry their
/// notices: `package-windows.ps1` and the others).
pub(super) const COMPONENTS: [(&str, &str); 9] = [
    ("SDL", "zlib"),
    ("FFmpeg", "LGPL-2.1-or-later"),
    ("dav1d", "BSD-2-Clause"),
    // Marked, as the codec's name is in the tables.
    (en("PyroWave"), "MIT"),
    ("Granite", "MIT"),
    ("AMD FSR 1", "MIT"),
    ("Qualcomm SGSR 1", "BSD-3-Clause"),
    ("sonora", "BSD-3-Clause"),
    ("Figtree", "OFL-1.1"),
];

/// A credit's second line: its authors and licence.
pub(super) fn credit_line(credit: &Credit) -> String {
    let by = if credit.contributors { trf("{} and contributors", &[&credit.by]) } else { credit.by.to_owned() };
    format!("{by} · {}", credit.licence)
}

impl Launcher {
    pub(super) fn about_page(&mut self, ui: &mut Ui, commands: &mut Vec<Command>) {
        let focus = std::mem::take(&mut self.focus_screen);
        column(ui, SETTINGS_W, |ui| {
            ui.label(RichText::new(NAME).font(semibold(text::TITLE)).color(ON_SURFACE));
            ui.label(RichText::new(trf("Version {} · Based on Moonlight (GPL-3.0)", &[&crate::VERSION])).font(regular(text::BODY)).color(ON_SURFACE_VARIANT));
            ui.label(RichText::new(COPYRIGHT).font(regular(text::CAPTION)).color(ON_SURFACE_VARIANT));
            ui.add_space(space::M);
            let source = quiet_button(ui, "Source and licenses");
            if focus {
                source.request_focus();
            }
            if source.clicked() {
                commands.push(Command::OpenSource);
            }
            section(ui, "Credits");
            for credit in &CREDITS {
                credit_row(ui, credit.name, &credit_line(credit));
            }
            section(ui, "Bundled components");
            for (name, licence) in COMPONENTS {
                credit_row(ui, tr(name), licence);
            }
        });
    }
}

/// A credit in the group: its name, and its authors and licence beneath. Read as one by a screen
/// reader.
fn credit_row(ui: &mut Ui, name: &str, line: &str) {
    let rect = group_row(ui, |ui| {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.set_min_height(design_tokens::SIZE_ROW_HEIGHT - 2.0 * 8.0);
            ui.label(RichText::new(name).font(semibold(text::BODY)).color(ON_SURFACE));
            ui.label(RichText::new(line).font(regular(text::CAPTION)).color(ON_SURFACE_VARIANT));
        });
    });
    let response = ui.interact(rect, Id::unique(("credit", name)), egui::Sense::hover());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, format!("{name}, {line}")));
}
