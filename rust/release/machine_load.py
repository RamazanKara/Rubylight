"""Wait for a quiet CPU before a timed stream, and measure the load during one.

The release check's streams are judged on frame gaps and fresh-picture
rates, and so are the soak's load cases. On a shared machine other lanes'
builds starved the fixtures: check.ps1 for rc.4 failed twice at 25-100 %
CPU (hevc-recovery and pyrowave-hdr-444 on delivery gaps, no new captured
picture for 50 ms) and passed on a quiet CPU. A failure under load
says nothing about the host, so the scripts wait for a quiet machine first
and each result records the load it ran under.

Quiet: the whole machine under THRESHOLD percent CPU over a WINDOW, and no
build tool (BUSY_TOOLS) using CPU in it. An idle Gradle or Kotlin daemon
(java) stays for hours and does not count; a new or working one does.

usage: machine_load.py [--threshold 30] [--window 5] [--timeout 900] [--out FILE]
Prints the verdict as JSON (and writes it to FILE); waits at most --timeout
seconds, then warns and goes on, the load recorded, rather than leaving the
release unchecked. check.ps1 spends one such budget over all its streams.
"""
import argparse
import ctypes
import json
import sys
import time
from ctypes import wintypes

BUSY_TOOLS = ('cargo', 'rustc', 'java', 'gradle', 'ninja')
THRESHOLD = 30
WINDOW = 5
TIMEOUT = 900
# A tool counts as working above this share of one core.
TOOL_ACTIVE_CORES = .05


def cpu_percent(before, after):
    """Whole-machine CPU between two (idle, kernel, user) readings; kernel time includes idle."""
    idle = after[0] - before[0]
    total = (after[1] - before[1]) + (after[2] - before[2])
    return 100 * (total - idle) / total if total > 0 else 0.0


def active_tools(before, after, seconds):
    """Names of build tools that used CPU between two tool_processes() readings.
    One that started in between counts too. One whose times cannot be read
    (another user's, or gone) counts only in the window it first appears:
    a protected idle process must not hold up every stream."""
    active = set()
    for pid, (name, used) in after.items():
        previous = before.get(pid)
        if previous is None or previous[0] != name:
            active.add(name)
        elif used is not None and previous[1] is not None and used - previous[1] > TOOL_ACTIVE_CORES * seconds * 1e7:
            active.add(name)
    return sorted(active)


def _kernel32():
    kernel32 = ctypes.WinDLL('kernel32', use_last_error=True)
    kernel32.GetSystemTimes.argtypes = [ctypes.POINTER(ctypes.c_uint64)] * 3
    kernel32.GetSystemTimes.restype = wintypes.BOOL
    kernel32.CreateToolhelp32Snapshot.argtypes = [wintypes.DWORD, wintypes.DWORD]
    kernel32.CreateToolhelp32Snapshot.restype = wintypes.HANDLE
    kernel32.Process32FirstW.argtypes = kernel32.Process32NextW.argtypes = [wintypes.HANDLE, ctypes.POINTER(_Entry)]
    kernel32.Process32FirstW.restype = kernel32.Process32NextW.restype = wintypes.BOOL
    kernel32.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    kernel32.OpenProcess.restype = wintypes.HANDLE
    kernel32.GetProcessTimes.argtypes = [wintypes.HANDLE] + [ctypes.POINTER(ctypes.c_uint64)] * 4
    kernel32.GetProcessTimes.restype = wintypes.BOOL
    kernel32.CloseHandle.argtypes = [wintypes.HANDLE]
    return kernel32


# PROCESSENTRY32W from tlhelp32.h.
class _Entry(ctypes.Structure):
    _fields_ = [('size', wintypes.DWORD), ('usage', wintypes.DWORD), ('pid', wintypes.DWORD),
                ('heap', ctypes.c_size_t), ('module', wintypes.DWORD), ('threads', wintypes.DWORD),
                ('parent', wintypes.DWORD), ('priority', wintypes.LONG), ('flags', wintypes.DWORD),
                ('exe', wintypes.WCHAR * 260)]


def system_times():
    kernel32 = _kernel32()
    times = [ctypes.c_uint64() for _ in range(3)]
    if not kernel32.GetSystemTimes(*map(ctypes.byref, times)):
        raise ctypes.WinError(ctypes.get_last_error())
    return tuple(t.value for t in times)


def tool_processes():
    """{pid: (tool name, kernel + user time in 100 ns, or None if unreadable)} for BUSY_TOOLS."""
    kernel32 = _kernel32()
    snapshot = kernel32.CreateToolhelp32Snapshot(0x2, 0)  # TH32CS_SNAPPROCESS
    if not snapshot or snapshot == wintypes.HANDLE(-1).value:
        raise ctypes.WinError(ctypes.get_last_error())
    found = {}
    try:
        entry = _Entry(size=ctypes.sizeof(_Entry))
        more = kernel32.Process32FirstW(snapshot, ctypes.byref(entry))
        while more:
            name = entry.exe.lower().removesuffix('.exe')
            if name in BUSY_TOOLS:
                used = None
                process = kernel32.OpenProcess(0x1000, False, entry.pid)  # PROCESS_QUERY_LIMITED_INFORMATION
                if process:
                    times = [ctypes.c_uint64() for _ in range(4)]
                    if kernel32.GetProcessTimes(process, *map(ctypes.byref, times)):
                        used = times[2].value + times[3].value
                    kernel32.CloseHandle(process)
                found[entry.pid] = (name, used)
            more = kernel32.Process32NextW(snapshot, ctypes.byref(entry))
    finally:
        kernel32.CloseHandle(snapshot)
    return found


def snapshot():
    return time.monotonic(), system_times(), tool_processes()


def load_since(start):
    """The machine's CPU and working build tools since snapshot(), for a result's machine_load."""
    began, times, tools = start
    seconds = time.monotonic() - began
    return dict(cpu_percent=round(cpu_percent(times, system_times()), 1),
                busy_tools=active_tools(tools, tool_processes(), seconds), seconds=round(seconds, 1))


def sample(window):
    start = snapshot()
    time.sleep(window)
    return load_since(start)


def wait_quiet(threshold=THRESHOLD, window=WINDOW, timeout=TIMEOUT, measure=sample, clock=time.monotonic,
               say=lambda text: print(text, file=sys.stderr, flush=True)):
    """Sample windows until one is quiet or timeout seconds passed; the last window's load, quiet or not."""
    began = clock()
    reported = None
    while True:
        load = measure(window)
        quiet = load['cpu_percent'] < threshold and not load['busy_tools']
        waited = clock() - began
        if quiet or waited >= timeout:
            if not quiet:
                say(f'WARNING: the machine stayed busy for {waited:.0f} s ({load["cpu_percent"]:.0f} % CPU'
                    f'{", " + ", ".join(load["busy_tools"]) if load["busy_tools"] else ""}); going on, the load is '
                    'recorded. A timing failure from here on may be the load, not the host.')
            return dict(load, quiet=quiet, threshold_percent=threshold, waited_seconds=round(waited, 1))
        busy = (round(load['cpu_percent'] / 10) * 10, tuple(load['busy_tools']))
        if busy != reported:
            say(f'waiting for a quiet CPU: {load["cpu_percent"]:.0f} % (under {threshold} % wanted)'
                + (f', working: {", ".join(load["busy_tools"])}' if load['busy_tools'] else ''))
            reported = busy


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__.split('\n\n')[0])
    parser.add_argument('--threshold', type=float, default=THRESHOLD, help='percent of the whole CPU')
    parser.add_argument('--window', type=float, default=WINDOW, help='seconds each measurement spans')
    parser.add_argument('--timeout', type=float, default=TIMEOUT, help='seconds to wait before going on under load')
    parser.add_argument('--out', help='also write the verdict here, for e2e.py --quiet-wait')
    args = parser.parse_args()
    verdict = json.dumps(wait_quiet(args.threshold, args.window, args.timeout))
    if args.out:
        with open(args.out, 'w', encoding='utf-8') as out:
            out.write(verdict)
    print(verdict)
