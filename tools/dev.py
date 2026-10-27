"""Run the smallest set of checks for what changed in this worktree.

usage: python tools/dev.py check [--base REF] [--dry-run] [--log-dir DIR]
                                 [--target-dir DIR] [--paths PATH ...]

The changed files are those between the merge base of HEAD and --base
(default origin/main) and the working tree, plus untracked files. Each maps to
checks (see plan()):

- host workspace crates, and the crates that depend on them, to
  C:\\src\\rl-quick.ps1 (fmt, clippy, tests) with this worktree's target dir;
- client/protocol crates, and their dependants, to `cargo test` in Nightjar's
  workspace, plus the interop tests once tests/interop exists;
- client/app sources to the Gradle unit tests of the touched classes;
- rust/web to the web console checks;
- rust/release scripts to their unittest suites, tools/ to its own.

Markdown is documentation and maps to nothing. One PASS/FAIL table is printed;
each check's output goes to <log dir>/<check>.log. Exits 1 if any check fails.
"""
import argparse, dataclasses, os, pathlib, re, shutil, subprocess, sys, time, tomllib

ROOT = pathlib.Path(__file__).resolve().parents[1]
RL_QUICK = pathlib.Path(r'C:\src\rl-quick.ps1')
LOG_ROOT = pathlib.Path(r'C:\src\butterpollo-release\unified\dev-check')
CLIENT_WORKSPACE = 'client/protocol'
# Files that change how every host crate builds.
HOST_WIDE = ('Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml', 'clippy.toml', '.cargo/')
# The suites CI runs from rust/release.
RELEASE_SUITES = ('test_release.py', 'test_gpu_hang.py', 'test_timing_report.py', 'test_cadence_matrix.py')
RELEASE_INPUTS = ('rust/release/', 'tools/nightjar_notes.py', '.github/workflows/rust-windows.yml')
GRADLE_WIDE = ('client/build.gradle', 'client/settings.gradle', 'client/gradle.properties', 'client/gradle/',
               'client/app/build.gradle', 'client/app/proguard-rules.pro', 'client/app/src/main/res/',
               'client/app/src/main/AndroidManifest.xml')
# Client workspace files outside its members that change every member's build.
CLIENT_WIDE = ('client/protocol/Cargo.toml', 'client/protocol/Cargo.lock', 'client/protocol/vendor/')
GRADLE_TASK = ':app:testNonRootDebugUnitTest'


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


def plan(paths, root=ROOT):
    """The checks for a list of repository-relative paths, and the paths no check covers."""
    paths = sorted({posix(p) for p in paths})
    host, host_default = workspace(root, '.')
    client, client_default = workspace(root, CLIENT_WORKSPACE)
    # Directories outside the host workspace that host crates take by path (client/protocol/core).
    host_external = {d for _, deps in host.values() for d in deps if d not in host}
    interop = root / 'tests/interop'
    interop_in_host = 'tests/interop' in host

    host_dirs, client_dirs, client_all = set(), set(), False
    app_sources, gradle_all, web, release, tools, interop_hit, notes = [], False, False, set(), False, False, []
    for path in paths:
        hit = False
        if path.endswith('.md'):
            notes.append(path)
            continue
        if under(path, HOST_WIDE):
            host_dirs.update(host)
            hit = True
        d = owner(path, set(host) | host_external)
        if d:
            host_dirs.add(d)
            hit = True
            if d.startswith('rust/core') or d.startswith('rust/host'):
                interop_hit = True
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
        if path.startswith('rust/web/'):
            web = True
            hit = True
        if under(path, RELEASE_INPUTS):
            release.update(RELEASE_SUITES)
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
        if client_all:
            crates = sorted(client[d][0] for d in client_default)
        else:
            crates = sorted(client[d][0] for d in dependants(client, client_dirs))
        checks.append(Check('client-protocol', 'client', crates))
    if interop_hit and not interop_in_host:
        if (interop / 'Cargo.toml').is_file():
            checks.append(Check('interop', 'interop', ['tests/interop']))
        else:
            checks.append(Check('interop', 'interop', skip='tests/interop does not exist yet'))
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


def default_target(root=ROOT):
    """C:\\src\\rl-target-<topic> for a worktree C:\\src\\rl-<topic> (tools/wt.ps1), else the shared one."""
    name = worktree_name(root)
    return pathlib.Path(r'C:\src') / (f'rl-target-{name[3:]}' if name.startswith('rl-') else 'rl-target')


def client_env(target):
    env = dict(os.environ)
    env['PATH'] = r'C:\msys64\ucrt64\bin;' + str(pathlib.Path.home() / '.cargo/bin') + ';' + env.get('PATH', '')
    env['LIBCLANG_PATH'] = r'C:\msys64\ucrt64\bin'
    env['CARGO_TARGET_DIR'] = str(target)
    return env


def run(command, log, cwd, env=None, shell=False):
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
            return subprocess.run(command, cwd=cwd, env=env, shell=shell, stdout=f, stderr=subprocess.STDOUT).returncode
        except OSError as error:
            f.write(f'{error}\n')
            return 127


def client_specs(root, names):
    """Package ID specs: `-p rubylight-protocol` is ambiguous, the client workspace also has it from git."""
    by_name = {package: rel for rel, (package, _) in workspace(root, CLIENT_WORKSPACE)[0].items()}
    return [f'path+file:///{posix((root / by_name[name]).resolve())}#{name}' for name in names]


def stop_stray_vite(root, log):
    # Only this worktree's node processes: another worktree may be running its own console build.
    script = ("Get-CimInstance Win32_Process -Filter \"Name='node.exe'\" | "
              f"Where-Object {{ $_.CommandLine -match 'vite' -and $_.CommandLine -like '*{root}*' }} | "
              "ForEach-Object { Stop-Process -Id $_.ProcessId -Force; \"stopped $($_.ProcessId)\" }")
    run(['powershell', '-NoProfile', '-Command', script], log, root)


def read_quick(path):
    """rl-quick's log as text, and whether it ended with EXIT 0.

    Windows PowerShell appends cargo's output (`*>>`) as UTF-16LE between its own UTF-8
    lines, so the file mixes both: drop the NULs and byte order marks, keep the ASCII.
    """
    data = path.read_bytes() if path.exists() else b''
    text = data.replace(b'\x00', b'').decode('utf-8', errors='replace')
    text = text.replace('﻿', '').replace('��', '').replace('\r\n', '\n')
    return text, re.search(r'(?m)^EXIT 0\s*$', text) is not None


def execute(check, root, log, target):
    log.write_text('', encoding='utf-8')
    if check.kind == 'host':
        crates = ','.join(f"'{c}'" for c in check.items)
        command = (f"& '{RL_QUICK}' -Repo '{root}' -Crates {crates} -TargetDir '{target}' "
                   f"-Jobs 4 -Log '{log}.quick'")
        code = run(['powershell', '-NoProfile', '-ExecutionPolicy', 'Bypass', '-Command', command], log, root)
        quick = pathlib.Path(f'{log}.quick')
        text, passed = read_quick(quick)
        with open(log, 'a', encoding='utf-8') as f:
            f.write(text)
        quick.unlink(missing_ok=True)
        return code == 0 and passed
    if check.kind == 'client':
        args = [a for spec in client_specs(root, check.items) for a in ('-p', spec)]
        env = client_env(f'{target}-client')
        return run(['cargo', 'test', '-j', '4', *args], log, root / CLIENT_WORKSPACE, env) == 0
    if check.kind == 'interop':
        env = client_env(f'{target}-client')
        return run(['cargo', 'test', '-j', '4', '--manifest-path', str(root / 'tests/interop/Cargo.toml')],
                   log, root, env) == 0
    if check.kind == 'gradle':
        env = {k: v for k, v in os.environ.items() if k.upper() != 'CARGO_TARGET_DIR'}
        filters = [a for t in check.items for a in ('--tests', t)]
        return run(['cmd', '/c', 'gradlew.bat', '--no-daemon', '--max-workers=2', GRADLE_TASK, *filters],
                   log, root / 'client', env) == 0
    if check.kind == 'web':
        web = root / 'rust/web'
        stop_stray_vite(root, log)
        for command in ('npm ci --no-audit --no-fund', 'npm run check', 'node --test "*.test.mjs"', 'npm run build'):
            if run(command, log, web, shell=True):
                return False
        return True
    if check.kind == 'python':
        return all([run([sys.executable, '-m', 'unittest', 'discover', '-s', 'rust/release', '-p', suite],
                        log, root) == 0 for suite in check.items])
    if check.kind == 'tools':
        return run([sys.executable, '-m', 'unittest', 'discover', '-s', 'tools', '-p', 'test_*.py'], log, root) == 0
    raise ValueError(check.kind)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    parser.add_argument('command', choices=['check'])
    parser.add_argument('--base', default='origin/main', help='compare against the merge base with this ref')
    parser.add_argument('--dry-run', action='store_true', help='print the plan only')
    parser.add_argument('--log-dir', type=pathlib.Path)
    parser.add_argument('--target-dir', type=pathlib.Path)
    parser.add_argument('--paths', nargs='*', help='plan for these paths instead of the changed files')
    args = parser.parse_args(argv)
    root = ROOT
    if args.paths is None:
        merge_base, paths = changed(args.base, root)
        print(f'{len(paths)} changed files against {args.base} (merge base {merge_base[:9]})')
    else:
        paths = args.paths
    checks, unchecked = plan(paths, root)
    target = args.target_dir or default_target(root)
    log_dir = args.log_dir or LOG_ROOT / worktree_name(root)
    if unchecked:
        shown = ', '.join(unchecked[:8]) + (f' and {len(unchecked) - 8} more' if len(unchecked) > 8 else '')
        print(f'No check for {len(unchecked)} files (docs and the like): {shown}')
    if not checks:
        print('Nothing to check.')
        return 0
    if args.dry_run:
        print(f'Plan (target dir {target}, logs in {log_dir}):')
        for check in checks:
            print(f'  {check.name:<16} {"SKIP" if check.skip else "run":<5} {check.describe()}')
        return 0
    log_dir.mkdir(parents=True, exist_ok=True)
    rows, failed = [], False
    for check in checks:
        log = log_dir / f'{check.name}.log'
        if check.skip:
            rows.append((check.name, 'SKIP', '', check.describe()))
            continue
        print(f'-- {check.name}: {check.describe()}', flush=True)
        start = time.monotonic()
        ok = execute(check, root, log, target)
        failed |= not ok
        rows.append((check.name, 'PASS' if ok else 'FAIL', f'{time.monotonic() - start:.0f} s',
                     check.describe() if ok else str(log)))
    print()
    print(f'{"check":<16} {"result":<6} {"time":>6}  detail')
    for name, result, took, detail in rows:
        print(f'{name:<16} {result:<6} {took:>6}  {detail}')
    print(f'Logs: {log_dir}')
    return 1 if failed else 0


if __name__ == '__main__':
    sys.exit(main())
