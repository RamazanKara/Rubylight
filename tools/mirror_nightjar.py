"""Publishes client/ to the nightjar repo, fast-forward only.

Incremental replacement for `git subtree split --prefix client`, which walks
all of the monorepo's history in a shell loop on every run (over 10 minutes on
Windows). This one starts from what nightjar main already has:

1. fetch nightjar main (the published split);
2. walk the monorepo's first-parent history back to the newest commit whose
   client/ tree matches a commit on nightjar's first-parent history (the anchor:
   that monorepo commit was the last one mirrored, or a `git subtree pull` of
   nightjar's own commits);
3. rewrite only the commits after the anchor, side branches included, the way
   `git subtree split` does: tree = client/, same author, committer and
   message, parents mapped, commits that leave client/ alone dropped.

Nightjar commits merged back with `git subtree pull` keep their own ids. The
push only ever fast-forwards nightjar main; anything else stops before pushing.
    python tools/mirror_nightjar.py --dry-run    # split, check, push --dry-run
    python tools/mirror_nightjar.py              # the real push
"""
import argparse
import subprocess
import sys

PREFIX = 'client'
REMOTE = 'https://github.com/RamazanKara/nightjar.git'


class MirrorError(Exception):
    pass


class Git:
    def __init__(self, repo):
        self.repo = repo

    def run(self, *args, stdin=None, check=True):
        p = subprocess.run(['git', '-C', self.repo, *args], input=stdin,
                           stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        if check and p.returncode:
            raise MirrorError(f"git {' '.join(args)} failed ({p.returncode}): "
                              f"{p.stderr.decode(errors='replace').strip()}")
        return p

    def out(self, *args, stdin=None):
        return self.run(*args, stdin=stdin).stdout.decode().strip()

    def lines(self, *args):
        text = self.out(*args)
        return text.splitlines() if text else []

    def is_ancestor(self, a, b):
        p = self.run('merge-base', '--is-ancestor', a, b, check=False)
        if p.returncode not in (0, 1):
            raise MirrorError(f'git merge-base failed: {p.stderr.decode(errors="replace")}')
        return p.returncode == 0

    def trees(self, specs):
        """`<rev>:<path>` -> tree id, or None where the path is missing."""
        if not specs:
            return {}
        data = ''.join(s + '\n' for s in specs).encode()
        out = self.run('cat-file', '--batch-check=%(objectname) %(objecttype)',
                       stdin=data).stdout.decode().splitlines()
        result = {}
        for spec, line in zip(specs, out):
            parts = line.split()
            result[spec] = parts[0] if len(parts) == 2 and parts[1] == 'tree' else None
        return result


def split(git, commit, published, prefix=PREFIX):
    """Returns the split commit of `commit` built on top of `published`."""
    commit = git.out('rev-parse', '--verify', f'{commit}^{{commit}}')
    # Nightjar's history: every commit and its tree.
    nj = [l.split() for l in git.lines('log', '--format=%H %T', published)]
    nj_set = {c for c, _ in nj}
    nj_tree_of = {c: t for c, t in nj}
    nj_by_tree = {}
    for c, t in nj:  # newest first: keep the newest commit per tree
        nj_by_tree.setdefault(t, c)
    nj_fp_by_tree = {}
    for line in git.lines('log', '--first-parent', '--format=%H %T', published):
        c, t = line.split()
        nj_fp_by_tree.setdefault(t, c)

    # Anchor: newest first-parent monorepo commit whose client/ is on nightjar.
    fp = git.lines('rev-list', '--first-parent', commit)
    fp_trees = git.trees([f'{c}:{prefix}' for c in fp])
    anchor = None
    for c in fp:
        if c in nj_set:
            break  # walked into nightjar's own history: no anchor in client/
        t = fp_trees[f'{c}:{prefix}']
        if t is not None and t in nj_fp_by_tree:
            anchor = (c, nj_fp_by_tree[t])
            break
    if anchor is None:
        raise MirrorError(f'no monorepo commit has a {prefix}/ tree that is on nightjar '
                          f'{published}; was {prefix}/ added with `git subtree add`?')
    mono_anchor, split_anchor = anchor

    todo = [l.split() for l in git.lines('rev-list', '--topo-order', '--reverse', '--parents',
                                         commit, f'^{mono_anchor}', f'^{published}')]
    trees = git.trees([f'{c[0]}:{prefix}' for c in todo])
    mapped = {mono_anchor: split_anchor}
    tree_of = dict(nj_tree_of)

    def map_parent(p):
        if p in mapped:
            return mapped[p]
        if p in nj_set:
            return p
        t = git.trees([f'{p}:{prefix}'])[f'{p}:{prefix}']
        mapped[p] = nj_by_tree.get(t) if t else None  # fork points before the anchor
        return mapped[p]

    def tree(c):
        if c not in tree_of:
            tree_of[c] = git.out('rev-parse', f'{c}^{{tree}}')
        return tree_of[c]

    for c, *parents in todo:
        t = trees[f'{c}:{prefix}']
        if t is None:
            mapped[c] = c  # a nightjar commit brought in by `git subtree pull`
            tree_of.pop(c, None)
            continue
        ps = []
        for p in parents:
            m = map_parent(p)
            if m and m not in ps:
                ps.append(m)
        if len(ps) > 1:
            ps = [p for p in ps if not any(q != p and git.is_ancestor(p, q) for q in ps)]
        if len(ps) == 1 and tree(ps[0]) == t:
            mapped[c] = ps[0]  # leaves client/ alone
            continue
        mapped[c] = rewrite(git, c, t, ps)
        tree_of[mapped[c]] = t
    return mapped[commit]


def rewrite(git, commit, tree, parents):
    raw = git.run('cat-file', 'commit', commit).stdout
    header, sep, message = raw.partition(b'\n\n')
    keep = [l for l in header.split(b'\n')
            if l.startswith((b'author ', b'committer ', b'encoding '))]
    obj = b'tree ' + tree.encode() + b'\n'
    obj += b''.join(b'parent ' + p.encode() + b'\n' for p in parents)
    obj += b'\n'.join(keep) + sep + message
    return git.out('hash-object', '-t', 'commit', '-w', '--stdin', stdin=obj)


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    ap.add_argument('--remote', default=REMOTE)
    ap.add_argument('--branch', default='main')
    ap.add_argument('--ref', default='HEAD', help='monorepo commit to publish (main after green CI)')
    ap.add_argument('--repo', default=None)
    ap.add_argument('--dry-run', action='store_true')
    args = ap.parse_args(argv)

    git = Git(args.repo or '.')
    git = Git(git.out('rev-parse', '--show-toplevel'))
    commit = git.out('rev-parse', '--verify', f'{args.ref}^{{commit}}')
    git.run('fetch', '--quiet', '--no-tags', args.remote, f'refs/heads/{args.branch}')
    published = git.out('rev-parse', '--verify', 'FETCH_HEAD^{commit}')
    result = split(git, commit, published)
    print(f'monorepo {commit}, {PREFIX}/ split {result}, nightjar {args.branch} {published}')
    if result == published:
        print(f'nightjar {args.branch} is up to date')
        return 0
    if not git.is_ancestor(published, result):
        raise MirrorError(
            f'not a fast-forward: nightjar {args.branch} {published} is not in the split. '
            f'Merge it with `git subtree pull --prefix {PREFIX} {args.remote} {args.branch}` first.')
    if git.out('rev-parse', f'{result}^{{tree}}') != git.out('rev-parse', f'{commit}:{PREFIX}'):
        raise MirrorError(f'split {result} does not match {PREFIX}/ of {commit}')
    count = git.out('rev-list', '--count', f'{published}..{result}')
    print(f'fast-forward by {count} commit(s):')
    for line in git.lines('log', '--oneline', '--no-decorate', f'{published}..{result}')[:20]:
        print(line)
    push = ['push'] + (['--dry-run'] if args.dry_run else [])
    p = git.run(*push, args.remote, f'{result}:refs/heads/{args.branch}', check=False)
    sys.stdout.write(p.stderr.decode(errors='replace'))
    if p.returncode:
        raise MirrorError(f'git push failed ({p.returncode})')
    return 0


if __name__ == '__main__':
    try:
        sys.exit(main())
    except MirrorError as e:
        print(f'error: {e}', file=sys.stderr)
        sys.exit(1)
