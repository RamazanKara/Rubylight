//! The launch scene: from Play until the stream takes the window, the game's cover in its own
//! ambient with one thin ring turning around it, as Nightjar's `ConnectingScreen` on Android. A start
//! that fails turns it into a card beside the cover, which offers the same start again, the
//! stream's settings and a test of the network. The scene is the launcher's: it is over when the
//! stream's window takes over. A launch curtain (`launch_curtain.rs`) draws its cover, ring and
//! words the same way over a stream, in the words written out here (`curtain_words`).

use super::design_tokens as t;
use super::*;
use crate::conn_test;

/// The cover's height, as a share of the window's, and its bounds (at most the size covers are
/// kept for); its width, at most, as a share of the window's, so a narrow window keeps room for
/// the words.
const COVER_SHARE: f32 = 0.56;
const COVER_LEAST: f32 = 220.0;
pub(super) const COVER_MOST: f32 = crate::hosts::COVER_POINTS[1];
const COVER_WIDE_SHARE: f32 = 0.26;
/// Where the cover's middle is across the window while connecting, and once the start failed.
pub(super) const COVER_AT: f32 = 0.34;
const COVER_FAILED_AT: f32 = 0.25;
/// What the cover glows and casts, as on the game's page.
pub(super) const GLOW_BLUR: f32 = 110.0;
pub(super) const SHADOW_Y: f32 = 30.0;
pub(super) const SHADOW_BLUR: f32 = 70.0;
/// The words' middle across the window, and the most they take.
const WORDS_AT: f32 = 0.75;
const WORDS_SHARE: f32 = 0.36;
const WORDS_MOST: f32 = 560.0;
/// The one ring around the cover, in its heights: clear of the cover's corners and of the words
/// beside it. A lit arc turns on it while the start runs, once in this many seconds, this long
/// and this strong at its head; it rests just below the left without motion.
const RING: f32 = 0.64;
const RING_SECONDS: f64 = 3.2;
const RING_REST: f32 = 2.65;
const RING_ARC: f32 = 1.9;
const RING_LIT: f32 = 0.62;
const RING_HALF_WIDTH: f32 = 1.25;
const RING_STEPS: u32 = 96;
const RING_FRAME_SECONDS: f32 = 1.0 / 60.0;
/// The failure card: where its room starts across the window, the edge it keeps on the right, and
/// the widest it gets.
const CARD_FROM: f32 = 0.49;
const CARD_EDGE: f32 = 0.05;
const CARD_MOST: f32 = 620.0;
/// The mark beside the failure's heading, the rows' corners and their least height.
const MARK: f32 = 48.0;
const ROW_RADIUS: f32 = 16.0;
const ROW_LEAST: f32 = 56.0;
const ROW_PAD: f32 = 16.0;
const MOVE_TUNE: spring::Tune = spring::Tune {
    stiffness: t::MOTION_SHEET_STIFFNESS,
    ratio: t::MOTION_SHEET_RATIO,
};

/// The failure card's network test.
pub enum CardTest {
    Offered,
    Running(std::sync::mpsc::Receiver<conn_test::Outcome>),
    Done(conn_test::Outcome),
}

/// What the network test found, in words.
pub(crate) fn test_words(outcome: conn_test::Outcome) -> &'static str {
    match outcome {
        conn_test::Outcome::Clear => tr("Nothing is blocking streaming."),
        conn_test::Outcome::Blocked => tr("Internet streaming may be blocked.\nFor local streaming, check the PC firewall."),
        conn_test::Outcome::Unknown => tr("The test could not finish."),
    }
}

/// The stream's size, rate and bitrate in a line, for the card's Stream settings row.
pub fn settings_line((width, height, fps, bitrate_kbps): (u32, u32, u32, u32)) -> String {
    let mbps = f64::from(bitrate_kbps) / 1000.0;
    let rate = if bitrate_kbps % 1000 == 0 { format!("{mbps:.0} Mbps") } else { format!("{mbps:.1} Mbps") };
    format!("{}, {rate}", trf("{} × {} at {} fps", &[&width, &height, &fps]))
}

/// A launch curtain line (`nightjar_policy::launch_curtain`) in the user's language, `{}` left for
/// the launcher's name. Each English text is written out so the translation tests find it in every
/// table; the stream's notices look theirs up by the same English.
pub(super) fn curtain_words(words: nightjar_policy::launch_curtain::Words) -> &'static str {
    match words.key {
        "preparing" => tr("Preparing PC"),
        "starting" => tr("Starting game"),
        "opening" => tr("Opening window"),
        "focusing" => tr("Focusing game"),
        "updating" => tr("{} is updating"),
        "updating_any" => tr("A launcher is updating"),
        "sign_in" => tr("{} wants you to sign in"),
        "sign_in_any" => tr("A launcher wants you to sign in"),
        "waiting" => tr("{} is waiting for you"),
        "waiting_any" => tr("A window on the PC is waiting for you"),
        "uac" => tr("Windows needs administrator approval on this PC"),
        "hint" => tr("Show the PC screen to answer it from this device, or cancel the launch."),
        "show_screen" => tr("Show PC screen"),
        "still_starting" => tr("Showing PC screen. The game is still starting."),
        "failed" => tr("The game didn’t start on the PC."),
        _ => tr(words.en),
    }
}

/// The cover's place, its middle `along` the window's width: as tall as the window allows, and
/// narrow enough in a narrow window to leave the words their room.
pub(super) fn cover_rect(screen: Rect, along: f32) -> Rect {
    let cover_h = (screen.height() * COVER_SHARE).clamp(COVER_LEAST, COVER_MOST).min(screen.width() * COVER_WIDE_SHARE * 1.5);
    Rect::from_center_size(pos2(screen.left() + screen.width() * along, screen.center().y), vec2(cover_h * 2.0 / 3.0, cover_h))
}

/// The words beside the cover: their middle across the window and the most they take.
pub(super) fn words_column(screen: Rect) -> (f32, f32) {
    (screen.left() + screen.width() * WORDS_AT, (screen.width() * WORDS_SHARE).min(WORDS_MOST))
}

/// The scene's backdrop: the cover's ambient, or the tile's for a game without art.
fn scene_want(view: &View<'_>, launch: &ConnectingView) -> backdrop::Want {
    match view.hosts.get(launch.host_id).and_then(|pc| pc.art.get(&launch.app_id)) {
        Some(_) => home::ambient_for(view, launch.host_id, launch.app_id as u32, &launch.app),
        None => backdrop::Want::Tile { title: launch.title().to_owned() },
    }
}

/// Which opening of the scene this is: a new one starts its moves from where they belong.
fn opening(ctx: &egui::Context) -> u64 {
    let id = Id::unique("launch-scene-opening");
    let pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|d| {
        let (last, opening) = d.get_temp::<(u64, u64)>(id).unwrap_or((u64::MAX, 0));
        let opening = if last.wrapping_add(1) == pass || last == pass { opening } else { opening + 1 };
        d.insert_temp(id, (pass, opening));
        opening
    })
}

/// The one thin ring around the cover: faint all the way round, and on it an arc in the page's
/// ink, lit at its head and gone at its tail, which turns while the start runs and holds still
/// with reduced motion.
pub(super) fn turning_ring(out: &mut Vec<egui::Shape>, ctx: &egui::Context, cover: Rect) {
    let radius = cover.height() * RING;
    let track = if high_contrast() { t::HC_COLOR_RIM } else { shade::SKY_RING.get() };
    out.push(egui::Shape::circle_stroke(cover.center(), radius, Stroke::new(1.0, track)));
    let moving = spring::motion(ctx);
    let head = if moving {
        let along = ctx.input(|i| i.time) / RING_SECONDS;
        along.fract() as f32 * std::f32::consts::TAU + RING_REST
    } else {
        RING_REST
    };
    let mut arc = egui::epaint::Mesh::default();
    for step in 0..=RING_STEPS {
        let back = step as f32 / RING_STEPS as f32;
        let turn = head - back * RING_ARC;
        let out_from_middle = vec2(turn.cos(), turn.sin());
        let lit = ON_SURFACE.gamma_multiply(RING_LIT * (1.0 - back) * (1.0 - back));
        for (off, color) in [(-RING_HALF_WIDTH, Color32::TRANSPARENT), (0.0, lit), (RING_HALF_WIDTH, Color32::TRANSPARENT)] {
            arc.vertices.push(egui::epaint::Vertex { pos: cover.center() + out_from_middle * (radius + off), uv: egui::epaint::WHITE_UV, color });
        }
    }
    for step in 0..RING_STEPS {
        let (a, b) = (step * 3, (step + 1) * 3);
        for k in 0..2 {
            arc.indices.extend([a + k, b + k, b + k + 1, a + k, b + k + 1, a + k + 1]);
        }
    }
    out.push(egui::Shape::mesh(arc));
    if moving {
        ctx.request_repaint_after(std::time::Duration::from_secs_f32(RING_FRAME_SECONDS));
    }
}

impl ConnectingView {
    /// What the cover's tile and the card name: the game, else the PC.
    fn title(&self) -> &str {
        if self.app.is_empty() { &self.host } else { &self.app }
    }
}

impl Launcher {
    /// A start failed before its first frame: the scene stays, with the card.
    pub fn start_failed(&mut self, launch: ConnectingView, body: impl Into<String>, settings: Option<String>) {
        self.dialog = Dialog::ConnectFailed { launch, body: body.into(), settings, test: CardTest::Offered };
        self.focus_dialog = true;
    }

    /// The message shown, as `title: body`, a failed start's card included: what an unattended
    /// start (`--host`, `--autostream`) reports and ends on.
    pub fn message_shown(&self) -> Option<String> {
        match &self.dialog {
            Dialog::Message { title, body } | Dialog::WakeFailed { title, body, .. } => {
                Some(format!("{title}: {body}"))
            }
            Dialog::ConnectFailed { body, .. } => Some(format!("{}: {body}", tr("The stream could not start"))),
            _ => None,
        }
    }

    /// Runs the card's network test; `waker` wakes the app once it is done.
    pub fn test_connection(&mut self, waker: crate::hosts::Waker) {
        if let Dialog::ConnectFailed { test, .. } = &mut self.dialog
            && !matches!(test, CardTest::Running(_)) {
                *test = CardTest::Running(conn_test::start(waker));
            }
    }

    /// Whether the launch scene covers the launcher: a start under way, or the card of one that failed.
    pub(super) fn scene_up(&self, view: &View<'_>) -> bool {
        view.connecting.is_some() || matches!(self.dialog, Dialog::ConnectFailed { .. })
    }

    /// The scene, over the whole window. Escape (B) cancels the start, or closes the card.
    pub(super) fn launch_scene(&mut self, ui: &mut Ui, view: &View<'_>, escape: bool, commands: &mut Vec<Command>) {
        let ctx = ui.ctx().clone();
        let screen = ctx.content_rect();
        // The test's answer, once it is in.
        if let Dialog::ConnectFailed { test, .. } = &mut self.dialog
            && let CardTest::Running(answer) = test {
                match answer.try_recv() {
                    Ok(outcome) => *test = CardTest::Done(outcome),
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => *test = CardTest::Done(conn_test::Outcome::Unknown),
                    Err(std::sync::mpsc::TryRecvError::Empty) => {}
                }
            }
        // A start under way is shown over the card of one that failed before it.
        let failed = match (&view.connecting, &self.dialog) {
            (None, Dialog::ConnectFailed { launch, body, settings, test }) => Some((launch.clone(), body.clone(), settings.clone(), match test {
                CardTest::Offered => None,
                CardTest::Running(_) => Some(None),
                CardTest::Done(outcome) => Some(Some(*outcome)),
            })),
            _ => None,
        };
        let Some(launch) = view.connecting.clone().or_else(|| failed.as_ref().map(|f| f.0.clone())) else {
            return;
        };
        if escape {
            if failed.is_some() {
                self.close_dialog();
            } else {
                commands.push(Command::CancelConnect);
            }
        }
        let opening = opening(&ctx);
        self.ambient.prepare(&ctx, scene_want(view, &launch));
        let under = ui.painter().clone();
        let backdrop_slot = under.add(egui::Shape::Noop);

        let target = if failed.is_some() { COVER_FAILED_AT } else { COVER_AT };
        let along = spring::animate(&ctx, Id::unique(("launch-cover-x", opening)), target, MOVE_TUNE);
        let cover = cover_rect(screen, along);
        let cover_response = ui.interact(cover, Id::unique("launch-cover"), Sense::hover());
        cover_response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Image, true, launch.title()));
        let mut out = Vec::new();
        if failed.is_none() {
            turning_ring(&mut out, &ctx, cover);
        }
        glass::cover_glow(&mut out, &ctx, cover, t::RADIUS_COVER, self.ambient.glow(), GLOW_BLUR, SHADOW_Y, SHADOW_BLUR);
        let art = view.hosts.get(launch.host_id).and_then(|pc| pc.art.get(&launch.app_id));
        cover_shapes(ui, &mut out, cover, cover.width(), t::RADIUS_COVER, Id::unique(("cover-in", launch.host_id, launch.app_id)), launch.title(), art, 1.0);
        ui.painter().extend(out);

        match failed {
            None => self.connecting_words(ui, screen, cover, &launch, commands),
            Some((launch, body, settings, test)) => self.failure_card(ui, screen, &launch, &body, settings.as_deref(), test, commands),
        }
        under.set(backdrop_slot, egui::Shape::Vec(self.ambient.finish(&ctx)));
    }

    /// Beside the cover while connecting: the game, its PC, the stage, and Cancel.
    fn connecting_words(&mut self, ui: &mut Ui, screen: Rect, cover: Rect, launch: &ConnectingView, commands: &mut Vec<Command>) {
        let (middle, width) = words_column(screen);
        let lay = |text: &str, font: egui::FontId, ink: Color32, rows: usize| {
            let mut job = egui::text::LayoutJob::simple(text.to_owned(), font, ink, width);
            job.wrap.max_rows = rows;
            job.halign = Align::Center;
            ui.painter().layout_job(job)
        };
        let title = lay(launch.title(), semibold(text::LARGE_TITLE), *ON_SURFACE, 2);
        let pc = (!launch.app.is_empty() && !launch.host.is_empty()).then(|| lay(&launch.host, regular(text::SECTION), *ON_SURFACE_VARIANT, 1));
        let stage = lay(tr(&launch.stage), regular(text::BODY), *ON_SURFACE, 2);
        let (pc_h, stage_gap, button_gap) = (pc.as_ref().map_or(0.0, |g| space::S + g.size().y), space::XL, space::XL);
        let height = title.size().y + pc_h + stage_gap + stage.size().y + button_gap + Size::CONTROL.height;
        let mut y = cover.center().y - height / 2.0;
        let words_top = y;
        let painter = ui.painter();
        let title_h = title.size().y;
        painter.galley(pos2(middle, y), title, *ON_SURFACE);
        y += title_h;
        if let Some(pc) = pc {
            y += space::S;
            let h = pc.size().y;
            painter.galley(pos2(middle, y), pc, *ON_SURFACE_VARIANT);
            y += h;
        }
        y += stage_gap;
        let stage_h = stage.size().y;
        painter.galley(pos2(middle, y), stage, *ON_SURFACE);
        y += stage_h;
        // A screen reader hears the words as one line.
        let words = Rect::from_x_y_ranges(middle - width / 2.0..=middle + width / 2.0, words_top..=y);
        let said = [launch.title(), if launch.app.is_empty() { "" } else { &launch.host }, tr(&launch.stage)]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(", ");
        ui.interact(words, Id::unique("launch-words"), Sense::hover())
            .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &said));
        y += button_gap;
        let row = Rect::from_x_y_ranges(middle - width / 2.0..=middle + width / 2.0, y..=y + Size::CONTROL.height);
        ui.scope_builder(UiBuilder::new().max_rect(row).layout(Layout::top_down(Align::Center)), |ui| {
            let cancel = button_at_least(ui, Some(Id::unique("launch-cancel")), None, tr("Cancel"), None, Tone::Quiet, Size::CONTROL, 140.0);
            // The one thing to press: it keeps the focus, so A or Enter cancels at once.
            cancel.request_focus();
            if cancel.clicked() {
                commands.push(Command::CancelConnect);
            }
        });
    }

    /// The card of a start that failed: what failed and why, Try again, the stream's settings, a
    /// test of the network, and Back.
    #[allow(clippy::too_many_arguments, reason = "the card's parts, taken out of the dialog it closes")]
    fn failure_card(&mut self, ui: &mut Ui, screen: Rect, launch: &ConnectingView, body: &str, settings: Option<&str>, test: Option<Option<conn_test::Outcome>>, commands: &mut Vec<Command>) {
        let ctx = ui.ctx().clone();
        let room = (screen.left() + screen.width() * CARD_FROM)..=(screen.right() - screen.width() * CARD_EDGE);
        let width = (room.end() - room.start()).min(CARD_MOST);
        let middle = pos2((room.start() + room.end()) / 2.0, screen.center().y);
        let first = std::mem::take(&mut self.focus_dialog);
        let mut close = false;
        let mut open_settings = false;
        egui::Area::new(Id::unique("launch-card"))
            .order(egui::Order::Middle)
            .pivot(Align2::CENTER_CENTER)
            .fixed_pos(middle)
            .show(&ctx, |ui| {
                // On the live tier the card's glass blurs what is under it: the cover's ambient.
                let band = Rect::from_x_y_ranges(middle.x - width / 2.0..=middle.x + width / 2.0, screen.y_range());
                glass::boundary(ui.painter(), vec![band]);
                Frame::new().inner_margin(Margin::same(DIALOG_MARGIN as i8)).show(ui, |ui| {
                    glass_behind(ui, glass::Material::Panel, t::RADIUS_PANEL, DIALOG_MARGIN, |ui| {
                        ui.set_width(width - DIALOG_MARGIN * 2.0);
                        ui.spacing_mut().item_spacing.y = 0.0;
                        heading(ui, launch);
                        ui.add_space(space::M);
                        ui.label(RichText::new(body).font(regular(text::BODY)).color(*ON_SURFACE));
                        ui.add_space(space::L);
                        let retry = button_at_least(ui, Some(Id::unique("launch-retry")), None, tr("Try again"), None, Tone::Primary, Size::CONTROL, ui.available_width());
                        if first {
                            retry.request_focus();
                        }
                        if retry.clicked() {
                            commands.push(Command::RetryLaunch);
                            close = true;
                        }
                        ui.add_space(space::M);
                        let running = matches!(test, Some(None));
                        let result = test.flatten().map(test_words);
                        let rows: Vec<(Id, &str, Option<&str>, bool)> = settings
                            .map(|line| (Id::unique("launch-settings"), tr("Stream settings"), Some(line), false))
                            .into_iter()
                            .chain(std::iter::once((Id::unique("launch-test"), tr("Test the network"), result, running)))
                            .collect();
                        let pressed = card_rows(ui, &rows);
                        match pressed {
                            Some(id) if id == Id::unique("launch-settings") => {
                                close = true;
                                open_settings = true;
                            }
                            // A second press while it runs does nothing: the row stays where it is
                            // so the focus does too.
                            Some(_) if !running => commands.push(Command::TestConnection),
                            _ => {}
                        }
                        ui.add_space(space::L);
                        ui.vertical_centered(|ui| {
                            if button_at_least(ui, Some(Id::unique("launch-back")), None, tr("Back"), None, Tone::Quiet, Size::CONTROL, 140.0).clicked() {
                                close = true;
                            }
                        });
                    });
                });
            });
        if close {
            self.close_dialog();
        }
        if open_settings {
            self.open_settings();
        }
    }
}

/// The card's heading: a mark, what happened, and the game and PC it happened to.
fn heading(ui: &mut Ui, launch: &ConnectingView) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = space::M;
        let (mark, _) = ui.allocate_exact_size(Vec2::splat(MARK), Sense::hover());
        let painter = ui.painter();
        painter.circle_filled(mark.center(), MARK / 2.0, ERROR.gamma_multiply(0.16));
        painter.circle_stroke(mark.center(), MARK / 2.0 - 0.75, Stroke::new(1.5, ERROR.gamma_multiply(0.6)));
        painter.text(mark.center(), Align2::CENTER_CENTER, "!", semibold(text::TITLE), *ERROR);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.add(egui::Label::new(RichText::new(tr("The stream could not start")).font(semibold(text::SECTION)).color(*ON_SURFACE)).wrap());
            let what = [launch.app.as_str(), launch.host.as_str()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(", ");
            if !what.is_empty() {
                ui.add(egui::Label::new(RichText::new(what).font(regular(text::LABEL)).color(*ON_SURFACE_VARIANT)).truncate());
            }
        });
    });
}

/// A group of rows, each a label with an optional line under it and a chevron, or a spinner while
/// it works. The pointer moving onto a row takes the focus there, as in a menu. The row pressed.
fn card_rows(ui: &mut Ui, rows: &[(Id, &str, Option<&str>, bool)]) -> Option<Id> {
    let ctx = ui.ctx().clone();
    let width = ui.available_width();
    let words_w = width - ROW_PAD * 3.0 - 20.0;
    let laid: Vec<_> = rows
        .iter()
        .map(|(_, label, detail, _)| {
            let label = ui.painter().layout(label.to_string(), semibold(text::LABEL), *ON_SURFACE, words_w);
            let detail = detail.map(|d| ui.painter().layout(d.to_owned(), regular(text::CAPTION), *ON_SURFACE_VARIANT, words_w));
            let height = (label.size().y + detail.as_ref().map_or(0.0, |d| 2.0 + d.size().y) + 2.0 * 14.0).max(ROW_LEAST);
            (label, detail, height)
        })
        .collect();
    let total: f32 = laid.iter().map(|l| l.2).sum();
    let (group, _) = ui.allocate_exact_size(vec2(width, total), Sense::hover());
    let mut shapes = Vec::new();
    glass::group(&mut shapes, &ctx, group, ROW_RADIUS, group_fill());
    ui.painter().extend(shapes);
    let mut pressed = None;
    let mut top = group.top();
    for (n, ((id, label, detail, busy), (label_g, detail_g, height))) in rows.iter().zip(laid).enumerate() {
        let rect = Rect::from_min_size(pos2(group.left(), top), vec2(width, height));
        top += height;
        if n > 0 {
            ui.painter().hline(rect.x_range().shrink(ROW_PAD), rect.top(), Stroke::new(1.0, shade::TINT_SEPARATOR.get()));
        }
        let response = ui.interact(rect, *id, Sense::click());
        let said = match detail {
            Some(detail) => format!("{label}, {}", detail.replace('\n', " ")),
            None => (*label).to_owned(),
        };
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &said));
        let (lit, down) = hover_takes_focus(ui, &response);
        let mut lift = Vec::new();
        lift_shapes(&mut lift, &ctx, rect, ROW_RADIUS, lit, down);
        ui.painter().extend(lift);
        let words_h = label_g.size().y + detail_g.as_ref().map_or(0.0, |d| 2.0 + d.size().y);
        let y = rect.center().y - words_h / 2.0;
        let label_h = label_g.size().y;
        ui.painter().galley(pos2(rect.left() + ROW_PAD, y), label_g, *ON_SURFACE);
        if let Some(detail_g) = detail_g {
            ui.painter().galley(pos2(rect.left() + ROW_PAD, y + label_h + 2.0), detail_g, *ON_SURFACE_VARIANT);
        }
        let end = pos2(rect.right() - ROW_PAD - 10.0, rect.center().y);
        if *busy {
            ui.scope_builder(UiBuilder::new().max_rect(Rect::from_center_size(end, Vec2::splat(20.0))), |ui| spinner(ui, 9.0));
        } else {
            let ink = Stroke::new(2.0, *ON_SURFACE_VARIANT);
            ui.painter().line_segment([end + vec2(-3.0, -6.0), end + vec2(3.0, 0.0)], ink);
            ui.painter().line_segment([end + vec2(3.0, 0.0), end + vec2(-3.0, 6.0)], ink);
        }
        focus_ring(ui, rect, &response, ROW_RADIUS as u8);
        if response.clicked() {
            pressed = Some(*id);
        }
    }
    pressed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_settings_line_says_size_rate_and_bitrate() {
        assert_eq!(settings_line((1920, 1080, 60, 20_000)), "1920 × 1080 at 60 fps, 20 Mbps");
        assert_eq!(settings_line((2560, 1440, 120, 12_500)), "2560 × 1440 at 120 fps, 12.5 Mbps");
    }

    #[test]
    fn the_curtain_says_what_android_says_in_every_line() {
        use crate::strings::{with_lang, Lang};
        for words in nightjar_policy::launch_curtain::ALL {
            assert_eq!(with_lang(Lang::En, || curtain_words(words)), words.en, "{}", words.key);
            assert_ne!(with_lang(Lang::De, || curtain_words(words)), words.en, "{} has no German", words.key);
        }
    }

    #[test]
    fn each_test_outcome_has_its_own_words() {
        let words: Vec<_> = [conn_test::Outcome::Clear, conn_test::Outcome::Blocked, conn_test::Outcome::Unknown].map(test_words).into();
        assert_eq!(words.len(), 3);
        assert!(words.iter().all(|w| !w.is_empty()));
        assert!(words[0] != words[1] && words[1] != words[2] && words[0] != words[2]);
    }
}
