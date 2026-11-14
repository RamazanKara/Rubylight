use super::*;
use std::{
    collections::BTreeMap,
    io::Write,
    sync::{Arc, Mutex},
};

/// A test's NVIDIA files: its own bytes, pinned like NVIDIA's.
const NVIDIA: [(&str, &[u8]); 3] = [
    ("nvngx_truehdr.dll", b"MZ truehdr"),
    ("nvngx_vsr.dll", b"MZ vsr"),
    ("NVIDIA_RTX_Video_SDK_License.pdf", b"%PDF licence"),
];

fn sha(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn test_pins() -> Vec<Pinned> {
    NVIDIA
        .iter()
        .map(|(name, bytes)| Pinned {
            name,
            size: bytes.len() as u64,
            sha256: Box::leak(sha(bytes).into_boxed_str()),
        })
        .collect()
}

fn archive(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, bytes) in files {
        zip.start_file(*name, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(bytes).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

/// The component's archive with `adapter`, NVIDIA's `nvidia` files and what
/// is never taken: other files, and its own names below the top level.
fn component_archive(adapter: &[u8], nvidia: &[(&str, &[u8])]) -> Vec<u8> {
    let nested = format!("sub/{ADAPTER}");
    let mut files: Vec<(&str, &[u8])> = vec![
        (ADAPTER, adapter),
        ("readme.txt", b"not taken"),
        ("dxgi.dll", b"MZ never taken"),
        (&nested, b"MZ not at the top"),
        ("sub/nvngx_vsr.dll", b"MZ not at the top"),
    ];
    files.extend_from_slice(nvidia);
    archive(&files)
}

type Files = Arc<BTreeMap<String, Vec<u8>>>;
type ServerState = (Arc<Value>, Files, Arc<Mutex<Vec<String>>>);

struct Server {
    base: String,
    log: Arc<Mutex<Vec<String>>>,
}

/// GitHub with this version's release holding `files`, each with its own
/// digest unless `digests` names another.
async fn serve(files: BTreeMap<String, Vec<u8>>, digests: BTreeMap<String, String>) -> Server {
    use axum::{
        extract::{Path as Url, State},
        http::StatusCode,
        response::IntoResponse,
        routing::get,
    };
    async fn release_route(
        State((release, _, log)): State<ServerState>,
        Url((owner, repo, tag)): Url<(String, String, String)>,
    ) -> axum::response::Response {
        log.lock()
            .unwrap()
            .push(format!("release {owner}/{repo} {tag}"));
        if format!("{owner}/{repo}") != crate::updater::REPOSITORIES[0] {
            return StatusCode::NOT_FOUND.into_response();
        }
        axum::Json((*release).clone()).into_response()
    }
    async fn file_route(
        State((_, files, log)): State<ServerState>,
        Url((_, _, _, name)): Url<(String, String, String, String)>,
    ) -> axum::response::Response {
        log.lock().unwrap().push(name.clone());
        match files.get(&name) {
            Some(bytes) => bytes.clone().into_response(),
            None => StatusCode::NOT_FOUND.into_response(),
        }
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let tag = format!("v{VERSION}");
    let repository = crate::updater::REPOSITORIES[0];
    let assets: Vec<Value> = files
        .iter()
        .map(|(name, bytes)| {
            let digest = digests.get(name).cloned().unwrap_or_else(|| sha(bytes));
            json!({
                "name": name,
                "browser_download_url":
                    format!("{base}/{repository}/releases/download/{tag}/{name}"),
                "digest": format!("sha256:{digest}"),
                "size": bytes.len(),
            })
        })
        .collect();
    let release = Arc::new(json!({"tag_name": tag, "assets": assets}));
    let log = Arc::new(Mutex::new(Vec::new()));
    let app = axum::Router::new()
        .route(
            "/api/repos/{owner}/{repo}/releases/tags/{tag}",
            get(release_route),
        )
        .route(
            "/{owner}/{repo}/releases/download/{tag}/{name}",
            get(file_route),
        )
        .with_state((release, Arc::new(files), log.clone()));
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Server { base, log }
}

fn use_server(h: &Shared, server: &Server) {
    *h.nvidia_component.origin.lock().unwrap() = Some(server.base.clone());
    *h.nvidia_component.runtime.lock().unwrap() = Some(test_pins());
}

fn names(folder: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(folder)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

fn component_files() -> Vec<String> {
    let mut names: Vec<String> = NVIDIA.iter().map(|(name, _)| name.to_string()).collect();
    names.push(ADAPTER.into());
    names.sort();
    names
}

fn release_assets(adapter: &[u8]) -> BTreeMap<String, Vec<u8>> {
    BTreeMap::from([(pins::asset(VERSION), component_archive(adapter, &NVIDIA))])
}

/// The download takes only the component's own files from each archive,
/// checks them all, and writes the record naming them last; the host then
/// finds them, and nothing staged stays behind.
#[tokio::test(flavor = "multi_thread")]
async fn a_download_installs_only_the_components_checked_files() {
    let f = crate::state::test_support::Fixture::new();
    let h = &f.host;
    let server = serve(release_assets(b"MZ adapter"), BTreeMap::new()).await;
    use_server(h, &server);

    let record = fetch(h).await.unwrap();
    assert_eq!(record.version, VERSION);
    assert_eq!(record.adapter_sha256, sha(b"MZ adapter"));
    assert_eq!(record.folder, folder_name(&record.adapter_sha256));
    let root = root(h);
    assert_eq!(Record::read(&root).unwrap(), Some(record.clone()));
    assert_eq!(names(&root), [record.folder.clone(), RECORD.to_string()]);
    assert_eq!(names(&record.files(&root)), component_files());
    assert_eq!(
        std::fs::read(record.files(&root).join(ADAPTER)).unwrap(),
        b"MZ adapter"
    );

    let component = component_for(&h.directory).unwrap();
    assert_eq!(component.folder, record.files(&root));
    assert_eq!(component.data, root.join(NGX_DATA));
    pins::verify_with(&component.folder, &component.adapter_sha256, &test_pins()).unwrap();
    let log = server.log.lock().unwrap().clone();
    assert_eq!(log.len(), 2, "{log:?}");
    assert!(log[0].starts_with("release "), "{log:?}");
    assert_eq!(log[1], pins::asset(VERSION));
}

/// A host update installs its own component into a folder of its own, and
/// the earlier one goes once the new one is in place.
#[tokio::test(flavor = "multi_thread")]
async fn an_update_installs_its_own_component_beside_the_earlier_one() {
    let f = crate::state::test_support::Fixture::new();
    let h = &f.host;
    let root = root(h);
    let earlier = root.join("4.4.0-0123456789abcdef");
    std::fs::create_dir_all(&earlier).unwrap();
    for (name, bytes) in NVIDIA {
        std::fs::write(earlier.join(name), bytes).unwrap();
    }
    std::fs::write(earlier.join(ADAPTER), b"MZ old adapter").unwrap();
    Record {
        version: "4.4.0".into(),
        folder: "4.4.0-0123456789abcdef".into(),
        adapter_sha256: sha(b"MZ old adapter"),
    }
    .write(&root)
    .unwrap();
    let server = serve(release_assets(b"MZ new"), BTreeMap::new()).await;
    use_server(h, &server);

    let record = fetch(h).await.unwrap();
    assert_eq!(record.adapter_sha256, sha(b"MZ new"));
    assert_ne!(record.folder, "4.4.0-0123456789abcdef");
    assert_eq!(names(&record.files(&root)), component_files());
    assert!(
        earlier.is_dir(),
        "the earlier component stays until the new one is recorded"
    );

    clean(&root, Some(&record.folder)).unwrap();
    assert_eq!(names(&root), [record.folder, RECORD.to_string()]);
}

/// An adapter that differs from its release's digest, or NVIDIA's files that
/// differ from the pins, install nothing and leave nothing staged.
#[tokio::test(flavor = "multi_thread")]
async fn a_file_that_fails_its_check_installs_nothing() {
    let f = crate::state::test_support::Fixture::new();
    let h = &f.host;
    let root = root(h);

    let wrong = BTreeMap::from([(pins::asset(VERSION), "0".repeat(64))]);
    let server = serve(release_assets(b"MZ adapter"), wrong).await;
    use_server(h, &server);
    begin(h, "download").unwrap();
    let outcome = finish(h, fetch(h).await).await;
    end(h, outcome.err());
    let error = summary(h)["error"].as_str().unwrap().to_owned();
    assert!(error.contains("SHA-256 digest"), "{error}");
    assert_eq!(summary(h)["state"], "failed");
    assert_eq!(Record::read(&root).unwrap(), None);
    assert_eq!(names(&root), Vec::<String>::new());

    let mut nvidia = NVIDIA.to_vec();
    nvidia[1] = ("nvngx_vsr.dll", b"MZ changed");
    let bad = BTreeMap::from([(
        pins::asset(VERSION),
        component_archive(b"MZ adapter", &nvidia),
    )]);
    let server = serve(bad, BTreeMap::new()).await;
    use_server(h, &server);
    begin(h, "download").unwrap();
    let outcome = finish(h, fetch(h).await).await;
    end(h, outcome.err());
    let error = summary(h)["error"].as_str().unwrap().to_owned();
    assert!(error.contains("nvngx_vsr.dll"), "{error}");
    assert_eq!(Record::read(&root).unwrap(), None);
    assert_eq!(names(&root), Vec::<String>::new());
}

/// A release without the component, or one whose asset comes from
/// elsewhere or lacks GitHub's digest, is refused before any download.
#[test]
fn only_this_releases_own_assets_are_taken() {
    let tag = format!("v{VERSION}");
    let name = pins::asset(VERSION);
    let listed = |url: String, digest: &str, size: u64| {
        json!({"tag_name": tag, "assets": [{
            "name": name, "browser_download_url": url, "digest": digest, "size": size,
        }]})
    };
    let good = format!("https://github.com/RamazanKara/Rubylight/releases/download/{tag}/{name}");
    let digest = format!("sha256:{}", "a".repeat(64));
    let taken = asset(
        &listed(good.clone(), &digest, 10),
        &name,
        "https://github.com",
    )
    .unwrap();
    assert_eq!(taken.url, good);
    assert_eq!(taken.sha256, "a".repeat(64));

    let elsewhere = format!("https://github.com/someone/else/releases/download/{tag}/{name}");
    for (release, why) in [
        (
            listed(elsewhere, &digest, 10),
            "Rubylight release repository",
        ),
        (listed(good.clone(), "", 10), "SHA-256 digest"),
        (listed(good.clone(), "sha1:abc", 10), "SHA-256 digest"),
        (listed(good.clone(), &digest, 0), "invalid size"),
        (listed(good.clone(), &digest, MAX_ASSET + 1), "invalid size"),
        (json!({"tag_name": tag, "assets": []}), "has no"),
    ] {
        let error = asset(&release, &name, "https://github.com").unwrap_err();
        assert!(format!("{error:#}").contains(why), "{why}: {error:#}");
    }
}

/// Removal forgets the component and leaves only NGX's data folder.
#[tokio::test(flavor = "multi_thread")]
async fn removal_deletes_the_component_and_keeps_nothing_loadable() {
    let f = crate::state::test_support::Fixture::new();
    let h = &f.host;
    let server = serve(release_assets(b"MZ adapter"), BTreeMap::new()).await;
    use_server(h, &server);
    fetch(h).await.unwrap();
    let root = root(h);
    std::fs::create_dir_all(root.join(NGX_DATA)).unwrap();
    std::fs::write(root.join("staging-1"), b"left by a crash").unwrap();

    remove(h).unwrap();
    assert_eq!(summary(h)["state"], "missing");
    assert_eq!(names(&root), [NGX_DATA.to_string()]);
    assert!(component_for(&h.directory).is_err());
    assert!(remove(h).is_ok(), "removing nothing is no failure");
}

/// Only one change at a time: a second download or a removal waits.
#[test]
fn one_change_runs_at_a_time() {
    let f = crate::state::test_support::Fixture::new();
    let h = &f.host;
    begin(h, "download").unwrap();
    assert_eq!(summary(h)["state"], "download");
    assert!(begin(h, "download").is_err());
    assert!(remove(h).is_err());
    end(h, None);
    assert!(begin(h, "remove").is_ok());
}

/// A folder reached through a link is never the component's, and clearing
/// up removes only the link.
#[test]
fn a_component_behind_a_link_is_refused() {
    let f = crate::state::test_support::Fixture::new();
    let h = &f.host;
    let root = root(h);
    let elsewhere = h.directory.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::fs::write(elsewhere.join("kept.txt"), b"kept").unwrap();
    std::fs::create_dir_all(&root).unwrap();
    let link = root.join("4.5.0-0123456789abcdef");
    let made = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&link)
        .arg(&elsewhere)
        .output()
        .unwrap();
    assert!(made.status.success(), "{made:?}");
    Record {
        version: VERSION.into(),
        folder: "4.5.0-0123456789abcdef".into(),
        adapter_sha256: "0".repeat(64),
    }
    .write(&root)
    .unwrap();
    let error = component_for(&h.directory).unwrap_err();
    assert!(format!("{error:#}").contains("link"), "{error:#}");
    clean(&root, None).unwrap();
    assert_eq!(names(&elsewhere), ["kept.txt"]);
    assert_eq!(names(&root), [RECORD.to_string()]);
}

/// A component folder that is itself a link to elsewhere is never read,
/// cleared or removed: nothing behind it is deleted.
#[tokio::test(flavor = "multi_thread")]
async fn a_component_folder_that_is_a_link_is_never_cleared() {
    let f = crate::state::test_support::Fixture::new();
    let h = &f.host;
    let root = root(h);
    let elsewhere = h.directory.join("elsewhere");
    std::fs::create_dir_all(elsewhere.join("4.0.0-0123456789abcdef")).unwrap();
    std::fs::write(elsewhere.join("precious.txt"), b"kept").unwrap();
    std::fs::write(elsewhere.join(RECORD), b"{}").unwrap();
    let made = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&root)
        .arg(&elsewhere)
        .output()
        .unwrap();
    assert!(made.status.success(), "{made:?}");

    let error = clean(&root, None).unwrap_err();
    assert!(format!("{error:#}").contains("link"), "{error:#}");
    let error = remove(h).unwrap_err();
    assert!(error.contains("link"), "{error}");
    assert_eq!(summary(h)["state"], "failed");
    start(h);
    for _ in 0..100 {
        if summary(h)["error"]
            .as_str()
            .is_some_and(|e| e.contains("link"))
            && summary(h)["state"] == "failed"
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(
        names(&elsewhere),
        ["4.0.0-0123456789abcdef", RECORD, "precious.txt"]
    );
    std::fs::remove_dir(&root).unwrap();
}

/// A folder an earlier install left with a file still in use is passed over
/// for the next free name, so a download after a removal still installs.
#[tokio::test(flavor = "multi_thread")]
async fn a_folder_still_in_use_is_passed_over() {
    use std::os::windows::fs::OpenOptionsExt;
    let f = crate::state::test_support::Fixture::new();
    let h = &f.host;
    let root = root(h);
    let busy = root.join(folder_name(&sha(b"MZ adapter")));
    std::fs::create_dir_all(&busy).unwrap();
    std::fs::write(busy.join(ADAPTER), b"MZ loaded").unwrap();
    let _held = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(busy.join(ADAPTER))
        .unwrap();
    let server = serve(release_assets(b"MZ adapter"), BTreeMap::new()).await;
    use_server(h, &server);

    let record = fetch(h).await.unwrap();
    assert_eq!(
        record.folder,
        format!("{}-1", folder_name(&record.adapter_sha256))
    );
    assert_eq!(names(&record.files(&root)), component_files());
}

/// The archive `rust/build.ps1 -Nvidia` made installs with the host's own
/// pins: `BUTTERPOLLO_TEST_NVIDIA_COMPONENT=<zip> cargo test -p butterpollo
/// -- --ignored the_built_archive`.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs the archive rust/build.ps1 -Nvidia makes"]
async fn the_built_archive_installs_with_the_hosts_own_pins() {
    let path = std::env::var_os("BUTTERPOLLO_TEST_NVIDIA_COMPONENT")
        .expect("BUTTERPOLLO_TEST_NVIDIA_COMPONENT names the archive");
    let f = crate::state::test_support::Fixture::new();
    let h = &f.host;
    let assets = BTreeMap::from([(pins::asset(VERSION), std::fs::read(path).unwrap())]);
    let server = serve(assets, BTreeMap::new()).await;
    *h.nvidia_component.origin.lock().unwrap() = Some(server.base.clone());

    let record = fetch(h).await.unwrap();
    let component = component_for(&h.directory).unwrap();
    assert_eq!(component.adapter_sha256, record.adapter_sha256);
    pins::verify(&component.folder, &component.adapter_sha256).unwrap();
    assert_eq!(
        names(&component.folder),
        component_files_named(&pins::RUNTIME)
    );
}

fn component_files_named(runtime: &[Pinned]) -> Vec<String> {
    let mut names: Vec<String> = runtime.iter().map(|file| file.name.to_string()).collect();
    names.push(ADAPTER.into());
    names.sort();
    names
}
