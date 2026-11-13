use super::*;
#[test]
fn opus_layouts_preserve_legacy_surround_quality_and_validate_custom_mapping() {
    for (channels, streams, coupled, bitrate) in
        [(2, 1, 1, 512000), (6, 6, 0, 1536000), (8, 8, 0, 2048000)]
    {
        let layout = OpusLayout::select(channels, true, None).unwrap();
        assert_eq!(
            (layout.streams, layout.coupled, layout.bitrate),
            (streams, coupled, bitrate)
        );
    }
    let layout = OpusLayout::select(6, false, Some("660012345")).unwrap();
    assert_eq!((layout.streams, layout.coupled), (6, 0));
    assert_eq!(layout.mapping, [0, 1, 2, 3, 4, 5]);
    for invalid in [
        "600012345",
        "606012345",
        "642012346",
        "88001234567",
        "64201💥",
    ] {
        assert_eq!(
            OpusLayout::select(6, false, Some(invalid)).unwrap(),
            OpusLayout::select(6, false, None).unwrap()
        );
    }
}
/// The decoded channel each input channel takes, by RFC 7845 section 5.1.1: coupled
/// stream `j` decodes to channels `2j` and `2j + 1`, mono stream `k` to `M + k`.
fn decoded_channels(layout: &OpusLayout) -> Vec<(usize, Option<usize>)> {
    let coupled = layout.coupled as usize;
    layout
        .mapping
        .iter()
        .map(|&index| {
            let index = usize::from(index);
            if index < 2 * coupled {
                (index / 2, Some(index % 2))
            } else {
                (index - coupled, None)
            }
        })
        .collect()
}
#[test]
fn seven_one_four_layouts_are_foundations_rfc_7845_mapping_tables() {
    for (quality, streams, coupled, bitrate) in [
        (false, 8, 4, audio_ext::BITRATE_714),
        (true, 12, 0, audio_ext::BITRATE_714_HQ),
    ] {
        let layout = OpusLayout::select(12, quality, None).unwrap();
        assert_eq!(
            (layout.channels, layout.streams, layout.coupled),
            (12, streams, coupled)
        );
        assert_eq!(layout.bitrate, bitrate as i32);
        // Family 255 (RFC 7845 5.1.1.4): N + M decoded channels, each entry below it,
        // and here every decoded channel used once, so nothing is duplicated or lost.
        let mut seen = layout.mapping.clone();
        seen.sort_unstable();
        assert_eq!(seen, (0..12).collect::<Vec<u8>>(), "{quality}");
        // It matches what DESCRIBE offers.
        let offered = if quality {
            audio_ext::LAYOUT_714_HQ
        } else {
            audio_ext::LAYOUT_714
        };
        assert_eq!(layout.mapping, offered.mapping);
        let decoded = decoded_channels(&layout);
        if !quality {
            // Foundation Sunshine's 600 kb/s layout, which V+ clients assume without
            // parameters: FL FR, FC LFE, BL BR and SL SR share coupled streams in that
            // order, and the four heights are mono.
            for (stream, pair) in decoded[..8].chunks(2).enumerate() {
                assert_eq!(pair, [(stream, Some(0)), (stream, Some(1))], "{stream}");
            }
            for (height, decoded) in decoded.iter().enumerate().skip(8) {
                assert_eq!(*decoded, (height - 4, None), "{height}");
            }
            assert_eq!(layout.bitrate, 600_000);
        }
    }
    // surroundParams is single digits, so a custom 12-channel layout cannot be named.
    assert!(!OpusLayout::valid_custom("12750110"));
}
#[test]
fn five_one_is_rfc_7845_family_one() {
    // Family 1's 5.1 (RFC 7845 5.1.1.2): 4 streams, 2 coupled, mapping 0 4 1 2 3 5 in
    // Vorbis order L C R RL RR LFE. Moonlight orders FL FR FC LFE BL BR.
    let family_one = [0u8, 4, 1, 2, 3, 5];
    let vorbis_of_moonlight = [0, 2, 1, 5, 3, 4];
    let layout = OpusLayout::select(6, false, None).unwrap();
    assert_eq!((layout.streams, layout.coupled), (4, 2));
    let reordered: Vec<u8> = vorbis_of_moonlight
        .iter()
        .map(|&vorbis| family_one[vorbis])
        .collect();
    assert_eq!(layout.mapping, reordered);
}
#[test]
fn seven_one_four_capture_keeps_every_speaker_and_invents_no_heights() {
    let mask = audio_ext::CHANNEL_MASK_714;
    let matrix = mix_matrix(12, mask, 12).unwrap();
    // The endpoint's channels are in bit order: FL FR FC LFE BL BR SL SR TFL TFR TBL
    // TBR is also Moonlight's 7.1.4 order, so this is the identity.
    for (output, row) in matrix.iter().enumerate() {
        for (input, gain) in row.iter().enumerate() {
            assert_eq!(*gain, if input == output { 1. } else { 0. });
        }
    }
    // A 7.1 endpoint feeds the bed and leaves the heights silent.
    let matrix = mix_matrix(8, 0x63f, 12).unwrap();
    for (output, row) in matrix.iter().enumerate() {
        for (input, gain) in row.iter().enumerate() {
            assert_eq!(*gain, if input == output { 1. } else { 0. }, "{output}");
        }
    }
    assert!(matrix[8..].iter().flatten().all(|v| *v == 0.));
    // A two-channel endpoint of back heights has no back pair to fold them into
    // in a stereo client: its left and right take them.
    let matrix = mix_matrix(2, 0x8000 | 0x20000, 2).unwrap();
    assert!(matrix[0][0] > 0. && matrix[1][1] > 0.);
    assert_eq!((matrix[0][1], matrix[1][0]), (0., 0.));
    // Stereo goes to the fronts only.
    let matrix = mix_matrix(2, 3, 12).unwrap();
    assert_eq!((matrix[0][0], matrix[1][1]), (1., 1.));
    assert!(matrix[2..].iter().flatten().all(|v| *v == 0.));
    // 7.1 clients of a 7.1.4 endpoint keep their bed and hear each height
    // from the speaker below it at -3 dB, scaled so nothing clips. One
    // factor scales the whole bed, so the center and the side pair stay as
    // loud as the fronts.
    let matrix = mix_matrix(12, mask, 8).unwrap();
    let bed = 1. / (1. + 0.70710677);
    for (output, row) in matrix.iter().enumerate() {
        let below = [0, 1, 4, 5].iter().position(|&o| o == output);
        assert!((row[output] - bed).abs() < 1e-6, "{output}");
        if let Some(height) = below {
            assert!(
                (row[8 + height] - 0.70710677 * bed).abs() < 1e-6,
                "{output}"
            );
        }
        assert!(row.iter().sum::<f32>() <= 1. + 1e-6);
        assert_eq!(
            row.iter().filter(|v| **v != 0.).count(),
            1 + below.map_or(0, |_| 1)
        );
    }
    // 5.1 and stereo clients keep the heights too, front ones in front.
    let matrix = mix_matrix(12, mask, 6).unwrap();
    assert!(matrix[0][8] > 0. && matrix[1][9] > 0. && matrix[4][10] > 0. && matrix[5][11] > 0.);
    let level = |row: &[f32]| row.iter().sum::<f32>();
    for row in &matrix {
        assert!(level(row) <= 1. + 1e-6);
    }
    // The fronts and the center keep their relative level.
    assert!((matrix[0][0] - matrix[2][2]).abs() < 1e-6);
    assert!((level(&matrix[0]) - level(&matrix[4])).abs() < 1e-6);
    let matrix = mix_matrix(12, mask, 2).unwrap();
    assert!(matrix[0][8] > 0. && matrix[0][10] > 0. && matrix[0][9] == 0.);
    assert!(matrix[1][9] > 0. && matrix[1][11] > 0. && matrix[1][8] == 0.);
    for row in &matrix {
        assert!(row.iter().sum::<f32>() <= 1. + 1e-6);
    }
    assert!(Resampler::new(48000, 12).is_ok());
    assert!(Resampler::new(48000, 10).is_err());
}
#[test]
fn a_48_khz_resampler_at_phase_zero_returns_the_queued_prefix() {
    let mut r = Resampler::new(48000, 2).unwrap();
    for i in 0..100 {
        r.push(&[i as f32 / 100., -(i as f32) / 100.]);
    }
    assert_eq!(r.read(0), Some(Vec::new()));
    assert_eq!(r.read(101), None);
    let out = r.read(10).unwrap();
    assert_eq!(out.len(), 20);
    for (i, frame) in out.chunks(2).enumerate() {
        assert_eq!(frame, [i as f32 / 100., -(i as f32) / 100.]);
    }
    // The read consumed exactly its prefix; the next one starts at frame 10.
    assert_eq!(r.queue.len(), 90 * 2);
    let out = r.read(90).unwrap();
    assert_eq!(out[0], 0.1);
    assert_eq!(out[out.len() - 2], 0.99);
    assert_eq!(r.read(1), None);
}
#[test]
fn fractional_rates_do_not_overrun_and_remain_bounded() {
    for rate in [8000, 44100, 48000, 96000, 192000] {
        let mut r = Resampler::new(rate, 2).unwrap();
        for i in 0..10000 {
            r.push(&[0.25, -0.25]);
            if i % 7 == 0
                && let Some(b) = r.read(240)
            {
                assert!(b.chunks(2).all(|f| f == [0.25, -0.25]));
            }
        }
        assert!(r.queue.len() <= rate as usize / 10 * 2);
    }
}
#[test]
fn a_backlog_after_a_late_tick_is_dropped_back_to_two_packets() {
    let mut r = Resampler::new(48000, 2).unwrap();
    for i in 0..48 * 50 {
        r.push(&[i as f32 / 1e6, 0.]);
    }
    // 50 ms queued: within a 60 ms limit nothing is dropped.
    assert_eq!(r.bound(60, 10), 0);
    // 50 ms down to 10 ms drops 40 ms.
    assert_eq!(r.bound(30, 10), 48 * 40);
    assert_eq!(r.queue.len() / 2, 480);
    // The newest audio is kept.
    let last = r.read(480).unwrap();
    assert_eq!(last[last.len() - 2], (48 * 50 - 1) as f32 / 1e6);
}
#[test]
fn the_hundred_millisecond_cap_reports_the_frames_it_drops() {
    for rate in [44100u32, 48000] {
        let mut r = Resampler::new(rate, 2).unwrap();
        let cap = rate as usize / 10;
        let dropped: usize = (0..cap + 300).map(|_| r.push(&[0.1, 0.1])).sum();
        assert_eq!(dropped, 300, "{rate} Hz");
        assert_eq!(r.queue.len() / 2, cap);
    }
}
#[test]
fn host_audio_loss_counts_only_skips_a_late_read_explains() {
    let ms = Duration::from_millis;
    let buffer = ms(22);
    let start = Instant::now();
    let none = Duration::ZERO;
    let mut loss = HostLoss::default();
    // A skip with no late read before it is the endpoint going quiet and
    // starting again.
    assert!(!loss.read(ms(10), Some(ms(10)), buffer));
    assert!(!loss.read(ms(5), Some(ms(480)), buffer));
    // The idle host's case: reads up to 27 ms apart, but Windows skipped
    // nothing because both 10 ms packets still fit, or nothing played.
    for waited in [10, 23, 26, 27] {
        assert!(!loss.read(ms(waited), Some(none), buffer));
        for _ in 0..5 {
            assert!(!loss.read(ms(5), None, buffer));
        }
    }
    assert_eq!(loss.report(start), None);
    // The host's 40 ms stall: the late read finds the packets that fit,
    // and the skip arrives a read or two later.
    assert!(!loss.read(ms(43), Some(none), buffer));
    assert!(!loss.read(ms(5), None, buffer));
    assert!(loss.read(ms(1), Some(ms(18)), buffer));
    // Counted once.
    assert!(!loss.read(ms(5), Some(ms(10)), buffer));
    // A skip on the late read itself counts too, unless it is longer
    // than the wait.
    assert!(loss.read(ms(52), Some(ms(30)), buffer));
    assert!(!loss.read(ms(30), Some(ms(480)), buffer));
    loss.unsent();
    assert_eq!(
        loss.report(start),
        Some(HostLossReport {
            late_reads: 2,
            lost: ms(48),
            longest: ms(52),
            buffer,
            unsent: 1,
        })
    );
    // A late read explains skips for one buffer after it, not later.
    assert!(!loss.read(ms(40), Some(none), buffer));
    assert!(!loss.read(ms(10), Some(none), buffer));
    assert!(!loss.read(ms(10), None, buffer));
    assert!(!loss.read(ms(5), Some(ms(10)), buffer));
    // Further loss waits five seconds and then reports only what is new.
    loss.unsent();
    assert_eq!(loss.report(start + ms(4999)), None);
    loss.unsent();
    assert_eq!(
        loss.report(start + ms(5000)),
        Some(HostLossReport {
            late_reads: 0,
            lost: Duration::ZERO,
            longest: Duration::ZERO,
            buffer: Duration::ZERO,
            unsent: 2,
        })
    );
    assert_eq!(loss.report(start + ms(20000)), None);
}
#[test]
fn host_audio_loss_blames_the_cause_it_saw() {
    let ms = Duration::from_millis;
    let late = HostLossReport {
        late_reads: 1,
        lost: ms(10),
        longest: ms(32),
        buffer: ms(22),
        unsent: 0,
    };
    let text = late.message();
    assert!(text.contains("Windows dropped 10 ms of captured sound because the audio sender read late 1 time (longest wait 32.0 ms; Windows keeps 22.0 ms)"), "{text}");
    assert!(text.contains("not by video bitrate"), "{text}");
    assert!(!text.contains("lower the video bitrate"), "{text}");
    let unsent = HostLossReport {
        late_reads: 0,
        lost: Duration::ZERO,
        longest: Duration::ZERO,
        buffer: Duration::ZERO,
        unsent: 3,
    };
    let text = unsent.message();
    assert!(
        text.contains("the network adapter refused 3 audio packets"),
        "{text}"
    );
    assert!(text.contains("lower the video bitrate"), "{text}");
    assert!(!text.contains("late"), "{text}");
}
#[test]
fn surround_upmix_does_not_duplicate_right_channel() {
    let m = mix_matrix(2, 3, 8).unwrap();
    assert_eq!(m[0], [1., 0.]);
    assert_eq!(m[1], [0., 1.]);
    assert!(m[2..].iter().flatten().all(|v| *v == 0.));
    let m = mix_matrix(6, 0x60f, 6).unwrap();
    assert_eq!(m[4][4], 1.);
    assert_eq!(m[5][5], 1.);
    assert!(
        mix_matrix(6, 0x3f, 2)
            .unwrap()
            .iter()
            .all(|r| r.iter().sum::<f32>() <= 1.00001)
    );
}
#[test]
fn seven_point_one_downmix_preserves_every_channel_for_a_five_point_one_client() {
    let matrix = mix_matrix(8, 0x63f, 6).unwrap();
    // Isolate each input speaker, as a speaker-test tone would. Fronts,
    // center and LFE stay discrete; each back/side pair shares one
    // surround output, without crossing left and right.
    for (source, destination) in [0, 1, 2, 3, 4, 5, 4, 5].into_iter().enumerate() {
        let gain = if source < 4 { 1. } else { 0.5 };
        for (output, row) in matrix.iter().enumerate() {
            assert_eq!(row[source], if output == destination { gain } else { 0. });
        }
    }
    // Correlated full-scale surround channels retain headroom.
    assert!(matrix.iter().all(|row| row.iter().sum::<f32>() == 1.));
    // Native 5.1 endpoints keep unity gain with either speaker mask.
    for mask in [0x3f, 0x60f] {
        let matrix = mix_matrix(6, mask, 6).unwrap();
        for (channel, row) in matrix.iter().enumerate() {
            assert_eq!(row[channel], 1.);
            assert_eq!(row.iter().sum::<f32>(), 1.);
        }
    }
}
