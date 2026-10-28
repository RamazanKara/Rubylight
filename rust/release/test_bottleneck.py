import csv, json, pathlib, tempfile, unittest
from bottleneck_metrics import (OVERSIZE_LINE, cut_metrics, host_environment, idr_probe_environment, latch_flags,
                                nominal_frame_bytes, pacing_events, route_kinds, timing_values)

FIELDS = ['wire_frame', 'arrival_ms', 'host_ms', 'decode_ms', 'render_frame', 'render_qpc', 'picture_age_ms',
          'first_packet_us', 'assembled_us', 'presentation_us', 'frame_type', 'bytes']


def write(path, frames):
    """frames: (wire_frame, arrival_ms, picture_age_ms, frame_type, bytes)."""
    with open(path, 'w', newline='') as f:
        out = csv.DictWriter(f, FIELDS)
        out.writeheader()
        for frame, at, age, kind, size in frames:
            out.writerow(dict(wire_frame=frame, arrival_ms=at, picture_age_ms=age, frame_type=kind, bytes=size))
    with open(path, newline='') as f:
        return list(csv.DictReader(f))


class CutMetrics(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.path = pathlib.Path(self.dir.name) / 'frames.csv'

    def tearDown(self):
        self.dir.cleanup()

    def test_nominal_frame_is_bitrate_over_frame_rate(self):
        self.assertEqual(nominal_frame_bytes(40000, 50), 100000)

    def test_cuts_stalls_and_ordinary_age(self):
        # 10 ms frames of 600 bytes (nominal 1000) aged 10 ms. A whole cut of
        # 5000 bytes at frame 10 and its two followers are slower; frames
        # 30-41 are lost (a 130 ms stall) and frame 42 is the recovery keyframe.
        frames = []
        for frame in range(1, 81):
            if 30 <= frame <= 41:
                continue
            size, age, kind = 600, 10, 0
            if frame == 10:
                size, age = 5000, 40
            elif frame in (11, 12):
                age = 30
            elif frame == 42:
                size, age, kind = 4000, 50, 1
            frames.append((frame, frame * 10, age, kind, size))
        rows = write(self.path, frames)
        metrics = cut_metrics(self.path, rows, 1000)
        self.assertEqual(metrics['stalls'], 1)
        self.assertEqual(metrics['stalls_100ms'], 1)
        # The whole cut and the recovery keyframe are both over 2 budgets.
        self.assertEqual(metrics['cuts_received'], 2)
        self.assertEqual(metrics['cut_bytes_median'], 4000)
        self.assertEqual(metrics['ordinary_bytes_median'], 600)
        self.assertEqual(metrics['ordinary_picture_age_p99_ms'], 10)
        self.assertEqual(metrics['ordinary_picture_age_after_first_cut_p50_ms'], 10)
        self.assertEqual(metrics['picture_age_p99_ms'], 40)

    def test_short_gaps_and_unread_ages_are_not_stalls(self):
        # A missing frame inside two periods is no stall; an unread age (-1) is skipped.
        frames = [(f, f * 10, -1 if f == 5 else 12, 0, 500) for f in range(1, 21) if f != 8]
        rows = write(self.path, frames)
        metrics = cut_metrics(self.path, rows, 1000)
        self.assertEqual(metrics['stalls'], 0)
        self.assertEqual(metrics['cuts_received'], 0)
        self.assertIsNone(metrics['cut_bytes_median'])
        self.assertEqual(metrics['picture_age_p50_ms'], 12)
        self.assertIsNone(metrics['ordinary_picture_age_after_first_cut_p50_ms'])


class LatchFlags(unittest.TestCase):
    def test_oversize_arm_and_full_latch_are_told_apart(self):
        armed = (f'2026-10-28T10:00:00Z  WARN butterpollo_core::session: {OVERSIZE_LINE} after a lost 812 KB frame 240\n'
                 '2026-10-28T10:00:00Z  INFO butterpollo_core::session: oversize pacing mode=frames\n')
        self.assertEqual(latch_flags(armed)['oversize_armed'], True)
        self.assertEqual(latch_flags(armed)['full_latch'], False)
        self.assertEqual(latch_flags(armed)['oversize_mode'], 'frames')
        full = ('WARN more video loss after the start-up keyframe; pacing video like a Wi-Fi route for the rest of the stream\n'
                'INFO video recovery frame payload_bytes=900000\nINFO link_rate lowered the bitrate\n')
        flags = latch_flags(full)
        self.assertEqual((flags['oversize_armed'], flags['full_latch'], flags['oversize_mode']), (False, True, None))
        self.assertEqual((flags['recovery_frames'], flags['link_rate_changes']), (1, 1))


SESSION = 'butterpollo_core::session'
PACING_LOG = '\n'.join((
    '2026-10-29T10:00:00.000Z  INFO butterpollo::stream: network pacing selected rate_mbps=1000',
    f'2026-10-29T10:00:01.000Z  INFO {SESSION}: video loss evidence not counted stream_id=1 '
    'why="repeated keyframe request 120 ms after the first from a client that reports its own losses: its decoder asked again"',
    f'2026-10-29T10:00:02.500Z  WARN {SESSION}: the host\'s socket refused video packets; pacing video like a Wi-Fi '
    'route until it has been clean for 10 s stream_id=1 reason="three recent video frames were lost on send" detail=""',
    f'2026-10-29T10:00:03.000Z  INFO butterpollo::video_send: the video socket was full; frame 1001 waited 9 ms '
    'for room instead of dropping packets',
    '2026-10-29T10:00:03.100Z  INFO butterpollo::stream: video recovery frame recovery_kind=keyframe frame=1002 '
    'payload_bytes=900000 cap_bytes=600000 cap_target_bytes=500000 reencoded=true fec_limited=false lost_on_send=false',
    f'2026-10-29T10:00:05.000Z  INFO butterpollo::stream: stream timings encode_mean_ms=2.5 reencoded_cuts=1 '
    'keyframe_cap_scale=0.75',
    f'2026-10-29T10:00:10.000Z  INFO butterpollo::stream: stream timings encode_mean_ms=3.5 reencoded_cuts=2 '
    'keyframe_cap_scale=0.9',
    f'2026-10-29T10:00:14.000Z  INFO {SESSION}: returning to the route\'s pacing rate: the loss did not recur '
    'why="only the host\'s socket refused video, nothing since for 10 s"',
    f'2026-10-29T10:00:20.000Z  WARN {SESSION}: the client reported video loss after the host\'s send loss; pacing '
    'video like a Wi-Fi route for the rest of the stream reason="video frames were reported lost" detail=""',
))


class PacingEvents(unittest.TestCase):
    def test_latch_kinds_lifts_and_ignored_requests(self):
        events = pacing_events(PACING_LOG)
        self.assertEqual([(latch['at_s'], latch['kind']) for latch in events['latches']],
                         [(2.5, 'send'), (20.0, 'confirmed_send')])
        self.assertEqual(events['latches'][0]['reason'], 'three recent video frames were lost on send')
        self.assertEqual(events['lifts'], [14.0])
        self.assertEqual((events['ignored_evidence'], events['ignored_repeated_requests']), (1, 1))
        self.assertEqual((events['held_frame_logs'], events['remembered_at_start']), (1, 0))
        self.assertEqual(events['host_encode_mean_ms'], 3.0)

    def test_recovery_keyframes_against_their_cap(self):
        events = pacing_events(PACING_LOG)
        self.assertEqual(events['recovery_keyframes'], [dict(
            frame=1002, payload_bytes=900000, cap_bytes=600000, cap_target_bytes=500000,
            reencoded=True, fec_limited=False, lost_on_send=False)])
        self.assertEqual((events['keyframes_over_target'], events['keyframes_fec_limited'],
                          events['keyframes_lost_on_send']), (1, 0, 0))

    def test_timing_values_keep_each_line(self):
        values = timing_values(PACING_LOG, ('reencoded_cuts', 'keyframe_cap_scale', 'keyframes_past_fec'))
        self.assertEqual(values, dict(reencoded_cuts=[1, 2], keyframe_cap_scale=[0.75, 0.9], keyframes_past_fec=[]))

    def test_route_kinds_from_the_paired_state(self):
        with tempfile.TemporaryDirectory() as folder:
            state = pathlib.Path(folder) / 'sunshine_state.json'
            self.assertEqual(route_kinds(state), [])
            state.write_text(json.dumps({'root': {'keyframe_loss_routes': [{'kind': 'reported'}, {'kind': 'sustained'}]}}))
            self.assertEqual(route_kinds(state), ['reported', 'sustained'])


class Environments(unittest.TestCase):
    def test_idr_probe(self):
        self.assertEqual(idr_probe_environment(None), {})
        self.assertEqual(idr_probe_environment('2:100'), {'BUTTERPOLLO_TEST_IDR_PROBE': '2',
                                                          'BUTTERPOLLO_TEST_IDR_PROBE_INTERVAL_MS': '100'})
        self.assertEqual(idr_probe_environment('3'), {'BUTTERPOLLO_TEST_IDR_PROBE': '3'})
        for bad in ('0:100', 'x:100', '2:50', '2:20000', '2:ms'):
            with self.assertRaises(ValueError, msg=bad):
                idr_probe_environment(bad)

    def test_send_outage(self):
        self.assertEqual(host_environment(None), {})
        self.assertEqual(host_environment('3000:20:stall'), {'BUTTERPOLLO_TEST_SEND_OUTAGE': '3000:20:stall'})


if __name__ == '__main__':
    unittest.main()
