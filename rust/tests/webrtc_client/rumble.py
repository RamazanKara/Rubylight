"""Drives the rumble leg of the WebRTC harness through XInput.

The host plugs a virtual Xbox pad per browser pad. This script never touches a
pad that was there before the stream: `baseline` lists the connected XInput
slots, and `rumble` only writes vibration to slots that appeared since.

  python -I rumble.py baseline            -> prints the connected slots as JSON
  python -I rumble.py rumble BASELINE_JSON [SECONDS]
"""
import ctypes
import json
import sys
import time

xinput = ctypes.WinDLL('xinput1_4')


class Vibration(ctypes.Structure):
    _fields_ = [('wLeftMotorSpeed', ctypes.c_ushort), ('wRightMotorSpeed', ctypes.c_ushort)]


class State(ctypes.Structure):
    _fields_ = [('packet', ctypes.c_uint), ('buttons', ctypes.c_ushort), ('lt', ctypes.c_ubyte), ('rt', ctypes.c_ubyte),
                ('lx', ctypes.c_short), ('ly', ctypes.c_short), ('rx', ctypes.c_short), ('ry', ctypes.c_short)]


def connected():
    out = []
    for slot in range(4):
        if xinput.XInputGetState(slot, ctypes.byref(State())) == 0:
            out.append(slot)
    return out


def main():
    if sys.argv[1] == 'baseline':
        print(json.dumps(connected()))
        return 0
    baseline = set(json.loads(sys.argv[2]))
    seconds = float(sys.argv[3]) if len(sys.argv) > 3 else 1.0
    deadline = time.monotonic() + 15
    new = []
    while time.monotonic() < deadline:
        new = [slot for slot in connected() if slot not in baseline]
        if new:
            break
        time.sleep(.1)
    if not new:
        print(json.dumps({'error': 'no new XInput pad appeared'}))
        return 2
    slot = new[0]
    state = State()
    xinput.XInputGetState(slot, ctypes.byref(state))
    snapshot = {'slot': slot, 'packet': state.packet, 'buttons': state.buttons, 'lx': state.lx}
    xinput.XInputSetState(slot, ctypes.byref(Vibration(0xC000, 0x4000)))
    time.sleep(seconds)
    xinput.XInputSetState(slot, ctypes.byref(Vibration(0, 0)))
    print(json.dumps({'slot': slot, 'state': snapshot}))
    return 0


sys.exit(main())
