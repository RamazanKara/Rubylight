//! Media, encoder, capture pacing, display policy, HDR, packet framing, FEC, and RTSP decisions.
#![warn(
    clippy::print_stdout,
    clippy::print_stderr,
    clippy::undocumented_unsafe_blocks
)]

/// How long a dropped packet, frame or input stays on the stream card after
/// it last happened.
pub const EVENT_PERIOD: std::time::Duration = std::time::Duration::from_secs(30);

/// Sink for user-facing session warnings raised by media-policy helpers.
pub trait WarningSink: Send + Sync {
    fn set(&self, code: &str, message: &str);
    fn event(&self, code: &str, message: &str, period: std::time::Duration) {
        let _ = period;
        self.set(code, message);
    }
    fn clear(&self, code: &str);
    fn replay(&self, _target: &dyn WarningSink) {}
    fn snapshot(&self) -> Vec<RecordedWarning> {
        Vec::new()
    }
}

impl<T: WarningSink + ?Sized> WarningSink for std::sync::Arc<T> {
    fn set(&self, code: &str, message: &str) {
        (**self).set(code, message);
    }

    fn event(&self, code: &str, message: &str, period: std::time::Duration) {
        (**self).event(code, message, period);
    }

    fn clear(&self, code: &str) {
        (**self).clear(code);
    }

    fn replay(&self, target: &dyn WarningSink) {
        (**self).replay(target);
    }

    fn snapshot(&self) -> Vec<RecordedWarning> {
        (**self).snapshot()
    }
}

/// A warning captured in [`RecordedWarnings`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedWarning {
    pub code: String,
    pub message: String,
}

/// In-memory [`WarningSink`] for startup probes before a session attaches and for tests.
#[derive(Default)]
pub struct RecordedWarnings {
    entries:
        std::sync::Mutex<std::collections::BTreeMap<String, (String, Option<std::time::Duration>)>>,
}

impl WarningSink for RecordedWarnings {
    fn set(&self, code: &str, message: &str) {
        self.entries
            .lock()
            .unwrap()
            .insert(code.to_owned(), (message.to_owned(), None));
    }

    fn event(&self, code: &str, message: &str, period: std::time::Duration) {
        self.entries
            .lock()
            .unwrap()
            .insert(code.to_owned(), (message.to_owned(), Some(period)));
    }

    fn clear(&self, code: &str) {
        self.entries.lock().unwrap().remove(code);
    }

    fn replay(&self, target: &dyn WarningSink) {
        let entries: Vec<_> = self
            .entries
            .lock()
            .unwrap()
            .iter()
            .map(|(code, (message, period))| (code.clone(), message.clone(), *period))
            .collect();
        for (code, message, period) in entries {
            match period {
                Some(period) => target.event(&code, &message, period),
                None => target.set(&code, &message),
            }
        }
    }

    fn snapshot(&self) -> Vec<RecordedWarning> {
        self.snapshot()
    }
}

/// Shared [`RecordedWarnings`] sink for probes and tests before a session attaches.
pub fn recorded_warnings() -> std::sync::Arc<dyn WarningSink> {
    std::sync::Arc::new(RecordedWarnings::default())
}

impl RecordedWarnings {
    pub fn snapshot(&self) -> Vec<RecordedWarning> {
        self.entries
            .lock()
            .unwrap()
            .iter()
            .map(|(code, (message, _))| RecordedWarning {
                code: code.clone(),
                message: message.clone(),
            })
            .collect()
    }
}

#[cfg(test)]
pub(crate) type TestWarnings = RecordedWarnings;

pub mod adaptive_bitrate;
pub mod adaptive_fec;
pub mod adaptive_pacing;
pub mod amf_vpe;
pub mod app_window;
pub mod arrival_gaps;
pub mod audio;
pub mod audio_cadence;
pub mod audio_defaults;
pub mod audio_dsp;
pub mod audio_passthrough;
pub mod bandwidth_probe;
pub mod bitstream;
pub mod capture_policy;
pub mod clock_sync;
pub mod codec_support;
pub mod congestion;
pub mod display_caps;
pub mod display_policy;
pub mod display_session_machine;
pub mod dovi;
pub mod edid;
pub mod encoder_backlog;
pub mod encoder_focus;
pub mod encoder_gpu;
pub mod encoder_policy;
pub mod extensions;
pub mod frame_warp;
pub mod framegen;
pub mod hdr;
pub mod hdr_dynamic;
pub mod host_timing;
pub mod image_quality;
pub mod keyframe_loss;
pub mod link_control;
pub mod link_notice;
pub mod link_rate;
pub mod lossless;
pub mod ltr;
pub mod network_pacing;
pub mod network_path;
pub mod nvenc;
pub mod nvidia_component;
pub mod nvidia_enhance;
pub mod os_gate;
pub mod pacing_high_hz;
pub mod packet;
pub mod performance;
pub mod phase_sync;
pub mod pipeline_rates;
pub mod present_timing;
pub mod pyrowave;
pub mod pyrowave_probe;
pub mod radeon_profile;
pub mod rate_change;
pub mod reconfigure;
pub mod rtsp;
pub mod stall_watch;
pub mod statistical_fec;
pub mod stream_policy;
pub mod stream_preset;
pub mod stutter_report;
pub mod timing_record;
pub mod topology;
pub mod virtual_output;
