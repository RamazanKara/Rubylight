"""Collect the release-note fragments in rust/release/notes.d into rust/RELEASE_NOTES.md.

Writes LF line endings on any platform.

usage: python rust/release/fragments.py

Each workstream writes rust/release/notes.d/<topic>.md (see the README there)
instead of editing RELEASE_NOTES.md, so parallel branches never conflict on
it. This moves every fragment's bullets, in file name order, to the end of
"## Unreleased" (creating it above the newest "## New in") and deletes the
fragments. bump.py does the same before it opens the new version's section.
"""
import pathlib, re

ROOT = pathlib.Path(__file__).resolve().parents[2]
NOTES_D = 'rust/release/notes.d'


def read(directory):
    """The bullets of every fragment in directory, in file name order, and the files."""
    files = sorted(p for p in pathlib.Path(directory).glob('*.md') if p.name.lower() != 'readme.md')
    bullets = []
    for path in files:
        text = re.sub(r'(?s)<!--.*?-->', '', path.read_text(encoding='utf-8'))
        lines = [line.rstrip() for line in text.strip().split('\n')]
        # An optional "# Topic" title line.
        if lines and lines[0].startswith('# '):
            lines = lines[1:]
        lines = [line for line in lines if line.strip()]
        if not lines or not lines[0].startswith('- '):
            raise SystemExit(f'{path}: a fragment is a list of "- " bullets, and it has none')
        stray = [line for line in lines if not line.startswith('- ') and not line[:1].isspace()]
        if stray:
            raise SystemExit(f'{path}: only "- " bullets and their indented continuation lines, not {stray[0]!r}')
        bullets += lines
    return bullets, files


def merge(lines, bullets):
    """lines (RELEASE_NOTES.md split on \\n) with bullets added to the end of ## Unreleased."""
    if not bullets:
        return lines
    lines = list(lines)
    if '## Unreleased' in lines:
        start = lines.index('## Unreleased')
        end = next((i for i in range(start + 1, len(lines)) if lines[i].startswith('## ')), len(lines))
    else:
        start = end = next(i for i, line in enumerate(lines) if line.startswith('## New in '))
    pending = '\n'.join(lines[start + 1:end]).strip()
    body = (pending.split('\n') if pending else []) + bullets
    lines[start:end] = ['## Unreleased', '', *body, '']
    return lines


def main():
    bullets, files = read(ROOT / NOTES_D)
    if not files:
        print(f'no fragments in {NOTES_D}')
        return
    notes = ROOT / 'rust/RELEASE_NOTES.md'
    lines = merge(notes.read_text(encoding='utf-8').split('\n'), bullets)
    notes.write_text('\n'.join(lines), encoding='utf-8', newline='')
    for path in files:
        path.unlink()
    print(f'{len(files)} fragments moved to "## Unreleased" in rust/RELEASE_NOTES.md')


if __name__ == '__main__':
    main()
