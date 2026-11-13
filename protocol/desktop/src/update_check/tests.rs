use super::*;
use crate::update_net::{Response, Url};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[test]
fn only_a_newer_stable_release_is_offered() {
    assert!(newer_stable("v4.0.1", "4.0.0"));
    assert!(newer_stable("v4.1.0", "4.0.9"));
    assert!(newer_stable("5.0.0", "4.9.9"));
    assert!(newer_stable("v4.0.0", "4.0.0-rc.3"), "the final release follows its candidates");
    assert!(!newer_stable("v4.0.0", "4.0.0"));
    assert!(!newer_stable("v3.9.9", "4.0.0"));
    assert!(!newer_stable("v4.1.0-rc.1", "4.0.0"), "a pre-release tag is never offered");
    assert!(!newer_stable("nightly", "4.0.0"));
    assert!(!newer_stable("v4.1.0", "a local build"));
    assert!(newer_stable("v4.1", "4.0.0"), "a two-part tag reads as .0");
    assert!(!newer_stable("v4.1.0.2", "4.0.0"));
    for odd in ["v9.9.9+a/b", "9.9.9+build", "v9.9.9/..", " v9.9.9"] {
        assert!(!newer_stable(odd, "4.0.0"), "{odd}");
        assert_eq!(offer(&answer(odd, false, false, ""), "4.0.0", &CopyKind::Installed), Ok(None), "{odd}");
    }
    assert!(newer_stable("v4.1.0", "4.0.0+local"), "only the tag is held to this");
}

/// The setup an installed copy on this PC takes (x86_64 or arm64, by the PC's Windows).
fn setup() -> String {
    CopyKind::Installed.asset_name("4.1.0").unwrap()
}
const DIGEST: &str = "sha256:AB12000000000000000000000000000000000000000000000000000000000000";

fn answer(tag: &str, draft: bool, pre: bool, assets: &str) -> String {
    format!(r#"{{"tag_name":"{tag}","draft":{draft},"prerelease":{pre},"html_url":"https://github.com/RamazanKara/nightjar/releases/tag/{tag}","assets":[{assets}]}}"#)
}

fn setup_asset(url: &str, digest: &str) -> String {
    format!(r#"{{"name":"{}","size":1000,"browser_download_url":"{url}"{digest}}}"#, setup())
}

fn github_url() -> String {
    format!("https://github.com/RamazanKara/nightjar/releases/download/v4.1.0/{}", setup())
}

#[test]
fn the_answer_is_read_like_v_plus_reads_it() {
    let release = |tag: &str, draft: bool, pre: bool| answer(tag, draft, pre, "");
    assert_eq!(offer(&release("v4.1.0", false, false), "4.0.0", &CopyKind::Portable), Ok(Some(Release {
        version: "4.1.0".into(),
        url: "https://github.com/RamazanKara/nightjar/releases/tag/v4.1.0".into(),
        asset: Err(Failure::NoInstaller),
    })));
    assert_eq!(offer(&release("v4.1.0", true, false), "4.0.0", &CopyKind::Portable), Ok(None), "a draft");
    assert_eq!(offer(&release("v4.1.0", false, true), "4.0.0", &CopyKind::Portable), Ok(None), "a pre-release");
    assert_eq!(offer(&release("v4.0.0", false, false), "4.0.0", &CopyKind::Portable), Ok(None), "this version");
    assert!(offer("{\"message\":\"Not Found\"}", "4.0.0", &CopyKind::Portable).is_err());
    assert!(offer("<html>", "4.0.0", &CopyKind::Portable).is_err());
    assert!(offer(r#"{"tag_name":"v9.0.0","html_url":"https://evil.example/"}"#, "4.0.0", &CopyKind::Portable).is_err(), "only GitHub pages open");
}

#[test]
fn the_release_answer_says_what_to_download_and_check() {
    let read = |assets: &str| offer(&answer("v4.1.0", false, false, assets), "4.0.0", &CopyKind::Installed).unwrap().unwrap().asset;
    let digest = format!(r#","digest":"{DIGEST}""#);
    assert_eq!(read(&setup_asset(&github_url(), &digest)), Ok(Asset {
        name: setup(),
        url: github_url(),
        digest: DIGEST[7..].to_ascii_lowercase(),
        size: 1000,
    }));
    assert_eq!(read(""), Err(Failure::NoInstaller));
}

#[test]
fn a_missing_digest_refuses() {
    let read = |digest: &str| offer(&answer("v4.1.0", false, false, &setup_asset(&github_url(), digest)), "4.0.0", &CopyKind::Installed).unwrap().unwrap().asset;
    assert_eq!(read(""), Err(Failure::NoDigest), "no field");
    assert_eq!(read(r#","digest":null"#), Err(Failure::NoDigest), "null");
    assert_eq!(read(r#","digest":"md5:abcd""#), Err(Failure::NoDigest), "another hash");
    assert_eq!(read(r#","digest":"sha256:abcd""#), Err(Failure::NoDigest), "too short");
}

#[test]
fn an_asset_on_another_host_refuses() {
    let digest = format!(r#","digest":"{DIGEST}""#);
    let url = format!("https://evil.example/RamazanKara/nightjar/releases/download/v4.1.0/{}", setup());
    let release = offer(&answer("v4.1.0", false, false, &setup_asset(&url, &digest)), "4.0.0", &CopyKind::Installed).unwrap().unwrap();
    assert_eq!(release.asset, Err(Failure::Address(url)));
}

/// The real request: `cargo test -p nightjar-desktop asks_github -- --ignored`.
#[test]
#[ignore = "asks GitHub over the network"]
fn asks_github() {
    let net = Native::new("Nightjar/test".into());
    let json = check(&net, &CopyKind::Installed, "0.0.1", || true).unwrap().unwrap();
    assert!(json.url.starts_with("https://github.com/RamazanKara/nightjar/"), "{json:?}");
    assert!(json.asset.as_ref().is_ok_and(|a| a.size > 0 && a.digest.len() == 64), "{json:?}");
    assert_eq!(check(&net, &CopyKind::Installed, "999.0.0", || true).unwrap(), None);
}

/// The real download, through GitHub's real redirect, without running anything:
/// `cargo test -p nightjar-desktop downloads_the_real -- --ignored`.
#[test]
#[ignore = "downloads from GitHub over the network"]
fn downloads_the_real_setup() {
    let net = Native::new("Nightjar/test".into());
    let asset = check(&net, &CopyKind::Installed, "0.0.1", || true).unwrap().unwrap().asset.unwrap();
    let part = folder("real").join("setup.part");
    let progress = Progress { cancel: AtomicBool::new(false), received: AtomicU64::new(0), wake: Arc::new(|| {}) };
    self_update::download(&net, self_update::download_host, &asset, &part, &progress).unwrap();
    assert_eq!(std::fs::metadata(&part).unwrap().len(), asset.size);
    std::fs::remove_file(&part).unwrap();
}

#[test]
fn off_is_off_and_no_request_is_made() {
    let updates = Updates::start(false, Arc::new(|| {}));
    assert!(updates.pending.is_none() && updates.found.is_none());
}

#[test]
fn with_the_check_off_leftovers_still_go_and_nothing_is_asked() {
    let installer = Installer::new("off-leftovers");
    let part = installer.dir.join(".nightjar-update-1-2.part");
    std::fs::write(&part, b"x").unwrap();
    std::fs::File::options().write(true).open(&part).unwrap().set_modified(std::time::SystemTime::now() - Duration::from_secs(7200)).unwrap();
    let count = Arc::new(Counting::default());
    let net: Arc<dyn Transport> = count.clone();
    let updates = Updates::begin(false, CopyKind::Installed, net.clone(), Some(installer.steps(net, local)), "4.0.0", || true, Arc::new(|| {}));
    wait_until("the leftover stays", || !part.exists());
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(count.gets.load(Ordering::Relaxed), 0, "no request");
    assert!(updates.pending.is_none() && updates.found.is_none() && !updates.checking());
}

#[test]
fn check_now_asks_once_when_chosen_with_the_setting_off() {
    let installer = Installer::new("check-now");
    let count = Arc::new(Counting::default());
    let digest = format!(r#","digest":"{DIGEST}""#);
    *count.answer.lock().unwrap() = answer("v4.1.0", false, false, &setup_asset(&github_url(), &digest));
    let net: Arc<dyn Transport> = count.clone();
    let mut updates = Updates::begin(false, CopyKind::Installed, net.clone(), Some(installer.steps(net, local)), "4.0.0", || true, Arc::new(|| {}));
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(count.gets.load(Ordering::Relaxed), 0, "nothing until it is chosen");
    assert!(updates.check_now());
    assert!(!updates.check_now(), "one check at a time");
    wait_until("no answer", || { updates.step(); !updates.checking() });
    assert_eq!(updates.answer(), Some(Answer::Found));
    assert_eq!(updates.answer(), None, "the answer comes once");
    assert!(updates.found.as_ref().unwrap().asset.is_ok(), "the notice offers Update now");
    assert_eq!(count.gets.load(Ordering::Relaxed), 1);
    assert!(installer.ran().is_empty(), "nothing installs without the click");

    // Dismissed, the notice comes back with the next check; this version is up to date.
    updates.found = None;
    assert!(updates.check_now());
    wait_until("no second answer", || { updates.step(); !updates.checking() });
    assert_eq!(updates.answer(), Some(Answer::Found));
    assert!(updates.found.is_some());
    *count.answer.lock().unwrap() = answer("v4.0.0", false, false, "");
    assert!(updates.check_now());
    wait_until("no third answer", || { updates.step(); !updates.checking() });
    assert_eq!(updates.answer(), Some(Answer::UpToDate));
    assert_eq!(Answer::UpToDate.toast(), Some("You have the latest version"));
    *count.answer.lock().unwrap() = "<html>".into();
    assert!(updates.check_now());
    wait_until("no fourth answer", || { updates.step(); !updates.checking() });
    assert_eq!(updates.answer(), Some(Answer::Failed));
    assert_eq!(Answer::Failed.toast(), Some("Couldn’t check for updates"));
    assert_eq!(Answer::Found.toast(), None, "the notice says it");
}

#[test]
fn a_managed_copy_has_no_check_now() {
    let mut updates = Updates::default();
    assert!(!updates.check_now());
    assert!(!updates.checking() && updates.answer().is_none());
}

// A local server stands in for GitHub; the real WinHTTP or ureq talks to it.

#[derive(Clone)]
enum Reply {
    Body(Vec<u8>),
    /// Says one length and sends fewer bytes.
    Short(u64, Vec<u8>),
    /// No length: the end of the connection ends the body.
    Unsized(Vec<u8>),
    Redirect(String),
    /// Sends the first part, then waits for the release before sending the rest.
    Hold(Vec<u8>, Vec<u8>, Arc<Mutex<Option<Receiver<()>>>>),
}

struct Fixture {
    port: u16,
    hits: Arc<Mutex<Vec<String>>>,
}

impl Fixture {
    fn start(route: impl Fn(&str, u16) -> Reply + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let hits = Arc::new(Mutex::new(Vec::new()));
        let (seen, route) = (hits.clone(), Arc::new(route));
        std::thread::spawn(move || {
            for connection in listener.incoming().flatten() {
                let (seen, route) = (seen.clone(), route.clone());
                std::thread::spawn(move || serve(connection, &seen, &*route, port));
            }
        });
        Self { port, hits }
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }

    fn hits(&self) -> Vec<String> {
        self.hits.lock().unwrap().clone()
    }
}

fn serve(mut connection: std::net::TcpStream, seen: &Mutex<Vec<String>>, route: &(dyn Fn(&str, u16) -> Reply + Send + Sync), port: u16) {
    let mut request = Vec::new();
    let mut byte = [0u8];
    while !request.ends_with(b"\r\n\r\n") {
        if connection.read(&mut byte).unwrap_or(0) == 0 { return; }
        request.push(byte[0]);
    }
    let text = String::from_utf8_lossy(&request);
    let path = text.split_whitespace().nth(1).unwrap_or("").to_owned();
    seen.lock().unwrap().push(path.clone());
    let _ = match route(&path, port) {
        Reply::Body(body) => write!(connection, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).and_then(|()| connection.write_all(&body)),
        Reply::Short(declared, body) => write!(connection, "HTTP/1.1 200 OK\r\nContent-Length: {declared}\r\nConnection: close\r\n\r\n").and_then(|()| connection.write_all(&body)),
        Reply::Unsized(body) => write!(connection, "HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n").and_then(|()| connection.write_all(&body)),
        Reply::Redirect(to) => write!(connection, "HTTP/1.1 302 Found\r\nLocation: {to}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"),
        Reply::Hold(first, rest, release) => {
            let length = first.len() + rest.len();
            let receiver = release.lock().unwrap().take();
            write!(connection, "HTTP/1.1 200 OK\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n")
                .and_then(|()| connection.write_all(&first))
                .and_then(|()| connection.flush())
                .map(|()| { if let Some(r) = receiver { let _ = r.recv_timeout(Duration::from_secs(20)); } })
                .and_then(|()| connection.write_all(&rest))
        }
    };
}

fn local(url: &Url) -> bool {
    !url.secure && url.host == "127.0.0.1"
}

fn folder(name: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!("nj-update-check-{}-{}-{name}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    // Windows reuses process ids: an earlier run's folder of the same name may still be there.
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// The fake installer: keeps what it was handed instead of running anything.
struct Installer {
    dir: PathBuf,
    ran: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl Installer {
    fn new(name: &str) -> Self {
        Self { dir: folder(name), ran: Arc::default() }
    }

    fn steps(&self, net: Arc<dyn Transport>, allow: fn(&Url) -> bool) -> Steps {
        let ran = self.ran.clone();
        Steps {
            net,
            allow,
            dir: self.dir.clone(),
            own_dir: true,
            marker: Some(self.marker()),
            stream_elsewhere: || false,
            setup_running: || false,
            apply: Box::new(move |part, _| {
                ran.lock().unwrap().push(std::fs::read(part).unwrap());
                std::fs::remove_file(part).unwrap();
                Ok(())
            }),
        }
    }

    fn marker(&self) -> PathBuf {
        self.dir.join("update-pending.json")
    }

    fn ran(&self) -> Vec<Vec<u8>> {
        self.ran.lock().unwrap().clone()
    }

    /// No unfinished file stays behind.
    fn parts(&self) -> usize {
        std::fs::read_dir(&self.dir).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().ends_with(".part")).count()
    }
}

fn native() -> Arc<dyn Transport> {
    Arc::new(Native::new("Nightjar/test".into()))
}

fn notice(asset: Result<Asset, Failure>, steps: Option<Steps>) -> Updates {
    Updates {
        found: Some(Release { version: "4.1.0".into(), url: "https://github.com/RamazanKara/nightjar/releases/tag/v4.1.0".into(), asset }),
        steps,
        ..Updates::default()
    }
}

fn asset_at(url: String, body: &[u8]) -> Asset {
    Asset { name: setup(), url, digest: sha256(body), size: body.len() as u64 }
}

/// Steps until the download is over. Whether Nightjar should exit.
fn finish(updates: &mut Updates) -> bool {
    let deadline = Instant::now() + Duration::from_secs(30);
    while updates.job.is_some() {
        assert!(Instant::now() < deadline, "the download never ended");
        if let Some(part) = updates.step() { return updates.hand_over(&part); }
        std::thread::sleep(Duration::from_millis(2));
    }
    false
}

fn body() -> Vec<u8> {
    (0..300_000u32).map(|i| (i % 251) as u8).collect()
}

#[test]
fn update_now_downloads_checks_hands_over_and_exits() {
    let bytes = body();
    let served = bytes.clone();
    let fixture = Fixture::start(move |path, port| match path {
        "/setup" => Reply::Redirect(format!("http://127.0.0.1:{port}/storage?sig=1")),
        _ => Reply::Body(served.clone()),
    });
    let installer = Installer::new("good");
    let mut updates = notice(Ok(asset_at(fixture.url("/setup"), &bytes)), Some(installer.steps(native(), local)));
    updates.update_now();
    assert!(finish(&mut updates), "Nightjar exits for the installer");
    assert_eq!(installer.ran(), [bytes]);
    assert_eq!(fixture.hits(), ["/setup", "/storage?sig=1"]);
    assert_eq!(installer.parts(), 0);
    let attempt = self_update::take_attempt(&installer.marker()).expect("the next launch can tell");
    assert_eq!(attempt.version, "4.1.0");
}

fn refused(reply: Reply, asset: impl FnOnce(&Fixture) -> Asset) -> Failure {
    let fixture = Fixture::start(move |_, _| reply.clone());
    let installer = Installer::new("refused");
    let mut updates = notice(Ok(asset(&fixture)), Some(installer.steps(native(), local)));
    updates.update_now();
    assert!(!finish(&mut updates), "Nightjar stays open");
    assert!(installer.ran().is_empty(), "nothing ran");
    assert_eq!(installer.parts(), 0, "nothing stays behind");
    assert!(!installer.marker().exists());
    updates.failure.expect("the notice says why")
}

#[test]
fn a_tampered_download_never_runs() {
    let bytes = body();
    let mut tampered = bytes.clone();
    tampered[1000] ^= 1;
    assert_eq!(refused(Reply::Body(tampered), |f| asset_at(f.url("/setup"), &bytes)), Failure::Digest);
}

#[test]
fn a_truncated_download_never_runs() {
    let bytes = body();
    let half = bytes[..bytes.len() / 2].to_vec();
    let failure = refused(Reply::Short(bytes.len() as u64, half.clone()), |f| asset_at(f.url("/setup"), &bytes));
    assert!(matches!(failure, Failure::Size | Failure::Network(_)), "the connection ended early: {failure:?}");
    assert_eq!(refused(Reply::Unsized(half), |f| asset_at(f.url("/setup"), &bytes)), Failure::Size, "no length to go by");
}

#[test]
fn a_download_of_another_size_never_runs() {
    let bytes = body();
    let mut longer = bytes.clone();
    longer.push(0);
    assert_eq!(refused(Reply::Body(longer.clone()), |f| asset_at(f.url("/setup"), &bytes)), Failure::Size, "the length says so");
    assert_eq!(refused(Reply::Unsized(longer), |f| asset_at(f.url("/setup"), &bytes)), Failure::Size, "the bytes say so");
}

#[test]
fn a_redirect_to_another_host_is_refused_unvisited() {
    let bytes = body();
    for target in ["http://localhost:{port}/evil", "https://evil.example/evil", "https://127.0.0.1:{port}/evil"] {
        let fixture = Fixture::start(move |path, port| match path {
            "/setup" => Reply::Redirect(target.replace("{port}", &port.to_string())),
            _ => Reply::Body(Vec::new()),
        });
        let installer = Installer::new("redirect");
        let mut updates = notice(Ok(asset_at(fixture.url("/setup"), &bytes)), Some(installer.steps(native(), local)));
        updates.update_now();
        assert!(!finish(&mut updates));
        assert!(matches!(updates.failure, Some(Failure::Address(_))), "{target}: {:?}", updates.failure);
        assert_eq!(fixture.hits(), ["/setup"], "{target}");
        assert!(installer.ran().is_empty());
    }
}

#[test]
fn a_wrong_host_is_refused_before_connecting() {
    let bytes = body();
    let fixture = Fixture::start(|_, _| Reply::Body(Vec::new()));
    let installer = Installer::new("host");
    // With the real rule, the fixture's plain local address is itself a wrong host.
    let mut updates = notice(Ok(asset_at(fixture.url("/setup"), &bytes)), Some(installer.steps(native(), self_update::download_host)));
    updates.update_now();
    assert!(!finish(&mut updates));
    assert_eq!(updates.failure, Some(Failure::Address("127.0.0.1".into())));
    assert!(fixture.hits().is_empty());
    assert!(installer.ran().is_empty());
}

#[test]
fn a_missing_digest_refuses_on_click_and_the_link_stays() {
    let installer = Installer::new("no-digest");
    let count = Arc::new(Counting::default());
    let mut updates = notice(Err(Failure::NoDigest), Some(installer.steps(count.clone(), local)));
    updates.update_now();
    assert_eq!(updates.failure, Some(Failure::NoDigest));
    assert!(updates.job.is_none() && updates.step().is_none() && installer.ran().is_empty());
    assert_eq!(count.gets.load(Ordering::Relaxed), 0, "nothing is fetched");
    assert!(updates.found.is_some(), "the release link stays");
    assert!(Failure::NoDigest.is_final(), "and Update now goes");
    assert!(!failure_text(&Failure::NoDigest, "4.1.0").is_empty());
}

fn held_reply(bytes: &[u8]) -> (Reply, std::sync::mpsc::Sender<()>) {
    let (release, wait) = channel();
    let (first, rest) = bytes.split_at(bytes.len() / 3);
    (Reply::Hold(first.to_vec(), rest.to_vec(), Arc::new(Mutex::new(Some(wait)))), release)
}

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !done() {
        assert!(Instant::now() < deadline, "{what}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn a_server_that_goes_silent_fails_the_download() {
    let bytes = body();
    // Kept until the end: the reply never goes on.
    let (reply, _release) = held_reply(&bytes);
    let fixture = Fixture::start(move |_, _| reply.clone());
    let installer = Installer::new("silent");
    let net: Arc<dyn Transport> = Arc::new(Native { agent: "Nightjar/test".into(), read_timeout: Duration::from_millis(300) });
    let mut updates = notice(Ok(asset_at(fixture.url("/setup"), &bytes)), Some(installer.steps(net, local)));
    updates.update_now();
    assert!(!finish(&mut updates), "Nightjar stays open");
    assert!(matches!(updates.failure, Some(Failure::Network(_))), "{:?}", updates.failure);
    assert_eq!(installer.parts(), 0);
    assert!(installer.ran().is_empty());
}

#[test]
fn cancel_stops_the_download_and_runs_nothing() {
    let bytes = body();
    let (reply, release) = held_reply(&bytes);
    let fixture = Fixture::start(move |_, _| reply.clone());
    let installer = Installer::new("cancel");
    let mut updates = notice(Ok(asset_at(fixture.url("/setup"), &bytes)), Some(installer.steps(native(), local)));
    updates.update_now();
    let progress = updates.job.as_ref().unwrap().progress.clone();
    wait_until("no progress", || progress.received.load(Ordering::Relaxed) > 0);
    updates.cancel();
    assert!(updates.job.is_none() && updates.failure.is_none(), "cancelling isn't a failure");
    let _ = release.send(());
    wait_until("the file stays", || installer.parts() == 0);
    for _ in 0..50 {
        assert!(updates.step().is_none());
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(installer.ran().is_empty());
    assert!(!installer.marker().exists());
    updates.update_now();
    assert!(updates.job.is_some(), "and it can start over");
    updates.cancel();
}

#[test]
fn a_stream_starting_stops_the_download_and_nothing_runs() {
    let bytes = body();
    let (reply, release) = held_reply(&bytes);
    let fixture = Fixture::start(move |_, _| reply.clone());
    let installer = Installer::new("stream");
    let mut updates = notice(Ok(asset_at(fixture.url("/setup"), &bytes)), Some(installer.steps(native(), local)));
    updates.update_now();
    let progress = updates.job.as_ref().unwrap().progress.clone();
    wait_until("no progress", || progress.received.load(Ordering::Relaxed) > 0);
    updates.hold(true);
    assert!(updates.job.is_none(), "no download next to a stream");
    let _ = release.send(());
    wait_until("the file stays", || installer.parts() == 0);
    assert!(updates.step().is_none());
    assert!(installer.ran().is_empty());
}

#[test]
fn update_now_does_nothing_during_a_stream() {
    let bytes = body();
    let count = Arc::new(Counting::default());
    let installer = Installer::new("streaming");
    let mut updates = notice(Ok(asset_at("http://127.0.0.1:1/setup".into(), &bytes)), Some(installer.steps(count.clone(), local)));
    updates.hold(true);
    updates.update_now();
    assert!(updates.job.is_none());
    std::thread::sleep(Duration::from_millis(20));
    assert_eq!(count.gets.load(Ordering::Relaxed), 0);
    updates.hold(false);
    updates.update_now();
    assert!(updates.job.is_some(), "back on the launcher it works");
    updates.cancel();
}

#[test]
fn a_finished_download_is_never_handed_over_once_a_stream_started() {
    let bytes = body();
    let served = bytes.clone();
    let fixture = Fixture::start(move |_, _| Reply::Body(served.clone()));
    let installer = Installer::new("finished");
    let mut updates = notice(Ok(asset_at(fixture.url("/setup"), &bytes)), Some(installer.steps(native(), local)));
    updates.update_now();
    // The download may end before the stream's first pass of the main loop; step isn't called
    // until after that pass. The thread is done when only the job and this test hold this.
    let progress = updates.job.as_ref().unwrap().progress.clone();
    wait_until("the download never ended", || Arc::strong_count(&progress) == 2);
    updates.hold(true);
    assert!(updates.step().is_none());
    wait_until("the file stays", || installer.parts() == 0);
    assert!(installer.ran().is_empty());
}

#[test]
fn a_stream_in_another_process_stops_the_update() {
    static STREAMING: AtomicBool = AtomicBool::new(true);
    let bytes = body();
    let served = bytes.clone();
    let fixture = Fixture::start(move |_, _| Reply::Body(served.clone()));
    let installer = Installer::new("elsewhere");
    let mut steps = installer.steps(native(), local);
    steps.stream_elsewhere = || STREAMING.load(Ordering::SeqCst);
    let mut updates = notice(Ok(asset_at(fixture.url("/setup"), &bytes)), Some(steps));
    updates.update_now();
    assert_eq!(updates.failure, Some(Failure::StreamElsewhere), "it says why");
    assert!(updates.job.is_none() && fixture.hits().is_empty(), "nothing is fetched");
    assert!(!Failure::StreamElsewhere.is_final(), "Update now stays for after that stream");
    // It starts during the download: the file goes and nothing runs.
    STREAMING.store(false, Ordering::SeqCst);
    updates.update_now();
    let progress = updates.job.as_ref().unwrap().progress.clone();
    wait_until("the download never ended", || Arc::strong_count(&progress) == 2);
    STREAMING.store(true, Ordering::SeqCst);
    assert!(updates.step().is_none(), "Nightjar stays open");
    assert_eq!(updates.failure, Some(Failure::StreamElsewhere));
    assert!(installer.ran().is_empty() && !installer.marker().exists());
    assert_eq!(installer.parts(), 0);
}

#[test]
fn a_setup_already_running_stops_the_hand_over() {
    let bytes = body();
    let served = bytes.clone();
    let fixture = Fixture::start(move |_, _| Reply::Body(served.clone()));
    let installer = Installer::new("second-setup");
    let mut steps = installer.steps(native(), local);
    steps.setup_running = || true;
    let mut updates = notice(Ok(asset_at(fixture.url("/setup"), &bytes)), Some(steps));
    updates.update_now();
    assert!(!finish(&mut updates), "Nightjar stays open");
    assert!(matches!(updates.failure, Some(Failure::Apply(_))), "it says why: {:?}", updates.failure);
    assert!(!updates.failure.as_ref().unwrap().is_final(), "Update now stays for afterwards");
    assert!(installer.ran().is_empty() && !installer.marker().exists(), "no second setup, no false alarm");
    assert_eq!(installer.parts(), 0);
}

/// The next Nightjar can read its files as soon as it starts, while this one is still leaving.
#[test]
fn nothing_runs_before_the_caller_has_saved() {
    let bytes = body();
    let served = bytes.clone();
    let fixture = Fixture::start(move |_, _| Reply::Body(served.clone()));
    let installer = Installer::new("saves-first");
    let mut updates = notice(Ok(asset_at(fixture.url("/setup"), &bytes)), Some(installer.steps(native(), local)));
    updates.update_now();
    let deadline = Instant::now() + Duration::from_secs(30);
    let part = loop {
        assert!(Instant::now() < deadline, "the download never ended");
        if let Some(part) = updates.step() { break part; }
        std::thread::sleep(Duration::from_millis(2));
    };
    assert!(installer.ran().is_empty() && !installer.marker().exists(), "the caller's turn to save");
    assert_eq!(std::fs::read(&part).unwrap(), bytes);
    assert!(updates.hand_over(&part));
    assert_eq!(installer.ran(), [bytes]);
    assert!(installer.marker().exists());

    let launcher = include_str!("../app/launcher.rs");
    let frame = launcher.split("self.updates.step()").nth(1).expect("the main loop steps the update");
    let (saved, handed) = (frame.find("self.save_for_next_run()").unwrap(), frame.find("self.updates.hand_over(").unwrap());
    assert!(saved < handed, "the main loop saves first");
}

#[test]
fn a_hand_over_that_fails_after_the_save_stays_open_and_says_why() {
    let bytes = body();
    let served = bytes.clone();
    let fixture = Fixture::start(move |_, _| Reply::Body(served.clone()));
    let installer = Installer::new("late-failure");
    let mut steps = installer.steps(native(), local);
    steps.apply = Box::new(|_, _| Err(Failure::Restart("no".into())));
    let mut updates = notice(Ok(asset_at(fixture.url("/setup"), &bytes)), Some(steps));
    updates.update_now();
    assert!(!finish(&mut updates));
    assert_eq!(updates.failure, Some(Failure::Restart("no".into())));
}

#[cfg(windows)]
#[test]
fn a_stream_here_is_visible_from_other_processes() {
    let mut updates = Updates::default();
    updates.hold(true);
    assert!(self_update::mutex_exists(self_update::STREAM_MUTEX));
    updates.hold(true);
    assert!(updates.stream_mark.is_some(), "while it runs");
    updates.hold(false);
    assert!(updates.stream_mark.is_none(), "and not after");
}

/// Answers the release question and counts what it is asked.
#[derive(Default)]
struct Counting {
    gets: AtomicU64,
    answer: Mutex<String>,
}

impl Transport for Counting {
    fn get(&self, _: &Url, _: bool) -> Result<Response, String> {
        self.gets.fetch_add(1, Ordering::Relaxed);
        let body = self.answer.lock().unwrap().clone().into_bytes();
        Ok(Response { status: 200, location: None, length: None, body: Box::new(std::io::Cursor::new(body)) })
    }
}

fn frame(updates: &mut Updates, ctx: &egui::Context) {
    ctx.run_ui(egui::RawInput::default(), |ui| updates.show(ui.ctx())).textures_delta.clear();
}

#[test]
fn drawing_the_notice_never_takes_the_answer_to_exit() {
    let bytes = body();
    let served = bytes.clone();
    let fixture = Fixture::start(move |_, _| Reply::Body(served.clone()));
    let installer = Installer::new("two-passes");
    let mut updates = notice(Ok(asset_at(fixture.url("/setup"), &bytes)), Some(installer.steps(native(), local)));
    updates.update_now();
    let progress = updates.job.as_ref().unwrap().progress.clone();
    wait_until("the download never ended", || Arc::strong_count(&progress) == 2);
    // egui runs a frame's closure a second time after request_discard.
    let ctx = egui::Context::default();
    frame(&mut updates, &ctx);
    frame(&mut updates, &ctx);
    assert!(installer.ran().is_empty(), "nothing happens while drawing");
    assert!(updates.step().is_some(), "the main loop gets the answer");
}

fn checked(copy: CopyKind, steps: impl FnOnce(Arc<dyn Transport>) -> Option<Steps>) -> (Updates, Arc<Counting>) {
    let count = Arc::new(Counting::default());
    let digest = format!(r#","digest":"{DIGEST}""#);
    *count.answer.lock().unwrap() = answer("v4.1.0", false, false, &setup_asset(&github_url(), &digest));
    let net: Arc<dyn Transport> = count.clone();
    let steps = steps(net.clone());
    let mut updates = Updates::begin(true, copy, net, steps, "4.0.0", || true, Arc::new(|| {}));
    wait_until("no answer", || { updates.step(); updates.found.is_some() });
    (updates, count)
}

#[test]
fn the_notice_starts_nothing_without_the_click() {
    let installer = Installer::new("no-click");
    let ctx = egui::Context::default();
    let (mut updates, count) = checked(CopyKind::Installed, |net| Some(installer.steps(net, local)));
    for _ in 0..20 {
        frame(&mut updates, &ctx);
        assert!(updates.step().is_none());
    }
    assert!(updates.job.is_none());
    assert!(updates.found.as_ref().unwrap().asset.is_ok());
    assert_eq!(count.gets.load(Ordering::Relaxed), 1, "only the release question");
    assert!(installer.ran().is_empty());
}

#[test]
fn a_portable_copy_only_has_the_link() {
    let ctx = egui::Context::default();
    let (mut updates, count) = checked(CopyKind::Portable, |_| None);
    assert_eq!(updates.found.as_ref().unwrap().asset, Err(Failure::NoInstaller));
    updates.update_now();
    for _ in 0..5 {
        frame(&mut updates, &ctx);
        assert!(updates.step().is_none());
    }
    assert!(updates.job.is_none() && updates.failure.is_none());
    assert_eq!(count.gets.load(Ordering::Relaxed), 1);
}

#[test]
fn a_setup_that_did_not_install_says_so_on_the_next_launch() {
    let installer = Installer::new("failed-setup");
    let attempt = Attempt { version: "4.1.0".into(), url: "https://github.com/RamazanKara/nightjar/releases/tag/v4.1.0".into() };
    self_update::write_attempt(&installer.marker(), &attempt).unwrap();
    let count: Arc<dyn Transport> = Arc::new(Counting::default());
    let mut updates = Updates::begin(true, CopyKind::Installed, count.clone(), Some(installer.steps(count, local)), "4.0.0", || false, Arc::new(|| {}));
    assert_eq!(updates.failure, Some(Failure::NotInstalled));
    assert_eq!(updates.found.as_ref().unwrap().url, attempt.url, "the link is there");
    assert!(!installer.marker().exists(), "it says so once");
    updates.update_now();
    assert!(updates.job.is_none(), "nothing to download until the check answers");
    frame(&mut updates, &egui::Context::default());
}

#[test]
fn a_setup_that_did_install_says_nothing() {
    for running in ["4.1.0", "4.2.0"] {
        let installer = Installer::new("good-setup");
        let attempt = Attempt { version: "4.1.0".into(), url: "https://github.com/RamazanKara/nightjar/releases/tag/v4.1.0".into() };
        self_update::write_attempt(&installer.marker(), &attempt).unwrap();
        assert_eq!(failed_attempt(&installer.marker(), running), None, "{running}");
        assert!(!installer.marker().exists());
    }
}

#[test]
fn the_appimage_flow_replaces_the_file_with_the_verified_download() {
    let bytes = body();
    let served = bytes.clone();
    let fixture = Fixture::start(move |_, _| Reply::Body(served.clone()));
    let dir = folder("appimage");
    let target = dir.join("Nightjar.AppImage");
    std::fs::write(&target, b"old").unwrap();
    let started = Arc::new(AtomicBool::new(false));
    let (copy, flag) = (target.clone(), started.clone());
    let steps = Steps {
        net: native(),
        allow: local,
        dir: dir.clone(),
        own_dir: false,
        marker: None,
        stream_elsewhere: || false,
        setup_running: || false,
        apply: Box::new(move |part, _| {
            self_update::replace_file(part, &copy).map_err(Failure::Apply)?;
            flag.store(true, Ordering::Relaxed);
            Ok(())
        }),
    };
    let mut updates = notice(Ok(asset_at(fixture.url("/image"), &bytes)), Some(steps));
    updates.update_now();
    assert!(finish(&mut updates));
    assert_eq!(std::fs::read(&target).unwrap(), bytes);
    assert!(started.load(Ordering::Relaxed));
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "only the new AppImage");
}

#[test]
fn a_failed_hand_over_stays_open_and_says_why() {
    let bytes = body();
    let served = bytes.clone();
    let fixture = Fixture::start(move |_, _| Reply::Body(served.clone()));
    let installer = Installer::new("no-start");
    let mut steps = installer.steps(native(), local);
    steps.apply = Box::new(|part, _| {
        std::fs::remove_file(part).unwrap();
        Err(Failure::Apply("no".into()))
    });
    let mut updates = notice(Ok(asset_at(fixture.url("/setup"), &bytes)), Some(steps));
    updates.update_now();
    assert!(!finish(&mut updates));
    assert_eq!(updates.failure, Some(Failure::Apply("no".into())));
    assert!(!installer.marker().exists(), "no false alarm on the next launch");
}

#[test]
fn every_failure_has_words() {
    for failure in [Failure::NoInstaller, Failure::NoDigest, Failure::Digest, Failure::Size, Failure::Address("x".into()), Failure::Network("x".into()), Failure::Apply("x".into()), Failure::Restart("x".into()), Failure::NotInstalled, Failure::StreamElsewhere] {
        assert!(!failure_text(&failure, "4.1.0").is_empty(), "{failure:?}");
    }
}
