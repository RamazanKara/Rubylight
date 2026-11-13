import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import timing_breakdown as tb  # noqa: E402


def row(n, ready, due, begin, shown, count, stat_count="", sync=""):
    return {"decoder_output_ns": str(ready), "due_ns": str(due), "present_begin_ns": str(begin), "render_ns": str(shown),
            "present_count": str(count), "stat_present_count": str(stat_count), "stat_sync_ns": str(sync)}


class Breakdown(unittest.TestCase):
    def test_splits_the_wait_and_finds_the_flip_in_a_later_row(self):
        ms = 1_000_000
        rows = [row(1, 0, 7 * ms, 7 * ms + 100_000, 8 * ms, 1),
                row(2, 16 * ms, 24 * ms, 24 * ms, 25 * ms, 2, stat_count=1, sync=12 * ms)]
        s = tb.stages(rows)
        self.assertEqual(s["hold"], [7.0, 8.0])
        self.assertAlmostEqual(s["due->begin"][0], 0.1)
        self.assertEqual(s["call"], [0.9, 1.0])
        self.assertEqual(s["call->flip"], [4.0])
        self.assertEqual(s["decoded->flip"], [12.0])

    def test_cadence_compares_shown_intervals_with_host_stamp_intervals(self):
        ms = 1_000_000
        rows = [{"render_ns": str(shown * ms // 10), "pts_us": str(pts)} for shown, pts in
                [(100, 0), (183, 8_333), (311, 20_833), (395, 29_166), (2000, 200_000), (2083, 190_000)]]
        errors, stamps = tb.cadence(rows)
        # 8.3 shown for 8.333, 12.8 for 12.5, 8.4 for 8.333; the 170.8 ms stall and the stamp
        # stepping back are skipped.
        self.assertEqual([round(s, 3) for s in stamps], [8.333, 12.5, 8.333])
        self.assertEqual([round(e, 3) for e in errors], [0.033, 0.3, 0.067])

    def test_frames_without_the_new_columns_are_skipped(self):
        self.assertEqual(tb.stages([row(1, 0, "", "", 5, 1)])["hold"], [])


if __name__ == "__main__":
    unittest.main()
