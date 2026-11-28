//! Unit tests for box art cover loading, ambient baking, memory budgeting and sharpening.

use super::*;

pub(super) fn png(color: [u8; 3]) -> Vec<u8> {
    png_of([30, 40], color)
}

/// A PNG of one colour and `size` pixels: 600 by 900 is the cover a store hands out.
fn png_of(size: [u32; 2], color: [u8; 3]) -> Vec<u8> {
    let image = image::RgbImage::from_pixel(size[0], size[1], image::Rgb(color));
    let mut bytes = std::io::Cursor::new(Vec::new());
    image.write_to(&mut bytes, image::ImageFormat::Png).expect("a PNG");
    bytes.into_inner()
}

/// A waker that tells the test of each wake, to wait for a cover loaded on the art threads.
fn telling_waker() -> (Waker, Receiver<()>) {
    let (tx, rx) = channel();
    let tx = Mutex::new(tx);
    let waker: Waker = Arc::new(move || {
        let _ = tx.lock().unwrap().send(());
    });
    (waker, rx)
}

/// An empty settings folder of this test's own.
pub(super) fn art_store(name: &str) -> (Store, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("nightjar-covers-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    (Store::at(dir.clone()).expect("a settings folder"), dir)
}

fn cover_file(store: &Store, app: i32, bytes: &[u8]) -> std::path::PathBuf {
    let path = store.art_path("uid", app, "v1");
    std::fs::create_dir_all(path.parent().expect("the PC's folder")).expect("the art cache");
    std::fs::write(&path, bytes).expect("the cover's file");
    path
}

fn cover_pixels(pc: &Host, app: i32) -> Option<[usize; 2]> {
    match pc.art.get(&app) {
        Some(Art::Ready(texture)) => Some(texture.size()),
        _ => None,
    }
}

/// Frees the textures a test loaded, as the renderer would.
fn end_pass(ctx: &egui::Context) {
    let mut output = ctx.run_ui(egui::RawInput::default(), |_| {});
    output.textures_delta.clear();
}

#[test]
fn a_cover_arrives_with_its_ambient_already_baked() {
    let (image, ambient, prepared, sized) = art_and_ambient(Some(png([200, 20, 20])), 1.0);
    assert_eq!(image.expect("the cover").size, [30, 40]);
    assert!(prepared.is_some(), "hero wash and thumbnail are prepared off the UI thread");
    assert_eq!(sized, None, "a small cover is never made larger");
    let ambient = ambient.expect("baked with the cover, off the UI thread");
    assert_eq!(ambient, crate::ambient::bake(&decode_image(&png([200, 20, 20])).expect("the cover")));
    assert_eq!(art_and_ambient(None, 1.0), (None, None, None, None), "no cover, no ambient");
    assert_eq!(art_and_ambient(Some(b"not an image".to_vec()), 1.0), (None, None, None, None));
}

/// The game page draws a cover 420 by 630 points and nothing draws one larger, so that many
/// points at the window's pixels per point, and half as many again to stay sharp where it
/// is drawn smaller, is all of a cover that is kept.
#[test]
fn a_cover_is_kept_no_larger_than_the_window_needs() {
    assert_eq!(cover_size([1000, 1500], 1.0), Some([630, 945]), "a large cover on a plain screen");
    assert_eq!(cover_size([900, 1350], 1.0), Some([630, 945]));
    assert_eq!(cover_size([900, 1350], 1.25), Some([788, 1182]));
    assert_eq!(cover_size([2000, 3000], 2.0), Some([1260, 1890]), "twice the pixels on a dense screen");
    assert_eq!(cover_size([4000, 6000], 4.5), Some([2835, 4253]), "150 % text on a 300 % screen");
    // Nothing to win, or too little to pay for another resampling of the picture.
    assert_eq!(cover_size([600, 900], 1.0), None, "a store's cover stays as its file has it");
    assert_eq!(cover_size([900, 1350], 1.5), None, "every pixel of it is kept");
    assert_eq!(cover_size([680, 1020], 1.0), None, "nine tenths of its side and more are kept whole");
    assert_eq!(cover_size([30, 40], 1.0), None, "never made larger");
    // A cover fills its place and is cut to it: the side that needs the most decides.
    assert_eq!(cover_size([920, 430], 1.0), None, "a wide banner is short of the place's height as it is");
    assert_eq!(cover_size([3840, 2160], 1.0), Some([1680, 945]), "a screenshot keeps the full height");
    assert_eq!(cover_size([1000, 6000], 1.0), Some([630, 3780]), "and a strip the full width");
    assert_eq!(cover_size([0, 900], 1.0), None);
    assert_eq!(cover_size([600, 0], 1.0), None);
    // The sizes the pages draw: the cards and the PC tile are all below the game page's.
    assert_eq!(COVER_POINTS, [420.0, 630.0]);
}

#[test]
fn a_cover_made_smaller_keeps_its_picture_and_its_ambient() {
    let bytes = png_of([900, 1350], [200, 20, 20]);
    let full = decode_image(&bytes).expect("the cover");
    let (image, ambient, prepared, sized) = art_and_ambient(Some(bytes), 1.0);
    let image = image.expect("the cover");
    assert!(prepared.is_some());
    assert_eq!(image.size, [630, 945]);
    assert_eq!(sized, Some(1.0), "the pixels per point it is good for");
    assert!(image.pixels.iter().all(|&pixel| pixel == egui::Color32::from_rgb(200, 20, 20)), "one colour stays that colour");
    assert_eq!(ambient.expect("the ambient"), crate::ambient::bake(&full), "baked from the cover as its file has it");
    // On a screen that shows every pixel it is handed over as decoded.
    let (same, sized) = cover_sized(full.clone(), 2.0);
    assert_eq!((same, sized), (full, None));
}

/// The pixels are multiplied by their alpha before they are resampled, so the colour a clear
/// pixel happens to have does not show as a fringe around the picture.
#[test]
fn a_clear_part_of_a_cover_does_not_bleed_into_the_picture_when_it_is_made_smaller() {
    let picture = image::RgbaImage::from_fn(900, 1350, |x, _| {
        if x < 450 { image::Rgba([200, 0, 0, 255]) } else { image::Rgba([0, 255, 0, 0]) }
    });
    let (small, sized) = cover_sized(egui::ColorImage::from_rgba_unmultiplied([900, 1350], picture.as_raw()), 1.0);
    assert_eq!((small.size, sized), ([630, 945], Some(1.0)));
    assert!(small.pixels.iter().all(|pixel| pixel.g() == 0), "no green from the clear half");
    assert_eq!(small.pixels[0], egui::Color32::from_rgb(200, 0, 0));
    assert_eq!(small.pixels[629], egui::Color32::TRANSPARENT);
}

#[test]
fn the_ambient_lands_with_its_cover_and_goes_with_it() {
    let ctx = egui::Context::default();
    let mut hosts = Hosts::without_discovery(vec![paired_record("uid")], Arc::new(|| {}));
    let id = hosts.list[0].id;
    Arc::make_mut(&mut hosts.list[0].art_versions).insert(7, "v1".into());
    let art = |version: &str, color: Option<[u8; 3]>| {
        let (image, ambient, prepared, sized) = art_and_ambient(color.map(png), 1.0);
        Event::Art { id, app: 7, version: version.into(), image, ambient, prepared, sized }
    };
    hosts.sender.send(art("v1", Some([20, 200, 20]))).expect("the channel");
    hosts.drain(&ctx);
    assert!(hosts.list[0].ambient.contains_key(&7));
    assert!(hosts.list[0].ambient_glow[&7].a() > 0, "its glow lands with it");
    let Some(Art::Ready(art_tex)) = hosts.list[0].art.get(&7) else { panic!("expected Ready art") };
    let amb_id = hosts.list[0].ambient.get(&7).map(egui::TextureHandle::id);
    assert!(crate::ambient::hero_for(&ctx, art_tex.id(), amb_id, None, false).is_some(), "prepared hero wash registered with cover");

    hosts.sender.send(art("old", None)).expect("the channel");
    hosts.drain(&ctx);
    assert!(hosts.list[0].ambient.contains_key(&7), "an answer for an older cover changes nothing");

    // The PC's empty answer landing after a SteamGridDB cover of the same version leaves it.
    hosts.sender.send(art("v1", None)).expect("the channel");
    hosts.drain(&ctx);
    assert!(matches!(hosts.list[0].art.get(&7), Some(Art::Ready(_))), "no cover never replaces one that landed");
    assert!(hosts.list[0].ambient.contains_key(&7));

    // Loaded again (given up for room) and found gone.
    Arc::make_mut(&mut hosts.list[0].art).insert(7, Art::Loading);
    hosts.sender.send(art("v1", None)).expect("the channel");
    hosts.drain(&ctx);
    assert!(!hosts.list[0].ambient.contains_key(&7), "no cover any more, no ambient of it");
    assert!(!hosts.list[0].ambient_glow.contains_key(&7), "nor its glow");
    let mut output = ctx.run_ui(egui::RawInput::default(), |_| {});
    output.textures_delta.clear();
}

/// Linux's switch to the GPU renderer: covers on the old renderer are dropped, so they load
/// again from the art cache; no copy of them is kept.
#[test]
fn a_new_renderer_drops_the_covers_so_they_load_again() {
    let ctx = egui::Context::default();
    let mut hosts = Hosts::without_discovery(vec![paired_record("uid")], Arc::new(|| {}));
    let id = hosts.list[0].id;
    Arc::make_mut(&mut hosts.list[0].art_versions).insert(7, "v1".into());
    let (image, ambient, prepared, sized) = art_and_ambient(Some(png([20, 200, 20])), 1.0);
    hosts.sender.send(Event::Art { id, app: 7, version: "v1".into(), image, ambient, prepared, sized }).expect("the channel");
    hosts.drain(&ctx);
    Arc::make_mut(&mut hosts.list[0].art).insert(8, Art::Missing);
    Arc::make_mut(&mut hosts.list[0].art).insert(9, Art::Loading);
    Arc::make_mut(&mut hosts.list[0].art).insert(10, Art::Dropped);
    hosts.renderer_changed();
    let pc = &hosts.list[0];
    assert!(!pc.art.contains_key(&7) && !pc.ambient.contains_key(&7), "asked for again, from the cache");
    assert!(pc.ambient_glow.contains_key(&7) && pc.art_versions.contains_key(&7), "no texture: kept");
    assert!(matches!(pc.art.get(&8), Some(Art::Missing)), "no cover is still no cover");
    assert!(matches!(pc.art.get(&9), Some(Art::Loading)), "one on its way lands as it is");
    assert!(pc.art_wanted(10, ""), "one given up for room is asked for again as before");
    assert!(hosts.art_used.is_empty(), "the old renderer's textures are gone");
    end_pass(&ctx);
}

/// A cover whose download failed (the PC busy or slow) is a title for a while, then asked for
/// again and lands; one the PC has none of stays a title.
#[test]
fn a_cover_whose_download_failed_is_asked_for_again_after_a_wait() {
    let ctx = egui::Context::default();
    let mut hosts = Hosts::without_discovery(vec![paired_record("uid")], Arc::new(|| {}));
    let id = hosts.list[0].id;
    for app in [7, 8] {
        assert_eq!(hosts.list[0].begin_art(app, "v1").as_deref(), Some("v1"));
    }
    hosts.sender.send(Event::ArtFailed { id, app: 7, version: "v1".into() }).expect("the channel");
    hosts.sender.send(Event::Art { id, app: 8, version: "v1".into(), image: None, ambient: None, prepared: None, sized: None }).expect("the channel");
    hosts.drain(&ctx);
    let pc = &hosts.list[0];
    assert!(matches!(pc.art.get(&7), Some(Art::Missing)), "the title shows meanwhile");
    assert!(!pc.art_wanted(7, "v1"), "not asked for again at once");
    assert!(!pc.art_wanted(8, "v1"), "no cover is still no cover");

    Arc::make_mut(&mut hosts.list[0].art_retry).insert(7, Instant::now() - Duration::from_millis(1));
    assert!(hosts.list[0].art_wanted(7, "v1"), "the wait is over");
    assert!(hosts.list[0].art_wanted(7, ""), "and pages that ask without a version ask too");
    assert!(!hosts.list[0].art_wanted(8, "v1"));
    assert_eq!(hosts.list[0].begin_art(7, "v1").as_deref(), Some("v1"));
    assert!(matches!(hosts.list[0].art.get(&7), Some(Art::Loading)) && !hosts.list[0].art_retry.contains_key(&7));
    let (image, ambient, prepared, sized) = art_and_ambient(Some(png([20, 200, 20])), 1.0);
    hosts.sender.send(Event::Art { id, app: 7, version: "v1".into(), image, ambient, prepared, sized }).expect("the channel");
    hosts.drain(&ctx);
    assert!(matches!(hosts.list[0].art.get(&7), Some(Art::Ready(_))));
    end_pass(&ctx);
}

/// Four covers of 30 by 40 pixels on one PC, landed and then shown for as many frames as
/// `shown` lists, each frame drawing the apps it names.
fn covers_shown_in(name: &str, shown: &[&[i32]]) -> (Hosts, egui::Context, Store, std::path::PathBuf) {
    let ctx = egui::Context::default();
    let (store, dir) = art_store(name);
    let mut hosts = Hosts::without_discovery(vec![paired_record("uid")], Arc::new(|| {}));
    let id = hosts.list[0].id;
    for app in 1..=4 {
        Arc::make_mut(&mut hosts.list[0].art_versions).insert(app, "v1".into());
        let (image, ambient, prepared, sized) = art_and_ambient(Some(png([20, 200, 20])), 1.0);
        hosts.sender.send(Event::Art { id, app, version: "v1".into(), image, ambient, prepared, sized }).expect("the channel");
    }
    hosts.drain(&ctx);
    for frame in shown {
        for app in *frame {
            let Some(Art::Ready(texture)) = hosts.list[0].art.get(app) else { panic!("cover {app} is not there to draw") };
            cover_used(&ctx, texture);
        }
        hosts.covers_shown(&ctx, &store);
    }
    (hosts, ctx, store, dir)
}

const COVER_BYTES: usize = 30 * 40 * 4;

#[test]
fn covers_past_their_budget_are_given_up_longest_off_screen_first() {
    let (mut hosts, ctx, _store, dir) = covers_shown_in("budget", &[&[1, 2, 3, 4], &[2, 3, 4], &[3, 4]]);
    assert!((1..=4).all(|app| cover_pixels(&hosts.list[0], app).is_some()), "all within the real budget");

    // Room for two and a bit: the two longest off screen go, and no more than needed.
    hosts.drop_covers(2 * COVER_BYTES + 100);
    let pc = &hosts.list[0];
    assert!(pc.art_dropped(1) && pc.art_dropped(2), "last shown two frames and one frame ago");
    assert!(cover_pixels(pc, 3).is_some() && cover_pixels(pc, 4).is_some());
    assert_eq!(hosts.art_used.len(), 2, "what is gone is no longer kept count of");
    // What it leaves: the page's backdrop, and the version its file is found by.
    assert!(pc.ambient.contains_key(&1) && pc.ambient_glow.contains_key(&1));
    assert_eq!(pc.art_versions[&1], "v1");

    // A cover that is given up is asked for again where it is drawn, as it was.
    let pc = &mut hosts.list[0];
    assert!(pc.art_wanted(1, "") && pc.art_wanted(1, "v1"));
    assert!(!pc.art_wanted(3, "") && !pc.art_wanted(3, "v1"));
    assert_eq!(pc.begin_art(1, "").as_deref(), Some("v1"));
    assert!(matches!(pc.art.get(&1), Some(Art::Loading)) && !pc.art_wanted(1, "v1"), "asked for once");
    end_pass(&ctx);
    let _ = std::fs::remove_dir_all(dir);
}

/// Dropping what is on screen would have it loaded again at once and something else dropped
/// for it: a page with more covers than the budget holds would load and drop in turns.
#[test]
fn a_cover_on_screen_is_never_given_up() {
    let (mut hosts, ctx, store, dir) = covers_shown_in("on-screen", &[&[1, 2, 3, 4], &[3, 4]]);
    hosts.drop_covers(0);
    let pc = &hosts.list[0];
    assert!(pc.art_dropped(1) && pc.art_dropped(2));
    assert!(cover_pixels(pc, 3).is_some() && cover_pixels(pc, 4).is_some(), "drawn in the frame just shown");
    // A frame that draws neither: no cover landed, so nothing is given up, and the textures
    // stay for the page that is gone back to. The next cover to land makes room with them.
    hosts.covers_shown(&ctx, &store);
    assert!(cover_pixels(&hosts.list[0], 3).is_some() && cover_pixels(&hosts.list[0], 4).is_some());
    hosts.drop_covers(0);
    assert!(hosts.list[0].art_dropped(3) && hosts.list[0].art_dropped(4));
    end_pass(&ctx);
    let _ = std::fs::remove_dir_all(dir);
}

/// A cover lands before the frame it was asked for, and by then its card may have scrolled
/// away. It counts as shown when it lands: without that it would be the first to go, and be
/// asked for and dropped in turns by a page that asks for a cover it does not draw.
#[test]
fn a_cover_that_just_landed_is_not_the_first_to_go() {
    let (mut hosts, ctx, store, dir) = covers_shown_in("landed", &[&[1, 2, 3, 4], &[3, 4]]);
    let id = hosts.list[0].id;
    Arc::make_mut(&mut hosts.list[0].art_versions).insert(5, "v1".into());
    let (image, ambient, prepared, sized) = art_and_ambient(Some(png([20, 20, 200])), 1.0);
    hosts.sender.send(Event::Art { id, app: 5, version: "v1".into(), image, ambient, prepared, sized }).expect("the channel");
    hosts.drain(&ctx);
    // The frame after draws nothing at all.
    hosts.covers_shown(&ctx, &store);
    hosts.drop_covers(3 * COVER_BYTES);
    let pc = &hosts.list[0];
    assert!(pc.art_dropped(1) && pc.art_dropped(2), "the two that were off screen before it came");
    assert!((3..=5).all(|app| cover_pixels(pc, app).is_some()));
    end_pass(&ctx);
    let _ = std::fs::remove_dir_all(dir);
}

/// Its PC may be off by the time a cover scrolls back in. The file it was saved to is all
/// that is read: this PC's address answers nobody.
#[test]
fn a_cover_given_up_comes_back_from_its_file_without_its_pc() {
    let ctx = egui::Context::default();
    let (store, dir) = art_store("back");
    let (waker, woken) = telling_waker();
    let mut hosts = Hosts::without_discovery(vec![paired_record("uid")], waker);
    hosts.set_identity(Arc::new(ClientIdentity::generate().expect("a key")));
    let id = hosts.list[0].id;
    cover_file(&store, 7, &png_of([900, 1350], [20, 200, 20]));
    Arc::make_mut(&mut hosts.list[0].art_versions).insert(7, "v1".into());
    Arc::make_mut(&mut hosts.list[0].art).insert(7, Art::Dropped);
    assert!(hosts.list[0].reach != Reach::Online, "away");

    hosts.request_art(id, 7, "", &store);
    assert!(matches!(hosts.list[0].art.get(&7), Some(Art::Loading)));
    hosts.request_art(id, 7, "", &store);
    woken.recv_timeout(Duration::from_secs(20)).expect("the cover is read from its file");
    hosts.drain(&ctx);
    assert_eq!(cover_pixels(&hosts.list[0], 7), Some([630, 945]), "and sized for the window as before");
    assert_eq!(hosts.list[0].art_sized.get(&7), Some(&1.0));
    assert!(woken.try_recv().is_err(), "asked for twice, loaded once");
    end_pass(&ctx);
    let _ = std::fs::remove_dir_all(dir);
}

/// The file can be gone (the art folder cleared, or a disk too full to save it). An away PC
/// is not asked, which would hold an art thread until the connection gives up, and the cover
/// is not taken as one the PC has none of: it is asked for again once the PC is up.
#[test]
fn a_cover_given_up_whose_file_is_gone_waits_for_its_pc() {
    let ctx = egui::Context::default();
    let (store, dir) = art_store("gone");
    let (waker, woken) = telling_waker();
    let mut hosts = Hosts::without_discovery(vec![paired_record("uid")], waker);
    hosts.set_identity(Arc::new(ClientIdentity::generate().expect("a key")));
    let id = hosts.list[0].id;
    Arc::make_mut(&mut hosts.list[0].art_versions).insert(7, "v1".into());
    Arc::make_mut(&mut hosts.list[0].art).insert(7, Art::Dropped);
    Arc::make_mut(&mut hosts.list[0].ambient_glow).insert(7, egui::Color32::RED);
    hosts.request_art(id, 7, "", &store);
    woken.recv_timeout(Duration::from_secs(20)).expect("its file is looked for");
    hosts.drain(&ctx);
    let pc = &hosts.list[0];
    assert!(!pc.art.contains_key(&7), "neither a cover nor known to have none");
    assert!(!pc.art_dropped(7), "so the pages do not ask for it while the PC is away");
    assert!(pc.art_wanted(7, "v1") && pc.ambient_glow.contains_key(&7), "and do when it is back");
    end_pass(&ctx);
    let _ = std::fs::remove_dir_all(dir);
}

/// A larger text size, or the window moved to a denser screen: the covers made smaller for
/// the old one are read again from their files and swapped in where they are, with no tile
/// in between.
#[test]
fn a_cover_made_smaller_is_loaded_sharper_when_the_window_shows_more_pixels() {
    let ctx = egui::Context::default();
    let (store, dir) = art_store("sharper");
    let (waker, woken) = telling_waker();
    let mut hosts = Hosts::without_discovery(vec![paired_record("uid")], waker);
    let id = hosts.list[0].id;
    let bytes = png_of([900, 1350], [20, 200, 20]);
    let file = cover_file(&store, 7, &bytes);
    Arc::make_mut(&mut hosts.list[0].art_versions).insert(7, "v1".into());
    hosts.covers_shown(&ctx, &store);
    assert_eq!(hosts.art_scale, 1.0);
    let (image, ambient, prepared, sized) = art_and_ambient(Some(bytes), hosts.art_scale);
    hosts.sender.send(Event::Art { id, app: 7, version: "v1".into(), image, ambient, prepared, sized }).expect("the channel");
    hosts.drain(&ctx);
    hosts.covers_shown(&ctx, &store);
    assert_eq!(cover_pixels(&hosts.list[0], 7), Some([630, 945]));
    assert_eq!(hosts.list[0].art_sized.get(&7), Some(&1.0));

    // 125 %: still smaller than its file, and sharper than it was.
    ctx.set_pixels_per_point(1.25);
    end_pass(&ctx);
    assert_eq!(ctx.pixels_per_point(), 1.25);
    hosts.covers_shown(&ctx, &store);
    assert_eq!(cover_pixels(&hosts.list[0], 7), Some([630, 945]), "it shows as it is until the sharper one lands");
    hosts.covers_shown(&ctx, &store);
    woken.recv_timeout(Duration::from_secs(20)).expect("the cover is read again from its file");
    hosts.drain(&ctx);
    assert_eq!(cover_pixels(&hosts.list[0], 7), Some([788, 1182]));
    assert_eq!(hosts.list[0].art_sized.get(&7), Some(&1.25));
    assert!(woken.try_recv().is_err(), "asked for once, though two frames passed before it landed");

    // 200 %: all of the file is kept, and there is nothing left to load after it.
    ctx.set_pixels_per_point(2.0);
    end_pass(&ctx);
    hosts.covers_shown(&ctx, &store);
    woken.recv_timeout(Duration::from_secs(20)).expect("read again");
    hosts.drain(&ctx);
    assert_eq!(cover_pixels(&hosts.list[0], 7), Some([900, 1350]));
    assert!(!hosts.list[0].art_sized.contains_key(&7));

    // Back to 100 % it stays: sharper than needed, and the budget is what bounds it.
    ctx.set_pixels_per_point(1.0);
    end_pass(&ctx);
    hosts.covers_shown(&ctx, &store);
    assert_eq!(cover_pixels(&hosts.list[0], 7), Some([900, 1350]));
    let _ = std::fs::remove_file(file);
    end_pass(&ctx);
    let _ = std::fs::remove_dir_all(dir);
}

/// The sharper cover was read for the cover that was there when it was asked for.
#[test]
fn a_sharper_cover_lands_only_on_the_cover_it_was_read_for() {
    let ctx = egui::Context::default();
    let mut hosts = Hosts::without_discovery(vec![paired_record("uid")], Arc::new(|| {}));
    let id = hosts.list[0].id;
    Arc::make_mut(&mut hosts.list[0].art_versions).insert(7, "v1".into());
    let sharper = |version: &str| {
        let image = decode_image(&png_of([60, 80], [20, 20, 200])).expect("the cover");
        Event::ArtSharper { id, app: 7, version: version.into(), image, sized: None }
    };
    let (image, ambient, prepared, _) = art_and_ambient(Some(png([20, 200, 20])), 1.0);
    hosts.sender.send(Event::Art { id, app: 7, version: "v1".into(), image, ambient, prepared, sized: Some(1.0) }).expect("the channel");
    hosts.sender.send(sharper("v0")).expect("the channel");
    hosts.drain(&ctx);
    assert_eq!(cover_pixels(&hosts.list[0], 7), Some([30, 40]), "the PC has another cover by now");
    assert_eq!(hosts.list[0].art_sized.get(&7), Some(&1.0));

    hosts.sender.send(sharper("v1")).expect("the channel");
    hosts.drain(&ctx);
    assert_eq!(cover_pixels(&hosts.list[0], 7), Some([60, 80]));
    assert!(!hosts.list[0].art_sized.contains_key(&7), "all of its file now");
    assert!(hosts.list[0].ambient.contains_key(&7), "the ambient is the cover's own and stays");

    // Given up for room in between, it is not brought back by a read that is late.
    Arc::make_mut(&mut hosts.list[0].art).insert(7, Art::Dropped);
    hosts.sender.send(sharper("v1")).expect("the channel");
    hosts.drain(&ctx);
    assert!(hosts.list[0].art_dropped(7));
    Arc::make_mut(&mut hosts.list[0].art).insert(7, Art::Missing);
    hosts.sender.send(sharper("v1")).expect("the channel");
    hosts.drain(&ctx);
    assert!(matches!(hosts.list[0].art.get(&7), Some(Art::Missing)));
    end_pass(&ctx);
}

/// A read asked for at an older scale lands after a newer one, or after the cover was loaded
/// again: the cover keeps the sharper picture it has.
#[test]
fn a_late_sharper_cover_for_an_older_scale_does_not_make_the_cover_blurrier() {
    let ctx = egui::Context::default();
    let mut hosts = Hosts::without_discovery(vec![paired_record("uid")], Arc::new(|| {}));
    let id = hosts.list[0].id;
    Arc::make_mut(&mut hosts.list[0].art_versions).insert(7, "v1".into());
    let sharper = |size: [u32; 2], sized: Option<f32>| {
        let image = decode_image(&png_of(size, [20, 20, 200])).expect("the cover");
        Event::ArtSharper { id, app: 7, version: "v1".into(), image, sized }
    };
    let (image, ambient, prepared, _) = art_and_ambient(Some(png([20, 200, 20])), 1.0);
    hosts.sender.send(Event::Art { id, app: 7, version: "v1".into(), image, ambient, prepared, sized: Some(1.0) }).expect("the channel");
    hosts.drain(&ctx);
    // 125 % then 150 % asked for before either landed: the 150 % read is the one `art_sized` names.
    Arc::make_mut(&mut hosts.list[0].art_sized).insert(7, 1.5);
    hosts.sender.send(sharper([90, 120], Some(1.5))).expect("the channel");
    hosts.sender.send(sharper([75, 100], Some(1.25))).expect("the channel");
    hosts.drain(&ctx);
    assert_eq!(cover_pixels(&hosts.list[0], 7), Some([90, 120]), "the late 125 % read is dropped");
    assert_eq!(hosts.list[0].art_sized.get(&7), Some(&1.5));

    // Loaded again whole (given up for room and fetched): no smaller read replaces it.
    let (image, ambient, prepared, _) = art_and_ambient(Some(png_of([60, 80], [20, 200, 20])), 1.0);
    hosts.sender.send(Event::Art { id, app: 7, version: "v1".into(), image, ambient, prepared, sized: None }).expect("the channel");
    hosts.sender.send(sharper([45, 60], Some(1.5))).expect("the channel");
    hosts.drain(&ctx);
    assert_eq!(cover_pixels(&hosts.list[0], 7), Some([60, 80]), "all of its file stays");
    assert!(!hosts.list[0].art_sized.contains_key(&7));
    end_pass(&ctx);
}

/// The cover's file is gone when the window shows more pixels: the cover that shows stays, and
/// it is not looked for again each frame.
#[test]
fn a_cover_whose_file_is_gone_keeps_showing_when_the_window_shows_more_pixels() {
    let ctx = egui::Context::default();
    let (store, dir) = art_store("sharper-gone");
    let (waker, woken) = telling_waker();
    let mut hosts = Hosts::without_discovery(vec![paired_record("uid")], waker);
    let id = hosts.list[0].id;
    let bytes = png_of([900, 1350], [20, 200, 20]);
    let file = cover_file(&store, 7, &bytes);
    Arc::make_mut(&mut hosts.list[0].art_versions).insert(7, "v1".into());
    hosts.covers_shown(&ctx, &store);
    let (image, ambient, prepared, sized) = art_and_ambient(Some(bytes), hosts.art_scale);
    hosts.sender.send(Event::Art { id, app: 7, version: "v1".into(), image, ambient, prepared, sized }).expect("the channel");
    hosts.drain(&ctx);
    hosts.covers_shown(&ctx, &store);
    assert_eq!(cover_pixels(&hosts.list[0], 7), Some([630, 945]));
    std::fs::remove_file(&file).expect("the file");

    ctx.set_pixels_per_point(1.25);
    end_pass(&ctx);
    hosts.covers_shown(&ctx, &store);
    hosts.covers_shown(&ctx, &store);
    assert!(woken.recv_timeout(Duration::from_millis(300)).is_err(), "nothing to land");
    hosts.drain(&ctx);
    assert_eq!(cover_pixels(&hosts.list[0], 7), Some([630, 945]), "the cover that shows stays");
    assert_eq!(hosts.list[0].art_sized.get(&7), Some(&1.25), "asked for once at this scale");
    end_pass(&ctx);
    let _ = std::fs::remove_dir_all(dir);
}

/// The Apps page asked for every cover without a version each pass, and the pages that knew
/// one asked with it: each dropped the other's cover and fetched it again.
#[test]
fn a_cover_asked_for_without_a_version_keeps_the_known_one() {
    let mut hosts = Hosts::without_discovery(vec![paired_record("uid")], Arc::new(|| {}));
    let pc = &mut hosts.list[0];
    assert!(pc.art_wanted(7, "") && pc.art_wanted(7, "v1"), "not asked for yet");
    assert_eq!(pc.begin_art(7, "v1").as_deref(), Some("v1"));
    Arc::make_mut(&mut pc.ambient_glow).insert(7, egui::Color32::RED);

    assert!(!pc.art_wanted(7, "") && !pc.art_wanted(7, "v1"));
    assert_eq!(pc.begin_art(7, ""), None, "no version says nothing of the cover");
    assert_eq!(pc.begin_art(7, "v1"), None);
    assert_eq!(pc.art_versions[&7], "v1");
    assert!(matches!(pc.art.get(&7), Some(Art::Loading)) && pc.ambient_glow.contains_key(&7), "kept");

    // Dropped with its renderer, it loads as the cover it was: from the file of that version.
    Arc::make_mut(&mut pc.art).remove(&7);
    assert!(pc.art_wanted(7, ""));
    assert_eq!(pc.begin_art(7, "").as_deref(), Some("v1"));

    assert!(pc.art_wanted(7, "v2"), "a new version is another cover");
    assert_eq!(pc.begin_art(7, "v2").as_deref(), Some("v2"));
    assert_eq!(pc.art_versions[&7], "v2");
    assert!(!pc.ambient_glow.contains_key(&7), "the old cover's glow goes with it");

    // A PC that gives its covers no version: asked for once.
    assert_eq!(pc.begin_art(8, "").as_deref(), Some(""));
    assert!(!pc.art_wanted(8, ""));
    assert_eq!(pc.begin_art(8, ""), None);
}

#[test]
fn every_page_asks_for_a_cover_at_one_version() {
    let app = |id: i32, art_version: &str| App {
        name: "Game".into(),
        id,
        hdr_supported: false,
        uuid: String::new(),
        host_index: None,
        art_version: art_version.into(),
        control: AppControl::None,
        monitor_resume: false,
    };
    let mut hosts = Hosts::without_discovery(vec![paired_record("uid")], Arc::new(|| {}));
    let pc = &mut hosts.list[0];
    assert_eq!(pc.cover_version(7), "", "nothing read yet");
    pc.apps = Apps::Loaded(Arc::new(vec![app(7, "7"), app(8, "8"), app(9, "")]));
    assert_eq!((pc.cover_version(7), pc.cover_version(8), pc.cover_version(9)), ("7", "8", ""));
    assert_eq!(pc.cover_version(10), "", "not an app of this PC");

    let mut library = library();
    let listed = library.games[0].app_id as i32;
    library.games[0].artwork.version = "a41c09e2".into();
    pc.apps = Apps::Loaded(Arc::new(vec![app(listed, "7"), app(listed + 1000, "8")]));
    pc.native.library = Some(Arc::new(library));
    assert_eq!(pc.cover_version(listed), "a41c09e2", "the library's, whatever the app list says");
    assert_eq!(pc.cover_version(listed + 1000), "8", "the app list's for an app the library leaves out");
    pc.apps = Apps::Loading;
    assert_eq!(pc.cover_version(listed), "a41c09e2");
}

/// A long app list asked for all its covers at once, a thread and a TLS handshake each.
#[test]
fn covers_load_a_few_at_a_time_and_all_of_them() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Condvar;

    let wait_for = |what: &str, met: &dyn Fn() -> bool| {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !met() {
            assert!(Instant::now() < deadline, "{what}");
            std::thread::sleep(Duration::from_millis(2));
        }
    };
    let pool = ArtPool::default();
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let (started, done) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let asked = ART_WORKERS * 3;
    for cover in 0..asked {
        let (gate, started, done) = (gate.clone(), started.clone(), done.clone());
        pool.run(move || {
            started.fetch_add(1, Ordering::SeqCst);
            let (open, opened) = &*gate;
            drop(opened.wait_while(open.lock().unwrap(), |open| !*open).unwrap());
            done.fetch_add(1, Ordering::SeqCst);
            assert!(cover != 1, "a cover that panics takes no other with it");
        });
    }
    wait_for("the first covers start", &|| started.load(Ordering::SeqCst) == ART_WORKERS);
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(started.load(Ordering::SeqCst), ART_WORKERS, "the rest wait their turn");
    let (workers, waiting) = {
        let queue = pool.queue();
        (queue.workers, queue.waiting.len())
    };
    assert_eq!((workers, waiting), (ART_WORKERS, asked - ART_WORKERS));

    *gate.0.lock().unwrap() = true;
    gate.1.notify_all();
    wait_for("every cover loads", &|| done.load(Ordering::SeqCst) == asked);
    wait_for("the workers end with the last cover", &|| pool.queue().workers == 0);

    // An idle pool starts again for the next cover.
    let (again, landed) = (done.clone(), done.clone());
    pool.run(move || {
        again.fetch_add(1, Ordering::SeqCst);
    });
    wait_for("a cover asked for later loads", &|| landed.load(Ordering::SeqCst) == asked + 1);
}

#[test]
fn corrupt_cached_cover_is_removed_instead_of_staying_stuck() {
    let ctx = egui::Context::default();
    let (store, dir) = art_store("corrupt");
    let (waker, woken) = telling_waker();
    let mut record = paired_record("uid");
    record.address = "127.0.0.1".into();
    let mut hosts = Hosts::without_discovery(vec![record], waker);
    hosts.set_identity(Arc::new(ClientIdentity::generate().expect("a key")));
    hosts.list[0].apps = Apps::Loaded(Arc::new(vec![App {
        name: "Game".into(),
        id: 7,
        hdr_supported: false,
        uuid: String::new(),
        host_index: None,
        art_version: "v1".into(),
        control: AppControl::None,
        monitor_resume: false,
    }]));
    let id = hosts.list[0].id;
    hosts.list[0].reach = Reach::Offline;
    let file = cover_file(&store, 7, b"<html>not a cover png</html>");
    assert!(file.exists());
    hosts.request_art(id, 7, "v1", &store);
    woken.recv_timeout(Duration::from_secs(10)).expect("cover job runs");
    hosts.drain(&ctx);
    assert!(!file.exists(), "corrupt non-placeholder cache must be removed");
    end_pass(&ctx);
    let _ = std::fs::remove_dir_all(dir);
}
