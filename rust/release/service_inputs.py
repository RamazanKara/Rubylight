"""The Windows service's version rule.

butterpollo-service.exe (rust/service) stays the same, byte for byte, from
release to release, so that the reputation Windows and antivirus programs give
its file hash carries over. It carries its own version instead of the
release's: every change to what goes into it comes with a new version in
rust/service/Cargo.toml, and

    python rust/release/service_inputs.py --update

records that version with the SHA-256 of those files in
rust/service/inputs.txt. It refuses to record different files under a version
already recorded. test_release.py fails while the last line does not match;
run without --update, this script says why.
"""
import argparse, hashlib, pathlib, sys, tomllib

ROOT = pathlib.Path(__file__).resolve().parents[2]
SERVICE = 'rust/service'
RECORD = f'{SERVICE}/inputs.txt'
# What the service's executable is made from: its crate, build.ps1 included
# (the toolchain, linker and flags it names), the core files it compiles
# (#[path] in its sources), the shared version resource script and the icon.
# Not rust-toolchain.toml or .cargo/config.toml: build.ps1's +toolchain,
# linker and CARGO_ENCODED_RUSTFLAGS override them (test_release.py checks
# that the configuration sets nothing else).
INPUTS = (SERVICE, 'rust/core/src/update_recovery.rs', 'rust/core/src/update_files.rs',
          'rust/core/src/state/file.rs', 'rust/assets/version_info.rs', 'rust/assets/butterpollo.ico')
# The only Cargo configuration in the repository's .cargo/config.toml that the
# service's build may meet: settings build.ps1 overrides.
OVERRIDDEN_CONFIG = {('target', 'x86_64-pc-windows-gnu', 'linker')}


def config_keys(text):
    """The dotted keys a Cargo configuration file sets, as tuples."""
    def walk(table, prefix):
        for key, value in table.items():
            if isinstance(value, dict):
                yield from walk(value, prefix + (key,))
            else:
                yield prefix + (key,)
    return set(walk(tomllib.loads(text), ()))


def files(root):
    """The input files, relative and sorted: build output, the record itself and
    Markdown left out."""
    found = []
    for name in INPUTS:
        path = root / name
        if path.is_file():
            found.append(name)
            continue
        for file in path.rglob('*'):
            relative = file.relative_to(root).as_posix()
            if file.is_file() and 'target' not in file.relative_to(path).parts[:-1] \
                    and relative != RECORD and file.suffix != '.md':
                found.append(relative)
    return sorted(found)


def digest(root):
    """SHA-256 over every input's path and contents. Text is hashed with LF line
    ends: a Windows checkout may have CRLF, which changes nothing in the build."""
    sha = hashlib.sha256()
    for name in files(root):
        data = (root / name).read_bytes()
        if b'\0' not in data:
            data = data.replace(b'\r\n', b'\n')
        sha.update(name.encode() + b'\0' + len(data).to_bytes(8, 'little') + data)
    return sha.hexdigest()


def version(root):
    return tomllib.loads((root / SERVICE / 'Cargo.toml').read_text(encoding='utf-8'))['package']['version']


def recorded(root):
    """(version, digest) per line of the record, oldest first."""
    path = root / RECORD
    lines = path.read_text(encoding='utf-8').splitlines() if path.is_file() else []
    return [tuple(line.split()) for line in lines if line.strip() and not line.startswith('#')]


def problems(root=ROOT):
    """Why the record and the service's inputs disagree; empty when they agree."""
    found = []
    entries = recorded(root)
    versions = [entry[0] for entry in entries]
    for name in sorted({v for v in versions if versions.count(v) > 1}):
        found.append(f'{RECORD} records version {name} more than once')
    current = (version(root), digest(root))
    if not entries or entries[-1] != current:
        if current[0] in versions:
            found.append(f'the service\'s files changed but its version did not: bump the version in '
                         f'{SERVICE}/Cargo.toml, then run python rust/release/service_inputs.py --update')
        else:
            found.append(f'version {current[0]} of the service is not recorded: '
                         f'run python rust/release/service_inputs.py --update')
    return found


def update(root=ROOT):
    """Record the current version and digest. Returns an error message, or None."""
    current = (version(root), digest(root))
    entries = recorded(root)
    if entries and entries[-1] == current:
        return None
    if any(entry[0] == current[0] for entry in entries):
        return (f'version {current[0]} is already recorded for other files: bump the version in '
                f'{SERVICE}/Cargo.toml first')
    path = root / RECORD
    text = path.read_text(encoding='utf-8') if path.is_file() else (
        '# The service\'s version and the SHA-256 of its files (rust/release/service_inputs.py).\n')
    path.write_text(text + f'{current[0]} {current[1]}\n', encoding='utf-8', newline='')
    return None


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    parser.add_argument('--update', action='store_true', help='record the current version and files')
    args = parser.parse_args(argv)
    if args.update:
        error = update()
        if error:
            print(error, file=sys.stderr)
            return 1
    found = problems()
    for problem in found:
        print(problem, file=sys.stderr)
    if not found:
        print(f'service {version(ROOT)}: {digest(ROOT)}')
    return 1 if found else 0


if __name__ == '__main__':
    sys.exit(main())
