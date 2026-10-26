//! File transfer (`docs/native-api.md`, "Files") through the router, into a
//! temporary transfer folder. The host runs as the developer here, so the
//! folder is opened without impersonation.
use super::*;
use axum::response::Response;
use butterpollo_core::native_api::{
    feature,
    files::{FileList, Uploaded},
};
use std::{path::PathBuf, time::Duration};
use tokio::io::AsyncWriteExt;

const UPLOAD: u32 = 1 << 18;
const DOWNLOAD: u32 = 1 << 19;
const MIB: usize = 1024 * 1024;

/// A paired device and a host with File transfer on, into its own folder.
struct Files {
    s: Setup,
    dir: PathBuf,
}
impl Files {
    fn with(perm: u32) -> Self {
        let s = Setup::with(perm);
        let dir = std::env::temp_dir().join(format!("butterpollo-files-{}", uuid::Uuid::new_v4()));
        let files = Self { s, dir };
        files.set("file_transfer", "enabled");
        files.set("file_transfer_dir", &files.dir.display().to_string());
        // Room for every test, however full the disk of the PC running it.
        files.transfers().fake_space(1 << 40, 1 << 41);
        files
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
    /// The names in the folder, `.part` files included.
    fn names(&self) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return vec![];
        };
        let mut names: Vec<String> = entries
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
        let response = router(self.s.host().clone(), true)
            .layer(Extension(self.s.connection.clone()))
            .oneshot(request)
            .await
            .unwrap();
        let (parts, body) = response.into_parts();
        Reply {
            status: parts.status,
            headers: parts.headers,
            body: to_bytes(body, 4 * MIB).await.unwrap().to_vec(),
        }
    }
    /// Uploads `bytes` as `name`, with a Content-Length of `length`.
    async fn upload_as(&self, name: &str, length: Option<u64>, bytes: Vec<u8>) -> Reply {
        self.send(self.request(
            "POST",
            &format!("/files?name={name}"),
            length,
            Body::from(bytes),
        ))
        .await
    }
    async fn upload(&self, name: &str, bytes: Vec<u8>) -> Reply {
        let length = bytes.len() as u64;
        self.upload_as(name, Some(length), bytes).await
    }
}
impl Drop for Files {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[tokio::test]
async fn a_2_mib_upload_is_accepted_while_the_clipboard_still_stops_at_1_mib() {
    let files = Files::with(UPLOAD | DOWNLOAD);
    let bytes: Vec<u8> = (0..2 * MIB).map(|i| (i % 251) as u8).collect();
    let reply = files.upload("clip%20one.bin", bytes.clone()).await;
    assert_eq!(reply.status, StatusCode::CREATED, "{}", reply.text());
    let uploaded: Uploaded = serde_json::from_slice(&reply.body).unwrap();
    assert_eq!(
        uploaded,
        Uploaded {
            name: "clip one.bin".into(),
            size: bytes.len() as u64
        }
    );
    assert_eq!(
        std::fs::read(files.dir.join("clip one.bin")).unwrap(),
        bytes
    );
    assert_eq!(files.names(), ["clip one.bin"]);
    // The same name again gets the next number.
    let again = files.upload("clip%20one.bin", vec![1, 2, 3]).await;
    let again: Uploaded = serde_json::from_slice(&again.body).unwrap();
    assert_eq!((again.name.as_str(), again.size), ("clip one (1).bin", 3));

    // Everywhere else the 1 MiB request limit stands.
    let clipboard = files
        .send(
            Request::builder()
                .method("POST")
                .uri("/actions/clipboard?type=text")
                .header(header::CONTENT_LENGTH, MIB + 1)
                .body(Body::from(vec![b'a'; MIB + 1]))
                .unwrap(),
        )
        .await;
    assert_eq!(clipboard.status, StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn file_transfer_needs_the_setting_and_the_devices_bit() {
    let files = Files::with(ALL | UPLOAD | DOWNLOAD);
    files.set("file_transfer", "disabled");
    files
        .upload("a.txt", b"hello".to_vec())
        .await
        .assert_error(403, "file_transfer_disabled");
    files
        .s
        .get("/files")
        .await
        .assert_error(403, "file_transfer_disabled");
    files
        .s
        .get("/files/content?name=a.txt")
        .await
        .assert_error(403, "file_transfer_disabled");
    assert!(files.names().is_empty());

    let files = Files::with(ALL | DOWNLOAD);
    files
        .upload("a.txt", b"hello".to_vec())
        .await
        .assert_error(403, "forbidden");
    assert!(files.names().is_empty());
    // Upload only: the room and the size limit, not the files.
    let files = Files::with(ALL | UPLOAD);
    files.upload("a.txt", b"hello".to_vec()).await;
    let reply = files.s.get("/files").await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    let list: FileList = serde_json::from_slice(&reply.body).unwrap();
    assert!(list.files.is_empty(), "{:?}", list.files);
    assert!(list.free_bytes > 0);
    assert_eq!(list.max_file_bytes, 4096 * MIB as u64);
    files
        .s
        .get("/files/content?name=a.txt")
        .await
        .assert_error(403, "forbidden");
}

#[tokio::test]
async fn capabilities_list_file_transfer_only_while_it_is_on() {
    let files = Files::with(LIST | UPLOAD);
    let capabilities = || async {
        serde_json::from_slice::<Capabilities>(&files.s.get("/capabilities").await.body).unwrap()
    };
    let on = capabilities().await;
    assert!(on.features.iter().any(|f| f == feature::FILES_UPLOAD));
    assert!(on.features.iter().any(|f| f == feature::FILES_DOWNLOAD));
    assert_eq!(
        (on.permissions.file_upload, on.permissions.file_download),
        (true, false)
    );
    files.set("file_transfer", "disabled");
    let off = capabilities().await;
    assert!(!off.features.iter().any(|f| f.starts_with("files")));
    // The bits still show what the device holds.
    assert!(off.permissions.file_upload);
}

#[tokio::test]
async fn an_upload_needs_a_name_a_length_and_room() {
    let files = Files::with(UPLOAD);
    for name in ["", "..%2Fup.txt", "a%3Ab.txt", "con.txt", "%FF.txt"] {
        files
            .upload(name, b"x".to_vec())
            .await
            .assert_error(400, "invalid_name");
    }
    files
        .send(files.request("POST", "/files", Some(1), Body::from("x")))
        .await
        .assert_error(400, "invalid_name");
    files
        .upload_as("a.txt", None, b"hello".to_vec())
        .await
        .assert_error(411, "length_required");

    files.set("file_transfer_max_mb", "1");
    files
        .upload_as("big.bin", Some(MIB as u64 + 1), vec![0; 16])
        .await
        .assert_error(413, "too_large");
    files.upload("fits.bin", vec![0; MIB]).await;
    assert_eq!(files.names(), ["fits.bin"]);

    // More than the disk holds: refused before a byte is written.
    files.set("file_transfer_max_mb", "1000000000000");
    files
        .upload_as("huge.bin", Some(1 << 59), vec![0; 16])
        .await
        .assert_error(507, "insufficient_storage");
    assert_eq!(files.names(), ["fits.bin"]);
}

#[tokio::test]
async fn a_short_body_is_refused_and_its_part_file_deleted() {
    let files = Files::with(UPLOAD);
    files
        .upload_as("short.bin", Some(100), vec![7; 60])
        .await
        .assert_error(400, "incomplete_upload");
    assert!(files.names().is_empty(), "{:?}", files.names());
    files
        .upload_as("long.bin", Some(10), vec![7; 60])
        .await
        .assert_error(400, "invalid_request");
    assert!(files.names().is_empty(), "{:?}", files.names());
}

#[tokio::test]
async fn one_upload_per_device_at_a_time() {
    let files = Files::with(UPLOAD);
    let (mut writer, reader) = tokio::io::duplex(1024);
    let first = files.request(
        "POST",
        "/files?name=first.bin",
        Some(8),
        Body::from_stream(tokio_util::io::ReaderStream::new(reader)),
    );
    let first = tokio::spawn({
        let router =
            router(files.s.host().clone(), true).layer(Extension(files.s.connection.clone()));
        async move { router.oneshot(first).await.unwrap().status() }
    });
    writer.write_all(b"half").await.unwrap();
    // The first upload holds its slot once its `.part` file exists.
    for _ in 0..500 {
        if !files.names().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(files.names(), ["first.bin.part"]);
    files
        .upload("second.bin", b"x".to_vec())
        .await
        .assert_error(409, "transfer_busy");
    // Another device is not held up.
    let other = other_device(&files.s.f, UPLOAD, false);
    let reply = router(files.s.host().clone(), true)
        .layer(Extension(other))
        .oneshot(files.request("POST", "/files?name=other.bin", Some(1), Body::from("x")))
        .await
        .unwrap();
    assert_eq!(reply.status(), StatusCode::CREATED);

    writer.write_all(b"done").await.unwrap();
    drop(writer);
    assert_eq!(first.await.unwrap(), StatusCode::CREATED);
    assert_eq!(files.names(), ["first.bin", "other.bin"]);
    // The slot is free again.
    let reply = files.upload("second.bin", b"x".to_vec()).await;
    assert_eq!(reply.status, StatusCode::CREATED, "{}", reply.text());
}

#[tokio::test]
async fn files_are_listed_and_downloaded_with_their_exact_length() {
    let files = Files::with(UPLOAD | DOWNLOAD);
    let bytes: Vec<u8> = (0..300_000).map(|i| (i % 253) as u8).collect();
    files.upload("save%20game.zip", bytes.clone()).await;
    files.upload("empty.txt", vec![]).await;
    std::fs::create_dir(files.dir.join("folder")).unwrap();

    let reply = files.s.get("/files").await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    let list: FileList = serde_json::from_slice(&reply.body).unwrap();
    assert_eq!(list.folder, files.dir.display().to_string());
    assert_eq!(list.max_file_bytes, 4096 * MIB as u64);
    assert!(list.free_bytes > 0);
    let mut listed: Vec<_> = list
        .files
        .iter()
        .map(|file| (file.name.as_str(), file.size))
        .collect();
    listed.sort();
    assert_eq!(
        listed,
        [("empty.txt", 0), ("save game.zip", bytes.len() as u64)]
    );

    let reply = files.s.get("/files/content?name=save%20game.zip").await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(
        reply.headers[header::CONTENT_LENGTH],
        bytes.len().to_string()
    );
    assert_eq!(
        reply.headers[header::CONTENT_TYPE],
        "application/octet-stream"
    );
    assert_eq!(reply.body, bytes);
    let reply = files.s.get("/files/content?name=empty.txt").await;
    assert_eq!(
        (
            reply.status,
            reply.headers[header::CONTENT_LENGTH].to_str().unwrap()
        ),
        (StatusCode::OK, "0")
    );

    for missing in ["nothing.txt", "folder"] {
        files
            .s
            .get(&format!("/files/content?name={missing}"))
            .await
            .assert_error(404, "not_found");
    }
    files
        .s
        .get("/files/content?name=..%5Csecret.txt")
        .await
        .assert_error(400, "invalid_name");
}

#[tokio::test]
async fn a_part_name_is_refused_both_ways() {
    let files = Files::with(UPLOAD | DOWNLOAD);
    files
        .upload("x.txt.part", b"x".to_vec())
        .await
        .assert_error(400, "invalid_name");
    files.put("x.txt.part", 7);
    files
        .s
        .get("/files/content?name=x.txt.part")
        .await
        .assert_error(400, "invalid_name");
}

/// Another paired device, `uuid`, with `perm`.
fn device(f: &Fixture, uuid: &str, perm: u32) -> Connection {
    let identity = Identity::generate().unwrap();
    let client = Client {
        name: uuid.into(),
        cert: identity.certificate.clone(),
        uuid: uuid.into(),
        perm,
        enabled: true,
        extra: Default::default(),
    };
    f.host
        .paired
        .write()
        .unwrap()
        .add(&f.host.paired_path, client)
        .unwrap();
    Connection {
        certificate: Some(identity.der.clone()),
        ..f.connection(true)
    }
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

impl Files {
    /// Starts a request as `connection`, leaving its answer's body unread.
    async fn start(&self, connection: &Connection, request: Request<Body>) -> Response {
        router(self.s.host().clone(), true)
            .layer(Extension(connection.clone()))
            .oneshot(request)
            .await
            .unwrap()
    }
    /// A file of `size` bytes in the folder.
    fn put(&self, name: &str, size: usize) -> Vec<u8> {
        let bytes: Vec<u8> = (0..size).map(|i| (i % 249) as u8).collect();
        std::fs::create_dir_all(&self.dir).unwrap();
        std::fs::write(self.dir.join(name), &bytes).unwrap();
        bytes
    }
    fn download(&self, name: &str) -> Request<Body> {
        self.request(
            "GET",
            &format!("/files/content?name={name}"),
            None,
            Body::empty(),
        )
    }
    /// An upload of `length` bytes as `name`, its body written to the
    /// returned writer.
    fn streamed(&self, name: &str, length: u64) -> (Request<Body>, tokio::io::DuplexStream) {
        let (writer, reader) = tokio::io::duplex(64 * 1024);
        let request = self.request(
            "POST",
            &format!("/files?name={name}"),
            Some(length),
            Body::from_stream(tokio_util::io::ReaderStream::new(reader)),
        );
        (request, writer)
    }
    fn transfers(&self) -> &crate::native_api::files::Transfers {
        &self.s.host().file_transfers
    }
    async fn free_bytes(&self) -> u64 {
        let reply = self.s.get("/files").await;
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
        serde_json::from_slice::<FileList>(&reply.body)
            .unwrap()
            .free_bytes
    }
}

/// 768 KiB at 8 Mbit/s: the 256 KiB burst, then about half a second.
const PACED: usize = 768 * 1024;
const PACED_LEAST: Duration = Duration::from_millis(400);

#[tokio::test]
async fn a_download_is_paced_while_a_stream_runs_or_starts() {
    let files = Files::with(UPLOAD | DOWNLOAD);
    files.set("file_transfer_stream_mbps", "8");
    let bytes = files.put("game.sav", PACED);
    files.s.f.session(
        files.s.client.clone(),
        butterpollo_core::session::Role::Stream,
    );
    let started = std::time::Instant::now();
    assert_eq!(
        files.s.get("/files/content?name=game.sav").await.body,
        bytes
    );
    assert!(started.elapsed() >= PACED_LEAST, "{:?}", started.elapsed());

    // A stream starting paces too.
    let files = Files::with(DOWNLOAD);
    files.set("file_transfer_stream_mbps", "8");
    let bytes = files.put("game.sav", PACED);
    let launch = files.s.f.launch(
        files.s.client.clone(),
        butterpollo_core::session::Role::Stream,
    );
    files
        .s
        .host()
        .sessions
        .lock()
        .unwrap()
        .pending
        .insert("starting".into(), launch);
    let started = std::time::Instant::now();
    assert_eq!(
        files.s.get("/files/content?name=game.sav").await.body,
        bytes
    );
    assert!(started.elapsed() >= PACED_LEAST, "{:?}", started.elapsed());
}

#[tokio::test]
async fn an_upload_is_paced_while_a_stream_runs() {
    let files = Files::with(UPLOAD);
    files.set("file_transfer_stream_mbps", "8");
    files.s.f.session(
        files.s.client.clone(),
        butterpollo_core::session::Role::Stream,
    );
    let started = std::time::Instant::now();
    let reply = files.upload("paced.bin", vec![1; PACED]).await;
    assert_eq!(reply.status, StatusCode::CREATED, "{}", reply.text());
    assert!(started.elapsed() >= PACED_LEAST, "{:?}", started.elapsed());
}

#[tokio::test]
async fn downloads_are_capped_per_device_and_in_all() {
    let files = Files::with(DOWNLOAD);
    files.put("big.bin", 4 * MIB);
    // Made first: a new identity takes a while in a debug build.
    let others: Vec<_> = (0..3)
        .map(|n| device(&files.s.f, &format!("device-{n}"), DOWNLOAD))
        .collect();
    let fifth = device(&files.s.f, "device-fifth", DOWNLOAD);
    let me = files.s.connection.clone();
    let mut held = vec![];
    for _ in 0..2 {
        let response = files.start(&me, files.download("big.bin")).await;
        assert_eq!(response.status(), StatusCode::OK);
        held.push(response);
    }
    files
        .s
        .get("/files/content?name=big.bin")
        .await
        .assert_error(409, "transfer_busy");
    for other in &others {
        for _ in 0..2 {
            let response = files.start(other, files.download("big.bin")).await;
            assert_eq!(response.status(), StatusCode::OK);
            held.push(response);
        }
    }
    assert_eq!(files.transfers().downloads(), 8);
    let refused = files.start(&fifth, files.download("big.bin")).await;
    assert_eq!(refused.status(), StatusCode::CONFLICT);

    // A client that goes away gives its slot back.
    held.pop();
    until("a slot to free", || files.transfers().downloads() == 7).await;
    let response = files.start(&fifth, files.download("big.bin")).await;
    assert_eq!(response.status(), StatusCode::OK);
    drop((held, response));
    until("all slots to free", || files.transfers().downloads() == 0).await;
}

#[tokio::test]
async fn a_download_the_client_stops_taking_ends_and_closes_its_file() {
    let files = Files::with(DOWNLOAD);
    files
        .transfers()
        .limit(|limits| limits.idle = Duration::from_millis(300));
    files.put("big.bin", 4 * MIB);
    let me = files.s.connection.clone();
    let response = files.start(&me, files.download("big.bin")).await;
    assert_eq!(response.status(), StatusCode::OK);
    until("the stalled download to end", || {
        files.transfers().downloads() == 0
    })
    .await;
    std::fs::remove_file(files.dir.join("big.bin")).unwrap();
    let body = to_bytes(response.into_body(), usize::MAX).await;
    assert!(body.is_err(), "the body ends short");
}

#[tokio::test]
async fn a_file_another_program_holds_is_busy() {
    use std::os::windows::fs::OpenOptionsExt;
    let files = Files::with(DOWNLOAD);
    files.put("locked.txt", 10);
    let locked = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(files.dir.join("locked.txt"))
        .unwrap();
    files
        .s
        .get("/files/content?name=locked.txt")
        .await
        .assert_error(409, "transfer_busy");
    drop(locked);
    assert_eq!(
        files.s.get("/files/content?name=locked.txt").await.status,
        StatusCode::OK
    );
}

#[tokio::test]
async fn a_device_refused_for_room_can_send_again_at_once() {
    const GIB: u64 = 1 << 30;
    let files = Files::with(UPLOAD | DOWNLOAD);
    files.set("file_transfer_max_mb", "16384");
    // 8 GiB beside the 2 GiB (2 %) reserve.
    files.transfers().fake_space(10 * GIB, 100 * GIB);
    // The refused upload's writer is still running when the retry comes.
    files.transfers().hold_refusals(Duration::from_secs(2));
    let refused = files
        .upload_as("big.bin", Some(9 * GIB), b"x".to_vec())
        .await;
    assert_eq!(refused.status, StatusCode::INSUFFICIENT_STORAGE);
    let sent = files.upload("small.txt", b"hi".to_vec()).await;
    assert_eq!(sent.status, StatusCode::CREATED, "{}", sent.text());
}

#[tokio::test]
async fn an_upload_leaves_room_for_the_uploads_running() {
    const GIB: u64 = 1 << 30;
    let files = Files::with(UPLOAD | DOWNLOAD);
    files.set("file_transfer_max_mb", "8192");
    let other = device(&files.s.f, "second-device", UPLOAD);
    // 8 GiB beside the 2 GiB (2 %) reserve.
    files.transfers().fake_space(10 * GIB, 100 * GIB);
    assert_eq!(files.free_bytes().await, 8 * GIB);
    let (first, mut writer) = files.streamed("first.bin", 5 * GIB);
    let first = tokio::spawn({
        let router =
            router(files.s.host().clone(), true).layer(Extension(files.s.connection.clone()));
        async move { router.oneshot(first).await.unwrap().status() }
    });
    writer.write_all(b"half").await.unwrap();
    until("the first upload to start", || {
        files.names() == ["first.bin.part"]
    })
    .await;
    let mut free = 0;
    for _ in 0..500 {
        free = files.free_bytes().await;
        if free == 3 * GIB + 4 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(free, 3 * GIB + 4, "less what the first upload still writes");

    let second = |length: u64| {
        files.request(
            "POST",
            "/files?name=second.bin",
            Some(length),
            Body::from("x"),
        )
    };
    let refused = files.start(&other, second(4 * GIB)).await;
    assert_eq!(refused.status(), StatusCode::INSUFFICIENT_STORAGE);
    // This one fits, and only falls short of its length.
    let short = files.start(&other, second(3 * GIB)).await;
    assert_eq!(short.status(), StatusCode::BAD_REQUEST);

    drop(writer);
    assert_eq!(first.await.unwrap(), StatusCode::BAD_REQUEST);
    assert!(files.names().is_empty(), "{:?}", files.names());
    assert_eq!(files.free_bytes().await, 8 * GIB);
}

#[tokio::test]
async fn an_upload_slower_than_the_least_rate_is_refused() {
    let files = Files::with(UPLOAD);
    files
        .transfers()
        .limit(|limits| limits.grace = Duration::from_millis(300));
    let (request, mut writer) = files.streamed("slow.bin", MIB as u64);
    writer.write_all(b"x").await.unwrap();
    let started = std::time::Instant::now();
    let reply = files.send(request).await;
    reply.assert_error(400, "incomplete_upload");
    assert!(reply.text().contains("slower"), "{}", reply.text());
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(files.names().is_empty(), "{:?}", files.names());
    drop(writer);
}

#[tokio::test]
async fn turning_file_transfer_off_ends_an_upload_and_deletes_its_part() {
    let files = Files::with(UPLOAD);
    let (request, mut writer) = files.streamed("cut.bin", MIB as u64);
    let upload = tokio::spawn({
        let router =
            router(files.s.host().clone(), true).layer(Extension(files.s.connection.clone()));
        async move { router.oneshot(request).await.unwrap().status() }
    });
    writer.write_all(b"half").await.unwrap();
    until("the upload to start", || files.names() == ["cut.bin.part"]).await;
    files.set("file_transfer", "disabled");
    let status = tokio::time::timeout(Duration::from_secs(5), upload)
        .await
        .expect("the upload ends")
        .unwrap();
    assert_eq!(status, StatusCode::FORBIDDEN);
    until("the part file to go", || files.names().is_empty()).await;
    drop(writer);
}

#[tokio::test]
async fn revoking_download_files_ends_a_download() {
    let files = Files::with(DOWNLOAD);
    files.put("big.bin", 4 * MIB);
    let me = files.s.connection.clone();
    let response = files.start(&me, files.download("big.bin")).await;
    assert_eq!(response.status(), StatusCode::OK);
    files.s.host().paired.write().unwrap().clients[0].perm = UPLOAD;
    until("the download to end", || files.transfers().downloads() == 0).await;
    let body = to_bytes(response.into_body(), usize::MAX).await;
    assert!(body.is_err(), "the body ends short");
}

/// Serves one connection of the router on a real socket. Without
/// `connection` the caller is an unpaired plain-HTTP client.
async fn serve_once(files: &Files, connection: Option<Connection>) -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let mut router = router(files.s.host().clone(), true);
    if let Some(connection) = connection {
        // Inside the plain connection's layer, so it wins.
        router = router.layer(Extension(connection));
    }
    tokio::spawn(async move {
        let (socket, peer) = listener.accept().await.unwrap();
        crate::tls::serve_connection(socket, peer, address, router, None).await;
    });
    address
}
fn upload_head(length: u64) -> String {
    format!(
        "POST {PREFIX}/files?name=a.bin HTTP/1.1\r\nHost: pc\r\nContent-Length: {length}\r\n\r\n"
    )
}
/// Writes zeros until the host stops reading; how much went out.
async fn blast(mut writer: tokio::net::tcp::OwnedWriteHalf) -> u64 {
    let chunk = vec![0; 64 * 1024];
    let mut sent = 0u64;
    while writer.write_all(&chunk).await.is_ok() {
        sent += chunk.len() as u64;
    }
    sent
}

#[tokio::test]
async fn an_unpaired_upload_is_answered_and_closed_at_once() {
    let files = Files::with(UPLOAD);
    let address = serve_once(&files, None).await;
    let socket = tokio::net::TcpStream::connect(address).await.unwrap();
    let (_reader, mut writer) = socket.into_split();
    writer
        .write_all(upload_head(1 << 40).as_bytes())
        .await
        .unwrap();
    let sent = tokio::time::timeout(Duration::from_secs(5), blast(writer))
        .await
        .expect("the host closes an unpaired upload at once");
    assert!(sent < 64 * MIB as u64, "{sent}");
    assert!(files.names().is_empty());
}

#[tokio::test]
async fn a_refused_upload_is_drained_for_a_while_then_closed() {
    let files = Files::with(UPLOAD);
    files.set("file_transfer_max_mb", "1");
    files
        .transfers()
        .limit(|limits| limits.drain_time = Duration::from_secs(1));
    let address = serve_once(&files, Some(files.s.connection.clone())).await;
    let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
    let started = std::time::Instant::now();
    socket
        .write_all(upload_head(64 * MIB as u64).as_bytes())
        .await
        .unwrap();
    socket.write_all(&vec![0; MIB]).await.unwrap();
    // The client reads its answer, not a reset, and the host closes.
    let mut answer = vec![];
    tokio::time::timeout(
        Duration::from_secs(5),
        tokio::io::AsyncReadExt::read_to_end(&mut socket, &mut answer),
    )
    .await
    .expect("the host closes after the drain")
    .unwrap();
    let answer = String::from_utf8_lossy(&answer).to_lowercase();
    assert!(answer.starts_with("http/1.1 413"), "{answer}");
    assert!(answer.contains("connection: close"), "{answer}");
    assert!(answer.contains("too_large"), "{answer}");
    assert!(started.elapsed() >= Duration::from_millis(900));

    // And no more than its bytes, however fast the client sends.
    let files = Files::with(UPLOAD);
    files.set("file_transfer_max_mb", "1");
    files.transfers().limit(|limits| {
        limits.drain_bytes = 2 * MIB as u64;
        limits.drain_time = Duration::from_secs(60);
    });
    let address = serve_once(&files, Some(files.s.connection.clone())).await;
    let socket = tokio::net::TcpStream::connect(address).await.unwrap();
    let (_reader, mut writer) = socket.into_split();
    writer
        .write_all(upload_head(1 << 40).as_bytes())
        .await
        .unwrap();
    let sent = tokio::time::timeout(Duration::from_secs(10), blast(writer))
        .await
        .expect("the host stops draining after its bytes");
    assert!(sent < 64 * MIB as u64, "{sent}");
}
