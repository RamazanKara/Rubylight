"""The NUC side of linkspeed.py: launch the Desktop app as a paired client and run nightjar-cli.

Runs inside the client container (rl-link-client), whose eth0 ingress linkshape.sh shapes. The
paired identity is client.pem, client-key.pem and uid in WORK, created by linkspeed.py.

  linkspeed_client.py HOST PORT CODEC WIDTH HEIGHT FPS SECONDS KBPS OUTDIR

LINKSPEED_CLIENT_FEC=host|auto|0-255 sends rlFecPercentage at launch, as Nightjar's FEC setting does.
LINKSPEED_CLIENT_PROBE=1 times GET /bandwidth-probe before the launch and starts at
first_bitrate_kbps(KBPS, result), as Nightjar does with an automatic bitrate; probe.json keeps it.
"""
import json, os, pathlib, re, subprocess, sys, time
import xml.etree.ElementTree as ET
import requests, urllib3

urllib3.disable_warnings(urllib3.exceptions.InsecureRequestWarning)
WORK = pathlib.Path('/work')
PROBE_BYTES = 4 * 1024 * 1024
# Nightjar's share of a probed link it starts at and its floor (nightjar_core::http::probe_start_kbps).
PROBE_START_SHARE = 0.8
PROBE_FLOOR_KBPS = 2000


def fec_choice(value):
    """The rlFecPercentage value for LINKSPEED_CLIENT_FEC, or None when unset; anything else is an error."""
    if value is None or value == '':
        return None
    if value in ('host', 'auto') or (re.fullmatch(r'[0-9]{1,3}', value) and int(value) <= 255):
        return value
    raise SystemExit(f'LINKSPEED_CLIENT_FEC={value!r}: expected host, auto or 0-255')


def first_bitrate_kbps(configured_kbps, probe_mbps):
    """The bitrate a client starts at after a probe: the configured one, lowered to a share of the probe."""
    return max(min(PROBE_FLOOR_KBPS, configured_kbps), min(configured_kbps, int(probe_mbps * 1000 * PROBE_START_SHARE)))


def probe(session, https):
    """Mbit/s of one /bandwidth-probe, timed from the response headers; a body the host cut at its 3 s cap counts."""
    started = time.perf_counter()
    with session.get(https + '/bandwidth-probe', params={'bytes': PROBE_BYTES}, timeout=15, stream=True) as response:
        response.raise_for_status()
        headers = time.perf_counter()
        received = sum(len(chunk) for chunk in response.iter_content(64 * 1024))
    elapsed = time.perf_counter() - headers
    if received == 0 or received > PROBE_BYTES:
        raise RuntimeError(f'bandwidth probe returned {received} bytes')
    return dict(bytes=received, seconds=round(elapsed, 4), request_seconds=round(headers - started, 4),
                mbps=received * 8 / max(elapsed, 1e-6) / 1e6)


def main():
    host, port, codec, width, height, fps, seconds, kbps, out = sys.argv[1:10]
    out = pathlib.Path(out)
    out.mkdir(parents=True, exist_ok=True)
    fec = fec_choice(os.environ.get('LINKSPEED_CLIENT_FEC'))
    http_port = int(port)
    https = f'https://{host}:{http_port - 5}'
    s = requests.Session()
    s.verify = False
    s.trust_env = False
    s.cert = (str(WORK / 'client.pem'), str(WORK / 'client-key.pem'))
    if os.environ.get('LINKSPEED_CLIENT_PROBE') == '1':
        measured = probe(s, https)
        kbps = str(first_bitrate_kbps(int(kbps), measured['mbps']))
        (out / 'probe.json').write_text(json.dumps(dict(measured, configured_kbps=int(sys.argv[8]), first_kbps=int(kbps))))
        print('PROBE', f"{measured['mbps']:.1f} Mbit/s, starting at {kbps} kbps", flush=True)
    apps = ET.fromstring(s.get(https + '/applist', timeout=10).text)
    app = next(a for a in apps.findall('App') if a.findtext('AppTitle') == 'Desktop')
    params = {'appid': app.findtext('ID'), 'rikey': bytes(range(16)).hex(), 'rikeyid': '123', 'corever': '1',
              'mode': f'{width}x{height}x{fps}', 'hdrMode': '0'}
    if fec is not None:
        params['rlFecPercentage'] = fec
    launch = ET.fromstring(s.get(https + '/launch', timeout=60, params=params).text)
    assert launch.attrib['status_code'] == '200', ET.tostring(launch)
    url = launch.findtext('sessionUrl0')
    print('LAUNCH', url, flush=True)
    env = dict(os.environ, BUTTERPOLLO_TEST_HOST=host, BUTTERPOLLO_TEST_HW_DECODER=os.environ.get('HW_DECODER', 'vaapi'),
               BUTTERPOLLO_TEST_HW_DEVICE='/dev/dri/renderD128', BUTTERPOLLO_TEST_WARMUP_SECONDS='5',
               BUTTERPOLLO_TEST_TIMING_CSV=str(out / 'frames.csv'))
    command = ['nightjar-cli', url, codec, width, height, fps, seconds, kbps, '1']
    started = time.time()
    try:
        result = subprocess.run(command, env=env, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                timeout=int(seconds) + 60)
        (out / 'client.log').write_text(result.stdout)
        (out / 'exit').write_text(f'{result.returncode} {time.time() - started:.1f}\n')
        print('CLIENT EXIT', result.returncode, flush=True)
    except subprocess.TimeoutExpired as error:
        (out / 'client.log').write_text((error.stdout or b'').decode(errors='replace') if isinstance(error.stdout, bytes) else (error.stdout or ''))
        (out / 'exit').write_text('timeout\n')
        print('CLIENT TIMEOUT', flush=True)
    finally:
        try:
            s.get(https + '/cancel', timeout=20)
        except requests.RequestException as error:
            print('cancel failed', error, flush=True)


if __name__ == '__main__':
    main()
