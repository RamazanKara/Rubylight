"""Assemble Nightjar's release notes from the fragments in client/notes.d.

Writes LF line endings on any platform.

usage: python tools/nightjar_notes.py --out FILE [--version-code N] [--force] [--keep]
                                     [--changelog-en FILE] [--changelog-de FILE]

Each fragment (client/notes.d/<topic>.md, see the README there) has a "## en"
and a "## de" list of bullets. In file name order they become the "What's
new" draft in --out (the GitHub release text; add the "Tested." paragraph)
and the store changelogs client/fastlane/metadata/android/{en-US,de}/
changelogs/<versionCode>.txt, versionCode from client/app/build.gradle unless
given. --changelog-en and --changelog-de give a hand-written store changelog
for that language instead, for a release whose bullets are too long together.
The fragments are deleted afterwards unless --keep. Nothing is written
when a fragment is malformed, a changelog would exceed Google Play's 500
characters, or a changelog for that versionCode exists (unless --force).
"""
import argparse, pathlib, re, sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
NOTES_D = 'client/notes.d'
CHANGELOGS = 'client/fastlane/metadata/android/{locale}/changelogs/{code}.txt'
LANGUAGES = {'en': 'en-US', 'de': 'de'}
PLAY_LIMIT = 500


def read(directory):
    """{language: [bullet, ...]} over every fragment in file name order, and the files."""
    files = sorted(p for p in pathlib.Path(directory).glob('*.md') if p.name.lower() != 'readme.md')
    bullets = {language: [] for language in LANGUAGES}
    for path in files:
        text = re.sub(r'(?s)<!--.*?-->', '', path.read_text(encoding='utf-8'))
        sections = dict(re.findall(r'(?ms)^## (\w+)\s*\n(.*?)(?=^## |\Z)', text))
        unknown = set(sections) - set(LANGUAGES)
        if unknown:
            raise SystemExit(f'{path}: unknown section {", ".join(sorted(unknown))}; use ## en and ## de')
        for language in LANGUAGES:
            lines = [line.rstrip() for line in sections.get(language, '').split('\n') if line.strip()]
            if not lines or any(not line.startswith('- ') and not line[:1].isspace() for line in lines):
                raise SystemExit(f'{path}: "## {language}" needs a list of "- " bullets')
            # Continuation lines join their bullet.
            for line in lines:
                if line.startswith('- '):
                    bullets[language].append(line[2:].strip())
                else:
                    bullets[language][-1] += ' ' + line.strip()
    return bullets, files


def plain(bullet):
    """Store changelogs are plain text: no links, bold or code marks."""
    text = re.sub(r'\[([^]]+)\]\([^)]+\)', r'\1', bullet)
    return re.sub(r'\*\*|__|`', '', text)


def version_code(root):
    gradle = (root / 'client/app/build.gradle').read_text(encoding='utf-8')
    match = re.search(r'(?m)^\s*versionCode\s*=?\s*(\d+)', gradle)
    if not match:
        raise SystemExit('no versionCode in client/app/build.gradle')
    return int(match[1])


def main(argv=None, root=ROOT):
    parser = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    parser.add_argument('--out', required=True, type=pathlib.Path, help="the What's new draft")
    parser.add_argument('--version-code', type=int)
    parser.add_argument('--force', action='store_true', help='replace existing changelogs for the versionCode')
    parser.add_argument('--keep', action='store_true', help='keep the fragments')
    for language in LANGUAGES:
        parser.add_argument(f'--changelog-{language}', type=pathlib.Path, metavar='FILE',
                            help=f'a hand-written {language} store changelog instead of the bullets')
    args = parser.parse_args(argv)
    bullets, files = read(root / NOTES_D)
    if not files:
        raise SystemExit(f'no fragments in {NOTES_D}')
    code = args.version_code or version_code(root)
    changelogs = {}
    for language, locale in LANGUAGES.items():
        path = root / CHANGELOGS.format(locale=locale, code=code)
        given = getattr(args, f'changelog_{language}')
        if given:
            text = given.read_text(encoding='utf-8').replace('\r\n', '\n').strip('\n') + '\n'
            if not text.strip():
                raise SystemExit(f'{given} is empty')
        else:
            text = '\n'.join('• ' + plain(b) for b in bullets[language]) + '\n'
        if len(text.rstrip('\n')) > PLAY_LIMIT:
            raise SystemExit(f'{path.relative_to(root)} would have {len(text.rstrip())} characters, over Google '
                             f"Play's {PLAY_LIMIT}: shorten the fragments or write it by hand (--changelog-{language})")
        if path.exists() and not args.force:
            raise SystemExit(f'{path.relative_to(root)} exists; bump versionCode first or pass --force')
        changelogs[path] = text
    draft = "## What's new\n\n" + '\n'.join(f'- {b}' for b in bullets['en']) + '\n\n**Tested.** \n'
    args.out.write_text(draft, encoding='utf-8', newline='\n')
    for path, text in changelogs.items():
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding='utf-8', newline='\n')
    if not args.keep:
        for path in files:
            path.unlink()
    print(f'{len(files)} fragments: {args.out} and the {code}.txt changelogs (en-US, de); '
          'add the "Tested." paragraph to the draft')
    return 0


if __name__ == '__main__':
    sys.exit(main())
