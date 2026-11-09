import contextlib, io, json, os, pathlib, sys, tempfile, unittest
from unittest import mock

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import dev  # noqa: E402

ROOT = dev.ROOT


def checks(*paths):
    planned, unchecked = dev.plan(paths, ROOT)
    return {c.name: c for c in planned}, unchecked


class Plan(unittest.TestCase):
    def test_core_change_checks_its_dependants(self):
        planned, _ = checks('rust/core/src/lib.rs')
        crates = set(planned['host'].items)
        self.assertTrue({'butterpollo-core', 'butterpollo-windows', 'butterpollo'} <= crates, crates)
        self.assertNotIn('butterpollo-vulkan-layer', crates)
        self.assertNotIn('client-protocol', planned)

    def test_host_crate_change_skips_the_crates_below_it(self):
        planned, _ = checks('rust/windows/src/lib.rs')
        self.assertEqual(planned['host'].items, ['butterpollo', 'butterpollo-probes', 'butterpollo-windows'])
        planned, _ = checks('rust/host/src/main.rs')
        self.assertEqual(planned['host'].items, ['butterpollo'])

    def test_nested_member_maps_to_itself(self):
        planned, _ = checks('rust/core/native-api/src/lib.rs')
        self.assertIn('rubylight-native-api', planned['host'].items)
        self.assertIn('butterpollo-core', planned['host'].items)

    def test_workspace_files_check_every_host_crate(self):
        for path in ('Cargo.lock', 'rust-toolchain.toml', '.cargo/config.toml'):
            with self.subTest(path=path):
                planned, _ = checks(path)
                self.assertIn('butterpollo-vulkan-layer', planned['host'].items)
                self.assertIn('rubylight-vdisplay-proto', planned['host'].items)

    def test_protocol_crate_checks_client_and_host(self):
        planned, _ = checks('client/protocol/core/src/lib.rs')
        self.assertIn('butterpollo-core', planned['host'].items)
        client = set(planned['client-protocol'].items)
        self.assertTrue({'rubylight-protocol', 'nightjar-core', 'nightjar-desktop'} <= client, client)
        self.assertIn('interop', planned)

    def test_leaf_client_crate_checks_only_itself_and_its_users(self):
        # nightjar-android and nightjar-desktop take nightjar-policy by path, so they are checked too.
        planned, _ = checks('client/protocol/policy/src/lib.rs')
        self.assertEqual(planned['client-protocol'].items, ['nightjar-android', 'nightjar-desktop', 'nightjar-policy'])
        self.assertNotIn('host', planned)
        planned, _ = checks('client/protocol/desktop/src/main.rs')
        self.assertEqual(planned['client-protocol'].items, ['nightjar-desktop'])

    def test_client_lock_file_checks_the_default_members(self):
        planned, _ = checks('client/protocol/Cargo.lock')
        client = planned['client-protocol'].items
        self.assertIn('rubylight-protocol', client)
        self.assertNotIn('nightjar-desktop', client)  # SDL3 and FFmpeg, not a default member

    def test_client_lock_file_keeps_a_crate_changed_with_it(self):
        planned, _ = checks('client/protocol/Cargo.lock', 'client/protocol/desktop/src/main.rs')
        client = planned['client-protocol'].items
        self.assertIn('rubylight-protocol', client)
        self.assertIn('nightjar-desktop', client)

    def test_interop_skips_until_it_exists(self):
        planned, _ = checks('client/protocol/client/src/lib.rs')
        if not (ROOT / 'tests/interop').exists():
            self.assertTrue(planned['interop'].skip)

    def test_client_specs_name_the_path_package(self):
        spec, = dev.client_specs(ROOT, ['rubylight-protocol'])
        self.assertTrue(spec.startswith('path+file:///'), spec)
        self.assertTrue(spec.endswith('/client/protocol/core#rubylight-protocol'), spec)

    def test_desktop_client_gets_clippy_as_ci_runs_it(self):
        test, clippy = dev.client_commands(ROOT, ['nightjar-desktop', 'nightjar-policy'])
        self.assertEqual(test[:2], ['cargo', 'test'])
        self.assertEqual(clippy[:2], ['cargo', 'clippy'])
        self.assertEqual(clippy[-3:], ['--', '-D', 'warnings'])
        self.assertIn('--all-targets', clippy)
        linted = [clippy[i + 1] for i, a in enumerate(clippy) if a == '-p']
        self.assertEqual(len(linted), 1)
        self.assertTrue(linted[0].endswith('#nightjar-desktop'), linted)

    def test_client_crates_ci_does_not_lint_get_tests_only(self):
        commands = dev.client_commands(ROOT, ['rubylight-protocol'])
        self.assertEqual([c[1] for c in commands], ['test'])

    def test_app_sources_map_to_their_unit_tests(self):
        test = 'client/app/src/test/java/com/limelight/AppViewLifecycleTest.java'
        planned, _ = checks(test)
        self.assertEqual(planned['app'].items, ['com.limelight.AppViewLifecycleTest'])
        planned, _ = checks('client/app/src/main/java/com/limelight/AppView.java')
        self.assertIn('com.limelight.AppViewLifecycleTest', planned['app'].items)
        self.assertNotIn('com.limelight.GameInputTest', planned['app'].items)

    def test_uncovered_app_class_is_reported(self):
        planned, _ = checks('client/app/src/main/java/com/limelight/NoSuchClassAnywhere.java')
        self.assertNotIn('app', planned)
        self.assertIn('NoSuchClassAnywhere', planned['app-uncovered'].skip)

    def test_app_resources_and_build_run_every_unit_test(self):
        for path in ('client/app/src/main/res/values/strings.xml', 'client/app/build.gradle'):
            with self.subTest(path=path):
                planned, _ = checks(path)
                self.assertEqual(planned['app'].items, [])

    def test_web_release_and_tools(self):
        planned, _ = checks('rust/web/src/App.svelte')
        self.assertEqual(set(planned), {'web'})
        planned, _ = checks('rust/release/e2e_result.py')
        self.assertIn('test_e2e_result.py', planned['release-py'].items)
        self.assertIn('test_release.py', planned['release-py'].items)
        planned, _ = checks('rust/release/test_soak.py')
        self.assertIn('test_soak.py', planned['release-py'].items)
        planned, _ = checks('tools/hw/catalog.yaml')
        self.assertEqual(set(planned), {'tools-py'})
        planned, _ = checks('tools/nightjar_notes.py')
        self.assertEqual(set(planned), {'tools-py', 'release-py'})

    def test_the_service_has_its_own_check_and_its_version_rule(self):
        planned, _ = checks('rust/service/src/supervisor.rs')
        self.assertEqual(set(planned), {'service', 'release-py'})
        self.assertIn('test_release.py', planned['release-py'].items)
        # The version resource script goes into every executable, the service's too.
        planned, _ = checks('rust/assets/version_info.rs')
        self.assertEqual(set(planned), {'host', 'service', 'release-py'})
        self.assertIn('butterpollo-setup', planned['host'].items)
        planned, _ = checks('rust-toolchain.toml')
        self.assertIn('service', planned)
        # It compiles core's update recovery itself.
        planned, _ = checks('rust/core/src/update_recovery.rs')
        self.assertIn('service', planned)
        self.assertIn('butterpollo-core', planned['host'].items)

    def test_docs_map_to_nothing(self):
        planned, unchecked = checks('docs/architecture.md', 'rust/core/README.md', 'client/notes.d/x.md',
                                    'client/site/index.html')
        self.assertEqual(planned, {})
        self.assertEqual(len(unchecked), 4)

    def test_target_dir_follows_the_worktree(self):
        self.assertEqual(dev.default_target(pathlib.Path(r'C:\src\rl-tools')), pathlib.Path(r'C:\src\rl-target-tools'))
        self.assertEqual(dev.default_target(pathlib.Path(r'C:\src\rubylight-rel')), pathlib.Path(r'C:\src\rl-target'))


class QuickLog(unittest.TestCase):
    def quick(self, exit_line, cargo=('test result: ok. 3 passed; 0 failed', 'Grüße')):
        # As rl-quick writes it under Windows PowerShell: Out-File -Encoding utf8 for its own
        # lines (a BOM each time), `*>>` appends cargo's output as UTF-16LE with CRLF.
        utf8 = lambda s: '\ufeff'.encode('utf-8') + (s + '\r\n').encode('utf-8')
        utf16 = lambda s: '\ufeff'.encode('utf-16-le') + (s + '\r\n').encode('utf-16-le')
        data = utf8('== test') + b''.join(utf16(line) for line in cargo) + utf8(exit_line)
        with tempfile.TemporaryDirectory() as directory:
            path = pathlib.Path(directory, 'host.log.quick')
            path.write_bytes(data)
            return dev.read_quick(path)

    def test_mixed_encodings_still_show_the_exit_line(self):
        text, passed = self.quick('EXIT 0')
        self.assertTrue(passed, repr(text))
        self.assertIn('\ntest result: ok. 3 passed; 0 failed\n', text)
        self.assertNotIn('\x00', text)

    def test_programs_come_from_the_env_path_and_a_missing_one_fails_in_its_log(self):
        with tempfile.TemporaryDirectory() as directory:
            bin_dir = pathlib.Path(directory, 'bin')
            bin_dir.mkdir()
            (bin_dir / 'only-here.cmd').write_text('@exit /b 7\n')
            log = pathlib.Path(directory, 'check.log')
            env = dict(os.environ, PATH=f'{bin_dir};{os.environ["PATH"]}')
            self.assertEqual(dev.run(['only-here', 'x'], log, directory, env), 7)
            self.assertEqual(dev.run(['no-such-program-here'], log, directory), 127)
            self.assertIn('no-such-program-here is not on PATH', log.read_text(encoding='utf-8'))

    def test_a_failure_or_a_missing_log_fails(self):
        self.assertFalse(self.quick('EXIT 1')[1])
        self.assertEqual(dev.read_quick(pathlib.Path('missing.quick')), ('', False))

    def test_exit_0_with_nothing_run_or_a_failed_test_fails(self):
        # cargo off PATH: PowerShell leaves $LASTEXITCODE at 0 and rl-quick says EXIT 0.
        missing = ("cargo : The term 'cargo' is not recognized as a name of a cmdlet",
                   '    + FullyQualifiedErrorId : CommandNotFoundException')
        self.assertFalse(self.quick('EXIT 0', missing)[1])
        self.assertFalse(self.quick('EXIT 0', ())[1])
        failed = ('test result: ok. 3 passed; 0 failed', 'test result: FAILED. 1 passed; 1 failed')
        self.assertFalse(self.quick('EXIT 0', failed)[1])


class Execution(unittest.TestCase):
    def test_failed_client_tests_do_not_start_clippy(self):
        with tempfile.TemporaryDirectory() as directory, mock.patch.object(dev, 'run', return_value=1) as run:
            self.assertFalse(dev.execute(dev.Check('client', 'client', ['nightjar-desktop']), ROOT,
                                         pathlib.Path(directory) / 'check.log', pathlib.Path(directory)))
            self.assertEqual(run.call_count, 1)
            self.assertIn('--locked', run.call_args.args[0])

    def test_run_logs_and_summaries_are_unique_and_fail_fast_is_optional(self):
        with tempfile.TemporaryDirectory() as directory:
            for extra, expected in (([], 1), (['--keep-going'], 2)):
                with mock.patch.object(dev, 'plan', return_value=([dev.Check('one', 'python'), dev.Check('two', 'python')], [])), \
                        mock.patch.object(dev, 'execute', side_effect=[False, True]) as execute, \
                        mock.patch.object(dev, 'git', return_value='abc123\n'), contextlib.redirect_stdout(io.StringIO()):
                    self.assertEqual(dev.main(['check', '--paths', 'rust/release/x.py', '--log-dir', directory, *extra]), 1)
                    self.assertEqual(execute.call_count, expected)
            summaries = [json.loads(p.read_text()) for p in pathlib.Path(directory).glob('*/summary.json')]
            self.assertEqual(len(summaries), 2)
            self.assertTrue(all(s['head'] == 'abc123' and not s['passed'] for s in summaries))
            self.assertEqual({s['checks'][1]['result'] for s in summaries}, {'NOT RUN', 'PASS'})

    def test_web_dependencies_reuse_only_a_successful_matching_install(self):
        with tempfile.TemporaryDirectory() as directory:
            web = pathlib.Path(directory)
            for name in ('package.json', 'package-lock.json'):
                (web / name).write_text('{}')
            installs = []
            def install(*args, **kwargs):
                installs.append(args[0])
                (web / 'node_modules').mkdir(exist_ok=True)
                (web / 'node_modules/.package-lock.json').write_text('{}')
                return 0
            with mock.patch.object(dev.subprocess, 'run', return_value=mock.Mock(returncode=0, stdout='v22')), \
                    mock.patch.object(dev, 'run', side_effect=install):
                self.assertTrue(dev.web_dependencies(web, web / 'log'))
                self.assertTrue(dev.web_dependencies(web, web / 'log'))
                self.assertEqual(len(installs), 1)
                (web / 'package-lock.json').write_text('{"changed":true}')
                self.assertTrue(dev.web_dependencies(web, web / 'log'))
                self.assertEqual(len(installs), 2)
                self.assertTrue(dev.web_dependencies(web, web / 'log', refresh=True))
                self.assertEqual(len(installs), 3)
                (web / 'node_modules/.package-lock.json').unlink()
                self.assertTrue(dev.web_dependencies(web, web / 'log'))
                self.assertEqual(len(installs), 4)
                with mock.patch.object(dev, 'run', return_value=1):
                    self.assertFalse(dev.web_dependencies(web, web / 'log', refresh=True))
                self.assertFalse((web / 'node_modules/.rubylight-dependencies.json').exists())


if __name__ == '__main__':
    unittest.main()
