import contextlib, io, pathlib, sys, tempfile, textwrap, unittest
from unittest import mock

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent / 'hw'))
import run  # noqa: E402

CASE = '''
  - id: {id}
    device: {device}
    title: t
    command: '{command}'
    preconditions: {pre}
    pass: p
    output: o
'''


def catalog(*cases, variables='vars: {a: x}\n'):
    cases = cases or ({},)
    return variables + 'cases:\n' + ''.join(CASE.format(**dict(dict(id='c', device='desktop', command='exit 0',
                                                                       pre='[]'), **c)) for c in cases)


class Catalog(unittest.TestCase):
    def test_the_catalog_loads_with_the_seeded_cases(self):
        values, cases = run.load(run.CATALOG.read_text(encoding='utf-8'))
        ids = {c.id for c in cases}
        for expected in ('e2e-hevc', 'release-check', 'smoke-laptop', 'soak-short', 'soak-30min', 'browser-e2e',
                         'browser-e2e-off', 'issue56-exclusive-layout', 'sleep-evidence', 'couch-audio-71',
                         'audio-bench', 'encoder-focus-amf-quality', 'encoder-focus-ab', 'couch-menu-stream',
                         'audio-passthrough-tv', 'audio-passthrough-desktop-pcm', 'audio-passthrough-retroid'):
            self.assertIn(expected, ids)
        self.assertIn(r'3.0.0-rc.3\release', run.expand('{{package}}', values, 'test'))
        for case in cases:
            with self.subTest(case=case.id):
                self.assertTrue(case.passes and case.output and case.title)

    def test_vars_nest_and_overrides_win(self):
        text = 'vars: {release: 1.0, package: "C:\\\\r\\\\{{release}}"}\ncases: []\n'
        values, _ = run.load(text, {'release': '2.0'})
        self.assertEqual(run.expand('{{package}}', values, 'x'), 'C:\\r\\2.0')
        self.assertEqual(values['repo'], str(run.ROOT))

    def test_mistakes_fail_when_the_catalog_loads(self):
        bad = {
            'unknown {{nope}}': catalog(dict(command='run {{nope}}')),
            'duplicate id': catalog({}, {}),
            "device 'phone'": catalog(dict(device='phone')),
            'unknown preconditions': catalog(dict(pre='[idle, sober]')),
            'needs a cases list': 'vars: {}\n',
        }
        for message, text in bad.items():
            with self.subTest(message=message), self.assertRaisesRegex(run.CatalogError, message):
                run.load(text)
        with self.assertRaisesRegex(run.CatalogError, 'missing command'):
            run.load('cases:\n  - {id: c, device: desktop, title: t, preconditions: [], pass: p, output: o}\n')
        with self.assertRaisesRegex(run.CatalogError, 'unknown fields colour'):
            run.load(catalog() + '    colour: red\n')

    def test_batches_skip_pending_and_manual_cases(self):
        text = (catalog(dict(id='ready'), dict(id='later'), dict(id='phone', device='user'),
                        dict(id='lap', device='laptop')))
        text = text.replace("id: later\n    device: desktop\n", "id: later\n    device: desktop\n    status: pending\n")
        _, cases = run.load(text)
        todo, skipped = run.select(cases, 'desktop')
        self.assertEqual([c.id for c in todo], ['ready'])
        self.assertEqual([(c.id, r) for c, r, _ in skipped], [('later', 'PENDING')])
        todo, _ = run.select(cases, 'desktop', ['later'])
        self.assertEqual([c.id for c in todo], ['later'])
        todo, skipped = run.select(cases, 'user')
        self.assertEqual((todo, [(c.id, r) for c, r, _ in skipped]), ([], [('phone', 'MANUAL')]))
        with self.assertRaisesRegex(run.CatalogError, 'no case typo'):
            run.select(cases, 'desktop', ['typo'])


class Preconditions(unittest.TestCase):
    def test_every_probe_of_a_precondition_runs_and_a_broken_probe_fails(self):
        def broken():
            raise OSError('no')
        fake = {'idle': (lambda: (True, 'sessions 0'), lambda: (False, 'input 2 min ago')),
                'isolated-ports': (broken,), 'system': (), 'gpu-lock': (lambda: (True, 'free'),)}
        results = run.check(['gpu-lock', 'idle', 'isolated-ports'], fake)
        self.assertEqual([(n, ok) for n, ok, _ in results],
                         [('gpu-lock', True), ('idle', True), ('idle', False), ('isolated-ports', False)])
        self.assertIn('probe failed', results[-1][2])

    def test_gpu_lock_is_free_held_or_ours(self):
        with tempfile.TemporaryDirectory() as t:
            lock = pathlib.Path(t) / 'GPU-LOCK'
            self.assertTrue(run.probe_gpu_lock(lock)[0])
            lock.write_text('another agent', encoding='utf-8')
            ok, detail = run.probe_gpu_lock(lock, ours='me')
            self.assertFalse(ok)
            self.assertIn('another agent', detail)
            lock.write_text('me', encoding='utf-8')
            self.assertTrue(run.probe_gpu_lock(lock, ours='me')[0])

    def test_serverinfo_counts_must_be_zero(self):
        def serve(xml):
            response = mock.MagicMock()
            response.__enter__.return_value.read.return_value = xml.encode()
            return mock.patch.object(run.urllib.request, 'urlopen', return_value=response)
        with serve('<root><RustHostSessionCount>0</RustHostSessionCount>'
                   '<RustHostPendingSessionCount>0</RustHostPendingSessionCount></root>'):
            self.assertTrue(run.probe_serverinfo()[0])
        with serve('<root><RustHostSessionCount>1</RustHostSessionCount>'
                   '<RustHostPendingSessionCount>0</RustHostPendingSessionCount></root>'):
            self.assertFalse(run.probe_serverinfo()[0])
        with serve('<root/>'):
            self.assertFalse(run.probe_serverinfo()[0])
        with mock.patch.object(run.urllib.request, 'urlopen', side_effect=ConnectionRefusedError()):
            ok, detail = run.probe_serverinfo()
            self.assertTrue(ok)
            self.assertIn('not reachable', detail)

    def test_ports_come_from_listeners_and_udp_sockets(self):
        netstat = textwrap.dedent('''\
            Aktive Verbindungen

              Proto  Lokale Adresse         Remoteadresse          Status           PID
              TCP    127.0.0.1:48123        0.0.0.0:0              ABHÖREN          100
              TCP    127.0.0.1:50000        127.0.0.1:48530        HERGESTELLT      101
              TCP    [::]:48989             [::]:0                 ABHÖREN          102
              UDP    127.0.0.1:48530        *:*                                     103
            ''')
        self.assertEqual(run.bound_ports(netstat), {48123, 48989, 48530})

    def test_live_probes_answer(self):
        for probe in (run.probe_input_idle, run.probe_fullscreen, run.probe_ports):
            ok, detail = probe()
            self.assertIsInstance(ok, bool)
            self.assertTrue(detail)


class Batch(unittest.TestCase):
    def test_verdicts(self):
        _, (case,) = run.load(catalog())
        self.assertEqual(run.verdict(case, 0, ''), 'PASS')
        self.assertEqual(run.verdict(case, 1, ''), 'FAIL')
        self.assertEqual(run.verdict(case, 3, ''), 'HANG')
        case.pass_regex = r'(?m)^EXITCODE 0$'
        self.assertEqual(run.verdict(case, 0, 'EXITCODE 1\n'), 'FAIL')
        self.assertEqual(run.verdict(case, 0, 'x\nEXITCODE 0\n'), 'PASS')
        case.verdict = 'review'
        self.assertEqual(run.verdict(case, 1, ''), 'REVIEW')
        self.assertEqual(run.verdict(case, 3, ''), 'HANG')

    def test_stream_timings_come_from_the_part_written_during_the_case(self):
        with tempfile.TemporaryDirectory() as t:
            log = pathlib.Path(t) / 'butterpollo.log'
            log.write_text('old stream timings\n', encoding='utf-8')
            start = run.log_mark(log)
            with open(log, 'a', encoding='utf-8') as f:
                f.write('x\nnew stream timings encode 2 ms\n')
            self.assertEqual(run.timing_lines(log, start), ['new stream timings encode 2 ms'])
            log.write_text('rotated stream timings\n', encoding='utf-8')
            self.assertEqual(run.timing_lines(log, start), ['rotated stream timings'])
            self.assertEqual(run.timing_lines(pathlib.Path(t) / 'missing.log'), [])

    def test_a_hang_stops_the_batch_and_everything_is_collected(self):
        ok = ("New-Item -ItemType Directory -Force {{out}}\\e2e\\config\\logs | Out-Null; "
              "Set-Content {{out}}\\e2e\\result.json '{\"passed\": true}'; "
              "Set-Content {{out}}\\e2e\\config\\logs\\butterpollo.log 'stream timings capture 1 ms'; "
              "Set-Content {{out}}\\shot.png 'png'; if (Test-Path {{lock}}) { exit 0 } else { exit 1 }")
        text = catalog(dict(id='first', command=ok.replace("'", "''"), pre='[gpu-lock]'),
                       dict(id='hang', command='exit 3'), dict(id='after'),
                       variables='vars: {lock: "%LOCK%"}\n')
        with tempfile.TemporaryDirectory() as t:
            root = pathlib.Path(t)
            lock = root / 'GPU-LOCK'
            (root / 'catalog.yaml').write_text(text.replace('%LOCK%', str(lock).replace('\\', '\\\\')),
                                               encoding='utf-8')
            with mock.patch.object(run, 'GPU_LOCK', lock), mock.patch.object(run, 'HOST_LOG', root / 'none.log'), \
                    contextlib.redirect_stdout(io.StringIO()):
                code = run.main(['--batch', 'desktop', '--catalog', str(root / 'catalog.yaml'),
                                 '--out-root', str(root / 'hw')])
            self.assertEqual(code, run.EXIT_GPU_HANG)
            self.assertFalse(lock.exists(), 'the batch releases the GPU lock')
            batch, = (root / 'hw').glob('*/desktop')
            summary = (batch / 'summary.md').read_text(encoding='utf-8')
            self.assertIn('| first | PASS | 0 |', summary)
            self.assertIn('e2e\\result.json (True)', summary)
            self.assertIn('| hang | HANG | 3 |', summary)
            self.assertIn('| after | NOT RUN |', summary)
            self.assertIn('encoder hang', summary)
            self.assertIn('stream timings capture 1 ms', (batch / 'first/stream-timings.txt').read_text(encoding='utf-8'))
            self.assertFalse((batch / 'after').exists())

    def test_a_failed_precondition_stops_before_running(self):
        text = catalog(dict(id='first', command='exit 0', pre='[gpu-lock]'))
        with tempfile.TemporaryDirectory() as t:
            root = pathlib.Path(t)
            lock = root / 'GPU-LOCK'
            lock.write_text('someone else', encoding='utf-8')
            (root / 'catalog.yaml').write_text(text, encoding='utf-8')
            with mock.patch.object(run, 'GPU_LOCK', lock), contextlib.redirect_stdout(io.StringIO()):
                code = run.main(['--batch', 'desktop', '--catalog', str(root / 'catalog.yaml'),
                                 '--out-root', str(root / 'hw')])
            self.assertEqual(code, 1)
            self.assertEqual(lock.read_text(encoding='utf-8'), 'someone else')
            summary, = (root / 'hw').glob('*/desktop/summary.md')
            self.assertIn('someone else', summary.read_text(encoding='utf-8'))

    def test_a_case_that_manages_its_locks_runs_without_the_batchs_gpu_lock(self):
        # Each case's exit code says whether it ran under the batch's GPU lock.
        held = 'if (Test-Path {{lock}}) { exit 0 } else { exit 1 }'
        free = 'if (Test-Path {{lock}}) { exit 1 } else { exit 0 }'
        text = catalog(dict(id='first', command=held, pre='[gpu-lock]'), dict(id='check', command=free),
                       dict(id='after', command=held, pre='[gpu-lock]'), variables='vars: {lock: "%LOCK%"}\n')
        text = text.replace('id: check\n    device: desktop\n', 'id: check\n    device: desktop\n    manages_locks: true\n')
        with tempfile.TemporaryDirectory() as t:
            root = pathlib.Path(t)
            lock = root / 'GPU-LOCK'
            (root / 'catalog.yaml').write_text(text.replace('%LOCK%', str(lock).replace('\\', '\\\\')), encoding='utf-8')
            self.assertTrue(run.load((root / 'catalog.yaml').read_text(encoding='utf-8'))[1][1].manages_locks)
            with mock.patch.object(run, 'GPU_LOCK', lock), mock.patch.object(run, 'HOST_LOG', root / 'none.log'), \
                    contextlib.redirect_stdout(io.StringIO()):
                code = run.main(['--batch', 'desktop', '--catalog', str(root / 'catalog.yaml'),
                                 '--out-root', str(root / 'hw')])
            summary, = (root / 'hw').glob('*/desktop/summary.md')
            text = summary.read_text(encoding='utf-8')
            self.assertEqual(code, 0, text)
            for case in ('first', 'check', 'after'):
                self.assertIn(f'| {case} | PASS | 0 |', text)
            self.assertFalse(lock.exists())
        with self.assertRaisesRegex(run.CatalogError, 'cannot need gpu-lock'):
            run.load(catalog(dict(pre='[gpu-lock]')) + '    manages_locks: true\n')
        with self.assertRaisesRegex(run.CatalogError, 'not true or false'):
            run.load(catalog() + '    manages_locks: yes please\n')

    def test_the_gpu_lock_is_waited_for_never_broken(self):
        with tempfile.TemporaryDirectory() as t:
            lock = pathlib.Path(t) / 'GPU-LOCK'
            lock.write_text('rc4 pass tv t6 4242', encoding='utf-8')
            now = [0.0]
            def sleep(seconds):
                now[0] += seconds
                if now[0] >= 30:
                    lock.unlink(missing_ok=True)
            with contextlib.redirect_stdout(io.StringIO()) as out:
                self.assertEqual(run.take_gpu_lock('me', lock, clock=lambda: now[0], sleep=sleep), (True, 'taken'))
            self.assertEqual(lock.read_text(encoding='utf-8'), 'me')
            self.assertIn('rc4 pass tv t6 4242', out.getvalue())
            lock.write_text('rc4 pass tv t6 4242', encoding='utf-8')
            # A holder that never lets go: said at 0 and 60 s, given up on at 120 s.
            with contextlib.redirect_stdout(io.StringIO()) as out:
                ok, detail = run.take_gpu_lock('me', lock, deadline=120, clock=lambda: now[0],
                                               sleep=lambda s: now.__setitem__(0, now[0] + s))
            self.assertFalse(ok)
            self.assertIn('rc4 pass tv t6 4242', detail)
            self.assertEqual(lock.read_text(encoding='utf-8'), 'rc4 pass tv t6 4242')
            self.assertEqual(out.getvalue().count('waiting for the GPU lock'), 2)

    def test_the_gpu_lock_is_released_past_a_reader(self):
        with tempfile.TemporaryDirectory() as t:
            lock = pathlib.Path(t) / 'GPU-LOCK'
            lock.write_text('me', encoding='utf-8')
            unlink, calls = pathlib.Path.unlink, []
            def busy_once(path, *args, **kwargs):
                calls.append(path)
                if len(calls) == 1:
                    raise PermissionError(32, 'The process cannot access the file because it is being used')
                return unlink(path, *args, **kwargs)
            with mock.patch.object(pathlib.Path, 'unlink', busy_once):
                run.release_gpu_lock('me', lock, sleep=lambda s: None)
            self.assertEqual(len(calls), 2)
            self.assertFalse(lock.exists())
            lock.write_text('me', encoding='utf-8')
            with mock.patch.object(pathlib.Path, 'unlink', side_effect=PermissionError(32, 'in use')), \
                    self.assertRaises(PermissionError):
                run.release_gpu_lock('me', lock, sleep=lambda s: None)
            lock.write_text('someone else', encoding='utf-8')
            run.release_gpu_lock('me', lock)
            self.assertEqual(lock.read_text(encoding='utf-8'), 'someone else')

    def test_dry_run_runs_nothing(self):
        out = io.StringIO()
        with tempfile.TemporaryDirectory() as t, contextlib.redirect_stdout(out):
            code = run.main(['--batch', 'desktop', '--dry-run', '--out-root', t])
            self.assertEqual(list(pathlib.Path(t).iterdir()), [])
        self.assertEqual(code, 0)
        self.assertIn('[e2e-hevc]', out.getvalue())
        self.assertIn('PENDING  encoder-focus-ab', out.getvalue())


if __name__ == '__main__':
    unittest.main()
