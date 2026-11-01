"""Writes rust/core/src/nucleus/release-2.4.2.txt from Nucleus Co-op's release
archive: each file as `<sha256> <size> <path>` and each empty folder as
`<path>\\`, `\\`-separated as it unpacks, in the archive's order. Run once per
pinned release, with the archive downloaded from its release page:

    python -I tools/nucleus_manifest.py NucleusApp.zip > rust/core/src/nucleus/release-2.4.2.txt

The archive is only read, never unpacked. Its entries are encrypted with the
password SplitScreen.Me publishes, "nucleus".
"""
import hashlib
import sys
import zipfile


def main(archive):
    lines = []
    with zipfile.ZipFile(archive) as zf:
        zf.setpassword(b'nucleus')
        files = {info.filename.rstrip('/') for info in zf.infolist() if not info.is_dir()}
        for info in zf.infolist():
            path = info.filename.replace('/', '\\')
            if info.is_dir():
                # Only empty folders: the others come with their files.
                inside = info.filename
                if not any(name.startswith(inside) for name in files):
                    lines.append(path if path.endswith('\\') else path + '\\')
                continue
            digest = hashlib.sha256()
            with zf.open(info) as f:
                for chunk in iter(lambda: f.read(1 << 20), b''):
                    digest.update(chunk)
            lines.append(f'{digest.hexdigest()} {info.file_size} {path}')
    sys.stdout.buffer.write(('\n'.join(lines) + '\n').encode('utf-8'))


if __name__ == '__main__':
    main(sys.argv[1])
