use super::*;
#[test]
fn microphone_journal_keeps_playback_restoration_out_of_recording_roles() -> Result<()> {
    let _active = ACTIVE.lock().unwrap();
    let directory = tempfile::tempdir()?;
    assert!(!microphone_owns_recording(directory.path()));
    let journal = butterpollo_core::audio_defaults::MicrophoneDefaults::new(
        std::array::from_fn(|_| Some("physical-mic".into())),
        "steam-mic".into(),
        None,
    );
    butterpollo_core::state::write_json(&microphone_path(directory.path()), &Some(&journal))?;
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
