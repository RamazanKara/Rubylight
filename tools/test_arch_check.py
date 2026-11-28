"""Unit tests for tools/arch_check.py and its tools/dev.py integration."""
import contextlib
import io
import json
import pathlib
import subprocess
import sys
import tempfile
import unittest

TOOLS_DIR = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(TOOLS_DIR))
import arch_check  # noqa: E402
import dev  # noqa: E402

ROOT = TOOLS_DIR.parent


def _write_files(base, mapping):
    for rel, content in mapping.items():
        path = base / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content, encoding='utf-8', newline='\n')


def _write_configs(base, baselines=None):
    _write_files(base, {
        'tools/arch_baselines.json': json.dumps(baselines or {
            'missing_headers': {'rust': [], 'java_kt': []},
            'raw_config_read': {},
            'rubylight_family_gating': {'allowed': {}, 'legacy': {}},
            'unannotated_cfg_unix': {},
        }, indent=2) + '\n',
    })


def _lines(n, prefix='// pad\n'):
    return prefix * n


class FileDiscoveryAndNoSizeLimitTests(unittest.TestCase):
    def test_large_files_have_no_size_limit_and_exclusions_hold(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            _write_configs(root)
            _write_files(root, {
                'rust/core/src/large_cohesive.rs': '//! Large cohesive module.\n' + _lines(1200, 'fn f() {}\n'),
                'rust/core/src/large_cpp.cpp': _lines(1200, 'int x = 0;\n'),
                # Excluded foreign and generated paths remain ignored:
                'vendor/dep/no_header.rs': 'fn f() {}\n',
                'client/app/src/main/jni/adrenotools/src/hook/kgsl.h': _lines(10, 'int a;\n'),
                'client/app/src/main/jni/vulkan/shaders_spv.h': _lines(10, '0,\n'),
            })
            self.assertEqual(arch_check.check_repo(root), [])

    def test_count_loc_and_git_discovery_respects_gitignore(self):
        self.assertEqual(arch_check.count_loc(''), 0)
        self.assertEqual(arch_check.count_loc('a\n'), 1)
        self.assertEqual(arch_check.count_loc('a'), 1)
        self.assertEqual(arch_check.count_loc('a\nb'), 2)

        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            subprocess.run(['git', 'init', '-q'], cwd=root, check=True)
            _write_configs(root)
            _write_files(root, {
                '.gitignore': 'ignored_dir/\n',
                'src/tracked.rs': '//! tracked\nfn a() {}\n',
                'ignored_dir/missing_hdr.rs': 'fn bad() {}\n',
            })
            subprocess.run(['git', 'add', '.gitignore', 'src/tracked.rs'], cwd=root, check=True)
            self.assertEqual(arch_check.discover_files(root), ['src/tracked.rs'])
            self.assertEqual(arch_check.check_repo(root), [])


class ModuleHeaderTests(unittest.TestCase):
    def test_rust_module_headers_spdx_inner_attrs_and_allowlist(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            _write_configs(root, baselines={
                'missing_headers': {'rust': ['src/legacy_no_doc.rs'], 'java_kt': []},
                'raw_config_read': {},
                'rubylight_family_gating': {'allowed': {}, 'legacy': {}},
                'unannotated_cfg_unix': {},
            })
            _write_files(root, {
                'src/with_spdx_and_multiline_attr.rs': (
                    '// SPDX-License-Identifier: GPL-3.0-only\n'
                    '\n'
                    '#![allow(\n'
                    '    dead_code,\n'
                    '    clippy::all\n'
                    ')]\n'
                    '//! Real module header.\n'
                    'pub fn ok() {}\n'
                ),
                'src/missing.rs': '/// Item doc is not a module header.\npub fn nope() {}\n',
                'src/empty_bang.rs': '//!\n//!\npub fn nope() {}\n',
                'src/legacy_no_doc.rs': 'pub fn grandfathered() {}\n',
                'src/trailing_comment_bracket.rs': (
                    '#![allow(unused)] // [unclosed bracket in comment\n'
                    '//! Real header after trailing comment bracket.\n'
                    'pub fn ok() {}\n'
                ),
                'src/attr_byte_char_and_raw.rs': (
                    '#![cfg_attr(all(), my_attr(b\'"\', \'"\', r#"[ignored"#))] // [unclosed\n'
                    '#![allow( // [comment inside multiline attr\n'
                    '    dead_code,\n'
                    ')]\n'
                    '//! Real header after byte-char/raw-string attr.\n'
                    'pub fn ok() {}\n'
                ),
            })
            violations = arch_check.check_repo(root)
            self.assertEqual(
                [(p, r) for p, _, r, _ in violations],
                [
                    ('src/empty_bang.rs', 'missing-module-header'),
                    ('src/missing.rs', 'missing-module-header'),
                ],
            )

    def test_java_and_kotlin_headers_long_imports_and_top_level_kt(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            _write_configs(root, baselines={
                'missing_headers': {'rust': [], 'java_kt': ['src/Legacy.java']},
                'raw_config_read': {},
                'rubylight_family_gating': {'allowed': {}, 'legacy': {}},
                'unannotated_cfg_unix': {},
            })
            long_imports = ''.join(f'import com.example.pkg.VeryLongClassNameNumber{i:03d};\n' for i in range(45))
            self.assertGreater(len(long_imports), 1500)
            _write_files(root, {
                'src/LongImports.java': (
                    'package com.example;\n\n'
                    + long_imports
                    + '/**\n * Class doc after >1500 chars of imports.\n */\n'
                    + 'public class LongImports {}\n'
                ),
                'src/BlockOnly.java': 'package com.example;\n/* Not Javadoc */\npublic class BlockOnly {}\n',
                'src/CommentHasClass.java': (
                    'package com.example;\n'
                    '/*\n'
                    'public class FakeInComment {}\n'
                    '*/\n'
                    '/** Real class doc. */\n'
                    'public class CommentHasClass {}\n'
                ),
                'src/Legacy.java': 'package com.example;\npublic class Legacy {}\n',
                'src/TopLevelOk.kt': 'package com.example\n\n/** Top-level Kotlin utilities. */\nfun helper() = 1\n',
                'src/ValueClassOk.kt': 'package com.example\n\n/** Value class doc. */\n@JvmInline\nvalue class Wrapper(val v: Int)\n',
                'src/MissingKt.kt': 'package com.example\n\nval x = 1\n',
                'src/UrlSingleLine.java': 'package com.example;\n/** See https://example.com */\npublic class UrlSingleLine {}\n',
                'src/UrlClosingLine.java': 'package com.example;\n/**\n * See https://example.com */\npublic class UrlClosingLine {}\n',
                'src/UrlClosingLineKt.kt': 'package com.example\n/**\n * See https://example.com */\nclass UrlClosingLineKt\n',
                'src/FakeInBlock.java': 'package com.example;\n/*\n * /** Fake javadoc inside block comment */\n */\npublic class FakeInBlock {}\n',
                'src/FakeInBlockKt.kt': 'package com.example\n/*\n * /** Fake javadoc inside nested block comment */\n */\nclass FakeInBlockKt\n',
                'src/EmptyJavadoc.java': 'package com.example;\n/**/\npublic class EmptyJavadoc {}\n',
                'src/RecordOk.java': 'package com.example;\n/** Record doc. */\npublic record RecordOk(int x) {}\n',
                'src/RecordMissing.java': 'package com.example;\npublic record RecordMissing(int x) {}\n',
                'src/NonSealedOk.java': 'package com.example;\n/** Non-sealed doc. */\npublic non-sealed class NonSealedOk extends Base {}\n',
                'src/NonSealedMissing.java': 'package com.example;\npublic non-sealed class NonSealedMissing extends Base {}\n',
                'src/ExpectOk.kt': 'package com.example\n/** Expect class doc. */\nexpect class Platform()\n',
                'src/ActualOk.kt': 'package com.example\n/** Actual class doc. */\nactual class Platform()\n',
                'src/ExpectMissing.kt': 'package com.example\nexpect class Platform()\n',
                'src/UndocFunBeforeDocClass.kt': 'package com.example\nfun unannotated() = 1\n/** Class doc. */\nclass Foo\n',
                'src/UndocValBeforeDocClass.kt': 'package com.example\nval x = 1\n/** Class doc. */\nclass Bar\n',
                'src/UndocTypealiasBeforeDocClass.kt': 'package com.example\ntypealias MyInt = Int\n/** Class doc. */\nclass Baz\n',
                'src/DocFunBeforeClass.kt': 'package com.example\n/** Top-level doc. */\nfun documented() = 1\nclass Qux\n',
            })
            violations = arch_check.check_repo(root)
            self.assertEqual(
                [(p, r) for p, _, r, _ in violations],
                [
                    ('src/BlockOnly.java', 'missing-module-header'),
                    ('src/EmptyJavadoc.java', 'missing-module-header'),
                    ('src/ExpectMissing.kt', 'missing-module-header'),
                    ('src/FakeInBlock.java', 'missing-module-header'),
                    ('src/FakeInBlockKt.kt', 'missing-module-header'),
                    ('src/MissingKt.kt', 'missing-module-header'),
                    ('src/NonSealedMissing.java', 'missing-module-header'),
                    ('src/RecordMissing.java', 'missing-module-header'),
                    ('src/UndocFunBeforeDocClass.kt', 'missing-module-header'),
                    ('src/UndocTypealiasBeforeDocClass.kt', 'missing-module-header'),
                    ('src/UndocValBeforeDocClass.kt', 'missing-module-header'),
                ],
            )


class RuleLintTests(unittest.TestCase):
    def test_raw_config_read_rule(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            _write_configs(root, baselines={
                'missing_headers': {'rust': [], 'java_kt': []},
                'raw_config_read': {'rust/host/stream/src/stream/baselined.rs': 1},
                'rubylight_family_gating': {'allowed': {}, 'legacy': {}},
                'unannotated_cfg_unix': {},
            })
            _write_files(root, {
                'rust/host/stream/src/stream/config.rs': '//! Allowed config module.\nfn f(c: &Config) { let _ = c.boolean("a", false); }\n',
                'rust/host/stream/src/stream/tests.rs': '//! Test module.\nfn t(c: &Config) { let _ = c.boolean("a", false); }\n',
                'rust/host/stream/src/stream/foo_tests.rs': '//! Test module.\nfn t(c: &Config) { let _ = c.integer("a", 1); }\n',
                'rust/host/stream/src/stream/reconnect_tests/helper.rs': '//! Test helper.\nfn t(c: &Config) { let _ = c.get("a", "b"); }\n',
                'rust/host/stream/src/stream/baselined.rs': (
                    '//! Baselined.\n'
                    'fn ok(c: &Config) {\n'
                    '    let _ = c.boolean("legacy_one", false);\n'
                    '}\n'
                ),
                'rust/host/stream/src/stream/clean.rs': (
                    '//! Clean production file.\n'
                    'fn clean(c: &Config, peers: &Mutex<Peers>, ciphers: &Ciphers, m: &HashMap<String, i32>, usb: &Usb) {\n'
                    '    // c.boolean("in_comment", false)\n'
                    '    /* nested /* c.get("in_block", "x") */ */\n'
                    '    let _s = r#"c.integer("in_raw_str", 1)"#;\n'
                    '    let _ = peers.lock().unwrap().get(&id, is_audio);\n'
                    '    let _ = ciphers.get(&key, flags & 1 != 0);\n'
                    '    let _ = m.get("one_arg");\n'
                    '    let _ = PROBES.get(a, b, c, d, e);\n'
                    '    let _ = usb.list();\n'
                    '    let _ = m.values().count();\n'
                    '    c.values.insert("k".into(), "v".into());\n'
                    '}\n'
                    '#[cfg(test)]\n'
                    'mod tests {\n'
                    '    fn t(c: &Config) { let _ = c.boolean("test_mod", false); }\n'
                    '}\n'
                ),
                'rust/host/stream/src/stream/bad.rs': (
                    '//! Violating file with inline cfg(test).\n'
                    'fn f(c: &Config) {\n'
                    '    #[cfg(test)]\n'
                    '    if let Some(x) = fix {\n'
                    '        let _ = c.boolean("only_in_test_if", false);\n'
                    '    }\n'
                    '    let _a = c\n'
                    '        .boolean("prod_after_cfg_test", false);\n'
                    '    let _b = c.integer("i", 1);\n'
                    '    let _c = c.path("p", dir, "d");\n'
                    '    let _d = c.list("apps");\n'
                    '    let _e = c\n'
                    '        .get("k",\n'
                    '             "default");\n'
                    '    let _f = c.values.get("x");\n'
                    '    let _g = &c.values["y"];\n'
                    '}\n'
                ),
                'rust/host/stream/src/stream/b_quote_and_cfg_edges.rs': (
                    '//! Regression tests for b\'"\' byte-char, arg2 if/match, and cfg(all(..., test)).\n'
                    'fn f<' + "'a" + '>(b: u8, s: &' + "'a" + ' str, c: &Config, flag: i32, map: &Map) {\n'
                    '    if b == b\'"\' || b == b\'\\\\\' || b == b\'\\\'\' || b == b\'\\x22\' {\n'
                    '        let _ = c.boolean("after_b_quote", false);\n'
                    '    }\n'
                    '    let _ = c.get("key", if flag == 1 { "a" } else { "b" });\n'
                    '    let _ = c.get("key2", if flag == 1 { s } else { "b" });\n'
                    '    let _ = c.get("key3", match flag == 1 { true => "a", false => "b" });\n'
                    '    let _ = c.get("key4", { let _ok = flag == 1; "b" });\n'
                    '    let _ = map.get(&s, flag & 1 != 0);\n'
                    '}\n'
                    '#[cfg(all(unix, test))]\n'
                    'mod unix_tests {\n'
                    '    fn t(c: &Config) { let _ = c.boolean("skip_all_unix_test", false); }\n'
                    '}\n'
                    '#[cfg(all(not(windows), test))]\n'
                    'mod nested_all_tests {\n'
                    '    fn t(c: &Config) { let _ = c.boolean("skip_nested_all_test", false); }\n'
                    '}\n'
                    '#[cfg(any(unix, test))]\n'
                    'fn any_compiles_in_prod(c: &Config) {\n'
                    '    let _ = c.boolean("caught_any_test", false);\n'
                    '}\n'
                    '#[cfg(all(any(unix, test), feature = "x"))]\n'
                    'fn nested_any_compiles_in_prod(c: &Config) {\n'
                    '    let _ = c.boolean("caught_nested_any_test", false);\n'
                    '}\n'
                ),
            })
            violations = arch_check.check_repo(root)
            bad_lines = [line for p, line, rule, _ in violations if p == 'rust/host/stream/src/stream/bad.rs' and rule == 'raw-config-read']
            edge_lines = [
                line for p, line, rule, _ in violations
                if p == 'rust/host/stream/src/stream/b_quote_and_cfg_edges.rs' and rule == 'raw-config-read'
            ]
            self.assertEqual(len(violations), 14, violations)
            self.assertEqual(bad_lines, [8, 9, 10, 11, 13, 15, 16])
            self.assertEqual(edge_lines, [4, 6, 7, 8, 9, 22, 26])

    def test_rubylight_family_gating_rule(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            _write_configs(root, baselines={
                'missing_headers': {'rust': [], 'java_kt': []},
                'raw_config_read': {},
                'rubylight_family_gating': {
                    'allowed': {'client/protocol/desktop/src/host_page.rs': 1},
                    'legacy': {'client/protocol/client/src/http/launch.rs': 1},
                },
                'unannotated_cfg_unix': {},
            })
            _write_files(root, {
                'client/protocol/desktop/src/host_page.rs': '//! Label.\nfn lbl(f: Family) { match f { Family::Rubylight => {}, _ => {} } }\n',
                'client/protocol/client/src/http/launch.rs': '//! Legacy.\nfn l(f: Family) -> bool { f == Family::Rubylight }\n',
                'client/protocol/client/src/http/new_bad.rs': (
                    '//! New.\n'
                    '// Family::Rubylight in comment\n'
                    'const S: &str = "Family::Rubylight";\n'
                    'fn bad(b: u8, f: Family) -> bool {\n'
                    '    if b == b\'"\' { return f == Family::Rubylight; }\n'
                    '    false\n'
                    '}\n'
                    '#[cfg(test)] mod tests { fn ok(f: Family) { let _ = f == Family::Rubylight; } }\n'
                    '#[cfg(all(unix, test))] mod unix_tests { fn ok(f: Family) { let _ = f == Family::Rubylight; } }\n'
                ),
                'client/app/src/main/java/com/limelight/NewBad.java': (
                    'package com.limelight;\n'
                    '/** Bad Java gating. */\n'
                    'class NewBad {\n'
                    '    // Family.RUBYLIGHT\n'
                    '    String s = "case RUBYLIGHT:";\n'
                    '    boolean a(Family f) { return f == Family.RUBYLIGHT; }\n'
                    '    void b(Family f) { switch (f) { case RUBYLIGHT: break; default: break; } }\n'
                    '}\n'
                ),
                'client/app/src/test/java/com/limelight/OkTest.java': (
                    'package com.limelight;\n'
                    '/** Test allowed. */\n'
                    'class OkTest { boolean a(Family f) { return f == Family.RUBYLIGHT; } }\n'
                ),
            })
            violations = arch_check.check_repo(root)
            self.assertEqual(
                [(p, line, rule) for p, line, rule, _ in violations],
                [
                    ('client/app/src/main/java/com/limelight/NewBad.java', 6, 'rubylight-family-gating'),
                    ('client/app/src/main/java/com/limelight/NewBad.java', 7, 'rubylight-family-gating'),
                    ('client/protocol/client/src/http/new_bad.rs', 5, 'rubylight-family-gating'),
                ],
            )

    def test_unannotated_cfg_unix_rule(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            _write_configs(root, baselines={
                'missing_headers': {'rust': [], 'java_kt': []},
                'raw_config_read': {},
                'rubylight_family_gating': {'allowed': {}, 'legacy': {}},
                'unannotated_cfg_unix': {'client/protocol/desktop/src/baselined.rs': 1},
            })
            _write_files(root, {
                'client/protocol/desktop/src/baselined.rs': '//! Baselined.\n#[cfg(unix)]\nfn old() {}\n',
                'client/protocol/desktop/src/good.rs': (
                    '//! Good.\n'
                    '// Linux and macOS both expose fd here.\n'
                    '#[allow(\n'
                    '    dead_code\n'
                    ')]\n'
                    '#[cfg(unix)]\n'
                    'fn ok_attr() {}\n'
                    '\n'
                    '// Linux and macOS both need this.\n'
                    '/// Doc comment on fn.\n'
                    '#[cfg(unix)]\n'
                    'fn ok_doc_after_os_comment() {}\n'
                    '\n'
                    '// Linux and macOS both need this too.\n'
                    '#[allow(dead_code)]\n'
                    '/// Doc comment between attr and cfg.\n'
                    '#[cfg(unix)]\n'
                    'fn ok_attr_then_doc() {}\n'
                    '\n'
                    'fn ok_macro() -> bool {\n'
                    '    cfg!(unix) // macOS and Linux use POSIX sockets\n'
                    '}\n'
                    '#[cfg(target_os = "linux")]\n'
                    'fn ok_linux() {}\n'
                    'const X: &str = "#[cfg(unix)]";\n'
                ),
                'client/protocol/desktop/src/bad.rs': (
                    '//! Bad.\n'
                    '#[cfg(unix)]\n'
                    'fn missing_comment() {}\n'
                    '\n'
                    '/// Doc comment does not explain OS intent.\n'
                    '#[cfg(all(unix, not(target_os = "macos")))]\n'
                    'fn doc_comment_rejected() {}\n'
                    '\n'
                    '// TODO: clean up later\n'
                    '#[cfg_attr(unix, inline)]\n'
                    'fn no_os_keyword() {}\n'
                    '\n'
                    'fn parse_quote(b: u8) -> bool { b == b\'"\' }\n'
                    '#[cfg(unix)]\n'
                    'fn after_b_quote() {}\n'
                    '\n'
                    'fn bad_macro() -> bool {\n'
                    '    cfg!(unix)\n'
                    '}\n'
                ),
            })
            violations = arch_check.check_repo(root)
            self.assertEqual(
                [(p, line, rule) for p, line, rule, _ in violations],
                [
                    ('client/protocol/desktop/src/bad.rs', 2, 'unannotated-cfg-unix'),
                    ('client/protocol/desktop/src/bad.rs', 6, 'unannotated-cfg-unix'),
                    ('client/protocol/desktop/src/bad.rs', 10, 'unannotated-cfg-unix'),
                    ('client/protocol/desktop/src/bad.rs', 14, 'unannotated-cfg-unix'),
                    ('client/protocol/desktop/src/bad.rs', 18, 'unannotated-cfg-unix'),
                ],
            )


class CliAndDevIntegrationTests(unittest.TestCase):
    def test_cli_exit_codes_and_sorted_output(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            _write_configs(root)
            _write_files(root, {
                'src/z_bad.rs': 'pub fn z() {}\n',
                'src/a_bad.rs': 'pub fn a() {}\n',
            })
            out = io.StringIO()
            with contextlib.redirect_stdout(out):
                rc = arch_check.main(['--root', str(root)])
            self.assertEqual(rc, 1)
            lines = out.getvalue().splitlines()
            self.assertEqual(lines, [
                'src/a_bad.rs:1: missing-module-header: missing //! module header',
                'src/z_bad.rs:1: missing-module-header: missing //! module header',
                'ERROR: arch_check failed (2 violation(s))',
            ])

    def test_dev_plan_runs_arch_check_in_full_and_fast_modes_and_extracts_cpp_digits(self):
        for fast in (False, True):
            planned, _ = dev.plan(['rust/core/session/src/lib.rs'], ROOT, fast=fast)
            by_name = {c.name: c for c in planned}
            self.assertIn('arch-check', by_name)
            self.assertEqual(by_name['arch-check'].skip, '')

        planned_cpp, unchecked_cpp = dev.plan(['client/app/src/main/jni/vulkan/vulkan_renderer.cpp'], ROOT, fast=False)
        self.assertEqual([c.name for c in planned_cpp], ['arch-check'])
        self.assertEqual(unchecked_cpp, [])

        planned_docs, unchecked_docs = dev.plan(['docs/architecture.md', 'client/site/index.html'], ROOT, fast=False)
        self.assertEqual(planned_docs, [])
        self.assertEqual(len(unchecked_docs), 2)

        log_text = (
            'client/app/src/main/jni/pyrowave-renderer/pyrowave_renderer.cpp:1: raw-config-read: bad read\n'
            'client/app/src/main/jni/vulkan/frame_pacer.h:12: missing-module-header: bad\n'
            'tools/arch_baselines.json:5: unannotated-cfg-unix: invalid\n'
            'ERROR: arch_check failed (3 violation(s))\n'
        )
        extracted = dev.extract_failure_lines('arch', log_text)
        self.assertIn(
            'client/app/src/main/jni/pyrowave-renderer/pyrowave_renderer.cpp:1:1: raw-config-read: bad read',
            extracted,
        )
        self.assertIn(
            'client/app/src/main/jni/vulkan/frame_pacer.h:12:1: missing-module-header: bad',
            extracted,
        )
        self.assertIn('tools/arch_baselines.json:5:1: unannotated-cfg-unix: invalid', extracted)
        with tempfile.TemporaryDirectory() as tmp:
            log_path = pathlib.Path(tmp) / 'arch.log'
            log_path.write_text(log_text, encoding='utf-8')
            digest = dev.format_failure_digest([(dev.Check('arch-check', 'arch'), log_path)])
            self.assertLessEqual(len(digest), dev.DIGEST_MAX_LINES)
            self.assertTrue(any('pyrowave_renderer.cpp:1:1:' in line for line in digest))

    def test_rust_lexer_byte_char_literals_do_not_break_lifetimes_or_blind_tail(self):
        src = (
            'fn f<\'a, \'b>(x: &\'a [u8], y: &\'b str) -> bool {\n'
            '    let _ = x.first() == Some(&b\'"\');\n'
            '    let _ = x.last() == Some(&b\'\\\\\');\n'
            '    let _ = b\'\\\'\' == b\'\\x22\';\n'
            '    let _ = stringify!(foo_b\'a, foo\'a);\n'
            '    let _ = y == "ok";\n'
            '    true\n'
            '}\n'
        )
        stripped = arch_check.strip_rust_comments_and_strings(src)
        self.assertEqual(len(stripped), len(src))
        self.assertEqual(stripped.count('\n'), src.count('\n'))
        self.assertIn("b' '", stripped)
        self.assertIn("b'  '", stripped)
        self.assertIn('let _ = y == "  ";', stripped)
        self.assertIn("<'a, 'b>", stripped)

    def test_live_repository_passes_arch_check(self):
        self.assertEqual(arch_check.check_repo(ROOT), [])


if __name__ == '__main__':
    unittest.main()
