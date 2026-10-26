import argparse, json, pathlib, subprocess, tempfile, unittest
from unittest import mock
import linkspeed as ls


class LinkSpeedHarness(unittest.TestCase):
    """A client that cannot reach the host must fail its own arm, never pass off an earlier run's files."""

    def test_client_output_is_cleared_before_the_client_starts(self):
        calls = []
        nuc = lambda *command, **kw: calls.append(('nuc', command))
        popen = lambda command, **kw: calls.append(('popen', command)) or 'client'
        spec = ls.arms()['r50-hevc-hi'] | {'name': 'r50-hevc-hi@b'}
        with mock.patch.object(ls, 'nuc', nuc), mock.patch.object(ls.subprocess, 'Popen', popen):
            self.assertEqual(ls.start_client(spec), 'client')
        self.assertEqual(calls[0], ('nuc', ('docker', 'exec', 'ljs', 'rm', '-rf', '--', '/work/r50-hevc-hi@b')))
        self.assertEqual(calls[1][0], 'popen')
        self.assertIn('linkspeed_client.py', calls[1][1][-1])
        self.assertTrue(calls[1][1][-1].endswith(' /work/r50-hevc-hi@b'))

    def test_client_does_not_start_when_the_old_output_cannot_be_removed(self):
        popen = mock.Mock()
        with mock.patch.object(ls, 'nuc', side_effect=RuntimeError('ssh nuc: rm failed')), \
                mock.patch.object(ls.subprocess, 'Popen', popen):
            with self.assertRaisesRegex(RuntimeError, 'rm failed'):
                ls.start_client(ls.arms()['r50-hevc-hi'])
        popen.assert_not_called()

    def test_firewall_rule_failure_asks_for_an_elevated_shell(self):
        host = ls.Host(r'C:\pkg')
        denied = subprocess.CompletedProcess([], 1, '', 'Access is denied.\r\n')
        with mock.patch.object(ls.subprocess, 'run', return_value=denied):
            with self.assertRaisesRegex(RuntimeError, r'elevated shell.*Access is denied\.\)$'):
                host.firewall(True)
            host.firewall(False)  # removing a rule that was never added stays quiet
        with mock.patch.object(ls.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0, '', '')) as run:
            host.firewall(True)
        script = run.call_args.args[0][-1]
        self.assertIn('-ErrorAction Stop', script)
        self.assertIn('exit 1', script)

    def test_elevation_check(self):
        for admin in (0, 1):
            with mock.patch('ctypes.windll', mock.Mock(**{'shell32.IsUserAnAdmin.return_value': admin})):
                if admin:
                    ls.require_elevated()
                else:
                    with self.assertRaisesRegex(RuntimeError, 'needs an elevated shell'):
                        ls.require_elevated()

    def test_run_without_elevation_stops_before_the_lock(self):
        args = argparse.Namespace(arms=['r50-hevc-hi'], package=r'C:\pkg', work=None, wait=60, config=[], content_height=0,
                                  keep_memory=False)
        with tempfile.TemporaryDirectory() as directory:
            args.work = directory
            with mock.patch.object(ls, 'require_elevated', side_effect=RuntimeError('needs an elevated shell')), \
                    mock.patch.object(ls, 'installed_idle') as idle, mock.patch.object(ls, 'take_lock') as lock:
                with self.assertRaisesRegex(RuntimeError, 'elevated shell'):
                    ls.run(args)
        idle.assert_not_called(); lock.assert_not_called()

    def test_missing_frames_carries_the_client_error(self):
        with tempfile.TemporaryDirectory() as directory:
            out = pathlib.Path(directory)
            spec = ls.arms()['r50-hevc-hi']
            (out / 'spec.json').write_text(json.dumps(dict(spec, steps_issued=[])))
            (out / 'runner.log').write_text('Traceback (most recent call last):\n  ...\n'
                                            "requests.exceptions.ConnectTimeout: HTTPSConnectionPool(host='192.168.4.69', port=48318)\n")
            result = ls.analyse(out)
            self.assertTrue(result['error'].startswith('no frames.csv (client: requests.exceptions.ConnectTimeout'), result['error'])
            self.assertEqual(json.loads((out / 'result.json').read_text())['error'], result['error'])
            self.assertEqual(result['client_exit'], 'none')


if __name__ == '__main__':
    unittest.main()
