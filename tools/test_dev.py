import os, pathlib, sys, tempfile, unittest

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
        self.assertEqual(planned['host'].items, ['butterpollo', 'butterpollo-windows'])
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

    def test_leaf_client_crate_checks_only_itself(self):
        planned, _ = checks('client/protocol/policy/src/lib.rs')
        self.assertEqual(planned['client-protocol'].items, ['nightjar-policy'])
        self.assertNotIn('host', planned)
        planned, _ = checks('client/protocol/desktop/src/main.rs')
        self.assertEqual(planned['client-protocol'].items, ['nightjar-desktop'])

    def test_client_lock_file_checks_the_default_members(self):
        planned, _ = checks('client/protocol/Cargo.lock')
        client = planned['client-protocol'].items
        self.assertIn('rubylight-protocol', client)
        self.assertNotIn('nightjar-desktop', client)  # SDL3 and FFmpeg, not a default member

    def test_interop_skips_until_it_exists(self):
        planned, _ = checks('client/protocol/client/src/lib.rs')
        if not (ROOT / 'tests/interop').exists():
            self.assertTrue(planned['interop'].skip)

    def test_client_specs_name_the_path_package(self):
        spec, = dev.client_specs(ROOT, ['rubylight-protocol'])
        self.assertTrue(spec.startswith('path+file:///'), spec)
        self.assertTrue(spec.endswith('/client/protocol/core#rubylight-protocol'), spec)

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

    def test_docs_map_to_nothing(self):
        planned, unchecked = checks('docs/architecture.md', 'rust/core/README.md', 'client/notes.d/x.md',
                                    'client/site/index.html')
        self.assertEqual(planned, {})
        self.assertEqual(len(unchecked), 4)

    def test_target_dir_follows_the_worktree(self):
        self.assertEqual(dev.default_target(pathlib.Path(r'C:\src\rl-tools')), pathlib.Path(r'C:\src\rl-target-tools'))
        self.assertEqual(dev.default_target(pathlib.Path(r'C:\src\rubylight-rel')), pathlib.Path(r'C:\src\rl-target'))


class QuickLog(unittest.TestCase):
    def quick(self, exit_line):
        # As rl-quick writes it under Windows PowerShell: Out-File -Encoding utf8 for its own
        # lines (a BOM each time), `*>>` appends cargo's output as UTF-16LE with CRLF.
        utf8 = lambda s: '\ufeff'.encode('utf-8') + (s + '\r\n').encode('utf-8')
        utf16 = lambda s: '\ufeff'.encode('utf-16-le') + (s + '\r\n').encode('utf-16-le')
        data = (utf8('== test') + utf16('test result: ok. 3 passed; 0 failed') + utf16('Grüße')
                + utf8(exit_line))
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


if __name__ == '__main__':
    unittest.main()
