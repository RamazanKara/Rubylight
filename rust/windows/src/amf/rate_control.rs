//! Keep each rate-control limit tied to the bitrate at which its last write
//! succeeded. A target-rate update can succeed while an individual limit fails.
use anyhow::{Context, Result};
use butterpollo_core::encoder_policy::Property;
use butterpollo_core::session::Warnings;

pub(super) fn frame_cap_name(codec: u8) -> &'static str {
    match codec {
        0 => "MaxAUSize",
        1 => "HevcMaxAUSize",
        _ => "Av1MaxCompressedFrameSize",
    }
}

/// Runtime AMF properties are bit budgets; the host and recovery logs use
/// encoded bytes. Keep the observed value separate from the requested one:
/// drivers can reject, clamp, or silently ignore a successful write.
pub(super) struct FrameCap {
    automatic: bool,
    requested: Option<u64>,
    effective: Option<u64>,
}

impl FrameCap {
    pub(super) fn new(automatic: bool) -> Self {
        Self {
            automatic,
            requested: None,
            effective: None,
        }
    }

    pub(super) fn observe(&mut self, bits: Option<i64>) {
        self.effective = bits
            .and_then(|bits| u64::try_from(bits).ok())
            .map(|bits| bits.div_ceil(8));
    }

    pub(super) fn effective(&self) -> Option<u64> {
        self.effective
    }

    pub(super) fn update(
        &mut self,
        codec: u8,
        bytes: u64,
        warnings: &Warnings,
        mut apply: impl FnMut(&str, i64) -> (Result<()>, Option<i64>),
    ) -> Option<u64> {
        if !self.automatic || bytes == 0 {
            return self.effective;
        }
        // First update applies before the first input, even if the initial
        // two-frame property failed. Avoid driver writes for small upward
        // jitter, but immediately honor decreases in the recovery budget.
        // Compare with the last attempt so a refused value is not retried on
        // every frame. A material route change gets another attempt.
        if self.requested.is_some_and(|previous| {
            bytes >= previous && u128::from(bytes - previous) * 10 <= u128::from(previous)
        }) {
            return self.effective;
        }
        self.requested = Some(bytes);
        let bits = bytes.saturating_mul(8).min(i64::MAX as u64) as i64;
        let name = frame_cap_name(codec);
        let (result, actual) = apply(name, bits);
        self.observe(actual);
        let error = match result {
            Err(error) => Some(format!("{error:#}")),
            Ok(()) if actual == Some(bits) => None,
            Ok(()) => Some(format!(
                "requested {bits} bits but driver reports {actual:?}"
            )),
        };
        if let Some(error) = error {
            warnings.set("encoder_frame_size", format!(
                "AMF could not apply the pace-based maximum frame size ({name}: {error}); streaming continues with the driver's effective limit, so recovery frames may be larger. Update the AMD driver or set Maximum frame size to Driver default to opt out."
            ));
        } else {
            warnings.clear("encoder_frame_size");
        }
        self.effective
    }
}

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

/// `value` inside the property's own minimum and maximum.
fn clamp_limit(value: i64, limit: Limit) -> i64 {
    let value = limit.minimum.map_or(value, |minimum| value.max(minimum));
    limit.maximum.map_or(value, |maximum| value.min(maximum))
}

pub(super) struct State {
    requested: u32,
    scale_frame_cap: bool,
    limits: [AppliedLimit; 3],
    /// Peak and buffer are held tight (see [`State::set_tight`]).
    tight: bool,
    /// The peak and buffer values before they were tightened, with the bitrate
    /// they were written for, put back (scaled) when the hold ends.
    saved: [Option<(i64, u32)>; 2],
}

impl State {
    pub(super) fn new(bitrate: u32, scale_frame_cap: bool) -> Self {
        Self {
            requested: bitrate,
            scale_frame_cap,
            limits: std::array::from_fn(|_| AppliedLimit {
                bitrate,
                warning: None,
            }),
            tight: false,
            saved: [None; 2],
        }
    }

    pub(super) fn requested(&self) -> u32 {
        self.requested
    }

    /// While the link is limiting the stream, hold the peak bitrate at the
    /// target and the buffer at one frame's budget, so frame sizes stay smooth
    /// for a bottleneck queue; `false` puts back what was there. Later bitrate
    /// updates keep scaling both with the target, so the ratios hold. A write
    /// that fails leaves that limit as it was and warns like `update`.
    pub(super) fn set_tight(
        &mut self,
        tight: bool,
        codec: u8,
        fps_millihz: u32,
        warnings: &Warnings,
        mut read: impl FnMut(&str) -> Option<Limit>,
        mut set: impl FnMut(&str, i64) -> Result<()>,
    ) {
        if tight == self.tight {
            return;
        }
        self.tight = tight;
        let prefix = match codec {
            0 => "",
            1 => "Hevc",
            _ => "Av1",
        };
        let target_bits = i64::from(self.requested) * 1000;
        let frame_bits = (i128::from(target_bits) * 1000 / i128::from(fps_millihz.max(1))) as i64;
        for (index, suffix) in ["PeakBitrate", "VBVBufferSize"].into_iter().enumerate() {
            let name = format!("{prefix}{suffix}");
            let limit = &mut self.limits[index];
            let Some(current) = read(&name).filter(|current| current.value > 0) else {
                continue;
            };
            let wanted = if tight {
                self.saved[index] = Some((current.value, limit.bitrate));
                clamp_limit(if index == 0 { target_bits } else { frame_bits }, current)
            } else {
                let Some((value, basis)) = self.saved[index].take() else {
                    continue;
                };
                clamp_limit(
                    super::scale_rate_control(value, basis, self.requested),
                    current,
                )
            };
            match set(&name, wanted) {
                Ok(()) => {
                    limit.bitrate = self.requested;
                    limit.warning = None;
                }
                Err(error) => {
                    // What the driver holds is unchanged: nothing to put back.
                    self.saved[index] = None;
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
        for (index, (limit, suffix)) in self.limits.iter_mut().zip(suffixes).enumerate() {
            if index == 2 && !self.scale_frame_cap {
                // The sender's pace can change independently of bitrate.
                // Only set_frame_cap owns the automatic recovery budget.
                // Explicit driver default also forbids writes, even when the
                // driver's default happens to read as a nonzero bit limit.
                continue;
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automatic_caps_use_codec_bit_units_and_filter_only_small_increases() {
        for codec in 0..=2 {
            let warnings = Warnings::default();
            let mut cap = FrameCap::new(true);
            cap.observe(Some(800_000));
            let mut writes = Vec::new();
            for bytes in [100_000, 100_000, 105_000, 110_000, 110_001, 109_999, 90_000] {
                cap.update(codec, bytes, &warnings, |name, bits| {
                    writes.push((name.to_string(), bits));
                    (Ok(()), Some(bits))
                });
            }
            assert_eq!(
                writes.iter().map(|(_, bits)| *bits).collect::<Vec<_>>(),
                [800_000, 880_008, 879_992, 720_000]
            );
            assert!(writes.iter().all(|(name, _)| name == frame_cap_name(codec)));
            assert_eq!(cap.effective(), Some(90_000));
            assert!(warnings.snapshot().is_empty());
        }
    }

    #[test]
    fn explicit_cap_and_driver_default_are_observed_but_never_overridden() {
        for bits in [0, 800_000, 800_001] {
            let warnings = Warnings::default();
            let mut cap = FrameCap::new(false);
            cap.observe(Some(bits));
            let actual = cap.update(0, 450_000, &warnings, |_, _| {
                panic!("explicit profile must win")
            });
            assert_eq!(actual, Some((bits as u64).div_ceil(8)));
            assert!(warnings.snapshot().is_empty());
        }
    }

    #[test]
    fn rejected_or_clamped_caps_report_observed_limits_and_suppress_identical_retries() {
        let warnings = Warnings::default();
        let mut cap = FrameCap::new(true);
        cap.observe(Some(800_000));
        assert_eq!(
            cap.update(1, 450_000, &warnings, |_, _| (
                Err(anyhow::anyhow!("unsupported")),
                Some(800_000)
            )),
            Some(100_000)
        );
        assert!(warnings.snapshot()[0].message.contains("unsupported"));
        assert_eq!(
            cap.update(1, 450_000, &warnings, |_, _| panic!("retry on every frame")),
            Some(100_000)
        );
        // A decrease gets an immediate attempt even below the 10% threshold.
        assert_eq!(
            cap.update(1, 449_999, &warnings, |_, _| (Ok(()), Some(1_600_000))),
            Some(200_000)
        );
        assert!(warnings.snapshot()[0].message.contains("driver reports"));
        // A read failure must not masquerade as the previously applied value.
        assert_eq!(
            cap.update(1, 300_000, &warnings, |_, _| (Ok(()), None)),
            None
        );
        assert!(warnings.snapshot()[0].message.contains("None"));
        assert_eq!(
            cap.update(1, 200_000, &warnings, |_, bits| (Ok(()), Some(bits))),
            Some(200_000)
        );
        assert!(warnings.snapshot().is_empty());
    }

    #[test]
    fn cap_conversion_bounds_values_without_overflow_or_disabling_on_zero() {
        let warnings = Warnings::default();
        let mut cap = FrameCap::new(true);
        cap.observe(Some(800_000));
        assert_eq!(
            cap.update(0, 0, &warnings, |_, _| panic!(
                "zero pace input cannot disable cap"
            )),
            Some(100_000)
        );
        assert_eq!(
            cap.update(0, u64::MAX, &warnings, |_, bits| {
                assert_eq!(bits, i64::MAX);
                (Ok(()), Some(bits))
            }),
            Some((i64::MAX as u64).div_ceil(8))
        );
        cap.observe(Some(-1));
        assert_eq!(cap.effective(), None);
    }
}
