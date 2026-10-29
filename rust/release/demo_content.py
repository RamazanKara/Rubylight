"""A looped real game clip as the stream's picture (e2e.py --content), instead of motion_probe's strip.

A real game is slow to launch, shows repeated desktop frames for its first
10-15 seconds and plays different content every run. The clip (raw Y4M, such
as Xiph's Twitch set: 1920x1080, 60 FPS, 4:2:0) plays in a borderless ffplay
window the size of the stream, and the test host captures that one window
with WGC (output_name = window:<hwnd>@<display>), so it needs no part of the
PC's monitor: the window sits at the bottom of the z-order, behind the
user's windows. Raw Y4M needs no decoder, so playback takes no time on the
video engine the encoder uses (hardware decode of an H.264 or HEVC copy
would); ffplay only uploads the YUV planes and presents.

The window opens paused on the clip's first picture and plays from there when
the stream starts (resume), so every run streams the same frames. ffplay
runs on its video clock and shows every picture, so a player that cannot keep
up falls behind the clip: the share of real time it kept and its CPU go into
result.json as content, and under 99 % fails the case.
"""
import ctypes
import os
import re
import subprocess
import time
from ctypes import wintypes

TITLE = 'rubylight-demo-content'
# The status line ffplay rewrites about every 30 ms: "  12.34 fd=   0 aq=    0KB vq=...".
_STATUS = re.compile(r'(\d+\.\d+)\s+(?:M-V:\s*\S+\s+)?fd=\s*(\d+)')


def y4m_header(data):
    """The stream header of a YUV4MPEG2 file's first bytes: width, height, fps and chroma."""
    line = data.split(b'\n', 1)[0].decode('ascii', 'replace')
    fields = line.split(' ')
    if fields[0] != 'YUV4MPEG2':
        raise ValueError('not a YUV4MPEG2 file')
    tags = {field[0]: field[1:] for field in fields[1:] if field}
    if 'W' not in tags or 'H' not in tags or 'F' not in tags:
        raise ValueError(f'YUV4MPEG2 header without size or rate: {line!r}')
    num, den = (int(part) for part in tags['F'].split(':'))
    return dict(width=int(tags['W']), height=int(tags['H']), fps=num / den, chroma=tags.get('C', '420jpeg'))


def read_header(clip):
    with open(clip, 'rb') as f:
        return y4m_header(f.read(256))


def check_clip(header, width, height, fps):
    """Why the clip cannot be the stream's picture at this mode, or None.
    The window is the stream's size, so a scaled clip would measure ffplay's
    scaler; a clip slower than the stream repeats pictures by design."""
    if (header['width'], header['height']) != (width, height):
        return f"the clip is {header['width']}x{header['height']}, the stream {width}x{height}"
    if header['fps'] < fps * .99:
        return f"the clip plays at {header['fps']:g} FPS, below the stream's {fps:g}"
    if not header['chroma'].startswith('420'):
        return f"the clip's chroma is {header['chroma']}, not 4:2:0"
    return None


def player_command(ffplay, clip, width, height, left, top):
    """ffplay looping the clip in a borderless window of exactly the stream's size, no audio."""
    return [str(ffplay), '-hide_banner', '-loglevel', 'info', '-stats', '-an', '-sn', '-noborder',
            '-window_title', TITLE, '-left', str(left), '-top', str(top), '-x', str(width), '-y', str(height),
            '-loop', '0', '-sync', 'video', '-i', str(clip)]


def player_environment(base=None):
    """SDL's Direct3D 11 renderer presenting on the monitor's vblank (both can be overridden).
    On the 240 Hz host a 30 s HEVC arm had 9-22 long intervals of 1620 with it (48-90 % CPU
    from other lanes) against 25-34 with SDL's default renderer, with or without vsync
    (87-99 %), at about half the player CPU; the default renderer with vsync also fell up to
    4 % behind the clip."""
    environment = dict(os.environ if base is None else base)
    environment.setdefault('SDL_RENDER_DRIVER', 'direct3d11')
    environment.setdefault('SDL_RENDER_VSYNC', '1')
    return environment


def player_stats(text):
    """ffplay's last status: seconds played and pictures dropped (fd=), or None before the first."""
    found = _STATUS.findall(text.replace('\r', '\n'))
    if not found:
        return None
    played, dropped = found[-1]
    return dict(played_seconds=float(played), dropped=int(dropped))


def clip_seconds(header, size):
    """The clip's length from its file size: each picture is FRAME\\n and its 4:2:0 planes."""
    frame = 6 + header['width'] * header['height'] * 3 // 2
    return (size // frame) / header['fps']


def content_verdict(stats, cpu_percent, elapsed, length, at_resume=0.0):
    """The playback's own record for result.json. ffplay shows every picture (video clock, no
    frame dropping), so a player that cannot keep up falls behind the clip instead: under
    99 % of real time fails the case, which would otherwise measure the player, not the host.
    Past the clip's end the clock starts over and real time is not measured (None)."""
    if stats is None:
        return dict(passed=False, error='ffplay reported no playback status', player_cpu_percent=cpu_percent)
    played = stats['played_seconds'] - at_resume
    realtime = round(played / elapsed, 4) if elapsed and elapsed < length - at_resume else None
    return dict(passed=realtime is None or realtime >= .99, realtime=realtime, played_seconds=round(played, 2),
                elapsed_seconds=round(elapsed, 2), dropped=stats['dropped'], player_cpu_percent=cpu_percent)


_user32 = ctypes.WinDLL('user32', use_last_error=True)
_kernel32 = ctypes.WinDLL('kernel32', use_last_error=True)
_ENUM = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
_user32.EnumWindows.argtypes = [_ENUM, wintypes.LPARAM]
_user32.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]
_user32.IsWindowVisible.argtypes = [wintypes.HWND]
_user32.GetWindowTextW.argtypes = [wintypes.HWND, wintypes.LPWSTR, ctypes.c_int]
_user32.SetWindowPos.argtypes = [wintypes.HWND, wintypes.HWND] + [ctypes.c_int] * 4 + [wintypes.UINT]
_user32.PostMessageW.argtypes = [wintypes.HWND, wintypes.UINT, wintypes.WPARAM, wintypes.LPARAM]
_kernel32.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
_kernel32.OpenProcess.restype = wintypes.HANDLE
_kernel32.GetProcessTimes.argtypes = [wintypes.HANDLE] + [ctypes.POINTER(ctypes.c_uint64)] * 4
_kernel32.CloseHandle.argtypes = [wintypes.HANDLE]


def find_window(pid, timeout=10):
    """The visible ffplay window of process pid, waiting for it to open."""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        found = []

        def each(hwnd, _):
            owner = wintypes.DWORD()
            _user32.GetWindowThreadProcessId(hwnd, ctypes.byref(owner))
            if owner.value == pid and _user32.IsWindowVisible(hwnd):
                title = ctypes.create_unicode_buffer(256)
                _user32.GetWindowTextW(hwnd, title, 256)
                if title.value == TITLE:
                    found.append(hwnd)
            return True
        _user32.EnumWindows(_ENUM(each), 0)
        if found:
            return found[0]
        time.sleep(.05)
    raise TimeoutError(f'ffplay (pid {pid}) opened no window within {timeout} s')


def send_to_back(hwnd):
    """Below every other window, unfocused: WGC still captures it, the user's windows stay on top."""
    HWND_BOTTOM, SWP_NOSIZE, SWP_NOMOVE, SWP_NOACTIVATE = 1, 0x1, 0x2, 0x10
    if not _user32.SetWindowPos(hwnd, HWND_BOTTOM, 0, 0, 0, 0, SWP_NOSIZE | SWP_NOMOVE | SWP_NOACTIVATE):
        raise ctypes.WinError(ctypes.get_last_error())


def toggle_pause(hwnd):
    """ffplay's pause key (space), posted to its window without focusing it."""
    WM_KEYDOWN, WM_KEYUP, VK_SPACE = 0x100, 0x101, 0x20
    scan = 0x39 << 16
    for message, flags in ((WM_KEYDOWN, 1 | scan), (WM_KEYUP, 1 | scan | 0xC0000000)):
        if not _user32.PostMessageW(hwnd, message, VK_SPACE, flags):
            raise ctypes.WinError(ctypes.get_last_error())


class _PowerThrottling(ctypes.Structure):
    _fields_ = [('version', wintypes.ULONG), ('control', wintypes.ULONG), ('state', wintypes.ULONG)]


_kernel32.SetProcessInformation.argtypes = [wintypes.HANDLE, ctypes.c_int, ctypes.c_void_p, wintypes.DWORD]


def keep_timer_resolution(pid):
    """Windows 11 ignores a process's 1 ms timer request once its windows are occluded, and
    ffplay's 1 ms sleeps between pictures become 15.6 ms ones: about 5 % of the pictures
    then came a frame late. Opting the player out of that throttling keeps it at 60 FPS
    behind other windows."""
    process = _kernel32.OpenProcess(0x0200, False, pid)  # PROCESS_SET_INFORMATION
    if not process:
        raise ctypes.WinError(ctypes.get_last_error())
    try:
        # ProcessPowerThrottling; IGNORE_TIMER_RESOLUTION (0x4) controlled and off; EXECUTION_SPEED (0x1) too.
        state = _PowerThrottling(1, 0x4 | 0x1, 0)
        if not _kernel32.SetProcessInformation(process, 4, ctypes.byref(state), ctypes.sizeof(state)):
            raise ctypes.WinError(ctypes.get_last_error())
        # Above other lanes' builds, as a game in the foreground would be.
        if not _kernel32.SetPriorityClass(process, 0x8000):  # ABOVE_NORMAL_PRIORITY_CLASS
            raise ctypes.WinError(ctypes.get_last_error())
    finally:
        _kernel32.CloseHandle(process)


def cpu_seconds(pid):
    """Kernel plus user time of process pid, in seconds."""
    process = _kernel32.OpenProcess(0x1000, False, pid)  # PROCESS_QUERY_LIMITED_INFORMATION
    if not process:
        raise ctypes.WinError(ctypes.get_last_error())
    try:
        times = [ctypes.c_uint64() for _ in range(4)]
        if not _kernel32.GetProcessTimes(process, *map(ctypes.byref, times)):
            raise ctypes.WinError(ctypes.get_last_error())
        return (times[2].value + times[3].value) / 1e7
    finally:
        _kernel32.CloseHandle(process)


class Player:
    """The clip's ffplay window, opened paused on its first picture."""

    def __init__(self, ffplay, clip, width, height, left, top, log):
        self.log = log
        self.length = clip_seconds(read_header(clip), os.path.getsize(clip))
        self.log_file = log.open('w')
        self.process = subprocess.Popen(player_command(ffplay, clip, width, height, left, top),
                                        stdout=subprocess.DEVNULL, stderr=self.log_file,
                                        stdin=subprocess.DEVNULL, env=player_environment())
        try:
            keep_timer_resolution(self.process.pid)
            self.hwnd = find_window(self.process.pid)
            toggle_pause(self.hwnd)
            send_to_back(self.hwnd)
        except BaseException:
            self.stop()
            raise
        self.started = None

    def target(self, display):
        return f'window:{self.hwnd}@{display}'

    def resume(self):
        stats = player_stats(self.log.read_text(errors='replace'))
        toggle_pause(self.hwnd)
        self.started = time.monotonic(), cpu_seconds(self.process.pid), stats['played_seconds'] if stats else 0.0

    def verdict(self):
        """Read before the player stops: its CPU (% of one core) since resume and ffplay's last status."""
        if not self.started or self.process.poll() is not None:
            return dict(passed=False, error=f'the player exited ({self.process.poll()}) or never played')
        began, used, at_resume = self.started
        elapsed = time.monotonic() - began
        cpu = round(100 * (cpu_seconds(self.process.pid) - used) / elapsed, 1)
        return content_verdict(player_stats(self.log.read_text(errors='replace')), cpu, elapsed, self.length, at_resume)

    def stop(self):
        if self.process.poll() is None:
            self.process.terminate()
        self.process.wait(timeout=10)
        self.log_file.close()


# The host's line when a stream's capture opens (stream.rs): the clip plays from there.
STREAM_STARTED = 'capture backend opened'


def serve(clip, display, out, host_log, seconds, at=(0, 0), ffplay='ffplay', wait=600):
    """For a device arm (tools/hw/demo-stream.ps1): open the clip paused, write its capture
    target to out/target.txt, play it once the host's log says a stream started, and after
    seconds write the player's verdict to out/content.json."""
    import json, pathlib
    out = pathlib.Path(out); out.mkdir(parents=True, exist_ok=True)
    header = read_header(clip)
    player = Player(ffplay, clip, header['width'], header['height'], *at, out / 'player.log')
    try:
        (out / 'target.txt').write_text(player.target(display))
        host_log = pathlib.Path(host_log)
        deadline = time.monotonic() + wait
        while not (host_log.exists() and STREAM_STARTED in host_log.read_text(errors='replace')):
            if time.monotonic() > deadline:
                raise TimeoutError(f'no stream started within {wait} s')
            time.sleep(.1)
        player.resume()
        time.sleep(seconds)
        verdict = dict(clip=str(clip), **player.verdict())
        (out / 'content.json').write_text(json.dumps(verdict, indent=2))
        print(json.dumps(verdict))
        return 0 if verdict['passed'] else 1
    finally:
        player.stop()


if __name__ == '__main__':
    import argparse
    parser = argparse.ArgumentParser(description=serve.__doc__)
    parser.add_argument('clip')
    parser.add_argument('--display', required=True, help=r'the display whose GPU captures the window, e.g. \\.\DISPLAY1')
    parser.add_argument('--out', required=True)
    parser.add_argument('--host-log', required=True, help="the test host's butterpollo.log, empty or new before the stream")
    parser.add_argument('--seconds', type=float, required=True)
    parser.add_argument('--at', default='0,0', metavar='X,Y')
    parser.add_argument('--ffplay', default='ffplay')
    parser.add_argument('--wait', type=float, default=600, help='seconds to wait for the stream to start')
    args = parser.parse_args()
    raise SystemExit(serve(args.clip, args.display, args.out, args.host_log, args.seconds,
                           tuple(map(int, args.at.split(','))), args.ffplay, args.wait))
