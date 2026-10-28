//! The JNI side of `com.limelight.nvstream.jni.FileTransferBridge`: File transfer (Experimental)
//! for the Android app, on nightjar-core's [`Transfers`] so Android and the desktop client queue,
//! limit, cancel and report the same way.
//!
//! Java holds a handle per screen (`create`/`destroy`); handles are keys into a registry, never
//! pointers, so a call that races `destroy` finds nothing instead of freed memory. The UI thread
//! queues, cancels, retries and reads `snapshot`; `startNext` names the transfers to run, and a
//! worker thread runs each with `run`, handing over the file descriptor it opened (an upload
//! reads it, a download writes it). `run` blocks on HTTPS, reports progress to the listener
//! (`onProgress(long id, long done, long total)`) and answers with the transfer's row.
//!
//! The rows and errors are JSON: a row carries the state and, for a failed transfer, the problem
//! id ([`Problem::id`]) Java maps to its text; `list` answers `{"ok": <FileList>}` or
//! `{"error": {"problem", "message", "http_status"?, "code"?}}`. Nothing resumes: the host has no
//! ranges, so a retry starts from the first byte.

use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use jni::objects::{JByteArray, JClass, JObject, JString, JValue};
use jni::sys::{jboolean, jint, jlong, jstring, JNI_FALSE, JNI_TRUE};
use jni::JNIEnv;
use serde_json::{json, Value};

use nightjar_core::http::transfers::{Direction, Problem, Start, State, Transfer, Transfers};
use nightjar_core::http::{Error, FileError, FileList, HostClient, TransferCancel, Uploaded};

use crate::native_api::{self, Endpoint};

/// The calls of `HostClient` the transfers make, so `run` is testable without a host.
pub trait FileHost {
    fn list(&self) -> Result<FileList, FileError>;
    fn upload(
        &self,
        name: &str,
        file: &mut dyn Read,
        length: u64,
        progress: &mut dyn FnMut(u64, u64),
        stop: &TransferCancel,
    ) -> Result<Uploaded, FileError>;
    fn download(
        &self,
        name: &str,
        sink: &mut dyn Write,
        progress: &mut dyn FnMut(u64, u64),
        stop: &TransferCancel,
    ) -> Result<u64, FileError>;
}

impl FileHost for HostClient {
    fn list(&self) -> Result<FileList, FileError> {
        self.list_files()
    }
    fn upload(
        &self,
        name: &str,
        file: &mut dyn Read,
        length: u64,
        progress: &mut dyn FnMut(u64, u64),
        stop: &TransferCancel,
    ) -> Result<Uploaded, FileError> {
        self.upload_file(name, file, length, progress, stop)
    }
    fn download(
        &self,
        name: &str,
        sink: &mut dyn Write,
        progress: &mut dyn FnMut(u64, u64),
        stop: &TransferCancel,
    ) -> Result<u64, FileError> {
        self.download_file(name, sink, progress, stop)
    }
}

#[derive(Default)]
struct Entry {
    transfers: Transfers,
    /// Started by `start_next` and not yet taken by `run`.
    started: HashMap<u64, Start>,
}

static ENTRIES: Mutex<BTreeMap<i64, Arc<Mutex<Entry>>>> = Mutex::new(BTreeMap::new());
static NEXT_HANDLE: AtomicI64 = AtomicI64::new(1);

/// A lock that a panicked holder did not poison for good: the list is plain data.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub fn create() -> i64 {
    let handle = NEXT_HANDLE.fetch_add(1, Ordering::Relaxed);
    lock(&ENTRIES).insert(handle, Arc::default());
    handle
}

/// Cancels what is still running and forgets the handle; running workers finish on their own.
pub fn destroy(handle: i64) {
    if let Some(entry) = lock(&ENTRIES).remove(&handle) {
        lock(&entry).transfers.cancel_all();
    }
}

fn entry(handle: i64) -> Option<Arc<Mutex<Entry>>> {
    lock(&ENTRIES).get(&handle).cloned()
}

fn with<R>(handle: i64, f: impl FnOnce(&mut Entry) -> R) -> Option<R> {
    let entry = entry(handle)?;
    let mut entry = lock(&entry);
    Some(f(&mut entry))
}

/// The new transfer's id, or -1 for a handle that is gone.
pub fn queue_upload(handle: i64, local_name: &str, size: u64) -> i64 {
    with(handle, |e| {
        e.transfers.queue_upload(local_name, size) as i64
    })
    .unwrap_or(-1)
}

pub fn queue_download(handle: i64, name: &str, local_name: &str, size: u64) -> i64 {
    with(handle, |e| {
        e.transfers.queue_download(name, local_name, size) as i64
    })
    .unwrap_or(-1)
}

pub fn cancel(handle: i64, id: u64) -> bool {
    with(handle, |e| {
        // A started transfer no worker took yet stops at once in `run`.
        e.transfers.cancel(id)
    })
    .unwrap_or(false)
}

pub fn cancel_all(handle: i64) {
    with(handle, |e| e.transfers.cancel_all());
}

pub fn retry(handle: i64, id: u64) -> bool {
    with(handle, |e| e.transfers.retry(id)).unwrap_or(false)
}

pub fn clear_finished(handle: i64) {
    with(handle, |e| e.transfers.clear_finished());
}

fn direction(direction: Direction) -> &'static str {
    match direction {
        Direction::Upload => "upload",
        Direction::Download => "download",
    }
}

/// One transfer as Java shows it.
pub fn row(t: &Transfer) -> Value {
    let mut row = json!({
        "id": t.id,
        "direction": direction(t.direction),
        "name": t.name,
        "local_name": t.local_name,
        "size": t.size,
        "cancelling": t.is_cancelling(),
        "can_retry": t.can_retry(),
    });
    match &t.state {
        State::Queued => row["state"] = json!("queued"),
        State::Running { done, total } => {
            row["state"] = json!("running");
            row["done"] = json!(done);
            row["total"] = json!(total);
        }
        State::Done { name, size } => {
            row["state"] = json!("done");
            row["saved_name"] = json!(name);
            row["done"] = json!(size);
            row["total"] = json!(size);
        }
        State::Failed { problem, message } => {
            row["state"] = json!("failed");
            row["problem"] = json!(problem.id());
            row["console_fix"] = json!(problem.console_fix());
            row["message"] = json!(message);
        }
        State::Cancelled => row["state"] = json!("cancelled"),
    }
    row
}

/// Every transfer of the handle, oldest first; `[]` for a handle that is gone.
pub fn snapshot(handle: i64) -> String {
    with(handle, |e| {
        Value::Array(e.transfers.items().iter().map(row).collect()).to_string()
    })
    .unwrap_or_else(|| "[]".into())
}

/// Starts what the host's limits leave room for and names it: `[{"id", "direction", "name",
/// "local_name", "size"}]`. Java runs each with `run`.
pub fn start_next(handle: i64) -> String {
    with(handle, |e| {
        let started = e.transfers.start_next();
        let rows: Vec<Value> = started
            .iter()
            .map(|s| {
                json!({
                    "id": s.id,
                    "direction": direction(s.direction),
                    "name": s.name,
                    "local_name": s.local_name,
                    "size": s.size,
                })
            })
            .collect();
        for start in started {
            e.started.insert(start.id, start);
        }
        Value::Array(rows).to_string()
    })
    .unwrap_or_else(|| "[]".into())
}

fn finish(entry: &Mutex<Entry>, start: &Start, result: Result<FinishedAs, FileError>) -> Value {
    let mut entry = lock(entry);
    match (start.direction, result) {
        (Direction::Upload, Ok(FinishedAs::Uploaded(uploaded))) => {
            entry.transfers.finish_upload(start.id, Ok(uploaded))
        }
        (Direction::Upload, Err(error)) => entry.transfers.finish_upload(start.id, Err(error)),
        (Direction::Download, Ok(FinishedAs::Downloaded(size))) => {
            entry.transfers.finish_download(start.id, Ok(size))
        }
        (Direction::Download, Err(error)) => entry.transfers.finish_download(start.id, Err(error)),
        (_, Ok(_)) => unreachable!("an upload answers with Uploaded, a download with its length"),
    }
    entry
        .transfers
        .get(start.id)
        .map(row)
        .unwrap_or(Value::Null)
}

enum FinishedAs {
    Uploaded(Uploaded),
    Downloaded(u64),
}

fn invalid(message: &str) -> String {
    json!({ "error": { "problem": Problem::Local.id(), "message": message } }).to_string()
}

/// Runs the started transfer `id` on `file`: reads it for an upload, writes it for a download.
/// `host` is the client and `file` the opened file, or why there is none: the transfer then
/// fails with it instead of staying started. Answers with
/// `{"ok": <row>}`, or an error when the handle is gone or `id` was not started.
pub fn run<H: FileHost>(
    handle: i64,
    id: u64,
    host: Result<H, Error>,
    file: std::io::Result<File>,
    progress: &mut dyn FnMut(u64, u64),
) -> String {
    let Some(entry) = entry(handle) else {
        return invalid("the transfer list is closed");
    };
    let Some(start) = lock(&entry).started.remove(&id) else {
        return invalid("that transfer was not started");
    };
    let result = file.map_err(FileError::Local).and_then(|mut file| {
        let host = host.map_err(FileError::Host)?;
        let mut report = |done: u64, total: u64| {
            if lock(&entry).transfers.progress(id, done, total) {
                progress(done, total);
            }
        };
        match start.direction {
            Direction::Upload => host
                .upload(
                    &start.name,
                    &mut file,
                    start.size,
                    &mut report,
                    &start.cancel,
                )
                .map(FinishedAs::Uploaded),
            Direction::Download => host
                .download(&start.name, &mut file, &mut report, &start.cancel)
                .and_then(|size| {
                    // What arrived is on disk before the row says done.
                    file.sync_all().map_err(FileError::Local)?;
                    Ok(FinishedAs::Downloaded(size))
                }),
        }
    });
    json!({ "ok": finish(&entry, &start, result) }).to_string()
}

/// `{"error": {"problem", "message", "http_status"?, "code"?}}`; a cancel is problem
/// `cancelled`.
pub fn file_error_envelope(error: &FileError) -> String {
    let mut body = json!({
        "problem": Problem::of(error).map_or("cancelled", Problem::id),
        "message": error.to_string(),
    });
    match error {
        FileError::Refused {
            refusal, status, ..
        } => {
            body["http_status"] = json!(status);
            body["code"] = json!(refusal.code());
        }
        FileError::Host(error) => {
            if let Some(status) = error.http_status() {
                body["http_status"] = json!(status);
            }
        }
        FileError::Local(_) => {}
    }
    json!({ "error": body }).to_string()
}

/// The PC's files: `{"ok": <FileList>}` or a file error.
pub fn list<H: FileHost>(host: Result<H, Error>) -> String {
    match host.map_err(FileError::Host).and_then(|host| host.list()) {
        Ok(list) => match serde_json::to_value(list) {
            Ok(list) => json!({ "ok": list }).to_string(),
            Err(e) => file_error_envelope(&FileError::Host(Error::Malformed(e.to_string()))),
        },
        Err(error) => file_error_envelope(&error),
    }
}

fn client(endpoint: Option<Endpoint>) -> Result<HostClient, Error> {
    match endpoint {
        Some(endpoint) => native_api::client(&endpoint),
        None => Err(Error::InvalidArgument(
            "missing or invalid host endpoint".into(),
        )),
    }
}

fn reply(env: &mut JNIEnv, reply: String) -> jstring {
    match env.new_string(reply) {
        Ok(reply) => reply.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

fn flag(value: bool) -> jboolean {
    if value {
        JNI_TRUE
    } else {
        JNI_FALSE
    }
}

fn id(value: jlong) -> u64 {
    u64::try_from(value).unwrap_or(0)
}

/// Owns the descriptor Java detached, so it is closed on every path.
#[cfg(unix)]
fn owned_file(fd: jint) -> Option<File> {
    use std::os::unix::io::FromRawFd;
    // SAFETY: Java hands over a descriptor it detached (`ParcelFileDescriptor.detachFd`) and no
    // longer closes; -1 is no descriptor.
    (fd >= 0).then(|| unsafe { File::from_raw_fd(fd) })
}

#[cfg(not(unix))]
fn owned_file(_fd: jint) -> Option<File> {
    None
}

#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_FileTransferBridge_create(
    _: JNIEnv,
    _: JClass,
) -> jlong {
    create()
}

#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_FileTransferBridge_destroy(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
) {
    destroy(handle)
}

#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_FileTransferBridge_queueUpload(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    local_name: JString,
    size: jlong,
) -> jlong {
    match (
        native_api::string(&mut env, &local_name),
        u64::try_from(size),
    ) {
        (Some(local_name), Ok(size)) => queue_upload(handle, &local_name, size),
        _ => -1,
    }
}

#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_FileTransferBridge_queueDownload(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    name: JString,
    local_name: JString,
    size: jlong,
) -> jlong {
    let name = native_api::string(&mut env, &name);
    let local_name = native_api::string(&mut env, &local_name);
    match (name, local_name) {
        (Some(name), Some(local_name)) => {
            queue_download(handle, &name, &local_name, u64::try_from(size).unwrap_or(0))
        }
        _ => -1,
    }
}

#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_FileTransferBridge_cancel(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
    transfer: jlong,
) -> jboolean {
    flag(cancel(handle, id(transfer)))
}

#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_FileTransferBridge_cancelAll(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
) {
    cancel_all(handle)
}

#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_FileTransferBridge_retry(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
    transfer: jlong,
) -> jboolean {
    flag(retry(handle, id(transfer)))
}

#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_FileTransferBridge_clearFinished(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
) {
    clear_finished(handle)
}

#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_FileTransferBridge_snapshot(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
) -> jstring {
    let rows = snapshot(handle);
    reply(&mut env, rows)
}

#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_FileTransferBridge_startNext(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
) -> jstring {
    let started = start_next(handle);
    reply(&mut env, started)
}

#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Java_com_limelight_nvstream_jni_FileTransferBridge_list(
    mut env: JNIEnv,
    _: JClass,
    address: JString,
    http_port: jint,
    https_port: jint,
    server_certificate_pem: JString,
    unique_id: JString,
    client_certificate_der: JByteArray,
    client_private_key_pkcs8: JByteArray,
) -> jstring {
    let endpoint = native_api::endpoint(
        &mut env,
        &address,
        http_port,
        https_port,
        &server_certificate_pem,
        &unique_id,
        &client_certificate_der,
        &client_private_key_pkcs8,
    );
    let files = list(client(endpoint));
    reply(&mut env, files)
}

#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Java_com_limelight_nvstream_jni_FileTransferBridge_run(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    transfer: jlong,
    address: JString,
    http_port: jint,
    https_port: jint,
    server_certificate_pem: JString,
    unique_id: JString,
    client_certificate_der: JByteArray,
    client_private_key_pkcs8: JByteArray,
    fd: jint,
    listener: JObject,
) -> jstring {
    // Owned first, so the descriptor is closed whatever else fails.
    let file = owned_file(fd);
    let endpoint = native_api::endpoint(
        &mut env,
        &address,
        http_port,
        https_port,
        &server_certificate_pem,
        &unique_id,
        &client_certificate_der,
        &client_private_key_pkcs8,
    );
    let file = file.ok_or_else(|| std::io::Error::other("the file could not be opened"));
    let transfer = id(transfer);
    let answer = {
        let mut progress = |done: u64, total: u64| {
            if listener.is_null() {
                return;
            }
            let args = [
                JValue::Long(transfer as i64),
                JValue::Long(i64::try_from(done).unwrap_or(i64::MAX)),
                JValue::Long(i64::try_from(total).unwrap_or(i64::MAX)),
            ];
            // No local references are left behind: the frame frees the class lookup.
            let called = env.with_local_frame(4, |env| {
                env.call_method(&listener, "onProgress", "(JJJ)V", &args)
                    .map(|_| ())
            });
            if called.is_err() && env.exception_check().unwrap_or(false) {
                let _ = env.exception_describe();
                let _ = env.exception_clear();
            }
        };
        run(handle, transfer, client(endpoint), file, &mut progress)
    };
    reply(&mut env, answer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nightjar_core::http::{FileRefusal, RemoteFile};
    use std::io::{Seek, SeekFrom};

    /// Sends and receives in 4-byte steps, failing as told.
    #[derive(Default)]
    struct Fake {
        refuse: Option<(FileRefusal, u16)>,
        content: Vec<u8>,
        /// Cancels the transfer after this many bytes, as a user would.
        cancel_after: Option<u64>,
        received: Mutex<Vec<u8>>,
    }

    impl Fake {
        fn refusal(&self) -> Result<(), FileError> {
            match self.refuse {
                Some((refusal, status)) => Err(FileError::Refused {
                    refusal,
                    status,
                    message: "refused".into(),
                }),
                None => Ok(()),
            }
        }
    }

    impl FileHost for Fake {
        fn list(&self) -> Result<FileList, FileError> {
            self.refusal()?;
            Ok(FileList {
                folder: "C:\\Users\\me\\Downloads\\Rubylight".into(),
                free_bytes: 1000,
                max_file_bytes: 500,
                files: vec![RemoteFile {
                    name: "a.txt".into(),
                    size: 3,
                    modified_unix_ms: 1,
                }],
            })
        }
        fn upload(
            &self,
            name: &str,
            file: &mut dyn Read,
            length: u64,
            progress: &mut dyn FnMut(u64, u64),
            stop: &TransferCancel,
        ) -> Result<Uploaded, FileError> {
            self.refusal()?;
            let mut done = 0;
            progress(0, length);
            let mut chunk = [0u8; 4];
            while done < length {
                if stop.is_cancelled() || self.cancel_after.is_some_and(|n| done >= n) {
                    return Err(FileError::Host(Error::Cancelled));
                }
                let n = file.read(&mut chunk).map_err(FileError::Local)?;
                if n == 0 {
                    break;
                }
                self.received.lock().unwrap().extend_from_slice(&chunk[..n]);
                done += n as u64;
                progress(done, length);
            }
            Ok(Uploaded {
                name: format!("{name} (1)"),
                size: done,
            })
        }
        fn download(
            &self,
            _name: &str,
            sink: &mut dyn Write,
            progress: &mut dyn FnMut(u64, u64),
            stop: &TransferCancel,
        ) -> Result<u64, FileError> {
            self.refusal()?;
            let total = self.content.len() as u64;
            progress(0, total);
            let mut done = 0;
            for chunk in self.content.chunks(4) {
                if stop.is_cancelled() || self.cancel_after.is_some_and(|n| done >= n) {
                    return Err(FileError::Host(Error::Cancelled));
                }
                sink.write_all(chunk).map_err(FileError::Local)?;
                done += chunk.len() as u64;
                progress(done, total);
            }
            Ok(done)
        }
    }

    fn temp(content: &[u8]) -> File {
        let path = std::env::temp_dir().join(format!(
            "nightjar-files-{}-{}",
            std::process::id(),
            NEXT_HANDLE.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = File::options()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        file.write_all(content).unwrap();
        file.seek(SeekFrom::Start(0)).unwrap();
        // The open file outlives its name, so nothing is left in the temp folder.
        let _ = std::fs::remove_file(&path);
        file
    }

    fn parse(reply: &str) -> Value {
        serde_json::from_str(reply).unwrap()
    }

    fn started(handle: i64) -> Vec<u64> {
        parse(&start_next(handle))
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["id"].as_u64().unwrap())
            .collect()
    }

    #[test]
    fn an_upload_runs_reports_progress_and_lands_under_the_hosts_name() {
        let handle = create();
        let id = queue_upload(handle, "/storage/emulated/0/DCIM/photo?.jpg", 10) as u64;
        let rows = parse(&snapshot(handle));
        assert_eq!(rows[0]["state"], "queued");
        assert_eq!(rows[0]["name"], "photo_.jpg");
        assert_eq!(rows[0]["local_name"], "/storage/emulated/0/DCIM/photo?.jpg");

        let starts = parse(&start_next(handle));
        assert_eq!(starts[0]["direction"], "upload");
        assert_eq!(starts[0]["name"], "photo_.jpg");
        let host = Fake::default();
        let mut reports = Vec::new();
        let reply = parse(&run(
            handle,
            id,
            Ok(&host),
            Ok(temp(b"0123456789")),
            &mut |done, total| reports.push((done, total)),
        ));
        assert_eq!(reports, [(4, 10), (8, 10), (10, 10)]);
        assert_eq!(host.received.lock().unwrap().as_slice(), b"0123456789");
        let row = &reply["ok"];
        assert_eq!(row["state"], "done");
        assert_eq!(row["saved_name"], "photo_.jpg (1)");
        assert_eq!(
            (row["done"].as_u64(), row["total"].as_u64()),
            (Some(10), Some(10))
        );
        // A transfer runs once per start.
        let again = parse(&run(handle, id, Ok(&host), Ok(temp(b"")), &mut |_, _| {}));
        assert_eq!(again["error"]["problem"], "local");
        destroy(handle);
    }

    #[test]
    fn a_download_writes_the_file_and_learns_its_size() {
        let handle = create();
        let id = queue_download(handle, "notes.txt", "notes.txt", 0) as u64;
        assert_eq!(started(handle), [id]);
        let host = Fake {
            content: b"hello world".to_vec(),
            ..Fake::default()
        };
        let mut file = temp(b"");
        let reply = parse(&run(
            handle,
            id,
            Ok(&host),
            file.try_clone(),
            &mut |_, _| {},
        ));
        assert_eq!(reply["ok"]["state"], "done");
        assert_eq!(reply["ok"]["size"], 11);
        let mut written = String::new();
        file.seek(SeekFrom::Start(0)).unwrap();
        file.read_to_string(&mut written).unwrap();
        assert_eq!(written, "hello world");
        destroy(handle);
    }

    #[test]
    fn refusals_become_problems_and_only_some_can_be_retried() {
        for (refusal, status, problem, console, retry) in [
            (FileRefusal::Forbidden, 403, "no_permission", true, true),
            (
                FileRefusal::FileTransferDisabled,
                403,
                "transfer_off",
                true,
                true,
            ),
            (FileRefusal::TooLarge, 413, "too_large", false, false),
            (FileRefusal::TransferBusy, 409, "busy", false, true),
            (FileRefusal::NotFound, 404, "not_found", false, false),
        ] {
            let handle = create();
            let id = queue_download(handle, "a", "a", 0) as u64;
            started(handle);
            let host = Fake {
                refuse: Some((refusal, status)),
                ..Fake::default()
            };
            let row = &parse(&run(handle, id, Ok(&host), Ok(temp(b"")), &mut |_, _| {}))["ok"];
            assert_eq!(row["state"], "failed", "{refusal:?}");
            assert_eq!(row["problem"], problem, "{refusal:?}");
            assert_eq!(row["console_fix"], console, "{refusal:?}");
            assert_eq!(row["can_retry"], retry, "{refusal:?}");
            assert_eq!(retry, super::retry(handle, id), "{refusal:?}");
            destroy(handle);
        }
    }

    #[test]
    fn no_client_fails_the_transfer_instead_of_dropping_it() {
        let handle = create();
        let id = queue_upload(handle, "a.bin", 4) as u64;
        started(handle);
        let reply = parse(&run::<&Fake>(
            handle,
            id,
            Err(Error::InvalidIdentity("bad".into())),
            Ok(temp(b"abcd")),
            &mut |_, _| {},
        ));
        assert_eq!(reply["ok"]["state"], "failed");
        assert_eq!(reply["ok"]["problem"], "host");
        destroy(handle);
    }

    #[test]
    fn a_file_that_would_not_open_fails_the_transfer() {
        let handle = create();
        let id = queue_download(handle, "a.bin", "a.bin", 0) as u64;
        started(handle);
        let reply = parse(&run(
            handle,
            id,
            Ok(&Fake::default()),
            Err(std::io::Error::other("gone")),
            &mut |_, _| {},
        ));
        assert_eq!(reply["ok"]["state"], "failed");
        assert_eq!(reply["ok"]["problem"], "local");
        assert_eq!(reply["ok"]["can_retry"], true);
        destroy(handle);
    }

    #[test]
    fn a_cancel_while_running_ends_cancelled_and_can_be_retried() {
        let handle = create();
        let id = queue_upload(handle, "big.bin", 12) as u64;
        started(handle);
        assert!(cancel(handle, id));
        let rows = parse(&snapshot(handle));
        assert_eq!(rows[0]["cancelling"], true);
        let host = Fake::default();
        let row = &parse(&run(
            handle,
            id,
            Ok(&host),
            Ok(temp(&[0; 12])),
            &mut |_, _| {},
        ))["ok"];
        assert_eq!(row["state"], "cancelled");
        assert_eq!(row["cancelling"], false);
        assert!(host.received.lock().unwrap().is_empty());
        assert!(retry(handle, id));
        assert_eq!(started(handle), [id]);
        let row = &parse(&run(
            handle,
            id,
            Ok(&host),
            Ok(temp(&[7; 12])),
            &mut |_, _| {},
        ))["ok"];
        assert_eq!(row["state"], "done");
        destroy(handle);
    }

    #[test]
    fn one_upload_and_two_downloads_run_at_a_time() {
        let handle = create();
        let up = [queue_upload(handle, "1", 1), queue_upload(handle, "2", 1)];
        let down = [
            queue_download(handle, "a", "a", 1),
            queue_download(handle, "b", "b", 1),
            queue_download(handle, "c", "c", 1),
        ];
        let first: Vec<i64> = started(handle).into_iter().map(|i| i as i64).collect();
        assert_eq!(first, [up[0], down[0], down[1]]);
        assert!(started(handle).is_empty());
        let host = Fake {
            content: vec![1],
            ..Fake::default()
        };
        run(
            handle,
            down[0] as u64,
            Ok(&host),
            Ok(temp(b"")),
            &mut |_, _| {},
        );
        assert_eq!(started(handle), [down[2] as u64]);
        destroy(handle);
    }

    #[test]
    fn a_destroyed_handle_answers_nothing_and_cancels_its_transfers() {
        let handle = create();
        let id = queue_download(handle, "a", "a", 0) as u64;
        started(handle);
        let kept = entry(handle).unwrap();
        destroy(handle);
        assert!(lock(&kept).transfers.get(id).unwrap().is_cancelling());
        assert_eq!(snapshot(handle), "[]");
        assert_eq!(start_next(handle), "[]");
        assert_eq!(queue_upload(handle, "a", 1), -1);
        assert!(!cancel(handle, id));
        let reply = parse(&run(
            handle,
            id,
            Ok(&Fake::default()),
            Ok(temp(b"")),
            &mut |_, _| {},
        ));
        assert_eq!(reply["error"]["problem"], "local");
    }

    #[test]
    fn a_handle_destroyed_while_its_transfer_runs_cancels_it_and_the_run_still_answers() {
        // The screen closes (main thread) while a worker is between two chunks.
        let handle = create();
        let id = queue_upload(handle, "big.bin", 12) as u64;
        started(handle);
        let host = Fake::default();
        let mut destroyed = false;
        let reply = parse(&run(
            handle,
            id,
            Ok(&host),
            Ok(temp(&[0; 12])),
            &mut |done, _| {
                if done > 0 && !destroyed {
                    destroyed = true;
                    destroy(handle);
                }
            },
        ));
        assert!(destroyed);
        assert_eq!(reply["ok"]["state"], "cancelled");
        assert_eq!(
            host.received.lock().unwrap().len(),
            4,
            "no chunk after the close"
        );
        assert_eq!(snapshot(handle), "[]");
    }

    #[test]
    fn the_list_and_its_errors_keep_the_status_and_code() {
        let reply = parse(&list(Ok(&Fake::default())));
        assert_eq!(reply["ok"]["files"][0]["name"], "a.txt");
        assert_eq!(reply["ok"]["max_file_bytes"], 500);
        let refused = parse(&list(Ok(&Fake {
            refuse: Some((FileRefusal::Forbidden, 403)),
            ..Fake::default()
        })));
        let error = &refused["error"];
        assert_eq!(error["problem"], "no_permission");
        assert_eq!(
            (error["http_status"].as_u64(), error["code"].as_str()),
            (Some(403), Some("forbidden"))
        );
        let offline = parse(&list::<&Fake>(Err(Error::Io(std::io::Error::other(
            "down",
        )))));
        assert_eq!(offline["error"]["problem"], "network");
        assert!(offline["error"].get("http_status").is_none());
        let cancelled = parse(&file_error_envelope(&FileError::Host(Error::Cancelled)));
        assert_eq!(cancelled["error"]["problem"], "cancelled");
    }

    impl<T: FileHost> FileHost for &T {
        fn list(&self) -> Result<FileList, FileError> {
            (*self).list()
        }
        fn upload(
            &self,
            name: &str,
            file: &mut dyn Read,
            length: u64,
            progress: &mut dyn FnMut(u64, u64),
            stop: &TransferCancel,
        ) -> Result<Uploaded, FileError> {
            (*self).upload(name, file, length, progress, stop)
        }
        fn download(
            &self,
            name: &str,
            sink: &mut dyn Write,
            progress: &mut dyn FnMut(u64, u64),
            stop: &TransferCancel,
        ) -> Result<u64, FileError> {
            (*self).download(name, sink, progress, stop)
        }
    }
}
