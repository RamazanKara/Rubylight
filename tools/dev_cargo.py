"""`dev.py cargo`: one hand-run Cargo command in a warm target slot, leased and claimed as in `dev.py check`.

usage: python tools/dev.py cargo [--client] [--fast] [--label L] [--target-dir DIR] -- <cargo args...>

Paths: wrapper options and --manifest-path are relative to the checkout root; in
client mode Cargo runs in client/protocol, so Cargo's other relative path
arguments resolve there.

A client command (--client, or a --manifest-path below client/) takes a client
slot and runs in Nightjar's workspace; any other takes a host slot and runs in
the checkout root. A --manifest-path below rust/service builds as `check` builds
the service: in <slot>/service, without the GNU/lld flags.
"""
import argparse, pathlib, shutil, subprocess, sys

import build_lock, dev


# Cargo subcommands (and cargo-nextest subcommands) that accept `--target-dir <DIR>`
# before any `--` separator. Passing `--target-dir` on the CLI instead of setting
# CARGO_TARGET_DIR keeps sccache's Rust cache keys identical across target-pool slots.
TARGET_DIR_SUBCOMMANDS = frozenset({
    'build', 'b', 'check', 'c', 'test', 't', 'bench', 'run', 'r',
    'clippy', 'doc', 'd', 'rustc', 'clean',
})
NEXTEST_TARGET_DIR_SUBCOMMANDS = frozenset({'run', 'list', 'archive'})
GLOBAL_VALUED_FLAGS = frozenset({'--color', '--config', '-Z', '-C'})
NEXTEST_GLOBAL_VALUED_FLAGS = frozenset({'--color', '--config-file', '--profile', '--tool-config-file'})


def inject_target_dir(cargo_args, target):
    """Inject `--target-dir <target>` before `--` for supported Cargo subcommands.

    Returns `(args, uses_flag)` where `uses_flag` is True when Cargo receives the target
    directory via CLI flag (injected here or already supplied), so `CARGO_TARGET_DIR` can
    stay unset in the child environment for cross-slot sccache sharing.
    """
    args = list(cargo_args)
    sep = args.index('--') if '--' in args else len(args)
    before, after = args[:sep], args[sep:]
    if any(a == '--target-dir' or a.startswith('--target-dir=') for a in before):
        return args, True
    if any(a in ('-h', '--help', '-V', '--version') for a in before):
        return args, False
    index = 0
    if index < len(before) and before[index].startswith('+'):
        index += 1
    while index < len(before) and before[index].startswith('-'):
        flag = before[index]
        index += 1
        if flag in GLOBAL_VALUED_FLAGS and index < len(before):
            index += 1
    if index >= len(before):
        return args, False
    sub = before[index]
    if sub == 'nextest':
        index += 1
        while index < len(before) and before[index].startswith('-'):
            flag = before[index]
            index += 1
            if flag in NEXTEST_GLOBAL_VALUED_FLAGS and index < len(before):
                index += 1
        if index >= len(before) or before[index] not in NEXTEST_TARGET_DIR_SUBCOMMANDS:
            return args, False
    elif sub not in TARGET_DIR_SUBCOMMANDS:
        return args, False
    return [*before, '--target-dir', str(target), *after], True


def manifest_paths(cargo_args, root):
    """(index, prefix, resolved path) of each --manifest-path among Cargo's own arguments."""
    found = []
    for index, arg in enumerate(cargo_args):
        if arg == '--':  # the rest is for the test binary or the tool
            break
        if arg == '--manifest-path' and index + 1 < len(cargo_args):
            found.append((index + 1, '', (root / cargo_args[index + 1]).resolve()))
        elif arg.startswith('--manifest-path='):
            found.append((index, '--manifest-path=', (root / arg.split('=', 1)[1]).resolve()))
    return found


def main(argv, root=None):
    root = pathlib.Path(root or dev.ROOT).resolve()
    parser = argparse.ArgumentParser(prog='dev.py cargo', description=__doc__.split('\n')[0])
    parser.add_argument('--client', action='store_true', help="Nightjar's workspace and a client slot")
    parser.add_argument('--fast', action='store_true', help='allow untranslated Nightjar strings, as check --fast')
    parser.add_argument('--label', help='name shown to those waiting for the lanes')
    parser.add_argument('--target-dir', type=pathlib.Path, help='use this folder instead of a pool slot')
    parser.add_argument('--lock-directory', type=pathlib.Path, help='shared machine lock directory')
    parser.add_argument('--lock-timeout', type=float, default=1800, help='maximum build wait in seconds')
    parser.add_argument('--pool', type=pathlib.Path, help='warm target pool')
    split = argv.index('--') if '--' in argv else len(argv)
    args = parser.parse_args(argv[:split])
    cargo_args = list(argv[split + 1:])
    if not cargo_args:
        parser.error('provide the cargo arguments after --')
    if args.lock_timeout < 0:
        parser.error('--lock-timeout must be nonnegative')
    for name in ('target_dir', 'pool', 'lock_directory'):
        if getattr(args, name):
            setattr(args, name, (root / getattr(args, name)).resolve())
    manifests = manifest_paths(cargo_args, root)
    service = any(path.is_relative_to(root / dev.SERVICE_WORKSPACE) for _, _, path in manifests)
    client = not service and (args.client or any(path.is_relative_to(root / 'client') for _, _, path in manifests))
    cwd = root
    if client:
        # Cargo reads .cargo/config.toml from its working folder, not from the manifest's.
        cwd = root / dev.CLIENT_WORKSPACE
        for index, prefix, path in manifests:
            cargo_args[index] = f'{prefix}{path}'
    check = dev.Check('cargo', 'service' if service else 'client' if client else 'host', [])
    label = args.label or f'dev cargo {root}'

    def say(*a, **k):
        print(*a, file=sys.stderr, **k)  # stdout is Cargo's

    build_lock.end_children_with_me()
    try:
        with build_lock.StageLease(dev.stage_groups(check, root, args), args.lock_directory, label,
                                   args.lock_timeout, say=say) as lease:
            target = dev.stage_target(check, lease.taken, root, args.target_dir)
            effective_target = pathlib.Path(target) / 'service' if service else pathlib.Path(target)
            cargo_args, uses_target_dir_flag = inject_target_dir(cargo_args, effective_target)
            set_target_dir_env = not uses_target_dir_flag
            if service:
                env = dev.service_env(target, fast=args.fast, lock_directory=args.lock_directory,
                                      set_target_dir_env=set_target_dir_env)
            elif client:
                env = dev.client_env(target, fast=args.fast, lock_directory=args.lock_directory,
                                     set_target_dir_env=set_target_dir_env)
            else:
                env = dev.host_env(target, lock_directory=args.lock_directory, fast=args.fast,
                                   set_target_dir_env=set_target_dir_env)
            say(f'dev cargo: target {effective_target}', flush=True)
            # Windows looks the program up on this process's PATH, not env's.
            program = shutil.which('cargo', path=env.get('PATH'))
            if not program:
                say('cargo is not on PATH')
                return 127
            return build_lock.wait_child(subprocess.Popen([program, *cargo_args], cwd=cwd, env=env))
    except KeyboardInterrupt:
        return 130
    except (OSError, RuntimeError) as error:
        say(str(error))
        return 1
