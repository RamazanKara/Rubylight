"""Stream a packaged host end to end, isolated from the installed one.

Starts the package's host with its own profile and ports (48518-48544),
pairs the independent moonlight-common-c client through rust/tests/interop.py,
launches the Desktop app, streams and decodes video and audio, then unpairs
and stops. A narrow moving strip and a virtual-speaker tone exercise capture.
No display-mode, HDR or default-audio change; the installed host must be idle.

usage: e2e.py --package DIR --work DIR --client EXE --codec CODEC [--mode 1280x720x60] [--seconds 12]
              [--vrr] [--recovery N] [--motion-at-rate] [--name NAME] [--noise-height PIXELS] [--hw-decoder TYPE]
              [--launch-fps FPS] [--record] [--motion-script SCRIPT] [--skip-first SECONDS] [--port PORT]
Writes WORK/e2e-NAME/result.json, NAME defaulting to CODEC[-vrr][-recovery|-at-rate].
A mode faster than the captured display refreshes is recorded as skipped.

--recovery N: the receiver asks for a keyframe N times, every
RECOVERY_INTERVAL_MS, as a client losing packets does, while the moving
strip runs at the stream rate (a game held there by a frame limit) and the
minimum frame rate keeps its default of 20. Each request must ride the
next new frame instead of the unchanged picture being encoded again.

--record: the host writes its timing record to WORK/e2e-NAME/record
(BUTTERPOLLO_TIMING_RECORD) and the receiver its frames.csv; the joined stage
table (timing_report.py: median, p95, p99, join mode, bound) goes into
result.json as timing. A record that cannot be joined fails the case.

The host traces every claim (RUST_LOG=info,pacing=trace) and the stream
fails if it encoded an unchanged picture again; the receiver's count of
pictures seen twice or skipped is reported only, since the strip at the
stream rate repeats and skips some pictures by itself.

--motion-script SCRIPT: the strip's animation, in motion_probe's argument form (a rate in Hz, jitter:8,14,9,13 or
hitch:90:100), instead of twice the stream rate or the stream rate; the cadence of a game for the pacing matrix.
--skip-first SECONDS: the warm-up left out of the --record timing table.
--port PORT: the host's HTTP port (default 48523, inside the 48518-48544 block); another runner takes another block.

--content CLIP: a raw Y4M game clip (demo_content.py) instead of the strip, for repeatable A/Bs on real
content: ffplay shows it in a borderless window of the stream's size behind the user's windows, the host
captures only that window, and the clip plays from its first picture when the receiver starts. The mode
defaults to the clip's. unique_fps, motion_coverage and picture age need the strip and are listed in
not_applicable, as are the frame-gap limits (the player adds jitter: content runs compare A/B arms,
the gap numbers stay in the result); the tone check stays (audio_probe is independent of the picture); result.json's content
has the player's CPU and the share of real time it kept, under 99 % failing the case. Needs a host that
takes window targets in output_name (after 3.0.0-rc.5).
--content-at X,Y: the clip window's corner (default 0,0); --ffplay: the player (default ffplay on PATH).

--motion-at-rate: the same strip, minimum frame rate and trace without
requests, the control for --recovery. BUTTERPOLLO_E2E_RUST_LOG replaces the
host's RUST_LOG; without pacing=trace in it the claim check is reported as
not measured (host_claims null) instead of failing.

The stream stops at the first sign of an AMD video-engine hang (gpu_hang.py)
and exits with EXIT_GPU_HANG, its result.json naming the evidence in gpu_hang.
"""
import argparse, json, pathlib, subprocess, sys, time
import xml.etree.ElementTree as ET
import requests
import demo_content
from e2e_result import evaluate, host_frames
from e2e_host import installed_idle, prepare, receiver_environment
from gpu_hang import EXIT_GPU_HANG, Guard, hang_result
from machine_load import load_since, snapshot
from timing_report import case_report, wait_for_footer

parser = argparse.ArgumentParser()
parser.add_argument('--package', required=True, type=pathlib.Path)
parser.add_argument('--work', required=True, type=pathlib.Path)
parser.add_argument('--client', required=True, type=pathlib.Path)
parser.add_argument('--codec', required=True)
parser.add_argument('--mode', help='defaults to 720 pixels high at 60 FPS, matching the desktop aspect ratio')
parser.add_argument('--seconds', default='12')
parser.add_argument('--bitrate', default='20000')
parser.add_argument('--vrr', action='store_true', help='launch as a client asking for VRR')
parser.add_argument('--config', action='append', default=[], metavar='KEY=VALUE', help='an extra sunshine.conf line for the host, e.g. wgc_user_helper=true')
parser.add_argument('--recovery', type=int, default=0, metavar='N', help='keyframe requests during the stream, with the motion strip at the stream rate')
parser.add_argument('--motion-at-rate', action='store_true', help='the strip at the stream rate without requests: the control for --recovery')
parser.add_argument('--name', help='the case name, e.g. hevc-120; defaults to the codec and options')
parser.add_argument('--hw-decoder', metavar='TYPE', help="the receiver decodes on the GPU, e.g. d3d11va; FFmpeg's software HEVC decoder takes about 14 ms a 1440p frame")
parser.add_argument('--launch-fps', metavar='FPS', help="launch at this rate and ask for the mode's in the stream request, as Artemis's Warp frame pacing does")
parser.add_argument('--record', action='store_true',
                    help='write the host timing record (BUTTERPOLLO_TIMING_RECORD) and the receiver frames.csv into the case, and the joined stage table into result.json as timing')
parser.add_argument('--motion-script', metavar='SCRIPT', help="the motion strip's animation: Hz, jitter:8,14,9,13 or hitch:90:100")
parser.add_argument('--port', type=int, default=48523, help="the host's HTTP port; its other ports follow from it")
parser.add_argument('--skip-first', type=float, default=0, metavar='SECONDS', help='warm-up left out of the --record timing table')
parser.add_argument('--noise-height', type=int, default=0, metavar='PIXELS',
                    help='a taller strip with fresh per-pixel noise above its bottom 128 rows, for high bitrates')
parser.add_argument('--content', type=pathlib.Path, metavar='CLIP',
                    help="a raw Y4M game clip, looped in ffplay's window, as the picture instead of the motion strip (demo_content.py)")
parser.add_argument('--content-at', default='0,0', metavar='X,Y', help="the clip window's top-left corner on the desktop")
parser.add_argument('--ffplay', default='ffplay', help='the ffplay that plays --content')
parser.add_argument('--quiet-wait', type=pathlib.Path, metavar='JSON',
                    help="machine_load.py's verdict before this stream (check.ps1), recorded as quiet_wait")
args = parser.parse_args()
quiet_wait = json.loads(args.quiet_wait.read_text()) if args.quiet_wait else None
RECOVERY_INTERVAL_MS = 500
MOTION_SCRIPT_EXTRA = 30  # seconds the strip runs beyond the stream when a script is set
at_rate = bool(args.recovery) or args.motion_at_rate
if at_rate:
    # The default an unconfigured host has; the release streams resend at the
    # full rate, which would add repeats of their own.
    args.config.insert(0, 'minimum_fps_target=20')
interop = pathlib.Path(__file__).resolve().parents[1] / 'tests' / 'interop.py'

installed_idle()
plain = requests.Session(); plain.trust_env = False
if args.content:
    assert not (at_rate or args.motion_script or args.noise_height), '--content replaces the motion strip'
    clip = demo_content.read_header(args.content)
    args.mode = args.mode or f"{clip['width']}x{clip['height']}x{round(clip['fps'])}"
    refused = demo_content.check_clip(clip, *map(int, args.mode.split('x')[:2]), float(args.mode.split('x')[2]))
    assert not refused, refused

name = args.name or (args.codec + ('-vrr' if args.vrr else '')
                     + ('-recovery' if args.recovery else '-at-rate' if args.motion_at_rate else ''))
case = args.work / f'e2e-{name}'
assert case.resolve().parent == args.work.resolve(), 'test output must stay inside the work directory'
if case.exists():
    import shutil; shutil.rmtree(case)
# The keyframe-request stream is judged on the host's per-claim trace.
env, display, sink = prepare(args, case, args.port, rust_log='info,pacing=trace' if at_rate else 'info')
# A near-silent sink fails the tone check for a reason the result must show.
audio_sink = dict(audio_sink=sink, warnings=[sink['warning']] if sink['warning'] else [])
profile = case / 'config'
host_exe = args.package / 'butterpollo.exe'
audio_probe = args.client.parent / 'audio_probe.exe'
motion_probe = args.client.parent / 'motion_probe.exe'
width, height, fps = args.mode.split('x')
# Fresh pictures cannot outpace the captured display: a faster stream case
# is recorded as skipped on a slower monitor instead of failing.
if display['refresh_hz'] < float(fps) * .97:
    result = dict(case=name, codec=args.codec, mode=args.mode, vrr=args.vrr, passed=True,
                  skipped=f"the display refreshes at {display['refresh_hz']} Hz, below the requested {fps} FPS")
    (case / 'result.json').write_text(json.dumps(result, indent=2))
    print(json.dumps(result))
    sys.exit(0)

owned = []
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
stream_load = None
player = content = None
try:
    if args.content:
        # The host captures only the clip's window (output_name = window:<hwnd>@<display>),
        # which waits paused on its first picture behind the user's windows.
        left, top = map(int, args.content_at.split(','))
        player = demo_content.Player(args.ffplay, args.content, int(width), int(height), left, top, case / 'player.log')
        conf = profile / 'sunshine.conf'
        lines = [line for line in conf.read_text().splitlines() if line.split('=', 1)[0].strip() != 'output_name']
        conf.write_text('\n'.join(lines + [f"output_name = {player.target(display['display_name'])}", '']))
    host_env = dict(env, BUTTERPOLLO_TIMING_RECORD=str(case / 'record')) if args.record else env
    if args.record:
        (case / 'record').mkdir()
    host = spawn([str(host_exe), '--config-dir', str(profile), '--assets', str(args.package / 'assets' / 'web'), '--bind', '127.0.0.1', '--no-tray'], 'host.stdout.log', cwd=case, env=host_env)
    deadline = time.monotonic() + 60
    while True:
        assert host.poll() is None, 'host exited'
        watch()
        if log.exists() and 'encoder capability probe completed' in log.read_text(errors='replace'):
            try:
                if plain.get(f'http://127.0.0.1:{args.port}/serverinfo', timeout=1).ok: break
            except requests.RequestException: pass
        assert time.monotonic() < deadline, 'host not ready'
        time.sleep(.2)
    # A scripted strip writes motion.json when it ends, so it runs only a little past the stream.
    fixture_seconds = str(int(args.seconds) + (MOTION_SCRIPT_EXTRA if args.motion_script else 90))
    tone = spawn([str(audio_probe), fixture_seconds, sink['id'], '--render-only'], 'tone.log', env=env)
    motion_rate = args.motion_script or (int(fps) if at_rate else int(fps) * 2)
    strip = [str(args.noise_height), 'noise'] if args.noise_height else ['128']
    motion = None if player else spawn([str(motion_probe), display['display_name'], fixture_seconds, str(case / 'motion.json'), str(motion_rate), *strip], 'motion.log', env=env)
    time.sleep(.5)
    assert tone.poll() is None and (motion is None or motion.poll() is None), 'audio or motion fixture exited'
    client_env = receiver_environment(env, display, args.mode, args.client, args.vrr, args.port)
    if player:
        # A game picture has no motion strip or barcode; the receiver still requires a detailed picture.
        for key in [key for key in client_env if key.startswith('BUTTERPOLLO_TEST_BARCODE')] + ['BUTTERPOLLO_TEST_REQUIRE_MOTION']:
            client_env.pop(key, None)
    if args.motion_script:
        # A script below the stream rate delivers fewer pictures by design; the matrix reads the timing table.
        client_env['BUTTERPOLLO_TEST_MIN_FPS'] = '1'
    if args.hw_decoder:
        client_env['BUTTERPOLLO_TEST_HW_DECODER'] = args.hw_decoder
    if args.launch_fps:
        client_env['BUTTERPOLLO_TEST_LAUNCH_FPS'] = args.launch_fps
    if args.record:
        client_env['BUTTERPOLLO_TEST_TIMING_CSV'] = str(case / 'frames.csv')
    if args.recovery:
        client_env.update(BUTTERPOLLO_TEST_IDR_PROBE=str(args.recovery),
                          BUTTERPOLLO_TEST_IDR_PROBE_INTERVAL_MS=str(RECOVERY_INTERVAL_MS))
    (case / 'receiver').mkdir()
    # Other work on the machine starves the fixtures: a timing failure under
    # load is told apart by the CPU the stream ran beside (its own included).
    load_start = snapshot()
    if player:
        # Every run streams the clip from its first picture.
        player.resume()
    receiver = spawn([sys.executable, str(interop), str(case / 'receiver'), args.codec, width, height, fps, args.seconds, args.bitrate, '4'], 'client.log', env=client_env)
    deadline = time.monotonic() + int(args.seconds) + 120
    while receiver.poll() is None:
        watch()
        if time.monotonic() > deadline: raise subprocess.TimeoutExpired(receiver.args, int(args.seconds) + 120)
        time.sleep(.25)
    rc = receiver.returncode
    stream_load = load_since(load_start)
    if player:
        content = dict(clip=str(args.content), **player.verdict())
    if args.motion_script:
        motion.wait(timeout=MOTION_SCRIPT_EXTRA + 30)
    if args.record:
        # The host writes the record's footer at stream teardown, just after the receiver is gone.
        wait_for_footer(case / 'record', 2)
except Hang as error:
    hang = str(error)
finally:
    for p, f in reversed(owned):
        if p.poll() is None: p.terminate()
        try: p.wait(timeout=10)
        except subprocess.TimeoutExpired:
            p.kill()
            # A host blocked in the driver exits only once the GPU reset returns its call.
            try: p.wait(timeout=120)
            except subprocess.TimeoutExpired: print(f'{name}: process {p.pid} did not exit', file=sys.stderr)
        f.close()
    if player:
        player.stop()

# A watchdog report can land after the receiver gave up on a frozen stream.
hang = hang or guard.check()
if hang:
    result = dict(hang_result(name, args.codec, args.mode, args.vrr, hang), machine_load=stream_load, quiet_wait=quiet_wait, **audio_sink)
    (case / 'result.json').write_text(json.dumps(result, indent=2))
    print(json.dumps(result))
    sys.exit(EXIT_GPU_HANG)

client = (case / 'client.log').read_text(errors='replace')
config = dict(line.split('=', 1) for line in (profile / 'sunshine.conf').read_text().splitlines() if '=' in line)
config = {key.strip(): value.strip() for key, value in config.items()}
requested_source = dict(width=int(width) if player else display['width'], height=int(height) if player else display['height'],
                        refresh_hz=display['refresh_hz'], pixel='RgbaF16' if display['hdr'] else 'Bgra8')
result = evaluate(client, rc, args.codec, args.mode, args.vrr, recovery=args.recovery,
                  requested_capture=config['capture'], requested_source=requested_source,
                  host_log=log.read_text(errors='replace') if log.exists() else '',
                  tone_log=(case / 'tone.log').read_text(errors='replace'),
                  host_frames=host_frames(case / 'receiver'),
                  claim_trace='pacing=trace' in env.get('RUST_LOG', ''), content=content)
result = dict(case=name, **result, machine_load=stream_load, quiet_wait=quiet_wait, **audio_sink)
if args.launch_fps:
    result['launch_fps'] = args.launch_fps
if args.record:
    # A record that cannot be joined fails the case: a missing measurement is not a pass.
    try:
        result['timing'] = case_report(case, args.skip_first)
    except (ValueError, OSError) as error:
        result.update(timing=dict(error=str(error)), passed=False)
(case / 'result.json').write_text(json.dumps(result, indent=2))
print(json.dumps(result))
sys.exit(0 if result['passed'] else 1)
