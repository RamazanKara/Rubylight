"""Run the smallest set of checks for what changed in this worktree.

usage: python tools/dev.py check [--base REF] [--dry-run] [--fast] [--log-dir DIR]
                                 [--target-dir DIR] [--paths PATH ...]
       python tools/dev.py cargo [--client] [--fast] [--label L] -- <cargo args...>   (tools/dev_cargo.py)

The changed files are those between the merge base of HEAD and --base
(default origin/main) and the working tree, plus untracked files. Each maps to
checks (see plan()):

- host workspace crates, and the crates that depend on them, to
  cargo fmt, clippy and nextest (or cargo test fallback) in a warm target slot;
- client/protocol crates, and their dependants, to `cargo nextest` / `cargo test`
  and `cargo clippy --all-targets -D warnings` in Nightjar's workspace,
  plus the interop tests once tests/interop exists;
- client/app sources to the Gradle unit tests of the touched classes;
- rust/service, the Windows service's own workspace, to its fmt, clippy and
  tests (and the release suites, which check its version rule);
- rust/web to the web console checks;
- rust/release scripts to their unittest suites, tools/ to its own.

Markdown is documentation and maps to nothing (in full check). Independent
lanes run concurrently via ThreadPoolExecutor; one PASS/FAIL table is printed
with <=40-line structured failure digests and each check's output in
<log dir>/<run>/<check>.log with summary.json.
Stops queueing new checks after failure unless --keep-going. Exits 1 if any check fails.
"""
import argparse, concurrent.futures, dataclasses, datetime, hashlib, json, os, pathlib, re, shutil, subprocess, sys, tempfile, threading, time, tomllib, uuid
import build_lock, generated_docs
from build_lock import StageLease, wait_child

ROOT = pathlib.Path(__file__).resolve().parents[1]


def default_log_root():
    return build_lock.default_directory() / 'unified' / 'dev-check'


_IMPORT_LOG_ROOT = default_log_root()
LOG_ROOT = _IMPORT_LOG_ROOT
HOST_TOOLCHAIN = '1.98.1-x86_64-pc-windows-gnu'
NEXTEST_CONFIG = '.config/nextest.toml'
DIGEST_MAX_LINES = 40
CLIENT_WORKSPACE = 'client/protocol'
# Files that change how every host crate builds.
HOST_WIDE = ('Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml', 'clippy.toml', '.cargo/',
             'rust/assets/version_info.rs')
# The Windows service's workspace and what else goes into its executable
# (rust/release/service_inputs.py), plus the toolchain and Cargo settings its
# tests and clippy run with here.
SERVICE_WORKSPACE = 'rust/service'
SERVICE_INPUTS = ('rust/service/', 'rust/core/src/update_recovery.rs', 'rust/core/src/update_files.rs',
                  'rust/core/src/state/file.rs', 'rust/assets/version_info.rs', 'rust/assets/butterpollo.ico',
                  'rust-toolchain.toml', '.cargo/')
# Fallback suites from rust/release when discovering against a synthetic test root.
RELEASE_SUITES = ('test_release.py', 'test_gpu_hang.py', 'test_timing_report.py', 'test_cadence_matrix.py',
                  'test_user_idle.py', 'test_e2e_result.py')
RELEASE_INPUTS = ('rust/release/', 'tools/nightjar_notes.py', '.github/workflows/rust-windows.yml') + SERVICE_INPUTS


def release_suites(root=ROOT):
    """Discover all test_*.py suites in rust/release/ (falling back to RELEASE_SUITES)."""
    found = tuple(sorted(p.name for p in (root / 'rust/release').glob('test_*.py')))
    return found or RELEASE_SUITES
GRADLE_WIDE = ('client/build.gradle', 'client/settings.gradle', 'client/gradle.properties', 'client/gradle/',
               'client/app/build.gradle', 'client/app/proguard-rules.pro', 'client/app/src/main/res/',
               'client/app/src/main/AndroidManifest.xml')
# Client workspace files outside its members that change every member's build.
CLIENT_WIDE = ('client/protocol/Cargo.toml', 'client/protocol/Cargo.lock', 'client/protocol/vendor/')
GRADLE_TASK = ':app:testNonRootDebugUnitTest'
GRADLE_LINT_TASK = ':app:lintNonRootDebug'


@dataclasses.dataclass
class Check:
    name: str
    kind: str
    items: list = dataclasses.field(default_factory=list)
    skip: str = ''

    def describe(self):
        return self.skip or (', '.join(self.items) if self.items else 'all')


def posix(path):
    return str(path).replace('\\', '/')


def workspace(root, manifest_dir):
    """Package name, directory and path dependencies of each workspace member."""
    base = root / manifest_dir
    manifest = tomllib.loads((base / 'Cargo.toml').read_text(encoding='utf-8'))['workspace']
    members, defaults = {}, set()
    for member in manifest['members']:
        directory = (base / member).resolve()
        package = tomllib.loads((directory / 'Cargo.toml').read_text(encoding='utf-8'))
        deps = set()
        tables = [package] + list(package.get('target', {}).values())
        for table in tables:
            for kind in ('dependencies', 'dev-dependencies', 'build-dependencies'):
                for spec in table.get(kind, {}).values():
                    if isinstance(spec, dict) and 'path' in spec:
                        deps.add(posix((directory / spec['path']).resolve().relative_to(root.resolve())))
        rel = posix(directory.relative_to(root.resolve()))
        members[rel] = (package['package']['name'], deps)
        if member in manifest.get('default-members', manifest['members']):
            defaults.add(rel)
    return members, defaults


def dependants(members, dirs):
    """The members in dirs plus every member that depends on them, transitively."""
    found, todo = set(), list(dirs)
    while todo:
        current = todo.pop()
        for rel, (_, deps) in members.items():
            if rel not in found and (rel == current or current in deps):
                found.add(rel)
                todo.append(rel)
    return found


def owner(path, dirs):
    """The deepest of dirs containing path."""
    best = None
    for d in dirs:
        if (path == d or path.startswith(d + '/')) and (best is None or len(d) > len(best)):
            best = d
    return best


def under(path, entries):
    """Whether path is one of the files, or inside one of the directories (ending in /), in entries."""
    return any(path.startswith(e) if e.endswith('/') else path == e for e in entries)


def gradle_tests(root, sources):
    """Gradle --tests filters for the unit tests covering the changed app sources."""
    tests = [p for p in (root / 'client/app/src').glob('test*/**/*') if p.suffix in ('.java', '.kt')]
    names = {p: p.stem for p in tests}

    def qualified(path):
        parts = path.relative_to(root / 'client/app/src').parts
        # src/<set>/<java|kotlin>/<package...>/<Class>.<ext>
        return '.'.join(parts[2:-1] + (path.stem,))

    filters, uncovered = set(), []
    for source in sources:
        path = root / source
        if posix(source).split('/')[3].startswith('test'):  # a test itself
            filters.add(qualified(path))
            continue
        # FooTest, else FooSomethingTest, else the tests that name Foo.
        cls = path.stem
        direct = [t for t, n in names.items() if re.fullmatch(re.escape(cls) + r'Tests?', n)]
        if not direct:
            direct = [t for t, n in names.items() if re.fullmatch(re.escape(cls) + r'[A-Z]\w*Tests?', n)]
        if not direct:
            word = re.compile(r'\b%s\b' % re.escape(cls))
            direct = [t for t in tests if re.fullmatch(r'\w*Tests?', t.stem)
                      and word.search(t.read_text(encoding='utf-8', errors='replace'))]
        if direct:
            filters.update(qualified(t) for t in direct)
        else:
            uncovered.append(cls)
    return sorted(filters), uncovered


def plan(paths, root=ROOT, fast=False):
    """The checks for a list of repository-relative paths, and the paths no check covers."""
    paths = sorted({posix(p) for p in paths})
    host, host_default = workspace(root, '.')
    client, client_default = workspace(root, CLIENT_WORKSPACE)
    # Directories outside the host workspace that host crates take by path (client/protocol/core).
    host_external = {d for _, deps in host.values() for d in deps if d not in host}
    interop = root / 'tests/interop'
    interop_in_host = 'tests/interop' in host

    host_dirs, client_dirs, client_all, service = set(), set(), False, False
    app_sources, gradle_all, web, release, tools, interop_hit, arch_hit, notes = [], False, False, set(), False, False, False, []
    for path in paths:
        hit = False
        if path.endswith('.md'):
            notes.append(path)
            continue
        if path.endswith(('.rs', '.java', '.kt', '.c', '.cpp', '.h', '.hpp', '.ts', '.svelte')):
            arch_hit = True
            hit = True
        if under(path, HOST_WIDE):
            host_dirs.update(host)
            hit = True
        d = owner(path, set(host) | host_external)
        if d:
            host_dirs.add(d)
            hit = True
            if d.startswith('rust/core') or d.startswith('rust/host'):
                interop_hit = True
        if path in ('rust/core/src/update_recovery.rs', 'rust/core/src/update_files.rs', 'rust/core/src/state/file.rs') and 'rust/core/session' in host:
            host_dirs.add('rust/core/session')
            interop_hit = True
            hit = True
        d = owner(path, set(client))
        if d or under(path, CLIENT_WIDE):
            if d:
                client_dirs.add(d)
            else:
                client_all = True
            interop_hit = True
            hit = True
        if path.startswith('tests/interop/'):
            interop_hit = True
            hit = True
        if path.startswith('client/app/src/') and path.endswith(('.java', '.kt')):
            app_sources.append(path)
            hit = True
        elif under(path, GRADLE_WIDE):
            gradle_all = True
            hit = True
        if under(path, SERVICE_INPUTS):
            service = True
            hit = True
        if path.startswith('rust/web/'):
            web = True
            hit = True
        if under(path, RELEASE_INPUTS):
            release.update(release_suites(root))
            # The script's own suite, or the suite itself.
            name = pathlib.PurePosixPath(path).name
            for suite in (name, f'test_{name}'):
                if path.startswith('rust/release/') and re.fullmatch(r'test_\w+\.py', suite) \
                        and (root / 'rust/release' / suite).is_file():
                    release.add(suite)
            hit = True
        if path.startswith('tools/'):
            tools = True
            hit = True
        if not hit:
            notes.append(path)

    checks = []
    if host_dirs:
        crates = sorted(host[d][0] for d in dependants(host, host_dirs) if d in host)
        if interop_in_host and interop_hit:
            crates = sorted(set(crates) | {host['tests/interop'][0]})
        checks.append(Check('host', 'host', crates))
    if client_dirs or client_all:
        # A lock file change checks the default members; a crate changed with it still gets its own check.
        dirs = dependants(client, client_dirs) | (set(client_default) if client_all else set())
        crates = sorted(client[d][0] for d in dirs)
        checks.append(Check('client-protocol', 'client', crates))
    if interop_hit and not interop_in_host:
        if (interop / 'Cargo.toml').is_file():
            checks.append(Check('interop', 'interop', ['tests/interop']))
        else:
            checks.append(Check('interop', 'interop', skip='tests/interop does not exist yet'))
    if service:
        checks.append(Check('service', 'service', ['butterpollo-service']))
    if fast:
        if gradle_all or app_sources:
            checks.append(Check('app', 'gradle', skip='skipped in --fast'))
        if web:
            checks.append(Check('web', 'web', ['check']))
        if release:
            checks.append(Check('release-py', 'python', skip='skipped in --fast'))
        if tools:
            checks.append(Check('tools-py', 'tools', ['tools/test_*.py']))
        checks.append(Check('code-map', 'code_map', ['docs/code-map.md', 'client/docs/code-map.md']))
        checks.append(Check('arch-check', 'arch', ['tools/arch_check.py']))
        checks.append(Check('generated-docs', 'generated', [d.path for d in generated_docs.DOCS]))
        return checks, notes
    if gradle_all:
        checks.append(Check('app', 'gradle', []))
    elif app_sources:
        filters, uncovered = gradle_tests(root, app_sources)
        if filters:
            checks.append(Check('app', 'gradle', filters))
        if uncovered:
            checks.append(Check('app-uncovered', 'gradle',
                                skip='no unit tests cover ' + ', '.join(sorted(set(uncovered)))))
    if web:
        checks.append(Check('web', 'web', ['check', 'node --test', 'build']))
    if release:
        checks.append(Check('release-py', 'python', sorted(release)))
    if tools:
        checks.append(Check('tools-py', 'tools', ['tools/test_*.py']))
    # A generated doc, its generator or its sources: is the doc still what they generate?
    generated = generated_docs.affected(paths)
    if generated:
        checks.append(Check('generated-docs', 'generated', [d.path for d in generated]))
    if checks or arch_hit:
        checks.append(Check('arch-check', 'arch', ['tools/arch_check.py']))
    return checks, notes


def git(*args, root=ROOT):
    out = subprocess.run(['git', '-C', str(root), *args], capture_output=True, text=True, encoding='utf-8')
    if out.returncode:
        raise SystemExit(f'git {" ".join(args)} failed: {out.stderr.strip()}')
    return out.stdout


def changed(base, root=ROOT):
    merge_base = git('merge-base', 'HEAD', base, root=root).strip()
    files = git('diff', '--name-only', '--no-renames', merge_base, root=root).splitlines()
    files += git('ls-files', '--others', '--exclude-standard', root=root).splitlines()
    return merge_base, [f for f in files if f]


def worktree_name(root=ROOT):
    return root.resolve().name


STAMP = '.rubylight-checkout'


def _expand_short_path(text):
    if os.name != 'nt' or not text or '~' not in text:
        return text
    import ctypes
    buf = ctypes.create_unicode_buffer(32768)
    n = ctypes.windll.kernel32.GetLongPathNameW(text, buf, len(buf))
    if 0 < n < len(buf):
        return buf.value
    # Expand the deepest existing 8.3 ancestor when a leaf does not exist yet.
    p = pathlib.Path(text)
    tail = []
    for _ in range(len(p.parts)):
        parent = p.parent
        if parent == p:
            break
        tail.append(p.name)
        p = parent
        if p.exists():
            n = ctypes.windll.kernel32.GetLongPathNameW(str(p), buf, len(buf))
            if 0 < n < len(buf):
                return str(pathlib.Path(buf.value, *reversed(tail)))
            break
    return text


def path_key(path):
    raw = str(path).strip()
    text = _expand_short_path(os.path.normpath(raw) if raw else '')
    return os.path.normcase(os.path.normpath(text))


def same_path(a, b):
    return path_key(a) == path_key(b)


def local_packages(root=ROOT):
    """The names of every Cargo package in this checkout and its submodules: the ones Cargo
    builds from paths here and judges by modification time."""
    names = set()
    out = subprocess.run(['git', '-C', str(root), 'ls-files', '--recurse-submodules', '--', 'Cargo.toml', '*/Cargo.toml'],
                         capture_output=True, text=True, encoding='utf-8')
    for rel in out.stdout.splitlines():
        try:
            name = tomllib.loads((root / rel).read_text(encoding='utf-8')).get('package', {}).get('name')
        except (OSError, ValueError):
            continue
        if isinstance(name, str):
            names.add(name)
    return names


def fingerprint_dirs(target):
    """Where Cargo keeps its build records in a target dir, two layouts: the .fingerprint
    folders (<profile>, <triple>/<profile>, and the same below a nested target such as the
    service's <target>/service; entries <name>-<16 hex>) and, since Cargo 1.100, the build
    folders whose entries are <name>/<16 hex>, one folder per build with its fingerprint
    and outputs inside (the 2026-11-20 CI failure: no .fingerprint exists there)."""
    target = pathlib.Path(target)
    found = []
    for pattern in ('*/.fingerprint', '*/*/.fingerprint', '*/*/*/.fingerprint',
                    '*/build', '*/*/build', '*/*/*/build'):
        found += [p for p in target.glob(pattern) if p.is_dir()]
    return found


def stale_entries(directory, names):
    """The folders in one fingerprint_dirs entry that belong to the named packages."""
    pattern = re.compile('^(?:%s)-[0-9a-f]{16}$' % '|'.join(re.escape(n) for n in sorted(names)))
    if directory.name == '.fingerprint':
        return [e for e in directory.iterdir() if e.is_dir() and pattern.match(e.name)]
    # build/<name>/<16 hex>; the older layout's build/<name>-<16 hex> script folders match
    # neither part and stay (Cargo reruns them by their own fingerprints).
    return [unit for package in directory.iterdir() if package.is_dir() and package.name in names
            for unit in package.iterdir() if unit.is_dir() and re.fullmatch('[0-9a-f]{16}', unit.name)]


def invalidate(target, names):
    """Remove the builds of the named packages so Cargo rebuilds them, from either layout of
    fingerprint_dirs; the dependencies' (registry and git crates, keyed by version and
    source) stay warm."""
    if not names:
        return 0
    removed, left = 0, []
    for directory in fingerprint_dirs(target):
        for entry in stale_entries(directory, names):
            shutil.rmtree(entry, ignore_errors=True)
            if entry.exists():
                left.append(str(entry))
            else:
                removed += 1
    if left:
        raise RuntimeError(f'could not reset {len(left)} fingerprints in {target} (is another build using it?): {left[0]}')
    return removed


def claim_target(target, root=ROOT, previous_owner=None, owner_since=None):
    r"""Make a reused target dir safe for this checkout; returns how many fingerprints it removed.

    Cargo judges a path package fresh when none of its files is newer than its last build,
    with the files listed relative to the package root, and hashes the package's path
    relative to the workspace. Two checkouts of the same version therefore share artifact
    names in one target dir, and a checkout whose files are older than another checkout's
    last build there gets that build (2026-11-18 in C:\src\rl-target: `cannot find value
    MAX_BITRATE_KBPS` and `unresolved import rubylight_protocol::invite_uri`, both present
    in the checkout's source). So when the folder's last checkout was another one, or is
    unknown while it holds builds, this checkout's packages lose their fingerprints first.
    owner_since: when the slot's owner file was last written; C:\src\rl-quick.ps1 rewrites it
    without claiming the folder, so an owner file newer than the stamp means someone else
    may have built there since, whoever it names.
    """
    target = pathlib.Path(target)
    stamp = target / STAMP
    last = build_lock.read(stamp)
    mine = os.path.normpath(str(pathlib.Path(root).resolve()))
    others = [o for o in (last, previous_owner) if o and o.strip() and not same_path(o, mine)]
    if owner_since is not None and stamp.exists() and owner_since > stamp.stat().st_mtime + 1:
        others.append('another run since the last claim')
    unknown = last is None and target.is_dir() and bool(fingerprint_dirs(target))
    removed = invalidate(target, local_packages(root)) if others or unknown else 0
    target.mkdir(parents=True, exist_ok=True)
    stamp.write_text(mine, encoding='utf-8')
    return removed


def by_affinity(slots, root=ROOT):
    """This checkout's own slots first, then other checkouts' least recently used ones (their
    dependencies are warm), then never used ones."""
    def key(slot):
        owner_file = slot.path.with_suffix('.owner')
        owner = build_lock.read(owner_file) if owner_file.exists() else None
        if owner and same_path(owner, root):
            return (0, 0)
        if owner and owner.strip():
            return (1, owner_file.stat().st_mtime)
        return (2, 0)
    return sorted(slots, key=key)


def stage_groups(check, root, args):
    """The lanes a check takes, as (name, candidates) for StageLease."""
    lock_dir, pool = args.lock_directory, args.pool
    cargo = ('cargo', lambda: build_lock.cargo_lanes(lock_dir))
    if check.kind in ('host', 'service', 'client', 'interop'):
        if args.target_dir:
            # Two checks never build one folder at once: claiming and building are not atomic.
            folder = str(args.target_dir) + ('-client' if check.kind in ('client', 'interop') else '')
            return [cargo, ('target dir', lambda: [build_lock.HandleSlot(f'{folder}.lock')])]
        if check.kind in ('host', 'service'):
            return [cargo, ('host target', lambda: by_affinity(build_lock.host_slots(lock_dir, pool), root))]
        return [cargo, ('client target', lambda: by_affinity(build_lock.client_slots(lock_dir, pool), root))]
    if check.kind == 'gradle':
        if args.target_dir:
            folder = f'{args.target_dir}-gradle'
            return [('gradle', lambda: build_lock.gradle_lanes(lock_dir)),
                    ('target dir', lambda: [build_lock.HandleSlot(f'{folder}.lock')])]
        pool_dir = pathlib.Path(pool) if pool else build_lock.pool_directory(lock_dir)
        return [('gradle', lambda: build_lock.gradle_lanes(lock_dir)),
                ('gradle target', lambda: [build_lock.HandleSlot(pool_dir / 'gradle-rust.lock')])]
    if check.kind == 'web':
        return [('web', lambda: build_lock.web_lanes(lock_dir))]
    return []


def stage_target(check, taken, root, target_dir=None):
    """The target dir of a Cargo or Gradle check, claimed for this checkout: its pool slot, else
    --target-dir (the client's and gradle's beside it, the service's inside it). None for other checks."""
    if check.kind not in ('host', 'service', 'client', 'interop', 'gradle'):
        return None
    slot = taken.get('host target') or taken.get('client target') or taken.get('gradle target')
    if slot is not None:
        return claim_slot(slot.path, root)
    if target_dir is None:
        return None
    target = pathlib.Path(target_dir)
    if check.kind in ('client', 'interop'):
        target = pathlib.Path(f'{target}-client')
    elif check.kind == 'gradle':
        target = pathlib.Path(f'{target}-gradle')
    claim_target(target, root)
    return target


def claim_slot(lock, root=ROOT):
    """Claim the pool slot of lock (<pool>\\<n>.lock, held by the caller) for this checkout and
    return its folder. C:\\src\\rl-env.ps1's Enter-RlSlot calls this too (`dev.py claim-slot`)."""
    lock = pathlib.Path(lock)
    owner_file = lock.with_suffix('.owner')
    previous = build_lock.read(owner_file) if owner_file.exists() else None
    since = owner_file.stat().st_mtime if owner_file.exists() else None
    target = lock.with_suffix('')
    owner_file.write_text(os.path.normpath(str(pathlib.Path(root).resolve())), encoding='utf-8')
    claim_target(target, root, previous, since)
    try:
        build_lock.prune_pool(lock.parent, keep=lock)
    except OSError:
        pass
    return target


def prepend_path(existing, dirs):
    seen, ordered = set(), []
    for entry in [*dirs, *(existing.split(os.pathsep) if existing else [])]:
        if not entry:
            continue
        text = str(entry)
        key = path_key(text)
        if key not in seen:
            seen.add(key)
            ordered.append(text)
    return os.pathsep.join(ordered)


def _is_msys_path(path):
    parts = [p.lower() for p in pathlib.Path(os.path.normpath(str(path))).parts]
    return 'msys64' in parts


def _is_windows_apps_path(path):
    return os.path.join('microsoft', 'windowsapps') in path_key(path)


def preferred_python(env=None):
    """Return a non-MSYS2 Windows Python when running under msys64, else sys.executable."""
    exe = sys.executable
    if os.name != 'nt' or not _is_msys_path(exe):
        return exe
    env = env or os.environ
    local = env.get('LOCALAPPDATA')
    if local:
        candidate = pathlib.Path(local) / 'Programs' / 'Python' / 'Python313' / 'python.exe'
        if candidate.is_file():
            return str(candidate)
    path_val = env.get('PATH', '')
    for entry in path_val.split(os.pathsep):
        if not entry or _is_msys_path(entry) or _is_windows_apps_path(entry):
            continue
        found = shutil.which('python', path=entry)
        if found and not _is_msys_path(found) and not _is_windows_apps_path(found):
            return found
    py_launcher = shutil.which('py', path=path_val)
    if py_launcher:
        try:
            res = subprocess.run([py_launcher, '-3.13', '-c', 'import sys; print(sys.executable)'],
                                 env=env, capture_output=True, text=True, timeout=5)
            resolved = res.stdout.strip().splitlines()[0].strip() if res.stdout.strip() else ''
            if res.returncode == 0 and resolved and pathlib.Path(resolved).is_file() and not _is_msys_path(resolved):
                return resolved
        except (OSError, subprocess.SubprocessError):
            pass
    return exe


def _python_dir(env):
    return str(pathlib.Path(preferred_python(env)).parent)


def tool_env(fast=False, lock_directory=None):
    env = build_lock.environ_copy()
    env['PATH'] = prepend_path(env.get('PATH', ''), [_python_dir(env)])
    if fast:
        env['NIGHTJAR_ALLOW_UNTRANSLATED'] = '1'
    else:
        env.pop('NIGHTJAR_ALLOW_UNTRANSLATED', None)
    if lock_directory is not None:
        env['RUBYLIGHT_LOCK_DIRECTORY'] = str(build_lock.lock_directory(lock_directory))
    return env


def apply_gnu_lld(env):
    if os.name != 'nt':
        return env
    if not shutil.which('ld.lld', path=env.get('PATH')):
        return env
    key = 'CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUSTFLAGS'
    current = env.get(key, '').strip()
    outer = env.pop('RUSTFLAGS', '').strip()
    env.pop('CARGO_ENCODED_RUSTFLAGS', None)
    if outer and not ('-fuse-ld=' in current and '-fuse-ld=' in outer):
        current = f'{current} {outer}'.strip()
    if '-fuse-ld=' not in current:
        current = f'{current} -Clink-arg=-fuse-ld=lld'.strip()
    env[key] = current
    return env


def _strip_target_dir_env(env):
    """Remove CARGO_TARGET_DIR and CARGO_BUILD_TARGET_DIR from env so sccache's Rust cache keys
    stay identical across target-pool slots. Callers pass `--target-dir` on Cargo's CLI instead."""
    for key in [k for k in env if k.upper() in ('CARGO_TARGET_DIR', 'CARGO_BUILD_TARGET_DIR')]:
        env.pop(key, None)
    return env


def _ensure_sccache_server(wrapper, env):
    """Best-effort start of the sccache server detached from our Windows job object so
    exiting this dev check never kills a shared sccache server used by concurrent builds."""
    if os.name != 'nt':
        return
    flags = (
        getattr(subprocess, 'CREATE_BREAKAWAY_FROM_JOB', 0)
        | getattr(subprocess, 'CREATE_NO_WINDOW', 0)
        | getattr(subprocess, 'NORMAL_PRIORITY_CLASS', build_lock.NORMAL_PRIORITY_CLASS)
    )
    server_env = dict(env)
    server_env.setdefault('SCCACHE_IDLE_TIMEOUT', '3600')
    try:
        subprocess.run([wrapper, '--start-server'], env=server_env, stdin=subprocess.DEVNULL,
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                       creationflags=flags, timeout=10)
    except (OSError, subprocess.SubprocessError):
        pass


def apply_sccache(env, start_server=False):
    """Enable sccache automatically when available on PATH, unless disabled or already wrapped."""
    if env.get('RUBYLIGHT_NO_SCCACHE', '').strip() == '1':
        return env
    if env.get('RUSTC_WRAPPER') or env.get('CARGO_BUILD_RUSTC_WRAPPER'):
        return env
    wrapper = shutil.which('sccache', path=env.get('PATH'))
    if not wrapper:
        return env
    env['RUSTC_WRAPPER'] = wrapper
    env.setdefault('CARGO_INCREMENTAL', '0')
    if start_server:
        _ensure_sccache_server(wrapper, env)
    return env


def _msys_ucrt_dir(env):
    raw = env.get('RUBYLIGHT_MSYS_ROOT') or r'C:\msys64'
    root = pathlib.Path(raw)
    ucrt = root / 'ucrt64'
    if ucrt.is_dir():
        return ucrt
    if (root / 'bin' / 'gcc.exe').is_file():
        return root
    return ucrt


def _cargo_bin_dir(env):
    cargo_home = env.get('CARGO_HOME')
    base = pathlib.Path(cargo_home) if cargo_home else (pathlib.Path.home() / '.cargo')
    return base / 'bin'


def _pyrowave_dir(env):
    if env.get('BUTTERPOLLO_PYROWAVE_ROOT'):
        return pathlib.Path(env['BUTTERPOLLO_PYROWAVE_ROOT'])
    local = env.get('LOCALAPPDATA')
    base = pathlib.Path(local) if local else (pathlib.Path.home() / 'AppData' / 'Local')
    return base / 'ButterpolloRust' / 'sdk' / 'pyrowave-a32b05e0'


def host_env(target, lock_directory=None, fast=False, set_target_dir_env=False):
    env = _strip_target_dir_env(tool_env(fast=fast, lock_directory=lock_directory))
    sys_exe_dir = _python_dir(env)
    cargo_bin = _cargo_bin_dir(env)
    msys = _msys_ucrt_dir(env)
    pyrowave = _pyrowave_dir(env)
    if os.name == 'nt':
        required = (
            cargo_bin / 'cargo.exe',
            msys / 'bin' / 'gcc.exe',
            msys / 'include' / 'vulkan',
            pyrowave / 'share' / 'pyrowave-shared' / 'build-info.txt',
        )
        for need in required:
            if not need.exists():
                raise RuntimeError(f'rl-env: {need} is missing')
        env['RUSTUP_TOOLCHAIN'] = HOST_TOOLCHAIN
    env['PATH'] = prepend_path(env.get('PATH', ''), [sys_exe_dir, str(cargo_bin), str(msys / 'bin'), str(pyrowave / 'bin')])
    env['LIBCLANG_PATH'] = str(msys / 'bin')
    env['BUTTERPOLLO_VULKAN_INCLUDE'] = str(msys / 'include')
    env['BUTTERPOLLO_PYROWAVE_ROOT'] = str(pyrowave)
    if set_target_dir_env and target is not None:
        env['CARGO_TARGET_DIR'] = str(target)
    env['CARGO_BUILD_JOBS'] = str(build_lock.cargo_jobs(lock_directory))
    threads = str(build_lock.nextest_threads())
    env.setdefault('NEXTEST_TEST_THREADS', threads)
    env.setdefault('RUST_TEST_THREADS', threads)
    env.setdefault('CARGO_PROFILE_DEV_DEBUG', '0')
    if lock_directory is not None or not env.get('RUBYLIGHT_LOCK_DIRECTORY'):
        env['RUBYLIGHT_LOCK_DIRECTORY'] = str(build_lock.lock_directory(lock_directory))
    return apply_sccache(apply_gnu_lld(env), start_server=True)


def client_env(target, fast=False, lock_directory=None, set_target_dir_env=False):
    env = _strip_target_dir_env(tool_env(fast=fast, lock_directory=lock_directory))
    sys_exe_dir = _python_dir(env)
    cargo_bin = _cargo_bin_dir(env)
    msys_bin = _msys_ucrt_dir(env) / 'bin'
    env['PATH'] = prepend_path(env.get('PATH', ''), [sys_exe_dir, str(cargo_bin), str(msys_bin)])
    env['LIBCLANG_PATH'] = str(msys_bin)
    if set_target_dir_env and target is not None:
        env['CARGO_TARGET_DIR'] = str(target)
    env['CARGO_BUILD_JOBS'] = str(build_lock.cargo_jobs(lock_directory))
    threads = str(build_lock.nextest_threads())
    env.setdefault('NEXTEST_TEST_THREADS', threads)
    env.setdefault('RUST_TEST_THREADS', threads)
    env.setdefault('CARGO_PROFILE_DEV_DEBUG', '0')
    return apply_sccache(apply_gnu_lld(env), start_server=True)


def interop_env(target, fast=False, lock_directory=None, set_target_dir_env=False):
    env = client_env(target, fast=fast, lock_directory=lock_directory, set_target_dir_env=set_target_dir_env)
    if os.name == 'nt':
        msys = _msys_ucrt_dir(env)
        pyrowave = _pyrowave_dir(env)
        if (msys / 'include' / 'vulkan').is_dir():
            env.setdefault('BUTTERPOLLO_VULKAN_INCLUDE', str(msys / 'include'))
        if (pyrowave / 'bin').is_dir():
            env.setdefault('BUTTERPOLLO_PYROWAVE_ROOT', str(pyrowave))
            env['PATH'] = prepend_path(env.get('PATH', ''), [_python_dir(env), str(_cargo_bin_dir(env)), str(msys / 'bin'), str(pyrowave / 'bin')])
    return env


def service_env(target, fast=False, lock_directory=None, set_target_dir_env=False):
    env = client_env(pathlib.Path(target) / 'service', fast=fast, lock_directory=lock_directory,
                     set_target_dir_env=set_target_dir_env)
    for name in ('RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'CARGO_BUILD_RUSTFLAGS',
                 'CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUSTFLAGS',
                 'RUSTC_WRAPPER', 'CARGO_BUILD_RUSTC_WRAPPER', 'CARGO_INCREMENTAL',
                 'CARGO_PROFILE_DEV_DEBUG'):
        env.pop(name, None)
    return env


# Robolectric's native Windows SQLite opens <TEMP>\robolectric-<Class>_<Method><id>\...\computers4.db-wal;
# sandbox TEMP paths longer than ~45 chars push long test names past Win32 MAX_PATH (260).
GRADLE_TEMP_MAX_LEN = 45


def short_windows_temp(env):
    """Return a shorter existing Windows temp directory when TEMP/TMP exceeds GRADLE_TEMP_MAX_LEN."""
    if os.name != 'nt':
        return None
    temps = [v for k, v in env.items() if k.upper() in ('TEMP', 'TMP') and v]
    if not temps or max(len(v) for v in temps) <= GRADLE_TEMP_MAX_LEN:
        return None
    cur_max = max(len(v) for v in temps)
    candidates = []
    for var, suffix in (('LOCALAPPDATA', ('Temp',)), ('USERPROFILE', ('AppData', 'Local', 'Temp'))):
        val = next((v for k, v in env.items() if k.upper() == var and v), None)
        if val:
            candidates.append(pathlib.Path(val).joinpath(*suffix))
    for cand in candidates:
        text = str(cand)
        if len(text) < cur_max and cand.is_dir():
            return text
    return None


def gradle_env(fast=False, lock_directory=None, target=None):
    env = tool_env(fast=fast, lock_directory=lock_directory)
    env = {k: v for k, v in env.items() if k.upper() != 'CARGO_TARGET_DIR'}
    if target is not None:
        env['CARGO_TARGET_DIR'] = str(target)
    # Its Rust engine builds beside the Cargo stages: scale dynamically with active Cargo lanes.
    env['CARGO_BUILD_JOBS'] = str(build_lock.cargo_jobs(lock_directory))
    env.setdefault('CARGO_PROFILE_DEV_DEBUG', '0')
    if os.name == 'nt':
        extra = [str(p) for p in (_cargo_bin_dir(env), _msys_ucrt_dir(env) / 'bin') if p.is_dir()]
        if extra:
            env['PATH'] = prepend_path(env.get('PATH', ''), extra)
        apply_gnu_lld(env)
    apply_sccache(env, start_server=True)
    short = short_windows_temp(env)
    if short:
        for key in [k for k in env if k.upper() in ('TEMP', 'TMP')]:
            env[key] = short
        env['TEMP'] = short
        env['TMP'] = short
    return env


# cmd takes at most 8191 characters on a command line ("Die Befehlszeile ist zu lang" with 170 test classes), and
# gradlew.bat hands the arguments on in a longer line: the JDK, JAVA_OPTS and GRADLE_OPTS come first.
CMD_LINE_LIMIT = 8191
# What else gradlew.bat's java line adds (java.exe, its default JVM options, the wrapper jar under the checkout, the
# main class) beyond the line here: about 140 characters, with room to spare.
GRADLEW_OVERHEAD = 400


def default_gradle_test_forks(cpus=None, memory_gb=None):
    """Parallel Robolectric test JVMs for local dev-check runs (each uses ~2 GB heap)."""
    override = os.environ.get('RUBYLIGHT_GRADLE_TEST_FORKS', '').strip()
    if override.isdigit() and int(override) > 0:
        return int(override)
    if os.environ.get('CI'):
        return None
    cpus = cpus or os.cpu_count() or 1
    memory_gb = build_lock.total_memory_gb() if memory_gb is None else memory_gb
    if cpus >= 12 and memory_gb >= 32:
        return 6
    if cpus >= 8 and memory_gb >= 24:
        return 4
    return None


def gradle_command(root, items, test_list_path=None, temp_dir=None, daemon=None, max_workers=None, unit_test_forks=None):
    """The app tests for these classes (via --tests or -PrubylightTestList), or the whole suite + lint."""
    # By its full path: with NoDefaultCurrentDirectoryInExePath set, cmd
    # doesn't look in its working folder for a bare gradlew.bat.
    if daemon is None:
        daemon = not (os.environ.get('CI') or os.environ.get('RUBYLIGHT_GRADLE_NO_DAEMON', '').strip() == '1')
    workers = max_workers if max_workers is not None else (2 if os.environ.get('CI') else 4)
    command = ['cmd', '/c', str(root / 'client' / 'gradlew.bat')]
    if not daemon:
        command.append('--no-daemon')
    command.append(f'--max-workers={workers}')
    forks = unit_test_forks if unit_test_forks is not None else default_gradle_test_forks()
    if forks and forks > 2:
        command.append(f'-PunitTestForks={forks}')
    if temp_dir:
        command.append(f'-Djava.io.tmpdir={temp_dir}')
    command.append(GRADLE_TASK)
    if not items:
        return command + [GRADLE_LINT_TASK]
    filters = [a for t in items for a in ('--tests', t)]
    budget = CMD_LINE_LIMIT - GRADLEW_OVERHEAD - sum(len(os.environ.get(name, ''))
                                                     for name in ('JAVA_HOME', 'JAVA_OPTS', 'GRADLE_OPTS'))
    if test_list_path is None and len(subprocess.list2cmdline(command + filters)) <= budget:
        return command + filters
    if test_list_path is None:
        raise ValueError('test_list_path is required when Gradle test filters exceed command line budget')
    list_file = pathlib.Path(test_list_path)
    list_file.write_text('\n'.join(items) + '\n', encoding='utf-8')
    return command + [f'-PrubylightTestList={list_file}']


def parse_cargo_json_line(line):
    """Parse one line of `cargo --message-format=json`; returns (rendered_text, structured_line)."""
    stripped = line.lstrip()
    if not stripped.startswith('{'):
        return line, None
    try:
        data = json.loads(stripped)
    except ValueError:
        return line, None
    if not isinstance(data, dict) or 'reason' not in data:
        return line, None
    if data.get('reason') != 'compiler-message':
        return None, None
    msg = data.get('message') or {}
    rendered = msg.get('rendered') or ''
    level = msg.get('level') or ''
    summary = None
    if level in ('error', 'warning'):
        spans = msg.get('spans') or []
        primary = next((s for s in spans if s.get('is_primary')), None) or (spans[0] if spans else None)
        code = (msg.get('code') or {}).get('code')
        code_tag = f'[{code}]' if code else ''
        text = msg.get('message') or ''
        if primary:
            summary = (f"{primary.get('file_name', '<unknown>')}:{primary.get('line_start', 0)}:"
                       f"{primary.get('column_start', 0)}: {level}{code_tag}: {text}")
        else:
            summary = f'{level}{code_tag}: {text}'
    if rendered and not rendered.endswith('\n'):
        rendered += '\n'
    return rendered or None, summary


def _gradle_wrapper_version(root):
    props = root / 'client' / 'gradle' / 'wrapper' / 'gradle-wrapper.properties'
    try:
        m = re.search(r'gradle-([0-9][0-9A-Za-z.-]*)-(?:bin|all)\.zip', props.read_text(encoding='utf-8'))
        return m.group(1) if m else ''
    except OSError:
        return ''


def _has_compatible_gradle_daemon(root, temp_dir=None):
    """Return True if a GradleDaemon JVM matching this checkout's Gradle version and tmpdir is running."""
    if os.name != 'nt':
        return True
    version = _gradle_wrapper_version(root)
    ps = ("Get-CimInstance Win32_Process -Filter \"Name='java.exe'\" "
          "| ForEach-Object { $_.CommandLine }")
    shell = shutil.which('pwsh') or shutil.which('powershell')
    if not shell:
        return False
    try:
        probe = subprocess.run([shell, '-NoProfile', '-Command', ps],
                               capture_output=True, text=True, encoding='utf-8', errors='replace', timeout=10)
    except (OSError, subprocess.SubprocessError):
        return False
    if probe.returncode != 0:
        return False
    want_tmp = f'-djava.io.tmpdir={temp_dir}'.lower() if temp_dir else None
    for line in probe.stdout.splitlines():
        low = line.lower()
        if 'org.gradle.launcher.daemon.bootstrap.gradledaemon' not in low:
            continue
        if version and version.lower() not in low:
            continue
        if '-xmx3072m' not in low:
            continue
        if want_tmp and want_tmp not in low:
            continue
        return True
    return False


def _ensure_gradle_daemon(root, env, temp_dir=None):
    """Ensure a compatible Gradle daemon exists outside our Windows job object so gradlew runs
    inside dev.py's job (cancelling active builds if dev.py is killed) while the warm daemon
    survives normal exits. Returns False if a breakaway daemon could not be confirmed/started,
    signalling the caller to fall back to --no-daemon so no shared daemon spawns inside our job."""
    if os.name != 'nt':
        return True
    if _has_compatible_gradle_daemon(root, temp_dir=temp_dir):
        return True
    cmd = ['cmd', '/c', str(root / 'client' / 'gradlew.bat'), '--daemon', '-q', 'help']
    if temp_dir:
        cmd.insert(-1, f'-Djava.io.tmpdir={temp_dir}')
    flags = (
        getattr(subprocess, 'CREATE_BREAKAWAY_FROM_JOB', 0)
        | getattr(subprocess, 'CREATE_NO_WINDOW', 0)
        | getattr(subprocess, 'NORMAL_PRIORITY_CLASS', build_lock.NORMAL_PRIORITY_CLASS)
    )
    try:
        res = subprocess.run(cmd, cwd=root / 'client', env=env, stdin=subprocess.DEVNULL,
                             stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                             creationflags=flags, timeout=60)
        return res.returncode == 0
    except (OSError, subprocess.SubprocessError):
        return False


def run(command, log, cwd, env=None, shell=False, json_messages=False):
    with open(log, 'a', encoding='utf-8', errors='replace') as f:
        f.write(f'\n== {command if isinstance(command, str) else " ".join(map(str, command))}\n')
        f.flush()
        if not shell:
            # Windows looks the program up on this process's PATH, not env's (client_env adds cargo).
            program = shutil.which(command[0], path=(env or os.environ).get('PATH'))
            if not program:
                f.write(f'{command[0]} is not on PATH\n')
                return 127
            command = [program, *command[1:]]
        try:
            if not json_messages:
                return wait_child(subprocess.Popen(command, cwd=cwd, env=env, shell=shell, stdout=f, stderr=subprocess.STDOUT))
            with subprocess.Popen(command, cwd=cwd, env=env, shell=shell,
                                  stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                  text=True, encoding='utf-8', errors='replace') as proc:
                assert proc.stdout is not None
                for raw_line in proc.stdout:
                    rendered, _ = parse_cargo_json_line(raw_line)
                    if rendered:
                        f.write(rendered)
                return wait_child(proc)
        except OSError as error:
            f.write(f'{error}\n')
            return 127


def has_nextest(env=None):
    env = env or os.environ
    path = env.get('PATH')
    if shutil.which('cargo-nextest', path=path):
        return True
    cargo = shutil.which('cargo', path=path)
    if not cargo:
        return False
    try:
        res = subprocess.run([cargo, 'nextest', '--version'], env=env, capture_output=True, text=True, timeout=10)
        return res.returncode == 0
    except (OSError, subprocess.SubprocessError):
        return False


def nextest_args(root):
    cfg = root / NEXTEST_CONFIG
    return ['--config-file', str(cfg)] if cfg.is_file() else []


def doctest_crates(root, manifest_dir, crates):
    base = root / manifest_dir
    try:
        manifest = tomllib.loads((base / 'Cargo.toml').read_text(encoding='utf-8'))
    except (OSError, ValueError):
        return []
    members = manifest.get('workspace', {}).get('members')
    dirs = [base / m for m in members] if members is not None else [base]
    eligible = []
    by_name = {}
    for directory in dirs:
        try:
            pkg = tomllib.loads((directory / 'Cargo.toml').read_text(encoding='utf-8'))
        except (OSError, ValueError):
            continue
        name = pkg.get('package', {}).get('name')
        if not name:
            continue
        lib = pkg.get('lib') or {}
        if lib.get('doctest') is False:
            continue
        crate_types = lib.get('crate-type') or []
        if crate_types and all(ct in ('cdylib', 'staticlib') for ct in crate_types):
            continue
        lib_path = directory / (lib.get('path') or 'src/lib.rs')
        if lib_path.is_file():
            by_name[name] = True
    for crate in crates:
        if crate in by_name:
            eligible.append(crate)
    return eligible


def start_user_idle(root, env):
    idle = root / 'rust/release/user_idle.py'
    if not idle.is_file():
        return None
    try:
        out = subprocess.run([preferred_python(env), str(idle), 'begin'], cwd=root, env=env,
                             capture_output=True, text=True, timeout=10)
    except (OSError, subprocess.SubprocessError):
        return None
    if out.returncode != 0:
        return None
    parts = out.stdout.strip().split()
    return (parts[0], parts[1]) if len(parts) >= 2 else None


def end_user_idle(root, env, mark):
    if not mark:
        return
    idle = root / 'rust/release/user_idle.py'
    if not idle.is_file():
        return
    try:
        subprocess.run([preferred_python(env), str(idle), 'end', *mark], cwd=root, env=env,
                       capture_output=True, text=True, timeout=10)
    except (OSError, subprocess.SubprocessError):
        pass


def client_commands(root, crates, use_nextest=False, jobs=None, target=None):
    """cargo nextest/test and clippy for the changed client crates."""
    j = str(jobs if jobs is not None else build_lock.cargo_jobs())
    args = [a for crate in crates for a in ('-p', crate)]
    td = ['--target-dir', str(target)] if target is not None else []
    if use_nextest:
        commands = [['cargo', 'nextest', 'run', *nextest_args(root), '--build-jobs', j, '--locked', *td, *args]]
        doc_crates = doctest_crates(root, CLIENT_WORKSPACE, crates)
        if doc_crates:
            doc_args = [a for crate in doc_crates for a in ('-p', crate)]
            commands.append(['cargo', 'test', '--doc', '-j', j, '--locked', *td, *doc_args])
    else:
        commands = [['cargo', 'test', '-j', j, '--locked', *td, *args]]
    commands.append(['cargo', 'clippy', '-j', j, '--all-targets', '--locked', *td,
                     '--message-format=json', *args, '--', '-D', 'warnings'])
    return commands


_DISCOVER_SCRIPT = (
    'import json, sys, unittest\n'
    'start, patterns = sys.argv[1], sys.argv[2:]\n'
    'loader = unittest.TestLoader()\n'
    'seen, ordered = set(), []\n'
    'def walk(suite):\n'
    '    for item in suite:\n'
    '        if isinstance(item, unittest.TestSuite):\n'
    '            walk(item)\n'
    '        elif isinstance(item, unittest.TestCase):\n'
    '            cls = item.__class__\n'
    '            if cls.__module__ == "unittest.loader":\n'
    '                raise RuntimeError(item.id())\n'
    '            tid = f"{cls.__module__}.{cls.__qualname__}"\n'
    '            if tid not in seen:\n'
    '                seen.add(tid)\n'
    '                ordered.append(tid)\n'
    'for pat in patterns:\n'
    '    walk(loader.discover(start, pattern=pat))\n'
    'print(json.dumps(ordered))\n'
)


def _discover_unittest_targets(root, start_dir, patterns, env=None):
    """Discover runnable `module.TestCase` targets using unittest's own loader (exact handling of
    async tests, conditional classes, load_tests, imported bases, and 0-test helpers)."""
    py = preferred_python(env)
    child_env = dict(env or os.environ)
    child_env['PYTHONPATH'] = prepend_path(child_env.get('PYTHONPATH', ''), [str(root / start_dir)])
    try:
        res = subprocess.run([py, '-c', _DISCOVER_SCRIPT, str(root / start_dir), *patterns],
                             cwd=root, env=child_env, capture_output=True, text=True,
                             encoding='utf-8', errors='replace', timeout=30)
        if res.returncode == 0 and res.stdout.strip():
            data = json.loads(res.stdout.splitlines()[-1])
            if isinstance(data, list):
                return data
    except (OSError, subprocess.SubprocessError, ValueError):
        pass
    return None


def run_unittest_parallel(root, start_dir, patterns, log, env):
    """Run discovered unittest TestCase classes across parallel Python subprocesses."""
    py = preferred_python(env)
    targets = None if env.get('RUBYLIGHT_SERIAL_PYTEST', '').strip() == '1' else _discover_unittest_targets(root, start_dir, patterns, env)
    if targets is None or len(targets) <= 1:
        return all(run([py, '-m', 'unittest', 'discover', '-s', start_dir, '-p', pat],
                       log, root, env=env) == 0 for pat in patterns)
    workers = min(len(targets), max(2, min(4, (os.cpu_count() or 4) // 2)))
    child_env = dict(env)
    child_env['PYTHONPATH'] = prepend_path(child_env.get('PYTHONPATH', ''), [str(root / start_dir)])
    log_lock = threading.Lock()

    def run_target(target):
        cmd = [py, '-m', 'unittest', target]
        proc = subprocess.Popen(cmd, cwd=root, env=child_env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                text=True, encoding='utf-8', errors='replace')
        build_lock.lower_handle_priority(getattr(proc, '_handle', None))
        out, _ = proc.communicate()
        with log_lock, open(log, 'a', encoding='utf-8', errors='replace') as f:
            f.write(f'\n== {" ".join(cmd)}\n')
            if out:
                f.write(out)
                if not out.endswith('\n'):
                    f.write('\n')
        return proc.returncode == 0

    with concurrent.futures.ThreadPoolExecutor(max_workers=workers) as pool:
        results = list(pool.map(run_target, targets))
    return all(results)


def web_dependencies(web, log, refresh=False, env=None):
    """Reuse only installs made successfully by this checker, for these inputs."""
    stamp = web / 'node_modules/.rubylight-dependencies.json'
    versions = {}
    for program in ('node', 'npm'):
        result = subprocess.run(f'{program} --version', cwd=web, env=env, shell=True, capture_output=True, text=True)
        if result.returncode:
            return False
        versions[program] = result.stdout.strip()
    inputs = {name: hashlib.sha256((web / name).read_bytes()).hexdigest()
              for name in ('package.json', 'package-lock.json')}
    environ = build_lock.environ_copy()
    config = {k: v for k, v in environ.items() if k.lower().startswith('npm_config_')}
    receipt = dict(inputs=inputs, versions=versions, platform=sys.platform,
                   node_env=environ.get('NODE_ENV'),
                   npm_config=hashlib.sha256(json.dumps(config, sort_keys=True).encode()).hexdigest())
    configs = [web / '.npmrc', pathlib.Path.home() / '.npmrc']
    if environ.get('NPM_CONFIG_USERCONFIG'):
        configs.append(pathlib.Path(environ['NPM_CONFIG_USERCONFIG']))
    receipt['npmrc'] = [hashlib.sha256(p.read_bytes()).hexdigest() if p.exists() else None for p in configs]
    installed = web / 'node_modules/.package-lock.json'
    receipt['installed'] = hashlib.sha256(installed.read_bytes()).hexdigest() if installed.exists() else None
    try:
        if not refresh and receipt['installed'] and json.loads(stamp.read_text()) == receipt:
            return True
    except (OSError, ValueError):
        pass
    stamp.unlink(missing_ok=True)
    if run('npm ci --no-audit --no-fund', log, web, env=env, shell=True):
        return False
    receipt['installed'] = hashlib.sha256(installed.read_bytes()).hexdigest() if installed.exists() else None
    stamp.write_text(json.dumps(receipt, sort_keys=True), encoding='utf-8')
    return True


def execute(check, root, log, target, fresh_dependencies=False, fast=False, lock_directory=None):
    if not log.exists():
        log.write_text('', encoding='utf-8')
    if check.kind == 'host':
        env = host_env(target, lock_directory=lock_directory, fast=fast)
        jobs = str(build_lock.cargo_jobs(lock_directory))
        p_args = [a for c in check.items for a in ('-p', c)]
        if fast:
            cmd = ['cargo', 'check', *p_args, '--tests', '--locked', '-j', jobs,
                   '--target-dir', str(target), '--message-format=json']
            return run(cmd, log, root, env, json_messages=True) == 0
        if run(['cargo', 'fmt', '--', '--check'], log, root, env) != 0:
            return False
        clippy_cmd = ['cargo', 'clippy', *p_args, '--all-targets', '--locked', '-j', jobs,
                      '--target-dir', str(target), '--message-format=json', '--', '-D', 'warnings']
        if run(clippy_cmd, log, root, env, json_messages=True) != 0:
            return False
        use_nextest = has_nextest(env)
        if use_nextest:
            pre_cmd = ['cargo', 'nextest', 'run', *nextest_args(root), *p_args, '--locked',
                       '--build-jobs', jobs, '--target-dir', str(target), '--no-run']
        else:
            pre_cmd = ['cargo', 'test', *p_args, '--locked', '-j', jobs, '--target-dir', str(target), '--no-run']
        if run(pre_cmd, log, root, env) != 0:
            return False
        mark = start_user_idle(root, env)
        try:
            if use_nextest:
                test_cmd = ['cargo', 'nextest', 'run', *nextest_args(root), *p_args, '--locked',
                            '--build-jobs', jobs, '--target-dir', str(target)]
            else:
                test_cmd = ['cargo', 'test', *p_args, '--locked', '-j', jobs, '--target-dir', str(target)]
            ok = run(test_cmd, log, root, env) == 0
            if ok and use_nextest:
                doc_crates = doctest_crates(root, '.', check.items)
                if doc_crates:
                    doc_args = [a for c in doc_crates for a in ('-p', c)]
                    ok = run(['cargo', 'test', '--doc', *doc_args, '--locked', '-j', jobs,
                              '--target-dir', str(target)], log, root, env) == 0
            return ok
        finally:
            end_user_idle(root, env, mark)
    if check.kind == 'client':
        env = client_env(target, fast=fast, lock_directory=lock_directory)
        jobs = str(build_lock.cargo_jobs(lock_directory))
        if fast:
            args = [a for crate in check.items for a in ('-p', crate)]
            cmd = ['cargo', 'check', '-j', jobs, '--tests', '--locked', '--target-dir', str(target),
                   '--message-format=json', *args]
            return run(cmd, log, root / CLIENT_WORKSPACE, env, json_messages=True) == 0
        for command in client_commands(root, check.items, use_nextest=has_nextest(env), jobs=int(jobs), target=target):
            if run(command, log, root / CLIENT_WORKSPACE, env,
                   json_messages='--message-format=json' in command) != 0:
                return False
        return True
    if check.kind == 'service':
        # Its own workspace, in a folder of this worktree's target dir; without a
        # developer's linker flags, as its release build (rust/service/build.ps1).
        env = service_env(target, fast=fast, lock_directory=lock_directory)
        service_target = pathlib.Path(target) / 'service'
        td = ['--target-dir', str(service_target)]
        jobs = str(build_lock.cargo_jobs(lock_directory))
        manifest = ['--manifest-path', str(root / SERVICE_WORKSPACE / 'Cargo.toml')]
        if fast:
            cmd = ['cargo', 'check', '-j', jobs, '--tests', '--locked', *td, '--message-format=json', *manifest]
            return run(cmd, log, root, env, json_messages=True) == 0
        use_nextest = has_nextest(env)
        test_cmd = (['cargo', 'nextest', 'run', *nextest_args(root), '--build-jobs', jobs, '--locked', *td, *manifest]
                    if use_nextest else ['cargo', 'test', '-j', jobs, '--locked', *td, *manifest])
        commands = [
            ['cargo', 'fmt', *manifest, '--', '--check'],
            ['cargo', 'clippy', '-j', jobs, '--all-targets', '--locked', *td, '--message-format=json',
             *manifest, '--', '-D', 'warnings'],
            test_cmd,
        ]
        if use_nextest and doctest_crates(root, SERVICE_WORKSPACE, ['butterpollo-service']):
            commands.append(['cargo', 'test', '--doc', '-j', jobs, '--locked', *td, *manifest])
        return all(run(command, log, root, env, json_messages='--message-format=json' in command) == 0
                   for command in commands)
    if check.kind == 'interop':
        env = interop_env(target, fast=fast, lock_directory=lock_directory)
        td = ['--target-dir', str(target)]
        jobs = str(build_lock.cargo_jobs(lock_directory))
        manifest = ['--manifest-path', str(root / 'tests/interop/Cargo.toml')]
        if fast:
            cmd = ['cargo', 'check', '-j', jobs, '--tests', '--locked', *td, '--message-format=json', *manifest]
            return run(cmd, log, root, env, json_messages=True) == 0
        test_cmd = (['cargo', 'nextest', 'run', *nextest_args(root), '--build-jobs', jobs, '--locked', *td, *manifest]
                    if has_nextest(env) else ['cargo', 'test', '-j', jobs, '--locked', *td, *manifest])
        return run(test_cmd, log, root, env) == 0
    if check.kind == 'gradle':
        before = os.environ.get('TEMP'), os.environ.get('TMP')
        env = gradle_env(fast=fast, lock_directory=lock_directory, target=target)
        normalized = env.get('TEMP') if os.name == 'nt' and env.get('TEMP') and (env.get('TEMP'), env.get('TMP')) != before else None
        list_path = None
        try:
            try:
                command = gradle_command(root, check.items, temp_dir=normalized)
            except ValueError:
                fd, raw = tempfile.mkstemp(prefix='rubylight-tests-', suffix='.txt', dir=env.get('TEMP'))
                os.close(fd)
                list_path = pathlib.Path(raw)
                command = gradle_command(root, check.items, test_list_path=list_path, temp_dir=normalized)
            if '--no-daemon' not in command and not _ensure_gradle_daemon(root, env, temp_dir=normalized):
                command = [*command[:3], '--no-daemon', *command[3:]]
            return run(command, log, root / 'client', env) == 0
        finally:
            if list_path is not None:
                list_path.unlink(missing_ok=True)
    if check.kind == 'web':
        web = root / 'rust/web'
        env = tool_env(fast=fast, lock_directory=lock_directory)
        if not web_dependencies(web, log, fresh_dependencies, env=env):
            return False
        commands = ('npm run check',) if fast else ('npm run check', 'node --test "*.test.mjs"', 'npm run build')
        for command in commands:
            if run(command, log, web, env=env, shell=True):
                return False
        return True
    if check.kind == 'python':
        env = tool_env(fast=fast, lock_directory=lock_directory)
        return run_unittest_parallel(root, 'rust/release', check.items, log, env)
    if check.kind == 'tools':
        env = tool_env(fast=fast, lock_directory=lock_directory)
        return run_unittest_parallel(root, 'tools', ['test_*.py'], log, env)
    if check.kind == 'generated':
        env = tool_env(fast=fast, lock_directory=lock_directory)
        py = preferred_python(env)
        return run([py, str(root / 'tools/generated_docs.py'), '--check', *check.items],
                   log, root, env=env) == 0
    if check.kind == 'code_map':
        env = tool_env(fast=fast, lock_directory=lock_directory)
        py = preferred_python(env)
        return run([py, str(root / 'tools/code_map.py'), 'check'], log, root, env=env) == 0
    if check.kind == 'arch':
        env = tool_env(fast=fast, lock_directory=lock_directory)
        py = preferred_python(env)
        return run([py, str(root / 'tools/arch_check.py')], log, root, env=env) == 0
    raise ValueError(check.kind)


_ANSI_RE = re.compile(r'\x1b\[[0-9;]*[A-Za-z]')


def extract_failure_lines(kind, log_text):
    """Extract structured `file:line:col` diagnostics and failing test names from a check log."""
    lines = [_ANSI_RE.sub('', raw).rstrip('\r') for raw in log_text.splitlines()]
    out = []
    if kind in ('host', 'client', 'service', 'interop'):
        i = 0
        while i < len(lines):
            stripped = lines[i].strip()
            m_err = re.match(r'^error(?:\[([^\]]+)\])?:\s*(.+)$', stripped)
            if m_err and not m_err.group(2).startswith(('could not compile', 'aborting due to')):
                code = f'[{m_err.group(1)}]' if m_err.group(1) else ''
                msg = m_err.group(2)
                loc = None
                for j in range(i + 1, min(i + 4, len(lines))):
                    m_loc = re.match(r'^\s*-->\s+((?:[A-Za-z]:)?.+?:\d+:\d+)\s*$', lines[j])
                    if m_loc:
                        loc = m_loc.group(1)
                        break
                out.append(f'{loc}: error{code}: {msg}' if loc else f'error{code}: {msg}')
            elif re.match(r'^[^\s].*?\.(?:rs|c|cpp|h):\d+:\d+:\s*(?:error|warning)', stripped):
                out.append(stripped)
            elif m_fmt := re.match(r'^Diff in (.+?) at line (\d+):', stripped):
                out.append(f'{m_fmt.group(1)}:{m_fmt.group(2)}:1: rustfmt diff')
            elif re.match(r'^(?:FAIL|TIMEOUT|LEAK)\s+\[[^\]]+\]\s+\S+', stripped):
                out.append(stripped)
            elif m_test := re.match(r'^test\s+(\S+)\s+\.\.\.\s+FAILED$', stripped):
                out.append(f'FAILED test {m_test.group(1)}')
            elif m_panic := re.match(r"^thread '[^']+' panicked at ((?:[A-Za-z]:)?[^:]+:\d+:\d+):$", stripped):
                detail = lines[i + 1].strip() if i + 1 < len(lines) else ''
                out.append(f'{m_panic.group(1)}: panicked: {detail}' if detail else f'{m_panic.group(1)}: panicked')
            elif m_old_panic := re.match(r"^thread '[^']+' panicked at '(.+?)',\s*((?:[A-Za-z]:)?[^:]+:\d+:\d+)$", stripped):
                out.append(f'{m_old_panic.group(2)}: panicked: {m_old_panic.group(1)}')
            elif any(tok in stripped for tok in ('undefined reference to', 'ld.lld: error:', 'ld.lld.exe: error:', 'error: linking with')):
                out.append(stripped)
            i += 1
    elif kind == 'gradle':
        for i, line in enumerate(lines):
            stripped = line.strip()
            if m := re.match(r'^(.+?\.java):(\d+):\s*error:\s*(.+)$', stripped):
                out.append(f'{m.group(1)}:{m.group(2)}:1: error: {m.group(3)}')
            elif m := re.match(r'^e:\s*(?:file:///)?(.+?\.kt):(\d+):(\d+)\s+(.+)$', stripped):
                out.append(f'{m.group(1)}:{m.group(2)}:{m.group(3)}: error: {m.group(4)}')
            elif m := re.match(r'^e:\s*(.+?\.kt):\s*\((\d+),\s*(\d+)\):\s*(.+)$', stripped):
                out.append(f'{m.group(1)}:{m.group(2)}:{m.group(3)}: error: {m.group(4)}')
            elif m := re.match(r'^(.+?):(\d+):\s*Error:\s*(.+)$', stripped):
                out.append(f'{m.group(1)}:{m.group(2)}:1: lint error: {m.group(3)}')
            elif re.match(r'^\S.*?>\s*\S.*?\s+FAILED$', stripped):
                out.append(stripped)
            elif stripped == '* What went wrong:' and i + 1 < len(lines) and lines[i + 1].strip():
                out.append(lines[i + 1].strip())
    elif kind == 'web':
        i = 0
        while i < len(lines):
            stripped = lines[i].strip()
            if m := re.match(r'^([^\s].*?\.(?:svelte|ts|tsx|js|mjs)):(\d+):(\d+)\s+(Error|Warn):\s*(.+)$', stripped):
                out.append(f'{m.group(1)}:{m.group(2)}:{m.group(3)}: {m.group(4)}: {m.group(5)}')
            elif m := re.match(r'^([^\s].*?\.(?:svelte|ts|tsx|js|mjs)):(\d+):(\d+)$', stripped):
                if i + 1 < len(lines) and lines[i + 1].strip():
                    out.append(f'{m.group(1)}:{m.group(2)}:{m.group(3)}: {lines[i + 1].strip()}')
                else:
                    out.append(stripped)
            elif re.match(r'^(?:[✖✗]\s+|not ok \d+\s+-\s+)', stripped):
                out.append(stripped)
            elif m := re.search(r'at .*?\(([^()]+\.(?:svelte|ts|tsx|js|mjs):\d+:\d+)\)', stripped):
                out.append(f'{m.group(1)}: {stripped}')
            elif stripped.startswith(('error during build:', '[vite]')) or re.match(r'^file:\s*\S+:\d+:\d+', stripped):
                out.append(stripped)
            i += 1
    elif kind in ('python', 'tools', 'code_map', 'arch', 'generated'):
        pending_test = None
        for line in lines:
            stripped = line.strip()
            if m := re.match(r'^(?:FAIL|ERROR):\s+(.+)$', stripped):
                pending_test = stripped
                out.append(stripped)
            elif m := re.match(r'^File "([^"]+)", line (\d+),', stripped):
                if pending_test:
                    out.append(f'{m.group(1)}:{m.group(2)}:1: {pending_test}')
                    pending_test = None
                else:
                    out.append(f'{m.group(1)}:{m.group(2)}:1')
            elif m := re.match(r'^((?:[A-Za-z]:)?[^\s:]+\.(?:md|py|rs|java|kt|ts|tsx|js|mjs|svelte|toml|gradle|c|cpp|cc|h|hpp|hh|json))(?::(\d+)(?::(\d+))?)?:\s*(.+)$', stripped):
                ln = m.group(2) or '1'
                col = m.group(3) or '1'
                out.append(f'{m.group(1)}:{ln}:{col}: {m.group(4)}')
            elif stripped.startswith(('Stale:', 'AssertionError', 'RuntimeError:', 'SyntaxError:', 'ImportError:')):
                out.append(stripped)
    deduped = list(dict.fromkeys(out))
    if not deduped:
        tail = [line.strip() for line in lines if line.strip() and not line.startswith('== ')]
        deduped = tail[-10:]
    return deduped


def format_failure_digest(failed_items, max_lines=DIGEST_MAX_LINES):
    """Build a bounded (<= max_lines) diagnostic digest across failed checks."""
    if not failed_items or max_lines <= 0:
        return []
    blocks = []
    for check, log_path in failed_items:
        try:
            text = pathlib.Path(log_path).read_text(encoding='utf-8', errors='replace')
        except OSError:
            text = ''
        extracted = extract_failure_lines(check.kind, text)
        blocks.append((check.name, log_path, extracted))
    lines = []
    if len(blocks) == 1:
        name, log_path, extracted = blocks[0]
        lines.append(f'FAIL {name} ({log_path}):')
        lines.extend(f'  {item}' for item in extracted)
    else:
        per_block = max(2, (max_lines - 1) // len(blocks) - 1)
        overflow = []
        for name, log_path, extracted in blocks:
            lines.append(f'FAIL {name} ({log_path}):')
            shown = extracted[:per_block]
            lines.extend(f'  {item}' for item in shown)
            overflow.extend(f'  [{name}] {item}' for item in extracted[per_block:])
        lines.extend(overflow)
    if len(lines) > max_lines:
        omitted = len(lines) - (max_lines - 1)
        lines = lines[:max_lines - 1] + [f'... (+{omitted} more diagnostic lines in logs)']
    return lines


def check_lane(check):
    if check.kind in ('host', 'client', 'service', 'interop'):
        return 'cargo'
    if check.kind in ('python', 'tools'):
        return 'python'
    return check.kind


def main(argv=None):
    argv = sys.argv[1:] if argv is None else list(argv)
    if argv[:1] == ['cargo']:
        import dev_cargo  # it imports this module
        return dev_cargo.main(argv[1:])
    parser = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    parser.add_argument('command', choices=['check', 'claim-slot'])
    parser.add_argument('--slot', type=pathlib.Path, help="claim-slot: the slot's lock file, which the caller holds")
    parser.add_argument('--base', default='origin/main', help='compare against the merge base with this ref')
    parser.add_argument('--dry-run', action='store_true', help='print the plan only')
    parser.add_argument('--fast', action='store_true',
                        help='run static checks, cargo check --tests, and svelte-check only')
    parser.add_argument('--log-dir', type=pathlib.Path)
    parser.add_argument('--target-dir', type=pathlib.Path)
    parser.add_argument('--paths', nargs='*', help='plan for these paths instead of the changed files')
    parser.add_argument('--keep-going', action='store_true', help='run remaining independent checks after a failure')
    parser.add_argument('--fresh-dependencies', action='store_true', help='force npm ci for web checks')
    parser.add_argument('--lock-directory', type=pathlib.Path, help='shared machine lock directory')
    parser.add_argument('--lock-timeout', type=float, default=1800, help='maximum build wait in seconds')
    parser.add_argument('--pool', type=pathlib.Path,
                        help='warm target pool (default: C:\\src\\rl-target-pool with the machine locks, else beside them)')
    args = parser.parse_args(argv)
    if args.lock_timeout < 0:
        parser.error('--lock-timeout must be nonnegative')
    root = ROOT
    if args.command == 'claim-slot':
        if not args.slot:
            parser.error('claim-slot needs --slot <pool>\\<n>.lock')
        print(claim_slot(args.slot, root))
        return 0
    if not args.dry_run:
        build_lock.end_children_with_me()
    if args.paths is None:
        merge_base, paths = changed(args.base, root)
        print(f'{len(paths)} changed files against {args.base} (merge base {merge_base[:9]})')
    else:
        paths = args.paths
    checks, unchecked = plan(paths, root, fast=args.fast)
    where = str(args.target_dir) if args.target_dir else f'a warm slot of {args.pool or build_lock.pool_directory(args.lock_directory)}'
    run_id = datetime.datetime.now(datetime.timezone.utc).strftime('%Y%m%dT%H%M%SZ') + '-' + uuid.uuid4().hex[:8]
    base_log_root = (build_lock.lock_directory(args.lock_directory) / 'unified' / 'dev-check') if args.lock_directory else (
        LOG_ROOT if LOG_ROOT != _IMPORT_LOG_ROOT else default_log_root()
    )
    log_dir = (args.log_dir or base_log_root / worktree_name(root)) / run_id
    if unchecked:
        shown = ', '.join(unchecked[:8]) + (f' and {len(unchecked) - 8} more' if len(unchecked) > 8 else '')
        print(f'No check for {len(unchecked)} files (docs and the like): {shown}')
    if not checks:
        print('Nothing to check.')
        return 0
    if args.dry_run:
        print(f'Plan (Cargo target: {where}; logs in {log_dir}):')
        for check in checks:
            print(f'  {check.name:<16} {"SKIP" if check.skip else "run":<5} {check.describe()}')
        return 0
    log_dir.mkdir(parents=True, exist_ok=True)
    rows = [None] * len(checks)
    targets = {}
    print_lock = threading.Lock()
    targets_lock = threading.Lock()
    stop_event = threading.Event()
    cargo_limit = max(1, build_lock.cargo_slots())
    cargo_sem = threading.Semaphore(cargo_limit)

    def safe_say(*a, **k):
        with print_lock:
            print(*a, **k)

    def run_one(idx, check):
        log = log_dir / f'{check.name}.log'
        if stop_event.is_set() and not args.keep_going:
            rows[idx] = (check.name, 'NOT RUN', '', '', 'earlier check failed')
            return False
        safe_say(f'-- {check.name}: {check.describe()}', flush=True)
        log.write_text('', encoding='utf-8')
        waited, start = 0.0, time.monotonic()
        try:
            # Each stage takes its own lanes; pure Python/static suites take none.
            groups = stage_groups(check, root, args)
            if groups:
                with StageLease(groups, args.lock_directory, f'dev {check.name} {root}',
                                args.lock_timeout, say=safe_say) as lease:
                    if stop_event.is_set() and not args.keep_going:
                        rows[idx] = (check.name, 'NOT RUN', '', '', 'earlier check failed')
                        return False
                    waited, start = lease.waited, time.monotonic()
                    target = stage_target(check, lease.taken, root, args.target_dir)
                    if target is not None:
                        with targets_lock:
                            targets[check.name] = str(target)
                        with log.open('a', encoding='utf-8') as out:
                            out.write(f'== target {target}\n')
                    ok = execute(check, root, log, target, args.fresh_dependencies,
                                 fast=args.fast, lock_directory=args.lock_directory)
            else:
                ok = execute(check, root, log, None, args.fresh_dependencies,
                             fast=args.fast, lock_directory=args.lock_directory)
        except (OSError, RuntimeError) as error:
            with log.open('a', encoding='utf-8') as out:
                out.write(str(error) + '\n')
            ok = False
        if not ok and not args.keep_going:
            stop_event.set()
        rows[idx] = (check.name, 'PASS' if ok else 'FAIL', f'{time.monotonic() - start:.0f} s',
                     f'{waited:.0f} s' if waited >= 1 else '', check.describe() if ok else str(log))
        return ok

    def run_cargo(idx, check):
        if stop_event.is_set() and not args.keep_going:
            rows[idx] = (check.name, 'NOT RUN', '', '', 'earlier check failed')
            return False
        with cargo_sem:
            return run_one(idx, check)

    def run_lane(items):
        for idx, check in items:
            run_one(idx, check)

    cargo_items = []
    lane_queues = {}
    for idx, check in enumerate(checks):
        if check.skip:
            rows[idx] = (check.name, 'SKIP', '', '', check.describe())
            continue
        lane = check_lane(check)
        if lane == 'cargo':
            cargo_items.append((idx, check))
        else:
            lane_queues.setdefault(lane, []).append((idx, check))

    workers = max(1, len(lane_queues) + min(len(cargo_items), cargo_limit))
    with concurrent.futures.ThreadPoolExecutor(max_workers=workers) as pool:
        futures = [pool.submit(run_lane, items) for items in lane_queues.values()]
        futures += [pool.submit(run_cargo, idx, check) for idx, check in cargo_items]
        for future in futures:
            future.result()

    failed = any(row[1] == 'FAIL' for row in rows)
    print()
    print(f'{"check":<16} {"result":<7} {"time":>6} {"wait":>6}  detail')
    for name, result, took, wait, detail in rows:
        print(f'{name:<16} {result:<7} {took:>6} {wait:>6}  {detail}')
    if failed:
        failed_items = [(checks[idx], log_dir / f'{checks[idx].name}.log')
                        for idx, row in enumerate(rows) if row[1] == 'FAIL']
        digest = format_failure_digest(failed_items)
        if digest:
            print()
            for line in digest:
                print(line)
    print(f'Logs: {log_dir}')
    summary = dict(head=git('rev-parse', 'HEAD', root=root).strip(), worktree=str(root), paths=paths,
                   targets=targets, passed=not failed, checks=[dict(name=n, result=r, time=t, wait=w, detail=d)
                   for n, r, t, w, d in rows])
    (log_dir / 'summary.json').write_text(json.dumps(summary, indent=2) + '\n', encoding='utf-8')
    return 1 if failed else 0


if __name__ == '__main__':
    # A log redirected on Windows is cp1252: a test name or message outside it must not end the run.
    sys.stdout.reconfigure(errors='replace')
    sys.exit(main())
