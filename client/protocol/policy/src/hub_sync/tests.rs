//! Unit tests for Host Hub Sync policy (3-way merge, outbox compaction, cover diffing).

use super::*;
use rubylight_native_api::hub::{NetworkMatcher, ProfileScope};

fn profile(rev: u64, fps: u32, bitrate: u32, res_scale: Option<u8>) -> SyncProfile {
    SyncProfile {
        id: "couch".into(),
        revision: rev,
        scope: ProfileScope::Device,
        owner_device_uuid: "dev-1".into(),
        name: "Couch".into(),
        network_matcher: Some(NetworkMatcher {
            kind: "lan".into(),
            ssid: None,
        }),
        stream: StreamProfileSpec {
            width: Some(1920),
            height: Some(1080),
            fps: Some(fps),
            bitrate_kbps: Some(bitrate),
            resolution_scale_pct: res_scale,
            ..Default::default()
        },
        input_deck_id: None,
        updated_unix_ms: 100,
        updated_by_device: "dev-1".into(),
    }
}

#[test]
fn merges_disjoint_three_way_edits_and_flags_same_field_collisions() {
    let base = profile(7, 60, 40_000, Some(100));
    let mut local = profile_to_write(&base);
    local.stream.bitrate_kbps = Some(25_000);
    let remote = profile(8, 90, 40_000, Some(75));

    match merge_three_way(Some(&base), &local, &remote) {
        MergeOutcome::Merged(merged) => {
            assert_eq!(merged.stream.bitrate_kbps, Some(25_000));
            assert_eq!(merged.stream.fps, Some(90));
            assert_eq!(merged.stream.resolution_scale_pct, Some(75));
        }
        MergeOutcome::Conflict(c) => panic!("unexpected conflict: {c:?}"),
    }

    // Same field changed differently -> Conflict.
    let mut collide = profile_to_write(&base);
    collide.stream.fps = Some(120);
    match merge_three_way(Some(&base), &collide, &remote) {
        MergeOutcome::Conflict(c) => {
            assert_eq!(c.server_revision, 8);
            assert_eq!(c.conflicting_fields, vec!["stream.fps"]);
        }
        MergeOutcome::Merged(_) => panic!("expected conflict on fps"),
    }
}

#[test]
fn compacts_outbox_puts_and_deletes_cleanly() {
    let base = profile(3, 60, 30_000, None);
    let w1 = profile_to_write(&base);
    let mut w2 = w1.clone();
    w2.name = "Updated".into();
    let compacted = compact_outbox(vec![
        OutboxOp::Put {
            id: "couch".into(),
            base_revision: 3,
            base: Some(base.clone()),
            write: w1.clone(),
            queued_unix_ms: 1,
        },
        OutboxOp::Put {
            id: "temp-new".into(),
            base_revision: 0,
            base: None,
            write: w1.clone(),
            queued_unix_ms: 2,
        },
        OutboxOp::Delete {
            id: "temp-new".into(),
            base_revision: 0,
            base: None,
            queued_unix_ms: 3,
        },
        OutboxOp::Put {
            id: "couch".into(),
            base_revision: 4,
            base: None,
            write: w2.clone(),
            queued_unix_ms: 4,
        },
    ]);
    assert_eq!(compacted.len(), 1);
    match &compacted[0] {
        OutboxOp::Put {
            id,
            base_revision,
            write,
            ..
        } => {
            assert_eq!(id, "couch");
            assert_eq!(*base_revision, 3);
            assert_eq!(write.name, "Updated");
        }
        _ => panic!("expected compacted put"),
    }
}

#[test]
fn plans_cover_fetches_only_when_etag_differs() {
    let remote = vec![
        CoverEntry {
            app_id: 1,
            etag: "sha256:aaa".into(),
            ..Default::default()
        },
        CoverEntry {
            app_id: 2,
            etag: "sha256:bbb".into(),
            ..Default::default()
        },
        CoverEntry {
            app_id: 3,
            etag: "sha256:ccc".into(),
            ..Default::default()
        },
    ];
    let local = BTreeMap::from([
        (1, "sha256:aaa".into()),
        (2, "sha256:old".into()),
        (99, "sha256:gone".into()),
    ]);
    let fetch: Vec<u32> = covers_to_fetch(&remote, &local)
        .into_iter()
        .map(|c| c.app_id)
        .collect();
    assert_eq!(fetch, vec![2, 3]);
    assert_eq!(stale_local_cover_ids(&remote, &local), vec![99]);
}
