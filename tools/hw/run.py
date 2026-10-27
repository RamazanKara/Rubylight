"""Run one device's hardware test cases from tools/hw/catalog.yaml in one batch.

usage: python tools/hw/run.py --batch DEVICE [--only ID ...] [--var NAME=VALUE ...]
                              [--dry-run] [--out-root DIR] [--catalog FILE]

Before each case its preconditions are checked on this PC (see the catalog's
header); a failed one stops the batch, as does exit code 3 (an AMD encoder
hang: never retried). Each case writes into
<out root>\\<date>\\<device>\\<id>\\ (output.log, the collected files and
stream-timings.txt, the `stream timings` lines the host logs wrote during the
case), and the batch ends with summary.md there. --dry-run checks the
preconditions and prints the plan without running anything or taking the
GPU lock. Exits 0 if every case that ran passed (or needs review), 1 if one
failed, 3 on an encoder hang.
"""
import argparse, ctypes, dataclasses, datetime, functools, glob, json, os, pathlib, re, shutil, subprocess, sys, time
import urllib.request

import yaml

ROOT = pathlib.Path(__file__).resolve().parents[2]
CATALOG = pathlib.Path(__file__).resolve().parent / 'catalog.yaml'
OUT_ROOT = pathlib.Path(r'C:\src\butterpollo-release\hw')
GPU_LOCK = pathlib.Path(r'C:\src\butterpollo-release\GPU-LOCK')
HOST_LOG = pathlib.Path(r'C:\ProgramData\Butterpollo\config\logs\butterpollo.log')
SERVERINFO = 'http://127.0.0.1:47989/serverinfo'
DEVICES = ('desktop', 'laptop', 'nuc', 'retroid', 'user')
PRECONDITIONS = ('idle', 'isolated-ports', 'system', 'gpu-lock')
TEST_PORTS = (48123, *range(48518, 48545))
IDLE_SECONDS = 600
EXIT_GPU_HANG = 3
PLACEHOLDER = re.compile(r'\{\{(\w+)\}\}')


class CatalogError(Exception):
    pass


@dataclasses.dataclass
class Case:
    id: str
    device: str
    title: str
    command: str
    preconditions: list
    passes: str
    output: str
    collect: list
    status: str = 'ready'
    verdict: str = 'exit'
    pass_regex: str = ''
    note: str = ''


def expand(text, values, where):
    def one(m):
        if m[1] not in values:
            raise CatalogError(f'{where}: unknown {{{{{m[1]}}}}}')
        return str(values[m[1]])
    for _ in range(5):  # vars may name other vars
        if not PLACEHOLDER.search(text):
            return text
        text = PLACEHOLDER.sub(one, text)
    raise CatalogError(f'{where}: placeholders nest too deep')


def load(text, overrides=None):
    """The catalog's vars and cases; fails on anything a batch would trip over later."""
    data = yaml.safe_load(text)
    if not isinstance(data, dict) or not isinstance(data.get('cases'), list):
        raise CatalogError('the catalog needs a cases list')
    variables = {k: str(v) for k, v in (data.get('vars') or {}).items()}
    variables.update(overrides or {})
    variables.setdefault('repo', str(ROOT))
    cases, seen = [], set()
    for i, raw in enumerate(data['cases']):
        where = f'case {raw.get("id", i)}' if isinstance(raw, dict) else f'case {i}'
        if not isinstance(raw, dict):
            raise CatalogError(f'{where}: not a mapping')
        missing = [k for k in ('id', 'device', 'title', 'command', 'preconditions', 'pass', 'output') if k not in raw]
        if missing:
            raise CatalogError(f'{where}: missing {", ".join(missing)}')
        unknown = set(raw) - {'id', 'device', 'title', 'command', 'preconditions', 'pass', 'output', 'collect',
                              'status', 'verdict', 'pass_regex', 'note'}
        if unknown:
            raise CatalogError(f'{where}: unknown fields {", ".join(sorted(unknown))}')
        if raw['id'] in seen:
            raise CatalogError(f'{where}: duplicate id')
        seen.add(raw['id'])
        if raw['device'] not in DEVICES:
            raise CatalogError(f'{where}: device {raw["device"]!r} is not one of {", ".join(DEVICES)}')
        pre = raw['preconditions'] or []
        bad = [p for p in pre if p not in PRECONDITIONS]
        if bad:
            raise CatalogError(f'{where}: unknown preconditions {", ".join(bad)}')
        status = raw.get('status', 'ready')
        if status not in ('ready', 'pending'):
            raise CatalogError(f'{where}: status {status!r} is not ready or pending')
        verdict = raw.get('verdict', 'exit')
        if verdict not in ('exit', 'review'):
            raise CatalogError(f'{where}: verdict {verdict!r} is not exit or review')
        if raw.get('pass_regex'):
            re.compile(raw['pass_regex'])
        # {{out}} is per case; check every other placeholder now.
        probe = dict(variables, out='<out>')
        for field in ('command', 'output'):
            expand(str(raw[field]), probe, f'{where} {field}')
        for path in raw.get('collect') or []:
            expand(str(path), probe, f'{where} collect')
        cases.append(Case(raw['id'], raw['device'], str(raw['title']), str(raw['command']), list(pre),
                          str(raw['pass']), str(raw['output']), [str(c) for c in raw.get('collect') or []],
                          status, verdict, raw.get('pass_regex') or '', str(raw.get('note', ''))))
    return variables, cases


def select(cases, device, only=None):
    """The cases to run and the ones skipped, with why."""
    run, skipped = [], []
    if only:
        known = {c.id for c in cases}
        wrong = [i for i in only if i not in known]
        if wrong:
            raise CatalogError(f'no case {", ".join(wrong)} in the catalog')
    for case in cases:
        if case.device != device or (only and case.id not in only):
            continue
        if case.device == 'user':
            skipped.append((case, 'MANUAL', 'the user runs it: ' + case.command))
        elif case.status == 'pending' and not only:
            skipped.append((case, 'PENDING', case.note or 'no runnable command yet'))
        else:
            run.append(case)
    return run, skipped


# Probes of this PC. Each returns (ok, detail); tests replace them.

def probe_serverinfo():
    try:
        with urllib.request.urlopen(SERVERINFO, timeout=3) as response:
            text = response.read().decode('utf-8', 'replace')
    except OSError as e:
        return True, f'installed host not reachable ({e.__class__.__name__}): no stream to interrupt'
    counts = {k: re.search(rf'<{k}>(\d+)</{k}>', text) for k in ('RustHostSessionCount', 'RustHostPendingSessionCount')}
    if not all(counts.values()):
        return False, 'serverinfo has no RustHostSessionCount/RustHostPendingSessionCount'
    values = {k: int(m[1]) for k, m in counts.items()}
    return all(v == 0 for v in values.values()), ', '.join(f'{k} {v}' for k, v in values.items())


def probe_input_idle():
    class LastInput(ctypes.Structure):
        _fields_ = [('cbSize', ctypes.c_uint), ('dwTime', ctypes.c_uint)]
    info = LastInput(ctypes.sizeof(LastInput), 0)
    if not ctypes.windll.user32.GetLastInputInfo(ctypes.byref(info)):
        return False, 'GetLastInputInfo failed'
    idle = ((ctypes.windll.kernel32.GetTickCount() - info.dwTime) & 0xFFFFFFFF) / 1000
    return idle >= IDLE_SECONDS, f'no input for {idle / 60:.1f} min (needs {IDLE_SECONDS // 60})'


def probe_fullscreen():
    user32 = ctypes.windll.user32
    hwnd = user32.GetForegroundWindow()
    if not hwnd:
        return True, 'no foreground window'
    name = ctypes.create_unicode_buffer(256)
    user32.GetClassNameW(hwnd, name, 256)
    if name.value in ('Progman', 'WorkerW', 'Shell_TrayWnd'):
        return True, 'the desktop is in front'

    class Rect(ctypes.Structure):
        _fields_ = [('left', ctypes.c_long), ('top', ctypes.c_long), ('right', ctypes.c_long), ('bottom', ctypes.c_long)]

    class MonitorInfo(ctypes.Structure):
        _fields_ = [('cbSize', ctypes.c_uint), ('rcMonitor', Rect), ('rcWork', Rect), ('dwFlags', ctypes.c_uint)]
    window = Rect()
    user32.GetWindowRect(hwnd, ctypes.byref(window))
    monitor = MonitorInfo()
    monitor.cbSize = ctypes.sizeof(MonitorInfo)
    user32.GetMonitorInfoW(user32.MonitorFromWindow(hwnd, 2), ctypes.byref(monitor))
    m = monitor.rcMonitor
    full = window.left <= m.left and window.top <= m.top and window.right >= m.right and window.bottom >= m.bottom
    title = ctypes.create_unicode_buffer(256)
    user32.GetWindowTextW(hwnd, title, 256)
    return not full, (f'fullscreen foreground app: {title.value or name.value}' if full
                      else f'foreground window is not fullscreen ({title.value or name.value})')


def bound_ports(netstat):
    """Local ports with a TCP listener or a UDP socket in `netstat -ano` output."""
    ports = set()
    for line in netstat.splitlines():
        parts = line.split()
        # A listener's remote address is 0.0.0.0:0 or [::]:0; the state column is localized.
        if len(parts) >= 4 and (parts[0] == 'UDP' or parts[0] == 'TCP' and parts[2] in ('0.0.0.0:0', '[::]:0')):
            port = parts[1].rsplit(':', 1)[-1]
            if port.isdigit():
                ports.add(int(port))
    return ports


def probe_ports():
    # One netstat instead of connect probes: a refused loopback connect takes about a second on Windows.
    netstat = subprocess.run(['netstat', '-ano'], capture_output=True, text=True, errors='replace')
    if netstat.returncode:
        return False, f'netstat failed with exit code {netstat.returncode}'
    busy = sorted(bound_ports(netstat.stdout) & set(TEST_PORTS))
    return not busy, (f'in use: {", ".join(map(str, busy))} (a test host still running?)' if busy
                      else 'test ports free')


def probe_system_task():
    result = subprocess.run(['schtasks', '/Query', '/TN', 'ButterpolloReleaseElevated'], capture_output=True)
    return result.returncode == 0, ('ButterpolloReleaseElevated task registered' if result.returncode == 0
                                    else 'no ButterpolloReleaseElevated task: run rust/release/elevation.ps1 elevated')


def probe_gpu_lock(lock=None, ours=None):
    lock = lock or GPU_LOCK
    if not lock.exists():
        return True, f'{lock} free'
    holder = lock.read_text(encoding='utf-8', errors='replace').strip()
    if ours and holder == ours:
        return True, 'held by this batch'
    return False, f'{lock} held: {holder or "(empty)"}'


def probes(lock_owner=None):
    return {
        'idle': (probe_serverinfo, probe_input_idle, probe_fullscreen),
        'isolated-ports': (probe_ports,),
        'system': (probe_system_task,),
        'gpu-lock': (lambda: probe_gpu_lock(ours=lock_owner),),
    }


def check(preconditions, available=None):
    """[(precondition, ok, detail)] in catalog order; a probe that raises fails its precondition."""
    available = available or probes()
    results = []
    for name in preconditions:
        for probe in available[name]:
            try:
                ok, detail = probe()
            except Exception as e:  # a broken probe must not read as a pass
                ok, detail = False, f'probe failed: {e!r}'
            results.append((name, ok, detail))
    return results


def verdict(case, code, output):
    if code == EXIT_GPU_HANG:
        return 'HANG'
    if case.verdict == 'review':
        return 'REVIEW'
    if code != 0:
        return 'FAIL'
    if case.pass_regex and not re.search(case.pass_regex, output):
        return 'FAIL'
    return 'PASS'


def batch_dir(out_root, device, today):
    base = out_root / today / device
    n, candidate = 1, base
    while (candidate / 'summary.md').exists():
        n += 1
        candidate = base.with_name(f'{device}-{n}')
    return candidate


def log_mark(path):
    """Where a log ends now, and how it begins, to tell appended lines from a rotated log."""
    try:
        with open(path, 'rb') as f:
            return f.seek(0, 2), (f.seek(0), f.read(256))[1]
    except OSError:
        return 0, b''


def timing_lines(path, mark=(0, b'')):
    size, head = mark
    try:
        with open(path, 'rb') as f:
            data = f.read()
    except OSError:
        return []
    if len(data) >= size and data.startswith(head):
        data = data[size:]  # else rotated or new: all of it
    return [line for line in data.decode('utf-8', 'replace').splitlines() if 'stream timings' in line]


def collect(case, values, out):
    copied = []
    target = out / 'collected'
    for pattern in case.collect:
        for match in glob.glob(expand(pattern, values, case.id), recursive=True):
            source = pathlib.Path(match)
            if out in source.parents:
                continue
            target.mkdir(exist_ok=True)
            destination = target / source.name
            if source.is_dir():
                shutil.copytree(source, destination, dirs_exist_ok=True)
            else:
                shutil.copy2(source, destination)
            copied.append(source)
    return copied


def run_case(case, values, out, host_log=HOST_LOG):
    out.mkdir(parents=True, exist_ok=True)
    values = dict(values, out=str(out))
    command = expand(case.command, values, case.id)
    start = log_mark(host_log)
    began = time.monotonic()
    with open(out / 'output.log', 'w', encoding='utf-8', errors='replace') as log:
        code = subprocess.run(['powershell', '-NoProfile', '-ExecutionPolicy', 'Bypass', '-Command', command],
                              cwd=values['repo'], stdout=log, stderr=subprocess.STDOUT).returncode
    took = time.monotonic() - began
    output = (out / 'output.log').read_text(encoding='utf-8', errors='replace')
    copied = collect(case, values, out)
    timings = timing_lines(host_log, start)
    for log in sorted(out.rglob('butterpollo.log')):
        timings += [f'{log.relative_to(out)}: {line}' for line in timing_lines(log)]
    if timings:
        (out / 'stream-timings.txt').write_text('\n'.join(timings) + '\n', encoding='utf-8')
    results = []
    for path in sorted(out.rglob('result.json')):
        try:
            passed = json.loads(path.read_text(encoding='utf-8')).get('passed')
        except (OSError, ValueError, AttributeError):
            passed = 'unreadable'
        results.append((path.relative_to(out), passed))
    shots = sorted(p.relative_to(out) for p in out.rglob('*.png'))
    return dict(result=verdict(case, code, output), code=code, seconds=took, results=results,
                timings=len(timings), screenshots=shots, copied=copied)


def summary(device, directory, rows, skipped, stopped):
    lines = [f'# Hardware batch: {device}, {datetime.datetime.now():%Y-%m-%d %H:%M}', '',
             '| case | result | exit | time | result.json | stream timings | screenshots |',
             '|---|---|---|---|---|---|---|']
    for case, r in rows:
        results = ', '.join(f'{p} ({v})' for p, v in r['results']) or '-'
        lines.append(f'| {case.id} | {r["result"]} | {r["code"]} | {r["seconds"]:.0f} s | {results} | '
                     f'{r["timings"]} | {len(r["screenshots"])} |')
    for case, result, why in skipped:
        lines.append(f'| {case.id} | {result} | | | {why} | | |')
    if stopped:
        lines += ['', f'**Stopped:** {stopped}']
    lines += ['', '## Pass conditions', '']
    lines += [f'- **{case.id}** ({case.title}): {case.passes}' for case, _ in rows]
    (directory / 'summary.md').write_text('\n'.join(lines) + '\n', encoding='utf-8')


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    parser.add_argument('--batch', required=True, choices=DEVICES)
    parser.add_argument('--only', nargs='*')
    parser.add_argument('--var', action='append', default=[], help='NAME=VALUE, overrides the catalog vars')
    parser.add_argument('--dry-run', action='store_true')
    parser.add_argument('--out-root', type=pathlib.Path, default=OUT_ROOT)
    parser.add_argument('--catalog', type=pathlib.Path, default=CATALOG)
    args = parser.parse_args(argv)
    overrides = dict(v.split('=', 1) for v in args.var)
    try:
        values, cases = load(args.catalog.read_text(encoding='utf-8'), overrides)
        run, skipped = select(cases, args.batch, args.only)
    except CatalogError as e:
        raise SystemExit(f'{args.catalog}: {e}')
    directory = batch_dir(args.out_root, args.batch, f'{datetime.date.today():%Y-%m-%d}')
    print(f'{args.batch}: {len(run)} cases to run, {len(skipped)} skipped; results in {directory}')
    for case, result, why in skipped:
        print(f'  {result:<8} {case.id}: {why}')
    if args.dry_run:
        # Probe once for the whole plan.
        once = {name: tuple(functools.cache(p) for p in fns) for name, fns in probes().items()}
        for case in run:
            print(f'\n[{case.id}] {case.title}')
            for name, ok, detail in check(case.preconditions, once):
                print(f'  {"ok  " if ok else "FAIL"} {name}: {detail}')
            command = expand(case.command, dict(values, out=str(directory / case.id)), case.id)
            print('  command: ' + command.strip().replace('\n', '\n           '))
            print(f'  pass: {case.passes}')
        return 0
    if not run:
        return 0
    directory.mkdir(parents=True, exist_ok=True)
    owner = f'tools/hw/run.py --batch {args.batch} pid {os.getpid()} since {datetime.datetime.now():%Y-%m-%d %H:%M}'
    lock_taken, rows, stopped, code = False, [], '', 0
    try:
        for i, case in enumerate(run):
            failed = [(n, d) for n, ok, d in check(case.preconditions, probes(owner)) if not ok]
            if failed:
                stopped = f'before {case.id}: ' + '; '.join(f'{n}: {d}' for n, d in failed)
                skipped += [(c, 'NOT RUN', 'batch stopped') for c in run[i:]]
                code = max(code, 1)
                break
            if 'gpu-lock' in case.preconditions and not lock_taken:
                GPU_LOCK.write_text(owner, encoding='utf-8')
                lock_taken = True
            print(f'-- {case.id}: {case.title}', flush=True)
            result = run_case(case, values, directory / case.id)
            rows.append((case, result))
            print(f'   {result["result"]} (exit {result["code"]}, {result["seconds"]:.0f} s)', flush=True)
            if result['result'] == 'HANG':
                stopped = (f'{case.id} hit an AMD encoder hang (exit 3): stop, do not retry; '
                           'reboot if Device Manager shows the Radeon with Code 31')
                skipped += [(c, 'NOT RUN', 'batch stopped') for c in run[i + 1:]]
                code = EXIT_GPU_HANG
                break
            if result['result'] == 'FAIL':
                code = max(code, 1)
    finally:
        if lock_taken and probe_gpu_lock(ours=owner)[1] == 'held by this batch':
            GPU_LOCK.unlink()
        summary(args.batch, directory, rows, skipped, stopped)
    print(f'summary: {directory / "summary.md"}')
    if stopped:
        print(f'stopped {stopped}')
    return code


if __name__ == '__main__':
    sys.exit(main())
