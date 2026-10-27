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

    def test_client_choices_reach_the_container_and_the_result(self):
        calls = []
        nuc = lambda *command, **kw: None
        popen = lambda command, **kw: calls.append(command) or 'client'
        spec = ls.arms()['wifi']
        env = {'LINKSPEED_CLIENT_FEC': 'auto', 'LINKSPEED_CLIENT_PROBE': '1'}
        with mock.patch.dict(ls.os.environ, env), mock.patch.object(ls, 'nuc', nuc), \
                mock.patch.object(ls.subprocess, 'Popen', popen):
            ls.start_client(spec)
        self.assertIn("docker exec -e LINKSPEED_CLIENT_FEC=auto -e LINKSPEED_CLIENT_PROBE=1 ljs python3", calls[0][-1])
        with mock.patch.dict(ls.os.environ, {'LINKSPEED_CLIENT_FEC': '', 'LINKSPEED_CLIENT_PROBE': ''}):
            self.assertEqual(ls.client_env(), [])
        with tempfile.TemporaryDirectory() as directory:
            out = pathlib.Path(directory)
            (out / 'spec.json').write_text(json.dumps(dict(spec, steps_issued=[], client_choices=env)))
            (out / 'probe.json').write_text(json.dumps({'mbps': 140.0, 'first_kbps': 20000}))
            result = ls.analyse(out)
            self.assertEqual(result['client_choices'], env)
            self.assertEqual(result['probe']['first_kbps'], 20000)


class LinkSpeedClient(unittest.TestCase):
    """What the NUC client asks for must match Nightjar: rlFecPercentage values and the probe's first bitrate."""

    def setUp(self):
        import linkspeed_client
        self.client = linkspeed_client

    def test_fec_choices(self):
        for value in ('host', 'auto', '0', '20', '255'):
            self.assertEqual(self.client.fec_choice(value), value)
        for value in (None, ''):
            self.assertIsNone(self.client.fec_choice(value))
        for value in ('256', '-1', 'Auto', '1e2', ' 5', '0020'):
            with self.assertRaises(SystemExit):
                self.client.fec_choice(value)

    def test_first_bitrate_is_a_share_of_the_probe_never_above_the_setting(self):
        self.assertEqual(self.client.first_bitrate_kbps(20000, 500.0), 20000)
        self.assertEqual(self.client.first_bitrate_kbps(75000, 50.0), 40000)
        self.assertEqual(self.client.first_bitrate_kbps(20000, 1.0), 2000)

    def test_probe_counts_a_body_the_host_cut_short(self):
        class Response:
            def __enter__(self): return self
            def __exit__(self, *a): return False
            def raise_for_status(self): pass
            def iter_content(self, size): return [b'x' * 65536] * 8
        session = mock.Mock()
        session.get.return_value = Response()
        measured = self.client.probe(session, 'https://h:1')
        self.assertEqual(measured['bytes'], 8 * 65536)
        self.assertGreater(measured['mbps'], 0)
        session.get.assert_called_once()
        self.assertEqual(session.get.call_args.kwargs['params'], {'bytes': 4 * 1024 * 1024})
        session.get.return_value.iter_content = lambda size: []
        with self.assertRaisesRegex(RuntimeError, 'returned 0 bytes'):
            self.client.probe(session, 'https://h:1')


if __name__ == '__main__':
    unittest.main()
