//! Writing Stream DVR's files: a replay clip on a `dvr-write` thread of its own at below-normal
//! priority (the replay buffer's frames are shared, so the worker keeps taking the stream while
//! it runs), and a recording fragment by fragment as the worker receives it.
//!
//! Every file is written as `<id>.mp4.tmp`, synced, given its sidecar and renamed to
//! `<id>.mp4` (`crate::dvr_store`); a failed write deletes its `.tmp`.
use super::Reply;
use crate::dvr_store::{self, DvrStore};
use butterpollo_media_policy::dvr::{
    AudioPacket, StreamInfo, VideoCodec, VideoFrame,
    fmp4::{ClipInfo, write_clip},
    quota,
    recorder::{RecPush, Recorder},
    ring::Clip,
};
use rubylight_native_api::dvr::{ClipKind, ClipSummary};
use rubylight_protocol::stream_dvr::Reason;
use std::fs::File;
use std::io::{self, BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::thread::JoinHandle;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

const BUFFER: usize = 1 << 20;
/// Windows' "There is not enough space on the disk".
const ERROR_DISK_FULL: i32 = 112;
const ERROR_HANDLE_DISK_FULL: i32 = 39;

/// The summary of a written clip.
pub(super) fn summary(id: &str, title: &str, kind: ClipKind, info: &ClipInfo) -> ClipSummary {
    ClipSummary {
        id: id.to_owned(),
        app_title: title.to_owned(),
        created_unix_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64),
        duration_ms: info.duration_ms,
        width: info.width,
        height: info.height,
        fps: (f64::from(info.fps) * 100.0).round() / 100.0,
        codec: codec_name(info.codec).into(),
        hdr: info.hdr,
        container: "mp4".into(),
        size_bytes: info.bytes,
        kind,
        recovered: false,
    }
}

/// The codec as clip summaries name it.
pub(super) fn codec_name(codec: VideoCodec) -> &'static str {
    match codec {
        VideoCodec::H264 => "h264",
        VideoCodec::Hevc => "hevc",
        VideoCodec::Av1 => "av1",
    }
}

/// The reason a write failed with `error`.
pub(super) fn io_reason(error: &io::Error) -> Reason {
    match error.raw_os_error() {
        Some(ERROR_DISK_FULL | ERROR_HANDLE_DISK_FULL) => Reason::DiskFull,
        _ => Reason::IoError,
    }
}

/// A replay clip being written.
pub(super) struct ReplayJob {
    handle: JoinHandle<Result<ClipSummary, Reason>>,
    reply: Reply,
    pub(super) id: String,
}

impl ReplayJob {
    /// Starts writing `clip` as `id` in `dir`.
    pub(super) fn spawn(
        dir: PathBuf,
        id: String,
        title: String,
        clip: Clip,
        reply: Reply,
    ) -> io::Result<Self> {
        let name = id.clone();
        let handle = std::thread::Builder::new()
            .name("dvr-write".into())
            .spawn(move || {
                butterpollo_win_io::background_write::lower_current_thread();
                write_replay(&dir, &name, &title, &clip)
            })?;
        Ok(Self { handle, reply, id })
    }

    pub(super) fn is_finished(&self) -> bool {
        self.handle.is_finished()
    }

    /// The written clip, once [`Self::is_finished`]; waits otherwise.
    pub(super) fn join(self) -> (Reply, Result<ClipSummary, Reason>) {
        let result = self.handle.join().unwrap_or(Err(Reason::Unknown));
        (self.reply, result)
    }
}

/// Writes `clip` to `dir` as `id`; see the module documentation.
pub(super) fn write_replay(
    dir: &Path,
    id: &str,
    title: &str,
    clip: &Clip,
) -> Result<ClipSummary, Reason> {
    let started = Instant::now();
    let temp = dvr_store::temp_path(dir, id);
    let written = (|| -> Result<ClipInfo, Reason> {
        let file = File::create(&temp).map_err(|e| io_reason(&e))?;
        let mut out = BufWriter::with_capacity(BUFFER, file);
        let info = write_clip(&mut out, clip).map_err(|error| {
            tracing::warn!(%error, "stream dvr: the clip could not be muxed");
            Reason::IoError
        })?;
        let file = out.into_inner().map_err(|e| io_reason(e.error()))?;
        file.sync_all().map_err(|e| io_reason(&e))?;
        Ok(info)
    })();
    let result = written.and_then(|info| {
        let summary = summary(id, title, ClipKind::Replay, &info);
        dvr_store::write_sidecar(dir, &summary)
            .and_then(|()| std::fs::rename(&temp, dvr_store::video_path(dir, id)))
            .map_err(|e| io_reason(&e))?;
        Ok(summary)
    });
    match &result {
        Ok(summary) => tracing::info!(
            clip = id,
            duration_ms = summary.duration_ms,
            bytes = summary.size_bytes,
            write_ms = started.elapsed().as_millis() as u64,
            "stream dvr: clip saved"
        ),
        Err(reason) => {
            let _ = std::fs::remove_file(&temp);
            tracing::warn!(
                clip = id,
                reason = reason.as_str(),
                "stream dvr: clip not saved"
            );
        }
    }
    result
}

/// What a recording asks of the worker after a picture.
pub(super) enum RecordingStep {
    Continue,
    /// The stream's format changed at this keyframe: finish this part and start the next with
    /// it.
    Restart(VideoFrame),
    /// Pictures wait for the next keyframe: ask the encoder for one.
    AwaitKeyframe,
}

/// A recording being written.
pub(super) struct Recording {
    pub(super) id: String,
    part: u32,
    base: String,
    dir: PathBuf,
    title: String,
    out: BufWriter<File>,
    recorder: Recorder,
    pub(super) since: Instant,
    written: u64,
}

impl Recording {
    /// Opens part `part` of recording `base` with `first`, a keyframe.
    pub(super) fn start(
        dir: &Path,
        base: &str,
        part: u32,
        title: &str,
        info: &StreamInfo,
        first: VideoFrame,
        since: Instant,
    ) -> Result<Self, Reason> {
        let id = if part <= 1 {
            base.to_owned()
        } else {
            format!("{base}-part{part}")
        };
        if !quota::valid_clip_id(&id) {
            return Err(Reason::IoError);
        }
        let (recorder, init) = Recorder::start(info, first).map_err(|error| {
            tracing::warn!(%error, "stream dvr: the recording could not start");
            Reason::IoError
        })?;
        let temp = dvr_store::temp_path(dir, &id);
        let opened = (|| -> io::Result<BufWriter<File>> {
            let mut out = BufWriter::with_capacity(BUFFER, File::create(&temp)?);
            out.write_all(&init)?;
            out.flush()?;
            Ok(out)
        })();
        let out = opened.map_err(|e| {
            let _ = std::fs::remove_file(&temp);
            io_reason(&e)
        })?;
        let draft = ClipSummary {
            id: id.clone(),
            app_title: title.to_owned(),
            created_unix_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_millis() as u64),
            codec: codec_name(info.codec).into(),
            hdr: info.hdr.is_some(),
            container: "mp4".into(),
            kind: ClipKind::Manual,
            ..Default::default()
        };
        if let Err(error) = dvr_store::write_draft(dir, &draft) {
            tracing::debug!(%error, "stream dvr: the recording's draft sidecar was not written");
        }
        tracing::info!(clip = %id, "stream dvr: recording started");
        Ok(Self {
            id,
            part: part.max(1),
            base: base.to_owned(),
            dir: dir.to_owned(),
            title: title.to_owned(),
            out,
            recorder,
            since,
            written: init.len() as u64,
        })
    }

    /// The bytes written so far.
    pub(super) fn written(&self) -> u64 {
        self.written
    }

    /// The next part's number, for [`Self::start`] after a [`RecordingStep::Restart`].
    pub(super) fn next_part(&self) -> (String, u32) {
        (self.base.clone(), self.part + 1)
    }

    /// Takes a picture; a full fragment is appended and the quota checked.
    pub(super) fn video(
        &mut self,
        store: &DvrStore,
        limit: u64,
        info: &StreamInfo,
        frame: &VideoFrame,
    ) -> Result<RecordingStep, Reason> {
        match self.recorder.push_video(info, frame.clone()) {
            RecPush::Buffered => Ok(RecordingStep::Continue),
            RecPush::Restart(frame) => Ok(RecordingStep::Restart(frame)),
            RecPush::AwaitingKeyframe => Ok(RecordingStep::AwaitKeyframe),
            RecPush::Fragment(bytes) => {
                let fragment = bytes.len() as u64;
                store
                    .make_room(
                        &self.dir,
                        self.written + fragment,
                        fragment,
                        limit,
                        &[self.id.clone()],
                    )
                    .map_err(|refusal| match refusal {
                        quota::Refusal::DiskFull => Reason::DiskFull,
                        _ => Reason::QuotaExceeded,
                    })?;
                self.out
                    .write_all(&bytes)
                    .and_then(|()| self.out.flush())
                    .map_err(|e| io_reason(&e))?;
                self.written += fragment;
                Ok(RecordingStep::Continue)
            }
        }
    }

    pub(super) fn audio(&mut self, packet: AudioPacket) {
        self.recorder.push_audio(packet);
    }

    /// A tapped copy was lost: pictures wait for the next keyframe.
    pub(super) fn discontinuity(&mut self) {
        self.recorder.discontinuity();
    }

    /// Finishes the file: its last fragment, its length, its sidecar and its name.
    pub(super) fn finish(self) -> Result<ClipSummary, Reason> {
        let Self {
            id,
            dir,
            title,
            mut out,
            recorder,
            ..
        } = self;
        let temp = dvr_store::temp_path(&dir, &id);
        let finished = recorder.finish();
        let result = (|| -> io::Result<ClipSummary> {
            out.write_all(&finished.tail)?;
            let mut file = out
                .into_inner()
                .map_err(|e| io::Error::other(e.to_string()))?;
            if let Some((offset, bytes)) = &finished.duration_patch {
                file.seek(SeekFrom::Start(*offset))?;
                file.write_all(bytes)?;
            }
            file.sync_all()?;
            let mut summary = summary(&id, &title, ClipKind::Manual, &finished.info);
            summary.size_bytes = file.metadata()?.len();
            drop(file);
            dvr_store::write_sidecar(&dir, &summary)?;
            std::fs::rename(&temp, dvr_store::video_path(&dir, &id))?;
            Ok(summary)
        })();
        match result {
            Ok(summary) => {
                tracing::info!(clip = %id, duration_ms = summary.duration_ms, bytes = summary.size_bytes, "stream dvr: recording saved");
                Ok(summary)
            }
            Err(error) => {
                // The complete fragments stay in the .tmp file: the next start salvages them.
                tracing::warn!(clip = %id, %error, "stream dvr: recording not finished");
                Err(io_reason(&error))
            }
        }
    }
}
