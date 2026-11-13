import json, os, pathlib, shutil, subprocess, sys, tempfile, time, unittest
from unittest import mock

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import build_lock


class BuildLease(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = pathlib.Path(temp.name)
        self.lock = self.root / 'BUILD-LOCK'
        self.environment = mock.patch.dict(os.environ)
        self.environment.start()
        self.addCleanup(self.environment.stop)
        os.environ.pop(build_lock.OWNER_ENV, None)
        os.environ.pop(build_lock.PATH_ENV, None)

    def cli(self, *command, timeout=0):
        return [sys.executable, str(pathlib.Path(build_lock.__file__)), '--lock-directory', str(self.root),
                '--timeout', str(timeout), '--', *command]

    def run_cli(self, *command, **kwargs):
        return subprocess.run(self.cli(*command), capture_output=True, text=True, timeout=20, **kwargs)

    def test_nested_commands_reuse_the_lease_and_preserve_exit_status(self):
        with build_lock.BuildLock(self.root, 'parent') as lease:
            before = self.lock.read_text()
            # The nested wrapper inherits the custom directory too, without
            # needing another --lock-directory on every child command.
            result = subprocess.run([sys.executable, build_lock.__file__, '--timeout', '0', '--',
                                     sys.executable, '-c', 'import sys; sys.exit(7)'],
                                    capture_output=True, text=True, timeout=15)
            self.assertEqual(result.returncode, 7, result.stderr)
            self.assertEqual(self.lock.read_text(), before)
        self.assertFalse(self.lock.exists())
        self.assertNotIn(build_lock.OWNER_ENV, os.environ)

    def test_unrelated_process_waits_and_names_the_holder(self):
        with build_lock.BuildLock(self.root, 'first'):
            env = dict(os.environ)
            env.pop(build_lock.OWNER_ENV)
            result = self.run_cli(sys.executable, '-c', 'raise AssertionError("must not run")', env=env)
            self.assertEqual(result.returncode, 1)
            self.assertIn('first', result.stderr)
            self.assertIn('No lock removed', result.stderr)
        self.assertFalse(self.lock.exists())

    def test_an_old_or_dead_owners_lock_is_never_removed(self):
        self.lock.write_text('old build pid=4294967')
        os.utime(self.lock, (1, 1))
        result = self.run_cli(sys.executable, '-c', 'pass')
        self.assertEqual(result.returncode, 1)
        self.assertEqual(self.lock.read_text(), 'old build pid=4294967')

    def test_waiter_runs_only_after_the_holder_releases(self):
        with build_lock.BuildLock(self.root):
            env = dict(os.environ)
            env.pop(build_lock.OWNER_ENV)
            marker = self.root / 'ran'
            child = subprocess.Popen(self.cli(sys.executable, '-c',
                'import pathlib,sys; pathlib.Path(sys.argv[1]).touch()', str(marker), timeout=10),
                env=env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
            try:
                time.sleep(.5)
                self.assertIsNone(child.poll())
                self.assertFalse(marker.exists())
            except BaseException:
                child.kill()
                child.communicate()
                raise
        _, error = child.communicate(timeout=15)
        self.assertEqual(child.returncode, 0, error)
        self.assertTrue(marker.exists())

    def test_interrupted_owner_keeps_lock_while_its_child_survives(self):
        pidfile = self.root / 'child'
        owner = subprocess.Popen(self.cli(sys.executable, '-c',
            'import os,pathlib,sys,time; pathlib.Path(sys.argv[1]).write_text(str(os.getpid())); time.sleep(60)', str(pidfile)),
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        child_pid = None
        try:
            # The file exists, empty, before the child has written its pid into it.
            deadline, written = time.monotonic() + 15, ''
            while not written and time.monotonic() < deadline:
                time.sleep(.05)
                written = pidfile.read_text() if pidfile.exists() else ''
            self.assertTrue(written, 'the child wrote no pid')
            child_pid = int(written)
            owner.kill()
            owner.wait(timeout=5)
            result = self.run_cli(sys.executable, '-c', 'pass')
            self.assertEqual(result.returncode, 1)
            self.assertTrue(self.lock.exists())
            self.assertTrue(build_lock.priority_alive(f'pid={child_pid}'))
        finally:
            if owner.poll() is None:
                owner.kill()
                owner.wait()
            if child_pid:
                if os.name == 'nt':
                    subprocess.run(['taskkill', '/PID', str(child_pid), '/F'], capture_output=True)
                else:
                    import signal
                    os.kill(child_pid, signal.SIGKILL)

    def test_live_release_priority_blocks_but_reused_pid_does_not(self):
        priority = self.root / 'PRIORITY'
        priority.write_text(f'release pid={os.getpid()}')
        self.assertEqual(self.run_cli(sys.executable, '-c', 'pass').returncode, 1)
        self.assertFalse(self.lock.exists())
        if os.name == 'nt':
            priority.write_text(f'release pid={os.getpid()} start=2001-01-01T00:00:00.0000000Z')
            result = self.run_cli(sys.executable, '-c', 'pass')
            self.assertEqual(result.returncode, 0, result.stderr)

    @unittest.skipUnless(os.name == 'nt', 'Windows delete sharing')
    def test_live_handle_prevents_legacy_expiry_deletion(self):
        with build_lock.BuildLock(self.root):
            with self.assertRaises(PermissionError):
                self.lock.unlink()

    def test_an_ownership_change_is_left_alone(self):
        with self.assertRaisesRegex(RuntimeError, 'ownership changed'):
            with build_lock.BuildLock(self.root):
                self.lock.write_text('other owner')
        self.assertEqual(self.lock.read_text(), 'other owner')

    def test_interrupt_does_not_release_until_the_child_exits(self):
        child = mock.Mock()
        child.wait.side_effect = [KeyboardInterrupt, 0]
        self.assertEqual(build_lock.wait_child(child), 0)
        self.assertEqual(child.wait.call_count, 2)

    @unittest.skipUnless(shutil.which('pwsh'), 'PowerShell 7')
    def test_powershell_builds_share_python_lease_and_observe_release_priority(self):
        driver = self.root / 'lease.ps1'
        driver.write_text("""param([string]$Locks)
$ErrorActionPreference = 'Stop'
. $env:CHECK_STREAMS
Initialize-CheckLocks $Locks
$CheckBuildDeadline = 0.3
$held = Enter-CheckBuildLock -RespectPriority
try { 'acquired' } finally { Exit-CheckLocks $held -NoPause }
""")
        env = dict(os.environ, CHECK_STREAMS=str(pathlib.Path(__file__).resolve().parents[1] / 'rust/release/check_streams.ps1'))
        def run():
            return subprocess.run(['pwsh', '-NoProfile', '-File', str(driver), '-Locks', str(self.root)],
                                  capture_output=True, text=True, env=env, timeout=15)
        with build_lock.BuildLock(self.root):
            before = self.lock.read_text()
            child = run()
            self.assertEqual(child.returncode, 0, child.stderr)
            self.assertEqual(self.lock.read_text(), before)
        (self.root / 'PRIORITY').write_text(f'release pid={os.getpid()}')
        child = run()
        self.assertEqual(child.returncode, 1)
        self.assertIn('release priority', child.stdout)
        self.assertNotIn('acquired', child.stdout)
        self.assertFalse(self.lock.exists())

    @unittest.skipUnless(shutil.which('pwsh'), 'PowerShell 7')
    def test_full_build_lease_preserves_shell_environment_and_failure(self):
        repo = self.root / 'checkout with spaces'
        (repo / 'rust').mkdir(parents=True)
        (repo / 'rust/release').mkdir()
        original = pathlib.Path(__file__).resolve().parents[1] / 'rust/build.ps1'
        # Exercise the real lease setup, substituting only the SDK/build work.
        prefix = original.read_text().split("$toolchain =", 1)[0]
        script = repo / 'rust/build.ps1'
        script.write_text(prefix + """
@{deps=$Dependencies; package=[bool]$Package; stage=$Stage; jobs=$env:CARGO_BUILD_JOBS; pyro=$env:PYROWAVE_JOBS} | ConvertTo-Json -Compress
throw 'synthetic failure'
} finally { Exit-CheckLocks $buildLease -NoPause }
""")
        shutil.copyfile(original.parent / 'release/check_streams.ps1', repo / 'rust/release/check_streams.ps1')
        env = dict(os.environ, RUBYLIGHT_LOCK_DIRECTORY=str(self.root))
        for shell in ('pwsh', 'powershell') if os.name == 'nt' else ('pwsh',):
            with self.subTest(shell=shell):
                result = subprocess.run([shell, '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', str(script), '-Dependencies', "space's and $literal", '-Package',
                                         '-Stage', 'Build', '-Jobs', '3'], env=env, capture_output=True, text=True, timeout=20)
                self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
                self.assertIn('synthetic failure', result.stderr)
                receipt = json.loads(result.stdout)
                self.assertEqual(receipt, dict(deps="space's and $literal", package=True, stage='Build', jobs='3', pyro='3'))
                self.assertFalse(self.lock.exists())


if __name__ == '__main__':
    unittest.main()
