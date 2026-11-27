"""Machine-wide build locks: the exclusive BUILD-LOCK and the per-stage lanes.

python tools/build_lock.py [--timeout SECONDS] -- <command> [arguments ...]
python tools/build_lock.py --lane cargo|gradle|web -- <command> [arguments ...]
python tools/build_lock.py --status | --drain | --prune [--all | --min-free-gb N] [--pool DIR]

BUILD-LOCK is exclusive: rust\\build.ps1, the release checks and this wrapper
without --lane take it, then wait until every lane is idle. A lane is one
stage's lock: tools/dev.py's host and client-protocol stages take a Cargo slot
(at most cargo_slots() at once) and a warm target slot, the app stage the
Gradle lane, the web stage the web lane. A stage takes its lanes first and then
gives them back while BUILD-LOCK or a release PRIORITY ticket is held, so either
the stage or the exclusive owner sees the other and nothing deadlocks.

Never expires or removes another owner's BUILD-LOCK: an interrupted runner may
have live compiler children. Inspect those processes before manually clearing
it. A lane is an open file handle with no sharing, which Windows closes when its
owner exits, so a crashed stage never leaves a lane taken and no lane file is
ever removed. Cargo and Gradle lock their own target and cache folders against
a surviving child.
"""
import argparse, collections, ctypes, datetime, math, os, pathlib, re, shutil, subprocess, sys, threading, time, uuid

DEFAULT_DIRECTORY = pathlib.Path(r'C:\src\butterpollo-release') if os.name == 'nt' else pathlib.Path('/tmp/rubylight-build')


def _default_fallback_directory():
    if os.name != 'nt':
        return pathlib.Path('/tmp/rubylight-build')
    local = os.environ.get('LOCALAPPDATA')
    base = pathlib.Path(local) if local else (pathlib.Path.home() / 'AppData' / 'Local')
    return base / 'ButterpolloRust' / 'locks'


_IMPORT_FALLBACK = _default_fallback_directory()
FALLBACK_DIRECTORY = _IMPORT_FALLBACK
OWNER_ENV = 'RUBYLIGHT_BUILD_LOCK_OWNER'
PATH_ENV = 'RUBYLIGHT_BUILD_LOCK_PATH'
# Lane files this process tree holds: an exclusive owner started inside a stage
# does not wait for its own parent's lanes.
LANES_ENV = 'RUBYLIGHT_LANES_HELD'
# The warm Cargo target folders, shared with C:\src\rl-quick.ps1's Enter-RlSlot
# (C:\src\rl-env.ps1): host slots <pool>\<n> with <n>.lock and <n>.owner.
POOL_ENV = 'RUBYLIGHT_TARGET_POOL'
DEFAULT_POOL = pathlib.Path(r'C:\src\rl-target-pool')
HOST_SLOTS = 8
CLIENT_SLOTS = 6
MIN_FREE_GB_ENV = 'RUBYLIGHT_POOL_MIN_FREE_GB'
DEFAULT_MIN_FREE_GB = 40.0
# Gradle runs outside the dev check take this lane through
# `build_lock.py --lock-directory <it> -- gradlew ...`; the app stage takes the same.
GRADLE_ENV = 'RUBYLIGHT_GRADLE_LANE'
DEFAULT_GRADLE_LANE = pathlib.Path(r'D:\CodexArtifacts\locks\gradle-lane')
CARGO_SLOTS_ENV = 'RUBYLIGHT_CARGO_SLOTS'
CARGO_JOBS_ENV = 'RUBYLIGHT_CARGO_JOBS'
RESERVED_THREADS_ENV = 'RUBYLIGHT_RESERVED_THREADS'
# Measured 2026-11-18 (docs/building.md, "Build locks and lanes"): a dev-check
# Cargo stage (a cold host check) runs at -j 4 and peaked at 7.0 GB of commit charge,
# 5.5 GB working set (rustc, the linker and the test binaries together).
CARGO_STAGE_THREADS = 4
CARGO_MAX_JOBS = 16
CARGO_STAGE_PEAK_GB = 7
# Left for the desktop, the user's games and streams, and Gradle.
MEMORY_RESERVE_GB = 16


def default_directory():
    if os.name != 'nt':
        return DEFAULT_DIRECTORY
    if DEFAULT_DIRECTORY.is_dir():
        return DEFAULT_DIRECTORY
    if FALLBACK_DIRECTORY != _IMPORT_FALLBACK:
        return FALLBACK_DIRECTORY
    return _default_fallback_directory()


def lock_directory(directory=None):
    if directory:
        return pathlib.Path(directory).resolve()
    env = os.environ.get('RUBYLIGHT_LOCK_DIRECTORY')
    if env:
        return pathlib.Path(env).resolve()
    return default_directory().resolve()


def machine_wide(directory):
    """Whether directory is the machine's own lock folder rather than a private (test) one."""
    return lock_directory(directory) == DEFAULT_DIRECTORY.resolve()


def pool_directory(directory=None):
    """The warm target pool: the machine's for the machine's locks, else one beside private locks."""
    if os.environ.get(POOL_ENV):
        return pathlib.Path(os.environ[POOL_ENV]).resolve()
    if machine_wide(directory) and (directory is not None or DEFAULT_POOL.is_dir()):
        return DEFAULT_POOL
    return lock_directory(directory) / 'target-pool'


def gradle_lane(directory=None):
    if os.environ.get(GRADLE_ENV):
        return pathlib.Path(os.environ[GRADLE_ENV]).resolve()
    if machine_wide(directory) and DEFAULT_GRADLE_LANE.parent.is_dir():
        return DEFAULT_GRADLE_LANE
    return lock_directory(directory) / 'gradle-lane'


def total_memory_gb():
    if os.name == 'nt':
        class Status(ctypes.Structure):
            _fields_ = [(n, ctypes.c_ulong if n in ('length', 'load') else ctypes.c_ulonglong) for n in
                        ('length', 'load', 'total', 'avail', 'page_total', 'page_avail',
                         'virtual_total', 'virtual_avail', 'extended')]
        status = Status(length=ctypes.sizeof(Status))
        if ctypes.windll.kernel32.GlobalMemoryStatusEx(ctypes.byref(status)):
            return status.total / 2 ** 30
        return 0
    try:
        return os.sysconf('SC_PAGE_SIZE') * os.sysconf('SC_PHYS_PAGES') / 2 ** 30
    except (ValueError, OSError):
        return 0


def cargo_slots(cpus=None, memory_gb=None):
    """How many Cargo stages may build at once: CPU threads per -j 4 stage, and memory
    above the reserve per stage peak, whichever is fewer; at least one.
    RUBYLIGHT_CARGO_SLOTS overrides it."""
    override = os.environ.get(CARGO_SLOTS_ENV, '').strip()
    if override.isdigit() and int(override) > 0:
        return int(override)
    cpus = cpus or os.cpu_count() or 1
    memory_gb = total_memory_gb() if memory_gb is None else memory_gb
    by_cpu = cpus // CARGO_STAGE_THREADS
    by_memory = int((memory_gb - MEMORY_RESERVE_GB) // CARGO_STAGE_PEAK_GB)
    return max(1, min(by_cpu, by_memory))


def busy_cargo_lanes(directory=None):
    """Count of Cargo stage lanes currently held (including our own if already leased)."""
    lanes = cargo_lanes(directory)
    held = held_lanes()
    count = 0
    for lane in lanes:
        if str(lane.path) in held or lane.busy():
            count += 1
    return count


def usable_build_threads(cpus=None):
    """Leave 1 physical core (2 logical threads) free on 8+ thread workstations so agents
    and the desktop stay responsive; smaller/CI runners keep all threads.
    RUBYLIGHT_RESERVED_THREADS overrides the reserved thread count (e.g. 1 on a build laptop)."""
    cpus = cpus or os.cpu_count() or 1
    override = os.environ.get(RESERVED_THREADS_ENV, '').strip()
    if override.isdigit():
        return max(1, cpus - int(override))
    return (cpus - 2) if cpus >= 8 else cpus


def nextest_threads(cpus=None):
    """Default test thread cap: leave 1 physical core free on 8+ thread machines."""
    return max(1, usable_build_threads(cpus))


def cargo_jobs(directory=None, cpus=None, active=None):
    """Dynamic Cargo -j count: scale up to CARGO_MAX_JOBS while reserving 1 physical core
    on 8+ thread machines so concurrent builds never starve agents or the desktop.
    RUBYLIGHT_CARGO_JOBS overrides."""
    override = os.environ.get(CARGO_JOBS_ENV, '').strip()
    if override.isdigit() and int(override) > 0:
        return int(override)
    usable = usable_build_threads(cpus)
    busy = max(1, busy_cargo_lanes(directory) if active is None else int(active))
    cap = min(CARGO_MAX_JOBS, usable)
    per_stage = usable // busy
    floor = 3 if (busy >= 4 and usable >= 8) else min(CARGO_STAGE_THREADS, cap)
    return max(floor, min(cap, per_stage))


def priority_alive(text, written=None):
    """Unknown ownership fails closed; PID reuse cannot prolong a known ticket.
    written: when the lock file was last written. Without start= in the text, a process
    that started after that is not the owner (its pid was reused)."""
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
    kernel.OpenProcess.restype, kernel.OpenProcess.argtypes = wintypes.HANDLE, [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    kernel.CloseHandle.argtypes = [wintypes.HANDLE]
    kernel.GetProcessTimes.argtypes = [wintypes.HANDLE, *([ctypes.POINTER(wintypes.FILETIME)] * 4)]
    handle = kernel.OpenProcess(0x1000, False, pid)
    if not handle:
        return ctypes.get_last_error() != 87  # access denied is not proof of exit
    try:
        times = [wintypes.FILETIME() for _ in range(4)]
        if not kernel.GetProcessTimes(handle, *(ctypes.byref(t) for t in times)):
            return True
        actual = ((times[0].dwHighDateTime << 32) + times[0].dwLowDateTime) / 10_000_000 - 11644473600
        start = re.search(r'\bstart=(\S+)', text)
        if not start:
            return written is None or actual <= written + 1
        try:
            expected = datetime.datetime.fromisoformat(start[1].replace('Z', '+00:00')).timestamp()
        except ValueError:
            return True
        return abs(actual - expected) < .001
    finally:
        kernel.CloseHandle(handle)


def read(path):
    try:
        return path.read_text(encoding='utf-8-sig')
    except FileNotFoundError:
        return None


def peek(path):
    """A lock's text for a decision: one its owner is still writing (PowerShell opens it
    unshared for a moment) counts as held."""
    try:
        return read(path)
    except PermissionError:
        return f'(owner is writing {path.name})'


NORMAL_PRIORITY_CLASS = 0x00000020
BELOW_NORMAL_PRIORITY_CLASS = 0x00004000
NORMAL_PRIORITY_ENV = 'RUBYLIGHT_BUILD_NORMAL_PRIORITY'


def lower_handle_priority(handle):
    """Lower a Windows process from Normal to BelowNormal so parallel Cargo/Gradle builds
    never starve DWM or user-mode desktop input hooks (`WH_MOUSE_LL`), while still using
    100% of idle cores. Leaves non-Normal processes (e.g. High-priority stream hosts) alone."""
    if os.name != 'nt' or not isinstance(handle, int) or handle <= 0:
        return False
    if os.environ.get(NORMAL_PRIORITY_ENV, '').strip() == '1':
        return False
    from ctypes import wintypes
    kernel = ctypes.WinDLL('kernel32', use_last_error=True)
    kernel.GetPriorityClass.restype, kernel.GetPriorityClass.argtypes = wintypes.DWORD, [wintypes.HANDLE]
    kernel.SetPriorityClass.restype, kernel.SetPriorityClass.argtypes = wintypes.BOOL, [wintypes.HANDLE, wintypes.DWORD]
    if kernel.GetPriorityClass(handle) != NORMAL_PRIORITY_CLASS:
        return False
    return bool(kernel.SetPriorityClass(handle, BELOW_NORMAL_PRIORITY_CLASS))


def wait_child(child):
    """Keep the lease until the command exits, including while it handles Ctrl+C."""
    lower_handle_priority(getattr(child, '_handle', None))
    while True:
        try:
            return child.wait()
        except KeyboardInterrupt:
            continue


def now():
    return datetime.datetime.now(datetime.timezone.utc).isoformat()


def process_start():
    """This process's start time as priority_alive reads it (start=), so a reused pid does not
    count. In .NET's round-trip form, as check_streams.ps1 writes and compares it (Test-LockOurs)."""
    if os.name != 'nt':
        return None
    from ctypes import wintypes
    kernel = ctypes.WinDLL('kernel32', use_last_error=True)
    kernel.GetCurrentProcess.restype = wintypes.HANDLE
    kernel.GetProcessTimes.argtypes = [wintypes.HANDLE, *([ctypes.POINTER(wintypes.FILETIME)] * 4)]
    times = [wintypes.FILETIME() for _ in range(4)]
    if not kernel.GetProcessTimes(kernel.GetCurrentProcess(), *(ctypes.byref(t) for t in times)):
        return None
    ticks = (times[0].dwHighDateTime << 32) + times[0].dwLowDateTime  # 100 ns since 1601
    whole = datetime.datetime(1601, 1, 1) + datetime.timedelta(seconds=ticks // 10_000_000)
    return whole.strftime('%Y-%m-%dT%H:%M:%S') + f'.{ticks % 10_000_000:07d}Z'


def end_children_with_me():
    """Put this process in a job that kills everything it starts when it ends, so a killed
    dev check leaves no build running in a slot it no longer holds (lane handles end with
    this process, not with its children). Best effort: False when Windows refuses."""
    if os.name != 'nt':
        return False
    from ctypes import wintypes

    class Basic(ctypes.Structure):
        _fields_ = [('per_process_time', ctypes.c_int64), ('per_job_time', ctypes.c_int64), ('flags', wintypes.DWORD),
                    ('min_ws', ctypes.c_size_t), ('max_ws', ctypes.c_size_t), ('active', wintypes.DWORD),
                    ('affinity', ctypes.c_size_t), ('priority', wintypes.DWORD), ('scheduling', wintypes.DWORD)]

    class Counters(ctypes.Structure):
        _fields_ = [(name, ctypes.c_uint64) for name in ('r', 'w', 'o', 'rb', 'wb', 'ob')]

    class Extended(ctypes.Structure):
        _fields_ = [('basic', Basic), ('io', Counters), ('process_memory', ctypes.c_size_t),
                    ('job_memory', ctypes.c_size_t), ('peak_process', ctypes.c_size_t), ('peak_job', ctypes.c_size_t)]

    kernel = ctypes.WinDLL('kernel32', use_last_error=True)
    kernel.CreateJobObjectW.restype, kernel.CreateJobObjectW.argtypes = wintypes.HANDLE, [wintypes.LPVOID, wintypes.LPCWSTR]
    kernel.SetInformationJobObject.argtypes = [wintypes.HANDLE, ctypes.c_int, wintypes.LPVOID, wintypes.DWORD]
    kernel.AssignProcessToJobObject.argtypes = [wintypes.HANDLE, wintypes.HANDLE]
    kernel.GetCurrentProcess.restype = wintypes.HANDLE
    job = kernel.CreateJobObjectW(None, None)
    if not job:
        return False
    info = Extended()
    # KILL_ON_JOB_CLOSE cleans up runaway compilers/tests if the check is killed;
    # BREAKAWAY_OK lets shared long-lived daemons (sccache) start outside this job so
    # exiting one dev check never tears down another concurrent build's cache server.
    info.basic.flags = 0x2000 | 0x0800  # JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_BREAKAWAY_OK
    if not kernel.SetInformationJobObject(job, 9, ctypes.byref(info), ctypes.sizeof(info)):  # extended limits
        return False
    # Lower this stage driver to BelowNormal before spawning compilers so every child
    # inherits BelowNormal from birth instead of competing with DWM/desktop input at Normal.
    lower_handle_priority(int(kernel.GetCurrentProcess()))
    # The job handle stays open (never closed) until this process ends.
    return bool(kernel.AssignProcessToJobObject(job, kernel.GetCurrentProcess()))


_LANES_LOCK = threading.RLock()
_ACTIVE_LANES = collections.Counter()
_EXTERNAL_LANES = None


def _sync_lanes_env():
    ext = _EXTERNAL_LANES or []
    paths = list(dict.fromkeys([*ext, *(p for p, count in _ACTIVE_LANES.items() if count > 0)]))
    if paths:
        os.environ[LANES_ENV] = os.pathsep.join(paths)
    else:
        os.environ.pop(LANES_ENV, None)


def held_lanes():
    with _LANES_LOCK:
        return {p for p in os.environ.get(LANES_ENV, '').split(os.pathsep) if p}


def environ_copy():
    with _LANES_LOCK:
        return dict(os.environ)


def own_exclusive(path):
    """This process tree holds BUILD-LOCK at path (a parent took it)."""
    inherited = os.environ.get(OWNER_ENV)
    return bool(inherited) and os.environ.get(PATH_ENV) == str(path) and peek(path) == inherited


if os.name == 'nt':
    from ctypes import wintypes
    _kernel = ctypes.WinDLL('kernel32', use_last_error=True)
    _kernel.CreateFileW.restype = wintypes.HANDLE
    _kernel.CreateFileW.argtypes = [wintypes.LPCWSTR, wintypes.DWORD, wintypes.DWORD, wintypes.LPVOID, wintypes.DWORD, wintypes.DWORD, wintypes.HANDLE]
    _kernel.CloseHandle.argtypes = [wintypes.HANDLE]
    _INVALID = wintypes.HANDLE(-1).value


def _open_unshared(path, create):
    """An open handle on path that nobody else may open, None while another process has
    it open, or False when it does not exist and create is off."""
    if os.name == 'nt':
        # GENERIC_READ | GENERIC_WRITE, no sharing, OPEN_ALWAYS or OPEN_EXISTING;
        # not inheritable, so the lane ends with this process.
        handle = _kernel.CreateFileW(str(path), 0xC0000000, 0, None, 4 if create else 3, 0x80, None)
        if handle == _INVALID or handle is None:
            error = ctypes.get_last_error()
            if error in (2, 3) and not create:  # ERROR_FILE_NOT_FOUND / ERROR_PATH_NOT_FOUND
                return False
            if error in (5, 32, 33):  # access denied (pending delete), sharing or lock violation
                return None
            raise OSError(error, f'cannot open lane {path}: {ctypes.FormatError(error).strip()}')
        return handle
    import fcntl
    try:
        fd = os.open(path, os.O_RDWR | (os.O_CREAT if create else 0))
    except FileNotFoundError:
        return False
    try:
        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except OSError:
        os.close(fd)
        return None
    return fd


def _close(handle):
    if os.name == 'nt':
        _kernel.CloseHandle(handle)
    else:
        os.close(handle)


class HandleSlot:
    """One lane file held open without sharing; <name>.holder says who holds it."""

    def __init__(self, path):
        self.path = pathlib.Path(path)
        self.handle = None

    @property
    def holder_path(self):
        return self.path.with_suffix('.holder')

    def try_take(self, label):
        self.path.parent.mkdir(parents=True, exist_ok=True)
        handle = _open_unshared(self.path, create=True)
        if not handle:
            return False
        self.handle = handle
        try:
            self.holder_path.write_text(f'{label} {now()} pid={os.getpid()}', encoding='utf-8')
        except OSError:
            pass
        return True

    def release(self):
        if self.handle is not None:
            _close(self.handle)
            self.handle = None

    def busy(self):
        """Another process holds it. Probing opens it for a moment: a stage taking it
        at the same time just tries again."""
        if str(self.path) in held_lanes() or self.handle is not None:
            return False
        handle = _open_unshared(self.path, create=False)
        if handle is False:
            return False
        if handle is None:
            return True
        _close(handle)
        return False

    def holder(self):
        return peek(self.holder_path) or str(self.path)


def _claim_file(path, label):
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open('x', encoding='utf-8') as out:
        start = process_start()
        text = f'{label} {now()} pid={os.getpid()}' + (f' start={start}' if start else '') + f' token={uuid.uuid4().hex}'
        out.write(text)
    # Python's Windows read handle denies deletion. Older age-based
    # waiters cannot remove this lease during a long live build.
    return text, path.open('rb')


def _release_file(path, text, raise_changed=False):
    # A changed owner is never deleted, even after command failure.
    for attempt in range(20):
        try:
            if read(path) != text:
                if raise_changed:
                    raise RuntimeError(f'BUILD-LOCK ownership changed: {path}; left alone')
                print(f'{path} ownership changed; left alone', file=sys.stderr)
            else:
                path.unlink()
            return
        except PermissionError:
            if attempt == 19:
                raise
            time.sleep(.05)


class FileSlot:
    """A BUILD-LOCK-style lease (the Gradle lane), for callers of build_lock.py --lock-directory."""

    def __init__(self, directory):
        self.path = pathlib.Path(directory) / 'BUILD-LOCK'
        self.text = None
        self.guard = None

    def try_take(self, label):
        if own_exclusive(self.path):
            return True
        try:
            self.text, self.guard = _claim_file(self.path, label)
            return True
        except FileExistsError:
            return False

    def release(self):
        if self.guard is None:
            return
        self.guard.close()
        self.guard = None
        _release_file(self.path, self.text)

    def busy(self):
        """Held by a live owner. A file whose owner is gone is never removed here, as with
        BUILD-LOCK, but an exclusive owner does not wait for it (holder() names it)."""
        text = peek(self.path)
        return self.guard is None and text is not None and not own_exclusive(self.path) \
            and str(self.path) not in held_lanes() and self.owner_alive(text)

    def owner_alive(self, text):
        try:
            written = self.path.stat().st_mtime
        except OSError:
            written = None
        return priority_alive(text, written)

    def holder(self):
        text = peek(self.path)
        if text and not self.owner_alive(text):
            return f'{text} (its owner is gone: check for surviving Gradle processes, then remove {self.path})'
        return text or str(self.path)


def cargo_lanes(directory=None):
    lanes = lock_directory(directory) / 'lanes'
    return [HandleSlot(lanes / f'cargo-{n}.lock') for n in range(1, cargo_slots() + 1)]


def web_lanes(directory=None):
    return [HandleSlot(lock_directory(directory) / 'lanes' / 'web-1.lock')]


def gradle_lanes(directory=None):
    return [FileSlot(gradle_lane(directory))]


def host_slots(directory=None, pool=None):
    pool = pathlib.Path(pool) if pool else pool_directory(directory)
    return [HandleSlot(pool / f'{n}.lock') for n in range(1, HOST_SLOTS + 1)]


def client_slots(directory=None, pool=None):
    pool = pathlib.Path(pool) if pool else pool_directory(directory)
    return [HandleSlot(pool / f'client-{n}.lock') for n in range(1, CLIENT_SLOTS + 1)]


def _min_free_gb(value=None):
    raw = os.environ.get(MIN_FREE_GB_ENV, '').strip() if value is None else value
    try:
        val = DEFAULT_MIN_FREE_GB if raw == '' else float(raw)
        return val if math.isfinite(val) else DEFAULT_MIN_FREE_GB
    except ValueError:
        return DEFAULT_MIN_FREE_GB


def _slot_mtime(pool, name, target):
    for probe in (pool / f'{name}.owner', pool / f'{name}.holder', target):
        try:
            return probe.stat().st_mtime
        except OSError:
            pass
    return 0.0


def _rmtree_reported(target, notify, errors):
    errs = []
    def on_err(_f, p, ei):
        exc = ei[1] if isinstance(ei, tuple) else ei
        if not isinstance(exc, FileNotFoundError):
            errs.append(f'{p}: {exc}')
    shutil.rmtree(target, **({'onexc' if sys.version_info >= (3, 12) else 'onerror': on_err}))
    if not target.exists() and not errs:
        return True
    msg = f'failed to prune idle target slot {target}: {"; ".join(errs) or "directory still exists"}'
    notify(msg)
    if errors is not None:
        errors.append(msg)
    return False


def prune_pool(pool=None, min_free_gb=None, all_idle=False, directory=None, keep=None, say=None, errors=None):
    """Remove idle target-pool slots oldest-first until free space meets the floor (or all idle when all_idle)."""
    notify = say or (lambda msg: print(msg, file=sys.stderr))
    if directory is not None:
        build = lock_directory(directory) / 'BUILD-LOCK'
        holder = None if own_exclusive(build) else peek(build)
        if holder is not None:
            msg = f'target pool: skipped prune; BUILD-LOCK held: {holder}'
            notify(msg)
            if errors is not None:
                errors.append(msg)
            return []
    pool = pathlib.Path(pool).resolve() if pool else pool_directory(directory)
    floor = 0 if all_idle else int(_min_free_gb(min_free_gb) * (2 ** 30))
    if not pool.is_dir() or (not all_idle and (floor <= 0 or shutil.disk_usage(pool).free >= floor)):
        return []
    keep_raw = () if keep is None else ([keep] if isinstance(keep, (str, os.PathLike)) else keep)
    keep_paths = {pathlib.Path(k).resolve() for k in keep_raw}
    names = ({str(n) for n in range(1, HOST_SLOTS + 1)} | {f'client-{n}' for n in range(1, CLIENT_SLOTS + 1)}
             | {'gradle-rust'} | {p.stem for p in pool.glob('*.lock')})
    candidates = sorted(
        (_slot_mtime(pool, n, pool / n), n, pool / n, pool / f'{n}.lock') for n in names
        if (pool / n).is_dir() and (pool / n).resolve() not in keep_paths and (pool / f'{n}.lock').resolve() not in keep_paths
    )
    pruned = []
    for _, name, target, lock_path in candidates:
        slot = HandleSlot(lock_path)
        if not slot.try_take('prune'):
            continue
        try:
            if _rmtree_reported(target, notify, errors):
                (pool / f'{name}.owner').unlink(missing_ok=True)
                pruned.append(target)
                if say:
                    say(f'pruned idle target slot {target}')
        finally:
            slot.release()
        if not all_idle and shutil.disk_usage(pool).free >= floor:
            break
    return pruned


def all_lanes(directory=None, pool=None):
    """Every lane an exclusive owner waits for, including Cargo slots past this machine's
    current count and pool slots another tool made."""
    lanes_dir = lock_directory(directory) / 'lanes'
    pool = pathlib.Path(pool) if pool else pool_directory(directory)
    paths = set(lanes_dir.glob('*.lock')) if lanes_dir.is_dir() else set()
    paths |= set(pool.glob('*.lock')) if pool.is_dir() else set()
    return [HandleSlot(p) for p in sorted(paths)] + gradle_lanes(directory)


def drain(directory=None, timeout=1800, poll=.5, pool=None, say=print, exclude=None):
    """Wait until no lane is held by another process tree; BUILD-LOCK keeps new stages out meanwhile.
    exclude: the exclusive lock itself, when its directory is also a lane (the Gradle lane)."""
    deadline, last = time.monotonic() + timeout, None
    while True:
        busy = [lane.holder() for lane in all_lanes(directory, pool)
                if (exclude is None or lane.path != exclude) and lane.busy()]
        if not busy:
            return
        text = '; '.join(busy)
        if text != last:
            say(f'BUILD-LOCK waiting for stages to finish: {text}', flush=True)
            last = text
        if time.monotonic() >= deadline:
            raise TimeoutError(f'stages still running after {timeout:g}s: {text}. No lock removed.')
        time.sleep(poll)


class StageLease:
    """The lanes of one dev-check stage: one free slot from each group, all or none.

    groups: [(name, callable returning the candidate slots in preference order)].
    Waits while BUILD-LOCK or a live release PRIORITY ticket is held by someone else.
    After entering, taken[name] is the slot taken from that group.
    """

    def __init__(self, groups, directory=None, label='stage', timeout=1800, poll=.5, say=print):
        self.groups, self.label, self.timeout, self.poll, self.say = groups, label, timeout, poll, say
        self.build = lock_directory(directory) / 'BUILD-LOCK'
        self.taken = {}
        self.waited = 0.0
        self._held = []

    def _release_lanes(self):
        global _EXTERNAL_LANES
        if not self._held:
            return
        with _LANES_LOCK:
            for path in self._held:
                if _ACTIVE_LANES[path] > 1:
                    _ACTIVE_LANES[path] -= 1
                else:
                    _ACTIVE_LANES.pop(path, None)
            self._held = []
            _sync_lanes_env()
            if not _ACTIVE_LANES:
                _EXTERNAL_LANES = None

    def _release(self):
        self._release_lanes()
        err = None
        for slot in self.taken.values():
            try:
                slot.release()
            except OSError as exc:
                err = err or exc
        self.taken = {}
        if err:
            raise err

    def __enter__(self):
        try:
            return self._enter()
        except BaseException:
            self._release()  # Ctrl+C or an error between taking a lane and checking BUILD-LOCK
            raise

    def _enter(self):
        global _EXTERNAL_LANES
        start = time.monotonic()
        deadline, last = start + self.timeout, None
        while True:
            holder = None
            priority = peek(self.build.with_name('PRIORITY'))
            if priority and priority_alive(priority) and not own_exclusive(self.build):
                holder = f'release priority: {priority}'
            else:
                for name, candidates in self.groups:
                    slots = candidates()
                    slot = next((s for s in slots if s.try_take(self.label)), None)
                    if slot is None:
                        holder = f'{name} slots busy: ' + '; '.join(s.holder() for s in slots)
                        break
                    self.taken[name] = slot
                if holder is None and not own_exclusive(self.build):
                    exclusive = peek(self.build)
                    if exclusive is not None:
                        holder = f'BUILD-LOCK: {exclusive}'
                if holder is not None:
                    self._release()
            if holder is None:
                break
            if holder != last:
                self.say(f'{self.label} waiting: {holder}', flush=True)
                last = holder
            if time.monotonic() >= deadline:
                raise TimeoutError(f'{self.label}: still waiting after {self.timeout:g}s: {holder}. No lock removed.')
            time.sleep(self.poll)
        self.waited = time.monotonic() - start
        held = [str(s.path) for s in self.taken.values()]
        with _LANES_LOCK:
            if not _ACTIVE_LANES:
                _EXTERNAL_LANES = [p for p in os.environ.get(LANES_ENV, '').split(os.pathsep) if p]
            for path in held:
                _ACTIVE_LANES[path] += 1
            self._held = held
            _sync_lanes_env()
        default_pool = pool_directory(self.build.parent)
        pool_slots = [s.path for k, s in self.taken.items()
                      if k in ('host target', 'client target', 'gradle target') or s.path.parent.resolve() == default_pool]
        if pool_slots or 'cargo' in self.taken:
            try:
                prune_pool(pool_slots[0].parent if pool_slots else default_pool, directory=self.build.parent, keep=pool_slots, say=self.say)
            except OSError:
                pass
        return self

    def __exit__(self, *_):
        self._release()


class BuildLock:
    """The exclusive lease. With drain (the default for the machine's BUILD-LOCK) it then
    waits for every stage lane to be given back."""

    def __init__(self, directory=None, label='build', timeout=1800, poll=.25, drain=True):
        self.path = lock_directory(directory) / 'BUILD-LOCK'
        self.label, self.timeout, self.poll, self.drain = label, timeout, poll, drain
        self.text = None
        self.guard = None
        self.previous = {}

    def __enter__(self):
        if own_exclusive(self.path):
            return self
        self.path.parent.mkdir(parents=True, exist_ok=True)
        start = time.monotonic()
        deadline, last = start + self.timeout, None
        while True:
            priority = peek(self.path.with_name('PRIORITY'))
            holder = f'release priority: {priority}' if priority and priority_alive(priority) else None
            if holder is None:
                try:
                    self.text, self.guard = _claim_file(self.path, self.label)
                    break
                except FileExistsError:
                    holder = peek(self.path) or '(owner is writing BUILD-LOCK)'
            if holder != last:
                print(f'BUILD-LOCK waiting: {holder}', flush=True)
                last = holder
            if time.monotonic() >= deadline:
                raise TimeoutError(f'BUILD-LOCK busy after {self.timeout:g}s: {holder}. No lock removed; inspect the owner and its children before recovery.')
            time.sleep(self.poll)
        if self.drain:
            try:
                drain(self.path.parent, max(0, deadline - time.monotonic()), self.poll, exclude=self.path)
            except BaseException:
                self.__exit__()
                raise
        for name, value in ((OWNER_ENV, self.text), (PATH_ENV, str(self.path)),
                            ('RUBYLIGHT_LOCK_DIRECTORY', str(self.path.parent))):
            self.previous[name] = os.environ.get(name)
            os.environ[name] = value
        return self

    def __exit__(self, *_):
        if self.guard is None:
            return
        self.guard.close()
        self.guard = None
        try:
            _release_file(self.path, self.text, raise_changed=True)
        finally:
            for name, value in self.previous.items():
                os.environ.pop(name, None) if value is None else os.environ.__setitem__(name, value)
            self.previous = {}


LANE_GROUPS = {
    'cargo': lambda d: [('cargo', lambda: cargo_lanes(d))],
    'gradle': lambda d: [('gradle', lambda: gradle_lanes(d))],
    'web': lambda d: [('web', lambda: web_lanes(d))],
}


def status(directory=None):
    directory = lock_directory(directory)
    lines = [f'BUILD-LOCK: {peek(directory / "BUILD-LOCK") or "free"}']
    priority = peek(directory / 'PRIORITY')
    if priority:
        lines.append(f'PRIORITY: {priority} ({"live" if priority_alive(priority) else "gone"})')
    lines.append(f'Cargo slots on this machine: {cargo_slots()}; target pool: {pool_directory(directory)}')
    return '\n'.join(lines + [f'{l.path}: {l.holder() if l.busy() else "free"}' for l in all_lanes(directory)])


def builds_without_target_dir(cargo_args):
    """Whether a hand-run cargo with these arguments builds into its default target dir."""
    own = cargo_args[:cargo_args.index('--')] if '--' in cargo_args else cargo_args
    if any(arg == '--target-dir' or arg.startswith('--target-dir=') for arg in own):
        return False
    rest = iter(own)
    for arg in rest:
        if arg in ('--version', '-V'):
            return False
        if arg in ('--color', '--config', '-C', '-Z'):
            next(rest, None)  # the option's value is not the subcommand
        elif not arg.startswith(('-', '+')):
            return arg not in ('fmt', 'metadata', 'tree', 'locate-project', 'pkgid')
    return True


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    parser.add_argument('--lock-directory')
    parser.add_argument('--timeout', type=float, default=1800)
    parser.add_argument('--label', default='build')
    parser.add_argument('--lane', choices=sorted(LANE_GROUPS), help='take this stage lane instead of BUILD-LOCK')
    parser.add_argument('--status', action='store_true', help='print who holds which lock')
    parser.add_argument('--drain', action='store_true',
                        help='wait until no stage lane is held (for an owner of BUILD-LOCK, as check_streams.ps1)')
    parser.add_argument('--prune', action='store_true', help='remove idle target-pool slot directories')
    parser.add_argument('--all', action='store_true', help='with --prune, remove all idle slots')
    parser.add_argument('--min-free-gb', type=float, help='with --prune, keep at least N GB free')
    parser.add_argument('--pool', help='target pool directory for --prune')
    parser.add_argument('command', nargs=argparse.REMAINDER)
    args = parser.parse_args(argv)
    command = args.command[1:] if args.command[:1] == ['--'] else args.command
    if args.timeout < 0 or (args.min_free_gb is not None and args.min_free_gb < 0):
        parser.error('timeout and --min-free-gb must be nonnegative')
    if args.all and args.min_free_gb is not None:
        parser.error('--all cannot be combined with --min-free-gb')
    if args.status:
        print(status(args.lock_directory))
        return 0
    if args.prune:
        failed = []
        say_cli = lambda m: print(m, file=sys.stderr if m.startswith(('failed', 'target pool: skipped')) else sys.stdout)
        if not prune_pool(args.pool, args.min_free_gb, args.all, lock_directory(args.lock_directory), say=say_cli, errors=failed) and not failed:
            print('target pool: nothing to prune')
        return 1 if failed else 0
    if not args.drain and not command:
        parser.error('provide a command after --')
    if args.lane == 'cargo' and pathlib.Path(command[0]).stem.lower() == 'cargo' and not os.environ.get('CARGO_TARGET_DIR') \
            and builds_without_target_dir(command[1:]):
        print('build_lock: no CARGO_TARGET_DIR, so this builds into a private target/; '
              'use `python tools/dev.py cargo -- ...` for a warm pool slot', file=sys.stderr)
    try:
        if args.drain:
            drain(args.lock_directory, args.timeout)
            return 0
        lease = StageLease(LANE_GROUPS[args.lane](args.lock_directory), args.lock_directory, args.label, args.timeout) if args.lane else BuildLock(args.lock_directory, args.label, args.timeout)
        with lease:
            return wait_child(subprocess.Popen(command))
    except (OSError, RuntimeError) as error:
        print(str(error), file=sys.stderr)
        return 1


if __name__ == '__main__':
    sys.exit(main())
