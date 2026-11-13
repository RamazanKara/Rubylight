import contextlib, io, os, pathlib, sys, tempfile, unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import code_map  # noqa: E402

LOG = '''@@fix(stream): keep the newest frame
rust/host/src/stream.rs
rust/core/src/session.rs
client/protocol/desktop/src/strings/de.rs
client/protocol/desktop/src/strings/fr.rs

@@feat(stream): a new setting
rust/host/src/stream.rs
rust/core/src/config.rs
docs/configuration.md

@@fix: installer text
rust/setup/src/install.rs

@@fixture(stream): not a fix
rust/host/src/stream.rs
'''


class Summary(unittest.TestCase):
    def test_rust_header_gives_its_first_sentence(self):
        text = '//! The control worker: ENet, input\n//! and feedback. More follows.\n//!\n//! Second paragraph.\nuse std;\n'
        self.assertEqual(code_map.summary(text, '.rs'), 'The control worker: ENet, input and feedback.')

    def test_rust_header_after_attributes_and_none_at_all(self):
        self.assertEqual(code_map.summary('#![allow(dead_code)]\n//! Probes.\n', '.rs'), 'Probes.')
        self.assertEqual(code_map.summary('use std;\n// a note\n//! not a header\n', '.rs'), '')
        self.assertEqual(code_map.summary('', '.rs'), '')

    def test_java_takes_the_class_comment_not_a_member_comment(self):
        text = 'package a;\n/**\n * Sends input to the host.\n *\n * Details.\n */\npublic final class Sender {\n    /** A field. */\n    int x;\n}\n'
        self.assertEqual(code_map.summary(text, '.java'), 'Sends input to the host.')
        self.assertEqual(code_map.summary('package a;\npublic class Bare {\n    /** A field. */\n    int x;\n}\n', '.java'), '')

    def test_python_docstring_and_script_comments(self):
        self.assertEqual(code_map.summary('#!/usr/bin/env python\n"""Run the checks.\n\nusage: x\n"""\n', '.py'), 'Run the checks.')
        self.assertEqual(code_map.summary('# Developer loop helpers. `check` runs them.\nparam()\n', '.ps1'), 'Developer loop helpers.')
        self.assertEqual(code_map.summary('/**\n * Typed client for the API.\n */\nexport {}\n', '.ts'), 'Typed client for the API.')
        self.assertEqual(code_map.summary('// Formatting shared by the pages.\nexport {}\n', '.ts'), 'Formatting shared by the pages.')

    def test_long_sentence_is_cut(self):
        self.assertLessEqual(len(code_map.summary('//! ' + 'word ' * 80 + '\n', '.rs')), 160)


class History(unittest.TestCase):
    def setUp(self):
        self.history = code_map.commits(LOG)

    def test_translations_count_as_one_file(self):
        self.assertEqual(len(self.history), 4)
        self.assertIn('client/protocol/desktop/src/strings/*.rs', self.history[0][1])
        self.assertEqual(code_map.grouped('client/app/src/main/res/values-de/strings.xml'),
                         code_map.grouped('client/app/src/main/res/values/strings.xml'))
        self.assertEqual(code_map.grouped('rust/release/notes.d/3.11.10/fix.md'), 'rust/release/notes.d/*')

    def test_together_ranks_the_files_of_shared_commits(self):
        count, rows = code_map.together(self.history, 'rust/host/src/stream.rs')
        self.assertEqual(count, 3)
        self.assertEqual({path for _, _, path in rows}, {'rust/core/src/session.rs', 'rust/core/src/config.rs',
                                                         'docs/configuration.md', 'client/protocol/desktop/src/strings/*.rs'})
        self.assertTrue(all(n == 1 and abs(share - 1 / 3) < 1e-9 for n, share, _ in rows))

    def test_together_ignores_sweeps(self):
        count, rows = code_map.together(self.history, 'rust/host/src/stream.rs', limit=2)
        self.assertEqual((count, rows), (1, []))

    def test_fixes_counts_only_fix_commits(self):
        rows = {path: (fixed, total) for fixed, total, path in code_map.fixes(self.history)}
        self.assertEqual(rows['rust/host/src/stream.rs'], (1, 3))
        self.assertEqual(rows['rust/setup/src/install.rs'], (1, 1))
        self.assertNotIn('rust/core/src/config.rs', rows)


class Stale(unittest.TestCase):
    def test_names_a_missing_file_and_a_missing_name(self):
        with tempfile.TemporaryDirectory() as temp:
            folder = pathlib.Path(temp)
            (folder / 'docs').mkdir()
            source = ('fn launch() {}', 'const WAIT: u32 = 1;', 'fn relaunched() {}')
            (folder / 'host.rs').write_text(chr(10).join(source), encoding='utf-8')
            text = chr(10).join((
                'See [`launch`, `WAIT` and the rest](../host.rs), [the site](https://example.org/x.rs) and [here](#top).',
                'Gone: [`start`](../host.rs#L3), [`relaunch()`](../host.rs), [`host::launch`](../host.rs), [x](../none.rs).',
                'A folder is enough: [docs](../docs), and prose in a link is not a name: [the launch](../host.rs).'))
            self.assertEqual(code_map.stale(text, folder / 'docs'), [
                (2, '../host.rs', 'no `start`'), (2, '../host.rs', 'no `relaunch`'), (2, '../none.rs', 'no such file')])

    def test_paths_in_backticks_and_in_table_columns(self):
        with tempfile.TemporaryDirectory() as temp:
            folder = pathlib.Path(temp)
            for name in ('docs', 'rust/core/src/config', 'rust/host/src'):
                (folder / name).mkdir(parents=True)
            for name in ('rust/core/src/config.rs', 'rust/host/src/state.rs'):
                (folder / name).write_text('', encoding='utf-8')
            text = chr(10).join((
                'Read `rust/core/src/config.rs` and `rust/core/src/gone.rs`; `cargo/test` and `a/b` are not in the tree.',
                '',
                '| Area | `rust/core/src/` | `rust/host/src/` |',
                '| --- | --- | --- |',
                '| Settings | `config.rs`, `config/`, `Config` | `state.rs`, `web.rs` |',
                '| Words | `lost.rs` and `rust/host/src/state.rs` | none |',
                '',
                'After the table `config.rs` is only a word again.'))
            self.assertEqual(code_map.stale(text, folder / 'docs'), [
                (1, 'rust/core/src/gone.rs', 'no such file'), (5, 'rust/host/src/web.rs', 'no such file'),
                (6, 'rust/core/src/lost.rs', 'no such file')])


class Repository(unittest.TestCase):
    def test_the_maps_name_only_what_exists(self):
        for name in code_map.MAPS:
            with self.subTest(map=name):
                path = code_map.ROOT / name
                self.assertEqual(code_map.stale(path.read_text(encoding='utf-8'), path.parent), [])

    def test_index_reads_this_tree(self):
        rows = {path: (lines, tests, text) for path, lines, tests, text in code_map.index(['tools'])}
        lines, tests, text = rows['tools/test_code_map.py']
        self.assertGreater(tests, 5)
        self.assertEqual(rows['tools/code_map.py'][2], 'Find the code for a change, and what usually changes with it.')
        self.assertEqual(rows['tools/code_map.py'][0], len(pathlib.Path(code_map.__file__).read_text(encoding='utf-8').splitlines()))
        folders = {folder: files for folder, files, _, _ in code_map.folders(code_map.index(['tools']))}
        self.assertGreaterEqual(folders['tools'], 5)

    def test_paths_are_taken_from_the_root_or_from_the_current_folder(self):
        self.assertEqual(code_map.relative('tools/dev.py'), 'tools/dev.py')
        self.assertEqual(code_map.relative(code_map.ROOT / 'tools'), 'tools')
        before = os.getcwd()
        os.chdir(code_map.ROOT / 'tools')
        try:
            self.assertEqual(code_map.relative('hw/run.py'), 'tools/hw/run.py')
            self.assertEqual(code_map.relative('docs'), 'docs')
        finally:
            os.chdir(before)

    def test_foreign_and_generated_code_is_left_out(self):
        self.assertFalse(code_map.ours('rust/windows/include/nvEncodeAPI.h'))
        self.assertFalse(code_map.ours('client/protocol/vendor/x/src/lib.rs'))
        self.assertFalse(code_map.ours('docs/configuration.md'))
        self.assertTrue(code_map.ours('rust/host/src/stream.rs'))

    def test_command_line(self):
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            self.assertEqual(code_map.main(['index', 'tools', '--dirs']), 0)
        self.assertIn('tools', out.getvalue())
        for bad in (['index'], ['with']):
            with self.subTest(bad=bad), self.assertRaises(SystemExit), contextlib.redirect_stderr(io.StringIO()):
                code_map.main(bad)


if __name__ == '__main__':
    unittest.main()
