//! Stream DVR over the native API (`/api/native/v1/dvr*`), for a host that lists
//! `stream.dvr`: the DVR state, saving a clip, starting and stopping a recording, and the saved
//! clips. A running stream asks over control message 0x554A instead; these calls serve a client
//! without that stream (the PC's page, a companion device). Types come from
//! `rubylight_native_api::dvr`.

use super::super::native_api::dvr::{
    ClipList, ClipRequest, ClipSaved, DvrStatus, RecordAction, RecordRequest, RecordResult,
};
use super::{ACTION_READ_TIMEOUT, Error, HostClient, MAX_JSON_BYTES, READ_TIMEOUT, url_component};

/// Most clips one page of [`HostClient::dvr_clips`] asks for.
pub const MAX_CLIPS_PAGE: u32 = 200;

impl HostClient {
    /// The DVR state for the current stream (`GET /dvr`).
    pub fn dvr_status(&self) -> Result<DvrStatus, Error> {
        self.native("dvr", None, MAX_JSON_BYTES, READ_TIMEOUT)
    }

    /// Saves the replay buffer's last `duration_secs` seconds, or the whole buffer for `None`
    /// (`POST /dvr/clip`); the host clamps it to at least 5 seconds and at most its buffer. A
    /// refusal is an [`Error::Api`] with `dvr_refused` and the reason in its message.
    pub fn save_dvr_clip(&self, duration_secs: Option<u16>) -> Result<ClipSaved, Error> {
        let body = serde_json::to_vec(&ClipRequest { duration_secs })
            .map_err(|e| Error::InvalidArgument(format!("dvr clip: {e}")))?;
        // Writing the clip waits for the host to finish it.
        self.native("dvr/clip", Some(&body), MAX_JSON_BYTES, ACTION_READ_TIMEOUT)
    }

    /// Starts or stops a recording (`POST /dvr/record`); a stop carries the finished clip.
    pub fn dvr_record(&self, action: RecordAction) -> Result<RecordResult, Error> {
        let body = serde_json::to_vec(&RecordRequest { action })
            .map_err(|e| Error::InvalidArgument(format!("dvr record: {e}")))?;
        self.native(
            "dvr/record",
            Some(&body),
            MAX_JSON_BYTES,
            ACTION_READ_TIMEOUT,
        )
    }

    /// One page of saved clips, newest first (`GET /dvr/clips`): up to `limit` (at most
    /// [`MAX_CLIPS_PAGE`]), after the `cursor` of the page before.
    pub fn dvr_clips(&self, limit: u32, cursor: Option<&str>) -> Result<ClipList, Error> {
        let mut path = format!("dvr/clips?limit={}", limit.clamp(1, MAX_CLIPS_PAGE));
        if let Some(cursor) = cursor.filter(|c| !c.is_empty()) {
            path.push_str("&cursor=");
            path.push_str(&url_component(cursor));
        }
        self.native(&path, None, MAX_JSON_BYTES, READ_TIMEOUT)
    }
}
