//! Console tests for host cards, recents, favourites, library cache invalidation, and sorting.

use super::*;

#[test]
fn the_fixture_capabilities_give_no_host_control() {
    let caps: Capabilities = serde_json::from_str(CAPABILITIES).unwrap();
    assert!(!caps.permissions.host_control);
}

#[test]
fn host_cards_say_what_they_know() {
    let lib = library();
    let none: Option<&Library> = None;
    assert_eq!(host_line(&source(1, "A", true, Some(&lib), None)), HostLine::Games(2));
    assert_eq!(host_line(&source(1, "A", false, Some(&lib), None)), HostLine::SavedGames(2));
    assert_eq!(host_line(&source(1, "A", false, none, None)), HostLine::Offline);
    assert_eq!(host_line(&source(1, "A", true, none, None)), HostLine::NoLibrary);
    assert_eq!(HostLine::Games(2).text(), "Games: 2");
    assert_eq!(HostLine::SavedGames(2).text(), "Offline. Saved games: 2");
    assert_eq!(HostLine::NoLibrary.text(), "Games not loaded yet");
    // A game the PC has not installed counts, as it does in Home's rows and their "All N".
    let mut some_missing = library();
    some_missing.games[1].installed = Some(false);
    assert_eq!(host_line(&source(1, "A", true, Some(&some_missing), None)), HostLine::Games(2));
    let entries = merge_library(&[source(1, "A", true, Some(&some_missing), None)]);
    assert_eq!(library_rows(&entries, Sort::default(), unplayed()).iter().map(|r| r.games.len()).sum::<usize>(), 2);
}

#[test]
fn the_stream_summary_reads_like_the_android_one() {
    assert_eq!(stream_summary(1920, 1080, 60, 20_000), "1920 × 1080, 60 FPS, 20 Mbps");
    assert_eq!(stream_summary(2560, 1440, 120, 37_500), "2560 × 1440, 120 FPS, 37.5 Mbps");
}

/// Android's GameDetailModel: a game the PC lists as not installed cannot be started, but one
/// that runs or is suspended there can still be resumed.
#[test]
fn a_game_the_pc_has_not_installed_cannot_start_but_resumes() {
    let mut lib = library();
    lib.games[0].installed = Some(false); // Dota 2
    let idle = idle();
    let d = detail(&source(1, "A", true, Some(&lib), Some(&idle)), DOTA, true);
    assert_eq!((d.installed, d.can_start), (false, false), "not installed: no Play");
    let suspended = session(); // Dota 2 suspended
    let d = detail(&source(1, "A", true, Some(&lib), Some(&suspended)), DOTA, true);
    assert_eq!((d.installed, d.can_start, d.resumes()), (false, true, true), "a suspended game resumes");
    // Saying nothing either way counts as installed.
    lib.games[0].installed = None;
    let d = detail(&source(1, "A", true, Some(&lib), Some(&idle)), DOTA, true);
    assert_eq!((d.installed, d.can_start), (true, true));
}

#[test]
fn play_finds_the_classic_entry_by_install_id_then_app_id() {
    let apps = [(11, ""), (12, "C6F1A9A4-4B51-5F43-9D0B-7A2B1C2D3E4F"), (13, "x")];
    assert_eq!(classic_app(&apps, "c6f1a9a4-4b51-5f43-9d0b-7a2b1c2d3e4f", 99), Some(12));
    assert_eq!(classic_app(&apps, "none", 13), Some(13));
    assert_eq!(classic_app(&apps, "none", 99), None);
    // An empty uuid never matches an empty install id.
    assert_eq!(classic_app(&apps, "", 99), None);
}

#[test]
fn a_saved_game_whose_install_id_is_gone_is_not_listed_unless_its_entry_has_no_uuid() {
    let apps = [(11, ""), (12, "C6F1A9A4-4B51-5F43-9D0B-7A2B1C2D3E4F"), (13, "another-game")];
    assert_eq!(saved_classic_app(&apps, "c6f1a9a4-4b51-5f43-9d0b-7a2b1c2d3e4f", 99), Some(12));
    assert_eq!(saved_classic_app(&apps, "gone", 13), None);
    assert_eq!(saved_classic_app(&apps, "gone", 11), Some(11), "the entry has no UUID to tell it by");
    assert_eq!(saved_classic_app(&apps, "gone", 99), None);
    assert_eq!(saved_classic_app(&apps, "", 13), Some(13), "saved without one: the app id is all there is");
}

fn recent(host: &str, install: &str, app: u32, title: &str) -> Recent {
    Recent { host: host.into(), install_id: install.into(), app_id: app, title: title.into(), at: 0 }
}

#[test]
fn recently_played_lists_known_games_latest_first() {
    let lib = library();
    let mut a = source(1, "Desk", true, Some(&lib), None);
    a.unique_id = "uid-a";
    let recents = [
        recent("uid-a", "8D4B3F55-0C2E-4F8A-9A61-2F1C6F0E9B17", 0, "old name"),
        recent("uid-a", "c6f1a9a4-4b51-5f43-9d0b-7a2b1c2d3e4f", 1093052211, "Dota 2"),
        // A game the PC no longer lists, a PC that is not here, a repeat.
        recent("uid-a", "gone", 5, "Gone"),
        recent("uid-z", "c6f1a9a4-4b51-5f43-9d0b-7a2b1c2d3e4f", 1093052211, "Dota 2"),
        recent("uid-a", "", 1093052211, "Dota 2"),
    ];
    let sources = [a];
    let list = recently_played(&sources, &recents, &[], 6);
    // Matched by install id without case, and titled from the library.
    assert_eq!(list.iter().map(|r| r.title.as_str()).collect::<Vec<_>>(), ["Desktop", "Dota 2"]);
    assert_eq!((list[0].host, list[0].host_name.as_str(), list[0].online), (1, "Desk", true));
    assert_eq!(recently_played(&sources, &recents, &[], 1).len(), 1, "capped");
}

#[test]
fn a_list_line_names_the_store_and_when_the_game_was_last_played() {
    let lib = library();
    let mut a = source(1, "Desk", true, Some(&lib), None);
    a.unique_id = "uid-a";
    let sources = [a];
    let entries = merge_library(&sources);
    let dota = entries.iter().find(|e| e.title == "Dota 2").expect("Dota 2");
    let at = |r: Recent, at: u64| Recent { at, ..r };
    let recents = [
        at(recent("uid-a", "", 1093052211, "Dota 2"), 1_000),
        at(recent("uid-a", "C6F1A9A4-4B51-5F43-9D0B-7A2B1C2D3E4F", 0, "Dota 2"), 5_000),
        // Another PC's copy of the same game is not this one.
        at(recent("uid-z", "", 1093052211, "Dota 2"), 9_000),
    ];
    assert_eq!(last_played(dota, &sources, &recents), Some(5_000), "the latest, by install id without case");
    assert_eq!(last_played(dota, &sources, &[]), None);
    // What the PCs say comes first: the newest across the copies, then never (0) over the
    // recents, which keep only the last few.
    let mut b = source(2, "Den", false, Some(&lib), None);
    b.unique_id = "uid-b";
    let install = &lib.games.iter().find(|g| g.title == "Dota 2").unwrap().install_id;
    let (older, newer) = (std::collections::BTreeMap::from([(install.clone(), 6_000)]), std::collections::BTreeMap::from([(install.clone(), 8_000)]));
    let mut a2 = source(1, "Desk", true, Some(&lib), None);
    a2.unique_id = "uid-a";
    let mut b2 = b;
    a2.last_played = Some(&older);
    b2.last_played = Some(&newer);
    let both = [a2, b2];
    let merged = merge_library(&both);
    let dota2 = merged.iter().find(|e| e.title == "Dota 2").expect("Dota 2");
    assert_eq!(dota2.copies.len(), 2);
    assert_eq!(last_played(dota2, &both, &recents), Some(8_000), "the newest a PC says, not the recents");
    let never = std::collections::BTreeMap::from([(install.clone(), 0)]);
    let mut a3 = source(1, "Desk", true, Some(&lib), None);
    a3.unique_id = "uid-a";
    a3.last_played = Some(&never);
    let one = [a3];
    let merged = merge_library(&one);
    let dota3 = merged.iter().find(|e| e.title == "Dota 2").expect("Dota 2");
    assert_eq!(last_played(dota3, &one, &recents), Some(0), "the PC says never played");
    let store = dota.best().store_name.clone();
    let line = |played, now| list_line(dota, played, now);
    let with_store = |when: &str| match &store { Some(s) => format!("{s} · {when}"), None => when.to_owned() };
    assert_eq!(line(Some(0), 9_000), with_store("Not played yet"), "0 is the host's never played");
    assert_eq!(line(None, 9_000), store.clone().unwrap_or_default(), "unknown says nothing about playing");
    assert_eq!(line(Some(5_000), 5_000 + 3_600), with_store("Played today"));
    assert_eq!(line(Some(5_000), 5_000 + 86_400 + 60), with_store("Played yesterday"));
    assert_eq!(line(Some(5_000), 5_000 + 9 * 86_400), with_store("Played 9 days ago"));
    assert_eq!(line(Some(9_000), 5_000), with_store("Played today"), "a clock that went back reads as today");
}

#[test]
fn a_hidden_game_takes_no_place_among_the_recent_ones() {
    let lib = library();
    let mut a = source(1, "Desk", true, Some(&lib), None);
    a.unique_id = "uid-a";
    let sources = [a];
    let recents = [recent("uid-a", "", 1093052211, "Dota 2"), recent("uid-a", "", 881448767, "Desktop")];
    let keys = hide_keys(&sources, 1, 1093052211);
    assert_eq!(keys.len(), 1);
    let list = recently_shown(&sources, &recents, &[], &keys, 1);
    assert_eq!(list.iter().map(|r| r.app_id).collect::<Vec<_>>(), [881448767], "the next one has its place");
    assert_eq!(recently_shown(&sources, &recents, &[], &[], 1)[0].app_id, 1093052211);
    assert!(recently_shown(&sources, &recents[..1], &[], &keys, 6).is_empty());
}

#[test]
fn recently_played_skips_what_is_under_continue_playing_and_uninstalled_games() {
    let mut lib = library();
    let mut a = source(1, "Desk", true, Some(&lib), None);
    a.unique_id = "uid-a";
    let recents = [recent("uid-a", "", 1093052211, "Dota 2"), recent("uid-a", "", 881448767, "Desktop")];
    let playing = [Continue { host: 1, host_name: "Desk".into(), app_id: 1093052211, title: "Dota 2".into(), playing: Playing::Running }];
    let list = recently_played(&[a], &recents, &playing, 6);
    assert_eq!(list.iter().map(|r| r.app_id).collect::<Vec<_>>(), [881448767]);
    lib.games[1].installed = Some(false);
    let mut a = source(1, "Desk", true, Some(&lib), None);
    a.unique_id = "uid-a";
    assert!(recently_played(&[a], &recents[1..], &[], 6).is_empty(), "an uninstalled game is not offered");
    // A PC that answered no unique id yet matches nothing, and a PC away still lists its games.
    let away = source(2, "Away", false, Some(&lib), None);
    assert!(recently_played(&[away], &recents, &[], 6).is_empty());
    let mut away = source(2, "Away", false, Some(&lib), None);
    away.unique_id = "uid-a";
    let list = recently_played(&[away], &recents[..1], &[], 6);
    assert_eq!((list.len(), list[0].online), (1, false));
}

fn star(host: &str, install: &str, app: u32, title: &str) -> Favourite {
    Favourite { host: host.into(), install_id: install.into(), app_id: app, title: title.into() }
}

#[test]
fn favourites_list_the_starred_games_newest_first_titled_from_the_library() {
    let lib = library();
    let mut a = source(1, "Desk", true, Some(&lib), None);
    a.unique_id = "uid-a";
    let mut b = source(2, "Den", false, Some(&lib), None);
    b.unique_id = "uid-b";
    let stars = [
        star("uid-b", "", 881448767, "old name"),
        star("uid-a", "c6f1a9a4-4b51-5f43-9d0b-7a2b1c2d3e4f", 0, "Dota 2"),
        // Not listed any more, a PC that is not here, a repeat.
        star("uid-a", "gone", 5, "Gone"),
        star("uid-z", "", 881448767, "Desktop"),
        star("uid-b", "", 881448767, "again"),
    ];
    let sources = [a, b];
    let list = favourite_games(&sources, &stars);
    assert_eq!(list.iter().map(|s| (s.host, s.title.as_str(), s.online)).collect::<Vec<_>>(),
        [(2, "Desktop", false), (1, "Dota 2", true)]);
    // Two PCs with the same game are two favourites, and each star only marks its own PC.
    let game = &lib.games[0];
    assert!(is_starred(&sources[0], game, &stars));
    assert!(!is_starred(&sources[1], game, &stars));
}

#[test]
fn a_hidden_favourite_comes_back_and_a_pc_without_a_library_shows_the_saved_title() {
    let mut lib = library();
    let stars = [star("uid-a", "", 881448767, "Desktop")];
    lib.games[1].installed = Some(false);
    let mut a = source(1, "Desk", true, Some(&lib), None);
    a.unique_id = "uid-a";
    assert!(favourite_games(&[a], &stars).is_empty(), "an uninstalled game is not offered");
    lib.games[1].installed = None;
    let mut a = source(1, "Desk", true, Some(&lib), None);
    a.unique_id = "uid-a";
    assert_eq!(favourite_games(&[a], &stars).len(), 1);
    let mut unknown = source(1, "Desk", false, None, None);
    unknown.unique_id = "uid-a";
    let list = favourite_games(&[unknown], &stars);
    assert_eq!((list[0].title.as_str(), list[0].app_id), ("Desktop", 881448767));
    // A PC that told no unique id yet cannot be starred on.
    let nameless = source(3, "New", true, Some(&lib), None);
    assert!(favourite_of(&nameless, &lib.games[0]).is_none());
}

#[test]
fn a_star_of_a_pc_that_is_no_longer_paired_is_not_offered_but_an_offline_paired_pc_keeps_it() {
    let stars = [star("uid-a", "", DESKTOP, "Desktop")];
    let mut unpaired = source(1, "Desk", true, None, None);
    unpaired.unique_id = "uid-a";
    unpaired.paired = false;
    assert!(favourite_games(&[unpaired], &stars).is_empty(), "a card that cannot start");
    let mut away = source(1, "Desk", false, None, None);
    away.unique_id = "uid-a";
    assert_eq!(favourite_games(&[away], &stars).len(), 1, "a paired PC that is away keeps its star");
}

#[test]
fn the_running_app_is_checked_again_before_it_is_closed() {
    assert_eq!(recheck_running(Some(11), Some(11)), Running::Same);
    assert_eq!(recheck_running(Some(11), None), Running::Gone);
    assert_eq!(recheck_running(Some(11), Some(12)), Running::Changed);
    // Nothing was named (the app list was not loaded): only a stopped app is told apart.
    assert_eq!(recheck_running(None, Some(12)), Running::Same);
    assert_eq!(recheck_running(None, None), Running::Gone);
}

#[test]
fn a_removed_favourite_hands_focus_to_the_next_card_else_the_previous() {
    assert_eq!(neighbour(3, 0), Some(1));
    assert_eq!(neighbour(3, 1), Some(2));
    assert_eq!(neighbour(3, 2), Some(1));
    assert_eq!(neighbour(1, 0), None);
    assert_eq!(neighbour(0, 0), None);
}

/// Everything a [`Source`] shows of one PC, owned, so a test can change any of it.
#[derive(Clone)]
struct Pc {
    id: u64,
    unique_id: String,
    name: String,
    online: bool,
    paired: bool,
    library: Option<Library>,
    classic: bool,
    last_played: Option<std::collections::BTreeMap<String, u64>>,
    playtime_minutes: Option<std::collections::BTreeMap<String, u64>>,
    collections: Option<std::collections::BTreeMap<String, Vec<String>>>,
    session: Option<Session>,
    host_control: bool,
}

fn sources_of(pcs: &[Pc]) -> Vec<Source<'_>> {
    pcs.iter()
        .map(|pc| Source {
            id: pc.id,
            unique_id: &pc.unique_id,
            name: &pc.name,
            online: pc.online,
            paired: pc.paired,
            library: pc.library.as_ref(),
            classic: pc.classic,
            last_played: pc.last_played.as_ref(),
            playtime_minutes: pc.playtime_minutes.as_ref(),
            collections: pc.collections.as_ref(),
            session: pc.session.as_ref(),
            host_control: pc.host_control,
        })
        .collect()
}

/// Two PCs with the fixture's library, the second one's Desktop hidden on this device.
fn two_pcs() -> (Vec<Pc>, Vec<Favourite>) {
    let lib = library();
    let pc = |id: u64, unique_id: &str, name: &str| Pc {
        id,
        unique_id: unique_id.into(),
        name: name.into(),
        online: true,
        paired: true,
        library: Some(lib.clone()),
        classic: false,
        last_played: Some([(lib.games[0].install_id.clone(), 100)].into()),
        playtime_minutes: Some([(lib.games[0].install_id.clone(), 30)].into()),
        collections: Some(in_collections(&lib)),
        session: Some(idle()),
        host_control: false,
    };
    let hidden = vec![star("two", &lib.games[1].install_id, DESKTOP, "")];
    (vec![pc(1, "one", "Den"), pc(2, "two", "Loft")], hidden)
}

type Change = fn(&mut Vec<Pc>, &mut Vec<Favourite>);

/// Each thing the library is made from, changed on its own. `true` where the change shows in
/// the merged library; the others are in a `Source` without the merge reading them today.
const CHANGES: &[(&str, bool, Change)] = &[
    ("a game's title", true, |pcs, _| pcs[0].library.as_mut().unwrap().games[0].title = "Dota 3".into()),
    ("a game starts", true, |pcs, _| pcs[0].library.as_mut().unwrap().games[0].running = true),
    ("a game is uninstalled", true, |pcs, _| pcs[0].library.as_mut().unwrap().games[0].installed = Some(false)),
    ("a game's store", true, |pcs, _| pcs[0].library.as_mut().unwrap().games[0].store.as_mut().unwrap().source = "epic".into()),
    ("a store's name", true, |pcs, _| pcs[0].library.as_mut().unwrap().games[0].store.as_mut().unwrap().name = "Valve".into()),
    ("a game leaves its store", true, |pcs, _| pcs[0].library.as_mut().unwrap().games[0].store = None),
    ("a cover goes", true, |pcs, _| pcs[0].library.as_mut().unwrap().games[0].artwork.cover = None),
    ("a cover is replaced", false, |pcs, _| pcs[0].library.as_mut().unwrap().games[0].artwork.version = "new".into()),
    ("a game's id", true, |pcs, _| pcs[0].library.as_mut().unwrap().games[0].game_id = Some("steam:571".into())),
    ("a game's install id", true, |pcs, _| pcs[0].library.as_mut().unwrap().games[0].install_id = "another".into()),
    ("a game's app id", true, |pcs, _| pcs[0].library.as_mut().unwrap().games[0].app_id = 7),
    ("a game is removed", true, |pcs, _| drop(pcs[0].library.as_mut().unwrap().games.remove(0))),
    ("a game is added", true, |pcs, _| {
        let games = &mut pcs[1].library.as_mut().unwrap().games;
        let mut game = games[0].clone();
        (game.title, game.game_id, game.app_id, game.install_id) = ("Another".into(), Some("steam:1".into()), 1, "new".into());
        games.push(game);
    }),
    ("the games change places", false, |pcs, _| pcs[0].library.as_mut().unwrap().games.swap(0, 1)),
    ("a library's revision", false, |pcs, _| pcs[0].library.as_mut().unwrap().revision = "next".into()),
    ("a library goes", true, |pcs, _| pcs[1].library = None),
    ("a game was played", true, |pcs, _| *pcs[0].last_played.as_mut().unwrap().values_mut().next().unwrap() = 200),
    ("the last played times go", true, |pcs, _| pcs[0].last_played = None),
    ("a game's play time grew", true, |pcs, _| *pcs[0].playtime_minutes.as_mut().unwrap().values_mut().next().unwrap() = 90),
    ("the play times go", true, |pcs, _| pcs[0].playtime_minutes = None),
    ("a game leaves a collection", true, |pcs, _| drop(pcs[0].collections.as_mut().unwrap().values_mut().next().unwrap().pop())),
    ("the collections go", true, |pcs, _| pcs[0].collections = None),
    ("a session starts", true, |pcs, _| pcs[1].session = Some(session())),
    ("a session's revision", false, |pcs, _| pcs[1].session.as_mut().unwrap().revision += 1),
    ("a session goes", false, |pcs, _| pcs[1].session = None),
    ("a pc goes away", true, |pcs, _| pcs[0].online = false),
    ("a pc is renamed", true, |pcs, _| pcs[0].name = "Study".into()),
    ("a pc's id", true, |pcs, _| pcs[0].id = 9),
    ("a pc tells another unique id", true, |pcs, _| pcs[1].unique_id = "zwei".into()),
    ("a pc is unpaired", false, |pcs, _| pcs[0].paired = false),
    ("a pc gives host control", false, |pcs, _| pcs[0].host_control = true),
    ("a library turns out to be an app list", false, |pcs, _| pcs[0].classic = true),
    ("a pc is removed", true, |pcs, _| drop(pcs.remove(1))),
    ("a pc is added", true, |pcs, _| {
        let mut pc = pcs[0].clone();
        (pc.id, pc.unique_id, pc.name) = (3, "three".into(), "Attic".into());
        pcs.push(pc);
    }),
    ("the pcs change places", true, |pcs, _| pcs.swap(0, 1)),
    ("a game is hidden", true, |pcs, hidden| {
        let game = &pcs[0].library.as_ref().unwrap().games[0];
        hidden.push(star("one", &game.install_id, game.app_id, ""));
    }),
    ("a game is shown again", true, |_, hidden| hidden.clear()),
    ("a hidden game's saved title", false, |_, hidden| hidden[0].title = "Desktop".into()),
];

#[test]
fn the_kept_library_is_merged_again_when_anything_it_was_made_from_changes() {
    for (what, shows, change) in CHANGES {
        let (before, hidden_before) = two_pcs();
        let memo = LibraryMemo::default();
        let first = memo.get(&sources_of(&before), &hidden_before);
        assert_eq!(*first, Merged::of(&sources_of(&before), &hidden_before), "{what}");

        let (mut after, mut hidden_after) = (before.clone(), hidden_before.clone());
        change(&mut after, &mut hidden_after);
        let changed = memo.get(&sources_of(&after), &hidden_after);
        assert_eq!(memo.merges(), 2, "{what}: merged again");
        assert_eq!(*changed, Merged::of(&sources_of(&after), &hidden_after), "{what}: what a merge now gives");
        assert_eq!(*changed != *first, *shows, "{what}: whether it shows");

        // And back: the kept one is never one of an earlier state.
        let back = memo.get(&sources_of(&before), &hidden_before);
        assert_eq!(memo.merges(), 3, "{what}: merged again on the way back");
        assert_eq!(*back, *first, "{what}");
    }
}

#[test]
fn the_library_is_merged_once_while_nothing_changes() {
    let (pcs, hidden) = two_pcs();
    let memo = LibraryMemo::default();
    let first = memo.get(&sources_of(&pcs), &hidden);
    // The same values from other places in memory, as each frame's `Source`s are.
    let (copies, hidden_copy) = (pcs.clone(), hidden.clone());
    for _ in 0..3 {
        let again = memo.get(&sources_of(&copies), &hidden_copy);
        assert!(std::rc::Rc::ptr_eq(&first, &again));
    }
    assert_eq!(memo.merges(), 1);

    // No PC at all is a library too (the empty one), and is not merged over and over either.
    let empty = LibraryMemo::default();
    assert!(empty.get(&[], &[]).all().is_empty());
    assert!(empty.get(&[], &[]).shown().is_empty());
    assert_eq!(empty.merges(), 1);
}

#[test]
fn the_kept_library_is_the_merge_and_its_split() {
    let (pcs, hidden) = two_pcs();
    let sources = sources_of(&pcs);
    let memo = LibraryMemo::default();

    let merged = memo.get(&sources, &hidden);
    let (shown, away) = split_hidden(merge_library(&sources), &sources, &hidden);
    assert_eq!(merged.all(), merge_library(&sources));
    assert_eq!(merged.shown(), shown);
    assert_eq!(merged.hidden(), away);
    assert_eq!(away.len(), 1, "the second PC's Desktop");
    assert_eq!(shown.iter().map(|e| e.copies.len()).sum::<usize>(), 3);

    // With nothing hidden all of it shows.
    let merged = memo.get(&sources, &[]);
    assert_eq!(merged.shown(), merge_library(&sources));
    assert!(merged.hidden().is_empty());
}
/// Sorting as Home does it with no PCs and no recents: what the PCs' times alone say.
/// Four games of one PC: two installed, times as the PC says them.
fn sortable() -> (Library, std::collections::BTreeMap<String, u64>, std::collections::BTreeMap<String, u64>) {
    let game = |id: &str, title: &str, installed: bool| {
        serde_json::json!({"install_id": id, "app_id": id.parse::<u32>().unwrap(), "title": title, "game_id": format!("steam:{id}"),
            "store": {"source": "steam", "id": id, "name": "Steam"}, "installed": installed, "running": false, "artwork": {"version": id}})
    };
    let library: Library = serde_json::from_value(serde_json::json!({"revision": "r", "games": [
        game("1", "Hades", true), game("2", "Celeste", false), game("3", "balatro", true), game("4", "Apex", false)]}))
    .unwrap();
    let played = [("1", 300), ("2", 900), ("3", 0)].map(|(id, at)| (id.to_owned(), at)).into();
    let minutes = [("1", 40), ("2", 10), ("3", 600)].map(|(id, at)| (id.to_owned(), at)).into();
    (library, played, minutes)
}

#[test]
fn rows_and_all_games_follow_the_chosen_sort_with_the_installed_first() {
    let (lib, played, minutes) = sortable();
    let sources = [Source { last_played: Some(&played), playtime_minutes: Some(&minutes), ..source(1, "A", true, Some(&lib), None) }];
    let entries = merge_library(&sources);
    let titles = |games: &[&Entry]| games.iter().map(|e| e.title.clone()).collect::<Vec<_>>();
    let by = |by: HomeSort, order: HomeOrder| Sort { by, order };
    let row = |sort: Sort| titles(&library_rows(&entries, sort, unplayed())[0].games);
    assert_eq!(row(Sort::default()), ["balatro", "Hades", "Apex", "Celeste"], "installed first, each part A to Z");
    assert_eq!(row(by(HomeSort::LastPlayed, HomeOrder::Descending)), ["Hades", "balatro", "Celeste", "Apex"]);
    assert_eq!(row(by(HomeSort::Playtime, HomeOrder::Descending)), ["balatro", "Hades", "Celeste", "Apex"]);
    assert_eq!(row(by(HomeSort::Title, HomeOrder::Descending)), ["Hades", "balatro", "Celeste", "Apex"]);
    let all = filtered(&entries, &LibraryFilter::All, by(HomeSort::LastPlayed, HomeOrder::Descending), unplayed());
    assert_eq!(titles(&all), ["Celeste", "Hades", "Apex", "balatro"], "All games sorts without putting the installed first");
}

#[test]
fn last_played_sorts_by_the_time_the_cards_show() {
    let (lib, times, minutes) = sortable();
    let mut a = source(1, "A", true, Some(&lib), None);
    a.unique_id = "uid-a";
    a.last_played = Some(&times);
    a.playtime_minutes = Some(&minutes);
    let sources = [a];
    let entries = merge_library(&sources);
    let titles = |games: Vec<&Entry>| games.iter().map(|e| e.title.clone()).collect::<Vec<_>>();
    let sort = Sort { by: HomeSort::LastPlayed, order: HomeOrder::Descending };
    // Apex's PC says nothing: it counts as never played, after the timed games.
    assert_eq!(titles(filtered(&entries, &LibraryFilter::All, sort, Played::default())), ["Celeste", "Hades", "Apex", "balatro"]);
    // This device streamed it yesterday: its card says so, and it sorts first, as on Android.
    let recents = [Recent { at: 5_000, ..recent("uid-a", "", 4, "Apex") }];
    let played = Played { sources: &sources, recents: &recents };
    let apex = entries.iter().find(|e| e.title == "Apex").expect("Apex");
    assert_eq!(played.last(apex), Some(5_000));
    assert_eq!(titles(filtered(&entries, &LibraryFilter::All, sort, played)), ["Apex", "Celeste", "Hades", "balatro"]);
}

#[test]
fn grouped_by_installation_the_installed_come_in_a_row_of_their_own() {
    let (lib, played, minutes) = sortable();
    let sources = [Source { last_played: Some(&played), playtime_minutes: Some(&minutes), ..source(1, "A", true, Some(&lib), None) }];
    let entries = merge_library(&sources);
    assert!(offered_groups(&entries).contains(&HomeGroup::Installation));
    let rows = home_rows(&entries, HomeGroup::Installation, Sort { by: HomeSort::LastPlayed, order: HomeOrder::Descending }, unplayed());
    let got: Vec<(RowKind, Vec<&str>)> = rows.iter().map(|r| (r.kind.clone(), r.games.iter().map(|e| e.title.as_str()).collect())).collect();
    assert_eq!(got, [(RowKind::Installed, vec!["Hades", "balatro"]), (RowKind::NotInstalled, vec!["Celeste", "Apex"])]);
    for row in &rows {
        assert_eq!(filtered(&entries, &row.filter, Sort::default(), unplayed()).len(), row.games.len(), "its All tile opens its games");
    }
    let installed: Vec<Entry> = entries.iter().filter(|e| e.installed()).cloned().collect();
    assert_eq!(shown_group(HomeGroup::Installation, &installed), HomeGroup::Store, "all installed: nothing to split");
    assert!(!offered_groups(&installed).contains(&HomeGroup::Installation));
}

#[test]
fn play_time_is_offered_only_while_some_pc_sends_it() {
    let (lib, played, minutes) = sortable();
    let timed = merge_library(&[Source { last_played: Some(&played), playtime_minutes: Some(&minutes), ..source(1, "A", true, Some(&lib), None) }]);
    assert_eq!(offered_sorts(&timed), HomeSort::ALL);
    let untimed = merge_library(&[source(1, "A", true, Some(&lib), None)]);
    assert_eq!(offered_sorts(&untimed), [HomeSort::Title, HomeSort::LastPlayed]);
    let settings = crate::config::Settings { home_sort: HomeSort::Playtime, home_order: HomeOrder::Descending, ..Default::default() };
    assert_eq!(Sort::shown(&settings, &timed), Sort { by: HomeSort::Playtime, order: HomeOrder::Descending });
    assert_eq!(Sort::shown(&settings, &untimed), Sort { by: HomeSort::Title, order: HomeOrder::Descending }, "the choice stays for when it does");
}
