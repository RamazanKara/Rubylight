use super::*;

const SECOND: usize = 48_000;

fn sine(hz: f32, amplitude: f32, frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|n| amplitude * (std::f32::consts::TAU * hz * n as f32 / RATE).sin())
        .collect()
}

fn analyze(settings: Settings, input: &[(f32, f32)]) -> (Vec<(f32, f32)>, Analyzer) {
    let mut analyzer = Analyzer::default();
    analyzer.set(settings);
    let mut out = vec![];
    analyzer.process(input, &mut out);
    (decode(&out), analyzer)
}

fn decode(stereo: &[u8]) -> Vec<(f32, f32)> {
    stereo
        .as_chunks::<4>()
        .0
        .iter()
        .map(|f| {
            (
                f32::from(i16::from_le_bytes([f[0], f[1]])) / 32767.,
                f32::from(i16::from_le_bytes([f[2], f[3]])) / 32767.,
            )
        })
        .collect()
}

fn rms(samples: impl Iterator<Item = f32>) -> f32 {
    let (sum, n) = samples.fold((0., 0), |(sum, n), x| (sum + x * x, n + 1));
    (sum / n.max(1) as f32).sqrt()
}

fn left(out: &[(f32, f32)]) -> f32 {
    rms(out.iter().map(|f| f.0))
}

fn right(out: &[(f32, f32)]) -> f32 {
    rms(out.iter().map(|f| f.1))
}

fn on() -> Settings {
    Settings {
        enabled: true,
        ..Settings::default()
    }
}

fn mono(signal: &[f32]) -> Vec<(f32, f32)> {
    signal.iter().map(|&x| (x, x)).collect()
}

#[test]
fn low_bass_on_the_left_drives_only_the_left_actuator() {
    let input: Vec<_> = sine(60., 0.1, SECOND / 2)
        .into_iter()
        .map(|x| (x, 0.))
        .collect();
    let (out, _) = analyze(on(), &input);
    let steady = &out[SECOND / 4..];
    // 0.1 of bass is 0.4 of the actuator at 100 %: RMS about 0.28.
    assert!((0.25..0.31).contains(&left(steady)), "{}", left(steady));
    assert_eq!(right(&out), 0.);
}

#[test]
fn treble_does_not_reach_the_actuators() {
    let (bass, _) = analyze(on(), &mono(&sine(60., 0.1, SECOND / 2)));
    let (treble, _) = analyze(on(), &mono(&sine(2_000., 0.1, SECOND / 2)));
    // Past the onset thump at the start.
    assert!(left(&treble[SECOND / 10..]) < left(&bass[SECOND / 4..]) / 100.);
}

#[test]
fn the_lfe_of_5_1_drives_both_actuators_through_the_tap() {
    let tap = Tap::default();
    tap.set_wanted(true);
    let lfe = sine(50., 0.1, KEEP);
    let packet: Vec<f32> = lfe.iter().flat_map(|&x| [0., 0., 0., x, 0., 0.]).collect();
    tap.push(&packet, 6);
    let mut lanes = vec![];
    assert!(tap.take(&mut lanes));
    assert_eq!(lanes.len(), KEEP);
    assert!(
        lanes
            .iter()
            .zip(&lfe)
            .all(|(l, x)| l.0 == 0.7 * x && l.1 == l.0)
    );
    let (out, _) = analyze(on(), &lanes);
    assert!(left(&out) > 0.05);
    assert_eq!(left(&out), right(&out));
}

#[test]
fn the_gate_keeps_quiet_bass_off_the_actuators() {
    let quiet = mono(&sine(60., 0.001, SECOND / 2)); // -60 dBFS
    let loud = mono(&sine(60., 0.01, SECOND / 2)); // -40 dBFS
    let (out, _) = analyze(on(), &quiet);
    assert!(out.iter().all(|&f| f == (0., 0.)));
    let (out, _) = analyze(on(), &loud);
    assert!(left(&out[SECOND / 4..]) > 0.02);
    let open = Settings {
        gate_db: -80,
        ..on()
    };
    let (out, _) = analyze(open, &quiet);
    assert!(left(&out[SECOND / 4..]) > 0.002);
}

#[test]
fn the_curve_and_strength_shape_the_level() {
    let level = |settings, amplitude| {
        let (out, _) = analyze(settings, &mono(&sine(60., amplitude, SECOND / 2)));
        left(&out[SECOND / 4..])
    };
    let ratio = |settings| level(settings, 0.04) / level(settings, 0.02);
    let close = |a: f32, b: f32| (a - b).abs() < 0.03 * b;
    let curve = |curve| Settings { curve, ..on() };
    assert!(close(ratio(curve(Curve::Linear)), 2.));
    assert!(close(ratio(curve(Curve::Soft)), 2f32.sqrt()));
    assert!(close(ratio(curve(Curve::Punchy)), 4.));
    let strength = |strength| Settings { strength, ..on() };
    assert_eq!(level(strength(0), 0.02), 0.);
    assert!(close(level(strength(200), 0.02), 2. * level(on(), 0.02)));
}

/// A kick every 500 ms: a 3 kHz click and a decaying 55 Hz body, panned left.
fn kicks(count: usize) -> Vec<(f32, f32)> {
    let period = SECOND / 2;
    (0..count * period)
        .map(|n| {
            let t = (n % period) as f32 / RATE;
            let click = if t < 0.002 {
                0.3 * (std::f32::consts::TAU * 3_000. * t).sin()
            } else {
                0.
            };
            let body = 0.3 * (std::f32::consts::TAU * 55. * t).sin() * (-t / 0.06).exp();
            (click + body, 0.)
        })
        .collect()
}

#[test]
fn each_kick_is_one_onset_and_its_energy_follows_the_kick() {
    let input = kicks(4);
    let (out, analyzer) = analyze(on(), &input);
    assert_eq!(analyzer.lanes[0].onsets, 4);
    assert_eq!(analyzer.lanes[1].onsets, 0);
    assert_eq!(right(&out), 0.);
    let period = SECOND / 2;
    for kick in 0..4 {
        let start = kick * period;
        let hit = left(&out[start..start + SECOND / 10]);
        let tail = left(&out[start + 3 * SECOND / 10..start + period]);
        assert!(hit > 0.2, "kick {kick}: {hit}");
        assert!(tail < hit / 50., "kick {kick}: {tail} after {hit}");
    }
}

#[test]
fn an_impact_without_bass_still_thumps() {
    let mut input = vec![(0., 0.); SECOND / 10];
    for (n, frame) in input.iter_mut().take(240).enumerate() {
        let x = 0.3 * (std::f32::consts::TAU * 3_000. * n as f32 / RATE).sin();
        *frame = (x, x);
    }
    let (out, analyzer) = analyze(on(), &input);
    assert_eq!(analyzer.lanes[0].onsets, 1);
    assert!(left(&out[..THUMP_FRAMES]) > 0.05);
    // Then only the burst's faint low-band residue, 40 dB down.
    assert!(out[THUMP_FRAMES..].iter().all(|f| f.0.abs() < 0.01));
}

#[test]
fn bad_samples_leave_the_output_bounded_and_recover() {
    let mut input = mono(&sine(60., 0.1, SECOND / 2));
    input[1_000] = (f32::NAN, f32::INFINITY);
    input[1_001] = (f32::NEG_INFINITY, f32::MAX);
    input[1_002] = (f32::MAX, f32::MAX);
    let (out, _) = analyze(on(), &input);
    assert!(out.iter().all(|f| f.0.abs() <= 1. && f.1.abs() <= 1.));
    assert!(left(&out[SECOND / 4..]) > 0.25);
    assert!(right(&out[SECOND / 4..]) > 0.25);
}

#[test]
fn the_tap_copies_only_while_wanted_in_order_and_fresh() {
    let tap = Tap::default();
    let packet = |first: usize, frames: usize| -> Vec<f32> {
        (first..first + frames)
            .flat_map(|n| [n as f32, -(n as f32)])
            .collect()
    };
    tap.push(&packet(0, 240), 2);
    let mut lanes = vec![];
    assert!(tap.take(&mut lanes));
    assert!(lanes.is_empty());
    tap.set_wanted(true);
    tap.push(&packet(0, 240), 2);
    tap.push(&packet(240, 240), 2);
    assert!(tap.take(&mut lanes));
    assert_eq!(lanes.len(), 480);
    assert!(
        lanes
            .iter()
            .enumerate()
            .all(|(n, &l)| l == (n as f32, -(n as f32)))
    );
    // A late pass keeps only the newest 20 ms.
    for first in (0..1_200).step_by(240) {
        tap.push(&packet(first, 240), 2);
    }
    lanes.clear();
    assert!(!tap.take(&mut lanes));
    assert_eq!(lanes.len(), KEEP);
    assert_eq!(lanes[0].0, (1_200 - KEEP) as f32);
    // A stalled consumer: the full ring is dropped, then copying resumes.
    for first in (0..RING + 240).step_by(240) {
        tap.push(&packet(first, 240), 2);
    }
    lanes.clear();
    assert!(!tap.take(&mut lanes));
    assert!(lanes.is_empty());
    tap.push(&packet(7, 240), 2);
    assert!(tap.take(&mut lanes));
    assert_eq!(lanes.len(), 240);
    assert_eq!(lanes[0].0, 7.);
    // Stopped and restarted: nothing from before the restart.
    tap.set_wanted(false);
    tap.push(&packet(0, 240), 2);
    tap.set_wanted(true);
    lanes.clear();
    assert!(tap.take(&mut lanes));
    assert!(lanes.is_empty());
}

fn arrival(id: u8, capabilities: u16) -> Input {
    Input::Arrival {
        id,
        kind: 2,
        capabilities,
        buttons: 0,
    }
}

/// A 5 ms bass packet of game audio, stereo.
fn audio(tap: &Tap) {
    let packet: Vec<f32> = sine(60., 0.1, 240)
        .into_iter()
        .flat_map(|x| [x, x])
        .collect();
    tap.push(&packet, 2);
}

fn real(id: u16, sequence: u32, level: i16) -> haptics::Frame {
    haptics::Frame {
        id,
        sequence,
        created: Instant::now(),
        samples: [level.to_le_bytes(); 2 * haptics::FRAMES].concat(),
    }
}

/// Runs `passes` 5 ms passes of game audio from `at`, returning the frames.
fn run(arbiter: &mut Arbiter, tap: &Tap, at: &mut Instant, passes: usize) -> Vec<haptics::Frame> {
    let mut frames = vec![];
    for _ in 0..passes {
        audio(tap);
        *at += Duration::from_millis(5);
        frames.extend(arbiter.generate(true, tap, *at));
    }
    frames
}

fn generating_arbiter() -> (Arbiter, Tap, Instant) {
    let mut arbiter = Arbiter::default();
    arbiter.set_settings(on());
    arbiter.input(&arrival(0, haptics::CAPABILITY));
    let tap = Tap::default();
    let now = Instant::now();
    assert!(arbiter.generate(true, &tap, now).is_empty());
    assert!(tap.wanted());
    (arbiter, tap, now)
}

#[test]
fn generated_frames_go_only_to_waveform_controllers_in_sequence() {
    let (mut arbiter, tap, mut at) = generating_arbiter();
    arbiter.input(&arrival(1, 0));
    let frames = run(&mut arbiter, &tap, &mut at, 40);
    assert!(frames.len() > 30);
    assert!(frames.iter().all(|f| f.id == 0 && f.payload().is_some()));
    assert!(
        frames
            .iter()
            .zip(0..)
            .all(|(f, sequence)| f.sequence == sequence)
    );
    assert_eq!(arbiter.next_due(), Some(at + Duration::from_millis(5)));
    // The controller left: no more frames, and the audio thread stops copying.
    arbiter.input(&Input::Controller {
        id: 0,
        active: 0b10,
        buttons: 0,
        left_trigger: 0,
        right_trigger: 0,
        sticks: [0; 4],
    });
    assert!(run(&mut arbiter, &tap, &mut at, 4).is_empty());
    assert!(!tap.wanted());
    assert_eq!(arbiter.next_due(), None);
}

#[test]
fn a_peer_that_goes_away_stops_the_copy_only_if_it_wanted_it() {
    let (mut arbiter, tap, _) = generating_arbiter();
    let mut idle = Arbiter::default();
    idle.release(&tap);
    assert!(tap.wanted());
    arbiter.release(&tap);
    assert!(!tap.wanted());
    assert_eq!(arbiter.next_due(), None);
}

#[test]
fn the_games_own_waveform_stops_audio_haptics_at_once_and_continues_the_sequence() {
    let (mut arbiter, tap, mut at) = generating_arbiter();
    let generated = run(&mut arbiter, &tap, &mut at, 20);
    let last = generated.last().unwrap().sequence;
    // An idle actuator endpoint's silence is dropped while audio haptics play.
    assert!(!arbiter.game_frame(&mut real(0, 5, 0), at));
    // The game's first real haptics: sent, numbered after the generated frames.
    let mut frame = real(0, 5, 3_000);
    assert!(arbiter.game_frame(&mut frame, at));
    assert_eq!(frame.sequence, last + 1);
    assert!(run(&mut arbiter, &tap, &mut at, 20).is_empty());
    assert!(!tap.wanted());
    // The pad's later frames, silent or not, keep the corrected numbering.
    let mut frame = real(0, 6, 0);
    assert!(arbiter.game_frame(&mut frame, at));
    assert_eq!(frame.sequence, last + 2);
    // Long after the game's last haptics, audio haptics resume after it.
    at += GAME_HOLD;
    let resumed = run(&mut arbiter, &tap, &mut at, 20);
    assert_eq!(resumed.first().unwrap().sequence, last + 3);
}

#[test]
fn rumble_stops_audio_haptics_but_a_stop_command_does_not() {
    let (mut arbiter, tap, mut at) = generating_arbiter();
    assert!(!run(&mut arbiter, &tap, &mut at, 10).is_empty());
    let rumble = |motors: [u8; 4]| [&0x00c0ffeeu32.to_le_bytes()[..], &[0, 0], &motors].concat();
    arbiter.game_feedback(RUMBLE, &rumble([0; 4]), at);
    arbiter.game_feedback(TRIGGER_RUMBLE, &[0, 0, 0, 0, 0, 0], at);
    arbiter.game_feedback(0x5503, &[0, 0, 1, 1], at);
    assert!(!run(&mut arbiter, &tap, &mut at, 2).is_empty());
    arbiter.game_feedback(RUMBLE, &rumble([0, 0, 0, 0x40]), at);
    assert!(run(&mut arbiter, &tap, &mut at, 10).is_empty());

    let (mut arbiter, tap, mut at) = generating_arbiter();
    arbiter.game_feedback(TRIGGER_RUMBLE, &[0, 0, 0x20, 0, 0, 0], at);
    assert!(run(&mut arbiter, &tap, &mut at, 10).is_empty());
}

#[test]
fn sequences_stay_newer_across_the_wrap() {
    let (mut arbiter, tap, mut at) = generating_arbiter();
    // The client last saw a real frame just before the wrap.
    arbiter.generate(false, &tap, at);
    assert!(arbiter.game_frame(&mut real(0, u32::MAX - 1, 0), at));
    arbiter.generate(true, &tap, at);
    let frames = run(&mut arbiter, &tap, &mut at, 10);
    assert_eq!(frames[0].sequence, u32::MAX);
    assert_eq!(frames[1].sequence, 0);
    // The pad's counter is far behind the generated ones.
    let last = frames.last().unwrap().sequence;
    let mut frame = real(0, u32::MAX - 50, 3_000);
    assert!(arbiter.game_frame(&mut frame, at));
    assert_eq!(frame.sequence, last + 1);
    assert!(newer(frame.sequence, last));
}

#[test]
fn with_audio_haptics_off_game_frames_pass_untouched_and_nothing_is_copied() {
    let mut arbiter = Arbiter::default();
    arbiter.input(&arrival(0, haptics::CAPABILITY));
    let tap = Tap::default();
    let mut at = Instant::now();
    assert!(run(&mut arbiter, &tap, &mut at, 10).is_empty());
    assert!(!tap.wanted());
    for (sequence, level) in [(7, 0), (3, 3_000), (2, 0)] {
        let mut frame = real(0, sequence, level);
        assert!(arbiter.game_frame(&mut frame, at));
        assert_eq!(frame.sequence, sequence);
    }
    // Not allowed for the peer (rumble forwarding off, haptics not negotiated).
    arbiter.set_settings(on());
    assert!(arbiter.generate(false, &tap, at).is_empty());
    assert!(!tap.wanted());
}

#[test]
fn settings_apply_live_between_packets() {
    let (mut arbiter, tap, mut at) = generating_arbiter();
    let peak = |frames: &[haptics::Frame]| {
        frames
            .iter()
            .flat_map(|f| decode(&f.samples))
            .map(|s| s.0.abs())
            .fold(0., f32::max)
    };
    let full = peak(&run(&mut arbiter, &tap, &mut at, 40)[20..]);
    arbiter.set_settings(Settings {
        strength: 50,
        ..on()
    });
    let half = peak(&run(&mut arbiter, &tap, &mut at, 40)[2..]);
    assert!((half - full / 2.).abs() < 0.02, "{half} vs {full}");
    arbiter.set_settings(Settings {
        gate_db: -10,
        ..on()
    });
    let gated = run(&mut arbiter, &tap, &mut at, 40);
    assert!(gated.len() < 10, "the gate closes within 30 ms");
    arbiter.set_settings(Settings::default());
    assert!(run(&mut arbiter, &tap, &mut at, 1).is_empty());
    assert!(!tap.wanted());
}

#[test]
fn settings_come_from_the_config_and_bad_values_are_refused() {
    let mut config = Config::default();
    assert_eq!(Settings::from_config(&config).unwrap(), Settings::default());
    for (key, value) in [
        (ENABLED, "true"),
        (STRENGTH, "150"),
        (CURVE, "Punchy"),
        (GATE, "-30"),
    ] {
        config.values.insert(key.into(), value.into());
    }
    assert_eq!(
        Settings::from_config(&config).unwrap(),
        Settings {
            enabled: true,
            strength: 150,
            curve: Curve::Punchy,
            gate_db: -30,
        }
    );
    for (key, value) in [
        (ENABLED, "maybe"),
        (STRENGTH, "201"),
        (STRENGTH, "-1"),
        (CURVE, "loud"),
        (GATE, "-9"),
        (GATE, "-81"),
    ] {
        assert!(check(key, value).unwrap_err().to_string().contains(key));
        let mut bad = config.clone();
        bad.values.insert(key.into(), value.into());
        assert!(Settings::from_config(&bad).is_err());
    }
}
