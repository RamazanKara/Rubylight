# Congestion control A/B decision tests on synthetic host logs and client results; no host or device.
# Run: python -m unittest discover -s rust/release -p test_congestion_ab.py
import json, pathlib, tempfile, unittest
import congestion_ab as ab


def timings(state='normal', shadow=False, queue=(4.0, 12.0), acked=18000, windows=4):
    line = ('2026-11-13T10:00:{s:02}.000Z  INFO butterpollo::stream: stream timings encode_mean_ms=2.5 '
            'cc_state={state} cc_shadow={shadow} cc_usage=normal cc_target_kbps=20000 cc_acked_kbps={acked} '
            'cc_queue_p50_ms={q50} cc_queue_p95_ms={q95} cc_queue_samples=900 cc_overuse_cuts=1')
    return '\n'.join(line.format(s=5 * i, state=state, shadow=str(shadow).lower(), acked=acked,
                                 q50=queue[0] + (100 if i == 0 else 0), q95=queue[1]) for i in range(windows))


def summary(cuts=2, recovery=1500):
    return ('2026-11-13T10:01:00.000Z  INFO butterpollo::stream::video::teardown: congestion summary '
            f'client="Retroid" shadow=false reports=900 overuse_cuts={cuts} loss_cuts=0 '
            f'recovery_p50_ms={recovery} recovery_max_ms={recovery * 2}')


def legs(device, baseline, on):
    """Legs from (p50, p95, fps, dropped) tuples per arm."""
    out = []
    for arm, values in (('baseline', baseline), ('on', on)):
        for i, (p50, p95, fps, dropped) in enumerate(values):
            host = ab.host_leg(timings(shadow=arm == 'baseline') + '\n' + summary())
            out.append(ab.leg(f'{device}-{arm}-{i}', host, dict(device=device, latency_ms=dict(p50=p50, p95=p95),
                                                                fps=fps, dropped=dropped)))
    return out


class HostLog(unittest.TestCase):
    def test_reads_the_arm_and_skips_the_first_window(self):
        host = ab.host_leg(timings(shadow=True) + '\n' + summary())
        self.assertEqual(host['arm'], 'baseline')
        self.assertTrue(host['shadow'])
        self.assertEqual(host['windows'], 3)
        # The first window's 100 ms warm-up queue is left out.
        self.assertEqual(host['queue_p50_ms'], 4.0)
        self.assertEqual(host['delivered_kbps'], 18000)
        self.assertEqual(host['overuse_cuts'], 2)
        self.assertEqual(host['recovery_max_ms'], 3000)
        self.assertEqual(ab.host_leg(timings())['arm'], 'on')
        self.assertEqual(ab.host_leg(timings(state='off'))['arm'], 'baseline')
        self.assertEqual(ab.host_leg('')['arm'], 'baseline')

    def test_counts_change_lines_but_not_the_summary(self):
        text = timings() + '\n' + summary() + '\n' + (
            '2026-11-13T10:00:30.000Z  WARN butterpollo::stream::control: congestion control '
            'client="Retroid" kind=overuse from_kbps=20000 to_kbps=15000')
        self.assertEqual(ab.host_leg(text)['changes'], 1)


class Decision(unittest.TestCase):
    def test_turns_on_when_latency_falls_beyond_the_spread(self):
        report = ab.decide(legs('retroid', [(40, 70, 59.9, 10), (42, 74, 60.0, 12)],
                                [(30, 50, 59.8, 8), (31, 52, 59.9, 9)]))
        self.assertTrue(report['turn_on'], report)
        self.assertEqual(report['devices']['retroid']['summary']['latency_p50_ms']['spread'], 2)

    def test_stays_off_within_the_spread(self):
        report = ab.decide(legs('retroid', [(40, 70, 60, 10), (48, 74, 60, 10)],
                                [(38, 60, 60, 10), (39, 61, 60, 10)]))
        self.assertFalse(report['turn_on'])
        self.assertIn('latency_p50_ms', report['devices']['retroid']['reasons'][0])

    def test_stays_off_with_fewer_frames_or_more_drops(self):
        fewer = ab.decide(legs('lap', [(40, 70, 60, 10)] * 2, [(30, 50, 58, 10)] * 2))
        self.assertFalse(fewer['turn_on'])
        self.assertTrue(any('fps' in r for r in fewer['devices']['lap']['reasons']))
        drops = ab.decide(legs('lap', [(40, 70, 60, 10)] * 2, [(30, 50, 60, 11)] * 2))
        self.assertFalse(drops['turn_on'])
        self.assertTrue(any('dropped' in r for r in drops['devices']['lap']['reasons']))
        # Rounding of the frame rate is not a loss.
        rounded = ab.decide(legs('lap', [(40, 70, 60, 10), (40.5, 70.5, 60, 10)],
                                 [(30, 50, 59.7, 10), (30, 50, 59.7, 10)]))
        self.assertTrue(rounded['turn_on'], rounded)

    def test_every_device_must_pass_and_needs_two_legs_an_arm(self):
        good = legs('retroid', [(40, 70, 60, 10), (41, 71, 60, 10)], [(30, 50, 60, 9), (30, 50, 60, 9)])
        bad = legs('lap', [(40, 70, 60, 10), (41, 71, 60, 10)], [(41, 72, 60, 10), (41, 72, 60, 10)])
        self.assertFalse(ab.decide(good + bad)['turn_on'])
        one = ab.decide(legs('retroid', [(40, 70, 60, 10)], [(30, 50, 60, 9)]))
        self.assertFalse(one['turn_on'])
        self.assertIn('need 2', one['devices']['retroid']['reasons'][0])
        self.assertFalse(ab.decide([])['turn_on'])

    def test_reads_leg_directories(self):
        with tempfile.TemporaryDirectory() as tmp:
            dirs = []
            for i, (shadow, p50) in enumerate([(True, 40), (False, 30), (True, 41), (False, 30)]):
                d = pathlib.Path(tmp) / f'leg{i}'
                d.mkdir()
                (d / 'host.log').write_text(timings(shadow=shadow) + '\n' + summary(), encoding='utf-8')
                (d / 'client.json').write_text(json.dumps(dict(device='retroid', latency_ms=dict(p50=p50, p95=p50 * 2),
                                                               fps=60, dropped=3)), encoding='utf-8')
                dirs.append(str(d))
            out = pathlib.Path(tmp) / 'report.json'
            self.assertEqual(ab.main(dirs + ['--json', str(out)]), 0)
            self.assertTrue(json.loads(out.read_text(encoding='utf-8'))['turn_on'])


if __name__ == '__main__':
    unittest.main()
