"""Machine-wide heavy-build lease, shared with release BUILD-LOCK/PRIORITY.

python tools/build_lock.py [--timeout SECONDS] -- <command> [arguments ...]
Never expires or removes another owner's lock: an interrupted runner may have
live compiler children. Inspect those processes before manually clearing it.
"""
import argparse, ctypes, datetime, os, pathlib, re, subprocess, sys, time, uuid

DEFAULT_DIRECTORY = pathlib.Path(r'C:\src\butterpollo-release') if os.name == 'nt' else pathlib.Path('/tmp/rubylight-build')
OWNER_ENV = 'RUBYLIGHT_BUILD_LOCK_OWNER'
PATH_ENV = 'RUBYLIGHT_BUILD_LOCK_PATH'


def priority_alive(text):
    """Unknown ownership fails closed; PID reuse cannot prolong a known ticket."""
    match = re.search(r'\bpid=(\d+)', text)
    if not match:
        return bool(text)
    pid = int(match[1])
    if os.name != 'nt':
        try:
            os.kill(pid, 0)
            return True
        except ProcessLookupError:
            return False
        except PermissionError:
            return True
    from ctypes import wintypes
    kernel = ctypes.WinDLL('kernel32', use_last_error=True)
    kernel.OpenProcess.restype = wintypes.HANDLE
    kernel.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    kernel.CloseHandle.argtypes = [wintypes.HANDLE]
    kernel.GetProcessTimes.argtypes = [wintypes.HANDLE, *([ctypes.POINTER(wintypes.FILETIME)] * 4)]
    handle = kernel.OpenProcess(0x1000, False, pid)
    if not handle:
        return ctypes.get_last_error() != 87  # access denied is not proof of exit
    try:
        times = [wintypes.FILETIME() for _ in range(4)]
        if not kernel.GetProcessTimes(handle, *(ctypes.byref(t) for t in times)):
            return True
        start = re.search(r'\bstart=(\S+)', text)
        if not start:
            return True
        try:
            expected = datetime.datetime.fromisoformat(start[1].replace('Z', '+00:00')).timestamp()
        except ValueError:
            return True
        actual = ((times[0].dwHighDateTime << 32) + times[0].dwLowDateTime) / 10_000_000 - 11644473600
        return abs(actual - expected) < .001
    finally:
        kernel.CloseHandle(handle)


def read(path):
    try:
        return path.read_text(encoding='utf-8-sig')
    except FileNotFoundError:
        return None


def wait_child(child):
    """Keep the lease until the command exits, including while it handles Ctrl+C."""
    while True:
        try:
            return child.wait()
        except KeyboardInterrupt:
            continue


class BuildLock:
    def __init__(self, directory=None, label='build', timeout=1800, poll=.25):
        self.path = pathlib.Path(directory or os.environ.get('RUBYLIGHT_LOCK_DIRECTORY') or DEFAULT_DIRECTORY).resolve() / 'BUILD-LOCK'
        self.label, self.timeout, self.poll = label, timeout, poll
        self.text = None
        self.guard = None
        self.previous = {}

    def __enter__(self):
        inherited = os.environ.get(OWNER_ENV)
        if inherited and os.environ.get(PATH_ENV) == str(self.path) and read(self.path) == inherited:
            return self
        self.path.parent.mkdir(parents=True, exist_ok=True)
        deadline, last = time.monotonic() + self.timeout, None
        while True:
            priority = read(self.path.with_name('PRIORITY'))
            holder = f'release priority: {priority}' if priority and priority_alive(priority) else None
            if holder is None:
                try:
                    with self.path.open('x', encoding='utf-8') as out:
                        self.text = f'{self.label} {datetime.datetime.now(datetime.timezone.utc).isoformat()} pid={os.getpid()} token={uuid.uuid4().hex}'
                        out.write(self.text)
                    # Python's Windows read handle denies deletion. Older age-based
                    # waiters cannot remove this lease during a long live build.
                    self.guard = self.path.open('rb')
                    break
                except FileExistsError:
                    holder = read(self.path) or '(owner is writing BUILD-LOCK)'
            if holder != last:
                print(f'BUILD-LOCK waiting: {holder}', flush=True)
                last = holder
            if time.monotonic() >= deadline:
                raise TimeoutError(f'BUILD-LOCK busy after {self.timeout:g}s: {holder}. No lock removed; inspect the owner and its children before recovery.')
            time.sleep(self.poll)
        for name, value in ((OWNER_ENV, self.text), (PATH_ENV, str(self.path)),
                            ('RUBYLIGHT_LOCK_DIRECTORY', str(self.path.parent))):
            self.previous[name] = os.environ.get(name)
            os.environ[name] = value
        return self

    def __exit__(self, *_):
        if self.guard is None:
            return
        self.guard.close()
        try:
            # A changed owner is never deleted, even after command failure.
            for attempt in range(20):
                try:
                    if read(self.path) != self.text:
                        raise RuntimeError(f'BUILD-LOCK ownership changed: {self.path}; left alone')
                    self.path.unlink()
                    break
                except PermissionError:
                    if attempt == 19:
                        raise
                    time.sleep(.05)
        finally:
            for name, value in self.previous.items():
                if value is None:
                    os.environ.pop(name, None)
                else:
                    os.environ[name] = value


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--lock-directory')
    parser.add_argument('--timeout', type=float, default=1800)
    parser.add_argument('--label', default='build')
    parser.add_argument('command', nargs=argparse.REMAINDER)
    args = parser.parse_args(argv)
    command = args.command[1:] if args.command[:1] == ['--'] else args.command
    if not command or args.timeout < 0:
        parser.error('provide a command after -- and a nonnegative timeout')
    try:
        with BuildLock(args.lock_directory, args.label, args.timeout):
            child = subprocess.Popen(command)
            return wait_child(child)
    except (OSError, RuntimeError) as error:
        print(str(error), file=sys.stderr)
        return 1


if __name__ == '__main__':
    sys.exit(main())
