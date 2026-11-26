//! Native API endpoints (`GET /audio`, `POST /audio/settings`) for couch audio DSP.

use butterpollo_media_policy::audio_dsp::{Controls, Inactive, Key, Settings};
use rubylight_native_api::audio::{self as api, Audio, AudioWrite};
use rubylight_native_api::error::{ApiError, ErrorCode};
use rubylight_native_api::radeon::{ControlState, Kind, Setting, Value};

/// The native API identifier for an audio DSP key.
pub fn key_id(key: Key) -> &'static str {
    match key {
        Key::NightMode => api::NIGHT_MODE,
        Key::HeadphoneSurround => api::HEADPHONE_SURROUND,
    }
}

/// The native API reason code for an inactive audio DSP setting.
pub fn inactive_code(reason: Inactive) -> &'static str {
    match reason {
        Inactive::ClientSurround => api::reason::CLIENT_SURROUND,
        Inactive::GameOnlyAudio => api::reason::GAME_ONLY_AUDIO,
        Inactive::StereoEndpoint => api::reason::STEREO_ENDPOINT,
        Inactive::Muted => api::reason::MUTED,
        Inactive::NextStream => api::reason::NEXT_STREAM,
    }
}

fn setting(key: Key) -> Setting {
    Setting {
        id: key_id(key).into(),
        group: api::GROUP.into(),
        kind: Kind::Toggle,
        state: ControlState::Configurable,
        value: None,
        range: None,
        options: Vec::new(),
        unit: None,
        restore: None,
        reason: None,
        message: None,
    }
}

/// `GET /audio`: the caller's session `controls`, or `None` without a
/// session. `base` is the choice a stream starts with (`audio_dsp::resolve`). A
/// setting that doesn't apply stays writable: its reason is a note.
pub fn state(controls: Option<&Controls>, base: Settings) -> Audio {
    let Some(controls) = controls else {
        return Audio {
            revision: 0,
            settings: Key::ALL
                .into_iter()
                .map(|key| Setting {
                    state: ControlState::TemporarilyUnavailable,
                    reason: Some(api::reason::NO_SESSION.into()),
                    message: Some("Start a stream to change this.".into()),
                    ..setting(key)
                })
                .collect(),
        };
    };
    let settings = controls.settings(base);
    Audio {
        revision: controls.revision(),
        settings: Key::ALL
            .into_iter()
            .map(|key| {
                let reason = controls.reason(key);
                Setting {
                    value: Some(Value::Bool(key.of(settings))),
                    reason: reason.map(inactive_code).map(str::to_owned),
                    message: reason.map(|reason| reason.message(key).into()),
                    ..setting(key)
                }
            })
            .collect(),
    }
}

/// `POST /audio/settings` for the owner of `controls`: checks every change,
/// then applies them all to this session from the next packet. The session
/// and owner refusals are the caller's. The refusal is boxed: an error body with its optional
/// title, next step and technical detail is large.
pub fn apply(
    controls: &Controls,
    base: Settings,
    write: &AudioWrite,
) -> Result<Audio, Box<ApiError>> {
    let refuse =
        |code, id: &str, message: String| Box::new(ApiError::new(code, message).for_setting(id));
    let mut changes = Vec::with_capacity(write.changes.len());
    for change in &write.changes {
        let key = Key::ALL
            .into_iter()
            .find(|key| key_id(*key) == change.id)
            .ok_or_else(|| {
                refuse(
                    ErrorCode::NotFound,
                    &change.id,
                    format!("This host has no audio setting {}.", change.id),
                )
            })?;
        let Value::Bool(on) = change.value else {
            return Err(refuse(
                ErrorCode::InvalidValue,
                &change.id,
                format!("{} takes true or false.", change.id),
            ));
        };
        changes.push((key, on));
    }
    for (key, on) in changes {
        controls.set(key, on);
    }
    Ok(state(Some(controls), base))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rubylight_native_api::radeon::Change;

    const OFF: Settings = Settings {
        night_mode: false,
        headphone_surround: false,
    };
    const BOTH: Settings = Settings {
        night_mode: true,
        headphone_surround: true,
    };

    #[test]
    fn state_and_apply_follow_the_contract() {
        let none = state(None, BOTH);
        assert_eq!(none.settings.len(), 2);
        for setting in &none.settings {
            assert_eq!(setting.state, ControlState::TemporarilyUnavailable);
            assert_eq!(setting.value, None);
            assert_eq!(setting.reason.as_deref(), Some("no_session"));
        }

        let controls = Controls::default();
        controls.start(Settings {
            night_mode: false,
            headphone_surround: true,
        });
        controls.set_reason(Key::HeadphoneSurround, Some(Inactive::NextStream));
        let audio = state(Some(&controls), OFF);
        let [night, surround] = &audio.settings[..] else {
            panic!()
        };
        assert_eq!(
            (night.id.as_str(), night.group.as_str(), night.kind),
            ("audio.night_mode", "sound", Kind::Toggle)
        );
        assert_eq!(night.state, ControlState::Configurable);
        assert_eq!(night.value, Some(Value::Bool(false)));
        assert_eq!(night.reason, None);
        // Inactive but writable: still configurable, the reason is a note.
        assert_eq!(surround.state, ControlState::Configurable);
        assert_eq!(surround.value, Some(Value::Bool(true)));
        assert_eq!(surround.reason.as_deref(), Some("next_stream"));
        assert_eq!(
            surround.message.as_deref(),
            Some("Headphone surround applies from the next stream.")
        );

        let write = |changes: Vec<(&str, Value)>| AudioWrite {
            changes: changes
                .into_iter()
                .map(|(id, value)| Change {
                    id: id.into(),
                    value,
                })
                .collect(),
        };
        let revision = controls.revision();
        let audio = apply(
            &controls,
            OFF,
            &write(vec![
                ("audio.night_mode", Value::Bool(true)),
                ("audio.headphone_surround", Value::Bool(false)),
            ]),
        )
        .unwrap();
        assert!(audio.revision > revision);
        assert_eq!(
            controls.settings(OFF),
            Settings {
                night_mode: true,
                headphone_surround: false
            }
        );
        // A bad change refuses the whole write.
        for (changes, code) in [
            (
                vec![
                    ("audio.night_mode", Value::Bool(false)),
                    ("audio.volume", Value::Bool(true)),
                ],
                ErrorCode::NotFound,
            ),
            (
                vec![
                    ("audio.night_mode", Value::Bool(false)),
                    ("audio.headphone_surround", Value::Integer(1)),
                ],
                ErrorCode::InvalidValue,
            ),
            (
                vec![("audio.night_mode", Value::Choice("on".into()))],
                ErrorCode::InvalidValue,
            ),
        ] {
            let error = apply(&controls, OFF, &write(changes)).unwrap_err();
            assert_eq!(error.code, code);
            assert!(error.setting.is_some());
            assert!(controls.settings(OFF).night_mode);
        }
        let revision = controls.revision();
        assert!(!controls.set_reason(Key::HeadphoneSurround, Some(Inactive::NextStream)));
        assert_eq!(controls.revision(), revision);
    }
}
