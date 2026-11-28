"""Tests for the TOML host settings catalog and tools/generated_docs.py codegen."""
import pathlib, shutil, sys, tempfile, tomllib, unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import generated_docs  # noqa: E402

ROOT = pathlib.Path(__file__).resolve().parents[1]
HOST_DOCS = [d for d in generated_docs.DOCS if d.render is not None]
NIGHTJAR_DOCS = [
    d for d in generated_docs.DOCS
    if generated_docs.NIGHTJAR_SETTINGS_TOML in d.inputs
]


class HostSettingsCodegen(unittest.TestCase):
    def test_catalog_parses_with_unique_keys_and_valid_references(self):
        cat = generated_docs.load_catalog(ROOT)
        settings = cat['setting']
        keys = [s['key'] for s in settings]
        self.assertEqual(len(keys), len(set(keys)), 'duplicate setting keys in settings.toml')
        self.assertGreaterEqual(len(keys), 290)
        by_key = cat['_by_key']
        for key in cat['rust']['override_order']:
            self.assertTrue(by_key[key].get('override_allowed'), key)
        for key in cat['rust']['restart_order']:
            self.assertTrue(by_key[key].get('restart'), key)
        for key in cat['rust']['settings_30_order']:
            self.assertTrue(by_key[key].get('settings_30'), key)
        for key in cat['console']['global_order']:
            self.assertIn('console', by_key[key], key)
        for sec in cat['docs']['sections']:
            for key in sec['keys']:
                self.assertIn(key, by_key, key)

    def test_generated_files_are_current_lf_and_headed(self):
        stale = generated_docs.run_all(check=True, docs=HOST_DOCS, root=ROOT)
        self.assertEqual(stale, [], f'stale generated files: {stale}')
        for doc in HOST_DOCS:
            raw = (ROOT / doc.path).read_bytes()
            self.assertNotIn(b'\r\n', raw, f'{doc.path} has CRLF line endings')
            text = raw.decode('utf-8')
            if doc.path.endswith('.rs'):
                self.assertTrue(text.startswith('//! '), f'{doc.path} missing //! module header')

    def test_check_detects_staleness_without_mutating_and_regen_fixes_it(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            # Copy settings.toml and all generated targets into temp worktree.
            toml_src = ROOT / generated_docs.SETTINGS_TOML
            toml_dst = root / generated_docs.SETTINGS_TOML
            toml_dst.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(toml_src, toml_dst)
            for doc in HOST_DOCS:
                dst = root / doc.path
                dst.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(ROOT / doc.path, dst)

            self.assertEqual(generated_docs.run_all(check=True, docs=HOST_DOCS, root=root), [])

            # Tamper with one generated Rust file and docs/configuration.md.
            tampered_rs = root / 'rust/core/config/src/config/retired_gen.rs'
            bad_rs = tampered_rs.read_text(encoding='utf-8') + '// drift\n'
            tampered_rs.write_text(bad_rs, encoding='utf-8', newline='\n')

            tampered_md = root / 'docs/configuration.md'
            bad_md = tampered_md.read_text(encoding='utf-8').replace('### This PC', '### Stale PC', 1)
            tampered_md.write_text(bad_md, encoding='utf-8', newline='\n')

            stale = generated_docs.run_all(check=True, docs=HOST_DOCS, root=root)
            self.assertEqual(sorted(stale), ['docs/configuration.md', 'rust/core/config/src/config/retired_gen.rs'])
            # --check must leave files untouched.
            self.assertEqual(tampered_rs.read_text(encoding='utf-8'), bad_rs)
            self.assertEqual(tampered_md.read_text(encoding='utf-8'), bad_md)

            # Regenerating restores exact parity and is idempotent.
            fixed = generated_docs.run_all(check=False, docs=HOST_DOCS, root=root)
            self.assertEqual(sorted(fixed), ['docs/configuration.md', 'rust/core/config/src/config/retired_gen.rs'])
            self.assertEqual(generated_docs.run_all(check=True, docs=HOST_DOCS, root=root), [])
            self.assertEqual(tampered_rs.read_bytes(), (ROOT / 'rust/core/config/src/config/retired_gen.rs').read_bytes())
            self.assertEqual(tampered_md.read_bytes(), (ROOT / 'docs/configuration.md').read_bytes())

    def test_catalog_parses_with_stdlib_tomllib_directly(self):
        raw = (ROOT / generated_docs.SETTINGS_TOML).read_text(encoding='utf-8')
        parsed = tomllib.loads(raw)
        self.assertIn('setting', parsed)
        self.assertIn('retired', parsed)
        self.assertIn('console', parsed)
        self.assertIn('web', parsed)
        self.assertIn('docs', parsed)


class NightjarSettingsCodegen(unittest.TestCase):
    def test_nightjar_targets_are_registered_current_lf_and_headed(self):
        expected = {
            'client/docs/settings.md',
            'client/protocol/policy/src/settings_catalog_gen.rs',
            'client/protocol/desktop-config/src/config/defaults_gen.rs',
            'client/app/src/main/java/com/limelight/preferences/SettingsCatalogGen.java',
        }
        self.assertEqual({d.path for d in NIGHTJAR_DOCS}, expected)
        stale = generated_docs.run_all(check=True, docs=NIGHTJAR_DOCS, root=ROOT)
        self.assertEqual(stale, [], f'stale Nightjar generated targets: {stale}')
        for doc in NIGHTJAR_DOCS:
            raw = (ROOT / doc.path).read_bytes()
            self.assertNotIn(b'\r\n', raw, f'{doc.path} has CRLF line endings')
            text = raw.decode('utf-8')
            if doc.path.endswith('.rs'):
                self.assertTrue(text.startswith('//! '), f'{doc.path} missing //! module header')
            if doc.path.endswith('.java'):
                self.assertIn('/**\n', text.split('public class', 1)[0], f'{doc.path} missing Javadoc header')


if __name__ == '__main__':
    unittest.main()
