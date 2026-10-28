"""Scene-cut metrics for bottleneck.py, from the receiver's frames.csv and the host log.

A cut is a frame larger than twice the nominal frame (the encoder bitrate over
the frame rate). Ordinary frames exclude each cut and the two frames after it,
the frames that answer a lost cut. pacing_events() reads the slow-pace guard's
latches, lifts and ignored evidence, frames held by a full socket and the
keyframes' sizes against their cap; route_kinds() the loss history the case's
profile kept.
"""
import datetime, json, pathlib, re, sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1] / 'tests'))
from recovery_gaps import stalls  # noqa: E402

CUT_BUDGETS = 2
OVERSIZE_LINE = 'pacing frames over 2 frame budgets like a Wi-Fi route'
FULL_LATCH_LINE = 'pacing video like a Wi-Fi route'


def nominal_frame_bytes(bitrate_kbps, fps):
    return bitrate_kbps * 125 / fps


def percentile(values, share):
    if not values:
        return None
    ordered = sorted(values)
    return round(ordered[min(len(ordered) - 1, int((len(ordered) - 1) * share))], 2)


def median(values):
    return percentile(values, .5)


def cut_metrics(csv_path, rows, nominal_bytes):
    """The stalls, picture age and frame sizes of one run's frames.csv rows."""
    found = stalls(csv_path)[3] if len(rows) >= 3 else []
    sizes = [(int(row['wire_frame']), int(row.get('bytes') or 0)) for row in rows]
    cuts = [frame for frame, size in sizes if size > CUT_BUDGETS * nominal_bytes]
    after_cut = {frame + step for frame in cuts for step in range(3)}
    first_cut = cuts[0] if cuts else None

    def age(row):
        try:
            value = float(row.get('picture_age_ms') or '')
        except ValueError:
            return None
        return value if value >= 0 else None

    every, ordinary, late = [], [], []
    for row in rows:
        value = age(row)
        if value is None:
            continue
        every.append(value)
        frame = int(row['wire_frame'])
        if frame not in after_cut:
            ordinary.append(value)
            if first_cut is not None and frame > first_cut + 2:
                late.append(value)
    metrics = dict(stalls=len(found), stalls_100ms=sum(1 for stall in found if stall[0] >= 100),
                   cuts_received=len(cuts),
                   cut_bytes_median=median([size for frame, size in sizes if frame in cuts]),
                   ordinary_bytes_median=median([size for frame, size in sizes if frame not in after_cut]),
                   nominal_frame_bytes=round(nominal_bytes))
    for name, values in (('picture_age', every), ('ordinary_picture_age', ordinary),
                         ('ordinary_picture_age_after_first_cut', late)):
        for share in (50, 95, 99):
            metrics[f'{name}_p{share}_ms'] = percentile(values, share / 100)
    return metrics


def latch_flags(host_log):
    """Whether the host armed oversize pacing or took the full Wi-Fi latch, and the mode it logged."""
    mode = re.search(r'oversize pacing mode=(\w+)', host_log)
    lines = host_log.splitlines()
    return dict(oversize_armed=OVERSIZE_LINE in host_log,
                full_latch=FULL_LATCH_LINE in host_log,
                oversize_mode=mode.group(1) if mode else None,
                recovery_frames=sum(1 for line in lines if 'video recovery frame' in line),
                # A bitrate change confounds a pacing comparison; record it.
                link_rate_changes=sum(1 for line in lines if re.search(r'link[ _]rate', line, re.I)
                                      and 'network pacing' not in line and 'stream timings' not in line))


# The latch warnings, by the kind of latch they announce (core session.rs report_keyframe_loss).
LATCH_KINDS = (
    ("the client reported video loss after the host's send loss", 'confirmed_send'),
    ('more video loss after the start-up keyframe', 'confirmed'),
    ("the host's socket refused video packets", 'send'),
    ("the stream's first keyframe was lost", 'startup'),
    ('video frames are not arriving', 'sustained'),
)
LIFT_LINE = "returning to the route's pacing rate"
IGNORED_LINE = 'video loss evidence not counted'
HELD_LINE = 'for room instead of dropping packets'
REMEMBERED_LINE = 'pacing video like Wi-Fi from the first frame'
FIELD = re.compile(r'(\w+)=("(?:[^"\\]|\\.)*"|\S+)')


def fields(line):
    """A tracing line's key=value fields, quotes removed."""
    return {key: value[1:-1] if value.startswith('"') else value for key, value in FIELD.findall(line)}


def timestamp(line):
    try:
        return datetime.datetime.fromisoformat(line.split(' ', 1)[0].replace('Z', '+00:00'))
    except ValueError:
        return None


def number(text):
    try:
        return float(text) if '.' in text else int(text)
    except (TypeError, ValueError):
        return None


def pacing_events(host_log):
    """Latches (kind, reason, seconds after the first paced frame), lifts, ignored
    evidence, held frames and the recovery keyframes against their cap."""
    lines = host_log.splitlines()
    start = next((timestamp(line) for line in lines if 'network pacing selected' in line), None)

    def at(line):
        when = timestamp(line)
        return round((when - start).total_seconds(), 2) if when and start else None

    latches, lifts, keyframes, ignored, encode = [], [], [], [], []
    held = remembered = 0
    for line in lines:
        if FULL_LATCH_LINE in line:
            kind = next((name for text, name in LATCH_KINDS if text in line), 'other')
            latches.append(dict(at_s=at(line), kind=kind, reason=fields(line).get('reason')))
        elif LIFT_LINE in line:
            lifts.append(at(line))
        elif IGNORED_LINE in line:
            ignored.append(fields(line).get('why', ''))
        elif HELD_LINE in line:
            held += 1
        elif REMEMBERED_LINE in line:
            remembered += 1
        elif 'video recovery frame' in line and 'recovery_kind=keyframe' in line:
            f = fields(line)
            keyframes.append({key: number(f.get(key)) if key not in ('reencoded', 'fec_limited', 'lost_on_send')
                              else f.get(key) == 'true'
                              for key in ('frame', 'payload_bytes', 'cap_bytes', 'cap_target_bytes',
                                          'reencoded', 'fec_limited', 'lost_on_send')})
        elif 'stream timings' in line:
            value = number(fields(line).get('encode_mean_ms'))
            if value is not None:
                encode.append(value)
    over = [k for k in keyframes if k['cap_target_bytes'] and k['payload_bytes']
            and k['payload_bytes'] > 1.1 * k['cap_target_bytes']]
    return dict(latches=latches, lifts=lifts, remembered_at_start=remembered,
                ignored_evidence=len(ignored),
                ignored_repeated_requests=sum(1 for why in ignored if 'reports its own losses' in why),
                held_frame_logs=held, recovery_keyframes=keyframes,
                keyframes_over_target=len(over),
                keyframes_fec_limited=sum(1 for k in keyframes if k['fec_limited']),
                keyframes_lost_on_send=sum(1 for k in keyframes if k['lost_on_send']),
                host_encode_mean_ms=round(sum(encode) / len(encode), 3) if encode else None)


def timing_values(host_log, names):
    """Each field's values over the 5 s stream timings lines, in order."""
    values = {name: [] for name in names}
    for line in host_log.splitlines():
        if 'stream timings' not in line:
            continue
        f = fields(line)
        for name in names:
            value = number(f.get(name))
            if value is not None:
                values[name].append(value)
    return values


def idr_probe_environment(spec):
    """The receiver's keyframe probe for `--idr-probe N:MS`: N requests per cut, MS apart."""
    if not spec:
        return {}
    count, _, interval = spec.partition(':')
    if not count.isdigit() or int(count) < 1 or (interval and not interval.isdigit()):
        raise ValueError(f'--idr-probe takes N:MS, got {spec!r}')
    env = {'BUTTERPOLLO_TEST_IDR_PROBE': count}
    if interval:
        if not 100 <= int(interval) <= 10000:
            raise ValueError(f'--idr-probe interval must be 100-10000 ms, got {interval}')
        env['BUTTERPOLLO_TEST_IDR_PROBE_INTERVAL_MS'] = interval
    return env


def host_environment(send_outage):
    """The test host's send outage for `--send-outage EVERY_MS:LENGTH_MS[:stall]` (core stream_policy)."""
    return {'BUTTERPOLLO_TEST_SEND_OUTAGE': send_outage} if send_outage else {}


def route_kinds(state_path):
    """The kinds of the keyframe loss records a profile's paired state holds."""
    try:
        document = json.loads(pathlib.Path(state_path).read_text())
    except (OSError, ValueError):
        return []
    routes = document.get('root', {}).get('keyframe_loss_routes') or []
    return [route.get('kind', '') for route in routes if isinstance(route, dict)]
