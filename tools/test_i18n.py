"""Tests for tools/i18n.py across Host JSON, Desktop Rust tables, and Android XML surfaces."""

import contextlib
import io
import json
import pathlib
import shutil
import subprocess
import sys
import tempfile
import unittest
import xml.etree.ElementTree as ET

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import i18n  # noqa: E402

ROOT = pathlib.Path(__file__).resolve().parents[1]


def run_cli(argv):
    stdout = io.StringIO()
    stderr = io.StringIO()
    with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
        rc = i18n.main(argv)
    return rc, stdout.getvalue(), stderr.getvalue()


def write_lf(path, text):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding='utf-8', newline='\n')


def seed_temp_repo(base):
    """Create a minimal valid 3-surface repo (22 host, 14 desktop, 14+14+1 android)."""
    # 1. 22 Host JSONs
    for loc in i18n.HOST_LOCALES:
        val_save = 'Save' if loc.startswith('en') else f'Save_{loc}'
        val_greet = 'Hello {name}' if loc.startswith('en') else f'Hello_{loc} {{name}}'
        doc = {
            '_common': {'save': val_save, 'greet': val_greet},
            'config': {'old_key': 'Old' if loc.startswith('en') else f'Old_{loc}'},
        }
        write_lf(base / i18n.HOST_REL / f'{loc}.json', json.dumps(doc, indent=2, ensure_ascii=False) + '\n')

    # 2. 14 Desktop Rust string tables
    for lang in i18n.DESKTOP_LOCALES:
        table = (
            f'// {lang} strings\n\n'
            'pub const TABLE: &[(&str, &str)] = &[\n'
            '    // Section comment preserved\n'
            f'    ("Cancel", "Cancel_{lang}"),\n'
            f'    ("Remove {{}}?", "Remove_{lang} {{}}?"),\n'
            f'    ("Only on desktop {{}}", "Desktop_{lang} {{}}"),\n'
            '];\n'
        )
        write_lf(base / i18n.DESKTOP_REL / f'{lang}.rs', table)
        # The 7.0 table starts empty, as it did in the repository.
        write_lf(base / i18n.DESKTOP_70_REL / f'{lang}.rs', f'//! {lang} 7.0 strings\npub const TABLE: &[(&str, &str)] = &[\n];\n')

    # 3. Android: values, 14 values-<lang>, values-television, 14 values-<lang>-television, values-w600dp
    def xml_doc(body):
        return f'<?xml version="1.0" encoding="utf-8"?>\n<!-- header comment -->\n<resources>\n{body}</resources>\n'

    base_en = (
        '    <string name="common_cancel">Cancel</string>\n'
        '    <string name="android_only_item">PC\\\'s item %1$s</string>\n'
        '    <string name="untranslatable_brand" translatable="false">Nightjar</string>\n'
        '    <plurals name="items_count">\n'
        '        <item quantity="one">%1$d item</item>\n'
        '        <item quantity="other">%1$d items</item>\n'
        '    </plurals>\n'
    )
    write_lf(base / i18n.ANDROID_REL / 'values/strings.xml', xml_doc(base_en))
    for loc in i18n.ANDROID_LOCALES:
        loc_body = (
            f'    <string name="common_cancel">Cancel_{loc}</string>\n'
            f'    <string name="android_only_item">PC_{loc}\\\'s item %1$s</string>\n'
            '    <plurals name="items_count">\n'
            f'        <item quantity="other">%1$d items_{loc}</item>\n'
            '    </plurals>\n'
        )
        write_lf(base / i18n.ANDROID_REL / f'values-{loc}/strings.xml', xml_doc(loc_body))

    write_lf(
        base / i18n.ANDROID_REL / 'values-television/strings_help.xml',
        xml_doc('    <string name="tv_tip">Select a PC</string>\n'),
    )
    for loc in i18n.ANDROID_LOCALES:
        write_lf(
            base / i18n.ANDROID_REL / f'values-{loc}-television/strings_help.xml',
            xml_doc(f'    <string name="tv_tip">Select_{loc}</string>\n'),
        )

    write_lf(
        base / i18n.ANDROID_REL / 'values-w600dp/dimens.xml',
        xml_doc('    <dimen name="card_pad">16dp</dimen>\n'),
    )


class LiveRepoCheckTests(unittest.TestCase):
    def test_check_passes_cleanly_on_current_repo(self):
        rc, out, err = run_cli(['check', '--json'])
        self.assertEqual(rc, 0, out + err)
        data = json.loads(out)
        self.assertTrue(data['ok'])
        self.assertEqual(data['surfaces']['host']['locales'], 22)
        self.assertEqual(data['surfaces']['desktop']['locales'], 14)
        self.assertEqual(data['surfaces']['android']['folders'], 42)
        self.assertEqual(data['surfaces']['android']['base_locales'], 14)
        self.assertEqual(data['surfaces']['android']['tv_locales'], 14)

    def test_strict_host_surfaces_legacy_missing_keys(self):
        rc, out, _ = run_cli(['check', '--surface', 'host', '--strict-host', '--json'])
        self.assertEqual(rc, 1)
        data = json.loads(out)
        self.assertGreater(len(data['surfaces']['host']['issues']), 0)


class HostSurfaceTests(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.root = pathlib.Path(self._tmp.name)
        seed_temp_repo(self.root)

    def test_host_apply_with_glossary_cross_fill_rename_and_prune(self):
        # "Close" is a 3-way verified term in i18n_glossary.json with all host locales!
        spec = self.root / 'spec.json'
        write_lf(spec, json.dumps({
            'host': [
                {'key': 'config.close_btn', 'en': 'Close', 'translations': {}},
            ]
        }))
        rc, out, err = run_cli(['--root', str(self.root), 'apply', str(spec)])
        self.assertEqual(rc, 0, out + err)
        self.assertIn('host=1', out)
        # Verify strict-host check passes (100% complete across all 22 locales)
        rc, out, err = run_cli(['--root', str(self.root), 'check', '--surface', 'host', '--strict-host'])
        self.assertEqual(rc, 0, out + err)
        de_doc = json.loads((self.root / i18n.HOST_REL / 'de.json').read_text(encoding='utf-8'))
        self.assertEqual(de_doc['config']['close_btn'], 'Schließen')

        # Rename across all 22 locales into a new nested section
        rc, out, err = run_cli(['--root', str(self.root), 'rename', 'host', 'config.old_key', 'advanced.renamed_key'])
        self.assertEqual(rc, 0, out + err)
        rc, out, err = run_cli(['--root', str(self.root), 'check', '--surface', 'host', '--strict-host'])
        self.assertEqual(rc, 0, out + err)
        fr_doc = json.loads((self.root / i18n.HOST_REL / 'fr.json').read_text(encoding='utf-8'))
        self.assertNotIn('old_key', fr_doc['config'])
        self.assertEqual(fr_doc['advanced']['renamed_key'], 'Old_fr')

        # Inject a stale key into de.json and verify check catches it, then prune --stale fixes it
        de_doc = json.loads((self.root / i18n.HOST_REL / 'de.json').read_text(encoding='utf-8'))
        de_doc['orphan'] = {'stale': 'Alt'}
        write_lf(self.root / i18n.HOST_REL / 'de.json', json.dumps(de_doc, indent=2) + '\n')
        rc, out, _ = run_cli(['--root', str(self.root), 'check', '--surface', 'host', '--strict-host'])
        self.assertEqual(rc, 1)
        self.assertIn('stale key', out)

        rc, out, err = run_cli(['--root', str(self.root), 'prune', 'host', '--stale'])
        self.assertEqual(rc, 0, out + err)
        rc, out, err = run_cli(['--root', str(self.root), 'check', '--surface', 'host', '--strict-host'])
        self.assertEqual(rc, 0, out + err)
        de_doc = json.loads((self.root / i18n.HOST_REL / 'de.json').read_text(encoding='utf-8'))
        self.assertNotIn('orphan', de_doc)

    def test_host_apply_rejects_incomplete_or_bad_placeholders_atomically(self):
        before = (self.root / i18n.HOST_REL / 'en.json').read_text(encoding='utf-8')
        spec = self.root / 'bad.json'
        write_lf(spec, json.dumps({
            'host': [
                {'key': 'config.untranslated_new', 'en': 'Brand new {item}', 'translations': {'de': 'Neu'}},
            ]
        }))
        rc, _, err = run_cli(['--root', str(self.root), 'apply', str(spec)])
        self.assertEqual(rc, 1)
        self.assertIn('missing translations', err)
        # File must remain untouched on failure
        self.assertEqual((self.root / i18n.HOST_REL / 'en.json').read_text(encoding='utf-8'), before)

        # Complete locales but broken placeholder in de
        all_tr = {loc: f'Val_{loc} {{item}}' for loc in i18n.HOST_LOCALES if loc != 'en'}
        all_tr['de'] = 'Falscher Platzhalter'
        write_lf(spec, json.dumps({'host': [{'key': 'config.ph_bad', 'en': 'New {item}', 'translations': all_tr}]}))
        rc, _, err = run_cli(['--root', str(self.root), 'apply', str(spec)])
        self.assertEqual(rc, 1)
        self.assertIn('placeholder mismatch', err)
        self.assertEqual((self.root / i18n.HOST_REL / 'en.json').read_text(encoding='utf-8'), before)


class DesktopSurfaceTests(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.root = pathlib.Path(self._tmp.name)
        seed_temp_repo(self.root)

    def test_desktop_apply_preserves_comments_and_merge_union_and_rename_prune(self):
        # Apply new key with glossary cross-fill ("Settings" is in glossary across all 14 langs)
        spec = self.root / 'desk.json'
        write_lf(spec, json.dumps({'desktop': [{'key': 'Settings', 'translations': {}}]}))
        rc, out, err = run_cli(['--root', str(self.root), 'apply', str(spec)])
        self.assertEqual(rc, 0, out + err)
        self.assertIn('desktop=1', out)
        frozen = (self.root / i18n.DESKTOP_REL / 'de.rs').read_text(encoding='utf-8')
        self.assertNotIn('"Settings"', frozen, 'the frozen table never grows')
        de_txt = (self.root / i18n.DESKTOP_70_REL / 'de.rs').read_text(encoding='utf-8')
        self.assertEqual(de_txt, '//! de 7.0 strings\npub const TABLE: &[(&str, &str)] = &[\n    ("Settings", "Einstellungen"),\n];\n')

        rc, out, err = run_cli(['--root', str(self.root), 'check', '--surface', 'desktop'])
        self.assertEqual(rc, 0, out + err)

        # Rename preserves translation and table structure
        rc, out, err = run_cli(['--root', str(self.root), 'rename', 'desktop', 'Settings', 'Preferences'])
        self.assertEqual(rc, 0, out + err)
        de_txt = (self.root / i18n.DESKTOP_70_REL / 'de.rs').read_text(encoding='utf-8')
        self.assertIn('("Preferences", "Einstellungen"),', de_txt)
        self.assertNotIn('("Settings",', de_txt)

        # Prune removes entry cleanly
        rc, out, err = run_cli(['--root', str(self.root), 'prune', 'desktop', 'Preferences'])
        self.assertEqual(rc, 0, out + err)
        rc, out, err = run_cli(['--root', str(self.root), 'check', '--surface', 'desktop'])
        self.assertEqual(rc, 0, out + err)
        self.assertNotIn('Preferences', (self.root / i18n.DESKTOP_70_REL / 'de.rs').read_text(encoding='utf-8'))

    def test_desktop_existing_key_is_updated_in_its_table_and_keys_live_in_one_table(self):
        # Updating a key of the frozen table edits it in place and adds nothing to strings_70/.
        new_tr = {lang: f'Abbrechen_{lang}' for lang in i18n.DESKTOP_LOCALES}
        spec = self.root / 'upd.json'
        write_lf(spec, json.dumps({'desktop': [{'key': 'Cancel', 'translations': new_tr}]}))
        rc, out, err = run_cli(['--root', str(self.root), 'apply', str(spec)])
        self.assertEqual(rc, 0, out + err)
        self.assertIn('("Cancel", "Abbrechen_de"),', (self.root / i18n.DESKTOP_REL / 'de.rs').read_text(encoding='utf-8'))
        self.assertNotIn('Cancel', (self.root / i18n.DESKTOP_70_REL / 'de.rs').read_text(encoding='utf-8'))

        # Renaming a frozen key keeps it in the frozen table; a target in strings_70/ is refused.
        write_lf(spec, json.dumps({'desktop': [{'key': 'Settings', 'translations': {}}]}))
        rc, out, err = run_cli(['--root', str(self.root), 'apply', str(spec)])
        self.assertEqual(rc, 0, out + err)
        rc, _, err = run_cli(['--root', str(self.root), 'rename', 'desktop', 'Cancel', 'Settings'])
        self.assertEqual(rc, 1)
        self.assertIn('already exists', err)
        rc, out, err = run_cli(['--root', str(self.root), 'rename', 'desktop', 'Cancel', 'Close'])
        self.assertEqual(rc, 0, out + err)
        self.assertIn('("Close", "Abbrechen_de"),', (self.root / i18n.DESKTOP_REL / 'de.rs').read_text(encoding='utf-8'))

        # Prune reaches both tables.
        rc, out, err = run_cli(['--root', str(self.root), 'prune', 'desktop', 'Close', 'Settings'])
        self.assertEqual(rc, 0, out + err)
        self.assertIn('28 entry(ies) across 28 file(s)', out)

        # Check covers the union: a key in both tables, or missing from a language's union, fails.
        dup = self.root / i18n.DESKTOP_70_REL / 'fr.rs'
        write_lf(dup, '//! fr\npub const TABLE: &[(&str, &str)] = &[\n    ("Remove {}?", "Retirer {} ?"),\n];\n')
        rc, out, err = run_cli(['--root', str(self.root), 'check', '--surface', 'desktop'])
        self.assertEqual(rc, 1)
        self.assertIn("desktop/strings_70/fr.rs: duplicate key 'Remove {}?'", out + err)
        write_lf(dup, '//! fr\npub const TABLE: &[(&str, &str)] = &[\n    ("Only in French", "Seulement en français"),\n];\n')
        rc, out, err = run_cli(['--root', str(self.root), 'check', '--surface', 'desktop'])
        self.assertEqual(rc, 1)
        self.assertIn("desktop/de.rs: missing key 'Only in French'", out + err)
        dup.unlink()
        rc, out, err = run_cli(['--root', str(self.root), 'check', '--surface', 'desktop'])
        self.assertEqual(rc, 1)
        self.assertIn('desktop/strings_70/fr.rs: missing table file', out + err)

    def test_desktop_apply_preserves_git_merge_union_without_conflicts(self):
        def git(*args):
            res = subprocess.run(['git', '-C', str(self.root), *args], capture_output=True, text=True, encoding='utf-8')
            self.assertEqual(res.returncode, 0, res.stdout + res.stderr)

        shutil.copyfile(ROOT / '.gitattributes', self.root / '.gitattributes')
        git('init', '-q', '-b', 'main')
        git('config', 'user.email', 'test@example.invalid')
        git('config', 'user.name', 'test')
        git('config', 'core.autocrlf', 'false')
        git('add', '-A')
        git('commit', '-q', '-m', 'base')

        # Branch A adds "Settings"; Branch B adds "Profiles" (both via i18n.py apply)
        git('checkout', '-q', '-b', 'branch-a')
        spec_a = self.root / 'a.json'
        write_lf(spec_a, json.dumps({'desktop': [{'key': 'Settings', 'translations': {}}]}))
        rc, _, err = run_cli(['--root', str(self.root), 'apply', str(spec_a)])
        self.assertEqual(rc, 0, err)
        spec_a.unlink()
        git('add', '-A')
        git('commit', '-q', '-m', 'add Settings')

        git('checkout', '-q', '-b', 'branch-b', 'main')
        spec_b = self.root / 'b.json'
        write_lf(spec_b, json.dumps({'desktop': [{'key': 'Profiles', 'translations': {}}]}))
        rc, _, err = run_cli(['--root', str(self.root), 'apply', str(spec_b)])
        self.assertEqual(rc, 0, err)
        spec_b.unlink()
        git('add', '-A')
        git('commit', '-q', '-m', 'add Profiles')

        git('checkout', '-q', 'branch-a')
        git('merge', '-q', '--no-edit', 'branch-b')
        rc, out, err = run_cli(['--root', str(self.root), 'check', '--surface', 'desktop'])
        self.assertEqual(rc, 0, out + err)
        de_txt = (self.root / i18n.DESKTOP_70_REL / 'de.rs').read_text(encoding='utf-8')
        self.assertIn('"Settings"', de_txt)
        self.assertIn('"Profiles"', de_txt)

    def test_desktop_check_catches_placeholders_and_untranslated_and_experimental(self):
        # Multi-placeholder without explicit {0}/{1}
        bad_tr = {lang: 'Startet auf {} in {}' for lang in i18n.DESKTOP_LOCALES}
        spec = self.root / 'bad_ph.json'
        write_lf(spec, json.dumps({'desktop': [{'key': 'Start {} in {}', 'translations': bad_tr}]}))
        rc, _, err = run_cli(['--root', str(self.root), 'apply', str(spec)])
        self.assertEqual(rc, 1)
        self.assertIn('indexed', err)

        # Experimental wording blocked
        exp_tr = {lang: f'Exp_{lang}' for lang in i18n.DESKTOP_LOCALES}
        exp_tr['de'] = 'Experimentelle Funktion'
        write_lf(spec, json.dumps({'desktop': [{'key': 'New feature', 'translations': exp_tr}]}))
        rc, _, err = run_cli(['--root', str(self.root), 'apply', str(spec)])
        self.assertEqual(rc, 1)
        self.assertIn('experimental', err)


class AndroidAndCrossFillTests(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.root = pathlib.Path(self._tmp.name)
        seed_temp_repo(self.root)

    def test_android_apply_escaping_tv_rename_and_prune(self):
        # Cross-fill from glossary ("Add PC") AND test escaping of apostrophes & ampersands
        tr_all = {loc: f"Host's & PC_{loc}" for loc in i18n.ANDROID_LOCALES}
        spec = self.root / 'and.json'
        write_lf(spec, json.dumps({
            'android': [
                {'key': 'add_pc_btn', 'en': 'Add PC', 'translations': {}},
                {'key': 'host_amp', 'en': "Host's & PC", 'file': 'strings_extra.xml', 'translations': tr_all},
            ],
            'android_tv': [
                {'key': 'tv_add_pc', 'en': 'Add PC', 'file': 'strings_help.xml', 'translations': {}},
            ],
        }))
        rc, out, err = run_cli(['--root', str(self.root), 'apply', str(spec)])
        self.assertEqual(rc, 0, out + err)
        self.assertIn('android=2', out)
        self.assertIn('android_tv=1', out)

        rc, out, err = run_cli(['--root', str(self.root), 'check', '--surface', 'android'])
        self.assertEqual(rc, 0, out + err)

        # Verify comments preserved and escaping proper
        de_xml = (self.root / i18n.ANDROID_REL / 'values-de/strings.xml').read_text(encoding='utf-8')
        self.assertIn('<!-- header comment -->', de_xml)
        self.assertIn('<string name="add_pc_btn">PC hinzufügen</string>', de_xml)
        extra_de = (self.root / i18n.ANDROID_REL / 'values-de/strings_extra.xml').read_text(encoding='utf-8')
        self.assertIn("Host\\'s &amp; PC_de", extra_de)

        # Verify TV translation landed
        tv_de = (self.root / i18n.ANDROID_REL / 'values-de-television/strings_help.xml').read_text(encoding='utf-8')
        self.assertIn('<string name="tv_add_pc">PC hinzufügen</string>', tv_de)

        # Rename across base + prune
        rc, out, err = run_cli(['--root', str(self.root), 'rename', 'android', 'add_pc_btn', 'add_host_btn'])
        self.assertEqual(rc, 0, out + err)
        rc, out, err = run_cli(['--root', str(self.root), 'check', '--surface', 'android'])
        self.assertEqual(rc, 0, out + err)
        rc, out, err = run_cli(['--root', str(self.root), 'prune', 'android', 'add_host_btn'])
        self.assertEqual(rc, 0, out + err)
        rc, out, err = run_cli(['--root', str(self.root), 'check', '--surface', 'android'])
        self.assertEqual(rc, 0, out + err)

    def test_cross_fill_live_desktop_to_android_and_android_to_desktop_and_same_batch(self):
        # 1. Live Desktop ("Only on desktop {}") -> Android ("Only on desktop %1$s")
        # 2. Live Android ("PC's item %1$s") -> Desktop ("PC's item {}")
        # 3. Same-batch Desktop -> Android with explicit translations only in desktop section
        batch_tr = {lang: f'Batch_{lang}' for lang in i18n.DESKTOP_LOCALES}
        spec = self.root / 'cross.json'
        write_lf(spec, json.dumps({
            'android': [
                {'key': 'from_desk', 'en': 'Only on desktop %1$s', 'translations': {}},
                {'key': 'from_batch', 'en': 'Fresh shared label', 'translations': {}},
            ],
            'desktop': [
                {'key': "PC's item {}", 'translations': {}},
                {'key': 'Fresh shared label', 'translations': batch_tr},
            ],
        }))
        rc, out, err = run_cli(['--root', str(self.root), 'apply', str(spec)])
        self.assertEqual(rc, 0, out + err)
        rc, out, err = run_cli(['--root', str(self.root), 'check'])
        self.assertEqual(rc, 0, out + err)

        # Verify placeholder conversion & apostrophe unescaping/escaping
        and_de = (self.root / i18n.ANDROID_REL / 'values-de/strings.xml').read_text(encoding='utf-8')
        self.assertIn('<string name="from_desk">Desktop_de %1$s</string>', and_de)
        self.assertIn('<string name="from_batch">Batch_de</string>', and_de)
        desk_de = (self.root / i18n.DESKTOP_70_REL / 'de.rs').read_text(encoding='utf-8')
        self.assertIn('("PC\'s item {}", "PC_de\'s item {}"),', desk_de)

    def test_dry_run_and_no_cross_fill_and_update_glossary(self):
        tmp_glossary = self.root / 'glossary.json'
        shutil.copyfile(i18n.DEFAULT_GLOSSARY, tmp_glossary)
        spec = self.root / 'spec.json'
        write_lf(spec, json.dumps({'desktop': [{'key': 'Settings', 'translations': {}}]}))

        # --no-cross-fill refuses empty translations
        rc, _, err = run_cli(['--root', str(self.root), '--glossary', str(tmp_glossary), 'apply', str(spec), '--no-cross-fill'])
        self.assertEqual(rc, 1)
        self.assertIn('missing translations', err)

        # --dry-run succeeds without touching disk
        before = (self.root / i18n.DESKTOP_70_REL / 'de.rs').read_text(encoding='utf-8')
        rc, out, err = run_cli(['--root', str(self.root), '--glossary', str(tmp_glossary), 'apply', str(spec), '--dry-run'])
        self.assertEqual(rc, 0, out + err)
        self.assertIn('[dry-run]', out)
        self.assertEqual((self.root / i18n.DESKTOP_70_REL / 'de.rs').read_text(encoding='utf-8'), before)

        # --update-glossary persists brand new term
        new_tr = {lang: f'Custom_{lang}' for lang in i18n.DESKTOP_LOCALES}
        write_lf(spec, json.dumps({'desktop': [{'key': 'Brand new term', 'translations': new_tr}]}))
        rc, out, err = run_cli(['--root', str(self.root), '--glossary', str(tmp_glossary), 'apply', str(spec), '--update-glossary'])
        self.assertEqual(rc, 0, out + err)
        g = json.loads(tmp_glossary.read_text(encoding='utf-8'))
        self.assertEqual(g['entries']['Brand new term']['de'], 'Custom_de')

    def test_android_check_catches_tight_label_and_bare_apostrophe(self):
        # Tight label "help" overflow
        write_lf(
            self.root / i18n.ANDROID_REL / 'values/strings_tight.xml',
            '<?xml version="1.0" encoding="utf-8"?>\n<resources>\n    <string name="help">Help</string>\n</resources>\n',
        )
        for loc in i18n.ANDROID_LOCALES:
            val = 'この非常に長いラベルは明らかに広すぎます' if loc == 'ja' else 'Hilfe'
            write_lf(
                self.root / i18n.ANDROID_REL / f'values-{loc}/strings_tight.xml',
                f'<?xml version="1.0" encoding="utf-8"?>\n<resources>\n    <string name="help">{val}</string>\n</resources>\n',
            )
        rc, out, _ = run_cli(['--root', str(self.root), 'check', '--surface', 'android'])
        self.assertEqual(rc, 1)
        self.assertIn('too long for tight label', out)

    def test_raw_xml_inline_tags_toml_spec_and_typographic_apostrophe_cross_fill(self):
        # raw_xml=true preserves XML tag attributes while escaping apostrophes in text nodes
        tr_raw = {loc: f'<b class="hi">PC\'s tip ({loc}):</b> ready' for loc in i18n.ANDROID_LOCALES}
        spec_json = self.root / 'raw.json'
        write_lf(spec_json, json.dumps({
            'android': [{
                'key': 'rich_tip',
                'en': '<b class="hi">PC\'s tip:</b> ready',
                'raw_xml': True,
                'translations': tr_raw,
            }],
        }))
        rc, out, err = run_cli(['--root', str(self.root), 'apply', str(spec_json)])
        self.assertEqual(rc, 0, out + err)
        de_xml = (self.root / i18n.ANDROID_REL / 'values-de/strings.xml').read_text(encoding='utf-8')
        self.assertIn('<string name="rich_tip"><b class="hi">PC\\\'s tip (de):</b> ready</string>', de_xml)

        # TOML batch spec + typographic right single quote ’ matching ASCII ' in Android live strings
        spec_toml = self.root / 'batch.toml'
        write_lf(
            spec_toml,
            '[[desktop]]\nkey = "PC\u2019s item {}"\n[desktop.translations]\n',
        )
        rc, out, err = run_cli(['--root', str(self.root), 'apply', str(spec_toml)])
        self.assertEqual(rc, 0, out + err)
        self.assertIn('desktop=1', out)

        # Renaming an Android key present in both values/ and values-television/ renames both
        rc, out, err = run_cli(['--root', str(self.root), 'rename', 'android', 'tv_tip', 'tv_tip_v2'])
        self.assertEqual(rc, 0, out + err)
        rc, out, err = run_cli(['--root', str(self.root), 'check'])
        self.assertEqual(rc, 0, out + err)

    def test_android_xml_entities_cross_fill_glossary_and_tight_label_width(self):
        # #1a: Desktop "Save & exit" cross-fills Android "Save & exit" (XML &amp;) and --update-glossary stays unescaped
        tmp_glossary = self.root / 'glossary_amp.json'
        shutil.copyfile(i18n.DEFAULT_GLOSSARY, tmp_glossary)
        tr_amp = {lang: f'Save & exit ({lang})' for lang in i18n.DESKTOP_LOCALES}
        spec = self.root / 'amp.json'
        write_lf(spec, json.dumps({
            'desktop': [{'key': 'Save & exit', 'translations': tr_amp}],
            'android': [{'key': 'save_and_exit', 'en': 'Save & exit', 'translations': {}}],
        }))
        rc, out, err = run_cli([
            '--root', str(self.root), '--glossary', str(tmp_glossary),
            'apply', str(spec), '--update-glossary',
        ])
        self.assertEqual(rc, 0, out + err)
        de_xml = (self.root / i18n.ANDROID_REL / 'values-de/strings.xml').read_text(encoding='utf-8')
        self.assertIn('<string name="save_and_exit">Save &amp; exit (de)</string>', de_xml)
        g = json.loads(tmp_glossary.read_text(encoding='utf-8'))
        self.assertIn('Save & exit', g['entries'])
        self.assertNotIn('Save &amp; exit', g['entries'])
        self.assertEqual(g['entries']['Save & exit']['de'], 'Save & exit (de)')

        # #1b: Tight label width counts decoded '&' as 1 col (not '&amp;' as 5 cols)
        # "Help" (4 cols) allows max(8, 14) = 14 cols. "A & B & C & D" is 13 cols decoded,
        # but 25 chars as "A &amp; B &amp; C &amp; D" — must pass BOTH apply and check_android!
        tr_tight = {loc: 'A & B & C & D' for loc in i18n.ANDROID_LOCALES}
        write_lf(spec, json.dumps({
            'android': [{'key': 'help', 'en': 'Help', 'translations': tr_tight}],
        }))
        rc, out, err = run_cli(['--root', str(self.root), 'apply', str(spec)])
        self.assertEqual(rc, 0, out + err)
        rc, out, err = run_cli(['--root', str(self.root), 'check', '--surface', 'android'])
        self.assertEqual(rc, 0, out + err)

    def test_apply_handles_unknown_locale_and_host_scalar_conflict_cleanly(self):
        # #2a: Unknown locale code across surfaces exits 1 with clean error (no ValueError traceback)
        before_host = (self.root / i18n.HOST_REL / 'en.json').read_text(encoding='utf-8')
        spec = self.root / 'bad_loc.json'
        for payload in (
            {'host': [{'key': 'config.x', 'en': 'X', 'translations': {'zz-ZZ': 'Bad'}}]},
            {'desktop': [{'key': 'X', 'translations': {'zz-ZZ': 'Bad'}}]},
            {'android': [{'key': 'x_key', 'en': 'X', 'translations': {'zz-ZZ': 'Bad'}}]},
        ):
            write_lf(spec, json.dumps(payload))
            rc, _, err = run_cli(['--root', str(self.root), 'apply', str(spec)])
            self.assertEqual(rc, 1)
            self.assertIn('unknown locale code', err)

        # #2b: Host nested key conflict under existing scalar (_common.save.nested) exits 1 cleanly
        all_tr = {loc: f'Val_{loc}' for loc in i18n.HOST_LOCALES if loc != 'en'}
        write_lf(spec, json.dumps({
            'host': [{'key': '_common.save.nested', 'en': 'Nested', 'translations': all_tr}],
        }))
        rc, _, err = run_cli(['--root', str(self.root), 'apply', str(spec)])
        self.assertEqual(rc, 1)
        self.assertIn('key conflict', err)
        self.assertEqual((self.root / i18n.HOST_REL / 'en.json').read_text(encoding='utf-8'), before_host)


if __name__ == '__main__':
    unittest.main()
