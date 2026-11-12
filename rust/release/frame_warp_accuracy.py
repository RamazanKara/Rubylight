"""Frame Warp's accuracy against look_probe, a fake mouse-look game whose response is known exactly.

    python frame_warp_accuracy.py --probe OUT.json --client fw.csv [--json result.json] [--skip-seconds 0]
        [--stream-size WIDTHxHEIGHT]

OUT.json is look_probe's report (rust/windows/examples/look_probe.rs) and OUT.frames.csv, beside it, its frames.
fw.csv is nightjar-cli's NIGHTJAR_CLI_FRAME_WARP log of the same run, made on the same PC (docs/frame-warp.md,
"Measuring"). Only the standard library is used.

How each number is made:
- Client rows join probe frames by barcode_seq == frame (the barcode's sequence is the frames.csv frame). A row
  without a barcode (0) or with a sequence the probe never drew is not joined.
- A joined row's picture shows the probe's mouse_sum at that frame (content_sum). The ideal shift for it, in picture
  pixels per axis, is g_true * (sent + offset - content_sum), and its error is shift - ideal. g_true is
  expected_host_model.mouse_gain (the content moves against the camera, so -gain) times the stream's size over the
  display's on that axis: 1 unless --stream-size gives the stream's size (the probe's display size is in its report).
- The client's sums count from its first send, the probe's from its start, so offset maps one onto the other
  (probe = sent + offset). It is the median, per axis, of content_sum - sent over idle rows: rows whose sent sums had
  not changed for at least max(300 ms, the probe's lag + 250 ms) by decode_qpc_us, so everything sent had reached the
  picture. The first row showing the current sums stands for when they changed (they changed no later). Their
  spread is reported; a wide one means counts were lost or another source moved the probe's camera. Without an idle
  row the offset is taken as 0 when the probe ran with --injected-only (only injected moves count; nothing but the
  host is assumed to inject), and the run is refused otherwise.
- Rows from --skip-seconds after the first client row on are counted. Among them, locked rows (a model with its mouse
  fit locked) are measured unless the probe's camera_y is at its pitch limit (the probe drops the turn there, so its
  response is not linear) or |ideal| <= 0.5 px. Lengths are Euclidean over x and y. Over the measured rows:
  |ideal| median and p95 (the displacement shown without Frame Warp), |error| median and p95 (with it), the median
  signed error per axis (a bias), and removed = 1 - sum |error| / sum |ideal| (the share of the input-to-picture
  displacement Frame Warp removed).
- seconds_to_first_lock runs from the first client row to the first locked one, over every row.
- The learned gain is the median logged mouse gain per axis over the counted locked rows, with its error relative to
  g_true; the learned lag is their median lag_ms. The probe's lag is a floor for it: the host's lag adds the time
  from drawing a frame to capturing it.
- picture_age is decode_qpc_us less the probe's render_qpc_us over the counted joined rows: both read the same
  performance counter.
- A probe run with --fov draws through a perspective camera, and its report carries the camera's focal length
  (expected_host_model.focal, display pixels; times the stream's width over the display's here, and a stream of
  another shape is refused). A turn then moves no two points of the picture alike, so the run is measured over the
  picture: at the points POINTS a side apart from 5% to 95% of each axis. For a measured row the picture shows the
  camera of its probe frame (camera_x, camera_y) and should show that camera turned by the counts still outstanding,
  -g_true * (sent + offset - content_sum), its y held at the probe's limit. Where a point of the picture the probe
  would draw then is in the picture it drew (exact_source: the probe's own projection, undone) is that point's
  ideal source; the client samples it at client_source, nightjar_core::frame_warp::source from the row's shift and
  focal (the logged model's focal length; 0 or no such column is a client that slides the picture, point - shift).
  A point's displacement without Frame Warp is the distance from itself to its ideal source and its error with it
  the distance from the client's source to the ideal one. The ideal, the error, the bias and removed above are then
  those of the picture's centre, and `lens` adds, for the centre, the whole picture (a row's mean over the points)
  and its edge (the mean over the outermost points), the same median, p95 and removed, with the learned field of
  view (from the median logged focal length over the counted locked rows) beside the probe's.
- Percentiles are soak_result.percentile's (nearest rank, rounding down).
"""
import argparse, csv, json, math, pathlib, sys
from soak_result import percentile

CLIENT_COLUMNS = ('wire_frame', 'barcode_seq', 'decode_qpc_us', 'sent_x', 'sent_y', 'active', 'shift_x', 'shift_y',
                  'locked', 'mouse_gain_x', 'mouse_gain_y', 'lag_ms')
PROBE_COLUMNS = ('frame', 'mouse_sum_x', 'mouse_sum_y', 'camera_x', 'camera_y', 'render_qpc_us')
# The points a side at which a perspective run is measured, and the least depth the client's mapping divides by
# (nightjar_core::frame_warp::MIN_DEPTH).
POINTS = 7
MIN_DEPTH = 0.25
IDLE_US = 300_000
IDLE_LAG_MARGIN_US = 250_000
MIN_IDEAL_PX = 0.5
# A camera_y this close to the limit is at it.
PITCH_LIMIT_MARGIN_PX = 0.5


def read_csv(path, columns, what):
    with pathlib.Path(path).open(newline='') as stream:
        reader = csv.DictReader(stream)
        missing = [column for column in columns if column not in (reader.fieldnames or [])]
        if missing:
            raise ValueError(f'{path} is not {what}: it has no {", ".join(missing)}')
        return list(reader)


def number(text):
    return float(text) if text not in (None, '') else None


def read_client(path):
    rows = [dict(wire_frame=int(row['wire_frame']), seq=int(row['barcode_seq']), decode_us=int(row['decode_qpc_us']),
                 sent=(int(row['sent_x']), int(row['sent_y'])), active=row['active'] == '1',
                 shift=(float(row['shift_x']), float(row['shift_y'])), locked=row['locked'] == '1',
                 gain=(number(row['mouse_gain_x']), number(row['mouse_gain_y'])), lag_ms=number(row['lag_ms']),
                 focal=number(row.get('focal')))
            for row in read_csv(path, CLIENT_COLUMNS, 'a NIGHTJAR_CLI_FRAME_WARP log')]
    if not rows:
        raise ValueError(f'{path} has no rows')
    return rows


def frames_path(probe, report):
    """OUT.frames.csv beside OUT.json, else the path the report names."""
    probe = pathlib.Path(probe)
    beside = probe.with_suffix('.frames.csv')
    if beside.is_file():
        return beside
    named = (report.get('files') or {}).get('frames')
    if named and pathlib.Path(named).is_file():
        return pathlib.Path(named)
    raise ValueError(f'{beside.name} is missing beside {probe}')


def read_probe(path):
    report = json.loads(pathlib.Path(path).read_text())
    if report.get('probe') != 'look_probe':
        raise ValueError(f'{path} is not a look_probe report')
    gain = (report.get('expected_host_model') or {}).get('mouse_gain')
    if not isinstance(gain, list) or len(gain) != 2:
        raise ValueError(f'{path} has no expected_host_model.mouse_gain')
    frames = {}
    for row in read_csv(frames_path(path, report), PROBE_COLUMNS, 'a look_probe frames.csv'):
        frames[int(row['frame'])] = dict(sum=(int(row['mouse_sum_x']), int(row['mouse_sum_y'])),
                                         camera=(float(row['camera_x']), float(row['camera_y'])),
                                         camera_y=float(row['camera_y']), render_us=int(row['render_qpc_us']))
    if not frames:
        raise ValueError(f'{path}: the probe drew no frames')
    return report, frames


def stream_size(text):
    try:
        width, height = (int(part) for part in text.lower().split('x'))
    except ValueError:
        raise argparse.ArgumentTypeError(f'{text}: expected WIDTHxHEIGHT') from None
    if width <= 0 or height <= 0:
        raise argparse.ArgumentTypeError(f'{text}: expected WIDTHxHEIGHT')
    return width, height


def scale_for(report, size):
    """The stream's size over the probe display's, per axis."""
    if size is None:
        return 1.0, 1.0
    display = report.get('display') or {}
    if not display.get('width') or not display.get('height'):
        raise ValueError('the probe report has no display size to scale the gain by')
    return size[0] / display['width'], size[1] / display['height']


def idle_rows(client, idle_us):
    """The rows whose sent sums had not changed for idle_us, in the client's order."""
    idle, previous, changed_us = [], None, None
    for row in client:
        if row['sent'] != previous:
            previous, changed_us = row['sent'], row['decode_us']
        if row['decode_us'] - changed_us >= idle_us:
            idle.append(row)
    return idle


def estimate_offset(client, frames, report, idle_us):
    """probe = sent + offset, per axis (see the module documentation)."""
    differences = [tuple(frames[row['seq']]['sum'][axis] - row['sent'][axis] for axis in (0, 1))
                   for row in idle_rows(client, idle_us) if row['seq'] in frames]
    if differences:
        axes = [[difference[axis] for difference in differences] for axis in (0, 1)]
        return dict(counts=[percentile(axes[0], 50), percentile(axes[1], 50)], source='idle', rows=len(differences),
                    min=[min(axes[0]), min(axes[1])], max=[max(axes[0]), max(axes[1])], idle_ms=idle_us / 1000)
    if (report.get('config') or {}).get('injected_only'):
        return dict(counts=[0, 0], source='injected_only', rows=0, min=None, max=None, idle_ms=idle_us / 1000)
    raise ValueError(f'no client row was idle for {idle_us / 1000:.0f} ms, so the client\'s counts cannot be mapped '
                     'onto the probe\'s; run the probe with --injected-only or let the client pause')


def spread(values):
    return dict(median=percentile(values, 50), p95=percentile(values, 95)) if values else dict(median=None, p95=None)


def exact_source(point, shown, wanted, focal):
    """Where the picture look_probe drew with its camera at `shown` holds what it draws at `point` with the camera at
    `wanted`: the probe's projection onto its cylinder for one camera, undone for the other. Points are pixels from
    the picture's centre, cameras the probe's (pixels of turn at the centre), all in the unit of `focal`. None where
    the first picture's camera does not see it."""
    yaw, pitch = wanted[0] / focal, wanted[1] / focal
    y = point[1] * math.cos(pitch) + focal * math.sin(pitch)
    z = focal * math.cos(pitch) - point[1] * math.sin(pitch)
    around, height = yaw + math.atan2(point[0], z) - shown[0] / focal, y / math.hypot(point[0], z)
    pitch = shown[1] / focal
    x, z = math.sin(around), math.cos(around)
    y, z = height * math.cos(pitch) - z * math.sin(pitch), z * math.cos(pitch) + height * math.sin(pitch)
    return (focal * x / z, focal * y / z) if z > 1e-9 else None


def client_source(point, shift, focal, size):
    """Where the client samples the picture for `point` (pixels from its centre) under the centre's `shift`:
    nightjar_core::frame_warp::source, with lens_terms undone from the shift; point - shift without a focal length."""
    if not focal or focal <= 0:
        return point[0] - shift[0], point[1] - shift[1]
    t = (shift[0] / focal, shift[1] / focal)
    k = (t[0] * size[0] / focal, t[1] * size[1] / focal)
    n = (math.sqrt(1 + t[1] * t[1]), math.sqrt(1 + t[0] * t[0]))
    q = (point[0] / size[0], point[1] / size[1])
    depth = max(1 + q[0] * k[0] + q[1] * k[1], MIN_DEPTH)
    return tuple((q[axis] * n[axis] - shift[axis] / size[axis]) / depth * size[axis] for axis in (0, 1))


def lens_points(size):
    """The points a perspective run is measured at, pixels from the picture's centre, and which are its edge."""
    places = [0.05 + 0.9 * index / (POINTS - 1) - 0.5 for index in range(POINTS)]
    points = [(x * size[0], y * size[1]) for y in places for x in places]
    edge = [row in (0, POINTS - 1) or column in (0, POINTS - 1) for row in range(POINTS) for column in range(POINTS)]
    return points, edge


def lens_for(report, scale, size):
    """The probe camera's focal length in the stream's pixels and the stream's size, or None for a flat probe."""
    focal = (report.get('expected_host_model') or {}).get('focal')
    if not focal:
        return None
    display = report.get('display') or {}
    if not display.get('width') or not display.get('height'):
        raise ValueError('the probe report has no display size to measure its lens over')
    if abs(scale[0] - scale[1]) > 0.01 * scale[0]:
        raise ValueError('the stream is not the shape of the probe\'s display, so its picture has no one lens')
    return dict(focal=focal * scale[0], size=size or (display['width'], display['height']))


def region():
    return dict(ideal=[], error=[])


def region_result(values):
    return dict(without_warp_px=spread(values['ideal']), with_warp_error_px=spread(values['error']),
                removed=1 - sum(values['error']) / sum(values['ideal']) if sum(values['ideal']) else None)


def analyze(report, frames, client, skip_seconds=0.0, size=None):
    expected = report['expected_host_model']
    scale = scale_for(report, size)
    g_true = (expected['mouse_gain'][0] * scale[0], expected['mouse_gain'][1] * scale[1])
    lens = lens_for(report, scale, size)
    points, edge = lens_points(lens['size']) if lens else ([], [])
    regions = dict(centre=region(), picture=region(), edge=region())
    unseen = 0
    lag_us = int(expected.get('lag_us_at_least') or 0)
    offset = estimate_offset(client, frames, report, max(IDLE_US, lag_us + IDLE_LAG_MARGIN_US))
    first_us = client[0]['decode_us']
    first_lock = next((row for row in client if row['locked']), None)
    counted = [(row, frames[row['seq']]) for row in client
               if row['seq'] in frames and row['decode_us'] >= first_us + skip_seconds * 1e6]
    locked = [(row, frame) for row, frame in counted if row['locked']]
    y_limit = report.get('camera_y_limit')
    pitch_limited = small = 0
    ideal_px, error_px, bias = [], [], ([], [])
    for row, frame in locked:
        if y_limit is not None and abs(frame['camera_y']) >= y_limit - PITCH_LIMIT_MARGIN_PX:
            pitch_limited += 1
            continue
        ideal = [g_true[axis] * (row['sent'][axis] + offset['counts'][axis] - frame['sum'][axis]) for axis in (0, 1)]
        exact = None
        if lens:
            # The picture's own camera, and that camera turned by what the picture has not seen yet.
            shown = (frame['camera'][0] * scale[0], frame['camera'][1] * scale[1])
            wanted = [shown[0] - ideal[0], shown[1] - ideal[1]]
            if y_limit is not None:
                wanted[1] = min(max(wanted[1], -y_limit * scale[1]), y_limit * scale[1])
            exact = [exact_source(point, shown, wanted, lens['focal']) for point in (*points, (0.0, 0.0))]
            if any(source is None for source in exact):
                unseen += 1
                continue
            ideal = [-exact[-1][0], -exact[-1][1]]
        length = math.hypot(*ideal)
        if length <= MIN_IDEAL_PX:
            small += 1
            continue
        error = [row['shift'][axis] - ideal[axis] for axis in (0, 1)]
        ideal_px.append(length)
        error_px.append(math.hypot(*error))
        for axis in (0, 1):
            bias[axis].append(error[axis])
        if lens:
            without, within = [], []
            for point, source in zip(points, exact):
                drawn = client_source(point, row['shift'], row['focal'], lens['size'])
                without.append(math.hypot(point[0] - source[0], point[1] - source[1]))
                within.append(math.hypot(drawn[0] - source[0], drawn[1] - source[1]))
            mean = lambda values, only: (sum(v for v, keep in zip(values, only) if keep) / sum(only))
            for name, only in (('picture', [True] * len(points)), ('edge', edge)):
                regions[name]['ideal'].append(mean(without, only))
                regions[name]['error'].append(mean(within, only))
            regions['centre']['ideal'].append(length)
            regions['centre']['error'].append(math.hypot(*error))
    gains = [[row['gain'][axis] for row, _ in locked if row['gain'][axis] is not None] for axis in (0, 1)]
    learned_gain = [percentile(gains[axis], 50) for axis in (0, 1)]
    lags = [row['lag_ms'] for row, _ in locked if row['lag_ms'] is not None]
    lens_result = None
    if lens:
        degrees = lambda focal: math.degrees(2 * math.atan(lens['size'][0] / 2 / focal)) if focal and focal > 0 else None
        learned_focal = percentile([row['focal'] or 0.0 for row, _ in locked], 50)
        lens_result = dict(focal=lens['focal'], field_of_view=degrees(lens['focal']), learned_focal=learned_focal,
                           learned_field_of_view=degrees(learned_focal),
                           with_lens_share=(sum(bool(row['focal']) for row, _ in locked) / len(locked) if locked else None),
                           unseen=unseen, **{name: region_result(values) for name, values in regions.items()})
    return dict(
        expected=dict(mouse_gain=list(g_true), scale=list(scale), lag_ms_at_least=lag_us / 1000),
        offset=offset,
        lens=lens_result,
        rows=dict(client=len(client), with_barcode=sum(row['seq'] != 0 for row in client),
                  joined=sum(row['seq'] in frames for row in client), counted=len(counted), locked=len(locked),
                  pitch_limited=pitch_limited, below_threshold=small, measured=len(ideal_px)),
        locked_share=len(locked) / len(counted) if counted else None,
        seconds_to_first_lock=(first_lock['decode_us'] - first_us) / 1e6 if first_lock else None,
        learned=dict(mouse_gain=learned_gain,
                     relative_error=[(learned_gain[axis] - g_true[axis]) / g_true[axis]
                                     if learned_gain[axis] is not None and g_true[axis] else None for axis in (0, 1)],
                     lag_ms=percentile(lags, 50)),
        without_warp_px=spread(ideal_px),
        with_warp_error_px=spread(error_px),
        error_bias_px=[percentile(bias[axis], 50) for axis in (0, 1)],
        removed=1 - sum(error_px) / sum(ideal_px) if ideal_px else None,
        picture_age_ms=spread([(row['decode_us'] - frame['render_us']) / 1000 for row, frame in counted]),
    )


def build_report(probe, client_path, skip_seconds=0.0, size=None):
    report, frames = read_probe(probe)
    result = analyze(report, frames, read_client(client_path), skip_seconds, size)
    return dict(probe=str(probe), client=str(client_path), skip_seconds=skip_seconds, **result)


def fixed(value, digits=3, suffix=''):
    return '-' if value is None else f'{value:.{digits}f}{suffix}'


def format_table(result):
    expected, rows, offset, learned = result['expected'], result['rows'], result['offset'], result['learned']
    share = '-' if result['locked_share'] is None else f'{result["locked_share"]:.1%}'
    lines = [f'probe: {result["probe"]}', f'client: {result["client"]}',
             f'rows: {rows["client"]} client, {rows["with_barcode"]} with a barcode, {rows["joined"]} joined, '
             f'{rows["counted"]} counted, {rows["locked"]} locked ({share}), {rows["measured"]} measured '
             f'({rows["pitch_limited"]} at the pitch limit, {rows["below_threshold"]} under {MIN_IDEAL_PX} px)']
    if offset['source'] == 'idle':
        lines.append(f'offset: {offset["counts"][0]},{offset["counts"][1]} counts from {offset["rows"]} rows idle '
                     f'{offset["idle_ms"]:.0f} ms (x {offset["min"][0]}..{offset["max"][0]}, '
                     f'y {offset["min"][1]}..{offset["max"][1]})')
    else:
        lines.append('offset: 0,0 counts, assumed (no idle rows; the probe counted injected moves only)')
    lines.append(f'first lock: {fixed(result["seconds_to_first_lock"], 2, " s")} after the first picture')
    errors = ', '.join(fixed(None if e is None else e * 100, 2, '%') for e in learned['relative_error'])
    lines.append(f'learned gain: {fixed(learned["mouse_gain"][0], 4)}, {fixed(learned["mouse_gain"][1], 4)} px/count '
                 f'(expected {expected["mouse_gain"][0]:.4f}, {expected["mouse_gain"][1]:.4f}; error {errors})')
    lines.append(f'learned lag: {fixed(learned["lag_ms"], 1, " ms")} (probe lag {expected["lag_ms_at_least"]:.1f} ms '
                 'plus drawing to capture)')
    age = result['picture_age_ms']
    lines.append(f'picture age: median {fixed(age["median"], 1, " ms")}, p95 {fixed(age["p95"], 1, " ms")}')
    lines.extend(['', f'{"displacement (px)":<24}{"median":>10}{"p95":>10}'])
    for name, key in (('without Frame Warp', 'without_warp_px'), ('with Frame Warp', 'with_warp_error_px')):
        lines.append(f'{name:<24}{fixed(result[key]["median"], 2):>10}{fixed(result[key]["p95"], 2):>10}')
    bias = result['error_bias_px']
    lines.append(f'error bias: x {fixed(bias[0], 2, " px")}, y {fixed(bias[1], 2, " px")}')
    removed = '-' if result['removed'] is None else f'{result["removed"]:.1%}'
    lines.append(f'removed: {removed} of the input-to-picture displacement')
    lens = result.get('lens')
    if lens:
        share = '-' if lens['with_lens_share'] is None else f'{lens["with_lens_share"]:.1%}'
        lines.extend(['', f'lens: {fixed(lens["field_of_view"], 1)} degrees across (focal {fixed(lens["focal"], 1)} px); '
                          f'learned {fixed(lens["learned_field_of_view"], 1)} degrees '
                          f'(focal {fixed(lens["learned_focal"], 1)} px, {share} of the locked rows with a lens)',
                      f'{"over the picture (px)":<24}{"without":>10}{"p95":>10}{"with":>10}{"p95":>10}{"removed":>10}'])
        for name, key in (('centre', 'centre'), ('whole picture', 'picture'), ('edge', 'edge')):
            part = lens[key]
            removed = '-' if part['removed'] is None else f'{part["removed"]:.1%}'
            lines.append(f'{name:<24}{fixed(part["without_warp_px"]["median"], 2):>10}'
                         f'{fixed(part["without_warp_px"]["p95"], 2):>10}{fixed(part["with_warp_error_px"]["median"], 2):>10}'
                         f'{fixed(part["with_warp_error_px"]["p95"], 2):>10}{removed:>10}')
    return '\n'.join(lines) + '\n'


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split('\n\n')[0])
    parser.add_argument('--probe', type=pathlib.Path, required=True, help="look_probe's OUT.json")
    parser.add_argument('--client', type=pathlib.Path, required=True, help="nightjar-cli's NIGHTJAR_CLI_FRAME_WARP CSV")
    parser.add_argument('--json', type=pathlib.Path, metavar='PATH', help='also write the result as JSON')
    parser.add_argument('--skip-seconds', type=float, default=0.0, metavar='SECONDS',
                        help='leave the first SECONDS after the first client row out of the accuracy figures')
    parser.add_argument('--stream-size', type=stream_size, metavar='WIDTHxHEIGHT',
                        help="the stream's size, when it differs from the probe display's")
    args = parser.parse_args(argv)
    try:
        result = build_report(args.probe, args.client, args.skip_seconds, args.stream_size)
    except (ValueError, OSError) as error:
        print(f'frame_warp_accuracy: {error}', file=sys.stderr)
        return 2
    print(format_table(result), end='')
    if args.json:
        args.json.write_text(json.dumps(result, indent=2) + '\n')
    return 0


if __name__ == '__main__':
    sys.exit(main())
