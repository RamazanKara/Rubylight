//! Which Windows default audio devices a stream puts back when it ends.
//!
//! A stream makes its virtual speakers (Steam Streaming Speakers, or the
//! configured `virtual_sink`) the default playback device for every role and
//! restores the user's devices afterwards. Installing Steam Streaming
//! Microphone during a stream adds endpoints that Windows can make a default
//! on arrival, for any role and either direction. Playback roles on a host
//! endpoint use the user's device from another role instead. Recording roles
//! already on a host endpoint at stream start keep that choice.
//!
//! Defaults are kept per role, in Windows' order: console, multimedia,
//! communications.

/// One device ID per role (console, multimedia, communications); `None` when
/// Windows has no default for that role.
pub type Defaults = [Option<String>; 3];

/// Role names for the log, in the order of [`Defaults`].
pub const ROLE_NAMES: [&str; 3] = ["console", "multimedia", "communications"];

/// A temporary recording default, independent of the stream's playback route.
/// Keep exact per-role originals, including an explicitly selected Steam mic.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct MicrophoneDefaults {
    pub before: Defaults,
    pub applied: String,
}

impl MicrophoneDefaults {
    pub fn new(current: Defaults, applied: String, pending: Option<&Self>) -> Self {
        let before = std::array::from_fn(|index| match pending {
            Some(old)
                if current[index]
                    .as_deref()
                    .is_some_and(|id| id.eq_ignore_ascii_case(&old.applied)) =>
            {
                old.before[index].clone()
            }
            _ => current[index].clone(),
        });
        Self { before, applied }
    }

    /// Do not take a role with no previous device: Windows cannot restore an
    /// absent default through PolicyConfig. Driver arrival normally fills it.
    pub fn apply(&self, current: &Defaults) -> Vec<usize> {
        (0..3)
            .filter(|&index| {
                self.before[index].is_some()
                    && !current[index]
                        .as_deref()
                        .is_some_and(|id| id.eq_ignore_ascii_case(&self.applied))
            })
            .collect()
    }

    /// Only undo our own selection; a user's subsequent selection wins.
    pub fn restore(&self, current: &Defaults) -> Vec<(usize, String)> {
        switches(current, &self.before, |id| {
            id.eq_ignore_ascii_case(&self.applied)
        })
    }
}

/// True for an endpoint the host installs and streams through, by its friendly
/// or adapter name: Steam Streaming Speakers, and both sides of Steam Streaming
/// Microphone ("Speakers (Steam Streaming Microphone)", "Microphone (Steam
/// Streaming Microphone)", and their translations, which keep the adapter name).
pub fn host_owned(name: &str, adapter: &str) -> bool {
    [name, adapter].iter().any(|text| {
        let text = text.to_ascii_lowercase();
        text.contains("steam streaming speakers") || text.contains("steam streaming microphone")
    })
}

/// The device to put back for each role. A saved device the host owns is
/// replaced by the user's device saved for another role (console first), or
/// left out when every role was on a host endpoint: the host never restores
/// a default to its own endpoint.
pub fn restore_targets(saved: &Defaults, owned: impl Fn(&str) -> bool) -> Defaults {
    let users = |id: &Option<String>| id.as_ref().filter(|id| !owned(id)).cloned();
    let fallback = saved.iter().find_map(users);
    std::array::from_fn(|index| match &saved[index] {
        Some(id) if owned(id) => fallback.clone(),
        other => other.clone(),
    })
}

/// A recording role already on a host endpoint is the user's choice, so the
/// stream must leave it alone when it ends.
pub fn recording_targets(saved: Defaults, owned: impl Fn(&str) -> bool) -> Defaults {
    saved.map(|id| id.filter(|id| !owned(id)))
}

/// The defaults to save at stream start: the current ones, except that a
/// role still on a host endpoint keeps the device an unfinished restore (from
/// a stream whose speakers were off when it ended, or a crash) was going to
/// put back. The result can still name host endpoints; pass it through
/// [`restore_targets`].
pub fn with_pending(
    current: Defaults,
    pending: Option<&Defaults>,
    owned: impl Fn(&str) -> bool,
) -> Defaults {
    let Some(pending) = pending else {
        return current;
    };
    std::array::from_fn(|index| match (&current[index], &pending[index]) {
        (Some(id), Some(original)) if owned(id) && !owned(original) => Some(original.clone()),
        _ => current[index].clone(),
    })
}

/// Records a device the user chose during the stream: a role whose default
/// is now neither the stream's sink nor another host endpoint (`owned` covers
/// both). Roles a host endpoint took keep the saved device. Returns whether
/// anything changed.
pub fn adopt(saved: &mut Defaults, current: &Defaults, owned: impl Fn(&str) -> bool) -> bool {
    let mut changed = false;
    for (saved, current) in saved.iter_mut().zip(current) {
        if let Some(id) = current
            && !owned(id)
            && saved.as_ref() != Some(id)
        {
            *saved = Some(id.clone());
            changed = true;
        }
    }
    changed
}

/// The roles to switch back when the stream ends, with the device for each:
/// those on a host endpoint (`owned` includes the stream's sink) whose target
/// is another device. A role the user moved to a device of their own during
/// the stream is left there.
pub fn switches(
    current: &Defaults,
    targets: &Defaults,
    owned: impl Fn(&str) -> bool,
) -> Vec<(usize, String)> {
    (0..3)
        .filter_map(|index| {
            let target = targets[index].as_ref()?;
            let now = current[index].as_deref()?;
            (owned(now) && now != target).then(|| (index, target.clone()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPEAKERS: &str = "{0.0.0.00000000}.{realtek}";
    const HEADSET: &str = "{0.0.0.00000000}.{headset}";
    const SINK: &str = "{0.0.0.00000000}.{steam-speakers}";
    const MIC_SPEAKERS: &str = "{0.0.0.00000000}.{steam-mic-render}";
    const MIC: &str = "{0.0.1.00000000}.{steam-mic-capture}";
    const USB_MIC: &str = "{0.0.1.00000000}.{usb-mic}";

    fn ids(values: [&str; 3]) -> Defaults {
        values.map(|id| (!id.is_empty()).then(|| id.to_string()))
    }

    fn owned(id: &str) -> bool {
        [SINK, MIC_SPEAKERS, MIC].contains(&id)
    }

    #[test]
    fn forwarded_microphone_restores_each_role_and_respects_manual_changes() {
        let before = ids([USB_MIC, USB_MIC, "headset-mic"]);
        let route = MicrophoneDefaults::new(before.clone(), MIC.into(), None);
        assert_eq!(route.apply(&before), [0, 1, 2]);
        assert_eq!(
            route.restore(&ids([MIC, MIC, MIC])),
            [
                (0, USB_MIC.into()),
                (1, USB_MIC.into()),
                (2, "headset-mic".into())
            ]
        );
        assert_eq!(
            route.restore(&ids([MIC, "new-mic", MIC])),
            [(0, USB_MIC.into()), (2, "headset-mic".into())]
        );
    }

    #[test]
    fn microphone_partial_failure_rolls_back_only_changed_roles() {
        let before = ids([USB_MIC, USB_MIC, "headset-mic"]);
        let route = MicrophoneDefaults::new(before.clone(), MIC.into(), None);
        assert_eq!(
            route.restore(&ids([MIC, USB_MIC, "headset-mic"])),
            [(0, USB_MIC.into())]
        );
        assert!(route.restore(&before).is_empty());
    }

    #[test]
    fn microphone_retry_keeps_originals_and_a_preselected_steam_microphone() {
        let before = ids([USB_MIC, MIC, "headset-mic"]);
        let old = MicrophoneDefaults::new(before.clone(), MIC.into(), None);
        let retry = MicrophoneDefaults::new(ids([MIC, MIC, "new-mic"]), MIC.into(), Some(&old));
        assert_eq!(retry.before, ids([USB_MIC, MIC, "new-mic"]));
        assert_eq!(
            retry.restore(&ids([MIC, MIC, MIC])),
            [(0, USB_MIC.into()), (2, "new-mic".into())]
        );
        assert_eq!(old.apply(&before), [0, 2]);
    }

    #[test]
    fn microphone_missing_roles_and_crash_journal_round_trip() {
        let before = ids([USB_MIC, "", USB_MIC]);
        let route = MicrophoneDefaults::new(before.clone(), MIC.into(), None);
        assert_eq!(route.apply(&before), [0, 2]);
        let saved = serde_json::to_vec(&route).unwrap();
        let recovered: MicrophoneDefaults = serde_json::from_slice(&saved).unwrap();
        assert_eq!(
            recovered.restore(&ids([MIC, "", MIC])),
            [(0, USB_MIC.into()), (2, USB_MIC.into())]
        );
    }

    #[test]
    fn steam_endpoints_are_host_owned_by_name_or_adapter() {
        for (name, adapter) in [
            ("Speakers (Steam Streaming Speakers)", ""),
            (
                "Lautsprecher (Steam Streaming Microphone)",
                "Steam Streaming Microphone",
            ),
            ("Microphone (Steam Streaming Microphone)", ""),
            ("Mikrofon", "Steam Streaming Microphone"),
            ("Haut-parleurs", "STEAM STREAMING SPEAKERS"),
        ] {
            assert!(host_owned(name, adapter), "{name} / {adapter}");
        }
        for (name, adapter) in [
            ("Speakers (Realtek(R) Audio)", "Realtek(R) Audio"),
            ("Headset (Steam Controller)", "Steam Controller"),
            ("", ""),
        ] {
            assert!(!host_owned(name, adapter), "{name} / {adapter}");
        }
    }

    #[test]
    fn the_2_2_0_test_the_microphone_speakers_took_communications() {
        // Saved at stream start: the user's speakers for every role.
        let mut saved = ids([SPEAKERS, SPEAKERS, SPEAKERS]);
        // The Steam microphone driver arrived mid-stream and Windows made its playback
        // side the communications default. The stream keeps its sink on the others.
        let current = ids([SINK, SINK, MIC_SPEAKERS]);
        let is_owned = |id: &str| id == SINK || owned(id);
        assert!(!adopt(&mut saved, &current, is_owned));
        assert_eq!(saved, ids([SPEAKERS, SPEAKERS, SPEAKERS]));
        // At the end every role goes back to the speakers, communications too.
        let targets = restore_targets(&saved, owned);
        assert_eq!(
            switches(&current, &targets, is_owned),
            [
                (0, SPEAKERS.to_string()),
                (1, SPEAKERS.to_string()),
                (2, SPEAKERS.to_string())
            ]
        );
    }

    #[test]
    fn a_device_the_user_picks_during_the_stream_is_kept() {
        let mut saved = ids([SPEAKERS, SPEAKERS, SPEAKERS]);
        let current = ids([SINK, SINK, HEADSET]);
        let is_owned = |id: &str| id == SINK || owned(id);
        assert!(adopt(&mut saved, &current, is_owned));
        assert_eq!(saved, ids([SPEAKERS, SPEAKERS, HEADSET]));
        // The headset is already the communications default: only the others switch.
        let targets = restore_targets(&saved, owned);
        assert_eq!(
            switches(&current, &targets, is_owned),
            [(0, SPEAKERS.to_string()), (1, SPEAKERS.to_string())]
        );
    }

    #[test]
    fn a_saved_host_endpoint_is_replaced_by_the_users_device() {
        // After 2.2.0 the PC itself was left with the microphone's speakers for
        // communications, so the next stream saves that.
        let saved = ids([SPEAKERS, SPEAKERS, MIC_SPEAKERS]);
        assert_eq!(
            restore_targets(&saved, owned),
            ids([SPEAKERS, SPEAKERS, SPEAKERS])
        );
        // The first role the user owns is the fallback.
        let saved = ids([MIC_SPEAKERS, HEADSET, SPEAKERS]);
        assert_eq!(
            restore_targets(&saved, owned),
            ids([HEADSET, HEADSET, SPEAKERS])
        );
        // Nothing the user owns: nothing to put back, never a host endpoint.
        let saved = ids([SINK, MIC_SPEAKERS, ""]);
        assert_eq!(restore_targets(&saved, owned), ids(["", "", ""]));
        // A role without a default stays without one.
        let saved = ids([SPEAKERS, "", SPEAKERS]);
        assert_eq!(restore_targets(&saved, owned), saved);
    }

    #[test]
    fn recording_defaults_go_back_from_the_steam_microphone() {
        let saved = ids([USB_MIC, USB_MIC, USB_MIC]);
        let current = ids([MIC, USB_MIC, MIC]);
        let targets = restore_targets(&saved, owned);
        assert_eq!(
            switches(&current, &targets, owned),
            [(0, USB_MIC.to_string()), (2, USB_MIC.to_string())]
        );
        // Not saved (a journal from 2.2.0): nothing to put back.
        assert!(switches(&current, &ids(["", "", ""]), owned).is_empty());
    }

    #[test]
    fn recording_roles_already_on_steam_are_left_alone() {
        let saved = ids([MIC, MIC, USB_MIC]);
        let targets = recording_targets(saved.clone(), owned);
        assert_eq!(targets, ids(["", "", USB_MIC]));
        assert!(switches(&saved, &targets, owned).is_empty());
        assert_eq!(
            switches(&ids([MIC, MIC, MIC]), &targets, owned),
            [(2, USB_MIC.to_string())]
        );
    }

    #[test]
    fn roles_on_a_users_device_are_left_alone() {
        let targets = ids([SPEAKERS, SPEAKERS, SPEAKERS]);
        assert!(switches(&ids([HEADSET, SPEAKERS, ""]), &targets, owned).is_empty());
    }

    #[test]
    fn a_pending_restore_keeps_the_original_for_roles_still_on_host_endpoints() {
        let pending = ids([SPEAKERS, SPEAKERS, HEADSET]);
        let current = ids([SINK, HEADSET, MIC_SPEAKERS]);
        assert_eq!(
            with_pending(current.clone(), Some(&pending), owned),
            ids([SPEAKERS, HEADSET, HEADSET])
        );
        assert_eq!(with_pending(current.clone(), None, owned), current);
        // A pending entry that is itself a host endpoint is no better.
        let pending = ids([MIC_SPEAKERS, "", ""]);
        assert_eq!(
            with_pending(current.clone(), Some(&pending), owned),
            current
        );
    }
}
