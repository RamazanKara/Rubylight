r"""A real background stream of a game on this PC, as a device plays it (catalog case background-stream-retroid): the
isolated test host as SYSTEM in the console session, the game through Nucleus Co-op on its own virtual display, the
Retroid streaming it with its own pad.

  background_stream.py up --game EXE --nucleus DIR   locks, stage of BACKGROUND_HOST's butterpollo.exe, profile, firewall, host up
  background_stream.py start [--hdr] [--codec av1|hevc|h264] [--kbps N] [--fps N]
  background_stream.py press a,down,...    pad buttons on the Retroid (a b x y start select up down left right)
  background_stream.py shot NAME           screencap of what the Retroid shows
  background_stream.py record NAME SECS    screenrecord of what the Retroid shows
  background_stream.py close               ends the stream through the host's console API; the game goes with down
  background_stream.py down                host down, stage's firewall rule and task removed, locks released

The system_launch one-shot and the firewall rule go through the release's elevation task (rust/release/elevation.ps1,
elevated_tasks.ps1), which accepts only the staged host at C:\tests\webrtc-host. Video stays on the virtual display, so
the PC's monitors keep what they show (layout extended). Exit 3: an encoder hang in the host log; stop, never retry.
Everything but up refuses to run without the locks, so a chained case stops doing anything after a failed up.
"""
import argparse, json, os, pathlib, re, shutil, subprocess, sys, time, urllib.request, uuid

import fec_wifi_ab as f

f.OWNER = 'nj-background'
OUT = f.REL / 'unified' / 'nj-background'
f.OUT = OUT
SOURCE_PROFILE = pathlib.Path(r'C:\tests\nj-fec-wifi')
PROFILE = pathlib.Path(r'C:\tests\nj-background')
STAGE = pathlib.Path(r'C:\tests\webrtc-host')
LAUNCHER = r'C:\ProgramData\ButterpolloRelease\system_launch.exe'
REQUESTS = f.REL / 'webrtc-e2e'
ELEVATION_TASK = 'ButterpolloReleaseElevated'
HOST_TASK = 'RubylightWebrtcE2eHost'
GAME_UUID = 'b1a2c3d4-0000-4000-8000-5ac0b1ac4f1a'
PAD = '/dev/input/event7'
KEYS = dict(a=304, b=305, x=308, y=307, select=314, start=315)
HATS = dict(up=(17, -1), down=(17, 1), left=(16, -1), right=(16, 1))
HANG = r'encoder hang|gpu hang|device removed|DXGI_ERROR_DEVICE_HUNG'


def copy_tree(*args):
    done = subprocess.run(['robocopy', *args, '/R:2', '/W:1', '/NFL', '/NDL', '/NJH', '/NJS', '/NP'], capture_output=True)
    if done.returncode >= 8:
        raise SystemExit(f'robocopy {args[0]} failed: {done.returncode}')


def elevate(steps):
    REQUESTS.mkdir(parents=True, exist_ok=True)
    request, answer, token = REQUESTS / 'request.json', REQUESTS / 'result.json', uuid.uuid4().hex
    answer.unlink(missing_ok=True)
    request.write_text(json.dumps(dict(id=token, steps=steps)))
    subprocess.run(['schtasks', '/Run', '/TN', ELEVATION_TASK], check=True, capture_output=True)
    deadline = time.monotonic() + 90
    while time.monotonic() < deadline:
        time.sleep(1)
        if answer.exists():
            try:
                got = json.loads(answer.read_text(encoding='utf-8-sig'))
            except ValueError:
                continue
            if got.get('id') == token:
                results = got['results'] if isinstance(got['results'], list) else [got['results']]
                for step, result in zip(steps, results):
                    f.log(f"elevated {step.get('args', [next(iter(step))])[0]}: {result['exit']} {result['output'][:120]}")
                return results
    request.unlink(missing_ok=True)
    raise SystemExit('the elevation task did not answer')


def write_profile(game, nucleus):
    if PROFILE.exists():
        # The profile's credentials are for SYSTEM and administrators once a SYSTEM host has run on it.
        for name in ('session.json', 'display-recovery.json', 'audio-recovery.json'):
            (PROFILE / name).unlink(missing_ok=True)
    else:
        shutil.copytree(SOURCE_PROFILE, PROFILE, ignore=shutil.ignore_patterns(
            'logs', 'crashes', 'covers', 'session.json', 'display-recovery.json', 'audio-recovery.json'))
        (PROFILE / 'logs').mkdir()
    conf = (PROFILE / 'sunshine.conf').read_text()
    output = f"output_name = {f.primary_monitor()['device_id']}"
    conf = re.sub(r'(?m)^output_name = .*$', lambda _: output, conf)
    conf = re.sub(r'(?m)^virtual_display_mode = .*$', 'virtual_display_mode = per_client', conf)
    conf = re.sub(r'(?m)^nucleus_path = .*\n', '', conf) + f'nucleus_path = {nucleus}\n'
    (PROFILE / 'sunshine.conf').write_text(conf)
    exe = pathlib.PureWindowsPath(game)
    apps = json.loads((SOURCE_PROFILE / 'apps.json').read_text(encoding='utf-8'))
    apps['apps'] = [a for a in apps['apps'] if a['name'] == 'Desktop'] + [{
        'name': exe.parent.name, 'uuid': GAME_UUID, 'cmd': f'"{exe}"', 'working-dir': str(exe.parent), 'prep-cmd': [],
        'background-stream': True}]
    (PROFILE / 'apps.json').write_text(json.dumps(apps, indent=2), encoding='utf-8')


def serverinfo():
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    return opener.open(f'http://{f.BIND}:{f.PORT}/serverinfo', timeout=5).read().decode()


def up(args):
    built = os.environ.get('BACKGROUND_HOST')
    if not built:
        raise SystemExit('set BACKGROUND_HOST to the butterpollo.exe to stage')
    f.installed_host_idle()
    f.take_locks(args.lock_wait)
    try:
        f.installed_host_idle()
        OUT.mkdir(parents=True, exist_ok=True)
        copy_tree(str(f.HOST_DIR), str(STAGE), '*.dll')
        for folder in ('assets', 'vulkan-layer', 'plugins', 'compatibility', 'drivers', 'tools'):
            copy_tree(str(f.HOST_DIR / folder), str(STAGE / folder), '/E')
        shutil.copy(built, STAGE / 'butterpollo.exe')
        write_profile(args.game, args.nucleus)
        for old in (PROFILE / 'logs').glob('*'):
            try:
                old.unlink()
            except OSError:
                pass
        command = f'{LAUNCHER} {STAGE}\\butterpollo.exe --config-dir {PROFILE} --port {f.PORT} --bind {f.BIND}'
        results = elevate([
            dict(firewall=True),
            dict(args=['/Create', '/TN', HOST_TASK, '/TR', command, '/SC', 'ONCE', '/ST', '23:59', '/RU', 'SYSTEM', '/RL',
                       'HIGHEST', '/F']),
            dict(args=['/Run', '/TN', HOST_TASK])])
        if any(r['exit'] for r in results):
            raise SystemExit(f'elevated steps failed: {results}')
        deadline = time.monotonic() + 40
        while time.monotonic() < deadline:
            try:
                text = serverinfo()
                f.log(f"host up: {re.search(r'<RustHostVersion>([^<]*)', text)[1]}")
                return
            except OSError:
                time.sleep(1)
        raise SystemExit('the host did not answer')
    except BaseException:
        down(None)
        raise


def start(args):
    f.sh('input keyevent KEYCODE_WAKEUP')
    f.sh('svc power stayon true')
    f.sh('wm dismiss-keyguard', check=False)
    f.sh(f'am force-stop {f.PACKAGE}')
    f.set_prefs('host', args.kbps)
    path = f'shared_prefs/{f.PACKAGE}_preferences.xml'
    prefs = f.run_as(f'cat {path}').replace('\r\n', '\n')
    for key, kind, value in (('checkbox_enable_hdr', 'boolean', str(args.hdr).lower()),
                             ('list_fps', 'string', str(args.fps)),
                             ('video_format', 'string', dict(av1='forceav1', hevc='forceh265', h264='neverh265')[args.codec])):
        prefs = f.pset(prefs, key, kind, value)
    local = OUT / 'prefs.xml'
    local.write_text(prefs, newline='\n')
    f.adb('push', str(local), '/data/local/tmp/nj-background-prefs.xml')
    local.unlink()
    f.run_as(f'cp /data/local/tmp/nj-background-prefs.xml {path}')
    f.sh('rm -f /data/local/tmp/nj-background-prefs.xml')
    name = re.sub(r'[^\w ]', '', args.app_name)
    f.sh(f"am start -n {f.PACKAGE}/com.limelight.ShortcutTrampoline --es Name njrf --es AppUuid {GAME_UUID} "
         f"--es AppName '{name}' --ez HDR {str(args.hdr).lower()}")
    f.log('started')


def press(args):
    for name in args.buttons.split(','):
        if name in KEYS:
            on, off = (1, KEYS[name], 1), (1, KEYS[name], 0)
        else:
            axis, value = HATS[name]
            on, off = (3, axis, value), (3, axis, 0)
        events = [on, (0, 0, 0), None, off, (0, 0, 0)]
        line = '; '.join('sleep 0.2' if e is None else f'sendevent {PAD} {e[0]} {e[1]} {e[2]}' for e in events)
        f.sh(line)
        f.log(f'pressed {name}')
        time.sleep(args.gap)


def shot(args):
    OUT.mkdir(parents=True, exist_ok=True)
    (OUT / f'{args.name}.png').write_bytes(f.adb('exec-out', 'screencap', '-p', binary=True))
    f.log(str(OUT / f'{args.name}.png'))


def record(args):
    f.sh('rm -f /data/local/tmp/nj-background.mp4')
    subprocess.run([f.ADB, '-s', f.SERIAL, 'shell', 'screenrecord', '--time-limit', str(args.seconds), '--bit-rate',
                    '30000000', '/data/local/tmp/nj-background.mp4'], timeout=args.seconds + 30)
    f.adb('pull', '/data/local/tmp/nj-background.mp4', str(OUT / f'{args.name}.mp4'))
    f.sh('rm -f /data/local/tmp/nj-background.mp4')
    f.log(str(OUT / f'{args.name}.mp4'))


def close(_):
    f.close_stream()
    f.log('closed')


def task_exists():
    return subprocess.run(['schtasks', '/Query', '/TN', HOST_TASK], capture_output=True).returncode == 0


def down(_):
    # Every step runs and the locks always go: a step that failed is reported at the end, with what is left behind.
    left = []
    try:
        results = elevate([dict(args=['/End', '/TN', HOST_TASK]), dict(kill=True),
                           dict(args=['/Delete', '/TN', HOST_TASK, '/F']), dict(firewall=True, remove=True)])
        # /End fails on a task that no longer runs, /Delete on one already gone.
        if results[2]['exit'] and task_exists():
            left.append(f"the task {HOST_TASK}: {results[2]['output'][:120]}")
        if results[3]['exit']:
            left.append(f"the firewall rule: {results[3]['output'][:120]}")
    except (SystemExit, OSError, subprocess.SubprocessError, KeyError, IndexError) as error:
        left.append(f'the host, its task and firewall rule: {error}')
    for line in (f'am force-stop {f.PACKAGE}', 'svc power stayon false'):
        try:
            f.sh(line, check=False)
        except (SystemExit, OSError, subprocess.SubprocessError) as error:
            left.append(f'the Retroid ({line}): {error}')
    f.release_locks()
    if left:
        raise SystemExit('down left behind: ' + '; '.join(left))
    log = PROFILE / 'logs' / 'butterpollo.log'
    if log.exists():
        shutil.copy(log, OUT / f'{time.strftime("%H%M%S")}-host.log')
        if re.search(HANG, log.read_text(errors='replace'), re.I):
            print('encoder hang in the host log: stop and investigate', file=sys.stderr)
            sys.exit(3)


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest='cmd', required=True)
    s = sub.add_parser('up'); s.add_argument('--game', required=True); s.add_argument('--lock-wait', type=int, default=1800)
    s.add_argument('--nucleus', required=True)
    s.set_defaults(fn=up)
    s = sub.add_parser('start'); s.add_argument('--hdr', action='store_true'); s.add_argument('--codec', default='av1')
    s.add_argument('--kbps', type=int, default=50000); s.add_argument('--fps', type=int, default=60)
    s.add_argument('--app-name', default='Game'); s.set_defaults(fn=start)
    s = sub.add_parser('press'); s.add_argument('buttons'); s.add_argument('--gap', type=float, default=1.0)
    s.set_defaults(fn=press)
    s = sub.add_parser('shot'); s.add_argument('name'); s.set_defaults(fn=shot)
    s = sub.add_parser('record'); s.add_argument('name'); s.add_argument('seconds', type=int); s.set_defaults(fn=record)
    sub.add_parser('close').set_defaults(fn=close)
    sub.add_parser('down').set_defaults(fn=down)
    args = p.parse_args()
    # A chained case goes on after a failed up: without the locks, what follows would hit whoever holds them.
    if args.cmd != 'up' and not all(f.ours(name) for name in f.LOCKS):
        raise SystemExit(f'{args.cmd}: this run does not hold the locks, so up did not work')
    args.fn(args)


if __name__ == '__main__':
    main()
