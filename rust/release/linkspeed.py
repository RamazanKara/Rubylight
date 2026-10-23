"""Rubylight against client link speeds: an isolated LAN host, a shaped Linux client on the NUC.

The host runs on this PC (isolated profile, bound to the LAN address). The client is nightjar-cli in
a container on the NUC (`ssh nuc`, image rl-link-client, built from Nightjar's protocol/ workspace);
linkshape.sh inside that container shapes its eth0 ingress, which is the host to client direction,
without touching the NUC's own network. Every arm launches the Desktop app, streams for SECONDS and
writes a summary row to <work>/results.jsonl; step arms change the rate during the stream.

  python linkspeed.py list
  python linkspeed.py shapecheck                   iperf3 -u from this PC against each rate
  python linkspeed.py run ARM [ARM ...] --work DIR [--package DIR] [--content-height PIXELS] [--keep-memory]
  python linkspeed.py report --work DIR            re-analyse the arms already run
  python linkspeed.py compare A_DIR B_DIR [--repeats N]   PASS/FAIL of candidate B against baseline A
  python linkspeed.py noise DIR [DIR ...]          run-to-run spread of picture age p50 (A/A) and repeats needed

An arm is `r<rate>-<codec>-<def|hi|4k>`, `step-<a>-<b>-<a>`, `wifi`, or a healthy-link arm
`hz-r<rate>-hevc-<mbps>`; `ARM@tag` runs the arm again into its own folder (a repeat for compare and noise).
Loss memory is reset before every arm unless --keep-memory is given.
The host holds C:\\src\\butterpollo-release\\GPU-LOCK while a batch runs (it waits for other agents).
"""
import argparse, csv, hashlib, json, os, pathlib, re, shlex, subprocess, sys, threading, time
import xml.etree.ElementTree as ET
import requests, urllib3

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from e2e_host import installed_idle  # noqa: E402
import timing_report as tr  # noqa: E402
import linkcompare as lc  # noqa: E402

urllib3.disable_warnings(urllib3.exceptions.InsecureRequestWarning)
LAN_IP = os.environ.get('LINKSPEED_HOST_IP', '192.168.4.69')
PORT = int(os.environ.get('LINKSPEED_PORT', '48323'))
NUC_IP = os.environ.get('LINKSPEED_NUC_IP', '192.168.4.10')
LOCK = pathlib.Path(os.environ.get('LINKSPEED_LOCK', r'C:\src\butterpollo-release\GPU-LOCK'))
LOCK_NAME = os.environ.get('LINKSPEED_LOCK_NAME', 'link-speed')
PROFILE = pathlib.Path(os.environ.get('LINKSPEED_PROFILE', r'C:\tests\linkspeed'))
DEFAULT_PACKAGE = pathlib.Path(r'C:\src\butterpollo-release\3.0.0-beta.1\release\butterpollo-rust-release')
MOTION_PROBE = pathlib.Path(r'C:\src\rl-target\release\examples\motion_probe.exe')
IPERF3 = pathlib.Path(r'C:\src\rl-link-tools\iperf3\iperf3.exe')
NUC_WORK = '/work'
NUC_WORK_HOST = 'rl-link/work'
RATES = [1000, 500, 300, 150, 80, 50, 30, 20, 10]
WARMUP = 5.0
SECONDS = 45
FREEZE_MS = 100.0


def nuc(*command, timeout=120, check=True, input_text=None):
    result = subprocess.run(['ssh', '-o', 'BatchMode=yes', 'nuc', shlex.join(command)], text=True, encoding='utf-8', errors='replace', capture_output=True,
                            timeout=timeout, input=input_text)
    if check and result.returncode:
        raise RuntimeError(f'ssh nuc {command}: {result.stderr.strip()[-400:]}')
    return result


def container(*command, **kw):
    return nuc('docker', 'exec', 'ljs', *command, **kw)


def shape(rate, queue=50, delay=0, jitter=0, loss=0, fresh=False):
    """Set the link. 1000 means unshaped; a rate change on a running shaper keeps its netem."""
    if rate >= 1000:
        container(f'{NUC_WORK}/linkshape.sh', 'down')
        return
    running = 'tbf' in container('tc', 'qdisc', 'show', 'dev', 'ifb0', check=False).stdout
    if fresh or not running or delay or jitter or loss:
        container(f'{NUC_WORK}/linkshape.sh', 'up', str(rate), str(queue), str(delay), str(jitter), str(loss))
    else:
        container(f'{NUC_WORK}/linkshape.sh', 'set', str(rate), str(queue))


def arm_spec(name, rate, codec='hevc', kbps=20000, width=1920, height=1080, fps=60, seconds=SECONDS, steps=(), queue=50,
             delay=0, jitter=0, loss=0, note=''):
    return dict(name=name, rate=rate, codec=codec, kbps=kbps, width=width, height=height, fps=fps, seconds=seconds,
                steps=list(steps), queue=queue, delay=delay, jitter=jitter, loss=loss, note=note)


def arms():
    table = {}
    for rate in RATES:
        high = 300 if rate >= 1000 else min(max(40, round(rate * 1.5)), 500)
        for codec, kind, kbps in (('hevc', 'def', 20000), ('hevc', 'hi', high * 1000), ('av1', 'def', 20000), ('h264', 'def', 20000)):
            name = f'r{rate}-{codec}-{kind}'
            table[name] = arm_spec(name, rate, codec, kbps)
    for rate in (1000, 300, 150):
        table[f'r{rate}-hevc-4k'] = arm_spec(f'r{rate}-hevc-4k', rate, 'hevc', 80000, 3840, 2160)
    # Rates change at STEP seconds after the stream started; each level is held 20 s.
    for a, b, kbps in ((300, 30, 20000), (80, 10, 20000), (300, 30, 40000), (80, 10, 40000)):
        suffix = '' if kbps == 20000 else f'-{kbps // 1000}m'
        name = f'step-{a}-{b}-{a}{suffix}'
        table[name] = arm_spec(name, a, 'hevc', kbps, seconds=75, steps=[(20, b), (40, a)])
    table['wifi'] = arm_spec('wifi', 150, delay=3, jitter=2, loss=0.5, note='150 Mbit/s, 3 ms +-2 ms, 0.5% loss')
    table['wifi-hi'] = arm_spec('wifi-hi', 150, kbps=60000, delay=3, jitter=2, loss=0.5, note='as wifi at 60 Mbps')
    # Healthy links: 1080p60 noise content at 20, 40 and 80 Mbps on an unshaped 1 GbE and on 500 and 300 Mbit/s.
    for rate in lc.HEALTHY_RATES:
        for mbps in lc.HEALTHY_MBPS:
            name = lc.hz_arm_name(rate, mbps)
            table[name] = arm_spec(name, rate, 'hevc', mbps * 1000, note='healthy link, must behave exactly as before')
    for rate in (150, 50):
        for codec in ('hevc',):
            name = f'q5-r{rate}-{codec}-def'
            table[name] = arm_spec(name, rate, codec, 20000, queue=5, note='5 ms queue')
            name = f'q5-r{rate}-{codec}-4k'
            table[name] = arm_spec(name, rate, codec, 80000, 3840, 2160, queue=5, note='5 ms queue, 4K: the keyframe overflows it')
    return table


# ---- host ---------------------------------------------------------------------------------------

class Host:
    def __init__(self, package, config_extra=()):
        self.package = pathlib.Path(package)
        self.process = None
        self.config_extra = list(config_extra)
        self.web = f'https://{LAN_IP}:{PORT + 1}'
        self.http = f'http://{LAN_IP}:{PORT}'
        self.log = PROFILE / 'logs' / 'butterpollo.log'

    def write_profile(self, device_id):
        import shutil
        if PROFILE.exists():
            shutil.rmtree(PROFILE)
        (PROFILE / 'record').mkdir(parents=True)
        lines = ['port = %d' % PORT, f'output_name = {device_id}', 'encoder = amf', 'capture = wgc', 'virtual_display_layout = extended',
                 'virtual_display_mode = disabled', 'dd_configuration_option = disabled', 'dd_resolution_option = disabled',
                 'dd_refresh_rate_option = disabled', 'dd_hdr_option = disabled', 'dd_always_restore_from_golden = false',
                 'dd_config_revert_on_disconnect = false', 'frame_limiter_enable = false', 'stream_audio = false',
                 'upnp = false', 'enable_discovery = false', 'vulkan_hdr_layer = false', 'system_tray = false',
                 'update_check_interval = 0', 'pyrowave = false', 'minimum_fps_target = 60'] + self.config_extra
        (PROFILE / 'sunshine.conf').write_text('\n'.join(lines) + '\n')
        (PROFILE / 'apps.json').write_text(json.dumps({'apps': [{'name': 'Desktop', 'cmd': '', 'virtual-display': False}]}))
        salt = os.urandom(8).hex()
        (PROFILE / 'sunshine_state.json').write_text(json.dumps({'username': 'test', 'salt': salt, 'password': hashlib.sha256(
            ('rust-smoke-only' + salt).encode()).digest()[::-1].hex().upper()}))

    FIREWALL = 'linkspeed test host'

    def firewall(self, add):
        # A scoped inbound rule for this package's exe from the NUC only; removed again at the end.
        script = (f"Remove-NetFirewallRule -DisplayName '{self.FIREWALL}' -ErrorAction SilentlyContinue; " +
                  (f"New-NetFirewallRule -DisplayName '{self.FIREWALL}' -Direction Inbound -Action Allow -Program '{self.package / 'butterpollo.exe'}' "
                   f"-RemoteAddress {NUC_IP} -Profile Any | Out-Null" if add else ''))
        subprocess.run(['powershell', '-NoProfile', '-Command', script], check=False, capture_output=True)

    def start(self):
        self.firewall(True)
        env = dict(os.environ, BUTTERPOLLO_TIMING_RECORD=str(PROFILE / 'record'), RUST_LOG='info', NO_PROXY='*',
                   PATH=str(self.package) + ';' + os.environ['PATH'])
        out = (PROFILE / 'host.stdout.log').open('w')
        self.process = subprocess.Popen([str(self.package / 'butterpollo.exe'), '--config-dir', str(PROFILE), '--assets',
                                         str(self.package / 'assets' / 'web'), '--port', str(PORT), '--bind', LAN_IP, '--no-tray'],
                                        stdout=out, stderr=subprocess.STDOUT, cwd=str(PROFILE), env=env,
                                        creationflags=subprocess.CREATE_NO_WINDOW)
        plain = requests.Session(); plain.trust_env = False
        deadline = time.monotonic() + 90
        while time.monotonic() < deadline:
            assert self.process.poll() is None, 'host exited'
            try:
                if plain.get(self.http + '/serverinfo', timeout=2).ok and self.log.exists() \
                        and 'encoder capability probe completed' in self.log.read_text(errors='replace'):
                    return
            except requests.RequestException:
                pass
            time.sleep(.5)
        raise RuntimeError('host not ready')

    def stop(self):
        self.firewall(False)
        if self.process and self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=20)
            except subprocess.TimeoutExpired:
                self.process.kill(); self.process.wait(timeout=60)

    def admin(self):
        s = requests.Session(); s.verify = False; s.trust_env = False
        r = s.post(self.web + '/api/auth/login', json={'username': 'test', 'password': 'rust-smoke-only'}, timeout=10)
        r.raise_for_status(); s.headers['X-CSRF-Token'] = r.json()['csrf_token']
        return s

    def pair(self, work):
        """Pair a client identity here and copy it to the container's /work (the launch must come from the NUC)."""
        from cryptography import x509
        from cryptography.hazmat.primitives import hashes, serialization
        from cryptography.hazmat.primitives.asymmetric import padding, rsa
        from cryptography.hazmat.primitives.ciphers import Cipher, algorithms, modes
        from cryptography.x509.oid import NameOID
        from datetime import datetime, timedelta, timezone
        import concurrent.futures
        key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
        subject = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, 'linkspeed')])
        cert = (x509.CertificateBuilder().subject_name(subject).issuer_name(subject).public_key(key.public_key())
                .serial_number(x509.random_serial_number()).not_valid_before(datetime.now(timezone.utc) - timedelta(days=1))
                .not_valid_after(datetime.now(timezone.utc) + timedelta(days=30)).sign(key, hashes.SHA256()))
        cert_pem = cert.public_bytes(serialization.Encoding.PEM)
        key_pem = key.private_bytes(serialization.Encoding.PEM, serialization.PrivateFormat.PKCS8, serialization.NoEncryption())
        work.mkdir(parents=True, exist_ok=True)
        (work / 'client.pem').write_bytes(cert_pem); (work / 'client-key.pem').write_bytes(key_pem)
        salt = os.urandom(16); pin = '1234'
        aes = hashlib.sha256(salt + pin.encode()).digest()[:16]
        uid = 'linkspeed-' + os.urandom(4).hex()

        def ecb(data, encrypt=True):
            c = Cipher(algorithms.AES(aes), modes.ECB()); op = c.encryptor() if encrypt else c.decryptor()
            return op.update(data) + op.finalize()

        def step(args):
            r = requests.get(self.http + '/pair', params={'uniqueid': uid, **args}, timeout=20, proxies={'http': None})
            r.raise_for_status(); root = ET.fromstring(r.text)
            assert root.attrib['status_code'] == '200' and root.findtext('paired') == '1', r.text
            return root

        admin = self.admin()
        with concurrent.futures.ThreadPoolExecutor() as executor:
            future = executor.submit(step, {'phrase': 'getservercert', 'salt': salt.hex(), 'clientcert': cert_pem.hex(), 'devicename': uid})
            deadline = time.monotonic() + 15
            while True:
                pending = admin.get(self.web + '/api/clients/pending', timeout=3).json()['requests']
                match = [p for p in pending if p['name'] == uid]
                if match:
                    break
                if future.done():
                    future.result()
                assert time.monotonic() < deadline, 'pairing request did not arrive'
                time.sleep(.1)
            admin.post(self.web + '/api/pin', json={'pin': pin, 'name': uid, 'uniqueid': match[0]['uniqueid']}, timeout=10).raise_for_status()
            root = future.result(timeout=20)
        server = x509.load_pem_x509_certificate(bytes.fromhex(root.findtext('plaincert')))
        challenge = os.urandom(16)
        root = step({'clientchallenge': ecb(challenge).hex()})
        response = ecb(bytes.fromhex(root.findtext('challengeresponse')), False)
        secret = os.urandom(16)
        root = step({'serverchallengeresp': ecb(hashlib.sha256(response[32:] + cert.signature + secret).digest()).hex()})
        server_proof = bytes.fromhex(root.findtext('pairingsecret'))
        assert response[:32] == hashlib.sha256(challenge + server.signature + server_proof[:16]).digest()
        server.public_key().verify(server_proof[16:], server_proof[:16], padding.PKCS1v15(), hashes.SHA256())
        step({'clientpairingsecret': (secret + key.sign(secret, padding.PKCS1v15(), hashes.SHA256())).hex()})
        paired = next(c for c in admin.get(self.web + '/api/clients/list', timeout=10).json()['clients'] if c['name'] == uid)
        admin.post(self.web + '/api/clients/update', json={'uuid': paired['uuid'], 'perm': 0x071f1f00}, timeout=10).raise_for_status()
        self.client_uuid = paired['uuid']
        for name in ('client.pem', 'client-key.pem'):
            subprocess.run(['scp', '-q', str(work / name), f'nuc:{NUC_WORK_HOST}/{name}'], check=True)
        print('paired', uid, flush=True)


class Sampler(threading.Thread):
    """The host's own view once a second: /api/rtsp/sessions, stamped with this PC's QPC in ms."""
    def __init__(self, admin, web, path):
        super().__init__(daemon=True)
        self.admin, self.web, self.path, self.stop_flag = admin, web, path, threading.Event()
        self.first_live = None

    def run(self):
        with self.path.open('w') as out:
            while not self.stop_flag.is_set():
                started = time.perf_counter()
                try:
                    sessions = self.admin.get(self.web + '/api/rtsp/sessions', timeout=3).json().get('sessions', [])
                except (requests.RequestException, ValueError):
                    sessions = []
                if sessions and self.first_live is None and sessions[0].get('frames_sent', 0) > 0:
                    self.first_live = started * 1000
                for s in sessions:
                    keep = {k: s.get(k) for k in ('encoder_bitrate_kbps', 'adaptive_bitrate', 'packet_pacing', 'fec_percent', 'frames_sent',
                                                  'frames_replaced', 'packets_sent', 'bytes_sent', 'idr_requests', 'reference_invalidations',
                                                  'send_loss_recoveries', 'slice_delivery_overflows', 'link_rate', 'state', 'uptime_seconds',
                                                  'width', 'height', 'fps', 'video_format')}
                    keep['qpc_ms'] = started * 1000
                    out.write(json.dumps(keep) + '\n')
                out.flush()
                self.stop_flag.wait(max(0, 1.0 - (time.perf_counter() - started)))


# ---- a batch ------------------------------------------------------------------------------------

def take_lock(wait_minutes):
    deadline = time.monotonic() + wait_minutes * 60
    while True:
        try:
            with LOCK.open('x') as f:
                f.write(f'{LOCK_NAME} {time.strftime("%Y-%m-%dT%H:%M:%S")}')
            return
        except FileExistsError:
            assert time.monotonic() < deadline, f'GPU-LOCK still held: {LOCK.read_text(errors="replace")}'
            time.sleep(1)


def release_lock():
    if LOCK.exists() and LOCK_NAME in LOCK.read_text(errors='replace'):
        LOCK.unlink()


def primary_display(package):
    diag = json.loads(subprocess.check_output([str(package / 'butterpollo.exe'), '--diagnostics'], text=True,
                                               env=dict(os.environ, PATH=str(package) + ';' + os.environ['PATH'])))
    monitor = next(m for m in diag['monitors'] if m['primary'])
    return monitor['display_name'], monitor['device_id']


def run_arm(spec, host, work, sampler_admin, keep_memory=False):
    out = work / spec['name']
    if out.exists():
        import shutil; shutil.rmtree(out)
    out.mkdir(parents=True)
    record_dir = PROFILE / 'record'
    before = {p.name for p in record_dir.glob('host-*.csv')}
    # The host remembers a lost keyframe for 24 h and paces the next sessions like Wi-Fi; every arm starts from the default.
    if not keep_memory:
        sampler_admin.post(host.web + '/api/clients/reset-network-history', json={'uuid': host.client_uuid}, timeout=10).raise_for_status()
    shape(spec['rate'], spec['queue'], spec['delay'], spec['jitter'], spec['loss'], fresh=True)
    log_offset = host.log.stat().st_size
    sampler = Sampler(sampler_admin, host.web, out / 'host-samples.jsonl')
    sampler.start()
    command = ['docker', 'exec', 'ljs', 'python3', f'{NUC_WORK}/linkspeed_client.py', LAN_IP, str(PORT), spec['codec'],
               str(spec['width']), str(spec['height']), str(spec['fps']), str(spec['seconds']), str(spec['kbps']),
               f'{NUC_WORK}/{spec["name"]}']
    started = time.perf_counter()
    client = subprocess.Popen(['ssh', '-o', 'BatchMode=yes', 'nuc', shlex.join(command)], text=True, stdout=subprocess.PIPE,
                              stderr=subprocess.STDOUT)
    steps = []
    # Step arms keep one shell open in the container: a new ssh connection from a Wi-Fi host stalls for
    # about a minute while the stream runs, which would put the step after the end of the arm.
    channel = None
    if spec['steps']:
        channel = subprocess.Popen(['ssh', '-o', 'BatchMode=yes', 'nuc', 'docker exec -i ljs sh'], text=True, stdin=subprocess.PIPE,
                                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    def shape_now(rate, queue):
        if channel is None or rate >= 1000:
            shape(rate, queue)
        else:
            channel.stdin.write(f'{NUC_WORK}/linkshape.sh set {rate} {queue}\n'); channel.stdin.flush()

    def stepper():
        deadline = time.monotonic() + 40
        while sampler.first_live is None and time.monotonic() < deadline and client.poll() is None:
            time.sleep(.1)
        if sampler.first_live is None:
            return
        for offset, rate in spec['steps']:
            wait = sampler.first_live / 1000 + offset - time.perf_counter()
            if wait > 0:
                time.sleep(wait)
            t0 = time.perf_counter()
            shape_now(rate, spec['queue'])
            steps.append(dict(offset=offset, rate=rate, issued_qpc_ms=t0 * 1000, done_qpc_ms=time.perf_counter() * 1000))

    thread = threading.Thread(target=stepper, daemon=True); thread.start()
    try:
        text = client.communicate(timeout=spec['seconds'] + 120)[0]
    except subprocess.TimeoutExpired:
        client.kill(); text = client.communicate()[0] + '\nTIMEOUT'
    (out / 'runner.log').write_text(text)
    thread.join(timeout=30)
    if channel is not None:
        channel.stdin.close(); channel.wait(timeout=10)
    time.sleep(2.5)  # the host writes the record's footer at stream teardown
    sampler.stop_flag.set(); sampler.join(timeout=5)
    subprocess.run(['scp', '-q', '-r', f'nuc:{NUC_WORK_HOST}/{spec["name"]}/.', str(out)], check=False)
    waited = time.monotonic() + 20
    new = []
    while time.monotonic() < waited:
        new = [p for p in record_dir.glob('host-*.csv') if p.name not in before]
        if new and tr.has_footer(new[0]):
            break
        time.sleep(.5)
    if new:
        (out / 'record').mkdir()
        for p in new:
            p.replace(out / 'record' / p.name)
    chunk = host.log.read_bytes()[log_offset:].decode(errors='replace')
    keep = [l for l in chunk.splitlines() if ' TRACE ' not in l and 'AMF encoder settings' not in l]
    (out / 'host-events.log').write_text('\n'.join(keep) + '\n')
    (out / 'spec.json').write_text(json.dumps(dict(spec, steps_issued=steps, wall_seconds=time.perf_counter() - started), indent=1))
    return analyse(out)


# ---- analysis -----------------------------------------------------------------------------------

def pct(values, p):
    return None if not values else tr.percentile(values, p)


def read_kv_lines(text, marker):
    rows = []
    for line in text.splitlines():
        if marker in line:
            rows.append({k: v for k, v in re.findall(r'(\w+)=("[^"]*"|\S+)', line)})
    return rows


def analyse(out):
    spec = json.loads((out / 'spec.json').read_text())
    result = dict(name=spec['name'], rate=spec['rate'], codec=spec['codec'], kbps=spec['kbps'], queue_ms=spec['queue'],
                  size=f"{spec['width']}x{spec['height']}", steps=spec['steps_issued'])
    client_log = (out / 'client.log').read_text(errors='replace') if (out / 'client.log').exists() else ''
    result['client_exit'] = (out / 'exit').read_text().strip() if (out / 'exit').exists() else 'none'
    result['client_summary'] = [l[:200] for l in client_log.splitlines() if l.startswith(('RESULT', 'STEADY', 'PERFORMANCE'))][:4]
    frames_csv = out / 'frames.csv'
    records = sorted((out / 'record').glob('host-*.csv')) if (out / 'record').exists() else []
    if not frames_csv.exists() or not records:
        result['error'] = 'no frames.csv' if not frames_csv.exists() else 'no host record'
        write_result(out, result)
        return result
    try:
        report, client, record = tr.build_report(frames_csv, records[-1])
    except ValueError as error:
        result['error'] = str(error); write_result(out, result); return result
    result['join'] = report['join']; result['bound_ms'] = report['bound_ms']
    with frames_csv.open(newline='') as f:
        extra = {int(r['wire_frame']): r for r in csv.DictReader(f)}
    frames = {f['frame_number']: f for f in client['frames']}
    record_present = record['present_source'] != 'capture'
    sent = sorted((n, r['first_sent']) for n, r in record['rows'].items() if r.get('first_sent') is not None)
    if not sent or not frames:
        result['error'] = 'empty'; write_result(out, result); return result
    host_t0 = sent[0][1]
    # The client ran for spec['seconds']; leave the last 1.5 s out so frames still in flight at its end are not counted lost.
    end_us = host_t0 + (spec['seconds'] - 1.5) * 1e6
    # Frames the client could have received: sent after the warm-up, up to the last one it shows.
    window = [(n, t) for n, t in sent if WARMUP * 1e6 <= t - host_t0 <= end_us - host_t0]
    delivered = [n for n, _ in window if n in frames]
    e2e, stages = [], {'network_last': [], 'decode': [], 'assemble': [], 'host_total': [], 'encode': []}
    for n in delivered:
        sv = tr.stage_values(frames[n], record_present)
        if sv.get('end_to_end') is not None:
            e2e.append(sv['end_to_end'] / 1000)
        for k in stages:
            if sv.get(k) is not None:
                stages[k].append(sv[k] / 1000)
    ages = [float(extra[n]['picture_age_ms']) for n in delivered if n in extra and extra[n].get('picture_age_ms')]
    sent_at = dict(sent)
    ends = sorted(frames[n]['end'] for n in delivered if frames[n]['end'] is not None)
    gaps = [(b - a) / 1000 for a, b in zip(ends, ends[1:])]
    freezes = [g for g in gaps if g > FREEZE_MS]
    # Each freeze with the send times (seconds into the stream) of the frames on either side of it.
    freeze_list = lc.freeze_events([(frames[n]['end'], (sent_at[n] - host_t0) / 1e6) for n in delivered
                                    if frames[n]['end'] is not None and n in sent_at], FREEZE_MS)
    idr_received = sum(1 for n in delivered if extra.get(n, {}).get('frame_type') == '1')
    summary = lambda v: dict(n=len(v), p50=pct(v, 50), p95=pct(v, 95), p99=pct(v, 99), max=max(v) if v else None)
    result.update(
        sent_frames=len(window), delivered_frames=len(delivered), lost_frames=len(window) - len(delivered),
        lost_percent=round(100 * (len(window) - len(delivered)) / max(1, len(window)), 2),
        delivered_fps=round(len(delivered) / max(1e-9, (window[-1][1] - window[0][1]) / 1e6), 1) if len(window) > 1 else None,
        e2e_ms=summary(e2e), picture_age_client_ms=summary(ages), freezes=dict(
            count=len(freezes), total_ms=round(sum(freezes), 1), longest_ms=round(max(freezes), 1) if freezes else 0),
        freeze_events=[[round(a, 2), round(b, 2), round(ms, 1)] for a, b, ms in freeze_list],
        network_last_ms=summary(stages['network_last']), host_total_ms=summary(stages['host_total']),
        decode_ms=summary(stages['decode']), idr_frames_received=idr_received)
    # The host's own view: sent Mbps over the steady window and the control counters at the end.
    samples = [json.loads(l) for l in (out / 'host-samples.jsonl').read_text().splitlines() if l.strip()]
    if samples:
        last = samples[-1]
        steady_samples = [s for s in samples if (s.get('uptime_seconds') or 0) >= WARMUP]
        if len(steady_samples) >= 2:
            dt = steady_samples[-1]['uptime_seconds'] - steady_samples[0]['uptime_seconds']
            result['host_sent_mbps'] = round((steady_samples[-1]['bytes_sent'] - steady_samples[0]['bytes_sent']) * 8 / dt / 1e6, 2) if dt > 0 else None
        result['host'] = {k: last.get(k) for k in ('idr_requests', 'reference_invalidations', 'send_loss_recoveries', 'frames_sent',
                                                   'frames_replaced', 'fec_percent', 'adaptive_bitrate')}
        result['encoder_kbps_seen'] = sorted({s['encoder_bitrate_kbps'] for s in samples if s.get('encoder_bitrate_kbps') is not None})
        pace = [json.dumps(s.get('packet_pacing'), sort_keys=True) for s in samples]
        result['pacing_snapshots'] = [json.loads(p) for p in dict.fromkeys(pace)][:6]
        # Per-sample series on the stream's own clock (seconds since the first frame was sent).
        clock = lambda s: round((s['qpc_ms'] - record['epoch_qpc_ms']) / 1000 - host_t0 / 1e6, 2)
        result['encoder_kbps_series'] = [[clock(s), s['encoder_bitrate_kbps']] for s in samples if s.get('encoder_bitrate_kbps') is not None]
        result['pacing_kbps_series'] = [[clock(s), (s.get('packet_pacing') or {}).get('current_kbps')] for s in samples]
        series_kbps = result['encoder_kbps_series']
        result['encoder_kbps_negotiated'] = series_kbps[0][1] if series_kbps else None
        mean = lc.mean_last([(t, v) for t, v in series_kbps if t >= 0])
        result['encoder_kbps_last20_mean'] = None if mean is None else round(mean, 1)
    events = (out / 'host-events.log').read_text(errors='replace') if (out / 'host-events.log').exists() else ''
    timings = read_kv_lines(events, 'stream timings')
    result['host_timing_lines'] = [{k: t.get(k) for k in ('fps', 'bitrate_kbps', 'sent_mbps', 'idr_requests', 'fec_recovered_frames',
                                                          'fec_unrecoverable_frames', 'fec_missing_packets', 'send_loss_recoveries')}
                                   for t in timings]
    result['pacing_selected'] = [l[l.find('network pacing'):][:230] for l in events.splitlines() if 'network pacing' in l][:6]
    result['loss_events'] = [l[:260] for l in events.splitlines() if re.search(r'lost keyframe|loss pacing|unrecoverable|loss response|pacing .*loss', l, re.I)
                             and 'stream timings' not in l][:10]
    result['started_with_loss_memory'] = 'pacing video like Wi-Fi from the first frame' in events
    result['latch'] = [l[l.find('reason='):][:300] for l in events.splitlines() if 'video frames are not arriving' in l][:2]
    # Rate changes the host announced: the legacy controller and, in builds that have it, LinkRate.
    result['adaptive_changes'] = dict(
        adaptive_bitrate=sum(1 for l in events.splitlines() if 'host adaptive bitrate' in l),
        link_rate=sum(1 for l in events.splitlines() if re.search(r'link[ _]rate', l, re.I) and 'network pacing' not in l and 'stream timings' not in l))
    result['recovery_keyframes'] = sum(1 for l in events.splitlines() if 'video recovery frame' in l)
    series = {}
    for n, t in sent:
        sec = int((t - host_t0) / 1e6)
        b = series.setdefault(sec, dict(sent=0, delivered=0, e2e=[]))
        if t > end_us:
            continue
        b['sent'] += 1
        if n in frames:
            b['delivered'] += 1
            v = tr.stage_values(frames[n], record_present).get('end_to_end')
            if v is not None:
                b['e2e'].append(v / 1000)
    result['series'] = {str(sec): dict(sent=b['sent'], delivered=b['delivered'], e2e_p50=pct(b['e2e'], 50), e2e_p95=pct(b['e2e'], 95))
                        for sec, b in sorted(series.items())}
    if result['steps']:
        epoch_ms = record['epoch_qpc_ms']
        result['clock_check_s'] = round(((samples[0]['qpc_ms'] if samples else 0) - epoch_ms) / 1000, 1)
        steps = []
        for st in result['steps']:
            at = (st['done_qpc_ms'] - epoch_ms) / 1000 - host_t0 / 1e6
            entry = dict(rate=st['rate'], at_s=round(at, 2), **settle(result['series'], at))
            negotiated = result.get('encoder_kbps_negotiated')
            if st['rate'] >= result['rate'] and negotiated:
                # The link is back: how long until the encoder is at 90 % of what the client asked for.
                entry['encoder_recovered_90_s'] = lc.recovery_time(result.get('encoder_kbps_series', []), at,
                                                                   lc.STEP_RECOVERY_FRACTION * negotiated)
            steps.append(entry)
        result['step_settle'] = steps
        result['phase_freeze_ms'] = lc.phase_freezes(freeze_list, [s['at_s'] for s in steps], spec['seconds'])
    write_result(out, result)
    return result


def settle(series, at):
    """Seconds from the step until three consecutive one-second bins deliver 95% of what was sent and
    have a median picture age within 2x the best bin median of the stream; the bins before `at` are
    ignored. Also the loss and age in the first 5 s after the step."""
    secs = sorted(int(k) for k in series)
    best = min((series[str(s)]['e2e_p50'] for s in secs if series[str(s)]['e2e_p50'] is not None), default=None)
    first = None
    for i, s in enumerate(secs):
        if s < at:
            continue
        run = secs[i:i + 3]
        if len(run) < 3:
            break
        ok = True
        for r in run:
            b = series[str(r)]
            if b['sent'] and b['delivered'] / b['sent'] < 0.95:
                ok = False
            if best is not None and (b['e2e_p50'] is None or b['e2e_p50'] > 2 * best + 5):
                ok = False
        if ok:
            first = s
            break
    after = [series[str(s)] for s in secs if at <= s < at + 5]
    sent_after = sum(b['sent'] for b in after)
    return dict(stable_after_s=None if first is None else round(first - at, 1),
                loss_first5s_percent=round(100 * (1 - sum(b['delivered'] for b in after) / max(1, sent_after)), 1) if sent_after else None,
                age_p95_first5s=pct([b['e2e_p95'] for b in after if b['e2e_p95'] is not None], 95))


def write_result(out, result):
    # JSON object keys must be strings
    (out / 'result.json').write_text(json.dumps(result, indent=1, default=str))


# ---- commands -----------------------------------------------------------------------------------

def shapecheck(args):
    for rate in [r for r in RATES if r < 1000]:
        for queue in (50, 5):
            shape(rate, queue, fresh=True)
            nuc('docker', 'exec', '-d', 'ljs', 'sh', '-c', 'iperf3 -s -1 --json > /work/iperf_srv.json 2>&1')
            time.sleep(1)
            subprocess.run([str(IPERF3), '-c', NUC_IP, '-u', '-b', f'{max(rate * 1.5, 40):.0f}M', '-l', '1400', '-t', '6', '--json'],
                           capture_output=True)
            time.sleep(1)
            data = json.loads(nuc('cat', f'{NUC_WORK_HOST}/iperf_srv.json').stdout)['end']['sum']
            got = (data['packets'] - data['lost_packets']) * 1428 * 8 / data['seconds'] / 1e6
            print(f'rate {rate} queue {queue} ms: received {got:.1f} Mbit/s at IP level ({got / rate * 100:.0f}%), '
                  f'lost {data["lost_percent"]:.1f}% of {data["packets"]}', flush=True)
    shape(1000)


def run(args):
    table = arms()
    names = args.arms
    unknown = [n for n in names if lc.base_name(n) not in table]
    assert not unknown, f'unknown arms: {unknown}'
    table = {n: dict(table[lc.base_name(n)], name=n) for n in names}  # ARM@tag is a repeat run in its own folder
    package = pathlib.Path(args.package)
    work = pathlib.Path(args.work); work.mkdir(parents=True, exist_ok=True)
    installed_idle()
    take_lock(args.wait)
    host = Host(package, args.config)
    motion = None
    try:
        display, device_id = primary_display(package)
        host.write_profile(device_id)
        host.start()
        host.pair(work)
        admin = host.admin()
        total = sum(table[n]['seconds'] + 45 for n in names) + 60
        strip = [str(args.content_height), 'noise'] if args.content_height > 128 else ['128']
        motion = subprocess.Popen([str(MOTION_PROBE), display, str(total), str(work / 'motion.json'), '120', *strip],
                                  stdout=(work / 'motion.log').open('w'), stderr=subprocess.STDOUT,
                                  creationflags=subprocess.CREATE_NO_WINDOW, env=dict(os.environ, PATH=r'C:\msys64\ucrt64\bin;' + os.environ['PATH']))
        time.sleep(1.5)
        assert motion.poll() is None, 'motion probe exited'
        for name in names:
            spec = table[name]
            print(f'== {name}', flush=True)
            result = run_arm(spec, host, work, admin, args.keep_memory)
            row = {k: result.get(k) for k in ('name', 'rate', 'codec', 'kbps', 'join', 'lost_percent', 'e2e_ms', 'freezes', 'host_sent_mbps',
                                              'decode_failures', 'recovery_keyframes', 'error', 'client_exit')}
            print(json.dumps(row), flush=True)
            with (work / 'results.jsonl').open('a') as f:
                f.write(json.dumps({k: v for k, v in result.items() if k != 'series'}, default=str) + '\n')
    finally:
        if motion and motion.poll() is None:
            motion.terminate()
        try:
            shape(1000)
        except Exception as error:  # noqa: BLE001
            print('shaper teardown failed:', error, file=sys.stderr)
        host.stop()
        release_lock()


def report(args):
    work = pathlib.Path(args.work)
    for d in sorted(p for p in work.iterdir() if (p / 'spec.json').exists()):
        analyse(d)
        print(d.name, 'reanalysed')


def noise(args):
    stats = lc.noise_stats(lc.load_runs(*args.dirs))
    print(f'{"arm":<22}{"runs":>5}  picture age p50 per run (ms)       range')
    for arm, a in stats['arms'].items():
        print(f'{arm:<22}{len(a["p50"]):>5}  {" ".join("%.3f" % v for v in a["p50"]):<34}{a["range"]:.3f}')
    if stats['pooled_sigma'] is None:
        print('no arm has two runs with data')
        return
    print(f'pooled sigma {stats["pooled_sigma"]:.3f} ms, worst arm {stats["worst_sigma"]:.3f} ms, largest run-to-run difference '
          f'{stats["max_range"]:.3f} ms')
    print(f'runs per side for a {lc.P50_BOUND_MS} ms bound (95 %): {stats["repeats_pooled"]} from the pooled sigma, '
          f'{stats["repeats_worst"]} from the worst arm')


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest='cmd', required=True)
    sub.add_parser('list'); sub.add_parser('shapecheck')
    r = sub.add_parser('run'); r.add_argument('arms', nargs='+'); r.add_argument('--work', required=True)
    r.add_argument('--package', default=str(DEFAULT_PACKAGE)); r.add_argument('--content-height', type=int, default=0)
    r.add_argument('--wait', type=int, default=60, help='minutes to wait for GPU-LOCK'); r.add_argument('--fresh', action='store_true')
    r.add_argument('--keep-memory', action='store_true', help='do not reset the host loss memory before each arm')
    r.add_argument('--config', action='append', default=[], metavar='KEY=VALUE')
    s = sub.add_parser('report'); s.add_argument('--work', required=True)
    c = sub.add_parser('compare', help='PASS/FAIL of candidate B_DIR against baseline A_DIR')
    c.add_argument('a_dir', metavar='A_DIR', help='baseline work directory'); c.add_argument('b_dir', metavar='B_DIR')
    c.add_argument('--repeats', type=int, default=lc.DEFAULT_REPEATS, help='runs per side an arm needs (default %(default)s)')
    c.add_argument('--arm', action='append', help='only this arm (repeatable)')
    n = sub.add_parser('noise', help='A/A spread of picture age p50 across repeated runs of the same arms')
    n.add_argument('dirs', nargs='+')
    args = p.parse_args()
    if args.cmd == 'list':
        for name, spec in arms().items():
            print(name, spec['rate'], spec['codec'], spec['kbps'], spec['steps'] or '')
    elif args.cmd == 'shapecheck':
        shapecheck(args)
    elif args.cmd == 'run':
        run(args)
    elif args.cmd == 'compare':
        rows = lc.compare([args.a_dir], [args.b_dir], args.repeats, args.arm)
        print(lc.format_rows(rows))
        sys.exit(1 if any(r['verdict'] == 'FAIL' for r in rows) else 0)
    elif args.cmd == 'noise':
        noise(args)
    else:
        report(args)


if __name__ == '__main__':
    main()
