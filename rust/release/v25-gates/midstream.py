"""Mid-stream changes on a per-client virtual display streamed with idd-push, run as SYSTEM.

Like vd_e2e.py, with events while the stream runs, then a client reconnect:
  * at --hdr-at seconds the virtual display's HDR state is toggled (display_poke),
  * at --mode-at seconds its resolution changes to --mode-to (refresh kept),
  * when the first client run ends the client disconnects without quitting, resumes the
    session (/resume, as Moonlight's reconnect does) and streams --resume-seconds more.

The first run only has to keep frames coming across the changes (the receiver's picture
checks assume a fixed desktop), so it records the longest gap after each event; the resumed
run must pass the receiver's own checks (all frames decoded, no failures).

usage: midstream.py TOOLS --package DIR --work DIR --client EXE --poke EXE --codec C
                    --mode WxHxF --label NAME [--seconds 30] [--resume-seconds 12]
                    [--hdr-at 8] [--mode-at 16] [--mode-to 1920x1080] [--config K=V]...
"""
import argparse, csv, ctypes, json, os, pathlib, re, runpy, subprocess, sys, threading, time, types
from ctypes import wintypes

tools = pathlib.Path(sys.argv.pop(1)).resolve()
sys.path.insert(0, str(tools))
import requests
from e2e_host import installed_idle, prepare, receiver_environment

parser = argparse.ArgumentParser()
parser.add_argument('--package', required=True, type=pathlib.Path)
parser.add_argument('--work', required=True, type=pathlib.Path)
parser.add_argument('--client', required=True, type=pathlib.Path)
parser.add_argument('--poke', required=True, type=pathlib.Path)
parser.add_argument('--codec', required=True)
parser.add_argument('--mode', required=True)
parser.add_argument('--label', required=True)
parser.add_argument('--seconds', default='30')
parser.add_argument('--resume-seconds', default='12')
parser.add_argument('--bitrate', default='80000')
parser.add_argument('--hdr-at', type=float, default=8)
parser.add_argument('--mode-at', type=float, default=16)
parser.add_argument('--mode-to', default='1920x1080')
parser.add_argument('--config', action='append', default=[])
args = parser.parse_args()
args.launch_fps = None
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
        if device.StateFlags & 1:
            names.add(device.DeviceName)
        index += 1


installed_idle()
plain = requests.Session(); plain.trust_env = False
case = args.work / f'mid-{args.label}'
if case.exists():
    import shutil; shutil.rmtree(case)
env, display, sink = prepare(args, case)
profile = case / 'config'
log = profile / 'logs' / 'butterpollo.log'
width, height, fps = args.mode.split('x')
before = active_displays()
owned, probes, events, stop = [], {}, [], threading.Event()
streaming = threading.Event()
hdr_stream = '-hdr' in args.codec


def spawn(command, name, **kw):
    f = (case / name).open('w')
    p = subprocess.Popen(command, stdout=f, stderr=subprocess.STDOUT, creationflags=subprocess.CREATE_NO_WINDOW, **kw)
    owned.append((p, f)); return p


def poke(*what):
    result = subprocess.run([str(args.poke), *what], capture_output=True, text=True, timeout=30)
    events.append(dict(at=time.monotonic(), what=' '.join(what[1:]), rc=result.returncode,
                       output=(result.stdout + result.stderr).strip()))


def watch():
    while not stop.is_set():
        for name in active_displays() - before:
            if name not in probes or probes[name].poll() is not None:
                probes[name] = spawn([str(args.client.parent / 'motion_probe.exe'), name, str(int(args.seconds) + 90),
                                      str(case / f'motion-{len(probes)}.json'), str(int(fps) * 2), '128'],
                                     f'motion-{len(probes)}.log', env=env)
        time.sleep(.1)


def changes():
    # The clock starts when the client is streaming (the first LAUNCH line).
    streaming.wait()
    start = time.monotonic()
    plan = sorted([(args.hdr_at, 'hdr'), (args.mode_at, 'mode')])
    for at, kind in plan:
        while time.monotonic() - start < at:
            if stop.is_set(): return
            time.sleep(.05)
        names = sorted(active_displays() - before)
        if not names:
            events.append(dict(at=time.monotonic(), what=f'{kind}: no virtual display', rc=-1, output=''))
            continue
        if kind == 'hdr':
            poke(names[0], 'hdr', 'off' if hdr_stream else 'on')
        else:
            poke(names[0], 'mode', args.mode_to)


# interop.py's RTSS hook is the one place it reconnects before quitting: stand in for it.
reconnect = types.ModuleType('rtss_lifecycle')


class Lifecycle:
    def __init__(self, profile_name, session, http, web, artifact):
        self.artifact = artifact

    def launched(self):
        streaming.set()

    def reconnect_and_expire(self, client, https, launch_args, command, env):
        import xml.etree.ElementTree as ET
        response = client.get(https + '/resume', params=launch_args, timeout=15)
        response.raise_for_status()
        root = ET.fromstring(response.text)
        assert root.attrib['status_code'] == '200', response.text
        events.append(dict(at=time.monotonic(), what='resume', rc=0, output=''))
        resumed_env = env.copy()
        resumed_env['BUTTERPOLLO_TEST_TIMING_CSV'] = str(self.artifact / 'resumed-frames.csv')
        for key in ('BUTTERPOLLO_TEST_REQUIRE_MOTION', 'BUTTERPOLLO_TEST_BARCODE_BOTTOM', 'BUTTERPOLLO_TEST_MIN_FPS'):
            resumed_env[key] = original[key]
        argv = list(command)
        argv[0:2] = [command[0], root.findtext('sessionUrl0')]
        argv[6] = args.resume_seconds  # seconds
        resumed = subprocess.run(argv, env=resumed_env, capture_output=True, text=True, timeout=int(args.resume_seconds) + 60)
        (self.artifact / 'resumed-client.log').write_text(resumed.stdout + resumed.stderr, encoding='utf-8')
        assert resumed.returncode == 0, resumed.stdout[-2000:]


reconnect.Lifecycle = Lifecycle
original = {}
rc, error = None, None
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
    spawn([str(args.client.parent / 'audio_probe.exe'), str(int(args.seconds) + int(args.resume_seconds) + 90), sink['id'], '--render-only'], 'tone.log', env=env)
    threading.Thread(target=watch, daemon=True).start()
    threading.Thread(target=changes, daemon=True).start()
    client_env = receiver_environment(env, dict(display, width=int(width), height=int(height)), args.mode, args.client)
    client_env['BUTTERPOLLO_TEST_HW_DECODER'] = 'd3d11va'
    client_env['BUTTERPOLLO_TEST_RTSS_PROFILE'] = 'midstream'
    client_env['BUTTERPOLLO_TEST_TIMING_CSV'] = str(case / 'frames.csv')
    original = {k: client_env[k] for k in ('BUTTERPOLLO_TEST_REQUIRE_MOTION', 'BUTTERPOLLO_TEST_BARCODE_BOTTOM', 'BUTTERPOLLO_TEST_MIN_FPS')}
    # The first run crosses a desktop that changes shape: frames must keep coming, not match
    # the barcode geometry or the rate.
    client_env.update(BUTTERPOLLO_TEST_REQUIRE_MOTION='0', BUTTERPOLLO_TEST_BARCODE_BOTTOM='0', BUTTERPOLLO_TEST_MIN_FPS='0')
    (case / 'receiver').mkdir()
    os.environ.update(client_env)
    sys.modules['rtss_lifecycle'] = reconnect
    interop = tools.parent / 'tests' / 'interop.py'
    sys.argv = [str(interop), str(case / 'receiver'), args.codec, width, height, fps, args.seconds, args.bitrate, '4']
    out = (case / 'client.log').open('w')
    saved = sys.stdout
    try:
        sys.stdout = out
        runpy.run_path(str(interop), run_name='__main__')
        rc = 0
    except BaseException as e:  # noqa: BLE001 - recorded in the result
        rc, error = 1, f'{type(e).__name__}: {e}'
    finally:
        sys.stdout = saved
        out.close()
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


def gaps(path, marks):
    """The longest arrival gap after each mark (a monotonic time), within 5 s of it, in ms."""
    try:
        rows = list(csv.DictReader(open(path, newline='')))
    except FileNotFoundError:
        return None
    arrivals = [float(r['arrival_ms']) for r in rows]
    if len(arrivals) < 2:
        return dict(frames=len(arrivals))
    longest = max(b - a for a, b in zip(arrivals, arrivals[1:]))
    return dict(frames=len(arrivals), longest_gap_ms=round(longest, 1),
                gaps_over_100ms=sum(1 for a, b in zip(arrivals, arrivals[1:]) if b - a > 100),
                span_s=round((arrivals[-1] - arrivals[0]) / 1000, 1))


host_log = log.read_text(errors='replace') if log.exists() else ''
client_log = (case / 'client.log').read_text(errors='replace')
resumed_log = (case / 'receiver' / 'resumed-client.log').read_text(errors='replace') if (case / 'receiver' / 'resumed-client.log').exists() else ''
grep = lambda pattern: [line[28:] for line in host_log.splitlines() if re.search(pattern, line)]
decoded = re.search(r'RESULT frames=(\d+) decoded_frames=(\d+).* failures=(\d+)', resumed_log)
first = re.search(r'RESULT frames=(\d+) decoded_frames=(\d+).* failures=(\d+)', client_log)
result = dict(
    label=args.label, rc=rc, error=error,
    events=[dict(e, at=None) for e in events],
    first_run=gaps(case / 'frames.csv', []), first_result=first.group(0) if first else None,
    resumed=gaps(case / 'receiver' / 'resumed-frames.csv', []), resumed_result=decoded.group(0) if decoded else None,
    resumed_ok=bool(decoded and int(decoded[1]) > 0 and decoded[1] == decoded[2] and decoded[3] == '0'),
    ring_opens=grep(r"capturing from the Rubylight display driver's frame push"),
    capture_errors=grep(r'(?i)frame ring.*(fail|refus|cannot)|falls? back|WGC until|panicked|device lost'),
    capture_reopens=grep(r'(?i)capture (reopen|restart|recover)|rebuilding the Rubylight frame ring|display changed'),
    displays_after_match_before=active_displays() == before)
result['passed'] = (rc == 0 and result['resumed_ok'] and not result['capture_errors']
                    and all(e['rc'] == 0 for e in events) and len(events) >= 3
                    and result['displays_after_match_before'])
(case / 'result.json').write_text(json.dumps(result, indent=2))
print(json.dumps({k: result.get(k) for k in ('label', 'passed', 'rc', 'error', 'events', 'first_run', 'resumed', 'resumed_ok', 'capture_errors')}))
