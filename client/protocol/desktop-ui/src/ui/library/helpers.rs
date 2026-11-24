//! Library filtering, loading placeholders, play-state resolution, and launch policy helpers.

use super::*;

/// The games of `pool` under `filter` whose title holds the lowercase `words`, in `sort`'s order.
pub(super) fn matching<'a>(pool: &'a [console::Entry], filter: &console::LibraryFilter, words: &str, sort: console::Sort, played: console::Played<'_>) -> Vec<&'a console::Entry> {
    let mut shown = console::filtered(pool, filter, sort, played);
    if !words.is_empty() {
        shown.retain(|entry| entry.title.to_lowercase().contains(words));
    }
    shown
}

/// A library entry as a facts lookup takes it, with the key its facts are kept under.
pub(in crate::ui) fn entry_game(entry: &console::Entry) -> (String, nightjar_policy::game_info::job::Game) {
    let steam = nightjar_policy::game_info::names::steam_id(entry.key.strip_prefix("game:"));
    let game = nightjar_policy::game_info::job::Game { name: entry.title.clone(), steam };
    (nightjar_policy::game_info::names::key(&game.name, steam), game)
}

/// A length bucket's name in the Length chip and its list.
pub(in crate::ui) fn length_name(length: nightjar_policy::game_info::library::Length) -> &'static str {
    use nightjar_policy::game_info::library::Length;
    match length {
        Length::Short => en("Under 5 hours"),
        Length::Medium => en("5 to 15 hours"),
        Length::Long => en("15 to 40 hours"),
        Length::VeryLong => en("Over 40 hours"),
    }
}

/// What each chip would show for the words in the search.
pub(super) fn count_chips(chips: &mut [console::FilterChip], entries: &[console::Entry], hidden: &[console::Entry], words: &str) {
    if words.is_empty() {
        return;
    }
    for chip in chips {
        let pool = if chip.filter == console::LibraryFilter::Hidden { hidden } else { entries };
        chip.count = matching(pool, &chip.filter, words, console::Sort::default(), console::Played::default()).len();
    }
}

/// Above a cover of `width`, what its rise under the focus and its ring take, and a gap.
pub(in crate::ui) fn ring_clearance(width: f32) -> f32 {
    const RING: f32 = t::HC_FOCUS_RING_WIDTH + 3.0;
    cover_height(width) * (t::FOCUS_SCALE_COVER - 1.0) + RING + space::S
}

/// The places of `rows` lines of covers while the games are on their way, `captioned` as the covers
/// that will land there are, `widest` at most as theirs are, under one chip of `words` with a
/// spinner. A screen reader hears the words.
pub(in crate::ui) fn loading_covers(ui: &mut Ui, words: &str, rows: usize, captioned: bool, widest: f32) {
    let (columns, width) = grid_up_to(ui.available_width(), widest);
    let top = ui.cursor().min;
    let row_h = cover_height(width) + if captioned { POSTER_CAPTION } else { 0.0 };
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = GRID_ROW_GAP;
        for _ in 0..rows {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = POSTER_GAP;
                for _ in 0..columns {
                    cover_placeholder(ui, width, captioned);
                }
            });
        }
    });
    let room = Rect::from_min_size(top, vec2(ui.available_width(), rows as f32 * (row_h + GRID_ROW_GAP)));
    loading_chip(ui, room, words);
}

/// `words` with a spinner, over the middle of the places held for the covers.
fn loading_chip(ui: &mut Ui, room: Rect, words: &str) {
    const H: f32 = 40.0;
    const R: f32 = 8.0;
    let said = words.to_owned();
    let words = ui.painter().layout_no_wrap(said.clone(), regular(text::LABEL), *ON_SURFACE);
    let rect = Rect::from_center_size(room.center(), vec2(14.0 + 2.0 * R + 10.0 + words.size().x + 18.0, H));
    let mut out = Vec::new();
    glass::control(&mut out, ui.ctx(), rect, H / 2.0);
    ui.painter().extend(out);
    let spinning = Rect::from_min_size(pos2(rect.left() + 14.0, rect.center().y - R), Vec2::splat(2.0 * R));
    ui.scope_builder(UiBuilder::new().max_rect(spinning), |ui| spinner(ui, R));
    ui.painter().galley(pos2(spinning.right() + 10.0, rect.center().y - words.size().y / 2.0), words, *ON_SURFACE);
    ui.interact(rect, Id::unique(("loading-chip", &said)), Sense::hover())
        .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &said));
}

/// A Play that waits for its PC's app list, and the page it was pressed on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::ui) struct PendingPlay {
    pub host: u64,
    pub app: u32,
    pub screen: Screen,
}

/// What a game can do on its PC now ([`Launcher::play_state`]).
pub(in crate::ui) struct PlayState {
    pub detail: console::Detail,
    /// Its entry in the PC's app list, which is what starts a stream.
    pub classic: Option<nightjar_policy::http::App>,
    pub resumes: bool,
    /// The other game that playing this one would close.
    pub other_running: Option<String>,
    pub can_play: bool,
}

impl PlayState {
    /// Whether Play is on: the PC can start it, and its app list has the entry or may yet.
    pub(in crate::ui) fn could_play(&self, host: &Host) -> bool {
        self.detail.can_start && (self.classic.is_some() || matches!(host.apps, Apps::NotLoaded | Apps::Loading))
    }

    /// Why the PC's app list stops a game that could start, once the list has had its say.
    pub(in crate::ui) fn list_problem(&self, host: &Host) -> Option<String> {
        if !self.detail.can_start || self.classic.is_some() {
            return None;
        }
        match &host.apps {
            Apps::Failed(e) => Some(trf("The app list could not be loaded. {}", &[&e])),
            Apps::Loaded(_) => Some(tr("The PC’s app list has no entry for this game.").to_owned()),
            Apps::NotLoaded | Apps::Loading => None,
        }
    }
}

/// A game's cover as it should show now: what is known of it, and for a PC that is up and has yet to
/// be asked, on its way. A PC that is away sends none, so its covers without art are tiles.
pub(in crate::ui) fn shown_art<'a>(view: &'a View<'_>, host: u64, app: u32) -> Option<&'a Art> {
    let pc = view.hosts.get(host)?;
    match pc.art.get(&(app as i32)) {
        Some(art) => Some(art),
        None if pc.reach == Reach::Online => Some(&Art::Loading),
        None => None,
    }
}

/// Whether a paired PC that is up and shares its library has not sent it yet.
pub(in crate::ui) fn library_loading(view: &View<'_>) -> bool {
    view.hosts.list.iter().any(|h| {
        h.reach == Reach::Online && h.paired() && h.native_api() && h.native.library.is_none() && h.native.error.is_none()
    })
}

/// A Favourites card's focus key.
pub(in crate::ui) fn favourite_key(item: &console::Starred) -> String {
    format!("favourite:{}:{}", item.host, item.app_id)
}

/// The card to focus once favourite `index` loses its star: the next, else the previous; None
/// when it was the last.
pub(in crate::ui) fn favourite_neighbour(starred: &[console::Starred], index: usize) -> Option<String> {
    console::neighbour(starred.len(), index).map(|near| favourite_key(&starred[near]))
}

/// The card to focus once the game of card `key` leaves the screen: the next card of another game,
/// else the previous; None when there is none. `cards` are the screen's, in the order drawn, each
/// with its game: a game shown in several rows leaves all of them.
pub(in crate::ui) fn card_neighbour(cards: &[(String, String)], key: &str) -> Option<String> {
    let at = cards.iter().position(|(card, _)| card == key)?;
    let game = &cards[at].1;
    cards[at + 1..].iter().chain(cards[..at].iter().rev()).find(|(_, other)| other != game).map(|(card, _)| card.clone())
}

/// Quick resume: a paired PC running a game from its known library goes straight back into it.
pub(in crate::ui) fn host_clicked(host: &Host, quick_resume: bool, commands: &mut Vec<Command>) {
    let running = running_entry(host).filter(|_| host.paired());
    if let Some(app) = running.filter(|_| nightjar_policy::launch::resumes_on_pc_tap(quick_resume, true)) {
        commands.push(Command::Launch { host: host.id, app: app.id, resume: true });
    } else if host.paired() {
        commands.push(Command::OpenApps(host.id));
    } else if host.reach == Reach::Offline {
        commands.push(Command::Refresh(host.id));
    } else {
        commands.push(Command::Pair(host.id));
    }
}

/// Whether a game offers Input-only through a small stream, and what blocks it now: only an
/// ordinary game on a PC without its own Input-only mode, with the setting on, that leaves the
/// PC's running game alone.
pub(in crate::ui) fn small_stream_offer(host: &Host, app: &nightjar_policy::http::App, running: bool, any_host: bool) -> Option<Option<nightjar_policy::launch::InputOnlyBlocker>> {
    use nightjar_policy::launch::{self, InputOnlyMode};
    let lists_input_only = matches!(&host.apps, Apps::Loaded(apps) if apps.iter().any(|a| a.role(false) == nightjar_policy::http::AppRole::InputOnly));
    let tiles = host.info.as_ref().is_some_and(|i| i.capabilities.has(nightjar_policy::http::Feature::RemoteSessionTiles));
    let offered = app.control == nightjar_policy::http::AppControl::None
        && launch::keeps_running_game(host.running_app().is_some(), running)
        && launch::input_only_mode(any_host, tiles, lists_input_only) == InputOnlyMode::SmallStream;
    let permission = host.info.as_ref().and_then(|i| i.permission).map(u64::from);
    offered.then(|| launch::small_stream_blocker(host.paired(), permission))
}

/// The launch policy's names for an app's role and control tile.
pub fn policy_role(role: nightjar_policy::http::AppRole) -> nightjar_policy::launch::Role {
    use nightjar_policy::http::AppRole;
    use nightjar_policy::launch::Role;
    match role {
        AppRole::Stream => Role::Stream,
        AppRole::RemoteMonitor => Role::RemoteMonitor,
        AppRole::InputOnly => Role::InputOnly,
    }
}

pub(in crate::ui) fn policy_control(control: nightjar_policy::http::AppControl) -> nightjar_policy::launch::Control {
    use nightjar_policy::http::AppControl;
    use nightjar_policy::launch::Control;
    match control {
        AppControl::None => Control::None,
        AppControl::Resume => Control::Resume,
        AppControl::DisconnectMonitor => Control::DisconnectMonitor,
        AppControl::DisconnectInput => Control::DisconnectInput,
        AppControl::Terminate => Control::Terminate,
        AppControl::Monitor => Control::Monitor,
        AppControl::Input => Control::Input,
        AppControl::RunningGame => Control::RunningGame,
    }
}

/// The library entry of the app the PC is running, when its library is known.
pub(in crate::ui) fn running_entry(host: &Host) -> Option<&nightjar_policy::http::App> {
    let running = host.running_app()?;
    match &host.apps {
        Apps::Loaded(apps) => apps.iter().find(|a| a.matches_running_app(running, None) && !a.is_control_action()),
        _ => None,
    }
}

/// A name that starts a sentence: the stand-in "the running app" gets its capital, a game's own
/// name stays as it is spelled.
pub fn at_sentence_start(name: &str) -> String {
    if name != tr("the running app") {
        return name.to_owned();
    }
    let mut chars = name.chars();
    chars.next().map_or_else(String::new, |first| first.to_uppercase().chain(chars).collect())
}

/// The game's title and the device playing it, when selecting `app` asks first because another
/// device plays that very game. Never on a PC that does not offer moving the stream.
pub fn playing_elsewhere(host: &Host, app: i32, read: &nightjar_policy::http::StreamMove) -> Option<(String, String)> {
    let offered = host.info.as_ref().is_some_and(|info| {
        info.capabilities.availability(nightjar_policy::http::Feature::ContinueHere) == nightjar_policy::http::Availability::Supported
    });
    let playing = read.playing_on.as_ref();
    let known = match &host.apps {
        crate::hosts::Apps::Loaded(apps) => apps.iter().find(|known| known.id == app),
        _ => None,
    };
    let same = playing.is_some_and(|playing| plays_this_game(known, app, playing));
    if !nightjar_policy::continue_here::asks_on_select(offered, true, same) {
        return None;
    }
    playing.map(|playing| (playing.app.title.clone(), playing.device.clone()))
}

/// The app a join starts: `app`, or the game the PC says runs when `app` is a tile that only names
/// a way to reach it (a stream's Resume or Running Game).
pub fn join_target(known: Option<&nightjar_policy::http::App>, app: i32, playing: Option<&nightjar_policy::http::PlayingOn>) -> i32 {
    let tile = known.is_some_and(|known| known.control != nightjar_policy::http::AppControl::None);
    match playing {
        Some(playing) if tile => playing.app.app_id as i32,
        _ => app,
    }
}

/// What the prompt about another device's game starts when answered, app and resume: a tile that
/// only names a way to reach the game resumes the game itself, since the stream refuses a resume of
/// anything but the app the PC runs.
pub fn asked_start(known: Option<&nightjar_policy::http::App>, app: i32, resume: bool, playing: Option<&nightjar_policy::http::PlayingOn>) -> (i32, bool) {
    let target = join_target(known, app, playing);
    (target, resume || target != app)
}

/// The same game by its install id where both sides have one, as a stream's resume decides it: an
/// id alone can have been given to another game since the list was read. A tile that only names a
/// way to reach the game (a stream's Resume or Running Game) means whatever runs, as `join_target`
/// has it.
pub(in crate::ui) fn plays_this_game(known: Option<&nightjar_policy::http::App>, app: i32, playing: &nightjar_policy::http::PlayingOn) -> bool {
    use nightjar_policy::http::AppControl;
    match known {
        Some(known) if matches!(known.control, AppControl::Resume | AppControl::RunningGame) => true,
        Some(known) => known.matches_running_app(playing.app.app_id as i32, Some(&playing.app.install_id)),
        None => playing.app.app_id as i32 == app,
    }
}

pub fn running_name(host: &Host) -> String {
    running_entry(host).map_or_else(|| tr("the running app").into(), |app| app.name.clone())
}

/// The classic app id of the app the PC runs now, once the app list is loaded.
pub(in crate::ui) fn running_app_id(host: &Host) -> Option<i32> {
    running_entry(host).map(|app| app.id)
}

/// Whether a PC's page has the round Refresh in its head, and whether it is at work: for a list to
/// read again. A page with no list to show has that action in its middle.
pub(in crate::ui) fn head_refresh(host: &Host, refreshing_empty: Option<u64>) -> Option<bool> {
    match &host.apps {
        Apps::Loaded(apps) if !apps.is_empty() => Some(false),
        Apps::Loading if refreshing_empty != Some(host.id) => Some(true),
        _ => None,
    }
}

/// What a Resume names: the list's entry, else the id the PC reports, which is all a resume needs.
pub(in crate::ui) fn resume_target(host: &Host) -> Option<i32> {
    running_app_id(host).or(host.running_app())
}

/// Where the game page's stream size, rate and bitrate come from, for those not from the main
/// settings: a PC or profile override would otherwise hold them out of sight of the game's
/// settings ("Frame rate set for this PC").
pub(in crate::ui) fn summary_sources(profile: Option<&crate::config::Profile>, pc: Option<&Overrides>, game: Option<&Overrides>) -> Vec<String> {
    use crate::config::{source, Source as Layer};
    let rows: [(&str, &[&str]); 3] = [(en("Resolution"), &["width", "height"]), (en("Frame rate"), &["fps"]), (en("Bitrate"), &["bitrate_kbps"])];
    rows.into_iter()
        .filter_map(|(name, keys)| match source(keys, profile, pc, game) {
            Layer::Main => None,
            Layer::Game => Some(trf("{} set for this game", &[&tr(name)])),
            Layer::Pc => Some(trf("{} set for this PC", &[&tr(name)])),
            Layer::Profile(profile) => Some(trf("{} from profile {}", &[&tr(name), &profile])),
        })
        .collect()
}
