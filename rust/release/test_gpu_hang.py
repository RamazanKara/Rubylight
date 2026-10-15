import pathlib, tempfile, unittest
from gpu_hang import EXIT_GPU_HANG, Guard, Scanner, hang_result

# Lines from the RX 7900 XT hangs: 2026-10-10 08:20Z (test host),
# 2026-10-14 06:41Z and 2026-10-10 12:00Z (installed host).
QUERY_3000 = ('2026-10-10T08:20:43.020429Z  WARN butterpollo_windows::amf: slow encoder call '
              'call="AMF QueryOutput" elapsed_ms=3000')
CONVERSION_2950 = ('2026-10-10T12:00:12.988691Z  WARN butterpollo_windows::amf: slow encoder call '
                   'call="compute conversion" elapsed_ms=2950')
STALLED = ('2026-10-14T06:41:46.016053Z  WARN butterpollo::stall_watch: stream thread stopped making progress '
           'thread="session" stream_id=f2462b79-5aab-46d4-b08b-baf13ffdb8d7 phase="encoding" stalled_ms=521 '
           'gpu_query_outstanding_ms=507 gpu_query_last_ms=1')
RESUMED = ('2026-10-14T06:41:48.495195Z  WARN butterpollo::stall_watch: stream thread resumed after a stall '
           'thread="session" stream_id=f2462b79-5aab-46d4-b08b-baf13ffdb8d7 phase="encoding" stalled_ms=3000 '
           'gpu_query_outstanding_ms=2987 gpu_query_last_ms=1')
# Seen in healthy streams.
CONVERSION_201 = ('2026-10-11T15:40:02.330024Z  WARN butterpollo_windows::amf: slow encoder call '
                  'call="compute conversion" elapsed_ms=201')
CAPTURE_STALL = ('2026-10-14T06:41:57.042776Z  WARN butterpollo::stall_watch: stream thread stopped making progress '
                 'thread="capture" stream_id=\\\\.\\DISPLAY16 phase="capturing" stalled_ms=544')
CREATE_STALL = ('2026-10-12T10:00:00.000000Z  WARN butterpollo::stall_watch: stream thread stopped making progress '
                'thread="session" stream_id=e2e phase="creating the encoder" stalled_ms=505 gpu_query_last_ms=1')
# A healthy AV1 1440p120 stream on 2026-10-15 waited 3.6 s for its first frame, then ran without a reset.
FIRST_FRAME_STALL = ('2026-10-15T06:44:32.670519Z  WARN butterpollo::stall_watch: stream thread stopped making progress '
                     'thread="session" stream_id=4631f1af-9d50-4ff5-964b-09e5225a1f34 phase="encoding" stalled_ms=523 '
                     'gpu_query_last_ms=1')
FIRST_FRAME_RESUMED = ('2026-10-15T06:44:35.789306Z  WARN butterpollo::stall_watch: stream thread resumed after a stall '
                       'thread="session" stream_id=4631f1af-9d50-4ff5-964b-09e5225a1f34 phase="encoding" stalled_ms=3639 '
                       'gpu_query_last_ms=1')


class Clock:
    def __init__(self):
        self.now = 100.0

    def __call__(self):
        return self.now


class GpuHangEvidence(unittest.TestCase):
    def test_an_encoder_call_of_a_second_or_more_is_a_hang(self):
        for line in (QUERY_3000, CONVERSION_2950):
            with self.subTest(line=line):
                scanner = Scanner()
                scanner.feed(line, 0)
                self.assertIn('took', scanner.check(0))

    def test_slow_calls_of_healthy_streams_are_not_a_hang(self):
        scanner = Scanner()
        scanner.feed(CONVERSION_201, 0)
        scanner.feed(CONVERSION_201.replace('compute conversion', 'AMF Init').replace('=201', '=1999'), 0)
        self.assertIsNone(scanner.check(60))

    def test_an_encoding_stall_without_resume_is_a_hang_after_the_grace(self):
        scanner = Scanner()
        scanner.feed(STALLED, 10)
        self.assertIsNone(scanner.check(14.4))
        self.assertIn('did not resume', scanner.check(14.5))

    def test_a_resumed_encoding_stall_is_not_a_hang(self):
        scanner = Scanner()
        scanner.feed(STALLED, 10)
        scanner.feed(RESUMED, 12.5)
        self.assertIsNone(scanner.check(30))

    def test_a_slow_first_frame_that_resumes_is_not_a_hang(self):
        scanner = Scanner()
        scanner.feed(FIRST_FRAME_STALL, 32.67)
        self.assertIsNone(scanner.check(35.0))
        scanner.feed(FIRST_FRAME_RESUMED, 35.79)
        self.assertIsNone(scanner.check(60))

    def test_capture_and_encoder_creation_stalls_are_not_encoder_hangs(self):
        scanner = Scanner()
        scanner.feed(CAPTURE_STALL, 0)
        scanner.feed(CREATE_STALL, 0)
        self.assertIsNone(scanner.check(30))

    def test_the_first_evidence_is_kept(self):
        scanner = Scanner()
        scanner.feed(CONVERSION_2950, 0)
        scanner.feed(QUERY_3000, 1)
        self.assertIn('compute conversion', scanner.check(2))


class GpuHangGuard(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.root = pathlib.Path(self.directory.name)
        self.log = self.root / 'butterpollo.log'
        self.reports = self.root / 'LiveKernelReports'
        (self.reports / 'WATCHDOG').mkdir(parents=True)
        (self.reports / 'WHEA').mkdir()
        (self.reports / 'WATCHDOG' / 'WATCHDOG-20261014-0841.dmp').write_bytes(b'old')

    def tearDown(self):
        self.directory.cleanup()

    def test_only_log_lines_written_after_the_start_count(self):
        # The soak's host log keeps earlier cases.
        self.log.write_text(QUERY_3000 + '\n')
        guard = Guard(self.log, self.reports)
        self.assertIsNone(guard.check())
        with self.log.open('a') as log:
            log.write(CONVERSION_2950 + '\n')
        self.assertIn('compute conversion', guard.check())

    def test_a_line_split_across_polls_is_read_once_whole(self):
        guard = Guard(self.log, self.reports)
        self.log.write_text(QUERY_3000[:60])
        self.assertIsNone(guard.check())
        with self.log.open('a') as log:
            log.write(QUERY_3000[60:] + '\n')
        self.assertIn('AMF QueryOutput took 3000 ms', guard.check())

    def test_the_stall_grace_runs_on_the_guards_clock(self):
        clock = Clock()
        guard = Guard(self.log, self.reports, clock)
        self.log.write_text(STALLED + '\n')
        self.assertIsNone(guard.check())
        clock.now += 4.4
        self.assertIsNone(guard.check())
        clock.now += .1
        self.assertIn('did not resume', guard.check())

    def test_a_new_or_rewritten_watchdog_report_is_a_hang(self):
        guard = Guard(self.log, self.reports)
        (self.reports / 'WHEA' / 'WHEA-20261015-0100.dmp').write_bytes(b'cpu')
        self.assertIsNone(guard.check())
        (self.reports / 'WATCHDOG' / 'WATCHDOG-20261015-0101.dmp').write_bytes(b'new')
        self.assertIn('WATCHDOG-20261015-0101.dmp', guard.check())
        rewritten = Guard(self.log, self.reports)
        (self.reports / 'WATCHDOG' / 'WATCHDOG-20261014-0841.dmp').write_bytes(b'rewritten')
        self.assertIn('WATCHDOG-20261014-0841.dmp', rewritten.check())

    def test_an_unreadable_reports_folder_leaves_the_log_checks(self):
        guard = Guard(self.log, self.root / 'missing')
        self.assertIsNone(guard.check())
        self.log.write_text(QUERY_3000 + '\n')
        self.assertIn('took 3000 ms', guard.check())

    def test_the_result_names_the_hang_and_forbids_a_retry(self):
        result = hang_result('hevc-120', 'hevc', '2560x1440x120', False, 'encoder call AMF QueryOutput took 3000 ms')
        self.assertFalse(result['passed'])
        self.assertEqual(result['gpu_hang'], 'encoder call AMF QueryOutput took 3000 ms')
        self.assertIn('Do not retry', result['failures'][0])
        self.assertEqual(EXIT_GPU_HANG, 3)


if __name__ == '__main__':
    unittest.main()
