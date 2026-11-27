import contextlib, io, json, os, pathlib, sys, tempfile, threading, time, unittest
from unittest import mock

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import dev  # noqa: E402

ROOT = dev.ROOT
CARGO = dev.shutil.which("cargo") or dev.shutil.which("cargo", path=str(pathlib.Path.home() / ".cargo/bin"))


def checks(*paths):
    planned, unchecked = dev.plan(paths, ROOT)
    return {c.name: c for c in planned}, unchecked


class Plan(unittest.TestCase):
    def test_core_change_checks_its_dependants(self):
        planned, _ = checks('rust/core/config/src/lib.rs')
        crates = set(planned['host'].items)
        self.assertTrue({
            'butterpollo-config', 'butterpollo-media-policy', 'butterpollo-input-policy',
            'butterpollo-session', 'butterpollo-win-display', 'butterpollo-win-graphics',
            'butterpollo-win-io', 'butterpollo-stream', 'butterpollo-webrtc',
            'butterpollo-web', 'butterpollo',
        } <= crates, crates)
        self.assertNotIn('butterpollo-crypto', crates)
        self.assertNotIn('butterpollo-win-abi', crates)
        self.assertNotIn('butterpollo-vulkan-layer', crates)
        self.assertNotIn('client-protocol', planned)

    def test_host_crate_change_skips_the_crates_below_it(self):
        planned, _ = checks('rust/windows/display/src/lib.rs')
        self.assertEqual(
            planned['host'].items,
            [
                'butterpollo',
                'butterpollo-host-tests',
                'butterpollo-probes',
                'butterpollo-stream',
                'butterpollo-web',
                'butterpollo-webrtc',
                'butterpollo-win-display',
                'rubylight-interop',
            ],
        )
        planned, _ = checks('rust/host/src/main.rs')
        self.assertEqual(planned['host'].items, ['butterpollo', 'rubylight-interop'])

    def test_nested_member_maps_to_itself(self):
        planned, _ = checks('client/protocol/native-api/src/lib.rs')
        self.assertIn('rubylight-native-api', planned['client-protocol'].items)
        self.assertIn('butterpollo-session', planned['host'].items)

    def test_workspace_files_check_every_host_crate(self):
        for path in ('Cargo.lock', 'rust-toolchain.toml', '.cargo/config.toml'):
            with self.subTest(path=path):
                planned, _ = checks(path)
                self.assertIn('butterpollo-vulkan-layer', planned['host'].items)
                self.assertIn('rubylight-vdisplay-proto', planned['host'].items)

    def test_protocol_crate_checks_client_and_host(self):
        planned, _ = checks('client/protocol/core/src/lib.rs')
        self.assertTrue({
            'butterpollo-media-policy', 'butterpollo-input-policy', 'butterpollo-session',
        } <= set(planned['host'].items), planned['host'].items)
        client = set(planned['client-protocol'].items)
        self.assertTrue({'rubylight-protocol', 'nightjar-core', 'nightjar-desktop'} <= client, client)
        self.assertIn('rubylight-interop', planned['host'].items)

    def test_leaf_client_crate_checks_only_itself_and_its_users(self):
        # nightjar-android, nightjar-cli, nightjar-core and nightjar-desktop* take nightjar-policy by path, so they are checked too.
        planned, _ = checks('client/protocol/policy/src/lib.rs')
        self.assertEqual(planned['client-protocol'].items, [
            'nightjar-android', 'nightjar-cli', 'nightjar-core', 'nightjar-desktop',
            'nightjar-desktop-config', 'nightjar-desktop-media', 'nightjar-desktop-ui', 'nightjar-policy',
        ])
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

    def test_interop_checks_run_on_client_protocol_changes(self):
        planned, _ = checks('client/protocol/client/src/lib.rs')
        if not (ROOT / 'tests/interop').exists():
            self.assertTrue(planned['interop'].skip)
        else:
            self.assertIn('rubylight-interop', planned['host'].items)

    def test_client_crates_get_clippy_as_ci_runs_it(self):
        test, clippy = dev.client_commands(ROOT, ['nightjar-desktop', 'nightjar-policy'])
        self.assertEqual(test[:2], ['cargo', 'test'])
        self.assertEqual(clippy[:2], ['cargo', 'clippy'])
        self.assertEqual(clippy[-3:], ['--', '-D', 'warnings'])
        self.assertIn('--all-targets', clippy)
        linted = [clippy[i + 1] for i, a in enumerate(clippy) if a == '-p']
        self.assertEqual(linted, ['nightjar-desktop', 'nightjar-policy'])

    def test_all_client_crates_run_tests_and_clippy(self):
        commands = dev.client_commands(ROOT, ['rubylight-protocol'])
        self.assertEqual([c[1] for c in commands], ['test', 'clippy'])

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
        self.assertEqual(set(planned), {'web', 'arch-check'})
        planned, _ = checks('rust/release/e2e_result.py')
        self.assertIn('test_e2e_result.py', planned['release-py'].items)
        self.assertIn('test_release.py', planned['release-py'].items)
        self.assertEqual(set(planned['release-py'].items),
                         {p.name for p in (ROOT / 'rust/release').glob('test_*.py')})
        self.assertGreaterEqual(len(planned['release-py'].items), 14)
        planned, _ = checks('rust/release/test_soak.py')
        self.assertIn('test_soak.py', planned['release-py'].items)
        planned, _ = checks('tools/hw/catalog.yaml')
        self.assertEqual(set(planned), {'tools-py', 'arch-check'})
        planned, _ = checks('tools/nightjar_notes.py')
        self.assertEqual(set(planned), {'tools-py', 'release-py', 'arch-check'})

    def test_the_service_has_its_own_check_and_its_version_rule(self):
        planned, _ = checks('rust/service/src/supervisor.rs')
        self.assertEqual(set(planned), {'service', 'release-py', 'arch-check'})
        self.assertIn('test_release.py', planned['release-py'].items)
        # The version resource script goes into every executable, the service's too.
        planned, _ = checks('rust/assets/version_info.rs')
        self.assertEqual(set(planned), {'host', 'service', 'release-py', 'arch-check'})
        self.assertIn('butterpollo-setup', planned['host'].items)
        planned, _ = checks('rust-toolchain.toml')
        self.assertIn('service', planned)
        # It compiles core's update recovery itself.
        planned, _ = checks('rust/core/src/update_recovery.rs')
        self.assertIn('service', planned)
        self.assertIn('butterpollo-session', planned['host'].items)

    def test_generated_docs_are_checked_when_their_sources_change(self):
        planned, _ = checks('client/app/src/main/res/xml/preferences.xml')
        self.assertEqual(planned['generated-docs'].items, ['client/docs/settings.md'])
        planned, _ = checks('client/docs/devices.md')
        self.assertEqual(planned['generated-docs'].items, ['client/docs/devices.md'])

    def test_docs_map_to_nothing(self):
        planned, unchecked = checks('docs/architecture.md', 'rust/core/README.md', 'client/notes.d/x.md',
                                    'client/site/index.html')
        self.assertEqual(planned, {})
        self.assertEqual(len(unchecked), 4)



class WarmTargets(unittest.TestCase):
    """Reused target dirs: a checkout never gets another checkout's build of its own packages."""

    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.target = pathlib.Path(temp.name, 'slot')
        self.dirs = []
        for profile in ('debug/.fingerprint', 'service/debug/.fingerprint'):
            for name in ('butterpollo-session-0123456789abcdef', 'rubylight-protocol-fedcba9876543210',
                         'tokio-00112233445566aa', 'butterpollo-session-extra'):
                path = self.target / profile / name
                path.mkdir(parents=True)
                self.dirs.append(path)

    def left(self):
        return sorted(str(p.relative_to(self.target)).replace('\\', '/') for p in self.dirs if p.exists())

    def test_the_repository_packages_are_found(self):
        names = dev.local_packages(ROOT)
        self.assertTrue({
            'butterpollo-crypto', 'butterpollo-config', 'butterpollo-media-policy',
            'butterpollo-input-policy', 'butterpollo-session', 'butterpollo',
            'rubylight-protocol', 'butterpollo-service',
        } <= names, names)

    def test_another_checkouts_build_loses_only_this_repositorys_fingerprints(self):
        (self.target / dev.STAMP).write_text(r'C:\src\other-checkout')
        removed = dev.claim_target(self.target, ROOT)
        self.assertEqual(removed, 4)
        self.assertEqual(self.left(), ['debug/.fingerprint/butterpollo-session-extra', 'debug/.fingerprint/tokio-00112233445566aa',
                                       'service/debug/.fingerprint/butterpollo-session-extra',
                                       'service/debug/.fingerprint/tokio-00112233445566aa'])
        self.assertTrue(dev.same_path((self.target / dev.STAMP).read_text(), ROOT))

    def test_cargo_1_100_build_folders_are_reset_too(self):
        # Cargo 1.100 keeps <profile>/build/<name>/<16 hex>/{fingerprint,out} and no .fingerprint.
        target = self.target.parent / 'new-layout'
        mine = [target / 'debug/build/butterpollo-session/0123456789abcdef/fingerprint',
                target / 'service/debug/build/rubylight-protocol/fedcba9876543210/out']
        kept = [target / 'debug/build/tokio/00112233445566aa/out', target / 'debug/build/butterpollo-session/notahash',
                target / 'debug/build/butterpollo-session-0123456789abcdef']
        for path in mine + kept:
            path.mkdir(parents=True)
        (target / dev.STAMP).write_text(r'C:\src\other-checkout')
        self.assertEqual(dev.claim_target(target, ROOT), 2)
        self.assertEqual([p.exists() for p in mine + kept], [False, False, True, True, True])
        unknown = self.target.parent / 'unknown'
        (unknown / 'debug/build/butterpollo-session/0123456789abcdef').mkdir(parents=True)
        self.assertEqual(dev.claim_target(unknown, ROOT), 1)

    def test_its_own_last_build_is_kept(self):
        (self.target / dev.STAMP).write_text(str(ROOT).upper() + '\\')
        self.assertEqual(dev.claim_target(self.target, ROOT), 0)
        self.assertEqual(len(self.left()), 8)

    def test_a_slot_rl_quick_last_entered_for_another_checkout_is_reset(self):
        # rl-quick writes the slot's owner file but not the stamp inside the folder.
        (self.target / dev.STAMP).write_text(str(ROOT))
        self.assertEqual(dev.claim_target(self.target, ROOT, previous_owner=r'C:\src\other'), 4)

    def test_a_slot_entered_after_the_last_claim_is_reset_whoever_it_names(self):
        # Claim by A, rl-quick from B, rl-quick from A again: the owner file names A, but B built there.
        (self.target / dev.STAMP).write_text(str(ROOT))
        os.utime(self.target / dev.STAMP, (1000, 1000))
        self.assertEqual(dev.claim_target(self.target, ROOT, previous_owner=str(ROOT), owner_since=5000), 4)
        self.assertEqual(dev.claim_target(self.target, ROOT, previous_owner=str(ROOT), owner_since=1000), 0)

    def test_a_target_dir_given_is_locked_too(self):
        args = dev.argparse.Namespace(lock_directory=self.target.parent, pool=None, target_dir=self.target)
        groups = dict(dev.stage_groups(dev.Check('client', 'client'), ROOT, args))
        self.assertEqual(groups['target dir']()[0].path, pathlib.Path(f'{self.target}-client.lock'))

    def test_a_folder_with_builds_of_unknown_checkouts_is_reset(self):
        self.assertEqual(dev.claim_target(self.target, ROOT), 4)

    def test_an_empty_folder_needs_nothing(self):
        empty = self.target.parent / 'empty'
        self.assertEqual(dev.claim_target(empty, ROOT), 0)
        self.assertTrue((empty / dev.STAMP).exists())

    def test_own_slots_first_then_others_oldest_then_new(self):
        pool = self.target.parent / 'pool'
        pool.mkdir()
        slots = [dev.build_lock.HandleSlot(pool / f'{n}.lock') for n in (1, 2, 3, 4)]
        (pool / '1.owner').write_text(r'C:\src\recent')
        (pool / '2.owner').write_text(r'C:\src\old')
        os.utime(pool / '2.owner', (1, 1))
        (pool / '4.owner').write_text(str(ROOT) + '\r\n')  # as Set-Content writes it
        order = [s.path.stem for s in dev.by_affinity(slots, ROOT)]
        self.assertEqual(order, ['4', '2', '1', '3'])

    def test_each_stage_takes_its_own_lanes(self):
        args = dev.argparse.Namespace(lock_directory=self.target.parent, pool=None, target_dir=None)
        names = lambda kind: [name for name, _ in dev.stage_groups(dev.Check(kind, kind), ROOT, args)]
        self.assertEqual(names('host'), ['cargo', 'host target'])
        self.assertEqual(names('service'), ['cargo', 'host target'])
        self.assertEqual(names('client'), ['cargo', 'client target'])
        self.assertEqual(names('gradle'), ['gradle', 'gradle target'])
        self.assertEqual(names('web'), ['web'])
        self.assertEqual(names('python'), [])
        args.target_dir = self.target
        self.assertEqual(names('host'), ['cargo', 'target dir'])
        self.assertEqual(names('gradle'), ['gradle', 'target dir'])
        # Dotted --target-dir appends .lock rather than replacing the version suffix.
        dotted = self.target.parent / 'v6.4.5'
        args.target_dir = dotted
        groups = dict(dev.stage_groups(dev.Check('app', 'gradle'), ROOT, args))
        self.assertEqual(groups['target dir']()[0].path, pathlib.Path(f'{dotted}-gradle.lock'))

    def test_a_target_dir_given_keeps_the_client_beside_it(self):
        target = self.target.parent / 'given'
        self.assertEqual(dev.stage_target(dev.Check('c', 'client'), {}, ROOT, target), pathlib.Path(f'{target}-client'))
        self.assertEqual(dev.stage_target(dev.Check('h', 'host'), {}, ROOT, target), target)
        self.assertEqual(dev.stage_target(dev.Check('a', 'gradle'), {}, ROOT, target), pathlib.Path(f'{target}-gradle'))
        self.assertIsNone(dev.stage_target(dev.Check('a', 'gradle'), {}, ROOT, None))

    @unittest.skipUnless(CARGO, 'cargo')
    def test_cargo_reuses_another_checkouts_build_unless_the_target_is_claimed(self):
        # The 2026-11-18 failure in miniature: checkout b adds Y to its library, but its
        # library files are older than checkout a's last build in the shared target dir.
        base = self.target.parent
        target = base / 'shared-target'
        def checkout(name, lib, app, age):
            root = base / name
            for rel, text in (('Cargo.toml', '[workspace]\nmembers = ["app", "lib"]\nresolver = "2"\n'),
                              ('lib/Cargo.toml', '[package]\nname = "lib"\nversion = "0.1.0"\nedition = "2021"\n'),
                              ('lib/src/lib.rs', lib),
                              ('app/Cargo.toml', '[package]\nname = "app"\nversion = "0.1.0"\nedition = "2021"\n'
                                                 '[dependencies]\nlib = { path = "../lib" }\n'),
                              ('app/src/main.rs', app)):
                (root / rel).parent.mkdir(parents=True, exist_ok=True)
                (root / rel).write_text(text)
            for git in (['init', '-q'], ['add', '-A']):
                dev.subprocess.run(['git', '-C', str(root), *git], check=True, capture_output=True)
            stamp = dev.time.time() - age
            os.utime(root / 'lib/src/lib.rs', (stamp, stamp))
            return root
        def check(root):
            return dev.subprocess.run([CARGO, 'check', '-q', '--offline', '--target-dir', str(target)], cwd=root,
                                      capture_output=True, text=True, env=dict(os.environ, CARGO_INCREMENTAL='0'))
        a = checkout('a', 'pub const X: u32 = 1;\n', 'fn main() { let _ = lib::X; }\n', 0)
        b = checkout('b', 'pub const X: u32 = 1;\npub const Y: u32 = 2;\n', 'fn main() { let _ = lib::Y; }\n', 3600)
        self.assertEqual(dev.claim_target(target, a), 0)
        self.assertEqual(check(a).returncode, 0)
        # b's application changed after a's build, its library long before: Cargo rebuilds the
        # application against a's library (left alone, both would pass as fresh, silently).
        later = dev.time.time() + 10
        os.utime(b / 'app/src/main.rs', (later, later))
        stale = check(b)
        self.assertNotEqual(stale.returncode, 0, 'Cargo now judges path packages by content; claim_target may be moot')
        self.assertIn('Y', stale.stderr)
        self.assertEqual(dev.claim_target(target, b), 2)  # lib and app lose their fingerprints
        fresh = check(b)
        self.assertEqual(fresh.returncode, 0, fresh.stderr)

    def test_a_pool_slot_records_this_checkout_for_rl_quick(self):
        pool = self.target.parent / 'pool'
        slot = dev.build_lock.HandleSlot(pool / '2.lock')
        self.assertTrue(slot.try_take('test'))
        self.addCleanup(slot.release)
        target = dev.stage_target(dev.Check('h', 'host'), {'host target': slot}, ROOT)
        self.assertEqual(target, pool / '2')
        self.assertTrue(dev.same_path((pool / '2.owner').read_text(), ROOT))


class EnvironmentAndNextest(unittest.TestCase):
    def _fake_sdk(self, base):
        cargo_bin = base / 'cargo/bin'
        msys = base / 'msys/ucrt64'
        pyrowave = base / 'sdk/pyrowave'
        for path in (
            cargo_bin / 'cargo.exe',
            msys / 'bin/gcc.exe',
            msys / 'bin/ld.lld.exe',
            msys / 'bin/python.exe',
            pyrowave / 'bin/libpyrowave-shared-1.dll',
            pyrowave / 'share/pyrowave-shared/build-info.txt',
        ):
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text('')
        (msys / 'include/vulkan').mkdir(parents=True, exist_ok=True)
        return cargo_bin.parent, msys.parent, pyrowave

    def test_host_env_validates_required_paths_and_sets_sdk_vars(self):
        with tempfile.TemporaryDirectory() as directory:
            base = pathlib.Path(directory)
            cargo_home, msys_root, pyrowave = self._fake_sdk(base)
            target = base / 'target'
            with mock.patch.dict(os.environ, {
                'CARGO_HOME': str(cargo_home),
                'RUBYLIGHT_MSYS_ROOT': str(msys_root),
                'BUTTERPOLLO_PYROWAVE_ROOT': str(pyrowave),
                'PATH': f'{msys_root / "ucrt64/bin"};{os.environ.get("PATH", "")}',
                'NIGHTJAR_ALLOW_UNTRANSLATED': '1',
                'RUSTUP_TOOLCHAIN': 'stable',
            }, clear=False):
                os.environ.pop('RUSTFLAGS', None)
                os.environ.pop('CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUSTFLAGS', None)
                env = dev.host_env(target, lock_directory=base / 'locks', fast=False)
                self.assertEqual(env['LIBCLANG_PATH'], str(msys_root / 'ucrt64/bin'))
                self.assertEqual(env['BUTTERPOLLO_VULKAN_INCLUDE'], str(msys_root / 'ucrt64/include'))
                self.assertEqual(env['BUTTERPOLLO_PYROWAVE_ROOT'], str(pyrowave))
                self.assertEqual(env['CARGO_TARGET_DIR'], str(target))
                self.assertEqual(env['RUBYLIGHT_LOCK_DIRECTORY'], str((base / 'locks').resolve()))
                self.assertNotIn('NIGHTJAR_ALLOW_UNTRANSLATED', env)
                self.assertNotIn('RUSTFLAGS', env)
                if os.name == 'nt':
                    self.assertEqual(env['RUSTUP_TOOLCHAIN'], dev.HOST_TOOLCHAIN)
                    self.assertIn('-Clink-arg=-fuse-ld=lld', env['CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUSTFLAGS'])
                # Python 3.13 (preferred_python parent) and cargo_bin must precede ucrt64\bin on PATH.
                parts = [dev.path_key(p) for p in env['PATH'].split(os.pathsep)]
                sys_dir = dev.path_key(pathlib.Path(dev.preferred_python()).parent)
                ucrt_dir = dev.path_key(msys_root / 'ucrt64/bin')
                cargo_dir = dev.path_key(cargo_home / 'bin')
                self.assertLess(parts.index(sys_dir), parts.index(ucrt_dir))
                self.assertLess(parts.index(cargo_dir), parts.index(ucrt_dir))

                if os.name == 'nt':
                    (msys_root / 'ucrt64/bin/gcc.exe').unlink()
                    with self.assertRaisesRegex(RuntimeError, 'rl-env: .*gcc.exe is missing'):
                        dev.host_env(target)

    def test_gnu_lld_isolation_and_service_rustflags_stripping(self):
        with tempfile.TemporaryDirectory() as directory:
            base = pathlib.Path(directory)
            cargo_home, msys_root, _ = self._fake_sdk(base)
            with mock.patch.dict(os.environ, {
                'CARGO_HOME': str(cargo_home),
                'RUBYLIGHT_MSYS_ROOT': str(msys_root),
                'RUSTFLAGS': '--cfg external',
                'CARGO_ENCODED_RUSTFLAGS': 'abc',
                'CARGO_BUILD_RUSTFLAGS': 'def',
                'CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUSTFLAGS': '-Copt-level=2',
            }):
                c_env = dev.client_env(base / 't', fast=True)
                i_env = dev.interop_env(base / 't', fast=False)
                s_env = dev.service_env(base / 't', fast=False)
                self.assertEqual(c_env['NIGHTJAR_ALLOW_UNTRANSLATED'], '1')
                self.assertNotIn('NIGHTJAR_ALLOW_UNTRANSLATED', i_env)
                if os.name == 'nt':
                    self.assertEqual(c_env['CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUSTFLAGS'],
                                     '-Copt-level=2 --cfg external -Clink-arg=-fuse-ld=lld')
                    self.assertEqual(i_env['CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUSTFLAGS'],
                                     '-Copt-level=2 --cfg external -Clink-arg=-fuse-ld=lld')
                    self.assertNotIn('RUSTFLAGS', c_env)
                    self.assertNotIn('CARGO_ENCODED_RUSTFLAGS', c_env)
                    self.assertNotIn('RUSTFLAGS', i_env)
                    self.assertNotIn('CARGO_ENCODED_RUSTFLAGS', i_env)
                for forbidden in ('RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'CARGO_BUILD_RUSTFLAGS',
                                  'CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUSTFLAGS',
                                  'RUSTC_WRAPPER', 'CARGO_BUILD_RUSTC_WRAPPER', 'CARGO_INCREMENTAL',
                                  'CARGO_PROFILE_DEV_DEBUG'):
                    self.assertNotIn(forbidden, s_env)

            # Without ld.lld on PATH, GNU RUSTFLAGS is left alone; if -fuse-ld= already set, no duplicate.
            with mock.patch.object(dev.shutil, 'which', return_value=None):
                env = dev.apply_gnu_lld({'PATH': str(base)})
                self.assertNotIn('CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUSTFLAGS', env)
            if os.name == 'nt':
                with mock.patch.object(dev.shutil, 'which', return_value='ld.lld.exe'):
                    env = dev.apply_gnu_lld({'PATH': str(base),
                                             'CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUSTFLAGS': '-Clink-arg=-fuse-ld=bfd'})
                    self.assertEqual(env['CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUSTFLAGS'], '-Clink-arg=-fuse-ld=bfd')

    def test_apply_sccache_wires_wrapper_and_respects_disable_or_existing(self):
        with mock.patch.object(dev.shutil, 'which', return_value=r'C:\bin\sccache.exe'):
            env = dev.apply_sccache({'PATH': r'C:\bin'})
            self.assertEqual(env['RUSTC_WRAPPER'], r'C:\bin\sccache.exe')
            self.assertEqual(env['CARGO_INCREMENTAL'], '0')
            disabled = dev.apply_sccache({'PATH': r'C:\bin', 'RUBYLIGHT_NO_SCCACHE': '1'})
            self.assertNotIn('RUSTC_WRAPPER', disabled)
            kept = dev.apply_sccache({'PATH': r'C:\bin', 'RUSTC_WRAPPER': 'custom-wrapper'})
            self.assertEqual(kept['RUSTC_WRAPPER'], 'custom-wrapper')
            self.assertNotIn('CARGO_INCREMENTAL', kept)

    def test_gradle_command_reuses_daemon_locally_and_disables_on_ci(self):
        with mock.patch.dict(os.environ, {}, clear=False):
            os.environ.pop('CI', None)
            os.environ.pop('RUBYLIGHT_GRADLE_NO_DAEMON', None)
            os.environ.pop('RUBYLIGHT_GRADLE_TEST_FORKS', None)
            with mock.patch.object(dev.build_lock, 'total_memory_gb', return_value=64.0), \
                    mock.patch.object(dev.os, 'cpu_count', return_value=16):
                local_cmd = dev.gradle_command(ROOT, [])
                self.assertNotIn('--no-daemon', local_cmd)
                self.assertIn('--max-workers=4', local_cmd)
                self.assertIn('-PunitTestForks=6', local_cmd)
        with mock.patch.dict(os.environ, {'CI': 'true'}):
            ci_cmd = dev.gradle_command(ROOT, [])
            self.assertIn('--no-daemon', ci_cmd)
            self.assertIn('--max-workers=2', ci_cmd)
            self.assertFalse(any(a.startswith('-PunitTestForks=') for a in ci_cmd))
        with mock.patch.dict(os.environ, {'RUBYLIGHT_GRADLE_NO_DAEMON': '1'}):
            self.assertIn('--no-daemon', dev.gradle_command(ROOT, []))

    def test_discover_unittest_targets_finds_direct_inherited_and_async_test_cases(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            suite_dir = root / 'suites'
            suite_dir.mkdir()
            (suite_dir / 'test_sample.py').write_text(
                'import unittest\n'
                'class Base(unittest.TestCase):\n'
                '    def setUp(self): pass\n'
                'class Child(Base):\n'
                '    def test_ok(self): pass\n'
                'class AsyncCase(unittest.IsolatedAsyncioTestCase):\n'
                '    async def test_async(self): pass\n'
                'class Helper: pass\n',
                encoding='utf-8',
            )
            targets = dev._discover_unittest_targets(root, 'suites', ['test_*.py'])
            self.assertEqual(targets, ['test_sample.AsyncCase', 'test_sample.Child'])

    def test_apply_gnu_lld_merges_caller_rustflags_and_handles_fuse_ld_conflict(self):
        if os.name != 'nt':
            self.skipTest('Windows GNU lld only')
        with mock.patch.object(dev.shutil, 'which', return_value='ld.lld.exe'):
            env = dev.apply_gnu_lld({
                'PATH': r'C:\msys64\ucrt64\bin',
                'RUSTFLAGS': '-C debuginfo=0',
                'CARGO_ENCODED_RUSTFLAGS': 'encoded',
            })
            self.assertEqual(env['CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUSTFLAGS'],
                             '-C debuginfo=0 -Clink-arg=-fuse-ld=lld')
            self.assertNotIn('RUSTFLAGS', env)
            self.assertNotIn('CARGO_ENCODED_RUSTFLAGS', env)

            conflict = dev.apply_gnu_lld({
                'PATH': r'C:\msys64\ucrt64\bin',
                'RUSTFLAGS': '-Clink-arg=-fuse-ld=bfd',
                'CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUSTFLAGS': '-Clink-arg=-fuse-ld=lld',
            })
            self.assertEqual(conflict['CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUSTFLAGS'],
                             '-Clink-arg=-fuse-ld=lld')
            self.assertNotIn('RUSTFLAGS', conflict)

    def test_preferred_python_escapes_msys64_and_puts_windows_python_first(self):
        if os.name != 'nt':
            self.skipTest('Windows PATH protection only')
        with tempfile.TemporaryDirectory() as directory:
            base = pathlib.Path(directory)
            msys_bin = base / 'msys64' / 'ucrt64' / 'bin'
            msys_bin.mkdir(parents=True)
            msys_py = msys_bin / 'python.exe'
            msys_py.write_text('')
            local = base / 'local'
            win_py = local / 'Programs' / 'Python' / 'Python313' / 'python.exe'
            win_py.parent.mkdir(parents=True)
            win_py.write_text('')
            alt_dir = base / 'alt_py'
            alt_dir.mkdir()
            alt_py = alt_dir / 'python.exe'
            alt_py.write_text('')

            with mock.patch.object(dev.sys, 'executable', str(msys_py)), \
                    mock.patch.dict(os.environ, {'LOCALAPPDATA': str(local), 'PATH': f'{msys_bin};{alt_dir}'}):
                self.assertEqual(dev.preferred_python(), str(win_py))
                env = dev.tool_env()
                parts = [dev.path_key(p) for p in env['PATH'].split(os.pathsep)]
                self.assertLess(parts.index(dev.path_key(win_py.parent)),
                                parts.index(dev.path_key(msys_bin)))
                self.assertEqual(parts[0], dev.path_key(win_py.parent))
                with mock.patch.object(dev, 'run', return_value=0) as run_mock:
                    self.assertTrue(dev.execute(dev.Check('code-map', 'code_map', ['docs/code-map.md']),
                                                ROOT, base / 'cm.log', None))
                self.assertEqual(run_mock.call_args.args[0][0], str(win_py))

            # Fallback to non-MSYS2 PATH entry when LOCALAPPDATA Python313 is absent.
            with mock.patch.object(dev.sys, 'executable', str(msys_py)), \
                    mock.patch.dict(os.environ, {'LOCALAPPDATA': str(base / 'empty'), 'PATH': f'{msys_bin};{alt_dir}'}):
                self.assertEqual(os.path.normcase(dev.preferred_python()), os.path.normcase(str(alt_py)))

    def test_windows_short_8_3_and_long_paths_normalize_consistently(self):
        if os.name != 'nt':
            self.skipTest('Windows 8.3 short path normalization only')
        import ctypes
        with tempfile.TemporaryDirectory() as directory:
            long_base = (pathlib.Path(directory) / 'RunnerAdminLongProfile').resolve()
            long_base.mkdir(parents=True)
            buf = ctypes.create_unicode_buffer(32768)
            n = ctypes.windll.kernel32.GetShortPathNameW(str(long_base), buf, len(buf))
            if not (0 < n < len(buf)) or '~' not in buf.value:
                self.skipTest('8.3 short names are disabled on this volume')
            short_base = pathlib.Path(buf.value)
            self.assertTrue(dev.same_path(short_base, long_base))
            self.assertTrue(dev.same_path(short_base / 'missing-leaf', long_base / 'missing-leaf'))
            self.assertTrue(dev.same_path(short_base / 'nonexistent' / '..', long_base))
            self.assertTrue(dev.same_path(short_base / 'nonexistent' / '..' / 'missing-leaf', long_base / 'missing-leaf'))
            self.assertTrue(dev.same_path('', '   '))
            self.assertEqual(dev.path_key(''), '.')

            msys_bin = short_base / 'msys64' / 'ucrt64' / 'bin'
            msys_bin.mkdir(parents=True)
            msys_py = msys_bin / 'python.exe'
            msys_py.write_text('')
            local_short = short_base / 'local'
            win_py_short = local_short / 'Programs' / 'Python' / 'Python313' / 'python.exe'
            win_py_short.parent.mkdir(parents=True)
            win_py_short.write_text('')
            win_py_long_dir = win_py_short.parent.resolve()
            self.assertNotEqual(str(win_py_short.parent).lower(), str(win_py_long_dir).lower())

            with mock.patch.object(dev.sys, 'executable', str(msys_py)), \
                    mock.patch.dict(os.environ, {
                        'LOCALAPPDATA': str(local_short),
                        'PATH': f'{msys_bin};{win_py_long_dir}',
                    }):
                env = dev.tool_env()
                raw_parts = env['PATH'].split(os.pathsep)
                norm_parts = [dev.path_key(p) for p in raw_parts]
                self.assertEqual(norm_parts[0], dev.path_key(win_py_short.parent))
                self.assertEqual(norm_parts[0], dev.path_key(win_py_long_dir))
                self.assertEqual(norm_parts.count(norm_parts[0]), 1)
                self.assertLess(norm_parts.index(dev.path_key(win_py_short.parent)),
                                norm_parts.index(dev.path_key(msys_bin)))

            winapps_long = long_base / 'Microsoft' / 'WindowsApps'
            winapps_long.mkdir(parents=True)
            (winapps_long / 'python.exe').write_text('')
            n_wa = ctypes.windll.kernel32.GetShortPathNameW(str(winapps_long), buf, len(buf))
            if 0 < n_wa < len(buf) and '~' in buf.value:
                winapps_short = pathlib.Path(buf.value)
                self.assertTrue(dev._is_windows_apps_path(winapps_short))
                self.assertTrue(dev._is_windows_apps_path(winapps_short / 'python.exe'))
                alt_dir = long_base / 'AltPython'
                alt_dir.mkdir()
                alt_py = alt_dir / 'python.exe'
                alt_py.write_text('')
                with mock.patch.object(dev.sys, 'executable', str(msys_py)), \
                        mock.patch.dict(os.environ, {
                            'LOCALAPPDATA': str(short_base / 'empty-local'),
                            'PATH': f'{winapps_short};{msys_bin};{alt_dir}',
                        }):
                    self.assertTrue(dev.same_path(dev.preferred_python(), alt_py))

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

    def test_cargo_json_messages_write_rendered_text_not_raw_json(self):
        with tempfile.TemporaryDirectory() as directory:
            log = pathlib.Path(directory, 'clippy.log')
            script = pathlib.Path(directory, 'fake_cargo.py')
            script.write_text(
                'import json, sys\n'
                'print(json.dumps({"reason": "compiler-artifact", "target": {"name": "x"}}))\n'
                'print(json.dumps({"reason": "compiler-message", "message": {"level": "error", "code": {"code": "E0425"}, '
                '"message": "cannot find value `x`", "spans": [{"file_name": "rust/core/src/lib.rs", "line_start": 12, "column_start": 5, "is_primary": True}], '
                '"rendered": "error[E0425]: cannot find value `x`\\n --> rust/core/src/lib.rs:12:5\\n"}}))\n'
                'sys.exit(1)\n',
                encoding='utf-8',
            )
            code = dev.run([sys.executable, str(script)], log, directory, json_messages=True)
            self.assertEqual(code, 1)
            text = log.read_text(encoding='utf-8')
            self.assertIn('error[E0425]: cannot find value `x`', text)
            self.assertIn('rust/core/src/lib.rs:12:5', text)
            self.assertNotIn('"compiler-artifact"', text)
            self.assertNotIn('"compiler-message"', text)

    def test_host_execution_runs_fmt_clippy_norun_and_brackets_tests_with_user_idle(self):
        with tempfile.TemporaryDirectory() as directory:
            base = pathlib.Path(directory)
            log = base / 'host.log'
            target = base / 'target'
            calls = []
            def fake_run(cmd, log_path, cwd, env=None, shell=False, json_messages=False):
                calls.append((cmd, json_messages))
                return 1 if cmd[:3] == ['cargo', 'nextest', 'run'] and '--no-run' not in cmd else 0
            with mock.patch.object(dev, 'host_env', return_value={'PATH': ''}), \
                    mock.patch.object(dev, 'has_nextest', return_value=True), \
                    mock.patch.object(dev, 'run', side_effect=fake_run), \
                    mock.patch.object(dev, 'start_user_idle', return_value=('100.0', '50.0')) as idle_begin, \
                    mock.patch.object(dev, 'end_user_idle') as idle_end:
                ok = dev.execute(dev.Check('host', 'host', ['butterpollo-core']), ROOT, log, target)
            self.assertFalse(ok)
            idle_begin.assert_called_once()
            idle_end.assert_called_once_with(ROOT, {'PATH': ''}, ('100.0', '50.0'))
            self.assertEqual([c[0][:2] for c in calls], [
                ['cargo', 'fmt'],
                ['cargo', 'clippy'],
                ['cargo', 'nextest'],
                ['cargo', 'nextest'],
            ])
            self.assertTrue(calls[1][1])
            self.assertIn('--message-format=json', calls[1][0])
            self.assertIn('--no-run', calls[2][0])
            self.assertNotIn('--no-run', calls[3][0])
            self.assertIn('--config-file', calls[3][0])

    def test_nextest_fallback_to_cargo_test_and_client_doctests(self):
        with tempfile.TemporaryDirectory() as directory:
            base = pathlib.Path(directory)
            log = base / 'client.log'
            cmds_nextest = dev.client_commands(ROOT, ['nightjar-core', 'nightjar-desktop'], use_nextest=True)
            self.assertEqual(cmds_nextest[0][:3], ['cargo', 'nextest', 'run'])
            self.assertIn('--config-file', cmds_nextest[0])
            self.assertEqual(cmds_nextest[1][:3], ['cargo', 'test', '--doc'])
            self.assertEqual(cmds_nextest[2][:2], ['cargo', 'clippy'])
            # When cargo-nextest is absent, host and client fall back to cargo test cleanly.
            with mock.patch.object(dev, 'host_env', return_value={'PATH': ''}), \
                    mock.patch.object(dev, 'has_nextest', return_value=False), \
                    mock.patch.object(dev, 'run', return_value=0) as run_mock, \
                    mock.patch.object(dev, 'start_user_idle', return_value=None):
                self.assertTrue(dev.execute(dev.Check('host', 'host', ['butterpollo-core']), ROOT, log, base / 't'))
            self.assertEqual([c.args[0][:2] for c in run_mock.call_args_list], [
                ['cargo', 'fmt'],
                ['cargo', 'clippy'],
                ['cargo', 'test'],
                ['cargo', 'test'],
            ])

    def test_nextest_config_profiles_keep_strict_default_and_ci_headroom(self):
        cfg = dev.tomllib.loads((ROOT / dev.NEXTEST_CONFIG).read_text(encoding='utf-8'))
        default_profile = cfg['profile']['default']
        ci_profile = cfg['profile']['ci']
        self.assertEqual(default_profile['slow-timeout'], {'period': '15s', 'terminate-after': 4})
        self.assertFalse(ci_profile['fail-fast'])
        ci_timeout = ci_profile['slow-timeout']
        period_s = int(ci_timeout['period'].rstrip('s'))
        self.assertGreaterEqual(period_s * ci_timeout['terminate-after'], 180)


class Execution(unittest.TestCase):
    def test_failed_client_tests_do_not_start_clippy(self):
        with tempfile.TemporaryDirectory() as directory, \
                mock.patch.object(dev, 'has_nextest', return_value=False), \
                mock.patch.object(dev, 'run', return_value=1) as run:
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

    def test_parallel_lanes_run_concurrently_and_keep_deterministic_summary_order(self):
        with tempfile.TemporaryDirectory() as directory:
            checks_list = [
                dev.Check('web', 'web', ['check']),
                dev.Check('generated-docs', 'generated', ['client/docs/settings.md']),
                dev.Check('code-map', 'code_map', ['docs/code-map.md']),
            ]
            barrier = threading.Barrier(3, timeout=3)
            def fake_execute(check, *args, **kwargs):
                barrier.wait()
                return True
            out = io.StringIO()
            with mock.patch.object(dev, 'plan', return_value=(checks_list, [])), \
                    mock.patch.object(dev, 'stage_groups', return_value=[]), \
                    mock.patch.object(dev, 'execute', side_effect=fake_execute), \
                    mock.patch.object(dev, 'git', return_value='deadbeef\n'), \
                    contextlib.redirect_stdout(out):
                code = dev.main(['check', '--paths', 'rust/web/src/App.svelte', '--log-dir', directory])
            self.assertEqual(code, 0, out.getvalue())
            summary = json.loads(next(pathlib.Path(directory).glob('*/summary.json')).read_text())
            self.assertEqual([c['name'] for c in summary['checks']], ['web', 'generated-docs', 'code-map'])

    def test_fast_mode_plan_and_env_gating(self):
        planned, _ = dev.plan([
            'rust/core/session/src/lib.rs',
            'rust/web/src/App.svelte',
            'client/app/src/main/java/com/limelight/AppView.java',
            'rust/release/e2e_result.py',
        ], ROOT, fast=True)
        by_name = {c.name: c for c in planned}
        self.assertIn('host', by_name)
        self.assertEqual(by_name['web'].items, ['check'])
        self.assertTrue(by_name['app'].skip)
        self.assertTrue(by_name['release-py'].skip)
        self.assertIn('code-map', by_name)
        self.assertIn('generated-docs', by_name)
        self.assertIn('arch-check', by_name)
        self.assertFalse(by_name['arch-check'].skip)

        with tempfile.TemporaryDirectory() as directory:
            fake_root = pathlib.Path(directory)
            (fake_root / 'tools').mkdir()
            (fake_root / 'tools/arch_check.py').write_text('pass\n')
            with mock.patch.object(dev, 'workspace', return_value=({}, set())):
                fast_checks, _ = dev.plan(['tools/dev.py'], fake_root, fast=True)
            arch = next(c for c in fast_checks if c.name == 'arch-check')
            self.assertFalse(arch.skip)

        with mock.patch.dict(os.environ, {'NIGHTJAR_ALLOW_UNTRANSLATED': '1'}):
            self.assertEqual(dev.tool_env(fast=True)['NIGHTJAR_ALLOW_UNTRANSLATED'], '1')
            self.assertNotIn('NIGHTJAR_ALLOW_UNTRANSLATED', dev.tool_env(fast=False))

    def test_failure_digest_extracts_locations_and_caps_at_40_lines(self):
        with tempfile.TemporaryDirectory() as directory:
            base = pathlib.Path(directory)
            cases = [
                ('host', 'error[E0308]: mismatched types\n  --> rust/core/src/lib.rs:42:9\n'
                         'FAIL [ 0.12s] butterpollo_core::tests my_failing_test\n'
                         'Diff in rust/host/src/main.rs at line 7:\n',
                 ('rust/core/src/lib.rs:42:9: error[E0308]: mismatched types',
                  'FAIL [ 0.12s] butterpollo_core::tests my_failing_test',
                  'rust/host/src/main.rs:7:1: rustfmt diff')),
                ('gradle', 'client/app/src/main/java/A.java:15: error: cannot find symbol\n'
                           'e: file:///C:/client/B.kt:22:3 unresolved reference\n'
                           'com.limelight.GameQualityTest > testScale FAILED\n',
                 ('client/app/src/main/java/A.java:15:1: error: cannot find symbol',
                  'C:/client/B.kt:22:3: error: unresolved reference',
                  'com.limelight.GameQualityTest > testScale FAILED')),
                ('web', 'src/App.svelte:19:4\nError: Type string is not assignable\n✖ renders host list\n',
                 ('src/App.svelte:19:4: Error: Type string is not assignable',
                  '✖ renders host list')),
                ('tools', 'FAIL: test_broken (tools.test_dev.Plan)\n  File "tools/test_dev.py", line 88, in test_broken\n'
                          'tools/code_map.py:12:1: stale link\n',
                 ('tools/test_dev.py:88:1: FAIL: test_broken (tools.test_dev.Plan)',
                  'tools/code_map.py:12:1: stale link')),
            ]
            for kind, text, expected in cases:
                extracted = dev.extract_failure_lines(kind, text)
                for needle in expected:
                    self.assertIn(needle, extracted)

            noisy_log = base / 'host.log'
            noisy_log.write_text('\n'.join(f'error: err {i}\n  --> rust/core/src/f.rs:{i}:1' for i in range(1, 60)),
                                 encoding='utf-8')
            digest = dev.format_failure_digest([(dev.Check('host', 'host'), noisy_log)])
            self.assertEqual(len(digest), 40)
            self.assertIn('more diagnostic lines', digest[-1])

    def test_extract_failure_lines_preserves_windows_drive_letter_paths(self):
        host_log = (
            "thread 'suite::panic_test_2' panicked at C:\\src\\rubylight-sprint\\rust\\core\\src\\abs.rs:44:5:\n"
            "assertion failed: x == y\n"
            "thread 'suite::old_panic' panicked at 'explicit boom', C:\\src\\rubylight-sprint\\rust\\core\\src\\abs.rs:45:6\n"
            "error[E0308]: mismatched types\n"
            "  --> C:\\src\\rubylight-sprint\\rust\\core\\src\\abs.rs:46:7\n"
        )
        host_extracted = dev.extract_failure_lines('host', host_log)
        self.assertIn(r'C:\src\rubylight-sprint\rust\core\src\abs.rs:44:5: panicked: assertion failed: x == y', host_extracted)
        self.assertIn(r'C:\src\rubylight-sprint\rust\core\src\abs.rs:45:6: panicked: explicit boom', host_extracted)
        self.assertIn(r'C:\src\rubylight-sprint\rust\core\src\abs.rs:46:7: error[E0308]: mismatched types', host_extracted)
        for kind in ('python', 'tools', 'arch', 'code_map', 'generated'):
            extracted = dev.extract_failure_lines(kind, r'C:\src\rubylight-sprint\tools\foo.py:12:3: error')
            self.assertIn(r'C:\src\rubylight-sprint\tools\foo.py:12:3: error', extracted)

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

    def test_app_tests_run_by_class_or_test_list_file_never_dropping_filters(self):
        command = dev.gradle_command(ROOT, ['com.limelight.GameQualityTest'])
        self.assertEqual(command[-2:], ['--tests', 'com.limelight.GameQualityTest'])
        self.assertNotIn(dev.GRADLE_LINT_TASK, command)
        empty = dev.gradle_command(ROOT, [])
        self.assertEqual(empty[-2:], [dev.GRADLE_TASK, dev.GRADLE_LINT_TASK])

        # 170 classes exceed cmd's 8191-char limit: gradle_command emits -PrubylightTestList=<tempfile>.
        many = [f'com.limelight.binding.video.MediaCodecSomethingLongerTest{i}' for i in range(170)]
        with self.assertRaises(ValueError):
            dev.gradle_command(ROOT, many)
        with tempfile.TemporaryDirectory() as directory:
            list_path = pathlib.Path(directory) / 'tests.txt'
            command = dev.gradle_command(ROOT, many, test_list_path=list_path)
            self.assertNotIn('--tests', command)
            self.assertEqual(command[-2:], [dev.GRADLE_TASK, f'-PrubylightTestList={list_path}'])
            self.assertEqual(list_path.read_text(encoding='utf-8').splitlines(), many)
            self.assertLessEqual(len(dev.subprocess.list2cmdline(command)), dev.CMD_LINE_LIMIT)
        few = many[:40]
        self.assertIn('--tests', dev.gradle_command(ROOT, few))
        with mock.patch.dict(os.environ, {'GRADLE_OPTS': '-Dx=' + 'y' * 6000}):
            with self.assertRaises(ValueError):
                dev.gradle_command(ROOT, few)

    def test_long_app_class_list_writes_temp_list_and_cleans_up_on_success_and_failure(self):
        many = [f'com.limelight.binding.video.MediaCodecSomethingLongerTest{i}' for i in range(170)]
        for exit_code in (0, 1):
            seen_path = None
            seen_lines = None
            def fake_run(cmd, *args, **kwargs):
                nonlocal seen_path, seen_lines
                arg = next(a for a in cmd if a.startswith('-PrubylightTestList='))
                seen_path = pathlib.Path(arg.split('=', 1)[1])
                seen_lines = seen_path.read_text(encoding='utf-8').splitlines()
                return exit_code
            with tempfile.TemporaryDirectory() as directory, \
                    mock.patch.object(dev, '_ensure_gradle_daemon', return_value=True), \
                    mock.patch.object(dev, 'run', side_effect=fake_run):
                ok = dev.execute(dev.Check('app', 'gradle', many), ROOT,
                                 pathlib.Path(directory) / 'check.log', pathlib.Path(directory))
            self.assertEqual(ok, exit_code == 0)
            self.assertEqual(seen_lines, many)
            self.assertFalse(seen_path.exists())

    def test_gradle_lane_normalizes_long_windows_temp_and_passes_java_io_tmpdir(self):
        if os.name != 'nt':
            self.skipTest('Windows MAX_PATH temp normalization only')
        with tempfile.TemporaryDirectory() as directory:
            base = pathlib.Path(directory)
            short_local = base / 'l'
            short_temp = short_local / 'Temp'
            short_temp.mkdir(parents=True)
            long_temp = base / ('a' * 50) / 'antigravity' / 'run-123456'
            long_temp.mkdir(parents=True)

            # Long TEMP/TMP (> 45 chars) normalizes to shorter existing LOCALAPPDATA\Temp and passes -Djava.io.tmpdir=.
            with mock.patch.dict(os.environ, {
                'TEMP': str(long_temp),
                'TMP': str(long_temp),
                'LOCALAPPDATA': str(short_local),
                'CARGO_TARGET_DIR': str(base / 'target'),
            }):
                env = dev.gradle_env()
                self.assertEqual(env['TEMP'], str(short_temp))
                self.assertEqual(env['TMP'], str(short_temp))
                self.assertNotIn('CARGO_TARGET_DIR', env)
                explicit = dev.gradle_env(target=base / 'gradle-rust')
                self.assertEqual(explicit['CARGO_TARGET_DIR'], str(base / 'gradle-rust'))

                seen = {}
                def fake_run(cmd, cwd_or_log, cwd, run_env=None, **kwargs):
                    seen['cmd'] = cmd
                    seen['env'] = run_env
                    return 0
                with mock.patch.object(dev, '_ensure_gradle_daemon', return_value=True), \
                        mock.patch.object(dev, 'run', side_effect=fake_run):
                    ok = dev.execute(dev.Check('app', 'gradle', ['com.limelight.GameQualityTest']),
                                     ROOT, base / 'gradle.log', None)
                self.assertTrue(ok)
                self.assertEqual(seen['env']['TEMP'], str(short_temp))
                self.assertEqual(seen['env']['TMP'], str(short_temp))
                self.assertIn(f'-Djava.io.tmpdir={short_temp}', seen['cmd'])
                self.assertLess(seen['cmd'].index(f'-Djava.io.tmpdir={short_temp}'),
                                seen['cmd'].index(dev.GRADLE_TASK))
                # If a breakaway daemon cannot be started, execute falls back to --no-daemon.
                with mock.patch.object(dev, '_ensure_gradle_daemon', return_value=False), \
                        mock.patch.object(dev, 'run', side_effect=fake_run):
                    self.assertTrue(dev.execute(dev.Check('app', 'gradle', ['com.limelight.GameQualityTest']),
                                                ROOT, base / 'gradle-fb.log', None))
                self.assertIn('--no-daemon', seen['cmd'])

            # Already short TEMP/TMP (<= 45 chars) is preserved and no -Djava.io.tmpdir override is injected.
            with mock.patch.dict(os.environ, {
                'TEMP': r'C:\T',
                'TMP': r'C:\T',
                'LOCALAPPDATA': str(short_local),
            }):
                env = dev.gradle_env()
                self.assertEqual(env['TEMP'], r'C:\T')
                self.assertEqual(env['TMP'], r'C:\T')
                with mock.patch.object(dev, '_ensure_gradle_daemon', return_value=True), \
                        mock.patch.object(dev, 'run', return_value=0) as run_mock:
                    self.assertTrue(dev.execute(dev.Check('app', 'gradle', []),
                                                ROOT, base / 'gradle-short.log', None))
                self.assertFalse(any(a.startswith('-Djava.io.tmpdir=') for a in run_mock.call_args.args[0]))

            # Long TEMP with no shorter existing directory leaves TEMP unchanged rather than inventing a missing path.
            missing_local = base / ('m' * 60)
            with mock.patch.dict(os.environ, {
                'TEMP': str(long_temp),
                'TMP': str(long_temp),
                'LOCALAPPDATA': str(missing_local),
                'USERPROFILE': str(missing_local),
            }):
                env = dev.gradle_env()
                self.assertEqual(env['TEMP'], str(long_temp))
                self.assertEqual(env['TMP'], str(long_temp))

    def test_breakaway_shared_daemons_explicitly_request_normal_priority(self):
        if os.name != 'nt':
            self.skipTest('Windows breakaway daemon creation flags only')
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            seen = []
            def fake_run(cmd, **kwargs):
                seen.append((cmd, kwargs.get('creationflags', 0)))
                return mock.Mock(returncode=0)
            with mock.patch.object(dev.subprocess, 'run', side_effect=fake_run), \
                    mock.patch.object(dev, '_has_compatible_gradle_daemon', return_value=False):
                dev._ensure_sccache_server('sccache', {})
                self.assertTrue(dev._ensure_gradle_daemon(root, {}))
            self.assertEqual(len(seen), 2)
            for _cmd, flags in seen:
                self.assertTrue(flags & dev.build_lock.NORMAL_PRIORITY_CLASS)


class HandRunCargo(unittest.TestCase):
    def cargo(self, *argv, code=0, error=None):
        """Run `dev.py cargo` with private locks and a fake cargo; what the child saw."""
        seen = {}

        def popen(command, cwd=None, env=None):
            seen.update(command=command, cwd=pathlib.Path(cwd), target=pathlib.Path(env['CARGO_TARGET_DIR']))
            if error:
                raise error
            return mock.Mock(wait=mock.Mock(return_value=code))

        def claim(lock, root=ROOT):
            seen['claimed'] = pathlib.Path(lock)
            return pathlib.Path(lock).with_suffix('')

        def env(target, **_):
            return {'CARGO_TARGET_DIR': str(target)}

        def service_env(target, **_):
            return env(pathlib.Path(target) / 'service')

        with tempfile.TemporaryDirectory() as directory, \
                mock.patch.object(dev, 'service_env', side_effect=service_env) as service, \
                mock.patch.object(dev, 'claim_slot', side_effect=claim), \
                mock.patch.object(dev, 'claim_target') as claim_target, \
                mock.patch.object(dev, 'host_env', side_effect=env), \
                mock.patch.object(dev, 'client_env', side_effect=env), \
                mock.patch.object(dev.build_lock, 'end_children_with_me'), \
                mock.patch.object(dev.build_lock, 'prune_pool'), \
                mock.patch.object(dev.shutil, 'which', return_value='cargo'), \
                mock.patch.object(dev.subprocess, 'Popen', side_effect=popen), \
                contextlib.redirect_stderr(io.StringIO()) as stderr:
            base = pathlib.Path(directory).resolve()
            pool = base / 'pool'
            options = ['--lock-directory', str(base / 'locks'), '--pool', str(pool), '--lock-timeout', '0']
            argv = [a.replace('TARGET', str(base / 'own')) for a in argv]
            try:
                seen['code'] = dev.main(['cargo', *options, *argv])
            finally:
                # Every lane is free again.
                locks = list(base.rglob('*.lock'))
                self.assertTrue(locks)
                for path in locks:
                    slot = dev.build_lock.HandleSlot(path)
                    self.assertTrue(slot.try_take('test'), path)
                    slot.release()
            seen.update(base=base, pool=pool, claim_target=claim_target.call_args, stderr=stderr.getvalue(),
                        service=service.called)
        return seen

    def test_a_host_command_builds_in_a_claimed_host_slot_and_returns_its_code(self):
        seen = self.cargo('--', 'test', '-p', 'x', '--', '--manifest-path', 'client/protocol/Cargo.toml', code=101)
        self.assertEqual(seen['code'], 101)
        self.assertEqual(seen['claimed'], seen['pool'] / '1.lock')
        self.assertEqual(seen['target'], seen['pool'] / '1')
        self.assertEqual(seen['cwd'], ROOT.resolve())
        self.assertEqual(seen['command'][1:3], ['test', '-p'])

    def test_a_client_flag_or_manifest_takes_a_client_slot(self):
        manifest = ROOT.resolve() / 'client' / 'protocol' / 'Cargo.toml'
        for argv, expected in (
                (['--client', '--', 'check'], ['check']),
                (['--', 'check', '--manifest-path', 'client/protocol/Cargo.toml'],
                 ['check', '--manifest-path', str(manifest)]),
                (['--', 'check', '--manifest-path=client/protocol/Cargo.toml'],
                 ['check', f'--manifest-path={manifest}'])):
            seen = self.cargo(*argv)
            self.assertEqual(seen['code'], 0)
            self.assertEqual(seen['claimed'], seen['pool'] / 'client-1.lock', argv)
            self.assertEqual(seen['target'], seen['pool'] / 'client-1', argv)
            self.assertEqual(seen['cwd'], ROOT.resolve() / 'client' / 'protocol')
            self.assertEqual(seen['command'][1:], expected)

    def test_target_dir_is_claimed_instead_of_a_slot(self):
        for flags, suffix in (([], ''), (['--client'], '-client')):
            seen = self.cargo(*flags, '--target-dir', 'TARGET', '--', 'build')
            target = pathlib.Path(f"{seen['base'] / 'own'}{suffix}")
            self.assertEqual(seen['target'], target)
            self.assertEqual(seen['claim_target'].args[0], target)
            self.assertNotIn('claimed', seen)

    def test_a_service_manifest_builds_as_the_service_check_does(self):
        for manifest in (['--manifest-path', 'rust/service/Cargo.toml'], ['--manifest-path=rust/service/Cargo.toml']):
            seen = self.cargo('--', 'build', *manifest)
            self.assertTrue(seen['service'])
            self.assertEqual(seen['claimed'], seen['pool'] / '1.lock')
            self.assertEqual(seen['target'], seen['pool'] / '1' / 'service')
            self.assertEqual(seen['cwd'], ROOT.resolve())
            self.assertEqual(seen['command'][1:], ['build', *manifest])
            self.assertIn(f"dev cargo: target {seen['target']}", seen['stderr'])
        self.assertFalse(self.cargo('--', 'build')['service'])

    def test_relative_wrapper_paths_are_the_checkout_roots(self):
        with mock.patch.object(dev, 'stage_groups', side_effect=RuntimeError('stop')) as groups, \
                mock.patch.object(dev.build_lock, 'end_children_with_me'), \
                contextlib.redirect_stderr(io.StringIO()):
            dev.main(['cargo', '--target-dir', 't', '--pool', 'p', '--lock-directory', 'l', '--', 'build'])
        args = groups.call_args.args[2]
        self.assertEqual([args.target_dir, args.pool, args.lock_directory],
                         [ROOT.resolve() / name for name in ('t', 'p', 'l')])

    def test_the_lanes_are_released_when_cargo_fails_to_run(self):
        seen = self.cargo('--', 'build', error=OSError('no cargo'))
        self.assertEqual(seen['code'], 1)
        self.assertIn('no cargo', seen['stderr'])
        self.assertEqual(self.cargo('--', 'build', error=KeyboardInterrupt())['code'], 130)
        with self.assertRaises(ValueError):  # anything else propagates, after the release
            self.cargo('--', 'build', error=ValueError())


if __name__ == '__main__':
    unittest.main()
