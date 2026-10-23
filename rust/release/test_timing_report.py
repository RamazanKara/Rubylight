import contextlib, io, json, pathlib, shutil, tempfile, unittest
import timing_report
from soak_result import percentile
from timing_report import (WRAP_US, build_report, cadence, case_report, check_bound, drop_warmup, main, read_host_record,
                           summarize, unwrap_rtp)

FIXTURES = pathlib.Path(__file__).parent / 'fixtures' / 'timing'
SHARED = FIXTURES / 'frames-shared.csv'
HOST_SHARED = FIXTURES / 'host-shared.csv'


def report(client, record=None, join=None):
    return build_report(FIXTURES / client, FIXTURES / record if record else None, join)[0]


class Temporary(unittest.TestCase):
    def setUp(self):
        self.directory = pathlib.Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.directory, True)

    def copy(self, name, edit=lambda text: text):
        target = self.directory / name
        target.write_bytes(edit((FIXTURES / name).read_text()).encode())
        return target


class SharedClock(unittest.TestCase):
    """The C receiver on the host's PC: both read QPC, so the bound is 0."""

    def setUp(self):
        self.report = report('frames-shared.csv', 'host-shared.csv')

    def test_the_join_is_chosen_and_the_bound_is_zero(self):
        self.assertEqual(self.report['join'], 'shared_clock')
        self.assertEqual(self.report['bound_ms'], 0)
        for name in ('network_first', 'network_last', 'end_to_end'):
            self.assertEqual(self.report['stages'][name]['bound_ms'], 0)
            self.assertFalse(self.report['stages'][name]['lower_bound'])

    def test_host_stages_come_from_the_record(self):
        stages = self.report['stages']
        self.assertEqual(stages['encode']['median_ms'], 2.0)
        self.assertEqual(stages['packetize_pace']['median_ms'], .4)
        self.assertEqual(stages['convert']['median_ms'], .25)
        self.assertEqual(stages['hold_claim']['n'], 11)
        self.assertEqual(stages['hold_claim_convert']['n'], 0)
        self.assertEqual(stages['capture']['p95_ms'], .45)

    def test_cross_clock_stages_use_the_shared_epoch(self):
        stages = self.report['stages']
        # 150, 175, 200 us to the first packet in the generator; 250 and 290 us to the last.
        self.assertAlmostEqual(stages['network_first']['median_ms'], .175)
        self.assertAlmostEqual(stages['network_first']['p99_ms'], .2)
        self.assertAlmostEqual(stages['network_last']['median_ms'], .25)
        self.assertAlmostEqual(stages['end_to_end']['median_ms'], 10.534, places=3)

    def test_client_stages_are_in_the_receivers_clock(self):
        stages = self.report['stages']
        self.assertEqual(stages['decode']['n'], 12)
        self.assertEqual(stages['decode']['median_ms'], 4.0)
        self.assertEqual(stages['client_queue']['n'], 0)
        self.assertEqual(stages['display']['n'], 0)
        self.assertAlmostEqual(stages['client_total']['median_ms'], 4.95)

    def test_unmatched_rows_are_counted(self):
        # The host recorded 100..112; the receiver lost 105 and 112 and has 120.
        self.assertEqual(self.report['host_rows'], 13)
        self.assertEqual(self.report['unmatched'], dict(client_without_host=1, host_without_client=2))
        self.assertEqual(self.report['matched'], 11)
        self.assertEqual(self.report['host_dropped'], 2)
        self.assertEqual(self.report['stages']['encode']['n'], 11)
        self.assertEqual(self.report['stages']['decode']['n'], 12)

    def test_the_rtp_wrap_does_not_leak_into_game_to_present(self):
        # The generator crosses the 32-bit RTP wrap at frame 101: rtp_us restarts near zero.
        record = read_host_record(HOST_SHARED)
        self.assertEqual(record['rows'][100]['rtp'], 47721858000)
        self.assertLess(record['rows'][101]['rtp'], 20000)
        stage = self.report['stages']['game_to_present']
        self.assertEqual(stage['n'], 11)
        self.assertAlmostEqual(stage['median_ms'], 1.602, places=3)
        self.assertLess(stage['p99_ms'], 2)
        self.assertEqual(stage['negative'], 0)

    def test_capture_sourced_present_stamps_skip_game_to_present(self):
        record = self.directory_copy(lambda text: text.replace('present_source=etw', 'present_source=capture'))
        stage = build_report(SHARED, record)[0]['stages']['game_to_present']
        self.assertEqual(stage['n'], 0)

    def directory_copy(self, edit):
        directory = pathlib.Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, directory, True)
        target = directory / 'host-1.csv'
        target.write_text(edit(HOST_SHARED.read_text()))
        return target

    def test_the_receiver_clock_is_rebased_to_qpc_with_arrival_ms(self):
        # first_packet_us and assembled_us count from the receiver's start, 67.89 s before the host's media epoch;
        # arrival_ms is absolute QPC and later than assembly by the software queue on two rows in three.
        client = timing_report.read_client(SHARED)
        self.assertLess(max(f['assembled'] for f in client['frames']), 1e11)
        self.assertEqual(timing_report.qpc_base(client['frames']), 1234500000500)
        record = read_host_record(HOST_SHARED)
        self.assertEqual(timing_report.shared_offset(client['frames'], record), 1234567890500 - 1234500000500)
        self.assertAlmostEqual(self.report['stages']['network_last']['median_ms'], .25)

    def test_the_minimum_strips_the_software_queue(self):
        lines = SHARED.read_text().splitlines()
        header = lines[0].split(',')
        column = header.index('arrival_ms')
        rows = [line.split(',') for line in lines[1:]]
        for cells in rows:
            cells[column] = f'{float(cells[column]) + .5:.6f}'  # every row waited 0.5 ms for the decoder
        path = self.directory_copy(lambda text: text)
        client = path.parent / 'frames.csv'
        client.write_bytes(('\n'.join([lines[0]] + [','.join(c) for c in rows]) + '\n').encode())
        # The base is 0.5 ms late, so every cross-clock stage is 0.5 ms long: the queue that every row shares is the
        # one thing the minimum cannot strip. A hardware decoder's arrival_ms has none.
        result = build_report(client, HOST_SHARED)[0]
        self.assertEqual(result['join'], 'shared_clock')
        self.assertAlmostEqual(result['stages']['network_first']['median_ms'], .175 + .5)

    def test_without_arrival_ms_the_receiver_cannot_be_rebased(self):
        lines = SHARED.read_text().splitlines()
        column = lines[0].split(',').index('arrival_ms')
        rows = [line.split(',') for line in lines]
        for cells in rows:
            del cells[column]
        path = self.directory_copy(lambda text: text).parent / 'frames.csv'
        path.write_bytes(('\n'.join(','.join(c) for c in rows) + '\n').encode())
        self.assertEqual(build_report(path, HOST_SHARED)[0]['join'], 'min_owd')
        with self.assertRaisesRegex(ValueError, 'arrival_ms'):
            build_report(path, HOST_SHARED, 'shared_clock')

    def test_a_forced_join_that_the_clocks_do_not_support_is_an_error(self):
        with self.assertRaisesRegex(ValueError, 'clock_offset_us'):
            report('frames-shared.csv', 'host-shared.csv', join='clock_sync')

    def test_the_table_names_the_join_and_the_bound(self):
        text = timing_report.format_table(self.report)
        self.assertIn('join: shared_clock', text)
        self.assertIn('+/- 0 ms', text)
        self.assertRegex(text, r'network_first +11 +0\.175 .*\+/- 0\.000')
        self.assertRegex(text, r'client_queue +0 +- +- +-')


class MinOwd(unittest.TestCase):
    """The receiver on another machine: no shared clock, so the minimum one-way delay is the offset."""

    def setUp(self):
        self.report = report('frames-nuc.csv', 'host-nuc.csv')

    def test_unrelated_clocks_fall_back_to_min_owd(self):
        self.assertEqual(self.report['join'], 'min_owd')
        self.assertIsNone(self.report['bound_ms'])
        for name in ('network_first', 'network_last', 'end_to_end'):
            self.assertTrue(self.report['stages'][name]['lower_bound'])
            self.assertIsNone(self.report['stages'][name]['bound_ms'])

    def test_network_stages_are_the_delay_above_the_minimum(self):
        stage = self.report['stages']['network_first']
        # one-way delays 400, 550, 480, 900, 420, 610, 700, 405, 1500, 460 us; the minimum is 400
        self.assertEqual(stage['n'], 10)
        self.assertEqual(stage['negative'], 0)
        self.assertAlmostEqual(stage['median_ms'], .08)
        self.assertAlmostEqual(stage['p99_ms'], .5)  # nearest rank, floor: the ninth of ten
        self.assertIn('lower bound', timing_report.format_table(self.report))

    def test_forcing_shared_clock_on_another_machines_receiver_fails_loudly(self):
        with self.assertRaisesRegex(ValueError, 'do not fit the host QPC'):
            report('frames-nuc.csv', 'host-nuc.csv', join='shared_clock')

    def test_min_owd_can_be_forced_on_a_shared_clock_file(self):
        forced = report('frames-shared.csv', 'host-shared.csv', join='min_owd')
        self.assertEqual(forced['join'], 'min_owd')
        # the minimum first-packet delay in that file is 150 us, the median 175 us
        self.assertAlmostEqual(forced['stages']['network_first']['median_ms'], .025)


class ClockSync(Temporary):
    def test_nightjar_csv_carries_its_own_host_columns(self):
        result = report('nightjar-clock-sync.csv')
        self.assertEqual(result['join'], 'clock_sync')
        # median of clock_rtt_us 6000..7500 us over 11 rows, halved
        self.assertAlmostEqual(result['bound_ms'], 3.5)
        stages = result['stages']
        self.assertEqual(stages['network_first']['bound_ms'], result['bound_ms'])
        self.assertEqual(stages['hold_claim']['n'], 0)
        self.assertEqual(stages['hold_claim_convert']['n'], 11)
        self.assertEqual(stages['encode']['median_ms'], 2.0)
        self.assertEqual(stages['display']['n'], 11)
        self.assertEqual(stages['display']['median_ms'], 8.0)
        self.assertEqual(stages['client_total']['n'], 11)

    def test_a_dropped_frame_has_no_display_and_a_frame_without_a_report_has_no_host_stages(self):
        result = report('nightjar-clock-sync.csv')
        self.assertEqual(result['rows'], 12)
        self.assertEqual(result['unmatched']['client_without_host'], 1)
        self.assertEqual(result['stages']['decode']['n'], 12)
        self.assertEqual(result['stages']['encode']['n'], 11)
        self.assertEqual(result['stages']['end_to_end']['n'], 10)

    def test_the_offset_error_is_inside_the_bound(self):
        stage = report('nightjar-clock-sync.csv')['stages']['network_first']
        # 3000 us of path plus 0..1200 us, with the offset off by -150..+250 us
        self.assertGreater(stage['median_ms'], 3.0)
        self.assertLess(stage['p99_ms'], 3.0 + 1.2 + .25 + 0.001)

    def test_a_host_record_splits_hold_claim_from_convert(self):
        result = report('nightjar-clock-sync.csv', 'host-shared.csv')
        self.assertEqual(result['join'], 'clock_sync')
        self.assertEqual(result['stages']['hold_claim']['n'], 12)
        self.assertEqual(result['stages']['convert']['median_ms'], .25)
        self.assertEqual(result['stages']['hold_claim_convert']['n'], 0)
        self.assertEqual(result['unmatched'], dict(client_without_host=0, host_without_client=1))

    def test_a_row_without_an_offset_keeps_its_same_clock_stages(self):
        result = report('frames-clock-sync.csv')
        stages = result['stages']
        self.assertEqual(result['join'], 'clock_sync')
        self.assertAlmostEqual(result['bound_ms'], .105)
        # 11 rows: 103 has no offset and 109 has no host columns
        self.assertEqual(stages['encode']['n'], 10)
        self.assertEqual(stages['network_first']['n'], 9)
        self.assertEqual(stages['decode']['n'], 11)

    def test_a_file_before_the_nine_columns_reports_the_client_only(self):
        result = report('nightjar-v1.csv')
        self.assertIsNone(result['join'])
        self.assertEqual(result['unmatched']['client_without_host'], 6)
        self.assertEqual(result['stages']['encode']['n'], 0)
        self.assertEqual(result['stages']['network_first']['n'], 0)
        self.assertEqual(result['stages']['assemble']['n'], 6)
        self.assertEqual(result['stages']['display']['n'], 5)
        self.assertTrue(any('host_present_us' in note for note in result['notes']))

    def test_a_host_record_joins_a_file_without_the_nine_columns_by_min_owd(self):
        result = report('nightjar-v1.csv', 'host-shared.csv')
        self.assertEqual(result['join'], 'min_owd')
        self.assertEqual(result['stages']['encode']['n'], 6)
        self.assertEqual(result['stages']['network_first']['n'], 6)


class BoundCheck(Temporary):
    def test_the_fixture_offsets_are_inside_rtt_over_two(self):
        check = check_bound(build_report(FIXTURES / 'frames-clock-sync.csv', HOST_SHARED)[1]['frames'],
                            read_host_record(HOST_SHARED))
        self.assertTrue(check['passed'])
        self.assertEqual((check['rows'], check['outside']), (10, 0))
        self.assertEqual(check['max_error_us'], 90)

    def test_cli_passes_and_fails_with_the_exit_code(self):
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            code = main([str(FIXTURES / 'frames-clock-sync.csv'), '--host-record', str(HOST_SHARED), '--check-bound'])
        self.assertEqual(code, 0)
        self.assertIn('bound check: PASS', output.getvalue())
        shifted = self.copy('frames-clock-sync.csv')
        lines = shifted.read_text().splitlines()
        header = lines[0].split(',')
        column = header.index('clock_offset_us')
        out = [lines[0]]
        for line in lines[1:]:
            cells = line.split(',')
            if cells[column]:
                cells[column] = str(int(cells[column]) + 500)
            out.append(','.join(cells))
        shifted.write_text('\n'.join(out) + '\n')
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            code = main([str(shifted), '--host-record', str(HOST_SHARED), '--check-bound'])
        self.assertEqual(code, 1)
        self.assertIn('bound check: FAIL - 10 of 10', output.getvalue())

    def test_one_percent_is_the_limit(self):
        record = dict(epoch_qpc_ms=1000.0)
        frames = lambda outside: [dict(off=1_000_000 + (500 if i < outside else 0), rtt=200) for i in range(200)]
        self.assertTrue(check_bound(frames(2), record)['passed'])
        self.assertFalse(check_bound(frames(3), record)['passed'])

    def test_without_offsets_or_a_record_it_is_an_error(self):
        with self.assertRaisesRegex(ValueError, 'clock_offset_us'):
            check_bound([dict(off=None, rtt=None)], read_host_record(HOST_SHARED))
        with self.assertRaisesRegex(ValueError, 'media_epoch_qpc_ms'):
            check_bound([dict(off=1, rtt=2)], None)


class Cadence(Temporary):
    """present_interval and the cadence errors: consecutive-frame gaps, lost frames, the RTP wrap."""
    HOST_HEAD = ('# rubylight timing record 1\n# media_epoch_qpc_ms=1000.0\n# codec=h264 capture=wgc present_source=etw\n'
                 'wire_frame,rtp_us,present_us,acquired_us,published_us,claimed_us,submitted_us,encoded_us,packetized_us,'
                 'prepared_us,first_sent_us,last_sent_us\n')

    def host_record(self, rows):
        """rows: (wire_frame, present_us, last_sent_us); the RTP stamp is the present time on the 90 kHz wrap."""
        lines = [self.HOST_HEAD]
        for frame, present, sent in rows:
            rtp = round(present * 90 / 1000) % (1 << 32) * 1000 / 90
            lines.append(f'{frame},{rtp:.0f},{present},,,,,,,,{sent},{sent}\n')
        path = self.directory / 'host-1.csv'
        path.write_text(''.join(lines) + '# dropped=0\n')
        return path

    def client_csv(self, rows):
        """rows: (frame_number, rtp_host_us, enqueue_us, render_us or None); None is a frame the client dropped."""
        lines = ['frame_number,receive_ns,enqueue_ns,render_ns,status,rtp_host_us,host_first_sent_us\n']
        for frame, rtp, enqueue, render in rows:
            status = 'rendered' if render is not None else 'dropped'
            shown = '' if render is None else round(render * 1000)
            lines.append(f'{frame},{round(enqueue * 1000 - 500)},{round(enqueue * 1000)},{shown},{status},{rtp:.0f},{rtp:.0f}\n')
        path = self.directory / 'client.csv'
        path.write_text(''.join(lines))
        return path

    def stages(self, client, record=None):
        return build_report(client, record)[0]['stages']

    def test_the_host_adds_the_gap_between_last_sent_and_present_gaps(self):
        # The game presents every 11111 us; the host sends 3000, 3400, 3000, 5000, 3000 us after each present.
        offsets = [3000, 3400, 3000, 5000, 3000]
        rows = [(i, 1_000_000 + i * 11111, 1_000_000 + i * 11111 + o) for i, o in enumerate(offsets)]
        stages = self.stages(self.client_csv([(i, 0, 0, None) for i in range(5)]), self.host_record(rows))
        self.assertEqual(stages['present_interval']['n'], 4)
        self.assertEqual(stages['present_interval']['median_ms'], 11.111)
        self.assertEqual(stages['present_interval']['p99_ms'], 11.111)
        # |+400|, |-400|, |+2000|, |-2000| us.
        error = stages['host_cadence_error']
        self.assertEqual((error['n'], error['median_ms'], error['p99_ms']), (4, .4, 2.0))

    def test_a_repeat_gap_in_the_host_record_is_a_long_interval(self):
        # Rows exist only for new pictures; frame 2 was a static repeat that used up its number. Frames 0, 1, 3, 4 are
        # 8000 us apart except the 1 -> 3 pair, which is a real 16000 us interval. Frame 3 goes out 700 us later than
        # the others, so its two pairs are 700 us off in the error.
        rows = [(i, 1_000_000 + i * 8000, 1_000_000 + i * 8000 + 2000 + (700 if i == 3 else 0)) for i in (0, 1, 3, 4)]
        stages = self.stages(self.client_csv([(i, 0, 0, None) for i in (0, 1, 3, 4)]), self.host_record(rows))
        self.assertEqual(stages['present_interval']['n'], 3)
        self.assertEqual(stages['present_interval']['median_ms'], 8.0)
        error = stages['host_cadence_error']
        self.assertEqual((error['n'], error['median_ms'], error['p99_ms']), (3, .7, .7))
        points = [(i, p, (p, False)) for i, p in ((0, 0), (1, 8000), (3, 24000), (4, 32000))]
        self.assertEqual(cadence(points, True), ([8000, 16000, 8000], [0, 0, 0]))
        # A series that is not all pictures (the client's) pairs only adjacent frame numbers for the interval.
        self.assertEqual(cadence(points), ([8000, 8000], [0, 0, 0]))

    def test_a_pair_on_two_clocks_has_no_error(self):
        # Frame 1 has only an RTP stamp, the others a present time: the pairs that meet it are skipped.
        points = [(0, 0, (0, False)), (1, 8000, (8000, True)), (2, 16000, (16000, False)), (3, 24500, (24000, False))]
        self.assertEqual(cadence(points), ([8000, 8000, 8500], [500]))

    def test_the_client_error_follows_the_rtp_gaps_of_the_frames_it_showed(self):
        # Host stamps 8333 us apart. Frame 3 is dropped by the client; frame 4 is shown 16000 us after frame 2, for a
        # host gap of 16666 us.
        rtp = [90_000 + i * 8333 for i in range(5)]
        render = [None] * 5
        render[0], render[1], render[2], render[4] = 100_000, 108_334, 116_667, 132_667
        rows = [(i, rtp[i], 99_000 + i * 8333, render[i]) for i in range(5)]
        stages = self.stages(self.client_csv(rows))
        # Shown frames 0, 1, 2 and 4: the interval only for the adjacent pairs 0-1 and 1-2.
        self.assertEqual(stages['client_present_interval']['n'], 2)
        self.assertEqual(stages['client_present_interval']['median_ms'], 8.333)
        self.assertEqual(stages['client_cadence_error']['n'], 3)
        shown = [(i, render[i], (rtp[i], True)) for i in range(5) if render[i] is not None]
        self.assertEqual(cadence(shown), ([8334, 8333], [1, 0, 666]))
        self.assertEqual(stages['arrival_interval']['n'], 4)
        self.assertEqual(stages['arrival_interval']['median_ms'], 8.333)
        self.assertEqual(stages['arrival_cadence_error']['p99_ms'], 0)

    def test_the_rtp_wrap_is_not_an_interval_error(self):
        # Four frames whose RTP stamps cross 2**32 ticks: the gap is 11111 us across the wrap.
        base = WRAP_US - 22222
        rows = [(i, (base + i * 11111) % WRAP_US, 5_000 + i * 11111, 6_000 + i * 11111) for i in range(4)]
        stages = self.stages(self.client_csv(rows))
        self.assertEqual(stages['client_cadence_error']['n'], 3)
        self.assertLess(stages['client_cadence_error']['p99_ms'], .002)
        self.assertLess(stages['arrival_cadence_error']['p99_ms'], .002)
        self.assertEqual(stages['client_present_interval']['median_ms'], 11.111)

    def test_a_hitch_in_the_game_is_content_not_error(self):
        # A 100 ms hitch in the game: the host follows it exactly, so there is no error, but the interval shows it.
        presents = [0, 11111, 111111, 122222, 222222, 233333, 333333, 344444, 444444]
        rows = [(i, 1_000_000 + p, 1_000_000 + p + 2500) for i, p in enumerate(presents)]
        stages = self.stages(self.client_csv([(i, 0, 0, None) for i in range(5)]), self.host_record(rows))
        self.assertEqual(stages['host_cadence_error']['p99_ms'], 0)
        self.assertEqual(stages['present_interval']['p99_ms'], 100.0)

    def test_skip_first_drops_the_warm_up_on_both_sides(self):
        # 2 s of frames 8 ms apart; the first second is warm-up with a 5 ms error on every send.
        rows = [(i, 1_000_000 + i * 8000, 1_000_000 + i * 8000 + 2000 + (5000 * (i % 2) if i < 125 else 0)) for i in range(250)]
        record = self.host_record(rows)
        client = self.client_csv([(i, 0, 0, None) for i in range(250)])
        everything = build_report(client, record)[0]['stages']
        later = build_report(client, record, skip_first=1)[0]
        self.assertGreater(everything['host_cadence_error']['p99_ms'], 4.9)
        self.assertEqual(later['stages']['host_cadence_error']['p99_ms'], 0)
        self.assertEqual(later['stages']['present_interval']['n'], 124)
        self.assertEqual(later['host_rows'], 125)
        self.assertEqual(later['rows'], 125)
        with self.assertRaisesRegex(ValueError, 'shorter'):
            build_report(client, record, skip_first=5)

    def test_skip_first_uses_one_clock_for_the_whole_file(self):
        # Present and acquire times are on different scales. Row 100 has no present time, so the whole file is cut by
        # acquire time (row 100 is 1 s in); a per-row choice would mix scales and cut at row 101.
        rows = {i: dict(present=None if i == 100 else 1_000_000 + i * 10_000, acquired=5000 + i * 10_000)
                for i in range(200)}
        record = dict(rows=dict(rows))
        client = dict(frames=[dict(frame_number=i, host=dict(present=None), first_packet=0) for i in range(200)])
        drop_warmup(client, record, 1)
        self.assertEqual(min(record['rows']), 100)
        self.assertEqual(min(f['frame_number'] for f in client['frames']), 100)
        # With a present time on every row that column is used.
        rows[100]['present'] = 1_000_000 + 100 * 10_000
        record = dict(rows=dict(rows))
        drop_warmup(dict(frames=[]), record, 1)
        self.assertEqual(min(record['rows']), 100)

    def test_a_file_without_pairs_reports_none_and_prints_no_rows(self):
        path = self.client_csv([(7, 5, 1000, 2000)])
        stages = self.stages(path)
        for name in ('present_interval', 'host_cadence_error', 'client_present_interval', 'client_cadence_error'):
            self.assertEqual(stages[name]['n'], 0, name)
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            self.assertEqual(main([str(path)]), 0)
        self.assertNotIn('cadence_error', output.getvalue())

    def test_the_fixtures_give_cadence_rows(self):
        stages = report('nightjar-clock-sync.csv', 'host-shared.csv')['stages']
        self.assertGreater(stages['present_interval']['n'], 5)
        self.assertAlmostEqual(stages['present_interval']['median_ms'], 16.667, places=0)
        self.assertGreater(stages['host_cadence_error']['n'], 5)
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            main([str(FIXTURES / 'nightjar-clock-sync.csv'), '--host-record', str(HOST_SHARED)])
        self.assertIn('host_cadence_error', output.getvalue())


class Formats(Temporary):
    def test_percentiles_are_the_release_reports_nearest_rank(self):
        values = [i * 1000 for i in range(1, 11)]
        stage = summarize(values)
        self.assertEqual(stage['median_ms'], percentile([v / 1000 for v in values], 50))
        self.assertEqual(stage['median_ms'], 5)
        self.assertEqual(stage['p95_ms'], 9)
        self.assertEqual(stage['p99_ms'], 9)
        self.assertEqual(summarize([-1000, 2000])['negative'], 1)

    def test_unwrap_rtp_picks_the_value_next_to_the_anchor(self):
        anchor = 47721874667
        self.assertEqual(unwrap_rtp(47721858000, anchor), 47721858000)
        self.assertAlmostEqual(unwrap_rtp(15822, anchor), 15822 + WRAP_US, delta=1)
        # the other way: the host stamp is just past the wrap and the RTP value just before
        self.assertAlmostEqual(unwrap_rtp(int(WRAP_US) - 100, 50), -100.44, delta=1)
        self.assertIsNone(unwrap_rtp(None, 5))

    def test_host_record_header_and_trailer(self):
        record = read_host_record(HOST_SHARED)
        self.assertEqual((record['epoch_qpc_ms'], record['codec'], record['capture'], record['present_source']),
                         (1234567890.5, 'h264', 'wgc', 'etw'))
        self.assertEqual((record['dropped'], len(record['rows'])), (2, 13))

    def test_a_directory_picks_the_largest_record(self):
        (self.directory / 'host-a.csv').write_text(HOST_SHARED.read_text())
        (self.directory / 'host-b.csv').write_text((FIXTURES / 'host-nuc.csv').read_text())
        self.assertEqual(pathlib.Path(read_host_record(self.directory)['path']).name, 'host-a.csv')
        empty = self.directory / 'empty'
        empty.mkdir()
        with self.assertRaisesRegex(ValueError, 'no host-'):
            read_host_record(empty)

    def test_foreign_or_incomplete_host_records_are_rejected(self):
        wrong = self.directory / 'host-1.csv'
        wrong.write_text('# something else\nwire_frame\n')
        with self.assertRaisesRegex(ValueError, 'not a "rubylight timing record 1"'):
            read_host_record(wrong)
        wrong.write_text(HOST_SHARED.read_text().replace('claimed_us,', ''))
        with self.assertRaisesRegex(ValueError, 'missing host record columns: claimed_us'):
            read_host_record(wrong)

    def test_a_record_without_its_footer_has_an_unknown_dropped_count(self):
        path = self.directory / 'host-1.csv'
        path.write_bytes(HOST_SHARED.read_bytes().replace(b'# dropped=2\n', b''))
        self.assertIsNone(read_host_record(path)['dropped'])
        result = build_report(SHARED, path)[0]
        self.assertIsNone(result['host_dropped'])
        self.assertFalse(result['host_footer'])
        self.assertIn('host rows dropped by the writer unknown (no footer)', timing_report.format_table(result))
        self.assertTrue(any('no "# dropped=" footer' in note for note in result['notes']))
        complete = build_report(SHARED, HOST_SHARED)[0]
        self.assertTrue(complete['host_footer'])
        self.assertIn('host rows dropped by the writer 2', timing_report.format_table(complete))
        self.assertNotIn('unknown', timing_report.format_table(complete))

    def test_wait_for_footer_returns_once_the_host_has_written_it(self):
        import threading
        path = self.directory / 'host-1.csv'
        path.write_bytes(HOST_SHARED.read_bytes().replace(b'# dropped=2\n', b''))
        self.assertFalse(timing_report.wait_for_footer(self.directory, .3))
        self.assertFalse(timing_report.wait_for_footer(self.directory / 'nowhere', .2))
        timer = threading.Timer(.3, lambda: path.write_bytes(HOST_SHARED.read_bytes()))
        timer.start()
        self.addCleanup(timer.join)
        self.assertTrue(timing_report.wait_for_footer(self.directory, 5))

    def test_empty_host_fields_stay_empty(self):
        path = self.directory / 'host-1.csv'
        path.write_text(HOST_SHARED.read_text().replace('47721859840,47721860640,', ',,'))
        row = read_host_record(path)['rows'][100]
        self.assertIsNone(row['published'])
        self.assertIsNone(row['claimed'])
        result = build_report(SHARED, path)[0]
        self.assertEqual(result['stages']['hold_claim']['n'], 10)
        self.assertEqual(result['stages']['hold_claim_convert']['n'], 1)
        self.assertEqual(result['stages']['encode']['n'], 11)

    def test_unknown_client_files_and_bad_cells_are_errors(self):
        path = self.directory / 'x.csv'
        path.write_text('a,b\n1,2\n')
        with self.assertRaisesRegex(ValueError, 'neither a Nightjar latency CSV'):
            build_report(path)
        path.write_text('wire_frame,first_packet_us\n1,2\n')
        with self.assertRaisesRegex(ValueError, 'missing columns: assembled_us'):
            build_report(path)
        lines = SHARED.read_text().splitlines()
        cells = lines[1].split(',')
        cells[7] = 'x'
        lines[1] = ','.join(cells)
        path.write_bytes(('\n'.join(lines) + '\n').encode())
        with self.assertRaisesRegex(ValueError, 'line 2: column .*not a number'):
            build_report(path)

    def test_a_forced_join_without_host_data_is_an_error(self):
        with self.assertRaisesRegex(ValueError, 'host data'):
            report('nightjar-v1.csv', join='min_owd')

    def test_json_and_stdout(self):
        target = self.directory / 'report.json'
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            code = main([str(FIXTURES / 'frames-nuc.csv'), '--host-record', str(FIXTURES / 'host-nuc.csv'),
                         '--json', str(target)])
        self.assertEqual(code, 0)
        saved = json.loads(target.read_text())
        self.assertEqual(saved['join'], 'min_owd')
        self.assertEqual(saved['stages']['encode']['median_ms'], 2.0)
        self.assertIn('join: min_owd', output.getvalue())

    def test_errors_exit_2_without_a_traceback(self):
        errors = io.StringIO()
        with contextlib.redirect_stderr(errors):
            self.assertEqual(main([str(self.directory / 'missing.csv')]), 2)
        self.assertIn('timing_report:', errors.getvalue())

    def test_case_report_reads_an_e2e_record_case(self):
        shutil.copy(SHARED, self.directory / 'frames.csv')
        (self.directory / 'record').mkdir()
        shutil.copy(HOST_SHARED, self.directory / 'record' / 'host-e2e.csv')
        result = case_report(self.directory)
        self.assertEqual(result['join'], 'shared_clock')
        self.assertEqual(result['stages']['encode']['n'], 11)

    def test_a_streamed_frame_reports_its_overlap_and_the_tail_after_the_encoder(self):
        # Frames 100 and 101 left their first block 700 us after submit, long before the last unit; 102 is streamed but
        # nothing left early (its first packet followed its last unit); 103 is a whole frame.
        record = read_host_record(FIXTURES / 'host-streamed.csv')
        self.assertEqual((record['rows'][100]['first_unit'], record['rows'][100]['early_blocks']), (47721861190, 1))
        self.assertEqual((record['rows'][103]['first_unit'], record['rows'][103]['early_blocks']), (None, None))
        frames = timing_report.read_client(SHARED)['frames']
        timing_report.attach_host(frames, record)
        by_frame = {f['frame_number']: timing_report.stage_values(f, True) for f in frames}
        got = lambda n: {k: by_frame[n][k] for k in ('encode', 'packetize_pace', 'overlap', 'send')}
        self.assertEqual(got(100), dict(encode=1800, packetize_pace=400, overlap=1100, send=1300))
        self.assertEqual(got(101), dict(encode=2000, packetize_pace=400, overlap=1300, send=1600))
        # Nothing left early: no overlap, and the whole wait for the first packet is packetize_pace.
        self.assertEqual(got(102), dict(encode=2200, packetize_pace=2300, overlap=0, send=900))
        # A whole frame is as before, with no overlap.
        self.assertEqual(got(103), dict(encode=1800, packetize_pace=400, overlap=None, send=1200))
        self.assertEqual(by_frame[100]['host_total'], 4490)

    def test_the_overlap_stage_is_in_the_report_only_for_streamed_frames(self):
        streamed = report('frames-shared.csv', 'host-streamed.csv')
        self.assertEqual(streamed['join'], 'shared_clock')
        self.assertEqual(streamed['stages']['overlap']['n'], 3)
        self.assertEqual(streamed['stages']['overlap']['median_ms'], 1.1)
        self.assertEqual(streamed['stages']['overlap']['negative'], 0)
        self.assertEqual(streamed['stages']['encode']['n'], 11)
        self.assertIn('overlap', timing_report.format_table(streamed))
        # A record from before the columns reports none, and the table leaves the row out.
        whole = report('frames-shared.csv', 'host-shared.csv')
        self.assertEqual(whole['stages']['overlap']['n'], 0)
        self.assertNotIn('overlap', timing_report.format_table(whole))
        self.assertEqual(whole['stages']['packetize_pace']['median_ms'], .4)

    def test_fixtures_keep_lf_endings(self):
        for path in FIXTURES.glob('*.csv'):
            self.assertNotIn(b'\r', path.read_bytes(), path.name)


if __name__ == '__main__':
    unittest.main()
