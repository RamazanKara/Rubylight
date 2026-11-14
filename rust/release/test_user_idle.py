"""user_idle.py: input that Windows reports from inside a test's time is not the user's."""
import os, pathlib, subprocess, sys, tempfile, unittest

HERE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import user_idle  # noqa: E402


class UserLastInput(unittest.TestCase):
    def test_input_outside_every_mark_is_the_users(self):
        marks = [dict(start=100, end=160, before=40)]
        self.assertEqual(user_idle.user_last_input(90, marks), 90)
        self.assertEqual(user_idle.user_last_input(100, marks), 100, 'the input a test began at is the user\'s')
        self.assertEqual(user_idle.user_last_input(160 + user_idle.MARGIN + 1, marks), 160 + user_idle.MARGIN + 1)

    def test_input_inside_a_mark_is_the_tests(self):
        marks = [dict(start=100, end=160, before=40)]
        self.assertEqual(user_idle.user_last_input(101, marks), 40)
        self.assertEqual(user_idle.user_last_input(160 + user_idle.MARGIN, marks), 40, 'a host leaving unplugs its devices')

    def test_a_chain_of_tests_leads_back_to_the_user(self):
        # The second test's `before` was itself input from the first.
        marks = [dict(start=100, end=160, before=40), dict(start=200, end=260, before=130)]
        self.assertEqual(user_idle.user_last_input(230, marks), 40)

    def test_a_test_inside_a_test(self):
        marks = [dict(start=110, end=150, before=40), dict(start=100, end=160, before=40)]
        self.assertEqual(user_idle.user_last_input(120, marks), 40)

    def test_marks_that_explain_each_other_end(self):
        marks = [dict(start=100, end=160, before=150), dict(start=140, end=200, before=120)]
        self.assertEqual(user_idle.user_last_input(150, marks), 100)

    def test_a_mark_after_the_fact_counts_the_user_present_until_it_began(self):
        self.assertEqual(user_idle.user_last_input(130, [dict(start=100, end=160, before=100)]), 100)


class State(unittest.TestCase):
    def setUp(self):
        self.folder = tempfile.TemporaryDirectory()
        self.addCleanup(self.folder.cleanup)
        self.path = pathlib.Path(self.folder.name) / 'locks' / 'USER-IDLE.json'

    def test_a_stream_between_two_checks_leaves_the_user_away(self):
        # The user left at 1000. A test from 2000 to 2060 had its host reset the timer at 2010.
        self.assertEqual(user_idle.idle(self.path, now=1900, last=1000), (900, 900))
        mark = user_idle.begin(self.path, now=2000, last=1000)
        user_idle.end(mark, self.path, now=2060)
        away, reported = user_idle.idle(self.path, now=2100, last=2010)
        self.assertEqual((away, reported), (1100, 90))
        self.assertIn('Windows says 1.5 min', user_idle.describe(away, reported))
        # The next test begins from the user's time, not from the first test's reset.
        self.assertEqual(user_idle.begin(self.path, now=2200, last=2010)['before'], 1000)

    def test_the_user_coming_back_after_a_test_is_seen(self):
        user_idle.end(user_idle.begin(self.path, now=2000, last=1000), self.path, now=2060)
        away, reported = user_idle.idle(self.path, now=2100, last=2090)
        self.assertEqual((away, reported), (10, 10))
        self.assertNotIn('Windows says', user_idle.describe(away, reported))

    def test_old_marks_are_dropped_and_a_damaged_file_is_no_marks(self):
        user_idle.end(dict(start=10, before=5), self.path, now=20)
        user_idle.end(dict(start=30 + user_idle.KEEP, before=5), self.path, now=40 + user_idle.KEEP)
        self.assertEqual([m['start'] for m in user_idle.load(self.path)], [30 + user_idle.KEEP])
        for damaged in ('not json', '{"start": 1}', '[1, {"start": "x", "end": 2, "before": 0}]'):
            self.path.write_text(damaged, encoding='utf-8')
            self.assertEqual(user_idle.load(self.path), [])

    @unittest.skipUnless(sys.platform == 'win32', 'GetLastInputInfo')
    def test_the_command_line_marks_a_time(self):
        try:
            user_idle.system_last_input()
        except OSError:
            self.skipTest('no input desktop in this session')
        env = dict(os.environ, RUBYLIGHT_LOCK_DIRECTORY=str(self.path.parent))

        def call(*arguments):
            return subprocess.run([sys.executable, str(HERE / 'user_idle.py'), *arguments], env=env,
                                  capture_output=True, text=True, check=True).stdout.strip()
        self.assertRegex(call(), r'^no input for \d+\.\d min')
        start, before = (float(value) for value in call('begin').split(' '))
        self.assertLessEqual(before, start)
        call('end', f'{start}', f'{before}')
        call('end', f'{start}')
        self.assertEqual([(m['start'], m['before']) for m in user_idle.load(self.path)],
                         [(start, before), (start, start)])


if __name__ == '__main__':
    unittest.main()
