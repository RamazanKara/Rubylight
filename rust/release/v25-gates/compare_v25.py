"""Rubylight 2.5 (runs-v25-1/2) against the 2.4.5 baseline (runs-base1/2): Task B's gate 3.

Picture age is the receiver's render_to_received (stages.receiver over frames.csv). Present to
sent comes from the host's `stream timings` lines (`present_to_sent_ms=mean/p95/p99` per 5 s
window): mean = frame-weighted mean of the window means, p95 = frame-weighted mean of the
window p95s. Noise per frame rate = the largest baseline pass-to-pass difference among that
rate's four cases, per metric. A case passes when, averaged over both passes, neither metric's
mean or p95 is above the baseline's by more than its rate's noise, no receiver check that
passed in both baseline passes fails, and dropped/overwritten counters are not higher.

usage: compare_v25.py [OUT.md]   (run with the test python)
"""
import json, pathlib, re, sys
sys.path.insert(0, r'C:\src\butterpollo-release\stage-latency')
import stages

ROOT = pathlib.Path(__file__).resolve().parent
CASES = [f'{codec}-{fps}' for fps in (60, 120, 240) for codec in ('hevc', 'av1', 'hevc-hdr', 'av1-hdr')]
RUNS = {'base': [('runs-base1', 'base', 1), ('runs-base2', 'base', 2)],
        'v25': [('runs-v25-1', 'v25', 1), ('runs-v25-2', 'v25', 2)]}


def present_to_sent(text):
    windows = [(int(frames), float(mean), float(p95)) for mean, p95, frames in re.findall(
        r'stream timings .*?present_to_sent_ms=([0-9.]+)/([0-9.]+)/[0-9.]+ .*?stage_frames=(\d+)', text)]
    total = sum(w[0] for w in windows)
    if not total:
        return None, None
    return (sum(f * m for f, m, _ in windows) / total, sum(f * p for f, _, p in windows) / total)


def load(runs, build, round_):
    out = {}
    for case in CASES:
        d = ROOT / runs / f'vd-{case}-{build}-r{round_}'
        result = json.loads((d / 'result.json').read_text()) if (d / 'result.json').exists() else {}
        log = d / 'config' / 'logs' / 'butterpollo.log'
        text = log.read_text(errors='replace') if log.exists() else ''
        counters = [tuple(int(x) for x in m) for m in re.findall(
            r'frame ring counters acquired=(\d+) pushed=(\d+) dropped=(\d+) overwritten=(\d+)', text)]
        receiver = stages.receiver(d / 'frames.csv') or {}
        rr = receiver.get('render_to_received') or {}
        p2s = present_to_sent(text)
        out[case] = dict(passed=result.get('passed'), failures=result.get('failures'),
                         age=(rr.get('mean'), rr.get('p95')), p2s=p2s,
                         dropped=sum(c[2] for c in counters), overwritten=sum(c[3] for c in counters))
    return out


base = [load(*r) for r in RUNS['base']]
new = [load(*r) for r in RUNS['v25']]
avg = lambda xs: sum(xs) / len(xs)
rate = lambda c: c.rsplit('-', 1)[1]
METRICS = ('age', 'p2s')
noise = {(m, r, i): max(abs(base[0][c][m][i] - base[1][c][m][i]) for c in CASES if rate(c) == r)
         for m in METRICS for r in ('60', '120', '240') for i in (0, 1)}
rows, ok = [], True
for c in CASES:
    b, n = [r[c] for r in base], [r[c] for r in new]
    deltas = {(m, i): avg([x[m][i] for x in n]) - avg([x[m][i] for x in b]) for m in METRICS for i in (0, 1)}
    latency_ok = all(deltas[(m, i)] <= noise[(m, rate(c), i)] for m in METRICS for i in (0, 1))
    checks_ok = not (all(x['passed'] for x in b) and not all(x['passed'] for x in n))
    counters_ok = sum(x['dropped'] for x in n) <= sum(x['dropped'] for x in b) and \
        sum(x['overwritten'] for x in n) <= sum(x['overwritten'] for x in b)
    case_ok = latency_ok and checks_ok and counters_ok
    ok &= case_ok
    rows.append((c, b, n, deltas, checks_ok, counters_ok, latency_ok, case_ok))

fmt = lambda xs, m: '; '.join(f'{x[m][0]:.2f} / {x[m][1]:.2f}' for x in xs)
lines = ['Noise per frame rate (largest 2.4.5 pass-to-pass difference), mean / p95:', '']
for m, name in (('age', 'picture age'), ('p2s', 'present to sent')):
    lines.append(f'- {name}: ' + ', '.join(f"{r} fps {noise[(m, r, 0)]:.2f} / {noise[(m, r, 1)]:.2f} ms" for r in ('60', '120', '240')))
lines += ['', '| Case | Picture age 2.4.5 (pass 1; 2) | Picture age 2.5 | Change | Present to sent 2.4.5 | Present to sent 2.5 | Change | Receiver checks | Dropped / overwritten (2.4.5; 2.5) | Verdict |',
          '| --- | --- | --- | ---: | --- | --- | ---: | --- | --- | --- |']
for c, b, n, d, checks_ok, counters_ok, latency_ok, case_ok in rows:
    checks = ' '.join('y' if x['passed'] else 'n' for x in b) + ' -> ' + ' '.join('y' if x['passed'] else 'n' for x in n)
    counters = f"{sum(x['dropped'] for x in b)}/{sum(x['overwritten'] for x in b)}; {sum(x['dropped'] for x in n)}/{sum(x['overwritten'] for x in n)}"
    lines.append(f"| {c} | {fmt(b, 'age')} | {fmt(n, 'age')} | {d[('age', 0)]:+.2f} / {d[('age', 1)]:+.2f} | "
                 f"{fmt(b, 'p2s')} | {fmt(n, 'p2s')} | {d[('p2s', 0)]:+.2f} / {d[('p2s', 1)]:+.2f} | {checks} | {counters} | "
                 f"{'pass' if case_ok else 'FAIL'} |")
for m, name in (('age', 'picture age'), ('p2s', 'present to sent')):
    lines.append('')
    lines.append(f"Mean change over all cases, {name}: mean {avg([r[3][(m, 0)] for r in rows]):+.2f} ms, p95 {avg([r[3][(m, 1)] for r in rows]):+.2f} ms")
failures = [f"{c}: {'; '.join(x['failures'] or [])}" for c, _, n, *_ in rows for x in n if not x['passed']]
if failures:
    lines += ['', 'Receiver failures on 2.5:', *[f'- {f}' for f in failures]]
lines += ['', f"GATE 3: {'PASS' if ok else 'FAIL'}"]
text = '\n'.join(lines)
print(text)
if len(sys.argv) > 1:
    pathlib.Path(sys.argv[1]).write_text(text + '\n')
