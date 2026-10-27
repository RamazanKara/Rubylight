import json, os, pathlib, re, shutil, subprocess, sys, tempfile, textwrap, unittest
import xml.etree.ElementTree as ET


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
                     'rust/release/fragments.py', 'rust/release/notes.d/README.md',
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
        # Only 2.0.0 release candidates are headed by their bare label; every later version keeps its full version.
        label = self.version.split('-')[-1] if self.version.startswith('2.0.0-rc.') else self.version
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
                # The SignPath Foundation asks for the policy on every download page.
                self.assertIn('[Code signing policy](https://ramazankara.github.io/Rubylight/docs/code-signing-policy.html)', body)

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

    def test_prerelease_keeps_its_full_version_as_label(self):
        readme = self.root / 'README.md'
        readme.write_text(f'[Download {self.version}](https://github.com/RamazanKara/Rubylight/releases/tag/v{self.version})\n'
                          f'`rubylight-setup-{self.version}.exe`\n', encoding='utf-8')
        notes = self.root / 'rust/RELEASE_NOTES.md'
        # Pending notes would replace the empty bullet this test fills in.
        notes.write_text(re.sub(r'(?ms)^## Unreleased\n.*?(?=^## )', '', notes.read_text(encoding='utf-8')),
                         encoding='utf-8')
        # Ahead of whatever the repository is at, so the test survives the next prerelease bump.
        major, minor, patch = map(int, self.version.split('-')[0].split('.'))
        base = f'{major}.{minor}.{patch + 1}'
        for new in (f'{base}-alpha.1', f'{base}-alpha.2', f'{base}-beta.1', f'{base}-rc.1'):
            with self.subTest(new=new):
                result = self.run_tool('bump.py', f'v{new}')
                self.assertEqual(result.returncode, 0, result.stderr)
                text = readme.read_text(encoding='utf-8')
                self.assertIn(f'[Download {new}](https://github.com/RamazanKara/Rubylight/releases/tag/v{new})', text)
                self.assertIn(f'`rubylight-setup-{new}.exe`', text)
                history = notes.read_text(encoding='utf-8')
                anchor = '#new-in-' + new.replace('.', '')
                self.assertIn(f'[{new}]({anchor})', history)
                self.assertEqual(history.count(f'## New in {new}\n'), 1)
                notes.write_text(history.replace(f'## New in {new}\n\n- \n', f'## New in {new}\n\n- Change.\n', 1),
                                 encoding='utf-8')
                out = self.root / 'body.md'
                result = self.run_tool('notes.py', f'v{new}', '--out', str(out))
                self.assertEqual(result.returncode, 0, result.stderr)
                body = out.read_text(encoding='utf-8')
                self.assertIn(f'### New in {new}', body)
                self.assertIn('- Change.', body)

    def test_bump_rejects_unknown_prerelease_kinds(self):
        result = self.run_tool('bump.py', '3.0.0-preview.1')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(f'version = "{self.version}"', (self.root / 'Cargo.toml').read_text(encoding='utf-8'))

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

    def test_bump_assembles_the_note_fragments_after_the_pending_notes(self):
        notes = self.root / 'rust/RELEASE_NOTES.md'
        history = re.sub(r'(?ms)^## Unreleased\n.*?(?=^## )', '', notes.read_text(encoding='utf-8'))
        notes.write_text(history.replace(f'## New in {self.version}',
                                         f'## Unreleased\n\n- Pending.\n\n## New in {self.version}', 1),
                         encoding='utf-8')
        fragments = self.root / 'rust/release/notes.d'
        (fragments / 'b-usb.md').write_text('# USB\n<!-- host side -->\n- USB forwarding.\n  Experimental.\n',
                                            encoding='utf-8')
        (fragments / 'a-hdr.md').write_text('- Dolby Vision.\n- HLG.\n', encoding='utf-8')
        major, minor, patch = map(int, self.version.split('-')[0].split('.'))
        new = f'{major}.{minor}.{patch + 1}'
        result = self.run_tool('bump.py', new)
        self.assertEqual(result.returncode, 0, result.stderr)
        updated = notes.read_text(encoding='utf-8')
        section = updated.split(f'## New in {new}\n', 1)[1].split('\n## ', 1)[0]
        self.assertEqual(section.strip(), '- Pending.\n- Dolby Vision.\n- HLG.\n- USB forwarding.\n  Experimental.')
        self.assertNotIn('## Unreleased', updated)
        self.assertNotIn('host side', updated)
        self.assertEqual(sorted(p.name for p in fragments.iterdir()), ['README.md'])

    def test_a_malformed_fragment_stops_bump_before_it_writes(self):
        (self.root / 'rust/release/notes.d/x.md').write_text('Just a sentence.\n', encoding='utf-8')
        cargo = (self.root / 'Cargo.toml').read_text(encoding='utf-8')
        notes = (self.root / 'rust/RELEASE_NOTES.md').read_text(encoding='utf-8')
        result = self.run_tool('bump.py', '9.9.9')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('x.md', result.stderr)
        self.assertEqual((self.root / 'Cargo.toml').read_text(encoding='utf-8'), cargo)
        self.assertEqual((self.root / 'rust/RELEASE_NOTES.md').read_text(encoding='utf-8'), notes)
        self.assertTrue((self.root / 'rust/release/notes.d/x.md').exists())

    def test_fragments_collect_into_unreleased_between_releases(self):
        notes = self.root / 'rust/RELEASE_NOTES.md'
        history = re.sub(r'(?ms)^## Unreleased\n.*?(?=^## )', '', notes.read_text(encoding='utf-8'))
        fragment = self.root / 'rust/release/notes.d/roaming.md'
        for existing in ('', '- Already here.'):
            with self.subTest(existing=existing):
                start = history.replace(f'## New in {self.version}',
                                        f'## Unreleased\n\n{existing}\n\n## New in {self.version}', 1) \
                    if existing else history
                notes.write_text(start, encoding='utf-8')
                fragment.write_text('- Roaming.\n', encoding='utf-8')
                result = self.run_tool('fragments.py')
                self.assertEqual(result.returncode, 0, result.stderr)
                updated = notes.read_text(encoding='utf-8')
                pending = updated.split('## Unreleased\n', 1)[1].split('\n## ', 1)[0]
                self.assertEqual(pending.strip(), (existing + '\n' if existing else '') + '- Roaming.')
                self.assertLess(updated.index('## Unreleased'), updated.index(f'## New in {self.version}'))
                self.assertFalse(fragment.exists())
        result = self.run_tool('fragments.py')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('no fragments', result.stdout)

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
        # A configured SignPath project signs the release, so it never reuses
        # main's unsigned build.
        for candidate, artifact, project, expected in [('', '', '', 'run=\n'), ('123', '456', '', 'run=123\n'),
                                                        ('123', '', '', 'run=\n'), ('123', 'error', '', 'run=\n'),
                                                        ('123', '456', 'rubylight', 'sign=true\n')]:
            with self.subTest(candidate=candidate, artifact=artifact, project=project):
                env = dict(os.environ, MOCK_RUN=candidate, MOCK_ARTIFACT=artifact, SIGNPATH_PROJECT=project,
                           GITHUB_OUTPUT='output.txt', GITHUB_REPOSITORY='owner/repo',
                           GITHUB_SHA='abc', GITHUB_SERVER_URL='https://github.com')
                out = self.root / 'output.txt'
                out.write_text('', encoding='utf-8')
                result = subprocess.run([str(BASH), '--noprofile', '--norc', '-ec', stub + script],
                                        cwd=self.root, env=env, capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(out.read_text(encoding='utf-8'), expected)


class NightjarNotes(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = pathlib.Path(self.temp.name)
        for name in ('tools/nightjar_notes.py', 'client/app/build.gradle', 'client/notes.d/README.md'):
            (self.root / name).parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(ROOT / name, self.root / name)
        self.code = int(re.search(r'versionCode\s*=?\s*(\d+)',
                                  (ROOT / 'client/app/build.gradle').read_text(encoding='utf-8'))[1])
        self.notes = self.root / 'client/notes.d'
        self.out = self.root / 'whats-new.md'

    def run_tool(self, *args):
        return subprocess.run([sys.executable, str(self.root / 'tools/nightjar_notes.py'), '--out', str(self.out),
                               *args], capture_output=True, text=True, encoding='utf-8')

    def changelog(self, locale, code=None):
        return self.root / f'client/fastlane/metadata/android/{locale}/changelogs/{code or self.code}.txt'

    def test_fragments_become_the_draft_and_both_changelogs(self):
        (self.notes / 'b-menu.md').write_text('## en\n- The **stream menu** opens\n  with a long press.\n\n'
                                              '## de\n- Das **Stream-Menü** öffnet sich\n  mit langem Drücken.\n',
                                              encoding='utf-8')
        (self.notes / 'a-tv.md').write_text('<!-- nightjar#7 -->\n## de\n- Neues [TV-Design](https://x.test).\n'
                                            '## en\n- A new [TV design](https://x.test).\n', encoding='utf-8')
        result = self.run_tool()
        self.assertEqual(result.returncode, 0, result.stderr)
        draft = self.out.read_text(encoding='utf-8')
        self.assertEqual(draft, "## What's new\n\n- A new [TV design](https://x.test).\n"
                                '- The **stream menu** opens with a long press.\n\n**Tested.** \n')
        self.assertEqual(self.changelog('en-US').read_text(encoding='utf-8'),
                         '• A new TV design.\n• The stream menu opens with a long press.\n')
        self.assertEqual(self.changelog('de').read_text(encoding='utf-8'),
                         '• Neues TV-Design.\n• Das Stream-Menü öffnet sich mit langem Drücken.\n')
        self.assertEqual([p.name for p in self.notes.iterdir()], ['README.md'])

    def test_mistakes_change_nothing(self):
        cases = {
            'needs a list': '## en\n- Only English.\n',
            'unknown section': '## en\n- A.\n## fr\n- B.\n',
            "Google Play's 500": '## en\n' + '- A long sentence about streaming.\n' * 20 + '## de\n- B.\n',
        }
        for message, text in cases.items():
            with self.subTest(message=message):
                (self.notes / 'x.md').write_text(text, encoding='utf-8')
                result = self.run_tool()
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(message, result.stderr)
                self.assertFalse(self.out.exists())
                self.assertFalse(self.changelog('en-US').exists())
                self.assertTrue((self.notes / 'x.md').exists())

    def test_an_existing_changelog_needs_force_and_keep_keeps_the_fragments(self):
        (self.notes / 'x.md').write_text('## en\n- A.\n## de\n- B.\n', encoding='utf-8')
        self.changelog('de').parent.mkdir(parents=True)
        self.changelog('de').write_text('published\n', encoding='utf-8')
        result = self.run_tool()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('exists', result.stderr)
        self.assertEqual(self.changelog('de').read_text(encoding='utf-8'), 'published\n')
        result = self.run_tool('--version-code', str(self.code + 1), '--keep')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.changelog('de', self.code + 1).read_text(encoding='utf-8'), '• B.\n')
        self.assertTrue((self.notes / 'x.md').exists())
        result = self.run_tool('--force')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.changelog('de').read_text(encoding='utf-8'), '• B.\n')
        self.assertFalse((self.notes / 'x.md').exists())


class Signing(unittest.TestCase):
    """The workflow, rust/release/sign-artifacts.ps1 and .signpath/ agree."""

    def setUp(self):
        self.workflow = (ROOT / '.github/workflows/rust-windows.yml').read_text(encoding='utf-8')
        self.configurations = {path.stem: ET.parse(path).getroot() for path in (ROOT / '.signpath').glob('*.xml')}

    def requests(self):
        """Each SignPath step's inputs, by artifact configuration slug."""
        steps = re.findall(r'(?ms)^        uses: SignPath/github-action-submit-signing-request@\w+\n(.*?)(?=^      - )', self.workflow)
        self.assertTrue(steps)
        return {re.search(r'artifact-configuration-slug: (\S+)', step)[1]: step for step in steps}

    def test_only_release_runs_sign(self):
        # Every SignPath request waits for a manual approval, so only the tag
        # job's release decision turns signing on.
        self.assertIn("SIGNPATH_ENABLED: ${{ needs.tag.outputs.sign == 'true' }}", self.workflow)
        self.assertIn("timeout-minutes: ${{ needs.tag.outputs.sign == 'true' && 240 || 75 }}", self.workflow)

    def test_each_request_names_a_configuration_and_the_version(self):
        requests = self.requests()
        self.assertEqual(set(requests), {'binaries', 'driver', 'installer'})
        for slug, step in requests.items():
            with self.subTest(slug=slug):
                self.assertIn(slug, self.configurations)
                self.assertIn('version: ${{ toJSON(env.RELEASE_VERSION) }}', step)
                self.assertIn('wait-for-completion: true', step)
                # Long enough for an approver to answer the e-mail.
                timeout = int(re.search(r'wait-for-completion-timeout-in-seconds: (\d+)', step)[1])
                self.assertGreaterEqual(timeout, 1800)

    def test_signed_files_name_rubylight_and_the_release(self):
        # The SignPath Foundation's metadata restriction.
        for slug, root in self.configurations.items():
            with self.subTest(slug=slug):
                parameter = root.find(f'{NS}parameters/{NS}parameter[@name="version"]')
                self.assertIsNotNone(parameter)
                self.assertEqual(parameter.get('required'), 'true')
                for element in root.iter():
                    if element.tag in (f'{NS}pe-file', f'{NS}pe-file-set'):
                        self.assertEqual(element.get('product-name'), 'Rubylight')
                        self.assertEqual(element.get('product-version'), '${version}')

    def test_binaries_configuration_lists_the_staged_binaries(self):
        script = (ROOT / 'rust/release/sign-artifacts.ps1').read_text(encoding='utf-8')
        staged = set(re.findall(r"'([^']+)'", re.search(r'(?s)\$binaries = @\((.*?)\)', script)[1]))
        includes = {element.get('path') for element in self.configurations['binaries'].iter(f'{NS}include')}
        self.assertEqual(includes - {'drivers/rubylight-display/*.dll'}, staged)

    def test_the_display_driver_is_packaged_where_setup_and_signing_look(self):
        # rust/build.ps1 lays the package out; setup and the signing steps
        # each name the same folder.
        build = (ROOT / 'rust/build.ps1').read_text(encoding='utf-8')
        self.assertIn(r"$distribution 'drivers\rubylight-display'", build)
        self.assertIn("'install-local.ps1'", build)
        self.assertIn(r"'sunshine\nefconc.exe'", build)
        self.assertIn(r"'rust\setup\remove-libvirtualdisplay.ps1'", build)
        # Rubylight's own driver replaced libvirtualdisplay, which is no
        # longer packaged; setup removes it from earlier installations.
        self.assertNotIn(r"'drivers\display'", build)
        self.assertNotIn('libvirtualdisplay v1.6.3', build)
        install = (ROOT / 'rust/setup/src/install.rs').read_text(encoding='utf-8')
        self.assertIn(r'"drivers\\rubylight-display\\remove-libvirtualdisplay.ps1"', install)
        self.assertNotIn(r'"drivers\\display\\install.ps1"', install)
        # A default build number lost to an installed development build: the
        # packaged driver carries the release version.
        self.assertIn("'build-package.ps1') -Version $driverVersion", build)
        script = r'drivers\\rubylight-display\\install-local.ps1'
        self.assertIn(f'"{script}"', (ROOT / 'rust/setup/src/install.rs').read_text(encoding='utf-8'))
        # The service sets the driver up again only when it is missing
        # (--repair-drivers), never after a settings edit: an unattended
        # uninstall after one broke measurements on the development PC.
        service = (ROOT / 'rust/windows/src/service.rs').read_text(encoding='utf-8')
        self.assertIn(r'"drivers\\rubylight-display\\install-local.ps1"', service)
        self.assertNotIn('--rubylight-display', service)
        self.assertNotIn('virtual_display_driver', service)
        signing = (ROOT / 'rust/release/sign-artifacts.ps1').read_text(encoding='utf-8')
        self.assertIn(r"$distribution 'drivers\rubylight-display'", signing)
        workflow = (ROOT / '.github/workflows/rust-windows.yml').read_text(encoding='utf-8')
        self.assertIn(r'butterpollo-rust-release\drivers\rubylight-display', workflow)
        # install-local.ps1 finds the package and nefconc.exe beside itself.
        installer = (ROOT / 'rust/drivers/rubylight-vdisplay/install-local.ps1').read_text(encoding='utf-8')
        self.assertIn("Join-Path $PSScriptRoot 'rubylight_vdisplay.inf'", installer)
        self.assertIn("Join-Path $PSScriptRoot 'nefconc.exe'", installer)
        # Removing a device the host still drives returns 3010: done at the
        # next restart, not a failure (setup's first removal on the
        # development PC failed on it).
        self.assertIn("Invoke-Restartable @('/remove-device'", installer)
        self.assertIn("Invoke-Restartable @('/delete-driver'", installer)
        self.assertNotRegex(installer, r"Invoke-Checked pnputil @\('/(remove-device|delete-driver)'")
        # A newer package in the driver store kept the device while its
        # signer lost its trust: the device must end on the package just
        # added, found by its INF's hash.
        self.assertIn('(Get-CurrentPackage) -ne $ours', installer)
        # Smart App Control, S mode and App Control (WDAC) refuse a driver
        # signed on the PC: setup says so instead of a generic failure, before
        # any certificate is made.
        for check in ('VerifiedAndReputablePolicyState -eq 1', 'SkuPolicyRequired -eq 1',
                      'UsermodeCodeIntegrityPolicyEnforcementStatus -eq 2', 'DRIVER_UNSUPPORTED: '):
            self.assertIn(check, installer)
        self.assertLess(installer.index('$refusal = if'), installer.index('Protect-Catalog (Join-Path $work'))
        self.assertIn("Get-Packages | Where-Object { $_ -ne $ours }", installer)
        # No half-installed device: a refusing PC loses an earlier install,
        # and a failed fresh install takes back what it added; both record
        # why for the console.
        refusal = installer[installer.index('$refusal = if'):installer.index('Protect-Catalog (Join-Path $work')]
        self.assertLess(refusal.index('Remove-Install $recorded'), refusal.index('Set-Problem $refusal'))
        rollback = installer[installer.index('    } catch {\n        $failure = $_'):]
        for step in ("pnputil /remove-device", "Where-Object { $_ -notin $before }", "Remove-Trust $signer.Thumbprint", "Set-Problem"):
            self.assertIn(step, rollback)


@unittest.skipUnless(os.name == 'nt', 'Windows PowerShell')
class DisplayDriverPolicy(unittest.TestCase):
    """install-local.ps1's Get-Refusal against the policies that refuse a
    driver signed on the PC, with the registry and WMI answers mocked."""

    def refusal(self, sac=0, sku=0, umci=0):
        installer = (ROOT / 'rust/drivers/rubylight-vdisplay/install-local.ps1').read_text(encoding='utf-8')
        start = installer.index('function Get-Refusal {')
        function = installer[start:installer.index('\n}\n', start) + 3]
        script = textwrap.dedent(f"""
            function Get-ItemProperty {{ [pscustomobject]@{{ VerifiedAndReputablePolicyState = {sac}; SkuPolicyRequired = {sku} }} }}
            function Get-CimInstance {{ [pscustomobject]@{{ UsermodeCodeIntegrityPolicyEnforcementStatus = {umci}; CodeIntegrityPolicyEnforcementStatus = 2 }} }}
            """) + function + "\n$r = Get-Refusal\nif ($r) { $r } else { 'NONE' }\n"
        result = subprocess.run(['powershell.exe', '-NoProfile', '-NonInteractive', '-Command', '-'],
                                input=script, capture_output=True, text=True, timeout=60)
        self.assertEqual(result.returncode, 0, result.stderr)
        return result.stdout.strip()

    def test_smart_app_control_on_refuses_and_says_what_to_do(self):
        text = self.refusal(sac=1)
        self.assertTrue(text.startswith('Smart App Control is on'), text)
        self.assertIn('signed Rubylight release', text)
        self.assertIn('physical display', text)

    def test_smart_app_control_evaluating_does_not_refuse(self):
        self.assertEqual(self.refusal(sac=2), 'NONE')

    def test_s_mode_refuses(self):
        self.assertTrue(self.refusal(sku=1).startswith('Windows runs in S mode'))

    def test_an_enforced_app_control_policy_refuses(self):
        self.assertTrue(self.refusal(umci=2).startswith('An App Control policy'))

    def test_memory_integrity_alone_does_not_refuse(self):
        # Kernel-mode code integrity (HVCI) is on in the mock; the driver
        # installs there, as on the development PC.
        self.assertEqual(self.refusal(), 'NONE')


class LinkCompare(unittest.TestCase):
    """The pure analysis of the link-speed harness (linkcompare.py): series, noise and the B8 verdicts."""

    @classmethod
    def setUpClass(cls):
        sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
        import linkcompare
        cls.lc = linkcompare

    def run_dict(self, p50=13.0, p99=26.0, lost=0.0, kbps=(14988,), rate=1000, **extra):
        run = dict(rate=rate, e2e_ms=dict(p50=p50, p99=p99), lost_percent=lost, freezes=dict(count=0, total_ms=0, longest_ms=0),
                   encoder_kbps_seen=list(kbps), encoder_kbps_negotiated=kbps[0], encoder_kbps_last20_mean=float(kbps[-1]),
                   adaptive_changes=dict(adaptive_bitrate=0, link_rate=0), freeze_events=[], series={},
                   pacing_selected=['network pacing selected pacing_bps=800000000'], pacing_kbps_series=[[0, 0], [1, 800000], [2, 800000]])
        run.update(extra)
        return run

    def judge2(self, *args):
        return self.lc.judge(*args, repeats=2)

    def failed(self, row):
        return sorted(c['name'] for c in row['checks'] if not c['ok'])

    def test_healthy_arms_are_the_nine_rate_and_bitrate_pairs(self):
        names = self.lc.hz_arm_names()
        self.assertEqual(len(names), 9)
        self.assertIn('hz-r1000-hevc-20', names)
        self.assertIn('hz-r300-hevc-80', names)
        self.assertEqual(self.lc.base_name('hz-r500-hevc-40@2'), 'hz-r500-hevc-40')

    def test_every_hz_arm_is_judged_as_healthy(self):
        for name in self.lc.hz_arm_names():
            self.assertEqual(self.lc.group_of(name), 'healthy')
        self.assertEqual(self.lc.group_of('r1000-av1-def'), 'healthy')
        self.assertEqual(self.lc.group_of('r150-hevc-hi'), 'above')
        self.assertEqual(self.lc.group_of('step-80-10-80-40m'), 'steps')

    def test_identical_healthy_runs_pass(self):
        row = self.judge2('hz-r1000-hevc-20', [self.run_dict()] * 2, [self.run_dict(p50=13.2)] * 2)
        self.assertEqual(row['verdict'], 'PASS', row['checks'])
        self.assertTrue(row['bitrate_equal'] and row['pacing_equal'])

    def test_healthy_p50_bound_is_two_sided_and_p99_is_one_sided(self):
        a = [self.run_dict()] * 2
        self.assertEqual(self.failed(self.judge2('hz-r1000-hevc-20', a, [self.run_dict(p50=13.4)] * 2)), ['p50'])
        self.assertEqual(self.failed(self.judge2('hz-r1000-hevc-20', a, [self.run_dict(p50=12.6)] * 2)), ['p50'])
        self.assertEqual(self.failed(self.judge2('hz-r1000-hevc-20', a, [self.run_dict(p99=27.5)] * 2)), ['p99'])
        self.assertEqual(self.judge2('hz-r1000-hevc-20', a, [self.run_dict(p99=20.0)] * 2)['verdict'], 'PASS')

    def test_a_bitrate_change_or_different_pacing_fails_a_healthy_arm(self):
        a = [self.run_dict()] * 2
        changed = self.run_dict(kbps=(14988, 9000), adaptive_changes=dict(adaptive_bitrate=0, link_rate=2))
        self.assertEqual(self.failed(self.judge2('hz-r1000-hevc-20', a, [changed] * 2)), ['bitrate'])
        slower = self.run_dict(pacing_selected=['network pacing selected pacing_bps=30000000'])
        self.assertEqual(self.failed(self.judge2('hz-r1000-hevc-20', a, [slower] * 2)), ['pacing'])
        paced = self.run_dict(pacing_kbps_series=[[0, 0], [1, 30000]])
        self.assertEqual(self.failed(self.judge2('hz-r1000-hevc-20', a, [paced] * 2)), ['pacing'])

    def test_a_baseline_from_before_the_pacing_series_matches_on_the_pacing_line_alone(self):
        old = self.run_dict()
        del old['pacing_kbps_series']
        row = self.judge2('hz-r1000-hevc-20', [old] * 2, [self.run_dict()] * 2)
        self.assertTrue(row['pacing_equal'])
        slower = self.run_dict(pacing_selected=['network pacing selected pacing_bps=30000000'])
        self.assertEqual(self.failed(self.judge2('hz-r1000-hevc-20', [old] * 2, [slower] * 2)), ['pacing'])

    def test_a_candidate_without_the_pacing_series_fails_against_a_baseline_with_it(self):
        lost = self.run_dict()
        del lost['pacing_kbps_series']
        a = [self.run_dict()] * 2
        row = self.judge2('hz-r1000-hevc-20', a, [lost] * 2)
        self.assertFalse(row['pacing_equal'])
        self.assertEqual(self.failed(row), ['pacing'])

    def test_a_field_new_on_one_side_of_the_pacing_line_is_not_a_difference(self):
        old = 'network pacing selected pacing_bps=800000000 wire_budget_bps=20173441 link_bps=2500000000 configured_kbps=0'
        new = 'network pacing selected pacing_bps=800000000 wire_budget_bps=20173441 link_bps=2500000000 client_link_mbps=0 configured_kbps=0'
        a = [self.run_dict(pacing_selected=[old])] * 2
        row = self.judge2('hz-r1000-hevc-20', a, [self.run_dict(pacing_selected=[new])] * 2)
        self.assertTrue(row['pacing_equal'])
        self.assertEqual(row['verdict'], 'PASS', row['checks'])
        # The pacing values themselves still count, and so does the number of lines.
        for text in (new.replace('pacing_bps=800000000', 'pacing_bps=30000000'),
                     new.replace('link_bps=2500000000', 'link_bps=1000000000'),
                     new.replace('wire_budget_bps=20173441', 'wire_budget_bps=1')):
            row = self.judge2('hz-r1000-hevc-20', a, [self.run_dict(pacing_selected=[text])] * 2)
            self.assertEqual(self.failed(row), ['pacing'], text)
        row = self.judge2('hz-r1000-hevc-20', a, [self.run_dict(pacing_selected=[new, new])] * 2)
        self.assertEqual(self.failed(row), ['pacing'])

    def test_loss_worse_than_the_baseline_fails(self):
        row = self.judge2('hz-r300-hevc-80', [self.run_dict(lost=0.0)] * 2, [self.run_dict(lost=0.2)] * 2)
        self.assertEqual(self.failed(row), ['loss'])

    def test_too_few_repeats_do_not_pass(self):
        row = self.lc.judge('hz-r1000-hevc-20', [self.run_dict()], [self.run_dict()], repeats=2)
        self.assertEqual((row['verdict'], self.failed(row)), ('FAIL', ['repeats']))
        self.assertEqual(self.lc.judge('hz-r1000-hevc-20', [self.run_dict()], [self.run_dict()], repeats=1)['verdict'], 'PASS')

    def test_median_over_repeats_absorbs_one_noisy_run(self):
        a = [self.run_dict(p50=13.0), self.run_dict(p50=13.1), self.run_dict(p50=13.0)]
        b = [self.run_dict(p50=13.1), self.run_dict(p50=14.5), self.run_dict(p50=13.2)]
        self.assertEqual(self.lc.judge('hz-r1000-hevc-20', a, b, repeats=3)['verdict'], 'PASS')

    def test_a_run_without_data_fails(self):
        row = self.lc.judge('hz-r1000-hevc-20', [self.run_dict()] * 2, [dict(error='no frames.csv')] * 2)
        self.assertEqual(row['verdict'], 'FAIL')

    def test_above_the_link_needs_low_late_loss_short_freezes_and_a_bitrate_near_the_fit(self):
        fit = self.lc.fit_encoder_kbps(150, 450000)
        good_series = {str(s): dict(sent=60, delivered=60) for s in range(10)}
        good = self.run_dict(rate=150, kbps=(450000, fit), series=good_series, lost=3.0, encoder_kbps_last20_mean=0.8 * fit)
        a = self.run_dict(rate=150, lost=99.0, kbps=(450000,))
        self.assertEqual(self.lc.judge('r150-hevc-hi', [a], [good], repeats=1)['verdict'], 'PASS')
        low = dict(good, encoder_kbps_last20_mean=0.5 * fit)
        self.assertEqual(self.failed(self.lc.judge('r150-hevc-hi', [a], [low], repeats=1)), ['bitrate'])
        late = {str(s): dict(sent=60, delivered=60 if s < 5 else 50) for s in range(10)}
        self.assertEqual(self.failed(self.lc.judge('r150-hevc-hi', [a], [dict(good, series=late)], repeats=1)), ['loss after 3 s'])
        frozen = dict(good, freeze_events=[[4.0, 5.5, 1500.0]])
        self.assertEqual(self.failed(self.lc.judge('r150-hevc-hi', [a], [frozen], repeats=1)), ['freeze after 3 s'])
        early = dict(good, freeze_events=[[0.5, 2.0, 1500.0]])
        self.assertEqual(self.lc.judge('r150-hevc-hi', [a], [early], repeats=1)['verdict'], 'PASS')

    def test_at_the_link_rate_limits_freezes_loss_and_bitrate(self):
        fit = self.lc.fit_encoder_kbps(20, 14988)
        ok = self.run_dict(rate=20, lost=1.0, kbps=(14988, fit), encoder_kbps_last20_mean=0.9 * fit,
                           freezes=dict(count=3, total_ms=900.0, longest_ms=400.0))
        a = self.run_dict(rate=20, lost=30.0)
        self.assertEqual(self.lc.judge('r20-hevc-def', [a], [ok], repeats=1)['verdict'], 'PASS')
        many = dict(ok, freezes=dict(count=4, total_ms=900.0, longest_ms=400.0))
        self.assertEqual(self.failed(self.lc.judge('r20-hevc-def', [a], [many], repeats=1)), ['freezes'])
        long = dict(ok, freezes=dict(count=2, total_ms=1200.0, longest_ms=700.0))
        self.assertEqual(self.failed(self.lc.judge('r20-hevc-def', [a], [long], repeats=1)), ['freezes'])
        self.assertEqual(self.failed(self.lc.judge('r20-hevc-def', [a], [dict(ok, lost_percent=2.5)], repeats=1)), ['loss'])

    def test_step_arm_needs_short_freezes_and_a_recovery_within_20_seconds(self):
        settle = lambda back: [dict(rate=10, at_s=20.0), dict(rate=80, at_s=40.0, encoder_recovered_90_s=back)]
        ok = self.run_dict(rate=80, lost=1.5, phase_freeze_ms=[0.0, 900.0, 0.0], step_settle=settle(12.0))
        a = self.run_dict(rate=80, lost=29.0)
        self.assertEqual(self.lc.judge('step-80-10-80', [a], [ok], repeats=1)['verdict'], 'PASS')
        self.assertEqual(self.failed(self.lc.judge('step-80-10-80', [a], [dict(ok, phase_freeze_ms=[0, 1600.0, 0])], repeats=1)), ['freeze per step'])
        self.assertEqual(self.failed(self.lc.judge('step-80-10-80', [a], [dict(ok, step_settle=settle(None))], repeats=1)), ['rate back'])
        self.assertEqual(self.failed(self.lc.judge('step-80-10-80', [a], [dict(ok, step_settle=settle(25.0))], repeats=1)), ['rate back'])
        self.assertEqual(self.failed(self.lc.judge('step-80-10-80', [a], [dict(ok, lost_percent=3.5)], repeats=1)), ['loss'])

    def test_arms_that_must_not_cut(self):
        a = self.run_dict(rate=150)
        self.assertEqual(self.lc.judge('wifi', [a], [self.run_dict(rate=150)], repeats=1)['verdict'], 'PASS')
        cut = self.run_dict(rate=150, adaptive_changes=dict(adaptive_bitrate=0, link_rate=1))
        self.assertEqual(self.failed(self.lc.judge('wifi', [a], [cut], repeats=1)), ['no change'])

    def test_unchanged_4k_arm_may_improve_but_not_regress(self):
        a = self.run_dict(p50=34.6, p99=91.4, lost=13.7)
        better = self.run_dict(p50=22.0, p99=40.0, lost=0.0)
        self.assertEqual(self.lc.judge('r1000-hevc-4k', [a], [better], repeats=1)['verdict'], 'PASS')
        worse = self.run_dict(p50=35.5, p99=91.4, lost=13.7)
        self.assertEqual(self.failed(self.lc.judge('r1000-hevc-4k', [a], [worse], repeats=1)), ['p50'])

    def test_fit_matches_the_design_numbers(self):
        # The design quotes a stream cut to about 13 Mbps on a 20 Mbit/s link.
        self.assertTrue(12500 <= self.lc.fit_encoder_kbps(20, 14988) <= 13500)
        self.assertEqual(self.lc.fit_encoder_kbps(1000, 14988), 14988)
        self.assertLess(self.lc.video_wire_bps(self.lc.fit_encoder_kbps(150, 450000)), 150e6 * 0.9)
        self.assertGreater(self.lc.fit_encoder_kbps(300, 450000), self.lc.fit_encoder_kbps(150, 450000))

    def test_fit_agrees_with_the_host_wire_budget(self):
        # Host log, 20 Mbps client: 14988 kbps of video is about 20.17 Mbps of wire (wire_budget_bps in "network pacing selected").
        self.assertAlmostEqual(self.lc.video_wire_bps(14988) / 20173441, 1.0, delta=0.02)

    def test_mean_of_the_last_twenty_seconds(self):
        series = [[t, 100.0] for t in range(0, 30)] + [[t, 10.0] for t in range(30, 51)]
        self.assertAlmostEqual(self.lc.mean_last(series), 10.0)
        self.assertAlmostEqual(self.lc.mean_last([[0, 5.0], [10, 15.0]]), 10.0)
        self.assertIsNone(self.lc.mean_last([]))

    def test_recovery_time_is_measured_from_the_link_returning(self):
        series = [[38, 3000], [40, 3400], [41, 4000], [45, 14000], [46, 14500]]
        self.assertEqual(self.lc.recovery_time(series, 40.0, 13489), 5.0)
        self.assertIsNone(self.lc.recovery_time(series, 40.0, 15000))
        self.assertEqual(self.lc.recovery_time(series, 40.0, 3000), 0.0)

    def test_a_freeze_across_a_step_belongs_to_the_phase_after_it(self):
        # Frames sent at 19.9 s and 40.0 s with 20.1 s between their display times, steps at 20 s and 40 s.
        events = self.lc.freeze_events([(0, 19.9), (20_100_000, 40.0), (20_116_000, 40.02)], 100.0)
        self.assertEqual(len(events), 1)
        longest = self.lc.phase_freezes(events, [20.0, 40.0], 60)
        self.assertLessEqual(longest[0], 100.5)
        self.assertGreater(longest[1], 19_900.0)
        self.assertEqual(longest[2], 0.0)

    def test_short_gaps_are_not_freezes(self):
        self.assertEqual(self.lc.freeze_events([(0, 0.0), (16_000, 0.016), (40_000, 0.04)], 100.0), [])

    def test_loss_after_a_second_counts_only_later_bins(self):
        series = {'0': dict(sent=60, delivered=0), '3': dict(sent=60, delivered=59), '4': dict(sent=60, delivered=60)}
        self.assertEqual(self.lc.loss_after(series, 3), round(100 / 120, 2))
        self.assertIsNone(self.lc.loss_after(series, 9))

    def test_noise_gives_the_repeats_a_bound_needs(self):
        runs = {'hz-r1000-hevc-20': [self.run_dict(p50=12.9), self.run_dict(p50=13.1)],
                'hz-r500-hevc-20': [self.run_dict(p50=13.0), self.run_dict(p50=13.3)], 'lone': [self.run_dict(p50=1.0)]}
        stats = self.lc.noise_stats(runs)
        self.assertEqual(sorted(stats['arms']), ['hz-r1000-hevc-20', 'hz-r500-hevc-20'])
        self.assertAlmostEqual(stats['max_range'], 0.3, places=3)
        self.assertAlmostEqual(stats['pooled_sigma'], ((0.2 ** 2 / 2 + 0.3 ** 2 / 2) / 2) ** 0.5, places=3)
        self.assertGreaterEqual(stats['repeats_worst'], stats['repeats_pooled'])

    def test_repeats_needed_grows_with_the_square_of_the_spread(self):
        self.assertEqual(self.lc.repeats_needed(0.0), 1)
        self.assertEqual(self.lc.repeats_needed(0.05), 1)
        self.assertEqual(self.lc.repeats_needed(0.2), 4)
        self.assertEqual(self.lc.repeats_needed(0.4), 14)

    def test_pooled_delta_over_healthy_arms(self):
        a = self.run_dict(p50=13.0)
        rows = [self.lc.judge('hz-r1000-hevc-20', [a], [self.run_dict(p50=13.2)], repeats=1),
                self.lc.judge('hz-r500-hevc-20', [a], [self.run_dict(p50=12.8)], repeats=1),
                self.lc.judge('wifi', [a], [self.run_dict(p50=20.0)], repeats=1)]
        self.assertEqual(self.lc.pooled_healthy_delta(rows), (0.0, 2))
        self.assertIn('mean delta p50 over 2 healthy arms', self.lc.format_rows(rows))
        self.assertGreaterEqual(self.lc.DEFAULT_REPEATS, self.lc.repeats_needed(0.5))

    def test_load_runs_groups_repeats_by_arm(self):
        with tempfile.TemporaryDirectory() as t:
            for folder, p50 in (('hz-r1000-hevc-20', 13.0), ('hz-r1000-hevc-20@2', 13.1), ('wifi', 16.0)):
                d = pathlib.Path(t, folder); d.mkdir()
                (d / 'result.json').write_text(json.dumps(self.run_dict(p50=p50)))
            pathlib.Path(t, 'motion.log').write_text('x')
            runs = self.lc.load_runs(t)
            self.assertEqual({k: len(v) for k, v in runs.items()}, {'hz-r1000-hevc-20': 2, 'wifi': 1})
            rows = self.lc.compare([t], [t], repeats=1)
            self.assertEqual([r['verdict'] for r in rows], ['PASS', 'PASS'])
            self.assertIn('hz-r1000-hevc-20', self.lc.format_rows(rows))


NS = '{http://signpath.io/artifact-configuration/v1}'


class PortedCredits(unittest.TestCase):
    """Code ported from Foundation Sunshine is credited by file in NOTICE and AGENTS.md."""

    PORTED = ['rust/core/src/hdr_dynamic.rs', 'rust/windows/src/shaders/color.hlsl',
              'rust/windows/src/compute_hdr_stats.rs']

    def test_notice_and_agents_name_every_ported_hdr_file(self):
        notice = ' '.join((ROOT / 'NOTICE').read_text(encoding='utf-8').split())
        agents = (ROOT / 'AGENTS.md').read_text(encoding='utf-8').splitlines()[2]
        for path in self.PORTED:
            self.assertIn('Foundation Sunshine', (ROOT / path).read_text(encoding='utf-8'), path)
            self.assertIn(path, notice)
            self.assertIn(path, agents)
        self.assertIn('hdr_luminance_analysis_cs.hlsl', notice)
        self.assertIn('hdr_stats_cs', notice)
        self.assertIn('hdr_stats_cs', agents)
        # The earlier credits stay.
        self.assertIn('video_hdr_metadata.cpp and video_hdr_bitstream.cpp', notice)
        self.assertIn('commit 3340d28', notice)

    def test_release_notes_and_setting_text_carry_no_port_credits(self):
        # Credits live in the code, NOTICE, AGENTS.md and the README, not in what users read per release.
        texts = [*sorted((ROOT / 'rust/release/notes.d').glob('*.md')),
                 *sorted((ROOT / 'client/notes.d').glob('*.md')),
                 *sorted((ROOT / 'rust/web/src/lib/schema').glob('*.ts'))]
        self.assertTrue(texts)
        for path in texts:
            # Code comments may credit; what the console shows may not.
            text = '\n'.join(line for line in path.read_text(encoding='utf-8').splitlines()
                             if not line.lstrip().startswith(('//', '*', '/*')))
            for name in ('Foundation Sunshine', 'AlkaidLab', 'Annex'):
                self.assertNotIn(name, text, path)


if __name__ == '__main__':
    unittest.main()
