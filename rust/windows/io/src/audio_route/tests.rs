use super::*;
#[test]
fn microphone_journal_keeps_playback_restoration_out_of_recording_roles() -> Result<()> {
    let _active = ACTIVE.lock().unwrap();
    let directory = tempfile::tempdir()?;
    assert!(!microphone_owns_recording(directory.path()));
    let journal = butterpollo_media_policy::audio_defaults::MicrophoneDefaults::new(
        std::array::from_fn(|_| Some("physical-mic".into())),
        "steam-mic".into(),
        None,
    );
    butterpollo_session::state::write_json(&microphone_path(directory.path()), &Some(&journal))?;
    assert!(microphone_owns_recording(directory.path()));
    assert_eq!(
        microphone_pending(directory.path())?.unwrap().before,
        journal.before
    );
    // A damaged journal must not authorize the older playback snapshot
    // to overwrite the microphone choices it can no longer read.
    std::fs::write(microphone_path(directory.path()), b"{")?;
    assert!(microphone_pending(directory.path()).is_err());
    assert!(microphone_owns_recording(directory.path()));
    std::fs::write(microphone_path(directory.path()), b"null")?;
    assert!(!microphone_owns_recording(directory.path()));
    Ok(())
}
#[test]
fn physical_audio_fallback_warns_only_when_a_virtual_route_was_expected() {
    assert!(
        virtual_sink_warning(false, false, false)
            .unwrap()
            .contains("Sound may play on the host")
    );
    for (host, capture, virtual_sink) in [
        (true, false, false),
        (false, true, false),
        (false, false, true),
    ] {
        assert!(virtual_sink_warning(host, capture, virtual_sink).is_none());
    }
}

fn device_format(channels: u16, rate: u32, sample: Sample, mask: u32) -> Vec<u8> {
    let mut bytes = virtual_format(usize::from(channels), sample, false);
    let (bits, _) = sample.bits();
    // SAFETY: virtual_format returns a whole WAVEFORMATEXTENSIBLE; the access is unaligned.
    unsafe {
        let format = bytes.as_mut_ptr().cast::<WAVEFORMATEXTENSIBLE>();
        let mut value = std::ptr::read_unaligned(format);
        value.Format.nSamplesPerSec = rate;
        value.Format.nAvgBytesPerSec = rate * u32::from(channels) * u32::from(bits / 8);
        value.dwChannelMask = mask;
        std::ptr::write_unaligned(format, value);
    }
    bytes
}
#[test]
fn steam_microphone_playback_takes_the_recording_format() {
    // Steam's defaults: stereo 48 kHz float playback, mono 44.1 kHz float
    // recording, which the driver copies without converting (issue 56).
    let playback = device_format(2, 48000, Sample::Float32, 3);
    let recording = device_format(1, 44100, Sample::Float32, 4);
    assert_eq!(
        matching_playback_format(&playback, &recording),
        Some(recording.clone())
    );
    assert_eq!(describe_format(&playback), "2 ch 48000 Hz 32-bit float");
    assert_eq!(describe_format(&recording), "1 ch 44100 Hz 32-bit float");
    // Rate, channels or sample type alone differing is a mismatch too.
    for other in [
        device_format(1, 48000, Sample::Float32, 4),
        device_format(2, 44100, Sample::Float32, 3),
        device_format(1, 44100, Sample::Pcm32, 4),
        device_format(1, 44100, Sample::Pcm24In32, 4),
    ] {
        assert!(matching_playback_format(&other, &recording).is_some());
    }
    // The same samples with another speaker mask need no change.
    let same = device_format(1, 44100, Sample::Float32, 0);
    assert_eq!(matching_playback_format(&same, &recording), None);
    assert_eq!(matching_playback_format(&recording, &recording), None);
    assert_eq!(
        describe_format(&device_format(1, 44100, Sample::Pcm24In32, 4)),
        "1 ch 44100 Hz 24-in-32-bit PCM"
    );
}
#[test]
fn steam_microphone_never_adds_gain_to_a_client_voice() {
    // Its slider at 100% is +30 dB: the voice clipped into noise (issue 56).
    assert_eq!(unity_cap(30.0), Some(0.0));
    assert_eq!(unity_cap(1.5), Some(0.0));
    // Unity, within the endpoint's rounding, and any cut stay as they are.
    for level in [0.75, 0.0, -0.000_01, -12.0, -96.0] {
        assert_eq!(unity_cap(level), None, "{level}");
    }
}
#[test]
fn steam_microphone_formats_are_reported_and_a_kept_format_fails() {
    // The log names both sides whether or not the playback side changed
    // (issue 56: a user's log could not tell whether the match applied).
    let playback = device_format(2, 48000, Sample::Float32, 3);
    let recording = device_format(1, 44100, Sample::Pcm16, 4);
    let kept = MicrophoneFormats::new(&recording, &recording, &recording);
    assert!(!kept.changed());
    assert_eq!(kept.recording, "1 ch 44100 Hz 16-bit PCM");
    let set = MicrophoneFormats::new(&playback, &recording, &recording);
    assert!(set.changed());
    assert_eq!(
        (set.before.as_str(), set.after.as_str()),
        ("2 ch 48000 Hz 32-bit float", "1 ch 44100 Hz 16-bit PCM")
    );
    // Windows accepting the format but reading back the old one is an error.
    check_matched(&recording, &recording).unwrap();
    let error = check_matched(&playback, &recording)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("still 2 ch 48000 Hz 32-bit float")
            && error.contains("1 ch 44100 Hz 16-bit PCM"),
        "{error}"
    );
}
#[test]
fn streams_sharing_the_sink_get_the_most_channels_any_asked_for() {
    assert_eq!(shared_channels(&BTreeMap::new()), None);
    // A stereo phone and a 5.1 living-room PC on the same app: 5.1 for
    // both, whoever reopened capture last.
    let both = BTreeMap::from([(1, 2), (2, 6)]);
    assert_eq!(shared_channels(&both), Some(6));
    assert_eq!(shared_channels(&BTreeMap::from([(1, 2)])), Some(2));
}
#[test]
fn a_pending_restore_keeps_the_users_original_speakers() {
    let some = |id: &str| Some(id.to_owned());
    let current = [some("virtual"), some("virtual"), some("headset")];
    // Nothing pending: the current defaults are the originals.
    assert_eq!(originals(current.clone(), None, |_| false), current);
    // The last restore failed while the speakers were off: roles still on
    // the virtual sink keep the speakers; a role the user changed since
    // keeps the user's choice.
    let pending = Journal {
        before: [some("speakers"), some("speakers"), some("speakers")],
        applied: "virtual".into(),
        format: None,
        capture_before: Default::default(),
    };
    assert_eq!(
        originals(current.clone(), Some(&pending), |_| false),
        [some("speakers"), some("speakers"), some("headset")]
    );
    assert_eq!(
        originals(
            [
                some("virtual"),
                some("virtual"),
                some("microphone-speakers")
            ],
            Some(&Journal {
                before: [some("speakers"), some("speakers"), some("headset")],
                ..pending.clone()
            }),
            |id| id == "microphone-speakers",
        ),
        [some("speakers"), some("speakers"), some("headset")]
    );
    // A capture-only journal sets no defaults.
    let capture_only = Journal {
        applied: String::new(),
        ..pending
    };
    assert_eq!(
        originals(current.clone(), Some(&capture_only), |_| false),
        current
    );
}
#[test]
fn the_steam_microphone_is_found_by_its_adapter_not_the_speakers() {
    let endpoint = |name: &str, adapter: &str| Endpoint {
        id: name.into(),
        name: name.into(),
        description: "Speakers".into(),
        adapter: adapter.into(),
        default: false,
        virtual_sink: false,
    };
    let endpoints = [
        endpoint(
            "Speakers (Steam Streaming Speakers)",
            "Steam Streaming Speakers",
        ),
        endpoint(
            "Speakers (Steam Streaming Microphone)",
            "Steam Streaming Microphone",
        ),
    ];
    assert_eq!(
        steam_microphone(&endpoints).unwrap().id,
        "Speakers (Steam Streaming Microphone)"
    );
    assert!(steam_microphone(&endpoints[..1]).is_none());
}
#[test]
fn sinks_match_by_id_name_description_or_adapter() {
    let endpoint = |id: &str, name: &str, description: &str, adapter: &str| Endpoint {
        id: id.into(),
        name: name.into(),
        description: description.into(),
        adapter: adapter.into(),
        default: false,
        virtual_sink: false,
    };
    let endpoints = [
        endpoint("{a}", "Speakers (Realtek)", "Speakers", "Realtek(R) Audio"),
        endpoint(
            "{b}",
            "Speakers (Steam Streaming Speakers)",
            "Speakers",
            "Steam Streaming Speakers",
        ),
    ];
    let id = |name: &str| find(&endpoints, name).map(|e| e.id.as_str());
    assert_eq!(id("{B}"), Some("{b}"));
    assert_eq!(id("speakers (realtek)"), Some("{a}"));
    assert_eq!(id("Steam Streaming Speakers"), Some("{b}"));
    // A shared description picks the first endpoint, as Vibepollo does.
    assert_eq!(id("Speakers"), Some("{a}"));
    assert_eq!(id("Headphones"), None);
}
#[test]
fn virtual_speakers_preserve_valid_depth_and_offer_pcm_fallbacks() -> Result<()> {
    let formats = virtual_formats(2, 24)?;
    // SAFETY: `formats[0]` is a whole WAVEFORMATEXTENSIBLE from virtual_format, read unaligned.
    let first =
        unsafe { std::ptr::read_unaligned(formats[0].as_ptr().cast::<WAVEFORMATEXTENSIBLE>()) };
    let container_bits = first.Format.wBitsPerSample;
    let alignment = first.Format.nBlockAlign;
    assert_eq!(container_bits, 32);
    assert_eq!(valid_bits(&formats[0]), 24);
    assert_eq!(alignment, 8);
    let subformat = first.SubFormat;
    assert_eq!(
        subformat,
        GUID::from_u128(0x00000001_0000_0010_8000_00aa00389b71)
    );
    let formats = virtual_formats(2, 32)?;
    assert_eq!(formats.len(), 5);
    // SAFETY: `formats[1]` is a whole WAVEFORMATEXTENSIBLE from virtual_format, read unaligned.
    let pcm =
        unsafe { std::ptr::read_unaligned(formats[1].as_ptr().cast::<WAVEFORMATEXTENSIBLE>()) };
    let subformat = pcm.SubFormat;
    assert_eq!(
        subformat,
        GUID::from_u128(0x00000001_0000_0010_8000_00aa00389b71)
    );
    assert!(virtual_formats(4, 24).is_err());
    assert_eq!(virtual_formats(6, 16)?.len(), 10);
    assert_eq!(virtual_formats(8, 16)?.len(), 5);
    Ok(())
}
#[test]
fn policy_config_abi_and_surround_formats_match_windows_contract() {
    assert_eq!(
        std::mem::offset_of!(PolicyVtbl, default),
        13 * size_of::<usize>()
    );
    for (channels, mask) in [(2, 3), (6, 0x3f), (8, 0x63f), (12, 0x2d63f)] {
        let bytes = virtual_format(channels, Sample::Pcm24, false);
        // SAFETY: `bytes` is a whole WAVEFORMATEXTENSIBLE from virtual_format, read unaligned.
        let format =
            unsafe { std::ptr::read_unaligned(bytes.as_ptr().cast::<WAVEFORMATEXTENSIBLE>()) };
        let actual_mask = format.dwChannelMask;
        let rate = format.Format.nAvgBytesPerSec;
        assert_eq!(actual_mask, mask);
        assert_eq!(rate, 48000 * channels as u32 * 3);
    }
}

struct FakeFormatPolicy {
    current: std::cell::RefCell<Vec<u8>>,
    max_channels: usize,
    set_calls: std::cell::RefCell<Vec<usize>>,
}

impl FakeFormatPolicy {
    fn new(initial: Vec<u8>, max_channels: usize) -> Self {
        Self {
            current: std::cell::RefCell::new(initial),
            max_channels,
            set_calls: std::cell::RefCell::new(Vec::new()),
        }
    }
}

impl FormatPolicy for FakeFormatPolicy {
    fn format(&self, _id: &str) -> Result<Vec<u8>> {
        Ok(self.current.borrow().clone())
    }
    fn set_format(&self, _id: &str, bytes: &[u8]) -> Result<()> {
        let ch = usize::from(sample_layout(bytes).0);
        self.set_calls.borrow_mut().push(ch);
        if ch > self.max_channels {
            bail!("{ch} channels rejected");
        }
        *self.current.borrow_mut() = bytes.to_vec();
        Ok(())
    }
}

#[test]
fn surround_fallback_drops_714_to_71_and_rechecks_cleanly() -> Result<()> {
    assert_eq!(fallback_channels(12)?, &[12, 8, 6]);
    assert_eq!(fallback_channels(8)?, &[8, 6]);
    assert_eq!(fallback_channels(6)?, &[6]);
    assert_eq!(fallback_channels(2)?, &[2]);
    let dir = tempfile::tempdir()?;
    let orig = virtual_format(2, Sample::Pcm16, false);
    let policy = FakeFormatPolicy::new(orig.clone(), 8);
    let mut journal = Journal {
        applied: "{virtual}".into(),
        ..Journal::default()
    };
    let applied = apply_virtual_format_with(
        &policy,
        "{virtual}",
        12,
        16,
        dir.path(),
        &mut journal,
        |_| Ok(()),
    )?;
    assert_eq!(applied, 8);
    assert_eq!(sample_layout(&policy.format("{virtual}")?).0, 8);
    let saved: Journal = serde_json::from_slice(&std::fs::read(path(dir.path()))?)?;
    assert_eq!(saved.format.as_ref().unwrap().before, orig);
    assert_eq!(
        saved.format.as_ref().unwrap().applied,
        policy.format("{virtual}")?
    );
    // Subsequent set_channels(12) from stream/audio.rs falls back to 8 cleanly
    // without disturbing the active 8ch format or corrupting the journal.
    let mut active = Some(Active {
        directory: dir.path().to_path_buf(),
        sink: "{virtual}".into(),
        virtual_sink: true,
        owns_journal: true,
        users: 1,
        channels: BTreeMap::from([(1, 8)]),
    });
    let calls_before = policy.set_calls.borrow().len();
    let applied = set_channels_with(&mut active, 1, "{virtual}", dir.path(), 12, &policy, |_| {
        Ok(())
    })?;
    // The caller learns the sink took less than it asked for.
    assert_eq!(applied, 8);
    // Only tried 12ch candidates, then recognized 8ch is already active and applied.
    assert!(
        policy.set_calls.borrow()[calls_before..]
            .iter()
            .all(|&ch| ch == 12)
    );
    assert_eq!(active.as_ref().unwrap().channels[&1], 8);
    let saved_after: Journal = serde_json::from_slice(&std::fs::read(path(dir.path()))?)?;
    assert_eq!(
        saved_after.format.as_ref().unwrap().applied,
        policy.format("{virtual}")?
    );
    Ok(())
}

#[test]
fn failed_surround_preserves_journal_format_and_shared_channels_for_teardown() -> Result<()> {
    let dir = tempfile::tempdir()?;
    // 1. Stereo-only sink refusing 8ch during acquire leaves journal.format None.
    let stereo_orig = virtual_format(2, Sample::Pcm16, false);
    let stereo_only = FakeFormatPolicy::new(stereo_orig.clone(), 2);
    let mut journal = Journal {
        applied: "{virtual}".into(),
        ..Journal::default()
    };
    assert!(
        apply_virtual_format_with(
            &stereo_only,
            "{virtual}",
            8,
            16,
            dir.path(),
            &mut journal,
            |_| Ok(()),
        )
        .is_err()
    );
    assert!(journal.format.is_none());
    let saved: Journal = serde_json::from_slice(&std::fs::read(path(dir.path()))?)?;
    assert!(saved.format.is_none());

    // 2. Previously applied 2ch format is preserved when a later set_channels(8) fails,
    // and shared.channels is NOT polluted with 8 so teardown restores `stereo_orig`.
    let applied_2ch = virtual_format(2, Sample::Pcm24In32, false);
    let policy = FakeFormatPolicy::new(applied_2ch.clone(), 2);
    let initial_journal = Journal {
        applied: "{virtual}".into(),
        format: Some(FormatChange {
            id: "{virtual}".into(),
            before: stereo_orig.clone(),
            applied: applied_2ch.clone(),
        }),
        ..Journal::default()
    };
    save(dir.path(), &initial_journal)?;
    let mut active = Some(Active {
        directory: dir.path().to_path_buf(),
        sink: "{virtual}".into(),
        virtual_sink: true,
        owns_journal: true,
        users: 1,
        channels: BTreeMap::from([(1, 2)]),
    });
    assert!(
        set_channels_with(&mut active, 1, "{virtual}", dir.path(), 8, &policy, |_| {
            Ok(())
        })
        .is_err()
    );
    // shared.channels stays 2 (not 8).
    assert_eq!(active.as_ref().unwrap().channels[&1], 2);
    // audio-recovery.json still records applied_2ch (matching current format), NOT a rejected 6ch/8ch format.
    let saved: Journal = serde_json::from_slice(&std::fs::read(path(dir.path()))?)?;
    let fmt = saved.format.unwrap();
    assert_eq!(fmt.before, stereo_orig);
    assert_eq!(fmt.applied, applied_2ch);
    assert_eq!(fmt.applied, policy.format("{virtual}")?);
    Ok(())
}

#[test]
fn saved_defaults_restore_attempts_every_playback_and_recording_role_despite_errors() {
    let saved = [
        [
            Some("spk-console".into()),
            Some("spk-media".into()),
            Some("spk-comms".into()),
        ],
        [
            Some("mic-console".into()),
            Some("mic-media".into()),
            Some("mic-comms".into()),
        ],
    ];
    let mut attempted = Vec::new();
    let err = restore_saved_defaults(
        &saved,
        |flow| {
            Ok(if flow == eRender {
                [
                    Some("steam-spk".into()),
                    Some("steam-spk".into()),
                    Some("steam-spk".into()),
                ]
            } else {
                [
                    Some("steam-mic".into()),
                    Some("mic-media".into()), // unchanged: must be skipped
                    Some("steam-mic".into()),
                ]
            })
        },
        |device, role| {
            attempted.push((device.to_string(), role));
            if device == "spk-console" {
                bail!("console speaker offline");
            }
            if device == "mic-console" {
                bail!("console mic offline");
            }
            Ok(())
        },
    )
    .unwrap_err();
    // First error is retained and returned.
    assert!(err.to_string().contains("console speaker offline"), "{err}");
    // All remaining playback AND recording roles were still attempted.
    assert_eq!(
        attempted,
        vec![
            ("spk-console".into(), eConsole),
            ("spk-media".into(), eMultimedia),
            ("spk-comms".into(), eCommunications),
            ("mic-console".into(), eConsole),
            ("mic-comms".into(), eCommunications),
        ]
    );
}

#[test]
fn set_channels_refuses_to_alter_unclaimed_sink_format() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let stereo_orig = virtual_format(2, Sample::Pcm16, false);
    let policy = FakeFormatPolicy::new(stereo_orig.clone(), 8);
    let initial_journal = Journal {
        applied: "{virtual}".into(),
        format: None,
        ..Journal::default()
    };
    save(dir.path(), &initial_journal)?;
    let mut active = Some(Active {
        directory: dir.path().to_path_buf(),
        sink: "{virtual}".into(),
        virtual_sink: true,
        owns_journal: true,
        users: 1,
        channels: BTreeMap::from([(1, 2)]),
    });
    let err = set_channels_with(&mut active, 1, "{virtual}", dir.path(), 8, &policy, |_| {
        Ok(())
    })
    .unwrap_err();
    assert!(err.to_string().contains("not claimed"), "{err}");
    assert!(policy.set_calls.borrow().is_empty());
    assert_eq!(policy.format("{virtual}")?, stereo_orig);
    let saved: Journal = serde_json::from_slice(&std::fs::read(path(dir.path()))?)?;
    assert!(saved.format.is_none());
    Ok(())
}

#[test]
fn capture_only_route_preserves_pending_recovery_journal_and_skips_restore_on_drop() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let pending = Journal {
        before: [
            Some("{speakers}".into()),
            Some("{speakers}".into()),
            Some("{headset}".into()),
        ],
        applied: "{virtual}".into(),
        format: Some(FormatChange {
            id: "{virtual}".into(),
            before: vec![1, 2],
            applied: vec![3, 4],
        }),
        capture_before: [Some("{mic}".into()), None, None],
    };
    save(dir.path(), &pending)?;
    let before_bytes = std::fs::read(path(dir.path()))?;

    let config = Config::parse("audio_sink={sink}\naudio_sink_capture_only=true\n")?;
    let endpoints = [Endpoint {
        id: "{sink}".into(),
        name: "Capture Sink".into(),
        description: "Capture Sink".into(),
        adapter: "Realtek".into(),
        default: false,
        virtual_sink: false,
    }];
    // Acquire under `capture_only`: must not overwrite `audio-recovery.json` and marks
    // `owns_journal = false`.
    let route = {
        let mut active = ACTIVE.lock().unwrap();
        let route = acquire_capture_only(&mut active, &config, dir.path(), 2, &endpoints)?;
        assert!(!active.as_ref().unwrap().owns_journal);
        route
    };
    assert!(route.capture_only);
    assert_eq!(std::fs::read(path(dir.path()))?, before_bytes);

    // Dropping the real Route must clear `ACTIVE` without restoring defaults or clearing
    // the pending crash-recovery journal.
    drop(route);
    assert!(ACTIVE.lock().unwrap().is_none());
    assert_eq!(std::fs::read(path(dir.path()))?, before_bytes);

    // Verify `release_active` directly: non-owner skips restore & mark_audio(false);
    // owner runs both once the last user leaves.
    let mut non_owner = Some(Active {
        directory: dir.path().into(),
        sink: "{sink}".into(),
        virtual_sink: false,
        owns_journal: false,
        users: 1,
        channels: BTreeMap::from([(1, 2)]),
    });
    let mut restored = 0usize;
    let mut marked = Vec::new();
    release_active(
        &mut non_owner,
        1,
        |_| {
            restored += 1;
            Ok(())
        },
        |on| {
            marked.push(on);
            Ok(())
        },
    );
    assert!(non_owner.is_none());
    assert_eq!(restored, 0);
    assert!(marked.is_empty());

    let mut owner = Some(Active {
        directory: dir.path().into(),
        sink: "{virtual}".into(),
        virtual_sink: true,
        owns_journal: true,
        users: 2,
        channels: BTreeMap::from([(1, 2), (2, 6)]),
    });
    release_active(&mut owner, 1, |_| unreachable!(), |_| unreachable!());
    assert_eq!(owner.as_ref().unwrap().users, 1);
    release_active(
        &mut owner,
        2,
        |_| {
            restored += 1;
            Ok(())
        },
        |on| {
            marked.push(on);
            Ok(())
        },
    );
    assert!(owner.is_none());
    assert_eq!(restored, 1);
    assert_eq!(marked, [false]);
    Ok(())
}
