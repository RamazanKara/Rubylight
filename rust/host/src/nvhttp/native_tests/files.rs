//! File transfer (`docs/native-api.md`, "Files") through the router, into a
//! temporary transfer folder. The host runs as the developer here, so the
//! folder is opened without impersonation.
use super::*;
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
    let files = Files::with(ALL | UPLOAD);
    files.s.get("/files").await.assert_error(403, "forbidden");
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
