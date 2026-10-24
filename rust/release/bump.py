"""Bump the release version and open its release notes section.

Writes LF line endings on any platform.

usage: python rust/release/bump.py 3.0.0-alpha.1 [--notes FILE]

Updates the workspace version (Cargo.toml and the workspace crates in
Cargo.lock); the full version, "Download <label>" and "For <label>," in the
READMEs and docs; the title, history and installer names in
rust/RELEASE_NOTES.md; and adds a "## New in <label>" section above the
previous one: the contents of --notes, or the bullets to fill in.

The label is the version, except that 2.0.0 release candidates were "rc.N".
Alpha and beta versions keep their full version so that 3.0.0-alpha.1 and
3.1.0-alpha.1 get different sections.
"""
import argparse, pathlib, re, sys

parser = argparse.ArgumentParser()
parser.add_argument('version')
parser.add_argument('--notes', type=pathlib.Path)
args = parser.parse_args()
root = pathlib.Path(__file__).resolve().parents[2]
new = args.version.removeprefix('v')
VERSION = r'\d+\.\d+\.\d+(?:-(?:alpha|beta|rc)\.\d+)?'
assert re.fullmatch(VERSION, new), new


def label(version):
    m = re.fullmatch(r'2\.0\.0-(rc\.\d+)', version)
    return m[1] if m else version


def anchor(version):
    return '#new-in-' + label(version).replace('.', '')


cargo = root / 'Cargo.toml'
text = cargo.read_text(encoding='utf-8')
old = re.search(r'(?m)^version = "([^"]+)"', text)[1]
assert old != new, f'already {new}'
cargo.write_text(text.replace(f'version = "{old}"', f'version = "{new}"', 1), encoding='utf-8', newline='')

# Cargo.lock: only the workspace's own crates carry the workspace version.
members = set(re.findall(r'(?m)^name = "([^"]+)"', ''.join(
    p.read_text(encoding='utf-8') for p in root.glob('rust/*/Cargo.toml'))))
lock = root / 'Cargo.lock'
text, count = re.subn(r'(?m)^(name = "(?:%s)"\nversion = )"%s"$' % ('|'.join(map(re.escape, members)), re.escape(old)),
                      lambda m: f'{m[1]}"{new}"', lock.read_text(encoding='utf-8'))
assert count, 'no workspace crate in Cargo.lock'
lock.write_text(text, encoding='utf-8', newline='')

short = re.compile(r'(?<![\w.])%s(?![\w])' % re.escape(label(old)))
# Only the short labels that name the release to download: a sentence about
# what an older release measured or changed keeps its version.
current = re.compile(rf'(?<=Download )(?:{VERSION}|rc\.\d+)(?![\w])|(?<=For )(?:{VERSION}|rc\.\d+)(?=,)')
for name in ('README.md', 'docs/README.md', 'docs/getting-started.md', 'rust/README.md'):
    path = root / name
    text = path.read_text(encoding='utf-8')
    text = re.sub(r'(\[(?:Download [^]]+|release)\]\(https://github.com/RamazanKara/Rubylight/releases/tag/)[^)]+\)',
                  lambda m: f'{m[1]}v{new})', text)
    # Releases since rc.30 are named Rubylight; earlier ones Butterpollo.
    for setup in ('butterpollo-setup', 'rubylight-setup'):
        text = re.sub(rf'{setup}-{VERSION}\.exe', f'rubylight-setup-{new}.exe', text)
    for package in ('butterpollo-rust', 'rubylight'):
        text = re.sub(rf'{package}-{VERSION}-windows-x64\.zip', f'rubylight-{new}-windows-x64.zip', text)
    text = text.replace(f'The workspace version is **{old}**', f'The workspace version is **{new}**')
    path.write_text(current.sub(label(new), text), encoding='utf-8', newline='')

for name in ('package.json', 'package-lock.json'):
    path = root / 'rust/web' / name
    text = re.sub(r'("version": )"[^"]+"', lambda m: f'{m[1]}"{new}"',
                  path.read_text(encoding='utf-8'), count=2 if name == 'package-lock.json' else 1)
    path.write_text(text, encoding='utf-8', newline='')

notes = root / 'rust/RELEASE_NOTES.md'
lines = notes.read_text(encoding='utf-8').split('\n')
assert old in lines[0], lines[0]
lines[0] = lines[0].replace(old, new)
history = next(i for i, l in enumerate(lines) if l.startswith('**Release history:** '))
entry = f'[{label(new)}]({anchor(new)})'
if entry not in lines[history]:
    lines[history] = lines[history].replace('**Release history:** ',
                                            f'**Release history:** {entry} · ', 1)
intro = next(i for i, l in enumerate(lines) if i > history and l.strip())
for setup in ('butterpollo-setup', 'rubylight-setup'):
    lines[intro] = lines[intro].replace(f'{setup}-{old}.exe', f'rubylight-setup-{new}.exe')
lines[intro] = short.sub(label(new), lines[intro].replace(old, new))
if f'## New in {label(new)}' not in lines:
    # Pending changes belong to the next release, including when release
    # preparation supplies additional notes with --notes.
    pending = []
    if '## Unreleased' in lines:
        start = lines.index('## Unreleased')
        end = next((i for i in range(start + 1, len(lines)) if lines[i].startswith('## ')), len(lines))
        pending = '\n'.join(lines[start + 1:end]).strip().splitlines()
        del lines[start:end]
    at = lines.index(f'## New in {label(old)}')
    body = args.notes.read_text(encoding='utf-8').strip().split('\n') if args.notes else []
    body = (pending + [''] + body) if pending and body else (pending or body or ['- '])
    lines[at:at] = [f'## New in {label(new)}', '', *body, '']
text = '\n'.join(lines)
for setup in ('butterpollo-setup', 'rubylight-setup'):
    text = text.replace(f'{setup}-{old}.exe` installs', f'rubylight-setup-{new}.exe` installs')
for package in ('butterpollo-rust', 'rubylight'):
    text = text.replace(f'{package}-{old}-windows-x64.zip`', f'rubylight-{new}-windows-x64.zip`')
notes.write_text(text, encoding='utf-8', newline='')
print(f'{old} -> {new}; {count} crates in Cargo.lock; fill in "## New in {label(new)}" in rust/RELEASE_NOTES.md'
      if not args.notes else f'{old} -> {new}; {count} crates in Cargo.lock')
