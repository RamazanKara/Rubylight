"""Stream a packaged host through a loopback bottleneck, isolated from the installed host.

Like e2e.py (isolated profile and ports 48518-48544, the independent
moonlight-common-c receiver, a moving strip at twice the stream rate), but
the receiver reaches the host through udp_shaper on 127.0.0.2. The shaper
relays RTSP, control and audio unchanged and drains the video port toward
the client at --rate-kbps through a --queue-kib drop-tail queue, as a 100 Mbps
switch port in front of a TV box does. --rate-kbps 0 relays without shaping
(the control). No display-mode, HDR or default-audio change; the installed
host must be idle.

usage: bottleneck.py --package DIR --work DIR --client EXE --codec CODEC [--mode 2560x1440x60]
                     [--seconds 20] [--bitrate 40000] [--rate-kbps 100000] [--queue-kib 64]
                     [--rfi] [--config KEY=VALUE]... [--label NAME] [--noise-height PIXELS]
                     [--cut-ms MS [--fresh-rows ROWS]] [--hw-decoder TYPE] [--loss-percent PERCENT]
                     [--idr-probe N:MS] [--send-outage EVERY_MS:LENGTH_MS[:air|:stall]]
Writes WORK/bottleneck-LABEL/result.json: the frames the host sent, the
frames the receiver got whole (Moonlight hands the decoder complete frames
only, and none while it waits for a keyframe), keyframe requests and the
shaper's counters. With --cut-ms the whole desktop is noise that changes every
MS milliseconds (a scene cut), and the result adds stalls, picture age with and
without the frames around each cut, and whether the host armed oversize pacing
or the full Wi-Fi latch. --idr-probe makes the receiver ask N times for each
keyframe, MS apart, as a decoder that re-requests does; --send-outage sets the
host's test send outage (:stall fills the socket at a large frame, as #59's
adapter did). Every result has the slow-pace guard's latches by kind, its
lifts and ignored evidence, held frames, the recovery keyframes against their
cap and the loss records the profile kept. It reports; it does not judge. At the first sign of an
AMD video-engine hang (gpu_hang.py) it stops and exits with EXIT_GPU_HANG.
"""
import argparse, csv, json, os, pathlib, re, shutil, subprocess, sys, time
import requests
from bottleneck_metrics import (cut_metrics, host_environment, idr_probe_environment, latch_flags,
                                nominal_frame_bytes, pacing_events, route_kinds, timing_values)
from e2e_host import installed_idle, prepare, receiver_environment
from gpu_hang import EXIT_GPU_HANG, Guard, hang_result

parser = argparse.ArgumentParser()
parser.add_argument('--package', required=True, type=pathlib.Path)
parser.add_argument('--work', required=True, type=pathlib.Path)
parser.add_argument('--client', required=True, type=pathlib.Path)
parser.add_argument('--shaper', type=pathlib.Path, help='defaults to udp_shaper.exe beside the receiver')
parser.add_argument('--codec', required=True)
parser.add_argument('--mode', help='defaults to 720 pixels high at 60 FPS, matching the desktop aspect ratio')
parser.add_argument('--seconds', default='20')
parser.add_argument('--bitrate', default='40000', help="the client's bitrate in Kbps")
parser.add_argument('--rate-kbps', type=int, default=100000, help='bottleneck rate; 0 relays without shaping')
parser.add_argument('--queue-kib', type=int, default=64)
parser.add_argument('--rfi', action='store_true', help='the receiver declares reference frame invalidation, as hardware decoders do')
parser.add_argument('--config', action='append', default=[], metavar='KEY=VALUE', help='an extra sunshine.conf line for the host')
parser.add_argument('--label', help='output directory suffix; defaults to the codec, rate and queue')
parser.add_argument('--noise-height', type=int, default=0, metavar='PIXELS',
                    help='a taller strip with fresh per-pixel noise above its bottom 128 rows: frames and keyframes as large as a busy game')
parser.add_argument('--cut-ms', type=int, default=0, metavar='MS',
                    help='full-screen noise that changes every MS milliseconds: a scene cut as large as the encoder cap allows')
parser.add_argument('--fresh-rows', type=int, default=0, metavar='ROWS',
                    help='with --cut-ms, rows above the bottom band with fresh noise every frame, sizing the frames between cuts')
parser.add_argument('--hw-decoder', metavar='TYPE', help='the receiver decodes on the GPU, e.g. d3d11va')
parser.add_argument('--loss-percent', type=float, default=0., metavar='PERCENT',
                    help='random loss of video datagrams, with or without --rate-kbps: a lossy path with no bottleneck')
parser.add_argument('--idr-probe', metavar='N:MS',
                    help="the receiver asks for N keyframes MS milliseconds apart once it decodes, as Moonlight's decoder asks again")
parser.add_argument('--send-outage', metavar='SPEC',
                    help="the host's BUTTERPOLLO_TEST_SEND_OUTAGE: every_ms:length_ms[:air|:stall]")
args = parser.parse_args()
assert not (args.cut_ms and args.noise_height), '--cut-ms and --noise-height are separate scenes'
port = 48523
shaper = args.shaper or args.client.parent / 'udp_shaper.exe'
assert shaper.is_file(), 'build udp_shaper beside the receiver (cargo build -p butterpollo-windows --example udp_shaper)'
interop = pathlib.Path(__file__).resolve().parents[1] / 'tests' / 'interop.py'

installed_idle()
plain = requests.Session(); plain.trust_env = False
label = args.label or f'{args.codec}-{args.rate_kbps}-{args.queue_kib}k'
case = args.work / f'bottleneck-{label}'
assert case.resolve().parent == args.work.resolve(), 'test output must stay inside the work directory'
if case.exists():
    shutil.rmtree(case)
env, display, sink = prepare(args, case, port=port)
env.update(host_environment(args.send_outage))
profile = case / 'config'
log = profile / 'logs' / 'butterpollo.log'
width, height, fps = args.mode.split('x')

owned = []
def spawn(command, name, **kw):
    f = (case / name).open('w')
    p = subprocess.Popen(command, stdout=f, stderr=subprocess.STDOUT, creationflags=subprocess.CREATE_NO_WINDOW, **kw)
    owned.append((p, f)); return p
class Hang(Exception): pass
guard = Guard(log)
def watch():
    evidence = guard.check()
    if evidence: raise Hang(evidence)
hang = None
try:
    host = spawn([str(args.package / 'butterpollo.exe'), '--config-dir', str(profile), '--assets', str(args.package / 'assets' / 'web'),
                  '--bind', '127.0.0.1', '--no-tray'], 'host.stdout.log', cwd=case, env=env)
    deadline = time.monotonic() + 60
    while True:
        assert host.poll() is None, 'host exited'
        watch()
        if log.exists() and 'encoder capability probe completed' in log.read_text(errors='replace'):
            try:
                if plain.get(f'http://127.0.0.1:{port}/serverinfo', timeout=1).ok: break
            except requests.RequestException: pass
        assert time.monotonic() < deadline, 'host not ready'
        time.sleep(.2)
    fixture_seconds = str(int(args.seconds) + 90)
    tone = spawn([str(args.client.parent / 'audio_probe.exe'), fixture_seconds, sink['id'], '--render-only'], 'tone.log', env=env)
    if args.cut_ms:
        strip = [str(display['height']), f'cuts:{args.cut_ms}:{args.fresh_rows}']
    else:
        strip = [str(args.noise_height), 'noise'] if args.noise_height else ['128']
    motion = spawn([str(args.client.parent / 'motion_probe.exe'), display['display_name'], fixture_seconds,
                    str(case / 'motion.json'), str(int(fps) * 2), *strip], 'motion.log', env=env)
    # Sunshine's port layout: video base+9, control base+10, audio base+11, RTSP base+21.
    loss = ['--loss', str(args.loss_percent)] if args.loss_percent else []
    relay = spawn([str(shaper), *loss, '127.0.0.2','127.0.0.1', str(args.rate_kbps), str(args.queue_kib * 1024), str(case / 'shaper.json'),
                   f'{port + 21}:tcp', f'{port + 9}:shape', str(port + 10), str(port + 11)], 'shaper.log')
    time.sleep(.5)
    assert tone.poll() is None and motion.poll() is None and relay.poll() is None, 'a fixture exited'
    client_env = receiver_environment(env, display, args.mode, args.client)
    # A starved receiver is the measurement here, not a fixture failure.
    for key in ('BUTTERPOLLO_TEST_MIN_FPS', 'BUTTERPOLLO_TEST_REQUIRE_MOTION', 'BUTTERPOLLO_TEST_REQUIRE_PICTURE'):
        client_env.pop(key, None)
    client_env.update(BUTTERPOLLO_TEST_CLIENT_HOST='127.0.0.2', BUTTERPOLLO_TEST_TIMING_CSV=str(case / 'frames.csv'),
                      BUTTERPOLLO_TEST_RFI='1' if args.rfi else '0')
    if args.hw_decoder:
        client_env['BUTTERPOLLO_TEST_HW_DECODER'] = args.hw_decoder
    client_env.update(idr_probe_environment(args.idr_probe))
    (case / 'receiver').mkdir()
    receiver = spawn([sys.executable, str(interop), str(case / 'receiver'), args.codec, width, height, fps, args.seconds, args.bitrate, '4'],
                     'client.log', env=client_env)
    deadline = time.monotonic() + int(args.seconds) + 120
    while receiver.poll() is None:
        watch()
        if time.monotonic() > deadline: raise subprocess.TimeoutExpired(receiver.args, int(args.seconds) + 120)
        time.sleep(.25)
    rc = receiver.returncode
    time.sleep(.5)
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
            except subprocess.TimeoutExpired: print(f'{label}: process {p.pid} did not exit', file=sys.stderr)
        f.close()

hang = hang or guard.check()
if hang:
    result = hang_result(label, args.codec, args.mode, False, hang)
    (case / 'result.json').write_text(json.dumps(result, indent=2))
    print(json.dumps(result))
    sys.exit(EXIT_GPU_HANG)

client = (case / 'client.log').read_text(errors='replace')
host_log = log.read_text(errors='replace') if log.exists() else ''
def find(pattern, text=client, cast=float):
    match = re.search(pattern, text, re.MULTILINE)
    return cast(match.group(1)) if match else None
session = {}
for report in sorted((case / 'receiver').glob('stream-*.json')):
    sessions = [s for sample in json.loads(report.read_text()).get('samples', []) for s in sample.get('sessions', [])]
    if sessions: session = sessions[-1]
frames = []
if (case / 'frames.csv').exists():
    with (case / 'frames.csv').open() as f:
        frames = list(csv.DictReader(f))
shaper_stats = json.loads((case / 'shaper.json').read_text()) if (case / 'shaper.json').exists() else {}
video = next((p for p in shaper_stats.get('ports', []) if p['port'] == port + 9), {})
received = find(r'RESULT frames=(\d+)', cast=int)
sent = session.get('frames_sent')
pacing = re.findall(r'network pacing selected pacing_bps=(\d+)', host_log)
result = dict(
    label=label, codec=args.codec, mode=args.mode, client_bitrate_kbps=int(args.bitrate), rfi=args.rfi, noise_height=args.noise_height,
    rate_kbps=args.rate_kbps, queue_kib=args.queue_kib, config=args.config, client_exit=rc,
    host_frames_sent=sent, frames_received=received, decoded=find(r'decoded_frames=(\d+)', cast=int),
    decode_failures=find(r'RESULT .*?failures=(\d+)', cast=int),
    intact_ratio=round(received / sent, 4) if received is not None and sent else None,
    keyframes_received=sum(1 for row in frames if row['frame_type'] == '1'),
    steady_fps=find(r'STEADY .*? fps=([0-9.]+)'), interval_p99_ms=find(r'ARRIVAL_INTERVAL .*?p99_ms=([0-9.]+)'),
    interval_max_ms=find(r'ARRIVAL_INTERVAL .*?max_ms=([0-9.]+)'),
    unique_fps=find(r'VISUAL .*?unique_fps=([0-9.]+)'), picture_age_mean_ms=find(r'^PICTURE_AGE .*?mean_ms=([0-9.]+)'),
    picture_age_p95_ms=find(r'^PICTURE_AGE .*?p95_ms=([0-9.]+)'), picture_age_max_ms=find(r'^PICTURE_AGE .*?max_ms=([0-9.]+)'),
    host_idr_requests=session.get('idr_requests'), host_reference_invalidations=session.get('reference_invalidations'),
    host_send_loss_recoveries=session.get('send_loss_recoveries'), host_encoder_bitrate_kbps=session.get('encoder_bitrate_kbps'),
    host_packet_pacing=session.get('packet_pacing'), pacing_bps=[int(p) for p in pacing],
    client_lost_frame_logs=len(re.findall(r'Network dropped', client)), client_idr_waits=len(re.findall(r'Waiting for IDR frame', client)),
    shaper_video=video, loss_percent=args.loss_percent, cut_ms=args.cut_ms, fresh_rows=args.fresh_rows, hw_decoder=args.hw_decoder, gpu_hang=None,
)
if frames:
    encoder_kbps = result['host_encoder_bitrate_kbps'] or int(args.bitrate)
    result.update(cut_metrics(case / 'frames.csv', frames, nominal_frame_bytes(encoder_kbps, int(fps))))
result.update(latch_flags(host_log))
result.update(pacing_events(host_log))
# Counters the host keeps for the stream (or the encoder): the last line has them all.
timings = timing_values(host_log, ('reencoded_cuts', 'keyframes_over_cap', 'keyframes_past_fec', 'keyframe_cap_scale'))
last = lambda name: timings[name][-1] if timings[name] else None
scales = timings['keyframe_cap_scale']
result.update(reencoded_cuts=last('reencoded_cuts'), keyframes_over_cap=last('keyframes_over_cap'),
              keyframes_past_fec=last('keyframes_past_fec'),
              keyframe_cap_scale_min=min(scales) if scales else None, keyframe_cap_scale_last=last('keyframe_cap_scale'),
              host_frames_lost_on_send=session.get('frames_lost_on_send'),
              idr_probe=args.idr_probe, send_outage=args.send_outage,
              route_kinds=route_kinds(profile / 'sunshine_state.json'))
(case / 'result.json').write_text(json.dumps(result, indent=2))
print(json.dumps(result))
