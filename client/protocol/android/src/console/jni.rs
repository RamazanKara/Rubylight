//! `NightjarConsoleJni` `#[unsafe(no_mangle)]` exports and `Slab`-backed `ConsoleRepository`
//! management, including `vm.attach_current_thread()` worker callbacks for fake/demo transports,
//! legacy `SharedPreferences` import, clean test reset hooks, and pure JSON/policy helpers.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use jni::objects::{GlobalRef, JByteArray, JClass, JObject, JString, JValue};
use jni::sys::{JNI_FALSE, jboolean, jint, jlong, jstring};
use jni::{JNIEnv, JavaVM};
use nightjar_core::console::{
    has_any_library, has_classic_app_list, ConsoleRepository, ConsoleSnapshot, Favourite,
    HostClientTransport, HostDescriptor, NativeTransport, Recent,
};
use nightjar_core::http::{AddedGame, ClientIdentity, NativeCapabilities, NativeLibrary};
use nightjar_policy::console::{
    detail, HiddenTracker, HomeFilterKey, HomeOrder, HomeSort, LibraryFilter, Sort, Source,
};
use nightjar_policy::http::native_api::session::Session;
use nightjar_policy::http::{guest_ended, parse_added, CoopProblem};
use serde_json::{json, Value};

use crate::http::{jbool, new_jstring, ok_json};
use crate::slab::{lock, Slab};

pub(super) static REPOS: Slab<ConsoleEntry> = Slab::new();
static TRACKERS: Mutex<BTreeMap<i64, Vec<Favourite>>> = Mutex::new(BTreeMap::new());

pub(super) struct ConsoleEntry {
    pub(super) repo: ConsoleRepository,
}

pub(super) fn jstr(env: &mut JNIEnv, value: &JString) -> Option<String> {
    if value.is_null() {
        return None;
    }
    env.get_string(value).ok().map(|s| s.to_string_lossy().into_owned())
}

fn jbytes(env: &mut JNIEnv, value: &JByteArray) -> Option<Vec<u8>> {
    if value.is_null() {
        return None;
    }
    env.convert_byte_array(value).ok()
}

fn err_envelope(msg: &str) -> String {
    json!({ "error": { "kind": "error", "message": msg } }).to_string()
}

/// Callback-backed [`NativeTransport`] that attaches each `std::thread::scope` worker to `JavaVM`.
struct JniNativeTransport {
    vm: JavaVM,
    callback: GlobalRef,
}

impl JniNativeTransport {
    fn invoke(&self, host_uuid: &str, op: &str, arg: &str) -> Result<Value, String> {
        let mut env = self
            .vm
            .attach_current_thread()
            .map_err(|e| format!("attach JNI thread: {e}"))?;
        let text = {
            let res: Result<Result<String, String>, jni::errors::Error> =
                env.with_local_frame(8, |env| {
                    let j_host = env.new_string(host_uuid)?;
                    let j_op = env.new_string(op)?;
                    let j_arg = env.new_string(arg)?;
                    let ret = env.call_method(
                        self.callback.as_obj(),
                        "call",
                        "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;",
                        &[
                            JValue::Object(&j_host),
                            JValue::Object(&j_op),
                            JValue::Object(&j_arg),
                        ],
                    )?;
                    let obj = ret.l()?;
                    if obj.is_null() {
                        return Ok(Err("callback returned null".into()));
                    }
                    let j_out = JString::from(obj);
                    let text = env.get_string(&j_out)?.to_string_lossy().into_owned();
                    Ok(Ok(text))
                });
            match res {
                Ok(inner) => inner?,
                Err(e) => {
                    if env.exception_check().unwrap_or(false) {
                        let _ = env.exception_clear();
                    }
                    return Err(format!("callback call failed: {e}"));
                }
            }
        };
        let envelope: Value =
            serde_json::from_str(&text).map_err(|e| format!("malformed callback JSON: {e}"))?;
        if let Some(err) = envelope.get("error") {
            let kind = err
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or("unavailable");
            let msg = err
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("request failed");
            return Err(format!("__nj_kind:{kind}__{msg}"));
        }
        envelope
            .get("ok")
            .cloned()
            .ok_or_else(|| "missing ok field in transport reply".to_owned())
    }
}

impl NativeTransport for JniNativeTransport {
    fn capabilities(&self, host: &HostDescriptor) -> Result<NativeCapabilities, String> {
        let ok = self.invoke(&host.uuid, "capabilities", "")?;
        NativeCapabilities::parse_lenient(&ok).map_err(|e| format!("malformed capabilities: {e}"))
    }

    fn library(
        &self,
        host: &HostDescriptor,
        include_unadded: bool,
    ) -> Result<NativeLibrary, String> {
        let arg = if include_unadded { "include=unadded" } else { "" };
        let ok = self.invoke(&host.uuid, "library", arg)?;
        serde_json::from_value::<NativeLibrary>(ok)
            .map_err(|e| format!("malformed library: {e}"))
    }

    fn session(&self, host: &HostDescriptor) -> Result<Session, String> {
        let ok = self.invoke(&host.uuid, "session", "")?;
        serde_json::from_value::<Session>(ok).map_err(|e| format!("malformed session: {e}"))
    }

    fn add_to_library(
        &self,
        host: &HostDescriptor,
        game_id: &str,
    ) -> Result<Option<AddedGame>, String> {
        let arg = json!({ "game_id": game_id }).to_string();
        let ok = self.invoke(&host.uuid, "library_add", &arg)?;
        parse_added(&ok, game_id)
    }
}

fn build_transport(
    env: &mut JNIEnv,
    cert_der: &JByteArray,
    key_pkcs8: &JByteArray,
    unique_id: &JString,
    callback: &JObject,
) -> Result<Box<dyn NativeTransport>, String> {
    if !callback.is_null() {
        let vm = env.get_java_vm().map_err(|e| e.to_string())?;
        let global = env.new_global_ref(callback).map_err(|e| e.to_string())?;
        return Ok(Box::new(JniNativeTransport {
            vm,
            callback: global,
        }));
    }
    let cert = jbytes(env, cert_der).ok_or_else(|| "missing client certificate".to_owned())?;
    let key = jbytes(env, key_pkcs8).ok_or_else(|| "missing client key".to_owned())?;
    let uid = jstr(env, unique_id).ok_or_else(|| "missing client unique id".to_owned())?;
    let identity = ClientIdentity::from_der(uid, cert, key).map_err(|e| e.to_string())?;
    Ok(Box::new(HostClientTransport::new(Arc::new(identity))))
}

pub fn create_repo(base_dir: &str) -> jlong {
    let path = PathBuf::from(base_dir);
    let handle = REPOS.insert(ConsoleEntry {
        repo: ConsoleRepository::new(&path),
    });
    lock(&TRACKERS).insert(handle, Vec::new());
    handle
}

pub fn destroy_repo(handle: jlong) {
    REPOS.remove(handle);
    lock(&TRACKERS).remove(&handle);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarConsoleJni_repoCreate(
    mut env: JNIEnv,
    _: JClass,
    base_dir: JString,
) -> jlong {
    let Some(dir) = jstr(&mut env, &base_dir) else {
        return 0;
    };
    create_repo(&dir)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarConsoleJni_repoDestroy(
    _env: JNIEnv,
    _: JClass,
    handle: jlong,
) {
    destroy_repo(handle);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarConsoleJni_repoClearCache(
    _env: JNIEnv,
    _: JClass,
    handle: jlong,
) {
    if let Some(entry) = REPOS.get(handle) {
        entry.repo.clear_in_memory_cache();
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarConsoleJni_repoResetForTests(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    base_dir: JString,
) {
    if let Some(entry) = REPOS.get(handle) {
        entry.repo.clear_in_memory_cache();
    }
    destroy_repo(handle);
    if let Some(dir) = jstr(&mut env, &base_dir) {
        let p = Path::new(&dir);
        let _ = std::fs::remove_dir_all(p.join("console"));
        let _ = std::fs::remove_dir_all(p.join("console-library"));
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarConsoleJni_repoImportLegacy(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    recents_json: JString,
    favourites_json: JString,
    hidden_entries_json: JString,
) -> jstring {
    let Some(entry) = REPOS.get(handle) else {
        return new_jstring(&env, r#"{"recents":false,"favourites":false,"hidden":false}"#);
    };
    let recents = jstr(&mut env, &recents_json)
        .and_then(|s| entry.repo.recents_store().import_legacy_json(&s).ok())
        .unwrap_or(false);
    let favourites = jstr(&mut env, &favourites_json)
        .and_then(|s| entry.repo.favourites_store().import_legacy_json(&s).ok())
        .unwrap_or(false);
    let hidden = jstr(&mut env, &hidden_entries_json)
        .and_then(|s| serde_json::from_str::<Vec<Favourite>>(&s).ok())
        .and_then(|list| entry.repo.hidden_store().import_legacy_entries(list).ok())
        .unwrap_or(false);
    new_jstring(
        &env,
        &json!({ "recents": recents, "favourites": favourites, "hidden": hidden }).to_string(),
    )
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarConsoleJni_repoCached(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    hosts_json: JString,
) -> jstring {
    let Some(entry) = REPOS.get(handle) else {
        return new_jstring(&env, &serde_json::to_string(&ConsoleSnapshot::default()).unwrap());
    };
    let hosts: Vec<HostDescriptor> = jstr(&mut env, &hosts_json)
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    let snap = entry.repo.cached(&hosts);
    new_jstring(&env, &serde_json::to_string(&snap).unwrap_or_else(|_| "{}".into()))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarConsoleJni_repoRefresh(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    hosts_json: JString,
    cert_der: JByteArray,
    key_pkcs8: JByteArray,
    unique_id: JString,
    now_ms: jlong,
    callback: JObject,
) -> jstring {
    let Some(entry) = REPOS.get(handle) else {
        return new_jstring(&env, &serde_json::to_string(&ConsoleSnapshot::default()).unwrap());
    };
    let hosts: Vec<HostDescriptor> = jstr(&mut env, &hosts_json)
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    let transport = match build_transport(&mut env, &cert_der, &key_pkcs8, &unique_id, &callback) {
        Ok(t) => t,
        Err(msg) => return new_jstring(&env, &err_envelope(&msg)),
    };
    let snap = if now_ms > 0 {
        entry.repo.refresh_at(&hosts, transport.as_ref(), now_ms as u64)
    } else {
        entry.repo.refresh(&hosts, transport.as_ref())
    };
    new_jstring(&env, &serde_json::to_string(&snap).unwrap_or_else(|_| "{}".into()))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarConsoleJni_repoAddToLibrary(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    host_json: JString,
    game_id: JString,
    cert_der: JByteArray,
    key_pkcs8: JByteArray,
    unique_id: JString,
    callback: JObject,
) -> jstring {
    let Some(entry) = REPOS.get(handle) else {
        return new_jstring(&env, &err_envelope("repository handle missing"));
    };
    let Some(host) = jstr(&mut env, &host_json).and_then(|s| serde_json::from_str::<HostDescriptor>(&s).ok()) else {
        return new_jstring(&env, &err_envelope("invalid host descriptor"));
    };
    let Some(gid) = jstr(&mut env, &game_id) else {
        return new_jstring(&env, &err_envelope("missing game_id"));
    };
    let transport = match build_transport(&mut env, &cert_der, &key_pkcs8, &unique_id, &callback) {
        Ok(t) => t,
        Err(msg) => return new_jstring(&env, &err_envelope(&msg)),
    };
    let out = match entry.repo.add_to_library(&host, &gid, transport.as_ref()) {
        Ok(Some(added)) => ok_json(crate::native_api::added_game_json(&added)),
        Ok(None) => ok_json(Value::Null),
        Err(msg) => err_envelope(&msg),
    };
    new_jstring(&env, &out)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarConsoleJni_repoToggleFavourite(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    entry_json: JString,
) -> jboolean {
    let Some(entry) = REPOS.get(handle) else {
        return JNI_FALSE;
    };
    let Some(fav) = jstr(&mut env, &entry_json).and_then(|s| serde_json::from_str::<Favourite>(&s).ok()) else {
        return JNI_FALSE;
    };
    jbool(entry.repo.set_favourite(fav).unwrap_or(false))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarConsoleJni_repoSetHidden(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    keys_json: JString,
    hidden: jboolean,
) -> jboolean {
    let Some(entry) = REPOS.get(handle) else {
        return JNI_FALSE;
    };
    let Some(keys) = jstr(&mut env, &keys_json).and_then(|s| serde_json::from_str::<Vec<Favourite>>(&s).ok()) else {
        return JNI_FALSE;
    };
    jbool(entry.repo.set_hidden(keys, hidden != JNI_FALSE).unwrap_or(false))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarConsoleJni_repoHideGame(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    hosts_json: JString,
    host_id: JString,
    app_id: jint,
    hidden: jboolean,
) -> jboolean {
    let Some(entry) = REPOS.get(handle) else {
        return JNI_FALSE;
    };
    let hosts: Vec<HostDescriptor> = jstr(&mut env, &hosts_json)
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    let Some(hid) = jstr(&mut env, &host_id) else {
        return JNI_FALSE;
    };
    let app_u32 = u32::try_from(app_id).unwrap_or(0);
    jbool(entry.repo.hide_game(&hosts, &hid, app_u32, hidden != JNI_FALSE).unwrap_or(false))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarConsoleJni_repoRecordLaunch(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    recent_json: JString,
) {
    if let Some(entry) = REPOS.get(handle)
        && let Some(raw) = jstr(&mut env, &recent_json)
        && let Ok(val) = serde_json::from_str::<Value>(&raw)
        && let Ok(recent) = serde_json::from_value::<Recent>(val.clone())
    {
        let host_name = val.get("host_name").and_then(Value::as_str).unwrap_or("");
        let _ = entry.repo.recents_store().record_with_host_name(recent, host_name);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarConsoleJni_repoForgetHost(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    host_id: JString,
) {
    if let Some(entry) = REPOS.get(handle)
        && let Some(hid) = jstr(&mut env, &host_id)
    {
        entry.repo.forget_host(&hid);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarConsoleJni_repoHiddenListings(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    hosts_json: JString,
) -> jstring {
    let Some(entry) = REPOS.get(handle) else {
        return new_jstring(&env, "[]");
    };
    let hosts: Vec<HostDescriptor> = jstr(&mut env, &hosts_json)
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    let list = entry.repo.hidden_listings(&hosts);
    new_jstring(&env, &serde_json::to_string(&list).unwrap_or_else(|_| "[]".into()))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarConsoleJni_repoIsHidden(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    host_id: JString,
    install_id: JString,
    app_id: jint,
) -> jboolean {
    let Some(entry) = REPOS.get(handle) else {
        return JNI_FALSE;
    };
    let hid = jstr(&mut env, &host_id).unwrap_or_default();
    let iid = jstr(&mut env, &install_id).unwrap_or_default();
    let app = u32::try_from(app_id).unwrap_or(0);
    jbool(entry.repo.hidden_store().is_hidden(&hid, &iid, app))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarConsoleJni_repoHiddenSnapshotChanged(
    _env: JNIEnv,
    _: JClass,
    handle: jlong,
    record_only: jboolean,
) -> jboolean {
    let Some(entry) = REPOS.get(handle) else {
        return JNI_FALSE;
    };
    let current = entry.repo.hidden_store().list();
    let mut map = lock(&TRACKERS);
    let prev = map.entry(handle).or_default();
    if record_only != JNI_FALSE {
        *prev = current;
        return JNI_FALSE;
    }
    let mut tracker = HiddenTracker::following(prev.clone());
    let changed = tracker.changed(&current);
    *prev = current;
    jbool(changed)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarConsoleJni_repoHasAnyLibrary(
    mut env: JNIEnv,
    _: JClass,
    base_dir: JString,
) -> jboolean {
    let Some(dir) = jstr(&mut env, &base_dir) else {
        return JNI_FALSE;
    };
    jbool(has_any_library(Path::new(&dir)))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarConsoleJni_repoHasClassicAppList(
    mut env: JNIEnv,
    _: JClass,
    app_list_dir: JString,
    host_id: JString,
) -> jboolean {
    let Some(dir) = jstr(&mut env, &app_list_dir) else {
        return JNI_FALSE;
    };
    let Some(hid) = jstr(&mut env, &host_id) else {
        return JNI_FALSE;
    };
    jbool(has_classic_app_list(Path::new(&dir), &hid))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarConsoleJni_repoProjectHome(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    hosts_json: JString,
    filter_json: JString,
    query: JString,
    sort_by: JString,
    sort_order: JString,
) -> jstring {
    let Some(entry) = REPOS.get(handle) else {
        return new_jstring(&env, "{}");
    };
    let hosts: Vec<HostDescriptor> = jstr(&mut env, &hosts_json)
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    let filter = jstr(&mut env, &filter_json)
        .and_then(|s| serde_json::from_str::<HomeFilterKey>(&s).ok())
        .map_or(LibraryFilter::All, |k| k.to_filter());
    let q = jstr(&mut env, &query).unwrap_or_default();
    let sort = Sort {
        by: jstr(&mut env, &sort_by).map_or(HomeSort::Title, |s| HomeSort::from_wire(&s)),
        order: jstr(&mut env, &sort_order).map_or(HomeOrder::Ascending, |s| HomeOrder::from_wire(&s)),
    };
    let proj = entry.repo.project_home(&hosts, &filter, &q, sort);
    new_jstring(&env, &serde_json::to_string(&proj).unwrap_or_else(|_| "{}".into()))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarConsoleJni_repoGameDetail(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    hosts_json: JString,
    host_id: JString,
    app_id: jint,
    native_api: jboolean,
) -> jstring {
    let Some(entry) = REPOS.get(handle) else {
        return new_jstring(&env, r#"{"state":"idle","can_start":false,"installed":true,"end":"none","other_running":null,"held_by_other":false,"resumes":false}"#);
    };
    let hosts: Vec<HostDescriptor> = jstr(&mut env, &hosts_json)
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    let hid = jstr(&mut env, &host_id).unwrap_or_default();
    let snap = entry.repo.cached(&hosts);
    let host_desc = hosts.iter().find(|h| h.uuid == hid);
    let lib = entry.repo.library_store().load(&hid).map(|nl| nl.library).or_else(|| {
        host_desc.and_then(|h| {
            h.classic_apps.as_ref().map(|ca| {
                let running = h.running_app_id.map(|id| (id, h.running_app_uuid.as_deref()));
                nightjar_core::console::classic_library(ca, running)
            })
        })
    });
    let sess = snap.sessions.get(&hid);
    let src = Source {
        id: 1,
        unique_id: &hid,
        name: host_desc.map_or("", |h| h.name.as_str()),
        online: host_desc.is_some_and(|h| h.online),
        paired: host_desc.is_some_and(|h| h.paired),
        library: lib.as_ref(),
        classic: host_desc.is_some_and(|h| !h.is_native),
        last_played: None,
        playtime_minutes: None,
        collections: None,
        session: sess,
        host_control: entry.repo.host_control_for(&hid),
    };
    let d = detail(&src, u32::try_from(app_id).unwrap_or(0), native_api != JNI_FALSE);
    let out = json!({
        "state": match d.state {
            nightjar_policy::console::GameState::Idle => "idle",
            nightjar_policy::console::GameState::Running => "running",
            nightjar_policy::console::GameState::Suspended => "suspended",
        },
        "can_start": d.can_start,
        "installed": d.installed,
        "end": match d.end {
            nightjar_policy::console::EndAction::None => "none",
            nightjar_policy::console::EndAction::CloseGame => "close_game",
            nightjar_policy::console::EndAction::Quit => "quit",
        },
        "other_running": d.other_running,
        "held_by_other": d.held_by_other,
        "resumes": d.resumes(),
    });
    new_jstring(&env, &out.to_string())
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarConsoleJni_parseAdded(
    mut env: JNIEnv,
    _: JClass,
    raw_json: JString,
    game_id: JString,
) -> jstring {
    let Some(raw) = jstr(&mut env, &raw_json) else {
        return new_jstring(&env, &err_envelope("missing reply JSON"));
    };
    let gid = jstr(&mut env, &game_id).unwrap_or_default();
    let Ok(val) = serde_json::from_str::<Value>(&raw) else {
        return new_jstring(&env, &err_envelope("malformed JSON"));
    };
    let out = match parse_added(&val, &gid) {
        Ok(Some(added)) => ok_json(crate::native_api::added_game_json(&added)),
        Ok(None) => ok_json(Value::Null),
        Err(msg) => err_envelope(&msg),
    };
    new_jstring(&env, &out)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarConsoleJni_parseCapabilitiesLenient(
    mut env: JNIEnv,
    _: JClass,
    raw_json: JString,
) -> jstring {
    let Some(raw) = jstr(&mut env, &raw_json) else {
        return new_jstring(&env, &err_envelope("missing capabilities JSON"));
    };
    let Ok(val) = serde_json::from_str::<Value>(&raw) else {
        return new_jstring(&env, &err_envelope("malformed capabilities JSON"));
    };
    let out = match NativeCapabilities::parse_lenient(&val) {
        Ok(caps) => ok_json(json!({
            "api_version": caps.capabilities.api_version,
            "features": caps.capabilities.features,
            "permissions": {
                "list": caps.capabilities.permissions.list,
                "view": caps.capabilities.permissions.view,
                "launch": caps.capabilities.permissions.launch,
                "host_control": caps.capabilities.permissions.host_control,
                "file_upload": caps.files.file_upload,
                "file_download": caps.files.file_download,
                "usb": caps.files.usb,
                "shared_folders": caps.files.shared_folders,
                "restart_shutdown": caps.files.restart_shutdown,
                "pairing_approve": caps.files.pairing_approve,
            },
            "offers_continue_here": caps.offers_continue_here(),
            "offers_usb_forwarding": caps.offers_usb_forwarding(),
            "may_approve_pairing": caps.may_approve_pairing(),
            "may_invite": caps.may_invite(),
        })),
        Err(e) => err_envelope(&format!("malformed capabilities: {e}")),
    };
    new_jstring(&env, &out)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarConsoleJni_coopProblemKind(
    mut env: JNIEnv,
    _: JClass,
    status: jint,
    code: JString,
    message: JString,
) -> jstring {
    let code_str = jstr(&mut env, &code).unwrap_or_default();
    let msg = jstr(&mut env, &message).unwrap_or_default();
    let parsed_code = serde_json::from_value(Value::String(code_str))
        .unwrap_or(nightjar_core::http::native_api::error::ErrorCode::Unknown);
    let err = nightjar_core::http::Error::Api {
        status: u16::try_from(status).unwrap_or(500),
        error: Box::new(nightjar_core::http::native_api::error::ApiError::new(parsed_code, msg)),
    };
    new_jstring(&env, CoopProblem::of(&err).kind())
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarConsoleJni_guestsEnded(
    mut env: JNIEnv,
    _: JClass,
    reason: JString,
) -> jboolean {
    let Some(r) = jstr(&mut env, &reason) else {
        return JNI_FALSE;
    };
    jbool(guest_ended(&r))
}
