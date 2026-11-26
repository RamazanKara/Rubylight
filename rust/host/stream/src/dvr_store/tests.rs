//! The clips folder: listing, pages, deleting, ids, making room and crash recovery, on a
//! scratch folder in the temp directory.
use super::*;
use std::time::Duration;

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "rubylight-dvr-{name}-{}-{:08x}",
            std::process::id(),
            rand::random::<u32>()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn clip(id: &str, created_unix_ms: u64, kind: ClipKind) -> ClipSummary {
    ClipSummary {
        id: id.into(),
        app_title: "Game".into(),
        created_unix_ms,
        duration_ms: 30_000,
        width: 1920,
        height: 1080,
        fps: 60.0,
        codec: "hevc".into(),
        container: "mp4".into(),
        kind,
        ..Default::default()
    }
}

fn save(dir: &Path, summary: &ClipSummary, bytes: usize) {
    std::fs::write(video_path(dir, &summary.id), vec![0u8; bytes]).unwrap();
    write_sidecar(dir, summary).unwrap();
}

#[test]
fn lists_saved_clips_newest_first_with_their_file_sizes() {
    let scratch = Scratch::new("list");
    let dir = &scratch.0;
    save(
        dir,
        &clip("20260101-100000-replay-0001", 1_000, ClipKind::Replay),
        10,
    );
    save(
        dir,
        &clip("20260101-110000-manual-0002", 3_000, ClipKind::Manual),
        30,
    );
    save(
        dir,
        &clip("20260101-105000-replay-0003", 2_000, ClipKind::Replay),
        20,
    );
    // A sidecar without its video, a video without a sidecar and a stray name are not clips.
    write_sidecar(
        dir,
        &clip("20260101-090000-replay-0004", 500, ClipKind::Replay),
    )
    .unwrap();
    std::fs::write(video_path(dir, "20260101-080000-replay-0005"), b"x").unwrap();
    std::fs::write(dir.join("notes.json"), b"{}").unwrap();
    let clips = list(dir);
    let ids: Vec<&str> = clips.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "20260101-110000-manual-0002",
            "20260101-105000-replay-0003",
            "20260101-100000-replay-0001"
        ]
    );
    assert_eq!(clips[0].size_bytes, 30);
    assert_eq!(used_bytes(&clips), 60);
    assert_eq!(
        find(dir, "20260101-105000-replay-0003").map(|c| c.size_bytes),
        Some(20)
    );
    assert_eq!(find(dir, "../../windows"), None);
}

#[test]
fn pages_follow_the_cursor() {
    let clips: Vec<ClipSummary> = (0..5)
        .map(|n| {
            clip(
                &format!("20260101-10000{n}-replay-000{n}"),
                10 - n,
                ClipKind::Replay,
            )
        })
        .collect();
    let first = page(clips.clone(), Some(2), None, 99);
    assert_eq!(first.clips.len(), 2);
    assert_eq!(
        first.next_cursor.as_deref(),
        Some("20260101-100001-replay-0001")
    );
    assert_eq!(first.quota_limit_bytes, 99);
    let second = page(clips.clone(), Some(2), first.next_cursor.as_deref(), 99);
    assert_eq!(second.clips[0].id, "20260101-100002-replay-0002");
    let last = page(
        clips.clone(),
        Some(2),
        Some("20260101-100003-replay-0003"),
        99,
    );
    assert_eq!(last.clips.len(), 1);
    assert_eq!(last.next_cursor, None);
    assert_eq!(page(clips.clone(), None, None, 0).clips.len(), 5);
    assert_eq!(page(clips, Some(0), None, 0).clips.len(), 1, "at least one");
}

#[test]
fn deletes_only_clip_ids_inside_the_folder() {
    let scratch = Scratch::new("delete");
    let dir = &scratch.0;
    let id = "20260101-100000-replay-0001";
    save(dir, &clip(id, 1, ClipKind::Replay), 4);
    assert!(!delete(dir, "..").unwrap());
    assert!(!delete(dir, "../20260101-100000-replay-0001").unwrap());
    let store = DvrStore::default();
    assert!(store.delete(dir, id).unwrap());
    assert!(!video_path(dir, id).exists());
    assert!(!store.delete(dir, id).unwrap(), "gone already");
}

#[test]
fn new_ids_are_valid_and_name_their_kind() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_790_000_000);
    let replay = new_id(ClipKind::Replay, now);
    let manual = new_id(ClipKind::Manual, now);
    assert!(quota::valid_clip_id(&replay), "{replay}");
    assert!(quota::valid_clip_id(&manual), "{manual}");
    assert!(replay.contains("-replay-"), "{replay}");
    assert!(manual.contains("-manual-"), "{manual}");
    assert_eq!(
        replay.len(),
        "YYYYMMDD-HHMMSS-replay-abcd".len(),
        "{replay}"
    );
    assert!(quota::valid_clip_id(&format!("{manual}-part2-recovered")));
}

#[test]
fn room_is_made_by_deleting_the_oldest_replays_never_recordings() {
    let scratch = Scratch::new("room");
    let dir = &scratch.0;
    save(
        dir,
        &clip("20260101-100000-replay-0001", 1, ClipKind::Replay),
        400,
    );
    save(
        dir,
        &clip("20260101-100000-manual-0002", 2, ClipKind::Manual),
        400,
    );
    save(
        dir,
        &clip("20260101-100000-replay-0003", 3, ClipKind::Replay),
        400,
    );
    let store = DvrStore::default();
    let evicted = store.make_room(dir, 300, 300, 1_000, &[]).unwrap();
    assert_eq!(evicted, ["20260101-100000-replay-0001"]);
    assert!(!video_path(dir, "20260101-100000-replay-0001").exists());
    // Only recordings and the clip being written are left: no room.
    assert!(
        store
            .make_room(
                dir,
                900,
                900,
                1_000,
                &["20260101-100000-replay-0003".into()]
            )
            .is_err()
    );
    assert!(video_path(dir, "20260101-100000-manual-0002").exists());
}

#[test]
fn recovery_completes_renamed_sidecars_and_deletes_torn_files() {
    let scratch = Scratch::new("recover");
    let dir = &scratch.0;
    // Written in full: the sidecar was renamed before the crash.
    let complete = clip("20260101-100000-replay-0001", 1, ClipKind::Replay);
    std::fs::write(temp_path(dir, &complete.id), b"complete").unwrap();
    write_sidecar(dir, &complete).unwrap();
    // Torn: no complete fragment, with a draft.
    let torn = "20260101-100000-manual-0002";
    std::fs::write(temp_path(dir, torn), b"\0\0\0\x08ftyp").unwrap();
    write_draft(dir, &clip(torn, 2, ClipKind::Manual)).unwrap();
    // A draft without its video and a write check.
    write_draft(
        dir,
        &clip("20260101-100000-manual-0003", 3, ClipKind::Manual),
    )
    .unwrap();
    std::fs::write(dir.join(".write-check-1234"), b"").unwrap();
    let cutoff = SystemTime::now() + Duration::from_secs(5);
    let done = recover_before(dir, cutoff);
    assert_eq!(done.completed, 1);
    assert_eq!(done.deleted, 1);
    assert_eq!(done.salvaged, 0);
    assert!(video_path(dir, &complete.id).exists());
    assert!(!temp_path(dir, torn).exists());
    let left: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().into_string().unwrap())
        .collect();
    assert!(
        left.iter()
            .all(|name| !name.ends_with(".tmp") && !name.starts_with(".write-check")),
        "{left:?}"
    );
}

#[test]
fn recovery_leaves_files_a_running_stream_writes() {
    let scratch = Scratch::new("recover-new");
    let dir = &scratch.0;
    let id = "20260101-100000-manual-0001";
    std::fs::write(temp_path(dir, id), b"in progress").unwrap();
    write_draft(dir, &clip(id, 1, ClipKind::Manual)).unwrap();
    let before = SystemTime::now() - Duration::from_secs(60);
    assert_eq!(recover_before(dir, before), Recovered::default());
    assert!(temp_path(dir, id).exists());
    assert!(draft_path(dir, id).exists());
}

#[test]
fn the_folder_and_quota_come_from_the_settings() {
    let config = Config::parse("").unwrap();
    let dir = clips_dir(
        Path::new(r"C:\ProgramData\Butterpollo\config\sunshine.conf"),
        &config,
    );
    assert_eq!(dir, Path::new(r"C:\ProgramData\Butterpollo\config\clips"));
    assert_eq!(quota_bytes(&config), 20 * GIB);
    let custom = Config::parse("dvr_directory = D:\\Clips\ndvr_quota_gb = 5\n").unwrap();
    assert_eq!(
        clips_dir(Path::new("x/sunshine.conf"), &custom),
        Path::new(r"D:\Clips")
    );
    assert_eq!(quota_bytes(&custom), 5 * GIB);
}
