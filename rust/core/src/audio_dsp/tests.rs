use super::*;
use rubylight_native_api::radeon::Change;

const SEVEN_ONE: u32 = 0x63f;

fn noise(samples: usize, seed: u64) -> Vec<f32> {
    let mut state = seed;
    (0..samples)
        .map(|_| {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((state >> 40) as f32 / (1u64 << 24) as f32 - 0.5) * 0.5
        })
        .collect()
}

const OFF: Settings = Settings {
    night_mode: false,
    headphone_surround: false,
};
const BOTH: Settings = Settings {
    night_mode: true,
    headphone_surround: true,
};

#[test]
fn with_both_settings_off_the_output_is_bit_identical() {
    for channels in [2, 6, 8] {
        let mut chain = Chain::new(channels, channels).unwrap();
        let mut input = noise(240 * channels * 20, 5);
        input[3] = -0.;
        input[7] = 1e-40;
        for packet in input.chunks(240 * channels) {
            let mut samples = packet.to_vec();
            chain.process(&mut samples, OFF);
            assert_eq!(
                samples.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                packet.iter().map(|v| v.to_bits()).collect::<Vec<_>>()
            );
        }
    }
    // 8-channel capture of a stereo endpoint passes FL and FR through.
    let mut chain = Chain::new(8, 2).unwrap();
    chain.set_source(2, 3).unwrap();
    let input = noise(240 * 8, 9);
    let mut samples = input.clone();
    chain.process(
        &mut samples,
        Settings {
            headphone_surround: true,
            ..OFF
        },
    );
    for (out, frame) in samples
        .as_chunks::<2>()
        .0
        .iter()
        .zip(input.as_chunks::<8>().0)
    {
        assert_eq!(out, &frame[..2]);
    }
}

#[test]
fn surround_capture_is_8_channels_only_for_a_stereo_system_audio_stream() {
    let on = Settings {
        headphone_surround: true,
        ..OFF
    };
    assert_eq!(Conditions::capture_channels(on, 2, false, false), 8);
    assert_eq!(Conditions::capture_channels(OFF, 2, false, false), 2);
    assert_eq!(Conditions::capture_channels(on, 6, false, false), 6);
    assert_eq!(Conditions::capture_channels(on, 8, false, false), 8);
    assert_eq!(Conditions::capture_channels(on, 2, true, false), 2);
    assert_eq!(Conditions::capture_channels(on, 2, false, true), 2);
}

#[test]
fn each_inactive_reason_is_set_for_its_case() {
    let active = Conditions {
        client_channels: 2,
        game_audio: false,
        muted: false,
        capture_channels: 8,
        source_surround: Some(true),
    };
    let table = [
        (active, None, None),
        (
            Conditions {
                source_surround: None,
                ..active
            },
            None,
            None,
        ),
        (
            Conditions {
                muted: true,
                client_channels: 6,
                game_audio: true,
                ..active
            },
            Some(Inactive::Muted),
            Some(Inactive::Muted),
        ),
        (
            Conditions {
                client_channels: 6,
                capture_channels: 6,
                game_audio: true,
                ..active
            },
            None,
            Some(Inactive::ClientSurround),
        ),
        (
            Conditions {
                game_audio: true,
                capture_channels: 2,
                ..active
            },
            None,
            Some(Inactive::GameOnlyAudio),
        ),
        (
            Conditions {
                capture_channels: 2,
                source_surround: Some(false),
                ..active
            },
            None,
            Some(Inactive::NextStream),
        ),
        (
            Conditions {
                source_surround: Some(false),
                ..active
            },
            None,
            Some(Inactive::StereoEndpoint),
        ),
    ];
    for (conditions, night, surround) in table {
        assert_eq!(conditions.reason(Key::NightMode), night, "{conditions:?}");
        assert_eq!(
            conditions.reason(Key::HeadphoneSurround),
            surround,
            "{conditions:?}"
        );
    }
    for reason in Inactive::ALL {
        assert_eq!(Inactive::from_u8(reason as u8), Some(reason));
    }
    assert_eq!(
        Inactive::NextStream.message(Key::HeadphoneSurround),
        "Headphone surround applies from the next stream."
    );
}

#[test]
fn the_report_warns_only_for_a_setting_that_is_on_and_clears_it() {
    let controls = Controls::default();
    let warnings = crate::session::Warnings::default();
    let mut report = Report::default();
    let mut chain = Chain::new(2, 2).unwrap();
    let on = Settings {
        headphone_surround: true,
        ..OFF
    };
    let game = Conditions {
        client_channels: 2,
        game_audio: true,
        muted: false,
        capture_channels: 2,
        source_surround: None,
    };
    let codes = |warnings: &crate::session::Warnings| {
        warnings
            .snapshot()
            .into_iter()
            .map(|w| (w.code, w.message))
            .collect::<Vec<_>>()
    };
    report.update(&controls, &warnings, &game, OFF, &mut chain);
    assert_eq!(codes(&warnings), []);
    assert_eq!(
        controls.reason(Key::HeadphoneSurround),
        Some(Inactive::GameOnlyAudio)
    );
    report.update(&controls, &warnings, &game, on, &mut chain);
    assert_eq!(
        codes(&warnings),
        [(
            Key::HeadphoneSurround.warning().to_string(),
            Inactive::GameOnlyAudio
                .message(Key::HeadphoneSurround)
                .to_string()
        )]
    );
    let muted = Conditions {
        muted: true,
        ..game
    };
    let both = Settings {
        night_mode: true,
        ..on
    };
    report.update(&controls, &warnings, &muted, both, &mut chain);
    assert_eq!(codes(&warnings).len(), 2);
    assert_eq!(controls.reason(Key::NightMode), Some(Inactive::Muted));
    report.update(&controls, &warnings, &muted, OFF, &mut chain);
    assert_eq!(codes(&warnings), []);
}

#[test]
fn precedence_is_live_then_launch_then_the_effective_config() {
    let mut config = crate::config::Config::parse("audio_night_mode = on\n").unwrap();
    let mut options = BTreeMap::new();
    assert_eq!(
        resolve(&config, &options, Live::default()),
        Settings {
            night_mode: true,
            headphone_surround: false
        }
    );
    // The app's and device's layers are already in the effective config.
    config
        .values
        .insert("audio_headphone_surround".into(), "true".into());
    assert!(resolve(&config, &options, Live::default()).headphone_surround);
    options.insert("audioNightMode".into(), "off".into());
    options.insert("audioHeadphoneSurround".into(), "loud".into());
    let start = resolve(&config, &options, Live::default());
    assert_eq!(
        start,
        Settings {
            night_mode: false,
            headphone_surround: true
        }
    );

    let controls = Controls::default();
    // Before the stream resolves, the base shows.
    assert_eq!(controls.settings(start), start);
    // A live change made before the audio thread starts keeps priority.
    controls.set(Key::NightMode, true);
    controls.start(start);
    assert_eq!(
        controls.settings(OFF),
        Settings {
            night_mode: true,
            headphone_surround: true
        }
    );
    controls.set(Key::HeadphoneSurround, false);
    assert!(!controls.settings(BOTH).headphone_surround);
}

#[test]
fn a_remembered_live_choice_sits_between_the_launch_and_the_config() {
    let config = crate::config::Config::parse("audio_night_mode = on\n").unwrap();
    let mut options = BTreeMap::new();
    let remembered = Live {
        night_mode: Some(false),
        headphone_surround: Some(true),
    };
    assert_eq!(
        resolve(&config, &options, remembered),
        Settings {
            night_mode: false,
            headphone_surround: true
        }
    );
    options.insert("audioHeadphoneSurround".into(), "off".into());
    assert!(!resolve(&config, &options, remembered).headphone_surround);

    // Only live changes are remembered, merged per client and app.
    let controls = Controls::default();
    controls.start(BOTH);
    assert_eq!(controls.live(), Live::default());
    controls.set(Key::NightMode, false);
    let memory = Memory::default();
    memory.remember("a", 1, controls.live());
    memory.remember(
        "a",
        1,
        Live {
            night_mode: None,
            headphone_surround: Some(true),
        },
    );
    assert_eq!(
        memory.recall("a", 1),
        Live {
            night_mode: Some(false),
            headphone_surround: Some(true)
        }
    );
    assert_eq!(memory.recall("a", 2), Live::default());
    assert_eq!(memory.recall("b", 1), Live::default());
}

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

#[test]
fn surround_renders_8_to_2_and_switches_live_with_a_crossfade() {
    let mut chain = Chain::new(8, 2).unwrap();
    chain.set_source(8, SEVEN_ONE).unwrap();
    assert_eq!(chain.source_surround(), Some(true));
    let surround = Settings {
        headphone_surround: true,
        ..OFF
    };
    assert!(chain.rendering(surround));
    let input = noise(240 * 8 * 40, 2);
    let mut output = Vec::new();
    for (i, packet) in input.chunks(240 * 8).enumerate() {
        let mut samples = packet.to_vec();
        let settings = if (10..20).contains(&i) { OFF } else { surround };
        chain.process(&mut samples, settings);
        assert_eq!(samples.len(), 480);
        assert!(samples.iter().all(|v| v.abs() <= night::CLIP_CEILING));
        output.extend(samples);
    }
    // Each switch is smooth: no frame jumps more than the signal does.
    let step = |s: &[f32]| {
        s.as_chunks::<2>()
            .0
            .windows(2)
            .map(|w| (w[1][0] - w[0][0]).abs())
            .fold(0f32, f32::max)
    };
    let typical = step(&output[..480 * 10]);
    for at in [10, 20] {
        let around = &output[(at - 1) * 480..(at + 2) * 480];
        assert!(step(around) <= typical * 1.5, "{at}");
    }
    // Without 8-channel capture, nothing renders.
    let chain = Chain::new(2, 2).unwrap();
    assert!(!chain.rendering(surround));
}

#[test]
fn silence_and_nan_through_the_whole_chain() {
    let mut chain = Chain::new(8, 2).unwrap();
    chain.set_source(8, SEVEN_ONE).unwrap();
    let mut silence = vec![0.; 240 * 8];
    chain.process(&mut silence, BOTH);
    assert_eq!(silence, vec![0.; 480]);
    let mut bad = vec![f32::NAN; 240 * 8];
    chain.process(&mut bad, BOTH);
    assert!(bad.iter().all(|v| *v == 0.));
    assert!(chain.take_bad_samples());
    assert!(!chain.take_bad_samples());
    let mut later = noise(240 * 8, 4);
    chain.process(&mut later, BOTH);
    assert!(later.iter().all(|v| v.is_finite()));
}

#[test]
fn with_night_mode_on_or_ramping_only_its_clipper_runs() {
    let mut chain = Chain::new(8, 2).unwrap();
    chain.set_source(8, SEVEN_ONE).unwrap();
    // The reference: the HRIR render without a clipper, then night mode.
    let mut surround = Surround::new(Layout::new(8, SEVEN_ONE).unwrap());
    let mut night = Night::default();
    let surround_only = Settings {
        headphone_surround: true,
        ..OFF
    };
    // 20 ms packets, so the 10 ms off ramp ends inside one.
    let input: Vec<f32> = noise(960 * 8 * 8, 6).iter().map(|v| v * 4.).collect();
    let mut loud = false;
    for (i, packet) in input.chunks(960 * 8).enumerate() {
        let settings = if i < 6 { BOTH } else { surround_only };
        let mut samples = packet.to_vec();
        chain.process(&mut samples, settings);
        assert!(surround.process(packet, true));
        let raw = surround.output.clone();
        loud |= raw.iter().any(|v| v.abs() > night::CLIP_KNEE * 1.1);
        let mut want = raw.clone();
        let clipped = night.process(&mut want, 2, settings.night_mode);
        assert_eq!(clipped, [960, 960, 960, 960, 960, 960, 480, 0][i]);
        for sample in &mut want[clipped * 2..] {
            *sample = night::soft_clip(*sample);
        }
        assert_eq!(bits(&samples), bits(&want), "packet {i}");
        assert!(samples.iter().all(|v| v.abs() <= night::CLIP_CEILING));
    }
    assert!(loud);
}

fn bits(samples: &[f32]) -> Vec<u32> {
    samples.iter().map(|v| v.to_bits()).collect()
}

#[test]
fn subnormal_input_leaves_no_subnormal_output() {
    for (settings, channels) in [
        (BOTH, 8),
        (
            Settings {
                headphone_surround: true,
                ..OFF
            },
            8,
        ),
        (
            Settings {
                night_mode: true,
                ..OFF
            },
            2,
        ),
    ] {
        let mut chain = Chain::new(channels, 2).unwrap();
        chain
            .set_source(channels, if channels == 8 { SEVEN_ONE } else { 3 })
            .unwrap();
        for i in 0..8 {
            let mut packet: Vec<f32> = (0..240 * channels)
                .map(|n| [1e-40, -1e-39, 5e-31, -f32::MIN_POSITIVE][(n + i) % 4])
                .collect();
            chain.process(&mut packet, settings);
            assert!(
                packet.iter().all(|v| v.to_bits() == 0),
                "{settings:?} packet {i}"
            );
        }
    }
}

/// `cargo test --release ... audio_dsp -- --ignored --nocapture bench`
#[test]
#[ignore]
fn bench() {
    let packets = 4000;
    let mut report = Vec::new();
    for (name, capture, output, settings) in [
        (
            "night mode, stereo",
            2,
            2,
            Settings {
                night_mode: true,
                ..OFF
            },
        ),
        (
            "headphone surround, 7.1 to 2",
            8,
            2,
            Settings {
                headphone_surround: true,
                ..OFF
            },
        ),
        ("both, 7.1 to 2", 8, 2, BOTH),
    ] {
        let mut chain = Chain::new(capture, output).unwrap();
        if capture == 8 {
            chain.set_source(8, SEVEN_ONE).unwrap();
        }
        let input = noise(240 * capture * 64, 8);
        let source: Vec<&[f32]> = input.chunks(240 * capture).collect();
        let mut packet = Vec::with_capacity(240 * capture);
        let mut times = Vec::with_capacity(packets);
        for i in 0..packets {
            packet.clear();
            packet.extend_from_slice(source[i % source.len()]);
            let start = std::time::Instant::now();
            chain.process(&mut packet, settings);
            times.push(start.elapsed().as_secs_f64() * 1e3);
            std::hint::black_box(&packet);
        }
        let mean = times.iter().sum::<f64>() / packets as f64;
        // A busy PC preempts some packets; the median is the work itself.
        times.sort_by(f64::total_cmp);
        let (median, p99) = (times[packets / 2], times[packets * 99 / 100]);
        report.push(format!(
            "{name}: {median:.4} ms median, {mean:.4} ms mean, {p99:.4} ms p99 per 5 ms packet"
        ));
        assert!(median < 0.25, "{name}: {median} ms");
    }
    println!("{}", report.join("\n"));
}
