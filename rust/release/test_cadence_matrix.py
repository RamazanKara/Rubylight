import contextlib, io, json, pathlib, tempfile, unittest
import cadence_matrix as cm


class Cases(unittest.TestCase):
    def test_every_case_name_is_unique_and_names_its_parts(self):
        table = cm.cases()
        self.assertEqual(len(table), 4 * 3 * 2 + 2 * 2 * 2)
        spec = table['hevc-144-vrr-jitter']
        self.assertEqual((spec['codec'], spec['fps'], spec['vrr'], spec['content']), ('hevc', 144, True, 'jitter'))
        self.assertIn('pyrowave-90-fixed90', table)
        self.assertEqual({c['group'] for c in table.values()}, {'core', 'codecs'})

    def test_the_content_scripts_are_motion_probes_forms(self):
        self.assertEqual(cm.CONTENT['hitch90'], 'hitch:90:100')
        self.assertEqual(cm.CONTENT['jitter'], 'jitter:8,14,9,13')

    def test_list_prints_the_cases(self):
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            self.assertEqual(cm.main(['list']), 0)
        self.assertIn('hevc-120-hitch90', output.getvalue())


class StampCheck(unittest.TestCase):
    def test_a_host_stamp_that_is_the_probes_present_matches_with_a_small_lag(self):
        probe = [i * 11_111 for i in range(100)]
        host = [t + 3000 for t in probe]
        result = cm.stamp_check(probe, host)
        self.assertEqual((result['matched'], result['gap_pairs']), (100, 99))
        self.assertEqual((result['lag_p50_ms'], result['lag_p99_ms']), (3.0, 3.0))
        self.assertEqual(result['gap_error_p99_ms'], 0)

    def test_a_stamp_that_follows_another_cadence_shows_in_the_gap_error(self):
        probe = [i * 11_111 for i in range(100)]
        host = [t + (3000 if i % 2 else 1000) for i, t in enumerate(probe)]
        result = cm.stamp_check(probe, host)
        self.assertEqual(result['gap_error_p50_ms'], 2.0)

    def test_the_wire_is_judged_against_the_game_not_the_stamp(self):
        probe = [i * 11_111 for i in range(100)]
        # The stamp is quantized to a 4 ms grid; the last packet follows the game's cadence 6 ms later.
        host = [t // 4000 * 4000 + 4000 for t in probe]
        sent = [t + 6000 + (500 if i % 2 else 0) for i, t in enumerate(probe)]
        result = cm.stamp_check(probe, host, sent_us=sent)
        self.assertGreater(result['gap_error_p99_ms'], 3.0)
        self.assertEqual((result['wire_error_p50_ms'], result['wire_lag_p50_ms']), (.5, 6.0))
        self.assertIsNone(cm.stamp_check(probe, host)['wire_error_p50_ms'])

    def test_stamps_far_from_any_present_do_not_match(self):
        result = cm.stamp_check([0, 11_111], [500_000])
        self.assertEqual(result['matched'], 0)
        self.assertIsNone(result['lag_p50_ms'])

    def test_one_present_is_matched_once(self):
        # Two stamps after one present: a repeated frame, the second is not a new present.
        result = cm.stamp_check([0, 50_000], [1000, 2000, 51_000])
        self.assertEqual(result['matched'], 2)

    def test_a_case_directory_is_read_against_its_host_record(self):
        directory = pathlib.Path(tempfile.mkdtemp())
        self.addCleanup(__import__('shutil').rmtree, directory, True)
        epoch_ms, frequency = 1_000_000.0, 10_000_000
        frames = [dict(frame=i + 1, qpc=round((epoch_ms * 1000 + 100_000 + i * 11_111) * frequency / 1e6)) for i in range(20)]
        (directory / 'motion.json').write_text(json.dumps(dict(qpc_frequency=frequency, frames=frames)))
        (directory / 'record').mkdir()
        header = ('# rubylight timing record 1\n# media_epoch_qpc_ms=%s\n# codec=hevc capture=wgc present_source=etw\n'
                  'wire_frame,rtp_us,present_us,acquired_us,published_us,claimed_us,submitted_us,encoded_us,packetized_us,'
                  'prepared_us,first_sent_us,last_sent_us\n' % epoch_ms)
        rows = ''.join(f'{i},0,{100_000 + i * 11_111 + 2500},,,,,,,,,{100_000 + i * 11_111 + 7000}\n' for i in range(20))
        (directory / 'record' / 'host-1.csv').write_text(header + rows + '# dropped=0\n')
        result = cm.case_stamp_check(directory)
        self.assertEqual(result['matched'], 20)
        self.assertAlmostEqual(result['lag_p50_ms'], 2.5, places=2)
        self.assertLess(result['gap_error_p99_ms'], .01)
        self.assertAlmostEqual(result['wire_lag_p50_ms'], 7.0, places=2)


class Report(unittest.TestCase):
    ROW = dict(name='hevc-90-fixed90', present_interval=dict(n=10, median_ms=11.11, p95_ms=11.2, p99_ms=11.9),
               host_cadence_error=dict(n=9, median_ms=.1, p95_ms=.3, p99_ms=.4),
               arrival_cadence_error=dict(n=0, median_ms=None, p95_ms=None, p99_ms=None),
               stamp=dict(lag_p50_ms=3.0, lag_p99_ms=4.0, gap_error_p50_ms=.2, gap_error_p99_ms=.9))

    def test_the_table_has_a_row_per_case_and_dashes_for_missing_stages(self):
        lines = cm.table_lines([self.ROW], markdown=True)
        self.assertEqual(len(lines), 3)
        self.assertIn('| hevc-90-fixed90 | 11.11 / 11.90 | 0.10 / 0.40 | - | 9 | 3.00 / 4.00 | 0.20 / 0.90 |', lines[2])

    def test_the_latest_row_of_a_case_wins(self):
        directory = pathlib.Path(tempfile.mkdtemp())
        self.addCleanup(__import__('shutil').rmtree, directory, True)
        old = dict(self.ROW, passed=False)
        (directory / 'results.jsonl').write_text(json.dumps(old) + '\n' + json.dumps(dict(self.ROW, passed=True)) + '\n')
        rows = cm.read_rows(directory)
        self.assertEqual(len(rows), 1)
        self.assertTrue(rows[0]['passed'])


if __name__ == '__main__':
    unittest.main()
