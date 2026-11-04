"""timings.py CSV...: per run (the CSV's folder), decode (input_to_output_ms: decoder input to
output), decode-to-present (decode_to_present_ms) and receive-to-present p50/p95 in ms, skipping
the first 5 s, plus rows, statuses and present drops. NIGHTJAR_TIMING_CSV writes the CSVs."""
import collections
import csv
import os
import sys


def pct(v, p):
    v = sorted(v)
    return v[min(len(v) - 1, round(p / 100 * (len(v) - 1)))] if v else float('nan')


for path in sys.argv[1:]:
    name = os.path.basename(os.path.dirname(path))
    try:
        rows = list(csv.DictReader(open(path)))
    except OSError as e:
        print(f'{name}  no timings: {e.strerror}')
        continue
    if not rows:
        print(f'{name}  no rows')
        continue
    t0 = int(rows[0]['receive_ns'] or 0)
    rows = [r for r in rows if r['receive_ns'] and int(r['receive_ns']) - t0 > 5e9]
    out = [name, f'rows {len(rows)}']
    for col in ('input_to_output_ms', 'decode_to_present_ms', 'receive_to_present_ms'):
        v = [float(r[col]) for r in rows if r.get(col)]
        out.append(f'{col} {pct(v, 50):.2f}/{pct(v, 95):.2f}')
    drops = [int(r['present_drops']) for r in rows if r.get('present_drops')]
    out.append(f'present_drops {max(drops) - min(drops) if drops else "-"}')
    out.append(str(dict(collections.Counter(r['status'] for r in rows))))
    print('  '.join(out))
