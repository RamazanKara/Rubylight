"""Per-stage latency of a stream from the host and client timing records (3.0 A8, measure only).

    python timing_report.py CLIENT.csv [--host-record FILE_OR_DIR] [--join MODE] [--json PATH] [--check-bound]

CLIENT.csv is a Nightjar latency CSV (it has `receive_ns`) or a `frames.csv` (it has `wire_frame`) from the C receiver
or nightjar-cli. --host-record is a `host-<stream_id>.csv` from BUTTERPOLLO_TIMING_RECORD, or the directory holding
them. The formats and the join modes are in docs/timing-record.md, section 2.

Assumptions where that document leaves room:
- Offsets follow "client = host + offset" (the sign of Nightjar's present_clock.rs). A frames.csv counts
  `first_packet_us` and `assembled_us` from the receiver's own start (common-c's PltGetMicroseconds), not in absolute
  QPC. `arrival_ms` is absolute QPC: exactly the assembled time with a hardware decoder, the start of decoding
  (assembled plus the software queue) otherwise. shared_clock therefore rebases the receiver clock with
  base_us = min over rows of (arrival_ms * 1000 - assembled_us), where the minimum strips the queue, and uses
  offset = media_epoch_qpc_ms * 1000 - base_us. A file without `arrival_ms` cannot be rebased and is never joined
  that way.
- Host-side values are integer microseconds on the media clock. `rtp_us` and `rtp_host_us` are the 32-bit RTP stamp
  times 1000 / 90, so they wrap every 2**32 * 1000 / 90 us (13.3 h). They are unwrapped next to a host stamp of the
  same frame before they are subtracted.
- The nine Nightjar columns hold the stamps named in the document (`rtp_host_us`, `host_present_us`,
  `host_acquired_us`, `host_submitted_us`, `host_encoded_us`, `host_first_sent_us`, `host_last_sent_us`) and
  `clock_offset_us`, `clock_rtt_us`. They carry no claim time, so without a host record `hold_claim` and `convert`
  stay empty and `hold_claim_convert` (acquired to submitted) is reported instead. A host record row replaces the
  client's host columns for that frame.
- Nightjar's `frame_number` equals the host record's `wire_frame`. Nightjar times are nanoseconds, a frames.csv's
  `first_packet_us`, `assembled_us` and the nine columns are microseconds, in the receiver's clock.
- A frames.csv has no display stage. Its end point for `end_to_end` and `client_total` is the end of decoding
  (assembled_us + decode_ms), which leaves out any queue before a hardware decoder. `render_qpc` (raw QPC ticks) is
  not used.
- shared_clock is chosen automatically only for a frames.csv whose rebased `assembled_us` lies within [-5 ms, 1 s] of
  the host record's last packet on the QPC base (median over the frames). Otherwise the receiver is on another clock and
  min_owd is used. --join shared_clock on a file that fails this check is an error.
- --check-bound compares each row's `clock_offset_us` with the shared offset and fails when more than 1% of the rows
  with an offset and an RTT differ by more than `clock_rtt_us / 2`. The shared offset is rebased like shared_clock's
  when the file has `arrival_ms`, and is media_epoch_qpc_ms * 1000 (an absolute receiver clock) when it has none.
- A host record without its `# dropped=` footer (the host was stopped before the stream wrote it) has an unknown
  dropped count, shown as such, never 0.
- Negative durations are kept in the percentiles (across clocks they are inside the bound) and counted per stage.
"""
import argparse, csv, json, pathlib, sys
from soak_result import percentile

RECORD_MAGIC = 'rubylight timing record 1'
HOST_COLUMNS = ('wire_frame', 'rtp_us', 'present_us', 'acquired_us', 'published_us', 'claimed_us', 'submitted_us',
                'encoded_us', 'packetized_us', 'prepared_us', 'first_sent_us', 'last_sent_us')
HOST_FIELDS = ('rtp', 'present', 'acquired', 'published', 'claimed', 'submitted', 'encoded', 'packetized', 'prepared',
               'first_sent', 'last_sent')
NIGHTJAR_HOST_COLUMNS = dict(rtp='rtp_host_us', present='host_present_us', acquired='host_acquired_us',
                             submitted='host_submitted_us', encoded='host_encoded_us',
                             first_sent='host_first_sent_us', last_sent='host_last_sent_us')
NIGHTJAR_COLUMNS = ('frame_number', 'receive_ns', 'enqueue_ns', 'decoder_input_ns', 'decoder_output_ns', 'render_ns',
                    'status', *NIGHTJAR_HOST_COLUMNS.values(), 'clock_offset_us', 'clock_rtt_us')
FRAMES_COLUMNS = ('wire_frame', 'first_packet_us', 'assembled_us', 'decode_ms', 'arrival_ms',
                  *NIGHTJAR_HOST_COLUMNS.values(), 'clock_offset_us', 'clock_rtt_us')
WRAP_US = (1 << 32) * 1000 / 90
JOINS = ('clock_sync', 'shared_clock', 'min_owd')
# frames.csv assembled_us minus the host's last packet on the QPC base, for a receiver that reads the host's QPC.
SHARED_WINDOW_US = (-5_000, 1_000_000)
BOUND_FAIL_FRACTION = .01
STAGES = (
    ('game_to_present', 'host'), ('capture', 'host'), ('hold_claim', 'host'), ('convert', 'host'),
    ('hold_claim_convert', 'host'), ('encode', 'host'), ('packetize_pace', 'host'), ('send', 'host'),
    ('host_total', 'host'),
    ('network_first', 'cross'), ('network_last', 'cross'), ('end_to_end', 'cross'),
    ('assemble', 'client'), ('client_queue', 'client'), ('decode', 'client'), ('display', 'client'),
    ('client_total', 'client'))


def number(text, column):
    text = (text or '').strip()
    if not text:
        return None
    try:
        return float(text)
    except ValueError:
        raise ValueError(f'column {column}: {text!r} is not a number')


def unwrap_rtp(rtp_us, anchor_us):
    """The RTP time on the host clock nearest to a host stamp of the same frame."""
    if rtp_us is None or anchor_us is None or abs(rtp_us - anchor_us) < WRAP_US / 2:
        return rtp_us
    return anchor_us + (rtp_us - anchor_us + WRAP_US / 2) % WRAP_US - WRAP_US / 2


def median(values):
    return percentile(values, 50)


def read_host_record(path):
    path = pathlib.Path(path)
    if path.is_dir():
        found = sorted(path.glob('host-*.csv'), key=lambda p: (p.stat().st_size, p.name))
        if not found:
            raise ValueError(f'{path}: no host-*.csv timing record in it')
        path = found[-1]
    header, lines = {}, []
    for number_, line in enumerate(path.read_text().splitlines(), 1):
        if not line.strip():
            continue
        if line.startswith('#'):
            body = line[1:].strip()
            if not header and not lines and not body.startswith(RECORD_MAGIC):
                raise ValueError(f'{path.name}: not a "{RECORD_MAGIC}" file (first line: {line[:60]!r})')
            header.setdefault('_comments', []).append(body)
            for token in body.split():
                if '=' in token:
                    key, value = token.split('=', 1)
                    header[key] = value
        else:
            if not header:
                raise ValueError(f'{path.name}: missing the "# {RECORD_MAGIC}" line')
            lines.append((number_, line))
    reader = csv.DictReader(line for _, line in lines)
    missing = [c for c in HOST_COLUMNS if c not in (reader.fieldnames or [])]
    if missing:
        raise ValueError(f'{path.name}: missing host record columns: {", ".join(missing)}')
    rows, duplicates = {}, 0
    for row in reader:
        values = {field: number(row[f'{field}_us'], f'{field}_us') for field in HOST_FIELDS}
        frame = int(row['wire_frame'])
        if frame in rows:
            duplicates += 1
            continue
        rows[frame] = values
    epoch = number(header.get('media_epoch_qpc_ms'), 'media_epoch_qpc_ms')
    return dict(path=str(path), rows=rows, epoch_qpc_ms=epoch, codec=header.get('codec'), capture=header.get('capture'),
                present_source=header.get('present_source'), dropped=int(header['dropped']) if 'dropped' in header else None,
                duplicates=duplicates)


def read_client(path):
    path = pathlib.Path(path)
    with path.open(newline='') as stream:
        reader = csv.DictReader(stream)
        fields = list(reader.fieldnames or [])
        rows = list(reader)
    if 'receive_ns' in fields and 'frame_number' in fields:
        kind, required, known = 'nightjar', ('frame_number', 'receive_ns'), NIGHTJAR_COLUMNS
    elif 'wire_frame' in fields:
        kind, required, known = 'frames', ('wire_frame', 'first_packet_us', 'assembled_us'), FRAMES_COLUMNS
    else:
        raise ValueError(f'{path.name}: neither a Nightjar latency CSV (receive_ns) nor a frames.csv (wire_frame)')
    absent = [c for c in required if c not in fields]
    if absent:
        raise ValueError(f'{path.name}: missing columns: {", ".join(absent)}')
    frames = []
    for index, row in enumerate(rows, 2):
        try:
            frames.append(client_frame(kind, row))
        except ValueError as error:
            raise ValueError(f'{path.name}: line {index}: {error}') from None
    return dict(path=str(path), kind=kind, frames=frames, missing=[c for c in known if c not in fields])


def client_frame(kind, row):
    host = dict.fromkeys(HOST_FIELDS)
    host.update({field: number(row.get(column), column) for field, column in NIGHTJAR_HOST_COLUMNS.items()})
    frame = dict(host=host, off=number(row.get('clock_offset_us'), 'clock_offset_us'),
                 rtt=number(row.get('clock_rtt_us'), 'clock_rtt_us'))
    if kind == 'nightjar':
        us = lambda column: None if number(row.get(column), column) is None else number(row.get(column), column) / 1000
        frame.update(frame_number=int(row['frame_number']), first_packet=us('receive_ns'), assembled=us('enqueue_ns'),
                     dec_in=us('decoder_input_ns'), dec_out=us('decoder_output_ns'), decode=None, arrival=None)
        status = row.get('status')
        render = us('render_ns')
        frame['present'] = render if status in (None, '', 'rendered') else None
        frame['end'] = frame['present']
    else:
        decode_ms = number(row.get('decode_ms'), 'decode_ms')
        arrival_ms = number(row.get('arrival_ms'), 'arrival_ms')
        frame.update(frame_number=int(row['wire_frame']), first_packet=number(row['first_packet_us'], 'first_packet_us'),
                     assembled=number(row['assembled_us'], 'assembled_us'), arrival=None if arrival_ms is None else arrival_ms * 1000, dec_in=None, dec_out=None, present=None,
                     decode=None if decode_ms is None else decode_ms * 1000)
        frame['end'] = None if frame['assembled'] is None or frame['decode'] is None else frame['assembled'] + frame['decode']
    return frame


def has_host(frame):
    return any(value is not None for value in frame['host'].values())


def qpc_base(frames):
    """The receiver's clock start on the QPC base: absolute QPC minus the receiver's own microseconds. arrival_ms is
    QPC at assembly with a hardware decoder and later with a software one, so the minimum strips the queue."""
    bases = [f['arrival'] - f['assembled'] for f in frames if f.get('arrival') is not None and f['assembled'] is not None]
    return min(bases) if bases else None


def shared_offset(frames, record):
    """client = host + offset for a receiver that reads the host's QPC; None when arrival_ms cannot rebase it."""
    base = qpc_base(frames)
    return None if base is None else record['epoch_qpc_ms'] * 1000 - base


def shared_plausible(frames, record):
    """Does the receiver read the host's QPC? True when its last-packet times fit the host's last-sent times."""
    epoch_us = shared_offset(frames, record)
    if epoch_us is None:
        return False
    gaps = [f['assembled'] - (f['host']['last_sent'] + epoch_us) for f in frames
            if f['assembled'] is not None and f['host']['last_sent'] is not None]
    return bool(gaps) and SHARED_WINDOW_US[0] <= median(gaps) <= SHARED_WINDOW_US[1]


def attach_host(frames, record):
    if record is None:
        return
    for frame in frames:
        row = record['rows'].get(frame['frame_number'])
        if row is not None:
            frame['host'] = dict(row)


def choose_join(client, record, forced):
    frames = client['frames']
    clock = any(f['off'] is not None for f in frames)
    host_data = record is not None or any(has_host(f) for f in frames)
    if forced == 'clock_sync' and not clock:
        raise ValueError('--join clock_sync needs a clock_offset_us column with values')
    if forced == 'shared_clock':
        if record is None or record['epoch_qpc_ms'] is None:
            raise ValueError('--join shared_clock needs a host record with media_epoch_qpc_ms')
        if client['kind'] != 'frames':
            raise ValueError('--join shared_clock needs a frames.csv from a receiver that reads the host QPC')
        if qpc_base(frames) is None:
            raise ValueError('--join shared_clock needs an arrival_ms column to rebase the receiver clock to QPC')
        if not shared_plausible(frames, record):
            raise ValueError('--join shared_clock: the receiver times do not fit the host QPC '
                             '(median rebased assembled_us minus last packet sent is outside -5 ms .. 1 s)')
    if forced == 'min_owd' and not host_data:
        raise ValueError('--join min_owd needs host data (a host record or the host columns)')
    if forced:
        return forced
    if not host_data:
        return None
    if clock:
        return 'clock_sync'
    if (record is not None and record['epoch_qpc_ms'] is not None and client['kind'] == 'frames'
            and shared_plausible(frames, record)):
        return 'shared_clock'
    return 'min_owd'


def assign_offsets(frames, join, record):
    """Set frame['clock'] to the offset (client = host + offset) per frame, and return the bound in us (None: unknown)."""
    if join == 'clock_sync':
        for frame in frames:
            frame['clock'] = frame['off']
        rtts = [f['rtt'] for f in frames if f['rtt'] is not None and f['off'] is not None]
        return median(rtts) / 2 if rtts else None
    if join == 'shared_clock':
        offset = shared_offset(frames, record)
        for frame in frames:
            frame['clock'] = offset
        return 0
    if join == 'min_owd':
        gaps = [f['first_packet'] - f['host']['first_sent'] for f in frames
                if f['first_packet'] is not None and f['host']['first_sent'] is not None]
        if not gaps:
            raise ValueError('min_owd: no frame has both a first packet time and a first_sent time')
        for frame in frames:
            frame['clock'] = min(gaps)
        return None
    for frame in frames:
        frame['clock'] = None
    return None


def difference(end, start):
    return None if end is None or start is None else end - start


def stage_values(frame, record_present):
    """Stage durations in microseconds for one frame; None where an input is missing."""
    h = frame['host']
    anchor = next((h[k] for k in ('present', 'acquired', 'submitted', 'encoded', 'first_sent', 'last_sent')
                   if h[k] is not None), None)
    game = unwrap_rtp(h['rtp'], anchor)
    split = h['claimed'] is not None
    values = dict(
        game_to_present=difference(h['present'], game) if record_present else None,
        capture=difference(h['acquired'], h['present']),
        hold_claim=difference(h['claimed'], h['acquired']),
        convert=difference(h['submitted'], h['claimed']),
        hold_claim_convert=None if split else difference(h['submitted'], h['acquired']),
        encode=difference(h['encoded'], h['submitted']),
        packetize_pace=difference(h['first_sent'], h['encoded']),
        send=difference(h['last_sent'], h['first_sent']),
        host_total=difference(h['last_sent'], h['present']),
        assemble=difference(frame['assembled'], frame['first_packet']),
        client_queue=difference(frame['dec_in'], frame['assembled']),
        decode=frame['decode'] if frame['decode'] is not None else difference(frame['dec_out'], frame['dec_in']),
        display=difference(frame['present'], frame['dec_out']))
    if frame['decode'] is not None:
        values['client_total'] = difference(frame['end'], frame['first_packet'])
    else:
        values['client_total'] = difference(frame['present'], frame['first_packet'])
    off = frame.get('clock')
    cross = dict(network_first=None, network_last=None, end_to_end=None)
    if off is not None:
        shifted = lambda value: None if value is None else value + off
        cross = dict(network_first=difference(frame['first_packet'], shifted(h['first_sent'])),
                     network_last=difference(frame['assembled'], shifted(h['last_sent'])),
                     end_to_end=difference(frame['end'], shifted(game)))
    values.update(cross)
    return values


def summarize(values):
    ms = [v / 1000 for v in values]
    return dict(n=len(ms), median_ms=median(ms), p95_ms=percentile(ms, 95), p99_ms=percentile(ms, 99),
                negative=sum(v < 0 for v in ms))


def build_report(client_path, record_path=None, join=None):
    client = read_client(client_path)
    record = read_host_record(record_path) if record_path else None
    frames = client['frames']
    attach_host(frames, record)
    mode = choose_join(client, record, join)
    bound_us = assign_offsets(frames, mode, record)
    record_present = record is None or record['present_source'] != 'capture'
    columns = {name: [] for name, _ in STAGES}
    for frame in frames:
        for name, value in stage_values(frame, record_present).items():
            if value is not None:
                columns[name].append(value)
    stages = {}
    for name, scope in STAGES:
        stage = summarize(columns[name])
        stage['scope'] = scope
        if scope == 'cross':
            stage['bound_ms'] = None if bound_us is None else bound_us / 1000
            stage['lower_bound'] = mode == 'min_owd'
        stages[name] = stage
    numbers = {f['frame_number'] for f in frames}
    without_host = sum(not has_host(f) for f in frames)
    unmatched = dict(client_without_host=without_host,
                     host_without_client=None if record is None else sum(n not in numbers for n in record['rows']))
    notes = []
    if mode is None:
        notes.append('no host data: only the client stages are reported')
    if mode == 'min_owd':
        notes.append('min_owd: cross-clock stages are the delay above the minimum one-way delay; end_to_end is a lower bound')
    if client['kind'] == 'frames':
        notes.append('frames.csv has no display stage: end_to_end and client_total end when decoding ends')
    if not record_present:
        notes.append('present_source=capture: game_to_present is not reported')
    if stages['hold_claim_convert']['n'] and not stages['hold_claim']['n']:
        notes.append('the client columns carry no claim time: hold_claim and convert need the host record')
    if record is not None and record['dropped'] is None:
        notes.append('the host record has no "# dropped=" footer: it was cut short and the dropped count is unknown')
    if client['missing'] and record is None:
        notes.append('columns absent from the client file: ' + ', '.join(client['missing']))
    return dict(client=client['path'], client_kind=client['kind'], host_record=record and record['path'], join=mode,
                bound_ms=None if bound_us is None else bound_us / 1000, rows=len(frames),
                matched=len(frames) - without_host if record is None else sum(f['frame_number'] in record['rows'] for f in frames),
                unmatched=unmatched, host_rows=None if record is None else len(record['rows']),
                host_dropped=None if record is None else record['dropped'],
                host_footer=None if record is None else record['dropped'] is not None,
                host_duplicate_rows=None if record is None else record['duplicates'],
                stages=stages, notes=notes), client, record


def has_footer(path):
    lines = [line for line in path.read_text(errors='replace').splitlines() if line.strip()]
    return bool(lines) and lines[-1].startswith('# dropped=')


def wait_for_footer(directory, seconds):
    """Wait until every host-*.csv in the directory (at least one) ends with its footer; False on timeout. The host
    writes the footer when the stream is torn down, a moment after the receiver has gone."""
    import time
    directory = pathlib.Path(directory)
    deadline = time.monotonic() + seconds
    while True:
        files = sorted(directory.glob('host-*.csv'))
        if files and all(has_footer(f) for f in files):
            return True
        if time.monotonic() >= deadline:
            return False
        time.sleep(.1)


def case_report(case):
    """The report of an e2e.py --record case: its receiver's frames.csv and the host records in record/."""
    case = pathlib.Path(case)
    return build_report(case / 'frames.csv', case / 'record')[0]


def check_bound(frames, record):
    """Compare each row's clock_offset_us with the shared QPC offset; the rows it differs on by more than rtt/2."""
    if record is None or record['epoch_qpc_ms'] is None:
        raise ValueError('--check-bound needs a host record with media_epoch_qpc_ms')
    shared = shared_offset(frames, record)
    if shared is None:
        shared = record['epoch_qpc_ms'] * 1000
    rows = [f for f in frames if f['off'] is not None and f['rtt'] is not None]
    if not rows:
        raise ValueError('--check-bound needs a client file with clock_offset_us and clock_rtt_us values')
    over = [abs(f['off'] - shared) - f['rtt'] / 2 for f in rows if abs(f['off'] - shared) > f['rtt'] / 2]
    fraction = len(over) / len(rows)
    errors = [abs(f['off'] - shared) for f in rows]
    return dict(rows=len(rows), outside=len(over), fraction=fraction, limit=BOUND_FAIL_FRACTION,
                max_error_us=max(errors), median_error_us=median(errors), passed=fraction <= BOUND_FAIL_FRACTION)


def format_table(report):
    join = report['join'] or 'none'
    if join == 'clock_sync':
        bound = 'none' if report['bound_ms'] is None else f'+/- {report["bound_ms"]:.3f} ms (median clock_rtt_us / 2)'
    elif join == 'shared_clock':
        bound = '+/- 0 ms (host and receiver read the same QPC)'
    elif join == 'min_owd':
        bound = 'lower bounds (delay above the minimum one-way delay)'
    else:
        bound = 'n/a'
    unmatched = report['unmatched']
    lines = [f'client: {report["client"]} ({report["client_kind"]})',
             f'host record: {report["host_record"] or "none"}',
             f'join: {join}   bound: {bound}',
             f'rows: {report["rows"]} client, {report["host_rows"] if report["host_rows"] is not None else "-"} host; '
             f'client without host data {unmatched["client_without_host"]}, '
             f'host without client row {unmatched["host_without_client"] if unmatched["host_without_client"] is not None else "-"}'
             + ('' if report['host_footer'] is None else f', host rows dropped by the writer {report["host_dropped"]}'
                if report['host_footer'] else ', host rows dropped by the writer unknown (no footer)'),
             '',
             f'{"stage":<20}{"n":>6}{"median":>10}{"p95":>10}{"p99":>10}  bound (ms)']
    for name, stage in report['stages'].items():
        if not stage['n'] and name == 'hold_claim_convert':
            continue
        cells = ('-', '-', '-') if not stage['n'] else tuple(f'{stage[k]:.3f}' for k in ('median_ms', 'p95_ms', 'p99_ms'))
        mark = ''
        if stage['scope'] == 'cross' and stage['n']:
            if stage['lower_bound']:
                mark = 'lower bound'
            elif stage['bound_ms'] is not None:
                mark = f'+/- {stage["bound_ms"]:.3f}'
        lines.append(f'{name:<20}{stage["n"]:>6}{cells[0]:>10}{cells[1]:>10}{cells[2]:>10}  {mark}'.rstrip())
    lines.extend(['', *(f'note: {note}' for note in report['notes'])])
    return '\n'.join(lines).rstrip() + '\n'


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split('\n\n')[0])
    parser.add_argument('client', type=pathlib.Path, help='a Nightjar latency CSV or a frames.csv')
    parser.add_argument('--host-record', type=pathlib.Path, help='host-<stream_id>.csv, or the directory holding it')
    parser.add_argument('--join', choices=JOINS, help='force a join mode instead of choosing one')
    parser.add_argument('--json', type=pathlib.Path, metavar='PATH', help='also write the report as JSON')
    parser.add_argument('--check-bound', action='store_true',
                        help='fail when clock_offset_us differs from the shared QPC offset by more than rtt/2 on over 1%% of rows')
    args = parser.parse_args(argv)
    try:
        report, client, record = build_report(args.client, args.host_record, args.join)
        if args.check_bound:
            report['bound_check'] = check_bound(client['frames'], record)
    except (ValueError, OSError) as error:
        print(f'timing_report: {error}', file=sys.stderr)
        return 2
    print(format_table(report), end='')
    if args.check_bound:
        check = report['bound_check']
        print(f'bound check: {"PASS" if check["passed"] else "FAIL"} - {check["outside"]} of {check["rows"]} rows differ from the '
              f'shared offset by more than rtt/2 ({check["fraction"]:.2%}, limit {check["limit"]:.0%}); '
              f'median error {check["median_error_us"]:.0f} us, max {check["max_error_us"]:.0f} us')
    if args.json:
        args.json.write_text(json.dumps(report, indent=2) + '\n')
    return 0 if not args.check_bound or report['bound_check']['passed'] else 1


if __name__ == '__main__':
    sys.exit(main())
