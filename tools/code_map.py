"""Find the code for a change, and what usually changes with it.

usage: python tools/code_map.py index PATH [PATH ...] [--dirs]
       python tools/code_map.py with FILE [--since DATE] [--top N]
       python tools/code_map.py hot [PATH ...] [--since DATE] [--top N]
       python tools/code_map.py check [MAP ...]

index  one line per source file under PATH: lines, tests, and the
       first sentence of its header comment (`//!` in Rust, the class Javadoc,
       a module docstring, a leading comment). --dirs sums each folder instead.
with   the files that changed in the same commits as FILE, most often first,
       with the share of FILE's commits each was part of. Read it before
       changing FILE: a setting, a control message or a string usually has a
       second and third home.
hot    the files with the most `fix` commits, as fixes/commits: where bugs
       have been found, and so where to read the tests first.

check  what the hand-written maps (docs/code-map.md, client/docs/code-map.md)
       name that is gone: a file, or a `name` in a link's text that its file no
       longer contains (the rules are on stale()). Exits 1 if anything is.

`with` and `hot` read `git log` since --since (default: 6 months ago). The
translations of one text count as one file. The maps hold what this tool
cannot derive: flows, seams and the checklist for each kind of change.
"""
import argparse, collections, pathlib, re, subprocess, sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
SOURCES = ('.rs', '.java', '.kt', '.svelte', '.ts', '.js', '.mjs', '.cjs', '.py', '.ps1', '.hlsl', '.c', '.cpp', '.h')
# Other people's code and generated output: never ours to map.
FOREIGN = re.compile(r'(^|/)(vendor|third-party|node_modules|include|openssl|moonlight-common-c)/')
# The translations of one text change together; count them as one file.
GROUPS = ((re.compile(r'(client/protocol/desktop/src/strings/)\w+\.rs$'), r'\1*.rs'),
          (re.compile(r'(client/app/src/main/res/values)[^/]*(/\w+\.xml)$'), r'\1*\2'),
          (re.compile(r'((?:rust/release|client)/notes\.d/).*$'), r'\1*'))
TESTS = {'.rs': re.compile(r'^\s*#\[(?:tokio::)?test\b', re.M), '.java': re.compile(r'^\s*@Test\b', re.M),
         '.kt': re.compile(r'^\s*@Test\b', re.M), '.py': re.compile(r'^\s*def test_', re.M),
         '.mjs': re.compile(r'^\s*(?:test|it)\(', re.M), '.cjs': re.compile(r'^\s*(?:test|it)\(', re.M)}
SENTENCE = re.compile(r'(?<=[.!?])\s')
TYPE = re.compile(r'^\s*(?:public |final |abstract |sealed |data |internal |private |open )*'
                  r'(?:class|interface|enum|object|@interface)\b', re.M)
LINE_COMMENT = {'.rs': '//!', '.py': '#', '.ps1': '#'}
MAPS = ('docs/code-map.md', 'client/docs/code-map.md')
# Files that are not indexed, and whose names a map may still cite.
NAMED = ('.gradle', '.xml', '.toml', '.yml', '.json')
LINK = re.compile(r'\[([^\]]+)\]\(([^)\s]+)\)')
NAME = re.compile(r'`([A-Za-z_][\w:]*)(?:\(\))?`')
PATH = re.compile(r'`([\w.-]+(?:/[\w.-]+)*/?)`')


def block(lines, mark):
    """The first run of lines starting with `mark`, up to its first blank one."""
    picked = []
    for line in lines[:60]:
        line = line.strip()
        if line.startswith(mark):
            text = line[len(mark):].strip()
            if not text and picked:
                break
            if text and not text.startswith(('!', '/usr/bin', 'SPDX', 'Copyright', 'eslint', '@ts-')):
                picked.append(text)
        elif picked or (line and mark == '//!' and not line.startswith(('#!', '//'))):
            break
    return picked


def summary(text, suffix):
    """The first sentence of a file's header comment, or ''."""
    picked = []
    if suffix == '.py':
        found = re.match(r'(?:\s*#[^\n]*\n)*\s*(?:"""|\'\'\')(.*?)(?:"""|\'\'\'|\n\s*\n)', text, re.S)
        picked = found.group(1).split('\n') if found else []
    elif suffix in ('.java', '.kt'):
        head = text[:m.start()] if (m := TYPE.search(text)) else ''
        found = re.findall(r'/\*\*(.*?)\*/', head, re.S)
        picked = [re.sub(r'^\s*\*\s?', '', line) for line in found[-1].split('\n')] if found else []
        picked = picked[:next((i for i, line in enumerate(picked) if i and not line.strip()), len(picked))]
    elif suffix == '.svelte' and (found := re.match(r'\s*(?:<script[^>]*>\s*)?(?:<!--|/\*+|//)(.*?)(?:-->|\*/|\n\s*\n)', text, re.S)):
        picked = [re.sub(r'^\s*(?://|\*)\s?', '', line) for line in found.group(1).split('\n')]
    elif found := re.match(r'\s*/\*+(.*?)\*/', text, re.S):
        picked = [re.sub(r'^\s*\*\s?', '', line) for line in found.group(1).split('\n')]
    else:
        picked = block(text.split('\n'), LINE_COMMENT.get(suffix, '//'))
    joined = ' '.join(' '.join(picked).split())
    first = SENTENCE.split(joined, maxsplit=1)[0]
    return first if len(first) <= 160 else first[:157].rstrip() + '...'


def ours(path):
    return path.endswith(SOURCES) and not FOREIGN.search(path)


def git(*args, root=ROOT):
    return subprocess.run(('git', '-c', 'core.quotepath=off') + args, cwd=root, check=True, capture_output=True,
                          text=True, encoding='utf-8', errors='replace').stdout


def relative(path, root=ROOT):
    """A path as git names it: given from the root, or from the folder the tool is run in."""
    path = pathlib.Path(path)
    if not path.is_absolute() and not (root / path).exists() and (pathlib.Path.cwd() / path).exists():
        path = pathlib.Path.cwd() / path
    if path.is_absolute():
        path = path.resolve().relative_to(root)
    return path.as_posix().rstrip('/') or '.'


def index(paths, root=ROOT):
    """(path, lines, tests, summary) for each of our sources under paths that git does not ignore."""
    rows = []
    for path in git('ls-files', '--cached', '--others', '--exclude-standard', '--', *paths, root=root).splitlines():
        if not ours(path):
            continue
        try:
            text = (root / path).read_text(encoding='utf-8', errors='replace')
        except OSError:
            continue  # deleted in the working tree
        suffix = pathlib.PurePosixPath(path).suffix
        tests = len(TESTS[suffix].findall(text)) if suffix in TESTS else 0
        rows.append((path, text.count('\n') + (not text.endswith('\n') and bool(text)), tests, summary(text, suffix)))
    return rows


def folders(rows):
    """index() rows summed per folder: (folder, files, lines, tests)."""
    sums = collections.defaultdict(lambda: [0, 0, 0])
    for path, lines, tests, _ in rows:
        entry = sums[path.rpartition('/')[0] or '.']
        entry[0] += 1; entry[1] += lines; entry[2] += tests
    return [(folder, *sums[folder]) for folder in sorted(sums)]


def grouped(path):
    for pattern, replacement in GROUPS:
        path = pattern.sub(replacement, path)
    return path


def commits(log):
    """(subject, files) per commit of `git log --name-only --format=@@%s`."""
    found = []
    for line in log.splitlines():
        if line.startswith('@@'):
            found.append((line[2:], set()))
        elif line.strip() and found:
            found[-1][1].add(grouped(line.strip()))
    return found


def together(history, target, limit=40):
    """(count, share of target's commits, path), for commits that changed target.

    A commit over `limit` files is a sweep (a rename, a version bump) and says
    nothing about what belongs together.
    """
    target = grouped(target)
    mine = [files for _, files in history if target in files and len(files) <= limit]
    counts = collections.Counter(path for files in mine for path in files if path != target)
    return len(mine), [(count, count / len(mine), path) for path, count in counts.most_common()]


def fixes(history):
    """(fix commits, commits, path), most fixes first."""
    total, fixed = collections.Counter(), collections.Counter()
    for subject, files in history:
        total.update(files)
        if re.match(r'fix[(:!]', subject):
            fixed.update(files)
    return [(count, total[path], path) for path, count in fixed.most_common()]


def stale(text, folder):
    """What a map names that is gone, as (line, target, what is missing).

    The map lies one folder below the tree it describes (docs/ or client/docs/).
    - A link's target must exist. Where it is a source file, every `name` in
      the link's text must still occur in it as a whole word:
      [`launch`](nvhttp.rs) stops being true the day the function is renamed.
    - A `path/in/backticks` must exist when its first part is in the tree.
    - In a table column headed by a `folder/`, each `file.rs` or `folder/` in
      the cells below must exist under it.
    """
    root, missing, bases = folder.parent, [], {}
    lines = text.splitlines()
    for number, line in enumerate(lines, 1):
        for label, target in LINK.findall(line):
            target = target.partition('#')[0]
            if not target or re.match(r'[a-z][a-z0-9+.-]*:', target):
                continue
            path = folder / target
            if not path.exists():
                missing.append((number, target, 'no such file'))
            elif path.is_file() and path.suffix in SOURCES + NAMED:
                source = path.read_text(encoding='utf-8', errors='replace')
                for name in NAME.findall(label):
                    if not re.search(rf'\b{re.escape(name.rpartition("::")[2])}\b', source):
                        missing.append((number, target, f'no `{name}`'))
        plain = LINK.sub('', line)

        def rooted(path):
            return '/' in path.rstrip('/') and (root / path.split('/')[0]).exists()
        missing += [(number, path, 'no such file') for path in PATH.findall(plain)
                    if rooted(path) and not (root / path).exists()]
        if not line.lstrip().startswith('|'):
            bases = {}
            continue
        cells = plain.split('|')
        if number < len(lines) and re.match(r'\s*\|[\s|:-]+\|\s*$', lines[number]):
            found = {column: [path for path in PATH.findall(cell) if path.endswith('/')] for column, cell in enumerate(cells)}
            bases = {column: paths[0] for column, paths in found.items() if paths}
            continue
        for column, base in bases.items():
            for path in PATH.findall(cells[column]) if column < len(cells) else ():
                if ('.' in path or path.endswith('/')) and not rooted(path) and not (root / base / path).exists():
                    missing.append((number, base + path, 'no such file'))
    return missing


def log(since, paths, full=False, root=ROOT):
    args = ['log', f'--since={since}', '--no-merges', '--name-only', '--format=@@%s']
    return commits(git(*args, *(['--full-diff'] if full else []), '--', *paths, root=root))


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    parser.add_argument('command', choices=['index', 'with', 'hot', 'check'])
    parser.add_argument('paths', nargs='*')
    parser.add_argument('--dirs', action='store_true', help='index: one line per folder')
    parser.add_argument('--since', default='6 months ago', help='with, hot: how far back to read git log')
    parser.add_argument('--top', type=int, default=30, help='with, hot: how many lines to print')
    args = parser.parse_args(argv)
    if hasattr(sys.stdout, 'reconfigure'):
        sys.stdout.reconfigure(encoding='utf-8', errors='replace')
    paths = [relative(path) for path in args.paths]
    if args.command == 'check':
        found = 0
        for name in paths or [name for name in MAPS if (ROOT / name).exists()]:
            for number, target, what in stale((ROOT / name).read_text(encoding='utf-8'), (ROOT / name).parent):
                found += 1
                print(f'{name}:{number}: {target}: {what}')
        print(f'{found} stale reference(s)' if found else 'the maps are current')
        return 1 if found else 0
    if args.command == 'index':
        if not paths:
            parser.error('index needs a path: the whole tree is over three thousand files')
        rows = index(paths)
        if args.dirs:
            print(f'{"files":>5} {"lines":>7} {"tests":>5}  folder')
            for folder, files, lines, tests in folders(rows):
                print(f'{files:5} {lines:7} {tests:5}  {folder}')
        else:
            print(f'{"lines":>5} {"tests":>5}  file')
            for path, lines, tests, text in rows:
                print(f'{lines:5} {tests or "":>5}  {path}' + (f'  -- {text}' if text else ''))
    elif args.command == 'with':
        if len(paths) != 1:
            parser.error('with takes one file')
        count, rows = together(log(args.since, paths, full=True), paths[0])
        print(f'{paths[0]}: {count} commits since {args.since}')
        for n, share, path in rows[:args.top]:
            print(f'{n:4} {share:4.0%}  {path}')
    else:
        print('fixes/commits  file')
        for fixed, total, path in [row for row in fixes(log(args.since, paths)) if ours(row[2])][:args.top]:
            print(f'{fixed:5}/{total:<5}  {path}')
    return 0


if __name__ == '__main__':
    sys.exit(main())
