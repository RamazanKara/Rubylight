"""Fence ring against the 2.4.5 baseline from runs-base1/2 and runs-fence1/2.

Noise is the difference between the two baseline passes, case by case; a frame rate's noise
is the largest of those among its four cases (the 60 and 120 fps baselines swing by up to 4 ms
between passes, the 240 fps ones by 0.3 ms). A case's latency passes when its fence mean (and
p95), averaged over both passes, is not above the baseline's average by more than its frame
rate's noise. Receiver
checks: no case that passed in both baseline passes may fail in a fence pass. Driver
counters: dropped and overwritten summed over a case's passes must not exceed the baseline's.

usage: compare_fence.py [OUT.md]   (run with the test python)
"""
import json, pathlib, re, sys
sys.path.insert(0, r'C:\src\butterpollo-release\stage-latency')
import stages

ROOT = pathlib.Path(__file__).resolve().parent
CASES = [f'{codec}-{fps}' for fps in (60, 120, 240) for codec in ('hevc', 'av1', 'hevc-hdr', 'av1-hdr')]


def load(runs, build, round_):
    out = {}
    for case in CASES:
        d = ROOT / runs / f'vd-{case}-{build}-r{round_}'
        result = json.loads((d / 'result.json').read_text()) if (d / 'result.json').exists() else {}
        log = d / 'config' / 'logs' / 'butterpollo.log'
        text = log.read_text(errors='replace') if log.exists() else ''
        counters = [tuple(int(x) for x in m) for m in re.findall(
            r'frame ring counters acquired=(\d+) pushed=(\d+) dropped=(\d+) overwritten=(\d+)', text)]
        kind = re.search(r"frame push .* ring=(\w+)", text)
        receiver = stages.receiver(d / 'frames.csv') or {}
        rr = receiver.get('render_to_received') or {}
        out[case] = dict(passed=result.get('passed'), failures=result.get('failures'),
                         mean=rr.get('mean'), p95=rr.get('p95'),
                         dropped=sum(c[2] for c in counters), overwritten=sum(c[3] for c in counters),
                         ring=kind.group(1) if kind else None)
    return out


base = [load('runs-base1', 'base', 1), load('runs-base2', 'base', 2)]
fence = [load('runs-fence1', 'fence', 1), load('runs-fence2', 'fence', 2)]
avg = lambda xs: sum(xs) / len(xs)
rate = lambda c: c.rsplit('-', 1)[1]
noise = {r: (max(abs(base[0][c]['mean'] - base[1][c]['mean']) for c in CASES if rate(c) == r),
             max(abs(base[0][c]['p95'] - base[1][c]['p95']) for c in CASES if rate(c) == r))
         for r in ('60', '120', '240')}
rows, ok = [], True
for c in CASES:
    b, f = [r[c] for r in base], [r[c] for r in fence]
    bm, fm = avg([x['mean'] for x in b]), avg([x['mean'] for x in f])
    bp, fp = avg([x['p95'] for x in b]), avg([x['p95'] for x in f])
    checks_ok = not (all(x['passed'] for x in b) and not all(x['passed'] for x in f))
    counters_ok = sum(x['dropped'] for x in f) <= sum(x['dropped'] for x in b) and \
        sum(x['overwritten'] for x in f) <= sum(x['overwritten'] for x in b)
    latency_ok = fm - bm <= noise[rate(c)][0] and fp - bp <= noise[rate(c)][1]
    case_ok = checks_ok and counters_ok and latency_ok
    ok &= case_ok
    rows.append((c, b, f, bm, fm, bp, fp, checks_ok, counters_ok, latency_ok, case_ok))

lines = ['Noise per frame rate (largest baseline pass-to-pass difference), mean / p95: '
         + ', '.join(f'{r} fps {m:.2f} / {p:.2f} ms' for r, (m, p) in noise.items()), '',
         '| Case | Baseline mean / p95 (pass 1; pass 2) | Fence mean / p95 (pass 1; pass 2) | Change, mean / p95 | Baseline noise, mean / p95 | Receiver checks | Dropped / overwritten (base; fence) | Verdict |',
         '| --- | --- | --- | ---: | ---: | --- | --- | --- |']
for c, b, f, bm, fm, bp, fp, checks_ok, counters_ok, latency_ok, case_ok in rows:
    fmt = lambda xs: '; '.join(f"{x['mean']:.2f} / {x['p95']:.2f}" for x in xs)
    checks = ' '.join('y' if x['passed'] else 'n' for x in b) + ' -> ' + ' '.join('y' if x['passed'] else 'n' for x in f)
    counters = f"{sum(x['dropped'] for x in b)}/{sum(x['overwritten'] for x in b)}; {sum(x['dropped'] for x in f)}/{sum(x['overwritten'] for x in f)}"
    lines.append(f"| {c} | {fmt(b)} | {fmt(f)} | {fm - bm:+.2f} / {fp - bp:+.2f} | "
                 f"{abs(b[0]['mean'] - b[1]['mean']):.2f} / {abs(b[0]['p95'] - b[1]['p95']):.2f} | {checks} | {counters} | "
                 f"{'pass' if case_ok else 'FAIL'} |")
deltas_m = [fm - bm for _, _, _, bm, fm, *_ in rows]
deltas_p = [fp - bp for *_, bp, fp, _, _, _, _ in rows]
lines += ['', f'Mean change over all cases: mean {avg(deltas_m):+.2f} ms, p95 {avg(deltas_p):+.2f} ms',
          f"Rings: base {sorted({r[c]['ring'] for r in base for c in CASES} - {None})}, fence {sorted({r[c]['ring'] for r in fence for c in CASES} - {None})}",
          f"GATE 3: {'PASS' if ok else 'FAIL'}"]
text = '\n'.join(lines)
print(text)
if len(sys.argv) > 1:
    pathlib.Path(sys.argv[1]).write_text(text + '\n')
