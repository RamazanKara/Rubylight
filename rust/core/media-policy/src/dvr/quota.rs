//! Clip storage policy: clip id validation and the quota plan that frees room for a new
//! clip by evicting the oldest replay clips (never manual recordings, never an open file),
//! refusing when the quota or the volume's free-space floor would still be exceeded.

/// Default total size of saved clips.
pub const DEFAULT_QUOTA_BYTES: u64 = 20 << 30;
/// Free space a clip write must leave on the clips volume.
pub const DISK_FLOOR_BYTES: u64 = 1 << 30;

/// A clip id is a file stem: 8 to 64 of `A-Z a-z 0-9 _ -`.
pub fn valid_clip_id(id: &str) -> bool {
    (8..=64).contains(&id.len())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClipKind {
    /// Saved from the instant-replay ring; evictable.
    Replay,
    /// A manual recording; never evicted automatically.
    Manual,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredClip {
    pub id: String,
    pub bytes: u64,
    pub created_unix_ms: u64,
    pub kind: ClipKind,
    /// Still being written (a recording in progress).
    pub open: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    QuotaExceeded,
    DiskFull,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::QuotaExceeded => "the clip quota is full",
            Self::DiskFull => "the clips drive is almost full",
        })
    }
}

impl std::error::Error for Refusal {}

/// The ids to delete, oldest first, before writing `incoming` bytes under `limit`.
/// `free_disk` is the volume's free space when known; the evicted bytes count as freed.
pub fn plan(
    clips: &[StoredClip],
    incoming: u64,
    limit: u64,
    free_disk: Option<u64>,
    floor: u64,
) -> Result<Vec<String>, Refusal> {
    let mut used: u64 = clips.iter().map(|clip| clip.bytes).sum();
    let mut candidates: Vec<&StoredClip> = clips
        .iter()
        .filter(|clip| clip.kind == ClipKind::Replay && !clip.open)
        .collect();
    candidates.sort_by(|a, b| {
        (a.created_unix_ms, a.id.as_str()).cmp(&(b.created_unix_ms, b.id.as_str()))
    });
    let mut evict = Vec::new();
    let mut freed = 0u64;
    let mut oldest = candidates.into_iter();
    while used.saturating_add(incoming) > limit {
        let Some(clip) = oldest.next() else {
            return Err(Refusal::QuotaExceeded);
        };
        used = used.saturating_sub(clip.bytes);
        freed = freed.saturating_add(clip.bytes);
        evict.push(clip.id.clone());
    }
    if let Some(free) = free_disk
        && free.saturating_add(freed) < incoming.saturating_add(floor)
    {
        return Err(Refusal::DiskFull);
    }
    Ok(evict)
}
