import json, os, pathlib, re, shutil, subprocess, sys, tempfile, textwrap, unittest


ROOT = pathlib.Path(__file__).resolve().parents[2]
BASH = pathlib.Path(os.environ.get('ProgramFiles', ''), 'Git/bin/bash.exe') if os.name == 'nt' else shutil.which('bash')


class ReleaseVersions(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = pathlib.Path(self.temp.name)
        for name in ('Cargo.toml', 'Cargo.lock', 'README.md', 'docs/README.md',
                     'docs/getting-started.md', 'rust/README.md', 'rust/RELEASE_NOTES.md',
                     'rust/release/bump.py', 'rust/release/notes.py', 'rust/release/body.md',
                     'rust/web/package.json', 'rust/web/package-lock.json'):
            target = self.root / name
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(ROOT / name, target)
        for source in ROOT.glob('rust/*/Cargo.toml'):
            target = self.root / source.relative_to(ROOT)
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source, target)
        self.version = re.search(r'(?m)^version = "([^"]+)"', (self.root / 'Cargo.toml').read_text(encoding='utf-8'))[1]

    def run_tool(self, name, *args):
        return subprocess.run([sys.executable, str(self.root / 'rust/release' / name), *args],
                              capture_output=True, text=True, encoding='utf-8')

    def test_prefixed_tag_keeps_bare_asset_version_and_tagged_source_links(self):
        notes = self.root / 'rust/RELEASE_NOTES.md'
        label = self.version.split('-')[-1]
        notes.write_text(f'## New in {label}\n\nA [guide](../docs/api.md).\n\n## Earlier\n', encoding='utf-8')
        for tag in (f'v{self.version}', self.version):
            with self.subTest(tag=tag):
                out = self.root / 'body.md'
                result = self.run_tool('notes.py', tag, '--out', str(out))
                self.assertEqual(result.returncode, 0, result.stderr)
                body = out.read_text(encoding='utf-8')
                self.assertIn(f'/download/{tag}/rubylight-setup-{self.version}.exe', body)
                self.assertIn(f'/download/{tag}/rubylight-{self.version}-windows-x64.zip', body)
                self.assertIn(f'/blob/{tag}/rust/../docs/api.md', body)
                self.assertIn(f'/blob/{tag}/rust/RELEASE_NOTES.md', body)

    def test_wrong_tag_version_is_rejected(self):
        out = self.root / 'body.md'
        result = self.run_tool('notes.py', 'v9.9.9', '--out', str(out))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('Cargo.toml is at', result.stderr)
        self.assertFalse(out.exists())

    def test_bump_updates_stale_public_downloads_and_web_without_rewriting_history(self):
        readme = self.root / 'README.md'
        readme.write_text('[Download 2.1.0](https://github.com/RamazanKara/Rubylight/releases/tag/2.1.0)\n'
                          '`rubylight-setup-2.1.0.exe`\nMeasured in 2.1.0.\n', encoding='utf-8')
        major, minor, patch = map(int, self.version.split('-')[0].split('.'))
        new = f'{major}.{minor}.{patch + 1}'
        result = self.run_tool('bump.py', f'v{new}')
        self.assertEqual(result.returncode, 0, result.stderr)
        text = readme.read_text(encoding='utf-8')
        self.assertIn(f'[Download {new}]', text)
        self.assertIn(f'/tag/v{new})', text)
        self.assertIn(f'rubylight-setup-{new}.exe', text)
        self.assertIn('Measured in 2.1.0.', text)
        self.assertIn(f'version = "{new}"', (self.root / 'Cargo.toml').read_text(encoding='utf-8'))
        self.assertIn(f'## New in {new}', (self.root / 'rust/RELEASE_NOTES.md').read_text(encoding='utf-8'))
        package = json.loads((self.root / 'rust/web/package.json').read_text(encoding='utf-8'))
        lock = json.loads((self.root / 'rust/web/package-lock.json').read_text(encoding='utf-8'))
        self.assertEqual(package['version'], new)
        self.assertEqual(lock['version'], new)
        self.assertEqual(lock['packages']['']['version'], new)

    def test_bump_carries_pending_notes_into_the_next_release(self):
        notes = self.root / 'rust/RELEASE_NOTES.md'
        history = notes.read_text(encoding='utf-8')
        history = re.sub(r'(?ms)^## Unreleased\n.*?(?=^## )', '', history)
        heading = f'## New in {self.version}'
        pending = '- Bundled USB audio and haptics for Steam Deck and DualSense.'
        original = history.replace(heading, f'## Unreleased\n\n{pending}\n\n{heading}', 1)
        extra = self.root / 'extra.md'
        extra.write_text('- Another release change.', encoding='utf-8')
        major, minor, patch = map(int, self.version.split('-')[0].split('.'))
        for with_extra in (False, True):
            with self.subTest(with_extra=with_extra):
                notes.write_text(original, encoding='utf-8')
                new = f'{major}.{minor}.{patch + 1}'
                result = self.run_tool('bump.py', new, *(['--notes', str(extra)] if with_extra else []))
                self.assertEqual(result.returncode, 0, result.stderr)
                updated = notes.read_text(encoding='utf-8')
                section = updated.split(f'## New in {new}\n', 1)[1].split('\n## ', 1)[0]
                self.assertIn(pending, section)
                self.assertEqual(updated.count(pending), 1)
                self.assertNotIn('## Unreleased', updated)
                if with_extra:
                    self.assertIn('- Another release change.', section)
            # Restore the input version and manifests for the second case.
            for name in ('Cargo.toml', 'Cargo.lock', 'rust/web/package.json', 'rust/web/package-lock.json'):
                shutil.copyfile(ROOT / name, self.root / name)

    @unittest.skipUnless(BASH and pathlib.Path(BASH).is_file(), 'Bash is not installed')
    def test_reuse_requires_a_push_build_with_an_available_artifact(self):
        workflow = (ROOT / '.github/workflows/rust-windows.yml').read_text(encoding='utf-8')
        script = textwrap.dedent(re.search(r'(?ms)^      - id: built\n.*?^        run: \|\n(.*?)(?=^  windows:)', workflow)[1])
        stub = '''gh() {
          if [ "$1" = run ]; then
            [[ " $* " == *" --event push "* ]] || return 1
            printf '%s' "$MOCK_RUN"
          else
            [ "$MOCK_ARTIFACT" != error ] || return 1
            printf '%s' "$MOCK_ARTIFACT"
          fi
        }
        '''
        for candidate, artifact, expected in [('', '', ''), ('123', '456', '123'),
                                               ('123', '', ''), ('123', 'error', '')]:
            with self.subTest(candidate=candidate, artifact=artifact):
                env = dict(os.environ, MOCK_RUN=candidate, MOCK_ARTIFACT=artifact,
                           GITHUB_OUTPUT='output.txt', GITHUB_REPOSITORY='owner/repo',
                           GITHUB_SHA='abc', GITHUB_SERVER_URL='https://github.com')
                out = self.root / 'output.txt'
                out.write_text('', encoding='utf-8')
                result = subprocess.run([str(BASH), '--noprofile', '--norc', '-ec', stub + script],
                                        cwd=self.root, env=env, capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(out.read_text(encoding='utf-8'), f'run={expected}\n')


if __name__ == '__main__':
    unittest.main()
