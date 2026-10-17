//! Keep each rate-control limit tied to the bitrate at which its last write
//! succeeded. A target-rate update can succeed while an individual limit fails.
use anyhow::{Context, Result};
use butterpollo_core::encoder_policy::Property;
use butterpollo_core::session::Warnings;

/// Frame caps must not prevent streaming on a driver that cannot apply them.
/// Peak and VBV limits retain their existing strict validation. `apply`
/// verifies both the write and readback, so clamped/ignored caps warn too.
pub(super) fn configure(
    properties: &[Property],
    warnings: &Warnings,
    mut apply: impl FnMut(&Property) -> Result<()>,
) -> Result<()> {
    for property in properties {
        if let Err(error) = apply(property) {
            if property.required {
                return Err(error).with_context(|| format!("AMF setting {}", property.name));
            }
            warnings.set("encoder_frame_size", format!(
                "AMF could not apply the requested maximum frame size ({}: {error:#}); streaming continues with the driver's effective limit, so recovery frames may be larger. Update the AMD driver or set Maximum frame size to Driver default to opt out.",
                property.name,
            ));
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Limit {
    pub(super) value: i64,
    pub(super) minimum: Option<i64>,
    pub(super) maximum: Option<i64>,
}

struct AppliedLimit {
    bitrate: u32,
    warning: Option<String>,
}

pub(super) struct State {
    requested: u32,
    limits: [AppliedLimit; 3],
}

impl State {
    pub(super) fn new(bitrate: u32) -> Self {
        Self {
            requested: bitrate,
            limits: std::array::from_fn(|_| AppliedLimit {
                bitrate,
                warning: None,
            }),
        }
    }

    pub(super) fn update(
        &mut self,
        codec: u8,
        bitrate: u32,
        warnings: &Warnings,
        mut read: impl FnMut(&str) -> Option<Limit>,
        mut set: impl FnMut(&str, i64) -> Result<()>,
    ) {
        if bitrate == self.requested {
            return;
        }
        // Preserve the existing policy: do not retry a refused target on every
        // frame. Another requested bitrate gets its own attempt.
        self.requested = bitrate;
        let prefix = match codec {
            0 => "",
            1 => "Hevc",
            _ => "Av1",
        };
        if let Err(error) = set(&format!("{prefix}TargetBitrate"), i64::from(bitrate) * 1000) {
            warnings.set("encoder_bitrate", format!("AMF rejected the requested bitrate {bitrate} Kbps ({error:#}); the driver kept its previous rate, so the displayed target is not the applied bitrate. Reconnect at the desired bitrate or update the AMD driver."));
            return;
        }
        warnings.clear("encoder_bitrate");
        let suffixes = [
            "PeakBitrate",
            "VBVBufferSize",
            if codec == 2 {
                "MaxCompressedFrameSize"
            } else {
                "MaxAUSize"
            },
        ];
        for (limit, suffix) in self.limits.iter_mut().zip(suffixes) {
            let name = format!("{prefix}{suffix}");
            let Some(current) = read(&name).filter(|current| current.value > 0) else {
                // Unsupported/disabled optional properties remain nonfatal.
                // A missing read after a failed write is not evidence that the
                // prior problem healed, and must not erase its warning/basis.
                if limit.warning.is_some() {
                    limit.warning = Some(format!(
                        "AMF {name} cannot be verified after its rejected update; its rate-control limit may still exceed the expected frame budget."
                    ));
                }
                continue;
            };
            let mut scaled = super::scale_rate_control(current.value, limit.bitrate, bitrate);
            if let Some(minimum) = current.minimum {
                scaled = scaled.max(minimum);
            }
            if let Some(maximum) = current.maximum {
                scaled = scaled.min(maximum);
            }
            match set(&name, scaled) {
                Ok(()) => {
                    limit.bitrate = bitrate;
                    limit.warning = None;
                }
                Err(error) => {
                    limit.warning = Some(format!(
                        "AMF {name} update rejected ({error:#}); the driver retained its old rate-control limit and may exceed the expected frame budget."
                    ));
                }
            }
        }
        let messages: Vec<_> = self
            .limits
            .iter()
            .filter_map(|limit| limit.warning.as_deref())
            .collect();
        if messages.is_empty() {
            warnings.clear("encoder_buffer");
        } else {
            warnings.set(
                "encoder_buffer",
                format!(
                    "{} Reconnect at the desired bitrate or update the AMD driver.",
                    messages.join(" ")
                ),
            );
        }
    }
}
