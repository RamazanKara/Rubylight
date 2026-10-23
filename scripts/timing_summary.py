#!/usr/bin/env python3
"""Summarize NIGHTJAR_TIMING_CSV legs of the PyroWave decode protocol.

usage: timing_summary.py [--discard SECONDS] [--keep [--metric NAME]] VARIANT:GROUP:CSV [...]

A leg is one CSV (VARIANT is A or B, GROUP is the bitrate or other condition it ran under). The
first SECONDS (default 5) of each leg are dropped by `decoder_output_ns`. Prints each leg and the
legs of a variant pooled per group. A `<csv minus .csv>.log` next to a leg adds its session-end
lines (counters and stage split). With --keep, prints the protocol's keep rule for B against A per
group on --metric (default input_to_output_ms): both B legs beat both A legs on p50, the pooled p95
is not worse, receive_to_present_ms p95 is at most 0.5 ms worse, shown fps at most 0.5 lower.
Exit code 1 when the rule says revert.
"""
import argparse
import csv
import math
import os
import re
import statistics
import sys

# metric -> percentiles reported
METRICS = {
    "input_to_output_ms": (50, 95, 99),
    "receive_to_input_ms": (95, 99),
    "queue_wait_ms": (95, 99),
    "decode_to_present_ms": (50, 95),
    "receive_to_present_ms": (50, 95, 99),
}
LOG_LINES = re.compile(r"^nightjar: (video:|early decode:|PyroWave (decode|presenter) stages)")


def percentile(values, p):
    """Nearest rank, the rule of stage_times.rs: sorted[min(n - 1, round(p / 100 * (n - 1)))]."""
    if not values:
        return float("nan")
    ordered = sorted(values)
    return ordered[min(len(ordered) - 1, math.floor(p / 100 * (len(ordered) - 1) + 0.5))]


def number(row, name):
    value = row.get(name)
    if value in (None, ""):
        return None
    try:
        return float(value)
    except ValueError:
        return None


def load_leg(path, discard_seconds=5.0):
    with open(path, newline="") as handle:
        rows = list(csv.DictReader(handle))
    stamps = [number(r, "decoder_output_ns") for r in rows]
    stamps = [s for s in stamps if s is not None]
    if not stamps:
        return []
    start = min(stamps)
    return [r for r in rows if (number(r, "decoder_output_ns") or 0) - start >= discard_seconds * 1e9]


def column(rows, name):
    return [v for v in (number(r, name) for r in rows) if v is not None]


def summarize(rows):
    """Metric name -> {percentile: value}, plus shown_fps and rows."""
    result = {"rows": len(rows)}
    for name, percentiles in METRICS.items():
        values = column(rows, name)
        result[name] = {p: percentile(values, p) for p in percentiles}
    fps = column(rows, "shown_fps")
    result["shown_fps"] = statistics.median(fps) if fps else rendered_fps(rows)
    return result


def rendered_fps(rows):
    """Shown fps from the render stamps, for CSVs whose shown_fps column is empty. Gaps over a
    second (between legs, or a stall) are left out so pooled legs do not dilute the rate."""
    stamps = sorted(v for v in column(rows, "render_ns") if v > 0)
    gaps = [(b - a) / 1e9 for a, b in zip(stamps, stamps[1:]) if b - a < 1e9]
    return len(gaps) / sum(gaps) if gaps and sum(gaps) > 0 else float("nan")


def pooled(legs):
    """Summary of all legs together; the shown fps is the mean of the legs' own, since each leg
    has its own clock."""
    result = summarize([row for rows in legs for row in rows])
    rates = [summarize(rows)["shown_fps"] for rows in legs]
    rates = [r for r in rates if r == r]
    if rates and not any(column(rows, "shown_fps") for rows in legs):
        result["shown_fps"] = sum(rates) / len(rates)
    return result


def keep_rule(a_legs, b_legs, metric="input_to_output_ms"):
    """(keep, reasons) for one group; each argument is a list of per-leg row lists."""
    reasons = []
    a_p50 = [summarize(rows)[metric][50] for rows in a_legs]
    b_p50 = [summarize(rows)[metric][50] for rows in b_legs]
    if not (a_p50 and b_p50 and max(b_p50) < min(a_p50)):
        reasons.append(f"{metric} p50: B legs {fmt(b_p50)} do not both beat A legs {fmt(a_p50)}")
    a, b = pooled(a_legs), pooled(b_legs)
    if not b[metric][95] <= a[metric][95]:
        reasons.append(f"{metric} pooled p95 worse: B {b[metric][95]:.2f} vs A {a[metric][95]:.2f}")
    drift = b["receive_to_present_ms"][95] - a["receive_to_present_ms"][95]
    if not drift <= 0.5:
        reasons.append(f"receive_to_present_ms p95 {drift:+.2f} ms")
    if not b["shown_fps"] >= a["shown_fps"] - 0.5:
        reasons.append(f"shown fps {b['shown_fps']:.2f} vs {a['shown_fps']:.2f}")
    return not reasons, reasons


def fmt(values):
    return "[" + ", ".join(f"{v:.2f}" for v in values) + "]"


def describe(title, summary):
    lines = [f"{title}  ({summary['rows']} rows, shown fps median {summary['shown_fps']:.2f})"]
    for name, percentiles in METRICS.items():
        cells = "  ".join(f"p{p} {summary[name][p]:7.2f}" for p in percentiles)
        lines.append(f"  {name:24s} {cells}")
    return lines


def log_lines(csv_path):
    path = os.path.splitext(csv_path)[0] + ".log"
    if not os.path.exists(path):
        return []
    with open(path, errors="replace") as handle:
        return ["  " + line.strip() for line in handle if LOG_LINES.match(line)]


def parse_leg(argument):
    parts = argument.split(":", 2)
    if len(parts) != 3 or not parts[0] or not parts[1]:
        raise ValueError(f"expected VARIANT:GROUP:CSV, got {argument!r}")
    return parts


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("legs", nargs="+")
    parser.add_argument("--discard", type=float, default=5.0)
    parser.add_argument("--keep", action="store_true")
    parser.add_argument("--metric", default="input_to_output_ms", choices=sorted(METRICS))
    args = parser.parse_args(argv)
    groups = {}
    for argument in args.legs:
        variant, group, path = parse_leg(argument)
        rows = load_leg(path, args.discard)
        groups.setdefault(group, {}).setdefault(variant, []).append(rows)
        print("\n".join(describe(f"{variant} @ {group}  {os.path.basename(path)}", summarize(rows))))
        print("\n".join(log_lines(path)))
    ok = True
    for group, variants in groups.items():
        print()
        for variant, legs in sorted(variants.items()):
            print("\n".join(describe(f"{variant} @ {group} pooled over {len(legs)} legs", pooled(legs))))
        if args.keep and "A" in variants and "B" in variants:
            keep, reasons = keep_rule(variants["A"], variants["B"], args.metric)
            ok = ok and keep
            print(f"{group}: {'KEEP' if keep else 'REVERT'}" + ("" if keep else " - " + "; ".join(reasons)))
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
