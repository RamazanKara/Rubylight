#!/usr/bin/env python3
"""Where the wait between decoded and shown goes, from the Nightjar desktop timing CSVs.

  timing_breakdown.py <ABBA folder> [--fps 60] [--discard 5]

The folder is a pyrowave-loopback.ps1 result (NN-A-<group>.csv, NN-B-<group>.csv). For every
group the A and the B legs are pooled and one table is printed per variant:

  hold           decoder_output_ns -> due_ns         the pacer's wait for the frame's time
  due->begin     due_ns -> present_begin_ns          the presenter's wake-up and queueing
  call           present_begin_ns -> render_ns       the draw and SDL_RenderPresent
  call->flip     render_ns -> the vblank of the swap chain statistics that count this present
                                                     (needs a later row; empty with tearing, where
                                                     the chain reports no usable vblank)
  jitter         the interval between two presents (render_ns) minus 1000 / fps, p99 of the
                 absolute value, plus the p50 and p99 of the interval itself

All values are ms. The stat columns are those written by the 42f6bdb0 timing CSV change.
"""
import argparse
import csv
import glob
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from timing_summary import column, load_leg, number, percentile, rendered_fps  # noqa: E402


def stages(rows):
    """Per-frame stage lengths in ms, by name."""
    out = {"hold": [], "due->begin": [], "call": [], "call->flip": [], "decoded->flip": []}
    syncs = {}
    for r in rows:
        count, sync = number(r, "stat_present_count"), number(r, "stat_sync_ns")
        if count is not None and sync:
            syncs.setdefault(int(count), sync)
    for r in rows:
        ready, due, begin, shown = (number(r, k) for k in ("decoder_output_ns", "due_ns", "present_begin_ns", "render_ns"))
        if None in (ready, due, begin, shown) or not due or not begin:
            continue
        out["hold"].append((due - ready) / 1e6)
        out["due->begin"].append((begin - due) / 1e6)
        out["call"].append((shown - begin) / 1e6)
        count = number(r, "present_count")
        sync = syncs.get(int(count)) if count else None
        if sync and sync >= shown:
            out["call->flip"].append((sync - shown) / 1e6)
            out["decoded->flip"].append((sync - ready) / 1e6)
    return out


def intervals(rows):
    stamps = sorted(v for v in column(rows, "render_ns") if v > 0)
    return [(b - a) / 1e6 for a, b in zip(stamps, stamps[1:])]


def cell(values, ps):
    if not values:
        return "      -"
    return "  ".join(f"p{p} {percentile(values, p):6.2f}" for p in ps)


def report(title, legs, fps):
    rows = [r for leg in legs for r in leg]
    print(f"{title}: {len(rows)} frames in {len(legs)} legs")
    for name, values in stages(rows).items():
        print(f"  {name:14s} n {len(values):6d}  {cell(values, (50, 95, 99))}")
    for name in ("decode_to_present_ms", "receive_to_present_ms"):
        print(f"  {name:21s} {cell(column(rows, name), (50, 95, 99))}")
    gaps = [g for leg in legs for g in intervals(leg)]
    nominal = 1000.0 / fps
    deviation = [abs(g - nominal) for g in gaps]
    rates = [rendered_fps(leg) for leg in legs]
    print(f"  present interval  {cell(gaps, (50, 99))}   |interval - {nominal:.2f}| {cell(deviation, (50, 99))}")
    print(f"  shown fps (mean of legs) {sum(rates) / len(rates):.2f}")


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("folder")
    parser.add_argument("--fps", type=float, default=60.0)
    parser.add_argument("--discard", type=float, default=5.0)
    args = parser.parse_args(argv)
    groups = {}
    for path in sorted(glob.glob(os.path.join(args.folder, "*.csv"))):
        match = re.match(r"\d+-([AB])-(\w+)\.csv$", os.path.basename(path))
        if match:
            groups.setdefault(match.group(2), {}).setdefault(match.group(1), []).append(load_leg(path, args.discard))
    for group, variants in groups.items():
        print(f"== {group} Mbps ==")
        for variant, legs in sorted(variants.items()):
            report(f"{variant}", legs, args.fps)
    return 0


if __name__ == "__main__":
    sys.exit(main())
