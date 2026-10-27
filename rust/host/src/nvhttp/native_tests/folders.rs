//! Shared folders (`docs/native-api.md`, "Shared folders") through the
//! router, into temporary folders. The host runs as the developer here, so
//! the folders are opened without impersonation.
use super::*;
use axum::response::Response;
use butterpollo_core::native_api::{
    feature,
    files::Uploaded,
    folders::{EntryKind, FolderAccess, FolderEntries, FolderList, SharedFolder},
};
use std::{path::PathBuf, time::Duration};
use tokio::io::AsyncWriteExt;

const MIB: usize = 1024 * 1024;

/// A paired device and a host sharing `games` and `music`, beside a folder
/// it doesn't share. The device has no grant until a test gives it one.
struct Folders {
    s: Setup,
    base: PathBuf,
    games: PathBuf,
    private: PathBuf,
}
impl Folders {
    fn new() -> Self {
        let s = Setup::new();
        let base =
            std::env::temp_dir().join(format!("butterpollo-folders-{}", uuid::Uuid::new_v4()));
        let (games, music, private) =
            (base.join("Games"), base.join("Music"), base.join("private"));
        for dir in [&games.join("saves"), &music, &private] {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::write(games.join("readme.txt"), b"hello").unwrap();
        std::fs::write(games.join("saves").join("slot1.sav"), b"slot").unwrap();
        std::fs::write(private.join("secret.txt"), b"secret").unwrap();
        let folders = Self {
            s,
            base,
            games,
            private,
        };
        folders.share(&[
            ("games", "Games", &folders.games),
            ("music", "Music", &music),
        ]);
        // Room for every test, however full the disk of the PC running it.
        folders.transfers().fake_space(1 << 40, 1 << 41);
        folders
    }
    fn set(&self, key: &str, value: &str) {
        self.s
            .host()
            .config
            .write()
            .unwrap()
            .values
            .insert(key.into(), value.into());
    }
    /// What the host shares: `(id, name, path)`.
    fn share(&self, folders: &[(&str, &str, &PathBuf)]) {
        let list: Vec<Value> = folders
            .iter()
            .map(|(id, name, path)| serde_json::json!({"id": id, "name": name, "path": path}))
            .collect();
        self.set("shared_folders", &Value::Array(list).to_string());
    }
    /// The device's grants, as the console stores them.
    fn grant(&self, grants: Value) {
        let mut paired = self.s.host().paired.write().unwrap();
        paired.clients[0]
            .extra
            .insert("shared_folders".into(), grants);
    }
    fn transfers(&self) -> &crate::native_api::files::Transfers {
        &self.s.host().file_transfers
    }
    /// The names in `dir`, `.part` files included.
    fn names(dir: &std::path::Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }
    fn request(&self, method: &str, path: &str, length: Option<u64>, body: Body) -> Request<Body> {
        let mut request = Request::builder()
            .method(method)
            .uri(format!("{PREFIX}{path}"));
        if let Some(length) = length {
            request = request.header(header::CONTENT_LENGTH, length);
        }
        request.body(body).unwrap()
    }
    async fn send(&self, request: Request<Body>) -> Reply {
        let (parts, body) = self.start(request).await.into_parts();
        Reply {
            status: parts.status,
            headers: parts.headers,
            body: to_bytes(body, 4 * MIB).await.unwrap().to_vec(),
        }
    }
    /// Starts a request, leaving its answer's body unread.
    async fn start(&self, request: Request<Body>) -> Response {
        router(self.s.host().clone(), true)
            .layer(Extension(self.s.connection.clone()))
            .oneshot(request)
            .await
            .unwrap()
    }
    async fn put(&self, id: &str, path: &str, bytes: &[u8]) -> Reply {
        self.put_as(id, path, Some(bytes.len() as u64), bytes.to_vec())
            .await
    }
    async fn put_as(&self, id: &str, path: &str, length: Option<u64>, bytes: Vec<u8>) -> Reply {
        self.send(self.request(
            "PUT",
            &format!("/folders/{id}/content?path={path}"),
            length,
            Body::from(bytes),
        ))
        .await
    }
    async fn entries(&self, id: &str, path: &str) -> Reply {
        self.s
            .get(&format!("/folders/{id}/entries?path={path}"))
            .await
    }
    async fn content(&self, id: &str, path: &str) -> Reply {
        self.s
            .get(&format!("/folders/{id}/content?path={path}"))
            .await
    }
}
impl Drop for Folders {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn junction(target: &std::path::Path, link: &std::path::Path) {
    let status = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .stdout(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "mklink /J failed");
}

/// Waits up to 5 s for `done`.
async fn until(what: &str, mut done: impl FnMut() -> bool) {
    for _ in 0..500 {
        if done() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("{what} did not happen within 5 s");
}

#[tokio::test]
async fn a_device_sees_only_the_folders_it_has_a_grant_for() {
    let f = Folders::new();
    // Shared, but not with this device.
    f.s.get("/folders").await.assert_error(403, "forbidden");
    f.entries("games", "").await.assert_error(403, "forbidden");
    f.content("games", "readme.txt")
        .await
        .assert_error(403, "forbidden");

    // A grant for a folder the host doesn't share, and one it can't hold.
    f.grant(
        serde_json::json!({"games": "read", "photos": "write", "../x": "write", "music": "all"}),
    );
    let reply = f.s.get("/folders").await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    assert_eq!(reply.headers[header::CACHE_CONTROL], "no-store");
    let list: FolderList = serde_json::from_slice(&reply.body).unwrap();
    assert_eq!(
        list.folders,
        [SharedFolder {
            id: "games".into(),
            name: "Games".into(),
            access: FolderAccess::Read
        }]
    );
    f.entries("music", "").await.assert_error(403, "forbidden");
    f.entries("photos", "").await.assert_error(404, "not_found");
    f.entries("..%2Fx", "").await.assert_error(403, "forbidden");

    // Full control and every perm bit grant no folder.
    f.s.host().paired.write().unwrap().clients[0].perm = u32::MAX;
    f.entries("music", "").await.assert_error(403, "forbidden");

    // File transfer's own setting doesn't matter; a host sharing nothing does.
    f.set("file_transfer", "disabled");
    assert_eq!(f.entries("games", "").await.status, StatusCode::OK);
    f.set("shared_folders", "[]");
    for reply in [
        f.s.get("/folders").await,
        f.entries("games", "").await,
        f.content("games", "readme.txt").await,
        f.put("games", "x.txt", b"x").await,
    ] {
        reply.assert_error(403, "folders_disabled");
    }
}

#[tokio::test]
async fn capabilities_list_folders_while_the_host_shares_one() {
    let f = Folders::new();
    let capabilities = || async {
        serde_json::from_slice::<Capabilities>(&f.s.get("/capabilities").await.body).unwrap()
    };
    let shared = capabilities().await;
    assert!(shared.features.iter().any(|x| x == feature::FOLDERS));
    assert!(!shared.features.iter().any(|x| x == feature::FOLDERS_MOUNT));
    assert!(!shared.permissions.shared_folders);
    f.grant(serde_json::json!({"games": "read"}));
    assert!(capabilities().await.permissions.shared_folders);
    f.set("shared_folders", "[]");
    let none = capabilities().await;
    assert!(!none.features.iter().any(|x| x.starts_with("folders")));
    // The grant still shows what the device holds.
    assert!(none.permissions.shared_folders);
}

#[tokio::test]
async fn a_folder_is_browsed_folders_first_and_its_files_downloaded_exactly() {
    let f = Folders::new();
    f.grant(serde_json::json!({"games": "read"}));
    std::fs::write(f.games.join("a.txt"), b"aa").unwrap();
    std::fs::write(f.games.join("half.bin.part"), b"half").unwrap();

    let reply = f.entries("games", "").await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    let root: FolderEntries = serde_json::from_slice(&reply.body).unwrap();
    assert_eq!(root.path, "");
    let listed: Vec<(&str, EntryKind, u64)> = root
        .entries
        .iter()
        .map(|e| (e.name.as_str(), e.kind, e.size))
        .collect();
    assert_eq!(
        listed,
        [
            ("saves", EntryKind::Dir, 0),
            ("a.txt", EntryKind::File, 2),
            ("readme.txt", EntryKind::File, 5)
        ]
    );
    // A missing `path` is the folder itself too.
    let bare = f.s.get("/folders/games/entries").await;
    assert_eq!(bare.body, reply.body);

    let saves: FolderEntries =
        serde_json::from_slice(&f.entries("games", "saves").await.body).unwrap();
    assert_eq!(saves.path, "saves");
    assert_eq!(saves.entries[0].name, "slot1.sav");
    assert!(saves.entries[0].modified_unix_ms > 0);

    let file = f.content("games", "saves%2Fslot1.sav").await;
    assert_eq!(file.status, StatusCode::OK);
    assert_eq!(file.body, b"slot");
    assert_eq!(file.headers[header::CONTENT_LENGTH], "4");
    assert_eq!(f.content("games", "saves/slot1.sav").await.body, b"slot");

    for (entries, path) in [
        (true, "missing"),
        (true, "readme.txt"),
        (false, "missing.txt"),
        (false, "saves"),
        (false, ""),
    ] {
        let reply = if entries {
            f.entries("games", path).await
        } else {
            f.content("games", path).await
        };
        reply.assert_error(404, "not_found");
    }
}

#[tokio::test]
async fn no_path_leaves_the_folder() {
    let f = Folders::new();
    f.grant(serde_json::json!({"games": "write"}));
    junction(&f.private, &f.games.join("link"));
    junction(&f.games.join("saves"), &f.games.join("again"));
    // Refused as names before the disk is asked.
    for path in [
        "..",
        "..%2Fprivate%2Fsecret.txt",
        "saves%2F..%2F..%2Fprivate%2Fsecret.txt",
        "%2E%2E%2Fprivate",
        "%2Fsaves",
        "saves%2F",
        "saves%2F%2Fslot1.sav",
        "saves%5C..%5C..%5Cprivate%5Csecret.txt",
        "C:%5CWindows",
        "readme.txt:hidden",
        "readme.txt::$DATA",
        "CON",
        "nul.txt",
        ".",
        "%FF",
    ] {
        f.entries("games", path)
            .await
            .assert_error(400, "invalid_name");
        f.content("games", path)
            .await
            .assert_error(400, "invalid_name");
        f.put("games", &format!("{path}%2Fx.txt"), b"x")
            .await
            .assert_error(400, "invalid_name");
    }
    f.s.get("/folders/games/entries?path=a&path=b")
        .await
        .assert_error(400, "invalid_name");
    // Links lead nowhere, not even back into the folder, and aren't listed.
    for path in ["link", "again"] {
        f.entries("games", path)
            .await
            .assert_error(404, "not_found");
    }
    for path in ["link%2Fsecret.txt", "again%2Fslot1.sav"] {
        f.content("games", path)
            .await
            .assert_error(404, "not_found");
    }
    f.put("games", "link%2Fplanted.txt", b"x")
        .await
        .assert_error(404, "not_found");
    let root: FolderEntries = serde_json::from_slice(&f.entries("games", "").await.body).unwrap();
    assert!(
        root.entries
            .iter()
            .all(|e| e.name != "link" && e.name != "again"),
        "{:?}",
        root.entries
    );
    assert_eq!(Folders::names(&f.private), ["secret.txt"]);
}

#[tokio::test]
async fn an_upload_needs_a_write_grant_and_lands_where_it_was_sent() {
    let f = Folders::new();
    f.grant(serde_json::json!({"games": "read"}));
    f.put("games", "saves%2Fnew.sav", b"new")
        .await
        .assert_error(403, "forbidden");
    assert_eq!(Folders::names(&f.games.join("saves")), ["slot1.sav"]);

    f.grant(serde_json::json!({"games": "write"}));
    let reply = f.put("games", "saves%2Fslot1.sav", b"new").await;
    assert_eq!(reply.status, StatusCode::CREATED, "{}", reply.text());
    let uploaded: Uploaded = serde_json::from_slice(&reply.body).unwrap();
    assert_eq!(
        uploaded,
        Uploaded {
            name: "slot1 (1).sav".into(),
            size: 3
        }
    );
    assert_eq!(
        std::fs::read(f.games.join("saves").join("slot1 (1).sav")).unwrap(),
        b"new"
    );
    assert_eq!(
        std::fs::read(f.games.join("saves").join("slot1.sav")).unwrap(),
        b"slot"
    );
    let reply = f.put("games", "top.txt", b"top").await;
    assert_eq!(reply.status, StatusCode::CREATED, "{}", reply.text());
    assert!(f.games.join("top.txt").is_file());

    // File transfer's rules: a name, a length, the size limit and the room.
    f.put("games", "", b"x")
        .await
        .assert_error(400, "invalid_name");
    f.put("games", "saves%2Fx.part", b"x")
        .await
        .assert_error(400, "invalid_name");
    f.put("games", "missing%2Fx.txt", b"x")
        .await
        .assert_error(404, "not_found");
    f.put_as("games", "a.txt", None, b"hello".to_vec())
        .await
        .assert_error(411, "length_required");
    f.set("file_transfer_max_mb", "1");
    f.put_as("games", "big.bin", Some(MIB as u64 + 1), vec![0; 16])
        .await
        .assert_error(413, "too_large");
    f.set("file_transfer_max_mb", "1000000000000");
    f.put_as("games", "huge.bin", Some(1 << 59), vec![0; 16])
        .await
        .assert_error(507, "insufficient_storage");
    f.put_as("games", "short.bin", Some(100), vec![7; 60])
        .await
        .assert_error(400, "incomplete_upload");
    assert_eq!(Folders::names(&f.games), ["readme.txt", "saves", "top.txt"]);
}

#[tokio::test]
async fn a_refused_upload_is_logged_as_a_shared_folder_upload() {
    let f = Folders::new();
    f.grant(serde_json::json!({"games": "read"}));
    let path = f.base.join("refused.log");
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .without_time()
        .with_ansi(false)
        .with_writer(std::sync::Mutex::new(std::fs::File::create(&path).unwrap()))
        .finish();
    let guard = tracing::subscriber::set_default(subscriber);
    f.put("games", "new.sav", b"new")
        .await
        .assert_error(403, "forbidden");
    drop(guard);
    let log = std::fs::read_to_string(&path).unwrap();
    assert!(
        log.contains("Shared folders refused an upload from"),
        "{log}"
    );
    assert!(!log.contains("File transfer"), "{log}");
}

#[tokio::test]
async fn dropping_a_write_grant_ends_an_upload_and_deletes_its_part() {
    let f = Folders::new();
    f.grant(serde_json::json!({"games": "write"}));
    let (mut writer, reader) = tokio::io::duplex(64 * 1024);
    let request = f.request(
        "PUT",
        "/folders/games/content?path=saves%2Fcut.bin",
        Some(MIB as u64),
        Body::from_stream(tokio_util::io::ReaderStream::new(reader)),
    );
    let upload = tokio::spawn({
        let router = router(f.s.host().clone(), true).layer(Extension(f.s.connection.clone()));
        async move { router.oneshot(request).await.unwrap().status() }
    });
    writer.write_all(b"half").await.unwrap();
    let saves = f.games.join("saves");
    until("the upload to start", || {
        Folders::names(&saves) == ["cut.bin.part", "slot1.sav"]
    })
    .await;
    // Read only now: the upload ends at once.
    f.grant(serde_json::json!({"games": "read"}));
    let status = tokio::time::timeout(Duration::from_secs(5), upload)
        .await
        .expect("the upload ends")
        .unwrap();
    assert_eq!(status, StatusCode::FORBIDDEN);
    until("the part file to go", || {
        Folders::names(&saves) == ["slot1.sav"]
    })
    .await;
    drop(writer);
}

#[tokio::test]
async fn dropping_a_grant_or_the_folder_ends_a_download() {
    let f = Folders::new();
    f.grant(serde_json::json!({"games": "read"}));
    let bytes: Vec<u8> = (0..4 * MIB).map(|i| (i % 249) as u8).collect();
    std::fs::write(f.games.join("big.bin"), &bytes).unwrap();
    let download = || {
        f.request(
            "GET",
            "/folders/games/content?path=big.bin",
            None,
            Body::empty(),
        )
    };

    let response = f.start(download()).await;
    assert_eq!(response.status(), StatusCode::OK);
    f.grant(serde_json::json!({"music": "read"}));
    until("the download to end", || f.transfers().downloads() == 0).await;
    let body = to_bytes(response.into_body(), usize::MAX).await;
    assert!(body.is_err(), "the body ends short");

    // The host stops sharing it, or shares another folder under its id.
    f.grant(serde_json::json!({"games": "read"}));
    let music = f.base.join("Music");
    for shared in [
        vec![("music", "Music", &music)],
        vec![("games", "Games", &music)],
    ] {
        let response = f.start(download()).await;
        assert_eq!(response.status(), StatusCode::OK);
        f.share(&shared);
        until("the download to end", || f.transfers().downloads() == 0).await;
        let body = to_bytes(response.into_body(), usize::MAX).await;
        assert!(body.is_err(), "the body ends short");
        let games = f.games.clone();
        f.share(&[("games", "Games", &games)]);
    }
    // Untouched, the whole file comes.
    let whole = f.send(download()).await;
    assert_eq!(whole.body, bytes);
}
