# Encoder focus A/B tests on synthetic scores and stubbed processes; no GPU. They need numpy and pillow
# (chroma_fixtures), which CI does not install.
# Run: python -m unittest discover -s rust/release -p test_focus_ab.py
import contextlib, io, json, math, pathlib, subprocess, sys, tempfile, types, unittest
from unittest import mock
import focus_ab as ab
import focus_ab_result as result
from gpu_hang import EXIT_GPU_HANG


def scores(vmaf, p5=None, settled=None):
    return dict(vmaf_mean=vmaf, vmaf_p5=vmaf - 4 if p5 is None else p5, vmaf_settled=vmaf if settled is None else settled,
                psnr_y_mean=vmaf / 2, ssim_mean=.99)


def quartet(gpu, comparison, fixture_class, kbps, off=(90., 90.), on=(91., 91.), moving=None, actual=None, codec='hevc',
            status=('ok',) * 4):
    """A1/B1/A2/B2 quality rows: full-frame VMAF per arm, and a moving region when given (off, on)."""
    actual = actual or (kbps,) * 4
    vmaf = (off[0], on[0], off[1], on[1])
    rows = []
    for index, arm in enumerate(result.ARMS):
        regions = dict(full=scores(vmaf[index]))
        if moving:
            regions['moving'] = scores(moving[index % 2])
        rows.append(dict(kind='quality', gpu=gpu, comparison=comparison, codec=codec, fixture=comparison,
                         fixture_class=fixture_class, target_kbps=kbps, arm=arm, mode='static' if index % 2 else 'off',
                         status=status[index], actual_bitrate_kbps=actual[index], scores=regions))
    return rows


def timing(gpu, comparison='hevc-1080p60-20000', arms=result.ARMS, convert=(.3, .3), encode=(2., 2.), present=(4., 4.),
           overflows=(0, 0), focus=None):
    focus = focus or dict(maps=5000, uniform=10, stale=0, skipped_recovery=2)
    rows = []
    for index, arm in enumerate(arms):
        on = index % 2
        rows.append(dict(kind='timing', gpu=gpu, comparison=comparison, arm=arm, status='ok', convert_p99_ms=convert[on],
                         encode_p99_ms=encode[on], present_to_sent_p99_ms=present[on], overflows=overflows[on],
                         focus=focus if on else None))
    return rows


def good(gpu='7900xt', codec='hevc'):
    """Text and mixed gain a point well beyond noise at 10 Mbps, a game loses 0.1, timing is flat."""
    return (quartet(gpu, 'editor', 'text', 10000, on=(91., 91.1), codec=codec)
            + quartet(gpu, 'mixed', 'mixed', 10000, on=(91., 91.1), moving=(80., 79.5), codec=codec)
            + quartet(gpu, 'tmnt', 'game', 10000, on=(89.9, 89.9), codec=codec) + timing(gpu))


def verdict(runs, plans=(), codec='hevc'):
    return result.decide(runs, plans)['codecs'][codec]['verdict']


class Decision(unittest.TestCase):
    def test_text_and_mixed_gain_without_cost_turn_focus_on_by_default(self):
        decision = result.decide(good())
        self.assertEqual(decision['codecs']['hevc'], dict(verdict='default_on', gpus={'7900xt': 'default_on'},
                                                          disagree=False, conflicts=[]))

    def test_a_gain_inside_twice_the_noise_does_not_count(self):
        # Gains of 0.6, but A1 and A2 differ by 0.25 (needs 0.5) or 0.4 (needs 0.8).
        for spread, expected in ((.25, 'default_on'), (.4, 'undecided')):
            off = (90. - spread / 2, 90. + spread / 2)
            runs = (quartet('g', 'editor', 'text', 10000, off=off, on=(90.6, 90.6))
                    + quartet('g', 'mixed', 'mixed', 10000, off=off, on=(90.6, 90.6)) + timing('g'))
            self.assertEqual(verdict(runs), expected, spread)

    def test_twenty_megabits_needs_a_smaller_gain(self):
        runs = (quartet('g', 'editor', 'text', 20000, on=(90.3, 90.3)) + quartet('g', 'editor10', 'text', 10000, on=(90.6, 90.6))
                + timing('g'))
        self.assertEqual(verdict(runs), 'default_on')
        runs = quartet('g', 'editor', 'text', 20000, on=(90.15, 90.15)) + timing('g')
        self.assertNotEqual(verdict(runs), 'default_on')

    def test_games_that_lose_up_to_half_a_point_leave_focus_opt_in(self):
        for loss, expected in ((.4, 'opt_in'), (.6, 'undecided')):
            runs = (quartet('g', 'editor', 'text', 10000, on=(91., 91.)) + quartet('g', 'tmnt', 'game', 10000, on=(90 - loss,) * 2)
                    + timing('g'))
            self.assertEqual(verdict(runs), expected, loss)

    def test_a_worse_p5_or_moving_region_blocks_default_on(self):
        runs = quartet('g', 'editor', 'text', 10000, on=(91., 91.)) + timing('g')
        for row in runs:
            if row['kind'] == 'quality' and row['arm'].startswith('B'):
                # The mean gains a point; p5 loses 0.6.
                row['scores']['full']['vmaf_p5'] -= 1.6
        self.assertNotIn(verdict(runs), ('default_on', 'opt_in'))
        runs = quartet('g', 'mixed', 'mixed', 10000, on=(91., 91.), moving=(80., 78.9)) + timing('g')
        self.assertNotIn(verdict(runs), ('default_on', 'opt_in'))

    def test_no_gain_beyond_noise_drops_focus(self):
        runs = (quartet('g', 'editor', 'text', 10000, off=(90., 90.1), on=(90.05, 90.05))
                + quartet('g', 'tmnt', 'game', 10000, off=(90., 90.1), on=(90., 90.)) + timing('g'))
        self.assertEqual(verdict(runs), 'drop')

    def test_a_timing_regression_drops_focus_whatever_the_quality(self):
        for kwargs in (dict(encode=(2., 2.2)), dict(convert=(.3, .36)), dict(present=(4., 4.3)), dict(overflows=(0, 1)),
                       dict(focus=dict(maps=980, uniform=0, stale=20, skipped_recovery=0))):
            runs = [r for r in good() if r['kind'] == 'quality'] + timing('7900xt', **kwargs)
            decision = result.decide(runs)
            self.assertEqual(decision['codecs']['hevc']['verdict'], 'drop', kwargs)
            self.assertFalse(decision['timing']['7900xt']['ok'])

    def test_a_single_pair_answers_overflows_and_stale_but_not_latency(self):
        runs = good() + timing('7900xt', 'hevc-1080p60-20000-slices', arms=result.PAIR, encode=(2., 3.))
        self.assertEqual(verdict(runs), 'default_on')
        runs = good() + timing('7900xt', 'hevc-1080p60-20000-slices', arms=result.PAIR, overflows=(0, 2))
        self.assertEqual(verdict(runs), 'drop')

    def test_missing_failed_or_unmatched_arms_are_incomplete(self):
        failed = quartet('g', 'editor', 'text', 10000, status=('ok', 'ok', 'ok', 'failed')) + timing('g')
        unmatched = quartet('g', 'editor', 'text', 10000, actual=(10000, 10500, 10000, 10400)) + timing('g')
        short = quartet('g', 'editor', 'text', 10000)[:3] + timing('g')
        untimed = quartet('g', 'editor', 'text', 10000)
        for runs in (failed, unmatched, short, untimed):
            self.assertEqual(verdict(runs), 'incomplete')
        # B2 at the rescaled request matches on its own and counts.
        rescued = quartet('g', 'editor', 'text', 10000, actual=(10000, 10500, 10000, 10050)) + quartet('g', 'mixed', 'mixed', 10000) + timing('g')
        self.assertEqual(verdict(rescued), 'default_on')
        row = next(r for r in result.quality_comparisons(rescued) if r['comparison'] == 'editor')
        self.assertEqual(row['treated_arms'], 'B2')

    def test_planned_comparisons_that_never_ran_are_incomplete(self):
        plan = dict(gpu='7900xt', quality=[dict(comparison='hevc-1920x1080-5000-sheet', codec='hevc', fixture='sheet',
                                                fixture_class='text', target_kbps=5000)], timing=['hevc-1080p60-20000'])
        self.assertEqual(verdict(good(), [plan]), 'incomplete')
        plan = dict(plan, quality=[], timing=['hevc-2160p60-40000'])
        decision = result.decide(good(), [plan])
        self.assertEqual(decision['codecs']['hevc']['verdict'], 'incomplete')
        self.assertIsNone(decision['timing']['7900xt']['ok'])

    def test_gpus_that_disagree_keep_the_lower_verdict(self):
        decision = result.decide(good('7900xt') + quartet('780m', 'editor', 'text', 10000, on=(90.6, 90.6))
                                 + quartet('780m', 'tmnt', 'game', 10000, on=(89.6, 89.6)) + timing('780m'))
        self.assertEqual(decision['codecs']['hevc']['gpus'], {'7900xt': 'default_on', '780m': 'opt_in'})
        self.assertEqual(decision['codecs']['hevc']['verdict'], 'opt_in')
        self.assertTrue(decision['codecs']['hevc']['disagree'])

    def test_opposite_directions_on_two_gpus_are_never_default_on(self):
        # Each GPU on its own would pass, but the game moves up on one and down on the other.
        first = (quartet('a', 'editor', 'text', 10000) + quartet('a', 'tmnt', 'game', 10000, on=(90.15, 90.15))
                 + timing('a'))
        second = (quartet('b', 'editor', 'text', 10000) + quartet('b', 'tmnt', 'game', 10000, on=(89.85, 89.85))
                  + timing('b'))
        for row in first + second:
            if row['kind'] == 'quality' and row['fixture'] == 'tmnt':
                row['scores']['full']['vmaf_mean'] += {'A1': -.05, 'A2': .05}.get(row['arm'], 0)
        codec = result.decide(first + second)['codecs']['hevc']
        self.assertEqual(codec['gpus'], {'a': 'default_on', 'b': 'default_on'})
        self.assertEqual((codec['verdict'], codec['conflicts'], codec['disagree']), ('undecided', ['tmnt'], True))

    def test_codecs_are_decided_apart(self):
        runs = good() + quartet('7900xt', 'av1-editor', 'text', 10000, on=(90., 90.), codec='av1')
        decision = result.decide(runs)
        self.assertEqual({c: v['verdict'] for c, v in decision['codecs'].items()}, {'av1': 'drop', 'hevc': 'default_on'})


class Centre(unittest.TestCase):
    def rows(self, centre, full):
        rows = quartet('g', 'mixed', 'mixed', 10000)
        for row in rows:
            row['scores'].update(centre=scores(80.), periphery=scores(80.))
        for arm, actual in (('C1', 10000), ('C2', 10000)):
            rows.append(dict(rows[1], arm=arm, mode='centre', actual_bitrate_kbps=actual,
                             scores=dict(full=scores(90 + full), centre=scores(80 + centre), periphery=scores(79.))))
        return rows

    def test_centre_stays_opt_in_only_with_a_centre_gain_and_a_small_full_frame_loss(self):
        for centre, full, expected in ((.6, -.4, 'keep_opt_in'), (.4, 0., 'remove'), (1., -.6, 'remove')):
            self.assertEqual(result.decide(self.rows(centre, full))['centre']['g']['verdict'], expected, (centre, full))

    def test_centre_regions_tile_the_frame(self):
        for width, height in ((1920, 1080), (2560, 1440), (3840, 2160)):
            regions = result.centre_regions(width, height)
            area = sum(w * h for rects in regions.values() for _, _, w, h in rects)
            self.assertEqual(area, width * height)
            self.assertFalse(any((x | y | w | h) & 1 for rects in regions.values() for x, y, w, h in rects))


class Scores(unittest.TestCase):
    def write_log(self, path, frames, vmaf=90.):
        path.write_text(json.dumps(dict(frames=[dict(frameNum=i, metrics=dict(vmaf=vmaf + i % 2, psnr_y=40., float_ssim=.98))
                                                for i in reversed(range(frames))])))

    def test_libvmaf_logs_need_every_frame(self):
        with tempfile.TemporaryDirectory() as directory:
            path = pathlib.Path(directory) / 'log.json'
            self.write_log(path, result.FRAMES)
            frames = result.vmaf_frames(path)
            self.assertEqual((len(frames), frames[0], frames[1]), (180, (90., 40., .98), (91., 40., .98)))
            self.write_log(path, result.FRAMES - 1)
            with self.assertRaises(ValueError):
                result.vmaf_frames(path)

    def test_regions_combine_vmaf_by_area_and_psnr_by_error(self):
        combined = result.combine([(3, [(90., 40., 1.)]), (1, [(70., 20., .6)])])
        self.assertAlmostEqual(combined[0][0], 85.)
        self.assertAlmostEqual(combined[0][1], -10 * math.log10(.75e-4 + .25e-2))
        self.assertAlmostEqual(combined[0][2], .9)

    def test_summary_reads_the_settled_window_and_the_fifth_percentile(self):
        frames = [(float(i), 0., 0.) for i in range(180)]
        summary = result.summary(frames)
        self.assertEqual((summary['vmaf_p5'], summary['vmaf_settled'], summary['vmaf_mean']), (8., 74.5, 89.5))

    def test_score_regions_reads_one_log_per_rectangle(self):
        with tempfile.TemporaryDirectory() as directory:
            directory = pathlib.Path(directory)
            self.write_log(directory / result.log_name('full', 0), 180, 90.)
            self.write_log(directory / result.log_name('still', 0), 180, 80.)
            self.write_log(directory / result.log_name('still', 1), 180, 60.)
            summary = result.score_regions(directory, dict(full=[[0, 0, 64, 64]], still=[[0, 0, 32, 32], [32, 0, 32, 32]]))
            self.assertAlmostEqual(summary['full']['vmaf_mean'], 90.5)
            self.assertAlmostEqual(summary['still']['vmaf_mean'], 70.5)

    def test_levels_rank_by_full_frame_gain(self):
        base = dict(kind='levels', gpu='g', status='ok', actual_bitrate_kbps=10000)
        runs = [dict(base, fixture=f, levels=None, scores=dict(full=scores(90.))) for f in ('mixed', 'editor')]
        runs += [dict(base, fixture=f, levels=l, scores=dict(full=scores(90 + gain)))
                 for l, gain in (('still=4', .5), ('still=8', 1.)) for f in ('mixed', 'editor')]
        ranked = result.level_ranking(runs)
        self.assertEqual([(r['rank'], r['levels'], r['fixtures']) for r in ranked], [(1, 'still=8', 2), (2, 'still=4', 2)])


LOG = '\n'.join((
    '2026-10-26T10:00:00Z  INFO butterpollo::stream: encoder focus: requested=static effective=static levels="moving=2,still=6"',
    *(f'2026-10-26T10:00:{5 * i:02}Z  INFO butterpollo::stream: stream timings frames=300 present_to_sent_ms=1.0/2.0/{p}'
      f' capture_age_at_submit_ms=0.5/0.8/{p / 2}' for i, p in enumerate((9., 3., 5.))),
    '2026-10-26T10:00:12Z  WARN butterpollo::stream: a streamed video frame passed the byte cap its blocks were planned for; '
    'slice delivery is off for this stream',
    '\x1b[2m2026-10-26T10:00:15Z\x1b[0m  INFO butterpollo::stream: encoder focus maps=10 uniform=1 stale=0 skipped_recovery=0 qp_delta=-2..4',
    '2026-10-26T10:00:20Z  INFO butterpollo::stream: encoder focus maps=600 uniform=3 stale=4 skipped_recovery=1 qp_delta=-2..4'))


class HostLog(unittest.TestCase):
    def test_stream_timings_overflows_and_focus_counters(self):
        stats = result.host_stats(LOG)
        self.assertEqual(stats['windows'], 2)
        self.assertAlmostEqual(stats['present_to_sent_p99_ms'], 4.)
        self.assertAlmostEqual(stats['capture_age_at_submit_p99_ms'], 2.)
        self.assertEqual(stats['overflows'], 1)
        self.assertEqual(stats['focus'], dict(maps=600, uniform=3, stale=4, skipped_recovery=1))
        self.assertEqual((stats['focus_qp_delta'], stats['focus_effective']), ('-2..4', 'static'))
        self.assertAlmostEqual(result.focus_fraction(stats['focus']), 4 / 608)

    def test_a_stream_without_focus_has_no_counters(self):
        stats = result.host_stats('stream timings present_to_sent_ms=1/2/3\nstream timings present_to_sent_ms=1/2/4\n')
        self.assertEqual((stats['focus'], stats['focus_effective'], stats['present_to_sent_p99_ms']), (None, None, 4.))


class Summary(unittest.TestCase):
    def test_summarize_writes_the_tables_and_prints_the_verdict(self):
        with tempfile.TemporaryDirectory() as directory:
            work = pathlib.Path(directory)
            (work / 'runs.jsonl').write_text(''.join(json.dumps(r) + '\n' for r in good()))
            (work / 'plan.json').write_text(json.dumps(dict(gpu='7900xt', quality=[], timing=['hevc-1080p60-20000'])))
            output = io.StringIO()
            with contextlib.redirect_stdout(output):
                summary = result.summarize([work])
            self.assertEqual(json.loads(output.getvalue())['verdict'], {'hevc': 'default_on'})
            self.assertEqual(summary['codecs']['hevc']['verdict'], 'default_on')
            for name in ('quality.csv', 'timing.csv', 'levels.csv', 'summary.json'):
                self.assertTrue((work / name).exists(), name)
            self.assertIn('| hevc | default_on |', (work / 'summary.md').read_text(encoding='utf-8'))


def installed(error=None):
    """A stand-in e2e_host whose installed_idle passes or raises."""
    def installed_idle():
        if error:
            raise error
    return mock.patch.dict(sys.modules, e2e_host=types.SimpleNamespace(installed_idle=installed_idle))


class Driver(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.root = pathlib.Path(self.directory.name)
        self.work = self.root / 'work'
        for name in ('amf_quality.exe', 'ffmpeg.exe', 'butterpollo.exe', 'client.exe'):
            (self.root / name).write_bytes(b'')
        regions = dict(still=[[0, 0, 960, 1080]], moving=[[960, 0, 960, 1080]])
        self.fixtures = mock.patch.object(ab, 'fixtures', return_value=[('mixed', 'mixed', self.root, regions)])
        self.fixtures.start()
        self.quiet = contextlib.redirect_stdout(io.StringIO())
        self.quiet.__enter__()
        self.addCleanup(self.directory.cleanup)
        self.addCleanup(self.fixtures.stop)
        self.addCleanup(self.quiet.__exit__, None, None, None)

    def quality(self, *extra):
        with contextlib.redirect_stderr(io.StringIO()):
            return ab.main(['quality', '--work', str(self.work), '--amf-quality', str(self.root / 'amf_quality.exe'),
                            '--ffmpeg', str(self.root / 'ffmpeg.exe'), '--gpu', '7900xt', '--plan', 'm4', *extra])

    def runs(self):
        return [json.loads(line) for line in (self.work / 'runs.jsonl').read_text().splitlines()]

    def test_a_busy_installed_host_refuses_the_run(self):
        encode = mock.Mock()
        with installed(AssertionError('RustHostSessionCount is 1')), mock.patch.object(ab, 'encode', encode):
            self.assertEqual(self.quality(), 1)
        encode.assert_not_called()
        self.assertEqual([(r['arm'], r['status']) for r in self.runs()], [('A1', 'failed')])
        self.assertIn('busy', self.runs()[0]['error'])

    def test_an_unreachable_installed_host_refuses_too(self):
        with installed(ConnectionError('refused')), mock.patch.object(ab, 'encode') as encode:
            self.assertEqual(self.quality(), 1)
        encode.assert_not_called()

    def test_an_encoder_hang_stops_with_exit_code_3_and_no_retry(self):
        encode = mock.Mock(side_effect=ab.Hang('slow encoder call call="AMF QueryOutput" elapsed_ms=3000'))
        with installed(), mock.patch.object(ab, 'encode', encode):
            self.assertEqual(self.quality(), EXIT_GPU_HANG)
        self.assertEqual(encode.call_count, 1)
        self.assertEqual([(r['arm'], r['status']) for r in self.runs()], [('A1', 'failed')])
        self.assertIn('Do not retry', (self.work / 'status.txt').read_text())

    def test_a_used_work_folder_is_refused(self):
        self.work.mkdir()
        (self.work / 'runs.jsonl').write_text('')
        with installed(), mock.patch.object(ab, 'encode') as encode:
            self.assertEqual(self.quality(), 1)
        encode.assert_not_called()

    def test_the_quartet_rescales_b2_from_a1_and_b1(self):
        actual = iter((10000, 9000, 10000, 10000))
        requests = []

        def fake(args, kind, comparison, job, fixture, name, mode, request, levels, regions):
            requests.append((name, mode, request))
            return dict(actual_bitrate_kbps=next(actual))
        with mock.patch.object(ab, 'arm', fake):
            args = types.SimpleNamespace(levels=None)
            ab.quartet(args, ('hevc', 1920, 1080, 10000, False), ('mixed', 'mixed', self.root, {}), 'static')
        self.assertEqual(requests, [('A1', 'off', 10000), ('B1', 'static', 10000), ('A2', 'off', 10000), ('B2', 'static', 11111)])

    def test_an_arm_whose_focus_did_not_run_as_asked_is_fatal(self):
        metrics = dict(encoded_frames=180, backend='amf', actual_bitrate_kbps=10000, focus_mode='off', focus_maps=0)
        with installed(), mock.patch.object(ab, 'encode', return_value=(metrics, 'null', [])), \
                mock.patch.object(ab.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0)) as run, \
                mock.patch.object(ab, 'score_regions', return_value={}):
            self.assertEqual(self.quality(), 1)
        # A1 (off, no maps) passes the check and is scored; B1 asked for static and got none.
        self.assertEqual(run.call_count, 1)
        self.assertEqual([(r['arm'], r['status']) for r in self.runs()][1], ('B1', 'failed'))
        self.assertIn('focus was set to static', self.runs()[1]['error'])

    def test_focus_reports(self):
        ab.check_focus(dict(focus_mode='off', focus_maps=0), 'off')
        ab.check_focus(dict(focus_mode='static', focus_maps=180), 'static')
        ab.check_focus(dict(focus_mode='center', focus_maps=180), 'centre')
        for metrics, mode in ((dict(), 'off'), (dict(focus_mode='off', focus_maps=3), 'off'),
                              (dict(focus_mode='static', focus_maps=0), 'static'), (dict(focus_mode='off'), 'static')):
            with self.assertRaises(RuntimeError, msg=(metrics, mode)):
                ab.check_focus(metrics, mode)

    def test_host_config_sets_tuning_only_on_the_on_arms(self):
        self.assertEqual(ab.host_config('off', 'still=6', True), 'encoder_focus = off\n')
        self.assertEqual(ab.host_config('static', 'moving=2,still=6', True),
                         'encoder_focus = static\nencoder_focus_levels = moving=2,still=6\nencoder_focus_verify = true\n')

    def test_scoring_trims_both_branches_and_logs_every_rectangle(self):
        regions = dict(full=[[0, 0, 1920, 1080]], still=[[0, 0, 960, 1080]], moving=[[960, 0, 960, 1080]])
        _, chain = ab.source(self.root, 1920, 1080, False)
        graph, outputs = ab.score_graph(regions, 1920, 1080, False, chain)
        self.assertEqual(graph.count('trim=end_frame=180,settb=1/60,setpts=N'), 2)
        self.assertEqual(outputs, ['[o0]', '[o1]', '[o2]'])
        for name in ('vmaf-full-0.json', 'vmaf-still-0.json', 'vmaf-moving-0.json'):
            self.assertIn(f'log_path={name}', graph)
        self.assertIn('out_color_matrix=bt709:out_range=tv', graph)
        _, chain = ab.source(self.root / 'clip.mkv', 2560, 1440, True)
        graph, _ = ab.score_graph(dict(full=[[0, 0, 2560, 1440]]), 2560, 1440, True, chain)
        self.assertIn('t=smpte2084', graph)
        self.assertIn('format=yuv420p10le,trim', graph)

    def timing_run(self, *extra):
        with contextlib.redirect_stderr(io.StringIO()):
            return ab.main(['timing', '--work', str(self.work), '--package', str(self.root), '--client', str(self.root / 'client.exe'),
                            '--gpu', '7900xt', '--plan', 'm4', *extra])

    def test_a_stream_that_exits_3_stops_the_timing_run(self):
        run = mock.Mock(return_value=subprocess.CompletedProcess([], EXIT_GPU_HANG))
        with installed(), mock.patch.object(ab.subprocess, 'run', run):
            self.assertEqual(self.timing_run(), EXIT_GPU_HANG)
        self.assertEqual(run.call_count, 1)
        command = run.call_args[0][0]
        self.assertIn('encoder_focus=off', command)
        self.assertEqual(command[command.index('--port') + 1], '48523')
        self.assertIn('STOPPED', (self.work / 'status.txt').read_text())

    def test_the_timing_port_block_misses_the_installed_host(self):
        ab.check_port(48523)
        for port in (47984, 48015, 47980, 1025):
            with self.assertRaises(ValueError, msg=port):
                ab.check_port(port)
        with installed(), mock.patch.object(ab.subprocess, 'run') as run:
            self.assertEqual(self.timing_run('--port', '47990'), 1)
        run.assert_not_called()

    def test_estimates(self):
        for plan in ('m2', 'm3', 'm4', 'levels'):
            self.assertGreater(ab.estimate(plan)['minutes'], 0, plan)
        self.assertEqual(ab.estimate('m4')['encode_arms'], 8)
        self.assertEqual(ab.estimate('levels')['encode_arms'], 26)
        self.assertEqual(ab.estimate('m3')['stream_arms'], 12)


if __name__ == '__main__':
    unittest.main()
