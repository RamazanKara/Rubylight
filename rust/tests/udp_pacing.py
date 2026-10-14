"""Compare isolated Windows UDP sender timing over loopback.

Build udp_pacing_probe at the baseline and changed revisions, retaining both
executables. Uses no host service, capture, displays or external receiver.
The 20-second fixture is 50 Mbps of UDP payload at 60 fps, including four 8x
keyframes. Compare automatic startup and its eventual steady rate with the
baseline and a fixed 80 Mbps comparison; the fixture approximates 40 Mbps
of encoded video plus 20% FEC and framing. Automatic mode injects synthetic
standard control pings and a stable 2 ms RTT to exercise the healthy ramp.
Optionally compare automatic-no-feedback to verify silence preserves the
cautious startup rate. Startup and steady timing after 15 seconds are
reported separately. Build both comparison executables with this fixture's
1,200-frame duration to keep their measurement windows identical.
This is a loopback sender timing measurement with synthetic feedback, not
a measurement of network capacity or a physical network or decoder test.
"""

import argparse
import gzip
import hashlib
import json
import os
from pathlib import Path
import socket
import struct
import subprocess
import threading
import time


def run(probe, bps, output, mode="paced"):
    records = []
    errors = []
    done = threading.Event()
    announced = []
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as receiver:
        receiver.bind(("127.0.0.1", 0))
        receiver.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, 4 * 1024 * 1024)
        receiver.settimeout(0.2)

        def receive():
            try:
                while True:
                    try:
                        packet, _ = receiver.recvfrom(65535)
                    except socket.timeout:
                        if done.is_set():
                            break
                        continue
                    at = time.perf_counter_ns()
                    if packet.startswith(b"END "):
                        announced.append(int(packet[4:]))
                    elif len(packet) == 1400 and packet[:4] == b"BPP1":
                        frame, index, count = struct.unpack_from(">IHH", packet, 4)
                        records.append((at, frame, index, count))
                    else:
                        raise ValueError("unexpected probe datagram")
            except Exception as error:
                errors.append(str(error))

        reader = threading.Thread(target=receive)
        reader.start()
        command = [str(probe), f"127.0.0.1:{receiver.getsockname()[1]}", mode, str(bps), "0"]
        try:
            result = subprocess.run(
                command, capture_output=True, text=True, timeout=45,
                creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0,
            )
        finally:
            done.set()
            reader.join()
    output.with_suffix(".stdout.txt").write_text(result.stdout, encoding="utf-8")
    output.with_suffix(".stderr.txt").write_text(result.stderr, encoding="utf-8")
    result.check_returncode()
    if errors:
        raise RuntimeError(errors)
    sender = json.loads(result.stdout)
    frames = {}
    for at, frame, index, count in records:
        state = frames.setdefault(frame, [set(), count, at, at])
        if state[1] != count or index >= count:
            raise ValueError("inconsistent probe frame header")
        state[0].add(index)
        state[3] = at
    unique = sum(len(state[0]) for state in frames.values())
    complete = sum(len(state[0]) == state[1] for state in frames.values())
    report = {
        "measurement": "loopback_sender_timing_not_network_capacity",
        "command": command,
        "probe_sha256": hashlib.sha256(probe.read_bytes()).hexdigest(),
        "sender": sender,
        "loopback": {
            "received": len(records), "unique_packets": unique,
            "missing_packets": sender["packets"] - unique,
            "complete_frames": complete,
        },
    }
    output.with_suffix(".json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    with gzip.open(output.with_suffix(".packets.json.gz"), "wt", encoding="utf-8") as raw:
        json.dump({"columns": ["arrival_ns", "frame", "index", "count"], "packets": records}, raw)
    if announced != [sender["packets"]] or unique != sender["packets"] or complete != 1200:
        raise RuntimeError(f"incomplete loopback delivery: {report['loopback']}")
    if mode in ("automatic", "automatic-no-feedback"):
        expected_initial = min(sender["ceiling_bps"], 80_000_000)
        if sender["initial_bps"] != expected_initial:
            raise RuntimeError(f"unexpected automatic startup rate: {sender['initial_bps']}")
        for window in ("startup_first_second", "startup_first_two_seconds",
                       "steady_after_fifteen_seconds"):
            if not sender.get(window) or sender[window]["frames"] <= 0:
                raise RuntimeError(f"missing sender timing window: {window}")
        if mode == "automatic":
            if sender["feedback"] != "synthetic_standard_ping_and_stable_2ms_rtt":
                raise RuntimeError("automatic timing needs explicitly labeled synthetic feedback")
            if sender["final_bps"] != sender["ceiling_bps"] or not sender["first_ceiling"]:
                raise RuntimeError("synthetic healthy feedback did not complete the startup ramp")
        elif sender["feedback"] != "none" or sender["final_bps"] != expected_initial:
            raise RuntimeError("automatic pacing changed rate without feedback")
    print(json.dumps({"case": output.name, **sender, **report["loopback"]}), flush=True)
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--before", type=Path, required=True)
    parser.add_argument("--after", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--runs", type=int, default=3)
    parser.add_argument("--after-mode", choices=["paced", "automatic", "automatic-no-feedback"],
                        default="automatic")
    parser.add_argument("--after-rate", type=int, default=800)
    parser.add_argument("--comparison-rate", type=int, default=80)
    parser.add_argument("--include-no-feedback", action="store_true",
                        help="also measure automatic startup without synthetic control feedback")
    args = parser.parse_args()
    if not 1 <= args.runs <= 10:
        parser.error("runs must be between 1 and 10")
    if not all(1 <= rate <= 1000 for rate in [args.after_rate, args.comparison_rate]):
        parser.error("rates must be between 1 and 1000 Mbps")
    args.output.mkdir(parents=True, exist_ok=True)
    results = []
    for repeat in range(1, args.runs + 1):
        cases = [("before", args.before, 800, "paced"),
                 ("after", args.after, args.after_rate, args.after_mode),
                 ("comparison", args.after, args.comparison_rate, "paced")]
        if args.include_no_feedback and args.after_mode != "automatic-no-feedback":
            cases.append(("no-feedback", args.after, args.after_rate, "automatic-no-feedback"))
        if repeat % 2 == 0:
            cases.reverse()
        for label, probe, rate, mode in cases:
            results.append(run(probe.resolve(), rate, args.output / f"{label}-{repeat}", mode))
    (args.output / "results.json").write_text(json.dumps(results, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
