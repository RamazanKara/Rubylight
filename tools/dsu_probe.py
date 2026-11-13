#!/usr/bin/env python3
"""A DSU (cemuhook) test client for Rubylight's motion server (enable_dsu_server).

Asks the server for its protocol version and its four ports, subscribes to
every slot's pad data, and for --seconds reports what arrives: packets per
second per slot, the latest buttons, accelerometer (g) and gyroscope (deg/s),
and whether the motion changed. It verifies every reply's magic, version,
length and CRC-32.

    python tools/dsu_probe.py --host 127.0.0.1 --port 26760 --seconds 30

Exit code 0 when at least one slot sent pad data whose motion changed and
every packet was valid, 1 otherwise. Writes a JSON summary with --json.
"""
import argparse
import json
import random
import socket
import struct
import sys
import time
import zlib

VERSION = 1001
MSG_VERSION, MSG_PORTS, MSG_PAD_DATA = 0x100000, 0x100001, 0x100002


def request(client_id: int, message: int, payload: bytes = b"") -> bytes:
    body = struct.pack("<I", message) + payload
    packet = bytearray(b"DSUC" + struct.pack("<HHII", VERSION, len(body), 0, client_id) + body)
    packet[8:12] = struct.pack("<I", zlib.crc32(bytes(packet)) & 0xFFFFFFFF)
    return bytes(packet)


def check(packet: bytes):
    """The message type and body of a valid server packet, else None."""
    if len(packet) < 20 or packet[:4] != b"DSUS":
        return None
    version, length = struct.unpack_from("<HH", packet, 4)
    if version != VERSION or 16 + length != len(packet):
        return None
    crc = struct.unpack_from("<I", packet, 8)[0]
    zeroed = packet[:8] + b"\0\0\0\0" + packet[12:]
    if crc != zlib.crc32(zeroed) & 0xFFFFFFFF:
        return None
    return struct.unpack_from("<I", packet, 16)[0], packet[20:]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=26760)
    parser.add_argument("--seconds", type=float, default=30.0)
    parser.add_argument("--json", help="write a summary to this file")
    args = parser.parse_args()

    client_id = random.getrandbits(32)
    server = (args.host, args.port)
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    sock.settimeout(0.5)
    sock.sendto(request(client_id, MSG_VERSION), server)
    sock.sendto(request(client_id, MSG_PORTS, struct.pack("<i", 4) + bytes([0, 1, 2, 3])), server)

    invalid = 0
    version = None
    ports = {}
    slots = {}
    start = time.monotonic()
    last_subscribe = 0.0
    while time.monotonic() - start < args.seconds:
        now = time.monotonic()
        # Clients renew their subscription well inside the server's 5 s timeout.
        if now - last_subscribe > 1.0:
            sock.sendto(request(client_id, MSG_PAD_DATA, bytes([0, 0]) + bytes(6)), server)
            last_subscribe = now
        try:
            packet, _ = sock.recvfrom(2048)
        except socket.timeout:
            continue
        except ConnectionResetError:
            print("no server on that port yet", file=sys.stderr)
            time.sleep(0.5)
            continue
        checked = check(packet)
        if checked is None:
            invalid += 1
            continue
        message, body = checked
        if message == MSG_VERSION:
            version = struct.unpack_from("<H", body)[0]
        elif message == MSG_PORTS:
            slot, state, model, connection = body[:4]
            ports[slot] = {"state": state, "model": model, "connection": connection, "battery": body[10]}
        elif message == MSG_PAD_DATA and len(body) == 80:
            slot = body[0]
            number = struct.unpack_from("<I", body, 12)[0]
            buttons = body[16:18]
            # After the 11-byte slot info: connected, number, buttons, sticks,
            # analog buttons, two touches, then the motion timestamp and motion.
            timestamp = struct.unpack_from("<Q", body, 48)[0]
            accel = struct.unpack_from("<3f", body, 56)
            gyro = struct.unpack_from("<3f", body, 68)
            s = slots.setdefault(slot, {"packets": 0, "first": now, "motion_changes": 0,
                                        "last_motion": None, "out_of_order": 0, "last_number": None})
            s["packets"] += 1
            if s["last_number"] is not None and number <= s["last_number"]:
                s["out_of_order"] += 1
            s["last_number"] = number
            motion = (timestamp, accel, gyro)
            if s["last_motion"] is not None and motion != s["last_motion"]:
                s["motion_changes"] += 1
            s["last_motion"] = motion
            s.update(buttons=f"{buttons[0]:02x} {buttons[1]:02x}", accel=[round(v, 3) for v in accel],
                     gyro=[round(v, 1) for v in gyro], battery=body[10], last=now)
        else:
            invalid += 1

    elapsed = time.monotonic() - start
    summary = {"version": version, "ports": ports, "invalid_packets": invalid, "slots": {}}
    for slot, s in sorted(slots.items()):
        rate = s["packets"] / max(s["last"] - s["first"], 1e-3)
        summary["slots"][slot] = {k: s[k] for k in ("packets", "motion_changes", "out_of_order",
                                                    "buttons", "accel", "gyro", "battery")}
        summary["slots"][slot]["packets_per_second"] = round(rate, 1)
        print(f"slot {slot}: {s['packets']} packets ({rate:.0f}/s), motion changed {s['motion_changes']} times, "
              f"out of order {s['out_of_order']}, buttons {s['buttons']}, accel {s['accel']} g, "
              f"gyro {s['gyro']} deg/s, battery 0x{s['battery']:02x}")
    print(f"version {version}, ports {ports}, invalid packets {invalid}, {elapsed:.0f} s")
    if args.json:
        with open(args.json, "w", encoding="utf-8") as out:
            json.dump(summary, out, indent=2)
    passed = invalid == 0 and version == VERSION and any(s["motion_changes"] > 0 for s in slots.values())
    return 0 if passed else 1


if __name__ == "__main__":
    sys.exit(main())
