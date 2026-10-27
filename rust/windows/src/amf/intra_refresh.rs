//! Apply the optional HEVC refresh workaround as a verified pair. A driver
//! that rejects either half keeps the previous policy, not a partial override.
//! Negotiated AV1 refresh is used only where the driver offers it.
use anyhow::{Context, Result, bail};
use butterpollo_core::{
    encoder_policy::{
        AMF_AV1_INTRA_REFRESH_MODE, AMF_AV1_INTRA_REFRESH_STRIPES, AmfHevcIntraRefresh,
    },
    rtsp::Negotiated,
    session::Warnings,
};

pub(super) const GOP: &str = "HevcGOPSize";
pub(super) const CTBS: &str = "HevcIntraRefreshCTBsNumberPerSlot";
pub(super) const AV1_MODE: &str = "Av1IntraRefreshMode";
pub(super) const AV1_STRIPES: &str = "Av1IntraRefreshNumOfStripes";

/// An integer property's bounds as AMF describes it before Init.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Range {
    pub minimum: Option<i64>,
    pub maximum: Option<i64>,
}

/// Why the driver cannot take the negotiated AV1 refresh; `None` when it
/// describes both properties with room for the policy's values. Asking
/// anyway fails the encoder, because the policy requires both.
pub(super) fn av1_unsupported(mode: Option<Range>, stripes: Option<Range>) -> Option<String> {
    let check = |name: &str, range: Option<Range>, value: i64| {
        let Some(range) = range else {
            return Some(format!("the driver has no {name}"));
        };
        let fits = range.minimum.is_none_or(|minimum| value >= minimum)
            && range.maximum.is_none_or(|maximum| value <= maximum);
        (!fits).then(|| {
            format!(
                "{name} accepts {:?} to {:?}, not {value}",
                range.minimum, range.maximum
            )
        })
    };
    check(AV1_MODE, mode, AMF_AV1_INTRA_REFRESH_MODE)
        .or_else(|| check(AV1_STRIPES, stripes, AMF_AV1_INTRA_REFRESH_STRIPES))
}

/// The stream to configure instead when the client negotiated AV1 refresh
/// and the driver cannot take it: the same stream without the request, so
/// it recovers as one that never asked. `None` keeps the stream. Encoders
/// are rebuilt on resets and codec changes; the log line comes once.
pub(super) fn av1_without_refresh(
    stream: &Negotiated,
    describe: impl FnOnce() -> (Option<Range>, Option<Range>),
) -> Option<Negotiated> {
    if stream.codec != 2 || !stream.intra_refresh {
        return None;
    }
    let (mode, stripes) = describe();
    let reason = av1_unsupported(mode, stripes)?;
    static LOGGED: std::sync::Once = std::sync::Once::new();
    LOGGED.call_once(|| {
        tracing::warn!(
            "AMF cannot refresh AV1 pictures on this driver ({reason}); the client asked for intra refresh, so lost frames are recovered with a keyframe or a long-term reference instead. Update the AMD driver, or use HEVC if recovery stutters"
        )
    });
    Some(Negotiated {
        intra_refresh: false,
        ..stream.clone()
    })
}

/// Init may change a preset. A refresh the driver dropped is reported, not
/// assumed; recovery then rests on the keyframes a client requests.
pub(super) fn av1_validate(mode: Option<i64>, stripes: Option<i64>, warnings: &Warnings) {
    if mode == Some(AMF_AV1_INTRA_REFRESH_MODE) && stripes == Some(AMF_AV1_INTRA_REFRESH_STRIPES) {
        tracing::info!(
            mode = AMF_AV1_INTRA_REFRESH_MODE,
            stripes = AMF_AV1_INTRA_REFRESH_STRIPES,
            "AMF AV1 intra-refresh settings verified"
        );
        return;
    }
    let message = format!(
        "AMF AV1 intra refresh did not hold after encoder initialization (driver reports mode {mode:?}, {stripes:?} stripes); lost frames are recovered with keyframes. Update the AMD driver, or use HEVC if recovery stutters"
    );
    tracing::warn!("{message}");
    warnings.set("encoder_intra_refresh", message);
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
    fn av1_refresh_is_asked_for_only_where_the_driver_describes_room_for_it() {
        let range = |minimum, maximum| Some(Range { minimum, maximum });
        for (mode, stripes) in [
            (range(Some(0), Some(2)), range(Some(1), Some(1000))),
            // A description without integer bounds leaves the check to the
            // readback after Init.
            (range(None, None), range(None, None)),
            (range(Some(0), None), range(None, Some(300))),
        ] {
            assert_eq!(av1_unsupported(mode, stripes), None);
        }
        for (mode, stripes, named) in [
            (None, range(Some(1), Some(1000)), AV1_MODE),
            (range(Some(0), Some(2)), None, AV1_STRIPES),
            (
                range(Some(0), Some(1)),
                range(Some(1), Some(1000)),
                AV1_MODE,
            ),
            (
                range(Some(0), Some(2)),
                range(Some(1), Some(255)),
                AV1_STRIPES,
            ),
            (range(Some(0), Some(2)), range(Some(301), None), AV1_STRIPES),
        ] {
            let reason = av1_unsupported(mode, stripes).expect("unsupported");
            assert!(reason.contains(named), "{reason}");
        }
    }

    #[test]
    fn a_driver_without_av1_refresh_gets_the_stream_that_never_asked() -> Result<()> {
        use butterpollo_core::encoder_policy::{amf, amf_ltr_frames};
        let asked = Negotiated {
            codec: 2,
            width: 2560,
            height: 1440,
            intra_refresh: true,
            ..Default::default()
        };
        let options = Default::default();
        let supported = || {
            let range = Some(Range {
                minimum: Some(0),
                maximum: Some(1000),
            });
            (range, range)
        };
        assert!(av1_without_refresh(&asked, supported).is_none());
        let stream = av1_without_refresh(&asked, || (None, None)).expect("fallback");
        let never_asked = Negotiated {
            intra_refresh: false,
            ..asked.clone()
        };
        assert_eq!(format!("{stream:?}"), format!("{never_asked:?}"));
        let names = |stream: &Negotiated| -> Result<Vec<String>> {
            Ok(amf(&options, stream)?.into_iter().map(|p| p.name).collect())
        };
        assert_eq!(names(&stream)?, names(&never_asked)?);
        assert!(
            !names(&stream)?
                .iter()
                .any(|name| name.contains("IntraRefresh"))
        );
        assert_eq!(
            amf_ltr_frames(&options, &stream),
            amf_ltr_frames(&options, &never_asked)
        );
        // Other codecs and streams without the request are not described.
        for stream in [
            never_asked.clone(),
            Negotiated {
                codec: 1,
                ..asked.clone()
            },
        ] {
            assert!(
                av1_without_refresh(&stream, || panic!("not an AV1 refresh request")).is_none()
            );
        }
        Ok(())
    }

    #[test]
    fn av1_refresh_dropped_after_init_warns_the_session() {
        let warnings = Warnings::default();
        av1_validate(
            Some(AMF_AV1_INTRA_REFRESH_MODE),
            Some(AMF_AV1_INTRA_REFRESH_STRIPES),
            &warnings,
        );
        assert!(warnings.snapshot().is_empty());
        for (mode, stripes) in [
            (Some(0), Some(AMF_AV1_INTRA_REFRESH_STRIPES)),
            (Some(AMF_AV1_INTRA_REFRESH_MODE), Some(34)),
            (None, None),
        ] {
            let warnings = Warnings::default();
            av1_validate(mode, stripes, &warnings);
            assert_eq!(warnings.snapshot()[0].code, "encoder_intra_refresh");
        }
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
