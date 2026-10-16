"""Stream a packaged host end to end, isolated from the installed one.

Starts the package's host with its own profile and ports (48518-48544),
pairs the independent moonlight-common-c client through rust/tests/interop.py,
launches the Desktop app, streams and decodes video and audio, then unpairs
and stops. The host streams a virtual display beside the monitor (extended
layout); a narrow moving strip drawn on it and a virtual-speaker tone exercise
capture. No default-audio change; the installed host must be idle, and the
shell elevated (the display driver admits administrators and SYSTEM).

usage: e2e.py --package DIR --work DIR --client EXE --codec CODEC [--mode 1280x720x60] [--seconds 12]
              [--vrr] [--recovery N] [--motion-at-rate] [--name NAME] [--noise-height PIXELS] [--hw-decoder TYPE]
              [--launch-fps FPS]
Writes WORK/e2e-NAME/result.json, NAME defaulting to CODEC[-vrr][-recovery|-at-rate].

--recovery N: the receiver asks for a keyframe N times, every
RECOVERY_INTERVAL_MS, as a client losing packets does, while the moving
strip runs at the stream rate (a game held there by a frame limit) and the
minimum frame rate keeps its default of 20. Each request must ride the
next new frame instead of the unchanged picture being encoded again.

The host traces every claim (RUST_LOG=info,pacing=trace) and the stream
fails if it encoded an unchanged picture again; the receiver's count of
pictures seen twice or skipped is reported only, since the strip at the
stream rate repeats and skips some pictures by itself.

--motion-at-rate: the same strip, minimum frame rate and trace without
requests, the control for --recovery. BUTTERPOLLO_E2E_RUST_LOG replaces the
host's RUST_LOG; without pacing=trace in it the claim check is reported as
not measured (host_claims null) instead of failing.

The stream stops at the first sign of an AMD video-engine hang (gpu_hang.py)
and exits with EXIT_GPU_HANG, its result.json naming the evidence in gpu_hang.
"""
import argparse, ctypes, json, pathlib, subprocess, sys, threading, time
from ctypes import wintypes
import requests
from e2e_result import evaluate, host_frames
from e2e_host import installed_idle, prepare, receiver_environment
from gpu_hang import EXIT_GPU_HANG, Guard, hang_result

parser = argparse.ArgumentParser()
parser.add_argument('--package', required=True, type=pathlib.Path)
parser.add_argument('--work', required=True, type=pathlib.Path)
parser.add_argument('--client', required=True, type=pathlib.Path)
parser.add_argument('--codec', required=True)
parser.add_argument('--mode', help='defaults to 720 pixels high at 60 FPS, matching the desktop aspect ratio')
parser.add_argument('--seconds', default='12')
parser.add_argument('--bitrate', default='20000')
parser.add_argument('--vrr', action='store_true', help='launch as a client asking for VRR')
parser.add_argument('--config', action='append', default=[], metavar='KEY=VALUE', help='an extra sunshine.conf line for the host, e.g. frame_pacing_smoothing=false')
parser.add_argument('--recovery', type=int, default=0, metavar='N', help='keyframe requests during the stream, with the motion strip at the stream rate')
parser.add_argument('--motion-at-rate', action='store_true', help='the strip at the stream rate without requests: the control for --recovery')
parser.add_argument('--name', help='the case name, e.g. hevc-120; defaults to the codec and options')
parser.add_argument('--hw-decoder', metavar='TYPE', help="the receiver decodes on the GPU, e.g. d3d11va; FFmpeg's software HEVC decoder takes about 14 ms a 1440p frame")
parser.add_argument('--launch-fps', metavar='FPS', help="launch at this rate and ask for the mode's in the stream request, as Artemis's Warp frame pacing does")
parser.add_argument('--noise-height', type=int, default=0, metavar='PIXELS',
                    help='a taller strip with fresh per-pixel noise above its bottom 128 rows, for high bitrates')
args = parser.parse_args()
RECOVERY_INTERVAL_MS = 500
at_rate = bool(args.recovery) or args.motion_at_rate
if at_rate:
    # The default an unconfigured host has; the release streams resend at the
    # full rate, which would add repeats of their own.
    args.config.insert(0, 'minimum_fps_target=20')
interop = pathlib.Path(__file__).resolve().parents[1] / 'tests' / 'interop.py'


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

name = args.name or (args.codec + ('-vrr' if args.vrr else '')
                     + ('-recovery' if args.recovery else '-at-rate' if args.motion_at_rate else ''))
case = args.work / f'e2e-{name}'
assert case.resolve().parent == args.work.resolve(), 'test output must stay inside the work directory'
if case.exists():
    import shutil; shutil.rmtree(case)
# The keyframe-request stream is judged on the host's per-claim trace.
env, display, sink = prepare(args, case, rust_log='info,pacing=trace' if at_rate else 'info')
profile = case / 'config'
host_exe = args.package / 'butterpollo.exe'
audio_probe = args.client.parent / 'audio_probe.exe'
motion_probe = args.client.parent / 'motion_probe.exe'
width, height, fps = args.mode.split('x')
# The stream's own virtual display has the stream's mode.
streamed = dict(display, width=int(width), height=int(height), hdr='hdr' in args.codec)
before = active_displays()

owned, probes, stop = [], {}, threading.Event()
def spawn(command, name, **kw):
    f = (case / name).open('w')
    p = subprocess.Popen(command, stdout=f, stderr=subprocess.STDOUT, creationflags=subprocess.CREATE_NO_WINDOW, **kw)
    owned.append((p, f)); return p
class Hang(Exception): pass
log = profile / 'logs' / 'butterpollo.log'
guard = Guard(log)
def watch():
    evidence = guard.check()
    if evidence: raise Hang(evidence)
hang = None
try:
    host = spawn([str(host_exe), '--config-dir', str(profile), '--assets', str(args.package / 'assets' / 'web'), '--bind', '127.0.0.1', '--no-tray'], 'host.stdout.log', cwd=case, env=env)
    deadline = time.monotonic() + 60
    while True:
        assert host.poll() is None, 'host exited'
        watch()
        if log.exists() and 'encoder capability probe completed' in log.read_text(errors='replace'):
            try:
                if plain.get('http://127.0.0.1:48523/serverinfo', timeout=1).ok: break
            except requests.RequestException: pass
        assert time.monotonic() < deadline, 'host not ready'
        time.sleep(.2)
    fixture_seconds = str(int(args.seconds) + 90)
    tone = spawn([str(audio_probe), fixture_seconds, sink['id'], '--render-only'], 'tone.log', env=env)
    motion_rate = int(fps) if at_rate else int(fps) * 2
    strip = [str(args.noise_height), 'noise'] if args.noise_height else ['128']

    def place_strip():
        # The strip goes onto the stream's virtual display as soon as it is on the desktop.
        while not stop.is_set():
            for display_name in active_displays() - before:
                if display_name not in probes or probes[display_name].poll() is not None:
                    probes[display_name] = spawn([str(motion_probe), display_name, fixture_seconds, str(case / f'motion-{len(probes)}.json'),
                                                  str(motion_rate), *strip], f'motion-{len(probes)}.log', env=env)
            time.sleep(.1)
    threading.Thread(target=place_strip, daemon=True).start()
    time.sleep(.5)
    assert tone.poll() is None, 'audio fixture exited'
    client_env = receiver_environment(env, streamed, args.mode, args.client, args.vrr)
    if args.hw_decoder:
        client_env['BUTTERPOLLO_TEST_HW_DECODER'] = args.hw_decoder
    if args.launch_fps:
        client_env['BUTTERPOLLO_TEST_LAUNCH_FPS'] = args.launch_fps
    if args.recovery:
        client_env.update(BUTTERPOLLO_TEST_IDR_PROBE=str(args.recovery),
                          BUTTERPOLLO_TEST_IDR_PROBE_INTERVAL_MS=str(RECOVERY_INTERVAL_MS))
    (case / 'receiver').mkdir()
    receiver = spawn([sys.executable, str(interop), str(case / 'receiver'), args.codec, width, height, fps, args.seconds, args.bitrate, '4'], 'client.log', env=client_env)
    deadline = time.monotonic() + int(args.seconds) + 120
    while receiver.poll() is None:
        watch()
        if time.monotonic() > deadline: raise subprocess.TimeoutExpired(receiver.args, int(args.seconds) + 120)
        time.sleep(.25)
    rc = receiver.returncode
except Hang as error:
    hang = str(error)
finally:
    stop.set()
    for p, f in reversed(owned):
        if p.poll() is None: p.terminate()
        try: p.wait(timeout=10)
        except subprocess.TimeoutExpired:
            p.kill()
            # A host blocked in the driver exits only once the GPU reset returns its call.
            try: p.wait(timeout=120)
            except subprocess.TimeoutExpired: print(f'{name}: process {p.pid} did not exit', file=sys.stderr)
        f.close()
# The host's teardown removes the virtual display and restores the layout.
deadline = time.monotonic() + 15
while active_displays() != before and time.monotonic() < deadline:
    time.sleep(.2)

# A watchdog report can land after the receiver gave up on a frozen stream.
hang = hang or guard.check()
if hang:
    result = hang_result(name, args.codec, args.mode, args.vrr, hang)
    (case / 'result.json').write_text(json.dumps(result, indent=2))
    print(json.dumps(result))
    sys.exit(EXIT_GPU_HANG)

client = (case / 'client.log').read_text(errors='replace')
# The display's refresh is the host's choice (a 120 fps stream ran on a 240 Hz display), so
# only its size and pixel format are expected.
requested_source = dict(width=streamed['width'], height=streamed['height'], pixel='RgbaF16' if streamed['hdr'] else 'Bgra8')
result = evaluate(client, rc, args.codec, args.mode, args.vrr, recovery=args.recovery,
                  requested_source=requested_source,
                  host_log=log.read_text(errors='replace') if log.exists() else '',
                  tone_log=(case / 'tone.log').read_text(errors='replace'),
                  host_frames=host_frames(case / 'receiver'),
                  claim_trace='pacing=trace' in env.get('RUST_LOG', ''))
result = dict(case=name, **result, virtual_displays=sorted(probes), displays_after_match_before=active_displays() == before)
if not probes:
    result['passed'] = False
    result['failures'] = [*result.get('failures', []), 'no virtual display appeared for the moving strip']
if not result['displays_after_match_before']:
    result['passed'] = False
    result['failures'] = [*result.get('failures', []), 'the display layout was not restored after the stream']
if args.launch_fps:
    result['launch_fps'] = args.launch_fps
(case / 'result.json').write_text(json.dumps(result, indent=2))
print(json.dumps(result))
sys.exit(0 if result['passed'] else 1)
