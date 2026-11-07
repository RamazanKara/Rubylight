r"""How long Nightjar takes to say the PC is gone, and that it stays quiet when the PC is not (catalog case nj-host-gone).

  host_gone.py run --event kill|stop|path|wifi|suspend|pathblip [--hold S] [--engine rust|classic] [--tag T]

One locked run on the Retroid against the isolated test host of fec_wifi_ab.py (run its setup once): a stream, then the
event, then the app's own log says when it noticed and what it did. Events:
  kill      the host process killed outright (nothing sent, its ports closed: a crash, a PC shutting down)
  stop      the host closed the way a service stop does (its control stream says goodbye)
  path      answers to this device blackholed on the PC (a host route to an unused address), the device's own link fine:
            a PC switched off or unplugged. The route goes when the run ends
  pathblip  the same for --hold seconds only (path and pathblip need an elevated shell)
  wifi      the device's Wi-Fi off for --hold seconds, then on again (adb is on USB)
  suspend   the host process frozen for --hold seconds (a stalled host: the OS still answers)
kill, stop and path pass when the message comes at all, and print the time; the others pass when the stream survives.
Results: <out>\results.jsonl, per run <tag>-logcat.txt, -end.png, -host.log.
"""
import argparse, ctypes, datetime, json, pathlib, re, shutil, subprocess, sys, time

sys.path.insert(0, str(pathlib.Path(__file__).parent))
import fec_wifi_ab as fec

fec.OWNER = 'nj-host-gone'
fec.OUT = fec.REL / 'unified' / 'nj-host-gone'
OUT = fec.OUT
DEVICE_IP = '192.168.4.107'
UNUSED_ON_LINK = '192.168.4.250'


def device_time_offset():
    """PC clock minus device clock, seconds, from the tightest of a few round trips."""
    best = None
    for _ in range(5):
        a = time.time()
        d = float(fec.adb('shell', 'date +%s.%N').strip())
        b = time.time()
        if best is None or b - a < best[0]:
            best = (b - a, (a + b) / 2 - d)
    return best[1]


def host_pid():
    pids = fec.our_hosts()
    if not pids:
        raise SystemExit('the test host is gone')
    return pids[0]


def suspend(pid, frozen):
    ntdll = ctypes.WinDLL('ntdll')
    kernel = ctypes.WinDLL('kernel32')
    kernel.OpenProcess.restype = ctypes.c_void_p
    handle = kernel.OpenProcess(0x0800, False, pid)  # PROCESS_SUSPEND_RESUME
    if not handle:
        raise SystemExit('cannot open the test host')
    (ntdll.NtSuspendProcess if frozen else ntdll.NtResumeProcess)(ctypes.c_void_p(handle))
    kernel.CloseHandle(ctypes.c_void_p(handle))


def ctrl_c(pid):
    """Ctrl+C on the host's hidden console, from a helper that takes it for the moment and ignores the event itself."""
    code = ('import ctypes,sys;k=ctypes.WinDLL("kernel32");k.FreeConsole();'
            f'sys.exit(0 if k.AttachConsole({pid}) and k.SetConsoleCtrlHandler(None,True) and k.GenerateConsoleCtrlEvent(0,0) else 1)')
    subprocess.run([sys.executable, '-c', code], check=True, creationflags=0x08000000)


def blackhole(on):
    subprocess.run(['route', 'add' if on else 'delete', DEVICE_IP, *(['mask', '255.255.255.255', UNUSED_ON_LINK] if on else [])],
                   check=True, capture_output=True)


def set_prefs(engine):
    path = f'shared_prefs/{fec.PACKAGE}_preferences.xml'
    prefs = fec.run_as(f'cat {path}').replace('\r\n', '\n')
    for key, kind, value in (('list_resolution', 'string', '1920x1080'), ('list_fps', 'string', '60'),
                             ('checkbox_enable_hdr', 'boolean', 'false'), ('checkbox_link_probe', 'boolean', 'false')):
        prefs = fec.pset(prefs, key, kind, value)
    if engine:
        prefs = fec.pset(prefs, 'checkbox_rust_engine', 'boolean', 'true' if engine == 'rust' else 'false')
    local = OUT / 'prefs.xml'
    local.write_text(prefs, newline='\n')
    fec.adb('push', str(local), '/data/local/tmp/nj-gone-prefs.xml')
    fec.run_as(f'cp /data/local/tmp/nj-gone-prefs.xml {path}')
    fec.sh('rm -f /data/local/tmp/nj-gone-prefs.xml')


def focus():
    return ' '.join(re.findall(r'mCurrentFocus=\S+ \S+ (\S+?)\}', fec.sh('dumpsys window', check=False))) or '?'


# App log lines that mark a moment of the end. The first of each counts.
MARKS = {'noticed': 'Stream disconnected', 'gone': 'The PC is gone', 'stopping': 'Stream stopping'}


def analyse(logcat, t0, offset):
    marks, attempts = {}, 0
    for line in logcat.splitlines():
        m = re.match(r'\s*(\d+\.\d+)\s', line)
        if not m:
            continue
        at = float(m[1]) + offset - t0
        if at < 0:
            continue
        for name, text in MARKS.items():
            if text in line and name not in marks:
                marks[name] = round(at, 2)
        if 'Automatic stream recovery' in line or 'Recovery:' in line:
            attempts += 1
    return marks, attempts


def run(args):
    OUT.mkdir(parents=True, exist_ok=True)
    tag = args.tag or f'{args.event}-{args.hold or 0}s-{args.engine or "default"}-{datetime.datetime.now():%H%M%S}'
    fec.installed_host_idle()
    fec.take_locks(args.lock_wait)
    host = logcat = None
    routed = False
    result = {'tag': tag, 'event': args.event, 'hold': args.hold, 'engine': args.engine}
    try:
        fec.installed_host_idle()
        fec.sh('input keyevent KEYCODE_WAKEUP'); fec.sh('svc power stayon true'); fec.sh('wm dismiss-keyguard', check=False)
        fec.sh(f'am force-stop {fec.PACKAGE}')
        set_prefs(args.engine)
        host = fec.start_host(tag)
        fec.adb('logcat', '-c')
        logcat = subprocess.Popen([fec.ADB, '-s', fec.SERIAL, 'logcat', '-v', 'epoch'],
                                  stdout=open(OUT / f'{tag}-logcat.txt', 'w', encoding='utf-8', errors='replace'),
                                  stderr=subprocess.DEVNULL)
        fec.sh(f'am start -n {fec.PACKAGE}/com.limelight.ShortcutTrampoline --es Name njrf --es AppUuid {fec.APP_UUID} '
               '--es AppName Desktop --ez HDR false')
        time.sleep(args.warmup)
        before = focus()
        result['focus_before'] = before
        (OUT / f'{tag}-mid.png').write_bytes(fec.adb('exec-out', 'screencap', '-p', binary=True))
        offset = device_time_offset()
        t0 = time.time()
        if args.event == 'kill':
            subprocess.run(['taskkill', '/F', '/PID', str(host_pid())], check=True, capture_output=True)
        elif args.event == 'stop':
            ctrl_c(host_pid())
        elif args.event in ('path', 'pathblip'):
            blackhole(True); routed = True
        elif args.event == 'wifi':
            fec.sh('svc wifi disable')
        elif args.event == 'suspend':
            suspend(host_pid(), True)
        if args.hold:
            time.sleep(args.hold)
            if args.event == 'pathblip':
                blackhole(False); routed = False
            elif args.event == 'wifi':
                fec.sh('svc wifi enable')
            elif args.event == 'suspend':
                suspend(host_pid(), False)
        # What the screen shows, over time, until the watch ends.
        trace = []
        end = t0 + args.watch
        while time.time() < end:
            now = focus()
            if not trace or trace[-1][1] != now:
                trace.append((round(time.time() - t0, 2), now))
            # The answer is in: the stream ended and the app is back on its own screen.
            if args.event in ('kill', 'stop', 'path') and 'Game' not in now and trace[-1][0] > 0.5:
                time.sleep(1)  # The message fades in.
                break
            time.sleep(0.25)
        result['focus_trace'] = trace
        (OUT / f'{tag}-end.png').write_bytes(fec.adb('exec-out', 'screencap', '-p', binary=True))
    finally:
        if routed:
            blackhole(False)
        if args.event == 'wifi':
            fec.sh('svc wifi enable', check=False)
        if logcat is not None:
            logcat.terminate()
        fec.sh(f'am force-stop {fec.PACKAGE}', check=False)
        if host is not None:
            fec.stop_host(host)
        if (fec.PROFILE / 'logs' / 'butterpollo.log').exists():
            shutil.copy(fec.PROFILE / 'logs' / 'butterpollo.log', OUT / f'{tag}-host.log')
        fec.release_locks()
        fec.sh('svc power stayon false', check=False)
    text = (OUT / f'{tag}-logcat.txt').read_text(encoding='utf-8', errors='replace')
    result['marks'], result['recovery_lines'] = analyse(text, t0, offset)
    with open(OUT / 'results.jsonl', 'a') as f:
        f.write(json.dumps(result) + '\n')
    print(json.dumps(result), flush=True)


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest='cmd', required=True)
    r = sub.add_parser('run')
    r.add_argument('--event', required=True, choices=['kill', 'stop', 'path', 'pathblip', 'wifi', 'suspend'])
    r.add_argument('--hold', type=float, default=0)
    r.add_argument('--engine', choices=['rust', 'classic'])
    r.add_argument('--tag')
    r.add_argument('--warmup', type=float, default=15)
    r.add_argument('--watch', type=float, default=40)
    r.add_argument('--lock-wait', type=float, default=600)
    args = p.parse_args()
    run(args)


if __name__ == '__main__':
    main()
