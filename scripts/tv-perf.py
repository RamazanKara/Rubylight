#!/usr/bin/env python3
"""Frame-time walk of the Nightjar TV menus over adb, for the console redesign's perf gate.

Launches a debug demo state, resets gfxinfo, walks the grid (ten single D-pad moves 300 ms apart, then
bursts of thirty key events in one `input keyevent` call, which emulates a held key), reads the frame
statistics and writes result.json plus screenshots.

  python scripts/tv-perf.py --serial emulator-5554 --label main --state library --out out/perf/main
  python scripts/tv-perf.py --serial e24bb88e --package com.butterpollo.client.debug --label branch --state home --host native
  python scripts/tv-perf.py --serial <phone> --label tv-on-phone --state home --host native --ui tv   # TV layouts on a handheld

Pass criteria (spec section 4): janky % no more than the baseline plus 2 points; 90th percentile at most 16 ms
when the AVD runs with -gpu host (or on real hardware); no red (4x) overdraw over the grid, which the
--overdraw shot shows for a person to judge. The nj-tv AVD renders with swiftshader, so its absolute numbers
are pessimistic: compare a branch with main, both measured the same way.
"""
import argparse
import json
import re
import subprocess
import sys
import time
from pathlib import Path

LAUNCHER = "com.limelight.demo.DemoLauncherActivity"
RIGHT, DOWN, LEFT, UP, SELECT = 22, 20, 21, 19, 23
# One burst: thirty key events back to back, the way a held key repeats.
BURST = [RIGHT] * 10 + [DOWN] * 5 + [LEFT] * 10 + [UP] * 5

STATS = {
    "total_frames": r"Total frames rendered:\s*(\d+)",
    "janky_frames": r"Janky frames:\s*(\d+)",
    "janky_percent": r"Janky frames:\s*\d+\s*\(([\d.]+)%\)",
    "p50_ms": r"50th percentile:\s*(\d+)ms",
    "p90_ms": r"90th percentile:\s*(\d+)ms",
    "p95_ms": r"95th percentile:\s*(\d+)ms",
    "p99_ms": r"99th percentile:\s*(\d+)ms",
    "missed_vsync": r"Number Missed Vsync:\s*(\d+)",
    "slow_ui_thread": r"Number Slow UI thread:\s*(\d+)",
    "slow_bitmap_uploads": r"Number Slow bitmap uploads:\s*(\d+)",
    "slow_issue_draw": r"Number Slow issue draw commands:\s*(\d+)",
    "deadline_missed": r"Number Frame deadline missed:\s*(\d+)",
}


class Adb:
    def __init__(self, serial):
        self.base = ["adb"] + (["-s", serial] if serial else [])

    def shell(self, *args, check=True):
        result = subprocess.run(self.base + ["shell", *args], capture_output=True, text=True, encoding="utf-8",
                                errors="replace")
        if check and result.returncode != 0:
            raise RuntimeError(f"adb shell {' '.join(args)} failed: {result.stderr.strip()}")
        return result.stdout

    def screencap(self, path):
        data = subprocess.run(self.base + ["exec-out", "screencap", "-p"], capture_output=True, check=True).stdout
        Path(path).write_bytes(data)


def launch(adb, package, state, extras):
    adb.shell("am", "force-stop", package)
    command = ["am", "start", "-W", "-n", f"{package}/{LAUNCHER}", "--es", "state", state]
    for extra in extras:
        command += extra.split(" ")
    adb.shell(*command)


def parse(dump):
    result = {}
    for name, pattern in STATS.items():
        match = re.search(pattern, dump)
        result[name] = (float(match.group(1)) if "." in match.group(1) else int(match.group(1))) if match else None
    return result


def walk(adb, singles, bursts, gap):
    for i in range(singles):
        adb.shell("input", "keyevent", str(RIGHT if i % 3 != 2 else DOWN))
        time.sleep(gap)
    for _ in range(bursts):
        adb.shell("input", "keyevent", *[str(k) for k in BURST])
        time.sleep(0.6)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--serial", help="adb serial (default: the only device)")
    parser.add_argument("--package", default="com.butterpollo.client.debug")
    parser.add_argument("--state", default="library", help="demo state: library, home, settings, ...")
    parser.add_argument("--host", help="demo host (sunshine or native; native for Home)")
    parser.add_argument("--ui", choices=["tv", "handheld"], help="force the TV or handheld interface layout (tv on a handheld)")
    parser.add_argument("--small", choices=["true", "false"], default="false", help="small tiles")
    parser.add_argument("--extra", action="append", default=[], help='more launcher extras, e.g. "--ez roles true"')
    parser.add_argument("--label", default="run")
    parser.add_argument("--out", default="out/perf")
    parser.add_argument("--singles", type=int, default=10)
    parser.add_argument("--bursts", type=int, default=3)
    parser.add_argument("--gap", type=float, default=0.3, help="seconds between single moves")
    parser.add_argument("--settle", type=float, default=3.0, help="seconds to let the screen load")
    parser.add_argument("--overdraw", action="store_true", help="also take an overdraw screenshot")
    args = parser.parse_args()

    adb = Adb(args.serial)
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    prefix = f"{args.label}-{args.state}-{'small' if args.small == 'true' else 'large'}"
    extras = ["--ez small " + args.small] + ([f"--es host {args.host}"] if args.host else []) + \
        ([f"--es ui {args.ui}"] if args.ui else []) + args.extra

    adb.shell("setprop", "debug.hwui.overdraw", "false", check=False)
    launch(adb, args.package, args.state, extras)
    time.sleep(args.settle)
    adb.screencap(out / f"{prefix}-start.png")
    adb.shell("dumpsys", "gfxinfo", args.package, "reset")
    walk(adb, args.singles, args.bursts, args.gap)
    time.sleep(0.5)
    adb.screencap(out / f"{prefix}-end.png")
    dump = adb.shell("dumpsys", "gfxinfo", args.package)
    stats = parse(dump)
    (out / f"{prefix}-gfxinfo.txt").write_text(dump, encoding="utf-8")

    # One select: the launch must start a game; it is measured apart from the walk above.
    adb.shell("input", "keyevent", str(SELECT))
    time.sleep(2.0)
    top = adb.shell("dumpsys", "activity", "activities", check=False)
    launched = bool(re.search(r"(?:topResumedActivity|mResumedActivity).*Game", top))
    adb.screencap(out / f"{prefix}-select.png")

    if args.overdraw:
        # hwui reads the property when a window's renderer is created, so the screen is restarted.
        adb.shell("setprop", "debug.hwui.overdraw", "show")
        launch(adb, args.package, args.state, extras)
        time.sleep(args.settle)
        adb.screencap(out / f"{prefix}-overdraw.png")
        adb.shell("setprop", "debug.hwui.overdraw", "false")

    adb.shell("am", "force-stop", args.package, check=False)
    result = {"label": args.label, "state": args.state, "package": args.package, "serial": args.serial,
              "small": args.small == "true", "singles": args.singles, "bursts": args.bursts,
              "burst_keys": len(BURST), "select_launched_game": launched, **stats}
    (out / f"{prefix}-result.json").write_text(json.dumps(result, indent=2), encoding="utf-8")
    print(json.dumps(result, indent=2))
    return 0 if stats["total_frames"] else 1


if __name__ == "__main__":
    sys.exit(main())
