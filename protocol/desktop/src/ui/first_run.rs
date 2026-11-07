//! The first run: the sky and the moon with its orbit, the one line of what is happening and one moon
//! action. A PC found rises in as a card with Pair as that action, and Add PC steps down.

use super::backdrop::{self, Moon};
use super::chrome::{capsule_width, control_capsule, play_button, play_width};
use super::design_tokens as t;
use super::pcs::{monitor, pc_action, pc_line_of, status_line};
use super::*;
use super::{glass, spring};
use egui::Shape;

/// Past this many PCs found it is a list, whatever their state.
const MOST_FOUND: usize = 3;
/// The ring the orbit dot runs on, in moon radii: the first of the sky's.
const ORBIT: f32 = backdrop::ORBIT;
const WORDS_GAP: f32 = 44.0;
const ADD_W: f32 = 220.0;
const ADD_H: f32 = 56.0;
const WORDS_TO_ADD: f32 = 28.0;
const CARD_W: f32 = 460.0;
const CARD_LEAST_W: f32 = 340.0;
const CARD_H: f32 = 196.0;
const CARD_PAD: f32 = 20.0;
const CARD_GAP: f32 = 16.0;
const FACE_W: f32 = 112.0;
const PAIR_H: f32 = 56.0;
/// A found card's top under the ring, and how far it rises in.
const CARD_UNDER_RING: f32 = 14.0;
const RISE: f32 = 28.0;
const RISE_TUNE: spring::Tune = spring::Tune {
    stiffness: t::MOTION_SHEET_STIFFNESS,
    ratio: t::MOTION_SHEET_RATIO,
};
const OVER_RINGS: f32 = 0.72;
const STILL_SEARCHING_GAP: f32 = 24.0;
const STEPPED_DOWN_GAP: f32 = 34.0;
const STEPPED_DOWN_H: f32 = 48.0;
const LINE_H: f32 = 20.0;
/// How to connect under Add PC.
const LINK_GAP: f32 = 8.0;
const LINK_H: f32 = 32.0;
const TIGHT_UNDER_MOON: f32 = 16.0;
const TIGHT_GAP: f32 = 10.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum FirstRun {
    Searching,
    Found,
}

impl FirstRun {
    pub(super) fn moon(self) -> Moon {
        match self {
            Self::Searching => Moon::Searching,
            Self::Found => Moon::Found,
        }
    }
}

/// Which first run this list of PCs is in, if any: none yet, or a few found with none of them paired.
/// A PC that has gone away is a list again, with its way to wake or check it.
pub(super) fn first_run_of(hosts: &Hosts) -> Option<FirstRun> {
    if hosts.list.is_empty() {
        return Some(FirstRun::Searching);
    }
    let found = hosts.list.len() <= MOST_FOUND
        && hosts
            .list
            .iter()
            .all(|h| !h.paired() && h.reach != Reach::Offline);
    found.then_some(FirstRun::Found)
}

/// From 1 to 0 as a card arrives. Reduced motion: it is simply there.
fn arrive(ctx: &egui::Context, id: Id) -> f32 {
    if !spring::motion(ctx) {
        return 0.0;
    }
    let seen = id.with("seen");
    if !ctx.data(|d| d.get_temp::<bool>(seen)).unwrap_or(false) {
        ctx.data_mut(|d| d.insert_temp(seen, true));
        spring::animate(ctx, id, 1.0, RISE_TUNE);
    }
    spring::animate(ctx, id, 0.0, RISE_TUNE)
}

/// Where the first run's own column of cards goes in `width`: how many on a line, and how wide.
fn found_grid(count: usize, width: f32) -> (usize, f32) {
    let fit = ((width + CARD_GAP) / (CARD_LEAST_W + CARD_GAP)).floor() as usize;
    let columns = fit.clamp(1, count.max(1));
    let card = ((width - CARD_GAP * (columns as f32 - 1.0)) / columns as f32).min(CARD_W);
    (columns, card)
}

impl Launcher {
    pub(super) fn first_run(
        &mut self,
        ui: &mut Ui,
        view: &View<'_>,
        state: FirstRun,
        commands: &mut Vec<Command>,
    ) {
        // A PC has just been found: its Pair takes the focus from Add PC.
        let found = state == FirstRun::Found;
        if found && !self.found_shown {
            self.focus_screen = true;
        }
        self.found_shown = found;
        let foot = match state {
            FirstRun::Searching => self.first_run_searching(ui, view, commands),
            FirstRun::Found => self.first_run_found(ui, view, commands),
        };
        let page = ui.max_rect();
        ui.advance_cursor_after_rect(Rect::from_min_max(
            ui.cursor().min,
            pos2(page.right(), foot + space::M),
        ));
    }

    /// What is happening, in as few words as say so.
    fn search_words(&self, view: &View<'_>) -> (&'static str, bool) {
        if !self.identity_ready {
            (tr("Creating this device's pairing key…"), true)
        } else if view.hosts.discovering {
            (tr("Searching for PCs…"), true)
        } else {
            (tr("Can't search this network for PCs"), false)
        }
    }

    /// How to connect, centred at `top`, as Android's search card has it. Returns where it ends.
    fn first_run_link(&mut self, ui: &mut Ui, top: f32, commands: &mut Vec<Command>) -> f32 {
        let page = ui.max_rect();
        let room = Rect::from_min_max(pos2(page.left(), top), pos2(page.right(), top + LINK_H));
        let help = ui
            .scope_builder(
                UiBuilder::new()
                    .max_rect(room)
                    .layout(Layout::top_down(Align::Center)),
                |ui| how_to_connect(ui, commands),
            )
            .inner;
        self.focus_item(&help, "how-to-connect", false);
        room.bottom()
    }

    fn first_run_searching(
        &mut self,
        ui: &mut Ui,
        view: &View<'_>,
        commands: &mut Vec<Command>,
    ) -> f32 {
        let (center, r) = backdrop::moon_disc(ui.ctx().content_rect(), Moon::Searching);
        let dy = -self.page_scrolled;
        let (words, fine) = self.search_words(view);
        let ink = if fine { *ON_SURFACE } else { *ERROR };
        let middle = center.x;
        let head = ui.painter().layout(
            words.to_owned(),
            semibold(t::TYPE_TITLE_SIZE),
            ink,
            ui.max_rect().width(),
        );
        let head_at = pos2(
            middle - head.size().x / 2.0,
            center.y + r * ORBIT + WORDS_GAP + dy,
        );
        let head_h = head.size().y;
        ui.painter().galley(head_at, head, ink);
        let label = tr("Add PC");
        let w = play_width(ui, label).max(ADD_W);
        let rect = Rect::from_min_size(
            pos2(middle - w / 2.0, head_at.y + head_h + WORDS_TO_ADD),
            vec2(w, ADD_H),
        );
        let add = play_button(
            ui,
            rect,
            Id::unique("first-run-add"),
            Icon::Plus,
            label,
            true,
            false,
        );
        self.focus_item(&add, "first-run-add", true);
        if add.clicked() {
            self.add_host_dialog();
        }
        self.first_run_link(ui, rect.bottom() + LINK_GAP, commands)
    }

    fn first_run_found(
        &mut self,
        ui: &mut Ui,
        view: &View<'_>,
        commands: &mut Vec<Command>,
    ) -> f32 {
        let (center, r) = backdrop::moon_disc(ui.ctx().content_rect(), Moon::Found);
        let page = ui.max_rect();
        let middle = page.center().x;
        let list = &view.hosts.list;
        let (columns, card_w) = found_grid(list.len(), page.width());
        let lines = list.len().div_ceil(columns) as f32;
        let cards_h = lines * (CARD_H + CARD_GAP) - CARD_GAP;
        let under = center.y + r * ORBIT + CARD_UNDER_RING;
        let needs = under
            + cards_h
            + STILL_SEARCHING_GAP
            + LINE_H
            + STEPPED_DOWN_GAP
            + STEPPED_DOWN_H
            + LINK_GAP
            + LINK_H;
        // A short window keeps it all in view: the card comes over the ring, the gaps close up.
        let tight = needs + space::M > ui.ctx().content_rect().bottom();
        let (card_top, still_gap, step_gap) = if tight {
            (center.y + r + TIGHT_UNDER_MOON, TIGHT_GAP, TIGHT_GAP)
        } else {
            (under, STILL_SEARCHING_GAP, STEPPED_DOWN_GAP)
        };
        let top = card_top - self.page_scrolled;
        let mut y = top;
        for (row, chunk) in list.chunks(columns).enumerate() {
            let line_w = chunk.len() as f32 * (card_w + CARD_GAP) - CARD_GAP;
            let left = middle - line_w / 2.0;
            for (n, host) in chunk.iter().enumerate() {
                let slot = Rect::from_min_size(
                    pos2(left + n as f32 * (card_w + CARD_GAP), y),
                    vec2(card_w, CARD_H),
                );
                self.found_card(ui, host, slot, row == 0 && n == 0, commands);
            }
            y += CARD_H + CARD_GAP;
        }
        y += still_gap - CARD_GAP;
        // The search goes on, under what it found.
        let (words, fine) = self.search_words(view);
        let ink = if fine { *ON_SURFACE_VARIANT } else { *ERROR };
        let line = ui
            .painter()
            .layout_no_wrap(words.to_owned(), regular(t::TYPE_SUB_SIZE), ink);
        let line_w = 8.0 + 8.0 + line.size().x;
        let line_h = line.size().y;
        let dot = pos2(middle - line_w / 2.0 + 4.0, y + line_h / 2.0);
        ui.painter().circle_filled(dot, 4.0, t::COLOR_OFFLINE);
        ui.painter().galley(pos2(dot.x + 12.0, y), line, ink);
        y += line_h + step_gap;
        let label = tr("Add PC");
        let w = capsule_width(ui, label);
        let rect = Rect::from_min_size(pos2(middle - w / 2.0, y), vec2(w, STEPPED_DOWN_H));
        let add = control_capsule(
            ui,
            rect,
            Id::unique("first-run-add-small"),
            Icon::Plus,
            label,
            label,
            true,
        );
        self.focus_item(&add, "first-run-add", false);
        if add.clicked() {
            self.add_host_dialog();
        }
        self.first_run_link(ui, rect.bottom() + LINK_GAP, commands)
    }

    /// A PC found: its screen, its name and its line, and Pair.
    fn found_card(
        &mut self,
        ui: &mut Ui,
        host: &Host,
        slot: Rect,
        first: bool,
        commands: &mut Vec<Command>,
    ) {
        let ctx = ui.ctx().clone();
        let id = Id::unique(("found-card", host.id));
        let card = ui.interact(slot, id, Sense::CLICK);
        let name = host.display_name();
        let line = pc_line_of(host);
        let said = format!("{name}, {}", line.words());
        card.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, said.clone()));
        let rising = arrive(&ctx, id.with("arrive"));
        let rect = slot.translate(vec2(0.0, RISE * rising));
        let inner = rect.shrink(CARD_PAD);
        let face = Rect::from_min_size(inner.min, vec2(FACE_W, (0.62 * FACE_W).round()));
        let words_left = face.right() + 16.0;
        let room = inner.right() - words_left;
        let action = pc_action(host);
        let words = action.words();
        let pair_rect = Rect::from_min_max(
            pos2(inner.left(), inner.bottom() - PAIR_H),
            inner.right_bottom(),
        );
        let mut pair = None;
        ui.scope(|ui| {
            ui.multiply_opacity((1.0 - rising).clamp(0.0, 1.0));
            let mut out = Vec::new();
            let r = t::RADIUS_PANEL;
            if !high_contrast() {
                glass::solid(
                    &mut out,
                    &ctx,
                    rect,
                    r,
                    t::COLOR_CANVAS.gamma_multiply(OVER_RINGS),
                );
            }
            glass::group(&mut out, &ctx, rect, r, group_fill());
            monitor(ui, &mut out, host, face, id.with("art-in"));
            let mut job = egui::text::LayoutJob::simple_singleline(
                name.to_owned(),
                semibold(t::TYPE_SECTION_SIZE),
                *ON_SURFACE,
            );
            job.wrap = egui::text::TextWrapping::truncate_at_width(room);
            let galley = ui.painter().layout_job(job);
            let name_h = galley.size().y;
            let block = (name_h + 4.0 + 20.0) / 2.0;
            let name_top = face.center().y - block;
            out.push(Shape::galley(
                pos2(words_left, name_top),
                galley,
                *ON_SURFACE,
            ));
            status_line(
                ui,
                &mut out,
                pos2(words_left, name_top + name_h + 4.0 + 10.0),
                room,
                &line,
            );
            ui.painter().extend(out);
            let icon = action.icon().unwrap_or(Icon::Lock);
            pair = Some(play_button(
                ui,
                pair_rect,
                id.with("pair"),
                icon,
                words,
                true,
                false,
            ));
        });
        let pair = pair.expect("the scope ran");
        self.focus_item(&pair, &format!("found:{}", host.id), first);
        if pair.clicked() {
            commands.push(action.command(host.id));
        } else if card.clicked() {
            host_clicked(host, self.settings.quick_resume, commands);
        }
        // Pair holds the focus for its card: its keys, its word, and the options by the card.
        let keys = menu_request_going(ui, &pair, None, action.words()).map(|_| Anchor::Beside(card.rect));
        let asked = menu_request(ui, &card, None).or(keys);
        if let Some(anchor) = asked {
            self.open_menu(
                Dialog::HostOptions { host: host.id },
                anchor,
                format!("found:{}", host.id),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_few_found_cards_share_a_line_and_a_narrow_window_stacks_them() {
        assert_eq!(found_grid(1, 1184.0), (1, CARD_W));
        let (columns, w) = found_grid(3, 1184.0);
        assert_eq!(columns, 3);
        assert!(
            w * 3.0 + 2.0 * CARD_GAP <= 1184.0 && w >= CARD_LEAST_W,
            "{w}"
        );
        let (columns, w) = found_grid(2, 660.0);
        assert_eq!(columns, 1);
        assert_eq!(w, CARD_W);
    }
}
