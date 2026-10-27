//! Encoder focus for one stream (docs/encoder-focus-design.md, section 7):
//! resolve the requested mode against what the session and the startup check
//! allow, and hand the encoder new settings only when they change.

use butterpollo_core::config::Config;
use butterpollo_core::encoder_focus::{self, Context, FocusState, Levels, Requested, Resolved};
use butterpollo_core::session::Warnings;
use butterpollo_windows::compute::FocusSettings;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU8, Ordering};

pub(crate) struct Focus {
    context: Context,
    levels: Levels,
    /// The requested value last resolved.
    seen: Option<u8>,
    resolved: Resolved,
    /// What the current encoder has; `None` until it gets its first.
    applied: Option<Option<FocusSettings>>,
    /// The encoder refused a map: off until the stream ends.
    refused: bool,
}

/// AMF pre-analysis as `encoder_policy::amf` turns it on.
pub(crate) fn preanalysis(config: &Config, ten_bit: bool) -> bool {
    !ten_bit
        && (config.boolean("amd_preanalysis", false)
            || matches!(config.get("amd_rc", ""), "qvbr" | "hqvbr" | "hqcbr"))
}

impl Focus {
    /// Reads `encoder_focus` and `encoder_focus_levels` into `requested`. An
    /// unknown value is a WARN and the session warning, and focus stays off.
    pub(crate) fn start(
        config: &Config,
        context: Context,
        requested: &AtomicU8,
        state: &Mutex<FocusState>,
        warnings: &Warnings,
    ) -> Self {
        let levels = config
            .values
            .get("encoder_focus_levels")
            .map(|value| Levels::parse(value));
        let parsed =
            Requested::parse(config.get("encoder_focus", "off")).and_then(
                |requested| match &levels {
                    Some(Err(error)) => Err(error.clone()),
                    _ => Ok(requested),
                },
            );
        let mut focus = Self {
            context,
            levels: levels.and_then(Result::ok).unwrap_or_default(),
            seen: None,
            resolved: Resolved::Off { reason: None },
            applied: None,
            refused: false,
        };
        match parsed {
            Ok(value) => requested.store(value.wire(), Ordering::Relaxed),
            Err(error) => {
                requested.store(Requested::Off.wire(), Ordering::Relaxed);
                focus.seen = Some(Requested::Off.wire());
                let reason = encoder_focus::Reason::InvalidValue;
                warnings.set("encoder_focus", format!("Encoder focus is off: {error}"));
                *state.lock().unwrap() = FocusState {
                    effective: None,
                    reason: Some(reason),
                    message: Some(error),
                };
            }
        }
        focus
    }
    /// Resolve again when `requested` changed. A mode that cannot run is a
    /// WARN and the session warning `encoder_focus`; focus stays off.
    pub(crate) fn update(
        &mut self,
        requested: &AtomicU8,
        state: &Mutex<FocusState>,
        warnings: &Warnings,
    ) {
        let wire = requested.load(Ordering::Relaxed);
        if self.refused || self.seen == Some(wire) {
            return;
        }
        self.seen = Some(wire);
        let wanted = Requested::from_wire(wire).unwrap_or(Requested::Off);
        self.resolved = encoder_focus::resolve(wanted, &self.context);
        let (effective, reason) = match &self.resolved {
            Resolved::On(mode) => (Some(*mode), None),
            Resolved::Off { reason } => (None, *reason),
        };
        let message = reason.map(|reason| match (&self.context.support, reason) {
            (encoder_focus::Support::Unsupported { message, .. }, _)
                if !matches!(
                    reason,
                    encoder_focus::Reason::Pyrowave
                        | encoder_focus::Reason::Packed444
                        | encoder_focus::Reason::D3d12ConversionUnavailable
                ) =>
            {
                message.clone()
            }
            _ => reason.message().to_string(),
        });
        match &message {
            Some(message) => warnings.set(
                "encoder_focus",
                format!(
                    "Encoder focus {} cannot run, so it is off: {message}",
                    wanted.as_str()
                ),
            ),
            None => warnings.clear("encoder_focus"),
        }
        tracing::info!(
            requested = wanted.as_str(),
            effective = effective.map_or("off", |mode| mode.as_str()),
            reason = reason.map(|reason| reason.wire()),
            codec = self.context.codec,
            preanalysis = self.context.preanalysis,
            levels = %self.levels,
            "encoder focus"
        );
        *state.lock().unwrap() = FocusState {
            effective,
            reason,
            message,
        };
    }
    /// The settings for this frame; `centre` follows the cursor.
    pub(crate) fn settings(
        &self,
        cursor: impl FnOnce() -> Option<(i32, i32)>,
    ) -> Option<FocusSettings> {
        match self.resolved {
            Resolved::On(mode) => Some(FocusSettings {
                mode,
                levels: self.levels,
                cursor: (mode == encoder_focus::Mode::Centre).then(cursor).flatten(),
            }),
            Resolved::Off { .. } => None,
        }
    }
    pub(crate) fn on(&self) -> bool {
        matches!(self.resolved, Resolved::On(_))
    }
    /// Hand `wanted` to the encoder through `set` if it differs from what the
    /// encoder has. An error leaves the encoder's value unknown.
    pub(crate) fn apply(
        &mut self,
        wanted: Option<FocusSettings>,
        set: impl FnOnce(Option<FocusSettings>) -> anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        if self.applied == Some(wanted) {
            return Ok(());
        }
        self.applied = None;
        set(wanted)?;
        self.applied = Some(wanted);
        Ok(())
    }
    /// A recreated encoder starts without a map.
    pub(crate) fn encoder_recreated(&mut self) {
        self.applied = None;
    }
    /// The encoder refused the map: off for the rest of the stream, loudly.
    pub(crate) fn failed(
        &mut self,
        error: &anyhow::Error,
        state: &Mutex<FocusState>,
        warnings: &Warnings,
    ) {
        let message = format!("{error:#}");
        warnings.set(
            "encoder_focus",
            format!("Encoder focus failed, so it is off: {message}"),
        );
        self.resolved = Resolved::Off { reason: None };
        self.refused = true;
        *state.lock().unwrap() = FocusState {
            effective: None,
            reason: None,
            message: Some(message),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use encoder_focus::{Mode, Reason, Support};

    fn context(support: Support) -> Context {
        Context {
            codec: 1,
            ten_bit: false,
            pyrowave: false,
            packed_444: false,
            d3d12: true,
            preanalysis: false,
            support,
        }
    }
    fn start(config: &str, support: Support) -> (Focus, AtomicU8, Mutex<FocusState>, Warnings) {
        let (requested, state, warnings) =
            (AtomicU8::new(0), Mutex::default(), Warnings::default());
        let config = Config::parse(config).unwrap();
        let mut focus = Focus::start(&config, context(support), &requested, &state, &warnings);
        focus.update(&requested, &state, &warnings);
        (focus, requested, state, warnings)
    }

    #[test]
    fn a_verified_codec_runs_the_requested_mode() {
        let (focus, _, state, warnings) =
            start("encoder_focus=static", Support::Verified { delta: 10.0 });
        assert!(focus.on());
        assert_eq!(state.lock().unwrap().effective, Some(Mode::Static));
        assert!(warnings.snapshot().is_empty());
    }
    #[test]
    fn unset_stays_off_without_a_warning() {
        let (focus, requested, state, warnings) = start("", Support::Verified { delta: 10.0 });
        assert!(!focus.on());
        assert_eq!(requested.load(Ordering::Relaxed), Requested::Off.wire());
        assert_eq!(*state.lock().unwrap(), FocusState::default());
        assert!(warnings.snapshot().is_empty());
    }
    #[test]
    fn auto_is_not_a_value() {
        let (focus, _, state, warnings) =
            start("encoder_focus=auto", Support::Verified { delta: 10.0 });
        assert!(!focus.on());
        assert_eq!(state.lock().unwrap().reason, Some(Reason::InvalidValue));
        assert_eq!(warnings.snapshot()[0].code, "encoder_focus");
    }
    #[test]
    fn a_mode_the_check_refused_warns_with_its_message() {
        let support = Support::Unsupported {
            reason: Reason::CodecUnsupported,
            message: "the focus check failed: timeout".into(),
        };
        let (focus, _, state, warnings) = start("encoder_focus=centre", support);
        assert!(!focus.on());
        let state = state.lock().unwrap().clone();
        assert_eq!(state.reason, Some(Reason::CodecUnsupported));
        assert_eq!(
            state.message.as_deref(),
            Some("the focus check failed: timeout")
        );
        let warning = &warnings.snapshot()[0];
        assert_eq!(warning.code, "encoder_focus");
        assert!(warning.message.contains("timeout"), "{}", warning.message);
    }
    #[test]
    fn an_unprobed_codec_and_an_unknown_value_warn() {
        let (focus, _, state, warnings) = start("encoder_focus=static", Support::NotProbed);
        assert!(!focus.on());
        assert_eq!(state.lock().unwrap().reason, Some(Reason::NotProbed));
        assert_eq!(warnings.snapshot().len(), 1);
        let (focus, requested, state, warnings) =
            start("encoder_focus=bogus", Support::Verified { delta: 10.0 });
        assert!(!focus.on());
        assert_eq!(requested.load(Ordering::Relaxed), Requested::Off.wire());
        assert_eq!(state.lock().unwrap().reason, Some(Reason::InvalidValue));
        assert_eq!(warnings.snapshot()[0].code, "encoder_focus");
    }
    #[test]
    fn the_encoder_gets_settings_only_when_they_change() {
        let (mut focus, requested, state, warnings) =
            start("encoder_focus=centre", Support::Verified { delta: 10.0 });
        let mut calls = Vec::new();
        let mut frame = |focus: &mut Focus, cursor: Option<(i32, i32)>| {
            let wanted = focus.settings(|| cursor);
            focus
                .apply(wanted, |settings| {
                    calls.push(settings.and_then(|settings| settings.cursor));
                    Ok(())
                })
                .unwrap();
        };
        frame(&mut focus, Some((10, 10)));
        frame(&mut focus, Some((10, 10)));
        frame(&mut focus, Some((20, 10)));
        focus.encoder_recreated();
        frame(&mut focus, Some((20, 10)));
        requested.store(Requested::Off.wire(), Ordering::Relaxed);
        focus.update(&requested, &state, &warnings);
        frame(&mut focus, Some((20, 10)));
        frame(&mut focus, Some((30, 10)));
        assert_eq!(
            calls,
            [Some((10, 10)), Some((20, 10)), Some((20, 10)), None]
        );
    }
    #[test]
    fn a_refused_map_turns_focus_off_loudly() {
        let (mut focus, requested, state, warnings) =
            start("encoder_focus=static", Support::Verified { delta: 10.0 });
        let error = focus
            .apply(focus.settings(|| None), |_| {
                anyhow::bail!("no D3D12 conversion")
            })
            .unwrap_err();
        focus.failed(&error, &state, &warnings);
        focus.update(&requested, &state, &warnings);
        assert!(!focus.on());
        assert_eq!(
            state.lock().unwrap().message.as_deref(),
            Some("no D3D12 conversion")
        );
        assert!(
            warnings.snapshot()[0]
                .message
                .contains("no D3D12 conversion")
        );
    }
}
