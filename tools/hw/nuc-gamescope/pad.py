"""pad.py: a uinput Xbox 360 pad (045e:028e) for the gamescope launcher run. A at 12 s (open the
paired host), A at 20 s (start its focused app), Start+Back at 85 s (stream menu), B at 90 s, gone at
95 s. Prints each press with its time."""
import glob, os, time
from evdev import UInput, ecodes as e, AbsInfo

caps = {
    e.EV_KEY: [e.BTN_SOUTH, e.BTN_EAST, e.BTN_NORTH, e.BTN_WEST, e.BTN_TL, e.BTN_TR, e.BTN_SELECT, e.BTN_START,
               e.BTN_MODE, e.BTN_THUMBL, e.BTN_THUMBR],
    e.EV_ABS: [(a, AbsInfo(0, -32768, 32767, 16, 128, 0)) for a in (e.ABS_X, e.ABS_Y, e.ABS_RX, e.ABS_RY)]
              + [(a, AbsInfo(0, 0, 255, 0, 0, 0)) for a in (e.ABS_Z, e.ABS_RZ)]
              + [(a, AbsInfo(0, -1, 1, 0, 0, 0)) for a in (e.ABS_HAT0X, e.ABS_HAT0Y)],
}
ui = UInput(caps, name='Microsoft X-Box 360 pad', vendor=0x045e, product=0x028e, version=0x110, bustype=e.BUS_USB)
time.sleep(1)
for n in glob.glob('/sys/devices/virtual/input/input*/event*'):
    if open(os.path.join(os.path.dirname(n), 'name')).read().strip() == 'Microsoft X-Box 360 pad':
        os.chmod('/dev/input/' + os.path.basename(n), 0o666); print('pad', os.path.basename(n), flush=True)
start = time.monotonic()
def at(t):
    time.sleep(max(0, start + t - time.monotonic()))
def btn(codes, name, hold=0.15):
    for c in codes: ui.write(e.EV_KEY, c, 1)
    ui.syn(); time.sleep(hold)
    for c in codes: ui.write(e.EV_KEY, c, 0)
    ui.syn(); print(name, round(time.monotonic() - start, 1), flush=True)
at(12); btn([e.BTN_SOUTH], 'A')
at(20); btn([e.BTN_SOUTH], 'A')
at(85); btn([e.BTN_START, e.BTN_SELECT], 'Start+Back', 0.3)
at(90); btn([e.BTN_EAST], 'B')
at(95); ui.close(); print('gone', flush=True)
