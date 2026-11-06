#!/usr/bin/env python3
"""A/B of Home's speed on a device: cold start, frame times of a scripted D-pad walk, and press to moved-focus frame.

Build each side from the debug sources but not debuggable, side by side:
  gradlew :app:assembleNonRootDebug -PdebugNotDebuggable -PdebugIdSuffix=.njglass -PabiFilters=armeabi-v7a,arm64-v8a

  python scripts/home-speed.py --serial 192.168.4.116:5555 --package com.butterpollo.client.njglass --glass baked --label baked --out out/speed
  python scripts/home-speed.py --serial e24bb88e --package com.butterpollo.client.njbase --label base --swipes 12 --out out/speed

Each run force-stops the app, cold-starts the demo Home with `am start -W`, resets gfxinfo, walks, and reads gfxinfo and
the NjLatency lines DemoFocusLatency logged. The keys come from one `monkey -f` process: `input keyevent` starts a
Java process per press, far slower than the cadence on a TV. Writes one JSON per run and <label>.json with the medians.
"""
import argparse
import json
import re
import statistics
import subprocess
import tempfile
import time
from pathlib import Path

LAUNCHER = "com.limelight.demo.DemoLauncherActivity"
RIGHT, DOWN, LEFT, UP = 22, 20, 21, 19
# Down to the demo's long Steam shelf, along it and back, so nearly every press moves focus and none hits a shelf's end.
PATTERN = [DOWN, DOWN] + ([RIGHT] * 8 + [LEFT] * 8) * 3
REMOTE = "/data/local/tmp/nj-walk.txt"

STATS = {
    "frames": r"Total frames rendered:\s*(\d+)",
    "janky": r"Janky frames:\s*(\d+)",
    "janky_pct": r"Janky frames:\s*\d+\s*\(([\d.]+)%\)",
    "p50": r"50th percentile:\s*(\d+)ms",
    "p90": r"90th percentile:\s*(\d+)ms",
    "p95": r"95th percentile:\s*(\d+)ms",
    "p99": r"99th percentile:\s*(\d+)ms",
    "missed_vsync": r"Number Missed Vsync:\s*(\d+)",
    "high_input_latency": r"Number High input latency:\s*(\d+)",
    "slow_ui": r"Number Slow UI thread:\s*(\d+)",
    "slow_upload": r"Number Slow bitmap uploads:\s*(\d+)",
    "slow_issue": r"Number Slow issue draw commands:\s*(\d+)",
    "deadline_missed": r"Number Frame deadline missed:\s*(\d+)",
}


class Adb:
    def __init__(self, serial):
        self.base = ["adb", "-s", serial]

    def run(self, *args):
        result = subprocess.run(self.base + list(args), capture_output=True, text=True, encoding="utf-8", errors="replace")
        if result.returncode != 0:
            raise RuntimeError(f"adb {' '.join(args)} failed: {result.stderr.strip()}")
        return result.stdout

    def shell(self, *args):
        return self.run("shell", *args)

    def screencap(self, path):
        Path(path).write_bytes(subprocess.run(self.base + ["exec-out", "screencap", "-p"], capture_output=True, check=True).stdout)


def monkey_script(presses, gap_ms):
    lines = ["type=raw events", f"count={presses * 2}", "speed=1.0", "start data >>"]
    for i in range(presses):
        lines += [f"DispatchPress({PATTERN[i % len(PATTERN)]})", f"UserWait({gap_ms})"]
    return "\n".join(lines) + "\n"


def percentile(values, q):
    ordered = sorted(values)
    return ordered[min(len(ordered) - 1, int(len(ordered) * q))] if ordered else None


def parse_stats(dump):
    result = {}
    for name, pattern in STATS.items():
        match = re.search(pattern, dump)
        result[name] = float(match.group(1)) if match else None
    return result


def parse_framestats(dump):
    """Median and p90 ms of where the last frames' time went: the UI thread, the render thread's issue, and the GPU."""
    lines = dump.split("---PROFILEDATA---")
    if len(lines) < 2:
        return {}
    rows = [line.split(",") for line in lines[1].strip().splitlines() if line.strip()]
    if len(rows) < 2:
        return {}
    col = {name: i for i, name in enumerate(rows[0]) if name}
    parts = {"ui": ("IntendedVsync", "SyncQueued"), "sync": ("SyncStart", "IssueDrawCommandsStart"),
             "issue": ("IssueDrawCommandsStart", "SwapBuffers"), "swap": ("SwapBuffers", "FrameCompleted"),
             "gpu": ("IssueDrawCommandsStart", "GpuCompleted")}
    frames = [row for row in rows[1:] if len(row) >= len(col) and row[col["Flags"]] == "0"]
    result = {"fs_frames": len(frames)}
    for name, (start, end) in parts.items():
        if start not in col or end not in col:
            continue
        values = [(int(row[col[end]]) - int(row[col[start]])) / 1e6 for row in frames
                  if int(row[col[end]]) > 0 and int(row[col[start]]) > 0]
        result[f"{name}_p50"] = percentile(values, 0.5)
        result[f"{name}_p90"] = percentile(values, 0.9)
    return result


def one_run(adb, args, extras, index, out):
    adb.shell("am", "force-stop", args.package)
    time.sleep(1.0)
    adb.run("logcat", "-c")
    started = adb.shell("am", "start", "-W", "-n", f"{args.package}/{LAUNCHER}", "--es", "state", "home", *extras)
    total = re.search(r"TotalTime:\s*(\d+)", started)
    if total is None:
        raise RuntimeError(f"no TotalTime: {started}")
    time.sleep(args.settle)
    if index == 0:
        adb.screencap(out / f"{args.label}-start.png")
    adb.shell("dumpsys", "gfxinfo", args.package, "reset")
    begun = time.monotonic()
    adb.shell("monkey", "-f", REMOTE, "1")
    walked = time.monotonic() - begun
    for i in range(args.swipes):
        y0, y1 = (800, 250) if i % 2 == 0 else (250, 800)
        adb.shell("input", "swipe", str(args.swipe_x), str(y0), str(args.swipe_x), str(y1), "220")
        time.sleep(0.25)
    time.sleep(0.6)
    if index == 0:
        adb.screencap(out / f"{args.label}-end.png")
    dump = adb.shell("dumpsys", "gfxinfo", args.package, "framestats")
    (out / f"{args.label}-{index}-gfxinfo.txt").write_text(dump, encoding="utf-8")
    log = adb.run("logcat", "-d", "-s", "NjLatency:I")
    to_frame = [float(v) for v in re.findall(r"toFrameMs=([\d.]+)", log)]
    to_handled = [float(v) for v in re.findall(r"toHandledMs=([\d.]+)", log)]
    result = {"cold_ms": int(total.group(1)), "walk_s": round(walked, 1), **parse_stats(dump),
              "presses": len(to_frame), "press_p50": percentile(to_frame, 0.5), "press_p90": percentile(to_frame, 0.9),
              "handled_p50": percentile(to_handled, 0.5), "handled_p90": percentile(to_handled, 0.9), **parse_framestats(dump),
              "press_samples": to_frame}
    (out / f"{args.label}-{index}.json").write_text(json.dumps(result, indent=1), encoding="utf-8")
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--serial", required=True)
    parser.add_argument("--package", required=True)
    parser.add_argument("--label", required=True)
    parser.add_argument("--glass", help="force this tier (a build without tiers ignores it)")
    parser.add_argument("--extra", action="append", default=[], help='more launcher extras, e.g. "--es ui tv"')
    parser.add_argument("--runs", type=int, default=5)
    parser.add_argument("--presses", type=int, default=40)
    parser.add_argument("--gap", type=int, default=150, help="ms between presses")
    parser.add_argument("--swipes", type=int, default=0, help="vertical swipes after the walk")
    parser.add_argument("--swipe-x", type=int, default=1400)
    parser.add_argument("--settle", type=float, default=4.0, help="seconds after the cold start before the walk")
    parser.add_argument("--out", default="out/speed")
    args = parser.parse_args()

    adb = Adb(args.serial)
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory() as folder:
        script = Path(folder) / "walk.txt"
        script.write_text(monkey_script(args.presses, args.gap), encoding="ascii", newline="\n")
        adb.run("push", str(script), REMOTE)
    extras = ["--es", "host", "native", "--ez", "small", "false", "--ez", "focusLatency", "true"]
    if args.glass:
        extras += ["--es", "glass", args.glass]
    for extra in args.extra:
        extras += extra.split(" ")

    runs = [one_run(adb, args, extras, i, out) for i in range(args.runs)]
    keys = [k for k in runs[0] if k != "press_samples"]
    medians = {k: statistics.median(r[k] for r in runs if r[k] is not None) for k in keys if any(r[k] is not None for r in runs)}
    medians["cold_ms_all"] = [r["cold_ms"] for r in runs]
    medians["p90_all"] = [r["p90"] for r in runs]
    (out / f"{args.label}.json").write_text(json.dumps({"medians": medians, "runs": runs}, indent=1), encoding="utf-8")
    print(args.label, json.dumps(medians))


if __name__ == "__main__":
    main()
