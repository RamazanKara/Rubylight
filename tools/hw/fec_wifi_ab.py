r"""Nightjar's error correction on real Wi-Fi: Host (the PC's adaptive FEC) against Auto (rlFecPercentage=auto, the
client's 0x5539 reports), one Android device streaming from an isolated test host (catalog case net-fec-auto-retroid).

  fec_wifi_ab.py setup                     the profile, paired with the device's existing computer entry (no PIN)
  fec_wifi_ab.py run --arm host|auto ...   one locked run: prefs, Wi-Fi link, host up, stream, close, host down, analyse
  fec_wifi_ab.py table                     every run in results.jsonl as a Markdown table
  fec_wifi_ab.py reanalyse                 results.jsonl rebuilt from the saved logs, then the table

setup reuses a computer entry the device already has (--computer, default njrf): the profile takes that entry's UUID
and lists the device's client certificate as paired, and the device's entry takes the profile's server certificate.
Metrics: lost % and unrecovered FEC blocks from the client's 0x5539 reports (debug "fec report" lines, sent under both
arms), IDR requests, recovery frames and host processing from the 5 s "stream timings" lines, bitrate and FEC changes
from the host log, freezes (displayed-frame gaps over 100 ms after the first 5 s) from the client's latency CSV.
Locks: GPU-LOCK, then RETROID-LOCK, only while the host runs; never one held while waiting for the other.
Exit 3: an encoder hang in the host log; stop and investigate, never retry.
"""
import argparse, base64, csv, datetime, hashlib, io, json, os, pathlib, re, shutil, sqlite3, statistics, subprocess
import sys, tempfile, time, urllib.request, uuid

REL = pathlib.Path(r'C:\src\butterpollo-release')
HOST_DIR = REL / 'nj-e2e-host'
PROFILE = pathlib.Path(r'C:\tests\nj-fec-wifi')
OUT = REL / 'unified' / 'nj-fec-wifi'
BIND, PORT = '192.168.4.69', 48463
SERIAL = 'e24bb88e'
PACKAGE = 'com.butterpollo.client.njrf'
MOTION_PROBE = REL / 'fixtures' / 'motion_probe.exe'
AUDIO_PROBE = REL / 'fixtures' / 'audio_probe.exe'
APP_UUID = '404ccbeb-9f1c-46f2-a532-5cdafaac46aa'
LOCKS = ('GPU-LOCK', 'RETROID-LOCK')
OWNER = 'nj-fec-wifi'
WARMUP_S = 5.0
FREEZE_MS = 100.0
WINGET_ADB = pathlib.Path(os.environ.get('LOCALAPPDATA', r'C:\Users\ramaz\AppData\Local')) / (
    r'Microsoft\WinGet\Packages\Google.PlatformTools_Microsoft.Winget.Source_8wekyb3d8bbwe\platform-tools\adb.exe')
ADB = os.environ.get('ADB') or shutil.which('adb') or str(WINGET_ADB)


def adb(*args, check=True, binary=False):
    text = {} if binary else dict(encoding='utf-8', errors='replace')
    r = subprocess.run([ADB, '-s', SERIAL, *args], capture_output=True, timeout=60, **text)
    if check and r.returncode:
        raise SystemExit(f'adb {" ".join(args)}: {r.stderr or r.stdout}')
    return r.stdout


def sh(cmd, check=True):
    return adb('shell', cmd, check=check)


def run_as(cmd, check=True):
    return sh(f"run-as {PACKAGE} sh -c '{cmd}'", check=check)


def log(msg):
    print(f'{datetime.datetime.now():%H:%M:%S} {msg}', flush=True)


# --- profile and pairing ---------------------------------------------------------------------------------------------

def primary_monitor():
    diag = json.loads(subprocess.check_output([str(HOST_DIR / 'butterpollo.exe'), '--diagnostics'], text=True))
    return next(m for m in diag['monitors'] if m['primary'])


def virtual_sink():
    # Capturing Steam Streaming Speakers without making it the default keeps the PC's own sound on its speakers.
    endpoints = json.loads(subprocess.check_output([str(AUDIO_PROBE), '--list'], text=True))
    sink = next((e for e in endpoints if e['virtual_sink']), None)
    if not sink:
        raise SystemExit("Steam Streaming Speakers is required: the test host must not move the PC's own sound")
    return sink['id']


def write_profile(host_uuid, client_pem):
    (PROFILE / 'logs').mkdir(parents=True, exist_ok=True)
    monitor = primary_monitor()
    conf = [f'port = {PORT}', 'encoder = amf', 'capture = wgc', f"output_name = {monitor['device_id']}",
            'virtual_display_mode = disabled', 'virtual_display_layout = extended', 'dd_configuration_option = disabled',
            'dd_resolution_option = disabled', 'dd_refresh_rate_option = disabled', 'dd_hdr_option = disabled',
            'dd_always_restore_from_golden = false', 'dd_config_revert_on_disconnect = false',
            'install_steam_audio_drivers = false', 'stream_audio = true', 'audio_sink_capture_only = true',
            'auto_capture_sink = false', f'audio_sink = {virtual_sink()}', 'keep_sink_default = false',
            'upnp = false', 'enable_discovery = false',
            'vulkan_hdr_layer = false', 'system_tray = false', 'update_check_interval = 0', 'pyrowave = false',
            'sunshine_name = njrf']
    (PROFILE / 'sunshine.conf').write_text('\n'.join(conf) + '\n')
    (PROFILE / 'apps.json').write_text(json.dumps({'apps': [
        {'name': 'Desktop', 'cmd': '', 'uuid': APP_UUID, 'prep-cmd': [], 'virtual-display': False, 'working-dir': ''}]}, indent=2))
    salt = os.urandom(8).hex()
    state = {'username': 'test', 'salt': salt,
             'password': hashlib.sha256(('rust-smoke-only' + salt).encode()).digest()[::-1].hex().upper(),
             'root': {'uniqueid': host_uuid, 'rubylight_state_version': 1, 'named_devices': [
                 {'name': 'Retroid Pocket 6', 'cert': client_pem, 'uuid': str(uuid.uuid4()), 'enabled': True,
                  'perm': 118693632, 'coop_join': True, 'last_seen': int(time.time())}]}}
    (PROFILE / 'sunshine_state.json').write_text(json.dumps(state, indent=2))


def setup(args):
    with tempfile.TemporaryDirectory() as tmp:
        db = pathlib.Path(tmp) / 'computers4.db'
        sh(f'am force-stop {PACKAGE}')
        db.write_bytes(adb('exec-out', 'run-as', PACKAGE, 'cat', 'databases/computers4.db', binary=True))
        con = sqlite3.connect(db)
        row = con.execute('select UUID from Computers where ComputerName = ?', (args.computer,)).fetchone()
        if not row:
            raise SystemExit(f'the device has no computer named {args.computer}')
        host_uuid = row[0]
        client_pem = run_as('cat files/client.crt').replace('\r\n', '\n').strip() + '\n'
        if PROFILE.exists() and not args.force:
            raise SystemExit(f'{PROFILE} exists (--force rewrites it)')
        write_profile(host_uuid, client_pem)
        # One start makes the profile's credentials; nothing streams, but the host probes the encoder.
        take_locks(1800)
        host = None
        try:
            host = start_host('setup')
            pem_path = PROFILE / 'credentials' / 'cacert.pem'
            for _ in range(30):
                if pem_path.exists():
                    break
                time.sleep(1)
            pem = pem_path.read_text()
        finally:
            if host is not None:
                stop_host(host)
            release_locks()
        der = base64.b64decode(''.join(l for l in pem.splitlines() if l and not l.startswith('-----')))
        con.execute('update Computers set ServerCert = ? where UUID = ?', (der, host_uuid))
        con.commit(); con.close()
        adb('push', str(db), '/data/local/tmp/nj-fec-computers4.db')
        run_as('cp /data/local/tmp/nj-fec-computers4.db databases/computers4.db && rm -f databases/computers4.db-journal')
        sh('rm -f /data/local/tmp/nj-fec-computers4.db')
    log(f'profile {PROFILE} paired as {host_uuid} ({args.computer}), server cert {len(der)} bytes on the device')


# --- locks, host, device ---------------------------------------------------------------------------------------------

def lock_path(name):
    return REL / name


def ours(name):
    try:
        return lock_path(name).read_text().startswith(OWNER + ' ')
    except OSError:
        return False


def release_locks():
    for name in reversed(LOCKS):
        if ours(name):
            lock_path(name).unlink()


def take_locks(wait_s):
    deadline = time.monotonic() + wait_s
    while True:
        taken, held = [], None
        for name in LOCKS:
            try:
                with open(lock_path(name), 'x') as f:
                    f.write(f'{OWNER} {datetime.datetime.now().astimezone().isoformat()} pid={os.getpid()}')
                taken.append(name)
            except FileExistsError:
                held = f'{name}: {lock_path(name).read_text(errors="replace").strip()}'
                break
        if held is None:
            return
        for name in taken:
            if ours(name):
                lock_path(name).unlink()
        if time.monotonic() > deadline:
            raise SystemExit(f'lock busy: {held}')
        log(f'waiting, {held}')
        time.sleep(20)


def installed_host_idle():
    text = urllib.request.urlopen('http://127.0.0.1:47989/serverinfo', timeout=10).read().decode()
    active = re.search(r'<RustHostSessionCount>(\d+)<', text)
    pending = re.search(r'<RustHostPendingSessionCount>(\d+)<', text)
    if not active or not pending or active[1] != '0' or pending[1] != '0':
        raise SystemExit(f'installed host is not idle: {active and active[1]}/{pending and pending[1]}')


def our_hosts():
    out = subprocess.run(['powershell', '-NoProfile', '-Command',
                          "Get-CimInstance Win32_Process -Filter \"Name='butterpollo.exe'\" | "
                          f"Where-Object {{ $_.CommandLine -like '*{PROFILE}*' }} | ForEach-Object {{ $_.ProcessId }}"],
                         capture_output=True, text=True).stdout
    return [int(p) for p in out.split()]


def start_host(tag, settle=6):
    if our_hosts():
        raise SystemExit('a host on this profile already runs')
    for f in (PROFILE / 'logs').glob('*'):
        f.unlink()
    # A display change since setup gives the primary monitor a new id; a stale one fails the launch ("display unavailable").
    conf = PROFILE / 'sunshine.conf'
    output = f"output_name = {primary_monitor()['device_id']}"
    conf.write_text(re.sub(r'(?m)^output_name = .*$', lambda _: output, conf.read_text()))
    env = dict(os.environ, RUST_LOG='info,butterpollo=debug', NO_PROXY='*')
    OUT.mkdir(parents=True, exist_ok=True)
    p = subprocess.Popen([str(HOST_DIR / 'butterpollo.exe'), '--config-dir', str(PROFILE), '--port', str(PORT),
                          '--bind', BIND], env=env, stdout=open(OUT / f'{tag}-host.out', 'w'),
                         stderr=subprocess.STDOUT, creationflags=0x08000000)
    time.sleep(settle)
    if p.poll() is not None:
        raise SystemExit(f'host exited {p.returncode}')
    return p


def stop_host(p):
    p.terminate()
    try:
        p.wait(10)
    except subprocess.TimeoutExpired:
        p.kill()
    for pid in our_hosts():
        subprocess.run(['taskkill', '/F', '/PID', str(pid)], capture_output=True)


def close_stream():
    import http.cookiejar, ssl
    web = f'https://{BIND}:{PORT + 1}'
    opener = urllib.request.build_opener(urllib.request.HTTPSHandler(context=ssl._create_unverified_context()),
                                         urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()),
                                         urllib.request.ProxyHandler({}))
    post = lambda path, body, headers={}: opener.open(urllib.request.Request(
        web + path, method='POST', data=json.dumps(body).encode(), headers={'Content-Type': 'application/json', **headers}), timeout=10)
    token = json.loads(post('/api/auth/login', {'username': 'test', 'password': 'rust-smoke-only'}).read())['csrf_token']
    post('/api/apps/close', {}, {'X-CSRF-Token': token})


def pset(prefs, key, kind, value):
    """Sets one shared preference (the app stopped)."""
    if kind == 'string':
        line = f'    <string name="{key}">{value}</string>'
    else:
        line = f'    <{kind} name="{key}" value="{value}" />'
    lines = [l for l in prefs.splitlines() if f'name="{key}"' not in l and l.strip() != '</map>']
    return '\n'.join(lines + [line, '</map>']) + '\n'


def set_prefs(arm, kbps):
    path = f'shared_prefs/{PACKAGE}_preferences.xml'
    prefs = run_as(f'cat {path}').replace('\r\n', '\n')
    for key, kind, value in (('list_fec_choice', 'string', arm), ('seekbar_bitrate_kbps', 'int', kbps),
                             ('bitrate_user_set', 'boolean', 'true'), ('checkbox_auto_bitrate', 'boolean', 'false'),
                             ('adaptive_quality_mode', 'string', 'off'), ('video_format', 'string', 'forceh265'),
                             ('list_resolution', 'string', '1920x1080'), ('list_fps', 'string', '60'),
                             ('checkbox_enable_hdr', 'boolean', 'false'), ('checkbox_link_probe', 'boolean', 'false')):
        prefs = pset(prefs, key, kind, value)
    with tempfile.NamedTemporaryFile('w', newline='\n', suffix='.xml', delete=False) as f:
        f.write(prefs)
    adb('push', f.name, '/data/local/tmp/nj-fec-prefs.xml')
    os.unlink(f.name)
    run_as(f'cp /data/local/tmp/nj-fec-prefs.xml {path}')
    sh('rm -f /data/local/tmp/nj-fec-prefs.xml')
    got = run_as(f'cat {path}')
    if f'<string name="list_fec_choice">{arm}</string>' not in got or f'name="seekbar_bitrate_kbps" value="{kbps}"' not in got:
        raise SystemExit('prefs did not take')


def wifi_link():
    info = sh('dumpsys wifi', check=False)
    line = next((l for l in info.splitlines() if 'mWifiInfo' in l or l.startswith('WifiInfo:')), '')
    get = lambda k: (re.search(rf'{k}: ([^,]+)', line) or [None, None])[1]
    return {'rssi': get('RSSI'), 'link_mbps': get('Link speed'), 'tx_mbps': get('Tx Link speed'),
            'rx_mbps': get('Rx Link speed'), 'freq': get('Frequency'), 'standard': get('Wi-Fi standard')}


def run(args):
    tag = f'{args.arm}-{args.kbps // 1000}m-{args.rep}' + ('' if args.motion else '-desktop')
    if (OUT / f'{tag}-host.log').exists():
        raise SystemExit(f'{tag} exists')
    installed_host_idle()
    take_locks(args.lock_wait)
    host = motion = None
    try:
        installed_host_idle()
        sh('input keyevent KEYCODE_WAKEUP'); sh('svc power stayon true'); sh('wm dismiss-keyguard', check=False)
        sh(f'am force-stop {PACKAGE}')
        set_prefs(args.arm, args.kbps)
        run_as('rm -f files/butterpollo-latency.csv files/butterpollo-latency.csv.tmp')
        link_before = wifi_link()
        host = start_host(tag)
        if args.motion:
            # The test-owned moving desktop (rust/windows/examples/motion_probe.rs): a timestamp strip and, above it,
            # fresh noise every frame, so the encoder fills the bitrate as a busy game does. It covers the monitor.
            strip = [str(args.motion), 'noise'] if args.motion > 128 else ['128']
            motion = subprocess.Popen([str(MOTION_PROBE), primary_monitor()['display_name'], str(args.seconds + 40),
                                       str(OUT / f'{tag}-motion.json'), '60', *strip],
                                      stdout=open(OUT / f'{tag}-motion.log', 'w'), stderr=subprocess.STDOUT,
                                      creationflags=0x08000000,
                                      env=dict(os.environ, PATH=r'C:\msys64\ucrt64\bin;' + os.environ['PATH']))
            time.sleep(1.5)
            if motion.poll() is not None:
                raise SystemExit(f'motion probe exited {motion.returncode}')
        log(f'{tag}: stream {args.seconds} s, link {link_before}')
        sh(f'am start -n {PACKAGE}/com.limelight.ShortcutTrampoline --es Name njrf --es AppUuid {APP_UUID} '
           '--es AppName Desktop --ez HDR false')
        time.sleep(args.seconds / 2)
        link_mid = wifi_link()
        (OUT / f'{tag}-mid.png').write_bytes(adb('exec-out', 'screencap', '-p', binary=True))
        time.sleep(args.seconds - args.seconds / 2)
        try:
            close_stream()
        except Exception as e:
            log(f'close failed: {e}')
        time.sleep(4)
        sh(f'am force-stop {PACKAGE}')
        time.sleep(2)
        stop_host(host); host = None
        if motion is not None:
            motion.terminate(); motion = None
        csv_text = run_as('cat files/butterpollo-latency.csv', check=False)
        if 'frame_number' not in csv_text:
            csv_text = run_as('cat files/butterpollo-latency.csv.tmp', check=False)
        (OUT / f'{tag}-latency.csv').write_text(csv_text.replace('\r\n', '\n'))
        shutil.copy(PROFILE / 'logs' / 'butterpollo.log', OUT / f'{tag}-host.log')
    finally:
        if motion is not None:
            motion.terminate()
        if host is not None:
            stop_host(host)
        release_locks()
        sh('svc power stayon false', check=False)
    result = analyse(tag, args, {'before': link_before, 'mid': link_mid})
    with open(OUT / 'results.jsonl', 'a') as f:
        f.write(json.dumps(result) + '\n')
    log(json.dumps({k: result[k] for k in ('tag', 'choice', 'lost_percent', 'unrecovered_blocks', 'idr_requests',
                                           'freezes', 'host_mean_ms', 'bitrate_changes', 'fec_changes')}))
    if result['encoder_hang']:
        print('encoder hang in the host log: stop and investigate', file=sys.stderr)
        sys.exit(3)


# --- analysis --------------------------------------------------------------------------------------------------------

def kv(line):
    return {k: v.strip('"') for k, v in re.findall(r'(\w+)=("[^"]*"|\S+)', line)}


def analyse(tag, args, link):
    text = (OUT / f'{tag}-host.log').read_text(errors='replace')
    lines = text.splitlines()
    choice = [kv(l) for l in lines if 'client fec choice' in l]
    timings = [kv(l) for l in lines if 'stream timings' in l]
    reports = [kv(l) for l in lines if re.search(r'\bfec report\b', l) and 'refused' not in l]
    sent = sum(int(r['data']) + int(r['parity']) for r in reports)
    missing = sum(int(r['missing']) for r in reports)
    last = timings[-1] if timings else {}
    hosts = [float(t['host_mean_ms']) for t in timings[1:] if 'host_mean_ms' in t] or \
            [float(t['host_mean_ms']) for t in timings if 'host_mean_ms' in t]
    rows = list(csv.DictReader(io.StringIO((OUT / f'{tag}-latency.csv').read_text())))
    shown = sorted(int(r['render_ns'] or r['release_ns']) for r in rows
                   if r.get('status') == 'rendered' and (r.get('render_ns') or r.get('release_ns')))
    freezes = []
    if shown:
        t0 = shown[0]
        gaps = [((b - a) / 1e6, a) for a, b in zip(shown, shown[1:]) if a - t0 >= WARMUP_S * 1e9]
        freezes = [round(g, 1) for g, _ in gaps if g > FREEZE_MS]
    numbers = sorted({int(r['frame_number']) for r in rows if r.get('frame_number', '').isdigit()})
    gaps_frames = sum(b - a - 1 for a, b in zip(numbers, numbers[1:]) if b > a + 1)
    statuses = {}
    for r in rows:
        statuses[r.get('status')] = statuses.get(r.get('status'), 0) + 1
    r2p = sorted(float(r['receive_to_present_ms']) for r in rows if r.get('receive_to_present_ms'))
    q = lambda v, p: round(v[min(len(v) - 1, int(p / 100 * len(v)))], 2) if v else None
    return dict(
        tag=tag, arm=args.arm, kbps=args.kbps, seconds=args.seconds, motion=args.motion,
        when=datetime.datetime.now().astimezone().isoformat(timespec='seconds'), link=link,
        choice=choice[0].get('choice') if choice else None,
        fec_reports=len(reports), fec_reports_used=sum(1 for r in reports if r.get('used') == 'true'),
        lost_percent=round(100 * missing / sent, 3) if sent else None, missing_shards=missing, sent_shards=sent,
        unrecovered_blocks=sum(int(r['unrecovered']) for r in reports),
        idr_requests=int(last.get('idr_requests', 0)), reference_invalidations=int(last.get('reference_invalidations', 0)),
        send_loss_recoveries=int(last.get('send_loss_recoveries', 0)),
        recovery_frames=sum(1 for l in lines if 'video recovery frame' in l),
        host_mean_ms=round(statistics.mean(hosts), 3) if hosts else None,
        moonlight_host_p50_ms=round(statistics.median(float(t['moonlight_host_ms'].split('/')[0]) for t in timings
                                                      if 'moonlight_host_ms' in t), 3) if timings else None,
        sent_mbps=round(statistics.mean(float(t['sent_mbps']) for t in timings[1:]), 2) if len(timings) > 1 else None,
        bitrate_kbps_seen=sorted({int(t['bitrate_kbps']) for t in timings if 'bitrate_kbps' in t}),
        # Every controller (adaptive bitrate, LinkRate, the client) shows in the 5 s lines' encoder bitrate.
        bitrate_changes=sum(1 for a, b in zip(timings, timings[1:]) if a.get('bitrate_kbps') != b.get('bitrate_kbps')),
        link_steps_down=int(last.get('link_steps_down', 0)),
        fec_changes=sum(1 for l in lines if re.search(r'\badaptive fec\b', l)),
        freezes=len(freezes), freeze_ms=freezes[:20], client_frames=len(rows), client_statuses=statuses,
        client_frame_gaps=gaps_frames, receive_to_present_p50_ms=q(r2p, 50), receive_to_present_p95_ms=q(r2p, 95),
        encoder_hang=bool(re.search(r'encoder hang|gpu hang|device removed|DXGI_ERROR_DEVICE_HUNG', text, re.I)))


def reanalyse(args):
    """Rebuilds results.jsonl from the saved logs, keeping each run's arm, bitrate, time and Wi-Fi readings."""
    rows = [json.loads(l) for l in (OUT / 'results.jsonl').read_text().splitlines() if l.strip()]
    redone = [dict(analyse(r['tag'], argparse.Namespace(arm=r['arm'], kbps=r['kbps'], seconds=r['seconds'],
                                                        motion=r.get('motion', 1080)), r['link']), when=r['when'])
              for r in rows]
    (OUT / 'results.jsonl').write_text(''.join(json.dumps(r) + '\n' for r in redone))
    table(args)


def table(args):
    rows = [json.loads(l) for l in (OUT / 'results.jsonl').read_text().splitlines() if l.strip()]
    print('| Run | Choice | RSSI dBm / link Mbps | Lost % | Unrecovered blocks | Frames lost | IDR requests | Freezes | '
          'Host ms | Sent Mbps | Bitrate / FEC changes | Receive to present p50 / p95 ms |')
    print('| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- | --- |')
    for r in rows:
        link = r['link']['before']
        print(f"| {r['tag']} | {r['choice'] or 'host'} | {link['rssi']} / {(link['link_mbps'] or '').removesuffix('Mbps')} | "
              f"{r['lost_percent']} | {r['unrecovered_blocks']} | {r['client_frame_gaps']} | {r['idr_requests']} | "
              f"{r['freezes']} | {r['host_mean_ms']} | {r['sent_mbps']} | {r['bitrate_changes']} / {r['fec_changes']} | "
              f"{r['receive_to_present_p50_ms']} / {r['receive_to_present_p95_ms']} |")


def main():
    global OUT
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('--out', default=str(OUT), help='folder for the logs and results.jsonl')
    sub = ap.add_subparsers(dest='cmd', required=True)
    s = sub.add_parser('setup'); s.add_argument('--computer', default='njrf'); s.add_argument('--force', action='store_true')
    r = sub.add_parser('run')
    r.add_argument('--arm', choices=('host', 'auto'), required=True)
    r.add_argument('--kbps', type=int, default=40000)
    r.add_argument('--seconds', type=int, default=60)
    r.add_argument('--rep', type=int, default=1)
    r.add_argument('--motion', type=int, default=1080,
                   help='motion probe height in pixels, noise above 128; 0 streams the plain desktop')
    r.add_argument('--lock-wait', type=int, default=1800)
    sub.add_parser('table')
    sub.add_parser('reanalyse')
    args = ap.parse_args()
    OUT = pathlib.Path(args.out)
    {'setup': setup, 'run': run, 'table': table, 'reanalyse': reanalyse}[args.cmd](args)


if __name__ == '__main__':
    main()
