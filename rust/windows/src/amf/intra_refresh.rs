//! Apply the optional HEVC refresh workaround as a verified pair. A driver
//! that rejects either half keeps the previous policy, not a partial override.
//! AV1 never refreshes (`encoder_policy::amf_intra_refresh`); only the native
//! measurement asks AMF for it.
use anyhow::{Context, Result, bail};
use butterpollo_core::{
    encoder_policy::{AmfHevcIntraRefresh, Property},
    rtsp::Negotiated,
    session::Warnings,
};

pub(super) const GOP: &str = "HevcGOPSize";
pub(super) const CTBS: &str = "HevcIntraRefreshCTBsNumberPerSlot";
#[cfg(test)]
pub(super) const AV1_MODE: &str = "Av1IntraRefreshMode";
#[cfg(test)]
pub(super) const AV1_STRIPES: &str = "Av1IntraRefreshNumOfStripes";
/// The AV1 refresh the host asked for before it was turned off: AMF's
/// continuous mode, one stripe of the picture per frame over 300 frames.
#[cfg(test)]
pub(super) const AV1_REFRESH: [(&str, i64); 2] = [(AV1_MODE, 2), (AV1_STRIPES, 300)];

#[cfg(test)]
thread_local! {
    static FORCE_AV1: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Test-only: encoders this thread creates for a negotiated AV1 refresh ask
/// AMF for it anyway, so the measurement that turned it off stays repeatable.
#[cfg(test)]
pub(super) fn force_av1(force: bool) {
    FORCE_AV1.set(force);
}

/// Whether this AV1 stream asks AMF for the refresh it negotiated: never
/// outside the measurement's `force_av1`.
pub(super) fn av1_forced(stream: &Negotiated) -> bool {
    #[cfg(test)]
    {
        stream.codec == 2 && stream.intra_refresh && FORCE_AV1.get()
    }
    #[cfg(not(test))]
    {
        let _ = stream;
        false
    }
}

/// The AMF properties of a forced AV1 refresh; none otherwise.
pub(super) fn av1_forced_properties(stream: &Negotiated) -> Vec<Property> {
    #[cfg(test)]
    if av1_forced(stream) {
        return AV1_REFRESH
            .into_iter()
            .map(|(name, value)| Property {
                name: name.into(),
                value: butterpollo_core::encoder_policy::Value::Integer(value),
                required: true,
            })
            .collect();
    }
    let _ = stream;
    Vec::new()
}

/// Only an Init failure after the verified override permits a fresh baseline
/// attempt. Other encoder failures keep their original diagnostics and policy.
#[derive(Debug)]
pub(super) struct InitializationFailed;

impl std::fmt::Display for InitializationFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AMF initialization failed with the HEVC intra-refresh workaround")
    }
}
impl std::error::Error for InitializationFailed {}

pub(super) fn initialize<T>(
    mut attempt: impl FnMut(bool) -> Result<T>,
) -> Result<(T, Option<String>)> {
    match attempt(true) {
        Ok(encoder) => Ok((encoder, None)),
        Err(error) if error.downcast_ref::<InitializationFailed>().is_some() => {
            let reason = format!("{error:#}");
            // The failed attempt owns and releases its component. Never call
            // Init again on a component whose initialization failed.
            let encoder =
                attempt(false).with_context(|| format!("AMF baseline retry after {reason}"))?;
            Ok((encoder, Some(reason)))
        }
        Err(error) => Err(error),
    }
}

fn apply(
    name: &str,
    value: i64,
    set: &mut impl FnMut(&str, i64) -> Result<Option<i64>>,
) -> Result<()> {
    let actual = set(name, value).with_context(|| format!("AMF property {name}"))?;
    if actual == Some(value) {
        Ok(())
    } else {
        bail!("AMF {name} requested {value}, driver reports {actual:?}")
    }
}

fn fallback(
    request: AmfHevcIntraRefresh,
    error: &str,
    warnings: &Warnings,
    mut set: impl FnMut(&str, i64) -> Result<Option<i64>>,
) -> Result<()> {
    // Restore the infinite GOP first: a partial workaround must not leave
    // periodic large keyframes behind. Preflight established readable baseline
    // properties, so rejected/unknown rollback must not silently continue.
    apply(GOP, 0, &mut set).context("AMF HEVC intra-refresh fallback GOP")?;
    apply(CTBS, request.fallback_ctbs_per_slot, &mut set)
        .context("AMF HEVC intra-refresh fallback CTBs")?;
    warnings.set(
        "encoder_intra_refresh",
        format!(
            "AMF could not establish the HEVC intra-refresh workaround ({error}); using the previous refresh policy with an infinite GOP. Xbox colour artefacts may persist. Update the AMD driver or reconnect the client if needed."
        ),
    );
    Ok(())
}

pub(super) fn configure(
    request: Option<AmfHevcIntraRefresh>,
    baseline_gop: Option<i64>,
    baseline_ctbs: Option<i64>,
    warnings: &Warnings,
    mut set: impl FnMut(&str, i64) -> Result<Option<i64>>,
) -> Result<Option<AmfHevcIntraRefresh>> {
    let Some(request) = request else {
        return Ok(None);
    };
    // Older drivers may support the original setters without GOP readback.
    // Do not touch that working policy unless rollback can also be verified.
    if baseline_gop != Some(0) || baseline_ctbs != Some(request.fallback_ctbs_per_slot) {
        warnings.set(
            "encoder_intra_refresh",
            format!(
                "AMF HEVC intra-refresh workaround skipped because the previous settings cannot be verified (GOP={baseline_gop:?}, CTBs={baseline_ctbs:?}); existing encoder settings remain. Xbox colour artefacts may persist. Update the AMD driver or reconnect the client if needed."
            ),
        );
        return Ok(None);
    }
    let result = apply(GOP, request.gop_frames, &mut set)
        .and_then(|()| apply(CTBS, request.ctbs_per_slot, &mut set));
    match result {
        Ok(()) => Ok(Some(request)),
        Err(error) => {
            fallback(request, &format!("{error:#}"), warnings, set)?;
            Ok(None)
        }
    }
}

pub(super) fn validate(
    request: Option<AmfHevcIntraRefresh>,
    actual_gop: Option<i64>,
    actual_ctbs: Option<i64>,
    warnings: &Warnings,
    set: impl FnMut(&str, i64) -> Result<Option<i64>>,
) -> Result<()> {
    let Some(request) = request else {
        return Ok(());
    };
    // Init may change a preset. Do not report the workaround as configured
    // merely because SetProperty succeeded before initialization.
    if actual_gop == Some(request.gop_frames) && actual_ctbs == Some(request.ctbs_per_slot) {
        warnings.clear("encoder_intra_refresh");
        tracing::info!(
            gop_frames = request.gop_frames,
            ctbs_per_slot = request.ctbs_per_slot,
            "AMF HEVC intra-refresh settings verified; startup and recovery keyframes remain enabled"
        );
        Ok(())
    } else {
        fallback(
            request,
            &format!("initialized driver reports GOP={actual_gop:?}, CTBs={actual_ctbs:?}"),
            warnings,
            set,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> AmfHevcIntraRefresh {
        AmfHevcIntraRefresh {
            gop_frames: 120,
            ctbs_per_slot: 60,
            fallback_ctbs_per_slot: 7,
        }
    }

    #[test]
    fn av1_refresh_is_asked_for_only_by_the_forced_measurement() {
        let asked = Negotiated {
            codec: 2,
            width: 2560,
            height: 1440,
            intra_refresh: true,
            ..Default::default()
        };
        let others = [
            Negotiated {
                intra_refresh: false,
                ..asked.clone()
            },
            Negotiated {
                codec: 1,
                ..asked.clone()
            },
        ];
        assert!(!av1_forced(&asked));
        assert!(av1_forced_properties(&asked).is_empty());
        force_av1(true);
        assert!(av1_forced(&asked));
        let forced: Vec<_> = av1_forced_properties(&asked)
            .into_iter()
            .map(|p| (p.name, p.value, p.required))
            .collect();
        for stream in &others {
            assert!(!av1_forced(stream));
            assert!(av1_forced_properties(stream).is_empty());
        }
        force_av1(false);
        assert_eq!(
            forced,
            AV1_REFRESH.map(|(name, value)| (
                name.to_string(),
                butterpollo_core::encoder_policy::Value::Integer(value),
                true
            ))
        );
        assert!(!av1_forced(&asked));
    }

    #[test]
    fn only_workaround_initialization_failure_retries_once_without_override() -> Result<()> {
        for first_result in 0..3 {
            let mut attempts = Vec::new();
            let result = initialize(|enabled| {
                attempts.push(enabled);
                if enabled {
                    match first_result {
                        1 => {
                            return Err(
                                anyhow::anyhow!("unsupported Init").context(InitializationFailed)
                            );
                        }
                        2 => bail!("unrelated encoder failure"),
                        _ => {}
                    }
                }
                Ok(42)
            });
            match first_result {
                0 => {
                    assert_eq!(result?, (42, None));
                    assert_eq!(attempts, [true]);
                }
                1 => {
                    let (value, reason) = result?;
                    assert_eq!(value, 42);
                    assert!(reason.unwrap().contains("unsupported Init"));
                    assert_eq!(attempts, [true, false]);
                }
                _ => {
                    assert!(result.is_err());
                    assert_eq!(attempts, [true]);
                }
            }
        }
        let mut attempts = Vec::new();
        let result: Result<((), Option<String>)> = initialize(|enabled| {
            attempts.push(enabled);
            Err(anyhow::anyhow!("Init failed").context(InitializationFailed))
        });
        assert!(result.is_err());
        assert_eq!(attempts, [true, false]);
        Ok(())
    }

    #[test]
    fn configured_pair_requires_initialized_readback_before_success() -> Result<()> {
        let warnings = Warnings::default();
        let mut calls = Vec::new();
        let pending = configure(
            Some(request()),
            Some(0),
            Some(7),
            &warnings,
            |name, value| {
                calls.push((name.to_string(), value));
                Ok(Some(value))
            },
        )?;
        assert_eq!(pending, Some(request()));
        assert_eq!(calls, [(GOP.to_string(), 120), (CTBS.to_string(), 60)]);
        validate(pending, Some(120), Some(60), &warnings, |_, _| {
            panic!("verified Init must not rewrite settings")
        })?;
        assert!(warnings.snapshot().is_empty());
        Ok(())
    }

    #[test]
    fn missing_baseline_readback_keeps_the_working_driver_untouched() -> Result<()> {
        for baseline in [
            (None, Some(7)),
            (Some(0), None),
            (Some(300), Some(7)),
            (Some(0), Some(0)),
        ] {
            let warnings = Warnings::default();
            assert_eq!(
                configure(
                    Some(request()),
                    baseline.0,
                    baseline.1,
                    &warnings,
                    |_, _| { panic!("unverifiable baseline must remain untouched") }
                )?,
                None
            );
            assert_eq!(warnings.snapshot()[0].code, "encoder_intra_refresh");
        }
        let warnings = Warnings::default();
        assert_eq!(
            configure(None, None, None, &warnings, |_, _| panic!(
                "no HEVC request"
            ))?,
            None
        );
        validate(None, None, None, &warnings, |_, _| {
            panic!("no HEVC request")
        })?;
        assert!(warnings.snapshot().is_empty());
        Ok(())
    }

    #[test]
    fn rejected_or_ignored_pair_restores_both_previous_settings() -> Result<()> {
        // Exercise refusal, silent clamping and lost readback at either stage.
        for failed_name in [GOP, CTBS] {
            for failure in 0..3 {
                let warnings = Warnings::default();
                let mut calls = Vec::new();
                let pending = configure(
                    Some(request()),
                    Some(0),
                    Some(7),
                    &warnings,
                    |name, value| {
                        calls.push((name.to_string(), value));
                        if name == failed_name && matches!(value, 120 | 60) {
                            return match failure {
                                0 => anyhow::bail!("unsupported property value"),
                                1 => Ok(Some(1)),
                                _ => Ok(None),
                            };
                        }
                        Ok(Some(value))
                    },
                )?;
                assert_eq!(pending, None);
                assert!(calls.ends_with(&[(GOP.to_string(), 0), (CTBS.to_string(), 7)]));
                assert_eq!(warnings.snapshot()[0].code, "encoder_intra_refresh");
            }
        }
        Ok(())
    }

    #[test]
    fn init_changes_restore_previous_policy_and_unrestorable_state_is_fatal() -> Result<()> {
        for actual in [
            (Some(0), Some(60)),
            (Some(120), Some(7)),
            (None, Some(60)),
            (Some(120), None),
        ] {
            let warnings = Warnings::default();
            let mut calls = Vec::new();
            validate(
                Some(request()),
                actual.0,
                actual.1,
                &warnings,
                |name, value| {
                    calls.push((name.to_string(), value));
                    Ok(Some(value))
                },
            )?;
            assert_eq!(calls, [(GOP.to_string(), 0), (CTBS.to_string(), 7)]);
            assert_eq!(warnings.snapshot()[0].code, "encoder_intra_refresh");
        }
        for failed_name in [GOP, CTBS] {
            let warnings = Warnings::default();
            let result = validate(
                Some(request()),
                Some(0),
                Some(7),
                &warnings,
                |name, value| {
                    if name == failed_name {
                        bail!("rollback rejected")
                    }
                    Ok(Some(value))
                },
            );
            assert!(result.is_err());
            // Do not announce a fallback that the encoder did not establish.
            assert!(warnings.snapshot().is_empty());
        }
        Ok(())
    }
}
