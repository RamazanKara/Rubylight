"""Scene-cut metrics for bottleneck.py, from the receiver's frames.csv and the host log.

A cut is a frame larger than twice the nominal frame (the encoder bitrate over
the frame rate). Ordinary frames exclude each cut and the two frames after it,
the frames that answer a lost cut.
"""
import pathlib, re, sys

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
