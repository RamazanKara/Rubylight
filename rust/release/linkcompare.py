"""The analysis half of the link-speed harness: no network, no third-party imports, unit-tested.

linkspeed.py measures; this module turns the per-arm result.json files into the series and verdicts
that link-adaptation work is judged by:

  * series helpers used by linkspeed.analyse (mean encoder bitrate, recovery time, freezes per phase);
  * `fit_encoder_kbps`: the largest encoder bitrate whose wire rate fits a shaped link (a port of
    core's WireBudget::encoder_kbps, close enough for a 60 % / 65 % bound);
  * `noise_stats` and `repeats_needed`: the run-to-run spread of picture age p50 and how many runs per
    side resolve a 0.3 ms difference;
  * `judge` and `compare`: PASS/FAIL per arm against the acceptance in the link-adaptation design (B8).

Run files: a work directory holds one folder per run, named after the arm (`hz-r1000-hevc-20`) or, for
a repeat, `ARM@tag` (`hz-r1000-hevc-20@2`). Picture age here is the host-to-display `e2e_ms` of the A8
timing join, the figure PERFORMANCE.md quotes.
"""
import json, math, pathlib, re, statistics

HEALTHY_RATES = (1000, 500, 300)
HEALTHY_MBPS = (20, 40, 80)

# Acceptance thresholds (design section 10, B8).
P50_BOUND_MS = 0.3
P99_BOUND_MS = 1.0
LOSS_TOLERANCE = 0.01          # percentage points; absorbs float rounding of the stored value
ABOVE_LOSS_PERCENT = 2.0       # after the first 3 s
ABOVE_FREEZE_MS = 1000.0
ABOVE_MEAN_FIT = 0.65
ABOVE_AFTER_S = 3.0
ATLINK_LOSS_PERCENT = 2.0
ATLINK_FREEZES = 3
ATLINK_FREEZE_TOTAL_MS = 1000.0
ATLINK_MEAN_FIT = 0.60
STEP_FREEZE_MS = 1500.0
STEP_LOSS_PERCENT = 3.0
STEP_RECOVERY_FRACTION = 0.90
STEP_RECOVERY_S = 20.0
LAST_SECONDS = 20.0

# From the A/A run of 3.0.0-beta.1 (PERFORMANCE.md, "Link adaptation harness"): three runs of each of
# the nine hz arms gave a pooled run-to-run sigma of 0.51 ms of picture age p50, so 23 runs per side
# resolve a 0.3 ms difference of one arm with 95 % confidence. The mean over the nine arms is far
# tighter, which is why format_rows also prints it.
DEFAULT_REPEATS = 23


def hz_arm_name(rate, mbps):
    return f'hz-r{rate}-hevc-{mbps}'


def hz_arm_names():
    return [hz_arm_name(rate, mbps) for rate in HEALTHY_RATES for mbps in HEALTHY_MBPS]


def base_name(name):
    """`hz-r1000-hevc-20@2` is the second run of `hz-r1000-hevc-20`."""
    return name.split('@', 1)[0]


# ---- series -------------------------------------------------------------------------------------

def mean_last(series, seconds=LAST_SECONDS):
    """Mean of the values in the last `seconds` of a [(t, value)] series; None without values."""
    pts = [(t, v) for t, v in series if v is not None]
    if not pts:
        return None
    end = pts[-1][0]
    window = [v for t, v in pts if t >= end - seconds]
    return sum(window) / len(window)


def recovery_time(series, at, target):
    """Seconds from `at` to the first sample of a [(t, value)] series at or above `target`; None if never."""
    for t, v in series:
        if t >= at and v is not None and v >= target:
            return round(t - at, 1)
    return None


def freeze_events(ends, threshold_ms):
    """`ends` is [(display_end_us, sent_s)] of the delivered frames. Returns [(start_s, end_s, ms)] for
    every gap longer than `threshold_ms`; start_s and end_s are the send times of the frames on each
    side, so the gap sits on the same clock as the step times."""
    ends = sorted(ends)
    events = []
    for (a, sa), (b, sb) in zip(ends, ends[1:]):
        ms = (b - a) / 1000
        if ms > threshold_ms:
            events.append((sa, sb, ms))
    return events


def phase_freezes(events, bounds, total):
    """Longest freeze in each phase between the step times `bounds` (phases [0,b0), [b0,b1), ... up to
    `total` seconds). A freeze spanning a step is split in proportion to its overlap with each phase,
    so the 20 s freeze of a link drop counts in the phase after the drop, not before it."""
    edges = [0.0, *bounds, float(total)]
    longest = []
    for lo, hi in zip(edges, edges[1:]):
        worst = 0.0
        for s, e, ms in events:
            overlap = min(e, hi) - max(s, lo)
            if e > s and overlap > 0:
                worst = max(worst, ms * overlap / (e - s))
        longest.append(round(worst, 1))
    return longest


def loss_after(series, from_s):
    """Percent of sent frames not delivered in the one-second bins at or after `from_s` (result['series'])."""
    sent = delivered = 0
    for key, b in series.items():
        if int(key) >= from_s:
            sent += b['sent']; delivered += b['delivered']
    return None if not sent else round(100 * (1 - delivered / sent), 2)


# ---- the link a stream fits ---------------------------------------------------------------------

def _parity(count, percent, minimum):
    return max(minimum if count else 0, math.ceil(count * percent / 100))


def video_wire_bps(kbps, fec=20, min_fec=2, packet_size=1392, fps=60, encrypted=False):
    """Wire rate of an encoder bitrate, after packets, parity and 86 bytes of headers (the host counts IPv6)."""
    period = 1 / fps
    payload = math.ceil(kbps * 125 * period)
    if payload == 0:
        return 0
    shards = math.ceil((payload + 8) / (packet_size + 16 - 32))
    blocks = max(1, math.ceil(shards / (255 * 100 // (100 + fec))))
    aligned = math.ceil(shards / blocks)
    packets = 0
    for block in range(blocks):
        data = min(max(shards - block * aligned, 0), aligned)
        if data:
            packets += data + _parity(data, fec, min_fec)
    datagram = packet_size + 16 + (32 if encrypted else 0)
    return math.ceil(packets * (datagram + 86) * 8 / period)


def fit_encoder_kbps(link_mbps, ceiling_kbps=None, fec=20, min_fec=2, packet_size=1392, fps=60, audio_channels=2):
    """Largest encoder kbps (up to `ceiling_kbps`) whose wire rate fits `link_mbps` the way
    WireBudget::encoder_kbps counts it: 90 % of what the link has left after audio."""
    audio = audio_channels * 96 * 1500 + 500_000
    video = max(0, int(link_mbps * 1e6) - audio) * 9 // 10
    lo, hi = 1, max(1, int(ceiling_kbps or link_mbps * 1000))
    while lo < hi:
        mid = lo + (hi - lo + 1) // 2
        if video_wire_bps(mid, fec, min_fec, packet_size, fps) <= video:
            lo = mid
        else:
            hi = mid - 1
    return lo


# ---- noise --------------------------------------------------------------------------------------

def pooled_sigma(groups):
    """Pooled standard deviation of repeated measurements: `groups` is a list of lists of values."""
    ss = dof = 0
    for values in groups:
        if len(values) > 1:
            mean = sum(values) / len(values)
            ss += sum((v - mean) ** 2 for v in values)
            dof += len(values) - 1
    return math.sqrt(ss / dof) if dof else None


def repeats_needed(sigma, bound=P50_BOUND_MS, z=1.96):
    """Runs per side so that the difference of the two sides' mean p50 stays within `bound` with the
    confidence of `z`: z * sigma * sqrt(2 / n) <= bound."""
    if not sigma:
        return 1
    return max(1, math.ceil(2 * (z * sigma / bound) ** 2))


def noise_stats(runs_by_arm):
    """Run-to-run spread of picture age p50 from repeated runs of the same arm. `runs_by_arm` is
    {arm: [result, ...]}. Returns per-arm p50 lists, the pooled and the worst-arm sigma, the largest
    |difference| between two runs of one arm, and the repeats each sigma implies."""
    arms = {}
    for arm, runs in sorted(runs_by_arm.items()):
        p50 = [p50_of(r) for r in runs if p50_of(r) is not None]
        if len(p50) >= 2:
            arms[arm] = dict(p50=p50, range=round(max(p50) - min(p50), 3), sigma=pooled_sigma([p50]))
    pooled = pooled_sigma([a['p50'] for a in arms.values()])
    worst = max((a['sigma'] for a in arms.values()), default=None)
    return dict(arms=arms, pooled_sigma=pooled, worst_sigma=worst,
                max_range=max((a['range'] for a in arms.values()), default=None),
                repeats_pooled=repeats_needed(pooled), repeats_worst=repeats_needed(worst))


# ---- run files ----------------------------------------------------------------------------------

def load_runs(*dirs):
    """{arm: [result.json, ...]} over the run folders of the given work directories."""
    runs = {}
    for d in dirs:
        for folder in sorted(pathlib.Path(d).iterdir()):
            f = folder / 'result.json'
            if folder.is_dir() and f.exists():
                runs.setdefault(base_name(folder.name), []).append(json.loads(f.read_text()))
    return runs


def p50_of(run):
    return (run.get('e2e_ms') or {}).get('p50')


def p99_of(run):
    return (run.get('e2e_ms') or {}).get('p99')


def _median(values):
    values = [v for v in values if v is not None]
    return statistics.median(values) if values else None


def changes_of(run):
    c = run.get('adaptive_changes') or {}
    return c.get('adaptive_bitrate', 0) + c.get('link_rate', 0)


# What "network pacing selected" decides; other fields on the line (client_link_mbps,
# configured_kbps, ...) come and go between versions and say nothing about the pacing.
PACING_KEYS = ('pacing_bps', 'wire_budget_bps', 'link_bps')


def pacing_fields(line):
    """The key=value fields of a "network pacing selected" log line."""
    return dict(re.findall(r'(\w+)=(\S+)', line))


def pacing_selected_equal(a, b):
    """The same pacing lines, judged on PACING_KEYS present on both sides; a field that only one
    side has (new in a later version) is ignored."""
    a, b = a or [], b or []
    if len(a) != len(b):
        return False
    for x, y in zip(a, b):
        fx, fy = pacing_fields(x), pacing_fields(y)
        common = [k for k in PACING_KEYS if k in fx and k in fy]
        if not common:
            if x != y:
                return False
        elif any(fx[k] != fy[k] for k in common):
            return False
    return True


def pacing_values(run):
    return sorted({v for _, v in run.get('pacing_kbps_series') or [] if v})


def pacing_series_equal(run, baseline):
    """The distinct pacing rates match. Only a baseline from before the harness recorded the series
    may lack it (the pacing line alone is compared then); a run without it against a baseline with
    it is a difference, or a candidate whose series was lost would pass."""
    if 'pacing_kbps_series' not in baseline:
        return True
    if 'pacing_kbps_series' not in run:
        return False
    return pacing_values(run) == pacing_values(baseline)


# ---- verdicts -----------------------------------------------------------------------------------

def group_of(name):
    name = base_name(name)
    if name in ('wifi', 'wifi-hi', 'step-300-30-300'):
        return 'nocut'
    if name in ('step-80-10-80', 'step-80-10-80-40m', 'step-300-30-300-40m'):
        return 'steps'
    if re.fullmatch(r'hz-r\d+-hevc-\d+', name) or name in ('r1000-hevc-def', 'r1000-av1-def', 'r1000-h264-def'):
        return 'healthy'
    if re.fullmatch(r'r(300|150|80|50|30|20)-hevc-hi', name):
        return 'above'
    if re.fullmatch(r'r20-(hevc|av1|h264)-def', name):
        return 'atlink'
    if name in ('r1000-hevc-4k', 'r300-hevc-4k', 'r150-hevc-4k'):
        return 'unchanged'
    return 'info'


def _check(checks, name, ok, detail):
    checks.append(dict(name=name, ok=bool(ok), detail=detail))


def _worst(values, high=True):
    values = [v for v in values if v is not None]
    return (max if high else min)(values) if values else None


def judge(name, a_runs, b_runs, repeats=DEFAULT_REPEATS):
    """Judge candidate runs `b_runs` of one arm against baseline runs `a_runs`. Latency and loss
    comparisons use the median over runs; absolute limits apply to the worst candidate run."""
    group = group_of(name)
    checks = []
    row = dict(arm=base_name(name), group=group, runs=(len(a_runs), len(b_runs)))
    bad = [r for r in a_runs + b_runs if 'error' in r or not r.get('e2e_ms')]
    if bad:
        _check(checks, 'data', False, 'run without data: ' + str(bad[0].get('error', 'no timing')))
    if len(a_runs) < repeats or len(b_runs) < repeats:
        _check(checks, 'repeats', False, f'{len(a_runs)} baseline and {len(b_runs)} candidate runs, {repeats} needed')
    if not a_runs or not b_runs:
        row.update(checks=checks, verdict='FAIL')
        return row
    pa, pb = _median([p50_of(r) for r in a_runs]), _median([p50_of(r) for r in b_runs])
    qa, qb = _median([p99_of(r) for r in a_runs]), _median([p99_of(r) for r in b_runs])
    la, lb = _median([r.get('lost_percent') for r in a_runs]), _median([r.get('lost_percent') for r in b_runs])
    row.update(p50=(pa, pb), p99=(qa, qb), lost=(la, lb),
               d_p50=None if pa is None or pb is None else round(pb - pa, 3),
               d_p99=None if qa is None or qb is None else round(qb - qa, 3),
               freezes=(_median([(r.get('freezes') or {}).get('count') for r in a_runs]),
                        _median([(r.get('freezes') or {}).get('count') for r in b_runs])))
    seen_a = [r.get('encoder_kbps_seen') for r in a_runs]
    seen_b = [r.get('encoder_kbps_seen') for r in b_runs]
    row['bitrate_equal'] = all(len(s or []) == 1 for s in seen_a + seen_b) and all(s == seen_a[0] for s in seen_a + seen_b) \
        and all(changes_of(r) == 0 for r in b_runs)
    row['pacing_equal'] = all(pacing_selected_equal(r.get('pacing_selected'), a_runs[0].get('pacing_selected')) for r in a_runs + b_runs) \
        and all(pacing_series_equal(r, a_runs[0]) for r in a_runs + b_runs)
    row['changes'] = max(changes_of(r) for r in b_runs)

    def loss_no_worse():
        _check(checks, 'loss', lb is not None and la is not None and lb <= la + LOSS_TOLERANCE, f'{lb} against {la} %')

    if group in ('healthy', 'unchanged'):
        _check(checks, 'p50', row['d_p50'] is not None and (row['d_p50'] <= P50_BOUND_MS if group == 'unchanged'
                                                              else abs(row['d_p50']) <= P50_BOUND_MS),
               f'delta {row["d_p50"]} ms, bound {P50_BOUND_MS}')
        _check(checks, 'p99', row['d_p99'] is not None and row['d_p99'] <= P99_BOUND_MS, f'delta {row["d_p99"]} ms, bound +{P99_BOUND_MS}')
        loss_no_worse()
    if group == 'healthy':
        _check(checks, 'bitrate', row['bitrate_equal'], f'baseline {seen_a[0]}, candidate {[s for s in seen_b]}, '
                                                          f'change lines {row["changes"]}')
        _check(checks, 'pacing', row['pacing_equal'], 'network pacing selected and the pacing series must match the baseline')
    if group == 'nocut':
        _check(checks, 'no change', row['changes'] == 0, f'{row["changes"]} bitrate change lines')
        loss_no_worse()
    if group in ('above', 'atlink'):
        ref = b_runs[0]
        fit = fit_encoder_kbps(ref['rate'], ref.get('encoder_kbps_negotiated'))
        mean = _worst([r.get('encoder_kbps_last20_mean') for r in b_runs], high=False)
        row['fit_kbps'], row['mean_kbps'] = fit, None if mean is None else round(mean)
        need = ABOVE_MEAN_FIT if group == 'above' else ATLINK_MEAN_FIT
        _check(checks, 'bitrate', mean is not None and mean >= need * fit, f'last {LAST_SECONDS:.0f} s mean {row["mean_kbps"]} kbps, '
                                                                          f'fit {fit}, need {need:.0%}')
    if group == 'above':
        late = _worst([loss_after(r.get('series') or {}, ABOVE_AFTER_S) for r in b_runs])
        row['lost_late'] = late
        _check(checks, 'loss after 3 s', late is not None and late < ABOVE_LOSS_PERCENT, f'{late} %, limit {ABOVE_LOSS_PERCENT}')
        longest = max((ms for r in b_runs for s, _, ms in r.get('freeze_events') or [] if s >= ABOVE_AFTER_S), default=0)
        _check(checks, 'freeze after 3 s', longest <= ABOVE_FREEZE_MS, f'longest {longest:.0f} ms, limit {ABOVE_FREEZE_MS:.0f}')
    if group == 'atlink':
        _check(checks, 'loss', lb is not None and lb <= ATLINK_LOSS_PERCENT, f'{lb} %, limit {ATLINK_LOSS_PERCENT}')
        count = _worst([(r.get('freezes') or {}).get('count') for r in b_runs])
        total = _worst([(r.get('freezes') or {}).get('total_ms') for r in b_runs])
        _check(checks, 'freezes', count is not None and count <= ATLINK_FREEZES and total <= ATLINK_FREEZE_TOTAL_MS,
               f'{count} freezes, {total} ms in total; limit {ATLINK_FREEZES} and {ATLINK_FREEZE_TOTAL_MS:.0f} ms')
    if group == 'steps':
        longest = _worst([_worst(r.get('phase_freeze_ms') or [None]) for r in b_runs])
        _check(checks, 'freeze per step', longest is not None and longest <= STEP_FREEZE_MS, f'longest {longest} ms, limit {STEP_FREEZE_MS:.0f}')
        _check(checks, 'loss', lb is not None and _worst([r.get('lost_percent') for r in b_runs]) <= STEP_LOSS_PERCENT,
               f'{_worst([r.get("lost_percent") for r in b_runs])} %, limit {STEP_LOSS_PERCENT}')
        back = [next((s.get('encoder_recovered_90_s') for s in reversed(r.get('step_settle') or []) if 'encoder_recovered_90_s' in s), None)
                for r in b_runs]
        row['recovery_s'] = _worst([b for b in back if b is not None]) if all(b is not None for b in back) else None
        _check(checks, 'rate back', row['recovery_s'] is not None and row['recovery_s'] <= STEP_RECOVERY_S,
               f'{STEP_RECOVERY_FRACTION:.0%} of the negotiated rate after {row["recovery_s"]} s, limit {STEP_RECOVERY_S:.0f}')
    row['checks'] = checks
    row['verdict'] = 'INFO' if group == 'info' else ('PASS' if all(c['ok'] for c in checks) else 'FAIL')
    return row


def compare(a_dirs, b_dirs, repeats=DEFAULT_REPEATS, only=None):
    a, b = load_runs(*a_dirs), load_runs(*b_dirs)
    names = sorted(set(a) & set(b) if not only else set(only))
    return [judge(n, a.get(n, []), b.get(n, []), repeats) for n in names]


def pooled_healthy_delta(rows):
    """(mean delta p50 over the healthy arms, arms) for the summary line; None without healthy rows."""
    deltas = [r['d_p50'] for r in rows if r['group'] == 'healthy' and r.get('d_p50') is not None]
    return (round(sum(deltas) / len(deltas), 3), len(deltas)) if deltas else None


def format_rows(rows):
    def num(v, digits=2):
        return '-' if v is None else f'{v:.{digits}f}'

    lines = [f'{"arm":<24}{"group":<10}{"runs":<6}{"dp50":>8}{"dp99":>8}{"loss A/B %":>14}{"frz A/B":>9}  bitrate  pacing  verdict']
    for r in rows:
        lines.append(f'{r["arm"]:<24}{r["group"]:<10}{"%d/%d" % r["runs"]:<6}{num(r.get("d_p50"), 3):>8}{num(r.get("d_p99"), 3):>8}'
                     f'{num((r.get("lost") or (None, None))[0]) + "/" + num((r.get("lost") or (None, None))[1]):>14}'
                     f'{"%s/%s" % tuple(num(v, 0) for v in (r.get("freezes") or (None, None))):>9}  '
                     f'{"same" if r.get("bitrate_equal") else "differs":<7}  {"same" if r.get("pacing_equal") else "differs":<6}  {r["verdict"]}')
        for c in r['checks']:
            if not c['ok']:
                lines.append(f'    FAIL {c["name"]}: {c["detail"]}')
    counts = {v: sum(1 for r in rows if r['verdict'] == v) for v in ('PASS', 'FAIL', 'INFO')}
    pooled = pooled_healthy_delta(rows)
    if pooled:
        lines.append(f'mean delta p50 over {pooled[1]} healthy arms: {pooled[0]:+.3f} ms')
    lines.append(f'{counts["PASS"]} pass, {counts["FAIL"]} fail, {counts["INFO"]} info')
    return '\n'.join(lines)
