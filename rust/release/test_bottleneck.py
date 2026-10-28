import csv, pathlib, tempfile, unittest
from bottleneck_metrics import OVERSIZE_LINE, cut_metrics, latch_flags, nominal_frame_bytes

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


if __name__ == '__main__':
    unittest.main()
