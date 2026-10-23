"""B1 H1: the host's cadence error across stream rate, codec, VRR hint and game cadence (measure only).

A game cadence (motion_probe's animation script) is streamed through e2e.py --record on an isolated host; the host
record and the receiver's frames.csv are joined by timing_report.py, whose present_interval, host_cadence_error and
arrival_cadence_error rows are the result. Each case also checks that the host's present stamp is the probe's own
present (the probe's QPC against the record's present_us).

  python cadence_matrix.py list
  python cadence_matrix.py run CASE [CASE ...] --work DIR --package DIR --client EXE [--seconds 25] [--port 48823]
  python cadence_matrix.py report --work DIR [--markdown]

CASE is a name from `list`, `all`, or a group (`core`, `codecs`). The run holds C:\\src\\butterpollo-release\\GPU-LOCK
while it streams and waits for another holder; exit code 3 from e2e.py is an AMD encoder hang and stops the batch.
"""
import argparse, json, os, pathlib, subprocess, sys, time

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from soak_result import percentile  # noqa: E402

LOCK = pathlib.Path(r'C:\src\butterpollo-release\GPU-LOCK')
LOCK_NAME = 'H1'
E2E = pathlib.Path(__file__).resolve().parent / 'e2e.py'
EXIT_GPU_HANG = 3
WARMUP = 5
# The cadences a game can have, as motion_probe's animation argument.
CONTENT = {'fixed90': '90', 'jitter': 'jitter:8,14,9,13', 'fixed72': '72', 'hitch90': 'hitch:90:100'}
STAMP_WINDOW_US = 20_000


def case_name(codec, fps, vrr, content):
    return f'{codec}-{fps}{"-vrr" if vrr else ""}-{content}'


def cases():
    table = {}
    def add(group, codec, fps, vrr, content):
        name = case_name(codec, fps, vrr, content)
        table[name] = dict(name=name, group=group, codec=codec, fps=fps, vrr=vrr, content=content)
    # HEVC: every game cadence in every stream rate, with and without the VRR hint.
    for content in CONTENT:
        for fps in (90, 120, 144):
            for vrr in (False, True):
                add('core', 'hevc', fps, vrr, content)
    # The other codecs on the 90 fps game: equal rate and the 144 fps stream.
    for codec in ('h264', 'pyrowave'):
        for fps in (90, 144):
            for vrr in (False, True):
                add('codecs', codec, fps, vrr, 'fixed90')
    return table


# ---- stamp check --------------------------------------------------------------------------------

def stamp_check(probe_us, host_us, window_us=STAMP_WINDOW_US):
    """Match each host present stamp to the probe's latest present before it (within the window) and report the lag
    and how well the host's gaps follow the probe's gaps. Both lists are QPC microseconds."""
    probe = sorted(probe_us)
    pairs, index, used = [], 0, set()
    for stamp in sorted(host_us):
        while index + 1 < len(probe) and probe[index + 1] <= stamp:
            index += 1
        if index < len(probe) and 0 <= stamp - probe[index] <= window_us and index not in used:
            used.add(index)
            pairs.append((index, stamp - probe[index], stamp))
    lags = [lag for _, lag, _ in pairs]
    gap_errors = [abs((b[2] - a[2]) - (probe[b[0]] - probe[a[0]])) for a, b in zip(pairs, pairs[1:]) if b[0] == a[0] + 1]
    ms = lambda values, p: None if not values else percentile(values, p) / 1000
    return dict(host_stamps=len(host_us), probe_presents=len(probe), matched=len(pairs), lag_p50_ms=ms(lags, 50),
                lag_p99_ms=ms(lags, 99), gap_error_p50_ms=ms(gap_errors, 50), gap_error_p99_ms=ms(gap_errors, 99),
                gap_pairs=len(gap_errors))


def case_stamp_check(case):
    case = pathlib.Path(case)
    motion = json.loads((case / 'motion.json').read_text())
    frequency = motion['qpc_frequency']
    probe = [f['qpc'] * 1e6 / frequency for f in motion['frames']]
    import timing_report as tr
    record = tr.read_host_record(case / 'record')
    if record['epoch_qpc_ms'] is None:
        raise ValueError('the host record has no media_epoch_qpc_ms')
    base = record['epoch_qpc_ms'] * 1000
    first = min(probe)
    host = [r['present'] + base for r in record['rows'].values() if r['present'] is not None]
    # Only the stream's own span: the probe also ran before and after it.
    last = max(host, default=first)
    probe = [t for t in probe if min(host, default=first) - STAMP_WINDOW_US <= t <= last]
    return stamp_check(probe, host)


# ---- a batch ------------------------------------------------------------------------------------

def take_lock(wait_minutes):
    deadline = time.monotonic() + wait_minutes * 60
    while True:
        try:
            with LOCK.open('x') as f:
                f.write(f'{LOCK_NAME} {time.strftime("%Y-%m-%dT%H:%M:%S")}')
            return
        except FileExistsError:
            if time.monotonic() > deadline:
                raise SystemExit(f'GPU-LOCK still held: {LOCK.read_text(errors="replace")}')
            time.sleep(5)


def release_lock():
    if LOCK.exists() and LOCK.read_text(errors='replace').startswith(LOCK_NAME):
        LOCK.unlink()


def run_case(spec, args):
    work = pathlib.Path(args.work)
    mode = f'2560x720x{spec["fps"]}'  # the 32:9 desktop's aspect ratio, as the B4 cases
    pyrowave = spec['codec'] == 'pyrowave'
    client = args.pyrowave_client if pyrowave and args.pyrowave_client else args.client
    bitrate = args.pyrowave_bitrate if pyrowave else args.bitrate
    command = [sys.executable, str(E2E), '--package', args.package, '--work', str(work), '--client', client,
               '--codec', spec['codec'], '--mode', mode, '--seconds', str(args.seconds), '--record',
               '--skip-first', str(WARMUP), '--name', spec['name'], '--motion-script', CONTENT[spec['content']],
               '--port', str(args.port), '--bitrate', str(bitrate)]
    if spec['vrr']:
        command.append('--vrr')
    for line in args.config:
        command += ['--config', line]
    result = subprocess.run(command, capture_output=True, text=True, errors='replace', env=dict(os.environ, NO_PROXY='*'))
    return result.returncode, result.stdout[-300:] + result.stderr[-600:]


def summarize_case(case_dir, spec):
    result = json.loads((pathlib.Path(case_dir) / 'result.json').read_text())
    timing = result.get('timing') or {}
    row = dict(spec, passed=result.get('passed'), skipped=result.get('skipped'), error=timing.get('error'))
    for name, stage in (timing.get('stages') or {}).items():
        row[name] = {k: stage[k] for k in ('n', 'median_ms', 'p95_ms', 'p99_ms')}
    try:
        row['stamp'] = case_stamp_check(case_dir)
    except (OSError, ValueError, KeyError) as error:
        row['stamp'] = dict(error=str(error))
    return row


def run(args):
    work = pathlib.Path(args.work)
    work.mkdir(parents=True, exist_ok=True)
    table = cases()
    names = []
    for item in args.cases:
        found = [n for n, c in table.items() if item in ('all', n) or c['group'] == item]
        if not found:
            raise SystemExit(f'unknown case or group {item!r}; see `list`')
        names += [n for n in found if n not in names]
    take_lock(args.wait)
    try:
        for name in names:
            started = time.monotonic()
            code, tail = run_case(table[name], args)
            case_dir = work / f'e2e-{name}'
            if code == EXIT_GPU_HANG:
                print(f'{name}: AMD encoder hang (exit 3); stopping, no retry', file=sys.stderr)
                return EXIT_GPU_HANG
            if not (case_dir / 'result.json').exists():
                print(f'{name}: no result (exit {code}): {tail}', file=sys.stderr)
                continue
            row = summarize_case(case_dir, table[name])
            with (work / 'results.jsonl').open('a') as out:
                out.write(json.dumps(row) + '\n')
            print(f'{name}: exit {code}, {time.monotonic() - started:.0f} s')
    finally:
        release_lock()
    return 0


# ---- report -------------------------------------------------------------------------------------

COLUMNS = (('present_interval', 'game interval'), ('host_cadence_error', 'host error'),
           ('arrival_cadence_error', 'arrival error'))


def cell(row, stage, keys=('median_ms', 'p99_ms')):
    data = row.get(stage)
    if not data or not data['n']:
        return '-'
    return ' / '.join(f'{data[k]:.2f}' for k in keys)


def table_lines(rows, markdown=True):
    head = ['case', 'game ms p50/p99', 'host error ms p50/p99', 'arrival error ms p50/p99', 'pairs', 'stamp lag ms p50/p99',
            'stamp gap error ms p50/p99']
    lines = []
    for row in rows:
        stamp = row.get('stamp') or {}
        lag = '-' if stamp.get('lag_p50_ms') is None else f'{stamp["lag_p50_ms"]:.2f} / {stamp["lag_p99_ms"]:.2f}'
        gap = '-' if stamp.get('gap_error_p50_ms') is None else f'{stamp["gap_error_p50_ms"]:.2f} / {stamp["gap_error_p99_ms"]:.2f}'
        pairs = (row.get('host_cadence_error') or {}).get('n', 0)
        lines.append([row['name'], cell(row, 'present_interval'), cell(row, 'host_cadence_error'),
                      cell(row, 'arrival_cadence_error'), str(pairs), lag, gap])
    if markdown:
        out = ['| ' + ' | '.join(head) + ' |', '|' + ' --- |' * len(head)]
        return out + ['| ' + ' | '.join(r) + ' |' for r in lines]
    widths = [max(len(r[i]) for r in [head, *lines]) for i in range(len(head))]
    return ['  '.join(c.ljust(w) for c, w in zip(r, widths)).rstrip() for r in [head, *lines]]


def read_rows(work):
    path = pathlib.Path(work) / 'results.jsonl'
    latest = {}
    for line in path.read_text().splitlines():
        if line.strip():
            row = json.loads(line)
            latest[row['name']] = row
    return list(latest.values())


def report(args):
    print('\n'.join(table_lines(read_rows(args.work), args.markdown)))
    return 0


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split('\n\n')[0])
    sub = parser.add_subparsers(dest='command', required=True)
    sub.add_parser('list')
    r = sub.add_parser('run')
    r.add_argument('cases', nargs='+')
    r.add_argument('--work', required=True)
    r.add_argument('--package', required=True)
    r.add_argument('--client', required=True)
    r.add_argument('--pyrowave-client', help='the PyroWave receiver (moonlight-pyrowave-client.exe) for the pyrowave cases')
    r.add_argument('--pyrowave-bitrate', type=int, default=200000, help='kbps')
    r.add_argument('--seconds', type=int, default=25)
    r.add_argument('--port', type=int, default=48823)
    r.add_argument('--bitrate', type=int, default=30000, help='kbps')
    r.add_argument('--wait', type=int, default=60, help='minutes to wait for GPU-LOCK')
    r.add_argument('--config', action='append', default=[], metavar='KEY=VALUE')
    p = sub.add_parser('report')
    p.add_argument('--work', required=True)
    p.add_argument('--markdown', action='store_true')
    args = parser.parse_args(argv)
    if args.command == 'list':
        for spec in cases().values():
            print(f'{spec["name"]:<28}{spec["group"]}')
        return 0
    return run(args) if args.command == 'run' else report(args)


if __name__ == '__main__':
    sys.exit(main())
