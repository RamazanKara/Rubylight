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
- Percentiles are soak_result.percentile's (nearest rank, rounding down).
"""
import argparse, csv, json, math, pathlib, sys
from soak_result import percentile

CLIENT_COLUMNS = ('wire_frame', 'barcode_seq', 'decode_qpc_us', 'sent_x', 'sent_y', 'active', 'shift_x', 'shift_y',
                  'locked', 'mouse_gain_x', 'mouse_gain_y', 'lag_ms')
PROBE_COLUMNS = ('frame', 'mouse_sum_x', 'mouse_sum_y', 'camera_y', 'render_qpc_us')
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
                 gain=(number(row['mouse_gain_x']), number(row['mouse_gain_y'])), lag_ms=number(row['lag_ms']))
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


def analyze(report, frames, client, skip_seconds=0.0, size=None):
    expected = report['expected_host_model']
    scale = scale_for(report, size)
    g_true = (expected['mouse_gain'][0] * scale[0], expected['mouse_gain'][1] * scale[1])
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
        length = math.hypot(*ideal)
        if length <= MIN_IDEAL_PX:
            small += 1
            continue
        error = [row['shift'][axis] - ideal[axis] for axis in (0, 1)]
        ideal_px.append(length)
        error_px.append(math.hypot(*error))
        for axis in (0, 1):
            bias[axis].append(error[axis])
    gains = [[row['gain'][axis] for row, _ in locked if row['gain'][axis] is not None] for axis in (0, 1)]
    learned_gain = [percentile(gains[axis], 50) for axis in (0, 1)]
    lags = [row['lag_ms'] for row, _ in locked if row['lag_ms'] is not None]
    return dict(
        expected=dict(mouse_gain=list(g_true), scale=list(scale), lag_ms_at_least=lag_us / 1000),
        offset=offset,
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
