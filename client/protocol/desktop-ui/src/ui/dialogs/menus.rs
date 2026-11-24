//! Popover menus, PC switcher sheet, game menu builders, PIN pad, and dialog layout helpers.

use super::*;

/// A game's menu: its title and what it offers, the action selecting it does first. None when the
/// game is gone from the PC. A favourite whose PC's library is not known (away with nothing saved,
/// not read yet, or the game gone) still has its menu, from the stored star: its card is on Home.
pub(in crate::ui) fn game_menu(host: &Host, view: &View<'_>, game: GameTarget, favourites: &[crate::config::Favourite], hidden: &[crate::config::Favourite]) -> Option<(String, Vec<GameAction>)> {
    let _ = view;
    match game {
        GameTarget::Library { app, favourite } => {
            let listed = host.library().and_then(|l| l.games.iter().find(|g| g.app_id == app));
            let (title, starred) = match listed {
                Some(entry) => {
                    let sources: Vec<Source> = std::iter::once(Source::of(host)).collect();
                    let star = console::favourite_for(&sources, host.id, app);
                    (entry.title.clone(), star.map(|star| favourites.iter().any(|f| f.same_game(&star))))
                }
                None if favourite => (stored_favourite(host, app, favourites)?.title.clone(), Some(true)),
                None => return None,
            };
            let mut actions = vec![GameAction::Open];
            if let Some(starred) = starred {
                actions.push(if starred { GameAction::Unstar } else { GameAction::Star });
            }
            if let Some(entry) = listed.filter(|_| !host.record.unique_id.is_empty()) {
                let hides = console::is_hidden(&Source::of(host), &entry.install_id, app, hidden);
                actions.push(if hides { GameAction::Unhide } else { GameAction::Hide });
            }
            if classic_of(host, game, favourites).is_some() {
                actions.push(GameAction::Settings);
            }
            if host.paired() {
                actions.push(GameAction::Library);
            }
            Some((title, actions))
        }
        GameTarget::Classic { app } => {
            let Apps::Loaded(apps) = &host.apps else { return None };
            let entry = apps.iter().find(|a| a.id == app)?;
            let running = host.running_app().is_some_and(|r| entry.matches_running_app(r, None));
            let mut actions = vec![if running { GameAction::Resume } else { GameAction::Play }];
            if !entry.is_control_action() {
                actions.push(GameAction::Settings);
                if !host.record.unique_id.is_empty() {
                    let hides = console::is_hidden(&Source::of(host), &entry.uuid, entry.id as u32, hidden);
                    actions.push(if hides { GameAction::Unhide } else { GameAction::Hide });
                }
            }
            Some((entry.name.clone(), actions))
        }
    }
}

/// The game a menu is about, as "When Nightjar opens" keeps it; None for what is not a game (a
/// control action) or a PC without a unique id to find it by again.
pub(in crate::ui) fn open_game_of(host: &Host, game: GameTarget, favourites: &[crate::config::Favourite]) -> Option<crate::config::OpenGame> {
    let (install_id, app_id, title) = match game {
        GameTarget::Library { app, .. } => match host.library().and_then(|l| l.games.iter().find(|g| g.app_id == app)) {
            Some(entry) => (entry.install_id.clone(), app, entry.title.clone()),
            None => {
                let star = stored_favourite(host, app, favourites)?;
                (star.install_id.clone(), app, star.title.clone())
            }
        },
        GameTarget::Classic { app } => {
            let Apps::Loaded(apps) = &host.apps else { return None };
            let entry = apps.iter().find(|a| a.id == app).filter(|a| !a.is_control_action())?;
            (entry.uuid.clone(), entry.id as u32, entry.name.clone())
        }
    };
    let unique_id = host.record.unique_id.clone();
    (!unique_id.is_empty()).then_some(crate::config::OpenGame { host: unique_id, install_id, app_id, title })
}

/// The game menu's row for "When Nightjar opens": take it back on the chosen game, else choose it.
pub(in crate::ui) fn open_on_start_action(host: &Host, game: GameTarget, favourites: &[crate::config::Favourite], chosen: Option<&crate::config::OpenGame>) -> Option<GameAction> {
    let this = open_game_of(host, game, favourites)?;
    Some(if chosen.is_some_and(|c| c.same_game(&this)) { GameAction::StopOpenOnStart } else { GameAction::OpenOnStart })
}

/// What hiding a game menu's game stores: every copy of a library title, or the one classic entry.
pub(in crate::ui) fn hide_keys_of(host: &Host, game: GameTarget, sources: &[Source<'_>]) -> Vec<crate::config::Favourite> {
    match game {
        GameTarget::Library { app, .. } => console::hide_keys(sources, host.id, app),
        GameTarget::Classic { app } => {
            let Apps::Loaded(apps) = &host.apps else { return Vec::new() };
            apps.iter()
                .find(|a| a.id == app)
                .map(|a| crate::config::Favourite { host: host.record.unique_id.clone(), install_id: a.uuid.clone(), app_id: a.id as u32, title: a.name.clone() })
                .into_iter()
                .collect()
        }
    }
}

/// The classic app entry a game menu's game starts through, once the PC's app list is loaded. A
/// game its library does not list is found by its stored star, if it has one.
pub(in crate::ui) fn classic_of<'a>(host: &'a Host, game: GameTarget, favourites: &[crate::config::Favourite]) -> Option<&'a nightjar_policy::http::App> {
    let Apps::Loaded(apps) = &host.apps else { return None };
    match game {
        GameTarget::Classic { app } => apps.iter().find(|a| a.id == app),
        GameTarget::Library { app, .. } => {
            let install_id = match host.library().and_then(|l| l.games.iter().find(|g| g.app_id == app)) {
                Some(entry) => entry.install_id.as_str(),
                None => stored_favourite(host, app, favourites)?.install_id.as_str(),
            };
            let pairs: Vec<(i32, &str)> = apps.iter().map(|a| (a.id, a.uuid.as_str())).collect();
            console::classic_app(&pairs, install_id, app).and_then(|id| apps.iter().find(|a| a.id == id))
        }
    }
}

/// The star stored for game `app` on `host`, matched as Home's Favourites row matches a star
/// whose library is not known: by the PC's unique id and the saved app id.
pub(super) fn stored_favourite<'a>(host: &Host, app: u32, favourites: &'a [crate::config::Favourite]) -> Option<&'a crate::config::Favourite> {
    let unique_id = host.record.unique_id.as_str();
    favourites.iter().find(|f| !unique_id.is_empty() && f.host == unique_id && f.app_id == app)
}

/// "Waking <PC>…" while the wake packets go out, with a way to stop.
pub(in crate::ui) fn waking(ctx: &egui::Context, host: &str, commands: &mut Vec<Command>) {
    modal(ctx, "waking", |ui| {
        ui.set_width(400.0);
        ui.label(RichText::new(trf("Waking {}…", &[&host])).font(semibold(text::SECTION)));
        ui.add_space(space::M);
        ui.horizontal(|ui| {
            spinner(ui, 10.0);
            ui.label(RichText::new(tr("Sending Wake-on-LAN until the PC answers.")).color(ON_SURFACE));
        });
        dialog_buttons(ui, |ui| {
            let r = quiet_button(ui, "Cancel");
            r.request_focus();
            if r.clicked() {
                commands.push(Command::CancelWake);
            }
        });
    });
}

/// A dialog's buttons, the same on every dialog: at its foot, aligned right, the one primary
/// action at the right edge and the other choices left of it, Cancel leftmost (Android's order).
/// Add them primary first: the layout runs right to left. Escape or B cancels any dialog.
pub(in crate::ui) fn dialog_buttons(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    ui.add_space(space::L);
    // As tall as a button: a row with no height of its own would centre them in what's left below.
    ui.allocate_ui_with_layout(vec2(ui.available_width(), Size::CONTROL.height), Layout::right_to_left(Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = space::S;
        add(ui);
    });
}

/// While the PC is asked, a spinner and "Asking the PC…" at the left of a dialog's buttons (added
/// after them, in their right-to-left row); it goes without moving them once the answer is in.
pub(super) fn asking_line(ui: &mut Ui, waiting: bool) {
    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
        if waiting {
            ui.spacing_mut().item_spacing.x = space::S;
            spinner(ui, 9.0);
            ui.add(egui::Label::new(RichText::new(tr("Asking the PC…")).color(ON_SURFACE_VARIANT)).truncate());
        }
    });
}

/// A dialog's one-line text field: full width, with room around its text.
pub(in crate::ui) fn dialog_field(ui: &mut Ui, edit: egui::TextEdit<'_>) -> Response {
    ui.add(edit.desired_width(f32::INFINITY).margin(Margin::symmetric(12, 10)))
}

/// The dialog for one waiting device.
pub(in crate::ui) fn approve_dialog(host: &Host, request: &PairingRequest) -> Dialog {
    Dialog::ApprovePairing {
        host: host.id,
        request: request.id.clone(),
        name: short_device_name(&request.name),
        address: request.address.clone(),
        pc: host.display_name().to_owned(),
        pin: String::new(),
        busy: false,
    }
}

/// A device names itself, at any length; a row stays one row.
pub(in crate::ui) fn short_device_name(name: &str) -> String {
    const MOST: usize = 32;
    if name.chars().count() <= MOST { name.to_owned() } else { name.chars().take(MOST).chain(['…']).collect() }
}

/// Text with a name in it that can be any length: two rows at most, so what follows stays in the window.
pub(super) fn two_rows(ui: &mut Ui, shown: String, font_id: egui::FontId) -> Response {
    let mut job = egui::text::LayoutJob::single_section(shown, egui::TextFormat { font_id, color: ui.visuals().text_color(), ..Default::default() });
    job.wrap = egui::text::TextWrapping { max_width: ui.available_width(), max_rows: 2, break_anywhere: false, overflow_character: Some('…') };
    ui.label(job)
}

/// How many digits a pairing PIN has.
pub(super) const PIN_LENGTH: usize = 4;

/// Too short a window for a PIN's slots and keys at their full height, with a two-row title.
pub(super) fn short_window(ctx: &egui::Context) -> bool {
    ctx.content_rect().height() < 600.0
}

/// The digit pad of a PIN, for a controller or the mouse; the keyboard's digits work beside it.
/// True when this frame's digit filled the PIN.
pub(super) fn pin_pad(ui: &mut Ui, pin: &mut String, focus_first: bool, focused: &mut bool) -> bool {
    let short = short_window(ui.ctx());
    let gap = if short { 6.0 } else { 12.0 };
    let key_size = vec2(100.0, if short { 44.0 } else { 56.0 });
    let before = pin.len();
    let enabled = ui.is_enabled();
    if enabled {
        let (typed, erase) = ui.input(|i| {
            let typed: String = i.events.iter().filter_map(|e| if let egui::Event::Text(t) = e { Some(t.as_str()) } else { None }).collect();
            (typed, i.key_pressed(Key::Backspace))
        });
        pin.extend(typed.chars().filter(char::is_ascii_digit));
        if erase {
            pin.pop();
        }
    }
    let digit_key = |ui: &mut Ui, digit: char, pin: &mut String, focused: &mut bool| {
        let word = digit.to_string();
        let key = pin_key(ui, None, &word, &word, Tone::Quiet, key_size);
        if focus_first && digit == '1' {
            key.request_focus();
            *focused = true;
        }
        if key.clicked() {
            pin.push(digit);
        }
    };
    // In the middle of the dialog, as its slots are.
    let indent = ((ui.available_width() - key_size.x * 3.0 - gap * 2.0) / 2.0).max(0.0);
    ui.spacing_mut().item_spacing = vec2(gap, gap);
    for digits in [['1', '2', '3'], ['4', '5', '6'], ['7', '8', '9']] {
        ui.horizontal(|ui| {
            ui.add_space(indent);
            for digit in digits {
                digit_key(ui, digit, pin, focused);
            }
        });
    }
    ui.horizontal(|ui| {
        ui.add_space(indent + key_size.x + gap);
        digit_key(ui, '0', pin, focused);
        let said = tr("Delete digit");
        if pin_key(ui, Some(Icon::Backspace), "", said, Tone::Quiet, key_size).on_hover_text(said).clicked() {
            pin.pop();
        }
    });
    pin.truncate(PIN_LENGTH);
    before < PIN_LENGTH && pin.len() == PIN_LENGTH
}

/// Where a popover menu opens.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Anchor {
    /// Under the ⋮ that opened it, right edges lined up (above it when there is no room below).
    Below(Rect),
    /// Beside the focused card it belongs to (F2, Shift+F10, Y or X), at its top.
    Beside(Rect),
    /// At the pointer of a right-click.
    Point(Pos2),
    /// Nothing to point at: in the middle of the window, over a dimmed page.
    Center,
}

/// Where a menu of `size` goes for `anchor`, kept inside `screen` with a margin: flipped above or
/// to the other side where it would not fit, then pushed in.
pub fn place(anchor: Anchor, size: Vec2, screen: Rect) -> Pos2 {
    const GAP: f32 = 6.0;
    const EDGE: f32 = 8.0;
    let fits_below = |y: f32| y + size.y <= screen.bottom() - EDGE;
    let fits_right = |x: f32| x + size.x <= screen.right() - EDGE;
    let (x, y) = match anchor {
        Anchor::Below(r) => (r.right() - size.x, if fits_below(r.bottom() + GAP) { r.bottom() + GAP } else { r.top() - GAP - size.y }),
        Anchor::Beside(r) => (if fits_right(r.right() + GAP) { r.right() + GAP } else { r.left() - GAP - size.x }, r.top()),
        Anchor::Point(p) => (if fits_right(p.x) { p.x } else { p.x - size.x }, if fits_below(p.y) { p.y } else { p.y - size.y }),
        Anchor::Center => (screen.center().x - size.x / 2.0, screen.center().y - size.y / 2.0),
    };
    let x = x.min(screen.right() - EDGE - size.x).max(screen.left() + EDGE);
    let y = y.min(screen.bottom() - EDGE - size.y).max(screen.top() + EDGE);
    Pos2::new(x, y)
}

/// The choice after `now` in `all`, back to the first after the last.
pub(super) fn next_of<T: Copy + PartialEq>(all: &[T], now: T) -> T {
    let at = all.iter().position(|v| *v == now).map_or(0, |i| (i + 1) % all.len());
    all[at]
}

/// One row of a popover menu.
pub struct MenuRow {
    pub icon: Icon,
    /// The English label (translated when drawn).
    pub label: &'static str,
    /// The key that does the same from the card, shown at the right.
    pub hint: Option<&'static str>,
    /// Destructive rows sit last, under a divider, in the error colour.
    pub destructive: bool,
    /// Why the row cannot be chosen now; shown under its label.
    pub blocked: Option<&'static str>,
}

/// What a menu did this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuOutcome {
    Open,
    Chosen(usize),
    /// A press outside it: it closes, as any menu does.
    Dismissed,
}

/// A popover menu: a raised panel by its anchor with a heading (what it is for) and rows of icon,
/// label and shortcut. It takes the keyboard and the pointer while open but leaves the page as it
/// is (no dimming), and scrolls with faded edges in a window too short for it.
#[allow(clippy::too_many_arguments)]
pub fn popover(
    ctx: &egui::Context,
    name: &str,
    anchor: Anchor,
    title: &str,
    subtitle: Option<(String, Color32)>,
    rows: &[MenuRow],
    note: Option<&str>,
    focus_first: bool,
) -> (MenuOutcome, bool) {
    let screen = ctx.content_rect();
    let mut outcome = MenuOutcome::Open;
    let mut focused = false;
    let dismissed = menu_shell(ctx, name, anchor, |ui, id| {
        // The rows' widths: the longest label and hint, at least a comfortable width.
        let measure = |text: &str, font: egui::FontId| ui.painter().layout_no_wrap(text.to_owned(), font, Color32::WHITE).size().x;
        let widest = rows
            .iter()
            .map(|r| 12.0 + 18.0 + 12.0 + measure(tr(r.label), semibold(text::LABEL)) + r.hint.map_or(0.0, |h| 24.0 + measure(tr(h), regular(text::CAPTION))) + 14.0)
            .fold(260.0_f32, f32::max);
        let title_w = measure(title, semibold(text::BODY)) + 24.0;
        let width = widest.max(title_w.min(380.0)).min(screen.width() - 32.0);
        ui.set_width(width);
        // egui lays a new area out in at most 600x400 points: let the menu take the window's
        // height less its margin instead, so it scrolls only when the window is too short.
        ui.set_max_height(menu_room(screen, anchor).height() - 16.0 - 10.0);
        ui.spacing_mut().item_spacing.y = 4.0;
        let top = ui.cursor().top();
        // The heading: what the menu is for.
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                let mut job = egui::text::LayoutJob::simple_singleline(title.to_owned(), semibold(text::BODY), *ON_SURFACE);
                job.wrap = egui::text::TextWrapping::truncate_at_width(width - 20.0);
                ui.label(job);
                if let Some((line, color)) = &subtitle {
                    ui.label(RichText::new(line).font(regular(text::CAPTION)).color(*color));
                }
            });
        });
        ui.add_space(4.0);
        divider(ui);
        let note = note.map(|n| {
            let text = RichText::new(tr(n)).font(regular(text::CAPTION)).color(OUTLINE);
            egui::WidgetText::from(text).into_galley(ui, Some(egui::TextWrapMode::Wrap), width - 20.0, egui::TextStyle::Body)
        });
        let below = note.as_ref().map_or(0.0, |n| 10.0 + n.size().y);
        // The window's height less its margins, the frame, the heading, the list's own margin and
        // the note: wherever the menu is placed, it then fits inside the window.
        let heading = ui.cursor().top() - top;
        let room = (screen.height() - 16.0 - 10.0 - heading - 8.0 - 4.0 - below).max(CONTROL_HEIGHT);
        menu_list(ui, id, room, |ui| {
            let mut divided = false;
            for (n, row) in rows.iter().enumerate() {
                if row.destructive && !divided && n > 0 {
                    divider(ui);
                }
                divided |= row.destructive;
                let response = ui.add_enabled_ui(row.blocked.is_none(), |ui| menu_row(ui, row)).inner;
                if focus_first && !focused && row.blocked.is_none() {
                    response.request_focus();
                    focused = true;
                }
                if response.clicked() {
                    outcome = MenuOutcome::Chosen(n);
                }
            }
        });
        if let Some(note) = note {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.add_space(10.0);
                ui.label(note);
            });
            ui.add_space(4.0);
        }
    });
    if dismissed && outcome == MenuOutcome::Open {
        outcome = MenuOutcome::Dismissed;
    }
    (outcome, focused)
}

/// What every menu stands on: a clear layer that takes the pointer and, by its anchor, the raised
/// glass that holds `add`. Says whether a press outside closed it.
/// Where a menu by its anchor may go: below the top bar, which it never covers. A sheet in the
/// middle takes the whole window.
fn menu_room(screen: Rect, anchor: Anchor) -> Rect {
    let clear = if anchor == Anchor::Center { 0.0 } else { chrome::BAR_TOP + chrome::BAR_H };
    Rect::from_min_max(Pos2::new(screen.left(), screen.top() + clear), screen.max)
}

fn menu_shell(ctx: &egui::Context, name: &str, anchor: Anchor, add: impl FnOnce(&mut Ui, Id)) -> bool {
    let id = Id::unique(("menu", name));
    let screen = ctx.content_rect();
    let last = ctx.memory(|m| m.area_rect(id));
    let size = last.map_or(vec2(280.0, 320.0), |r| r.size());
    // A clear layer between the page and the menu takes the pointer, so the page under the menu
    // neither lights up nor takes the click that closes it. With nothing to point at, it dims
    // the page as a dialog does.
    let catch = egui::Area::new(id.with("backdrop"))
        .order(egui::Order::Middle)
        .fixed_pos(screen.min)
        .show(ctx, |ui| {
            if anchor == Anchor::Center {
                // A sheet in the middle blurs the page as a dialog does.
                glass::page_behind(ui.ctx(), ui.opacity());
                ui.painter().rect_filled(screen, 0.0, Color32::from_black_alpha(150));
            }
            let catch = ui.allocate_rect(screen, Sense::click());
            catch.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, tr("Close menu")));
            catch
        })
        .inner;
    let frame = Frame::new().inner_margin(Margin::same(MENU_MARGIN as i8));
    let area = egui::Area::new(id).order(egui::Order::Foreground).fixed_pos(place(anchor, size, menu_room(screen, anchor))).constrain(false).show(ctx, |ui| {
        frame.show(ui, |ui| glass_behind(ui, glass::Material::Panel, design_tokens::RADIUS_GROUP, MENU_MARGIN, |ui| add(ui, id)));
    });
    // The menu holds the keyboard while open; a click outside closes it (on the release, so
    // nothing under it takes that click).
    ctx.memory_mut(|m| m.set_modal_layer(area.response.layer_id));
    catch.clicked() || catch.secondary_clicked()
}

/// A menu's rows, `room` tall at most, scrolling with faded edges and keeping the focus in view.
fn menu_list(ui: &mut Ui, id: Id, room: f32, add: impl FnOnce(&mut Ui)) {
    // A thin bar that always shows when the list scrolls.
    ui.spacing_mut().scroll = egui::style::ScrollStyle { foreground_color: true, ..egui::style::ScrollStyle::thin() };
    let list = egui::ScrollArea::vertical().id_salt(id.with("rows")).max_height(room).content_margin(Margin::symmetric(MENU_RING as i8, (2.0 * MENU_RING) as i8)).show(ui, |ui| {
        let follow = id.with("focus");
        let mut last = ui.ctx().data(|d| d.get_temp::<Option<Id>>(follow)).flatten();
        crate::gui::follow_focus(ui, &mut last);
        ui.ctx().data_mut(|d| d.insert_temp(follow, last));
        let ctx = ui.ctx().clone();
        glass::with_halo_reach(&ctx, MENU_RING, || add(ui));
    });
    scroll_fades(ui, &list, MENU_FADE);
}

/// What a focused row's ring takes around it in a menu, and what its glow takes past that: the list
/// keeps room for both, so neither ends in a hard line at the list's edge.
const MENU_RING: f32 = design_tokens::FOCUS_GAP + design_tokens::FOCUS_RING_WIDTH;

/// The PCs behind the top bar's capsule: a row for each with its status dot, its name and what it
/// is doing, and a mark on `current`, the one the capsule shows. `focus_first` puts the ring on the
/// first row; opened by the pointer there is no ring until a key asks for one.
pub fn pc_sheet(ctx: &egui::Context, anchor: Anchor, hosts: &[Host], current: u64, focus_first: bool) -> (MenuOutcome, bool) {
    let screen = ctx.content_rect();
    let mut outcome = MenuOutcome::Open;
    let mut focused = false;
    let dismissed = menu_shell(ctx, "pc-sheet", anchor, |ui, id| {
        let measure = |text: &str, font: egui::FontId| ui.painter().layout_no_wrap(text.to_owned(), font, Color32::WHITE).size().x;
        let widest = hosts
            .iter()
            .map(|h| measure(h.display_name(), semibold(text::LABEL)).max(measure(pcs::pc_line_of(h).words(), regular(text::CAPTION))))
            .fold(0.0_f32, f32::max);
        let width = (PC_ROW_WORDS + widest + PC_ROW_MARK + 8.0).clamp(280.0, 420.0).min(screen.width() - 32.0);
        ui.set_width(width);
        ui.set_max_height(menu_room(screen, anchor).height() - 16.0 - 10.0);
        ui.spacing_mut().item_spacing.y = 4.0;
        let top = ui.cursor().top();
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.label(RichText::new(tr("PCs")).font(semibold(text::BODY)).color(*ON_SURFACE));
        });
        ui.add_space(4.0);
        divider(ui);
        let heading = ui.cursor().top() - top;
        let room = (screen.height() - 16.0 - 10.0 - heading - 8.0 - 4.0).max(CONTROL_HEIGHT);
        menu_list(ui, id, room, |ui| {
            let rows: Vec<Response> = hosts.iter().map(|host| pc_row(ui, host, host.id == current)).collect();
            let asked = ui.input(|i| [Key::ArrowDown, Key::ArrowUp, Key::Tab].iter().any(|key| i.key_pressed(*key)));
            let by_key = asked && !rows.iter().any(Response::has_focus);
            if let Some(first) = rows.first().filter(|_| focus_first || by_key) {
                first.request_focus();
                focused = true;
                // That key's move ends here, on the first row, not one further.
                ui.memory_mut(|m| m.move_focus(egui::FocusDirection::None));
            }
            if let Some(n) = rows.iter().position(Response::clicked) {
                outcome = MenuOutcome::Chosen(n);
            }
        });
    });
    if dismissed && outcome == MenuOutcome::Open {
        outcome = MenuOutcome::Dismissed;
    }
    (outcome, focused)
}

/// A menu row's state: the pointer moving onto it takes the focus there, and only the focused row
/// lights, so one highlight shows whether the pointer or the keys moved last (a resting pointer no
/// longer lights a second row beside the focus). Lit and pressed; neither for a row that is off.
pub(in crate::ui) fn hover_takes_focus(ui: &Ui, response: &Response) -> (bool, bool) {
    if !ui.is_enabled() {
        return (false, false);
    }
    let moved_in = response.hovered() && ui.input(|i| i.pointer.is_moving());
    if moved_in && !response.has_focus() {
        response.request_focus();
    }
    (response.has_focus() || moved_in, response.is_pointer_button_down_on())
}

/// Where a PC row's words start.
const PC_ROW_WORDS: f32 = 14.0 + 9.0 + 12.0;
/// What a PC row keeps free at its right for the mark.
const PC_ROW_MARK: f32 = 8.0 + 20.0 + 14.0;

fn pc_row(ui: &mut Ui, host: &Host, current: bool) -> Response {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(vec2(width, 60.0), Sense::click());
    let (lit, pressed) = hover_takes_focus(ui, &response);
    let line = pcs::pc_line_of(host);
    let (name, status) = (host.display_name(), line.words());
    response.widget_info(|| {
        let mut info = egui::WidgetInfo::labeled(egui::Role::Button, true, format!("{name}, {status}"));
        info.selected = Some(current);
        info
    });
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let mut shapes = Vec::new();
    lift_shapes(&mut shapes, ui.ctx(), rect, 14.0, lit, pressed);
    chrome::pc_dot(&mut shapes, ui.ctx(), Pos2::new(rect.left() + 14.0 + 4.5, rect.center().y), host);
    ui.painter().extend(shapes);
    let room = width - PC_ROW_WORDS - PC_ROW_MARK;
    let words = |text: &str, font: egui::FontId, ink: Color32| {
        let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font, ink);
        job.wrap = egui::text::TextWrapping::truncate_at_width(room);
        ui.painter().layout_job(job)
    };
    let upper = words(name, semibold(text::LABEL), *ON_SURFACE);
    let lower = words(status, regular(text::CAPTION), *ON_SURFACE_VARIANT);
    let x = rect.left() + PC_ROW_WORDS;
    let top = rect.center().y - (upper.size().y + 2.0 + lower.size().y) / 2.0;
    let below = top + upper.size().y + 2.0;
    ui.painter().galley(Pos2::new(x, top), upper, *ON_SURFACE);
    ui.painter().galley(Pos2::new(x, below), lower, *ON_SURFACE_VARIANT);
    if current {
        let mark = Rect::from_center_size(Pos2::new(rect.right() - 14.0 - 10.0, rect.center().y), Vec2::splat(20.0));
        icons::paint(ui.painter(), Icon::Check, mark, *MOON);
    }
    focus_ring(ui, rect, &response, 14);
    response
}

/// A thin line between a menu's parts.
fn divider(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 9.0), Sense::hover());
    ui.painter().hline(rect.x_range().shrink(6.0), rect.center().y, Stroke::new(1.0, shade::TINT_SEPARATOR.get()));
}

/// The glass around a menu's rows.
const MENU_MARGIN: f32 = 6.0;

/// What a menu's list fades into where it is cut short.
pub(in crate::ui) const MENU_FADE: Color32 = Color32::from_rgba_premultiplied(0, 0, 0, 120);

/// A menu row: icon, label, and the shortcut at the right; why it is off under the label.
fn menu_row(ui: &mut Ui, row: &MenuRow) -> Response {
    let width = ui.available_width();
    let enabled = ui.is_enabled();
    // Why it is off wraps under the label.
    let why = row.blocked.map(|why| ui.painter().layout(tr(why).to_owned(), regular(text::CAPTION), *OUTLINE, width - 42.0 - 14.0));
    let height = why.as_ref().map_or(44.0, |w| 20.0 + 11.0 + w.size().y + 9.0);
    let (rect, response) = ui.allocate_exact_size(vec2(width, height), Sense::click());
    let label = tr(row.label);
    // A screen reader hears why a row is off too, not only the painted line under it.
    let said = match row.blocked {
        Some(why) => format!("{label}, {}", tr(why)),
        None => label.to_owned(),
    };
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, &said));
    let (lit, pressed) = hover_takes_focus(ui, &response);
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let painter = ui.painter();
    let mut lift = Vec::new();
    lift_shapes(&mut lift, ui.ctx(), rect, 14.0, lit, pressed);
    painter.extend(lift);
    let (ink, icon_ink) = if row.destructive { (*ERROR, *ERROR) } else if lit { (*ON_SURFACE, *ON_SURFACE) } else { (*ON_SURFACE, *ON_SURFACE_VARIANT) };
    let top = if row.blocked.is_some() { rect.top() + 20.0 } else { rect.center().y };
    icons::paint(painter, row.icon, Rect::from_center_size(Pos2::new(rect.left() + 21.0, top), Vec2::splat(18.0)), icon_ink);
    let words = painter.layout_no_wrap(label.to_owned(), semibold(text::LABEL), ink);
    painter.galley(Pos2::new(rect.left() + 42.0, top - words.size().y / 2.0), words, ink);
    if let Some(hint) = row.hint {
        let keys = painter.layout_no_wrap(tr(hint).to_owned(), regular(text::CAPTION), *OUTLINE);
        painter.galley(Pos2::new(rect.right() - 14.0 - keys.size().x, top - keys.size().y / 2.0), keys, *OUTLINE);
    }
    if let Some(why) = why {
        painter.galley(Pos2::new(rect.left() + 42.0, top + 11.0), why, *OUTLINE);
    }
    focus_ring(ui, rect, &response, 14);
    response
}
