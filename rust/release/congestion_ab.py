"""Congestion control A/B: reads the legs of a run and applies the decision rule for turning
`congestion_control` on by default.

A leg is one 30-60 s stream in its own directory:

* `host.log`: the host log of that stream (the isolated test host's `butterpollo.log`, or the slice of
  it for the leg). Its 5 s `stream timings` lines carry the `cc_*` fields and the stream's end a
  `congestion summary` line.
* `client.json`: what the client measured, normalised by whoever ran the leg:
  `{"device": "retroid", "latency_ms": {"p50": 31.0, "p95": 48.0}, "fps": 59.8, "dropped": 12}`.
  `dropped` is frames lost or dropped over the leg (network plus jitter drops, as the overlay counts
  them); `arm` may override the arm read from the host log.

The arm comes from the host log: `cc_shadow=true` is the baseline (today's controllers, with the
congestion controller only measuring, so the queueing delay of the baseline is known too),
`cc_state=off` a baseline without measurement, any other state the treatment (`on`). Run the
baseline with `BUTTERPOLLO_CONGESTION_SHADOW=1` on the isolated test host, the treatment with
`congestion_control = true`, alternating legs (baseline, on, baseline, on, ...), at least two each.

The rule, per device: turn it on only if the treatment's mean latency p50 and p95 are lower than the
baseline's by more than the leg-to-leg spread (the larger arm's max - min), its frame rate is not
lower (beyond FPS_TOLERANCE, the clients' rounding) and it has no more drops. All devices must pass.
Delivered bitrate, queueing delay, cuts and recovery time are reported beside it, not gated on.

Run: python rust/release/congestion_ab.py <leg dir>... [--json out.json]
Tests: python -m unittest discover -s rust/release -p test_congestion_ab.py
"""
import argparse, json, pathlib, re, sys

FIELD = re.compile(r'(\w+)=("[^"]*"|\S+)')
ANSI = re.compile(r'\x1b\[[0-9;]*m')
FPS_TOLERANCE = 0.5
MIN_LEGS = 2


def fields(line):
    return {k: v.strip('"') for k, v in FIELD.findall(ANSI.sub('', line))}


def number(value):
    try:
        return float(value)
    except (TypeError, ValueError):
        return None


def mean(values):
    values = [v for v in values if v is not None]
    return sum(values) / len(values) if values else None


def host_leg(text):
    """One leg's host log: the arm and the congestion controller's numbers. The first 5 s window is
    warm-up and left out of the means."""
    lines = ANSI.sub('', text).splitlines()
    windows = [fields(line) for line in lines if 'stream timings' in line][1:]
    summary = [fields(line) for line in lines if 'congestion summary' in line]
    changes = [fields(line) for line in lines if 'congestion control' in line and ' kind=' in line]
    states = [w.get('cc_state', 'off') for w in windows]
    shadow = any(w.get('cc_shadow') == 'true' for w in windows)
    if shadow:
        arm = 'baseline'
    elif not states or all(s == 'off' for s in states):
        arm = 'baseline'
    else:
        arm = 'on'
    of = lambda key: [number(w.get(key)) for w in windows]
    last = summary[-1] if summary else {}
    return dict(
        arm=arm,
        shadow=shadow,
        windows=len(windows),
        queue_p50_ms=mean(of('cc_queue_p50_ms')),
        queue_p95_ms=mean(of('cc_queue_p95_ms')),
        target_kbps=mean(of('cc_target_kbps')),
        delivered_kbps=mean(of('cc_acked_kbps')),
        overuse_cuts=number(last.get('overuse_cuts')),
        loss_cuts=number(last.get('loss_cuts')),
        recovery_p50_ms=number(last.get('recovery_p50_ms')),
        recovery_max_ms=number(last.get('recovery_max_ms')),
        changes=len(changes),
    )


def read_leg(directory):
    directory = pathlib.Path(directory)
    host = host_leg((directory / 'host.log').read_text(encoding='utf-8', errors='replace'))
    client = json.loads((directory / 'client.json').read_text(encoding='utf-8'))
    return leg(directory.name, host, client)


def leg(name, host, client):
    latency = client.get('latency_ms') or {}
    return dict(
        name=name,
        device=client.get('device', 'device'),
        arm=client.get('arm', host['arm']),
        latency_p50_ms=number(latency.get('p50')),
        latency_p95_ms=number(latency.get('p95')),
        fps=number(client.get('fps')),
        dropped=number(client.get('dropped')),
        host=host,
    )


def spread(values):
    values = [v for v in values if v is not None]
    return max(values) - min(values) if len(values) >= 2 else None


def compare(legs):
    """One device's legs: the arms' means and spreads, and whether the treatment passes."""
    arms = {arm: [l for l in legs if l['arm'] == arm] for arm in ('baseline', 'on')}
    reasons = []
    for arm, group in arms.items():
        if len(group) < MIN_LEGS:
            reasons.append(f'{len(group)} {arm} legs, need {MIN_LEGS}')
    summary = {}
    for key in ('latency_p50_ms', 'latency_p95_ms', 'fps', 'dropped'):
        summary[key] = {arm: mean([l[key] for l in group]) for arm, group in arms.items()}
        spreads = [spread([l[key] for l in group]) for group in arms.values()]
        summary[key]['spread'] = max((s for s in spreads if s is not None), default=None)
    for key in ('queue_p50_ms', 'queue_p95_ms', 'delivered_kbps', 'recovery_max_ms'):
        summary[key] = {arm: mean([l['host'][key] for l in group]) for arm, group in arms.items()}
    if not reasons:
        for key in ('latency_p50_ms', 'latency_p95_ms'):
            s = summary[key]
            if None in (s['baseline'], s['on'], s['spread']):
                reasons.append(f'{key} missing')
            elif not s['baseline'] - s['on'] > s['spread']:
                reasons.append(f"{key} {s['on']:.2f} vs {s['baseline']:.2f}: not lower by more than the "
                               f"spread {s['spread']:.2f}")
        f = summary['fps']
        if None in (f['baseline'], f['on']):
            reasons.append('fps missing')
        elif f['on'] < f['baseline'] - FPS_TOLERANCE:
            reasons.append(f"fps {f['on']:.2f} vs {f['baseline']:.2f}: lower")
        d = summary['dropped']
        if None in (d['baseline'], d['on']):
            reasons.append('dropped missing')
        elif d['on'] > d['baseline']:
            reasons.append(f"dropped {d['on']:.1f} vs {d['baseline']:.1f}: more")
    return dict(passed=not reasons, reasons=reasons, summary=summary, legs=[l['name'] for l in legs])


def decide(legs):
    devices = {}
    for l in legs:
        devices.setdefault(l['device'], []).append(l)
    results = {device: compare(group) for device, group in sorted(devices.items())}
    on = bool(results) and all(r['passed'] for r in results.values())
    return dict(turn_on=on, devices=results)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    parser.add_argument('legs', nargs='+', help='leg directories with host.log and client.json')
    parser.add_argument('--json', help='write the report here too')
    args = parser.parse_args(argv)
    report = decide([read_leg(d) for d in args.legs])
    text = json.dumps(report, indent=2)
    print(text)
    if args.json:
        pathlib.Path(args.json).write_text(text + '\n', encoding='utf-8')
    return 0


if __name__ == '__main__':
    sys.exit(main())
