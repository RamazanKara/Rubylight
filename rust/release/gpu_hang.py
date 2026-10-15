"""Stop a test stream at the first sign of an AMD video-engine hang.

On the RX 7900 XT the driver's video encoder sometimes stops answering
inside an encode call. Windows resets the engine about two seconds later
(LiveKernelEvent 141), and on drivers 32.0.31041.1004 and 32.0.32015.2008
every such reset so far went on to a failed adapter reset (live dumps 0x193
and 0x1B0) with the Radeon in Code 31 until a reboot.

Stopping the stream does not prevent that. The work already queued on the
engine is what hangs, and a thread blocked in the driver call cannot leave
it before the reset; two October 10 runs ended within about two seconds of
the stall and still reset and stayed in Code 31. Releasing the AMF context
from another thread while SubmitInput is blocked would race the call on the
same component, and AMF Terminate itself waited 6 s on the reset GPU. What
an early stop does give: the run ends with its evidence and a named cause,
and no retry or later case runs on a GPU that is resetting.

Evidence, any one of:
- an encoder call that took two seconds or more (`slow encoder call`); the
  hung calls took 2950 ms to 34 s, healthy ones at most about 0.2 s outside
  AMF Init (under 1 s);
- the stall watch reporting the session thread stopped while encoding, with
  no resume within STALL_GRACE seconds of seeing it (it reports at 0.5 s).
  A healthy AV1 stream on October 15 waited 3.6 s for its first frame and
  went on without a reset, so a stall counts only from 5 s;
- a new or changed live kernel report from the display driver watchdog,
  which Windows writes a few seconds after the engine timeout.
"""
import pathlib
import re
import time

LIVE_KERNEL_REPORTS = pathlib.Path(r'C:\Windows\LiveKernelReports')
# Exit code of e2e.py when it stopped at a hang: check.ps1 must not retry.
EXIT_GPU_HANG = 3
SLOW_CALL_MS = 2000
STALL_GRACE = 4.5
ENCODER_PHASES = ('encoding', 'collecting encoder output')

_slow = re.compile(r'slow encoder call call="(?P<call>[^"]+)" elapsed_ms=(?P<ms>\d+)')
_stall = re.compile(r'stream thread (?P<event>stopped making progress|resumed after a stall) '
                    r'thread="(?P<thread>[^"]+)" stream_id=(?P<stream>\S+) phase="(?P<phase>[^"]+)"')


def _reports(root):
    """Display driver live dumps (WATCHDOG, AMD_*) by path, or None if unreadable."""
    try:
        reports = {}
        for path in root.rglob('*.dmp'):
            folder = path.parent.name.upper()
            if 'WATCHDOG' in folder or folder.startswith('AMD'):
                stat = path.stat()
                reports[str(path)] = (stat.st_size, stat.st_mtime_ns)
        return reports
    except OSError:
        # An unprivileged shell may not list the folder; the log still counts.
        return None


class Scanner:
    """Hang evidence in host log lines; pure, so a test drives the clock."""

    def __init__(self):
        self.evidence = None
        self.stalls = {}

    def feed(self, line, now):
        if self.evidence:
            return
        slow = _slow.search(line)
        if slow and int(slow['ms']) >= SLOW_CALL_MS:
            self.evidence = f"encoder call {slow['call']} took {slow['ms']} ms: {line.strip()}"
            return
        stall = _stall.search(line)
        if stall:
            key = (stall['thread'], stall['stream'])
            if stall['event'] == 'resumed after a stall':
                self.stalls.pop(key, None)
            elif stall['phase'] in ENCODER_PHASES:
                self.stalls.setdefault(key, (now, line.strip()))

    def check(self, now):
        if not self.evidence:
            for seen, line in self.stalls.values():
                if now - seen >= STALL_GRACE:
                    self.evidence = f'encoder stalled and did not resume within {STALL_GRACE:g} s: {line}'
                    break
        return self.evidence


class Guard:
    """Polls one host log, from its size at start, and the watchdog reports."""

    def __init__(self, host_log, reports=LIVE_KERNEL_REPORTS, clock=time.monotonic):
        self.host_log = pathlib.Path(host_log)
        self.offset = self.host_log.stat().st_size if self.host_log.exists() else 0
        self.reports = pathlib.Path(reports)
        self.before = _reports(self.reports)
        self.clock = clock
        self.partial = b''
        self.scanner = Scanner()

    def check(self):
        now = self.clock()
        if self.host_log.exists():
            with self.host_log.open('rb') as log:
                log.seek(self.offset)
                data = self.partial + log.read()
            self.offset += len(data) - len(self.partial)
            *lines, self.partial = data.split(b'\n')
            for line in lines:
                self.scanner.feed(line.decode(errors='replace'), now)
        evidence = self.scanner.check(now)
        if evidence:
            return evidence
        current = _reports(self.reports)
        if current is not None and self.before is None:
            # Unreadable at start: what exists now is the baseline.
            self.before = current
        elif current is not None:
            changed = sorted(path for path, signature in current.items() if self.before.get(path) != signature)
            if changed:
                self.scanner.evidence = 'new display driver watchdog report: ' + ', '.join(changed)
        return self.scanner.evidence


def hang_result(case, codec, mode, vrr, evidence):
    return dict(case=case, codec=codec, mode=mode, vrr=vrr, passed=False, gpu_hang=evidence,
                failures=[f'stopped at an AMD video engine hang: {evidence}. Do not retry: reboot '
                          'if Device Manager shows the Radeon with Code 31, then run the check again'])
