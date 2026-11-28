use super::*;

/// Answers from fixtures by the request's URL and body, counting what it was asked; never a network.
struct Fixtures {
    asked: Mutex<Vec<String>>,
    answer: Box<dyn Fn(&Request) -> Answer + Send + Sync>,
}

impl Http for Arc<Fixtures> {
    fn send(&self, request: &Request) -> Answer {
        self.asked.lock().unwrap().push(format!("{} {}", request.url, request.body.clone().unwrap_or_default()));
        (self.answer)(request)
    }
}

fn ok_bytes(body: &[u8]) -> Answer {
    Answer { status: 200, retry_after: None, body: body.to_vec() }
}

fn ok(body: &str) -> Answer {
    ok_bytes(body.as_bytes())
}

fn tiny_png() -> Vec<u8> {
    let image = image::RgbImage::from_pixel(2, 2, image::Rgb([10, 120, 220]));
    let mut bytes = std::io::Cursor::new(Vec::new());
    image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
    bytes.into_inner()
}

fn services(request: &Request) -> Answer {
    let what = format!("{} {}", request.url, request.body.clone().unwrap_or_default());
    match request.service {
        Service::SteamGridDb if what.contains("/games/steam/1145360") => ok(r#"{"success":true,"data":{"id":42}}"#),
        Service::SteamGridDb if what.contains("/grids/game/42") => ok(r#"{"success":true,"data":[{"score":1,"url":"https://cdn2.steamgriddb.com/grid/hades.png"}]}"#),
        Service::Image if request.url == "https://cdn2.steamgriddb.com/grid/hades.png" => ok_bytes(&tiny_png()),
        Service::Twitch => ok(r#"{"access_token":"tok-1","expires_in":5000000}"#),
        Service::Igdb if what.contains("external_games") => ok(r#"[{"game":7,"external_game_source":1}]"#),
        Service::Igdb if what.contains("game_time_to_beats") => ok(r#"[{"game_id":7,"hastily":72000,"normally":86400,"completely":324000}]"#),
        Service::Igdb if what.contains("/games") => ok(r#"[{"id":7,"name":"Hades","genres":[{"name":"Roguelike"}],"themes":[{"name":"Action"}]}]"#),
        _ => Answer { status: 404, retry_after: None, body: Vec::new() },
    }
}

fn keys() -> Keys {
    Keys { steamgriddb: "sgdb-key-1234".into(), igdb_client_id: "client-5678".into(), igdb_client_secret: "secret-9012".into() }
}

fn fixture(answer: impl Fn(&Request) -> Answer + Send + Sync + 'static) -> Arc<Fixtures> {
    Arc::new(Fixtures { asked: Mutex::new(Vec::new()), answer: Box::new(answer) })
}

fn dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("nightjar-game-info-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

const HADES: fn() -> Game = || game_of("Hades™", Some("steam:1145360"), None);

#[test]
fn without_keys_nothing_is_asked_and_nothing_is_kept() {
    let http = fixture(|_| panic!("asked without keys"));
    let info = GameInfo::new(&dir("none"), Keys::default(), Box::new(http.clone()));
    assert_eq!(info.record(&HADES(), true), None);
    assert_eq!(info.cover(&HADES()), None);
    assert!(http.asked.lock().unwrap().is_empty());
}

#[test]
fn a_cover_and_facts_are_looked_up_once_and_then_read_from_disk() {
    let root = dir("cover");
    let expected = tiny_png();
    let expected_rgba = image::load_from_memory(&expected).unwrap().to_rgba8();
    let http = fixture(services);
    let info = GameInfo::new(&root, keys(), Box::new(http.clone()));
    let fetched = info.cover(&HADES()).expect("decoded cover");
    assert_eq!(fetched.to_rgba8(), expected_rgba);
    let record = info.kept("steam:1145360").unwrap();
    assert_eq!(record.facts.as_ref().unwrap().genres, ["Roguelike"]);
    let asked = http.asked.lock().unwrap().len();
    assert_eq!(asked, 7, "game, cover, token, external, game, time to beat, image: {:?}", http.asked.lock().unwrap());

    // A new run reads the record and the image from disk: no request at all.
    let again = fixture(|_| panic!("asked again"));
    let info = GameInfo::new(&root, keys(), Box::new(again.clone()));
    let cached = info.cover(&HADES()).expect("cached decoded cover");
    assert_eq!(cached.to_rgba8(), expected_rgba);
    assert_eq!(info.kept_cover(&HADES()).as_deref(), Some(expected.as_slice()));
    assert_eq!(info.record(&HADES(), false).unwrap().facts.unwrap().normally, Some(86400));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_facts_only_lookup_keeps_the_cover_found_before() {
    let root = dir("merge");
    let info = GameInfo::new(&root, keys(), Box::new(fixture(services)));
    // Facts first (the library's pass), then the cover (a placeholder on the PC): the record
    // ends with both, and the facts are not asked twice.
    let facts = info.record(&HADES(), false).unwrap();
    assert_eq!(facts.cover, None);
    let both = info.record(&HADES(), true).unwrap();
    assert_eq!(both.cover.as_deref(), Some("https://cdn2.steamgriddb.com/grid/hades.png"));
    assert!(both.facts.is_some());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_failing_service_is_quiet_and_its_log_holds_no_key() {
    let root = dir("fail");
    let info = GameInfo::new(&root, keys(), Box::new(fixture(|_| Answer { status: 400, retry_after: None, body: b"sgdb-key-1234".to_vec() })));
    // Refused everywhere: no cover, no facts, no panic, and a record that is tried again later.
    assert_eq!(info.cover(&Game { name: "Hades".into(), steam: None }), None);
    let record = info.kept("name:hades").unwrap();
    assert_eq!(record.art, nightjar_policy::game_info::cache::Outcome::Failed);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn keys_are_kept_apart_from_the_settings_and_removed_when_cleared() {
    let root = dir("keys");
    save_keys(&root, &keys()).unwrap();
    assert_eq!(load_keys(&root), keys());
    assert!(root.join(KEYS_FILE).is_file());
    save_keys(&root, &Keys::default()).unwrap();
    assert!(!root.join(KEYS_FILE).exists());
    assert_eq!(load_keys(&root), Keys::default());
    save_keys(&root, &Keys::default()).unwrap();
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_hosts_game_names_its_steam_id_from_either_field() {
    assert_eq!(game_of("A", Some("steam:10"), None).steam, Some(10));
    assert_eq!(game_of("A", None, Some(("steam", "20"))).steam, Some(20));
    assert_eq!(game_of("A", Some("epic:x"), Some(("epic", "x"))).steam, None);
}

#[test]
fn undecodable_or_truncated_images_are_not_cached_and_corrupt_cache_is_replaced() {
    let root = dir("bad-img");
    let url = "https://cdn2.steamgriddb.com/grid/test.png";
    let bad = GameInfo::new(&root, keys(), Box::new(fixture(|_| ok("<html>not an image</html>"))));
    assert_eq!(bad.image(url), None);
    let img_path = root.join("game-info").join("img").join(cache::image_file(url));
    assert!(!img_path.exists());

    // Simulate a 24 MiB truncated body matching EXCHANGE_LIMIT.
    let cut = vec![0u8; crate::update_net::EXCHANGE_LIMIT as usize];
    let oversized = GameInfo::new(&root, keys(), Box::new(fixture(move |_| ok_bytes(&cut))));
    assert_eq!(oversized.image(url), None);
    assert!(!img_path.exists());

    // Corrupt cached bytes on disk are deleted and re-fetched from HTTP.
    std::fs::create_dir_all(img_path.parent().unwrap()).unwrap();
    std::fs::write(&img_path, b"corrupt-cache").unwrap();
    let expected = tiny_png();
    let expected_rgba = image::load_from_memory(&expected).unwrap().to_rgba8();
    let healed = GameInfo::new(&root, keys(), Box::new(fixture(|_| ok_bytes(&tiny_png()))));
    assert_eq!(healed.image(url).expect("decoded image").to_rgba8(), expected_rgba);
    assert_eq!(std::fs::read(&img_path).unwrap(), expected);
    let _ = std::fs::remove_dir_all(&root);
}

// Unix (Linux and macOS): saved keys use mode 0o600 from initial file creation.
#[cfg(unix)]
#[test]
fn saved_keys_start_with_user_only_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let root = dir("mode");
    save_keys(&root, &keys()).unwrap();
    let mode = std::fs::metadata(root.join(KEYS_FILE)).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    let _ = std::fs::remove_dir_all(&root);
}
