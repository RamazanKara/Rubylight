r"""Nightjar's Smoothest pacing on a device we own: one build against another, streaming a steady 60 fps from the
isolated host of fec_wifi_ab.py (catalog case nj-smoothest-retroid). Shows what a change to the decoder queue costs a
device that has no display queue problem: drops it should not make, by reason, next to the wait it should cut.

  smoothest_ab.py run --arm NAME --apk PATH --rep N [--seconds 300]   one locked run: install, prefs, host, stream, analyse
  smoothest_ab.py table                                               every run in results.jsonl as a Markdown table

Exit 3: an encoder hang in the host log; stop and investigate, never retry.
"""
import argparse, collections, csv, io, json, pathlib, re, shutil, statistics, subprocess, sys, tempfile, time, os

import fec_wifi_ab as f

OUT = f.REL / 'unified' / 'nj-smoothest'
WARMUP_S = 5.0
KBPS = 20000


def set_prefs(pacing):
    f.set_prefs('host', KBPS)
    path = f'shared_prefs/{f.PACKAGE}_preferences.xml'
    prefs = f.pset(f.run_as(f'cat {path}').replace('\r\n', '\n'), 'frame_pacing', 'string', pacing)
    with tempfile.NamedTemporaryFile('w', newline='\n', suffix='.xml', delete=False) as t:
        t.write(prefs)
    f.adb('push', t.name, '/data/local/tmp/nj-smoothest-prefs.xml')
    os.unlink(t.name)
    f.run_as(f'cp /data/local/tmp/nj-smoothest-prefs.xml {path}')
    f.sh('rm -f /data/local/tmp/nj-smoothest-prefs.xml')
    if f'<string name="frame_pacing">{pacing}</string>' not in f.run_as(f'cat {path}'):
        raise SystemExit('prefs did not take')


def active_hz(dumpsys):
    """The panel's rate now: the mode whose id dumpsys display names as active."""
    active = re.search(r'mActiveModeId=(\d+)', dumpsys)
    mode = active and re.search(rf'\bid={active[1]}\b[^}}]*?fps=([\d.]+)', dumpsys)
    return round(float(mode[1])) if mode else None


def pct(values, p):
    return round(sorted(values)[min(len(values) - 1, int(len(values) * p / 100))], 2) if values else None


def number(row, key):
    try:
        return float(row[key])
    except (KeyError, TypeError, ValueError):
        return None


def analyse(tag, arm, rep, seconds, mode):
    rows = list(csv.DictReader(io.StringIO((OUT / f'{tag}-latency.csv').read_text())))
    start = next((int(r['receive_ns']) for r in rows if r.get('receive_ns')), 0)
    late = [r for r in rows if r.get('receive_ns') and (int(r['receive_ns']) - start) / 1e9 >= WARMUP_S]
    reasons = collections.Counter(r['drop_reason'] for r in late if r.get('drop_reason'))
    span_s = (int(late[-1]['receive_ns']) - int(late[0]['receive_ns'])) / 1e9 if len(late) > 1 else 0
    residency = [v for v in (number(r, 'residency_excess_ms') for r in late) if v is not None]
    # Per second, as the Shield replays read it: the seconds whose median wait in the decoder is above 2 ms.
    seconds_wait = collections.defaultdict(list)
    for r in late:
        v = number(r, 'input_to_output_ms')
        if v is not None:
            seconds_wait[(int(r['receive_ns']) - start) // 1_000_000_000].append(v)
    base = min((statistics.median(v) for v in seconds_wait.values()), default=0)
    over_2ms = sum(1 for v in seconds_wait.values() if statistics.median(v) - base > 2)
    # This panel reports no render times (render_ns is blank, status invalid_render_time): read when the frame was released.
    shown = [number(r, 'release_ns') for r in late]
    shown = sorted(v for v in shown if v)
    gaps = [(b - a) / 1e6 for a, b in zip(shown, shown[1:])]
    release = [(number(r, 'release_ns') - number(r, 'receive_ns')) / 1e6 for r in late if number(r, 'release_ns')]
    host = (OUT / f'{tag}-host.log').read_text(errors='replace')
    return dict(tag=tag, arm=arm, rep=rep, seconds=seconds, mode=mode, frames=len(late), span_s=round(span_s, 1),
                reasons=dict(reasons), drops_per_min=round(sum(reasons.values()) / span_s * 60, 2) if span_s else None,
                csv_rows_lost=max((int(r['csv_rows_lost'] or 0) for r in late), default=0),
                input_to_output_p50=pct([v for w in seconds_wait.values() for v in w], 50),
                input_to_output_p95=pct([v for w in seconds_wait.values() for v in w], 95),
                seconds_wait_over_2ms=over_2ms, residency_p95=pct(residency, 95),
                receive_to_release_p50=pct(release, 50), receive_to_release_p95=pct(release, 95),
                gaps_over_25ms=sum(1 for g in gaps if g > 25), gaps_over_100ms=sum(1 for g in gaps if g > 100),
                encoder_hang=bool(re.search(r'encoder hang|gpu hang|device removed|DXGI_ERROR_DEVICE_HUNG', host, re.I)))


def run(args):
    tag = f'{args.arm}-{args.rep}'
    OUT.mkdir(parents=True, exist_ok=True)
    if (OUT / f'{tag}-host.log').exists():
        raise SystemExit(f'{tag} exists')
    f.OUT = OUT
    f.OWNER = 'nj-smoothest'
    f.installed_host_idle()
    f.take_locks(args.lock_wait)
    host = motion = None
    try:
        f.installed_host_idle()
        f.sh('input keyevent KEYCODE_WAKEUP'); f.sh('svc power stayon true'); f.sh('wm dismiss-keyguard', check=False)
        f.sh(f'am force-stop {f.PACKAGE}')
        f.adb('install', '-r', '-t', str(args.apk))
        set_prefs(args.pacing)
        f.run_as('rm -f files/butterpollo-latency.csv files/butterpollo-latency.csv.tmp')
        host = f.start_host(tag)
        # The test-owned moving desktop: every frame changes, so the stream stays at 60 fps. It covers the monitor.
        motion = subprocess.Popen([str(f.MOTION_PROBE), f.primary_monitor()['display_name'], str(args.seconds + 40),
                                   str(OUT / f'{tag}-motion.json'), '60', '128'],
                                  stdout=open(OUT / f'{tag}-motion.log', 'w'), stderr=subprocess.STDOUT,
                                  creationflags=0x08000000,
                                  env=dict(os.environ, PATH=r'C:\msys64\ucrt64\bin;' + os.environ['PATH']))
        time.sleep(1.5)
        if motion.poll() is not None:
            raise SystemExit(f'motion probe exited {motion.returncode}')
        f.log(f'{tag}: stream {args.seconds} s')
        f.sh(f'am start -n {f.PACKAGE}/com.limelight.ShortcutTrampoline --es Name njrf --es AppUuid {f.APP_UUID} '
             '--es AppName Desktop --ez HDR false')
        time.sleep(args.seconds / 2)
        mode = active_hz(f.sh('dumpsys display', check=False))
        (OUT / f'{tag}-mid.png').write_bytes(f.adb('exec-out', 'screencap', '-p', binary=True))
        time.sleep(args.seconds - args.seconds / 2)
        try:
            f.close_stream()
        except Exception as e:
            f.log(f'close failed: {e}')
        time.sleep(4)
        f.sh(f'am force-stop {f.PACKAGE}')
        time.sleep(2)
        f.stop_host(host); host = None
        motion.terminate(); motion = None
        text = f.run_as('cat files/butterpollo-latency.csv', check=False)
        if 'frame_number' not in text:
            text = f.run_as('cat files/butterpollo-latency.csv.tmp', check=False)
        (OUT / f'{tag}-latency.csv').write_text(text.replace('\r\n', '\n'))
        shutil.copy(f.PROFILE / 'logs' / 'butterpollo.log', OUT / f'{tag}-host.log')
    finally:
        if motion is not None:
            motion.terminate()
        if host is not None:
            f.stop_host(host)
        f.release_locks()
        f.sh('svc power stayon false', check=False)
    result = analyse(tag, args.arm, args.rep, args.seconds, mode)
    with open(OUT / 'results.jsonl', 'a') as out:
        out.write(json.dumps(result) + '\n')
    f.log(json.dumps(result))
    if result['encoder_hang']:
        print('encoder hang in the host log: stop and investigate', file=sys.stderr)
        sys.exit(3)


def table(_):
    print('| Run | Panel Hz | Frames | Drops by reason | Per min | Wait p50 / p95 ms | Seconds over 2 ms | '
          'Receive to release p50 / p95 ms | Release gaps over 25 / 100 ms |')
    print('|---|---|---|---|---|---|---|---|---|')
    for line in (OUT / 'results.jsonl').read_text().splitlines():
        r = json.loads(line)
        # Runs saved before the analysis read this column are read again from their CSV.
        r = dict(r, **analyse(r['tag'], r['arm'], r['rep'], r['seconds'], r['mode']))
        print(f"| {r['tag']} | {r['mode']} | {r['frames']} | {r['reasons'] or '-'} | {r['drops_per_min']} | "
              f"{r['input_to_output_p50']} / {r['input_to_output_p95']} | {r['seconds_wait_over_2ms']} | "
              f"{r['receive_to_release_p50']} / {r['receive_to_release_p95']} | {r['gaps_over_25ms']} / {r['gaps_over_100ms']} |")


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest='cmd', required=True)
    s = sub.add_parser('run')
    s.add_argument('--arm', required=True); s.add_argument('--apk', required=True, type=pathlib.Path)
    s.add_argument('--rep', type=int, default=1); s.add_argument('--seconds', type=int, default=300)
    s.add_argument('--pacing', default='smoothness'); s.add_argument('--lock-wait', type=int, default=1800)
    s.set_defaults(fn=run)
    sub.add_parser('table').set_defaults(fn=table)
    args = p.parse_args()
    args.fn(args)


if __name__ == '__main__':
    main()
