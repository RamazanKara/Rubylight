//! Driver-independent HDR update bookkeeping. A request is not applied until
//! AMF accepts it, and its keyframe remains pending until an input is accepted.
use crate::amf_abi::*;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct EncodedMetadata {
    primaries: [[u16; 2]; 3],
    white: [u16; 2],
    maximum: u32,
    minimum: u32,
    max_cll: u16,
    max_fall: u16,
}

impl EncodedMetadata {
    pub(super) fn new(codec: u8, metadata: butterpollo_core::hdr::Metadata) -> Self {
        // HEVC takes 1/50000 chromaticity and 1/10000 nit luminance. AMF
        // copies AV1 values into the OBU unscaled: 0.16 chromaticity, 24.8
        // maximum luminance, and 18.14 minimum luminance.
        let av1 = codec == 2;
        let chroma = |pair: [u16; 2]| {
            if av1 {
                pair.map(|value| (u32::from(value) * 65536 / 50000).min(65535) as u16)
            } else {
                pair
            }
        };
        Self {
            primaries: metadata.primaries.map(chroma),
            white: chroma(metadata.white),
            maximum: u32::from(metadata.maximum_nits) * if av1 { 256 } else { 10000 },
            minimum: if av1 {
                u32::from(metadata.minimum) * 16384 / 10000
            } else {
                u32::from(metadata.minimum)
            },
            max_cll: metadata.max_cll,
            max_fall: metadata.max_fall,
        }
    }

    pub(super) fn value(self) -> AMFHDRMetadata {
        AMFHDRMetadata {
            redPrimary: self.primaries[0],
            greenPrimary: self.primaries[1],
            bluePrimary: self.primaries[2],
            whitePoint: self.white,
            maxMasteringLuminance: self.maximum,
            minMasteringLuminance: self.minimum,
            maxContentLightLevel: self.max_cll,
            maxFrameAverageLightLevel: self.max_fall,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WriteStage {
    Buffer,
    Property,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct WriteError {
    pub(super) stage: WriteStage,
    pub(super) code: AMF_RESULT,
}

impl WriteError {
    pub(super) fn check(stage: WriteStage, code: AMF_RESULT) -> Result<(), Self> {
        if code == AMF_RESULT_AMF_OK {
            Ok(())
        } else {
            Err(Self { stage, code })
        }
    }

    fn retry(self, failures: u32) -> Retry {
        if self.stage == WriteStage::Property {
            match self.code {
                AMF_RESULT_AMF_NOT_SUPPORTED
                | AMF_RESULT_AMF_NOT_IMPLEMENTED
                | AMF_RESULT_AMF_NOT_FOUND => return Retry::Unsupported,
                AMF_RESULT_AMF_INVALID_ARG
                | AMF_RESULT_AMF_OUT_OF_RANGE
                | AMF_RESULT_AMF_INVALID_FORMAT
                | AMF_RESULT_AMF_INVALID_DATA_TYPE => return Retry::ChangedValue,
                _ => {}
            }
        }
        Retry::After(Duration::from_secs((1u64 << failures.min(5)).min(30)))
    }
}

impl std::fmt::Display for WriteError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "AMF HDR metadata {:?}: AMF error {}",
            self.stage, self.code
        )
    }
}

impl std::error::Error for WriteError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Retry {
    Unsupported,
    ChangedValue,
    After(Duration),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Update {
    Unchanged,
    Deferred,
    Applied,
    Failed {
        error: WriteError,
        retry: Retry,
        report: bool,
    },
}

#[derive(Default)]
pub(super) struct State {
    applied: Option<EncodedMetadata>,
    requested: Option<EncodedMetadata>,
    rejected: Option<EncodedMetadata>,
    unsupported: bool,
    failures: u32,
    retry_at: Option<Instant>,
    next_warning: Option<Instant>,
    pending_idr: bool,
}

impl State {
    pub(super) fn update(
        &mut self,
        requested: EncodedMetadata,
        now: Instant,
        write: impl FnOnce(&EncodedMetadata) -> Result<(), WriteError>,
    ) -> Update {
        self.requested = Some(requested);
        if self.applied == self.requested {
            self.failures = 0;
            self.retry_at = None;
            self.rejected = None;
            return Update::Unchanged;
        }
        if self.unsupported
            || self.rejected == self.requested
            || self.retry_at.is_some_and(|retry_at| now < retry_at)
        {
            return Update::Deferred;
        }
        match write(&requested) {
            Ok(()) => {
                self.applied = Some(requested);
                self.rejected = None;
                self.failures = 0;
                self.retry_at = None;
                self.next_warning = None;
                self.pending_idr = true;
                Update::Applied
            }
            Err(error) => {
                let retry = error.retry(self.failures);
                self.failures = self.failures.saturating_add(1);
                match retry {
                    Retry::Unsupported => self.unsupported = true,
                    Retry::ChangedValue => {
                        self.rejected = Some(requested);
                        self.retry_at = None;
                    }
                    Retry::After(delay) => self.retry_at = Some(now + delay),
                }
                // An unsupported property is reported once even if a preceding
                // transient warning was recent: no later attempt will report it.
                let report = retry == Retry::Unsupported
                    || self
                        .next_warning
                        .is_none_or(|next_warning| now >= next_warning);
                if report {
                    self.next_warning = Some(now + Duration::from_secs(30));
                }
                Update::Failed {
                    error,
                    retry,
                    report,
                }
            }
        }
    }

    pub(super) fn needs_idr(&self) -> bool {
        self.pending_idr
    }

    /// A full queue or failed submission leaves the updated headers pending.
    pub(super) fn submitted(&mut self, result: AMF_RESULT) {
        if result == AMF_RESULT_AMF_OK {
            self.pending_idr = false;
        }
    }
}
