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
        for new in ('3.0.0-alpha.1', '3.0.0-alpha.2', '3.0.0-beta.1'):
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
        self.assertIn("timeout-minutes: ${{ needs.tag.outputs.sign == 'true' && 240 || 50 }}", self.workflow)

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


NS = '{http://signpath.io/artifact-configuration/v1}'


if __name__ == '__main__':
    unittest.main()
