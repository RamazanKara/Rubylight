import contextlib, io, json, math, pathlib, shutil, tempfile, unittest
import frame_warp_accuracy
from frame_warp_accuracy import build_report, client_source, exact_source, idle_rows, lens_points, main

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


def level(t_us):
    """`pausing` to the side only."""
    return pausing(t_us)[0], 0


class Run:
    """A synthetic 60 fps run: the probe draws frame k at START_US + k frames showing the client's input as of LAG_US
    before (plus `offset` in the probe's counts), and the client decodes it DELAY_US later and shifts it by `fraction`
    of the ideal. With `fov` the probe draws through a lens of that many degrees and the ideal is the centre's; the
    client then logs `client_focal` as its model's focal length (None: a log without the column)."""

    def __init__(self, directory, fraction=1.0, offset=(0, 0), gain=1.0, sent=pausing, injected_only=False,
                 locked_from_us=0, seconds=6.0, scale=1.0, pitch_limited=(), display=(1920, 1080), fov=None,
                 client_focal=None):
        self.directory = directory
        self.frames, self.client = [], []
        self.client_focal = client_focal
        focal = display[0] / 2 / math.tan(math.radians(fov) / 2) if fov else None
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
            if focal:
                shown = (gain * probe_sum[0] * scale, camera_y * scale)
                centre = exact_source((0.0, 0.0), shown, (shown[0] - ideal[0], shown[1] - ideal[1]), focal * scale)
                ideal = (-centre[0], -centre[1])
            locked = decode >= START_US + locked_from_us
            self.client.append(dict(wire_frame=k + 7, seq=k, decode=decode, sent=now,
                                    shift=(fraction * ideal[0], fraction * ideal[1]), locked=locked,
                                    gain=(g, g) if locked else None, lag_ms=82.5 if locked else None))
        self.report = {
            'probe': 'look_probe',
            'config': {'gain': gain, 'lag_ms': LAG_US / 1000, 'injected_only': injected_only},
            'display': {'width': display[0], 'height': display[1]},
            'camera_y_limit': Y_LIMIT,
            'expected_host_model': {'mouse_gain': [-gain, -gain], 'lag_us_at_least': LAG_US, 'focal': focal},
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
        lens = self.client_focal is not None
        lines = [','.join(frame_warp_accuracy.CLIENT_COLUMNS) + (',focal' if lens else '')]
        for row in [*self.client, *extra_client]:
            gain = ('', '') if row['gain'] is None else tuple(f'{g:.6f}' for g in row['gain'])
            lag = '' if row['lag_ms'] is None else f'{row["lag_ms"]:.3f}'
            active = int(abs(row['shift'][0]) >= 0.01 or abs(row['shift'][1]) >= 0.01)
            focal = f',{self.client_focal if row["locked"] else 0.0:.2f}' if lens else ''
            lines.append(f'{row["wire_frame"]},{row["seq"]},{row["decode"]},{row["sent"][0]},{row["sent"][1]},{active},'
                         f'{row["shift"][0]:.3f},{row["shift"][1]:.3f},{int(row["locked"])},{gain[0]},{gain[1]},{lag}{focal}')
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


class Lens(Temporary):
    def test_the_probes_projection_is_undone_exactly(self):
        focal = 960.0
        for camera in ((0.0, 0.0), (500.0, -200.0), (-3000.0, 350.0)):
            for point in ((0.0, 0.0), (800.0, -400.0), (-950.0, 530.0)):
                source = exact_source(point, camera, camera, focal)
                self.assertAlmostEqual(source[0], point[0], places=6)
                self.assertAlmostEqual(source[1], point[1], places=6)
        # A level turn to the side adds its angle to the point's.
        source = exact_source((300.0, 0.0), (100.0, 0.0), (148.0, 0.0), focal)
        self.assertAlmostEqual(source[0], focal * math.tan(math.atan(300 / focal) + 48 / focal), places=6)
        self.assertAlmostEqual(source[1], 0.0, places=6)
        # Looking down, it also rolls the picture: a point to the side comes from higher or lower.
        rolled = exact_source((800.0, 0.0), (0.0, 400.0), (48.0, 400.0), focal)
        self.assertGreater(abs(rolled[1]), 5)
        # What the turned camera sees behind the first one is in no picture of it.
        self.assertIsNone(exact_source((0.0, 0.0), (0.0, 0.0), (focal * 2, 0.0), focal))

    def test_the_clients_mapping_is_the_shift_or_the_lens(self):
        size, focal = (1920, 1080), 960.0
        self.assertEqual(client_source((100.0, -50.0), (30.0, -12.0), None, size), (70.0, -38.0))
        self.assertEqual(client_source((100.0, -50.0), (30.0, -12.0), 0.0, size), (70.0, -38.0))
        # A level turn of 48 pixels at the centre moves it by the tangent; the lens then draws every point exactly.
        shift = (-focal * math.tan(48 / focal), 0.0)
        points, edge = lens_points(size)
        self.assertEqual((len(points), sum(edge)), (49, 24))
        self.assertAlmostEqual(points[0][0], -0.45 * 1920)
        for point in points:
            exact = exact_source(point, (0.0, 0.0), (48.0, 0.0), focal)
            drawn = client_source(point, shift, focal, size)
            self.assertAlmostEqual(drawn[0], exact[0], places=6)
            self.assertAlmostEqual(drawn[1], exact[1], places=6)
        # Level with the camera the depth is held (nightjar_core's MIN_DEPTH).
        held = client_source((960.0, 0.0), (-2000.0, 0.0), 960.0, size)
        self.assertAlmostEqual(held[0], (0.5 * math.sqrt(1) + 2000 / 1920) / 0.25 * 1920)

    def test_a_client_with_the_lens_is_right_over_the_whole_picture(self):
        result = self.result(Run(self.directory, sent=level, fov=90, client_focal=960.0))
        lens = result['lens']
        self.assertAlmostEqual(lens['field_of_view'], 90.0)
        self.assertAlmostEqual(lens['learned_field_of_view'], 90.0)
        self.assertEqual((lens['with_lens_share'], lens['unseen']), (1.0, 0))
        for part in ('centre', 'picture', 'edge'):
            self.assertLess(lens[part]['with_warp_error_px']['p95'], 0.01, part)
            self.assertAlmostEqual(lens[part]['removed'], 1.0, places=3)
        # The edge of a 90 degree picture moves far more than its centre.
        self.assertGreater(lens['edge']['without_warp_px']['median'], 1.4 * lens['centre']['without_warp_px']['median'])
        self.assertEqual(result['with_warp_error_px'], lens['centre']['with_warp_error_px'])

    def test_a_client_that_slides_the_picture_is_right_only_where_its_shift_is(self):
        result = self.result(Run(self.directory, sent=level, fov=90))
        lens = result['lens']
        self.assertEqual((lens['learned_focal'], lens['learned_field_of_view'], lens['with_lens_share']), (0.0, None, 0.0))
        self.assertLess(lens['centre']['with_warp_error_px']['p95'], 0.01)
        self.assertGreater(lens['edge']['with_warp_error_px']['median'], 10)
        self.assertLess(lens['edge']['removed'], 0.7)
        self.assertLess(lens['edge']['removed'], lens['picture']['removed'])
        self.assertLess(lens['picture']['removed'], 0.9)
        # A shift that suits the picture's middle ring, as a median over it does, is too long at the centre.
        long = self.result(Run(self.directory, sent=level, fov=90, fraction=1.3))['lens']
        self.assertAlmostEqual(long['centre']['removed'], 0.7, places=2)
        self.assertGreater(long['picture']['removed'], lens['picture']['removed'])

    def test_a_turn_while_looking_down_rolls_the_picture_past_the_lens(self):
        # Both axes, down to a quarter of the picture's height below the horizon: the lens leaves the roll.
        lens = self.result(Run(self.directory, fov=90, client_focal=960.0))['lens']
        flat = self.result(Run(self.directory, fov=90))['lens']
        self.assertLess(lens['centre']['with_warp_error_px']['p95'], 0.01)
        self.assertGreater(lens['edge']['with_warp_error_px']['median'], 1)
        self.assertLess(lens['edge']['with_warp_error_px']['median'], 0.5 * flat['edge']['with_warp_error_px']['median'])

    def test_a_flat_probe_has_no_lens_and_another_shape_is_refused(self):
        self.assertIsNone(self.result(Run(self.directory))['lens'])
        probe, client = Run(self.directory, fov=90).write()
        with self.assertRaisesRegex(ValueError, 'shape'):
            build_report(probe, client, 0.0, (1280, 1080))
        half = build_report(probe, client, 0.0, (960, 540))['lens']
        self.assertAlmostEqual(half['focal'], 480.0)

    def test_the_table_shows_the_lens(self):
        probe, client = Run(self.directory, sent=level, fov=90, client_focal=960.0).write()
        printed = io.StringIO()
        with contextlib.redirect_stdout(printed):
            self.assertEqual(main(['--probe', str(probe), '--client', str(client)]), 0)
        self.assertIn('lens: 90.0 degrees across (focal 960.0 px); learned 90.0 degrees', printed.getvalue())
        self.assertRegex(printed.getvalue(), r'whole picture +\d+\.\d+ +\d+\.\d+ +0\.00 +0\.00 +100\.0%')


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
