"""Tests for tools/scaffold.py and .agents/skills/ integrity."""

import contextlib
import copy
import io
import json
import pathlib
import re
import shutil
import subprocess
import sys
import tempfile
import tomllib
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import arch_check  # noqa: E402
import generated_docs  # noqa: E402
import scaffold  # noqa: E402

ROOT = pathlib.Path(__file__).resolve().parents[1]
EXPECTED_SKILLS = (
    'add-setting',
    'add-control-message',
    'add-native-endpoint',
    'check-nvenc',
    'isolated-e2e',
    'release-prep',
    'issue-triage',
    'device-checks',
)


def run_cli(argv):
    stdout = io.StringIO()
    stderr = io.StringIO()
    with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
        rc = scaffold.main(argv)
    return rc, stdout.getvalue(), stderr.getvalue()


def parse_entry(out):
    block = out.split('(rust/core/config/settings.toml) ===\n', 1)[1].split('=== 2.', 1)[0]
    return tomllib.loads(block)['setting'][0]


class ScaffoldTests(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.root = pathlib.Path(self._tmp.name)

    def _assert_rust_conventions(self, path):
        raw = path.read_bytes()
        self.assertNotIn(b'\r\n', raw, f'{path} must use LF line endings')
        text = raw.decode('utf-8')
        self.assertTrue(arch_check.has_rust_module_header(text), f'{path} missing //! header')

    def test_control_message_dry_run_collision_and_compile(self):
        # Collision check against real repo's ids.rs (0x5508 is clipboard sync)
        rc, _, err = run_cli(['--root', str(ROOT), 'control-message', 'dup_msg', '--id', '0x5508', '--dry-run'])
        self.assertEqual(rc, 1)
        self.assertIn('already registered', err)

        # Dry run writes nothing
        rc, out, err = run_cli([
            '--root', str(self.root), 'control-message', 'sample_ping',
            '--id', '0x5599', '--launch-param', 'rlSamplePing', '--dry-run',
        ])
        self.assertEqual(rc, 0, out + err)
        self.assertIn('[dry-run]', out)
        target = self.root / 'client/protocol/core/src/sample_ping.rs'
        self.assertFalse(target.exists())

        # Real write creates valid Rust module
        rc, out, err = run_cli([
            '--root', str(self.root), 'control-message', 'sample_ping',
            '--id', '0x5599', '--launch-param', 'rlSamplePing',
        ])
        self.assertEqual(rc, 0, out + err)
        self.assertTrue(target.is_file())
        self._assert_rust_conventions(target)
        text = target.read_text(encoding='utf-8')
        self.assertIn('pub const SAMPLE_PING_MESSAGE_TYPE: u16 = 0x5599;', text)
        self.assertIn('pub const LAUNCH_PARAMETER: &str = "rlSamplePing";', text)

        # Refuses overwrite without --force, allows with --force
        rc, _, err = run_cli(['--root', str(self.root), 'control-message', 'sample_ping', '--id', '0x5599'])
        self.assertEqual(rc, 1)
        self.assertIn('refusing to overwrite', err)
        rc, _, err = run_cli(['--root', str(self.root), 'control-message', 'sample_ping', '--id', '0x5599', '--force'])
        self.assertEqual(rc, 0, err)

        # Compile and run generated unit tests if rustc is installed
        rustc = shutil.which('rustc') or shutil.which('rustc', path=str(pathlib.Path.home() / '.cargo/bin'))
        if rustc:
            bin_path = self.root / ('sample_ping_test.exe' if sys.platform == 'win32' else 'sample_ping_test')
            comp = subprocess.run(
                [rustc, '--edition=2021', '--test', str(target), '-o', str(bin_path)],
                capture_output=True,
                text=True,
            )
            self.assertEqual(comp.returncode, 0, comp.stderr)
            run = subprocess.run([str(bin_path)], capture_output=True, text=True)
            self.assertEqual(run.returncode, 0, run.stdout + run.stderr)

    def test_native_endpoint_get_and_post(self):
        # GET dry-run
        rc, out, err = run_cli(['--root', str(self.root), 'native-endpoint', 'display_info', '--dry-run'])
        self.assertEqual(rc, 0, out + err)
        self.assertEqual(list(self.root.rglob('*')), [])

        # POST real write
        rc, out, err = run_cli([
            '--root', str(self.root), 'native-endpoint', 'audio_boost',
            '--method', 'post', '--permission', 'host_control',
        ])
        self.assertEqual(rc, 0, out + err)
        expected_files = (
            'client/protocol/native-api/src/audio_boost.rs',
            'client/protocol/native-api/tests/fixtures/audio_boost.json',
            'rust/host/web/src/native_api/audio_boost.rs',
            'tests/host/tests/nvhttp_parity/native_tests/audio_boost.rs',
        )
        for rel in expected_files:
            p = self.root / rel
            self.assertTrue(p.is_file(), f'missing {rel}')
            self.assertNotIn(b'\r\n', p.read_bytes())
            if rel.endswith('.rs'):
                self._assert_rust_conventions(p)
        wire = (self.root / expected_files[0]).read_text(encoding='utf-8')
        self.assertIn('#[serde(deny_unknown_fields)]', wire)
        self.assertIn('#[serde(other)]', wire)
        fixture = json.loads((self.root / expected_files[1]).read_text(encoding='utf-8'))
        self.assertEqual(fixture['state'], 'ready')
        handler = (self.root / expected_files[2]).read_text(encoding='utf-8')
        self.assertIn('require_host_control(&client)?;', handler)
        self.assertIn('read_body(request).await?', handler)
        self.assertIn('ErrorCode::InvalidRequest', handler)
        self.assertNotIn('parse_body', handler)
        parity = (self.root / expected_files[3]).read_text(encoding='utf-8')
        self.assertIn('s.host_control(true);', parity)
        self.assertIn('send(&s.f, true, &s.connection, "POST", &format!("{PREFIX}/audio-boost")', parity)
        self.assertIn('.assert_error(403, "host_control_required");', parity)
        self.assertNotIn('post_json', parity)
        self.assertIn('.route(&path("/audio-boost"), post(audio_boost::write).fallback(unknown))', out)

    def test_setting_types_validation_and_note(self):
        # Bool + note dry-run
        rc, out, err = run_cli([
            '--root', str(self.root), 'setting', 'fast_resume',
            '--type', 'bool', '--default', 'true', '--category', 'general', '--group', 'Power',
            '--label', 'Fast resume', '--description', 'Resumes streams without resetting capture.',
            '--with-note', '--dry-run',
        ])
        self.assertEqual(rc, 0, out + err)
        entry = parse_entry(out)
        self.assertEqual(
            (entry['part'], entry['type'], entry['default'], entry['category']),
            ('basics_core', 'bool', True, 'general'),
        )
        self.assertEqual(entry['control'], {'kind': 'toggle'})
        self.assertEqual(entry['console']['control'], 'bool')
        self.assertEqual(entry['rust_accessor']['kind'], 'bool')
        self.assertIn('Insert "fast_resume" in [console] global_order next to', out)
        self.assertIn('[[docs.sections]] entry "### This PC"', out)
        self.assertIn('catalog_parity.rs: settings (toml_keys), GLOBAL console rows\n', out)
        self.assertIn('python tools/generated_docs.py', out)
        self.assertIn('python tools/i18n.py apply <spec.json>', out)
        self.assertNotIn('[rust]', out)
        self.assertFalse((self.root / 'rust/release/notes.d/fast_resume.md').exists())

        # Choice + real note write, then --extension with the order-list flags
        rc, out, err = run_cli([
            '--root', str(self.root), 'setting', 'capture_mode',
            '--type', 'choice', '--choices', 'auto,wgc,dd', '--default', 'auto',
            '--category', 'frame-limiting', '--group', 'Frame limiter',
            '--label', 'Capture mode', '--description', 'Selects capture API.',
            '--with-note',
        ])
        self.assertEqual(rc, 0, out + err)
        entry = parse_entry(out)
        self.assertEqual((entry['type'], entry['default']), ('enum', 'auto'))
        self.assertEqual([o['value'] for o in entry['control']['options']], ['auto', 'wgc', 'dd'])
        self.assertEqual(entry['console']['choices'], ['auto', 'wgc', 'dd'])
        self.assertEqual(entry['category'], 'frame-limiting')
        note = self.root / 'rust/release/notes.d/capture_mode.md'
        self.assertTrue(note.is_file())
        self.assertNotIn(b'\r\n', note.read_bytes())

        rc, out, err = run_cli([
            '--root', str(self.root), 'setting', 'ext_flag',
            '--type', 'bool', '--default', 'false', '--category', 'input', '--group', 'Controller',
            '--label', 'Ext flag', '--description', 'Extension option.', '--dry-run',
            '--settings-30', '--override-allowed', '--restart',
        ])
        self.assertEqual(rc, 0, out + err)
        entry = parse_entry(out)
        self.assertEqual((entry['part'], entry['category']), ('extensions', 'input'))
        self.assertEqual(entry['settings_30_default'], 'false')
        self.assertTrue(entry['settings_30'] and entry['override_allowed'] and entry['restart'])
        self.assertNotIn('console', entry)
        self.assertNotIn('global_order', out)
        for order in ('settings_30_order', 'override_order', 'restart_order'):
            self.assertIn(f'Add "ext_flag" to [rust] {order}', out)
        self.assertIn('catalog_parity.rs: settings (toml_keys), overrides, restarts, SETTINGS_30\n', out)
        self.assertIn('settings30.rs', out)
        self.assertIn('[[docs.sections]] entry "### Sound & controls"', out)

        # --group is required
        with self.assertRaises(SystemExit):
            run_cli(['setting', 'x', '--type', 'bool', '--default', 'true', '--label', 'X', '--description', 'Y'])

        # Encoder console groups follow the vendor
        for vendor, group in (('amd', 'AMD encoding'), ('nvidia', 'NVIDIA encoding')):
            rendered, _ = scaffold.generate_setting_stub(
                'x', 'bool', 'true', 'X', 'Y', group='G', category='encoders', vendor=vendor,
            )
            self.assertEqual(parse_entry(rendered)['console']['group'], group)
            self.assertIn(f'other "{group}" keys', rendered)

        # Validation failures (including invalid CategoryId like schema filenames 'basics'/'extensions')
        with self.assertRaises(ValueError):
            scaffold.generate_setting_stub('x', 'bool', 'true', 'X', 'Y', group='G', category='basics')
        with self.assertRaises(ValueError):
            scaffold.generate_setting_stub('x', 'bool', 'true', 'X', 'Y', group='G', category='extensions')
        # Every extensions row is a 3.0 setting, and a 3.0 setting has no console group to pick.
        with self.assertRaises(ValueError):
            scaffold.generate_setting_stub('x', 'bool', 'true', 'X', 'Y', group='G', extension=True)
        with self.assertRaises(ValueError):
            scaffold.generate_setting_stub('x', 'bool', 'true', 'X', 'Y', group='G', category='encoders',
                                           vendor='amd', settings_30=True)
        rendered, _ = scaffold.generate_setting_stub('x', 'bool', 'true', 'X', 'Y', group='G', settings_30=True)
        self.assertIn('pending = true', rendered)
        self.assertIn('pending.length', rendered)
        self.assertIn('parts.length', rendered)
        rc, _, _ = run_cli([
            '--root', str(self.root), 'setting', 'BadKey',
            '--type', 'bool', '--default', 'true', '--group', 'G', '--label', 'X', '--description', 'Y',
        ])
        self.assertEqual(rc, 1)
        rc, _, _ = run_cli([
            '--root', str(self.root), 'setting', 'bad_int',
            '--type', 'int', '--default', '500', '--min', '1', '--max', '100', '--group', 'G', '--label', 'X', '--description', 'Y',
        ])
        self.assertEqual(rc, 1)

    def test_setting_entry_uses_only_catalog_fields(self):
        catalog = tomllib.loads((ROOT / 'rust/core/config/settings.toml').read_text(encoding='utf-8'))
        used = {}
        for existing in catalog['setting']:
            for field, value in existing.items():
                used.setdefault(field, set())
                if isinstance(value, dict):
                    used[field].update(value)
        text = 'say "hi" \\ there'
        cases = (
            ('bool', 'true', {}),
            ('int', '5', {'min_val': 1, 'max_val': 10}),
            ('choice', 'a', {'choices': ('a', 'b')}),
            ('text', text, {}),
        )
        for kind, default, extra in cases:
            rendered, _ = scaffold.generate_setting_stub(
                'demo_flag', kind, default, 'Demo', 'Demo help.', group='G',
                settings_30=True, override_allowed=True, restart=True, **extra,
            )
            entry = parse_entry(rendered)
            self.assertLessEqual(set(entry), set(used), kind)
            for field, value in entry.items():
                if isinstance(value, dict):
                    self.assertLessEqual(set(value), used[field], f'{kind} {field}')
            if kind == 'text':
                self.assertEqual(entry['default'], text)

    def test_setting_entries_render_with_generated_docs(self):
        settings = ROOT / generated_docs.SETTINGS_TOML
        before = settings.read_bytes()
        catalog = generated_docs.load_catalog(ROOT)
        existing = catalog['setting']
        parts = {s.get('part') for s in existing}
        types = {s['type'] for s in existing}
        accessors = {s['rust_accessor']['kind'] for s in existing if 'rust_accessor' in s}
        controls = {s['console']['control'] for s in existing if 'console' in s}
        cases = (
            ('bool', 'true', {}),
            ('int', '5', {'min_val': 1, 'max_val': 10}),
            ('int', '5', {'min_val': 1}),
            ('choice', 'a', {'choices': ('a', 'b')}),
            ('text', 'hello', {}),
        )
        for kind, default, extra in cases:
            with self.subTest(kind=kind, **extra):
                entry = parse_entry(scaffold.generate_setting_stub(
                    'demo_flag', kind, default, 'Demo', 'Demo help.', group='Capture',
                    override_allowed=True, restart=True, **extra,
                )[0])
                self.assertIn(entry['part'], parts)
                self.assertIn(entry['type'], types)
                self.assertIn(entry['console']['control'], controls)
                accessor = entry.get('rust_accessor')
                if accessor:
                    self.assertIn(accessor['kind'], accessors)
                if 'max_val' in extra:
                    self.assertEqual(
                        (accessor['kind'], accessor['ret'], accessor['min'], accessor['max']),
                        ('clamped_int', 'u64', 1, 10),
                    )
                elif kind == 'int':
                    self.assertIsNone(accessor)

                cat = copy.deepcopy(catalog)
                cat['setting'].append(entry)
                cat['_by_key']['demo_flag'] = entry
                cat['rust']['override_order'].append('demo_flag')
                cat['rust']['restart_order'].append('demo_flag')
                order = cat['console']['global_order']
                group = entry['console']['group']
                last = max(i for i, k in enumerate(order) if cat['_by_key'][k]['console']['group'] == group)
                order.insert(last + 1, 'demo_flag')
                next(s for s in cat['docs']['sections'] if s['heading'] == '### Picture')['keys'].append('demo_flag')
                rendered = {}
                for doc in generated_docs.DOCS:
                    if doc.render is not None:
                        path = ROOT / doc.path
                        current = path.read_text(encoding='utf-8') if path.exists() else None
                        rendered[doc.path] = doc.render(cat, current)
                for path in ('rust/core/config/src/config/catalog_gen.rs' if accessor else
                             'rust/core/config/src/config/console_global_gen.rs',
                             'rust/web/src/lib/schema/video_stream.gen.ts', 'docs/configuration.md'):
                    self.assertIn('demo_flag', rendered[path], path)
        self.assertEqual(settings.read_bytes(), before)

    def test_setting_output_never_clobbers(self):
        settings = ROOT / generated_docs.SETTINGS_TOML
        before = settings.read_bytes()
        base = [
            'setting', 'demo_flag', '--type', 'bool', '--default', 'true', '--group', 'G',
            '--label', 'X', '--description', 'Y', '--output',
        ]
        target = self.root / 'snippets.txt'
        target.write_text('keep', encoding='utf-8')
        rc, _, err = run_cli([*base, str(target)])
        self.assertEqual(rc, 1)
        self.assertIn('refusing to overwrite', err)
        self.assertEqual(target.read_text(encoding='utf-8'), 'keep')
        rc, _, err = run_cli([*base, str(target), '--force'])
        self.assertEqual(rc, 0, err)
        self.assertIn('[[setting]]', target.read_text(encoding='utf-8'))

        detour = ROOT / 'rust/core/../core/config/settings.toml'
        for path in (settings, detour):
            for flags in ((), ('--force',), ('--force', '--dry-run')):
                rc, _, err = run_cli([*base, str(path), *flags])
                self.assertEqual(rc, 1, (path, flags))
                self.assertIn('must not be rust/core/config/settings.toml', err)
        self.assertEqual(settings.read_bytes(), before)

    def test_test_scenario_all_suites(self):
        suites = {
            'reconnect': 'tests/host/tests/reconnect_tests/my_reconnect.rs',
            'native': 'tests/host/tests/nvhttp_parity/native_tests/my_native.rs',
            'config': 'rust/core/config/src/config/tests/my_config.rs',
            'interop': 'tests/interop/tests/my_interop.rs',
        }
        for suite, rel in suites.items():
            name = pathlib.PurePosixPath(rel).stem
            rc, out, err = run_cli(['--root', str(self.root), 'test-scenario', name, '--suite', suite])
            self.assertEqual(rc, 0, out + err)
            p = self.root / rel
            self.assertTrue(p.is_file(), f'missing {rel}')
            self._assert_rust_conventions(p)


def expand_braces(ref):
    m = re.search(r'\{([^{}]+)\}', ref)
    if not m:
        return [ref]
    head, tail = ref[:m.start()], ref[m.end():]
    expanded = []
    for part in m.group(1).split(','):
        expanded.extend(expand_braces(head + part.strip() + tail))
    return expanded


class SkillsIntegrityTests(unittest.TestCase):
    def test_all_skills_exist_with_valid_frontmatter_lf_and_grounded_paths(self):
        skills_dir = ROOT / '.agents/skills'
        path_re = re.compile(r'`((?:rust|client|docs|tools|tests|\.agents)/[\w./{},-]+)`')
        for name in EXPECTED_SKILLS:
            path = skills_dir / name / 'SKILL.md'
            self.assertTrue(path.is_file(), f'missing skill {path}')
            raw = path.read_bytes()
            self.assertNotIn(b'\r\n', raw, f'{path} must use LF line endings')
            text = raw.decode('utf-8')
            self.assertLessEqual(len(text.splitlines()), 150, f'{path} should stay concise (<=150 LOC)')
            m = re.match(r'^---\n(.*?)\n---\n', text, re.S)
            self.assertIsNotNone(m, f'{path} missing YAML frontmatter')
            fm = m.group(1)
            self.assertIn(f'name: {name}', fm)
            self.assertIn('description:', fm)
            # Expand {a,b} brace groups and verify every concrete cited repo path exists!
            for ref in path_re.findall(text):
                if any(tok in ref for tok in ('*', '<', '>', 'spec.json')):
                    continue
                for candidate in expand_braces(ref):
                    target = ROOT / candidate.rstrip('/')
                    self.assertTrue(target.exists(), f'{path}: cited path does not exist: {candidate} (from {ref})')

        # Spot-check isolated-e2e CLI invocations match real scripts
        e2e_skill = (skills_dir / 'isolated-e2e/SKILL.md').read_text(encoding='utf-8')
        self.assertIn('python rust/release/protocol.py --package <pkg-dir> --work <work-dir>', e2e_skill)
        self.assertIn('python rust/release/timing_report.py <frames.csv> --host-record <host-record>', e2e_skill)


if __name__ == '__main__':
    unittest.main()
