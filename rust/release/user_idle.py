"""How long the user has been away from this PC, not counting what our own tests do to it.

usage: python rust/release/user_idle.py              # prints the idle time
       python rust/release/user_idle.py begin        # prints START BEFORE for `end`
       python rust/release/user_idle.py end START [BEFORE]

Windows' last-input time (GetLastInputInfo) is not only the user's. A host that starts a stream
sets up its input devices and plugs in its virtual mouse, and Windows counts that as input: the
timer was reset within 10 ms of `CLIENT CONNECTED` in an e2e.py stream with nobody at the PC.
An install over the running host resets it too. So after any test the PC looked in use for as
long as the idle rule asks, and the next test was held back or skipped.

A test therefore marks the time it runs (`begin`, `end`; e2e.py, tools/hw/run.py and check.ps1's
install do), together with the user's last input as it stood when the test began. An input Windows reports from inside
a marked time is the test's, and the earlier one is the user's. What this cannot see is the user
touching the PC during a test: that input counts as the test's until the next one after it.

`end START` without BEFORE marks a time after the fact, for a step that did not call `begin`.
Nothing is known then about the user before it, so the user counts as present until START.
"""
import argparse, atexit, ctypes, json, os, pathlib, sys, time

# Beside the locks, and moved with them (tools/build_lock.py).
STATE = pathlib.Path(os.environ.get('RUBYLIGHT_LOCK_DIRECTORY') or r'C:\src\butterpollo-release') / 'USER-IDLE.json'
# Input this long after a marked time still counts as the test's: a host leaving unplugs its devices.
MARGIN = 5.0
KEEP = 2 * 86400


def system_last_input(now=None):
    """When Windows last saw input, as seconds since the epoch."""
    class LastInput(ctypes.Structure):
        _fields_ = [('cbSize', ctypes.c_uint), ('dwTime', ctypes.c_uint)]
    info = LastInput(ctypes.sizeof(LastInput), 0)
    if not ctypes.windll.user32.GetLastInputInfo(ctypes.byref(info)):
        raise OSError('GetLastInputInfo failed')
    idle = ((ctypes.windll.kernel32.GetTickCount() - info.dwTime) & 0xFFFFFFFF) / 1000
    return (time.time() if now is None else now) - idle


def load(path=STATE):
    try:
        marks = json.loads(pathlib.Path(path).read_text(encoding='utf-8'))
        return [m for m in marks if all(isinstance(m.get(k), (int, float)) for k in ('start', 'end', 'before'))]
    except (OSError, ValueError, AttributeError, TypeError):
        return []


def save(marks, path=STATE, now=None):
    now = time.time() if now is None else now
    path = pathlib.Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    scratch = path.with_name(f'{path.name}.{os.getpid()}')
    scratch.write_text(json.dumps([m for m in marks if m['end'] >= now - KEEP]), encoding='utf-8')
    os.replace(scratch, path)


def user_last_input(last, marks):
    """The user's last input, given Windows' `last` and the marked times."""
    for _ in range(len(marks) + 1):
        inside = [m for m in marks if m['start'] < last <= m['end'] + MARGIN]
        if not inside:
            break
        # Never later than the time it explains, so overlapping marks cannot loop.
        last = min(min(m['before'] for m in inside), min(m['start'] for m in inside))
    return last


def begin(path=STATE, now=None, last=None):
    """Call before a test touches the PC; hand the result to `end`."""
    now = time.time() if now is None else now
    last = system_last_input(now) if last is None else last
    return dict(start=now, before=user_last_input(last, load(path)))


def end(mark, path=STATE, now=None):
    now = time.time() if now is None else now
    save(load(path) + [dict(mark, end=now)], path, now)


def mark_this_process(path=STATE):
    """Marks from now until this process exits. A test never fails over it."""
    try:
        mark = begin(path)
    except OSError:
        return

    def finish():
        try:
            end(mark, path)
        except OSError:
            pass
    atexit.register(finish)


def idle(path=STATE, now=None, last=None):
    """(seconds the user has been away, seconds Windows reports)."""
    now = time.time() if now is None else now
    last = system_last_input(now) if last is None else last
    return max(0.0, now - user_last_input(last, load(path))), max(0.0, now - last)


def describe(away, reported):
    text = f'no input for {away / 60:.1f} min'
    if away - reported > 1:
        text += f' (Windows says {reported / 60:.1f} min: its last input came from a test)'
    return text


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    sub = parser.add_subparsers(dest='command')
    sub.add_parser('begin', help='print START BEFORE for a test that is about to touch the PC')
    done = sub.add_parser('end', help='mark the time since START as a test')
    done.add_argument('start', type=float, metavar='START')
    done.add_argument('before', type=float, nargs='?', metavar='BEFORE')
    args = parser.parse_args(argv)
    if args.command == 'begin':
        mark = begin()
        print(f"{mark['start']:.3f} {mark['before']:.3f}")
    elif args.command == 'end':
        end(dict(start=args.start, before=args.start if args.before is None else min(args.before, args.start)))
    else:
        print(describe(*idle()))
    return 0


if __name__ == '__main__':
    sys.exit(main())
