"""Task B gate 5: two devices on one virtual display, each with its own settings.

One isolated host (virtual_display_mode = shared, extended layout) streams the Desktop to two
independently paired receivers at once for SECONDS: one SDR HEVC, one HDR AV1, same mode. The
moving strip is drawn on the shared virtual display. Pass only if both receivers decoded
pictures for the whole stream (each passes the e2e receiver checks), the host opened one
capture for the display (one "capture backend opened" line) and both streams ran to the end.

usage: two_clients.py TOOLS --package DIR --work DIR --client EXE [--mode 1920x1080x60] [--seconds 300]
Run elevated (the display driver admits administrators and SYSTEM), in the signed-in session.
"""
import argparse, ctypes, json, pathlib, re, subprocess, sys, threading, time
from ctypes import wintypes

tools = pathlib.Path(sys.argv.pop(1)).resolve()
sys.path.insert(0, str(tools))
import requests
from e2e_host import installed_idle, prepare, receiver_environment
from e2e_result import evaluate, host_frames

parser = argparse.ArgumentParser()
parser.add_argument('--package', required=True, type=pathlib.Path)
parser.add_argument('--work', required=True, type=pathlib.Path)
parser.add_argument('--client', required=True, type=pathlib.Path)
parser.add_argument('--mode', default='1920x1080x60')
parser.add_argument('--seconds', default='300')
parser.add_argument('--bitrate', default='40000')
parser.add_argument('--label', default='two-clients')
parser.add_argument('--config', action='append', default=[])
args = parser.parse_args()
args.codec = 'hevc'
args.config = ['virtual_display_mode=shared', 'virtual_display_layout=extended', 'frame_limiter_provider=none',
               'minimum_fps_target=20'] + args.config
CLIENTS = [('sdr-hevc', 'hevc'), ('hdr-av1', 'av1-hdr')]


class DISPLAY_DEVICEW(ctypes.Structure):
    _fields_ = [('cb', wintypes.DWORD), ('DeviceName', wintypes.WCHAR * 32), ('DeviceString', wintypes.WCHAR * 128),
                ('StateFlags', wintypes.DWORD), ('DeviceID', wintypes.WCHAR * 128), ('DeviceKey', wintypes.WCHAR * 128)]


def active_displays():
    names, index = set(), 0
    while True:
        device = DISPLAY_DEVICEW(cb=ctypes.sizeof(DISPLAY_DEVICEW))
        if not ctypes.windll.user32.EnumDisplayDevicesW(None, index, ctypes.byref(device), 0):
            return names
        if device.StateFlags & 1:
            names.add(device.DeviceName)
        index += 1


installed_idle()
plain = requests.Session(); plain.trust_env = False
case = args.work / args.label
assert case.resolve().parent == args.work.resolve()
if case.exists():
    import shutil; shutil.rmtree(case)
env, display, sink = prepare(args, case)
profile = case / 'config'
log = profile / 'logs' / 'butterpollo.log'
width, height, fps = args.mode.split('x')
before = active_displays()
owned, probes, stop = [], {}, threading.Event()


def spawn(command, name, **kw):
    f = (case / name).open('w')
    p = subprocess.Popen(command, stdout=f, stderr=subprocess.STDOUT, creationflags=subprocess.CREATE_NO_WINDOW, **kw)
    owned.append((p, f)); return p


def watch():
    while not stop.is_set():
        for name in active_displays() - before:
            if name not in probes or probes[name].poll() is not None:
                probes[name] = spawn([str(args.client.parent / 'motion_probe.exe'), name, str(int(args.seconds) + 120),
                                      str(case / f'motion-{len(probes)}.json'), str(int(fps) * 2), '128'],
                                     f'motion-{len(probes)}.log', env=env)
        time.sleep(.1)


receivers = {}
try:
    host = spawn([str(args.package / 'butterpollo.exe'), '--config-dir', str(profile), '--assets', str(args.package / 'assets' / 'web'),
                  '--bind', '127.0.0.1', '--no-tray'], 'host.stdout.log', cwd=case, env=env)
    deadline = time.monotonic() + 60
    while True:
        assert host.poll() is None, 'host exited'
        if log.exists() and 'encoder capability probe completed' in log.read_text(errors='replace'):
            try:
                if plain.get('http://127.0.0.1:48523/serverinfo', timeout=1).ok: break
            except requests.RequestException: pass
        assert time.monotonic() < deadline, 'host not ready'
        time.sleep(.2)
    spawn([str(args.client.parent / 'audio_probe.exe'), str(int(args.seconds) + 120), sink['id'], '--render-only'], 'tone.log', env=env)
    threading.Thread(target=watch, daemon=True).start()
    for index, (name, codec) in enumerate(CLIENTS):
        client_env = receiver_environment(env, dict(display, width=int(width), height=int(height)), args.mode, args.client)
        client_env['BUTTERPOLLO_TEST_HW_DECODER'] = 'd3d11va'
        client_env['BUTTERPOLLO_TEST_TIMING_CSV'] = str(case / f'{name}-frames.csv')
        (case / name).mkdir()
        receivers[name] = (codec, spawn([sys.executable, str(tools.parent / 'tests' / 'interop.py'), str(case / name), codec,
                                         width, height, fps, args.seconds, args.bitrate, '4'], f'{name}.log', env=client_env))
        if index == 0:
            # The second device joins a display that is already streaming.
            time.sleep(10)
    deadline = time.monotonic() + int(args.seconds) + 120
    while any(p.poll() is None for _, p in receivers.values()):
        assert time.monotonic() < deadline, 'receivers did not finish'
        time.sleep(1)
finally:
    stop.set()
    for p, f in reversed(owned):
        if p.poll() is None: p.terminate()
        try: p.wait(timeout=10)
        except subprocess.TimeoutExpired: p.kill(); p.wait()
        f.close()
deadline = time.monotonic() + 15
while active_displays() != before and time.monotonic() < deadline:
    time.sleep(.2)
host_log = log.read_text(errors='replace') if log.exists() else ''
results, failures = {}, []
for name, (codec, process) in receivers.items():
    r = evaluate((case / f'{name}.log').read_text(errors='replace'), process.returncode, codec, args.mode, host_log=host_log,
                 host_frames=host_frames(case / name), claim_trace=False)
    results[name] = {k: r.get(k) for k in ('passed', 'frames', 'steady_fps', 'unique_fps', 'picture_age_mean_ms', 'picture_age_p95_ms', 'failures')}
    if not r.get('passed'):
        failures.append(f"{name}: {'; '.join(r.get('failures') or ['failed'])}")
captures = len(re.findall(r'capture backend opened', host_log))
if captures != 1:
    failures.append(f'the host opened {captures} captures for the shared display, not one')
if len(probes) != 1:
    failures.append(f'{len(probes)} virtual displays appeared, not one shared display')
if active_displays() != before:
    failures.append('the display layout was not restored after the streams')
result = dict(label=args.label, mode=args.mode, seconds=int(args.seconds), passed=not failures, failures=failures,
              clients=results, captures_opened=captures, virtual_displays=sorted(probes))
(case / 'result.json').write_text(json.dumps(result, indent=2))
print(json.dumps(result))
sys.exit(0 if result['passed'] else 1)
