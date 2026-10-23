"""The NUC side of linkspeed.py: launch the Desktop app as a paired client and run nightjar-cli.

Runs inside the client container (rl-link-client), whose eth0 ingress linkshape.sh shapes. The
paired identity is client.pem, client-key.pem and uid in WORK, created by linkspeed.py.

  linkspeed_client.py HOST PORT CODEC WIDTH HEIGHT FPS SECONDS KBPS OUTDIR
"""
import os, pathlib, subprocess, sys, time
import xml.etree.ElementTree as ET
import requests, urllib3

urllib3.disable_warnings(urllib3.exceptions.InsecureRequestWarning)
WORK = pathlib.Path('/work')


def main():
    host, port, codec, width, height, fps, seconds, kbps, out = sys.argv[1:10]
    out = pathlib.Path(out)
    out.mkdir(parents=True, exist_ok=True)
    http_port = int(port)
    https = f'https://{host}:{http_port - 5}'
    s = requests.Session()
    s.verify = False
    s.trust_env = False
    s.cert = (str(WORK / 'client.pem'), str(WORK / 'client-key.pem'))
    apps = ET.fromstring(s.get(https + '/applist', timeout=10).text)
    app = next(a for a in apps.findall('App') if a.findtext('AppTitle') == 'Desktop')
    launch = ET.fromstring(s.get(https + '/launch', timeout=60, params={
        'appid': app.findtext('ID'), 'rikey': bytes(range(16)).hex(), 'rikeyid': '123', 'corever': '1',
        'mode': f'{width}x{height}x{fps}', 'hdrMode': '0'}).text)
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
