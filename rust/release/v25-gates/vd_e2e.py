"""Isolated host stream on a per-client virtual display (extended layout), run as SYSTEM.

Like rust/release/e2e.py, but the host creates a virtual display beside the
physical monitor (never exclusive) and the moving strip is drawn on that
display once it appears. Must run as SYSTEM in the signed-in session: the
virtual display driver accepts only SYSTEM.

usage: vd_e2e.py TOOLS --package DIR --work DIR --client EXE --codec C --mode WxHxF
                 [--launch-fps F] [--seconds 12] [--bitrate 80000] [--label NAME] [--config K=V]...
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
parser.add_argument('--codec', required=True)
parser.add_argument('--mode', required=True)
parser.add_argument('--launch-fps')
parser.add_argument('--seconds', default='12')
parser.add_argument('--bitrate', default='80000')
parser.add_argument('--label', default='vd')
parser.add_argument('--config', action='append', default=[])
args = parser.parse_args()
# A --config of the same key replaces a default here (virtual_display_layout=exclusive).
_defaults = ['virtual_display_mode=per_client', 'virtual_display_layout=extended',
             'frame_limiter_provider=none', 'output_name=', 'minimum_fps_target=20']
_given = {line.split('=', 1)[0].strip() for line in args.config}
args.config = [line for line in _defaults if line.split('=', 1)[0].strip() not in _given] + args.config


class DISPLAY_DEVICEW(ctypes.Structure):
    _fields_ = [('cb', wintypes.DWORD), ('DeviceName', wintypes.WCHAR * 32), ('DeviceString', wintypes.WCHAR * 128),
                ('StateFlags', wintypes.DWORD), ('DeviceID', wintypes.WCHAR * 128), ('DeviceKey', wintypes.WCHAR * 128)]


def active_displays():
    names, index = set(), 0
    while True:
        device = DISPLAY_DEVICEW(cb=ctypes.sizeof(DISPLAY_DEVICEW))
        if not ctypes.windll.user32.EnumDisplayDevicesW(None, index, ctypes.byref(device), 0):
            return names
        if device.StateFlags & 1:  # DISPLAY_DEVICE_ATTACHED_TO_DESKTOP
            names.add(device.DeviceName)
        index += 1


installed_idle()
plain = requests.Session(); plain.trust_env = False
case = args.work / f'vd-{args.label}'
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
    # The strip goes onto the virtual display as soon as it is on the desktop.
    while not stop.is_set():
        for name in active_displays() - before:
            if name not in probes or probes[name].poll() is not None:
                probes[name] = spawn([str(args.client.parent / 'motion_probe.exe'), name, str(int(args.seconds) + 60),
                                      str(case / f'motion-{len(probes)}.json'), str(int(fps) * 2), '128'],
                                     f'motion-{len(probes)}.log', env=env)
        time.sleep(.1)


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
    spawn([str(args.client.parent / 'audio_probe.exe'), str(int(args.seconds) + 60), sink['id'], '--render-only'], 'tone.log', env=env)
    watcher = threading.Thread(target=watch, daemon=True); watcher.start()
    client_env = receiver_environment(env, dict(display, width=int(width), height=int(height)), args.mode, args.client)
    client_env['BUTTERPOLLO_TEST_HW_DECODER'] = 'd3d11va'
    if args.launch_fps:
        client_env['BUTTERPOLLO_TEST_LAUNCH_FPS'] = args.launch_fps
    (case / 'receiver').mkdir()
    receiver = spawn([sys.executable, str(tools.parent / 'tests' / 'interop.py'), str(case / 'receiver'), args.codec, width, height, fps,
                      args.seconds, args.bitrate, '4'], 'client.log', env=client_env)
    rc = receiver.wait(timeout=int(args.seconds) + 60)
finally:
    stop.set()
    for p, f in reversed(owned):
        if p.poll() is None: p.terminate()
        try: p.wait(timeout=10)
        except subprocess.TimeoutExpired: p.kill(); p.wait()
        f.close()
# The host's teardown removes the display and restores the layout.
deadline = time.monotonic() + 15
while active_displays() != before and time.monotonic() < deadline:
    time.sleep(.2)
host_log = log.read_text(errors='replace') if log.exists() else ''
client = (case / 'client.log').read_text(errors='replace')
config = {k.strip(): v for k, v in (line.split('=', 1) for line in (profile / 'sunshine.conf').read_text().splitlines() if '=' in line)}
import inspect
# Tools from before 2.5 check the configured capture backend; 2.5's always expect the driver.
extra = dict(requested_capture=config.get('capture', 'idd-push').strip()) if 'requested_capture' in inspect.signature(evaluate).parameters else {}
result = evaluate(client, rc, args.codec, args.mode, host_log=host_log, tone_log=(case / 'tone.log').read_text(errors='replace'),
                  host_frames=host_frames(case / 'receiver'), claim_trace=False, **extra)
grep = lambda pattern: [line[28:] for line in host_log.splitlines() if re.search(pattern, line)]
result.update(label=args.label, launch_fps=args.launch_fps, displays_after_match_before=active_displays() == before,
              virtual_display_names=sorted(probes), display_events=grep(r'virtual display mode|asks for another rate|differs from launch|stream display selection|stream configured|virtual display created'))
(case / 'result.json').write_text(json.dumps(result, indent=2))
print(json.dumps({k: result.get(k) for k in ('label', 'passed', 'steady_fps', 'unique_fps', 'interval_p99_ms', 'picture_age_p95_ms', 'displays_after_match_before', 'failures')}))
