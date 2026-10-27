"""Encoder focus A/B scores and the decision of docs/encoder-focus-design.md section 9; no hardware access.

  python focus_ab_result.py WORK [WORK ...] [--out DIR]

Reads runs.jsonl and plan.json from each work folder (one per GPU and step), writes quality.csv,
timing.csv, levels.csv, summary.json and summary.md into DIR (the first WORK by default) and prints
the verdict per codec. Quality rows are ABBA quartets: A1 off, B1 on, A2 off, B2 on at the request
rescaled to A1/B1's actual bitrate; centre adds C1 and C2 the same way. A comparison counts only if
an on arm's actual bitrate is within 3% of the off arms' mean. Noise is the larger of |A1 - A2| and
|B1 - B2|. Every rule must hold on each GPU; GPUs that disagree in direction are never default-on.
"""
import argparse, json, math, pathlib, re
from collections import defaultdict
from amf_sweep_result import ARMS, BITRATE_TOLERANCE, read_runs, write_csv

FRAMES = 180
# The text fixtures hold, scroll for 30 frames, then hold again: these are the first settled ones.
SETTLED = range(60, 90)
CENTRE_ARMS = ('C1', 'C2')
PAIR = ('A1', 'B1')
METRICS = ('vmaf_mean', 'vmaf_p5', 'vmaf_settled', 'psnr_y_mean', 'ssim_mean')
TEXT_CLASSES = ('text', 'mixed')
# Section 9 limits, in VMAF points and milliseconds.
GAIN_LOW, GAIN_HIGH, LOW_KBPS, NOISE_FACTOR = .5, .2, 10000, 2
WORSE_MEAN, WORSE_P5, GAME_OPT_IN, MOVING_DROP = .2, .5, .5, 1.
CENTRE_GAIN, CENTRE_FULL_LOSS = .5, .5
TIMING_LIMITS = dict(convert_p99_ms=.05, encode_p99_ms=.1, present_to_sent_p99_ms=.2)
STALE_LIMIT = .01
# Worst first: a combined verdict is the lowest of its GPUs'.
VERDICTS = ('incomplete', 'drop', 'undecided', 'opt_in', 'default_on')


def mean(values):
    values = list(values)
    return sum(values) / len(values)


def vmaf_frames(path, frames=FRAMES):
    """(vmaf, psnr_y, float_ssim) per frame of a libvmaf JSON log, in frame order."""
    path = pathlib.Path(path)
    rows = sorted(json.loads(path.read_text())['frames'], key=lambda f: f['frameNum'])
    if [f['frameNum'] for f in rows] != list(range(frames)):
        raise ValueError(f'{path.name}: {len(rows)} scored frames, expected {frames}')
    output = []
    for frame in rows:
        metrics = frame['metrics']
        if not all(k in metrics for k in ('vmaf', 'psnr_y', 'float_ssim')):
            raise ValueError(f'{path.name}: frame {frame["frameNum"]} lacks vmaf, psnr_y or float_ssim')
        values = (metrics['vmaf'], metrics['psnr_y'], metrics['float_ssim'])
        if not all(math.isfinite(v) for v in values):
            raise ValueError(f'{path.name}: frame {frame["frameNum"]} has a non-finite score')
        output.append(values)
    return output


def combine(parts):
    """One region's per-frame scores from its rectangles' (area, frames): VMAF and SSIM by area, PSNR through
    the mean squared error, so a clean rectangle cannot hide a damaged one."""
    total = sum(area for area, _ in parts)
    output = []
    for frame in zip(*(frames for _, frames in parts)):
        weights = [area / total for area, _ in parts]
        vmaf = sum(w * f[0] for w, f in zip(weights, frame))
        error = sum(w * 10 ** (-f[1] / 10) for w, f in zip(weights, frame))
        ssim = sum(w * f[2] for w, f in zip(weights, frame))
        output.append((vmaf, -10 * math.log10(error), ssim))
    return output


def summary(frames):
    vmaf = [f[0] for f in frames]
    ordered = sorted(vmaf)
    return dict(vmaf_mean=mean(vmaf), vmaf_p5=ordered[(len(ordered) - 1) * 5 // 100],
                vmaf_settled=mean(vmaf[i] for i in SETTLED), psnr_y_mean=mean(f[1] for f in frames),
                ssim_mean=mean(f[2] for f in frames))


def log_name(region, index):
    return f'vmaf-{region}-{index}.json'


def score_regions(directory, regions, frames=FRAMES):
    """Each region's summary from the libvmaf logs focus_ab.py writes, one per rectangle."""
    directory = pathlib.Path(directory)
    return {region: summary(combine([(w * h, vmaf_frames(directory / log_name(region, i), frames))
                                     for i, (_, _, w, h) in enumerate(rects)]))
            for region, rects in regions.items()}


def even(x, y, w, h):
    return [int(x) & ~1, int(y) & ~1, int(w) & ~1, int(h) & ~1]


def centre_regions(width, height):
    """The middle half of each axis, and the four bands around it."""
    x, y = even(width // 4, height // 4, 0, 0)[:2]
    w, h = even(0, 0, width - 2 * x, height - 2 * y)[2:]
    return dict(centre=[[x, y, w, h]],
                periphery=[[0, 0, width, y], [0, y + h, width, height - y - h], [0, y, x, h], [x + w, y, width - x - w, h]])


def rescale(request, control, treated):
    """amf_sweep.py quartet: the on arm's next request, so its actual bitrate meets the off arm's."""
    return max(1000, min(200000, round(request * control['actual_bitrate_kbps'] / treated['actual_bitrate_kbps'])))


def deltas(control, treated, region_names):
    """Mean of the treated arms minus the mean of the controls, per region and metric."""
    return {region: {m: mean(r['scores'][region][m] for r in treated) - mean(r['scores'][region][m] for r in control)
                     for m in METRICS} for region in region_names}


def quality_comparisons(runs):
    groups = defaultdict(list)
    for row in runs:
        if row['kind'] == 'quality':
            groups[(row['gpu'], row['comparison'])].append(row)
    output = []
    for (gpu, comparison), rows in groups.items():
        first = rows[0]
        result = {k: first.get(k) for k in ('gpu', 'comparison', 'codec', 'fixture', 'fixture_class', 'target_kbps')}
        result.update(complete=False, matched=False)
        arms = tuple(r['arm'] for r in rows)
        if arms[:4] != ARMS or arms[4:] not in ((), CENTRE_ARMS) or any(r['status'] != 'ok' for r in rows):
            result['problem'] = f"arms {'/'.join(arms)}, statuses {'/'.join(r['status'] for r in rows)}"
            output.append(result)
            continue
        by_arm = {r['arm']: r for r in rows}
        a1, b1, a2, b2 = (by_arm[a] for a in ARMS)
        control_kbps = mean(r['actual_bitrate_kbps'] for r in (a1, a2))
        matched = lambda r: abs(r['actual_bitrate_kbps'] - control_kbps) <= control_kbps * BITRATE_TOLERANCE
        treated = [r for r in (b1, b2) if matched(r)]
        regions = sorted(a1['scores'])
        result.update(complete=True, matched=bool(treated), control_kbps=control_kbps,
                      treated_kbps=mean(r['actual_bitrate_kbps'] for r in treated or [b2]),
                      treated_arms='+'.join(r['arm'] for r in treated),
                      delta=deltas((a1, a2), treated or [b2], regions),
                      noise={region: {m: max(abs(a1['scores'][region][m] - a2['scores'][region][m]),
                                             abs(b1['scores'][region][m] - b2['scores'][region][m])) for m in METRICS}
                             for region in regions})
        if not treated:
            result['problem'] = f'no on arm within {BITRATE_TOLERANCE:.0%} of the off arms\' {control_kbps:.0f} kb/s'
        if 'C1' in by_arm:
            centre = [r for r in (by_arm['C1'], by_arm['C2']) if matched(r)]
            result['centre'] = dict(matched=bool(centre),
                                    delta=deltas((a1, a2), centre, ('full', 'centre', 'periphery')) if centre else None)
        output.append(result)
    return sorted(output, key=lambda r: (r['gpu'], r['codec'] or '', r['target_kbps'] or 0, r['fixture'] or ''))


FIELD = re.compile(r'(\w+)=("[^"]*"|\S+)')
ANSI = re.compile(r'\x1b\[[0-9;]*m')
OVERFLOW = 'a streamed video frame passed the byte cap its blocks were planned for'
FOCUS_COUNTERS = ('maps', 'uniform', 'stale', 'skipped_recovery')


def focus_totals(lines):
    """The stream's focus counters from the `encoder focus` lines. Each encoder counts from its start, so a drop in
    the frames counted means a recreated encoder, and the counters before it add to the ones after."""
    totals, base, last = None, dict.fromkeys(FOCUS_COUNTERS, 0), None
    for line in lines:
        counters = {k: int(line.get(k, 0)) for k in FOCUS_COUNTERS}
        if last and sum(counters.values()) < sum(last.values()):
            base = {k: base[k] + last[k] for k in FOCUS_COUNTERS}
        last = counters
        totals = {k: base[k] + counters[k] for k in FOCUS_COUNTERS}
    return totals


def host_stats(text):
    """A stream's host log: the mean of the 5 s windows' present_to_sent and capture_age_at_submit p99 (the first
    window is warm-up), slice delivery overflows, and the focus counters over the whole stream (`focus_totals`),
    with the effective mode of the focus state line."""
    lines = ANSI.sub('', text).splitlines()
    windows = [dict(FIELD.findall(line)) for line in lines if 'stream timings' in line][1:]
    p99 = lambda key: [float(w[key].split('/')[2]) for w in windows if key in w]
    present, capture = p99('present_to_sent_ms'), p99('capture_age_at_submit_ms')
    focus = [dict(FIELD.findall(line)) for line in lines if 'encoder focus' in line and ' maps=' in line]
    # The state line at start says effective=; the 5 s lines say mode= (design section 7). The host's
    # log quotes string fields: effective="static".
    effective = [m for line in lines if 'encoder focus' in line
                 for m in re.findall(r'\b(?:effective|mode)="?(\w+)', line)]
    return dict(windows=len(windows), present_to_sent_p99_ms=mean(present) if present else None,
                capture_age_at_submit_p99_ms=mean(capture) if capture else None,
                overflows=sum(OVERFLOW in line for line in lines),
                focus=focus_totals(focus),
                focus_qp_delta=focus[-1].get('qp_delta') if focus else None,
                focus_effective=effective[-1] if effective else None)


def focus_fraction(focus):
    """The share of encoded frames that went out without a fresh map."""
    frames = sum(focus[k] for k in ('maps', 'uniform', 'stale', 'skipped_recovery'))
    return focus['stale'] / frames if frames else None


def timing_comparisons(runs):
    groups = defaultdict(list)
    for row in runs:
        if row['kind'] == 'timing':
            groups[(row['gpu'], row['comparison'])].append(row)
    output = []
    for (gpu, comparison), rows in groups.items():
        result = dict(gpu=gpu, comparison=comparison, complete=False)
        arms = tuple(r['arm'] for r in rows)
        if arms not in (ARMS, PAIR) or any(r['status'] != 'ok' for r in rows):
            result['problem'] = f"arms {'/'.join(arms)}, statuses {'/'.join(r['status'] for r in rows)}"
            output.append(result)
            continue
        control, treated = rows[::2], rows[1::2]
        focus = {k: sum(r['focus'][k] for r in treated) for k in ('maps', 'uniform', 'stale', 'skipped_recovery')}
        result.update(complete=True, abba=arms == ARMS, quick=any(r.get('step') == 'quick' for r in rows), focus=focus, stale_fraction=focus_fraction(focus),
                      overflows_off=sum(r['overflows'] for r in control), overflows_on=sum(r['overflows'] for r in treated))
        for key in TIMING_LIMITS:
            result['delta_' + key] = mean(r[key] for r in treated) - mean(r[key] for r in control)
        output.append(result)
    return sorted(output, key=lambda r: (r['gpu'], r['comparison']))


def timing_state(rows):
    """(ok, reasons, caps) of one GPU's timing comparisons; ok is None while any is missing or incomplete. Caps are
    latency limits a quick pass's single pair passed: without a noise estimate they keep focus from default-on
    but do not drop it."""
    if not rows:
        return None, ['no timing comparison'], []
    missing = [r['comparison'] for r in rows if not r['complete']]
    if missing:
        return None, ['timing incomplete: ' + ', '.join(missing)], []
    reasons, caps = [], []
    for row in rows:
        # A single pair (slice delivery, intra refresh) has no noise estimate: it answers overflows and stale only.
        for key, limit in TIMING_LIMITS.items() if row['abba'] or row.get('quick') else ():
            if row['delta_' + key] > limit:
                (reasons if row['abba'] else caps).append(
                    f"{row['comparison']}: {key} +{row['delta_' + key]:.3f} ms, limit +{limit} ms"
                    + ('' if row['abba'] else ' in one quick pair: confirm with the M3 ABBA'))
        if row['overflows_on'] > row['overflows_off']:
            reasons.append(f"{row['comparison']}: slice delivery overflows {row['overflows_off']} -> {row['overflows_on']}")
        if row['stale_fraction'] is None:
            reasons.append(f"{row['comparison']}: the on arms encoded no frame with focus")
        elif row['stale_fraction'] >= STALE_LIMIT:
            reasons.append(f"{row['comparison']}: stale {row['stale_fraction']:.2%} of frames, limit {STALE_LIMIT:.0%}")
    return not reasons, reasons, caps


def full(row, metric='vmaf_mean'):
    return row['delta']['full'][metric]


def gains(row):
    """Section 9 quality gain: +0.5 VMAF at 10 Mbps or less, +0.2 above, and at least twice the noise."""
    need = GAIN_LOW if row['target_kbps'] <= LOW_KBPS else GAIN_HIGH
    return full(row) >= need and full(row) >= NOISE_FACTOR * row['noise']['full']['vmaf_mean']


def worse(row):
    return full(row) < -WORSE_MEAN or full(row, 'vmaf_p5') < -WORSE_P5


def evaluate(rows, timing):
    """The verdict of one GPU and codec from its quality comparisons and that GPU's timing state."""
    broken = [r['comparison'] for r in rows if not (r['complete'] and r['matched'])]
    if not rows or broken:
        return dict(verdict='incomplete', reasons=['quality incomplete or unmatched: ' + ', '.join(broken or ['none'])])
    timing_ok, timing_reasons, caps = timing
    if timing_ok is None:
        return dict(verdict='incomplete', reasons=timing_reasons)
    if not timing_ok:
        return dict(verdict='drop', reasons=['timing regression (drop unless it can be fixed): '] + timing_reasons)
    text = [r for r in rows if r['fixture_class'] in TEXT_CLASSES]
    games = [r for r in rows if r['fixture_class'] == 'game']
    no_gain = [r['comparison'] for r in text if not gains(r)]
    dropped = [r['comparison'] for r in rows if worse(r)]
    moving = [r['comparison'] for r in rows if 'moving' in r['delta'] and r['delta']['moving']['vmaf_mean'] < -MOVING_DROP]
    reasons = ([f'no gain: {", ".join(no_gain)}'] if no_gain else []) + ([f'worse: {", ".join(dropped)}'] if dropped else []) + \
              ([f'moving region drops over {MOVING_DROP}: {", ".join(moving)}'] if moving else []) + ([] if text else ['no text fixture'])
    if not reasons:
        return dict(verdict='opt_in', reasons=caps) if caps else dict(verdict='default_on', reasons=[])
    # Opt-in: text and mixed gain (at 10 Mbps or below when measured there), none of them worse, games lose at most 0.5.
    low = [r for r in text if r['target_kbps'] <= LOW_KBPS] or text
    if (text and all(gains(r) for r in low) and not any(worse(r) for r in text) and not moving
            and all(full(r) >= -GAME_OPT_IN for r in games)):
        return dict(verdict='opt_in', reasons=reasons)
    if not any(full(r) > 0 and full(r) > r['noise']['full']['vmaf_mean'] for r in rows):
        return dict(verdict='drop', reasons=['no gain beyond noise anywhere'] + reasons)
    return dict(verdict='undecided', reasons=reasons)


def centre_verdict(rows):
    rows = [r for r in rows if 'centre' in r]
    if not rows:
        return None
    if not all(r['centre']['matched'] for r in rows):
        return dict(verdict='incomplete', reasons=['centre arms unmatched: ' + ', '.join(r['comparison'] for r in rows if not r['centre']['matched'])])
    gain = mean(r['centre']['delta']['centre']['vmaf_mean'] for r in rows)
    loss = min(r['centre']['delta']['full']['vmaf_mean'] for r in rows)
    keep = gain >= CENTRE_GAIN and loss >= -CENTRE_FULL_LOSS
    return dict(verdict='keep_opt_in' if keep else 'remove', centre_gain=gain, worst_full=loss,
                reasons=[] if keep else [f'centre gain {gain:+.2f} (needs +{CENTRE_GAIN}), worst full frame {loss:+.2f} (limit -{CENTRE_FULL_LOSS})'])


def direction(row):
    """+1 or -1 when the full-frame change is beyond noise, else 0."""
    if not (row['complete'] and row['matched']):
        return 0
    noise = row['noise']['full']['vmaf_mean']
    return 1 if full(row) > noise else -1 if full(row) < -noise else 0


def placeholders(plans, quality, timing):
    """Planned comparisons without rows, as incomplete ones."""
    have = {(r['gpu'], r['comparison']) for r in quality + timing}
    for plan in plans:
        for item in plan.get('quality', []):
            if (plan['gpu'], item['comparison']) not in have:
                quality.append(dict(item, gpu=plan['gpu'], complete=False, matched=False, problem='not run'))
        for comparison in plan.get('timing', []):
            if (plan['gpu'], comparison) not in have:
                timing.append(dict(gpu=plan['gpu'], comparison=comparison, complete=False, problem='not run'))


def decide(runs, plans=()):
    quality, timing = quality_comparisons(runs), timing_comparisons(runs)
    placeholders(plans, quality, timing)
    gpus = sorted({r['gpu'] for r in quality + timing})
    states = {gpu: timing_state([t for t in timing if t['gpu'] == gpu]) for gpu in gpus}
    codecs = sorted({r['codec'] for r in quality})
    per_gpu, combined = defaultdict(dict), {}
    for codec in codecs:
        votes = {}
        for gpu in gpus:
            rows = [r for r in quality if r['gpu'] == gpu and r['codec'] == codec]
            if rows:
                votes[gpu] = per_gpu[gpu][codec] = evaluate(rows, states[gpu])
        # The same comparison on two GPUs must not move in opposite directions.
        signs = defaultdict(dict)
        for row in quality:
            if row['codec'] == codec:
                signs[row['comparison']][row['gpu']] = direction(row)
        conflicts = sorted(c for c, by_gpu in signs.items() if {1, -1} <= set(by_gpu.values()))
        verdict = min((v['verdict'] for v in votes.values()), key=VERDICTS.index)
        if conflicts and VERDICTS.index(verdict) > VERDICTS.index('undecided'):
            verdict = 'undecided'
        combined[codec] = dict(verdict=verdict, gpus={g: v['verdict'] for g, v in votes.items()},
                               disagree=len({v['verdict'] for v in votes.values()}) > 1 or bool(conflicts),
                               conflicts=conflicts)
    centre = {gpu: centre_verdict([r for r in quality if r['gpu'] == gpu]) for gpu in gpus}
    centre = {gpu: v for gpu, v in centre.items() if v}
    quick = any(p.get('step') == 'quick' for p in plans) or any(r.get('quick') for r in timing)
    return dict(codecs=combined, per_gpu=dict(per_gpu), timing={g: dict(ok=s[0], reasons=s[1] + s[2]) for g, s in states.items()},
                centre=centre, quality=quality, timing_rows=timing, quick=quick)


QUICK_NOTE = ('quick pass: one fixture set and one stream pair; default_on still needs every section 9 rule, '
              'and anything that only reaches opt_in ships as Experimental')


def verdict_label(verdict, quick):
    return f'{verdict} (Experimental)' if quick and verdict == 'opt_in' else verdict


def level_ranking(runs):
    """M1: each tuning's mean full-frame and settled VMAF change against the off run of the same fixture."""
    rows = [r for r in runs if r['kind'] == 'levels' and r['status'] == 'ok']
    off = {(r['gpu'], r['fixture']): r for r in rows if r['levels'] is None}
    groups = defaultdict(list)
    for row in rows:
        if row['levels'] is not None and (row['gpu'], row['fixture']) in off:
            groups[(row['gpu'], row['levels'])].append(row)
    output = []
    for (gpu, levels), group in groups.items():
        base = [off[(gpu, r['fixture'])] for r in group]
        output.append(dict(gpu=gpu, levels=levels, fixtures=len(group),
                           delta_vmaf_mean=mean(r['scores']['full']['vmaf_mean'] - b['scores']['full']['vmaf_mean'] for r, b in zip(group, base)),
                           delta_vmaf_settled=mean(r['scores']['full']['vmaf_settled'] - b['scores']['full']['vmaf_settled'] for r, b in zip(group, base)),
                           bitrate_error=max(abs(r['actual_bitrate_kbps'] / b['actual_bitrate_kbps'] - 1) for r, b in zip(group, base))))
    ranked = sorted(output, key=lambda r: (r['gpu'], -r['delta_vmaf_mean'], -r['delta_vmaf_settled']))
    for gpu in {r['gpu'] for r in ranked}:
        for index, row in enumerate((r for r in ranked if r['gpu'] == gpu), 1):
            row['rank'] = index
    return ranked


def summarize(directories, out=None):
    directories = [pathlib.Path(d) for d in directories]
    out = pathlib.Path(out or directories[0])
    runs, plans = [], []
    for directory in directories:
        runs += read_runs(directory)
        if (directory / 'plan.json').exists():
            plans.append(json.loads((directory / 'plan.json').read_text()))
    result = decide(runs, plans)
    levels = level_ranking(runs)
    write_csv(out / 'quality.csv', result['quality'])
    write_csv(out / 'timing.csv', result['timing_rows'])
    write_csv(out / 'levels.csv', levels)
    summary_json = {k: result[k] for k in ('codecs', 'per_gpu', 'timing', 'centre', 'quick')} | dict(levels=levels[:4], work=[str(d) for d in directories])
    (out / 'summary.json').write_text(json.dumps(summary_json, indent=2))
    fmt = lambda v: '-' if v is None else f'{v:+.2f}' if isinstance(v, float) else str(v)
    name = lambda v: verdict_label(v, result['quick'])
    lines = ['# Encoder focus A/B' + (' (quick pass)' if result['quick'] else ''), '',
             'Section 9 of docs/encoder-focus-design.md. Deltas are on minus off, VMAF points; noise is max(|A1 - A2|, |B1 - B2|).'
             + (f' {QUICK_NOTE}' if result['quick'] else ''), '', '## Verdict', '', '| Codec | Verdict | Per GPU | Direction conflicts |', '|---|---|---|---|']
    for codec, verdict in result['codecs'].items():
        lines.append(f"| {codec} | {name(verdict['verdict'])} | {', '.join(f'{g}: {name(v)}' for g, v in verdict['gpus'].items())} | "
                     f"{', '.join(verdict['conflicts']) or '-'} |")
    lines += ['']
    for gpu, codecs in result['per_gpu'].items():
        for codec, verdict in codecs.items():
            for reason in verdict['reasons']:
                lines.append(f'- {gpu} {codec}: {reason}')
    for gpu, state in result['timing'].items():
        lines.append(f"- {gpu} timing: {'ok' if state['ok'] else 'incomplete' if state['ok'] is None else 'fails'}"
                     + (': ' + '; '.join(state['reasons']) if state['reasons'] else ''))
    for gpu, centre in result['centre'].items():
        lines.append(f"- {gpu} centre: {centre['verdict']}" + (': ' + '; '.join(centre['reasons']) if centre['reasons'] else ''))
    lines += ['', '## Quality', '', '| GPU | Comparison | kb/s off / on | Full mean | Full p5 | Settled | Still | Moving | HUD | Noise |',
              '|---|---|---:|---:|---:|---:|---:|---:|---:|---:|']
    for row in result['quality']:
        delta = row.get('delta') or {}
        region = lambda name, metric='vmaf_mean': delta.get(name, {}).get(metric)
        lines.append(f"| {row['gpu']} | {row['comparison']} | "
                     f"{fmt(row.get('control_kbps') and round(row['control_kbps']))} / {fmt(row.get('treated_kbps') and round(row['treated_kbps']))} | "
                     f"{fmt(region('full'))} | {fmt(region('full', 'vmaf_p5'))} | {fmt(region('full', 'vmaf_settled'))} | "
                     f"{fmt(region('still'))} | {fmt(region('moving'))} | {fmt(region('hud'))} | "
                     f"{fmt(row.get('noise', {}).get('full', {}).get('vmaf_mean'))} |" + (f" {row['problem']}" if row.get('problem') else ''))
    lines += ['', '## Timing', '', '| GPU | Comparison | Convert p99 | Encode p99 | Present to sent p99 | Overflows off / on | Stale |',
              '|---|---|---:|---:|---:|---:|---:|']
    for row in result['timing_rows']:
        stale = row.get('stale_fraction')
        lines.append(f"| {row['gpu']} | {row['comparison']} | {fmt(row.get('delta_convert_p99_ms'))} | {fmt(row.get('delta_encode_p99_ms'))} | "
                     f"{fmt(row.get('delta_present_to_sent_p99_ms'))} | {row.get('overflows_off', '-')} / {row.get('overflows_on', '-')} | "
                     f"{'-' if stale is None else f'{stale:.2%}'} |" + (f" {row['problem']}" if row.get('problem') else ''))
    if levels:
        lines += ['', '## Levels (M1)', '', '| GPU | Rank | Levels | Full mean | Settled | Bitrate error |', '|---|---:|---|---:|---:|---:|']
        lines += [f"| {r['gpu']} | {r['rank']} | {r['levels']} | {fmt(r['delta_vmaf_mean'])} | {fmt(r['delta_vmaf_settled'])} | "
                  f"{r['bitrate_error']:.1%} |" for r in levels]
    (out / 'summary.md').write_text('\n'.join(lines) + '\n', encoding='utf-8')
    print(json.dumps(dict(**(dict(quick=QUICK_NOTE) if result['quick'] else {}),
                          verdict={c: name(v['verdict']) for c, v in result['codecs'].items()},
                          centre={g: v['verdict'] for g, v in result['centre'].items()}, summary=str(out / 'summary.md'))))
    return summary_json


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    parser.add_argument('work', nargs='+', type=pathlib.Path)
    parser.add_argument('--out', type=pathlib.Path)
    args = parser.parse_args()
    summarize(args.work, args.out)
