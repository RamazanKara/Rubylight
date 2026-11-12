import contextlib, io, json, pathlib, shutil, tempfile, unittest
import frame_warp_accuracy
from frame_warp_accuracy import build_report, idle_rows, main

START_US = 1_000_000_000
FRAME_US = 16_667
LAG_US = 50_000
DELAY_US = 30_000
Y_LIMIT = 540.0
FRAMES_HEADER = ('frame,present_qpc_us,input_time_us,mouse_sum_x,mouse_sum_y,stick_integral_x,stick_integral_y,'
                 'camera_x,camera_y,render_qpc_us')


def pausing(t_us):
    """The client's sums t_us after its first send: 1 s at 600 counts/s right and 120 up, 0.5 s still, again."""
    cycles, within = divmod(max(t_us, 0), 1_500_000)
    moving = cycles * 1_000_000 + min(within, 1_000_000)
    return moving * 600 // 1_000_000, -(moving * 120 // 1_000_000)


def steady(t_us):
    """The same turn without pauses."""
    moving = max(t_us, 0)
    return moving * 600 // 1_000_000, -(moving * 120 // 1_000_000)


class Run:
    """A synthetic 60 fps run: the probe draws frame k at START_US + k frames showing the client's input as of LAG_US
    before (plus `offset` in the probe's counts), and the client decodes it DELAY_US later and shifts it by `fraction`
    of the ideal."""

    def __init__(self, directory, fraction=1.0, offset=(0, 0), gain=1.0, sent=pausing, injected_only=False,
                 locked_from_us=0, seconds=6.0, scale=1.0, pitch_limited=(), display=(1920, 1080)):
        self.directory = directory
        self.frames, self.client = [], []
        g = -gain * scale
        for k in range(1, int(seconds * 60) + 1):
            render = START_US + k * FRAME_US
            content = sent(render - START_US - LAG_US)
            probe_sum = (content[0] + offset[0], content[1] + offset[1])
            camera_y = Y_LIMIT if k in pitch_limited else gain * probe_sum[1]
            self.frames.append((k, render, probe_sum, gain * probe_sum[0], camera_y))
            decode = render + DELAY_US
            now = sent(decode - START_US)
            ideal = (g * (now[0] - content[0]), g * (now[1] - content[1]))
            locked = decode >= START_US + locked_from_us
            self.client.append(dict(wire_frame=k + 7, seq=k, decode=decode, sent=now,
                                    shift=(fraction * ideal[0], fraction * ideal[1]), locked=locked,
                                    gain=(g, g) if locked else None, lag_ms=82.5 if locked else None))
        self.report = {
            'probe': 'look_probe',
            'config': {'gain': gain, 'lag_ms': LAG_US / 1000, 'injected_only': injected_only},
            'display': {'width': display[0], 'height': display[1]},
            'camera_y_limit': Y_LIMIT,
            'expected_host_model': {'mouse_gain': [-gain, -gain], 'lag_us_at_least': LAG_US},
            'files': {},
        }

    def write(self, extra_client=()):
        probe = self.directory / 'run.json'
        probe.write_text(json.dumps(self.report))
        lines = [FRAMES_HEADER]
        for k, render, (sum_x, sum_y), camera_x, camera_y in self.frames:
            lines.append(f'{k},{render + 1000},{render - LAG_US},{sum_x},{sum_y},0.000000,0.000000,'
                         f'{camera_x:.4f},{camera_y:.4f},{render}')
        (self.directory / 'run.frames.csv').write_text('\n'.join(lines) + '\n')
        lines = [','.join(frame_warp_accuracy.CLIENT_COLUMNS)]
        for row in [*self.client, *extra_client]:
            gain = ('', '') if row['gain'] is None else tuple(f'{g:.6f}' for g in row['gain'])
            lag = '' if row['lag_ms'] is None else f'{row["lag_ms"]:.3f}'
            active = int(abs(row['shift'][0]) >= 0.01 or abs(row['shift'][1]) >= 0.01)
            lines.append(f'{row["wire_frame"]},{row["seq"]},{row["decode"]},{row["sent"][0]},{row["sent"][1]},{active},'
                         f'{row["shift"][0]:.3f},{row["shift"][1]:.3f},{int(row["locked"])},{gain[0]},{gain[1]},{lag}')
        client = self.directory / 'fw.csv'
        client.write_text('\n'.join(lines) + '\n')
        return probe, client


class Temporary(unittest.TestCase):
    def setUp(self):
        self.directory = pathlib.Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.directory, True)

    def result(self, run, extra_client=(), skip_seconds=0.0, size=None):
        probe, client = run.write(extra_client)
        return build_report(probe, client, skip_seconds, size)


class Accuracy(Temporary):
    def test_a_perfect_warp_removes_everything_and_finds_the_offset(self):
        result = self.result(Run(self.directory, offset=(37, -5)))
        self.assertEqual(result['offset']['source'], 'idle')
        self.assertEqual(result['offset']['counts'], [37, -5])
        self.assertEqual((result['offset']['min'], result['offset']['max']), ([37, -5], [37, -5]))
        self.assertAlmostEqual(result['removed'], 1.0, places=3)
        self.assertLess(result['with_warp_error_px']['p95'], 0.01)
        self.assertGreater(result['without_warp_px']['median'], 20)
        self.assertEqual(result['learned']['mouse_gain'], [-1.0, -1.0])
        self.assertEqual(result['learned']['relative_error'], [0.0, 0.0])
        self.assertEqual(result['learned']['lag_ms'], 82.5)
        self.assertEqual(result['picture_age_ms']['median'], DELAY_US / 1000)
        self.assertEqual(result['locked_share'], 1.0)
        self.assertEqual(result['seconds_to_first_lock'], 0.0)

    def test_no_warp_removes_nothing_and_half_removes_half(self):
        none = self.result(Run(self.directory, fraction=0.0))
        self.assertEqual(none['removed'], 0.0)
        self.assertEqual(none['with_warp_error_px'], none['without_warp_px'])
        half = self.result(Run(self.directory, fraction=0.5))
        self.assertAlmostEqual(half['removed'], 0.5, places=3)
        self.assertGreater(half['error_bias_px'][0], 0)
        self.assertLess(half['error_bias_px'][1], 0)

    def test_still_pictures_and_unlocked_rows_are_not_measured(self):
        run = Run(self.directory, locked_from_us=2_000_000)
        result = self.result(run)
        locked = [row for row in run.client if row['locked']]
        rows = result['rows']
        self.assertEqual((rows['client'], rows['joined'], rows['counted']), (360, 360, 360))
        self.assertEqual(rows['locked'], len(locked))
        # Pictures that already show all the input need no shift: under the threshold.
        self.assertGreater(rows['below_threshold'], 0)
        self.assertEqual(rows['measured'] + rows['below_threshold'], rows['locked'])
        expected = (locked[0]['decode'] - run.client[0]['decode']) / 1e6
        self.assertAlmostEqual(result['seconds_to_first_lock'], expected)
        self.assertAlmostEqual(result['locked_share'], len(locked) / 360)

    def test_rows_without_a_known_barcode_are_not_joined(self):
        run = Run(self.directory)
        last = run.client[-1]
        extra = [dict(last, seq=0, decode=last['decode'] + 1000), dict(last, seq=99_999, decode=last['decode'] + 2000)]
        rows = self.result(run, extra)['rows']
        self.assertEqual((rows['client'], rows['with_barcode'], rows['joined']), (362, 361, 360))

    def test_pictures_at_the_pitch_limit_are_left_out(self):
        result = self.result(Run(self.directory, pitch_limited=set(range(100, 120))))
        self.assertEqual(result['rows']['pitch_limited'], 20)
        self.assertAlmostEqual(result['removed'], 1.0, places=3)

    def test_skipped_seconds_leave_the_accuracy_figures_but_not_the_lock_time(self):
        run = Run(self.directory, locked_from_us=1_000_000)
        result = self.result(run, skip_seconds=3.0)
        first = run.client[0]['decode']
        self.assertEqual(result['rows']['counted'], sum(row['decode'] >= first + 3_000_000 for row in run.client))
        self.assertEqual(result['locked_share'], 1.0)
        self.assertLess(result['seconds_to_first_lock'], 1.0)

    def test_a_smaller_stream_scales_the_gain(self):
        run = Run(self.directory, scale=1280 / 1920)
        scaled = self.result(run, size=(1280, 720))
        self.assertAlmostEqual(scaled['expected']['mouse_gain'][0], -2 / 3)
        self.assertAlmostEqual(scaled['removed'], 1.0, places=3)
        self.assertAlmostEqual(scaled['learned']['relative_error'][0], 0.0, places=5)
        unscaled = self.result(run)
        self.assertLess(unscaled['removed'], 0.9)


class Offset(Temporary):
    def test_idle_means_unchanged_sums_for_the_whole_time(self):
        rows = [dict(sent=(0, 0), decode_us=0), dict(sent=(5, 0), decode_us=100_000),
                dict(sent=(5, 0), decode_us=399_999), dict(sent=(5, 0), decode_us=400_000),
                dict(sent=(6, 0), decode_us=900_000)]
        self.assertEqual(idle_rows(rows, 300_000), [rows[3]])

    def test_without_pauses_an_injected_only_probe_assumes_zero(self):
        result = self.result(Run(self.directory, sent=steady, injected_only=True))
        self.assertEqual(result['offset']['source'], 'injected_only')
        self.assertEqual(result['offset']['counts'], [0, 0])
        self.assertAlmostEqual(result['removed'], 1.0, places=3)

    def test_without_pauses_any_other_probe_is_refused(self):
        run = Run(self.directory, sent=steady, offset=(10, 0))
        with self.assertRaisesRegex(ValueError, 'idle'):
            self.result(run)

    def test_a_longer_game_lag_needs_a_longer_pause(self):
        run = Run(self.directory, offset=(3, 3))
        # 0.5 s pauses are shorter than a 300 ms lag plus the 250 ms margin.
        run.report['expected_host_model']['lag_us_at_least'] = 300_000
        with self.assertRaisesRegex(ValueError, '550 ms'):
            self.result(run)


class Command(Temporary):
    def test_main_prints_a_table_and_writes_json(self):
        probe, client = Run(self.directory, offset=(1, 2)).write()
        output = self.directory / 'result.json'
        printed = io.StringIO()
        with contextlib.redirect_stdout(printed):
            code = main(['--probe', str(probe), '--client', str(client), '--json', str(output)])
        self.assertEqual(code, 0)
        self.assertIn('removed: 100.0% of the input-to-picture displacement', printed.getvalue())
        self.assertIn('offset: 1,2 counts', printed.getvalue())
        saved = json.loads(output.read_text())
        self.assertEqual(saved['offset']['counts'], [1, 2])
        self.assertEqual(saved['client'], str(client))

    def test_the_frames_named_in_the_report_are_found_when_not_beside_it(self):
        run = Run(self.directory)
        probe, client = run.write()
        moved = self.directory / 'elsewhere.frames.csv'
        (self.directory / 'run.frames.csv').rename(moved)
        run.report['files'] = {'frames': str(moved)}
        probe.write_text(json.dumps(run.report))
        self.assertAlmostEqual(build_report(probe, client)['removed'], 1.0, places=3)

    def test_bad_inputs_fail_with_exit_code_2(self):
        probe, client = Run(self.directory).write()
        client.write_text('wire_frame,barcode_seq\n1,1\n')
        errors = io.StringIO()
        with contextlib.redirect_stderr(errors), contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(main(['--probe', str(probe), '--client', str(client)]), 2)
        self.assertIn('decode_qpc_us', errors.getvalue())
        (self.directory / 'run.frames.csv').unlink()
        with contextlib.redirect_stderr(io.StringIO()):
            self.assertEqual(main(['--probe', str(probe), '--client', str(client)]), 2)


if __name__ == '__main__':
    unittest.main()
