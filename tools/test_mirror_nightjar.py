"""Tests for mirror_nightjar.py against a small monorepo built with `git subtree add`."""
import os
import subprocess
import tempfile
import unittest

import mirror_nightjar as m

ENV = {**os.environ, 'GIT_AUTHOR_NAME': 't', 'GIT_AUTHOR_EMAIL': 't@t',
       'GIT_COMMITTER_NAME': 't', 'GIT_COMMITTER_EMAIL': 't@t',
       'GIT_CONFIG_GLOBAL': os.devnull, 'GIT_CONFIG_NOSYSTEM': '1'}


class Repo:
    def __init__(self, path):
        self.path = path
        self.n = 0
        os.makedirs(path)
        self.git('init', '-q', '-b', 'main')

    def git(self, *args):
        self.n += 1
        env = {**ENV, 'GIT_AUTHOR_DATE': f'{1700000000 + self.n} +0000',
               'GIT_COMMITTER_DATE': f'{1700000000 + self.n} +0000'}
        p = subprocess.run(['git', '-C', self.path, *args], env=env,
                           capture_output=True, text=True)
        if p.returncode:
            raise AssertionError(f'git {args}: {p.stderr}')
        return p.stdout.strip()

    def commit(self, rel, text, msg):
        full = os.path.join(self.path, rel)
        os.makedirs(os.path.dirname(full), exist_ok=True)
        with open(full, 'w', newline='') as f:
            f.write(text)
        self.git('add', '-A')
        self.git('commit', '-q', '-m', msg)
        return self.git('rev-parse', 'HEAD')


class MirrorTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.nj = Repo(os.path.join(self.tmp.name, 'nj'))
        self.nj.commit('app/a.txt', '1', 'nj one')
        self.nj.commit('app/b.txt', '2', 'nj two')
        self.mono = Repo(os.path.join(self.tmp.name, 'mono'))
        self.mono.commit('rust/r.txt', 'r', 'host one')
        self.mono.git('subtree', 'add', '-q', '--prefix', 'client', self.nj.path, 'main')
        self.git = m.Git(self.mono.path)

    def tearDown(self):
        self.tmp.cleanup()

    def published(self):
        self.mono.git('fetch', '-q', self.nj.path, 'main')
        return self.mono.git('rev-parse', 'FETCH_HEAD')

    def check(self, result, published, subjects):
        self.assertTrue(self.git.is_ancestor(published, result))
        self.assertEqual(self.mono.git('rev-parse', f'{result}^{{tree}}'),
                         self.mono.git('rev-parse', 'HEAD:client'))
        log = self.mono.git('log', '--format=%s', f'{published}..{result}').splitlines()
        self.assertEqual(sorted(log), sorted(subjects))

    def test_up_to_date_after_subtree_add(self):
        pub = self.published()
        self.assertEqual(m.split(self.git, 'HEAD', pub), pub)

    def test_host_only_commits_are_dropped(self):
        pub = self.published()
        self.mono.commit('client/app/a.txt', '3', 'client change')
        self.mono.commit('rust/r.txt', 'r2', 'host change')
        result = m.split(self.git, 'HEAD', pub)
        self.check(result, pub, ['client change'])
        # Author, committer and message come across unchanged.
        src = self.mono.git('log', '-1', '--format=%an %ae %ad %cn %ce %cd %B', 'HEAD~1')
        self.assertEqual(self.mono.git('log', '-1', '--format=%an %ae %ad %cn %ce %cd %B', result), src)

    def test_incremental_from_last_push_matches_subtree_split(self):
        self.mono.commit('client/app/a.txt', '3', 'first')
        first = m.split(self.git, 'HEAD', self.published())
        self.nj.git('fetch', '-q', self.mono.path, f'{first}:refs/heads/tmp')
        self.nj.git('merge', '-q', '--ff-only', 'tmp')
        self.mono.commit('client/app/a.txt', '4', 'second')
        pub = self.published()
        self.assertEqual(pub, first)
        result = m.split(self.git, 'HEAD', pub)
        self.check(result, pub, ['second'])
        # The very commits git subtree split makes.
        ref = self.mono.git('subtree', 'split', '-q', '--prefix', 'client', 'HEAD')
        self.assertEqual(result, ref)

    def test_side_branch_commits_are_kept(self):
        pub = self.published()
        self.mono.git('checkout', '-q', '-b', 'side')
        self.mono.commit('client/app/c.txt', 'c', 'side client')
        self.mono.commit('rust/s.txt', 's', 'side host')
        self.mono.git('checkout', '-q', 'main')
        self.mono.commit('client/app/a.txt', '5', 'main client')
        self.mono.git('merge', '-q', '--no-ff', '-m', 'merge side', 'side')
        result = m.split(self.git, 'HEAD', pub)
        self.check(result, pub, ['side client', 'main client', 'merge side'])

    def test_subtree_pull_keeps_nightjar_commits(self):
        self.nj.commit('app/t.txt', 'tr', 'translation')
        self.mono.git('subtree', 'pull', '-q', '--prefix', 'client', self.nj.path, 'main',
                      '-m', 'pull nightjar')
        self.mono.commit('client/app/a.txt', '6', 'after pull')
        pub = self.published()
        result = m.split(self.git, 'HEAD', pub)
        self.check(result, pub, ['after pull'])


if __name__ == '__main__':
    unittest.main()
