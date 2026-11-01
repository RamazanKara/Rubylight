use super::*;

#[test]
fn datagrams_match_the_apollo_client_layout() {
    let key = [7; 16];
    let datagram = seal(&key, 0xffff_fffe, 3, 0x0102_0304, b"opus frame");
    // flags, type, LE sequence, LE timestamp, LE magic.
    assert_eq!(
        &datagram[..HEADER],
        &[0, 0x61, 3, 0, 4, 3, 2, 1, 0x78, 0x56, 0x34, 0x12]
    );
    // The packet padded to a block, and a block of padding after it.
    assert_eq!(datagram.len(), HEADER + 32);
    let packet = parse(&datagram).unwrap();
    assert_eq!((packet.sequence, packet.timestamp), (3, 0x0102_0304));
    // The key ID and sequence add with wrap-around, as in C.
    let mut iv = [0; 16];
    iv[..4].copy_from_slice(&1u32.to_be_bytes());
    let mut inner = b"opus frame".to_vec();
    inner.extend([6; 6]);
    assert_eq!(crypto::cbc_open(&key, &iv, packet.payload).unwrap(), inner);
    assert_eq!(open(&key, 0xffff_fffe, &packet).unwrap(), b"opus frame");
    // Ciphertext from moonlight-common-c's own PltEncryptMessage (OpenSSL),
    // key 07.., key ID 9, sequence 70, packet bytes 0x20, 0x21, ...
    for (length, hex) in [
        (
            4,
            "306a36d0512bb803ccc69a69ae7e2aed501d20abae2a575017bdb0ca2cf32fc3",
        ),
        (
            16,
            "1e10689ae2b276383d9f36756341ba398f6060d44c12333163d5a710f9c2c20e",
        ),
        (
            33,
            "1e10689ae2b276383d9f36756341ba3996287332b3430f8ba7119c451d482a13c19f4f2afd4574a761afc77c968ba6025e91b8ea56f889d013870cd329a41bd6",
        ),
    ] {
        let opus: Vec<u8> = (0x20..0x20 + length).collect();
        let datagram = seal(&key, 9, 70, 0, &opus);
        assert_eq!(hex::encode(&datagram[HEADER..]), hex, "{length}");
        assert_eq!(open(&key, 9, &parse(&datagram).unwrap()).unwrap(), opus);
    }
    // A client that pads once.
    let once = Packet {
        sequence: 70,
        timestamp: 0,
        payload: &crypto::cbc_seal(&key, &super::iv(9, 70), &[1, 2, 3]),
    };
    assert_eq!(open(&key, 9, &once).unwrap(), [1, 2, 3]);
    assert_ne!(
        open(&[8; 16], 0xffff_fffe, &packet).ok().as_deref(),
        Some(&b"opus frame"[..])
    );
}

#[test]
fn other_datagrams_are_not_microphone_packets() {
    let datagram = seal(&[1; 16], 0, 0, 0, b"x");
    assert!(parse(&datagram[..HEADER]).is_none());
    let mut wrong_type = datagram.clone();
    wrong_type[1] = 0x60;
    assert!(parse(&wrong_type).is_none());
    let mut wrong_magic = datagram.clone();
    wrong_magic[8] ^= 1;
    assert!(parse(&wrong_magic).is_none());
    for payload in [&[0; 15][..], &[]] {
        let packet = Packet {
            sequence: 0,
            timestamp: 0,
            payload,
        };
        assert!(open(&[1; 16], 0, &packet).is_err());
    }
}

#[test]
fn the_offer_is_what_clients_look_for() {
    let sdp = sdp(48001);
    assert!(sdp.starts_with("m=audio 48001 RTP/AVP 96\r\n"));
    assert!(sdp.contains("a=rtpmap:96 opus/48000/1\r\n"));
}

#[test]
fn gaps_are_concealed_late_packets_dropped_and_restarts_followed() {
    let mut s = Sequencer::default();
    assert_eq!(s.arrive(65534), Arrival::Restart);
    assert_eq!(s.arrive(65535), Arrival::Next);
    // Wrap-around is the next packet.
    assert_eq!(s.arrive(0), Arrival::Next);
    assert_eq!(s.arrive(0), Arrival::Late);
    assert_eq!(s.arrive(65535), Arrival::Late);
    assert_eq!(s.arrive(3), Arrival::AfterLoss { missing: 2 });
    // A packet that was concealed and then arrives is late.
    assert_eq!(s.arrive(2), Arrival::Late);
    assert_eq!(s.arrive(4), Arrival::Next);
    assert_eq!(
        s.arrive(4 + 1 + MAX_CONCEALED),
        Arrival::AfterLoss {
            missing: MAX_CONCEALED
        }
    );
    assert_eq!(s.arrive(100), Arrival::Restart);
    assert_eq!(s.arrive(101), Arrival::Next);
    // A client that counts from 0 again is followed after a short run.
    for n in 0..LATE_RESTART - 1 {
        assert_eq!(s.arrive(n as u16), Arrival::Late);
    }
    assert_eq!(s.arrive(LATE_RESTART as u16 - 1), Arrival::Restart);
    assert_eq!(s.arrive(LATE_RESTART as u16), Arrival::Next);
}

#[test]
fn the_queue_stays_within_three_packets_and_keeps_the_newest() {
    let mut p = Playout::default();
    let packet: Vec<f32> = (0..960).map(|n| n as f32 / 960.).collect();
    for _ in 0..3 {
        p.push(&packet);
    }
    assert_eq!((p.len(), p.trimmed), (2880, 0));
    p.push(&packet);
    // Cut back to one packet, the newest.
    assert_eq!((p.len(), p.trimmed), (960, 2880));
    let mut out = vec![0.; 1000];
    assert_eq!(p.take(&mut out), 960);
    assert_eq!(out[959], 959. / 960.);
    assert!(p.is_empty());
    // Small packets still get 60 ms before anything goes.
    for _ in 0..12 {
        p.push(&[2.; 240]);
    }
    assert_eq!((p.len(), p.trimmed), (2880, 2880));
    assert_eq!(p.take(&mut out[..1]), 1);
    assert_eq!(out[0], 1., "samples are clamped");
}

#[test]
fn concealment_batches_keep_the_individual_packet_latency_bound() {
    for packet_frames in [240, 960, 5760] {
        let mut playout = Playout::default();
        let count = (6 * packet_frames).max(RATE * 80 / 1000);
        let batch: Vec<_> = (0..count).map(|i| i as f32 / count as f32).collect();
        playout.push_batch(&batch, packet_frames);
        let keep = packet_frames.max(RATE * 20 / 1000);
        assert_eq!(playout.len(), keep);
        assert_eq!(playout.trimmed, (count - keep) as u64);
        let mut newest = vec![0.; keep];
        assert_eq!(playout.take(&mut newest), keep);
        assert_eq!(newest, batch[count - keep..]);
    }
}

/// What a render pull drains: the playout, or the bound it replaced, so
/// the two can be compared.
trait Queue: Default {
    fn push_batch(&mut self, samples: &[f32], packet_frames: usize);
    fn pull(&mut self, buffer: usize, padding: usize, period: usize, out: &mut Vec<f32>) -> usize;
    fn len(&self) -> usize;
}
impl Queue for Playout {
    fn push_batch(&mut self, samples: &[f32], packet_frames: usize) {
        Playout::push_batch(self, samples, packet_frames);
    }
    fn pull(&mut self, buffer: usize, padding: usize, period: usize, out: &mut Vec<f32>) -> usize {
        Playout::pull(self, buffer, padding, period, out)
    }
    fn len(&self) -> usize {
        Playout::len(self)
    }
}
/// The queue before: above three packets or 60 ms, cut to one packet
/// (at least 20 ms), and the device fed whatever is there.
#[derive(Default)]
struct Bounded(VecDeque<f32>);
impl Queue for Bounded {
    fn push_batch(&mut self, samples: &[f32], packet_frames: usize) {
        self.0.extend(samples);
        if self.0.len() > (3 * packet_frames).max(ms(60)) {
            let drop = self.0.len() - packet_frames.max(ms(20));
            self.0.drain(..drop);
        }
    }
    fn pull(&mut self, buffer: usize, padding: usize, period: usize, out: &mut Vec<f32>) -> usize {
        let (samples, silence) = render_plan(buffer, padding, period, self.0.len());
        out.clear();
        out.extend(self.0.drain(..samples));
        silence
    }
    fn len(&self) -> usize {
        self.0.len()
    }
}
struct Run<Q> {
    queue: Q,
    /// Everything the device played.
    played: Vec<f32>,
    /// The mean of what the device and the queue held at the pulls
    /// after `from`, in ms: how far playback is behind arrival.
    delay: f64,
}
/// A device of `period` samples in a buffer of two, pulled every period
/// until `until` (µs), fed [`tone`]: `deliveries` are (µs, samples),
/// ascending, in packets of `packet` (more is concealment before one).
/// `each` sees the queue after every push and pull, with the time.
fn simulate<Q: Queue>(
    deliveries: &[(u64, usize)],
    packet: usize,
    period: usize,
    until: u64,
    from: u64,
    mut each: impl FnMut(u64, &Q),
) -> Run<Q> {
    let mut queue = Q::default();
    let (mut played, mut out) = (Vec::new(), Vec::new());
    let (mut padding, mut sent) = (0, 0);
    let (mut delay, mut pulls) = (0, 0);
    let mut deliveries = deliveries.iter().copied().peekable();
    for now in (0..until).step_by(period * 1_000_000 / RATE) {
        while let Some((at, frames)) = deliveries.next_if(|&(at, _)| at < now) {
            let samples: Vec<f32> = (sent..sent + frames).map(tone).collect();
            sent += frames;
            queue.push_batch(&samples, packet);
            each(at, &queue);
        }
        padding = usize::saturating_sub(padding, period);
        if now >= from {
            delay += queue.len() + padding;
            pulls += 1;
        }
        let silence = queue.pull(2 * period, padding, period, &mut out);
        played.extend(&out);
        played.extend(std::iter::repeat_n(0., silence));
        padding += out.len() + silence;
        each(now, &queue);
    }
    let delay = delay as f64 / pulls as f64 * 1000. / RATE as f64;
    Run {
        queue,
        played,
        delay,
    }
}
/// 20 ms packets at `times`.
fn packets(times: impl IntoIterator<Item = u64>) -> Vec<(u64, usize)> {
    times.into_iter().map(|at| (at, 960)).collect()
}
/// After the first `primed` µs: the most the queue held, and whether
/// anything was trimmed or ran dry.
fn after_priming(
    deliveries: &[(u64, usize)],
    until: u64,
    primed: u64,
) -> (Run<Playout>, usize, u64, u64) {
    let mut most = 0;
    let mut at_priming = None;
    let run = simulate(deliveries, 960, 480, until, primed, |now, p: &Playout| {
        if now >= primed {
            at_priming.get_or_insert((p.trimmed, p.underruns));
            most = most.max(p.len());
        }
    });
    let (trimmed, underruns) = at_priming.unwrap();
    let trimmed = run.queue.trimmed - trimmed;
    let underruns = run.queue.underruns - underruns;
    (run, most, trimmed, underruns)
}
/// A small deterministic jitter in 0..=2 * `range` µs.
fn jitter(seed: &mut u32, range: u64) -> u64 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 17;
    *seed ^= *seed << 5;
    u64::from(*seed) % (2 * range + 1)
}

#[test]
fn a_client_that_sends_in_bursts_is_not_cut() {
    // Three packets every 60 ms, as some Android TVs send them.
    let regular = packets((0..500u64).flat_map(|k| [5_000 + k * 60_000; 3]));
    let (run, most, trimmed, underruns) = after_priming(&regular, 30_000_000, 0);
    assert_eq!((trimmed, underruns, run.queue.underrun_samples), (0, 0, 0));
    assert!(most <= ms(60), "{most}");
    // Every sample sent was played.
    assert!(run.played.iter().filter(|s| **s != 0.).count() > 499 * 2880 * 9 / 10);
    // Bursts that are irregular, which a 60 ms bound cut a chunk out of
    // every time: each up to 15 ms early or late, or of two to four
    // packets. Once the queue has learnt the spread, it neither cuts nor
    // runs dry.
    let mut seed = 0x2545_f491;
    let jittered =
        packets((0..500u64).flat_map(|k| [2_000 + k * 60_000 + jitter(&mut seed, 15_000); 3]));
    let sized =
        packets((0..500u64).flat_map(|k| vec![5_000 + k * 60_000; [3, 2, 4, 3][k as usize % 4]]));
    for deliveries in [jittered, sized] {
        let (_, most, trimmed, underruns) = after_priming(&deliveries, 30_000_000, 5_000_000);
        assert_eq!((trimmed, underruns), (0, 0));
        assert!(most <= CAP, "{most}");
    }
}

#[test]
fn a_steady_client_is_played_no_later_than_before() {
    let mut seed = 0x1234_5678;
    let straddling = packets((0..1500u64).map(|k| 8_000 + k * 20_000 + jitter(&mut seed, 2_000)));
    let short: Vec<_> = (0..3000u64)
        .map(|k| (3_000 + k * 10_000 + jitter(&mut seed, 1_500), 480))
        .collect();
    let late = packets((0..1500u64).map(|k| 5_000 + k * 20_000 + if k == 150 { 8_000 } else { 0 }));
    let muted = packets(
        (0..1500u64)
            .filter(|k| !(150..180).contains(k))
            .map(|k| 5_000 + k * 20_000),
    );
    // 100 ms with nothing, then its packets at once: once, and every 4 s.
    let stall = |every: u64| {
        packets((0..1500u64).map(move |k| {
            let at = 5_000 + k * 20_000;
            let phase = at % every;
            if (3_000_000..3_100_000).contains(&phase) {
                at - phase + 3_100_000
            } else {
                at
            }
        }))
    };
    // Three or five packets lost and concealed in front of the next one.
    let concealed = |lost: u64| -> Vec<(u64, usize)> {
        (0..1500u64)
            .filter(|k| !(150..150 + lost).contains(k))
            .map(|k| {
                (
                    5_000 + k * 20_000,
                    if k == 150 + lost {
                        960 * (lost as usize + 1)
                    } else {
                        960
                    },
                )
            })
            .collect()
    };
    for (name, deliveries, packet, period) in [
        (
            "20 ms packets around a wake-up",
            straddling.clone(),
            960,
            480,
        ),
        ("10 ms packets", short, 480, 480),
        ("one packet 8 ms late", late, 960, 480),
        ("600 ms muted", muted.clone(), 960, 480),
        ("one stall", stall(30_000_000), 960, 480),
        ("a stall every 4 s", stall(4_000_000), 960, 480),
        ("3 concealed", concealed(3), 960, 480),
        ("5 concealed", concealed(5), 960, 480),
        ("5 ms device", straddling, 960, 240),
    ] {
        let new = simulate::<Playout>(
            &deliveries,
            packet,
            period,
            30_000_000,
            1_000_000,
            |_, _| {},
        );
        let old = simulate::<Bounded>(
            &deliveries,
            packet,
            period,
            30_000_000,
            1_000_000,
            |_, _| {},
        );
        assert!(
            new.delay <= old.delay + 0.01,
            "{name}: {:.1} ms, before {:.1} ms",
            new.delay,
            old.delay
        );
    }
    // A mute is not an underrun.
    let run = simulate::<Playout>(&muted, 960, 480, 30_000_000, 0, |_, _| {});
    assert_eq!(run.queue.underruns, 0);
}

#[test]
fn a_stall_released_in_two_batches_is_not_learnt() {
    // 80 ms with nothing, two packets, 60 ms more, then the rest: two
    // long gaps, often in neighbouring windows, but one stall. Every
    // phase of the stall against the windows keeps the old delay.
    for phase in 0..20u64 {
        let stall = 3_000_000 + phase * 5_000;
        let deliveries = packets((0..500u64).map(|k| {
            let at = 5_000 + k * 20_000;
            match at {
                _ if at > stall && at < stall + 40_000 => stall + 80_000,
                _ if at > stall + 40_000 && at < stall + 140_000 => stall + 140_000,
                _ => at,
            }
        }));
        let new = simulate::<Playout>(&deliveries, 960, 480, 6_000_000, 2_000_000, |_, _| {});
        let old = simulate::<Bounded>(&deliveries, 960, 480, 6_000_000, 2_000_000, |_, _| {});
        assert!(
            new.delay <= old.delay + 0.01,
            "phase {phase}: {:.1} ms, before {:.1} ms",
            new.delay,
            old.delay
        );
    }
}

#[test]
fn a_fast_client_clock_loses_small_crossfaded_slices() {
    // The client's 20 ms take 19.8 ms of the host's time: 1% too much audio.
    let deliveries = packets((0..3000u64).map(|k| 5_000 + k * 19_800));
    let mut trimmed = 0;
    let run = simulate(
        &deliveries,
        960,
        480,
        59_000_000,
        1_000_000,
        |now, p: &Playout| {
            assert!(p.trimmed - trimmed <= SLICE as u64, "{now}");
            trimmed = p.trimmed;
            if now > 1_000_000 {
                assert!(p.len() <= ms(60), "{now}: {}", p.len());
            }
        },
    );
    assert_eq!(run.queue.underruns, 0);
    assert_eq!(run.queue.trimmed % SLICE as u64, 0);
    let sent = deliveries
        .iter()
        .filter(|&&(at, _)| at < 58_990_000)
        .count()
        * 960;
    let share = run.queue.trimmed as f64 / sent as f64;
    assert!((0.008..0.0105).contains(&share), "{share}");
    // The tone moves at most 0.065 a sample; a cut without the fade
    // would jump by up to 1.
    let jump = run
        .played
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0., f32::max);
    assert!(jump < 0.08, "{jump}");
    // No later than the old bound, which let it grow to 60 ms and cut.
    let old = simulate::<Bounded>(&deliveries, 960, 480, 59_000_000, 1_000_000, |_, _| {});
    assert!(
        run.delay <= old.delay,
        "{:.1} ms, before {:.1} ms",
        run.delay,
        old.delay
    );
}

#[test]
fn a_short_cut_fades_from_the_samples_it_drops() {
    let ramp: Vec<f32> = (0..400).map(|n| n as f32 / 400.).collect();
    let mut playout = Playout::default();
    playout.push(&ramp);
    playout.playing = true;
    playout.cut(10);
    let mut kept = vec![0.; 390];
    assert_eq!(playout.take(&mut kept), 390);
    for (i, &sample) in kept.iter().enumerate() {
        let expected = if i < FADE {
            let weight = (i + 1) as f32 / (FADE + 1) as f32;
            ramp[i] + (ramp[10 + i] - ramp[i]) * weight
        } else {
            ramp[10 + i]
        };
        assert!((sample - expected).abs() < 1e-6, "{i}: {sample} {expected}");
    }
}

#[test]
fn underruns_are_counted_and_refilled_but_pauses_are_not() {
    // Steady for two seconds with a 60 ms stall (its packets then come
    // at once), 600 ms of mute, a second more, and the end of the stream.
    let deliveries = packets((0..100u64).chain(130..180).map(|k| {
        5_000
            + k * 20_000
            + if (50..53).contains(&k) {
                (53 - k) * 20_000
            } else {
                0
            }
    }));
    let mut dry_pulls = 0;
    let mut was_dry = false;
    let run = simulate(&deliveries, 960, 480, 4_000_000, 0, |_, p: &Playout| {
        // One underrun is one dry spell, not one per device period.
        if p.dry.is_some() && !was_dry {
            dry_pulls += 1;
        }
        was_dry = p.dry.is_some();
    });
    let mut playout = run.queue;
    assert_eq!(playout.underruns, 1);
    // 60 ms late, less what the device still held.
    assert!(
        (ms(40)..=ms(70)).contains(&(playout.underrun_samples as usize)),
        "{}",
        playout.underrun_samples
    );
    // The stall, the mute, and the end of the stream.
    assert_eq!(dry_pulls, 3);
    playout.clear();
    assert_eq!(
        (playout.underruns, playout.trimmed, playout.len()),
        (0, 0, 0)
    );
}

#[test]
fn the_device_is_kept_two_periods_ahead_and_never_runs_dry() {
    // 10 ms periods in a 20 ms buffer.
    assert_eq!(render_plan(960, 0, 480, 0), (0, 480));
    assert_eq!(render_plan(960, 0, 480, 960), (960, 0));
    assert_eq!(render_plan(960, 480, 480, 960), (480, 0));
    assert_eq!(render_plan(960, 480, 480, 100), (100, 0));
    assert_eq!(render_plan(960, 100, 480, 100), (100, 280));
    assert_eq!(render_plan(960, 960, 480, 960), (0, 0));
    // A larger buffer is not filled past two periods.
    assert_eq!(render_plan(4800, 0, 480, 4000), (960, 0));
}

/// The simulated voice: a 437 Hz tone, whose period divides neither a
/// packet nor a device period, so seams land anywhere in its cycle.
fn tone(n: usize) -> f32 {
    ((n * 437 % RATE) as f64 * std::f64::consts::TAU / RATE as f64).sin() as f32 * 0.5
}
/// The playout with a concealer that, like Opus PLC, continues the voice
/// from the last sample pushed, one 20 ms frame a call: the packet that
/// follows starts out of phase with what was concealed.
#[derive(Default)]
struct Concealing {
    playout: Playout,
    pushed: usize,
    next: usize,
}
impl Queue for Concealing {
    fn push_batch(&mut self, samples: &[f32], packet_frames: usize) {
        self.pushed += samples.len();
        self.next = self.pushed;
        self.playout.push_batch(samples, packet_frames);
    }
    fn pull(&mut self, buffer: usize, padding: usize, period: usize, out: &mut Vec<f32>) -> usize {
        let Self { playout, next, .. } = self;
        playout.pull_concealing(buffer, padding, period, out, &mut |frame| {
            frame.extend((*next..*next + 960).map(tone));
            *next += 960;
            Ok(960)
        })
    }
    /// Real audio only: concealment kept back is replaced by the next
    /// packet, so it delays nothing.
    fn len(&self) -> usize {
        self.playout.len() - self.playout.provisional
    }
}
/// The largest step between neighbouring samples played after `from`
/// samples: a hard edge to or from silence is a click. The tone itself
/// moves at most 0.029 a sample.
fn largest_step(played: &[f32], from: usize) -> f32 {
    played[from..]
        .windows(2)
        .map(|pair| (pair[1] - pair[0]).abs())
        .fold(0., f32::max)
}
/// Whether two samples in a row after the first 100 ms are silent (the
/// tone is never 0 twice in a row).
fn breaks_off(played: &[f32]) -> bool {
    played[ms(100)..].windows(2).any(|pair| pair == [0., 0.])
}
/// 20 ms packets with a stall of `stall` µs every two seconds, its packets
/// then released at once.
fn stalling(stall: u64) -> Vec<(u64, usize)> {
    packets((0..500u64).map(|k| {
        let at = 5_000 + k * 20_000;
        let phase = at % 2_000_000;
        if (1_000_000..1_000_000 + stall).contains(&phase) {
            at - phase + 1_000_000 + stall
        } else {
            at
        }
    }))
}

#[test]
fn a_dry_spell_plays_concealment_without_clicks_or_delay() {
    // Stalls as on a phone's Wi-Fi (issue 56: 7 underruns in 51 s), each
    // long enough to run the queue dry, with the packet's seam falling at
    // a different point of the device period and of the tone each time.
    for stall in [20_000, 30_000, 40_000, 60_000] {
        let deliveries = stalling(stall);
        let silent = simulate::<Playout>(&deliveries, 960, 480, 10_000_000, 1_000_000, |_, _| {});
        let concealed =
            simulate::<Concealing>(&deliveries, 960, 480, 10_000_000, 1_000_000, |_, _| {});
        // Before: each spell broke the voice off into silence.
        assert!(
            silent.queue.underruns >= 4,
            "{stall}: {}",
            silent.queue.underruns
        );
        assert!(breaks_off(&silent.played), "{stall}");
        // Now the same spells are counted, but the voice never breaks off
        // and neither end of a spell clicks.
        let playout = &concealed.queue.playout;
        assert_eq!(playout.underruns, silent.queue.underruns, "{stall}");
        assert!(playout.concealed_samples > 0, "{stall}");
        assert!(!breaks_off(&concealed.played), "{stall}");
        let step = largest_step(&concealed.played, ms(100));
        assert!(step < 0.06, "{stall}: {step}");
        // And it plays exactly as late as the silence did, trimming no more.
        assert!(
            concealed.delay <= silent.delay + 0.01,
            "{stall}: {:.2} ms, before {:.2} ms",
            concealed.delay,
            silent.delay
        );
        assert!(
            playout.trimmed <= silent.queue.trimmed,
            "{stall}: {} trimmed, before {}",
            playout.trimmed,
            silent.queue.trimmed
        );
    }
}

#[test]
fn long_concealment_fades_out_and_the_voice_fades_back_in() {
    // 300 ms without packets in the middle of speech, then speech again.
    let deliveries = packets(
        (0..200u64)
            .filter(|k| !(100..115).contains(k))
            .map(|k| 5_000 + k * 20_000),
    );
    let run = simulate::<Concealing>(&deliveries, 960, 480, 4_000_000, 0, |_, _| {});
    let step = largest_step(&run.played, ms(100));
    assert!(step < 0.06, "{step}");
    // Concealment for at most 60 ms, then silence until the voice returns.
    let longest_silence = run.played[ms(100)..]
        .split(|sample| *sample != 0.)
        .map(<[f32]>::len)
        .max()
        .unwrap();
    assert!(longest_silence >= ms(200), "{longest_silence}");
    assert_eq!(
        run.queue.playout.underruns, 0,
        "a pause longer than 250 ms is not an underrun"
    );
}

#[test]
fn a_failing_or_busy_decoder_leaves_silence_and_is_counted() {
    let mut playout = Playout::default();
    let mut out = Vec::new();
    playout.push(&(0..960).map(tone).collect::<Vec<_>>());
    while !playout.is_empty() {
        playout.pull_concealing(960, 0, 480, &mut out, &mut |_| unreachable!());
    }
    // Dry: a failure is counted and the wake-up is silent.
    let silence = playout.pull_concealing(960, 0, 480, &mut out, &mut |_| Err(()));
    assert_eq!((out.len(), silence, playout.conceal_failed), (0, 480, 1));
    // A busy decoder gives nothing and fails nothing.
    playout.push(&(0..960).map(tone).collect::<Vec<_>>());
    playout.pull_concealing(960, 0, 480, &mut out, &mut |_| Ok(0));
    playout.pull_concealing(960, 0, 480, &mut out, &mut |_| Ok(0));
    let silence = playout.pull_concealing(960, 0, 480, &mut out, &mut |_| Ok(0));
    assert_eq!((out.len(), silence, playout.conceal_failed), (0, 480, 1));
    assert_eq!(playout.concealed_samples, 0);
}

#[test]
fn taking_the_queue_takes_its_concealment_too() {
    let mut playout = Playout::default();
    let mut out = Vec::new();
    playout.push(&(0..600).map(tone).collect::<Vec<_>>());
    // 480 real, then 120 real and 360 concealed, with more kept back.
    playout.pull_concealing(960, 480, 480, &mut out, &mut |_| unreachable!());
    playout.pull_concealing(960, 0, 480, &mut out, &mut |frame| {
        frame.extend((600..1560).map(tone));
        Ok(960)
    });
    assert!(playout.provisional > 0);
    let mut all = vec![0.; 4000];
    playout.take(&mut all);
    assert_eq!((playout.len(), playout.provisional), (0, 0));
    // The queue still works: a new packet and a pull do not underflow.
    playout.push(&(0..960).map(tone).collect::<Vec<_>>());
    playout.pull_concealing(960, 0, 480, &mut out, &mut |_| Ok(0));
    assert_eq!(out.len(), 960);
}
