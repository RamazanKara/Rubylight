//! Android JNI + NativeApiClient operations for Host Hub Sync (`hub.snapshot`,
//! `hub.profile.put`, `hub.profile.delete`, `hub.cover.to_file`) and pure JSON policy
//! entrypoints (`merge_three_way`, `covers_to_fetch`, `compact_outbox`) via `nightjar_policy::hub_sync`.

use jni::JNIEnv;
use jni::objects::{JClass, JString};
use jni::sys::jstring;
use nightjar_core::http::native_api::hub::{CoverEntry, ProfileWrite};
use nightjar_core::http::{
    Error, HostClient, HubCoverResult, HubDeleteResult, HubPutResult, HubSnapshotResult,
};
use nightjar_policy::hub_sync::{
    MergeOutcome, OutboxOp, compact_outbox, covers_to_fetch, merge_three_way,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;

/// Subset of `HostClient` used by Hub Sync operations so tests can supply a fake host.
pub trait HubHost {
    fn hub_snapshot(
        &self,
        if_none_match: Option<&str>,
        all_devices: bool,
    ) -> Result<HubSnapshotResult, Error>;
    fn put_hub_profile(
        &self,
        id: &str,
        if_match: &str,
        write: &ProfileWrite,
    ) -> Result<HubPutResult, Error>;
    fn delete_hub_profile(&self, id: &str, if_match: &str) -> Result<HubDeleteResult, Error>;
    fn hub_cover(&self, app_id: u32, if_none_match: Option<&str>) -> Result<HubCoverResult, Error>;
}

impl HubHost for HostClient {
    fn hub_snapshot(
        &self,
        if_none_match: Option<&str>,
        all_devices: bool,
    ) -> Result<HubSnapshotResult, Error> {
        HostClient::hub_snapshot(self, if_none_match, all_devices)
    }

    fn put_hub_profile(
        &self,
        id: &str,
        if_match: &str,
        write: &ProfileWrite,
    ) -> Result<HubPutResult, Error> {
        HostClient::put_hub_profile(self, id, if_match, write)
    }

    fn delete_hub_profile(&self, id: &str, if_match: &str) -> Result<HubDeleteResult, Error> {
        HostClient::delete_hub_profile(self, id, if_match)
    }

    fn hub_cover(&self, app_id: u32, if_none_match: Option<&str>) -> Result<HubCoverResult, Error> {
        HostClient::hub_cover(self, app_id, if_none_match)
    }
}

#[derive(Deserialize)]
struct SnapshotArgs {
    #[serde(default)]
    if_none_match: Option<String>,
    #[serde(default)]
    all_devices: bool,
}

#[derive(Deserialize)]
struct PutArgs {
    id: String,
    if_match: String,
    write: ProfileWrite,
}

#[derive(Deserialize)]
struct DeleteArgs {
    id: String,
    if_match: String,
}

#[derive(Deserialize)]
struct CoverFileArgs {
    app_id: u32,
    #[serde(default)]
    if_none_match: Option<String>,
    dest_path: String,
}

#[derive(Deserialize)]
struct MergeArgs {
    base: Option<nightjar_core::http::native_api::hub::SyncProfile>,
    local: ProfileWrite,
    remote: nightjar_core::http::native_api::hub::SyncProfile,
}

#[derive(Deserialize)]
struct CoversDiffArgs {
    remote: Vec<CoverEntry>,
    local_etags: BTreeMap<u32, String>,
}

#[derive(Deserialize)]
struct CompactArgs {
    ops: Vec<OutboxOp>,
}

fn ok_val(val: Value) -> String {
    json!({ "ok": val }).to_string()
}

fn err_arg(msg: impl Into<String>) -> String {
    crate::native_api::error_envelope(&Error::InvalidArgument(msg.into()))
}

fn write_atomic_bytes(dest: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file_name = dest
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("cover.img");
    let tmp = dest.with_file_name(format!("{file_name}.tmp"));
    std::fs::write(&tmp, bytes)?;
    if std::fs::rename(&tmp, dest).is_err() {
        let _ = std::fs::remove_file(dest);
        let res = std::fs::rename(&tmp, dest);
        if res.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        return res;
    }
    Ok(())
}

/// Executes one network-backed Hub Sync native API operation (`hub.*`), or returns `None`.
pub fn execute(host: &dyn HubHost, op: &str, argument: &str) -> Option<String> {
    Some(match op {
        "hub.snapshot" => {
            let args: SnapshotArgs = if argument.trim().is_empty() {
                SnapshotArgs {
                    if_none_match: None,
                    all_devices: false,
                }
            } else {
                match serde_json::from_str(argument) {
                    Ok(v) => v,
                    Err(e) => return Some(err_arg(format!("hub.snapshot: {e}"))),
                }
            };
            match host.hub_snapshot(args.if_none_match.as_deref(), args.all_devices) {
                Ok(HubSnapshotResult::NotModified) => ok_val(json!({ "status": "not_modified" })),
                Ok(HubSnapshotResult::Modified { snapshot, etag }) => ok_val(json!({
                    "status": "ok",
                    "etag": etag,
                    "snapshot": snapshot,
                })),
                Err(e) => crate::native_api::error_envelope(&e),
            }
        }
        "hub.profile.put" => {
            let args: PutArgs = match serde_json::from_str(argument) {
                Ok(v) => v,
                Err(e) => return Some(err_arg(format!("hub.profile.put: {e}"))),
            };
            match host.put_hub_profile(&args.id, &args.if_match, &args.write) {
                Ok(HubPutResult::Saved { profile, etag }) => ok_val(json!({
                    "status": "ok",
                    "etag": etag,
                    "profile": profile,
                })),
                Ok(HubPutResult::Conflict {
                    error,
                    current_revision,
                }) => ok_val(json!({
                    "status": "conflict",
                    "current_revision": current_revision,
                    "error": error,
                })),
                Err(e) => crate::native_api::error_envelope(&e),
            }
        }
        "hub.profile.delete" => {
            let args: DeleteArgs = match serde_json::from_str(argument) {
                Ok(v) => v,
                Err(e) => return Some(err_arg(format!("hub.profile.delete: {e}"))),
            };
            match host.delete_hub_profile(&args.id, &args.if_match) {
                Ok(HubDeleteResult::Deleted(res)) => ok_val(json!({
                    "status": "ok",
                    "deleted": res.deleted,
                    "snapshot_revision": res.snapshot_revision,
                })),
                Ok(HubDeleteResult::Conflict {
                    error,
                    current_revision,
                }) => ok_val(json!({
                    "status": "conflict",
                    "current_revision": current_revision,
                    "error": error,
                })),
                Err(e) => crate::native_api::error_envelope(&e),
            }
        }
        "hub.cover.to_file" => {
            let args: CoverFileArgs = match serde_json::from_str(argument) {
                Ok(v) => v,
                Err(e) => return Some(err_arg(format!("hub.cover.to_file: {e}"))),
            };
            if args.dest_path.trim().is_empty() {
                return Some(err_arg("hub.cover.to_file: missing dest_path"));
            }
            match host.hub_cover(args.app_id, args.if_none_match.as_deref()) {
                Ok(HubCoverResult::NotModified) => ok_val(json!({ "status": "not_modified" })),
                Ok(HubCoverResult::Image {
                    etag,
                    content_type,
                    bytes,
                }) => {
                    if let Err(e) = write_atomic_bytes(Path::new(&args.dest_path), &bytes) {
                        return Some(err_arg(format!("hub.cover.to_file write failed: {e}")));
                    }
                    ok_val(json!({
                        "status": "ok",
                        "etag": etag,
                        "mime": content_type,
                        "bytes": bytes.len(),
                    }))
                }
                Err(e) => crate::native_api::error_envelope(&e),
            }
        }
        _ => return None,
    })
}

/// Executes pure policy computations (`merge_three_way`, `covers_to_fetch`, `compact_outbox`).
pub fn policy(op: &str, argument: &str) -> String {
    match op {
        "merge_three_way" => {
            let args: MergeArgs = match serde_json::from_str(argument) {
                Ok(v) => v,
                Err(e) => return err_arg(format!("merge_three_way: {e}")),
            };
            match merge_three_way(args.base.as_ref(), &args.local, &args.remote) {
                MergeOutcome::Merged(write) => ok_val(json!({
                    "status": "merged",
                    "write": write,
                })),
                MergeOutcome::Conflict(conflict) => ok_val(json!({
                    "status": "conflict",
                    "conflict": conflict,
                })),
            }
        }
        "covers_to_fetch" => {
            let args: CoversDiffArgs = match serde_json::from_str(argument) {
                Ok(v) => v,
                Err(e) => return err_arg(format!("covers_to_fetch: {e}")),
            };
            let missing: Vec<u32> = covers_to_fetch(&args.remote, &args.local_etags)
                .into_iter()
                .map(|c| c.app_id)
                .collect();
            ok_val(json!({ "app_ids": missing }))
        }
        "compact_outbox" => {
            let args: CompactArgs = match serde_json::from_str(argument) {
                Ok(v) => v,
                Err(e) => return err_arg(format!("compact_outbox: {e}")),
            };
            let ops = compact_outbox(args.ops);
            ok_val(json!({ "ops": ops }))
        }
        other => err_arg(format!("unknown hub sync policy op: {other:?}")),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_hubSyncPolicy(
    mut env: JNIEnv,
    _: JClass,
    op: JString,
    argument: JString,
) -> jstring {
    let reply = match (
        crate::native_api::string(&mut env, &op),
        crate::native_api::string(&mut env, &argument),
    ) {
        (Some(op), Some(arg)) => policy(&op, &arg),
        (Some(op), None) => policy(&op, ""),
        _ => err_arg("hubSyncPolicy: missing op"),
    };
    match env.new_string(reply) {
        Ok(s) => s.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nightjar_core::http::native_api::error::{ApiError, ErrorCode};
    use nightjar_core::http::native_api::hub::{
        HubSnapshot, ProfileScope, StreamProfileSpec, SyncProfile,
    };

    struct FakeHost;

    impl HubHost for FakeHost {
        fn hub_snapshot(&self, inm: Option<&str>, _: bool) -> Result<HubSnapshotResult, Error> {
            if inm == Some("\"rev:7\"") {
                return Ok(HubSnapshotResult::NotModified);
            }
            Ok(HubSnapshotResult::Modified {
                snapshot: HubSnapshot {
                    schema_version: 1,
                    snapshot_revision: 7,
                    ..Default::default()
                },
                etag: Some("\"rev:7\"".into()),
            })
        }

        fn put_hub_profile(
            &self,
            id: &str,
            if_match: &str,
            write: &ProfileWrite,
        ) -> Result<HubPutResult, Error> {
            if if_match != "7" {
                return Ok(HubPutResult::Conflict {
                    error: Box::new(ApiError::new(ErrorCode::RevisionConflict, "Conflict")),
                    current_revision: Some(8),
                });
            }
            Ok(HubPutResult::Saved {
                profile: SyncProfile {
                    id: id.into(),
                    revision: 8,
                    scope: write.scope,
                    name: write.name.clone(),
                    stream: write.stream.clone(),
                    ..Default::default()
                },
                etag: Some("\"8\"".into()),
            })
        }

        fn delete_hub_profile(&self, _: &str, _: &str) -> Result<HubDeleteResult, Error> {
            Ok(HubDeleteResult::Deleted(
                nightjar_core::http::native_api::hub::ProfileDeleteResult {
                    deleted: true,
                    snapshot_revision: 9,
                },
            ))
        }

        fn hub_cover(&self, _: u32, inm: Option<&str>) -> Result<HubCoverResult, Error> {
            if inm == Some("sha256:abc") {
                return Ok(HubCoverResult::NotModified);
            }
            Ok(HubCoverResult::Image {
                etag: Some("sha256:abc".into()),
                content_type: Some("image/png".into()),
                bytes: b"pngbytes".to_vec(),
            })
        }
    }

    #[test]
    fn dispatches_hub_ops_and_pure_three_way_merge() {
        let host = FakeHost;
        let snap: Value =
            serde_json::from_str(&execute(&host, "hub.snapshot", "{}").unwrap()).unwrap();
        assert_eq!(snap["ok"]["snapshot"]["snapshot_revision"], 7);
        let nm: Value = serde_json::from_str(
            &execute(&host, "hub.snapshot", r#"{"if_none_match":"\"rev:7\""}"#).unwrap(),
        )
        .unwrap();
        assert_eq!(nm["ok"]["status"], "not_modified");

        let dir = std::env::temp_dir().join(format!("nightjar-android-hub-{}", std::process::id()));
        let dest = dir.join("1042.png");
        let cov: Value = serde_json::from_str(
            &execute(
                &host,
                "hub.cover.to_file",
                &json!({"app_id": 1042, "dest_path": dest.to_string_lossy()}).to_string(),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(cov["ok"]["status"], "ok");
        assert_eq!(std::fs::read(&dest).unwrap(), b"pngbytes");
        let _ = std::fs::remove_dir_all(&dir);

        let base = SyncProfile {
            id: "c".into(),
            revision: 1,
            scope: ProfileScope::Device,
            name: "Base".into(),
            stream: StreamProfileSpec {
                fps: Some(60),
                bitrate_kbps: Some(20_000),
                ..Default::default()
            },
            ..Default::default()
        };
        let mut remote = base.clone();
        remote.revision = 2;
        remote.stream.fps = Some(90);
        let merged: Value = serde_json::from_str(&policy(
            "merge_three_way",
            &json!({
                "base": base,
                "local": {
                    "scope": "device",
                    "name": "Base",
                    "stream": { "fps": 60, "bitrate_kbps": 35_000 }
                },
                "remote": remote
            })
            .to_string(),
        ))
        .unwrap();
        assert_eq!(merged["ok"]["status"], "merged");
        assert_eq!(merged["ok"]["write"]["stream"]["fps"], 90);
        assert_eq!(merged["ok"]["write"]["stream"]["bitrate_kbps"], 35_000);
    }
}
