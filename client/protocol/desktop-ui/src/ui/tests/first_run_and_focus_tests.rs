//! First-run onboarding, Back button stickiness, and focus-ring tests.

use super::*;

#[test]
fn the_open_searchs_words_sit_on_the_closed_ones_line() {
    let mut f = store_mix();
    f.settle();
    let closed = f.placed.iter().find(|(t, r, _)| t == "Search" && r.top() < 68.0).expect("the closed field").1;
    f.l.open_search();
    f.settle();
    let open = f.placed.iter().find(|(t, r, _)| t == "Search" && r.top() < 68.0).expect("the field's hint").1;
    assert!((open.center().y - closed.center().y).abs() <= 0.5, "open {open:?}, closed {closed:?}");
    f.l.library_query = "Search".into();
    f.settle();
    let typed = f.placed.iter().find(|(t, r, _)| t == "Search" && r.top() < 68.0).expect("the typed words").1;
    assert!((typed.center().y - closed.center().y).abs() <= 0.5, "typed {typed:?}, closed {closed:?}");
}

#[test]
#[ignore = "writes PNGs; set NIGHTJAR_SHOTS"]
fn page_screenshots() {
    let Some(dir) = std::env::var_os("NIGHTJAR_SHOTS").map(std::path::PathBuf::from) else { return };
    std::fs::create_dir_all(&dir).expect("the folder for the screenshots");
    let prefix = std::env::var("NIGHTJAR_SHOTS_PREFIX").unwrap_or_default();
    let named = |name: &str, id| nightjar_policy::http::App { name: name.into(), ..game(id) };
    let games = vec![named("Hollow Knight", 7), named("Forza Horizon 5", 9), named("Desktop", 11), named("Steam Big Picture", 13)];
    for high in [false, true] {
        let look = if high { "high-contrast" } else { "normal" };
        let fresh = || {
            crate::theme::own_palette();
            let ctx = egui::Context::default();
            crate::theme::apply(&ctx);
            let mut l = launcher(false, false);
            l.settings.high_contrast = high;
            l.go(Screen::Hosts);
            let records = vec![
                HostRecord { address: "192.0.2.10".into(), ..record("Living Room PC", true, true) },
                HostRecord { address: "192.0.2.11".into(), ..record("Office PC", true, true) },
                HostRecord { address: "192.0.2.12".into(), ..record("Studio", false, false) },
                HostRecord { address: "192.0.2.13".into(), ..record("Laptop", true, false) },
            ];
            let mut hosts = Hosts::without_discovery(records, Arc::new(|| {}));
            hosts.list[0].reach = Reach::Online;
            hosts.list[0].info = Some(answer(true, 7));
            hosts.list[0].apps = Apps::Loaded(Arc::new(games.clone()));
            hosts.list[1].reach = Reach::Offline;
            hosts.list[2].reach = Reach::Online;
            hosts.list[2].info = Some(answer(false, 0));
            hosts.list[3].reach = Reach::Unknown;
            let mut f = Frames::over(ctx, l, hosts);
            f.size = vec2(1280.0, 800.0);
            f.native_ppp = 1.5;
            f.settle();
            f
        };
        let shoot = |f: &mut Frames, page: &str| {
            f.settle();
            f.shoot(&dir, &format!("{prefix}{page}-{look}-desktop"));
        };
        let mut f = fresh();
        shoot(&mut f, "hosts");
        let id = f.hosts.list[0].id;
        f.l.go(Screen::Apps(id));
        shoot(&mut f, "library");
        f.l.go(Screen::Settings);
        shoot(&mut f, "settings");
        let mut f = fresh();
        let last = host_options(&f.hosts.list[0]).len() - 1;
        f.press(Key::F2);
        shoot(&mut f, "host-options");
        for _ in 0..last {
            f.press(Key::ArrowDown);
        }
        f.press(Key::Enter);
        shoot(&mut f, "confirm-remove");
        // The smallest window SDL allows: the list scrolls and fades its edge.
        let mut f = fresh();
        f.size = vec2(720.0, 480.0);
        f.settle();
        f.press(Key::F2);
        shoot(&mut f, "host-options-short-window");
    }
    // A long address and GPU end in "…" and the tooltip shows them whole.
    crate::theme::own_palette();
    let ctx = egui::Context::default();
    crate::theme::apply(&ctx);
    let mut l = launcher(false, false);
    l.go(Screen::Hosts);
    let address = "fd12:3456:789a:bcde:f012:3456:789a:bcde";
    let mut hosts = Hosts::without_discovery(vec![HostRecord { address: address.into(), ..record("Living Room PC", true, true) }], Arc::new(|| {}));
    let mut info = answer(true, 0);
    info.gpu_type = Some("AMD Radeon RX 7900 XTX Phantom Gaming OC".into());
    hosts.list[0].reach = Reach::Online;
    hosts.list[0].info = Some(info);
    let mut f = Frames::over(ctx, l, hosts);
    f.size = vec2(720.0, 480.0);
    f.native_ppp = 1.5;
    f.settle();
    let card = f.ctx.read_response(f.ctx.memory(|m| m.focused()).expect("the card has focus")).expect("the card").rect;
    hover_card(&mut f, card);
    f.shoot(&dir, &format!("{prefix}hosts-long-detail-desktop"));
}

/// The glass Home on each tier and every other page once, beside the prototype:
/// `NIGHTJAR_SHOTS=<dir> cargo test -p nightjar-desktop --bin nightjar glass_page_screenshots --release -- --ignored`.
#[test]
#[ignore = "writes PNGs; set NIGHTJAR_SHOTS"]
fn glass_page_screenshots() {
    use crate::live_glass::Tier;
    use rubylight_native_api::library::{Artwork, Game, Library, Store};
    let Some(dir) = std::env::var_os("NIGHTJAR_SHOTS").map(std::path::PathBuf::from) else { return };
    std::fs::create_dir_all(&dir).expect("the folder for the screenshots");
    let library = |tier: Tier, ppp: f32, hero: usize, art_less: &[usize]| {
        crate::theme::own_palette();
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        crate::live_glass::set_tier(&ctx, tier);
        let mut l = launcher(true, false);
        l.identity_ready = true;
        let records = vec![HostRecord { address: "192.0.2.10".into(), ..record("Living Room PC", true, true) }];
        let hosts = Hosts::without_discovery(records, Arc::new(|| {}));
        let mut f = Frames::over(ctx, l, hosts);
        f.size = vec2(1280.0, 800.0);
        f.native_ppp = ppp;
        f.shots = true;
        let games: Vec<Game> = POLISH_GAMES
            .iter()
            .enumerate()
            .map(|(n, (title, store))| Game {
                install_id: format!("install-{n}"),
                app_id: 1000 + n as u32,
                title: (*title).into(),
                game_id: (!store.is_empty()).then(|| format!("{store}:{n}")),
                store: (!store.is_empty()).then(|| Store { source: (*store).into(), id: n.to_string(), name: String::new() }),
                installed: Some(true),
                running: n == hero,
                artwork: Artwork { cover: (!art_less.contains(&n)).then(|| format!("/cover/{n}")), version: "1".into() },
                last_played: None,
                playtime_minutes: None,
                collections: Vec::new(),
            })
            .collect();
        let apps: Vec<nightjar_policy::http::App> = games
            .iter()
            .map(|g| nightjar_policy::http::App { name: g.title.clone(), uuid: g.install_id.clone(), ..game(g.app_id as i32) })
            .collect();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("the clock").as_secs();
        let pc = &mut f.hosts.list[0];
        pc.reach = Reach::Online;
        pc.info = Some(files::tests::native_answer());
        pc.native.library = Some(Arc::new(Library { revision: "r1".into(), games }));
        pc.apps = Apps::Loaded(Arc::new(apps.clone()));
        for (n, app) in apps.iter().enumerate() {
            if art_less.contains(&n) {
                Arc::make_mut(&mut pc.art).insert(app.id, Art::Missing);
                continue;
            }
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../app/src/debug/assets/demo/art/{}.jpg", n + 1));
            let image = image::load_from_memory(&std::fs::read(&path).expect("the demo cover")).expect("a JPEG").to_rgba8();
            let image = egui::ColorImage::from_rgba_unmultiplied([image.width() as usize, image.height() as usize], &image);
            let ambient_img = crate::ambient::bake(&image);
            Arc::make_mut(&mut pc.ambient_glow).insert(app.id, crate::ambient::glow_of(&ambient_img));
            let ambient_tex = f.ctx.load_texture(format!("ambient-{n}"), ambient_img, egui::TextureOptions::LINEAR);
            let cover_tex = f.ctx.load_texture(format!("cover-{n}"), image.clone(), egui::TextureOptions::LINEAR);
            crate::ambient::register_cover(&f.ctx, Some(cover_tex.id()), Some(ambient_tex.id()), &image);
            Arc::make_mut(&mut pc.ambient).insert(app.id, ambient_tex);
            Arc::make_mut(&mut pc.art).insert(app.id, Art::Ready(cover_tex));
        }
        let uuid = f.hosts.list[0].record.unique_id.clone();
        for (n, days) in [(2u32, 1u64), (8, 3), (3, 8)] {
            f.l.recents.push(crate::config::Recent { host: uuid.clone(), install_id: format!("install-{n}"), app_id: 1000 + n, title: POLISH_GAMES[n as usize].0.into(), at: now - days * 86400 });
        }
        f.l.go(Screen::Home);
        f
    };
    let at_rest = |f: &mut Frames| {
        for _ in 0..16 {
            f.settle();
        }
    };
    for (name, tier, ppp) in [("home-live", Tier::Live, 1.0), ("home-baked", Tier::Baked, 1.0), ("home-solid", Tier::Solid, 1.0), ("home-150", Tier::Live, 1.5)] {
        let mut f = library(tier, ppp, 1, &[]);
        at_rest(&mut f);
        f.shoot(&dir, name);
    }
    // A dialog over Home: the page blurred under the dim on the live tier, the dim alone on baked.
    for (name, tier) in [("dialog-live", Tier::Live), ("dialog-baked", Tier::Baked)] {
        let mut f = library(tier, 1.0, 1, &[]);
        at_rest(&mut f);
        f.l.add_host_dialog();
        at_rest(&mut f);
        f.shoot(&dir, name);
    }
    // A running game with no art, one on a shelf too: each game's own tile.
    let mut f = library(Tier::Live, 1.0, 18, &[18, 11, 13]);
    at_rest(&mut f);
    f.shoot(&dir, "home-noart");
    // Down to the shelves: the focused cover lifts.
    let mut f = library(Tier::Live, 1.0, 1, &[]);
    at_rest(&mut f);
    for _ in 0..3 {
        f.press(Key::ArrowDown);
    }
    at_rest(&mut f);
    f.shoot(&dir, "home-focus-cover");
    let pc = f.hosts.list[0].id;
    for (name, screen) in [
        ("pcs", Screen::Hosts),
        ("settings", Screen::Settings),
        ("help", Screen::Help),
        ("all-games", Screen::AllGames),
        ("game", Screen::Game { host: pc, app: 1002 }),
        ("apps", Screen::Apps(pc)),
    ] {
        f.l.go(screen);
        at_rest(&mut f);
        f.shoot(&dir, name);
    }
    f.l.go(Screen::Home);
    f.l.open_search();
    f.l.library_query = "ocean".into();
    at_rest(&mut f);
    f.shoot(&dir, "search");
    // The Theme setting's other looks: by day (System on a light system) and OLED black.
    for (look, theme, light) in [("day", crate::config::Theme::System, true), ("oled", crate::config::Theme::Oled, false)] {
        let mut f = library(Tier::Live, 1.0, 1, &[]);
        f.l.settings.theme = theme;
        f.l.system_light = light;
        at_rest(&mut f);
        f.shoot(&dir, &format!("home-{look}"));
        let pc = f.hosts.list[0].id;
        for (name, screen) in [("pcs", Screen::Hosts), ("settings", Screen::Settings), ("game", Screen::Game { host: pc, app: 1002 }), ("all-games", Screen::AllGames)] {
            f.l.go(screen);
            at_rest(&mut f);
            f.shoot(&dir, &format!("{name}-{look}"));
        }
        f.l.go(Screen::Home);
        at_rest(&mut f);
        f.l.add_host_dialog();
        at_rest(&mut f);
        f.shoot(&dir, &format!("dialog-{look}"));
    }
    // By day on the baked tier, whose glass samples the ambient instead of the page.
    let mut f = library(Tier::Baked, 1.0, 1, &[]);
    f.l.settings.theme = crate::config::Theme::System;
    f.l.system_light = true;
    at_rest(&mut f);
    f.shoot(&dir, "home-day-baked");
}

/// All 36 games of the Android demo's library have a cover of their own that this app's image
/// decoder reads. Covers carry their titles, so one repeated would show the wrong name.
#[test]
fn demo_covers_decode_and_none_repeats() {
    let art = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../app/src/debug/assets/demo/art");
    let mut seen = std::collections::HashSet::new();
    for n in 1..=36 {
        let bytes = std::fs::read(art.join(format!("{n}.jpg"))).expect("the demo cover");
        image::load_from_memory(&bytes).expect("a JPEG");
        assert!(seen.insert(bytes), "cover {n} repeats another");
    }
}

/// A showcase game shows art/N.jpg of its own place in the demo library, so no game wears another's
/// cover and none stays empty: its title must be the one the demo keeps at that place.
#[test]
fn showcase_games_sit_at_their_own_covers_place() {
    let java = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../app/src/debug/java/com/limelight/demo/DemoConsole.java")).expect("the demo library");
    let library = java.split("LIBRARY = {").nth(1).expect("the list").split("};").next().expect("its end");
    let demo: Vec<&str> = library.lines().filter_map(|line| line.trim().strip_prefix("{\"")?.split('"').next()).collect();
    for (n, (title, _)) in POLISH_GAMES.iter().enumerate() {
        assert_eq!(Some(title), demo.get(n), "place {n}");
    }
}

/// Every main page and state as a first-time and a returning user meet them, at 1280x800 (150 %)
/// and 1920x1080: `NIGHTJAR_SHOTS=<dir> cargo test -p nightjar-desktop polish_screenshots -- --ignored`.
#[test]
#[ignore = "writes PNGs; set NIGHTJAR_SHOTS"]
fn polish_screenshots() {
    use rubylight_native_api::library::{Artwork, Game, Library, Store};
    let Some(dir) = std::env::var_os("NIGHTJAR_SHOTS").map(std::path::PathBuf::from) else { return };
    std::fs::create_dir_all(&dir).expect("the folder for the screenshots");
    let only = std::env::var("NIGHTJAR_SHOTS_ONLY").unwrap_or_default();
    let titles = POLISH_GAMES;
    for (width, height, ppp) in [(1280.0, 800.0, 1.5), (1920.0, 1080.0, 1.0)] {
        let size = format!("{}x{}", width as u32, height as u32);
        let fresh = |records: Vec<HostRecord>, home: bool| {
            crate::theme::own_palette();
            let ctx = egui::Context::default();
            crate::theme::apply(&ctx);
            let mut l = launcher(home, false);
            l.identity_ready = true;
            let hosts = Hosts::without_discovery(records, Arc::new(|| {}));
            let mut f = Frames::over(ctx, l, hosts);
            f.size = vec2(width, height);
            f.native_ppp = ppp;
            f.shots = true;
            f
        };
        let shoot = |f: &mut Frames, page: &str| {
            if !only.is_empty() && !only.split(',').any(|o| page.starts_with(o)) {
                return;
            }
            f.settle();
            f.settle();
            f.shoot(&dir, &format!("{page}-{size}"));
        };
        // First run: nothing found yet.
        let mut f = fresh(Vec::new(), true);
        f.l.go(Screen::Home);
        shoot(&mut f, "first-run-home");
        f.l.go(Screen::Hosts);
        shoot(&mut f, "first-run-pcs");
        f.l.add_host_dialog();
        shoot(&mut f, "dialog-add-pc");

        // A returning user: a PC with a library, one offline, one to pair.
        let records = vec![
            HostRecord { address: "192.0.2.10".into(), ..record("Living Room PC", true, true) },
            HostRecord { address: "192.0.2.11".into(), ..record("Office PC", true, true) },
            HostRecord { address: "192.0.2.12".into(), ..record("Studio", false, false) },
        ];
        let mut f = fresh(records, true);
        let games: Vec<Game> = titles
            .iter()
            .enumerate()
            .map(|(n, (title, store))| Game {
                install_id: format!("install-{n}"),
                app_id: 1000 + n as u32,
                title: (*title).into(),
                game_id: (!store.is_empty()).then(|| format!("{store}:{n}")),
                store: (!store.is_empty()).then(|| Store { source: (*store).into(), id: n.to_string(), name: String::new() }),
                installed: Some(*title != "Frostline"),
                running: n == 1,
                artwork: Artwork { cover: Some(format!("/cover/{n}")), version: "1".into() },
                last_played: None,
                playtime_minutes: None,
                collections: Vec::new(),
            })
            .collect();
        let apps: Vec<nightjar_policy::http::App> = games
            .iter()
            .map(|g| nightjar_policy::http::App { name: g.title.clone(), uuid: g.install_id.clone(), ..game(g.app_id as i32) })
            .collect();
        {
            let pc = &mut f.hosts.list[0];
            pc.reach = Reach::Online;
            pc.info = Some(files::tests::native_answer());
            pc.native.library = Some(Arc::new(Library { revision: "r1".into(), games }));
            pc.apps = Apps::Loaded(Arc::new(apps.clone()));
            for (n, app) in apps.iter().enumerate() {
                let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../app/src/debug/assets/demo/art/{}.jpg", n + 1));
                let raw = image::load_from_memory(&std::fs::read(&path).expect("the demo cover")).expect("a JPEG").to_rgba8();
                let img = egui::ColorImage::from_rgba_unmultiplied([raw.width() as usize, raw.height() as usize], &raw);
                let amb = crate::ambient::bake(&img);
                Arc::make_mut(&mut pc.ambient_glow).insert(app.id, crate::ambient::glow_of(&amb));
                let amb_tex = f.ctx.load_texture(format!("polish-amb-{n}"), amb, egui::TextureOptions::LINEAR);
                let art_tex = f.ctx.load_texture(format!("polish-art-{n}"), img.clone(), egui::TextureOptions::LINEAR);
                crate::ambient::register_cover(&f.ctx, Some(art_tex.id()), Some(amb_tex.id()), &img);
                Arc::make_mut(&mut pc.ambient).insert(app.id, amb_tex);
                Arc::make_mut(&mut pc.art).insert(app.id, Art::Ready(art_tex));
            }
        }
        f.hosts.list[1].reach = Reach::Offline;
        f.hosts.list[2].reach = Reach::Online;
        f.hosts.list[2].info = Some(answer(false, 0));
        let pc = f.hosts.list[0].id;
        f.l.go(Screen::Home);
        shoot(&mut f, "home");
        f.press(Key::ArrowRight);
        shoot(&mut f, "home-focus-moved");
        // A little way down: the page passes under the top bar's frosted band.
        f.frame(vec![egui::Event::PointerMoved((f.size / 2.0).to_pos2())]);
        f.frame(vec![egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: vec2(0.0, -260.0),
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Unknown,
            modifiers: egui::Modifiers::NONE,
        }]);
        shoot(&mut f, "home-scrolled");
        f.scroll_to_end();
        shoot(&mut f, "home-end");
        // Home's View: its menu, then each layout, grouping and card size.
        f.l.go(Screen::Home);
        f.settle();
        if let Some(view) = f.placed.iter().find(|(t, ..)| t == "View").map(|p| p.1) {
            f.click_at(view.center(), egui::PointerButton::Primary);
            shoot(&mut f, "home-view-menu");
            f.press(Key::Escape);
        }
        for (layout, group, card) in [
            (HomeLayout::Grid, HomeGroup::Store, CardSize::Medium),
            (HomeLayout::Grid, HomeGroup::None, CardSize::Small),
            (HomeLayout::Rows, HomeGroup::None, CardSize::Large),
            (HomeLayout::Rows, HomeGroup::Store, CardSize::Small),
        ] {
            (f.l.settings.home_layout, f.l.settings.home_group, f.l.settings.home_card_size) = (layout, group, card);
            let name = format!("home-view-{}-{}-{}", layout.label(), group.label(), card.label()).to_lowercase();
            shoot(&mut f, &name);
        }
        (f.l.settings.home_layout, f.l.settings.home_group, f.l.settings.home_card_size) = (HomeLayout::Rows, HomeGroup::Store, CardSize::Medium);
        // Help as a returning user meets it, Home in the top bar (the README's shot).
        f.l.go(Screen::Help);
        shoot(&mut f, "help-returning");
        // All games with every game, then with a store's chip clicked.
        f.l.open(Screen::AllGames);
        shoot(&mut f, "all-games");
        f.settle();
        let steam = f.placed.iter().find(|(t, r, _)| t == "Steam" && r.center().y < 200.0).map(|p| p.1).expect("the Steam chip");
        f.click_at(steam.center(), egui::PointerButton::Primary);
        shoot(&mut f, "all-games-store");
        f.l.library_filter = console::LibraryFilter::All;
        f.frame(vec![egui::Event::PointerGone]);
        f.settle();
        // A cover's menu from its ⋮ (the first cover, at the left edge) and from a right-click on
        // the last cover of the first line (at the right edge).
        f.l.focused_key = None;
        f.l.restore_key = None;
        f.l.focus_screen = true;
        f.settle();
        let cover = f.ctx.memory(|m| m.focused()).expect("the first cover has focus");
        let first = f.ctx.read_response(cover).expect("the cover").rect;
        f.frame(vec![egui::Event::PointerMoved(first.center())]);
        f.settle();
        let dots = f.ctx.read_response(cover.with("options")).expect("the cover's ⋮").rect;
        f.click_at(dots.center(), egui::PointerButton::Primary);
        shoot(&mut f, "game-menu-dots");
        f.l.close_dialog();
        // Every cover of the line draws its title inside its own width: the rightmost title is the last cover's.
        let right = f
            .placed
            .iter()
            .filter(|(_, r, _)| r.center().y > first.top() && r.center().y < first.bottom())
            .map(|p| p.1.center().x)
            .max_by(f32::total_cmp)
            .map(|x| pos2(x, first.center().y))
            .expect("the first line's titles");
        f.click_at(right + vec2(first.width() * 0.25, 0.0), egui::PointerButton::Secondary);
        shoot(&mut f, "game-menu-right-click");
        f.l.close_dialog();
        // A cover under the pointer with the focus elsewhere: it rises, no ring.
        // (Closing the menu gave the focus back to the cover it was opened on.)
        f.settle();
        if let Some(held) = f.ctx.memory(|m| m.focused()) {
            f.ctx.memory_mut(|m| m.surrender_focus(held));
        }
        f.frame(vec![egui::Event::PointerMoved(right)]);
        f.settle();
        shoot(&mut f, "all-games-cover-hover");
        // The same in high contrast with the first cover focused: one amber ring.
        f.l.settings.high_contrast = true;
        f.frame(vec![egui::Event::PointerGone]);
        f.ctx.memory_mut(|m| m.request_focus(cover));
        shoot(&mut f, "all-games-focus-high-contrast");
        f.l.go(Screen::Hosts);
        shoot(&mut f, "pcs-focus-high-contrast");
        f.l.settings.high_contrast = false;
        // A PC's card under the pointer with no focus on it: one step lighter, no outline.
        let card = f.ctx.memory(|m| m.focused()).and_then(|id| f.ctx.read_response(id)).expect("the first PC has focus");
        f.ctx.memory_mut(|m| m.surrender_focus(card.id));
        hover_card(&mut f, card.rect);
        shoot(&mut f, "pcs-card-hover");
        f.frame(vec![egui::Event::PointerGone]);
        f.l.go(Screen::Game { host: pc, app: 1001 });
        shoot(&mut f, "game-running");
        f.l.go(Screen::Game { host: pc, app: 1002 });
        shoot(&mut f, "game");
        // Frostline, which the PC has not installed: no Play.
        f.l.go(Screen::Game { host: pc, app: 1015 });
        shoot(&mut f, "game-not-installed");
        f.l.go(Screen::Hosts);
        shoot(&mut f, "pcs");
        // A PC's card with the last game played there on its screen.
        f.l.recents = vec![crate::config::Recent { host: "Living Room PC".into(), install_id: "install-3".into(), app_id: 1003, title: titles[3].0.into(), at: 1 }];
        shoot(&mut f, "pcs-covers");
        f.l.recents.clear();
        f.l.go(Screen::Apps(pc));
        shoot(&mut f, "library");
        f.l.go(Screen::HostDetails(pc));
        shoot(&mut f, "pc-details");
        f.l.go(Screen::Settings);
        shoot(&mut f, "settings");
        f.scroll_to_end();
        shoot(&mut f, "settings-end");
        // A controller's mapping, as after choosing it: Connect as leads its rows.
        f.l.go(Screen::Controllers);
        f.l.controller_key = "demo-pad".into();
        f.l.controller_mapping = crate::gamepad_types::Mapping { name: "Xbox Wireless Controller".into(), ..Default::default() };
        shoot(&mut f, "controllers");
        // Its Motion rows, below the stick axes: the gyroscope's axes and sensitivity.
        let middle = (f.size / 2.0).to_pos2();
        wheel(&mut f, middle, -720.0);
        shoot(&mut f, "controllers-motion");
        f.l.go(Screen::Hosts);
        f.settle();
        f.press(Key::F2);
        shoot(&mut f, "dialog-pc-options");
        f.l.close_dialog();
        let studio = f.hosts.list[2].id;
        f.l.dialog = Dialog::Pairing { host: studio, pin: "4071".into() };
        f.l.focus_dialog = true;
        shoot(&mut f, "dialog-pairing");
        f.l.pair_failed(studio, crate::hosts::PAIR_PIN_WRONG, false);
        shoot(&mut f, "dialog-pair-failed");
        f.l.close_dialog();
        f.l.dialog = Dialog::ConfirmRemove { host: studio, name: "Studio".into() };
        f.l.focus_dialog = true;
        shoot(&mut f, "dialog-remove");
        f.l.close_dialog();
        f.l.dialog = Dialog::Rename { host: studio, text: String::new() };
        f.l.focus_dialog = true;
        shoot(&mut f, "dialog-rename");
        f.l.close_dialog();
        f.l.dialog = Dialog::ConfirmSleep { host: pc, ask: 1, name: "Living Room PC".into(), offer: None };
        f.l.focus_dialog = true;
        shoot(&mut f, "dialog-sleep-asking");
        f.l.sleep_answered(pc, 1, "Living Room PC", false, Ok(SleepStep::Offer { token: "t".into(), at: Handed::now() }));
        shoot(&mut f, "dialog-sleep-offered");
        f.l.close_dialog();
        // Connection test: running, then its result with the speed (a Rubylight PC) and with the
        // delay alone (a PC without a speed test).
        f.l.ask(Dialog::NetworkTest { stage: "Measuring ping, jitter and loss".into(), calibrate: false });
        shoot(&mut f, "dialog-connection-test");
        let measured = nightjar_policy::connection_test::Speed::Measured { mbps: 912.6, link_mbps: 2500 };
        let result = crate::network_test::Connection { latency_ms: Some(4.2), jitter_ms: Some(0.8), loss_percent: Some(0.0), speed: Some(measured) };
        f.l.ask(Dialog::ConnectionTest { host: pc, result });
        shoot(&mut f, "dialog-connection-test-result");
        f.l.ask(Dialog::ConnectionTest { host: pc, result: crate::network_test::Connection { latency_ms: Some(23.0), jitter_ms: Some(6.0), loss_percent: Some(10.0), speed: None } });
        shoot(&mut f, "dialog-connection-test-delay");
        f.l.close_dialog();
        f.waking = Some(WakingView::new("Office PC"));
        shoot(&mut f, "waking");
        f.waking = None;
        // The launch scene: the game's cover in its ambient with its ring, then the card of a
        // start that failed; then the stream menu over a game, with More open.
        let launch = ConnectingView { app: apps[0].name.clone(), host: "Living Room PC".into(), stage: "Checking network profile".into(), host_id: pc, app_id: apps[0].id };
        f.connecting = Some(launch.clone());
        shoot(&mut f, "connecting");
        f.connecting = None;
        f.l.start_failed(launch, "The PC did not answer. Check that it is on and on this network.", Some(settings_line((1920, 1080, 60, 20_000))));
        shoot(&mut f, "connect-failed");
        f.l.close_dialog();
        for (menu, name) in [(demo::DemoMenu::Grid, "stream-menu"), (demo::DemoMenu::More, "stream-menu-more")] {
            f.l.demo_menu = Some(menu);
            shoot(&mut f, name);
        }
        f.l.demo_menu = None;
        // Home's rows in one column, drawn alone as `at_one_column_a_long_row_is_its_all_tile`
        // does (the top bar keeps the window wider): a row too long to fit is its All tile.
        if only.is_empty() || only.split(',').any(|o| "home-one-column".starts_with(o)) {
            let column = vec2(200.0, height);
            let mut shapes = Vec::new();
            for _ in 0..6 {
                f.time += 0.05;
                let mut input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, column)), time: Some(f.time), focused: true, ..Default::default() };
                input.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(f.native_ppp);
                let mut commands = Vec::new();
                let output = f.ctx.run_ui(input, |ui| {
                    let view = View { hosts: &f.hosts, pads: &f.pads, connecting: None, waking: None };
                    egui::CentralPanel::default().show(ui, |ui| draw_home(&mut f.l, ui, &view, &mut commands));
                });
                f.textures.append(output.textures_delta);
                shapes = output.shapes;
            }
            let (window, page) = (f.size, std::mem::replace(&mut f.shapes, shapes));
            f.size = column;
            f.shoot(&dir, &format!("home-one-column-{size}"));
            (f.size, f.shapes) = (window, page);
        }
        // All games of a PC whose library is empty: no chips, and the page says why. A chip exists
        // only for games it shows, so no filter is ever empty on its own.
        let mut f = fresh(vec![HostRecord { address: "192.0.2.10".into(), ..record("Living Room PC", true, true) }], true);
        f.hosts.list[0].reach = Reach::Online;
        f.hosts.list[0].info = Some(files::tests::native_answer());
        f.hosts.list[0].native.library = Some(Arc::new(Library { revision: "r1".into(), games: Vec::new() }));
        f.l.open(Screen::AllGames);
        shoot(&mut f, "all-games-empty");
        // Nothing installed: the Installed chip is offered and shows no game.
        let none = |title: &str, n: u32| Game {
            install_id: format!("install-{n}"),
            app_id: 1000 + n,
            title: title.into(),
            game_id: Some(format!("steam:{n}")),
            store: Some(Store { source: "steam".into(), id: n.to_string(), name: String::new() }),
            installed: Some(false),
            running: false,
            artwork: Artwork { cover: None, version: String::new() },
            last_played: None,
            playtime_minutes: None,
            collections: Vec::new(),
        };
        f.hosts.list[0].native.library = Some(Arc::new(Library { revision: "r2".into(), games: vec![none("Night Drive", 0), none("Farm Life", 1)] }));
        f.l.library_filter = console::LibraryFilter::Installed;
        shoot(&mut f, "all-games-empty-filter");
        // The library still on its way: the spinner Home shows.
        f.hosts.list[0].native.library = None;
        shoot(&mut f, "all-games-loading");
    }
}
