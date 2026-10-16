"""Stage timings from an isolated host's log and the receiver's frame CSV.

The host must run with RUST_LOG=info,stages=trace: each new picture's stages
are logged as `stages: picture capture_us=.. hold_us=.. ...`.
"""
import csv, ctypes, datetime, json, math, re

STAGES = ['capture', 'hold', 'claim', 'convert', 'encode', 'queue', 'packetize', 'send',
          'present_to_sent', 'moonlight_host']
LINE = re.compile(r'^(\S+Z)\s+TRACE stages: picture (.*)$')


def when(stamp):
    return datetime.datetime.fromisoformat(stamp.replace('Z', '+00:00')).timestamp()


def spread(values):
    if not values:
        return None
    ordered = sorted(values)
    at = lambda p: ordered[(len(ordered) - 1) * p // 100]
    return dict(mean=sum(ordered) / len(ordered), p50=at(50), p95=at(95), p99=at(99), max=ordered[-1], n=len(ordered))


def host_stages(log_text, warmup=3.0, tail=1.0):
    """Per-stage spreads in ms over every new picture after the warm-up."""
    samples = []
    for line in log_text.splitlines():
        match = LINE.match(line)
        if match:
            fields = dict(re.findall(r'(\w+)=(\d+)', match.group(2)))
            samples.append((when(match.group(1)), {k: int(fields[k + '_us']) / 1000 for k in STAGES}))
    if not samples:
        return None
    first, last = samples[0][0], samples[-1][0]
    kept = [s for t, s in samples if first + warmup <= t <= last - tail]
    result = {k: spread([s[k] for s in kept]) for k in STAGES}
    result['seconds'] = round(last - tail - first - warmup, 2)
    return result


def timings_lines(log_text):
    """The 5 s 'stream timings' lines' numeric fields."""
    lines = []
    for line in log_text.splitlines():
        if 'stream timings' in line:
            lines.append({k: v for k, v in re.findall(r'(\w+)=(\S+)', line)})
    return lines


def qpc_frequency():
    frequency = ctypes.c_int64()
    ctypes.windll.kernel32.QueryPerformanceFrequency(ctypes.byref(frequency))
    return frequency.value


def receiver(csv_path, warmup=3.0):
    """Render (the probe's QPC before Present) to fully received, and the host latency
    Moonlight shows, per frame after the warm-up. arrival_ms is the assembly time on
    the QPC clock when the receiver decodes on the GPU."""
    try:
        rows = list(csv.DictReader(open(csv_path, newline='')))
    except FileNotFoundError:
        return None
    if not rows:
        return None
    frequency = qpc_frequency()
    start = float(rows[0]['arrival_ms'])
    received, first_packet, host = [], [], []
    for row in rows:
        arrival = float(row['arrival_ms'])
        if arrival < start + warmup * 1000:
            continue
        host.append(float(row['host_ms']))
        ticks = int(row['render_qpc'])
        if int(row['render_frame']) and ticks and float(row['picture_age_ms']) >= 0:
            age = arrival - ticks * 1000 / frequency
            if 0 <= age < 1000:
                received.append(age)
                spread_ms = (int(row['assembled_us']) - int(row['first_packet_us'])) / 1000
                first_packet.append(age - spread_ms)
    return dict(render_to_received=spread(received), render_to_first_packet=spread(first_packet),
                moonlight_host_shown=spread(host))
