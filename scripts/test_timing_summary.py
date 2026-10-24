import os
import tempfile
import unittest

import timing_summary as ts


def leg(path, values, fps=60.0, receive_to_present=None, step_ms=10):
    """A CSV with one row per `step_ms`, input_to_output_ms from `values`."""
    with open(path, "w", newline="") as handle:
        handle.write("decoder_output_ns,input_to_output_ms,receive_to_present_ms,shown_fps\n")
        for i, value in enumerate(values):
            handle.write(f"{i * step_ms * 1_000_000},{value},{receive_to_present or value + 10},{fps}\n")


class PercentileTests(unittest.TestCase):
    def test_nearest_rank_matches_the_rust_rule_including_half_up(self):
        values = list(range(1, 102))
        self.assertEqual([ts.percentile(values, p) for p in (50, 95, 99)], [51, 96, 100])
        self.assertEqual(ts.percentile([1500, 2500], 50), 2500)  # round half up, not banker's
        self.assertEqual(ts.percentile([7], 95), 7)
        self.assertTrue(ts.percentile([], 50) != ts.percentile([], 50))  # nan


class LegTests(unittest.TestCase):
    def test_shown_fps_comes_from_the_render_stamps_when_the_column_is_empty(self):
        first = [{"render_ns": str(int(i * 1e9 / 60))} for i in range(61)]
        second = [{"render_ns": str(int(500e9 + i * 1e9 / 50))} for i in range(51)]
        self.assertAlmostEqual(ts.summarize(first)["shown_fps"], 60.0, places=1)
        # A stall counts: 61 frames in 1 s, then a 1 s gap and 60 more in the next second.
        stalled = first + [{"render_ns": str(int(2e9 + i * 1e9 / 60))} for i in range(61)]
        self.assertAlmostEqual(ts.summarize(stalled)["shown_fps"], 121 / 3.0, delta=0.5)
        # Legs from separate processes have separate clocks: pooled rates average, not add.
        self.assertAlmostEqual(ts.pooled([first, first])["shown_fps"], 60.0, places=1)
        self.assertAlmostEqual(ts.pooled([first, second])["shown_fps"], 55.0, delta=0.1)
        # A leg without the column still counts when another has it.
        column = [{"shown_fps": "50", "render_ns": "0"}] * 3
        self.assertAlmostEqual(ts.pooled([first, column])["shown_fps"], 55.0, delta=0.1)
        self.assertEqual(ts.summarize([{"shown_fps": "59"}, {"shown_fps": "61"}, {"shown_fps": "60"}])["shown_fps"], 60)

    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.addCleanup(self.dir.cleanup)

    def path(self, name):
        return os.path.join(self.dir.name, name)

    def test_the_first_seconds_are_discarded_by_decoder_output_time(self):
        leg(self.path("a.csv"), [100] * 500 + [10] * 500)  # 10 ms apart: 500 rows = 5 s
        rows = ts.load_leg(self.path("a.csv"), 5.0)
        self.assertEqual(len(rows), 500)
        self.assertEqual(ts.summarize(rows)["input_to_output_ms"][50], 10)

    def test_a_leg_without_a_timestamp_column_value_is_empty(self):
        with open(self.path("e.csv"), "w") as handle:
            handle.write("decoder_output_ns,input_to_output_ms\n")
        self.assertEqual(ts.load_leg(self.path("e.csv")), [])

    def test_keep_rule_keeps_a_clear_win_and_reverts_each_kind_of_regression(self):
        a = [ts.load_leg(self.write("a1", [10.0] * 1000), 0), ts.load_leg(self.write("a2", [10.2] * 1000), 0)]
        better = [ts.load_leg(self.write("b1", [9.0] * 1000), 0), ts.load_leg(self.write("b2", [9.1] * 1000), 0)]
        self.assertEqual(ts.keep_rule(a, better), (True, []))
        one_leg_worse = [better[0], ts.load_leg(self.write("b3", [10.1] * 1000), 0)]
        keep, reasons = ts.keep_rule(a, one_leg_worse)
        self.assertFalse(keep)
        self.assertIn("p50", reasons[0])
        tail = [9.0] * 900 + [30.0] * 100  # better median, worse tail
        keep, reasons = ts.keep_rule(a, [ts.load_leg(self.write("t1", tail), 0), ts.load_leg(self.write("t2", tail), 0)])
        self.assertFalse(keep)
        self.assertTrue(any("pooled p95" in r for r in reasons))
        slow_present = [ts.load_leg(self.write("s1", [9.0] * 1000, receive_to_present=30), 0)] * 2
        keep, reasons = ts.keep_rule(a, slow_present)
        self.assertFalse(keep)
        self.assertTrue(any("receive_to_present_ms" in r for r in reasons))
        slow = [ts.load_leg(self.write("f1", [9.0] * 1000, fps=58.0), 0)] * 2
        keep, reasons = ts.keep_rule(a, slow)
        self.assertFalse(keep)
        self.assertTrue(any("shown fps" in r for r in reasons))

    def write(self, name, values, fps=60.0, receive_to_present=None):
        path = self.path(name + ".csv")
        leg(path, values, fps, receive_to_present)
        return path

    def test_main_prints_the_verdict_and_returns_one_on_revert(self):
        for name, value in (("a1", 10.0), ("b1", 11.0), ("b2", 11.0), ("a2", 10.0)):
            self.write(name, [value] * 600)
        legs = [f"{n[0].upper()}:133:{self.path(n)}.csv" for n in ("a1", "b1", "b2", "a2")]
        import contextlib
        import io
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            code = ts.main(["--discard", "0", "--keep"] + legs)
        self.assertEqual(code, 1)
        self.assertIn("133: REVERT", out.getvalue())
        self.assertIn("pooled over 2 legs", out.getvalue())

    def test_presentmon_modes_and_race_verdicts_are_counted_next_to_a_leg(self):
        self.write("c1", [10.0] * 50)
        base = self.path("c1")
        self.assertEqual(ts.present_modes(base + ".csv"), [])  # no capture, no line
        with open(base + ".presentmon.csv", "w", newline="") as handle:
            handle.write("Application,PresentMode,FrameTime\n")
            for mode in ["Hardware: Independent Flip"] * 3 + ["Composed: Flip"]:
                handle.write(f"nightjar.exe,{mode},4.1\n")
        self.assertEqual(
            ts.present_modes(base + ".csv"),
            ["  PresentMon present modes: Hardware: Independent Flip 3 (75%), Composed: Flip 1 (25%)"],
        )
        with open(base + ".presentmon.csv", "w", newline="") as handle:
            handle.write("Application,PresentMode\n")
        self.assertEqual(ts.present_modes(base + ".csv"), ["  PresentMon present modes: no presents captured"])
        rows = [{"race_reason": "on"}] * 2 + [{"race_reason": ""}, {"race_reason": "no VRR scan restart"}]
        self.assertEqual(ts.race_reasons(rows), ["  beam race verdicts: on 2 (67%), no VRR scan restart 1 (33%)"])
        self.assertEqual(ts.race_reasons([{"race_reason": ""}, {}]), [])
        # main prints both under the leg.
        import contextlib
        import io
        with open(base + ".presentmon.csv", "w", newline="") as handle:
            handle.write("Application,PresentMode\n" + "nightjar.exe,Hardware: Independent Flip\n" * 2)
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            ts.main(["--discard", "0", f"A:60:{base}.csv"])
        self.assertIn("PresentMon present modes: Hardware: Independent Flip 2 (100%)", out.getvalue())

    def test_a_bad_leg_argument_is_an_error(self):
        with self.assertRaises(ValueError):
            ts.parse_leg("A:only-two-parts")


if __name__ == "__main__":
    unittest.main()
