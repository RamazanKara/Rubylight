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
        listed: u64,
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
        listed: u64,
        sink: &mut dyn Write,
        progress: &mut dyn FnMut(u64, u64),
        stop: &TransferCancel,
    ) -> Result<u64, FileError> {
        self.download_file(name, listed, sink, progress, stop)
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
                .download(&start.name, start.size, &mut file, &mut report, &start.cancel)
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
mod tests;
