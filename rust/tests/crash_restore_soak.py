"""Windows scale soak for issue #129: connect and disconnect a virtual display
stream N times against an isolated test host and check after each cycle that
every physical display, the primary first, is back at the scale it had before.

Cycles listed in --crash-cycles kill the test host mid-stream instead
(TerminateProcess); its display watcher (`--display-watch`) must restore the
scales from display-recovery.json, which must end up empty, before the host
starts again. Run it on hardware, with the primary display at a scale other
than 100 %, never against the installed host:

    python rust/tests/crash_restore_soak.py --package <runtime dir> \
        --client <moonlight-client.exe> --work <empty dir> --cycles 10 \
        --expect-primary-scale 150 [--layout exclusive] [--crash-cycles 4,7]

Exit codes: 0 every cycle restored the scales, 1 a cycle did not,
3 an AMD encoder or display driver hang (gpu_hang.py): investigate, never retry.
"""
import argparse, hashlib, json, os, pathlib, subprocess, sys, time

RELEASE = pathlib.Path(__file__).resolve().parents[1] / 'release'
INTEROP = pathlib.Path(__file__).resolve().parent / 'interop.py'


def read_scales():
    """Every active display's Windows scale, by monitor interface path, read
    per-monitor DPI aware. Runs in a fresh process for each read."""
    import ctypes
    from ctypes import wintypes
    user32, shcore = ctypes.WinDLL('user32'), ctypes.WinDLL('shcore')
    user32.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))

    class MonitorInfo(ctypes.Structure):
        _fields_ = [('cbSize', wintypes.DWORD), ('rcMonitor', wintypes.RECT), ('rcWork', wintypes.RECT),
                    ('dwFlags', wintypes.DWORD), ('szDevice', wintypes.WCHAR * 32)]

    class DisplayDevice(ctypes.Structure):
        _fields_ = [('cb', wintypes.DWORD), ('DeviceName', wintypes.WCHAR * 32), ('DeviceString', wintypes.WCHAR * 128),
                    ('StateFlags', wintypes.DWORD), ('DeviceID', wintypes.WCHAR * 128), ('DeviceKey', wintypes.WCHAR * 128)]

    found = {}

    def visit(monitor, _dc, _rect, _data):
        info = MonitorInfo(); info.cbSize = ctypes.sizeof(info)
        if not user32.GetMonitorInfoW(monitor, ctypes.byref(info)):
            return True
        device = DisplayDevice(); device.cb = ctypes.sizeof(device)
        # EDD_GET_DEVICE_INTERFACE_NAME: the monitor's path, stable across layouts.
        user32.EnumDisplayDevicesW(info.szDevice, 0, ctypes.byref(device), 1)
        x, y = wintypes.UINT(), wintypes.UINT()
        if shcore.GetDpiForMonitor(monitor, 0, ctypes.byref(x), ctypes.byref(y)) == 0:
            path = device.DeviceID or info.szDevice
            found[path] = dict(output=info.szDevice, scale=round(x.value * 100 / 96),
                               primary=bool(info.dwFlags & 1), virtual='#RBY' in path.upper())
        return True

    callback = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HMONITOR, wintypes.HDC,
                                  ctypes.POINTER(wintypes.RECT), wintypes.LPARAM)(visit)
    user32.EnumDisplayMonitors(None, None, callback, 0)
    return found


def scales():
    return json.loads(subprocess.check_output([sys.executable, '-I', __file__, '--read-scales'], text=True))


def physical(found):
    return {path: d['scale'] for path, d in found.items() if not d['virtual']}


def differences(before, now):
    """Physical displays whose scale is not the one before; a display that is
    off now (exclusive layout still restoring) counts as a difference."""
    return {path: (scale, now.get(path)) for path, scale in before.items() if now.get(path) != scale}


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--read-scales', action='store_true', help=argparse.SUPPRESS)
    parser.add_argument('--package', type=pathlib.Path, help='runtime directory with butterpollo.exe and assets')
    parser.add_argument('--client', type=pathlib.Path, help='moonlight-client.exe (rust/tests/build-moonlight-client.ps1)')
    parser.add_argument('--work', type=pathlib.Path, help='directory for this run; a new case folder goes inside')
    parser.add_argument('--cycles', type=int, default=10)
    parser.add_argument('--crash-cycles', default='', help='comma-separated cycles that kill the host mid-stream')
    parser.add_argument('--layout', default='exclusive',
                        choices=['exclusive', 'extended', 'extended_primary', 'extended_isolated', 'extended_primary_isolated'])
    parser.add_argument('--expect-primary-scale', type=int, help='fail at once unless the primary display starts at this scale')
    parser.add_argument('--seconds', type=int, default=8, help='stream length per cycle')
    parser.add_argument('--settle-seconds', type=float, default=6, help='how long the scales may take to come back')
    parser.add_argument('--mode', default='1280x720x60')
    parser.add_argument('--port', type=int, default=48623)
    args = parser.parse_args()
    if args.read_scales:
        print(json.dumps(read_scales()))
        return 0
    for name in ('package', 'client', 'work'):
        if getattr(args, name) is None:
            parser.error(f'--{name.replace("_", "-")} is required')
    sys.path.insert(0, str(RELEASE))
    from e2e_host import installed_idle
    from gpu_hang import EXIT_GPU_HANG, Guard
    installed_idle()
    crash_cycles = {int(c) for c in args.crash_cycles.split(',') if c.strip()}
    package, client, work = args.package.resolve(strict=True), args.client.resolve(strict=True), args.work.resolve(strict=True)
    start_scales = scales()
    before = physical(start_scales)
    primary = next((p for p, d in start_scales.items() if d['primary'] and not d['virtual']), None)
    assert primary, 'no physical primary display: start from the desktop the user normally has'
    print('BEFORE', json.dumps(start_scales, indent=1), flush=True)
    if args.expect_primary_scale is not None:
        assert before[primary] == args.expect_primary_scale, \
            f'the primary display is at {before[primary]} %, not {args.expect_primary_scale} %'
    case = work / f'dpi-soak-{int(time.time())}'
    profile = case / 'config'; (profile / 'logs').mkdir(parents=True)
    (profile / 'sunshine.conf').write_text('\n'.join([
        f'port = {args.port}', 'encoder = amf', 'capture = wgc', 'virtual_display_mode = per_client',
        f'virtual_display_layout = {args.layout}', 'stream_audio = false', 'install_steam_audio_drivers = false',
        'upnp = false', 'enable_discovery = false', 'vulkan_hdr_layer = false', 'system_tray = false',
        'updates = off', '']))
    (profile / 'apps.json').write_text(json.dumps({'apps': [{'name': 'Desktop', 'cmd': ''}]}))
    salt = os.urandom(8).hex()  # interop.py's fixture account
    (profile / 'sunshine_state.json').write_text(json.dumps({'username': 'test', 'salt': salt, 'password': hashlib.sha256(
        ('rust-smoke-only' + salt).encode()).digest()[::-1].hex().upper()}))
    journal = profile / 'display-recovery.json'
    log = profile / 'logs' / 'butterpollo.log'
    env = dict(os.environ, PATH=str(package) + ';' + os.environ['PATH'], NO_PROXY='*',
               BUTTERPOLLO_TEST_PORT=str(args.port), BUTTERPOLLO_TEST_CLIENT_EXE=str(client))
    host, outputs, results = None, [], []

    def start_host():
        out = (case / f'host-{len(outputs)}.log').open('w'); outputs.append(out)
        process = subprocess.Popen([str(package / 'butterpollo.exe'), '--config-dir', str(profile), '--assets',
                                    str(package / 'assets/web'), '--port', str(args.port), '--bind', '127.0.0.1', '--no-tray'],
                                   cwd=package, env=env, stdout=out, stderr=subprocess.STDOUT,
                                   creationflags=subprocess.CREATE_NO_WINDOW)
        time.sleep(3)
        assert process.poll() is None, 'the test host exited at start; see its log'
        return process

    def journal_entries():
        try:
            return len(json.loads(journal.read_text()).get('entries', {}))
        except (OSError, ValueError):
            return 0

    width, height, fps = args.mode.split('x')
    guard = Guard(log)
    try:
        host = start_host()
        for cycle in range(1, args.cycles + 1):
            folder = case / f'cycle-{cycle:02}'; folder.mkdir()
            command = [sys.executable, str(INTEROP), str(folder), 'h264', width, height, fps, str(args.seconds)]
            crashed = cycle in crash_cycles
            with (folder / 'interop.log').open('w') as out:
                stream = subprocess.Popen(command, env=env, stdout=out, stderr=subprocess.STDOUT, cwd=folder)
                if crashed:
                    deadline = time.monotonic() + 30
                    while time.monotonic() < deadline and not any(d['virtual'] for d in scales().values()):
                        time.sleep(.5)
                    time.sleep(2)
                    host.kill()  # TerminateProcess: no teardown runs
                    host.wait(10)
                try:
                    stream.wait(args.seconds + 60)
                except subprocess.TimeoutExpired:
                    stream.kill(); stream.wait()
            evidence = guard.check()
            if evidence:
                print('HANG', evidence, flush=True)
                return EXIT_GPU_HANG
            # Windows can move a scale again as the virtual display leaves,
            # which after a kill may be a little after the host is gone.
            started = time.monotonic()
            while any(d['virtual'] for d in scales().values()) and time.monotonic() - started < 30:
                time.sleep(.5)
            gone_after = round(time.monotonic() - started, 2)
            started = time.monotonic()
            while True:
                now = physical(scales())
                wrong = differences(before, now)
                waiting = crashed and journal_entries() > 0
                if (not wrong and not waiting) or time.monotonic() - started > args.settle_seconds:
                    break
                time.sleep(.25)
            result = dict(cycle=cycle, crashed=crashed, stream_exit=stream.returncode,
                          virtual_display_gone_after_s=gone_after,
                          restored_after_s=round(time.monotonic() - started, 2),
                          wrong={p: {'before': b, 'now': n} for p, (b, n) in wrong.items()},
                          journal_entries=journal_entries(), primary_scale=now.get(primary))
            results.append(result)
            print('CYCLE', json.dumps(result), flush=True)
            if not crashed and stream.returncode != 0:
                print(f'cycle {cycle}: the stream failed; see {folder / "interop.log"}', flush=True)
                break
            if wrong or (crashed and result['journal_entries']):
                break
            if crashed:
                host = start_host()
                guard = Guard(log)
    finally:
        if host and host.poll() is None:
            host.terminate()
            try:
                host.wait(20)
            except subprocess.TimeoutExpired:
                host.kill()
        for out in outputs:
            out.close()
    passed = len(results) == args.cycles and all(
        not r['wrong'] and not r['journal_entries'] and (r['crashed'] or r['stream_exit'] == 0) for r in results)
    (case / 'result.json').write_text(json.dumps(dict(passed=passed, layout=args.layout, before=before,
                                                      primary=primary, cycles=results), indent=2))
    print('DPI SOAK', 'PASS' if passed else 'FAIL', case / 'result.json', flush=True)
    return 0 if passed else 1


if __name__ == '__main__':
    sys.exit(main())
