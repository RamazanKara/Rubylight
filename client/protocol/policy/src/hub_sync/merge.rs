//! Three-way field merge (`Base + Local + Remote`) for Host Hub Sync profiles.

use rubylight_native_api::hub::{
    NetworkMatcher, ProfileScope, ProfileWrite, StreamProfileSpec, SyncProfile,
};

use super::{MergeOutcome, ProfileConflict};

fn merge_field<T: Clone + PartialEq>(
    field: &str,
    base: Option<&T>,
    local: &T,
    remote: &T,
    conflicts: &mut Vec<String>,
) -> T {
    match base {
        Some(b) => {
            let local_changed = local != b;
            let remote_changed = remote != b;
            if local_changed && remote_changed && local != remote {
                conflicts.push(field.to_string());
                local.clone()
            } else if local_changed {
                local.clone()
            } else {
                remote.clone()
            }
        }
        None => {
            if local != remote {
                conflicts.push(field.to_string());
            }
            local.clone()
        }
    }
}

fn merge_opt<T: Clone + PartialEq>(
    field: &str,
    base: Option<&Option<T>>,
    local: &Option<T>,
    remote: &Option<T>,
    conflicts: &mut Vec<String>,
) -> Option<T> {
    match base {
        Some(b) => {
            let local_changed = local != b;
            let remote_changed = remote != b;
            if local_changed && remote_changed && local != remote {
                conflicts.push(field.to_string());
                local.clone()
            } else if local_changed {
                local.clone()
            } else {
                remote.clone()
            }
        }
        None => match (local, remote) {
            (Some(l), None) => Some(l.clone()),
            (None, Some(r)) => Some(r.clone()),
            (Some(l), Some(r)) if l == r => Some(l.clone()),
            (None, None) => None,
            _ => {
                conflicts.push(field.to_string());
                local.clone()
            }
        },
    }
}

/// Performs field-level 3-way merge of a profile write against a newer server `remote`.
pub fn merge_three_way(
    base: Option<&SyncProfile>,
    local: &ProfileWrite,
    remote: &SyncProfile,
) -> MergeOutcome {
    let mut conflicts = Vec::new();
    let base_owner: Option<Option<String>> =
        base.map(|b| (!b.owner_device_uuid.is_empty()).then(|| b.owner_device_uuid.clone()));
    let remote_owner =
        (!remote.owner_device_uuid.is_empty()).then(|| remote.owner_device_uuid.clone());

    let scope = merge_field(
        "scope",
        base.map(|b| &b.scope),
        &local.scope,
        &remote.scope,
        &mut conflicts,
    );
    let owner_device_uuid = if scope == ProfileScope::Device && local.owner_device_uuid.is_none() {
        remote_owner.clone()
    } else {
        merge_opt(
            "owner_device_uuid",
            base_owner.as_ref(),
            &local.owner_device_uuid,
            &remote_owner,
            &mut conflicts,
        )
    };
    let name = merge_field(
        "name",
        base.map(|b| &b.name),
        &local.name,
        &remote.name,
        &mut conflicts,
    );
    let network_matcher: Option<NetworkMatcher> = merge_opt(
        "network_matcher",
        base.map(|b| &b.network_matcher),
        &local.network_matcher,
        &remote.network_matcher,
        &mut conflicts,
    );
    let input_deck_id = merge_opt(
        "input_deck_id",
        base.map(|b| &b.input_deck_id),
        &local.input_deck_id,
        &remote.input_deck_id,
        &mut conflicts,
    );

    let bs = base.map(|b| &b.stream);
    let ls = &local.stream;
    let rs = &remote.stream;
    let stream = StreamProfileSpec {
        width: merge_opt(
            "stream.width",
            bs.map(|s| &s.width),
            &ls.width,
            &rs.width,
            &mut conflicts,
        ),
        height: merge_opt(
            "stream.height",
            bs.map(|s| &s.height),
            &ls.height,
            &rs.height,
            &mut conflicts,
        ),
        fps: merge_opt(
            "stream.fps",
            bs.map(|s| &s.fps),
            &ls.fps,
            &rs.fps,
            &mut conflicts,
        ),
        bitrate_kbps: merge_opt(
            "stream.bitrate_kbps",
            bs.map(|s| &s.bitrate_kbps),
            &ls.bitrate_kbps,
            &rs.bitrate_kbps,
            &mut conflicts,
        ),
        codec: merge_opt(
            "stream.codec",
            bs.map(|s| &s.codec),
            &ls.codec,
            &rs.codec,
            &mut conflicts,
        ),
        hdr: merge_opt(
            "stream.hdr",
            bs.map(|s| &s.hdr),
            &ls.hdr,
            &rs.hdr,
            &mut conflicts,
        ),
        audio_layout: merge_opt(
            "stream.audio_layout",
            bs.map(|s| &s.audio_layout),
            &ls.audio_layout,
            &rs.audio_layout,
            &mut conflicts,
        ),
        upscaling: merge_opt(
            "stream.upscaling",
            bs.map(|s| &s.upscaling),
            &ls.upscaling,
            &rs.upscaling,
            &mut conflicts,
        ),
        video_scaling: merge_opt(
            "stream.video_scaling",
            bs.map(|s| &s.video_scaling),
            &ls.video_scaling,
            &rs.video_scaling,
            &mut conflicts,
        ),
        orientation: merge_opt(
            "stream.orientation",
            bs.map(|s| &s.orientation),
            &ls.orientation,
            &rs.orientation,
            &mut conflicts,
        ),
        resolution_scale_pct: merge_opt(
            "stream.resolution_scale_pct",
            bs.map(|s| &s.resolution_scale_pct),
            &ls.resolution_scale_pct,
            &rs.resolution_scale_pct,
            &mut conflicts,
        ),
        display_matcher: merge_opt(
            "stream.display_matcher",
            bs.map(|s| &s.display_matcher),
            &ls.display_matcher,
            &rs.display_matcher,
            &mut conflicts,
        ),
    };

    if conflicts.is_empty() {
        MergeOutcome::Merged(ProfileWrite {
            scope,
            owner_device_uuid,
            name,
            network_matcher,
            stream,
            input_deck_id,
        })
    } else {
        MergeOutcome::Conflict(Box::new(ProfileConflict {
            id: remote.id.clone(),
            server_revision: remote.revision,
            conflicting_fields: conflicts,
            base: base.cloned(),
            local: local.clone(),
            remote: remote.clone(),
        }))
    }
}
